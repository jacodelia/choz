//! Granular delay — stereo delay whose feedback path randomises grain positions
//! for infinite texture.  Uses lightweight per-sample grain read-pointers with
//! random pitch scatter, no Grain struct needed.

use super::delay_line::DelayLine as Line;
use super::FxProcessor;

/// The longest delay this effect holds, at **every** rate. The knob goes to
/// 1 s; the shared line is sized in time, so a 192 kHz interface gets the same
/// second of audio a 44.1 one does.
const MAX_TIME_MS: f32 = 1000.0;
const MAX_GRAINS: usize = 8;

/// A grain is a read head, and a read head is a **distance behind the write
/// head** — not an absolute index. The write head advances one frame per
/// sample and the grain advances `speed`, so the distance closes at
/// `1 - speed` per sample, which is the whole of `advance()`.
///
/// Keeping it as a distance is what lets the grain read from the shared line,
/// which only knows how far back to look.
struct GrainReader {
    dist: f32,
    speed: f32,
    active: bool,
    age: u32,
    life: u32,
}

/// Granular feedback delay — grain-clouds from the delay buffer fed back.
pub struct GranularDelay {
    line: [Line; 2],
    delay_frames: usize,
    feedback: f32,
    /// Pitch scatter in semitones (0 = no scatter).
    scatter_st: f32,
    /// Grain density (spawns per second).
    density: f32,
    wet: f32,
    grains: [GrainReader; MAX_GRAINS],
    rng: u64,
    spawn_timer: f64,
    sample_rate: u32,
    delay_ms: f32,
}

impl GranularDelay {
    pub fn new(delay_ms: f32, feedback: f32, scatter_st: f32, density: f32) -> Self {
        const INIT: GrainReader = GrainReader {
            dist: 0.0,
            speed: 1.0,
            active: false,
            age: 0,
            life: 1,
        };
        Self {
            line: [Line::with_ms(MAX_TIME_MS), Line::with_ms(MAX_TIME_MS)],
            delay_frames: ((delay_ms / 1000.0) * 48000.0) as usize,
            feedback,
            scatter_st,
            density,
            wet: 0.7,
            grains: [INIT; MAX_GRAINS],
            rng: 0xBEEF_F00D_1234_ABCD,
            spawn_timer: 0.0,
            sample_rate: 48000,
            delay_ms,
        }
    }

    fn rand_f32(&mut self) -> f32 {
        self.rng = self
            .rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.rng >> 33) as f32 / u32::MAX as f32
    }

    fn rand_signed(&mut self) -> f32 {
        self.rand_f32() * 2.0 - 1.0
    }

    /// The delay in frames, never further back than the line can be read.
    fn frames_for(&self, ms: f32) -> usize {
        ((ms * 0.001 * self.sample_rate as f32) as usize).clamp(1, self.line[0].capacity() - 8)
    }

    fn spawn_grain(&mut self) {
        let slot = self.grains.iter().position(|g| !g.active);
        let Some(idx) = slot else { return };
        // Somewhere in the last `delay_frames`, never nearer than the cubic
        // read can look.
        let dist = (self.rand_f32() * self.delay_frames as f32).max(4.0);
        let semitones = self.scatter_st * self.rand_signed();
        let speed = 2.0_f32.powf(semitones / 12.0);
        let grain_ms = 80.0 + self.rand_f32() * 120.0;
        let life = ((grain_ms / 1000.0) * self.sample_rate as f32) as u32;
        self.grains[idx] = GrainReader {
            dist,
            speed,
            active: true,
            age: 0,
            life,
        };
    }
}

impl FxProcessor for GranularDelay {
    /// The chain is **not** rebuilt for a knob turn — that would throw away the
    /// delay buffer, and a delay with no buffer is silence. Same order as
    /// `build_processor` maps them.
    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => {
                self.delay_ms = 20.0 + v * 980.0;
                self.delay_frames = self.frames_for(self.delay_ms);
            }
            1 => self.feedback = v,
            2 => self.scatter_st = (v - 0.5) * 24.0,
            3 => self.density = 1.0 + v * 31.0,
            _ => {}
        }
    }

    fn process_block(&mut self, block: &mut [f32], sample_rate: u32) {
        if sample_rate != self.sample_rate {
            self.sample_rate = sample_rate;
            self.delay_frames = self.frames_for(self.delay_ms);
        }
        let frames = block.len() / 2;
        let far = (self.line[0].capacity() - 8) as f32;
        let inter_spawn = sample_rate as f64 / self.density as f64;

        for i in 0..frames {
            let dry_l = block[i * 2];
            let dry_r = block[i * 2 + 1];

            // The feedback tap comes back into the line, so it is read with the
            // cubic: a two-tap average applied on every pass is a low-pass that
            // takes the top off the cloud.
            let d = self.delay_frames.max(2) as f32;
            let fb_l = self.line[0].read_cubic(d) * self.feedback;
            let fb_r = self.line[1].read_cubic(d) * self.feedback;
            self.line[0].write(dry_l + fb_l);
            self.line[1].write(dry_r + fb_r);

            // Spawn new grains.
            if self.spawn_timer <= 0.0 {
                self.spawn_grain();
                self.spawn_timer = inter_spawn;
            }
            self.spawn_timer -= 1.0;

            // Accumulate grain outputs.
            let mut gl = 0.0f32;
            let mut gr = 0.0f32;
            for grain in self.grains.iter_mut() {
                if !grain.active {
                    continue;
                }
                // A grain leaves the effect and does not come back, so linear
                // is what it wants: one average, once.
                let sl = self.line[0].read(grain.dist);
                let sr = self.line[1].read(grain.dist);
                // Hann envelope.
                let env_phase = grain.age as f32 / grain.life as f32;
                let env = (std::f32::consts::PI * env_phase).sin().powi(2);
                gl += sl * env;
                gr += sr * env;
                // The write head moves one frame a sample and the grain moves
                // `speed`, so this is the two of them closing or opening.
                grain.dist += 1.0 - grain.speed;
                grain.age += 1;
                // A grain that caught up with the write head has nothing left
                // to read but what is being written this instant. It ends here
                // rather than wrapping the buffer, which was a jump to audio a
                // whole delay away — an audible click at high scatter.
                if grain.age >= grain.life || !(4.0..=far).contains(&grain.dist) {
                    grain.active = false;
                }
            }

            block[i * 2] = dry_l * (1.0 - self.wet) + gl * self.wet;
            block[i * 2 + 1] = dry_r * (1.0 - self.wet) + gr * self.wet;
        }
    }

    fn reset(&mut self) {
        self.line[0].clear();
        self.line[1].clear();
        self.grains.iter_mut().for_each(|g| g.active = false);
        self.spawn_timer = 0.0;
    }

    fn set_mix(&mut self, wet: f32) {
        self.wet = wet.clamp(0.0, 1.0);
    }
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

    /// Heavy feedback with the pitch scatter wide open is the setting that used
    /// to wind up: the cloud has to stay bounded while it is fed, and reach
    /// true silence after — the shared line flushes denormals on the way in, so
    /// a tail that goes quiet stops costing anything.
    #[test]
    fn the_cloud_stays_bounded_and_reaches_silence() {
        let sr = 48_000u32;
        let mut d = GranularDelay::new(200.0, 0.0, 0.0, 0.0);
        d.set_param(0, 0.2);
        d.set_param(1, 0.9);
        d.set_param(2, 1.0);
        d.set_param(3, 1.0);
        d.set_mix(1.0);
        let mut rng = 0x1234_5678u32;
        let mut peak = 0.0f32;
        for _ in 0..200 {
            let mut block: Vec<f32> = (0..512)
                .map(|_| {
                    rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
                    (rng >> 8) as f32 / 16_777_216.0 - 0.5
                })
                .collect();
            d.process_block(&mut block, sr);
            peak = peak.max(block.iter().copied().map(f32::abs).fold(0.0, f32::max));
        }
        assert!(peak.is_finite() && peak < 8.0, "the cloud ran away: {peak}");

        // Backed off to 0.5 for the tail: at 0.9 the loop is still audible
        // half a minute later by design, and this test is about reaching zero
        // rather than about how long a decay takes.
        d.set_param(1, 0.5);
        let mut tail = 0.0f32;
        for _ in 0..1600 {
            let mut block = vec![0.0f32; 512];
            d.process_block(&mut block, sr);
            tail = block.iter().copied().map(f32::abs).fold(0.0, f32::max);
        }
        assert!(tail < 1e-6, "the tail never went quiet: {tail}");
    }

    #[test]
    fn gran_delay_reset_clears_buffers() {
        let mut d = GranularDelay::new(100.0, 0.5, 0.0, 4.0);
        let mut block = vec![1.0f32; 64];
        d.process_block(&mut block, 48000);
        d.reset();
        assert_eq!(d.line[0].read(100.0), 0.0);
        assert_eq!(d.line[1].read(100.0), 0.0);
    }
}
