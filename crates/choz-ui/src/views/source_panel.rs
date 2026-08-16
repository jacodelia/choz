//! INPUTS panel — the left column.
//!
//! Lists the note inputs (hardware MIDI ports + OSC); selecting one binds it to
//! a rack tab. SF2 programs are *not* listed here any more — the RACK's
//! `[BANK/PRESET]` button opens them in a modal instead.

use crate::i18n::t;
use crate::views::theme::{border as ui_border, text as ui_text};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

const ACCENT: Color = Color::Rgb(31, 111, 235);
const HEADER: Color = Color::Rgb(240, 136, 62);

/// One row of the input list: a note input (MIDI/OSC), an audio capture pair,
/// or a section title.
pub struct InputRow {
    /// "MIDI", "OSC" or "AUDIO".
    pub kind: &'static str,
    pub name: String,
    /// Whether choz is currently listening to it.
    pub connected: bool,
    /// Rack tab bound to this input, if any (0-based).
    pub bound_tab: Option<usize>,
    /// Section titles are dimmed and never selectable.
    pub header: bool,
}

/// Panel lines above the input list (active-tab line, scan-button line, hint).
pub const INPUT_LIST_TOP: usize = 3;

/// Lines the MIDI-learn banner takes at the bottom, when it is up: a blank one
/// and the banner. They are pinned there, so the list gets that much less.
pub const LEARN_LINES: usize = 2;

/// Rows of the input list that fit, and the first one on screen. The hit-test
/// in `compute_layout` calls this with the same arguments — one function, so a
/// scrolled list cannot answer clicks for rows it is not showing.
pub fn input_window(area: Rect, rows: usize, cursor: usize, learn: bool) -> (usize, usize) {
    let height = crate::views::drawer::list_height(
        area,
        INPUT_LIST_TOP,
        if learn { LEARN_LINES } else { 0 },
    );
    (
        crate::views::drawer::list_scroll(cursor, rows, height),
        height,
    )
}

/// Label of the rescan button (translated, padded), so the panel and its click
/// rect always agree on the width.
fn scan_label() -> String {
    format!(" {} ", t("SCAN INPUTS"))
}

/// Draw the INPUTS panel.
pub fn draw_input_panel(
    f: &mut Frame,
    area: Rect,
    focused: bool,
    inputs: &[InputRow],
    input_cursor: usize,
    active_label: &str,
    learn: Option<&str>,
) -> Option<Rect> {
    let border_style = if focused {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(ui_border())
    };
    let (scroll, height) = input_window(area, inputs.len(), input_cursor, learn.is_some());
    let title = format!(
        " {}{} ",
        if focused {
            format!("{} [ACTIVE]", t("INPUTS"))
        } else {
            t("INPUTS").to_string()
        },
        if height > 0 && inputs.len() > height {
            format!(
                " \u{2195} {}-{}/{}",
                scroll + 1,
                (scroll + height).min(inputs.len()),
                inputs.len()
            )
        } else {
            String::new()
        }
    );

    let block = Block::default()
        .title(title)
        .title_style(Style::default().fg(HEADER))
        .borders(Borders::ALL)
        .border_style(border_style)
        .style(super::theme::panel_style());

    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height < 2 || inner.width == 0 {
        return None;
    }

    let dim = Style::default().fg(Color::DarkGray);
    let mut lines: Vec<Line> = Vec::new();

    // Line 0: what the active rack tab is.
    lines.push(Line::from(vec![
        Span::styled(" TAB: ", dim),
        Span::styled(
            active_label.to_string(),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
    ]));

    // Line 1: rescan button — MIDI devices come and go while choz runs.
    let scan_rect = Rect::new(
        inner.x + 1,
        inner.y + 1,
        scan_label().chars().count() as u16,
        1,
    );
    lines.push(Line::from(vec![
        Span::raw(" "),
        Span::styled(
            scan_label(),
            Style::default()
                .fg(Color::Black)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    // Line 2: hint for the input list.
    lines.push(Line::from(Span::styled(
        if focused {
            " \u{2191}\u{2193} \u{00B7} Enter/Space=on-off \u{00B7} RMB=off \u{00B7} c=connect \u{00B7} r=rescan"
        } else {
            " INPUTS (Tab to select)"
        },
        dim,
    )));

    if inputs.is_empty() {
        lines.push(Line::from(Span::styled("   (no inputs found)", dim)));
    }
    for (i, row) in inputs.iter().enumerate().skip(scroll).take(height) {
        if row.header {
            lines.push(Line::from(Span::styled(format!(" {}", row.name), dim)));
            continue;
        }
        let mark = if row.connected {
            "\u{2713}"
        } else {
            "\u{00B7}"
        };
        let style = if focused && i == input_cursor {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else if row.connected {
            Style::default().fg(ui_text())
        } else {
            Style::default().fg(Color::Rgb(110, 115, 125))
        };
        let bound = match row.bound_tab {
            Some(t) => format!(" \u{2192} tab {}", t + 1),
            None => String::new(),
        };
        lines.push(Line::from(vec![
            Span::styled(format!(" {mark} {:<4} {}", row.kind, row.name), style),
            Span::styled(bound, Style::default().fg(HEADER)),
        ]));
    }

    // MIDI-learn banner: which rack control is waiting for a CC.
    if let Some(target) = learn {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!(" \u{25CF} MIDI LEARN \u{00B7} {target}"),
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));
    }

    f.render_widget(
        Paragraph::new(lines).style(super::theme::panel_style()),
        inner,
    );
    Some(scan_rect)
}
