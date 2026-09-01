//! Pitch shifter: the same sound, transposed, with the tempo left alone.
//!
//! The transposition itself is [`super::shift::VoiceShifter`], which the
//! shimmer and the harmoniser already use — one shifter per channel, the knob
//! on the front of it. What this adds over those two is that the shift is the
//! whole effect rather than a stage inside one: a guitar an octave down, a
//! voice a fifth up, a −12 doubling under a lead.
//!
//! Semitones and cents are separate knobs because they are separate decisions:
//! the interval is musical and lands on whole numbers, the detune is a fatness
//! control that never wants to be one. A single knob covering ±24 semitones at
//! cent resolution cannot be put on an exact fifth with a mouse.

use super::shift::VoiceShifter;
use choz_ports::FxParam;

/// The range the semitone knob covers, each way. Two octaves: past that the
/// window's own warble is more of the sound than the note is.
pub const RANGE_SEMIS: f32 = 24.0;

pub struct PitchShifter {
    shifter: [VoiceShifter; 2],
    semitones: f32,
    /// Detune in cents, ±100 — a semitone either way, which is as far as a
    /// detune goes before it is an interval.
    cents: f32,
    wet: f32,
    sample_rate: f32,
}

impl PitchShifter {
    pub fn new(sample_rate: u32) -> Self {
        let mut s = Self {
            shifter: [VoiceShifter::new(), VoiceShifter::new()],
            semitones: 0.0,
            cents: 0.0,
            wet: 1.0,
            sample_rate: sample_rate.max(8000) as f32,
        };
        s.refresh();
        s
    }

    /// Build from the rack's normalised parameter array, in `params()` order.
    pub fn with_params(sample_rate: u32, params: &[f32]) -> Self {
        let mut s = Self::new(sample_rate);
        for (i, v) in params.iter().enumerate() {
            <Self as choz_ports::FxProcessor>::set_param(&mut s, i, *v);
        }
        s
    }

    /// Push the sample rate and the total shift into both shifters.
    fn refresh(&mut self) {
        let semis = self.semitones + self.cents / 100.0;
        for s in &mut self.shifter {
            s.set_sample_rate(self.sample_rate);
            s.set_semitones(semis);
        }
    }
}

impl choz_ports::FxProcessor for PitchShifter {
    fn name(&self) -> &str {
        "Pitch Shifter"
    }

    fn params(&self) -> Vec<FxParam> {
        vec![
            FxParam::new(
                "Semi",
                (self.semitones + RANGE_SEMIS) / (RANGE_SEMIS * 2.0),
                -RANGE_SEMIS,
                RANGE_SEMIS,
                "st",
            ),
            FxParam::new("Fine", (self.cents + 100.0) / 200.0, -100.0, 100.0, "ct"),
            FxParam::new("Wet", self.wet, 0.0, 1.0, ""),
        ]
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            // Whole semitones: an interval is a note, and a fifth that is
            // 6.98 semitones is a fifth nobody can play in tune with.
            0 => self.semitones = ((v * 2.0 - 1.0) * RANGE_SEMIS).round(),
            1 => self.cents = (v * 2.0 - 1.0) * 100.0,
            2 => {
                self.wet = v;
                return;
            }
            _ => return,
        }
        self.refresh();
    }

    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        let sr = sample_rate.max(8000) as f32;
        if (sr - self.sample_rate).abs() > f32::EPSILON {
            self.sample_rate = sr;
            self.refresh();
        }
        for frame in buf.chunks_exact_mut(2) {
            for (ch, s) in frame.iter_mut().enumerate() {
                let dry = *s;
                let shifted = self.shifter[ch].process(dry);
                *s = dry + self.wet * (shifted - dry);
            }
        }
    }

    fn reset(&mut self) {
        for s in &mut self.shifter {
            s.reset();
        }
    }

    fn set_mix(&mut self, wet: f32) {
        self.wet = wet.clamp(0.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use choz_ports::FxProcessor;

    /// A shifted sine comes out at the interval that was asked for. Measured by
    /// counting zero crossings, which needs no FFT and no tolerance games.
    #[test]
    fn an_octave_up_comes_out_an_octave_up() {
        let sr = 48_000u32;
        let hz_of = |semis: f32| {
            let mut fx = PitchShifter::new(sr);
            fx.set_param(0, (semis / RANGE_SEMIS + 1.0) / 2.0);
            let n = sr as usize; // one second
            let mut buf: Vec<f32> = (0..n)
                .flat_map(|i| {
                    let v =
                        (std::f32::consts::TAU * 220.0 * i as f32 / sr as f32).sin() * 0.5;
                    [v, v]
                })
                .collect();
            fx.process_block(&mut buf, sr);
            // The second half only: the first is the line filling up.
            let tail = &buf[buf.len() / 2..];
            let crossings = tail
                .chunks_exact(2)
                .map(|f| f[0])
                .collect::<Vec<_>>()
                .windows(2)
                .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
                .count();
            crossings as f32 / 0.5 // cycles per second over half a second
        };

        let unity = hz_of(0.0);
        assert!(
            (unity - 220.0).abs() < 8.0,
            "no shift is the note that went in, got {unity} Hz"
        );
        let up = hz_of(12.0);
        assert!(
            (up - 440.0).abs() < 16.0,
            "twelve semitones up is an octave, got {up} Hz"
        );
        let down = hz_of(-12.0);
        assert!(
            (down - 110.0).abs() < 8.0,
            "and twelve down is the other way, got {down} Hz"
        );
    }

    /// Dry at zero wet, whatever the shift says — the mix law every effect
    /// here obeys.
    #[test]
    fn a_dry_shifter_is_a_wire() {
        let mut fx = PitchShifter::new(48_000);
        fx.set_param(0, 1.0);
        fx.set_mix(0.0);
        let mut buf: Vec<f32> = (0..512).map(|i| (i as f32 * 0.01).sin()).collect();
        let before = buf.clone();
        fx.process_block(&mut buf, 48_000);
        assert_eq!(buf, before);
    }
}
