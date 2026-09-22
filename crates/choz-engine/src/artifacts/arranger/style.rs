//! Styles, as data.
//!
//! A style is a set of numbers — feel, swing, and one knob-set per role — and
//! **not** a branch inside a generator: adding a bossa must not mean touching
//! the bass line's code.
//!
//! # Where the numbers come from
//!
//! Every one of them is **measured**, off the rhythms in `arranger/rhythms` —
//! one MIDI file per accompaniment, named after the style it becomes, its
//! sections laid end to end and each track named with the part that plays it.
//! `tools/mid_to_styles.py` is what measures them and [`super::styles`] is
//! what it writes, one [`Style`] per rhythm: the
//! variation section is folded a bar at a time, a position is in the pattern
//! when it comes back in more than a third of the bars, the backbeat is the
//! loud snares and `ghost` is the rate of the quiet ones, and swing is how
//! late the off-divisions sit — measured before any rounding, because rounding
//! a shuffle to the sixteenth grid is what turns a shuffle into a sixteenth.
//!
//! Nothing is hand-written here any more, and there is no style file format:
//! the library is the source, the script is the reader, and a style that is
//! wrong is a measurement to fix rather than a constant to tune. choz itself
//! reads no MIDI to do it: the folder is the source, the table is what ships,
//! and `arranger/rhythms/README.md` says what a file has to look like to be
//! measured.

/// How a style wants its bass played.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bass {
    /// One note per beat, walking, against notes held on the chord.
    pub walking: bool,
    /// How much of the bar speaks, 0..1. Under 1 the off-beats drop first.
    pub density: f32,
    /// How likely the last beat before a chord change is a chromatic approach
    /// rather than a chord tone.
    pub approach: f32,
    /// How likely a beat that is not going anywhere in particular is a step
    /// through the scale rather than another chord tone. What turns a line that
    /// outlines the chord into one that goes somewhere.
    pub passing: f32,
    /// How likely a chord is arrived at an eighth early — the anticipation, and
    /// the pickup into the first bar.
    pub pickup: f32,
    /// How likely a note jumps an octave rather than staying in the register.
    pub octave_jump: f32,
    /// The register, as MIDI notes. A bass line outside it is not a bass line.
    pub low: u8,
    pub high: u8,
}

/// How a style wants its drums played. Beats are positions in the bar, `0.0`
/// being the downbeat.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Drums {
    pub kick: &'static [f64],
    pub snare: &'static [f64],
    /// The **second bar** of a groove that is two bars long — the two-step's
    /// answer — played on every other bar. Empty is a one-bar groove, which is
    /// every rhythm that does not ask for two (a drum track named `… Drum
    /// 2bar`, see `rhythms/README.md`).
    pub kick_b: &'static [f64],
    pub snare_b: &'static [f64],
    /// How often the cymbal speaks, in beats. `0.5` is eighths.
    pub cymbal: f64,
    /// Ride rather than hi-hat, and the swung "ding-ding-a-ding" with it.
    pub ride: bool,
    /// How likely a snare ghost lands on an off-beat, 0..1.
    pub ghost: f32,
    /// A fill on the last bar of every `fill_every` bars. `0` is never.
    pub fill_every: usize,
}

/// How a style plays a bar of **another** signature than its own — what a
/// heterometric rhythm measures for each meter it changes to (a `Meter`
/// section of the rhythm, see `rhythms/README.md`). A meter with none of these
/// gets the style's own bar cut to length, or carried on into the extra beats.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeterGroove {
    pub meter: (u8, u8),
    pub kick: &'static [f64],
    pub snare: &'static [f64],
    /// Where the comp hits in that bar.
    pub hits: &'static [f64],
}

/// How a style wants its chords comped. `hits` are positions in the bar.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Comp {
    pub hits: &'static [f64],
    /// How likely a hit is played at all — what keeps a comp from being a
    /// machine playing the same bar twelve times.
    pub density: f32,
    /// How long a hit is held, in beats.
    pub hold: f64,
    /// The register the voicings are kept in.
    pub low: u8,
    pub high: u8,
}

/// A style: the whole of what separates one accompaniment from another.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    /// What a chart writes, and what a project stores.
    pub name: &'static str,
    /// What a list shows. A rhythm is named the way an instrument's front panel
    /// had room for it — `16btshfl` — and two hundred of those is not something
    /// anybody can choose between, so the picker reads this instead.
    pub label: &'static str,
    /// Beats in a bar. Four until a style asks for something else — the
    /// transport's own signature is what the *host* counts in, and a style
    /// written in 4 stays in 4 inside a 6/8 session.
    pub beats_per_bar: f64,
    /// The signature the rhythm was written in, `(4, 4)`. What
    /// [`Self::beats_per_bar`] cannot say: a 6/8 and a 3/4 are both three
    /// quarter notes, and the panel has to be able to tell them apart.
    pub meter: (u8, u8),
    /// How far the off-divisions are pushed late, as a share of one. The
    /// sequencer's scale and the arpeggiator's, because there is one swing in
    /// this program.
    pub swing: f32,
    /// What gets swung, in beats: `0.5` is the eighths a blues shuffles, `0.25`
    /// the sixteenths a funk or a drum'n'bass does. A feel lives in whichever
    /// division the style counts in, and that is this number.
    pub swing_div: f64,
    /// How hard the roles hit by default, before humanisation.
    pub vel: u8,
    /// How loose the band plays, as a scale on the timing and velocity jitter.
    /// `1.0` is a blues band; a style that wants to sound programmed asks for
    /// less. Each role scales it again — the drummer is the clock and the
    /// soloist leans on it.
    pub human: f32,
    /// How far apart the strings of a strummed chord are struck, in beats. At
    /// 120 bpm `0.03` is 15 ms a string, which is a strum rather than an
    /// arpeggio; a country band strums wider and a funk guitar barely at all.
    pub strum: f64,
    pub bass: Bass,
    pub drums: Drums,
    pub comp: Comp,
    /// The guitar's own comp. `None` derives it from `comp`: the style's rhythm
    /// played on a guitar's strings.
    pub guitar: Option<Comp>,
    /// The GM programs each melodic role is played on, best first — the patch
    /// the rhythm itself asked for, then what any SoundFont is sure to have.
    /// See [`Style::programs`].
    pub bass_gm: &'static [u8],
    pub piano_gm: &'static [u8],
    pub guitar_gm: &'static [u8],
    /// The bars it knows in other signatures, for a chart that changes meter.
    /// Empty for a style measured in one.
    pub meters: &'static [MeterGroove],
}

pub use super::styles::ALL;

/// A pattern written for a bar of `own` beats, laid over a bar of `len`: cut
/// where the bar ends, and carried on from the top into the beats a longer bar
/// has — a 4/4 groove in a 5/4 bar plays its first beat again.
pub fn fit(pattern: &[f64], own: f64, len: f64) -> Vec<f64> {
    if own <= 0.0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut from = 0.0;
    while from < len - 1e-9 {
        out.extend(pattern.iter().map(|p| from + p).filter(|p| *p < len - 1e-9));
        from += own;
    }
    out
}

impl Style {
    /// The groove this style plays in a bar of `meter`, when it measured one.
    pub fn groove_for(&self, meter: Option<(u16, u16)>) -> Option<&MeterGroove> {
        let (num, den) = meter?;
        self.meters
            .iter()
            .find(|g| (g.meter.0 as u16, g.meter.1 as u16) == (num, den))
    }

    /// The GM programs a role is played on, best first.
    ///
    /// A list rather than a number because a SoundFont is not obliged to have
    /// any particular one of them: the caller walks it until the bank answers,
    /// and the last entry is the acoustic piano every SoundFont has. See
    /// [`super::pick_program`].
    pub fn programs(&self, role: super::generate::Role) -> &'static [u8] {
        use super::generate::Role;
        match role {
            // Drums are not a program: the kit is the bank, and every caller
            // that asks gets nothing rather than a tuned instrument.
            Role::Drums => &[],
            Role::Bass => self.bass_gm,
            Role::Piano => self.piano_gm,
            Role::Guitar => self.guitar_gm,
        }
    }
}

/// Every style there is, in the order a list would show them.
///
/// A `Vec` for the callers that had one while styles could also be read off
/// disk. They cannot any more — the archive is the only source — so this is
/// [`ALL`] copied, and a caller that only reads should take [`ALL`] itself.
pub fn all() -> Vec<Style> {
    ALL.to_vec()
}

/// A style by the name a progression writes. Unknown names fall back to the
/// first: a misspelt style plays the wrong feel, which is a thing you can hear
/// and fix, rather than nothing at all.
pub fn by_name(name: &str) -> Style {
    let name = name.trim().to_ascii_lowercase();
    ALL.iter()
        .find(|s| s.name == name)
        .copied()
        .unwrap_or(ALL[0])
}
