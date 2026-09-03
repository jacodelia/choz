//! What the pitch shifter, the vocoder's carrier and the frequency shifter put
//! in the output that was never in the input.
//!
//! Feed one pure tone, take the spectrum of the tail, call the biggest bin the
//! note and everything else the artifact. The number printed is the artifact
//! energy against the note, in dB — lower is cleaner.
//!
//! `cargo run --release -p choz-engine --example alias_probe`

use choz_engine::fx::{Carrier, FreqShift, FxProcessor, Vocoder, VoiceShifter};

const SR: f32 = 48_000.0;
const N: usize = 8192;

/// Magnitude spectrum of `n` samples, Hann windowed. Naive, and fast enough.
fn spectrum(x: &[f32]) -> Vec<f32> {
    let n = x.len();
    let w: Vec<f32> = (0..n)
        .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos())
        .collect();
    (0..n / 2)
        .map(|k| {
            let (mut re, mut im) = (0.0f32, 0.0f32);
            for (i, (&s, &wi)) in x.iter().zip(w.iter()).enumerate() {
                let a = std::f32::consts::TAU * k as f32 * i as f32 / n as f32;
                re += s * wi * a.cos();
                im -= s * wi * a.sin();
            }
            (re * re + im * im).sqrt() / n as f32
        })
        .collect()
}

/// Artifact-to-signal, in dB: everything outside a narrow skirt around the
/// loudest bin, against the loudest bin.
fn artifacts_db(x: &[f32]) -> (f32, f32) {
    let sp = spectrum(x);
    let (peak_k, peak) =
        sp.iter()
            .enumerate()
            .skip(2)
            .fold(
                (0usize, 0.0f32),
                |(bk, bv), (k, &v)| {
                    if v > bv {
                        (k, v)
                    } else {
                        (bk, bv)
                    }
                },
            );
    let skirt = 6;
    let rest: f32 = sp
        .iter()
        .enumerate()
        .skip(2)
        .filter(|(k, _)| k.abs_diff(peak_k) > skirt)
        .map(|(_, v)| v * v)
        .sum();
    let db = 10.0 * (rest / (peak * peak).max(1e-30)).max(1e-30).log10();
    (peak_k as f32 * SR / x.len() as f32, db)
}

/// Everything that is not a multiple of `f0`, against everything that is.
///
/// A saw is meant to be broadband, so "energy outside the loudest bin" says
/// nothing about it. What a naive saw does wrong is put energy at frequencies
/// that are **not** harmonics: the partials above Nyquist fold back and land
/// wherever they land.
fn inharmonic_db(x: &[f32], f0: f32) -> f32 {
    let sp = spectrum(x);
    let bin = SR / x.len() as f32;
    let (mut harm, mut junk) = (0.0f32, 0.0f32);
    for (k, v) in sp.iter().enumerate().skip(2) {
        let hz = k as f32 * bin;
        let n = (hz / f0).round();
        let near = n >= 1.0 && (hz - n * f0).abs() < bin * 2.0;
        if near {
            harm += v * v;
        } else {
            junk += v * v;
        }
    }
    10.0 * (junk / harm.max(1e-30)).max(1e-30).log10()
}

fn tone(hz: f32, n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| (std::f32::consts::TAU * hz * i as f32 / SR).sin() * 0.5)
        .collect()
}

fn main() {
    // What one second of stereo audio costs the frequency shifter.
    {
        let mut f = FreqShift::new(Carrier::Shift, SR as u32);
        f.set_param(0, 0.9);
        f.set_mix(1.0);
        let mut buf = vec![0.1f32; 512];
        let t = std::time::Instant::now();
        let blocks = 48_000 / 256;
        for _ in 0..blocks {
            f.process_block(&mut buf, SR as u32);
        }
        println!(
            "frequency shifter: {:.2} ms of CPU per second of audio\n",
            t.elapsed().as_secs_f32() * 1000.0
        );
    }

    println!("== pitch shifter (harmonizer, shimmer) ==");
    for hz in [220.0f32, 1000.0, 4000.0] {
        for semis in [7.0f32, 12.0, -12.0] {
            let mut sh = VoiceShifter::new();
            sh.set_semitones(semis);
            let out: Vec<f32> = tone(hz, N * 4).iter().map(|&x| sh.process(x)).collect();
            let (at, db) = artifacts_db(&out[N * 3..]);
            println!(
                "  {hz:>6.0} Hz {semis:>4.0} st -> tone at {at:>7.0} Hz, artifacts {db:>6.1} dB"
            );
        }
    }

    println!("== what the shifter's read costs the top end ==");
    // A ratio just off unity sweeps the read head slowly through every
    // fraction of a sample, which is exactly where an interpolator is judged.
    for hz in [2000.0f32, 8000.0, 14000.0] {
        let mut sh = VoiceShifter::new();
        sh.set_semitones(0.5);
        let out: Vec<f32> = tone(hz, N * 4).iter().map(|&x| sh.process(x)).collect();
        let tail = &out[N * 3..];
        let rms = (tail.iter().map(|s| s * s).sum::<f32>() / tail.len() as f32).sqrt();
        println!(
            "  {hz:>6.0} Hz -> {:>6.2} dB",
            20.0 * (rms / 0.3536).log10()
        );
    }

    println!("== vocoder carrier ==");
    for (name, knob) in [("saw", 0.0f32), ("pulse", 0.25)] {
        for pitch in [0.2f32, 0.6, 0.95] {
            let mut v = Vocoder::new(SR as u32);
            // 1 is the carrier, 2 the pitch.
            v.set_param(1, knob);
            v.set_param(2, pitch);
            v.set_mix(1.0);
            // Straight through the band bank with a steady voice, so what comes
            // out is the carrier as the effect actually uses it.
            let mut buf: Vec<f32> = (0..N * 8).flat_map(|_| [0.5f32, 0.0]).collect();
            v.process_block(&mut buf, SR as u32);
            let l: Vec<f32> = buf.chunks(2).map(|c| c[0]).skip(N * 3).take(N).collect();
            let f0 = 40.0 * 10.0f32.powf(pitch);
            let db = inharmonic_db(&l, f0);
            println!("  {name:>5} f0 {f0:>6.1} Hz -> inharmonic {db:>6.1} dB");
        }
    }

    println!("== the carrier on its own, naive against band-limited ==");
    let blep = |t: f32, dt: f32| {
        if t < dt {
            let t = t / dt;
            t + t - t * t - 1.0
        } else if t > 1.0 - dt {
            let t = (t - 1.0) / dt;
            t * t + t + t + 1.0
        } else {
            0.0
        }
    };
    for f0 in [63.4f32, 159.2, 356.5] {
        let dt = f0 / SR;
        let mut ph = 0.0f32;
        let naive: Vec<f32> = (0..N)
            .map(|_| {
                ph = (ph + dt).fract();
                ph * 2.0 - 1.0
            })
            .collect();
        ph = 0.0;
        let limited: Vec<f32> = (0..N)
            .map(|_| {
                ph = (ph + dt).fract();
                ph * 2.0 - 1.0 - blep(ph, dt)
            })
            .collect();
        println!(
            "  saw f0 {f0:>6.1} Hz -> naive {:>6.1} dB, polyblep {:>6.1} dB",
            inharmonic_db(&naive, f0),
            inharmonic_db(&limited, f0)
        );
    }

    println!("== frequency shifter ==");
    for hz in [1000.0f32, 8000.0] {
        for knob in [0.9f32, 1.0] {
            let mut f = FreqShift::new(Carrier::Shift, SR as u32);
            f.set_param(0, knob);
            f.set_mix(1.0);
            let mut buf: Vec<f32> = tone(hz, N * 4).iter().flat_map(|&s| [s, s]).collect();
            f.process_block(&mut buf, SR as u32);
            let l: Vec<f32> = buf.chunks(2).map(|c| c[0]).skip(N * 3).take(N).collect();
            let (at, db) = artifacts_db(&l);
            println!(
                "  {hz:>6.0} Hz +{:>5.0} Hz -> tone at {at:>7.0} Hz, artifacts {db:>6.1} dB",
                f.freq_hz()
            );
        }
    }
}
