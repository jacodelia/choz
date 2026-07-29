/// Stereo widener using M/S (mid/side) processing.
///
/// `width = 0.0` → mono (sides silenced)
/// `width = 1.0` → unchanged (unity)
/// `width = 2.0` → doubled stereo width
pub struct StereoWidener {
    pub width: f32, // 0.0..2.0
    mix: f32,
}

impl StereoWidener {
    pub fn new() -> Self {
        Self { width: 1.0, mix: 1.0 }
    }
}

impl Default for StereoWidener { fn default() -> Self { Self::new() } }

impl super::FxProcessor for StereoWidener {
    fn process_block(&mut self, buf: &mut [f32], _sample_rate: u32) {
        if buf.len() < 2 { return; }
        // width = 1.0 → mid_gain = 1.0, side_gain = 1.0
        // width = 0.0 → mono: mid = 1.0, side = 0.0
        // width = 2.0 → enhanced: mid = 1.0, side = 2.0
        let side_gain = self.width.clamp(0.0, 2.0);
        let frames = buf.len() / 2;
        for i in 0..frames {
            let l = buf[i * 2];
            let r = buf[i * 2 + 1];
            let mid  = (l + r) * 0.5;
            let side = (l - r) * 0.5;
            let wet_l = mid + side * side_gain;
            let wet_r = mid - side * side_gain;
            buf[i * 2]     = l + self.mix * (wet_l - l);
            buf[i * 2 + 1] = r + self.mix * (wet_r - r);
        }
    }

    fn reset(&mut self) {}
    fn set_mix(&mut self, wet: f32) { self.mix = wet.clamp(0.0, 1.0); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::FxProcessor;

    #[test]
    fn width_zero_produces_mono() {
        let mut w = StereoWidener::new();
        w.width = 0.0;
        let mut buf = vec![0.0f32; 8];
        buf[0] = 1.0; buf[1] = 0.0; // L=1, R=0
        buf[2] = 1.0; buf[3] = 0.0;
        w.process_block(&mut buf, 48000);
        // Both channels should equal 0.5 (mono sum).
        assert!((buf[0] - 0.5).abs() < 1e-5, "L should be 0.5, got {}", buf[0]);
        assert!((buf[1] - 0.5).abs() < 1e-5, "R should be 0.5, got {}", buf[1]);
    }

    #[test]
    fn width_one_is_unity() {
        let mut w = StereoWidener::new();
        w.width = 1.0;
        let mut buf = vec![0.0f32; 8];
        buf[0] = 0.8; buf[1] = 0.3;
        buf[2] = 0.8; buf[3] = 0.3;
        let orig_l = buf[0];
        let orig_r = buf[1];
        w.process_block(&mut buf, 48000);
        assert!((buf[0] - orig_l).abs() < 1e-5, "width=1 should not change L");
        assert!((buf[1] - orig_r).abs() < 1e-5, "width=1 should not change R");
    }
}
