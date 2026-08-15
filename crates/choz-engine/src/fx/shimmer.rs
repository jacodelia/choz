//! Shimmer: a reverb with a pitch shifter inside its own feedback loop.
//!
//! ```text
//!  in ─► pre-delay ─┬─────────────────────────────────► dry
//!                   ▼
//!            ┌──► reverb ──┬──────────────────────────► wet
//!            │             ▼
//!            │        pitch shift (+12 st)
//!            │             ▼
//!            └── × fb ── damping low-pass
//! ```
//!
//! The tail is fed back through the shifter, so every pass is an octave above
//! the one before: the reverb climbs instead of just decaying. That is the
//! whole effect, and it is why the shifter is **inside** the loop — one after
//! the reverb would give a single transposed copy and no climb.
//!
//! # Why the shifter is this one
//!
//! [`super::autotune::shifter::RetuneShifter`] cuts its jumps on a **detected
//! period**, which is what makes it clean on a voice at ratios near 1. Here
//! there is no detector, the ratio is 2, and the material is a reverb tail —
//! so this uses the plain two-head crossfade, whose artefacts are a light
//! warble that a reverb smears anyway.
//!
//! # Real-time
//!
//! Every buffer is allocated at construction, including the scratch the inner
//! reverb is handed, and the block is processed in chunks that fit it. No
//! allocation, no locks.

use super::reverb::Reverb;
use super::FxProcessor;

/// Longest pre-delay: 250 ms at 192 kHz.
const PREDELAY_CAP: usize = 48_000;

/// Where the loop saturates, as a reciprocal: the fed-back tail cannot exceed
/// `1/LOOP_CEILING`. Sized so a wash settles around the level of the signal
/// that started it instead of thirty times louder, which is what an unscaled
/// `tanh` gives once the reverb's own resonant gain is behind it.
const LOOP_CEILING: f32 = 3.0;

pub struct ShimmerReverb {
    predelay: Vec<f32>,
    pre_write: usize,
    pre_frames: usize,
    reverb: Reverb,
    shifter: [super::shift::VoiceShifter; 2],
    /// One-pole low-pass in the loop: without it every pass gets brighter
    /// until the tail is a whistle.
    damp_state: [f32; 2],
    damp: f32,
    feedback: f32,
    /// What is going back round, from the last chunk.
    tail: [f32; 2],
    /// One frame, handed to the inner reverb.
    scratch: [f32; 2],
    mix: f32,
    sample_rate: f32,
}

impl ShimmerReverb {
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate.max(8000) as f32;
        let mut reverb = Reverb::new(sample_rate);
        reverb.set_room_size(0.85);
        reverb.set_damp(0.35);
        reverb.set_mix(1.0);
        Self {
            predelay: vec![0.0; PREDELAY_CAP * 2],
            pre_write: 0,
            pre_frames: (0.06 * sr) as usize,
            reverb,
            shifter: [
                super::shift::VoiceShifter::new(),
                super::shift::VoiceShifter::new(),
            ],
            damp_state: [0.0; 2],
            damp: 0.5,
            feedback: 0.5,
            tail: [0.0; 2],
            scratch: [0.0; 2],
            mix: 0.4,
            sample_rate: sr,
        }
    }

    /// Build from the rack's knob positions: size, predelay, shift, feedback,
    /// damping, width.
    pub fn with_params(sample_rate: u32, p: &[f32]) -> Self {
        let get = |i: usize, d: f32| p.get(i).copied().unwrap_or(d);
        let mut s = Self::new(sample_rate);
        s.reverb.set_room_size(get(0, 0.85));
        s.set_predelay(get(1, 0.25));
        s.set_shift(get(2, 1.0));
        s.set_feedback(get(3, 0.5));
        s.damp = get(4, 0.5).clamp(0.0, 1.0);
        s.reverb.set_width(get(5, 1.0) * 2.0);
        s
    }

    /// 0..1 → 0–250 ms.
    pub fn set_predelay(&mut self, v: f32) {
        let ms = v.clamp(0.0, 1.0) * 250.0;
        self.pre_frames = ((ms * 0.001 * self.sample_rate) as usize).min(PREDELAY_CAP - 1);
    }

    /// 0..1 → −12…+24 semitones, snapped to whole semitones.
    ///
    /// Snapped because a shimmer that is 11.6 semitones up is out of tune with
    /// whatever it is on, and there is nothing between two semitones that a
    /// reverb tail wants to be.
    pub fn set_shift(&mut self, v: f32) {
        let st = (v.clamp(0.0, 1.0) * 36.0 - 12.0).round();
        let ratio = 2.0f32.powf(st / 12.0);
        for s in &mut self.shifter {
            s.set_ratio(ratio);
        }
    }

    /// How much of the shifted tail goes back round.
    ///
    /// Capped at 0.6, with the loop bounded by a saturator rather than by this
    /// number.
    ///
    /// The reverb inside the loop is a bank of comb **resonators**: at their
    /// own resonances their gain is far above the 1.17 they show on broadband
    /// noise, and it moves with the room size. Measured, a burst fed back at
    /// 0.26 still grew without bound over ten seconds — so no honest constant
    /// here makes the loop stable, and hunting for one is hunting for a number
    /// that a future room size invalidates. The saturator in the loop bounds
    /// it structurally instead: the tail settles into a wash rather than
    /// exploding, which is what a shimmer at full feedback is supposed to do.
    pub fn set_feedback(&mut self, v: f32) {
        self.feedback = v.clamp(0.0, 1.0) * 0.6;
    }

    pub fn shift_semitones(&self) -> f32 {
        12.0 * self.shifter[0].ratio().log2()
    }
}

impl FxProcessor for ShimmerReverb {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        let sr = sample_rate.max(8000) as f32;
        if (sr - self.sample_rate).abs() > 0.5 {
            let secs = self.pre_frames as f32 / self.sample_rate;
            self.sample_rate = sr;
            self.pre_frames = ((secs * sr) as usize).min(PREDELAY_CAP - 1);
            self.reset();
        }
        // One-pole, ~2 kHz at full damping up to ~18 kHz at none.
        let cutoff = 18_000.0 * 0.11f32.powf(self.damp);
        let damp_coeff = (-std::f32::consts::TAU * cutoff / sr)
            .exp()
            .clamp(0.0, 0.999);
        let cap = self.predelay.len() / 2;

        for frame in buf.chunks_exact_mut(2) {
            let dry = [frame[0], frame[1]];
            // Pre-delay, plus whatever is coming back round.
            let read = (self.pre_write + cap - self.pre_frames) % cap;
            for (ch, dry_ch) in dry.iter().enumerate() {
                self.scratch[ch] = self.predelay[read * 2 + ch] + self.tail[ch] * self.feedback;
                self.predelay[self.pre_write * 2 + ch] = *dry_ch;
            }
            self.pre_write = (self.pre_write + 1) % cap;

            // **One frame at a time.** The loop's delay is then exactly one
            // sample, whatever block the caller brought: handing the reverb a
            // chunk and feeding back one value for all of it made the result
            // depend on the block size, which is the definition of a bug.
            self.reverb.process_block(&mut self.scratch, sample_rate);

            for (ch, dry_ch) in dry.iter().enumerate() {
                let wet = self.scratch[ch];
                // Round the loop: shift, then damp, then saturate.
                let shifted = self.shifter[ch].process(wet);
                self.damp_state[ch] = shifted + damp_coeff * (self.damp_state[ch] - shifted);
                // The one thing that makes this loop safe. Below about a third
                // of full scale it is within a percent of a wire; above it, it
                // is what turns "grows without bound" into "settles".
                self.tail[ch] = (self.damp_state[ch] * LOOP_CEILING).tanh() / LOOP_CEILING;

                let out = dry_ch + self.mix * (wet - dry_ch);
                // A loop with a reverb, a resampler and a filter in it has more
                // ways to go non-finite than can be checked one by one; one
                // guard at the exit, and the loop is cleared.
                frame[ch] = if out.is_finite() {
                    out
                } else {
                    self.tail = [0.0; 2];
                    self.damp_state = [0.0; 2];
                    *dry_ch
                };
            }
        }
    }

    fn reset(&mut self) {
        self.predelay.fill(0.0);
        self.pre_write = 0;
        self.reverb.reset();
        for s in &mut self.shifter {
            s.reset();
        }
        self.damp_state = [0.0; 2];
        self.tail = [0.0; 2];
        self.scratch = [0.0; 2];
    }

    fn set_mix(&mut self, wet: f32) {
        self.mix = wet.clamp(0.0, 1.0);
    }

    fn name(&self) -> &str {
        "Shimmer"
    }

    fn params(&self) -> Vec<crate::fx::FxParam> {
        use crate::fx::FxParam;
        vec![
            FxParam::new("Size", 0.85, 0.0, 1.0, ""),
            FxParam::new(
                "PreDelay",
                (self.pre_frames as f32 / self.sample_rate / 0.25).clamp(0.0, 1.0),
                0.0,
                250.0,
                "ms",
            ),
            FxParam::new(
                "Shift",
                ((self.shift_semitones() + 12.0) / 36.0).clamp(0.0, 1.0),
                -12.0,
                24.0,
                "st",
            ),
            FxParam::new("Feedback", self.feedback / 0.85, 0.0, 1.0, ""),
            FxParam::new("Damping", self.damp, 0.0, 1.0, ""),
            FxParam::new("Width", 0.5, 0.0, 1.0, ""),
            FxParam::new("Wet", self.mix, 0.0, 1.0, ""),
        ]
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => self.reverb.set_room_size(v),
            1 => self.set_predelay(v),
            2 => self.set_shift(v),
            3 => self.set_feedback(v),
            4 => self.damp = v,
            5 => self.reverb.set_width(v * 2.0),
            6 => self.mix = v,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn burst(frames: usize, hz: f32, sr: f32, len: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let s = if i < len {
                    (std::f32::consts::TAU * hz * i as f32 / sr).sin() * 0.5
                } else {
                    0.0
                };
                [s, s]
            })
            .collect()
    }

    pub(super) fn rms(buf: &[f32]) -> f32 {
        (buf.iter().map(|s| s * s).sum::<f32>() / buf.len().max(1) as f32).sqrt()
    }

    pub(super) fn energy_at(buf: &[f32], probe: f32, sr: f32) -> f32 {
        let l: Vec<f32> = buf.iter().step_by(2).copied().collect();
        let n = l.len() as f32;
        let k = (probe * n / sr).round();
        let w = std::f32::consts::TAU * k / n;
        let coeff = 2.0 * w.cos();
        let (mut s1, mut s2) = (0.0f32, 0.0f32);
        for x in &l {
            let s0 = x + coeff * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        ((s1 * s1 + s2 * s2 - coeff * s1 * s2).max(0.0)).sqrt() / n
    }

    /// The shifter on its own, before anything is fed back through it: 400 Hz
    /// in at ratio 2 has to come out at 800 Hz.
    #[test]
    fn the_shifter_moves_the_pitch_up() {
        let sr = 48000.0;
        let mut sh = super::super::shift::VoiceShifter::new();
        sh.set_ratio(2.0);
        let mut out = Vec::new();
        for i in 0..48000 {
            let x = (std::f32::consts::TAU * 400.0 * i as f32 / sr).sin() * 0.5;
            let y = sh.process(x);
            out.push(y);
            out.push(y);
        }
        let tail = &out[24000 * 2..];
        let up = energy_at(tail, 800.0, sr);
        let same = energy_at(tail, 400.0, sr);
        assert!(up > same * 4.0, "expected 800 Hz: up={up} same={same}");
    }

    /// The claim: the tail climbs. A 400 Hz burst leaves an 800 Hz tail behind
    /// it, and one that outlasts the burst itself.
    #[test]
    fn the_tail_comes_back_an_octave_up() {
        let sr = 48000.0;
        let mut s = ShimmerReverb::new(48000);
        s.set_shift(24.0 / 36.0); // +12 semitones
        s.set_feedback(1.0);
        s.set_mix(1.0);
        s.damp = 0.2;
        let mut buf = burst(48000 * 3, 400.0, sr, 12000);
        s.process_block(&mut buf, 48000);
        // The second of tail that starts three quarters of a second after the
        // burst has stopped: the direct reverb of the 400 Hz is still dying,
        // and what has been round the loop is already an octave above it.
        let tail = &buf[48000 * 2..48000 * 4];
        let octave = energy_at(tail, 800.0, sr);
        let fundamental = energy_at(tail, 400.0, sr);
        assert!(rms(tail) > 1e-5, "there should still be a tail");
        assert!(
            octave > fundamental * 1.5,
            "the tail should have climbed: 800={octave} 400={fundamental}"
        );
        // And a second octave behind it, from the pass after that.
        assert!(
            energy_at(tail, 1600.0, sr) > fundamental * 0.5,
            "the second pass should be up there too"
        );
    }

    /// With no feedback it is a reverb and nothing else: the tail stays where
    /// the input was.
    #[test]
    fn without_feedback_nothing_climbs() {
        let sr = 48000.0;
        let mut s = ShimmerReverb::new(48000);
        s.set_feedback(0.0);
        s.set_mix(1.0);
        let mut buf = burst(96000, 400.0, sr, 12000);
        s.process_block(&mut buf, 48000);
        let tail = &buf[24000 * 2..48000 * 2];
        assert!(
            energy_at(tail, 400.0, sr) > energy_at(tail, 800.0, sr),
            "no feedback, no shimmer"
        );
    }

    /// A reverb, a resampler and a filter in one loop. Modest feedback has to
    /// die away; full feedback has to settle into a wash rather than grow —
    /// and, either way, stay somewhere near the level it was given.
    #[test]
    fn the_loop_decays_at_low_feedback_and_is_bounded_at_full() {
        let sr = 48000.0;
        let run = |knob: f32| {
            let mut s = ShimmerReverb::new(48000);
            s.set_feedback(knob);
            s.set_mix(1.0);
            let mut buf = burst(48000 * 10, 300.0, sr, 4800);
            s.process_block(&mut buf, 48000);
            assert!(buf.iter().all(|x| x.is_finite()));
            let early = rms(&buf[24000 * 2..36000 * 2]);
            let late = rms(&buf[(48000 * 9) * 2..]);
            let peak = buf.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            (early, late, peak)
        };
        let (early, late, _) = run(0.25);
        assert!(
            late < early * 0.01,
            "a quarter of the feedback should die away: {early} → {late}"
        );
        // The input burst peaks at 0.5; a full-feedback wash may sustain, but
        // not at ten times what started it.
        let (_, late, peak) = run(1.0);
        assert!(
            late < 2.0 && peak < 5.0,
            "the wash ran hot: late={late} peak={peak}"
        );
    }

    /// The block size cannot change the result: the inner reverb is fed in
    /// chunks, and the chunking must not be audible.
    #[test]
    fn the_block_size_does_not_change_the_result() {
        let run = |chunk: usize| {
            let mut s = ShimmerReverb::new(48000);
            s.set_feedback(0.7);
            s.set_mix(1.0);
            let mut buf = burst(24000, 300.0, 48000.0, 4800);
            for part in buf.chunks_mut(chunk * 2) {
                s.process_block(part, 48000);
            }
            buf
        };
        let a = run(512);
        let b = run(97);
        let worst = a
            .iter()
            .zip(b.iter())
            .fold(0.0f32, |m, (x, y)| m.max((x - y).abs()));
        assert!(worst < 1e-4, "the chunking is audible: off by {worst}");
    }

    #[test]
    fn it_survives_silence_extremes_and_a_rate_change() {
        let mut s = ShimmerReverb::with_params(48000, &[0.9, 0.5, 1.0, 1.0, 0.5, 1.0]);
        let mut buf = vec![0.0f32; 4096];
        s.process_block(&mut buf, 48000);
        assert!(buf.iter().all(|x| x.is_finite()));
        let mut hot = vec![4.0f32; 8192];
        s.process_block(&mut hot, 96000);
        assert!(hot.iter().all(|x| x.is_finite()));
        s.process_block(&mut [], 96000);
        s.process_block(&mut [1.0], 96000);
        s.reset();
    }
}
