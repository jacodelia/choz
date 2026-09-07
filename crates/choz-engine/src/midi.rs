//! MIDI input, straight off the ALSA sequencer.
//!
//! The reader thread can't touch the engine's single-producer note ring, so it
//! forwards parsed events over a `flume` channel; the UI loop drains that
//! channel and calls `engine.note_on` (the sole producer of the RT note ring).
//!
//! **Why not midir.** midir gives every open port its own sequencer client, its
//! own queue and its own thread, and closing one *joins* that thread from the
//! UI. choz hit both ends of that. An event the kernel left stamped in ticks —
//! which is what Ardour and REAPER send — panics midir's reader on an `unwrap`
//! of the timestamp, and the join that follows either panics in turn or waits
//! forever on a thread that will never answer: a frozen TUI with nothing in the
//! log. Here there is one client and one thread, the thread is never stopped,
//! and reconnecting is a subscription change made from outside — exactly what
//! `aconnect` does. Output still goes through midir, which never reads.

pub use crate::input::{BendMsg, CcMsg, ClockMsg, InputEvent, InputSource, NoteMsg, ProgramMsg};

use alsa::seq::{
    Addr, ClientIter, EventType, MidiEvent, PortCap, PortIter, PortSubscribe, PortType, Seq,
};
use std::collections::HashMap;
use std::ffi::CString;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

/// The name of the sequencer port choz publishes for other programs to play.
///
/// ALSA's sequencer only lets a program *subscribe to* ports that already
/// exist, and a DAW's MIDI output is not one of them: REAPER opens the devices
/// it was told to and publishes nothing anybody else can send to. So choz
/// publishes the port instead — the DAW picks "choz MIDI IN" out of its device
/// list, and the sequence plays a rack tab with no loopback module, no
/// `a2jmidid`, and no JACK graph.
///
/// **Created and deleted only by the MENU → MIDI IN switch**, never by a
/// reconnect. A port destroyed and remade every time somebody touches a USB
/// cable would drop the DAW's subscription with it, silently, because nothing
/// on either side reports that.
pub const VIRTUAL_IN_PORT: &str = "choz MIDI IN";

/// The reader thread's client and the port it never gives up: choz's own end
/// of every subscription [`connect_inputs`] makes. The published port comes and
/// goes with the MENU → MIDI IN switch, so it lives in [`Routes::published`].
#[derive(Clone, Copy)]
struct Ports {
    client: i32,
    hw: i32,
}

/// Started at the first connect, never stopped. `None` when this machine has no
/// sequencer to open (a container without `snd-seq`), which is not an error —
/// choz runs, it just has nothing to listen to.
static READER: OnceLock<Option<Ports>> = OnceLock::new();

/// Everything the reader thread needs to place what it reads. Rewritten whole
/// by every [`connect_inputs`], which is the only thing that ever moves.
struct Routes {
    /// Where parsed events go. Taken from the latest [`connect_inputs`] rather
    /// than from the one that started the thread, so the reader can never hold
    /// a channel whose other end has been dropped.
    sink: Option<flume::Sender<InputEvent>>,
    /// The entry [`VIRTUAL_IN_PORT`] belongs to, or `None` while it is
    /// switched off.
    virt: Option<usize>,
    /// The published port's number while it exists, written by the reader —
    /// the only thread allowed to create or delete it, because ALSA lets no
    /// other client touch a port's existence.
    published: Option<i32>,
    /// One entry per subscribed sender. `InputSource::Midi(i)` indexes the name
    /// list `connect_inputs` returned, and this is what keeps the two aligned.
    ///
    /// ponytail: a Vec scanned start to finish, not a map — it holds one entry
    /// per plugged-in controller, and the scan happens once per MIDI message.
    hw: Vec<(Addr, usize)>,
}

static ROUTES: Mutex<Routes> = Mutex::new(Routes {
    sink: None,
    virt: None,
    published: None,
    hw: Vec::new(),
});

/// Whether [`VIRTUAL_IN_PORT`] should be on the sequencer at all. Outside
/// [`ROUTES`] on purpose: [`poke`] waits for the reader to act on it, and the
/// reader needs that lock to do so.
static VIRT_WANTED: AtomicBool = AtomicBool::new(true);

/// The reader saying it has read [`VIRT_WANTED`] and done what it says.
///
/// ponytail: one slot, and a poke drains it before sending — a queue of acks
/// would only ever be answering a question nobody is still asking.
fn ack() -> &'static (flume::Sender<()>, flume::Receiver<()>) {
    static ACK: OnceLock<(flume::Sender<()>, flume::Receiver<()>)> = OnceLock::new();
    ACK.get_or_init(|| flume::bounded(1))
}

/// Wake the reader and wait for it to publish or unpublish the port.
///
/// The reader sits blocked in `event_input`, so the way to reach it is to send
/// it an event. `Usr0` is a sequencer-private type that cannot come off a MIDI
/// cable, which is what makes it unmistakable at the other end.
fn poke(ctl: &Seq, ports: Ports) {
    let Ok(port) = ctl.create_simple_port(
        c"poke",
        PortCap::READ,
        PortType::MIDI_GENERIC | PortType::APPLICATION,
    ) else {
        return;
    };
    while ack().1.try_recv().is_ok() {}
    let mut ev = alsa::seq::Event::new(EventType::Usr0, &[0u8; 12]);
    ev.set_source(port);
    ev.set_dest(Addr {
        client: ports.client,
        port: ports.hw,
    });
    ev.set_direct();
    if ctl.event_output_direct(&mut ev).is_err() {
        return;
    }
    // Bounded: a reader that has stopped must not take the UI down with it. The
    // port list is redrawn either way, and one that is a beat stale is a far
    // smaller problem than a rack that stops answering the keyboard.
    let _ = ack().1.recv_timeout(std::time::Duration::from_millis(500));
}

/// A sequencer client that only looks and wires. Opening one is cheap, and the
/// subscriptions it makes outlive it — `aconnect` exits too.
fn control_client(name: &str) -> Option<Seq> {
    let seq = Seq::open(None, None, true).ok()?;
    let _ = seq.set_client_name(&CString::new(name).ok()?);
    Some(seq)
}

/// Every port on the machine that can *send* MIDI, with the name choz shows.
///
/// The name keeps midir's shape — `"Client:Port n:m"` — because that is what
/// the saved "switched off" list has in it.
fn sources(seq: &Seq) -> Vec<(Addr, String)> {
    ClientIter::new(seq)
        .flat_map(|c| PortIter::new(seq, c.get_client()))
        .filter(|p| {
            p.get_type()
                .intersects(PortType::MIDI_GENERIC | PortType::SYNTH | PortType::APPLICATION)
        })
        .filter(|p| {
            p.get_capability()
                .contains(PortCap::READ | PortCap::SUBS_READ)
        })
        .filter_map(|p| {
            let client = seq.get_any_client_info(p.get_client()).ok()?;
            let name = format!(
                "{}:{} {}:{}",
                client.get_name().ok()?,
                p.get_name().ok()?,
                p.get_client(),
                p.get_port()
            );
            Some((p.addr(), name))
        })
        .collect()
}

/// All available MIDI **input** port names (what devices we can listen to).
pub fn list_input_ports() -> Vec<String> {
    // choz's own published port leads the list, because that is where
    // `connect_inputs` puts it — and being *in* the list is what gives it a row
    // under MENU → MIDI IN to switch off. Named unconditionally: the row has to
    // exist for the port to be switched back *on* after it was switched off.
    let mut ports = vec![VIRTUAL_IN_PORT.to_string()];
    if let Some(seq) = control_client("choz-scan") {
        // choz's own two ports are write-only, so they are not in here twice.
        ports.extend(sources(&seq).into_iter().map(|(_, name)| name));
    }
    ports
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

/// Subscribe to every MIDI input port except those named in `disabled`, so any
/// plugged-in controller drives the synth (Carla-style — no manual wiring
/// needed). Returns the connected port names, which `InputSource::Midi(i)`
/// indexes into.
///
/// Nothing here is a handle the caller has to hold on to: the reader thread
/// owns the only client that matters and outlives every call. Calling it again
/// is how a port is switched off — the subscription goes, the thread stays.
pub fn connect_inputs(tx: flume::Sender<InputEvent>, disabled: &[String]) -> Vec<String> {
    // Set before the reader is even started, so its first look is the right
    // one and a choz that opens with the port switched off never publishes it.
    let publish = !is_disabled(VIRTUAL_IN_PORT, disabled);
    let toggled = VIRT_WANTED.swap(publish, Ordering::Relaxed) != publish;

    let Some(ports) = reader() else {
        return Vec::new();
    };
    let Some(ctl) = control_client("choz-wire") else {
        return Vec::new();
    };
    // Before the lock, never under it: acting on the switch is the reader's
    // job, and the reader needs that lock to say it has.
    if toggled {
        poke(&ctl, ports);
    }
    let dest = Addr {
        client: ports.client,
        port: ports.hw,
    };
    let Ok(mut routes) = ROUTES.lock() else {
        return Vec::new();
    };
    routes.sink = Some(tx);

    // Drop what the last call wired: a port switched off, or unplugged, has to
    // stop arriving. Unsubscribing a port that is already gone fails, which is
    // the outcome we wanted anyway.
    for (addr, _) in routes.hw.drain(..) {
        let _ = ctl.unsubscribe_port(addr, dest);
    }

    let mut names = Vec::new();
    // choz's own port, first in the list when it is on — and the index stored
    // here is what the reader tags its events with, so the two cannot drift.
    routes.virt = match publish {
        false => None,
        true => {
            names.push(VIRTUAL_IN_PORT.to_string());
            Some(0)
        }
    };

    for (addr, name) in sources(&ctl) {
        if addr.client == ports.client || is_disabled(&name, disabled) {
            continue;
        }
        let Ok(sub) = PortSubscribe::empty() else {
            continue;
        };
        sub.set_sender(addr);
        sub.set_dest(dest);
        if let Err(e) = ctl.subscribe_port(&sub) {
            eprintln!("choz: MIDI subscribe '{name}' failed: {e}");
            continue;
        }
        routes.hw.push((addr, names.len()));
        names.push(name);
    }
    names
}

/// The client every input arrives on, opened once for the life of the process.
fn reader() -> Option<Ports> {
    *READER.get_or_init(|| {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("choz MIDI in".into())
            .spawn(move || run(ready_tx))
            .ok()?;
        // The ports have to exist before the caller can wire anything to them.
        ready_rx.recv().ok()?
    })
}

/// The client and the port the reader always has.
fn open_ports() -> Option<(Seq, Ports)> {
    // Blocking: this thread has nothing to do but wait for the next event.
    let seq = Seq::open(None, None, false).ok()?;
    let _ = seq.set_client_name(&CString::new("choz").ok()?);
    // SUBS_WRITE because the wiring is done by a *different* client (see
    // `connect_inputs`), and the kernel only lets a third party subscribe to a
    // port that says it accepts subscriptions.
    let hw = seq
        .create_simple_port(
            c"in",
            PortCap::WRITE | PortCap::SUBS_WRITE,
            PortType::MIDI_GENERIC | PortType::APPLICATION,
        )
        .ok()?;
    let client = seq.client_id().ok()?;
    Some((seq, Ports { client, hw }))
}

/// Publish or unpublish [`VIRTUAL_IN_PORT`] to match the switch, and record
/// where it ended up. Runs on the reader thread and nowhere else: ALSA lets
/// only the owning client create or delete one of its ports.
///
/// Switched off the port is **gone**, not merely ignored. A port left on the
/// sequencer while choz drops what arrives on it is a DAW playing into silence
/// with nothing to show for it — the switch has to be visible from the other
/// side of the wire.
fn republish(seq: &Seq, published: Option<i32>) -> Option<i32> {
    let published = match (VIRT_WANTED.load(Ordering::Relaxed), published) {
        (true, None) => CString::new(VIRTUAL_IN_PORT).ok().and_then(|name| {
            seq.create_simple_port(
                &name,
                PortCap::WRITE | PortCap::SUBS_WRITE,
                PortType::MIDI_GENERIC | PortType::APPLICATION,
            )
            .ok()
        }),
        (false, Some(port)) => {
            let _ = seq.delete_port(port);
            None
        }
        (_, unchanged) => unchanged,
    };
    if let Ok(mut routes) = ROUTES.lock() {
        routes.published = published;
    }
    published
}

/// Read the sequencer until the process ends.
fn run(ready: std::sync::mpsc::Sender<Option<Ports>>) {
    let Some((seq, ports)) = open_ports() else {
        let _ = ready.send(None);
        return;
    };
    let _ = ready.send(Some(ports));

    let Ok(coder) = MidiEvent::new(0) else { return };
    // Full status byte on every message: `parse` reads one message at a time
    // and has no running status to carry between them.
    coder.enable_running_status(false);

    // Per sender: its last Bank Select MSB, and its clock. Both belong to the
    // *port* rather than to the note — see `event_of` and `ClockCounter`.
    let mut state: HashMap<Addr, (u8, ClockCounter)> = HashMap::new();
    // ponytail: choz stamps arrivals off its own monotonic clock rather than
    // asking ALSA to stamp them. Asking means a queue per port, and an event
    // whose queue is gone keeps the sender's tick stamp — the exact event midir
    // unwrapped on. One thread hop of jitter, averaged over 24 pulses.
    let start = std::time::Instant::now();
    let mut published = republish(&seq, None);
    let mut input = seq.input();
    loop {
        let mut ev = match input.event_input() {
            Ok(ev) => ev,
            // The input buffer overran, or the wait was interrupted: the
            // sequencer is still there, so read the next one.
            Err(e) if matches!(e.errno(), libc::ENOSPC | libc::EAGAIN | libc::EINTR) => continue,
            Err(e) => {
                eprintln!("choz: MIDI reader stopped: {e}");
                return;
            }
        };
        // `connect_inputs` asking for the published port to appear or go away
        // — the one thing only this thread can do. It waits for the answer, so
        // the answer goes out even when nothing had to change.
        if ev.get_type() == EventType::Usr0 {
            published = republish(&seq, published);
            let _ = ack().0.try_send(());
            continue;
        }
        // A subscription coming or going is the sequencer talking about itself.
        if matches!(
            ev.get_type(),
            EventType::PortSubscribed | EventType::PortUnsubscribed
        ) {
            continue;
        }
        let sender = ev.get_source();
        // One look at the routing per message, and the channel comes out of the
        // same look: a reconnect swaps both at once, under this lock.
        let (index, sink) = {
            let Ok(routes) = ROUTES.lock() else { continue };
            let index = match Some(ev.get_dest().port) == published {
                // `None` is the published port switched off under MENU → MIDI
                // IN; on the other side, a sender wired by somebody else or
                // unwired a moment ago. Neither is ours to forward.
                true => routes.virt,
                false => routes
                    .hw
                    .iter()
                    .find(|(addr, _)| *addr == sender)
                    .map(|(_, index)| *index),
            };
            match (index, routes.sink.clone()) {
                (Some(index), Some(sink)) => (index, sink),
                _ => continue,
            }
        };
        let mut buf = [0u8; 12];
        // Sysex is longer than the buffer and comes back as an error: choz has
        // no use for it, and this is where it stops.
        let Ok(len) = coder.decode(&mut buf, &mut ev) else {
            continue;
        };
        let data = &buf[..len];
        let stamp = start.elapsed().as_micros() as u64;
        let source = InputSource::Midi(index);
        let (bank, clock) = state.entry(sender).or_default();
        // The clock is counted here rather than upstream: this is the last
        // place the timestamp is honest — a pulse read from a UI loop has that
        // loop's jitter in it.
        if let Some(msg) = clock.feed(data, stamp) {
            let _ = sink.send(InputEvent::Clock(source, msg));
            continue;
        }
        if let Some(event) = event_of(data, source, bank) {
            let _ = sink.send(event);
        }
    }
}

/// Is this port switched off? A port is named `"Client:Port n:m"`, so the saved
/// `"Midi Through"` never matched the full name and the loopback port stayed
/// connected. A saved entry disables the whole client when it names one.
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
pub(crate) struct ClockCounter {
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
    pub(crate) fn feed(&mut self, data: &[u8], stamp: u64) -> Option<ClockMsg> {
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

/// One raw MIDI message as the event it becomes, tagged with where it came
/// from. `None` for anything choz has no use for, the clock included — that is
/// counted by [`ClockCounter`], which needs the port's own timestamp and so
/// cannot be folded in here.
///
/// `bank` is the port's last Bank Select MSB, kept by the caller because it
/// belongs to the *port*: the pair arrives as its own CC just before the
/// program change and has to travel with it.
///
/// **One translation, two ports.** The ALSA callback above and choz's own JACK
/// MIDI input both come through here, so a note from a DAW on the graph and a
/// note from a keyboard on a DIN cable become the same event by the same rules.
pub fn event_of(data: &[u8], source: InputSource, bank: &mut u8) -> Option<InputEvent> {
    match parse(data)? {
        Msg::Note {
            channel,
            on,
            note,
            vel,
        } => Some(InputEvent::Note(NoteMsg {
            source,
            channel,
            on,
            note,
            vel,
        })),
        // Control changes drive MIDI-learn bindings (rack faders) and reach the
        // instrument, which is what makes the pedals and the modulation wheel
        // work.
        Msg::Cc { channel, cc, value } => {
            if cc == 0 {
                *bank = value;
            }
            Some(InputEvent::Cc(CcMsg {
                source,
                channel,
                cc,
                value,
            }))
        }
        Msg::Program { program } => Some(InputEvent::Program(ProgramMsg {
            source,
            bank: *bank,
            program,
        })),
        Msg::Bend { value } => Some(InputEvent::Bend(BendMsg { source, value })),
    }
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

    /// The name of every port choz's own reader client has on the sequencer.
    ///
    /// Matching on the port name alone is not enough: `cargo test` runs several
    /// binaries at once, each with a sequencer client of its own called `choz`,
    /// and one of them holding the published port says nothing about this one.
    fn our_ports(client: i32) -> Vec<String> {
        let Some(seq) = control_client("choz-test-scan") else {
            return Vec::new();
        };
        PortIter::new(&seq, client)
            .filter_map(|p| p.get_name().ok().map(str::to_string))
            .collect()
    }

    /// The port choz publishes: other programs must be able to *find* it, and
    /// it must be the first name `connect_inputs` returns — that index is what
    /// the reader tags everything arriving on it with.
    /// The two tests below rewire the *same* process-wide client, so they
    /// cannot run at once.
    static PORT: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn the_virtual_port_is_published_and_comes_first() {
        let _g = PORT.lock().unwrap_or_else(|e| e.into_inner());
        // No sequencer in this environment (a container without snd-seq):
        // there is nothing to publish a port on, and nothing to assert.
        if control_client("choz-test").is_none() {
            return;
        }
        let (tx, _rx) = flume::unbounded();
        let names = connect_inputs(tx, &[]);
        assert_eq!(
            names.first().map(String::as_str),
            Some(VIRTUAL_IN_PORT),
            "{names:?}"
        );
        // And a DAW sees it as somewhere to send: it is really on the
        // sequencer, under this process's own client.
        let Some(ports) = *READER.get().expect("the reader is up") else {
            return;
        };
        let seen = our_ports(ports.client);
        assert!(
            seen.iter().any(|n| n == VIRTUAL_IN_PORT),
            "the published port is not on the sequencer: {seen:?}"
        );
    }

    /// Switched off under MENU → MIDI IN, the port goes away. It has to be
    /// visible from the other side of the wire: a port still on the sequencer
    /// is a DAW playing into a rack that has stopped listening, with nothing
    /// anywhere to say so.
    #[test]
    fn a_disabled_virtual_port_is_closed_not_just_hidden() {
        let _g = PORT.lock().unwrap_or_else(|e| e.into_inner());
        if control_client("choz-test-off").is_none() {
            return;
        }
        let (tx, _rx) = flume::unbounded();
        let off = [VIRTUAL_IN_PORT.to_string()];
        let names = connect_inputs(tx.clone(), &off);
        assert!(
            !names.iter().any(|n| n == VIRTUAL_IN_PORT),
            "a disabled port is still named: {names:?}"
        );
        assert_eq!(
            ROUTES.lock().unwrap().virt,
            None,
            "a disabled port still has an index to tag events with"
        );
        let Some(ports) = *READER.get().expect("the reader is up") else {
            return;
        };
        let seen = our_ports(ports.client);
        assert!(
            !seen.iter().any(|n| n == VIRTUAL_IN_PORT),
            "the port is still on the sequencer: {seen:?}"
        );

        // And switching it back on names it first again.
        let names = connect_inputs(tx, &[]);
        assert_eq!(names.first().map(String::as_str), Some(VIRTUAL_IN_PORT));
        assert_eq!(ROUTES.lock().unwrap().virt, Some(0));
    }

    /// The bug this whole module exists for: a note sent to the published port
    /// with **no real-time stamp** on it — which is what Ardour and REAPER
    /// send, and what midir unwrapped a `None` out of, killing its reader
    /// thread and then hanging the UI on the join that followed. A plain
    /// direct event is stamped in ticks, so this is that event exactly.
    #[test]
    fn a_note_with_no_real_timestamp_arrives_instead_of_killing_the_reader() {
        let _g = PORT.lock().unwrap_or_else(|e| e.into_inner());
        let Some(daw) = control_client("choz-test-daw") else {
            return;
        };
        let (tx, rx) = flume::unbounded();
        assert!(connect_inputs(tx, &[]).contains(&VIRTUAL_IN_PORT.to_string()));
        let Some(ports) = *READER.get().expect("the reader is up") else {
            return;
        };
        let published = ROUTES.lock().unwrap().published.expect("the port is up");
        let Ok(out) = daw.create_simple_port(
            c"out",
            PortCap::READ | PortCap::SUBS_READ,
            PortType::MIDI_GENERIC | PortType::APPLICATION,
        ) else {
            return;
        };

        let mut ev = alsa::seq::Event::new(
            EventType::Noteon,
            &alsa::seq::EvNote {
                channel: 0,
                note: 60,
                velocity: 100,
                off_velocity: 0,
                duration: 0,
            },
        );
        ev.set_source(out);
        ev.set_dest(Addr {
            client: ports.client,
            port: published,
        });
        ev.set_direct();
        daw.event_output_direct(&mut ev).expect("send");

        match rx.recv_timeout(std::time::Duration::from_secs(2)) {
            Ok(InputEvent::Note(n)) => {
                assert_eq!((n.note, n.on, n.vel), (60, true, 100));
                assert_eq!(n.source, InputSource::Midi(0));
            }
            other => panic!("the reader dropped the note: {other:?}"),
        }
    }
}
