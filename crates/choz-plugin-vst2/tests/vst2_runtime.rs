//! Runtime checks against the VST2 plugins installed on this machine.
//! Skips when `/usr/lib/vst` holds nothing loadable.

use choz_plugin_vst2::{scan_directory, Vst2Effect, Vst2Instrument, Vst2PluginInfo};
use choz_ports::{AudioSource, FxProcessor};

const SR: u32 = 48_000;
const BLOCK: u32 = 256;

/// Serialises every test that dlopens a plugin.
///
/// The harness runs test *functions* in parallel and both of these load the
/// same shared objects. The DPF-based Zam plugins assert on their own
/// half-initialised state when two threads instantiate them at once
/// (`assertion failure: "bufferSize != 0"`, then a SIGSEGV that takes the whole
/// binary down) — the LV2, CLAP, VST3 and DSSI suites all carry this lock for
/// the same reason. This one only grew a second test today.
fn plugin_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn installed() -> Vec<Vst2PluginInfo> {
    let mut found = scan_directory(std::path::Path::new("/usr/lib/vst"));
    // The stock directory holds only effects on most machines. `CHOZ_VST2_DIR`
    // points the instrument half of these checks at wherever the user unpacked
    // theirs (TyrellN6, TripleCheese, Pianoteq…) without hardcoding a path that
    // means nothing anywhere else.
    if let Some(extra) = std::env::var_os("CHOZ_VST2_DIR") {
        found.extend(scan_directory(std::path::Path::new(&extra)));
    }
    found
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
    let _guard = plugin_lock();
    let found = installed();
    if found.is_empty() {
        eprintln!("no VST2 plugins installed; skipping");
        return;
    }
    for p in &found {
        assert!(!p.name.is_empty(), "{} has no name", p.path.display());
        assert!(p.id.starts_with("vst2:"), "odd id {}", p.id);
    }

    // An instrument: choz must be able to drive its parameters (that is what a
    // MIDI-learn binding ends up calling), and it must offer the feed that
    // reports what the user moves inside its own window.
    for info in found.iter().filter(|p| p.is_instrument) {
        let Some(mut inst) = Vst2Instrument::build(&info.path, SR, BLOCK) else {
            continue;
        };
        let params = choz_plugin_vst2::read_params(&info.path, "");
        assert!(!params.is_empty(), "{} exposes no parameters", info.name);
        assert!(
            inst.param_touch().is_some(),
            "{}: no way to report what the user moves in its window",
            info.name
        );

        let render = |inst: &mut Vst2Instrument| {
            let mut buf = vec![0.0f32; BLOCK as usize * 2];
            let mut peak = 0.0f32;
            for _ in 0..8 {
                buf.iter_mut().for_each(|s| *s = 0.0);
                inst.render(&mut buf, SR);
                peak = peak.max(buf.iter().fold(0.0f32, |a, s| a.max(s.abs())));
            }
            peak
        };
        inst.note_on(60, 100);
        // A plugin that makes no sound of its own cannot demonstrate anything
        // about its parameters. Pianoteq without a licence is exactly this: it
        // loads, it accepts notes and it renders silence. Try the next one.
        if render(&mut inst) <= 1e-4 {
            continue;
        }
        // Sweep a few parameters; at least one has to move the sound, or choz
        // cannot control this plugin at all.
        let mut changed = false;
        for i in 0..params.len().min(12) {
            inst.set_param(i, 0.0);
            let low = render(&mut inst);
            inst.set_param(i, 1.0);
            let high = render(&mut inst);
            if (low - high).abs() > 1e-4 {
                changed = true;
                break;
            }
        }
        assert!(changed, "{}: no parameter changed the sound", info.name);
        break;
    }

    // The plugin's own chunk: a u-he patch is not a list of parameter values,
    // and a project that only saved the values would reopen on a different
    // sound. Saved from one instance, restored into a fresh one.
    for info in found.iter().filter(|p| p.is_instrument) {
        let Some(mut inst) = Vst2Instrument::build(&info.path, SR, BLOCK) else {
            continue;
        };
        let Some(state) = inst.state() else { continue };
        inst.set_param(0, 0.7);
        let Some(blob) = state.save() else {
            // No `effFlagsProgramChunks`: nothing to carry, which is legal.
            continue;
        };
        drop(inst);

        let Some(fresh) = Vst2Instrument::build(&info.path, SR, BLOCK) else {
            continue;
        };
        let restored = fresh.state().expect("same plugin, same capability");
        restored.restore(&blob);
        let once = restored
            .save()
            .expect("a plugin that saved once saves again");
        assert!(!once.is_empty(), "{}: nothing came back", info.name);

        // **Stable from there**, rather than byte-for-byte the first save.
        // That is what a project reopening twice gets, and it is what the
        // plugin owes: amsynth's first save writes an empty `<name>` line and
        // after a round trip it writes the name and the next parameter on one
        // line — 1176 bytes in, 1200 out, the same patch. Measured, and a fixed
        // point from the second pass on.
        restored.restore(&once);
        assert_eq!(
            restored.save(),
            Some(once),
            "{}: the chunk did not survive the round trip",
            info.name
        );
        break;
    }

    // The first few installed effects load, process, and stay finite.
    let mut hosted = 0;
    for info in found.iter().filter(|p| !p.is_instrument) {
        let Some(mut fx) = Vst2Effect::build(&info.path, SR, BLOCK) else {
            continue;
        };
        let mut buf = sine_block(BLOCK as usize);
        fx.process_block(&mut buf, SR);
        assert!(
            buf.iter().all(|s| s.is_finite()),
            "{} produced non-finite",
            info.name
        );
        hosted += 1;
        if hosted == 3 {
            break;
        }
    }
    assert!(hosted > 0, "no installed VST2 effect could be hosted");

    // Parameters come back with the plugin's own names, normalised 0..1.
    let Some(info) = found.iter().find(|p| !p.is_instrument) else {
        return;
    };
    let params = choz_plugin_vst2::read_params(&info.path, &info.id);
    if params.is_empty() {
        eprintln!("{} exposes no parameters; skipping", info.name);
        return;
    }
    for p in &params {
        assert!(!p.name.is_empty());
        assert_eq!((p.min, p.max), (0.0, 1.0));
        assert!(
            (0.0..=1.0).contains(&p.default),
            "{} default out of range",
            p.name
        );
    }

    // An editor handle may outlive the plugin: the window thread can still be
    // pumping idle when the user swaps the instrument out of the slot. Past the
    // drop every call must be a no-op instead of a use-after-free. The window
    // itself is never opened here — a test must not pop up a GUI.
    for info in found.iter().filter(|p| p.is_instrument) {
        let Some(inst) = Vst2Instrument::build(&info.path, SR, BLOCK) else {
            continue;
        };
        let Some(editor) = inst.editor() else {
            continue;
        };
        drop(inst);
        editor.idle();
        editor.close();
        break;
    }
}

/// A VST2 with more than one program must list them by name and switch between
/// them. Verified through `effGetProgram` rather than through the audio: a
/// plugin can be silent for reasons of its own (an unlicensed demo, a sampler
/// with nothing loaded) and still be switching programs correctly.
///
/// Skipped when nothing installed declares more than one program — which is
/// most effects, and both u-he synths (they browse their own presets instead).
#[test]
fn vst2_programs_are_listed_and_selected() {
    let _guard = plugin_lock();
    let found = installed();
    if found.is_empty() {
        eprintln!("no VST2 plugins installed; skipping");
        return;
    }

    for info in &found {
        let Some(inst) = Vst2Instrument::build(&info.path, SR, BLOCK)
            .map(Some)
            .unwrap_or(None)
        else {
            continue;
        };
        let Some(browser) = inst.presets() else {
            continue; // one program (or none): nothing to pick
        };
        let list = browser.list();
        assert!(list.len() > 1, "{}: a browser with one row", info.name);
        for p in list.iter().take(20) {
            assert!(!p.name.is_empty(), "{}: a program with no name", info.name);
            assert!(p.key.parse::<i32>().is_ok(), "{}: {p:?}", info.name);
        }

        // Pick one and ask the plugin where it is.
        let target = &list[list.len() / 2];
        browser.load(&target.key);
        assert_eq!(
            browser.current().as_deref(),
            Some(target.key.as_str()),
            "{}: selecting '{}' did not stick",
            info.name,
            target.name
        );
        browser.load(&list[0].key);
        assert_eq!(browser.current().as_deref(), Some(list[0].key.as_str()));

        // A key that is not a program index is ignored rather than fatal.
        browser.load("not-a-number");
        assert_eq!(browser.current().as_deref(), Some(list[0].key.as_str()));
        return;
    }
    eprintln!("no installed VST2 declares more than one program; skipping");
}
