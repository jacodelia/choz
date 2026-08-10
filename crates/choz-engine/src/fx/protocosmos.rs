//! Protocosmos — granular cloud / glitch / particle-delay processor, inspired by
//! the *kinds* of processing in the Hologram Protocosmos (NOT its algorithm). One
//! circular buffer feeds a pool of windowed grains whose position, pitch and
//! direction are randomised; the grain sum is diffused and reverberated.
//!
//! ```text
//!   in ──┬──────────────────────────────────────────────► dry
//!        ▼ (skipped when frozen)
//!     [ circular buffer ] ◄── feedback (texture sustain)
//!        ▲                                   │
//!   grain pool: pos+spray, pitch, reverse, Hann window, crossfaded
//!        │ sum                               │
//!        ▼                                   │
//!     diffusion (allpass) → integrated reverb (comb) ──► wet
//! ```
//!
//! DSP decisions (see docs/audio/protocosmos.md):
//!   • Grains read the shared buffer at a random offset within `spray` of the
//!     write head; overlapping Hann windows crossfade them → smooth clouds.
//!   • `pitch` resamples each grain (2^(st/12)); `reverse` is the probability a
//!     grain plays backwards → mosaic/glitch motion.
//!   • `freeze` stops buffer writes → the current audio is held forever (infinite
//!     texture) while grains keep scanning it.
//!   • Light feedback recirculates the grain output so clouds sustain and bloom.
//!   • `diffuse` blends in an allpass-dispersed comb reverb for ambient tails.
//!   • All buffers preallocated; the audio callback never allocates.

use super::FxProcessor;

const MAX_BUF_S: f32 = 4.0;
const MAX_GRAINS: usize = 12;

struct Grain {
    pos: f64,   // fractional read index into the buffer
    speed: f64, // signed playback rate (negative = reverse)
    age: u32,
    life: u32,
    gain: f32,
    active: bool,
}

/// Schroeder allpass (diffusion).
struct Allpass {
    buf: Vec<f32>,
    pos: usize,
    g: f32,
}
impl Allpass {
    fn new(len: usize, g: f32) -> Self {
        Self {
            buf: vec![0.0; len.max(1)],
            pos: 0,
            g,
        }
    }
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let b = self.buf[self.pos];
        let y = -x + b;
        self.buf[self.pos] = x + b * self.g;
        self.pos = (self.pos + 1) % self.buf.len();
        y
    }
}

/// Damped feedback comb (reverb tail).
struct Comb {
    buf: Vec<f32>,
    pos: usize,
    fb: f32,
    z: f32,
}
impl Comb {
    fn new(len: usize, fb: f32) -> Self {
        Self {
            buf: vec![0.0; len.max(1)],
            pos: 0,
            fb,
            z: 0.0,
        }
    }
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.buf[self.pos];
        self.z = y * 0.6 + self.z * 0.4; // fixed HF damping
        self.buf[self.pos] = x + self.z * self.fb;
        self.pos = (self.pos + 1) % self.buf.len();
        y
    }
}

/// Granular texture processor.
pub struct Protocosmos {
    sample_rate: u32,
    // Normalised 0..1 controls.
    size: f32,    // grain length
    density: f32, // grains/sec
    pitch: f32,   // 0.5 = unison; ±12 st
    spray: f32,   // position scatter
    reverse: f32, // probability of a reversed grain
    freeze: f32,  // >0.5 = hold buffer
    diffuse: f32, // reverb/diffusion amount
    wet: f32,

    buf_l: Vec<f32>,
    buf_r: Vec<f32>,
    write: usize,
    grains: [Grain; MAX_GRAINS],
    spawn_timer: f64,
    rng: u64,

    aps: Vec<Allpass>,
    combs: Vec<Comb>,
}

impl Protocosmos {
    // One argument per knob: the FX chain builds these straight from the
    // parameter vector, so a struct would only add ceremony.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        sr: u32,
        size: f32,
        density: f32,
        pitch: f32,
        spray: f32,
        reverse: f32,
        freeze: f32,
        diffuse: f32,
    ) -> Self {
        let sr = sr.max(8000);
        let len = (MAX_BUF_S * sr as f32) as usize + 4;
        const DEAD: Grain = Grain {
            pos: 0.0,
            speed: 1.0,
            age: 0,
            life: 1,
            gain: 0.0,
            active: false,
        };
        let s = |n: usize| ((n as f32) * sr as f32 / 44100.0) as usize;
        Self {
            sample_rate: sr,
            size,
            density,
            pitch,
            spray,
            reverse,
            freeze,
            diffuse,
            wet: 0.6,
            buf_l: vec![0.0; len],
            buf_r: vec![0.0; len],
            write: 0,
            grains: [DEAD; MAX_GRAINS],
            spawn_timer: 0.0,
            rng: 0x1234_5678_9ABC_DEF0,
            aps: vec![
                Allpass::new(s(441), 0.7),
                Allpass::new(s(341), 0.7),
                Allpass::new(s(225), 0.7),
            ],
            combs: vec![Comb::new(s(1617), 0.78), Comb::new(s(1277), 0.78)],
        }
    }

    #[inline]
    fn rand(&mut self) -> f32 {
        self.rng = self
            .rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.rng >> 40) as f32 / (1u32 << 24) as f32
    }

    fn spawn(&mut self) {
        let Some(idx) = self.grains.iter().position(|g| !g.active) else {
            return;
        };
        let len = self.buf_l.len();
        let sr = self.sample_rate as f32;
        // Position: scatter back from the write head by up to `spray` * 0.5 s.
        let spray_samps = self.spray * 0.5 * sr;
        let offset = (self.rand() as f64) * spray_samps as f64 + 1.0;
        let pos = (self.write as f64 - offset).rem_euclid(len as f64);
        // Pitch: ±12 semitones; reverse with probability `reverse`.
        let st = (self.pitch - 0.5) * 24.0;
        let mut speed = 2.0_f64.powf(st as f64 / 12.0);
        if self.rand() < self.reverse {
            speed = -speed;
        }
        // Grain length 20..200 ms scaled by `size`.
        let grain_ms = 20.0 + self.size * 180.0;
        let life = ((grain_ms / 1000.0) * sr).max(2.0) as u32;
        self.grains[idx] = Grain {
            pos,
            speed,
            age: 0,
            life,
            gain: 0.9,
            active: true,
        };
    }
}

impl FxProcessor for Protocosmos {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        if sample_rate != self.sample_rate {
            *self = Protocosmos::new(
                sample_rate,
                self.size,
                self.density,
                self.pitch,
                self.spray,
                self.reverse,
                self.freeze,
                self.diffuse,
            );
        }
        let len = self.buf_l.len();
        let sr = self.sample_rate as f32;
        let frozen = self.freeze > 0.5;
        let density = 1.0 + self.density * 79.0; // 1..80 grains/sec
        let inter_spawn = (sr / density) as f64;
        let fb = 0.35; // texture-sustain feedback (fixed, bounded)
        let frames = buf.len() / 2;

        for i in 0..frames {
            let dry_l = buf[i * 2];
            let dry_r = buf[i * 2 + 1];

            // Spawn grains on schedule.
            if self.spawn_timer <= 0.0 {
                self.spawn();
                self.spawn_timer += inter_spawn;
            }
            self.spawn_timer -= 1.0;

            // Sum active grains (Hann-windowed, fractional read).
            let mut gl = 0.0f32;
            let mut gr = 0.0f32;
            for g in self.grains.iter_mut() {
                if !g.active {
                    continue;
                }
                let p0 = g.pos as usize % len;
                let p1 = (p0 + 1) % len;
                let frac = g.pos.fract() as f32;
                let sl = self.buf_l[p0] * (1.0 - frac) + self.buf_l[p1] * frac;
                let sr_ = self.buf_r[p0] * (1.0 - frac) + self.buf_r[p1] * frac;
                let env = (std::f32::consts::PI * g.age as f32 / g.life as f32)
                    .sin()
                    .powi(2);
                let w = env * g.gain;
                gl += sl * w;
                gr += sr_ * w;
                g.pos = (g.pos + g.speed).rem_euclid(len as f64);
                g.age += 1;
                if g.age >= g.life {
                    g.active = false;
                }
            }

            // Write input (+ grain feedback) into the buffer, unless frozen.
            if !frozen {
                self.buf_l[self.write] = dry_l + gl * fb;
                self.buf_r[self.write] = dry_r + gr * fb;
            }
            self.write = (self.write + 1) % len;

            // Diffusion + integrated reverb on the grain cloud.
            let mut d = (gl + gr) * 0.5;
            for ap in self.aps.iter_mut() {
                d = ap.process(d);
            }
            let mut tail = 0.0;
            for cb in self.combs.iter_mut() {
                tail += cb.process(d);
            }
            tail *= 0.5 * self.diffuse;

            let wet_l = gl + tail;
            let wet_r = gr + tail;
            buf[i * 2] = dry_l + self.wet * (wet_l - dry_l);
            buf[i * 2 + 1] = dry_r + self.wet * (wet_r - dry_r);
        }
    }

    fn reset(&mut self) {
        self.buf_l.iter_mut().for_each(|v| *v = 0.0);
        self.buf_r.iter_mut().for_each(|v| *v = 0.0);
        self.write = 0;
        self.grains.iter_mut().for_each(|g| g.active = false);
        self.spawn_timer = 0.0;
    }

    fn set_mix(&mut self, wet: f32) {
        self.wet = wet.clamp(0.0, 1.0);
    }
    fn name(&self) -> &str {
        "Protocosmos"
    }

    fn params(&self) -> Vec<super::FxParam> {
        use super::FxParam as P;
        vec![
            P::new("Size", self.size, 0.0, 1.0, ""),
            P::new("Density", self.density, 0.0, 1.0, ""),
            P::new("Pitch", self.pitch, 0.0, 1.0, ""),
            P::new("Spray", self.spray, 0.0, 1.0, ""),
            P::new("Reverse", self.reverse, 0.0, 1.0, ""),
            P::new("Freeze", self.freeze, 0.0, 1.0, ""),
            P::new("Diffuse", self.diffuse, 0.0, 1.0, ""),
            P::new("Wet", self.wet, 0.0, 1.0, ""),
        ]
    }

    /// Live param update — preserves the circular buffer + grains. Critical for
    /// `Freeze`: toggling it must NOT rebuild (which would wipe the held audio).
    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => self.size = v,
            1 => self.density = v,
            2 => self.pitch = v,
            3 => self.spray = v,
            4 => self.reverse = v,
            5 => self.freeze = v,
            6 => self.diffuse = v,
            7 => self.wet = v,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocosmos_is_finite() {
        let mut fx = Protocosmos::new(48000, 0.5, 0.6, 0.6, 0.4, 0.3, 0.0, 0.5);
        let mut block: Vec<f32> = (0..2048).map(|i| 0.4 * (i as f32 * 0.05).sin()).collect();
        for _ in 0..20 {
            fx.process_block(&mut block, 48000);
            assert!(block.iter().all(|s| s.is_finite()));
            assert!(block.iter().all(|s| s.abs() < 8.0));
        }
    }

    #[test]
    fn protocosmos_freeze_holds_after_silence() {
        // Realistic flow: play audio (freeze OFF) to fill the rolling buffer, then
        // engage Freeze via the live set_param path (no rebuild) and go silent —
        // grains must keep scanning the held audio.
        let mut fx = Protocosmos::new(48000, 0.5, 0.8, 0.5, 0.3, 0.0, 0.0, 0.2);
        fx.set_mix(1.0);
        let mut prime: Vec<f32> = (0..8192).map(|i| 0.6 * (i as f32 * 0.07).sin()).collect();
        fx.process_block(&mut prime, 48000);
        fx.set_param(5, 1.0); // Freeze ON — buffer preserved
        let mut silence = vec![0.0f32; 8192];
        fx.process_block(&mut silence, 48000);
        let energy: f32 = silence.iter().map(|s| s.abs()).sum();
        assert!(
            energy > 0.0,
            "frozen buffer should still emit grains on silence"
        );
    }
}
