/// Short-delay flanger with LFO modulation and feedback.
const MAX_FLANGER_SAMPLES: usize = 2048; // ~46ms at 44.1kHz

pub struct Flanger {
    pub rate:     f32, // LFO Hz (0.1–5.0)
    pub depth:    f32, // modulation range in ms (0.0–7.0)
    pub delay_ms: f32, // base delay (0.5–10.0 ms)
    pub feedback: f32, // -0.95..0.95
    pub stereo:   bool,
    mix: f32,
    lfo_phase: f32,
    buf_l: Vec<f32>,
    buf_r: Vec<f32>,
    write_pos: usize,
}

impl Flanger {
    pub fn new() -> Self {
        Self {
            rate:     0.3,
            depth:    2.5,
            delay_ms: 3.0,
            feedback: 0.5,
            stereo:   true,
            mix: 0.7,
            lfo_phase: 0.0,
            buf_l: vec![0.0; MAX_FLANGER_SAMPLES],
            buf_r: vec![0.0; MAX_FLANGER_SAMPLES],
            write_pos: 0,
        }
    }

    fn read_interp(buf: &[f32], pos: usize, delay: f32) -> f32 {
        let len = buf.len();
        let d = delay as usize;
        let frac = delay - d as f32;
        let p0 = (pos + len - d    ) % len;
        let p1 = (pos + len - d - 1) % len;
        buf[p0] * (1.0 - frac) + buf[p1] * frac
    }
}

impl Default for Flanger { fn default() -> Self { Self::new() } }

impl super::FxProcessor for Flanger {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        if buf.len() < 2 { return; }
        let sr = sample_rate as f32;
        let lfo_inc = self.rate / sr;
        let depth_s = (self.depth    * sr / 1000.0).clamp(0.0, MAX_FLANGER_SAMPLES as f32 / 2.0);
        let base_s  = (self.delay_ms * sr / 1000.0).clamp(1.0, MAX_FLANGER_SAMPLES as f32 - depth_s - 2.0);

        let frames = buf.len() / 2;
        use std::f32::consts::TAU;

        for i in 0..frames {
            let lfo_l = (self.lfo_phase * TAU).sin();
            let lfo_r = if self.stereo { (self.lfo_phase * TAU + std::f32::consts::FRAC_PI_2).sin() } else { lfo_l };
            self.lfo_phase = (self.lfo_phase + lfo_inc) % 1.0;

            let delay_l = (base_s + depth_s * lfo_l).max(1.0);
            let delay_r = (base_s + depth_s * lfo_r).max(1.0);

            let wet_l = Self::read_interp(&self.buf_l, self.write_pos, delay_l);
            let wet_r = Self::read_interp(&self.buf_r, self.write_pos, delay_r);

            let in_l = buf[i * 2];
            let in_r = buf[i * 2 + 1];

            self.buf_l[self.write_pos] = in_l + self.feedback * wet_l;
            self.buf_r[self.write_pos] = in_r + self.feedback * wet_r;
            self.write_pos = (self.write_pos + 1) % MAX_FLANGER_SAMPLES;

            buf[i * 2]     = in_l + self.mix * (wet_l - in_l);
            buf[i * 2 + 1] = in_r + self.mix * (wet_r - in_r);
        }
    }

    fn reset(&mut self) {
        self.buf_l.fill(0.0);
        self.buf_r.fill(0.0);
        self.write_pos = 0;
        self.lfo_phase = 0.0;
    }

    fn set_mix(&mut self, wet: f32) { self.mix = wet.clamp(0.0, 1.0); }

    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => self.rate     = 0.05 + v * 4.95,
            1 => self.depth    = v * 7.0,
            2 => self.delay_ms = 0.5 + v * 9.5,
            3 => self.feedback = (v - 0.5) * 1.9,
            4 => self.mix      = v,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::FxProcessor;

    #[test]
    fn flanger_output_differs_from_dry() {
        let mut fl = Flanger::new();
        fl.set_mix(1.0);
        fl.feedback = 0.5;
        let dry: Vec<f32> = (0..256).map(|i| (i as f32 * 0.04).sin() * 0.5).collect();
        let mut buf = dry.clone();
        fl.process_block(&mut buf, 48000);
        let max_diff = dry.iter().zip(buf.iter()).map(|(d, w)| (d - w).abs()).fold(0.0f32, f32::max);
        assert!(max_diff > 0.001, "flanger output should differ from dry, max_diff={}", max_diff);
    }

    #[test]
    fn flanger_at_zero_mix_is_passthrough() {
        let mut fl = Flanger::new();
        fl.set_mix(0.0);
        let dry: Vec<f32> = (0..256).map(|i| (i as f32 * 0.04).sin() * 0.5).collect();
        let mut buf = dry.clone();
        fl.process_block(&mut buf, 48000);
        for (d, w) in dry.iter().zip(buf.iter()) {
            assert!((d - w).abs() < 1e-6, "at mix=0 output should equal input");
        }
    }
}
