//! Bitcrusher — bit-depth reduction + sample-rate decimation.
//!
//! Reduces perceived bit depth by quantising to `bits` levels, and simulates a
//! lower sample rate by holding each sample.
//!
//! # The decimation is a frequency, not a count
//!
//! It used to be a count: `hold` output frames per sample. That makes the whole
//! effect scale with the device — `hold = 8` decimates to 6 kHz at 48 kHz and
//! to 24 kHz at 192 kHz, so on a fast interface the crusher quietly stops
//! crushing. The knob now asks for a **rate in hertz** and the hold length is
//! worked out from whatever sample rate the block arrives at, which is what
//! makes a saved patch sound the same everywhere.
//!
//! The quantiser is deliberately *not* anti-aliased. Aliasing is not a defect
//! here, it is the effect: a bit crusher that resolved its own images would be
//! a low-pass filter with extra steps.

use super::FxProcessor;

/// Bitcrusher FX — combines bit reduction with sample-rate decimation.
pub struct Bitcrusher {
    /// Effective bit depth (1–16).
    bits: u8,
    /// What the crusher decimates to, in hertz. Above the device's own rate it
    /// is off, which is what the top of the knob means.
    rate_hz: f32,
    /// The hold length that rate works out to at the rate blocks are arriving
    /// at. Recomputed when either changes; never inside the loop.
    hold: u32,
    hold_counter: u32,
    held_l: f32,
    held_r: f32,
    wet: f32,
    sample_rate: f32,
}

impl Bitcrusher {
    pub fn new() -> Self {
        Self {
            bits: 8,
            rate_hz: f32::INFINITY,
            hold: 1,
            hold_counter: 0,
            held_l: 0.0,
            held_r: 0.0,
            wet: 1.0,
            sample_rate: 48_000.0,
        }
    }

    /// Set bit depth (clamped 1–16).
    pub fn set_bits(&mut self, bits: u8) {
        self.bits = bits.clamp(1, 16);
    }

    /// Set the sample-hold factor at the *current* sample rate.
    ///
    /// Kept because it is the public API and because "hold two frames" is how
    /// anybody thinks about a crusher — it is stored as the frequency that
    /// works out to, so the sound survives a change of device.
    pub fn set_hold(&mut self, hold: u32) {
        self.rate_hz = self.sample_rate / hold.max(1) as f32;
        self.refresh();
    }

    /// What the crusher decimates to, in hertz.
    pub fn set_rate_hz(&mut self, hz: f32) {
        self.rate_hz = hz.max(20.0);
        self.refresh();
    }

    fn refresh(&mut self) {
        let want = (self.sample_rate / self.rate_hz.max(1.0)).round();
        self.hold = (want as u32).clamp(1, 4096);
    }

    #[inline]
    fn crush(&self, s: f32) -> f32 {
        // Quantise to 2^bits steps in [-1, 1].
        let levels = (1u32 << self.bits) as f32;
        let half = levels * 0.5;
        ((s * half).round() / half).clamp(-1.0, 1.0)
    }
}

impl Default for Bitcrusher {
    fn default() -> Self {
        Self::new()
    }
}

impl FxProcessor for Bitcrusher {
    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => self.set_bits((1.0 + v * 15.0) as u8),
            // The knob still reads 1..16 "frames", which is what the rack
            // labels it — resolved against 48 kHz so the number means the same
            // hertz on every device, which is the whole point.
            1 => {
                let frames = (1.0 + v * 15.0).max(1.0);
                self.set_rate_hz(48_000.0 / frames);
            }
            _ => {}
        }
    }

    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        let sr = sample_rate.max(8000) as f32;
        if sr != self.sample_rate {
            self.sample_rate = sr;
            self.refresh();
        }
        let frames = buf.len() / 2;
        for i in 0..frames {
            if self.hold_counter == 0 {
                self.held_l = self.crush(buf[i * 2]);
                self.held_r = self.crush(buf[i * 2 + 1]);
            }
            let dry_l = buf[i * 2];
            let dry_r = buf[i * 2 + 1];
            buf[i * 2] = dry_l + self.wet * (self.held_l - dry_l);
            buf[i * 2 + 1] = dry_r + self.wet * (self.held_r - dry_r);
            self.hold_counter += 1;
            if self.hold_counter >= self.hold {
                self.hold_counter = 0;
            }
        }
    }

    fn reset(&mut self) {
        self.hold_counter = 0;
        self.held_l = 0.0;
        self.held_r = 0.0;
    }

    fn set_mix(&mut self, wet: f32) {
        self.wet = wet.clamp(0.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The bug this replaced.** `hold` was a count of frames, so the
    /// frequency it decimated to scaled with the device: eight frames is 6 kHz
    /// at 48 kHz and 24 kHz at 192, and at 24 kHz there is nothing left to
    /// hear. The knob asks for hertz now.
    ///
    /// Measured as the period of the staircase: how many *seconds* each held
    /// value lasts has to be the same at every rate.
    #[test]
    fn the_crush_rate_is_the_same_hz_at_every_sample_rate() {
        let period_ms = |sr: u32| {
            let mut fx = Bitcrusher::new();
            fx.set_bits(16);
            fx.set_rate_hz(4_000.0);
            let n = sr as usize / 10;
            let mut buf: Vec<f32> = (0..n)
                .flat_map(|i| {
                    let v = (std::f32::consts::TAU * 100.0 * i as f32 / sr as f32).sin() * 0.5;
                    [v, v]
                })
                .collect();
            fx.process_block(&mut buf, sr);
            // Count how many times the held value actually changes.
            let l: Vec<f32> = buf.chunks(2).map(|c| c[0]).collect();
            let steps = l.windows(2).filter(|w| w[0] != w[1]).count().max(1);
            (n as f32 / steps as f32) / sr as f32 * 1000.0
        };
        let a = period_ms(48_000);
        let b = period_ms(192_000);
        assert!(
            (a - 0.25).abs() < 0.05 && (b - 0.25).abs() < 0.05,
            "4 kHz is a step every 0.25 ms: {a:.3} ms at 48k, {b:.3} ms at 192k"
        );
    }

    /// The old `set_hold` still means what it always meant at the rate it is
    /// called at — it is public API and projects were written against it.
    #[test]
    fn set_hold_still_holds_that_many_frames() {
        let mut fx = Bitcrusher::new();
        fx.process_block(&mut [0.0; 2], 48_000);
        fx.set_hold(4);
        let mut buf: Vec<f32> = (0..64).flat_map(|i| [i as f32 * 0.01, 0.0]).collect();
        fx.process_block(&mut buf, 48_000);
        let l: Vec<f32> = buf.chunks(2).map(|c| c[0]).collect();
        let changes = l.windows(2).filter(|w| w[0] != w[1]).count();
        assert!(
            (14..=17).contains(&changes),
            "64 frames held four at a time is ~16 changes, got {changes}"
        );
    }

    #[test]
    fn crushes_to_1bit_extremes() {
        let mut fx = Bitcrusher::new();
        fx.set_bits(1);
        let mut buf = [0.9f32, -0.9, 0.1, -0.1];
        fx.process_block(&mut buf, 48000);
        // 1-bit crush: only +1.0 and -1.0 possible
        assert!(buf[0] == 1.0 || buf[0] == -1.0);
    }

    #[test]
    fn hold_2_repeats_sample() {
        let mut fx = Bitcrusher::new();
        fx.set_bits(16); // no bit reduction
        fx.set_hold(2);
        let mut buf = [1.0f32, 1.0, 0.0, 0.0];
        fx.process_block(&mut buf, 48000);
        // Frame 0: sample is held; frame 1: still held (same)
        assert_eq!(buf[0], buf[2]);
    }
}
