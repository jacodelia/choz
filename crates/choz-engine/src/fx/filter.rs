//! State-Variable Filter (SVF) — simultaneous LP/HP/BP/notch outputs.
//!
//! Based on the topology-preserving SVF from Simper (2012).
//! Stable at all frequencies, minimal distortion, suitable for realtime.

use super::FxProcessor;
use std::f32::consts::PI;

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
    cutoff_hz: f32,
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
        self.update_coeffs();
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
        self.g = (PI * self.cutoff_hz / sr).tan();
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
    /// Cutoff and resonance, live: rebuilding would clear the filter's state
    /// and a state-variable filter with no state is a click.
    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => self.set_cutoff(20.0 + v * 19980.0),
            1 => self.set_resonance(v * 0.98),
            _ => {}
        }
    }

    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        if self.sample_rate != sample_rate {
            self.sample_rate = sample_rate;
            self.update_coeffs();
        }
        let frames = buf.len() / 2;
        for i in 0..frames {
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
