//! Harmoniser: up to eight transposed copies of what is played, in tune with
//! each other and with the key.
//!
//! ```text
//!            ┌─ voice 1 ── shift ── detune ── delay ── pan ─┐
//!  in ─┬─────┼─ voice 2 ── shift ── detune ── delay ── pan ─┼─► wet
//!      │     └─ …up to 8 …                                  │
//!      └──────────────────────────────────────────────────► dry
//! ```
//!
//! # What each part is for
//!
//! * **Voices** — one is a transposer, two is the classic double, four and
//!   eight are the stacked-harmoniser sound. Each has its own interval.
//! * **Diatonic**, and this is what makes it musical rather than parallel:
//!   with a key and a scale set, an interval is a **number of scale steps**,
//!   not a fixed number of semitones. A third above a C in C major is E (four
//!   semitones); above a D it is F (three). Shifting everything by a constant
//!   is the sound of a cheap pitch shifter, and it is wrong in exactly the
//!   places a listener notices.
//! * **Micro-pitch** — a few cents of detune spread across the voices. Two
//!   copies at exactly the same pitch are one louder copy; a few cents apart
//!   they are two singers.
//! * **Delay** — staggered per voice. A harmony that arrives at the same
//!   instant is a chorus; a few tens of milliseconds later it is a second
//!   person.
//! * **Envelope follower** — the voices open with the input rather than
//!   sitting there, so a harmoniser on a mic does not sing through the gaps.
//!
//! # What it does not do, and why
//!
//! **The intervals do not come from MIDI.** An [`super::FxProcessor`] is handed
//! audio and nothing else — there is no note input in an FX chain, by design
//! (see the trait). Chord-driven harmony belongs in the input-algorithm
//! section, where notes exist; the roadmap has it. What is here is the part
//! that works with only a signal: a key, a scale, and intervals.

use super::shift::VoiceShifter;
use super::smooth::Smoothed;
use crate::fx::autotune::{Scale, ScaleType, NOTE_NAMES};
use crate::fx::vocoder::Vocoder;
use crate::fx::FxProcessor as _;

/// What the effect does with the chord it is given.
///
/// **The vocoder lives here too.** It was a separate effect and it should not
/// have been: both answer the same question — "what should the voice be sung
/// *on*" — and both read the same held chord. As two effects they needed two
/// MIDI inputs, two dry/wets and two places to look; as one, `MODE` is the only
/// thing that differs, and `Carrier::Chord` is the setting that makes the
/// vocoder a harmoniser with a different voice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Pitch-shifted voices: the harmony sings the notes.
    #[default]
    Harmony,
    /// A band vocoder: the voice's shape on the chord's sound.
    Vocoder,
}

impl Mode {
    pub const ALL: [Mode; 2] = [Mode::Harmony, Mode::Vocoder];

    pub fn label(self) -> &'static str {
        match self {
            Mode::Harmony => "HARMONY",
            Mode::Vocoder => "VOCODER",
        }
    }

    pub fn from_norm(v: f32) -> Self {
        match v >= 0.5 {
            true => Mode::Vocoder,
            false => Mode::Harmony,
        }
    }
}

/// The most voices, and the width of everything sized per voice.
pub const MAX_VOICES: usize = 8;

/// How the voices are spread out, as scale steps (or semitones when there is
/// no scale) from the note being played.
///
/// Named shapes rather than eight interval knobs: eight knobs is a matrix, and
/// the shapes below are what people actually stack. The list is read in order
/// and truncated to the voice count, so two voices of `Thirds` are the first
/// two of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Shape {
    /// A third and a fifth above, then their octaves: the standard stack.
    Thirds,
    /// Fifths and octaves — open, and the safest against a wrong key.
    Fifths,
    /// Octaves only, up and down. No key needed to be right.
    Octaves,
    /// Above and below in pairs: the "two more singers" shape.
    Above,
    /// Everything below the note, for weight.
    Below,
    /// Tight, for the chorus-of-one sound rather than a chord.
    Cluster,
    /// Third, fifth and seventh: a major seventh over the note being sung, and
    /// the shape a harmoniser is reached for. The default, because two voices
    /// of thirds is the safe answer and this is the one people want to hear.
    #[default]
    Maj7,
}

impl Shape {
    pub const ALL: [Shape; 7] = [
        Shape::Thirds,
        Shape::Fifths,
        Shape::Octaves,
        Shape::Above,
        Shape::Below,
        Shape::Cluster,
        Shape::Maj7,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Shape::Thirds => "3rds",
            Shape::Fifths => "5ths",
            Shape::Octaves => "OCT",
            Shape::Above => "ABOVE",
            Shape::Below => "BELOW",
            Shape::Cluster => "CLUSTER",
            Shape::Maj7 => "MAJ7",
        }
    }

    /// The interval each voice takes, in **scale steps** when a scale is in
    /// use and in semitones when it is not. Eight of them; the voice count
    /// decides how many are read.
    pub fn steps(self) -> [i32; MAX_VOICES] {
        match self {
            Shape::Thirds => [2, 4, -3, 7, 9, -7, 11, 14],
            Shape::Fifths => [4, -4, 7, 11, -7, 14, 18, -11],
            // Octaves are octaves in any scale: seven steps is one, and it is
            // the one shape that cannot be out of key.
            Shape::Octaves => [7, -7, 14, -14, 7, -7, 21, -21],
            Shape::Above => [2, 4, 6, 8, 10, 12, 14, 16],
            Shape::Below => [-2, -4, -6, -7, -9, -11, -14, -16],
            Shape::Cluster => [1, -1, 2, -2, 3, -3, 4, -4],
            // Scale steps, so in a major key these are the major third, the
            // fifth and the major seventh, then the same chord an octave up.
            Shape::Maj7 => [2, 4, 6, 9, 11, 13, -3, -5],
        }
    }

    pub fn to_norm(self) -> f32 {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0) as f32 / (Self::ALL.len() - 1) as f32
    }

    pub fn from_norm(v: f32) -> Self {
        let n = Self::ALL.len();
        let i = (v.clamp(0.0, 1.0) * (n - 1) as f32).round() as usize;
        Self::ALL[i.min(n - 1)]
    }
}

/// Voice counts the knob steps through. Not 1..8 continuously: three voices
/// and five voices are not sounds anybody asks for, and a stepped knob says
/// what it will do before it is turned.
pub const VOICE_COUNTS: [usize; 4] = [1, 2, 4, 8];

/// The longest a voice can lag, in samples: 50 ms at 96 kHz, which is the
/// delay knob's top at the highest rate choz opens.
const MAX_DELAY: usize = 4800;

struct Voice {
    shifter: VoiceShifter,
    /// What this voice is actually transposed by, in semitones. Kept rather
    /// than recomputed: with a chord driving the harmony there is nothing to
    /// recompute it *from*, and [`Harmonizer::intervals`] used to answer with
    /// the shape's intervals whatever the voices were really doing.
    semitones: f32,
    level: f32,
    /// Constant-power pan, precomputed.
    gain: [f32; 2],
    delay_frames: f32,
    /// The voice's own delay line, **after** the shift.
    ///
    /// It used to reuse the shifter's input line (`VoiceShifter::tap`), which
    /// cost no memory and was wrong: that line holds the signal *before* it is
    /// transposed, so every voice with a delay came out at the original pitch.
    /// With the two-voice default that meant half the harmony was a slapback of
    /// the input — measured as the fifth sitting 42 dB under the dry, which is
    /// "the harmoniser does nothing" to anybody listening.
    delay: Vec<f32>,
    write: usize,
}

impl Voice {
    fn new() -> Self {
        Self {
            shifter: VoiceShifter::new(),
            semitones: 0.0,
            level: 1.0,
            gain: [std::f32::consts::FRAC_1_SQRT_2; 2],
            delay_frames: 0.0,
            delay: vec![0.0; MAX_DELAY],
            write: 0,
        }
    }

    /// Push one shifted sample in and take out what is due, interpolated
    /// between the two samples the fractional delay falls between.
    #[inline]
    fn delayed(&mut self, x: f32) -> f32 {
        let len = self.delay.len();
        self.delay[self.write] = x;
        self.write = (self.write + 1) % len;
        if self.delay_frames < 1.0 {
            return x;
        }
        let back = self.delay_frames.min((len - 2) as f32);
        let whole = back.floor();
        let frac = back - whole;
        let i = (self.write + len - whole as usize - 1) % len;
        let j = (i + len - 1) % len;
        self.delay[i] * (1.0 - frac) + self.delay[j] * frac
    }
}

pub struct Harmonizer {
    voices: Vec<Voice>,
    count: usize,
    shape: Shape,
    /// Key and scale. `ScaleType::Chromatic` **is** "no key": every semitone
    /// belongs to it, so walking a step is walking a semitone and the harmony
    /// comes out parallel. One representation, not an `Option` beside an enum
    /// that already has the case.
    scale: Scale,
    key: u8,
    kind: ScaleType,
    /// Follow the notes held on a MIDI keyboard instead of the shape and key.
    ///
    /// Off by default, and off is what every project written before it says.
    /// When on, the harmony is **the chord being played**: the lowest held note
    /// is the root and the ones above it are the intervals, so a musician plays
    /// the harmony rather than describing it.
    midi: bool,
    /// Which MIDI channel that keyboard is on, 1..16. Read by the interface,
    /// which is the only thing here that can see a MIDI port.
    midi_channel: u8,
    /// The chord generation this was last built from, so a rebuild only happens
    /// when the hand on the keyboard moves.
    chord_seen: u32,
    /// Cents of detune spread across the voices.
    detune: f32,
    /// Milliseconds the last voice lags by; the rest are spread under it.
    delay_ms: f32,
    /// How much the input's envelope opens the voices, 0..1.
    env_amount: f32,
    env: Smoothed,
    /// A slow peak of the same signal, so the envelope above is read as **how
    /// loud this is compared with how loud it has been** rather than against an
    /// absolute number.
    ///
    /// The absolute version is what made the harmoniser collapse on a quiet
    /// source: a headset microphone sits around -30 dBFS, the gate opened on a
    /// fixed 0.25, and the voices never came past half open however hard
    /// somebody sang. A follower has to follow the singer, not the meter.
    peak: f32,
    width: f32,
    /// Which of the two things this effect is right now, and the vocoder it
    /// keeps for when it is the other one. Built either way: switching mode
    /// mid-song must not stop to build a filter bank.
    mode: Mode,
    voc: Vocoder,
    /// What the panned voices have to be multiplied by for full wet to come out
    /// as loud as the dry. Computed in [`Harmonizer::rebuild`] from the pans it
    /// just assigned — see there for why it is not a constant.
    makeup: f32,
    mix: f32,
    sample_rate: f32,
    dirty: bool,
}

impl Harmonizer {
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate.max(8000) as f32;
        let mut h = Self {
            voices: (0..MAX_VOICES).map(|_| Voice::new()).collect(),
            count: 2,
            shape: Shape::default(),
            scale: Scale::new(0, ScaleType::Major),
            key: 0,
            kind: ScaleType::Major,
            midi: false,
            midi_channel: 1,
            chord_seen: 0,
            detune: 8.0,
            delay_ms: 18.0,
            env_amount: 0.5,
            // 40 ms: opens with a syllable, not with a waveform.
            env: Smoothed::new(0.0, 40.0, sr),
            peak: 0.0,
            width: 1.0,
            mode: Mode::default(),
            voc: Vocoder::new(sample_rate),
            makeup: 1.0,
            mix: 0.5,
            sample_rate: sr,
            dirty: true,
        };
        h.rebuild();
        h
    }

    /// Build from the rack's knob positions: voices, shape, key, scale,
    /// detune, delay, env, width.
    pub fn with_params(sample_rate: u32, p: &[f32]) -> Self {
        let get = |i: usize, d: f32| p.get(i).copied().unwrap_or(d);
        let mut h = Self::new(sample_rate);
        h.set_voices(get(0, 0.334));
        h.shape = Shape::from_norm(get(1, 0.0));
        h.set_key(get(2, 0.0));
        h.set_scale(get(3, 0.0));
        h.detune = get(4, 0.32) * 25.0;
        h.delay_ms = get(5, 0.36) * 50.0;
        h.env_amount = get(6, 0.5).clamp(0.0, 1.0);
        h.width = get(7, 1.0).clamp(0.0, 1.0);
        // 8 is the dry/wet, which **was not read here** — so every rebuild of
        // the chain (adding another effect, reopening a project) put the knob
        // back to half whatever it had been set to.
        h.mix = get(8, 0.5).clamp(0.0, 1.0);
        h.midi = get(9, 0.0) >= 0.5;
        h.set_midi_channel(get(10, 0.0));
        // 11 onwards is the vocoder half: the mode, then its own knobs.
        // **Appended, not interleaved** — every index above is where it was
        // before the two effects became one, so a project written against the
        // old harmoniser opens with its knobs still on their own controls.
        h.mode = Mode::from_norm(get(11, 0.0));
        h.voc = Vocoder::with_params(sample_rate, &p[VOC_PARAM0.min(p.len())..]);
        h.voc.set_mix(1.0);
        h.rebuild();
        h
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// 1..16, from a knob position.
    pub fn set_midi_channel(&mut self, v: f32) {
        self.midi_channel = 1 + (v.clamp(0.0, 1.0) * 15.0).round() as u8;
    }

    /// Which MIDI channel this harmoniser listens to, and whether it listens at
    /// all. The interface reads both: it is the side that can see a keyboard.
    pub fn midi_input(&self) -> Option<u8> {
        self.midi.then_some(self.midi_channel)
    }

    pub fn set_voices(&mut self, v: f32) {
        let n = VOICE_COUNTS.len();
        let i = (v.clamp(0.0, 1.0) * (n - 1) as f32).round() as usize;
        self.count = VOICE_COUNTS[i.min(n - 1)];
        self.dirty = true;
    }

    pub fn voices(&self) -> usize {
        self.count
    }

    pub fn set_key(&mut self, v: f32) {
        self.key = (v.clamp(0.0, 1.0) * 11.0).round() as u8;
        self.dirty = true;
    }

    /// The scale, chromatic first — and chromatic is "no key": the setting for
    /// material that has none, and where a harmoniser should be parked rather
    /// than left singing wrong notes in the wrong one.
    pub fn set_scale(&mut self, v: f32) {
        let all = ScaleType::ALL;
        let i = (v.clamp(0.0, 1.0) * (all.len() - 1) as f32).round() as usize;
        self.kind = all[i.min(all.len() - 1)];
        self.scale = Scale::new(self.key, self.kind);
        self.dirty = true;
    }

    /// What each voice is transposed by right now, in semitones — for a test
    /// or a display, and the one number that says whether the harmony is
    /// diatonic or parallel.
    pub fn intervals(&self) -> Vec<f32> {
        self.voices
            .iter()
            .take(self.count)
            .map(|v| v.semitones)
            .collect()
    }

    /// A voice's shift in semitones.
    ///
    /// With a scale, the step is a **scale step**: walk that many degrees from
    /// the root and take the distance. Without one it is a semitone count, and
    /// the harmony is parallel — which is a sound, just not a musical one.
    fn semitones_for(&self, step: i32, voice: usize) -> f32 {
        // The scale is a set of pitch classes; a step is a move through it.
        // Counting the degrees keeps a third a third whatever degree it starts
        // on, which is the whole point — and under a chromatic scale every
        // semitone is a degree, so this reduces to `step` on its own.
        let root = self.key as i32 + 60;
        let mut note = root;
        let mut left = step.abs();
        let dir = step.signum();
        while left > 0 {
            note += dir;
            if self.scale.contains(note) {
                left -= 1;
            }
            // Safety: a scale with no notes at all would loop for ever.
            if (note - root).abs() > 96 {
                break;
            }
        }
        let base = (note - root) as f32;
        // Micro-pitch: the voices fan out around the note rather than all
        // sitting a fixed distance off it, so an odd voice count is not
        // lopsided.
        let spread = if self.count > 1 {
            (voice as f32 / (self.count - 1) as f32) * 2.0 - 1.0
        } else {
            0.0
        };
        base + spread * self.detune / 100.0
    }

    /// Recompute what each voice does. Off the audio path: this walks scales
    /// and calls `powf`, and none of that belongs in a callback.
    fn rebuild(&mut self) {
        self.scale = Scale::new(self.key, self.kind);
        // With MIDI on, the chord replaces the shape: the intervals are the
        // ones being held, measured from the lowest note. Nothing held leaves
        // the last chord standing — a harmoniser that stops the moment the
        // hands come off the keys is one nobody can play.
        let mut held = [0u8; crate::chord::MAX_NOTES];
        let chord = match self.midi {
            true => crate::chord::chord().read(&mut held),
            false => 0,
        };
        let steps = self.shape.steps();
        let count = match chord > 1 {
            true => (chord - 1).min(MAX_VOICES),
            false => self.count,
        };
        let width = self.width;
        let delay_ms = self.delay_ms;
        let sr = self.sample_rate;
        // Level so eight voices are not eight times one voice.
        //
        // **`1/√n`, not `1/n`.** The voices sing *different notes*, so they are
        // uncorrelated and their powers add, not their amplitudes — dividing by
        // `n` took another 3 dB off two voices and 9 dB off eight. Measured
        // against a 220 Hz tone with the two-voice default: full wet came out
        // **7.2 dB under the dry**, which is a harmony that is technically
        // there and practically inaudible, and is what "the harmoniser does
        // nothing" turned out to mean.
        let level = 1.0 / (count as f32).max(1.0).sqrt();
        for i in 0..MAX_VOICES {
            // Chord: the interval from the root to the i-th note above it, in
            // semitones and needing no scale at all — the hand already chose
            // the notes. Otherwise the shape, walked through the key.
            let semis = match chord > 1 && i + 1 < chord {
                true => (held[i + 1] as i32 - held[0] as i32) as f32,
                false => self.semitones_for(steps[i.min(steps.len() - 1)], i),
            };
            let voice = &mut self.voices[i];
            voice.shifter.set_semitones(semis);
            voice.semitones = semis;
            voice.level = level;
            // Fanned across the image, and the odd one in the middle.
            let pan = if count > 1 {
                ((i as f32 / (count - 1) as f32) * 2.0 - 1.0) * width
            } else {
                0.0
            };
            let angle = (pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
            voice.gain = [angle.cos(), angle.sin()];
            // Staggered: the first voice is nearly on the note, the last one is
            // the full delay behind it.
            let share = if count > 1 {
                i as f32 / (count - 1) as f32
            } else {
                0.0
            };
            voice.delay_frames = (delay_ms * share * 0.001 * sr).clamp(0.0, (MAX_DELAY - 2) as f32);
        }
        // **What the pans cost, given back.** Two voices at full width sit hard
        // left and hard right, so each channel carries exactly one of them at
        // `1/sqrt(2)` — the wet arrives 3 dB under the dry before the shifter
        // has taken its own cut, and a harmony 5 dB down is one that disappears
        // into the track. Rather than a constant fudge, the loss is read off
        // the pans that were just assigned: each channel is normalised to the
        // power it would have had unpanned, so width and voice count can move
        // without changing how loud the effect is.
        let power = |ch: usize| -> f32 {
            self.voices
                .iter()
                .take(count)
                .map(|v| (v.level * v.gain[ch]).powi(2))
                .sum::<f32>()
        };
        let loudest = power(0).max(power(1)).max(1e-6);
        // Capped: a single hard-panned voice would otherwise ask for infinite
        // makeup on the silent side.
        self.makeup = (1.0 / loudest.sqrt()).clamp(1.0, 4.0);
        self.dirty = false;
        self.chord_seen = crate::chord::chord().generation();
        // The voice count follows the chord while it is driving.
        if chord > 1 {
            self.count = count;
        }
    }
}

/// Where the vocoder's own knobs start in the merged parameter list, and how
/// many of them there are — its dry/wet is not one of them: the merged effect
/// has one, and it is `Wet` above.
pub const VOC_PARAM0: usize = 12;
pub const VOC_PARAMS: usize = 6;

impl super::FxProcessor for Harmonizer {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        if self.mode == Mode::Vocoder {
            // The vocoder owns the whole block in this mode, dry/wet and all:
            // two mixes in series would be a mix nobody can predict.
            self.voc.set_mix(self.mix);
            self.voc.process_block(buf, sample_rate);
            return;
        }
        let sr = sample_rate.max(8000) as f32;
        if (sr - self.sample_rate).abs() > 0.5 {
            self.sample_rate = sr;
            self.env.set_sample_rate(sr);
            self.dirty = true;
        }
        // A hand that moved on the keyboard is a rebuild, and only that: the
        // generation is one relaxed load per block.
        if self.midi && crate::chord::chord().generation() != self.chord_seen {
            self.dirty = true;
        }
        if self.dirty {
            self.rebuild();
        }
        let count = self.count;
        let mix = self.mix;
        let makeup = self.makeup;
        let env_amount = self.env_amount;
        // Two seconds to fall by 1/e, as a per-sample coefficient.
        let peak_decay = (-1.0 / (2.0 * sr)).exp();

        for frame in buf.as_chunks_mut::<2>().0 {
            let (dry_l, dry_r) = (frame[0], frame[1]);
            // One voice in, one signal to harmonise: a harmoniser fed a stereo
            // pair would be transposing two different signals into one chord.
            let mono = (dry_l + dry_r) * 0.5;

            let level = mono.abs().min(1.0);
            self.env.set_target(level);
            let e = self.env.tick();
            // The slow peak: straight up, and a couple of seconds to fall. It
            // is the reference the fast envelope is read against.
            self.peak = if level > self.peak {
                level
            } else {
                self.peak * peak_decay
            };
            // How open the voices are: the fast envelope as a **fraction of how
            // loud this signal has been**, so a quiet microphone opens them as
            // wide as a hot line does. Half of the recent peak counts as fully
            // open — a syllable is well past that, a gap between them is not.
            let reference = self.peak.max(1e-4) * 0.5;
            let open = 1.0 - env_amount + env_amount * (e / reference).min(1.0);

            let mut wet = [0.0f32; 2];
            for voice in self.voices.iter_mut().take(count) {
                // Every voice is written every sample even when its delay is
                // zero: the line has to keep moving or the tap reads the past
                // of a stopped clock.
                // Shift first, delay second. The other order is what made
                // every delayed voice come out at the original pitch.
                let shifted = voice.shifter.process(mono);
                let sound = voice.delayed(shifted);
                let g = sound * voice.level * open;
                wet[0] += g * voice.gain[0];
                wet[1] += g * voice.gain[1];
            }

            let (wet_l, wet_r) = (wet[0] * makeup, wet[1] * makeup);
            frame[0] = dry_l + mix * (wet_l - dry_l);
            frame[1] = dry_r + mix * (wet_r - dry_r);
        }
    }

    fn reset(&mut self) {
        self.voc.reset();
        for v in self.voices.iter_mut() {
            v.shifter.reset();
            v.delay.fill(0.0);
            v.write = 0;
        }
        self.env.snap(0.0);
        self.peak = 0.0;
    }

    fn set_mix(&mut self, wet: f32) {
        self.mix = wet.clamp(0.0, 1.0);
    }

    fn name(&self) -> &str {
        "Harmonizer"
    }

    fn params(&self) -> Vec<crate::fx::FxParam> {
        use crate::fx::FxParam;
        let voice_norm = VOICE_COUNTS
            .iter()
            .position(|c| *c == self.count)
            .unwrap_or(1) as f32
            / (VOICE_COUNTS.len() - 1) as f32;
        let scale_norm = ScaleType::ALL
            .iter()
            .position(|s| *s == self.kind)
            .unwrap_or(0) as f32
            / (ScaleType::ALL.len() - 1) as f32;
        vec![
            FxParam::new("Voices", voice_norm, 1.0, MAX_VOICES as f32, ""),
            FxParam::new("Shape", self.shape.to_norm(), 0.0, 1.0, ""),
            FxParam::new("Key", self.key as f32 / 11.0, 0.0, 11.0, ""),
            FxParam::new("Scale", scale_norm, 0.0, 1.0, ""),
            FxParam::new("Detune", self.detune / 25.0, 0.0, 25.0, "ct"),
            FxParam::new("Delay", self.delay_ms / 50.0, 0.0, 50.0, "ms"),
            FxParam::new("Env", self.env_amount, 0.0, 1.0, ""),
            FxParam::new("Width", self.width, 0.0, 1.0, ""),
            FxParam::new("Wet", self.mix, 0.0, 1.0, ""),
            // The order is frozen: a CC learned on `Wet` has to stay on `Wet`,
            // so these two go on the end.
            FxParam::new("MIDI", self.midi as u8 as f32, 0.0, 1.0, ""),
            FxParam::new(
                "Ch",
                (self.midi_channel.clamp(1, 16) - 1) as f32 / 15.0,
                1.0,
                16.0,
                "",
            ),
            // The vocoder half, appended so nothing above it moved: the mode,
            // then the vocoder's own knobs in its own order.
            FxParam::new(
                "Mode",
                (self.mode == Mode::Vocoder) as u8 as f32,
                0.0,
                1.0,
                "",
            ),
        ]
        .into_iter()
        .chain(self.voc.params().into_iter().take(VOC_PARAMS))
        .collect()
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => self.set_voices(v),
            1 => {
                self.shape = Shape::from_norm(v);
                self.dirty = true;
            }
            2 => self.set_key(v),
            3 => self.set_scale(v),
            4 => {
                self.detune = v * 25.0;
                self.dirty = true;
            }
            5 => {
                self.delay_ms = v * 50.0;
                self.dirty = true;
            }
            6 => self.env_amount = v,
            7 => {
                self.width = v;
                self.dirty = true;
            }
            8 => self.mix = v,
            9 => {
                self.midi = v >= 0.5;
                self.dirty = true;
            }
            10 => {
                self.set_midi_channel(v);
                self.dirty = true;
            }
            11 => self.mode = Mode::from_norm(v),
            i if i >= VOC_PARAM0 => self.voc.set_param(i - VOC_PARAM0, v),
            _ => {}
        }
    }
}

/// The twelve note names, for the `Key` knob's labels.
pub fn key_names() -> &'static [&'static str; 12] {
    &NOTE_NAMES
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The vocoder is this effect's other mode, and it is carried by the same
    /// held chord the harmony follows.
    #[test]
    fn the_vocoder_is_a_mode_of_the_harmoniser_and_the_chord_carries_it() {
        use crate::fx::FxProcessor;
        let sr = 48_000u32;
        let mut h = Harmonizer::new(sr);

        // The knobs the harmoniser always had are where they were: the merge
        // appended, so a project written before it opens unchanged.
        let params = h.params();
        assert_eq!(params[8].name, "Wet");
        assert_eq!(params[9].name, "MIDI");
        assert_eq!(params[11].name, "Mode");
        assert_eq!(params.len(), VOC_PARAM0 + VOC_PARAMS);
        assert_eq!(params[VOC_PARAM0].name, "Bands");

        // Harmony by default; the mode knob swaps what the block does.
        assert_eq!(h.mode(), Mode::Harmony);
        h.set_param(11, 1.0);
        assert_eq!(h.mode(), Mode::Vocoder);

        // Vocoder mode, carried by the chord: nothing held is silence — a
        // vocoder with no carrier says nothing — and a chord makes it speak.
        h.set_param(VOC_PARAM0 + 1, crate::fx::vocoder::Carrier::Chord.to_norm());
        h.set_mix(1.0);
        crate::chord::chord().clear();
        let mut buf: Vec<f32> = (0..2048).map(|i| ((i as f32) * 0.05).sin() * 0.5).collect();
        h.process_block(&mut buf, sr);
        let silent = buf.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(silent < 1e-3, "no chord, no carrier: {silent}");

        crate::chord::chord().set(&[48, 52, 55]);
        let mut buf: Vec<f32> = (0..8192).map(|i| ((i as f32) * 0.05).sin() * 0.5).collect();
        h.process_block(&mut buf, sr);
        let sounding = buf.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(sounding > 1e-3, "the chord carries the voice: {sounding}");
        crate::chord::chord().clear();
    }

    /// Full wet has to arrive at the level of what went in.
    ///
    /// It did not: two voices at full width pan hard left and hard right, each
    /// channel carried one of them at `1/sqrt(2)`, and with the pitch shifter's
    /// own cut on top the harmony came out **4.9 dB under the dry** — present
    /// on a meter, gone in a mix. The makeup in `rebuild` gives the pan's share
    /// of that back; what is left is the shifter, which is material-dependent
    /// and not a number to fake.
    #[test]
    fn the_wet_harmony_arrives_at_the_level_of_the_dry() {
        use crate::fx::FxProcessor;
        let sr = 48_000u32;
        let mut h = Harmonizer::new(sr);
        h.set_mix(1.0);
        let (mut sum_in, mut sum_out, mut n) = (0.0f64, 0.0f64, 0u64);
        let mut phase = 0.0f32;
        for block in 0..200 {
            let mut buf = vec![0.0f32; 512];
            for f in buf.as_chunks_mut::<2>().0 {
                let s = (phase * std::f32::consts::TAU).sin() * 0.3;
                phase = (phase + 220.0 / sr as f32).fract();
                f[0] = s;
                f[1] = s;
            }
            let dry = buf.clone();
            h.process_block(&mut buf, sr);
            // The first blocks are the shifter and the envelope filling up.
            if block > 100 {
                for (a, b) in dry.iter().zip(buf.iter()) {
                    sum_in += (*a as f64) * (*a as f64);
                    sum_out += (*b as f64) * (*b as f64);
                    n += 1;
                }
            }
        }
        let loss = 20.0 * ((sum_out / n as f64).sqrt() / (sum_in / n as f64).sqrt()).log10();
        assert!(
            loss > -3.0,
            "full wet is {loss:.1} dB under the dry — the harmony is being lost"
        );
        assert!(
            loss < 3.0,
            "and it must not be louder than what went in either"
        );
    }
    use crate::fx::FxProcessor;

    fn tone(hz: f32, sr: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let s = (std::f32::consts::TAU * hz * i as f32 / sr).sin() * 0.4;
                [s, s]
            })
            .collect()
    }

    fn energy_at(buf: &[f32], probe: f32, sr: f32) -> f32 {
        let l: Vec<f32> = buf.iter().step_by(2).copied().collect();
        let n = l.len() as f32;
        let k = (probe * n / sr).round();
        let w = std::f32::consts::TAU * k / n;
        let coeff = 2.0 * w.cos();
        let (mut s1, mut s2) = (0.0f32, 0.0f32);
        for x in &l {
            let s0 = x + coeff * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        ((s1 * s1 + s2 * s2 - coeff * s1 * s2).max(0.0)).sqrt() / n
    }

    /// **Diatonic, not parallel.** A third above the root of C major is four
    /// semitones; a third above the second degree is three. A harmoniser that
    /// shifts everything by a constant is wrong in exactly the places a
    /// listener notices, and this is the test that says which one this is.
    #[test]
    fn a_third_is_a_scale_step_not_a_fixed_distance() {
        let mut h = Harmonizer::new(48_000);
        h.set_voices(0.0); // one voice
        h.shape = Shape::Thirds; // its first step is a third
        h.set_scale(1.0 / (ScaleType::ALL.len() - 1) as f32); // major
        h.set_key(0.0); // C
        h.rebuild();
        let from_c = h.intervals()[0];
        assert!(
            (from_c - 4.0).abs() < 0.5,
            "a third above C in C major is four semitones: {from_c}"
        );

        // The same shape in a minor key is a minor third: three.
        h.set_scale(2.0 / (ScaleType::ALL.len() - 1) as f32);
        h.rebuild();
        let minor = h.intervals()[0];
        assert!(
            minor < from_c,
            "the third of a minor scale is smaller: {minor} vs {from_c}"
        );

        // Chromatic is the escape hatch: the step becomes a semitone count and
        // the harmony is parallel, which is a sound and not a mistake.
        h.set_scale(0.0);
        h.rebuild();
        assert!(
            (h.intervals()[0] - 2.0).abs() < 0.5,
            "chromatic takes the step as semitones: {}",
            h.intervals()[0]
        );
    }

    /// The voices actually sound, at the pitches they were told to.
    #[test]
    fn the_voices_are_there_and_in_tune() {
        let sr = 48_000.0;
        let mut h = Harmonizer::new(48_000);
        h.set_voices(0.334); // two
        h.shape = Shape::Octaves;
        h.set_scale(0.0); // chromatic: steps are semitones, so ±7 is a fifth
        h.detune = 0.0;
        h.delay_ms = 0.0;
        h.env_amount = 0.0;
        // Centred, because the reading below is of one channel and the voices
        // are fanned across the image by default.
        h.width = 0.0;
        h.set_mix(1.0);
        h.rebuild();

        let mut buf = tone(300.0, sr, 48_000);
        h.process_block(&mut buf, 48_000);
        let tail = &buf[24_000 * 2..];
        // Chromatic `Octaves` is +7 and −7 semitones: a fifth up and down.
        //
        // Read against a frequency neither voice is at, rather than against an
        // absolute: the shifter's crossfade puts a slight warble on a held
        // tone, which spreads a half-second reading across neighbouring bins
        // and makes every absolute number look small. What has to be true is
        // that there is energy where the voices are and none where they are
        // not.
        let up = energy_at(tail, 300.0 * 2f32.powf(7.0 / 12.0), sr);
        let down = energy_at(tail, 300.0 * 2f32.powf(-7.0 / 12.0), sr);
        let nowhere = energy_at(tail, 1_500.0, sr);
        assert!(
            up > nowhere * 8.0 && down > nowhere * 8.0,
            "both voices sound: up={up} down={down}, floor={nowhere}"
        );
        // And the note that was played is gone: fully wet is the harmony only.
        let original = energy_at(tail, 300.0, sr);
        assert!(
            original < up.max(down),
            "the dry note should not survive a full-wet harmoniser: {original}"
        );
    }

    /// Voice count is a count: eight voices are eight, and they do not add up
    /// to eight times the level of one.
    #[test]
    fn more_voices_do_not_mean_more_level() {
        let sr = 48_000.0;
        let peak_of = |knob: f32| {
            let mut h = Harmonizer::new(48_000);
            h.set_voices(knob);
            h.env_amount = 0.0;
            h.set_mix(1.0);
            h.rebuild();
            let mut buf = tone(220.0, sr, 24_000);
            h.process_block(&mut buf, 48_000);
            buf[12_000 * 2..].iter().fold(0.0f32, |m, s| m.max(s.abs()))
        };
        let one = peak_of(0.0);
        let eight = peak_of(1.0);
        assert!(one > 0.05, "one voice sounds: {one}");
        assert!(
            eight < one * 2.5,
            "eight voices must not be eight times louder: {one} then {eight}"
        );
    }

    /// The envelope follower is what stops a harmoniser singing through the
    /// gaps: with it up, silence in is silence out even though the delay lines
    /// still hold the last note.
    #[test]
    fn the_envelope_follower_closes_the_voices() {
        let sr = 48_000.0;
        let mut h = Harmonizer::new(48_000);
        h.set_voices(0.334);
        h.env_amount = 1.0;
        h.delay_ms = 50.0;
        h.set_mix(1.0);
        h.rebuild();
        let mut buf = tone(220.0, sr, 24_000);
        h.process_block(&mut buf, 48_000);
        // Then silence: the lines are full of the note, and the follower is
        // what decides whether it keeps coming out.
        let mut quiet = vec![0.0f32; 24_000 * 2];
        h.process_block(&mut quiet, 48_000);
        let tail = quiet[12_000 * 2..]
            .iter()
            .fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(tail < 0.01, "the voices should have closed: {tail}");
    }

    #[test]
    fn it_survives_silence_extremes_and_a_rate_change() {
        for shape in Shape::ALL {
            let mut h = Harmonizer::with_params(
                48_000,
                &[1.0, shape.to_norm(), 0.5, 0.5, 1.0, 1.0, 0.5, 1.0],
            );
            h.set_mix(1.0);
            let mut buf = vec![0.0f32; 1024];
            h.process_block(&mut buf, 48_000);
            assert!(
                buf.iter().all(|s| *s == 0.0),
                "{} rang in silence",
                shape.label()
            );
            let mut hot = vec![4.0f32; 4096];
            h.process_block(&mut hot, 96_000);
            assert!(
                hot.iter().all(|s| s.is_finite()),
                "{} went non-finite",
                shape.label()
            );
            h.process_block(&mut [], 96_000);
            h.process_block(&mut [1.0], 96_000);
            h.reset();
        }
    }
    /// **The harmony comes out as loud as what went in**, and it does so at any
    /// input level.
    ///
    /// This is the test that would have caught the report "I plugged a headset
    /// microphone into the harmoniser and got no response at all". Two things
    /// were taking it away: the voices were divided by their count when they
    /// should be divided by its square root (they sing different notes, so
    /// their powers add and not their amplitudes), and the envelope follower
    /// opened against an **absolute** level — so a microphone sitting at
    /// -40 dBFS never opened the voices past half however hard anybody sang.
    ///
    /// Measured on power, not on a single frequency bin: a delay-line shifter
    /// warbles, and a warble spreads a tone into sidebands that a one-bin
    /// measurement reads as silence. That mistake is how this was nearly
    /// "fixed" in the wrong place.
    #[test]
    fn the_harmony_is_as_loud_as_the_input_at_any_level() {
        let sr = 48_000.0f32;
        let rms = |v: &[f32]| {
            (v.iter().map(|s| (*s as f64) * (*s as f64)).sum::<f64>() / v.len() as f64).sqrt()
        };

        // A hot line and a quiet microphone, 30 dB apart.
        for amp in [0.4f32, 0.012] {
            let mut h = Harmonizer::new(sr as u32);
            h.set_mix(1.0);
            let (mut input, mut output) = (0.0f64, 0.0f64);
            for block in 0..40 {
                let mut buf: Vec<f32> = (0..512)
                    .flat_map(|i| {
                        let n = block * 512 + i;
                        let s = amp * (std::f32::consts::TAU * 220.0 * n as f32 / sr).sin();
                        [s, s]
                    })
                    .collect();
                let before = rms(&buf);
                h.process_block(&mut buf, sr as u32);
                // The first blocks are the shifter filling its line and the
                // follower finding the signal.
                if block > 20 {
                    input += before * before;
                    output += rms(&buf).powi(2);
                }
            }
            let loss = 10.0 * (output / input).max(1e-12).log10();
            assert!(
                loss > -7.0,
                "at amp {amp} the wet output is {loss:.1} dB under the input"
            );
        }
    }

    /// The chord being played decides the harmony, and the lowest note is the
    /// root it is measured from.
    ///
    /// What was asked for: "make the harmony follow what I play on the piano".
    /// With the switch off, nothing about the effect changes — which is the
    /// other half of the promise.
    #[test]
    fn a_held_chord_becomes_the_harmony() {
        let sr = 48_000.0;
        let mut h = Harmonizer::new(sr as u32);

        // Off: the shape and the key decide, and the chord is ignored.
        crate::chord::chord().set(&[60, 63, 70]);
        h.set_param(9, 0.0);
        let by_shape = h.intervals();
        assert!(!by_shape.is_empty());

        // On: a minor third and a fifth above the root, whatever the key says.
        h.set_param(9, 1.0);
        h.process_block(&mut [0.0f32; 64], sr as u32);
        let played = h.intervals();
        assert_eq!(played.len(), 2, "two notes above the root: {played:?}");
        assert!((played[0] - 3.0).abs() < 0.01, "{played:?}");
        assert!((played[1] - 10.0).abs() < 0.01, "{played:?}");

        // A new chord under the hand is a new harmony.
        crate::chord::chord().set(&[60, 64, 67]);
        h.process_block(&mut [0.0f32; 64], sr as u32);
        let played = h.intervals();
        assert!((played[0] - 4.0).abs() < 0.01, "{played:?}");
        assert!((played[1] - 7.0).abs() < 0.01, "{played:?}");

        // And with the switch off again it is back to the shape.
        h.set_param(9, 0.0);
        h.process_block(&mut [0.0f32; 64], sr as u32);
        assert_eq!(h.intervals().len(), by_shape.len());

        // The channel is a setting the interface reads; the DSP only stores it.
        h.set_param(9, 1.0);
        h.set_param(10, 0.0);
        assert_eq!(h.midi_input(), Some(1));
        h.set_param(10, 1.0);
        assert_eq!(h.midi_input(), Some(16));
        h.set_param(9, 0.0);
        assert_eq!(h.midi_input(), None);
        crate::chord::chord().clear();
    }
}
