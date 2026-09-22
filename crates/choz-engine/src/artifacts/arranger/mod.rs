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
pub mod smf;
pub mod style;
pub mod styles;

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
    /// Which musician this tab is — the one its *sound* is fitted to, and the
    /// one it falls back to when no part has been switched on. See [`Self::band`].
    pub role: Role,
    /// The interpretation. Same text, same style, same seed — same notes.
    pub seed: u32,
    /// How loud each role plays, indexed by `Role as usize`. Zero is a musician
    /// who is not in the band, and all zero is the shape a project written
    /// before the faders has: the tab plays [`Self::role`] alone.
    ///
    /// A tab has one instrument, so a band in one tab is one timbre — the band
    /// a player builds out of a tab per musician is still the way to give the
    /// bass a bass and the drums a kit. What this is for is hearing the whole
    /// arrangement from one tab, and balancing it.
    ///
    /// A `Vec` and not an array the length of `Role::ALL`: a project written
    /// when there were six roles has six numbers in it, and a rack that will
    /// not open because a role went is worse than a fader nobody reads.
    #[serde(default)]
    pub parts: Vec<f32>,
    /// How far the off-divisions are pushed late, **on top of the style's own
    /// swing** — the sequencer's knob, on the same scale, because there is one
    /// swing in this program.
    #[serde(default)]
    pub swing: f32,
    /// How far a note may stray from what was generated, 0..1.
    #[serde(default)]
    pub random: f32,
    /// How often it does, 0..1. The sequencer's pair, with the same meanings.
    #[serde(default)]
    pub prob: f32,
    /// How **this band** groups the bar — `[3, 2, 2]` for a 7/8 counted that
    /// way, the metronome's own notation.
    ///
    /// The arranger's own, not the click's: a band can play 3+2+2 against a
    /// metronome counting 2+2+3, which is a real arrangement and was not
    /// sayable while this was read off [`session_groups`]. Empty falls back to
    /// the click's grouping, which is what every project written before this
    /// means, and a grouping that does not add up to the bar is ignored the
    /// same way the metronome ignores one.
    #[serde(default)]
    pub groups: Vec<u8>,
    /// Whether the chart is written in roman degrees rather than american
    /// symbols — `I7` or `C7`, the same chart either way.
    ///
    /// Not a view: the switch **rewrites the text**, because the text is the one
    /// source and a chart half in each notation is the thing nobody can read.
    /// See [`chord::respell_text`].
    #[serde(default)]
    pub roman: bool,
    /// **Split outputs**: every musician of the band on a tab of their own —
    /// and so on a strip of their own in the mixer, with its fader, its FX,
    /// its group and its direct out. The rack's tabs are its channels, so this
    /// is the arranger asking for more of them; see choz-ui's
    /// `sync_arranger_split`.
    #[serde(default)]
    pub split: bool,
    /// Each musician's strip when the band is split out, by `Role::ALL`
    /// place. Missing is a strip at unity, centred, unmuted.
    #[serde(default)]
    pub split_mix: Vec<BandStrip>,
}

/// One musician's strip in the mixer, when the band is split out: how loud
/// their channel of the tab's instrument goes into the tab, where it sits, and
/// whether it is heard at all.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BandStrip {
    pub gain: f32,
    pub pan: f32,
    pub mute: bool,
}

impl Default for BandStrip {
    fn default() -> Self {
        Self {
            gain: 1.0,
            pan: 0.0,
            mute: false,
        }
    }
}

impl BandStrip {
    /// Left and right gain: the balance law, unity in the middle, the far
    /// side down to nothing at the edge — the tab's own pan still places the
    /// whole of it.
    pub fn sides(&self) -> (f32, f32) {
        if self.mute {
            return (0.0, 0.0);
        }
        let p = self.pan.clamp(-1.0, 1.0);
        (
            self.gain * (1.0 - p).min(1.0),
            self.gain * (1.0 + p).min(1.0),
        )
    }
}

impl ArrangerSettings {
    /// Who is playing, and how loud. Never empty: a band with nobody in it is
    /// what the box's own switch is for, not something the settings can say.
    pub fn band(&self) -> Vec<(Role, f32)> {
        let on: Vec<(Role, f32)> = Role::ALL
            .iter()
            .map(|r| (*r, self.gain(*r)))
            .filter(|(_, gain)| *gain > 0.0)
            .collect();
        match on.is_empty() {
            true => vec![(self.role, 1.0)],
            false => on,
        }
    }

    /// A musician's strip, when the band is split out.
    pub fn strip(&self, role: Role) -> BandStrip {
        self.split_mix
            .get(role as usize)
            .copied()
            .unwrap_or_default()
    }

    pub fn strip_mut(&mut self, role: Role) -> &mut BandStrip {
        if self.split_mix.len() < Role::ALL.len() {
            self.split_mix.resize(Role::ALL.len(), BandStrip::default());
        }
        &mut self.split_mix[role as usize]
    }

    pub fn gain(&self, role: Role) -> f32 {
        self.parts.get(role as usize).copied().unwrap_or(0.0)
    }

    /// Set one player's fader. `0.0` takes them out of the band.
    ///
    /// The first touch writes out what the tab was already playing: a project
    /// from before the faders says its band with `role` alone, and turning the
    /// piano up must not be what silences the bass.
    pub fn set_gain(&mut self, role: Role, gain: f32) {
        self.parts
            .resize(Role::ALL.len().max(self.parts.len()), 0.0);
        if self.parts.iter().all(|g| *g <= 0.0) {
            self.parts[self.role as usize] = 1.0;
        }
        self.parts[role as usize] = gain.clamp(0.0, 2.0);
        // The tab's sound follows whoever is left at the top of the band: it is
        // what `role` means, and a `role` nobody plays would fit the tab to an
        // instrument nothing is played on.
        if let Some((first, _)) = self.band().first() {
            self.role = *first;
        }
    }
}

impl ArrangerSettings {
    /// How the band groups the bar: its own grouping, or the click's when it
    /// has none of its own. What a bar that was written without weights of its
    /// own is counted in.
    pub fn grouping(&self) -> Vec<u8> {
        match self.groups.is_empty() {
            true => session_groups(),
            false => self.groups.clone(),
        }
    }
}

impl Default for ArrangerSettings {
    fn default() -> Self {
        Self {
            on: false,
            text: DEFAULT_TEXT.to_string(),
            role: Role::Bass,
            seed: 1,
            parts: Vec::new(),
            swing: 0.0,
            random: 0.0,
            prob: 0.0,
            groups: Vec::new(),
            // Letters, which is what `DEFAULT_TEXT` is written in and what a
            // player reads off a chart. Degrees are the switch — see
            // [`Arranger::set_roman`].
            roman: false,
            // Off until asked for: a band is one strip, like any tab.
            split: false,
            split_mix: Vec::new(),
        }
    }
}

/// A twelve-bar blues in C, which is the shortest thing that shows whether any
/// of this works.
///
/// **In letters**: it is what a chart handed to a player is written in, and the
/// degrees are one press away for whoever is thinking about the form instead.
///
/// **The file is the one copy**: `assets/default.chord` ships beside the
/// wallpapers, so the chart a fresh tab plays is also one LOAD can open.
pub const DEFAULT_TEXT: &str = include_str!("../../../../../assets/default.chord");

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
    /// The tonic the text named, as a pitch class — what the panel's key
    /// picker shows and writes back.
    key: u8,
    /// How the bar was grouped when the part was baked — `[3, 2, 2]` for a 7/8.
    /// Kept so the panel can draw the bar the band is actually playing, and so
    /// a grouping change is noticed.
    groups: Vec<u8>,
    /// The `meter` and `groups` lines of the text, as it last read. **What the
    /// chart says wins** over the session and the tab: a file that says 7/8 is
    /// a song in 7/8.
    written_meter: Option<(u16, u16)>,
    written_groups: Vec<u8>,
    /// Where each bar starts and what it is in, for a chart that changes
    /// meter. Empty for one that does not — a bar is then `beats_per_bar`.
    bar_starts: Vec<f64>,
    bar_meters: Vec<(u16, u16)>,
    /// The style's own bar, before the session's was put over it: what the
    /// band should be counting when the session says nothing (4/4). Kept
    /// because `style.beats_per_bar` is overwritten, and without it a band
    /// that was baked in a waltz's 3 while the click still said 3/4 had no way
    /// to see it should now be in 4.
    own_bpb: f64,
    /// The tempo the part was baked for. What a kit can play at 130 and what it
    /// can play at 230 are different parts — see [`generate::FAST_BPM`] — so
    /// the band is re-baked when the tempo crosses one of those lines, and
    /// nowhere else: a chorus that re-bakes on every bpm message is a chorus
    /// that stutters.
    bpm: f32,
    bars: usize,
    /// `(name, first bar)` of each part of the form, in playing order — what
    /// the panel says you are in. Empty for a text that named no parts.
    sections: Vec<(String, usize)>,
    playing: bool,
    /// Started by the transport, and waiting for it to roll. Its own clock is
    /// not counted while this is up — see [`Arranger::play_on_transport`].
    follow: bool,
    /// Where the playhead is inside the progression, in beats.
    at: f64,
    /// The next note of `baked` that has not been played yet.
    cursor: usize,
    /// The shared position the last tick read, to turn into a step forward.
    last_pos: Option<f64>,
    /// The downbeat PLAY is waiting for, worked out **once**, when it is
    /// pressed. Worked out again on every tick it was always the *next* one,
    /// and a tick had to land on it to the millionth of a beat or the wait
    /// moved on a bar — the band came in whenever luck let it, or not at all.
    ///
    /// With the clock it was worked out on — the transport's or the free one —
    /// because they are two different numbers: PLAY on the free clock and the
    /// rack's PLAY a moment later is a target the transport's position may be
    /// bars short of.
    enter_at: Option<(f64, bool)>,
    /// What is sounding, how many beats of it are left, and who is playing it.
    sounding: Vec<(u8, f64, Role)>,
}

impl Default for Arranger {
    fn default() -> Self {
        let mut arranger = Self {
            settings: ArrangerSettings::default(),
            baked: Vec::new(),
            total: 0.0,
            error: None,
            style: style::ALL[0],
            key: 0,
            groups: Vec::new(),
            written_meter: None,
            written_groups: Vec::new(),
            bar_starts: Vec::new(),
            bar_meters: Vec::new(),
            own_bpb: 4.0,
            bpm: 120.0,
            bars: 0,
            sections: Vec::new(),
            playing: false,
            follow: false,
            at: 0.0,
            cursor: 0,
            last_pos: None,
            enter_at: None,
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
    /// The style the text asked for, resolved — as the picker names it, which
    /// is what a player reads off the box.
    pub style: &'static str,
    /// The bar the band is counting, as a signature. The session's when
    /// somebody set one, the style's own otherwise — and **not** the
    /// transport's, which is what the box used to print and why a waltz said
    /// 4/4 while it played in three.
    pub meter: (u16, u16),
    /// The tonic the text named, as a pitch class.
    pub key: u8,
    /// How the bar is grouped — `[3, 2, 2]` for a 7/8 counted that way. Empty
    /// for a bar nobody grouped, which is most of them.
    pub groups: &'a [u8],
    /// How many subdivisions a bar of this part has: what the grouping adds up
    /// to, or one a beat when there is no grouping. What the panel's matrix
    /// draws a cell for.
    pub cells: usize,
    /// Which of those cells is sounding, while it is playing.
    pub pulse: Option<usize>,
    /// Beats in the bar the band is counting — the session's when somebody set
    /// one, the style's otherwise.
    pub beats_per_bar: f64,
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

/// The tempo everything is baked for: the transport's, which is choz's own
/// clock or whatever is driving it — MIDI clock included.
fn tempo() -> f32 {
    choz_ports::transport().bpm().max(1.0)
}

/// The bar the session is counting, in quarter-note beats, or `None` while it
/// is counting the plain 4/4 every session starts in.
///
/// **A meter somebody set wins over the style's own.** A style that counts in 3
/// goes on counting in 3 in an ordinary 4/4 session — that is the style, not an
/// accident — but a session put into 7/8 is a session where the band plays 7/8,
/// and the panel's own bar matrix is drawn from the same number.
pub fn session_bar() -> Option<f64> {
    let (num, den) = choz_ports::transport().time_signature();
    if (num, den) == (4, 4) {
        return None;
    }
    Some((num.max(1) as f64 * 4.0 / den.max(1) as f64).max(0.25))
}

/// How the bar is grouped, as the metronome clicks it: `[3, 2, 2]` for a 7/8.
pub fn session_groups() -> Vec<u8> {
    crate::artifacts::metronome::metronome().groups()
}

/// Which side of the two lines a tempo is on. Only a change between bands is
/// worth re-baking for.
fn band_of(bpm: f32) -> u8 {
    match bpm {
        b if b >= generate::VERY_FAST_BPM => 2,
        b if b >= generate::FAST_BPM => 1,
        _ => 0,
    }
}

/// A transport that moved more than this between two ticks did not play the
/// gap — it was dragged. Four beats is a bar at the signature everything
/// defaults to.
const JUMP_BEATS: f64 = 4.0;

/// How late after a downbeat PLAY still counts as on it, in beats: an eighth
/// of one, 62 ms at 120 — a hand pressing on the one, not a bar early.
const ENTRY_GRACE: f64 = 0.125;

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

    /// The tonic of the progression as it last read, as a pitch class.
    pub fn key(&self) -> u8 {
        self.key
    }

    /// The bar the band counts, as a signature — see [`ArrangerView::meter`].
    pub fn meter(&self) -> (u16, u16) {
        if let Some(m) = self.bar_index().and_then(|i| self.bar_meters.get(i)) {
            return *m;
        }
        if let Some(meter) = self.written_meter {
            return meter;
        }
        match session_bar() {
            Some(_) => choz_ports::transport().time_signature(),
            None => (self.style.meter.0 as u16, self.style.meter.1 as u16),
        }
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
            style: self.style.label,
            meter: self.meter(),
            key: self.key,
            groups: &self.groups,
            cells: self.cells(),
            pulse: match self.playing {
                true => self.pulse(),
                false => None,
            },
            beats_per_bar: self.style.beats_per_bar,
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

    /// How many subdivisions the bar has: the grouping's total, or one a beat.
    pub fn cells(&self) -> usize {
        let grouped: usize = self.groups.iter().map(|g| *g as usize).sum();
        match grouped > 0 {
            true => grouped,
            false => (self.style.beats_per_bar.round() as usize).max(1),
        }
    }

    /// Which subdivision of its bar the playhead is in.
    fn pulse(&self) -> Option<usize> {
        let bpb = self.style.beats_per_bar;
        if bpb <= 0.0 {
            return None;
        }
        let cells = self.cells();
        // In a chart that changes meter, the share of *this* bar gone by.
        let (into, bpb) = match self.bar_index().filter(|_| !self.bar_starts.is_empty()) {
            Some(i) => {
                let (n, d) = self.bar_meters[i];
                (
                    self.at - self.bar_starts[i],
                    n as f64 * 4.0 / d.max(1) as f64,
                )
            }
            None => (self.at.rem_euclid(bpb), bpb),
        };
        let into = into / bpb.max(1e-9) * cells as f64;
        Some((into.floor() as usize).min(cells.saturating_sub(1)))
    }

    /// Which bar the playhead is in, from 0 — `None` when the style has no
    /// bar length to divide by.
    fn bar_index(&self) -> Option<usize> {
        if !self.bar_starts.is_empty() {
            let i = self.bar_starts.partition_point(|s| *s <= self.at + 1e-9);
            return Some(i.saturating_sub(1));
        }
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

    /// How the band counts the bar: what the chart says, else the tab's own
    /// grouping, else the click's.
    fn grouping(&self) -> Vec<u8> {
        match self.written_groups.is_empty() {
            true => self.settings.grouping(),
            false => self.written_groups.clone(),
        }
    }

    /// The bar the band is written in, whatever the session counts: the
    /// chart's `meter`, else the style's own (a waltz is 3/4). What the
    /// metronome takes when it follows the arranger.
    pub fn own_meter(&self) -> (u16, u16) {
        // A chart that changes meter is in the bar the playhead is in.
        if let Some(m) = self.bar_index().and_then(|i| self.bar_meters.get(i)) {
            return *m;
        }
        self.written_meter
            .unwrap_or((self.style.meter.0 as u16, self.style.meter.1 as u16))
    }

    /// The chart's bar before any change of meter in it: its `meter` line, or
    /// the style's own.
    pub fn default_meter(&self) -> (u16, u16) {
        self.written_meter
            .unwrap_or((self.style.meter.0 as u16, self.style.meter.1 as u16))
    }

    /// Where the bar the playhead is in started, on the shared clock, in
    /// quarter notes — what the metronome counts its one from when the bar
    /// changes length under it. `None` before the band has counted anything.
    pub fn bar_origin_ppq(&self) -> Option<f64> {
        let last = self.last_pos?;
        let start = match self.bar_index() {
            Some(i) if !self.bar_starts.is_empty() => self.bar_starts[i],
            Some(i) => i as f64 * self.style.beats_per_bar,
            None => return None,
        };
        Some(last - (self.at - start))
    }

    /// The `meter` and `groups` the chart names — what LOAD hands the session
    /// so the click and the grid count the bar the band does.
    pub fn written_bar(&self) -> (Option<(u16, u16)>, &[u8]) {
        (self.written_meter, &self.written_groups)
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        self.settings.text = text.into();
        // The notation is what the text says it is: a chart loaded from a file
        // in letters must not leave the switch claiming degrees — see
        // [`chord::is_roman`].
        self.settings.roman = chord::is_roman(&self.settings.text);
        self.rebake();
    }

    pub fn set_role(&mut self, role: Role) {
        if self.settings.role != role {
            self.settings.role = role;
            self.rebake_keeping_place();
        }
    }

    /// Put one player in or out of the band, or move their fader.
    pub fn set_gain(&mut self, role: Role, gain: f32) {
        self.settings.set_gain(role, gain);
        // **Not `rebake`.** A fader is moved while the band is playing, and
        // baking from scratch put the playhead back at bar one — which is heard
        // as the form jumping *and* as a bar of downbeat hitting at once,
        // because the cursor was back at the top of a part that was already
        // half played. Taking somebody out of the band is not a rewind.
        self.rebake_keeping_place();
    }

    /// How this band groups the bar — `&[2, 2, 3]` for a 7/8 counted that way,
    /// the metronome's notation. An empty slice hands the count back to the
    /// click's own grouping.
    ///
    /// The band's, not the click's: the whole point of it being here is that the
    /// two can differ.
    pub fn set_groups(&mut self, groups: &[u8]) {
        let groups: Vec<u8> = groups.iter().copied().filter(|g| *g > 0).collect();
        if self.settings.groups == groups {
            return;
        }
        self.settings.groups = groups;
        // Not `rebake`: a grouping is changed while the band plays, and the
        // form must not jump back to bar one for it.
        self.rebake_keeping_place();
    }

    /// Read the chart as degrees or as letters. The text is rewritten, so the
    /// panel, the grid and the file all say the same thing.
    pub fn set_roman(&mut self, roman: bool) {
        if self.settings.roman == roman {
            return;
        }
        self.settings.text = chord::respell_text(&self.settings.text, roman);
        self.settings.roman = roman;
        // The notes are the same notes: nothing about the music moved, so the
        // playhead does not either. Re-baked all the same because the part
        // carries the symbols the panel prints.
        self.rebake_keeping_place();
    }

    pub fn set_seed(&mut self, seed: u32) {
        if self.settings.seed != seed {
            self.settings.seed = seed;
            // Another interpretation of the same form, from where it stands.
            self.rebake_keeping_place();
        }
    }

    /// Read the text and generate this role's part. Everything expensive the
    /// arranger does happens here, off the audio thread and off the tick.
    pub fn rebake(&mut self) {
        match chord::parse_progression(&self.settings.text) {
            Ok(prog) => {
                self.style = style::by_name(&prog.style);
                self.own_bpb = self.style.beats_per_bar;
                self.written_meter = prog.meter;
                self.written_groups = prog.groups.clone();
                // The bar the chart names, or the one the session counts when
                // somebody set one.
                // A chart that changes meter keeps its own: the session follows
                // it bar by bar (see choz-ui's `follow_arranger_meter`), and
                // letting the session's bar back in here would re-bake the form
                // every time it did.
                let bar = match (prog.meter, prog.heterometric()) {
                    (Some((num, den)), _) => Some((num as f64 * 4.0 / den as f64).max(0.25)),
                    (None, true) => None,
                    (None, false) => session_bar(),
                };
                if let Some(bar) = bar {
                    self.style.beats_per_bar = bar;
                }
                self.key = prog.key;
                self.bars = prog.bars.len();
                self.sections = prog.sections.clone();
                self.total = prog.beats(self.style.beats_per_bar);
                let default = self
                    .written_meter
                    .unwrap_or((self.style.meter.0 as u16, self.style.meter.1 as u16));
                (self.bar_starts, self.bar_meters) = match prog.heterometric() {
                    true => (0..prog.bars.len())
                        .map(|i| {
                            (
                                prog.bar_start(i, self.style.beats_per_bar),
                                prog.bars[i].meter.unwrap_or(default),
                            )
                        })
                        .unzip(),
                    false => (Vec::new(), Vec::new()),
                };
                // Every player in the band, on one timeline. Each is baked on
                // its own — that is what lets a tab play one of them — and the
                // fader is a scale on the velocity, because a tab has one
                // instrument and one level.
                self.baked.clear();
                self.bpm = tempo();
                self.groups = self.grouping();
                let ctx = generate::Bake {
                    bpm: self.bpm,
                    swing: self.settings.swing,
                    random: self.settings.random,
                    prob: self.settings.prob,
                    groups: &self.groups,
                };
                for (role, gain) in self.settings.band() {
                    let mut part =
                        generate::bake(&prog, &self.style, role, self.settings.seed, &ctx);
                    if (gain - 1.0).abs() > 1e-3 {
                        for note in &mut part {
                            note.vel = (note.vel as f32 * gain).round().clamp(1.0, 127.0) as u8;
                        }
                    }
                    self.baked.append(&mut part);
                }
                self.baked
                    .sort_by(|a, b| a.start.total_cmp(&b.start).then(a.note.cmp(&b.note)));
                self.error = None;
            }
            // The old part stays: a half-typed chord would otherwise stop the
            // band mid-bar, and the text is edited while it plays.
            //
            // **The key and the style still follow the text.** They are picked
            // from dialogues of their own and written into it, and a chart with
            // a typo further down used to swallow the pick: the box went on
            // saying the old style, which reads as a picker that does nothing.
            Err(e) => {
                let (key, style) = chord::settings_of(&self.settings.text);
                if let Some(key) = key {
                    self.key = key;
                }
                if let Some(name) = style {
                    self.style = style::by_name(&name);
                }
                self.error = Some(e.to_string());
            }
        }
        // The tick must not allocate, and a chord is at most a handful of notes
        // at a time.
        self.sounding.reserve(16);
        self.rewind();
    }

    /// Bake again if the tempo has moved into another band — the drums thin out
    /// above [`generate::FAST_BPM`] and again above [`generate::VERY_FAST_BPM`],
    /// and a part baked at 120 and played at 230 is the part this is here to
    /// stop.
    ///
    /// **Not called from `tick`**: this parses, generates and allocates, and
    /// `tick` is what a plugin's process callback runs. The host loop calls it
    /// — see `choz-ui`'s `tick_arrangers` — and it comes to nothing on the
    /// tempos either side of a line, which is nearly all of them.
    ///
    /// The playhead stays where it was: the form does not go back to the top
    /// because somebody moved the tempo.
    pub fn retune_to_tempo(&mut self) -> bool {
        let moved = band_of(tempo()) != band_of(self.bpm)
            || self.grouping() != self.groups
            // The bar the band should be in — the session's when somebody set
            // one, the style's own when the session is plain 4/4 — against the
            // one it was baked in. Checking only the first half left a band
            // baked in a waltz's 3 (the click still in 3/4 when the style
            // changed) counting 3 for ever once the click went back to 4/4.
            || (self.written_meter.is_none()
                && self.bar_starts.is_empty()
                && (session_bar().unwrap_or(self.own_bpb) - self.style.beats_per_bar).abs() > 1e-9);
        if !moved {
            return false;
        }
        self.rebake_keeping_place();
        true
    }

    /// Bake again and put the playhead back where it was.
    ///
    /// What everything that changes the *part* rather than the *form* uses: a
    /// fader, a musician joining or leaving, another seed, the tempo crossing a
    /// line. [`Self::rebake`] rewinds, which is right for a new chart and wrong
    /// for all of these — a band does not go back to bar one because somebody
    /// turned the piano up.
    pub fn rebake_keeping_place(&mut self) {
        let (at, last_pos) = (self.at, self.last_pos);
        self.rebake();
        self.at = at.min(self.total);
        // The note the playhead is in front of: walking from the top would play
        // the whole bar it landed in all over again, at once.
        self.cursor = self
            .baked
            .iter()
            .position(|n| n.start >= self.at)
            .unwrap_or(self.baked.len());
        self.last_pos = last_pos;
    }

    /// Back to the top of the progression, nothing sounding.
    pub fn rewind(&mut self) {
        self.at = 0.0;
        self.cursor = 0;
        self.last_pos = None;
        self.enter_at = None;
    }

    /// **PLAY starts the progression from the top.** Bar one, on the clock's
    /// downbeat — what the button with a triangle on it means everywhere.
    pub fn play(&mut self) {
        self.playing = true;
        self.follow = false;
        self.rewind();
    }

    /// **PAUSE stops without losing the place.** Nothing is left sounding — a
    /// held chord through a pause is a drone — but the playhead stays in the bar
    /// it was in, and the next press carries on from there.
    pub fn pause(&mut self, out: &mut Vec<ArpEvent>) {
        self.playing = false;
        self.follow = false;
        self.silence(out);
        // Not `rewind`: the place is the point. The clock will have moved on by
        // the time it comes back, so the next tick measures from wherever it is
        // then rather than counting the gap as music.
        self.last_pos = None;
    }

    /// Carry on from where PAUSE left it.
    pub fn resume(&mut self) {
        self.playing = true;
        self.follow = false;
        // The clock has run on while it was out; the next tick measures from
        // wherever it is now rather than counting the gap as music.
        self.last_pos = None;
        self.enter_at = None;
        // Back in at the top of the bar it was left in, because it comes back
        // in on a downbeat: carrying on from mid-bar would put the rest of the
        // bar over the click's "one".
        let bpb = self.style.beats_per_bar;
        if let Some(start) = self
            .bar_index()
            .and_then(|i| self.bar_starts.get(i).copied())
        {
            self.at = start;
            self.cursor = self
                .baked
                .iter()
                .position(|n| n.start >= self.at - 1e-9)
                .unwrap_or(self.baked.len());
        } else if bpb > 0.0 {
            self.at = (self.at / bpb).floor() * bpb;
            self.cursor = self
                .baked
                .iter()
                .position(|n| n.start >= self.at - 1e-9)
                .unwrap_or(self.baked.len());
        }
    }

    /// The pause button: out of the band, or back into it at the same bar.
    pub fn toggle_pause(&mut self, out: &mut Vec<ArpEvent>) {
        match self.playing {
            true => self.pause(out),
            false => self.resume(),
        }
    }

    /// Started **by the transport** — its own button was not pressed.
    ///
    /// The same door the sequencer has, and for the same reason: a tab with a
    /// chart on it did nothing when the transport rolled, so the one button
    /// that starts the rack was the obvious way to start the band and the one
    /// that did nothing at all. MIDI clock's START and CONTINUE come through
    /// here too, because they come through the transport — which is what makes
    /// the band follow an external clock the way the metronome does.
    ///
    /// Its own clock is not counted until the transport actually moves:
    /// counting time in the gap before the first audio block is what fired the
    /// downbeat twice in [`crate::artifacts::seq::Seq`].
    pub fn play_on_transport(&mut self) {
        self.play();
        self.follow = true;
    }

    pub fn stop(&mut self, out: &mut Vec<ArpEvent>) {
        self.playing = false;
        self.follow = false;
        self.silence(out);
        self.rewind();
    }

    /// Let go of everything this arranger has sounding. `PANIC`, and switching
    /// it off.
    pub fn silence(&mut self, out: &mut Vec<ArpEvent>) {
        for (note, _, _) in self.sounding.drain(..) {
            out.push(ArpEvent::Off { note, at: 0 });
        }
    }

    /// Play whatever falls between the last tick and this one.
    ///
    /// Takes the instant rather than reading the clock, for the reason the
    /// sequencer does: the whole thing is testable without sleeping.
    pub fn tick(&mut self, now: Instant, out: &mut Vec<(Role, ArpEvent)>) {
        // `now` is no longer what the band counts — the shared clock is — but
        // the signature stays: the interface drives all three artifacts with
        // the same instant, and the gate lengths are still wall-clock.
        let _ = now;
        if !self.settings.on || !self.playing || self.baked.is_empty() || self.total <= 0.0 {
            // Letting go is not aimed anywhere: a note-off goes wherever the
            // note went — see [`choz_ports::AudioSource::note_on_zone`].
            let mut offs = Vec::new();
            self.silence(&mut offs);
            out.extend(offs.into_iter().map(|e| (self.settings.role, e)));
            return;
        }
        let Some(delta) = self.advance() else {
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
                    out.push((
                        note.role,
                        ArpEvent::On {
                            note: note.note,
                            vel: note.vel,
                            at: 0,
                        },
                    ));
                    self.sounding.push((note.note, note.len, note.role));
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
    /// Off the one shared position — the transport while it rolls, the free
    /// clock otherwise — so nothing drifts, a stall skips rather than catching
    /// up in a burst, and the band counts the bar the metronome is clicking.
    fn advance(&mut self) -> Option<f64> {
        let transport = choz_ports::transport();
        // Started by the transport and the transport has not begun to roll:
        // wait for it rather than counting the free clock, which would play the
        // first bar twice.
        match transport.playing() {
            true => self.follow = false,
            false if self.follow => return None,
            false => {}
        }
        let pos = transport.position_ppq();
        let delta = match self.last_pos {
            Some(last) => pos - last,
            // Coming in: **on the next downbeat of the bar the metronome
            // clicks.** The form still starts where the button says — PLAY is
            // bar one, PAUSE the bar it was left in — but *when* is the
            // clock's: coming in the instant the button was pressed put the
            // band's "one" anywhere in the click's bar.
            None => {
                let bar = transport.bar_quarters().max(0.25);
                let rolling = transport.playing();
                // Worked out again only when the clock under it changed, or
                // went back past it (a rewind): otherwise it is the one PLAY
                // was pressed before.
                let stale = self
                    .enter_at
                    .is_none_or(|(at, on)| on != rolling || at - pos > bar + 1e-9);
                if stale {
                    let origin = transport.bar_origin();
                    let this = origin + ((pos - origin) / bar + 1e-9).floor() * bar;
                    // Pressed on the one — a hair late, as a hand is — is this
                    // bar, not a whole bar's wait for the next.
                    let at = match pos - this <= ENTRY_GRACE {
                        true => this,
                        false => this + bar,
                    };
                    self.enter_at = Some((at, rolling));
                }
                let target = self.enter_at.map(|(at, _)| at).unwrap_or(pos);
                if pos < target - 1e-9 {
                    return None;
                }
                self.enter_at = None;
                // The interface stalled past it (a dialogue, a load): come in on
                // the next one rather than play the gap at once.
                if pos - target > JUMP_BEATS {
                    return None;
                }
                self.last_pos = Some(target);
                return Some(pos - target);
            }
        };
        self.last_pos = Some(pos);
        // Rewound, or dragged: play from there rather than playing everything
        // in between at once.
        if !(0.0..=JUMP_BEATS).contains(&delta) {
            return None;
        }
        Some(delta)
    }

    /// Let go of what has run out of length.
    fn release(&mut self, delta: f64, out: &mut Vec<(Role, ArpEvent)>) {
        let mut i = 0;
        while i < self.sounding.len() {
            self.sounding[i].1 -= delta;
            match self.sounding[i].1 <= 0.0 {
                true => {
                    let (note, _, role) = self.sounding.swap_remove(i);
                    // Only when nobody else is still holding it: a band in one
                    // tab has the bass and the piano landing on the same note,
                    // and the shorter of the two used to take the other's note
                    // off with it.
                    if !self.sounding.iter().any(|(n, _, _)| *n == note) {
                        out.push((role, ArpEvent::Off { note, at: 0 }));
                    }
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

    /// A band in one tab: every player that is switched on is in the part, the
    /// fader is what they are worth, and nobody is dropped for landing on the
    /// same note as somebody else.
    #[test]
    fn a_band_plays_every_part_it_was_given() {
        // The tempo is a global and baking reads it: without the lock another
        // test moving it mid-bake is two different parts.
        let _g = crate::test_locks::transport();
        let mut one = arranger(Role::Bass, 7);
        let alone = one.part().len();
        one.set_gain(Role::Bass, 1.0);
        one.set_gain(Role::Drums, 0.5);
        let band = one.part().len();
        let drums_alone = arranger(Role::Drums, 7).part().len();
        assert_eq!(band, alone + drums_alone, "the band is not both parts");
        assert!(
            one.part().windows(2).all(|w| w[0].start <= w[1].start),
            "the merged part is not in time order"
        );
        // The fader: the drums at half are quieter than the drums on their own,
        // and the bass at full is untouched.
        // By role and not by pitch alone: a bass playing its bottom C is
        // MIDI 36, which is also the kick, and in a merged band the two are
        // the same number.
        let loudest_kick = |a: &Arranger| {
            a.part()
                .iter()
                .filter(|n| n.note == gm::KICK && n.role == Role::Drums)
                .map(|n| n.vel)
                .max()
                .unwrap_or(0)
        };
        assert!(
            loudest_kick(&one) < loudest_kick(&arranger(Role::Drums, 7)),
            "the fader does not move the part"
        );
        // Out of the band, and the part is the bass alone again.
        one.set_gain(Role::Drums, 0.0);
        assert_eq!(one.part().len(), alone, "a fader at zero still plays");
        assert_eq!(one.settings.role, Role::Bass, "the tab is fitted to nobody");
    }

    /// The piano and the guitar hold opposite lengths. Two chord instruments
    /// playing the same rhythm is one chord instrument with a chorus on it:
    /// whichever one is chopping, the other is ringing.
    #[test]
    fn the_guitar_plays_against_the_pianos_length() {
        // The tempo is a global and baking reads it: without the lock another
        // test moving it mid-bake is two different parts.
        let _g = crate::test_locks::transport();
        for name in style::ALL.iter().map(|s| s.name) {
            let style = style::by_name(name);
            let text = format!("key = C\nstyle = {name}\n|| I | IV | V | I ||");
            let prog = chord::parse_progression(&text).unwrap();
            let piano = generate::bake(&prog, &style, Role::Piano, 5, &generate::Bake::at(120.0));
            let guitar = generate::bake(&prog, &style, Role::Guitar, 5, &generate::Bake::at(120.0));
            let mean = |part: &[generate::Note]| match part.is_empty() {
                true => 0.0,
                false => part.iter().map(|n| n.len).sum::<f64>() / part.len() as f64,
            };
            // A comp whose hits are closer together than either hold can
            // express says nothing about this rule: both parts are cut off by
            // the next hit, not by how long they were asked to ring.
            let hits = style.comp.hits;
            let wrap = style.beats_per_bar - hits[hits.len() - 1] + hits[0];
            let room = hits.windows(2).map(|w| w[1] - w[0]).fold(wrap, f64::min);
            // …and neither does a comp whose piano already holds nearly all
            // the room there is: the counter-length has nowhere longer to go.
            if room < 1.0 || style.comp.hold > room * 0.75 {
                continue;
            }
            let (p, g) = (mean(&piano), mean(&guitar));
            let short = style.comp.hold <= 1.0;
            match short {
                true => assert!(g > p, "{name}: piano {p:.2}, guitar {g:.2} — both short"),
                false => assert!(g < p, "{name}: piano {p:.2}, guitar {g:.2} — both long"),
            }
        }
        // And the rule itself, at the edges: nothing is held past the bar, and
        // nothing comes out as a note of no length.
        for bpb in [3.0, 4.0, 7.0] {
            for piano in [0.2, 0.5, 1.0, 1.5, 3.0, 8.0] {
                let g = generate::counter_hold(piano, bpb);
                assert!(g > 0.0 && g <= bpb, "{piano} in {bpb}/4 gave {g}");
            }
        }
    }

    /// Same text, same style, same seed: the same notes, one at a time. An
    /// interpretation you liked is one you can get back.
    #[test]
    fn a_seed_names_an_interpretation() {
        // The tempo is a global and baking reads it: without the lock another
        // test moving it mid-bake is two different parts.
        let _g = crate::test_locks::transport();
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
        // The tempo is a global and baking reads it: without the lock another
        // test moving it mid-bake is two different parts.
        let _g = crate::test_locks::transport();
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
        // The tempo is a global and baking reads it: without the lock another
        // test moving it mid-bake is two different parts.
        let _g = crate::test_locks::transport();
        let style = style::by_name("shfblues");
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
        // The tempo is a global and baking reads it: without the lock another
        // test moving it mid-bake is two different parts.
        let _g = crate::test_locks::transport();
        let lengths: Vec<f64> = Role::ALL.map(|r| arranger(r, 42).beats()).to_vec();
        assert!(
            lengths.iter().all(|l| (*l - 48.0).abs() < 1e-9),
            "{lengths:?}"
        );
    }

    /// A walking bass walks: the failure this is here to catch is `C C C C`.
    #[test]
    fn the_bass_does_not_stand_still() {
        // The tempo is a global and baking reads it: without the lock another
        // test moving it mid-bake is two different parts.
        let _g = crate::test_locks::transport();
        let bass = arranger(Role::Bass, 42);
        let notes: Vec<u8> = bass.part().iter().map(|n| n.note).collect();
        assert!(
            notes.len() >= 40,
            "a beat a bar is not a bass line: {}",
            notes.len()
        );
        let distinct = notes.iter().collect::<std::collections::HashSet<_>>().len();
        assert!(
            distinct >= 8,
            "only {distinct} different notes in twelve bars"
        );
        assert!(
            notes.windows(2).filter(|w| w[0] == w[1]).count() * 3 < notes.len(),
            "the same note over and over: {notes:?}"
        );
    }

    /// A comp leads its voices: `C7` to `F7` moves by a step or two, not by an
    /// octave, and that is the difference between hands and a lookup table.
    #[test]
    fn the_voicings_lead() {
        // The tempo is a global and baking reads it: without the lock another
        // test moving it mid-bake is two different parts.
        let _g = crate::test_locks::transport();
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
        assert_eq!(
            jumps, 0,
            "{jumps} voicings jumped more than a fourth: {tops:?}"
        );
    }

    /// A guitar strums: the pick crosses the strings one at a time, and the
    /// hands on a piano do not. Same chords, same rhythm, different instrument
    /// — which is the whole of why the role exists.
    #[test]
    fn the_guitar_strums_and_the_piano_does_not() {
        // The tempo is a global and baking reads it: without the lock another
        // test moving it mid-bake is two different parts.
        let _g = crate::test_locks::transport();
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

    /// A text that does not parse keeps the last part that did: a half-typed
    /// chord must not silence the band mid-bar.
    #[test]
    fn a_typo_does_not_stop_the_music() {
        // The tempo is a global and baking reads it: without the lock another
        // test moving it mid-bake is two different parts.
        let _g = crate::test_locks::transport();
        let mut a = arranger(Role::Bass, 42);
        let before = a.part().to_vec();
        a.set_text("key = C\n|| I7 | Zq9 ||");
        assert!(a.error().is_some());
        assert_eq!(
            a.part(),
            before,
            "the last part that read has to keep playing"
        );
        a.set_text("key = C\n|| I7 | IV7 ||");
        assert!(a.error().is_none());
        assert_eq!(a.bars(), 2);
    }

    /// The style comes out of the text, and an unknown one plays a feel you can
    /// hear is wrong rather than nothing at all.
    #[test]
    fn the_text_chooses_the_style() {
        // The tempo is a global and baking reads it: without the lock another
        // test moving it mid-bake is two different parts.
        let _g = crate::test_locks::transport();
        let mut a = arranger(Role::Drums, 42);
        assert_eq!(a.style().name, "shfblues");
        a.set_text("key = C\nstyle = slwbossa\n|| Im7 | IVm7 ||");
        assert_eq!(a.style().name, "slwbossa");
        a.set_text("key = C\nstyle = no_such_style\n|| Im7 ||");
        assert_eq!(
            a.style().name,
            style::ALL[0].name,
            "unknown styles fall back"
        );
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
            .filter_map(|(_, e)| match e {
                ArpEvent::On { note, vel, .. } => Some((*note, *vel)),
                _ => None,
            })
            .collect();
        let want: Vec<(u8, u8)> = baked.iter().map(|n| (n.note, n.vel)).collect();
        assert_eq!(ons, want, "a chorus is the part, in order");

        // Everything that was started gets let go of: the ones that ran out of
        // length as it played, and whatever was still sounding when it stopped.
        let mut stopped = Vec::new();
        a.stop(&mut stopped);
        let offs = out
            .iter()
            .filter(|(_, e)| matches!(e, ArpEvent::Off { .. }))
            .count()
            + stopped
                .iter()
                .filter(|e| matches!(e, ArpEvent::Off { .. }))
                .count();
        assert_eq!(
            offs,
            ons.len(),
            "a note-on with no note-off is a stuck note"
        );

        // And the form comes round rather than stopping at the twelfth bar.
        a.play();
        out.clear();
        for beat in 48..=60 {
            t.set_position_beats(beat as f64);
            a.tick(now + Duration::from_millis(beat * 10), &mut out);
        }
        assert!(
            out.iter().any(|(_, e)| matches!(e, ArpEvent::On { .. })),
            "the second chorus never started"
        );
        t.set_playing(false);
    }

    /// The fills are playable as written: in time order, inside their own
    /// length, on a drum the kit has, and at a velocity that is a note rather
    /// than a note-off. Data lifted off a MIDI library needs its own check,
    /// because a typo here is a part nobody can hear the mistake in.
    #[test]
    fn every_fill_is_playable() {
        let kit = [
            gm::KICK,
            gm::SNARE,
            gm::HAT_CLOSED,
            gm::HAT_OPEN,
            gm::CRASH,
            gm::RIDE,
            gm::TOM_LO,
            gm::TOM_MID,
            gm::TOM_HI,
        ];
        let mut eighths = 0;
        for (i, f) in generate::FILLS.iter().enumerate() {
            assert!(
                f.beats > 0.0 && f.beats <= 2.0,
                "fill {i}: {} beats",
                f.beats
            );
            assert!(!f.hits.is_empty(), "fill {i} plays nothing");
            let mut last = -1.0;
            for (pos, note, vel) in f.hits.iter().copied() {
                assert!(pos >= last, "fill {i} is not in time order");
                last = pos;
                assert!(pos < f.beats, "fill {i}: a hit past its own end");
                assert!(kit.contains(&note), "fill {i}: {note} is not on the kit");
                assert!(
                    (0.5..=1.2).contains(&vel),
                    "fill {i}: {vel} of the style's velocity"
                );
            }
            // Every fill ends on its loudest hit: that is what hands the bar
            // back to the downbeat.
            let last_vel = f.hits.last().map(|(_, _, v)| *v).unwrap_or(0.0);
            assert!(
                f.hits.iter().all(|(_, _, v)| *v <= last_vel + 1e-6),
                "fill {i} does not build"
            );
            if f.hits
                .windows(2)
                .all(|w| w[1].0 - w[0].0 > 0.4 || w[1].0 == w[0].0)
            {
                eighths += 1;
            }
        }
        // …and enough of them are eighths that a fast tempo still has a choice.
        assert!(eighths >= 4, "only {eighths} fills a fast tempo can play");
    }

    /// Moving a fader — or taking a musician out of the band — does not rewind
    /// the form, and does not dump a bar of downbeat at whoever moved it.
    ///
    /// Both came from the same place: baking put the playhead back at bar one,
    /// so the next tick played everything from the top of the part at once,
    /// which is heard as a loud note out of nowhere.
    #[test]
    fn a_fader_does_not_restart_the_form() {
        let _g = crate::test_locks::transport();
        let t = choz_ports::transport();
        t.set_playing(false);
        t.set_bpm(120.0);

        let mut a = arranger(Role::Bass, 3);
        a.play();
        // Halfway through the ninth bar, with a bar already played.
        a.at = 34.0;
        a.cursor = a
            .part()
            .iter()
            .position(|n| n.start >= 34.0)
            .unwrap_or_default();

        a.set_gain(Role::Piano, 1.0);
        assert!((a.at - 34.0).abs() < 1e-9, "the form went back to bar one");
        assert_eq!(
            a.part().iter().filter(|n| n.start < a.at).count(),
            a.cursor,
            "the cursor is not where the playhead is"
        );

        // …and the next tick plays what is *due*, not the whole first bar.
        let now = Instant::now();
        let mut out = Vec::new();
        a.tick(now, &mut out);
        a.tick(now + Duration::from_millis(30), &mut out);
        let ons = out
            .iter()
            .filter(|(_, e)| matches!(e, ArpEvent::On { .. }))
            .count();
        assert!(ons <= 4, "{ons} notes at once out of a fader move");
    }

    /// The bar the session counts is the bar the band plays, and the grouping
    /// is what it leans on: a 7/8 counted 3+2+2 is not seven equal eighths.
    /// **What the chart says wins.** A 7/8 file plays 7/8 in a 4/4 session,
    /// counted the way it is written — and that is not a difference the tempo
    /// loop re-bakes over on every tick.
    #[test]
    fn a_chart_that_names_its_bar_is_played_in_it() {
        let _g = crate::test_locks::transport();
        let t = choz_ports::transport();
        t.set_time_signature(4, 4);
        let arr = Arranger::new(ArrangerSettings {
            on: true,
            text: "meter = 7/8\ngroups = 3+2+2\n| C | F |".into(),
            ..Default::default()
        });
        assert!(arr.error().is_none(), "{:?}", arr.error());
        assert_eq!(arr.meter(), (7, 8));
        assert!(
            (arr.beats() - 7.0).abs() < 1e-9,
            "two bars of 3.5: {}",
            arr.beats()
        );
        assert_eq!(arr.view().groups, &[3, 2, 2]);
        assert_eq!(arr.written_bar(), (Some((7, 8)), &[3u8, 2, 2][..]));
        let mut arr = arr;
        assert!(
            !arr.retune_to_tempo(),
            "a written bar is re-baked every tick"
        );
    }

    /// **Drum & bass is a two-bar two-step at 174**, measured off
    /// `rhythms/drumnbass.mid`: bar A has the kick on one and the and-of-three,
    /// bar B answers with a kick on the "a" of two and a snare picking up into
    /// the next bar; the hats run in sixteenths, so the ghosts land somewhere
    /// else every bar. The comp is a Rhodes — a pad's slow attack never opened
    /// at this tempo, and the chord came and went.
    #[test]
    fn drum_and_bass_plays_a_two_bar_two_step() {
        let _g = crate::test_locks::transport();
        let style = style::by_name("drumnbass");
        assert_eq!(style.name, "drumnbass", "the style is in the table");
        assert_eq!(style.label, "Drum & Bass");
        assert_eq!(style.swing, 0.0, "straight, not shuffled");
        assert_eq!(style.piano_gm.first(), Some(&4), "a Rhodes, not a slow pad");
        let prog =
            chord::parse_progression("style = drumnbass\n| Cm7 | Abmaj7 | Fm7 | G7 |").unwrap();
        let part = generate::bake(&prog, &style, Role::Drums, 1, &generate::Bake::at(174.0));
        let hits = |key: u8, bar: usize| -> Vec<f64> {
            let from = bar as f64 * 4.0;
            let mut at: Vec<f64> = part
                .iter()
                .filter(|n| n.note == key && n.vel >= 70)
                .map(|n| ((n.start - from) * 4.0).round() / 4.0)
                .filter(|p| (0.0..4.0).contains(p))
                .collect();
            at.dedup();
            at
        };
        assert_eq!(
            hits(gm::KICK, 0),
            vec![0.0, 2.5],
            "bar A: one and the and-of-three"
        );
        assert_eq!(hits(gm::SNARE, 0), vec![1.0, 3.0], "bar A: two and four");
        assert_eq!(
            hits(gm::KICK, 1),
            vec![0.0, 1.75, 2.5],
            "bar B pushes the kick"
        );
        assert_eq!(hits(gm::SNARE, 1), vec![1.0, 3.0, 3.75], "and picks up");
        let hats = part
            .iter()
            .filter(|n| n.note == gm::HAT_CLOSED && n.start < 4.0)
            .count();
        assert!(hats >= 12, "sixteenth hats at 174: {hats}");
        // Other styles keep their one-bar groove.
        assert!(style::by_name("strtrock").drums.kick_b.is_empty());
    }

    /// **Tarkus is heterometric**: measured off `rhythms/tarkus.mid`, a 5/4
    /// of its own and a groove for each signature the piece changes to. In a
    /// chart that changes meter every bar is as long as its signature, plays
    /// the groove measured for it, and the arranger says which bar — and which
    /// meter — the playhead is in, for the click to follow.
    #[test]
    fn a_heterometric_style_plays_each_bar_in_its_own_meter() {
        let _g = crate::test_locks::transport();
        let style = style::by_name("tarkus");
        assert_eq!(style.name, "tarkus");
        assert_eq!(style.meter, (5, 4));
        for want in [(3, 4), (4, 4), (5, 8), (7, 8), (9, 8), (12, 8)] {
            assert!(
                style.meters.iter().any(|g| g.meter == want),
                "no {want:?} groove"
            );
        }
        let text = include_str!("../../../../../assets/tarkus.chord");
        let prog = chord::parse_progression(text).unwrap();
        assert!(prog.heterometric());
        let part = generate::bake(&prog, &style, Role::Drums, 1, &generate::Bake::at(160.0));
        // Every bar starts on a kick, wherever its length put it.
        for i in 0..prog.bars.len() {
            let at = prog.bar_start(i, style.beats_per_bar);
            assert!(
                part.iter()
                    .any(|n| n.note == gm::KICK && (n.start - at).abs() < 0.1),
                "bar {} ({:?}) has no downbeat at {at}",
                i + 1,
                prog.bars[i].meter
            );
        }
        // Nothing of a bar spills into the next.
        let three = (0..prog.bars.len())
            .find(|i| prog.bars[*i].meter == Some((3, 4)))
            .unwrap();
        let (from, len) = (prog.bar_start(three, 5.0), prog.bar_beats(three, 5.0));
        assert_eq!(len, 3.0);
        let groove = style.groove_for(Some((3, 4))).unwrap();
        let kicks: Vec<f64> = part
            .iter()
            .filter(|n| n.note == gm::KICK && n.start >= from - 0.05 && n.start < from + len - 0.05)
            .map(|n| ((n.start - from) * 4.0).round() / 4.0)
            .collect();
        assert_eq!(
            kicks.first(),
            groove.kick.first(),
            "the 3/4 bar plays the 3/4 groove"
        );

        // The arranger follows it: the meter of the bar the playhead is in.
        let t = choz_ports::transport();
        t.set_playing(false);
        t.set_bpm(120.0);
        t.set_sample_rate(48_000);
        t.set_time_signature(5, 4);
        t.set_bar_origin(0.0);
        t.set_free_samples(0);
        let mut arr = Arranger::new(ArrangerSettings {
            on: true,
            text: text.into(),
            ..Default::default()
        });
        assert!(arr.error().is_none(), "{:?}", arr.error());
        arr.play();
        let mut out = Vec::new();
        let q = 24_000.0;
        let at = |beat: f64| t.set_free_samples((beat * q) as u64);
        let mut seen = Vec::new();
        let mut beat = 0.0;
        while beat < arr.beats() {
            at(beat);
            arr.tick(Instant::now(), &mut out);
            if seen.last() != Some(&arr.own_meter()) {
                seen.push(arr.own_meter());
            }
            beat += 0.1;
        }
        for want in [
            (5, 4),
            (3, 4),
            (4, 4),
            (2, 4),
            (5, 8),
            (7, 8),
            (6, 8),
            (9, 8),
            (12, 8),
            (6, 4),
        ] {
            assert!(seen.contains(&want), "never in {want:?}: {seen:?}");
        }
        let origin = arr.bar_origin_ppq().unwrap();
        let bar = arr.view().bar - 1;
        assert!(
            ((arr.last_pos.unwrap() - origin) - (arr.at - arr.bar_starts[bar])).abs() < 1e-6,
            "the origin is the bar's downbeat on the clock"
        );
        t.set_time_signature(4, 4);
    }

    /// **A style change is a bar change**, even when the click was still in
    /// the last style's meter as it happened: a band baked in a waltz's 3
    /// while the session said 3/4 moves to 4 once the session is back to 4/4,
    /// instead of counting 3 under a panel that says 4/4.
    #[test]
    fn a_style_change_leaves_no_bar_behind() {
        let _g = crate::test_locks::transport();
        let t = choz_ports::transport();
        t.set_time_signature(3, 4); // the click, still following a waltz
        let mut arr = Arranger::new(ArrangerSettings {
            on: true,
            text: "style = strtrock\n| C | F |".into(),
            ..Default::default()
        });
        assert_eq!(arr.view().beats_per_bar, 3.0, "baked in the click's 3/4");
        t.set_time_signature(4, 4); // the click follows the new style
        assert!(arr.retune_to_tempo(), "the bar the band counts moved");
        assert_eq!(arr.view().beats_per_bar, 4.0);
        assert_eq!(arr.meter(), (4, 4));
        assert!(!arr.retune_to_tempo(), "and it settles");
    }

    /// **PLAY on the free clock, then the transport**: the downbeat it was
    /// waiting for was on the other clock, and the band comes in on the
    /// transport's — not bars later, where the free clock's number would be.
    #[test]
    fn play_follows_the_clock_it_ends_up_on() {
        let _g = crate::test_locks::transport();
        let t = choz_ports::transport();
        t.set_sample_rate(48_000);
        t.set_bpm(120.0);
        t.set_time_signature(4, 4);
        t.set_bar_origin(0.0);
        t.set_playing(false);
        t.set_free_samples(24_000 * 37); // the free clock, bars in
        let mut arr = arranger(Role::Drums, 1);
        arr.play();
        let mut out = Vec::new();
        arr.tick(Instant::now(), &mut out);
        assert!(out.is_empty(), "waiting for the free clock's downbeat");
        // The rack's PLAY: the transport rolls from the top.
        t.set_playing(true);
        t.set_position_beats(0.0);
        arr.tick(Instant::now(), &mut out);
        t.set_position_beats(0.05);
        arr.tick(Instant::now(), &mut out);
        assert!(
            out.iter().any(|(_, e)| matches!(e, ArpEvent::On { .. })),
            "the band came in on the transport's one"
        );
        t.set_playing(false);
    }

    /// **PLAY comes in on the downbeat it was pressed before**, whatever the
    /// ticks land on. The wait used to be worked out again every tick and
    /// moved on a bar whenever no tick hit the downbeat to the millionth of a
    /// beat — at 285 the band never came in. And pressed on the one, it is
    /// that bar.
    #[test]
    fn play_comes_in_on_the_next_downbeat_whatever_the_ticks() {
        let _g = crate::test_locks::transport();
        let t = choz_ports::transport();
        t.set_playing(false);
        t.set_sample_rate(48_000);
        t.set_time_signature(4, 4);
        t.set_bar_origin(0.0);
        for (bpm, press, want) in [
            (285.0f32, 13.37, 16.0),
            (174.0, 13.37, 16.0),
            (120.0, 16.05, 16.0),
            (120.0, 16.3, 20.0),
        ] {
            t.set_bpm(bpm);
            let spb = 48_000.0 * 60.0 / bpm as f64;
            // The interface's own rate: a tick every 5 ms, never on a beat.
            let step = 0.005 * bpm as f64 / 60.0;
            let mut arr = arranger(Role::Drums, 1);
            let mut beat = press;
            t.set_free_samples((beat * spb) as u64);
            arr.play();
            let mut first = None;
            let mut out = Vec::new();
            while beat < press + 12.0 {
                t.set_free_samples((beat * spb) as u64);
                out.clear();
                arr.tick(Instant::now(), &mut out);
                if out.iter().any(|(_, e)| matches!(e, ArpEvent::On { .. })) {
                    first = Some(beat);
                    break;
                }
                beat += step;
            }
            let first = first.unwrap_or_else(|| panic!("{bpm} bpm: never came in"));
            // On the downbeat, or at once when pressed on the one.
            let due = f64::max(want, press);
            assert!(
                first >= due - 1e-9 && first - due < 2.0 * step + 1e-6,
                "{bpm} bpm pressed at {press}: in at {first}, due at {due}"
            );
        }
    }

    /// **PLAY waits for the downbeat.** Pressed a beat and a bit into the
    /// click's bar, the band's bar one starts on the next "one" — not at the
    /// press, which put the arranger's bar anywhere inside the metronome's.
    #[test]
    fn play_comes_in_on_the_clicks_downbeat() {
        let _g = crate::test_locks::transport();
        let t = choz_ports::transport();
        t.set_playing(false);
        t.set_bpm(120.0);
        t.set_sample_rate(48_000);
        t.set_time_signature(4, 4);
        let quarter = 24_000u64; // at 120 bpm
        let at = |q: f64| t.set_free_samples((q * quarter as f64) as u64);
        let mut arr = arranger(Role::Bass, 1);
        at(1.3);
        arr.play();
        let mut out = Vec::new();
        for step in 0..26 {
            at(1.3 + step as f64 * 0.1); // up to 3.8
            arr.tick(Instant::now(), &mut out);
        }
        assert!(out.is_empty(), "played before the downbeat: {out:?}");
        at(4.0);
        arr.tick(Instant::now(), &mut out);
        at(4.05);
        arr.tick(Instant::now(), &mut out);
        assert!(!out.is_empty(), "the downbeat brought nothing in");
        assert_eq!(arr.view().bar, 1, "and it is bar one");
    }

    #[test]
    fn the_session_bar_and_its_grouping_reach_the_part() {
        // The tempo is a global and baking reads it: without the lock another
        // test moving it mid-bake is two different parts.
        let _g = crate::test_locks::transport();
        let prog = chord::parse_progression(DEFAULT_TEXT).unwrap();
        let style = style::Style {
            beats_per_bar: 3.5,
            ..style::by_name("strtrock")
        };
        let plain = generate::bake(&prog, &style, Role::Drums, 6, &generate::Bake::at(120.0));
        let grouped = generate::bake(
            &prog,
            &style,
            Role::Drums,
            6,
            &generate::Bake {
                groups: &[3, 2, 2],
                ..generate::Bake::at(120.0)
            },
        );
        assert_eq!(plain.len(), grouped.len(), "the grouping added notes");
        // The hits that fall where a group starts are the ones that changed:
        // 3+2+2 of an eighth is 1.5 and 2.5 beats in.
        let louder = plain
            .iter()
            .zip(&grouped)
            .filter(|(a, b)| b.vel > a.vel)
            .count();
        assert!(louder > 0, "nothing leans on the groups");
        for (a, b) in plain.iter().zip(&grouped) {
            let into = a.start.rem_euclid(3.5);
            let on_group = (into - 1.5).abs() < 0.13 || (into - 2.5).abs() < 0.13;
            assert!(
                b.vel >= a.vel,
                "the grouping took weight off a note instead of adding it"
            );
            if b.vel > a.vel {
                assert!(on_group, "an accent at {into} of a 3+2+2 bar");
            }
        }
    }

    /// **The band's grouping is its own.** A tab set to 3+2+2 counts 3+2+2
    /// whatever the click is counting, and a tab that says nothing follows the
    /// click — which is every project written before the band had one.
    #[test]
    fn the_band_groups_the_bar_its_own_way() {
        let _g = crate::test_locks::transport();
        let click = crate::artifacts::metronome::metronome();
        let had = click.groups();
        click.set_groups(&[2, 2, 3]);

        let mut a = arranger(Role::Drums, 6);
        assert_eq!(
            a.settings.grouping(),
            vec![2, 2, 3],
            "with none of its own it counts what the click counts"
        );
        a.set_groups(&[3, 2, 2]);
        assert_eq!(a.settings.grouping(), vec![3, 2, 2], "its own wins");
        assert_eq!(a.view().groups, [3, 2, 2], "the panel draws the band's bar");
        // Handing it back is asking for the click's again.
        a.set_groups(&[]);
        assert_eq!(a.settings.grouping(), vec![2, 2, 3]);

        click.set_groups(&had);
    }

    /// A bar written with its own weights is accented where *it* says, not where
    /// the tab's grouping does: the whole point of a weight per bar.
    #[test]
    fn a_bar_with_its_own_weights_is_accented_by_them() {
        let _g = crate::test_locks::transport();
        let style = style::Style {
            beats_per_bar: 3.5,
            ..style::by_name("strtrock")
        };
        let even = chord::parse_progression("|| C | C ||").unwrap();
        let weighted = chord::parse_progression("|| C:3 -:2 -:2 | C:3 -:2 -:2 ||").unwrap();
        let ctx = generate::Bake::at(120.0);
        let plain = generate::bake(&even, &style, Role::Drums, 6, &ctx);
        let grouped = generate::bake(&weighted, &style, Role::Drums, 6, &ctx);
        assert_eq!(plain.len(), grouped.len(), "the weights added notes");
        let louder = plain
            .iter()
            .zip(&grouped)
            .filter(|(a, b)| b.vel > a.vel)
            .count();
        assert!(louder > 0, "nothing leans on the bar's own weights");
        for (a, b) in plain.iter().zip(&grouped) {
            let into = a.start.rem_euclid(3.5);
            if b.vel > a.vel {
                let on_group = (into - 1.5).abs() < 0.13 || (into - 2.5).abs() < 0.13;
                assert!(on_group, "an accent at {into} of a 3+2+2 bar");
            }
        }
    }

    /// The most notes sounding together anywhere in `part`.
    #[cfg(test)]
    fn max_polyphony(part: &[generate::Note]) -> usize {
        let mut edges: Vec<(f64, i32)> = Vec::new();
        for n in part {
            edges.push((n.start, 1));
            edges.push((n.start + n.len, -1));
        }
        // Ends before starts at the same instant: a note that stops exactly
        // where the next one begins is not two notes sounding.
        edges.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let (mut now, mut most) = (0i32, 0i32);
        for (_, d) in edges {
            now += d;
            most = most.max(now);
        }
        most as usize
    }

    /// **The accompaniment does not pile up, however extended the chart is.**
    ///
    /// What this pins, measured on the four bars below: a chart of thirteenths
    /// had the guitar sounding **twenty-two notes at once** and the whole band
    /// thirty-five, which is not an accompaniment, it is a wall. Two causes, both
    /// of them here:
    ///
    /// 1. a voicing was the chord's whole spelling — eight notes for a `maj13`,
    ///    where a pianist plays four (`generate::hand`);
    /// 2. `Comp::hold` outlasted the gap to the next hit, so four strums rang
    ///    together (`generate::damp`).
    #[test]
    fn the_accompaniment_does_not_pile_up() {
        let _g = crate::test_locks::transport();
        let style = style::by_name("strtrock");
        let ctx = generate::Bake::at(120.0);
        for (what, text) in [
            ("triads", "key = C\n|| C | F | G | C ||"),
            ("sevenths", "key = C\n|| Cmaj7 | Fm7 | G7 | Cmaj7 ||"),
            // Every tension the dialogue can build, on every bar.
            (
                "thirteenths",
                "key = C\n|| Cmaj13 | Fm13 | G13b9#11 | Cm(maj7)11 ||",
            ),
        ] {
            let prog = chord::parse_progression(text).unwrap();
            let mut band = Vec::new();
            for (role, hand, most) in [
                (Role::Piano, 4, 8),
                (Role::Guitar, 5, 6),
                (Role::Bass, 1, 2),
                // A drummer has four limbs: kick, snare and a cymbal together
                // is one hit of a bar, not a pile.
                (Role::Drums, 4, 4),
            ] {
                let part = generate::bake(&prog, &style, role, 5, &ctx);
                assert!(!part.is_empty(), "{what} {role:?} plays nothing");
                // One hit is one hand: a chord is voiced with the notes that say
                // what it is, not with everything its symbol spells.
                let mut per_hit = std::collections::BTreeMap::<i64, usize>::new();
                for n in &part {
                    // A strum crosses the strings over a few hundredths of a
                    // beat and is still one chord.
                    *per_hit.entry((n.start * 10.0).round() as i64).or_default() += 1;
                }
                let biggest = per_hit.values().copied().max().unwrap_or(0);
                assert!(
                    biggest <= hand,
                    "{what} {role:?}: {biggest} notes in one hit, a hand is {hand}"
                );
                let poly = max_polyphony(&part);
                assert!(
                    poly <= most,
                    "{what} {role:?}: {poly} notes sounding at once (at most {most})"
                );
                band.extend(part);
            }
            // The whole band in one tab, which is what a tab plays: a SoundFont
            // has sixteen channels and a player has two ears.
            let poly = max_polyphony(&band);
            assert!(
                poly <= 20,
                "{what}: the band sounds {poly} notes at once, which is a wall"
            );
        }
    }

    /// A tab's chart is in letters unless the chart it was given says degrees:
    /// what a player reads is the default, and the switch is for whoever is
    /// thinking about the form.
    #[test]
    fn a_new_chart_is_written_in_letters() {
        let _g = crate::test_locks::transport();
        assert!(!ArrangerSettings::default().roman, "degrees by default");
        assert!(
            DEFAULT_TEXT.contains("C7") && !DEFAULT_TEXT.contains("I7"),
            "the blues everybody starts on is not in letters"
        );
        let a = Arranger::new(ArrangerSettings::default());
        assert!(a.error().is_none(), "{:?}", a.error());
        assert_eq!(a.bars(), 12);

        // A chart that arrives in degrees is read as degrees: the file is what
        // says which, not this default.
        let mut a = arranger(Role::Piano, 1);
        a.set_text("key = C\n|| I7 | IV7 ||");
        assert!(a.settings.roman, "a chart of degrees read as letters");
    }

    /// **PAUSE carries on from the bar it was in; PLAY starts from the top.**
    #[test]
    fn pause_keeps_the_bar_and_play_goes_back_to_the_top() {
        let _g = crate::test_locks::transport();
        let t = choz_ports::transport();
        t.set_playing(false);
        t.set_bpm(120.0);
        t.set_sample_rate(48_000);
        t.set_free_samples(0);
        // Four beats a bar at 120: one bar is two seconds, 96_000 frames.
        let bar = |n: u64| t.set_free_samples(n * 96_000);

        let mut a = arranger(Role::Piano, 3);
        let now = Instant::now();
        let mut out = Vec::new();
        a.play();
        a.tick(now, &mut out);
        // Three bars in, half a bar a tick: a jump bigger than a bar is a drag
        // rather than music played, and `advance` refuses it — see `JUMP_BEATS`.
        for half in 1..=6u64 {
            t.set_free_samples(half * 48_000);
            a.tick(now + Duration::from_millis(10 * half), &mut out);
        }
        let bars_in = a.bar_index().expect("nothing playing");
        assert!(bars_in >= 2, "the form did not move: bar {bars_in}");

        // Paused: nothing left sounding, and the playhead stays where it was.
        let at = a.at;
        let mut offs = Vec::new();
        a.pause(&mut offs);
        assert!(!a.is_playing(), "pause did not stop it");
        assert!(
            offs.iter().all(|e| matches!(e, ArpEvent::Off { .. })),
            "pause sounded something: {offs:?}"
        );
        assert_eq!(a.at, at, "pause moved the playhead");

        // The clock runs on while it is paused — a band does not play the bars
        // it sat out — and coming back carries on from the same bar.
        bar(7);
        a.resume();
        let mut back = Vec::new();
        a.tick(now + Duration::from_millis(200), &mut back);
        assert!(
            (a.at - at).abs() < 0.5,
            "resume jumped from {at} to {}",
            a.at
        );
        assert_eq!(
            a.bar_index(),
            Some(bars_in),
            "resume did not come back in the bar it left"
        );

        // PLAY is the other button: back to bar one.
        a.play();
        assert_eq!(a.at, 0.0, "play did not go back to the top");
        bar(8);
        a.tick(now + Duration::from_millis(300), &mut back);
        assert_eq!(a.bar_index(), Some(0), "play came in mid-form");
    }

    /// An extended chord is voiced, not spelled: the hand keeps the notes that
    /// say what the chord is and leaves the root to the bass player.
    #[test]
    fn an_extended_chord_is_voiced_by_a_hand() {
        let _g = crate::test_locks::transport();
        let prog = chord::parse_progression("key = C\n|| Cmaj13 | Cmaj13 ||").unwrap();
        let style = style::by_name("strtrock");
        let ctx = generate::Bake::at(120.0);
        let chord = chord::parse("Cmaj13", Some(0)).unwrap();
        assert!(
            chord.tones.len() > 5,
            "the test needs a chord bigger than a hand: {:?}",
            chord.tones
        );
        let part = generate::bake(&prog, &style, Role::Piano, 5, &ctx);
        // Whatever it plays is in the chord: voicing it smaller must not invent
        // a note.
        let pcs = chord.pitch_classes();
        for n in &part {
            assert!(
                pcs.contains(&(n.note % 12)),
                "{} is not in Cmaj13: {pcs:?}",
                n.note
            );
        }
        // And the third is in there: it is the note that says what the chord is.
        let third = (chord.root + 4) % 12;
        assert!(
            part.iter().any(|n| n.note % 12 == third),
            "the third was voiced away"
        );
    }

    /// The three knobs: the swing is on top of the style's, and `random` only
    /// strays as often as `prob` says.
    #[test]
    fn the_knobs_are_the_sequencers() {
        // The tempo is a global and baking reads it: without the lock another
        // test moving it mid-bake is two different parts.
        let _g = crate::test_locks::transport();
        let prog = chord::parse_progression(DEFAULT_TEXT).unwrap();
        let style = style::by_name("strtrock");
        let plain = generate::bake(&prog, &style, Role::Drums, 8, &generate::Bake::at(120.0));
        let swung = generate::bake(
            &prog,
            &style,
            Role::Drums,
            8,
            &generate::Bake {
                swing: 0.5,
                ..generate::Bake::at(120.0)
            },
        );
        assert_eq!(plain.len(), swung.len());
        assert!(
            plain
                .iter()
                .zip(&swung)
                .any(|(a, b)| b.start > a.start + 0.05),
            "the swing knob pushed nothing late"
        );

        // `random` with `prob` at zero is a knob that does nothing, which is
        // what the sequencer's pair means.
        let asleep = generate::bake(
            &prog,
            &style,
            Role::Drums,
            8,
            &generate::Bake {
                random: 1.0,
                ..generate::Bake::at(120.0)
            },
        );
        assert_eq!(plain, asleep, "RAND strayed with PROB at zero");
        let loose = generate::bake(
            &prog,
            &style,
            Role::Drums,
            8,
            &generate::Bake {
                random: 1.0,
                prob: 1.0,
                ..generate::Bake::at(120.0)
            },
        );
        assert_ne!(plain, loose, "RAND and PROB up changed nothing");
        // Never into silence, and never out of the form.
        for n in &loose {
            assert!(n.vel >= 1 && n.len > 0.0 && n.start >= 0.0, "{n:?}");
        }
    }

    /// A fast tempo gets a part a kit can actually play: the hats thin to
    /// eighths, the ghosts go, and the fills come out of the half of [`FILLS`]
    /// that has nothing faster than an eighth in it.
    #[test]
    fn a_fast_tempo_thins_the_kit() {
        // The tempo is a global and baking reads it: without the lock another
        // test moving it mid-bake is two different parts.
        let _g = crate::test_locks::transport();
        let prog = chord::parse_progression(DEFAULT_TEXT).unwrap();
        // Humanisation off: what is left is the grid, which is what thins.
        let style = style::Style {
            human: 0.0,
            ..style::by_name("easy_bld")
        };
        let gap = |part: &[generate::Note]| -> f64 {
            let mut starts: Vec<f64> = part.iter().map(|n| n.start).collect();
            starts.sort_by(f64::total_cmp);
            starts
                .windows(2)
                .map(|w| w[1] - w[0])
                .filter(|g| *g > 0.02)
                .fold(f64::MAX, f64::min)
        };
        let slow = generate::bake(&prog, &style, Role::Drums, 9, &generate::Bake::at(120.0));
        let fast = generate::bake(&prog, &style, Role::Drums, 9, &generate::Bake::at(210.0));
        let flat_out = generate::bake(&prog, &style, Role::Drums, 9, &generate::Bake::at(260.0));

        // The hi-hat counts sixteenths at 120 and cannot at 210.
        assert!(gap(&slow) < 0.26, "the slow part is not in sixteenths");
        assert!(
            gap(&fast) > 0.4,
            "210 bpm still asks for sixteenths: {}",
            gap(&fast)
        );
        // Above 240 the cymbal counts the beat and nothing else: the kick and
        // the snare keep whatever the style wrote, which is why the shortest
        // gap in the part is still the style's own and not the hand's.
        let cymbals = |part: &[generate::Note]| {
            part.iter()
                .filter(|n| n.note == gm::HAT_CLOSED || n.note == gm::RIDE)
                .count()
        };
        assert!(
            cymbals(&flat_out) * 2 <= cymbals(&fast),
            "260 bpm rides as busily as 210: {} vs {}",
            cymbals(&flat_out),
            cymbals(&fast)
        );
        assert!(
            cymbals(&fast) < cymbals(&slow),
            "210 bpm rides as busily as 120"
        );
        assert!(
            fast.len() < slow.len() && flat_out.len() < fast.len(),
            "nothing thinned out: {} / {} / {}",
            slow.len(),
            fast.len(),
            flat_out.len()
        );
        // The backbeat is not what goes: thinning a kit is not silencing it.
        for part in [&fast, &flat_out] {
            assert!(
                part.iter().any(|n| n.note == gm::SNARE),
                "the snare went with the sixteenths"
            );
            assert!(part.iter().any(|n| n.note == gm::KICK), "and the kick");
        }
    }

    /// Moving the tempo across one of those lines re-bakes the band where it
    /// stands: the form does not go back to the top because somebody turned the
    /// tempo up.
    #[test]
    fn the_tempo_rebakes_the_band_where_it_stands() {
        let _g = crate::test_locks::transport();
        let t = choz_ports::transport();
        t.set_playing(false);
        t.set_bpm(120.0);

        let mut a = arranger(Role::Drums, 4);
        let slow = a.part().len();
        assert!(!a.retune_to_tempo(), "nothing moved and it baked anyway");
        t.set_bpm(125.0);
        assert!(!a.retune_to_tempo(), "the same band, baked again");

        a.play();
        a.at = 17.0;
        t.set_bpm(230.0);
        assert!(a.retune_to_tempo(), "the tempo crossed and it did not bake");
        assert!(a.part().len() < slow, "the part did not thin out");
        assert!((a.at - 17.0).abs() < 1e-9, "the form went back to the top");
        assert!(
            a.cursor <= a.part().len(),
            "the cursor is past the end of the part it is walking"
        );
        t.set_bpm(120.0);
    }

    /// The transport starts the band, and does not let it count a clock of its
    /// own in the gap before the first block — which is where a sequencer used
    /// to play its downbeat twice. MIDI clock's START arrives the same way,
    /// because it arrives as the transport.
    #[test]
    fn the_transport_starts_the_band() {
        let _g = crate::test_locks::transport();
        let t = choz_ports::transport();
        t.set_playing(false);
        t.set_bpm(120.0);
        t.set_position_beats(0.0);

        let mut a = arranger(Role::Drums, 42);
        a.play_on_transport();
        assert!(a.is_playing(), "started, waiting for the transport");
        let now = Instant::now();
        let mut out = Vec::new();
        for ms in [0, 200, 400] {
            a.tick(now + Duration::from_millis(ms), &mut out);
        }
        assert!(
            out.is_empty(),
            "it counted its own clock before the transport rolled"
        );

        t.set_playing(true);
        for beat in 0..=4 {
            t.set_position_beats(beat as f64);
            a.tick(now + Duration::from_millis(500 + beat * 10), &mut out);
        }
        assert!(!out.is_empty(), "the transport rolled and nothing played");
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
        assert!(out.iter().any(|(_, e)| matches!(e, ArpEvent::On { .. })));
        t.set_playing(false);
    }

    /// Every style plays, and plays inside what it says about itself. A style
    /// is data, so the only thing that can be wrong with a new one is a number,
    /// and this is the check that reads them all.
    #[test]
    fn every_style_plays() {
        // The tempo is a global and baking reads it: without the lock another
        // test moving it mid-bake is two different parts.
        let _g = crate::test_locks::transport();
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
                    assert!(
                        n.start + n.len <= a.beats() + 1e-9,
                        "{what}: hangs past the end"
                    );
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
        // The tempo is a global and baking reads it: without the lock another
        // test moving it mid-bake is two different parts.
        let _g = crate::test_locks::transport();
        let prog = chord::parse_progression(DEFAULT_TEXT).unwrap();
        let mut dead = style::by_name("strtrock");
        dead.human = 0.0;
        for role in Role::ALL {
            // The guitar is off the grid on purpose: the strum is the hand
            // crossing the strings, not the drummer being human.
            if role == Role::Guitar {
                continue;
            }
            for n in generate::bake(&prog, &dead, role, 3, &generate::Bake::at(120.0)) {
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
            let loose = generate::bake(
                &prog,
                &style::by_name("strtrock"),
                role,
                3,
                &generate::Bake::at(120.0),
            );
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
        // The tempo is a global and baking reads it: without the lock another
        // test moving it mid-bake is two different parts.
        let _g = crate::test_locks::transport();
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
        assert!(
            up > 0 && down > 0,
            "the hand only goes one way: up {up}, down {down}"
        );
    }

    /// The feel lives in whatever division the style counts in: a blues swings
    /// its eighths, a funk its sixteenths. One number, two feels — the failure
    /// this catches is a funk played dead straight because `swung()` only knew
    /// about eighths.
    #[test]
    fn the_swing_is_in_the_styles_own_division() {
        // The tempo is a global and baking reads it: without the lock another
        // test moving it mid-bake is two different parts.
        let _g = crate::test_locks::transport();
        let prog = chord::parse_progression(DEFAULT_TEXT).unwrap();
        // With the humanisation off, what is left off the grid is the swing.
        let off_by = |st: &style::Style| -> Vec<f64> {
            let mut st = *st;
            st.human = 0.0;
            let mut out = Vec::new();
            for role in [Role::Drums, Role::Bass] {
                for n in generate::bake(&prog, &st, role, 5, &generate::Bake::at(120.0)) {
                    let sixteenth = (n.start / 0.25).round() * 0.25;
                    let off = (n.start - sixteenth).abs();
                    if off > 1e-9 {
                        out.push(off);
                    }
                }
            }
            out
        };
        // What a style pushes its off-divisions by, as a distance from the
        // sixteenth grid: its own division times its own swing.
        let want = |st: &style::Style| -> f64 {
            let late = st.swing_div * st.swing as f64;
            (late - (late / 0.25).round() * 0.25).abs()
        };
        // A style that swings the sixteenths is off the grid by a fraction of
        // one, and a style that swings the eighths by a fraction of an eighth.
        // Neither is off it by the other's amount, which is the whole point.
        for name in ["kwaito", "shfblues"] {
            let st = style::by_name(name);
            assert!(st.swing > 0.0, "{name} came out dead straight");
            let off = off_by(&st);
            assert!(!off.is_empty(), "{name} played nothing off the grid");
            assert!(
                off.iter().all(|o| (o - want(&st)).abs() < 1e-6),
                "{name} does not swing its own division: {off:?}"
            );
        }
        assert!(
            (want(&style::by_name("kwaito")) - want(&style::by_name("shfblues"))).abs() > 1e-6,
            "the two divisions were not told apart"
        );
    }

    /// The bar that hands the form back to the top is not the bar that hands
    /// over bar four: the turnaround gets two beats of fill where the others
    /// get one.
    #[test]
    fn the_turnaround_is_a_longer_fill() {
        // The tempo is a global and baking reads it: without the lock another
        // test moving it mid-bake is two different parts.
        let _g = crate::test_locks::transport();
        let mut drums = arranger(Role::Drums, 8);
        // A style with a whole kit in it, so a fill has toms to come down.
        drums.set_text(DEFAULT_TEXT.replace("shfblues", "strtrock"));
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
        // The tempo is a global and baking reads it: without the lock another
        // test moving it mid-bake is two different parts.
        let _g = crate::test_locks::transport();
        // A straight style, so an anticipation is half a beat early and not
        // half a beat plus the shuffle.
        let mut a = arranger(Role::Bass, 19);
        a.set_text("key = C\nstyle = pnomrch1\n|| I7 | IV7 | I7 | V7 | IV7 | I7 | V7 | I7 ||");
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
        let jazz = style::by_name("jzcombo2");
        assert!(jazz.programs(Role::Drums).is_empty(), "drums are a bank");
        // The rhythm's own bass patch is what it asks for first.
        assert_eq!(jazz.programs(Role::Bass).first(), Some(&jazz.bass_gm[0]));

        // A SoundFont without it: the next one down the list plays it.
        let wanted = jazz.programs(Role::Bass);
        let second = wanted[1];
        let bank: Vec<(u8, u8)> = vec![(0, 0), (0, second), (128, 0)];
        let at = pick_program(&bank, wanted).unwrap();
        assert_eq!(bank[at], (0, second));
        // A drum-only bank answers nothing rather than a kit.
        assert_eq!(pick_program(&[(128, 0)], wanted), None);
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

    /// The box says the bar the band is counting, and a waltz is not a 4/4.
    ///
    /// It used to print the transport's signature, which is 4/4 in a session
    /// nobody has changed — so every three-four style in the list said four.
    #[test]
    fn the_box_counts_the_bar_the_band_is_playing() {
        let _g = crate::test_locks::transport();
        choz_ports::transport().set_time_signature(4, 4);
        let waltz = |name: &str| {
            let mut a = arranger(Role::Piano, 3);
            a.set_text(format!("key = C\nstyle = {name}\n|| I | IV | V | I ||"));
            a
        };
        // A 6/8 and a 3/4 are both three quarter notes: the signature is the
        // only thing that tells them apart, and the box prints the signature.
        assert_eq!(waltz("cntywltz").meter(), (3, 4));
        assert_eq!(waltz("6_8blues").meter(), (6, 8));
        assert_eq!(waltz("strtrock").meter(), (4, 4));

        // A session that counts something else still wins: that is what the
        // METER button on the box sets, and a band plays the session's bar.
        choz_ports::transport().set_time_signature(7, 8);
        assert_eq!(waltz("cntywltz").meter(), (7, 8));
        choz_ports::transport().set_time_signature(4, 4);
    }

    /// The band mixes itself: the bass is the part you have to hear, and a
    /// chord is not five times a note.
    #[test]
    fn the_band_mixes_itself() {
        let _g = crate::test_locks::transport();
        let prog = chord::parse_progression(DEFAULT_TEXT).unwrap();
        let style = style::by_name("strtrock");
        let loudest = |role: Role| -> u8 {
            generate::bake(&prog, &style, role, 5, &generate::Bake::at(120.0))
                .iter()
                .map(|n| n.vel)
                .max()
                .unwrap_or(0)
        };
        assert!(
            loudest(Role::Bass) > loudest(Role::Piano),
            "the comp is over the bass: bass {}, piano {}",
            loudest(Role::Bass),
            loudest(Role::Piano)
        );
        assert!(
            loudest(Role::Bass) > loudest(Role::Guitar),
            "the guitar is over the bass"
        );

        // Notes struck together share the room they are given, so a voicing is
        // not the sum of its notes at full velocity.
        let piano = generate::bake(&prog, &style, Role::Piano, 5, &generate::Bake::at(120.0));
        let chord = piano
            .windows(2)
            .find(|w| (w[1].start - w[0].start).abs() < 1e-6)
            .expect("the piano plays no chords");
        let alone = piano
            .iter()
            .find(|n| {
                piano
                    .iter()
                    .filter(|m| (m.start - n.start).abs() < 1e-6)
                    .count()
                    == 1
            })
            .map(|n| n.vel);
        if let Some(alone) = alone {
            assert!(
                chord[0].vel < alone,
                "a note in a chord is as loud as a note on its own: {} vs {alone}",
                chord[0].vel
            );
        }
    }
}
