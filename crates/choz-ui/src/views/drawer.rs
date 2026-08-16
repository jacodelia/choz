//! Collapsible side drawers: IN on the left, OUT on the right.
//!
//! Closed, a drawer is a narrow vertical handle glued to the screen edge with
//! its name running downwards; open, it takes a share of the body and the RACK
//! shrinks. The left one hosts the INPUTS panel (see
//! [`crate::views::source_panel`]); the right one lists the audio output
//! devices, which is why it lives here rather than in a panel of its own.

use crate::i18n::t;
use crate::views::theme::{border as ui_border, text as ui_text};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

const HEADER: Color = Color::Rgb(240, 136, 62);

/// Width of a closed drawer: two border columns plus the letter between them.
pub const HANDLE_W: u16 = 3;

/// Lines the OUT panel draws above its device list.
pub const OUTPUT_LIST_TOP: usize = 1;

/// Columns a drawer gets out of a body `total` wide: `pct` of it when open,
/// never under `min` (device names are long) and never past half the screen,
/// so the RACK always keeps the bigger half.
pub fn drawer_width(open: bool, total: u16, pct: u16, min: u16) -> u16 {
    if !open {
        return HANDLE_W;
    }
    let half = total / 2;
    (total * pct / 100).max(min).min(half).max(HANDLE_W)
}

/// How many list rows fit in a drawer of outer rect `area`, under `top` header
/// lines and above `extra` lines pinned at the bottom (the MIDI-learn banner).
///
/// Takes the **outer** rect and subtracts the two border rows itself: the draw
/// code has the inner rect and the hit-test code has the outer one, and the two
/// disagreeing by one is exactly the bug this whole thing is about.
pub fn list_height(area: Rect, top: usize, extra: usize) -> usize {
    (area.height as usize)
        .saturating_sub(2)
        .saturating_sub(top)
        .saturating_sub(extra)
}

/// First visible row of a list `rows` long showing `height` of them at once,
/// with `cursor` on screen.
///
/// **There is no scroll offset stored anywhere.** The window is a function of
/// the cursor, which is the same thing the rack's knob box does: no second
/// piece of state to keep in step with the list, nothing to reset when the list
/// changes under it, and the draw and the click rects cannot drift apart
/// because both call this.
///
/// The cost is that the wheel moves the cursor rather than the view. On a list
/// where every row is a thing you act on, that is what a wheel is for anyway.
pub fn list_scroll(cursor: usize, rows: usize, height: usize) -> usize {
    if height == 0 || rows <= height {
        return 0;
    }
    cursor
        .saturating_sub(height - 1)
        .min(rows.saturating_sub(height))
}

/// What the drawer's title says about a list that does not fit: where the
/// window is, so "there is more" is visible without spending a row on it.
fn scroll_hint(scroll: usize, rows: usize, height: usize) -> String {
    if height == 0 || rows <= height {
        return String::new();
    }
    format!(
        " \u{2195} {}-{}/{}",
        scroll + 1,
        (scroll + height).min(rows),
        rows
    )
}

fn border_style(focused: bool) -> Style {
    if focused {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(ui_border())
    }
}

/// The close button of an open drawer: an `✕` on the top border at the right,
/// where a window's close button lives. Both drawers put it in the same corner
/// — the left one would land on the panel title. Returns its rect so the click
/// can be routed back.
pub fn draw_close_button(f: &mut Frame, area: Rect) -> Option<Rect> {
    if area.width < 8 || area.height == 0 {
        return None;
    }
    let rect = Rect::new(area.right().saturating_sub(4), area.y, 3, 1);
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "[\u{2715}]",
            Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
        )))
        .style(super::theme::panel_style()),
        rect,
    );
    Some(rect)
}

/// The closed drawer: `label` runs down the edge, one character per row.
pub fn draw_handle(f: &mut Frame, area: Rect, label: &str, focused: bool) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style(focused))
        .style(super::theme::panel_style());
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let lines: Vec<Line> = label
        .chars()
        .take(inner.height as usize)
        .map(|c| {
            Line::from(Span::styled(
                c.to_string(),
                Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
            ))
        })
        .collect();
    f.render_widget(
        Paragraph::new(lines).style(super::theme::panel_style()),
        inner,
    );
}

/// One row of the OUT drawer. The panel only draws them; what a row *means*
/// (an output device, a channel pair) stays in the app, which is what Enter
/// and the mouse dispatch on.
pub struct OutRow {
    /// Text of the row, already laid out by the caller.
    pub label: String,
    /// Leading glyph: a tick for what is live, a dot for the rest.
    pub mark: char,
    /// Section titles are dimmed and never selectable.
    pub header: bool,
}

/// The open OUT drawer: the output devices, then the device's channel pairs so
/// a rack tab can be sent to any jack of the interface.
pub fn draw_output_panel(f: &mut Frame, area: Rect, focused: bool, rows: &[OutRow], cursor: usize) {
    let height = list_height(area, OUTPUT_LIST_TOP, 0);
    let scroll = list_scroll(cursor, rows.len(), height);
    let title = format!(
        " {}{} ",
        if focused {
            format!("{} [ACTIVE]", t("OUT"))
        } else {
            t("OUT").to_string()
        },
        scroll_hint(scroll, rows.len(), height)
    );
    let block = Block::default()
        .title(title)
        .title_style(Style::default().fg(HEADER))
        .borders(Borders::ALL)
        .border_style(border_style(focused))
        .style(super::theme::panel_style());

    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height < 2 || inner.width == 0 {
        return;
    }

    let dim = Style::default().fg(Color::DarkGray);
    let mut lines: Vec<Line> = vec![Line::from(Span::styled(
        if focused {
            " \u{2191}\u{2193} \u{00B7} Enter/Space=on-off \u{00B7} RMB=off \u{00B7} r=rescan"
        } else {
            " OUT (Tab to select)"
        },
        dim,
    ))];

    if rows.is_empty() {
        lines.push(Line::from(Span::styled("   (no outputs found)", dim)));
    }
    for (i, row) in rows.iter().enumerate().skip(scroll).take(height) {
        if row.header {
            lines.push(Line::from(Span::styled(format!(" {}", row.label), dim)));
            continue;
        }
        let live = row.mark == '\u{2713}';
        let style = if focused && i == cursor {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else if live {
            Style::default().fg(ui_text())
        } else {
            Style::default().fg(Color::Rgb(110, 115, 125))
        };
        lines.push(Line::from(Span::styled(
            format!(" {} {}", row.mark, row.label),
            style,
        )));
    }

    f.render_widget(
        Paragraph::new(lines).style(super::theme::panel_style()),
        inner,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The window always contains the cursor, never runs past the end, and
    /// stays at zero while the list fits — the three things a click rect
    /// computed from it depends on.
    #[test]
    fn the_window_follows_the_cursor_and_stops_at_the_end() {
        // Fits: no scrolling, whatever the cursor does.
        assert_eq!(list_scroll(0, 5, 8), 0);
        assert_eq!(list_scroll(4, 5, 8), 0);

        // Twenty rows through a window of seven.
        assert_eq!(list_scroll(0, 20, 7), 0, "the top stays at the top");
        assert_eq!(list_scroll(6, 20, 7), 0, "the last visible row is still 0");
        assert_eq!(list_scroll(7, 20, 7), 1, "one past it moves by one");
        assert_eq!(list_scroll(19, 20, 7), 13, "the end shows the last seven");
        assert_eq!(list_scroll(99, 20, 7), 13, "and never goes past them");

        for cursor in 0..20 {
            let s = list_scroll(cursor, 20, 7);
            assert!((s..s + 7).contains(&cursor), "cursor {cursor} off screen");
        }

        // A drawer with no room for a list asks for none.
        assert_eq!(list_scroll(5, 20, 0), 0);
        assert_eq!(list_height(Rect::new(0, 0, 20, 4), 3, 0), 0);
        assert_eq!(list_height(Rect::new(0, 0, 20, 12), 3, 0), 7);
        assert_eq!(
            list_height(Rect::new(0, 0, 20, 12), 3, 2),
            5,
            "learn banner"
        );
    }

    #[test]
    fn closed_is_a_handle_open_never_eats_the_rack() {
        assert_eq!(drawer_width(false, 200, 40, 24), HANDLE_W);
        assert_eq!(drawer_width(true, 200, 40, 24), 80);
        // Narrow terminal: the minimum yields to the half-screen cap...
        assert_eq!(drawer_width(true, 40, 40, 24), 20);
        // ...and the cap itself never goes under the handle.
        assert_eq!(drawer_width(true, 4, 40, 24), HANDLE_W);
    }
}
