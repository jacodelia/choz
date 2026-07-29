//! 48-band graphic EQ filter bank.
//!
//! 48 peaking-EQ biquad filters, logarithmically spaced 20 Hz – 20 kHz.
//! Cascade topology; per-band gain ±24 dB; stereo.
//! REALTIME SAFE: coefficients updated on-demand, no alloc in process_block.

use crate::fx::FxProcessor;

pub const BANDS: usize = 48;
const Q_BANK: f32 = 4.0; // ~1/5-octave bandwidth per band

/// 48-band graphic EQ. Each band is a peaking (bell) biquad EQ.
pub struct FilterBankFx {
    /// Per-band gain in dB (−24 to +24). Default 0.0 = flat.
    gains_db: [f32; BANDS],
    /// Transposed-DF-II state per band: [z1L, z2L, z1R, z2R].
    state: [[f32; 4]; BANDS],
    /// Normalized biquad coefficients per band: [b0, b1, b2, a1, a2].
    coeffs: [[f32; 5]; BANDS],
    sample_rate: u32,
    mix: f32,
}

impl FilterBankFx {
    pub fn new(sample_rate: u32) -> Self {
        let mut fx = Self {
            gains_db: [0.0; BANDS],
            state:    [[0.0; 4]; BANDS],
            coeffs:   [[0.0; 5]; BANDS],
            sample_rate,
            mix: 1.0,
        };
        fx.recompute_all();
        fx
    }

    /// Set per-band gain in dB. `band` is 0-based (0 = 20 Hz, 47 = 20 kHz).
    pub fn set_band_gain(&mut self, band: usize, gain_db: f32) {
        if band < BANDS {
            self.gains_db[band] = gain_db.clamp(-24.0, 24.0);
            let sr = self.sample_rate;
            self.coeffs[band] = peaking_coeffs(center_freq(band), self.gains_db[band], Q_BANK, sr);
        }
    }

    /// Set all 48 band gains at once (slice may be shorter than 48).
    pub fn set_all_gains(&mut self, gains_db: &[f32]) {
        let sr = self.sample_rate;
        for (b, &g) in gains_db.iter().take(BANDS).enumerate() {
            self.gains_db[b] = g.clamp(-24.0, 24.0);
            self.coeffs[b] = peaking_coeffs(center_freq(b), self.gains_db[b], Q_BANK, sr);
        }
    }

    /// Return center frequency for a band index.
    pub fn band_freq(band: usize) -> f32 { center_freq(band) }

    fn recompute_all(&mut self) {
        let sr = self.sample_rate;
        for b in 0..BANDS {
            self.coeffs[b] = peaking_coeffs(center_freq(b), self.gains_db[b], Q_BANK, sr);
        }
    }
}

/// Log-spaced center frequency for band `b` over 20 Hz – 20 kHz.
#[inline]
fn center_freq(b: usize) -> f32 {
    20.0_f32 * 1000.0_f32.powf(b as f32 / (BANDS - 1) as f32)
}

/// Normalized biquad peaking-EQ coefficients (transposed DF-II form).
/// Returns [b0, b1, b2, a1, a2] (a0-divided).
fn peaking_coeffs(fc: f32, gain_db: f32, q: f32, sr: u32) -> [f32; 5] {
    use std::f32::consts::PI;
    let a = 10.0_f32.powf(gain_db / 40.0); // sqrt(linear gain)
    let w0 = 2.0 * PI * fc / sr as f32;
    let cos_w = w0.cos();
    let alpha = w0.sin() / (2.0 * q);

    let b0 = 1.0 + alpha * a;
    let b1 = -2.0 * cos_w;
    let b2 = 1.0 - alpha * a;
    let a0 = 1.0 + alpha / a;
    let a1 = -2.0 * cos_w;
    let a2 = 1.0 - alpha / a;

    [b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0]
}

/// Process one sample through a transposed DF-II biquad.
/// Takes state by value, returns (output, new_z1, new_z2).
#[inline]
fn biquad(x: f32, z1: f32, z2: f32, c: &[f32; 5]) -> (f32, f32, f32) {
    let y    = c[0] * x + z1;
    let nz1  = c[1] * x - c[3] * y + z2;
    let nz2  = c[2] * x - c[4] * y;
    (y, nz1, nz2)
}

impl FxProcessor for FilterBankFx {
    fn process_block(&mut self, block: &mut [f32], sample_rate: u32) {
        if sample_rate != self.sample_rate {
            self.sample_rate = sample_rate;
            self.recompute_all();
        }

        let frames = block.len() / 2;
        for i in 0..frames {
            let dry_l = block[i * 2];
            let dry_r = block[i * 2 + 1];
            let mut l = dry_l;
            let mut r = dry_r;

            for b in 0..BANDS {
                let c = self.coeffs[b];
                let [z1l, z2l, z1r, z2r] = self.state[b];
                let (yl, nz1l, nz2l) = biquad(l, z1l, z2l, &c);
                let (yr, nz1r, nz2r) = biquad(r, z1r, z2r, &c);
                self.state[b] = [nz1l, nz2l, nz1r, nz2r];
                l = yl;
                r = yr;
            }

            block[i * 2]     = self.mix * l + (1.0 - self.mix) * dry_l;
            block[i * 2 + 1] = self.mix * r + (1.0 - self.mix) * dry_r;
        }
    }

    fn reset(&mut self) {
        self.state = [[0.0; 4]; BANDS];
    }

    fn set_mix(&mut self, wet: f32) {
        self.mix = wet.clamp(0.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_gains_unity_output() {
        let mut fb = FilterBankFx::new(48000);
        // All gains at 0 dB — output should be close to input (small rounding errors).
        let input: Vec<f32> = (0..64).flat_map(|i| {
            let s = (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 48000.0).sin();
            [s, s]
        }).collect();
        let mut block = input.clone();
        fb.process_block(&mut block, 48000);
        // Allow up to 1% amplitude error after transient settles (skip first 16 frames).
        let e_in:  f32 = input[32..].iter().map(|&s| s * s).sum();
        let e_out: f32 = block[32..].iter().map(|&s| s * s).sum();
        let ratio = e_out / e_in.max(1e-9);
        assert!(ratio > 0.9 && ratio < 1.1, "flat filter should be near-unity: ratio={ratio:.3}");
    }

    #[test]
    fn boost_all_bands_increases_energy() {
        let mut fb = FilterBankFx::new(48000);
        // Boost all 48 bands by +12 dB; any test signal should be amplified.
        fb.set_all_gains(&[12.0f32; BANDS]);
        let input: Vec<f32> = (0..256).flat_map(|i| {
            let s = (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 48000.0).sin();
            [s, s]
        }).collect();
        let mut block = input.clone();
        fb.process_block(&mut block, 48000);
        let e_in:  f32 = input[64..].iter().map(|&s| s * s).sum();
        let e_out: f32 = block[64..].iter().map(|&s| s * s).sum();
        assert!(e_out > e_in * 1.5, "+12 dB all-band boost should increase energy: ratio={:.2}", e_out / e_in.max(1e-9));
    }

    #[test]
    fn cut_all_bands_reduces_energy() {
        let mut fb = FilterBankFx::new(48000);
        fb.set_all_gains(&[-12.0f32; BANDS]);
        let input: Vec<f32> = (0..256).flat_map(|i| {
            let s = (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 48000.0).sin();
            [s, s]
        }).collect();
        let mut block = input.clone();
        fb.process_block(&mut block, 48000);
        let e_in:  f32 = input[64..].iter().map(|&s| s * s).sum();
        let e_out: f32 = block[64..].iter().map(|&s| s * s).sum();
        assert!(e_out < e_in * 0.5, "-12 dB all-band cut should reduce energy: ratio={:.2}", e_out / e_in.max(1e-9));
    }

    #[test]
    fn reset_clears_state() {
        let mut fb = FilterBankFx::new(48000);
        let mut block = vec![1.0f32; 128];
        fb.process_block(&mut block, 48000);
        fb.reset();
        assert!(fb.state.iter().all(|s| s.iter().all(|&v| v == 0.0)));
    }

    #[test]
    fn band_freq_range() {
        let f0 = FilterBankFx::band_freq(0);
        let fn_ = FilterBankFx::band_freq(BANDS - 1);
        assert!((f0 - 20.0).abs() < 0.1, "band 0 should be ~20 Hz: {f0}");
        assert!((fn_ - 20000.0).abs() < 1.0, "band 47 should be ~20 kHz: {fn_}");
    }
}
