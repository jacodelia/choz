//! Stereo panning effect.
//!
//! `pan` ranges from -1.0 (full left) through 0.0 (center) to +1.0 (full right).
//! Two pan laws available:
//! - Linear: simple L/R level scaling.
//! - ConstantPower: trigonometric (maintains perceived loudness).

pub struct Pan {
    /// -1.0 (full left) … 0.0 (center) … +1.0 (full right).
    pub pan: f32,
    /// Use constant-power panning (true) or linear (false).
    pub constant_power: bool,
    mix: f32,
}

impl Pan {
    pub fn new() -> Self {
        Self {
            pan: 0.0,
            constant_power: true,
            mix: 1.0,
        }
    }

    fn gains(&self) -> (f32, f32) {
        let p = self.pan.clamp(-1.0, 1.0);
        if self.constant_power {
            // Constant-power: angle 0..π/2 mapped to pan -1..+1
            let angle = (p + 1.0) * std::f32::consts::FRAC_PI_4;
            (angle.cos(), angle.sin())
        } else {
            // Linear: L = 1 - p/2, R = 1 + p/2 (summed to 2 at extreme, 1 at center)
            ((1.0 - p) * 0.5, (1.0 + p) * 0.5)
        }
    }
}

impl Default for Pan {
    fn default() -> Self {
        Self::new()
    }
}

impl super::FxProcessor for Pan {
    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => self.pan = (v - 0.5) * 2.0,
            1 => self.constant_power = v > 0.5,
            _ => {}
        }
    }

    fn process_block(&mut self, buf: &mut [f32], _sample_rate: u32) {
        if self.pan == 0.0 {
            return;
        }
        let (gl, gr) = self.gains();
        for chunk in buf.chunks_mut(2) {
            if chunk.len() < 2 {
                break;
            }
            let dry_l = chunk[0];
            let dry_r = chunk[1];
            chunk[0] = dry_l * gl * self.mix + dry_l * (1.0 - self.mix);
            chunk[1] = dry_r * gr * self.mix + dry_r * (1.0 - self.mix);
        }
    }

    fn reset(&mut self) {}

    fn set_mix(&mut self, wet: f32) {
        self.mix = wet.clamp(0.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::FxProcessor;

    #[test]
    fn pan_center_is_unity() {
        let mut p = Pan::new();
        p.pan = 0.0;
        let mut buf = vec![0.5f32, 0.5, 0.5, 0.5];
        let before = buf.clone();
        p.process_block(&mut buf, 48000);
        for (b, a) in before.iter().zip(buf.iter()) {
            assert!((b - a).abs() < 1e-6, "center pan should be unity");
        }
    }

    #[test]
    fn pan_full_left_silences_right() {
        let mut p = Pan::new();
        p.pan = -1.0;
        p.constant_power = false;
        let mut buf = vec![1.0f32, 1.0, 1.0, 1.0];
        p.process_block(&mut buf, 48000);
        // Right channel should be near zero
        assert!(
            buf[1].abs() < 0.01,
            "full-left pan should silence R, got {}",
            buf[1]
        );
        // Left channel should be non-zero
        assert!(buf[0] > 0.5, "full-left pan should preserve L");
    }

    #[test]
    fn pan_full_right_silences_left() {
        let mut p = Pan::new();
        p.pan = 1.0;
        p.constant_power = false;
        let mut buf = vec![1.0f32, 1.0, 1.0, 1.0];
        p.process_block(&mut buf, 48000);
        assert!(
            buf[0].abs() < 0.01,
            "full-right pan should silence L, got {}",
            buf[0]
        );
        assert!(buf[1] > 0.5, "full-right pan should preserve R");
    }
}
