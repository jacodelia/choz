//! Multi-tap delay: four heads on one line, each with its own time, level and
//! place in the image.
//!
//! The stereo delay next door is one repeat fed back into itself, which is what
//! an echo is. This is the other thing: four *separate* taps off the same
//! recording, so the pattern is written rather than decayed — a triplet against
//! the beat, a stereo bounce that is not a ping-pong, a slap plus a long one.
//!
//! Times are set as a fraction of `Time`, so moving that one knob keeps the
//! pattern and changes its tempo. Feedback is taken from the **last** tap, so
//! turning it up repeats the whole figure instead of just the first head.

use super::delay_line::DelayLine as Line;
use choz_ports::{FxParam, FxProcessor};

pub const TAPS: usize = 4;

/// Longest a tap can be, in ms — and the line is built for it.
const MAX_MS: f32 = 2000.0;

struct Tap {
    /// Where this tap sits, as a fraction of `Time`.
    frac: f32,
    level: f32,
    /// −1 left, 0 centre, +1 right.
    pan: f32,
}

pub struct MultiTapDelay {
    line: [Line; 2],
    taps: [Tap; TAPS],
    time_ms: f32,
    feedback: f32,
    wet: f32,
    sample_rate: f32,
}

impl MultiTapDelay {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            line: [Line::with_ms(MAX_MS + 50.0), Line::with_ms(MAX_MS + 50.0)],
            // Opens as a pattern rather than as four heads in the same place:
            // four taps all at `Time` is one tap four times as loud.
            taps: [
                Tap { frac: 0.25, level: 0.8, pan: -0.7 },
                Tap { frac: 0.50, level: 0.6, pan: 0.7 },
                Tap { frac: 0.75, level: 0.45, pan: -0.4 },
                Tap { frac: 1.00, level: 0.35, pan: 0.4 },
            ],
            time_ms: 500.0,
            feedback: 0.25,
            wet: 0.4,
            sample_rate: sample_rate.max(8000) as f32,
        }
    }

    pub fn with_params(sample_rate: u32, params: &[f32]) -> Self {
        let mut d = Self::new(sample_rate);
        for (i, p) in params.iter().enumerate() {
            <Self as FxProcessor>::set_param(&mut d, i, *p);
        }
        d
    }
}

impl FxProcessor for MultiTapDelay {
    fn name(&self) -> &str {
        "Multi-tap Delay"
    }

    fn params(&self) -> Vec<FxParam> {
        let mut out = vec![FxParam::new(
            "Time",
            (self.time_ms - 20.0) / (MAX_MS - 20.0),
            20.0,
            MAX_MS,
            "ms",
        )];
        for (i, t) in self.taps.iter().enumerate() {
            out.push(FxParam::new(
                match i {
                    0 => "T1 Time",
                    1 => "T2 Time",
                    2 => "T3 Time",
                    _ => "T4 Time",
                },
                t.frac,
                0.0,
                1.0,
                "x",
            ));
            out.push(FxParam::new(
                match i {
                    0 => "T1 Level",
                    1 => "T2 Level",
                    2 => "T3 Level",
                    _ => "T4 Level",
                },
                t.level,
                0.0,
                1.0,
                "",
            ));
            out.push(FxParam::new(
                match i {
                    0 => "T1 Pan",
                    1 => "T2 Pan",
                    2 => "T3 Pan",
                    _ => "T4 Pan",
                },
                (t.pan + 1.0) * 0.5,
                -1.0,
                1.0,
                "",
            ));
        }
        out.push(FxParam::new("Feedback", self.feedback, 0.0, 0.95, ""));
        out.push(FxParam::new("Wet", self.wet, 0.0, 1.0, ""));
        out
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        if index == 0 {
            self.time_ms = 20.0 + v * (MAX_MS - 20.0);
            return;
        }
        let tap_params = TAPS * 3;
        if index <= tap_params {
            let i = (index - 1) / 3;
            match (index - 1) % 3 {
                0 => self.taps[i].frac = v,
                1 => self.taps[i].level = v,
                _ => self.taps[i].pan = v * 2.0 - 1.0,
            }
            return;
        }
        match index - tap_params - 1 {
            0 => self.feedback = v * 0.95,
            1 => self.wet = v,
            _ => {}
        }
    }

    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        let sr = sample_rate.max(8000) as f32;
        self.sample_rate = sr;
        let cap = self.line[0].capacity() as f32 - 4.0;
        for frame in buf.chunks_exact_mut(2) {
            let dry = [frame[0], frame[1]];
            let mut wet = [0.0f32; 2];
            // The last tap is the one that feeds back, so the whole figure
            // repeats rather than only its first head.
            let mut last = [0.0f32; 2];
            for tap in &self.taps {
                let d = (self.time_ms * tap.frac * 0.001 * sr).clamp(1.0, cap);
                for ch in 0..2 {
                    let s = self.line[ch].read_cubic(d);
                    last[ch] = s;
                    // Constant-power pan, so a tap swept across the image keeps
                    // its level.
                    let side = match ch {
                        0 => (1.0 - tap.pan) * 0.5,
                        _ => (1.0 + tap.pan) * 0.5,
                    };
                    wet[ch] += s * tap.level * side.max(0.0).sqrt();
                }
            }
            for ch in 0..2 {
                self.line[ch].write(super::delay_line::safe(
                    dry[ch] + last[ch] * self.feedback,
                ));
                frame[ch] = dry[ch] + self.wet * (wet[ch] - dry[ch]);
            }
        }
    }

    fn reset(&mut self) {
        for l in &mut self.line {
            l.clear();
        }
    }

    fn set_mix(&mut self, wet: f32) {
        self.wet = wet.clamp(0.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One click in, four echoes out, at the times the taps were set to.
    #[test]
    fn every_tap_arrives_when_it_was_told_to() {
        let sr = 48_000u32;
        let mut fx = MultiTapDelay::new(sr);
        // 100 ms base, taps at 1/4, 1/2, 3/4 and 1 of it, dead centre.
        fx.set_param(0, (100.0 - 20.0) / (MAX_MS - 20.0));
        fx.set_param(13, 0.0); // no feedback
        fx.set_param(14, 1.0); // fully wet, so only the taps are heard
        for i in 0..TAPS {
            fx.set_param(1 + i * 3 + 2, 0.5); // centre
        }

        let mut buf = vec![0.0f32; sr as usize / 2 * 2];
        buf[0] = 1.0;
        buf[1] = 1.0;
        fx.process_block(&mut buf, sr);

        let mono: Vec<f32> = buf.chunks_exact(2).map(|f| f[0]).collect();
        for (n, ms) in [25.0, 50.0, 75.0, 100.0].iter().enumerate() {
            let at = (ms * 0.001 * sr as f32) as usize;
            let window = &mono[at - 4..at + 4];
            let peak = window.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            assert!(
                peak > 0.1,
                "tap {} did not arrive at {ms} ms (peak {peak})",
                n + 1
            );
        }
        // …and nothing in between: a four-tap delay that smears is a reverb.
        let quiet = (0.16 * sr as f32) as usize;
        assert!(
            mono[quiet..quiet + 100].iter().all(|s| s.abs() < 0.01),
            "the line is still ringing where nothing was asked for"
        );
    }
}
