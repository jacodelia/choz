//! The audition path of the SOUND/BANK picker, with the real backend running:
//! a plugin slot in a started engine, handed one patch file after another.
//!
//! ```sh
//! cargo run --release -p choz-engine --example audition_hang -- \
//!     "/usr/lib/vst3/Surge XT.vst3" /usr/share/surge-xt/patches_factory
//! ```
fn main() {
    if choz_engine::worker_main() {
        return;
    }
    let mut args = std::env::args().skip(1);
    let plugin = args.next().expect("plugin path");
    let dir = args.next().expect("bank dir");
    let mut engine = choz_engine::engine::AudioEngine::new(48_000, 256);
    engine.start().expect("audio backend");
    let slot = engine
        .add_plugin(
            choz_engine::PluginFormat::Vst3,
            std::path::Path::new(&plugin),
            "",
        )
        .expect("load")
        .expect("slot");
    println!("slot {slot}, state handle: {}", engine.slot_has_state(slot));
    let files = choz_engine::preset_files::list_bank(std::path::Path::new(&dir));
    println!("{} patches", files.len());
    let take: usize = std::env::var("TAKE").ok().and_then(|v| v.parse().ok()).unwrap_or(usize::MAX);
    for (n, p) in files.iter().take(take).enumerate() {
        let Ok(blob) = choz_engine::preset_files::read_state_key(&p.key) else {
            continue;
        };
        println!("{n} -> {} ({} bytes)", p.name, blob.len());
        engine.set_slot_state(slot, &blob);
        // What the picker does between rows: play the thing.
        engine.note_on(slot, 60, 100);
        std::thread::sleep(std::time::Duration::from_millis(120));
        engine.note_off(slot, 60);
        println!("{n} back");
    }
    println!("auditioned; dropping the engine (what quitting does)");
    drop(engine);
    println!("done, no hang");
}
