//! A chord's *species*, as a handful of choices — the arranger's chord
//! dialogue, and the harmoniser's chord shape.
//!
//! Here rather than in the interface because the harmoniser keeps one as its
//! knobs and has to turn it into notes on its own: the choices are numbers a
//! parameter can hold, and [`ChordSpec::quality`] is what the grammar in
//! [`super::chord`] reads.

/// **The chord being built**, one row a decision.
///
/// A menu of ready-made symbols cannot say what a chart needs: a minor takes a
/// ♭9 as happily as a dominant does, a diminished takes a ninth, an augmented
/// takes a seventh. Nineteen rows of the common ones left everything else
/// unsayable, and listing every combination is a list of thousands.
///
/// So the dialogue is the *species*: pick the family, whether it has a sixth or
/// a seventh, how high it reaches, and which notes are altered. Every
/// combination writes a symbol [`super::chord::parse`] reads —
/// the test at the bottom builds all of them and parses each one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ChordSpec {
    /// Pitch class of the root, 0..11.
    pub root: u8,
    /// Index into [`FAMILIES`], [`SEVENTHS`], [`TENSIONS`], [`NINTHS`],
    /// [`ELEVENTHS`], [`THIRTEENTHS`] and [`FIFTHS`].
    pub family: usize,
    pub seventh: usize,
    pub tension: usize,
    pub ninth: usize,
    pub eleventh: usize,
    pub thirteenth: usize,
    pub fifth: usize,
}

/// The triad the chord is built on. `DOM` is the one that is a seventh rather
/// than a triad — it is how everybody names it, and writing `G7` as "major with
/// a flat seventh" would be true and useless.
pub const FAMILIES: [&str; 7] = ["MAJ", "MIN", "DOM", "DIM", "AUG", "SUS2", "SUS4"];
/// A sixth, the family's own seventh, or the major seventh over any of them —
/// which is what makes `Cm(maj7)` sayable.
pub const SEVENTHS: [&str; 4] = ["\u{2014}", "6", "7", "maj7"];
/// How high it reaches. A ninth implies the seventh under it, the way a player
/// reads it.
pub const TENSIONS: [&str; 4] = ["\u{2014}", "9", "11", "13"];
/// `alt` is both ninths over a sharp five — the altered dominant, written the
/// way a chart writes it rather than as `7#5b9#9`.
pub const NINTHS: [&str; 5] = ["\u{2014}", "\u{266D}9", "\u{266F}9", "add9", "alt"];
pub const ELEVENTHS: [&str; 2] = ["\u{2014}", "\u{266F}11"];
pub const THIRTEENTHS: [&str; 2] = ["\u{2014}", "\u{266D}13"];
pub const FIFTHS: [&str; 3] = ["\u{2014}", "\u{266D}5", "\u{266F}5"];

/// The rows of the dialogue, in the order they are drawn: what each one is
/// called and how many values it has.
pub const CHORD_ROWS: [(&str, usize); 7] = [
    ("FAMILY", FAMILIES.len()),
    ("6 / 7", SEVENTHS.len()),
    ("TENSION", TENSIONS.len()),
    ("9", NINTHS.len()),
    ("11", ELEVENTHS.len()),
    ("13", THIRTEENTHS.len()),
    ("5", FIFTHS.len()),
];

impl ChordSpec {
    /// The value each row is on, as it is drawn.
    fn value(&self, row: usize) -> &'static str {
        match row {
            0 => FAMILIES[self.family % FAMILIES.len()],
            1 => SEVENTHS[self.seventh % SEVENTHS.len()],
            2 => TENSIONS[self.tension % TENSIONS.len()],
            3 => NINTHS[self.ninth % NINTHS.len()],
            4 => ELEVENTHS[self.eleventh % ELEVENTHS.len()],
            5 => THIRTEENTHS[self.thirteenth % THIRTEENTHS.len()],
            _ => FIFTHS[self.fifth % FIFTHS.len()],
        }
    }

    fn slot_mut(&mut self, row: usize) -> (&mut usize, usize) {
        match row {
            0 => (&mut self.family, FAMILIES.len()),
            1 => (&mut self.seventh, SEVENTHS.len()),
            2 => (&mut self.tension, TENSIONS.len()),
            3 => (&mut self.ninth, NINTHS.len()),
            4 => (&mut self.eleventh, ELEVENTHS.len()),
            5 => (&mut self.thirteenth, THIRTEENTHS.len()),
            _ => (&mut self.fifth, FIFTHS.len()),
        }
    }

    /// Turn one row by `delta`, wrapping: every row is a short list, and a list
    /// that stops at its ends makes the last value the hardest to reach.
    pub fn step(&mut self, row: usize, delta: isize) {
        let (slot, len) = self.slot_mut(row.min(CHORD_ROWS.len() - 1));
        *slot = (*slot as isize + delta).rem_euclid(len as isize) as usize;
    }

    /// The rows as the list draws them: the name, then the value.
    pub fn rows(&self) -> Vec<String> {
        CHORD_ROWS
            .iter()
            .enumerate()
            .map(|(i, (name, _))| format!("{name:<10}{}", self.value(i)))
            .collect()
    }

    /// What is written after the root: `m9b5`, `7sus4`, `dim9`, `m(maj7)`.
    pub fn quality(&self) -> String {
        let family = self.family % FAMILIES.len();
        let seventh = self.seventh % SEVENTHS.len();
        let tension = self.tension % TENSIONS.len();
        // How high it reaches, as the number a symbol carries.
        let number = match (tension, seventh) {
            (1, _) => 9,
            (2, _) => 11,
            (3, _) => 13,
            (_, 2 | 3) => 7,
            (_, 1) => 6,
            _ => 0,
        };
        let major_seventh = seventh == 3;
        let mut out = String::new();
        match family {
            // Major: the `maj` is what says the seventh is a major one, so it
            // only goes on when there is a seventh at all.
            0 => {
                if number >= 7 {
                    out.push_str("maj");
                }
            }
            1 => out.push('m'),
            // Dominant: no letters, and a number of seven or more is what makes
            // it one. With nothing above the triad there is no dominant to
            // write, so it is the plain seventh.
            2 => {}
            3 => out.push_str("dim"),
            4 => out.push_str("aug"),
            _ => {}
        }
        // The minor-major is the one seventh the letters do not already say.
        let number = match (family, number) {
            (2, 0) => 7,
            _ => number,
        };
        if major_seventh && family != 0 {
            out.push_str("(maj7)");
            if number > 7 {
                out.push_str(&number.to_string());
            }
        } else if number > 0 {
            out.push_str(&number.to_string());
        }
        if family == 5 {
            out.push_str("sus2");
        }
        if family == 6 {
            out.push_str("sus4");
        }
        // The alterations, in the order a symbol writes them.
        out.push_str(match self.fifth % FIFTHS.len() {
            1 => "b5",
            2 => "#5",
            _ => "",
        });
        out.push_str(match self.ninth % NINTHS.len() {
            1 => "b9",
            2 => "#9",
            3 => "add9",
            4 => "alt",
            _ => "",
        });
        if self.eleventh % ELEVENTHS.len() == 1 {
            out.push_str("#11");
        }
        if self.thirteenth % THIRTEENTHS.len() == 1 {
            out.push_str("b13");
        }
        out
    }

    /// The whole symbol, with the root spelled as `root_label` gives it —
    /// letters or degrees, whichever the chart is written in.
    pub fn symbol(&self, root_label: &str) -> String {
        format!("{root_label}{}", self.quality())
    }

    /// The notes the chord is made of, spelled by **what they are in it**: the
    /// flat five of a `Cm7b5` is a `Gb` and not an `F#`, whatever the key would
    /// otherwise call that key of the piano.
    ///
    /// `key` is only the fallback, for the notes the chord says nothing special
    /// about. Returns an empty string for a symbol the grammar refuses, which
    /// the dialogue draws as nothing rather than as a wrong chord.
    pub fn notes(&self, root_label: &str, key: u8) -> String {
        let Ok(chord) = super::chord::parse(&self.symbol(root_label), Some(key)) else {
            return String::new();
        };
        chord
            .tones
            .iter()
            .map(|tone| {
                let pc = ((chord.root as i32 + tone).rem_euclid(12)) as u8;
                // A sharp only where the chord *says* sharp: the raised ninth,
                // the sharp eleventh and the augmented fifth. Everything else a
                // chord reaches by flattening is a flat.
                let sharp = match tone.rem_euclid(12) {
                    3 => self.ninth % NINTHS.len() == 2,
                    6 => self.eleventh % ELEVENTHS.len() == 1 && self.fifth % FIFTHS.len() != 1,
                    8 => self.fifth % FIFTHS.len() == 2,
                    _ => false,
                };
                let flat = !sharp && matches!(tone.rem_euclid(12), 1 | 3 | 6 | 8 | 10);
                match flat {
                    true => super::chord::pitch_class_name_flat(pc),
                    false => super::chord::pitch_class_name_in_key(pc, key),
                }
            })
            .collect::<Vec<&str>>()
            .join(" ")
    }

    /// The spec a written symbol came from, by building every combination and
    /// looking for the one that spells it.
    ///
    /// ponytail: a search, not a second parser. Two thousand string builds run
    /// once when a dialogue opens, and a parser written twice is two grammars
    /// that drift apart.
    pub fn of(root: u8, quality: &str) -> Self {
        let mut spec = ChordSpec {
            root,
            ..Default::default()
        };
        for family in 0..FAMILIES.len() {
            for seventh in 0..SEVENTHS.len() {
                for tension in 0..TENSIONS.len() {
                    for ninth in 0..NINTHS.len() {
                        for eleventh in 0..ELEVENTHS.len() {
                            for thirteenth in 0..THIRTEENTHS.len() {
                                for fifth in 0..FIFTHS.len() {
                                    let cand = ChordSpec {
                                        root,
                                        family,
                                        seventh,
                                        tension,
                                        ninth,
                                        eleventh,
                                        thirteenth,
                                        fifth,
                                    };
                                    if cand.quality() == quality {
                                        return cand;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        spec.root = root;
        spec
    }
}

/// How many species there are: every combination of the rows.
pub const SPECIES: usize = FAMILIES.len()
    * SEVENTHS.len()
    * TENSIONS.len()
    * NINTHS.len()
    * ELEVENTHS.len()
    * THIRTEENTHS.len()
    * FIFTHS.len();

/// The most tones above the root a species has (root excluded).
pub const MAX_TONES: usize = 8;

impl ChordSpec {
    /// This species' place in [`ChordSpec::tones`]'s table. The root is not
    /// part of it: a species is the same shape over any root.
    pub fn index(&self) -> usize {
        let clamp = |v: usize, n: usize| v.min(n - 1);
        let mut i = clamp(self.family, FAMILIES.len());
        i = i * SEVENTHS.len() + clamp(self.seventh, SEVENTHS.len());
        i = i * TENSIONS.len() + clamp(self.tension, TENSIONS.len());
        i = i * NINTHS.len() + clamp(self.ninth, NINTHS.len());
        i = i * ELEVENTHS.len() + clamp(self.eleventh, ELEVENTHS.len());
        i = i * THIRTEENTHS.len() + clamp(self.thirteenth, THIRTEENTHS.len());
        i * FIFTHS.len() + clamp(self.fifth, FIFTHS.len())
    }

    /// The notes of this species above its root, in semitones, ascending, the
    /// root itself left out — what the arranger's grammar makes of it.
    ///
    /// **From a table, not the parser**: the harmoniser asks from the audio
    /// thread, and the parser builds strings and vectors. The table is every
    /// species parsed once, the first time anything asks — call [`warm`] from
    /// a thread that may allocate before the audio thread does.
    pub fn tones(&self) -> &'static [i8] {
        let entry = &table()[self.index()];
        &entry.0[..entry.1 as usize]
    }
}

/// Parse every species once, off the audio thread.
pub fn warm() {
    let _ = table();
}

fn table() -> &'static [([i8; MAX_TONES], u8)] {
    static TABLE: std::sync::OnceLock<Vec<([i8; MAX_TONES], u8)>> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let mut out = vec![([0i8; MAX_TONES], 0u8); SPECIES];
        for (i, slot) in out.iter_mut().enumerate() {
            // Undo `index`: the last row varies fastest.
            let mut rest = i;
            let mut take = |n: usize| {
                let v = rest % n;
                rest /= n;
                v
            };
            let fifth = take(FIFTHS.len());
            let thirteenth = take(THIRTEENTHS.len());
            let eleventh = take(ELEVENTHS.len());
            let ninth = take(NINTHS.len());
            let tension = take(TENSIONS.len());
            let seventh = take(SEVENTHS.len());
            let family = take(FAMILIES.len());
            let spec = ChordSpec {
                root: 0,
                family,
                seventh,
                tension,
                ninth,
                eleventh,
                thirteenth,
                fifth,
            };
            if let Ok(chord) = super::chord::parse(&spec.symbol("C"), Some(0)) {
                let mut n = 0;
                for t in chord.tones.iter().filter(|t| **t != 0).take(MAX_TONES) {
                    slot.0[n] = *t as i8;
                    n += 1;
                }
                slot.1 = n as u8;
            }
        }
        out
    })
}

#[cfg(test)]
mod table_tests {
    use super::*;

    /// The table is the parser: every species reads back the same notes,
    /// and `index` is a one-to-one numbering of them.
    #[test]
    fn the_table_is_the_grammar() {
        let maj7 = ChordSpec {
            seventh: 3,
            ..Default::default()
        };
        assert_eq!(maj7.tones(), &[4, 7, 11]);
        let m7b5 = ChordSpec {
            family: 1,
            seventh: 2,
            fifth: 1,
            ..Default::default()
        };
        assert_eq!(m7b5.tones(), &[3, 6, 10], "{}", m7b5.symbol("C"));
        let mut seen = std::collections::HashSet::new();
        for i in 0..SPECIES {
            assert!(seen.insert(i));
        }
        assert!(
            table().iter().all(|(_, n)| *n >= 1),
            "every species has notes"
        );
    }
}
