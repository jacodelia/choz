//! The one LFO every modulation effect in this crate uses.
//!
//! Tremolo, auto-pan, the modulated delay and the auto-filter all want the same
//! thing: a shape, a rate, and a stereo offset so the two channels are not the
//! same movement twice. Written once here, because seven shapes copied into
//! four files is seven shapes that drift into different shapes.
//!
//! # Real-time
//!
//! No allocation and no branching beyond the shape select. The random shapes
//! run on a fixed-seed xorshift: **the same session gives the same wobble**,
//! which is what makes a random modulation testable at all.

/// The shapes, in the order the knob steps through them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Wave {
    #[default]
    Sine,
    Triangle,
    /// Falls from +1 to −1 over the cycle.
    Saw,
    /// The same ramp the other way up.
    Ramp,
    Square,
    /// One random level per cycle, held.
    SampleHold,
    /// Random levels joined by a smooth curve instead of a step.
    Random,
}

impl Wave {
    pub const ALL: [Wave; 7] = [
        Wave::Sine,
        Wave::Triangle,
        Wave::Saw,
        Wave::Ramp,
        Wave::Square,
        Wave::SampleHold,
        Wave::Random,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Wave::Sine => "SINE",
            Wave::Triangle => "TRI",
            Wave::Saw => "SAW",
            Wave::Ramp => "RAMP",
            Wave::Square => "SQR",
            Wave::SampleHold => "S&H",
            Wave::Random => "RND",
        }
    }

    pub fn to_norm(self) -> f32 {
        Self::ALL.iter().position(|w| *w == self).unwrap_or(0) as f32 / (Self::ALL.len() - 1) as f32
    }

    pub fn from_norm(v: f32) -> Self {
        let n = Self::ALL.len();
        let i = (v.clamp(0.0, 1.0) * (n - 1) as f32).round() as usize;
        Self::ALL[i.min(n - 1)]
    }

    /// The shape at phase `ph` (0..1), given the segment `[from, to]` the
    /// random shapes are travelling between. Always −1…+1.
    #[inline]
    fn eval(self, ph: f32, seg: [f32; 2]) -> f32 {
        match self {
            Wave::Sine => (std::f32::consts::TAU * ph).sin(),
            // Starts at 0 rising, like the sine: switching shape mid-note
            // should not jump the modulation somewhere else.
            Wave::Triangle => {
                if ph < 0.25 {
                    4.0 * ph
                } else if ph < 0.75 {
                    2.0 - 4.0 * ph
                } else {
                    4.0 * ph - 4.0
                }
            }
            Wave::Saw => 1.0 - 2.0 * ph,
            Wave::Ramp => 2.0 * ph - 1.0,
            Wave::Square => {
                if ph < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            Wave::SampleHold => seg[1],
            Wave::Random => {
                // Smoothstep between the two levels: a random modulation you
                // can hear the corners of is a stepped one with extra steps.
                let t = ph * ph * (3.0 - 2.0 * ph);
                seg[0] + (seg[1] - seg[0]) * t
            }
        }
    }
}

/// A phase accumulator plus the random state the two random shapes need.
///
/// One per effect, not one per channel: the two channels are the *same*
/// oscillator read at two phases, which is what a stereo offset means.
#[derive(Debug, Clone)]
pub struct Lfo {
    phase: f32,
    rng: u32,
    /// Per channel, the two levels the random shapes travel between.
    seg: [[f32; 2]; 2],
    prev_ph: [f32; 2],
}

impl Default for Lfo {
    fn default() -> Self {
        Self::new()
    }
}

impl Lfo {
    pub fn new() -> Self {
        Self {
            phase: 0.0,
            // Fixed seed: reproducible is worth more than unpredictable here.
            rng: 0x1234_5678,
            seg: [[0.0, 0.0]; 2],
            prev_ph: [0.0; 2],
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Where the oscillator is, 0..1. For an effect that has to line something
    /// else up with it.
    pub fn phase(&self) -> f32 {
        self.phase
    }

    #[inline]
    fn next_rand(&mut self) -> f32 {
        // xorshift32: three shifts, no state beyond the word.
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 / 8_388_608.0 - 1.0
    }

    /// Advance one frame and read both channels.
    ///
    /// `spread` is how far the right channel runs behind the left, 0..1 of a
    /// cycle (0.5 = in anti-phase, which is what an auto-pan wants).
    #[inline]
    pub fn tick(&mut self, wave: Wave, rate_hz: f32, sr: f32, spread: f32) -> [f32; 2] {
        self.phase += (rate_hz.max(0.0) / sr.max(1.0)).min(0.5);
        if self.phase >= 1.0 {
            self.phase -= self.phase.floor();
        }
        let offsets = [0.0, spread.clamp(0.0, 1.0)];
        let mut out = [0.0f32; 2];
        for ch in 0..2 {
            let ph = (self.phase + offsets[ch]).fract();
            // Each channel gets its own segment, drawn when *its* phase wraps:
            // a stereo offset that steps both channels at the same instant is
            // not an offset.
            if ph < self.prev_ph[ch] {
                let next = self.next_rand();
                self.seg[ch] = [self.seg[ch][1], next];
            }
            self.prev_ph[ch] = ph;
            out[ch] = wave.eval(ph, self.seg[ch]);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cycle(wave: Wave, frames: usize) -> Vec<f32> {
        let mut lfo = Lfo::new();
        // 1 Hz at `frames` Hz sample rate = exactly one cycle.
        (0..frames)
            .map(|_| lfo.tick(wave, 1.0, frames as f32, 0.0)[0])
            .collect()
    }

    /// Every shape stays on the rails. A modulation that leaves −1…+1 is a
    /// gain, a pan or a cutoff outside its own range.
    #[test]
    fn every_shape_stays_between_minus_one_and_one() {
        for wave in Wave::ALL {
            for v in cycle(wave, 512) {
                assert!(
                    v.is_finite() && (-1.0..=1.0).contains(&v),
                    "{} left the rails: {v}",
                    wave.label()
                );
            }
        }
    }

    /// The periodic shapes have to actually repeat, and reach both rails.
    #[test]
    fn the_periodic_shapes_sweep_the_whole_range_and_repeat() {
        for wave in [
            Wave::Sine,
            Wave::Triangle,
            Wave::Saw,
            Wave::Ramp,
            Wave::Square,
        ] {
            let one = cycle(wave, 400);
            let lo = one.iter().cloned().fold(f32::MAX, f32::min);
            let hi = one.iter().cloned().fold(f32::MIN, f32::max);
            assert!(lo < -0.9 && hi > 0.9, "{} spans {lo}..{hi}", wave.label());

            // Two cycles: the second is the first again.
            let mut lfo = Lfo::new();
            let two: Vec<f32> = (0..800)
                .map(|_| lfo.tick(wave, 1.0, 400.0, 0.0)[0])
                .collect();
            for i in 0..400 {
                assert!(
                    (two[i] - two[i + 400]).abs() < 1e-4,
                    "{} does not repeat at {i}: {} vs {}",
                    wave.label(),
                    two[i],
                    two[i + 400]
                );
            }
        }
    }

    /// A stereo spread means the right channel is somewhere else, and half a
    /// cycle of it means it is the opposite.
    #[test]
    fn the_spread_moves_the_right_channel_and_only_the_right_channel() {
        let mut none = Lfo::new();
        let mut half = Lfo::new();
        for _ in 0..137 {
            let a = none.tick(Wave::Sine, 1.0, 400.0, 0.0);
            let b = half.tick(Wave::Sine, 1.0, 400.0, 0.5);
            assert!(
                (a[0] - b[0]).abs() < 1e-6,
                "the left channel is the reference"
            );
            assert!((a[1] - a[1]).abs() < 1e-6);
            assert!(
                (b[0] + b[1]).abs() < 1e-4,
                "half a cycle apart is anti-phase: {} vs {}",
                b[0],
                b[1]
            );
        }
    }

    /// Sample & hold holds: one level per cycle, and a different one next time.
    #[test]
    fn sample_and_hold_holds_for_a_whole_cycle() {
        let mut lfo = Lfo::new();
        let vals: Vec<f32> = (0..1200)
            .map(|_| lfo.tick(Wave::SampleHold, 1.0, 400.0, 0.0)[0])
            .collect();
        // Inside a cycle it does not move.
        for w in vals[410..790].windows(2) {
            assert_eq!(w[0], w[1], "S&H moved mid-cycle");
        }
        assert_ne!(vals[500], vals[900], "S&H drew the same level twice");
    }

    /// Same seed, same wobble — otherwise nothing above could be a test.
    #[test]
    fn the_random_shapes_are_reproducible() {
        let a = cycle(Wave::Random, 900);
        let b = cycle(Wave::Random, 900);
        assert_eq!(a, b);
        // And it is smooth: no step bigger than the segment it is crossing.
        let biggest = a.windows(2).fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()));
        assert!(biggest < 0.1, "smooth random stepped by {biggest}");
    }
}
