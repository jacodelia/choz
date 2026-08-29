//! Saturator: one waveshaper, eight curves, selectable oversampling.
//!
//! The existing distortions in this crate each bake in one curve and one
//! oversampling factor (`SoftClipper` is `tanh` at 2×, the pedals are voiced
//! stompboxes). This is the general one: pick the curve, pick how much aliasing
//! you are willing to pay to avoid, and drive it.
//!
//! # Signal flow
//!
//! ```text
//! in ──► drive ──► bias ──► [ oversampled curve ] ──► DC block ──► tone ──► out
//!                                                                    │
//!                                                              dry ──┴──► mix
//! ```
//!
//! # Why each piece is there
//!
//! * **Bias** offsets the signal before the curve, so it works a different part
//!   of it. That is what makes even harmonics — the difference between "louder"
//!   and "warmer" — and it is also why the DC blocker after it is not optional:
//!   an asymmetric curve returns a signal with an offset, and an offset costs
//!   headroom in everything downstream while making no sound of its own.
//! * **Tone** after the curve, not before: filtering the harmonics is the point;
//!   filtering the input just drives it less.
//! * **Output gain compensation** — every curve here is normalised so its
//!   output stays in the same place as the drive goes up. Without it, "which
//!   curve sounds better" is only ever "which curve is louder".
//!
//! # Real-time
//!
//! No allocation, no locks, no branches per sample beyond the curve select.
//! Drive, bias, tone amount and mix are smoothed ([`super::smooth::Smoothed`]);
//! the curve and the oversampling factor are **not** — half way between two
//! curves is not a curve, so those switch on a block boundary.

use super::dc::DcBlock;
use super::oversample::{Factor, Oversampler, Tone};
use super::smooth::Smoothed;
use super::utility::Biquad;

/// The transfer curves. Each one is normalised to roughly unity at its knee, so
/// switching between them compares their *shape* and not their level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Curve {
    /// `tanh`: the gentle one. Compresses before it clips, no corner at all.
    #[default]
    Soft,
    /// A straight clamp. All the corner, all the harmonics.
    Hard,
    /// Asymmetric soft clipping: one half harder than the other, which is what
    /// makes even-order harmonics without needing bias.
    Tube,
    /// Soft knee with a slow, level-dependent compression above it.
    Tape,
    /// Past the ceiling the signal turns back on itself instead of flattening.
    Foldback,
    /// The same idea taken further: a triangle wave of the input's amplitude.
    Wavefolder,
    /// The exponential knee of a diode pair — very soft, very asymmetric.
    Diode,
    /// A cubic with the peak flattened: the textbook soft clipper.
    Polynomial,
}

impl Curve {
    pub const ALL: [Curve; 8] = [
        Curve::Soft,
        Curve::Hard,
        Curve::Tube,
        Curve::Tape,
        Curve::Foldback,
        Curve::Wavefolder,
        Curve::Diode,
        Curve::Polynomial,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Curve::Soft => "SOFT",
            Curve::Hard => "HARD",
            Curve::Tube => "TUBE",
            Curve::Tape => "TAPE",
            Curve::Foldback => "FOLD",
            Curve::Wavefolder => "WFOLD",
            Curve::Diode => "DIODE",
            Curve::Polynomial => "POLY",
        }
    }

    pub fn from_norm(v: f32) -> Self {
        let i = (v.clamp(0.0, 1.0) * (Self::ALL.len() - 1) as f32).round() as usize;
        Self::ALL[i.min(Self::ALL.len() - 1)]
    }

    pub fn to_norm(self) -> f32 {
        let i = Self::ALL.iter().position(|c| *c == self).unwrap_or(0);
        i as f32 / (Self::ALL.len() - 1) as f32
    }

    /// The curve itself. Bounded for every input, which is what lets the
    /// processor promise its output cannot run away.
    #[inline]
    pub fn apply(self, x: f32) -> f32 {
        match self {
            Curve::Soft => x.tanh(),
            Curve::Hard => x.clamp(-1.0, 1.0),
            Curve::Tube => {
                // Harder on the way up than on the way down.
                if x >= 0.0 {
                    x.tanh()
                } else {
                    (x * 0.6).tanh() / 0.6 * 0.75
                }
            }
            Curve::Tape => {
                // A soft knee that keeps giving a little above it: no hard
                // ceiling, which is what makes tape forgiving of transients.
                let a = x.abs();
                let y = a / (1.0 + a * a * 0.4).sqrt();
                y.copysign(x)
            }
            Curve::Foldback => {
                // Reflect around ±1 rather than flattening at it — in closed
                // form, not by looping: a `while` here is unbounded work for an
                // unbounded input, which is exactly what an audio thread must
                // never contain. This is the same triangle a repeated
                // reflection converges to, and it equals `x` inside ±1.
                let p = ((x + 1.0) * 0.25).rem_euclid(1.0);
                1.0 - 4.0 * (p - 0.5).abs()
            }
            Curve::Wavefolder => {
                // A triangle: fold repeatedly, in closed form.
                let p = (x * 0.25 + 0.25).fract();
                let p = if p < 0.0 { p + 1.0 } else { p };
                4.0 * (p - 0.5).abs() - 1.0
            }
            Curve::Diode => {
                // Exponential knee, asymmetric like a real pair.
                let y = if x >= 0.0 {
                    1.0 - (-x * 1.2).exp()
                } else {
                    -(1.0 - (x * 2.4).exp()) * 0.7
                };
                y * 1.3
            }
            Curve::Polynomial => {
                let c = x.clamp(-1.5, 1.5);
                (c - c * c * c / 6.75) * 0.75
            }
        }
    }
}

/// How many points the drawn curve is made of.
///
/// Eight: enough to bend a knee, put a step in, or invert half of the curve,
/// and few enough that every point is a knob the rack already knows how to
/// draw. More points would need a curve editor, and a curve editor is a mode.
pub const TABLE_POINTS: usize = 8;

/// A transfer curve the user drew, instead of one this file computed.
///
/// The points are the output at inputs spread evenly over −1…+1; between them
/// the curve is a straight line, and outside them it is flat at the end point.
/// Flat, not extrapolated: a line that keeps rising is a waveshaper that
/// cannot promise a bounded output, and every other curve here can.
// ponytail: straight lines between points. A spline the day someone can hear
// the corners over the harmonics the corners are there to make.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Table {
    pub points: [f32; TABLE_POINTS],
}

impl Default for Table {
    /// The identity: out = in. A shaper that does nothing until it is drawn.
    fn default() -> Self {
        let mut points = [0.0; TABLE_POINTS];
        for (i, p) in points.iter_mut().enumerate() {
            *p = Self::input_at(i);
        }
        Self { points }
    }
}

impl Table {
    /// The input level point `i` sits at, −1…+1.
    pub fn input_at(i: usize) -> f32 {
        -1.0 + 2.0 * i as f32 / (TABLE_POINTS - 1) as f32
    }

    /// Read the points from normalised knob positions (0 = −1, 1 = +1).
    pub fn from_norm(p: &[f32]) -> Self {
        let mut t = Self::default();
        for (i, point) in t.points.iter_mut().enumerate() {
            if let Some(v) = p.get(i) {
                *point = v.clamp(0.0, 1.0) * 2.0 - 1.0;
            }
        }
        t
    }

    #[inline]
    fn apply(&self, x: f32) -> f32 {
        let t = (x.clamp(-1.0, 1.0) + 1.0) * 0.5 * (TABLE_POINTS - 1) as f32;
        let i = (t as usize).min(TABLE_POINTS - 2);
        let frac = t - i as f32;
        self.points[i] + (self.points[i + 1] - self.points[i]) * frac
    }
}

/// Where the curve comes from: one of the eight built in, or one drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    Curve(Curve),
    Table(Table),
}

impl Shape {
    #[inline]
    fn apply(self, x: f32) -> f32 {
        match self {
            Shape::Curve(c) => c.apply(x),
            Shape::Table(t) => t.apply(x),
        }
    }
}

/// One channel's worth of state.
#[derive(Clone, Copy)]
struct Channel {
    os: Oversampler,
    dc: DcBlock,
    tone: Tone,
}

impl Channel {
    fn new(sr: f32, factor: Factor, tone: f32) -> Self {
        Self {
            os: Oversampler::new(sr, factor),
            dc: DcBlock::new(sr),
            tone: Tone::new(tone, sr),
        }
    }
}

pub struct Saturator {
    shape: Shape,
    factor: Factor,
    drive: Smoothed,
    bias: Smoothed,
    output: Smoothed,
    mix: f32,
    /// Where the tone filter is set. Rebuilt on change rather than smoothed:
    /// recomputing biquad coefficients every sample costs more than the click
    /// it would avoid, and this one is a set-and-leave control.
    tone: f32,
    sample_rate: f32,
    left: Channel,
    right: Channel,
    /// Peak in and out of the last block, for a meter that costs nothing.
    /// Shared, because the processor is on the RT thread by the time anyone
    /// wants to look at it.
    meter: choz_ports::FxMeter,
}

impl Saturator {
    pub fn new(sample_rate: u32) -> Self {
        let sr = (sample_rate.max(8000)) as f32;
        Self {
            shape: Shape::Curve(Curve::Soft),
            factor: Factor::X2,
            drive: Smoothed::new(1.0, 15.0, sr),
            bias: Smoothed::new(0.0, 15.0, sr),
            output: Smoothed::new(1.0, 15.0, sr),
            mix: 1.0,
            tone: 1.0,
            sample_rate: sr,
            left: Channel::new(sr, Factor::X2, 1.0),
            right: Channel::new(sr, Factor::X2, 1.0),
            meter: choz_ports::FxMeter::default(),
        }
    }

    /// Build from the normalised knob positions the rack stores, in `params()`
    /// order: drive, curve, bias, tone, output, oversampling.
    pub fn with_params(sample_rate: u32, p: &[f32]) -> Self {
        let get = |i: usize, d: f32| p.get(i).copied().unwrap_or(d);
        let mut s = Self::new(sample_rate);
        s.set_drive(get(0, 0.3));
        s.set_curve(Curve::from_norm(get(1, 0.0)));
        s.set_bias(get(2, 0.5));
        s.set_tone(get(3, 1.0));
        s.set_output(get(4, 0.5));
        s.set_factor(Factor::from_norm(get(5, Factor::X2.to_norm())));
        s
    }

    /// Build the drawn-curve version from the rack's knob positions: the eight
    /// points first, then drive, tone, output and oversampling.
    ///
    /// It is the same processor — the oversampler, the DC blocker, the tone
    /// filter and the meter are what make a waveshaper usable, and they are
    /// already here. Only where the curve comes from changes.
    pub fn waveshaper(sample_rate: u32, p: &[f32]) -> Self {
        let get = |i: usize, d: f32| p.get(i).copied().unwrap_or(d);
        let mut s = Self::new(sample_rate);
        s.set_table(Table::from_norm(p));
        s.set_drive(get(TABLE_POINTS, 0.0));
        s.set_bias(0.5);
        s.set_tone(get(TABLE_POINTS + 1, 1.0));
        s.set_output(get(TABLE_POINTS + 2, 0.5));
        s.set_factor(Factor::from_norm(get(
            TABLE_POINTS + 3,
            Factor::X4.to_norm(),
        )));
        s
    }

    /// 0..1 → 1×..40× into the curve. Exponential, because the ear hears drive
    /// in ratios and a linear knob is all effect in the last quarter.
    pub fn set_drive(&mut self, v: f32) {
        self.drive.set_target(40.0f32.powf(v.clamp(0.0, 1.0)));
    }

    /// 0..1 with 0.5 centred: how far the curve is worked off-centre.
    pub fn set_bias(&mut self, v: f32) {
        self.bias.set_target((v.clamp(0.0, 1.0) - 0.5) * 1.2);
    }

    /// 0..1 → -12..+12 dB after the curve.
    pub fn set_output(&mut self, v: f32) {
        let db = (v.clamp(0.0, 1.0) - 0.5) * 24.0;
        self.output.set_target(10.0f32.powf(db / 20.0));
    }

    pub fn set_tone(&mut self, v: f32) {
        let v = v.clamp(0.0, 1.0);
        if (v - self.tone).abs() > 1e-4 {
            self.tone = v;
            self.left.tone = Tone::new(v, self.sample_rate);
            self.right.tone = Tone::new(v, self.sample_rate);
        }
    }

    pub fn set_curve(&mut self, curve: Curve) {
        self.shape = Shape::Curve(curve);
    }

    /// Use a drawn curve instead of a computed one.
    pub fn set_table(&mut self, table: Table) {
        self.shape = Shape::Table(table);
    }

    pub fn set_factor(&mut self, factor: Factor) {
        self.factor = factor;
    }

    pub fn curve(&self) -> Curve {
        match self.shape {
            Shape::Curve(c) => c,
            Shape::Table(_) => Curve::Soft,
        }
    }

    pub fn factor(&self) -> Factor {
        self.factor
    }

    /// Peak in and out of the last block, linear. The meter a gain stage needs
    /// to be usable: without it, "how hard am I driving this" is guesswork.
    pub fn levels(&self) -> (f32, f32) {
        self.meter.peaks()
    }
}

impl super::FxProcessor for Saturator {
    fn process_block(&mut self, buf: &mut [f32], sr: u32) {
        let sr = (sr.max(8000)) as f32;
        if (sr - self.sample_rate).abs() > 0.5 {
            self.sample_rate = sr;
            self.left = Channel::new(sr, self.factor, self.tone);
            self.right = Channel::new(sr, self.factor, self.tone);
            for s in [&mut self.drive, &mut self.bias, &mut self.output] {
                s.set_sample_rate(sr);
            }
        }
        self.left.os.configure(sr, self.factor);
        self.right.os.configure(sr, self.factor);

        let shape_fn = self.shape;
        let mix = self.mix;
        let (mut pin, mut pout) = (0.0f32, 0.0f32);

        for frame in buf.as_chunks_mut::<2>().0 {
            let drive = self.drive.tick();
            let bias = self.bias.tick();
            let out_gain = self.output.tick();
            // Compensating by the drive keeps the level where it was: a curve
            // is judged by its shape only when it is not also louder.
            let comp = 1.0 / (1.0 + (drive - 1.0) * 0.6).max(0.25);
            let shape = move |x: f32| shape_fn.apply(x * drive + bias) * comp;

            let (dry_l, dry_r) = (frame[0], frame[1]);
            pin = pin.max(dry_l.abs()).max(dry_r.abs());

            let mut wl = self.left.os.process(dry_l, shape);
            let mut wr = self.right.os.process(dry_r, shape);
            wl = self.left.tone.process(self.left.dc.process(wl)) * out_gain;
            wr = self.right.tone.process(self.right.dc.process(wr)) * out_gain;

            // A curve that returned a non-finite sample must not reach the
            // mixer: pass the dry signal instead of poisoning the bus.
            if !wl.is_finite() || !wr.is_finite() {
                self.left.os.reset();
                self.right.os.reset();
                wl = dry_l;
                wr = dry_r;
            }

            frame[0] = dry_l + mix * (wl - dry_l);
            frame[1] = dry_r + mix * (wr - dry_r);
            pout = pout.max(frame[0].abs()).max(frame[1].abs());
        }

        self.meter.publish(pin, pout);
    }

    fn reset(&mut self) {
        self.left = Channel::new(self.sample_rate, self.factor, self.tone);
        self.right = Channel::new(self.sample_rate, self.factor, self.tone);
        self.drive.snap(self.drive.target());
        self.bias.snap(self.bias.target());
        self.output.snap(self.output.target());
        self.meter.clear();
    }

    fn set_mix(&mut self, wet: f32) {
        self.mix = wet.clamp(0.0, 1.0);
    }

    fn meter(&self) -> Option<choz_ports::FxMeter> {
        Some(self.meter.clone())
    }

    fn name(&self) -> &str {
        match self.shape {
            Shape::Curve(_) => "Saturator",
            Shape::Table(_) => "WaveShaper",
        }
    }
}

/// Kept out of the struct: a `Biquad` import that only the tone control uses
/// would otherwise look unused.
const _: Option<Biquad> = None;

#[cfg(test)]
mod tests {
    use super::super::FxProcessor;
    use super::*;

    fn stereo(hz: f32, sr: f32, frames: usize, amp: f32) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let s = (2.0 * std::f32::consts::PI * hz * i as f32 / sr).sin() * amp;
                [s, s]
            })
            .collect()
    }

    fn peak(buf: &[f32]) -> f32 {
        buf.iter().fold(0.0f32, |m, s| m.max(s.abs()))
    }

    /// Every curve has to be bounded, at any drive, for any input — including
    /// inputs no sane signal reaches. An unbounded waveshaper takes the whole
    /// mix bus with it.
    #[test]
    fn every_curve_is_bounded_and_finite() {
        for curve in Curve::ALL {
            for x in [-100.0f32, -3.0, -1.0, 0.0, 0.5, 1.0, 3.0, 100.0] {
                let y = curve.apply(x);
                assert!(y.is_finite(), "{} at {x} gave {y}", curve.label());
                assert!(
                    y.abs() <= 2.0,
                    "{} at {x} left the rails: {y}",
                    curve.label()
                );
            }
            // And it passes through zero: a curve with an offset at silence is
            // a DC generator.
            assert!(curve.apply(0.0).abs() < 1e-6, "{}", curve.label());
        }
    }

    /// The curve you draw is the curve you get — and undrawn, it is the
    /// identity, so adding the effect and touching nothing changes nothing.
    #[test]
    fn a_drawn_curve_starts_as_a_wire() {
        let t = Table::default();
        for x in [-1.0f32, -0.5, -0.13, 0.0, 0.37, 1.0] {
            assert!(
                (t.apply(x) - x).abs() < 1e-5,
                "identity at {x}: {}",
                t.apply(x)
            );
        }
        // Flat outside the drawn range, never extrapolated.
        assert!((t.apply(9.0) - 1.0).abs() < 1e-5);
        assert!((t.apply(-9.0) + 1.0).abs() < 1e-5);

        // And through the whole processor: identity table, no drive, unity out.
        let mut params = vec![0.0f32; TABLE_POINTS];
        for (i, p) in params.iter_mut().enumerate() {
            *p = i as f32 / (TABLE_POINTS - 1) as f32;
        }
        params.extend_from_slice(&[0.0, 1.0, 0.5, Factor::X1.to_norm()]);
        let mut ws = Saturator::waveshaper(48000, &params);
        let mut buf = stereo(220.0, 48000.0, 2048, 0.5);
        let before = buf.clone();
        ws.process_block(&mut buf, 48000);
        // Not sample-identical and never will be: the 10 Hz DC blocker and the
        // 18 kHz tone filter are still in the path, and both cost a couple of
        // degrees of phase at 220 Hz. What must not change is the level.
        assert!(
            (peak(&buf) - peak(&before)).abs() < 0.02,
            "an undrawn shaper should not change the level: {} vs {}",
            peak(&buf),
            peak(&before)
        );
        let n = before.len();
        let worst = buf[n / 2..]
            .iter()
            .zip(before[n / 2..].iter())
            .fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
        assert!(
            worst < 0.05,
            "an undrawn shaper should be a wire, off by {worst}"
        );
    }

    /// Whatever gets drawn, the output stays on the rails and finite: the
    /// points are clamped and the curve is flat outside them.
    #[test]
    fn any_drawn_curve_is_bounded() {
        // The nastiest table there is: alternating rails, every segment a cliff.
        let mut params: Vec<f32> = (0..TABLE_POINTS)
            .map(|i| if i % 2 == 0 { 0.0 } else { 1.0 })
            .collect();
        params.extend_from_slice(&[1.0, 0.5, 1.0, Factor::X8.to_norm()]);
        let mut ws = Saturator::waveshaper(48000, &params);
        let mut buf = stereo(1000.0, 48000.0, 4096, 4.0);
        ws.process_block(&mut buf, 48000);
        assert!(buf.iter().all(|s| s.is_finite()));
        assert!(peak(&buf) < 8.0, "a drawn curve ran away: {}", peak(&buf));
        // And it says what it is, because the meter and the rack read the name.
        assert_eq!(ws.name(), "WaveShaper");
    }

    /// A curve drawn upside down inverts the signal — the drawing is doing the
    /// work, not the drive or the tone.
    #[test]
    fn an_inverted_curve_inverts_the_signal() {
        let mut params: Vec<f32> = (0..TABLE_POINTS)
            .map(|i| 1.0 - i as f32 / (TABLE_POINTS - 1) as f32)
            .collect();
        params.extend_from_slice(&[0.0, 1.0, 0.5, Factor::X1.to_norm()]);
        let mut ws = Saturator::waveshaper(48000, &params);
        let mut buf = stereo(220.0, 48000.0, 2048, 0.5);
        let before = buf.clone();
        ws.process_block(&mut buf, 48000);
        // Skip the first frames: the smoothers and the DC blocker settle.
        let n = before.len();
        let worst = buf[n / 2..]
            .iter()
            .zip(before[n / 2..].iter())
            .fold(0.0f32, |m, (a, b)| m.max((a + b).abs()));
        assert!(worst < 0.05, "expected the mirror image, off by {worst}");
    }

    #[test]
    fn silence_in_silence_out() {
        for curve in Curve::ALL {
            let mut s = Saturator::new(48_000);
            s.set_curve(curve);
            s.set_drive(1.0);
            s.set_bias(0.9); // the worst case: bias makes DC out of nothing
                             // Long enough for the 10 Hz blocker to settle: its time constant
                             // is ~16 ms, so a tenth of a second is six of them.
            let mut buf = vec![0.0f32; 16_384];
            s.process_block(&mut buf, 48_000);
            let tail = peak(&buf[12_288..]);
            assert!(tail < 1e-3, "{}: {tail}", curve.label());
        }
    }

    /// Drive makes it louder in harmonics, not in level: the compensation is
    /// what makes the curves comparable at all.
    #[test]
    fn the_output_stays_in_range_as_the_drive_goes_up() {
        for curve in Curve::ALL {
            for drive in [0.0f32, 0.5, 1.0] {
                let mut s = Saturator::new(48_000);
                s.set_curve(curve);
                s.set_drive(drive);
                let mut buf = stereo(220.0, 48_000.0, 4096, 0.7);
                s.process_block(&mut buf, 48_000);
                let p = peak(&buf);
                assert!(
                    p.is_finite() && p < 4.0,
                    "{} at {drive}: {p}",
                    curve.label()
                );
                assert!(p > 0.01, "{} at {drive} went silent", curve.label());
            }
        }
    }

    /// Wet at zero is the dry signal, sample for sample. The one property that
    /// makes an effect safe to leave in a chain.
    #[test]
    fn a_dry_mix_passes_the_input_through_untouched() {
        let mut s = Saturator::new(48_000);
        s.set_curve(Curve::Hard);
        s.set_drive(1.0);
        s.set_mix(0.0);
        let input = stereo(440.0, 48_000.0, 512, 0.8);
        let mut buf = input.clone();
        s.process_block(&mut buf, 48_000);
        assert_eq!(buf, input);
    }

    /// The thing the whole module exists for: at the same drive, more
    /// oversampling means less inharmonic rubbish. Measured at 13 kHz, where
    /// the 7th harmonic of a 5 kHz tone reflects to.
    #[test]
    fn more_oversampling_means_less_aliasing() {
        let sr = 48_000.0;
        let alias = |factor: Factor| {
            let mut s = Saturator::new(48_000);
            s.set_curve(Curve::Hard);
            s.set_drive(0.8);
            s.set_tone(1.0);
            s.set_factor(factor);
            let mut buf = stereo(5_000.0, sr, 8192, 0.9);
            s.process_block(&mut buf, 48_000);
            let left: Vec<f32> = buf
                .as_chunks::<2>()
                .0
                .iter()
                .skip(1024)
                .map(|f| f[0])
                .collect();
            let w = 2.0 * std::f32::consts::PI * 13_000.0 / sr;
            let coeff = 2.0 * w.cos();
            let (mut s1, mut s2) = (0.0f32, 0.0f32);
            for &x in &left {
                let s0 = x + coeff * s1 - s2;
                s2 = s1;
                s1 = s0;
            }
            (s1 * s1 + s2 * s2 - coeff * s1 * s2).max(0.0).sqrt() / left.len() as f32
        };
        let (x1, x8) = (alias(Factor::X1), alias(Factor::X8));
        assert!(x8 < x1 * 0.5, "8x did not help: {x8} vs {x1}");
    }

    /// Bias is what makes even harmonics, and the DC blocker is what stops it
    /// costing headroom. Both, in one measurement: the mean must come back to
    /// zero even though the curve is being worked off-centre.
    #[test]
    fn bias_leaves_no_dc_behind() {
        let mut s = Saturator::new(48_000);
        s.set_curve(Curve::Tube);
        s.set_drive(0.7);
        s.set_bias(1.0);
        let mut buf = stereo(110.0, 48_000.0, 16_384, 0.6);
        s.process_block(&mut buf, 48_000);
        let tail = &buf[8192..];
        let mean = tail.iter().sum::<f32>() / tail.len() as f32;
        assert!(mean.abs() < 0.01, "DC left in the output: {mean}");
    }

    /// Block size must not change the sound: a per-block reset or a smoother
    /// that restarts would show up here as a different output.
    #[test]
    fn the_block_size_does_not_change_the_result() {
        let input = stereo(330.0, 48_000.0, 4096, 0.7);
        let render = |block: usize| {
            let mut s = Saturator::with_params(
                48_000,
                &[
                    0.6,
                    Curve::Tape.to_norm(),
                    0.5,
                    0.8,
                    0.5,
                    Factor::X4.to_norm(),
                ],
            );
            let mut buf = input.clone();
            for chunk in buf.chunks_mut(block * 2) {
                s.process_block(chunk, 48_000);
            }
            buf
        };
        let a = render(64);
        let b = render(512);
        for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
            assert!((x - y).abs() < 1e-5, "sample {i}: {x} vs {y}");
        }
    }

    /// Odd shapes a host can hand a processor: zero frames, one frame, and a
    /// rate it was not built for.
    #[test]
    fn it_survives_empty_blocks_and_a_rate_change() {
        let mut s = Saturator::new(48_000);
        s.process_block(&mut [], 48_000);
        let mut one = [0.5f32, -0.5];
        s.process_block(&mut one, 48_000);
        assert!(one.iter().all(|x| x.is_finite()));

        let mut buf = stereo(440.0, 96_000.0, 1024, 0.8);
        s.process_block(&mut buf, 96_000);
        assert!(buf.iter().all(|x| x.is_finite()));
        s.reset();
        let mut buf = stereo(440.0, 44_100.0, 1024, 0.8);
        s.process_block(&mut buf, 44_100);
        assert!(buf.iter().all(|x| x.is_finite()));
    }

    /// A meter that reads zero is a meter nobody trusts.
    #[test]
    fn the_meters_read_what_went_in_and_out() {
        let mut s = Saturator::new(48_000);
        s.set_drive(0.9);
        let mut buf = stereo(440.0, 48_000.0, 2048, 0.5);
        s.process_block(&mut buf, 48_000);
        let (pin, pout) = s.levels();
        assert!((pin - 0.5).abs() < 0.02, "input peak: {pin}");
        assert!(pout > 0.0 && pout.is_finite(), "output peak: {pout}");
    }

    /// Curve and factor are selected by knobs, so both round trips must be
    /// exact — a curve that drifts one position on save is a different effect.
    #[test]
    fn the_curve_survives_the_knob() {
        for c in Curve::ALL {
            assert_eq!(Curve::from_norm(c.to_norm()), c, "{}", c.label());
        }
    }
}
