//! Open every installed VST3 plugin's window inside a real X11 window and count
//! what actually got created.
//! `cargo run -p choz-plugin-vst3 --example gui_probe [filtro]`
//!
//! Two things this probe does on purpose, both learned the hard way (see
//! docs/roadmap.md):
//!
//! - **The plugin stays alive while its editor is used.** `.and_then(|i| i.editor())`
//!   drops the instance, whose `Drop` empties the shared cell, so every call
//!   quietly takes the "dead instance" path and measures nothing.
//! - **The parent window is created but never mapped**, so a sweep does not
//!   throw dozens of windows onto the user's desktop. Embedded children are
//!   still created and still show up in `query_tree`, which is what is counted
//!   here — the return values are not trusted.

use std::io::Write;

use choz_ports::{AudioSource, FxProcessor};
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::ConnectionExt as _;

/// Where VST3 bundles live on this system, plus whatever is passed as extra
/// arguments.
const DIRS: [&str; 3] = ["/usr/lib/vst3", "/usr/local/lib/vst3", "/home/jorge/repo"];

fn say(line: &str) {
    println!("{line}");
    std::io::stdout().flush().ok();
}

fn main() {
    let only = std::env::args().nth(1);
    let (conn, screen_num) = x11rb::connect(None).expect("no DISPLAY");
    let win = make_window(&conn, screen_num);

    let (mut opened, mut no_window, mut no_editor) = (0, 0, 0);

    for dir in DIRS {
        for info in choz_plugin_vst3::scan_directory(std::path::Path::new(dir)) {
            if only.as_ref().is_some_and(|f| !info.name.contains(f.as_str())) {
                continue;
            }
            say(&format!("try {} [{}]", info.name, info.path.display()));

            let instrument;
            let effect;
            let editor = if info.is_instrument {
                instrument = choz_plugin_vst3::Vst3Instrument::build(&info.path, 48_000, 256);
                instrument.as_ref().and_then(|i| i.editor())
            } else {
                effect = choz_plugin_vst3::Vst3Effect::build(&info.path, 48_000, 256);
                effect.as_ref().and_then(|i| i.editor())
            };
            let Some(ed) = editor else {
                no_editor += 1;
                say("    (sin editor X11)");
                continue;
            };

            let size = ed.open(win as u64);
            // A VST3 UI draws from the run loop, so give it real idle turns.
            for _ in 0..30 {
                ed.idle();
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            conn.flush().unwrap();
            let kids = conn
                .query_tree(win)
                .unwrap()
                .reply()
                .map(|r| r.children.len())
                .unwrap_or(0);
            let geom = conn
                .query_tree(win)
                .unwrap()
                .reply()
                .ok()
                .and_then(|r| r.children.first().copied())
                .and_then(|c| conn.get_geometry(c).ok()?.reply().ok())
                .map(|g| format!("{}x{}", g.width, g.height))
                .unwrap_or_else(|| "-".into());
            say(&format!("    hijos={kids} geom={geom} size={size:?}"));
            if kids > 0 {
                opened += 1;
            } else {
                no_window += 1;
            }
            ed.close();
        }
    }
    say(&format!(
        "\ncon ventana real: {opened}   sin ventana: {no_window}   sin editor: {no_editor}"
    ));
}

fn make_window(conn: &impl x11rb::connection::Connection, screen_num: usize) -> u32 {
    use x11rb::protocol::xproto::*;
    let screen = &conn.setup().roots[screen_num];
    let win = conn.generate_id().unwrap();
    conn.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        win,
        screen.root,
        0,
        0,
        800,
        600,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &CreateWindowAux::new().event_mask(EventMask::EXPOSURE),
    )
    .unwrap();
    // Deliberately NOT mapped: see the module comment.
    conn.flush().unwrap();
    win
}
