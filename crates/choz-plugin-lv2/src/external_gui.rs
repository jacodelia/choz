//! Editors that are a **separate program**, driven over OSC.
//!
//! One plugin family needs this, and it is not a detail of the LV2 spec: the
//! ZynAddSubFX bundle's `ui:showInterface` UI draws nothing itself — it starts
//! `zynaddsubfx-ext-gui` and hands it the address of the OSC server the DSP
//! opened. DPF passes that address to the UI over an atom port choz does not
//! implement, so nothing was ever started; [`crate::osc`] finds the address
//! instead, and then the program can simply be started here.
//!
//! What the user gets is the plugin's real window, talking to *this* instance —
//! the same one Carla shows.

use std::process::{Child, Command};

use parking_lot::Mutex;

/// Plugins whose editor is a program of its own: `(plugin URI prefix, the
/// program, how it is told where to connect)`.
///
/// ponytail: a table of one. It is a fact about ZynAddSubFX, not a pattern to
/// generalise before there is a second entry.
const OSC_GUIS: &[(&str, &str, &str)] = &[(
    "http://zynaddsubfx.sourceforge.net",
    "zynaddsubfx-ext-gui",
    "osc.udp://localhost:{port}/",
)];

/// The program that shows `plugin_uri`'s editor, if there is one and it is
/// installed. Checked here so the `[GUI]` button never offers a window that
/// cannot open.
pub fn program_for(plugin_uri: &str) -> Option<(&'static str, &'static str)> {
    let (_, program, url) = OSC_GUIS
        .iter()
        .find(|(uri, ..)| plugin_uri.starts_with(uri))?;
    in_path(program).then_some((*program, *url))
}

fn in_path(program: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join(program).is_file())
}

/// An editor that is a child process.
pub struct ExternalGuiEditor {
    program: &'static str,
    url: String,
    child: Mutex<Option<Child>>,
}

impl ExternalGuiEditor {
    pub fn new(program: &'static str, url_template: &str, port: u16) -> Self {
        Self {
            program,
            url: url_template.replace("{port}", &port.to_string()),
            child: Mutex::new(None),
        }
    }
}

impl choz_ports::PluginEditor for ExternalGuiEditor {
    fn open(&self, _parent: u64) -> Option<(u16, u16)> {
        let mut guard = self.child.lock();
        if guard.is_some() {
            return None; // already up
        }
        match Command::new(self.program).arg(&self.url).spawn() {
            Ok(child) => *guard = Some(child),
            Err(e) => eprintln!("choz: {} would not start: {e}", self.program),
        }
        // Its own window, at its own size.
        None
    }

    fn idle(&self) {}

    fn owns_window(&self) -> bool {
        true
    }

    /// False once the program has quit — which is how the user closes it.
    fn is_open(&self) -> bool {
        let mut guard = self.child.lock();
        match guard.as_mut() {
            Some(child) => matches!(child.try_wait(), Ok(None)),
            None => false,
        }
    }

    fn close(&self) {
        let mut guard = self.child.lock();
        let Some(mut child) = guard.take() else {
            return;
        };
        // The window belongs to the instrument: closing the tab closes it too,
        // or the rack is left driving a window whose plugin is gone.
        let _ = child.kill();
        let _ = child.wait();
    }
}
