//! Vibrato: the pitch moved by an LFO, and nothing else.
//!
//! A chorus without the dry half. The delay line is read by a head whose
//! distance from the writer is swung by the oscillator, and a moving read head
//! *is* a pitch change — the same mechanism the chorus and the flanger use, put
//! on its own so the effect has one job: the finger vibrato a keyboard has no
//! way to play.
//!
//! The two channels read the same oscillator at a phase offset (`Spread`), so a
//! wide vibrato drifts across the image instead of moving both sides together.

use super::delay_line::DelayLine as Line;
use super::lfo::{Lfo, Wave};
use choz_ports::FxParam;

/// Centre delay. Deep enough that the deepest swing never reaches the writer,
/// short enough that the effect is heard as pitch rather than as a slap.
const CENTRE_MS: f32 = 12.0;

/// How far the head swings at `Depth` 1, in milliseconds either way.
const MAX_DEPTH_MS: f32 = 8.0;

pub struct Vibrato {
    line: [Line; 2],
    lfo: Lfo,
    wave: Wave,
    rate_hz: f32,
    depth: f32,
    spread: f32,
    wet: f32,
    sample_rate: f32,
}

impl Vibrato {
    pub fn new(sample_rate: u32) -> Self {
        let ms = CENTRE_MS + MAX_DEPTH_MS + 2.0;
        Self {
            line: [Line::with_ms(ms), Line::with_ms(ms)],
            lfo: Lfo::new(),
            wave: Wave::Sine,
            rate_hz: 5.0,
            depth: 0.35,
            spread: 0.0,
            wet: 1.0,
            sample_rate: sample_rate.max(8000) as f32,
        }
    }

    pub fn with_params(sample_rate: u32, params: &[f32]) -> Self {
        let mut v = Self::new(sample_rate);
        for (i, p) in params.iter().enumerate() {
            <Self as choz_ports::FxProcessor>::set_param(&mut v, i, *p);
        }
        v
    }
}

impl choz_ports::FxProcessor for Vibrato {
    fn name(&self) -> &str {
        "Vibrato"
    }

    fn params(&self) -> Vec<FxParam> {
        vec![
            FxParam::new("Rate", (self.rate_hz - 0.1) / 11.9, 0.1, 12.0, "Hz"),
            FxParam::new("Depth", self.depth, 0.0, 1.0, ""),
            FxParam::new("Shape", self.wave.to_norm(), 0.0, 1.0, ""),
            FxParam::new("Spread", self.spread, 0.0, 1.0, ""),
            FxParam::new("Wet", self.wet, 0.0, 1.0, ""),
        ]
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => self.rate_hz = 0.1 + v * 11.9,
            1 => self.depth = v,
            2 => self.wave = Wave::from_norm(v),
            3 => self.spread = v,
            4 => self.wet = v,
            _ => {}
        }
    }

    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        self.sample_rate = sample_rate.max(8000) as f32;
        let sr = self.sample_rate;
        let centre = CENTRE_MS * 0.001 * sr;
        let swing = self.depth * MAX_DEPTH_MS * 0.001 * sr;
        for frame in buf.chunks_exact_mut(2) {
            let lfo = self.lfo.tick(self.wave, self.rate_hz, sr, self.spread);
            for (ch, s) in frame.iter_mut().enumerate() {
                let dry = *s;
                self.line[ch].write(dry);
                // `read_cubic`, not `read`: a head that lands between samples
                // every sample is exactly where linear interpolation is heard
                // as a buzz on a held note.
                let wet = self.line[ch].read_cubic(centre + lfo[ch] * swing);
                *s = dry + self.wet * (wet - dry);
            }
        }
    }

    fn reset(&mut self) {
        for l in &mut self.line {
            l.clear();
        }
        self.lfo.reset();
    }

    fn set_mix(&mut self, wet: f32) {
        self.wet = wet.clamp(0.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use choz_ports::FxProcessor;

    /// The pitch has to actually move: a held sine comes out with its zero
    /// crossings no longer evenly spaced. Measured as the spread of the
    /// crossing intervals, which is zero for a steady tone.
    #[test]
    fn the_pitch_wobbles_and_the_depth_is_what_says_how_far() {
        let sr = 48_000u32;
        let jitter = |depth: f32| {
            let mut fx = Vibrato::new(sr);
            fx.set_param(0, 0.5);
            fx.set_param(1, depth);
            let mut buf: Vec<f32> = (0..sr as usize)
                .flat_map(|i| {
                    let v = (std::f32::consts::TAU * 220.0 * i as f32 / sr as f32).sin() * 0.5;
                    [v, v]
                })
                .collect();
            fx.process_block(&mut buf, sr);
            let tail: Vec<f32> = buf[buf.len() / 2..].chunks_exact(2).map(|f| f[0]).collect();
            let mut gaps = Vec::new();
            let mut last = None;
            for (i, w) in tail.windows(2).enumerate() {
                if w[0] <= 0.0 && w[1] > 0.0 {
                    if let Some(p) = last {
                        gaps.push((i - p) as f32);
                    }
                    last = Some(i);
                }
            }
            let mean = gaps.iter().sum::<f32>() / gaps.len() as f32;
            gaps.iter().map(|g| (g - mean).abs()).sum::<f32>() / gaps.len() as f32
        };
        let none = jitter(0.0);
        let deep = jitter(1.0);
        // Not zero: a 220 Hz cycle is 218.18 samples, so the crossings
        // alternate between 218 and 219 whatever the head is doing. What a
        // still head means is that they do not do anything *else*.
        assert!(none < 1.0, "a still head is a steady pitch, got {none}");
        assert!(
            deep > none * 4.0,
            "the depth moved it: {deep} against {none}"
        );
    }
}
