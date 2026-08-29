//! Cassette saturation FX.
//!
//! Emulates the character of recording to magnetic tape via:
//!   1. Pre-emphasis  — 1-pole HP shelving boost of HF before saturation
//!   2. Soft-clip     — tanh waveshaper; `drive` controls harmonic content
//!   3. De-emphasis   — matching 1-pole LP shelving cut (Dolby-style)
//!   4. Flutter noise — tiny broadband noise simulating tape flutter
//!
//! No dynamic compression stage to keep latency at zero (sample-by-sample).

use super::FxProcessor;
use super::dc::DcBlock;

/// Cassette tape saturation effect.
pub struct Cassette {
    /// Saturation drive (1.0 = gentle, 4.0+ = heavy). Default: 2.0.
    drive: f32,
    /// Pre/de-emphasis corner frequency (Hz). Default: 4000 Hz.
    emphasis_hz: f32,
    /// Flutter/hiss amplitude (0.0 = silent, 0.003 = subtle). Default: 0.001.
    noise_amp: f32,
    /// Dry/wet mix.
    wet: f32,
    /// Pre-emphasis 1-pole LP state (stereo) — HP = input - LP.
    pre_lp: [f32; 2],
    /// Pre-emphasis coefficient (α for the LP pole).
    pre_alpha: f32,
    /// De-emphasis 1-pole LP state (stereo).
    de_lp: [f32; 2],
    /// De-emphasis coefficient (same α, inverse of pre).
    de_alpha: f32,
    /// LCG seed for noise.
    rng: u32,
    /// What the drive is allowed to add to the level — see [`Cassette::update_makeup`].
    makeup: f32,
    /// The tape curve is asymmetric once the emphasis has tilted the signal;
    /// what it leaves behind would eat headroom further down the chain.
    dc: [DcBlock; 2],
    sample_rate: u32,
}

impl Cassette {
    pub fn new() -> Self {
        let mut c = Self {
            drive: 2.0,
            emphasis_hz: 4000.0,
            noise_amp: 0.001,
            wet: 1.0,
            pre_lp: [0.0; 2],
            pre_alpha: 0.0,
            de_lp: [0.0; 2],
            de_alpha: 0.0,
            rng: 0xCAFE_BABE,
            makeup: 1.0,
            dc: [DcBlock::new(48000.0); 2],
            sample_rate: 48000,
        };
        c.update_coeffs();
        c.update_makeup();
        c
    }

    pub fn set_drive(&mut self, d: f32) {
        self.drive = d.clamp(0.5, 8.0);
        self.update_makeup();
    }

    /// What the drive is allowed to add to the level.
    ///
    /// A tape is a saturator, and a saturator is a *shape*, not a volume: the
    /// drive knob decides how hard the curve is worked, and if it also decides
    /// how loud the effect is then the level jumps whenever the character does
    /// — and a chain of effects becomes a stack of gains. Measured against a
    /// bus-level signal, the cassette was 4.6 dB louder than what went into it
    /// at its default drive, the loudest built-in in the suite.
    ///
    /// Half the drive in dB (`1/√drive`), not all of it: a tape that got
    /// quieter the harder it was hit would not read as a tape either. At the
    /// top of the knob the curve is doing the limiting and the makeup lands
    /// near unity, which is where it belongs.
    ///
    /// Kept rather than worked out per sample: it changes when the knob does,
    /// which is not ninety-six thousand times a second.
    fn update_makeup(&mut self) {
        self.makeup = 1.0 / self.drive.sqrt();
    }
    pub fn set_noise(&mut self, amp: f32) {
        self.noise_amp = amp.clamp(0.0, 0.1);
    }
    pub fn set_emphasis_hz(&mut self, hz: f32) {
        self.emphasis_hz = hz.clamp(500.0, 15000.0);
        self.update_coeffs();
    }

    fn update_coeffs(&mut self) {
        // α = e^(-2π·f/sr) — standard 1-pole LP decay coefficient.
        let rc = 1.0 / (2.0 * std::f32::consts::PI * self.emphasis_hz);
        let dt = 1.0 / self.sample_rate as f32;
        let alpha = (-dt / rc).exp();
        self.pre_alpha = alpha;
        self.de_alpha = alpha;
    }

    #[inline]
    fn rand_f32(&mut self) -> f32 {
        self.rng = self.rng.wrapping_mul(1664525).wrapping_add(1013904223);
        (self.rng >> 8) as f32 / (1u32 << 24) as f32 - 0.5
    }

    #[inline]
    fn process_sample(&mut self, ch: usize, x: f32) -> f32 {
        // 1. Pre-emphasis: LP pole → high-shelf via HP = input - LP.
        self.pre_lp[ch] = self.pre_alpha * self.pre_lp[ch] + (1.0 - self.pre_alpha) * x;
        let pre = x + 0.7 * (x - self.pre_lp[ch]); // add boosted HF

        // 2. Soft-clip (tanh).
        let driven = pre * self.drive;
        let sat = driven.tanh();

        // 3. De-emphasis: 1-pole LP removes the pre-emphasis bump.
        self.de_lp[ch] = self.de_alpha * self.de_lp[ch] + (1.0 - self.de_alpha) * sat;
        let de = self.de_lp[ch];

        // 4. Flutter noise (only on L channel to avoid phase cancellation).
        let noise = if ch == 0 {
            self.rand_f32() * self.noise_amp
        } else {
            0.0
        };

        self.dc[ch].process((de + noise) * self.makeup)
    }
}

impl Default for Cassette {
    fn default() -> Self {
        Self::new()
    }
}

impl FxProcessor for Cassette {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        if self.sample_rate != sample_rate {
            self.sample_rate = sample_rate;
            self.update_coeffs();
            self.dc = [DcBlock::new(sample_rate as f32); 2];
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
        self.pre_lp = [0.0; 2];
        self.de_lp = [0.0; 2];
        for dc in &mut self.dc {
            dc.reset();
        }
    }

    fn set_mix(&mut self, wet: f32) {
        self.wet = wet.clamp(0.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cassette_is_nonlinear() {
        let sr = 48000u32;
        let amp = 0.9f32;
        // Process loud signal (0.9 amplitude) and quiet signal (0.09 amplitude).
        let mut loud: Vec<f32> = (0..512)
            .flat_map(|i| {
                let s = amp * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / sr as f32).sin();
                [s, s]
            })
            .collect();
        let mut quiet: Vec<f32> = loud.iter().map(|&s| s * 0.1).collect();

        let mut fx = Cassette::new();
        let mut fx2 = Cassette::new();
        fx.process_block(&mut loud, sr);
        fx2.process_block(&mut quiet, sr);

        // Scale quiet output back up: if linear, peaks would match; saturation makes loud smaller.
        let peak_loud: f32 = loud.iter().copied().map(f32::abs).fold(0.0, f32::max);
        let peak_quiet: f32 = quiet.iter().map(|&s| s.abs() * 10.0).fold(0.0, f32::max);
        assert!(peak_loud < peak_quiet,
            "saturation should reduce loud peak vs scaled quiet: loud={peak_loud:.4} quiet×10={peak_quiet:.4}");
    }

    #[test]
    fn the_offset_a_tape_leaves_does_not_reach_the_next_effect() {
        let sr = 48000u32;
        // Asymmetric: a sine that only swings up, which is what leaves a bias.
        let mut buf: Vec<f32> = (0..sr as usize)
            .flat_map(|i| {
                let s = (std::f32::consts::TAU * 220.0 * i as f32 / sr as f32)
                    .sin()
                    .max(0.0)
                    * 0.8;
                [s, s]
            })
            .collect();
        let mut fx = Cassette::new();
        fx.process_block(&mut buf, sr);
        // Mean of the last 100 ms.
        let tail = &buf[buf.len() - 9600..];
        let mean = tail.iter().sum::<f32>() / tail.len() as f32;
        assert!(mean.abs() < 0.01, "DC left on the output: {mean}");
    }

    #[test]
    fn reset_clears_state() {
        let mut fx = Cassette::new();
        let mut buf = vec![0.8f32; 128];
        fx.process_block(&mut buf, 48000);
        fx.reset();
        assert_eq!(fx.pre_lp, [0.0; 2]);
        assert_eq!(fx.de_lp, [0.0; 2]);
    }
}
