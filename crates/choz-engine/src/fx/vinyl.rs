//! Vinyl simulation — wow/flutter (slow pitch modulation) + crackle noise.

use super::dc::DcBlock;
use super::FxProcessor;

/// LFO-based wow and flutter + crackle noise, producing an analogue vinyl feel.
pub struct VinylSim {
    /// Wow depth: slow LFO modulation depth (0.0–1.0).
    wow_depth: f32,
    /// Flutter depth: fast LFO modulation depth (0.0–1.0).
    flutter_depth: f32,
    /// Crackle density (0.0 = silent, 1.0 = heavy).
    crackle: f32,
    wet: f32,

    // Internal state
    wow_phase: f32,
    flutter_phase: f32,
    // Simple interpolating delay line for pitch modulation
    delay_buf: Vec<f32>,
    delay_write: usize,
    // LCG random state for crackle
    rng_state: u32,
    // Crackle and a groove are not symmetric around zero; what they leave
    // behind would eat headroom in every effect after this one.
    dc: DcBlock,
    sample_rate: u32,
}

impl VinylSim {
    pub fn new() -> Self {
        Self {
            wow_depth: 0.003,
            flutter_depth: 0.001,
            crackle: 0.05,
            wet: 1.0,
            wow_phase: 0.0,
            flutter_phase: 0.0,
            delay_buf: vec![0.0f32; 4096],
            delay_write: 0,
            rng_state: 0xDEAD_BEEF,
            dc: DcBlock::new(48000.0),
            sample_rate: 48000,
        }
    }

    pub fn set_wow(&mut self, depth: f32) {
        self.wow_depth = depth.clamp(0.0, 0.1);
    }
    pub fn set_flutter(&mut self, depth: f32) {
        self.flutter_depth = depth.clamp(0.0, 0.05);
    }
    pub fn set_crackle(&mut self, amount: f32) {
        self.crackle = amount.clamp(0.0, 1.0);
    }

    fn lcg_next(&mut self) -> f32 {
        self.rng_state = self
            .rng_state
            .wrapping_mul(1664525)
            .wrapping_add(1013904223);
        // Map to [-1, 1]
        (self.rng_state as i32 as f32) / i32::MAX as f32
    }

    fn read_interp(&self, frac_offset: f32) -> f32 {
        let cap = self.delay_buf.len();
        let total = frac_offset.max(0.0);
        let i0 = total as usize;
        let frac = total - i0 as f32;
        let idx0 = if self.delay_write > i0 {
            self.delay_write - i0 - 1
        } else {
            cap - (i0 + 1 - self.delay_write)
        } % cap;
        let idx1 = if idx0 + 1 < cap { idx0 + 1 } else { 0 };
        self.delay_buf[idx0] * (1.0 - frac) + self.delay_buf[idx1] * frac
    }
}

impl Default for VinylSim {
    fn default() -> Self {
        Self::new()
    }
}

impl FxProcessor for VinylSim {
    fn params(&self) -> Vec<crate::fx::FxParam> {
        use crate::fx::FxParam;
        vec![
            FxParam::new("Wow", self.wow_depth / 0.1, 0.0, 0.1, ""),
            FxParam::new("Flutter", self.flutter_depth / 0.05, 0.0, 0.05, ""),
            FxParam::new("Crackle", self.crackle, 0.0, 1.0, ""),
            FxParam::new("Wet", self.wet, 0.0, 1.0, ""),
        ]
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => self.set_wow(v * 0.1),
            1 => self.set_flutter(v * 0.05),
            2 => self.set_crackle(v),
            3 => self.wet = v,
            _ => {}
        }
    }

    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        if self.sample_rate != sample_rate {
            self.sample_rate = sample_rate;
            self.dc = DcBlock::new(sample_rate as f32);
        }
        let sr = sample_rate as f32;
        let wow_rate = 0.5_f32; // Hz — slow wow
        let flutter_rate = 8.0_f32; // Hz — flutter

        let wow_inc = wow_rate / sr;
        let flutter_inc = flutter_rate / sr;

        let max_delay = 512.0_f32; // frames — max modulation delay

        let frames = buf.len() / 2;
        for i in 0..frames {
            let dry_l = buf[i * 2];
            let dry_r = buf[i * 2 + 1];

            // Write into delay buffer (mono mix for wow/flutter)
            let mono = (dry_l + dry_r) * 0.5;
            self.delay_buf[self.delay_write] = mono;
            self.delay_write = (self.delay_write + 1) % self.delay_buf.len();

            // Modulated read offset (wow + flutter)
            let wow_lfo = self.wow_phase.sin();
            let flutter_lfo = self.flutter_phase.sin();
            let offset = max_delay * 0.5
                + max_delay * 0.5 * (self.wow_depth * wow_lfo + self.flutter_depth * flutter_lfo);

            let modulated = self.read_interp(offset);

            // Crackle
            let crack = if self.crackle > 0.0 {
                let r = self.lcg_next();
                if r.abs() > (1.0 - self.crackle) {
                    r * 0.2 * self.crackle
                } else {
                    0.0
                }
            } else {
                0.0
            };

            let wet = self.dc.process(modulated + crack);
            let wet_l = wet;
            let wet_r = wet;
            buf[i * 2] = dry_l + self.wet * (wet_l - dry_l);
            buf[i * 2 + 1] = dry_r + self.wet * (wet_r - dry_r);

            self.wow_phase = (self.wow_phase + wow_inc * std::f32::consts::TAU)
                .rem_euclid(std::f32::consts::TAU);
            self.flutter_phase = (self.flutter_phase + flutter_inc * std::f32::consts::TAU)
                .rem_euclid(std::f32::consts::TAU);
        }
    }

    fn reset(&mut self) {
        self.delay_buf.fill(0.0);
        self.delay_write = 0;
        self.wow_phase = 0.0;
        self.flutter_phase = 0.0;
        self.dc.reset();
    }

    fn set_mix(&mut self, wet: f32) {
        self.wet = wet.clamp(0.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_groove_does_not_leave_an_offset_behind() {
        let sr = 48000u32;
        let mut buf: Vec<f32> = (0..sr as usize)
            .flat_map(|i| {
                let s = (std::f32::consts::TAU * 220.0 * i as f32 / sr as f32)
                    .sin()
                    .max(0.0)
                    * 0.8;
                [s, s]
            })
            .collect();
        let mut fx = VinylSim::new();
        fx.set_crackle(1.0);
        fx.process_block(&mut buf, sr);
        let tail = &buf[buf.len() - 9600..];
        let mean = tail.iter().sum::<f32>() / tail.len() as f32;
        assert!(mean.abs() < 0.01, "DC left on the output: {mean}");
    }
}
