//! State-Variable Filter (SVF) — simultaneous LP/HP/BP/notch outputs.
//!
//! Based on the topology-preserving SVF from Simper (2012).
//! Stable at all frequencies, minimal distortion, suitable for realtime.

use super::smooth::Smoothed;
use super::FxProcessor;
use std::f32::consts::PI;

/// Time constant of the cutoff sweep. Long enough to have no corner, short
/// enough that a filter still feels like it answers the knob.
const CUTOFF_MS: f32 = 15.0;

/// How often the coefficients are rebuilt while the cutoff is moving.
const COEFF_EVERY: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SvfMode {
    Lowpass,
    Highpass,
    Bandpass,
    Notch,
}

/// Stereo State Variable Filter.
pub struct Svf {
    mode: SvfMode,
    /// Where the knob is. The coefficients follow `cutoff` towards it.
    cutoff_hz: f32,
    /// The cutoff actually in the coefficients, smoothed in octaves so a sweep
    /// sounds even. Recalculating `g = tan(πf/sr)` the instant the knob moves
    /// is a step in the coefficient, and a step is a click.
    cutoff: Smoothed,
    resonance: f32, // 0.0 (max resonance) – 1.0 (no resonance, butterworth)
    wet: f32,
    // Per-channel state (L, R)
    ic1eq: [f32; 2],
    ic2eq: [f32; 2],
    // Precomputed coefficients
    g: f32,
    k: f32,
    a1: f32,
    a2: f32,
    a3: f32,
    sample_rate: u32,
}

impl Svf {
    pub fn new(mode: SvfMode, cutoff_hz: f32, resonance: f32) -> Self {
        // Clamp here so no construction path can hand the filter a resonance
        // ≥1 (k ≤ 0 = negative damping = self-oscillation / runaway).
        let mut s = Self {
            mode,
            cutoff_hz: cutoff_hz.clamp(10.0, 20000.0),
            cutoff: Smoothed::new(cutoff_hz.clamp(10.0, 20000.0).log2(), CUTOFF_MS, 48000.0),
            resonance: resonance.clamp(0.0, 1.0),
            wet: 1.0,
            ic1eq: [0.0; 2],
            ic2eq: [0.0; 2],
            g: 0.0,
            k: 0.0,
            a1: 0.0,
            a2: 0.0,
            a3: 0.0,
            sample_rate: 48000,
        };
        s.update_coeffs();
        s
    }

    pub fn set_cutoff(&mut self, hz: f32) {
        self.cutoff_hz = hz.clamp(10.0, 20000.0);
        self.cutoff.set_target(self.cutoff_hz.log2());
    }

    pub fn set_resonance(&mut self, r: f32) {
        self.resonance = r.clamp(0.0, 1.0);
        self.update_coeffs();
    }

    pub fn set_mode(&mut self, mode: SvfMode) {
        self.mode = mode;
    }

    fn update_coeffs(&mut self) {
        let sr = self.sample_rate as f32;
        self.g = (PI * self.cutoff.value().exp2().clamp(10.0, sr * 0.45) / sr).tan();
        // k = 2*(1 - resonance): at resonance=0 → k=2 (max damp), at resonance=1 → k=0 (self-osc)
        self.k = 2.0 - 2.0 * self.resonance;
        let g = self.g;
        let k = self.k;
        self.a1 = 1.0 / (1.0 + g * (g + k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
    }

    #[inline]
    fn process_sample(&mut self, ch: usize, x: f32) -> f32 {
        let v3 = x - self.ic2eq[ch];
        let v1 = self.a1 * self.ic1eq[ch] + self.a2 * v3;
        let v2 = self.ic2eq[ch] + self.a2 * self.ic1eq[ch] + self.a3 * v3;
        self.ic1eq[ch] = 2.0 * v1 - self.ic1eq[ch];
        self.ic2eq[ch] = 2.0 * v2 - self.ic2eq[ch];
        match self.mode {
            SvfMode::Lowpass => v2,
            SvfMode::Highpass => x - self.k * v1 - v2,
            SvfMode::Bandpass => v1,
            SvfMode::Notch => x - self.k * v1,
        }
    }
}

impl FxProcessor for Svf {
    fn params(&self) -> Vec<crate::fx::FxParam> {
        use crate::fx::FxParam;
        vec![
            FxParam::new(
                "Cutoff",
                ((self.cutoff_hz - 20.0) / 19980.0).clamp(0.0, 1.0),
                20.0,
                20000.0,
                "Hz",
            ),
            FxParam::new("Res", self.resonance / 0.98, 0.0, 0.98, ""),
            FxParam::new("Wet", self.wet, 0.0, 1.0, ""),
        ]
    }

    /// Cutoff and resonance, live: rebuilding would clear the filter's state
    /// and a state-variable filter with no state is a click.
    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => self.set_cutoff(20.0 + v * 19980.0),
            1 => self.set_resonance(v * 0.98),
            2 => self.wet = v,
            _ => {}
        }
    }

    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        if self.sample_rate != sample_rate {
            self.sample_rate = sample_rate;
            self.cutoff.set_sample_rate(sample_rate as f32);
            self.update_coeffs();
        }
        let frames = buf.len() / 2;
        for i in 0..frames {
            // The pole moves at control rate: a `tan()` per sample is what F4
            // is about, and 16 samples is a third of a millisecond.
            let moving = self.cutoff.tick() != self.cutoff.target();
            if i % COEFF_EVERY == 0 && (moving || i == 0) {
                self.update_coeffs();
            }
            let dry_l = buf[i * 2];
            let dry_r = buf[i * 2 + 1];
            let wet_l = self.process_sample(0, dry_l);
            let wet_r = self.process_sample(1, dry_r);
            buf[i * 2] = dry_l + self.wet * (wet_l - dry_l);
            buf[i * 2 + 1] = dry_r + self.wet * (wet_r - dry_r);
        }
    }

    fn reset(&mut self) {
        self.ic1eq = [0.0; 2];
        self.ic2eq = [0.0; 2];
        self.cutoff.snap(self.cutoff_hz.log2());
        self.update_coeffs();
    }

    fn set_mix(&mut self, wet: f32) {
        self.wet = wet.clamp(0.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowpass_attenuates_high_freq() {
        let mut svf = Svf::new(SvfMode::Lowpass, 200.0, 0.5);
        // Simulate a 10 kHz signal at 48kHz
        let sr = 48000u32;
        let freq = 10000.0f32;
        let mut buf: Vec<f32> = (0..256)
            .flat_map(|i| {
                let s = (2.0 * std::f32::consts::PI * freq * i as f32 / sr as f32).sin();
                [s, s]
            })
            .collect();
        svf.process_block(&mut buf, sr);
        // After the filter the amplitude should be significantly reduced
        let peak = buf.iter().copied().map(f32::abs).fold(0.0f32, f32::max);
        assert!(peak < 0.1, "highfreq should be attenuated, peak={peak}");
    }

    /// Slamming the cutoff end to end must not step the coefficient. Measured
    /// against the same signal with the cutoff held still, because a resonant
    /// filter always moves the waveform some.
    #[test]
    fn sweeping_the_cutoff_does_not_click() {
        let sr = 48_000u32;
        let worst = |automate: bool| {
            let mut svf = Svf::new(SvfMode::Lowpass, 800.0, 0.7);
            let mut worst = 0.0f32;
            let mut prev = 0.0f32;
            let mut phase = 0.0f32;
            for block in 0..40 {
                if automate {
                    svf.set_param(0, (block % 2) as f32);
                }
                let mut buf: Vec<f32> = (0..256)
                    .flat_map(|_| {
                        phase = (phase + 220.0 / sr as f32).fract();
                        let s = (std::f32::consts::TAU * phase).sin() * 0.8;
                        [s, s]
                    })
                    .collect();
                svf.process_block(&mut buf, sr);
                for s in buf.chunks(2).map(|c| c[0]) {
                    worst = worst.max((s - prev).abs());
                    prev = s;
                }
            }
            worst
        };
        let still = worst(false);
        let swept = worst(true);
        assert!(
            swept < still * 3.0,
            "the cutoff stepped: {swept:.3} while automated, {still:.3} while still"
        );
    }

    #[test]
    fn reset_clears_state() {
        let mut svf = Svf::new(SvfMode::Lowpass, 1000.0, 0.5);
        let mut buf = [1.0f32; 64];
        svf.process_block(&mut buf, 48000);
        svf.reset();
        assert_eq!(svf.ic1eq, [0.0; 2]);
        assert_eq!(svf.ic2eq, [0.0; 2]);
    }
}
