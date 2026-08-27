//! One list-modal widget shared by every picker in the app.
//!
//! Every modal that shows a list (source, ADD FX, output device, SF2 presets,
//! MIDI learn, file browser) draws through [`draw_list_modal`], so they all get
//! the same scrollbar, filter chips, SELECT/CANCEL buttons and hit rects — and
//! the mouse handling only has to know about one set of rects.

use ratatui::{
    layout::{Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
    Frame,
};

use super::theme::*;
use crate::i18n::t;

/// A scrollable list with an optional filter bar. Owns cursor/scroll state; the
/// items are plain strings so every caller can build them however it likes.
#[derive(Default, Clone)]
pub struct ListModal {
    pub title: String,
    /// Filter chips shown above the list. Empty = no filter bar.
    pub filters: Vec<String>,
    /// A colour per chip, when the chips stand for things that have one — the
    /// split's sound banks, which are the same colours the octaves are drawn
    /// in. Empty (the usual) draws them all in the panel's own colours.
    pub filter_colours: Vec<Option<Color>>,
    pub filter: usize,
    pub items: Vec<String>,
    /// A colour per row, when the list is about things that have one — the
    /// keyboard split's sounds. Empty (the usual) draws every row in the
    /// panel's own text colour.
    pub colours: Vec<Option<Color>>,
    pub cursor: usize,
    pub scroll: usize,
    /// Extra line above the buttons (e.g. the current directory).
    pub note: String,
    /// Extra buttons on the button row: `(label, the key they stand for)`.
    /// Clicking one is the same as pressing that key, so mouse and keyboard
    /// share a single handler.
    pub actions: Vec<(String, char)>,
    /// An image to show beside the list — the file under the cursor, for a
    /// picker whose rows are pictures. A name is not what anyone is choosing a
    /// wallpaper by.
    pub preview: Option<std::path::PathBuf>,
    /// Left sidebar sections: `(label, how many entries are in it)`. Empty =
    /// no sidebar, and the list takes the full width.
    pub sidebar: Vec<(String, usize)>,
    pub sidebar_cursor: usize,
    /// Whether the arrows drive the sidebar rather than the list.
    pub sidebar_focused: bool,
}

impl ListModal {
    pub fn new(title: impl Into<String>, items: Vec<String>) -> Self {
        Self {
            title: title.into(),
            items,
            ..Default::default()
        }
    }

    pub fn with_filters(mut self, filters: &[&str]) -> Self {
        self.filters = filters.iter().map(|s| s.to_string()).collect();
        self
    }

    pub fn move_cursor(&mut self, delta: isize) {
        let last = self.items.len().saturating_sub(1);
        self.cursor = (self.cursor as isize + delta).clamp(0, last as isize) as usize;
    }

    /// Move the sidebar cursor, wrapping at neither end.
    pub fn move_section(&mut self, delta: isize) {
        let last = self.sidebar.len().saturating_sub(1);
        self.sidebar_cursor =
            (self.sidebar_cursor as isize + delta).clamp(0, last as isize) as usize;
    }

    /// Put the cursor where a click at `row` inside the scrollbar `track` points.
    /// The fraction runs over the whole list, so the top of the track is the
    /// first item and the bottom is the last, whatever the viewport height.
    pub fn drag_to(&mut self, track: Rect, row: u16) {
        let last = self.items.len().saturating_sub(1);
        if last == 0 || track.height == 0 {
            return;
        }
        let offset = row.saturating_sub(track.y).min(track.height - 1) as usize;
        // Round to nearest so the bottom cell of the track reaches the last
        // item instead of stopping one short.
        self.cursor =
            (offset * last + (track.height as usize - 1) / 2) / (track.height as usize - 1).max(1);
        self.cursor = self.cursor.min(last);
    }

    pub fn cycle_filter(&mut self, delta: isize) {
        if self.filters.is_empty() {
            return;
        }
        let n = self.filters.len() as isize;
        self.filter = (((self.filter as isize + delta) % n + n) % n) as usize;
        self.cursor = 0;
        self.scroll = 0;
    }
}

/// Draw an image into `area`, two pixel rows to a cell.
///
/// Half-blocks rather than a graphics protocol: this has to look the same on
/// the terminal the rest of choz is drawn in, and a picker is not the place to
/// find out whether the terminal has kitty's protocol.
fn draw_preview(f: &mut Frame, area: Rect, path: &std::path::Path) {
    let Some(cells) = super::background::thumbnail(path, area.width, area.height) else {
        // Not an image, or not readable: say so rather than leaving a hole the
        // user reads as a broken panel.
        f.render_widget(
            Paragraph::new(Span::styled(
                format!(" {} ", t("NO PREVIEW")),
                Style::default().fg(HINT),
            )),
            Rect::new(area.x, area.y + area.height / 2, area.width, 1),
        );
        return;
    };
    for row in 0..area.height {
        let spans: Vec<Span> = (0..area.width)
            .map(|col| {
                let i = row as usize * area.width as usize + col as usize;
                // The thumbnail is built to this size, so this is belt and
                // braces — but it is drawn from a file, and a panic here is a
                // dead interface.
                let (top, bottom) = cells.get(i).copied().unwrap_or_default();
                Span::styled(
                    "\u{2580}".to_string(),
                    Style::default()
                        .fg(Color::Rgb(top.0, top.1, top.2))
                        .bg(Color::Rgb(bottom.0, bottom.1, bottom.2)),
                )
            })
            .collect();
        f.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect::new(area.x, area.y + row, area.width, 1),
        );
    }
}

/// Clickable areas of the last drawn modal.
#[derive(Default, Clone)]
pub struct ModalRects {
    pub area: Option<Rect>,
    /// (item index, rect) for the rows currently on screen.
    pub rows: Vec<(usize, Rect)>,
    /// (filter index, rect).
    pub filters: Vec<(usize, Rect)>,
    pub select: Option<Rect>,
    pub cancel: Option<Rect>,
    /// (key the button stands for, rect) for [`ListModal::actions`].
    pub actions: Vec<(char, Rect)>,
    /// (section index, rect) for the sidebar rows.
    pub sidebar: Vec<(usize, Rect)>,
    /// The list body — wheel events inside it scroll the list.
    pub list: Option<Rect>,
    /// The scrollbar **track**, arrow heads excluded, so a click maps straight
    /// onto a scroll fraction. `None` when the list fits and no bar is drawn.
    pub scrollbar: Option<Rect>,
}

/// Truncate to `max` chars.
fn trunc(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

/// What the SPLIT dialogue is looking at.
///
/// The keyboard is set by pointing at it: the left button paints the chosen
/// sound onto the octave under the pointer, the right button takes it off. It
/// is the KEYS tab's own piano — same helper, same geometry — because a split
/// is a statement about *this* keyboard, and a second drawing of it would put
/// the zones somewhere the player cannot see them.
pub struct SplitView<'a> {
    /// Which sound each octave plays, indexed as `RackSlot::octave_sound` is:
    /// entry `i` is the octave whose C is `C(i - 1)`.
    pub octaves: &'a [Option<usize>],
    /// One entry per sound button: its label and the colour it paints with.
    pub sounds: &'a [(String, Color)],
    /// The button the pointer paints with.
    pub chosen: usize,
    /// Whether `+` can still add one.
    pub can_add: bool,
    /// The dialogue's own name: SPLIT, or the lane whose note is being picked.
    pub title: String,
    /// The line under the keyboard that says what clicking it does.
    pub hint: String,
    /// One key drawn as the chosen one — for the pickers that are choosing a
    /// **note** rather than painting a zone, where nothing else on the keyboard
    /// says where the setting currently sits.
    pub highlight: Option<u8>,
    /// Draw SELECT and CANCEL. SPLIT does not: it paints, and there is nothing
    /// to take back. A picker does, because a note tried by ear has to be
    /// possible to try and then not keep.
    pub buttons: bool,
}

/// Where the SPLIT dialogue put things, for the mouse.
#[derive(Default, Clone)]
pub struct SplitRects {
    pub area: Option<Rect>,
    /// `(sound index, rect)` for the coloured squares.
    pub chips: Vec<(usize, Rect)>,
    pub add: Option<Rect>,
    /// The piano. A click resolves to a note, and the note to its octave.
    pub keys: crate::views::midi_monitor::KeyMap,
    /// Keep what was picked, and put back what was there. Drawn only for the
    /// pickers that have something to take back — see [`SplitView::buttons`].
    pub select: Option<Rect>,
    pub cancel: Option<Rect>,
}

/// The octave index a note belongs to — the same indexing `octaves` uses.
pub fn octave_of(note: u8) -> usize {
    note as usize / 12
}

pub fn draw_split_modal(f: &mut Frame, area: Rect, v: SplitView) -> SplitRects {
    f.render_widget(Clear, area);
    f.render_widget(Block::default().style(Style::default().bg(BACKDROP)), area);

    // Wide and short. The height is what the content needs and not a share of
    // the screen: a percentage gave a piano nine rows of black keys over two of
    // white, which is a wall, not a keyboard.
    const PIANO_ROWS: u16 = 6;
    let h = (3 + PIANO_ROWS + 1 + 2).min(area.height);
    let w = (area.width * 92 / 100).max(24).min(area.width);
    let popup = Rect {
        x: area.x + area.width.saturating_sub(w) / 2,
        y: area.y + area.height.saturating_sub(h) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, popup);
    let block = Block::default()
        .title(format!(" {} ", v.title))
        .title_style(Style::default().fg(HEADER).add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border()))
        .style(super::theme::overlay_style());
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let mut rects = SplitRects {
        area: Some(popup),
        ..Default::default()
    };
    let content = inner.inner(Margin {
        vertical: 0,
        horizontal: 1,
    });
    if content.height < 4 || content.width < 20 {
        return rects;
    }

    // ─── The sound buttons, as the squares they paint with ─────────────
    let mut x = content.x;
    let y = content.y;
    for (i, (name, colour)) in v.sounds.iter().enumerate() {
        let label = format!(" \u{25A0} {} {name} ", i + 1);
        let w = label.chars().count() as u16;
        if x + w + 4 > content.x + content.width {
            break;
        }
        let rect = Rect::new(x, y, w, 1);
        // The chosen one is the colour; the rest wear it on the square alone,
        // so the row says which sound the pointer is holding.
        let st = match i == v.chosen {
            true => Style::default()
                .fg(Color::Black)
                .bg(*colour)
                .add_modifier(Modifier::BOLD),
            false => Style::default().fg(*colour).bg(PANEL_BG),
        };
        f.render_widget(Paragraph::new(Span::styled(label, st)), rect);
        rects.chips.push((i, rect));
        x += w + 1;
    }
    if v.can_add && x + 3 <= content.x + content.width {
        let rect = Rect::new(x, y, 3, 1);
        f.render_widget(
            Paragraph::new(Span::styled(
                " + ",
                Style::default().fg(OK).add_modifier(Modifier::BOLD),
            )),
            rect,
        );
        rects.add = Some(rect);
    }

    // ─── The piano, painted by which sound each octave plays ───────────
    let note = v.hint.as_str();
    let rows = content.height.saturating_sub(3).clamp(2, PIANO_ROWS);
    let paint = |n: u8| -> Option<Color> {
        // The chosen note outranks the zones: on a picker there are none, and
        // on one that has them the key being set is the thing to see.
        if v.highlight == Some(n) {
            return Some(HEADER); // the chosen note wears the accent every heading here does
        }
        v.octaves
            .get(octave_of(n))
            .copied()
            .flatten()
            .and_then(|i| v.sounds.get(i))
            .map(|(_, c)| *c)
    };
    let (lines, mut keys) =
        crate::views::midi_monitor::full_piano(content.width as usize, rows as usize, &paint);
    // Centred on the box: the keys are a whole number of cells wide, so what is
    // left over is a margin, and a keyboard pushed against the left border with
    // a gap on the right reads as a drawing that ran out of room.
    let drawn = keys.drawn();
    let x0 = content.x + content.width.saturating_sub(drawn) / 2;
    let key_rows = lines.len().saturating_sub(usize::from(rows >= 3)) as u16;
    let top = y + 2;
    keys.area = Rect::new(x0, top, drawn, key_rows);
    f.render_widget(
        Paragraph::new(lines).style(super::theme::overlay_style()),
        Rect::new(x0, top, drawn, rows),
    );
    rects.keys = keys;

    // The hint and the buttons share the last row: the box is sized to the
    // keyboard, and a row of its own for two buttons would cost a row of keys.
    let last = content.y + content.height - 1;
    let mut hint_x = content.x;
    if v.buttons {
        let select = Rect::new(content.x, last, 10, 1);
        let cancel = Rect::new(content.x + 11, last, 10, 1);
        f.render_widget(
            Paragraph::new(Span::styled(
                format!("  {}  ", t("SELECT")),
                Style::default()
                    .fg(Color::Black)
                    .bg(OK)
                    .add_modifier(Modifier::BOLD),
            )),
            select,
        );
        f.render_widget(
            Paragraph::new(Span::styled(
                format!("  {}  ", t("CANCEL")),
                Style::default()
                    .fg(Color::White)
                    .bg(ERR)
                    .add_modifier(Modifier::BOLD),
            )),
            cancel,
        );
        hint_x = cancel.x + cancel.width + 2;
        rects.select = Some(select);
        rects.cancel = Some(cancel);
    }
    if hint_x < content.x + content.width {
        f.render_widget(
            Paragraph::new(Span::styled(note, Style::default().fg(HINT))),
            Rect::new(hint_x, last, content.x + content.width - hint_x, 1),
        );
    }
    rects
}

fn centered(pct_x: u16, pct_y: u16, area: Rect) -> Rect {
    let w = area.width * pct_x / 100;
    let h = area.height * pct_y / 100;
    Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w.max(20).min(area.width),
        height: h.max(7).min(area.height),
    }
}

/// Draw `m` centred over `area`. Returns the rects to hit-test, and writes the
/// scroll position back into `m` so the keyboard and the mouse agree on it.
pub fn draw_list_modal(
    f: &mut Frame,
    m: &mut ListModal,
    area: Rect,
    pct: (u16, u16),
) -> ModalRects {
    f.render_widget(Clear, area);
    f.render_widget(Block::default().style(Style::default().bg(BACKDROP)), area);

    let popup = centered(pct.0, pct.1, area);
    f.render_widget(Clear, popup);
    let block = Block::default()
        .title(format!(" {} ", m.title))
        .title_style(Style::default().fg(HEADER).add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border()))
        .style(super::theme::overlay_style());
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let mut rects = ModalRects {
        area: Some(popup),
        ..Default::default()
    };
    let content = inner.inner(Margin {
        vertical: 0,
        horizontal: 1,
    });
    if content.height == 0 || content.width == 0 {
        return rects;
    }

    // Rows, top to bottom: [filters] list [note] buttons.
    let mut y = content.y;
    let bottom = content.y + content.height;
    if !m.filters.is_empty() {
        let mut x = content.x;
        for (i, name) in m.filters.iter().enumerate() {
            let label = format!(" {name} ");
            let w = label.chars().count() as u16;
            if x + w > content.x + content.width {
                break;
            }
            let rect = Rect::new(x, y, w, 1);
            rects.filters.push((i, rect));
            let own = m.filter_colours.get(i).copied().flatten();
            let st = if i == m.filter {
                Style::default()
                    .fg(Color::Black)
                    .bg(own.unwrap_or(ACCENT))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(own.unwrap_or(HINT)).bg(PANEL_BG)
            };
            f.render_widget(Paragraph::new(Span::styled(label, st)), rect);
            x += w + 1;
        }
        y += 1;
    }

    // Buttons always sit on the last row, the note (if any) just above it.
    let btn_y = bottom.saturating_sub(1);
    let note_y = btn_y.saturating_sub(1);
    let list_bottom = if m.note.is_empty() {
        note_y
    } else {
        note_y.saturating_sub(1)
    };
    let rows = list_bottom.saturating_sub(y) as usize;

    // Keep the cursor visible.
    if m.cursor < m.scroll {
        m.scroll = m.cursor;
    } else if rows > 0 && m.cursor >= m.scroll + rows {
        m.scroll = m.cursor + 1 - rows;
    }

    // Sidebar down the left, when there is one.
    let mut list_x = content.x;
    let mut list_w = content.width;
    if !m.sidebar.is_empty() && content.width > 24 {
        let sw = 18u16.min(content.width / 3);
        for (i, (label, count)) in m.sidebar.iter().enumerate() {
            if i >= rows {
                break;
            }
            let rect = Rect::new(content.x, y + i as u16, sw, 1);
            rects.sidebar.push((i, rect));
            let sel = i == m.sidebar_cursor;
            let (bg, fg) = match (sel, m.sidebar_focused) {
                (true, true) => (ACCENT, Color::Black),
                (true, false) => (Color::Rgb(30, 38, 50), WARN),
                _ => (PANEL_BG, HINT),
            };
            let text = format!(
                " {:<w$}{count:>3} ",
                trunc(label, sw as usize - 5),
                w = sw as usize - 5
            );
            f.render_widget(
                Paragraph::new(Span::styled(
                    text,
                    Style::default().fg(fg).add_modifier(if sel {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
                ))
                .style(Style::default().bg(bg)),
                rect,
            );
        }
        // Fill the rest of the sidebar column so it reads as one panel.
        for row in m.sidebar.len()..rows {
            f.render_widget(
                Block::default().style(super::theme::panel_style()),
                Rect::new(content.x, y + row as u16, sw, 1),
            );
        }
        list_x += sw + 1;
        list_w = list_w.saturating_sub(sw + 1);
    }

    // A picture beside the list, for a picker whose rows are pictures. It takes
    // the right-hand third and the list keeps the rest; on a panel too narrow
    // for both, the list wins — a preview that leaves no room to read the names
    // is a preview of the wrong thing.
    if let Some(path) = m.preview.clone() {
        let pw = (list_w / 2).min(34);
        if pw >= 12 && rows >= 4 {
            let area = Rect::new(list_x + list_w - pw, y, pw, rows as u16);
            draw_preview(f, area, &path);
            list_w = list_w.saturating_sub(pw + 1);
        }
    }

    let list_area = Rect::new(list_x, y, list_w.saturating_sub(1), rows as u16);
    rects.list = Some(list_area);
    if m.items.is_empty() && rows > 0 {
        f.render_widget(
            Paragraph::new(Span::styled("  (empty)", Style::default().fg(DIM)))
                .style(super::theme::panel_style()),
            list_area,
        );
    }
    for (i, label) in m.items.iter().enumerate().skip(m.scroll).take(rows) {
        let rect = Rect::new(list_area.x, y + (i - m.scroll) as u16, list_area.width, 1);
        rects.rows.push((i, rect));
        let sel = i == m.cursor;
        let (bg, fg) = if sel {
            (ACCENT, Color::Black)
        } else {
            (
                PANEL_BG,
                m.colours.get(i).copied().flatten().unwrap_or_else(text),
            )
        };
        let text = format!("{}{}", if sel { "\u{25B6} " } else { "  " }, label);
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                text,
                Style::default().fg(fg).add_modifier(if sel {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
            )))
            .style(Style::default().bg(bg)),
            rect,
        );
    }

    // Scrollbar: only when the list doesn't fit.
    if m.items.len() > rows && rows > 0 {
        let sb = Rect::new(list_area.x + list_area.width, y, 1, rows as u16);
        // `content_length` is the number of scroll *positions*, not the number
        // of items: ratatui clamps the thumb to `content_length - 1`, so
        // passing `items.len()` makes the thumb both too short and unable to
        // reach the bottom of the track. The last scroll offset is
        // `items.len() - rows`, so there are one more than that many of them.
        let mut state = ScrollbarState::new(m.items.len() - rows + 1)
            .viewport_content_length(rows)
            .position(m.scroll);
        // The track the thumb actually moves in, which is the bar minus the two
        // arrow heads. Hit-testing anything wider makes a drag jump.
        if rows > 2 {
            rects.scrollbar = Some(Rect::new(sb.x, sb.y + 1, 1, sb.height - 2));
        }
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(Some("\u{25B2}"))
                .end_symbol(Some("\u{25BC}"))
                .thumb_symbol("\u{2588}")
                .track_symbol(Some("\u{2502}")),
            sb,
            &mut state,
        );
    }

    if !m.note.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled(m.note.clone(), Style::default().fg(DIM)))
                .style(super::theme::panel_style()),
            Rect::new(content.x, note_y, content.width, 1),
        );
    }

    // SELECT / CANCEL buttons + the keyboard hint on the same row.
    let select = Rect::new(content.x, btn_y, 10, 1);
    let cancel = Rect::new(content.x + 11, btn_y, 10, 1);
    f.render_widget(
        Paragraph::new(Span::styled(
            format!("  {}  ", t("SELECT")),
            Style::default()
                .fg(Color::Black)
                .bg(OK)
                .add_modifier(Modifier::BOLD),
        )),
        select,
    );
    f.render_widget(
        Paragraph::new(Span::styled(
            format!("  {}  ", t("CANCEL")),
            Style::default()
                .fg(Color::White)
                .bg(ERR)
                .add_modifier(Modifier::BOLD),
        )),
        cancel,
    );
    // Modal-specific buttons (EDIT / ADD / …) follow CANCEL.
    let mut bx = cancel.x + cancel.width + 2;
    for (label, key) in m.actions.iter() {
        let text = format!(" {label} ");
        let w = text.chars().count() as u16;
        if bx + w >= content.x + content.width {
            break;
        }
        let rect = Rect::new(bx, btn_y, w, 1);
        rects.actions.push((*key, rect));
        f.render_widget(
            Paragraph::new(Span::styled(
                text,
                Style::default()
                    .fg(Color::Black)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD),
            )),
            rect,
        );
        bx += w + 1;
    }

    let hint_x = if rects.actions.is_empty() {
        cancel.x + cancel.width + 2
    } else {
        bx + 1
    };
    if hint_x < content.x + content.width {
        f.render_widget(
            Paragraph::new(Span::styled(
                "\u{2191}\u{2193}=move  wheel=scroll  Enter=select  Esc=cancel",
                Style::default().fg(DIM),
            ))
            .style(super::theme::panel_style()),
            Rect::new(hint_x, btn_y, content.x + content.width - hint_x, 1),
        );
    }
    rects.select = Some(select);
    rects.cancel = Some(cancel);
    rects
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    fn render(m: &mut ListModal, w: u16, h: u16) -> (String, ModalRects) {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        let mut rects = ModalRects::default();
        term.draw(|f| rects = draw_list_modal(f, m, f.area(), (90, 90)))
            .unwrap();
        let screen: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        (screen, rects)
    }

    #[test]
    fn long_list_scrolls_with_the_cursor_and_shows_a_scrollbar() {
        let items: Vec<String> = (0..50).map(|i| format!("item{i}")).collect();
        let mut m = ListModal::new("PICK", items);
        m.cursor = 49;
        let (screen, rects) = render(&mut m, 60, 14);

        assert!(m.scroll > 0, "cursor at the end must scroll the window");
        assert!(
            screen.contains("item49"),
            "selected row must be visible:\n{screen}"
        );
        assert!(
            !screen.contains("item0 "),
            "the top of the list scrolled away"
        );
        assert!(
            screen.contains('\u{2588}'),
            "scrollbar thumb missing:\n{screen}"
        );
        assert!(
            screen.contains("SELECT") && screen.contains("CANCEL"),
            "buttons missing"
        );
        assert!(rects.select.is_some() && rects.cancel.is_some());
        // Every drawn row is hit-testable and maps back to its item index.
        assert_eq!(rects.rows.last().map(|(i, _)| *i), Some(49));
    }

    /// A modal with a sidebar splits the width: the sections are drawn (with
    /// their counts) on the left, hit-testable, and the list moves right.
    #[test]
    fn the_sidebar_is_drawn_and_hit_testable() {
        let mut m = ListModal::new("PICK", vec!["one".into(), "two".into()]);
        m.sidebar = vec![("ALL".into(), 12), ("DELAY".into(), 4)];
        m.sidebar_cursor = 1;
        m.sidebar_focused = true;
        let (screen, rects) = render(&mut m, 70, 14);

        assert!(screen.contains("ALL"), "sections missing:\n{screen}");
        assert!(screen.contains("DELAY"), "sections missing:\n{screen}");
        assert!(screen.contains("12"), "counts missing:\n{screen}");
        assert_eq!(rects.sidebar.len(), 2);
        let list = rects.list.expect("list area");
        let side = rects.sidebar[0].1;
        assert!(
            list.x > side.x + side.width - 1,
            "the list sits right of the sidebar"
        );
        assert!(rects.rows.iter().all(|(_, r)| r.x == list.x));
    }

    #[test]
    fn filters_are_clickable_chips_and_cycle() {
        let mut m = ListModal::new("PICK", vec!["a".into()]).with_filters(&["ALL", "CLAP", "SF2"]);
        let (screen, rects) = render(&mut m, 60, 12);
        assert!(screen.contains("ALL") && screen.contains("CLAP"));
        assert_eq!(rects.filters.len(), 3);

        m.cursor = 0;
        m.cycle_filter(-1);
        assert_eq!(
            m.filter, 2,
            "cycling back from the first filter wraps to the last"
        );
    }

    /// The thumb has to be able to reach the bottom of its track, and be sized
    /// by the viewport rather than by one row. Passing `items.len()` as the
    /// content length got both wrong: a stubby thumb that stopped short.
    #[test]
    fn the_scrollbar_thumb_spans_the_track_and_reaches_the_end() {
        let items: Vec<String> = (0..200).map(|i| format!("item {i}")).collect();
        let mut m = ListModal::new("PICK", items);

        let (_, rects) = render(&mut m, 40, 20);
        let track = rects.scrollbar.expect("a 200-item list does not fit");

        // Bottom of the list: the thumb's last cell must be the track's last.
        m.cursor = 199;
        let (screen, _) = render(&mut m, 40, 20);
        let rows: Vec<&str> = screen
            .as_str()
            .split("")
            .filter(|s| !s.is_empty())
            .collect();
        let cell = |x: u16, y: u16| rows[(y as usize) * 40 + x as usize];
        let last = track.y + track.height - 1;
        assert_eq!(
            cell(track.x, last),
            "\u{2588}",
            "scrolled to the end, the thumb sits on the last track cell"
        );

        // And at the top it is off the bottom again — otherwise the test above
        // would pass with a thumb that filled the whole track.
        m.cursor = 0;
        let (screen, _) = render(&mut m, 40, 20);
        let rows: Vec<&str> = screen
            .as_str()
            .split("")
            .filter(|s| !s.is_empty())
            .collect();
        assert_ne!(
            rows[(last as usize) * 40 + track.x as usize],
            "\u{2588}",
            "at the top the thumb must not still cover the end of the track"
        );
    }

    /// Dragging the bar is the whole point of exporting its rect: the ends must
    /// land on the first and last item, not near them.
    #[test]
    fn dragging_the_scrollbar_maps_its_ends_to_the_first_and_last_item() {
        let items: Vec<String> = (0..200).map(|i| format!("item {i}")).collect();
        let mut m = ListModal::new("PICK", items);
        let (_, rects) = render(&mut m, 40, 20);
        let track = rects.scrollbar.expect("a 200-item list does not fit");

        m.drag_to(track, track.y);
        assert_eq!(m.cursor, 0, "the top of the track is the first item");

        m.drag_to(track, track.y + track.height - 1);
        assert_eq!(m.cursor, 199, "the bottom of the track is the last item");

        // Every cell maps linearly onto the list. An even-height track has no
        // exact middle cell, so the check is against the ratio the cell really
        // stands for, not against a hand-picked "half".
        let span = (track.height - 1) as usize;
        for cell in 0..=span {
            m.drag_to(track, track.y + cell as u16);
            let expect = cell * 199 / span;
            assert!(
                m.cursor.abs_diff(expect) <= 1,
                "cell {cell} of {span} should land near item {expect}, got {}",
                m.cursor
            );
        }

        // A click past the end clamps instead of panicking on the subtraction.
        m.drag_to(track, track.y + track.height + 50);
        assert_eq!(m.cursor, 199);
    }
}
