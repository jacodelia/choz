//! Styles, as data.
//!
//! A style is a set of numbers — feel, swing, and one knob-set per role — and
//! **not** a branch inside a generator: adding `bossa` must not mean touching
//! the bass line's code. The structs are shaped so they can be read from a file
//! the day there is a file; there is no loader yet and inventing one before a
//! second source of styles exists would be a format nobody writes.

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
    /// How often the cymbal speaks, in beats. `0.5` is eighths.
    pub cymbal: f64,
    /// Ride rather than hi-hat, and the swung "ding-ding-a-ding" with it.
    pub ride: bool,
    /// How likely a snare ghost lands on an off-beat, 0..1.
    pub ghost: f32,
    /// A fill on the last bar of every `fill_every` bars. `0` is never.
    pub fill_every: usize,
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

/// How a style wants its single line played — the melody, and the solo that is
/// the same shape played busier. `notes` is the menu of durations a motif is
/// built from, in beats.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lead {
    pub notes: &'static [f64],
    /// How likely a slot of the motif is silence rather than a note. A melody
    /// with no rests in it is an exercise.
    pub rest: f32,
    /// How likely the note before a chord change leans into the next chord a
    /// semitone away.
    pub chromatic: f32,
    /// How far the answering phrase (`A'`) is pushed late, in beats.
    pub displace: f64,
    /// The register the line is kept in.
    pub low: u8,
    pub high: u8,
}

/// A style: the whole of what separates one accompaniment from another.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    pub name: &'static str,
    /// Beats in a bar. Four until a style asks for something else — the
    /// transport's own signature is what the *host* counts in, and a style
    /// written in 4 stays in 4 inside a 6/8 session.
    pub beats_per_bar: f64,
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
    pub lead: Lead,
    /// The solo's own numbers. `None` derives them from `lead` — the melody
    /// played busier, which is what a solo is until a style says otherwise.
    pub solo: Option<Lead>,
    /// The guitar's own comp. `None` derives it from `comp`: the style's rhythm
    /// played on a guitar's strings.
    pub guitar: Option<Comp>,
}

/// The shuffle every blues in this file is played with.
const SWING: f32 = 0.6;

pub const MAJOR_BLUES: Style = Style {
    name: "major_blues",
    beats_per_bar: 4.0,
    swing: SWING,
    swing_div: 0.5,
    vel: 96,
    human: 1.0,
    strum: 0.03,
    bass: Bass {
        walking: true,
        density: 1.0,
        approach: 0.7,
        passing: 0.35,
        pickup: 0.25,
        octave_jump: 0.1,
        low: 36,
        high: 55,
    },
    drums: Drums {
        kick: &[0.0, 2.5],
        snare: &[1.0, 3.0],
        cymbal: 0.5,
        ride: true,
        ghost: 0.25,
        fill_every: 4,
    },
    comp: Comp {
        hits: &[0.0, 1.5, 2.0, 3.5],
        density: 0.7,
        hold: 0.75,
        low: 52,
        high: 76,
    },
    lead: Lead {
        notes: &[1.0, 0.5, 0.5, 1.5, 2.0],
        rest: 0.25,
        chromatic: 0.3,
        displace: 0.5,
        low: 60,
        high: 84,
    },
    solo: None,
    guitar: None,
};

pub const MINOR_BLUES: Style = Style {
    name: "minor_blues",
    beats_per_bar: 4.0,
    swing: SWING,
    swing_div: 0.5,
    // Slower and heavier: fewer notes from everybody, and the kick on the
    // beats rather than pushed.
    vel: 90,
    human: 1.0,
    strum: 0.03,
    bass: Bass {
        walking: true,
        density: 1.0,
        approach: 0.5,
        passing: 0.35,
        pickup: 0.25,
        octave_jump: 0.05,
        low: 36,
        high: 55,
    },
    drums: Drums {
        kick: &[0.0, 2.0],
        snare: &[1.0, 3.0],
        cymbal: 0.5,
        ride: true,
        ghost: 0.15,
        fill_every: 4,
    },
    comp: Comp {
        hits: &[0.0, 2.5],
        density: 0.6,
        hold: 1.5,
        low: 52,
        high: 76,
    },
    lead: Lead {
        // Slower and with more air in it: longer notes and more rests.
        notes: &[2.0, 1.0, 1.0, 0.5, 1.5],
        rest: 0.35,
        chromatic: 0.15,
        displace: 0.5,
        low: 58,
        high: 82,
    },
    solo: None,
    guitar: None,
};

/// Four to the bar, swung, and the ride keeping it: the feel every standard is
/// played with. The comp is off the beat on purpose — the piano answers the
/// bass, it does not double it.
pub const JAZZ_SWING: Style = Style {
    name: "jazz_swing",
    beats_per_bar: 4.0,
    swing: SWING,
    swing_div: 0.5,
    vel: 88,
    human: 1.2,
    strum: 0.03,
    bass: Bass {
        walking: true,
        density: 1.0,
        approach: 0.8,
        passing: 0.35,
        pickup: 0.25,
        octave_jump: 0.05,
        low: 36,
        high: 55,
    },
    drums: Drums {
        kick: &[0.0],
        snare: &[3.0],
        cymbal: 0.5,
        ride: true,
        ghost: 0.35,
        fill_every: 8,
    },
    comp: Comp {
        hits: &[0.5, 1.5, 2.5, 3.5],
        density: 0.5,
        hold: 0.5,
        low: 52,
        high: 79,
    },
    lead: Lead {
        notes: &[0.5, 0.5, 1.0, 1.5, 0.5],
        rest: 0.3,
        chromatic: 0.45,
        displace: 0.5,
        low: 60,
        high: 86,
    },
    solo: None,
    guitar: None,
};

/// The same blues with the hat instead of the ride and the kick on every other
/// beat: a shuffle is what a blues band plays when nobody is soloing.
pub const SHUFFLE: Style = Style {
    name: "shuffle",
    beats_per_bar: 4.0,
    swing: SWING,
    swing_div: 0.5,
    vel: 100,
    human: 0.9,
    strum: 0.03,
    bass: Bass {
        walking: false,
        density: 0.8,
        approach: 0.5,
        passing: 0.35,
        pickup: 0.25,
        octave_jump: 0.3,
        low: 36,
        high: 55,
    },
    drums: Drums {
        kick: &[0.0, 2.0],
        snare: &[1.0, 3.0],
        cymbal: 0.5,
        ride: false,
        ghost: 0.2,
        fill_every: 4,
    },
    comp: Comp {
        hits: &[0.0, 1.0, 2.0, 3.0],
        density: 0.8,
        hold: 0.9,
        low: 52,
        high: 76,
    },
    lead: Lead {
        notes: &[0.5, 1.0, 0.5, 1.5, 1.0],
        rest: 0.25,
        chromatic: 0.3,
        displace: 0.5,
        low: 60,
        high: 84,
    },
    solo: None,
    guitar: None,
};

/// The blues with the shuffle taken out: eighths land where they are written.
/// Everything else is `major_blues`, which is the point — the feel is one
/// number.
pub const STRAIGHT_BLUES: Style = Style {
    name: "straight_blues",
    beats_per_bar: 4.0,
    swing: 0.0,
    swing_div: 0.5,
    vel: 98,
    human: 0.9,
    strum: 0.03,
    bass: Bass {
        walking: true,
        density: 1.0,
        approach: 0.6,
        passing: 0.35,
        pickup: 0.25,
        octave_jump: 0.1,
        low: 36,
        high: 55,
    },
    drums: Drums {
        kick: &[0.0, 2.5],
        snare: &[1.0, 3.0],
        cymbal: 0.5,
        ride: false,
        ghost: 0.2,
        fill_every: 4,
    },
    comp: Comp {
        hits: &[0.0, 1.5, 2.0, 3.5],
        density: 0.7,
        hold: 0.75,
        low: 52,
        high: 76,
    },
    lead: Lead {
        notes: &[1.0, 0.5, 0.5, 1.5, 2.0],
        rest: 0.25,
        chromatic: 0.25,
        displace: 0.5,
        low: 60,
        high: 84,
    },
    solo: None,
    guitar: None,
};

/// Straight eighths, the kick pushed into three, and a bass that holds the
/// chord down instead of walking out of it.
pub const ROCK: Style = Style {
    name: "rock",
    beats_per_bar: 4.0,
    swing: 0.0,
    swing_div: 0.5,
    vel: 104,
    human: 0.7,
    strum: 0.03,
    bass: Bass {
        walking: false,
        density: 0.85,
        approach: 0.25,
        passing: 0.35,
        pickup: 0.25,
        octave_jump: 0.1,
        low: 33,
        high: 52,
    },
    drums: Drums {
        kick: &[0.0, 2.0, 2.5],
        snare: &[1.0, 3.0],
        cymbal: 0.5,
        ride: false,
        ghost: 0.1,
        fill_every: 4,
    },
    comp: Comp {
        hits: &[0.0, 2.0],
        density: 0.9,
        hold: 1.8,
        low: 48,
        high: 72,
    },
    lead: Lead {
        notes: &[1.0, 1.0, 2.0, 0.5, 1.5],
        rest: 0.3,
        chromatic: 0.1,
        displace: 0.5,
        low: 60,
        high: 84,
    },
    solo: None,
    guitar: None,
};

/// Sixteenths on the hat, the one that everything answers to, and short stabs
/// off the beat. The swing stays at zero: a funk feel lives in the sixteenths,
/// which the generator does not divide yet.
pub const FUNK: Style = Style {
    name: "funk",
    beats_per_bar: 4.0,
    // The sixteenths, pushed a hair: the whole of what separates a funk from a
    // drum machine playing the same table.
    swing: 0.25,
    swing_div: 0.25,
    vel: 100,
    human: 0.5,
    // Barely a strum: a funk chord is struck, not swept.
    strum: 0.012,
    bass: Bass {
        walking: false,
        density: 0.9,
        approach: 0.35,
        passing: 0.35,
        pickup: 0.25,
        octave_jump: 0.25,
        low: 33,
        high: 52,
    },
    drums: Drums {
        kick: &[0.0, 0.75, 2.5],
        snare: &[1.0, 3.0],
        cymbal: 0.25,
        ride: false,
        ghost: 0.5,
        fill_every: 4,
    },
    comp: Comp {
        hits: &[0.0, 0.75, 1.5, 2.25, 3.0],
        density: 0.6,
        hold: 0.25,
        low: 55,
        high: 79,
    },
    lead: Lead {
        notes: &[0.5, 0.5, 0.5, 1.0, 1.5],
        rest: 0.35,
        chromatic: 0.2,
        displace: 0.5,
        low: 62,
        high: 86,
    },
    solo: None,
    guitar: None,
};

/// Quiet, even, and syncopated: the kick and the comp fall between the beats
/// and nothing is loud.
pub const BOSSA: Style = Style {
    name: "bossa",
    beats_per_bar: 4.0,
    swing: 0.0,
    swing_div: 0.5,
    vel: 80,
    human: 0.8,
    strum: 0.03,
    bass: Bass {
        walking: false,
        density: 0.5,
        approach: 0.2,
        passing: 0.35,
        pickup: 0.25,
        octave_jump: 0.05,
        low: 36,
        high: 52,
    },
    drums: Drums {
        kick: &[0.0, 1.5, 2.0, 3.5],
        snare: &[1.0, 2.5],
        cymbal: 0.5,
        ride: false,
        ghost: 0.15,
        fill_every: 8,
    },
    comp: Comp {
        hits: &[0.0, 1.5, 2.5],
        density: 0.8,
        hold: 1.0,
        low: 55,
        high: 79,
    },
    lead: Lead {
        notes: &[1.0, 1.5, 0.5, 2.0, 1.0],
        rest: 0.4,
        chromatic: 0.2,
        displace: 0.5,
        low: 62,
        high: 84,
    },
    solo: None,
    guitar: None,
};

/// Three beats to the bar, swung: the one style here that is not in four, and
/// the proof that `beats_per_bar` is a number and not a comment.
pub const JAZZ_WALTZ: Style = Style {
    name: "jazz_waltz",
    beats_per_bar: 3.0,
    swing: SWING,
    swing_div: 0.5,
    vel: 86,
    human: 1.2,
    strum: 0.03,
    bass: Bass {
        walking: true,
        density: 1.0,
        approach: 0.6,
        passing: 0.35,
        pickup: 0.25,
        octave_jump: 0.05,
        low: 36,
        high: 55,
    },
    drums: Drums {
        kick: &[0.0],
        snare: &[2.0],
        cymbal: 0.5,
        ride: true,
        ghost: 0.3,
        fill_every: 8,
    },
    comp: Comp {
        hits: &[0.0, 1.5],
        density: 0.7,
        hold: 1.0,
        low: 52,
        high: 79,
    },
    lead: Lead {
        notes: &[1.0, 0.5, 1.5, 0.5, 2.0],
        rest: 0.3,
        chromatic: 0.35,
        displace: 0.5,
        low: 60,
        high: 84,
    },
    solo: None,
    guitar: None,
};

/// Clave on the comp, the bass off the downbeat and the kit playing the
/// pattern rather than the backbeat: a son montuno rather than a rock bar with
/// congas on it.
pub const LATIN: Style = Style {
    name: "latin",
    beats_per_bar: 4.0,
    swing: 0.0,
    swing_div: 0.5,
    vel: 94,
    human: 0.9,
    strum: 0.03,
    bass: Bass {
        walking: false,
        density: 0.7,
        approach: 0.2,
        passing: 0.35,
        pickup: 0.25,
        octave_jump: 0.15,
        low: 36,
        high: 52,
    },
    drums: Drums {
        kick: &[0.00, 1.50, 2.50],
        snare: &[1.00, 2.50, 3.50],
        cymbal: 0.5,
        ride: false,
        ghost: 0.3,
        fill_every: 4,
    },
    comp: Comp {
        hits: &[0.00, 0.75, 1.50, 2.50, 3.50],
        density: 0.75,
        hold: 0.5,
        low: 55,
        high: 79,
    },
    lead: Lead {
        notes: &[0.50, 0.50, 1.00, 1.00, 1.50],
        rest: 0.3,
        chromatic: 0.15,
        displace: 0.5,
        low: 62,
        high: 86,
    },
    solo: None,
    guitar: None,
};

/// Two beats to the bar: the bass alternates root and fifth, the comp
/// answers it on the off-beats, and nothing gets in the way of the words.
pub const COUNTRY: Style = Style {
    name: "country",
    beats_per_bar: 4.0,
    swing: 0.0,
    swing_div: 0.5,
    vel: 98,
    human: 0.8,
    // A flat-top strummed with a pick, across six strings and in no hurry.
    strum: 0.05,
    bass: Bass {
        walking: false,
        density: 1.0,
        approach: 0.3,
        passing: 0.35,
        pickup: 0.25,
        octave_jump: 0.0,
        low: 36,
        high: 52,
    },
    drums: Drums {
        kick: &[0.00, 2.00],
        snare: &[1.00, 3.00],
        cymbal: 0.5,
        ride: false,
        ghost: 0.1,
        fill_every: 4,
    },
    comp: Comp {
        hits: &[1.00, 3.00],
        density: 0.9,
        hold: 0.9,
        low: 52,
        high: 76,
    },
    lead: Lead {
        notes: &[1.00, 1.00, 2.00, 0.50, 1.50],
        rest: 0.25,
        chromatic: 0.1,
        displace: 0.5,
        low: 60,
        high: 82,
    },
    solo: None,
    guitar: None,
};

/// Sixteenths under a heavy backbeat, a bass that answers the kick and a
/// comp of short chords in the gaps.
pub const SOUL: Style = Style {
    name: "soul",
    beats_per_bar: 4.0,
    swing: 0.2,
    swing_div: 0.25,
    vel: 98,
    human: 1.1,
    strum: 0.03,
    bass: Bass {
        walking: false,
        density: 0.9,
        approach: 0.35,
        passing: 0.35,
        pickup: 0.25,
        octave_jump: 0.2,
        low: 33,
        high: 52,
    },
    drums: Drums {
        kick: &[0.00, 1.75, 2.50],
        snare: &[1.00, 3.00],
        cymbal: 0.25,
        ride: false,
        ghost: 0.4,
        fill_every: 4,
    },
    comp: Comp {
        hits: &[0.50, 1.50, 2.75, 3.50],
        density: 0.7,
        hold: 0.4,
        low: 55,
        high: 79,
    },
    lead: Lead {
        notes: &[0.50, 0.75, 1.00, 0.50, 1.50],
        rest: 0.35,
        chromatic: 0.25,
        displace: 0.5,
        low: 62,
        high: 86,
    },
    solo: None,
    guitar: None,
};

/// The one dropped: no kick on the downbeat, the chop on the off-beats and
/// a bass that plays the low notes nobody else is playing.
pub const REGGAE: Style = Style {
    name: "reggae",
    beats_per_bar: 4.0,
    swing: 0.0,
    swing_div: 0.5,
    vel: 88,
    human: 0.9,
    strum: 0.03,
    bass: Bass {
        walking: false,
        density: 0.6,
        approach: 0.2,
        passing: 0.35,
        pickup: 0.25,
        octave_jump: 0.05,
        low: 33,
        high: 48,
    },
    drums: Drums {
        kick: &[2.00],
        snare: &[2.00],
        cymbal: 0.5,
        ride: false,
        ghost: 0.15,
        fill_every: 8,
    },
    comp: Comp {
        hits: &[0.50, 1.50, 2.50, 3.50],
        density: 0.95,
        hold: 0.3,
        low: 55,
        high: 76,
    },
    lead: Lead {
        notes: &[1.00, 1.50, 0.50, 2.00, 1.00],
        rest: 0.4,
        chromatic: 0.15,
        displace: 0.5,
        low: 60,
        high: 82,
    },
    solo: None,
    // The chop: every off-beat, damped the instant it sounds, and high up the
    // neck where it cannot be mistaken for the bass.
    guitar: Some(Comp {
        hits: &[0.5, 1.5, 2.5, 3.5],
        density: 1.0,
        hold: 0.15,
        low: 55,
        high: 72,
    }),
};

/// Fast, swung and busy: the ride carries it, the comp punctuates and the
/// line is eighths with the chromatics a bebop head is made of.
pub const BEBOP: Style = Style {
    name: "bebop",
    beats_per_bar: 4.0,
    swing: 0.55,
    swing_div: 0.5,
    vel: 86,
    human: 1.3,
    strum: 0.03,
    bass: Bass {
        walking: true,
        density: 1.0,
        approach: 0.85,
        passing: 0.35,
        pickup: 0.25,
        octave_jump: 0.05,
        low: 36,
        high: 57,
    },
    drums: Drums {
        kick: &[],
        snare: &[],
        cymbal: 0.5,
        ride: true,
        ghost: 0.45,
        fill_every: 8,
    },
    comp: Comp {
        hits: &[0.50, 2.50],
        density: 0.45,
        hold: 0.5,
        low: 52,
        high: 79,
    },
    lead: Lead {
        notes: &[0.50, 0.50, 0.50, 1.00, 0.25],
        rest: 0.2,
        chromatic: 0.6,
        displace: 0.5,
        low: 60,
        high: 88,
    },
    // A bebop solo is not the head played busier — it is eighths that do not
    // stop, chromatic through every change, and it lives above the head.
    solo: Some(Lead {
        notes: &[0.5, 0.5, 0.5, 0.25, 0.5],
        rest: 0.05,
        chromatic: 0.8,
        displace: 0.5,
        low: 64,
        high: 93,
    }),
    guitar: None,
};

/// Four to the bar and little else: the bass is the piece and everybody
/// else stays out of its way.
pub const WALKING_BASS: Style = Style {
    name: "walking_bass",
    beats_per_bar: 4.0,
    swing: 0.6,
    swing_div: 0.5,
    vel: 88,
    human: 1.0,
    strum: 0.03,
    bass: Bass {
        walking: true,
        density: 1.0,
        approach: 0.9,
        passing: 0.35,
        pickup: 0.25,
        octave_jump: 0.05,
        low: 36,
        high: 57,
    },
    drums: Drums {
        kick: &[0.00],
        snare: &[3.00],
        cymbal: 0.5,
        ride: true,
        ghost: 0.2,
        fill_every: 8,
    },
    comp: Comp {
        hits: &[1.50, 3.50],
        density: 0.35,
        hold: 0.5,
        low: 52,
        high: 76,
    },
    lead: Lead {
        notes: &[1.00, 0.50, 1.50, 1.00, 2.00],
        rest: 0.4,
        chromatic: 0.35,
        displace: 0.5,
        low: 60,
        high: 84,
    },
    solo: None,
    guitar: None,
};

/// Four on the floor at walking pace, an off-beat hat and a bass that moves
/// one note at a time under a comp that barely changes.
pub const ORGANIC_HOUSE: Style = Style {
    name: "organic_house",
    beats_per_bar: 4.0,
    swing: 0.0,
    swing_div: 0.5,
    vel: 90,
    human: 0.6,
    strum: 0.03,
    bass: Bass {
        walking: false,
        density: 0.8,
        approach: 0.15,
        passing: 0.35,
        pickup: 0.25,
        octave_jump: 0.3,
        low: 33,
        high: 48,
    },
    drums: Drums {
        kick: &[0.00, 1.00, 2.00, 3.00],
        snare: &[1.00, 3.00],
        cymbal: 0.5,
        ride: false,
        ghost: 0.1,
        fill_every: 8,
    },
    comp: Comp {
        hits: &[0.00, 2.50],
        density: 0.6,
        hold: 1.5,
        low: 55,
        high: 79,
    },
    lead: Lead {
        notes: &[2.00, 1.00, 1.50, 0.50, 2.00],
        rest: 0.45,
        chromatic: 0.1,
        displace: 0.5,
        low: 62,
        high: 84,
    },
    solo: None,
    guitar: None,
};

/// The same floor with the percussion in triplet-feel sixteenths and a bass
/// that plays the gaps rather than the beats.
pub const AFRO_HOUSE: Style = Style {
    name: "afro_house",
    beats_per_bar: 4.0,
    swing: 0.3,
    swing_div: 0.25,
    vel: 92,
    human: 0.8,
    strum: 0.03,
    bass: Bass {
        walking: false,
        density: 0.85,
        approach: 0.2,
        passing: 0.35,
        pickup: 0.25,
        octave_jump: 0.25,
        low: 33,
        high: 48,
    },
    drums: Drums {
        kick: &[0.00, 1.00, 2.00, 3.00],
        snare: &[1.00, 3.00],
        cymbal: 0.25,
        ride: false,
        ghost: 0.35,
        fill_every: 8,
    },
    comp: Comp {
        hits: &[0.75, 2.25, 3.50],
        density: 0.65,
        hold: 0.75,
        low: 55,
        high: 79,
    },
    lead: Lead {
        notes: &[0.50, 1.00, 0.50, 1.50, 1.00],
        rest: 0.4,
        chromatic: 0.15,
        displace: 0.5,
        low: 62,
        high: 86,
    },
    solo: None,
    guitar: None,
};

/// A jazz comp over a programmed kit: swung eighths, a ride, and a bass
/// that walks where the drums do not.
pub const NU_JAZZ: Style = Style {
    name: "nu_jazz",
    beats_per_bar: 4.0,
    swing: 0.55,
    swing_div: 0.5,
    vel: 88,
    human: 1.0,
    strum: 0.03,
    bass: Bass {
        walking: true,
        density: 0.9,
        approach: 0.6,
        passing: 0.35,
        pickup: 0.25,
        octave_jump: 0.1,
        low: 36,
        high: 55,
    },
    drums: Drums {
        kick: &[0.00, 2.50],
        snare: &[1.00, 3.00],
        cymbal: 0.5,
        ride: true,
        ghost: 0.3,
        fill_every: 4,
    },
    comp: Comp {
        hits: &[0.50, 1.50, 3.00],
        density: 0.6,
        hold: 0.75,
        low: 52,
        high: 79,
    },
    lead: Lead {
        notes: &[0.50, 1.00, 0.50, 1.50, 2.00],
        rest: 0.3,
        chromatic: 0.4,
        displace: 0.5,
        low: 60,
        high: 86,
    },
    solo: None,
    guitar: None,
};

/// Machine-tight: the floor, an off-beat hat and long chords that move under
/// a line with almost nothing in it.
pub const MELODIC_TECHNO: Style = Style {
    name: "melodic_techno",
    beats_per_bar: 4.0,
    swing: 0.0,
    swing_div: 0.5,
    vel: 94,
    human: 0.3,
    strum: 0.03,
    bass: Bass {
        walking: false,
        density: 1.0,
        approach: 0.1,
        passing: 0.35,
        pickup: 0.25,
        octave_jump: 0.4,
        low: 33,
        high: 45,
    },
    drums: Drums {
        kick: &[0.00, 1.00, 2.00, 3.00],
        snare: &[1.00, 3.00],
        cymbal: 0.5,
        ride: false,
        ghost: 0.05,
        fill_every: 8,
    },
    comp: Comp {
        hits: &[0.00],
        density: 0.9,
        hold: 3.5,
        low: 52,
        high: 76,
    },
    lead: Lead {
        notes: &[2.00, 2.00, 1.00, 3.00, 1.00],
        rest: 0.5,
        chromatic: 0.05,
        displace: 0.5,
        low: 62,
        high: 84,
    },
    solo: None,
    guitar: None,
};

/// The same grid played by hands: the same floor, more going on above it,
/// and enough give in the timing to hear the difference.
pub const LIVE_TECHNO: Style = Style {
    name: "live_techno",
    beats_per_bar: 4.0,
    swing: 0.0,
    swing_div: 0.5,
    vel: 96,
    human: 0.8,
    strum: 0.03,
    bass: Bass {
        walking: false,
        density: 1.0,
        approach: 0.2,
        passing: 0.35,
        pickup: 0.25,
        octave_jump: 0.35,
        low: 33,
        high: 48,
    },
    drums: Drums {
        kick: &[0.00, 1.00, 2.00, 3.00],
        snare: &[1.00, 3.00],
        cymbal: 0.25,
        ride: false,
        ghost: 0.3,
        fill_every: 4,
    },
    comp: Comp {
        hits: &[0.00, 1.50, 2.50],
        density: 0.7,
        hold: 1.0,
        low: 52,
        high: 79,
    },
    lead: Lead {
        notes: &[0.50, 1.00, 1.00, 0.50, 2.00],
        rest: 0.35,
        chromatic: 0.15,
        displace: 0.5,
        low: 62,
        high: 86,
    },
    solo: None,
    guitar: None,
};

/// Long chords, a rolling off-beat bass and a line that sits on top and
/// holds its notes: what a vocal would be sung over.
pub const TRANCE_VOCAL: Style = Style {
    name: "trance_vocal",
    beats_per_bar: 4.0,
    swing: 0.0,
    swing_div: 0.5,
    vel: 98,
    human: 0.4,
    strum: 0.03,
    bass: Bass {
        walking: false,
        density: 1.0,
        approach: 0.1,
        passing: 0.35,
        pickup: 0.25,
        octave_jump: 0.15,
        low: 36,
        high: 48,
    },
    drums: Drums {
        kick: &[0.00, 1.00, 2.00, 3.00],
        snare: &[1.00, 3.00],
        cymbal: 0.5,
        ride: false,
        ghost: 0.05,
        fill_every: 8,
    },
    comp: Comp {
        hits: &[0.00, 2.00],
        density: 0.95,
        hold: 2.0,
        low: 55,
        high: 79,
    },
    lead: Lead {
        notes: &[2.00, 1.00, 2.00, 1.00, 3.00],
        rest: 0.35,
        chromatic: 0.1,
        displace: 0.5,
        low: 64,
        high: 88,
    },
    solo: None,
    guitar: None,
};

/// Sixteenths at speed: two-step kit, a bass on the one and the five, and a
/// comp of soft chords over the top.
pub const LIQUID_DNB: Style = Style {
    name: "liquid_dnb",
    beats_per_bar: 4.0,
    swing: 0.15,
    swing_div: 0.25,
    vel: 92,
    human: 0.7,
    strum: 0.03,
    bass: Bass {
        walking: false,
        density: 0.7,
        approach: 0.2,
        passing: 0.35,
        pickup: 0.25,
        octave_jump: 0.2,
        low: 33,
        high: 45,
    },
    drums: Drums {
        kick: &[0.00, 2.50],
        snare: &[1.00, 3.00],
        cymbal: 0.25,
        ride: false,
        ghost: 0.4,
        fill_every: 8,
    },
    comp: Comp {
        hits: &[0.00, 1.50, 3.00],
        density: 0.65,
        hold: 1.25,
        low: 55,
        high: 79,
    },
    lead: Lead {
        notes: &[1.00, 0.50, 2.00, 1.50, 0.50],
        rest: 0.4,
        chromatic: 0.15,
        displace: 0.5,
        low: 62,
        high: 86,
    },
    solo: None,
    guitar: None,
};

/// The same tempo played on a kit: the breaks are busier, the ghosts are
/// everywhere and the timing is a person's.
pub const LIVE_DNB: Style = Style {
    name: "live_dnb",
    beats_per_bar: 4.0,
    swing: 0.15,
    swing_div: 0.25,
    vel: 96,
    human: 1.1,
    strum: 0.03,
    bass: Bass {
        walking: false,
        density: 0.8,
        approach: 0.25,
        passing: 0.35,
        pickup: 0.25,
        octave_jump: 0.2,
        low: 33,
        high: 48,
    },
    drums: Drums {
        kick: &[0.00, 1.75, 2.50],
        snare: &[1.00, 3.00],
        cymbal: 0.25,
        ride: false,
        ghost: 0.55,
        fill_every: 4,
    },
    comp: Comp {
        hits: &[0.00, 1.50, 2.75],
        density: 0.7,
        hold: 0.75,
        low: 55,
        high: 79,
    },
    lead: Lead {
        notes: &[0.50, 0.50, 1.00, 1.50, 0.50],
        rest: 0.35,
        chromatic: 0.2,
        displace: 0.5,
        low: 62,
        high: 86,
    },
    solo: None,
    guitar: None,
};

/// Almost nothing, slowly: no kit to speak of, chords held under a line
/// that is mostly rests.
pub const NEOCLASSICAL_AMBIENT: Style = Style {
    name: "neoclassical_ambient",
    beats_per_bar: 4.0,
    swing: 0.0,
    swing_div: 0.5,
    vel: 72,
    human: 1.2,
    strum: 0.03,
    bass: Bass {
        walking: false,
        density: 0.4,
        approach: 0.1,
        passing: 0.35,
        pickup: 0.25,
        octave_jump: 0.0,
        low: 36,
        high: 52,
    },
    drums: Drums {
        kick: &[0.00],
        snare: &[],
        cymbal: 2.0,
        ride: false,
        ghost: 0.0,
        fill_every: 0,
    },
    comp: Comp {
        hits: &[0.00, 2.00],
        density: 0.85,
        hold: 2.0,
        low: 48,
        high: 79,
    },
    lead: Lead {
        notes: &[2.00, 3.00, 1.00, 4.00, 2.00],
        rest: 0.5,
        chromatic: 0.1,
        displace: 1.0,
        low: 60,
        high: 84,
    },
    solo: None,
    guitar: None,
};

/// A programmed kit under something played by hand: light backbeat, a bass
/// that holds and a line of short notes with air between them.
pub const FOLKTRONICA: Style = Style {
    name: "folktronica",
    beats_per_bar: 4.0,
    swing: 0.0,
    swing_div: 0.5,
    vel: 86,
    human: 0.9,
    strum: 0.03,
    bass: Bass {
        walking: false,
        density: 0.7,
        approach: 0.25,
        passing: 0.35,
        pickup: 0.25,
        octave_jump: 0.1,
        low: 36,
        high: 52,
    },
    drums: Drums {
        kick: &[0.00, 2.50],
        snare: &[1.00, 3.00],
        cymbal: 0.5,
        ride: false,
        ghost: 0.25,
        fill_every: 8,
    },
    comp: Comp {
        hits: &[0.00, 1.00, 2.00, 3.00],
        density: 0.55,
        hold: 0.6,
        low: 55,
        high: 79,
    },
    lead: Lead {
        notes: &[0.50, 1.00, 0.50, 1.50, 1.00],
        rest: 0.4,
        chromatic: 0.15,
        displace: 0.5,
        low: 62,
        high: 84,
    },
    solo: None,
    guitar: None,
};

/// Every style there is, in the order a list would show them.
pub const ALL: &[Style] = &[
    MAJOR_BLUES,
    MINOR_BLUES,
    JAZZ_SWING,
    SHUFFLE,
    STRAIGHT_BLUES,
    ROCK,
    FUNK,
    BOSSA,
    JAZZ_WALTZ,
    LATIN,
    COUNTRY,
    SOUL,
    REGGAE,
    BEBOP,
    WALKING_BASS,
    ORGANIC_HOUSE,
    AFRO_HOUSE,
    NU_JAZZ,
    MELODIC_TECHNO,
    LIVE_TECHNO,
    TRANCE_VOCAL,
    LIQUID_DNB,
    LIVE_DNB,
    NEOCLASSICAL_AMBIENT,
    FOLKTRONICA,
];


/// The family a style belongs to, for the one thing that is not a per-style
/// number: which instrument each role is played on.
///
/// A family rather than a field per style because the answer is the same for
/// every blues and for every house track, and writing it out twenty-five times
/// is twenty-five chances to write it differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Jazz,
    Blues,
    Rock,
    Soul,
    Latin,
    Electronic,
    Ambient,
}

impl Style {
    /// Which family this style is played out of.
    pub fn family(&self) -> Family {
        match self.name {
            "jazz_swing" | "jazz_waltz" | "bebop" | "walking_bass" | "nu_jazz" => Family::Jazz,
            "major_blues" | "minor_blues" | "shuffle" | "straight_blues" | "country" => {
                Family::Blues
            }
            "rock" => Family::Rock,
            "funk" | "soul" | "reggae" => Family::Soul,
            "latin" | "bossa" => Family::Latin,
            "neoclassical_ambient" => Family::Ambient,
            _ => Family::Electronic,
        }
    }

    /// The GM programs a role is played on, best first.
    ///
    /// A list rather than a number because a SoundFont is not obliged to have
    /// any particular one of them: the caller walks it until the bank answers,
    /// and the last entry is the acoustic piano every SoundFont has. See
    /// [`super::pick_program`].
    pub fn programs(&self, role: super::generate::Role) -> &'static [u8] {
        use super::generate::Role;
        // GM: 0 piano, 4 rhodes, 16 organ, 24 nylon, 25 steel, 26 jazz guitar,
        // 27 clean, 29 overdrive, 32 acoustic bass, 33 finger bass, 38 synth
        // bass, 48 strings, 56 trumpet, 65 alto sax, 66 tenor sax, 71 clarinet,
        // 73 flute, 80 square lead, 88 new-age pad, 89 warm pad.
        match (role, self.family()) {
            // Drums are not a program: the kit is the bank, and every caller
            // that asks gets nothing rather than a tuned instrument.
            (Role::Drums, _) => &[],
            (Role::Bass, Family::Jazz | Family::Blues | Family::Latin | Family::Ambient) => {
                &[32, 33, 43, 0]
            }
            (Role::Bass, Family::Rock | Family::Soul) => &[33, 34, 32, 0],
            (Role::Bass, Family::Electronic) => &[38, 39, 33, 0],
            (Role::Piano, Family::Jazz | Family::Blues) => &[0, 4, 1, 0],
            (Role::Piano, Family::Soul) => &[4, 16, 0],
            (Role::Piano, Family::Rock) => &[0, 16, 4, 0],
            (Role::Piano, Family::Latin) => &[0, 4, 24, 0],
            (Role::Piano, Family::Electronic) => &[89, 88, 4, 0],
            (Role::Piano, Family::Ambient) => &[88, 89, 48, 0],
            (Role::Guitar, Family::Jazz) => &[26, 27, 24, 0],
            (Role::Guitar, Family::Blues) => &[25, 27, 26, 0],
            (Role::Guitar, Family::Rock) => &[29, 30, 27, 0],
            (Role::Guitar, Family::Soul) => &[27, 28, 26, 0],
            (Role::Guitar, Family::Latin | Family::Ambient) => &[24, 25, 26, 0],
            (Role::Guitar, Family::Electronic) => &[27, 24, 0],
            // The tune: a horn where a horn would play it, a voice-shaped
            // sound where a synth would.
            (Role::Melody, Family::Jazz) => &[66, 65, 56, 71, 0],
            (Role::Melody, Family::Blues) => &[66, 56, 22, 0],
            (Role::Melody, Family::Rock) => &[30, 29, 56, 0],
            (Role::Melody, Family::Soul) => &[65, 66, 56, 0],
            (Role::Melody, Family::Latin) => &[73, 56, 65, 0],
            (Role::Melody, Family::Electronic) => &[80, 81, 73, 0],
            (Role::Melody, Family::Ambient) => &[48, 73, 88, 0],
            (Role::Solo, Family::Jazz) => &[56, 66, 65, 0],
            (Role::Solo, Family::Blues) => &[29, 66, 56, 0],
            (Role::Solo, Family::Rock) => &[30, 29, 81, 0],
            (Role::Solo, Family::Soul) => &[66, 65, 30, 0],
            (Role::Solo, Family::Latin) => &[56, 73, 65, 0],
            (Role::Solo, Family::Electronic) => &[81, 80, 62, 0],
            (Role::Solo, Family::Ambient) => &[73, 48, 0],
        }
    }
}

/// Styles read off disk, which are the same thing as the ones above once they
/// are in: the names they were given, leaked so every style in the program is
/// `&'static` whether it was written in Rust or in a file.
///
/// Leaked rather than reference-counted because a style is read once at start
/// and lives as long as the program; a style that could be dropped would mean
/// every generator holding one alive for no reason anybody can point at.
static LOADED: std::sync::OnceLock<std::sync::Mutex<Vec<&'static Style>>> =
    std::sync::OnceLock::new();

fn loaded() -> &'static std::sync::Mutex<Vec<&'static Style>> {
    LOADED.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// Every style there is right now, built-in and read from disk, in the order a
/// list would show them.
pub fn all() -> Vec<Style> {
    let mut out: Vec<Style> = ALL.to_vec();
    if let Ok(extra) = loaded().lock() {
        out.extend(extra.iter().map(|s| **s));
    }
    out
}

/// A style by the name a progression writes. Unknown names fall back to the
/// first: a misspelt style plays the wrong feel, which is a thing you can hear
/// and fix, rather than nothing at all.
pub fn by_name(name: &str) -> Style {
    let name = name.trim().to_ascii_lowercase();
    // What was read off disk wins: a file named after a built-in is somebody
    // changing that built-in, which is the whole reason for the file.
    if let Ok(extra) = loaded().lock() {
        if let Some(s) = extra.iter().find(|s| s.name == name) {
            return **s;
        }
    }
    ALL.iter()
        .find(|s| s.name == name)
        .copied()
        .unwrap_or(MAJOR_BLUES)
}

/// Read a style out of text: a base to start from and the numbers that differ.
///
/// ```text
/// name = slow_blues
/// from = minor_blues
/// swing = 0.66
/// bass.density = 0.75
/// drums.kick = 0 2.5
/// ```
///
/// Every field is optional but the name: a style file says what is *different*
/// about it, because a format that makes you write forty numbers to change one
/// is a format nobody writes twice. Read by hand rather than through a
/// serialiser — the whole grammar is `a.b = numbers`, and a dependency for that
/// would be a dependency for nothing.
pub fn parse(text: &str) -> anyhow::Result<Style> {
    use anyhow::{bail, Context};
    let mut name = String::new();
    let mut base = MAJOR_BLUES;
    let mut fields: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            bail!("{line}: not a setting");
        };
        let (key, value) = (key.trim().to_ascii_lowercase(), value.trim().to_string());
        match key.as_str() {
            "name" => name = value.to_ascii_lowercase(),
            "from" => base = by_name(&value),
            _ => fields.push((key, value)),
        }
    }
    if name.is_empty() {
        bail!("a style with no name");
    }
    let mut style = base;
    style.name = Box::leak(name.into_boxed_str());
    for (key, value) in fields {
        let one = |v: &str| -> anyhow::Result<f64> {
            v.trim()
                .parse::<f64>()
                .with_context(|| format!("{key}: {v} is not a number"))
        };
        let many = |v: &str| -> anyhow::Result<&'static [f64]> {
            let list: anyhow::Result<Vec<f64>> = v
                .split(|c: char| c.is_whitespace() || c == ',')
                .filter(|p| !p.is_empty())
                .map(|p| {
                    p.parse::<f64>()
                        .with_context(|| format!("{key}: {p} is not a number"))
                })
                .collect();
            Ok(Box::leak(list?.into_boxed_slice()))
        };
        match key.as_str() {
            "beats_per_bar" => style.beats_per_bar = one(&value)?,
            "swing" => style.swing = one(&value)? as f32,
            "swing_div" => style.swing_div = one(&value)?,
            "vel" => style.vel = one(&value)?.clamp(1.0, 127.0) as u8,
            "human" => style.human = one(&value)? as f32,
            "strum" => style.strum = one(&value)?,
            "bass.walking" => style.bass.walking = one(&value)? != 0.0,
            "bass.density" => style.bass.density = one(&value)? as f32,
            "bass.approach" => style.bass.approach = one(&value)? as f32,
            "bass.passing" => style.bass.passing = one(&value)? as f32,
            "bass.pickup" => style.bass.pickup = one(&value)? as f32,
            "bass.octave_jump" => style.bass.octave_jump = one(&value)? as f32,
            "bass.low" => style.bass.low = one(&value)?.clamp(0.0, 127.0) as u8,
            "bass.high" => style.bass.high = one(&value)?.clamp(0.0, 127.0) as u8,
            "drums.kick" => style.drums.kick = many(&value)?,
            "drums.snare" => style.drums.snare = many(&value)?,
            "drums.cymbal" => style.drums.cymbal = one(&value)?,
            "drums.ride" => style.drums.ride = one(&value)? != 0.0,
            "drums.ghost" => style.drums.ghost = one(&value)? as f32,
            "drums.fill_every" => style.drums.fill_every = one(&value)?.max(0.0) as usize,
            "comp.hits" => style.comp.hits = many(&value)?,
            "comp.density" => style.comp.density = one(&value)? as f32,
            "comp.hold" => style.comp.hold = one(&value)?,
            "comp.low" => style.comp.low = one(&value)?.clamp(0.0, 127.0) as u8,
            "comp.high" => style.comp.high = one(&value)?.clamp(0.0, 127.0) as u8,
            "lead.notes" => style.lead.notes = many(&value)?,
            "lead.rest" => style.lead.rest = one(&value)? as f32,
            "lead.chromatic" => style.lead.chromatic = one(&value)? as f32,
            "lead.displace" => style.lead.displace = one(&value)?,
            "lead.low" => style.lead.low = one(&value)?.clamp(0.0, 127.0) as u8,
            "lead.high" => style.lead.high = one(&value)?.clamp(0.0, 127.0) as u8,
            other => bail!("{other}: not a setting a style has"),
        }
    }
    if style.beats_per_bar <= 0.0 {
        bail!("{}: a bar of no beats", style.name);
    }
    Ok(style)
}

/// Read every `*.style` file in a folder, newest reading last.
///
/// Missing folder is not an error: styles from disk are something a user adds,
/// not something choz ships. A file that does not read is reported and the rest
/// are still read — one bad file must not cost the others.
pub fn load_dir(dir: &std::path::Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut paths: Vec<std::path::PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "style"))
        .collect();
    paths.sort();
    let mut n = 0;
    for path in paths {
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("choz: style {}: {e}", path.display());
                continue;
            }
        };
        match parse(&text) {
            Ok(style) => {
                let style: &'static Style = Box::leak(Box::new(style));
                if let Ok(mut list) = loaded().lock() {
                    // A name read twice is the later file: the folder is a
                    // place to override, not a place to accumulate.
                    list.retain(|s| s.name != style.name);
                    list.push(style);
                    n += 1;
                }
            }
            Err(e) => eprintln!("choz: style {}: {e:#}", path.display()),
        }
    }
    n
}
