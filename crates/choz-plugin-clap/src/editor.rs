//! The plugin's own window, via the `clap.gui` extension.
//!
//! clack's safe wrapper for `clap.gui` needs a `&mut PluginMainThreadHandle`,
//! and choz's instance has already moved to the audio thread by the time anyone
//! can click a button — so the extension is driven through the raw `clap_plugin`
//! pointer instead. The types come from `clap-sys`, the same version clack
//! itself depends on, so the layouts are the ones the plugin was built against.
//!
//! **Thread caveat, stated plainly**: CLAP marks the whole `clap.gui` extension
//! `[main-thread]`, and these calls arrive on choz's editor thread instead. That
//! is the same bargain the VST2 editor makes, and it is why the window is opened
//! and closed from one thread only, under a mutex, never concurrently with a
//! teardown. A plugin that checks the calling thread will refuse (or crash); the
//! honest fix is running the editor in the sandbox process, which is on the
//! roadmap.

use std::ffi::CString;
use std::sync::{Arc, Mutex};

use choz_ports::PluginEditor;
use clap_sys::ext::gui::{
    clap_plugin_gui, clap_window, clap_window_handle, CLAP_EXT_GUI, CLAP_WINDOW_API_X11,
};
use clap_sys::ext::timer_support::{clap_plugin_timer_support, CLAP_EXT_TIMER_SUPPORT};
use clap_sys::plugin::clap_plugin;

use crate::host::SharedGuiState;

/// The live plugin plus its GUI vtable, shared with the editor thread.
///
/// `None` once the instance is gone, so a window still open when its slot is
/// replaced stops calling into freed memory.
pub type SharedGui = Arc<Mutex<Option<GuiCell>>>;

pub struct GuiCell {
    pub plugin: *const clap_plugin,
    pub gui: *const clap_plugin_gui,
    /// `clap.timer-support`, when the plugin has it. A CLAP UI paints from
    /// `on_timer`, so without this the window is created and never draws.
    pub timer: Option<*const clap_plugin_timer_support>,
}

// SAFETY: only ever dereferenced under the mutex, and only from the editor
// thread while the owning instance is alive (which is what `None` marks).
unsafe impl Send for GuiCell {}

pub struct ClapEditor {
    shared: SharedGui,
    /// Timers the plugin registered through the host extension, ticked in
    /// [`PluginEditor::idle`].
    state: SharedGuiState,
    /// Whether the plugin currently holds a created GUI. Guarded by the same
    /// mutex ordering as `shared`: open/close never overlap.
    created: Mutex<bool>,
}

impl ClapEditor {
    /// Build an editor handle if the plugin has a `clap.gui` that can embed into
    /// an X11 window. Returns `None` otherwise, so the `GUI` button only appears
    /// where it can do something.
    ///
    /// Two host extensions have to exist for this to draw anything, and both
    /// are declared in `host.rs`: `clap.gui` (plugins check for it before
    /// building a UI) and `clap.timer-support` (a CLAP UI paints from
    /// `on_timer`, not from an idle callback). Surge XT registers a 20 ms timer
    /// the moment the host offers one.
    ///
    /// Measured with `examples/gui_probe`, which counts the parent window's real
    /// X11 children rather than trusting the return values: **20 of the 20 CLAP
    /// plugins installed here open a window with the size they ask for**.
    pub fn new(shared: SharedGui, state: SharedGuiState) -> Option<Arc<Self>> {
        {
            let guard = shared.lock().unwrap_or_else(|e| e.into_inner());
            let cell = guard.as_ref()?;
            // SAFETY: the cell is `Some` only while the instance is alive.
            let supported = unsafe {
                (*cell.gui)
                    .is_api_supported
                    .is_some_and(|f| f(cell.plugin, CLAP_WINDOW_API_X11.as_ptr(), false))
            };
            if !supported {
                return None;
            }
        }
        Some(Arc::new(Self {
            shared,
            state,
            created: Mutex::new(false),
        }))
    }

    /// Look up `clap.gui` on a freshly built instance. Called while the instance
    /// is still on the building thread.
    ///
    /// # Safety
    /// `plugin` must be a live `clap_plugin` whose `get_extension` is callable.
    pub unsafe fn extension_of(plugin: *const clap_plugin) -> Option<*const clap_plugin_gui> {
        let get = unsafe { (*plugin).get_extension }?;
        let ext = unsafe { get(plugin, CLAP_EXT_GUI.as_ptr()) } as *const clap_plugin_gui;
        (!ext.is_null()).then_some(ext)
    }

    /// The plugin's `clap.timer-support`, if it has one.
    ///
    /// # Safety
    /// `plugin` must be a live `clap_plugin` whose `get_extension` is callable.
    pub unsafe fn timer_of(plugin: *const clap_plugin) -> Option<*const clap_plugin_timer_support> {
        let get = unsafe { (*plugin).get_extension }?;
        let ext = unsafe { get(plugin, CLAP_EXT_TIMER_SUPPORT.as_ptr()) }
            as *const clap_plugin_timer_support;
        (!ext.is_null()).then_some(ext)
    }
}

impl PluginEditor for ClapEditor {
    fn open(&self, parent: u64) -> Option<(u16, u16)> {
        let mut created = self.created.lock().unwrap_or_else(|e| e.into_inner());
        if *created {
            return None;
        }
        let guard = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        let cell = guard.as_ref()?;
        let gui = cell.gui;
        let plugin = cell.plugin;

        // SAFETY: `cell` is `Some` only while the instance lives, and every call
        // below is on this one thread under the two locks held here.
        unsafe {
            if !(*gui)
                .create
                .is_some_and(|f| f(plugin, CLAP_WINDOW_API_X11.as_ptr(), false))
            {
                eprintln!("choz: CLAP gui create(x11) refused");
                return None;
            }
            // Asked for before parenting, which is the order the CLAP host
            // sequence uses: a plugin can legitimately refuse to report a size
            // once it is embedded.
            let mut w = 0u32;
            let mut h = 0u32;
            let size = (*gui)
                .get_size
                .is_some_and(|f| f(plugin, &mut w, &mut h))
                .then(|| (w.min(u16::MAX as u32) as u16, h.min(u16::MAX as u32) as u16))
                .filter(|(w, h)| *w > 0 && *h > 0);

            let window = clap_window {
                api: CLAP_WINDOW_API_X11.as_ptr(),
                specific: clap_window_handle {
                    x11: parent as std::os::raw::c_ulong,
                },
            };
            if !(*gui).set_parent.is_some_and(|f| f(plugin, &window)) {
                eprintln!("choz: CLAP gui set_parent refused");
                // Undo the create: leaving a GUI alive that is parented nowhere
                // is what leaks a floating window with no way to close it.
                if let Some(destroy) = (*gui).destroy {
                    destroy(plugin);
                }
                return None;
            }
            *created = true;

            if let Some(show) = (*gui).show {
                show(plugin);
            }
            size
        }
    }

    /// Tick every timer the plugin registered. This is what makes a CLAP window
    /// actually paint — the extension is where its drawing happens.
    fn idle(&self) {
        let created = self.created.lock().unwrap_or_else(|e| e.into_inner());
        if !*created {
            return;
        }
        let due = crate::host::lock(&self.state).due();
        if due.is_empty() {
            return;
        }
        let guard = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        let Some(cell) = guard.as_ref() else { return };
        let Some(timer) = cell.timer else { return };
        // SAFETY: live instance under the mutex; ids come from our own registry.
        unsafe {
            if let Some(on_timer) = (*timer).on_timer {
                for id in due {
                    on_timer(cell.plugin, id);
                }
            }
        }
    }

    fn close(&self) {
        let mut created = self.created.lock().unwrap_or_else(|e| e.into_inner());
        if !*created {
            return;
        }
        *created = false;
        let guard = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        let Some(cell) = guard.as_ref() else { return };
        // SAFETY: the GUI was created by this type and is destroyed exactly
        // once — `created` is cleared above before anything can re-enter.
        unsafe {
            if let Some(hide) = (*cell.gui).hide {
                hide(cell.plugin);
            }
            if let Some(destroy) = (*cell.gui).destroy {
                destroy(cell.plugin);
            }
        }
    }
}

/// Set a window title on the plugin's GUI, if it takes one.
pub fn suggest_title(shared: &SharedGui, title: &str) {
    let Ok(title) = CString::new(title) else {
        return;
    };
    let guard = shared.lock().unwrap_or_else(|e| e.into_inner());
    let Some(cell) = guard.as_ref() else { return };
    // SAFETY: live instance under the mutex.
    unsafe {
        if let Some(f) = (*cell.gui).suggest_title {
            f(cell.plugin, title.as_ptr());
        }
    }
}
