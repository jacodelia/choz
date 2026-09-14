//! The arranger: a backing band from a chord progression.
//!
//! # What it is
//!
//! Write a key, a style and a progression:
//!
//! ```text
//! key = C
//! style = major_blues
//!
//! || I7 | IV7 | I7 | I7 | IV7 | IV7 | I7 | VI7 | IIm7 | V7 | I7 V7 ||
//! ```
//!
//! and one of these plays a part of it — bass, drums or piano — against the
//! same clock everything else in choz runs off. Band-in-a-Box is the idea;
//! none of its code, formats or content is here.
//!
//! # Why it is not the sequencer
//!
//! [`crate::artifacts::seq`] is eight lanes of sixteen steps with one note per
//! lane for the whole project. That is a grid, and it is right for a drum
//! pattern; it has nowhere to put a walking bass, a four-note voicing or a
//! chord that changes half way through the bar. So this is a third artifact
//! beside the arpeggiator and the sequencer rather than a bigger `Seq` —
//! `SeqSettings`, `Pattern` and [`ArpEvent`] are untouched.
//!
//! # One role per instance
//!
//! An artifact lives in a tab and plays *one* instrument, so an instance is one
//! musician: [`generate::Role`] picks which. A band is a tab per player, all
//! reading the same text with the same seed — which is how choz already thinks
//! about a rack, and what the CLAP export can publish. The other way round — one
//! instance, a MIDI channel per role — would be tidier for channel 10 and is
//! not what the note bus carries: [`ArpEvent`] has no channel in it.
//!
//! # What runs where
//!
//! Parsing, harmony and generation happen **once**, on the thread that changed
//! the text, and produce a list of notes in beats. [`Arranger::tick`] reads that
//! list against the transport and emits [`ArpEvent`]s; it works out no music and
//! allocates nothing.

pub mod chord;
pub mod generate;
pub mod style;

use crate::artifacts::arp::ArpEvent;
use generate::{Note, Role};
use std::time::Instant;

/// What the arranger is, as the project stores it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ArrangerSettings {
    /// Playable at all. Off is what a tab starts as.
    pub on: bool,
    /// The progression, as written. The single source: the key and the style
    /// are read out of it rather than kept beside it and left to disagree.
    pub text: String,
    /// Which musician this tab is.
    pub role: Role,
    /// The interpretation. Same text, same style, same seed — same notes.
    pub seed: u32,
}

impl Default for ArrangerSettings {
    fn default() -> Self {
        Self {
            on: false,
            text: DEFAULT_TEXT.to_string(),
            role: Role::Bass,
            seed: 1,
        }
    }
}

/// A twelve-bar blues in C, which is the shortest thing that shows whether any
/// of this works.
pub const DEFAULT_TEXT: &str = "key = C\nstyle = major_blues\n\n|| I7 | IV7 | I7 | I7 | IV7 | IV7 | I7 | VI7 | IIm7 | V7 | I7 V7 | I7 ||";

/// A tab's arranger: the text, the part it baked from it, and where the
/// playhead is in it.
#[derive(Debug, Clone)]
pub struct Arranger {
    pub settings: ArrangerSettings,
    /// The part, in beats from the top. Rebuilt when anything it depends on
    /// moves, and only read from there on.
    baked: Vec<Note>,
    /// How long the whole progression is, in beats.
    total: f64,
    /// Why the text did not parse, for the panel to show. The last part that
    /// did parse keeps playing: a typo must not silence the band.
    error: Option<String>,
    style: style::Style,
    bars: usize,
    /// `(name, first bar)` of each part of the form, in playing order — what
    /// the panel says you are in. Empty for a text that named no parts.
    sections: Vec<(String, usize)>,
    playing: bool,
    /// Where the playhead is inside the progression, in beats.
    at: f64,
    /// The next note of `baked` that has not been played yet.
    cursor: usize,
    /// The transport position the last tick read, to turn into a step forward.
    last_pos: Option<f64>,
    /// The position its own clock is at, for when there is no transport
    /// rolling, and the instant it was last read at.
    free: f64,
    last_instant: Option<Instant>,
    /// What is sounding, and how many beats of it are left.
    sounding: Vec<(u8, f64)>,
}

impl Default for Arranger {
    fn default() -> Self {
        let mut arranger = Self {
            settings: ArrangerSettings::default(),
            baked: Vec::new(),
            total: 0.0,
            error: None,
            style: style::MAJOR_BLUES,
            bars: 0,
            sections: Vec::new(),
            playing: false,
            at: 0.0,
            cursor: 0,
            last_pos: None,
            free: 0.0,
            last_instant: None,
            sounding: Vec::new(),
        };
        arranger.rebake();
        arranger
    }
}

/// The snapshot the panel draws from, in the shape the other two artifacts
/// hand theirs over in: what is on the box, and nothing the box has to work out
/// again for itself.
#[derive(Debug, Clone, Copy)]
pub struct ArrangerView<'a> {
    pub settings: &'a ArrangerSettings,
    pub playing: bool,
    /// The style the text asked for, resolved.
    pub style: &'static str,
    pub bars: usize,
    /// Which bar of the progression is sounding, from 1. `0` when it is not
    /// playing.
    pub bar: usize,
    /// The part that bar belongs to — `A`, `INTRO`, whatever the text called
    /// it. `None` for a progression that named no parts, which is most of
    /// them.
    pub section: Option<&'a str>,
    /// Why the text did not read, if it did not.
    pub error: Option<&'a str>,
    /// Notes in this role's part — what tells "generated nothing" from "has not
    /// been switched on".
    pub notes: usize,
    /// Which of its controls has the arrows.
    pub cursor: usize,
    pub focused: bool,
}

/// Where a user's own styles live: `~/.local/state/choz/styles`, beside
/// everything else choz remembers. Read once at start — see
/// [`style::load_dir`].
pub fn styles_dir() -> std::path::PathBuf {
    crate::cache::state_dir().join("styles")
}

/// Read the user's styles, if they wrote any. Returns how many came in.
pub fn load_styles() -> usize {
    style::load_dir(&styles_dir())
}

/// Which of a SoundFont's presets a role should be played on: the first of
/// `wanted` that the bank actually has.
///
/// `presets` is `(bank, program)` in the order the tab lists them. Nothing
/// matches — a bank with no horns in it, a single-instrument SoundFont — and
/// the answer is `None`: the caller leaves the sound alone and the part still
/// plays, because a missing preset must never be the reason a band does not.
pub fn pick_program(presets: &[(u8, u8)], wanted: &[u8]) -> Option<usize> {
    for program in wanted {
        // The melodic banks first: bank 128 is the drum kit and a tenor sax
        // that landed there would be a tenor sax nobody can play.
        if let Some(i) = presets
            .iter()
            .position(|(bank, p)| *bank != 128 && p == program)
        {
            return Some(i);
        }
    }
    None
}

/// A transport that moved more than this between two ticks did not play the
/// gap — it was dragged. Four beats is a bar at the signature everything
/// defaults to.
const JUMP_BEATS: f64 = 4.0;

impl Arranger {
    pub fn new(settings: ArrangerSettings) -> Self {
        let mut arranger = Self {
            settings,
            ..Self::default()
        };
        arranger.rebake();
        arranger
    }

    pub fn is_on(&self) -> bool {
        self.settings.on
    }

    pub fn is_playing(&self) -> bool {
        self.settings.on && self.playing
    }

    /// Why the last text did not read, if it did not.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn style(&self) -> &style::Style {
        &self.style
    }

    /// Bars in the progression, and beats in the whole of it — what a panel
    /// puts in its header.
    pub fn bars(&self) -> usize {
        self.bars
    }

    pub fn beats(&self) -> f64 {
        self.total
    }

    /// The part, for the tests and for a panel that draws it.
    pub fn part(&self) -> &[Note] {
        &self.baked
    }

    pub fn view(&self) -> ArrangerView<'_> {
        ArrangerView {
            settings: &self.settings,
            playing: self.playing,
            style: self.style.name,
            bars: self.bars,
            bar: match self.playing {
                true => self.bar_index().map(|b| b + 1).unwrap_or(0),
                false => 0,
            },
            section: self.section_at(self.bar_index()),
            error: self.error(),
            notes: self.baked.len(),
            cursor: 0,
            focused: false,
        }
    }

    /// Which bar the playhead is in, from 0 — `None` when the style has no
    /// bar length to divide by.
    fn bar_index(&self) -> Option<usize> {
        (self.style.beats_per_bar > 0.0)
            .then(|| (self.at / self.style.beats_per_bar).floor() as usize)
    }

    /// The part a bar belongs to: the last one that started at or before it.
    fn section_at(&self, bar: Option<usize>) -> Option<&str> {
        let bar = bar?;
        self.sections
            .iter()
            .rev()
            .find(|(_, from)| *from <= bar)
            .map(|(name, _)| name.as_str())
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        self.settings.text = text.into();
        self.rebake();
    }

    pub fn set_role(&mut self, role: Role) {
        if self.settings.role != role {
            self.settings.role = role;
            self.rebake();
        }
    }

    pub fn set_seed(&mut self, seed: u32) {
        if self.settings.seed != seed {
            self.settings.seed = seed;
            self.rebake();
        }
    }

    /// Read the text and generate this role's part. Everything expensive the
    /// arranger does happens here, off the audio thread and off the tick.
    pub fn rebake(&mut self) {
        match chord::parse_progression(&self.settings.text) {
            Ok(prog) => {
                self.style = style::by_name(&prog.style);
                self.bars = prog.bars.len();
                self.sections = prog.sections.clone();
                self.total = prog.beats(self.style.beats_per_bar);
                self.baked = generate::bake(
                    &prog,
                    &self.style,
                    self.settings.role,
                    self.settings.seed,
                );
                self.error = None;
            }
            // The old part stays: a half-typed chord would otherwise stop the
            // band mid-bar, and the text is edited while it plays.
            Err(e) => self.error = Some(e.to_string()),
        }
        // The tick must not allocate, and a chord is at most a handful of notes
        // at a time.
        self.sounding.reserve(16);
        self.rewind();
    }

    /// Back to the top of the progression, nothing sounding.
    pub fn rewind(&mut self) {
        self.at = 0.0;
        self.cursor = 0;
        self.last_pos = None;
        self.free = 0.0;
        self.last_instant = None;
    }

    pub fn play(&mut self) {
        self.playing = true;
        self.rewind();
    }

    pub fn stop(&mut self, out: &mut Vec<ArpEvent>) {
        self.playing = false;
        self.silence(out);
        self.rewind();
    }

    /// Let go of everything this arranger has sounding. `PANIC`, and switching
    /// it off.
    pub fn silence(&mut self, out: &mut Vec<ArpEvent>) {
        for (note, _) in self.sounding.drain(..) {
            out.push(ArpEvent::Off { note, at: 0 });
        }
    }

    /// Play whatever falls between the last tick and this one.
    ///
    /// Takes the instant rather than reading the clock, for the reason the
    /// sequencer does: the whole thing is testable without sleeping.
    pub fn tick(&mut self, now: Instant, out: &mut Vec<ArpEvent>) {
        if !self.settings.on || !self.playing || self.baked.is_empty() || self.total <= 0.0 {
            self.silence(out);
            return;
        }
        let Some(delta) = self.advance(now) else {
            return;
        };
        self.release(delta, out);

        // Beats are counted forward from where the playhead is, wrapping at the
        // end of the progression: the form comes round, which is the whole idea
        // of a twelve-bar.
        let mut left = delta;
        while left > 1e-9 {
            let to = (self.at + left).min(self.total);
            while let Some(note) = self.baked.get(self.cursor) {
                if note.start >= to {
                    break;
                }
                if note.start >= self.at {
                    out.push(ArpEvent::On {
                        note: note.note,
                        vel: note.vel,
                        at: 0,
                    });
                    self.sounding.push((note.note, note.len));
                }
                self.cursor += 1;
            }
            left -= to - self.at;
            self.at = to;
            if self.at >= self.total - 1e-9 {
                self.at = 0.0;
                self.cursor = 0;
            }
        }
    }

    /// How far the music moved since the last tick, in beats.
    ///
    /// The transport while it rolls — so nothing drifts and a stall skips
    /// rather than catching up in a burst — and its own count of the clock
    /// otherwise, which is what a box with no transport under it has always
    /// done here.
    // ponytail: its own clock counts beats off `now` and nothing else — no
    // lookahead, so a note lands within one pass of the UI loop. The
    // arpeggiator's `next_grid_step` is the upgrade path if that is ever
    // audible, and it would be the same change in three artifacts.
    fn advance(&mut self, now: Instant) -> Option<f64> {
        let transport = choz_ports::transport();
        let pos = match transport.playing() {
            true => transport.ppq(),
            false => {
                let seconds = match self.last_instant {
                    Some(last) => now.saturating_duration_since(last).as_secs_f64(),
                    None => 0.0,
                };
                self.free + seconds * transport.bpm().max(1.0) as f64 / 60.0
            }
        };
        let delta = match self.last_pos {
            Some(last) => pos - last,
            None => 0.0,
        };
        self.last_pos = Some(pos);
        self.free = pos;
        self.last_instant = Some(now);
        // Rewound, or dragged: play from there rather than playing everything
        // in between at once.
        if !(0.0..=JUMP_BEATS).contains(&delta) {
            return None;
        }
        Some(delta)
    }

    /// Let go of what has run out of length.
    fn release(&mut self, delta: f64, out: &mut Vec<ArpEvent>) {
        let mut i = 0;
        while i < self.sounding.len() {
            self.sounding[i].1 -= delta;
            match self.sounding[i].1 <= 0.0 {
                true => {
                    let (note, _) = self.sounding.swap_remove(i);
                    out.push(ArpEvent::Off { note, at: 0 });
                }
                false => i += 1,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use generate::gm;
    use std::time::Duration;

    fn arranger(role: Role, seed: u32) -> Arranger {
        Arranger::new(ArrangerSettings {
            on: true,
            role,
            seed,
            ..Default::default()
        })
    }

    /// Same text, same style, same seed: the same notes, one at a time. An
    /// interpretation you liked is one you can get back.
    #[test]
    fn a_seed_names_an_interpretation() {
        for role in Role::ALL {
            let a = arranger(role, 42);
            let b = arranger(role, 42);
            assert_eq!(a.part(), b.part(), "{} is not deterministic", role.name());
            let other = arranger(role, 43);
            assert_ne!(
                a.part(),
                other.part(),
                "{} plays the same thing whatever the seed",
                role.name()
            );
        }
    }

    /// What every role owes whatever it plays.
    #[test]
    fn the_parts_are_playable() {
        for role in Role::ALL {
            let a = arranger(role, 42);
            let total = a.beats();
            assert!(!a.part().is_empty(), "{} plays nothing", role.name());
            for note in a.part() {
                let what = role.name();
                assert!(note.vel >= 1, "{what}: a note-on of zero is a note-off");
                assert!(note.len > 0.0, "{what}: a note of no length");
                assert!(note.start >= 0.0, "{what}: a note before the downbeat");
                assert!(
                    note.start + note.len <= total + 1e-9,
                    "{what}: a note hanging past the end of the form"
                );
            }
            // Sorted: the tick walks the list with one cursor and never looks
            // back.
            assert!(
                a.part().windows(2).all(|w| w[0].start <= w[1].start),
                "{} is not in time order",
                role.name()
            );
        }
    }

    /// A bass line stays in the bass, a drum part in the kit, and both keep out
    /// of what the other is for.
    #[test]
    fn each_role_stays_in_its_register() {
        let style = style::MAJOR_BLUES;
        let bass = arranger(Role::Bass, 42);
        for note in bass.part() {
            assert!(
                (style.bass.low..=style.bass.high).contains(&note.note),
                "{} is not a bass note",
                note.note
            );
        }
        let drums = arranger(Role::Drums, 42);
        for note in drums.part() {
            assert!(
                (gm::KICK..=gm::RIDE).contains(&note.note),
                "{} is not in the GM kit",
                note.note
            );
        }
        assert!(
            drums.part().iter().any(|n| n.note == gm::KICK)
                && drums.part().iter().any(|n| n.note == gm::SNARE),
            "a drum part with no kick or no snare"
        );
        let piano = arranger(Role::Piano, 42);
        for note in piano.part() {
            assert!(
                (style.comp.low..=style.comp.high).contains(&note.note),
                "{} is outside the comping register",
                note.note
            );
        }
        // The guitar plays in open position, below the piano's right hand and
        // above the bass.
        let guitar = arranger(Role::Guitar, 42);
        for note in guitar.part() {
            assert!(
                (40..=72).contains(&note.note),
                "{} is not a note on a guitar",
                note.note
            );
        }
    }

    /// The band plays the same form: twelve bars is twelve bars for everybody,
    /// and they all come round together.
    #[test]
    fn the_parts_are_the_same_length() {
        let lengths: Vec<f64> = Role::ALL.map(|r| arranger(r, 42).beats()).to_vec();
        assert!(lengths.iter().all(|l| (*l - 48.0).abs() < 1e-9), "{lengths:?}");
    }

    /// A walking bass walks: the failure this is here to catch is `C C C C`.
    #[test]
    fn the_bass_does_not_stand_still() {
        let bass = arranger(Role::Bass, 42);
        let notes: Vec<u8> = bass.part().iter().map(|n| n.note).collect();
        assert!(notes.len() >= 40, "a beat a bar is not a bass line: {}", notes.len());
        let distinct = notes.iter().collect::<std::collections::HashSet<_>>().len();
        assert!(distinct >= 8, "only {distinct} different notes in twelve bars");
        assert!(
            notes.windows(2).filter(|w| w[0] == w[1]).count() * 3 < notes.len(),
            "the same note over and over: {notes:?}"
        );
    }

    /// A comp leads its voices: `C7` to `F7` moves by a step or two, not by an
    /// octave, and that is the difference between hands and a lookup table.
    #[test]
    fn the_voicings_lead() {
        let piano = arranger(Role::Piano, 42);
        let mut tops: Vec<u8> = Vec::new();
        let mut at = f64::MIN;
        for note in piano.part() {
            match (note.start - at).abs() < 1e-6 {
                true => *tops.last_mut().unwrap() = (*tops.last().unwrap()).max(note.note),
                false => {
                    at = note.start;
                    tops.push(note.note);
                }
            }
        }
        let jumps = tops.windows(2).filter(|w| w[0].abs_diff(w[1]) > 5).count();
        assert_eq!(jumps, 0, "{jumps} voicings jumped more than a fourth: {tops:?}");
    }

    /// A guitar strums: the pick crosses the strings one at a time, and the
    /// hands on a piano do not. Same chords, same rhythm, different instrument
    /// — which is the whole of why the role exists.
    #[test]
    fn the_guitar_strums_and_the_piano_does_not() {
        let together = |role: Role| -> usize {
            let a = arranger(role, 42);
            let starts: Vec<f64> = a.part().iter().map(|n| n.start).collect();
            starts
                .windows(2)
                .filter(|w| (w[1] - w[0]).abs() < 1e-9)
                .count()
        };
        assert!(together(Role::Piano) > 0, "the piano is not playing chords");
        assert_eq!(
            together(Role::Guitar),
            0,
            "the guitar landed a whole chord on one instant"
        );
        // And it keeps its root: a guitar shape has one, a piano's shell
        // voicing leaves it to the bass.
        let guitar = arranger(Role::Guitar, 42);
        let low = guitar.part().iter().map(|n| n.note).min().unwrap_or(0);
        assert!(low < 52, "nothing below the piano's register: {low}");
    }

    /// The melody plays a form, not twelve different bars: `A A' B A''` means
    /// the first bar's rhythm comes back on the fourth, changed in pitch and
    /// not in shape. Roll the dice every bar and this is what stops matching.
    #[test]
    fn the_melody_repeats_its_motif() {
        for role in [Role::Melody, Role::Solo] {
            let a = arranger(role, 42);
            let bar = |n: usize| -> Vec<f64> {
                a.part()
                    .iter()
                    // A hair before the barline: the humanisation moves a
                    // downbeat either way, and a note a hundredth early is
                    // still that bar's first note.
                    .filter(|x| {
                        x.start >= n as f64 * 4.0 - 0.1 && x.start < (n as f64 + 1.0) * 4.0 - 0.1
                    })
                    .map(|x| x.start - n as f64 * 4.0)
                    .collect()
            };
            let (first, fourth) = (bar(0), bar(3));
            assert_eq!(
                first.len(),
                fourth.len(),
                "{}: the phrase came back with a different number of notes",
                role.name()
            );
            for (a, b) in first.iter().zip(&fourth) {
                // Only the humanisation is allowed to move it, and that is a
                // hundredth of a beat either way.
                assert!(
                    (a - b).abs() < 0.05,
                    "{}: the phrase came back on a different rhythm: {first:?} vs {fourth:?}",
                    role.name()
                );
            }
            let pitches: Vec<u8> = a.part().iter().map(|n| n.note).collect();
            let distinct = pitches.iter().collect::<std::collections::HashSet<_>>().len();
            assert!(distinct >= 5, "{}: {distinct} pitches in twelve bars", role.name());
        }
    }

    /// The solo is the melody played busier — if it is not, there is no reason
    /// for the two roles to both exist.
    #[test]
    fn the_solo_plays_more_than_the_melody() {
        let melody = arranger(Role::Melody, 42).part().len();
        let solo = arranger(Role::Solo, 42).part().len();
        assert!(solo > melody, "melody {melody} notes, solo {solo}");
    }

    /// A text that does not parse keeps the last part that did: a half-typed
    /// chord must not silence the band mid-bar.
    #[test]
    fn a_typo_does_not_stop_the_music() {
        let mut a = arranger(Role::Bass, 42);
        let before = a.part().to_vec();
        a.set_text("key = C\n|| I7 | Zq9 ||");
        assert!(a.error().is_some());
        assert_eq!(a.part(), before, "the last part that read has to keep playing");
        a.set_text("key = C\n|| I7 | IV7 ||");
        assert!(a.error().is_none());
        assert_eq!(a.bars(), 2);
    }

    /// The style comes out of the text, and an unknown one plays a feel you can
    /// hear is wrong rather than nothing at all.
    #[test]
    fn the_text_chooses_the_style() {
        let mut a = arranger(Role::Drums, 42);
        assert_eq!(a.style().name, "major_blues");
        a.set_text("key = C\nstyle = minor_blues\n|| Im7 | IVm7 ||");
        assert_eq!(a.style().name, "minor_blues");
        a.set_text("key = C\nstyle = polka\n|| Im7 ||");
        assert_eq!(a.style().name, "major_blues", "unknown styles fall back");
    }

    /// The tick plays the baked part against the transport and nothing else:
    /// what comes out of a chorus is exactly what was baked, and the form comes
    /// round to the top.
    #[test]
    fn a_chorus_plays_what_was_baked() {
        let _g = crate::test_locks::transport();
        let t = choz_ports::transport();
        t.set_playing(false);
        t.set_bpm(120.0);
        t.set_position_beats(0.0);

        let mut a = arranger(Role::Bass, 42);
        let baked = a.part().to_vec();
        a.play();
        let now = Instant::now();
        let mut out = Vec::new();

        // Not rolling and not started by its own PLAY yet: silence.
        a.tick(now, &mut out);
        assert!(out.is_empty());

        // A chorus of the transport, a beat at a time.
        t.set_playing(true);
        for beat in 0..=48 {
            t.set_position_beats(beat as f64);
            a.tick(now + Duration::from_millis(beat * 10), &mut out);
        }
        let ons: Vec<(u8, u8)> = out
            .iter()
            .filter_map(|e| match e {
                ArpEvent::On { note, vel, .. } => Some((*note, *vel)),
                _ => None,
            })
            .collect();
        let want: Vec<(u8, u8)> = baked.iter().map(|n| (n.note, n.vel)).collect();
        assert_eq!(ons, want, "a chorus is the part, in order");

        // Everything that was started gets let go of.
        a.stop(&mut out);
        let offs = out
            .iter()
            .filter(|e| matches!(e, ArpEvent::Off { .. }))
            .count();
        assert_eq!(offs, ons.len(), "a note-on with no note-off is a stuck note");

        // And the form comes round rather than stopping at the twelfth bar.
        a.play();
        out.clear();
        for beat in 48..=60 {
            t.set_position_beats(beat as f64);
            a.tick(now + Duration::from_millis(beat * 10), &mut out);
        }
        assert!(
            out.iter().any(|e| matches!(e, ArpEvent::On { .. })),
            "the second chorus never started"
        );
        t.set_playing(false);
    }

    /// A transport that is dragged does not play the gap it was dragged over.
    #[test]
    fn a_jump_is_not_played() {
        let _g = crate::test_locks::transport();
        let t = choz_ports::transport();
        t.set_playing(true);
        t.set_bpm(120.0);
        t.set_position_beats(0.0);

        let mut a = arranger(Role::Drums, 42);
        a.play();
        let now = Instant::now();
        let mut out = Vec::new();
        a.tick(now, &mut out);
        out.clear();
        t.set_position_beats(32.0);
        a.tick(now + Duration::from_millis(10), &mut out);
        assert!(out.is_empty(), "eight bars played at once: {}", out.len());
        // And it picks up from where it was dragged to.
        t.set_position_beats(33.0);
        a.tick(now + Duration::from_millis(20), &mut out);
        assert!(out.iter().any(|e| matches!(e, ArpEvent::On { .. })));
        t.set_playing(false);
    }

    /// Every style plays, and plays inside what it says about itself. A style
    /// is data, so the only thing that can be wrong with a new one is a number,
    /// and this is the check that reads them all.
    #[test]
    fn every_style_plays() {
        for style in style::ALL {
            let text = format!(
                "key = C\nstyle = {}\n|| I7 | IV7 | I7 | V7 | IV7 | I7 | V7 | I7 ||",
                style.name
            );
            for role in Role::ALL {
                let mut a = arranger(role, 7);
                a.set_text(text.clone());
                let what = format!("{} / {}", style.name, role.name());
                assert!(a.error().is_none(), "{what}: {:?}", a.error());
                assert_eq!(a.style().name, style.name, "{what}: wrong style");
                assert_eq!(
                    a.beats(),
                    8.0 * style.beats_per_bar,
                    "{what}: the form is not eight bars of this style"
                );
                assert!(!a.part().is_empty(), "{what}: plays nothing");
                for n in a.part() {
                    assert!(n.vel >= 1 && n.len > 0.0 && n.start >= 0.0, "{what}: {n:?}");
                    assert!(n.start + n.len <= a.beats() + 1e-9, "{what}: hangs past the end");
                }
                assert!(
                    a.part().windows(2).all(|w| w[0].start <= w[1].start),
                    "{what}: not in time order"
                );
                let range = match role {
                    Role::Bass => (style.bass.low, style.bass.high),
                    Role::Drums => (gm::KICK, gm::RIDE),
                    Role::Piano => (style.comp.low, style.comp.high),
                    Role::Guitar => (40, 72),
                    Role::Melody => (style.lead.low, style.lead.high),
                    // The soloist sits a fourth above the tune (`solo_of`),
                    // unless the style wrote the solo its own register.
                    Role::Solo => match style.solo {
                        Some(l) => (l.low, l.high),
                        None => (style.lead.low + 5, style.lead.high + 5),
                    },
                };
                for n in a.part() {
                    assert!(
                        (range.0..=range.1).contains(&n.note),
                        "{what}: {} is outside {range:?}",
                        n.note
                    );
                }
            }
        }
    }

    /// The band plays loose, and how loose is the style's number times the
    /// role's. With the style's set to zero every note lands where the table
    /// says, which is the proof that all of the jitter goes through one knob.
    #[test]
    fn the_feel_is_one_knob() {
        let prog = chord::parse_progression(DEFAULT_TEXT).unwrap();
        let mut dead = style::ROCK;
        dead.human = 0.0;
        for role in Role::ALL {
            // The guitar is off the grid on purpose: the strum is the hand
            // crossing the strings, not the drummer being human.
            if role == Role::Guitar {
                continue;
            }
            for n in generate::bake(&prog, &dead, role, 3) {
                let sixteenth = n.start * 4.0;
                assert!(
                    (sixteenth - sixteenth.round()).abs() < 1e-9,
                    "{}: {} is off the grid with the feel at zero",
                    role.name(),
                    n.start
                );
            }
            // And with the feel on, it is not: the drummer is tighter than the
            // soloist, and neither is a sequencer.
            let loose = generate::bake(&prog, &style::ROCK, role, 3);
            let off = loose
                .iter()
                .filter(|n| ((n.start * 4.0) - (n.start * 4.0).round()).abs() > 1e-9)
                .count();
            assert!(off > 0, "{}: nothing moved", role.name());
        }
    }

    /// A drummer does not play the same fill every four bars, and a guitar
    /// hand comes back up: the two things that give a generated band away
    /// inside one chorus.
    #[test]
    fn the_fills_vary_and_the_hand_comes_back_up() {
        let drums = arranger(Role::Drums, 42);
        // The fills live on the last beat of every fourth bar; what is played
        // there has to differ between two of them somewhere in twelve bars.
        let fill_of = |bar: usize| -> Vec<u8> {
            let from = bar as f64 * 4.0 + 3.0;
            let mut notes: Vec<(f64, u8)> = drums
                .part()
                .iter()
                .filter(|n| n.start >= from - 0.05 && n.start < from + 1.0)
                .map(|n| (n.start, n.note))
                .collect();
            notes.sort_by(|a, b| a.0.total_cmp(&b.0));
            notes.into_iter().map(|(_, n)| n).collect()
        };
        let fills: Vec<Vec<u8>> = [3, 7, 11].into_iter().map(fill_of).collect();
        assert!(
            fills.iter().any(|f| *f != fills[0]),
            "the same fill three times: {fills:?}"
        );

        // The guitar: a downstroke walks up the strings in time, an upstroke
        // walks down them. Both have to be in there.
        let guitar = arranger(Role::Guitar, 42);
        let part = guitar.part();
        let mut up = 0;
        let mut down = 0;
        for w in part.windows(2) {
            if w[1].start - w[0].start > 1e-9 && w[1].start - w[0].start < 0.05 {
                match w[1].note > w[0].note {
                    true => up += 1,
                    false => down += 1,
                }
            }
        }
        assert!(up > 0 && down > 0, "the hand only goes one way: up {up}, down {down}");
    }

    /// The feel lives in whatever division the style counts in: a blues swings
    /// its eighths, a funk its sixteenths. One number, two feels — the failure
    /// this catches is a funk played dead straight because `swung()` only knew
    /// about eighths.
    #[test]
    fn the_swing_is_in_the_styles_own_division() {
        let prog = chord::parse_progression(DEFAULT_TEXT).unwrap();
        // With the humanisation off, what is left off the grid is the swing.
        let off_by = |st: &style::Style| -> Vec<f64> {
            let mut st = *st;
            st.human = 0.0;
            let mut out = Vec::new();
            for role in [Role::Drums, Role::Bass] {
                for n in generate::bake(&prog, &st, role, 5) {
                    let sixteenth = (n.start / 0.25).round() * 0.25;
                    let off = (n.start - sixteenth).abs();
                    if off > 1e-9 {
                        out.push(off);
                    }
                }
            }
            out
        };
        // The funk swings the sixteenths: a quarter of one is 0.0625 of a beat,
        // and nothing else in it is off the grid.
        let funk = off_by(&style::FUNK);
        assert!(!funk.is_empty(), "the funk came out dead straight");
        assert!(
            funk.iter().all(|o| (o - 0.0625).abs() < 1e-6),
            "not a sixteenth swing: {funk:?}"
        );
        // The blues swings the eighths, which lands *on* the sixteenth grid
        // (0.3 of a beat is not one of them, so it shows up here) — and never
        // a sixteenth of a beat away.
        let blues = off_by(&style::MAJOR_BLUES);
        assert!(!blues.is_empty(), "the blues came out straight");
        assert!(
            blues.iter().all(|o| (o - 0.05).abs() < 1e-6),
            "not an eighth swing: {blues:?}"
        );
    }

    /// The bar that hands the form back to the top is not the bar that hands
    /// over bar four: the turnaround gets two beats of fill where the others
    /// get one.
    #[test]
    fn the_turnaround_is_a_longer_fill() {
        let drums = arranger(Role::Drums, 11);
        let kit = [gm::SNARE, gm::TOM_HI, gm::TOM_MID, gm::TOM_LO];
        let fill_hits = |bar: usize| -> usize {
            let from = bar as f64 * 4.0 + 2.0;
            drums
                .part()
                .iter()
                .filter(|n| n.start >= from - 0.05 && n.start < from + 2.0)
                .filter(|n| kit.contains(&n.note))
                .count()
        };
        assert!(
            fill_hits(11) > fill_hits(3),
            "the turnaround plays no more than any other fill: {} vs {}",
            fill_hits(11),
            fill_hits(3)
        );
    }

    /// The bass is a line and not an arpeggio: it steps through the scale
    /// between chord tones, and it arrives early where a player would.
    #[test]
    fn the_bass_steps_and_anticipates() {
        // A straight style, so an anticipation is half a beat early and not
        // half a beat plus the shuffle.
        let mut a = arranger(Role::Bass, 19);
        a.set_text("key = C\nstyle = straight_blues\n|| I7 | IV7 | I7 | V7 | IV7 | I7 | V7 | I7 ||");
        let part = a.part();
        // A step: two notes a tone or a semitone apart, one after the other.
        let steps = part
            .windows(2)
            .filter(|w| {
                let d = (w[1].note as i32 - w[0].note as i32).abs();
                (1..=2).contains(&d)
            })
            .count();
        assert!(steps > 4, "a line that only jumps: {steps} steps");
        // An anticipation: a note that starts off the beat and is held over
        // one. The humanisation is small enough not to make one of these.
        let early = part
            .iter()
            .filter(|n| {
                let into = n.start.rem_euclid(1.0);
                (0.35..0.65).contains(&into) && n.len > 1.0
            })
            .count();
        assert!(early > 0, "nothing arrived early");
    }

    /// The soloist hears the tune: the two lines do not land on the same note
    /// at the same moment, which is the one clash a listener hears as a
    /// mistake rather than as harmony.
    #[test]
    fn the_solo_keeps_out_of_the_melodys_way() {
        let melody = arranger(Role::Melody, 42);
        let solo = arranger(Role::Solo, 42);
        let clashes = solo
            .part()
            .iter()
            .filter(|s| {
                melody
                    .part()
                    .iter()
                    .any(|m| m.note == s.note && (m.start - s.start).abs() < 0.25)
            })
            .count();
        assert_eq!(clashes, 0, "{clashes} unisons with the tune");
    }

    /// A figure that comes back exactly as it left is an exercise: the phrase
    /// the melody opens with is worked as the form goes round.
    #[test]
    fn the_motif_grows() {
        let a = arranger(Role::Melody, 42);
        let bar = |n: usize| -> Vec<(f64, u8)> {
            a.part()
                .iter()
                .filter(|x| x.start >= n as f64 * 4.0 && x.start < (n + 1) as f64 * 4.0)
                .map(|x| ((x.start * 4.0).round() / 4.0 - n as f64 * 4.0, x.note))
                .collect()
        };
        // Bars 0, 4 and 8 are the same place in the four-bar phrase.
        assert_ne!(bar(0), bar(8), "the figure came back untouched");
    }

    /// A chord written held across two slots of a bar is one chord: the bass
    /// must not hear it arrive twice.
    #[test]
    fn a_chord_held_across_a_bar_lands_once() {
        let prog = chord::parse_progression("key = C\n|| I7 - - IV7 ||").unwrap();
        // Beat 0 is the chord landing with three beats of it left; beat 1 is
        // the same chord with two.
        let (first, left) = prog.at(0.0, 4.0).unwrap();
        assert_eq!(first.symbol, "I7");
        assert!((left - 3.0).abs() < 1e-9, "{left}");
        let (mid, left) = prog.at(1.0, 4.0).unwrap();
        assert_eq!(mid.symbol, "I7");
        assert!((left - 2.0).abs() < 1e-9, "{left}");
        let (last, left) = prog.at(3.0, 4.0).unwrap();
        assert_eq!(last.symbol, "IV7");
        assert!((left - 1.0).abs() < 1e-9, "{left}");
    }

    /// A role is played on the instrument the style says, and a SoundFont that
    /// does not have it is not a reason for the band to stop: the next one
    /// down the list plays it, and a bank with none of them leaves the sound
    /// alone.
    #[test]
    fn the_style_says_what_plays_the_part() {
        use generate::Role;
        let jazz = style::by_name("jazz_swing");
        assert_eq!(jazz.programs(Role::Melody), &[66, 65, 56, 71, 0]);
        assert!(jazz.programs(Role::Drums).is_empty(), "drums are a bank");

        // A SoundFont with no tenor: the alto plays it.
        let bank: Vec<(u8, u8)> = vec![(0, 0), (0, 65), (0, 56), (128, 0)];
        let at = pick_program(&bank, jazz.programs(Role::Melody)).unwrap();
        assert_eq!(bank[at], (0, 65));
        // …and with neither, the trumpet.
        let bank: Vec<(u8, u8)> = vec![(0, 0), (0, 56)];
        let at = pick_program(&bank, jazz.programs(Role::Melody)).unwrap();
        assert_eq!(bank[at], (0, 56));
        // A drum-only bank answers nothing rather than a kit.
        assert_eq!(pick_program(&[(128, 0)], jazz.programs(Role::Melody)), None);
        // Every style, every role: what is asked for ends in a piano, which is
        // the one program a SoundFont always has.
        for st in style::ALL {
            for role in Role::ALL {
                let want = st.programs(role);
                if role == Role::Drums {
                    assert!(want.is_empty(), "{} drums", st.name);
                    continue;
                }
                assert_eq!(
                    want.last(),
                    Some(&0),
                    "{} / {} does not fall back to a piano",
                    st.name,
                    role.name()
                );
            }
        }
    }

    /// A style is data, and data can come from a file. A file says what is
    /// *different* about it; everything else comes from the style it is based
    /// on, because a format that makes you write forty numbers to change one
    /// is a format nobody writes twice.
    #[test]
    fn a_style_can_be_written_in_a_file() {
        let style = style::parse(
            "# a slower shuffle\nname = slow_blues\nfrom = minor_blues\nswing = 0.66\nbass.density = 0.75\ndrums.kick = 0 2.5\ncomp.hits = 0, 1.5\n",
        )
        .unwrap();
        assert_eq!(style.name, "slow_blues");
        assert!((style.swing - 0.66).abs() < 1e-6);
        assert!((style.bass.density - 0.75).abs() < 1e-6);
        assert_eq!(style.drums.kick, &[0.0, 2.5]);
        assert_eq!(style.comp.hits, &[0.0, 1.5]);
        // Everything it did not say is what it was based on.
        assert_eq!(style.lead.notes, style::MINOR_BLUES.lead.notes);
        assert_eq!(style.vel, style::MINOR_BLUES.vel);

        // And it plays: a style read off disk is the same kind of thing as one
        // written in Rust.
        let prog = chord::parse_progression(DEFAULT_TEXT).unwrap();
        for role in Role::ALL {
            assert!(
                !generate::bake(&prog, &style, role, 3).is_empty(),
                "{} plays nothing",
                role.name()
            );
        }

        // What is not a setting is an error, not a number quietly ignored.
        assert!(style::parse("name = x\nswinng = 0.5").is_err());
        assert!(style::parse("from = rock").is_err(), "a style with no name");
        assert!(style::parse("name = x\nbeats_per_bar = 0").is_err());
    }
}
