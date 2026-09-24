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

use super::delay_line::MAX_RATE;
use super::FxProcessor;

const MAX_DELAY_S: f32 = 2.0;
/// Fixed RE-201-style head delay ratios and their relative gains.
///
/// Equidistant, as on the machine: heads 1, 2 and 3 at a third, two thirds and
/// all of the delay — so with Sync on, all three land on the grid.
const HEAD_RATIOS: [f32; 3] = [1.0, 2.0 / 3.0, 1.0 / 3.0];
const HEAD_GAINS: [f32; 3] = [1.0, 0.70, 0.45];

/// The mode selector: which heads play, in `HEAD_RATIOS` order (longest
/// first). Named the RE-201 way, head 1 being the shortest — seven echo
/// combinations and then the spring on its own.
pub const HEAD_MODES: [(&str, [bool; 3]); 8] = [
    ("1", [false, false, true]),
    ("2", [false, true, false]),
    ("3", [true, false, false]),
    ("1+2", [false, true, true]),
    ("2+3", [true, true, false]),
    ("1+3", [true, false, true]),
    ("1+2+3", [true, true, true]),
    ("REV", [false, false, false]),
];
/// All three heads — what it always was, and the knob's default.
pub const HEADS_DEFAULT: f32 = 6.0 / 7.0;

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
    len: usize,
    pos: usize,
    g: f32,
}
impl Allpass {
    /// `cap` is the buffer, `len` the delay actually read. They are separate so
    /// that a sample-rate change can move the delay without asking the audio
    /// thread for memory — see [`SpaceEcho::retune`].
    fn new(cap: usize, len: usize, g: f32) -> Self {
        Self {
            buf: vec![0.0; cap.max(1)],
            len: len.clamp(1, cap.max(1)),
            pos: 0,
            g,
        }
    }
    fn set_len(&mut self, len: usize) {
        self.len = len.clamp(1, self.buf.len());
        self.pos %= self.len;
    }
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let buffered = self.buf[self.pos];
        let y = -x + buffered;
        self.buf[self.pos] = x + buffered * self.g;
        self.pos = (self.pos + 1) % self.len;
        y
    }
}

/// Feedback comb (metallic spring resonance).
struct Comb {
    buf: Vec<f32>,
    len: usize,
    pos: usize,
    fb: f32,
    damp: OnePole,
}
impl Comb {
    fn new(cap: usize, len: usize, fb: f32) -> Self {
        Self {
            buf: vec![0.0; cap.max(1)],
            len: len.clamp(1, cap.max(1)),
            pos: 0,
            fb,
            damp: OnePole::new(),
        }
    }
    fn set_len(&mut self, len: usize) {
        self.len = len.clamp(1, self.buf.len());
        self.pos %= self.len;
    }
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.buf[self.pos];
        let d = self.damp.lp(y);
        self.buf[self.pos] = x + d * self.fb;
        self.pos = (self.pos + 1) % self.len;
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
    /// The delay the heads are at right now, in samples, gliding toward what
    /// Time asks for — the way a tape machine's motor changes speed.
    delay_now: f32,
    /// Index into [`HEAD_MODES`].
    heads: usize,
    /// Index into [`super::sync::DIVISIONS`]; 0 is free-running Time.
    sync: usize,
    /// Output shelves on the effect sound, 0..1, 0.5 flat — the RE-201's Bass
    /// and Treble. Tone stays what it was: the hi-cut inside the loop.
    bass: f32,
    treble: f32,
    eq_lo: [OnePole; 2],
    eq_hi: [OnePole; 2],
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
        // **Sized for the highest rate choz supports, not for this one.** The
        // device can change under a running chain, and rebuilding the tape to
        // fit would be an allocation on the audio thread — which is the one
        // thing `FxProcessor` promises never happens. Buying the memory once
        // costs 2.4 MB of tape at 192 kHz and buys `retune`, which is
        // arithmetic.
        let cap = (MAX_DELAY_S * MAX_RATE) as usize + 4;
        // Spring: 3 allpass (prime-ish lengths) + 2 short combs, scaled to sr.
        let s = |n: usize| ((n as f32) * sr as f32 / 44100.0) as usize;
        let c = |n: usize| ((n as f32) * MAX_RATE / 44100.0) as usize + 4;
        let aps = vec![
            Allpass::new(c(225), s(225), 0.6),
            Allpass::new(c(556), s(556), 0.6),
            Allpass::new(c(341), s(341), 0.6),
        ];
        let combs = vec![
            Comb::new(c(1557), s(1557), 0.7),
            Comb::new(c(1116), s(1116), 0.7),
        ];
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
            tape_l: Tape::new(cap),
            tape_r: Tape::new(cap),
            aps,
            combs,
            wow_phase: 0.0,
            flutter_phase: 0.3,
            delay_now: (50.0 + time.clamp(0.0, 1.0) * 1450.0) / 1000.0 * sr as f32,
            heads: 6,
            sync: 0,
            bass: 0.5,
            treble: 0.5,
            eq_lo: [OnePole::new(); 2],
            eq_hi: [OnePole::new(); 2],
        }
    }

    /// Follow a sample-rate change. Arithmetic only — the buffers were sized
    /// for the highest rate at construction, so all that moves is how far into
    /// them the spring reads.
    fn retune(&mut self, sr: u32) {
        self.sample_rate = sr.max(8000);
        let s = |n: usize| ((n as f32) * self.sample_rate as f32 / 44100.0) as usize;
        for (ap, n) in self.aps.iter_mut().zip([225usize, 556, 341]) {
            ap.set_len(s(n));
        }
        for (comb, n) in self.combs.iter_mut().zip([1557usize, 1116]) {
            comb.set_len(s(n));
        }
        self.delay_now = self.delay_samps();
    }

    /// The longest head's delay, in samples. Synced, head 1 — a third of it —
    /// is the note value, so the three heads are one, two and three of them;
    /// held under the tape's two seconds.
    fn delay_samps(&self) -> f32 {
        let sr = self.sample_rate as f32;
        match super::sync::quarters(self.sync) {
            Some(q) => (super::sync::samples(q, sr) * 3.0).min((MAX_DELAY_S - 0.05) * sr),
            None => (50.0 + self.time.clamp(0.0, 1.0) * 1450.0) / 1000.0 * sr,
        }
    }
}

impl FxProcessor for SpaceEcho {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        if sample_rate != self.sample_rate {
            self.retune(sample_rate);
        }
        let sr = self.sample_rate as f32;
        let target = self.delay_samps();
        // ~80 ms glide: turning Time bends the pitch of what is on the tape,
        // as it does on the machine, instead of jumping the heads (a click).
        let glide = 1.0 - (-1.0 / (0.08 * sr)).exp();
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
        // ±12 dB shelves: 250 Hz and under, 3 kHz and over. At 0.5 the gain is
        // exactly 1 and the shelf adds exactly nothing.
        let db = |v: f32| 10f32.powf((v.clamp(0.0, 1.0) - 0.5) * 24.0 / 20.0);
        let (g_lo, g_hi) = (db(self.bass) - 1.0, db(self.treble) - 1.0);
        for (lo, hi) in self.eq_lo.iter_mut().zip(self.eq_hi.iter_mut()) {
            lo.set_hz(250.0, sr);
            hi.set_hz(3000.0, sr);
        }

        let fb = self.feedback.clamp(0.0, 1.0) * 1.1;
        let active = HEAD_MODES[self.heads.min(HEAD_MODES.len() - 1)].1;
        // The loop is divided by the gain of the heads that are on. The three
        // together read the tape back at up to 2.15× — fed back raw, that was
        // a loop that oscillated from Feedback 0.45 on new tape and 0.2 on
        // worn, where the knob promises it only past 1.0 — and normalising by
        // the ones on keeps Feedback meaning the same whichever of them play.
        let head_sum = HEAD_GAINS
            .iter()
            .zip(active)
            .filter(|(_, on)| *on)
            .map(|(g, _)| g)
            .sum::<f32>()
            .max(1e-6);
        let sat_drive = 1.0 + self.age * 1.5;
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

            // Capped at half a sample a sample — the tape never runs past
            // ±50 % of its speed — or a big turn of Time would be a squeal
            // twelve times the pitch, which is a click spread over 80 ms.
            self.delay_now += ((target - self.delay_now) * glide).clamp(-0.5, 0.5);
            let base = self.delay_now;
            // Sum the heads the mode selector has on.
            let mut echo_l = 0.0;
            let mut echo_r = 0.0;
            for ((ratio, gain), on) in HEAD_RATIOS.iter().zip(HEAD_GAINS.iter()).zip(active) {
                if on {
                    echo_l += self.tape_l.read(base * ratio + mod_l) * gain;
                    echo_r += self.tape_r.read(base * ratio + mod_r) * gain;
                }
            }

            // Feedback colour: HP → LP(age) → tanh(age), then back into the tape.
            // The saturator is unity at low level (`tanh(d·x)/d`): age brings
            // the clipping in sooner, it does not add gain to the loop. With
            // the head sum normalised the loop gain is `fb` at most, so it
            // only runs away past Feedback ≈ 0.91, as documented.
            let sat = |x: f32| (x * sat_drive).tanh() / sat_drive;
            let col_l = sat(self.tape_l.lp.lp(self.tape_l.hp.hp(echo_l / head_sum)));
            let col_r = sat(self.tape_r.lp.lp(self.tape_r.hp.hp(echo_r / head_sum)));
            self.tape_l.write(dry_l + col_l * fb);
            self.tape_r.write(dry_r + col_r * fb);

            // Spring reverb on the input **and** the echoes: the RE-201's
            // spring hears what comes in, which is what makes its REV-only
            // position possible — with only the echoes feeding it, no heads
            // meant no reverb.
            let mut spr = (echo_l + echo_r + dry_l + dry_r) * 0.5;
            for ap in self.aps.iter_mut() {
                spr = ap.process(spr);
            }
            let mut tail = 0.0;
            for cb in self.combs.iter_mut() {
                tail += cb.process(spr);
            }
            tail *= 0.5;

            let mut wet = [echo_l + tail * self.spring, echo_r + tail * self.spring];
            for (ch, w) in wet.iter_mut().enumerate() {
                let low = self.eq_lo[ch].lp(*w);
                let high = *w - self.eq_hi[ch].lp(*w);
                *w += g_lo * low + g_hi * high;
            }
            let [wet_l, wet_r] = wet;
            buf[i * 2] = dry_l + self.wet * (wet_l - dry_l);
            buf[i * 2 + 1] = dry_r + self.wet * (wet_r - dry_r);
        }
    }

    /// Silence, in place: rebuilding allocated ~3 MB of tape and dropped the
    /// Wet back to its default.
    fn reset(&mut self) {
        for tape in [&mut self.tape_l, &mut self.tape_r] {
            tape.buf.fill(0.0);
            tape.hp.z = 0.0;
            tape.lp.z = 0.0;
        }
        for ap in self.aps.iter_mut() {
            ap.buf.fill(0.0);
        }
        for comb in self.combs.iter_mut() {
            comb.buf.fill(0.0);
            comb.damp.z = 0.0;
        }
        for f in self.eq_lo.iter_mut().chain(self.eq_hi.iter_mut()) {
            f.z = 0.0;
        }
        self.delay_now = self.delay_samps();
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
            P::new("Heads", self.heads as f32 / 7.0, 0.0, 1.0, ""),
            P::new("Sync", super::sync::norm_of(self.sync), 0.0, 1.0, ""),
            P::new("Bass", self.bass, -12.0, 12.0, "dB"),
            P::new("Treble", self.treble, -12.0, 12.0, "dB"),
        ]
    }

    /// Live param update — preserves the tape buffer + reverb tail (no rebuild).
    /// Time glides (see `delay_now`), so moving it bends rather than clicks.
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
            8 => self.heads = (v * 7.0).round() as usize,
            9 => self.sync = super::sync::index_of(v),
            10 => self.bass = v,
            11 => self.treble = v,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A sample-rate change must not ask the audio thread for memory.**
    ///
    /// It used to do `*self = SpaceEcho::new(...)` inside `process_block`,
    /// which reallocates the tape and the whole spring network on the callback
    /// — the one thing `FxProcessor` promises never happens. The buffers are
    /// sized for the highest rate now and a change only moves the read lengths.
    ///
    /// Checked at the mechanism rather than with a counting allocator: if any
    /// buffer had been rebuilt, it would be at a different address. A global
    /// allocator would prove the same thing and slow every other test in the
    /// binary to do it.
    #[test]
    fn a_rate_change_does_not_allocate() {
        let mut fx = SpaceEcho::new(48_000, 0.4, 0.5, 0.3, 0.3, 0.4, 0.3, 0.5);
        let mut buf = vec![0.1f32; 512];
        fx.process_block(&mut buf, 48_000);

        let fingerprint = |fx: &SpaceEcho| {
            let mut v = vec![
                (fx.tape_l.buf.as_ptr() as usize, fx.tape_l.buf.len()),
                (fx.tape_r.buf.as_ptr() as usize, fx.tape_r.buf.len()),
            ];
            v.extend(
                fx.aps
                    .iter()
                    .map(|a| (a.buf.as_ptr() as usize, a.buf.len())),
            );
            v.extend(
                fx.combs
                    .iter()
                    .map(|c| (c.buf.as_ptr() as usize, c.buf.len())),
            );
            v
        };
        let before = fingerprint(&fx);
        for sr in [96_000u32, 44_100, 192_000, 48_000] {
            fx.process_block(&mut buf, sr);
            assert_eq!(
                fingerprint(&fx),
                before,
                "{sr} Hz moved a buffer, so it allocated one"
            );
        }
        assert!(buf.iter().all(|s| s.is_finite()), "and it stayed finite");
    }

    /// The spring still rings the same way after the device changes — the read
    /// lengths follow the rate, which is what `retune` is for.
    #[test]
    fn the_spring_follows_the_sample_rate() {
        for sr in [44_100u32, 48_000, 96_000, 192_000] {
            let mut a = SpaceEcho::new(sr, 0.3, 0.4, 0.0, 0.0, 0.3, 0.9, 0.5);
            let mut b = SpaceEcho::new(48_000, 0.3, 0.4, 0.0, 0.0, 0.3, 0.9, 0.5);
            b.process_block(&mut [0.0; 64], 48_000);
            b.process_block(&mut [0.0; 64], sr); // retuned rather than rebuilt

            let mut buf_a = vec![0.0f32; sr as usize / 2 * 2];
            let mut buf_b = buf_a.clone();
            buf_a[0] = 1.0;
            buf_a[1] = 1.0;
            buf_b[0] = 1.0;
            buf_b[1] = 1.0;
            a.process_block(&mut buf_a, sr);
            b.process_block(&mut buf_b, sr);
            let rms = |x: &[f32]| (x.iter().map(|s| s * s).sum::<f32>()).sqrt();
            let (ea, eb) = (rms(&buf_a), rms(&buf_b));
            assert!(
                eb > ea * 0.5 && eb < ea * 2.0,
                "{sr} Hz: rebuilt {ea:.4}, retuned {eb:.4}"
            );
        }
    }

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

    /// Below the top of its travel the echo dies away, on new tape and worn.
    /// It used to grow on its own from Feedback 0.45 (new) and 0.2 (half
    /// worn) — most of the knob was a runaway.
    #[test]
    fn the_echo_dies_away_below_full_feedback() {
        for age in [0.0f32, 0.5, 1.0] {
            let mut fx = SpaceEcho::new(48_000, 0.2, 0.75, 0.0, 0.0, age, 0.0, 1.0);
            fx.set_mix(1.0);
            let mut peaks = Vec::new();
            for s in 0..8 {
                let mut b = vec![0.0f32; 96_000];
                if s == 0 {
                    for i in 0..480 {
                        let v = 0.3 * (i as f32 * 0.1).sin();
                        b[i * 2] = v;
                        b[i * 2 + 1] = v;
                    }
                }
                fx.process_block(&mut b, 48_000);
                peaks.push(b.iter().fold(0.0f32, |m, x| m.max(x.abs())));
            }
            assert!(
                peaks[7] < peaks[1] * 0.1,
                "age {age}: still ringing after 8 s: {peaks:?}"
            );
        }
    }

    /// Past 0.91 it may sing on its own — that is what the top of the knob is
    /// for — but the saturator holds it.
    #[test]
    fn full_feedback_sings_but_stays_bounded() {
        let mut fx = SpaceEcho::new(48_000, 0.2, 1.0, 0.3, 0.3, 0.5, 0.5, 1.0);
        fx.set_mix(1.0);
        let mut b: Vec<f32> = (0..96_000)
            .map(|i| 0.5 * ((i / 2) as f32 * 0.03).sin())
            .collect();
        let mut peak = 0.0f32;
        for _ in 0..10 {
            fx.process_block(&mut b, 48_000);
            assert!(b.iter().all(|x| x.is_finite()));
            peak = b.iter().fold(peak, |m, x| m.max(x.abs()));
            b.iter_mut().for_each(|x| *x = 0.0);
        }
        assert!(peak < 3.0, "peak {peak}");
    }

    /// Moving Time glides the heads: no sample-to-sample jump in the output
    /// bigger than the signal itself makes.
    #[test]
    fn turning_time_does_not_click() {
        let mut fx = SpaceEcho::new(48_000, 0.2, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0);
        fx.set_mix(1.0);
        let tone = |k: usize| -> Vec<f32> {
            (0..4096)
                .map(|i| 0.5 * (((i / 2) + k * 2048) as f32 * 0.02).sin())
                .collect()
        };
        for k in 0..30 {
            fx.process_block(&mut tone(k), 48_000);
        }
        fx.set_param(0, 0.9);
        let mut worst = 0.0f32;
        let mut prev: Option<f32> = None;
        for k in 30..40 {
            let mut b = tone(k);
            fx.process_block(&mut b, 48_000);
            for x in b.iter().step_by(2) {
                if let Some(p) = prev {
                    worst = worst.max((x - p).abs());
                }
                prev = Some(*x);
            }
        }
        // A 0.5 sine at 0.02 rad/sample moves ≤ 0.01 a sample; the three
        // heads summed, ≤ ~0.03. A jump of the heads is an order more.
        assert!(worst < 0.1, "biggest step {worst}");
    }

    /// The mode selector picks the heads, and its last position is the
    /// spring alone — which only works because the spring hears the input.
    #[test]
    fn the_mode_selector_picks_heads_and_rev_is_spring_only() {
        let echo_at = |mode: f32| {
            let mut fx = SpaceEcho::new(48_000, 0.2, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0);
            fx.set_param(8, mode);
            fx.set_mix(1.0);
            let mut b = vec![0.0f32; 96_000];
            b[0] = 1.0;
            b[1] = 1.0;
            fx.process_block(&mut b, 48_000);
            b
        };
        // Time 0.2 → 340 ms; head 1 (the shortest) is a third of it: 113 ms.
        let near = |b: &[f32], ms: f32| {
            let at = (ms * 48.0) as usize * 2;
            b[at - 40..at + 40]
                .iter()
                .fold(0.0f32, |m, x| m.max(x.abs()))
        };
        let one = echo_at(0.0);
        assert!(
            near(&one, 113.0) > 0.1 && near(&one, 340.0) < 1e-3,
            "mode 1 is head 1 alone"
        );
        let three = echo_at(2.0 / 7.0);
        assert!(
            near(&three, 113.0) < 1e-3 && near(&three, 340.0) > 0.1,
            "mode 3 is head 3 alone"
        );

        let mut fx = SpaceEcho::new(48_000, 0.2, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0);
        fx.set_param(8, 1.0); // REV
        fx.set_mix(1.0);
        let mut b = vec![0.0f32; 96_000];
        b[0] = 1.0;
        b[1] = 1.0;
        fx.process_block(&mut b, 48_000);
        assert!(near(&b, 340.0) < 0.05, "no echo in REV");
        assert!(
            b[2_000..40_000].iter().any(|x| x.abs() > 1e-3),
            "but a spring"
        );
    }

    /// Synced to 1/8 at 120 bpm, head 1 is 250 ms and head 3 is 750.
    #[test]
    fn sync_puts_the_heads_on_the_grid() {
        let _g = crate::test_locks::transport();
        let t = choz_ports::transport();
        let old = t.bpm();
        t.set_bpm(120.0);
        let mut fx = SpaceEcho::new(48_000, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0);
        let eighth = super::super::sync::DIVISIONS
            .iter()
            .position(|(_, n)| *n == "1/8")
            .unwrap();
        fx.set_param(9, super::super::sync::norm_of(eighth));
        fx.set_param(8, 0.0); // head 1 alone
        fx.reset(); // land on the synced time instead of gliding there
        fx.set_mix(1.0);
        let mut b = vec![0.0f32; 96_000];
        b[0] = 1.0;
        b[1] = 1.0;
        fx.process_block(&mut b, 48_000);
        t.set_bpm(old);
        let at = 250 * 48 * 2;
        let peak = b[at - 80..at + 80]
            .iter()
            .fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(peak > 0.1, "head 1 is not at 250 ms: {peak}");
    }

    /// Bass and Treble at the middle change nothing; turned up, they lift
    /// their end of the echo.
    #[test]
    fn bass_and_treble_shelve_the_echo() {
        let run = |bass: f32, treble: f32, hz: f32| {
            let mut fx = SpaceEcho::new(48_000, 0.1, 0.3, 0.0, 0.0, 0.0, 0.0, 1.0);
            fx.set_param(10, bass);
            fx.set_param(11, treble);
            fx.set_mix(1.0);
            let mut b: Vec<f32> = (0..96_000)
                .map(|i| 0.2 * (std::f32::consts::TAU * hz * (i / 2) as f32 / 48_000.0).sin())
                .collect();
            fx.process_block(&mut b, 48_000);
            b[48_000..].iter().map(|x| x * x).sum::<f32>()
        };
        assert!(
            run(1.0, 0.5, 80.0) > run(0.5, 0.5, 80.0) * 4.0,
            "bass lifts the lows"
        );
        assert!(
            run(0.5, 1.0, 8000.0) > run(0.5, 0.5, 8000.0) * 4.0,
            "treble lifts the highs"
        );
        assert!((run(0.5, 0.5, 80.0) - run(0.5, 0.5, 80.0)).abs() < 1e-9);
    }

    #[test]
    fn space_echo_reset_clears_tail() {
        let mut fx = SpaceEcho::new(48000, 0.3, 0.7, 0.2, 0.2, 0.3, 0.3, 0.5);
        let mut block = vec![0.5f32; 256];
        fx.process_block(&mut block, 48000);
        fx.set_mix(0.3);
        let ptr = fx.tape_l.buf.as_ptr() as usize;
        fx.reset();
        assert!(fx.tape_l.buf.iter().all(|&v| v == 0.0));
        assert_eq!(
            ptr,
            fx.tape_l.buf.as_ptr() as usize,
            "rebuilt, so allocated"
        );
        assert_eq!(fx.wet, 0.3);
    }
}
