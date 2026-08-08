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

/// Draw the monitor. `events` is oldest-first; the newest ones are shown at the
/// bottom, so the eye follows new arrivals downward like a terminal log.
pub fn draw_midi_monitor(f: &mut Frame, area: Rect, events: &[InputEvent], ports: &[String]) {
    let block = Block::default()
        .title(" MIDI IN ")
        .title_style(Style::default().fg(HEADER).add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::border()))
        .style(super::theme::panel_style());

    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 {
        return;
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
        use ratatui::{backend::TestBackend, Terminal};
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw_midi_monitor(f, f.area(), events, ports)).unwrap();
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
}
