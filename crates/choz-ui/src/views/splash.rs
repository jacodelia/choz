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
    // The box was 24 rows tall with eleven of content, so most of it was empty.
    // What fills it now says something: what choz hosts, and a moving line that
    // makes the wait look like an instrument warming up rather than a freeze.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(LOGO_LINES as u16), // logo
            Constraint::Length(LOGO_LINES as u16 / 2), // its reflection
            Constraint::Length(1),                 // tagline + version
            Constraint::Length(1),                 // the formats it hosts
            Constraint::Length(1),                 // spacer
            Constraint::Length(1),                 // wave
            Constraint::Length(1),                 // spinner + status
            Constraint::Length(1),                 // progress
            Constraint::Length(1),                 // hint
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
                    Color::Rgb(
                        color_to_u8(color, 0).saturating_add(20),
                        color_to_u8(color, 1).saturating_add(20),
                        color_to_u8(color, 2).saturating_add(20),
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

    // ─── Reflection ─────────────────────────────────────────────────────
    // The logo again, upside down and fading: two rows of "this is a screen with
    // something on it" instead of two rows of nothing.
    let mut mirror: Vec<Line> = Vec::new();
    for (i, logo_line) in LOGO.iter().rev().take(LOGO_LINES / 2).enumerate() {
        let fade = 1.0 - (i as f32 + 1.0) / (LOGO_LINES as f32 / 2.0 + 1.0);
        let spans: Vec<Span> = logo_line
            .chars()
            .enumerate()
            .map(|(ci, ch)| {
                if ch == ' ' {
                    return Span::raw(" ");
                }
                let color = SPLASH_GRADIENT[((ci as u64 + logo_tick) % 12) as usize];
                let dim = Color::Rgb(
                    (color_to_u8(color, 0) as f32 * fade * 0.45) as u8,
                    (color_to_u8(color, 1) as f32 * fade * 0.45) as u8,
                    (color_to_u8(color, 2) as f32 * fade * 0.45) as u8,
                );
                Span::styled(ch.to_string(), Style::default().fg(dim))
            })
            .collect();
        mirror.push(Line::from(spans));
    }
    f.render_widget(
        Paragraph::new(mirror).style(Style::default().bg(PANEL_BG)),
        chunks[1],
    );

    // ─── Tagline ────────────────────────────────────────────────────────
    let tagline = Line::from(vec![
        Span::styled("  choz ", Style::default().fg(HEADER).add_modifier(Modifier::BOLD)),
        Span::styled(
            concat!("v", env!("CARGO_PKG_VERSION")),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  \u{00B7}  terminal audio plugin host", Style::default().fg(DIM)),
    ]);
    f.render_widget(
        Paragraph::new(tagline).style(Style::default().bg(PANEL_BG)),
        chunks[2],
    );

    // ─── What it hosts ──────────────────────────────────────────────────
    // Each badge lights up in turn: the six formats are the reason choz exists,
    // and the sweep is the same clock as the logo's.
    let mut badges: Vec<Span> = vec![Span::raw("  ")];
    for (i, name) in FORMATS.iter().enumerate() {
        let lit = (logo_tick as usize % (FORMATS.len() * 2)) == i;
        let style = if lit {
            Style::default().fg(Color::Black).bg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(HINT)
        };
        badges.push(Span::styled(format!(" {name} "), style));
        badges.push(Span::raw(" "));
    }
    f.render_widget(
        Paragraph::new(Line::from(badges)).style(Style::default().bg(PANEL_BG)),
        chunks[3],
    );

    // ─── A line that moves ──────────────────────────────────────────────
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            splash_wave(state.tick, chunks[5].width as usize),
            Style::default().fg(ACCENT),
        )))
        .style(Style::default().bg(PANEL_BG)),
        chunks[5],
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
        chunks[6],
    );

    // ─── Progress bar ───────────────────────────────────────────────────
    let progress = if state.ready { 1.0 } else { (state.tick as f64 / 60.0).min(1.0) };
    let gauge = Gauge::default()
        .gauge_style(Style::default().fg(ACCENT).bg(PANEL_BG))
        .ratio(progress);
    f.render_widget(gauge, chunks[7]);

    // ─── Hint ───────────────────────────────────────────────────────────
    let hint = Line::from(Span::styled(
        "  Press any key to skip",
        Style::default().fg(DIM),
    ));
    f.render_widget(
        Paragraph::new(hint).style(Style::default().bg(PANEL_BG)),
        chunks[8],
    );
}

/// The formats choz hosts, which is the short answer to what it is.
const FORMATS: [&str; 6] = ["CLAP", "LV2", "VST2", "VST3", "LADSPA", "DSSI"];

/// One row of travelling sine, drawn with the eighth-blocks the knob arc uses.
///
/// Deterministic in `tick` and `width` so it can be tested without a terminal —
/// an animation nobody can check is an animation that breaks quietly.
pub fn splash_wave(tick: u64, width: usize) -> String {
    const LEVELS: [char; 8] = ['\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}', '\u{2585}', '\u{2586}', '\u{2587}', '\u{2588}'];
    (0..width)
        .map(|x| {
            // Two sines of different periods, so the crest travels instead of
            // pulsing in place.
            let t = tick as f32 * 0.18;
            let a = ((x as f32 * 0.28 - t).sin() + (x as f32 * 0.11 + t * 0.6).sin()) * 0.5;
            let level = ((a * 0.5 + 0.5) * (LEVELS.len() - 1) as f32).round() as usize;
            LEVELS[level.min(LEVELS.len() - 1)]
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The wave has to be exactly as wide as it is asked for — a splash row that
    /// overflows pushes the box open — and it has to actually travel.
    #[test]
    fn the_splash_wave_fits_its_row_and_moves() {
        for w in [0, 1, 40, 76] {
            assert_eq!(splash_wave(0, w).chars().count(), w);
        }
        assert_ne!(splash_wave(0, 40), splash_wave(7, 40), "it moves with the tick");
        // Every glyph is one of the eighth blocks, so the row is one cell tall.
        assert!(splash_wave(3, 40).chars().all(|c| ('\u{2581}'..='\u{2588}').contains(&c)));
    }

    /// The box is 24 rows; the content used to be eleven of them. What fills the
    /// rest is listed here so a layout change has to face the question.
    #[test]
    fn the_splash_says_what_choz_is() {
        assert_eq!(FORMATS.len(), 6, "the six formats choz hosts");
        assert!(FORMATS.contains(&"CLAP") && FORMATS.contains(&"VST3"));
        assert!(!env!("CARGO_PKG_VERSION").is_empty(), "the version is shown on it");
    }
}
