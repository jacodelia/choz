/// Feed-forward peak compressor and hard limiter.
///
/// Both share the same gain-computer topology; `is_limiter = true` forces
/// ratio to infinity and snaps attack to 0.1 ms.
pub struct Compressor {
    pub threshold_db: f32, // -60..0
    pub ratio: f32,        // 1.0..100.0 (ignored when is_limiter=true)
    pub attack_ms: f32,
    pub release_ms: f32,
    pub makeup_db: f32, // 0..24
    pub knee_db: f32,   // 0..12  (soft knee width)
    pub is_limiter: bool,
    mix: f32,
    // RT state
    envelope: f32,
    gain_smooth: f32,
}

impl Compressor {
    pub fn new() -> Self {
        Self {
            threshold_db: -12.0,
            ratio: 4.0,
            attack_ms: 10.0,
            release_ms: 100.0,
            makeup_db: 0.0,
            knee_db: 6.0,
            is_limiter: false,
            mix: 1.0,
            envelope: 0.0,
            gain_smooth: 1.0,
        }
    }

    pub fn limiter() -> Self {
        Self {
            threshold_db: -0.3,
            ratio: 100.0,
            attack_ms: 0.1,
            release_ms: 50.0,
            makeup_db: 0.0,
            knee_db: 0.0,
            is_limiter: true,
            mix: 1.0,
            envelope: 0.0,
            gain_smooth: 1.0,
        }
    }

    fn gain_reduction_db(&self, level_db: f32) -> f32 {
        let thr = self.threshold_db;
        let ratio = if self.is_limiter { 1000.0 } else { self.ratio };
        let knee = self.knee_db;

        if knee > 0.0 {
            let diff = level_db - thr;
            let half_knee = knee * 0.5;
            if diff < -half_knee {
                0.0
            } else if diff < half_knee {
                let t = (diff + half_knee) / knee;
                (1.0 / ratio - 1.0) * t * t * knee * 0.5
            } else {
                (level_db - thr) * (1.0 / ratio - 1.0)
            }
        } else {
            if level_db > thr {
                (level_db - thr) * (1.0 / ratio - 1.0)
            } else {
                0.0
            }
        }
    }
}

impl Default for Compressor {
    fn default() -> Self { Self::new() }
}

impl super::FxProcessor for Compressor {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        if buf.len() < 2 { return; }
        let sr = sample_rate as f32;
        let attack_coeff  = (-1.0 / (self.attack_ms  * 0.001 * sr)).exp();
        let release_coeff = (-1.0 / (self.release_ms * 0.001 * sr)).exp();
        let makeup_linear = db_to_linear(self.makeup_db);

        let frames = buf.len() / 2;
        for i in 0..frames {
            let l = buf[i * 2];
            let r = buf[i * 2 + 1];

            // Peak envelope follower
            let peak = l.abs().max(r.abs());
            if peak > self.envelope {
                self.envelope = attack_coeff  * (self.envelope - peak) + peak;
            } else {
                self.envelope = release_coeff * (self.envelope - peak) + peak;
            }

            // Gain computer
            let level_db  = linear_to_db(self.envelope.max(1e-10));
            let gr_db     = self.gain_reduction_db(level_db);
            let target_gain = db_to_linear(gr_db) * makeup_linear;

            // Gain smoother (same attack/release)
            if target_gain < self.gain_smooth {
                self.gain_smooth = attack_coeff  * (self.gain_smooth - target_gain) + target_gain;
            } else {
                self.gain_smooth = release_coeff * (self.gain_smooth - target_gain) + target_gain;
            }

            let wet_l = l * self.gain_smooth;
            let wet_r = r * self.gain_smooth;
            buf[i * 2]     = l + self.mix * (wet_l - l);
            buf[i * 2 + 1] = r + self.mix * (wet_r - r);
        }
    }

    fn reset(&mut self) {
        self.envelope    = 0.0;
        self.gain_smooth = 1.0;
    }

    fn set_mix(&mut self, wet: f32) { self.mix = wet.clamp(0.0, 1.0); }

    fn name(&self) -> &str { if self.is_limiter { "Limiter" } else { "Compressor" } }

    fn params(&self) -> Vec<crate::fx::FxParam> {
        use crate::fx::FxParam;
        vec![
            FxParam::new("Threshold", (self.threshold_db + 60.0) / 60.0, -60.0, 0.0, "dB"),
            FxParam::new("Ratio",     ((self.ratio - 1.0) / 99.0).clamp(0.0, 1.0), 1.0, 100.0, ":1"),
            FxParam::new("Attack",    (self.attack_ms / 200.0).clamp(0.0, 1.0), 0.0, 200.0, "ms"),
            FxParam::new("Release",   (self.release_ms / 2000.0).clamp(0.0, 1.0), 0.0, 2000.0, "ms"),
            FxParam::new("Makeup",    (self.makeup_db / 24.0).clamp(0.0, 1.0), 0.0, 24.0, "dB"),
            FxParam::new("Knee",      (self.knee_db / 12.0).clamp(0.0, 1.0), 0.0, 12.0, "dB"),
            FxParam::new("Wet",       self.mix, 0.0, 1.0, ""),
        ]
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => self.threshold_db = -60.0 + v * 60.0,
            1 => self.ratio        = 1.0 + v * 19.0,
            2 => self.attack_ms    = 0.1 + v * 99.9,
            3 => self.release_ms   = 10.0 + v * 990.0,
            4 => self.makeup_db    = v * 24.0,
            5 => self.knee_db      = v * 12.0,
            6 => self.mix          = v,
            _ => {}
        }
    }
}

#[inline] fn db_to_linear(db: f32) -> f32 { 10.0f32.powf(db / 20.0) }
#[inline] fn linear_to_db(lin: f32) -> f32 { 20.0 * lin.max(1e-10).log10() }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::FxProcessor;

    #[test]
    fn unity_gain_below_threshold() {
        let mut c = Compressor::new();
        c.threshold_db = -6.0;
        c.ratio = 4.0;
        c.attack_ms = 0.0;
        c.release_ms = 0.0;
        c.makeup_db = 0.0;
        c.knee_db = 0.0;
        // Signal well below threshold: gain reduction should be ~0.
        let mut buf = vec![0.01f32; 256]; // 256 interleaved samples (128 frames)
        let before = buf[0];
        c.process_block(&mut buf, 48000);
        // Output should be close to input (minimal compression below threshold).
        assert!((buf[0] - before).abs() < 0.01, "expected ~unity below threshold");
    }

    #[test]
    fn gain_reduction_above_threshold() {
        let mut c = Compressor::new();
        c.threshold_db = -20.0;
        c.ratio = 10.0;
        c.attack_ms = 0.1;
        c.release_ms = 10.0;
        c.makeup_db = 0.0;
        c.knee_db = 0.0;
        // Hot signal well above threshold.
        let mut buf = vec![0.5f32; 1024];
        c.process_block(&mut buf, 48000);
        // After enough frames the gain should be significantly reduced.
        let last = buf[1020].abs();
        assert!(last < 0.45, "expected gain reduction, got {}", last);
    }

    #[test]
    fn limiter_prevents_overshoot() {
        let mut lim = Compressor::limiter();
        let mut buf = vec![2.0f32; 1024];
        lim.process_block(&mut buf, 48000);
        // Limiter should bring the signal down to ~threshold.
        let last = buf[1020].abs();
        assert!(last < 1.2, "limiter should prevent overshoot, got {}", last);
    }
}
