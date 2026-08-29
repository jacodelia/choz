//! What moving `Wet` does to the level, effect by effect.
//!
//! The same signal through every built-in at four mix positions. The number is
//! output RMS against input RMS, in dB. What is being looked for is not a
//! particular number — a distortion is meant to be louder and a gate is meant
//! to be quieter — but the **shape** of the four: a dry/wet that behaves the
//! same way whichever effect it is on.
//!
//! Two effects need real knob positions to say anything: the wave shaper's
//! table at 0.5 everywhere is a flat curve, which is silence and correctly so,
//! and the harmoniser in vocoder mode says nothing until a chord is held.
//!
//! `cargo run --release -p choz-engine --example mix_probe`

use choz_engine::fx_chain::{build_processor, BUILT_IN_KINDS};

const SR: u32 = 48_000;

/// Pink-ish noise plus a note: something with a level, a spectrum and a pitch,
/// so a filter, a shifter and a compressor all have something to work on.
fn signal(n: usize) -> Vec<f32> {
    let mut rng = 0x1234_5678u32;
    let mut lp = 0.0f32;
    (0..n)
        .flat_map(|i| {
            rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
            let white = (rng >> 8) as f32 / 8_388_608.0 - 1.0;
            lp += 0.05 * (white - lp);
            let tone = (std::f32::consts::TAU * 220.0 * i as f32 / SR as f32).sin();
            let s = 0.35 * tone + 0.25 * lp;
            [s, s]
        })
        .collect()
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|s| s * s).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

fn main() {
    let dry = signal(SR as usize * 2);
    let dry_rms = rms(&dry);
    println!("{:<16} {:>8} {:>8} {:>8} {:>8}", "effect", "wet 0", "0.25", "0.5", "1.0");
    for (kind, _) in BUILT_IN_KINDS {
        // Mid-position knobs: what the rack hands a freshly added effect.
        let params = [0.5f32; 16];
        let mut row = String::new();
        for wet in [0.0f32, 0.25, 0.5, 1.0].iter() {
            let Some(mut p) = build_processor(kind, &params, SR) else {
                continue;
            };
            p.set_mix(*wet);
            let mut buf = dry.clone();
            for block in buf.chunks_mut(512) {
                p.process_block(block, SR);
            }
            // The second half, so a delay or a reverb has filled up.
            let out = rms(&buf[buf.len() / 2..]);
            let db = 20.0 * (out / dry_rms).log10();
            row.push_str(&format!(" {db:>8.2}"));
        }
        // A dry/wet at zero that is not a wire is the other half of the story.
        let mut p = build_processor(kind, &params, SR).unwrap();
        p.set_mix(0.0);
        let mut buf = dry.clone();
        for block in buf.chunks_mut(512) {
            p.process_block(block, SR);
        }
        let diff: Vec<f32> = buf.iter().zip(dry.iter()).map(|(a, b)| a - b).collect();
        let leak = 20.0 * (rms(&diff) / dry_rms).max(1e-12).log10();
        println!("{kind:<16}{row}   leak {leak:>8.1} dB");
    }
}
