//! Noise gate: envelope follower, hysteresis and an explicit state machine.
//!
//! The gate opens at `threshold_db` and closes `hysteresis_db` *below* it.
//! With one threshold a signal sitting on it chatters — open, closed, open —
//! several times a second; the two thresholds are what stop that.

/// Where the gate is right now. Named, because "is_open plus a counter" was
/// two booleans that could disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateState {
    /// Below the close threshold, sitting at the floor.
    Closed,
    /// Opening: the gain is ramping up to 1.
    Attack,
    /// Open, above the close threshold.
    Open,
    /// Below the close threshold, but the hold timer has not run out.
    Hold,
    /// Hold spent: the gain is ramping down to the floor.
    Release,
}

pub struct Gate {
    pub threshold_db: f32,
    pub attack_ms: f32,
    pub hold_ms: f32,
    pub release_ms: f32,
    /// How far below the open threshold the gate closes. 0 = one threshold.
    pub hysteresis_db: f32,
    /// Floor in dB when gate is fully closed (0 = silence, -80 = very quiet).
    pub floor_db: f32,
    mix: f32,
    // RT state
    envelope: f32,
    gain: f32,
    hold_counter: usize,
    state: GateState,
}

impl Gate {
    pub fn new() -> Self {
        Self {
            threshold_db: -40.0,
            attack_ms: 1.0,
            hold_ms: 50.0,
            release_ms: 200.0,
            hysteresis_db: 6.0,
            floor_db: -80.0,
            mix: 1.0,
            envelope: 0.0,
            gain: 0.0,
            hold_counter: 0,
            state: GateState::Closed,
        }
    }

    /// What the gate is doing, for a meter or a test.
    pub fn state(&self) -> GateState {
        self.state
    }
}

impl Default for Gate {
    fn default() -> Self {
        Self::new()
    }
}

impl super::FxProcessor for Gate {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        if buf.len() < 2 {
            return;
        }
        let sr = sample_rate as f32;
        let attack_coeff = (-1.0 / (self.attack_ms * 0.001 * sr)).exp();
        let release_coeff = (-1.0 / (self.release_ms * 0.001 * sr)).exp();
        let hold_samples = (self.hold_ms * 0.001 * sr) as usize;
        let open_linear = 10.0f32.powf(self.threshold_db / 20.0);
        let close_linear = 10.0f32.powf((self.threshold_db - self.hysteresis_db.max(0.0)) / 20.0);
        let floor_linear = 10.0f32.powf(self.floor_db / 20.0);
        let target_open = 1.0f32;
        let target_closed = floor_linear;

        let frames = buf.len() / 2;
        for i in 0..frames {
            let l = buf[i * 2];
            let r = buf[i * 2 + 1];

            // Fast peak envelope
            let peak = l.abs().max(r.abs());
            if peak > self.envelope {
                self.envelope = peak;
            } else {
                self.envelope *= release_coeff;
            }

            // ── State machine ─────────────────────────────────────────────
            // Opening looks at the open threshold, closing at the lower one.
            self.state = match self.state {
                GateState::Closed | GateState::Release | GateState::Attack
                    if self.envelope >= open_linear =>
                {
                    self.hold_counter = hold_samples;
                    if self.gain >= 0.999 {
                        GateState::Open
                    } else {
                        GateState::Attack
                    }
                }
                GateState::Attack => GateState::Attack,
                GateState::Open | GateState::Hold if self.envelope >= close_linear => {
                    self.hold_counter = hold_samples;
                    GateState::Open
                }
                GateState::Open => GateState::Hold,
                GateState::Hold => {
                    self.hold_counter = self.hold_counter.saturating_sub(1);
                    if self.hold_counter == 0 {
                        GateState::Release
                    } else {
                        GateState::Hold
                    }
                }
                GateState::Release if self.gain <= target_closed * 1.001 => GateState::Closed,
                other => other,
            };
            if self.state == GateState::Attack && self.gain >= 0.999 {
                self.state = GateState::Open;
            }

            let target = match self.state {
                GateState::Attack | GateState::Open | GateState::Hold => target_open,
                GateState::Release | GateState::Closed => target_closed,
            };
            if target > self.gain {
                self.gain = attack_coeff * (self.gain - target) + target;
            } else {
                self.gain = release_coeff * (self.gain - target) + target;
            }

            let wet_l = l * self.gain;
            let wet_r = r * self.gain;
            buf[i * 2] = l + self.mix * (wet_l - l);
            buf[i * 2 + 1] = r + self.mix * (wet_r - r);
        }
    }

    fn reset(&mut self) {
        self.envelope = 0.0;
        self.gain = 0.0;
        self.hold_counter = 0;
        self.state = GateState::Closed;
    }

    fn set_mix(&mut self, wet: f32) {
        self.mix = wet.clamp(0.0, 1.0);
    }
    fn name(&self) -> &str {
        "Gate"
    }

    fn params(&self) -> Vec<crate::fx::FxParam> {
        use crate::fx::FxParam;
        vec![
            FxParam::new(
                "Threshold",
                (self.threshold_db + 80.0) / 80.0,
                -80.0,
                0.0,
                "dB",
            ),
            FxParam::new(
                "Attack",
                (self.attack_ms / 200.0).clamp(0.0, 1.0),
                0.0,
                200.0,
                "ms",
            ),
            FxParam::new(
                "Hold",
                (self.hold_ms / 500.0).clamp(0.0, 1.0),
                0.0,
                500.0,
                "ms",
            ),
            FxParam::new(
                "Release",
                (self.release_ms / 2000.0).clamp(0.0, 1.0),
                0.0,
                2000.0,
                "ms",
            ),
            FxParam::new(
                "Floor",
                ((self.floor_db + 80.0) / 80.0).clamp(0.0, 1.0),
                -80.0,
                0.0,
                "dB",
            ),
            FxParam::new(
                "Hyst",
                (self.hysteresis_db / 24.0).clamp(0.0, 1.0),
                0.0,
                24.0,
                "dB",
            ),
            FxParam::new("Wet", self.mix, 0.0, 1.0, ""),
        ]
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => self.threshold_db = -80.0 + v * 80.0,
            1 => self.attack_ms = 0.1 + v * 49.9,
            2 => self.hold_ms = 1.0 + v * 499.0,
            3 => self.release_ms = 10.0 + v * 990.0,
            4 => self.floor_db = -80.0 + v * 80.0,
            5 => self.hysteresis_db = v * 24.0,
            6 => self.mix = v,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::FxProcessor;

    #[test]
    fn gate_opens_above_threshold() {
        let mut g = Gate::new();
        g.threshold_db = -40.0;
        g.attack_ms = 0.1;
        g.hold_ms = 0.0;
        g.release_ms = 1.0;
        g.floor_db = -80.0;
        // Hot signal above threshold.
        let mut buf = vec![0.2f32; 1024];
        g.process_block(&mut buf, 48000);
        // Gate should be open → signal mostly passes.
        let last = buf[1020].abs();
        assert!(
            last > 0.1,
            "gate should be open above threshold, got {}",
            last
        );
    }

    #[test]
    fn gate_closes_below_threshold() {
        let mut g = Gate::new();
        g.threshold_db = -6.0; // high threshold
        g.attack_ms = 0.1;
        g.hold_ms = 1.0;
        g.release_ms = 1.0;
        g.floor_db = -80.0;
        // Quiet signal below threshold.
        let mut buf = vec![0.001f32; 4096];
        g.process_block(&mut buf, 48000);
        let last = buf[4090].abs();
        assert!(
            last < 0.001,
            "gate should be closed below threshold, got {}",
            last
        );
    }

    /// A signal parked between the two thresholds must not chatter: once it is
    /// open, only falling past `threshold - hysteresis` closes it.
    #[test]
    fn hysteresis_holds_a_signal_sitting_on_the_threshold() {
        let mut g = Gate::new();
        g.threshold_db = -20.0; // 0.1
        g.hysteresis_db = 12.0; // closes at ~0.025
        g.attack_ms = 0.1;
        g.hold_ms = 0.0;
        g.release_ms = 1.0;
        g.floor_db = -80.0;

        // Open it.
        let mut open = vec![0.2f32; 2048];
        g.process_block(&mut open, 48000);
        assert_eq!(g.state(), GateState::Open);

        // Now sit just below the open threshold but above the close one.
        let mut between = vec![0.06f32; 8192];
        g.process_block(&mut between, 48000);
        assert_eq!(
            g.state(),
            GateState::Open,
            "hysteresis should keep it open between the thresholds"
        );
        assert!(
            between[8190].abs() > 0.05,
            "signal should still pass, got {}",
            between[8190]
        );

        // Below the close threshold it goes through hold/release to closed.
        let mut quiet = vec![0.001f32; 16384];
        g.process_block(&mut quiet, 48000);
        assert_eq!(g.state(), GateState::Closed);
    }

    #[test]
    fn the_hold_phase_keeps_the_gate_open_for_its_time() {
        let mut g = Gate::new();
        g.threshold_db = -20.0;
        g.hysteresis_db = 0.0;
        g.attack_ms = 0.1;
        g.hold_ms = 100.0;
        g.release_ms = 1.0;
        let mut open = vec![0.5f32; 2048];
        g.process_block(&mut open, 48000);
        // 10 ms of silence: well inside the 100 ms hold.
        let mut gap = vec![0.0f32; 48 * 10 * 2];
        g.process_block(&mut gap, 48000);
        assert_eq!(g.state(), GateState::Hold);
    }
}
