//! One pitch shifter, shared by everything that needs a transposed copy.
//!
//! Two read heads walking a delay line at `ratio`, half a window apart,
//! crossfaded by `sin²`/`cos²` — which sum to exactly one, so the wrap of
//! either head lands where that head is silent and nothing is heard jumping.
//!
//! # Why this one and not the other one
//!
//! [`super::autotune::shifter::RetuneShifter`] cuts its jumps on a **detected
//! period**, which is what makes it clean on a voice at ratios near 1 — it is
//! a corrector's shifter, and it needs a detector behind it. This one takes any
//! ratio, needs nothing, and pays for it with a light warble that is inaudible
//! inside a reverb tail and characterful inside a harmony. Two shifters,
//! because they are for two different jobs; one implementation of each, because
//! the shimmer and the harmoniser wanted the same one and were about to have a
//! copy each.

/// How much delay line one voice gets.
///
/// The window is the trade: long enough that the crossfade is not heard as a
/// flutter, short enough that the echo of it is not heard as a second sound.
/// ~46 ms at 48 kHz.
pub const SHIFT_WINDOW: usize = 2048;

pub struct VoiceShifter {
    buf: Vec<f32>,
    write: usize,
    /// How far behind the writer the first head reads, in samples. The writer
    /// advances by 1 and the reader by `ratio`, so this closes at `ratio − 1`
    /// per sample and wraps round the window when it runs out.
    behind: f32,
    ratio: f32,
}

impl Default for VoiceShifter {
    fn default() -> Self {
        Self::new()
    }
}

impl VoiceShifter {
    pub fn new() -> Self {
        Self {
            // Two windows: the second head reads half a window away from the
            // first, and both have to stay inside the buffer.
            buf: vec![0.0; SHIFT_WINDOW * 2],
            write: 0,
            behind: SHIFT_WINDOW as f32 * 0.5,
            ratio: 1.0,
        }
    }

    pub fn reset(&mut self) {
        self.buf.fill(0.0);
        self.write = 0;
        self.behind = SHIFT_WINDOW as f32 * 0.5;
    }

    /// Above 1 raises. Set on a block boundary, not per sample: half way
    /// between two ratios is a ratio, but it is not one anybody asked for.
    pub fn set_ratio(&mut self, ratio: f32) {
        self.ratio = ratio.clamp(0.125, 8.0);
    }

    /// Set the shift in semitones, which is how a musician says it.
    pub fn set_semitones(&mut self, semitones: f32) {
        self.set_ratio(2.0f32.powf(semitones / 12.0));
    }

    pub fn ratio(&self) -> f32 {
        self.ratio
    }

    #[inline]
    fn read_behind(&self, distance: f32) -> f32 {
        let cap = self.buf.len() as f32;
        let p = (self.write as f32 - distance).rem_euclid(cap);
        let i = p as usize;
        let frac = p - i as f32;
        let n = self.buf.len();
        let a = self.buf[i % n];
        let b = self.buf[(i + 1) % n];
        a + (b - a) * frac
    }

    /// One sample in, one sample out, pitched by `ratio`.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let win = SHIFT_WINDOW as f32;
        self.buf[self.write] = x;
        self.write = (self.write + 1) % self.buf.len();

        // Reading faster than writing closes the gap; reading slower opens it.
        self.behind += 1.0 - self.ratio;
        self.behind = self.behind.rem_euclid(win);

        let t = self.behind / win;
        let g1 = (std::f32::consts::PI * t).sin();
        let g2 = (std::f32::consts::PI * t).cos();
        self.read_behind(self.behind) * g1 * g1
            + self.read_behind(self.behind + win * 0.5) * g2 * g2
    }

    /// Read the line `distance` samples back without shifting it — the same
    /// buffer a voice is already writing, used as that voice's delay.
    ///
    /// A harmony that arrives at exactly the same instant as the note under it
    /// is a chorus; one that arrives a few tens of milliseconds later is a
    /// second singer. Reusing the shifter's own line means the delay costs no
    /// memory of its own.
    #[inline]
    pub fn tap(&self, distance: f32) -> f32 {
        self.read_behind(distance.clamp(0.0, (SHIFT_WINDOW * 2 - 2) as f32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn energy_at(buf: &[f32], probe: f32, sr: f32) -> f32 {
        let n = buf.len() as f32;
        let k = (probe * n / sr).round();
        let w = std::f32::consts::TAU * k / n;
        let coeff = 2.0 * w.cos();
        let (mut s1, mut s2) = (0.0f32, 0.0f32);
        for x in buf {
            let s0 = x + coeff * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        ((s1 * s1 + s2 * s2 - coeff * s1 * s2).max(0.0)).sqrt() / n
    }

    /// The whole claim: what comes out is the same sound at another pitch.
    #[test]
    fn it_moves_the_pitch_by_the_ratio_it_was_given() {
        let sr = 48_000.0;
        let run = |semitones: f32| -> Vec<f32> {
            let mut sh = VoiceShifter::new();
            sh.set_semitones(semitones);
            (0..48_000)
                .map(|i| sh.process((std::f32::consts::TAU * 400.0 * i as f32 / sr).sin() * 0.5))
                .collect()
        };
        // An octave up, a fifth up, an octave down.
        for (semis, expect) in [(12.0f32, 800.0f32), (7.0, 599.0), (-12.0, 200.0)] {
            let out = run(semis);
            let tail = &out[24_000..];
            let there = energy_at(tail, expect, sr);
            let here = energy_at(tail, 400.0, sr);
            assert!(
                there > here * 3.0,
                "{semis} semitones should land at {expect} Hz: {there} vs {here} at the original"
            );
        }
    }

    /// Unity is a wire, near enough: at ratio 1 the two heads read the same
    /// signal and their gains still sum to one.
    #[test]
    fn a_ratio_of_one_passes_the_signal() {
        let sr = 48_000.0;
        let mut sh = VoiceShifter::new();
        sh.set_semitones(0.0);
        let out: Vec<f32> = (0..8192)
            .map(|i| sh.process((std::f32::consts::TAU * 300.0 * i as f32 / sr).sin() * 0.5))
            .collect();
        let peak = out[4096..].iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(
            (peak - 0.5).abs() < 0.05,
            "unity should keep the level: {peak}"
        );
    }

    /// Whatever it is handed, it stays on the rails: a shifter that can run
    /// away takes the mix bus with it.
    #[test]
    fn it_stays_bounded_at_every_ratio() {
        for semis in [-24.0f32, -12.0, -7.0, 0.0, 7.0, 12.0, 24.0, 36.0] {
            let mut sh = VoiceShifter::new();
            sh.set_semitones(semis);
            let mut peak = 0.0f32;
            for i in 0..20_000 {
                let x = if i % 97 == 0 { 1.0 } else { -0.9 };
                let y = sh.process(x);
                assert!(y.is_finite(), "{semis} went non-finite");
                peak = peak.max(y.abs());
            }
            assert!(peak <= 1.2, "{semis} amplified to {peak}");
        }
    }
}
