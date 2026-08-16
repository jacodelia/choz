//! Parameter smoothing.
//!
//! A knob is set between blocks; a gain is read every sample. Jump one to the
//! other and the waveform gets a step in it, which is a click — and turning the
//! knob continuously turns that into zipper noise, the sound of a parameter
//! being quantised to the block rate.
//!
//! One pole, because it is the cheapest thing that has no corner: the value
//! approaches its target asymptotically, so there is no instant where the
//! derivative jumps. What is *not* smoothed matters as much: a filter cutoff
//! usually wants this, a sample-rate reducer's bit depth does not (stepping is
//! the effect), and anything that selects a mode must never be smoothed at all
//! — half way between two modes is not a mode.

/// A value that walks to its target instead of jumping there.
#[derive(Debug, Clone, Copy)]
pub struct Smoothed {
    current: f32,
    target: f32,
    /// Per-sample coefficient, derived from the time constant and the rate.
    coeff: f32,
    ms: f32,
    sample_rate: f32,
}

impl Smoothed {
    /// `ms` is the time to cover ~63 % of a jump. 5–20 ms suits a gain; a
    /// cutoff can take longer without feeling laggy.
    pub fn new(value: f32, ms: f32, sample_rate: f32) -> Self {
        let mut s = Self {
            current: value,
            target: value,
            coeff: 0.0,
            ms: ms.max(0.1),
            sample_rate: sample_rate.max(8000.0),
        };
        s.recalc();
        s
    }

    fn recalc(&mut self) {
        // exp(-1 / (t · fs)): one time constant per `ms`.
        let samples = (self.ms * 0.001 * self.sample_rate).max(1.0);
        self.coeff = (-1.0 / samples).exp();
    }

    /// Follow a sample-rate change without moving the value.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        let sr = sample_rate.max(8000.0);
        if (sr - self.sample_rate).abs() > 0.5 {
            self.sample_rate = sr;
            self.recalc();
        }
    }

    pub fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    /// Go there now: what `reset()` does, and what a processor does when the
    /// parameter changed because the *preset* changed rather than because
    /// somebody turned it.
    pub fn snap(&mut self, value: f32) {
        self.current = value;
        self.target = value;
    }

    pub fn target(&self) -> f32 {
        self.target
    }

    pub fn value(&self) -> f32 {
        self.current
    }

    /// Advance one sample and return the value to use for it.
    ///
    /// Named `tick` rather than `next` on purpose: this is not an iterator and
    /// never ends, and a `next()` that always returns a value reads like one.
    #[inline]
    pub fn tick(&mut self) -> f32 {
        // Snap once the gap stops mattering. Not only to keep a denormal out of
        // every idle parameter: in `f32`, `target - diff·coeff` reaches a fixed
        // point while the gap is still ~1e-5 near 0.75, because the step falls
        // below the ulp of the result. Without this the value would sit one
        // rounding error short of the target forever — inaudible, but it makes
        // "did the parameter arrive" unanswerable.
        let diff = self.target - self.current;
        if diff.abs() < 1e-5 {
            self.current = self.target;
        } else {
            self.current = self.target - diff * self.coeff;
        }
        self.current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_walks_to_the_target_instead_of_jumping() {
        let mut s = Smoothed::new(0.0, 10.0, 48_000.0);
        s.set_target(1.0);
        let first = s.tick();
        assert!(first > 0.0 && first < 0.05, "no jump: {first}");

        // One time constant is ~63 % of the way.
        for _ in 1..(0.010 * 48_000.0) as usize {
            s.tick();
        }
        let after = s.value();
        assert!((0.60..0.68).contains(&after), "got {after}");
    }

    /// It has to actually arrive: a smoother that only ever approaches leaves
    /// every gain slightly wrong forever.
    #[test]
    fn it_arrives_and_stays() {
        let mut s = Smoothed::new(0.0, 5.0, 48_000.0);
        s.set_target(0.75);
        for _ in 0..48_000 {
            s.tick();
        }
        assert_eq!(s.value(), 0.75);
        assert_eq!(s.tick(), 0.75, "and does not drift off it");
    }

    /// The step never exceeds the distance left, in either direction — that is
    /// what "no click" means numerically.
    #[test]
    fn every_step_is_monotone_towards_the_target() {
        for (from, to) in [(0.0f32, 1.0f32), (1.0, -1.0), (-0.5, 0.5)] {
            let mut s = Smoothed::new(from, 8.0, 44_100.0);
            s.set_target(to);
            let mut prev = from;
            for _ in 0..44_100 {
                let v = s.tick();
                assert!(
                    (v - prev).abs() <= (to - from).abs(),
                    "overshoot: {prev} -> {v}"
                );
                assert!((v - to).abs() <= (prev - to).abs() + 1e-6, "went backwards");
                prev = v;
            }
        }
    }

    #[test]
    fn snap_is_immediate_and_a_rate_change_keeps_the_value() {
        let mut s = Smoothed::new(0.0, 10.0, 48_000.0);
        s.snap(0.3);
        assert_eq!(s.value(), 0.3);
        assert_eq!(s.tick(), 0.3);

        s.set_sample_rate(96_000.0);
        assert_eq!(s.value(), 0.3, "a rate change is not a parameter change");
        s.set_target(1.0);
        // Twice the rate, so the same time takes twice the samples.
        for _ in 0..(0.010 * 96_000.0) as usize {
            s.tick();
        }
        let after = s.value();
        assert!((0.72..0.80).contains(&after), "got {after}");
    }
}
