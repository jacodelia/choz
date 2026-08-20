//! What the runaway catcher does to a sung note, next to what it does to a
//! real howl. Both are "it got louder"; only one of them is a loop.
//!
//! `cargo run -p choz-engine --release --example guard_probe`

fn run(name: &str, sr: f32, amp: impl Fn(f32) -> f32, secs: f32) {
    let mut g = choz_engine::feedback::FeedbackGuard::new(sr);
    let hz = 220.0f32;
    let n = (secs * sr) as usize;
    let mut phase = 0.0f32;
    let mut worst = 1.0f32;
    let mut marks = Vec::new();
    for i in 0..n {
        let t = i as f32 / sr;
        let x = (phase * std::f32::consts::TAU).sin() * amp(t);
        phase = (phase + hz / sr).fract();
        let gain = g.step(x);
        worst = worst.min(gain);
        if i % (sr as usize / 4) == 0 {
            marks.push(format!("{gain:.2}"));
        }
    }
    println!("{name:<22} worst {worst:.3}  gain each 250 ms: {}", marks.join(" "));
}

fn main() {
    let sr = 48_000.0;
    // A sung note: swells in over 150 ms, then held flat and loud.
    run("sung note", sr, |t| (t / 0.15).min(1.0) * 0.35, 4.0);
    // A slower swell, the way a singer actually enters a long note.
    run("sung swell 600 ms", sr, |t| (t / 0.6).min(1.0) * 0.4, 4.0);
    // A held note with vibrato on it.
    run("sung + vibrato", sr, |t| {
        (t / 0.3).min(1.0) * 0.35 * (1.0 + 0.25 * (t * 5.0 * std::f32::consts::TAU).sin())
    }, 4.0);
    // The hardest case for any rule of this shape: a slow operatic crescendo,
    // which really is 1.5 s of continuous growth and which the guard cannot
    // tell from a slow loop by growth alone.
    run("crescendo 2 s", sr, |t| (0.06 + 0.3 * (t / 2.0).min(1.0)).min(1.0), 5.0);
    // A real loop: 6 dB a second, forever, from something already audible.
    run("howl +6 dB/s", sr, |t| (0.06 * 2.0f32.powf(t)).min(1.0), 4.0);
    // A fast one.
    run("howl +18 dB/s", sr, |t| (0.06 * 8.0f32.powf(t)).min(1.0), 4.0);
}
