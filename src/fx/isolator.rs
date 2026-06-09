use std::f32::consts::PI;
use super::FxProcessor;

const STAGES: usize = 4;

struct CascadedSvf {
    ic1eq: [[f32; 2]; STAGES],
    ic2eq: [[f32; 2]; STAGES],
    a1: f32,
    a2: f32,
    a3: f32,
    k:  f32,
    mode: SvfPole,
}

#[derive(Clone, Copy)]
enum SvfPole { Lp, Hp }

#[allow(dead_code)]
impl CascadedSvf {
    fn new(mode: SvfPole) -> Self {
        Self {
            ic1eq: [[0.0; 2]; STAGES],
            ic2eq: [[0.0; 2]; STAGES],
            a1: 0.0, a2: 0.0, a3: 0.0, k: 1.0,
            mode,
        }
    }

    fn set_cutoff(&mut self, hz: f32, sr: f32) {
        let g  = (PI * hz / sr).tan();
        let k  = std::f32::consts::SQRT_2;
        self.a1 = 1.0 / (1.0 + g * (g + k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
        self.k  = k;
    }

    #[inline]
    fn process(&mut self, ch: usize, mut x: f32) -> f32 {
        for s in 0..STAGES {
            let v3 = x - self.ic2eq[s][ch];
            let v1 = self.a1 * self.ic1eq[s][ch] + self.a2 * v3;
            let v2 = self.ic2eq[s][ch] + self.a2 * self.ic1eq[s][ch] + self.a3 * v3;
            self.ic1eq[s][ch] = 2.0 * v1 - self.ic1eq[s][ch];
            self.ic2eq[s][ch] = 2.0 * v2 - self.ic2eq[s][ch];
            x = match self.mode {
                SvfPole::Lp => v2,
                SvfPole::Hp => x - self.k * v1 - v2,
            };
        }
        x
    }

    fn reset(&mut self) {
        self.ic1eq = [[0.0; 2]; STAGES];
        self.ic2eq = [[0.0; 2]; STAGES];
    }
}

pub struct Isolator {
    bass_lp:    CascadedSvf,
    treble_hp:  CascadedSvf,
    band_gain:  [f32; 3],
    bass_freq:  f32,
    treble_freq: f32,
    wet: f32,
    sample_rate: u32,
}

#[allow(dead_code)]
impl Isolator {
    pub fn new() -> Self {
        let mut iso = Self {
            bass_lp:    CascadedSvf::new(SvfPole::Lp),
            treble_hp:  CascadedSvf::new(SvfPole::Hp),
            band_gain:  [1.0; 3],
            bass_freq:  200.0,
            treble_freq: 3000.0,
            wet: 1.0,
            sample_rate: 48000,
        };
        iso.update_filters();
        iso
    }

    pub fn set_bass_gain(&mut self, g: f32)   { self.band_gain[0] = g.max(0.0); }
    pub fn set_mid_gain(&mut self, g: f32)    { self.band_gain[1] = g.max(0.0); }
    pub fn set_treble_gain(&mut self, g: f32) { self.band_gain[2] = g.max(0.0); }
    pub fn set_gains(&mut self, bass: f32, mid: f32, treble: f32) {
        self.band_gain = [bass.max(0.0), mid.max(0.0), treble.max(0.0)];
    }

    pub fn set_bass_freq(&mut self, hz: f32) {
        self.bass_freq = hz.clamp(20.0, 500.0);
        self.update_filters();
    }

    pub fn set_treble_freq(&mut self, hz: f32) {
        self.treble_freq = hz.clamp(500.0, 12000.0);
        self.update_filters();
    }

    fn update_filters(&mut self) {
        let sr = self.sample_rate as f32;
        self.bass_lp.set_cutoff(self.bass_freq, sr);
        self.treble_hp.set_cutoff(self.treble_freq, sr);
    }
}

impl Default for Isolator { fn default() -> Self { Self::new() } }

impl FxProcessor for Isolator {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        if self.sample_rate != sample_rate {
            self.sample_rate = sample_rate;
            self.update_filters();
        }
        let [gb, gm, gt] = self.band_gain;
        let frames = buf.len() / 2;
        for i in 0..frames {
            let dry_l = buf[i * 2];
            let dry_r = buf[i * 2 + 1];

            let bass_l = self.bass_lp.process(0, dry_l);
            let bass_r = self.bass_lp.process(1, dry_r);
            let treb_l = self.treble_hp.process(0, dry_l);
            let treb_r = self.treble_hp.process(1, dry_r);
            let mid_l  = dry_l - bass_l - treb_l;
            let mid_r  = dry_r - bass_r - treb_r;

            let wet_l = bass_l * gb + mid_l * gm + treb_l * gt;
            let wet_r = bass_r * gb + mid_r * gm + treb_r * gt;

            buf[i * 2]     = dry_l + self.wet * (wet_l - dry_l);
            buf[i * 2 + 1] = dry_r + self.wet * (wet_r - dry_r);
        }
    }

    fn reset(&mut self) {
        self.bass_lp.reset();
        self.treble_hp.reset();
    }

    fn set_mix(&mut self, wet: f32) {
        self.wet = wet.clamp(0.0, 1.0);
    }
}
