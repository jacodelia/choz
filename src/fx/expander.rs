pub struct Expander {
    pub threshold_db: f32,
    pub ratio:        f32,
    pub attack_ms:    f32,
    pub release_ms:   f32,
    pub range_db:     f32,
    mix:              f32,
    env:              f32,
    gain_db:          f32,
}

impl Expander {
    pub fn new() -> Self {
        Self {
            threshold_db: -40.0,
            ratio:        2.0,
            attack_ms:    10.0,
            release_ms:   100.0,
            range_db:     60.0,
            mix:          1.0,
            env:          0.0,
            gain_db:      0.0,
        }
    }
}

impl Default for Expander { fn default() -> Self { Self::new() } }

impl super::FxProcessor for Expander {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        if self.ratio == 1.0 { return; }
        let sr = sample_rate as f32;
        let attack_coef  = (-1.0 / (self.attack_ms  * 0.001 * sr)).exp();
        let release_coef = (-1.0 / (self.release_ms * 0.001 * sr)).exp();
        let thresh_lin = 10f32.powf(self.threshold_db / 20.0);

        for chunk in buf.chunks_mut(2) {
            if chunk.len() < 2 { break; }
            let peak = chunk[0].abs().max(chunk[1].abs());

            if peak > self.env {
                self.env = attack_coef  * self.env + (1.0 - attack_coef)  * peak;
            } else {
                self.env = release_coef * self.env + (1.0 - release_coef) * peak;
            }

            let target_gain_db = if self.env < thresh_lin && self.env > 1e-10 {
                let over_db = 20.0 * (self.env / thresh_lin).log10();
                let reduced = over_db * (self.ratio - 1.0);
                if self.range_db > 0.0 {
                    reduced.clamp(-self.range_db, self.range_db)
                } else {
                    reduced
                }
            } else {
                0.0
            };

            let coef = if target_gain_db < self.gain_db { attack_coef } else { release_coef };
            self.gain_db = coef * self.gain_db + (1.0 - coef) * target_gain_db;

            let gain = 10f32.powf(self.gain_db / 20.0);
            for s in chunk.iter_mut() {
                *s = *s * gain * self.mix + *s * (1.0 - self.mix);
            }
        }
    }

    fn reset(&mut self) {
        self.env     = 0.0;
        self.gain_db = 0.0;
    }

    fn set_mix(&mut self, wet: f32) { self.mix = wet.clamp(0.0, 1.0); }
}
