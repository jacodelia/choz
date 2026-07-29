/// Classic stereo chorus using two LFO-modulated delay lines.
///
/// L channel is modulated at `rate` Hz; R is phase-shifted by π for width.
const MAX_DELAY_SAMPLES: usize = 4096;

pub struct Chorus {
    pub rate:     f32, // LFO Hz (0.1–5.0)
    pub depth:    f32, // modulation depth in ms (0.5–10.0)
    pub delay_ms: f32, // base delay (5.0–30.0 ms)
    pub feedback: f32, // feedback level (-0.9..0.9)
    mix: f32,
    // state
    lfo_phase: f32,
    buf_l: Vec<f32>,
    buf_r: Vec<f32>,
    write_pos: usize,
}

impl Chorus {
    pub fn new() -> Self {
        Self {
            rate:     0.5,
            depth:    3.0,
            delay_ms: 15.0,
            feedback: 0.1,
            mix: 0.5,
            lfo_phase: 0.0,
            buf_l: vec![0.0; MAX_DELAY_SAMPLES],
            buf_r: vec![0.0; MAX_DELAY_SAMPLES],
            write_pos: 0,
        }
    }

    fn read_interpolated(buf: &[f32], pos: usize, frac_delay: f32) -> f32 {
        let len = buf.len();
        let d = frac_delay as usize;
        let frac = frac_delay - d as f32;
        let p0 = (pos + len - d) % len;
        let p1 = (pos + len - d - 1) % len;
        buf[p0] * (1.0 - frac) + buf[p1] * frac
    }
}

impl Default for Chorus { fn default() -> Self { Self::new() } }

impl super::FxProcessor for Chorus {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        if buf.len() < 2 { return; }
        let sr = sample_rate as f32;
        let lfo_inc   = self.rate / sr;
        let depth_s   = (self.depth    * sr / 1000.0).clamp(1.0, MAX_DELAY_SAMPLES as f32 / 2.0);
        let base_s    = (self.delay_ms * sr / 1000.0).clamp(1.0, MAX_DELAY_SAMPLES as f32 - depth_s - 2.0);

        let frames = buf.len() / 2;
        use std::f32::consts::TAU;

        for i in 0..frames {
            let lfo_l = (self.lfo_phase * TAU).sin();
            let lfo_r = ((self.lfo_phase + 0.5) * TAU).sin(); // π phase offset
            self.lfo_phase = (self.lfo_phase + lfo_inc) % 1.0;

            let delay_l = base_s + depth_s * lfo_l;
            let delay_r = base_s + depth_s * lfo_r;

            let wet_l = Self::read_interpolated(&self.buf_l, self.write_pos, delay_l);
            let wet_r = Self::read_interpolated(&self.buf_r, self.write_pos, delay_r);

            let in_l = buf[i * 2];
            let in_r = buf[i * 2 + 1];

            self.buf_l[self.write_pos] = in_l + self.feedback * wet_l;
            self.buf_r[self.write_pos] = in_r + self.feedback * wet_r;
            self.write_pos = (self.write_pos + 1) % MAX_DELAY_SAMPLES;

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
    fn name(&self) -> &str { "Chorus" }

    fn params(&self) -> Vec<crate::fx::FxParam> {
        use crate::fx::FxParam;
        vec![
            FxParam::new("Rate",     (self.rate / 5.0).clamp(0.0, 1.0), 0.0, 5.0, "Hz"),
            FxParam::new("Depth",    (self.depth / 10.0).clamp(0.0, 1.0), 0.0, 10.0, "ms"),
            FxParam::new("Delay",    ((self.delay_ms - 5.0) / 25.0).clamp(0.0, 1.0), 5.0, 30.0, "ms"),
            FxParam::new("Feedback", (self.feedback + 0.9) / 1.8, -0.9, 0.9, ""),
            FxParam::new("Wet",      self.mix, 0.0, 1.0, ""),
        ]
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => self.rate     = 0.05 + v * 4.95,
            1 => self.depth    = 0.5  + v * 9.5,
            2 => self.delay_ms = 5.0 + v * 25.0,
            3 => self.feedback = -0.9 + v * 1.8,
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
    fn chorus_output_differs_from_dry() {
        let mut ch = Chorus::new();
        ch.set_mix(1.0);
        let dry: Vec<f32> = (0..256).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        let mut buf = dry.clone();
        ch.process_block(&mut buf, 48000);
        // After processing, at least some samples should differ from dry (modulated delay).
        let max_diff = dry.iter().zip(buf.iter()).map(|(d, w)| (d - w).abs()).fold(0.0f32, f32::max);
        assert!(max_diff > 0.001, "chorus output should differ from dry input, max_diff={}", max_diff);
    }

    #[test]
    fn chorus_at_zero_mix_is_passthrough() {
        let mut ch = Chorus::new();
        ch.set_mix(0.0);
        let dry: Vec<f32> = (0..256).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        let mut buf = dry.clone();
        ch.process_block(&mut buf, 48000);
        for (d, w) in dry.iter().zip(buf.iter()) {
            assert!((d - w).abs() < 1e-6, "at mix=0 output should equal input");
        }
    }
}
