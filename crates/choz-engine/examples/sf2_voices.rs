//! What a pedalful of notes costs an SF2 slot, voice by voice.
//!
//! ```sh
//! cargo run --release --example sf2_voices -p choz-engine -- <sf2> [sample rate] [block]
//! ```
//! The report is "collapses under the pedal": oxisynth allows **256** voices by
//! default, and at 96 kHz a 128-frame block is 1.33 ms for the whole rack.
use choz_engine::sources::{AudioSource, Sf2Synth};
use std::time::Instant;

fn main() {
    if choz_engine::worker_main() {
        return;
    }
    let mut a = std::env::args().skip(1);
    let sf2 = a.next().expect("usage: sf2_voices <sf2> [sr] [block]");
    let sr: u32 = a.next().and_then(|s| s.parse().ok()).unwrap_or(96_000);
    let block: usize = a.next().and_then(|s| s.parse().ok()).unwrap_or(128);
    let budget = block as f64 / sr as f64 * 1000.0;
    println!("{sf2}\n{sr} Hz / {block} frames = {budget:.2} ms a block");

    let mut synth = Sf2Synth::load(std::path::Path::new(&sf2), 0, 0, sr).expect("load");
    synth.control_change(64, 127); // pedal down, nothing is ever released
    let mut buf = vec![0.0f32; block * 2];
    let mut held = 0;
    for round in 1..=12 {
        // Eight more keys under the pedal, then measure a second of blocks.
        for i in 0..8 {
            let n = 21 + ((held + i) as u8 % 88);
            synth.note_on(n, 100);
            synth.note_off(n);
        }
        held += 8;
        let mut times = Vec::new();
        for _ in 0..400 {
            let t = Instant::now();
            synth.render(&mut buf, sr);
            times.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(f64::total_cmp);
        let med = times[times.len() / 2];
        println!(
            "{:>3} keys held: median {:>6.3} ms  p99 {:>6.3}  = {:>5.0}% of the block{}",
            held,
            med,
            times[times.len() * 99 / 100],
            med / budget * 100.0,
            if med > budget {
                "   ← over budget on its own"
            } else {
                ""
            }
        );
        let _ = round;
    }
}
