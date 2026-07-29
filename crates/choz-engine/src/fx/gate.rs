/// Noise gate with hold phase and optional expander mode.
///
/// When `ratio = 1.0` the processor is a hard gate (below threshold = silence).
/// Values between 1.0 and ∞ act as a downward expander.
pub struct Gate {
    pub threshold_db: f32,
    pub attack_ms: f32,
    pub hold_ms: f32,
    pub release_ms: f32,
    /// Floor in dB when gate is fully closed (0 = silence, -80 = very quiet).
    pub floor_db: f32,
    mix: f32,
    // RT state
    envelope: f32,
    gain: f32,
    hold_counter: usize,
    is_open: bool,
}

impl Gate {
    pub fn new() -> Self {
        Self {
            threshold_db: -40.0,
            attack_ms: 1.0,
            hold_ms: 50.0,
            release_ms: 200.0,
            floor_db: -80.0,
            mix: 1.0,
            envelope: 0.0,
            gain: 0.0,
            hold_counter: 0,
            is_open: false,
        }
    }
}

impl Default for Gate {
    fn default() -> Self { Self::new() }
}

impl super::FxProcessor for Gate {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        if buf.len() < 2 { return; }
        let sr = sample_rate as f32;
        let attack_coeff  = (-1.0 / (self.attack_ms  * 0.001 * sr)).exp();
        let release_coeff = (-1.0 / (self.release_ms * 0.001 * sr)).exp();
        let hold_samples  = (self.hold_ms * 0.001 * sr) as usize;
        let thr_linear    = 10.0f32.powf(self.threshold_db / 20.0);
        let floor_linear  = 10.0f32.powf(self.floor_db / 20.0);
        let target_open   = 1.0f32;
        let target_closed = floor_linear;

        let frames = buf.len() / 2;
        for i in 0..frames {
            let l = buf[i * 2];
            let r = buf[i * 2 + 1];

            // Fast peak envelope
            let peak = l.abs().max(r.abs());
            if peak > self.envelope {
                self.envelope = peak;
            } else {
                self.envelope *= release_coeff;
            }

            // Gate logic
            if self.envelope >= thr_linear {
                self.is_open = true;
                self.hold_counter = hold_samples;
            } else if self.hold_counter > 0 {
                self.hold_counter -= 1;
            } else {
                self.is_open = false;
            }

            let target = if self.is_open { target_open } else { target_closed };
            if target > self.gain {
                self.gain = attack_coeff  * (self.gain - target) + target;
            } else {
                self.gain = release_coeff * (self.gain - target) + target;
            }

            let wet_l = l * self.gain;
            let wet_r = r * self.gain;
            buf[i * 2]     = l + self.mix * (wet_l - l);
            buf[i * 2 + 1] = r + self.mix * (wet_r - r);
        }
    }

    fn reset(&mut self) {
        self.envelope     = 0.0;
        self.gain         = 0.0;
        self.hold_counter = 0;
        self.is_open      = false;
    }

    fn set_mix(&mut self, wet: f32) { self.mix = wet.clamp(0.0, 1.0); }
    fn name(&self) -> &str { "Gate" }

    fn params(&self) -> Vec<crate::fx::FxParam> {
        use crate::fx::FxParam;
        vec![
            FxParam::new("Threshold", (self.threshold_db + 80.0) / 80.0, -80.0, 0.0, "dB"),
            FxParam::new("Attack",    (self.attack_ms / 200.0).clamp(0.0, 1.0), 0.0, 200.0, "ms"),
            FxParam::new("Hold",      (self.hold_ms / 500.0).clamp(0.0, 1.0), 0.0, 500.0, "ms"),
            FxParam::new("Release",   (self.release_ms / 2000.0).clamp(0.0, 1.0), 0.0, 2000.0, "ms"),
            FxParam::new("Floor",     ((self.floor_db + 80.0) / 80.0).clamp(0.0, 1.0), -80.0, 0.0, "dB"),
            FxParam::new("Wet",       self.mix, 0.0, 1.0, ""),
        ]
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => self.threshold_db = -80.0 + v * 80.0,
            1 => self.attack_ms    = 0.1 + v * 49.9,
            2 => self.hold_ms      = 1.0 + v * 499.0,
            3 => self.release_ms   = 10.0 + v * 990.0,
            4 => self.floor_db     = -80.0 + v * 80.0,
            5 => self.mix          = v,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::FxProcessor;

    #[test]
    fn gate_opens_above_threshold() {
        let mut g = Gate::new();
        g.threshold_db = -40.0;
        g.attack_ms = 0.1;
        g.hold_ms = 0.0;
        g.release_ms = 1.0;
        g.floor_db = -80.0;
        // Hot signal above threshold.
        let mut buf = vec![0.2f32; 1024];
        g.process_block(&mut buf, 48000);
        // Gate should be open → signal mostly passes.
        let last = buf[1020].abs();
        assert!(last > 0.1, "gate should be open above threshold, got {}", last);
    }

    #[test]
    fn gate_closes_below_threshold() {
        let mut g = Gate::new();
        g.threshold_db = -6.0; // high threshold
        g.attack_ms = 0.1;
        g.hold_ms = 1.0;
        g.release_ms = 1.0;
        g.floor_db = -80.0;
        // Quiet signal below threshold.
        let mut buf = vec![0.001f32; 4096];
        g.process_block(&mut buf, 48000);
        let last = buf[4090].abs();
        assert!(last < 0.001, "gate should be closed below threshold, got {}", last);
    }
}
