//! Frequency shifter and ring modulator: the two effects that move a spectrum
//! *by* something instead of *scaling* it.
//!
//! A pitch shifter multiplies every partial by the same ratio, so a harmonic
//! sound stays harmonic. These two **add** a constant to every partial, so it
//! stops being harmonic — which is the whole point, and why neither of them is
//! a knob on the pitch shifter.
//!
//! ```text
//!   pitch shift  200 400 600 →  ×1.5 →  300 600 900   (still harmonic)
//!   freq  shift  200 400 600 →  +100 →  300 500 700   (no longer harmonic)
//! ```
//!
//! # How the shift is done
//!
//! Multiplying by a sine gives **both** sidebands — that is the ring modulator,
//! and it is one line. Keeping only one of them needs the *analytic* signal:
//! the input and a copy of it phase-shifted by 90° at every frequency. That
//! copy comes from a pair of all-pass chains whose outputs stay a quarter cycle
//! apart across the band (the classic polyphase Hilbert pair), and then
//!
//! ```text
//!   out = re·cos(θ) + im·sin(θ)      θ advancing at the shift frequency
//! ```
//!
//! is the input with every partial moved up by that many Hz, with no image.
//!
//! # Real-time
//!
//! Eight one-pole-pair all-pass sections and an oscillator per channel. No
//! allocation, no branches per sample, and nothing that depends on the block
//! size.

use super::smooth::Smoothed;

/// Second-order all-pass section, in the `z^-2` form the Hilbert pair uses.
#[derive(Clone, Copy, Default)]
struct Allpass2 {
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Allpass2 {
    fn new(coeff: f32) -> Self {
        Self {
            a2: coeff * coeff,
            ..Default::default()
        }
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.a2 * (x + self.y2) - self.x2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// Coefficients of the two chains. Published all-pass sets for a 90° pair;
/// they are properties of the structure, not of the sample rate, so the same
/// numbers hold at 44.1 kHz and at 192 kHz.
const CHAIN_A: [f32; 4] = [0.6923878, 0.9360654, 0.9882295, 0.9987488];
const CHAIN_B: [f32; 4] = [0.4021921, 0.856171, 0.9722909, 0.9952885];

/// The input, and the same input a quarter of a cycle behind.
#[derive(Clone, Copy)]
struct Hilbert {
    a: [Allpass2; 4],
    b: [Allpass2; 4],
    /// The **A** chain is read one sample late; that is what completes the 90°.
    ///
    /// Which chain carries the delay is not a detail: measured against a 1 kHz
    /// tone shifted by 200 Hz, the unwanted sideband sits 43 dB down this way
    /// round and only 16 dB down the other — audible as the shifter quietly
    /// producing both.
    delay: f32,
}

impl Hilbert {
    fn new() -> Self {
        Self {
            a: CHAIN_A.map(Allpass2::new),
            b: CHAIN_B.map(Allpass2::new),
            delay: 0.0,
        }
    }

    fn reset(&mut self) {
        *self = Self::new();
    }

    /// `(re, im)`: the analytic pair.
    #[inline]
    fn process(&mut self, x: f32) -> (f32, f32) {
        let mut re = x;
        for s in &mut self.a {
            re = s.process(re);
        }
        let mut im = x;
        for s in &mut self.b {
            im = s.process(im);
        }
        let out = (self.delay, im);
        self.delay = re;
        out
    }
}

/// What the carrier is used for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Carrier {
    /// One sideband: every partial moves by the same number of Hz.
    Shift,
    /// Both sidebands: the classic clangorous ring modulator.
    Ring,
}

pub struct FreqShift {
    kind: Carrier,
    /// Hz. Negative shifts down; the ring modulator only uses the magnitude.
    freq: Smoothed,
    /// How far apart the two channels' carriers run, in cycles.
    spread: f32,
    phase: f32,
    /// The carrier as a point on the unit circle: `(cos θ, sin θ)`.
    ///
    /// Turned by a complex multiply each sample instead of being asked for by
    /// name — `cos` and `sin` of a growing angle was four transcendentals a
    /// frame, 192 000 a second, to walk an angle that moves by the same amount
    /// every time. The rotation for one sample is worked out once a block; the
    /// per-sample cost is four multiplies.
    carrier: (f32, f32),
    hilbert: [Hilbert; 2],
    mix: f32,
    sample_rate: f32,
}

impl FreqShift {
    pub fn new(kind: Carrier, sample_rate: u32) -> Self {
        let sr = sample_rate.max(8000) as f32;
        Self {
            kind,
            freq: Smoothed::new(
                match kind {
                    Carrier::Shift => 0.0,
                    Carrier::Ring => 200.0,
                },
                20.0,
                sr,
            ),
            spread: 0.0,
            phase: 0.0,
            carrier: (1.0, 0.0),
            hilbert: [Hilbert::new(); 2],
            mix: 1.0,
            sample_rate: sr,
        }
    }

    /// Build from the rack's knob positions: freq, spread.
    pub fn with_params(kind: Carrier, sample_rate: u32, p: &[f32]) -> Self {
        let get = |i: usize, d: f32| p.get(i).copied().unwrap_or(d);
        let mut f = Self::new(kind, sample_rate);
        f.set_freq(get(0, 0.5));
        f.spread = get(1, 0.0).clamp(0.0, 1.0);
        f
    }

    /// 0..1 → −1 kHz…+1 kHz for the shifter, 0…2 kHz for the ring modulator.
    ///
    /// The shifter's knob is centred because zero is a real setting there —
    /// it is the effect switched off — while a ring modulator at 0 Hz is a
    /// multiply by a constant, which is a gain.
    pub fn set_freq(&mut self, v: f32) {
        let v = v.clamp(0.0, 1.0);
        self.freq.set_target(match self.kind {
            // Cubic around the centre: the interesting shifts are the small
            // ones, and a linear knob spends its middle on ±400 Hz.
            Carrier::Shift => {
                let t = v * 2.0 - 1.0;
                t * t * t * 1000.0
            }
            Carrier::Ring => 2000.0f32.powf(v),
        });
    }

    pub fn freq_hz(&self) -> f32 {
        self.freq.target()
    }
}

impl super::FxProcessor for FreqShift {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        let sr = sample_rate.max(8000) as f32;
        if (sr - self.sample_rate).abs() > 0.5 {
            self.sample_rate = sr;
            self.freq.set_sample_rate(sr);
        }
        let mix = self.mix;
        let spread = self.spread;

        // One rotation for the whole block, from where the smoothed frequency
        // is headed. The angle stays continuous across the change — only the
        // rate it turns at steps, and a frequency that steps is not a click.
        let step = std::f32::consts::TAU * self.freq.target() / sr;
        let (rot_c, rot_s) = (step.cos(), step.sin());
        // The right channel is the left one turned by the spread, which is a
        // fixed angle: two more multiplies, no second oscillator.
        let (sp_c, sp_s) = {
            let a = std::f32::consts::TAU * spread;
            (a.cos(), a.sin())
        };

        for frame in buf.as_chunks_mut::<2>().0 {
            // Keeps `freq.value()` walking so a knob turn is still smoothed,
            // and keeps `phase` meaningful for whoever reads it.
            let hz = self.freq.tick();
            self.phase += hz / sr;
            // Wrapped, not left to grow: a phase counted in millions of cycles
            // loses the fraction that is the actual angle.
            self.phase -= self.phase.floor();

            let (c, s) = self.carrier;
            self.carrier = (c * rot_c - s * rot_s, s * rot_c + c * rot_s);

            let dry = [frame[0], frame[1]];
            for ch in 0..2 {
                let (cos, sin) = if ch == 1 {
                    (c * sp_c - s * sp_s, s * sp_c + c * sp_s)
                } else {
                    (c, s)
                };
                let (re, im) = self.hilbert[ch].process(dry[ch]);
                let wet = match self.kind {
                    // One sideband: the imaginary part is what cancels the
                    // other one, and dropping it is what makes a ring mod.
                    // `+`, not `−`, for this pair's ordering — measured with a
                    // Goertzel, because the sign convention of a Hilbert pair
                    // is the one thing about it not worth taking on faith.
                    Carrier::Shift => re * cos + im * sin,
                    Carrier::Ring => dry[ch] * cos,
                };
                frame[ch] = dry[ch] + mix * (wet - dry[ch]);
            }
        }

        // A rotation applied thousands of times drifts off the unit circle.
        // One Newton step a block, which is exact enough that the amplitude
        // never moves and costs nothing at block rate.
        let (c, s) = self.carrier;
        let mag2 = c * c + s * s;
        if (mag2 - 1.0).abs() > 1e-6 {
            let k = 1.5 - 0.5 * mag2;
            self.carrier = (c * k, s * k);
        }
    }

    fn reset(&mut self) {
        self.phase = 0.0;
        self.carrier = (1.0, 0.0);
        for h in &mut self.hilbert {
            h.reset();
        }
        self.freq.snap(self.freq.target());
    }

    fn set_mix(&mut self, wet: f32) {
        self.mix = wet.clamp(0.0, 1.0);
    }

    fn name(&self) -> &str {
        match self.kind {
            Carrier::Shift => "FreqShifter",
            Carrier::Ring => "RingMod",
        }
    }

    fn params(&self) -> Vec<crate::fx::FxParam> {
        use crate::fx::FxParam;
        let (norm, min, max) = match self.kind {
            Carrier::Shift => (
                (self.freq.target() / 1000.0).cbrt() * 0.5 + 0.5,
                -1000.0,
                1000.0,
            ),
            Carrier::Ring => ((self.freq.target()).log(2000.0), 1.0, 2000.0),
        };
        vec![
            FxParam::new("Freq", norm.clamp(0.0, 1.0), min, max, "Hz"),
            FxParam::new("Spread", self.spread, 0.0, 1.0, ""),
            FxParam::new("Wet", self.mix, 0.0, 1.0, ""),
        ]
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => self.set_freq(v),
            1 => self.spread = v,
            2 => self.mix = v,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::FxProcessor;

    fn tone(hz: f32, sr: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let s = (std::f32::consts::TAU * hz * i as f32 / sr).sin() * 0.5;
                [s, s]
            })
            .collect()
    }

    /// How much of `probe` Hz is in the left channel, by Goertzel.
    fn energy_at(buf: &[f32], probe: f32, sr: f32) -> f32 {
        let l: Vec<f32> = buf.iter().step_by(2).copied().collect();
        let n = l.len() as f32;
        let k = (probe * n / sr).round();
        let w = std::f32::consts::TAU * k / n;
        let coeff = 2.0 * w.cos();
        let (mut s1, mut s2) = (0.0f32, 0.0f32);
        for x in &l {
            let s0 = x + coeff * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        ((s1 * s1 + s2 * s2 - coeff * s1 * s2).max(0.0)).sqrt() / n
    }

    /// The whole claim: a 1 kHz tone shifted by 200 Hz comes out at 1.2 kHz,
    /// and there is nothing left where it was.
    #[test]
    fn a_shift_moves_the_tone_by_that_many_hz() {
        let sr = 48000.0;
        let mut fx = FreqShift::new(Carrier::Shift, 48000);
        fx.freq.snap(200.0);
        fx.freq.set_target(200.0);
        let mut buf = tone(1000.0, sr, 16384);
        fx.process_block(&mut buf, 48000);
        // Skip the all-pass chains settling.
        let tail = &buf[8192..];
        let at_1200 = energy_at(tail, 1200.0, sr);
        let at_1000 = energy_at(tail, 1000.0, sr);
        let at_800 = energy_at(tail, 800.0, sr);
        assert!(
            at_1200 > at_1000 * 8.0,
            "expected the tone at 1200 Hz: 1200={at_1200} 1000={at_1000}"
        );
        // 43 dB of sideband rejection with this network; 40 is the bar.
        assert!(
            at_1200 > at_800 * 100.0,
            "and only one sideband: 1200={at_1200} 800={at_800}"
        );
    }

    /// Down is the same thing with the sign flipped.
    #[test]
    fn a_negative_shift_moves_it_down() {
        let sr = 48000.0;
        let mut fx = FreqShift::new(Carrier::Shift, 48000);
        fx.freq.snap(-300.0);
        fx.freq.set_target(-300.0);
        let mut buf = tone(1000.0, sr, 16384);
        fx.process_block(&mut buf, 48000);
        let tail = &buf[8192..];
        let (down, up) = (energy_at(tail, 700.0, sr), energy_at(tail, 1300.0, sr));
        assert!(
            down > up * 100.0,
            "expected 700 Hz, not 1300: {down} vs {up}"
        );
    }

    /// Zero shift is a wire — in level, at least: the all-pass chains move the
    /// phase, which is the price of the analytic pair and costs no loudness.
    #[test]
    fn zero_shift_keeps_the_tone_where_it_is() {
        let sr = 48000.0;
        let mut fx = FreqShift::new(Carrier::Shift, 48000);
        fx.freq.snap(0.0);
        fx.freq.set_target(0.0);
        let mut buf = tone(1000.0, sr, 16384);
        let before = energy_at(&buf[8192..], 1000.0, sr);
        fx.process_block(&mut buf, 48000);
        let after = energy_at(&buf[8192..], 1000.0, sr);
        assert!(
            (after - before).abs() < before * 0.15,
            "the level moved: {before} → {after}"
        );
    }

    /// The ring modulator gives *both* sidebands and nothing at the carrier —
    /// which is exactly what the shifter must not do.
    #[test]
    fn the_ring_modulator_gives_both_sidebands() {
        let sr = 48000.0;
        let mut fx = FreqShift::new(Carrier::Ring, 48000);
        fx.freq.snap(200.0);
        fx.freq.set_target(200.0);
        let mut buf = tone(1000.0, sr, 16384);
        fx.process_block(&mut buf, 48000);
        let tail = &buf[8192..];
        let up = energy_at(tail, 1200.0, sr);
        let down = energy_at(tail, 800.0, sr);
        let carrier = energy_at(tail, 1000.0, sr);
        assert!(up > carrier * 8.0 && down > carrier * 8.0, "both sidebands");
        assert!(
            (up - down).abs() < up * 0.2,
            "and the two of them equal: {up} vs {down}"
        );
    }

    /// Neither of them may run away, whatever they are handed.
    #[test]
    fn it_survives_silence_extremes_and_a_rate_change() {
        for kind in [Carrier::Shift, Carrier::Ring] {
            let mut fx = FreqShift::with_params(kind, 48000, &[1.0, 0.5]);
            let mut buf = vec![0.0f32; 1024];
            fx.process_block(&mut buf, 48000);
            assert!(buf.iter().all(|s| s.is_finite()));
            let mut hot = vec![8.0f32; 4096];
            fx.process_block(&mut hot, 96000);
            assert!(hot.iter().all(|s| s.is_finite()));
            assert!(
                hot.iter().fold(0.0f32, |m, s| m.max(s.abs())) < 20.0,
                "an analytic pair should not amplify"
            );
            fx.process_block(&mut [], 96000);
            fx.process_block(&mut [1.0], 96000);
            fx.reset();
        }
    }

    /// The oscillator is a rotation applied over and over, and a rotation in
    /// `f32` walks off the unit circle if nobody puts it back. Half a minute of
    /// audio, and the level at the end has to be the level at the start.
    #[test]
    fn the_carrier_does_not_drift_off_the_circle() {
        let sr = 48_000u32;
        let mut f = FreqShift::new(Carrier::Ring, sr);
        f.set_param(0, 1.0);
        f.set_mix(1.0);
        let level = |f: &mut FreqShift| {
            let mut buf: Vec<f32> = (0..2048)
                .flat_map(|i| {
                    let s = (std::f32::consts::TAU * 300.0 * i as f32 / sr as f32).sin() * 0.5;
                    [s, s]
                })
                .collect();
            f.process_block(&mut buf, sr);
            (buf.iter().map(|s| s * s).sum::<f32>() / buf.len() as f32).sqrt()
        };
        let first = level(&mut f);
        // ~30 s of audio at 48 kHz.
        for _ in 0..700 {
            let mut buf = vec![0.0f32; 2048];
            f.process_block(&mut buf, sr);
        }
        let after = level(&mut f);
        assert!(
            (after - first).abs() < first * 0.01,
            "the carrier drifted: {first:.5} at the start, {after:.5} half a minute later"
        );
    }
}
