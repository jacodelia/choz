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
    pub filter: usize,
    pub items: Vec<String>,
    pub cursor: usize,
    pub scroll: usize,
    /// Extra line above the buttons (e.g. the current directory).
    pub note: String,
    /// Extra buttons on the button row: `(label, the key they stand for)`.
    /// Clicking one is the same as pressing that key, so mouse and keyboard
    /// share a single handler.
    pub actions: Vec<(String, char)>,
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
}

/// Truncate to `max` chars.
fn trunc(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
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
            let st = if i == m.filter {
                Style::default()
                    .fg(Color::Black)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(HINT).bg(PANEL_BG)
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
            (PANEL_BG, text())
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
        let mut state = ScrollbarState::new(m.items.len())
            .viewport_content_length(rows)
            .position(m.scroll);
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
}
