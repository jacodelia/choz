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
use std::os::raw::{c_char, c_void};
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
    bag.props.push(Property {
        key,
        type_uri,
        flags,
        value: bytes,
    });
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
    let Some(key_uri) = uri_of(&bag.urids, key) else {
        return std::ptr::null();
    };
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

// ─── Paths (state:mapPath / state:freePath) ─────────────────────────────────
//
// A plugin that holds file paths — a sampler, a convolver, an IR loader — is
// required to hand them through `abstract_path` before storing them, and a good
// many refuse to save anything at all when the feature is missing. Both
// directions return a string **the plugin owns**: it frees it with `free()`
// unless the host also offers `state:freePath`, so the strings have to come
// from `malloc` and not from Rust's allocator.
//
// ponytail: the mapping is the identity — what is stored is the absolute path.
// Real hosts copy the file into the project's own directory so the project is
// self-contained; choz's project is a single YAML file with nowhere to put a
// 300 MB sample library, and a path that still works on this machine beats a
// state the plugin refused to save. The day projects become directories, this
// is the one place that has to change.

/// A copy of `s` the plugin can hold and `free()`. Null if the string cannot be
/// a C string at all, which the callers treat as "no path".
fn dup_c(s: &CStr) -> *mut c_char {
    let bytes = s.to_bytes_with_nul();
    // SAFETY: a `malloc` of the exact length, then filled with exactly that
    // many bytes, terminator included.
    unsafe {
        let out = libc::malloc(bytes.len()) as *mut c_char;
        if out.is_null() {
            return std::ptr::null_mut();
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), out as *mut u8, bytes.len());
        out
    }
}

unsafe extern "C" fn abstract_path_cb(_handle: *mut c_void, path: *const c_char) -> *mut c_char {
    if path.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: the plugin passes a C string it owns for the duration of the call.
    dup_c(unsafe { CStr::from_ptr(path) })
}

unsafe extern "C" fn absolute_path_cb(_handle: *mut c_void, path: *const c_char) -> *mut c_char {
    if path.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: same contract as above.
    dup_c(unsafe { CStr::from_ptr(path) })
}

unsafe extern "C" fn free_path_cb(_handle: *mut c_void, path: *mut c_char) {
    if !path.is_null() {
        // SAFETY: every string this host hands out comes from `dup_c`'s malloc.
        unsafe { libc::free(path as *mut c_void) };
    }
}

/// The two path features, alive for one `save` or `restore` call.
///
/// Everything the plugin sees points into a heap allocation — the boxes and the
/// `CString`s — so moving this struct does not move anything under it.
struct PathFeatures {
    features: [LV2_Feature; 2],
    _map: Box<LV2_State_Map_Path>,
    _free: Box<LV2_State_Free_Path>,
    _uris: [CString; 2],
}

impl PathFeatures {
    fn new() -> Option<Self> {
        let map_uri = CString::new(LV2_STATE_MAP_PATH_URI).ok()?;
        let free_uri = CString::new(LV2_STATE_FREE_PATH_URI).ok()?;
        let map = Box::new(LV2_State_Map_Path {
            handle: std::ptr::null_mut(),
            abstract_path: Some(abstract_path_cb),
            absolute_path: Some(absolute_path_cb),
        });
        let free = Box::new(LV2_State_Free_Path {
            handle: std::ptr::null_mut(),
            free_path: Some(free_path_cb),
        });
        let features = [
            LV2_Feature {
                uri: map_uri.as_ptr(),
                data: &*map as *const _ as *mut c_void,
            },
            LV2_Feature {
                uri: free_uri.as_ptr(),
                data: &*free as *const _ as *mut c_void,
            },
        ];
        Some(Self {
            features,
            _map: map,
            _free: free,
            _uris: [map_uri, free_uri],
        })
    }

    /// The null-terminated list `save`/`restore` take.
    fn list(&self) -> [*const LV2_Feature; 3] {
        [&self.features[0], &self.features[1], std::ptr::null()]
    }
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

        let mut bag = Bag {
            props: Vec::new(),
            urids: Arc::clone(&cell.urids),
            lent: Vec::new(),
        };
        // Without these a sampler stores nothing: the paths are half its state.
        let paths = PathFeatures::new();
        let features = paths
            .as_ref()
            .map(|p| p.list())
            .unwrap_or([std::ptr::null(); 3]);
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
        restore_props(&self.shared, props);
    }
}

/// A preset's `state:state`, as the Turtle spells it: `(key URI, type URI,
/// bytes)`. The flags are the host's business, and `POD` is what a value read
/// out of a file is.
pub(crate) fn restore_state(shared: &SharedState, props: &[(String, String, Vec<u8>)]) {
    restore_props(
        shared,
        props
            .iter()
            .map(|(key, type_uri, value)| Property {
                key: key.clone(),
                type_uri: type_uri.clone(),
                // `state:Pod` (1) | `state:Portable` (2): a value out of a file is both.
                flags: 3,
                value: value.clone(),
            })
            .collect(),
    );
}

/// Hand a set of properties to the instance's `restore`.
fn restore_props(shared: &SharedState, props: Vec<Property>) {
    {
        if props.is_empty() {
            return;
        }
        let guard = shared.lock();
        let Some(cell) = guard.as_ref() else { return };
        // SAFETY: live instance under the mutex.
        let Some(iface) = (unsafe { interface(cell.descriptor) }) else {
            return;
        };
        let Some(restore) = (unsafe { (*iface).restore }) else {
            return;
        };

        let mut bag = Bag {
            props,
            urids: Arc::clone(&cell.urids),
            lent: Vec::new(),
        };
        let paths = PathFeatures::new();
        let features = paths
            .as_ref()
            .map(|p| p.list())
            .unwrap_or([std::ptr::null(); 3]);
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
        out.push(Property {
            key,
            type_uri,
            flags,
            value,
        });
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
        Property {
            key: key.into(),
            type_uri: ty.into(),
            flags: 3,
            value: value.to_vec(),
        }
    }

    /// The strings the two path functions return belong to the plugin, which
    /// frees them with `free()` — so they have to be `malloc`ed copies, never a
    /// borrow of what was passed in and never a Rust allocation.
    #[test]
    fn a_mapped_path_is_the_plugins_own_malloc_copy() {
        let src = CString::new("/home/user/samples/kick.wav").unwrap();
        let null = std::ptr::null_mut();

        let stored = unsafe { abstract_path_cb(null, src.as_ptr()) };
        assert!(!stored.is_null());
        assert_ne!(stored.cast_const(), src.as_ptr(), "the plugin must own it");
        assert_eq!(
            unsafe { CStr::from_ptr(stored) },
            src.as_c_str(),
            "stored as-is"
        );

        let back = unsafe { absolute_path_cb(null, stored) };
        assert_eq!(
            unsafe { CStr::from_ptr(back) },
            src.as_c_str(),
            "and read back as-is"
        );

        // Freed the way the plugin would, whichever of the two ways it uses.
        unsafe { free_path_cb(null, back) };
        unsafe { libc::free(stored as *mut c_void) };

        // A plugin asking about nothing gets nothing, and freeing it is a no-op.
        assert!(unsafe { abstract_path_cb(null, std::ptr::null()) }.is_null());
        unsafe { free_path_cb(null, std::ptr::null_mut()) };
    }

    /// `save`/`restore` take a NULL-terminated array, and a plugin that walks
    /// past the end of it reads whatever follows.
    #[test]
    fn the_path_features_are_offered_as_a_terminated_list() {
        let paths = PathFeatures::new().expect("the two URIs are literals");
        let list = paths.list();
        assert!(list[2].is_null(), "terminated");
        let uri = |f: *const LV2_Feature| {
            unsafe { CStr::from_ptr((*f).uri) }
                .to_string_lossy()
                .into_owned()
        };
        assert_eq!(uri(list[0]), LV2_STATE_MAP_PATH_URI);
        assert_eq!(uri(list[1]), LV2_STATE_FREE_PATH_URI);
        // The data pointers survive the move out of `new`.
        let map = unsafe { &*((*list[0]).data as *const LV2_State_Map_Path) };
        assert!(map.abstract_path.is_some() && map.absolute_path.is_some());
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
