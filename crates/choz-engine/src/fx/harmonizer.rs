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
//!   the note being sung is **detected** (the autotune's YIN detector), and
//!   each voice is that note plus the shape's interval, moved onto the nearest
//!   note of the scale — or of the chord, when a keyboard or a chart gives
//!   one. A third above a C in C major is E (four semitones); above a D it is
//!   F (three). Shifting everything by a constant is the sound of a cheap
//!   pitch shifter, and it is wrong in exactly the places a listener notices.
//!   It was that, for a while: the steps were walked from the key's tonic and
//!   the same shift applied to every note sung.
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
//! **No note input.** An [`super::FxProcessor`] is handed audio and nothing
//! else — there is no note port in an FX chain, by design (see the trait). The
//! chords it follows arrive through two process-wide doors instead: the one the
//! keyboard publishes ([`crate::chord::chord`], `MIDI`) and the one a `.chord`
//! chart publishes as the interface plays it ([`crate::chord::chart`],
//! `Chart`). The chart wins when both are on: it was started on purpose.

use super::psola::{PsolaSource, PsolaVoice};
use super::smooth::Smoothed;
use crate::fx::autotune::PitchDetector;
use crate::fx::autotune::{Scale, ScaleType, NOTE_NAMES};
use crate::fx::vocoder::Vocoder;
use crate::fx::FxProcessor as _;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

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

/// What the harmoniser did since the log last asked, for the interface to
/// write down: the notes it moved to, how often it rebuilt its voices, how
/// loud its output got and whether any of it passed full scale. Atomics, so
/// the audio thread writes without a lock; one set for the process, like the
/// chord it follows.
pub struct HarmStats {
    blocks: AtomicU32,
    notes: AtomicU32,
    last_note: AtomicU32,
    rebuilds: AtomicU32,
    /// Peak of the output, as `f32` bits — a positive float's bits order the
    /// way the float does, so the loudest block is an integer max.
    peak: AtomicU32,
    over: AtomicU32,
    intervals: [AtomicU32; MAX_VOICES],
    voices: AtomicU32,
}

/// One reading of [`HarmStats`], taken and reset.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct HarmReading {
    pub blocks: u32,
    pub notes: u32,
    /// The note the voices are measured from, `None` before one is heard.
    pub sung: Option<u8>,
    pub rebuilds: u32,
    pub peak: f32,
    /// Samples that left the effect past full scale.
    pub over: u32,
    /// What each voice is set to, in semitones, `voices` of them.
    pub intervals: [f32; MAX_VOICES],
    pub voices: usize,
}

static STATS: HarmStats = HarmStats {
    blocks: AtomicU32::new(0),
    notes: AtomicU32::new(0),
    last_note: AtomicU32::new(u32::MAX),
    rebuilds: AtomicU32::new(0),
    peak: AtomicU32::new(0),
    over: AtomicU32::new(0),
    intervals: [const { AtomicU32::new(0) }; MAX_VOICES],
    voices: AtomicU32::new(0),
};

pub fn stats() -> &'static HarmStats {
    &STATS
}

impl HarmStats {
    fn note(&self, n: i32) {
        self.notes.fetch_add(1, Relaxed);
        self.last_note.store(n.clamp(0, 127) as u32, Relaxed);
    }

    fn forget(&self) {
        self.last_note.store(u32::MAX, Relaxed);
    }

    fn rebuilt(&self, intervals: &[f32]) {
        self.rebuilds.fetch_add(1, Relaxed);
        for (slot, v) in self.intervals.iter().zip(intervals) {
            slot.store(v.to_bits(), Relaxed);
        }
        self.voices.store(intervals.len() as u32, Relaxed);
    }

    fn block(&self, peak: f32, over: u32) {
        self.blocks.fetch_add(1, Relaxed);
        self.peak.fetch_max(peak.max(0.0).to_bits(), Relaxed);
        self.over.fetch_add(over, Relaxed);
    }

    /// Everything since the last call, and start the counts over.
    pub fn take(&self) -> HarmReading {
        let note = self.last_note.load(Relaxed);
        let voices = (self.voices.load(Relaxed) as usize).min(MAX_VOICES);
        HarmReading {
            blocks: self.blocks.swap(0, Relaxed),
            notes: self.notes.swap(0, Relaxed),
            sung: (note <= 127).then_some(note as u8),
            rebuilds: self.rebuilds.swap(0, Relaxed),
            peak: f32::from_bits(self.peak.swap(0, Relaxed)),
            over: self.over.swap(0, Relaxed),
            intervals: std::array::from_fn(|i| f32::from_bits(self.intervals[i].load(Relaxed))),
            voices,
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
    /// **A chord of the player's choosing** over the note being sung — any
    /// species the arranger's chord dialogue can build (family, sixth or
    /// seventh, tensions, alterations), set with the `Ch…` knobs. The default,
    /// set to a major seventh: the shape a harmoniser is reached for.
    #[default]
    Chord,
}

impl Shape {
    pub const ALL: [Shape; 7] = [
        Shape::Thirds,
        Shape::Fifths,
        Shape::Octaves,
        Shape::Above,
        Shape::Below,
        Shape::Cluster,
        Shape::Chord,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Shape::Thirds => "3rds",
            Shape::Fifths => "5ths",
            Shape::Octaves => "OCT",
            Shape::Above => "ABOVE",
            Shape::Below => "BELOW",
            Shape::Cluster => "CLUSTER",
            Shape::Chord => "CHORD",
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
            // A major seventh in steps; the chosen species replaces it — see
            // `Harmonizer::intervals_now`.
            Shape::Chord => [2, 4, 6, 9, 11, 13, -3, -5],
        }
    }

    /// The same intervals in semitones, read against a **major** scale: a
    /// third is four, a fifth seven, an octave twelve. What the voices aim at
    /// before the scale or the chord moves them onto a note that belongs.
    ///
    /// Semitones rather than steps because a step means nothing outside a
    /// seven-note scale: seven steps is an octave in major, an octave and a
    /// second in a pentatonic, and a fifth in chromatic — which is what `OCT`
    /// played there.
    pub fn semitones(self) -> [i32; MAX_VOICES] {
        const MAJOR: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];
        self.steps().map(|step| {
            let octave = step.div_euclid(7);
            MAJOR[step.rem_euclid(7) as usize] + 12 * octave
        })
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
    /// The voice's pitch shift, formants kept — see [`super::psola`]. The
    /// delay-line shifter it replaced moved the vowels with the note, and a
    /// harmony an octave from the singer sounded sung inside a tube.
    shifter: PsolaVoice,
    /// What this voice is actually transposed by, in semitones. Kept rather
    /// than recomputed: with a chord driving the harmony there is nothing to
    /// recompute it *from*, and [`Harmonizer::intervals`] used to answer with
    /// the shape's intervals whatever the voices were really doing.
    semitones: f32,
    /// Where the shifter actually is, gliding to `semitones`: a new harmony
    /// arrives over a few milliseconds rather than in one sample, which is a
    /// portamento rather than a click. `NaN` until the first build, which
    /// lands on its target at once.
    gliding: f32,
    level: f32,
    /// The note this voice last sang, in MIDI — what the next chord is voiced
    /// *from*. `None` until it has sung, and again after a forgotten phrase.
    note: Option<i32>,
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
            shifter: PsolaVoice::new(),
            semitones: 0.0,
            gliding: f32::NAN,
            note: None,
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
    /// Follow the chord a `.chord` chart publishes as it plays
    /// ([`crate::chord::chart`]) — the harmony changes with the progression.
    chart: bool,
    /// See [`OCTAVE_PARAM`].
    octave: Option<i32>,
    /// See [`SPEC_PARAM0`].
    spec: crate::artifacts::arranger::spec::ChordSpec,
    /// See [`ARR_SYNC_PARAM`].
    arr_sync: bool,
    chart_seen: u32,
    /// What is being sung, and the note it was last heard as — the note every
    /// voice is measured from. `None` until something voiced has been heard;
    /// the key's tonic stands in until then.
    detector: PitchDetector,
    /// The input's recent past and period, shared by every voice's shifter.
    psola: PsolaSource,
    sung: Option<i32>,
    /// A note heard but not yet believed, and for how many frames it has held.
    candidate: Option<(i32, u32)>,
    /// Frames since anything was heard: past [`FORGET_MS`] the note is let go.
    silent: u32,
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
    /// Scales the voices back to the input's loudness: `1/√n` for the `n`
    /// input-sized voices the harmony holds (two at most, see `rebuild`).
    balance: f32,
    /// Voices the last rebuild set up: the count, or the chord's size while a
    /// chord with nothing sung over it is the harmony.
    built_count: usize,
    /// See [`LEAD_PARAM`].
    lead: f32,
    mix: f32,
    sample_rate: f32,
    dirty: bool,
}

impl Harmonizer {
    pub fn new(sample_rate: u32) -> Self {
        // The chord shape reads its notes from a table parsed once; parse it
        // here, on the thread that builds effects, not the one that runs them.
        crate::artifacts::arranger::spec::warm();
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
            chart: false,
            octave: None,
            spec: default_spec(),
            arr_sync: false,
            chart_seen: 0,
            detector: PitchDetector::with_window(sr, DETECT_WINDOW, DETECT_HOP),
            psola: PsolaSource::new(sr),
            sung: None,
            candidate: None,
            silent: 0,
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
            balance: 1.0,
            built_count: 2,
            lead: 0.0,
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
        h.chart = get(CHART_PARAM, 0.0) >= 0.5;
        // AUTO when it is not there: a project from before the knob.
        h.octave = octave_of(get(OCTAVE_PARAM, 0.0));
        // The chord shape's species, a major seventh when it is not there —
        // which is the MAJ7 shape a project from before it was using.
        let d = default_spec();
        for (row, (v, (_, n))) in [
            d.family,
            d.seventh,
            d.tension,
            d.ninth,
            d.eleventh,
            d.thirteenth,
            d.fifth,
        ]
        .into_iter()
        .zip(SPEC_ROWS)
        .enumerate()
        {
            h.set_param(SPEC_PARAM0 + row, get(SPEC_PARAM0 + row, spec_norm(v, n)));
        }
        h.arr_sync = get(ARR_SYNC_PARAM, 0.0) >= 0.5;
        h.lead = get(LEAD_PARAM, 0.0).clamp(0.0, 1.0);
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
            .take(self.built_count)
            .map(|v| v.semitones)
            .collect()
    }

    /// Whether this is following a chart right now, rather than a keyboard.
    pub fn follows_chart(&self) -> bool {
        self.chart || self.arr_sync
    }

    /// The species' rows, in [`SPEC_ROWS`] order.
    fn spec_values(&self) -> [usize; SPEC_PARAMS] {
        let s = &self.spec;
        [
            s.family,
            s.seventh,
            s.tension,
            s.ninth,
            s.eleventh,
            s.thirteenth,
            s.fifth,
        ]
    }

    /// The chord `Shape::Chord` sings, for the panel.
    pub fn spec(&self) -> crate::artifacts::arranger::spec::ChordSpec {
        self.spec
    }

    /// Whether it follows the arranger's progression.
    pub fn follows_arranger(&self) -> bool {
        self.arr_sync
    }

    /// Each voice's interval, in semitones, before the scale or chord has its
    /// say. For `Chord`, the chosen species over the note: its tones, then
    /// the same an octave down, then an octave up — so two voices are the two
    /// nearest chord tones above, and eight fill the chord both sides.
    fn intervals_now(&self) -> [i32; MAX_VOICES] {
        if self.shape != Shape::Chord {
            return self.shape.semitones();
        }
        let tones = self.spec.tones();
        if tones.is_empty() {
            return self.shape.semitones();
        }
        let mut out = [0i32; MAX_VOICES];
        for (i, v) in out.iter_mut().enumerate() {
            let t = tones[i % tones.len()] as i32;
            *v = match i / tones.len() {
                0 => t,
                1 => t - 12,
                2 => t + 12,
                k => t - 12 * (k as i32 - 1),
            };
        }
        out
    }

    /// The note every voice is measured from: what is being sung, and the
    /// key's tonic until something has been heard.
    pub fn sung(&self) -> Option<i32> {
        self.sung
    }

    /// Where a voice lands: `raw`, moved onto the nearest note `allowed` says
    /// belongs, **towards the sung note on a tie** — a major third in a minor
    /// key is the minor third, not the fourth. A voice that would land on the
    /// sung note's own pitch class is moved on unless the shape asked for an
    /// octave: two singers on one note is one louder singer.
    fn snap(raw: i32, sung: i32, allowed: &dyn Fn(i32) -> bool, taken: &[i32]) -> i32 {
        let octave = (raw - sung).rem_euclid(12) == 0;
        let up = raw >= sung;
        for d in 0..=12 {
            let (toward, away) = match up {
                true => (raw - d, raw + d),
                false => (raw + d, raw - d),
            };
            for c in [toward, away] {
                if allowed(c) && (octave || (c - sung).rem_euclid(12) != 0) && !taken.contains(&c) {
                    return c;
                }
            }
        }
        raw
    }

    /// A voice's shift in semitones from `sung`: the shape's interval, fitted
    /// to the scale — or to `chord`'s pitch classes when there is a chord —
    /// and the micro-pitch spread on top.
    ///
    /// **Not onto a note another voice already has** (`taken`): a third and a
    /// fifth snapped to a three-note chord could both land on its fifth, and
    /// two voices a few cents apart on one note is a beating chorus, which is
    /// what the harmony sounded like on a chart — measured, `[4.92, 5.08]`.
    /// Voice leading: of the chord's tones near where the shape puts this
    /// voice, the one it gets to by moving least.
    ///
    /// A choir does not jump a voice to "the third above the tune" on every
    /// chord; each part moves to the nearest note of the new chord, holding a
    /// common tone where there is one. So the cost is the step from `prev`,
    /// counted twice, plus the distance from `raw` (the shape's own note),
    /// which keeps the part in its register — and the search stays within a
    /// fifth of `raw`, so a voice meant a third above never wanders below the
    /// tune. Unisons and the sung note's pitch class are still out, as in
    /// [`Self::snap`]; nothing that fits is found, and `snap` answers.
    fn lead(
        raw: i32,
        sung: i32,
        prev: Option<i32>,
        allowed: &dyn Fn(i32) -> bool,
        taken: &[i32],
    ) -> i32 {
        let Some(prev) = prev else {
            return Self::snap(raw, sung, allowed, taken);
        };
        let octave = (raw - sung).rem_euclid(12) == 0;
        let mut best: Option<(i32, i32)> = None;
        for c in raw - 7..=raw + 7 {
            let fits = allowed(c)
                && (octave || (c - sung).rem_euclid(12) != 0)
                && c != sung
                && !taken.contains(&c);
            if !fits {
                continue;
            }
            let cost = 2 * (c - prev).abs() + (c - raw).abs();
            if best.is_none_or(|(_, b)| cost < b) {
                best = Some((c, cost));
            }
        }
        best.map(|(c, _)| c)
            .unwrap_or_else(|| Self::snap(raw, sung, allowed, taken))
    }

    fn semitones_for(
        &self,
        interval: i32,
        voice: usize,
        sung: i32,
        chord: &[u8],
        taken: &mut ([i32; MAX_VOICES], usize),
    ) -> (f32, i32) {
        let raw = sung + interval;
        // A scale moves the harmony in parallel — that *is* the effect asked
        // for, a third above the tune. A chord is where parts move the least.
        // A chord picked by name is the chord: the key does not bend it.
        let chosen = self.shape == Shape::Chord;
        let target = match chord.is_empty() {
            true if chosen => Self::snap(raw, sung, &|_| true, &taken.0[..taken.1]),
            true => Self::snap(raw, sung, &|n| self.scale.contains(n), &taken.0[..taken.1]),
            false => Self::lead(
                raw,
                sung,
                self.voices[voice].note,
                &|n| chord.iter().any(|c| (*c as i32 - n).rem_euclid(12) == 0),
                &taken.0[..taken.1],
            ),
        };
        if taken.1 < MAX_VOICES {
            taken.0[taken.1] = target;
            taken.1 += 1;
        }
        // Micro-pitch: the voices fan out around the note rather than all
        // sitting a fixed distance off it, so an odd voice count is not
        // lopsided.
        let spread = if self.count > 1 {
            (voice as f32 / (self.count - 1) as f32) * 2.0 - 1.0
        } else {
            0.0
        };
        (
            (target - sung) as f32 + spread * self.detune / 100.0,
            target,
        )
    }

    /// The chord the harmony follows, if any: the chart's when `Chart` is on
    /// (it was started on purpose), else the keyboard's when `MIDI` is.
    fn held(&self, out: &mut [u8; crate::chord::MAX_NOTES]) -> usize {
        match (self.chart || self.arr_sync, self.midi) {
            (true, _) => crate::chord::chart().read(out),
            (false, true) => crate::chord::chord().read(out),
            _ => 0,
        }
    }

    /// Recompute what each voice does. Off the audio path: this walks scales
    /// and calls `powf`, and none of that belongs in a callback.
    fn rebuild(&mut self) {
        self.scale = Scale::new(self.key, self.kind);
        let mut held = [0u8; crate::chord::MAX_NOTES];
        let chord = self.held(&mut held);
        let intervals = self.intervals_now();
        // **Nothing sung yet, and a chord to follow**: the chord itself is the
        // harmony, measured from its lowest note — a harmoniser that waits for
        // a voice before it says anything is one that cannot be checked with
        // the singer silent. Once a note is heard the voices are fitted around
        // *it*, which is what a harmony is.
        let from_chord = chord > 1 && self.sung.is_none();
        let count = match from_chord {
            true => (chord - 1).min(MAX_VOICES),
            false => self.count,
        };
        let sung = self.sung.unwrap_or(self.key as i32 + 60);
        let pool: &[u8] = match chord > 0 && !from_chord {
            true => &held[..chord],
            false => &[],
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
        //
        // **Each voice at the input's level, up to two of them**: one or two
        // voices each as loud as what was sung, and past two the choir shares
        // twice the input's power — `√(2/n)` a voice — so eight are not a
        // wall.
        let level = (2.0 / (count as f32).max(2.0)).sqrt();
        // On the stack: this runs on the audio thread on every new note, and a
        // `Vec` here was an allocation per note sung.
        let mut taken = ([0i32; MAX_VOICES], 0usize);
        for i in 0..MAX_VOICES {
            // Chord: the interval from the root to the i-th note above it, in
            // semitones and needing no scale at all — the hand already chose
            // the notes. Otherwise the shape, walked through the key.
            let (semis, note) = match from_chord && i + 1 < chord {
                true => ((held[i + 1] as i32 - held[0] as i32) as f32, None),
                false => {
                    let (semis, note) = self.semitones_for(
                        intervals[i.min(intervals.len() - 1)],
                        i,
                        sung,
                        pool,
                        &mut taken,
                    );
                    (semis, Some(note))
                }
            };
            // The register asked for: the same note, folded into the octave.
            let (semis, note) = match self.octave {
                None => (semis, note),
                Some(o) => {
                    let base = 12 * (o + 1);
                    let at = note.unwrap_or(sung + semis.round() as i32);
                    let mut folded = base + (at - base).rem_euclid(12);
                    // **Never more than two octaves from the voice.** The
                    // register asked for is where the voices go *if they can*:
                    // a tune sung two octaves above C1 put every voice at 30 Hz
                    // — a sub-bass a headset does not play — and past ±36 the
                    // shifter cannot even reach the note. Too far, and the
                    // voice comes back an octave at a time, keeping its note.
                    while folded - sung < -MAX_FOLD {
                        folded += 12;
                    }
                    while folded - sung > MAX_FOLD {
                        folded -= 12;
                    }
                    (semis + (folded - at) as f32, note.map(|_| folded))
                }
            };
            let voice = &mut self.voices[i];
            voice.note = note;
            // The target: `process_block` glides the shifter there.
            voice.semitones = semis;
            if voice.gliding.is_nan() {
                voice.gliding = semis;
                voice.shifter.set_semitones(semis);
            }
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
        // What each channel would carry with every voice in the middle.
        let unpanned: f32 = self
            .voices
            .iter()
            .take(count)
            .map(|v| v.level.powi(2))
            .sum();
        // Capped: a single hard-panned voice would otherwise ask for infinite
        // makeup on the silent side. `SHIFTER_LOSS` is the pitch shifter's own
        // cut, measured: its two heads crossfade uncorrelated material, and
        // the sum comes out ~2 dB under what went in.
        self.makeup = ((unpanned / loudest).sqrt() * SHIFTER_LOSS).clamp(1.0, 4.0);
        // Each voice is as loud as the input, so two together are louder than
        // it — +3 dB, which is a gain stage and not a harmoniser. The balance
        // between them is what was asked for; the level of the whole is put
        // back where it came in.
        // The lead, when it is kept, counts for its share.
        self.balance = (self.lead * self.lead + count.min(2) as f32)
            .max(1.0)
            .sqrt()
            .recip();
        self.dirty = false;
        self.chord_seen = crate::chord::chord().generation();
        self.chart_seen = crate::chord::chart().generation();
        self.built_count = count;
        let mut now = [0.0f32; MAX_VOICES];
        for (slot, v) in now.iter_mut().zip(self.voices.iter()) {
            *slot = v.semitones;
        }
        stats().rebuilt(&now[..count]);
    }
}

/// Where the vocoder's own knobs start in the merged parameter list, and how
/// many of them there are — its dry/wet is not one of them: the merged effect
/// has one, and it is `Wet` above.
pub const VOC_PARAM0: usize = 12;
pub const VOC_PARAMS: usize = 6;

/// The `Chart` switch: after the vocoder's knobs, so nothing before it moved.
pub const CHART_PARAM: usize = VOC_PARAM0 + VOC_PARAMS;

/// Where the voices sing: `None` follows the tune (every voice a shape's
/// distance from it), `Some(n)` puts every voice inside octave `n` — C`n` up to
/// the B above it — keeping its note and changing only its register. Appended
/// after the chart switch, so every index before it is where it was.
pub const OCTAVE_PARAM: usize = CHART_PARAM + 1;

/// The chord `Shape::Chord` sings: the arranger's chord dialogue, a knob a row
/// — family, 6/7, tension, 9, 11, 13, 5 — in [`SPEC_ROWS`] order.
pub const SPEC_PARAM0: usize = OCTAVE_PARAM + 1;
pub const SPEC_PARAMS: usize = 7;

/// Follow the arranger's progression: while any tab's arranger plays, its
/// chord is the harmony. Its ▶ is the master; the interface does the
/// following, this switch says to be followed.
pub const ARR_SYNC_PARAM: usize = SPEC_PARAM0 + SPEC_PARAMS;

/// How much of the singer's own voice comes out with the harmony: 0, the
/// default, is the voices alone — what a call through `choz Mic` should hear —
/// and 1 is the voice with its harmony.
pub const LEAD_PARAM: usize = ARR_SYNC_PARAM + 1;

/// The chord rows as knob names, and how many values each has.
pub const SPEC_ROWS: [(&str, usize); SPEC_PARAMS] = {
    use crate::artifacts::arranger::spec as s;
    [
        ("ChFamily", s::FAMILIES.len()),
        ("Ch6/7", s::SEVENTHS.len()),
        ("ChTension", s::TENSIONS.len()),
        ("Ch9", s::NINTHS.len()),
        ("Ch11", s::ELEVENTHS.len()),
        ("Ch13", s::THIRTEENTHS.len()),
        ("Ch5", s::FIFTHS.len()),
    ]
};

/// The species `Shape::Chord` starts on: a major seventh.
fn default_spec() -> crate::artifacts::arranger::spec::ChordSpec {
    crate::artifacts::arranger::spec::ChordSpec {
        seventh: 3,
        ..Default::default()
    }
}

/// A row of the species as a 0..1 knob, and back.
pub fn spec_norm(v: usize, n: usize) -> f32 {
    v as f32 / (n.max(2) - 1) as f32
}
pub fn spec_step(v: f32, n: usize) -> usize {
    ((v.clamp(0.0, 1.0) * (n.max(2) - 1) as f32).round() as usize).min(n - 1)
}

/// The furthest the octave may put a voice from the note being sung, in
/// semitones: two octaves. The shifter's own reach is three, and it sounds
/// like a voice for about two.
const MAX_FOLD: i32 = 24;

/// The octaves the knob offers, after AUTO: C1 up to C7.
pub const OCTAVES: std::ops::RangeInclusive<i32> = 1..=7;

/// The knob's steps: AUTO, then each octave.
pub const OCTAVE_STEPS: usize = 8;

/// Which octave a 0..1 knob sits on — `None` for AUTO.
pub fn octave_of(v: f32) -> Option<i32> {
    match (v.clamp(0.0, 1.0) * (OCTAVE_STEPS - 1) as f32).round() as i32 {
        0 => None,
        i => Some(i.min(*OCTAVES.end())),
    }
}

/// The knob position for an octave, `None` being AUTO.
pub fn octave_norm(octave: Option<i32>) -> f32 {
    match octave {
        None => 0.0,
        Some(o) => o.clamp(*OCTAVES.start(), *OCTAVES.end()) as f32 / (OCTAVE_STEPS - 1) as f32,
    }
}

/// What the knob says: `AUTO`, or the octave's C (`C3`).
pub fn octave_label(octave: Option<i32>) -> String {
    match octave {
        None => "AUTO".to_string(),
        Some(o) => format!("C{o}"),
    }
}

/// What the harmoniser listens with: 32 ms of the voice, looked at every
/// 4 ms. Half the autotune's window — enough for two periods of a 70 Hz note,
/// which is below where anybody sings — and the harmony moves to a new note in
/// under half the time it took with the autotune's 64 ms.
const DETECT_WINDOW: usize = 512;
const DETECT_HOP: usize = 64;

/// How far, in semitones, the sung pitch has to move from the note it was
/// heard as before the voices are moved: past the halfway point to the next
/// note and a little more, so vibrato and scoops stay one note.
const NOTE_HYSTERESIS: f32 = 0.65;

/// How long a new note has to read the same before the voices move to it.
/// Three of the detector's 4 ms looks: enough to sit out an attack's first
/// wrong readings, short enough that the harmony still follows in under 50 ms.
const STEADY_MS: f32 = 12.0;

/// Gives back what the pitch shifter takes: measured, a PSOLA voice comes out
/// a little under its input (its grains' windows do not quite add to one),
/// and the level test holds each voice to the input within 1.5 dB with this.
const SHIFTER_LOSS: f32 = 1.245;

/// How clean the detector's reading must be for the grains to be cut at its
/// period. Under it the input is treated as unvoiced — short fixed grains —
/// which is the safe mistake: cutting a consonant at a wrong period is a buzz.
const PSOLA_CONFIDENCE: f32 = 0.6;

/// How clean a reading must be to count towards a new note at all.
const STEADY_CONFIDENCE: f32 = 0.8;

/// Where the envelope follower counts the voices as fully open, as a share of
/// the recent peak: -20 dB. Under that is a gap, and the voices close with it.
const OPEN_BELOW_PEAK: f32 = 0.1;

/// How long a silence has to last before the note it followed is forgotten.
const FORGET_MS: f32 = 1000.0;

/// How long the voices take to move to a new harmony.
const GLIDE_MS: f32 = 8.0;

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
            // The grains are cut in periods, and a period in samples is the
            // rate's business.
            self.psola.set_sample_rate(sr);
            self.detector.set_sample_rate(sr);
            self.dirty = true;
        }
        // A hand that moved on the keyboard is a rebuild, and only that: the
        // generation is one relaxed load per block.
        if self.midi && crate::chord::chord().generation() != self.chord_seen {
            self.dirty = true;
        }
        if (self.chart || self.arr_sync) && crate::chord::chart().generation() != self.chart_seen {
            self.dirty = true;
        }
        if self.dirty {
            self.rebuild();
        }
        let count = self.built_count;
        // The glide, a step a block: a block is a few milliseconds, the glide
        // is `GLIDE_MS`, so a change of harmony is heard as the voices moving
        // to it rather than as a jump in the shifter's speed.
        let frames = (buf.len() / 2) as f32;
        let step = 1.0 - (-frames / (GLIDE_MS * 0.001 * sr)).exp();
        for v in self.voices.iter_mut().take(count) {
            if v.gliding.is_nan() {
                v.gliding = v.semitones;
                v.shifter.set_semitones(v.gliding);
            } else if (v.gliding - v.semitones).abs() > 1e-3 {
                v.gliding += (v.semitones - v.gliding) * step;
                if (v.gliding - v.semitones).abs() < 0.01 {
                    v.gliding = v.semitones;
                }
                v.shifter.set_semitones(v.gliding);
            }
        }
        // The period the grains are cut at: what the detector heard up to the
        // end of the last block. Unvoiced — a consonant, a breath — gets short
        // fixed grains, which move nothing: a sibilant is not a note.
        let heard = self.detector.estimate();
        self.psola.set_pitch(
            (heard.voiced && heard.confidence >= PSOLA_CONFIDENCE).then_some(heard.frequency_hz),
        );
        let mix = self.mix;
        let makeup = self.makeup;
        let (mut block_peak, mut block_over) = (0.0f32, 0u32);
        let env_amount = self.env_amount;
        // Two seconds to fall by 1/e, as a per-sample coefficient.
        let peak_decay = (-1.0 / (2.0 * sr)).exp();

        for frame in buf.as_chunks_mut::<2>().0 {
            let (dry_l, dry_r) = (frame[0], frame[1]);
            // One voice in, one signal to harmonise: a harmoniser fed a stereo
            // pair would be transposing two different signals into one chord.
            let mono = (dry_l + dry_r) * 0.5;
            self.detector.process(std::slice::from_ref(&mono));
            // Once, for every voice to cut its grains from.
            self.psola.push(mono);

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
            // wide as a hot line does.
            //
            // **Fully open down to `OPEN_BELOW_PEAK` under the peak**, not
            // half of it. The voices are copies of the input, so they already
            // decay with it; closing them as well whenever it fell under half
            // its peak made a piano's harmony die twice as fast as the piano —
            // 5 dB under the dry at the default `Env`, 10 at full, which is
            // "at 100 % wet I hear more of what goes in". What this is for is
            // the gaps, and a gap is far further down than a decaying note.
            let reference = self.peak.max(1e-4) * OPEN_BELOW_PEAK;
            let open = 1.0 - env_amount + env_amount * (e / reference).min(1.0);

            let mut wet = [0.0f32; 2];
            for voice in self.voices.iter_mut().take(count) {
                // Every voice is written every sample even when its delay is
                // zero: the line has to keep moving or the tap reads the past
                // of a stopped clock.
                // Shift first, delay second. The other order is what made
                // every delayed voice come out at the original pitch.
                let shifted = voice.shifter.process(&self.psola);
                let sound = voice.delayed(shifted);
                let g = sound * voice.level * open;
                wet[0] += g * voice.gain[0];
                wet[1] += g * voice.gain[1];
            }

            // **The voices alone, unless `Lead` asks for the singer.** At
            // full Wet and the default Lead of 0 what leaves is only what choz
            // made: through `choz Mic` a call hears the choir, not the
            // microphone under it.
            let (wet_l, wet_r) = (wet[0] * makeup, wet[1] * makeup);
            let processed = [
                (dry_l * self.lead + wet_l) * self.balance,
                (dry_r * self.lead + wet_r) * self.balance,
            ];
            frame[0] = dry_l + mix * (processed[0] - dry_l);
            frame[1] = dry_r + mix * (processed[1] - dry_r);
            let loudest = frame[0].abs().max(frame[1].abs());
            block_peak = block_peak.max(loudest);
            block_over += (loudest > 1.0) as u32;
        }
        stats().block(block_peak, block_over);
        // The note being sung, for the next block's voices. A new note is a
        // rebuild; a note held through a consonant or a breath is kept, and so
        // is one wobbling inside its own semitone — vibrato is not a new note.
        //
        // **And only a note that has held.** The first reading of an attack is
        // the least believable one: a SoundFont's C4 was read as C3, a G4 went
        // 67 → 39 → 43 → 67 in 18 ms, and every one of those moved the voices
        // for a moment onto notes nobody sang — the "noise" in a harmony that
        // was never clipping. A note has to read the same for `STEADY_MS`.
        let e = self.detector.estimate();
        let frames = (buf.len() / 2) as u32;
        let voiced = e.voiced && e.frequency_hz > 0.0 && e.confidence >= STEADY_CONFIDENCE;
        // **A silence forgets the note.** The last note heard stood as the
        // reference for as long as nothing replaced it — minutes after the
        // player stopped, the voices still moved round it on every chord and
        // the log said "hears G2" over nothing. After `FORGET_MS` of nothing,
        // there is no note: the next one is measured from itself.
        self.silent = match voiced {
            true => 0,
            false => self.silent.saturating_add(frames),
        };
        if !voiced && self.sung.is_some() && self.silent as f32 >= FORGET_MS * 0.001 * sr {
            self.sung = None;
            // A new phrase starts its parts where the shape puts them, not
            // where the last phrase left them.
            for v in self.voices.iter_mut() {
                v.note = None;
            }
            self.dirty = true;
            stats().forget();
        }
        if voiced {
            let note = 69.0 + 12.0 * (e.frequency_hz / 440.0).log2();
            let moved = self
                .sung
                .is_none_or(|s| (note - s as f32).abs() > NOTE_HYSTERESIS);
            let heard = note.round() as i32;
            self.candidate = match (moved, self.candidate) {
                (false, _) => None,
                (true, Some((n, held))) if n == heard => Some((n, held + frames)),
                (true, _) => Some((heard, frames)),
            };
            if let Some((n, held)) = self.candidate {
                if held as f32 >= STEADY_MS * 0.001 * sr {
                    self.sung = Some(n);
                    self.candidate = None;
                    self.dirty = true;
                    stats().note(n);
                }
            }
        } else {
            self.candidate = None;
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
        self.detector.reset();
        self.psola.reset();
        self.sung = None;
        self.candidate = None;
        for v in self.voices.iter_mut() {
            v.note = None;
        }
        self.silent = 0;
        for v in self.voices.iter_mut() {
            v.gliding = f32::NAN;
        }
        self.dirty = true;
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
        // Appended after the vocoder's, for the same reason: follow the chart
        // the interface plays.
        .chain(std::iter::once(FxParam::new(
            "Chart",
            self.chart as u8 as f32,
            0.0,
            1.0,
            "",
        )))
        .chain(std::iter::once(FxParam::new(
            "Octave",
            octave_norm(self.octave),
            0.0,
            1.0,
            "",
        )))
        .chain(
            self.spec_values()
                .into_iter()
                .zip(SPEC_ROWS)
                .map(|(v, (name, n))| FxParam::new(name, spec_norm(v, n), 0.0, 1.0, "")),
        )
        .chain(std::iter::once(FxParam::new(
            "ArrSync",
            self.arr_sync as u8 as f32,
            0.0,
            1.0,
            "",
        )))
        .chain(std::iter::once(FxParam::new(
            "Lead", self.lead, 0.0, 1.0, "",
        )))
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
            CHART_PARAM => {
                self.chart = v >= 0.5;
                self.dirty = true;
            }
            OCTAVE_PARAM => {
                self.octave = octave_of(v);
                self.dirty = true;
            }
            i if (SPEC_PARAM0..SPEC_PARAM0 + SPEC_PARAMS).contains(&i) => {
                let row = i - SPEC_PARAM0;
                let step = spec_step(v, SPEC_ROWS[row].1);
                let s = &mut self.spec;
                *[
                    &mut s.family,
                    &mut s.seventh,
                    &mut s.tension,
                    &mut s.ninth,
                    &mut s.eleventh,
                    &mut s.thirteenth,
                    &mut s.fifth,
                ][row] = step;
                self.dirty = true;
            }
            LEAD_PARAM => {
                self.lead = v;
                self.dirty = true;
            }
            ARR_SYNC_PARAM => {
                self.arr_sync = v >= 0.5;
                self.dirty = true;
            }
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
        let _chord = crate::test_locks::chord();
        use crate::fx::FxProcessor;
        let sr = 48_000u32;
        let mut h = Harmonizer::new(sr);

        // The knobs the harmoniser always had are where they were: the merge
        // appended, so a project written before it opens unchanged.
        let params = h.params();
        assert_eq!(params[8].name, "Wet");
        assert_eq!(params[9].name, "MIDI");
        assert_eq!(params[11].name, "Mode");
        assert_eq!(params.len(), LEAD_PARAM + 1);
        assert_eq!(params[LEAD_PARAM].name, "Lead");
        assert_eq!(params[SPEC_PARAM0].name, "ChFamily");
        assert_eq!(params[ARR_SYNC_PARAM].name, "ArrSync");
        assert_eq!(params[CHART_PARAM].name, "Chart");
        assert_eq!(params[OCTAVE_PARAM].name, "Octave");
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

    /// Each voice arrives at the level of the input, and at full Wet the input
    /// itself is gone.
    ///
    /// Reported twice: "the input is louder than the harmonised voice", and
    /// then, through `choz Mic`, "I still hear my own voice under the
    /// harmony". At full Wet only the voices come out, each as loud as the
    /// input within 1.5 dB.
    #[test]
    fn the_wet_harmony_arrives_at_the_level_of_the_dry() {
        use crate::fx::FxProcessor;
        let sr = 48_000u32;
        let mut h = Harmonizer::new(sr);
        h.set_mix(1.0);
        let (mut sum_in, mut sum_out, mut sum_all) = (0.0f64, 0.0f64, 0.0f64);
        let mut sum_cross = 0.0f64;
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
                    let harmony = b / h.balance;
                    sum_in += (*a as f64) * (*a as f64);
                    sum_cross += (*a as f64) * (*b as f64);
                    sum_out += (harmony as f64) * (harmony as f64);
                    let out = *b as f64;
                    sum_all += out * out;
                }
            }
        }
        // Two voices, uncorrelated: their powers add, so each is half of it.
        let per_voice = 10.0 * (sum_out / 2.0 / sum_in).log10();
        assert!(
            per_voice.abs() < 1.5,
            "each voice is {per_voice:+.1} dB against the input"
        );
        // None of the singer in it: what is in phase with the input is the
        // input leaking through.
        let leak = (sum_cross / sum_in).abs();
        assert!(leak < 0.1, "the input leaks through at {leak:.2}");
        // …and the voices come out at the input's level: the balance changed,
        // the tab's loudness did not.
        let whole = 10.0 * (sum_all / sum_in).log10();
        assert!(whole.abs() < 2.0, "the harmoniser adds {whole:+.1} dB");
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

        // Chromatic is the escape hatch: every note belongs, so the interval
        // is taken as it is — a major third, parallel, which is a sound and
        // not a mistake. (It used to be two semitones: the step count read as
        // semitones, so `3rds` sang seconds and `OCT` fifths.)
        h.set_scale(0.0);
        h.rebuild();
        assert!(
            (h.intervals()[0] - 4.0).abs() < 0.5,
            "chromatic takes the interval as written: {}",
            h.intervals()[0]
        );
    }

    /// **The third is measured from the note being sung**, not from the key.
    /// A D sung in C major takes F above it — three semitones — and a C takes
    /// E, four. It used to walk the steps from the tonic and apply the same
    /// shift to every note, which is the parallel harmoniser this effect says
    /// it is not.
    #[test]
    fn the_third_follows_the_note_being_sung() {
        let third_over = |hz: f32| {
            let mut h = Harmonizer::new(48_000);
            h.set_voices(0.0);
            h.shape = Shape::Thirds;
            h.set_scale(1.0 / (ScaleType::ALL.len() - 1) as f32); // C major
            h.set_key(0.0);
            h.detune = 0.0;
            let mut buf = tone(hz, 48_000.0, 24_000);
            h.process_block(&mut buf, 48_000);
            h.process_block(&mut [0.0f32; 64], 48_000);
            (h.sung(), h.intervals()[0])
        };
        let (c, from_c) = third_over(261.63);
        assert_eq!(c, Some(60), "C4 is heard as C4");
        assert!((from_c - 4.0).abs() < 0.01, "E over C: {from_c}");
        let (d, from_d) = third_over(293.66);
        assert_eq!(d, Some(62), "D4 is heard as D4");
        assert!((from_d - 3.0).abs() < 0.01, "F over D, not F#: {from_d}");
    }

    /// **The harmony moves with the singer, soon.** From C to D, the voices
    /// are measured from the new note within 45 ms — it took 69 ms with the
    /// autotune's 64 ms window, which is most of a sixteenth at 120 bpm of the
    /// old harmony sung on the new note.
    #[test]
    fn a_new_note_is_heard_within_45_ms() {
        let sr = 48_000.0f32;
        let mut h = Harmonizer::new(48_000);
        let mut phase = 0.0f32;
        let mut feed = |h: &mut Harmonizer, hz: f32| {
            let mut buf = Vec::with_capacity(512);
            for _ in 0..256 {
                let s = (phase * std::f32::consts::TAU).sin() * 0.4;
                phase = (phase + hz / sr).fract();
                buf.push(s);
                buf.push(s);
            }
            h.process_block(&mut buf, 48_000);
        };
        for _ in 0..100 {
            feed(&mut h, 261.63);
        }
        assert_eq!(h.sung(), Some(60));
        let blocks = (1..=40)
            .find(|_| {
                feed(&mut h, 293.66);
                h.sung() == Some(62)
            })
            .expect("D is heard at all");
        let ms = blocks as f32 * 256.0 / 48.0;
        assert!(ms <= 45.0, "the harmony took {ms:.1} ms to follow");
        // And a low voice is still read at its own octave in the shorter window.
        let mut h = Harmonizer::new(48_000);
        for _ in 0..100 {
            feed(&mut h, 82.41); // E2
        }
        assert_eq!(h.sung(), Some(40), "E2 is not the octave above");
    }

    /// **Two voices never share a note** — not the note each other has, and
    /// not the sung one — whatever is sung over whatever chord. A third and a
    /// fifth snapped onto a three-note chord both landed on its fifth, a few
    /// cents apart, and the harmony beat like a chorus.
    #[test]
    fn two_voices_never_share_a_note() {
        let _chart = crate::test_locks::chart();
        let mut h = Harmonizer::new(48_000);
        h.set_voices(0.667); // four
        h.shape = Shape::Thirds;
        h.detune = 0.0;
        h.set_param(CHART_PARAM, 1.0);
        for chord in [[48u8, 52, 55], [45, 48, 52], [41, 45, 48], [43, 47, 50]] {
            crate::chord::chart().set(&chord);
            for sung in 48..=72 {
                h.sung = Some(sung);
                h.rebuild();
                let notes: Vec<i32> = h
                    .intervals()
                    .iter()
                    .map(|s| sung + s.round() as i32)
                    .collect();
                let mut unique = notes.clone();
                unique.sort_unstable();
                unique.dedup();
                assert_eq!(
                    unique.len(),
                    notes.len(),
                    "{chord:?} over {sung}: {notes:?}"
                );
                assert!(
                    !notes.contains(&sung),
                    "a voice on the sung note: {notes:?}"
                );
            }
        }
        crate::chord::chart().clear();
    }

    /// The log's counters: what happened since the last reading, and a clean
    /// start after it.
    #[test]
    fn the_stats_count_since_the_last_reading() {
        let s = HarmStats {
            blocks: AtomicU32::new(0),
            notes: AtomicU32::new(0),
            last_note: AtomicU32::new(u32::MAX),
            rebuilds: AtomicU32::new(0),
            peak: AtomicU32::new(0),
            over: AtomicU32::new(0),
            intervals: [const { AtomicU32::new(0) }; MAX_VOICES],
            voices: AtomicU32::new(0),
        };
        assert_eq!(s.take().sung, None, "nothing heard yet");
        s.note(64);
        s.rebuilt(&[3.0, 7.0]);
        s.block(0.4, 0);
        s.block(1.2, 3);
        let r = s.take();
        assert_eq!(
            (r.blocks, r.notes, r.sung, r.rebuilds, r.over),
            (2, 1, Some(64), 1, 3)
        );
        assert!((r.peak - 1.2).abs() < 1e-6);
        assert_eq!(&r.intervals[..r.voices], &[3.0, 7.0]);
        let again = s.take();
        assert_eq!((again.blocks, again.notes, again.over), (0, 0, 0));
        assert_eq!(
            again.sung,
            Some(64),
            "the note heard is kept, the counts are not"
        );
    }

    /// **A second of silence forgets the note**; half a second between two
    /// phrases does not.
    #[test]
    fn a_silence_forgets_the_note_it_followed() {
        let sr = 48_000.0f32;
        let mut h = Harmonizer::new(48_000);
        let mut buf = tone(261.63, sr, 24_000);
        h.process_block(&mut buf, 48_000);
        assert_eq!(h.sung(), Some(60));
        let quiet = |h: &mut Harmonizer, ms: usize| {
            for _ in 0..ms * 48 / 256 {
                h.process_block(&mut [0.0f32; 512], 48_000);
            }
        };
        quiet(&mut h, 500);
        assert_eq!(h.sung(), Some(60), "a breath between phrases keeps it");
        quiet(&mut h, 700);
        assert_eq!(h.sung(), None, "a second and more is nothing heard");
    }

    /// Octaves are octaves in every scale: twelve semitones, whatever the
    /// scale has in it. Seven steps was a fifth in chromatic and an octave and
    /// a second in a pentatonic.
    #[test]
    fn an_octave_is_an_octave_in_every_scale() {
        for (i, _) in ScaleType::ALL.iter().enumerate() {
            let mut h = Harmonizer::new(48_000);
            h.set_voices(0.334);
            h.shape = Shape::Octaves;
            h.detune = 0.0;
            h.set_scale(i as f32 / (ScaleType::ALL.len() - 1) as f32);
            h.rebuild();
            let iv = h.intervals();
            assert_eq!(iv, vec![12.0, -12.0], "scale {i}: {iv:?}");
        }
    }

    /// **A chart's chord decides the notes once something is sung**: the
    /// voices land on the chord's tones nearest the shape's intervals, and a
    /// new chord in the chart is a new harmony without anything played on a
    /// keyboard.
    #[test]
    fn the_chart_chord_is_the_harmony_under_the_voice() {
        let _chart = crate::test_locks::chart();
        let mut h = Harmonizer::new(48_000);
        h.set_voices(0.334); // two
        h.shape = Shape::Thirds; // a third and a fifth above
        h.detune = 0.0;
        h.set_param(CHART_PARAM, 1.0);
        assert!(h.follows_chart());
        // Sing a G.
        let mut buf = tone(392.0, 48_000.0, 24_000);
        crate::chord::chart().set(&[48, 52, 55]); // C major
        h.process_block(&mut buf, 48_000);
        h.process_block(&mut [0.0f32; 64], 48_000);
        assert_eq!(h.sung(), Some(67));
        // Over G in C: a third above is B → nearest C-chord tone towards G is
        // G itself, which is the sung note, so C; the fifth D → C or E, E.
        let c = h.intervals();
        let notes: Vec<i32> = c
            .iter()
            .map(|s| (67 + s.round() as i32).rem_euclid(12))
            .collect();
        assert!(
            notes.iter().all(|n| [0, 4, 7].contains(n)),
            "C-chord tones: {c:?}"
        );
        // The chart moves on to E minor: the voices move with it.
        crate::chord::chart().set(&[52, 55, 59]);
        h.process_block(&mut [0.0f32; 64], 48_000);
        let e = h.intervals();
        let notes: Vec<i32> = e
            .iter()
            .map(|s| (67 + s.round() as i32).rem_euclid(12))
            .collect();
        assert!(
            notes.iter().all(|n| [4, 7, 11].contains(n)),
            "Em tones: {e:?}"
        );
        assert_ne!(c, e, "a new chord is a new harmony");
        crate::chord::chart().clear();
    }

    /// The voices actually sound, at the pitches they were told to.
    #[test]
    fn lead_brings_back_the_singer_and_nothing_else() {
        // The same harmony run twice, lead gone and lead whole: what separates
        // the two is the singer, and nothing else.
        let run = |lead: f32| {
            let mut h = Harmonizer::new(48_000);
            h.set_voices(0.334);
            h.shape = Shape::Octaves;
            h.set_mix(1.0);
            h.set_param(LEAD_PARAM, lead);
            let dry: Vec<f32> = (0..24_000)
                .flat_map(|i| {
                    let s = (std::f32::consts::TAU * 220.0 * i as f32 / 48_000.0).sin() * 0.3;
                    [s, s]
                })
                .collect();
            let mut buf = dry.clone();
            for block in buf.chunks_mut(1024) {
                h.process_block(block, 48_000);
            }
            (buf, dry, h.balance)
        };
        let (with, dry, b1) = run(1.0);
        let (without, _, b0) = run(0.0);
        let worst = with
            .iter()
            .zip(&without)
            .zip(&dry)
            .map(|((w, o), d)| (w / b1 - o / b0 - d).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 1e-4, "Lead moved more than the singer: {worst}");
        assert!(
            without.iter().any(|x| x.abs() > 0.01),
            "the harmony is there"
        );
    }

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

        // Rich in harmonics, as a voice is: the shifter keeps the spectrum's
        // shape (the formants) and moves the harmonics under it, so a pure
        // sine has nothing to move — its one "formant" is itself.
        let dry: Vec<f32> = (0..48_000)
            .flat_map(|i| {
                let t = i as f32 / sr;
                let s: f32 = (1..=12)
                    .map(|k| (std::f32::consts::TAU * 300.0 * k as f32 * t).sin() / k as f32)
                    .sum::<f32>()
                    * 0.2;
                [s, s]
            })
            .collect();
        let mut buf = dry.clone();
        // In blocks, as the engine runs it: the grains are cut at the period
        // the detector heard by the end of the previous block.
        for block in buf.chunks_mut(1024) {
            h.process_block(block, 48_000);
        }
        // The harmony alone is what comes out.
        let harmony: Vec<f32> = buf.iter().map(|o| o / h.balance).collect();
        let tail = &harmony[24_000 * 2..];
        //
        // Read against a frequency neither voice is at, rather than against an
        // absolute: the shifter's crossfade puts a slight warble on a held
        // tone, which spreads a half-second reading across neighbouring bins
        // and makes every absolute number look small. What has to be true is
        // that there is energy where the voices are and none where they are
        // not.
        // Chromatic `Octaves` is ±12: an octave up and down.
        let up = energy_at(tail, 600.0, sr);
        let down = energy_at(tail, 150.0, sr);
        // Between harmonics of every note in play (150, 300, 600 Hz).
        let nowhere = energy_at(tail, 1_575.0, sr);
        assert!(
            up > nowhere * 8.0 && down > nowhere * 8.0,
            "both voices sound: up={up} down={down}, floor={nowhere}"
        );
        // And the harmony is the other notes, not a copy of the one sung.
        let original = energy_at(tail, 300.0, sr);
        assert!(
            original < up.max(down),
            "the harmony is the sung note again: {original}"
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
    /// Over a chord, the parts move the least they can — a choir, not a
    /// parallel shift. The tune leaps a fifth under a held C chord: a voice
    /// on E has E still in the chord and within its register, so it holds it.
    /// Snapped from "the tune plus the interval" it jumped to C, eight
    /// semitones away.
    #[test]
    fn over_a_chord_the_voices_move_the_least_they_can() {
        let _chart = crate::test_locks::chart();
        let mut h = Harmonizer::new(48_000);
        h.set_voices(0.334); // two
        h.shape = Shape::Thirds;
        h.detune = 0.0;
        h.set_param(CHART_PARAM, 1.0);
        crate::chord::chart().set(&[48, 52, 55]); // C major
        let voiced = |h: &mut Harmonizer, sung: i32| -> Vec<i32> {
            h.sung = Some(sung);
            h.dirty = true;
            h.rebuild();
            h.voices.iter().take(2).map(|v| v.note.unwrap()).collect()
        };
        let before = voiced(&mut h, 60);
        let after = voiced(&mut h, 67);
        crate::chord::chart().clear();
        let moved: i32 = before.iter().zip(&after).map(|(a, b)| (a - b).abs()).sum();
        for n in &after {
            assert!(
                [0, 4, 7].contains(&n.rem_euclid(12)),
                "chord tones: {after:?}"
            );
            assert_ne!(n.rem_euclid(12), 7, "not doubling the tune");
        }
        // Two parts, a fifth leap in the tune: they move at most a few
        // semitones between them, not in parallel with it.
        assert!(
            moved <= 6,
            "{before:?} → {after:?}: {moved} semitones of movement"
        );
    }

    /// A forgotten phrase forgets its parts too: the next one starts where
    /// the shape puts them.
    #[test]
    fn a_new_phrase_starts_its_parts_afresh() {
        let mut h = Harmonizer::new(48_000);
        h.sung = Some(60);
        h.dirty = true;
        h.rebuild();
        assert!(h.voices[0].note.is_some());
        h.reset();
        assert!(h.voices.iter().all(|v| v.note.is_none()));
    }

    /// An octave far from the tune does not take the voices with it: sung at
    /// C#6 with the octave on C1, a voice was put 51 semitones down — 30 Hz,
    /// past what the shifter reaches and under what a headset plays, so the
    /// harmony vanished and only the lead was heard. It stays within two
    /// octaves of the voice, keeping its note.
    #[test]
    fn a_far_octave_does_not_bury_the_voices() {
        let mut h = Harmonizer::new(48_000);
        h.set_voices(0.334);
        h.shape = Shape::Thirds;
        h.detune = 0.0;
        h.set_param(OCTAVE_PARAM, octave_norm(Some(1)));
        h.sung = Some(85);
        h.dirty = true;
        h.rebuild();
        for v in h.voices.iter().take(2) {
            let shift = v.semitones.round() as i32;
            assert!(shift.abs() <= MAX_FOLD, "a voice {shift} semitones away");
        }
    }

    /// Octave: every voice keeps its note and sings it inside the octave
    /// asked for, wherever the tune is. AUTO is what it always was.
    #[test]
    fn the_octave_puts_every_voice_in_its_register() {
        let voiced = |octave: f32, sung: i32| -> Vec<i32> {
            let mut h = Harmonizer::new(48_000);
            h.set_voices(0.334); // two
            h.shape = Shape::Thirds;
            h.detune = 0.0;
            h.set_param(OCTAVE_PARAM, octave);
            h.sung = Some(sung);
            h.dirty = true;
            h.rebuild();
            h.voices
                .iter()
                .take(2)
                .map(|v| sung + v.semitones.round() as i32)
                .collect()
        };
        let auto = voiced(0.0, 64);
        let c3 = voiced(octave_norm(Some(3)), 64);
        assert!(
            auto.iter().all(|n| (64..=76).contains(n)),
            "AUTO follows the tune: {auto:?}"
        );
        for (a, c) in auto.iter().zip(&c3) {
            assert_eq!(
                a.rem_euclid(12),
                c.rem_euclid(12),
                "same notes: {auto:?} {c3:?}"
            );
            assert!((48..60).contains(c), "inside C3–B3: {c3:?}");
        }
        // Wherever the tune goes — within two octaves — the voices stay there.
        // Further than that they follow it; see the test below.
        assert!(voiced(octave_norm(Some(3)), 70)
            .iter()
            .all(|n| (48..60).contains(n)));
        assert_eq!(octave_of(octave_norm(Some(5))), Some(5));
        assert_eq!(octave_of(0.0), None);
        assert_eq!(octave_label(Some(4)), "C4");
    }

    /// CHORD sings the species picked on the `Ch…` knobs over the note —
    /// exactly, whatever the key says — and starts on a major seventh, which
    /// is what the MAJ7 shape it replaced sang.
    #[test]
    fn the_chord_shape_sings_the_chosen_species() {
        let voices = |h: &mut Harmonizer| -> Vec<i32> {
            h.set_voices(0.667); // four
            h.detune = 0.0;
            h.sung = Some(60);
            h.dirty = true;
            h.rebuild();
            h.voices
                .iter()
                .take(3)
                .map(|v| v.semitones.round() as i32)
                .collect()
        };
        let mut h = Harmonizer::new(48_000);
        assert_eq!(h.shape, Shape::Chord);
        assert_eq!(voices(&mut h), vec![4, 7, 11], "a major seventh by default");
        // Minor seven flat five, in a C major key that has neither of its notes.
        h.set_param(SPEC_PARAM0, spec_norm(1, SPEC_ROWS[0].1)); // MIN
        h.set_param(SPEC_PARAM0 + 1, spec_norm(2, SPEC_ROWS[1].1)); // 7
        h.set_param(SPEC_PARAM0 + 6, spec_norm(1, SPEC_ROWS[6].1)); // b5
        assert_eq!(
            voices(&mut h),
            vec![3, 6, 10],
            "m7b5 over the note, not bent to the key"
        );
    }

    /// ArrSync follows the chart channel the arranger's chord is published
    /// on, as the chart switch does — and says so.
    #[test]
    fn arrsync_follows_the_published_progression() {
        let _chart = crate::test_locks::chart();
        let mut h = Harmonizer::new(48_000);
        assert!(!h.follows_chart());
        h.set_param(ARR_SYNC_PARAM, 1.0);
        assert!(h.follows_chart() && h.follows_arranger());
        crate::chord::chart().set(&[50, 53, 57]); // Dm
        let mut held = [0u8; crate::chord::MAX_NOTES];
        assert_eq!(h.held(&mut held), 3, "it reads the progression's chord");
        crate::chord::chart().clear();
    }

    #[test]
    fn a_held_chord_becomes_the_harmony() {
        let _chord = crate::test_locks::chord();
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
