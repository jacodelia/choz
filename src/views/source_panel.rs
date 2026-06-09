//! SOURCE panel view — select audio source per channel.
//! Based on the PATTERN view's SourcePicker.

use ratatui::{
    layout::Rect, style::{Color, Modifier, Style}, text::{Line, Span},
    widgets::{Block, Borders, Paragraph}, Frame,
};

use crate::source::AudioSource;

pub const SOURCE_CATEGORIES: [&str; 4] = ["MIDI", "SF2", "AUDIO", "SYNTH"];

const PANEL: Color = Color::Rgb(22, 27, 34);
const BORDER: Color = Color::Rgb(48, 54, 61);
const ACCENT: Color = Color::Rgb(31, 111, 235);
const HEADER: Color = Color::Rgb(240, 136, 62);

/// Draw the SOURCE selection panel.
pub fn draw_source_panel(
    f: &mut Frame,
    area: Rect,
    source: &AudioSource,
    selected: bool,
    source_cat: usize,
    midi_ports: &[String],
    synths: &[crate::SynthEntry],
) {
    let border_style = if selected {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(BORDER)
    };

    let title = if selected {
        " SOURCE [ACTIVE] ".to_string()
    } else {
        " SOURCE ".to_string()
    };

    let block = Block::default()
        .title(title)
        .title_style(Style::default().fg(HEADER))
        .borders(Borders::ALL)
        .border_style(border_style)
        .style(Style::default().bg(PANEL));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();

    // Current source
    lines.push(Line::from(vec![
        Span::styled(" Now: ", Style::default().fg(Color::DarkGray)),
        Span::styled(source.kind_label(), Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
    ]));

    lines.push(Line::from(""));

    // Categories
    let mut cat_spans = Vec::new();
    for (i, &cat) in SOURCE_CATEGORIES.iter().enumerate() {
        let style = if i == source_cat {
            Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        cat_spans.push(Span::styled(format!(" {cat} "), style));
    }
    lines.push(Line::from(cat_spans));

    // Content for selected category
    lines.push(Line::from(""));
    let category = SOURCE_CATEGORIES[source_cat];
    match category {
        "MIDI" => {
            lines.push(Line::from(Span::styled(" Available MIDI ports:", Style::default().fg(Color::DarkGray))));
            for (i, port) in midi_ports.iter().enumerate() {
                let sty = if matches!(source, AudioSource::Midi) && source_kind_is_type(source, i, midi_ports, &[], true) {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::White)
                };
                lines.push(Line::from(Span::styled(format!("   {i}: {port}"), sty)));
            }
            if midi_ports.is_empty() {
                lines.push(Line::from(Span::styled("   (no ports found)", Style::default().fg(Color::DarkGray))));
            }
        }
        "SF2" => {
            lines.push(Line::from(Span::styled(" Browse SF2 file...", Style::default().fg(Color::White))));
        }
        "AUDIO" => {
            lines.push(Line::from(Span::styled(" Browse audio file...", Style::default().fg(Color::White))));
        }
        "SYNTH" => {
            if synths.is_empty() {
                lines.push(Line::from(Span::styled(" (no plugins found — run scan)", Style::default().fg(Color::DarkGray))));
            } else {
                for s in synths.iter() {
                    lines.push(Line::from(Span::styled(
                        format!(" [{}] {}", s.format, s.name),
                        Style::default().fg(Color::White),
                    )));
                }
            }
        }
        _ => {}
    }

    // Status line
    lines.push(Line::from(""));
    let status = match source {
        AudioSource::Midi => "Output: MIDI passthrough",
        AudioSource::Sf2 { ref path, bank, preset } => &format!("Output: SF2 {} (bank {bank}, preset {preset})", path.display()),
        AudioSource::AudioFile { ref path, .. } => &format!("Output: Audio {}", path.display()),
        AudioSource::Plugin { ref name, ref format, .. } => &format!("Output: {name} [{format}]"),
    };
    lines.push(Line::from(Span::styled(status.to_string(), Style::default().fg(Color::DarkGray))));

    f.render_widget(Paragraph::new(lines).style(Style::default().bg(PANEL)), inner);
}

fn source_kind_is_type(
    source: &AudioSource,
    _i: usize,
    _midi_ports: &[String],
    _synths: &[crate::SynthEntry],
    is_midi: bool,
) -> bool {
    if is_midi && matches!(source, AudioSource::Midi) {
        return true;
    }
    if !is_midi && matches!(source, AudioSource::Plugin { .. }) {
        return true;
    }
    false
}
