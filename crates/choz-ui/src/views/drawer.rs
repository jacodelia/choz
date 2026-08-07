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

fn border_style(focused: bool) -> Style {
    if focused {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
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
    f.render_widget(Paragraph::new(lines).style(super::theme::panel_style()), inner);
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
pub fn draw_output_panel(
    f: &mut Frame,
    area: Rect,
    focused: bool,
    rows: &[OutRow],
    cursor: usize,
) {
    let title = format!(
        " {} ",
        if focused { format!("{} [ACTIVE]", t("OUT")) } else { t("OUT").to_string() }
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
            " \u{2191}\u{2193} \u{00B7} Enter=use \u{00B7} r=rescan"
        } else {
            " OUT (Tab to select)"
        },
        dim,
    ))];

    if rows.is_empty() {
        lines.push(Line::from(Span::styled("   (no outputs found)", dim)));
    }
    for (i, row) in rows.iter().enumerate() {
        if row.header {
            lines.push(Line::from(Span::styled(format!(" {}", row.label), dim)));
            continue;
        }
        let live = row.mark == '\u{2713}';
        let style = if focused && i == cursor {
            Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else if live {
            Style::default().fg(ui_text())
        } else {
            Style::default().fg(Color::Rgb(110, 115, 125))
        };
        lines.push(Line::from(Span::styled(format!(" {} {}", row.mark, row.label), style)));
    }

    f.render_widget(Paragraph::new(lines).style(super::theme::panel_style()), inner);
}

#[cfg(test)]
mod tests {
    use super::*;

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
