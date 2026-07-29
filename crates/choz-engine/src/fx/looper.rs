//! Looper / stutter FX.
//!
//! A fixed-length stereo looper pre-allocated at construction time.
//! States: Idle → Recording → Playing → Overdub
//!
//! - `record()` — starts recording from the current position (wraps at capacity)
//! - `stop_record()` — freezes the loop length at current position, transitions to Playing
//! - `play()` — (re)starts playback from the top
//! - `stop()` — stops playback (output silence)
//! - `overdub()` — plays + mixes new input on top of the loop
//!
//! RT-safe: no allocation after `new()`. State transitions happen in `process_block()`
//! when `pending_cmd` is set — so they are gated to block boundaries.

use super::FxProcessor;

const MAX_LOOP_SECS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LooperState {
    Idle,
    Recording,
    Playing,
    Overdub,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cmd { Record, StopRecord, Play, Stop, Overdub, Clear }

/// Looper / stutter effect.
pub struct Looper {
    buf: Vec<f32>,        // interleaved stereo, pre-allocated for MAX_LOOP_SECS @ 48 kHz
    cap_frames: usize,    // max frames in buf
    loop_frames: usize,   // frozen loop length (0 = not yet set)
    write_pos: usize,
    read_pos: usize,
    state: LooperState,
    pending_cmd: Option<Cmd>,
    wet: f32,
    overdub_mix: f32,     // 0.0–1.0: how much old loop survives on overdub (default 0.85)
}

impl Looper {
    pub fn new(sample_rate: u32) -> Self {
        let cap = MAX_LOOP_SECS * sample_rate as usize;
        Self {
            buf: vec![0.0f32; cap * 2],
            cap_frames: cap,
            loop_frames: 0,
            write_pos: 0,
            read_pos: 0,
            state: LooperState::Idle,
            pending_cmd: None,
            wet: 1.0,
            overdub_mix: 0.85,
        }
    }

    pub fn state(&self) -> LooperState { self.state }

    /// Start recording (overwrites existing loop).
    pub fn record(&mut self) { self.pending_cmd = Some(Cmd::Record); }

    /// Stop recording and lock the loop length; begin playback.
    pub fn stop_record(&mut self) { self.pending_cmd = Some(Cmd::StopRecord); }

    /// Toggle record/stop_record in one call.
    pub fn toggle_record(&mut self) {
        match self.state {
            LooperState::Recording => self.stop_record(),
            _ => self.record(),
        }
    }

    /// Restart playback.
    pub fn play(&mut self) { self.pending_cmd = Some(Cmd::Play); }

    /// Stop playback.
    pub fn stop(&mut self) { self.pending_cmd = Some(Cmd::Stop); }

    /// Toggle play/stop.
    pub fn toggle_play(&mut self) {
        match self.state {
            LooperState::Playing | LooperState::Overdub => self.stop(),
            _ => self.play(),
        }
    }

    /// Enable overdub (play + add new input).
    pub fn overdub(&mut self) { self.pending_cmd = Some(Cmd::Overdub); }

    /// Clear the loop buffer and return to Idle.
    pub fn clear(&mut self) { self.pending_cmd = Some(Cmd::Clear); }

    pub fn set_overdub_mix(&mut self, v: f32) { self.overdub_mix = v.clamp(0.0, 1.0); }
}

impl FxProcessor for Looper {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        // Apply any pending command at block boundary.
        if let Some(cmd) = self.pending_cmd.take() {
            match cmd {
                Cmd::Record => {
                    // Resize cap if sample_rate changed.
                    let new_cap = MAX_LOOP_SECS * sample_rate as usize;
                    if new_cap != self.cap_frames {
                        self.buf = vec![0.0f32; new_cap * 2];
                        self.cap_frames = new_cap;
                    }
                    self.write_pos = 0;
                    self.loop_frames = 0;
                    self.state = LooperState::Recording;
                }
                Cmd::StopRecord => {
                    if self.state == LooperState::Recording {
                        self.loop_frames = self.write_pos.min(self.cap_frames);
                        self.read_pos = 0;
                        self.state = LooperState::Playing;
                    }
                }
                Cmd::Play => {
                    self.read_pos = 0;
                    self.state = LooperState::Playing;
                }
                Cmd::Stop => {
                    self.state = LooperState::Idle;
                }
                Cmd::Overdub => {
                    if self.loop_frames > 0 {
                        self.read_pos = 0;
                        self.state = LooperState::Overdub;
                    }
                }
                Cmd::Clear => {
                    self.buf.fill(0.0);
                    self.loop_frames = 0;
                    self.write_pos = 0;
                    self.read_pos = 0;
                    self.state = LooperState::Idle;
                }
            }
        }

        let frames = buf.len() / 2;
        match self.state {
            LooperState::Idle => {}

            LooperState::Recording => {
                for i in 0..frames {
                    if self.write_pos < self.cap_frames {
                        self.buf[self.write_pos * 2]     = buf[i * 2];
                        self.buf[self.write_pos * 2 + 1] = buf[i * 2 + 1];
                        self.write_pos += 1;
                    } else {
                        // Buffer full — auto-stop and play
                        self.loop_frames = self.cap_frames;
                        self.read_pos    = 0;
                        self.state       = LooperState::Playing;
                        break;
                    }
                }
            }

            LooperState::Playing => {
                if self.loop_frames == 0 { return; }
                for i in 0..frames {
                    let loop_l = self.buf[self.read_pos * 2];
                    let loop_r = self.buf[self.read_pos * 2 + 1];
                    buf[i * 2]     = buf[i * 2]     + self.wet * (loop_l - buf[i * 2]);
                    buf[i * 2 + 1] = buf[i * 2 + 1] + self.wet * (loop_r - buf[i * 2 + 1]);
                    self.read_pos = (self.read_pos + 1) % self.loop_frames;
                }
            }

            LooperState::Overdub => {
                if self.loop_frames == 0 { return; }
                for i in 0..frames {
                    // Mix input onto existing loop
                    self.buf[self.read_pos * 2]     =
                        self.buf[self.read_pos * 2]     * self.overdub_mix + buf[i * 2];
                    self.buf[self.read_pos * 2 + 1] =
                        self.buf[self.read_pos * 2 + 1] * self.overdub_mix + buf[i * 2 + 1];
                    // Output the loop
                    let loop_l = self.buf[self.read_pos * 2];
                    let loop_r = self.buf[self.read_pos * 2 + 1];
                    buf[i * 2]     = buf[i * 2]     + self.wet * (loop_l - buf[i * 2]);
                    buf[i * 2 + 1] = buf[i * 2 + 1] + self.wet * (loop_r - buf[i * 2 + 1]);
                    self.read_pos = (self.read_pos + 1) % self.loop_frames;
                }
            }
        }
    }

    fn reset(&mut self) {
        self.buf.fill(0.0);
        self.loop_frames = 0;
        self.write_pos   = 0;
        self.read_pos    = 0;
        self.state       = LooperState::Idle;
        self.pending_cmd = None;
    }

    fn set_mix(&mut self, wet: f32) { self.wet = wet.clamp(0.0, 1.0); }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fill_sine(sr: u32, freq: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let s = (2.0 * std::f32::consts::PI * freq * i as f32 / sr as f32).sin();
                [s, s]
            })
            .collect()
    }

    #[test]
    fn record_then_play() {
        let sr = 48000u32;
        let mut lp = Looper::new(sr);

        // Record 1024 frames
        lp.record();
        let input = fill_sine(sr, 440.0, 1024);
        let mut buf = input.clone();
        lp.process_block(&mut buf, sr);
        assert_eq!(lp.state(), LooperState::Recording);

        lp.stop_record();
        let mut buf2 = vec![0.0f32; 256];
        lp.process_block(&mut buf2, sr);
        assert_eq!(lp.state(), LooperState::Playing);
        // Some loop content should appear in output
        let energy: f32 = buf2.iter().map(|&s| s * s).sum();
        assert!(energy > 0.0, "playing loop should produce output");
    }

    #[test]
    fn idle_passes_silence() {
        let mut lp = Looper::new(48000);
        let mut buf = vec![0.5f32; 64];
        lp.process_block(&mut buf, 48000);
        // Idle: input unchanged
        assert_eq!(buf[0], 0.5);
    }

    #[test]
    fn clear_resets_to_idle() {
        let mut lp = Looper::new(48000);
        lp.record();
        let mut buf = vec![1.0f32; 64];
        lp.process_block(&mut buf, 48000);
        lp.clear();
        lp.process_block(&mut buf, 48000);
        assert_eq!(lp.state(), LooperState::Idle);
        assert_eq!(lp.loop_frames, 0);
    }
}
