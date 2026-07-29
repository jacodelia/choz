//! Bitcrusher — bit-depth reduction + sample-rate decimation.
//!
//! Reduces perceived bit depth by quantising to `bits` levels, and simulates
//! lower sample rates by holding each sample for `hold` output frames.

use super::FxProcessor;

/// Bitcrusher FX — combines bit reduction with sample-rate decimation.
pub struct Bitcrusher {
    /// Effective bit depth (1–16).
    bits: u8,
    /// Sample-rate reduction factor (1 = off, N = hold each sample N frames).
    hold: u32,
    hold_counter: u32,
    held_l: f32,
    held_r: f32,
    wet: f32,
}

impl Bitcrusher {
    pub fn new() -> Self {
        Self {
            bits: 8,
            hold: 1,
            hold_counter: 0,
            held_l: 0.0,
            held_r: 0.0,
            wet: 1.0,
        }
    }

    /// Set bit depth (clamped 1–16).
    pub fn set_bits(&mut self, bits: u8) {
        self.bits = bits.clamp(1, 16);
    }

    /// Set sample-hold factor (1 = off, 2 = half rate, etc.).
    pub fn set_hold(&mut self, hold: u32) {
        self.hold = hold.max(1);
    }

    #[inline]
    fn crush(&self, s: f32) -> f32 {
        // Quantise to 2^bits steps in [-1, 1].
        let levels = (1u32 << self.bits) as f32;
        let half   = levels * 0.5;
        ((s * half).round() / half).clamp(-1.0, 1.0)
    }
}

impl Default for Bitcrusher {
    fn default() -> Self { Self::new() }
}

impl FxProcessor for Bitcrusher {
    fn process_block(&mut self, buf: &mut [f32], _sample_rate: u32) {
        let frames = buf.len() / 2;
        for i in 0..frames {
            if self.hold_counter == 0 {
                self.held_l = self.crush(buf[i * 2]);
                self.held_r = self.crush(buf[i * 2 + 1]);
            }
            let dry_l = buf[i * 2];
            let dry_r = buf[i * 2 + 1];
            buf[i * 2]     = dry_l + self.wet * (self.held_l - dry_l);
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
