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
    while !rest.is_empty() {
        let before = rest;
        for (token, alter) in [
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
/// A bar's chords divide it evenly: two chords in a bar of four are two beats
/// each, which is what `I7 IIIb7` means everywhere it is written that way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bar {
    pub chords: Vec<Chord>,
}

/// What was written: a key, a style by name, and the bars.
#[derive(Debug, Clone, PartialEq, Eq)]
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
}

impl Progression {
    /// The chord sounding at `beat` of the whole arrangement, and how many
    /// beats are left of it.
    pub fn at(&self, beat: f64, beats_per_bar: f64) -> Option<(&Chord, f64)> {
        if self.bars.is_empty() || beats_per_bar <= 0.0 {
            return None;
        }
        let bar = (beat / beats_per_bar).floor() as usize;
        let bar = self.bars.get(bar.min(self.bars.len() - 1))?;
        let into = beat.rem_euclid(beats_per_bar);
        let share = beats_per_bar / bar.chords.len() as f64;
        let index = ((into / share).floor() as usize).min(bar.chords.len() - 1);
        // A chord held over the slots next to it is one chord, not three: the
        // way to write `I7` for three beats of four is `| I7 - - IV7 |`, and a
        // bass line told it changes every beat would approach a change that is
        // not happening.
        let mut last = index;
        while bar.chords.get(last + 1) == Some(&bar.chords[index]) {
            last += 1;
        }
        Some((&bar.chords[index], share * (last + 1) as f64 - into))
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
        self.bars.len() as f64 * beats_per_bar
    }
}

/// Read a progression: the headers, then the bars.
///
/// Tolerant on purpose — `||` and `|` are the same delimiter, blank bars carry
/// the last chord on, and `-` holds the one before it. What is not tolerated is
/// a chord it cannot read: silently dropping one would play a different song
/// than the one written.
pub fn parse_progression(text: &str) -> Result<Progression> {
    let mut key = None;
    let mut style = String::new();
    let mut form: Vec<String> = Vec::new();
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
                    other => bail!("{other}: not a setting the arranger knows"),
                }
                continue;
            }
        }
        let body = &mut parts[cur].1;
        body.push(' ');
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
    Ok(Progression {
        key: key.unwrap_or(0),
        style,
        bars,
        sections,
    })
}

/// The bars of one part. Split out of [`parse_progression`] so a form can ask
/// for the same part twice without the chords being read twice differently.
fn parse_bars(body: &str, key: Option<u8>) -> Result<Vec<Bar>> {

    let mut bars = Vec::new();
    let mut last: Option<Chord> = None;
    for bar in body.split('|') {
        let bar = bar.trim();
        if bar.is_empty() {
            // `||` at either end, and the space between two bar lines.
            continue;
        }
        let mut chords = Vec::new();
        for symbol in bar.split_whitespace() {
            let chord = match symbol {
                "-" | "%" | "/" => match last.clone() {
                    Some(chord) => chord,
                    None => bail!("{symbol}: nothing before it to hold"),
                },
                _ => parse(symbol, key)?,
            };
            last = Some(chord.clone());
            chords.push(chord);
        }
        if chords.is_empty() {
            continue;
        }
        bars.push(Bar { chords });
    }
    Ok(bars)
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
        assert_eq!(parse("IV7", Some(bb)).unwrap().root, pitch_class("Eb").unwrap());
        assert_eq!(parse("V7", Some(bb)).unwrap().root, pitch_class("F").unwrap());
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
        assert_eq!(parse("C7b9", None).unwrap().scale, vec![0, 1, 3, 4, 6, 8, 10]);
        assert_eq!(parse("Cm7", None).unwrap().scale, vec![0, 2, 3, 5, 7, 9, 10]);
        assert_eq!(parse("Cm7b5", None).unwrap().scale, vec![0, 1, 3, 5, 6, 8, 10]);
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
}
