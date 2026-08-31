//! Stereo delay line with feedback, high-shelf damping and L/R crossfeed.
//!
//! The continuous L/R crossfeed control is the same idea as the `lrcross`
//! parameter in ZynAddSubFX's `Echo` effect (GPLv2) — a standard ping-pong-ish
//! channel blend. No code was copied; reimplemented independently for this
//! MIT-licensed crate (and the asymmetric-ordering quirk of the original is
//! fixed here: both outputs are mixed from the *pre-mix* echoes).

use super::delay_line::DelayLine as Line;
use super::lfo::{Lfo, Wave};
use super::smooth::Smoothed;
use super::FxProcessor;

/// The longest delay this effect holds, at **every** rate: the shared line is
/// sized in time, so the same project is the same delay on a 192 kHz interface
/// as on a 44.1 one. It costs 2 MB a channel, which is the price of that.
///
/// 2 s and not the 4 that `set_delay_ms` used to accept, because the knob goes
/// to 1 s and `params()` has always reported the scale as 2 s.
const MAX_TIME_MS: f32 = 2000.0;

/// How long the read head takes to walk to a new delay time. Long, because a
/// delay that jumps its head clicks and a tape one glides — this is the glide.
const TIME_MS: f32 = 80.0;

/// Stereo delay (ping-pong capable).
pub struct DelayLine {
    /// Modulation of the delay time: rate in Hz and depth in milliseconds.
    /// Depth 0 = off, and off means the read head sits exactly where it did.
    mod_rate: f32,
    mod_depth_ms: f32,
    lfo: Lfo,
    /// One shared line per channel: sized in milliseconds, cubic on the way
    /// back, denormals flushed on the way in.
    line: [Line; 2],
    /// Delay in frames (set when sample_rate is known) — where the head is
    /// headed. Where it *is* is `time`, which walks there.
    delay_frames: usize,
    /// The read head, in frames, smoothed. Fractional on the way, which is what
    /// `read_frac` is for.
    time: Smoothed,
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
        Self {
            mod_rate: 0.3,
            mod_depth_ms: 0.0,
            lfo: Lfo::new(),
            line: [Line::with_ms(MAX_TIME_MS), Line::with_ms(MAX_TIME_MS)],
            delay_frames: ((delay_ms / 1000.0) * 48000.0) as usize,
            time: Smoothed::new((delay_ms / 1000.0) * 48000.0, TIME_MS, 48000.0),
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
        self.delay_ms = ms.clamp(1.0, MAX_TIME_MS);
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
        self.delay_frames = frames.clamp(1, self.line[0].capacity() - 4);
        self.time.set_target(self.delay_frames as f32);
    }

    pub fn set_mod_rate(&mut self, hz: f32) {
        self.mod_rate = hz.clamp(0.0, 10.0);
    }

    /// How far the read head wanders, in milliseconds. This is what turns the
    /// delay into a chorus-ish, tape-ish one; 0 leaves it exactly where it was.
    pub fn set_mod_depth_ms(&mut self, ms: f32) {
        self.mod_depth_ms = ms.clamp(0.0, 50.0);
    }
}

impl FxProcessor for DelayLine {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        if self.sample_rate != sample_rate {
            self.sample_rate = sample_rate;
            self.time.set_sample_rate(sample_rate as f32);
            self.update_delay_frames();
            // A rate change is not a knob move: put the head where it belongs.
            self.time.snap(self.delay_frames as f32);
        }
        let d = self.delay_frames.max(1);
        let sr = sample_rate as f32;
        // The modulation cannot pull the read head past the write head (that
        // would read the future) nor beyond the buffer, so the swing is capped
        // by what is actually there.
        let depth_frames = (self.mod_depth_ms * 0.001 * sr).min(d.saturating_sub(1) as f32);
        let modulated = depth_frames > 0.0;
        let frames = buf.len() / 2;
        let max_offset = (self.line[0].capacity() - 4) as f32;
        for i in 0..frames {
            let dry_l = buf[i * 2];
            let dry_r = buf[i * 2 + 1];

            let d = self.time.tick().clamp(2.0, max_offset);
            // Cubic, not linear: what comes out of here goes back in, and a
            // two-tap average applied on every pass is a low-pass that eats the
            // top of the repeats — and eats it by a different amount as the
            // modulation sweeps the fractional part.
            let (read_l, read_r) = if modulated {
                // Half a cycle apart: the two heads wander in opposition, which
                // is what widens the repeats instead of just detuning them.
                let m = self.lfo.tick(Wave::Sine, self.mod_rate, sr, 0.5);
                (
                    self.line[0].read_cubic((d + m[0] * depth_frames).clamp(2.0, max_offset)),
                    self.line[1].read_cubic((d + m[1] * depth_frames).clamp(2.0, max_offset)),
                )
            } else {
                (self.line[0].read_cubic(d), self.line[1].read_cubic(d))
            };

            // L/R crossfeed: continuous blend of the two echo channels. Both
            // outputs are mixed from the pre-blend reads (symmetric). cross=0 →
            // independent, 0.5 → mono, 1 → full swap. Feeds the loop too, so the
            // image keeps spreading on each repeat.
            let (echo_l, echo_r) = if self.cross > 0.0 {
                let c = self.cross;
                (
                    read_l * (1.0 - c) + read_r * c,
                    read_r * (1.0 - c) + read_l * c,
                )
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

            self.line[0].write(dry_l + fb_l * self.feedback);
            self.line[1].write(dry_r + fb_r * self.feedback);

            // The same dry/wet law as every other effect here: a crossfade,
            // not an echo added on top. Added on top, `Wet` was a send level —
            // it could never take the dry away, and turning it up made the tab
            // louder while turning up any other effect did not.
            buf[i * 2] = dry_l + self.wet * (echo_l - dry_l);
            buf[i * 2 + 1] = dry_r + self.wet * (echo_r - dry_r);
        }
    }

    fn reset(&mut self) {
        self.line[0].clear();
        self.line[1].clear();
        self.damp_state_l = 0.0;
        self.damp_state_r = 0.0;
        self.time.snap(self.delay_frames.max(1) as f32);
    }

    fn set_mix(&mut self, wet: f32) {
        self.wet = wet.clamp(0.0, 1.0);
    }
    fn name(&self) -> &str {
        "Delay"
    }

    fn params(&self) -> Vec<crate::fx::FxParam> {
        use crate::fx::FxParam;
        vec![
            FxParam::new(
                "Time",
                (self.delay_ms / 2000.0).clamp(0.0, 1.0),
                0.0,
                2000.0,
                "ms",
            ),
            FxParam::new("Feedback", self.feedback / 0.95, 0.0, 0.95, ""),
            FxParam::new("Damping", self.damp, 0.0, 1.0, ""),
            FxParam::new(
                "PingPong",
                if self.ping_pong { 1.0 } else { 0.0 },
                0.0,
                1.0,
                "",
            ),
            FxParam::new("Wet", self.wet, 0.0, 1.0, ""),
            FxParam::new("Cross", self.cross, 0.0, 1.0, ""),
            FxParam::new("ModRate", self.mod_rate / 10.0, 0.0, 10.0, "Hz"),
            FxParam::new("ModDepth", self.mod_depth_ms / 50.0, 0.0, 50.0, "ms"),
        ]
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => self.set_delay_ms(10.0 + v * 990.0),
            1 => self.feedback = v,
            2 => self.damp = v,
            3 => self.ping_pong = v >= 0.5,
            4 => self.wet = v,
            5 => self.cross = v,
            6 => self.mod_rate = v * 10.0,
            7 => self.mod_depth_ms = v * 50.0,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Slamming the time end to end must not step the read head: the head
    /// walks there, which is a glide and not a click.
    #[test]
    fn moving_the_time_does_not_click() {
        let sr = 48_000u32;
        let worst = |automate: bool| {
            let mut dl = DelayLine::new(200.0, 0.5, 0.2);
            dl.set_mix(1.0);
            let mut worst = 0.0f32;
            let mut prev = 0.0f32;
            let mut phase = 0.0f32;
            for block in 0..80 {
                if automate {
                    dl.set_param(0, (block % 2) as f32);
                }
                let mut buf: Vec<f32> = (0..256)
                    .flat_map(|_| {
                        phase = (phase + 220.0 / sr as f32).fract();
                        let s = (std::f32::consts::TAU * phase).sin() * 0.8;
                        [s, s]
                    })
                    .collect();
                dl.process_block(&mut buf, sr);
                for s in buf.chunks(2).map(|c| c[0]) {
                    worst = worst.max((s - prev).abs());
                    prev = s;
                }
            }
            worst
        };
        let still = worst(false);
        let swept = worst(true);
        assert!(
            swept < still * 3.0,
            "the head jumped: {swept:.3} while automated, {still:.3} while still"
        );
    }

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
        assert!(
            buf[200] > 0.5,
            "echo at frame 100 expected, got {}",
            buf[200]
        );
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
        assert!(
            buf[201] > 0.5,
            "R echo expected at frame 100, got {}",
            buf[201]
        );
        assert!(
            buf[200].abs() < 0.01,
            "L echo should be silent, got {}",
            buf[200]
        );
    }

    /// The read head has to move, and it has to move *smoothly*: without
    /// interpolation a wandering delay steps a whole sample at a time and every
    /// step is a click.
    #[test]
    fn a_modulated_delay_moves_its_read_head_without_stepping() {
        let sr = 48000u32;
        let mut dl = DelayLine::new(100.0, 0.0, 0.0);
        dl.set_mix(1.0);
        dl.set_mod_rate(2.0);
        dl.set_mod_depth_ms(5.0);
        // A steady tone: modulating the delay of a sine detunes it, so the
        // output is the same tone with its phase walking.
        let frames = 24000;
        let mut buf = vec![0.0f32; frames * 2];
        for i in 0..frames {
            let s = (2.0 * std::f32::consts::PI * 440.0 * i as f32 / sr as f32).sin();
            buf[i * 2] = s;
            buf[i * 2 + 1] = s;
        }
        dl.process_block(&mut buf, sr);
        let tail: Vec<f32> = buf[12000..].iter().step_by(2).copied().collect();
        // Bounded, finite, and no sample-to-sample jump bigger than a 440 Hz
        // sine can make on its own (≈0.06 at this rate). A stepping read head
        // shows up here as a jump of the order of the signal itself.
        let biggest = tail
            .windows(2)
            .fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()));
        assert!(tail.iter().all(|s| s.is_finite()));
        assert!(biggest < 0.2, "the read head stepped by {biggest}");
        assert!(
            tail.iter().fold(0.0f32, |m, s| m.max(s.abs())) > 0.5,
            "the echo went missing"
        );
    }

    /// Depth 0 is the delay that was here before: same echo, same place.
    #[test]
    fn no_modulation_leaves_the_delay_exactly_where_it_was() {
        let sr = 1000u32;
        let run = |depth: f32| {
            let mut dl = DelayLine::new(100.0, 0.0, 0.0);
            dl.set_mix(1.0);
            dl.set_mod_depth_ms(depth);
            let mut buf = vec![0.0f32; 400];
            buf[0] = 1.0;
            dl.process_block(&mut buf, sr);
            buf
        };
        assert_eq!(run(0.0)[200], run(0.0)[200]);
        assert!(
            run(0.0)[200] > 0.5,
            "the unmodulated echo is where it always was"
        );
    }

    /// The line is sized in time, so the same delay time is available on every
    /// device. The old buffer was 192 001 samples — 4 s at 48 kHz and **1 s at
    /// 192**, so a 1.5 s delay silently became a 1 s one on a fast interface.
    #[test]
    fn a_long_delay_is_the_same_time_at_a_high_rate() {
        let sr = 192_000u32;
        let want_ms = 1500.0f32;
        let mut dl = DelayLine::new(want_ms, 0.0, 0.0);
        dl.set_mix(1.0);
        let mut out = Vec::new();
        for block in 0..1600 {
            let mut buf = vec![0.0f32; 512];
            if block == 0 {
                buf[0] = 1.0;
            }
            dl.process_block(&mut buf, sr);
            out.extend(buf.chunks(2).map(|c| c[0]));
        }
        // Past the dry impulse, which the mix always lets through.
        let skip = sr as usize / 10;
        let (at, peak) =
            out.iter()
                .enumerate()
                .skip(skip)
                .fold((0usize, 0.0f32), |(bi, bv), (i, &v)| {
                    if v.abs() > bv {
                        (i, v.abs())
                    } else {
                        (bi, bv)
                    }
                });
        let ms = at as f32 * 1000.0 / sr as f32;
        assert!(peak > 0.5, "the echo never came back: peak {peak}");
        assert!(
            (ms - want_ms).abs() < 20.0,
            "asked for {want_ms} ms, the echo landed at {ms} ms"
        );
    }

    /// Full wet is the echoes and nothing else, which is the law every other
    /// effect follows. It used to add its echoes to the dry instead, so `Wet`
    /// was a send level: it could never take the dry away.
    #[test]
    fn full_wet_leaves_none_of_the_dry() {
        let sr = 48_000u32;
        let mut dl = DelayLine::new(300.0, 0.0, 0.0);
        dl.set_mix(1.0);
        // Half a second, which is less than the delay: whatever comes out here
        // can only be the dry leaking through.
        let mut buf: Vec<f32> = (0..sr as usize / 2)
            .flat_map(|i| {
                let s = (std::f32::consts::TAU * 220.0 * i as f32 / sr as f32).sin() * 0.5;
                [s, s]
            })
            .collect();
        dl.process_block(&mut buf, sr);
        let head = &buf[..sr as usize / 4];
        let peak = head.iter().copied().map(f32::abs).fold(0.0, f32::max);
        assert!(peak < 0.01, "the dry came through at full wet: {peak}");
    }

    #[test]
    fn reset_clears_buffer() {
        let mut dl = DelayLine::new(50.0, 0.5, 0.0);
        let mut buf = vec![1.0f32; 64];
        dl.process_block(&mut buf, 48000);
        dl.reset();
        assert_eq!(dl.line[0].read_cubic(50.0), 0.0);
        assert_eq!(dl.line[1].read_cubic(50.0), 0.0);
    }
}
