//! The text, and what a chord *is* in notes.
//!
//! Every rule of theory the arranger knows lives here. The generators ask this
//! module what a chord's notes are and what scale goes over it; none of them
//! works it out on its own, or there would be two answers to the same question
//! the first time a rule changed.
//!
//! Two spellings of the same thing go through one grammar: roman degrees
//! (`I7`, `IIm7`, `IV#m7b5`, `VIIb7`, `I7b9`) which need a key, and american
//! symbols (`C7`, `F#m7b5`, `Bbmaj9`) which do not — the key is still read as
//! context for the scales, not for the roots.

use anyhow::{bail, Result};

/// Semitones above the tonic for each degree of the major scale, `I`..`VII`.
const DEGREE: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];

/// Which triad and seventh a symbol's letters ask for, before the number and
/// the alterations have their say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Maj,
    Min,
    Dom,
    Dim,
    HalfDim,
    Aug,
}

/// A chord, resolved: a root and the notes that are in it.
///
/// `tones` and `scale` are semitones **above the root**, ascending and starting
/// at 0. A generator turns them into pitches by adding the root and an octave;
/// where in the register that lands is the generator's business, not this
/// module's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chord {
    /// Pitch class, 0..11, `0` = C.
    pub root: u8,
    /// What is in the chord: root, third, fifth, seventh, tensions.
    pub tones: Vec<i32>,
    /// What may be played over it.
    pub scale: Vec<i32>,
    /// What was written, for the panel and for error messages.
    pub symbol: String,
}

impl Chord {
    /// The chord's pitch classes, which is what "is this note in the chord"
    /// asks.
    pub fn pitch_classes(&self) -> Vec<u8> {
        self.tones
            .iter()
            .map(|t| ((self.root as i32 + t).rem_euclid(12)) as u8)
            .collect()
    }

    /// The `n`th chord tone as a pitch class, wrapping round the chord.
    pub fn tone(&self, n: usize) -> u8 {
        let t = self.tones[n % self.tones.len()];
        ((self.root as i32 + t).rem_euclid(12)) as u8
    }

    /// The chord tone nearest `note`, as a MIDI note. What a bass line and a
    /// voicing both need and would otherwise each write badly.
    pub fn nearest_tone(&self, note: u8) -> u8 {
        let mut best = note;
        let mut dist = i32::MAX;
        for pc in self.pitch_classes() {
            for oct in -1..=1 {
                let cand = (note as i32 / 12 + oct) * 12 + pc as i32;
                let d = (cand - note as i32).abs();
                if (0..=127).contains(&cand) && d < dist {
                    dist = d;
                    best = cand as u8;
                }
            }
        }
        best
    }
}

/// `C`, `Bb`, `F#` — a note name as a pitch class.
pub fn pitch_class(name: &str) -> Result<u8> {
    let mut chars = name.chars();
    let letter = match chars.next() {
        Some(c) => c.to_ascii_uppercase(),
        None => bail!("empty note name"),
    };
    let base = match letter {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => bail!("{name}: not a note name"),
    };
    let mut pc: i32 = base;
    for c in chars {
        match c {
            '#' | '♯' => pc += 1,
            'b' | '♭' => pc -= 1,
            _ => bail!("{name}: not a note name"),
        }
    }
    Ok(pc.rem_euclid(12) as u8)
}

/// `0` = C, the way the rest of choz names notes.
pub fn pitch_class_name(pc: u8) -> &'static str {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    NAMES[(pc % 12) as usize]
}

/// `0` = C, with a flat: what a note called by a flat function is called.
pub fn pitch_class_name_flat(pc: u8) -> &'static str {
    const FLAT: [&str; 12] = [
        "C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B",
    ];
    FLAT[(pc % 12) as usize]
}

/// `0` = C, spelled the way a chart in `key` would spell it.
///
/// The sharps are only right half the time: the flat seven of C is `Bb`
/// everywhere it is written, and `A#` is a chart a player has to translate. Two
/// rules, which is all a key really says about spelling — the flat side of the
/// circle writes flats, and the degrees a progression *reaches by flattening*
/// (`bII`, `bIII`, `bVI`, `bVII`) are flats in any key. What is left is the
/// sharp four, which is a sharp.
pub fn pitch_class_name_in_key(pc: u8, key: u8) -> &'static str {
    // F, Bb, Eb, Ab, Db, Gb: the flat side of the circle.
    const FLAT_KEYS: [u8; 6] = [5, 10, 3, 8, 1, 6];
    let semis = (pc as i32 - key as i32).rem_euclid(12);
    let flat = FLAT_KEYS.contains(&(key % 12)) || matches!(semis, 1 | 3 | 8 | 10);
    match flat {
        true => pitch_class_name_flat(pc),
        false => pitch_class_name(pc),
    }
}

/// A symbol split where the root ends: `Cm7b5` is `(0, "m7b5")`, `IIm7` in C is
/// `(2, "m7")`. `None` for a symbol that is not a chord.
///
/// What a dialogue that *builds* a chord opens on: it has to know what is
/// already written without re-implementing the grammar that reads it.
pub fn root_and_quality(symbol: &str, key: u8) -> Option<(u8, String)> {
    let sym = symbol.trim();
    if matches!(sym, "-" | "%" | "/" | "") {
        return None;
    }
    let (root, rest) = split_root(sym, Some(key)).ok()?;
    // A quality that does not parse is not a chord: the caller would build a
    // dialogue around something the arranger refuses.
    quality(rest).ok()?;
    Some((root, rest.to_string()))
}

/// Whether a chart is written in degrees, by reading the first symbol in it.
///
/// The notation is a property of the text, not a flag beside it: a chart opened
/// from a file says which it is, and a switch that disagreed with what is on
/// screen would be the worst of both.
pub fn is_roman(text: &str) -> bool {
    for line in text.lines() {
        let body = line.split('#').next().unwrap_or("").trim();
        if body.is_empty()
            || body.starts_with('[')
            || body
                .split_once('=')
                .is_some_and(|(name, _)| !name.contains('|'))
        {
            continue;
        }
        for token in body.split(['|', ' ', '\t']) {
            let sym = token.split(':').next().unwrap_or("").trim();
            if sym.is_empty() || matches!(sym, "-" | "%" | "/") {
                continue;
            }
            return sym.starts_with(['I', 'V']);
        }
    }
    false
}

/// The same chord, spelled the other way: `C7` in the key of C is `I7`, and
/// `I7` is `C7`.
///
/// What the arranger's notation switch is: a chart is read as degrees by
/// somebody thinking about the form and as letters by somebody playing it, and
/// they are the same chart. A hold (`-`) is a hold either way, and a symbol that
/// does not parse is handed back untouched — it is somebody's typing, not ours
/// to lose.
pub fn respell(symbol: &str, key: u8, roman: bool) -> String {
    let sym = symbol.trim();
    if matches!(sym, "-" | "%" | "/" | "") {
        return sym.to_string();
    }
    let Ok((root, rest)) = split_root(sym, Some(key)) else {
        return sym.to_string();
    };
    match roman {
        true => format!("{}{rest}", roman_of(root, key)),
        false => format!("{}{rest}", pitch_class_name_in_key(root, key)),
    }
}

/// The degree a pitch class is in `key`, in the notation [`split_root`] reads:
/// the accidental comes **after** the numeral, so the flat seven is `VIIb`.
fn roman_of(root: u8, key: u8) -> String {
    const NUMERAL: [&str; 7] = ["I", "II", "III", "IV", "V", "VI", "VII"];
    let semis = (root as i32 - key as i32).rem_euclid(12);
    if let Some(i) = DEGREE.iter().position(|d| *d == semis) {
        return NUMERAL[i].to_string();
    }
    // Between two degrees: the one above it, flattened — `bIII` rather than
    // `#II`, which is how a progression is written.
    match DEGREE.iter().position(|d| *d > semis) {
        Some(i) => format!("{}b", NUMERAL[i]),
        None => format!("{}#", NUMERAL[6]),
    }
}

/// A whole chart, respelled. The lines that are not bars — the headers, the
/// section names — are left exactly as they were, and so is every space and bar
/// line: only the symbols change, so a chart keeps the shape it was written in.
pub fn respell_text(text: &str, roman: bool) -> String {
    let key = settings_of(text).0.unwrap_or(0);
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let body = line.split('#').next().unwrap_or("");
        let is_bar = !body.trim().is_empty()
            && !body.trim_start().starts_with('[')
            && !body
                .split_once('=')
                .is_some_and(|(name, _)| !name.contains('|'));
        if !is_bar {
            out.push_str(line);
            continue;
        }
        // Word by word, keeping everything that is not a word: the `|`, the
        // spaces a chart is laid out with, and a comment at the end of the line.
        let mut word = String::new();
        for c in line.chars() {
            match c.is_whitespace() || c == '|' || c == '#' {
                true => {
                    if !word.is_empty() {
                        out.push_str(&respell_token(&word, key, roman));
                        word.clear();
                    }
                    out.push(c);
                }
                false => word.push(c),
            }
        }
        if !word.is_empty() {
            out.push_str(&respell_token(&word, key, roman));
        }
    }
    out
}

/// One word of a bar: the symbol respelled, the `:3` that says how long it is
/// left alone.
fn respell_token(token: &str, key: u8, roman: bool) -> String {
    match token.split_once(':') {
        Some((sym, weight)) => format!("{}:{weight}", respell(sym, key, roman)),
        None => respell(token, key, roman),
    }
}

/// One chord symbol, roman or american.
///
/// `key` is the tonic as a pitch class. Roman degrees without one are an error
/// rather than a guess: `I7` in no key is not a chord.
pub fn parse(symbol: &str, key: Option<u8>) -> Result<Chord> {
    let sym = symbol.trim();
    if sym.is_empty() {
        bail!("empty chord symbol");
    }
    let (root, rest) = split_root(sym, key)?;
    let (tones, scale) = quality(rest).map_err(|e| anyhow::anyhow!("{sym}: {e}"))?;
    Ok(Chord {
        root,
        tones,
        scale,
        symbol: sym.to_string(),
    })
}

/// The root, and whatever the quality is written in after it.
fn split_root(sym: &str, key: Option<u8>) -> Result<(u8, &str)> {
    // Roman first: `I`..`VII`, longest match, because `III` also starts `I`.
    // Upper case only — a lower-case `i` would collide with nothing here, but
    // it is also how nobody writes a progression, and `IIm7` says minor with
    // its own letters.
    let numeral = sym
        .char_indices()
        .take_while(|(_, c)| matches!(c, 'I' | 'V'))
        .last()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    if numeral > 0 {
        let (head, mut rest) = sym.split_at(numeral);
        let degree = match head {
            "I" => 0,
            "II" => 1,
            "III" => 2,
            "IV" => 3,
            "V" => 4,
            "VI" => 5,
            "VII" => 6,
            _ => bail!("{sym}: {head} is not a degree"),
        };
        let Some(key) = key else {
            bail!("{sym}: a roman degree needs a key");
        };
        // An accidental **immediately** after the numeral raises or lowers the
        // degree; one further in is an altered tension. That is what tells
        // `VIIb7` (flat seven, dominant) from `I7b9`.
        let mut offset = 0;
        while let Some(stripped) = rest.strip_prefix(['#', '♯']) {
            offset += 1;
            rest = stripped;
        }
        while let Some(stripped) = rest.strip_prefix(['b', '♭']) {
            offset -= 1;
            rest = stripped;
        }
        let root = (key as i32 + DEGREE[degree] + offset).rem_euclid(12) as u8;
        return Ok((root, rest));
    }
    // American: a letter and its accidentals.
    let len = sym
        .char_indices()
        .take_while(|(i, c)| *i == 0 || matches!(c, '#' | '♯' | 'b' | '♭'))
        .map(|(i, c)| i + c.len_utf8())
        .last()
        .unwrap_or(0);
    // The accidental right after the letter is **always** part of the root:
    // `Bb9` is a B flat ninth, not a B with a flat nine. The chord that wants
    // that alteration is written `B7b9`, which is how everybody writes it and
    // what the loop below reads.
    Ok((pitch_class(&sym[..len])?, &sym[len..]))
}

/// One thing a quality can say, so the table below stays a table.
enum Alter {
    /// `m(maj7)`, `mMaj9`: the major seventh on top of whatever triad the
    /// letters asked for. Written after the family, so it cannot be the family.
    MajSeventh(i32),
    Sus(i32),
    Add9,
    B5,
    S5,
    B9,
    S9,
    S11,
    B13,
    Number(i32),
}

/// The letters and numbers after the root: `m7b5`, `maj9`, `7#11`, `sus4`.
fn quality(mut rest: &str) -> Result<(Vec<i32>, Vec<i32>), String> {
    let mut family = None;
    for (token, f) in [
        ("maj", Family::Maj),
        ("Maj", Family::Maj),
        ("MAJ", Family::Maj),
        ("Δ", Family::Maj),
        ("M", Family::Maj),
        ("min", Family::Min),
        ("m", Family::Min),
        ("-", Family::Min),
        ("dim", Family::Dim),
        ("°", Family::Dim),
        ("o", Family::Dim),
        ("ø", Family::HalfDim),
        ("aug", Family::Aug),
        ("+", Family::Aug),
    ] {
        if let Some(stripped) = rest.strip_prefix(token) {
            family = Some(f);
            rest = stripped;
            break;
        }
    }

    // The rest is a number and any number of alterations, in whatever order
    // they were written.
    let mut number = 0;
    let (mut sus, mut add9) = (None, false);
    let (mut b5, mut s5) = (false, false);
    let (mut b9, mut s9, mut s11, mut b13) = (false, false, false, false);
    let mut major_seventh = false;
    while !rest.is_empty() {
        let before = rest;
        for (token, alter) in [
            ("(maj7)", Alter::MajSeventh(7)),
            ("(maj9)", Alter::MajSeventh(9)),
            ("maj13", Alter::MajSeventh(13)),
            ("maj11", Alter::MajSeventh(11)),
            ("maj9", Alter::MajSeventh(9)),
            ("maj7", Alter::MajSeventh(7)),
            ("sus2", Alter::Sus(2)),
            ("sus4", Alter::Sus(5)),
            ("sus", Alter::Sus(5)),
            ("add9", Alter::Add9),
            ("b5", Alter::B5),
            ("♭5", Alter::B5),
            ("#5", Alter::S5),
            ("♯5", Alter::S5),
            ("b9", Alter::B9),
            ("♭9", Alter::B9),
            ("#9", Alter::S9),
            ("♯9", Alter::S9),
            ("#11", Alter::S11),
            ("♯11", Alter::S11),
            ("b13", Alter::B13),
            ("♭13", Alter::B13),
            ("13", Alter::Number(13)),
            ("11", Alter::Number(11)),
            ("9", Alter::Number(9)),
            ("7", Alter::Number(7)),
            ("6", Alter::Number(6)),
            ("5", Alter::Number(5)),
        ] {
            let Some(stripped) = rest.strip_prefix(token) else {
                continue;
            };
            rest = stripped;
            match alter {
                Alter::MajSeventh(n) => {
                    major_seventh = true;
                    number = number.max(n);
                }
                Alter::Sus(s) => sus = Some(s),
                Alter::Add9 => add9 = true,
                Alter::B5 => b5 = true,
                Alter::S5 => s5 = true,
                Alter::B9 => b9 = true,
                Alter::S9 => s9 = true,
                Alter::S11 => s11 = true,
                Alter::B13 => b13 = true,
                Alter::Number(n) => number = number.max(n),
            }
            break;
        }
        if rest.len() == before.len() {
            return Err(format!("{rest} is not a chord quality"));
        }
    }

    // A number of seven or more with no letters in front of it is a dominant:
    // that is what `I7` and `C7` mean and the reason the blues is writeable at
    // all. Anything else with no letters is a plain major.
    let family = family.unwrap_or(match number >= 7 {
        true => Family::Dom,
        false => Family::Maj,
    });

    let mut tones = vec![0];
    match sus {
        Some(s) => tones.push(s),
        // A fifth chord — `C5` — has no third on purpose.
        None if number == 5 => {}
        None => tones.push(match family {
            Family::Min | Family::Dim | Family::HalfDim => 3,
            _ => 4,
        }),
    }
    tones.push(match (family, b5, s5) {
        (_, true, _) => 6,
        (_, _, true) => 8,
        (Family::Dim | Family::HalfDim, ..) => 6,
        (Family::Aug, ..) => 8,
        _ => 7,
    });
    // `dim` with no number is a diminished **seventh**: that is what it means
    // in a progression, whoever wrote it.
    let number = match family == Family::Dim && number == 0 {
        true => 7,
        false => number,
    };
    if number >= 7 {
        tones.push(match family {
            // A minor with a major seventh is the one chord whose seventh its
            // letters do not say: `Cm(maj7)` is a minor triad and a B.
            _ if major_seventh => 11,
            Family::Maj => 11,
            // A full diminished seventh, which is what `dim7` means and what
            // `dim` is taken to mean in a progression.
            Family::Dim => 9,
            _ => 10,
        });
    } else if number == 6 {
        tones.push(9);
    }
    if number >= 9 || b9 || s9 || add9 {
        tones.push(match (b9, s9) {
            (true, _) => 13,
            (_, true) => 15,
            _ => 14,
        });
    }
    if number >= 11 || s11 {
        tones.push(if s11 { 18 } else { 17 });
    }
    if number >= 13 || b13 {
        tones.push(if b13 { 20 } else { 21 });
    }
    tones.sort_unstable();
    tones.dedup();

    // One scale per chord, chosen the way a player would: the mode the chord
    // came from, and the altered scale when the chord says it is leaving.
    let scale = match family {
        Family::Maj => match s11 {
            true => vec![0, 2, 4, 6, 7, 9, 11],
            false => vec![0, 2, 4, 5, 7, 9, 11],
        },
        // `m7b5` is written with an `m`, so the flat five is what tells the
        // half-diminished from the dorian minor — not the letters.
        Family::HalfDim => vec![0, 1, 3, 5, 6, 8, 10],
        Family::Min if b5 => vec![0, 1, 3, 5, 6, 8, 10],
        // Melodic minor: the scale the minor-major seventh came from.
        Family::Min if major_seventh => vec![0, 2, 3, 5, 7, 9, 11],
        Family::Min => vec![0, 2, 3, 5, 7, 9, 10],
        Family::Dim => vec![0, 2, 3, 5, 6, 8, 9, 11],
        Family::Aug => vec![0, 2, 4, 6, 8, 10],
        Family::Dom => match (b9 || s9 || s5, s11) {
            (true, _) => vec![0, 1, 3, 4, 6, 8, 10],
            (_, true) => vec![0, 2, 4, 6, 7, 9, 10],
            _ => vec![0, 2, 4, 5, 7, 9, 10],
        },
    };
    Ok((tones, scale))
}

// ─── The progression ────────────────────────────────────────────────────────

/// One bar of the progression, and the chords that share it.
///
/// A bar's chords divide it by `weights`: `| C G |` in four is two beats each,
/// and `| Cm:3 F:2 Bb:2 |` in 7/8 is the bar grouped 3+2+2 — the metronome's
/// own grouping, written where the chords are. `weights` is always as long as
/// `chords`; all ones is the even split every chart written before the
/// groupings means.
#[derive(Debug, Clone, PartialEq)]
pub struct Bar {
    pub chords: Vec<Chord>,
    /// How much of the bar each chord holds, as shares of their sum. Never
    /// empty, never zero — see [`Bar::new`].
    pub weights: Vec<f64>,
    /// The bar's own signature, when the chart changes meter: `| 5/4 Fm |`,
    /// and every bar after it until the next change. `None` is the chart's
    /// (or the style's) bar — see [`Progression::bar_beats`].
    pub meter: Option<(u16, u16)>,
}

impl Bar {
    /// A bar of chords with the weight each one was written with. A missing or
    /// unusable weight is `1.0`, which is the even split.
    pub fn new(chords: Vec<Chord>, weights: Vec<f64>) -> Self {
        let mut weights = weights;
        weights.resize(chords.len(), 1.0);
        for w in &mut weights {
            if !w.is_finite() || *w <= 0.0 {
                *w = 1.0;
            }
        }
        Self {
            chords,
            weights,
            meter: None,
        }
    }

    /// Where slot `i` starts and how long it is, in beats of a
    /// `beats_per_bar` bar.
    fn slot(&self, i: usize, beats_per_bar: f64) -> (f64, f64) {
        let total: f64 = self.weights.iter().sum();
        if total <= 0.0 || self.weights.is_empty() {
            return (0.0, beats_per_bar);
        }
        let start: f64 = self.weights[..i.min(self.weights.len())].iter().sum();
        let len = self.weights.get(i).copied().unwrap_or(total);
        (beats_per_bar * start / total, beats_per_bar * len / total)
    }

    /// Which slot the bar is `into` beats in, or `None` for an empty bar.
    fn slot_at(&self, into: f64, beats_per_bar: f64) -> Option<usize> {
        if self.chords.is_empty() {
            return None;
        }
        let last = self.chords.len() - 1;
        for i in 0..=last {
            let (start, len) = self.slot(i, beats_per_bar);
            if into < start + len {
                return Some(i);
            }
        }
        Some(last)
    }

    /// The grouping the bar is written in, as the metronome counts one: the
    /// weights as whole numbers when they are whole, which is every bar a
    /// grouping wrote.
    pub fn groups(&self) -> Vec<u8> {
        self.weights
            .iter()
            .map(|w| w.round().clamp(1.0, 15.0) as u8)
            .collect()
    }
}

/// What was written: a key, a style by name, and the bars.
#[derive(Debug, Clone, PartialEq)]
pub struct Progression {
    /// Tonic as a pitch class. `C` when nothing said otherwise — american
    /// symbols do not need it, and the scales still want a context.
    pub key: u8,
    /// The style's name, resolved by [`super::style`]. Empty when the text did
    /// not name one.
    pub style: String,
    /// Every bar of the arrangement, in the order it is played — the form
    /// already laid out flat. A text with sections and a `form` line is
    /// expanded here at parse time, so nothing downstream has to know that a
    /// part can be played twice.
    pub bars: Vec<Bar>,
    /// `(name, first bar)` of each section, in playing order. Empty for a text
    /// that never named one. What the panel says you are in.
    pub sections: Vec<(String, usize)>,
    /// `meter = 7/8`: the bar this chart is written in. `None` leaves it to the
    /// session, and to the style under that.
    pub meter: Option<(u16, u16)>,
    /// `groups = 3+2+2`: how the bar is counted, in the metronome's notation.
    /// Empty leaves it to the tab and the click.
    pub groups: Vec<u8>,
}

impl Progression {
    /// Whether any bar has a signature of its own — a chart that changes
    /// meter. Everything below takes the plain arithmetic when it does not.
    pub fn heterometric(&self) -> bool {
        self.bars.iter().any(|b| b.meter.is_some())
    }

    /// How long bar `i` is, in quarter-note beats: its own signature, or
    /// `beats_per_bar`.
    pub fn bar_beats(&self, i: usize, beats_per_bar: f64) -> f64 {
        match self.bars.get(i).and_then(|b| b.meter) {
            Some((num, den)) => (num as f64 * 4.0 / den.max(1) as f64).max(0.25),
            None => beats_per_bar,
        }
    }

    /// Where bar `i` starts, in beats from the top.
    pub fn bar_start(&self, i: usize, beats_per_bar: f64) -> f64 {
        match self.heterometric() {
            false => i as f64 * beats_per_bar,
            true => (0..i.min(self.bars.len()))
                .map(|j| self.bar_beats(j, beats_per_bar))
                .sum(),
        }
    }

    /// Which bar `beat` falls in and how far into it. Past the end is the last
    /// bar, the way it always read.
    pub fn locate(&self, beat: f64, beats_per_bar: f64) -> Option<(usize, f64)> {
        if self.bars.is_empty() || beats_per_bar <= 0.0 {
            return None;
        }
        if !self.heterometric() {
            let bar = ((beat / beats_per_bar).floor() as usize).min(self.bars.len() - 1);
            return Some((bar, beat.rem_euclid(beats_per_bar)));
        }
        let mut start = 0.0;
        for i in 0..self.bars.len() {
            let len = self.bar_beats(i, beats_per_bar);
            if beat < start + len || i + 1 == self.bars.len() {
                return Some((i, (beat - start).clamp(0.0, len - 1e-9)));
            }
            start += len;
        }
        None
    }

    /// The chord sounding at `beat` of the whole arrangement, and how many
    /// beats are left of it.
    pub fn at(&self, beat: f64, beats_per_bar: f64) -> Option<(&Chord, f64)> {
        let (i, into) = self.locate(beat, beats_per_bar)?;
        let bar = self.bars.get(i)?;
        let beats_per_bar = self.bar_beats(i, beats_per_bar);
        let index = bar.slot_at(into, beats_per_bar)?;
        // A chord held over the slots next to it is one chord, not three: the
        // way to write `I7` for three beats of four is `| I7 - - IV7 |`, and a
        // bass line told it changes every beat would approach a change that is
        // not happening.
        let mut last = index;
        while bar.chords.get(last + 1) == Some(&bar.chords[index]) {
            last += 1;
        }
        let (start, len) = bar.slot(last, beats_per_bar);
        Some((&bar.chords[index], start + len - into))
    }

    /// The chord that follows the one at `beat` — what a bass line has to know
    /// a beat before it gets there.
    pub fn next_after(&self, beat: f64, beats_per_bar: f64) -> Option<&Chord> {
        let (chord, left) = self.at(beat, beats_per_bar)?;
        let total = self.bars.len() as f64 * beats_per_bar;
        let next = (beat + left + beats_per_bar * 1e-6).rem_euclid(total);
        let (after, _) = self.at(next, beats_per_bar)?;
        // A progression that never changes chord: the answer is the same one,
        // and a generator that approaches it is approaching where it already is.
        Some(match after == chord {
            true => chord,
            false => after,
        })
    }

    pub fn beats(&self, beats_per_bar: f64) -> f64 {
        self.bar_start(self.bars.len(), beats_per_bar)
    }

    /// Where each chord starts, in beats from the top, and what it is called —
    /// a held chord (`C - -`) once, not once a slot.
    pub fn changes(&self, beats_per_bar: f64) -> Vec<(f64, &str)> {
        let mut out: Vec<(f64, &str)> = Vec::new();
        let mut last: Option<&Chord> = None;
        for (b, bar) in self.bars.iter().enumerate() {
            let len = self.bar_beats(b, beats_per_bar);
            let from = self.bar_start(b, beats_per_bar);
            for (i, chord) in bar.chords.iter().enumerate() {
                if last != Some(chord) {
                    let (start, _) = bar.slot(i, len);
                    out.push((from + start, chord.symbol.as_str()));
                }
                last = Some(chord);
            }
        }
        out
    }
}

/// Read a progression: the headers, then the bars.
///
/// The `key` and the `style` a chart names, without reading its bars.
///
/// What a text that does not parse is still asked: both are picked from
/// dialogues of their own and written back into the text, and a typo three bars
/// down must not be what swallows the pick.
pub fn settings_of(text: &str) -> (Option<u8>, Option<String>) {
    let mut key = None;
    let mut style = None;
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        match name.trim().to_ascii_lowercase().as_str() {
            "key" => key = pitch_class(value.trim()).ok(),
            "style" => style = Some(value.trim().to_ascii_lowercase()),
            _ => {}
        }
    }
    (key, style)
}

/// Tolerant on purpose — `||` and `|` are the same delimiter, blank bars carry
/// the last chord on, and `-` holds the one before it. What is not tolerated is
/// a chord it cannot read: silently dropping one would play a different song
/// than the one written.
pub fn parse_progression(text: &str) -> Result<Progression> {
    let mut key = None;
    let mut style = String::new();
    let mut form: Vec<String> = Vec::new();
    let mut meter = None;
    let mut groups = Vec::new();
    // The sections, in the order they were written: `("", body)` for a text
    // that never opened one, which is every text written before sections
    // existed.
    let mut parts: Vec<(String, String)> = vec![(String::new(), String::new())];
    let mut cur = 0usize;
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        // `[A]`, `[intro]`, `[end]`: everything under it belongs to that part
        // until the next one. A name written twice carries on the same part.
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            let name = name.trim().to_ascii_lowercase();
            if name.is_empty() {
                bail!("a part with no name");
            }
            // Written twice: carry on the one that is already there, and
            // leave it where it was — the order the parts were written in is
            // the form a text with no `form` line means.
            cur = match parts.iter().position(|(n, _)| *n == name) {
                Some(at) => at,
                None => {
                    parts.push((name.clone(), String::new()));
                    parts.len() - 1
                }
            };
            continue;
        }
        if let Some((name, value)) = line.split_once('=') {
            // A header, unless the bar line happens to have one in it — which
            // it cannot, so anything before the first `|` with an `=` is one.
            if !name.contains('|') {
                match name.trim().to_ascii_lowercase().as_str() {
                    "key" => key = Some(pitch_class(value.trim())?),
                    "style" => style = value.trim().to_ascii_lowercase(),
                    // The form: which parts are played, in which order.
                    "form" => {
                        form = value
                            .split_whitespace()
                            .map(|p| p.trim().to_ascii_lowercase())
                            .collect()
                    }
                    "meter" | "time" => meter = Some(parse_meter(value.trim())?),
                    "groups" | "grouping" => groups = parse_groups(value.trim())?,
                    other => bail!("{other}: not a setting the arranger knows"),
                }
                continue;
            }
        }
        // The newline is kept, because it is a bar line: a chart written in
        // rows of four bars with no `|` at the ends of them used to have the
        // last bar of each row run into the first of the next — see
        // [`parse_bars`].
        let body = &mut parts[cur].1;
        body.push('\n');
        body.push_str(line);
    }

    // The form, laid out: what `form` says, or every part in the order it was
    // written, which is what a text with no `form` line means.
    let order: Vec<String> = match form.is_empty() {
        true => parts
            .iter()
            .filter(|(_, body)| !body.trim().is_empty())
            .map(|(name, _)| name.clone())
            .collect(),
        false => form,
    };
    let mut sections = Vec::new();
    let mut bars = Vec::new();
    for name in &order {
        let Some((_, body)) = parts.iter().find(|(n, _)| n == name) else {
            bail!("{name}: no part written with that name");
        };
        let part = parse_bars(body, key)?;
        if part.is_empty() {
            bail!("{name}: a part with no bars in it");
        }
        if !name.is_empty() {
            sections.push((name.clone(), bars.len()));
        }
        bars.extend(part);
    }
    if bars.is_empty() {
        bail!("no bars in the progression");
    }
    // The same rule the metronome counts by: a grouping is of *this* bar.
    let sum: u16 = groups.iter().map(|g| *g as u16).sum();
    if let Some((num, _)) = meter.filter(|_| !groups.is_empty()) {
        if sum != num {
            bail!("groups add up to {sum}, and a {num}-beat bar needs {num}");
        }
    }
    Ok(Progression {
        key: key.unwrap_or(0),
        style,
        bars,
        sections,
        meter,
        groups,
    })
}

/// `7/8`: a count of 1 to 32 over a note value the metronome has.
fn parse_meter(value: &str) -> Result<(u16, u16)> {
    let parsed = value
        .split_once('/')
        .and_then(|(n, d)| Some((n.trim().parse::<u16>().ok()?, d.trim().parse::<u16>().ok()?)));
    match parsed {
        Some((num, den)) if (1..=32).contains(&num) && [1, 2, 4, 8, 16].contains(&den) => {
            Ok((num, den))
        }
        _ => bail!("meter = {value}: not a signature (write it 7/8)"),
    }
}

/// `3+2+2`, `3 2 2` or `3,2,2`: groups of 1 to 15 units.
fn parse_groups(value: &str) -> Result<Vec<u8>> {
    value
        .split(['+', ',', ' '])
        .filter(|g| !g.is_empty())
        .map(|g| match g.parse::<u8>() {
            Ok(n) if (1..=15).contains(&n) => Ok(n),
            _ => bail!("groups = {value}: {g} is not a group (write it 3+2+2)"),
        })
        .collect()
}

/// `C:3` — a chord and how much of its bar it holds. `1.0` when nothing said.
///
/// The `:` cannot collide with a chord symbol: no quality is written with one,
/// and a weight that is not a number is an error rather than a chord nobody
/// asked for.
fn split_weight(token: &str) -> Result<(&str, f64)> {
    let Some((symbol, weight)) = token.split_once(':') else {
        return Ok((token, 1.0));
    };
    let Ok(weight) = weight.parse::<f64>() else {
        bail!("{token}: {weight} is not how long a chord is");
    };
    if !weight.is_finite() || weight <= 0.0 {
        bail!("{token}: a chord cannot be {weight} of a bar long");
    }
    Ok((symbol, weight))
}

/// The bars of one part. Split out of [`parse_progression`] so a form can ask
/// for the same part twice without the chords being read twice differently.
fn parse_bars(body: &str, key: Option<u8>) -> Result<Vec<Bar>> {
    let mut bars = Vec::new();
    let mut last: Option<Chord> = None;
    // `| 5/4 Fm |`: the bar's signature, and every bar's after it in this part
    // until the next one — a score's rule, written where the chords are.
    let mut meter: Option<(u16, u16)> = None;
    // **A line ends a bar, whether or not it is closed with a bar line.** A
    // chart is written in rows of four:
    //
    // ```text
    // Im7  | Im7      | Im7 | Im7
    // IVm7 | IVm7     | Im7 | Im7
    // ```
    //
    // and joining the rows on nothing ran the last bar of one into the first of
    // the next — twelve bars read as ten, with two of them carrying two chords
    // nobody wrote together.
    for bar in body.split(['|', '\n']) {
        let bar = bar.trim();
        if bar.is_empty() {
            // `||` at either end, and the space between two bar lines.
            continue;
        }
        let mut chords = Vec::new();
        let mut weights = Vec::new();
        for token in bar.split_whitespace() {
            if chords.is_empty() && is_meter_token(token) {
                meter = Some(parse_meter(token)?);
                continue;
            }
            // `C:3` is a chord that holds three of the bar's own units — the
            // grouping written where the chords are, so a 7/8 counted 3+2+2 is
            // three chords of the lengths it is counted in. Without one a bar
            // divides evenly, which is what every chart written before this
            // means.
            let (symbol, weight) = split_weight(token)?;
            let chord = match symbol {
                "-" | "%" | "/" => match last.clone() {
                    Some(chord) => chord,
                    None => bail!("{symbol}: nothing before it to hold"),
                },
                _ => parse(symbol, key)?,
            };
            last = Some(chord.clone());
            chords.push(chord);
            weights.push(weight);
        }
        if chords.is_empty() {
            continue;
        }
        let mut bar = Bar::new(chords, weights);
        bar.meter = meter;
        bars.push(bar);
    }
    Ok(bars)
}

/// `5/4`, `12/8`: a signature at the head of a bar, not a chord — no chord
/// symbol is two numbers either side of a slash.
fn is_meter_token(token: &str) -> bool {
    token.split_once('/').is_some_and(|(n, d)| {
        !n.is_empty()
            && !d.is_empty()
            && n.bytes().all(|b| b.is_ascii_digit())
            && d.bytes().all(|b| b.is_ascii_digit())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcs(symbol: &str, key: Option<&str>) -> Vec<u8> {
        let key = key.map(|k| pitch_class(k).unwrap());
        parse(symbol, key).unwrap().pitch_classes()
    }

    /// A degree is only a chord once there is a key, and the accidental that
    /// moves it is the one written **on** it.
    #[test]
    fn roman_degrees_resolve_against_the_key() {
        let c = |sym| parse(sym, Some(0)).unwrap().root;
        assert_eq!(c("I7"), pitch_class("C").unwrap());
        assert_eq!(c("IV7"), pitch_class("F").unwrap());
        assert_eq!(c("VIb7"), pitch_class("Ab").unwrap(), "bVII of the degree");
        assert_eq!(c("IV#7"), pitch_class("F#").unwrap());
        assert_eq!(c("IIb7"), pitch_class("Db").unwrap());
        // Another key, same degrees.
        let bb = pitch_class("Bb").unwrap();
        assert_eq!(
            parse("IV7", Some(bb)).unwrap().root,
            pitch_class("Eb").unwrap()
        );
        assert_eq!(
            parse("V7", Some(bb)).unwrap().root,
            pitch_class("F").unwrap()
        );
        // And without one it is not a chord, rather than a guess.
        assert!(parse("I7", None).is_err());
    }

    /// `I7` in C is `C7` — the same notes by either spelling, which is the
    /// whole reason both go through one grammar.
    #[test]
    fn the_two_spellings_meet() {
        assert_eq!(pcs("I7", Some("C")), pcs("C7", None));
        assert_eq!(pcs("IIm7", Some("C")), pcs("Dm7", None));
        assert_eq!(pcs("IV#m7b5", Some("C")), pcs("F#m7b5", None));
        assert_eq!(pcs("VIIb7", Some("C")), pcs("Bb7", None));
    }

    #[test]
    fn qualities_are_the_notes_they_name() {
        let notes = |sym| parse(sym, None).unwrap().tones;
        assert_eq!(notes("C"), vec![0, 4, 7]);
        assert_eq!(notes("Cm"), vec![0, 3, 7]);
        assert_eq!(notes("C7"), vec![0, 4, 7, 10], "dominant with no letters");
        assert_eq!(notes("Cmaj7"), vec![0, 4, 7, 11]);
        assert_eq!(notes("Cm7b5"), vec![0, 3, 6, 10]);
        assert_eq!(notes("Cø7"), notes("Cm7b5"), "the same chord, one glyph");
        assert_eq!(notes("Cdim"), vec![0, 3, 6, 9], "dim is dim7 here");
        assert_eq!(notes("Caug"), vec![0, 4, 8]);
        assert_eq!(notes("Csus4"), vec![0, 5, 7]);
        assert_eq!(notes("C6"), vec![0, 4, 7, 9]);
        assert_eq!(notes("Cmaj9"), vec![0, 4, 7, 11, 14]);
        assert_eq!(notes("C7b9"), vec![0, 4, 7, 10, 13]);
        assert_eq!(notes("C7#11"), vec![0, 4, 7, 10, 18]);
        assert_eq!(notes("Cm11"), vec![0, 3, 7, 10, 14, 17]);
        // The accidental belongs to the letter: `Bb9` is a B flat ninth, and
        // the chord with a flat ninth is `B7b9`.
        assert_eq!(parse("Bb7", None).unwrap().root, pitch_class("Bb").unwrap());
        assert_eq!(parse("Bb9", None).unwrap().root, pitch_class("Bb").unwrap());
        assert_eq!(parse("Bb9", None).unwrap().tones, vec![0, 4, 7, 10, 14]);
        assert_eq!(parse("B7b9", None).unwrap().root, pitch_class("B").unwrap());
        assert!(parse("Cz9", None).is_err());
    }

    /// An altered dominant is played over a different scale from a plain one —
    /// the rule lives here and the generators never ask twice.
    #[test]
    fn a_chord_brings_its_scale() {
        assert_eq!(parse("C7", None).unwrap().scale, vec![0, 2, 4, 5, 7, 9, 10]);
        assert_eq!(
            parse("C7b9", None).unwrap().scale,
            vec![0, 1, 3, 4, 6, 8, 10]
        );
        assert_eq!(
            parse("Cm7", None).unwrap().scale,
            vec![0, 2, 3, 5, 7, 9, 10]
        );
        assert_eq!(
            parse("Cm7b5", None).unwrap().scale,
            vec![0, 1, 3, 5, 6, 8, 10]
        );
    }

    /// **A chart can change meter.** A bar that starts with a signature is in
    /// it, and so is every bar after it in the part until the next one; the
    /// bars are as long as their signatures, and a chord's place is inside its
    /// own bar.
    #[test]
    fn a_chart_changes_meter_bar_by_bar() {
        let p = parse_progression("| 5/4 C | F | 3/4 G | 7/8 Am G |").unwrap();
        let meters: Vec<_> = p.bars.iter().map(|b| b.meter).collect();
        assert_eq!(
            meters,
            vec![Some((5, 4)), Some((5, 4)), Some((3, 4)), Some((7, 8))]
        );
        assert!(p.heterometric());
        assert_eq!(p.beats(4.0), 5.0 + 5.0 + 3.0 + 3.5);
        assert_eq!(p.bar_start(3, 4.0), 13.0);
        let name = |beat: f64| p.at(beat, 4.0).unwrap().0.symbol.clone();
        assert_eq!(name(9.9), "F");
        assert_eq!(name(10.0), "G", "bar three starts at ten");
        assert_eq!(name(13.0), "Am");
        assert_eq!(name(15.0), "G", "the second half of a 7/8 bar");
        assert_eq!(
            p.at(10.0, 4.0).unwrap().1,
            3.0,
            "a 3/4 bar holds three beats"
        );
        let changes: Vec<f64> = p.changes(4.0).iter().map(|(at, _)| *at).collect();
        assert_eq!(changes, vec![0.0, 5.0, 10.0, 13.0, 14.75]);
        // A part starts back in the chart's bar, and a plain chart is not
        // heterometric at all.
        let parts = parse_progression("[a]\n| 3/4 C |\n[b]\n| F |").unwrap();
        assert_eq!(parts.bars[1].meter, None);
        assert!(!parse_progression("| C | F |").unwrap().heterometric());
        assert!(parse_progression("| 7/9 C |").is_err(), "not a signature");
    }

    /// A chart says its bar: the signature and how it is counted, in the
    /// spellings people write them in — and one that does not add up is an
    /// error, not a grouping quietly dropped.
    #[test]
    fn a_chart_names_its_meter_and_its_grouping() {
        let p = parse_progression("meter = 7/8\ngroups = 3+2+2\n| C | F |").unwrap();
        assert_eq!(p.meter, Some((7, 8)));
        assert_eq!(p.groups, vec![3, 2, 2]);
        for spelling in ["time = 7/8\ngrouping = 3 2 2", "meter=7/8\ngroups=3,2,2"] {
            let p = parse_progression(&format!("{spelling}\n| C |")).unwrap();
            assert_eq!(
                (p.meter, p.groups),
                (Some((7, 8)), vec![3, 2, 2]),
                "{spelling}"
            );
        }
        let plain = parse_progression("| C |").unwrap();
        assert_eq!((plain.meter, plain.groups), (None, vec![]));
        for bad in [
            "meter = 7\n| C |",
            "meter = 7/9\n| C |",
            "meter = 0/4\n| C |",
            "groups = 3+x\n| C |",
            "groups = 3+0\n| C |",
            "meter = 7/8\ngroups = 3+3+3\n| C |",
        ] {
            assert!(parse_progression(bad).is_err(), "{bad} read");
        }
    }

    /// A row of bars with no bar line at the ends of it is still four bars: a
    /// chart is written in rows, and the newline is a bar line.
    #[test]
    fn a_line_ends_a_bar() {
        let prog = parse_progression(
            "key = C\nIm7  | Im7      | Im7 | Im7\nIVm7 | IVm7     | Im7 | Im7\nV7   | VIb7 V7  | Im7 | V7",
        )
        .unwrap();
        assert_eq!(prog.bars.len(), 12, "the rows ran into each other");
        // The one bar that really does hold two chords is the one that was
        // written that way.
        let two: Vec<usize> = prog
            .bars
            .iter()
            .enumerate()
            .filter(|(_, b)| b.chords.len() > 1)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(two, vec![9], "{two:?}");
    }

    #[test]
    fn a_bar_can_hold_more_than_one_chord() {
        let prog = parse_progression("key = C\n|| I7 | IV7 VIIb7 | I7 IIIb7 | - ||").unwrap();
        assert_eq!(prog.bars.len(), 4);
        assert_eq!(prog.bars[1].chords.len(), 2);
        // `-` holds the chord before it, which is what the bar it is alone in
        // means.
        assert_eq!(prog.bars[3].chords[0].root, prog.bars[2].chords[1].root);

        // Two chords in a bar of four are two beats each, and the second half
        // of the bar is the second chord.
        let (first, left) = prog.at(4.0, 4.0).unwrap();
        assert_eq!(first.root, pitch_class("F").unwrap());
        assert!((left - 2.0).abs() < 1e-9, "{left}");
        let (second, _) = prog.at(6.0, 4.0).unwrap();
        assert_eq!(second.root, pitch_class("Bb").unwrap());
        // And what follows one is what the bass has to approach.
        assert_eq!(
            prog.next_after(4.0, 4.0).unwrap().root,
            pitch_class("Bb").unwrap()
        );
    }

    /// The whole of a minor blues, read as written.
    #[test]
    fn the_minor_blues_reads() {
        let text = "key = C\nstyle = minor_blues\n\n\
            || Im7 | IVm7 | Im7 | Im7 | IVm7 | IVm7 | Im7 | Im7 \
            | VIbmaj7 | V7b9 | Im7 | V7b9 ||";
        let prog = parse_progression(text).unwrap();
        assert_eq!(prog.style, "minor_blues");
        assert_eq!(prog.key, pitch_class("C").unwrap());
        assert_eq!(prog.bars.len(), 12, "twelve bars is twelve bars");
        assert_eq!(prog.beats(4.0), 48.0);
        assert_eq!(prog.bars[8].chords[0].root, pitch_class("Ab").unwrap());
        assert_eq!(prog.bars[9].chords[0].root, pitch_class("G").unwrap());
    }

    #[test]
    fn what_does_not_read_says_so() {
        assert!(parse_progression("key = H\n|| I7 ||").is_err());
        assert!(parse_progression("tempo = 120\n|| C7 ||").is_err());
        assert!(parse_progression("|| C7 | Zq9 ||").is_err());
        assert!(parse_progression("key = C").is_err(), "no bars is no song");
        assert!(parse_progression("|| - ||").is_err(), "nothing to hold");
    }

    /// A form: parts written once and played in the order the `form` line
    /// says, intro and ending included. What comes out is the bars laid flat,
    /// so nothing downstream has to know a part can be played twice.
    #[test]
    fn a_form_lays_the_parts_out_flat() {
        let prog = parse_progression(
            "key = C\nform = intro a a b a end\n\n[intro]\n| V7 |\n[a]\n| I7 | IV7 |\n[b]\n| VIm7 | V7 |\n[end]\n| I7 |",
        )
        .unwrap();
        // 1 + 2 + 2 + 2 + 2 + 1
        assert_eq!(prog.bars.len(), 10);
        let roots: Vec<u8> = prog.bars.iter().map(|b| b.chords[0].root).collect();
        assert_eq!(roots, vec![7, 0, 5, 0, 5, 9, 7, 0, 5, 0]);
        assert_eq!(
            prog.sections,
            vec![
                ("intro".to_string(), 0),
                ("a".to_string(), 1),
                ("a".to_string(), 3),
                ("b".to_string(), 5),
                ("a".to_string(), 7),
                ("end".to_string(), 9),
            ]
        );

        // No `form` line: the parts play in the order they were written, and
        // a part written in two places is one part.
        let prog = parse_progression("key = C\n[a]\n| I7 |\n[b]\n| IV7 |\n[a]\n| V7 |").unwrap();
        let roots: Vec<u8> = prog.bars.iter().map(|b| b.chords[0].root).collect();
        assert_eq!(roots, vec![0, 7, 5], "the parts did not keep their order");

        // No parts at all: what every progression written before this said.
        let plain = parse_progression("key = C\n|| I7 | IV7 ||").unwrap();
        assert_eq!(plain.bars.len(), 2);
        assert!(plain.sections.is_empty());

        // A form that names a part nobody wrote is an error, not silence.
        assert!(parse_progression("key = C\nform = a c\n[a]\n| I7 |").is_err());
    }

    /// A bar is grouped where it is written: `C:3 F:2 Bb:2` in a 7/8 bar of 3.5
    /// beats is 1.5 + 1 + 1, not three equal thirds. Without weights a bar
    /// still divides evenly, which is every chart written before this.
    #[test]
    fn a_bars_weights_are_how_long_its_chords_are() {
        let prog = parse_progression("|| Cm:3 F:2 Bb:2 | C G ||").unwrap();
        assert_eq!(prog.bars[0].groups(), vec![3, 2, 2]);
        assert_eq!(prog.bars[1].groups(), vec![1, 1], "no weights, even split");

        let sym = |beat: f64| prog.at(beat, 3.5).map(|(c, left)| (c.symbol.clone(), left));
        // The 3+2+2 bar: three eighths of Cm, two of F, two of Bb.
        let (first, left) = sym(0.0).unwrap();
        assert_eq!(first, "Cm");
        assert!((left - 1.5).abs() < 1e-9, "Cm holds 1.5 beats, not {left}");
        assert_eq!(sym(1.4).unwrap().0, "Cm", "still inside the first group");
        let (second, left) = sym(1.5).unwrap();
        assert_eq!(second, "F");
        assert!((left - 1.0).abs() < 1e-9, "F holds one beat, not {left}");
        assert_eq!(sym(2.5).unwrap().0, "Bb");
        // The even bar after it splits in two.
        assert_eq!(sym(3.5).unwrap().0, "C");
        assert_eq!(sym(5.3).unwrap().0, "G");

        // A weight that is not a length is an error rather than a chord nobody
        // wrote.
        assert!(parse_progression("|| C:x ||").is_err());
        assert!(parse_progression("|| C:0 ||").is_err());
    }

    /// The same chart, spelled the other way: degrees for whoever is thinking
    /// about the form, letters for whoever is playing it.
    #[test]
    fn a_chart_can_be_read_as_degrees_or_as_letters() {
        // In C: the degrees of a blues, and the chords they are.
        let c = pitch_class("C").unwrap();
        assert_eq!(respell("I7", c, false), "C7");
        assert_eq!(respell("IV7", c, false), "F7");
        assert_eq!(respell("VIIb7", c, false), "Bb7", "the flat seven");
        assert_eq!(respell("C7", c, true), "I7");
        assert_eq!(respell("F7", c, true), "IV7");
        assert_eq!(respell("Bb7", c, true), "VIIb7");
        assert_eq!(respell("Eb", c, true), "IIIb");
        // Round trip, in a flat key where the spelling is the point: Bb's
        // degrees are written with flats, not with A sharps.
        let bb = pitch_class("Bb").unwrap();
        assert_eq!(respell("I7", bb, false), "Bb7");
        assert_eq!(respell("IIIb", bb, false), "Db");
        assert_eq!(respell("Db", bb, true), "IIIb");
        // A hold is a hold, and typing nobody can read is handed back whole.
        assert_eq!(respell("-", c, true), "-");
        assert_eq!(respell("what", c, true), "what");

        // A whole chart keeps its shape: the headers, the sections, the bar
        // lines and the weights are where they were.
        let text = "key = C\nstyle = rock\n\n[a]\n| I7  | IVm7:3 -:2 | VIIb7 |  # tail";
        let letters = respell_text(text, false);
        assert_eq!(
            letters, "key = C\nstyle = rock\n\n[a]\n| C7  | Fm7:3 -:2 | Bb7 |  # tail",
            "{letters:?}"
        );
        assert_eq!(respell_text(&letters, true), text, "not a round trip");
        // And both read as the same music.
        let a = parse_progression(text).unwrap();
        let b = parse_progression(&letters).unwrap();
        assert_eq!(
            a.bars.iter().map(|b| b.chords[0].root).collect::<Vec<_>>(),
            b.bars.iter().map(|b| b.chords[0].root).collect::<Vec<_>>()
        );
    }

    /// The one chord whose seventh its letters do not say: a minor triad with a
    /// major seventh, out of the melodic minor.
    #[test]
    fn a_minor_can_take_a_major_seventh() {
        let notes = pcs("Cm(maj7)", None);
        assert_eq!(notes, vec![0, 3, 7, 11], "C Eb G B");
        assert_eq!(
            pcs("Cmmaj7", None),
            notes,
            "the same chord, written plainer"
        );
        let scale = parse("Cm(maj7)", None).unwrap().scale;
        assert_eq!(scale, vec![0, 2, 3, 5, 7, 9, 11], "melodic minor");
        // The plain minor seventh is untouched by it.
        assert_eq!(pcs("Cm7", None), vec![0, 3, 7, 10]);
        // And the major-major is still the major.
        assert_eq!(pcs("Cmaj7", None), vec![0, 4, 7, 11]);
    }
}
