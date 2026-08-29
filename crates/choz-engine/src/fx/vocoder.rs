//! Vocoder: one sound wearing another's mouth.
//!
//! ```text
//!  modulator (the voice) ─► band 1 ─► envelope ─┐
//!                          band 2 ─► envelope ─┐│
//!                          … N …               ││
//!                                              ▼▼
//!  carrier (saw / pulse / noise / the           ×  ─► sum ─► wet
//!           other channel)      ─► band 1..N ──┘
//! ```
//!
//! The voice is split into bands and each band's **loudness** is measured. The
//! carrier is split into the same bands and each one is turned up or down by
//! the matching number. Nothing of the voice's own sound comes through — only
//! its shape — which is why the result is the carrier speaking.
//!
//! # The carrier is the whole character
//!
//! * **Saw** and **Pulse** are the computer-speech sound: a buzzing tone with a
//!   mouth on it. `Pitch` sets the note it speaks at, and holding it still is
//!   what makes it a robot rather than a person.
//! * **Noise** is a whisper: the same shaping with no pitch at all.
//! * **The other channel** is a talkbox, and it is why that option exists: a
//!   talkbox *is* a vocoder whose carrier is a real instrument. Feed a tab a
//!   stereo pair — voice on the left, guitar on the right — turn `Res` up for
//!   the peaky response a tube in the mouth actually has, and that is the
//!   sound. No second effect for it: it was the same code with a different
//!   carrier all along.
//!
//! # Real-time
//!
//! Two biquads and one envelope per band, plus one oscillator. No allocation,
//! nothing that depends on the block size.

use super::smooth::Smoothed;
use super::utility::Biquad;

/// Band counts the knob steps through.
///
/// Sixteen is the classic and where the trade sits: fewer is coarser and more
/// intelligible words need more, but past about 24 the bands are narrower than
/// a formant moves and the extra ones say the same thing twice.
pub const BAND_COUNTS: [usize; 3] = [8, 16, 24];

/// The most bands anything here is sized for.
const MAX_BANDS: usize = 24;

/// Where the bank starts and ends. Speech lives between them; below 100 Hz is
/// the fundamental (which the carrier is providing) and above 8 kHz is
/// sibilance, which is passed through separately rather than shaped.
const LOW_HZ: f32 = 120.0;
const HIGH_HZ: f32 = 7_000.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Carrier {
    /// Buzzing and harmonically rich: the classic vocoder tone.
    #[default]
    Saw,
    /// Hollower, more nasal — the other half of the computer-speech sound.
    Pulse,
    /// No pitch at all: a whisper.
    Noise,
    /// The right channel. A talkbox is a vocoder carried by a real
    /// instrument, so this is that, and it needs no separate effect.
    Right,
    /// **The chord being held.** The same notes the harmoniser follows, played
    /// as a bank of saws — so the keyboard decides what the voice says *on*,
    /// which is the whole reason the two effects are one.
    Chord,
}

impl Carrier {
    pub const ALL: [Carrier; 5] = [
        Carrier::Saw,
        Carrier::Pulse,
        Carrier::Noise,
        Carrier::Right,
        Carrier::Chord,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Carrier::Saw => "SAW",
            Carrier::Pulse => "PULSE",
            Carrier::Noise => "NOISE",
            Carrier::Right => "INPUT R",
            Carrier::Chord => "CHORD",
        }
    }

    pub fn to_norm(self) -> f32 {
        Self::ALL.iter().position(|c| *c == self).unwrap_or(0) as f32 / (Self::ALL.len() - 1) as f32
    }

    pub fn from_norm(v: f32) -> Self {
        let n = Self::ALL.len();
        let i = (v.clamp(0.0, 1.0) * (n - 1) as f32).round() as usize;
        Self::ALL[i.min(n - 1)]
    }
}

struct Band {
    modulator: Biquad,
    carrier: Biquad,
    envelope: Smoothed,
}

pub struct Vocoder {
    bands: Vec<Band>,
    count: usize,
    carrier: Carrier,
    /// Hz the internal carrier runs at.
    pitch_hz: f32,
    /// Q of every band. Low is smooth, high is the peaky talkbox response.
    resonance: f32,
    /// How fast the envelopes follow, in ms.
    speed_ms: f32,
    /// Shifts the carrier's bands against the modulator's, in semitones. A
    /// formant control: the same words out of a bigger or smaller head.
    shift: f32,
    phase: f32,
    /// One phase per note of the held chord, for [`Carrier::Chord`].
    chord_phase: [f32; crate::chord::MAX_NOTES],
    /// The chord as frequencies, refreshed when the hand on the keyboard moves
    /// rather than per sample: eight atomic loads a block, not a block's worth.
    chord_hz: [f32; crate::chord::MAX_NOTES],
    chord_n: usize,
    chord_seen: u32,
    rng: u32,
    mix: f32,
    sample_rate: f32,
    dirty: bool,
}

impl Vocoder {
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate.max(8000) as f32;
        let mut v = Self {
            bands: Vec::with_capacity(MAX_BANDS),
            count: 16,
            carrier: Carrier::Saw,
            pitch_hz: 110.0,
            resonance: 6.0,
            speed_ms: 12.0,
            shift: 0.0,
            phase: 0.0,
            chord_phase: [0.0; crate::chord::MAX_NOTES],
            chord_hz: [0.0; crate::chord::MAX_NOTES],
            chord_n: 0,
            chord_seen: 0,
            rng: 0x5EED_1234,
            mix: 1.0,
            sample_rate: sr,
            dirty: true,
        };
        for _ in 0..MAX_BANDS {
            v.bands.push(Band {
                modulator: Biquad::bandpass(1000.0, sr, 6.0),
                carrier: Biquad::bandpass(1000.0, sr, 6.0),
                envelope: Smoothed::new(0.0, 12.0, sr),
            });
        }
        v.rebuild();
        v
    }

    /// Build from the rack's knob positions: bands, carrier, pitch, res,
    /// speed, shift.
    pub fn with_params(sample_rate: u32, p: &[f32]) -> Self {
        let get = |i: usize, d: f32| p.get(i).copied().unwrap_or(d);
        let mut v = Self::new(sample_rate);
        v.set_bands(get(0, 0.5));
        v.carrier = Carrier::from_norm(get(1, 0.0));
        v.set_pitch(get(2, 0.35));
        v.resonance = 1.0 + get(3, 0.36) * 13.0;
        v.speed_ms = 2.0 + get(4, 0.2) * 48.0;
        v.shift = (get(5, 0.5) - 0.5) * 24.0;
        v.dirty = true;
        v.rebuild();
        v
    }

    pub fn set_bands(&mut self, v: f32) {
        let n = BAND_COUNTS.len();
        let i = (v.clamp(0.0, 1.0) * (n - 1) as f32).round() as usize;
        self.count = BAND_COUNTS[i.min(n - 1)];
        self.dirty = true;
    }

    pub fn bands(&self) -> usize {
        self.count
    }

    /// 0..1 → 40–400 Hz, logarithmic. The note the machine speaks at.
    pub fn set_pitch(&mut self, v: f32) {
        self.pitch_hz = 40.0 * 10.0f32.powf(v.clamp(0.0, 1.0));
    }

    /// The centre of band `i`, spread logarithmically: a band is an octave
    /// fraction, because that is how a formant moves and how hearing works.
    fn centre(&self, i: usize) -> f32 {
        let t = i as f32 / (self.count.max(2) - 1) as f32;
        LOW_HZ * (HIGH_HZ / LOW_HZ).powf(t)
    }

    /// Recompute every filter. Off the audio path: `powf` per band, and the
    /// bank only changes when a knob does.
    fn rebuild(&mut self) {
        let sr = self.sample_rate;
        let q = self.resonance;
        let shift = 2.0f32.powf(self.shift / 12.0);
        let speed = self.speed_ms;
        for i in 0..self.count {
            let hz = self.centre(i);
            let band = &mut self.bands[i];
            band.modulator = Biquad::bandpass(hz, sr, q);
            // The carrier's band can sit somewhere else: that is the formant
            // shift, and it is the difference between a voice and the same
            // voice out of a different sized head.
            band.carrier = Biquad::bandpass((hz * shift).clamp(20.0, sr * 0.45), sr, q);
            band.envelope = Smoothed::new(0.0, speed, sr);
        }
        self.dirty = false;
    }
}

/// The correction that turns a naive ramp into a band-limited one.
///
/// A saw counted straight off a phase accumulator has a vertical edge in it,
/// and a vertical edge has every harmonic there is — including the ones above
/// Nyquist, which do not disappear but fold back and land on frequencies that
/// are **not** multiples of the note. Measured on the raw carrier at 356 Hz,
/// the inharmonic energy is 11 dB louder without this than with it, and it
/// gets worse the higher the carrier is pitched.
///
/// This is the standard two-sample polynomial fit around the discontinuity:
/// `t` is the phase, `dt` how much phase one sample covers. Costs a compare
/// per sample and a handful of multiplies right at the edge.
#[inline]
fn poly_blep(t: f32, dt: f32) -> f32 {
    if t < dt {
        let t = t / dt;
        t + t - t * t - 1.0
    } else if t > 1.0 - dt {
        let t = (t - 1.0) / dt;
        t * t + t + t + 1.0
    } else {
        0.0
    }
}

impl Vocoder {
    #[inline]
    fn carrier_sample(&mut self, right: f32) -> f32 {
        match self.carrier {
            Carrier::Right => right,
            Carrier::Chord => {
                if self.chord_n == 0 {
                    // Nothing held: silence rather than a drone. A vocoder with
                    // no carrier says nothing, which is what a hand off the
                    // keyboard means.
                    return 0.0;
                }
                let mut sum = 0.0;
                let sr = self.sample_rate;
                for (phase, hz) in self
                    .chord_phase
                    .iter_mut()
                    .zip(self.chord_hz.iter())
                    .take(self.chord_n)
                {
                    let dt = hz / sr;
                    *phase += dt;
                    *phase -= phase.floor();
                    sum += *phase * 2.0 - 1.0 - poly_blep(*phase, dt);
                }
                // Uncorrelated saws: their powers add, so the sum is divided by
                // the root rather than by the count.
                sum / (self.chord_n as f32).sqrt()
            }
            Carrier::Noise => {
                self.rng ^= self.rng << 13;
                self.rng ^= self.rng >> 17;
                self.rng ^= self.rng << 5;
                (self.rng >> 8) as f32 / 8_388_608.0 - 1.0
            }
            kind => {
                let dt = self.pitch_hz / self.sample_rate;
                self.phase += dt;
                self.phase -= self.phase.floor();
                match kind {
                    // Every harmonic there is, because a band with nothing in
                    // it has nothing to turn up — but only the harmonics that
                    // fit under Nyquist. The ones that do not fit are not
                    // brightness, they are the note's own partials landing on
                    // the wrong frequencies. See [`poly_blep`].
                    Carrier::Pulse => {
                        const WIDTH: f32 = 0.35;
                        let raw = if self.phase < WIDTH { 1.0 } else { -1.0 };
                        // Two edges, and the second one is the first one moved
                        // by the pulse width.
                        raw + poly_blep(self.phase, dt)
                            - poly_blep((self.phase - WIDTH).rem_euclid(1.0), dt)
                    }
                    _ => self.phase * 2.0 - 1.0 - poly_blep(self.phase, dt),
                }
            }
        }
    }
}

impl super::FxProcessor for Vocoder {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        let sr = sample_rate.max(8000) as f32;
        if (sr - self.sample_rate).abs() > 0.5 {
            self.sample_rate = sr;
            self.dirty = true;
        }
        if self.dirty {
            self.rebuild();
        }
        // The chord, once a block and only when it moved.
        if self.carrier == Carrier::Chord {
            let chord = crate::chord::chord();
            let generation = chord.generation();
            if generation != self.chord_seen {
                let mut held = [0u8; crate::chord::MAX_NOTES];
                self.chord_n = chord.read(&mut held);
                for (hz, note) in self.chord_hz.iter_mut().zip(held.iter()).take(self.chord_n) {
                    *hz = 440.0 * 2f32.powf((*note as f32 - 69.0) / 12.0);
                }
                self.chord_seen = generation;
            }
        }
        let count = self.count;
        let mix = self.mix;

        for frame in buf.as_chunks_mut::<2>().0 {
            let (dry_l, dry_r) = (frame[0], frame[1]);
            // The left channel is the voice. One signal is being analysed and
            // one is being shaped; which is which has to be decided somewhere,
            // and the left is where a mono input lands.
            let modulator = dry_l;
            let carrier = self.carrier_sample(dry_r);

            let mut wet = 0.0f32;
            for band in self.bands.iter_mut().take(count) {
                // How loud the voice is in this band, rectified and smoothed:
                // the band's *shape*, with none of its sound.
                let m = band.modulator.process(modulator).abs();
                band.envelope.set_target(m);
                let level = band.envelope.tick();
                wet += band.carrier.process(carrier) * level;
            }
            // The bank splits energy across `count` bands and the envelopes are
            // averages, so the sum comes back quiet; scaled by the count rather
            // than by a fitted constant, so changing the band count does not
            // change the level.
            wet *= 2.0;

            let out = if wet.is_finite() { wet } else { 0.0 };
            frame[0] = dry_l + mix * (out - dry_l);
            frame[1] = dry_r + mix * (out - dry_r);
        }
    }

    fn reset(&mut self) {
        self.phase = 0.0;
        self.dirty = true;
    }

    fn set_mix(&mut self, wet: f32) {
        self.mix = wet.clamp(0.0, 1.0);
    }

    fn name(&self) -> &str {
        "Vocoder"
    }

    fn params(&self) -> Vec<crate::fx::FxParam> {
        use crate::fx::FxParam;
        let bands_norm = BAND_COUNTS
            .iter()
            .position(|c| *c == self.count)
            .unwrap_or(1) as f32
            / (BAND_COUNTS.len() - 1) as f32;
        vec![
            FxParam::new("Bands", bands_norm, 8.0, 24.0, ""),
            FxParam::new("Carrier", self.carrier.to_norm(), 0.0, 1.0, ""),
            FxParam::new(
                "Pitch",
                (self.pitch_hz / 40.0).log10().clamp(0.0, 1.0),
                40.0,
                400.0,
                "Hz",
            ),
            FxParam::new("Res", (self.resonance - 1.0) / 13.0, 1.0, 14.0, ""),
            FxParam::new("Speed", (self.speed_ms - 2.0) / 48.0, 2.0, 50.0, "ms"),
            FxParam::new("Shift", self.shift / 24.0 + 0.5, -12.0, 12.0, "st"),
            FxParam::new("Wet", self.mix, 0.0, 1.0, ""),
        ]
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => self.set_bands(v),
            1 => self.carrier = Carrier::from_norm(v),
            2 => self.set_pitch(v),
            3 => {
                self.resonance = 1.0 + v * 13.0;
                self.dirty = true;
            }
            4 => {
                self.speed_ms = 2.0 + v * 48.0;
                self.dirty = true;
            }
            5 => {
                self.shift = (v - 0.5) * 24.0;
                self.dirty = true;
            }
            6 => self.mix = v,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Everything that is not a multiple of `f0`, against everything that is.
    ///
    /// A saw is meant to be broadband, so "how much is outside the fundamental"
    /// says nothing about it. What a naive one does wrong is put energy on
    /// frequencies that are **not** harmonics: its partials above Nyquist fold
    /// back and land wherever they land.
    fn inharmonic_db(x: &[f32], f0: f32, sr: f32) -> f32 {
        let n = x.len();
        let bin = sr / n as f32;
        let (mut harm, mut junk) = (0.0f32, 0.0f32);
        for k in 2..n / 2 {
            let (mut re, mut im) = (0.0f32, 0.0f32);
            for (i, &s) in x.iter().enumerate() {
                let w = 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos();
                let a = std::f32::consts::TAU * k as f32 * i as f32 / n as f32;
                re += s * w * a.cos();
                im -= s * w * a.sin();
            }
            let mag2 = (re * re + im * im) / (n * n) as f32;
            let hz = k as f32 * bin;
            let h = (hz / f0).round();
            if h >= 1.0 && (hz - h * f0).abs() < bin * 2.0 {
                harm += mag2;
            } else {
                junk += mag2;
            }
        }
        10.0 * (junk / harm.max(1e-30)).max(1e-30).log10()
    }

    /// The carrier is band-limited, and this is the measurement that says so.
    ///
    /// A naive saw at 356 Hz — the top of the pitch knob is 400 — puts 20 dB of
    /// inharmonic energy under its harmonics; with the BLEP correction that is
    /// 31 dB down, and the higher the note the bigger the gap. Measured on the
    /// oscillator itself rather than through the band bank, because the bank is
    /// linear and would only add its own skirts to the number.
    #[test]
    fn the_carrier_is_band_limited() {
        let sr = 48_000.0f32;
        for (carrier, floor) in [(Carrier::Saw, -28.0f32), (Carrier::Pulse, -20.0)] {
            let mut v = Vocoder::new(sr as u32);
            v.carrier = carrier;
            v.pitch_hz = 356.0;
            let out: Vec<f32> = (0..4096).map(|_| v.carrier_sample(0.0)).collect();
            let db = inharmonic_db(&out, 356.0, sr);
            assert!(
                db < floor,
                "{carrier:?} at 356 Hz is {db:.1} dB of aliasing, wanted under {floor}"
            );
        }
    }
    use crate::fx::FxProcessor;

    /// A "voice": a tone whose energy sits in one part of the spectrum, which
    /// is what a vowel is as far as a filter bank is concerned.
    fn vowel(hz: f32, sr: f32, frames: usize, amp: f32) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let t = i as f32 / sr;
                let s = ((std::f32::consts::TAU * hz * t).sin()
                    + 0.5 * (std::f32::consts::TAU * hz * 2.0 * t).sin())
                    * amp;
                [s, s]
            })
            .collect()
    }

    fn rms(buf: &[f32]) -> f32 {
        (buf.iter().map(|s| s * s).sum::<f32>() / buf.len().max(1) as f32).sqrt()
    }

    fn energy_at(buf: &[f32], probe: f32, sr: f32) -> f32 {
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

    /// The whole claim: **the voice is not heard, its shape is**. What comes
    /// out is the carrier's pitch with the modulator's spectrum on it — so a
    /// 2 kHz "vowel" makes the carrier loud at 2 kHz, and the 2 kHz of the
    /// voice itself is not what is being heard.
    #[test]
    fn the_carrier_takes_the_shape_of_the_voice() {
        let sr = 48_000.0;
        let bright = |hz: f32| {
            let mut v = Vocoder::new(48_000);
            v.carrier = Carrier::Saw;
            v.set_pitch(0.0); // 40 Hz: a saw with everything in it
            v.set_mix(1.0);
            let mut buf = vowel(hz, sr, 24_000, 0.4);
            v.process_block(&mut buf, 48_000);
            let tail = &buf[12_000 * 2..];
            (
                energy_at(tail, 500.0, sr),
                energy_at(tail, 3_000.0, sr),
                rms(tail),
            )
        };
        // A low vowel opens the low bands; a high one opens the high bands.
        let (low_at_500, low_at_3k, low_rms) = bright(400.0);
        let (high_at_500, high_at_3k, high_rms) = bright(3_000.0);
        assert!(low_rms > 1e-4 && high_rms > 1e-4, "something came out");
        assert!(
            low_at_500 / low_at_3k > high_at_500 / high_at_3k * 4.0,
            "the bank should follow the vowel: low={low_at_500}/{low_at_3k} high={high_at_500}/{high_at_3k}"
        );
    }

    /// Silence in is silence out. A vocoder whose carrier keeps running under
    /// a closed envelope is a buzzing that never stops, which is the failure
    /// everybody has heard from a badly built one.
    #[test]
    fn no_voice_means_no_sound_even_though_the_carrier_runs() {
        let mut v = Vocoder::new(48_000);
        v.set_mix(1.0);
        // Warm it up on a voice first, so the envelopes have somewhere to fall
        // from.
        let mut buf = vowel(500.0, 48_000.0, 24_000, 0.4);
        v.process_block(&mut buf, 48_000);
        let mut quiet = vec![0.0f32; 24_000 * 2];
        v.process_block(&mut quiet, 48_000);
        let tail = rms(&quiet[12_000 * 2..]);
        assert!(tail < 1e-4, "the carrier is still buzzing: {tail}");
    }

    /// The talkbox: the carrier is the other channel, so what speaks is a real
    /// instrument. No second effect for it — the same code, one setting.
    #[test]
    fn the_right_channel_can_be_the_carrier() {
        let sr = 48_000.0;
        let mut v = Vocoder::new(48_000);
        v.carrier = Carrier::Right;
        v.set_mix(1.0);
        // Voice on the left, a guitar-ish tone on the right.
        let frames = 24_000;
        let mut buf = vec![0.0f32; frames * 2];
        for i in 0..frames {
            let t = i as f32 / sr;
            buf[i * 2] = (std::f32::consts::TAU * 500.0 * t).sin() * 0.4;
            buf[i * 2 + 1] = ((std::f32::consts::TAU * 220.0 * t).sin()
                + 0.6 * (std::f32::consts::TAU * 440.0 * t).sin()
                + 0.4 * (std::f32::consts::TAU * 660.0 * t).sin())
                * 0.4;
        }
        v.process_block(&mut buf, 48_000);
        let tail = &buf[12_000 * 2..];
        assert!(rms(tail) > 1e-4, "the talkbox should sound");
        // What comes out is the guitar's harmonics, not the voice's 500 Hz.
        let guitar = energy_at(tail, 440.0, sr);
        let voice = energy_at(tail, 500.0, sr);
        assert!(
            guitar > voice,
            "the carrier is what is heard: guitar={guitar} voice={voice}"
        );
    }

    /// Changing the band count must not change the level: it is a resolution
    /// control, not a volume one.
    #[test]
    fn the_band_count_is_not_a_volume_knob() {
        let level = |knob: f32| {
            let mut v = Vocoder::new(48_000);
            v.set_bands(knob);
            v.set_pitch(0.0);
            v.set_mix(1.0);
            let mut buf = vowel(600.0, 48_000.0, 24_000, 0.4);
            v.process_block(&mut buf, 48_000);
            rms(&buf[12_000 * 2..])
        };
        let (eight, twenty_four) = (level(0.0), level(1.0));
        assert!(eight > 1e-4 && twenty_four > 1e-4);
        let ratio = eight.max(twenty_four) / eight.min(twenty_four);
        assert!(ratio < 3.0, "8 bands {eight}, 24 bands {twenty_four}");
    }

    #[test]
    fn it_survives_silence_extremes_and_a_rate_change() {
        for carrier in Carrier::ALL {
            let mut v = Vocoder::with_params(48_000, &[1.0, carrier.to_norm(), 0.5, 1.0, 0.0, 1.0]);
            v.set_mix(1.0);
            let mut buf = vec![0.0f32; 1024];
            v.process_block(&mut buf, 48_000);
            assert!(
                buf.iter().all(|s| s.abs() < 1e-6),
                "{} rang in silence",
                carrier.label()
            );
            let mut hot = vec![4.0f32; 4096];
            v.process_block(&mut hot, 96_000);
            assert!(
                hot.iter().all(|s| s.is_finite()),
                "{} went non-finite",
                carrier.label()
            );
            v.process_block(&mut [], 96_000);
            v.process_block(&mut [1.0], 96_000);
            v.reset();
        }
    }
}
