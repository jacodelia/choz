//! Runtime checks against the VST2 plugins installed on this machine.
//! Skips when `/usr/lib/vst` holds nothing loadable.

use choz_ports::{AudioSource, FxProcessor};
use choz_plugin_vst2::{Vst2Effect, Vst2Instrument, Vst2PluginInfo, scan_directory};

const SR: u32 = 48_000;
const BLOCK: u32 = 256;

fn installed() -> Vec<Vst2PluginInfo> {
    scan_directory(std::path::Path::new("/usr/lib/vst"))
}

fn sine_block(frames: usize) -> Vec<f32> {
    let mut buf = vec![0.0f32; frames * 2];
    for f in 0..frames {
        let s = (2.0 * std::f32::consts::PI * 220.0 * f as f32 / SR as f32).sin() * 0.5;
        buf[f * 2] = s;
        buf[f * 2 + 1] = s;
    }
    buf
}

/// One test, on purpose: JUCE-based VST2 plugins do global initialisation on
/// load and crash when two threads load plugins at once. choz only ever loads
/// them from the UI thread, and the test harness runs test *functions* in
/// parallel — so all the plugin work lives in this single function.
#[test]
fn installed_vst2_plugins_scan_host_and_expose_params() {
    let found = installed();
    if found.is_empty() {
        eprintln!("no VST2 plugins installed; skipping");
        return;
    }
    for p in &found {
        assert!(!p.name.is_empty(), "{} has no name", p.path.display());
        assert!(p.id.starts_with("vst2:"), "odd id {}", p.id);
    }

    // The first few installed effects load, process, and stay finite.
    let mut hosted = 0;
    for info in found.iter().filter(|p| !p.is_instrument) {
        let Some(mut fx) = Vst2Effect::build(&info.path, SR, BLOCK) else { continue };
        let mut buf = sine_block(BLOCK as usize);
        fx.process_block(&mut buf, SR);
        assert!(buf.iter().all(|s| s.is_finite()), "{} produced non-finite", info.name);
        hosted += 1;
        if hosted == 3 {
            break;
        }
    }
    assert!(hosted > 0, "no installed VST2 effect could be hosted");

    // Parameters come back with the plugin's own names, normalised 0..1.
    let Some(info) = found.iter().find(|p| !p.is_instrument) else { return };
    let params = choz_plugin_vst2::read_params(&info.path, &info.id);
    if params.is_empty() {
        eprintln!("{} exposes no parameters; skipping", info.name);
        return;
    }
    for p in &params {
        assert!(!p.name.is_empty());
        assert_eq!((p.min, p.max), (0.0, 1.0));
        assert!((0.0..=1.0).contains(&p.default), "{} default out of range", p.name);
    }

    // An editor handle may outlive the plugin: the window thread can still be
    // pumping idle when the user swaps the instrument out of the slot. Past the
    // drop every call must be a no-op instead of a use-after-free. The window
    // itself is never opened here — a test must not pop up a GUI.
    for info in found.iter().filter(|p| p.is_instrument) {
        let Some(inst) = Vst2Instrument::build(&info.path, SR, BLOCK) else { continue };
        let Some(editor) = inst.editor() else { continue };
        drop(inst);
        editor.idle();
        editor.close();
        break;
    }
}
