//! FX Chain panel — display and edit the insert FX chain.
//! Based on the PATTERN view's FX Chain panel.

use ratatui::{
    layout::Rect, style::{Color, Modifier, Style}, text::{Line, Span},
    widgets::{Block, Borders, Paragraph}, Frame,
};

use crate::source::{AudioFxEntry, fx_param_descs};

const PANEL: Color = Color::Rgb(22, 27, 34);
const BORDER: Color = Color::Rgb(48, 54, 61);
const HEADER: Color = Color::Rgb(240, 136, 62);

pub const FX_CELL_W: u16 = 13;

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

/// Draw the FX Chain panel.
pub fn draw_fx_chain_panel(
    f: &mut Frame,
    area: Rect,
    chain: &[AudioFxEntry],
    fx_slot: usize,
    fx_param: usize,
    focused: bool,
) {
    let border_style = if focused {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(BORDER)
    };

    let title = if focused {
        " FX CHAIN [ACTIVE] ".to_string()
    } else {
        " FX CHAIN ".to_string()
    };

    let block = Block::default()
        .title(title)
        .title_style(Style::default().fg(HEADER).add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_style(border_style)
        .style(Style::default().bg(PANEL));

    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 { return; }

    let bg = Style::default().bg(PANEL);
    let put = |f: &mut Frame, line: Line, y: u16| {
        if y < inner.y + inner.height {
            f.render_widget(Paragraph::new(line).style(bg), Rect::new(inner.x, y, inner.width, 1));
        }
    };

    let cy = inner.y;
    let mut y = cy;

    // Hint
    let hint = if focused {
        "  \u{2190}\u{2192}=select FX  \u{2191}\u{2193}=param  wheel=value  a=add d=del Tab=toggle".to_string()
    } else {
        "  Tab=enter FX chain".to_string()
    };
    put(f, Line::from(Span::styled(hint, Style::default().fg(BORDER))), y);
    y += 1;

    // FX slot buttons
    let mut fx_line: Vec<Span> = vec![Span::styled("  ", Style::default().bg(PANEL))];
    for (i, entry) in chain.iter().enumerate() {
        let is_sel = i == fx_slot && focused;
        let st = if is_sel {
            Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else if entry.enabled {
            Style::default().fg(Color::Rgb(56, 200, 100)).bg(PANEL)
        } else {
            Style::default().fg(Color::Rgb(90, 95, 105)).bg(PANEL).add_modifier(Modifier::CROSSED_OUT)
        };
        fx_line.push(Span::styled(format!(" {}:{} ", i + 1, entry.kind.label()), st));
    }
    if chain.len() < 5 {
        fx_line.push(Span::styled(" [+ ADD] ", Style::default().fg(Color::Rgb(100, 160, 220)).bg(PANEL)));
    }
    put(f, Line::from(fx_line), y);
    y += 2;

    // Parameters for selected FX
    if let Some(entry) = chain.get(fx_slot) {
        let descs = fx_param_descs(entry.kind);
        let n = descs.len();

        // ROUTING line
        let mut rt: Vec<Span> = vec![
            Span::styled("  ROUTING: ", Style::default().fg(HEADER).add_modifier(Modifier::BOLD)),
            Span::styled("IN", Style::default().fg(Color::Rgb(120, 130, 150))),
        ];
        if chain.is_empty() {
            rt.push(Span::styled(" -> OUT (no FX)", Style::default().fg(Color::Rgb(120, 130, 150))));
        } else {
            for (i, e) in chain.iter().enumerate() {
                rt.push(Span::styled(" -> ", Style::default().fg(Color::Rgb(120, 130, 150))));
                let st = if i == fx_slot {
                    Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD)
                } else if e.enabled {
                    Style::default().fg(Color::Rgb(120, 220, 150))
                } else {
                    Style::default().fg(Color::Rgb(110, 115, 125)).add_modifier(Modifier::CROSSED_OUT)
                };
                rt.push(Span::styled(format!("{}:{}", i + 1, e.kind.label()), st));
            }
            rt.push(Span::styled(" -> OUT", Style::default().fg(Color::Rgb(120, 130, 150))));
        }
        put(f, Line::from(rt), y);
        y += 1;

        // Parameter knobs
        let avail = inner.width.saturating_sub(2) as usize;
        let visible = (avail / FX_CELL_W as usize).max(1);
        let start = if focused && fx_param >= visible { fx_param + 1 - visible } else { 0 };
        let end = (start + visible).min(n);

        let mut top_spans: Vec<Span> = vec![Span::raw("  ")];
        let mut mid_spans: Vec<Span> = vec![Span::raw("  ")];
        let mut lbl_spans: Vec<Span> = vec![Span::raw("  ")];

        for pi in start..end {
            let val = entry.params.get(pi).copied().unwrap_or(0.0);
            let is_p = pi == fx_param && focused;
            let col_k = if is_p { Color::Yellow } else { Color::Rgb(100, 160, 220) };

            top_spans.push(Span::styled(
                format!("{:<width$}", format!("[{}]", knob_arc(val, 8)), width = FX_CELL_W as usize),
                Style::default().fg(col_k)));

            let ind = knob_indicator(val);
            let col_v = if is_p { Color::Yellow } else { Color::White };
            mid_spans.push(Span::styled(
                format!("{:<width$}", format!(" {ind}{:4.2}", val), width = FX_CELL_W as usize),
                Style::default().fg(col_v).add_modifier(if is_p { Modifier::BOLD } else { Modifier::empty() })));

            let name = descs.get(pi).map(|d| d.name).unwrap_or("?");
            lbl_spans.push(Span::styled(
                format!(" {:<width$}", name, width = (FX_CELL_W - 1) as usize),
                Style::default().fg(if is_p { Color::Yellow } else { HEADER })));
        }
        put(f, Line::from(top_spans), y);
        put(f, Line::from(mid_spans), y + 1);
        put(f, Line::from(lbl_spans), y + 2);
        y += 3;

        // Controls
        let mut ctrl: Vec<Span> = vec![Span::raw("  ")];
        let (en_lbl, en_style) = if entry.enabled {
            (" ON ", Style::default().fg(Color::Black).bg(Color::Rgb(56, 200, 100)))
        } else {
            (" OFF ", Style::default().fg(Color::Rgb(180, 185, 195)).bg(PANEL))
        };
        ctrl.push(Span::styled(en_lbl, en_style));
        ctrl.push(Span::raw(" "));
        if fx_slot > 0 {
            ctrl.push(Span::styled(" <-MOVE ", Style::default().fg(Color::Rgb(150, 195, 245)).bg(PANEL)));
            ctrl.push(Span::raw(" "));
        }
        if fx_slot + 1 < chain.len() {
            ctrl.push(Span::styled(" MOVE-> ", Style::default().fg(Color::Rgb(150, 195, 245)).bg(PANEL)));
            ctrl.push(Span::raw(" "));
        }
        ctrl.push(Span::styled(" DEL ", Style::default().fg(Color::White).bg(Color::Rgb(170, 50, 50))));
        put(f, Line::from(ctrl), y);
    } else if chain.is_empty() {
        put(f, Line::from(Span::styled(
            "  No FX — press 'a' to add one, or select from list",
            Style::default().fg(Color::DarkGray))), y);
    }
}
