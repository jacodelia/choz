//! The one delay line every modulated effect in this crate reads from.
//!
//! Written once here because a delay line copied into four files is four delay
//! lines that drift into different bugs — and they had. Before this, the chorus
//! and the flanger each carried their own, both sized in **samples** rather than
//! in time (`const MAX_DELAY_SAMPLES: usize = 4096`), so the same patch was a
//! different effect at 96 kHz than at 44.1, and both read with linear
//! interpolation *inside a feedback loop*, which is a low-pass applied on every
//! pass.
//!
//! # What it gives you
//!
//! - **A capacity in milliseconds, not samples.** Sized for the highest rate
//!   choz supports, so a buffer built once holds the same *time* whatever
//!   device turns up. Nothing here has to be rebuilt on a rate change.
//! - **Two reads.** [`DelayLine::read`] interpolates linearly and
//!   [`DelayLine::read_cubic`] with a Catmull-Rom through four points. The
//!   choice is not about taste: see below.
//! - **A flush on every write**, so a decaying feedback path reaches zero
//!   instead of grinding through denormals.
//!
//! # Which read to use
//!
//! Linear interpolation is a two-tap average, which is a low-pass. At a
//! half-sample offset it is **3.0 dB** down at a quarter of the sample rate and
//! 0.68 dB down at an eighth; the four-point cubic is 1.1 dB and 0.08 dB in the
//! same places. Read once and passed on, either is nothing. Read **inside a
//! feedback loop** it is applied on every pass, so the top of the signal dies
//! many times faster than the bottom — a flanger at 0.9 feedback goes dull, and
//! worse, its timbre changes as the LFO sweeps because the fractional part
//! sweeps with it.
//!
//! So: `read` for a tap that leaves, `read_cubic` for anything that comes back.
//!
//! # Real-time
//!
//! The buffer is allocated in `new`. Everything else is loads, stores and
//! arithmetic; the wrap is an `AND`, because the capacity is a power of two.
//!
//! The indexing is **checked**. An earlier version used `get_unchecked` on the
//! grounds that this is the hottest loop in the crate and every index is
//! provably masked into range; measured against the reverb benchmark — 33 reads
//! a frame across eight feedback lines — it bought 0.018 % of a core, 1.340 %
//! against 1.358 %. Four `unsafe` blocks in a feedback path is not a trade
//! worth making for a fiftieth of a percent, so the bounds checks stayed.

/// The highest sample rate choz sizes its buffers for. Above this a device
/// still works — the delay times available just stop growing.
pub const MAX_RATE: f32 = 192_000.0;

/// Anything that is not a usable number becomes zero.
///
/// One test, three jobs: a NaN that got in from outside cannot circulate, an
/// infinity cannot, and a denormal — which costs up to a hundred times a normal
/// multiply on x86, and which every decaying feedback path eventually becomes —
/// is flushed before it reaches the multiplier. An effect that gets *more*
/// expensive once it goes quiet is the one thing a host cannot schedule around.
///
/// −500 dB is where the floor is set, which is silence by any measure.
#[inline(always)]
pub fn safe(v: f32) -> f32 {
    let a = v.abs();
    if a > 1e-25 && a < 1e12 {
        v
    } else {
        0.0
    }
}

/// Where a soft clip stops being the identity, as a fraction of its ceiling.
const SOFT_KNEE: f32 = 0.7;

/// Soft limit: exactly the identity below the knee, a smooth bend above it,
/// bounded at `ceiling`.
///
/// Exactly, not approximately — a bare `tanh` is 3 % low at half scale, so it
/// colours everything that passes through it whether it needed limiting or not.
/// Below `0.7 × ceiling` this returns its argument unchanged, and only something
/// that would otherwise wind a feedback path up is bent.
///
/// A hard clip in the same place would fold harmonics into the loop and they
/// would never leave.
#[inline(always)]
pub fn soft_clip(x: f32, ceiling: f32) -> f32 {
    let n = x / ceiling;
    let a = n.abs();
    if a <= SOFT_KNEE {
        return x;
    }
    let over = (a - SOFT_KNEE) / (1.0 - SOFT_KNEE);
    let bent = SOFT_KNEE + (1.0 - SOFT_KNEE) * over.tanh();
    bent.copysign(n) * ceiling
}

/// A cubic that behaves like a sine and costs three multiplies.
///
/// `phase` runs 0..1. Zero at both ends with the same slope at each, so it is
/// smooth across the wrap — the only property a modulation LFO needs. It is not
/// a sine and does not have to be: what matters is that it is bounded, has no
/// corner in it, and does not cost a transcendental per sample. Two of these
/// replaced 96 000 `sin()` calls a second in the chorus alone.
#[inline(always)]
pub fn wobble(phase: f32) -> f32 {
    let t = phase * 2.0 - 1.0;
    2.598_076 * t * (1.0 - t * t)
}

/// A power-of-two delay line read at a fractional distance.
pub struct DelayLine {
    buf: Vec<f32>,
    mask: usize,
    write: usize,
}

impl DelayLine {
    /// A line that can hold `ms` milliseconds **at every rate choz supports**.
    ///
    /// Sized against [`MAX_RATE`] and not against the rate it happens to be
    /// built at, which is what makes the effect above it sound the same on
    /// every device. The cost of that is memory at low rates — 45 ms is 8 640
    /// samples at 192 kHz and 35 KB, which is nothing next to being a different
    /// effect on a different interface.
    pub fn with_ms(ms: f32) -> Self {
        Self::with_samples((ms * 0.001 * MAX_RATE) as usize)
    }

    pub fn with_samples(min: usize) -> Self {
        let cap = (min + 4).next_power_of_two().max(8);
        Self {
            buf: vec![0.0; cap],
            mask: cap - 1,
            write: 0,
        }
    }

    /// The longest delay this line can be read at, in samples.
    pub fn capacity(&self) -> usize {
        self.buf.len()
    }

    /// Linearly interpolated. For a tap that leaves the effect.
    #[inline(always)]
    pub fn read(&self, distance: f32) -> f32 {
        let max = (self.buf.len() - 2) as f32;
        let d = distance.clamp(1.0, max);
        let whole = d as usize;
        let frac = d - whole as f32;
        let i0 = (self.write + self.buf.len() - whole) & self.mask;
        let i1 = (i0 + self.mask) & self.mask;
        let a = self.buf[i0];
        let b = self.buf[i1];
        a + (b - a) * frac
    }

    /// Catmull-Rom through four points. For anything that is fed back.
    #[inline(always)]
    pub fn read_cubic(&self, distance: f32) -> f32 {
        let cap = self.buf.len();
        let max = (cap - 4) as f32;
        let d = distance.clamp(2.0, max);
        let whole = d as usize;
        let f = d - whole as f32;
        let i1 = (self.write + cap - whole) & self.mask;
        let i0 = (i1 + 1) & self.mask;
        let i2 = (i1 + self.mask) & self.mask;
        let i3 = (i2 + self.mask) & self.mask;
        let (y0, y1, y2, y3) = (self.buf[i0], self.buf[i1], self.buf[i2], self.buf[i3]);
        // Horner form. At `f == 0` this is exactly `y1`, so an integer distance
        // is lossless — which is what lets a frozen or unmodulated loop hold.
        let c1 = 0.5 * (y2 - y0);
        let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
        let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
        ((c3 * f + c2) * f + c1) * f + y1
    }

    /// Store one sample and advance. Flushed on the way in — see [`safe`].
    #[inline(always)]
    pub fn write(&mut self, v: f32) {
        self.buf[self.write] = safe(v);
        self.write = (self.write + 1) & self.mask;
    }

    pub fn clear(&mut self) {
        self.buf.fill(0.0);
        self.write = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of sizing in time: a line asked for 40 ms has 40 ms at
    /// every rate, so an effect built on it is the same effect on every device.
    ///
    /// The old chorus failed this — 4096 samples is 93 ms at 44.1 kHz and 21 ms
    /// at 192 kHz, so its delay and depth were silently clamped to a quarter of
    /// what was asked for on a fast interface.
    #[test]
    fn the_delay_line_is_the_same_time_at_every_rate() {
        let line = DelayLine::with_ms(45.0);
        for sr in [44_100.0f32, 48_000.0, 96_000.0, 192_000.0] {
            let want = 45.0 * 0.001 * sr;
            assert!(
                line.capacity() as f32 >= want + 4.0,
                "{sr} Hz needs {want} samples, the line holds {}",
                line.capacity()
            );
        }
    }

    /// A delayed impulse comes back where it was put, and at full height.
    #[test]
    fn it_delays_by_what_it_was_asked_for() {
        let mut line = DelayLine::with_samples(64);
        line.write(1.0);
        for _ in 0..9 {
            line.write(0.0);
        }
        // Ten samples written, so the impulse is now ten back.
        assert_eq!(line.read(10.0), 1.0);
        assert_eq!(line.read_cubic(10.0), 1.0);
        assert_eq!(line.read(11.0), 0.0);
        // Half way between: linear splits it, the cubic overshoots slightly the
        // way an interpolating polynomial does. Both are bounded.
        assert!((line.read(10.5) - 0.5).abs() < 1e-6);
        assert!(line.read_cubic(10.5) > 0.5 && line.read_cubic(10.5) < 0.8);
    }

    /// The reason `read_cubic` exists, in decibels per pass.
    ///
    /// A half-sample offset read over and over is what a feedback loop does. At
    /// a *quarter* of the sample rate the two-tap average is 3.0 dB down and
    /// the four-point cubic is 1.1 dB down; at an *eighth* — 6 kHz at 48 k,
    /// where a flanger's resonance lives — linear is 0.68 dB down and the cubic
    /// is 0.08. Read once that is nothing either way. Read twelve times round a
    /// loop it is the difference between a resonance and a thud.
    #[test]
    fn a_cubic_read_keeps_the_top_end_a_linear_one_loses() {
        // `period` samples per cycle: 4 is fs/4, 8 is fs/8.
        let loss_db = |cubic: bool, period: usize| {
            let mut line = DelayLine::with_samples(256);
            let tone: Vec<f32> = (0..4096)
                .map(|i| (std::f32::consts::TAU * i as f32 / period as f32).sin())
                .collect();
            let mut x = tone.clone();
            let passes = 12;
            for _ in 0..passes {
                line.clear();
                let mut out = Vec::with_capacity(x.len());
                for &s in x.iter() {
                    line.write(s);
                    // Deliberately fractional, as a swept modulation spends
                    // most of its time being.
                    out.push(match cubic {
                        true => line.read_cubic(8.5),
                        false => line.read(8.5),
                    });
                }
                x = out;
            }
            let rms = |v: &[f32]| {
                (v[512..].iter().map(|s| s * s).sum::<f32>() / (v.len() - 512) as f32).sqrt()
            };
            -20.0 * (rms(&x) / rms(&tone)).log10() / passes as f32
        };

        let (lin4, cub4) = (loss_db(false, 4), loss_db(true, 4));
        let (lin8, cub8) = (loss_db(false, 8), loss_db(true, 8));
        assert!(
            (2.5..3.5).contains(&lin4) && (0.8..1.4).contains(&cub4),
            "at fs/4: linear {lin4:.2} dB/pass, cubic {cub4:.2} dB/pass"
        );
        assert!(
            (0.5..0.9).contains(&lin8) && cub8 < 0.15,
            "at fs/8: linear {lin8:.2} dB/pass, cubic {cub8:.2} dB/pass"
        );
        assert!(cub8 * 4.0 < lin8, "the cubic has to win where it matters");
    }

    /// A feedback path has to reach zero, not a denormal.
    #[test]
    fn the_feedback_path_reaches_true_silence() {
        let mut line = DelayLine::with_samples(64);
        line.write(1.0);
        // Decay it round a loop until it would be a denormal.
        for _ in 0..2000 {
            let v = line.read(8.0) * 0.5;
            line.write(v);
        }
        assert_eq!(line.read(8.0), 0.0, "it never reached silence");
    }

    #[test]
    fn nothing_that_is_not_a_number_survives_a_write() {
        let mut line = DelayLine::with_samples(16);
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1e-30] {
            line.write(bad);
            assert_eq!(line.read(1.0), 0.0, "{bad} got through");
        }
    }

    /// The soft clip does nothing at all where it says it does nothing.
    #[test]
    fn the_soft_clip_is_transparent_below_its_knee() {
        for ceiling in [1.0f32, 4.0] {
            for x in [0.0f32, 0.1, ceiling * SOFT_KNEE] {
                assert_eq!(soft_clip(x, ceiling), x, "bent at {x} of {ceiling}");
                assert_eq!(soft_clip(-x, ceiling), -x);
            }
            assert!(soft_clip(1e6, ceiling) <= ceiling + 1e-3);
            assert!(soft_clip(-1e6, ceiling) >= -ceiling - 1e-3);
        }
    }

    /// The LFO has no corner in it, which is what a delay-line modulation needs
    /// — a step in the read distance is a click.
    ///
    /// "No corner" is not "small steps": the curve is *steepest* at the wrap,
    /// which is fine. What matters is that the slope is the **same** either
    /// side of it, so the read head does not change direction abruptly.
    #[test]
    fn the_wobble_is_bounded_and_smooth_across_the_wrap() {
        let n = 4096.0;
        for i in 0..n as usize {
            let v = wobble(i as f32 / n);
            assert!(v.abs() <= 1.001, "unbounded at {i}: {v}");
        }
        let step = |p: f32| wobble((p + 1.0 / n) % 1.0) - wobble(p);
        let before = step(1.0 - 2.0 / n);
        let across = step(1.0 - 1.0 / n);
        let after = step(0.0);
        assert!(
            (across - before).abs() < 1e-4 && (after - across).abs() < 1e-4,
            "the slope jumps at the wrap: {before:.6} {across:.6} {after:.6}"
        );
    }
}
