pub struct StereoWidener {
    pub width: f32,
    mix: f32,
}

impl StereoWidener {
    pub fn new() -> Self { Self { width: 1.0, mix: 1.0 } }
}

impl Default for StereoWidener { fn default() -> Self { Self::new() } }

impl super::FxProcessor for StereoWidener {
    fn process_block(&mut self, buf: &mut [f32], _sample_rate: u32) {
        if buf.len() < 2 { return; }
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
