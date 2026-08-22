//! Tremolo and auto-pan: the same LFO, pointed at level or at position.
//!
//! One processor, two effects. A tremolo moves both channels' gain together; an
//! auto-pan moves the balance between them. The difference is four lines, and
//! the shape, rate, depth and stereo spread controls are identical — which is
//! why this is one file and not two that fall out of step.
//!
//! The existing [`super::pan::Pan`] is static: a position, not a movement. It
//! stays as it is, because "put this tab slightly left" is not a modulation.

use super::lfo::{Lfo, Wave};
use super::smooth::Smoothed;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModTarget {
    /// Level, both channels together.
    Tremolo,
    /// Balance, one channel against the other.
    AutoPan,
}

pub struct Tremolo {
    target: ModTarget,
    wave: Wave,
    /// Cycles per second, 0.02–20.
    rate_hz: f32,
    /// How far the LFO moves it, 0..1.
    depth: Smoothed,
    /// How far the right channel runs behind the left, in cycles.
    spread: f32,
    lfo: Lfo,
    mix: f32,
    sample_rate: f32,
}

impl Tremolo {
    pub fn new(target: ModTarget, sample_rate: u32) -> Self {
        let sr = sample_rate.max(8000) as f32;
        Self {
            target,
            wave: Wave::Sine,
            rate_hz: 4.0,
            depth: Smoothed::new(0.5, 20.0, sr),
            // An auto-pan with both channels in phase is a tremolo, so that is
            // what it starts as: the two channels in opposition.
            spread: match target {
                ModTarget::Tremolo => 0.0,
                ModTarget::AutoPan => 0.5,
            },
            lfo: Lfo::new(),
            mix: 1.0,
            sample_rate: sr,
        }
    }

    /// Build from the rack's knob positions: rate, depth, shape, spread.
    pub fn with_params(target: ModTarget, sample_rate: u32, p: &[f32]) -> Self {
        let get = |i: usize, d: f32| p.get(i).copied().unwrap_or(d);
        let mut t = Self::new(target, sample_rate);
        t.set_rate(get(0, 0.35));
        t.set_depth(get(1, 0.5));
        t.wave = Wave::from_norm(get(2, 0.0));
        t.spread = get(
            3,
            match target {
                ModTarget::Tremolo => 0.0,
                ModTarget::AutoPan => 0.5,
            },
        )
        .clamp(0.0, 1.0);
        t
    }

    /// 0..1 → 0.02–20 Hz, exponential: the ear hears rate in ratios, and a
    /// linear knob spends its first half on speeds nobody sets.
    pub fn set_rate(&mut self, v: f32) {
        self.rate_hz = 0.02 * 1000.0f32.powf(v.clamp(0.0, 1.0));
    }

    pub fn set_depth(&mut self, v: f32) {
        self.depth.set_target(v.clamp(0.0, 1.0));
    }

    pub fn rate_hz(&self) -> f32 {
        self.rate_hz
    }
}

impl super::FxProcessor for Tremolo {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        let sr = sample_rate.max(8000) as f32;
        if (sr - self.sample_rate).abs() > 0.5 {
            self.sample_rate = sr;
            self.depth.set_sample_rate(sr);
        }
        let wave = self.wave;
        let rate = self.rate_hz;
        let spread = self.spread;
        let mix = self.mix;

        for frame in buf.as_chunks_mut::<2>().0 {
            let m = self.lfo.tick(wave, rate, sr, spread);
            let depth = self.depth.tick();
            let (dry_l, dry_r) = (frame[0], frame[1]);

            let (gl, gr) = match self.target {
                // Modulate downwards from unity: at any depth the loudest the
                // tremolo gets is the signal it was given, so switching it on
                // never makes the tab jump in level.
                ModTarget::Tremolo => (
                    1.0 - depth * 0.5 * (1.0 - m[0]),
                    1.0 - depth * 0.5 * (1.0 - m[1]),
                ),
                // Constant power, so a sweep across the image does not dip in
                // the middle the way a linear pan does.
                ModTarget::AutoPan => {
                    let p = (m[0] * depth).clamp(-1.0, 1.0);
                    let angle = (p + 1.0) * std::f32::consts::FRAC_PI_4;
                    (
                        angle.cos() * std::f32::consts::SQRT_2,
                        angle.sin() * std::f32::consts::SQRT_2,
                    )
                }
            };

            frame[0] = dry_l + mix * (dry_l * gl - dry_l);
            frame[1] = dry_r + mix * (dry_r * gr - dry_r);
        }
    }

    fn reset(&mut self) {
        self.lfo.reset();
        self.depth.snap(self.depth.target());
    }

    fn set_mix(&mut self, wet: f32) {
        self.mix = wet.clamp(0.0, 1.0);
    }

    fn name(&self) -> &str {
        match self.target {
            ModTarget::Tremolo => "Tremolo",
            ModTarget::AutoPan => "AutoPan",
        }
    }

    fn params(&self) -> Vec<crate::fx::FxParam> {
        use crate::fx::FxParam;
        vec![
            FxParam::new(
                "Rate",
                (self.rate_hz / 0.02).log(1000.0).clamp(0.0, 1.0),
                0.02,
                20.0,
                "Hz",
            ),
            FxParam::new("Depth", self.depth.target(), 0.0, 1.0, ""),
            FxParam::new("Shape", self.wave.to_norm(), 0.0, 1.0, ""),
            FxParam::new("Spread", self.spread, 0.0, 1.0, ""),
            FxParam::new("Wet", self.mix, 0.0, 1.0, ""),
        ]
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => self.set_rate(v),
            1 => self.set_depth(v),
            2 => self.wave = Wave::from_norm(v),
            3 => self.spread = v,
            4 => self.mix = v,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::FxProcessor;

    /// DC in: the output *is* the modulation, which is the only way to look at
    /// it without guessing.
    fn envelope(fx: &mut Tremolo, frames: usize, sr: u32) -> (Vec<f32>, Vec<f32>) {
        let mut buf = vec![1.0f32; frames * 2];
        fx.process_block(&mut buf, sr);
        (
            buf.iter().step_by(2).copied().collect(),
            buf.iter().skip(1).step_by(2).copied().collect(),
        )
    }

    #[test]
    fn a_tremolo_moves_the_level_and_never_above_the_signal_it_was_given() {
        let mut t = Tremolo::new(ModTarget::Tremolo, 48000);
        t.set_rate(1.0); // 20 Hz: whole cycles inside the window below
        t.set_depth(1.0);
        let (l, r) = envelope(&mut t, 48000, 48000);
        let hi = l.iter().cloned().fold(f32::MIN, f32::max);
        let lo = l.iter().cloned().fold(f32::MAX, f32::min);
        assert!(hi <= 1.0001, "a tremolo must not add gain: {hi}");
        assert!(lo < 0.05, "full depth should reach silence: {lo}");
        // No spread: both channels are the same movement.
        assert_eq!(l, r);
    }

    #[test]
    fn zero_depth_is_a_wire() {
        let mut t = Tremolo::new(ModTarget::Tremolo, 48000);
        t.set_depth(0.0);
        t.reset();
        let (l, _) = envelope(&mut t, 4096, 48000);
        assert!(
            l.iter().all(|s| (s - 1.0).abs() < 1e-4),
            "depth 0 changed the signal"
        );
    }

    /// The auto-pan moves the *balance*: when one channel is up the other is
    /// down, and the two never move together.
    #[test]
    fn an_auto_pan_trades_one_channel_against_the_other() {
        let mut t = Tremolo::new(ModTarget::AutoPan, 48000);
        t.set_rate(1.0);
        t.set_depth(1.0);
        let (l, r) = envelope(&mut t, 48000, 48000);
        let peak_l = l.iter().cloned().fold(f32::MIN, f32::max);
        let peak_r = r.iter().cloned().fold(f32::MIN, f32::max);
        assert!(
            peak_l > 1.3 && peak_r > 1.3,
            "each side should reach its own extreme"
        );
        // Where the left is loudest the right is quietest.
        let at_max = l
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap()
            .0;
        assert!(
            r[at_max] < 0.2,
            "the other channel should be away: {}",
            r[at_max]
        );
    }

    /// Constant power: whatever the position, the two channels together carry
    /// the same energy. A pan that dips in the middle is a tremolo by accident.
    #[test]
    fn the_auto_pan_keeps_its_power_constant() {
        let mut t = Tremolo::new(ModTarget::AutoPan, 48000);
        t.set_rate(0.5);
        t.set_depth(1.0);
        let (l, r) = envelope(&mut t, 48000, 48000);
        for (a, b) in l.iter().zip(r.iter()) {
            let power = a * a + b * b;
            assert!(
                (power - 2.0).abs() < 0.02,
                "power moved to {power} at ({a}, {b})"
            );
        }
    }

    #[test]
    fn every_shape_survives_silence_extremes_and_a_rate_change() {
        for wave in Wave::ALL {
            for target in [ModTarget::Tremolo, ModTarget::AutoPan] {
                let mut t = Tremolo::new(target, 48000);
                t.wave = wave;
                t.set_depth(1.0);
                t.set_rate(1.0);
                let mut buf = vec![0.0f32; 512];
                t.process_block(&mut buf, 48000);
                assert!(
                    buf.iter().all(|s| *s == 0.0),
                    "{} rang in silence",
                    wave.label()
                );
                let mut hot = vec![4.0f32; 512];
                t.process_block(&mut hot, 96000);
                assert!(hot.iter().all(|s| s.is_finite()));
                t.process_block(&mut [], 96000);
                t.process_block(&mut [0.0], 96000);
                t.reset();
            }
        }
    }

    #[test]
    fn the_block_size_does_not_change_the_result() {
        let run = |chunk: usize| {
            let mut t = Tremolo::with_params(ModTarget::Tremolo, 48000, &[0.5, 1.0, 0.0, 0.0]);
            let mut buf = vec![1.0f32; 4096];
            for part in buf.chunks_mut(chunk * 2) {
                t.process_block(part, 48000);
            }
            buf
        };
        let a = run(64);
        let b = run(517);
        for (x, y) in a.iter().zip(b.iter()) {
            assert!((x - y).abs() < 1e-5, "{x} vs {y}");
        }
    }
}
