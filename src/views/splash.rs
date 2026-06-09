//! Splash screen — animated logo with spinner, progress bar, and auto-dismiss.
//!
//! Adapted from seqterm's splash screen pattern.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, BorderType, Clear, Gauge, Paragraph},
    Frame,
};

use super::theme::*;

const LOGO: &[&str] = &[
    "   ██████╗  ██╗  ██╗  ██████╗  ███████╗",
    "  ██╔════╝  ██║  ██║ ██╔═══██╗ ╚════██║",
    "  ██║       ███████║ ██║   ██║    ██╔╝",
    "  ██║       ██╔══██║ ██║   ██║   ██╔╝ ",
    "  ╚██████╗  ██║  ██║ ╚██████╔╝  ███████╗",
    "   ╚═════╝  ╚═╝  ╚═╝  ╚═════╝  ╚══════╝",
];
const LOGO_LINES: usize = LOGO.len();

/// Splash animation state.
pub struct SplashState {
    pub tick: u64,
    pub ready: bool,
    pub dismiss_at: Option<std::time::Instant>,
}

impl SplashState {
    pub fn new() -> Self {
        Self { tick: 0, ready: false, dismiss_at: None }
    }

    pub fn dismiss(&mut self) {
        self.ready = true;
        self.dismiss_at = Some(std::time::Instant::now() + std::time::Duration::from_millis(800));
    }
}

/// Returns true when the splash should still be shown.
pub fn is_active(state: &SplashState) -> bool {
    match state.dismiss_at {
        Some(deadline) => std::time::Instant::now() < deadline,
        None => true,
    }
}

/// Draw the splash screen overlay.
pub fn draw_splash(f: &mut Frame, state: &SplashState, area: Rect) {
    // Opaque backdrop
    f.render_widget(Clear, area);
    f.render_widget(
        Block::default().style(Style::default().bg(BACKDROP)),
        area,
    );

    let modal = centered_fixed(76, 24, area);

    // Drop shadow
    draw_shadow(f, modal, area);

    // Modal background
    f.render_widget(Clear, modal);
    let border_color = if state.ready { OK } else { BORDER_LT };
    let block = Block::default()
        .title(" choz ")
        .title_style(Style::default().fg(HEADER).add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .style(Style::default().bg(PANEL_BG));
    let inner = block.inner(modal);
    f.render_widget(block, modal);

    // Layout
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(LOGO_LINES as u16),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    // ─── Logo ──────────────────────────────────────────────────────────
    let logo_tick = state.tick / 3;
    let mut logo_lines: Vec<Line> = Vec::new();
    for logo_line in LOGO.iter() {
        let mut spans: Vec<Span> = Vec::new();
        for (ci, ch) in logo_line.chars().enumerate() {
            if ch == ' ' {
                spans.push(Span::raw(" "));
            } else {
                let grad_idx = ((ci as u64 + logo_tick) % 12) as usize;
                let color = SPLASH_GRADIENT[grad_idx];
                let pulse = (state.tick % 16) < 8;
                let c = if pulse {
                    #[allow(clippy::unnecessary_min_or_max)]
                    Color::Rgb(
                        (color_to_u8(color, 0) + 20).min(255),
                        (color_to_u8(color, 1) + 20).min(255),
                        (color_to_u8(color, 2) + 20).min(255),
                    )
                } else {
                    color
                };
                spans.push(Span::styled(
                    ch.to_string(),
                    Style::default().fg(c).add_modifier(Modifier::BOLD),
                ));
            }
        }
        logo_lines.push(Line::from(spans));
    }
    f.render_widget(
        Paragraph::new(logo_lines).style(Style::default().bg(PANEL_BG)),
        chunks[0],
    );

    // ─── Tagline ────────────────────────────────────────────────────────
    let tagline = Line::from(Span::styled(
        "  choz — terminal audio plugin host",
        Style::default().fg(DIM),
    ));
    f.render_widget(
        Paragraph::new(tagline).style(Style::default().bg(PANEL_BG)),
        chunks[1],
    );

    // ─── Spinner + status ───────────────────────────────────────────────
    let spinner_char = if state.ready {
        '✓'
    } else {
        SPINNER[(state.tick as usize) % SPINNER.len()]
    };
    let spinner_color = if state.ready {
        let pulse = (state.tick % 10) < 5;
        if pulse { Color::Rgb(70, 210, 110) } else { Color::Rgb(40, 160, 75) }
    } else {
        HEADER
    };
    let status_text = if state.ready {
        "  Ready — starting..."
    } else {
        "  Initialising audio engine..."
    };
    let status_line = Line::from(vec![
        Span::styled(
            format!("  {spinner_char}"),
            Style::default().fg(spinner_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(status_text, Style::default().fg(HINT)),
    ]);
    f.render_widget(
        Paragraph::new(status_line).style(Style::default().bg(PANEL_BG)),
        chunks[3],
    );

    // ─── Progress bar ───────────────────────────────────────────────────
    let progress = if state.ready { 1.0 } else { (state.tick as f64 / 60.0).min(1.0) };
    let gauge = Gauge::default()
        .gauge_style(Style::default().fg(ACCENT).bg(PANEL_BG))
        .ratio(progress);
    f.render_widget(gauge, chunks[4]);

    // ─── Hint ───────────────────────────────────────────────────────────
    let hint = Line::from(Span::styled(
        "  Press any key to skip",
        Style::default().fg(DIM),
    ));
    f.render_widget(
        Paragraph::new(hint).style(Style::default().bg(PANEL_BG)),
        chunks[5],
    );
}

// ─── Helpers ───────────────────────────────────────────────────────────────

fn centered_fixed(w: u16, h: u16, area: Rect) -> Rect {
    let cw = w.min(area.width);
    let ch = h.min(area.height);
    let x = area.x + area.width.saturating_sub(cw) / 2;
    let y = area.y + area.height.saturating_sub(ch) / 2;
    Rect::new(x, y, cw, ch)
}

/// 1px offset drop shadow (seqterm style).
fn draw_shadow(f: &mut Frame, modal: Rect, screen: Rect) {
    let sx = (modal.x + 1).min(screen.x + screen.width.saturating_sub(1));
    let sy = (modal.y + 1).min(screen.y + screen.height.saturating_sub(1));
    let sw = modal.width.min(screen.width - (sx - screen.x));
    let sh = modal.height.min(screen.height - (sy - screen.y));

    if sw > 0 && sh > 0 {
        f.render_widget(
            Block::default().style(Style::default().bg(SHADOW)),
            Rect::new(sx, sy, sw, sh),
        );
    }
}

fn color_to_u8(c: Color, channel: usize) -> u8 {
    match c {
        Color::Rgb(r, g, b) => match channel {
            0 => r, 1 => g, _ => b,
        },
        _ => 0,
    }
}
