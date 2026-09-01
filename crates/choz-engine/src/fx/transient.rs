//! Transient shaper: the attack and the tail, on separate knobs.
//!
//! Not a compressor. A compressor works from a threshold, so how much it does
//! depends on how loud the take is; this works from the *shape* of the
//! envelope, so a snare hit at −20 dB and the same hit at −6 get the same
//! treatment. Two envelope followers chase the signal at different speeds and
//! the difference between them is the transient: fast above slow is the stick
//! hitting the head, fast below slow is the room going on afterwards.
//!
//! `Attack` above the middle brings the stick forward and below it takes it
//! away; `Sustain` does the same to the tail.

use choz_ports::{FxParam, FxProcessor};

/// The two followers, in ms. Fast is short enough to ride a drum hit, slow is
/// long enough to be the level it stands out from.
const FAST_MS: f32 = 3.0;
const SLOW_MS: f32 = 60.0;

/// Most a knob at its end can do, in dB. ±15 is enough to rebuild a drum and
/// short of where the shaper starts to sound like a gate.
const RANGE_DB: f32 = 15.0;

pub struct TransientShaper {
    /// −1..1: the attack half of the shape.
    attack: f32,
    /// −1..1: the sustain half.
    sustain: f32,
    wet: f32,
    fast: [f32; 2],
    slow: [f32; 2],
    fast_c: f32,
    slow_c: f32,
    sample_rate: f32,
}

impl TransientShaper {
    pub fn new(sample_rate: u32) -> Self {
        let mut t = Self {
            attack: 0.0,
            sustain: 0.0,
            wet: 1.0,
            fast: [0.0; 2],
            slow: [0.0; 2],
            fast_c: 0.0,
            slow_c: 0.0,
            sample_rate: 0.0,
        };
        t.refresh(sample_rate.max(8000) as f32);
        t
    }

    pub fn with_params(sample_rate: u32, params: &[f32]) -> Self {
        let mut t = Self::new(sample_rate);
        for (i, p) in params.iter().enumerate() {
            <Self as FxProcessor>::set_param(&mut t, i, *p);
        }
        t
    }

    fn refresh(&mut self, sr: f32) {
        self.sample_rate = sr;
        self.fast_c = (-1.0 / (FAST_MS * 0.001 * sr)).exp();
        self.slow_c = (-1.0 / (SLOW_MS * 0.001 * sr)).exp();
    }
}

impl FxProcessor for TransientShaper {
    fn name(&self) -> &str {
        "Transient Shaper"
    }

    fn params(&self) -> Vec<FxParam> {
        vec![
            FxParam::new(
                "Attack",
                (self.attack + 1.0) * 0.5,
                -RANGE_DB,
                RANGE_DB,
                "dB",
            ),
            FxParam::new(
                "Sustain",
                (self.sustain + 1.0) * 0.5,
                -RANGE_DB,
                RANGE_DB,
                "dB",
            ),
            FxParam::new("Wet", self.wet, 0.0, 1.0, ""),
        ]
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => self.attack = v * 2.0 - 1.0,
            1 => self.sustain = v * 2.0 - 1.0,
            2 => self.wet = v,
            _ => {}
        }
    }

    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        let sr = sample_rate.max(8000) as f32;
        if (sr - self.sample_rate).abs() > f32::EPSILON {
            self.refresh(sr);
        }
        for frame in buf.as_chunks_mut::<2>().0 {
            for (ch, s) in frame.iter_mut().enumerate() {
                let dry = *s;
                let level = dry.abs();
                // The fast one jumps to a peak and falls in milliseconds; the
                // slow one has to *climb* as well. Both rising instantly was
                // the first version, and it detected nothing: two followers
                // that jump together are equal for the whole of the attack,
                // which is precisely the part being shaped.
                self.fast[ch] = level.max(self.fast[ch] * self.fast_c);
                self.slow[ch] = level + self.slow_c * (self.slow[ch] - level);
                // How far above the slow envelope the fast one is, in dB, and
                // whether we are in the attack (positive) or the tail.
                let diff = 20.0
                    * ((self.fast[ch] + 1e-9) / (self.slow[ch] + 1e-9))
                        .max(1e-6)
                        .log10();
                // `diff` is ~0 inside a steady sound and swings positive on a
                // hit; the tail is where the fast follower has fallen below.
                let shape = (diff / 6.0).clamp(-1.0, 1.0);
                let db = match shape >= 0.0 {
                    true => shape * self.attack * RANGE_DB,
                    false => -shape * self.sustain * RANGE_DB,
                };
                let processed = dry * 10f32.powf(db / 20.0);
                *s = dry + self.wet * (processed - dry);
            }
        }
    }

    fn reset(&mut self) {
        self.fast = [0.0; 2];
        self.slow = [0.0; 2];
    }

    fn set_mix(&mut self, wet: f32) {
        self.wet = wet.clamp(0.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One drum-shaped hit: a click and a decaying tail. The attack knob has to
    /// move the first milliseconds and leave the tail where it is.
    fn hit(sr: u32) -> Vec<f32> {
        let n = (sr as f32 * 0.4) as usize;
        (0..n)
            .flat_map(|i| {
                let t = i as f32 / sr as f32;
                let env = (-t * 12.0).exp();
                let v = (std::f32::consts::TAU * 180.0 * t).sin() * env * 0.6;
                [v, v]
            })
            .collect()
    }

    fn peak(buf: &[f32], from: usize, to: usize) -> f32 {
        buf[from * 2..to * 2].iter().fold(0.0f32, |m, s| m.max(s.abs()))
    }

    #[test]
    fn the_attack_knob_moves_the_stick_and_not_the_tail() {
        let sr = 48_000;
        let shaped = |attack: f32| {
            let mut fx = TransientShaper::new(sr);
            fx.set_param(0, attack);
            fx.set_param(1, 0.5);
            let mut buf = hit(sr);
            fx.process_block(&mut buf, sr);
            buf
        };
        let flat = shaped(0.5);
        let up = shaped(1.0);
        let down = shaped(0.0);

        let head = |b: &[f32]| peak(b, 0, sr as usize / 200); // first 5 ms
        let tail = |b: &[f32]| peak(b, sr as usize / 5, sr as usize / 4); // 200–250 ms
        assert!(
            head(&up) > head(&flat) * 1.5,
            "the attack was not brought forward: {} against {}",
            head(&up),
            head(&flat)
        );
        assert!(
            head(&down) < head(&flat) * 0.8,
            "and it was not taken away either"
        );
        let moved = (tail(&up) - tail(&flat)).abs() / tail(&flat);
        assert!(moved < 0.1, "the tail moved with it, by {moved}");
    }
}
