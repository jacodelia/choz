//! Audio engine — runs the real-time audio thread with cpal.
//!
//! Auto-detects the best available audio backend using the approach from
//! seqterm: tries JACK first when PipeWire is detected, falls back to ALSA.
//! Both ALSA and JACK work through PipeWire's compatibility layers.
//!
//! RT-safety (patterns copied from seqterm-audio-engine): the audio callback
//! never locks, never allocates, and never builds an FX chain. The UI thread
//! builds the chain and hands the finished `Vec<Box<dyn FxProcessor>>` to the
//! callback over an `rtrb` lock-free ring; the callback swaps it in and ships
//! the old chain back over a second ring so its deallocation happens on the UI
//! thread, not in the RT context. Transport state is a plain `AtomicBool`.

use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::fx::FxProcessor;
use crate::fx_chain::{build_chain_from_specs, FxSpec};
use crate::sources::{AudioSource, Sf2Synth, WavPlayer};

type FxChain = Vec<Box<dyn FxProcessor>>;
type Source = Box<dyn AudioSource>;

/// One rack slot: an audio source plus its own post-source FX chain and mixer
/// strip. The RT callback mixes every slot's output together.
pub(crate) struct Slot {
    source: Source,
    fx: FxChain,
    /// Where this slot sums: a device pair, or a subgroup.
    dest: Dest,
    /// Linear output gain (0.0..=2.0). Two of them, one per side: a desk lets
    /// you trim one channel of a stereo instrument against the other, and a
    /// linked strip simply keeps them equal — see `App::set_gain_side`.
    gain: f32,
    gain_r: f32,
    /// Stereo position, -1.0 = hard left, 0.0 = center, 1.0 = hard right.
    pan: f32,
    mute: bool,
    /// Device output channels this slot's stereo pair lands on, 0-based.
    /// `(0, 1)` is the first pair; the JACK backend can address every channel
    /// the interface has, the cpal one clamps everything onto 0/1.
    out_pair: (usize, usize),
    /// Device *input* channels feeding this slot instead of its own source.
    /// `None` = the source plays (an instrument); `Some` = live audio in.
    /// Registered by the JACK backend now, exposed in the UI in stage 2.
    in_pair: Option<(usize, usize)>,
    /// Linear gain on the audio coming in, before the FX and before the pitch
    /// tracker hears it. A guitar is nowhere near the level of a synth, and
    /// without this the two are stuck at whatever the interface's preamp gave.
    in_gain: f32,
    /// Catches a microphone that has started to howl into the speakers it is
    /// feeding — see [`crate::feedback`]. One per slot because the loop is per
    /// input, armed globally because a room is a room.
    guard: crate::feedback::FeedbackGuard,
    /// Audio in, notes out: `Some` while the tab is converting what it hears
    /// into notes for its own instrument. The tracker lives here because it is
    /// per slot and used only from the audio callback.
    pitch: Option<crate::pitch::PitchTracker>,
    /// How much of a converting tab's output is the instrument rather than the
    /// audio that drove it. 1 = only the instrument, 0 = only the input.
    ///
    /// A converter that can only replace the sound is half a tool: a guitar
    /// doubled by a synth is the sound most people are after, and it needs the
    /// guitar still there.
    pitch_mix: f32,
    /// Notes with a time on them, waiting for the block that contains it.
    ///
    /// Fixed size and never grown: this is the audio thread. A queue that
    /// fills applies its oldest immediately rather than dropping it — a note
    /// slightly early is a note, and a dropped note-on is silence while a
    /// dropped note-off is a note that never stops.
    pending: [Option<Scheduled>; MAX_SCHEDULED],
    /// Which notes this slot has been told to play, one bit per MIDI note.
    ///
    /// Panic uses it to send the exact note-offs that are missing, which is the
    /// only thing that works everywhere: `all notes off` is a MIDI CC, and a
    /// VST3 plugin never sees CCs as events at all.
    held: u128,
}

impl Slot {
    fn new(source: Source) -> Self {
        Slot {
            source,
            fx: Vec::new(),
            dest: Dest::default(),
            gain: 1.0,
            gain_r: 1.0,
            pan: 0.0,
            mute: false,
            out_pair: (0, 1),
            in_pair: None,
            in_gain: 1.0,
            guard: crate::feedback::FeedbackGuard::new(48_000.0),
            pitch: None,
            pitch_mix: 1.0,
            pending: [None; MAX_SCHEDULED],
            held: 0,
        }
    }

    /// Play a note now, and remember it for `PANIC`.
    #[inline]
    fn play(&mut self, note: u8, vel: u8, on: bool) {
        if on {
            self.held |= 1u128 << (note & 0x7F);
            self.source.note_on(note, vel);
        } else {
            self.held &= !(1u128 << (note & 0x7F));
            self.source.note_off(note);
        }
    }

    /// Take a note, now or later. `at == 0` is now.
    fn schedule(&mut self, at: u64, note: u8, vel: u8, on: bool) {
        if at == 0 {
            self.play(note, vel, on);
            return;
        }
        if let Some(slot) = self.pending.iter_mut().find(|p| p.is_none()) {
            *slot = Some(Scheduled { at, note, vel, on });
            return;
        }
        // Full. A note slightly early is still the note; a dropped one is
        // silence, or worse, a note that never stops.
        self.play(note, vel, on);
    }

    /// The earliest pending note inside `[start, end)`, taken off the queue.
    fn due(&mut self, start: u64, end: u64) -> Option<Scheduled> {
        let mut best: Option<(usize, u64)> = None;
        for (i, p) in self.pending.iter().enumerate() {
            let Some(s) = p else { continue };
            // Anything already past is due immediately: a block was missed, and
            // the note is late rather than cancelled.
            let when = s.at.max(start);
            if when < end && best.is_none_or(|(_, b)| when < b) {
                best = Some((i, when));
            }
        }
        let (i, _) = best?;
        self.pending[i].take()
    }

    /// Constant-power pan law → (left, right) channel gains, each side times
    /// its own fader.
    fn channel_gains(&self) -> (f32, f32) {
        if self.mute {
            return (0.0, 0.0);
        }
        let theta = (self.pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
        (self.gain * theta.cos(), self.gain_r * theta.sin())
    }
}

/// Commands sent UI → RT over a lock-free ring. Notes name their target slot:
/// the UI resolves input→slot routing (it owns the input↔slot bindings), so the
/// RT side never has to know about MIDI ports or OSC.
pub(crate) enum EngineCommand {
    AddSlot(Source),
    RemoveSlot(usize),
    SetSlotSource {
        slot: usize,
        source: Source,
    },
    SetSlotFx {
        slot: usize,
        fx: FxChain,
    },
    SetSlotMix {
        slot: usize,
        gain: f32,
        gain_r: f32,
        pan: f32,
        mute: bool,
    },
    /// Which device output channels this slot lands on (0-based).
    SetSlotOut {
        slot: usize,
        left: usize,
        right: usize,
    },
    /// Which device input channels feed this slot; `None` = play its source.
    SetSlotIn {
        slot: usize,
        pair: Option<(usize, usize)>,
    },
    /// Listen to the slot's audio input and play its instrument from the pitch
    /// heard, instead of passing the audio through.
    SetSlotPitchToMidi {
        slot: usize,
        on: bool,
    },
    /// Dry/wet of a converting tab: 1 = only the instrument.
    SetSlotPitchMix {
        slot: usize,
        mix: f32,
    },
    /// Where a slot sums: a device pair, or one of the subgroups.
    SetSlotDest {
        slot: usize,
        dest: Dest,
    },
    /// One subgroup's strip.
    SetBus {
        bus: usize,
        gain: f32,
        mute: bool,
        left: usize,
        right: usize,
    },
    /// The main fader, on the first output pair.
    SetMain {
        gain: f32,
        mute: bool,
    },
    /// Trim on the slot's audio input, and how loud that input has to be before
    /// the pitch tracker calls it a note.
    SetSlotInTrim {
        slot: usize,
        gain: f32,
        gate: f32,
    },
    SetSlotProgram {
        slot: usize,
        bank: u8,
        preset: u8,
    },
    /// Live parameter tweak for a slot's *instrument* (hosted plugin).
    SetSlotParam {
        slot: usize,
        index: usize,
        value: f32,
    },
    /// Live parameter tweak for one FX in a slot's chain — avoids rebuilding
    /// the chain (which, for a hosted plugin, means re-instantiating it).
    SetFxParam {
        slot: usize,
        fx: usize,
        index: usize,
        value: f32,
    },
    NoteOn {
        slot: usize,
        note: u8,
        vel: u8,
        /// Transport sample the note is **for**. `0` means "as soon as it
        /// arrives", which is what every input that has no schedule of its own
        /// sends: a key, a MIDI port, OSC. A generator that knows when its next
        /// step lands sends that sample, and the callback splits its render
        /// there rather than starting the note at the top of a block.
        at: u64,
    },
    NoteOff {
        slot: usize,
        note: u8,
        at: u64,
    },
    /// Silence every slot: the panic button. One command rather than a burst
    /// of note-offs, so the ring cannot fill up halfway through and leave a
    /// note ringing — which would be the one thing panic must never do.
    Panic,
    /// Pedals, modulation wheel — any control change, unfiltered.
    ControlChange {
        slot: usize,
        cc: u8,
        value: u8,
    },
    /// Pitch bend, raw 14-bit wire value (0..16383, centred at 8192).
    PitchBend {
        slot: usize,
        value: u16,
    },
}

/// Items retired by the RT thread, returned to the UI thread to be dropped
/// off the audio thread (avoids deallocation in the RT context). The payload is
/// never read — only dropped — which is the whole point.
#[allow(dead_code)]
// The `Slot` variant is much the biggest, and it stays that way on purpose:
// boxing it would move the allocation onto the audio thread, which is the one
// thing this whole channel exists to avoid.
#[allow(clippy::large_enum_variant)]
pub(crate) enum Retired {
    Slot(Slot),
    Fx(FxChain),
    Source(Source),
}

/// Max rack slots before the slot Vec would reallocate on the RT thread.
/// ponytail: fixed cap keeps `AddSlot` alloc-free; raise it if anyone needs
/// more than this many simultaneous sources. The number lives in the meter,
/// which has to size a per-slot array to the same cap.
use crate::meter::MAX_SLOTS;

/// Capacity of the UI → RT command ring, and of the retired ring that comes
/// back. Sized for a burst: a chord plus a fader sweep between two audio
/// callbacks. ponytail: a full ring still drops commands — and a dropped
/// note-off is a note that hangs. Raise it if that is ever observed rather than
/// making the push block the UI.
const CMD_RING: usize = 512;

/// Which audio backend was selected at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioBackend {
    Alsa,
    Jack,
}

impl AudioBackend {
    pub fn label(&self) -> &'static str {
        match self {
            AudioBackend::Alsa => "ALSA",
            AudioBackend::Jack => "JACK",
        }
    }
}

/// Shared audio engine state (UI-thread side).
pub struct AudioEngine {
    pub sample_rate: u32,
    pub buffer_size: u32,
    pub backend: AudioBackend,
    playing: Arc<AtomicBool>,
    /// Number of rack slots the UI has created (kept in sync with the RT side).
    slot_count: usize,
    /// Native-editor handle per slot, mirrored with `slot_count`. Taken from
    /// each source before it moves to the RT thread — that is the only moment
    /// the UI can still reach it.
    editors: Vec<Option<choz_ports::EditorHandle>>,
    /// Same, for each slot's FX chain: `fx_editors[slot][fx]`.
    fx_editors: Vec<Vec<Option<choz_ports::EditorHandle>>>,
    /// Each slot's plugin state handle: its patch, not just its parameters.
    /// Captured where the editor is, and for the same reason — it is the last
    /// moment the UI can reach the plugin.
    states: Vec<Option<choz_ports::StateHandle>>,
    /// Same, per FX: `fx_states[slot][fx]`.
    fx_states: Vec<Vec<Option<choz_ports::StateHandle>>>,
    /// Each slot's instrument preset browser, when the plugin has one. Same
    /// capture moment, same reason: listing and loading a preset are main-thread
    /// work that the RT copy of the source can no longer be asked for.
    presets: Vec<Option<choz_ports::PresetsHandle>>,
    /// What each slot's instrument reports when the user moves one of its knobs
    /// **inside the plugin's own window**. Same capture moment as the editor.
    touches: Vec<Option<choz_ports::TouchHandle>>,
    /// Same, per FX: `fx_touches[slot][fx]`.
    fx_touches: Vec<Vec<Option<choz_ports::TouchHandle>>>,
    /// Counters of the slot's instrument when it plays in its own process,
    /// taken at the same moment as the editor handle. `None` = in-process.
    sandboxes: Vec<Option<choz_ports::SandboxStatus>>,
    /// Same, per FX: `fx_sandboxes[slot][fx]`.
    fx_sandboxes: Vec<Vec<Option<choz_ports::SandboxStatus>>>,
    /// Peak in/out per effect, for the meter the rack draws:
    /// `fx_meters[slot][fx]`. Captured where the editor is — the last moment
    /// the UI can reach the processor.
    fx_meters: Vec<Vec<Option<choz_ports::FxMeter>>>,
    /// What each effect adds to the signal's delay, in samples:
    /// `fx_latency[slot][fx]`. A constant of the algorithm, so it is read once,
    /// here, instead of asked for on the audio thread.
    fx_latency: Vec<Vec<u32>>,
    /// UI → RT: engine commands (add/remove slot, set fx, notes).
    cmd_tx: rtrb::Producer<EngineCommand>,
    /// RT → UI: retired slots/chains to drop off the audio thread.
    retired_rx: rtrb::Consumer<Retired>,
    /// Name of the output device in use (set once the stream is open).
    output_device: Option<String>,
    /// Backend the user asked for: `AUTO`, `JACK`, `PIPEWIRE` or `ALSA`.
    /// `AUTO` keeps the historical behaviour (JACK when PipeWire is up).
    backend_pref: String,
    /// RT endpoints, taken by `start()` and moved into the callback.
    rt_endpoints: Option<RtEndpoints>,
    _stream: Option<BackendHandle>,
    /// The live capture stream on the cpal backends. Held only to keep it
    /// alive; dropping it stops the input, which is the whole point.
    _input_stream: Option<cpal::Stream>,
    /// Which input the user asked for: `None` = the host's default, and
    /// `wants_input` is what decides whether one is opened at all.
    input_device: Option<String>,
    /// PipeWire quantum to force on the **whole graph**, or 0 to leave it
    /// alone. See [`request_pipewire_period`] for why the default is 0.
    force_quantum: u32,
    /// Whether to open a capture stream on the cpal backends. Off by default:
    /// a host that grabs the microphone on start-up is a host nobody asked.
    wants_input: bool,
    /// Device output channels the running backend gives us. 2 on cpal; the
    /// interface's real count on the native JACK client.
    out_channels: usize,
    /// How many of our output ports actually reached a sink. Zero means choz
    /// is computing a mix that goes nowhere — silence that looks exactly like
    /// a broken effect, so the interface says it out loud.
    out_wired: usize,
    /// Device input channels available to slots (native JACK only).
    in_channels: usize,
    /// The graph port behind each input channel, in channel order — every
    /// capture jack in the system, not one device's. The UI reads these for
    /// the row labels, so what the drawer lists is what is actually wired.
    input_ports: Vec<String>,
}

/// The live audio connection. Held only to keep it alive: dropping either
/// variant stops audio, which is the whole point.
#[allow(dead_code)]
enum BackendHandle {
    Cpal(cpal::Stream),
    /// Native JACK client — the only backend that can address more than two
    /// device channels.
    Jack(Box<crate::jack_backend::Handle>),
}

/// The consumer/producer ends the RT callback owns.
struct RtEndpoints {
    cmd_rx: rtrb::Consumer<EngineCommand>,
    retired_tx: rtrb::Producer<Retired>,
}

/// Subgroups. Four, because a rack is grouped by what it is — keys, drums,
/// guitars, the click — and a fifth name is one nobody uses.
///
/// A bus is a **destination that is not a device**: tabs sum into it, it has
/// its own fader and mute, and what comes out of it lands on a device pair like
/// a tab would. That is the whole of it — no sends, no inserts, no nesting.
/// ponytail: flat and fixed-size, because both are what makes it allocation-free
/// on the audio thread; a bus that feeds another bus is a different feature.
pub const BUSES: usize = 4;

/// Where a tab's (or the click's) audio goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Dest {
    /// Straight to a pair of device channels, which is what everything did
    /// before there were buses.
    #[default]
    Direct,
    /// Into subgroup `0..BUSES`.
    Bus(usize),
}

impl Dest {
    /// `0` is direct, `1..=BUSES` are the subgroups — the encoding projects and
    /// the command ring carry, so a number is enough to name a destination.
    pub fn from_index(i: usize) -> Self {
        match i {
            0 => Dest::Direct,
            n if n <= BUSES => Dest::Bus(n - 1),
            _ => Dest::Direct,
        }
    }

    pub fn index(self) -> usize {
        match self {
            Dest::Direct => 0,
            Dest::Bus(b) => b + 1,
        }
    }

    /// `OUT`, `A`, `B`, `C`, `D` — what the strip shows.
    pub fn label(self) -> &'static str {
        match self {
            Dest::Direct => "OUT",
            Dest::Bus(0) => "A",
            Dest::Bus(1) => "B",
            Dest::Bus(2) => "C",
            Dest::Bus(3) => "D",
            Dest::Bus(_) => "?",
        }
    }
}

/// One subgroup's strip, as the audio thread holds it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Bus {
    gain: f32,
    mute: bool,
    out_pair: (usize, usize),
}

impl Default for Bus {
    fn default() -> Self {
        Self {
            gain: 1.0,
            mute: false,
            out_pair: (0, 1),
        }
    }
}

/// State owned by the real-time audio callback. No locks, no allocation.
pub(crate) struct RtState {
    pub(crate) playing: Arc<AtomicBool>,
    pub(crate) cmd_rx: rtrb::Consumer<EngineCommand>,
    pub(crate) retired_tx: rtrb::Producer<Retired>,
    pub(crate) slots: Vec<Slot>,
    /// Pre-allocated per-slot mix scratch (interleaved stereo).
    pub(crate) scratch: Vec<f32>,
    /// A second one, for holding a converting tab's input while its instrument
    /// renders over the first.
    pub(crate) dry: Vec<f32>,
    /// One pre-allocated buffer per device output channel. Slots sum into the
    /// pair they are routed to; the backend copies these to its ports.
    pub(crate) mix: Vec<Vec<f32>>,
    /// Two pre-allocated buffers per subgroup, laid out `[bus * 2 + channel]`.
    /// A tab routed to a bus sums here instead of into `mix`, and the bus is
    /// folded into `mix` once its own fader has been applied.
    pub(crate) bus_mix: Vec<Vec<f32>>,
    pub(crate) buses: [Bus; BUSES],
    /// The main strip: the last thing the **first** output pair passes through,
    /// which is the pair everything calls "the output" and the one the meter
    /// reads. The other pairs are separate outputs, not part of a main.
    pub(crate) main_gain: f32,
    pub(crate) main_mute: bool,
    /// One pre-allocated buffer per device *input* channel, filled by the
    /// backend before `render` so a slot can process live audio.
    pub(crate) capture: Vec<Vec<f32>>,
    /// Interleaved input coming from a **separate** cpal input stream.
    ///
    /// JACK hands playback and capture to one callback, so there it is `None`
    /// and the backend writes `capture` directly. Every other backend gives
    /// audio in and audio out their own callbacks on their own clocks, and a
    /// lock-free ring is the only thing that may pass between two audio
    /// threads.
    pub(crate) capture_rx: Option<rtrb::Consumer<f32>>,
    pub(crate) sample_rate: u32,
}

/// Notes a slot can have waiting for their sample. Eight is two bars of
/// sixteenths at the fastest an arpeggiator schedules ahead, and the overflow
/// path is "play it now", not "lose it".
const MAX_SCHEDULED: usize = 8;

/// A note and the transport sample it is for.
#[derive(Clone, Copy)]
struct Scheduled {
    at: u64,
    note: u8,
    vel: u8,
    on: bool,
}

/// Smallest quantum choz will force on the graph. Forcing 64 frames onto a
/// class-compliant USB interface stalls its endpoints (`urb status -32`), and on
/// AMD Renoir xHCI a stalled endpoint can take the whole host controller down
/// ("HC died"), dropping every USB device until a PCI rebind — see
/// `docs/usb-xhci-crash.md`. Below this, choz asks and lets the graph decide.
const MIN_FORCED_QUANTUM: u32 = 128;

/// Tell PipeWire what period to run our node at, before the JACK client exists —
/// the placement is read at client-open time.
///
/// `PIPEWIRE_LATENCY` alone is only a *request*: pipewire-jack opens every JACK
/// client with `node.lock-quantum` and a `node.force-quantum` inherited from
/// whatever the graph happened to be running at, and force beats latency. A
/// client started while the graph sat at 1024 stayed at 1024 (21 ms) no matter
/// what buffer size the settings asked for. `PIPEWIRE_QUANTUM` writes
/// `node.force-quantum` **and `node.force-rate`**, so the configured buffer size
/// is what actually runs.
///
/// # Why it is off unless asked for
///
/// Because that force is not choz's to take. `force-quantum`/`force-rate` move
/// the **whole graph**, not choz's node: every other application on the machine
/// gets resampled to whatever choz asked for, and a browser playing 44.1 kHz
/// through a headset that was happy at its own rate comes out thin and
/// distorted while choz is running. That was reported, and it was choz doing
/// it. The Settings row has said `PW quantum: system` all along — this makes
/// that true instead of decorative.
///
/// `force` is that setting: 0 leaves the graph alone and only *asks*, anything
/// else is the quantum to force. The floor still applies to the force, because
/// a 64-frame quantum stalls a class-compliant USB interface.
fn request_pipewire_period(buffer_size: u32, sample_rate: u32, force: u32) {
    let period = format!("{buffer_size}/{sample_rate}");
    unsafe { std::env::set_var("PIPEWIRE_LATENCY", &period) };
    if force == 0 {
        // Asking, not taking. Whatever the graph is running at, it keeps.
        unsafe { std::env::remove_var("PIPEWIRE_QUANTUM") };
        eprintln!(
            "choz: PipeWire period requested {period} ({:.1} ms), graph left alone",
            period_ms(buffer_size, sample_rate)
        );
        return;
    }
    if force >= MIN_FORCED_QUANTUM {
        let forced = format!("{force}/{sample_rate}");
        unsafe { std::env::set_var("PIPEWIRE_QUANTUM", &forced) };
        eprintln!(
            "choz: PipeWire quantum forced to {forced} ({:.1} ms) \u{2014} this moves the whole graph",
            period_ms(force, sample_rate)
        );
    } else {
        // Never below what the USB bus survives, whatever was asked for.
        unsafe { std::env::remove_var("PIPEWIRE_QUANTUM") };
        eprintln!("choz: PipeWire quantum {force} not forced (< {MIN_FORCED_QUANTUM} frames)");
    }
}

/// The rate to build the engine for: the graph's when it reported one, the
/// saved one when it did not. A graph that answers `0` has not started its
/// driver yet and is telling us nothing.
fn adopt_rate(saved: u32, graph: Option<u32>) -> u32 {
    graph.filter(|r| *r > 0).unwrap_or(saved)
}

fn period_ms(buffer_size: u32, sample_rate: u32) -> f32 {
    buffer_size as f32 * 1000.0 / sample_rate.max(1) as f32
}

// ─── PipeWire / sound server detection ─────────────────────────────────────

/// Returns true when a PipeWire daemon is reachable on this session.
///
/// Checks `$XDG_RUNTIME_DIR/pipewire-0` (Linux) or fallback
/// `/run/user/<uid>/pipewire-0`. No client connection — filesystem check only.
fn pipewire_is_running() -> bool {
    let pipewire_socket = runtime_dir().join("pipewire-0");
    std::env::var("PIPEWIRE_REMOTE").is_ok() || pipewire_socket.exists()
}

fn runtime_dir() -> std::path::PathBuf {
    std::env::var("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let uid = std::process::id(); // not perfect but avoids libc
            std::path::PathBuf::from(format!("/run/user/{uid}"))
        })
}

/// Human-readable label of the detected sound server.
fn detect_sound_server() -> &'static str {
    if pipewire_is_running() {
        return "PipeWire";
    }
    if std::env::var("PULSE_RUNTIME_PATH").is_ok() || std::env::var("PULSE_SERVER").is_ok() {
        return "PulseAudio";
    }
    "ALSA"
}

impl AudioEngine {
    pub fn new(sample_rate: u32, buffer_size: u32) -> Self {
        // SPSC rings: a full command ring drops the excess rather than blocking
        // the RT thread.
        let (cmd_tx, cmd_rx) = rtrb::RingBuffer::new(CMD_RING);
        // At least as large as the command ring: every command retires at most
        // one item, so the RT thread can always hand its cast-offs back. A full
        // retired ring would make it drop them itself — freeing a chain (dlclose
        // on a hosted plugin) inside the audio callback, which hangs the device.
        let (retired_tx, retired_rx) = rtrb::RingBuffer::new(CMD_RING);
        Self {
            sample_rate,
            buffer_size,
            backend: AudioBackend::Alsa,
            playing: Arc::new(AtomicBool::new(false)),
            slot_count: 0,
            editors: Vec::new(),
            fx_editors: Vec::new(),
            touches: Vec::new(),
            fx_touches: Vec::new(),
            states: Vec::new(),
            fx_states: Vec::new(),
            presets: Vec::new(),
            sandboxes: Vec::new(),
            fx_sandboxes: Vec::new(),
            fx_meters: Vec::new(),
            fx_latency: Vec::new(),
            cmd_tx,
            retired_rx,
            output_device: None,
            backend_pref: "AUTO".to_string(),
            rt_endpoints: Some(RtEndpoints { cmd_rx, retired_tx }),
            _stream: None,
            _input_stream: None,
            input_device: None,
            wants_input: false,
            force_quantum: 0,
            out_channels: 2,
            out_wired: 0,
            in_channels: 0,
            input_ports: Vec::new(),
        }
    }

    /// Start the audio stream. Strategy (mirrors seqterm):
    ///
    /// 1. Detect PipeWire: if running, prefer JACK backend with PipeWire's
    ///    JACK compatibility layer (sets PIPEWIRE_QUANTUM for correct buffer).
    /// 2. Fallback: use ALSA (works through PipeWire's ALSA compat too).
    /// 3. If none work, return installation hints.
    pub fn start(&mut self) -> Result<()> {
        let sound_server = detect_sound_server();
        // A position in frames means nothing once the frames change length, so
        // opening a stream is also where the host clock is told its rate (and
        // rewound).
        choz_ports::transport().set_sample_rate(self.sample_rate);

        // The native JACK client comes first: it is the only backend that can
        // address every channel of the interface, which is what per-slot output
        // routing needs. cpal stays as the fallback for boxes without JACK.
        if self.backend_pref != "ALSA" {
            match self.start_jack_native() {
                Ok(()) => {
                    eprintln!(
                        "choz: using native JACK client via {sound_server} (sr={}, buf={}, out={} ch, in={} ch, dev={})",
                        self.sample_rate, self.buffer_size, self.out_channels,
                        self.in_channels,
                        self.output_device.as_deref().unwrap_or("default"),
                    );
                    return Ok(());
                }
                Err(e) => eprintln!("choz: native JACK unavailable ({e}), falling back to cpal..."),
            }
        }

        // Pick backend + device + config BEFORE we touch the RT ring endpoints,
        // so a failed JACK probe can cleanly fall through to ALSA.
        let (device, config, backend) = self
            .pick_backend(sound_server, self.output_device.as_deref())
            .context(no_backend_hint(sound_server))?;

        let ep = self
            .rt_endpoints
            .take()
            .context("audio engine already started")?;

        // Live input, when the user asked for one. Its own stream, its own
        // clock, joined to the output by a ring — see `build_input_stream`.
        let (in_stream, capture_rx, ins, in_name) = match self.open_cpal_input() {
            Some((stream, rx, channels, name)) => (Some(stream), Some(rx), channels, Some(name)),
            None => (None, None, 0, None),
        };
        let state = self.new_rt_state(ep, 2, ins, capture_rx);
        if in_name.is_some() {
            self.input_device = in_name;
        }

        // On JACK the wanted name is a graph sink, not a cpal device: keep it
        // and patch the ports once the stream (and its ports) exist.
        let wanted = self.output_device.take();
        let stream = build_stream(&device, &config, state)?;
        stream.play().context("failed to start audio stream")?;
        self._stream = Some(BackendHandle::Cpal(stream));
        self._input_stream = in_stream;
        self.backend = backend;
        self.out_channels = 2;
        self.in_channels = ins;
        self.input_ports = self.cpal_input_labels(ins);
        self.output_device = match (backend, wanted) {
            (AudioBackend::Jack, Some(sink)) => match jack_route_to(&sink, CPAL_JACK_CLIENT) {
                Ok(()) => Some(sink),
                Err(e) => {
                    eprintln!("choz: {e}, staying on the default output");
                    jack_current_sink()
                }
            },
            (AudioBackend::Jack, None) => jack_current_sink(),
            (_, _) => device.name().ok(),
        };
        eprintln!(
            "choz: using {} backend via {sound_server} (sr={}, buf={}, out={})",
            backend.label(),
            self.sample_rate,
            self.buffer_size,
            self.output_device.as_deref().unwrap_or("default"),
        );
        Ok(())
    }

    /// Open the native JACK client: one port per device channel, wired to the
    /// wanted sink. Errors here are not fatal — `start` falls back to cpal.
    fn start_jack_native(&mut self) -> Result<()> {
        if !pipewire_is_running() && self.backend_pref != "JACK" {
            anyhow::bail!("no JACK graph detected");
        }
        // The graph's rate beats the saved one. `ui.json` is a preference; the
        // running graph is a fact, and JACK never negotiates it per client: it
        // hands out whatever the graph runs at and the client is expected to
        // keep up. Building oxisynth (and every plugin, and the pitch tracker)
        // for 48000 while JACK is delivering 44100 plays the whole rig a
        // semitone and a half flat, for as long as the session lasts, with
        // nothing anywhere saying why.
        let adopted = adopt_rate(self.sample_rate, crate::jack_backend::graph_rate());
        if adopted != self.sample_rate {
            eprintln!(
                "choz: the JACK graph runs at {adopted} Hz, not the saved {} Hz \u{2014} following the graph",
                self.sample_rate
            );
            self.sample_rate = adopted;
            choz_ports::transport().set_sample_rate(adopted);
        }
        request_pipewire_period(self.buffer_size, self.sample_rate, self.force_quantum);

        let sink = self.output_device.clone().or_else(jack_current_sink);
        // An unknown sink still gets a stereo client: the user can patch it.
        let (outs, sink_ins) = sink
            .as_deref()
            .and_then(crate::jack_backend::device_channels)
            .unwrap_or((2, 0));
        // Every capture jack in the graph, whatever card it belongs to: the
        // sink's own capture ports (what this used to ask for) are none at all
        // on PipeWire, where an interface is two nodes.
        let _ = sink_ins;
        let capture = crate::jack_backend::all_capture_ports();
        let ins = capture.len();

        let ep = self
            .rt_endpoints
            .take()
            .context("audio engine already started")?;
        let state = self.new_rt_state(ep, outs.max(2), ins, None);

        match crate::jack_backend::start(sink.as_deref(), &capture, outs.max(2), state) {
            Ok((handle, channels, wired)) => {
                self._stream = Some(BackendHandle::Jack(Box::new(handle)));
                self.backend = AudioBackend::Jack;
                self.out_channels = channels;
                self.in_channels = ins;
                self.input_ports = capture;
                // The device the audio actually reached, which is not always
                // the one that was asked for: a saved name outlives the box.
                self.out_wired = wired.as_ref().map(|(_, n)| *n).unwrap_or(0);
                self.output_device = wired
                    .map(|(name, _)| name)
                    .or(sink)
                    .or_else(jack_current_sink);
                Ok(())
            }
            Err(e) => {
                // The endpoints went into the state we just lost. Fresh rings on
                // both ends put the engine back where `new` left it, so the cpal
                // path can still start — no slots exist this early, so nothing
                // queued is lost.
                let (cmd_tx, cmd_rx) = rtrb::RingBuffer::new(CMD_RING);
                let (retired_tx, retired_rx) = rtrb::RingBuffer::new(CMD_RING);
                self.cmd_tx = cmd_tx;
                self.retired_rx = retired_rx;
                self.rt_endpoints = Some(RtEndpoints { cmd_rx, retired_tx });
                Err(e)
            }
        }
    }

    /// Rebuild the native client on `sink` with `outs`/`ins` ports. Every slot
    /// is lost (they live in the old client's RT state), which is why the
    /// caller is told to reload the rack.
    fn restart_jack_native(&mut self, sink: Option<&str>, outs: usize) -> Result<()> {
        // The graph is re-read here rather than passed in: a card that came or
        // went since the last client is exactly what a restart is for.
        let capture = crate::jack_backend::all_capture_ports();
        let ins = capture.len();
        // Fresh rings: the old ones belong to the client we are about to drop.
        let (cmd_tx, cmd_rx) = rtrb::RingBuffer::new(CMD_RING);
        let (retired_tx, retired_rx) = rtrb::RingBuffer::new(CMD_RING);
        let state = self.new_rt_state(RtEndpoints { cmd_rx, retired_tx }, outs, ins, None);

        // Drop first: two live clients would fight over the name `choz`, and
        // the graph would rename the second one. If the new client then fails
        // to open there is no audio until the next attempt — the error says so.
        self._stream = None;
        let (handle, channels, wired) = crate::jack_backend::start(sink, &capture, outs, state)
            .context("cannot reopen the JACK client")?;

        self.cmd_tx = cmd_tx;
        self.retired_rx = retired_rx;
        self.slot_count = 0;
        self.editors.clear();
        self.fx_editors.clear();
        self.touches.clear();
        self.fx_touches.clear();
        self.states.clear();
        self.fx_states.clear();
        self.presets.clear();
        self.sandboxes.clear();
        self.fx_sandboxes.clear();
        self.fx_meters.clear();
        self.fx_latency.clear();
        self._stream = Some(BackendHandle::Jack(Box::new(handle)));
        self.out_channels = channels;
        self.in_channels = ins;
        self.input_ports = capture;
        self.out_wired = wired.as_ref().map(|(_, n)| *n).unwrap_or(0);
        self.output_device = wired
            .map(|(name, _)| name)
            .or_else(|| sink.map(str::to_string))
            .or_else(jack_current_sink);
        Ok(())
    }

    /// RT state sized for `outs` output and `ins` input channels.
    ///
    /// `capture_rx` is the ring a separate input stream fills; `None` on the
    /// native JACK client, which writes the capture buffers itself.
    fn new_rt_state(
        &self,
        ep: RtEndpoints,
        outs: usize,
        ins: usize,
        capture_rx: Option<rtrb::Consumer<f32>>,
    ) -> RtState {
        // Frames a callback can ever hand us: the configured buffer, but never
        // less than 8192 — cpal and PipeWire both hand out bigger blocks than
        // the size we asked for.
        let frames = self.buffer_size.max(8192) as usize;
        RtState {
            playing: Arc::clone(&self.playing),
            cmd_rx: ep.cmd_rx,
            retired_tx: ep.retired_tx,
            slots: Vec::with_capacity(MAX_SLOTS),
            scratch: vec![0.0; frames * 2],
            dry: vec![0.0; frames * 2],
            mix: vec![vec![0.0; frames]; outs.max(2)],
            bus_mix: vec![vec![0.0; frames]; BUSES * 2],
            buses: [Bus::default(); BUSES],
            main_gain: 1.0,
            main_mute: false,
            capture: vec![vec![0.0; frames]; ins],
            capture_rx,
            sample_rate: self.sample_rate,
        }
    }

    /// Output devices offered by the current backend, by name.
    ///
    /// On JACK the list comes from the graph, not from cpal — cpal's JACK host
    /// invents a single `cpal_client_out` device, whereas the graph names every
    /// real sink PipeWire is publishing.
    pub fn output_devices(&self) -> Vec<String> {
        if self.backend == AudioBackend::Jack {
            let sinks = jack_sinks();
            if !sinks.is_empty() {
                return sinks;
            }
        }
        match cpal::default_host().output_devices() {
            Ok(devs) => devs.filter_map(|d| d.name().ok()).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Name of the device audio is currently going to.
    pub fn output_device(&self) -> Option<&str> {
        self.output_device.as_deref()
    }

    /// Whether the mix is actually reaching a device.
    ///
    /// Only the native JACK client can answer honestly — it does its own
    /// patching, so it knows. On cpal the stream *is* the connection: if it
    /// opened, it is wired.
    pub fn output_wired(&self) -> bool {
        match self._stream {
            Some(BackendHandle::Jack(_)) => self.out_wired > 0,
            Some(BackendHandle::Cpal(_)) => true,
            None => false,
        }
    }

    /// The graph port behind each input channel, in channel order.
    ///
    /// Under the native JACK client this is every capture jack in the system,
    /// so what the user picks is a **channel**. On the cpal backends (ALSA,
    /// PulseAudio, PipeWire) there is one input *device* and these are its
    /// channels, named after it.
    pub fn input_ports(&self) -> &[String] {
        &self.input_ports
    }

    /// Capture devices the current backend can open, by name.
    ///
    /// On JACK the answer is the graph's capture ports — there is no device to
    /// choose, every jack is already wired — so this is the cpal list, which is
    /// what the Settings row offers on ALSA / PulseAudio / PipeWire.
    pub fn input_devices(&self) -> Vec<String> {
        match cpal::default_host().input_devices() {
            Ok(devs) => devs.filter_map(|d| d.name().ok()).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// The capture device in use, if any.
    pub fn input_device(&self) -> Option<&str> {
        self.input_device.as_deref()
    }

    /// Whether a capture stream is open (or wanted, before the stream is up).
    pub fn input_enabled(&self) -> bool {
        self.wants_input
    }

    /// Choose the capture device. `None` turns live input off.
    ///
    /// Returns `true` when the stream had to be rebuilt, which loses every
    /// engine slot — the caller reloads the rack, exactly as it does for an
    /// output change. On the native JACK client there is nothing to choose and
    /// this is a no-op: the graph's jacks are already all wired.
    pub fn set_input_device(&mut self, name: Option<String>) -> Result<bool> {
        if self.backend == AudioBackend::Jack
            && matches!(self._stream, Some(BackendHandle::Jack(_)))
        {
            return Ok(false);
        }
        let same = self.input_device.as_deref() == name.as_deref();
        let was = self.wants_input;
        self.wants_input = name.is_some();
        self.input_device = name;
        if same && was == self.wants_input {
            return Ok(false);
        }
        self.restart_cpal().map(|()| true)
    }

    /// Open the capture stream the user asked for, if any. Failure is not
    /// fatal: choz plays without an input rather than not starting.
    fn open_cpal_input(&self) -> Option<(cpal::Stream, rtrb::Consumer<f32>, usize, String)> {
        if !self.wants_input {
            return None;
        }
        match build_input_stream(
            &cpal::default_host(),
            self.input_device.as_deref(),
            self.sample_rate,
            self.buffer_size,
        ) {
            Ok(open) => Some(open),
            Err(e) => {
                eprintln!("choz: no audio input ({e})");
                None
            }
        }
    }

    /// Channel labels for a cpal capture device: what the IN drawer lists.
    fn cpal_input_labels(&self, channels: usize) -> Vec<String> {
        let name = self.input_device.as_deref().unwrap_or("input");
        (0..channels)
            .map(|i| format!("{name}:in_{}", i + 1))
            .collect()
    }

    /// Re-read the graph and rebuild the client, so a card plugged in after
    /// start-up shows up. Every slot is lost, exactly like an output change,
    /// and the caller reloads the rack.
    pub fn rescan_inputs(&mut self) -> Result<bool> {
        if !matches!(self._stream, Some(BackendHandle::Jack(_))) {
            // On the cpal backends there is a device rather than a graph, so a
            // rescan is "open it again": a headset plugged in after start-up
            // appears in the list, and re-opening picks up its channel count.
            if self.wants_input {
                return self.restart_cpal().map(|()| true);
            }
            return Ok(false);
        }
        // The sink may never have been named — PipeWire auto-connects us and
        // `output_device` stays empty until someone picks one. Reconnect to
        // whatever we are wired to now.
        let sink = self.output_device.clone().or_else(jack_current_sink);
        self.restart_jack_native(sink.as_deref(), self.out_channels)
            .map(|()| true)
    }

    /// Move playback to `name`. Returns `true` when the stream had to be rebuilt,
    /// which loses **every rack slot** — the caller must then re-add them (the UI
    /// owns the rack model and reloads it). On JACK the ports are simply
    /// re-patched, nothing is lost and `false` comes back. Returns an error and
    /// keeps the current output if the device can't be opened.
    pub fn set_output_device(&mut self, name: &str) -> Result<bool> {
        match self._stream {
            // Native client. Same channel count → re-patch our ports and keep
            // playing. A different one (the 2-channel default → a 12-output
            // interface) needs a new set of ports, so the client is rebuilt and
            // the caller reloads the rack.
            Some(BackendHandle::Jack(_)) => {
                let (outs, _) = crate::jack_backend::device_channels(name).unwrap_or((2, 0));
                let outs = outs.max(2);
                // Inputs do not depend on the sink any more — they are the
                // whole graph's — so only the output count can force a rebuild.
                if outs == self.out_channels {
                    jack_route_to(name, crate::jack_backend::CLIENT_NAME)?;
                    self.output_device = Some(name.to_string());
                    return Ok(false);
                }
                return self.restart_jack_native(Some(name), outs).map(|()| true);
            }
            Some(BackendHandle::Cpal(_)) if self.backend == AudioBackend::Jack => {
                jack_route_to(name, CPAL_JACK_CLIENT)?;
                self.output_device = Some(name.to_string());
                return Ok(false);
            }
            _ => {}
        }
        self.output_device = Some(name.to_string());
        self.restart_cpal().map(|()| true)
    }

    /// Tear the cpal streams down and open them again from the current
    /// preferences — output device, input device, rate, buffer.
    ///
    /// **Every engine slot is lost**: the RT state lives inside the callback
    /// being dropped. The caller reloads the rack, which is what
    /// `App::rebuild_rack` is for. Used by both device changes, because
    /// "restart the audio" is one operation whichever end of it moved.
    fn restart_cpal(&mut self) -> Result<()> {
        let want = self.output_device.clone();
        let sound_server = detect_sound_server();
        let (device, config, backend) = self
            .pick_backend(sound_server, want.as_deref())
            .with_context(|| match want.as_deref() {
                Some(name) => format!("cannot open output device '{name}'"),
                None => "cannot open an output device".to_string(),
            })?;

        // Fresh rings: the old ones are owned by the callback we're dropping.
        let (cmd_tx, cmd_rx) = rtrb::RingBuffer::new(CMD_RING);
        // At least as large as the command ring: every command retires at most
        // one item, so the RT thread can always hand its cast-offs back. A full
        // retired ring would make it drop them itself — freeing a chain (dlclose
        // on a hosted plugin) inside the audio callback, which hangs the device.
        let (retired_tx, retired_rx) = rtrb::RingBuffer::new(CMD_RING);
        // The input goes up first: if the user asked for a capture device that
        // cannot be opened, they should hear about it before the rack is torn
        // down, and `open_cpal_input` has already said so on stderr.
        let (in_stream, capture_rx, ins, in_name) = match self.open_cpal_input() {
            Some((stream, rx, channels, name)) => (Some(stream), Some(rx), channels, Some(name)),
            None => (None, None, 0, None),
        };
        if in_name.is_some() {
            self.input_device = in_name;
        }
        let state = self.new_rt_state(RtEndpoints { cmd_rx, retired_tx }, 2, ins, capture_rx);
        let stream = build_stream(&device, &config, state)?;
        stream.play().context("failed to start audio stream")?;

        // The numbers belong to the device that is open now.
        crate::meter::capture_health().clear();
        // Drops the previous streams (and its slots).
        self._stream = Some(BackendHandle::Cpal(stream));
        self._input_stream = in_stream;
        self.out_channels = 2;
        self.in_channels = ins;
        self.input_ports = self.cpal_input_labels(ins);
        self.cmd_tx = cmd_tx;
        self.retired_rx = retired_rx;
        self.slot_count = 0;
        self.editors.clear();
        self.fx_editors.clear();
        self.touches.clear();
        self.fx_touches.clear();
        self.states.clear();
        self.fx_states.clear();
        self.presets.clear();
        self.sandboxes.clear();
        self.fx_sandboxes.clear();
        self.fx_meters.clear();
        self.fx_latency.clear();
        self.backend = backend;
        self.output_device = device.name().ok();
        Ok(())
    }

    /// Choose a working (device, config, backend) without consuming RT state.
    /// `want` names a specific output device; `None` takes the host default.
    /// Ask for a specific backend on the next [`Self::start`]. Values other
    /// than `AUTO`/`JACK`/`PIPEWIRE`/`ALSA` are ignored.
    pub fn set_backend_preference(&mut self, pref: &str) {
        let pref = pref.to_uppercase();
        if matches!(pref.as_str(), "AUTO" | "JACK" | "PIPEWIRE" | "ALSA") {
            self.backend_pref = pref;
        }
    }

    /// Force a PipeWire quantum on the graph, or 0 to leave it alone. Applied
    /// on the next [`Self::start`].
    pub fn set_force_quantum(&mut self, frames: u32) {
        self.force_quantum = frames;
    }

    /// Ask for a specific output device on the next [`Self::start`].
    pub fn set_output_device_preference(&mut self, name: &str) {
        self.output_device = Some(name.to_string());
    }

    /// Ask for a capture device on the next [`Self::start`]. An empty name
    /// means "the host default"; `None` means no live input at all.
    pub fn set_input_device_preference(&mut self, name: Option<&str>) {
        self.wants_input = name.is_some();
        self.input_device = name.filter(|n| !n.is_empty()).map(str::to_string);
    }

    pub fn backend_preference(&self) -> &str {
        &self.backend_pref
    }

    fn pick_backend(
        &self,
        sound_server: &str,
        want: Option<&str>,
    ) -> Result<(cpal::Device, cpal::StreamConfig, AudioBackend)> {
        // An explicit ALSA choice skips the JACK probe entirely.
        if self.backend_pref == "ALSA" {
            let (dev, cfg) = self.alsa_device_config(want)?;
            return Ok((dev, cfg, AudioBackend::Alsa));
        }
        let force_jack = matches!(self.backend_pref.as_str(), "JACK" | "PIPEWIRE");
        if force_jack || pipewire_is_running() {
            request_pipewire_period(self.buffer_size, self.sample_rate, self.force_quantum);

            match self.jack_device_config(want) {
                Ok((dev, cfg)) => return Ok((dev, cfg, AudioBackend::Jack)),
                Err(e) => eprintln!(
                    "choz: JACK backend unavailable ({e}), trying ALSA via {sound_server}..."
                ),
            }
        }
        let (dev, cfg) = self.alsa_device_config(want)?;
        Ok((dev, cfg, AudioBackend::Alsa))
    }

    /// `want` is deliberately ignored here: on JACK the wanted name is a graph
    /// sink, not one of cpal's devices (cpal only ever offers its own
    /// `cpal_client_out`). The stream opens on that one device and `start`
    /// patches its ports onto the wanted sink afterwards.
    fn jack_device_config(
        &self,
        _want: Option<&str>,
    ) -> Result<(cpal::Device, cpal::StreamConfig)> {
        let host = cpal::host_from_id(cpal::HostId::Jack)
            .context("JACK host not available (is libjack installed?)")?;
        let device = named_or_default(&host, None).context("no JACK output device found")?;
        let config = pick_config(&device, self.sample_rate, self.buffer_size)?;
        Ok((device, config))
    }

    fn alsa_device_config(&self, want: Option<&str>) -> Result<(cpal::Device, cpal::StreamConfig)> {
        let device = named_or_default(&cpal::default_host(), want)
            .context("no output audio device found")?;
        let config = pick_config(&device, self.sample_rate, self.buffer_size)?;
        Ok((device, config))
    }

    /// Number of rack slots.
    pub fn slot_count(&self) -> usize {
        self.slot_count
    }

    /// Handle to slot `slot`'s plugin window, when its instrument has one.
    pub fn slot_editor(&self, slot: usize) -> Option<choz_ports::EditorHandle> {
        self.editors.get(slot).cloned().flatten()
    }

    /// Handle to the window of FX `fx` in slot `slot`, when it's a plugin with
    /// an editor.
    pub fn fx_editor(&self, slot: usize, fx: usize) -> Option<choz_ports::EditorHandle> {
        self.fx_editors.get(slot)?.get(fx).cloned().flatten()
    }

    /// Slot `slot`'s instrument state — its patch, for the project file.
    /// `None` when the plugin has none or the format cannot report one.
    pub fn slot_state(&self, slot: usize) -> Option<Vec<u8>> {
        self.states.get(slot)?.as_ref()?.save()
    }

    /// Whether slot `slot`'s instrument can be handed a state blob at all.
    /// Cheap — it asks the handle, not the plugin — because the UI reads it
    /// every frame to decide whether the tab can take a folder of presets.
    pub fn slot_has_state(&self, slot: usize) -> bool {
        matches!(self.states.get(slot), Some(Some(_)))
    }

    /// Restore a blob saved by [`Self::slot_state`] onto the same plugin.
    pub fn set_slot_state(&self, slot: usize, data: &[u8]) {
        if let Some(Some(h)) = self.states.get(slot) {
            h.restore(data);
        }
    }

    /// Everything slot `slot`'s instrument offers in its own preset browser.
    /// Empty for a SoundFont (its programs are the engine's own business), for
    /// an effect, and for any plugin whose format cannot report presets.
    pub fn slot_presets(&self, slot: usize) -> Vec<choz_ports::PresetEntry> {
        match self.presets.get(slot) {
            Some(Some(h)) => h.list(),
            _ => Vec::new(),
        }
    }

    /// The key of the preset slot `slot`'s instrument says it is on, when its
    /// format can be asked. `None` otherwise — most of them.
    pub fn slot_current_preset(&self, slot: usize) -> Option<String> {
        match self.presets.get(slot) {
            Some(Some(h)) => h.current(),
            _ => None,
        }
    }

    /// Load one of them, by the key [`Self::slot_presets`] handed out. Runs on
    /// the calling (UI) thread: every format allocates or reads files here.
    pub fn load_slot_preset(&self, slot: usize, key: &str) {
        if let Some(Some(h)) = self.presets.get(slot) {
            h.load(key);
        }
    }

    /// Same for an effect in the slot's chain.
    pub fn fx_state(&self, slot: usize, fx: usize) -> Option<Vec<u8>> {
        self.fx_states.get(slot)?.get(fx)?.as_ref()?.save()
    }

    pub fn set_fx_state(&self, slot: usize, fx: usize, data: &[u8]) {
        if let Some(Some(h)) = self.fx_states.get(slot).and_then(|v| v.get(fx)) {
            h.restore(data);
        }
    }

    /// The parameter the user last moved inside slot `slot`'s instrument
    /// window, if that plugin reports them. Reading it consumes it.
    pub fn slot_touched_param(&self, slot: usize) -> Option<(u32, f32)> {
        self.touches.get(slot)?.as_ref()?.take_touched()
    }

    /// Same for an effect in the slot's chain.
    pub fn fx_touched_param(&self, slot: usize, fx: usize) -> Option<(u32, f32)> {
        self.fx_touches.get(slot)?.get(fx)?.as_ref()?.take_touched()
    }

    /// Live counters when slot `slot`'s instrument plays in its own process.
    /// Blocks every sandboxed plugin in the rack has failed to answer in time,
    /// and how many times one has crashed and come back. Each missed block is a
    /// hole the user heard, so this is half of "why did the sound break up".
    pub fn sandbox_health(&self) -> (u64, u64) {
        let all = self
            .sandboxes
            .iter()
            .flatten()
            .chain(self.fx_sandboxes.iter().flatten().flatten());
        all.fold((0, 0), |(m, r), s| (m + s.missed(), r + s.restarts()))
    }

    pub fn slot_sandbox(&self, slot: usize) -> Option<choz_ports::SandboxStatus> {
        self.sandboxes.get(slot).cloned().flatten()
    }

    /// Same for FX `fx` of slot `slot`.
    pub fn fx_sandbox(&self, slot: usize, fx: usize) -> Option<choz_ports::SandboxStatus> {
        self.fx_sandboxes.get(slot)?.get(fx).cloned().flatten()
    }

    /// Peak in and out of FX `fx` in slot `slot`, when that effect meters
    /// itself. `None` for everything that does not.
    pub fn fx_meter(&self, slot: usize, fx: usize) -> Option<choz_ports::FxMeter> {
        self.fx_meters.get(slot)?.get(fx).cloned().flatten()
    }

    /// How much delay slot `slot`'s FX chain adds, in samples: the sum of what
    /// every effect in it reports.
    ///
    /// choz does not compensate it — there is no arrangement to line up
    /// against — but the number is the difference between "the rack feels
    /// sluggish" and "the lookahead limiter costs 5 ms", so the rack shows it.
    pub fn slot_latency(&self, slot: usize) -> u32 {
        self.fx_latency
            .get(slot)
            .map_or(0, |chain| chain.iter().copied().sum())
    }

    fn drain_retired(&mut self) {
        while let Ok(retired) = self.retired_rx.pop() {
            drop(retired);
        }
    }

    fn send(&mut self, cmd: EngineCommand) {
        self.drain_retired();
        // A full ring means the RT thread is behind; drop rather than block.
        let _ = self.cmd_tx.push(cmd);
    }

    /// Play a note into a freshly-built instrument and measure how loud it is,
    /// **before** it reaches the audio thread.
    ///
    /// This is the reading the interface trims the tab's fader from, and taking it
    /// here is what makes the trim happen at load rather than after the player has
    /// already been deafened by the first chord. The source is not in a slot yet,
    /// so this thread owns it outright: rendering it costs nobody a deadline, and
    /// it runs as fast as the CPU allows rather than in real time.
    ///
    /// Only instruments are probed — [`AudioSource::plays_on_transport_stop`] is
    /// what separates them from a WAV or a tone, and rendering one of those would
    /// eat the first half-second of the file before the play button was ever
    /// pressed. A plugin that answers with silence (it wants warming up, or the
    /// note lands where it has no sample) publishes nothing, and the fader is left
    /// where it was rather than moved on a reading of nothing.
    fn probe_levels(source: &mut Source, slot: usize, sample_rate: u32) {
        /// Long enough for an attack and the start of a decay, short enough that a
        /// load does not visibly wait on it.
        const SECONDS: f32 = 0.6;
        /// Middle C at a normal velocity: what a player checking a new patch hits.
        const NOTE: u8 = 60;
        const VELOCITY: u8 = 100;
        const BLOCK: usize = 256;

        let levels = crate::meter::slot_levels();
        levels.reset(slot);
        if sample_rate == 0 || !source.plays_on_transport_stop() {
            return;
        }
        let mut buf = vec![0.0f32; BLOCK * 2];
        source.note_on(NOTE, VELOCITY);
        let blocks = ((sample_rate as f32 * SECONDS) as usize).div_ceil(BLOCK);
        for _ in 0..blocks {
            buf.fill(0.0);
            // A source is not required to overwrite what it is handed, so the
            // buffer is cleared each time and everything past what it wrote is
            // silence by construction.
            let written = source.render(&mut buf, sample_rate);
            levels.publish(slot, &buf[..(written * 2).min(buf.len())]);
        }
        // Hand it over silent. The note was ours, not the player's.
        source.note_off(NOTE);
        source.all_notes_off();
    }

    /// Append a source as a new rack slot. Returns its index. No-op past
    /// [`MAX_SLOTS`].
    fn add_slot(&mut self, mut source: Source) -> Option<usize> {
        if self.slot_count >= MAX_SLOTS {
            return None;
        }
        let idx = self.slot_count;
        Self::probe_levels(&mut source, idx, self.sample_rate);
        self.editors.push(source.editor());
        self.touches.push(source.param_touch());
        self.states.push(source.state());
        self.presets.push(source.presets());
        self.sandboxes.push(source.sandbox());
        self.fx_editors.push(Vec::new());
        self.fx_touches.push(Vec::new());
        self.fx_states.push(Vec::new());
        self.fx_sandboxes.push(Vec::new());
        self.fx_meters.push(Vec::new());
        self.fx_latency.push(Vec::new());
        self.send(EngineCommand::AddSlot(source));
        self.slot_count += 1;
        Some(idx)
    }

    /// Remove the slot at `slot`. Later slots shift down by one.
    pub fn remove_slot(&mut self, slot: usize) {
        if slot < self.slot_count {
            self.send(EngineCommand::RemoveSlot(slot));
            // Later tabs shift down a place, so every stored level now belongs
            // to a different tab. Start them all again rather than keep a set
            // of readings that are off by one.
            crate::meter::slot_levels().reset_all();
            self.slot_count -= 1;
            self.editors.remove(slot);
            self.fx_editors.remove(slot);
            self.touches.remove(slot);
            self.fx_touches.remove(slot);
            self.states.remove(slot);
            self.fx_states.remove(slot);
            self.presets.remove(slot);
            self.sandboxes.remove(slot);
            self.fx_sandboxes.remove(slot);
            self.fx_meters.remove(slot);
            self.fx_latency.remove(slot);
        }
    }

    /// Rebuild slot `slot`'s FX chain from specs (built off the RT thread).
    pub fn set_slot_fx(&mut self, slot: usize, specs: Vec<FxSpec>) {
        if slot >= self.slot_count {
            return;
        }
        let fx = build_chain_from_specs(&specs, self.sample_rate, self.buffer_size);
        // Last chance to reach the processors: after this they belong to the
        // RT thread.
        self.fx_editors[slot] = fx.iter().map(|p| p.editor()).collect();
        self.fx_touches[slot] = fx.iter().map(|p| p.param_touch()).collect();
        self.fx_states[slot] = fx.iter().map(|p| p.state()).collect();
        self.fx_sandboxes[slot] = fx.iter().map(|p| p.sandbox()).collect();
        self.fx_meters[slot] = fx.iter().map(|p| p.meter()).collect();
        self.fx_latency[slot] = fx.iter().map(|p| p.latency_samples()).collect();
        self.send(EngineCommand::SetSlotFx { slot, fx });
    }

    /// Device output channels the running backend exposes. 2 on cpal; the
    /// interface's real count under the native JACK client.
    pub fn output_channels(&self) -> usize {
        self.out_channels
    }

    /// Device input channels available as slot sources (native JACK only).
    pub fn input_channels(&self) -> usize {
        self.in_channels
    }

    /// Route slot `slot` to a device output pair, 0-based: `(0, 1)` is the
    /// first pair, `(4, 5)` the third. Channels past the device's count fold
    /// onto the last one.
    pub fn set_slot_out(&mut self, slot: usize, left: usize, right: usize) {
        if slot >= self.slot_count {
            return;
        }
        self.send(EngineCommand::SetSlotOut { slot, left, right });
    }

    /// Feed slot `slot` from a device input pair instead of its own source;
    /// `None` puts its instrument back in charge.
    pub fn set_slot_in(&mut self, slot: usize, pair: Option<(usize, usize)>) {
        if slot >= self.slot_count {
            return;
        }
        self.send(EngineCommand::SetSlotIn { slot, pair });
    }

    /// Turn a tab's audio input into notes for its own instrument — a guitar
    /// playing a synth. Only means anything on a tab fed by a capture pair.
    pub fn set_slot_pitch_to_midi(&mut self, slot: usize, on: bool) {
        self.send(EngineCommand::SetSlotPitchToMidi { slot, on });
    }

    /// How much of a converting tab is the instrument and how much is the
    /// audio that drove it. 1 = only the instrument.
    pub fn set_slot_pitch_mix(&mut self, slot: usize, mix: f32) {
        if slot >= self.slot_count {
            return;
        }
        self.send(EngineCommand::SetSlotPitchMix { slot, mix });
    }

    /// Trim the slot's audio input (linear `gain`) and set how loud it must be
    /// before `A→M` hears a note (`gate`, RMS 0..1). Only means anything on a
    /// tab fed by a capture channel.
    pub fn set_slot_in_trim(&mut self, slot: usize, gain: f32, gate: f32) {
        self.send(EngineCommand::SetSlotInTrim { slot, gain, gate });
    }

    /// Point slot `slot` at a device pair or a subgroup. The pair itself is
    /// still [`Self::set_slot_out`]'s — a tab keeps where it would land if it
    /// were taken off the bus again.
    pub fn set_slot_dest(&mut self, slot: usize, dest: Dest) {
        if slot >= self.slot_count {
            return;
        }
        self.send(EngineCommand::SetSlotDest { slot, dest });
    }

    /// One subgroup's fader, mute and output pair.
    pub fn set_bus(&mut self, bus: usize, gain: f32, mute: bool, pair: (usize, usize)) {
        if bus >= BUSES {
            return;
        }
        self.send(EngineCommand::SetBus {
            bus,
            gain,
            mute,
            left: pair.0,
            right: pair.1,
        });
    }

    /// The main strip: one fader over the first output pair.
    pub fn set_main(&mut self, gain: f32, mute: bool) {
        self.send(EngineCommand::SetMain { gain, mute });
    }

    /// Set slot `slot`'s mixer strip: linear `gain`, `pan` (-1 left .. 1 right)
    /// and `mute`.
    pub fn set_slot_mix(&mut self, slot: usize, gain: f32, gain_r: f32, pan: f32, mute: bool) {
        if slot >= self.slot_count {
            return;
        }
        self.send(EngineCommand::SetSlotMix {
            slot,
            gain,
            gain_r,
            pan,
            mute,
        });
    }

    /// Change one parameter of the FX at `fx` in slot `slot`'s chain, without
    /// rebuilding the chain. `value` is a normalised 0..1 knob position.
    pub fn set_fx_param(&mut self, slot: usize, fx: usize, index: usize, value: f32) {
        if slot >= self.slot_count {
            return;
        }
        self.send(EngineCommand::SetFxParam {
            slot,
            fx,
            index,
            value,
        });
    }

    /// Change one parameter of slot `slot`'s instrument. `value` is a normalised
    /// 0..1 knob position; sources without parameters ignore it.
    pub fn set_slot_param(&mut self, slot: usize, index: usize, value: f32) {
        if slot >= self.slot_count {
            return;
        }
        self.send(EngineCommand::SetSlotParam { slot, index, value });
    }

    /// Select a bank/preset on slot `slot` (SF2 program change).
    pub fn set_slot_program(&mut self, slot: usize, bank: u8, preset: u8) {
        if slot >= self.slot_count {
            return;
        }
        self.send(EngineCommand::SetSlotProgram { slot, bank, preset });
    }

    /// Replace slot `slot`'s source. The old one is dropped off the RT thread.
    fn set_slot_source(&mut self, slot: usize, mut source: Source) {
        if slot >= self.slot_count {
            return;
        }
        Self::probe_levels(&mut source, slot, self.sample_rate);
        self.editors[slot] = source.editor();
        self.touches[slot] = source.param_touch();
        self.states[slot] = source.state();
        self.presets[slot] = source.presets();
        self.sandboxes[slot] = source.sandbox();
        self.send(EngineCommand::SetSlotSource { slot, source });
    }

    /// Add a slot with no instrument yet (renders silence). Used when a rack tab
    /// is created for an input before an instrument is chosen.
    pub fn add_silent(&mut self) -> Option<usize> {
        self.add_slot(Box::new(crate::sources::Silence))
    }

    /// Add a WAV-playback slot.
    pub fn add_wav(&mut self, path: &std::path::Path, looping: bool) -> Result<Option<usize>> {
        let player = WavPlayer::load(path, looping)?;
        Ok(self.add_slot(Box::new(player)))
    }

    /// Add an SF2 SoundFont instrument slot (MIDI-playable).
    pub fn add_sf2(
        &mut self,
        path: &std::path::Path,
        bank: u8,
        preset: u8,
    ) -> Result<Option<usize>> {
        let synth = Sf2Synth::load(path, bank, preset, self.sample_rate)?;
        Ok(self.add_slot(Box::new(synth)))
    }

    /// Load a WAV as slot `slot`'s source, replacing whatever was there.
    pub fn load_wav(&mut self, slot: usize, path: &std::path::Path, looping: bool) -> Result<()> {
        let player = WavPlayer::load(path, looping)?;
        self.set_slot_source(slot, Box::new(player));
        Ok(())
    }

    /// Load an SF2 as slot `slot`'s source, replacing whatever was there.
    pub fn load_sf2(
        &mut self,
        slot: usize,
        path: &std::path::Path,
        bank: u8,
        preset: u8,
    ) -> Result<()> {
        let synth = Sf2Synth::load(path, bank, preset, self.sample_rate)?;
        self.set_slot_source(slot, Box::new(synth));
        Ok(())
    }

    /// Scan every configured plugin directory, all formats.
    pub fn scan_plugins(&self, paths: &crate::PluginPaths) -> Vec<crate::FoundPlugin> {
        crate::scan_all(paths)
    }

    /// Discovered plugins, served from the on-disk cache when it's still fresh.
    /// Use this at startup; [`Self::rescan_plugins`] for an explicit refresh.
    pub fn cached_plugins(&self, paths: &crate::PluginPaths) -> Vec<crate::FoundPlugin> {
        // The config file counts as an input: editing the search paths must
        // invalidate a cache written before the edit.
        let mut dirs = paths.all_enabled();
        dirs.push(crate::PluginPaths::config_file());
        crate::cache::cached_or_scan(&dirs, || self.scan_plugins(paths))
    }

    /// Force a full scan and refresh the cache.
    pub fn rescan_plugins(&self, paths: &crate::PluginPaths) -> Vec<crate::FoundPlugin> {
        crate::cache::rescan(|| self.scan_plugins(paths))
    }

    /// Add a CLAP instrument slot.
    fn add_clap(&mut self, path: &std::path::Path, plugin_id: &str) -> Result<Option<usize>> {
        let inst = choz_plugin_clap::host::ClapInstrument::build(
            path,
            plugin_id,
            self.sample_rate,
            self.buffer_size,
        )
        .ok_or_else(|| anyhow::anyhow!("failed to instantiate CLAP plugin: {plugin_id}"))?;
        Ok(self.add_slot(Box::new(inst)))
    }

    /// Load a CLAP instrument as slot `slot`'s source.
    fn load_clap(&mut self, slot: usize, path: &std::path::Path, plugin_id: &str) -> Result<()> {
        let inst = choz_plugin_clap::host::ClapInstrument::build(
            path,
            plugin_id,
            self.sample_rate,
            self.buffer_size,
        )
        .ok_or_else(|| anyhow::anyhow!("failed to instantiate CLAP plugin: {plugin_id}"))?;
        self.set_slot_source(slot, Box::new(inst));
        Ok(())
    }

    /// Add a hosted plugin instrument as a new slot.
    pub fn add_plugin(
        &mut self,
        format: crate::PluginFormat,
        path: &std::path::Path,
        id: &str,
    ) -> Result<Option<usize>> {
        refuse_if_quarantined(format, path, id)?;
        match format {
            crate::PluginFormat::Clap => self.add_clap(path, id),
            crate::PluginFormat::Lv2
            | crate::PluginFormat::Dssi
            | crate::PluginFormat::Vst2
            | crate::PluginFormat::Vst3
            | crate::PluginFormat::Sfz => {
                Ok(self.add_slot(self.build_instrument(format, path, id)?))
            }
            _ => anyhow::bail!("{} hosting is not implemented yet", format.label()),
        }
    }

    /// Load a hosted plugin instrument as slot `slot`'s source.
    pub fn load_plugin(
        &mut self,
        slot: usize,
        format: crate::PluginFormat,
        path: &std::path::Path,
        id: &str,
    ) -> Result<()> {
        refuse_if_quarantined(format, path, id)?;
        match format {
            crate::PluginFormat::Clap => self.load_clap(slot, path, id),
            crate::PluginFormat::Lv2
            | crate::PluginFormat::Dssi
            | crate::PluginFormat::Vst2
            | crate::PluginFormat::Vst3
            | crate::PluginFormat::Sfz => {
                let inst = self.build_instrument(format, path, id)?;
                self.set_slot_source(slot, inst);
                Ok(())
            }
            _ => anyhow::bail!("{} hosting is not implemented yet", format.label()),
        }
    }

    /// Load a DSSI synth with its `configure` settings applied **before** it
    /// reaches the audio thread.
    ///
    /// `configure` is how DSSI carries everything that is not a parameter — the
    /// SoundFont FluidSynth-DSSI needs to make any sound at all, the patch file
    /// hexter and WhySynth read. It is not RT-safe (it opens files), and once a
    /// source is in a slot the audio thread owns it, so the only safe moment is
    /// this one: build, configure, hand over.
    ///
    /// With an empty `config` this is exactly [`Self::load_plugin`], sandbox and
    /// all. With settings to send it builds in-process, because the sandbox
    /// bridge has no message for `configure` yet.
    pub fn load_dssi(
        &mut self,
        slot: usize,
        path: &std::path::Path,
        id: &str,
        config: &[(String, String)],
    ) -> Result<()> {
        if config.is_empty() {
            return self.load_plugin(slot, crate::PluginFormat::Dssi, path, id);
        }
        refuse_if_quarantined(crate::PluginFormat::Dssi, path, id)?;
        let mut inst =
            choz_plugin_ladspa::DssiInstrument::build(path, id, self.sample_rate, self.buffer_size)
                .ok_or_else(|| anyhow::anyhow!("DSSI {id} would not load"))?;
        for (key, value) in config {
            if let Some(complaint) = inst.configure(key, value) {
                // The plugin refusing one key is not a reason to lose the
                // instrument: it says so and the rest still applies.
                eprintln!("choz: DSSI {id} rejected {key}={value}: {complaint}");
            }
        }
        self.set_slot_source(slot, Box::new(inst));
        Ok(())
    }

    /// Instantiate a plugin instrument off the RT thread, at this engine's
    /// sample rate and block size.
    fn build_instrument(
        &self,
        format: crate::PluginFormat,
        path: &std::path::Path,
        id: &str,
    ) -> Result<Box<dyn crate::sources::AudioSource>> {
        build_hosted_instrument(format, path, id, self.sample_rate, self.buffer_size)
    }

    /// Send a note-on to one slot. Input→slot routing lives in the UI.
    pub fn note_on(&mut self, slot: usize, note: u8, vel: u8) {
        self.send(EngineCommand::NoteOn {
            slot,
            note,
            vel,
            at: 0,
        });
    }

    pub fn note_off(&mut self, slot: usize, note: u8) {
        self.send(EngineCommand::NoteOff { slot, note, at: 0 });
    }

    /// A note the sender knows the time of: `at` is an absolute transport
    /// sample. The callback holds it until the block that contains it and
    /// splits that slot's render there, so the note starts on the sample it
    /// was written for and not at the top of whichever block noticed.
    ///
    /// This is what a generator with its own clock uses — the arpeggiator —
    /// and it is the only way its resolution stops being the interface's wake
    /// interval.
    pub fn note_on_at(&mut self, slot: usize, note: u8, vel: u8, at: u64) {
        self.send(EngineCommand::NoteOn {
            slot,
            note,
            vel,
            at,
        });
    }

    pub fn note_off_at(&mut self, slot: usize, note: u8, at: u64) {
        self.send(EngineCommand::NoteOff { slot, note, at });
    }

    /// Stop every note in every slot.
    ///
    /// One command for the whole rack: sending 128 note-offs per slot could
    /// overrun the command ring and leave the last slot ringing, which is the
    /// exact failure this button exists to fix.
    pub fn panic(&mut self) {
        self.send(EngineCommand::Panic);
    }

    /// Send a control change to one slot: sustain and the other pedals, the
    /// modulation wheel, expression. Not filtered — the instrument decides what
    /// it understands.
    pub fn control_change(&mut self, slot: usize, cc: u8, value: u8) {
        self.send(EngineCommand::ControlChange { slot, cc, value });
    }

    /// Send pitch bend to one slot, as the raw 14-bit wire value.
    pub fn pitch_bend(&mut self, slot: usize, value: u16) {
        self.send(EngineCommand::PitchBend { slot, value });
    }

    pub fn set_playing(&self, play: bool) {
        self.playing.store(play, Ordering::Relaxed);
    }
}

/// Real-time callback body. Runs on the audio thread — no locks, no alloc.
/// The cpal path: mix into the two channel buffers and interleave them into
/// the device buffer. Every backend shares [`RtState::apply_commands`] and
/// [`RtState::render`]; only the hand-off to the device differs.
fn audio_callback(buf: &mut [f32], state: &mut RtState) {
    let started = std::time::Instant::now();
    state.apply_commands();
    let frames = buf.len() / 2;
    state.drain_capture(frames);
    state.render(frames);
    for f in 0..frames {
        buf[f * 2] = state.mix[0][f];
        buf[f * 2 + 1] = state.mix[1][f];
    }
    publish_load(started, frames, state.sample_rate);
}

/// What this block cost against what it had. One clock read per block, and the
/// only place either backend measures it — see [`crate::meter::Load`].
pub(crate) fn publish_load(started: std::time::Instant, frames: usize, sample_rate: u32) {
    let budget = std::time::Duration::from_secs_f64(frames as f64 / sample_rate.max(1) as f64);
    crate::meter::load().publish(started.elapsed(), budget);
}

impl RtState {
    /// Apply pending commands to the slot list. Retired slots/chains go back
    /// over `retired_tx` so they are freed on the UI thread, not here.
    pub(crate) fn apply_commands(&mut self) {
        let state = self;
        while let Ok(cmd) = state.cmd_rx.pop() {
            match cmd {
                EngineCommand::AddSlot(source) => {
                    if state.slots.len() < MAX_SLOTS {
                        state.slots.push(Slot::new(source));
                    } else {
                        let _ = state.retired_tx.push(Retired::Slot(Slot::new(source)));
                    }
                }
                EngineCommand::RemoveSlot(i) => {
                    if i < state.slots.len() {
                        let old = state.slots.remove(i);
                        let _ = state.retired_tx.push(Retired::Slot(old));
                    }
                }
                EngineCommand::SetSlotSource { slot, source } => {
                    // A new instrument holds nothing, whatever the old one was
                    // playing when it was swapped out.
                    if let Some(s) = state.slots.get_mut(slot) {
                        s.held = 0;
                    }
                    if let Some(s) = state.slots.get_mut(slot) {
                        let old = std::mem::replace(&mut s.source, source);
                        let _ = state.retired_tx.push(Retired::Source(old));
                    } else {
                        let _ = state.retired_tx.push(Retired::Source(source));
                    }
                }
                EngineCommand::SetSlotFx { slot, fx } => {
                    if let Some(s) = state.slots.get_mut(slot) {
                        let old = std::mem::replace(&mut s.fx, fx);
                        let _ = state.retired_tx.push(Retired::Fx(old));
                    } else {
                        let _ = state.retired_tx.push(Retired::Fx(fx));
                    }
                }
                EngineCommand::SetSlotMix {
                    slot,
                    gain,
                    gain_r,
                    pan,
                    mute,
                } => {
                    if let Some(s) = state.slots.get_mut(slot) {
                        s.gain = gain;
                        s.gain_r = gain_r;
                        s.pan = pan;
                        s.mute = mute;
                    }
                }
                EngineCommand::SetSlotDest { slot, dest } => {
                    if let Some(s) = state.slots.get_mut(slot) {
                        s.dest = dest;
                    }
                }
                EngineCommand::SetBus {
                    bus,
                    gain,
                    mute,
                    left,
                    right,
                } => {
                    if let Some(b) = state.buses.get_mut(bus) {
                        b.gain = gain.clamp(0.0, 2.0);
                        b.mute = mute;
                        b.out_pair = (left, right);
                    }
                }
                EngineCommand::SetMain { gain, mute } => {
                    state.main_gain = gain.clamp(0.0, 2.0);
                    state.main_mute = mute;
                }
                EngineCommand::SetSlotOut { slot, left, right } => {
                    if let Some(s) = state.slots.get_mut(slot) {
                        s.out_pair = (left, right);
                    }
                }
                EngineCommand::SetSlotPitchToMidi { slot, on } => {
                    let sr = state.sample_rate;
                    if let Some(s) = state.slots.get_mut(slot) {
                        match (on, s.pitch.take()) {
                            (true, None) => s.pitch = Some(crate::pitch::PitchTracker::new(sr)),
                            (true, some) => s.pitch = some,
                            // Switching it off must not leave the note hanging.
                            (false, Some(mut t)) => {
                                if let Some(crate::pitch::PitchEvent::Off { note }) = t.release() {
                                    s.source.note_off(note);
                                    s.held &= !(1u128 << (note & 0x7F));
                                }
                            }
                            (false, None) => {}
                        }
                    }
                }
                EngineCommand::SetSlotPitchMix { slot, mix } => {
                    if let Some(s) = state.slots.get_mut(slot) {
                        s.pitch_mix = mix.clamp(0.0, 1.0);
                    }
                }
                EngineCommand::SetSlotInTrim { slot, gain, gate } => {
                    if let Some(s) = state.slots.get_mut(slot) {
                        s.in_gain = gain.clamp(0.0, 8.0);
                        if let Some(t) = s.pitch.as_mut() {
                            t.gate = gate.clamp(0.0, 1.0);
                        }
                    }
                }
                EngineCommand::SetSlotIn { slot, pair } => {
                    if let Some(s) = state.slots.get_mut(slot) {
                        s.in_pair = pair;
                    }
                }
                EngineCommand::SetFxParam {
                    slot,
                    fx,
                    index,
                    value,
                } => {
                    if let Some(p) = state.slots.get_mut(slot).and_then(|s| s.fx.get_mut(fx)) {
                        if index == crate::FX_MIX_PARAM {
                            p.set_mix(value);
                        } else {
                            p.set_param(index, value);
                        }
                    }
                }
                EngineCommand::SetSlotParam { slot, index, value } => {
                    if let Some(s) = state.slots.get_mut(slot) {
                        s.source.set_param(index, value);
                    }
                }
                EngineCommand::SetSlotProgram { slot, bank, preset } => {
                    if let Some(s) = state.slots.get_mut(slot) {
                        s.source.program_change(bank, preset);
                    }
                }
                // Notes are addressed to one slot; the UI already decided which.
                //
                // `at == 0` is "now" — a key, a port, OSC, anything with no
                // schedule of its own — and goes straight through. Anything
                // else waits for the block that contains its sample.
                EngineCommand::NoteOn {
                    slot,
                    note,
                    vel,
                    at,
                } => {
                    if let Some(s) = state.slots.get_mut(slot) {
                        s.schedule(at, note, vel, true);
                    }
                }
                EngineCommand::NoteOff { slot, note, at } => {
                    if let Some(s) = state.slots.get_mut(slot) {
                        s.schedule(at, note, 0, false);
                    }
                }
                EngineCommand::Panic => {
                    for s in state.slots.iter_mut() {
                        // The notes choz knows about get a real note-off each —
                        // that is what a plugin cannot ignore. Then the broadcast,
                        // for anything it started on its own (an arpeggiator, a
                        // note that arrived before choz was listening).
                        let mut held = s.held;
                        while held != 0 {
                            let note = held.trailing_zeros() as u8;
                            held &= held - 1;
                            s.source.note_off(note);
                        }
                        s.held = 0;
                        s.source.all_notes_off();
                    }
                }
                EngineCommand::ControlChange { slot, cc, value } => {
                    if let Some(s) = state.slots.get_mut(slot) {
                        s.source.control_change(cc, value);
                    }
                }
                EngineCommand::PitchBend { slot, value } => {
                    if let Some(s) = state.slots.get_mut(slot) {
                        s.source.pitch_bend(value);
                    }
                }
            }
        }
    }

    /// Pull one block of live input off the ring the input stream fills.
    ///
    /// Two audio clocks that are not the same clock: the input callback and
    /// the output callback drift apart, so this has to answer for both ends of
    /// that. **Short** (the input has not produced yet) fills the rest of the
    /// block with silence rather than repeating stale audio. **Long** (the
    /// input is running ahead) throws away everything beyond a small backlog,
    /// because a ring that is allowed to fill is latency that grows all night
    /// and never comes back.
    // ponytail: drop/zero, no resampling. An adaptive resampler the day the
    // drift is audible as pitch rather than as the occasional lost block.
    pub(crate) fn drain_capture(&mut self, frames: usize) {
        // Destructured so the ring can be drained while the buffers are
        // written: two fields of one struct, borrowed apart.
        let Self {
            capture,
            capture_rx,
            ..
        } = self;
        let channels = capture.len();
        let Some(rx) = capture_rx.as_mut() else {
            return;
        };
        if channels == 0 || frames == 0 {
            return;
        }
        let want = frames * channels;
        // Two blocks of slack: enough that a late input callback does not
        // starve us, little enough that the delay stays in milliseconds.
        let backlog = want * 2;
        let available = rx.slots();
        if available > want + backlog {
            let excess = available - want - backlog;
            for _ in 0..excess {
                if rx.pop().is_err() {
                    break;
                }
            }
            crate::meter::capture_health().dropped_samples(excess);
        }
        // Counted once per block, not once per missing sample: what matters is
        // "this block had a hole in it", and a block is what the ear hears.
        if available < want {
            crate::meter::capture_health().late_block();
        }
        for f in 0..frames {
            for ch in capture.iter_mut() {
                let s = rx.pop().unwrap_or(0.0);
                if let Some(dst) = ch.get_mut(f) {
                    *dst = s;
                }
            }
        }
    }

    /// Copy one capture port's block into the matching input buffer.
    pub(crate) fn write_capture(&mut self, channel: usize, src: &[f32]) {
        let Some(dst) = self.capture.get_mut(channel) else {
            return;
        };
        let n = src.len().min(dst.len());
        dst[..n].copy_from_slice(&src[..n]);
    }

    /// Render `frames` of every slot into [`Self::mix`], one buffer per device
    /// output channel. Each slot lands on the pair it is routed to, so two
    /// slots can play out of different jacks of the same interface.
    pub(crate) fn render(&mut self, frames: usize) {
        // Destructured so the capture buffers can be read while the slots are
        // borrowed mutably.
        let Self {
            slots,
            scratch,
            dry,
            mix,
            bus_mix,
            buses,
            main_gain,
            main_mute,
            capture,
            playing,
            sample_rate,
            ..
        } = self;
        let last = mix.len().saturating_sub(1);
        for ch in mix.iter_mut().chain(bus_mix.iter_mut()) {
            let n = frames.min(ch.len());
            ch[..n].fill(0.0);
        }
        let playing = playing.load(Ordering::Relaxed);
        // Where this block sits on the transport's timeline, taken **before**
        // it is advanced: a scheduled note is a position on that line.
        let block_start = choz_ports::transport().samples();
        let block_end = block_start + frames as u64;
        // The host clock every plugin that syncs anything reads: it has to move
        // with the audio, so it is advanced here and nowhere else.
        let transport = choz_ports::transport();
        transport.set_playing(playing);
        // Only while rolling: a stopped transport keeps its position, it does
        // not creep. A tempo-synced delay reading a position that moves with
        // the stop button pressed is worse than one that reads nothing.
        if playing {
            transport.advance(frames);
        }
        let n = (frames * 2).min(scratch.len());
        let sr = *sample_rate;

        // What is arriving on each jack, before any slot decides what to do
        // with it. This is the reading that says whether live audio is even
        // reaching choz — the difference between a wiring problem and an
        // effect problem, which look the same from a panel.
        crate::meter::capture_levels().publish(capture, frames);

        for (slot_index, slot) in slots.iter_mut().enumerate() {
            let slot_started = std::time::Instant::now();
            // Synths always render (envelope tails / live keys); generators
            // (tone, WAV) honor the transport play flag.
            if !playing && !slot.source.plays_on_transport_stop() {
                continue;
            }
            let sc = &mut scratch[..n];
            sc.fill(0.0);
            match slot.in_pair {
                // Live audio in: interleave the two capture channels the slot
                // listens to, then treat it exactly like a rendered source.
                Some((l, r)) => {
                    let last_in = capture.len().saturating_sub(1);
                    if !capture.is_empty() {
                        let (l, r) = (l.min(last_in), r.min(last_in));
                        let g = slot.in_gain;
                        // A trim that reaches +24 dB reaches past full scale,
                        // and a signal driven past full scale is a square wave:
                        // it saturates what you hear **and** it hands the pitch
                        // detector a waveform whose period is no longer the one
                        // that was played. Soft, not hard — a `tanh` ceiling
                        // degrades into compression instead of into a corner —
                        // and counted, because a trim that is quietly limiting
                        // is a trim that is set wrong.
                        // **The trim belongs to the detector, not to the mix.**
                        // With `A→M` on, the input is being *listened to*: the
                        // trim is how loud the tracker hears it, and a guitar
                        // needs a lot of it. Passing that same gain on to what
                        // the player hears is why a tab that tracked well
                        // sounded saturated — so the untouched input is kept
                        // aside here and it is that one the `MIX` knob brings
                        // back. The tab's own `VOL` is the level control; this
                        // never was one.
                        slot.guard.set_sample_rate(sr as f32);
                        let listening = slot.pitch.is_some();
                        let keep_dry = listening && slot.pitch_mix < 0.999;
                        let mut clipped = false;
                        for f in 0..(n / 2) {
                            // The guard watches the **left** channel's raw
                            // sample and its answer is applied to both: a howl
                            // is a property of the room, not of one side of a
                            // stereo pair, and two independent ducks on one
                            // signal would move the image while they worked.
                            let guard = slot.guard.step(capture[l][f]);
                            for (ch, src) in [l, r].into_iter().enumerate() {
                                let raw = capture[src][f] * guard;
                                if keep_dry {
                                    dry[f * 2 + ch] = raw;
                                }
                                let x = raw * g;
                                sc[f * 2 + ch] = if x.abs() > 1.0 {
                                    clipped = true;
                                    x.tanh()
                                } else {
                                    x
                                };
                            }
                        }
                        if clipped {
                            crate::meter::capture_health().clipped_block();
                        }
                        // What the guard is holding down, for the panel: a duck
                        // nobody can see is indistinguishable from the room
                        // having gone quiet on its own.
                        crate::meter::capture_health().guard(slot.guard.reduction_db());
                    }
                    // With A→M on, the input is listened to rather than passed
                    // through: what comes out of the slot is its instrument
                    // playing the notes just heard, so a guitar drives a synth.
                    if let Some(tracker) = slot.pitch.as_mut() {
                        let (events, count) = tracker.process(sc, sr);
                        for event in events.iter().take(count).flatten() {
                            match *event {
                                crate::pitch::PitchEvent::On { note, velocity } => {
                                    slot.source.note_on(note, velocity);
                                    slot.held |= 1u128 << (note & 0x7F);
                                }
                                crate::pitch::PitchEvent::Off { note } => {
                                    slot.source.note_off(note);
                                    slot.held &= !(1u128 << (note & 0x7F));
                                }
                            }
                        }
                        // What it heard, for the rack to draw: a tracker that
                        // plays nothing and one that plays the wrong thing look
                        // identical without this, and `SENS` has nothing to aim
                        // at.
                        crate::meter::pitch_meter().publish(
                            tracker.sounding(),
                            tracker.cents(),
                            tracker.level(),
                        );
                        // How much of the input comes back. `dry` already
                        // holds it, untrimmed, from the loop above.
                        let wet = slot.pitch_mix;
                        let keep_dry = wet < 0.999;
                        // **Clear it before the instrument plays.** The buffer
                        // still holds the microphone, and `render` is not
                        // required to overwrite what it was handed — a hosted
                        // plugin with nothing to play may add to it, or leave
                        // it alone entirely. Either way the input would leak
                        // out alongside the synth, and how much of it comes
                        // back is the player's decision, not the plugin's.
                        sc.fill(0.0);
                        let written = slot.source.render(sc, sr);
                        for s in sc[written * 2..].iter_mut() {
                            *s = 0.0;
                        }
                        if keep_dry {
                            for (out, d) in sc.iter_mut().zip(dry[..n].iter()) {
                                *out = *out * wet + *d * (1.0 - wet);
                            }
                        }
                    }
                }
                None => {
                    // **Split at the notes.** A slot with scheduled notes is
                    // rendered in segments: apply what is due at a sample,
                    // render up to the next one, repeat. That is what makes a
                    // generator's timing the sample it asked for rather than
                    // the top of whichever block noticed — the interface's
                    // wake interval stops being the resolution.
                    //
                    // The cost is real and worth naming: a slot with notes in
                    // the middle of a block calls `render` more than once, in
                    // pieces. For choz's own sources that is nothing; for a
                    // hosted plugin it is several small process calls instead
                    // of one, which is legal, and which every host with
                    // sample-accurate automation already does.
                    let frames = n / 2;
                    // Everything due this block, oldest first. A fixed array,
                    // because this is the audio thread.
                    let mut due: [Option<(usize, Scheduled)>; MAX_SCHEDULED] =
                        [None; MAX_SCHEDULED];
                    let mut count = 0usize;
                    while count < MAX_SCHEDULED {
                        let Some(ev) = slot.due(block_start, block_end) else {
                            break;
                        };
                        // Anything already past lands on the first sample:
                        // late, not lost.
                        let offset = (ev.at.saturating_sub(block_start) as usize).min(frames);
                        due[count] = Some((offset, ev));
                        count += 1;
                    }
                    // Insertion sort over at most eight: the order they come
                    // off the queue is not the order they are played in.
                    for i in 1..count {
                        let mut j = i;
                        while j > 0 && due[j - 1].unwrap().0 > due[j].unwrap().0 {
                            due.swap(j - 1, j);
                            j -= 1;
                        }
                    }

                    let mut at = 0usize;
                    let mut next_event = 0usize;
                    while at < frames {
                        // Everything that lands on this sample, before the
                        // segment that starts here is rendered.
                        while next_event < count && due[next_event].unwrap().0 <= at {
                            let ev = due[next_event].unwrap().1;
                            slot.play(ev.note, ev.vel, ev.on);
                            next_event += 1;
                        }
                        let end = due
                            .get(next_event)
                            .and_then(|d| d.map(|(o, _)| o))
                            .unwrap_or(frames)
                            .clamp(at + 1, frames);
                        let seg = &mut sc[at * 2..end * 2];
                        let written = slot.source.render(seg, sr);
                        for s in seg[written * 2..].iter_mut() {
                            *s = 0.0;
                        }
                        at = end;
                    }
                }
            }
            for fx in slot.fx.iter_mut() {
                fx.process_block(sc, sr);
            }
            // How loud this tab is on its own, **before** the strip: the
            // number auto-trim solves against, and the only way the health log
            // can name which tab clipped the mix.
            crate::meter::slot_levels().publish(slot_index, sc);
            // Muted slots still render (so envelopes/playheads keep moving) but
            // sum in at zero gain. A pair pointing past the device's channels
            // folds onto the last one rather than going silent.
            let (gl, gr) = slot.channel_gains();
            // A tab routed to a subgroup never touches a device pair: the bus
            // owns where it lands, which is the point of having one.
            match slot.dest {
                Dest::Bus(b) if b < BUSES => {
                    for f in 0..(n / 2) {
                        bus_mix[b * 2][f] += sc[f * 2] * gl;
                        bus_mix[b * 2 + 1][f] += sc[f * 2 + 1] * gr;
                    }
                }
                _ => {
                    let (l, r) = (slot.out_pair.0.min(last), slot.out_pair.1.min(last));
                    for f in 0..(n / 2) {
                        mix[l][f] += sc[f * 2] * gl;
                        mix[r][f] += sc[f * 2 + 1] * gr;
                    }
                }
            }
            // What this tab cost, source and FX chain together. Two clock
            // reads a block per tab, and the only way the log can name which
            // one ran the callback out of time.
            crate::meter::load().publish_slot(slot_index, slot_started.elapsed());
        }

        // Each subgroup, through its own fader, onto the pair it points at. A
        // muted bus still had its tabs rendered — envelopes and playheads keep
        // moving — it simply does not arrive.
        for (b, bus) in buses.iter().enumerate() {
            let g = match bus.mute {
                true => 0.0,
                false => bus.gain,
            };
            let (l, r) = (bus.out_pair.0.min(last), bus.out_pair.1.min(last));
            let n = frames
                .min(bus_mix[b * 2].len())
                .min(bus_mix[b * 2 + 1].len());
            for f in 0..n {
                let (a, c) = (bus_mix[b * 2][f] * g, bus_mix[b * 2 + 1][f] * g);
                mix[l][f] += a;
                mix[r][f] += c;
            }
        }

        // The click, on top of everything and through no tab's FX: a metronome
        // that a reverb smears is a metronome you cannot play to. Where it
        // lands is the metronome's own setting — the point of a subgroup is
        // being able to send the click to the player's wedge and nowhere else.
        if mix.len() >= 2 {
            let n = frames.min(mix[0].len()).min(mix[1].len());
            let len = (n * 2).min(scratch.len());
            let sc = &mut scratch[..len];
            sc.fill(0.0);
            let click = crate::metronome::metronome();
            click.render(sc, n, sr);
            match click.dest() {
                Dest::Bus(b) if b < BUSES => {
                    // Straight past the bus fader: the click is a reference, and
                    // a reference that moves when somebody rides the group
                    // fader is not one. It borrows the bus's *routing*, not its
                    // level.
                    let bus = buses[b];
                    let (l, r) = (bus.out_pair.0.min(last), bus.out_pair.1.min(last));
                    for f in 0..sc.len() / 2 {
                        mix[l][f] += sc[f * 2];
                        mix[r][f] += sc[f * 2 + 1];
                    }
                }
                _ => {
                    for f in 0..sc.len() / 2 {
                        mix[0][f] += sc[f * 2];
                        mix[1][f] += sc[f * 2 + 1];
                    }
                }
            }
        }

        // The main strip, last: one fader on the pair everything calls the
        // output. The other pairs are separate outputs and are left alone —
        // a main that also trimmed channels 7 and 8 would be a master fader
        // that silences a monitor send.
        let main = match *main_mute {
            true => 0.0,
            false => *main_gain,
        };
        if (main - 1.0).abs() > f32::EPSILON && mix.len() >= 2 {
            let n = frames.min(mix[0].len()).min(mix[1].len());
            for ch in mix.iter_mut().take(2) {
                for s in ch[..n].iter_mut() {
                    *s *= main;
                }
            }
        }

        // What went out, for whoever draws it. The first pair is the one the
        // interface calls the output; a meter of every channel would be a
        // different panel.
        if mix.len() >= 2 {
            let frames = frames.min(mix[0].len()).min(mix[1].len());
            let scratch_len = scratch.len().min(frames * 2);
            for f in 0..scratch_len / 2 {
                scratch[f * 2] = mix[0][f];
                scratch[f * 2 + 1] = mix[1][f];
            }
            crate::meter::meter().publish(&scratch[..scratch_len]);
        }
    }
}

// ─── Stream / config helpers ───────────────────────────────────────────────

/// Open an input stream and return the ring its callback fills.
///
/// This is what makes choz a multi-effect on a box without JACK: cpal gives
/// playback and capture their own devices and their own callbacks, so the only
/// thing that can pass between them is a lock-free ring.
///
/// The rate is **not** negotiated: it is the one the engine already runs at. A
/// capture stream at another rate would be transposed, and a microphone that
/// comes out a tone higher is worse than one that says why it will not open.
fn build_input_stream(
    host: &cpal::Host,
    name: Option<&str>,
    sample_rate: u32,
    buffer: u32,
) -> Result<(cpal::Stream, rtrb::Consumer<f32>, usize, String)> {
    let device = match name {
        Some(want) => host
            .input_devices()
            .context("no audio input devices")?
            .find(|d| d.name().is_ok_and(|n| n == want))
            .with_context(|| format!("input device '{want}' is gone"))?,
        None => host
            .default_input_device()
            .context("this host has no default input device")?,
    };
    let picked = device.name().unwrap_or_else(|_| "input".to_string());
    let default = device
        .default_input_config()
        .context("the input device reports no usable format")?;
    let channels = (default.channels() as usize).clamp(1, crate::jack_backend::MAX_PORTS);
    let config = cpal::StreamConfig {
        channels: channels as u16,
        sample_rate: cpal::SampleRate(sample_rate),
        buffer_size: cpal::BufferSize::Fixed(buffer.max(64)),
    };
    // Eight blocks: room for the input to run ahead of the output without the
    // ring ever being the thing that drops audio. `drain_capture` is what keeps
    // the backlog from growing into latency.
    let capacity = (buffer.max(1024) as usize) * channels * 8;
    let (mut tx, rx) = rtrb::RingBuffer::<f32>::new(capacity);
    let stream = device
        .build_input_stream(
            &config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                // A full ring means the output side stopped taking; dropping
                // here is right, and it must never block an audio callback.
                for s in data {
                    if tx.push(*s).is_err() {
                        break;
                    }
                }
            },
            |err| eprintln!("choz: audio input error: {err}"),
            None,
        )
        .with_context(|| {
            format!("cannot open '{picked}' at {sample_rate} Hz; try that rate in Settings → AUDIO")
        })?;
    stream.play().context("cannot start the input stream")?;
    Ok((stream, rx, channels, picked))
}

fn build_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut state: RtState,
) -> Result<cpal::Stream> {
    device
        .build_output_stream(
            config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                audio_callback(data, &mut state);
            },
            |err| eprintln!("Audio error: {err}"),
            None,
        )
        .context("failed to build output stream")
}

/// JACK port type of a mono float audio port (`JACK_DEFAULT_AUDIO_TYPE`).
pub(crate) const JACK_AUDIO: &str = "32 bit float mono audio";

/// The JACK client cpal registers our output stream under: cpal names its
/// JACK host `cpal_client` and appends `_out` for the playback device.
///
/// ponytail: hard-coded rather than plumbed out of cpal, which never exposes
/// it. Only wrong if a second choz is already connected, in which case JACK
/// renames the client and the routing below silently finds no ports.
pub(crate) const CPAL_JACK_CLIENT: &str = "cpal_client_out";

/// Open a short-lived JACK client just to inspect/patch the graph.
fn jack_probe(name: &str) -> Result<jack::Client> {
    let (client, _) = jack::Client::new(name, jack::ClientOptions::NO_START_SERVER)
        .context("cannot reach the JACK graph")?;
    Ok(client)
}

/// Audio sinks the JACK graph exposes, by client name — under PipeWire these
/// are the real outputs (speakers, HDMI, USB headset), the same list Carla
/// shows, rather than cpal's single synthetic `cpal_client_out` device.
fn jack_sinks() -> Vec<String> {
    let Ok(client) = jack_probe("choz-probe") else {
        return Vec::new();
    };
    let mut sinks: Vec<String> = Vec::new();
    for port in client.ports(None, Some(JACK_AUDIO), jack::PortFlags::IS_INPUT) {
        let Some((owner, _)) = port.rsplit_once(':') else {
            continue;
        };
        let ours = owner == CPAL_JACK_CLIENT || owner == crate::jack_backend::CLIENT_NAME;
        if !ours && !sinks.iter().any(|s| s == owner) {
            sinks.push(owner.to_string());
        }
    }
    sinks
}

/// The sink our first playback port is wired to, if any — after start-up this
/// is whatever PipeWire auto-connected us to.
fn jack_current_sink() -> Option<String> {
    let client = jack_probe("choz-probe").ok()?;
    let out = client
        .ports(None, Some(JACK_AUDIO), jack::PortFlags::IS_OUTPUT)
        .into_iter()
        .find(|p| {
            p.starts_with(&format!("{}:", crate::jack_backend::CLIENT_NAME))
                || p.starts_with(&format!("{CPAL_JACK_CLIENT}:"))
        })?;
    let connected = client.port_by_name(&out)?.get_connections();
    let (owner, _) = connected.first()?.rsplit_once(':')?;
    Some(owner.to_string())
}

/// Point our playback ports at `sink`'s inputs, dropping whatever they were
/// wired to before (PipeWire auto-connects new clients to the default sink).
fn jack_route_to(sink: &str, client_name: &str) -> Result<()> {
    let client = jack_probe("choz-router")?;
    let ours = crate::jack_backend::in_order(
        client
            .ports(None, Some(JACK_AUDIO), jack::PortFlags::IS_OUTPUT)
            .into_iter()
            .filter(|p| p.starts_with(&format!("{client_name}:")))
            .collect(),
    );
    if ours.is_empty() {
        anyhow::bail!("choz has no JACK output ports (is the stream running?)");
    }
    let targets = crate::jack_backend::sink_ports(&client, sink);
    if targets.is_empty() {
        anyhow::bail!("output '{sink}' has no playback ports");
    }

    for (out, target) in pair_ports(&ours, &targets) {
        // Tear the old wiring down first: a port left connected to two sinks
        // plays through both.
        if let Some(port) = client.port_by_name(out) {
            for old in port.get_connections() {
                let _ = client.disconnect_ports_by_name(out, &old);
            }
        }
        client
            .connect_ports_by_name(out, target)
            .with_context(|| format!("cannot connect {out} to {target}"))?;
    }
    Ok(())
}

/// Wire our playback ports to a sink's inputs, in order. A sink with fewer
/// inputs than we have ports (a mono output fed by our stereo pair) folds the
/// extras onto its last input rather than dropping them.
fn pair_ports<'a>(ours: &'a [String], targets: &'a [String]) -> Vec<(&'a str, &'a str)> {
    ours.iter()
        .enumerate()
        .filter_map(|(i, out)| {
            let t = targets.get(i).or_else(|| targets.last())?;
            Some((out.as_str(), t.as_str()))
        })
        .collect()
}

/// The named output device, or the host default when `want` is `None` or the
/// name isn't there any more (device disappeared between listing and opening).
fn named_or_default(host: &cpal::Host, want: Option<&str>) -> Option<cpal::Device> {
    if let Some(name) = want {
        let found = host
            .output_devices()
            .ok()?
            .find(|d| d.name().is_ok_and(|n| n == name));
        if let Some(dev) = found {
            return Some(dev);
        }
        eprintln!("choz: output device '{name}' not found, using the default");
    }
    host.default_output_device()
}

/// Pick the best supported stream config.
fn pick_config(
    device: &cpal::Device,
    target_rate: u32,
    target_buffer: u32,
) -> Result<cpal::StreamConfig> {
    let target_rate = cpal::SampleRate(target_rate);

    let supported = device
        .supported_output_configs()
        .context("cannot query supported configs")?;

    let mut best: Option<cpal::SupportedStreamConfig> = None;

    for cfg in supported {
        let range = cfg.min_sample_rate()..=cfg.max_sample_rate();
        if range.contains(&target_rate) {
            best = Some(cfg.with_sample_rate(target_rate));
            break;
        }
        if best.is_none() {
            best = Some(cfg.with_max_sample_rate());
        }
    }

    let supported = best.context("no compatible sample rate found")?;

    let buffer_size = match supported.buffer_size() {
        cpal::SupportedBufferSize::Range { min, max } => {
            if target_buffer >= *min && target_buffer <= *max {
                cpal::BufferSize::Fixed(target_buffer)
            } else {
                cpal::BufferSize::Default
            }
        }
        cpal::SupportedBufferSize::Unknown => cpal::BufferSize::Default,
    };

    Ok(cpal::StreamConfig {
        channels: 2,
        sample_rate: supported.sample_rate(),
        buffer_size,
    })
}

/// Multi-line install hint shown when no backend could be opened.
fn no_backend_hint(sound_server: &str) -> String {
    format!(
        "No audio backend available.\n\n\
         Sound server detected: {sound_server}\n\n\
         Installation hints:\n\
         \x20 Ubuntu/Debian:  sudo apt install libasound2-dev libjack-dev\n\
         \x20 Fedora:         sudo dnf install alsa-lib-devel jack-audio-connection-kit-devel\n\
         \x20 Arch:           sudo pacman -S alsa-lib jack2\n\
         \nPipeWire users — install compatibility layers:\n\
         \x20 Ubuntu/Debian:  sudo apt install pipewire-alsa pipewire-jack\n\
         \x20 Fedora:         sudo dnf install pipewire-alsa pipewire-jack-audio-connection-kit\n\
         \x20 Arch:           sudo pacman -S pipewire-alsa pipewire-jack"
    )
}

/// Refuse a plugin that has already been seen to die on its way in. The check
/// probes it in a child process the first time and remembers the answer, so the
/// cost is one extra process the first time each plugin is used.
fn refuse_if_quarantined(
    format: crate::PluginFormat,
    path: &std::path::Path,
    id: &str,
) -> Result<()> {
    let verdict = crate::quarantine::check(format, path, id).verdict;
    // It plays, it just can't be destroyed: tell the host crate to leak it
    // rather than take the app down when the tab is removed.
    if verdict == crate::quarantine::Verdict::CrashesOnTeardown
        && format == crate::PluginFormat::Lv2
    {
        choz_plugin_lv2::leak_on_teardown(id);
    }
    if verdict.loadable() {
        return Ok(());
    }
    anyhow::bail!(
        "{} crashes when it is loaded; choz will not host it (see {})",
        path.display(),
        crate::cache::state_dir()
            .join("plugin-verdicts.json")
            .display()
    )
}

/// Instantiate an instrument for a rack slot, in its own process when the load
/// probe said this plugin cannot be destroyed safely.
///
/// A sandboxed plugin dies with its child, so there is nothing to leak and
/// nothing to take the app down with it. If the sandbox itself won't start,
/// choz falls back to hosting in-process — which is what it did before any of
/// this existed, leak and all.
pub fn build_hosted_instrument(
    format: crate::PluginFormat,
    path: &std::path::Path,
    id: &str,
    sr: u32,
    block: u32,
) -> Result<Box<dyn crate::sources::AudioSource>> {
    if crate::quarantine::wants_sandbox(format, path, id) {
        match crate::sandboxed::SandboxedPlugin::build(format, path, id, sr, block) {
            Ok(p) => {
                eprintln!("choz: hosting {} in its own process", path.display());
                return Ok(Box::new(p));
            }
            Err(e) => eprintln!(
                "choz: sandbox for {} failed ({e}); hosting in-process",
                path.display()
            ),
        }
    }
    build_instrument(format, path, id, sr, block)
}

/// Instantiate a plugin instrument. `path` is the plugin file, or the bundle
/// directory for LV2; `id` its URI/label. Non-RT: this loads a shared object.
/// Always in this process — the sandbox child calls exactly this.
pub fn build_instrument(
    format: crate::PluginFormat,
    path: &std::path::Path,
    id: &str,
    sr: u32,
    block: u32,
) -> Result<Box<dyn crate::sources::AudioSource>> {
    let source: Option<Box<dyn crate::sources::AudioSource>> = match format {
        crate::PluginFormat::Lv2 => choz_plugin_lv2::Lv2Instrument::build(path, id, sr, block)
            .map(|i| Box::new(i) as Box<dyn crate::sources::AudioSource>),
        crate::PluginFormat::Dssi => choz_plugin_ladspa::DssiInstrument::build(path, id, sr, block)
            .map(|i| Box::new(i) as Box<dyn crate::sources::AudioSource>),
        crate::PluginFormat::Vst2 => choz_plugin_vst2::Vst2Instrument::build(path, sr, block)
            .map(|i| Box::new(i) as Box<dyn crate::sources::AudioSource>),
        crate::PluginFormat::Vst3 => choz_plugin_vst3::Vst3Instrument::build(path, sr, block)
            .map(|i| Box::new(i) as Box<dyn crate::sources::AudioSource>),
        // Not a plugin at all: a text file pointing at samples, played by
        // choz's own sampler.
        crate::PluginFormat::Sfz => match crate::sfz::SfzSampler::build(path, sr) {
            Ok(s) => Some(Box::new(s) as Box<dyn crate::sources::AudioSource>),
            Err(e) => {
                eprintln!("choz: SFZ {}: {e}", path.display());
                None
            }
        },
        _ => None,
    };
    source.ok_or_else(|| anyhow::anyhow!("failed to instantiate {} plugin: {id}", format.label()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A graph that says what rate it runs at is believed over the settings —
    /// that mismatch is what detunes every synth in the rack. A graph that says
    /// nothing (unreachable, or a driver that hasn't started) leaves the saved
    /// rate alone.
    #[test]
    fn the_graph_rate_beats_the_saved_one() {
        assert_eq!(adopt_rate(48_000, Some(44_100)), 44_100);
        assert_eq!(adopt_rate(48_000, Some(48_000)), 48_000);
        assert_eq!(adopt_rate(48_000, None), 48_000, "no graph, no opinion");
        assert_eq!(adopt_rate(48_000, Some(0)), 48_000, "0 Hz is not an answer");
    }

    /// The configured buffer size has to reach PipeWire as a *force*, or
    /// pipewire-jack's inherited `node.force-quantum` wins and the node runs at
    /// whatever the graph was doing (1024 = 21 ms, unplayable live).
    #[test]
    fn forces_the_quantum_only_when_usb_can_take_it() {
        request_pipewire_period(256, 48000, 256);
        assert_eq!(
            std::env::var("PIPEWIRE_LATENCY").as_deref(),
            Ok("256/48000")
        );
        assert_eq!(
            std::env::var("PIPEWIRE_QUANTUM").as_deref(),
            Ok("256/48000")
        );

        // Below the floor choz still asks, but never forces — a 64-frame quantum
        // is what took the xHCI controller down.
        request_pipewire_period(64, 48000, 64);
        assert_eq!(std::env::var("PIPEWIRE_LATENCY").as_deref(), Ok("64/48000"));
        assert!(
            std::env::var("PIPEWIRE_QUANTUM").is_err(),
            "64 frames must not be forced"
        );

        // And with the setting at its default choz **asks and does not take**:
        // forcing moves the whole graph, so every other application on the
        // machine is resampled to whatever choz wanted. That is not choz's to
        // decide, and doing it anyway was heard as a browser going thin and
        // distorted while choz was running.
        request_pipewire_period(256, 48000, 0);
        assert_eq!(
            std::env::var("PIPEWIRE_LATENCY").as_deref(),
            Ok("256/48000"),
            "it still asks"
        );
        assert!(
            std::env::var("PIPEWIRE_QUANTUM").is_err(),
            "and leaves the graph alone"
        );

        unsafe { std::env::remove_var("PIPEWIRE_LATENCY") };
    }

    /// Panic has to send a real note-off for everything the slot was told to
    /// play. The broadcast alone is not enough: `all notes off` is a MIDI CC,
    /// and a VST3 plugin never sees CCs as events — only the note-offs reach it.
    #[test]
    fn panic_sends_a_note_off_for_every_held_note() {
        #[derive(Default)]
        struct Spy {
            offs: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
            broadcast: std::sync::Arc<std::sync::atomic::AtomicBool>,
        }
        impl choz_ports::AudioSource for Spy {
            fn render(&mut self, _out: &mut [f32], _sr: u32) -> usize {
                0
            }
            fn note_off(&mut self, note: u8) {
                self.offs.lock().unwrap().push(note);
            }
            fn all_notes_off(&mut self) {
                self.broadcast
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }

        let offs = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let broadcast = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut slot = Slot::new(Box::new(Spy {
            offs: std::sync::Arc::clone(&offs),
            broadcast: std::sync::Arc::clone(&broadcast),
        }));

        // Two notes down, one already released.
        for note in [60u8, 64, 67] {
            slot.held |= 1u128 << note;
        }
        slot.held &= !(1u128 << 64);

        let mut held = slot.held;
        while held != 0 {
            let note = held.trailing_zeros() as u8;
            held &= held - 1;
            slot.source.note_off(note);
        }
        slot.held = 0;
        slot.source.all_notes_off();

        assert_eq!(
            *offs.lock().unwrap(),
            vec![60, 67],
            "exactly the notes still down"
        );
        assert!(broadcast.load(std::sync::atomic::Ordering::Relaxed));
        assert_eq!(slot.held, 0);
    }

    #[test]
    fn playback_ports_pair_up_and_mono_sinks_fold() {
        let ours = vec![
            "cpal_client_out:out_0".into(),
            "cpal_client_out:out_1".into(),
        ];
        let stereo = vec!["Speaker:playback_FL".into(), "Speaker:playback_FR".into()];
        assert_eq!(
            pair_ports(&ours, &stereo),
            vec![
                ("cpal_client_out:out_0", "Speaker:playback_FL"),
                ("cpal_client_out:out_1", "Speaker:playback_FR"),
            ]
        );

        let mono = vec!["Beeper:playback_MONO".into()];
        assert_eq!(
            pair_ports(&ours, &mono),
            vec![
                ("cpal_client_out:out_0", "Beeper:playback_MONO"),
                ("cpal_client_out:out_1", "Beeper:playback_MONO"),
            ],
            "both channels reach a mono sink instead of the right one vanishing"
        );

        assert!(
            pair_ports(&ours, &[]).is_empty(),
            "a sink with no inputs wires nothing"
        );
    }

    /// Adds a constant to every sample — enough to prove the chain ran.
    struct AddFx(f32);
    impl FxProcessor for AddFx {
        fn process_block(&mut self, buf: &mut [f32], _sr: u32) {
            for s in buf.iter_mut() {
                *s += self.0;
            }
        }
        fn reset(&mut self) {}
        fn set_mix(&mut self, _wet: f32) {}
    }

    /// A source that renders a constant DC level — makes mixing observable.
    struct DcSource(f32);
    impl AudioSource for DcSource {
        fn render(&mut self, out: &mut [f32], _sr: u32) -> usize {
            out.fill(self.0);
            out.len() / 2
        }
        fn plays_on_transport_stop(&self) -> bool {
            true
        }
    }

    /// Records note events it receives; proves omni routing reaches it.
    struct RecordingSynth(std::sync::Arc<parking_lot::Mutex<Vec<(bool, u8)>>>);
    impl AudioSource for RecordingSynth {
        fn render(&mut self, out: &mut [f32], _sr: u32) -> usize {
            out.fill(0.0);
            out.len() / 2
        }
        fn note_on(&mut self, note: u8, _vel: u8) {
            self.0.lock().push((true, note));
        }
        fn note_off(&mut self, note: u8) {
            self.0.lock().push((false, note));
        }
        /// Recorded as note 200+preset so it can't be confused with a note event.
        fn program_change(&mut self, _bank: u8, preset: u8) {
            self.0.lock().push((true, 200 + preset));
        }
        fn plays_on_transport_stop(&self) -> bool {
            true
        }
    }

    /// Records the pedal/wheel traffic a slot receives.
    #[derive(Default)]
    struct Expressive {
        ccs: std::sync::Arc<parking_lot::Mutex<Vec<(u8, u8)>>>,
        bends: std::sync::Arc<parking_lot::Mutex<Vec<u16>>>,
    }
    impl AudioSource for Expressive {
        fn render(&mut self, out: &mut [f32], _sr: u32) -> usize {
            out.fill(0.0);
            out.len() / 2
        }
        fn control_change(&mut self, cc: u8, value: u8) {
            self.ccs.lock().push((cc, value));
        }
        fn pitch_bend(&mut self, value: u16) {
            self.bends.lock().push(value);
        }
    }

    fn mk_state() -> (
        rtrb::Producer<EngineCommand>,
        rtrb::Consumer<Retired>,
        RtState,
    ) {
        mk_state_ch(2, 0)
    }

    /// RT state with `outs` output and `ins` input channels, as the backends
    /// build it.
    fn mk_state_ch(
        outs: usize,
        ins: usize,
    ) -> (
        rtrb::Producer<EngineCommand>,
        rtrb::Consumer<Retired>,
        RtState,
    ) {
        let (cmd_tx, cmd_rx) = rtrb::RingBuffer::new(32);
        let (retired_tx, retired_rx) = rtrb::RingBuffer::new(32);
        let state = RtState {
            playing: Arc::new(AtomicBool::new(true)),
            cmd_rx,
            retired_tx,
            slots: Vec::with_capacity(MAX_SLOTS),
            scratch: vec![0.0; 64],
            dry: vec![0.0; 64],
            mix: vec![vec![0.0; 32]; outs],
            bus_mix: vec![vec![0.0; 32]; BUSES * 2],
            buses: [Bus::default(); BUSES],
            main_gain: 1.0,
            main_mute: false,
            capture: vec![vec![0.0; 32]; ins],
            capture_rx: None,
            sample_rate: 48_000,
        };
        (cmd_tx, retired_rx, state)
    }

    /// The ring between the input callback and the output callback, both ways
    /// it can be wrong: not enough (the input has not produced yet) fills with
    /// silence, and too much (the input is running ahead) is thrown away so the
    /// backlog cannot turn into latency that grows all night.
    #[test]
    fn the_capture_ring_answers_for_both_kinds_of_drift() {
        let (_tx, _rx, mut state) = mk_state_ch(2, 2);
        let (mut tx, rx) = rtrb::RingBuffer::<f32>::new(4096);
        state.capture_rx = Some(rx);
        let health = crate::meter::capture_health();
        health.clear();

        // Exactly one block of a two-channel input: L = 1, R = 2.
        for _ in 0..8 {
            tx.push(1.0).unwrap();
            tx.push(2.0).unwrap();
        }
        state.drain_capture(8);
        assert!(state.capture[0][..8].iter().all(|s| *s == 1.0), "left");
        assert!(state.capture[1][..8].iter().all(|s| *s == 2.0), "right");

        // Nothing left: the next block is silence, not the last one repeated.
        state.drain_capture(8);
        assert!(
            state.capture[0][..8].iter().all(|s| *s == 0.0),
            "an empty ring is silence, got {:?}",
            &state.capture[0][..8]
        );

        // The input runs away: 100 blocks queued, and the drain must not be
        // reading hundred-block-old audio for the rest of the session.
        for i in 0..(8 * 2 * 100) {
            tx.push(i as f32).unwrap();
        }
        state.drain_capture(8);
        let backlog = state.capture_rx.as_ref().unwrap().slots();
        assert!(
            backlog <= 8 * 2 * 2,
            "the backlog should have been cut back, {backlog} samples left"
        );
        // And what came out is the *newest* audio, not the oldest.
        assert!(
            state.capture[0][0] > 1000.0,
            "expected recent samples, got {}",
            state.capture[0][0]
        );

        // Both kinds of trouble are counted, because "does it drift on my
        // machine" has to be answerable without playing it for an hour.
        let (late, dropped) = health.counts();
        assert_eq!(late, 1, "one block was short and one was counted");
        assert!(
            dropped > 1000,
            "the backlog it threw away is counted: {dropped}"
        );
        health.clear();
        assert_eq!(health.counts(), (0, 0));
    }

    /// choz as a multi-effect: a tab fed by a capture pair, with an effect on
    /// it, has to come out **processed**. This is the whole "plug a microphone
    /// in and put a reverb on it" case, and nothing covered it.
    #[test]
    fn an_effect_on_a_capture_fed_tab_processes_the_input() {
        // Shares the process-wide transport: `render` advances it.
        let _clock = crate::test_locks::transport();
        let (mut cmd_tx, _retired, mut state) = mk_state_ch(2, 2);
        cmd_tx
            .push(EngineCommand::AddSlot(Box::new(crate::sources::Silence)))
            .unwrap();
        cmd_tx
            .push(EngineCommand::SetSlotIn {
                slot: 0,
                pair: Some((0, 1)),
            })
            .unwrap();
        cmd_tx
            .push(EngineCommand::SetSlotFx {
                slot: 0,
                fx: vec![Box::new(AddFx(1.0))],
            })
            .unwrap();
        state.apply_commands();
        // A microphone, arriving.
        for ch in state.capture.iter_mut() {
            ch.fill(0.25);
        }
        state.render(8);

        let c = std::f32::consts::FRAC_1_SQRT_2;
        assert!(
            (state.mix[0][0] - 1.25 * c).abs() < 1e-5,
            "the effect should have processed the input, got {}",
            state.mix[0][0]
        );
    }

    /// `A→M` plays its instrument and **nothing else**. The buffer handed to
    /// the instrument still holds the microphone, and `render` is not required
    /// to overwrite what it was given — a plugin with nothing to play may add
    /// to it or leave it alone. Either way the input would leak out next to the
    /// synth, which is the one thing this mode exists to prevent.
    #[test]
    fn audio_to_midi_does_not_leak_the_input() {
        // Shares the process-wide transport: `render` advances it.
        let _clock = crate::test_locks::transport();
        /// A source that adds to the buffer instead of filling it, which is
        /// what a plugin idling on an empty note list does.
        struct Adder;
        impl AudioSource for Adder {
            fn render(&mut self, out: &mut [f32], _sr: u32) -> usize {
                for s in out.iter_mut() {
                    *s += 0.0;
                }
                out.len() / 2
            }
            fn plays_on_transport_stop(&self) -> bool {
                true
            }
        }

        let (mut cmd_tx, _retired, mut state) = mk_state_ch(2, 2);
        cmd_tx
            .push(EngineCommand::AddSlot(Box::new(Adder)))
            .unwrap();
        cmd_tx
            .push(EngineCommand::SetSlotIn {
                slot: 0,
                pair: Some((0, 1)),
            })
            .unwrap();
        cmd_tx
            .push(EngineCommand::SetSlotPitchToMidi { slot: 0, on: true })
            .unwrap();
        state.apply_commands();
        for ch in state.capture.iter_mut() {
            ch.fill(0.5);
        }
        state.render(8);
        assert!(
            state.mix[0][..8].iter().all(|s| s.abs() < 1e-6),
            "the microphone reached the mix: {:?}",
            &state.mix[0][..8]
        );
    }

    /// The converter's dry/wet: how much of the input comes back with the
    /// instrument. All wet is the instrument alone; anything less brings the
    /// guitar back under the synth it is driving, which is the sound most
    /// people are actually after.
    #[test]
    fn the_converter_can_bring_the_input_back_under_the_instrument() {
        // Shares the process-wide transport: `render` advances it.
        let _clock = crate::test_locks::transport();
        /// Renders a constant, so the two halves of the blend are told apart.
        struct Dc(f32);
        impl AudioSource for Dc {
            fn render(&mut self, out: &mut [f32], _sr: u32) -> usize {
                out.fill(self.0);
                out.len() / 2
            }
            fn plays_on_transport_stop(&self) -> bool {
                true
            }
        }

        let run = |mix: f32| -> f32 {
            let (mut cmd_tx, _retired, mut state) = mk_state_ch(2, 2);
            cmd_tx
                .push(EngineCommand::AddSlot(Box::new(Dc(1.0))))
                .unwrap();
            cmd_tx
                .push(EngineCommand::SetSlotIn {
                    slot: 0,
                    pair: Some((0, 1)),
                })
                .unwrap();
            cmd_tx
                .push(EngineCommand::SetSlotPitchToMidi { slot: 0, on: true })
                .unwrap();
            cmd_tx
                .push(EngineCommand::SetSlotPitchMix { slot: 0, mix })
                .unwrap();
            state.apply_commands();
            for ch in state.capture.iter_mut() {
                ch.fill(0.5);
            }
            state.render(8);
            // Undo the constant-power pan so the numbers are the blend itself.
            state.mix[0][0] / std::f32::consts::FRAC_1_SQRT_2
        };
        assert!(
            (run(1.0) - 1.0).abs() < 1e-5,
            "all wet is the instrument alone"
        );
        assert!((run(0.0) - 0.5).abs() < 1e-5, "all dry is the input alone");
        assert!(
            (run(0.5) - 0.75).abs() < 1e-5,
            "and half is half of each: {}",
            run(0.5)
        );
    }

    /// The instrument's preset browser has to survive the trip: the handle is
    /// captured when the source is added **and** when one replaces another, and
    /// the UI reaches the real plugin through it. Without the second capture a
    /// tab that changed instrument would list the old one's patches.
    #[test]
    fn a_slots_preset_browser_follows_the_instrument_in_it() {
        use std::sync::{Arc, Mutex};

        #[derive(Default)]
        struct Browser {
            name: &'static str,
            loaded: Mutex<Vec<String>>,
        }
        impl choz_ports::PluginPresets for Browser {
            fn list(&self) -> Vec<choz_ports::PresetEntry> {
                vec![choz_ports::PresetEntry {
                    name: self.name.to_string(),
                    category: "Keys".to_string(),
                    key: format!("{}-key", self.name),
                }]
            }
            fn load(&self, key: &str) {
                self.loaded.lock().unwrap().push(key.to_string());
            }
        }

        struct Synth(Arc<Browser>);
        impl AudioSource for Synth {
            fn render(&mut self, out: &mut [f32], _sr: u32) -> usize {
                out.fill(0.0);
                out.len() / 2
            }
            fn presets(&self) -> Option<choz_ports::PresetsHandle> {
                Some(self.0.clone())
            }
        }

        let mut engine = AudioEngine::new(48_000, 256);
        let first = Arc::new(Browser {
            name: "first",
            ..Default::default()
        });
        let slot = engine.add_slot(Box::new(Synth(first.clone()))).unwrap();

        let listed = engine.slot_presets(slot);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "first");
        assert_eq!(listed[0].category, "Keys");

        engine.load_slot_preset(slot, &listed[0].key);
        assert_eq!(first.loaded.lock().unwrap().as_slice(), ["first-key"]);

        // Another instrument in the same tab: the old browser is gone, and
        // loading goes to the new one.
        let second = Arc::new(Browser {
            name: "second",
            ..Default::default()
        });
        engine.set_slot_source(slot, Box::new(Synth(second.clone())));
        let listed = engine.slot_presets(slot);
        assert_eq!(listed[0].name, "second");
        engine.load_slot_preset(slot, &listed[0].key);
        assert_eq!(second.loaded.lock().unwrap().len(), 1);
        assert_eq!(first.loaded.lock().unwrap().len(), 1, "the old one is idle");

        // A source with no browser of its own says so, and so does a tab that
        // does not exist.
        let silent = engine.add_silent().unwrap();
        assert!(engine.slot_presets(silent).is_empty());
        assert!(engine.slot_presets(99).is_empty());
        engine.load_slot_preset(99, "nowhere");
    }

    /// **The whole path a live microphone takes**: capture buffers → the tab's
    /// FX chain → the mix.
    ///
    /// Written because "I plugged a microphone into the Harmonizer and got no
    /// response" kept being answered by measuring the effect on its own, which
    /// only ever said the effect was fine. This one asks the question the
    /// report actually asks: does what comes in a capture jack reach the
    /// effects on that tab, and does what they produce reach the output?
    #[test]
    fn live_input_reaches_the_tabs_fx_chain_and_comes_back_out() {
        let _clock = crate::test_locks::transport();
        struct Silent;
        impl AudioSource for Silent {
            fn render(&mut self, out: &mut [f32], _sr: u32) -> usize {
                out.fill(0.0);
                out.len() / 2
            }
            fn plays_on_transport_stop(&self) -> bool {
                true
            }
        }

        let (mut cmd_tx, _retired, mut state) = mk_state_ch(2, 2);
        cmd_tx
            .push(EngineCommand::AddSlot(Box::new(Silent)))
            .unwrap();
        cmd_tx
            .push(EngineCommand::SetSlotIn {
                slot: 0,
                pair: Some((0, 1)),
            })
            .unwrap();
        // A harmoniser, exactly as the rack builds one: from a spec.
        let spec = crate::fx_chain::FxSpec {
            gate: None,
            kind: "harmonizer".into(),
            enabled: true,
            wet: 1.0,
            params: vec![0.334, 0.0, 0.0, 0.20, 0.32, 0.36, 0.50, 1.0, 1.0],
            plugin: None,
        };
        let chain = crate::fx_chain::build_chain_from_specs(&[spec], 48_000, 64);
        assert_eq!(chain.len(), 1, "the harmoniser built");
        cmd_tx
            .push(EngineCommand::SetSlotFx { slot: 0, fx: chain })
            .unwrap();
        state.apply_commands();

        // A tone in the capture buffers, block after block, and the tail is
        // what the tab put out.
        let mut worst = 0.0f32;
        for block in 0..40 {
            for (ch, buf) in state.capture.iter_mut().enumerate() {
                for (i, s) in buf.iter_mut().enumerate() {
                    let n = (block * 32 + i) as f32;
                    *s = 0.3 * (std::f32::consts::TAU * 220.0 * n / 48_000.0).sin();
                    let _ = ch;
                }
            }
            for m in state.mix.iter_mut() {
                m.fill(0.0);
            }
            state.render(64);
            if block > 20 {
                worst = worst.max(
                    state.mix[0]
                        .iter()
                        .chain(state.mix[1].iter())
                        .fold(0.0f32, |a, s| a.max(s.abs())),
                );
            }
        }
        assert!(
            worst > 0.02,
            "the microphone never reached the FX chain: peak {worst}"
        );
    }

    /// **The input trim is the detector's, not the mix's.**
    ///
    /// Reported from a real guitar: a tab that tracked well sounded saturated,
    /// because the only way to make the tracker hear was `IN`, and `IN` was
    /// also multiplying what came back through `MIX`. Turning one knob had to
    /// wreck the other. Now the trim reaches the tracker and nothing else — the
    /// tab's `VOL` is the level control, and it always was.
    #[test]
    fn the_input_trim_does_not_reach_what_comes_back_through_the_mix() {
        let _clock = crate::test_locks::transport();
        struct Silent;
        impl AudioSource for Silent {
            fn render(&mut self, out: &mut [f32], _sr: u32) -> usize {
                out.fill(0.0);
                out.len() / 2
            }
            fn plays_on_transport_stop(&self) -> bool {
                true
            }
        }

        let heard = |trim: f32| -> f32 {
            let (mut cmd_tx, _retired, mut state) = mk_state_ch(2, 2);
            cmd_tx
                .push(EngineCommand::AddSlot(Box::new(Silent)))
                .unwrap();
            cmd_tx
                .push(EngineCommand::SetSlotIn {
                    slot: 0,
                    pair: Some((0, 1)),
                })
                .unwrap();
            cmd_tx
                .push(EngineCommand::SetSlotPitchToMidi { slot: 0, on: true })
                .unwrap();
            // All dry: what comes out is the input and nothing else.
            cmd_tx
                .push(EngineCommand::SetSlotPitchMix { slot: 0, mix: 0.0 })
                .unwrap();
            cmd_tx
                .push(EngineCommand::SetSlotInTrim {
                    slot: 0,
                    gain: trim,
                    gate: crate::pitch::DEFAULT_GATE,
                })
                .unwrap();
            state.apply_commands();
            for ch in state.capture.iter_mut() {
                ch.fill(0.25);
            }
            state.render(8);
            state.mix[0][0] / std::f32::consts::FRAC_1_SQRT_2
        };

        let unity = heard(1.0);
        assert!(
            (unity - 0.25).abs() < 1e-5,
            "unity trim is the input: {unity}"
        );
        for trim in [2.0f32, 6.0] {
            let loud = heard(trim);
            assert!(
                (loud - unity).abs() < 1e-5,
                "trim {trim} moved what the player hears: {loud} vs {unity}"
            );
        }
    }

    /// A trim that reaches +24 dB reaches past full scale. Driven past it the
    /// signal is a square wave: it saturates what comes out, and it hands the
    /// pitch detector a waveform whose period is not the one that was played.
    /// The ceiling is soft so it degrades into compression, and it is counted
    /// so the panel can say which knob to turn down.
    #[test]
    fn a_trim_past_full_scale_is_limited_and_counted() {
        // Shares the process-wide transport: `render` advances it.
        let _clock = crate::test_locks::transport();
        let (mut cmd_tx, _retired, mut state) = mk_state_ch(2, 2);
        let health = crate::meter::capture_health();
        health.clear();
        cmd_tx
            .push(EngineCommand::AddSlot(Box::new(crate::sources::Silence)))
            .unwrap();
        cmd_tx
            .push(EngineCommand::SetSlotIn {
                slot: 0,
                pair: Some((0, 1)),
            })
            .unwrap();
        cmd_tx
            .push(EngineCommand::SetSlotInTrim {
                slot: 0,
                gain: 8.0,
                gate: crate::pitch::DEFAULT_GATE,
            })
            .unwrap();
        state.apply_commands();
        for ch in state.capture.iter_mut() {
            ch.fill(0.5); // ×8 = 4.0, four times over
        }
        state.render(8);

        let peak = state.mix[0][..8].iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(
            peak < 1.05,
            "the ceiling should hold it near full scale, got {peak}"
        );
        assert!(peak > 0.5, "and it should still be loud: {peak}");
        assert!(health.clipping() > 0, "and it has to say it is limiting");

        // A trim that fits leaves the signal alone and says nothing.
        health.clear();
        cmd_tx
            .push(EngineCommand::SetSlotInTrim {
                slot: 0,
                gain: 1.0,
                gate: crate::pitch::DEFAULT_GATE,
            })
            .unwrap();
        state.apply_commands();
        state.render(8);
        assert_eq!(health.clipping(), 0, "a trim that fits is not limiting");
        health.clear();
    }

    /// **Sample-accurate notes.** A note with a transport sample on it starts
    /// on that sample, not at the top of whichever block noticed it — the slot
    /// is rendered in segments and the note is applied between them. That is
    /// the whole point: the resolution stops being how often the interface
    /// wakes up.
    #[test]
    fn a_scheduled_note_starts_on_the_sample_it_was_written_for() {
        /// Silent until told to play, then a constant. Where the constant
        /// starts *is* the note's timing, visible in the buffer.
        struct Switch(bool);
        impl AudioSource for Switch {
            fn render(&mut self, out: &mut [f32], _sr: u32) -> usize {
                out.fill(if self.0 { 1.0 } else { 0.0 });
                out.len() / 2
            }
            fn note_on(&mut self, _n: u8, _v: u8) {
                self.0 = true;
            }
            fn note_off(&mut self, _n: u8) {
                self.0 = false;
            }
            fn plays_on_transport_stop(&self) -> bool {
                true
            }
        }

        let _g = crate::test_locks::transport();
        let _m = crate::test_locks::meter();
        let t = choz_ports::transport();
        let was = t.playing();
        t.set_sample_rate(48_000);
        t.set_playing(true);
        t.rewind();

        let (mut cmd_tx, _retired, mut state) = mk_state_ch(2, 0);
        cmd_tx
            .push(EngineCommand::AddSlot(Box::new(Switch(false))))
            .unwrap();
        // Nine samples into a sixteen-sample block.
        cmd_tx
            .push(EngineCommand::NoteOn {
                slot: 0,
                note: 60,
                vel: 100,
                at: 9,
            })
            .unwrap();
        state.apply_commands();
        state.render(16);

        let c = std::f32::consts::FRAC_1_SQRT_2;
        for f in 0..9 {
            assert!(
                state.mix[0][f].abs() < 1e-6,
                "silent until sample 9, but frame {f} is {}",
                state.mix[0][f]
            );
        }
        for f in 9..16 {
            assert!(
                (state.mix[0][f] - c).abs() < 1e-5,
                "sounding from sample 9, but frame {f} is {}",
                state.mix[0][f]
            );
        }

        t.set_playing(was);
    }

    /// Two notes in one block, and the second one stops what the first
    /// started. Both land where they were written, which a queue that only
    /// looked at the first event of a block would get wrong.
    #[test]
    fn several_scheduled_notes_in_one_block_all_land() {
        struct Switch(bool);
        impl AudioSource for Switch {
            fn render(&mut self, out: &mut [f32], _sr: u32) -> usize {
                out.fill(if self.0 { 1.0 } else { 0.0 });
                out.len() / 2
            }
            fn note_on(&mut self, _n: u8, _v: u8) {
                self.0 = true;
            }
            fn note_off(&mut self, _n: u8) {
                self.0 = false;
            }
            fn plays_on_transport_stop(&self) -> bool {
                true
            }
        }

        let _g = crate::test_locks::transport();
        let _m = crate::test_locks::meter();
        let t = choz_ports::transport();
        let was = t.playing();
        t.set_sample_rate(48_000);
        t.set_playing(true);
        t.rewind();

        let (mut cmd_tx, _retired, mut state) = mk_state_ch(2, 0);
        cmd_tx
            .push(EngineCommand::AddSlot(Box::new(Switch(false))))
            .unwrap();
        // Pushed out of order on purpose: the queue is not the running order.
        cmd_tx
            .push(EngineCommand::NoteOff {
                slot: 0,
                note: 60,
                at: 12,
            })
            .unwrap();
        cmd_tx
            .push(EngineCommand::NoteOn {
                slot: 0,
                note: 60,
                vel: 100,
                at: 4,
            })
            .unwrap();
        state.apply_commands();
        state.render(16);

        let c = std::f32::consts::FRAC_1_SQRT_2;
        let sounding: Vec<bool> = (0..16).map(|f| state.mix[0][f].abs() > c * 0.5).collect();
        let expected: Vec<bool> = (0..16).map(|f| (4..12).contains(&f)).collect();
        assert_eq!(sounding, expected, "the note should run from 4 to 12");

        t.set_playing(was);
    }

    /// A note with no time on it is still immediate — every input that has no
    /// schedule of its own sends one, and none of them may be delayed.
    #[test]
    fn a_note_without_a_time_plays_at_once() {
        let (mut cmd_tx, _retired, mut state) = mk_state_ch(2, 0);
        let played = std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
        cmd_tx
            .push(EngineCommand::AddSlot(Box::new(RecordingSynth(
                played.clone(),
            ))))
            .unwrap();
        cmd_tx
            .push(EngineCommand::NoteOn {
                slot: 0,
                note: 64,
                vel: 100,
                at: 0,
            })
            .unwrap();
        state.apply_commands();
        assert_eq!(
            *played.lock(),
            vec![(true, 64)],
            "an untimed note is played by `apply_commands`, before any render"
        );
    }

    /// The same tab with the transport stopped. A multi-effect is not a
    /// sequencer: a microphone does not wait for the play button.
    #[test]
    fn a_capture_fed_tab_works_with_the_transport_stopped() {
        // Shares the process-wide transport: `render` advances it.
        let _clock = crate::test_locks::transport();
        let (mut cmd_tx, _retired, mut state) = mk_state_ch(2, 2);
        state.playing.store(false, Ordering::Relaxed);
        cmd_tx
            .push(EngineCommand::AddSlot(Box::new(crate::sources::Silence)))
            .unwrap();
        cmd_tx
            .push(EngineCommand::SetSlotIn {
                slot: 0,
                pair: Some((0, 1)),
            })
            .unwrap();
        cmd_tx
            .push(EngineCommand::SetSlotFx {
                slot: 0,
                fx: vec![Box::new(AddFx(1.0))],
            })
            .unwrap();
        state.apply_commands();
        for ch in state.capture.iter_mut() {
            ch.fill(0.25);
        }
        state.render(8);
        assert!(
            state.mix[0][0].abs() > 0.1,
            "a stopped transport must not mute the input, got {}",
            state.mix[0][0]
        );
    }

    /// Per-slot output routing: two slots on different pairs of a 6-channel
    /// interface land on their own channels and nowhere else. This is what
    /// "MIDI → plugin → out 5/6" rests on.
    #[test]
    fn slots_land_on_the_output_pair_they_are_routed_to() {
        // Shares the process-wide transport: `render` advances it.
        let _clock = crate::test_locks::transport();
        let (mut cmd_tx, _retired, mut state) = mk_state_ch(6, 0);
        cmd_tx
            .push(EngineCommand::AddSlot(Box::new(DcSource(0.25))))
            .unwrap();
        cmd_tx
            .push(EngineCommand::AddSlot(Box::new(DcSource(0.5))))
            .unwrap();
        cmd_tx
            .push(EngineCommand::SetSlotOut {
                slot: 1,
                left: 4,
                right: 5,
            })
            .unwrap();
        state.apply_commands();
        state.render(8);

        // Centred slots come through the constant-power pan law at -3 dB.
        let c = std::f32::consts::FRAC_1_SQRT_2;
        assert!(
            (state.mix[0][0] - 0.25 * c).abs() < 1e-6,
            "slot 0 on the default pair"
        );
        assert!((state.mix[1][0] - 0.25 * c).abs() < 1e-6);
        assert_eq!(
            state.mix[2][0], 0.0,
            "nothing bleeds onto the untouched pair"
        );
        assert_eq!(state.mix[3][0], 0.0);
        assert!(
            (state.mix[4][0] - 0.5 * c).abs() < 1e-6,
            "slot 1 plays out of 5/6"
        );
        assert!((state.mix[5][0] - 0.5 * c).abs() < 1e-6);
    }

    /// A pair pointing past the device's channels folds onto the last one
    /// instead of panicking or going silent.
    #[test]
    fn an_out_of_range_pair_folds_onto_the_last_channel() {
        // Shares the process-wide transport: `render` advances it.
        let _clock = crate::test_locks::transport();
        let (mut cmd_tx, _retired, mut state) = mk_state_ch(2, 0);
        cmd_tx
            .push(EngineCommand::AddSlot(Box::new(DcSource(0.5))))
            .unwrap();
        cmd_tx
            .push(EngineCommand::SetSlotOut {
                slot: 0,
                left: 8,
                right: 9,
            })
            .unwrap();
        state.apply_commands();
        state.render(8);
        let c = std::f32::consts::FRAC_1_SQRT_2;
        assert!(
            (state.mix[1][0] - 2.0 * 0.5 * c).abs() < 1e-6,
            "both sides summed onto the last channel"
        );
    }

    /// A slot fed by a capture pair ignores its own source and runs its FX
    /// chain over the live audio instead — "AUDIO 1/2 → plugin → out 5/6".
    #[test]
    fn a_slot_routed_to_an_input_pair_processes_live_audio() {
        // Shares the process-wide transport: `render` advances it.
        let _clock = crate::test_locks::transport();
        let (mut cmd_tx, _retired, mut state) = mk_state_ch(4, 2);
        cmd_tx
            .push(EngineCommand::AddSlot(Box::new(DcSource(0.9))))
            .unwrap();
        cmd_tx
            .push(EngineCommand::SetSlotIn {
                slot: 0,
                pair: Some((0, 1)),
            })
            .unwrap();
        cmd_tx
            .push(EngineCommand::SetSlotOut {
                slot: 0,
                left: 2,
                right: 3,
            })
            .unwrap();
        cmd_tx
            .push(EngineCommand::SetSlotFx {
                slot: 0,
                fx: vec![Box::new(AddFx(0.1))],
            })
            .unwrap();
        state.apply_commands();
        // Hardware input: what the backend copies in before rendering.
        state.write_capture(0, &[0.2; 8]);
        state.write_capture(1, &[0.4; 8]);
        state.render(8);

        let c = std::f32::consts::FRAC_1_SQRT_2;
        assert!(
            (state.mix[2][0] - 0.3 * c).abs() < 1e-6,
            "left input + FX, not the source"
        );
        assert!((state.mix[3][0] - 0.5 * c).abs() < 1e-6, "right input + FX");
        assert_eq!(state.mix[0][0], 0.0, "and none of it on the default pair");
    }

    #[test]
    fn mixes_slots_and_applies_per_slot_fx() {
        // Shares the process-wide transport: `render` advances it.
        let _clock = crate::test_locks::transport();
        let (mut cmd_tx, _retired, mut state) = mk_state();

        cmd_tx
            .push(EngineCommand::AddSlot(Box::new(DcSource(0.25))))
            .unwrap();
        cmd_tx
            .push(EngineCommand::AddSlot(Box::new(DcSource(0.25))))
            .unwrap();
        // Give slot 1 a +1.0 FX.
        cmd_tx
            .push(EngineCommand::SetSlotFx {
                slot: 1,
                fx: vec![Box::new(AddFx(1.0))],
            })
            .unwrap();

        let mut buf = [0.0f32; 8];
        audio_callback(&mut buf, &mut state);
        // slot0 = 0.25, slot1 = 0.25 + 1.0 = 1.25 → sum 1.5, then the default
        // centered constant-power pan (-3 dB) scales both channels.
        let expect = 1.5 * std::f32::consts::FRAC_1_SQRT_2;
        assert!(
            buf.iter().all(|&s| (s - expect).abs() < 1e-6),
            "got {buf:?}"
        );
    }

    #[test]
    fn mixer_strip_applies_gain_pan_mute() {
        // Shares the process-wide transport: `render` advances it.
        let _clock = crate::test_locks::transport();
        let (mut cmd_tx, _retired, mut state) = mk_state();
        cmd_tx
            .push(EngineCommand::AddSlot(Box::new(DcSource(1.0))))
            .unwrap();
        // Half gain, hard left.
        cmd_tx
            .push(EngineCommand::SetSlotMix {
                slot: 0,
                gain: 0.5,
                gain_r: 0.5,
                pan: -1.0,
                mute: false,
            })
            .unwrap();

        let mut buf = [0.0f32; 8];
        audio_callback(&mut buf, &mut state);
        assert!(
            buf.iter().step_by(2).all(|&s| (s - 0.5).abs() < 1e-6),
            "left = gain, got {buf:?}"
        );
        assert!(
            buf.iter().skip(1).step_by(2).all(|&s| s.abs() < 1e-6),
            "right silent, got {buf:?}"
        );

        cmd_tx
            .push(EngineCommand::SetSlotMix {
                slot: 0,
                gain: 0.5,
                gain_r: 0.5,
                pan: -1.0,
                mute: true,
            })
            .unwrap();
        audio_callback(&mut buf, &mut state);
        assert!(
            buf.iter().all(|&s| s == 0.0),
            "mute silences the slot, got {buf:?}"
        );
    }

    #[test]
    fn notes_reach_only_their_target_slot() {
        // Shares the process-wide transport: `render` advances it.
        let _clock = crate::test_locks::transport();
        let (mut cmd_tx, _retired, mut state) = mk_state();
        let a = std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
        let b = std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
        cmd_tx
            .push(EngineCommand::AddSlot(Box::new(RecordingSynth(a.clone()))))
            .unwrap();
        cmd_tx
            .push(EngineCommand::AddSlot(Box::new(RecordingSynth(b.clone()))))
            .unwrap();
        cmd_tx
            .push(EngineCommand::NoteOn {
                slot: 1,
                note: 60,
                vel: 100,
                at: 0,
            })
            .unwrap();

        let mut buf = [0.0f32; 8];
        audio_callback(&mut buf, &mut state);
        assert!(a.lock().is_empty(), "slot 0 is bound to another input");
        assert_eq!(&*b.lock(), &[(true, 60)]);

        // Out-of-range targets are dropped, not panics.
        cmd_tx
            .push(EngineCommand::NoteOn {
                slot: 99,
                note: 62,
                vel: 100,
                at: 0,
            })
            .unwrap();
        cmd_tx
            .push(EngineCommand::SetSlotProgram {
                slot: 1,
                bank: 0,
                preset: 42,
            })
            .unwrap();
        audio_callback(&mut buf, &mut state);
        assert!(a.lock().is_empty());
        assert_eq!(
            b.lock().last(),
            Some(&(true, 200 + 42)),
            "slot 1 got the program change"
        );
    }

    #[test]
    fn pedals_and_wheels_reach_the_targeted_slot_only() {
        // Shares the process-wide transport: `render` advances it.
        let _clock = crate::test_locks::transport();
        let (mut cmd_tx, _retired, mut state) = mk_state();
        let quiet = Expressive::default();
        let played = Expressive::default();
        let (q_ccs, p_ccs) = (quiet.ccs.clone(), played.ccs.clone());
        let p_bends = played.bends.clone();

        cmd_tx
            .push(EngineCommand::AddSlot(Box::new(quiet)))
            .unwrap();
        cmd_tx
            .push(EngineCommand::AddSlot(Box::new(played)))
            .unwrap();
        // Sustain down, modulation wheel up, bend fully up.
        cmd_tx
            .push(EngineCommand::ControlChange {
                slot: 1,
                cc: 64,
                value: 127,
            })
            .unwrap();
        cmd_tx
            .push(EngineCommand::ControlChange {
                slot: 1,
                cc: 1,
                value: 90,
            })
            .unwrap();
        cmd_tx
            .push(EngineCommand::PitchBend {
                slot: 1,
                value: 16383,
            })
            .unwrap();
        // Out-of-range targets are dropped, not panics.
        cmd_tx
            .push(EngineCommand::ControlChange {
                slot: 99,
                cc: 64,
                value: 127,
            })
            .unwrap();
        cmd_tx
            .push(EngineCommand::PitchBend { slot: 99, value: 0 })
            .unwrap();

        let mut buf = [0.0f32; 8];
        audio_callback(&mut buf, &mut state);

        assert_eq!(
            &*p_ccs.lock(),
            &[(64, 127), (1, 90)],
            "sustain and mod wheel arrive unfiltered"
        );
        assert_eq!(&*p_bends.lock(), &[16383]);
        assert!(
            q_ccs.lock().is_empty(),
            "a slot bound to another input stays untouched"
        );
    }

    #[test]
    fn set_slot_source_swaps_and_retires_the_old_one() {
        // Shares the process-wide transport: `render` advances it.
        let _clock = crate::test_locks::transport();
        let (mut cmd_tx, mut retired_rx, mut state) = mk_state();
        cmd_tx
            .push(EngineCommand::AddSlot(Box::new(DcSource(0.5))))
            .unwrap();
        cmd_tx
            .push(EngineCommand::SetSlotSource {
                slot: 0,
                source: Box::new(DcSource(0.25)),
            })
            .unwrap();

        let mut buf = [0.0f32; 8];
        audio_callback(&mut buf, &mut state);
        let expect = 0.25 * std::f32::consts::FRAC_1_SQRT_2;
        assert!(
            buf.iter().all(|&s| (s - expect).abs() < 1e-6),
            "new source is live, got {buf:?}"
        );
        assert!(
            matches!(retired_rx.pop(), Ok(Retired::Source(_))),
            "old source dropped off-RT"
        );
    }

    /// A source that is gated by the play button — a WAV, a tone — must not be
    /// probed: rendering it would spend the start of the file before anyone
    /// pressed play.
    struct Gated;
    impl AudioSource for Gated {
        fn render(&mut self, out: &mut [f32], _sr: u32) -> usize {
            out.fill(1.0);
            out.len() / 2
        }
    }

    /// A fresh instrument is measured on the way in, so the fader can be set
    /// before the first note is heard rather than after.
    #[test]
    fn a_new_instrument_is_measured_before_it_reaches_the_audio_thread() {
        let levels = crate::meter::slot_levels();
        // Two tabs nothing else in this file touches.
        let (probed, skipped) = (20usize, 21usize);

        let mut synth: Source = Box::new(DcSource(0.4));
        AudioEngine::probe_levels(&mut synth, probed, 48_000);
        let (peak, rms) = levels.read(probed);
        assert!((peak - 0.4).abs() < 1e-6, "peak {peak}");
        assert!((rms - 0.4).abs() < 1e-6, "rms {rms}");

        // The probe leaves a reading, not a running note.
        let mut src: Source = Box::new(Gated);
        AudioEngine::probe_levels(&mut src, skipped, 48_000);
        assert_eq!(
            levels.read(skipped),
            (0.0, 0.0),
            "a gated source is left alone"
        );

        // And a probe of a new instrument replaces the old one's reading rather
        // than being hidden behind it.
        let mut quiet: Source = Box::new(DcSource(0.05));
        AudioEngine::probe_levels(&mut quiet, probed, 48_000);
        let (peak, _) = levels.read(probed);
        assert!((peak - 0.05).abs() < 1e-6, "the loud one is gone: {peak}");

        levels.reset(probed);
        levels.reset(skipped);
    }

    /// A subgroup is a destination that is not a device: tabs sum into it, its
    /// own fader rides them together, and its output pair decides where the
    /// group lands. The main fader is the last thing the first pair sees.
    #[test]
    fn a_subgroup_carries_its_tabs_and_the_main_rides_everything() {
        let _clock = crate::test_locks::transport();
        // Four outputs: the group can be sent somewhere the main is not.
        let (mut cmd_tx, _retired, mut state) = mk_state_ch(4, 0);
        cmd_tx
            .push(EngineCommand::AddSlot(Box::new(DcSource(0.5))))
            .unwrap();
        cmd_tx
            .push(EngineCommand::AddSlot(Box::new(DcSource(0.25))))
            .unwrap();
        // Both tabs onto bus A, and the bus out of channels 3/4.
        for slot in 0..2 {
            cmd_tx
                .push(EngineCommand::SetSlotDest {
                    slot,
                    dest: Dest::Bus(0),
                })
                .unwrap();
        }
        cmd_tx
            .push(EngineCommand::SetBus {
                bus: 0,
                gain: 1.0,
                mute: false,
                left: 2,
                right: 3,
            })
            .unwrap();
        state.apply_commands();
        state.render(8);

        // Constant-power pan sits a centred tab at 1/sqrt(2) on each side.
        let unity = std::f32::consts::FRAC_1_SQRT_2;
        let both = (0.5 + 0.25) * unity;
        assert!(
            (state.mix[2][0] - both).abs() < 1e-5,
            "the group carries both tabs: {}",
            state.mix[2][0]
        );
        assert!(
            state.mix[0][0].abs() < 1e-6,
            "and nothing reached the pair they are not routed to: {}",
            state.mix[0][0]
        );

        // The group's own fader rides the tabs together.
        cmd_tx
            .push(EngineCommand::SetBus {
                bus: 0,
                gain: 0.5,
                mute: false,
                left: 2,
                right: 3,
            })
            .unwrap();
        state.apply_commands();
        state.render(8);
        assert!(
            (state.mix[2][0] - both * 0.5).abs() < 1e-5,
            "half the group: {}",
            state.mix[2][0]
        );

        // Muted, it is gone — but its tabs still rendered, which is what keeps
        // envelopes and playheads moving under a muted group.
        cmd_tx
            .push(EngineCommand::SetBus {
                bus: 0,
                gain: 0.5,
                mute: true,
                left: 2,
                right: 3,
            })
            .unwrap();
        state.apply_commands();
        state.render(8);
        assert!(
            state.mix[2][0].abs() < 1e-6,
            "a muted group does not arrive"
        );

        // Back to the main pair, where the main fader is the last word.
        for slot in 0..2 {
            cmd_tx
                .push(EngineCommand::SetSlotDest {
                    slot,
                    dest: Dest::Direct,
                })
                .unwrap();
        }
        cmd_tx
            .push(EngineCommand::SetMain {
                gain: 0.5,
                mute: false,
            })
            .unwrap();
        state.apply_commands();
        state.render(8);
        assert!(
            (state.mix[0][0] - both * 0.5).abs() < 1e-5,
            "the main halves the first pair: {}",
            state.mix[0][0]
        );
        assert!(
            state.mix[2][0].abs() < 1e-6,
            "and the group's pair is empty again"
        );

        // The main is the first pair only: another pair is a separate output,
        // not something a master fader may silence.
        cmd_tx
            .push(EngineCommand::SetSlotOut {
                slot: 0,
                left: 2,
                right: 3,
            })
            .unwrap();
        cmd_tx
            .push(EngineCommand::SetMain {
                gain: 0.0,
                mute: false,
            })
            .unwrap();
        state.apply_commands();
        state.render(8);
        assert!(
            (state.mix[2][0] - 0.5 * unity).abs() < 1e-5,
            "channels 3/4 are their own output: {}",
            state.mix[2][0]
        );
    }

    /// The click can be sent to a subgroup — a wedge, say — and it borrows that
    /// group's routing without passing through its fader: a reference that
    /// moves when somebody rides the group is not a reference.
    #[test]
    fn the_click_goes_where_the_metronome_says() {
        let _clock = crate::test_locks::transport();
        let (mut cmd_tx, _retired, mut state) = mk_state_ch(4, 0);
        let m = crate::metronome::metronome();
        m.set_on(true);
        m.set_gain(1.0);
        choz_ports::transport().set_bpm(120.0);
        cmd_tx
            .push(EngineCommand::SetBus {
                bus: 1,
                gain: 0.0,
                mute: true,
                left: 2,
                right: 3,
            })
            .unwrap();
        m.set_dest(Dest::Bus(1));
        state.apply_commands();
        state.render(8);
        let click = (0..8).fold(0.0f32, |acc, f| acc.max(state.mix[2][f].abs()));
        assert!(click > 0.01, "the click reached the group's pair: {click}");
        assert!(
            (0..8).all(|f| state.mix[0][f].abs() < 1e-6),
            "and not the main pair"
        );

        m.set_dest(Dest::Direct);
        m.set_on(false);
    }

    #[test]
    fn remove_slot_returns_it_off_rt() {
        // Shares the process-wide transport: `render` advances it.
        let _clock = crate::test_locks::transport();
        let (mut cmd_tx, mut retired_rx, mut state) = mk_state();
        cmd_tx
            .push(EngineCommand::AddSlot(Box::new(DcSource(0.5))))
            .unwrap();
        cmd_tx.push(EngineCommand::RemoveSlot(0)).unwrap();

        let mut buf = [0.0f32; 8];
        audio_callback(&mut buf, &mut state);
        assert!(state.slots.is_empty(), "slot removed");
        assert!(
            matches!(retired_rx.pop(), Ok(Retired::Slot(_))),
            "removed slot returned for off-RT drop"
        );
        assert!(buf.iter().all(|&s| s == 0.0), "empty rack is silent");
    }
}
