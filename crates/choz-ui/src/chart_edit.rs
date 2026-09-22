//! The chart, as something to click on rather than type into.
//!
//! A progression is bars, and a bar is the slots its grouping gives it: a 4/4
//! counted `2+2` has two slots, counted `1+1+1+1` has four, and a 7/8 counted
//! `3+2+2` has three of different lengths. **One chord a slot, at most** — the
//! grouping is what says how many chords a bar can hold, which is the rule the
//! text could never state.
//!
//! The text is still the one source: this reads it in and writes it back out,
//! so a chart opened from a file, edited with the buttons and saved is the same
//! format it arrived in. The weights go out as `C:3`, which is how
//! [`choz_engine::arranger::chord`] reads a bar that is not divided evenly.

use choz_engine::artifacts::metronome::musical_groupings;

/// **The chord being built**, one row a decision.
///
/// A menu of ready-made symbols cannot say what a chart needs: a minor takes a
/// ♭9 as happily as a dominant does, a diminished takes a ninth, an augmented
/// takes a seventh. Nineteen rows of the common ones left everything else
/// unsayable, and listing every combination is a list of thousands.
///
/// So the dialogue is the *species*: pick the family, whether it has a sixth or
/// a seventh, how high it reaches, and which notes are altered. Every
/// combination writes a symbol [`choz_engine::arranger::chord::parse`] reads —
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
pub const NINTHS: [&str; 4] = ["\u{2014}", "\u{266D}9", "\u{266F}9", "add9"];
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
        let Ok(chord) = crate::arranger::chord::parse(&self.symbol(root_label), Some(key)) else {
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
                    true => crate::arranger::chord::pitch_class_name_flat(pc),
                    false => crate::arranger::chord::pitch_class_name_in_key(pc, key),
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

/// How wide a cell of the grid is. Seven characters holds the symbols a chart
/// is written with — `Cm(maj7)` runs on rather than being cut, because a chord
/// half-drawn is a chord misread.
const CELL: usize = 7;
/// The bar number in front of the cells: `{:>3}` and two spaces.
const NUMBER: usize = 5;
/// A drawn cell is the label, its brackets, and the bar line after it.
const DRAWN: usize = CELL + 2;
const STEP: usize = DRAWN + 1;

/// One slot of a bar: the chord written in it, or the bar line's `-` for a slot
/// that holds whatever came before it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    /// The symbol, as written — `C`, `Am7`, `I7`. `None` is `-`: hold.
    pub sym: Option<String>,
    /// How much of the bar it holds, in the bar's own units. `1` in an evenly
    /// divided bar.
    pub weight: u8,
}

impl Slot {
    fn hold() -> Self {
        Self {
            sym: None,
            weight: 1,
        }
    }

    /// What goes in the text: the symbol and its weight, the weight left off
    /// when the bar is divided evenly.
    fn token(&self, weighted: bool) -> String {
        let sym = self.sym.clone().unwrap_or_else(|| "-".to_string());
        match weighted {
            true => format!("{sym}:{}", self.weight.max(1)),
            false => sym,
        }
    }

    /// What the row shows: the symbol, or a dash for a slot holding the one
    /// before it.
    fn label(&self) -> String {
        self.sym.clone().unwrap_or_else(|| "\u{2014}".to_string())
    }
}

/// One bar: its slots, in the order they are played.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BarEdit {
    pub slots: Vec<Slot>,
}

impl BarEdit {
    /// A bar of one chord, which is what a bar added to the end of a chart is.
    fn new(sym: Option<String>, cells: u8) -> Self {
        Self {
            slots: vec![Slot {
                sym,
                weight: cells.max(1),
            }],
        }
    }

    /// How the bar is grouped, which is the same thing as how long its slots
    /// are.
    pub fn groups(&self) -> Vec<u8> {
        self.slots.iter().map(|s| s.weight.max(1)).collect()
    }

    /// Put the bar into `groups`, keeping the chords that fit: a bar of `C G`
    /// regrouped into four slots is `C G - -`, and back into one is `C`.
    fn regroup(&mut self, groups: &[u8]) {
        let mut slots: Vec<Slot> = Vec::with_capacity(groups.len());
        for (i, g) in groups.iter().enumerate() {
            let sym = self.slots.get(i).and_then(|s| s.sym.clone());
            slots.push(Slot {
                sym,
                weight: (*g).max(1),
            });
        }
        // The first slot of a bar has to say a chord: a bar that opens on a
        // hold is a bar whose chord came from the one before it, which is legal
        // in the text and unreadable in a grid.
        self.slots = slots;
    }

    fn weighted(&self) -> bool {
        self.slots.iter().any(|s| s.weight != 1)
    }

    fn line(&self) -> String {
        let weighted = self.weighted();
        let tokens: Vec<String> = self.slots.iter().map(|s| s.token(weighted)).collect();
        format!("| {} |", tokens.join(" "))
    }
}

/// The chart being edited, and where the cursor is in it.
#[derive(Debug, Clone, Default)]
pub struct ChartEdit {
    pub bars: Vec<BarEdit>,
    /// Which bar the cursor is on, and which slot of it.
    pub bar: usize,
    pub slot: usize,
    /// The band's own grouping when the dialogue was opened, and what `G` sets
    /// it to on the way out. Empty hands the count back to the metronome.
    pub groups: Vec<u8>,
    /// The chart's key, for spelling the chords the pickers offer.
    pub key: u8,
    /// Whether the chart is written in degrees. What the pickers write, so a
    /// chord picked into a chart of degrees is a degree.
    pub roman: bool,
}

impl ChartEdit {
    /// Read a chart. Anything that is not a bar — the `key` and `style` lines,
    /// the section headers, a `form` — is left alone: those are other
    /// dialogues' and they are put back by the caller.
    pub fn parse(bars: &[String]) -> Self {
        let bars: Vec<BarEdit> = bars
            .iter()
            .map(|bar| BarEdit {
                slots: match bar.split_whitespace().map(slot_of).collect::<Vec<Slot>>() {
                    slots if slots.is_empty() => vec![Slot::hold()],
                    slots => slots,
                },
            })
            .collect();
        Self {
            bars,
            ..Default::default()
        }
    }

    /// The chart as text, one bar a line — what the arranger is given.
    pub fn text(&self) -> String {
        self.bars
            .iter()
            .map(BarEdit::line)
            .collect::<Vec<String>>()
            .join("\n")
    }

    /// One row a bar: its number, then a cell a slot with the one under the
    /// cursor in brackets. The cells are a fixed width, so the columns line up
    /// down the grid the way a chart's bar lines do.
    pub fn rows(&self) -> Vec<String> {
        self.bars
            .iter()
            .enumerate()
            .map(|(b, bar)| {
                let cells: Vec<String> = bar
                    .slots
                    .iter()
                    .enumerate()
                    .map(|(s, slot)| {
                        // The weight is shown where it is not one: it is the
                        // length of the cell, and a 3+2+2 bar drawn as three
                        // equal cells is a lie about what is playing.
                        let label = match slot.weight {
                            1 => slot.label(),
                            w => format!("{}\u{00b7}{w}", slot.label()),
                        };
                        match b == self.bar && s == self.slot {
                            true => format!("[{label:^CELL$}]"),
                            false => format!(" {label:^CELL$} "),
                        }
                    })
                    .collect();
                format!(
                    "{:>width$}  {}",
                    b + 1,
                    cells.join("\u{2502}"),
                    width = NUMBER - 2
                )
            })
            .collect()
    }

    /// Which cell of bar `bar` is drawn at `col` characters into its row, or
    /// `None` for the bar number and the space past the last cell.
    ///
    /// The pointer has to reach every chord of a bar, not just the first: a bar
    /// grouped in four is four buttons, and it was one.
    pub fn slot_at(&self, bar: usize, col: usize) -> Option<usize> {
        let slots = self.bars.get(bar)?.slots.len();
        let into = col.checked_sub(NUMBER)?;
        let slot = into / STEP;
        // The bar line between two cells belongs to neither.
        match into % STEP < DRAWN && slot < slots {
            true => Some(slot),
            false => None,
        }
    }

    fn cur(&mut self) -> Option<&mut Slot> {
        self.bars.get_mut(self.bar)?.slots.get_mut(self.slot)
    }

    /// Put a chord in the slot under the cursor. `None` is a hold.
    pub fn set_chord(&mut self, sym: Option<String>) {
        if let Some(slot) = self.cur() {
            slot.sym = sym;
        }
    }

    /// The symbol under the cursor, for the picker to open on.
    pub fn chord(&self) -> Option<&str> {
        self.bars
            .get(self.bar)?
            .slots
            .get(self.slot)?
            .sym
            .as_deref()
    }

    /// Move the cursor by whole slots, walking into the next bar at the ends —
    /// the chart reads left to right, and so does the cursor.
    pub fn step_slot(&mut self, delta: isize) {
        let Some(bar) = self.bars.get(self.bar) else {
            return;
        };
        let next = self.slot as isize + delta;
        if next < 0 {
            if self.bar > 0 {
                self.bar -= 1;
                self.slot = self.bars[self.bar].slots.len().saturating_sub(1);
            }
            return;
        }
        if next as usize >= bar.slots.len() {
            if self.bar + 1 < self.bars.len() {
                self.bar += 1;
                self.slot = 0;
            }
            return;
        }
        self.slot = next as usize;
    }

    /// Which bar the cursor is on, clamped, with the slot brought inside it.
    pub fn go_to_bar(&mut self, bar: usize) {
        self.bar = bar.min(self.bars.len().saturating_sub(1));
        let slots = self
            .bars
            .get(self.bar)
            .map(|b| b.slots.len())
            .unwrap_or(1)
            .max(1);
        self.slot = self.slot.min(slots - 1);
    }

    /// A bar after the one the cursor is on, holding what it holds — a chart
    /// grows a bar at a time and usually by repeating one.
    pub fn add_bar(&mut self, cells: u8) {
        let sym = self.chord().map(|s| s.to_string());
        let at = (self.bar + 1).min(self.bars.len());
        self.bars.insert(at, BarEdit::new(sym, cells));
        self.bar = at;
        self.slot = 0;
    }

    /// Take the bar out. The last one stays: a chart with no bars is not a
    /// chart, and the arranger would refuse the text.
    pub fn remove_bar(&mut self) {
        if self.bars.len() <= 1 {
            return;
        }
        self.bars.remove(self.bar);
        self.go_to_bar(self.bar);
    }

    /// The next grouping this bar can be counted in, out of the ones a musician
    /// writes — the metronome's own list, so the two dialogues offer the same
    /// bars. `cells` is what the bar adds up to: 4 in 4/4, 7 in 7/8.
    pub fn regroup(&mut self, delta: isize, cells: u8) {
        let options = groupings(cells);
        let Some(bar) = self.bars.get_mut(self.bar) else {
            return;
        };
        let now = bar.groups();
        let at = options.iter().position(|g| *g == now).unwrap_or(0) as isize;
        let next = (at + delta).rem_euclid(options.len() as isize) as usize;
        bar.regroup(&options[next]);
        self.slot = self.slot.min(bar.slots.len().saturating_sub(1));
    }

    /// The bar the cursor is on, as a grouping — what `G` hands the band.
    pub fn grouping_of_cursor(&self) -> Vec<u8> {
        self.bars
            .get(self.bar)
            .map(BarEdit::groups)
            .unwrap_or_default()
    }
}

/// `C:3` → the slot it stands for. An unreadable weight is `1`: the grid is
/// opened on text nobody promised was right, and a bar that will not draw is
/// worse than a bar drawn evenly.
fn slot_of(token: &str) -> Slot {
    let (sym, weight) = match token.split_once(':') {
        Some((sym, weight)) => (sym, weight.parse::<f64>().unwrap_or(1.0)),
        None => (token, 1.0),
    };
    let sym = match sym {
        "-" | "%" | "/" | "" => None,
        sym => Some(sym.to_string()),
    };
    Slot {
        sym,
        weight: weight.round().clamp(1.0, 15.0) as u8,
    }
}

/// Every way a bar of `cells` can be grouped, the whole bar first: one chord a
/// bar is the common case and the list opens on it.
///
/// The metronome's own list, plus the two ends it does not need — the whole bar
/// and one cell each — because those are the two a chart is usually written in.
pub fn groupings(cells: u8) -> Vec<Vec<u8>> {
    let cells = cells.clamp(1, 16);
    let mut out = vec![vec![cells]];
    for g in musical_groupings(cells) {
        if !out.contains(&g) {
            out.push(g);
        }
    }
    let ones = vec![1u8; cells as usize];
    if !out.contains(&ones) {
        out.push(ones);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chart(text: &str) -> ChartEdit {
        ChartEdit::parse(&crate::views::fx_chain_panel::chart_bars(text))
    }

    /// The text goes in and comes back out: a bar of one chord, a bar of two,
    /// and a bar with weights on it.
    #[test]
    fn a_chart_round_trips_through_the_grid() {
        let c = chart("|| C | C G | Cm:3 F:2 Bb:2 ||");
        assert_eq!(c.bars.len(), 3);
        assert_eq!(c.bars[0].groups(), vec![1]);
        assert_eq!(c.bars[1].groups(), vec![1, 1]);
        assert_eq!(c.bars[2].groups(), vec![3, 2, 2]);
        assert_eq!(c.text(), "| C |\n| C G |\n| Cm:3 F:2 Bb:2 |");
        // And what comes out is what the arranger reads.
        let prog = crate::arranger::chord::parse_progression(&c.text()).unwrap();
        assert_eq!(prog.bars.len(), 3);
        assert_eq!(prog.bars[2].groups(), vec![3, 2, 2]);
    }

    /// A hold is a slot like any other: it keeps the chord before it, and it
    /// survives the round trip as the `-` the parser knows.
    #[test]
    fn a_held_slot_is_a_dash() {
        let mut c = chart("|| C - | C ||");
        assert_eq!(c.bars[0].slots[1].sym, None);
        c.bar = 1;
        c.slot = 0;
        c.set_chord(None);
        // The first bar's chord is what a leading hold means, and the text says
        // so rather than the grid inventing one.
        assert_eq!(c.text(), "| C - |\n| - |");
    }

    /// The grouping is what says how many chords a bar can hold: regrouping a
    /// bar keeps the chords that still have a slot and gives the new ones a
    /// hold.
    #[test]
    fn regrouping_a_bar_is_how_many_chords_it_can_hold() {
        let mut c = chart("|| C ||");
        assert_eq!(c.bars[0].groups(), vec![1], "one chord to start");
        // 4/4: the whole bar, then the groupings, then one a beat.
        let options = groupings(4);
        assert_eq!(options.first(), Some(&vec![4]));
        assert_eq!(options.last(), Some(&vec![1, 1, 1, 1]));
        assert!(options.contains(&vec![2, 2]));

        c.regroup(1, 4);
        assert_eq!(c.bars[0].slots.len(), options[1].len());
        // Every step is a grouping of the same bar: the weights add up to four
        // however it is counted.
        for _ in 0..options.len() {
            let sum: u8 = c.bars[0].groups().iter().sum();
            assert_eq!(sum, 4, "a bar of 4/4 stopped adding up to four");
            c.regroup(1, 4);
        }
        // A 7/8 is grouped the way the metronome groups one.
        let mut seven = chart("|| Cm ||");
        seven.regroup(1, 7);
        assert_eq!(seven.bars[0].groups().iter().sum::<u8>(), 7);
    }

    /// **Every species the dialogue can build is a chord the arranger reads.**
    /// Two thousand combinations, each one parsed — the rows and the grammar
    /// cannot drift apart while this passes.
    #[test]
    fn every_chord_the_dialogue_builds_parses() {
        let mut seen = std::collections::HashSet::new();
        for family in 0..FAMILIES.len() {
            for seventh in 0..SEVENTHS.len() {
                for tension in 0..TENSIONS.len() {
                    for ninth in 0..NINTHS.len() {
                        for eleventh in 0..ELEVENTHS.len() {
                            for thirteenth in 0..THIRTEENTHS.len() {
                                for fifth in 0..FIFTHS.len() {
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
                                    let sym = spec.symbol("C");
                                    let chord = crate::arranger::chord::parse(&sym, Some(0))
                                        .unwrap_or_else(|e| {
                                            panic!("{sym} is a row of the dialogue: {e}")
                                        });
                                    // Whatever it spells, it is a chord with
                                    // notes in it — not an empty symbol.
                                    assert!(chord.tones.len() >= 2, "{sym} is not a chord");
                                    seen.insert(sym);
                                }
                            }
                        }
                    }
                }
            }
        }
        assert!(seen.len() > 500, "only {} species?", seen.len());
    }

    /// The tensions belong to every family, which is the whole point of the
    /// rows: a minor takes a ♭9, a diminished takes a ninth, an augmented takes
    /// a seventh.
    #[test]
    fn any_family_takes_any_tension() {
        let spec = |family: usize, seventh: usize, tension: usize, ninth: usize| ChordSpec {
            root: 0,
            family,
            seventh,
            tension,
            ninth,
            ..Default::default()
        };
        // MIN with a flat nine, which the old menu could not say at all.
        let m = spec(1, 2, 0, 1);
        assert_eq!(m.quality(), "m7b9");
        let notes = crate::arranger::chord::parse(&m.symbol("C"), Some(0)).unwrap();
        assert_eq!(notes.pitch_classes(), vec![0, 3, 7, 10, 1], "C Eb G Bb Db");
        // MIN reaching the thirteenth.
        assert_eq!(spec(1, 0, 3, 0).quality(), "m13");
        // DIM with a ninth, AUG with a seventh.
        assert_eq!(spec(3, 0, 1, 0).quality(), "dim9");
        assert_eq!(spec(4, 2, 0, 0).quality(), "aug7");
        // The minor-major, and the same seventh over an augmented triad.
        assert_eq!(spec(1, 3, 0, 0).quality(), "m(maj7)");
        assert_eq!(spec(4, 3, 0, 0).quality(), "aug(maj7)");
        // A plain major says nothing, and its seventh is a major seventh.
        assert_eq!(spec(0, 0, 0, 0).quality(), "");
        assert_eq!(spec(0, 2, 0, 0).quality(), "maj7");
        assert_eq!(spec(0, 0, 1, 0).quality(), "maj9");
        // A dominant is the one family that is a seventh: with nothing above
        // the triad it is still a seventh.
        assert_eq!(spec(2, 0, 0, 0).quality(), "7");
        assert_eq!(spec(2, 0, 1, 0).quality(), "9");
        // Suspensions carry their number the way a chart writes them.
        assert_eq!(spec(6, 2, 0, 0).quality(), "7sus4");
    }

    /// The dialogue opens on what is already written: a chord being changed
    /// keeps everything about it that is not being changed.
    #[test]
    fn the_dialogue_opens_on_the_chord_that_is_there() {
        for sym in [
            "Cm7b5", "C7#9", "Cm(maj7)", "Cdim9", "C7sus4", "C", "Cmaj13",
        ] {
            let (root, quality) = crate::arranger::chord::root_and_quality(sym, 0).expect(sym);
            let spec = ChordSpec::of(root, &quality);
            assert_eq!(spec.symbol("C"), sym, "{sym} did not come back");
        }
    }

    /// The pointer reaches every chord of a bar: a bar grouped in four is four
    /// buttons, drawn where `rows` draws them.
    #[test]
    fn a_click_lands_on_the_cell_it_is_over() {
        let mut c = chart("|| C G Am F ||");
        assert_eq!(c.bars[0].slots.len(), 4);
        let row = c.rows()[0].clone();
        // Every cell of the drawn row answers for itself, and the label is
        // inside the cell the column says it is.
        for slot in 0..4 {
            let label = ["C", "G", "Am", "F"][slot];
            // Columns, not bytes: the bar line between cells is one column and
            // three bytes, and it is columns the pointer lands in.
            let at = row
                .chars()
                .collect::<Vec<char>>()
                .windows(label.chars().count())
                .position(|w| w.iter().collect::<String>() == label)
                .expect("the chord is not drawn");
            assert_eq!(
                c.slot_at(0, at),
                Some(slot),
                "column {at} of {row:?} is not slot {slot}"
            );
        }
        // The bar number is not a cell, and neither is the space past the end.
        assert_eq!(c.slot_at(0, 0), None);
        assert_eq!(c.slot_at(0, row.chars().count() + 4), None);
        // A bar with one chord has one cell, however wide the row next to it is.
        c.bars.push(BarEdit::new(Some("C".into()), 4));
        assert_eq!(c.slot_at(1, 6), Some(0));
        assert_eq!(c.slot_at(1, 20), None);
    }

    /// The cursor reads left to right and walks into the next bar at the end of
    /// one, and a bar taken out never leaves the cursor pointing past the end.
    #[test]
    fn the_cursor_walks_the_chart() {
        let mut c = chart("|| C G | Am F ||");
        assert_eq!((c.bar, c.slot), (0, 0));
        c.step_slot(1);
        assert_eq!((c.bar, c.slot), (0, 1));
        c.step_slot(1);
        assert_eq!((c.bar, c.slot), (1, 0), "into the next bar");
        c.step_slot(-1);
        assert_eq!((c.bar, c.slot), (0, 1), "and back into the last slot");

        c.go_to_bar(1);
        c.add_bar(4);
        assert_eq!(c.bars.len(), 3);
        assert_eq!(c.bar, 2, "the cursor follows the bar that was added");
        c.remove_bar();
        c.remove_bar();
        c.remove_bar();
        assert_eq!(c.bars.len(), 1, "the last bar stays");
        assert!(c.bar < c.bars.len());
    }
}
