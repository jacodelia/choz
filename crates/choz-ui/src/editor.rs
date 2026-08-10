//! The plugin's own window, next to the terminal.
//!
//! A plugin editor is an X11 child window: choz creates a top-level window,
//! hands its XID to the plugin (`effEditOpen` and friends) and pumps the
//! plugin's idle callback until the window is closed. Every X11 call — and
//! every editor call — happens on the one thread this module spawns, which is
//! what the plugin APIs require; the audio thread keeps rendering meanwhile.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use choz_ports::EditorHandle;

/// An open plugin window. Dropping it closes the window and joins its thread.
pub struct EditorWindow {
    close: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// What the window belongs to: `(rack slot, FX index or None for the
    /// instrument)`. A second `[GUI]` click on the same plugin closes it; a
    /// different plugin opens its own.
    pub key: (usize, Option<usize>),
}

impl EditorWindow {
    /// Open `handle`'s editor in a new window. `None` if no window system is
    /// available (no `DISPLAY`, or a build without X11).
    pub fn open(key: (usize, Option<usize>), handle: EditorHandle, title: String) -> Option<Self> {
        let close = Arc::new(AtomicBool::new(false));
        let thread = spawn(handle, Arc::clone(&close), title)?;
        Some(Self {
            close,
            thread: Some(thread),
            key,
        })
    }

    /// False once the user closed the window from the window manager.
    pub fn is_open(&self) -> bool {
        !self.close.load(Ordering::Relaxed)
    }
}

impl Drop for EditorWindow {
    fn drop(&mut self) {
        self.close.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(target_os = "linux")]
fn spawn(
    handle: EditorHandle,
    close: Arc<AtomicBool>,
    title: String,
) -> Option<std::thread::JoinHandle<()>> {
    if std::env::var_os("DISPLAY").is_none() {
        eprintln!("choz: no DISPLAY — plugin editors need an X11 (or XWayland) session");
        return None;
    }
    std::thread::Builder::new()
        .name("choz-plugin-editor".into())
        .spawn(move || {
            if let Err(e) = x11::run(&handle, &close, &title) {
                eprintln!("choz: plugin editor: {e}");
            }
            // Whatever happened, the plugin must not be left with a window it
            // thinks is alive.
            handle.close();
            close.store(true, Ordering::Relaxed);
        })
        .ok()
}

#[cfg(not(target_os = "linux"))]
fn spawn(
    _handle: EditorHandle,
    _close: Arc<AtomicBool>,
    _title: String,
) -> Option<std::thread::JoinHandle<()>> {
    eprintln!("choz: plugin editors are only implemented for X11");
    None
}

#[cfg(target_os = "linux")]
mod x11 {
    use super::{AtomicBool, EditorHandle, Ordering};
    use std::time::Duration;

    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{
        AtomEnum, ConfigureWindowAux, ConnectionExt, CreateWindowAux, EventMask, PropMode,
        WindowClass,
    };
    use x11rb::protocol::Event;
    use x11rb::wrapper::ConnectionExt as _;

    /// Default size until the plugin reports the one it wants.
    const FALLBACK: (u16, u16) = (600, 400);

    pub fn run(handle: &EditorHandle, close: &AtomicBool, title: &str) -> anyhow::Result<()> {
        let (conn, screen_num) = x11rb::connect(None)?;
        let screen = &conn.setup().roots[screen_num];
        let win = conn.generate_id()?;
        let (mut w, mut h) = FALLBACK;
        conn.create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            win,
            screen.root,
            0,
            0,
            w,
            h,
            0,
            WindowClass::INPUT_OUTPUT,
            screen.root_visual,
            &CreateWindowAux::new().event_mask(EventMask::STRUCTURE_NOTIFY),
        )?;

        // Ask the window manager to tell us about its close button instead of
        // killing the connection under the plugin.
        let wm_protocols = conn.intern_atom(false, b"WM_PROTOCOLS")?.reply()?.atom;
        let wm_delete = conn.intern_atom(false, b"WM_DELETE_WINDOW")?.reply()?.atom;
        conn.change_property32(
            PropMode::REPLACE,
            win,
            wm_protocols,
            AtomEnum::ATOM,
            &[wm_delete],
        )?;
        conn.change_property8(
            PropMode::REPLACE,
            win,
            AtomEnum::WM_NAME,
            AtomEnum::STRING,
            title.as_bytes(),
        )?;
        conn.map_window(win)?;
        conn.flush()?;

        // The parent handle a plugin expects on Linux is the X11 window XID.
        if let Some((rw, rh)) = handle.open(win as u64) {
            (w, h) = (rw, rh);
            conn.configure_window(
                win,
                &ConfigureWindowAux::new().width(w as u32).height(h as u32),
            )?;
            conn.flush()?;
        }

        while !close.load(Ordering::Relaxed) {
            while let Some(event) = conn.poll_for_event()? {
                if let Event::ClientMessage(cm) = event {
                    if cm.data.as_data32()[0] == wm_delete {
                        close.store(true, Ordering::Relaxed);
                    }
                }
            }
            handle.idle();
            conn.flush()?;
            std::thread::sleep(Duration::from_millis(30));
        }

        handle.close();
        conn.destroy_window(win)?;
        conn.flush()?;
        Ok(())
    }
}
