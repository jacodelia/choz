//! What each installed synth actually offers as presets, in process.
use choz_engine::PluginFormat;
fn main() {
    if choz_engine::worker_main() { return; }
    for (fmt, path, id) in [
        (PluginFormat::Vst3, "/usr/lib/vst3/Surge XT.vst3", ""),
        (PluginFormat::Vst3, "/home/jorge/.vst3/u-he/TyrellN6.vst3", ""),
        (PluginFormat::Vst2, "/home/jorge/.vst/u-he/TyrellN6.64.so", ""),
    ] {
        if !std::path::Path::new(path).exists() { println!("{path}: not here"); continue; }
        match choz_engine::engine::build_instrument(fmt, std::path::Path::new(path), id, 48_000, 128) {
            Ok(inst) => {
                let n = inst.presets().map(|p| p.list().len());
                println!("{path}: presets = {n:?}, state = {:?}",
                    inst.state().and_then(|s| s.save()).map(|b| b.len()));
            }
            Err(e) => println!("{path}: {e}"),
        }
    }
}
