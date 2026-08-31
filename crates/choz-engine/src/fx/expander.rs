//! Upward/downward expander — the complement to the `Gate`.
//!
//! - Downward expansion (`ratio > 1.0`): signals below threshold are attenuated.
//!   At high ratios (≥ 10) this behaves like a gate with a smooth knee.
//! - Upward expansion (`ratio < 1.0`): signals below threshold are boosted.
//!   Useful for bringing up soft transients.

pub struct Expander {
    pub threshold_db: f32, // default -40 dB
    pub ratio: f32,        // > 1.0 = downward, < 1.0 = upward; 1.0 = bypass
    pub attack_ms: f32,    // default 10 ms
    pub release_ms: f32,   // default 100 ms
    pub range_db: f32,     // max gain change magnitude (0 = unlimited)
    mix: f32,
    // detector state
    env: f32,
    gain_db: f32,
}

impl Expander {
    pub fn new() -> Self {
        Self {
            threshold_db: -40.0,
            ratio: 2.0,
            attack_ms: 10.0,
            release_ms: 100.0,
            range_db: 60.0,
            mix: 1.0,
            env: 0.0,
            gain_db: 0.0,
        }
    }
}

impl Default for Expander {
    fn default() -> Self {
        Self::new()
    }
}

impl super::FxProcessor for Expander {
    /// The five knobs the rack draws, in `set_param` order. No dry/wet: an
    /// expander that is half in is a gate that does not gate.
    fn params(&self) -> Vec<crate::fx::FxParam> {
        use crate::fx::FxParam;
        vec![
            FxParam::new("Thresh", 1.0 + self.threshold_db / 80.0, -80.0, 0.0, "dB"),
            FxParam::new("Ratio", (self.ratio - 1.0) / 9.0, 1.0, 10.0, ":1"),
            FxParam::new("Attack", (self.attack_ms - 0.1) / 49.9, 0.1, 50.0, "ms"),
            FxParam::new(
                "Release",
                (self.release_ms - 10.0) / 990.0,
                10.0,
                1000.0,
                "ms",
            ),
            FxParam::new("Range", self.range_db / 80.0, 0.0, 80.0, "dB"),
        ]
    }

    /// Live, so a knob turn does not rebuild the chain — which would reset
    /// every other effect's tail along with this one's envelope.
    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => self.threshold_db = -(1.0 - v) * 80.0,
            1 => self.ratio = 1.0 + v * 9.0,
            2 => self.attack_ms = 0.1 + v * 49.9,
            3 => self.release_ms = 10.0 + v * 990.0,
            4 => self.range_db = v * 80.0,
            _ => {}
        }
    }

    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        if self.ratio == 1.0 {
            return;
        }
        let sr = sample_rate as f32;
        let attack_coef = (-1.0 / (self.attack_ms * 0.001 * sr)).exp();
        let release_coef = (-1.0 / (self.release_ms * 0.001 * sr)).exp();
        let thresh_lin = 10f32.powf(self.threshold_db / 20.0);

        for chunk in buf.chunks_mut(2) {
            if chunk.len() < 2 {
                break;
            }
            let peak = chunk[0].abs().max(chunk[1].abs());

            // Envelope follower.
            if peak > self.env {
                self.env = attack_coef * self.env + (1.0 - attack_coef) * peak;
            } else {
                self.env = release_coef * self.env + (1.0 - release_coef) * peak;
            }

            // Gain computation: below threshold, apply ratio-based expansion.
            let target_gain_db = if self.env < thresh_lin && self.env > 1e-10 {
                let over_db = 20.0 * (self.env / thresh_lin).log10(); // negative below threshold
                                                                      // Downward: ratio > 1 → amplify |over_db|, moving gain more negative.
                                                                      // Upward:   ratio < 1 → compress |over_db|, moving gain closer to zero.
                let reduced = over_db * (self.ratio - 1.0);
                if self.range_db > 0.0 {
                    reduced.clamp(-self.range_db, self.range_db)
                } else {
                    reduced
                }
            } else {
                0.0
            };

            // Smooth gain changes with release coefficient.
            let coef = if target_gain_db < self.gain_db {
                attack_coef
            } else {
                release_coef
            };
            self.gain_db = coef * self.gain_db + (1.0 - coef) * target_gain_db;

            let gain = 10f32.powf(self.gain_db / 20.0);
            for s in chunk.iter_mut() {
                *s = *s * gain * self.mix + *s * (1.0 - self.mix);
            }
        }
    }

    fn reset(&mut self) {
        self.env = 0.0;
        self.gain_db = 0.0;
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
    fn expander_downward_attenuates_below_threshold() {
        let mut exp = Expander::new();
        exp.threshold_db = -20.0;
        exp.ratio = 4.0;
        exp.attack_ms = 0.1;
        exp.release_ms = 10.0;
        exp.range_db = 40.0;
        // Signal below threshold (-40 dBFS ≈ 0.01 linear).
        let mut buf = vec![0.01f32; 1024];
        let before = buf[0];
        exp.process_block(&mut buf, 48000);
        let after = buf[1020].abs();
        assert!(
            after < before * 0.9,
            "expander should attenuate below threshold, before={before}, after={after}"
        );
    }

    #[test]
    fn expander_bypass_at_ratio_one() {
        let mut exp = Expander::new();
        exp.ratio = 1.0;
        let mut buf: Vec<f32> = (0..256).map(|i| (i as f32 * 0.05).sin() * 0.1).collect();
        let before = buf.clone();
        exp.process_block(&mut buf, 48000);
        for (b, a) in before.iter().zip(buf.iter()) {
            assert!((b - a).abs() < 1e-9, "ratio=1 should be bypass");
        }
    }
}
