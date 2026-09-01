//! Moog-style ladder filter: four one-pole sections and a feedback path.
//!
//! The state-variable filter choz already has is clean, cheap and correct — and
//! that is exactly what it sounds like. A ladder is the other thing: 24 dB an
//! octave instead of 12, a resonance that thins the body as it climbs, and a
//! saturating feedback path that keeps self-oscillation musical instead of
//! letting it blow up.
//!
//! The topology is the zero-delay-feedback form (Zavalishin): each stage is
//! solved for its own output rather than reading the previous sample, which is
//! what keeps the cutoff honest up near Nyquist where the naive cascade drifts
//! flat.

use super::smooth::Smoothed;
use choz_ports::FxParam;
use std::f32::consts::PI;

/// How long the cutoff takes to walk to a new knob position, in ms. Same
/// reasoning as the SVF's: a jump in the coefficient is a click.
const CUTOFF_MS: f32 = 20.0;

#[derive(Clone, Copy, Default)]
struct Ladder1p {
    /// The stage's integrator state.
    z: f32,
}

impl Ladder1p {
    #[inline]
    fn tick(&mut self, x: f32, g: f32) -> f32 {
        // One-pole TPT: v = (x - z) * g / (1 + g), y = v + z, z = y + v.
        let v = (x - self.z) * g / (1.0 + g);
        let y = v + self.z;
        self.z = y + v;
        y
    }
}

pub struct MoogLadder {
    cutoff_hz: f32,
    cutoff: Smoothed,
    /// 0..1, where 1 is on the edge of self-oscillation.
    resonance: f32,
    /// How hard the feedback path is driven before it clips: the difference
    /// between a filter and an instrument.
    drive: f32,
    wet: f32,
    stages: [[Ladder1p; 4]; 2],
    /// Last output per channel, which is what the feedback path reads.
    fb: [f32; 2],
    sample_rate: f32,
}

impl MoogLadder {
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate.max(8000) as f32;
        Self {
            cutoff_hz: 1000.0,
            cutoff: Smoothed::new(1000.0f32.log2(), CUTOFF_MS, sr),
            resonance: 0.3,
            drive: 1.0,
            wet: 1.0,
            stages: [[Ladder1p::default(); 4]; 2],
            fb: [0.0; 2],
            sample_rate: sr,
        }
    }

    pub fn with_params(sample_rate: u32, params: &[f32]) -> Self {
        let mut f = Self::new(sample_rate);
        for (i, p) in params.iter().enumerate() {
            <Self as choz_ports::FxProcessor>::set_param(&mut f, i, *p);
        }
        f
    }

    /// The integrator coefficient for the cutoff the smoother is on now.
    #[inline]
    fn g(&self) -> f32 {
        let hz = self.cutoff.value().exp2().clamp(20.0, self.sample_rate * 0.45);
        (PI * hz / self.sample_rate).tan()
    }
}

impl choz_ports::FxProcessor for MoogLadder {
    fn name(&self) -> &str {
        "Moog Ladder"
    }

    fn params(&self) -> Vec<FxParam> {
        vec![
            FxParam::new(
                "Cutoff",
                // Logarithmic, like every cutoff a hand expects: half way up
                // the knob is 630 Hz, not 10 kHz.
                ((self.cutoff_hz / 20.0).log2() / 10.0).clamp(0.0, 1.0),
                20.0,
                20000.0,
                "Hz",
            ),
            FxParam::new("Res", self.resonance, 0.0, 1.0, ""),
            FxParam::new("Drive", (self.drive - 1.0) / 7.0, 1.0, 8.0, "x"),
            FxParam::new("Wet", self.wet, 0.0, 1.0, ""),
        ]
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => {
                self.cutoff_hz = (20.0 * (v * 10.0).exp2()).clamp(20.0, 20000.0);
                self.cutoff.set_target(self.cutoff_hz.log2());
            }
            // Stops just short of 4: at exactly the oscillation point the
            // filter is an oscillator, and a knob that silently becomes a tone
            // generator at its last degree is a knob nobody can use.
            1 => self.resonance = v,
            2 => self.drive = 1.0 + v * 7.0,
            3 => self.wet = v,
            _ => {}
        }
    }

    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        let sr = sample_rate.max(8000) as f32;
        if (sr - self.sample_rate).abs() > f32::EPSILON {
            self.sample_rate = sr;
            self.cutoff.set_sample_rate(sr);
        }
        // Four stages of feedback want a gain of 4 to oscillate; 3.9 leaves the
        // filter on the near side of it at the top of the knob.
        let k = self.resonance * 3.9;
        for frame in buf.as_chunks_mut::<2>().0 {
            self.cutoff.tick();
            let g = self.g();
            for (ch, s) in frame.iter_mut().enumerate() {
                let dry = *s;
                // The feedback path saturates: that is what keeps a resonating
                // ladder a sound rather than an overflow.
                let x = (dry * self.drive - k * self.fb[ch]).tanh();
                let mut y = x;
                for stage in &mut self.stages[ch] {
                    y = stage.tick(y, g);
                }
                self.fb[ch] = y;
                // The drive is taken back out: it is a colour, not a fader.
                let wet = y / self.drive;
                *s = dry + self.wet * (wet - dry);
            }
        }
    }

    fn reset(&mut self) {
        self.stages = [[Ladder1p::default(); 4]; 2];
        self.fb = [0.0; 2];
    }

    fn set_mix(&mut self, wet: f32) {
        self.wet = wet.clamp(0.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use choz_ports::FxProcessor;

    fn rms_of(fx: &mut MoogLadder, hz: f32, sr: u32) -> f32 {
        let n = sr as usize / 2;
        let mut buf: Vec<f32> = (0..n)
            .flat_map(|i| {
                let v = (std::f32::consts::TAU * hz * i as f32 / sr as f32).sin() * 0.5;
                [v, v]
            })
            .collect();
        fx.process_block(&mut buf, sr);
        let tail = &buf[buf.len() / 2..];
        (tail.iter().map(|s| s * s).sum::<f32>() / tail.len() as f32).sqrt()
    }

    /// Four poles, not two: an octave above the cutoff has to lose far more
    /// than the state-variable filter's 12 dB — and the band below has to come
    /// through.
    #[test]
    fn the_ladder_rolls_off_at_four_poles() {
        let sr = 48_000;
        let at = |hz: f32| {
            let mut fx = MoogLadder::new(sr);
            // Cutoff at 500 Hz on the log knob.
            fx.set_param(0, (500.0f32 / 20.0).log2() / 10.0);
            fx.set_param(1, 0.0);
            rms_of(&mut fx, hz, sr)
        };
        let pass = at(100.0);
        let one_oct = at(1000.0);
        let three_oct = at(4000.0);
        let db = |a: f32, b: f32| 20.0 * (a / b).log10();
        assert!(db(pass, 0.35) > -3.0, "the passband is through: {pass}");
        let slope = db(one_oct, three_oct);
        assert!(
            slope > 36.0,
            "two octaves of a 24 dB/oct slope is ~48 dB, got {slope}"
        );
    }

    /// Resonance lifts the band around the cutoff — the thing the SVF's `Res`
    /// does gently and this one does like a synth.
    #[test]
    fn resonance_lifts_the_corner() {
        let sr = 48_000;
        let at_res = |r: f32| {
            let mut fx = MoogLadder::new(sr);
            fx.set_param(0, (500.0f32 / 20.0).log2() / 10.0);
            fx.set_param(1, r);
            rms_of(&mut fx, 500.0, sr)
        };
        let flat = at_res(0.0);
        let peaky = at_res(0.9);
        assert!(
            peaky > flat * 1.5,
            "resonance did not lift the corner: {peaky} against {flat}"
        );
    }
}
