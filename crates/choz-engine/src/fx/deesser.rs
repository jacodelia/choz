//! De-esser: a compressor that only hears the sibilance.
//!
//! A vocal that sits right in a mix still spits on every `s`, and turning the
//! whole track down for it is what a de-esser exists to avoid. The detector
//! listens through a high-pass at the sibilant corner (5–12 kHz, `Freq`), and
//! what it finds turns down **that band only** — the body of the voice is never
//! touched, which is the difference between de-essing and ducking.
//!
//! `Listen` monitors the detector's band, because the only way to set the
//! frequency by ear is to hear what the detector is hearing.

use super::filter::{Svf, SvfMode};
use choz_ports::{FxParam, FxProcessor};

pub struct DeEsser {
    /// Splits the signal. It runs as a **low**-pass and the sibilant band is
    /// what is left over — `high := x - low` reconstructs exactly, where a
    /// low-pass and a high-pass at the same corner do not: two filters put the
    /// band back with a phase error, and at the corner that error is louder
    /// than the ducking is quiet. This was measured: the first version, built
    /// from a high-pass, made an 8 kHz tone *louder*.
    band: Svf,
    freq_hz: f32,
    threshold_db: f32,
    /// How much of the reduction is applied, 0..1 — a full-strength de-esser on
    /// a voice that only lisps a little is a lisp of its own.
    amount: f32,
    listen: bool,
    wet: f32,
    /// The split band, allocated once: `process_block` runs on the audio
    /// thread, and a `Vec` built per block is a malloc per block.
    low: Vec<f32>,
    /// Envelope of the sibilant band, per channel.
    env: [f32; 2],
    /// Attack/release coefficients, from the sample rate.
    atk: f32,
    rel: f32,
    sample_rate: f32,
}

/// Frames of scratch allocated up front, stereo. Enough for any block a host
/// asks for; a bigger one grows it once rather than every time.
const SCRATCH: usize = 8192;

/// Attack and release, in ms. Fixed: an `s` is 40–120 ms long, and the two
/// numbers that catch one are not a decision anybody wants to make per take.
const ATTACK_MS: f32 = 1.0;
const RELEASE_MS: f32 = 60.0;

impl DeEsser {
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate.max(8000) as f32;
        let mut d = Self {
            band: Svf::new(SvfMode::Lowpass, 6500.0, 0.0),
            freq_hz: 6500.0,
            threshold_db: -30.0,
            amount: 1.0,
            listen: false,
            wet: 1.0,
            low: vec![0.0; SCRATCH],
            env: [0.0; 2],
            atk: 0.0,
            rel: 0.0,
            sample_rate: sr,
        };
        d.refresh(sr);
        d
    }

    pub fn with_params(sample_rate: u32, params: &[f32]) -> Self {
        let mut d = Self::new(sample_rate);
        for (i, p) in params.iter().enumerate() {
            <Self as FxProcessor>::set_param(&mut d, i, *p);
        }
        d
    }

    fn refresh(&mut self, sr: f32) {
        self.sample_rate = sr;
        self.atk = (-1.0 / (ATTACK_MS * 0.001 * sr)).exp();
        self.rel = (-1.0 / (RELEASE_MS * 0.001 * sr)).exp();
        self.band.set_cutoff(self.freq_hz);
    }
}

impl FxProcessor for DeEsser {
    fn name(&self) -> &str {
        "De-esser"
    }

    fn params(&self) -> Vec<FxParam> {
        vec![
            FxParam::new(
                "Freq",
                (self.freq_hz - 3000.0) / 12000.0,
                3000.0,
                15000.0,
                "Hz",
            ),
            FxParam::new(
                "Thresh",
                (self.threshold_db + 60.0) / 60.0,
                -60.0,
                0.0,
                "dB",
            ),
            FxParam::new("Amount", self.amount, 0.0, 1.0, ""),
            FxParam::new("Listen", if self.listen { 1.0 } else { 0.0 }, 0.0, 1.0, ""),
            FxParam::new("Wet", self.wet, 0.0, 1.0, ""),
        ]
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => {
                self.freq_hz = 3000.0 + v * 12000.0;
                self.band.set_cutoff(self.freq_hz);
            }
            1 => self.threshold_db = -60.0 + v * 60.0,
            2 => self.amount = v,
            3 => self.listen = v >= 0.5,
            4 => self.wet = v,
            _ => {}
        }
    }

    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        let sr = sample_rate.max(8000) as f32;
        if (sr - self.sample_rate).abs() > f32::EPSILON {
            self.refresh(sr);
        }
        // Everything *below* the corner, on a copy: the band worked on is the
        // rest of the signal, which is this subtracted from it.
        if self.low.len() < buf.len() {
            self.low.resize(buf.len(), 0.0);
        }
        let low = &mut self.low[..buf.len()];
        low.copy_from_slice(buf);
        self.band.process_block(low, sample_rate);
        let thr = 10f32.powf(self.threshold_db / 20.0);
        for (i, frame) in buf.as_chunks_mut::<2>().0.iter_mut().enumerate() {
            for (ch, s) in frame.iter_mut().enumerate() {
                let hi = *s - self.low[i * 2 + ch];
                let level = hi.abs();
                let coeff = if level > self.env[ch] {
                    self.atk
                } else {
                    self.rel
                };
                self.env[ch] = level + coeff * (self.env[ch] - level);
                // 4:1 above the threshold, in linear terms: the gain is what
                // the band has to be multiplied by to sit a quarter of the way
                // over it.
                let over = self.env[ch] / thr.max(1e-9);
                let gain = if over > 1.0 {
                    let g = over.powf(-0.75);
                    1.0 - self.amount * (1.0 - g)
                } else {
                    1.0
                };
                let dry = *s;
                let processed = match self.listen {
                    // What the detector hears, so the frequency can be set by
                    // ear instead of by arithmetic.
                    true => hi * gain,
                    // The body, plus the band turned down.
                    false => (dry - hi) + hi * gain,
                };
                *s = dry + self.wet * (processed - dry);
            }
        }
    }

    fn reset(&mut self) {
        self.env = [0.0; 2];
        self.band.reset();
    }

    fn set_mix(&mut self, wet: f32) {
        self.wet = wet.clamp(0.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f32, amp: f32, sr: u32, secs: f32) -> Vec<f32> {
        (0..(sr as f32 * secs) as usize)
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

    /// The sibilant band comes down and the body does not: the whole point,
    /// and the thing a plain compressor gets wrong.
    #[test]
    fn it_ducks_the_sibilance_and_leaves_the_body_alone() {
        let sr = 48_000;
        let mut fx = DeEsser::new(sr);
        fx.set_param(0, 0.3); // ~6.6 kHz
        fx.set_param(1, 0.5); // -30 dB
        fx.set_param(2, 1.0);

        let mut sss = tone(8000.0, 0.5, sr, 0.5);
        let before = rms(&sss);
        fx.process_block(&mut sss, sr);
        let after = rms(&sss);
        assert!(
            after < before * 0.6,
            "the ess was not ducked: {after} against {before}"
        );

        fx.reset();
        let mut body = tone(300.0, 0.5, sr, 0.5);
        let before = rms(&body);
        fx.process_block(&mut body, sr);
        let after = rms(&body);
        assert!(
            (after - before).abs() < before * 0.1,
            "the voice was touched: {after} against {before}"
        );
    }
}
