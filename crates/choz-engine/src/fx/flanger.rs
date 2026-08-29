//! Flanger: one short delay a side, swept, with feedback.
//!
//! The same three fixes the chorus needed, and the interpolation one matters
//! more here than anywhere else in the tree.
//!
//! A flanger *is* its feedback path: the notches are what a comb makes, and how
//! sharp they are is how much of the signal comes back round. At the 0.95 this
//! allows, a pass of the loop happens every few milliseconds — hundreds a
//! second — and the old code interpolated that read linearly, which is 3 dB
//! down at a quarter of the sample rate. Hundreds of passes at 3 dB is not a
//! resonance, it is a thud; and because the fractional part of the read sweeps
//! with the LFO, the amount of dulling swept too. Reading with a cubic is what
//! gives the sweep its whistle back.
//!
//! The buffer used to be `2048` samples with a comment that said "~46ms at
//! 44.1kHz" — which is exactly the bug: at 192 kHz it is 10.7 ms, so the
//! deepest setting was quietly a third of what the knob said. It is sized in
//! milliseconds now.
//!
//! The stereo — a quarter cycle between the sides — is left exactly as it was.
//! An earlier draft of the audit called the chorus's half-cycle offset a
//! mono-collapse bug and this one the fix; measuring the two proved that wrong
//! (see `chorus::tests::the_chorus_still_moves_in_mono`), so neither effect had
//! its voicing changed.

use super::delay_line::{safe, soft_clip, wobble, DelayLine};
use super::smooth::Smoothed;

/// Deepest base delay plus deepest sweep, plus room for the cubic's four points.
const MAX_TIME_MS: f32 = 10.0 + 7.0 + 4.0;

/// Where the feedback path bends. Lower than the chorus's: a flanger runs its
/// loop at a few milliseconds, so resonant gain builds far faster.
const FEEDBACK_CEIL: f32 = 1.5;

/// A flanger's delay is short enough that a fast glide is heard as pitch, so
/// this is shorter than the chorus's — long enough not to click, short enough
/// that sweeping the knob still feels like a sweep.
const GLIDE_MS: f32 = 40.0;

pub struct Flanger {
    pub rate: f32,     // LFO Hz (0.1–5.0)
    pub depth: f32,    // modulation range in ms (0.0–7.0)
    pub delay_ms: f32, // base delay (0.5–10.0 ms)
    pub feedback: f32, // -0.95..0.95
    pub stereo: bool,
    mix: f32,
    seen: (f32, f32, f32),
    delay: Smoothed,
    depth_s: Smoothed,
    lfo_phase: f32,
    line: [DelayLine; 2],
    sample_rate: f32,
}

impl Flanger {
    pub fn new() -> Self {
        let sr = 48_000.0;
        Self {
            rate: 0.3,
            depth: 2.5,
            delay_ms: 3.0,
            feedback: 0.5,
            stereo: true,
            mix: 0.7,
            seen: (f32::NAN, f32::NAN, f32::NAN),
            delay: Smoothed::new(0.0, GLIDE_MS, sr),
            depth_s: Smoothed::new(0.0, GLIDE_MS, sr),
            lfo_phase: 0.0,
            line: [
                DelayLine::with_ms(MAX_TIME_MS),
                DelayLine::with_ms(MAX_TIME_MS),
            ],
            sample_rate: sr,
        }
    }

    /// Pick up the public fields, in samples, clamped against the line's real
    /// capacity rather than against a constant that meant something at one rate.
    fn retarget(&mut self, sr: f32) {
        let now = (self.delay_ms, self.depth, sr);
        if now == self.seen {
            return;
        }
        let room = (self.line[0].capacity() - 8) as f32;
        let depth = (self.depth.max(0.0) * sr / 1000.0).clamp(0.0, room * 0.4);
        let base = (self.delay_ms * sr / 1000.0).clamp(2.0, room - depth);
        self.depth_s.set_target(depth);
        self.delay.set_target(base);
        if self.seen.0.is_nan() {
            self.depth_s.snap(depth);
            self.delay.snap(base);
        }
        self.seen = now;
    }
}

impl Default for Flanger {
    fn default() -> Self {
        Self::new()
    }
}

impl super::FxProcessor for Flanger {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        if buf.len() < 2 {
            return;
        }
        let sr = sample_rate.max(8000) as f32;
        if sr != self.sample_rate {
            self.sample_rate = sr;
            self.delay.set_sample_rate(sr);
            self.depth_s.set_sample_rate(sr);
        }
        self.retarget(sr);

        let lfo_inc = self.rate.clamp(0.01, 20.0) / sr;
        let feedback = self.feedback.clamp(-0.95, 0.95);
        let mix = self.mix;
        let stereo = self.stereo;
        let frames = buf.len() / 2;

        for i in 0..frames {
            let lfo_l = wobble(self.lfo_phase);
            // A quarter cycle. Uncorrelated rather than opposite, so the width
            // is real and the fold-down keeps it.
            let lfo_r = match stereo {
                true => wobble((self.lfo_phase + 0.25) % 1.0),
                false => lfo_l,
            };
            self.lfo_phase += lfo_inc;
            if self.lfo_phase >= 1.0 {
                self.lfo_phase -= 1.0;
            }

            let base = self.delay.tick();
            let depth = self.depth_s.tick();
            let wet_l = self.line[0].read_cubic(base + depth * lfo_l);
            let wet_r = self.line[1].read_cubic(base + depth * lfo_r);

            let in_l = buf[i * 2];
            let in_r = buf[i * 2 + 1];
            self.line[0].write(soft_clip(in_l + feedback * wet_l, FEEDBACK_CEIL));
            self.line[1].write(soft_clip(in_r + feedback * wet_r, FEEDBACK_CEIL));

            buf[i * 2] = in_l + mix * (safe(wet_l) - in_l);
            buf[i * 2 + 1] = in_r + mix * (safe(wet_r) - in_r);
        }
    }

    fn reset(&mut self) {
        self.line[0].clear();
        self.line[1].clear();
        self.lfo_phase = 0.0;
        self.seen = (f32::NAN, f32::NAN, f32::NAN);
    }

    fn set_mix(&mut self, wet: f32) {
        self.mix = wet.clamp(0.0, 1.0);
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => self.rate = 0.05 + v * 4.95,
            1 => self.depth = v * 7.0,
            2 => self.delay_ms = 0.5 + v * 9.5,
            3 => self.feedback = (v - 0.5) * 1.9,
            4 => self.mix = v,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::FxProcessor;

    fn noise(n: usize) -> Vec<f32> {
        let mut seed = 0x9E37_79B9u32;
        (0..n)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 8) as f32 / (1 << 23) as f32 - 1.0
            })
            .collect()
    }

    fn run(fl: &mut Flanger, sr: u32, mono: &[f32]) -> Vec<f32> {
        let mut buf: Vec<f32> = mono.iter().flat_map(|&s| [s, s]).collect();
        fl.process_block(&mut buf, sr);
        buf
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|s| s * s).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    /// **The measurement the cubic read exists for.**
    ///
    /// A flanger's resonance is its feedback path going round hundreds of times
    /// a second. With a linear read, each pass cost 3 dB at a quarter of the
    /// rate; the resonance came out dull and — because the fractional part of
    /// the read sweeps with the LFO — dull by a varying amount.
    ///
    /// Measured as the high-frequency share of the wet signal at high feedback.
    /// It has to survive.
    #[test]
    fn the_resonance_keeps_its_top_end() {
        let sr = 48_000;
        let mut fl = Flanger::new();
        fl.set_mix(1.0);
        fl.feedback = 0.9;
        fl.rate = 0.2;
        fl.depth = 3.0;
        fl.delay_ms = 2.0;
        let dry = noise(sr as usize);
        let out = run(&mut fl, sr, &dry);

        // Energy above a one-pole split at 6 kHz, against energy below it.
        let split = |x: &[f32]| {
            let a = 1.0 - (-std::f32::consts::TAU * 6000.0 / sr as f32).exp();
            let (mut lp, mut hi, mut lo) = (0.0f32, 0.0f32, 0.0f32);
            for s in x.chunks(2).map(|c| c[0]).skip(4_800) {
                lp += a * (s - lp);
                hi += (s - lp) * (s - lp);
                lo += lp * lp;
            }
            hi / lo.max(1e-12)
        };
        let wet = split(&out);
        let dry_ratio = split(&dry.iter().flat_map(|&s| [s, s]).collect::<Vec<_>>());
        assert!(
            wet > dry_ratio * 0.35,
            "the resonance lost its top: wet {wet:.3} against dry {dry_ratio:.3}"
        );
    }

    /// It has to survive a fold-down to mono — a property worth locking,
    /// whatever the LFO offset happens to be.
    #[test]
    fn the_flanger_still_moves_in_mono() {
        let sr = 48_000;
        let dry = noise(24_000);
        let mut fl = Flanger::new();
        fl.set_mix(1.0);
        fl.feedback = 0.0;
        fl.stereo = true;
        let out = run(&mut fl, sr, &dry);
        let mono: Vec<f32> = out.chunks(2).map(|c| (c[0] + c[1]) * 0.5).collect();
        let from = 2_000;
        let moved = rms(&mono[from..]
            .iter()
            .zip(&dry[from..])
            .map(|(w, d)| w - d)
            .collect::<Vec<_>>());
        assert!(
            moved > rms(&out[from * 2..]) * 0.25,
            "the flanger vanishes in mono: {moved:.4}"
        );
    }

    /// Same effect on every interface — the old buffer was 2048 samples, which
    /// is 46 ms at 44.1 kHz and 10.7 at 192.
    #[test]
    fn the_flanger_is_the_same_at_every_sample_rate() {
        let level = |sr: u32| {
            let n = sr as usize;
            let dry: Vec<f32> = (0..n)
                .map(|i| (std::f32::consts::TAU * 330.0 * i as f32 / sr as f32).sin() * 0.5)
                .collect();
            let mut fl = Flanger::new();
            fl.set_mix(1.0);
            fl.feedback = 0.6;
            fl.rate = 0.5;
            fl.depth = 7.0;
            fl.delay_ms = 9.0;
            let out = run(&mut fl, sr, &dry);
            rms(&out[(sr as usize / 2)..])
        };
        let a = level(44_100);
        let b = level(192_000);
        assert!(
            (a / b) > 0.75 && (a / b) < 1.35,
            "the flanger changed with the rate: {a:.4} at 44.1k, {b:.4} at 192k"
        );
    }

    /// Driven hard at full feedback it must stay bounded, and it must go quiet.
    #[test]
    fn the_feedback_is_bounded_and_reaches_silence() {
        let sr = 48_000;
        let mut fl = Flanger::new();
        fl.set_mix(1.0);
        fl.feedback = 0.95;
        let hot: Vec<f32> = noise(sr as usize).iter().map(|s| s * 8.0).collect();
        let out = run(&mut fl, sr, &hot);
        assert!(out.iter().all(|s| s.is_finite()));
        assert!(
            out.iter().fold(0.0f32, |m, s| m.max(s.abs())) < FEEDBACK_CEIL * 4.0,
            "the feedback ran away"
        );

        let mut quiet = Flanger::new();
        quiet.set_mix(1.0);
        quiet.feedback = 0.5;
        let _ = run(&mut quiet, sr, &noise(4_800));
        let mut tail = Vec::new();
        for _ in 0..20 {
            tail = run(&mut quiet, sr, &vec![0.0f32; 4_800]);
        }
        assert!(
            tail.iter().all(|s| *s == 0.0),
            "it never reached silence: {}",
            tail.iter().fold(0.0f32, |m, s| m.max(s.abs()))
        );
    }

    /// Sweeping the delay while it sounds must glide, not step.
    #[test]
    fn moving_the_delay_does_not_click() {
        let sr = 48_000;
        let dry = noise(sr as usize / 2);
        let worst = |automate: bool| {
            let mut fl = Flanger::new();
            fl.set_mix(1.0);
            fl.feedback = 0.5;
            let (mut worst, mut prev) = (0.0f32, 0.0f32);
            for (block, chunk) in dry.chunks(256).enumerate() {
                if automate {
                    fl.set_param(2, (block % 2) as f32);
                }
                for s in run(&mut fl, sr, chunk).chunks(2).map(|c| c[0]) {
                    worst = worst.max((s - prev).abs());
                    prev = s;
                }
            }
            worst
        };
        let (still, swept) = (worst(false), worst(true));
        assert!(
            swept < still * 2.0,
            "the delay stepped: {swept:.3} automated, {still:.3} still"
        );
    }

    #[test]
    fn flanger_output_differs_from_dry() {
        let mut fl = Flanger::new();
        fl.set_mix(1.0);
        fl.feedback = 0.5;
        let dry: Vec<f32> = (0..256).map(|i| (i as f32 * 0.04).sin() * 0.5).collect();
        let mut buf = dry.clone();
        fl.process_block(&mut buf, 48000);
        let max_diff = dry
            .iter()
            .zip(buf.iter())
            .map(|(d, w)| (d - w).abs())
            .fold(0.0f32, f32::max);
        assert!(
            max_diff > 0.001,
            "flanger output should differ from dry, max_diff={}",
            max_diff
        );
    }

    #[test]
    fn flanger_at_zero_mix_is_passthrough() {
        let mut fl = Flanger::new();
        fl.set_mix(0.0);
        let dry: Vec<f32> = (0..256).map(|i| (i as f32 * 0.04).sin() * 0.5).collect();
        let mut buf = dry.clone();
        fl.process_block(&mut buf, 48000);
        for (d, w) in dry.iter().zip(buf.iter()) {
            assert!((d - w).abs() < 1e-6, "at mix=0 output should equal input");
        }
    }
}
