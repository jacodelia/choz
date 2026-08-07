//! Open every installed LV2 X11 UI in a real window and report what happened.
//! Run with a live DISPLAY: `cargo run -p choz-plugin-lv2 --example ui_probe`.

use choz_ports::{AudioSource, FxProcessor};

/// Print and flush. stdout to a file is block-buffered, so a segfault in the
/// next plugin would swallow lines already "printed" — which is how an earlier
/// sweep silently lost 74 results and reported a clean count.
fn say(line: String) {
    use std::io::Write;
    println!("{line}");
    std::io::stdout().flush().ok();
}

fn main() {
    // `--skip N` resumes past a UI that killed the previous run, so one bad
    // plugin does not hide the count for all the rest.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let skip: usize = args
        .iter()
        .position(|a| a == "--skip")
        .and_then(|i| args.get(i + 1))
        .and_then(|n| n.parse().ok())
        .unwrap_or(0);
    let only = args.first().filter(|a| *a != "--skip").cloned();
    let mut seen = 0usize;
    let mut opened = 0;
    let mut no_ui = 0;
    let mut failed = Vec::new();

    // A throwaway X11 window to parent the UIs into.
    let (conn, screen_num) = x11rb::connect(None).expect("no DISPLAY");
    let win = make_window(&conn, screen_num);

    for e in std::fs::read_dir("/usr/lib/lv2").unwrap().flatten() {
        let p = e.path();
        if p.extension().is_none_or(|x| x != "lv2") {
            continue;
        }
        for info in choz_plugin_lv2::discovery::discover_bundle(&p) {
            if let Some(f) = &only
                && !info.name.contains(f.as_str())
            {
                continue;
            }
            if info.x11_ui.is_none() {
                no_ui += 1;
                continue;
            }
            seen += 1;
            if seen <= skip {
                continue;
            }
            say(format!("try #{seen} {} [{}]", info.name, info.uri));
            // El plugin tiene que seguir vivo mientras se usa el editor: su Drop
            // vacía los controles compartidos.
            let instrument;
            let effect;
            let editor = if info.is_instrument {
                instrument = choz_plugin_lv2::Lv2Instrument::build(&p, &info.uri, 48_000, 256);
                instrument.as_ref().and_then(|i| i.editor())
            } else {
                effect = choz_plugin_lv2::Lv2Effect::build(&p, &info.uri, 48_000, 256);
                effect.as_ref().and_then(|i| i.editor())
            };
            let Some(ed) = editor else {
                say(format!("NOEDITOR  {}", info.name));
                failed.push(info.name.clone());
                continue;
            };
            ed.open(win as u64);
            ed.idle();
            use x11rb::connection::Connection as _;
            use x11rb::protocol::xproto::ConnectionExt as _;
            conn.flush().unwrap();
            let kids = conn
                .query_tree(win)
                .unwrap()
                .reply()
                .map(|r| r.children.len())
                .unwrap_or(0);
            ed.close();
            if kids > 0 {
                opened += 1;
                say(format!("ok  {} (hijos={kids})", info.name));
            } else {
                say(format!("SINVENTANA  {}", info.name));
                failed.push(info.name.clone());
            }
        }
    }

    println!("\nabiertas: {opened}   sin UI: {no_ui}   fallidas: {}", failed.len());
    for f in failed.iter().take(20) {
        println!("  {f}");
    }
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
        600,
        400,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &CreateWindowAux::new().event_mask(EventMask::EXPOSURE),
    )
    .unwrap();
    // choz's real editor thread maps the window before handing the XID to the
    // plugin; a UI that draws into an unmapped parent can fault.
    conn.map_window(win).unwrap();
    conn.flush().unwrap();
    win
}
