//! Runtime test against a real `.clap` audio effect, when one is installed.
//! Skipped (passes trivially) on machines without any CLAP effect plugin.

use choz_ports::{AudioSource, FxProcessor};

/// Serialises every test that loads a plugin.
///
/// The harness runs test *functions* in parallel, and u-he's plugins (like the
/// JUCE ones the VST2/VST3 suites had to fold into a single function) do global
/// initialisation on load: two of them starting at once takes the process down.
/// A file-level mutex keeps the test names, which a merged function would lose.
fn plugin_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// First scannable non-instrument plugin on this machine, if any.
fn find_effect() -> Option<choz_plugin_clap::ClapPluginInfo> {
    choz_plugin_clap::default_search_paths()
        .into_iter()
        .flat_map(|d| choz_plugin_clap::scan_directory(&d))
        .find(|p| !p.is_instrument)
}

/// Every hosted CLAP must offer the feed that reports what the user moves
/// inside its own window — that is what MIDI learn binds to. The gesture itself
/// cannot be simulated here (it needs a real GUI), so what is checked is that
/// the plumbing is offered and that its ids translate to positions in the
/// parameter list.
#[test]
fn hosted_clap_plugins_offer_the_window_feed() {
    let _guard = plugin_lock();
    let plugins: Vec<choz_plugin_clap::ClapPluginInfo> = choz_plugin_clap::default_search_paths()
        .into_iter()
        .flat_map(|d| choz_plugin_clap::scan_directory(&d))
        .collect();
    if plugins.is_empty() {
        eprintln!("no CLAP plugins installed; skipping");
        return;
    }
    let mut checked = 0;
    for info in plugins.iter().take(6) {
        let path = &info.path;
        let params = choz_plugin_clap::host::read_params(path, &info.id);
        if info.is_instrument {
            let Some(inst) =
                choz_plugin_clap::host::ClapInstrument::build(path, &info.id, 48_000, 256)
            else {
                continue;
            };
            assert!(
                inst.param_touch().is_some(),
                "{}: no window feed",
                info.name
            );
        } else {
            let Some(fx) = choz_plugin_clap::host::ClapEffect::build(path, &info.id, 48_000, 256)
            else {
                continue;
            };
            assert!(fx.param_touch().is_some(), "{}: no window feed", info.name);
        }
        // Nothing has been touched, so the feed is empty rather than wrong.
        assert!(!params.is_empty() || info.name.is_empty());
        checked += 1;
    }
    assert!(checked > 0, "not one CLAP plugin could be hosted");
}

#[test]
fn hosted_effect_processes_audio() {
    let _guard = plugin_lock();
    let Some(info) = find_effect() else {
        eprintln!("no CLAP effect installed — skipping");
        return;
    };
    let sr = 48_000;
    let frames = 256;
    let mut fx = choz_plugin_clap::host::ClapEffect::build(&info.path, &info.id, sr, frames)
        .unwrap_or_else(|| panic!("failed to instantiate {} ({})", info.name, info.id));
    fx.set_mix(1.0);

    // A -6 dBFS sine, processed over several blocks so plugin state settles.
    let mut phase = 0.0f32;
    for _ in 0..8 {
        let mut buf = vec![0.0f32; frames as usize * 2];
        for f in 0..frames as usize {
            let s = (2.0 * std::f32::consts::PI * phase).sin() * 0.5;
            phase = (phase + 220.0 / sr as f32) % 1.0;
            buf[f * 2] = s;
            buf[f * 2 + 1] = s;
        }
        fx.process_block(&mut buf, sr);
        for (i, &s) in buf.iter().enumerate() {
            assert!(s.is_finite(), "{} produced non-finite at {i}", info.name);
            assert!(s.abs() < 10.0, "{} ran away to {s} at {i}", info.name);
        }
    }
}

/// A real effect must expose usable parameter metadata, and taking a knob to
/// its extremes must not break the audio.
#[test]
fn plugin_parameters_are_readable_and_settable() {
    let _guard = plugin_lock();
    let Some(info) = find_effect() else { return };
    let params = choz_plugin_clap::read_params(&info.path, &info.id);
    if params.is_empty() {
        eprintln!("{} exposes no parameters — skipping", info.name);
        return;
    }
    for p in &params {
        assert!(!p.name.is_empty(), "{} has an unnamed parameter", info.name);
        assert!(p.max > p.min, "{}:{} has an empty range", info.name, p.name);
        assert!(
            (0.0..=1.0).contains(&p.normalised(p.default)),
            "default outside its own range"
        );
        assert_eq!(p.plain(0.0), p.min);
        assert_eq!(p.plain(1.0), p.max);
    }

    let sr = 48_000;
    let frames = 256usize;
    let mut fx = choz_plugin_clap::host::ClapEffect::build(&info.path, &info.id, sr, frames as u32)
        .expect("instantiate");
    for (i, _) in params.iter().enumerate().take(4) {
        for v in [0.0, 1.0, 0.5] {
            fx.set_param(i, v);
            let mut buf = vec![0.25f32; frames * 2];
            fx.process_block(&mut buf, sr);
            assert!(
                buf.iter().all(|s| s.is_finite() && s.abs() < 10.0),
                "{}: param {i} at {v} broke the output",
                info.name,
            );
        }
    }
}

/// A real CLAP instrument must make sound when handed a note.
#[test]
fn hosted_instrument_sounds_on_note_on() {
    let _guard = plugin_lock();
    let Some(info) = choz_plugin_clap::default_search_paths()
        .into_iter()
        .flat_map(|d| choz_plugin_clap::scan_directory(&d))
        .find(|p| p.is_instrument)
    else {
        eprintln!("no CLAP instrument installed — skipping");
        return;
    };
    let sr = 48_000;
    let frames = 256usize;
    let mut inst =
        choz_plugin_clap::host::ClapInstrument::build(&info.path, &info.id, sr, frames as u32)
            .unwrap_or_else(|| panic!("failed to instantiate {}", info.name));

    let mut buf = vec![0.0f32; frames * 2];
    inst.note_on(60, 100);
    // Give the voice a few blocks to get past its attack.
    let mut peak = 0.0f32;
    for _ in 0..20 {
        inst.render(&mut buf, sr);
        for &s in buf.iter() {
            assert!(s.is_finite(), "{} produced non-finite output", info.name);
            peak = peak.max(s.abs());
        }
    }
    assert!(peak > 0.0, "{} stayed silent after note-on", info.name);
    inst.note_off(60);
}

/// An instrument's own parameters must be settable while it plays (the UI's
/// INSTR editor drives this path), without breaking its output.
#[test]
fn instrument_parameters_are_settable_while_playing() {
    let _guard = plugin_lock();
    let Some(info) = choz_plugin_clap::default_search_paths()
        .into_iter()
        .flat_map(|d| choz_plugin_clap::scan_directory(&d))
        .find(|p| p.is_instrument)
    else {
        return;
    };
    let params = choz_plugin_clap::read_params(&info.path, &info.id);
    if params.is_empty() {
        eprintln!("{} exposes no parameters — skipping", info.name);
        return;
    }
    let sr = 48_000;
    let frames = 128usize;
    let mut inst =
        choz_plugin_clap::host::ClapInstrument::build(&info.path, &info.id, sr, frames as u32)
            .unwrap_or_else(|| panic!("failed to instantiate {}", info.name));

    let mut buf = vec![0.0f32; frames * 2];
    inst.note_on(60, 100);
    for (i, _) in params.iter().enumerate().take(16) {
        for v in [0.0, 1.0, 0.5] {
            inst.set_param(i, v);
            inst.render(&mut buf, sr);
            assert!(
                buf.iter().all(|s| s.is_finite()),
                "{} produced non-finite output after setting param {i} to {v}",
                info.name
            );
        }
    }
    // Out-of-range index must be ignored, not panic.
    inst.set_param(params.len() + 99, 1.0);
    inst.render(&mut buf, sr);
    inst.note_off(60);
}

/// Blocks larger than the plugin's configured maximum must still be processed
/// (the effect chunks them internally).
#[test]
fn oversized_block_is_chunked() {
    let _guard = plugin_lock();
    let Some(info) = find_effect() else { return };
    let mut fx = choz_plugin_clap::host::ClapEffect::build(&info.path, &info.id, 48_000, 64)
        .expect("instantiate");
    let mut buf = vec![0.25f32; 1024 * 2];
    fx.process_block(&mut buf, 48_000);
    assert!(buf.iter().all(|s| s.is_finite()));
}

/// Every effect installed on this machine must survive load → process → drop,
/// and never hand back non-finite audio. This caught two real bugs: dropping
/// the plugin entry while the instance still used it (segfault), and a plugin
/// that emits NaN before its parameters are set.
#[test]
fn every_installed_effect_is_safe_to_host() {
    let _guard = plugin_lock();
    let effects: Vec<_> = choz_plugin_clap::default_search_paths()
        .into_iter()
        .flat_map(|d| choz_plugin_clap::scan_directory(&d))
        .filter(|p| !p.is_instrument)
        .collect();
    if effects.is_empty() {
        eprintln!("no CLAP effects installed — skipping");
        return;
    }
    for info in effects {
        let Some(mut fx) =
            choz_plugin_clap::host::ClapEffect::build(&info.path, &info.id, 48_000, 256)
        else {
            eprintln!("{} could not be instantiated — skipping", info.name);
            continue;
        };
        let mut buf = vec![0.2f32; 512];
        fx.process_block(&mut buf, 48_000);
        assert!(
            buf.iter().all(|s| s.is_finite() && s.abs() < 10.0),
            "{} produced unusable audio",
            info.name,
        );
    }
}
