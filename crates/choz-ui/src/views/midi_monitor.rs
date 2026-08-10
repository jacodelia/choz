//! MIDI monitor panel — the last few messages that reached choz.
//!
//! Answers "is the keyboard actually talking to choz?" without leaving the app:
//! notes, pedals and wheels show up here the moment they arrive. Only real
//! input traffic (MIDI ports, OSC) passes through — the QWERTY piano drives the
//! engine directly and is deliberately not logged as MIDI.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use choz_engine::input::{InputEvent, InputSource};

use super::theme::{self, ACCENT, DIM, HEADER, OK, WARN};

/// Note names, sharps only — flats would need a key signature to choose.
const NOTE_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// Scientific pitch: MIDI 60 is C4, so octave = note/12 - 1.
pub fn note_name(note: u8) -> String {
    format!("{}{}", NOTE_NAMES[note as usize % 12], note as i16 / 12 - 1)
}

/// The common controllers, so the monitor says "SUSTAIN" instead of "CC 64"
/// when the user steps on a pedal.
fn cc_name(cc: u8) -> Option<&'static str> {
    Some(match cc {
        0 => "BANK MSB",
        32 => "BANK LSB",
        1 => "MOD WHEEL",
        7 => "VOLUME",
        11 => "EXPRESSION",
        64 => "SUSTAIN",
        66 => "SOSTENUTO",
        67 => "SOFT",
        _ => return None,
    })
}

/// Where the message came from, short enough to sit in a narrow panel.
fn source_label(source: InputSource, ports: &[String]) -> String {
    match source {
        InputSource::Midi(i) => ports
            .get(i)
            .map(|n| n.chars().take(14).collect())
            .unwrap_or_else(|| format!("port {i}")),
        InputSource::Osc => "OSC".to_string(),
        InputSource::Keyboard => "keyboard".to_string(),
    }
}

/// One log line: `<source>  <what>  <detail>`, coloured by message type.
fn line(event: &InputEvent, ports: &[String]) -> Line<'static> {
    let (source, what, detail, colour) = match event {
        InputEvent::Note(m) if m.on => (
            m.source,
            "NOTE ON".to_string(),
            format!("{:<4} vel {}", note_name(m.note), m.vel),
            OK,
        ),
        InputEvent::Note(m) => (m.source, "NOTE OFF".to_string(), note_name(m.note), DIM),
        InputEvent::Cc(m) => (
            m.source,
            cc_name(m.cc).map(str::to_string).unwrap_or_else(|| format!("CC {}", m.cc)),
            m.value.to_string(),
            ACCENT,
        ),
        InputEvent::Program(m) => (
            m.source,
            "PROGRAM".to_string(),
            format!("{} bank {}", m.program, m.bank),
            OK,
        ),
        InputEvent::Bend(m) => (
            m.source,
            "PITCH BEND".to_string(),
            // Shown relative to the centre detent, which is how it reads on a wheel.
            format!("{:+}", m.value as i32 - 8192),
            WARN,
        ),
        // OSC control messages are remote UI actions, not performance data.
        InputEvent::Control(_) => (
            InputSource::Osc,
            "CONTROL".to_string(),
            String::new(),
            theme::HINT,
        ),
    };

    Line::from(vec![
        Span::styled(format!("{:<15}", source_label(source, ports)), Style::default().fg(DIM)),
        Span::styled(format!("{what:<11}"), Style::default().fg(colour).add_modifier(Modifier::BOLD)),
        Span::styled(detail, Style::default().fg(theme::text())),
    ])
}

/// What the monitor is showing: the messages, or the sound they made.
///
/// seqterm has these as sidebar tabs; here they share the MIDI panel, because
/// "did the note arrive" and "did anything come out" are the same question asked
/// twice, and answering the second one needs no MIDI at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MonitorTab {
    #[default]
    Midi,
    /// The output's shape, as a travelling window.
    Wave,
    /// How loud it is: peak and RMS, held and decaying.
    Activity,
}

impl MonitorTab {
    pub const ALL: [MonitorTab; 3] = [MonitorTab::Midi, MonitorTab::Wave, MonitorTab::Activity];

    pub fn label(self) -> &'static str {
        match self {
            MonitorTab::Midi => "MIDI",
            MonitorTab::Wave => "WAVE",
            MonitorTab::Activity => "ACTIVITY",
        }
    }

    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|t| *t == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
}

/// Draw the monitor. `events` is oldest-first; the newest ones are shown at the
/// bottom, so the eye follows new arrivals downward like a terminal log.
pub fn draw_midi_monitor(
    f: &mut Frame,
    area: Rect,
    events: &[InputEvent],
    ports: &[String],
    tab: MonitorTab,
) -> Vec<(MonitorTab, Rect)> {
    let block = Block::default()
        .title(" MIDI IN ")
        .title_style(Style::default().fg(HEADER).add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::border()))
        .style(super::theme::panel_style());

    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 {
        return Vec::new();
    }

    // The tab strip takes the first row; the rest is whichever tab is showing.
    let mut rects = Vec::new();
    let mut spans: Vec<Span> = Vec::new();
    let mut x = inner.x;
    for t in MonitorTab::ALL {
        let text = format!(" {} ", t.label());
        let w = text.chars().count() as u16;
        rects.push((t, Rect::new(x, inner.y, w, 1)));
        let style = if t == tab {
            Style::default().fg(ratatui::style::Color::Black).bg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(HEADER)
        };
        spans.push(Span::styled(text, style));
        spans.push(Span::styled("\u{2502}", Style::default().fg(DIM)));
        x += w + 1;
    }
    f.render_widget(
        Paragraph::new(Line::from(spans)).style(super::theme::panel_style()),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    let inner = Rect::new(inner.x, inner.y + 1, inner.width, inner.height.saturating_sub(1));
    if inner.height == 0 {
        return rects;
    }

    match tab {
        MonitorTab::Wave => {
            draw_wave(f, inner);
            return rects;
        }
        MonitorTab::Activity => {
            draw_activity(f, inner);
            return rects;
        }
        MonitorTab::Midi => {}
    }

    let rows = inner.height as usize;
    let lines: Vec<Line> = if events.is_empty() {
        vec![Line::from(Span::styled(
            "waiting for MIDI…",
            Style::default().fg(DIM).add_modifier(Modifier::ITALIC),
        ))]
    } else {
        events[events.len().saturating_sub(rows)..]
            .iter()
            .map(|e| line(e, ports))
            .collect()
    };

    f.render_widget(Paragraph::new(lines).style(super::theme::panel_style()), inner);
    rects
}

/// The output's shape: a window of the mixed signal, oldest on the left.
///
/// Half-blocks, so one row of cells carries two rows of resolution — the same
/// trick the wallpaper uses, and the reason this reads as a wave rather than as
/// a bar chart.
fn draw_wave(f: &mut Frame, area: Rect) {
    let wave = choz_engine::meter::meter().wave();
    let rows = area.height as usize;
    let cols = area.width as usize;
    if rows == 0 || cols == 0 {
        return;
    }
    // Auto-gain so a quiet signal is still a picture, with a floor so silence
    // does not become noise magnified to full scale.
    let peak = wave.iter().fold(0.02f32, |m, s| m.max(s.abs()));
    let mid = (rows * 2) as f32 / 2.0;

    let mut lines: Vec<Line> = Vec::with_capacity(rows);
    for row in 0..rows {
        let spans: Vec<Span> = (0..cols)
            .map(|col| {
                let s = wave[col * wave.len() / cols.max(1)] / peak;
                let y = mid - s * (mid - 1.0);
                // The two half-cells this character covers.
                let (top, bottom) = ((row * 2) as f32, (row * 2 + 1) as f32);
                let hit = |cell: f32| (y - cell).abs() < 1.0;
                let ch = match (hit(top), hit(bottom)) {
                    (true, true) => '\u{2588}',
                    (true, false) => '\u{2580}',
                    (false, true) => '\u{2584}',
                    // The centre line, so a silent signal is still a signal.
                    _ if (row * 2 + 1) as f32 == mid.floor() => '\u{2500}',
                    _ => ' ',
                };
                let colour = if ch == '\u{2500}' { DIM } else { ACCENT };
                Span::styled(ch.to_string(), Style::default().fg(colour))
            })
            .collect();
        lines.push(Line::from(spans));
    }
    f.render_widget(Paragraph::new(lines).style(super::theme::panel_style()), area);
}

/// How loud it is: peak and RMS as two bars, with the numbers in dB.
fn draw_activity(f: &mut Frame, area: Rect) {
    let m = choz_engine::meter::meter();
    let (peak, rms) = (m.peak(), m.rms());
    // Six columns for the label, ten for the reading, and the bar gets the rest
    // — the first version gave the bar too much and pushed the dB off the edge.
    let width = area.width.saturating_sub(17) as usize;
    let bar = |v: f32| -> String {
        // dBFS over 60 dB, because a linear meter is all top and no bottom.
        let db = if v > 1e-6 { 20.0 * v.log10() } else { -60.0 };
        let filled = (((db + 60.0) / 60.0).clamp(0.0, 1.0) * width as f32).round() as usize;
        format!("{}{}", "\u{2588}".repeat(filled), "\u{2591}".repeat(width.saturating_sub(filled)))
    };
    let db_text = |v: f32| -> String {
        // Always in dB, silence included: "-inf dB" is a reading, "-inf" alone
        // looks like a missing unit.
        if v > 1e-6 { format!("{:>6.1} dB", 20.0 * v.log10()) } else { "  -inf dB".to_string() }
    };
    let colour = |v: f32| {
        if v >= 0.99 {
            WARN
        } else if v > 0.7 {
            OK
        } else {
            ACCENT
        }
    };

    let lines = vec![
        Line::from(vec![
            Span::styled(" PEAK ", Style::default().fg(HEADER)),
            Span::styled(bar(peak), Style::default().fg(colour(peak))),
            Span::styled(db_text(peak), Style::default().fg(theme::text())),
        ]),
        Line::from(vec![
            Span::styled(" RMS  ", Style::default().fg(HEADER)),
            Span::styled(bar(rms), Style::default().fg(colour(rms))),
            Span::styled(db_text(rms), Style::default().fg(theme::text())),
        ]),
        Line::from(Span::styled(
            if peak >= 0.99 { "  CLIPPING" } else { "" },
            Style::default().fg(WARN).add_modifier(Modifier::BOLD),
        )),
    ];
    f.render_widget(Paragraph::new(lines).style(super::theme::panel_style()), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use choz_engine::input::{BendMsg, CcMsg, NoteMsg};

    #[test]
    fn note_names_follow_scientific_pitch() {
        assert_eq!(note_name(60), "C4", "middle C");
        assert_eq!(note_name(69), "A4", "concert A");
        assert_eq!(note_name(0), "C-1", "lowest MIDI note");
        assert_eq!(note_name(127), "G9", "highest MIDI note");
        assert_eq!(note_name(61), "C#4");
    }

    /// The panel is only useful if a glance tells you *which* pedal moved.
    #[test]
    fn lines_name_the_pedals_and_centre_the_bend() {
        let ports = vec!["Keystation Pro 88".to_string()];
        let src = InputSource::Midi(0);
        let text = |e: InputEvent| {
            line(&e, &ports).spans.iter().map(|s| s.content.to_string()).collect::<String>()
        };

        let sustain = text(InputEvent::Cc(CcMsg { source: src, channel: 0, cc: 64, value: 127 }));
        assert!(sustain.contains("SUSTAIN"), "got {sustain:?}");
        assert!(sustain.contains("Keystation"), "port is named: {sustain:?}");

        let unknown = text(InputEvent::Cc(CcMsg { source: src, channel: 0, cc: 23, value: 5 }));
        assert!(unknown.contains("CC 23"), "unnamed controllers fall back to a number");

        // A wheel at rest reads 0, not 8192.
        let centre = text(InputEvent::Bend(BendMsg { source: src, value: 8192 }));
        assert!(centre.contains("+0"), "got {centre:?}");
        let down = text(InputEvent::Bend(BendMsg { source: src, value: 0 }));
        assert!(down.contains("-8192"), "got {down:?}");

        let note = text(InputEvent::Note(NoteMsg { source: src, channel: 0, on: true, note: 60, vel: 100 }));
        assert!(note.contains("C4") && note.contains("vel 100"), "got {note:?}");
    }

    /// An unknown port index must not panic the draw path.
    #[test]
    fn missing_port_names_degrade_to_an_index() {
        let label = source_label(InputSource::Midi(7), &[]);
        assert_eq!(label, "port 7");
    }

    fn render(events: &[InputEvent], ports: &[String], w: u16, h: u16) -> String {
        render_tab(events, ports, w, h, MonitorTab::Midi)
    }

    fn render_tab(
        events: &[InputEvent],
        ports: &[String],
        w: u16,
        h: u16,
        tab: MonitorTab,
    ) -> String {
        use ratatui::{backend::TestBackend, Terminal};
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| {
            draw_midi_monitor(f, f.area(), events, ports, tab);
        })
        .unwrap();
        // One string per row, so assertions can talk about lines.
        let buf = term.backend().buffer().clone();
        (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn shows_the_newest_messages_and_drops_the_oldest() {
        let ports = vec!["Keystation Pro 88".to_string()];
        let src = InputSource::Midi(0);
        // More messages than the 6 inner rows of an 8-row panel.
        let events: Vec<InputEvent> = (0..20)
            .map(|i| InputEvent::Note(NoteMsg { source: src, channel: 0, on: true, note: 40 + i, vel: 100 }))
            .collect();

        let screen = render(&events, &ports, 50, 8);
        assert!(screen.contains("MIDI IN"), "panel is titled:\n{screen}");
        assert!(screen.contains(&note_name(59)), "newest message is shown:\n{screen}");
        assert!(!screen.contains(&note_name(40)), "oldest scrolled off:\n{screen}");
    }

    #[test]
    fn says_it_is_waiting_when_nothing_has_arrived() {
        let screen = render(&[], &[], 40, 8);
        assert!(screen.contains("waiting for MIDI"), "got:\n{screen}");
    }

    /// A panel too short for a single row must not panic or overflow.
    #[test]
    fn survives_being_squeezed_to_nothing() {
        let src = InputSource::Midi(0);
        let e = [InputEvent::Note(NoteMsg { source: src, channel: 0, on: true, note: 60, vel: 100 })];
        for h in 0..4 {
            render(&e, &[], 30, h);
        }
    }

    /// Three tabs, and each one shows its own thing. WAVE and ACTIVITY read the
    /// engine's meter, so with no audio running they draw an empty picture
    /// rather than nothing at all — "no signal" is information.
    #[test]
    fn the_monitor_has_three_tabs_and_each_draws_its_own() {
        choz_engine::meter::meter().clear();
        let midi = render_tab(&[], &[], 60, 10, MonitorTab::Midi);
        assert!(midi.contains("MIDI") && midi.contains("WAVE") && midi.contains("ACTIVITY"));
        assert!(midi.contains("waiting for MIDI"), "the MIDI tab is the messages");

        let wave = render_tab(&[], &[], 60, 10, MonitorTab::Wave);
        assert!(!wave.contains("waiting for MIDI"), "a different tab, different content");
        assert!(wave.contains('\u{2500}'), "silence still draws its centre line");

        // A block through the meter and the wave has something in it.
        let buf: Vec<f32> = (0..512)
            .flat_map(|i| {
                let s = 0.8 * (2.0 * std::f32::consts::PI * i as f32 / 32.0).sin();
                [s, s]
            })
            .collect();
        choz_engine::meter::meter().publish(&buf);
        let wave = render_tab(&[], &[], 60, 10, MonitorTab::Wave);
        assert!(
            wave.chars().any(|c| c == '\u{2588}' || c == '\u{2580}' || c == '\u{2584}'),
            "the shape of the sound: {wave}"
        );

        let activity = render_tab(&[], &[], 60, 10, MonitorTab::Activity);
        assert!(activity.contains("PEAK") && activity.contains("RMS"), "{activity}");
        assert!(activity.contains("dB"), "levels in dB, not in fractions: {activity}");
        choz_engine::meter::meter().clear();
    }

    /// The tabs are clickable, which means they have to hand back where they
    /// were drawn.
    #[test]
    fn the_tab_strip_reports_its_rects() {
        use ratatui::{backend::TestBackend, Terminal};
        let mut term = Terminal::new(TestBackend::new(60, 10)).unwrap();
        let mut rects = Vec::new();
        term.draw(|f| {
            rects = draw_midi_monitor(f, f.area(), &[], &[], MonitorTab::Midi);
        })
        .unwrap();
        assert_eq!(rects.len(), 3);
        assert_eq!(rects[0].0, MonitorTab::Midi);
        assert!(rects[1].1.x > rects[0].1.x, "left to right");
        assert!(rects.iter().all(|(_, r)| r.height == 1));

        assert_eq!(MonitorTab::Midi.next(), MonitorTab::Wave);
        assert_eq!(MonitorTab::Activity.next(), MonitorTab::Midi, "it wraps");
    }
}
