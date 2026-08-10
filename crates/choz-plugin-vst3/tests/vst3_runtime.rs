//! Runtime checks against the VST3 bundles installed on this machine.
//! Skips when `/usr/lib/vst3` holds nothing loadable.
//!
//! One test function on purpose: VST3 modules do global initialisation on load
//! (`ModuleEntry`), and the harness runs test functions in parallel. choz only
//! ever loads plugins from the UI thread.

use choz_plugin_vst3::{scan_directory, Vst3Effect, Vst3Instrument};
use choz_ports::{AudioSource, FxProcessor};

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
        let Some(mut fx) = Vst3Effect::build(&info.path, SR, BLOCK) else {
            continue;
        };
        let mut buf = vec![0.25f32; BLOCK as usize * 2];
        fx.process_block(&mut buf, SR);
        assert!(
            buf.iter().all(|s| s.is_finite()),
            "{} produced non-finite",
            info.name
        );
        hosted += 1;
        break;
    }

    // A parameter change must reach the *processor*, not just the controller.
    // In VST3 the GUI lives in the edit controller, and a host that does not
    // carry `performEdit` values into `ProcessData.inputParameterChanges`
    // leaves the knob moving on screen with the sound unchanged — which is
    // exactly what Surge XT did here. The check is deliberately weak about
    // *which* parameter does what: it sweeps each of the first few and asks
    // that at least one of them changes the output.
    for info in found.iter().filter(|p| !p.is_instrument) {
        let Some(mut fx) = Vst3Effect::build(&info.path, SR, BLOCK) else {
            continue;
        };
        let params = choz_plugin_vst3::read_params(&info.path, "");
        if params.is_empty() {
            continue;
        }
        let input: Vec<f32> = (0..BLOCK as usize * 2)
            .map(|i| (i as f32 * 0.05).sin() * 0.5)
            .collect();
        let render = |fx: &mut Vst3Effect| {
            let mut buf = input.clone();
            // Two blocks: some plugins smooth a change over one.
            fx.process_block(&mut buf, SR);
            let mut buf = input.clone();
            fx.process_block(&mut buf, SR);
            buf
        };
        let before = render(&mut fx);
        let mut changed = false;
        for i in 0..params.len().min(8) {
            fx.set_param(i, 0.0);
            let low = render(&mut fx);
            fx.set_param(i, 1.0);
            let high = render(&mut fx);
            if low.iter().zip(&high).any(|(a, b)| (a - b).abs() > 1e-4) {
                changed = true;
                break;
            }
        }
        assert!(
            changed || before.iter().all(|s| *s == 0.0),
            "{}: sweeping its parameters changed nothing in the audio",
            info.name
        );
        break;
    }

    // The plugin's own state, which is what a project has to carry: parameter
    // values alone cannot describe a patch. Saved from one instance, restored
    // into a fresh one, and the parameters have to come back with it.
    for info in found.iter() {
        let Some(mut fx) = Vst3Effect::build(&info.path, SR, BLOCK) else {
            continue;
        };
        let Some(state) = fx.state() else { continue };
        let params = choz_plugin_vst3::read_params(&info.path, "");
        if params.is_empty() {
            continue;
        }
        // Move something, then take the picture.
        fx.set_param(0, 0.83);
        let Some(blob) = state.save() else { continue };
        assert!(!blob.is_empty());
        drop(fx);

        let Some(fresh) = Vst3Effect::build(&info.path, SR, BLOCK) else {
            continue;
        };
        let restored = fresh.state().expect("the same plugin still has state");
        restored.restore(&blob);
        let after = restored.save().expect("state readable again");
        assert_eq!(
            after, blob,
            "{}: the patch did not survive the round trip",
            info.name
        );
        break;
    }

    // An instrument makes sound after a note-on.
    for info in found.iter().filter(|p| p.is_instrument) {
        let Some(mut inst) = Vst3Instrument::build(&info.path, SR, BLOCK) else {
            continue;
        };
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

    assert!(
        hosted > 0,
        "nothing among {} installed VST3 bundles could be hosted",
        found.len()
    );
}
