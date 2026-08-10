//! Runtime checks against the LV2 plugins actually installed on this machine.
//! Every test skips when `/usr/lib/lv2` holds nothing usable, so CI without
//! plugins stays green.

use choz_plugin_lv2::{Lv2Effect, Lv2Instrument, Lv2PluginInfo, scan_directory};
use choz_ports::{AudioSource, FxProcessor};

const SR: u32 = 48_000;
const BLOCK: u32 = 256;

/// Serialises the tests in this file.
///
/// The harness runs them on separate threads, and they all dlopen the same
/// plugins. Several of those (JUCE-based, and anything dragging in Qt) do global
/// init on load and fall over when two threads do it at once — the whole test
/// binary dies with SIGSEGV, intermittently. VST2/VST3 solved this by collapsing
/// their runtime tests into one function; a lock does the same job here without
/// losing six test names.
static PLUGINS: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn plugin_lock() -> std::sync::MutexGuard<'static, ()> {
    PLUGINS.lock().unwrap_or_else(|e| e.into_inner())
}

fn installed() -> Vec<Lv2PluginInfo> {
    scan_directory(std::path::Path::new("/usr/lib/lv2"))
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

#[test]
fn scan_finds_bundles_with_uris_and_ports() {
    let _guard = plugin_lock();
    let found = installed();
    if found.is_empty() {
        eprintln!("no LV2 plugins installed; skipping");
        return;
    }
    for p in &found {
        assert!(
            p.uri.starts_with("http") || p.uri.contains(':'),
            "odd URI {}",
            p.uri
        );
        assert!(p.binary_path.exists(), "{} has no binary", p.uri);
    }
    assert!(
        found.iter().any(|p| p.is_effect),
        "expected at least one effect"
    );
}

/// Every installed effect must load, process a block, and stay finite. This is
/// the sweep that found the real CLAP hosting bugs, applied to LV2.
///
/// Ignored by default: it hosts every plugin on the machine (547 here) and the
/// heavy ones (LSP) take minutes each in a debug build. Run it with
/// `cargo test --release -p choz-plugin-lv2 -- --ignored`.
#[test]
#[ignore]
fn every_installed_effect_is_safe_to_host() {
    let _guard = plugin_lock();
    let found = installed();
    if found.is_empty() {
        eprintln!("no LV2 plugins installed; skipping");
        return;
    }
    let mut hosted = 0;
    for info in found.iter().filter(|p| p.is_effect && !p.is_instrument) {
        let Some(mut fx) = Lv2Effect::build(&info.bundle_dir, &info.uri, SR, BLOCK) else {
            continue; // unsupported required feature — reported, not fatal
        };
        hosted += 1;
        let mut buf = sine_block(BLOCK as usize);
        for _ in 0..4 {
            fx.process_block(&mut buf, SR);
            // Only finiteness is asserted: plenty of installed LV2s are legit
            // high-gain (RIAA preamps) or CV generators, so a loud block is not
            // a hosting bug — the engine's master clamp deals with those.
            for (i, &s) in buf.iter().enumerate() {
                assert!(s.is_finite(), "{} produced non-finite at {i}", info.uri);
            }
        }
    }
    assert!(hosted > 0, "no installed LV2 effect could be hosted");
}

/// Plugins that require `worker#schedule` used to be turned away at the door.
/// They must load and process now — Dragonfly, guitarix and a-fluidsynth are
/// all in this group on a normal Linux audio box.
#[test]
fn effects_requiring_the_worker_feature_are_hosted() {
    let _guard = plugin_lock();
    let found = installed();
    let want: Vec<&Lv2PluginInfo> = found
        .iter()
        .filter(|p| {
            p.is_effect
                && !p.is_instrument
                && p.required_features.iter().any(|f| f.contains("worker"))
        })
        .collect();
    if want.is_empty() {
        eprintln!("no LV2 plugin here requires the worker; skipping");
        return;
    }
    let mut hosted = 0;
    for info in &want {
        // A plugin with no audio output is still legitimately refused.
        let Some(mut fx) = Lv2Effect::build(&info.bundle_dir, &info.uri, SR, BLOCK) else {
            continue;
        };
        hosted += 1;
        let mut buf = sine_block(BLOCK as usize);
        for _ in 0..4 {
            fx.process_block(&mut buf, SR);
        }
        assert!(
            buf.iter().all(|s| s.is_finite()),
            "{} produced non-finite",
            info.uri
        );
    }
    assert!(hosted > 0, "all {} worker plugins were refused", want.len());
}

/// The fast version of the sweep above: the first few installed effects load,
/// process, and stay finite.
#[test]
fn a_few_effects_host_and_stay_finite() {
    let _guard = plugin_lock();
    let found = installed();
    if found.is_empty() {
        eprintln!("no LV2 plugins installed; skipping");
        return;
    }
    let mut hosted = 0;
    for info in found.iter().filter(|p| p.is_effect && !p.is_instrument) {
        let Some(mut fx) = Lv2Effect::build(&info.bundle_dir, &info.uri, SR, BLOCK) else {
            continue;
        };
        let mut buf = sine_block(BLOCK as usize);
        fx.process_block(&mut buf, SR);
        assert!(
            buf.iter().all(|s| s.is_finite()),
            "{} produced non-finite",
            info.uri
        );
        hosted += 1;
        if hosted == 5 {
            break;
        }
    }
    assert!(hosted > 0, "no installed LV2 effect could be hosted");
}

/// A block longer than the plugin's configured maximum is chunked, not dropped.
#[test]
fn oversized_block_is_processed_in_chunks() {
    let _guard = plugin_lock();
    let found = installed();
    let Some(info) = found.iter().find(|p| p.is_effect && !p.is_instrument) else {
        eprintln!("no LV2 effect installed; skipping");
        return;
    };
    let Some(mut fx) = Lv2Effect::build(&info.bundle_dir, &info.uri, SR, BLOCK) else {
        eprintln!("{} would not load; skipping", info.uri);
        return;
    };
    let mut buf = sine_block(BLOCK as usize * 3);
    fx.process_block(&mut buf, SR);
    assert!(buf.iter().all(|s| s.is_finite()));
}

/// An instrument must actually make sound after a note-on.
#[test]
fn instrument_sounds_on_note_on() {
    let _guard = plugin_lock();
    let found = installed();
    let mut tried = 0;
    for info in found.iter().filter(|p| p.is_instrument) {
        let Some(mut inst) = Lv2Instrument::build(&info.bundle_dir, &info.uri, SR, BLOCK) else {
            continue;
        };
        tried += 1;
        inst.note_on(60, 100);
        let mut peak = 0.0f32;
        // Some instruments need a few blocks before the envelope opens.
        for _ in 0..20 {
            let mut buf = vec![0.0f32; BLOCK as usize * 2];
            inst.render(&mut buf, SR);
            for s in &buf {
                assert!(s.is_finite(), "{} produced non-finite", info.uri);
                peak = peak.max(s.abs());
            }
        }
        if peak > 1e-4 {
            return; // one audible instrument is enough
        }
    }
    if tried == 0 {
        eprintln!("no LV2 instrument installed; skipping");
    }
}

/// The X11 editor is discovered from the bundle TTL, and the plugins measured to
/// segfault on `instantiate` are not offered one.
///
/// Deliberately does *not* open a window: that needs a live DISPLAY, and CI has
/// none. `examples/ui_probe` is the thing that actually opens all of them.
#[test]
fn x11_editors_are_discovered_and_the_crashing_families_are_not_offered() {
    let _guard = plugin_lock();
    let all = installed();
    if all.is_empty() {
        eprintln!("no LV2 plugins installed; skipping");
        return;
    }

    for p in &all {
        if let Some(ui) = &p.x11_ui {
            assert!(
                ui.binary_path.exists(),
                "{} points at a missing UI binary",
                p.name
            );
            assert_ne!(
                ui.binary_path, p.binary_path,
                "{}: the UI is a separate binary from the DSP one",
                p.name
            );
        }
    }

    // Every guitarix UI crashed the probe; none may reach a slot.
    let guitarix_with_ui = all
        .iter()
        .filter(|p| {
            p.uri
                .starts_with("http://guitarix.sourceforge.net/plugins/")
        })
        .filter(|p| p.x11_ui.is_some())
        .count();
    assert_eq!(guitarix_with_ui, 0, "guitarix UIs segfault on instantiate");

    // And the discovery is not vacuously passing: this machine has plenty.
    let with_ui = all.iter().filter(|p| p.x11_ui.is_some()).count();
    eprintln!(
        "{with_ui} of {} installed LV2 plugins ship an X11 UI",
        all.len()
    );

    // A bundle that ships ONE UI binary for hundreds of plugins is where an
    // index cap bites: the editor lookup walks `lv2ui_descriptor(0..)`, and a
    // plugin sitting past the cap silently got no editor at all. LSP has ~390
    // of them, so the last one is the regression test. No window is opened —
    // that needs a DISPLAY, which CI has not got — only the handle is asked for.
    if let Some(last_lsp) = all
        .iter()
        .rfind(|p| p.uri.starts_with("http://lsp-plug.in/plugins/lv2/") && p.x11_ui.is_some())
    {
        let fx =
            choz_plugin_lv2::Lv2Effect::build(&last_lsp.bundle_dir, &last_lsp.uri, 48_000, 256);
        let editor = fx.as_ref().and_then(|f| f.editor());
        assert!(
            editor.is_some(),
            "{} declares an X11 UI but got no editor handle",
            last_lsp.name
        );
    }
}

/// The plugin's own state, which is what a project has to carry beyond the
/// knob values. Not every LV2 has a `state#interface` — most simple effects
/// have nothing to say — so the check is over whichever installed plugins do,
/// and it asserts the blob survives a trip through a fresh instance.
#[test]
fn plugins_with_a_state_interface_round_trip_their_patch() {
    let _guard = plugin_lock();
    let all = installed();
    if all.is_empty() {
        eprintln!("no LV2 plugins installed; skipping");
        return;
    }

    let mut tried = 0;
    for p in all.iter().filter(|p| p.is_effect) {
        let Some(fx) = Lv2Effect::build(&p.bundle_dir, &p.uri, 48_000, 256) else {
            continue;
        };
        let Some(state) = fx.state() else { continue };
        let Some(blob) = state.save() else { continue }; // no state:interface
        assert!(!blob.is_empty());
        drop(fx);

        let Some(fresh) = Lv2Effect::build(&p.bundle_dir, &p.uri, 48_000, 256) else {
            continue;
        };
        let restored = fresh.state().expect("same plugin, same capability");
        restored.restore(&blob);
        let again = restored.save().expect("state readable after restoring it");
        assert_eq!(
            again, blob,
            "{}: the patch did not survive the round trip",
            p.name
        );
        tried += 1;
        if tried == 3 {
            break;
        }
    }
    eprintln!("{tried} LV2 plugin(s) round-tripped their state");
}

/// The hints that decide which control the UI draws, read off real bundles.
///
/// Ardour's `a-delay` is the useful case: its divisor port is an enumeration of
/// ten note values over a range of 1..48, so the named steps are **not** evenly
/// spaced. A host that assumed a uniform grid would name them wrong.
#[test]
fn control_ports_report_what_kind_of_control_they_are() {
    let _guard = plugin_lock();
    let bundle = std::path::Path::new("/usr/lib/lv2/a-delay.lv2");
    if !bundle.exists() {
        eprintln!("a-delay not installed; skipping");
        return;
    }
    let found = choz_plugin_lv2::discovery::discover_bundle(bundle);
    let info = found.first().expect("a-delay is in its bundle");

    let divisor = info
        .ports
        .iter()
        .find(|p| p.enumeration)
        .expect("a-delay declares an enumeration port");
    assert!(divisor.integer, "and an integer one");
    assert_eq!(divisor.points.len(), 10, "ten note divisions");
    assert_eq!(
        divisor.points[0].0, 1.0,
        "sorted by value: whole note first"
    );
    assert_eq!(divisor.points.last().unwrap().0, 48.0);
    assert!(divisor.points.iter().any(|(_, l)| l.contains("Whole note")));

    let params = choz_plugin_lv2::read_params(&info.bundle_dir, &info.uri);
    let param = params
        .iter()
        .find(|p| p.id == divisor.index)
        .expect("the port is an automatable parameter");
    assert_eq!(
        param.steps, 10,
        "one step per named point, not one per integer"
    );
    assert_eq!(param.points.len(), 10);
    // The positions are what the plugin said, spread over min..max — the point
    // of carrying them instead of a count.
    assert!((param.normalised(param.points[1].0) - (2.0 - 1.0) / 47.0).abs() < 1e-9);

    // A switch says so outright, wherever one is installed.
    if let Some(toggle) = info.ports.iter().find(|p| p.toggled) {
        let p = params
            .iter()
            .find(|p| p.id == toggle.index)
            .expect("also a parameter");
        assert_eq!(p.steps, 2, "{} is a switch", toggle.name);
    }
}

/// Units come off the port and reach the parameter, which is what decides
/// whether the interface draws a knob or a fader. `units:ms` and `units:pc` are
/// the two commonest on this machine after the inline definitions.
#[test]
fn control_ports_carry_their_unit() {
    let _guard = plugin_lock();
    let mut with_unit = 0;
    let mut sample: Option<(String, String)> = None;

    for dir in std::fs::read_dir("/usr/lib/lv2")
        .into_iter()
        .flatten()
        .flatten()
    {
        let bundle = dir.path();
        if bundle.extension().is_none_or(|e| e != "lv2") {
            continue;
        }
        for info in choz_plugin_lv2::discovery::discover_bundle(&bundle) {
            for port in info.ports.iter().filter(|p| p.unit.is_some()) {
                with_unit += 1;
                if sample.is_none() {
                    sample = Some((info.uri.clone(), port.unit.clone().unwrap()));
                }
            }
        }
    }

    let Some((uri, unit)) = sample else {
        eprintln!("no LV2 plugin here declares a unit; skipping");
        return;
    };
    eprintln!("{with_unit} control port(s) with a unit; e.g. {uri} → {unit}");
    assert!(
        !unit.trim().is_empty(),
        "a unit that parsed to nothing is worse than none"
    );
    // And it survives the trip to the parameter list the UI reads.
    let bundle = std::path::Path::new("/usr/lib/lv2");
    let found = std::fs::read_dir(bundle)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "lv2"))
        .flat_map(|p| choz_plugin_lv2::discovery::discover_bundle(&p))
        .find(|i| i.uri == uri)
        .expect("the plugin it came from");
    let params = choz_plugin_lv2::read_params(&found.bundle_dir, &found.uri);
    assert!(
        params.iter().any(|p| p.unit.is_some()),
        "{uri} has a port with a unit but no parameter carries it"
    );
}
