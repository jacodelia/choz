//! RACK panel — tabs, mixer strip, instrument line and the insert FX chain.
//!
//! The panel computes its own click rects while it draws and hands them back in
//! a [`RackLayout`]: there is exactly one place that decides where a control
//! sits, so the hit test can't drift from the pixels the way hand-mirrored
//! offsets used to.

use ratatui::{
    layout::Rect, style::{Color, Modifier, Style}, text::{Line, Span},
    widgets::{Block, Borders, Paragraph}, Frame,
};

use crate::i18n::t;
use crate::source::{AudioFxEntry, MAX_FX};
use crate::views::theme::{border as ui_border, text as ui_text};

const PANEL: Color = Color::Rgb(22, 27, 34);
const HEADER: Color = Color::Rgb(240, 136, 62);
const LABEL: Color = Color::Rgb(120, 132, 155);
const RULE: Color = Color::Rgb(38, 44, 54);
const KNOB: Color = Color::Rgb(100, 160, 220);
const SEL: Color = Color::Yellow;

pub const FX_CELL_W: u16 = 13;

/// Width of the close button at the right edge of a tab (the ✕ plus the pad
/// cell after it), in columns.
pub const TAB_CLOSE_W: u16 = 2;

/// Buttons on the RACK's instrument line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RackButton {
    /// Open the source/synth picker.
    Source,
    /// Open the SF2 bank/preset picker (SoundFont tabs only).
    Preset,
    /// Arm MIDI learn.
    Learn,
    /// Previous / next program of the loaded SoundFont.
    PresetPrev,
    PresetNext,
}

/// Every clickable area of the panel, filled in as it draws.
#[derive(Default, Clone)]
pub struct RackLayout {
    pub tabs: Vec<(usize, Rect)>,
    pub tab_close: Vec<(usize, Rect)>,
    pub gain: Option<Rect>,
    pub pan: Option<Rect>,
    pub mute: Option<Rect>,
    pub solo: Option<Rect>,
    pub buttons: Vec<(RackButton, Rect)>,
    /// (FX index, rect) for the chain buttons — they wrap onto further lines
    /// when the chain is wider than the panel.
    pub fx_slots: Vec<(usize, Rect)>,
    pub fx_add: Option<Rect>,
    /// (parameter index, rect) for the knob cells currently on screen.
    pub params: Vec<(usize, Rect)>,
    pub on_off: Option<Rect>,
    pub move_left: Option<Rect>,
    pub move_right: Option<Rect>,
    pub del: Option<Rect>,
}

pub fn knob_indicator(val: f32) -> char {
    match (val.clamp(0.0, 1.0) * 7.99) as usize {
        0 => '\u{2199}', 1 => '\u{2190}', 2 => '\u{2196}', 3 => '\u{2191}',
        4 => '\u{2197}', 5 => '\u{2192}', 6 => '\u{2198}',
        _ => '\u{2193}',
    }
}

pub fn knob_arc(val: f32, width: usize) -> String {
    let filled = (val.clamp(0.0, 1.0) * width as f32).round() as usize;
    format!("{}{}", "\u{2593}".repeat(filled), "\u{2591}".repeat(width.saturating_sub(filled)))
}

/// Active slot's mixer strip: gain (linear), pan (-1..1), mute, solo.
pub type MixStrip = (f32, f32, bool, bool);

/// Max linear slot gain, mirrors `MAX_GAIN` in main.rs — only used to scale the
/// gain bar.
const MAX_GAIN: f32 = 2.0;

/// Instrument-line button labels.
pub const BTN_SOURCE: &str = " SOURCE ";
pub const BTN_PRESET: &str = " BANK/PRESET ";
pub const BTN_LEARN: &str = " MIDI LEARN ";
pub const BTN_PREV: &str = " \u{25C0} ";
pub const BTN_NEXT: &str = " \u{25B6} ";

pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max.saturating_sub(1)).chain(['\u{2026}']).collect()
    }
}

/// Pan position as a small slider, e.g. "L--|-o-R".
pub fn pan_slider(pan: f32) -> String {
    const W: usize = 7;
    let idx = (((pan.clamp(-1.0, 1.0) + 1.0) / 2.0) * (W - 1) as f32).round() as usize;
    let mut s: Vec<char> = "\u{2500}".repeat(W).chars().collect();
    s[W / 2] = '\u{253C}';
    s[idx] = '\u{25CF}';
    format!("L{}R", s.into_iter().collect::<String>())
}

/// Human-readable pan label: "C", "L64", "R30".
pub fn pan_label(pan: f32) -> String {
    let p = (pan.clamp(-1.0, 1.0) * 100.0).round() as i32;
    match p {
        0 => "C".to_string(),
        n if n < 0 => format!("L{}", -n),
        n => format!("R{n}"),
    }
}

/// How many knob columns fit in `width`, and how many rows `n` knobs need.
/// Exposed so the parameter cursor logic and the tests agree with the drawing.
pub fn param_grid(width: u16, n: usize) -> (usize, usize) {
    let cols = (width.saturating_sub(2) / FX_CELL_W).max(1) as usize;
    let rows = n.div_ceil(cols);
    (cols, rows)
}

/// Draw the RACK panel and return its click rects.
#[allow(clippy::too_many_arguments)]
pub fn draw_fx_chain_panel(
    f: &mut Frame,
    area: Rect,
    chain: &[AudioFxEntry],
    fx_slot: usize,
    fx_param: usize,
    focused: bool,
    tabs: &[String],
    active_slot: usize,
    mix: Option<MixStrip>,
    instrument: &str,
    // `preset` is the current bank:preset name, when the tab holds a SoundFont.
    preset: Option<&str>,
) -> RackLayout {
    let has_presets = preset.is_some();
    let mut layout = RackLayout::default();
    let border_style = if focused {
        Style::default().fg(SEL).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(ui_border())
    };

    let block = Block::default()
        .title(format!(" {} ", if focused { format!("{} [ACTIVE]", t("RACK")) } else { t("RACK").to_string() }))
        .title_style(Style::default().fg(HEADER).add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_style(border_style)
        .style(Style::default().bg(PANEL));

    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return layout;
    }

    let bg = Style::default().bg(PANEL);
    let put = |f: &mut Frame, line: Line, y: u16| {
        if y < inner.y + inner.height {
            f.render_widget(Paragraph::new(line).style(bg), Rect::new(inner.x, y, inner.width, 1));
        }
    };
    let rule = |f: &mut Frame, label: &str, y: u16| {
        if y >= inner.y + inner.height {
            return;
        }
        let text = format!("\u{2500}\u{2500} {label} ");
        let pad = (inner.width as usize).saturating_sub(text.chars().count());
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(text, Style::default().fg(LABEL).add_modifier(Modifier::BOLD)),
                Span::styled("\u{2500}".repeat(pad), Style::default().fg(RULE)),
            ]))
            .style(bg),
            Rect::new(inner.x, y, inner.width, 1),
        );
    };

    let mut y = inner.y;

    // ── Tabs ───────────────────────────────────────────────────────────────
    let mut tab_line: Vec<Span> = vec![Span::raw(" ")];
    if tabs.is_empty() {
        tab_line.push(Span::styled(
            " (empty rack \u{2014} bind an input on the left) ",
            Style::default().fg(Color::DarkGray).bg(PANEL),
        ));
    } else {
        let mut x = inner.x + 1;
        for (i, label) in tabs.iter().enumerate() {
            let w = label.chars().count() as u16 + 2;
            layout.tabs.push((i, Rect::new(x, y, w, 1)));
            layout.tab_close.push((i, Rect::new(x + w - TAB_CLOSE_W, y, TAB_CLOSE_W, 1)));
            let st = if i == active_slot {
                Style::default().fg(Color::Black).bg(HEADER).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Rgb(200, 205, 215)).bg(Color::Rgb(40, 46, 56))
            };
            tab_line.push(Span::styled(format!(" {label} "), st));
            tab_line.push(Span::raw(" "));
            x += w + 1;
        }
    }
    put(f, Line::from(tab_line), y);
    y += 1;

    if tabs.is_empty() {
        return layout;
    }

    // ── Mixer strip ────────────────────────────────────────────────────────
    let (gain, pan, mute, solo) = mix.unwrap_or((1.0, 0.0, false, false));
    let flag = |on: bool, on_bg: Color| {
        if on {
            Style::default().fg(Color::Black).bg(on_bg).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Rgb(110, 115, 125)).bg(Color::Rgb(32, 38, 47))
        }
    };
    let label_style = Style::default().fg(LABEL).add_modifier(Modifier::BOLD);
    // (text, style, which rect it feeds) — laid out left to right, so the rect
    // of each cell falls out of the widths instead of being re-derived.
    let cells: Vec<(String, Style, Option<usize>)> = vec![
        (format!("{} ", t("VOL")), label_style, None),
        (format!("[{}] {gain:4.2}  ", knob_arc(gain / MAX_GAIN, 8)), Style::default().fg(KNOB), Some(0)),
        (format!("{} ", t("PAN")), label_style, None),
        (format!("{} {:<4}  ", pan_slider(pan), pan_label(pan)), Style::default().fg(KNOB), Some(1)),
        (format!(" {} ", t("MUTE")), flag(mute, Color::Rgb(200, 80, 80)), Some(2)),
        (" ".to_string(), bg, None),
        (format!(" {} ", t("SOLO")), flag(solo, Color::Rgb(220, 190, 70)), Some(3)),
    ];
    let mut x = inner.x + 2;
    let mut mix_line: Vec<Span> = vec![Span::raw("  ")];
    for (text, style, target) in cells {
        let w = text.chars().count() as u16;
        let rect = Rect::new(x, y, w, 1);
        match target {
            Some(0) => layout.gain = Some(rect),
            Some(1) => layout.pan = Some(rect),
            Some(2) => layout.mute = Some(rect),
            Some(3) => layout.solo = Some(rect),
            _ => {}
        }
        mix_line.push(Span::styled(text, style));
        x += w;
    }
    put(f, Line::from(mix_line), y);
    y += 1;

    // ── Instrument line ────────────────────────────────────────────────────
    let btn_style = Style::default().fg(Color::Black).bg(KNOB).add_modifier(Modifier::BOLD);
    let mut instr_line: Vec<Span> = vec![
        Span::styled(format!("  {} ", t("INSTR")), Style::default().fg(LABEL).add_modifier(Modifier::BOLD)),
        Span::styled(format!("{:<18}", truncate(instrument, 18)), Style::default().fg(ui_text())),
    ];
    let mut bx = inner.x + 2 + 8 + 18;
    for (btn, text) in [
        (RackButton::Source, Some(BTN_SOURCE)),
        // Bank/preset only exists while the tab holds a SoundFont.
        (RackButton::Preset, has_presets.then_some(BTN_PRESET)),
        (RackButton::Learn, Some(BTN_LEARN)),
    ] {
        let Some(text) = text else { continue };
        let w = text.chars().count() as u16;
        layout.buttons.push((btn, Rect::new(bx, y, w, 1)));
        instr_line.push(Span::styled(text, btn_style));
        instr_line.push(Span::raw(" "));
        bx += w + 1;
    }
    put(f, Line::from(instr_line), y);
    y += 1;

    // ── Bank / preset line (SoundFont tabs only) ───────────────────────────
    if let Some(name) = preset {
        let mut px = inner.x + 2 + 8;
        let mut line: Vec<Span> = vec![
            Span::styled(format!("  {}  ", t("BANK")), Style::default().fg(LABEL).add_modifier(Modifier::BOLD)),
        ];
        for (btn, text) in [(RackButton::PresetPrev, BTN_PREV), (RackButton::PresetNext, BTN_NEXT)] {
            let w = text.chars().count() as u16;
            layout.buttons.push((btn, Rect::new(px, y, w, 1)));
            line.push(Span::styled(text, btn_style));
            line.push(Span::raw(" "));
            px += w + 1;
            if btn == RackButton::PresetPrev {
                // The name sits between the two arrows.
                let w = 30u16;
                line.push(Span::styled(
                    format!("{:<30}", truncate(name, 30)),
                    Style::default().fg(ui_text()),
                ));
                line.push(Span::raw(" "));
                px += w + 1;
            }
        }
        put(f, Line::from(line), y);
        y += 1;
    }

    // ── FX chain ───────────────────────────────────────────────────────────
    rule(f, t("FX CHAIN"), y);
    y += 1;

    // Chain buttons wrap onto further lines instead of running off the panel.
    let mut cx = inner.x + 2;
    let right = inner.x + inner.width;
    let mut chain_line: Vec<Span> = vec![Span::raw("  ")];
    let flush = |f: &mut Frame, line: &mut Vec<Span>, y: u16| {
        put(f, Line::from(std::mem::replace(line, vec![Span::raw("  ")])), y);
    };
    for (i, entry) in chain.iter().enumerate() {
        let text = format!(" {}:{} ", i + 1, entry.label());
        let w = text.chars().count() as u16;
        if cx + w > right && cx > inner.x + 2 {
            flush(f, &mut chain_line, y);
            y += 1;
            cx = inner.x + 2;
        }
        let st = if i == fx_slot && focused {
            Style::default().fg(Color::Black).bg(SEL).add_modifier(Modifier::BOLD)
        } else if entry.enabled {
            Style::default().fg(Color::Rgb(56, 200, 100)).bg(Color::Rgb(30, 40, 34))
        } else {
            Style::default()
                .fg(Color::Rgb(90, 95, 105))
                .bg(Color::Rgb(30, 34, 40))
                .add_modifier(Modifier::CROSSED_OUT)
        };
        layout.fx_slots.push((i, Rect::new(cx, y, w, 1)));
        chain_line.push(Span::styled(text, st));
        chain_line.push(Span::raw(" "));
        cx += w + 1;
    }
    if chain.len() < MAX_FX {
        let add = " + ADD ";
        let w = add.chars().count() as u16;
        if cx + w > right && cx > inner.x + 2 {
            flush(f, &mut chain_line, y);
            y += 1;
            cx = inner.x + 2;
        }
        layout.fx_add = Some(Rect::new(cx, y, w, 1));
        chain_line.push(Span::styled(
            add,
            Style::default().fg(Color::Black).bg(Color::Rgb(100, 160, 220)),
        ));
    }
    flush(f, &mut chain_line, y);
    y += 1;

    let Some(entry) = chain.get(fx_slot) else {
        if chain.is_empty() {
            put(
                f,
                Line::from(Span::styled(
                    "   no FX yet \u{2014} press 'a' or click [+ ADD]",
                    Style::default().fg(Color::DarkGray),
                )),
                y,
            );
        }
        return layout;
    };

    // ── Selected FX: parameters in a bordered box, wrapping over rows ──────
    let descs = entry.param_descs();
    let n = descs.len();
    let (cols, rows_needed) = param_grid(inner.width, n);
    // 3 rows per knob row, plus the box border.
    let room = (inner.y + inner.height).saturating_sub(y + 5).max(3) as usize;
    let rows_shown = (room / 3).clamp(1, rows_needed.max(1));
    // `param_grid` never returns 0 columns, so this can't divide by zero.
    let cursor_row = fx_param / cols.max(1);
    let first_row = cursor_row.saturating_sub(rows_shown.saturating_sub(1));

    let box_h = (rows_shown * 3) as u16 + 2;
    let box_rect = Rect::new(inner.x + 1, y, inner.width.saturating_sub(2), box_h.min(inner.height));
    let more = if rows_needed > rows_shown {
        format!(" ({}/{} rows) ", first_row + rows_shown, rows_needed)
    } else {
        String::new()
    };
    let param_block = Block::default()
        .title(format!(" {}:{}{} ", fx_slot + 1, entry.label(), more))
        .title_style(Style::default().fg(HEADER).add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(RULE))
        .style(bg);
    let param_inner = param_block.inner(box_rect);
    f.render_widget(param_block, box_rect);

    for row in 0..rows_shown {
        let ry = param_inner.y + (row * 3) as u16;
        if ry + 2 > param_inner.y + param_inner.height {
            break;
        }
        let mut knob_spans: Vec<Span> = Vec::new();
        let mut val_spans: Vec<Span> = Vec::new();
        let mut name_spans: Vec<Span> = Vec::new();
        for col in 0..cols {
            let pi = (first_row + row) * cols + col;
            if pi >= n {
                break;
            }
            let val = entry.params.get(pi).copied().unwrap_or(0.0);
            let is_p = pi == fx_param && focused;
            layout.params.push((
                pi,
                Rect::new(param_inner.x + (col as u16) * FX_CELL_W, ry, FX_CELL_W, 3),
            ));
            knob_spans.push(Span::styled(
                format!("{:<width$}", format!("[{}]", knob_arc(val, 8)), width = FX_CELL_W as usize),
                Style::default().fg(if is_p { SEL } else { KNOB }),
            ));
            val_spans.push(Span::styled(
                format!("{:<width$}", format!(" {}{val:4.2}", knob_indicator(val)), width = FX_CELL_W as usize),
                Style::default()
                    .fg(if is_p { SEL } else { ui_text() })
                    .add_modifier(if is_p { Modifier::BOLD } else { Modifier::empty() }),
            ));
            let name = descs.get(pi).map(|d| d.name.as_ref()).unwrap_or("?");
            name_spans.push(Span::styled(
                format!(" {:<width$}", truncate(name, FX_CELL_W as usize - 2), width = FX_CELL_W as usize - 1),
                Style::default().fg(if is_p { SEL } else { LABEL }),
            ));
        }
        let put_row = |f: &mut Frame, spans: Vec<Span>, yy: u16| {
            f.render_widget(
                Paragraph::new(Line::from(spans)).style(bg),
                Rect::new(param_inner.x, yy, param_inner.width, 1),
            );
        };
        put_row(f, knob_spans, ry);
        put_row(f, val_spans, ry + 1);
        put_row(f, name_spans, ry + 2);
    }
    y = box_rect.y + box_rect.height;

    // ── Slot controls, in their own box, one blank line below the knobs ────
    if y + 2 < inner.y + inner.height {
        y += 1;
        let ctrl_h = 3u16;
        let ctrl_rect = Rect::new(inner.x + 1, y, inner.width.saturating_sub(2), ctrl_h);
        let ctrl_block = Block::default()
            .title(format!(" {} ", t("SLOT")))
            .title_style(Style::default().fg(LABEL))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(RULE))
            .style(bg);
        let ctrl_inner = ctrl_block.inner(ctrl_rect);
        f.render_widget(ctrl_block, ctrl_rect);

        let mut cx = ctrl_inner.x + 1;
        let mut spans: Vec<Span> = vec![Span::raw(" ")];
        let mut button = |spans: &mut Vec<Span<'static>>, text: String, style: Style, rect: &mut Option<Rect>| {
            let w = text.chars().count() as u16;
            *rect = Some(Rect::new(cx, ctrl_inner.y, w, 1));
            spans.push(Span::styled(text, style));
            // Buttons are spaced out so they don't read as one bar.
            spans.push(Span::raw("   "));
            cx += w + 3;
        };
        let (on_lbl, on_style) = if entry.enabled {
            ("  ON  ", Style::default().fg(Color::Black).bg(Color::Rgb(56, 200, 100)))
        } else {
            (" OFF  ", Style::default().fg(Color::Rgb(180, 185, 195)).bg(Color::Rgb(40, 46, 56)))
        };
        button(&mut spans, on_lbl.into(), on_style.add_modifier(Modifier::BOLD), &mut layout.on_off);
        let mv = Style::default().fg(Color::Black).bg(Color::Rgb(150, 195, 245));
        let mv_off = Style::default().fg(Color::Rgb(70, 78, 92)).bg(Color::Rgb(32, 38, 47));
        // Disabled ends of the chain still draw a (greyed) button, they just
        // don't get a click rect.
        let can_left = fx_slot > 0;
        let can_right = fx_slot + 1 < chain.len();
        let mut left = None;
        let mut right = None;
        button(&mut spans, " \u{25C0} MOVE ".into(), if can_left { mv } else { mv_off }, &mut left);
        button(&mut spans, " MOVE \u{25B6} ".into(), if can_right { mv } else { mv_off }, &mut right);
        layout.move_left = can_left.then_some(left).flatten();
        layout.move_right = can_right.then_some(right).flatten();
        button(&mut spans, " DEL ".into(),
               Style::default().fg(Color::White).bg(Color::Rgb(170, 50, 50)).add_modifier(Modifier::BOLD),
               &mut layout.del);
        f.render_widget(
            Paragraph::new(Line::from(spans)).style(bg),
            Rect::new(ctrl_inner.x, ctrl_inner.y, ctrl_inner.width, 1),
        );
        y = ctrl_rect.y + ctrl_rect.height;
    }

    // ── Hint, last line of the panel ───────────────────────────────────────
    let hint_y = (inner.y + inner.height).saturating_sub(1).max(y);
    let hint = if focused {
        "  1=source 2=bank/preset 3=learn p=instr \u{00B7} a=add d=del \u{2190}\u{2192}=FX \u{2191}\u{2193}=param wheel=value \u{00B7} -/+=vol ,/.=pan m=mute S=solo"
    } else {
        "  Tab=enter the rack"
    };
    put(f, Line::from(Span::styled(truncate(hint, inner.width as usize), Style::default().fg(ui_border()))), hint_y);

    layout
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_wrap_onto_more_rows_when_they_dont_fit() {
        // 80 columns of panel → 6 knob columns of 13.
        assert_eq!(param_grid(80, 6), (6, 1));
        assert_eq!(param_grid(80, 7), (6, 2), "a 7th knob starts a second row");
        assert_eq!(param_grid(80, 16), (6, 3), "Z5 Texture's 16 params take 3 rows");
        assert_eq!(param_grid(10, 4), (1, 4), "a narrow panel stacks them");
        assert_eq!(param_grid(80, 0), (6, 0));
    }
}
