//! The programs a plugin publishes through the kx `programs#Interface`.
//!
//! Some instruments keep their patches inside themselves and describe none of
//! them in Turtle: Yoshimi's 4466 instruments live in its own bank files, and
//! its bundle declares exactly one control port. What it *does* declare is
//! `<http://kxstudio.sf.net/ns/lv2ext/programs#Interface>` — the extension every
//! host that shows those banks uses, and the only door to them.
//!
//! Two calls: `get_program(index)` walks the list until it answers null, and
//! `select_program(bank, program)` puts one on the instrument. Both go through
//! the instance, so both are made under the same mutex `state` uses.

use std::ffi::{CStr, CString};
use std::os::raw::c_char;

use choz_ports::PresetEntry;

use crate::lv2_abi::{LV2_Descriptor, LV2_Handle};
use crate::state::SharedState;

pub const LV2_PROGRAMS_INTERFACE_URI: &str = "http://kxstudio.sf.net/ns/lv2ext/programs#Interface";

/// One entry of the plugin's own list.
#[repr(C)]
pub struct LV2_Program_Descriptor {
    pub bank: u32,
    pub program: u32,
    pub name: *const c_char,
}

/// What `extension_data(programs#Interface)` returns.
#[repr(C)]
pub struct LV2_Programs_Interface {
    pub get_program: Option<unsafe extern "C" fn(LV2_Handle, u32) -> *const LV2_Program_Descriptor>,
    pub select_program: Option<unsafe extern "C" fn(LV2_Handle, u32, u32)>,
}

/// A plugin with thousands of programs is still a list somebody has to walk;
/// this is only a guard against a plugin that never answers null.
const MAX_PROGRAMS: u32 = 20_000;

/// Read the plugin's program list. Empty when it has no such interface — which
/// is most plugins.
///
/// Calls into the instance: UI thread, under the state mutex.
pub fn scan(shared: &SharedState) -> Vec<PresetEntry> {
    let guard = shared.lock();
    let Some(cell) = guard.as_ref() else {
        return Vec::new();
    };
    // SAFETY: a live instance under the mutex.
    let Some(iface) = (unsafe { interface(cell.descriptor) }) else {
        return Vec::new();
    };
    let Some(get) = (unsafe { (*iface).get_program }) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for i in 0..MAX_PROGRAMS {
        // SAFETY: the plugin owns the descriptor and its name; both are read
        // before the next call, which is what the extension promises.
        let d = unsafe { get(cell.handle, i) };
        if d.is_null() {
            break;
        }
        let (bank, program, name) = unsafe {
            let name = match (*d).name.is_null() {
                true => String::new(),
                false => CStr::from_ptr((*d).name).to_string_lossy().into_owned(),
            };
            ((*d).bank, (*d).program, name)
        };
        // The extension numbers banks and does not name them, but a plugin
        // with 112 of them usually says which is which in the name itself:
        // Yoshimi's read "Arpeggios -> Arpeggio1". Split there and the bank is
        // the sidebar, the instrument is the row.
        let (category, name) = match name.split_once(" -> ") {
            Some((bank, patch)) if !bank.is_empty() && !patch.is_empty() => {
                (bank.trim().to_string(), patch.trim().to_string())
            }
            _ => (format!("BANK {bank}"), name),
        };
        out.push(PresetEntry {
            name: match name.trim().is_empty() {
                true => format!("{bank}:{program}"),
                false => name,
            },
            category,
            key: format!("{bank}:{program}"),
        });
    }
    out
}

/// The list plus the instance to select one on.
pub struct Lv2Programs {
    pub shared: SharedState,
    pub list: Vec<PresetEntry>,
}

impl choz_ports::PluginPresets for Lv2Programs {
    fn list(&self) -> Vec<PresetEntry> {
        self.list.clone()
    }

    fn load(&self, key: &str) {
        let Some((bank, program)) = key
            .split_once(':')
            .and_then(|(b, p)| Some((b.parse::<u32>().ok()?, p.parse::<u32>().ok()?)))
        else {
            return;
        };
        let guard = self.shared.lock();
        let Some(cell) = guard.as_ref() else { return };
        // SAFETY: a live instance under the mutex, and the same call the
        // plugin's own program-change handling makes.
        unsafe {
            if let Some(select) = interface(cell.descriptor).and_then(|i| (*i).select_program) {
                select(cell.handle, bank, program);
            }
        }
    }
}

/// `extension_data(programs#Interface)`, or `None`.
///
/// # Safety
/// `descriptor` must be a live descriptor from this plugin's library.
unsafe fn interface(descriptor: *const LV2_Descriptor) -> Option<*const LV2_Programs_Interface> {
    let f = unsafe { (*descriptor).extension_data }?;
    let uri = CString::new(LV2_PROGRAMS_INTERFACE_URI).ok()?;
    let ptr = unsafe { f(uri.as_ptr()) } as *const LV2_Programs_Interface;
    (!ptr.is_null()).then_some(ptr)
}
