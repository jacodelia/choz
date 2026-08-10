//! Two stompbox-flavoured distortions.
//!
//! * [`AmberFang`] — the asymmetric op-amp-plus-diode voice of a hard-clipping
//!   orange distortion pedal: a gain stage into asymmetric clipping, then a
//!   single tone knob tilting between a dark low-pass and a bright high-pass.
//! * [`VelvetFuzz`] — the four-stage sustain of a big violet fuzz: two cascaded
//!   soft-clipping stages with an inter-stage high-pass, then the scooped tone
//!   stack that gives that pedal its hollow midrange.
//!
//! Both waveshape at 2× oversampling (shared with the saturators in
//! [`super::utility`]), so hard clipping doesn't fold aliasing back down.

use super::utility::{Biquad, Oversampler2x};
use super::FxProcessor;

/// One-pole state used for the simple tilt/scoop tone stacks below.
#[derive(Clone, Copy, Default)]
struct OnePole {
    z: f32,
}

impl OnePole {
    /// Low-passed value; `a` is the smoothing coefficient (0..1).
    #[inline]
    fn lp(&mut self, x: f32, a: f32) -> f32 {
        self.z += a * (x - self.z);
        self.z
    }
}

/// Coefficient for a one-pole low-pass at `fc`.
#[inline]
fn coeff(fc: f32, sr: f32) -> f32 {
    let x = (-2.0 * std::f32::consts::PI * fc / sr).exp();
    (1.0 - x).clamp(0.0, 1.0)
}

// ─── Amber Fang (hard-clipping distortion) ──────────────────────────────────

/// Asymmetric hard-clipping distortion with a tilt tone control.
pub struct AmberFang {
    /// 0..1 — gain into the clipper.
    pub dist: f32,
    /// 0..1 — dark to bright.
    pub tone: f32,
    /// 0..1 — output level.
    pub level: f32,
    wet: f32,
    /// Input high-pass state (kills the mud the gain stage would multiply).
    hp: [OnePole; 2],
    /// Tone-stack low-pass state.
    tone_lp: [OnePole; 2],
    os: [Oversampler2x; 2],
    post: [Biquad; 2],
    sample_rate: u32,
}

impl AmberFang {
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate.max(8000);
        Self {
            dist: 0.5,
            tone: 0.5,
            level: 0.7,
            wet: 1.0,
            hp: [OnePole::default(); 2],
            tone_lp: [OnePole::default(); 2],
            os: [Oversampler2x::new(sr as f32), Oversampler2x::new(sr as f32)],
            post: [
                Biquad::lowpass(6000.0, sr as f32, 0.707),
                Biquad::lowpass(6000.0, sr as f32, 0.707),
            ],
            sample_rate: sr,
        }
    }

    /// Asymmetric clipper: the positive half clips harder than the negative one,
    /// which is what puts even harmonics into this kind of pedal.
    #[inline]
    fn clip(x: f32) -> f32 {
        if x >= 0.0 {
            x.tanh()
        } else {
            // Softer, slightly louder negative half.
            (x * 0.7).tanh() * 1.15
        }
    }
}

impl FxProcessor for AmberFang {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        if sample_rate != self.sample_rate {
            *self = Self {
                dist: self.dist,
                tone: self.tone,
                level: self.level,
                ..Self::new(sample_rate)
            };
        }
        let sr = self.sample_rate as f32;
        let hp_a = coeff(120.0, sr);
        let tone_a = coeff(400.0 + self.tone.clamp(0.0, 1.0) * 3600.0, sr);
        let drive = 1.0 + self.dist.clamp(0.0, 1.0) * 60.0;
        // Loud settings would otherwise just get louder, not dirtier.
        let makeup = 1.0 / (1.0 + drive * 0.05);
        let level = self.level.clamp(0.0, 1.0);

        for frame in buf.chunks_mut(2) {
            if frame.len() < 2 {
                break;
            }
            // `ch` indexes the per-channel filter state as well as the frame.
            #[allow(clippy::needless_range_loop)]
            for ch in 0..2 {
                let dry = frame[ch];
                let low = self.hp[ch].lp(dry, hp_a);
                let x = (dry - low) * drive;
                let shaped = self.os[ch].process(x, Self::clip) * makeup;
                let shaped = self.post[ch].process(shaped);
                // Tone: blend the low-passed body against the rest (the highs).
                let body = self.tone_lp[ch].lp(shaped, tone_a);
                let highs = shaped - body;
                let t = self.tone.clamp(0.0, 1.0);
                let wet = (body * (1.0 - t) + highs * t + body * 0.35) * level;
                frame[ch] = dry + self.wet * (wet - dry);
            }
        }
    }

    fn reset(&mut self) {
        *self = Self {
            dist: self.dist,
            tone: self.tone,
            level: self.level,
            ..Self::new(self.sample_rate)
        };
    }

    fn set_mix(&mut self, wet: f32) {
        self.wet = wet.clamp(0.0, 1.0);
    }

    fn name(&self) -> &str {
        "Amber Fang"
    }

    fn params(&self) -> Vec<super::FxParam> {
        use super::FxParam as P;
        vec![
            P::new("Dist", self.dist, 0.0, 1.0, ""),
            P::new("Tone", self.tone, 0.0, 1.0, ""),
            P::new("Level", self.level, 0.0, 1.0, ""),
            P::new("Wet", self.wet, 0.0, 1.0, ""),
        ]
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => self.dist = v,
            1 => self.tone = v,
            2 => self.level = v,
            3 => self.wet = v,
            _ => {}
        }
    }
}

// ─── Velvet Fuzz (cascaded fuzz with a scooped tone stack) ──────────────────

/// Two-stage sustaining fuzz with the classic mid-scooped tone control.
pub struct VelvetFuzz {
    /// 0..1 — how hard the stages are driven (the "sustain" knob).
    pub sustain: f32,
    /// 0..1 — bass to treble, scooped in the middle.
    pub tone: f32,
    /// 0..1 — output level.
    pub level: f32,
    wet: f32,
    /// Input, inter-stage and tone-stack one-pole states, per channel.
    hp_in: [OnePole; 2],
    hp_mid: [OnePole; 2],
    tone_lp: [OnePole; 2],
    tone_hp: [OnePole; 2],
    os: [[Oversampler2x; 2]; 2],
    post: [Biquad; 2],
    sample_rate: u32,
}

impl VelvetFuzz {
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate.max(8000);
        let os = || [Oversampler2x::new(sr as f32), Oversampler2x::new(sr as f32)];
        Self {
            sustain: 0.6,
            tone: 0.5,
            level: 0.6,
            wet: 1.0,
            hp_in: [OnePole::default(); 2],
            hp_mid: [OnePole::default(); 2],
            tone_lp: [OnePole::default(); 2],
            tone_hp: [OnePole::default(); 2],
            os: [os(), os()],
            post: [
                Biquad::lowpass(7000.0, sr as f32, 0.707),
                Biquad::lowpass(7000.0, sr as f32, 0.707),
            ],
            sample_rate: sr,
        }
    }

    /// Symmetric soft clip — two of these in series is what makes the fuzz
    /// compress into that endless sustain instead of just breaking up.
    #[inline]
    fn stage(x: f32) -> f32 {
        x.tanh()
    }
}

impl FxProcessor for VelvetFuzz {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        if sample_rate != self.sample_rate {
            *self = Self {
                sustain: self.sustain,
                tone: self.tone,
                level: self.level,
                ..Self::new(sample_rate)
            };
        }
        let sr = self.sample_rate as f32;
        let in_a = coeff(80.0, sr);
        let mid_a = coeff(700.0, sr);
        let lp_a = coeff(800.0, sr);
        let hp_a = coeff(1600.0, sr);
        let drive = 1.0 + self.sustain.clamp(0.0, 1.0) * 80.0;
        let makeup = 1.0 / (1.0 + drive * 0.08);
        let level = self.level.clamp(0.0, 1.0);
        let t = self.tone.clamp(0.0, 1.0);

        for frame in buf.chunks_mut(2) {
            if frame.len() < 2 {
                break;
            }
            #[allow(clippy::needless_range_loop)]
            for ch in 0..2 {
                let dry = frame[ch];
                // Stage 1.
                let low = self.hp_in[ch].lp(dry, in_a);
                let x = (dry - low) * drive;
                let y = self.os[0][ch].process(x, Self::stage);
                // Inter-stage high-pass, then stage 2 — the cascade is the sound.
                let mid = self.hp_mid[ch].lp(y, mid_a);
                let y2 = self.os[1][ch].process((y - mid) * 4.0, Self::stage) * makeup;
                let y2 = self.post[ch].process(y2);
                // Tone stack: a bass path and a treble path, mixed against each
                // other so the middle drops out at the centre of the knob.
                let bass = self.tone_lp[ch].lp(y2, lp_a);
                let treble = y2 - self.tone_hp[ch].lp(y2, hp_a);
                let wet = (bass * (1.0 - t) + treble * t) * level * 2.0;
                frame[ch] = dry + self.wet * (wet - dry);
            }
        }
    }

    fn reset(&mut self) {
        *self = Self {
            sustain: self.sustain,
            tone: self.tone,
            level: self.level,
            ..Self::new(self.sample_rate)
        };
    }

    fn set_mix(&mut self, wet: f32) {
        self.wet = wet.clamp(0.0, 1.0);
    }

    fn name(&self) -> &str {
        "Velvet Fuzz"
    }

    fn params(&self) -> Vec<super::FxParam> {
        use super::FxParam as P;
        vec![
            P::new("Sustain", self.sustain, 0.0, 1.0, ""),
            P::new("Tone", self.tone, 0.0, 1.0, ""),
            P::new("Level", self.level, 0.0, 1.0, ""),
            P::new("Wet", self.wet, 0.0, 1.0, ""),
        ]
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => self.sustain = v,
            1 => self.tone = v,
            2 => self.level = v,
            3 => self.wet = v,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A -6 dBFS sine, several blocks, so the filter states settle.
    fn sine(frames: usize, freq: f32, sr: f32, amp: f32) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let s = (2.0 * std::f32::consts::PI * freq * i as f32 / sr).sin() * amp;
                [s, s]
            })
            .collect()
    }

    fn peak(buf: &[f32]) -> f32 {
        buf.iter().copied().map(f32::abs).fold(0.0, f32::max)
    }

    /// Distortion must compress: a quiet input and a loud one come out much
    /// closer together than they went in.
    #[test]
    fn both_pedals_clip_and_stay_finite() {
        let sr = 48_000u32;
        for (name, mut fx) in [
            (
                "AmberFang",
                Box::new(AmberFang::new(sr)) as Box<dyn FxProcessor>,
            ),
            (
                "VelvetFuzz",
                Box::new(VelvetFuzz::new(sr)) as Box<dyn FxProcessor>,
            ),
        ] {
            fx.set_mix(1.0);
            let mut quiet = sine(2048, 220.0, sr as f32, 0.05);
            let mut loud = sine(2048, 220.0, sr as f32, 0.9);
            for _ in 0..4 {
                fx.process_block(&mut quiet, sr);
                fx.process_block(&mut loud, sr);
            }
            for (label, buf) in [("quiet", &quiet), ("loud", &loud)] {
                assert!(
                    buf.iter().all(|s| s.is_finite()),
                    "{name} produced non-finite output on the {label} input"
                );
                assert!(
                    peak(buf) < 4.0,
                    "{name} ran away on the {label} input: {}",
                    peak(buf)
                );
            }
            let ratio = peak(&loud) / peak(&quiet).max(1e-9);
            assert!(
                ratio < 8.0,
                "{name} barely compressed (18:1 in, {ratio:.1}:1 out)"
            );
            assert!(peak(&quiet) > 0.0, "{name} silenced a quiet input");
        }
    }

    /// The tone knob really moves the balance: fully clockwise must be brighter
    /// (more sample-to-sample movement) than fully counter-clockwise.
    #[test]
    fn the_tone_knob_changes_the_spectrum() {
        let sr = 48_000u32;
        let hf = |v: &[f32]| v.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum::<f32>();
        for name in ["AmberFang", "VelvetFuzz"] {
            let run = |tone: f32| {
                let mut fx: Box<dyn FxProcessor> = if name == "AmberFang" {
                    let mut p = AmberFang::new(sr);
                    p.tone = tone;
                    Box::new(p)
                } else {
                    let mut p = VelvetFuzz::new(sr);
                    p.tone = tone;
                    Box::new(p)
                };
                fx.set_mix(1.0);
                let mut buf = sine(4096, 300.0, sr as f32, 0.5);
                for _ in 0..3 {
                    fx.process_block(&mut buf, sr);
                }
                hf(&buf) / peak(&buf).max(1e-9).powi(2)
            };
            assert!(run(1.0) > run(0.0), "{name}: tone up must be brighter");
        }
    }

    /// Dry/wet still works: fully dry is the input untouched.
    #[test]
    fn dry_is_the_input() {
        let sr = 48_000u32;
        let mut fx = AmberFang::new(sr);
        fx.set_mix(0.0);
        let input = sine(256, 440.0, sr as f32, 0.4);
        let mut buf = input.clone();
        fx.process_block(&mut buf, sr);
        for (a, b) in buf.iter().zip(input.iter()) {
            assert!((a - b).abs() < 1e-6, "dry output must equal the input");
        }
    }
}
