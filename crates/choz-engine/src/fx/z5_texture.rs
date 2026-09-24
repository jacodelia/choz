//! Z5Texture — live granular "texture engine" inspired by the *workflow* of
//! gesture/granular performance boxes (e.g. the Torso S-4 family): a continuously
//! written circular buffer scanned by a pool of windowed grains around an
//! independent **scrub** read head, with **Freeze** (hold the buffer = drones),
//! **Feedback** (resample its own grain output for layering), and **Stretch**
//! (the scrub head drifts slower/faster/backwards than real time).
//!
//! NOT a clone of any product's DSP — original implementation. It shares the
//! grain/circular-buffer skeleton with [`super::Protocosmos`] (so the two stay
//! consistent and cheap), but exposes a richer, *category-grouped* set of live
//! controls so a MIDI control surface can map ~8 knobs at a time.
//!
//! Params (flat index → grouped by the UI into categories):
//! ```text
//!   GRAIN : 0 Size  1 Density  2 Spray  3 Overlap  4 Pitch  5 RndPitch  6 Reverse  7 Spread
//!   MOTION: 8 Freeze 9 Feedbk  10 Stretch 11 Position 12 Drift 13 Blur   14 BufLen  15 Wet
//!   SHAPE : 16 Contour
//!   LFO   : 17 LfoRate 18 LfoDepth 19 LfoShape 20 LfoDest
//!   (added last, so older projects keep their knobs)
//! ```
//!
//! Source = whatever audio is already on the slot's FX chain, i.e. the Pattern
//! voice (sample / synth / tracker). No global input is read. All buffers are
//! preallocated; the audio callback never allocates.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::Arc;

use super::{FxParam, FxProcessor};

const MAX_BUF_S: f32 = 4.0;
const MAX_GRAINS: usize = 16;
pub const Z5_PARAM_COUNT: usize = 21;
/// Number of downsampled waveform bins published to the UI scope.
pub const Z5_WAVE_BINS: usize = 64;

/// Lock-free snapshot of the live engine state, shared (via `Arc`) between the
/// audio thread (writer) and the UI (reader) so the panel can scope the actual
/// buffer + heads. Single writer, relaxed atomics — never blocks the callback.
pub struct Z5Meter {
    write_norm: AtomicU32, // f32 bits, 0..1
    scrub_norm: AtomicU32,
    grains: AtomicU32,
    frozen: AtomicBool,
    wave: [AtomicU8; Z5_WAVE_BINS],
}

impl Z5Meter {
    fn new() -> Self {
        Self {
            write_norm: AtomicU32::new(0),
            scrub_norm: AtomicU32::new(0),
            grains: AtomicU32::new(0),
            frozen: AtomicBool::new(false),
            wave: std::array::from_fn(|_| AtomicU8::new(0)),
        }
    }
    pub fn write_pos(&self) -> f32 {
        f32::from_bits(self.write_norm.load(Ordering::Relaxed))
    }
    pub fn scrub_pos(&self) -> f32 {
        f32::from_bits(self.scrub_norm.load(Ordering::Relaxed))
    }
    pub fn grain_count(&self) -> u32 {
        self.grains.load(Ordering::Relaxed)
    }
    pub fn is_frozen(&self) -> bool {
        self.frozen.load(Ordering::Relaxed)
    }
    /// Copy the current waveform envelope (0..255 per bin) into `out`.
    pub fn waveform(&self, out: &mut [u8; Z5_WAVE_BINS]) {
        for (o, a) in out.iter_mut().zip(self.wave.iter()) {
            *o = a.load(Ordering::Relaxed);
        }
    }
}

struct Grain {
    pos: f64,   // fractional read index into the buffer
    speed: f64, // signed playback rate (negative = reverse)
    pan: f32,   // -1..1 stereo placement
    age: u32,
    life: u32,
    gain: f32,
    active: bool,
}

/// Named preset param-sets (flat order above). Usable as future preset picks.
pub const Z5_PRESETS: &[(&str, [f32; Z5_PARAM_COUNT]); 10] = &[
    //                  Size Dens Spry Ovlp Pit  Rnd  Rev  Sprd | Frz  Fbk  Str  Pos  Drf  Blr  Buf  Wet | Ctr | LfoR LfoD LfoS LfoT
    (
        "Frozen Pad",
        [
            0.70, 0.50, 0.20, 0.60, 0.50, 0.05, 0.00, 0.40, 1.00, 0.30, 0.50, 0.30, 0.10, 0.30,
            1.00, 0.60, 0.50, 0.35, 0.00, 0.00, 0.00,
        ],
    ),
    (
        "Cloud",
        [
            0.45, 0.65, 0.40, 0.50, 0.50, 0.10, 0.00, 0.60, 0.00, 0.25, 0.50, 0.50, 0.20, 0.20,
            1.00, 0.55, 0.50, 0.35, 0.00, 0.00, 0.00,
        ],
    ),
    (
        "Broken Tape",
        [
            0.30, 0.40, 0.55, 0.40, 0.45, 0.15, 0.20, 0.30, 0.00, 0.45, 0.30, 0.50, 0.35, 0.15,
            0.70, 0.60, 0.50, 0.35, 0.00, 0.00, 0.00,
        ],
    ),
    (
        "Microloop",
        [
            0.12, 0.80, 0.10, 0.70, 0.50, 0.05, 0.00, 0.30, 1.00, 0.55, 0.55, 0.40, 0.05, 0.10,
            0.25, 0.70, 0.50, 0.35, 0.00, 0.00, 0.00,
        ],
    ),
    (
        "Reverse Rain",
        [
            0.25, 0.70, 0.60, 0.50, 0.55, 0.20, 0.80, 0.70, 0.00, 0.30, 0.20, 0.50, 0.30, 0.25,
            0.90, 0.60, 0.50, 0.35, 0.00, 0.00, 0.00,
        ],
    ),
    (
        "Particle Swarm",
        [
            0.18, 0.90, 0.75, 0.30, 0.60, 0.40, 0.30, 0.85, 0.00, 0.35, 0.65, 0.45, 0.50, 0.10,
            0.60, 0.65, 0.50, 0.35, 0.00, 0.00, 0.00,
        ],
    ),
    (
        "Digital Mist",
        [
            0.55, 0.55, 0.35, 0.55, 0.50, 0.10, 0.00, 0.50, 1.00, 0.20, 0.50, 0.50, 0.15, 0.40,
            1.00, 0.50, 0.50, 0.35, 0.00, 0.00, 0.00,
        ],
    ),
    (
        "Infinite Drone",
        [
            0.80, 0.45, 0.15, 0.65, 0.50, 0.05, 0.00, 0.30, 1.00, 0.60, 0.50, 0.50, 0.05, 0.35,
            1.00, 0.70, 0.50, 0.35, 0.00, 0.00, 0.00,
        ],
    ),
    (
        "Granular Delay",
        [
            0.35, 0.50, 0.30, 0.50, 0.50, 0.10, 0.00, 0.40, 0.00, 0.50, 0.50, 0.50, 0.10, 0.15,
            0.50, 0.55, 0.50, 0.35, 0.00, 0.00, 0.00,
        ],
    ),
    (
        "Glitch Clouds",
        [
            0.15, 0.85, 0.85, 0.30, 0.55, 0.50, 0.50, 0.80, 0.00, 0.40, 0.70, 0.50, 0.60, 0.05,
            0.40, 0.60, 0.50, 0.35, 0.00, 0.00, 0.00,
        ],
    ),
];

/// What the LFO can move, in the order its Dest knob steps through them.
pub const LFO_DESTS: [&str; 6] = ["Position", "Pitch", "Size", "Spray", "Density", "Spread"];

/// Live granular texture processor.
pub struct Z5Texture {
    sample_rate: u32,
    // ── GRAIN ──
    size: f32,
    density: f32,
    spray: f32,
    overlap: f32,
    pitch: f32,
    rnd_pitch: f32,
    reverse: f32,
    spread: f32,
    // ── MOTION ──
    freeze: f32,
    feedback: f32,
    stretch: f32,
    position: f32,
    drift: f32,
    blur: f32,
    buflen: f32,
    wet: f32,
    /// Grain envelope: 0 percussive (instant attack, long fall), 0.5 Hann —
    /// what every grain always was — 1 flat-topped. The S-4's *contour*.
    contour: f32,
    // ── LFO ── one oscillator, pointed at one of `LFO_DESTS`. Depth 0 is off.
    lfo: super::lfo::Lfo,
    lfo_rate: f32,
    lfo_depth: f32,
    lfo_wave: super::lfo::Wave,
    lfo_dest: usize,
    /// Where it is this sample, −1..1.
    lfo_now: f32,

    buf_l: Vec<f32>,
    buf_r: Vec<f32>,
    write: usize,
    scrub: f64, // independent read head the grains cluster around
    /// How far behind the write head the scrub sits, in samples. Stretch moves
    /// it, Position sets it, un-freezing puts it back to zero (live).
    lag: f64,
    /// Position was turned since the last block — the scrub jumps there.
    position_moved: bool,
    was_frozen: bool,
    drift_walk: f64, // slow random-walk offset (Drift), in samples
    blur_l: f32,     // one-pole smoothing state (Blur)
    blur_r: f32,
    grains: [Grain; MAX_GRAINS],
    spawn_timer: f64,
    rng: u64,
    /// Smoothed input level — gates the scope so the bar only moves while audio is
    /// actually flowing (pattern STOP → no input → static).
    in_level: f32,
    /// Last published (normalised) head positions, held while idle.
    held_write: f32,
    held_scrub: f32,
    /// Loudness of the cloud against how many grains overlap in it, smoothed.
    cloud_norm: f32,
    meter: Arc<Z5Meter>,
}

impl Z5Texture {
    /// A knob as the LFO leaves it: `base` swung by up to ±half its travel
    /// when the LFO points at `dest`, untouched otherwise.
    #[inline]
    fn modded(&self, base: f32, dest: usize) -> f32 {
        match self.lfo_dest == dest && self.lfo_depth > 0.0 {
            true => (base + self.lfo_depth * self.lfo_now * 0.5).clamp(0.0, 1.0),
            false => base,
        }
    }

    pub fn new(sr: u32) -> Self {
        let sr = sr.max(8000);
        let len = (MAX_BUF_S * sr as f32) as usize + 4;
        const DEAD: Grain = Grain {
            pos: 0.0,
            speed: 1.0,
            pan: 0.0,
            age: 0,
            life: 1,
            gain: 0.0,
            active: false,
        };
        Self {
            meter: Arc::new(Z5Meter::new()),
            sample_rate: sr,
            size: 0.4,
            density: 0.55,
            spray: 0.35,
            overlap: 0.5,
            pitch: 0.5,
            rnd_pitch: 0.0,
            reverse: 0.0,
            spread: 0.5,
            freeze: 0.0,
            feedback: 0.3,
            stretch: 0.5,
            position: 0.5,
            drift: 0.0,
            blur: 0.0,
            buflen: 1.0,
            wet: 0.55,
            contour: 0.5,
            lfo: super::lfo::Lfo::new(),
            lfo_rate: 0.35,
            lfo_depth: 0.0,
            lfo_wave: super::lfo::Wave::Sine,
            lfo_dest: 0,
            lfo_now: 0.0,
            buf_l: vec![0.0; len],
            buf_r: vec![0.0; len],
            write: 0,
            scrub: 0.0,
            lag: 0.0,
            position_moved: false,
            was_frozen: false,
            drift_walk: 0.0,
            blur_l: 0.0,
            blur_r: 0.0,
            grains: [DEAD; MAX_GRAINS],
            spawn_timer: 0.0,
            rng: 0x5EED_4D1C_7E27_0001,
            in_level: 0.0,
            held_write: 0.0,
            held_scrub: 0.0,
            cloud_norm: 1.0,
        }
    }

    /// Build with a flat param slice (as stored on the UI FX entry).
    pub fn with_params(sr: u32, params: &[f32]) -> Self {
        let mut s = Self::new(sr);
        // Only what was given: a list from before a knob existed leaves that
        // knob at its default (Contour's is the Hann every grain used to be).
        for (i, v) in params.iter().take(Z5_PARAM_COUNT).enumerate() {
            s.set_param(i, *v);
        }
        // Building is not turning the knob: a preset starts live, at the head.
        s.position_moved = false;
        s
    }

    /// Shared live-state meter for the UI scope (clone the `Arc`, never blocks).
    pub fn meter(&self) -> Arc<Z5Meter> {
        Arc::clone(&self.meter)
    }

    #[inline]
    fn rand(&mut self) -> f32 {
        self.rng = self
            .rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.rng >> 40) as f32 / (1u32 << 24) as f32
    }

    /// Active scrub window length in samples (BufLen shrinks the loop).
    fn window_len(&self) -> usize {
        let full = self.buf_l.len();
        ((self.buflen.clamp(0.05, 1.0) * full as f32) as usize).clamp(2, full)
    }

    fn spawn(&mut self) {
        let Some(idx) = self.grains.iter().position(|g| !g.active) else {
            return;
        };
        let win = self.window_len();
        let sr = self.sample_rate as f32;
        // Sample distinct points around the scrub head — Spray scatters across the
        // WHOLE loop window (Torso-S-4-style multi-point granular texture). At
        // Spray=0 grains sit on the scrub head; at 1 they pick anywhere in the loop.
        let spray_samps = self.modded(self.spray, 3) * win as f32;
        let offset = (self.rand() as f64) * spray_samps as f64;
        // The LFO on Position swings the grains up to half the loop either way.
        let swing = match self.lfo_dest == 0 {
            true => (self.lfo_depth * self.lfo_now * 0.5) as f64 * win as f64,
            false => 0.0,
        };
        let base = (self.scrub - self.drift_walk - swing).rem_euclid(win as f64);
        let mut pos = (base - offset).rem_euclid(win as f64);
        // Pitch: ±12 st from the knob + ±RndPitch jitter; Reverse with probability.
        let jitter = (self.rand() * 2.0 - 1.0) * self.rnd_pitch * 7.0;
        let st = (self.modded(self.pitch, 1) - 0.5) * 24.0 + jitter;
        let mut speed = 2.0_f64.powf(st as f64 / 12.0);
        if self.rand() < self.reverse {
            speed = -speed;
        }
        // Grain length 20..200 ms scaled by Size.
        let grain_ms = 20.0 + self.modded(self.size, 2) * 180.0;
        let life = ((grain_ms / 1000.0) * sr).max(2.0) as u32;
        // A grain played faster than the tape gains on the write head. Started
        // too close it overtakes it and reads the far end of the loop — a
        // click on every sped-up grain near the head. So it starts far enough
        // back to finish its life still behind.
        if self.freeze <= 0.5 && speed > 1.0 {
            let need = (speed - 1.0) * life as f64 + 2.0;
            let behind = (self.write as f64 - 1.0 - pos).rem_euclid(win as f64);
            if behind < need && need < win as f64 {
                pos = (self.write as f64 - 1.0 - need).rem_euclid(win as f64);
            }
        }
        // Overlap lowers per-grain gain so dense clouds don't clip.
        let gain = 0.9 * (1.0 - 0.5 * self.overlap);
        let pan = (self.rand() * 2.0 - 1.0) * self.modded(self.spread, 5);
        self.grains[idx] = Grain {
            pos,
            speed,
            pan,
            age: 0,
            life,
            gain,
            active: true,
        };
    }
}

/// A grain's envelope at `t` (0..1 of its life), for a Contour of `c`.
///
/// Both halves of the knob meet Hann at 0.5: below it the rise shortens (the
/// grain gets a hit), above it both edges shorten around a flat middle (the
/// grain gets a body). `sin²` rises everywhere, so no shape has a corner.
#[inline]
fn contour_env(t: f32, c: f32) -> f32 {
    use std::f32::consts::FRAC_PI_2;
    let t = t.clamp(0.0, 1.0);
    if c <= 0.5 {
        let rise = 0.03 + c / 0.5 * 0.47;
        match t < rise {
            true => (FRAC_PI_2 * t / rise).sin().powi(2),
            false => (FRAC_PI_2 * (t - rise) / (1.0 - rise)).cos().powi(2),
        }
    } else {
        let edge = 0.5 - (c - 0.5) / 0.5 * 0.4;
        if t < edge {
            (FRAC_PI_2 * t / edge).sin().powi(2)
        } else if t > 1.0 - edge {
            (FRAC_PI_2 * (1.0 - t) / edge).sin().powi(2)
        } else {
            1.0
        }
    }
}

impl FxProcessor for Z5Texture {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        if sample_rate != self.sample_rate {
            // ponytail: no rebuild — that allocated 4 s of buffer on the audio
            // thread. The buffer keeps its length in samples, so at a higher rate
            // the longest loop is shorter in seconds (2 s at 96 kHz for one built
            // at 48). Size it for `MAX_RATE`, as Space Echo does, if that matters.
            self.sample_rate = sample_rate.max(8000);
            self.reset();
        }
        let win = self.window_len();
        let sr = self.sample_rate as f32;
        let frozen = self.freeze > 0.5;
        // The LFO moves Density a block at a time: it sets how often grains
        // start, and a block is a few ms.
        let density = 1.0 + self.modded(self.density, 4) * 79.0; // 1..80 grains/sec
        let lfo_hz = 0.02 * 1000.0f32.powf(self.lfo_rate);
        let inter_spawn = (sr / density) as f64;
        // Stretch: how fast the scrub moves against the write head. 0.5 keeps
        // pace (live), 0.25 stands still on the tape, 0 runs backwards at real
        // time, 1 runs at 3×. Continuous through 0.5 — it used to jump from
        // "live" to "almost stopped" a hair either side of it.
        let rel = (self.stretch as f64 - 0.5) * 4.0;
        // A slow wander whose spread is ~0.1 s at full Drift.
        let drift_step = self.drift as f64 * 90.0 * sr as f64 / 48_000.0;
        let drift_decay = 1.0 - 1.0 / sr as f64;
        if self.was_frozen && !frozen {
            self.lag = 0.0; // back to live, where the new audio is
        }
        self.was_frozen = frozen;
        if std::mem::take(&mut self.position_moved) {
            self.lag = self.position as f64 * win as f64;
        }
        let fb = self.feedback.clamp(0.0, 0.95);
        // Blur one-pole coefficient (0 = no smoothing).
        let blur_a = self.blur.clamp(0.0, 0.98);
        let contour = self.contour;
        let frames = buf.len() / 2;
        // How many grains sound at once on average — density × grain length.
        // They read the buffer at different points, so their powers add: the
        // cloud is `√n` louder than one grain. Without this the level was the
        // Density × Size knobs, and with Feedback up it measured 20× the input
        // (+32 dB). From the *expected* overlap, not the count of the moment,
        // so a grain starting or ending does not step the gain of the others.
        let overlap_n = (density * (20.0 + self.modded(self.size, 2) * 180.0) / 1000.0)
            .clamp(1.0, MAX_GRAINS as f32);
        let norm_target = overlap_n.sqrt().recip();

        // Input activity (pre-process the dry buffer): the scope only animates while
        // audio is actually flowing. Frozen = drone still sounds → counts as active.
        let in_e: f32 = if buf.is_empty() {
            0.0
        } else {
            buf.iter().map(|s| s.abs()).sum::<f32>() / buf.len() as f32
        };
        self.in_level = self.in_level * 0.9 + in_e * 0.1;
        let capturing = self.in_level > 2.0e-4;
        let active = frozen || capturing;

        for i in 0..frames {
            let dry_l = buf[i * 2];
            let dry_r = buf[i * 2 + 1];

            if self.lfo_depth > 0.0 {
                self.lfo_now = self.lfo.tick(self.lfo_wave, lfo_hz, sr, 0.0)[0];
            }

            // Drift: a random walk the grains are placed around.
            self.drift_walk =
                (self.drift_walk + ((self.rand() as f64) * 2.0 - 1.0) * drift_step) * drift_decay;
            self.drift_walk = self.drift_walk.clamp(-(win as f64) * 0.5, win as f64 * 0.5);

            // The scrub, as a distance behind the write head. Frozen, the head
            // keeps moving over a buffer that does not, so standing still on
            // the tape means the distance grows.
            self.lag += if frozen { 1.0 - rel } else { -rel };
            self.lag = self.lag.rem_euclid(win as f64);
            self.scrub = (self.write as f64 - 1.0 - self.lag).rem_euclid(win as f64);

            // Spawn grains on schedule.
            if self.spawn_timer <= 0.0 {
                self.spawn();
                self.spawn_timer += inter_spawn;
            }
            self.spawn_timer -= 1.0;

            // Sum active grains (Hann-windowed, fractional, stereo-panned).
            let mut gl = 0.0f32;
            let mut gr = 0.0f32;
            for g in self.grains.iter_mut() {
                if !g.active {
                    continue;
                }
                let p0 = g.pos as usize % win;
                let p1 = (p0 + 1) % win;
                let frac = g.pos.fract() as f32;
                let sl = self.buf_l[p0] * (1.0 - frac) + self.buf_l[p1] * frac;
                let sr_ = self.buf_r[p0] * (1.0 - frac) + self.buf_r[p1] * frac;
                let env = contour_env(g.age as f32 / g.life as f32, contour);
                let mono = (sl + sr_) * 0.5;
                let w = env * g.gain;
                // Equal-power-ish pan.
                let pl = ((1.0 - g.pan) * 0.5).sqrt();
                let pr = ((1.0 + g.pan) * 0.5).sqrt();
                gl += mono * w * pl;
                gr += mono * w * pr;
                g.pos = (g.pos + g.speed).rem_euclid(win as f64);
                g.age += 1;
                if g.age >= g.life {
                    g.active = false;
                }
            }

            self.cloud_norm += (norm_target - self.cloud_norm) * 0.001;
            gl *= self.cloud_norm;
            gr *= self.cloud_norm;

            // Blur: smooth the grain cloud.
            self.blur_l = self.blur_l * blur_a + gl * (1.0 - blur_a);
            self.blur_r = self.blur_r * blur_a + gr * (1.0 - blur_a);
            let out_l = self.blur_l;
            let out_r = self.blur_r;

            // Write input (+ Feedback of the grain cloud) unless Freeze holds it.
            if !frozen {
                self.buf_l[self.write] = (dry_l + out_l * fb).clamp(-4.0, 4.0);
                self.buf_r[self.write] = (dry_r + out_r * fb).clamp(-4.0, 4.0);
            }
            self.write = (self.write + 1) % win;

            buf[i * 2] = dry_l + self.wet * (out_l - dry_l);
            buf[i * 2 + 1] = dry_r + self.wet * (out_r - dry_r);
        }

        // ── Publish a state snapshot for the UI scope (once per block). The heads
        //    only advance while sounding: write needs live input, scrub needs the
        //    audio to be active (input or frozen drone). Idle → held static. ──────
        let win_f = win as f32;
        if capturing {
            self.held_write = self.write as f32 / win_f;
        }
        if active {
            self.held_scrub = (self.scrub as f32) / win_f;
        }
        self.meter
            .write_norm
            .store(self.held_write.to_bits(), Ordering::Relaxed);
        self.meter
            .scrub_norm
            .store(self.held_scrub.to_bits(), Ordering::Relaxed);
        let live_grains = if active {
            self.grains.iter().filter(|g| g.active).count() as u32
        } else {
            0
        };
        self.meter.grains.store(live_grains, Ordering::Relaxed);
        self.meter.frozen.store(frozen, Ordering::Relaxed);
        // Waveform = the PEAK envelope over each bin's sample range (a single sample
        // per bin would jump 0..peak with phase and look like random noise). Then
        // normalise to the loudest bin so quiet buffers stay visible.
        let mut env = [0.0f32; Z5_WAVE_BINS];
        let mut peak = 1.0e-6f32;
        for (b, e) in env.iter_mut().enumerate() {
            let lo = (b * win) / Z5_WAVE_BINS;
            let hi = (((b + 1) * win) / Z5_WAVE_BINS)
                .max(lo + 1)
                .min(self.buf_l.len());
            let step = ((hi - lo) / 24).max(1); // cap the scan per bin
            let mut p = 0.0f32;
            let mut i = lo;
            while i < hi {
                p = p.max(self.buf_l[i].abs().max(self.buf_r[i].abs()));
                i += step;
            }
            *e = p;
            peak = peak.max(p);
        }
        let norm = 1.0 / peak;
        for (b, e) in env.iter().enumerate() {
            self.meter.wave[b].store((e * norm * 255.0).min(255.0) as u8, Ordering::Relaxed);
        }
    }

    fn reset(&mut self) {
        self.buf_l.iter_mut().for_each(|v| *v = 0.0);
        self.buf_r.iter_mut().for_each(|v| *v = 0.0);
        self.write = 0;
        self.scrub = 0.0;
        self.lag = 0.0;
        self.lfo.reset();
        self.lfo_now = 0.0;
        self.drift_walk = 0.0;
        self.blur_l = 0.0;
        self.blur_r = 0.0;
        self.grains.iter_mut().for_each(|g| g.active = false);
        self.spawn_timer = 0.0;
    }

    fn set_mix(&mut self, wet: f32) {
        self.wet = wet.clamp(0.0, 1.0);
    }
    fn name(&self) -> &str {
        "Z5Texture"
    }

    fn params(&self) -> Vec<FxParam> {
        let p = |n, v| FxParam::new(n, v, 0.0, 1.0, "");
        vec![
            p("Size", self.size),
            p("Density", self.density),
            p("Spray", self.spray),
            p("Overlap", self.overlap),
            p("Pitch", self.pitch),
            p("RndPitch", self.rnd_pitch),
            p("Reverse", self.reverse),
            p("Spread", self.spread),
            p("Freeze", self.freeze),
            p("Feedbk", self.feedback),
            p("Stretch", self.stretch),
            p("Position", self.position),
            p("Drift", self.drift),
            p("Blur", self.blur),
            p("BufLen", self.buflen),
            p("Wet", self.wet),
            p("Contour", self.contour),
            p("LfoRate", self.lfo_rate),
            p("LfoDepth", self.lfo_depth),
            p("LfoShape", self.lfo_wave.to_norm()),
            p(
                "LfoDest",
                self.lfo_dest as f32 / (LFO_DESTS.len() - 1) as f32,
            ),
        ]
    }

    /// Live update — preserves the buffer + grains (Freeze must NOT rebuild).
    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => self.size = v,
            1 => self.density = v,
            2 => self.spray = v,
            3 => self.overlap = v,
            4 => self.pitch = v,
            5 => self.rnd_pitch = v,
            6 => self.reverse = v,
            7 => self.spread = v,
            8 => self.freeze = v,
            9 => self.feedback = v,
            10 => self.stretch = v,
            11 => {
                self.position_moved |= (self.position - v).abs() > 1e-4;
                self.position = v;
            }
            12 => self.drift = v,
            13 => self.blur = v,
            14 => self.buflen = v,
            15 => self.wet = v,
            16 => self.contour = v,
            17 => self.lfo_rate = v,
            18 => self.lfo_depth = v,
            19 => self.lfo_wave = super::lfo::Wave::from_norm(v),
            20 => {
                self.lfo_dest =
                    ((v * (LFO_DESTS.len() - 1) as f32).round() as usize).min(LFO_DESTS.len() - 1)
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s4_is_finite_and_bounded() {
        let mut fx = Z5Texture::with_params(
            48000,
            &[
                0.4, 0.6, 0.5, 0.5, 0.5, 0.3, 0.2, 0.6, 0.0, 0.4, 0.5, 0.5, 0.2, 0.2, 1.0, 0.8,
            ],
        );
        let mut block: Vec<f32> = (0..2048).map(|i| 0.4 * (i as f32 * 0.05).sin()).collect();
        for _ in 0..20 {
            fx.process_block(&mut block, 48000);
            assert!(block.iter().all(|s| s.is_finite()));
            assert!(block.iter().all(|s| s.abs() < 8.0));
        }
    }

    #[test]
    fn freeze_holds_after_silence() {
        let mut fx = Z5Texture::new(48000);
        fx.set_mix(1.0);
        fx.set_param(14, 0.05); // short BufLen → a tight loop the prime fully fills
        let mut prime: Vec<f32> = (0..8192).map(|i| 0.6 * (i as f32 * 0.07).sin()).collect();
        fx.process_block(&mut prime, 48000);
        fx.set_param(8, 1.0); // Freeze ON (index 8)
        let mut silence = vec![0.0f32; 8192];
        fx.process_block(&mut silence, 48000);
        let energy: f32 = silence.iter().map(|s| s.abs()).sum();
        assert!(
            energy > 0.0,
            "frozen buffer should still emit grains on silence"
        );
    }

    /// The cloud stays near the level it was given, whatever Density, Size
    /// and Feedback are doing. At the top of all three it used to reach 20×
    /// the input.
    #[test]
    fn a_dense_cloud_with_feedback_does_not_blow_up() {
        let mut fx = Z5Texture::with_params(
            48000,
            &[
                1.0, 1.0, 0.3, 0.0, 0.5, 0.0, 0.0, 0.0, 0.0, 1.0, 0.5, 0.5, 0.0, 0.0, 0.3, 1.0,
            ],
        );
        let mut peak = 0.0f32;
        for k in 0..200 {
            let mut b: Vec<f32> = (0..1024)
                .map(|i| 0.5 * ((i / 2 + k * 512) as f32 * 0.03).sin())
                .collect();
            fx.process_block(&mut b, 48000);
            peak = b.iter().fold(peak, |m, x| m.max(x.abs()));
        }
        assert!(peak < 1.5, "0.5 in, {peak} out");
    }

    fn render(fx: &mut Z5Texture, blocks: usize) -> Vec<f32> {
        let mut all = Vec::new();
        for k in 0..blocks {
            let mut b: Vec<f32> = (0..1024)
                .map(|i| 0.5 * ((i / 2 + k * 512) as f32 * 0.01).sin())
                .collect();
            fx.process_block(&mut b, 48000);
            all.extend(b);
        }
        all
    }

    /// Turning Position moves where the grains read. It used to be multiplied
    /// by zero on its way in, so the knob changed nothing at all.
    #[test]
    fn position_moves_the_scrub() {
        let mut a = Z5Texture::new(48000);
        let mut b = Z5Texture::new(48000);
        render(&mut a, 40);
        render(&mut b, 40);
        a.set_param(11, 0.1);
        b.set_param(11, 0.6);
        render(&mut a, 2);
        render(&mut b, 2);
        assert!((a.lag - b.lag).abs() > 1000.0, "{} vs {}", a.lag, b.lag);
    }

    /// A hair either side of 0.5 is a hair away from live — not a jump from
    /// following the head to standing still.
    #[test]
    fn stretch_is_continuous_through_the_middle() {
        let lag_after = |stretch: f32| {
            let mut fx = Z5Texture::new(48000);
            fx.set_param(10, stretch);
            render(&mut fx, 20);
            fx.lag.min(fx.window_len() as f64 - fx.lag)
        };
        let (at, near) = (lag_after(0.5), lag_after(0.501));
        assert!(at < 1.0 && near < 100.0, "live {at}, a hair off {near}");
    }

    /// A new device rate does not ask the audio thread for memory.
    #[test]
    fn a_rate_change_does_not_allocate() {
        let mut fx = Z5Texture::new(48000);
        let before = (fx.buf_l.as_ptr() as usize, fx.buf_r.as_ptr() as usize);
        fx.set_mix(0.3);
        let mut b = vec![0.1f32; 512];
        fx.process_block(&mut b, 96000);
        assert_eq!(
            before,
            (fx.buf_l.as_ptr() as usize, fx.buf_r.as_ptr() as usize)
        );
        assert_eq!(fx.wet, 0.3);
        assert!(b.iter().all(|x| x.is_finite()));
    }

    /// Contour 0.5 is exactly the Hann window every grain used to have, and
    /// the ends of the knob are a hit and a plateau.
    #[test]
    fn contour_meets_hann_in_the_middle() {
        for i in 0..=100 {
            let t = i as f32 / 100.0;
            let hann = (std::f32::consts::PI * t).sin().powi(2);
            assert!((contour_env(t, 0.5) - hann).abs() < 1e-5, "t {t}");
        }
        assert!(
            contour_env(0.05, 0.0) > 0.9,
            "percussive peaks almost at once"
        );
        assert!(contour_env(0.3, 1.0) == 1.0, "flat-topped holds its middle");
    }

    /// The LFO moves what it points at, and at depth 0 it is not there.
    #[test]
    fn the_lfo_moves_its_destination() {
        let mut fx = Z5Texture::new(48000);
        fx.set_param(17, 0.8); // ~5 Hz
        render(&mut fx, 10);
        assert_eq!(fx.lfo_now, 0.0, "depth 0 leaves it off");
        assert_eq!(fx.modded(0.4, 1), 0.4);

        fx.set_param(18, 1.0);
        fx.set_param(20, 1.0 / 5.0); // Pitch
        let mut seen = (f32::MAX, f32::MIN);
        for _ in 0..20 {
            render(&mut fx, 1);
            let p = fx.modded(0.5, 1);
            seen = (seen.0.min(p), seen.1.max(p));
        }
        assert!(seen.1 - seen.0 > 0.4, "pitch swung {seen:?}");
        assert_eq!(fx.modded(0.4, 2), 0.4, "and only the pitch");
    }

    #[test]
    fn presets_have_full_params() {
        assert!(Z5_PRESETS.iter().all(|(_, p)| p.len() == Z5_PARAM_COUNT));
    }

    #[test]
    fn meter_publishes_state() {
        let mut fx = Z5Texture::new(48000);
        fx.set_mix(1.0);
        let meter = fx.meter();
        let mut block: Vec<f32> = (0..4096).map(|i| 0.6 * (i as f32 * 0.05).sin()).collect();
        fx.process_block(&mut block, 48000);
        assert!((0.0..=1.0).contains(&meter.write_pos()));
        assert!((0.0..=1.0).contains(&meter.scrub_pos()));
        let mut wave = [0u8; Z5_WAVE_BINS];
        meter.waveform(&mut wave);
        assert!(
            wave.iter().any(|&b| b > 0),
            "buffer waveform should be non-silent"
        );
    }
}
