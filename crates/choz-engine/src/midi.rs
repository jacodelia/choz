//! Hardware MIDI input via midir.
//!
//! The midir callback runs on its own thread, so it can't touch the engine's
//! single-producer note ring directly. It forwards parsed note events over a
//! `flume` channel; the UI loop drains that channel and calls `engine.note_on`
//! (the sole producer of the RT note ring).

pub use crate::input::{BendMsg, CcMsg, InputEvent, InputSource, NoteMsg, ProgramMsg};

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
        .filter(|n| !is_disabled(n, disabled))
        .collect();

    for want in wanted {
        // Fresh MidiInput per port; re-find the port by name on this instance.
        let Ok(mi) = midir::MidiInput::new("choz-in") else { continue };
        let Some(port) = mi.ports().into_iter().find(|p| mi.port_name(p).as_deref() == Ok(want.as_str())) else {
            continue;
        };
        let txc = tx.clone();
        // Bank Select arrives as its own CC just before the program change, so
        // the port's last MSB is remembered here to travel with it. ponytail:
        // MSB only — SF2 banks are 0..128 and the LSB is what this keyboard
        // (and most others) always leaves at 0.
        let mut bank = 0u8;
        // Each connection tags its events with its index in `names`, which is
        // the list the caller gets back — so index ↔ port name stay aligned.
        let source = InputSource::Midi(names.len());
        match mi.connect(
            &port,
            "choz-in-conn",
            move |_ts, data, _| {
                match parse(data) {
                    Some(Msg::Note { channel, on, note, vel }) => {
                        let _ =
                            txc.send(InputEvent::Note(NoteMsg { source, channel, on, note, vel }));
                    }
                    // Control changes drive MIDI-learn bindings (rack faders)
                    // and reach the instrument, which is what makes the pedals
                    // and the modulation wheel work.
                    Some(Msg::Cc { channel, cc, value }) => {
                        if cc == 0 {
                            bank = value;
                        }
                        let _ = txc.send(InputEvent::Cc(CcMsg { source, channel, cc, value }));
                    }
                    Some(Msg::Program { program }) => {
                        let _ = txc.send(InputEvent::Program(ProgramMsg { source, bank, program }));
                    }
                    Some(Msg::Bend { value }) => {
                        let _ = txc.send(InputEvent::Bend(BendMsg { source, value }));
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

/// Is this port switched off? midir names a port `"Client:Port n:m"`, so the
/// saved `"Midi Through"` never matched the full name and the loopback port
/// stayed connected. A saved entry disables the whole client when it names one.
fn is_disabled(port: &str, disabled: &[String]) -> bool {
    disabled.iter().any(|d| port == d || port.starts_with(&format!("{d}:")))
}

/// A raw MIDI message choz cares about.
#[derive(Debug, PartialEq, Eq)]
enum Msg {
    /// `channel` is 0-based, as it is on the wire. It only matters in the
    /// rack's multi-timbral mode, where one port drives several tabs at once —
    /// the way a sampler answers a DAW's orchestral template.
    Note { channel: u8, on: bool, note: u8, vel: u8 },
    Cc { channel: u8, cc: u8, value: u8 },
    /// Pitch bend, as the 14-bit value the wire carries: 0..16383, centred at
    /// 8192. Kept unsigned because that is what synths take.
    Bend { value: u16 },
    /// Program change — the buttons on a controller keyboard usually send these
    /// (preceded by a Bank Select pair), not CCs.
    Program { program: u8 },
}

/// Parse a raw MIDI message. Note-on with velocity 0 is the conventional
/// note-off. Returns `None` for anything choz has no use for (clock, aftertouch,
/// sysex).
fn parse(data: &[u8]) -> Option<Msg> {
    if data.len() < 2 {
        return None;
    }
    // Program change is the one two-byte message choz uses; everything below
    // needs the second data byte.
    if data[0] & 0xF0 == 0xC0 {
        return Some(Msg::Program { program: data[1] & 0x7F });
    }
    if data.len() < 3 {
        return None;
    }
    let channel = data[0] & 0x0F;
    match data[0] & 0xF0 {
        0x90 if data[2] > 0 => {
            Some(Msg::Note { channel, on: true, note: data[1], vel: data[2] })
        }
        0x80 | 0x90 => Some(Msg::Note { channel, on: false, note: data[1], vel: 0 }),
        0xB0 => Some(Msg::Cc { channel, cc: data[1], value: data[2] }),
        // LSB first, then MSB — both 7-bit.
        0xE0 => Some(Msg::Bend { value: (data[1] as u16 & 0x7F) | ((data[2] as u16 & 0x7F) << 7) }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_note_on_off_and_ignores_others() {
        assert_eq!(parse(&[0x90, 60, 100]), Some(Msg::Note { channel: 0, on: true, note: 60, vel: 100 }));
        assert_eq!(parse(&[0x90, 60, 0]), Some(Msg::Note { channel: 0, on: false, note: 60, vel: 0 }), "vel0 = note-off");
        assert_eq!(parse(&[0x80, 60, 40]), Some(Msg::Note { channel: 0, on: false, note: 60, vel: 0 }));
        assert_eq!(parse(&[0xB0, 7, 100]), Some(Msg::Cc { channel: 0, cc: 7, value: 100 }), "CC drives MIDI learn");
        assert_eq!(parse(&[0xF8]), None, "clock is neither");
        assert_eq!(parse(&[0x90, 60]), None, "truncated");
    }

    /// A Keystation Pro 88 button sends bank select then a two-byte program
    /// change. Requiring three bytes dropped the program change, so every
    /// button looked like the same CC 32.
    #[test]
    fn parses_two_byte_program_change() {
        assert_eq!(parse(&[0xC0, 13]), Some(Msg::Program { program: 13 }));
        assert_eq!(parse(&[0xC5, 0]), Some(Msg::Program { program: 0 }), "channel is ignored");
        assert_eq!(parse(&[0xC0]), None, "truncated");
        assert_eq!(parse(&[0xB0, 32, 0]), Some(Msg::Cc { channel: 0, cc: 32, value: 0 }), "bank LSB still a CC");
    }

    #[test]
    fn disabled_client_name_matches_full_port_name() {
        let off = vec!["Midi Through".to_string()];
        assert!(is_disabled("Midi Through:Midi Through Port-0 14:0", &off));
        assert!(is_disabled("Midi Through", &off), "bare client name still works");
        assert!(!is_disabled("Keystation Pro 88:Keystation Pro 88 MIDI 1 36:0", &off));
        assert!(!is_disabled("Midi Throughput:port 1 20:0", &off), "prefix needs the colon");
    }

    #[test]
    fn parses_pitch_bend_as_14_bit_lsb_first() {
        assert_eq!(parse(&[0xE0, 0, 64]), Some(Msg::Bend { value: 8192 }), "wheel at rest is centre");
        assert_eq!(parse(&[0xE0, 0, 0]), Some(Msg::Bend { value: 0 }), "fully down");
        assert_eq!(parse(&[0xE0, 127, 127]), Some(Msg::Bend { value: 16383 }), "fully up");
        // The LSB is the *first* data byte: swapping them would read 8192 here.
        assert_eq!(parse(&[0xE0, 64, 0]), Some(Msg::Bend { value: 64 }));
        assert_eq!(parse(&[0xE5, 0, 64]), Some(Msg::Bend { value: 8192 }), "channel is ignored");
    }
}
