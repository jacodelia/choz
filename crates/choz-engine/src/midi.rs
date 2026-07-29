//! Hardware MIDI input via midir.
//!
//! The midir callback runs on its own thread, so it can't touch the engine's
//! single-producer note ring directly. It forwards parsed note events over a
//! `flume` channel; the UI loop drains that channel and calls `engine.note_on`
//! (the sole producer of the RT note ring).

pub use crate::input::{CcMsg, InputEvent, InputSource, NoteMsg};

/// All available MIDI **input** port names (what devices we can listen to).
pub fn list_input_ports() -> Vec<String> {
    match midir::MidiInput::new("choz-scan-in") {
        Ok(m) => m.ports().iter().filter_map(|p| m.port_name(p).ok()).collect(),
        Err(_) => Vec::new(),
    }
}

/// All available MIDI **output** port names (shown for reference, like Carla).
pub fn list_output_ports() -> Vec<String> {
    match midir::MidiOutput::new("choz-scan-out") {
        Ok(m) => m.ports().iter().filter_map(|p| m.port_name(p).ok()).collect(),
        Err(_) => Vec::new(),
    }
}

/// Connect to every MIDI input port except those named in `disabled`, so any
/// plugged-in controller drives the synth (Carla-style — no manual wiring
/// needed). Returns the connected port names and their live connections (which
/// must be kept alive). Each port needs its own `MidiInput` because `connect`
/// consumes it.
pub fn connect_inputs(
    tx: flume::Sender<InputEvent>,
    disabled: &[String],
) -> (Vec<String>, Vec<midir::MidiInputConnection<()>>) {
    let mut names = Vec::new();
    let mut conns = Vec::new();

    let Ok(scan) = midir::MidiInput::new("choz-scan") else {
        return (names, conns);
    };
    let wanted: Vec<String> = scan
        .ports()
        .iter()
        .filter_map(|p| scan.port_name(p).ok())
        .filter(|n| !disabled.contains(n))
        .collect();

    for want in wanted {
        // Fresh MidiInput per port; re-find the port by name on this instance.
        let Ok(mi) = midir::MidiInput::new("choz-in") else { continue };
        let Some(port) = mi.ports().into_iter().find(|p| mi.port_name(p).as_deref() == Ok(want.as_str())) else {
            continue;
        };
        let txc = tx.clone();
        // Each connection tags its events with its index in `names`, which is
        // the list the caller gets back — so index ↔ port name stay aligned.
        let source = InputSource::Midi(names.len());
        match mi.connect(
            &port,
            "choz-in-conn",
            move |_ts, data, _| {
                match parse(data) {
                    Some(Msg::Note { on, note, vel }) => {
                        let _ = txc.send(InputEvent::Note(NoteMsg { source, on, note, vel }));
                    }
                    // Control changes drive MIDI-learn bindings (rack faders).
                    Some(Msg::Cc { cc, value }) => {
                        let _ = txc.send(InputEvent::Cc(CcMsg { source, cc, value }));
                    }
                    None => {}
                }
            },
            (),
        ) {
            Ok(c) => {
                names.push(want);
                conns.push(c);
            }
            Err(e) => eprintln!("choz: MIDI connect '{want}' failed: {e}"),
        }
    }
    (names, conns)
}

/// A raw MIDI message choz cares about.
#[derive(Debug, PartialEq, Eq)]
enum Msg {
    Note { on: bool, note: u8, vel: u8 },
    Cc { cc: u8, value: u8 },
}

/// Parse a raw MIDI message. Note-on with velocity 0 is the conventional
/// note-off. Returns `None` for anything that is neither a note nor a CC.
fn parse(data: &[u8]) -> Option<Msg> {
    if data.len() < 3 {
        return None;
    }
    match data[0] & 0xF0 {
        0x90 if data[2] > 0 => Some(Msg::Note { on: true, note: data[1], vel: data[2] }),
        0x80 | 0x90 => Some(Msg::Note { on: false, note: data[1], vel: 0 }),
        0xB0 => Some(Msg::Cc { cc: data[1], value: data[2] }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_note_on_off_and_ignores_others() {
        assert_eq!(parse(&[0x90, 60, 100]), Some(Msg::Note { on: true, note: 60, vel: 100 }));
        assert_eq!(parse(&[0x90, 60, 0]), Some(Msg::Note { on: false, note: 60, vel: 0 }), "vel0 = note-off");
        assert_eq!(parse(&[0x80, 60, 40]), Some(Msg::Note { on: false, note: 60, vel: 0 }));
        assert_eq!(parse(&[0xB0, 7, 100]), Some(Msg::Cc { cc: 7, value: 100 }), "CC drives MIDI learn");
        assert_eq!(parse(&[0xF8]), None, "clock is neither");
        assert_eq!(parse(&[0x90, 60]), None, "truncated");
    }
}
