//! Runtime checks against the VST3 bundles installed on this machine.
//! Skips when `/usr/lib/vst3` holds nothing loadable.
//!
//! One test function on purpose: VST3 modules do global initialisation on load
//! (`ModuleEntry`), and the harness runs test functions in parallel. choz only
//! ever loads plugins from the UI thread.

use choz_ports::{AudioSource, FxProcessor};
use choz_plugin_vst3::{Vst3Effect, Vst3Instrument, scan_directory};

const SR: u32 = 48_000;
const BLOCK: u32 = 256;

#[test]
fn installed_vst3_plugins_scan_host_and_sound() {
    let found = scan_directory(std::path::Path::new("/usr/lib/vst3"));
    if found.is_empty() {
        eprintln!("no VST3 plugins installed; skipping");
        return;
    }
    for p in &found {
        assert!(!p.name.is_empty(), "{} has no name", p.path.display());
    }

    // An effect processes a block and stays finite.
    let mut hosted = 0;
    for info in found.iter().filter(|p| !p.is_instrument) {
        let Some(mut fx) = Vst3Effect::build(&info.path, SR, BLOCK) else { continue };
        let mut buf = vec![0.25f32; BLOCK as usize * 2];
        fx.process_block(&mut buf, SR);
        assert!(buf.iter().all(|s| s.is_finite()), "{} produced non-finite", info.name);
        hosted += 1;
        break;
    }

    // An instrument makes sound after a note-on.
    for info in found.iter().filter(|p| p.is_instrument) {
        let Some(mut inst) = Vst3Instrument::build(&info.path, SR, BLOCK) else { continue };
        inst.note_on(60, 100);
        let mut peak = 0.0f32;
        for _ in 0..20 {
            let mut buf = vec![0.0f32; BLOCK as usize * 2];
            inst.render(&mut buf, SR);
            for s in &buf {
                assert!(s.is_finite(), "{} produced non-finite", info.name);
                peak = peak.max(s.abs());
            }
        }
        assert!(peak > 1e-4, "{} made no sound on note-on", info.name);
        hosted += 1;
        break;
    }

    assert!(hosted > 0, "nothing among {} installed VST3 bundles could be hosted", found.len());
}
