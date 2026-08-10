//! Runtime checks against the LADSPA/DSSI plugins installed on this machine.
//! Each test skips when nothing is installed, so CI stays green without them.

use choz_plugin_ladspa::{scan_directory, DssiInstrument, LadspaEffect, PluginInfo};
use choz_ports::{AudioSource, FxProcessor};

const SR: u32 = 48_000;
const BLOCK: u32 = 256;

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

/// A DSSI synth must load and make sound after a note-on.
#[test]
fn dssi_instrument_sounds_on_note_on() {
    let found = dssi();
    let mut tried = 0;
    for info in found.iter().filter(|p| p.is_instrument) {
        let Some(mut inst) = DssiInstrument::build(&info.path, &info.label, SR, BLOCK) else {
            continue;
        };
        tried += 1;
        inst.note_on(60, 100);
        let mut peak = 0.0f32;
        for _ in 0..20 {
            let mut buf = vec![0.0f32; BLOCK as usize * 2];
            inst.render(&mut buf, SR);
            for s in &buf {
                assert!(s.is_finite(), "{} produced non-finite", info.label);
                peak = peak.max(s.abs());
            }
        }
        if peak > 1e-4 {
            return; // one audible synth is enough
        }
    }
    if tried == 0 {
        eprintln!("no DSSI instrument installed; skipping");
    } else {
        panic!("{tried} DSSI instrument(s) loaded but none made a sound");
    }
}
