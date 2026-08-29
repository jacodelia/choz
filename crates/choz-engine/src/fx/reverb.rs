//! Choz reverb — a modulated feedback delay network with its own early
//! reflections, diffusion, in-loop damping and stereo decorrelation.
//!
//! This replaces the Freeverb-derived Schroeder bank that used to live here. A
//! bank of parallel combs is cheap and it is 1997: each comb is a resonator at
//! a fixed set of frequencies, so its tail is a chord of its own delay lengths,
//! and eight of them per channel is eight chords. That is the metallic ring —
//! and no number of extra combs cures it, because every comb added is another
//! chord. It is also why the old code needed `input * 0.015`: eight resonators
//! in parallel have a resonant gain nobody worked out, so the input was scaled
//! down until it stopped exploding.
//!
//! # The signal path
//!
//! ```text
//! in ─▶ mono send ─▶ pre-delay ─┬─▶ early reflections ──────────────┐
//!                               │                                   │
//!                               └─▶ input diffusion ─▶ FDN ─▶ late ─┤
//!                                                                   ▼
//!  dry ────────────────────────────────────────────▶ + ◀── width ◀ tone/cuts
//! ```
//!
//! ## The late network
//!
//! An **FDN** (Jot & Chaigne, 1991): `N` delay lines whose outputs are mixed by
//! an orthogonal matrix and fed back. Orthogonal is the whole point — it is a
//! rotation, so it moves energy between the lines without creating or
//! destroying any, and the decay is then set *only* by the per-line gains. That
//! is what makes an explicit RT60 possible and what makes the network stable by
//! construction rather than by trimming the input until it behaves.
//!
//! The matrix here is a Householder reflection, `I − (2/N)·11ᵀ`, composed with a
//! diagonal of ±1 and a rotation of the line order. Each factor is orthogonal,
//! so the product is; the reflection alone is an involution (`H² = I`) and the
//! other two are what stop the network from undoing its own mixing every second
//! pass. The reflection costs one sum and one multiply-add per line, not `N²`.
//!
//! ## Why the lines are the lengths they are
//!
//! A delay line of `T` seconds resonates at every multiple of `1/T`. Two lines
//! whose lengths are in a simple ratio share most of those modes, and a shared
//! mode is a frequency the tail sings at. So the eight ratios are spread
//! geometrically over 1 : 3.23 — geometric because that spaces the *modal
//! densities* evenly rather than the lengths — and then pushed off the exact
//! geometric grid, so no pair is near a small rational. Nothing is derived from
//! Freeverb's table.
//!
//! ## Decay
//!
//! `g_i = 10^(−3·T_i/RT60)` is the exact per-pass gain for a −60 dB decay in
//! `RT60` seconds, so `Decay` is a *time* and not a feedback number that
//! happens to sound long. `Size` moves `T_i`; the gains follow, which is why
//! changing the room does not change how long it rings.
//!
//! The damping and low-cut filters sit **inside** the loop, so they are applied
//! once per pass: highs lose a little every time round and the tail darkens as
//! it decays, the way a real room does. A filter at the output could only make
//! the whole tail darker, never *progressively* darker.
//!
//! # Real-time
//!
//! Every buffer is allocated in [`Reverb::new`]. `process_block` allocates
//! nothing, locks nothing and branches only on the line count. Every
//! coefficient that moves is a [`Smoothed`] ticked once per sample — never once
//! per block — so the output does not depend on the block size at all.

use super::delay_line::{safe, soft_clip, wobble, DelayLine as Line};
use super::smooth::Smoothed;
use super::FxProcessor;

// ─── The shape of the network ───────────────────────────────────────────────

/// Late delay lines the network can run. The buffers are always this many;
/// [`Quality`] only changes how many are *used*, so switching it in the middle
/// of a set allocates nothing.
const MAX_LINES: usize = 8;
/// What Economy runs. The ratios below are ordered so that the first four
/// already span the whole range of lengths — Economy is a smaller network, not
/// a shorter one.
const ECON_LINES: usize = 4;

/// Line lengths as multiples of the shortest one.
///
/// Geometrically spread over 1 : 3.23, then nudged off the exact grid so that
/// no two are in a near-rational ratio: a shared mode between two lines is a
/// frequency the tail sings at. The order is *not* ascending — the first four
/// span the range, which is what Economy uses.
const LINE_RATIOS: [f32; MAX_LINES] = [1.000, 1.371, 2.351, 3.229, 1.213, 1.669, 1.913, 2.677];

/// The ±1 diagonal in front of the feedback matrix. Orthogonal on its own, and
/// what stops the Householder reflection from being its own inverse.
const LINE_SIGNS: [f32; MAX_LINES] = [1.0, -1.0, 1.0, 1.0, -1.0, 1.0, -1.0, -1.0];

/// How far the line order rotates each pass. Coprime with the line count in
/// both cases (3 with 8, 1 with 4), so the rotation is a single cycle and every
/// line reaches every other one.
const ROTATE_HIGH: usize = 3;
const ROTATE_ECON: usize = 1;

/// How much of each line goes to the left output, and to the right.
///
/// **Positive on both sides, and different rather than opposite.** Sign-flipped
/// taps are the cheap way to get a wide reverb and the reason so many of them
/// collapse in mono: what the width gained, the fold-down cancels. These lean
/// alternate lines to alternate sides instead, so the two outputs are built
/// from largely *different* — and therefore uncorrelated — audio. Summing them
/// adds energy rather than removing it. Measured across the five characters the
/// two outputs come out essentially uncorrelated (|r| < 0.11) and the fold-down
/// costs 2.6–3.4 dB — which is the arithmetic ideal for two uncorrelated
/// channels, and is what an opposite-sign pair trades for a null.
const OUT_L: [f32; MAX_LINES] = [1.00, 0.20, 0.92, 0.26, 0.84, 0.15, 0.96, 0.22];
const OUT_R: [f32; MAX_LINES] = [0.20, 1.00, 0.26, 0.92, 0.15, 0.84, 0.22, 0.96];

/// Modulation rates, Hz. All under half a hertz and none a multiple of another,
/// so the lines never come into step. Slow on purpose: fast enough to hear is a
/// chorus, and this is here to break up modes, not to be an effect.
const MOD_RATES: [f32; MAX_LINES] = [0.109, 0.163, 0.211, 0.277, 0.131, 0.191, 0.243, 0.317];

/// Early reflection times in milliseconds at `Size` = 0.5, one set a side.
///
/// A first reflection arrives at `2d/c` for a surface `d` away, so a room a few
/// metres across puts its first arrivals between roughly 8 and 90 ms — that
/// range is the room's size, and it is what the ear reads as one. Spaced so no
/// two taps land within about 4 ms of each other (closer than that they fuse
/// into one arrival with a comb notch between them), and the two sides are
/// offset from each other throughout: **no reflection arrives at both ears at
/// once**, which is the whole of the stereo image at this stage. Ordered so the
/// first four span the range, for Economy.
const ER_L_MS: [f32; MAX_LINES] = [9.7, 22.1, 41.3, 68.3, 15.3, 30.7, 53.9, 84.7];
const ER_R_MS: [f32; MAX_LINES] = [12.1, 26.3, 46.7, 74.9, 18.7, 35.9, 59.3, 91.3];
/// Polarities for those taps. A set of arrivals all the same way up combs; a
/// fixed scatter of signs does not, and no ear can tell which reflection was
/// inverted.
const ER_SIGNS: [f32; MAX_LINES] = [1.0, 1.0, -1.0, 1.0, -1.0, 1.0, 1.0, -1.0];
/// How fast a reflection loses level with its time of arrival. `1/r` on the
/// amplitude — spherical spreading — would be an exponent of 1; a little under
/// that keeps the later arrivals in the picture, which is what makes a room
/// sound like one rather than like a slap.
const ER_FALLOFF: f32 = 0.65;

/// Input diffusion: an allpass chain, lengths in milliseconds. Increasing and
/// mutually irrational, so the chain smears rather than repeats.
const DIFF_IN_MS: [f32; 4] = [4.77, 7.31, 11.53, 17.91];
/// Output diffusion, a side each. Short — this is here to take the last edges
/// off the line taps, not to add its own time.
const DIFF_OUT_L_MS: [f32; 2] = [3.11, 6.83];
const DIFF_OUT_R_MS: [f32; 2] = [3.67, 8.09];

// ─── Ranges ─────────────────────────────────────────────────────────────────

/// The shortest late line, at `Size` 0 and at `Size` 1. Everything else is
/// these times a ratio.
const SIZE_MIN_MS: f32 = 13.0;
const SIZE_MAX_MS: f32 = 92.0;
/// Longest late line to allocate for: the largest size, the longest ratio, the
/// deepest modulation and a sample of interpolation margin.
const LINE_CAP_MS: f32 = SIZE_MAX_MS * 3.229 + MOD_MAX_MS + 4.0;
/// Pre-delay range. 250 ms is about where a pre-delay stops reading as depth
/// and starts reading as a separate event.
const PREDELAY_MAX_MS: f32 = 250.0;
/// The largest early reflection: the table's longest time at the widest spread.
const ER_CAP_MS: f32 = 91.3 * ER_SPREAD_MAX;
const ER_SPREAD_MIN: f32 = 0.30;
const ER_SPREAD_MAX: f32 = 1.45;
/// Deepest modulation swing, one way, in milliseconds. Past a few milliseconds
/// the pitch movement stops being a shimmer and becomes a warble.
const MOD_MAX_MS: f32 = 4.0;

/// Decay time at `Decay` = 0 and at `Decay` = 1, seconds. Exponential between
/// them, so the knob is even in the way the ear hears time: half way is 1.55 s,
/// which is a real hall, and the top is long enough for sound design without
/// being a freeze.
const RT60_MIN: f32 = 0.20;
const RT60_MAX: f32 = 12.0;

/// What the loop gain becomes when the reverb is frozen. Not 1.0: in `f32` an
/// exactly lossless loop has no reason to stay bounded once rounding is in it,
/// and the difference between this and unity is 40 minutes of decay — infinite
/// as far as anybody listening is concerned.
const FREEZE_GAIN: f32 = 0.999_99;

/// Where the loop's soft limiter tops out. The curve itself — exactly the
/// identity below 70 % of this — is [`soft_clip`], shared with every other
/// effect that has a feedback path to protect.
const SOFT_CEIL: f32 = 4.0;

/// The wet output's makeup gain.
///
/// A reverberant space stores energy: the longer the decay the higher the
/// steady-state level, by `1/√(1 − g²)` for a loop gain `g`. That is physically
/// right and musically useless — it means turning the decay up turns the reverb
/// up. So the late output is divided by that estimate and multiplied by this,
/// which puts a fully wet reverb at roughly the level of what went into it.
/// Measured, not guessed: see `the_wet_level_does_not_run_away_with_the_decay`.
const LATE_MAKEUP: f32 = 1.9;

// ─── Pieces ─────────────────────────────────────────────────────────────────

/// A Schroeder allpass: flat magnitude, dispersed phase.
///
/// It is what turns a handful of discrete arrivals into a smear without
/// colouring them — the magnitude response is exactly flat, so however many are
/// chained the spectrum is unchanged and only the timing is smudged. The
/// coefficient is the diffusion control; above about 0.8 the chain starts to
/// ring at its own delay and stops being transparent, which is why the range
/// stops short of it.
struct Allpass {
    line: Line,
    len: f32,
}

impl Allpass {
    fn new(samples: usize) -> Self {
        Self {
            line: Line::with_samples(samples + 2),
            len: samples.max(1) as f32,
        }
    }

    #[inline(always)]
    fn process(&mut self, x: f32, k: f32) -> f32 {
        let delayed = self.line.read(self.len);
        let v = x + k * delayed;
        // Flushed on the way in: an allpass is lossless by construction, so
        // whatever tiny value is circulating in one circulates forever, and
        // "forever" at 1e-40 is a denormal on every multiply.
        self.line.write(safe(v));
        delayed - k * v
    }
}

// ─── The named modes ────────────────────────────────────────────────────────

/// What kind of space the same engine is being asked to be.
///
/// Not five reverbs: one network, with the balance between its stages moved.
/// The difference between a room and a hall really *is* this — how much of what
/// you hear is discrete reflections against how much is the diffuse tail, how
/// far apart the surfaces are, and how long the energy takes to go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Character {
    /// Reflections you can almost count, a short tail, a located sound.
    Room,
    #[default]
    /// The tail builds rather than arrives, and it stays.
    Hall,
    /// Dense at once, medium tail, no sense of delay. Voices and pianos.
    Chamber,
    /// No realistic reflections at all, immediate density, bright.
    Plate,
    /// Long pre-delay, very long tail, wide and moving. For pads.
    Ambient,
}

impl Character {
    pub const ALL: [Character; 5] = [
        Character::Room,
        Character::Hall,
        Character::Chamber,
        Character::Plate,
        Character::Ambient,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Character::Room => "ROOM",
            Character::Hall => "HALL",
            Character::Chamber => "CHAMBER",
            Character::Plate => "PLATE",
            Character::Ambient => "AMBIENT",
        }
    }

    pub fn to_norm(self) -> f32 {
        Self::ALL.iter().position(|c| *c == self).unwrap_or(0) as f32 / (Self::ALL.len() - 1) as f32
    }

    pub fn from_norm(v: f32) -> Self {
        let n = Self::ALL.len();
        let i = (v.clamp(0.0, 1.0) * (n - 1) as f32).round() as usize;
        Self::ALL[i.min(n - 1)]
    }

    /// How this character bends the knobs. Every field is a multiplier or an
    /// offset on what the user set, never a replacement: `Decay` still means
    /// decay in a plate, it just means a different number of seconds.
    fn shape(self) -> Shape {
        match self {
            // Close walls, a short tail, and the reflections left audible —
            // that is what tells you the size of a room.
            Character::Room => Shape {
                er: 1.00,
                late: 0.80,
                er_spread: 0.70,
                size: 0.65,
                decay: 0.55,
                diffusion: -0.15,
                damp: 1.00,
                modulation: 0.45,
                width: 0.90,
            },
            // The opposite trade: the discrete arrivals pushed down and spread
            // out so the tail seems to grow out of nothing.
            Character::Hall => Shape {
                er: 0.45,
                late: 1.00,
                er_spread: 1.25,
                size: 1.25,
                decay: 1.50,
                diffusion: 0.15,
                damp: 0.85,
                modulation: 1.00,
                width: 1.15,
            },
            // A small hard space: dense immediately, no time to read as delay.
            Character::Chamber => Shape {
                er: 0.70,
                late: 1.00,
                er_spread: 0.55,
                size: 0.85,
                decay: 0.90,
                diffusion: 0.08,
                damp: 1.10,
                modulation: 0.65,
                width: 1.00,
            },
            // A plate has no walls, so it has no reflections to speak of: the
            // density is there from the first millisecond, and it is bright
            // because steel is.
            Character::Plate => Shape {
                er: 0.12,
                late: 1.00,
                er_spread: 0.35,
                size: 0.50,
                decay: 1.00,
                diffusion: 0.30,
                damp: 1.45,
                modulation: 0.55,
                width: 1.05,
            },
            // Not a space anybody has stood in. Everything long and moving.
            Character::Ambient => Shape {
                er: 0.22,
                late: 1.00,
                er_spread: 1.45,
                size: 1.50,
                decay: 2.50,
                diffusion: 0.25,
                damp: 0.70,
                modulation: 1.70,
                width: 1.30,
            },
        }
    }
}

struct Shape {
    er: f32,
    late: f32,
    er_spread: f32,
    size: f32,
    decay: f32,
    diffusion: f32,
    damp: f32,
    modulation: f32,
    width: f32,
}

/// How much network to run.
///
/// The buffers are the same either way — this only changes how many lines,
/// diffusers and reflections are read, so it can be switched while audio is
/// running without allocating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Quality {
    /// Four lines, two diffusion stages, four reflections a side.
    Economy,
    #[default]
    /// Eight of each. Denser tail, more decorrelated stereo, about twice the
    /// arithmetic.
    High,
}

impl Quality {
    pub const ALL: [Quality; 2] = [Quality::Economy, Quality::High];

    pub fn label(self) -> &'static str {
        match self {
            Quality::Economy => "ECONOMY",
            Quality::High => "HIGH",
        }
    }

    pub fn lines(self) -> usize {
        match self {
            Quality::Economy => ECON_LINES,
            Quality::High => MAX_LINES,
        }
    }

    fn rotate(self) -> usize {
        match self {
            Quality::Economy => ROTATE_ECON,
            Quality::High => ROTATE_HIGH,
        }
    }

    pub fn to_norm(self) -> f32 {
        match self {
            Quality::Economy => 0.0,
            Quality::High => 1.0,
        }
    }

    pub fn from_norm(v: f32) -> Self {
        match v >= 0.5 {
            true => Quality::High,
            false => Quality::Economy,
        }
    }
}

// ─── The reverb ─────────────────────────────────────────────────────────────

/// Choz's algorithmic reverb: pre-delay, early reflections, diffusion and a
/// modulated feedback delay network with in-loop damping.
pub struct Reverb {
    /// The late network. Always [`MAX_LINES`] of them; [`Quality`] says how
    /// many are read.
    lines: Vec<Line>,
    /// The mono send, delayed. The pre-delay tap and every early reflection
    /// read from this one buffer at different distances, which is why the
    /// reflections cost a read each and not a delay line each.
    pre: Line,
    diff_in: Vec<Allpass>,
    diff_l: Vec<Allpass>,
    diff_r: Vec<Allpass>,

    /// Per-line filter state: the damping low-pass and the low-cut high-pass,
    /// both inside the feedback path.
    lp: [f32; MAX_LINES],
    hp: [f32; MAX_LINES],
    /// Where each line's modulation is, and how fast it moves.
    phase: [f32; MAX_LINES],
    phase_inc: [f32; MAX_LINES],
    /// How deeply each line is modulated, relative to the others. Different per
    /// line on purpose: lines that move together do not decorrelate.
    mod_scale: [f32; MAX_LINES],

    /// Read distance and feedback gain per line, both smoothed — this is where
    /// `Size` and `Decay` actually land.
    dist: [Smoothed; MAX_LINES],
    gain: [Smoothed; MAX_LINES],

    /// Output filter state, a side each.
    out_lp: [f32; 2],
    out_hp: [f32; 2],
    tilt_lp: [f32; 2],

    s_damp_a: Smoothed,
    s_locut_a: Smoothed,
    s_hicut_a: Smoothed,
    s_outlow_a: Smoothed,
    s_tilt: Smoothed,
    s_predelay: Smoothed,
    s_er_spread: Smoothed,
    s_er_gain: Smoothed,
    s_late_gain: Smoothed,
    s_wet: Smoothed,
    s_width: Smoothed,
    s_mod_depth: Smoothed,
    s_diff_k: Smoothed,
    s_in_gain: Smoothed,

    /// Reflection times in samples at spread 1.0, and their gains. Recomputed
    /// when the rate or the quality changes; never in the loop.
    er_t: [[f32; MAX_LINES]; 2],
    er_g: [[f32; MAX_LINES]; 2],
    /// Output weights, normalised for the line count in use.
    out_w: [[f32; MAX_LINES]; 2],

    p_size: f32,
    p_damp: f32,
    p_width: f32,
    p_wet: f32,
    p_decay: f32,
    p_predelay: f32,
    p_diffusion: f32,
    p_tone: f32,
    p_mod: f32,
    p_locut: f32,
    p_hicut: f32,
    character: Character,
    quality: Quality,
    freeze: bool,
    /// The tilt filter's coefficient. A field and not a local, because the
    /// shimmer calls `process_block` one frame at a time and an `exp` per
    /// block would then be an `exp` per sample.
    tilt_a: f32,
    sample_rate: f32,
}

/// One pole, from a corner frequency. `1 − e^(−2πf/fs)`, the impulse-invariant
/// mapping — accurate right up to Nyquist, unlike the bilinear shortcut that
/// pretends the corner is `2πf/fs`.
#[inline]
fn one_pole(fc: f32, sr: f32) -> f32 {
    let f = fc.clamp(10.0, sr * 0.49);
    1.0 - (-std::f32::consts::TAU * f / sr).exp()
}

/// A range walked exponentially: what every frequency and every time control
/// here uses, because both are heard in ratios.
#[inline]
fn exp_map(v: f32, lo: f32, hi: f32) -> f32 {
    lo * (hi / lo).powf(v.clamp(0.0, 1.0))
}

impl Reverb {
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate.max(8000) as f32;
        let ms = |m: f32| (m * 0.001 * sr) as usize + 2;

        let lines = (0..MAX_LINES)
            .map(|_| Line::with_samples(ms(LINE_CAP_MS)))
            .collect();
        let pre = Line::with_samples(ms(PREDELAY_MAX_MS + ER_CAP_MS));
        let diff_in = DIFF_IN_MS.iter().map(|&m| Allpass::new(ms(m))).collect();
        let diff_l = DIFF_OUT_L_MS.iter().map(|&m| Allpass::new(ms(m))).collect();
        let diff_r = DIFF_OUT_R_MS.iter().map(|&m| Allpass::new(ms(m))).collect();

        // 20 ms on everything that is a level or a coefficient, 60 ms on the
        // delay distances: moving a read head is heard as pitch, so `Size` has
        // to arrive slowly enough that the glide is a glide and not a chirp.
        let s = |v: f32, t: f32| Smoothed::new(v, t, sr);
        let mut r = Self {
            lines,
            pre,
            diff_in,
            diff_l,
            diff_r,
            lp: [0.0; MAX_LINES],
            hp: [0.0; MAX_LINES],
            phase: [0.0; MAX_LINES],
            phase_inc: [0.0; MAX_LINES],
            mod_scale: [1.0; MAX_LINES],
            dist: [s(0.0, 60.0); MAX_LINES],
            gain: [s(0.0, 30.0); MAX_LINES],
            out_lp: [0.0; 2],
            out_hp: [0.0; 2],
            tilt_lp: [0.0; 2],
            s_damp_a: s(0.0, 20.0),
            s_locut_a: s(0.0, 20.0),
            s_hicut_a: s(0.0, 20.0),
            s_outlow_a: s(0.0, 20.0),
            s_tilt: s(0.0, 20.0),
            s_predelay: s(1.0, 40.0),
            s_er_spread: s(1.0, 60.0),
            s_er_gain: s(0.0, 20.0),
            s_late_gain: s(0.0, 20.0),
            s_wet: s(0.0, 20.0),
            s_width: s(1.0, 20.0),
            s_mod_depth: s(0.0, 40.0),
            s_diff_k: s(0.5, 20.0),
            s_in_gain: s(1.0, 25.0),
            er_t: [[0.0; MAX_LINES]; 2],
            er_g: [[0.0; MAX_LINES]; 2],
            out_w: [[0.0; MAX_LINES]; 2],
            // The defaults are a hall at a send level: what a reverb should be
            // the moment it is added, without touching a knob.
            p_size: 0.50,
            p_damp: 0.50,
            p_width: 1.00,
            p_wet: 0.35,
            p_decay: 0.45,
            p_predelay: 0.08,
            p_diffusion: 0.70,
            p_tone: 0.50,
            p_mod: 0.25,
            p_locut: 0.15,
            p_hicut: 0.80,
            character: Character::Hall,
            quality: Quality::High,
            freeze: false,
            tilt_a: one_pole(TILT_SPLIT_HZ, sr),
            sample_rate: sr,
        };
        r.rebuild_tables();
        r.update();
        r.snap();
        r
    }

    /// Everything that depends on the rate or the line count and nothing on a
    /// knob: reflection times and gains, output weights, modulation rates.
    fn rebuild_tables(&mut self) {
        let sr = self.sample_rate;
        let n = self.quality.lines();
        self.tilt_a = one_pole(TILT_SPLIT_HZ, sr);
        for (i, rate) in MOD_RATES.iter().enumerate() {
            self.phase_inc[i] = rate / sr;
            // Spread from a bit over half depth to full, so no two lines swing
            // by the same amount either.
            self.mod_scale[i] = 0.55 + 0.45 * (i as f32 / (MAX_LINES - 1) as f32);
        }
        // Reflections: time in samples, gain from that time.
        let mut e2 = [0.0f32; 2];
        for (side, table) in [ER_L_MS, ER_R_MS].iter().enumerate() {
            for (k, t) in table.iter().enumerate() {
                self.er_t[side][k] = t * 0.001 * sr;
                let g = (table[0] / t).powf(ER_FALLOFF) * ER_SIGNS[k];
                self.er_g[side][k] = g;
                if k < n {
                    e2[side] += g * g;
                }
            }
            // Unit energy over the taps actually read, so Economy's four are
            // as loud as High's eight rather than half as loud.
            let norm = e2[side].sqrt().max(1e-6);
            for g in self.er_g[side].iter_mut() {
                *g /= norm;
            }
        }
        // Output weights, likewise normalised over the lines in use.
        for (side, table) in [OUT_L, OUT_R].iter().enumerate() {
            let energy: f32 = table[..n].iter().map(|w| w * w).sum();
            let norm = energy.sqrt().max(1e-6);
            for (i, w) in table.iter().enumerate() {
                self.out_w[side][i] = w / norm;
            }
        }
    }

    /// Turn the knobs into the coefficients the loop actually reads.
    ///
    /// Called whenever a parameter moves — off the audio thread when the chain
    /// is built, on it when a knob or a learned CC arrives. It is arithmetic
    /// only: no allocation, no locks, and everything it writes is a *target*,
    /// so the loop still walks there one sample at a time.
    fn update(&mut self) {
        let sr = self.sample_rate;
        let sh = self.character.shape();
        let n = self.quality.lines();

        // ── Size and decay ─────────────────────────────────────────────────
        let base_ms = (SIZE_MIN_MS + self.p_size * (SIZE_MAX_MS - SIZE_MIN_MS)) * sh.size;
        let base = base_ms * 0.001 * sr;
        let rt60 = (exp_map(self.p_decay, RT60_MIN, RT60_MAX) * sh.decay).clamp(0.05, 90.0);

        let mut open_sum = 0.0;
        for (i, ratio) in LINE_RATIOS.iter().enumerate() {
            let d = base * ratio;
            self.dist[i].set_target(d);
            // The exact per-pass gain for −60 dB in `rt60` seconds. This is the
            // whole of the decay control: no feedback number is guessed at, and
            // `Size` cannot walk the network towards instability because the
            // gain falls as the line lengthens.
            let secs = d / sr;
            let open = 10f32.powf(-3.0 * secs / rt60).min(0.9995);
            self.gain[i].set_target(match self.freeze {
                true => FREEZE_GAIN,
                false => open,
            });
            if self.freeze {
                // A whole number of samples, so the cubic reads a stored sample
                // rather than interpolating one. Interpolation is a fraction of
                // a decibel a pass; a frozen loop makes hundreds of passes a
                // second, and a fraction of a decibel hundreds of times is a
                // fade. The glide to the nearest sample is under half a sample
                // and takes 60 ms, which nothing can hear.
                self.dist[i].set_target(d.round());
            }
            if i < n {
                open_sum += open;
            }
        }
        // From the *unfrozen* gains, so freezing holds the level it had rather
        // than diving as the loop gain goes to one.
        let g_avg = open_sum / n as f32;
        let store = (1.0 - g_avg * g_avg).max(1e-4).sqrt();
        self.s_late_gain.set_target(LATE_MAKEUP * store * sh.late);

        // ── The filters in the loop ────────────────────────────────────────
        // Damping is the corner the tail loses its top over, once per pass;
        // Tone slides that corner as well as tilting the output, because a
        // brightness control that only touched the output could make the tail
        // darker but never *progressively* darker.
        let tone_mul = 2f32.powf((self.p_tone - 0.5) * 2.4);
        let damp_fc = exp_map(self.p_damp, 16_000.0, 800.0) * sh.damp * tone_mul;
        self.s_damp_a.set_target(match self.freeze {
            // A frozen tail must not keep darkening: it would fade to nothing
            // through the filter instead of through the gain.
            true => 1.0,
            false => one_pole(damp_fc, sr),
        });
        let locut_fc = exp_map(self.p_locut, 20.0, 1000.0);
        self.s_locut_a.set_target(match self.freeze {
            // Down to a DC blocker and no further. Removing the filter outright
            // would leave its state subtracting a constant — a frozen loop with
            // a DC offset in it, which is the one thing a freeze must not grow.
            true => one_pole(5.0, sr),
            false => one_pole(locut_fc, sr),
        });

        // ── The filters after it ───────────────────────────────────────────
        self.s_hicut_a
            .set_target(one_pole(exp_map(self.p_hicut, 2000.0, 20_000.0), sr));
        self.s_outlow_a.set_target(one_pole(locut_fc * 0.7, sr));
        // ±8 dB either way about a 900 Hz split: enough to warm or open the
        // tail, not enough to make it a filter sweep.
        self.s_tilt.set_target((self.p_tone - 0.5) * 1.2);

        // ── Everything else ────────────────────────────────────────────────
        self.s_predelay
            .set_target((self.p_predelay * PREDELAY_MAX_MS * 0.001 * sr).max(1.0));
        let spread = ((ER_SPREAD_MIN + self.p_size * (ER_SPREAD_MAX - ER_SPREAD_MIN))
            * sh.er_spread)
            .clamp(0.2, 1.6);
        self.s_er_spread.set_target(spread);
        self.s_er_gain.set_target(sh.er);
        let diffusion = (self.p_diffusion + sh.diffusion).clamp(0.0, 1.0);
        self.s_diff_k.set_target(0.35 + diffusion * 0.42);
        // Frozen, the modulation stops: a held tail is a still image, and
        // moving the read heads through it smears it away. It is also the other
        // half of keeping the read distances whole.
        let depth = match self.freeze {
            true => 0.0,
            false => (self.p_mod * sh.modulation).clamp(0.0, 1.0),
        };
        self.s_mod_depth.set_target(depth * MOD_MAX_MS * 0.001 * sr);
        self.s_width.set_target(self.p_width * sh.width);
        // A send law rather than a fader law: most of the useful travel of a
        // reverb mix is in its bottom third, and a linear knob spends two
        // thirds of itself past the point where the reverb is already louder
        // than the source.
        self.s_wet.set_target(self.p_wet.powf(1.4));
        self.s_in_gain.set_target(match self.freeze {
            true => 0.0,
            false => 1.0,
        });
    }

    /// Put every smoother on its target at once. For construction and for
    /// `reset`, where there is no audio to click.
    fn snap(&mut self) {
        for i in 0..MAX_LINES {
            let (d, g) = (self.dist[i].target(), self.gain[i].target());
            self.dist[i].snap(d);
            self.gain[i].snap(g);
            self.phase[i] = i as f32 * 0.618_034 % 1.0;
        }
        for s in self.scalars() {
            let t = s.target();
            s.snap(t);
        }
    }

    // ── The public knobs ───────────────────────────────────────────────────

    /// Stereo width of the wet signal: 0 mono, 1 as the network made it, 2
    /// wide. Mid/side, so the mono fold-down is the same at every setting.
    pub fn set_width(&mut self, w: f32) {
        self.p_width = w.clamp(0.0, 2.0);
        self.update();
    }

    /// How far apart the surfaces are. Kept under its old name because it is
    /// the same control: the reverb's `Size`.
    pub fn set_room_size(&mut self, size: f32) {
        self.p_size = size.clamp(0.0, 1.0);
        self.update();
    }

    pub fn set_damp(&mut self, d: f32) {
        self.p_damp = d.clamp(0.0, 1.0);
        self.update();
    }

    /// Decay time, as a normalised knob. See [`Reverb::rt60`] for the seconds.
    pub fn set_decay(&mut self, d: f32) {
        self.p_decay = d.clamp(0.0, 1.0);
        self.update();
    }

    /// The decay time the current settings actually give, in seconds.
    pub fn rt60(&self) -> f32 {
        (exp_map(self.p_decay, RT60_MIN, RT60_MAX) * self.character.shape().decay).clamp(0.05, 90.0)
    }

    pub fn set_predelay(&mut self, v: f32) {
        self.p_predelay = v.clamp(0.0, 1.0);
        self.update();
    }

    pub fn set_diffusion(&mut self, v: f32) {
        self.p_diffusion = v.clamp(0.0, 1.0);
        self.update();
    }

    pub fn set_tone(&mut self, v: f32) {
        self.p_tone = v.clamp(0.0, 1.0);
        self.update();
    }

    pub fn set_modulation(&mut self, v: f32) {
        self.p_mod = v.clamp(0.0, 1.0);
        self.update();
    }

    pub fn set_low_cut(&mut self, v: f32) {
        self.p_locut = v.clamp(0.0, 1.0);
        self.update();
    }

    pub fn set_high_cut(&mut self, v: f32) {
        self.p_hicut = v.clamp(0.0, 1.0);
        self.update();
    }

    pub fn set_character(&mut self, c: Character) {
        self.character = c;
        self.update();
    }

    pub fn character(&self) -> Character {
        self.character
    }

    /// Hold the tail where it is: loop gain to one, input muted, damping out of
    /// the way.
    ///
    /// Not a knob yet — this is the hook the interface will reach for. It is
    /// here now because a freeze bolted onto a network that was not built for
    /// one is how a reverb learns to produce infinities: the gain, the in-loop
    /// filters and the limiter all have to agree, and they do here because the
    /// matrix is orthogonal and the loss is the only thing setting the decay.
    pub fn set_freeze(&mut self, on: bool) {
        self.freeze = on;
        self.update();
    }

    pub fn frozen(&self) -> bool {
        self.freeze
    }

    /// Swap network size. Allocation-free: the buffers are always the larger
    /// shape, and only the count read changes.
    pub fn set_quality(&mut self, q: Quality) {
        if q == self.quality {
            return;
        }
        // The lines that go out of use keep whatever was in them. Left alone
        // they would be dumped back into the tail on the next switch, seconds
        // or minutes later, so they are emptied now — a few hundred kilobytes
        // of `memset` on a mode change, which is not a knob turn.
        if q == Quality::Economy {
            for j in ECON_LINES..MAX_LINES {
                self.lines[j].clear();
                // The filters too. A parked line's buffer stops moving but its
                // damping and low-cut states hold whatever was in them, and an
                // unparked line pours that back into the loop as an impulse
                // from minutes ago.
                self.lp[j] = 0.0;
                self.hp[j] = 0.0;
            }
            // And the diffusion stages Economy stops reading. An allpass that
            // is not processed does not decay — it *holds*, exactly, for as
            // long as it is parked. Left alone, the third and fourth stages
            // would hand the network a slice of audio from whenever the mode
            // was last changed, which is the loudest thing in an otherwise
            // silent reverb.
            for ap in self
                .diff_in
                .iter_mut()
                .skip(2)
                .chain(self.diff_l.iter_mut().skip(1))
                .chain(self.diff_r.iter_mut().skip(1))
            {
                ap.line.clear();
            }
        }
        self.quality = q;
        self.rebuild_tables();
        self.update();
    }

    pub fn quality(&self) -> Quality {
        self.quality
    }
}

/// Where the tone control splits low from high, Hz. Fixed: it is a tilt, not a
/// crossover anybody has to place.
const TILT_SPLIT_HZ: f32 = 900.0;

/// The polarity the input arrives on each line with. Deliberately not
/// [`LINE_SIGNS`] — an input pattern lined up with the matrix's own diagonal
/// injects into its symmetry instead of across it, and the first passes come
/// out correlated.
const IN_SIGNS: [f32; MAX_LINES] = [1.0, 1.0, -1.0, 1.0, 1.0, -1.0, -1.0, 1.0];

impl FxProcessor for Reverb {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        // A rate change reconfigures; it does not reallocate. Every time in
        // here is derived from the rate, so the acoustics survive it — the one
        // limit is that the buffers were sized for the rate the chain was built
        // at, so a jump to a much higher rate clamps the longest sizes until
        // the chain is rebuilt (which choz does on a device change anyway).
        if (sample_rate as f32 - self.sample_rate).abs() > 0.5 {
            self.sample_rate = sample_rate.max(8000) as f32;
            let sr = self.sample_rate;
            for i in 0..MAX_LINES {
                self.dist[i].set_sample_rate(sr);
                self.gain[i].set_sample_rate(sr);
            }
            for s in self.scalars() {
                s.set_sample_rate(sr);
            }
            self.rebuild_tables();
            self.update();
            self.snap();
        }

        let n = self.quality.lines();
        let rot = self.quality.rotate();
        let n_diff_in = match self.quality {
            Quality::High => DIFF_IN_MS.len(),
            Quality::Economy => 2,
        };
        let n_diff_out = match self.quality {
            Quality::High => 2,
            Quality::Economy => 1,
        };
        // The Householder reflection `I − (2/N)·11ᵀ`, as the one scalar it
        // needs. Orthogonal, so the mixing moves energy and never makes it.
        let house = 2.0 / n as f32;
        // Unit-norm injection: the input reaches every line, and the total
        // energy put in is exactly the energy that arrived.
        let inject = 1.0 / (n as f32).sqrt();
        let tilt_a = self.tilt_a;

        let frames = buf.len() / 2;
        for f in 0..frames {
            let dry_l = buf[f * 2];
            let dry_r = buf[f * 2 + 1];
            // One mono send. A reverberant field is diffuse by the time it is
            // heard — where the source sat in the stereo picture survives in
            // the dry signal, not in the tail — and a mono send is what keeps
            // the wet output's stereo the *room's*, decorrelated by the network
            // rather than inherited from the input.
            let x = safe((dry_l + dry_r) * 0.5) * self.s_in_gain.tick();

            // ── Pre-delay, and the reflections that read out of it ─────────
            self.pre.write(x);
            let pre_d = self.s_predelay.tick();
            let spread = self.s_er_spread.tick();
            let mut er_l = 0.0f32;
            let mut er_r = 0.0f32;
            for k in 0..n {
                er_l += self.er_g[0][k] * self.pre.read(pre_d + self.er_t[0][k] * spread);
                er_r += self.er_g[1][k] * self.pre.read(pre_d + self.er_t[1][k] * spread);
            }
            let pd = self.pre.read(pre_d);

            // ── Input diffusion ────────────────────────────────────────────
            let k = self.s_diff_k.tick();
            let mut diffused = pd;
            for ap in self.diff_in.iter_mut().take(n_diff_in) {
                diffused = ap.process(diffused, k);
            }

            // ── The late network ───────────────────────────────────────────
            let damp_a = self.s_damp_a.tick();
            let locut_a = self.s_locut_a.tick();
            let depth = self.s_mod_depth.tick();
            // Every line's smoother is ticked, in use or not, so that switching
            // quality does not resume four stale parameters.
            let mut dist = [0.0f32; MAX_LINES];
            let mut gain = [0.0f32; MAX_LINES];
            for j in 0..MAX_LINES {
                self.phase[j] += self.phase_inc[j];
                if self.phase[j] >= 1.0 {
                    self.phase[j] -= 1.0;
                }
                dist[j] = self.dist[j].tick() + wobble(self.phase[j]) * depth * self.mod_scale[j];
                gain[j] = self.gain[j].tick();
            }

            let mut z = [0.0f32; MAX_LINES];
            let mut sum = 0.0f32;
            for j in 0..n {
                let mut v = self.lines[j].read_cubic(dist[j]);
                // Damping: one pole of loss per pass, so the tail keeps
                // darkening as it decays instead of being uniformly dull.
                self.lp[j] = safe(self.lp[j] + damp_a * (v - self.lp[j]));
                v = self.lp[j];
                // And the low cut, in the same place and for the same reason:
                // bass that never leaves is what turns a long reverb to mud.
                self.hp[j] = safe(self.hp[j] + locut_a * (v - self.hp[j]));
                v -= self.hp[j];
                v = safe(v) * gain[j];
                z[j] = v;
                sum += v;
            }

            let mut late_l = 0.0f32;
            let mut late_r = 0.0f32;
            for (j, zj) in z.iter().enumerate().take(n) {
                late_l += self.out_w[0][j] * zj;
                late_r += self.out_w[1][j] * zj;
            }

            let c = sum * house;
            for j in 0..n {
                // Rotate, reflect, flip: three orthogonal factors, so the
                // product is orthogonal and the only loss in the loop is the
                // one the decay asked for.
                let src = (j + rot) % n;
                let v = LINE_SIGNS[j] * (z[src] - c) + IN_SIGNS[j] * inject * diffused;
                // The only place the network is allowed to bend. Transparent
                // below the knee, so this does nothing at all until something
                // arrives that would otherwise wind the loop up.
                self.lines[j].write(soft_clip(v, SOFT_CEIL));
            }

            // ── What comes out ─────────────────────────────────────────────
            let eg = self.s_er_gain.tick();
            let lg = self.s_late_gain.tick();
            let mut wl = safe(er_l * eg + late_l * lg);
            let mut wr = safe(er_r * eg + late_r * lg);
            for ap in self.diff_l.iter_mut().take(n_diff_out) {
                wl = ap.process(wl, k);
            }
            for ap in self.diff_r.iter_mut().take(n_diff_out) {
                wr = ap.process(wr, k);
            }

            // High cut, then low cut, on the wet only — the dry is never
            // filtered, which is the difference between a reverb and a tone
            // control.
            // Every filter state on the way out is flushed as it is written.
            // A one-pole approaches zero and never arrives, so without this the
            // tail ends in denormals that cost more to multiply than the music
            // did — and the reverb never actually goes quiet.
            let hc = self.s_hicut_a.tick();
            self.out_lp[0] = safe(self.out_lp[0] + hc * (wl - self.out_lp[0]));
            self.out_lp[1] = safe(self.out_lp[1] + hc * (wr - self.out_lp[1]));
            wl = self.out_lp[0];
            wr = self.out_lp[1];
            let lc = self.s_outlow_a.tick();
            self.out_hp[0] = safe(self.out_hp[0] + lc * (wl - self.out_hp[0]));
            self.out_hp[1] = safe(self.out_hp[1] + lc * (wr - self.out_hp[1]));
            wl -= self.out_hp[0];
            wr -= self.out_hp[1];

            // The tilt: what is left of Tone after it has already moved the
            // damping corner inside the loop.
            let tilt = self.s_tilt.tick();
            self.tilt_lp[0] = safe(self.tilt_lp[0] + tilt_a * (wl - self.tilt_lp[0]));
            self.tilt_lp[1] = safe(self.tilt_lp[1] + tilt_a * (wr - self.tilt_lp[1]));
            let low_l = self.tilt_lp[0];
            let low_r = self.tilt_lp[1];
            wl = low_l * (1.0 - tilt) + (wl - low_l) * (1.0 + tilt);
            wr = low_r * (1.0 - tilt) + (wr - low_r) * (1.0 + tilt);

            // Width, mid/side: the fold-down is the mid whatever this is set
            // to, so widening cannot cost anything in mono.
            let width = self.s_width.tick();
            let mid = safe((wl + wr) * 0.5);
            let side = safe((wl - wr) * 0.5 * width);
            let wet = self.s_wet.tick();

            // The dry goes out exactly as it came in.
            buf[f * 2] = dry_l + soft_clip(mid + side, SOFT_CEIL) * wet;
            buf[f * 2 + 1] = dry_r + soft_clip(mid - side, SOFT_CEIL) * wet;
        }
    }

    fn reset(&mut self) {
        for line in self.lines.iter_mut() {
            line.clear();
        }
        self.pre.clear();
        for ap in self
            .diff_in
            .iter_mut()
            .chain(self.diff_l.iter_mut())
            .chain(self.diff_r.iter_mut())
        {
            ap.line.clear();
        }
        self.lp = [0.0; MAX_LINES];
        self.hp = [0.0; MAX_LINES];
        self.out_lp = [0.0; 2];
        self.out_hp = [0.0; 2];
        self.tilt_lp = [0.0; 2];
        self.update();
        self.snap();
    }

    fn set_mix(&mut self, wet: f32) {
        self.p_wet = wet.clamp(0.0, 1.0);
        self.update();
    }

    fn name(&self) -> &str {
        "Reverb"
    }

    fn params(&self) -> Vec<crate::fx::FxParam> {
        use crate::fx::FxParam;
        // The first four indices are what they always were — a project written
        // before this engine existed opens with its size, damping, width and
        // wet where it left them.
        vec![
            FxParam::new("Size", self.p_size, 0.0, 1.0, ""),
            FxParam::new("Damping", self.p_damp, 0.0, 1.0, ""),
            FxParam::new("Width", self.p_width.min(1.0), 0.0, 1.0, ""),
            FxParam::new("Wet", self.p_wet, 0.0, 1.0, ""),
            FxParam::new("Decay", self.p_decay, 0.0, 1.0, "s"),
            FxParam::new("PreDelay", self.p_predelay, 0.0, 1.0, "ms"),
            FxParam::new("Diffusion", self.p_diffusion, 0.0, 1.0, ""),
            FxParam::new("Tone", self.p_tone, 0.0, 1.0, ""),
            FxParam::new("Modulation", self.p_mod, 0.0, 1.0, ""),
            FxParam::new("LowCut", self.p_locut, 0.0, 1.0, "Hz"),
            FxParam::new("HighCut", self.p_hicut, 0.0, 1.0, "Hz"),
            FxParam::new("Character", self.character.to_norm(), 0.0, 1.0, ""),
            FxParam::new("Quality", self.quality.to_norm(), 0.0, 1.0, ""),
        ]
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => self.set_room_size(v),
            1 => self.set_damp(v),
            2 => self.set_width(v),
            3 => self.set_mix(v),
            4 => self.set_decay(v),
            5 => self.set_predelay(v),
            6 => self.set_diffusion(v),
            7 => self.set_tone(v),
            8 => self.set_modulation(v),
            9 => self.set_low_cut(v),
            10 => self.set_high_cut(v),
            11 => self.set_character(Character::from_norm(v)),
            12 => self.set_quality(Quality::from_norm(v)),
            _ => {}
        }
    }
}

impl Reverb {
    /// Every smoother that is not per-line, for the two places that have to
    /// walk all of them.
    fn scalars(&mut self) -> [&mut Smoothed; 14] {
        [
            &mut self.s_damp_a,
            &mut self.s_locut_a,
            &mut self.s_hicut_a,
            &mut self.s_outlow_a,
            &mut self.s_tilt,
            &mut self.s_predelay,
            &mut self.s_er_spread,
            &mut self.s_er_gain,
            &mut self.s_late_gain,
            &mut self.s_wet,
            &mut self.s_width,
            &mut self.s_mod_depth,
            &mut self.s_diff_k,
            &mut self.s_in_gain,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic noise source. A reverb test that used a real RNG would
    /// pass or fail differently every run, which is not a test.
    struct Noise(u32);
    impl Noise {
        fn new() -> Self {
            Noise(0x2545_F491)
        }
        fn next(&mut self) -> f32 {
            self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (self.0 >> 8) as f32 / (1 << 23) as f32 * 2.0 - 1.0
        }
    }

    /// Run `frames` of `input` through `r` in blocks of `block`, and hand back
    /// the **wet only** — the dry is subtracted, which is exact because the
    /// reverb is documented to leave it alone.
    fn run(
        r: &mut Reverb,
        sr: u32,
        frames: usize,
        block: usize,
        input: &mut dyn FnMut(usize) -> (f32, f32),
    ) -> Vec<f32> {
        let mut out = Vec::with_capacity(frames * 2);
        let mut scratch = vec![0.0f32; block * 2];
        let mut done = 0;
        while done < frames {
            let n = block.min(frames - done);
            let mut dry = Vec::with_capacity(n * 2);
            for i in 0..n {
                let (l, rr) = input(done + i);
                scratch[i * 2] = l;
                scratch[i * 2 + 1] = rr;
                dry.push(l);
                dry.push(rr);
            }
            r.process_block(&mut scratch[..n * 2], sr);
            for i in 0..n * 2 {
                out.push(scratch[i] - dry[i]);
            }
            done += n;
        }
        out
    }

    fn impulse(r: &mut Reverb, sr: u32, frames: usize) -> Vec<f32> {
        run(r, sr, frames, 256, &mut |i| match i {
            0 => (1.0, 1.0),
            _ => (0.0, 0.0),
        })
    }

    fn rms(x: &[f32]) -> f32 {
        match x.is_empty() {
            true => 0.0,
            false => (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt(),
        }
    }

    fn wet(sr: u32) -> Reverb {
        let mut r = Reverb::new(sr);
        r.set_mix(1.0);
        r
    }

    // ── The impulse response ────────────────────────────────────────────────

    /// An impulse has to come back as a room: discrete arrivals first, a tail
    /// that thickens behind them, then a decay — and the two channels must not
    /// be the same signal.
    ///
    /// The old comb bank failed the first half of that: with nothing but
    /// parallel resonators there are no early reflections at all, only the
    /// combs' first echoes, which is why it never sounded like a place.
    #[test]
    fn an_impulse_arrives_then_thickens_then_decays() {
        let sr = 48_000;
        let mut r = wet(sr);
        r.set_character(Character::Room);
        let out = impulse(&mut r, sr, sr as usize);

        // Something arrives inside the first 100 ms — the reflections. The
        // pre-delay is 8 % of 250 ms = 20 ms, so nothing before that.
        let at = |ms: f32| (ms * 0.001 * sr as f32) as usize * 2;
        assert!(
            rms(&out[..at(15.0)]) < 1e-6,
            "the pre-delay has to be silent"
        );
        let early = rms(&out[at(20.0)..at(100.0)]);
        assert!(early > 1e-4, "no early reflections: {early}");

        // Density, measured as crest factor: a handful of discrete arrivals is
        // mostly silence with spikes in it, so peak/RMS is high; a diffuse tail
        // is filled in, so it is low. Counting zero crossings would not tell
        // the two apart — noise and a sparse tap set cross at similar rates.
        let crest = |s: &[f32]| {
            let peak = s.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            peak / rms(s).max(1e-12)
        };
        let sparse = crest(&out[at(20.0)..at(60.0)]);
        let dense = crest(&out[at(200.0)..at(240.0)]);
        assert!(
            dense < sparse * 0.8,
            "the tail must thicken: crest {sparse:.2} early vs {dense:.2} late"
        );

        // And it decays.
        let head = rms(&out[at(100.0)..at(200.0)]);
        let tail = rms(&out[at(700.0)..at(800.0)]);
        assert!(tail < head * 0.6, "it has to decay: {head} -> {tail}");

        // Two channels, not one signal twice.
        let diff: f32 = (0..out.len() / 2)
            .map(|i| (out[i * 2] - out[i * 2 + 1]).abs())
            .fold(0.0, f32::max);
        assert!(diff > 1e-3, "the sides are identical: {diff}");
    }

    /// The measured decay has to be the decay that was asked for.
    ///
    /// T30: the time to fall from −5 dB to −35 dB below the peak, doubled. It
    /// is the standard measurement precisely because the very top and the very
    /// bottom of a real decay are never straight.
    fn measure_t30(out: &[f32], sr: u32) -> f32 {
        let win = (0.05 * sr as f32) as usize;
        let env: Vec<f32> = out
            .chunks(win * 2)
            .map(|c| 20.0 * rms(c).max(1e-12).log10())
            .collect();
        let peak = env.iter().cloned().fold(f32::MIN, f32::max);
        let after = |db: f32| {
            env.iter()
                .position(|e| *e <= peak + db)
                .map(|i| i as f32 * win as f32 / sr as f32)
        };
        match (after(-5.0), after(-35.0)) {
            (Some(a), Some(b)) if b > a => (b - a) * 2.0,
            _ => f32::INFINITY,
        }
    }

    #[test]
    fn the_measured_decay_follows_the_decay_knob() {
        let sr = 48_000;
        let mut last = 0.0f32;
        for decay in [0.2f32, 0.45, 0.7] {
            let mut r = wet(sr);
            // A bright, unfiltered tail: damping and the cuts are *supposed* to
            // shorten the measured decay, so they are out of the way for the
            // measurement of the decay itself.
            r.set_character(Character::Chamber);
            r.set_damp(0.0);
            r.set_low_cut(0.0);
            r.set_high_cut(1.0);
            r.set_decay(decay);
            let want = r.rt60();
            let out = impulse(&mut r, sr, (want * 3.0 * sr as f32) as usize + sr as usize);
            let got = measure_t30(&out, sr);
            assert!(
                got > want * 0.45 && got < want * 1.8,
                "decay {decay}: asked {want:.2}s, measured {got:.2}s"
            );
            assert!(got > last, "a longer decay has to measure longer");
            last = got;
        }
    }

    // ── Stability ───────────────────────────────────────────────────────────

    /// Seconds of loud noise at every decay, and nothing may come out that is
    /// not a number — or that is growing.
    #[test]
    fn the_network_stays_finite_and_bounded_at_every_decay() {
        let sr = 48_000;
        for decay in [0.0f32, 0.5, 1.0] {
            for quality in Quality::ALL {
                let mut r = wet(sr);
                r.set_decay(decay);
                r.set_quality(quality);
                r.set_character(Character::Ambient);
                let mut n = Noise::new();
                let mut peak_first = 0.0f32;
                let mut peak_last = 0.0f32;
                let secs = 4;
                for s in 0..secs {
                    let out = run(&mut r, sr, sr as usize, 128, &mut |_| {
                        let v = n.next() * 0.8;
                        (v, -v * 0.7)
                    });
                    assert!(
                        out.iter().all(|v| v.is_finite()),
                        "decay {decay} {quality:?}: not finite in second {s}"
                    );
                    let p = out.iter().fold(0.0f32, |m, v| m.max(v.abs()));
                    if s == 0 {
                        peak_first = p;
                    }
                    peak_last = p;
                }
                assert!(
                    peak_last < 8.0 && peak_last < peak_first * 4.0 + 1.0,
                    "decay {decay} {quality:?}: runaway, {peak_first} -> {peak_last}"
                );
            }
        }
    }

    /// The tail has to actually stop. A reverb that decays to a very small but
    /// non-zero number forever is a denormal farm and a CPU spike.
    #[test]
    fn the_tail_reaches_true_silence() {
        let sr = 48_000;
        let mut r = wet(sr);
        r.set_decay(0.3);
        let _ = impulse(&mut r, sr, sr as usize / 10);
        // Ten seconds of nothing after it.
        let mut tail = Vec::new();
        for _ in 0..10 {
            tail = run(&mut r, sr, sr as usize, 512, &mut |_| (0.0, 0.0));
        }
        assert!(
            tail.iter().all(|v| *v == 0.0),
            "the tail never reached zero: {}",
            tail.iter().fold(0.0f32, |m, v| m.max(v.abs()))
        );
    }

    /// A signal far past full scale must be bent, not allowed to wind the loop
    /// up and not hard-clipped either.
    #[test]
    fn a_hot_input_is_bent_rather_than_exploded() {
        let sr = 48_000;
        let mut r = wet(sr);
        r.set_decay(0.9);
        let mut n = Noise::new();
        let mut peak = 0.0f32;
        for _ in 0..3 {
            let out = run(&mut r, sr, sr as usize, 256, &mut |_| {
                let v = n.next() * 12.0;
                (v, v)
            });
            assert!(out.iter().all(|v| v.is_finite()));
            peak = peak.max(out.iter().fold(0.0f32, |m, v| m.max(v.abs())));
        }
        assert!(peak < SOFT_CEIL * 3.0, "the limiter let it go: {peak}");
    }

    // ── Level ───────────────────────────────────────────────────────────────

    /// Turning the decay up must not turn the reverb up.
    ///
    /// Physically a longer room *is* louder — it stores more energy — which is
    /// why the late output is divided by an estimate of that storage. This is
    /// the measurement that sets [`LATE_MAKEUP`], and it is what replaced the
    /// old `input * 0.015`.
    #[test]
    fn the_wet_level_does_not_run_away_with_the_decay() {
        let sr = 48_000;
        let mut levels = Vec::new();
        for decay in [0.2f32, 0.5, 0.8] {
            let mut r = wet(sr);
            r.set_decay(decay);
            let mut n = Noise::new();
            // Let it fill, then measure.
            let _ = run(&mut r, sr, sr as usize * 2, 256, &mut |_| {
                let v = n.next() * 0.25;
                (v, v)
            });
            let out = run(&mut r, sr, sr as usize, 256, &mut |_| {
                let v = n.next() * 0.25;
                (v, v)
            });
            levels.push(rms(&out));
        }
        let lo = levels.iter().cloned().fold(f32::MAX, f32::min);
        let hi = levels.iter().cloned().fold(0.0f32, f32::max);
        assert!(
            hi / lo < 2.0,
            "the decay moved the level by {:.1} dB: {levels:?}",
            20.0 * (hi / lo).log10()
        );
        // And it sits near what went in — 0.25 RMS — rather than a tenth of it
        // or ten times it.
        assert!(
            (0.10..0.60).contains(&hi),
            "wet level is wrong: {levels:?} against 0.25 in"
        );
    }

    /// Nothing may be added to the dry when the mix is dry.
    #[test]
    fn a_dry_mix_leaves_the_signal_alone() {
        let sr = 48_000;
        let mut r = Reverb::new(sr);
        r.set_mix(0.0);
        let mut n = Noise::new();
        let mut buf: Vec<f32> = (0..2048).map(|_| n.next()).collect();
        let before = buf.clone();
        r.process_block(&mut buf, sr);
        assert_eq!(buf, before, "the dry path was touched");
    }

    // ── Stereo ──────────────────────────────────────────────────────────────

    #[test]
    fn width_zero_is_mono_and_width_one_is_not() {
        let sr = 48_000;
        let spread = |w: f32| {
            let mut r = wet(sr);
            r.set_width(w);
            // Past the smoothing: `Width` glides like every other control, so
            // the first 20 ms after setting it are on the way there. That is
            // the feature, not the measurement.
            let out = impulse(&mut r, sr, sr as usize / 2);
            let skip = (0.3 * sr as f32) as usize;
            (skip..out.len() / 2)
                .map(|i| (out[i * 2] - out[i * 2 + 1]).abs())
                .fold(0.0f32, f32::max)
        };
        assert!(spread(0.0) < 1e-6, "width 0 must be mono: {}", spread(0.0));
        assert!(spread(1.0) > 1e-3, "width 1 must not be");
    }

    /// The fold-down test the sign-flipped designs fail.
    ///
    /// `(L+R)/2` must keep most of the energy. Two channels built from opposite
    /// signs of the same lines null here; two built from *different* lines do
    /// not, which is why the output weights are all positive.
    #[test]
    fn the_wet_survives_a_mono_fold_down() {
        let sr = 48_000;
        for character in Character::ALL {
            let mut r = wet(sr);
            r.set_character(character);
            let mut n = Noise::new();
            let _ = run(&mut r, sr, sr as usize, 256, &mut |_| {
                let v = n.next() * 0.3;
                (v, v)
            });
            let out = run(&mut r, sr, sr as usize, 256, &mut |_| {
                let v = n.next() * 0.3;
                (v, v)
            });
            let l: Vec<f32> = out.chunks(2).map(|c| c[0]).collect();
            let rr: Vec<f32> = out.chunks(2).map(|c| c[1]).collect();
            let mono: Vec<f32> = l.iter().zip(&rr).map(|(a, b)| (a + b) * 0.5).collect();
            let sides = (rms(&l) + rms(&rr)) * 0.5;
            let db = 20.0 * (rms(&mono) / sides.max(1e-9)).log10();
            // Two uncorrelated channels lose 3 dB folded down, and the wider
            // characters push a little past that on purpose. Anything beyond
            // this is a design that cancels rather than one that decorrelates.
            assert!(db > -5.0, "{character:?} loses {db:.1} dB folded to mono");
        }
    }

    // ── Independence ────────────────────────────────────────────────────────

    /// The block size is the host's business and must not be audible.
    ///
    /// Exactly, not approximately: every coefficient that moves is smoothed per
    /// **sample**, so there is nothing in the algorithm that knows where a
    /// block ends. A design that updated its coefficients once a block would
    /// fail this, which is why this one does not.
    #[test]
    fn the_block_size_does_not_change_a_single_sample() {
        let sr = 48_000;
        let frames = 20_000;
        let reference = {
            let mut r = wet(sr);
            let mut n = Noise::new();
            run(&mut r, sr, frames, 64, &mut |_| {
                let v = n.next() * 0.4;
                (v, v * 0.5)
            })
        };
        for block in [32usize, 96, 128, 256, 512, 1024] {
            let mut r = wet(sr);
            let mut n = Noise::new();
            let got = run(&mut r, sr, frames, block, &mut |_| {
                let v = n.next() * 0.4;
                (v, v * 0.5)
            });
            assert_eq!(got, reference, "block size {block} changed the output");
        }
    }

    /// Built at any rate, the reverb must be the same *acoustics* — the same
    /// decay in seconds and the same reflection pattern in milliseconds.
    #[test]
    fn the_acoustics_hold_across_sample_rates() {
        for sr in [44_100u32, 48_000, 96_000, 192_000] {
            let mut r = wet(sr);
            r.set_character(Character::Chamber);
            r.set_damp(0.0);
            r.set_low_cut(0.0);
            r.set_high_cut(1.0);
            r.set_decay(0.45);
            let want = r.rt60();
            let out = impulse(&mut r, sr, (want * 3.0 * sr as f32) as usize);
            let got = measure_t30(&out, sr);
            assert!(
                got > want * 0.45 && got < want * 1.8,
                "{sr} Hz: asked {want:.2}s, measured {got:.2}s"
            );
        }
    }

    /// A rate change mid-stream reconfigures rather than exploding — and it
    /// does it without asking for memory.
    #[test]
    fn a_rate_change_mid_stream_is_survivable() {
        let mut r = wet(48_000);
        let mut n = Noise::new();
        let _ = run(&mut r, 48_000, 8_000, 256, &mut |_| {
            let v = n.next() * 0.5;
            (v, v)
        });
        for sr in [96_000u32, 44_100, 192_000, 48_000] {
            let out = run(&mut r, sr, 8_000, 256, &mut |_| {
                let v = n.next() * 0.5;
                (v, v)
            });
            assert!(out.iter().all(|v| v.is_finite()), "{sr} Hz broke it");
            assert!(out.iter().all(|v| v.abs() < 8.0), "{sr} Hz ran away");
        }
    }

    // ── Automation ──────────────────────────────────────────────────────────

    /// Every knob swept while audio is running: no jump, no infinity, no
    /// runaway. The step limit is what "no click" means as a number.
    #[test]
    fn every_parameter_can_be_automated_while_it_sounds() {
        let sr = 48_000;
        let mut r = wet(sr);
        let mut n = Noise::new();
        let mut prev = 0.0f32;
        let mut worst = 0.0f32;
        for step in 0..600 {
            let v = (step as f32 / 600.0 * 3.0) % 1.0;
            // Everything except the two that are modes rather than knobs:
            // there is no half-way between a hall and a plate, and smoothing
            // one into the other would be smoothing a name.
            for p in [0usize, 1, 2, 4, 5, 6, 7, 8, 9, 10] {
                r.set_param(p, v);
            }
            let out = run(&mut r, sr, 256, 256, &mut |_| {
                let s = n.next() * 0.4;
                (s, s)
            });
            assert!(out.iter().all(|s| s.is_finite()), "step {step}: not finite");
            for s in out.chunks(2).map(|c| c[0]) {
                worst = worst.max((s - prev).abs());
                prev = s;
            }
        }
        // The input itself steps by up to 0.8 between samples, and the wet is
        // of that order; anything much past it would be a discontinuity the
        // smoothing failed to catch.
        assert!(worst < 2.0, "a parameter change stepped by {worst}");
    }

    /// The two modes that must *not* be smoothed still work, and both of them
    /// are reverbs.
    #[test]
    fn both_qualities_and_all_characters_produce_a_tail() {
        let sr = 48_000;
        for quality in Quality::ALL {
            for character in Character::ALL {
                let mut r = wet(sr);
                r.set_quality(quality);
                r.set_character(character);
                let out = impulse(&mut r, sr, sr as usize / 2);
                let tail = rms(&out[(sr as usize / 4)..]);
                assert!(
                    tail > 1e-5 && tail.is_finite(),
                    "{quality:?}/{character:?} produced no tail: {tail}"
                );
            }
        }
    }

    /// Switching quality with audio in the network must not dump the old lines
    /// back into the tail later.
    #[test]
    fn switching_quality_does_not_leave_audio_behind() {
        let sr = 48_000;
        let mut r = wet(sr);
        r.set_decay(0.8);
        let _ = impulse(&mut r, sr, sr as usize / 4);
        r.set_quality(Quality::Economy);
        // Let the four that stayed decay away.
        for _ in 0..14 {
            let _ = run(&mut r, sr, sr as usize, 512, &mut |_| (0.0, 0.0));
        }
        let before = run(&mut r, sr, sr as usize, 512, &mut |_| (0.0, 0.0))
            .iter()
            .fold(0.0f32, |m, v| m.max(v.abs()));
        r.set_quality(Quality::High);
        let after = run(&mut r, sr, sr as usize, 512, &mut |_| (0.0, 0.0))
            .iter()
            .fold(0.0f32, |m, v| m.max(v.abs()));
        // What is left of the old tail may still be leaving; what must not
        // happen is a *jump* — audio from before the switch arriving all at
        // once because a parked line or a parked allpass held it.
        assert!(
            after <= before.max(1e-9) * 1.5,
            "unparking dumped audio: {before} -> {after}"
        );
        assert!(after < 1e-3, "and it should be near silent by now: {after}");
    }

    // ── Character ───────────────────────────────────────────────────────────

    /// The named modes have to actually differ, and differ in the direction
    /// their names claim: a room is mostly reflections, a hall is mostly tail.
    #[test]
    fn a_room_is_reflections_and_a_hall_is_tail() {
        let sr = 48_000;
        let balance = |c: Character| {
            let mut r = wet(sr);
            r.set_character(c);
            r.set_predelay(0.0);
            let out = impulse(&mut r, sr, sr as usize);
            let at = |ms: f32| (ms * 0.001 * sr as f32) as usize * 2;
            let early = rms(&out[..at(90.0)]);
            let late = rms(&out[at(300.0)..at(600.0)]);
            early / late.max(1e-9)
        };
        let room = balance(Character::Room);
        let hall = balance(Character::Hall);
        let plate = balance(Character::Plate);
        assert!(
            room > hall * 1.5,
            "a room must lead with its reflections: room {room:.2} vs hall {hall:.2}"
        );
        assert!(
            plate < room,
            "a plate has no reflections to lead with: {plate:.2} vs {room:.2}"
        );
    }

    /// Damping and the cuts have to move the spectrum of the tail, and only of
    /// the tail.
    #[test]
    fn damping_darkens_the_tail_and_the_cuts_trim_it() {
        let sr = 48_000;
        // High-frequency share of the tail, as the energy above a one-pole
        // split at 4 kHz.
        let brightness = |damp: f32| {
            let mut r = wet(sr);
            r.set_decay(0.6);
            r.set_damp(damp);
            let out = impulse(&mut r, sr, sr as usize);
            let a = one_pole(4000.0, sr as f32);
            let (mut lp, mut hi, mut lo) = (0.0f32, 0.0f32, 0.0f32);
            for s in out.chunks(2).map(|c| c[0]).skip(sr as usize / 8) {
                lp += a * (s - lp);
                hi += (s - lp) * (s - lp);
                lo += lp * lp;
            }
            hi / lo.max(1e-12)
        };
        let bright = brightness(0.0);
        let dark = brightness(1.0);
        assert!(
            dark < bright * 0.5,
            "damping did not darken the tail: {bright:.4} -> {dark:.4}"
        );

        // The low cut must remove low end from the wet.
        let bass = |cut: f32| {
            let mut r = wet(sr);
            r.set_low_cut(cut);
            let out = impulse(&mut r, sr, sr as usize / 2);
            let a = one_pole(120.0, sr as f32);
            let mut lp = 0.0f32;
            let mut e = 0.0f32;
            for s in out.chunks(2).map(|c| c[0]) {
                lp += a * (s - lp);
                e += lp * lp;
            }
            e
        };
        assert!(
            bass(1.0) < bass(0.0) * 0.3,
            "the low cut did nothing: {} -> {}",
            bass(0.0),
            bass(1.0)
        );
    }

    /// Modulation has to move the tail without becoming an effect of its own.
    #[test]
    fn modulation_moves_the_tail() {
        let sr = 48_000;
        let tail = |amount: f32| {
            let mut r = wet(sr);
            r.set_decay(0.7);
            r.set_modulation(amount);
            impulse(&mut r, sr, sr as usize)
        };
        let still = tail(0.0);
        let moving = tail(1.0);
        let diff: f32 = still
            .iter()
            .zip(&moving)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max);
        assert!(diff > 1e-3, "the modulation changed nothing: {diff}");
        // But the level is the same: modulation is not a gain.
        let ratio = rms(&moving) / rms(&still).max(1e-9);
        assert!(
            (0.5..2.0).contains(&ratio),
            "modulation changed the level by {ratio}"
        );
    }

    // ── Freeze ──────────────────────────────────────────────────────────────

    /// Frozen, the tail holds instead of decaying — and holds without growing,
    /// which is the half that is hard.
    #[test]
    fn freeze_holds_the_tail_without_running_away() {
        let sr = 48_000;
        let mut r = wet(sr);
        r.set_decay(0.5);
        let mut n = Noise::new();
        let _ = run(&mut r, sr, sr as usize, 256, &mut |_| {
            let v = n.next() * 0.4;
            (v, v)
        });
        r.set_freeze(true);
        assert!(r.frozen());
        // Give the input mute and the gains time to arrive, then watch it.
        let _ = run(&mut r, sr, sr as usize / 2, 256, &mut |_| (0.5, 0.5));
        let first = rms(&run(&mut r, sr, sr as usize, 256, &mut |_| (0.5, 0.5)));
        let mut last = first;
        for _ in 0..8 {
            let out = run(&mut r, sr, sr as usize, 256, &mut |_| (0.5, 0.5));
            assert!(out.iter().all(|v| v.is_finite()), "freeze produced a NaN");
            last = rms(&out);
        }
        assert!(
            last > first * 0.5,
            "frozen and still decaying: {first} -> {last}"
        );
        assert!(last < first * 2.0, "frozen and growing: {first} -> {last}");

        // And it lets go.
        r.set_freeze(false);
        let mut level = 1.0f32;
        for _ in 0..10 {
            level = rms(&run(&mut r, sr, sr as usize, 256, &mut |_| (0.0, 0.0)));
        }
        assert!(
            level < first * 0.05,
            "unfreezing did not release it: {level}"
        );
    }

    // ── Contracts the rest of the tree relies on ────────────────────────────

    /// The names and order the interface's table is written against.
    #[test]
    fn the_parameter_list_is_what_the_rack_expects() {
        let r = Reverb::new(48_000);
        let names: Vec<&str> = r.params().iter().map(|p| p.name).collect();
        assert_eq!(
            names,
            vec![
                "Size",
                "Damping",
                "Width",
                "Wet",
                "Decay",
                "PreDelay",
                "Diffusion",
                "Tone",
                "Modulation",
                "LowCut",
                "HighCut",
                "Character",
                "Quality",
            ]
        );
        // The first four indices are where they have always been, so a project
        // written before this engine opens with its settings intact.
        assert_eq!(names[0..4], ["Size", "Damping", "Width", "Wet"]);
    }

    #[test]
    fn the_named_modes_round_trip() {
        for c in Character::ALL {
            assert_eq!(Character::from_norm(c.to_norm()), c);
        }
        for q in Quality::ALL {
            assert_eq!(Quality::from_norm(q.to_norm()), q);
        }
    }

    /// `reset` has to leave nothing behind: the next thing through must be the
    /// same as the first thing ever was.
    #[test]
    fn reset_returns_it_to_the_state_it_was_built_in() {
        let sr = 48_000;
        let mut a = wet(sr);
        // `set_mix` is smoothed like everything else, so a freshly built reverb
        // is still gliding to it. Reset first, or this measures the glide.
        <Reverb as FxProcessor>::reset(&mut a);
        let first = impulse(&mut a, sr, 8_000);
        let mut n = Noise::new();
        let _ = run(&mut a, sr, sr as usize, 256, &mut |_| {
            let v = n.next();
            (v, v)
        });
        <Reverb as FxProcessor>::reset(&mut a);
        let again = impulse(&mut a, sr, 8_000);
        assert_eq!(first, again, "reset left state behind");
    }

    /// The feedback matrix has to be a rotation. If it is not, the decay is
    /// whatever the matrix does and the RT60 is a lie.
    ///
    /// Checked as the property that matters: the mixing stage alone, with the
    /// gains at unity, neither adds energy nor loses it.
    #[test]
    fn the_feedback_matrix_conserves_energy() {
        for (n, rot) in [(ECON_LINES, ROTATE_ECON), (MAX_LINES, ROTATE_HIGH)] {
            let house = 2.0 / n as f32;
            let mut noise = Noise::new();
            for _ in 0..64 {
                let mut z = [0.0f32; MAX_LINES];
                for v in z.iter_mut().take(n) {
                    *v = noise.next();
                }
                let sum: f32 = z[..n].iter().sum();
                let c = sum * house;
                let mut out = [0.0f32; MAX_LINES];
                for (j, o) in out.iter_mut().enumerate().take(n) {
                    *o = LINE_SIGNS[j] * (z[(j + rot) % n] - c);
                }
                let before: f32 = z[..n].iter().map(|v| v * v).sum();
                let after: f32 = out[..n].iter().map(|v| v * v).sum();
                assert!(
                    (after - before).abs() < before * 1e-4 + 1e-6,
                    "n={n}: {before} in, {after} out"
                );
            }
        }
    }
}
