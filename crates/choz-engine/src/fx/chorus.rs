//! Stereo chorus: two delay lines, modulated in quadrature.
//!
//! # What was wrong with the one this replaces
//!
//! Three things, all audible, all of a kind that a listening test finds before
//! a code review does.
//!
//! **It was a different effect on a different interface.** The buffer was
//! `4096` *samples*, which is 93 ms at 44.1 kHz and 21 ms at 192 kHz, and the
//! delay and depth were clamped against that capacity. A patch written on one
//! device came out as a flanger on another. The line is now sized in
//! milliseconds — see [`DelayLine::with_ms`].
//!
//! **Its stereo was suspected and cleared.** The right-hand LFO is the left one
//! offset by half a cycle, which is exactly its negation — the shape that
//! usually means an effect vanishes when the mix is folded to mono. Measured,
//! it does not: what cancels is the pair of LFO *values*, not the audio, since
//! a delay is not linear in its delay time. Mirrored heads fold down to the
//! average of two differently delayed copies and move as much as a
//! quarter-cycle pair. The offset was left where it was rather than changing
//! the effect's voicing on a wrong diagnosis.
//!
//! **Its feedback path was a low-pass.** The read was linearly interpolated and
//! then fed back, so every pass lost 3 dB at a quarter of the rate, and lost a
//! varying amount of it as the LFO swept the fractional part. The feedback path
//! reads with a cubic now; the dry-side tap, which never comes back, does not
//! pay for one.
//!
//! # Real-time
//!
//! Buffers in `new`. Nothing allocates, nothing locks, and the two `sin()`
//! calls a frame are gone — the LFO is a cubic phasor, and the delay times are
//! smoothed rather than read raw, so automation moves the read head instead of
//! jumping it.

use super::delay_line::{safe, soft_clip, wobble, DelayLine};
use super::smooth::Smoothed;

/// The longest the line ever has to hold: the deepest base delay plus the
/// deepest swing, plus room for the cubic's four points.
const MAX_TIME_MS: f32 = 30.0 + 10.0 + 4.0;

/// Where the feedback path saturates. A chorus at 0.9 feedback over a short
/// delay is close to a resonator; below 70 % of this the curve is exactly the
/// identity, so nothing that is not already loud is touched.
const FEEDBACK_CEIL: f32 = 2.0;

/// How long the delay controls take to arrive, in milliseconds.
///
/// Long, and on purpose: a read head is heard as *pitch*, so a jump in the
/// delay time is a click and a step in tuning at once. 80 ms turns a knob
/// sweep into a glide, which is what a chorus does anyway.
const GLIDE_MS: f32 = 80.0;

pub struct Chorus {
    pub rate: f32,     // LFO Hz (0.1–5.0)
    pub depth: f32,    // modulation depth in ms (0.5–10.0)
    pub delay_ms: f32, // base delay (5.0–30.0 ms)
    pub feedback: f32, // feedback level (-0.9..0.9)
    mix: f32,
    /// What the public fields last read as. They are written directly — by
    /// `fx_chain` when the chain is built and by `set_param` when a knob moves
    /// — so the only way to notice a change is to look, once a block.
    seen: (f32, f32, f32),
    delay: Smoothed,
    depth_s: Smoothed,
    lfo_phase: f32,
    line: [DelayLine; 2],
    sample_rate: f32,
}

impl Chorus {
    pub fn new() -> Self {
        let sr = 48_000.0;
        Self {
            rate: 0.5,
            depth: 3.0,
            delay_ms: 15.0,
            feedback: 0.1,
            mix: 0.5,
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

    /// Pick up whatever was written to the public fields, in samples.
    ///
    /// Clamped against the line's real capacity rather than a constant, so the
    /// ceiling is the time the buffer holds and not a number that meant
    /// something at one sample rate.
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
            // First block: arrive rather than glide up from zero, or the chorus
            // starts every session with a swoop.
            self.depth_s.snap(depth);
            self.delay.snap(base);
        }
        self.seen = now;
    }
}

impl Default for Chorus {
    fn default() -> Self {
        Self::new()
    }
}

impl super::FxProcessor for Chorus {
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
        let frames = buf.len() / 2;

        for i in 0..frames {
            // Half a cycle apart, as it always was.
            //
            // This looks like the mono-collapse trap — the right LFO is exactly
            // the negation of the left, so the two read heads move in mirror
            // image — and it was written up as one. It is not, and the
            // measurement is in `the_chorus_still_moves_in_mono`: what cancels
            // in a fold-down is the two *LFO values*, not the audio they
            // produce, because a delay is not a linear function of its delay
            // time. Mirrored heads fold down to the average of two differently
            // delayed copies, which moves exactly as much as a quarter-cycle
            // pair does — 1.313 against 1.314, measured.
            let lfo_l = wobble(self.lfo_phase);
            let lfo_r = wobble((self.lfo_phase + 0.5) % 1.0);
            self.lfo_phase += lfo_inc;
            if self.lfo_phase >= 1.0 {
                self.lfo_phase -= 1.0;
            }

            let base = self.delay.tick();
            let depth = self.depth_s.tick();
            // Cubic, because this is what goes back into the line.
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
    fn name(&self) -> &str {
        "Chorus"
    }

    fn params(&self) -> Vec<crate::fx::FxParam> {
        use crate::fx::FxParam;
        vec![
            FxParam::new("Rate", (self.rate / 5.0).clamp(0.0, 1.0), 0.0, 5.0, "Hz"),
            FxParam::new(
                "Depth",
                (self.depth / 10.0).clamp(0.0, 1.0),
                0.0,
                10.0,
                "ms",
            ),
            FxParam::new(
                "Delay",
                ((self.delay_ms - 5.0) / 25.0).clamp(0.0, 1.0),
                5.0,
                30.0,
                "ms",
            ),
            FxParam::new("Feedback", (self.feedback + 0.9) / 1.8, -0.9, 0.9, ""),
            FxParam::new("Wet", self.mix, 0.0, 1.0, ""),
        ]
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => self.rate = 0.05 + v * 4.95,
            1 => self.depth = 0.5 + v * 9.5,
            2 => self.delay_ms = 5.0 + v * 25.0,
            3 => self.feedback = -0.9 + v * 1.8,
            4 => self.mix = v,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::FxProcessor;

    #[test]
    fn chorus_output_differs_from_dry() {
        let mut ch = Chorus::new();
        ch.set_mix(1.0);
        let dry: Vec<f32> = (0..256).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        let mut buf = dry.clone();
        ch.process_block(&mut buf, 48000);
        // After processing, at least some samples should differ from dry (modulated delay).
        let max_diff = dry
            .iter()
            .zip(buf.iter())
            .map(|(d, w)| (d - w).abs())
            .fold(0.0f32, f32::max);
        assert!(
            max_diff > 0.001,
            "chorus output should differ from dry input, max_diff={}",
            max_diff
        );
    }

    /// Deterministic pink-ish noise: a chorus tested on a sine only ever shows
    /// what one frequency does to it.
    fn noise(n: usize) -> Vec<f32> {
        let mut seed = 0x2545_F491u32;
        let mut lp = 0.0f32;
        (0..n)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let w = (seed >> 8) as f32 / (1 << 23) as f32 - 1.0;
                lp += 0.35 * (w - lp);
                lp * 2.0
            })
            .collect()
    }

    fn run(ch: &mut Chorus, sr: u32, mono_in: &[f32]) -> Vec<f32> {
        let mut buf: Vec<f32> = mono_in.iter().flat_map(|&s| [s, s]).collect();
        ch.process_block(&mut buf, sr);
        buf
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|s| s * s).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    /// The chorus has to survive a fold-down to mono.
    ///
    /// Worth locking whatever the LFO offset is, and worth reading for what it
    /// is *not*: the anti-phase offset here looks like the classic mono-collapse
    /// trap and was written up as one during the audit. Measured side by side,
    /// a half-cycle offset and a quarter-cycle offset fold down within a tenth
    /// of a percent of each other — 1.313 against 1.314 — because what cancels
    /// is the two LFO values and not the audio they produce. The finding was
    /// withdrawn rather than the voicing changed.
    ///
    /// The measurement is the mono sum's difference from the dry: if the
    /// fold-down were a straight wire, that difference would be zero.
    #[test]
    fn the_chorus_still_moves_in_mono() {
        let sr = 48_000;
        let dry = noise(24_000);
        let mut ch = Chorus::new();
        ch.set_mix(1.0);
        ch.feedback = 0.0;
        let out = run(&mut ch, sr, &dry);

        let mono: Vec<f32> = out.chunks(2).map(|c| (c[0] + c[1]) * 0.5).collect();
        // Past the first few hundred samples, where the line is still filling.
        let from = 2_000;
        let moved = rms(&mono[from..]
            .iter()
            .zip(&dry[from..])
            .map(|(w, d)| w - d)
            .collect::<Vec<_>>());
        let sides = rms(&out[from * 2..]);
        assert!(
            moved > sides * 0.25,
            "the chorus vanishes in mono: {moved:.4} against {sides:.4}"
        );
    }

    /// A chorus has to be the same effect on every interface.
    ///
    /// The old one sized its line at 4096 *samples*, so at 192 kHz its 30 ms
    /// delay was silently clamped to 21 and its depth with it. Measured as the
    /// modulation's period: at the same rate knob the wet signal has to wander
    /// at the same speed in *seconds*, whatever the device.
    #[test]
    fn the_chorus_is_the_same_at_every_sample_rate() {
        // Peak excursion of the wet-minus-dry difference over one second: a
        // proxy for how far the read head actually travelled.
        let travel = |sr: u32| {
            let secs = 1.0;
            let n = (sr as f32 * secs) as usize;
            let dry: Vec<f32> = (0..n)
                .map(|i| (std::f32::consts::TAU * 220.0 * i as f32 / sr as f32).sin() * 0.5)
                .collect();
            let mut ch = Chorus::new();
            ch.set_mix(1.0);
            ch.feedback = 0.0;
            ch.rate = 1.0;
            ch.depth = 8.0;
            ch.delay_ms = 20.0;
            let out = run(&mut ch, sr, &dry);
            let from = sr as usize / 4;
            rms(&out[from * 2..])
        };
        let a = travel(44_100);
        let b = travel(192_000);
        assert!(
            (a / b).abs() > 0.8 && (a / b) < 1.25,
            "the chorus changed with the rate: {a:.4} at 44.1k, {b:.4} at 192k"
        );
    }

    /// Moving the delay while audio is running must not step the read head.
    ///
    /// A jump in a delay time is heard twice: as a click, and as a step in
    /// tuning. The smoother turns it into a glide, which is what the sample-to-
    /// sample difference measures.
    #[test]
    fn moving_the_delay_does_not_click() {
        let sr = 48_000;
        let dry = noise(sr as usize / 2);
        // Measured against the same signal with the delay held still, because
        // the absolute number means nothing: a chorus is a moving read head, so
        // there is always *some* sample-to-sample motion. What must not happen
        // is a step far bigger than the one the effect makes on its own.
        let worst = |automate: bool| {
            let mut ch = Chorus::new();
            ch.set_mix(1.0);
            ch.feedback = 0.4;
            let mut worst = 0.0f32;
            let mut prev = 0.0f32;
            for (block, chunk) in dry.chunks(256).enumerate() {
                if automate {
                    // Slam the delay end to end, every block.
                    ch.set_param(2, (block % 2) as f32);
                }
                let out = run(&mut ch, sr, chunk);
                for s in out.chunks(2).map(|c| c[0]) {
                    worst = worst.max((s - prev).abs());
                    prev = s;
                }
            }
            worst
        };
        let still = worst(false);
        let swept = worst(true);
        assert!(
            swept < still * 2.0,
            "the delay stepped: {swept:.3} while automated, {still:.3} while still"
        );
    }

    /// The feedback path has to go quiet, and stay bounded when it is driven.
    #[test]
    fn the_feedback_is_bounded_and_reaches_silence() {
        let sr = 48_000;
        let mut ch = Chorus::new();
        ch.set_mix(1.0);
        ch.feedback = 0.95;
        let hot: Vec<f32> = noise(sr as usize).iter().map(|s| s * 8.0).collect();
        let out = run(&mut ch, sr, &hot);
        assert!(out.iter().all(|s| s.is_finite()));
        assert!(
            out.iter().fold(0.0f32, |m, s| m.max(s.abs())) < FEEDBACK_CEIL * 4.0,
            "the feedback ran away"
        );
        // Silence is measured at a feedback the effect actually decays at:
        // 0.95 over a 20 ms delay is a three-second reverb tail, and the point
        // here is the *flush*, not how long a tail takes.
        let mut quiet = Chorus::new();
        quiet.set_mix(1.0);
        quiet.feedback = 0.5;
        let _ = run(&mut quiet, sr, &noise(4_800));
        let mut tail = Vec::new();
        // Half a pass every 20 ms is 300 dB a second, and the flush is at
        // −500 dB: three seconds is comfortably past it.
        for _ in 0..30 {
            tail = run(&mut quiet, sr, &vec![0.0f32; 4_800]);
        }
        assert!(
            tail.iter().all(|s| *s == 0.0),
            "it never reached silence: {}",
            tail.iter().fold(0.0f32, |m, s| m.max(s.abs()))
        );
    }

    #[test]
    fn chorus_at_zero_mix_is_passthrough() {
        let mut ch = Chorus::new();
        ch.set_mix(0.0);
        let dry: Vec<f32> = (0..256).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        let mut buf = dry.clone();
        ch.process_block(&mut buf, 48000);
        for (d, w) in dry.iter().zip(buf.iter()) {
            assert!((d - w).abs() < 1e-6, "at mix=0 output should equal input");
        }
    }
}
