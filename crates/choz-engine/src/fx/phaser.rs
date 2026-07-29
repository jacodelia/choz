/// All-pass chain phaser (2–8 stages per side).
///
/// Each stage is a first-order all-pass filter. An LFO sweeps the
/// notch frequencies. Adjacent notches create the characteristic
/// "swoosh" as they pass through the audible spectrum.
const MAX_STAGES: usize = 8;

pub struct Phaser {
    pub rate:     f32, // LFO Hz
    pub depth:    f32, // frequency range (0.0..1.0)
    pub center:   f32, // center frequency (200–2000 Hz)
    pub feedback: f32, // -0.9..0.9
    pub stages:   usize, // 2|4|6|8
    mix: f32,
    lfo_phase: f32,
    // All-pass filter states: [stage][L/R]
    ap_l: [f32; MAX_STAGES],
    ap_r: [f32; MAX_STAGES],
    fb_l: f32,
    fb_r: f32,
}

impl Phaser {
    pub fn new() -> Self {
        Self {
            rate:     0.4,
            depth:    0.7,
            center:   800.0,
            feedback: 0.5,
            stages:   4,
            mix: 0.7,
            lfo_phase: 0.0,
            ap_l: [0.0; MAX_STAGES],
            ap_r: [0.0; MAX_STAGES],
            fb_l: 0.0,
            fb_r: 0.0,
        }
    }

}

impl Default for Phaser { fn default() -> Self { Self::new() } }

impl super::FxProcessor for Phaser {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        if buf.len() < 2 { return; }
        let sr = sample_rate as f32;
        let lfo_inc = self.rate / sr;
        let stages = self.stages.min(MAX_STAGES);

        let frames = buf.len() / 2;
        use std::f32::consts::{TAU, PI};

        for i in 0..frames {
            let lfo = (self.lfo_phase * TAU).sin() * 0.5 + 0.5;
            self.lfo_phase = (self.lfo_phase + lfo_inc) % 1.0;

            // Sweep frequency
            let freq = self.center * (0.1 + self.depth * lfo * 9.9);
            let freq_clamped = freq.clamp(20.0, sr * 0.45);
            // All-pass coefficient: a = (tan(π*f/sr) - 1) / (tan(π*f/sr) + 1)
            let t = (PI * freq_clamped / sr).tan();
            let coeff = (t - 1.0) / (t + 1.0);

            let in_l = buf[i * 2]     + self.feedback * self.fb_l;
            let in_r = buf[i * 2 + 1] + self.feedback * self.fb_r;

            // Chain all-pass stages
            let mut sig_l = in_l;
            let mut sig_r = in_r;
            for s in 0..stages {
                // Direct form I all-pass: y = coeff * (x - y_prev) + x_prev
                let y_l = coeff * sig_l + self.ap_l[s];
                self.ap_l[s] = sig_l - coeff * y_l;
                sig_l = y_l;

                let y_r = coeff * sig_r + self.ap_r[s];
                self.ap_r[s] = sig_r - coeff * y_r;
                sig_r = y_r;
            }

            self.fb_l = sig_l;
            self.fb_r = sig_r;

            let orig_l = buf[i * 2];
            let orig_r = buf[i * 2 + 1];
            // Classic phaser: sum of dry + phase-shifted (notch interference)
            buf[i * 2]     = orig_l + self.mix * (sig_l - orig_l);
            buf[i * 2 + 1] = orig_r + self.mix * (sig_r - orig_r);
        }
    }

    fn reset(&mut self) {
        self.ap_l = [0.0; MAX_STAGES];
        self.ap_r = [0.0; MAX_STAGES];
        self.fb_l = 0.0;
        self.fb_r = 0.0;
        self.lfo_phase = 0.0;
    }

    fn set_mix(&mut self, wet: f32) { self.mix = wet.clamp(0.0, 1.0); }

    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => self.rate     = 0.05 + v * 4.95,
            1 => self.depth    = v,
            2 => self.center   = 200.0 + v * 1800.0,
            3 => self.feedback = (v - 0.5) * 1.8,
            4 => self.mix      = v,
            _ => {}
        }
    }
}
