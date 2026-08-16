//! Beat repeat: catch a slice of what is playing and loop it, on the grid.
//!
//! The grid is choz's own transport ([`choz_ports::transport`]), the same clock
//! the arpeggiator and every synced plugin read, so a repeat lands with the bar
//! rather than near it. **A stopped transport is a pass-through**: its position
//! does not creep, so there is no grid, and inventing one would put the effect
//! somewhere the rest of the rack is not.
//!
//! # How it behaves
//!
//! Every `Interval` quarters it rolls a die against `Chance`. If it wins it
//! captures the next `Grain` quarters *while passing them through* — so the
//! capture itself is never heard as a gap — and then loops that grain until the
//! interval runs out, each repetition `Decay` quieter than the last.
//!
//! # Real-time
//!
//! One buffer, allocated at construction. No allocation, no locks. The die is a
//! fixed-seed xorshift, so a session repeats itself and a test can exist.

use choz_ports::transport;

/// How much audio can be captured: 2 s at 192 kHz, 8 s at 48 kHz.
const CAPACITY_FRAMES: usize = 384_000;

/// Loop crossfade, in frames at 48 kHz. Scaled with the sample rate.
const FADE_48K: usize = 128;

/// The intervals the knob steps through, in quarter notes.
pub const INTERVALS: [(f32, &str); 5] = [
    (0.5, "1/8"),
    (1.0, "1/4"),
    (2.0, "1/2"),
    (4.0, "1 bar"),
    (8.0, "2 bars"),
];

/// The grain lengths, in quarter notes.
pub const GRAINS: [(f32, &str); 6] = [
    (0.0625, "1/64"),
    (0.125, "1/32"),
    (0.25, "1/16"),
    (0.5, "1/8"),
    (1.0, "1/4"),
    (2.0, "1/2"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Nothing to do: the input goes through untouched.
    Idle,
    /// Filling the grain while passing the input through.
    Capturing,
    /// Playing the grain back, over and over.
    Repeating,
}

pub struct BeatRepeat {
    interval_q: f32,
    grain_q: f32,
    chance: f32,
    /// Gain multiplier per repetition: 1 = every repeat as loud as the first.
    decay: f32,
    buf: Vec<f32>,
    grain_len: usize,
    write: usize,
    read: usize,
    reps: u32,
    state: State,
    /// Frames left of the fade back to the dry signal when a repeat ends.
    exiting: usize,
    rng: u32,
    /// Which interval the grid was in last frame. `None` before the first one,
    /// so starting mid-bar does not count as a boundary.
    last_step: Option<i64>,
    mix: f32,
    sample_rate: f32,
}

impl BeatRepeat {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            interval_q: 4.0,
            grain_q: 0.25,
            chance: 1.0,
            decay: 1.0,
            buf: vec![0.0; CAPACITY_FRAMES * 2],
            grain_len: 0,
            write: 0,
            read: 0,
            reps: 0,
            state: State::Idle,
            exiting: 0,
            rng: 0x9E37_79B9,
            last_step: None,
            mix: 1.0,
            sample_rate: sample_rate.max(8000) as f32,
        }
    }

    /// Build from the rack's knob positions: interval, grain, chance, decay.
    pub fn with_params(sample_rate: u32, p: &[f32]) -> Self {
        let get = |i: usize, d: f32| p.get(i).copied().unwrap_or(d);
        let mut b = Self::new(sample_rate);
        b.interval_q = pick(&INTERVALS, get(0, 0.75));
        b.grain_q = pick(&GRAINS, get(1, 0.4));
        b.chance = get(2, 1.0).clamp(0.0, 1.0);
        b.decay = 0.5 + get(3, 1.0).clamp(0.0, 1.0) * 0.5;
        b
    }

    fn fade(&self) -> usize {
        (FADE_48K as f32 * self.sample_rate / 48_000.0) as usize
    }

    #[inline]
    fn roll(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 / 16_777_216.0
    }

    /// The grain, read with the loop crossfade already applied: the last few
    /// milliseconds are mixed into the first few, so going round again is not
    /// a step from wherever the grain ended to wherever it began.
    #[inline]
    fn grain(&self, i: usize, ch: usize) -> f32 {
        let x = self.buf[i * 2 + ch];
        let fade = self.fade();
        if i < fade && self.grain_len > fade * 2 {
            let t = i as f32 / fade as f32;
            let tail = self.buf[(self.grain_len - fade + i) * 2 + ch];
            x * t + tail * (1.0 - t)
        } else {
            x
        }
    }

    /// What it is doing right now, for a test or a meter.
    pub fn repeating(&self) -> bool {
        self.state == State::Repeating
    }
}

/// Nearest entry of a named list, from a 0..1 knob.
fn pick(list: &[(f32, &str)], v: f32) -> f32 {
    let i = (v.clamp(0.0, 1.0) * (list.len() - 1) as f32).round() as usize;
    list[i.min(list.len() - 1)].0
}

/// The knob position an entry sits at, for `params()`.
fn norm_of(list: &[(f32, &str)], value: f32) -> f32 {
    let i = list
        .iter()
        .position(|(v, _)| (*v - value).abs() < 1e-6)
        .unwrap_or(0);
    i as f32 / (list.len() - 1) as f32
}

impl super::FxProcessor for BeatRepeat {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        let sr = sample_rate.max(8000) as f32;
        if (sr - self.sample_rate).abs() > 0.5 {
            self.sample_rate = sr;
            self.state = State::Idle;
            self.last_step = None;
        }
        let t = transport();
        // A stopped transport has no grid. Pass through, and forget where the
        // grid was: coming back in should not fire a repeat on the first frame.
        if !t.playing() {
            self.state = State::Idle;
            self.last_step = None;
            return;
        }
        // Counted from the block's own start in f64, not accumulated in f32:
        // adding 4e-5 to a growing f32 forty-eight thousand times drifts by
        // more than a millisecond, and the drift lands as an early boundary.
        let q0 = t.ppq();
        let q_per_frame = (t.bpm() / 60.0 / sr) as f64;
        let want_grain = ((self.grain_q * 60.0 / t.bpm().max(1.0)) * sr) as usize;
        let want_grain = want_grain.clamp(1, CAPACITY_FRAMES);
        let fade = self.fade();
        let mix = self.mix;

        for (i, frame) in buf.chunks_exact_mut(2).enumerate() {
            let dry = [frame[0], frame[1]];
            let q = q0 + i as f64 * q_per_frame;
            let step = (q / self.interval_q.max(1e-3) as f64).floor() as i64;
            if self.last_step != Some(step) {
                let first = self.last_step.is_none();
                self.last_step = Some(step);
                // The very first block starts mid-interval: wait for a real
                // boundary rather than firing wherever the playhead was.
                let win = !first && self.roll() < self.chance;
                // Whatever comes next, coming *out* of a repeat is a jump from
                // wherever the grain was to wherever the signal is now. The
                // fade is owed to the next interval too, not only to silence.
                if self.state == State::Repeating {
                    self.exiting = fade;
                }
                if win {
                    self.grain_len = want_grain;
                    self.write = 0;
                    self.reps = 0;
                    self.state = State::Capturing;
                } else {
                    self.state = State::Idle;
                }
            }

            let mut out = dry;
            match self.state {
                State::Idle => {}
                State::Capturing => {
                    // Heard as it is captured: a beat repeat that goes silent
                    // while it listens is a hole in the bar.
                    self.buf[self.write * 2] = dry[0];
                    self.buf[self.write * 2 + 1] = dry[1];
                    self.write += 1;
                    if self.write >= self.grain_len {
                        self.state = State::Repeating;
                        self.read = 0;
                        self.reps = 1;
                    }
                }
                State::Repeating => {
                    let gain = self.decay.powi(self.reps as i32);
                    out = [
                        self.grain(self.read, 0) * gain,
                        self.grain(self.read, 1) * gain,
                    ];
                    self.read += 1;
                    if self.read >= self.grain_len {
                        self.read = 0;
                        self.reps += 1;
                    }
                }
            }

            // Coming out of a repeat: cross back into the live signal instead
            // of cutting to it.
            if self.exiting > 0 && self.state != State::Repeating {
                let t = 1.0 - self.exiting as f32 / fade.max(1) as f32;
                let tail = [self.grain(self.read, 0), self.grain(self.read, 1)];
                out = [
                    tail[0] * (1.0 - t) + out[0] * t,
                    tail[1] * (1.0 - t) + out[1] * t,
                ];
                self.read = (self.read + 1) % self.grain_len.max(1);
                self.exiting -= 1;
            }

            frame[0] = dry[0] + mix * (out[0] - dry[0]);
            frame[1] = dry[1] + mix * (out[1] - dry[1]);
        }
    }

    fn reset(&mut self) {
        self.buf.fill(0.0);
        self.state = State::Idle;
        self.last_step = None;
        self.write = 0;
        self.read = 0;
        self.reps = 0;
        self.exiting = 0;
    }

    fn set_mix(&mut self, wet: f32) {
        self.mix = wet.clamp(0.0, 1.0);
    }

    fn name(&self) -> &str {
        "BeatRepeat"
    }

    fn params(&self) -> Vec<crate::fx::FxParam> {
        use crate::fx::FxParam;
        vec![
            FxParam::new(
                "Interval",
                norm_of(&INTERVALS, self.interval_q),
                0.0,
                1.0,
                "",
            ),
            FxParam::new("Grain", norm_of(&GRAINS, self.grain_q), 0.0, 1.0, ""),
            FxParam::new("Chance", self.chance, 0.0, 1.0, ""),
            FxParam::new("Decay", (self.decay - 0.5) * 2.0, 0.0, 1.0, ""),
            FxParam::new("Wet", self.mix, 0.0, 1.0, ""),
        ]
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => self.interval_q = pick(&INTERVALS, v),
            1 => self.grain_q = pick(&GRAINS, v),
            2 => self.chance = v,
            3 => self.decay = 0.5 + v * 0.5,
            4 => self.mix = v,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::FxProcessor;

    fn with_transport<R>(bpm: f32, sr: u32, f: impl FnOnce() -> R) -> R {
        let _g = crate::test_locks::transport();
        let t = transport();
        let (old_bpm, old_sr, old_play) = (t.bpm(), t.sample_rate(), t.playing());
        t.set_sample_rate(sr);
        t.set_bpm(bpm);
        t.set_playing(true);
        t.rewind();
        let out = f();
        t.set_playing(old_play);
        t.set_bpm(old_bpm);
        t.set_sample_rate(old_sr);
        out
    }

    /// A ramp, so a repeated slice is obvious: the output going *backwards* is
    /// the grain starting again.
    fn ramp(frames: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let s = i as f32 / frames as f32;
                [s, s]
            })
            .collect()
    }

    #[test]
    fn a_stopped_transport_is_a_pass_through() {
        let _g = crate::test_locks::transport();
        let t = transport();
        let was = t.playing();
        t.set_playing(false);
        let mut br = BeatRepeat::new(48000);
        let mut buf = ramp(2048);
        let before = buf.clone();
        br.process_block(&mut buf, 48000);
        t.set_playing(was);
        assert_eq!(buf, before, "no grid, no repeat");
    }

    /// On the grid, a slice comes back: the ramp stops rising and repeats a
    /// stretch it already played.
    #[test]
    fn it_captures_a_grain_and_loops_it() {
        with_transport(120.0, 48000, || {
            // 120 bpm: a quarter is 24000 frames. Interval 1/4, grain 1/16
            // (6000 frames) → capture, then three repeats inside the beat.
            let mut br = BeatRepeat::new(48000);
            br.interval_q = 1.0;
            br.grain_q = 0.25;
            br.chance = 1.0;
            br.set_mix(1.0);

            // Two beats of a rising ramp, in one block.
            let frames = 48000;
            let mut buf = ramp(frames);
            br.process_block(&mut buf, 48000);
            let l: Vec<f32> = buf.iter().step_by(2).copied().collect();

            // The first beat is untouched (no boundary has been crossed yet).
            for (i, out) in l.iter().enumerate().take(23000) {
                assert!(
                    (out - i as f32 / frames as f32).abs() < 1e-5,
                    "the first interval should pass through, at {i}"
                );
            }
            // Past the boundary the output stops climbing: it is stuck inside
            // the slice it captured (0.5…0.625) while the dry ramp walks on to
            // 1.0. That is the whole effect, and a rising ramp cannot fake it.
            let after = &l[30500..48000];
            let hi = after.iter().cloned().fold(f32::MIN, f32::max);
            let lo = after.iter().cloned().fold(f32::MAX, f32::min);
            assert!(
                hi < 0.64 && lo > 0.48,
                "the output should be stuck in the captured slice, saw {lo}..{hi}"
            );
            assert!(br.repeating(), "and it should still be in the repeat");
        });
    }

    /// Chance 0 never fires: the effect is a wire until it is asked for.
    #[test]
    fn chance_zero_never_repeats() {
        with_transport(120.0, 48000, || {
            let mut br = BeatRepeat::new(48000);
            br.interval_q = 0.5;
            br.chance = 0.0;
            br.set_mix(1.0);
            let mut buf = ramp(48000);
            let before = buf.clone();
            br.process_block(&mut buf, 48000);
            assert_eq!(buf, before);
        });
    }

    /// The loop point is crossfaded, so going round again is not a step. A
    /// ramp is the worst case: its ends are as far apart as the signal gets.
    #[test]
    fn the_loop_does_not_click() {
        with_transport(120.0, 48000, || {
            let mut br = BeatRepeat::new(48000);
            br.interval_q = 1.0;
            br.grain_q = 0.25;
            br.chance = 1.0;
            br.set_mix(1.0);
            let mut buf = ramp(48000);
            br.process_block(&mut buf, 48000);
            let l: Vec<f32> = buf.iter().step_by(2).copied().collect();
            // The grain spans 6000 frames of a 48000-frame ramp, so its ends
            // are 0.125 apart. Crossfaded, no single frame may jump that far.
            let biggest = l[24000..]
                .windows(2)
                .fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()));
            assert!(biggest < 0.02, "the loop clicked by {biggest}");
        });
    }

    /// Decay makes each repetition quieter, and the first one is not touched.
    #[test]
    fn decay_turns_the_repeats_down() {
        with_transport(120.0, 48000, || {
            let mut br = BeatRepeat::new(48000);
            br.interval_q = 1.0;
            br.grain_q = 0.25;
            br.chance = 1.0;
            br.decay = 0.5;
            br.set_mix(1.0);
            let mut buf = vec![0.0f32; 48000 * 2];
            for (i, f) in buf.chunks_exact_mut(2).enumerate() {
                let s = (2.0 * std::f32::consts::PI * 200.0 * i as f32 / 48000.0).sin();
                f[0] = s;
                f[1] = s;
            }
            br.process_block(&mut buf, 48000);
            let peak = |r: std::ops::Range<usize>| {
                buf[r.start * 2..r.end * 2]
                    .iter()
                    .fold(0.0f32, |m, s| m.max(s.abs()))
            };
            let first = peak(31000..35000);
            let later = peak(43000..47000);
            assert!(
                later < first * 0.6,
                "the repeats should fade: {first} then {later}"
            );
        });
    }

    #[test]
    fn it_survives_silence_empty_blocks_and_a_rate_change() {
        with_transport(140.0, 48000, || {
            let mut br = BeatRepeat::with_params(48000, &[0.5, 0.5, 1.0, 0.8]);
            let mut buf = vec![0.0f32; 4096];
            br.process_block(&mut buf, 48000);
            assert!(buf.iter().all(|s| *s == 0.0));
            let mut hot = vec![4.0f32; 4096];
            br.process_block(&mut hot, 96000);
            assert!(hot.iter().all(|s| s.is_finite()));
            br.process_block(&mut [], 96000);
            br.process_block(&mut [1.0], 96000);
            br.reset();
        });
    }
}
