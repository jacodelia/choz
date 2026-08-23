//! The harmonics of a sound, as the row of bars its own editor draws.
//!
//! ZynAddSubFX's oscillator is 128 harmonics, each `0..127` with **64 meaning
//! silent** — so a bar grows *from the middle*, up for a positive magnitude and
//! down for an inverted one, which is what its own window shows and what makes
//! a saw look like a saw.
//!
//! Two rows of them, the way that editor is laid out: the **magnitude** of each
//! harmonic above, its **phase** below. One column per harmonic in both. A
//! terminal has fewer columns than the synth has harmonics, so the view scrolls
//! with the cursor rather than squeezing 128 bars into 60 cells and showing
//! neither.

use ratatui::{
    layout::{Margin, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use super::theme::*;
use crate::i18n::t;

pub struct HarmonicsView<'a> {
    /// What each harmonic holds, in order. `None` until the plugin has
    /// answered — a bar nobody knows yet is drawn as the line it sits on.
    pub values: &'a [Option<f32>],
    /// The phase of each, drawn under it. Empty for a plugin that has none.
    pub phases: &'a [Option<f32>],
    pub cursor: usize,
    /// Which of the two rows the cursor is in: 0 the magnitudes, 1 the phases.
    pub row: usize,
    pub scroll: usize,
    pub min: f32,
    pub max: f32,
    pub zero: f32,
    /// Shown in the corner: the tab this belongs to.
    pub title: String,
}

/// Where the bars were drawn, for the mouse.
#[derive(Default, Clone)]
pub struct HarmonicsRects {
    pub area: Option<Rect>,
    /// `(harmonic, its column)` for the magnitudes.
    pub bars: Vec<(usize, Rect)>,
    /// The same for the phases, when they are drawn.
    pub phase_bars: Vec<(usize, Rect)>,
    /// The bar field itself, for the wheel.
    pub field: Option<Rect>,
}

/// How many harmonics fit, given the room there is.
pub fn visible(width: u16) -> usize {
    width.max(1) as usize
}

pub fn draw(f: &mut Frame, area: Rect, v: HarmonicsView) -> HarmonicsRects {
    f.render_widget(Clear, area);
    f.render_widget(Block::default().style(Style::default().bg(BACKDROP)), area);

    let h = (area.height * 70 / 100).clamp(9, area.height);
    let w = (area.width * 90 / 100).max(24).min(area.width);
    let popup = Rect {
        x: area.x + area.width.saturating_sub(w) / 2,
        y: area.y + area.height.saturating_sub(h) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, popup);
    let block = Block::default()
        .title(format!(" {} \u{00b7} {} ", t("HARMONICS"), v.title))
        .title_style(Style::default().fg(HEADER).add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border()))
        .style(super::theme::overlay_style());
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let mut rects = HarmonicsRects {
        area: Some(popup),
        ..Default::default()
    };
    let content = inner.inner(Margin {
        vertical: 0,
        horizontal: 1,
    });
    // Two rows go to the readout and the key line; the rest is the field.
    if content.height < 5 || content.width < 8 {
        return rects;
    }
    let field = Rect {
        x: content.x,
        y: content.y,
        width: content.width,
        height: content.height - 2,
    };
    rects.field = Some(field);

    // Two blocks when the plugin has phases, one when it does not. The
    // magnitudes are the shape of the sound; the phases are what turns a saw
    // into a triangle without touching a single level.
    let two = !v.phases.is_empty() && field.height >= 6;
    let mag_h = match two {
        true => field.height / 2,
        false => field.height,
    };
    let mag = Rect {
        height: mag_h,
        ..field
    };
    let phase = Rect {
        y: field.y + mag_h,
        height: field.height - mag_h,
        ..field
    };
    let count = visible(field.width);

    for (row, area, values) in [(0usize, mag, v.values), (1, phase, v.phases)] {
        if row == 1 && !two {
            break;
        }
        // The middle row of a block is where nothing is: above it the value,
        // below it the same distance the other way — which is how the synth's
        // own editor draws a harmonic that is turned over.
        let half = (area.height / 2).max(1);
        let mid = area.y + half;
        let span = (v.max - v.zero).max(1.0);

        for col in 0..count {
            let i = v.scroll + col;
            let Some(value) = values.get(i) else { break };
            let x = area.x + col as u16;
            let rect = Rect::new(x, area.y, 1, area.height);
            match row {
                0 => rects.bars.push((i, rect)),
                _ => rects.phase_bars.push((i, rect)),
            }

            let chosen = i == v.cursor && row == v.row;
            // The fundamental is worth telling apart at a glance; so is the row
            // the cursor is not in, which is drawn as the quieter of the two.
            let colour = match (chosen, i == 0, row == v.row) {
                (true, ..) => ACCENT,
                (false, true, _) => HEADER,
                (false, false, true) => text(),
                (false, false, false) => DIM,
            };
            let style = Style::default().fg(colour);

            // The line it sits on is always drawn, so an empty harmonic is
            // still a place the cursor can be.
            f.render_widget(
                Paragraph::new(Span::styled("\u{00b7}", Style::default().fg(DIM))),
                Rect::new(x, mid, 1, 1),
            );
            let Some(value) = value else { continue };
            let cells = (((value - v.zero) / span) * half as f32).round() as i32;
            let cells = cells.clamp(-(half as i32), half as i32);
            for step in 1..=cells.unsigned_abs() {
                let y = match cells > 0 {
                    true => mid.saturating_sub(step as u16),
                    false => mid + step as u16,
                };
                if y < area.y || y >= area.y + area.height {
                    break;
                }
                f.render_widget(
                    Paragraph::new(Span::styled("\u{2588}", style)),
                    Rect::new(x, y, 1, 1),
                );
            }
            if chosen && cells == 0 {
                // A cursor on a silent harmonic still has to be visible.
                f.render_widget(
                    Paragraph::new(Span::styled(
                        "\u{2500}",
                        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                    )),
                    Rect::new(x, mid, 1, 1),
                );
            }
        }
        // Which block is which, in the corner it cannot cover a bar.
        let label = match row {
            0 => t("MAG"),
            _ => t("PHASE"),
        };
        let st = match row == v.row {
            true => Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
            false => Style::default().fg(DIM),
        };
        let w = (label.chars().count() as u16).min(area.width);
        f.render_widget(
            Paragraph::new(Span::styled(label.to_string(), st)),
            Rect::new(area.x + area.width - w, area.y, w, 1),
        );
    }

    // What the cursor is on, in the plugin's own numbers — both halves of it,
    // because a harmonic is a level *and* a phase.
    let at = |vals: &[Option<f32>]| {
        vals.get(v.cursor)
            .and_then(|x| *x)
            .map(|x| format!("{x:.0}"))
            .unwrap_or_else(|| "\u{2014}".into())
    };
    let readout = format!(
        "  {} {:>3}   {} {:>4}   {} {:>4}   ({}..{}, {} = {})",
        t("HARMONIC"),
        v.cursor + 1,
        t("MAG"),
        at(v.values),
        t("PHASE"),
        at(v.phases),
        v.min as i32,
        v.max as i32,
        v.zero as i32,
        t("SILENT")
    );
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            readout,
            Style::default().fg(HEADER),
        ))),
        Rect::new(content.x, content.y + field.height, content.width, 1),
    );
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(
                "  \u{2190}\u{2192} {}   \u{2191}\u{2193} {}   Tab {}/{}   PgUp/PgDn \u{00b1}8   Home {}   End {}   Esc {}",
                t("HARMONIC"),
                t("LEVEL"),
                t("MAG"),
                t("PHASE"),
                t("FULL"),
                t("SILENT"),
                t("CLOSE")
            ),
            Style::default().fg(DIM),
        ))),
        Rect::new(
            content.x,
            content.y + field.height + 1,
            content.width,
            1,
        ),
    );
    rects
}
