//! Pitch-synchronous overlap-add: a voice moved in pitch **with its formants
//! left where they were**.
//!
//! The delay-line shifter ([`super::shift::VoiceShifter`]) changes pitch by
//! reading the signal faster or slower, which stretches *everything* by the
//! same ratio — the voice's resonances included. An octave down puts the
//! vowels an octave down too: the voice sounds like it is sung inside a tube,
//! and the further the harmony is from the singer the less of the words come
//! through. That is what was heard.
//!
//! PSOLA does not stretch. It cuts the input into grains two periods long,
//! centred a whole number of periods apart so each one starts at the same
//! point of the waveform, and lays them back down at the **new** period. The
//! spacing sets the pitch; the grains themselves are the original sound, so
//! what is inside one period — the vowel — is untouched.
//!
//! Unvoiced sound (a consonant, a breath) has no period. It is cut into short
//! grains at a fixed spacing, which moves nothing: a sibilant is not a note,
//! and pitching it is how a harmoniser lisps.
//!
//! Real-time: the history is one ring shared by every voice, written once a
//! sample; a voice is a handful of grains and two numbers. Nothing allocates
//! after construction.

use std::f32::consts::PI;

/// History kept, in samples: two periods of the lowest voice (50 Hz) and the
/// latency in front of them, at 192 kHz.
const RING: usize = 16_384;
const MASK: usize = RING - 1;

/// Grains a voice can have sounding at once: two octaves up overlaps eight.
const MAX_GRAINS: usize = 12;

/// Lowest and highest pitch a period is taken from.
const MIN_HZ: f32 = 50.0;
const MAX_HZ: f32 = 1_000.0;

/// The grain spacing used when there is no pitch: 5 ms, short enough that a
/// consonant keeps its edges.
const UNVOICED_S: f32 = 0.005;

/// The input as every voice reads it: its recent past and its period.
pub struct PsolaSource {
    ring: Vec<f32>,
    /// Samples written so far — absolute time, which is what the grid of
    /// grain centres is laid on.
    now: u64,
    /// The period grains are cut at, in samples.
    period: f32,
    /// Whether that period is a pitch that was heard. Without one the grains
    /// are laid down as they were cut — nothing moves.
    voiced: bool,
    sample_rate: f32,
}

impl PsolaSource {
    pub fn new(sample_rate: f32) -> Self {
        let mut s = Self {
            ring: vec![0.0; RING],
            now: 0,
            period: 0.0,
            voiced: false,
            sample_rate: 0.0,
        };
        s.set_sample_rate(sample_rate);
        s
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(8_000.0);
        self.set_pitch(None);
    }

    /// The pitch being sung, or `None` when there is none.
    pub fn set_pitch(&mut self, hz: Option<f32>) {
        let hz = hz.filter(|h| h.is_finite() && *h > 0.0);
        self.voiced = hz.is_some();
        self.period = match hz {
            Some(h) => self.sample_rate / h.clamp(MIN_HZ, MAX_HZ),
            None => self.sample_rate * UNVOICED_S,
        };
    }

    /// One input sample.
    #[inline]
    pub fn push(&mut self, x: f32) {
        self.ring[(self.now as usize) & MASK] = x;
        self.now += 1;
    }

    pub fn reset(&mut self) {
        self.ring.fill(0.0);
    }
}

#[derive(Clone, Copy, Default)]
struct Grain {
    /// First input sample, absolute.
    start: u64,
    len: u32,
    age: u32,
    active: bool,
}

/// One voice: the grains it has sounding and when the next is due.
pub struct PsolaVoice {
    grains: [Grain; MAX_GRAINS],
    /// Output samples until the next grain starts.
    next: f64,
    ratio: f32,
}

impl Default for PsolaVoice {
    fn default() -> Self {
        Self::new()
    }
}

impl PsolaVoice {
    pub fn new() -> Self {
        Self {
            grains: [Grain::default(); MAX_GRAINS],
            next: 0.0,
            ratio: 1.0,
        }
    }

    /// Above 1 raises. Kept inside four octaves either way.
    pub fn set_ratio(&mut self, ratio: f32) {
        self.ratio = ratio.clamp(0.25, 4.0);
    }

    pub fn set_semitones(&mut self, semitones: f32) {
        self.set_ratio(2f32.powf(semitones / 12.0));
    }

    pub fn reset(&mut self) {
        self.grains = [Grain::default(); MAX_GRAINS];
        self.next = 0.0;
    }

    /// One output sample. Call after the source has had this sample pushed.
    #[inline]
    pub fn process(&mut self, src: &PsolaSource) -> f32 {
        let period = src.period.max(2.0);
        // Unvoiced, the grains keep their spacing: a consonant is not a note,
        // and spacing noise grains closer only adds a buzz at the new rate.
        let ratio = match src.voiced {
            true => self.ratio,
            false => 1.0,
        };
        if self.next <= 0.0 {
            self.spawn(src, period);
            self.next += (period / ratio) as f64;
        }
        self.next -= 1.0;

        let mut out = 0.0;
        for g in self.grains.iter_mut().filter(|g| g.active) {
            let x = src.ring[((g.start + g.age as u64) as usize) & MASK];
            let w = (PI * g.age as f32 / g.len as f32).sin();
            out += x * w * w;
            g.age += 1;
            if g.age >= g.len {
                g.active = false;
            }
        }
        // Each grain is one period's pulse, and there are `ratio` times as
        // many of them a second as there were periods: the power is `ratio`
        // times the input's, and so the level `√ratio`.
        out / ratio.sqrt()
    }

    /// Start a grain two periods long, **centred on the pulse** of the last
    /// whole period that has been heard in full.
    ///
    /// The centre is the period's peak, not a point on a clock. A grain
    /// centred between two pulses carries half of each, and laid down at a
    /// new spacing those halves become two pulses a period — a buzz at the
    /// wrong pitch, and a level that swung ±6 dB with the interval. On the
    /// peak, every grain is one pulse with the vowel around it, which is what
    /// makes moving them move the pitch and nothing else. Consecutive grains
    /// may find the same peak (raising repeats a period) or skip one
    /// (lowering drops one); both are what PSOLA does.
    fn spawn(&mut self, src: &PsolaSource, period: f32) {
        let Some(slot) = self.grains.iter_mut().find(|g| !g.active) else {
            return;
        };
        let p = period.round().max(2.0) as u64;
        // The window the centre is looked for in: the last period that still
        // has a whole period after it.
        let Some(hi) = src.now.checked_sub(p + 1) else {
            return;
        };
        let Some(lo) = hi.checked_sub(p) else {
            return;
        };
        let at = |t: u64| src.ring[(t as usize) & MASK];
        let centre = (lo..hi).fold(hi, |best, t| if at(t) > at(best) { t } else { best });
        *slot = Grain {
            start: centre.saturating_sub(p),
            len: (2 * p) as u32,
            age: 0,
            active: true,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A voice-like signal: a pulse train at `f0` through one resonance at
    /// `formant` — a vowel with one formant.
    fn vowel(f0: f32, formant: f32, sr: f32, n: usize) -> Vec<f32> {
        vowel_bw(f0, formant, 76.0, sr, n)
    }

    /// The same, with the resonance `bw` Hz wide.
    fn vowel_bw(f0: f32, formant: f32, bw: f32, sr: f32, n: usize) -> Vec<f32> {
        let (r, w) = ((-PI * bw / sr).exp(), 2.0 * PI * formant / sr);
        let (a1, a2) = (2.0 * r * w.cos(), -r * r);
        let (mut y1, mut y2) = (0.0f32, 0.0f32);
        let period = sr / f0;
        let mut next = 0.0f32;
        (0..n)
            .map(|i| {
                let x = if i as f32 >= next {
                    next += period;
                    1.0
                } else {
                    0.0
                };
                let y = x + a1 * y1 + a2 * y2;
                y2 = y1;
                y1 = y;
                y * 0.02
            })
            .collect()
    }

    /// Energy of `x` near `hz`, a Goertzel over a narrow band.
    fn band(x: &[f32], hz: f32, sr: f32) -> f32 {
        (-2..=2)
            .map(|k| {
                let f = hz + k as f32 * 20.0;
                let w = 2.0 * PI * f / sr;
                let c = 2.0 * w.cos();
                let (mut s1, mut s2) = (0.0f32, 0.0f32);
                for v in x {
                    let s0 = v + c * s1 - s2;
                    s2 = s1;
                    s1 = s0;
                }
                s1 * s1 + s2 * s2 - c * s1 * s2
            })
            .sum()
    }

    fn run(input: &[f32], sr: f32, f0: f32, semitones: f32) -> Vec<f32> {
        let mut src = PsolaSource::new(sr);
        src.set_pitch(Some(f0));
        let mut v = PsolaVoice::new();
        v.set_semitones(semitones);
        input
            .iter()
            .map(|x| {
                src.push(*x);
                v.process(&src)
            })
            .collect()
    }

    /// An octave down keeps the vowel where it was. The delay-line shifter
    /// took the 900 Hz resonance to 450 — the tube.
    #[test]
    fn an_octave_down_keeps_the_formant() {
        let sr = 48_000.0;
        let input = vowel(200.0, 900.0, sr, 48_000);
        let out = run(&input, sr, 200.0, -12.0);
        let tail = &out[12_000..];
        let kept = band(tail, 900.0, sr);
        let moved = band(tail, 450.0, sr);
        assert!(
            kept > moved * 4.0,
            "the formant moved: 900 Hz {kept:.3e} vs 450 Hz {moved:.3e}"
        );
    }

    /// …and the pitch did move: the output repeats at the new period.
    #[test]
    fn the_pitch_moves_by_the_ratio() {
        let sr = 48_000.0;
        let input = vowel(200.0, 900.0, sr, 48_000);
        for (semis, f) in [(-12.0f32, 100.0f32), (12.0, 400.0), (7.0, 299.66)] {
            let out = run(&input, sr, 200.0, semis);
            let tail = &out[12_000..];
            let at = band(tail, f, sr);
            let old = band(tail, 200.0, sr);
            // The old fundamental has to be the weaker one, except where the
            // new pitch's harmonics land on it (down an octave, 200 Hz is the
            // second harmonic of 100).
            if semis > 0.0 {
                assert!(
                    at > old * 2.0,
                    "{semis} st: {f} Hz {at:.3e} vs 200 Hz {old:.3e}"
                );
            } else {
                assert!(at > 1e-6, "{semis} st: nothing at {f} Hz");
            }
        }
    }

    /// Level: the same loudness in and out at every shift, within 3 dB.
    #[test]
    ///
    /// Against a formant as wide as a voice's (300 Hz): a narrow one makes the
    /// level depend on whether a harmonic of the new note lands on it, which
    /// is true of a singer too and not something to normalise away.
    fn a_shift_keeps_its_level() {
        let sr = 48_000.0;
        let rms = |x: &[f32]| (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
        for f0 in [110.0f32, 160.0, 250.0] {
            let input = vowel_bw(f0, 700.0, 300.0, sr, 48_000);
            let dry = rms(&input[12_000..]);
            for semis in [
                -24.0f32, -12.0, -7.0, -5.0, 0.0, 3.0, 4.0, 7.0, 12.0, 19.0, 24.0,
            ] {
                let out = run(&input, sr, f0, semis);
                let db = 20.0 * (rms(&out[12_000..]) / dry).log10();
                // 3.5, not 3: a 250 Hz voice two octaves up is a 1 kHz note
                // with no harmonic anywhere near a 700 Hz vowel, and it is
                // quieter for the same reason it would be sung quieter.
                assert!(
                    db.abs() < 3.5,
                    "{f0} Hz, {semis:+} st comes out {db:+.1} dB"
                );
            }
        }
    }
}
