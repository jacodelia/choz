//! 4-band parametric EQ using biquad filters.
//!
//! Band layout as [`ParametricEq::from_params`] builds it — one band per knob:
//!   0 — Low shelf   (`Low`,    frequency from `LowFreq`)
//!   1 — Peak/bell   (`LowMid`, 250 Hz, Q from `MidQ`)
//!   2 — Peak/bell   (`HiMid`,  2 kHz,  Q from `MidQ`)
//!   3 — High shelf  (`High`,   frequency from `HiFreq`)
//!
//! The bands can be aimed at one side or at mid/side ([`EqMode`]) and listened
//! to one at a time (`solo`), and [`ParametricEq::response_db`] hands the
//! interface the curve they add up to.

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EqBandKind {
    HighPass,
    LowShelf,
    Peak,
    HighShelf,
    LowPass,
    /// Not a band the user picks: what `solo` listens through.
    BandPass,
    Bypass,
}

/// Which part of the stereo signal the bands work on.
///
/// One set of bands, aimed at a component — not two curves to edit. Aiming
/// covers what the split is actually for ("de-ess only the sides", "the mud is
/// on the left") for the price of one knob.
// ponytail: one band-set with a target; two independent curves the day someone
// needs a different shape per channel, not before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EqMode {
    Stereo,
    Left,
    Right,
    Mid,
    Side,
}

impl EqMode {
    pub const ALL: [EqMode; 5] = [
        EqMode::Stereo,
        EqMode::Left,
        EqMode::Right,
        EqMode::Mid,
        EqMode::Side,
    ];

    pub fn label(self) -> &'static str {
        match self {
            EqMode::Stereo => "Stereo",
            EqMode::Left => "L only",
            EqMode::Right => "R only",
            EqMode::Mid => "Mid",
            EqMode::Side => "Side",
        }
    }

    pub fn to_norm(self) -> f32 {
        Self::ALL.iter().position(|m| *m == self).unwrap_or(0) as f32 / (Self::ALL.len() - 1) as f32
    }

    pub fn from_norm(v: f32) -> Self {
        let n = Self::ALL.len();
        let i = (v.clamp(0.0, 1.0) * (n - 1) as f32).round() as usize;
        Self::ALL[i.min(n - 1)]
    }
}

pub struct EqBand {
    pub kind: EqBandKind,
    pub freq: f32,    // Hz
    pub gain_db: f32, // ±18 dB (ignored for HP/LP)
    pub q: f32,       // 0.1..10.0
    pub enabled: bool,
    // biquad state (stereo)
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1l: f32,
    x2l: f32,
    y1l: f32,
    y2l: f32,
    x1r: f32,
    x2r: f32,
    y1r: f32,
    y2r: f32,
    last_sr: u32,
}

impl EqBand {
    fn new(kind: EqBandKind, freq: f32, gain_db: f32, q: f32) -> Self {
        let mut b = Self {
            kind,
            freq,
            gain_db,
            q,
            enabled: true,
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            x1l: 0.0,
            x2l: 0.0,
            y1l: 0.0,
            y2l: 0.0,
            x1r: 0.0,
            x2r: 0.0,
            y1r: 0.0,
            y2r: 0.0,
            last_sr: 0,
        };
        b.compute_coeffs(44100);
        b
    }

    fn compute_coeffs(&mut self, sr: u32) {
        self.last_sr = sr;
        use std::f32::consts::PI;
        let w0 = 2.0 * PI * self.freq / sr as f32;
        let cos_w = w0.cos();
        let sin_w = w0.sin();
        let alpha = sin_w / (2.0 * self.q);
        let a_lin = 10.0f32.powf(self.gain_db / 40.0); // gain as amplitude (shelves/peak)

        let (b0, b1, b2, a0, a1, a2) = match self.kind {
            EqBandKind::HighPass => {
                let b0 = (1.0 + cos_w) / 2.0;
                let b1 = -(1.0 + cos_w);
                let b2 = (1.0 + cos_w) / 2.0;
                let a0 = 1.0 + alpha;
                let a1 = -2.0 * cos_w;
                let a2 = 1.0 - alpha;
                (b0, b1, b2, a0, a1, a2)
            }
            EqBandKind::LowPass => {
                let b0 = (1.0 - cos_w) / 2.0;
                let b1 = 1.0 - cos_w;
                let b2 = (1.0 - cos_w) / 2.0;
                let a0 = 1.0 + alpha;
                let a1 = -2.0 * cos_w;
                let a2 = 1.0 - alpha;
                (b0, b1, b2, a0, a1, a2)
            }
            EqBandKind::LowShelf => {
                let a_sq = a_lin.sqrt();
                let b0 = a_lin * ((a_lin + 1.0) - (a_lin - 1.0) * cos_w + 2.0 * a_sq * alpha);
                let b1 = 2.0 * a_lin * ((a_lin - 1.0) - (a_lin + 1.0) * cos_w);
                let b2 = a_lin * ((a_lin + 1.0) - (a_lin - 1.0) * cos_w - 2.0 * a_sq * alpha);
                let a0 = (a_lin + 1.0) + (a_lin - 1.0) * cos_w + 2.0 * a_sq * alpha;
                let a1 = -2.0 * ((a_lin - 1.0) + (a_lin + 1.0) * cos_w);
                let a2 = (a_lin + 1.0) + (a_lin - 1.0) * cos_w - 2.0 * a_sq * alpha;
                (b0, b1, b2, a0, a1, a2)
            }
            EqBandKind::HighShelf => {
                let a_sq = a_lin.sqrt();
                let b0 = a_lin * ((a_lin + 1.0) + (a_lin - 1.0) * cos_w + 2.0 * a_sq * alpha);
                let b1 = -2.0 * a_lin * ((a_lin - 1.0) + (a_lin + 1.0) * cos_w);
                let b2 = a_lin * ((a_lin + 1.0) + (a_lin - 1.0) * cos_w - 2.0 * a_sq * alpha);
                let a0 = (a_lin + 1.0) - (a_lin - 1.0) * cos_w + 2.0 * a_sq * alpha;
                let a1 = 2.0 * ((a_lin - 1.0) - (a_lin + 1.0) * cos_w);
                let a2 = (a_lin + 1.0) - (a_lin - 1.0) * cos_w - 2.0 * a_sq * alpha;
                (b0, b1, b2, a0, a1, a2)
            }
            EqBandKind::Peak => {
                let b0 = 1.0 + alpha * a_lin;
                let b1 = -2.0 * cos_w;
                let b2 = 1.0 - alpha * a_lin;
                let a0 = 1.0 + alpha / a_lin;
                let a1 = -2.0 * cos_w;
                let a2 = 1.0 - alpha / a_lin;
                (b0, b1, b2, a0, a1, a2)
            }
            // Constant-peak-gain bandpass: what soloing a band listens through.
            EqBandKind::BandPass => {
                let b0 = alpha;
                let b1 = 0.0;
                let b2 = -alpha;
                let a0 = 1.0 + alpha;
                let a1 = -2.0 * cos_w;
                let a2 = 1.0 - alpha;
                (b0, b1, b2, a0, a1, a2)
            }
            EqBandKind::Bypass => (1.0, 0.0, 0.0, 1.0, 0.0, 0.0),
        };

        let inv_a0 = 1.0 / a0;
        self.b0 = b0 * inv_a0;
        self.b1 = b1 * inv_a0;
        self.b2 = b2 * inv_a0;
        self.a1 = a1 * inv_a0;
        self.a2 = a2 * inv_a0;
    }

    #[inline]
    fn process_sample_l(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1l + self.b2 * self.x2l
            - self.a1 * self.y1l
            - self.a2 * self.y2l;
        self.x2l = self.x1l;
        self.x1l = x;
        self.y2l = self.y1l;
        self.y1l = y;
        y
    }

    #[inline]
    fn process_sample_r(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1r + self.b2 * self.x2r
            - self.a1 * self.y1r
            - self.a2 * self.y2r;
        self.x2r = self.x1r;
        self.x1r = x;
        self.y2r = self.y1r;
        self.y1r = y;
        y
    }

    /// |H(e^jw)| of this band's biquad. `w` is the normalised frequency.
    fn magnitude_at(&self, w: f32) -> f32 {
        let (cw, sw) = (w.cos(), w.sin());
        let (c2w, s2w) = ((2.0 * w).cos(), (2.0 * w).sin());
        // e^-jw = cos w − j sin w
        let num_re = self.b0 + self.b1 * cw + self.b2 * c2w;
        let num_im = -(self.b1 * sw + self.b2 * s2w);
        let den_re = 1.0 + self.a1 * cw + self.a2 * c2w;
        let den_im = -(self.a1 * sw + self.a2 * s2w);
        let num = (num_re * num_re + num_im * num_im).sqrt();
        let den = (den_re * den_re + den_im * den_im).sqrt().max(1e-12);
        num / den
    }

    fn clear_state(&mut self) {
        self.x1l = 0.0;
        self.x2l = 0.0;
        self.y1l = 0.0;
        self.y2l = 0.0;
        self.x1r = 0.0;
        self.x2r = 0.0;
        self.y1r = 0.0;
        self.y2r = 0.0;
    }
}

pub struct ParametricEq {
    pub bands: [EqBand; 4],
    pub mode: EqMode,
    /// Band being listened to on its own, if any.
    pub solo: Option<usize>,
    mix: f32,
    last_sr: u32,
    /// Scratch bandpass the solo runs through — built once, retuned in place,
    /// so listening to a band allocates nothing.
    solo_band: EqBand,
}

impl ParametricEq {
    pub fn new() -> Self {
        Self {
            bands: [
                EqBand::new(EqBandKind::HighPass, 80.0, 0.0, 0.707),
                EqBand::new(EqBandKind::LowShelf, 200.0, 0.0, 0.707),
                EqBand::new(EqBandKind::Peak, 1000.0, 0.0, 1.0),
                EqBand::new(EqBandKind::HighShelf, 8000.0, 0.0, 0.707),
            ],
            mode: EqMode::Stereo,
            solo: None,
            mix: 1.0,
            last_sr: 0,
            solo_band: EqBand::new(EqBandKind::BandPass, 1000.0, 0.0, 1.0),
        }
    }

    /// Build from the chain's normalised parameter array.
    ///
    /// The chain and the interface both go through here, so the curve that
    /// gets drawn is the curve that is running: two copies of this mapping
    /// would be a plot that lies the day one of them is edited.
    pub fn from_params(p: &[f32], sample_rate: u32) -> Self {
        let v = |i: usize| p.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0);
        let mut eq = Self::new();
        // Four knobs, four bands. The old mapping wrote band 3 twice, so the
        // `HiMid` knob moved nothing, and band 0 was a high-pass at 80 Hz that
        // no knob reached and every signal went through.
        eq.bands[0].kind = EqBandKind::LowShelf;
        eq.bands[0].gain_db = (v(0) - 0.5) * 36.0;
        eq.bands[0].freq = 20.0 * (800.0f32 / 20.0).powf(v(4));
        eq.bands[1].kind = EqBandKind::Peak;
        eq.bands[1].gain_db = (v(1) - 0.5) * 36.0;
        eq.bands[1].freq = 250.0;
        eq.bands[2].kind = EqBandKind::Peak;
        eq.bands[2].gain_db = (v(2) - 0.5) * 36.0;
        eq.bands[2].freq = 2000.0;
        eq.bands[3].kind = EqBandKind::HighShelf;
        eq.bands[3].gain_db = (v(3) - 0.5) * 36.0;
        eq.bands[3].freq = 1000.0 * 20.0f32.powf(v(5));
        let q = 0.1 + v(6) * 9.9;
        eq.bands[1].q = q;
        eq.bands[2].q = q;
        eq.mode = EqMode::from_norm(v(7));
        // Solo is "off plus four bands": a knob at 0 is off, and the four
        // quarters above it are the bands in the order they are drawn.
        let solo = (v(8) * 4.0).round() as usize;
        eq.solo = (solo > 0).then(|| solo - 1);
        for b in &mut eq.bands {
            b.compute_coeffs(sample_rate);
        }
        eq.last_sr = sample_rate;
        eq
    }

    /// Magnitude of the whole curve at `freq_hz`, in dB — what a response plot
    /// asks for. Computed from the same coefficients that process the audio,
    /// so the drawing cannot claim a shape the filter does not have.
    pub fn response_db(&self, freq_hz: f32, sample_rate: u32) -> f32 {
        let w = 2.0 * std::f32::consts::PI * freq_hz / sample_rate as f32;
        let mut mag = 1.0f32;
        for b in &self.bands {
            if !b.enabled || matches!(b.kind, EqBandKind::Bypass) {
                continue;
            }
            mag *= b.magnitude_at(w);
        }
        20.0 * mag.max(1e-10).log10()
    }
}

impl Default for ParametricEq {
    fn default() -> Self {
        Self::new()
    }
}

impl super::FxProcessor for ParametricEq {
    /// The ten knobs [`ParametricEq::from_params`] reads, read back out. The
    /// inverse of that mapping, index for index: what a host saves has to be
    /// what a rebuild would have made of the same numbers.
    fn params(&self) -> Vec<crate::fx::FxParam> {
        use crate::fx::FxParam;
        let gain = |i: usize| self.bands[i].gain_db / 36.0 + 0.5;
        vec![
            FxParam::new("Low", gain(0), -18.0, 18.0, "dB"),
            FxParam::new("LowMid", gain(1), -18.0, 18.0, "dB"),
            FxParam::new("HiMid", gain(2), -18.0, 18.0, "dB"),
            FxParam::new("High", gain(3), -18.0, 18.0, "dB"),
            FxParam::new(
                "LowFreq",
                (self.bands[0].freq / 20.0).max(1.0).ln() / 40.0f32.ln(),
                20.0,
                800.0,
                "Hz",
            ),
            FxParam::new(
                "HiFreq",
                (self.bands[3].freq / 1000.0).max(1.0).ln() / 20.0f32.ln(),
                1000.0,
                20000.0,
                "Hz",
            ),
            FxParam::new("MidQ", (self.bands[1].q - 0.1) / 9.9, 0.1, 10.0, ""),
            FxParam::new("Mode", self.mode.to_norm(), 0.0, 4.0, ""),
            FxParam::new(
                "Solo",
                self.solo.map_or(0.0, |i| (i + 1) as f32 / 4.0),
                0.0,
                4.0,
                "",
            ),
            FxParam::new("Wet", self.mix, 0.0, 1.0, ""),
        ]
    }

    /// Live, and the same mapping as [`ParametricEq::from_params`]: only the
    /// coefficients of the band that moved are recomputed, so riding the low
    /// shelf does not rebuild the other three.
    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        let sr = self.last_sr.max(1);
        let mut touched: &[usize] = &[];
        match index {
            0..=3 => {
                self.bands[index].gain_db = (v - 0.5) * 36.0;
                touched = match index {
                    0 => &[0],
                    1 => &[1],
                    2 => &[2],
                    _ => &[3],
                };
            }
            4 => {
                self.bands[0].freq = 20.0 * (800.0f32 / 20.0).powf(v);
                touched = &[0];
            }
            5 => {
                self.bands[3].freq = 1000.0 * 20.0f32.powf(v);
                touched = &[3];
            }
            6 => {
                let q = 0.1 + v * 9.9;
                self.bands[1].q = q;
                self.bands[2].q = q;
                touched = &[1, 2];
            }
            7 => self.mode = EqMode::from_norm(v),
            8 => {
                let solo = (v * 4.0).round() as usize;
                self.solo = (solo > 0).then(|| solo - 1);
            }
            9 => self.mix = v,
            _ => {}
        }
        for &b in touched {
            self.bands[b].compute_coeffs(sr);
        }
    }

    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        if buf.len() < 2 {
            return;
        }
        if sample_rate != self.last_sr {
            self.last_sr = sample_rate;
            for b in &mut self.bands {
                b.compute_coeffs(sample_rate);
            }
        }

        // Solo: one bandpass at the soloed band's frequency, so what comes out
        // is the range that band is working on and nothing else.
        let solo = self.solo.filter(|i| *i < self.bands.len());
        if let Some(i) = solo {
            let (freq, q) = (self.bands[i].freq, self.bands[i].q);
            if self.solo_band.freq != freq || self.solo_band.q != q {
                self.solo_band.freq = freq;
                self.solo_band.q = q.max(0.5); // a wide "listen" is not a listen
                self.solo_band.compute_coeffs(sample_rate);
            }
        }

        let frames = buf.len() / 2;
        for i in 0..frames {
            let orig_l = buf[i * 2];
            let orig_r = buf[i * 2 + 1];

            // Split into the pair the bands work on. Right is left untouched
            // in the single-channel modes: that is the whole point of them.
            let (mut a, mut b, passthrough) = match self.mode {
                EqMode::Stereo => (orig_l, orig_r, false),
                EqMode::Left => (orig_l, orig_r, true),
                EqMode::Right => (orig_r, orig_l, true),
                EqMode::Mid => ((orig_l + orig_r) * 0.5, (orig_l - orig_r) * 0.5, true),
                EqMode::Side => ((orig_l - orig_r) * 0.5, (orig_l + orig_r) * 0.5, true),
            };

            if solo.is_some() {
                a = self.solo_band.process_sample_l(a);
                if !passthrough {
                    b = self.solo_band.process_sample_r(b);
                }
            } else {
                for band in &mut self.bands {
                    if band.enabled && !matches!(band.kind, EqBandKind::Bypass) {
                        a = band.process_sample_l(a);
                        if !passthrough {
                            b = band.process_sample_r(b);
                        }
                    }
                }
            }

            let (l, r) = match self.mode {
                EqMode::Stereo | EqMode::Left => (a, b),
                EqMode::Right => (b, a),
                // a = processed mid, b = untouched side (and the reverse).
                EqMode::Mid => (a + b, a - b),
                EqMode::Side => (b + a, b - a),
            };
            buf[i * 2] = orig_l + self.mix * (l - orig_l);
            buf[i * 2 + 1] = orig_r + self.mix * (r - orig_r);
        }
    }

    fn reset(&mut self) {
        for b in &mut self.bands {
            b.clear_state();
        }
        self.solo_band.clear_state();
    }

    fn set_mix(&mut self, wet: f32) {
        self.mix = wet.clamp(0.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::FxProcessor;

    fn sine_block(freq_hz: f32, sr: u32, frames: usize) -> Vec<f32> {
        let mut buf = vec![0.0f32; frames * 2];
        for i in 0..frames {
            let s = (2.0 * std::f32::consts::PI * freq_hz * i as f32 / sr as f32).sin();
            buf[i * 2] = s;
            buf[i * 2 + 1] = s;
        }
        buf
    }

    fn rms(buf: &[f32]) -> f32 {
        let sum: f32 = buf.iter().map(|s| s * s).sum();
        (sum / buf.len() as f32).sqrt()
    }

    #[test]
    fn all_bands_bypass_is_unity() {
        let mut eq = ParametricEq::new();
        for band in &mut eq.bands {
            band.kind = EqBandKind::Bypass;
        }
        let mut buf = sine_block(440.0, 48000, 512);
        let before = rms(&buf);
        eq.process_block(&mut buf, 48000);
        let after = rms(&buf);
        assert!(
            (after - before).abs() < 1e-4,
            "bypass should be unity, before={before} after={after}"
        );
    }

    #[test]
    fn peak_boost_increases_level_at_target_freq() {
        let mut eq = ParametricEq::new();
        for band in &mut eq.bands {
            band.kind = EqBandKind::Bypass;
        }
        eq.bands[2].kind = EqBandKind::Peak;
        eq.bands[2].freq = 1000.0;
        eq.bands[2].gain_db = 12.0;
        eq.bands[2].q = 2.0;

        let mut buf = sine_block(1000.0, 48000, 4096);
        let before = rms(&buf);
        eq.process_block(&mut buf, 48000);
        let after = rms(&buf[4096..]); // skip transient startup
        assert!(
            after > before * 1.5,
            "12 dB peak should boost 1 kHz, before={before} after={after}"
        );
    }

    #[test]
    fn high_pass_attenuates_dc() {
        let mut eq = ParametricEq::new();
        for band in &mut eq.bands {
            band.kind = EqBandKind::Bypass;
        }
        eq.bands[0].kind = EqBandKind::HighPass;
        eq.bands[0].freq = 200.0;

        // DC signal (0 Hz) should be heavily attenuated by the HP filter.
        let mut buf = vec![1.0f32; 2048]; // stereo DC
        eq.process_block(&mut buf, 48000);
        let last = buf[2040].abs();
        assert!(last < 0.1, "HP filter should attenuate DC, got {last}");
    }

    /// The drawn curve has to be the filter's, not a picture of one: the
    /// magnitude computed from the coefficients matches what a sine measures.
    #[test]
    fn the_response_curve_matches_what_the_filter_does() {
        let mut eq = ParametricEq::new();
        for band in &mut eq.bands {
            band.kind = EqBandKind::Bypass;
        }
        eq.bands[2].kind = EqBandKind::Peak;
        eq.bands[2].freq = 1000.0;
        eq.bands[2].gain_db = 9.0;
        eq.bands[2].q = 1.5;
        eq.bands[2].compute_coeffs(48000);
        eq.last_sr = 48000;

        for probe in [200.0f32, 1000.0, 5000.0] {
            let mut buf = sine_block(probe, 48000, 8192);
            let before = rms(&buf[8192..]);
            eq.reset();
            eq.process_block(&mut buf, 48000);
            let after = rms(&buf[8192..]);
            let measured = 20.0 * (after / before).log10();
            let predicted = eq.response_db(probe, 48000);
            assert!(
                (measured - predicted).abs() < 0.5,
                "at {probe} Hz: measured {measured} dB, curve says {predicted} dB"
            );
        }
    }

    /// Aiming the bands at one component leaves the other exactly as it came.
    #[test]
    fn mid_side_and_single_channel_modes_leave_the_rest_alone() {
        let make = |mode: EqMode| {
            let mut eq = ParametricEq::new();
            for band in &mut eq.bands {
                band.kind = EqBandKind::Bypass;
            }
            eq.bands[2].kind = EqBandKind::Peak;
            eq.bands[2].freq = 1000.0;
            eq.bands[2].gain_db = -18.0;
            eq.bands[2].q = 1.0;
            eq.mode = mode;
            eq
        };
        // Mono 1 kHz: all mid, no side.
        let mut buf = sine_block(1000.0, 48000, 4096);
        make(EqMode::Side).process_block(&mut buf, 48000);
        let reference = sine_block(1000.0, 48000, 4096);
        assert!(
            buf.iter()
                .zip(reference.iter())
                .all(|(a, b)| (a - b).abs() < 1e-3),
            "a mono signal has no side content to cut"
        );

        // The same signal in Mid mode does get cut.
        let mut buf = sine_block(1000.0, 48000, 4096);
        make(EqMode::Mid).process_block(&mut buf, 48000);
        assert!(
            rms(&buf[4096..]) < rms(&reference[4096..]) * 0.5,
            "mid should be cut"
        );

        // Left-only leaves the right channel byte for byte.
        let mut buf = sine_block(1000.0, 48000, 4096);
        make(EqMode::Left).process_block(&mut buf, 48000);
        let right: Vec<f32> = buf.iter().skip(1).step_by(2).copied().collect();
        let right_ref: Vec<f32> = reference.iter().skip(1).step_by(2).copied().collect();
        assert_eq!(right, right_ref, "L-only must not touch R");
        let left: Vec<f32> = buf.iter().step_by(2).copied().collect();
        assert!(rms(&left[2048..]) < rms(&right_ref[2048..]) * 0.5);
    }

    /// Soloing a band is a listen, not a bypass: everything outside it goes.
    #[test]
    fn solo_leaves_only_the_band_being_listened_to() {
        let run = |probe: f32| {
            let mut eq = ParametricEq::new();
            eq.bands[2].freq = 1000.0;
            eq.bands[2].q = 2.0;
            eq.solo = Some(2);
            let mut buf = sine_block(probe, 48000, 8192);
            let before = rms(&buf[8192..]);
            eq.process_block(&mut buf, 48000);
            rms(&buf[8192..]) / before
        };
        assert!(run(1000.0) > 0.7, "the soloed band should pass");
        assert!(run(60.0) < 0.2, "everything below it should not");
        assert!(run(12000.0) < 0.2, "nor above");
    }

    #[test]
    fn low_shelf_cut_reduces_bass() {
        let mut eq = ParametricEq::new();
        for band in &mut eq.bands {
            band.kind = EqBandKind::Bypass;
        }
        eq.bands[1].kind = EqBandKind::LowShelf;
        eq.bands[1].freq = 500.0;
        eq.bands[1].gain_db = -12.0;

        let mut buf = sine_block(100.0, 48000, 4096);
        let before = rms(&buf);
        eq.process_block(&mut buf, 48000);
        let after = rms(&buf[4096..]);
        assert!(
            after < before * 0.5,
            "−12 dB low shelf should reduce 100 Hz, before={before} after={after}"
        );
    }
}
