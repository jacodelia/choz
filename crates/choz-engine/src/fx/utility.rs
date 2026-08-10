//! Utility processors: Gain, Phase Invert, Mono Maker, Soft Clipper, Tube Saturation.

// ─── Gain ────────────────────────────────────────────────────────────────────

/// Simple gain stage (dB).
pub struct Gain {
    pub gain_db: f32, // -60..+24
    mix: f32,
}

impl Gain {
    pub fn new() -> Self {
        Self {
            gain_db: 0.0,
            mix: 1.0,
        }
    }
}

impl Default for Gain {
    fn default() -> Self {
        Self::new()
    }
}

impl super::FxProcessor for Gain {
    fn process_block(&mut self, buf: &mut [f32], _sr: u32) {
        let g = 10.0f32.powf(self.gain_db / 20.0);
        for s in buf.iter_mut() {
            *s *= g;
        }
    }
    fn reset(&mut self) {}
    fn set_mix(&mut self, wet: f32) {
        self.mix = wet.clamp(0.0, 1.0);
    }
}

// ─── Phase Invert ─────────────────────────────────────────────────────────────

/// Polarity inversion per channel.
pub struct PhaseInvert {
    pub invert_l: bool,
    pub invert_r: bool,
}

impl PhaseInvert {
    pub fn new() -> Self {
        Self {
            invert_l: true,
            invert_r: false,
        }
    }
    pub fn both() -> Self {
        Self {
            invert_l: true,
            invert_r: true,
        }
    }
}

impl Default for PhaseInvert {
    fn default() -> Self {
        Self::new()
    }
}

impl super::FxProcessor for PhaseInvert {
    fn process_block(&mut self, buf: &mut [f32], _sr: u32) {
        let frames = buf.len() / 2;
        for i in 0..frames {
            if self.invert_l {
                buf[i * 2] = -buf[i * 2];
            }
            if self.invert_r {
                buf[i * 2 + 1] = -buf[i * 2 + 1];
            }
        }
    }
    fn reset(&mut self) {}
    fn set_mix(&mut self, _wet: f32) {}
}

// ─── Mono Maker ───────────────────────────────────────────────────────────────

/// Sum L+R to mono on both channels.
pub struct MonoMaker {
    mix: f32,
}

impl MonoMaker {
    pub fn new() -> Self {
        Self { mix: 1.0 }
    }
}

impl Default for MonoMaker {
    fn default() -> Self {
        Self::new()
    }
}

impl super::FxProcessor for MonoMaker {
    fn process_block(&mut self, buf: &mut [f32], _sr: u32) {
        let frames = buf.len() / 2;
        for i in 0..frames {
            let l = buf[i * 2];
            let r = buf[i * 2 + 1];
            let m = (l + r) * 0.5;
            buf[i * 2] = l + self.mix * (m - l);
            buf[i * 2 + 1] = r + self.mix * (m - r);
        }
    }
    fn reset(&mut self) {}
    fn set_mix(&mut self, wet: f32) {
        self.mix = wet.clamp(0.0, 1.0);
    }
}

// ─── Soft Clipper ─────────────────────────────────────────────────────────────

/// RBJ biquad lowpass (transposed direct-form II) — the anti-alias filter for
/// the 2× oversampled saturators below.
#[derive(Clone, Copy)]
pub(crate) struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}
impl Biquad {
    pub(crate) fn lowpass(fc: f32, sr: f32, q: f32) -> Self {
        let w0 = 2.0 * std::f32::consts::PI * (fc / sr).clamp(1e-4, 0.49);
        let (sn, cs) = w0.sin_cos();
        let alpha = sn / (2.0 * q);
        let a0 = 1.0 + alpha;
        let b1 = (1.0 - cs) / a0;
        Self {
            b0: b1 / 2.0,
            b1,
            b2: b1 / 2.0,
            a1: (-2.0 * cs) / a0,
            a2: (1.0 - alpha) / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }
    #[inline]
    pub(crate) fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// 2× oversampler for a per-sample nonlinearity. Linear-interpolated upsample,
/// the nonlinearity applied at the doubled rate, then a 2nd-order Butterworth
/// lowpass before decimation — so the harmonics a hard waveshaper generates
/// above Nyquist are filtered instead of folding back as aliasing.
#[derive(Clone, Copy)]
pub(crate) struct Oversampler2x {
    last: f32,
    lp: Biquad,
}
impl Oversampler2x {
    pub(crate) fn new(base_sr: f32) -> Self {
        // Filter runs at 2× rate; cut just below the base Nyquist.
        Self {
            last: 0.0,
            lp: Biquad::lowpass(base_sr * 0.45, base_sr * 2.0, 0.707),
        }
    }
    #[inline]
    pub(crate) fn process<F: Fn(f32) -> f32>(&mut self, x: f32, f: F) -> f32 {
        let mid = 0.5 * (self.last + x); // upsampled sample between last and x
        self.last = x;
        let _ = self.lp.process(f(mid)); // first half-rate sample (discarded)
        self.lp.process(f(x)) // second → decimated output
    }
}

/// Smooth soft-clip saturation using `tanh`, 2× oversampled to suppress the
/// aliasing a `tanh` waveshaper would otherwise fold back at high drive.
pub struct SoftClipper {
    pub drive: f32, // 1.0..10.0 — amount of gain into the clipper
    mix: f32,
    os_l: Oversampler2x,
    os_r: Oversampler2x,
    sample_rate: u32,
}

impl SoftClipper {
    pub fn new() -> Self {
        Self {
            drive: 2.0,
            mix: 1.0,
            os_l: Oversampler2x::new(48000.0),
            os_r: Oversampler2x::new(48000.0),
            sample_rate: 48000,
        }
    }
}

impl Default for SoftClipper {
    fn default() -> Self {
        Self::new()
    }
}

impl super::FxProcessor for SoftClipper {
    fn process_block(&mut self, buf: &mut [f32], sr: u32) {
        if sr != self.sample_rate {
            self.sample_rate = sr;
            self.os_l = Oversampler2x::new(sr as f32);
            self.os_r = Oversampler2x::new(sr as f32);
        }
        let drive = self.drive;
        let inv_drive = 1.0 / drive;
        let shape = |x: f32| (x * drive).tanh() * inv_drive;
        let frames = buf.len() / 2;
        for i in 0..frames {
            let l = buf[i * 2];
            let r = buf[i * 2 + 1];
            let wl = self.os_l.process(l, shape);
            let wr = self.os_r.process(r, shape);
            buf[i * 2] = l + self.mix * (wl - l);
            buf[i * 2 + 1] = r + self.mix * (wr - r);
        }
    }
    fn reset(&mut self) {
        self.os_l = Oversampler2x::new(self.sample_rate as f32);
        self.os_r = Oversampler2x::new(self.sample_rate as f32);
    }
    fn set_mix(&mut self, wet: f32) {
        self.mix = wet.clamp(0.0, 1.0);
    }
}

// ─── Tube Saturation ──────────────────────────────────────────────────────────

/// Warm asymmetric tube-style saturation using a polynomial waveshaper.
///
/// Combines soft-clip for positive half-waves with a slightly harder
/// negative-half characteristic, reproducing the asymmetry of a triode stage.
pub struct TubeSaturation {
    pub drive: f32, // 1.0..20.0
    pub tone: f32,  // 0.0..1.0 — amount of 1-pole HP to remove mud
    mix: f32,
    hp_state_l: f32,
    hp_state_r: f32,
    os_l: Oversampler2x,
    os_r: Oversampler2x,
    sample_rate: u32,
}

impl TubeSaturation {
    pub fn new() -> Self {
        Self {
            drive: 3.0,
            tone: 0.3,
            mix: 0.6,
            hp_state_l: 0.0,
            hp_state_r: 0.0,
            os_l: Oversampler2x::new(48000.0),
            os_r: Oversampler2x::new(48000.0),
            sample_rate: 48000,
        }
    }

    #[inline]
    fn waveshape(x: f32, drive: f32) -> f32 {
        let x_d = x * drive;
        if x_d >= 0.0 {
            // Soft positive half
            x_d.tanh() / drive
        } else {
            // Slightly harder negative half (asymmetry)
            let clamped = x_d.max(-1.0);
            (clamped - clamped * clamped * clamped / 3.0) / drive
        }
    }
}

impl Default for TubeSaturation {
    fn default() -> Self {
        Self::new()
    }
}

impl super::FxProcessor for TubeSaturation {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        if buf.len() < 2 {
            return;
        }
        if sample_rate != self.sample_rate {
            self.sample_rate = sample_rate;
            self.os_l = Oversampler2x::new(sample_rate as f32);
            self.os_r = Oversampler2x::new(sample_rate as f32);
        }
        let sr = sample_rate as f32;
        // 1-pole HP to remove DC / low-frequency mud; fc ≈ 80 Hz
        let hp_coeff = 1.0 - (-2.0 * std::f32::consts::PI * 80.0 / sr).exp();
        let drive = self.drive;
        let shape = |x: f32| Self::waveshape(x, drive);

        let frames = buf.len() / 2;
        for i in 0..frames {
            let l = buf[i * 2];
            let r = buf[i * 2 + 1];

            // 2× oversampled waveshaping (the asymmetric shaper is rich in
            // aliasing-prone harmonics); tone HP stays at base rate.
            let sat_l = self.os_l.process(l, shape);
            let sat_r = self.os_r.process(r, shape);

            // Tone HP (removes muddiness introduced by saturation)
            self.hp_state_l += hp_coeff * (sat_l - self.hp_state_l) * self.tone;
            self.hp_state_r += hp_coeff * (sat_r - self.hp_state_r) * self.tone;
            let out_l = sat_l - self.hp_state_l;
            let out_r = sat_r - self.hp_state_r;

            buf[i * 2] = l + self.mix * (out_l - l);
            buf[i * 2 + 1] = r + self.mix * (out_r - r);
        }
    }

    fn reset(&mut self) {
        self.hp_state_l = 0.0;
        self.hp_state_r = 0.0;
        self.os_l = Oversampler2x::new(self.sample_rate as f32);
        self.os_r = Oversampler2x::new(self.sample_rate as f32);
    }
    fn set_mix(&mut self, wet: f32) {
        self.mix = wet.clamp(0.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::FxProcessor;

    #[test]
    fn phase_invert_l_flips_left() {
        let mut p = PhaseInvert::new(); // invert_l=true, invert_r=false
        let mut buf = vec![0.5f32, 0.3f32, 0.5f32, 0.3f32];
        p.process_block(&mut buf, 48000);
        assert!((buf[0] - (-0.5)).abs() < 1e-6, "L should be inverted");
        assert!((buf[1] - 0.3).abs() < 1e-6, "R should be unchanged");
    }

    #[test]
    fn mono_maker_sums_to_mono() {
        let mut m = MonoMaker::new();
        let mut buf = vec![1.0f32, 0.0f32, 1.0f32, 0.0f32]; // L=1, R=0
        m.process_block(&mut buf, 48000);
        assert!((buf[0] - 0.5).abs() < 1e-6, "L should be 0.5");
        assert!((buf[1] - 0.5).abs() < 1e-6, "R should be 0.5");
    }

    #[test]
    fn gain_unity_at_zero_db() {
        let mut g = Gain::new(); // 0.0 dB
        let mut buf = vec![0.4f32; 8];
        let before = buf[0];
        g.process_block(&mut buf, 48000);
        assert!((buf[0] - before).abs() < 1e-5, "0 dB gain should be unity");
    }

    #[test]
    fn soft_clipper_reduces_hot_signal() {
        let mut s = SoftClipper::new();
        s.drive = 5.0;
        // Settle the oversampler with a steady hot DC level.
        let mut buf = vec![2.0f32; 256];
        s.process_block(&mut buf, 48000);
        assert!(
            buf.last().copied().unwrap() < 1.5,
            "soft clipper should reduce amplitude"
        );
    }

    #[test]
    fn oversampling_reduces_aliasing() {
        // Hard-drive a high sine (near Nyquist/4) through tanh saturation. With 2×
        // oversampling the out-of-band aliasing energy stays well below the
        // fundamental. Compare oversampled vs. a naive 1× tanh reference.
        let sr = 48000.0f32;
        let f0 = 9000.0f32; // harmonics 27 k/45 k fold below Nyquist if not filtered
        let n = 4096;
        let sig: Vec<f32> = (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * f0 * i as f32 / sr).sin())
            .collect();

        let drive = 6.0f32;
        // Naive 1× reference.
        let naive: Vec<f32> = sig.iter().map(|&x| (x * drive).tanh() / drive).collect();
        // Oversampled.
        let mut os = Oversampler2x::new(sr);
        let over: Vec<f32> = sig
            .iter()
            .map(|&x| os.process(x, |v| (v * drive).tanh() / drive))
            .collect();

        // Crude aliasing proxy: energy of the sample-to-sample difference that is
        // NOT explained by the fundamental tends to drop with anti-aliasing.
        let hf = |v: &[f32]| -> f32 { v.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum::<f32>() };
        let alias_naive = hf(&naive);
        let alias_over = hf(&over);
        assert!(alias_over < alias_naive,
            "oversampling should reduce HF/alias energy: over={alias_over:.3} naive={alias_naive:.3}");
    }
}
