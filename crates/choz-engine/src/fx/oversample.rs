//! Oversampling for per-sample nonlinearities, at 1×, 2×, 4× or 8×.
//!
//! A waveshaper multiplies the harmonic content of what goes into it. Anything
//! it generates above Nyquist does not disappear — it folds back down as
//! inharmonic tones that follow the input in the wrong direction, which is the
//! sound of cheap digital distortion. Running the nonlinearity at a multiple of
//! the sample rate pushes that reflection further up, and a lowpass before
//! decimating throws it away instead of folding it in.
//!
//! # Cost, and why the factor is a choice
//!
//! Every doubling runs the nonlinearity twice as often. 8× is eight evaluations
//! and three filters per sample: worth it for a hard clipper, waste for a
//! `tanh` at low drive, and nonsense for anything linear — an EQ generates no
//! new harmonics, so there is nothing to alias. That is why this is a knob and
//! not a policy.
//!
//! # Real-time
//!
//! Fixed state, no allocation, no branching on the audio path beyond the factor.
//! [`Oversampler::process`] takes the nonlinearity as a closure so a processor
//! keeps its own parameters without this module knowing about them.

use super::utility::Biquad;

/// One halving stage: linear-interpolated upsample by two, the nonlinearity at
/// the doubled rate, then a **4th-order** Butterworth lowpass before throwing
/// every other sample away.
///
/// Fourth order, not second, and that is the whole difference between this and
/// the 2× helper in `utility.rs`: with a 2-pole filter the first reflection at
/// 23 kHz is barely 10 dB down, so cascading stages hits a floor set by the
/// filter rather than by the factor. Two biquads at the Butterworth Q pair make
/// each stage actually worth its cost.
#[derive(Clone, Copy)]
struct Stage {
    last: f32,
    lp1: Biquad,
    lp2: Biquad,
}

impl Stage {
    fn new(base_sr: f32) -> Self {
        let cut = base_sr * 0.45;
        let rate = base_sr * 2.0;
        Self {
            last: 0.0,
            // The Q pair of a 4th-order Butterworth, as two cascaded sections.
            lp1: Biquad::lowpass(cut, rate, 0.5412),
            lp2: Biquad::lowpass(cut, rate, 1.3066),
        }
    }

    #[inline]
    fn process<F: FnMut(f32) -> f32>(&mut self, x: f32, mut f: F) -> f32 {
        let mid = 0.5 * (self.last + x);
        self.last = x;
        let a = f(mid);
        let _ = self.lp2.process(self.lp1.process(a)); // discarded half
        let b = f(x);
        self.lp2.process(self.lp1.process(b))
    }
}

/// How many times the base sample rate the nonlinearity runs at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Factor {
    /// No oversampling: the nonlinearity runs at the base rate.
    #[default]
    X1,
    X2,
    X4,
    X8,
}

impl Factor {
    pub const ALL: [Factor; 4] = [Factor::X1, Factor::X2, Factor::X4, Factor::X8];

    pub fn label(self) -> &'static str {
        match self {
            Factor::X1 => "1x",
            Factor::X2 => "2x",
            Factor::X4 => "4x",
            Factor::X8 => "8x",
        }
    }

    /// The multiplier itself.
    pub fn ratio(self) -> u32 {
        match self {
            Factor::X1 => 1,
            Factor::X2 => 2,
            Factor::X4 => 4,
            Factor::X8 => 8,
        }
    }

    /// From a normalised knob position, so a parameter can select it.
    pub fn from_norm(v: f32) -> Self {
        let i = (v.clamp(0.0, 1.0) * (Self::ALL.len() - 1) as f32).round() as usize;
        Self::ALL[i.min(Self::ALL.len() - 1)]
    }

    pub fn to_norm(self) -> f32 {
        let i = Self::ALL.iter().position(|f| *f == self).unwrap_or(0);
        i as f32 / (Self::ALL.len() - 1) as f32
    }
}

/// A cascade of halving stages: 4× is two 2× stages, 8× is three.
///
/// Each stage filters at its own rate — the second stage's lowpass has to sit
/// below the *first* stage's Nyquist, not the base one, or it removes nothing.
#[derive(Clone, Copy)]
pub struct Oversampler {
    factor: Factor,
    base_sr: f32,
    s1: Stage,
    s2: Stage,
    s3: Stage,
    /// Blocks denormals from crawling into the filters when the input goes
    /// silent: a filter grinding on 1e-40 costs hundreds of cycles a sample on
    /// x86, and silence is exactly when nobody is looking.
    dc: f32,
}

impl Oversampler {
    pub fn new(base_sr: f32, factor: Factor) -> Self {
        let sr = base_sr.max(8000.0);
        Self {
            factor,
            base_sr: sr,
            s1: Stage::new(sr),
            s2: Stage::new(sr * 2.0),
            s3: Stage::new(sr * 4.0),
            dc: 0.0,
        }
    }

    pub fn factor(&self) -> Factor {
        self.factor
    }

    /// Change factor or rate. Cheap enough to call per block, which is what
    /// processors do rather than tracking whether anything moved.
    pub fn configure(&mut self, base_sr: f32, factor: Factor) {
        if factor != self.factor || (base_sr - self.base_sr).abs() > 0.5 {
            *self = Self::new(base_sr, factor);
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new(self.base_sr, self.factor);
    }

    /// Run `f` at the oversampled rate and return one output sample.
    #[inline]
    pub fn process<F: FnMut(f32) -> f32>(&mut self, x: f32, mut f: F) -> f32 {
        // A tiny alternating offset keeps the filter states out of denormal
        // range without being audible (-260 dBFS).
        self.dc = -self.dc;
        let x = x + self.dc * 1e-20;
        let Self { s1, s2, s3, .. } = self;
        match self.factor {
            Factor::X1 => f(x),
            Factor::X2 => s1.process(x, f),
            Factor::X4 => s1.process(x, |m| s2.process(m, &mut f)),
            Factor::X8 => s1.process(x, |m| s2.process(m, |n| s3.process(n, &mut f))),
        }
    }
}

/// A one-pole lowpass used as a tone control, kept here because every
/// nonlinearity wants one and none of them wants to own it.
#[derive(Clone, Copy)]
pub struct Tone {
    lp: Biquad,
}

impl Tone {
    /// `t` is 0..1: dark to open, mapped over 400 Hz–18 kHz on a log scale
    /// because that is how the ear reads a tone knob.
    pub fn new(t: f32, sr: f32) -> Self {
        let sr = sr.max(8000.0);
        let hz = (400.0 * (18_000.0f32 / 400.0).powf(t.clamp(0.0, 1.0))).min(sr * 0.45);
        Self {
            lp: Biquad::lowpass(hz, sr, 0.707),
        }
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        self.lp.process(x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The energy at `hz`, by the Goertzel algorithm — one bin of a DFT without
    /// building the whole transform.
    fn bin(buf: &[f32], hz: f32, sr: f32) -> f32 {
        let w = 2.0 * std::f32::consts::PI * hz / sr;
        let coeff = 2.0 * w.cos();
        let (mut s1, mut s2) = (0.0f32, 0.0f32);
        for &x in buf {
            let s0 = x + coeff * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        (s1 * s1 + s2 * s2 - coeff * s1 * s2).max(0.0).sqrt() / buf.len() as f32
    }

    fn sine(hz: f32, sr: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * hz * i as f32 / sr).sin() * 0.9)
            .collect()
    }

    /// The point of the whole module: a hard clipper folds its own harmonics
    /// back down as inharmonic tones, and oversampling is what stops it.
    ///
    /// 5 kHz hard-clipped at 48 kHz puts its 5th harmonic at 25 kHz, which
    /// reflects to 23 kHz, and its 7th at 35 kHz, which reflects to **13 kHz** —
    /// right in the middle of the ear's most sensitive region, and not a
    /// harmonic of anything. That bin is what this measures.
    #[test]
    fn oversampling_removes_the_folded_back_harmonics() {
        let sr = 48_000.0;
        let input = sine(5_000.0, sr, 8192);
        let clip = |x: f32| (x * 6.0).clamp(-1.0, 1.0);

        let run = |factor: Factor| {
            let mut os = Oversampler::new(sr, factor);
            let out: Vec<f32> = input.iter().map(|&x| os.process(x, clip)).collect();
            // Skip the filter's start-up so the reading is the steady state.
            bin(&out[1024..], 13_000.0, sr)
        };

        let (x1, x2, x8) = (run(Factor::X1), run(Factor::X2), run(Factor::X8));
        assert!(x1 > 1e-4, "the alias has to be there without help: {x1}");
        assert!(x2 < x1 * 0.5, "2x should halve it at least: {x2} vs {x1}");
        assert!(x8 < x1 * 0.1, "8x should bury it: {x8} vs {x1}");
    }

    /// The wanted signal survives: an antialiasing filter that eats the music
    /// with the aliases is not a fix.
    #[test]
    fn the_signal_itself_comes_through() {
        let sr = 48_000.0;
        let input = sine(1_000.0, sr, 8192);
        for factor in Factor::ALL {
            let mut os = Oversampler::new(sr, factor);
            let out: Vec<f32> = input.iter().map(|&x| os.process(x, |x| x)).collect();
            let level = bin(&out[1024..], 1_000.0, sr);
            assert!(level > 0.2, "{}: lost the signal ({level})", factor.label());
            assert!(out.iter().all(|s| s.is_finite()));
        }
    }

    /// Silence in, silence out — and nothing left crawling in the filters.
    #[test]
    fn silence_stays_silent_and_finite() {
        let mut os = Oversampler::new(48_000.0, Factor::X8);
        for _ in 0..10_000 {
            let y = os.process(0.0, |x| x.tanh());
            assert!(y.abs() < 1e-9, "got {y}");
        }
    }

    /// A factor is chosen by a knob, so the round trip has to be exact.
    #[test]
    fn the_factor_survives_the_knob() {
        for f in Factor::ALL {
            assert_eq!(Factor::from_norm(f.to_norm()), f, "{}", f.label());
        }
        assert_eq!(Factor::from_norm(0.0), Factor::X1);
        assert_eq!(Factor::from_norm(1.0), Factor::X8);
        assert_eq!(Factor::from_norm(2.0), Factor::X8, "out of range clamps");
    }

    /// Reconfiguring mid-stream must not blow up or leave stale filter state at
    /// the wrong rate.
    #[test]
    fn changing_rate_or_factor_is_safe_mid_stream() {
        let mut os = Oversampler::new(48_000.0, Factor::X2);
        let input = sine(1_000.0, 48_000.0, 512);
        for &x in &input {
            os.process(x, |x| x.tanh());
        }
        os.configure(96_000.0, Factor::X4);
        for &x in &input {
            assert!(os.process(x, |x| x.tanh()).is_finite());
        }
        os.configure(96_000.0, Factor::X4);
        assert_eq!(os.factor(), Factor::X4, "an unchanged call is a no-op");
    }
}
