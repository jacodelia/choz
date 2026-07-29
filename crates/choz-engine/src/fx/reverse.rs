//! Reverse delay — records the incoming audio and plays it back **backwards** in
//! overlapping, crossfaded segments. The reversed-segment idea (and its sync
//! cycle) is the same concept as ZynAddSubFX's `Reverse`/`Reverter` effect
//! (GPLv2, Michael Kirchner) — reimplemented independently for this MIT crate
//! with a free-running, overlap-add (granular) playback so it needs no host/MIDI
//! sync and never clicks.
//!
//! Two reverse playheads, staggered by half a segment, each Hann-windowed:
//! as one fades out the other fades in, so the constant-power crossfade hides
//! the segment seam. Optional feedback recirculates the reversed output for
//! cascading "backwards tape" textures.

use super::FxProcessor;

const MAX_BUF: usize = 105_800; // ~2.2 s @ 48 kHz
const HEADS: usize = 2;

#[derive(Clone, Copy)]
struct Head {
    anchor: usize, // write position captured at spawn (reverse reads back from here)
    ph: u32,       // samples elapsed since spawn
    life: u32,     // segment length in samples
    active: bool,
}

/// Reverse (backwards) delay with overlap-add crossfade.
pub struct ReverseDelay {
    sample_rate: u32,
    seg_ms: f32,
    feedback: f32,
    wet: f32,

    buf_l: Vec<f32>,
    buf_r: Vec<f32>,
    write: usize,
    heads: [Head; HEADS],
    spawn_timer: f64,
}

impl ReverseDelay {
    /// `time`/`feedback` normalised 0..1. `time`→50..2000 ms segment length.
    pub fn new(sr: u32, time: f32, feedback: f32) -> Self {
        const DEAD: Head = Head { anchor: 0, ph: 0, life: 1, active: false };
        Self {
            sample_rate: sr.max(8000),
            seg_ms: 50.0 + time.clamp(0.0, 1.0) * 1950.0,
            feedback: feedback.clamp(0.0, 0.95),
            wet: 0.5,
            buf_l: vec![0.0; MAX_BUF],
            buf_r: vec![0.0; MAX_BUF],
            write: 0,
            heads: [DEAD; HEADS],
            spawn_timer: 0.0,
        }
    }

    fn seg_frames(&self) -> u32 {
        ((self.seg_ms / 1000.0) * self.sample_rate as f32).clamp(2.0, (MAX_BUF - 1) as f32) as u32
    }

    fn spawn(&mut self, life: u32) {
        if let Some(slot) = self.heads.iter().position(|h| !h.active) {
            self.heads[slot] = Head { anchor: self.write, ph: 0, life, active: true };
        }
    }
}

impl FxProcessor for ReverseDelay {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        if sample_rate != self.sample_rate {
            self.sample_rate = sample_rate;
        }
        let len = self.buf_l.len();
        let seg = self.seg_frames();
        // Spawn a fresh reverse head every half segment → 2-way overlap.
        let inter = (seg as f64 / 2.0).max(1.0);
        let frames = buf.len() / 2;

        for i in 0..frames {
            let dry_l = buf[i * 2];
            let dry_r = buf[i * 2 + 1];

            if self.spawn_timer <= 0.0 {
                self.spawn(seg);
                self.spawn_timer += inter;
            }
            self.spawn_timer -= 1.0;

            // Sum the active reverse heads (Hann-windowed).
            let mut wl = 0.0f32;
            let mut wr = 0.0f32;
            for h in self.heads.iter_mut() {
                if !h.active { continue; }
                // Reverse read: back from the anchor by `ph` samples.
                let idx = (h.anchor + len - (h.ph as usize % len)) % len;
                let env = (std::f32::consts::PI * h.ph as f32 / h.life as f32).sin().powi(2);
                wl += self.buf_l[idx] * env;
                wr += self.buf_r[idx] * env;
                h.ph += 1;
                if h.ph >= h.life { h.active = false; }
            }

            // Record input + feedback of the reversed output.
            self.buf_l[self.write] = dry_l + wl * self.feedback;
            self.buf_r[self.write] = dry_r + wr * self.feedback;
            self.write = (self.write + 1) % len;

            buf[i * 2]     = dry_l + self.wet * (wl - dry_l);
            buf[i * 2 + 1] = dry_r + self.wet * (wr - dry_r);
        }
    }

    fn reset(&mut self) {
        self.buf_l.iter_mut().for_each(|v| *v = 0.0);
        self.buf_r.iter_mut().for_each(|v| *v = 0.0);
        self.write = 0;
        self.heads.iter_mut().for_each(|h| h.active = false);
        self.spawn_timer = 0.0;
    }

    fn set_mix(&mut self, wet: f32) { self.wet = wet.clamp(0.0, 1.0); }
    fn name(&self) -> &str { "Reverse Delay" }

    fn params(&self) -> Vec<super::FxParam> {
        use super::FxParam as P;
        vec![
            P::new("Time", ((self.seg_ms - 50.0) / 1950.0).clamp(0.0, 1.0), 0.0, 1.0, ""),
            P::new("Feedback", self.feedback / 0.95, 0.0, 1.0, ""),
            P::new("Wet", self.wet, 0.0, 1.0, ""),
        ]
    }

    /// Live update — preserves the record buffer + in-flight heads (no rebuild).
    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => self.seg_ms = 50.0 + v * 1950.0,
            1 => self.feedback = v * 0.95,
            2 => self.wet = v,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverse_plays_segment_backwards() {
        // Record a short ramp, then feed silence: the output should contain the
        // ramp played back (reversed), i.e. non-trivial energy with mix=1.
        let mut fx = ReverseDelay::new(48000, 0.1, 0.0);
        fx.set_mix(1.0);
        let mut ramp: Vec<f32> = (0..8192).flat_map(|i| { let s = (i % 256) as f32 / 256.0; [s, s] }).collect();
        fx.process_block(&mut ramp, 48000);
        let mut silence = vec![0.0f32; 8192];
        fx.process_block(&mut silence, 48000);
        let energy: f32 = silence.iter().map(|s| s.abs()).sum();
        assert!(energy > 0.0, "reversed segment should emit after the input");
        assert!(silence.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn reverse_reset_clears_buffer() {
        let mut fx = ReverseDelay::new(48000, 0.3, 0.5);
        let mut block = vec![0.7f32; 512];
        fx.process_block(&mut block, 48000);
        fx.reset();
        assert!(fx.buf_l.iter().all(|&v| v == 0.0));
    }
}
