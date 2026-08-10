/// 4-band parametric EQ using biquad filters.
///
/// Band layout (default):
///   0 — High-pass  (12 dB/oct, HPF)
///   1 — Low shelf  (±12 dB)
///   2 — Peak / bell
///   3 — High shelf (±12 dB) / Low-pass

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EqBandKind {
    HighPass,
    LowShelf,
    Peak,
    HighShelf,
    LowPass,
    Bypass,
}

pub struct EqBand {
    pub kind: EqBandKind,
    pub freq: f32,    // Hz
    pub gain_db: f32, // ±18 dB (ignored for HP/LP)
    pub q: f32,       // 0.1..10.0
    pub enabled: bool,
    // biquad state (stereo)
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1l: f32,
    x2l: f32,
    y1l: f32,
    y2l: f32,
    x1r: f32,
    x2r: f32,
    y1r: f32,
    y2r: f32,
    last_sr: u32,
}

impl EqBand {
    fn new(kind: EqBandKind, freq: f32, gain_db: f32, q: f32) -> Self {
        let mut b = Self {
            kind,
            freq,
            gain_db,
            q,
            enabled: true,
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            x1l: 0.0,
            x2l: 0.0,
            y1l: 0.0,
            y2l: 0.0,
            x1r: 0.0,
            x2r: 0.0,
            y1r: 0.0,
            y2r: 0.0,
            last_sr: 0,
        };
        b.compute_coeffs(44100);
        b
    }

    fn compute_coeffs(&mut self, sr: u32) {
        if self.last_sr == sr && matches!(self.kind, _) { /* recompute always for simplicity */ }
        self.last_sr = sr;
        use std::f32::consts::PI;
        let w0 = 2.0 * PI * self.freq / sr as f32;
        let cos_w = w0.cos();
        let sin_w = w0.sin();
        let alpha = sin_w / (2.0 * self.q);
        let a_lin = 10.0f32.powf(self.gain_db / 40.0); // gain as amplitude (shelves/peak)

        let (b0, b1, b2, a0, a1, a2) = match self.kind {
            EqBandKind::HighPass => {
                let b0 = (1.0 + cos_w) / 2.0;
                let b1 = -(1.0 + cos_w);
                let b2 = (1.0 + cos_w) / 2.0;
                let a0 = 1.0 + alpha;
                let a1 = -2.0 * cos_w;
                let a2 = 1.0 - alpha;
                (b0, b1, b2, a0, a1, a2)
            }
            EqBandKind::LowPass => {
                let b0 = (1.0 - cos_w) / 2.0;
                let b1 = 1.0 - cos_w;
                let b2 = (1.0 - cos_w) / 2.0;
                let a0 = 1.0 + alpha;
                let a1 = -2.0 * cos_w;
                let a2 = 1.0 - alpha;
                (b0, b1, b2, a0, a1, a2)
            }
            EqBandKind::LowShelf => {
                let a_sq = a_lin.sqrt();
                let b0 = a_lin * ((a_lin + 1.0) - (a_lin - 1.0) * cos_w + 2.0 * a_sq * alpha);
                let b1 = 2.0 * a_lin * ((a_lin - 1.0) - (a_lin + 1.0) * cos_w);
                let b2 = a_lin * ((a_lin + 1.0) - (a_lin - 1.0) * cos_w - 2.0 * a_sq * alpha);
                let a0 = (a_lin + 1.0) + (a_lin - 1.0) * cos_w + 2.0 * a_sq * alpha;
                let a1 = -2.0 * ((a_lin - 1.0) + (a_lin + 1.0) * cos_w);
                let a2 = (a_lin + 1.0) + (a_lin - 1.0) * cos_w - 2.0 * a_sq * alpha;
                (b0, b1, b2, a0, a1, a2)
            }
            EqBandKind::HighShelf => {
                let a_sq = a_lin.sqrt();
                let b0 = a_lin * ((a_lin + 1.0) + (a_lin - 1.0) * cos_w + 2.0 * a_sq * alpha);
                let b1 = -2.0 * a_lin * ((a_lin - 1.0) + (a_lin + 1.0) * cos_w);
                let b2 = a_lin * ((a_lin + 1.0) + (a_lin - 1.0) * cos_w - 2.0 * a_sq * alpha);
                let a0 = (a_lin + 1.0) - (a_lin - 1.0) * cos_w + 2.0 * a_sq * alpha;
                let a1 = 2.0 * ((a_lin - 1.0) - (a_lin + 1.0) * cos_w);
                let a2 = (a_lin + 1.0) - (a_lin - 1.0) * cos_w - 2.0 * a_sq * alpha;
                (b0, b1, b2, a0, a1, a2)
            }
            EqBandKind::Peak => {
                let b0 = 1.0 + alpha * a_lin;
                let b1 = -2.0 * cos_w;
                let b2 = 1.0 - alpha * a_lin;
                let a0 = 1.0 + alpha / a_lin;
                let a1 = -2.0 * cos_w;
                let a2 = 1.0 - alpha / a_lin;
                (b0, b1, b2, a0, a1, a2)
            }
            EqBandKind::Bypass => (1.0, 0.0, 0.0, 1.0, 0.0, 0.0),
        };

        let inv_a0 = 1.0 / a0;
        self.b0 = b0 * inv_a0;
        self.b1 = b1 * inv_a0;
        self.b2 = b2 * inv_a0;
        self.a1 = a1 * inv_a0;
        self.a2 = a2 * inv_a0;
    }

    #[inline]
    fn process_sample_l(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1l + self.b2 * self.x2l
            - self.a1 * self.y1l
            - self.a2 * self.y2l;
        self.x2l = self.x1l;
        self.x1l = x;
        self.y2l = self.y1l;
        self.y1l = y;
        y
    }

    #[inline]
    fn process_sample_r(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1r + self.b2 * self.x2r
            - self.a1 * self.y1r
            - self.a2 * self.y2r;
        self.x2r = self.x1r;
        self.x1r = x;
        self.y2r = self.y1r;
        self.y1r = y;
        y
    }

    fn clear_state(&mut self) {
        self.x1l = 0.0;
        self.x2l = 0.0;
        self.y1l = 0.0;
        self.y2l = 0.0;
        self.x1r = 0.0;
        self.x2r = 0.0;
        self.y1r = 0.0;
        self.y2r = 0.0;
    }
}

pub struct ParametricEq {
    pub bands: [EqBand; 4],
    mix: f32,
    last_sr: u32,
}

impl ParametricEq {
    pub fn new() -> Self {
        Self {
            bands: [
                EqBand::new(EqBandKind::HighPass, 80.0, 0.0, 0.707),
                EqBand::new(EqBandKind::LowShelf, 200.0, 0.0, 0.707),
                EqBand::new(EqBandKind::Peak, 1000.0, 0.0, 1.0),
                EqBand::new(EqBandKind::HighShelf, 8000.0, 0.0, 0.707),
            ],
            mix: 1.0,
            last_sr: 0,
        }
    }
}

impl Default for ParametricEq {
    fn default() -> Self {
        Self::new()
    }
}

impl super::FxProcessor for ParametricEq {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        if buf.len() < 2 {
            return;
        }
        if sample_rate != self.last_sr {
            self.last_sr = sample_rate;
            for b in &mut self.bands {
                b.compute_coeffs(sample_rate);
            }
        }
        let frames = buf.len() / 2;
        for i in 0..frames {
            let orig_l = buf[i * 2];
            let orig_r = buf[i * 2 + 1];
            let mut l = orig_l;
            let mut r = orig_r;
            for band in &mut self.bands {
                if band.enabled && !matches!(band.kind, EqBandKind::Bypass) {
                    l = band.process_sample_l(l);
                    r = band.process_sample_r(r);
                }
            }
            buf[i * 2] = orig_l + self.mix * (l - orig_l);
            buf[i * 2 + 1] = orig_r + self.mix * (r - orig_r);
        }
    }

    fn reset(&mut self) {
        for b in &mut self.bands {
            b.clear_state();
        }
    }

    fn set_mix(&mut self, wet: f32) {
        self.mix = wet.clamp(0.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::FxProcessor;

    fn sine_block(freq_hz: f32, sr: u32, frames: usize) -> Vec<f32> {
        let mut buf = vec![0.0f32; frames * 2];
        for i in 0..frames {
            let s = (2.0 * std::f32::consts::PI * freq_hz * i as f32 / sr as f32).sin();
            buf[i * 2] = s;
            buf[i * 2 + 1] = s;
        }
        buf
    }

    fn rms(buf: &[f32]) -> f32 {
        let sum: f32 = buf.iter().map(|s| s * s).sum();
        (sum / buf.len() as f32).sqrt()
    }

    #[test]
    fn all_bands_bypass_is_unity() {
        let mut eq = ParametricEq::new();
        for band in &mut eq.bands {
            band.kind = EqBandKind::Bypass;
        }
        let mut buf = sine_block(440.0, 48000, 512);
        let before = rms(&buf);
        eq.process_block(&mut buf, 48000);
        let after = rms(&buf);
        assert!(
            (after - before).abs() < 1e-4,
            "bypass should be unity, before={before} after={after}"
        );
    }

    #[test]
    fn peak_boost_increases_level_at_target_freq() {
        let mut eq = ParametricEq::new();
        for band in &mut eq.bands {
            band.kind = EqBandKind::Bypass;
        }
        eq.bands[2].kind = EqBandKind::Peak;
        eq.bands[2].freq = 1000.0;
        eq.bands[2].gain_db = 12.0;
        eq.bands[2].q = 2.0;

        let mut buf = sine_block(1000.0, 48000, 4096);
        let before = rms(&buf);
        eq.process_block(&mut buf, 48000);
        let after = rms(&buf[4096..]); // skip transient startup
        assert!(
            after > before * 1.5,
            "12 dB peak should boost 1 kHz, before={before} after={after}"
        );
    }

    #[test]
    fn high_pass_attenuates_dc() {
        let mut eq = ParametricEq::new();
        for band in &mut eq.bands {
            band.kind = EqBandKind::Bypass;
        }
        eq.bands[0].kind = EqBandKind::HighPass;
        eq.bands[0].freq = 200.0;

        // DC signal (0 Hz) should be heavily attenuated by the HP filter.
        let mut buf = vec![1.0f32; 2048]; // stereo DC
        eq.process_block(&mut buf, 48000);
        let last = buf[2040].abs();
        assert!(last < 0.1, "HP filter should attenuate DC, got {last}");
    }

    #[test]
    fn low_shelf_cut_reduces_bass() {
        let mut eq = ParametricEq::new();
        for band in &mut eq.bands {
            band.kind = EqBandKind::Bypass;
        }
        eq.bands[1].kind = EqBandKind::LowShelf;
        eq.bands[1].freq = 500.0;
        eq.bands[1].gain_db = -12.0;

        let mut buf = sine_block(100.0, 48000, 4096);
        let before = rms(&buf);
        eq.process_block(&mut buf, 48000);
        let after = rms(&buf[4096..]);
        assert!(
            after < before * 0.5,
            "−12 dB low shelf should reduce 100 Hz, before={before} after={after}"
        );
    }
}
