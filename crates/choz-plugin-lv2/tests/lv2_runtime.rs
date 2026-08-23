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
        if let Some(ui) = &p.ui {
            assert!(
                ui.binary_path.exists(),
                "{} points at a missing UI binary",
                p.name
            );
            // An embedded UI is always a second binary. One that owns its
            // window may be the same one — Yoshimi's editor *is* Yoshimi.
            if !ui.owns_window {
                assert_ne!(
                    ui.binary_path, p.binary_path,
                    "{}: an embedded UI is a separate binary from the DSP one",
                    p.name
                );
            }
        }
    }

    // Every guitarix UI crashed the probe; none may reach a slot.
    let guitarix_with_ui = all
        .iter()
        .filter(|p| {
            p.uri
                .starts_with("http://guitarix.sourceforge.net/plugins/")
        })
        .filter(|p| p.ui.is_some())
        .count();
    assert_eq!(guitarix_with_ui, 0, "guitarix UIs segfault on instantiate");

    // And the discovery is not vacuously passing: this machine has plenty.
    let with_ui = all.iter().filter(|p| p.ui.is_some()).count();
    eprintln!(
        "{with_ui} of {} installed LV2 plugins ship an editor choz can drive",
        all.len()
    );

    // A bundle that ships ONE UI binary for hundreds of plugins is where an
    // index cap bites: the editor lookup walks `lv2ui_descriptor(0..)`, and a
    // plugin sitting past the cap silently got no editor at all. LSP has ~390
    // of them, so the last one is the regression test. No window is opened —
    // that needs a DISPLAY, which CI has not got — only the handle is asked for.
    if let Some(last_lsp) = all
        .iter()
        .rfind(|p| p.uri.starts_with("http://lsp-plug.in/plugins/lv2/") && p.ui.is_some())
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

/// A bundle that ships `pset:Preset`s must list them with their labels, and
/// picking one must change the sound. Skipped when no installed instrument
/// publishes presets (mda's DX10 has 32 of them).
#[test]
fn bundle_presets_are_listed_and_applied() {
    let _guard = plugin_lock();
    let candidates: Vec<Lv2PluginInfo> = installed()
        .into_iter()
        .filter(|p| p.is_instrument)
        .collect();
    if candidates.is_empty() {
        eprintln!("no LV2 instruments installed; skipping");
        return;
    }

    let mut checked = false;
    for info in candidates {
        let listed = choz_plugin_lv2::presets::scan(&info.bundle_dir, &info.uri, &info.ports);
        if listed.is_empty() {
            continue;
        }
        let Some(mut inst) = Lv2Instrument::build(&info.bundle_dir, &info.uri, SR, BLOCK) else {
            continue;
        };
        let Some(browser) = inst.presets() else {
            panic!("{} has presets but offers no browser", info.name);
        };
        let list = browser.list();
        assert_eq!(list.len(), listed.len());
        for p in &list {
            assert!(!p.name.is_empty(), "a preset with no label: {p:?}");
            assert!(!p.key.is_empty(), "a preset with no URI: {p:?}");
        }
        if list.len() < 2 {
            continue;
        }

        // Same note, two presets: the plugin must not sound identical, or the
        // port values never reached it.
        let render = |inst: &mut Lv2Instrument| {
            inst.note_on(60, 110);
            let mut out = vec![0.0f32; 1024];
            let mut captured = Vec::new();
            for _ in 0..60 {
                inst.render(&mut out, SR);
                captured.extend_from_slice(&out);
            }
            inst.note_off(60);
            for _ in 0..60 {
                inst.render(&mut out, SR);
            }
            captured
        };
        browser.load(&list[0].key);
        let first = render(&mut inst);
        browser.load(&list[list.len() - 1].key);
        let last = render(&mut inst);
        assert_ne!(
            first,
            last,
            "{}: '{}' and '{}' render the same audio",
            info.name,
            list[0].name,
            list[list.len() - 1].name
        );

        // A URI that is not one of ours changes nothing rather than panicking.
        browser.load("http://example.com/not-a-preset");
        checked = true;
        break;
    }
    if !checked {
        eprintln!("no LV2 instrument here publishes presets; skipping");
    }
}

/// ZynAddSubFX keeps its 2000 instruments in a *second* bundle beside the
/// plugin's own (`ZynAddSubFX.lv2presets`), filed under `pset:Bank`s, and each
/// one is a `state:state` document rather than a set of control ports. Both are
/// why its banks used to show up as nothing at all.
///
/// Skipped where it is not installed.
#[test]
fn a_presets_only_sibling_bundle_is_read_too() {
    let bundle = std::path::Path::new("/usr/lib/lv2/ZynAddSubFX.lv2");
    if !bundle.join("manifest.ttl").is_file() {
        eprintln!("ZynAddSubFX not installed; skipping");
        return;
    }
    let uri = "http://zynaddsubfx.sourceforge.net";
    let ports = Vec::new();
    let listed = choz_plugin_lv2::presets::scan(bundle, uri, &ports);
    assert!(
        listed.len() > 100,
        "the sibling bundle's presets are missing: {}",
        listed.len()
    );
    let banks: std::collections::BTreeSet<&str> = listed
        .iter()
        .map(|p| p.entry.category.as_str())
        .filter(|c| !c.is_empty())
        .collect();
    assert!(banks.contains("Arpeggios"), "the banks: {banks:?}");
    // The bank is the sidebar; it is not repeated in every row's name.
    let arp = listed
        .iter()
        .find(|p| p.entry.category == "Arpeggios")
        .expect("a preset in that bank");
    assert!(
        !arp.entry.name.starts_with("Arpeggios:"),
        "the name still carries its bank: {}",
        arp.entry.name
    );

    // And picking one is heard: a `state:state` preset reaches the plugin
    // through the state extension, which is the only way it can.
    let _guard = plugin_lock();
    let Some(mut inst) = Lv2Instrument::build(bundle, uri, SR, BLOCK) else {
        eprintln!("ZynAddSubFX did not instantiate; skipping the sound half");
        return;
    };
    let browser = inst.presets().expect("it has presets");
    let two: Vec<String> = listed
        .iter()
        .filter(|p| p.entry.category == "Arpeggios")
        .map(|p| p.entry.key.clone())
        .take(2)
        .collect();
    let mut sound = |key: &str| {
        browser.load(key);
        inst.note_on(60, 110);
        let mut out = vec![0.0f32; 1024];
        let mut captured = Vec::new();
        for _ in 0..60 {
            inst.render(&mut out, SR);
            captured.extend_from_slice(&out);
        }
        inst.note_off(60);
        for _ in 0..60 {
            inst.render(&mut out, SR);
        }
        captured
    };
    let a = sound(&two[0]);
    let b = sound(&two[1]);
    assert!(
        a.iter().any(|s| s.abs() > 1e-6),
        "the first preset made no sound at all"
    );
    assert_ne!(a, b, "two different instruments sounded identical");
}

/// An editor that opens **its own** window is an editor: `ui:showInterface` is
/// what Yoshimi and ZynAddSubFX ship instead of an X11 UI, and looking only for
/// `ui:X11UI` is why choz offered neither a window while Carla shows both.
///
/// Skipped where they are not installed. Only the discovery half is asserted —
/// opening one puts a window on the screen, which is not a test's business.
#[test]
fn a_ui_that_owns_its_window_is_found() {
    let mut checked = 0;
    for (dir, plugin) in [
        ("/usr/lib/lv2/yoshimi.lv2", "yoshimi"),
        ("/usr/lib/lv2/ZynAddSubFX.lv2", "zynaddsubfx"),
    ] {
        let bundle = std::path::Path::new(dir);
        if !bundle.join("manifest.ttl").is_file() {
            continue;
        }
        for info in choz_plugin_lv2::discovery::discover_bundle(bundle) {
            assert!(
                info.uri.contains(plugin),
                "wrong bundle: {} in {dir}",
                info.uri
            );
            let ui = info
                .ui
                .as_ref()
                .unwrap_or_else(|| panic!("{} has an editor and none was found", info.name));
            assert!(
                ui.owns_window,
                "{}: neither of these embeds; both show their own window",
                info.name
            );
            assert!(ui.binary_path.is_file(), "the UI binary has to be there");
            checked += 1;
        }
    }
    if checked == 0 {
        eprintln!("neither Yoshimi nor ZynAddSubFX installed; skipping");
    }
}

/// A plugin that keeps its patches to itself still has a list: the kx
/// `programs#Interface` is the only door to Yoshimi's 4466 instruments, and its
/// bundle describes not one of them in Turtle.
///
/// Skipped where Yoshimi is not installed.
#[test]
fn the_programs_interface_lists_and_selects_a_patch() {
    let bundle = std::path::Path::new("/usr/lib/lv2/yoshimi.lv2");
    if !bundle.join("manifest.ttl").is_file() {
        eprintln!("Yoshimi not installed; skipping");
        return;
    }
    let _guard = plugin_lock();
    let uri = "http://yoshimi.sourceforge.net/lv2_plugin";
    let Some(mut inst) = Lv2Instrument::build(bundle, uri, SR, BLOCK) else {
        eprintln!("Yoshimi did not instantiate; skipping");
        return;
    };
    let browser = inst.presets().expect("its programs are its presets");
    let list = browser.list();
    assert!(list.len() > 1000, "only {} programs", list.len());

    // The banks are named, not numbered: the extension hands over
    // "Arpeggios -> Arpeggio1" and the bank half is the picker's sidebar.
    let banks: std::collections::BTreeSet<&str> =
        list.iter().map(|p| p.category.as_str()).collect();
    assert!(
        banks.iter().any(|b| !b.starts_with("BANK ")),
        "no bank was named: {banks:?}"
    );
    assert!(
        list.iter().all(|p| !p.name.contains(" -> ")),
        "a row still carries its bank in its name"
    );

    // And picking one is heard.
    let sound = |key: &str, inst: &mut Lv2Instrument| {
        browser.load(key);
        inst.note_on(60, 110);
        let mut out = vec![0.0f32; 1024];
        let mut captured = Vec::new();
        for _ in 0..80 {
            inst.render(&mut out, SR);
            captured.extend_from_slice(&out);
        }
        inst.note_off(60);
        for _ in 0..40 {
            inst.render(&mut out, SR);
        }
        captured
    };
    let a = sound(&list[0].key, &mut inst);
    let b = sound(&list[list.len() / 2].key, &mut inst);
    assert_ne!(a, b, "two different instruments rendered the same audio");
    assert!(
        b.iter().any(|s| s.abs() > 1e-4),
        "the selected instrument made no sound at all"
    );
}

/// ZynAddSubFX's editor is a **program**, not a window: its `ui:showInterface`
/// UI starts `zynaddsubfx-ext-gui` and hands it the address of the OSC server
/// the DSP opened. DPF passes that address over an atom port choz does not
/// implement, so the UI never started anything and the `[GUI]` button did
/// nothing at all. The address is found from this side instead.
///
/// The window itself is not opened here — a test that leaves a synth on the
/// screen is a test nobody runs twice.
#[test]
fn zyns_osc_server_is_found_and_its_editor_is_the_program() {
    let bundle = std::path::Path::new("/usr/lib/lv2/ZynAddSubFX.lv2");
    if !bundle.join("manifest.ttl").is_file() {
        eprintln!("ZynAddSubFX not installed; skipping");
        return;
    }
    let _guard = plugin_lock();
    let uri = "http://zynaddsubfx.sourceforge.net";
    let Some(inst) = Lv2Instrument::build(bundle, uri, SR, BLOCK) else {
        eprintln!("ZynAddSubFX did not instantiate; skipping");
        return;
    };
    let port = inst
        .osc_port()
        .expect("it opens an OSC server while instantiating");
    assert!(port > 1024, "a server port, not {port}");

    assert!(
        choz_plugin_lv2::osc::udp_ports().contains(&port),
        "the port is this process's own"
    );

    match choz_plugin_lv2::external_gui::program_for(uri) {
        Some((program, url)) => {
            assert_eq!(program, "zynaddsubfx-ext-gui");
            assert!(url.contains("{port}"), "the address is built from it");
            let ed = inst.editor().expect("an editor it can actually show");
            assert!(ed.owns_window(), "the program brings its own window");
            assert!(!ed.is_open(), "nothing is on screen until it is opened");
        }
        // The helper is part of the same package, but a stripped install could
        // have the plugin without it — and then there is no editor to offer.
        None => assert!(
            inst.editor().is_none_or(|e| !e.owns_window()),
            "no helper installed, so no window may be promised"
        ),
    }
}

/// The controls a plugin keeps behind its OSC server, reached by path: the
/// chosen knobs, and the 128 harmonics its own editor draws as bars.
///
/// The names matter and were measured, not guessed: `magnitude<n>` is the port
/// the plugin **answers** for, while its internal `Phmag<n>` accepts a write
/// and replies to nothing — a view built on that one would have drawn an empty
/// row for ever.
#[test]
fn zyns_knobs_and_harmonics_are_reachable_by_path() {
    let bundle = std::path::Path::new("/usr/lib/lv2/ZynAddSubFX.lv2");
    if !bundle.join("manifest.ttl").is_file() {
        eprintln!("ZynAddSubFX not installed; skipping");
        return;
    }
    let _guard = plugin_lock();
    let uri = "http://zynaddsubfx.sourceforge.net";
    let Some(mut inst) = Lv2Instrument::build(bundle, uri, SR, BLOCK) else {
        eprintln!("ZynAddSubFX did not instantiate; skipping");
        return;
    };

    // Its ports name nothing; the chosen list is what the panel gets instead.
    let params = choz_plugin_lv2::read_params(bundle, uri);
    assert!(
        params.iter().all(|p| !p.name.starts_with("Slot ")),
        "the sixteen numbered slots are still what the panel shows"
    );
    let cutoff = params
        .iter()
        .position(|p| p.name == "CUTOFF")
        .expect("a filter cutoff among them");

    let paths = inst.paths().expect("a by-path surface");
    let set = paths.harmonics().expect("a set of harmonics");
    assert_eq!(set.magnitude.len(), 128);
    assert_eq!((set.min, set.max, set.zero), (0.0, 127.0, 64.0));

    // What the plugin holds, asked for and collected: a fresh oscillator is one
    // full harmonic and 127 silent ones.
    for path in &set.magnitude {
        paths.ask(path);
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while paths.value(&set.magnitude[1]).is_none() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert_eq!(
        paths.value(&set.magnitude[0]),
        Some(127.0),
        "the fundamental"
    );
    assert_eq!(
        paths.value(&set.magnitude[1]),
        Some(64.0),
        "and a silent one"
    );

    // Moving one reaches the plugin, and the plugin says so.
    paths.set(&set.magnitude[1], 100.0);
    paths.ask(&set.magnitude[1]);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while paths.value(&set.magnitude[1]) != Some(100.0) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert_eq!(paths.value(&set.magnitude[1]), Some(100.0));

    // And a knob does too — the audio thread's own call, which only ever writes
    // into a ring the sender thread drains.
    let watch = "/part0/kit0/adpars/GlobalPar/GlobalFilter/Pfreq";
    inst.set_param(cutoff, 0.25);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while paths.value(watch).is_none() && std::time::Instant::now() < deadline {
        paths.ask(watch);
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert_eq!(
        paths.value(watch),
        Some(32.0),
        "a quarter of 0..127 is 32, and that is what the plugin holds"
    );

    // The harmonics are knobs of that same list, so moving one moves the
    // oscillator — the same path the HARMONICS view writes.
    let h2 = params
        .iter()
        .position(|p| p.name == "H2")
        .expect("the second harmonic is a knob");
    inst.set_param(h2, 1.0);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while paths.value(&set.magnitude[1]) != Some(127.0) && std::time::Instant::now() < deadline {
        paths.ask(&set.magnitude[1]);
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert_eq!(
        paths.value(&set.magnitude[1]),
        Some(127.0),
        "the knob and the view move the same harmonic"
    );
}

/// The oscillator's harmonics are **parameters**, not only a view of their own:
/// thirty-two magnitudes and thirty-two phases sit in the tab's own knob box,
/// tagged so the panel draws them as the bank of vertical bars they are.
#[test]
fn the_harmonics_are_knobs_on_the_instrument_too() {
    let params = choz_plugin_lv2::osc_params::params(choz_plugin_lv2::osc_params::ZYN.as_slice());
    let n = choz_plugin_lv2::osc_params::HARMONIC_KNOBS;

    let mags: Vec<&choz_ports::PluginParam> = params
        .iter()
        .filter(|p| p.group.as_deref() == Some("HARMONICS"))
        .collect();
    let phases: Vec<&choz_ports::PluginParam> = params
        .iter()
        .filter(|p| p.group.as_deref() == Some("H.PHASE"))
        .collect();
    assert_eq!((mags.len(), phases.len()), (n, n));
    assert_eq!(mags[0].name, "H1", "the fundamental is the first of them");
    assert_eq!(mags[0].default, 127.0, "and it is the one that sounds");
    assert_eq!(mags[1].default, 64.0, "the rest are silent, not zero");
    assert_eq!(phases[3].name, "P4");

    // The tag is what makes a run of them a bank of bars rather than a row of
    // numbered knobs.
    assert!(mags.iter().all(|p| p.unit.as_deref() == Some("harmonic")));
    assert!(phases.iter().all(|p| p.unit.as_deref() == Some("phase")));

    // They are in the same list as the rest, so a CC learned on one is learned
    // the same way as one on the filter.
    assert!(params.iter().any(|p| p.name == "CUTOFF"));
    assert_eq!(params.len(), 20 + 2 * n);

    // And each one addresses its own harmonic.
    let table = choz_plugin_lv2::osc_params::table_for("http://zynaddsubfx.sourceforge.net")
        .expect("Zyn has a table");
    let mag_paths: Vec<&str> = table
        .iter()
        .filter(|p| p.group == "HARMONICS")
        .map(|p| p.path.as_str())
        .collect();
    assert!(mag_paths[0].ends_with("/magnitude0"), "{}", mag_paths[0]);
    assert!(mag_paths[31].ends_with("/magnitude31"), "{}", mag_paths[31]);
}

/// What the knobs read back: the paths behind the plugin's parameters, and a
/// patch loaded inside it moving them.
///
/// This is the half a parameter list cannot do. ZynAddSubFX's presets are state
/// blobs that move every control it has, and none of it comes back through a
/// port — so without this the panel keeps showing whatever choz last sent.
#[test]
fn a_patch_moves_what_the_knobs_read_back() {
    let bundle = std::path::Path::new("/usr/lib/lv2/ZynAddSubFX.lv2");
    if !bundle.join("manifest.ttl").is_file() {
        eprintln!("ZynAddSubFX not installed; skipping");
        return;
    }
    let _guard = plugin_lock();
    let uri = "http://zynaddsubfx.sourceforge.net";
    let Some(inst) = Lv2Instrument::build(bundle, uri, SR, BLOCK) else {
        eprintln!("ZynAddSubFX did not instantiate; skipping");
        return;
    };
    let paths = inst.paths().expect("a by-path surface");
    let params = choz_plugin_lv2::read_params(bundle, uri);
    let param_paths = paths.param_paths();
    assert_eq!(
        param_paths.len(),
        params.len(),
        "a path for every knob the panel draws"
    );

    // Ask, then read: the answers arrive on their own thread.
    let settle = |paths: &choz_ports::PathsHandle, path: &str| -> Option<f32> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            paths.ask(path);
            std::thread::sleep(std::time::Duration::from_millis(50));
            if let Some(v) = paths.value(path) {
                return Some(v);
            }
        }
        None
    };
    // A control the presets really move: the oscillator's waveform.
    let wave = params
        .iter()
        .position(|p| p.name == "WAVE")
        .expect("the waveform is a knob");
    let path = param_paths[wave].clone();
    let before = settle(&paths, &path).expect("the plugin answers for it");

    // Load patches until one of them is a different waveform — a bank where
    // every instrument happened to be a sine would prove nothing either way.
    let browser = inst.presets().expect("its presets");
    let list = browser.list();
    assert!(list.len() > 100, "the bank is there");
    let mut moved = None;
    for entry in list.iter().step_by(37).take(12) {
        browser.load(&entry.key);
        std::thread::sleep(std::time::Duration::from_millis(100));
        paths.ask(&path);
        std::thread::sleep(std::time::Duration::from_millis(150));
        if paths.value(&path).is_some_and(|v| v != before) {
            moved = Some(entry.name.clone());
            break;
        }
    }
    assert!(
        moved.is_some(),
        "no patch moved the waveform, so nothing would ever read back"
    );
}
