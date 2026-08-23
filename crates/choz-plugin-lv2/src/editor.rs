//! The plugin's own X11 window (`ui:X11UI`), without suil.
//!
//! An LV2 UI is a *separate* shared object from the DSP one, and it never
//! touches the plugin instance: it talks to the host through a write callback,
//! and the host is what moves the value onto the port. That is why this works
//! while the instance lives on the audio thread — the editor only ever writes
//! into the control-value array, one `f32` per port, which the RT side reads.
//!
//! Two kinds are driven: `ui:X11UI`, which embeds into the window choz creates,
//! and `ui:showInterface`, where the UI puts up a window of its own and the host
//! only calls `show()` and `idle()` — that is how Yoshimi's and ZynAddSubFX's
//! editors appear (in Carla too; they have no X11UI at all). A Gtk or Qt UI with
//! neither needs its toolkit's main loop running in the host, which is exactly
//! the job suil does and choz does not.

use std::ffi::{CStr, CString};
use std::os::raw::c_void;
use std::sync::Arc;

use parking_lot::Mutex;

use choz_ports::PluginEditor;
use libloading::Library;

use crate::discovery::Lv2UiInfo;
use crate::lv2_abi::*;

/// The plugin's control-value array, shared with the GUI thread.
///
/// `None` once the owning instance is gone, so a window that outlives its slot
/// turns into a no-op instead of writing through a dangling pointer — same
/// contract as the VST2 editor.
pub type SharedControls = Arc<Mutex<Option<ControlsCell>>>;

pub struct ControlsCell {
    /// Base of `Lv2Instance::control_values`; the plugin holds pointers into it
    /// via `connect_port`, so the buffer never moves for the instance's life.
    pub values: *mut f32,
    pub len: usize,
}

// SAFETY: the pointer is only dereferenced under the mutex. Writing an f32 that
// the audio thread reads is the same racy-but-benign store every LV2 host does
// for control ports — the port protocol is "latest value wins".
unsafe impl Send for ControlsCell {}

/// A live UI instance plus everything that has to outlive it.
struct UiInstance {
    descriptor: *const LV2UI_Descriptor,
    handle: LV2UI_Handle,
    idle: Option<*const LV2UI_Idle_Interface>,
    /// `show`/`hide`, for a UI that owns its window.
    show: Option<*const LV2UI_Show_Interface>,
    /// Boxed so the pointer handed to the UI as `ui:parent` stays put.
    _features: Box<UiFeatures>,
}

/// Feature array for the UI: `ui:parent` (the X11 window), `urid:map`, and
/// `ui:idleInterface`.
///
/// That last one is both a feature and an extension: a UI that lists it under
/// `requiredFeature` (guitarix does) checks for it in the array, and gets a
/// null dereference — a segfault, not a polite null return — when it is absent.
struct UiFeatures {
    _uris: Vec<CString>,
    _map: Box<LV2_URID_Map>,
    /// Sample rate, in a box the options array points at.
    _sample_rate: Box<f32>,
    _options: Vec<LV2_Options_Option>,
    _feats: Vec<LV2_Feature>,
    ptrs: Vec<*const LV2_Feature>,
}

pub struct Lv2Editor {
    info: Lv2UiInfo,
    plugin_uri: CString,
    bundle_path: CString,
    controls: SharedControls,
    /// The last control port the UI wrote, and the value it wrote — the plain
    /// one, in the port's own units. Read by MIDI learn and by the UI keeping
    /// its knobs in step with the plugin's window.
    touched: Arc<Mutex<Option<(u32, f32)>>>,
    sample_rate: u32,
    /// The live plugin instance, for `instance-access` — passed straight back to
    /// a UI that requires it and never dereferenced here.
    instance: usize,
    /// Cleared when a UI that owns its window says it has been closed, which is
    /// the only way the host can find that out: there is no window of ours for
    /// the window manager to tell about it.
    open: std::sync::atomic::AtomicBool,
    /// `None` while closed. The mutex serialises open/idle/close, all of which
    /// arrive on the editor thread, and makes a double `close()` harmless.
    ui: Mutex<Option<UiInstance>>,
    /// Kept mapped for the process's life, like the DSP libraries: a UI can
    /// leave threads or atexit handlers behind, and unmapping under them
    /// crashes inside the loader.
    _lib: Arc<Library>,
}

// SAFETY: every raw pointer is reached only under `ui`'s mutex.
unsafe impl Send for Lv2Editor {}
unsafe impl Sync for Lv2Editor {}

impl Lv2Editor {
    /// The last control port the UI wrote, and its value in the port's own
    /// units. Handed to the instrument/effect, which is where the port index
    /// can be turned into the parameter index choz addresses knobs by.
    pub fn touched(&self) -> Arc<Mutex<Option<(u32, f32)>>> {
        Arc::clone(&self.touched)
    }

    /// Load the UI binary and get it ready to open. `None` if the library or
    /// its descriptor is not usable — the caller then reports no editor at all,
    /// so the button never offers a window that cannot appear.
    pub fn load(
        info: &Lv2UiInfo,
        plugin_uri: &str,
        bundle_dir: &std::path::Path,
        controls: SharedControls,
        sample_rate: u32,
        instance: LV2_Handle,
    ) -> Option<Arc<Self>> {
        let lib = Arc::new(unsafe { Library::new(&info.binary_path) }.ok()?);
        crate::keep_loaded(&lib);
        // Check the descriptor exists now rather than on the click.
        descriptor_for(&lib, &info.uri)?;

        // LV2 wants the bundle path with its trailing slash.
        let mut bundle = bundle_dir.to_string_lossy().into_owned();
        if !bundle.ends_with('/') {
            bundle.push('/');
        }
        Some(Arc::new(Self {
            info: info.clone(),
            plugin_uri: CString::new(plugin_uri).ok()?,
            bundle_path: CString::new(bundle).ok()?,
            controls,
            touched: Arc::default(),
            sample_rate,
            instance: instance as usize,
            open: std::sync::atomic::AtomicBool::new(false),
            ui: Mutex::new(None),
            _lib: lib,
        }))
    }
}

/// Walk `lv2ui_descriptor(0..)` for the one whose URI matches.
///
/// The walk ends where the plugin says it does — at the first null. The cap is
/// only there so a broken binary that never returns null cannot hang choz, and
/// it has to be generous: LSP ships **one** UI binary for its ~390 plugins, so
/// an earlier limit of 64 silently denied an editor to everything past the 64th
/// descriptor. That was 135 plugins reported as "no editor" in the sweep.
const MAX_UI_DESCRIPTORS: u32 = 4096;

fn descriptor_for(lib: &Library, uri: &str) -> Option<*const LV2UI_Descriptor> {
    let entry: libloading::Symbol<Lv2UiDescriptorFn> =
        unsafe { lib.get(LV2UI_DESCRIPTOR_SYM) }.ok()?;
    for i in 0..MAX_UI_DESCRIPTORS {
        let d = unsafe { entry(i) };
        if d.is_null() {
            return None;
        }
        let this = unsafe { CStr::from_ptr((*d).uri) }.to_string_lossy();
        if this == uri {
            return Some(d);
        }
    }
    None
}

/// What the UI calls to move a control. The controller is the `Lv2Editor`, so
/// the write lands in the same array the audio thread reads.
///
/// Only the default (float) port protocol is handled: `format == 0`. Anything
/// else is an atom-based protocol for a port choz does not expose as a knob.
unsafe extern "C" fn write_control(
    controller: LV2UI_Controller,
    port_index: u32,
    buffer_size: u32,
    format: u32,
    buffer: *const c_void,
) {
    if controller.is_null() || buffer.is_null() || format != 0 || buffer_size as usize != 4 {
        return;
    }
    // SAFETY: the controller is the &Lv2Editor passed to `instantiate`, which
    // outlives the UI instance (it owns it).
    let editor = unsafe { &*(controller as *const Lv2Editor) };
    let value = unsafe { std::ptr::read_unaligned(buffer as *const f32) };
    if !value.is_finite() {
        return;
    }
    let guard = editor.controls.lock();
    let Some(cell) = guard.as_ref() else { return };
    if (port_index as usize) < cell.len {
        // SAFETY: index checked against the array the cell describes.
        unsafe { cell.values.add(port_index as usize).write(value) };
        *editor.touched.lock() = Some((port_index, value));
    }
}

impl UiFeatures {
    fn new(parent: u64, map: &LV2_URID_Map, sample_rate: u32, instance: usize) -> Box<Self> {
        let uris: Vec<CString> = [
            LV2_UI_PARENT_URI,
            LV2_URID_MAP_URI,
            LV2_UI_IDLE_INTERFACE_URI,
            LV2_OPTIONS_URI,
            LV2_UI_SHOW_INTERFACE_URI,
            LV2_INSTANCE_ACCESS_URI,
        ]
        .iter()
        .map(|u| CString::new(*u).expect("static URI"))
        .collect();
        // The map struct is copied so the UI's feature array owns its own.
        let map = Box::new(LV2_URID_Map {
            handle: map.handle,
            map: map.map,
        });

        // DPF UIs (Zam, Dragonfly) list `opts:options` as required and read the
        // sample rate out of it. Interning goes through the shared UI store, so
        // the URIDs match the ones the UI will map for itself.
        let intern = |uri: &str| {
            let c = CString::new(uri).expect("static URI");
            map.map.map_or(0, |f| unsafe { f(map.handle, c.as_ptr()) })
        };
        let rate_key = intern(LV2_PARAM_SAMPLE_RATE_URI);
        let float_urid = intern(LV2_ATOM_FLOAT_URI);
        let sample_rate = Box::new(sample_rate as f32);

        let options = vec![
            LV2_Options_Option {
                context: LV2_OPTIONS_INSTANCE,
                subject: 0,
                key: rate_key,
                size: 4,
                type_: float_urid,
                value: &*sample_rate as *const f32 as *const c_void,
            },
            // Terminator.
            LV2_Options_Option {
                context: 0,
                subject: 0,
                key: 0,
                size: 0,
                type_: 0,
                value: std::ptr::null(),
            },
        ];

        let mut me = Box::new(Self {
            _uris: uris,
            _map: map,
            _sample_rate: sample_rate,
            _options: options,
            _feats: Vec::new(),
            ptrs: Vec::new(),
        });
        // `ui:parent`'s data *is* the window id, not a pointer to it.
        me._feats = vec![
            LV2_Feature {
                uri: me._uris[0].as_ptr(),
                data: parent as usize as *mut c_void,
            },
            LV2_Feature {
                uri: me._uris[1].as_ptr(),
                data: &*me._map as *const LV2_URID_Map as *mut c_void,
            },
            // Presence is the whole signal; this feature carries no data.
            LV2_Feature {
                uri: me._uris[2].as_ptr(),
                data: std::ptr::null_mut(),
            },
            LV2_Feature {
                uri: me._uris[3].as_ptr(),
                data: me._options.as_ptr() as *mut c_void,
            },
            // Presence again; a UI that shows its own window lists this one.
            LV2_Feature {
                uri: me._uris[4].as_ptr(),
                data: std::ptr::null_mut(),
            },
            // `instance-access`'s data *is* the instance handle, like the
            // parent's is the window id.
            LV2_Feature {
                uri: me._uris[5].as_ptr(),
                data: instance as *mut c_void,
            },
        ];
        me.ptrs = me._feats.iter().map(|f| f as *const LV2_Feature).collect();
        me.ptrs.push(std::ptr::null());
        me
    }
}

impl PluginEditor for Lv2Editor {
    fn open(&self, parent: u64) -> Option<(u16, u16)> {
        let mut guard = self.ui.lock();
        if guard.is_some() {
            return None; // already open
        }
        let descriptor = descriptor_for(&self._lib, &self.info.uri)?;
        let instantiate = unsafe { (*descriptor).instantiate }?;

        let map = crate::shared_urid_map();
        // A UI that owns its window has nothing to embed into; handing it a
        // parent is how one ends up drawing into a window nobody mapped.
        let parent = if self.info.owns_window { 0 } else { parent };
        let features = UiFeatures::new(parent, &map, self.sample_rate, self.instance);
        let mut widget: LV2UI_Widget = std::ptr::null_mut();

        // SAFETY: every pointer is owned by `features`/`self` and outlives the
        // call; `widget` is written by the UI.
        let handle = unsafe {
            instantiate(
                descriptor,
                self.plugin_uri.as_ptr(),
                self.bundle_path.as_ptr(),
                Some(write_control),
                self as *const Lv2Editor as LV2UI_Controller,
                &mut widget,
                features.ptrs.as_ptr(),
            )
        };
        if handle.is_null() {
            eprintln!("choz: LV2 UI {} refused to instantiate", self.info.uri);
            return None;
        }

        let idle = unsafe { (*descriptor).extension_data }.and_then(|ext| {
            let uri = CString::new(LV2_UI_IDLE_INTERFACE_URI).ok()?;
            let p = unsafe { ext(uri.as_ptr()) } as *const LV2UI_Idle_Interface;
            (!p.is_null()).then_some(p)
        });
        let show = unsafe { (*descriptor).extension_data }.and_then(|ext| {
            let uri = CString::new(LV2_UI_SHOW_INTERFACE_URI).ok()?;
            let p = unsafe { ext(uri.as_ptr()) } as *const LV2UI_Show_Interface;
            (!p.is_null()).then_some(p)
        });

        // Nothing is on screen until it is asked for: this is the whole of the
        // show interface, and without it Yoshimi and ZynAddSubFX instantiate
        // their editor and never draw it.
        if self.info.owns_window {
            let shown = show
                .and_then(|s| unsafe { (*s).show })
                .map(|f| unsafe { f(handle) } == 0)
                .unwrap_or(false);
            if !shown {
                eprintln!("choz: LV2 UI {} would not show its window", self.info.uri);
                if let Some(cleanup) = unsafe { (*descriptor).cleanup } {
                    unsafe { cleanup(handle) };
                }
                return None;
            }
        }
        self.open.store(true, std::sync::atomic::Ordering::Relaxed);

        *guard = Some(UiInstance {
            descriptor,
            handle,
            idle,
            show,
            _features: features,
        });
        // An X11UI parents itself into the window we gave it and reports no size
        // of its own; the editor thread keeps its default and the plugin resizes
        // through the window manager if it wants to.
        None
    }

    fn idle(&self) {
        let guard = self.ui.lock();
        let Some(ui) = guard.as_ref() else { return };
        let Some(idle) = ui.idle else { return };
        // SAFETY: the interface belongs to the descriptor, alive with the UI.
        let asked_to_close = unsafe {
            match (*idle).idle {
                Some(f) => f(ui.handle) != 0,
                None => false,
            }
        };
        // Non-zero means the UI wants to be closed — a window of its own has no
        // other way of saying the user shut it.
        if asked_to_close {
            self.open.store(false, std::sync::atomic::Ordering::Relaxed);
        }
    }

    fn owns_window(&self) -> bool {
        self.info.owns_window
    }

    fn is_open(&self) -> bool {
        self.open.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn close(&self) {
        let mut guard = self.ui.lock();
        self.open.store(false, std::sync::atomic::Ordering::Relaxed);
        let Some(ui) = guard.take() else { return };
        // Down before it is torn down: a UI that put up its own window has to be
        // told to take it away, or the window outlives the plugin behind it.
        if let Some(hide) = ui.show.and_then(|s| unsafe { (*s).hide }) {
            unsafe { hide(ui.handle) };
        }
        // SAFETY: the handle came from this descriptor's `instantiate` and is
        // dropped exactly once — `take` above makes a second close a no-op.
        unsafe {
            if let Some(cleanup) = (*ui.descriptor).cleanup {
                cleanup(ui.handle);
            }
        }
    }
}
