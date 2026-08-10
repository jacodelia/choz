//! Space Echo — vintage tape-delay + spring-reverb, in the spirit of the Roland
//! RE-201 (NOT a clone of its circuit; an original model of its *acoustic*
//! behaviour). Signal flow per sample:
//!
//! ```text
//!   in ──┬─────────────────────────────────────────────► dry
//!        │   ┌──────────────── feedback loop ─────────┐
//!        ▼   ▼                                         │
//!      write→[ tape buffer ]→ 3 playback heads ─sum──┐ │
//!                ▲ wow+flutter modulate read pos      │ │
//!                                                     ▼ │
//!                              tape colour: HP→LP(age)→tanh(age)
//!                                                     │ │
//!                                          ×feedback ─┴─┘
//!        echo sum ──► spring reverb (3 allpass + 2 comb) ──► wet
//! ```
//!
//! DSP decisions (documented like a pedal — see docs/audio/space-echo.md):
//!   • 3 virtual heads at fixed delay ratios = the RE-201 multi-head "smear".
//!   • Wow (slow ~0.6 Hz) + flutter (fast ~7 Hz) modulate the fractional read
//!     position → the characteristic pitch wobble. Linear interpolation.
//!   • Tape colour lives in the FEEDBACK path only, so each repeat is darker and
//!     more saturated than the last (cumulative degradation): 1-pole HP (lf
//!     rolloff) → 1-pole LP whose corner falls with `age` → tanh whose drive
//!     rises with `age`.
//!   • Feedback may exceed 1.0 for controlled self-oscillation; a tanh soft-clip
//!     on the loop keeps it bounded.
//!   • Spring reverb = Schroeder dispersion (allpass) + 2 short combs for the
//!     metallic resonant tail.

use super::FxProcessor;

const MAX_DELAY_S: f32 = 2.0;
/// Fixed RE-201-style head delay ratios and their relative gains.
const HEAD_RATIOS: [f32; 3] = [1.0, 0.68, 0.40];
const HEAD_GAINS: [f32; 3] = [1.0, 0.70, 0.45];

/// One-pole lowpass (tape HF loss / damping).
#[derive(Clone, Copy)]
struct OnePole {
    z: f32,
    a: f32,
}
impl OnePole {
    fn new() -> Self {
        Self { z: 0.0, a: 0.5 }
    }
    /// Set corner frequency.
    fn set_hz(&mut self, hz: f32, sr: f32) {
        let x = (-2.0 * std::f32::consts::PI * hz.clamp(20.0, sr * 0.49) / sr).exp();
        self.a = x;
    }
    #[inline]
    fn lp(&mut self, x: f32) -> f32 {
        self.z = self.a * self.z + (1.0 - self.a) * x;
        self.z
    }
    #[inline]
    fn hp(&mut self, x: f32) -> f32 {
        x - self.lp(x)
    }
}

/// Schroeder allpass for spring dispersion.
struct Allpass {
    buf: Vec<f32>,
    pos: usize,
    g: f32,
}
impl Allpass {
    fn new(len: usize, g: f32) -> Self {
        Self {
            buf: vec![0.0; len.max(1)],
            pos: 0,
            g,
        }
    }
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let buffered = self.buf[self.pos];
        let y = -x + buffered;
        self.buf[self.pos] = x + buffered * self.g;
        self.pos = (self.pos + 1) % self.buf.len();
        y
    }
}

/// Feedback comb (metallic spring resonance).
struct Comb {
    buf: Vec<f32>,
    pos: usize,
    fb: f32,
    damp: OnePole,
}
impl Comb {
    fn new(len: usize, fb: f32) -> Self {
        Self {
            buf: vec![0.0; len.max(1)],
            pos: 0,
            fb,
            damp: OnePole::new(),
        }
    }
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.buf[self.pos];
        let d = self.damp.lp(y);
        self.buf[self.pos] = x + d * self.fb;
        self.pos = (self.pos + 1) % self.buf.len();
        y
    }
}

/// One tape channel: a circular buffer read by 3 modulated heads.
struct Tape {
    buf: Vec<f32>,
    write: usize,
    hp: OnePole,
    lp: OnePole,
}
impl Tape {
    fn new(len: usize) -> Self {
        Self {
            buf: vec![0.0; len.max(2)],
            write: 0,
            hp: OnePole::new(),
            lp: OnePole::new(),
        }
    }
    #[inline]
    fn read(&self, delay_samps: f32) -> f32 {
        let len = self.buf.len();
        let rp = (self.write as f32 - delay_samps).rem_euclid(len as f32);
        let i0 = rp as usize % len;
        let i1 = (i0 + 1) % len;
        let frac = rp - rp.floor();
        self.buf[i0] * (1.0 - frac) + self.buf[i1] * frac
    }
    #[inline]
    fn write(&mut self, x: f32) {
        self.buf[self.write] = x;
        self.write = (self.write + 1) % self.buf.len();
    }
}

/// Vintage tape echo + spring reverb.
pub struct SpaceEcho {
    sample_rate: u32,
    // Normalised 0..1 controls (mapped to native each block).
    time: f32,
    feedback: f32,
    wow: f32,
    flutter: f32,
    age: f32,
    spring: f32,
    tone: f32,
    wet: f32,

    tape_l: Tape,
    tape_r: Tape,
    // Spring reverb network (shared mono tail, panned out).
    aps: Vec<Allpass>,
    combs: Vec<Comb>,

    wow_phase: f32,
    flutter_phase: f32,
}

impl SpaceEcho {
    /// All params normalised 0..1. `time`→50..1500 ms, `feedback`→0..1.1, etc.
    // One argument per knob: the FX chain builds these straight from the
    // parameter vector, so a struct would only add ceremony.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        sr: u32,
        time: f32,
        feedback: f32,
        wow: f32,
        flutter: f32,
        age: f32,
        spring: f32,
        tone: f32,
    ) -> Self {
        let sr = sr.max(8000);
        let len = (MAX_DELAY_S * sr as f32) as usize + 4;
        // Spring: 3 allpass (prime-ish lengths) + 2 short combs, scaled to sr.
        let s = |n: usize| ((n as f32) * sr as f32 / 44100.0) as usize;
        let aps = vec![
            Allpass::new(s(225), 0.6),
            Allpass::new(s(556), 0.6),
            Allpass::new(s(341), 0.6),
        ];
        let combs = vec![Comb::new(s(1557), 0.7), Comb::new(s(1116), 0.7)];
        Self {
            sample_rate: sr,
            time,
            feedback,
            wow,
            flutter,
            age,
            spring,
            tone,
            wet: 0.4,
            tape_l: Tape::new(len),
            tape_r: Tape::new(len),
            aps,
            combs,
            wow_phase: 0.0,
            flutter_phase: 0.3,
        }
    }

    fn delay_samps(&self) -> f32 {
        let ms = 50.0 + self.time.clamp(0.0, 1.0) * 1450.0;
        (ms / 1000.0) * self.sample_rate as f32
    }
}

impl FxProcessor for SpaceEcho {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        if sample_rate != self.sample_rate {
            *self = SpaceEcho::new(
                sample_rate,
                self.time,
                self.feedback,
                self.wow,
                self.flutter,
                self.age,
                self.spring,
                self.tone,
            );
        }
        let sr = self.sample_rate as f32;
        let base = self.delay_samps();
        // Tape colour: HF loss corner falls from 10 kHz (new) to ~1.8 kHz (worn);
        // `tone` adds an extra global hi-cut. LP applied in the feedback loop.
        let lp_hz = (1800.0 + (1.0 - self.age) * 8200.0) * (0.4 + 0.6 * self.tone);
        self.tape_l.lp.set_hz(lp_hz, sr);
        self.tape_r.lp.set_hz(lp_hz, sr);
        self.tape_l.hp.set_hz(110.0, sr);
        self.tape_r.hp.set_hz(110.0, sr);
        for c in self.combs.iter_mut() {
            c.damp.set_hz(2600.0, sr);
        }

        let fb = self.feedback.clamp(0.0, 1.0) * 1.1;
        let sat_drive = 1.0 + self.age * 3.0;
        // Wow/flutter modulation depth, in samples.
        let wow_d = self.wow * 0.004 * sr; // up to ~4 ms slow
        let flut_d = self.flutter * 0.0009 * sr; // up to ~0.9 ms fast
        let wow_inc = 2.0 * std::f32::consts::PI * 0.6 / sr;
        let flut_inc = 2.0 * std::f32::consts::PI * 7.0 / sr;

        let frames = buf.len() / 2;
        for i in 0..frames {
            let dry_l = buf[i * 2];
            let dry_r = buf[i * 2 + 1];

            self.wow_phase = (self.wow_phase + wow_inc) % (2.0 * std::f32::consts::PI);
            self.flutter_phase = (self.flutter_phase + flut_inc) % (2.0 * std::f32::consts::PI);
            // Right channel uses an offset wow phase → stereo drift.
            let mod_l = self.wow_phase.sin() * wow_d + self.flutter_phase.sin() * flut_d;
            let mod_r =
                (self.wow_phase + 1.7).sin() * wow_d + (self.flutter_phase + 0.9).sin() * flut_d;

            // Sum the 3 heads per channel.
            let mut echo_l = 0.0;
            let mut echo_r = 0.0;
            for (ratio, gain) in HEAD_RATIOS.iter().zip(HEAD_GAINS.iter()) {
                echo_l += self.tape_l.read(base * ratio + mod_l) * gain;
                echo_r += self.tape_r.read(base * ratio + mod_r) * gain;
            }

            // Feedback colour: HP → LP(age) → tanh(age), then back into the tape.
            let col_l = (self.tape_l.lp.lp(self.tape_l.hp.hp(echo_l)) * sat_drive).tanh();
            let col_r = (self.tape_r.lp.lp(self.tape_r.hp.hp(echo_r)) * sat_drive).tanh();
            self.tape_l.write(dry_l + col_l * fb);
            self.tape_r.write(dry_r + col_r * fb);

            // Spring reverb on the mono echo sum.
            let mut spr = (echo_l + echo_r) * 0.5;
            for ap in self.aps.iter_mut() {
                spr = ap.process(spr);
            }
            let mut tail = 0.0;
            for cb in self.combs.iter_mut() {
                tail += cb.process(spr);
            }
            tail *= 0.5;

            let wet_l = echo_l + tail * self.spring;
            let wet_r = echo_r + tail * self.spring;
            buf[i * 2] = dry_l + self.wet * (wet_l - dry_l);
            buf[i * 2 + 1] = dry_r + self.wet * (wet_r - dry_r);
        }
    }

    fn reset(&mut self) {
        *self = SpaceEcho::new(
            self.sample_rate,
            self.time,
            self.feedback,
            self.wow,
            self.flutter,
            self.age,
            self.spring,
            self.tone,
        );
    }

    fn set_mix(&mut self, wet: f32) {
        self.wet = wet.clamp(0.0, 1.0);
    }
    fn name(&self) -> &str {
        "Space Echo"
    }

    fn params(&self) -> Vec<super::FxParam> {
        use super::FxParam as P;
        vec![
            P::new("Time", self.time, 0.0, 1.0, ""),
            P::new("Feedback", self.feedback, 0.0, 1.0, ""),
            P::new("Wow", self.wow, 0.0, 1.0, ""),
            P::new("Flutter", self.flutter, 0.0, 1.0, ""),
            P::new("Age", self.age, 0.0, 1.0, ""),
            P::new("Spring", self.spring, 0.0, 1.0, ""),
            P::new("Tone", self.tone, 0.0, 1.0, ""),
            P::new("Wet", self.wet, 0.0, 1.0, ""),
        ]
    }

    /// Live param update — preserves the tape buffer + reverb tail (no rebuild),
    /// so e.g. crossfading Time/Feedback or automating Wow stays click-free.
    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => self.time = v,
            1 => self.feedback = v,
            2 => self.wow = v,
            3 => self.flutter = v,
            4 => self.age = v,
            5 => self.spring = v,
            6 => self.tone = v,
            7 => self.wet = v,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn space_echo_is_finite_and_bounded() {
        let mut fx = SpaceEcho::new(48000, 0.4, 0.6, 0.3, 0.2, 0.5, 0.4, 0.6);
        fx.set_mix(0.6);
        // Impulse then silence — should ring out without blowing up.
        let mut block = vec![0.0f32; 4096];
        block[0] = 1.0;
        block[1] = 1.0;
        for _ in 0..40 {
            fx.process_block(&mut block, 48000);
            assert!(block.iter().all(|s| s.is_finite()));
            assert!(
                block.iter().all(|s| s.abs() < 8.0),
                "self-oscillation unbounded"
            );
            block.iter_mut().for_each(|s| *s = 0.0);
        }
    }

    #[test]
    fn space_echo_reset_clears_tail() {
        let mut fx = SpaceEcho::new(48000, 0.3, 0.7, 0.2, 0.2, 0.3, 0.3, 0.5);
        let mut block = vec![0.5f32; 256];
        fx.process_block(&mut block, 48000);
        fx.reset();
        assert!(fx.tape_l.buf.iter().all(|&v| v == 0.0));
    }
}
