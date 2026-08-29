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

use super::delay_line::DelayLine as Line;

/// The window, in milliseconds.
///
/// The trade: long enough that the crossfade is not heard as a flutter, short
/// enough that its echo is not heard as a second sound. **In time and not in
/// samples** — it used to be a count of 2048, which is 43 ms at 48 kHz and
/// 11 at 192, so the same harmony warbled four times as fast on a fast
/// interface.
///
/// Written as the old count over the common rate rather than as a round
/// number: the shimmer's second octave is a resonance of this length against
/// the reverb inside its loop, and moving the window by two samples moves it.
/// Same sound at 48 kHz, and now the same sound everywhere else too.
pub const WINDOW_MS: f32 = 2048.0 / 48.0;

pub struct VoiceShifter {
    line: Line,
    /// The window in samples at the rate this shifter was last told about.
    window: f32,
    /// How far behind the writer the first head reads, in samples. The writer
    /// advances by 1 and the reader by `ratio`, so this closes at `ratio − 1`
    /// per sample and wraps round the window when it runs out.
    behind: f32,
    ratio: f32,
}

/// The gain of the first head at phase `t`, the second head getting `1 − g`.
///
/// A bump, not a ramp: the heads are half a window apart, so head one is
/// silent at both ends of the window and loudest in the middle, and head two
/// is the other way round. `smoothstep(u) + smoothstep(1 − u) == 1` exactly,
/// which is the one property the pair needs — the two heads read the same
/// signal, so their gains have to add to one or the level moves as they cross.
///
/// `sin²`/`cos²` had that property too, and cost two transcendentals per
/// sample per voice: eight voices of harmony is 768 000 of them a second, for
/// a curve three multiplies can draw.
#[inline(always)]
fn head_gain(t: f32) -> f32 {
    let u = 1.0 - (t + t - 1.0).abs();
    u * u * (3.0 - u - u)
}

impl Default for VoiceShifter {
    fn default() -> Self {
        Self::new()
    }
}

impl VoiceShifter {
    pub fn new() -> Self {
        // Two windows: the second head reads half a window away from the
        // first, and both have to stay inside the line.
        let mut s = Self {
            line: Line::with_ms(WINDOW_MS * 2.0),
            window: 0.0,
            behind: 0.0,
            ratio: 1.0,
        };
        s.set_sample_rate(48_000.0);
        s
    }

    /// The window is a length of **time**, so it needs to know the rate. Call
    /// it from the block: it does nothing when the rate has not moved.
    pub fn set_sample_rate(&mut self, sr: f32) {
        // Rounded to whole samples: the wrap then lands on a sample boundary
        // every time round, which keeps the crossfade's own sidebands where
        // they were rather than smearing them with a fractional period.
        let window = (WINDOW_MS * 0.001 * sr.max(8000.0))
            .round()
            .min((self.line.capacity() / 2 - 4) as f32)
            .max(64.0);
        if (window - self.window).abs() < 0.5 {
            return;
        }
        self.window = window;
        self.behind = window * 0.5;
    }

    pub fn reset(&mut self) {
        self.line.clear();
        self.behind = self.window * 0.5;
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

    /// One sample in, one sample out, pitched by `ratio`.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let win = self.window;
        self.line.write(x);

        // Reading faster than writing closes the gap; reading slower opens it.
        self.behind += 1.0 - self.ratio;
        self.behind = self.behind.rem_euclid(win);

        // Cubic on both heads. This is a resampler — the head walks the line at
        // a fraction of a sample per sample — and a two-tap average is a
        // low-pass whose depth follows the fraction, so with linear reads a
        // held note is dulled by an amount that moves as the head does.
        let t = self.behind / win;
        let g = head_gain(t);
        self.line.read_cubic(self.behind) * g
            + self.line.read_cubic(self.behind + win * 0.5) * (1.0 - g)
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

    /// The top end survives the read.
    ///
    /// A ratio just off unity walks the read head slowly through every
    /// fraction of a sample, which is where an interpolator is judged: a
    /// two-tap average is a low-pass whose depth follows the fraction, so with
    /// linear reads a held 14 kHz came out 3.6 dB down, against 2.1 with the
    /// four-point cubic. The rest of that 2.1 dB is the two heads themselves:
    /// they are half a window apart, and summing a high note with a delayed
    /// copy of itself combs it whatever reads it.
    #[test]
    fn a_high_note_comes_through_the_read() {
        let sr = 48_000.0;
        let mut sh = VoiceShifter::new();
        sh.set_semitones(0.5);
        let out: Vec<f32> = (0..48_000)
            .map(|i| sh.process((std::f32::consts::TAU * 14_000.0 * i as f32 / sr).sin() * 0.5))
            .collect();
        let tail = &out[24_000..];
        let rms = (tail.iter().map(|s| s * s).sum::<f32>() / tail.len() as f32).sqrt();
        let db = 20.0 * (rms / 0.353_55).log10();
        assert!(db > -2.8, "14 kHz came out {db:.2} dB down");
    }

    /// The window is a length of time, so the warble it puts on a held note is
    /// the same speed on every device. As a count of samples it was four times
    /// faster at 192 kHz than at 48.
    #[test]
    fn the_window_is_the_same_time_at_every_rate() {
        for sr in [44_100.0f32, 48_000.0, 96_000.0, 192_000.0] {
            let mut sh = VoiceShifter::new();
            sh.set_sample_rate(sr);
            let ms = sh.window * 1000.0 / sr;
            assert!(
                (ms - WINDOW_MS).abs() < 0.1,
                "{sr} Hz gives a {ms} ms window, wanted {WINDOW_MS}"
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
