//! Moving the pitch without moving the time.
//!
//! ## The method, and why this one
//!
//! This is the shape Fons Adriaensen's **zita-at1** uses (and x42's `fat1.lv2`
//! after it), because it is the one that behaves on a voice:
//!
//! * One **read pointer** walks a delay line at `ratio` samples per output
//!   sample. That is the pitch shift, and it is exact.
//! * Walking at a rate other than 1 makes the pointer drift towards — or away
//!   from — the writer. When it leaves its window it **jumps by a whole number
//!   of pitch periods**, which lands it on the same phase of the waveform.
//! * The jump is a **crossfade** between the old position and the new one, over
//!   a raised cosine. Nothing is discontinuous, so nothing clicks.
//!
//! Reading at `ratio` moves the whole spectrum, which is why a resampler alone
//! would shorten the sound; here the *time* is put back by the periodic jumps,
//! and only the pitch is left moved.
//!
//! ```text
//! write ─────────────────────────────────────►  now
//!                    r1 ──►                     reads at `ratio`
//!        r2 ──►                                 …and during a jump, both,
//!        └── crossfade ──┘                      blended by a raised cosine
//! ```
//!
//! ### Why it replaced the overlap-add
//!
//! The first version was PSOLA — grains windowed and summed. It worked on a
//! sine and misbehaved on a voice, and the three ways it did all come from one
//! place: **a sum of windowed copies has a gain**, and that gain depends on the
//! grain spacing, the window length and how well consecutive grains line up.
//! Get any of them slightly wrong on a signal whose period is moving, and the
//! output is both dirty and *louder than the input* — which is heard as the
//! effect clipping. Two readers crossfaded cannot do that: the output is a
//! convex combination of two samples of the input, so `|out| ≤ max |in|`,
//! always, whatever the ratio and whatever the pitch does.
//!
//! ### Formants
//!
//! They move with the pitch — this is a resampler, so a shift of `r` moves the
//! spectral envelope by `r` too. At the ratios a *corrector* lives at (a
//! semitone is 6 %) that is inaudible, and it is what zita-at1 does. A
//! formant-preserving path would be a different implementation of
//! [`PitchShifter`], which is what that trait is for — there is no switch for
//! it today, because a switch that does nothing is worse than no switch.

use super::detector::{MAX_PERIOD, MIN_SUPPORTED_HZ};

/// One way of moving pitch without moving time.
///
/// A trait with one implementation today, and that is deliberate: the pitch
/// shifter is the piece most likely to be replaced, and everything around it is
/// written against this shape rather than against the method's internals. It
/// has already been swapped once.
pub trait PitchShifter {
    fn reset(&mut self);
    /// Samples the output runs behind the input.
    fn latency_samples(&self) -> usize;
    /// `input` and `output` are the same length; `pitch_ratio` above 1 raises.
    /// `period` is the detected period in samples, or 0 when unvoiced — with no
    /// period there is nothing to jump by, so the input passes through.
    fn process(&mut self, input: &[f32], output: &mut [f32], pitch_ratio: f32, period: f32);
}

pub struct RetuneShifter {
    /// Circular input; four of the longest period, so a jump always has room.
    buf: Vec<f32>,
    /// Absolute count of samples written — the input timeline.
    written: u64,
    /// Read position, absolute and fractional.
    r1: f64,
    /// The position being crossfaded *to*.
    r2: f64,
    xfade: usize,
    xflen: usize,
    /// Period the jumps are cut on, smoothed. The detector's answer moves a
    /// fraction of a sample between analyses, and a jump that is not a whole
    /// period lands on the wrong part of the wave.
    period: f64,
    latency: usize,
}

/// Two periods of the lowest note, at this rate, and never more than the buffer
/// holds. Fixed for the rate rather than for the note being sung: a latency
/// that moved with the pitch would be a time machine.
fn latency_for(sample_rate: f32) -> usize {
    let p = (sample_rate.max(1.0) / MIN_SUPPORTED_HZ).ceil() as usize;
    (2 * p).clamp(64, 2 * MAX_PERIOD)
}

impl RetuneShifter {
    /// The buffer is sized here, for the longest period at the highest
    /// supported rate, and never resized: `process` runs on the audio thread.
    pub fn new(sample_rate: f32) -> Self {
        Self {
            buf: vec![0.0; 4 * MAX_PERIOD],
            written: 0,
            r1: 0.0,
            r2: 0.0,
            xfade: 0,
            xflen: 256,
            period: 0.0,
            latency: latency_for(sample_rate),
        }
    }

    /// The delay changes with the rate and with nothing else.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.latency = latency_for(sample_rate);
    }

    /// Cubic interpolation, because a fractional read at a rate near 1 is where
    /// a linear one leaves its whistle.
    fn read(&self, pos: f64) -> f32 {
        let n = self.buf.len();
        if pos < 1.0 || pos + 2.0 >= self.written as f64 {
            return 0.0;
        }
        let i = pos.floor() as usize;
        let f = (pos - i as f64) as f32;
        let s = |k: usize| self.buf[k % n];
        let (y0, y1, y2, y3) = (s(i - 1), s(i), s(i + 1), s(i + 2));
        // Catmull-Rom.
        let a0 = -0.5 * y0 + 1.5 * y1 - 1.5 * y2 + 0.5 * y3;
        let a1 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
        let a2 = -0.5 * y0 + 0.5 * y2;
        ((a0 * f + a1) * f + a2) * f + y1
    }
}

impl Default for RetuneShifter {
    fn default() -> Self {
        Self::new(48_000.0)
    }
}

impl PitchShifter for RetuneShifter {
    fn reset(&mut self) {
        self.buf.fill(0.0);
        self.written = 0;
        self.r1 = 0.0;
        self.r2 = 0.0;
        self.xfade = 0;
        self.period = 0.0;
    }

    fn latency_samples(&self) -> usize {
        self.latency
    }

    fn process(&mut self, input: &[f32], output: &mut [f32], pitch_ratio: f32, period: f32) {
        let voiced = period.is_finite() && period >= 2.0 && (period as usize) < MAX_PERIOD;
        // Unvoiced: no period to jump by, so no shift either. Reading at 1 is a
        // plain delay, which is what should come out of a consonant.
        let ratio = match pitch_ratio {
            r if voiced && r.is_finite() => r.clamp(0.5, 2.0) as f64,
            _ => 1.0,
        };
        if voiced {
            self.period = if self.period > 1.0 {
                self.period * 0.9 + period as f64 * 0.1
            } else {
                period as f64
            };
        }
        let p = self.period.max(2.0);
        // A crossfade of about a period: long enough to hide the seam, short
        // enough that two jumps never overlap.
        self.xflen = (p as usize).clamp(64, 1024);
        let nominal = self.latency as f64;
        // How far the reader may drift before it is pulled back.
        let slack = p;

        for (n, &x) in input.iter().enumerate() {
            let len = self.buf.len();
            self.buf[(self.written as usize) % len] = if x.is_finite() { x } else { 0.0 };
            self.written += 1;
            let now = self.written as f64;

            // Until the delay line holds a whole latency there is nothing to
            // read: that wait *is* the latency. Parking the reader early would
            // leave it level with the writer, reading samples that have not
            // been written — which is silence, forever.
            if self.written <= self.latency as u64 + 2 {
                output[n] = 0.0;
                continue;
            }
            if self.r1 < 1.0 {
                self.r1 = now - nominal;
                self.r2 = self.r1;
            }

            let mut y = self.read(self.r1);
            self.r1 += ratio;

            if self.xfade > 0 {
                let u2 = self.read(self.r2);
                self.r2 += ratio;
                // Raised cosine: starts and ends flat, so neither end of the
                // jump is a corner.
                let done = (self.xflen - self.xfade) as f32 / self.xflen as f32;
                let v = 0.5 - 0.5 * (std::f32::consts::PI * done).cos();
                y = y * (1.0 - v) + u2 * v;
                self.xfade -= 1;
                if self.xfade == 0 {
                    self.r1 = self.r2;
                }
            } else if voiced {
                // The reader drifts because it is walking at a rate other than
                // one. Pull it back by **whole periods**, which lands it on the
                // same phase of the wave, and crossfade the jump.
                let delay = now - self.r1;
                if delay < nominal - slack || delay > nominal + slack {
                    let jumps = ((nominal - delay) / p).round();
                    if jumps.abs() >= 1.0 {
                        let target = self.r1 - jumps * p;
                        // Never read the future, and never read past the end of
                        // what the buffer still holds.
                        let oldest = now - (len as f64 - 4.0);
                        if target > oldest && target + 2.0 < now {
                            self.r2 = target;
                            self.xfade = self.xflen;
                        }
                    }
                }
            }

            if !y.is_finite() || y.abs() < 1e-20 {
                y = 0.0;
            }
            output[n] = y;
        }
    }
}
