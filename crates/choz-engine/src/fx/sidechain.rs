//! Sidechain pump / ducking FX.
//!
//! Applies a rhythmic volume duck to the signal — the classic "pumping" feel
//! used in dance music. Two trigger modes:
//!
//! - **LFO mode** (default): an internal LFO fires at `rate_hz` and shapes a duck
//!   envelope. Set `rate_hz > 0` to enable.
//! - **Triggered mode**: call `trigger()` from the non-RT thread (e.g. from the MIDI
//!   scheduler on every kick drum hit) to fire the duck manually.
//!
//! The duck envelope is:
//!   - Instant drop to `1.0 - depth` on trigger
//!   - Exponential recovery back to 1.0 with time constant `release_secs`
//!   - A short initial hold of `hold_secs` before recovery starts

use super::FxProcessor;

/// Sidechain pump / duck effect.
pub struct SidechainDuck {
    /// Duck depth (0.0 = no effect, 1.0 = full mute on trigger). Default: 0.8.
    depth: f32,
    /// Release time constant in seconds. Default: 0.15.
    release_secs: f32,
    /// Hold time (seconds before recovery starts). Default: 0.0.
    hold_secs: f32,
    /// LFO rate in Hz (0.0 = triggered mode only). Default: 2.0 Hz.
    rate_hz: f32,
    /// Dry/wet mix.
    wet: f32,

    // Internal state
    /// Current envelope value (1.0 = no duck, 0.0 = fully ducked).
    env: f32,
    /// LFO phase (0.0–1.0).
    lfo_phase: f32,
    /// Remaining hold frames.
    hold_frames: usize,
    /// Whether a trigger arrived (from non-RT `trigger()` call).
    triggered: std::sync::atomic::AtomicBool,

    sample_rate: u32,
}

impl SidechainDuck {
    pub fn new() -> Self {
        Self {
            depth:        0.8,
            release_secs: 0.15,
            hold_secs:    0.0,
            rate_hz:      2.0,
            wet:          1.0,
            env:          1.0,
            lfo_phase:    0.0,
            hold_frames:  0,
            triggered:    std::sync::atomic::AtomicBool::new(false),
            sample_rate:  48000,
        }
    }

    pub fn set_depth(&mut self, d: f32) { self.depth = d.clamp(0.0, 1.0); }
    pub fn set_release(&mut self, secs: f32) { self.release_secs = secs.clamp(0.001, 4.0); }
    pub fn set_hold(&mut self, secs: f32) { self.hold_secs = secs.clamp(0.0, 1.0); }
    pub fn set_rate(&mut self, hz: f32) { self.rate_hz = hz.clamp(0.0, 20.0); }

    /// Fire an external trigger (call from non-RT thread, e.g. MIDI scheduler).
    /// Thread-safe: uses atomic store.
    pub fn trigger(&self) {
        self.triggered.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    #[inline]
    fn release_coef(&self) -> f32 {
        (-1.0 / (self.release_secs * self.sample_rate as f32)).exp()
    }
}

impl Default for SidechainDuck {
    fn default() -> Self { Self::new() }
}

impl FxProcessor for SidechainDuck {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        self.sample_rate = sample_rate;
        let sr = sample_rate as f32;
        let rel_coef = self.release_coef();
        let frames   = buf.len() / 2;

        // Check external trigger (atomic).
        let ext_trigger = self.triggered.swap(false, std::sync::atomic::Ordering::Relaxed);

        for i in 0..frames {
            // LFO-based trigger: fire when LFO phase crosses 0.0 (start of each cycle).
            let lfo_trigger = if self.rate_hz > 0.0 {
                let prev = self.lfo_phase;
                self.lfo_phase = (self.lfo_phase + self.rate_hz / sr).fract();
                prev > self.lfo_phase  // wrapped around → new cycle started
            } else {
                false
            };

            // External trigger only fires on first frame of the block.
            let fire = lfo_trigger || (ext_trigger && i == 0);
            if fire {
                self.env = 1.0 - self.depth;
                self.hold_frames = (self.hold_secs * sr) as usize;
            }

            // Hold then release.
            if self.hold_frames > 0 {
                self.hold_frames -= 1;
            } else {
                self.env = 1.0 - rel_coef * (1.0 - self.env);
                if self.env > 0.9999 { self.env = 1.0; }
            }

            let gain = 1.0 - self.wet * self.depth * (1.0 - self.env);
            buf[i * 2]     *= gain;
            buf[i * 2 + 1] *= gain;
        }
    }

    fn reset(&mut self) {
        self.env        = 1.0;
        self.lfo_phase  = 0.0;
        self.hold_frames = 0;
        self.triggered.store(false, std::sync::atomic::Ordering::Relaxed);
    }

    fn set_mix(&mut self, wet: f32) { self.wet = wet.clamp(0.0, 1.0); }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duck_reduces_level_on_trigger() {
        let sr = 48000u32;
        let mut fx = SidechainDuck::new();
        fx.set_rate(0.0); // disable LFO
        fx.set_depth(1.0);
        fx.set_release(0.5);

        // Fill with constant 1.0 signal
        let mut buf = vec![1.0f32; 256];
        // Fire trigger before processing
        fx.trigger();
        fx.process_block(&mut buf, sr);

        // First samples should be near 0.0 (fully ducked)
        assert!(buf[0] < 0.2, "immediately after trigger: sample={}", buf[0]);
        // Later samples should recover
        assert!(buf[250] > buf[0], "signal should recover after duck");
    }

    #[test]
    fn lfo_mode_produces_periodic_dips() {
        let sr = 48000u32;
        let mut fx = SidechainDuck::new();
        fx.set_rate(4.0);  // 4 Hz → duck every 12000 frames
        fx.set_depth(0.9);
        fx.set_release(0.1);

        let mut buf = vec![1.0f32; 96000]; // 2 seconds
        fx.process_block(&mut buf, sr);

        // Find minimum value — should be well below 1.0 due to LFO dips
        let min = buf.iter().copied().fold(1.0f32, f32::min);
        assert!(min < 0.5, "LFO should create dips: min={min}");
    }

    #[test]
    fn no_effect_with_zero_depth() {
        let sr = 48000u32;
        let mut fx = SidechainDuck::new();
        fx.set_depth(0.0);
        fx.trigger();
        let input: Vec<f32> = vec![0.5f32; 64];
        let mut buf = input.clone();
        fx.process_block(&mut buf, sr);
        for (a, b) in input.iter().zip(buf.iter()) {
            assert!((a - b).abs() < 1e-6, "zero depth should pass through unchanged");
        }
    }
}
