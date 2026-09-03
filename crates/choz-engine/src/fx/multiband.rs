//! Three-band compressor: one set of dynamics per part of the spectrum.
//!
//! A bass note that ducks the whole mix is the reason this exists. One
//! compressor hears everything, so the loudest thing in the room decides what
//! happens to all of it; three of them, behind a crossover, let the low end be
//! held still while the top is left alone.
//!
//! The crossover is Linkwitz-Riley, fourth order: two identical Butterworth
//! sections per side, which cross at −6 dB and sum flat. The low band goes
//! through an all-pass at the upper corner so that it comes back in step with
//! the two that were split there — without it the bands recombine with a phase
//! error and the mix gets a comb through the middle of it.
//!
//! The first version split by subtraction (`high := x − low`), which
//! reconstructs *exactly* but separates badly: measured, an 80 Hz tone left
//! 60% of itself in the mid band with the corner at 200 Hz, so "compress the
//! low band" quietly meant "compress most of the track".

use super::filter::{Svf, SvfMode};
use choz_ports::{FxParam, FxProcessor};

/// One band's dynamics. Peak detection, soft-ish knee, per-channel envelope.
struct Band {
    threshold_db: f32,
    ratio: f32,
    makeup_db: f32,
    env: [f32; 2],
    atk: f32,
    rel: f32,
}

impl Band {
    fn new(threshold_db: f32) -> Self {
        Self {
            threshold_db,
            ratio: 3.0,
            makeup_db: 0.0,
            env: [0.0; 2],
            atk: 0.0,
            rel: 0.0,
        }
    }

    fn refresh(&mut self, sr: f32, attack_ms: f32, release_ms: f32) {
        self.atk = (-1.0 / (attack_ms * 0.001 * sr)).exp();
        self.rel = (-1.0 / (release_ms * 0.001 * sr)).exp();
    }

    /// The gain this band's sample should be multiplied by.
    #[inline]
    fn gain(&mut self, ch: usize, x: f32) -> f32 {
        let level = x.abs();
        let coeff = if level > self.env[ch] {
            self.atk
        } else {
            self.rel
        };
        self.env[ch] = level + coeff * (self.env[ch] - level);
        let db = 20.0 * self.env[ch].max(1e-9).log10();
        let over = db - self.threshold_db;
        let reduction = if over > 0.0 {
            over * (1.0 - 1.0 / self.ratio)
        } else {
            0.0
        };
        10f32.powf((self.makeup_db - reduction) / 20.0)
    }
}

/// The SVF's `resonance` that puts a section at Butterworth, Q = 0.707.
///
/// `k = 2 − 2·resonance` and `Q = 1/k`, so Q = 0.707 is k = √2. At the
/// default 0 the section is critically damped (Q = 0.5) and two of them
/// cascaded are **not** a Linkwitz-Riley pair: measured, the bands came back
/// 2.4 dB down at the corner.
const BUTTERWORTH: f32 = 1.0 - std::f32::consts::SQRT_2 / 2.0;

/// A Linkwitz-Riley section: two Butterworth passes of the same kind.
struct Lr4 {
    a: Svf,
    b: Svf,
}

impl Lr4 {
    fn new(mode: SvfMode, hz: f32) -> Self {
        Self {
            a: Svf::new(mode, hz, BUTTERWORTH),
            b: Svf::new(mode, hz, BUTTERWORTH),
        }
    }

    fn set_cutoff(&mut self, hz: f32) {
        self.a.set_cutoff(hz);
        self.b.set_cutoff(hz);
    }

    fn process(&mut self, buf: &mut [f32], sample_rate: u32) {
        self.a.process_block(buf, sample_rate);
        self.b.process_block(buf, sample_rate);
    }

    fn reset(&mut self) {
        self.a.reset();
        self.b.reset();
    }
}

/// Frames of scratch allocated up front, stereo: `process_block` runs on the
/// audio thread and must not allocate. A host that asks for a bigger block
/// grows these once instead of every time.
const SCRATCH: usize = 8192;

pub struct MultibandCompressor {
    /// The lower corner: everything under it, and everything over it.
    lo_lp: Lr4,
    lo_hp: Lr4,
    /// The upper corner, applied to what came out of the lower one.
    hi_lp: Lr4,
    hi_hp: Lr4,
    /// The low band's phase, matched to the two bands that went through the
    /// upper split.
    low_ap: Lr4,
    low_hz: f32,
    high_hz: f32,
    bands: [Band; 3],
    /// The three bands, kept between blocks rather than built per block.
    split: [Vec<f32>; 3],
    attack_ms: f32,
    release_ms: f32,
    wet: f32,
    sample_rate: f32,
}

impl MultibandCompressor {
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate.max(8000) as f32;
        let mut m = Self {
            lo_lp: Lr4::new(SvfMode::Lowpass, 200.0),
            lo_hp: Lr4::new(SvfMode::Highpass, 200.0),
            hi_lp: Lr4::new(SvfMode::Lowpass, 3000.0),
            hi_hp: Lr4::new(SvfMode::Highpass, 3000.0),
            low_ap: Lr4::new(SvfMode::Allpass, 3000.0),
            low_hz: 200.0,
            high_hz: 3000.0,
            bands: [Band::new(-24.0), Band::new(-18.0), Band::new(-18.0)],
            split: [vec![0.0; SCRATCH], vec![0.0; SCRATCH], vec![0.0; SCRATCH]],
            attack_ms: 10.0,
            release_ms: 120.0,
            wet: 1.0,
            sample_rate: sr,
        };
        m.refresh(sr);
        m
    }

    pub fn with_params(sample_rate: u32, params: &[f32]) -> Self {
        let mut m = Self::new(sample_rate);
        for (i, p) in params.iter().enumerate() {
            <Self as FxProcessor>::set_param(&mut m, i, *p);
        }
        m
    }

    fn refresh(&mut self, sr: f32) {
        self.sample_rate = sr;
        for b in &mut self.bands {
            b.refresh(sr, self.attack_ms, self.release_ms);
        }
    }
}

impl FxProcessor for MultibandCompressor {
    fn name(&self) -> &str {
        "Multiband Comp"
    }

    fn params(&self) -> Vec<FxParam> {
        let thr = |i: usize| (self.bands[i].threshold_db + 60.0) / 60.0;
        let ratio = |i: usize| (self.bands[i].ratio - 1.0) / 19.0;
        vec![
            FxParam::new("LoXover", (self.low_hz - 50.0) / 450.0, 50.0, 500.0, "Hz"),
            FxParam::new(
                "HiXover",
                (self.high_hz - 1000.0) / 9000.0,
                1000.0,
                10000.0,
                "Hz",
            ),
            FxParam::new("Lo Thr", thr(0), -60.0, 0.0, "dB"),
            FxParam::new("Lo Ratio", ratio(0), 1.0, 20.0, ":1"),
            FxParam::new("Mid Thr", thr(1), -60.0, 0.0, "dB"),
            FxParam::new("Mid Ratio", ratio(1), 1.0, 20.0, ":1"),
            FxParam::new("Hi Thr", thr(2), -60.0, 0.0, "dB"),
            FxParam::new("Hi Ratio", ratio(2), 1.0, 20.0, ":1"),
            FxParam::new("Attack", (self.attack_ms - 0.5) / 99.5, 0.5, 100.0, "ms"),
            FxParam::new(
                "Release",
                (self.release_ms - 20.0) / 980.0,
                20.0,
                1000.0,
                "ms",
            ),
            FxParam::new("Makeup", self.bands[0].makeup_db / 24.0, 0.0, 24.0, "dB"),
            FxParam::new("Wet", self.wet, 0.0, 1.0, ""),
        ]
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => {
                self.low_hz = 50.0 + v * 450.0;
                self.lo_lp.set_cutoff(self.low_hz);
                self.lo_hp.set_cutoff(self.low_hz);
            }
            1 => {
                self.high_hz = 1000.0 + v * 9000.0;
                self.hi_lp.set_cutoff(self.high_hz);
                self.hi_hp.set_cutoff(self.high_hz);
                self.low_ap.set_cutoff(self.high_hz);
            }
            2 => self.bands[0].threshold_db = -60.0 + v * 60.0,
            3 => self.bands[0].ratio = 1.0 + v * 19.0,
            4 => self.bands[1].threshold_db = -60.0 + v * 60.0,
            5 => self.bands[1].ratio = 1.0 + v * 19.0,
            6 => self.bands[2].threshold_db = -60.0 + v * 60.0,
            7 => self.bands[2].ratio = 1.0 + v * 19.0,
            8 => {
                self.attack_ms = 0.5 + v * 99.5;
                let sr = self.sample_rate;
                self.refresh(sr);
            }
            9 => {
                self.release_ms = 20.0 + v * 980.0;
                let sr = self.sample_rate;
                self.refresh(sr);
            }
            // One makeup for the whole unit: three of them is a mixer, and the
            // rack already has one.
            10 => {
                for b in &mut self.bands {
                    b.makeup_db = v * 24.0;
                }
            }
            11 => self.wet = v,
            _ => {}
        }
    }

    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        let sr = sample_rate.max(8000) as f32;
        if (sr - self.sample_rate).abs() > f32::EPSILON {
            self.refresh(sr);
        }
        let n = buf.len();
        for b in &mut self.split {
            if b.len() < n {
                b.resize(n, 0.0);
            }
        }
        // Split at the lower corner…
        let [low, mid_buf, hi_buf] = &mut self.split;
        low[..n].copy_from_slice(buf);
        self.lo_lp.process(&mut low[..n], sample_rate);
        self.low_ap.process(&mut low[..n], sample_rate);
        hi_buf[..n].copy_from_slice(buf);
        self.lo_hp.process(&mut hi_buf[..n], sample_rate);
        // …then split what is left at the upper one.
        mid_buf[..n].copy_from_slice(&hi_buf[..n]);
        self.hi_lp.process(&mut mid_buf[..n], sample_rate);
        self.hi_hp.process(&mut hi_buf[..n], sample_rate);
        for (i, frame) in buf.as_chunks_mut::<2>().0.iter_mut().enumerate() {
            for (ch, s) in frame.iter_mut().enumerate() {
                let dry = *s;
                let lo = self.split[0][i * 2 + ch];
                let mid = self.split[1][i * 2 + ch];
                let hi = self.split[2][i * 2 + ch];
                let out = lo * self.bands[0].gain(ch, lo)
                    + mid * self.bands[1].gain(ch, mid)
                    + hi * self.bands[2].gain(ch, hi);
                *s = dry + self.wet * (out - dry);
            }
        }
    }

    fn reset(&mut self) {
        for b in &mut self.bands {
            b.env = [0.0; 2];
        }
        for f in [
            &mut self.lo_lp,
            &mut self.lo_hp,
            &mut self.hi_lp,
            &mut self.hi_hp,
            &mut self.low_ap,
        ] {
            f.reset();
        }
    }

    fn set_mix(&mut self, wet: f32) {
        self.wet = wet.clamp(0.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f32, amp: f32, sr: u32) -> Vec<f32> {
        (0..sr as usize / 2)
            .flat_map(|i| {
                let v = (std::f32::consts::TAU * hz * i as f32 / sr as f32).sin() * amp;
                [v, v]
            })
            .collect()
    }

    fn rms(buf: &[f32]) -> f32 {
        let tail = &buf[buf.len() / 2..];
        (tail.iter().map(|s| s * s).sum::<f32>() / tail.len() as f32).sqrt()
    }

    /// The band that is over its threshold comes down, and the band that is
    /// not stays where it was — the whole reason for three of them.
    #[test]
    fn only_the_loud_band_is_compressed() {
        let sr = 48_000;
        let mut fx = MultibandCompressor::new(sr);
        // Low band on a hair trigger, top band effectively off.
        fx.set_param(2, 0.0); // Lo Thr −60 dB
        fx.set_param(3, 1.0); // Lo Ratio 20:1
        fx.set_param(6, 1.0); // Hi Thr 0 dB
        fx.set_param(7, 0.0); // Hi Ratio 1:1

        let mut bass = tone(80.0, 0.5, sr);
        let before = rms(&bass);
        fx.process_block(&mut bass, sr);
        assert!(
            rms(&bass) < before * 0.3,
            "the low band was not held: {} against {before}",
            rms(&bass)
        );

        fx.reset();
        let mut top = tone(8000.0, 0.5, sr);
        let before = rms(&top);
        fx.process_block(&mut top, sr);
        let after = rms(&top);
        assert!(
            (after - before).abs() < before * 0.1,
            "the top band moved with it: {after} against {before}"
        );
    }

    /// With every band idle the three come back at the level that went in, at
    /// every frequency — including right on a corner, which is where a
    /// crossover that does not sum flat puts a hole. (The *phase* is turned:
    /// that is what Linkwitz-Riley does, and it is inaudible on a mix bus.)
    #[test]
    fn the_bands_sum_back_flat() {
        let sr = 48_000;
        for hz in [80.0, 200.0, 500.0, 3000.0, 8000.0] {
            let mut fx = MultibandCompressor::new(sr);
            for i in [2, 4, 6] {
                fx.set_param(i, 1.0); // every threshold at 0 dB
            }
            for i in [3, 5, 7] {
                fx.set_param(i, 0.0); // and 1:1, so nothing can move
            }
            let src = tone(hz, 0.4, sr);
            let mut out = src.clone();
            fx.process_block(&mut out, sr);
            let db = 20.0 * (rms(&out) / rms(&src)).log10();
            assert!(
                db.abs() < 0.5,
                "{hz} Hz came back {db:.2} dB off through an idle crossover"
            );
        }
    }
}
