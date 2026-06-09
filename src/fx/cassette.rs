use super::FxProcessor;

pub struct Cassette {
    drive: f32,
    emphasis_hz: f32,
    noise_amp: f32,
    wet: f32,
    pre_lp: [f32; 2],
    pre_alpha: f32,
    de_lp: [f32; 2],
    de_alpha: f32,
    rng: u32,
    sample_rate: u32,
}

#[allow(dead_code)]
impl Cassette {
    pub fn new() -> Self {
        let mut c = Self {
            drive: 2.0,
            emphasis_hz: 4000.0,
            noise_amp: 0.001,
            wet: 1.0,
            pre_lp: [0.0; 2],
            pre_alpha: 0.0,
            de_lp: [0.0; 2],
            de_alpha: 0.0,
            rng: 0xCAFE_BABE,
            sample_rate: 48000,
        };
        c.update_coeffs();
        c
    }

    pub fn set_drive(&mut self, d: f32) { self.drive = d.clamp(0.5, 8.0); }
    pub fn set_noise(&mut self, amp: f32) { self.noise_amp = amp.clamp(0.0, 0.1); }
    pub fn set_emphasis_hz(&mut self, hz: f32) {
        self.emphasis_hz = hz.clamp(500.0, 15000.0);
        self.update_coeffs();
    }

    fn update_coeffs(&mut self) {
        let rc  = 1.0 / (2.0 * std::f32::consts::PI * self.emphasis_hz);
        let dt  = 1.0 / self.sample_rate as f32;
        let alpha = (-dt / rc).exp();
        self.pre_alpha = alpha;
        self.de_alpha  = alpha;
    }

    #[inline]
    fn rand_f32(&mut self) -> f32 {
        self.rng = self.rng.wrapping_mul(1664525).wrapping_add(1013904223);
        (self.rng >> 8) as f32 / (1u32 << 24) as f32 - 0.5
    }

    #[inline]
    fn process_sample(&mut self, ch: usize, x: f32) -> f32 {
        self.pre_lp[ch] = self.pre_alpha * self.pre_lp[ch] + (1.0 - self.pre_alpha) * x;
        let pre = x + 0.7 * (x - self.pre_lp[ch]);

        let driven = pre * self.drive;
        let sat = driven.tanh();

        self.de_lp[ch] = self.de_alpha * self.de_lp[ch] + (1.0 - self.de_alpha) * sat;
        let de = self.de_lp[ch];

        let noise = if ch == 0 { self.rand_f32() * self.noise_amp } else { 0.0 };

        de + noise
    }
}

impl Default for Cassette { fn default() -> Self { Self::new() } }

impl FxProcessor for Cassette {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        if self.sample_rate != sample_rate {
            self.sample_rate = sample_rate;
            self.update_coeffs();
        }
        let frames = buf.len() / 2;
        for i in 0..frames {
            let dry_l = buf[i * 2];
            let dry_r = buf[i * 2 + 1];
            let wet_l = self.process_sample(0, dry_l);
            let wet_r = self.process_sample(1, dry_r);
            buf[i * 2]     = dry_l + self.wet * (wet_l - dry_l);
            buf[i * 2 + 1] = dry_r + self.wet * (wet_r - dry_r);
        }
    }

    fn reset(&mut self) {
        self.pre_lp = [0.0; 2];
        self.de_lp  = [0.0; 2];
    }

    fn set_mix(&mut self, wet: f32) {
        self.wet = wet.clamp(0.0, 1.0);
    }
}
