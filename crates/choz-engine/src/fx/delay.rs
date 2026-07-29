//! Stereo delay line with feedback, high-shelf damping and L/R crossfeed.
//!
//! The continuous L/R crossfeed control is the same idea as the `lrcross`
//! parameter in ZynAddSubFX's `Echo` effect (GPLv2) — a standard ping-pong-ish
//! channel blend. No code was copied; reimplemented independently for this
//! MIT-licensed crate (and the asymmetric-ordering quirk of the original is
//! fixed here: both outputs are mixed from the *pre-mix* echoes).

use super::FxProcessor;

/// Stereo delay (ping-pong capable).
pub struct DelayLine {
    buf_l: Vec<f32>,
    buf_r: Vec<f32>,
    /// Write head (frames).
    write: usize,
    /// Delay in frames (set when sample_rate is known).
    delay_frames: usize,
    /// Feedback level (0.0–0.95).
    feedback: f32,
    /// Damping: 1-pole LP on feedback path (0.0 = no damp, 1.0 = max).
    damp: f32,
    damp_state_l: f32,
    damp_state_r: f32,
    /// Ping-pong: swap L/R on each echo.
    ping_pong: bool,
    /// L/R crossfeed (0 = independent channels, 0.5 = mono blend, 1 = full swap).
    cross: f32,
    wet: f32,
    /// Delay time in milliseconds (for reinitialisation on sample-rate change).
    delay_ms: f32,
    sample_rate: u32,
}

impl DelayLine {
    pub fn new(delay_ms: f32, feedback: f32, damp: f32) -> Self {
        let max_cap = 192001; // ~4 s @ 48 kHz
        Self {
            buf_l: vec![0.0; max_cap],
            buf_r: vec![0.0; max_cap],
            write: 0,
            delay_frames: ((delay_ms / 1000.0) * 48000.0) as usize,
            feedback: feedback.clamp(0.0, 0.95),
            damp: damp.clamp(0.0, 1.0),
            damp_state_l: 0.0,
            damp_state_r: 0.0,
            ping_pong: false,
            cross: 0.0,
            wet: 0.5,
            delay_ms,
            sample_rate: 48000,
        }
    }

    pub fn set_crossfeed(&mut self, c: f32) {
        self.cross = c.clamp(0.0, 1.0);
    }

    pub fn set_delay_ms(&mut self, ms: f32) {
        self.delay_ms = ms.clamp(1.0, 4000.0);
        self.update_delay_frames();
    }

    pub fn set_feedback(&mut self, fb: f32) {
        self.feedback = fb.clamp(0.0, 0.95);
    }

    pub fn set_damp(&mut self, d: f32) {
        self.damp = d.clamp(0.0, 1.0);
    }

    pub fn set_ping_pong(&mut self, pp: bool) {
        self.ping_pong = pp;
    }

    fn update_delay_frames(&mut self) {
        let frames = ((self.delay_ms / 1000.0) * self.sample_rate as f32) as usize;
        self.delay_frames = frames.clamp(1, self.buf_l.len() - 1);
    }

    #[inline]
    fn read(&self, buf: &[f32], offset: usize) -> f32 {
        let cap = buf.len();
        let idx = if self.write >= offset {
            self.write - offset
        } else {
            cap - (offset - self.write)
        };
        buf[idx % cap]
    }

    #[inline]
    fn write_sample(&mut self, l: f32, r: f32) {
        self.buf_l[self.write] = l;
        self.buf_r[self.write] = r;
        self.write += 1;
        if self.write >= self.buf_l.len() { self.write = 0; }
    }
}

impl FxProcessor for DelayLine {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        if self.sample_rate != sample_rate {
            self.sample_rate = sample_rate;
            self.update_delay_frames();
        }
        let d = self.delay_frames.max(1);
        let frames = buf.len() / 2;
        for i in 0..frames {
            let dry_l = buf[i * 2];
            let dry_r = buf[i * 2 + 1];

            let read_l = self.read(&self.buf_l, d);
            let read_r = self.read(&self.buf_r, d);

            // L/R crossfeed: continuous blend of the two echo channels. Both
            // outputs are mixed from the pre-blend reads (symmetric). cross=0 →
            // independent, 0.5 → mono, 1 → full swap. Feeds the loop too, so the
            // image keeps spreading on each repeat.
            let (echo_l, echo_r) = if self.cross > 0.0 {
                let c = self.cross;
                (read_l * (1.0 - c) + read_r * c, read_r * (1.0 - c) + read_l * c)
            } else {
                (read_l, read_r)
            };

            // One-pole LP damping on feedback
            self.damp_state_l = echo_l + self.damp * (self.damp_state_l - echo_l);
            self.damp_state_r = echo_r + self.damp * (self.damp_state_r - echo_r);

            let (fb_l, fb_r) = if self.ping_pong {
                (self.damp_state_r, self.damp_state_l)
            } else {
                (self.damp_state_l, self.damp_state_r)
            };

            self.write_sample(
                dry_l + fb_l * self.feedback,
                dry_r + fb_r * self.feedback,
            );

            buf[i * 2]     = dry_l + self.wet * echo_l;
            buf[i * 2 + 1] = dry_r + self.wet * echo_r;
        }
    }

    fn reset(&mut self) {
        self.buf_l.fill(0.0);
        self.buf_r.fill(0.0);
        self.write = 0;
        self.damp_state_l = 0.0;
        self.damp_state_r = 0.0;
    }

    fn set_mix(&mut self, wet: f32) { self.wet = wet.clamp(0.0, 1.0); }
    fn name(&self) -> &str { "Delay" }

    fn params(&self) -> Vec<crate::fx::FxParam> {
        use crate::fx::FxParam;
        vec![
            FxParam::new("Time",      (self.delay_ms / 2000.0).clamp(0.0, 1.0), 0.0, 2000.0, "ms"),
            FxParam::new("Feedback",  self.feedback / 0.95, 0.0, 0.95, ""),
            FxParam::new("Damping",   self.damp, 0.0, 1.0, ""),
            FxParam::new("PingPong",  if self.ping_pong { 1.0 } else { 0.0 }, 0.0, 1.0, ""),
            FxParam::new("Wet",       self.wet, 0.0, 1.0, ""),
            FxParam::new("Cross",     self.cross, 0.0, 1.0, ""),
        ]
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => { self.delay_ms = 10.0 + v * 990.0; self.delay_frames = (self.delay_ms / 1000.0 * self.sample_rate as f32) as usize; }
            1 => self.feedback  = v,
            2 => self.damp      = v,
            3 => self.ping_pong = v >= 0.5,
            4 => self.wet       = v,
            5 => self.cross     = v,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_produces_echo_after_delay_time() {
        let sr = 1000u32; // 1 kHz for easy math
        let mut dl = DelayLine::new(100.0, 0.0, 0.0); // 100 ms = 100 frames
        dl.set_mix(1.0);
        // Emit one impulse
        let mut buf = vec![0.0f32; 400]; // 200 frames
        buf[0] = 1.0; // L impulse at frame 0
        buf[1] = 0.0;
        dl.process_block(&mut buf, sr);
        // Echo should appear at frame 100 (index 200)
        assert!(buf[200] > 0.5, "echo at frame 100 expected, got {}", buf[200]);
    }

    #[test]
    fn crossfeed_full_swaps_channels() {
        let sr = 1000u32;
        let mut dl = DelayLine::new(100.0, 0.0, 0.0); // 100 ms = 100 frames
        dl.set_mix(1.0);
        dl.set_crossfeed(1.0); // full swap
        let mut buf = vec![0.0f32; 400]; // 200 frames; L impulse only
        buf[0] = 1.0;
        dl.process_block(&mut buf, sr);
        // With full crossfeed, an L-only impulse echoes on the R channel.
        assert!(buf[201] > 0.5, "R echo expected at frame 100, got {}", buf[201]);
        assert!(buf[200].abs() < 0.01, "L echo should be silent, got {}", buf[200]);
    }

    #[test]
    fn reset_clears_buffer() {
        let mut dl = DelayLine::new(50.0, 0.5, 0.0);
        let mut buf = vec![1.0f32; 64];
        dl.process_block(&mut buf, 48000);
        dl.reset();
        assert!(dl.buf_l.iter().all(|&s| s == 0.0));
    }
}
