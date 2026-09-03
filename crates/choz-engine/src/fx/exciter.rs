//! Harmonic exciter and bass enhancer: two sides of the same trick.
//!
//! Both take a band, make harmonics out of it, and mix those back under the
//! original. The exciter works on the top — the "air" and presence a dull
//! source has none of, generated rather than boosted, so it does not lift the
//! hiss an EQ would. The bass enhancer works on the bottom and mixes the
//! harmonics back **without** the fundamental, which is the psychoacoustic
//! trick: the ear hears a bass note that the speaker never reproduced, so a
//! small box gets low end it cannot physically play.
//!
//! The harmonics come from an asymmetric shaper: `x + a·x²` adds the even
//! harmonics (the warm ones), `tanh` adds the odd; the `Blend` knob crossfades
//! between them.

use super::filter::{Svf, SvfMode};
use choz_ports::{FxParam, FxProcessor};

/// The SVF resonance that puts a section at Butterworth (Q = 0.707), so two
/// cascaded ones are a clean 24 dB/octave. Same constant, same reason, as the
/// multiband compressor's crossover.
const BUTTERWORTH: f32 = 1.0 - std::f32::consts::SQRT_2 / 2.0;

/// A 24 dB/octave pass, as two Butterworth sections.
struct Pass {
    a: Svf,
    b: Svf,
}

impl Pass {
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

/// The shaper both effects use. `even` at 0 is pure odd (tanh), at 1 pure even.
#[inline]
fn harmonics(x: f32, drive: f32, even: f32) -> f32 {
    let d = x * drive;
    let odd = d.tanh();
    // Squared, sign-carrying: the even-order series, and it stays bounded.
    let ev = (d * d.abs()).tanh();
    odd + even * (ev - odd)
}

/// Frames of scratch allocated up front, stereo: the audio thread must not
/// allocate, and both of these need a copy of the block to filter.
const SCRATCH: usize = 8192;

/// The top end, generated rather than boosted.
pub struct Exciter {
    /// The band that gets excited: everything above the corner. A real
    /// high-pass and not `x − lowpass(x)`, which leaves a phase-shifted copy of
    /// the bass in the band and shapes that too.
    split: Pass,
    freq_hz: f32,
    drive: f32,
    even: f32,
    amount: f32,
    wet: f32,
    band: Vec<f32>,
}

impl Exciter {
    pub fn new(_sample_rate: u32) -> Self {
        Self {
            split: Pass::new(SvfMode::Highpass, 3000.0),
            freq_hz: 3000.0,
            drive: 3.0,
            even: 0.5,
            amount: 0.3,
            wet: 1.0,
            band: vec![0.0; SCRATCH],
        }
    }

    pub fn with_params(sample_rate: u32, params: &[f32]) -> Self {
        let mut e = Self::new(sample_rate);
        for (i, p) in params.iter().enumerate() {
            <Self as FxProcessor>::set_param(&mut e, i, *p);
        }
        e
    }
}

impl FxProcessor for Exciter {
    fn name(&self) -> &str {
        "Exciter"
    }

    fn params(&self) -> Vec<FxParam> {
        vec![
            FxParam::new(
                "Freq",
                (self.freq_hz - 1000.0) / 9000.0,
                1000.0,
                10000.0,
                "Hz",
            ),
            FxParam::new("Drive", (self.drive - 1.0) / 9.0, 1.0, 10.0, "x"),
            FxParam::new("Blend", self.even, 0.0, 1.0, ""),
            FxParam::new("Amount", self.amount, 0.0, 1.0, ""),
            FxParam::new("Wet", self.wet, 0.0, 1.0, ""),
        ]
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => {
                self.freq_hz = 1000.0 + v * 9000.0;
                self.split.set_cutoff(self.freq_hz);
            }
            1 => self.drive = 1.0 + v * 9.0,
            2 => self.even = v,
            3 => self.amount = v,
            4 => self.wet = v,
            _ => {}
        }
    }

    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        let n = buf.len();
        if self.band.len() < n {
            self.band.resize(n, 0.0);
        }
        self.band[..n].copy_from_slice(buf);
        self.split.process(&mut self.band[..n], sample_rate);
        for (i, frame) in buf.as_chunks_mut::<2>().0.iter_mut().enumerate() {
            for (ch, s) in frame.iter_mut().enumerate() {
                let dry = *s;
                let high = self.band[i * 2 + ch];
                // Under the original, not instead of it: an exciter that
                // replaces the top end is a distortion box.
                let added = harmonics(high, self.drive, self.even) * self.amount * 0.5;
                *s = dry + self.wet * added;
            }
        }
    }

    fn reset(&mut self) {
        self.split.reset();
    }

    fn set_mix(&mut self, wet: f32) {
        self.wet = wet.clamp(0.0, 1.0);
    }
}

/// The bottom end, heard rather than played.
pub struct BassEnhancer {
    /// Everything below the corner: the band the harmonics are made from.
    split: Pass,
    /// Keeps the fundamental out of what is *added*, so the added part is the
    /// harmonics and not a second copy of the bass. A real high-pass: built by
    /// subtraction it left half the fundamental behind and the enhancer was
    /// simply a bass boost, which is the one thing it must not be.
    guard: Pass,
    freq_hz: f32,
    drive: f32,
    even: f32,
    amount: f32,
    wet: f32,
    added: Vec<f32>,
}

impl BassEnhancer {
    pub fn new(_sample_rate: u32) -> Self {
        Self {
            split: Pass::new(SvfMode::Lowpass, 120.0),
            guard: Pass::new(SvfMode::Highpass, 240.0),
            freq_hz: 120.0,
            drive: 4.0,
            even: 0.5,
            amount: 0.4,
            wet: 1.0,
            added: vec![0.0; SCRATCH],
        }
    }

    pub fn with_params(sample_rate: u32, params: &[f32]) -> Self {
        let mut b = Self::new(sample_rate);
        for (i, p) in params.iter().enumerate() {
            <Self as FxProcessor>::set_param(&mut b, i, *p);
        }
        b
    }
}

impl FxProcessor for BassEnhancer {
    fn name(&self) -> &str {
        "Bass Enhancer"
    }

    fn params(&self) -> Vec<FxParam> {
        vec![
            FxParam::new("Freq", (self.freq_hz - 40.0) / 260.0, 40.0, 300.0, "Hz"),
            FxParam::new("Drive", (self.drive - 1.0) / 9.0, 1.0, 10.0, "x"),
            FxParam::new("Blend", self.even, 0.0, 1.0, ""),
            FxParam::new("Amount", self.amount, 0.0, 1.0, ""),
            FxParam::new("Wet", self.wet, 0.0, 1.0, ""),
        ]
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => {
                self.freq_hz = 40.0 + v * 260.0;
                self.split.set_cutoff(self.freq_hz);
                // An octave above the band: the harmonics start at twice the
                // fundamental, so that is where what is added may begin.
                self.guard.set_cutoff(self.freq_hz * 2.0);
            }
            1 => self.drive = 1.0 + v * 9.0,
            2 => self.even = v,
            3 => self.amount = v,
            4 => self.wet = v,
            _ => {}
        }
    }

    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        let n = buf.len();
        if self.added.len() < n {
            self.added.resize(n, 0.0);
        }
        // One buffer, three passes over it: the low band, then its harmonics
        // in place, then everything still down where the bass was filtered
        // back out of them.
        let work = &mut self.added[..n];
        work.copy_from_slice(buf);
        self.split.process(work, sample_rate);
        let (drive, even, amount) = (self.drive, self.even, self.amount);
        for s in work.iter_mut() {
            *s = harmonics(*s, drive, even) * amount * 0.5;
        }
        self.guard.process(work, sample_rate);
        for (i, frame) in buf.as_chunks_mut::<2>().0.iter_mut().enumerate() {
            for (ch, s) in frame.iter_mut().enumerate() {
                let dry = *s;
                *s = dry + self.wet * self.added[i * 2 + ch];
            }
        }
    }

    fn reset(&mut self) {
        self.split.reset();
        self.guard.reset();
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

    /// Energy at `hz`, by correlating against a sine and a cosine of it.
    fn energy_at(buf: &[f32], hz: f32, sr: u32) -> f32 {
        let mono: Vec<f32> = buf.as_chunks::<2>().0.iter().map(|f| f[0]).collect();
        let half = mono.len() / 2;
        let tail = &mono[half..];
        let (mut re, mut im) = (0.0f32, 0.0f32);
        for (i, s) in tail.iter().enumerate() {
            let ph = std::f32::consts::TAU * hz * (half + i) as f32 / sr as f32;
            re += s * ph.cos();
            im += s * ph.sin();
        }
        (re * re + im * im).sqrt() / tail.len() as f32
    }

    /// The exciter makes top end that was not there, out of top end that was —
    /// and leaves a source with nothing above the corner alone.
    #[test]
    fn the_exciter_adds_harmonics_of_the_band_it_is_given() {
        let sr = 48_000;
        let mut fx = Exciter::new(sr);
        fx.set_param(0, 0.0); // 1 kHz corner
        fx.set_param(3, 1.0); // full amount

        let mut top = tone(2000.0, 0.4, sr);
        let before = energy_at(&top, 6000.0, sr);
        fx.process_block(&mut top, sr);
        let after = energy_at(&top, 6000.0, sr);
        assert!(
            after > before + 0.001,
            "no third harmonic was generated: {after} against {before}"
        );

        fx.reset();
        let mut low = tone(100.0, 0.4, sr);
        let before = energy_at(&low, 300.0, sr);
        fx.process_block(&mut low, sr);
        let after = energy_at(&low, 300.0, sr);
        assert!(
            (after - before).abs() < 0.005,
            "a source with no top end was distorted anyway"
        );
    }

    /// The enhancer's harmonics sit **above** the band it listened to: that is
    /// what makes a small speaker imply a note it cannot play.
    #[test]
    fn the_bass_enhancer_adds_harmonics_and_not_more_bass() {
        let sr = 48_000;
        let mut fx = BassEnhancer::new(sr);
        fx.set_param(0, 0.15); // ~80 Hz corner
        fx.set_param(1, 1.0); // and driven hard, so the harmonics are audible
        fx.set_param(3, 1.0);

        let mut buf = tone(50.0, 0.4, sr);
        let fund_before = energy_at(&buf, 50.0, sr);
        let harm_before = energy_at(&buf, 150.0, sr);
        fx.process_block(&mut buf, sr);
        let fund_after = energy_at(&buf, 50.0, sr);
        let harm_after = energy_at(&buf, 150.0, sr);

        assert!(
            harm_after > harm_before + 0.001,
            "no harmonics: {harm_after} against {harm_before}"
        );
        assert!(
            fund_after < fund_before * 1.15,
            "it just turned the bass up: {fund_after} against {fund_before}"
        );
    }
}
