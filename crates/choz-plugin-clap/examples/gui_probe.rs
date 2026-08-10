//! Open every installed CLAP plugin's window in a real X11 window.
//! `cargo run -p choz-plugin-clap --example gui_probe [filtro]`

use choz_ports::{AudioSource, FxProcessor};

fn main() {
    let only = std::env::args().nth(1);
    let (conn, screen_num) = x11rb::connect(None).expect("no DISPLAY");
    let win = make_window(&conn, screen_num);

    let mut opened = 0;
    let mut no_gui = 0;

    for dir in ["/usr/lib/clap", "/usr/local/lib/clap"] {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for e in entries.flatten() {
            let path = e.path();
            if path.extension().is_none_or(|x| x != "clap") {
                continue;
            }
            for info in choz_plugin_clap::describe(&path) {
                if only
                    .as_ref()
                    .is_some_and(|f| !info.name.contains(f.as_str()))
                {
                    continue;
                }
                println!("try {} [{}]", info.name, info.id);
                use std::io::Write;
                std::io::stdout().flush().ok();

                // El plugin tiene que seguir VIVO mientras se usa el editor: su
                // Drop vacía la celda compartida, que es justo lo que protege a
                // una ventana huérfana. Consumirlo aquí medía otra cosa.
                let instrument;
                let effect;
                let editor = if info.is_instrument {
                    instrument =
                        choz_plugin_clap::host::ClapInstrument::build(&path, &info.id, 48_000, 256);
                    instrument.as_ref().and_then(|i| i.editor())
                } else {
                    effect =
                        choz_plugin_clap::host::ClapEffect::build(&path, &info.id, 48_000, 256);
                    effect.as_ref().and_then(|i| i.editor())
                };
                let Some(ed) = editor else {
                    no_gui += 1;
                    println!("    (sin GUI embebible)");
                    continue;
                };
                let size = ed.open(win as u64);
                // Por si la ventana llega con retardo (timer / on_main_thread).
                for _ in 0..30 {
                    ed.idle();
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                // La prueba real: ¿el plugin creó su ventana dentro de la nuestra?
                use x11rb::connection::Connection as _;
                use x11rb::protocol::xproto::ConnectionExt as _;
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
                println!("    hijos={kids} geom={geom} size={size:?}");
                if kids > 0 {
                    opened += 1;
                } else {
                    no_gui += 1;
                }
                ed.idle();
                ed.close();
            }
        }
    }
    println!("\ncon ventana real: {opened}   sin ventana: {no_gui}");
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
    conn.map_window(win).unwrap();
    conn.flush().unwrap();
    win
}
