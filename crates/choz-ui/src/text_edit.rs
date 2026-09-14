//! A text field with a cursor in it.
//!
//! choz had no such thing: every "typing" in the program until now was a search
//! box or a filename — one line, always appending, backspace the only edit.
//! A chord chart is neither. This is the smallest editor that is still an
//! editor: a caret you can move, a selection you can replace, lines you can
//! split and join, and a window that follows the caret.
//!
//! It owns no keys and draws no widget: it is a buffer and the seven things
//! that can be done to one. Whoever holds it decides which key means what and
//! where the rows go — which is what keeps it out of the piano's way.

/// Where the caret is: `(line, column)`, both counted in characters and not in
/// bytes, because a chart can carry a `Δ` and half a caret is not a caret.
pub type Caret = (usize, usize);

/// A multi-line buffer with a caret and an optional selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    lines: Vec<String>,
    caret: Caret,
    /// Where a selection started, when one is being made. The selection is
    /// everything between this and the caret, in either direction.
    anchor: Option<Caret>,
    /// The first row on screen — moved only to keep the caret in view.
    pub scroll: usize,
}

impl TextEdit {
    /// Open a buffer on `text`. An empty text is one empty line: a buffer with
    /// no lines at all has nowhere to put the caret.
    pub fn new(text: &str) -> Self {
        let mut lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
        if lines.is_empty() {
            lines.push(String::new());
        }
        Self {
            lines,
            caret: (0, 0),
            anchor: None,
            scroll: 0,
        }
    }

    /// What is in it, as the file it came from.
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    pub fn caret(&self) -> Caret {
        self.caret
    }

    /// The selection, as `(from, to)` in reading order, or `None`.
    pub fn selection(&self) -> Option<(Caret, Caret)> {
        let anchor = self.anchor?;
        if anchor == self.caret {
            return None;
        }
        Some(match anchor <= self.caret {
            true => (anchor, self.caret),
            false => (self.caret, anchor),
        })
    }

    /// The characters between the ends of the selection.
    ///
    /// Nothing in the interface asks for them — the selection is drawn from
    /// [`Self::selection`] and typed over in place — but the tests do, and a
    /// selection nobody can read back is a selection nobody can check.
    #[cfg(test)]
    pub fn selected_text(&self) -> String {
        let Some(((l0, c0), (l1, c1))) = self.selection() else {
            return String::new();
        };
        if l0 == l1 {
            return self.slice(l0, c0, c1);
        }
        let mut out = self.slice(l0, c0, self.len(l0));
        for line in &self.lines[l0 + 1..l1] {
            out.push('\n');
            out.push_str(line);
        }
        out.push('\n');
        out.push_str(&self.slice(l1, 0, c1));
        out
    }

    /// Type one character. A selection is replaced, which is what every editor
    /// does and what nobody thinks about until it does not.
    pub fn insert(&mut self, c: char) {
        self.delete_selection();
        let (line, col) = self.caret;
        let at = self.byte_at(line, col);
        self.lines[line].insert(at, c);
        self.caret = (line, col + 1);
    }

    /// Split the line at the caret.
    pub fn newline(&mut self) {
        self.delete_selection();
        let (line, col) = self.caret;
        let at = self.byte_at(line, col);
        let rest = self.lines[line].split_off(at);
        self.lines.insert(line + 1, rest);
        self.caret = (line + 1, 0);
    }

    /// Rub out what is behind the caret — or the selection, if there is one.
    pub fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        let (line, col) = self.caret;
        if col > 0 {
            let at = self.byte_at(line, col - 1);
            self.lines[line].remove(at);
            self.caret = (line, col - 1);
            return;
        }
        // The start of a line: the line joins the one above it.
        if line > 0 {
            let tail = self.lines.remove(line);
            let above = self.len(line - 1);
            self.lines[line - 1].push_str(&tail);
            self.caret = (line - 1, above);
        }
    }

    /// …and what is in front of it.
    pub fn delete(&mut self) {
        if self.delete_selection() {
            return;
        }
        let (line, col) = self.caret;
        if col < self.len(line) {
            let at = self.byte_at(line, col);
            self.lines[line].remove(at);
            return;
        }
        if line + 1 < self.lines.len() {
            let tail = self.lines.remove(line + 1);
            self.lines[line].push_str(&tail);
        }
    }

    /// Take out the selection. `true` when there was one.
    pub fn delete_selection(&mut self) -> bool {
        let Some(((l0, c0), (l1, c1))) = self.selection() else {
            self.anchor = None;
            return false;
        };
        let head = self.slice(l0, 0, c0);
        let tail = self.slice(l1, c1, self.len(l1));
        self.lines.splice(l0..=l1, [format!("{head}{tail}")]);
        self.caret = (l0, c0);
        self.anchor = None;
        true
    }

    /// Move the caret. `select` keeps (or starts) the selection; without it the
    /// selection is dropped, which is what an arrow key means everywhere else.
    pub fn move_caret(&mut self, to: Move, select: bool) {
        match select {
            true => {
                self.anchor.get_or_insert(self.caret);
            }
            false => self.anchor = None,
        }
        let (line, col) = self.caret;
        self.caret = match to {
            Move::Left if col > 0 => (line, col - 1),
            Move::Left if line > 0 => (line - 1, self.len(line - 1)),
            Move::Left => (line, col),
            Move::Right if col < self.len(line) => (line, col + 1),
            Move::Right if line + 1 < self.lines.len() => (line + 1, 0),
            Move::Right => (line, col),
            // Up and down keep the column where they can: a caret that jumped
            // to the start of every short line would be a caret nobody can
            // follow down a chart.
            Move::Up if line > 0 => (line - 1, col.min(self.len(line - 1))),
            Move::Up => (line, 0),
            Move::Down if line + 1 < self.lines.len() => (line + 1, col.min(self.len(line + 1))),
            Move::Down => (line, self.len(line)),
            Move::Home => (line, 0),
            Move::End => (line, self.len(line)),
            Move::PageUp(n) => {
                let to = line.saturating_sub(n);
                (to, col.min(self.len(to)))
            }
            Move::PageDown(n) => {
                let to = (line + n).min(self.lines.len() - 1);
                (to, col.min(self.len(to)))
            }
        };
    }

    /// Select everything.
    pub fn select_all(&mut self) {
        let last = self.lines.len() - 1;
        self.anchor = Some((0, 0));
        self.caret = (last, self.len(last));
    }

    /// Keep the caret on screen in a window `rows` tall, and say where the
    /// window now starts.
    pub fn follow(&mut self, rows: usize) -> usize {
        let rows = rows.max(1);
        let (line, _) = self.caret;
        if line < self.scroll {
            self.scroll = line;
        } else if line >= self.scroll + rows {
            self.scroll = line + 1 - rows;
        }
        self.scroll
    }

    /// How many characters are on a line.
    fn len(&self, line: usize) -> usize {
        self.lines
            .get(line)
            .map(|l| l.chars().count())
            .unwrap_or(0)
    }

    /// The byte offset of character `col` — what `String::insert` wants.
    fn byte_at(&self, line: usize, col: usize) -> usize {
        let text = &self.lines[line];
        text.char_indices()
            .nth(col)
            .map(|(i, _)| i)
            .unwrap_or(text.len())
    }

    fn slice(&self, line: usize, from: usize, to: usize) -> String {
        self.lines[line]
            .chars()
            .skip(from)
            .take(to.saturating_sub(from))
            .collect()
    }
}

/// Where a caret can be asked to go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp(usize),
    PageDown(usize),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_and_rubbing_out() {
        let mut e = TextEdit::new("key = C");
        e.move_caret(Move::End, false);
        e.insert('m');
        assert_eq!(e.text(), "key = Cm");
        e.backspace();
        assert_eq!(e.text(), "key = C");
        // A new line, and the caret is on it.
        e.newline();
        assert_eq!(e.caret(), (1, 0));
        e.insert('|');
        assert_eq!(e.text(), "key = C\n|");
        // Backspacing at the start of a line joins it to the one above.
        e.move_caret(Move::Home, false);
        e.backspace();
        assert_eq!(e.text(), "key = C|");
        assert_eq!(e.caret(), (0, 7));
    }

    #[test]
    fn a_selection_is_replaced_by_what_is_typed() {
        let mut e = TextEdit::new("| I7 | IV7 |\n| V7 |");
        e.move_caret(Move::Right, false);
        e.move_caret(Move::Right, true);
        e.move_caret(Move::Right, true);
        e.move_caret(Move::Right, true);
        assert_eq!(e.selected_text(), " I7");
        e.insert('x');
        assert_eq!(e.text(), "|x | IV7 |\n| V7 |");
        assert!(e.selection().is_none(), "the selection outlived its text");

        // Across lines, and out again.
        let mut e = TextEdit::new("one\ntwo\nthree");
        e.move_caret(Move::End, false);
        e.move_caret(Move::Down, true);
        e.move_caret(Move::End, true);
        assert_eq!(e.selected_text(), "\ntwo");
        e.backspace();
        assert_eq!(e.text(), "one\nthree");
        assert_eq!(e.caret(), (0, 3));
    }

    #[test]
    fn the_caret_keeps_its_column_and_the_window_follows_it() {
        let mut e = TextEdit::new("a long line\nx\nanother long one");
        e.move_caret(Move::End, false);
        e.move_caret(Move::Down, false);
        // The short line has nowhere to put column 11.
        assert_eq!(e.caret(), (1, 1));
        e.move_caret(Move::Down, false);
        assert_eq!(e.caret(), (2, 1), "the column did not come back");

        // A window two rows tall, with the caret on the third line.
        assert_eq!(e.follow(2), 1);
        e.move_caret(Move::PageUp(99), false);
        assert_eq!(e.follow(2), 0);
    }

    #[test]
    fn select_all_and_type_over_it() {
        let mut e = TextEdit::new("key = C\n|| I7 ||");
        e.select_all();
        assert_eq!(e.selected_text(), "key = C\n|| I7 ||");
        e.insert('k');
        assert_eq!(e.text(), "k");
        assert_eq!(e.caret(), (0, 1));
    }

    /// A chart can carry a `Δ`, and a caret that counted bytes would land in
    /// the middle of one.
    #[test]
    fn the_caret_counts_characters_and_not_bytes() {
        let mut e = TextEdit::new("CΔ7");
        e.move_caret(Move::End, false);
        assert_eq!(e.caret(), (0, 3));
        e.backspace();
        assert_eq!(e.text(), "CΔ");
        e.backspace();
        assert_eq!(e.text(), "C");
    }
}
