//! Hardware MIDI input via midir.
//!
//! The midir callback runs on its own thread, so it can't touch the engine's
//! single-producer note ring directly. It forwards parsed note events over a
//! `flume` channel; the UI loop drains that channel and calls `engine.note_on`
//! (the sole producer of the RT note ring).

pub use crate::input::{BendMsg, CcMsg, ClockMsg, InputEvent, InputSource, NoteMsg, ProgramMsg};

/// All available MIDI **input** port names (what devices we can listen to).
pub fn list_input_ports() -> Vec<String> {
    match midir::MidiInput::new("choz-scan-in") {
        Ok(m) => m
            .ports()
            .iter()
            .filter_map(|p| m.port_name(p).ok())
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// All available MIDI **output** port names: what a tab can play *to*.
pub fn list_output_ports() -> Vec<String> {
    match midir::MidiOutput::new("choz-scan-out") {
        Ok(m) => m
            .ports()
            .iter()
            .filter_map(|p| m.port_name(p).ok())
            .collect(),
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
        let Ok(mi) = midir::MidiInput::new("choz-in") else {
            continue;
        };
        let Some(port) = mi
            .ports()
            .into_iter()
            .find(|p| mi.port_name(p).as_deref() == Ok(want.as_str()))
        else {
            continue;
        };
        let txc = tx.clone();
        // Bank Select arrives as its own CC just before the program change, so
        // the port's last MSB is remembered here to travel with it. ponytail:
        // MSB only — SF2 banks are 0..128 and the LSB is what this keyboard
        // (and most others) always leaves at 0.
        let mut bank = 0u8;
        // The clock is counted here rather than upstream: this callback is
        // handed the port's own timestamp, and it is the last place that number
        // is honest — a pulse read from a UI loop has that loop's jitter in it.
        let mut clock = ClockCounter::default();
        // Each connection tags its events with its index in `names`, which is
        // the list the caller gets back — so index ↔ port name stay aligned.
        let source = InputSource::Midi(names.len());
        match mi.connect(
            &port,
            "choz-in-conn",
            move |_ts, data, _| {
                if let Some(msg) = clock.feed(data, _ts) {
                    let _ = txc.send(InputEvent::Clock(source, msg));
                    return;
                }
                match parse(data) {
                    Some(Msg::Note {
                        channel,
                        on,
                        note,
                        vel,
                    }) => {
                        let _ = txc.send(InputEvent::Note(NoteMsg {
                            source,
                            channel,
                            on,
                            note,
                            vel,
                        }));
                    }
                    // Control changes drive MIDI-learn bindings (rack faders)
                    // and reach the instrument, which is what makes the pedals
                    // and the modulation wheel work.
                    Some(Msg::Cc { channel, cc, value }) => {
                        if cc == 0 {
                            bank = value;
                        }
                        let _ = txc.send(InputEvent::Cc(CcMsg {
                            source,
                            channel,
                            cc,
                            value,
                        }));
                    }
                    Some(Msg::Program { program }) => {
                        let _ = txc.send(InputEvent::Program(ProgramMsg {
                            source,
                            bank,
                            program,
                        }));
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
    disabled
        .iter()
        .any(|d| port == d || port.starts_with(&format!("{d}:")))
}

// ─── Output ─────────────────────────────────────────────────────────────────

/// An open MIDI output: what a tab plays, on somebody else's synth.
///
/// The arpeggiator is the reason this exists. Everything in choz until now
/// ended at its own instrument, and an arpeggiator that can only drive the
/// plugin in the same tab is half of one — the other half is a desk full of
/// hardware that has no arpeggiator of its own.
///
/// Sending is best-effort: a port that has gone away (a synth switched off
/// mid-set) drops its notes rather than taking the rack with it. What it must
/// not do is leave them **sounding**, which is what [`Self::all_notes_off`] is
/// for.
pub struct MidiOut {
    name: String,
    conn: midir::MidiOutputConnection,
    /// Notes sent and not yet stopped, so a disconnection can end them.
    sounding: Vec<u8>,
}

impl MidiOut {
    /// Open the port called `name`. `None` when there is no such port — a saved
    /// project naming a synth that is not plugged in today is a normal Tuesday,
    /// not an error worth stopping for.
    pub fn open(name: &str) -> Option<Self> {
        let out = midir::MidiOutput::new("choz-out").ok()?;
        let port = out
            .ports()
            .into_iter()
            .find(|p| out.port_name(p).as_deref() == Ok(name))?;
        let conn = out.connect(&port, "choz-out-conn").ok()?;
        Some(Self {
            name: name.to_string(),
            conn,
            sounding: Vec::new(),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Channel is 0-based on the wire, as everywhere else in this crate.
    pub fn note_on(&mut self, channel: u8, note: u8, vel: u8) {
        if self
            .conn
            .send(&[0x90 | (channel & 0x0F), note, vel])
            .is_ok()
            && !self.sounding.contains(&note)
        {
            self.sounding.push(note);
        }
    }

    pub fn note_off(&mut self, channel: u8, note: u8) {
        let _ = self.conn.send(&[0x80 | (channel & 0x0F), note, 0]);
        self.sounding.retain(|n| *n != note);
    }

    /// Every note this port was told to play, stopped one by one.
    ///
    /// Note-offs rather than CC 123: a hardware synth that ignores "all notes
    /// off" is a synth that drones until it is power-cycled, and the list of
    /// what is actually down is right here.
    pub fn all_notes_off(&mut self, channel: u8) {
        for note in std::mem::take(&mut self.sounding) {
            let _ = self.conn.send(&[0x80 | (channel & 0x0F), note, 0]);
        }
    }
}

impl Drop for MidiOut {
    fn drop(&mut self) {
        self.all_notes_off(0);
    }
}

impl std::fmt::Debug for MidiOut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MidiOut({})", self.name)
    }
}

/// Counts the clock of one port into something worth sending on.
///
/// Twenty-four pulses is a quarter note, so a quarter's worth of them is one
/// tempo reading — averaging over the quarter rather than over one interval,
/// because a single pulse carries every bit of jitter the cable and the sender
/// have between them.
#[derive(Default)]
struct ClockCounter {
    /// Pulses since the reading, and when that run started (microseconds, the
    /// port's own clock).
    pulses: u32,
    started: u64,
}

/// Pulses per quarter note on MIDI's clock wire. Fixed by the standard.
const CLOCK_PPQ: u32 = 24;

impl ClockCounter {
    /// Feed a raw message. `Some` when it was a clock byte worth passing on;
    /// `None` for a pulse that is still being counted, and for anything that is
    /// not the clock at all.
    fn feed(&mut self, data: &[u8], stamp: u64) -> Option<ClockMsg> {
        match data.first().copied()? {
            // A run of pulses is only a tempo once there is a quarter of it.
            0xF8 => {
                if self.pulses == 0 {
                    self.started = stamp;
                    self.pulses = 1;
                    return None;
                }
                self.pulses += 1;
                if self.pulses <= CLOCK_PPQ {
                    return None;
                }
                let elapsed = stamp.saturating_sub(self.started);
                // This pulse opens the next quarter, so the count restarts at
                // one rather than at zero: dropping it would lose a beat of
                // every measurement.
                self.pulses = 1;
                self.started = stamp;
                if elapsed == 0 {
                    return None;
                }
                Some(ClockMsg::Tempo(60_000_000.0 / elapsed as f32))
            }
            // A transport command restarts the count: the run that was being
            // measured belongs to whatever was playing before.
            0xFA => {
                self.pulses = 0;
                Some(ClockMsg::Start)
            }
            0xFB => {
                self.pulses = 0;
                Some(ClockMsg::Continue)
            }
            0xFC => {
                self.pulses = 0;
                Some(ClockMsg::Stop)
            }
            _ => None,
        }
    }
}

/// A raw MIDI message choz cares about.
#[derive(Debug, PartialEq, Eq)]
enum Msg {
    /// `channel` is 0-based, as it is on the wire. It only matters in the
    /// rack's multi-timbral mode, where one port drives several tabs at once —
    /// the way a sampler answers a DAW's orchestral template.
    Note {
        channel: u8,
        on: bool,
        note: u8,
        vel: u8,
    },
    Cc {
        channel: u8,
        cc: u8,
        value: u8,
    },
    /// Pitch bend, as the 14-bit value the wire carries: 0..16383, centred at
    /// 8192. Kept unsigned because that is what synths take.
    Bend {
        value: u16,
    },
    /// Program change — the buttons on a controller keyboard usually send these
    /// (preceded by a Bank Select pair), not CCs.
    Program {
        program: u8,
    },
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
        return Some(Msg::Program {
            program: data[1] & 0x7F,
        });
    }
    if data.len() < 3 {
        return None;
    }
    let channel = data[0] & 0x0F;
    match data[0] & 0xF0 {
        0x90 if data[2] > 0 => Some(Msg::Note {
            channel,
            on: true,
            note: data[1],
            vel: data[2],
        }),
        0x80 | 0x90 => Some(Msg::Note {
            channel,
            on: false,
            note: data[1],
            vel: 0,
        }),
        0xB0 => Some(Msg::Cc {
            channel,
            cc: data[1],
            value: data[2],
        }),
        // LSB first, then MSB — both 7-bit.
        0xE0 => Some(Msg::Bend {
            value: (data[1] as u16 & 0x7F) | ((data[2] as u16 & 0x7F) << 7),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Twenty-four pulses is a quarter note, so a quarter of them is one tempo
    /// reading — and the pulse that closes a quarter opens the next, or every
    /// measurement would lose a beat.
    #[test]
    fn a_quarter_of_pulses_is_one_tempo_reading() {
        let mut c = ClockCounter::default();
        // 120 BPM: a quarter is half a second, so a pulse every 20833 µs.
        let step = 500_000 / CLOCK_PPQ as u64;
        let mut stamp = 1_000_000u64;
        for _ in 0..CLOCK_PPQ {
            assert_eq!(c.feed(&[0xF8], stamp), None, "still counting");
            stamp += step;
        }
        match c.feed(&[0xF8], stamp) {
            Some(ClockMsg::Tempo(bpm)) => assert!((bpm - 120.0).abs() < 0.5, "{bpm}"),
            other => panic!("expected a tempo, got {other:?}"),
        }

        // And straight into the next quarter, with no pulse lost.
        for _ in 0..(CLOCK_PPQ - 1) {
            stamp += step;
            assert_eq!(c.feed(&[0xF8], stamp), None);
        }
        stamp += step;
        assert!(matches!(c.feed(&[0xF8], stamp), Some(ClockMsg::Tempo(_))));
    }

    /// The three transport bytes come through as themselves, and they restart
    /// the count: the run being measured belonged to what was playing before.
    #[test]
    fn start_continue_and_stop_come_through_and_reset_the_count() {
        let mut c = ClockCounter::default();
        assert_eq!(c.feed(&[0xFA], 0), Some(ClockMsg::Start));
        assert_eq!(c.feed(&[0xFB], 0), Some(ClockMsg::Continue));
        assert_eq!(c.feed(&[0xFC], 0), Some(ClockMsg::Stop));
        // Not the clock at all: the parser downstream gets it.
        assert_eq!(c.feed(&[0x90, 60, 100], 0), None);

        let step = 500_000 / CLOCK_PPQ as u64;
        let mut stamp = 0u64;
        for _ in 0..CLOCK_PPQ {
            c.feed(&[0xF8], stamp);
            stamp += step;
        }
        c.feed(&[0xFA], stamp);
        // Counting starts again from here, so the next quarter is not reported
        // one pulse early.
        for _ in 0..CLOCK_PPQ {
            assert_eq!(c.feed(&[0xF8], stamp), None);
            stamp += step;
        }
        assert!(matches!(c.feed(&[0xF8], stamp), Some(ClockMsg::Tempo(_))));
    }

    #[test]
    fn parses_note_on_off_and_ignores_others() {
        assert_eq!(
            parse(&[0x90, 60, 100]),
            Some(Msg::Note {
                channel: 0,
                on: true,
                note: 60,
                vel: 100
            })
        );
        assert_eq!(
            parse(&[0x90, 60, 0]),
            Some(Msg::Note {
                channel: 0,
                on: false,
                note: 60,
                vel: 0
            }),
            "vel0 = note-off"
        );
        assert_eq!(
            parse(&[0x80, 60, 40]),
            Some(Msg::Note {
                channel: 0,
                on: false,
                note: 60,
                vel: 0
            })
        );
        assert_eq!(
            parse(&[0xB0, 7, 100]),
            Some(Msg::Cc {
                channel: 0,
                cc: 7,
                value: 100
            }),
            "CC drives MIDI learn"
        );
        assert_eq!(parse(&[0xF8]), None, "clock is neither");
        assert_eq!(parse(&[0x90, 60]), None, "truncated");
    }

    /// A Keystation Pro 88 button sends bank select then a two-byte program
    /// change. Requiring three bytes dropped the program change, so every
    /// button looked like the same CC 32.
    #[test]
    fn parses_two_byte_program_change() {
        assert_eq!(parse(&[0xC0, 13]), Some(Msg::Program { program: 13 }));
        assert_eq!(
            parse(&[0xC5, 0]),
            Some(Msg::Program { program: 0 }),
            "channel is ignored"
        );
        assert_eq!(parse(&[0xC0]), None, "truncated");
        assert_eq!(
            parse(&[0xB0, 32, 0]),
            Some(Msg::Cc {
                channel: 0,
                cc: 32,
                value: 0
            }),
            "bank LSB still a CC"
        );
    }

    #[test]
    fn disabled_client_name_matches_full_port_name() {
        let off = vec!["Midi Through".to_string()];
        assert!(is_disabled("Midi Through:Midi Through Port-0 14:0", &off));
        assert!(
            is_disabled("Midi Through", &off),
            "bare client name still works"
        );
        assert!(!is_disabled(
            "Keystation Pro 88:Keystation Pro 88 MIDI 1 36:0",
            &off
        ));
        assert!(
            !is_disabled("Midi Throughput:port 1 20:0", &off),
            "prefix needs the colon"
        );
    }

    #[test]
    fn parses_pitch_bend_as_14_bit_lsb_first() {
        assert_eq!(
            parse(&[0xE0, 0, 64]),
            Some(Msg::Bend { value: 8192 }),
            "wheel at rest is centre"
        );
        assert_eq!(
            parse(&[0xE0, 0, 0]),
            Some(Msg::Bend { value: 0 }),
            "fully down"
        );
        assert_eq!(
            parse(&[0xE0, 127, 127]),
            Some(Msg::Bend { value: 16383 }),
            "fully up"
        );
        // The LSB is the *first* data byte: swapping them would read 8192 here.
        assert_eq!(parse(&[0xE0, 64, 0]), Some(Msg::Bend { value: 64 }));
        assert_eq!(
            parse(&[0xE5, 0, 64]),
            Some(Msg::Bend { value: 8192 }),
            "channel is ignored"
        );
    }
}
