//! What holding the sustain pedal does to level and to CPU, over minutes.
//!
//! Two reports point at the same place: the sound saturates when the pedal
//! goes down, and the DSP readout climbs the longer choz stays open. Both are
//! what a synth whose voices are held and never reclaimed looks like. This
//! plays a chord a second with CC64 down and prints, per minute, the peak of
//! the block and what a block costs.
//!
//! `cargo run -p choz-engine --release --example sustain_probe [minutes] [sf2]`

use choz_ports::AudioSource;

fn main() {
    let mut args = std::env::args().skip(1);
    let minutes: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(5);
    let path = args
        .next()
        .unwrap_or_else(|| "/usr/share/sounds/sf2/FluidR3_GM.sf2".into());
    let path = std::path::Path::new(&path);
    if !path.exists() {
        eprintln!("no SF2 at {}", path.display());
        return;
    }
    let sr = 48_000u32;
    let frames = 256usize;
    let pedal: bool = std::env::var("PEDAL").map(|v| v != "0").unwrap_or(true);

    let mut synth = choz_engine::sources::Sf2Synth::load(path, 0, 0, sr).expect("load SF2");
    if pedal {
        synth.control_change(64, 127);
    }
    let mut buf = vec![0.0f32; frames * 2];
    let blocks_per_sec = sr as usize / frames;
    let mut note = 48u8;
    println!("pedal={pedal}  min   peak   us/block");
    for minute in 0..minutes {
        let mut peak = 0.0f32;
        let mut cost = std::time::Duration::ZERO;
        for b in 0..blocks_per_sec * 60 {
            // One three-note chord a second, walking up the keyboard.
            if b % blocks_per_sec == 0 {
                for n in [note, note + 4, note + 7] {
                    synth.note_on(n, 100);
                }
                note = 36 + (note + 1 - 36) % 48;
            }
            // …released half a second later. With the pedal down the voices
            // stay, which is the whole question.
            if b % blocks_per_sec == blocks_per_sec / 2 {
                for n in [note, note + 4, note + 7] {
                    synth.note_off(n);
                }
            }
            buf.fill(0.0);
            let t = std::time::Instant::now();
            synth.render(&mut buf, sr);
            cost += t.elapsed();
            peak = buf.iter().fold(peak, |m, s| m.max(s.abs()));
        }
        let n = blocks_per_sec * 60;
        println!(
            "{:>3}  {peak:>6.3}  {:>8.1}",
            minute + 1,
            cost.as_secs_f64() * 1e6 / n as f64
        );
    }
}
