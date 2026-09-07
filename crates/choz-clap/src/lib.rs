//! choz itself, as a CLAP instrument: the whole rack inside somebody else's
//! host, with one stereo output per tab.
//!
//! The other CLAP crates in this workspace go the other two directions —
//! `choz-plugin-clap` is choz *loading* a plugin, `choz-plugin-clap-export`
//! publishes choz's own effects one at a time. This one publishes **choz**: the
//! rack, its tabs, their instruments and FX chains, the mixer, all of it, as a
//! single plugin a DAW loads on a track.
//!
//! # Sixteen outs, which is the point
//!
//! A sampler earns its place in a session by handing the host each of its parts
//! separately, so the desk can treat them as tracks. choz already routes every
//! tab to a pair of output channels (`set_slot_out`), and a plugin is the only
//! place where those pairs land somewhere useful without a patchbay. So this
//! publishes sixteen stereo output ports: put a tab on pair 4 in the rack and
//! it comes out of the host's fourth output, on its own track, with the host's
//! own inserts on it.
//!
//! # What runs where
//!
//! Nothing new. choz already splits the rack between the thread that edits it
//! and the thread that renders it, because a sound card imposes exactly that —
//! and the command ring between them is unchanged here. The interface half
//! ([`choz_ui::embed::Embedded`]) stays on the host's main thread; the RT half
//! (`choz_engine::EmbeddedRt`) is called from `process` and nowhere else.
//!
//! The host's notes cross the same way choz's own JACK MIDI port crosses: a
//! lock-free ring filled on the audio thread and drained on the main one, so
//! `process` never touches the rack's data structures.
//!
//! # What this does not do yet
//!
//! **One instance per process.** The transport, the meters and the sequencer
//! reader choz's MIDI input runs on are singletons of the process, on purpose —
//! standalone there is one rack and one clock. Two instances of this plugin in
//! one DAW share all three, and the second one to be wired takes the first
//! one's MIDI. Making them per-instance is a change to the engine, not to this
//! crate.
//!
//! **The window needs an activated plugin.** The rack is built in `activate`,
//! which is the first moment the sample rate is known; a host that opens the
//! editor before activating gets an empty window. Every host tried activates
//! first.
//!
//! # Installing
//!
//! ```bash
//! cargo build --release -p choz-clap
//! cp target/release/libchoz_clap.so ~/.clap/choz.clap
//! ```

mod grid;
mod gui;

use std::ffi::{c_char, c_void, CStr};

use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::entry::clap_plugin_entry;
use clap_sys::events::{
    clap_event_header, clap_event_midi, clap_event_note, CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_MIDI,
    CLAP_EVENT_NOTE_OFF, CLAP_EVENT_NOTE_ON,
};
use clap_sys::ext::audio_ports::{
    clap_audio_port_info, clap_plugin_audio_ports, CLAP_AUDIO_PORT_IS_MAIN, CLAP_EXT_AUDIO_PORTS,
    CLAP_PORT_STEREO,
};
use clap_sys::ext::gui::{
    clap_gui_resize_hints, clap_plugin_gui, clap_window, CLAP_EXT_GUI, CLAP_WINDOW_API_X11,
};
use clap_sys::ext::note_ports::{
    clap_note_port_info, clap_plugin_note_ports, CLAP_EXT_NOTE_PORTS, CLAP_NOTE_DIALECT_CLAP,
    CLAP_NOTE_DIALECT_MIDI,
};
use clap_sys::ext::state::{clap_plugin_state, CLAP_EXT_STATE};
use clap_sys::ext::timer_support::{
    clap_host_timer_support, clap_plugin_timer_support, CLAP_EXT_TIMER_SUPPORT,
};
use clap_sys::factory::plugin_factory::{clap_plugin_factory, CLAP_PLUGIN_FACTORY_ID};
use clap_sys::host::clap_host;
use clap_sys::id::clap_id;
use clap_sys::plugin::{clap_plugin, clap_plugin_descriptor};
use clap_sys::process::{clap_process, clap_process_status, CLAP_PROCESS_CONTINUE};
use clap_sys::stream::{clap_istream, clap_ostream};
use clap_sys::version::CLAP_VERSION;

use ratatui::Terminal;

use choz_ui::embed::{Embedded, PAIRS};

use crate::grid::Grid;
use crate::gui::{Input, Window};

/// How many host MIDI messages may be waiting when the interface next looks.
/// A held chord plus a controller sweep between two turns of the loop; a full
/// ring drops, because the alternative is blocking the host's audio thread.
const MIDI_RING: usize = 512;

/// The host's own audio, as a stereo pair a rack tab can listen to. One pair:
/// a track has one signal on it, and a tab that wants a second one has the
/// machine's own card for that.
const INPUTS: usize = 2;

/// How often the interface gets a turn. Sixty a second is what the panels are
/// drawn at standalone, and it is also the arpeggiator's clock — a slower timer
/// is a pattern that stutters, not just a panel that lags.
const TIMER_MS: u32 = 16;

// ─── The instance ───────────────────────────────────────────────────────────

/// One loaded choz. The C side is handed `&self.plugin` and finds the rest
/// through `plugin_data`, which points back here.
struct Instance {
    plugin: clap_plugin,
    /// The rack and its panels. Main thread only, and `None` until the host
    /// activates the plugin — there is no rack before there is a sample rate.
    app: Option<Embedded>,
    /// The renderer. Audio thread only, from `process` and nowhere else.
    rt: Option<choz_engine::EmbeddedRt>,
    /// The host's notes, audio thread → main thread.
    midi_tx: Option<rtrb::Producer<[u8; 3]>>,
    midi_rx: Option<rtrb::Consumer<[u8; 3]>>,
    /// Kept across deactivate/activate so a host that stops and starts the
    /// plugin does not lose the rack — `state` is the only other way back.
    saved: Option<String>,
    /// The host, for the one thing choz asks of it: a timer to run the
    /// interface on. Null until `create`.
    host: *const clap_host,
    /// The timer, while the host is running one. Without it the interface has
    /// to make do with `on_main_thread`, which is asked for from `process`.
    timer: Option<clap_id>,
    /// The window and the grid drawn into it, while the host has the editor
    /// open. Main thread only.
    window: Option<Window>,
    terminal: Option<Terminal<Grid>>,
}

/// The interface's turn: the notes that arrived, what the window has to say,
/// and everything `run_app` does between two frames. Main thread only.
fn service(inst: &mut Instance) {
    if let (Some(app), Some(rx)) = (inst.app.as_mut(), inst.midi_rx.as_mut()) {
        while let Ok(bytes) = rx.pop() {
            app.midi(&bytes);
        }
    }
    // The window first: a key pressed before this turn belongs to it.
    if let (Some(window), Some(terminal)) = (inst.window.as_mut(), inst.terminal.as_mut()) {
        for input in window.pump(terminal.backend_mut()) {
            match input {
                Input::Key(code) => {
                    if let Some(app) = inst.app.as_mut() {
                        app.key(code);
                    }
                }
                Input::Mouse(event) => {
                    if let Some(app) = inst.app.as_mut() {
                        app.mouse(event);
                    }
                }
                Input::Resize(cols, rows) => {
                    terminal.backend_mut().resize(cols, rows);
                    let _ = terminal.resize(ratatui::layout::Rect::new(0, 0, cols, rows));
                }
            }
        }
    }
    let Some(app) = inst.app.as_mut() else {
        return;
    };
    app.tick();
    if let (Some(window), Some(terminal)) = (inst.window.as_mut(), inst.terminal.as_mut()) {
        // A draw that fails is a window that has gone; the rack plays on.
        let _ = app.draw(terminal);
        let _ = window.paint(terminal.backend_mut());
    }
}

impl Instance {
    /// The instance behind a `clap_plugin` the C side handed us.
    ///
    /// # Safety
    /// `plugin` must be one this crate created and not yet destroyed.
    unsafe fn of<'a>(plugin: *const clap_plugin) -> Option<&'a mut Instance> {
        let data = unsafe { (*plugin).plugin_data } as *mut Instance;
        (!data.is_null()).then(|| unsafe { &mut *data })
    }
}

// ─── The plugin ─────────────────────────────────────────────────────────────

unsafe extern "C" fn plugin_init(plugin: *const clap_plugin) -> bool {
    let Some(inst) = (unsafe { Instance::of(plugin) }) else {
        return false;
    };
    // A timer is how a CLAP plugin gets a main thread of its own. Hosts that
    // do not run one leave `timer` as `None`, and `process` asks for a callback
    // per block instead — slower and lumpier, but the rack still answers.
    if let Some(timers) =
        unsafe { host_extension::<clap_host_timer_support>(inst.host, CLAP_EXT_TIMER_SUPPORT) }
    {
        if let Some(register) = unsafe { (*timers).register_timer } {
            let mut id: clap_id = 0;
            if unsafe { register(inst.host, TIMER_MS, &mut id) } {
                inst.timer = Some(id);
            }
        }
    }
    true
}

/// One of the host's extensions, or `None` when it does not have it.
///
/// # Safety
/// `host` must be the pointer the factory was handed, or null.
unsafe fn host_extension<T>(host: *const clap_host, id: &CStr) -> Option<*const T> {
    if host.is_null() {
        return None;
    }
    let get = unsafe { (*host).get_extension }?;
    let ext = unsafe { get(host, id.as_ptr()) };
    (!ext.is_null()).then_some(ext as *const T)
}

unsafe extern "C" fn plugin_destroy(plugin: *const clap_plugin) {
    if let Some(inst) = unsafe { Instance::of(plugin) } {
        // The host keeps firing a timer nobody unregistered, at a plugin that
        // is gone.
        if let (Some(id), Some(timers)) = (inst.timer.take(), unsafe {
            host_extension::<clap_host_timer_support>(inst.host, CLAP_EXT_TIMER_SUPPORT)
        }) {
            if let Some(unregister) = unsafe { (*timers).unregister_timer } {
                unsafe { unregister(inst.host, id) };
            }
        }
    }
    let data = unsafe { (*plugin).plugin_data } as *mut Instance;
    if !data.is_null() {
        drop(unsafe { Box::from_raw(data) });
    }
}

unsafe extern "C" fn plugin_activate(
    plugin: *const clap_plugin,
    sample_rate: f64,
    _min_frames: u32,
    max_frames: u32,
) -> bool {
    let Some(inst) = (unsafe { Instance::of(plugin) }) else {
        return false;
    };
    // A rack built for the wrong rate plays the whole session flat, so it is
    // built here — the first moment the rate is known — and not before.
    let Some((mut app, rt)) = Embedded::new(sample_rate as u32, max_frames.max(1), INPUTS) else {
        return false;
    };
    // What the host gave us before it activated, or what it had before it
    // deactivated. Either way the rack comes back the way it was left.
    if let Some(state) = inst.saved.as_deref() {
        if let Err(e) = app.load_state(state) {
            eprintln!("choz: the host's saved rack would not load: {e}");
        }
    }
    let (tx, rx) = rtrb::RingBuffer::new(MIDI_RING);
    inst.app = Some(app);
    inst.rt = Some(rt);
    inst.midi_tx = Some(tx);
    inst.midi_rx = Some(rx);
    true
}

unsafe extern "C" fn plugin_deactivate(plugin: *const clap_plugin) {
    let Some(inst) = (unsafe { Instance::of(plugin) }) else {
        return;
    };
    // Keep the rack as text before letting go of it: a host is free to
    // deactivate and activate again, and everything else here is gone by then.
    if let Some(app) = inst.app.as_mut() {
        if let Ok(state) = app.save_state() {
            inst.saved = Some(state);
        }
    }
    inst.rt = None;
    inst.app = None;
    inst.midi_tx = None;
    inst.midi_rx = None;
}

unsafe extern "C" fn plugin_start_processing(_plugin: *const clap_plugin) -> bool {
    true
}

unsafe extern "C" fn plugin_stop_processing(_plugin: *const clap_plugin) {}

unsafe extern "C" fn plugin_reset(_plugin: *const clap_plugin) {}

/// One block: the host's notes into the ring, the rack rendered, every pair
/// copied out. Audio thread — nothing here allocates or locks.
unsafe extern "C" fn plugin_process(
    plugin: *const clap_plugin,
    process: *const clap_process,
) -> clap_process_status {
    let Some(inst) = (unsafe { Instance::of(plugin) }) else {
        return CLAP_PROCESS_CONTINUE;
    };
    let process = unsafe { &*process };
    let frames = process.frames_count as usize;

    if let Some(tx) = inst.midi_tx.as_mut() {
        unsafe { forward_events(process, tx) };
    }

    let Some(rt) = inst.rt.as_mut() else {
        unsafe { silence(process, frames) };
        return CLAP_PROCESS_CONTINUE;
    };
    // What the host put on the track, before anything is rendered over it: a
    // tab set to listen to `host:in_1` is processing the DAW's audio, which is
    // what makes an embedded rack an effect chain as well as an instrument.
    unsafe { take_input(process, rt, frames) };
    rt.render(frames);

    let ports = process.audio_outputs_count as usize;
    for port in 0..ports {
        let buffer: &clap_audio_buffer = unsafe { &*process.audio_outputs.add(port) };
        if buffer.data32.is_null() {
            continue;
        }
        for channel in 0..(buffer.channel_count as usize).min(2) {
            let dst = unsafe { *buffer.data32.add(channel) };
            if dst.is_null() {
                continue;
            }
            let src = rt.output(port * 2 + channel);
            let n = frames.min(src.len());
            unsafe { std::ptr::copy_nonoverlapping(src.as_ptr(), dst, n) };
            // A pair the rack is not filling has to be *silent*, not whatever
            // the host left in the buffer.
            for f in n..frames {
                unsafe { *dst.add(f) = 0.0 };
            }
        }
    }
    CLAP_PROCESS_CONTINUE
}

/// Every note and MIDI message in this block, as three bytes each, into the
/// ring the interface drains. Anything else the host sends is not ours.
///
/// # Safety
/// `process` must be the block the host is calling `process` with.
unsafe fn forward_events(process: &clap_process, tx: &mut rtrb::Producer<[u8; 3]>) {
    let events = process.in_events;
    if events.is_null() {
        return;
    }
    let (Some(size), Some(get)) = (unsafe { (*events).size }, unsafe { (*events).get }) else {
        return;
    };
    let count = unsafe { size(events) };
    for i in 0..count {
        let header: *const clap_event_header = unsafe { get(events, i) };
        if header.is_null() || unsafe { (*header).space_id } != CLAP_CORE_EVENT_SPACE_ID {
            continue;
        }
        let bytes = match unsafe { (*header).type_ } {
            CLAP_EVENT_NOTE_ON | CLAP_EVENT_NOTE_OFF => {
                let note: &clap_event_note = unsafe { &*(header as *const clap_event_note) };
                let channel = (note.channel.max(0) as u8) & 0x0F;
                // CLAP velocity is 0..1; the wire is 0..127, and a note-on that
                // rounds to zero is a note-off by the oldest convention there
                // is — so it is floored at one.
                let velocity = ((note.velocity * 127.0).round() as i32).clamp(1, 127) as u8;
                match unsafe { (*header).type_ } {
                    CLAP_EVENT_NOTE_ON => [0x90 | channel, note.key.max(0) as u8, velocity],
                    _ => [0x80 | channel, note.key.max(0) as u8, 0],
                }
            }
            CLAP_EVENT_MIDI => {
                let midi: &clap_event_midi = unsafe { &*(header as *const clap_event_midi) };
                midi.data
            }
            _ => continue,
        };
        // A full ring is the interface being slow, and a dropped note-on is
        // better than a blocked audio thread.
        let _ = tx.push(bytes);
    }
}

/// Copy the host's input into the rack's capture buffers. A track with nothing
/// on it hands over silence, and so must a host that gave us no port.
///
/// # Safety
/// `process` must be the block the host is calling `process` with.
unsafe fn take_input(process: &clap_process, rt: &mut choz_engine::EmbeddedRt, frames: usize) {
    let buffer = match process.audio_inputs_count {
        0 => None,
        _ => Some(unsafe { &*process.audio_inputs }),
    };
    for channel in 0..INPUTS {
        let dst = rt.capture_mut(channel);
        let n = frames.min(dst.len());
        let src = buffer.and_then(|b| match b.data32.is_null() {
            true => None,
            // A mono track feeds both sides rather than half the pair: that is
            // what every host does with a mono source on a stereo input.
            false => {
                let index = channel.min(b.channel_count.saturating_sub(1) as usize);
                let ptr = unsafe { *b.data32.add(index) };
                (!ptr.is_null()).then_some(ptr)
            }
        });
        match src {
            Some(src) => unsafe { std::ptr::copy_nonoverlapping(src, dst.as_mut_ptr(), n) },
            None => dst[..n].fill(0.0),
        }
    }
}

/// Fill every output with silence — what a plugin that has no rack yet owes
/// the host, which is not whatever was in the buffer before.
///
/// # Safety
/// `process` must be the block the host is calling `process` with.
unsafe fn silence(process: &clap_process, frames: usize) {
    for port in 0..process.audio_outputs_count as usize {
        let buffer: &clap_audio_buffer = unsafe { &*process.audio_outputs.add(port) };
        if buffer.data32.is_null() {
            continue;
        }
        for channel in 0..buffer.channel_count as usize {
            let dst = unsafe { *buffer.data32.add(channel) };
            if !dst.is_null() {
                unsafe { std::ptr::write_bytes(dst, 0, frames) };
            }
        }
    }
}

/// The fallback interface turn, for a host with no timer. With one, the timer
/// owns the loop and this must not run it too — an arpeggiator ticked twice per
/// frame plays at double speed.
unsafe extern "C" fn plugin_on_main_thread(plugin: *const clap_plugin) {
    let Some(inst) = (unsafe { Instance::of(plugin) }) else {
        return;
    };
    if inst.timer.is_none() {
        service(inst);
    }
}

unsafe extern "C" fn plugin_on_timer(plugin: *const clap_plugin, _timer: clap_id) {
    if let Some(inst) = unsafe { Instance::of(plugin) } {
        service(inst);
    }
}

static TIMER: clap_plugin_timer_support = clap_plugin_timer_support {
    on_timer: Some(plugin_on_timer),
};

unsafe extern "C" fn plugin_get_extension(
    _plugin: *const clap_plugin,
    id: *const c_char,
) -> *const c_void {
    if id.is_null() {
        return std::ptr::null();
    }
    let id = unsafe { CStr::from_ptr(id) };
    if id == CLAP_EXT_AUDIO_PORTS {
        return &AUDIO_PORTS as *const _ as *const c_void;
    }
    if id == CLAP_EXT_NOTE_PORTS {
        return &NOTE_PORTS as *const _ as *const c_void;
    }
    if id == CLAP_EXT_STATE {
        return &STATE as *const _ as *const c_void;
    }
    if id == CLAP_EXT_GUI {
        return &GUI as *const _ as *const c_void;
    }
    if id == CLAP_EXT_TIMER_SUPPORT {
        return &TIMER as *const _ as *const c_void;
    }
    std::ptr::null()
}

// ─── Ports ──────────────────────────────────────────────────────────────────

unsafe extern "C" fn audio_ports_count(_plugin: *const clap_plugin, is_input: bool) -> u32 {
    // One pair in — the track's own audio, for a tab to process — and sixteen
    // out, one per tab.
    match is_input {
        true => 1,
        false => PAIRS as u32,
    }
}

unsafe extern "C" fn audio_ports_get(
    _plugin: *const clap_plugin,
    index: u32,
    is_input: bool,
    info: *mut clap_audio_port_info,
) -> bool {
    if info.is_null() || index as usize >= if is_input { 1 } else { PAIRS } {
        return false;
    }
    let info = unsafe { &mut *info };
    info.id = match is_input {
        // Distinct from the outputs': a host matches a saved wiring by id, and
        // two ports sharing one is a wiring that lands on the wrong side.
        true => u32::MAX - 1,
        false => index,
    };
    let name = match is_input {
        true => "in".to_string(),
        false => format!("out {}", index + 1),
    };
    let bytes = name.as_bytes();
    info.name = [0; 256];
    let n = bytes.len().min(info.name.len() - 1);
    for (slot, byte) in info.name.iter_mut().zip(&bytes[..n]) {
        *slot = *byte as c_char;
    }
    // Pair 1 is the main out — the one everything lands on until a tab is sent
    // somewhere else, and the one a host wires up by itself. The single input
    // is the main one for the same reason.
    info.flags = match (is_input, index) {
        (true, _) | (false, 0) => CLAP_AUDIO_PORT_IS_MAIN,
        _ => 0,
    };
    info.channel_count = 2;
    info.port_type = CLAP_PORT_STEREO.as_ptr();
    info.in_place_pair = u32::MAX;
    true
}

static AUDIO_PORTS: clap_plugin_audio_ports = clap_plugin_audio_ports {
    count: Some(audio_ports_count),
    get: Some(audio_ports_get),
};

unsafe extern "C" fn note_ports_count(_plugin: *const clap_plugin, is_input: bool) -> u32 {
    match is_input {
        true => 1,
        false => 0,
    }
}

unsafe extern "C" fn note_ports_get(
    _plugin: *const clap_plugin,
    index: u32,
    is_input: bool,
    info: *mut clap_note_port_info,
) -> bool {
    if !is_input || index != 0 || info.is_null() {
        return false;
    }
    let info = unsafe { &mut *info };
    info.id = 0;
    info.supported_dialects = CLAP_NOTE_DIALECT_CLAP | CLAP_NOTE_DIALECT_MIDI;
    info.preferred_dialect = CLAP_NOTE_DIALECT_MIDI;
    info.name = [0; 256];
    for (slot, byte) in info.name.iter_mut().zip(b"notes") {
        *slot = *byte as c_char;
    }
    true
}

static NOTE_PORTS: clap_plugin_note_ports = clap_plugin_note_ports {
    count: Some(note_ports_count),
    get: Some(note_ports_get),
};

// ─── State ──────────────────────────────────────────────────────────────────

/// The rack, as the YAML a project file holds. The host stores it with the
/// session, so opening the session opens the rack — every tab, every plugin,
/// every knob — the way saving a project does.
unsafe extern "C" fn state_save(plugin: *const clap_plugin, stream: *const clap_ostream) -> bool {
    let Some(inst) = (unsafe { Instance::of(plugin) }) else {
        return false;
    };
    let yaml = match inst.app.as_mut().map(|a| a.save_state()) {
        Some(Ok(yaml)) => yaml,
        // Not activated: the last rack this instance had is still the answer.
        _ => match inst.saved.clone() {
            Some(yaml) => yaml,
            None => return false,
        },
    };
    let Some(write) = (unsafe { (*stream).write }) else {
        return false;
    };
    let bytes = yaml.as_bytes();
    let mut sent = 0usize;
    while sent < bytes.len() {
        let n = unsafe {
            write(
                stream,
                bytes[sent..].as_ptr() as *const c_void,
                (bytes.len() - sent) as u64,
            )
        };
        // 0 is the stream being full for now and a negative is it being over;
        // neither will finish the rack, and a half-written one is worse than
        // none.
        if n <= 0 {
            return false;
        }
        sent += n as usize;
    }
    true
}

unsafe extern "C" fn state_load(plugin: *const clap_plugin, stream: *const clap_istream) -> bool {
    let Some(inst) = (unsafe { Instance::of(plugin) }) else {
        return false;
    };
    let Some(read) = (unsafe { (*stream).read }) else {
        return false;
    };
    let mut yaml = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = unsafe {
            read(
                stream,
                chunk.as_mut_ptr() as *mut c_void,
                chunk.len() as u64,
            )
        };
        if n < 0 {
            return false;
        }
        if n == 0 {
            break;
        }
        yaml.extend_from_slice(&chunk[..n as usize]);
    }
    let Ok(yaml) = String::from_utf8(yaml) else {
        return false;
    };
    // Held either way: a host may load state before it activates, and then the
    // rack is built out of this at `activate`.
    inst.saved = Some(yaml.clone());
    match inst.app.as_mut() {
        Some(app) => app.load_state(&yaml).is_ok(),
        None => true,
    }
}

static STATE: clap_plugin_state = clap_plugin_state {
    save: Some(state_save),
    load: Some(state_load),
};

// ─── The window ─────────────────────────────────────────────────────────────

unsafe extern "C" fn gui_is_api_supported(
    _plugin: *const clap_plugin,
    api: *const c_char,
    is_floating: bool,
) -> bool {
    // X11 only, and inside the host's window. A floating window is one the
    // plugin places and keeps on top itself, which is a window manager's job
    // and not one choz is going to do better.
    !api.is_null() && !is_floating && unsafe { CStr::from_ptr(api) } == CLAP_WINDOW_API_X11
}

unsafe extern "C" fn gui_get_preferred_api(
    _plugin: *const clap_plugin,
    api: *mut *const c_char,
    is_floating: *mut bool,
) -> bool {
    if api.is_null() || is_floating.is_null() {
        return false;
    }
    unsafe {
        *api = CLAP_WINDOW_API_X11.as_ptr();
        *is_floating = false;
    }
    true
}

unsafe extern "C" fn gui_create(
    plugin: *const clap_plugin,
    api: *const c_char,
    is_floating: bool,
) -> bool {
    if !unsafe { gui_is_api_supported(plugin, api, is_floating) } {
        return false;
    }
    let Some(inst) = (unsafe { Instance::of(plugin) }) else {
        return false;
    };
    // Opened under the root for now: X wants a parent at creation and the host
    // does not hand one over until `set_parent`, which reparents this one.
    let Ok((window, cols, rows)) = Window::open(None) else {
        return false;
    };
    let Ok(terminal) = Terminal::new(Grid::new(cols, rows)) else {
        return false;
    };
    inst.window = Some(window);
    inst.terminal = Some(terminal);
    true
}

unsafe extern "C" fn gui_destroy(plugin: *const clap_plugin) {
    if let Some(inst) = unsafe { Instance::of(plugin) } {
        inst.window = None;
        inst.terminal = None;
    }
}

unsafe extern "C" fn gui_set_scale(_plugin: *const clap_plugin, _scale: f64) -> bool {
    // A core X font comes in one size. Reporting failure is what tells the host
    // to leave the window alone rather than scale it behind our back.
    false
}

unsafe extern "C" fn gui_get_size(
    plugin: *const clap_plugin,
    width: *mut u32,
    height: *mut u32,
) -> bool {
    if width.is_null() || height.is_null() {
        return false;
    }
    let Some(inst) = (unsafe { Instance::of(plugin) }) else {
        return false;
    };
    let Some(window) = inst.window.as_ref() else {
        return false;
    };
    let (w, h) = window.pixel_size();
    unsafe {
        *width = w;
        *height = h;
    }
    true
}

unsafe extern "C" fn gui_can_resize(_plugin: *const clap_plugin) -> bool {
    true
}

unsafe extern "C" fn gui_get_resize_hints(
    _plugin: *const clap_plugin,
    hints: *mut clap_gui_resize_hints,
) -> bool {
    if hints.is_null() {
        return false;
    }
    unsafe {
        (*hints).can_resize_horizontally = true;
        (*hints).can_resize_vertically = true;
        (*hints).preserve_aspect_ratio = false;
        (*hints).aspect_ratio_width = 1;
        (*hints).aspect_ratio_height = 1;
    }
    true
}

unsafe extern "C" fn gui_adjust_size(
    plugin: *const clap_plugin,
    width: *mut u32,
    height: *mut u32,
) -> bool {
    if width.is_null() || height.is_null() {
        return false;
    }
    let Some(inst) = (unsafe { Instance::of(plugin) }) else {
        return false;
    };
    let Some(window) = inst.window.as_ref() else {
        return false;
    };
    // Down to whole cells: half a row of characters is a row that is not drawn,
    // and the host would rather be told the size it can actually have.
    let (cols, rows) = unsafe { window.cells_for(*width, *height) };
    let (w, h) = window.pixels_for(cols, rows);
    unsafe {
        *width = w;
        *height = h;
    }
    true
}

unsafe extern "C" fn gui_set_size(plugin: *const clap_plugin, width: u32, height: u32) -> bool {
    let Some(inst) = (unsafe { Instance::of(plugin) }) else {
        return false;
    };
    let (Some(window), Some(terminal)) = (inst.window.as_mut(), inst.terminal.as_mut()) else {
        return false;
    };
    if window.resize(width, height).is_err() {
        return false;
    }
    let (cols, rows) = window.cells_for(width, height);
    terminal.backend_mut().resize(cols, rows);
    let _ = terminal.resize(ratatui::layout::Rect::new(0, 0, cols, rows));
    true
}

unsafe extern "C" fn gui_set_parent(
    plugin: *const clap_plugin,
    window: *const clap_window,
) -> bool {
    if window.is_null() {
        return false;
    }
    let Some(inst) = (unsafe { Instance::of(plugin) }) else {
        return false;
    };
    let Some(ours) = inst.window.as_mut() else {
        return false;
    };
    let parent = unsafe { (*window).specific.x11 } as u32;
    ours.reparent(parent).is_ok()
}

unsafe extern "C" fn gui_set_transient(
    _plugin: *const clap_plugin,
    _window: *const clap_window,
) -> bool {
    // Only a floating window has a parent to be transient for, and this one is
    // never floating.
    false
}

unsafe extern "C" fn gui_suggest_title(_plugin: *const clap_plugin, _title: *const c_char) {}

unsafe extern "C" fn gui_show(plugin: *const clap_plugin) -> bool {
    let Some(inst) = (unsafe { Instance::of(plugin) }) else {
        return false;
    };
    match inst.window.as_ref() {
        Some(window) => window.show().is_ok(),
        None => false,
    }
}

unsafe extern "C" fn gui_hide(plugin: *const clap_plugin) -> bool {
    let Some(inst) = (unsafe { Instance::of(plugin) }) else {
        return false;
    };
    match inst.window.as_ref() {
        Some(window) => window.hide().is_ok(),
        None => false,
    }
}

static GUI: clap_plugin_gui = clap_plugin_gui {
    is_api_supported: Some(gui_is_api_supported),
    get_preferred_api: Some(gui_get_preferred_api),
    create: Some(gui_create),
    destroy: Some(gui_destroy),
    set_scale: Some(gui_set_scale),
    get_size: Some(gui_get_size),
    can_resize: Some(gui_can_resize),
    get_resize_hints: Some(gui_get_resize_hints),
    adjust_size: Some(gui_adjust_size),
    set_size: Some(gui_set_size),
    set_parent: Some(gui_set_parent),
    set_transient: Some(gui_set_transient),
    suggest_title: Some(gui_suggest_title),
    show: Some(gui_show),
    hide: Some(gui_hide),
};

// ─── The factory ────────────────────────────────────────────────────────────

/// Null-terminated, because the descriptor hands the C side a pointer into it.
const PLUGIN_ID: &str = "com.choz.rack\0";
const PLUGIN_NAME: &[u8] = b"choz\0";
const PLUGIN_VENDOR: &[u8] = b"choz\0";
const PLUGIN_VERSION: &[u8] = b"1.3.10\0";
const PLUGIN_DESCRIPTION: &[u8] = b"The whole rack, one stereo output per tab, inside the host.\0";

/// `instrument` and `stereo`, which is what a host filters its browser by.
///
/// Wrapped because a bare array of pointers is not `Sync` and a `static` must
/// be. They point at string literals in this object's own read-only data and
/// are never written, which is the whole of what `Sync` is asking about.
struct Features([*const c_char; 3]);
unsafe impl Sync for Features {}

static FEATURES: Features =
    Features([c"instrument".as_ptr(), c"stereo".as_ptr(), std::ptr::null()]);

static DESCRIPTOR: clap_plugin_descriptor = clap_plugin_descriptor {
    clap_version: CLAP_VERSION,
    id: PLUGIN_ID.as_ptr() as *const c_char,
    name: PLUGIN_NAME.as_ptr() as *const c_char,
    vendor: PLUGIN_VENDOR.as_ptr() as *const c_char,
    url: c"https://github.com/jacodelia/choz".as_ptr(),
    manual_url: std::ptr::null(),
    support_url: std::ptr::null(),
    version: PLUGIN_VERSION.as_ptr() as *const c_char,
    description: PLUGIN_DESCRIPTION.as_ptr() as *const c_char,
    features: FEATURES.0.as_ptr(),
};

unsafe extern "C" fn factory_count(_factory: *const clap_plugin_factory) -> u32 {
    1
}

unsafe extern "C" fn factory_descriptor(
    _factory: *const clap_plugin_factory,
    index: u32,
) -> *const clap_plugin_descriptor {
    match index {
        0 => &DESCRIPTOR,
        _ => std::ptr::null(),
    }
}

unsafe extern "C" fn factory_create(
    _factory: *const clap_plugin_factory,
    host: *const clap_host,
    plugin_id: *const c_char,
) -> *const clap_plugin {
    if plugin_id.is_null() {
        return std::ptr::null();
    }
    let wanted = unsafe { CStr::from_ptr(plugin_id) };
    if wanted.to_bytes_with_nul() != PLUGIN_ID.as_bytes() {
        return std::ptr::null();
    }
    let instance = Box::new(Instance {
        plugin: clap_plugin {
            desc: &DESCRIPTOR,
            plugin_data: std::ptr::null_mut(),
            init: Some(plugin_init),
            destroy: Some(plugin_destroy),
            activate: Some(plugin_activate),
            deactivate: Some(plugin_deactivate),
            start_processing: Some(plugin_start_processing),
            stop_processing: Some(plugin_stop_processing),
            reset: Some(plugin_reset),
            process: Some(plugin_process),
            get_extension: Some(plugin_get_extension),
            on_main_thread: Some(plugin_on_main_thread),
        },
        app: None,
        rt: None,
        midi_tx: None,
        midi_rx: None,
        saved: None,
        host,
        timer: None,
        window: None,
        terminal: None,
    });
    let raw = Box::into_raw(instance);
    // The instance points at itself, which is how the C side finds it again.
    unsafe {
        (*raw).plugin.plugin_data = raw as *mut c_void;
        &(*raw).plugin
    }
}

static FACTORY: clap_plugin_factory = clap_plugin_factory {
    get_plugin_count: Some(factory_count),
    get_plugin_descriptor: Some(factory_descriptor),
    create_plugin: Some(factory_create),
};

unsafe extern "C" fn entry_init(_path: *const c_char) -> bool {
    true
}

unsafe extern "C" fn entry_deinit() {}

unsafe extern "C" fn entry_get_factory(id: *const c_char) -> *const c_void {
    if id.is_null() {
        return std::ptr::null();
    }
    match unsafe { CStr::from_ptr(id) } == CLAP_PLUGIN_FACTORY_ID {
        true => &FACTORY as *const _ as *const c_void,
        false => std::ptr::null(),
    }
}

/// The symbol every CLAP host looks for.
#[allow(non_upper_case_globals)]
#[no_mangle]
pub static clap_entry: clap_plugin_entry = clap_plugin_entry {
    clap_version: CLAP_VERSION,
    init: Some(entry_init),
    deinit: Some(entry_deinit),
    get_factory: Some(entry_get_factory),
};

#[cfg(test)]
mod tests {
    use super::*;

    /// The rack's clock is global to the process and `activate` rewinds it, so
    /// two tests that both build one must not overlap.
    fn guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Open the plugin the way a host does: through the entry point, by id.
    unsafe fn open() -> *const clap_plugin {
        let factory = unsafe { entry_get_factory(CLAP_PLUGIN_FACTORY_ID.as_ptr()) }
            as *const clap_plugin_factory;
        assert!(!factory.is_null(), "the factory is not published");
        let create = unsafe { (*factory).create_plugin }.expect("a factory creates");
        let plugin = unsafe {
            create(
                factory,
                std::ptr::null(),
                PLUGIN_ID.as_ptr() as *const c_char,
            )
        };
        assert!(!plugin.is_null(), "the host could not create choz");
        plugin
    }

    /// What a host sees before it loads anything: one plugin, sixteen stereo
    /// outs, one note port in. The sixteen are the whole point — they are what
    /// puts each rack tab on its own track, the way a sampler's individual outs
    /// do.
    #[test]
    fn the_host_is_offered_one_instrument_with_sixteen_stereo_outs() {
        let _g = guard();
        let plugin = unsafe { open() };

        let ports = unsafe { plugin_get_extension(plugin, CLAP_EXT_AUDIO_PORTS.as_ptr()) }
            as *const clap_plugin_audio_ports;
        assert!(!ports.is_null(), "no audio ports extension");
        let count = unsafe { (*ports).count }.unwrap();
        assert_eq!(unsafe { count(plugin, false) }, 16, "sixteen pairs out");
        assert_eq!(unsafe { count(plugin, true) }, 1, "and the track's own in");

        let get = unsafe { (*ports).get }.unwrap();
        let mut info: clap_audio_port_info = unsafe { std::mem::zeroed() };
        assert!(unsafe { get(plugin, 0, false, &mut info) });
        assert_eq!(info.channel_count, 2);
        assert_eq!(
            info.flags & CLAP_AUDIO_PORT_IS_MAIN,
            CLAP_AUDIO_PORT_IS_MAIN,
            "the first pair is the main out"
        );
        assert!(
            unsafe { get(plugin, 15, false, &mut info) },
            "the sixteenth"
        );
        assert_eq!(info.flags & CLAP_AUDIO_PORT_IS_MAIN, 0);
        assert!(!unsafe { get(plugin, 16, false, &mut info) }, "and no more");

        // The input is a port in its own right, with an id of its own: a host
        // matches saved wiring by id, and a shared one lands on the wrong side.
        assert!(unsafe { get(plugin, 0, true, &mut info) });
        assert_eq!(info.channel_count, 2);
        let input_id = info.id;
        assert!(unsafe { get(plugin, 0, false, &mut info) });
        assert_ne!(info.id, input_id, "the input shares an id with an output");
        assert!(!unsafe { get(plugin, 1, true, &mut info) }, "only one in");

        let notes = unsafe { plugin_get_extension(plugin, CLAP_EXT_NOTE_PORTS.as_ptr()) }
            as *const clap_plugin_note_ports;
        assert!(!notes.is_null(), "no note ports extension");
        let count = unsafe { (*notes).count }.unwrap();
        assert_eq!(unsafe { count(plugin, true) }, 1, "one note port in");

        unsafe { plugin_destroy(plugin) };
    }

    /// Activating builds the rack, and the block that comes back is silence
    /// rather than whatever the host left in its buffers — an empty rack has to
    /// sound like nothing, not like the last plugin on the track.
    #[test]
    fn an_activated_rack_renders_a_block_of_silence_into_every_pair() {
        let _g = guard();
        let plugin = unsafe { open() };
        assert!(unsafe { plugin_init(plugin) });
        assert!(
            unsafe { plugin_activate(plugin, 48_000.0, 32, 128) },
            "the rack would not build"
        );

        const FRAMES: usize = 64;
        // Dirty on purpose: silence has to be *written*, not merely not added.
        let mut left = [1.0f32; FRAMES];
        let mut right = [1.0f32; FRAMES];
        let mut channels = [left.as_mut_ptr(), right.as_mut_ptr()];
        let mut out = clap_audio_buffer {
            data32: channels.as_mut_ptr(),
            data64: std::ptr::null_mut(),
            channel_count: 2,
            latency: 0,
            constant_mask: 0,
        };
        let mut process: clap_process = unsafe { std::mem::zeroed() };
        process.frames_count = FRAMES as u32;
        process.audio_outputs = &mut out;
        process.audio_outputs_count = 1;
        assert_eq!(
            unsafe { plugin_process(plugin, &process) },
            CLAP_PROCESS_CONTINUE
        );

        assert!(
            left.iter().chain(right.iter()).all(|s| s.abs() < 1e-6),
            "the empty rack left something in the buffer: {:?}",
            &left[..8]
        );

        unsafe { plugin_deactivate(plugin) };
        unsafe { plugin_destroy(plugin) };
    }

    /// The track's own audio reaches the rack. A tab listening to `host:in_1`
    /// is processing what the DAW put on the track, which is what makes an
    /// embedded rack an effect chain and not only an instrument.
    #[test]
    fn what_the_host_puts_on_the_track_reaches_the_racks_input() {
        let _g = guard();
        let plugin = unsafe { open() };
        assert!(unsafe { plugin_init(plugin) });
        assert!(unsafe { plugin_activate(plugin, 48_000.0, 32, 128) });

        const FRAMES: usize = 32;
        let mut in_l = [0.25f32; FRAMES];
        let mut in_r = [-0.25f32; FRAMES];
        let mut in_channels = [in_l.as_mut_ptr(), in_r.as_mut_ptr()];
        let input = clap_audio_buffer {
            data32: in_channels.as_mut_ptr(),
            data64: std::ptr::null_mut(),
            channel_count: 2,
            latency: 0,
            constant_mask: 0,
        };
        let mut out_l = [0.0f32; FRAMES];
        let mut out_r = [0.0f32; FRAMES];
        let mut out_channels = [out_l.as_mut_ptr(), out_r.as_mut_ptr()];
        let mut output = clap_audio_buffer {
            data32: out_channels.as_mut_ptr(),
            data64: std::ptr::null_mut(),
            channel_count: 2,
            latency: 0,
            constant_mask: 0,
        };
        let mut process: clap_process = unsafe { std::mem::zeroed() };
        process.frames_count = FRAMES as u32;
        process.audio_inputs = &input;
        process.audio_inputs_count = 1;
        process.audio_outputs = &mut output;
        process.audio_outputs_count = 1;
        assert_eq!(
            unsafe { plugin_process(plugin, &process) },
            CLAP_PROCESS_CONTINUE
        );

        let inst = unsafe { Instance::of(plugin) }.unwrap();
        let rt = inst.rt.as_mut().unwrap();
        assert!(
            rt.capture_mut(0)[..FRAMES]
                .iter()
                .all(|s| (*s - 0.25).abs() < 1e-6),
            "the left side never arrived"
        );
        assert!(
            rt.capture_mut(1)[..FRAMES]
                .iter()
                .all(|s| (*s + 0.25).abs() < 1e-6),
            "the right side never arrived"
        );
        // And with no track connected the rack hears silence, not the last
        // block that happened to be in the buffer.
        let empty = clap_audio_buffer {
            data32: std::ptr::null_mut(),
            ..input
        };
        process.audio_inputs = &empty;
        assert_eq!(
            unsafe { plugin_process(plugin, &process) },
            CLAP_PROCESS_CONTINUE
        );
        let inst = unsafe { Instance::of(plugin) }.unwrap();
        let rt = inst.rt.as_mut().unwrap();
        assert!(rt.capture_mut(0)[..FRAMES].iter().all(|s| s.abs() < 1e-6));

        unsafe { plugin_deactivate(plugin) };
        unsafe { plugin_destroy(plugin) };
    }

    /// **Nothing here may re-run `current_exe`.** Embedded, that is the DAW:
    /// the scan worker, the crash probe and the plugin sandbox would each start
    /// a second copy of it, with its own windows and its own claim on the
    /// audio device. Building a rack is what would trip them, so this checks
    /// the switch is thrown by the time one exists.
    #[test]
    fn an_embedded_rack_never_spawns_a_child_of_the_host() {
        let _g = guard();
        let plugin = unsafe { open() };
        assert!(unsafe { plugin_init(plugin) });
        assert!(unsafe { plugin_activate(plugin, 48_000.0, 32, 128) });
        assert!(
            choz_engine::is_embedded(),
            "a rack was built without telling the engine it is a guest here"
        );
        unsafe { plugin_deactivate(plugin) };
        unsafe { plugin_destroy(plugin) };
    }

    /// The editor the host opens: an X11 window, a grid in it, and choz's own
    /// panels drawn on that grid. Skipped where there is no display, which is
    /// every build machine.
    #[test]
    fn the_host_gets_an_x11_window_with_the_panels_drawn_in_it() {
        let _g = guard();
        if std::env::var_os("DISPLAY").is_none() {
            return;
        }
        let plugin = unsafe { open() };
        assert!(unsafe { plugin_init(plugin) });
        assert!(unsafe { plugin_activate(plugin, 48_000.0, 32, 128) });

        let gui = unsafe { plugin_get_extension(plugin, CLAP_EXT_GUI.as_ptr()) }
            as *const clap_plugin_gui;
        assert!(!gui.is_null(), "no gui extension");

        let supported = unsafe { (*gui).is_api_supported }.unwrap();
        assert!(
            unsafe { supported(plugin, CLAP_WINDOW_API_X11.as_ptr(), false) },
            "embedded X11 is the one thing this window is"
        );
        assert!(
            !unsafe { supported(plugin, CLAP_WINDOW_API_X11.as_ptr(), true) },
            "and it never floats"
        );

        let create = unsafe { (*gui).create }.unwrap();
        assert!(
            unsafe { create(plugin, CLAP_WINDOW_API_X11.as_ptr(), false) },
            "the window would not open"
        );

        let get_size = unsafe { (*gui).get_size }.unwrap();
        let (mut w, mut h) = (0u32, 0u32);
        assert!(unsafe { get_size(plugin, &mut w, &mut h) });
        assert!(w > 0 && h > 0, "an empty window: {w}x{h}");

        // A size the host picks comes back rounded to whole cells — half a row
        // of characters is a row that is not drawn.
        let adjust = unsafe { (*gui).adjust_size }.unwrap();
        let (mut aw, mut ah) = (w + 3, h + 3);
        assert!(unsafe { adjust(plugin, &mut aw, &mut ah) });
        assert!(aw >= w && ah >= h, "adjusted below the grid: {aw}x{ah}");

        // And the panels really are on the grid: choz's name is in the menu
        // bar of every frame it draws.
        unsafe { plugin_on_timer(plugin, 0) };
        let inst = unsafe { Instance::of(plugin) }.unwrap();
        let terminal = inst.terminal.as_ref().expect("a grid was made");
        let drawn: String = terminal
            .backend()
            .cells
            .iter()
            .map(|c| c.symbol.as_str())
            .collect();
        assert!(
            drawn.trim().chars().any(|c| !c.is_whitespace()),
            "the window was painted with nothing at all"
        );

        let destroy = unsafe { (*gui).destroy }.unwrap();
        unsafe { destroy(plugin) };
        unsafe { plugin_deactivate(plugin) };
        unsafe { plugin_destroy(plugin) };
    }

    /// The rack travels with the host's session. Saved before the plugin is
    /// ever activated it is nothing, but once there is a rack the state is the
    /// same YAML a project file holds — and loading it back is what makes
    /// opening a session open the rack.
    #[test]
    fn the_rack_is_saved_and_restored_as_the_hosts_state() {
        let _g = guard();
        let plugin = unsafe { open() };
        assert!(unsafe { plugin_init(plugin) });
        assert!(unsafe { plugin_activate(plugin, 48_000.0, 32, 128) });

        // A host's stream, as the two function pointers the ABI is.
        let saved = Box::into_raw(Box::new(Vec::<u8>::new()));
        unsafe extern "C" fn write(
            stream: *const clap_ostream,
            buffer: *const c_void,
            size: u64,
        ) -> i64 {
            let sink = unsafe { (*stream).ctx } as *mut Vec<u8>;
            let bytes = unsafe { std::slice::from_raw_parts(buffer as *const u8, size as usize) };
            unsafe { (*sink).extend_from_slice(bytes) };
            size as i64
        }
        let ostream = clap_ostream {
            ctx: saved as *mut c_void,
            write: Some(write),
        };
        let state = unsafe { plugin_get_extension(plugin, CLAP_EXT_STATE.as_ptr()) }
            as *const clap_plugin_state;
        assert!(!state.is_null(), "no state extension");
        let save = unsafe { (*state).save }.unwrap();
        assert!(unsafe { save(plugin, &ostream) });

        let yaml = String::from_utf8(unsafe { *Box::from_raw(saved) }).expect("state is text");
        assert!(
            yaml.contains("rack:"),
            "that is not a choz project: {}",
            &yaml[..yaml.len().min(120)]
        );

        // And back in, on a second instance that has never been activated:
        // a host loads state before it starts the plugin as often as after.
        let other = unsafe { open() };
        assert!(unsafe { plugin_init(other) });
        struct Source {
            bytes: Vec<u8>,
            read: usize,
        }
        unsafe extern "C" fn read(
            stream: *const clap_istream,
            buffer: *mut c_void,
            size: u64,
        ) -> i64 {
            let source = unsafe { (*stream).ctx } as *mut Source;
            let left = unsafe { (*source).bytes.len() - (*source).read };
            let n = left.min(size as usize);
            unsafe {
                std::ptr::copy_nonoverlapping(
                    (*source).bytes.as_ptr().add((*source).read),
                    buffer as *mut u8,
                    n,
                );
                (*source).read += n;
            }
            n as i64
        }
        let source = Box::into_raw(Box::new(Source {
            bytes: yaml.into_bytes(),
            read: 0,
        }));
        let istream = clap_istream {
            ctx: source as *mut c_void,
            read: Some(read),
        };
        let load = unsafe { (*state).load }.unwrap();
        assert!(unsafe { load(other, &istream) });
        // Activating now builds the rack out of what the host handed over.
        assert!(unsafe { plugin_activate(other, 48_000.0, 32, 128) });

        drop(unsafe { Box::from_raw(source) });
        unsafe { plugin_deactivate(other) };
        unsafe { plugin_destroy(other) };
        unsafe { plugin_deactivate(plugin) };
        unsafe { plugin_destroy(plugin) };
    }
}

/// The built `.clap` loaded the way a DAW loads it — `dlopen`, the entry
/// symbol, the factory — rather than through this crate's own functions.
///
/// The ABI tests above prove the plugin answers correctly; this one proves the
/// *bundle* does, which is the part a host actually sees. It is a separate file
/// because it needs the shared object to exist, and that only happens after a
/// build of this crate's `cdylib`.
#[cfg(test)]
mod bundle {
    use std::path::PathBuf;

    /// Where cargo left the shared object for this build profile.
    fn bundle() -> Option<PathBuf> {
        // `target/<profile>/deps/<test binary>` — the bundle is two up.
        let exe = std::env::current_exe().ok()?;
        let path = exe.parent()?.parent()?.join("libchoz_clap.so");
        path.exists().then_some(path)
    }

    /// A host reading the bundle finds one instrument, named choz.
    #[test]
    fn a_host_scanning_the_bundle_finds_choz() {
        let Some(path) = bundle() else {
            // The `cdylib` is not built when only the test target is; that is a
            // build that has nothing to scan, not a failure.
            return;
        };
        let found = choz_plugin_clap::describe(&path);
        assert_eq!(found.len(), 1, "one plugin in the bundle: {found:?}");
        assert_eq!(found[0].id, super::PLUGIN_ID.trim_end_matches('\0'));
        assert_eq!(found[0].name, "choz");
    }
}
