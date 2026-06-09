pub struct Pan {
    pub pan: f32,
    pub constant_power: bool,
    mix: f32,
}

impl Pan {
    pub fn new() -> Self {
        Self { pan: 0.0, constant_power: true, mix: 1.0 }
    }

    fn gains(&self) -> (f32, f32) {
        let p = self.pan.clamp(-1.0, 1.0);
        if self.constant_power {
            let angle = (p + 1.0) * std::f32::consts::FRAC_PI_4;
            (angle.cos(), angle.sin())
        } else {
            ((1.0 - p) * 0.5, (1.0 + p) * 0.5)
        }
    }
}

impl Default for Pan { fn default() -> Self { Self::new() } }

impl super::FxProcessor for Pan {
    fn process_block(&mut self, buf: &mut [f32], _sample_rate: u32) {
        if self.pan == 0.0 { return; }
        let (gl, gr) = self.gains();
        for chunk in buf.chunks_mut(2) {
            if chunk.len() < 2 { break; }
            let dry_l = chunk[0];
            let dry_r = chunk[1];
            chunk[0] = dry_l * gl * self.mix + dry_l * (1.0 - self.mix);
            chunk[1] = dry_r * gr * self.mix + dry_r * (1.0 - self.mix);
        }
    }

    fn reset(&mut self) {}
    fn set_mix(&mut self, wet: f32) { self.mix = wet.clamp(0.0, 1.0); }
}
