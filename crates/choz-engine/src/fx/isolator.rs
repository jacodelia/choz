//! 3-band DJ isolator — bass / mid / treble with ±∞ dB gain per band.
//!
//! Uses 4× cascaded 2nd-order SVF filters at each crossover to achieve
//! ≈48 dB/oct slopes (similar to SP-404 isolator and Pioneer DJM mixers).
//!
//! Band extraction:
//!   bass   = LP @ bass_freq (cascaded 4×)
//!   treble = HP @ treble_freq (cascaded 4×)
//!   mid    = input − bass − treble  (subtraction — works well up to 4th order)

use std::f32::consts::PI;
use super::FxProcessor;

const STAGES: usize = 4;

/// Cascaded 4× 2nd-order SVF lowpass (or highpass via negation), stereo.
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
        let k  = std::f32::consts::SQRT_2; // k=√2 → Butterworth Q=1/√2, no resonant peak
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

/// 3-band isolator.
///
/// `band_gain[0]` = bass, `band_gain[1]` = mid, `band_gain[2]` = treble.
/// Linear gain: 0.0 = kill, 1.0 = unity.
pub struct Isolator {
    bass_lp:    CascadedSvf,
    treble_hp:  CascadedSvf,
    band_gain:  [f32; 3],
    bass_freq:  f32,
    treble_freq: f32,
    wet: f32,
    sample_rate: u32,
}

impl Isolator {
    /// Create an isolator with default crossovers (200 Hz / 3 kHz).
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

impl Default for Isolator {
    fn default() -> Self { Self::new() }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    // At 1000 Hz both the LP@200Hz and HP@3000Hz are deeply attenuated (4th-order slopes
    // mean >60 dB rejection at 5× / 0.33× the cutoff). mid = input − LP − HP ≈ input there,
    // so killing mid is the cleanest way to verify band isolation without phase-leakage artefacts.
    #[test]
    fn mid_kill_reduces_mid_freq_energy() {
        let sr = 48000u32;
        let frames = 8192usize;

        let mut buf_unity: Vec<f32> = (0..frames)
            .flat_map(|i| {
                let s = (2.0 * PI * 1000.0 * i as f32 / sr as f32).sin();
                [s, s]
            })
            .collect();
        let mut buf_kill = buf_unity.clone();

        let mut iso_unity = Isolator::new();
        iso_unity.process_block(&mut buf_unity, sr);

        let mut iso_kill = Isolator::new();
        iso_kill.set_mid_gain(0.0);
        iso_kill.process_block(&mut buf_kill, sr);

        // Skip the first 512 frames (filter transient), compare steady-state energy.
        let skip = 1024; // 512 frames × 2 channels
        let e_unity: f32 = buf_unity[skip..].iter().map(|&s| s * s).sum();
        let e_kill:  f32 = buf_kill[skip..].iter().map(|&s| s * s).sum();
        assert!(
            e_kill < e_unity * 0.05,
            "mid kill should reduce 1kHz energy to <5%, e_kill={e_kill:.4} e_unity={e_unity:.4}"
        );
    }

    #[test]
    fn unity_gains_preserve_energy() {
        let sr = 48000u32;
        // Use 4096 frames so the filter transient is a small fraction of total
        let frames = 4096usize;
        let input: Vec<f32> = (0..frames)
            .flat_map(|i| {
                let s = (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / sr as f32).sin();
                [s, s]
            })
            .collect();
        let mut buf = input.clone();
        let mut iso = Isolator::new();
        iso.process_block(&mut buf, sr);
        // Compare only the steady-state portion (skip first 256 frames of transient)
        let skip = 512;
        let e_in:  f32 = input[skip..].iter().map(|&s| s * s).sum();
        let e_out: f32 = buf[skip..].iter().map(|&s| s * s).sum();
        let ratio = e_out / e_in.max(1e-9);
        assert!((0.7..=1.3).contains(&ratio), "energy ratio={ratio}");
    }
}
