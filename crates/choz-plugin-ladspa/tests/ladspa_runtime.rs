//! Runtime checks against the LADSPA/DSSI plugins installed on this machine.
//! Each test skips when nothing is installed, so CI stays green without them.

use choz_plugin_ladspa::{scan_directory, DssiInstrument, LadspaEffect, PluginInfo};
use choz_ports::{AudioSource, FxProcessor};

const SR: u32 = 48_000;
const BLOCK: u32 = 256;

/// Serialises every test that dlopens a plugin.
///
/// The harness runs test *functions* in parallel and these all load the same
/// shared objects. Several of them do global initialisation on load — WhySynth
/// and hexter bring GTK in, FluidSynth-DSSI brings fluidsynth's own — and two
/// threads doing it at once takes the whole test binary down with a SIGSEGV,
/// intermittently. The LV2, CLAP and VST3 suites carry the same lock for the
/// same reason; this one grew a second DSSI test and started needing it too.
fn plugin_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn ladspa() -> Vec<PluginInfo> {
    scan_directory(std::path::Path::new("/usr/lib/ladspa"))
}

fn dssi() -> Vec<PluginInfo> {
    scan_directory(std::path::Path::new("/usr/lib/dssi"))
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
fn scan_reports_labels_and_ports() {
    let found = ladspa();
    if found.is_empty() {
        eprintln!("no LADSPA plugins installed; skipping");
        return;
    }
    for p in &found {
        assert!(!p.label.is_empty(), "{} has no label", p.path.display());
    }
    assert!(
        found.iter().any(|p| p.audio_outputs > 0),
        "no plugin has an audio output"
    );
    assert!(
        found.iter().any(|p| !p.params.is_empty()),
        "no plugin exposes parameters"
    );
}

/// The first few installed effects load, process, and stay finite.
#[test]
fn a_few_effects_host_and_stay_finite() {
    let _guard = plugin_lock();
    let found = ladspa();
    if found.is_empty() {
        eprintln!("no LADSPA plugins installed; skipping");
        return;
    }
    let mut hosted = 0;
    for info in found
        .iter()
        .filter(|p| p.audio_outputs > 0 && p.audio_inputs > 0)
    {
        let Some(mut fx) = LadspaEffect::build(&info.path, &info.label, SR, BLOCK) else {
            continue;
        };
        let mut buf = sine_block(BLOCK as usize);
        fx.process_block(&mut buf, SR);
        assert!(
            buf.iter().all(|s| s.is_finite()),
            "{} produced non-finite",
            info.label
        );
        hosted += 1;
        if hosted == 5 {
            break;
        }
    }
    assert!(hosted > 0, "no installed LADSPA effect could be hosted");
}

/// The full sweep. Ignored by default (hundreds of dlopens); run with
/// `cargo test --release -p choz-plugin-ladspa -- --ignored`.
#[test]
#[ignore]
fn every_installed_effect_is_safe_to_host() {
    let _guard = plugin_lock();
    let found = ladspa();
    if found.is_empty() {
        eprintln!("no LADSPA plugins installed; skipping");
        return;
    }
    let mut hosted = 0;
    for info in found
        .iter()
        .filter(|p| p.audio_outputs > 0 && p.audio_inputs > 0)
    {
        let Some(mut fx) = LadspaEffect::build(&info.path, &info.label, SR, BLOCK) else {
            continue;
        };
        hosted += 1;
        let mut buf = sine_block(BLOCK as usize);
        for _ in 0..4 {
            fx.process_block(&mut buf, SR);
            for &s in buf.iter() {
                assert!(s.is_finite(), "{} produced non-finite", info.label);
            }
        }
    }
    assert!(hosted > 0);
}

/// Everything DSSI, in **one** test function and **one** build per plugin, on
/// purpose.
///
/// FluidSynth-DSSI cannot be instantiated twice in one process: it drags in
/// libinstpatch, whose GLib type registration does not survive the first
/// instance being torn down, and the second build segfaults. (The VST3 suite is
/// one function for the neighbouring reason.) So each plugin is built once and
/// every question is asked of that instance: does it sound, does it list its
/// programs, does picking one change the audio.
///
/// The three installed here cover the format's two shapes: hexter and WhySynth
/// carry their patches inside (128 and 397), and FluidSynth-DSSI has **none**
/// until `configure` gives it a SoundFont — which is why the list is read again
/// after every configure.
#[test]
fn dssi_instruments_sound_and_switch_programs() {
    let _guard = plugin_lock();
    let sf2 = std::path::Path::new("/usr/share/sounds/sf2/FluidR3_GM.sf2");
    let mut built = 0;
    let mut sounded = 0;
    let mut with_programs = 0;

    for info in dssi().iter().filter(|p| p.is_instrument) {
        let Some(mut inst) = DssiInstrument::build(&info.path, &info.label, SR, BLOCK) else {
            continue;
        };
        built += 1;

        // The one plugin here that arrives empty: no SoundFont, no programs and
        // no sound. Giving it one is what the rest of the checks then use.
        if info.label.contains("FluidSynth") && sf2.exists() {
            assert!(inst.presets().is_none(), "no SoundFont, no programs");
            let complaint = inst.configure("load", &sf2.to_string_lossy());
            assert!(complaint.is_none(), "{}: {complaint:?}", info.label);
        }

        // A synth that has a program list starts silent until one is picked —
        // WhySynth does exactly that — so the programs come first.
        let browser = inst.presets();
        if let Some(browser) = &browser {
            let list = browser.list();
            assert!(list.len() > 1, "{}: {} program(s)", info.label, list.len());
            for p in list.iter().take(20) {
                assert!(!p.name.is_empty(), "{}: a program with no name", info.label);
                assert!(
                    p.key.split_once(':').is_some_and(|(b, pr)| {
                        b.parse::<u32>().is_ok() && pr.parse::<u32>().is_ok()
                    }),
                    "{}: key is not bank:program — {p:?}",
                    info.label
                );
            }

            // Two programs, same note: the audio has to differ, which is only
            // true if the request crossed into the audio thread and was
            // selected there.
            browser.load(&list[0].key);
            let first = render_note(&mut inst, &info.label);
            browser.load(&list[list.len() / 2].key);
            let other = render_note(&mut inst, &info.label);
            assert_ne!(
                first,
                other,
                "{}: '{}' and '{}' render the same audio",
                info.label,
                list[0].name,
                list[list.len() / 2].name
            );
            // Garbage keys are ignored, not fatal.
            browser.load("not:a:program");
            browser.load("");
            with_programs += 1;
        }

        // …and it has to make a sound at all.
        let peak = render_note(&mut inst, &info.label)
            .iter()
            .fold(0.0f32, |m, s| m.max(s.abs()));
        if peak > 1e-4 {
            sounded += 1;
        }
    }

    if built == 0 {
        eprintln!("no DSSI instrument installed; skipping");
        return;
    }
    assert!(
        sounded > 0,
        "{built} DSSI instrument(s) loaded and none made a sound"
    );
    if with_programs == 0 {
        eprintln!("no DSSI instrument here publishes programs; that half was skipped");
    }
}

/// One note through a synth, with the tail: what two programs are compared by.
fn render_note(inst: &mut DssiInstrument, label: &str) -> Vec<f32> {
    inst.note_on(60, 110);
    let mut captured = Vec::new();
    for _ in 0..40 {
        let mut buf = vec![0.0f32; BLOCK as usize * 2];
        inst.render(&mut buf, SR);
        for s in &buf {
            assert!(s.is_finite(), "{label} produced non-finite");
        }
        captured.extend_from_slice(&buf);
    }
    inst.note_off(60);
    for _ in 0..40 {
        let mut buf = vec![0.0f32; BLOCK as usize * 2];
        inst.render(&mut buf, SR);
    }
    captured
}

/// A port whose positions have names comes back with them.
///
/// LADSPA's ABI cannot say this — a hint gives the *count* and nothing else —
/// so the names come from the metadata files beside the plugins, which is what
/// every other host reads them from too. Skipped when neither the plugins nor
/// their metadata are installed.
#[test]
fn a_ports_named_positions_come_from_the_metadata_beside_the_plugin() {
    let _g = plugin_lock();
    let found = ladspa();
    if found.is_empty() {
        eprintln!("no LADSPA plugins installed; skipping");
        return;
    }
    let named: Vec<(&str, &choz_ports::PluginParam)> = found
        .iter()
        .flat_map(|p| {
            p.params
                .iter()
                .filter(|q| !q.points.is_empty())
                .map(move |q| (p.label.as_str(), q))
        })
        .collect();
    if named.is_empty() {
        eprintln!("no LADSPA metadata with scale points installed; skipping");
        return;
    }
    for (label, p) in &named {
        assert!(
            p.points.iter().all(|(_, l)| !l.is_empty()),
            "{label}/{}: a position with no name is not a name",
            p.name
        );
        // In value order, so stepping through them walks the knob one way.
        assert!(
            p.points.windows(2).all(|w| w[0].0 <= w[1].0),
            "{label}/{}: the positions are out of order",
            p.name
        );
        // Every named value has to be a value the port can actually take.
        for (v, name) in &p.points {
            assert!(
                *v >= p.min - 1e-6 && *v <= p.max + 1e-6,
                "{label}/{}: \"{name}\" sits at {v}, outside [{}..{}]",
                p.name,
                p.min,
                p.max
            );
        }
        // A list is drawn only when the names cover every position — see
        // `ParamShape::of`. Where they do not, the port keeps the count its
        // hint gave and the names label whatever the knob lands on; what must
        // never happen is the count shrinking to the number of names.
        assert!(
            p.steps as usize >= p.points.len(),
            "{label}/{}: {} steps for {} names",
            p.name,
            p.steps,
            p.points.len()
        );
        // `steps == points.len()` is exactly the condition `ParamShape::of`
        // asks before drawing a list of names rather than a knob — asserted
        // here as the number it is, because the engine that owns that rule
        // depends on this crate and not the other way round.
    }
    eprintln!("{} LADSPA port(s) have named positions", named.len());
}

/// A file that names only *some* of a port's positions must not shrink it.
///
/// swh's `gate` runs −1..1 with three integer settings — key listen, gate,
/// bypass — and its metadata names two of them. Taking the file's word turned
/// the port into a two-position switch whose ends are −1 and 1, so the middle
/// setting, the one the plugin calls "gate", could not be reached at all.
#[test]
fn a_partial_scale_does_not_shrink_the_port_it_names() {
    let _g = plugin_lock();
    let found = ladspa();
    if found.is_empty() {
        eprintln!("no LADSPA plugins installed; skipping");
        return;
    }
    let mut checked = 0usize;
    for p in &found {
        for q in p.params.iter().filter(|q| !q.points.is_empty()) {
            // Whatever the count is, the two ends of the travel have to be
            // reachable **and** every integer setting between them.
            let span = (q.max - q.min).abs();
            if q.steps == 2 && span > 1.5 {
                panic!(
                    "{}/{}: drawn as a switch over a range of {span} ({} named positions)",
                    p.label,
                    q.name,
                    q.points.len()
                );
            }
            checked += 1;
        }
    }
    assert!(checked > 0, "nothing with names to check");
}
