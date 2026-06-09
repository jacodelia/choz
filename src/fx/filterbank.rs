use crate::fx::FxProcessor;

pub const BANDS: usize = 48;
const Q_BANK: f32 = 4.0;

pub struct FilterBankFx {
    gains_db: [f32; BANDS],
    state: [[f32; 4]; BANDS],
    coeffs: [[f32; 5]; BANDS],
    sample_rate: u32,
    mix: f32,
}

#[allow(dead_code)]
impl FilterBankFx {
    pub fn new(sample_rate: u32) -> Self {
        let mut fx = Self {
            gains_db: [0.0; BANDS],
            state:    [[0.0; 4]; BANDS],
            coeffs:   [[0.0; 5]; BANDS],
            sample_rate,
            mix: 1.0,
        };
        fx.recompute_all();
        fx
    }

    pub fn set_band_gain(&mut self, band: usize, gain_db: f32) {
        if band < BANDS {
            self.gains_db[band] = gain_db.clamp(-24.0, 24.0);
            let sr = self.sample_rate;
            self.coeffs[band] = peaking_coeffs(center_freq(band), self.gains_db[band], Q_BANK, sr);
        }
    }

    pub fn set_all_gains(&mut self, gains_db: &[f32]) {
        let sr = self.sample_rate;
        for (b, &g) in gains_db.iter().take(BANDS).enumerate() {
            self.gains_db[b] = g.clamp(-24.0, 24.0);
            self.coeffs[b] = peaking_coeffs(center_freq(b), self.gains_db[b], Q_BANK, sr);
        }
    }

    pub fn band_freq(band: usize) -> f32 { center_freq(band) }

    fn recompute_all(&mut self) {
        let sr = self.sample_rate;
        for b in 0..BANDS {
            self.coeffs[b] = peaking_coeffs(center_freq(b), self.gains_db[b], Q_BANK, sr);
        }
    }
}

#[inline]
fn center_freq(b: usize) -> f32 {
    20.0_f32 * 1000.0_f32.powf(b as f32 / (BANDS - 1) as f32)
}

fn peaking_coeffs(fc: f32, gain_db: f32, q: f32, sr: u32) -> [f32; 5] {
    use std::f32::consts::PI;
    let a = 10.0_f32.powf(gain_db / 40.0);
    let w0 = 2.0 * PI * fc / sr as f32;
    let cos_w = w0.cos();
    let alpha = w0.sin() / (2.0 * q);

    let b0 = 1.0 + alpha * a;
    let b1 = -2.0 * cos_w;
    let b2 = 1.0 - alpha * a;
    let a0 = 1.0 + alpha / a;
    let a1 = -2.0 * cos_w;
    let a2 = 1.0 - alpha / a;

    [b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0]
}

#[inline]
fn biquad(x: f32, z1: f32, z2: f32, c: &[f32; 5]) -> (f32, f32, f32) {
    let y    = c[0] * x + z1;
    let nz1  = c[1] * x - c[3] * y + z2;
    let nz2  = c[2] * x - c[4] * y;
    (y, nz1, nz2)
}

impl FxProcessor for FilterBankFx {
    fn process_block(&mut self, block: &mut [f32], sample_rate: u32) {
        if sample_rate != self.sample_rate {
            self.sample_rate = sample_rate;
            self.recompute_all();
        }

        let frames = block.len() / 2;
        for i in 0..frames {
            let dry_l = block[i * 2];
            let dry_r = block[i * 2 + 1];
            let mut l = dry_l;
            let mut r = dry_r;

            for b in 0..BANDS {
                let c = self.coeffs[b];
                let [z1l, z2l, z1r, z2r] = self.state[b];
                let (yl, nz1l, nz2l) = biquad(l, z1l, z2l, &c);
                let (yr, nz1r, nz2r) = biquad(r, z1r, z2r, &c);
                self.state[b] = [nz1l, nz2l, nz1r, nz2r];
                l = yl;
                r = yr;
            }

            block[i * 2]     = self.mix * l + (1.0 - self.mix) * dry_l;
            block[i * 2 + 1] = self.mix * r + (1.0 - self.mix) * dry_r;
        }
    }

    fn reset(&mut self) {
        self.state = [[0.0; 4]; BANDS];
    }

    fn set_mix(&mut self, wet: f32) {
        self.mix = wet.clamp(0.0, 1.0);
    }
}
