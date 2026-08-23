//! The plugin's own window, next to the terminal.
//!
//! A plugin editor is an X11 child window: choz creates a top-level window,
//! hands its XID to the plugin (`effEditOpen` and friends) and pumps the
//! plugin's idle callback until the window is closed. Every X11 call — and
//! every editor call — happens on the one thread this module spawns, which is
//! what the plugin APIs require; the audio thread keeps rendering meanwhile.
//!
//! Some editors are not a child of anything: an LV2 UI with `ui:showInterface`
//! (Yoshimi, ZynAddSubFX) opens **its own** window and the host only pumps it
//! and asks whether it is still there. Those get [`run_owned`] — the same
//! thread, no window of ours, and the close comes from the plugin instead of
//! from the window manager.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use choz_ports::EditorHandle;

/// An open plugin window. Dropping it closes the window and joins its thread.
pub struct EditorWindow {
    close: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// The thread reports here on its way out, so [`Drop`] can wait for it
    /// **with a deadline** — see there for why a plain join is not enough.
    gone: std::sync::mpsc::Receiver<()>,
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
        let (tx, gone) = std::sync::mpsc::channel();
        let thread = spawn(handle, Arc::clone(&close), title, tx)?;
        Some(Self {
            close,
            thread: Some(thread),
            gone,
            key,
        })
    }

    /// False once the user closed the window from the window manager.
    pub fn is_open(&self) -> bool {
        !self.close.load(Ordering::Relaxed)
    }
}

impl Drop for EditorWindow {
    /// Close the window and wait for its thread — **but not forever**.
    ///
    /// The thread is inside the plugin (`idle()` runs a JUCE plugin's own
    /// message loop), so a plugin that wedges there wedges the join, and the
    /// whole interface with it: choz frozen mid-set because a window would not
    /// close. Everything that drops an instrument closes its window first
    /// (`App::close_editor_for`), which is what keeps this from happening at
    /// all; this is the second line — after two seconds the thread is left to
    /// its own devices and choz carries on. It holds an `Arc` on the editor
    /// handle, so nothing it can still touch has been freed.
    fn drop(&mut self) {
        self.close.store(true, Ordering::Relaxed);
        let Some(t) = self.thread.take() else {
            return;
        };
        match self.gone.recv_timeout(std::time::Duration::from_secs(2)) {
            Ok(()) => {
                let _ = t.join();
            }
            Err(_) => {
                eprintln!(
                    "choz: this plugin's window did not close; leaving its thread behind \
                     rather than freezing (the window may stay on screen)"
                );
                std::mem::forget(t);
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn spawn(
    handle: EditorHandle,
    close: Arc<AtomicBool>,
    title: String,
    gone: std::sync::mpsc::Sender<()>,
) -> Option<std::thread::JoinHandle<()>> {
    if std::env::var_os("DISPLAY").is_none() {
        eprintln!("choz: no DISPLAY — plugin editors need an X11 (or XWayland) session");
        return None;
    }
    std::thread::Builder::new()
        .name("choz-plugin-editor".into())
        .spawn(move || {
            let run = match handle.owns_window() {
                true => run_owned(&handle, &close),
                false => x11::run(&handle, &close, &title),
            };
            if let Err(e) = run {
                eprintln!("choz: plugin editor: {e}");
            }
            // Whatever happened, the plugin must not be left with a window it
            // thinks is alive.
            handle.close();
            close.store(true, Ordering::Relaxed);
            // Only now is it safe to drop the instrument this thread was
            // driving: `Drop` waits for this.
            let _ = gone.send(());
        })
        .ok()
}

/// Drive an editor that owns its window: open it, pump it, and stop when either
/// side says so — the user closing choz's `[GUI]` button, or the plugin
/// reporting that its own window has gone.
#[cfg(target_os = "linux")]
fn run_owned(handle: &EditorHandle, close: &Arc<AtomicBool>) -> anyhow::Result<()> {
    // There is no window id to hand over; the UI makes its own.
    handle.open(0);
    while !close.load(Ordering::Relaxed) {
        handle.idle();
        if !handle.is_open() {
            close.store(true, Ordering::Relaxed);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    handle.close();
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn spawn(
    _handle: EditorHandle,
    _close: Arc<AtomicBool>,
    _title: String,
    _gone: std::sync::mpsc::Sender<()>,
) -> Option<std::thread::JoinHandle<()>> {
    eprintln!("choz: plugin editors are only implemented for X11");
    None
}

#[cfg(target_os = "linux")]
mod x11 {
    use super::{AtomicBool, EditorHandle, Ordering};
    use std::time::Duration;

    use x11rb::connection::Connection;
    use x11rb::protocol::randr::ConnectionExt as _;
    use x11rb::protocol::xproto::{
        AtomEnum, ConfigureWindowAux, ConnectionExt, CreateWindowAux, EventMask, PropMode,
        WindowClass,
    };
    use x11rb::protocol::Event;
    use x11rb::wrapper::ConnectionExt as _;

    /// Default size until the plugin reports the one it wants.
    const FALLBACK: (u16, u16) = (600, 400);

    /// Centre `win` on whichever monitor the pointer is on, clamped so the
    /// whole window stays inside it.
    ///
    /// Best effort by design: no pointer, no RandR, or a server that answers
    /// neither simply leaves the window where the window manager put it, which
    /// is what happened before this existed.
    fn place_on_pointer_monitor<C: Connection>(conn: &C, root: u32, win: u32, w: u16, h: u16) {
        let Ok(Ok(p)) = conn.query_pointer(root).map(|c| c.reply()) else {
            return;
        };
        let (px, py) = (p.root_x as i32, p.root_y as i32);
        // The monitor under the pointer, or the whole screen when RandR has
        // nothing to say.
        let mon = conn
            .randr_get_monitors(root, true)
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| {
                r.monitors
                    .into_iter()
                    .find(|m| {
                        px >= m.x as i32
                            && px < m.x as i32 + m.width as i32
                            && py >= m.y as i32
                            && py < m.y as i32 + m.height as i32
                    })
                    .map(|m| (m.x as i32, m.y as i32, m.width as i32, m.height as i32))
            });
        let Some((mx, my, mw, mh)) = mon else {
            return;
        };
        let x = (mx + (mw - w as i32) / 2).clamp(mx, mx + (mw - w as i32).max(0));
        let y = (my + (mh - h as i32) / 2).clamp(my, my + (mh - h as i32).max(0));
        let _ = conn.configure_window(win, &ConfigureWindowAux::new().x(x).y(y));
    }

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
        // On the screen the user is looking at. Two monitors are one X screen,
        // so a window left at 0,0 is placed by the window manager — and with a
        // second monitor attached that regularly meant "the other one", which
        // reads exactly like the editor never opened. The pointer is where the
        // person is: put the window on that monitor.
        place_on_pointer_monitor(&conn, screen.root, win, w, h);
        conn.map_window(win)?;
        conn.flush()?;

        // The parent handle a plugin expects on Linux is the X11 window XID.
        if let Some((rw, rh)) = handle.open(win as u64) {
            (w, h) = (rw, rh);
            conn.configure_window(
                win,
                &ConfigureWindowAux::new().width(w as u32).height(h as u32),
            )?;
            // Re-centred at the size the plugin asked for, or a big editor
            // opens centred as a 600×400 box and hangs off the screen.
            place_on_pointer_monitor(&conn, screen.root, win, w, h);
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
