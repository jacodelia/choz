//! Arpeggiator: held keys in, a pattern of notes out.
//!
//! # Why it is not an FX
//!
//! `choz_ports::FxProcessor::process_block` takes interleaved audio and returns
//! interleaved audio. There is no note in that signature and no place to put
//! one, so an arpeggiator cannot be a slot in the FX chain however much it
//! looks like an effect. It belongs where notes are decided, which today is the
//! UI thread: that is where routing is resolved (`note_targets`/`targets_for`),
//! where MIDI learn lives, and where the project is written.
//!
//! # What that costs
//!
//! The UI loop is the clock, so a step lands within one pass of it. The loop
//! polls faster while an arpeggiator is running for exactly this reason, but
//! this is still software timing on a thread that also draws: the sample-exact
//! version runs in the engine against the transport, and is the next step, not
//! a different design — [`Arp::tick`] already takes the current instant rather
//! than reading a clock itself, so it can be driven from anywhere.
//!
//! Every note this emits is paired: [`Arp`] remembers what it started and stops
//! it, and [`Arp::silence`] is what `PANIC` calls. A generator that forgets a
//! note-off is a stuck note that no amount of releasing keys will clear.

use crate::source::ParamShape;
use std::time::{Duration, Instant};

/// The order the held notes are played in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ArpMode {
    #[default]
    Up,
    Down,
    /// Up then back down **repeating** the turning notes — the KeyStep calls
    /// this Inclusive, and it is the one that swings evenly on a triad.
    Inclusive,
    /// Up then back down without repeating them (the KeyStep's Exclusive).
    UpDown,
    /// The order the keys were pressed in — the one that lets a player phrase.
    /// The KeyStep calls it Order.
    AsPlayed,
    /// Deterministic: same seed, same sequence, so a bug is reproducible.
    Random,
    /// Each note twice on the way up, and on the way down.
    UpX2,
    DownX2,
}

impl ArpMode {
    /// In the KeyStep's own order, which is the order a player who knows one
    /// expects to walk them in.
    pub const ALL: [ArpMode; 8] = [
        ArpMode::Up,
        ArpMode::Down,
        ArpMode::Inclusive,
        ArpMode::UpDown,
        ArpMode::Random,
        ArpMode::AsPlayed,
        ArpMode::UpX2,
        ArpMode::DownX2,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ArpMode::Up => "UP",
            ArpMode::Down => "DOWN",
            ArpMode::Inclusive => "INCL",
            ArpMode::UpDown => "EXCL",
            ArpMode::AsPlayed => "ORDER",
            ArpMode::Random => "RANDOM",
            ArpMode::UpX2 => "UP\u{00D7}2",
            ArpMode::DownX2 => "DN\u{00D7}2",
        }
    }
}

/// How long a step lasts, as a fraction of a quarter note.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum TimeDiv {
    Quarter,
    QuarterTriplet,
    Eighth,
    EighthTriplet,
    #[default]
    Sixteenth,
    SixteenthTriplet,
    ThirtySecond,
    ThirtySecondTriplet,
}

impl TimeDiv {
    /// The KeyStep's eight positions, in its order: each value and its triplet.
    pub const ALL: [TimeDiv; 8] = [
        TimeDiv::Quarter,
        TimeDiv::QuarterTriplet,
        TimeDiv::Eighth,
        TimeDiv::EighthTriplet,
        TimeDiv::Sixteenth,
        TimeDiv::SixteenthTriplet,
        TimeDiv::ThirtySecond,
        TimeDiv::ThirtySecondTriplet,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TimeDiv::Quarter => "1/4",
            TimeDiv::QuarterTriplet => "1/4T",
            TimeDiv::Eighth => "1/8",
            TimeDiv::EighthTriplet => "1/8T",
            TimeDiv::Sixteenth => "1/16",
            TimeDiv::SixteenthTriplet => "1/16T",
            TimeDiv::ThirtySecond => "1/32",
            TimeDiv::ThirtySecondTriplet => "1/32T",
        }
    }

    /// In quarter notes. A triplet is three in the space of two, so an eighth
    /// triplet is a third of a quarter, not two thirds of an eighth.
    pub fn quarters(self) -> f32 {
        match self {
            TimeDiv::Quarter => 1.0,
            TimeDiv::QuarterTriplet => 2.0 / 3.0,
            TimeDiv::Eighth => 0.5,
            TimeDiv::EighthTriplet => 1.0 / 3.0,
            TimeDiv::Sixteenth => 0.25,
            TimeDiv::SixteenthTriplet => 1.0 / 6.0,
            TimeDiv::ThirtySecond => 0.125,
            TimeDiv::ThirtySecondTriplet => 1.0 / 12.0,
        }
    }
}

/// What the arpeggiator wants done, in the order it wants it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArpEvent {
    On { note: u8, vel: u8 },
    Off { note: u8 },
}

/// What the arpeggiator is: saved with the project, because it is part of how a
/// tab sounds, not of a session.
///
/// `Copy`, deliberately: the panel is handed a snapshot of it every frame.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ArpSettings {
    pub on: bool,
    pub mode: ArpMode,
    pub div: TimeDiv,
    /// Its own tempo, used while [`Self::sync`] is off.
    pub bpm: f32,
    /// Follow choz's transport instead of the tempo above.
    ///
    /// While the transport is **playing**, the steps land on its grid — the
    /// beat is read from the song position, so two tabs stay in phase with each
    /// other and with a tempo-synced plugin, and a busy UI thread cannot drag
    /// them off it. While it is **stopped** the grid has nothing to lock to, so
    /// the arpeggiator free-runs at the transport's tempo: somebody holding a
    /// chord with the transport stopped wants to hear it, not to be told why
    /// they cannot.
    #[serde(default)]
    pub sync: bool,
    /// Fraction of a step the note is held, 0.05..1.0.
    pub gate: f32,
    /// How far the off-beats are pushed late, 0.0..0.75 of a step.
    pub swing: f32,
    /// How many octaves the pattern climbs, 1..4.
    pub octaves: u8,
    /// Keep playing after the keys are released, until new keys arrive.
    pub latch: bool,
    /// One key plays a memorised chord, transposed.
    ///
    /// Switching it on **while holding a chord** is what memorises that chord;
    /// switching it on with nothing down keeps whatever was memorised last, so
    /// it is a mode and not a gesture that has to be repeated.
    #[serde(default)]
    pub chord: bool,
}

impl Default for ArpSettings {
    fn default() -> Self {
        Self {
            on: false,
            mode: ArpMode::Up,
            div: TimeDiv::Sixteenth,
            bpm: 120.0,
            sync: false,
            gate: 0.5,
            swing: 0.0,
            octaves: 1,
            latch: false,
            chord: false,
        }
    }
}

/// One tab's arpeggiator: the settings, the keys being held, and the note it
/// currently has sounding.
#[derive(Debug, Clone)]
pub struct Arp {
    pub settings: ArpSettings,
    /// Held keys in the order they were pressed — `AsPlayed` needs the order,
    /// and the other modes sort a copy.
    held: Vec<(u8, u8)>,
    step: usize,
    /// The note this arpeggiator started and has not stopped yet.
    sounding: Option<u8>,
    /// When the next step is due, and when the current note should be released.
    next_step: Option<Instant>,
    off_at: Option<Instant>,
    /// xorshift, so `Random` is a sequence rather than a surprise.
    rng: u32,
    /// The last few taps, for the tempo they imply.
    taps: Vec<Instant>,
    /// The transport step last fired while synced, so the same one is not
    /// played twice on the next tick.
    grid: Option<i64>,
    /// The memorised chord as semitones above its lowest note, so it can be
    /// played from any key. Empty until a chord is memorised.
    chord: Vec<i16>,
}

/// The snapshot the panel draws from.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ArpView<'a> {
    pub settings: ArpSettings,
    /// The memorised chord, as semitones above its root — so the panel can say
    /// how many notes one key is about to play.
    pub chord: &'a [i16],
    /// Which knob of the box the arrows are on, and whether they are on this
    /// box at all. Both belong to the interface rather than to the machine, so
    /// `Arp::view` leaves them at zero and the panel's caller fills them in.
    pub cursor: usize,
    pub focused: bool,
}

impl ArpView<'_> {
    /// The knobs the panel draws.
    pub fn knobs(&self) -> Vec<(ArpParam, &'static str, f32, ParamShape)> {
        self.settings.knob_list()
    }
}

impl Default for Arp {
    fn default() -> Self {
        Self {
            settings: ArpSettings::default(),
            held: Vec::new(),
            step: 0,
            sounding: None,
            next_step: None,
            off_at: None,
            rng: 0x9E37_79B9,
            taps: Vec::new(),
            grid: None,
            chord: Vec::new(),
        }
    }
}

impl Arp {
    pub fn new(settings: ArpSettings) -> Self {
        Self {
            settings,
            ..Self::default()
        }
    }

    pub fn is_on(&self) -> bool {
        self.settings.on
    }

    /// Whether it currently has anything to play — what the UI loop asks to
    /// decide how fast to come back.
    pub fn running(&self) -> bool {
        self.settings.on && !self.held.is_empty()
    }

    /// A key went down. The first key of a new chord starts the pattern
    /// immediately, so the first note is heard when it is played rather than on
    /// the next step boundary.
    pub fn note_on(&mut self, note: u8, vel: u8, now: Instant) {
        if self.settings.latch && self.next_step.is_none() {
            self.held.clear();
        }
        self.held.retain(|(n, _)| *n != note);
        self.held.push((note, vel));
        // Chord mode: the key is the root, and the memorised intervals come
        // with it. They go into `held` like any other key, so every mode, the
        // octave stack and the latch keep working on them unchanged.
        if self.settings.chord {
            for interval in self.chord.clone() {
                let Some(n) = transposed(note, interval) else {
                    continue;
                };
                if n != note && !self.held.iter().any(|(h, _)| *h == n) {
                    self.held.push((n, vel));
                }
            }
        }
        if self.next_step.is_none() {
            self.step = 0;
            self.next_step = Some(now);
        }
    }

    /// A key came up. With `latch` on the pattern keeps its notes.
    pub fn note_off(&mut self, note: u8) {
        if self.settings.latch {
            return;
        }
        self.held.retain(|(n, _)| *n != note);
        // Chord mode: the notes this key brought with it go up with it. They
        // were never pressed, so nothing else will ever take them off.
        if self.settings.chord {
            let companions: Vec<u8> = self
                .chord
                .iter()
                .filter_map(|i| transposed(note, *i))
                .collect();
            self.held.retain(|(n, _)| !companions.contains(n));
        }
    }

    /// Memorise the notes held right now as the chord one key will play.
    ///
    /// Nothing held leaves the last one alone: turning the mode on to use the
    /// chord you already have is the common case, and wiping it would make the
    /// switch destructive.
    pub fn memorise_chord(&mut self) {
        let mut notes: Vec<u8> = self.held.iter().map(|(n, _)| *n).collect();
        notes.sort_unstable();
        notes.dedup();
        if notes.len() < 2 {
            return;
        }
        let root = notes[0] as i16;
        self.chord = notes.iter().map(|n| *n as i16 - root).collect();
    }

    /// The memorised chord, as semitones above its root.
    pub fn chord(&self) -> &[i16] {
        &self.chord
    }

    /// Advance to `now`, appending what has to be sent.
    ///
    /// Takes the instant rather than reading the clock so the whole thing can
    /// be tested without sleeping, and so the engine can drive it later.
    pub fn tick(&mut self, now: Instant, out: &mut Vec<ArpEvent>) {
        if !self.settings.on {
            self.silence(out);
            return;
        }
        if self.held.is_empty() {
            // Nothing held: stop cleanly and forget where we were, so the next
            // chord starts on its first note instead of mid-pattern.
            self.silence(out);
            self.next_step = None;
            return;
        }
        // The gate closes even if the next step is far away.
        if let Some(at) = self.off_at {
            if now >= at {
                self.release(out);
            }
        }
        // Synced: the transport says which step it is, and a step is due when
        // that number changes. The free-running clock below is what runs when
        // there is no transport to follow.
        let due = match self.grid_step() {
            Some(index) => {
                if self.grid == Some(index) {
                    return;
                }
                self.grid = Some(index);
                now
            }
            None => {
                self.grid = None;
                let Some(due) = self.next_step else {
                    self.next_step = Some(now);
                    return;
                };
                if now < due {
                    return;
                }
                due
            }
        };

        let sequence = self.sequence();
        if sequence.is_empty() {
            return;
        }
        let idx = self.pick(sequence.len());
        let (note, vel) = sequence[idx];

        self.release(out);
        out.push(ArpEvent::On { note, vel });
        self.sounding = Some(note);

        let step = self.step_len();
        // Swing pushes every other step late. Applied to *this* step's length
        // so the pair still adds up to two straight steps — a swing that
        // stretched both would just be a slower tempo.
        let swung = if self.step % 2 == 1 {
            step.mul_f32(1.0 + self.settings.swing)
        } else {
            step.mul_f32(1.0 - self.settings.swing)
        };
        self.off_at = Some(now + swung.mul_f32(self.settings.gate.clamp(0.05, 1.0)));
        // From `due`, not from `now`: a late tick must not push the grid, or a
        // busy UI thread would drag the tempo down over time.
        self.next_step = Some(due + swung);
        // …but never so far behind that it fires several steps in a row to
        // catch up. A stall is a stall; catching up would sound like a burst.
        if let Some(next) = self.next_step {
            if next < now {
                self.next_step = Some(now + swung);
            }
        }
        self.step = self.step.wrapping_add(1);
    }

    /// Tap the tempo. The average of the last few intervals, so one bad tap
    /// does not throw it; a gap longer than two seconds starts a new count,
    /// because that was somebody stopping rather than playing very slowly.
    pub fn tap(&mut self, now: Instant) {
        const MAX_TAPS: usize = 4;
        if self
            .taps
            .last()
            .is_some_and(|t| now.saturating_duration_since(*t) > Duration::from_secs(2))
        {
            self.taps.clear();
        }
        self.taps.push(now);
        if self.taps.len() > MAX_TAPS {
            self.taps.remove(0);
        }
        if self.taps.len() < 2 {
            return;
        }
        let total = self
            .taps
            .last()
            .unwrap()
            .saturating_duration_since(self.taps[0])
            .as_secs_f32();
        let intervals = (self.taps.len() - 1) as f32;
        if total <= 0.0 {
            return;
        }
        // Taps are quarter notes, whatever the division is: that is what a tap
        // tempo means on every box that has one.
        let bpm = (60.0 / (total / intervals)).clamp(MIN_BPM, MAX_BPM);
        // Synced there is one clock, and this is somebody asking *it* to go
        // faster: writing a number the arpeggiator is not counting at would be
        // a tap that does nothing.
        if self.settings.sync {
            choz_ports::transport().set_bpm(bpm);
        } else {
            self.settings.bpm = bpm;
        }
    }

    /// Stop whatever is sounding. `PANIC`, and switching the arpeggiator off.
    pub fn silence(&mut self, out: &mut Vec<ArpEvent>) {
        self.release(out);
        self.off_at = None;
    }

    /// Everything up and nothing held: the reset a tab gets when its instrument
    /// changes under it.
    pub fn reset(&mut self, out: &mut Vec<ArpEvent>) {
        self.silence(out);
        self.held.clear();
        self.next_step = None;
        self.step = 0;
    }

    /// The snapshot the panel draws from.
    pub fn view(&self) -> ArpView<'_> {
        ArpView {
            settings: self.settings,
            chord: self.chord(),
            cursor: 0,
            focused: false,
        }
    }

    fn release(&mut self, out: &mut Vec<ArpEvent>) {
        if let Some(note) = self.sounding.take() {
            out.push(ArpEvent::Off { note });
        }
    }

    fn step_len(&self) -> Duration {
        Duration::from_secs_f32(60.0 / self.bpm() * self.settings.div.quarters())
    }

    /// The tempo the steps are counted at: the transport's while `SYNC` is on,
    /// its own otherwise.
    pub fn bpm(&self) -> f32 {
        self.settings.tempo()
    }

    /// Which step of the transport's grid the playhead is on, or `None` when
    /// there is no grid to follow — `SYNC` off, or a transport that is not
    /// running.
    ///
    /// This is the whole of "synced": the step number comes from the song
    /// position rather than from counting durations, so nothing accumulates.
    /// A stall skips steps instead of firing a burst to catch up, which is the
    /// same rule the free-running clock follows and for the same reason.
    fn grid_step(&self) -> Option<i64> {
        if !self.settings.sync {
            return None;
        }
        let transport = choz_ports::transport();
        if !transport.playing() {
            return None;
        }
        let step_q = self.settings.div.quarters() as f64;
        if step_q <= 0.0 {
            return None;
        }
        let position = transport.ppq() / step_q;
        let index = position.floor() as i64;
        // Swing pushes the off-beats late, so the grid has not reached an odd
        // step until that share of the step has gone by: until then the
        // playhead still belongs to the one before it.
        let into_step = position - index as f64;
        if index % 2 == 1 && into_step < self.settings.swing as f64 {
            return Some(index - 1);
        }
        Some(index)
    }

    /// The notes to walk, with the octaves stacked on top.
    fn sequence(&self) -> Vec<(u8, u8)> {
        let mut base: Vec<(u8, u8)> = self.held.clone();
        match self.settings.mode {
            ArpMode::AsPlayed | ArpMode::Random => {}
            ArpMode::Down | ArpMode::DownX2 => {
                base.sort_by_key(|(n, _)| std::cmp::Reverse(*n));
            }
            _ => base.sort_by_key(|(n, _)| *n),
        }

        let octaves = self.settings.octaves.clamp(1, 4);
        let mut out: Vec<(u8, u8)> = Vec::with_capacity(base.len() * octaves as usize);
        for o in 0..octaves {
            for &(n, v) in &base {
                // A note pushed past the top of MIDI is dropped, not wrapped:
                // wrapping would play a bass note in the middle of a climb.
                if let Some(n) = n.checked_add(o * 12).filter(|n| *n < 128) {
                    out.push((n, v));
                }
            }
        }
        match self.settings.mode {
            // Exclusive: back down without repeating the turning notes, which
            // is what makes it a cycle rather than a stutter at each end.
            ArpMode::UpDown if out.len() > 2 => {
                let middle: Vec<(u8, u8)> = out[1..out.len() - 1].iter().rev().copied().collect();
                out.extend(middle);
            }
            // Inclusive: the turning notes *are* repeated, so every note of the
            // chord gets the same share of the cycle.
            ArpMode::Inclusive if out.len() > 1 => {
                let back: Vec<(u8, u8)> = out.iter().rev().copied().collect();
                out.extend(back);
            }
            // Each note twice, up or down: the sort above already picked the
            // direction, so both are the same doubling.
            ArpMode::UpX2 | ArpMode::DownX2 => {
                out = out.iter().flat_map(|n| [*n, *n]).collect();
            }
            _ => {}
        }
        out
    }

    fn pick(&mut self, len: usize) -> usize {
        if self.settings.mode == ArpMode::Random {
            self.rng ^= self.rng << 13;
            self.rng ^= self.rng >> 17;
            self.rng ^= self.rng << 5;
            (self.rng as usize) % len
        } else {
            self.step % len
        }
    }
}

/// A note moved by `semitones`, or `None` when that leaves MIDI's range.
fn transposed(note: u8, semitones: i16) -> Option<u8> {
    let moved = note as i16 + semitones;
    (0..128).contains(&moved).then_some(moved as u8)
}

// ─── The arpeggiator as knobs ───────────────────────────────────────────────

/// One control of the arpeggiator, as the RACK's knob box sees it.
///
/// The panel used to draw these as a row of buttons, which is fine for a switch
/// and wrong for a number: BPM, GATE and SWING are *values*, and a value wants
/// the same arc every other parameter in this program is drawn with. The knob
/// box is the design that already exists (`draw_knob_box`, shared by the FX and
/// the instrument), so the arpeggiator uses it rather than inventing a third
/// look.
///
/// The knobs are addressed by **what they are**, never by their position in the
/// list: a control inserted in the middle would otherwise move the meaning of
/// every one below it without anything failing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArpParam {
    /// Running or not. It is a knob like the rest because the box replaces the
    /// row the switch used to sit on, and that row is what a short screen does
    /// not have to spare.
    On,
    /// Follow the transport's tempo and grid.
    Sync,
    Mode,
    Div,
    Bpm,
    Gate,
    Swing,
    Octaves,
    Latch,
    /// One key plays the memorised chord.
    Chord,
}

/// The tempo range of the arpeggiator's own clock, which is also what the knob
/// spans.
pub const MIN_BPM: f32 = 20.0;
pub const MAX_BPM: f32 = 300.0;
/// A gate under this is a click rather than a note.
pub const MIN_GATE: f32 = 0.05;
/// Past this the off-beat swallows the on-beat.
pub const MAX_SWING: f32 = 0.75;

fn named(points: &[(f32, &str)]) -> ParamShape {
    ParamShape::Named(points.iter().map(|(v, l)| (*v, l.to_string())).collect())
}

/// Even positions for `n` named steps.
fn steps_of<T: Copy>(all: &[T], label: impl Fn(T) -> &'static str) -> ParamShape {
    let last = (all.len().max(2) - 1) as f32;
    ParamShape::Named(
        all.iter()
            .enumerate()
            .map(|(i, v)| (i as f32 / last, label(*v).to_string()))
            .collect(),
    )
}

impl ArpSettings {
    /// The knobs to draw, in order, with the value each one is at.
    fn knob_list(&self) -> Vec<(ArpParam, &'static str, f32, ParamShape)> {
        let out = vec![
            (
                ArpParam::On,
                "ARP",
                self.on as u8 as f32,
                ParamShape::Toggle,
            ),
            (
                ArpParam::Sync,
                "SYNC",
                self.sync as u8 as f32,
                ParamShape::Toggle,
            ),
            (
                ArpParam::Mode,
                "MODE",
                self.norm(ArpParam::Mode),
                steps_of(&ArpMode::ALL, ArpMode::label),
            ),
            (
                ArpParam::Div,
                "DIV",
                self.norm(ArpParam::Div),
                steps_of(&TimeDiv::ALL, TimeDiv::label),
            ),
            (
                ArpParam::Bpm,
                "BPM",
                self.norm(ArpParam::Bpm),
                ParamShape::Continuous,
            ),
            (
                ArpParam::Gate,
                "GATE",
                self.norm(ArpParam::Gate),
                ParamShape::Continuous,
            ),
            (
                ArpParam::Swing,
                "SWING",
                self.norm(ArpParam::Swing),
                ParamShape::Continuous,
            ),
            (
                ArpParam::Octaves,
                "OCT",
                self.norm(ArpParam::Octaves),
                named(&[(0.0, "1"), (1.0 / 3.0, "2"), (2.0 / 3.0, "3"), (1.0, "4")]),
            ),
            (
                ArpParam::Latch,
                "LATCH",
                self.latch as u8 as f32,
                ParamShape::Toggle,
            ),
            (
                ArpParam::Chord,
                "CHORD",
                self.chord as u8 as f32,
                ParamShape::Toggle,
            ),
        ];
        out
    }

    /// The tempo the steps are actually counted at: the transport's while
    /// `SYNC` is on, its own otherwise. What the panel prints and what the
    /// clock uses come from here, so they cannot disagree.
    pub fn tempo(&self) -> f32 {
        if self.sync {
            choz_ports::transport().bpm()
        } else {
            self.bpm
        }
        .clamp(MIN_BPM, MAX_BPM)
    }

    /// Where a control sits in 0..1 — the position the knob is drawn at.
    pub fn norm(&self, p: ArpParam) -> f32 {
        match p {
            ArpParam::On => self.on as u8 as f32,
            ArpParam::Sync => self.sync as u8 as f32,
            ArpParam::Mode => {
                let i = ArpMode::ALL
                    .iter()
                    .position(|m| *m == self.mode)
                    .unwrap_or(0);
                i as f32 / (ArpMode::ALL.len() - 1) as f32
            }
            ArpParam::Div => {
                let i = TimeDiv::ALL
                    .iter()
                    .position(|d| *d == self.div)
                    .unwrap_or(0);
                i as f32 / (TimeDiv::ALL.len() - 1) as f32
            }
            // Synced, the knob shows the clock it is following — its own
            // number would be a tempo nothing is playing at.
            ArpParam::Bpm => (self.tempo() - MIN_BPM) / (MAX_BPM - MIN_BPM),
            ArpParam::Gate => self.gate,
            ArpParam::Swing => self.swing / MAX_SWING,
            ArpParam::Octaves => (self.octaves.clamp(1, 4) - 1) as f32 / 3.0,
            ArpParam::Latch => self.latch as u8 as f32,
            ArpParam::Chord => self.chord as u8 as f32,
        }
        .clamp(0.0, 1.0)
    }

    /// Move a control to a 0..1 knob position.
    ///
    /// Returns whether the play mode changed: switching between keys and the
    /// sequence changes what everything else means, and whatever was sounding
    /// belongs to the mode being left — the caller has to stop it.
    #[must_use]
    pub fn set_norm(&mut self, p: ArpParam, v: f32) -> bool {
        let v = v.clamp(0.0, 1.0);
        let pick = |n: usize| ((v * (n - 1) as f32).round() as usize).min(n - 1);
        match p {
            ArpParam::On => self.on = v >= 0.5,
            ArpParam::Sync => self.sync = v >= 0.5,
            ArpParam::Mode => self.mode = ArpMode::ALL[pick(ArpMode::ALL.len())],
            ArpParam::Div => self.div = TimeDiv::ALL[pick(TimeDiv::ALL.len())],
            // Synced, the knob moves the transport: there is one clock, and
            // this is the tab asking for it to run faster.
            ArpParam::Bpm => {
                let bpm = MIN_BPM + v * (MAX_BPM - MIN_BPM);
                if self.sync {
                    choz_ports::transport().set_bpm(bpm);
                } else {
                    self.bpm = bpm;
                }
            }
            ArpParam::Gate => self.gate = v.max(MIN_GATE),
            ArpParam::Swing => self.swing = v * MAX_SWING,
            ArpParam::Octaves => self.octaves = pick(4) as u8 + 1,
            ArpParam::Latch => self.latch = v >= 0.5,
            // Handled by the caller, which has the machine and not just its
            // settings — switching sequences releases what was sounding.
            ArpParam::Chord => self.chord = v >= 0.5,
        }
        false
    }
}
