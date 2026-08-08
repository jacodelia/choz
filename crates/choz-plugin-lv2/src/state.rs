//! The plugin's own state, through `state#interface`.
//!
//! LV2 does not hand over a blob: the plugin calls back once per property with
//! a **URID** for the key and another for the type, and the host stores them.
//! URIDs are only meaningful inside one process — the numbers are handed out by
//! this host's own map — so a project cannot store them. What is stored are the
//! **URIs** they stand for, resolved through the same store that minted them,
//! and mapped back to fresh numbers when the state is restored.
//!
//! The instance lives on the audio thread by the time anyone saves, which is
//! the same bargain the editor makes: the calls are serialised by the cell's
//! mutex, and a plugin that insists on `state:threadSafeRestore` is asking for
//! a handshake choz does not implement — it will simply be restored between
//! blocks like everyone else.

use std::ffi::{CStr, CString};
use std::os::raw::c_void;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::lv2_abi::*;

/// The live plugin plus what is needed to turn URIDs into URIs, shared with the
/// UI thread. `None` once the instance is gone.
pub type SharedState = Arc<Mutex<Option<StateCell>>>;

pub struct StateCell {
    pub(crate) handle: LV2_Handle,
    pub(crate) descriptor: *const LV2_Descriptor,
    /// The map/unmap store this instance was instantiated with.
    pub(crate) urids: Arc<Mutex<crate::UridStore>>,
    /// Keeps the binary mapped while the UI thread can still call in.
    pub(crate) _lib: Arc<libloading::Library>,
}

// SAFETY: every pointer is used only under the cell's mutex, and only while the
// instance is alive — which is what `Some` marks.
unsafe impl Send for StateCell {}

/// One stored property, in the only form that survives a file: URIs, not URIDs.
struct Property {
    key: String,
    type_uri: String,
    flags: u32,
    value: Vec<u8>,
}

/// Collected during `save`, or fed back during `restore`.
struct Bag {
    props: Vec<Property>,
    urids: Arc<Mutex<crate::UridStore>>,
    /// Values handed back to the plugin during `restore`. They must stay put
    /// for the whole call, so they are kept here rather than rebuilt per
    /// lookup.
    lent: Vec<Vec<u8>>,
}

unsafe extern "C" fn store_cb(
    handle: LV2_State_Handle,
    key: u32,
    value: *const c_void,
    size: usize,
    type_: u32,
    flags: u32,
) -> i32 {
    if handle.is_null() || value.is_null() {
        return 1;
    }
    // SAFETY: the handle is the `Bag` this host passed to `save`.
    let bag = unsafe { &mut *(handle as *mut Bag) };
    let (Some(key), Some(type_uri)) = (uri_of(&bag.urids, key), uri_of(&bag.urids, type_)) else {
        // A URID this host never minted cannot be written down.
        return 1;
    };
    // SAFETY: the plugin promised `size` readable bytes, and they are copied
    // before the call returns.
    let bytes = unsafe { std::slice::from_raw_parts(value as *const u8, size) }.to_vec();
    bag.props.push(Property { key, type_uri, flags, value: bytes });
    LV2_STATE_SUCCESS
}

unsafe extern "C" fn retrieve_cb(
    handle: LV2_State_Handle,
    key: u32,
    size: *mut usize,
    type_: *mut u32,
    flags: *mut u32,
) -> *const c_void {
    if handle.is_null() {
        return std::ptr::null();
    }
    // SAFETY: the handle is the `Bag` this host passed to `restore`.
    let bag = unsafe { &mut *(handle as *mut Bag) };
    let Some(key_uri) = uri_of(&bag.urids, key) else { return std::ptr::null() };
    let Some(prop) = bag.props.iter().find(|p| p.key == key_uri) else {
        return std::ptr::null();
    };
    let type_urid = bag.urids.lock().intern(&prop.type_uri);
    // The pointer has to outlive this call, so the copy is parked in the bag.
    bag.lent.push(prop.value.clone());
    let lent = bag.lent.last().expect("just pushed");
    unsafe {
        if !size.is_null() {
            *size = lent.len();
        }
        if !type_.is_null() {
            *type_ = type_urid;
        }
        if !flags.is_null() {
            *flags = prop.flags;
        }
    }
    lent.as_ptr() as *const c_void
}

fn uri_of(store: &Arc<Mutex<crate::UridStore>>, urid: u32) -> Option<String> {
    store.lock().uri(urid)
}

/// `extension_data(state#interface)` of a live plugin.
///
/// # Safety
/// `descriptor` must be the live descriptor of the instance in `cell`.
unsafe fn interface(descriptor: *const LV2_Descriptor) -> Option<*const LV2_State_Interface> {
    let f = unsafe { (*descriptor).extension_data }?;
    let uri = CString::new(LV2_STATE_INTERFACE_URI).ok()?;
    let ptr = unsafe { f(uri.as_ptr()) } as *const LV2_State_Interface;
    (!ptr.is_null()).then_some(ptr)
}

pub struct Lv2State {
    pub shared: SharedState,
}

impl choz_ports::PluginState for Lv2State {
    fn save(&self) -> Option<Vec<u8>> {
        let guard = self.shared.lock();
        let cell = guard.as_ref()?;
        // SAFETY: live instance under the mutex.
        let iface = unsafe { interface(cell.descriptor) }?;
        let save = unsafe { (*iface).save }?;

        let mut bag = Bag { props: Vec::new(), urids: Arc::clone(&cell.urids), lent: Vec::new() };
        let features: [*const LV2_Feature; 1] = [std::ptr::null()];
        // SAFETY: the plugin calls `store_cb` with our bag for the duration of
        // this call and not after it.
        let status = unsafe {
            save(
                cell.handle,
                store_cb,
                &mut bag as *mut Bag as LV2_State_Handle,
                0,
                features.as_ptr(),
            )
        };
        if status != LV2_STATE_SUCCESS || bag.props.is_empty() {
            return None;
        }
        Some(encode(&bag.props))
    }

    fn restore(&self, data: &[u8]) {
        let Some(props) = decode(data) else { return };
        if props.is_empty() {
            return;
        }
        let guard = self.shared.lock();
        let Some(cell) = guard.as_ref() else { return };
        // SAFETY: live instance under the mutex.
        let Some(iface) = (unsafe { interface(cell.descriptor) }) else { return };
        let Some(restore) = (unsafe { (*iface).restore }) else { return };

        let mut bag = Bag { props, urids: Arc::clone(&cell.urids), lent: Vec::new() };
        let features: [*const LV2_Feature; 1] = [std::ptr::null()];
        unsafe {
            restore(
                cell.handle,
                retrieve_cb,
                &mut bag as *mut Bag as LV2_State_Handle,
                0,
                features.as_ptr(),
            );
        }
    }
}

// ─── The blob ───────────────────────────────────────────────────────────────
//
// A flat, self-describing layout: nothing about it depends on this process, so
// a project written here loads anywhere the same plugin is installed.
//
//   [u32 count] then per property:
//     [u32 key len][key][u32 type len][type][u32 flags][u32 value len][value]

fn put(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
}

fn encode(props: &[Property]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(props.len() as u32).to_le_bytes());
    for p in props {
        put(&mut out, p.key.as_bytes());
        put(&mut out, p.type_uri.as_bytes());
        out.extend_from_slice(&p.flags.to_le_bytes());
        put(&mut out, &p.value);
    }
    out
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn u32(&mut self) -> Option<u32> {
        let end = self.pos.checked_add(4)?;
        let v = u32::from_le_bytes(self.data.get(self.pos..end)?.try_into().ok()?);
        self.pos = end;
        Some(v)
    }

    fn bytes(&mut self) -> Option<&'a [u8]> {
        let len = self.u32()? as usize;
        let end = self.pos.checked_add(len)?;
        let out = self.data.get(self.pos..end)?;
        self.pos = end;
        Some(out)
    }
}

/// Parse a blob. `None` for anything malformed — a truncated or foreign file
/// must leave the plugin alone rather than feed it nonsense.
fn decode(data: &[u8]) -> Option<Vec<Property>> {
    let mut r = Reader { data, pos: 0 };
    let count = r.u32()? as usize;
    let mut out = Vec::with_capacity(count.min(1024));
    for _ in 0..count {
        let key = String::from_utf8(r.bytes()?.to_vec()).ok()?;
        let type_uri = String::from_utf8(r.bytes()?.to_vec()).ok()?;
        let flags = r.u32()?;
        let value = r.bytes()?.to_vec();
        out.push(Property { key, type_uri, flags, value });
    }
    Some(out)
}

/// The URI a `CStr` key stands for, for tests and diagnostics.
#[allow(dead_code)]
fn uri_str(c: &CStr) -> String {
    c.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prop(key: &str, ty: &str, value: &[u8]) -> Property {
        Property { key: key.into(), type_uri: ty.into(), flags: 3, value: value.to_vec() }
    }

    /// The blob is what ends up in the project file, so it has to survive the
    /// round trip exactly — and reject anything it cannot read instead of
    /// handing a plugin half a property.
    #[test]
    fn properties_round_trip_and_rubbish_is_refused() {
        let props = vec![
            prop("urn:choz:patch", "urn:choz:string", b"Bright Pad"),
            prop("urn:choz:blob", "urn:choz:chunk", &[0u8, 255, 7, 7]),
        ];
        let blob = encode(&props);
        let back = decode(&blob).expect("its own output");
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].key, "urn:choz:patch");
        assert_eq!(back[0].value, b"Bright Pad");
        assert_eq!(back[1].type_uri, "urn:choz:chunk");
        assert_eq!(back[1].flags, 3);

        assert!(decode(&blob[..blob.len() - 3]).is_none(), "truncated");
        assert!(decode(&[]).is_none(), "empty");
        // A count that promises more than the bytes hold must not allocate it.
        assert!(decode(&u32::MAX.to_le_bytes()).is_none());
    }
}
