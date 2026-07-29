//! Granular delay — stereo delay whose feedback path randomises grain positions
//! for infinite texture.  Uses lightweight per-sample grain read-pointers with
//! random pitch scatter, no Grain struct needed.

use super::FxProcessor;

const MAX_BUF: usize = 192_002; // ~4 s @ 48 kHz
const MAX_GRAINS: usize = 8;

struct GrainReader {
    pos:    f64,
    speed:  f64,
    active: bool,
    age:    u32,
    life:   u32,
}

impl GrainReader {
}

/// Granular feedback delay — grain-clouds from the delay buffer fed back.
pub struct GranularDelay {
    buf_l:       Vec<f32>,
    buf_r:       Vec<f32>,
    write:       usize,
    delay_frames: usize,
    feedback:    f32,
    /// Pitch scatter in semitones (0 = no scatter).
    scatter_st:  f32,
    /// Grain density (spawns per second).
    density:     f32,
    wet:         f32,
    grains:      [GrainReader; MAX_GRAINS],
    rng:         u64,
    spawn_timer: f64,
    sample_rate: u32,
    delay_ms:    f32,
}

impl GranularDelay {
    pub fn new(delay_ms: f32, feedback: f32, scatter_st: f32, density: f32) -> Self {
        const INIT: GrainReader = GrainReader { pos: 0.0, speed: 1.0, active: false, age: 0, life: 1 };
        Self {
            buf_l:        vec![0.0; MAX_BUF],
            buf_r:        vec![0.0; MAX_BUF],
            write:        0,
            delay_frames: ((delay_ms / 1000.0) * 48000.0) as usize,
            feedback,
            scatter_st,
            density,
            wet:          0.7,
            grains:       [INIT; MAX_GRAINS],
            rng:          0xBEEF_F00D_1234_ABCD,
            spawn_timer:  0.0,
            sample_rate:  48000,
            delay_ms,
        }
    }

    fn rand_f32(&mut self) -> f32 {
        self.rng = self.rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.rng >> 33) as f32 / u32::MAX as f32
    }

    fn rand_signed(&mut self) -> f32 { self.rand_f32() * 2.0 - 1.0 }

    fn spawn_grain(&mut self, buf_len: usize) {
        let slot = self.grains.iter().position(|g| !g.active);
        let Some(idx) = slot else { return };
        let write_f = self.write as f64;
        let offset = (self.rand_f32() as f64) * self.delay_frames as f64;
        let pos = (write_f - offset).rem_euclid(buf_len as f64);
        let semitones = self.scatter_st * self.rand_signed();
        let speed = 2.0_f64.powf(semitones as f64 / 12.0);
        let grain_ms = 80.0 + self.rand_f32() * 120.0;
        let life = ((grain_ms / 1000.0) * self.sample_rate as f32) as u32;
        self.grains[idx] = GrainReader { pos, speed, active: true, age: 0, life };
    }
}

impl FxProcessor for GranularDelay {
    fn process_block(&mut self, block: &mut [f32], sample_rate: u32) {
        if sample_rate != self.sample_rate {
            self.sample_rate = sample_rate;
            self.delay_frames = ((self.delay_ms / 1000.0) * sample_rate as f32) as usize;
        }
        let frames = block.len() / 2;
        let buf_len = MAX_BUF;
        let inter_spawn = sample_rate as f64 / self.density as f64;

        for i in 0..frames {
            let dry_l = block[i * 2];
            let dry_r = block[i * 2 + 1];

            // Write dry + feedback into delay buffer.
            let read_pos = (self.write + buf_len - self.delay_frames.max(1)) % buf_len;
            let fb_l = self.buf_l[read_pos] * self.feedback;
            let fb_r = self.buf_r[read_pos] * self.feedback;
            self.buf_l[self.write] = dry_l + fb_l;
            self.buf_r[self.write] = dry_r + fb_r;
            self.write = (self.write + 1) % buf_len;

            // Spawn new grains.
            if self.spawn_timer <= 0.0 {
                self.spawn_grain(buf_len);
                self.spawn_timer = inter_spawn;
            }
            self.spawn_timer -= 1.0;

            // Accumulate grain outputs.
            let mut gl = 0.0f32;
            let mut gr = 0.0f32;
            for grain in self.grains.iter_mut() {
                if !grain.active { continue; }
                let p0 = grain.pos as usize % buf_len;
                let p1 = (p0 + 1) % buf_len;
                let frac = grain.pos.fract() as f32;
                let sl = self.buf_l[p0] * (1.0 - frac) + self.buf_l[p1] * frac;
                let sr = self.buf_r[p0] * (1.0 - frac) + self.buf_r[p1] * frac;
                // Hann envelope.
                let env_phase = grain.age as f32 / grain.life as f32;
                let env = (std::f32::consts::PI * env_phase).sin().powi(2);
                gl += sl * env;
                gr += sr * env;
                grain.pos = (grain.pos + grain.speed).rem_euclid(buf_len as f64);
                grain.age += 1;
                if grain.age >= grain.life { grain.active = false; }
            }

            block[i * 2]     = dry_l * (1.0 - self.wet) + gl * self.wet;
            block[i * 2 + 1] = dry_r * (1.0 - self.wet) + gr * self.wet;
        }
    }

    fn reset(&mut self) {
        self.buf_l.iter_mut().for_each(|v| *v = 0.0);
        self.buf_r.iter_mut().for_each(|v| *v = 0.0);
        self.write = 0;
        self.grains.iter_mut().for_each(|g| g.active = false);
        self.spawn_timer = 0.0;
    }

    fn set_mix(&mut self, wet: f32) { self.wet = wet.clamp(0.0, 1.0); }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gran_delay_processes_without_panic() {
        let mut d = GranularDelay::new(200.0, 0.4, 3.0, 8.0);
        let mut block = vec![0.5f32; 256];
        d.process_block(&mut block, 48000);
        // After processing, output should be finite.
        assert!(block.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn gran_delay_reset_clears_buffers() {
        let mut d = GranularDelay::new(100.0, 0.5, 0.0, 4.0);
        let mut block = vec![1.0f32; 64];
        d.process_block(&mut block, 48000);
        d.reset();
        assert!(d.buf_l.iter().all(|&v| v == 0.0));
    }
}
