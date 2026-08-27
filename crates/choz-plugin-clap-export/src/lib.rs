//! choz's own effects and artifacts, published as CLAP plugins.
//!
//! The other direction from `choz-plugin-clap`: instead of choz loading
//! somebody else's plugin, this lets somebody else's host — Bitwig, Reaper,
//! Carla — load choz's effects *and* its two note generators. One `.clap` file
//! publishes all of them, because a CLAP factory answers with as many plugins
//! as it likes.
//!
//! # Two kinds of plugin
//!
//! **The effects** are audio in, audio out, with the dry/wet on the end — see
//! [`Sort::Fx`].
//!
//! **The artifacts** — the arpeggiator and the step sequencer — are note
//! effects: no audio ports at all, note ports instead. They were the harder
//! half of this crate's job and the reason they lived in the interface for so
//! long: a note is not audio, so neither has a `process_block` to be. What they
//! do have is a `tick` against choz's clock, and out here that clock is the
//! host's transport.
//!
//! # What travels and what does not
//!
//! **The DSP travels.** [`choz_ports::FxProcessor`] is already the shape of a
//! plugin — `process_block`, `params`, `set_param`, `reset` — and the effects
//! are built from an array of normalised `f32` and know nothing about racks or
//! tabs. That is why this crate is small. The generators travel for the same
//! reason: `choz_engine::arp` and `choz_engine::seq` are settings and a clock,
//! with no interface in them.
//!
//! **The panel does not.** The meter, the presets, the drawn EQ curve and the
//! waveshaper's point bank belong to choz's interface. An exported effect is
//! the algorithm, with the host's generic parameter view on top. Building it a
//! window of its own is a different project.
//!
//! # Written against the raw ABI
//!
//! `clap-sys`, the same crate `clack-host` builds on, rather than a plugin
//! framework. The entry point, one factory and two extensions is the whole
//! surface, and it is the same trade the LV2 side already made.
//!
//! # Installing
//!
//! ```bash
//! cargo build --release -p choz-plugin-clap-export
//! cp target/release/libchoz_plugin_clap_export.so ~/.clap/choz.clap
//! ```

use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::OnceLock;

use choz_ports::{FxParam, FxProcessor};

use clap_sys::entry::clap_plugin_entry;
use clap_sys::events::{
    clap_event_header, clap_event_note, clap_event_param_value, CLAP_CORE_EVENT_SPACE_ID,
    CLAP_EVENT_NOTE_OFF, CLAP_EVENT_NOTE_ON, CLAP_EVENT_PARAM_VALUE,
};
use clap_sys::ext::audio_ports::{
    clap_audio_port_info, clap_plugin_audio_ports, CLAP_AUDIO_PORT_IS_MAIN, CLAP_EXT_AUDIO_PORTS,
    CLAP_PORT_STEREO,
};
use clap_sys::ext::note_ports::{
    clap_note_port_info, clap_plugin_note_ports, CLAP_EXT_NOTE_PORTS, CLAP_NOTE_DIALECT_CLAP,
    CLAP_NOTE_DIALECT_MIDI,
};
use clap_sys::ext::params::{
    clap_param_info, clap_plugin_params, CLAP_EXT_PARAMS, CLAP_PARAM_IS_AUTOMATABLE,
};
use clap_sys::factory::plugin_factory::{clap_plugin_factory, CLAP_PLUGIN_FACTORY_ID};
use clap_sys::host::clap_host;
use clap_sys::plugin::{clap_plugin, clap_plugin_descriptor};
use clap_sys::process::{clap_process, clap_process_status, CLAP_PROCESS_CONTINUE};
use clap_sys::version::CLAP_VERSION;

/// Interleaved stereo, which is what every choz effect processes.
const CHANNELS: usize = 2;

/// The dry/wet, which in choz belongs to the chain rather than to the effect.
/// Exported as the last parameter so a host can reach it: outside choz there is
/// no chain to hold it.
const MIX_NAME: &str = "Mix";

// ─── The catalogue ──────────────────────────────────────────────────────────

/// Which of the two kinds of thing choz publishes an exported plugin is.
///
/// The effects were here first and are the reason the crate is small: an
/// `FxProcessor` is already the shape of a plugin. The two **artifacts** are
/// not effects at all — a note is not audio, so they have no `process_block`
/// to be — and they are here because a generator nobody outside choz can load
/// is a generator that only exists inside one program.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Sort {
    /// A built-in effect, by the id [`choz_engine::fx_chain::build_processor`]
    /// answers to. Audio in, audio out, and a dry/wet on the end.
    Fx(&'static str),
    /// The arpeggiator: notes in, notes out.
    Arp,
    /// The step sequencer: nothing in, notes out.
    Seq,
}

impl Sort {
    fn is_generator(self) -> bool {
        !matches!(self, Sort::Fx(_))
    }
}

/// One exported plugin: what the host is told about it, and what its knobs are.
struct Exported {
    sort: Sort,
    /// Its knobs, in order. For an effect these are what the processor itself
    /// reports and the dry/wet is **not** among them — it is appended, and it
    /// is the only parameter this crate invents. A generator's list is the
    /// whole of it.
    params: Vec<FxParam>,
    descriptor: clap_plugin_descriptor,
}

// SAFETY: the descriptor's pointers are into leaked `CString`s, which live for
// the process. Nothing here is ever mutated after `catalogue()` builds it.
unsafe impl Send for Exported {}
unsafe impl Sync for Exported {}

/// A string the descriptor can point at forever. Leaked on purpose: it is a
/// handful of bytes per effect, once per process, and the alternative is a
/// self-referential struct with the same lifetime and more ways to be wrong.
fn forever(s: &str) -> *const c_char {
    let owned = CString::new(s).unwrap_or_default();
    Box::leak(owned.into_boxed_c_str()).as_ptr()
}

/// The same, for a parameter name: [`FxParam`] holds a `&'static str`, and the
/// sequencer's are generated — one per cell of its grid.
fn forever_str(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// A descriptor, filled in the same way for everything this crate publishes.
fn describe(id: &str, name: &str, blurb: &str, features: &[&str]) -> clap_plugin_descriptor {
    let mut list: Vec<*const c_char> = features.iter().map(|f| forever(f)).collect();
    list.push(std::ptr::null());
    let features: &'static [*const c_char] = Box::leak(list.into_boxed_slice());
    clap_plugin_descriptor {
        clap_version: CLAP_VERSION,
        id: forever(id),
        name: forever(name),
        vendor: forever("choz"),
        url: forever("https://github.com/jcodelia/choz"),
        manual_url: std::ptr::null(),
        support_url: std::ptr::null(),
        version: forever(env!("CARGO_PKG_VERSION")),
        description: forever(blurb),
        features: features.as_ptr(),
    }
}

/// Every built-in effect, described once.
///
/// Each one is instantiated at a nominal rate to ask it its name and its
/// parameters — the same question the interface asks — and then dropped. It
/// happens once, when the host first opens the bundle.
fn catalogue() -> &'static [Exported] {
    static CATALOGUE: OnceLock<Vec<Exported>> = OnceLock::new();
    CATALOGUE.get_or_init(|| {
        // The mid-range defaults the rack starts an effect at, so what a host
        // sees is what choz shows on adding one.
        let defaults = [0.5f32; 16];
        let mut out: Vec<Exported> = choz_engine::fx_chain::BUILT_IN_KINDS
            .iter()
            .filter_map(|&(kind, name)| {
                let proc = choz_engine::fx_chain::build_processor(kind, &defaults, 48_000)?;
                Some(Exported {
                    sort: Sort::Fx(kind),
                    params: proc.params(),
                    descriptor: describe(
                        &format!("org.choz.fx.{kind}"),
                        // The catalogue's name, not `proc.name()`: half the
                        // effects never override it, and "FX" is not a plugin
                        // anybody can find in a list of four hundred.
                        name,
                        "A choz effect, outside choz",
                        &["audio-effect", "stereo"],
                    ),
                })
            })
            .collect();
        // The two artifacts. `note-effect` for both: CLAP has no separate
        // feature for a sequencer, and what a host needs to know is that these
        // speak notes and not audio.
        out.push(Exported {
            sort: Sort::Arp,
            params: arp_params(),
            descriptor: describe(
                "org.choz.gen.arp",
                "choz Arpeggiator",
                "choz's arpeggiator, outside choz",
                &["note-effect"],
            ),
        });
        out.push(Exported {
            sort: Sort::Seq,
            params: seq_params(),
            descriptor: describe(
                "org.choz.gen.seq",
                "choz Sequencer",
                "choz's step sequencer, outside choz",
                &["note-effect"],
            ),
        });
        out
    })
}

/// How many knobs a host sees: an effect's own plus the dry/wet, and a
/// generator's exactly as listed — there is no chain out here to hold a mix,
/// and a note has no wet.
fn param_count(e: &Exported) -> usize {
    match e.sort {
        Sort::Fx(_) => e.params.len() + 1,
        _ => e.params.len(),
    }
}

// ─── The artifacts ──────────────────────────────────────────────────────────

use choz_engine::arp::{Arp, ArpEvent, ArpParam, TimeDiv};
use choz_engine::seq::{Seq, PARTS, STEPS, TRACKS};

/// An arpeggiator as the exported plugin starts: choz's defaults, but **on**.
///
/// On matters more than it looks. `ARP` is one of its own knobs, so the default
/// this reports is also the value a host hands back on the first block — and
/// with choz's `on: false` the plugin switched itself off the moment it was
/// built from its own defaults. A plugin that does nothing until a knob is
/// found reads as a plugin that does not work.
fn arp_defaults() -> Arp {
    let mut arp = Arp::default();
    arp.settings.on = true;
    arp
}

/// The arpeggiator's knobs are the ones its own panel draws — the box already
/// describes itself for the interface, and a host asking the same question
/// deserves the same answer.
fn arp_params() -> Vec<FxParam> {
    arp_defaults()
        .settings
        .knob_list()
        .into_iter()
        .map(|(_, name, value, _)| FxParam::new(name, value, 0.0, 1.0, ""))
        .collect()
}

/// The arpeggiator's controls, in the order [`arp_params`] lists them.
fn arp_param_at(index: usize) -> Option<ArpParam> {
    arp_defaults()
        .settings
        .knob_list()
        .get(index)
        .map(|(p, ..)| *p)
}

/// Where the sequencer's grid ends and its lanes begin.
const SEQ_GRID: usize = TRACKS * STEPS;
/// …and where the lanes end and the four controls begin.
const SEQ_LANES: usize = SEQ_GRID + TRACKS;

/// The sequencer, as a host can reach it: every cell of the grid, the note each
/// lane plays, and the four controls.
///
/// **The grid is parameters and not state** because this crate publishes no
/// `clap.state`: a host saves parameter values, and a sequencer whose pattern
/// did not survive reopening the project would be a demonstration rather than
/// an instrument. A hundred and forty knobs is a lot for a generic panel; a
/// pattern that cannot be written is worse.
///
/// ponytail: one part, not the eight choz keeps. Parts and the song chain are a
/// workflow rather than a sound, and eight of them would be a thousand
/// parameters. The upgrade path is `clap.state` — which would take the grid out
/// of the parameter list entirely.
fn seq_params() -> Vec<FxParam> {
    let mut out = Vec::with_capacity(SEQ_LANES + 4);
    for track in 0..TRACKS {
        let lane = choz_engine::seq::track_name(track);
        for step in 0..STEPS {
            out.push(FxParam::new(
                forever_str(format!("{lane}{}", step + 1)),
                0.0,
                0.0,
                1.0,
                "",
            ));
        }
    }
    let defaults = choz_engine::seq::SeqSettings::default();
    for track in 0..TRACKS {
        out.push(FxParam::new(
            forever_str(format!("{} Note", choz_engine::seq::track_name(track))),
            defaults.notes[track] as f32 / 127.0,
            0.0,
            127.0,
            "",
        ));
    }
    out.push(FxParam::new("Div", 0.0, 0.0, 1.0, ""));
    out.push(FxParam::new("Swing", 0.0, 0.0, 1.0, ""));
    out.push(FxParam::new("Rand", 0.0, 0.0, 1.0, ""));
    out.push(FxParam::new("Prob", 0.0, 0.0, 1.0, ""));
    out
}

/// A running artifact. Built at `activate` beside the effects' processor, and
/// for the same reason: the host says the sample rate then and not before.
enum Generator {
    Arp(Box<Arp>),
    Seq(Box<Seq>),
}

impl Generator {
    /// Start one from the knob positions the host has been setting all along.
    fn build(sort: Sort, values: &[f32]) -> Option<Generator> {
        let mut gen = match sort {
            Sort::Arp => Generator::Arp(Box::new(arp_defaults())),
            Sort::Seq => {
                let mut seq = Seq::default();
                seq.settings.on = true;
                Generator::Seq(Box::new(seq))
            }
            Sort::Fx(_) => return None,
        };
        for (i, v) in values.iter().copied().enumerate() {
            gen.apply(i, v);
        }
        if let Generator::Seq(seq) = &mut gen {
            // Rolling: outside choz there is no PLAY button on the box, and the
            // transport is what a host offers instead. `tick` still counts the
            // host's grid whenever it is moving.
            seq.play();
        }
        Some(gen)
    }

    /// One knob, as the host's 0..1.
    fn apply(&mut self, index: usize, value: f32) {
        match self {
            Generator::Arp(arp) => {
                if let Some(param) = arp_param_at(index) {
                    // It answers whether the play mode moved, which matters to
                    // the panel that has a chord to drop; out here nothing is
                    // held between blocks that a mode change invalidates.
                    let _ = arp.settings.set_norm(param, value);
                }
            }
            Generator::Seq(seq) => {
                let s = &mut seq.settings;
                match index {
                    // The grid: a cell is on above half, which is what a host's
                    // generic control lands on either side of.
                    i if i < SEQ_GRID => {
                        let (track, step) = (i / STEPS, i % STEPS);
                        let bit = 1u16 << step;
                        let part = s.part.min(PARTS - 1);
                        match value >= 0.5 {
                            true => s.parts[part][track] |= bit,
                            false => s.parts[part][track] &= !bit,
                        }
                    }
                    i if i < SEQ_LANES => {
                        s.notes[i - SEQ_GRID] = (value * 127.0).round().clamp(0.0, 127.0) as u8
                    }
                    // The divisions are not evenly spaced in time, but they are
                    // a list, and a knob across a list is how choz's own panel
                    // reaches them too.
                    i if i == SEQ_LANES => {
                        let n = TimeDiv::ALL.len();
                        let k = ((value * (n - 1) as f32).round() as usize).min(n - 1);
                        s.div = TimeDiv::ALL[k];
                    }
                    i if i == SEQ_LANES + 1 => {
                        s.swing = value.clamp(0.0, 1.0) * choz_engine::seq::MAX_SWING
                    }
                    i if i == SEQ_LANES + 2 => s.random = value.clamp(0.0, 1.0),
                    i if i == SEQ_LANES + 3 => s.prob = value.clamp(0.0, 1.0),
                    _ => {}
                }
            }
        }
    }

    /// A note the host sent in. Only the arpeggiator has anything to do with
    /// one — the sequencer plays what is written on it.
    fn note_on(&mut self, note: u8, vel: u8, now: std::time::Instant) {
        if let Generator::Arp(arp) = self {
            arp.note_on(note, vel, now);
        }
    }

    fn note_off(&mut self, note: u8) {
        if let Generator::Arp(arp) = self {
            arp.note_off(note);
        }
    }

    fn tick(&mut self, now: std::time::Instant, out: &mut Vec<ArpEvent>) {
        match self {
            Generator::Arp(arp) => arp.tick(now, out),
            Generator::Seq(seq) => seq.tick(now, out),
        }
    }
}

// ─── One running instance ───────────────────────────────────────────────────

/// The plugin handed to the host. `plugin` is first so the two can be cast into
/// each other, which is the usual C idiom and what `plugin_data` is for anyway.
#[repr(C)]
struct Instance {
    plugin: clap_plugin,
    index: usize,
    /// Built at `activate`, when the sample rate is finally known. `None`
    /// between `deactivate` and the next `activate`, and always `None` for an
    /// artifact — which has [`Instance::generator`] instead.
    processor: Option<Box<dyn FxProcessor>>,
    /// The same, for the two note generators.
    generator: Option<Generator>,
    /// What the generator asked for this block. Kept here so the audio thread
    /// never allocates one.
    notes: Vec<ArpEvent>,
    /// Knob positions, 0..1, in parameter order with the dry/wet last. Kept
    /// here rather than asked of the processor: a host may set a parameter
    /// while the plugin is deactivated, and that value has to survive.
    values: Vec<f32>,
    sample_rate: u32,
    /// Interleaved scratch, sized at `activate`. The audio thread never
    /// allocates.
    scratch: Vec<f32>,
}

impl Instance {
    fn exported(&self) -> &'static Exported {
        &catalogue()[self.index]
    }

    /// Apply a knob position, whichever side of the boundary it came from.
    fn apply(&mut self, param: usize, value: f32) {
        let value = value.clamp(0.0, 1.0);
        let Some(slot) = self.values.get_mut(param) else {
            return;
        };
        *slot = value;
        if let Some(gen) = self.generator.as_mut() {
            gen.apply(param, value);
            return;
        }
        let mix = self.values.len() - 1;
        if let Some(proc) = self.processor.as_mut() {
            if param == mix {
                proc.set_mix(value);
            } else {
                proc.set_param(param, value);
            }
        }
    }

    /// Everything the host queued for this block. Sample-accurate automation is
    /// deliberately flattened: choz's own effects take a parameter as "from
    /// now on", so honouring the timestamp would mean splitting every block for
    /// a difference nothing here can hear.
    ///
    /// # Safety
    /// `events` must be the list the host handed to `process`/`flush`.
    unsafe fn drain_events(&mut self, events: *const clap_sys::events::clap_input_events) {
        if events.is_null() {
            return;
        }
        let list = unsafe { &*events };
        let (Some(size), Some(get)) = (list.size, list.get) else {
            return;
        };
        for i in 0..unsafe { size(events) } {
            let header = unsafe { get(events, i) };
            if header.is_null() {
                continue;
            }
            let header: &clap_event_header = unsafe { &*header };
            if header.space_id != CLAP_CORE_EVENT_SPACE_ID {
                continue;
            }
            match header.type_ {
                CLAP_EVENT_PARAM_VALUE => {
                    let event: &clap_event_param_value = unsafe {
                        &*(header as *const clap_event_header as *const clap_event_param_value)
                    };
                    self.apply(event.param_id as usize, event.value as f32);
                }
                // Keys the host is playing into an artifact. `key` is -1 for
                // "every note", which only note-off ever sends and which the
                // arpeggiator answers to by letting go of what it holds.
                CLAP_EVENT_NOTE_ON | CLAP_EVENT_NOTE_OFF => {
                    let Some(gen) = self.generator.as_mut() else {
                        continue;
                    };
                    let event: &clap_event_note =
                        unsafe { &*(header as *const clap_event_header as *const clap_event_note) };
                    if !(0..=127).contains(&event.key) {
                        continue;
                    }
                    let note = event.key as u8;
                    if header.type_ == CLAP_EVENT_NOTE_ON {
                        let vel = (event.velocity * 127.0).round().clamp(1.0, 127.0) as u8;
                        gen.note_on(note, vel, std::time::Instant::now());
                    } else {
                        gen.note_off(note);
                    }
                }
                _ => {}
            }
        }
    }
}

/// The instance behind a `*const clap_plugin`.
///
/// # Safety
/// `plugin` must be one this crate created and not yet destroyed.
unsafe fn instance<'a>(plugin: *const clap_plugin) -> Option<&'a mut Instance> {
    if plugin.is_null() {
        return None;
    }
    let data = unsafe { (*plugin).plugin_data } as *mut Instance;
    (!data.is_null()).then(|| unsafe { &mut *data })
}

// ─── The plugin vtable ──────────────────────────────────────────────────────

unsafe extern "C" fn plugin_init(_plugin: *const clap_plugin) -> bool {
    true
}

unsafe extern "C" fn plugin_destroy(plugin: *const clap_plugin) {
    if plugin.is_null() {
        return;
    }
    let data = unsafe { (*plugin).plugin_data } as *mut Instance;
    if !data.is_null() {
        // Back into a Box, and dropped here — which is also where the effect's
        // buffers go.
        drop(unsafe { Box::from_raw(data) });
    }
}

unsafe extern "C" fn plugin_activate(
    plugin: *const clap_plugin,
    sample_rate: f64,
    _min_frames: u32,
    max_frames: u32,
) -> bool {
    let Some(inst) = (unsafe { instance(plugin) }) else {
        return false;
    };
    inst.sample_rate = sample_rate.max(1.0) as u32;
    // The clock converts the host's beats into frames, so it has to know how
    // long a frame is before the first block arrives.
    choz_ports::transport().set_sample_rate(inst.sample_rate);
    let sort = inst.exported().sort;
    // An artifact has no audio to process: it is built from the same knob
    // positions and then handed the host's clock, block by block.
    if sort.is_generator() {
        inst.generator = Generator::build(sort, &inst.values);
        return inst.generator.is_some();
    }
    let Sort::Fx(kind) = sort else { return false };
    // The processor is built from the knob positions the host has been setting
    // all along, so activating twice does not reset the sound.
    let Some(mut proc) =
        choz_engine::fx_chain::build_processor(kind, &inst.values, inst.sample_rate)
    else {
        return false;
    };
    // `build_processor` reads the array once; anything the host moved since is
    // applied on top, and the dry/wet is never in that array.
    for (i, v) in inst.values.iter().copied().enumerate() {
        if i + 1 == inst.values.len() {
            proc.set_mix(v);
        } else {
            proc.set_param(i, v);
        }
    }
    inst.processor = Some(proc);
    inst.scratch = vec![0.0; max_frames.max(1) as usize * CHANNELS];
    true
}

unsafe extern "C" fn plugin_deactivate(plugin: *const clap_plugin) {
    if let Some(inst) = unsafe { instance(plugin) } {
        inst.processor = None;
        inst.generator = None;
    }
}

unsafe extern "C" fn plugin_start_processing(_plugin: *const clap_plugin) -> bool {
    true
}

unsafe extern "C" fn plugin_stop_processing(_plugin: *const clap_plugin) {}

unsafe extern "C" fn plugin_reset(plugin: *const clap_plugin) {
    if let Some(inst) = unsafe { instance(plugin) } {
        if let Some(proc) = inst.processor.as_mut() {
            proc.reset();
        }
    }
}

unsafe extern "C" fn plugin_process(
    plugin: *const clap_plugin,
    process: *const clap_process,
) -> clap_process_status {
    let Some(inst) = (unsafe { instance(plugin) }) else {
        return clap_sys::process::CLAP_PROCESS_ERROR;
    };
    if process.is_null() {
        return clap_sys::process::CLAP_PROCESS_ERROR;
    }
    let p = unsafe { &*process };
    unsafe { inst.drain_events(p.in_events) };
    let frames = p.frames_count as usize;
    unsafe { follow_host_transport(p.transport, frames) };

    if inst.generator.is_some() {
        return unsafe { generate(inst, p, frames) };
    }
    if frames == 0 || p.audio_outputs_count == 0 || p.audio_outputs.is_null() {
        return CLAP_PROCESS_CONTINUE;
    }
    let out = unsafe { &*p.audio_outputs };
    if out.data32.is_null() || out.channel_count == 0 {
        return CLAP_PROCESS_CONTINUE;
    }
    // A host may hand over mono (one channel) or a shared in-place buffer.
    // Whatever it gives, choz works in interleaved stereo, so the block is
    // gathered, processed and written back in that shape.
    let out_channels = out.channel_count as usize;
    let inputs = (p.audio_inputs_count > 0 && !p.audio_inputs.is_null())
        .then(|| unsafe { &*p.audio_inputs })
        .filter(|b| !b.data32.is_null() && b.channel_count > 0);

    let need = frames * CHANNELS;
    if inst.scratch.len() < need {
        // A host that renders a longer block than it declared. Growing here is
        // an allocation on the audio thread, which is exactly what must not
        // happen, so the block is refused instead.
        return clap_sys::process::CLAP_PROCESS_ERROR;
    }
    for frame in 0..frames {
        for ch in 0..CHANNELS {
            let value = match inputs {
                Some(buf) => {
                    let src = ch.min(buf.channel_count as usize - 1);
                    unsafe { *(*buf.data32.add(src)).add(frame) }
                }
                // No input port connected: an effect with nothing to process.
                None => 0.0,
            };
            inst.scratch[frame * CHANNELS + ch] = value;
        }
    }

    let sr = inst.sample_rate;
    if let Some(proc) = inst.processor.as_mut() {
        proc.process_block(&mut inst.scratch[..need], sr);
    }

    for frame in 0..frames {
        for ch in 0..out_channels {
            let value = inst.scratch[frame * CHANNELS + ch.min(CHANNELS - 1)];
            unsafe { *(*out.data32.add(ch)).add(frame) = value };
        }
    }
    CLAP_PROCESS_CONTINUE
}

/// Run an artifact for one block and hand the host what it played.
///
/// The generators keep their own clock — the interface's event loop drives them
/// inside choz — and follow the transport whenever one is rolling, which in a
/// host it always is. So the work here is the two translations: the host's
/// timeline into choz's (already done by `follow_host_transport`), and
/// [`ArpEvent`] into the host's note events.
///
/// # Safety
/// `p` is the block the host handed to `process`, and `inst` has a generator.
unsafe fn generate(inst: &mut Instance, p: &clap_process, frames: usize) -> clap_process_status {
    inst.notes.clear();
    let now = std::time::Instant::now();
    if let Some(gen) = inst.generator.as_mut() {
        gen.tick(now, &mut inst.notes);
    }
    if inst.notes.is_empty() || p.out_events.is_null() {
        return CLAP_PROCESS_CONTINUE;
    }
    // Where this block starts on the timeline, so an event scheduled against
    // the grid lands on the frame it was scheduled for. Everything from the
    // free-running clock carries `at: 0`, which is "now" — the top of the
    // block, which is as accurate as a clock with no timeline can be.
    let block_start = choz_ports::transport().samples();
    let last = frames.saturating_sub(1) as u32;
    for event in inst.notes.iter().copied() {
        let (type_, key, velocity, at) = match event {
            ArpEvent::On { note, vel, at } => (CLAP_EVENT_NOTE_ON, note, vel as f64 / 127.0, at),
            ArpEvent::Off { note, at } => (CLAP_EVENT_NOTE_OFF, note, 0.0, at),
        };
        let time = match at {
            0 => 0,
            at => (at.saturating_sub(block_start) as u32).min(last),
        };
        let note = clap_event_note {
            header: clap_event_header {
                size: std::mem::size_of::<clap_event_note>() as u32,
                time,
                space_id: CLAP_CORE_EVENT_SPACE_ID,
                type_,
                flags: 0,
            },
            // -1 is CLAP's "not one the host is tracking": choz's generators
            // number nothing, and a note id invented here would be one the
            // note-off could not match.
            note_id: -1,
            port_index: 0,
            // Everything, because a note effect that answered on one channel
            // would be silent wherever the host was sending on another.
            channel: 0,
            key: key as i16,
            velocity,
        };
        unsafe {
            let list = &*p.out_events;
            if let Some(push) = list.try_push {
                push(p.out_events, &note.header);
            }
        }
    }
    CLAP_PROCESS_CONTINUE
}

/// Point choz's clock at the host's.
///
/// The effects that care about time — `BeatRepeat` above all — read
/// [`choz_ports::transport`], which inside choz is driven by the audio
/// callback. Out here the DAW owns the timeline, and CLAP hands it over on
/// every block, so this is where the two are joined. A host that offers no
/// transport gets the clock choz would keep for itself: it advances by the
/// block, which at least makes a tempo-synced effect run rather than freeze.
///
/// The clock is global to the **process**, so several choz plugins in one
/// project all write it — with the same values from the same host, which is
/// why that is harmless rather than a race worth fixing.
///
/// # Safety
/// `transport` is the pointer the host put in `clap_process`.
unsafe fn follow_host_transport(
    transport: *const clap_sys::events::clap_event_transport,
    frames: usize,
) {
    use clap_sys::events::{
        CLAP_TRANSPORT_HAS_BEATS_TIMELINE, CLAP_TRANSPORT_HAS_TEMPO,
        CLAP_TRANSPORT_HAS_TIME_SIGNATURE, CLAP_TRANSPORT_IS_PLAYING,
    };
    let clock = choz_ports::transport();
    if transport.is_null() {
        clock.advance(frames);
        return;
    }
    let t = unsafe { &*transport };
    if t.flags & CLAP_TRANSPORT_HAS_TEMPO != 0 {
        clock.set_bpm(t.tempo as f32);
    }
    if t.flags & CLAP_TRANSPORT_HAS_TIME_SIGNATURE != 0 {
        clock.set_time_signature(t.tsig_num, t.tsig_denom);
    }
    clock.set_playing(t.flags & CLAP_TRANSPORT_IS_PLAYING != 0);
    if t.flags & CLAP_TRANSPORT_HAS_BEATS_TIMELINE != 0 {
        // Fixed point, 1/2³¹ of a beat.
        clock.set_position_beats(
            t.song_pos_beats as f64 / clap_sys::fixedpoint::CLAP_BEATTIME_FACTOR as f64,
        );
    } else {
        clock.advance(frames);
    }
}

unsafe extern "C" fn plugin_get_extension(
    _plugin: *const clap_plugin,
    id: *const c_char,
) -> *const c_void {
    if id.is_null() {
        return std::ptr::null();
    }
    let id = unsafe { CStr::from_ptr(id) };
    if id == CLAP_EXT_PARAMS {
        return &PARAMS as *const _ as *const c_void;
    }
    // The two are exclusive on purpose: an effect has no note ports and an
    // artifact has no audio ones, and a plugin that claims both and then
    // reports zero of one is a plugin a host has to guess about.
    let generator = unsafe { instance(_plugin) }.is_some_and(|i| i.exported().sort.is_generator());
    if id == CLAP_EXT_AUDIO_PORTS && !generator {
        return &AUDIO_PORTS as *const _ as *const c_void;
    }
    if id == CLAP_EXT_NOTE_PORTS && generator {
        return &NOTE_PORTS as *const _ as *const c_void;
    }
    std::ptr::null()
}

unsafe extern "C" fn plugin_on_main_thread(_plugin: *const clap_plugin) {}

// ─── Audio ports ────────────────────────────────────────────────────────────

unsafe extern "C" fn ports_count(plugin: *const clap_plugin, _is_input: bool) -> u32 {
    match unsafe { instance(plugin) } {
        Some(inst) if inst.exported().sort.is_generator() => 0,
        _ => 1,
    }
}

unsafe extern "C" fn ports_get(
    _plugin: *const clap_plugin,
    index: u32,
    _is_input: bool,
    info: *mut clap_audio_port_info,
) -> bool {
    if index != 0 || info.is_null() {
        return false;
    }
    // `c_char` is `i8` on x86_64 and **`u8` on ARM**, so writing the array as
    // `[0i8; _]` compiles here and fails on a Raspberry Pi build. Let the
    // platform say what a char is.
    let mut name = [0 as c_char; clap_sys::string_sizes::CLAP_NAME_SIZE];
    for (slot, byte) in name.iter_mut().zip(b"Stereo\0".iter()) {
        *slot = *byte as c_char;
    }
    unsafe {
        *info = clap_audio_port_info {
            id: 0,
            name,
            flags: CLAP_AUDIO_PORT_IS_MAIN,
            channel_count: CHANNELS as u32,
            port_type: CLAP_PORT_STEREO.as_ptr(),
            // The same buffer may be used for input and output: every choz
            // effect processes in place, which is what this promises.
            in_place_pair: 0,
        };
    }
    true
}

static AUDIO_PORTS: clap_plugin_audio_ports = clap_plugin_audio_ports {
    count: Some(ports_count),
    get: Some(ports_get),
};

// ─── Note ports ─────────────────────────────────────────────────────────────

/// The arpeggiator takes keys and answers with keys; the sequencer plays what
/// is written on it and takes nothing.
unsafe extern "C" fn note_ports_count(plugin: *const clap_plugin, is_input: bool) -> u32 {
    let Some(inst) = (unsafe { instance(plugin) }) else {
        return 0;
    };
    match (inst.exported().sort, is_input) {
        (Sort::Arp, _) => 1,
        (Sort::Seq, false) => 1,
        _ => 0,
    }
}

unsafe extern "C" fn note_ports_get(
    plugin: *const clap_plugin,
    index: u32,
    is_input: bool,
    info: *mut clap_note_port_info,
) -> bool {
    if index != 0 || info.is_null() {
        return false;
    }
    if unsafe { note_ports_count(plugin, is_input) } == 0 {
        return false;
    }
    // `c_char` is `i8` on x86_64 and **`u8` on ARM** — the same reason the
    // audio port spells its name out a byte at a time.
    let mut name = [0 as c_char; clap_sys::string_sizes::CLAP_NAME_SIZE];
    let label: &[u8] = if is_input {
        b"Notes In\0"
    } else {
        b"Notes Out\0"
    };
    for (slot, byte) in name.iter_mut().zip(label.iter()) {
        *slot = *byte as c_char;
    }
    unsafe {
        *info = clap_note_port_info {
            id: 0,
            // MIDI as well as CLAP's own: choz speaks note numbers and
            // velocities, which both dialects carry, and a host with only MIDI
            // to offer is a host these should still work in.
            supported_dialects: CLAP_NOTE_DIALECT_CLAP | CLAP_NOTE_DIALECT_MIDI,
            preferred_dialect: CLAP_NOTE_DIALECT_CLAP,
            name,
        };
    }
    true
}

static NOTE_PORTS: clap_plugin_note_ports = clap_plugin_note_ports {
    count: Some(note_ports_count),
    get: Some(note_ports_get),
};

// ─── Parameters ─────────────────────────────────────────────────────────────

unsafe extern "C" fn params_count(plugin: *const clap_plugin) -> u32 {
    match unsafe { instance(plugin) } {
        Some(inst) => param_count(inst.exported()) as u32,
        None => 0,
    }
}

unsafe extern "C" fn params_get_info(
    plugin: *const clap_plugin,
    index: u32,
    info: *mut clap_param_info,
) -> bool {
    let Some(inst) = (unsafe { instance(plugin) }) else {
        return false;
    };
    if info.is_null() {
        return false;
    }
    let exported = inst.exported();
    let index = index as usize;
    let (label, default) = match exported.params.get(index) {
        Some(p) => (p.name, p.value),
        // Past the effect's own knobs is the dry/wet, and nothing past that.
        None if index == exported.params.len() => (MIX_NAME, 1.0),
        None => return false,
    };
    // `c_char` is `i8` on x86_64 and **`u8` on ARM**, so writing the array as
    // `[0i8; _]` compiles here and fails on a Raspberry Pi build. Let the
    // platform say what a char is.
    let mut name = [0 as c_char; clap_sys::string_sizes::CLAP_NAME_SIZE];
    for (slot, byte) in name.iter_mut().zip(label.as_bytes().iter()) {
        *slot = *byte as c_char;
    }
    unsafe {
        *info = clap_param_info {
            id: index as u32,
            flags: CLAP_PARAM_IS_AUTOMATABLE,
            cookie: std::ptr::null_mut(),
            name,
            module: [0; clap_sys::string_sizes::CLAP_PATH_SIZE],
            // Every choz knob is a 0..1 position; what that position *means* is
            // what `value_to_text` spells out, in the effect's own units.
            min_value: 0.0,
            max_value: 1.0,
            default_value: default as f64,
        };
    }
    true
}

unsafe extern "C" fn params_get_value(
    plugin: *const clap_plugin,
    param_id: u32,
    out: *mut f64,
) -> bool {
    let Some(inst) = (unsafe { instance(plugin) }) else {
        return false;
    };
    match inst.values.get(param_id as usize) {
        Some(v) if !out.is_null() => {
            unsafe { *out = *v as f64 };
            true
        }
        _ => false,
    }
}

unsafe extern "C" fn params_value_to_text(
    plugin: *const clap_plugin,
    param_id: u32,
    value: f64,
    buffer: *mut c_char,
    capacity: u32,
) -> bool {
    let Some(inst) = (unsafe { instance(plugin) }) else {
        return false;
    };
    if buffer.is_null() || capacity == 0 {
        return false;
    }
    let exported = inst.exported();
    let text = match exported.params.get(param_id as usize) {
        // The knob's real range is what the effect reports, so a host shows
        // "480 ms" rather than "0.24".
        Some(p) => {
            let real = p.min as f64 + value * (p.max - p.min) as f64;
            match p.unit.is_empty() {
                true => format!("{real:.2}"),
                false => format!("{real:.2} {}", p.unit),
            }
        }
        None if param_id as usize == exported.params.len() => format!("{:.0} %", value * 100.0),
        None => return false,
    };
    let Ok(text) = CString::new(text) else {
        return false;
    };
    let bytes = text.as_bytes_with_nul();
    let room = (capacity as usize).min(bytes.len());
    if room == 0 {
        return false;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr() as *const c_char, buffer, room);
        // Truncated or not, it ends where the host can find the end.
        *buffer.add(room - 1) = 0;
    }
    true
}

unsafe extern "C" fn params_text_to_value(
    _plugin: *const clap_plugin,
    _param_id: u32,
    _text: *const c_char,
    _out: *mut f64,
) -> bool {
    // Optional, and a host without it falls back to its own editing. Parsing
    // "480 ms" back into a position needs the inverse of every effect's
    // mapping, which is a table nobody has asked for yet.
    false
}

unsafe extern "C" fn params_flush(
    plugin: *const clap_plugin,
    input: *const clap_sys::events::clap_input_events,
    _output: *const clap_sys::events::clap_output_events,
) {
    if let Some(inst) = unsafe { instance(plugin) } {
        unsafe { inst.drain_events(input) };
    }
}

static PARAMS: clap_plugin_params = clap_plugin_params {
    count: Some(params_count),
    get_info: Some(params_get_info),
    get_value: Some(params_get_value),
    value_to_text: Some(params_value_to_text),
    text_to_value: Some(params_text_to_value),
    flush: Some(params_flush),
};

// ─── The factory ────────────────────────────────────────────────────────────

unsafe extern "C" fn factory_count(_factory: *const clap_plugin_factory) -> u32 {
    catalogue().len() as u32
}

unsafe extern "C" fn factory_descriptor(
    _factory: *const clap_plugin_factory,
    index: u32,
) -> *const clap_plugin_descriptor {
    match catalogue().get(index as usize) {
        Some(e) => &e.descriptor,
        None => std::ptr::null(),
    }
}

unsafe extern "C" fn factory_create(
    _factory: *const clap_plugin_factory,
    _host: *const clap_host,
    plugin_id: *const c_char,
) -> *const clap_plugin {
    if plugin_id.is_null() {
        return std::ptr::null();
    }
    let wanted = unsafe { CStr::from_ptr(plugin_id) };
    let Some(index) = catalogue().iter().position(|e| {
        // The descriptor's id is the same leaked string, so comparing the
        // bytes is comparing the plugin.
        let id = unsafe { CStr::from_ptr(e.descriptor.id) };
        id == wanted
    }) else {
        return std::ptr::null();
    };
    let exported = &catalogue()[index];
    // Defaults from the effect itself, plus a fully wet mix — which is what an
    // effect in a host's chain is expected to be.
    let mut values: Vec<f32> = exported.params.iter().map(|p| p.value).collect();
    values.push(1.0);

    let instance = Box::new(Instance {
        plugin: clap_plugin {
            desc: &exported.descriptor,
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
        index,
        processor: None,
        generator: None,
        // Sized once, here: a step can start eight notes and let eight go, and
        // the audio thread must not be the place that finds room for them.
        notes: Vec::with_capacity(64),
        values,
        sample_rate: 48_000,
        scratch: Vec::new(),
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

    /// The clock is global to the process (see [`follow_host_transport`]), and
    /// `cargo test` runs these in one — so a test that activates a plugin is a
    /// test that rewinds it under whoever else is looking. One lock, held for
    /// the whole of each test.
    fn guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A minimal host, in this process: no `.clap` file, no dlopen, no DAW.
    /// What is exercised is the ABI itself — the factory, the descriptors, the
    /// extensions and one block of audio — which is the part that can be wrong
    /// in ways a Rust type never catches.
    unsafe fn open(id: &str) -> *const clap_plugin {
        let factory =
            entry_get_factory(CLAP_PLUGIN_FACTORY_ID.as_ptr()) as *const clap_plugin_factory;
        assert!(
            !factory.is_null(),
            "the plugin factory is what a host asks for"
        );
        let id = CString::new(id).unwrap();
        let plugin = ((*factory).create_plugin.unwrap())(factory, std::ptr::null(), id.as_ptr());
        assert!(!plugin.is_null(), "the host asked for a plugin we publish");
        assert!(((*plugin).init.unwrap())(plugin));
        plugin
    }

    /// One block through an exported effect, the way a host runs it: planar
    /// buffers in, planar buffers out, and a parameter set by event.
    #[test]
    fn a_host_can_load_an_effect_and_push_a_block_through_it() {
        let _guard = guard();
        const FRAMES: usize = 128;
        unsafe {
            let plugin = open("org.choz.fx.gain");
            assert!(((*plugin).activate.unwrap())(
                plugin,
                48_000.0,
                32,
                FRAMES as u32
            ));
            assert!(((*plugin).start_processing.unwrap())(plugin));

            // Gain's first knob at full: whatever comes out must be louder than
            // what went in, and the point is that the *host's* value reached
            // choz's DSP.
            let event = clap_event_param_value {
                header: clap_event_header {
                    size: std::mem::size_of::<clap_event_param_value>() as u32,
                    time: 0,
                    space_id: CLAP_CORE_EVENT_SPACE_ID,
                    type_: CLAP_EVENT_PARAM_VALUE,
                    flags: 0,
                },
                param_id: 0,
                cookie: std::ptr::null_mut(),
                note_id: -1,
                port_index: 0,
                channel: -1,
                key: -1,
                value: 1.0,
            };
            let events = [&event.header as *const clap_event_header];
            let list = events_list(&events);

            let mut left_in = vec![0.25f32; FRAMES];
            let mut right_in = vec![0.25f32; FRAMES];
            let mut left_out = vec![0.0f32; FRAMES];
            let mut right_out = vec![0.0f32; FRAMES];
            let mut in_ptrs = [left_in.as_mut_ptr(), right_in.as_mut_ptr()];
            let mut out_ptrs = [left_out.as_mut_ptr(), right_out.as_mut_ptr()];
            let input = clap_sys::audio_buffer::clap_audio_buffer {
                data32: in_ptrs.as_mut_ptr(),
                data64: std::ptr::null_mut(),
                channel_count: 2,
                latency: 0,
                constant_mask: 0,
            };
            let mut output = clap_sys::audio_buffer::clap_audio_buffer {
                data32: out_ptrs.as_mut_ptr(),
                data64: std::ptr::null_mut(),
                channel_count: 2,
                latency: 0,
                constant_mask: 0,
            };
            let process = clap_process {
                steady_time: 0,
                frames_count: FRAMES as u32,
                transport: std::ptr::null(),
                audio_inputs: &input,
                audio_outputs: &mut output,
                audio_inputs_count: 1,
                audio_outputs_count: 1,
                in_events: &list,
                out_events: std::ptr::null(),
            };
            let status = ((*plugin).process.unwrap())(plugin, &process);
            assert_eq!(status, CLAP_PROCESS_CONTINUE);
            assert!(
                left_out[0] > 0.25 && left_out[0].is_finite(),
                "the host's parameter reached the DSP: {}",
                left_out[0]
            );
            assert_eq!(left_out[10], right_out[10], "both channels were written");

            ((*plugin).stop_processing.unwrap())(plugin);
            ((*plugin).deactivate.unwrap())(plugin);
            ((*plugin).destroy.unwrap())(plugin);
        }
    }

    /// A tempo-synced effect follows the **host's** clock, not one nobody
    /// winds. Inside choz the transport is driven by the audio callback; out
    /// here the DAW owns it and hands it over on every block.
    #[test]
    fn the_host_transport_drives_the_clock() {
        let _guard = guard();
        const FRAMES: usize = 64;
        unsafe {
            let plugin = open("org.choz.fx.beatrepeat");
            assert!(((*plugin).activate.unwrap())(
                plugin,
                48_000.0,
                32,
                FRAMES as u32
            ));

            let transport = clap_sys::events::clap_event_transport {
                header: clap_event_header {
                    size: std::mem::size_of::<clap_sys::events::clap_event_transport>() as u32,
                    time: 0,
                    space_id: CLAP_CORE_EVENT_SPACE_ID,
                    type_: clap_sys::events::CLAP_EVENT_TRANSPORT,
                    flags: 0,
                },
                flags: clap_sys::events::CLAP_TRANSPORT_HAS_TEMPO
                    | clap_sys::events::CLAP_TRANSPORT_HAS_TIME_SIGNATURE
                    | clap_sys::events::CLAP_TRANSPORT_HAS_BEATS_TIMELINE
                    | clap_sys::events::CLAP_TRANSPORT_IS_PLAYING,
                // Two bars and a half in, at 90 bpm in 6/8.
                song_pos_beats: (7.5 * clap_sys::fixedpoint::CLAP_BEATTIME_FACTOR as f64) as i64,
                song_pos_seconds: 0,
                tempo: 90.0,
                tempo_inc: 0.0,
                loop_start_beats: 0,
                loop_end_beats: 0,
                loop_start_seconds: 0,
                loop_end_seconds: 0,
                bar_start: 0,
                bar_number: 2,
                tsig_num: 6,
                tsig_denom: 8,
            };

            let mut left = vec![0.1f32; FRAMES];
            let mut right = vec![0.1f32; FRAMES];
            let mut ptrs = [left.as_mut_ptr(), right.as_mut_ptr()];
            let mut buffer = clap_sys::audio_buffer::clap_audio_buffer {
                data32: ptrs.as_mut_ptr(),
                data64: std::ptr::null_mut(),
                channel_count: 2,
                latency: 0,
                constant_mask: 0,
            };
            let process = clap_process {
                steady_time: 0,
                frames_count: FRAMES as u32,
                transport: &transport,
                audio_inputs: &buffer,
                audio_outputs: &mut buffer,
                audio_inputs_count: 1,
                audio_outputs_count: 1,
                in_events: std::ptr::null(),
                out_events: std::ptr::null(),
            };
            assert_eq!(
                ((*plugin).process.unwrap())(plugin, &process),
                CLAP_PROCESS_CONTINUE
            );

            let clock = choz_ports::transport();
            assert_eq!(clock.bpm(), 90.0);
            assert_eq!(clock.time_signature(), (6, 8));
            assert!(clock.playing());
            assert!(
                (clock.ppq() - 7.5).abs() < 1e-3,
                "the clock is where the host says: {}",
                clock.ppq()
            );
            ((*plugin).destroy.unwrap())(plugin);
        }
    }

    /// The bundle publishes every built-in **and both artifacts**, each with a
    /// name, an id and a parameter list a host can walk — for an effect that
    /// includes the dry/wet choz normally keeps in the chain.
    #[test]
    fn the_factory_publishes_every_built_in_effect() {
        let _guard = guard();
        unsafe {
            let factory =
                entry_get_factory(CLAP_PLUGIN_FACTORY_ID.as_ptr()) as *const clap_plugin_factory;
            let count = ((*factory).get_plugin_count.unwrap())(factory) as usize;
            assert_eq!(
                count,
                choz_engine::fx_chain::BUILT_IN_KINDS.len() + 2,
                "every built-in travels, and so do both artifacts"
            );

            let mut artifacts = Vec::new();
            for i in 0..count {
                let desc = ((*factory).get_plugin_descriptor.unwrap())(factory, i as u32);
                assert!(!desc.is_null());
                let id = CStr::from_ptr((*desc).id).to_string_lossy().into_owned();
                assert!(!CStr::from_ptr((*desc).name).to_bytes().is_empty(), "{id}");
                let is_fx = id.starts_with("org.choz.fx.");
                assert!(is_fx || id.starts_with("org.choz.gen."), "{id}");
                if !is_fx {
                    artifacts.push(id.clone());
                }

                let plugin = open(&id);
                let params = ((*plugin).get_extension.unwrap())(plugin, CLAP_EXT_PARAMS.as_ptr())
                    as *const clap_plugin_params;
                assert!(!params.is_null(), "{id} has no parameters extension");
                let n = ((*params).count.unwrap())(plugin);
                assert!(n >= 1, "{id} publishes something");

                if is_fx {
                    // The last one is the mix, and every one of them reads back.
                    let mut info = std::mem::zeroed::<clap_param_info>();
                    assert!(((*params).get_info.unwrap())(plugin, n - 1, &mut info));
                    let name = CStr::from_ptr(info.name.as_ptr())
                        .to_string_lossy()
                        .into_owned();
                    assert_eq!(name, MIX_NAME, "{id}");
                    let mut value = 0.0f64;
                    assert!(((*params).get_value.unwrap())(plugin, n - 1, &mut value));
                    assert_eq!(value, 1.0, "{id} starts fully wet");
                }

                ((*plugin).destroy.unwrap())(plugin);
            }
            artifacts.sort();
            assert_eq!(artifacts, ["org.choz.gen.arp", "org.choz.gen.seq"]);
        }
    }

    /// An artifact is a **note** plugin: note ports and no audio ones, which is
    /// the whole of what tells a host these speak keys rather than samples.
    #[test]
    fn the_artifacts_are_note_plugins() {
        use clap_sys::ext::audio_ports::clap_plugin_audio_ports;
        use clap_sys::ext::note_ports::{clap_plugin_note_ports, CLAP_EXT_NOTE_PORTS};
        let _guard = guard();
        unsafe {
            for (id, ins) in [("org.choz.gen.arp", 1u32), ("org.choz.gen.seq", 0)] {
                let plugin = open(id);
                let notes = ((*plugin).get_extension.unwrap())(plugin, CLAP_EXT_NOTE_PORTS.as_ptr())
                    as *const clap_plugin_note_ports;
                assert!(!notes.is_null(), "{id} offers no note ports");
                assert_eq!(((*notes).count.unwrap())(plugin, true), ins, "{id} inputs");
                assert_eq!(((*notes).count.unwrap())(plugin, false), 1, "{id} outputs");

                // …and none of the other kind. A plugin claiming both and then
                // reporting zero of one is a plugin a host has to guess about.
                let audio =
                    ((*plugin).get_extension.unwrap())(plugin, CLAP_EXT_AUDIO_PORTS.as_ptr())
                        as *const clap_plugin_audio_ports;
                assert!(audio.is_null(), "{id} claims audio ports");
                ((*plugin).destroy.unwrap())(plugin);
            }

            // The effects are the other way round, and were before this.
            let plugin = open("org.choz.fx.delay");
            let notes = ((*plugin).get_extension.unwrap())(plugin, CLAP_EXT_NOTE_PORTS.as_ptr());
            assert!(notes.is_null(), "an effect has nothing to say about notes");
            ((*plugin).destroy.unwrap())(plugin);
        }
    }

    /// Deactivating and activating again keeps whatever the host had set —
    /// otherwise reopening a project would silently reset every knob.
    #[test]
    fn a_reactivated_plugin_keeps_its_parameters() {
        let _guard = guard();
        unsafe {
            let plugin = open("org.choz.fx.delay");
            let params = ((*plugin).get_extension.unwrap())(plugin, CLAP_EXT_PARAMS.as_ptr())
                as *const clap_plugin_params;
            assert!(((*plugin).activate.unwrap())(plugin, 44_100.0, 32, 256));

            let inst = instance(plugin).unwrap();
            inst.apply(1, 0.75);
            ((*plugin).deactivate.unwrap())(plugin);
            assert!(((*plugin).activate.unwrap())(plugin, 48_000.0, 32, 256));

            let mut value = 0.0f64;
            assert!(((*params).get_value.unwrap())(plugin, 1, &mut value));
            assert!((value - 0.75).abs() < 1e-6, "{value}");
            ((*plugin).destroy.unwrap())(plugin);
        }
    }

    /// A parameter reads in the effect's own units, not as a bare position:
    /// "0.24" is not a delay time anybody can dial in.
    #[test]
    fn a_value_reads_in_the_units_the_effect_uses() {
        let _guard = guard();
        unsafe {
            let plugin = open("org.choz.fx.delay");
            let params = ((*plugin).get_extension.unwrap())(plugin, CLAP_EXT_PARAMS.as_ptr())
                as *const clap_plugin_params;
            // `c_char`, not `i8`: the same thing on x86_64, `u8` on ARM.
            let mut buffer = [0 as c_char; 64];
            assert!(((*params).value_to_text.unwrap())(
                plugin,
                0,
                0.5,
                buffer.as_mut_ptr(),
                buffer.len() as u32
            ));
            let text = CStr::from_ptr(buffer.as_ptr())
                .to_string_lossy()
                .into_owned();
            assert!(text.contains("ms"), "the delay time is a time: {text}");
            ((*plugin).destroy.unwrap())(plugin);
        }
    }

    /// The event list a host hands over, as a `clap_input_events`.
    /// An output list that keeps what the plugin pushed, so a test can read it.
    fn out_list() -> (
        clap_sys::events::clap_output_events,
        &'static Vec<clap_event_note>,
    ) {
        unsafe extern "C" fn push(
            list: *const clap_sys::events::clap_output_events,
            event: *const clap_event_header,
        ) -> bool {
            unsafe {
                let out = &mut *((*list).ctx as *mut Vec<clap_event_note>);
                let header = &*event;
                if header.type_ != CLAP_EVENT_NOTE_ON && header.type_ != CLAP_EVENT_NOTE_OFF {
                    return true;
                }
                out.push(*(event as *const clap_event_note));
            }
            true
        }
        let ctx: &'static mut Vec<clap_event_note> = Box::leak(Box::new(Vec::new()));
        let raw = ctx as *mut Vec<clap_event_note>;
        (
            clap_sys::events::clap_output_events {
                ctx: raw as *mut c_void,
                try_push: Some(push),
            },
            unsafe { &*raw },
        )
    }

    /// One block through an artifact, with `params` set first.
    ///
    /// # Safety
    /// `id` is one this crate publishes.
    unsafe fn run_generator(
        id: &str,
        params: &[(u32, f64)],
        notes_in: &[(u16, i16)],
        blocks: usize,
    ) -> Vec<clap_event_note> {
        const FRAMES: usize = 256;
        unsafe {
            let plugin = open(id);
            // Parameters before `activate`: that is where the generator is
            // built from them, exactly as a host reopening a project does it.
            let mut headers: Vec<*const clap_event_header> = Vec::new();
            let param_events: Vec<clap_event_param_value> = params
                .iter()
                .map(|&(param_id, value)| clap_event_param_value {
                    header: clap_event_header {
                        size: std::mem::size_of::<clap_event_param_value>() as u32,
                        time: 0,
                        space_id: CLAP_CORE_EVENT_SPACE_ID,
                        type_: CLAP_EVENT_PARAM_VALUE,
                        flags: 0,
                    },
                    param_id,
                    cookie: std::ptr::null_mut(),
                    note_id: -1,
                    port_index: 0,
                    channel: -1,
                    key: -1,
                    value,
                })
                .collect();
            let note_events: Vec<clap_event_note> = notes_in
                .iter()
                .map(|&(type_, key)| clap_event_note {
                    header: clap_event_header {
                        size: std::mem::size_of::<clap_event_note>() as u32,
                        time: 0,
                        space_id: CLAP_CORE_EVENT_SPACE_ID,
                        type_,
                        flags: 0,
                    },
                    note_id: -1,
                    port_index: 0,
                    channel: 0,
                    key,
                    velocity: 0.8,
                })
                .collect();
            for e in param_events.iter() {
                headers.push(&e.header);
            }
            let params_only = events_list(&headers);
            ((*plugin).get_extension.unwrap())(plugin, CLAP_EXT_PARAMS.as_ptr());
            let inst = instance(plugin).unwrap();
            inst.drain_events(&params_only);

            assert!(((*plugin).activate.unwrap())(
                plugin,
                48_000.0,
                32,
                FRAMES as u32
            ));
            assert!(((*plugin).start_processing.unwrap())(plugin));

            // The keys arrive **once**, on the first block, the way a host
            // sends them. Resending a note-on every block is a retrigger, not a
            // held key.
            let mut first: Vec<*const clap_event_header> = headers.clone();
            for e in note_events.iter() {
                first.push(&e.header);
            }
            let first = events_list(&first);
            let list = events_list(&headers);
            let (out, kept) = out_list();
            // A rolling host, because that is the only clock a plugin has: the
            // generators follow a transport when one is moving, and a test that
            // called `process` in a tight loop with none would be asking them
            // to count wall-clock microseconds.
            use clap_sys::events::{
                clap_event_transport, CLAP_TRANSPORT_HAS_BEATS_TIMELINE, CLAP_TRANSPORT_HAS_TEMPO,
                CLAP_TRANSPORT_HAS_TIME_SIGNATURE, CLAP_TRANSPORT_IS_PLAYING,
            };
            const BPM: f64 = 120.0;
            let mut transport = clap_event_transport {
                header: clap_event_header {
                    size: std::mem::size_of::<clap_event_transport>() as u32,
                    time: 0,
                    space_id: CLAP_CORE_EVENT_SPACE_ID,
                    type_: clap_sys::events::CLAP_EVENT_TRANSPORT,
                    flags: 0,
                },
                flags: CLAP_TRANSPORT_HAS_TEMPO
                    | CLAP_TRANSPORT_HAS_BEATS_TIMELINE
                    | CLAP_TRANSPORT_HAS_TIME_SIGNATURE
                    | CLAP_TRANSPORT_IS_PLAYING,
                song_pos_beats: 0,
                song_pos_seconds: 0,
                tempo: BPM,
                tempo_inc: 0.0,
                loop_start_beats: 0,
                loop_end_beats: 0,
                loop_start_seconds: 0,
                loop_end_seconds: 0,
                bar_start: 0,
                bar_number: 0,
                tsig_num: 4,
                tsig_denom: 4,
            };
            // Through a pointer, because `process` holds one to it and the loop
            // moves the playhead between blocks — which is what a host does, and
            // what a `&mut` beside a live `&` would not allow.
            let clock: *mut clap_event_transport = &mut transport;
            let mut process = clap_process {
                steady_time: 0,
                frames_count: FRAMES as u32,
                transport: clock,
                audio_inputs: std::ptr::null(),
                audio_outputs: std::ptr::null_mut(),
                audio_inputs_count: 0,
                audio_outputs_count: 0,
                in_events: &list,
                out_events: &out,
            };
            let per_block = FRAMES as f64 / 48_000.0 * (BPM / 60.0);
            for i in 0..blocks {
                process.in_events = if i == 0 { &first } else { &list };
                (*clock).song_pos_beats =
                    (i as f64 * per_block * clap_sys::fixedpoint::CLAP_BEATTIME_FACTOR as f64)
                        as i64;
                assert_eq!(
                    ((*plugin).process.unwrap())(plugin, &process),
                    CLAP_PROCESS_CONTINUE
                );
            }
            ((*plugin).stop_processing.unwrap())(plugin);
            ((*plugin).destroy.unwrap())(plugin);
            kept.clone()
        }
    }

    /// The sequencer plays what a host wrote onto its grid — the whole point of
    /// publishing the grid as parameters.
    #[test]
    fn the_exported_sequencer_plays_its_grid() {
        let _guard = guard();
        let root = choz_engine::seq::SeqSettings::default().notes[0];
        unsafe {
            // Nothing written: nothing played, however long it runs.
            let quiet = run_generator("org.choz.gen.seq", &[], &[], 400);
            assert!(
                quiet.iter().all(|e| e.header.type_ != CLAP_EVENT_NOTE_ON),
                "an empty grid is silent"
            );

            // Lane A, every step. Param `track * STEPS + step`.
            let grid: Vec<(u32, f64)> = (0..STEPS).map(|i| (i as u32, 1.0)).collect();
            let played = run_generator("org.choz.gen.seq", &grid, &[], 400);
            let ons: Vec<i16> = played
                .iter()
                .filter(|e| e.header.type_ == CLAP_EVENT_NOTE_ON)
                .map(|e| e.key)
                .collect();
            assert!(!ons.is_empty(), "the written grid plays");
            assert!(
                ons.iter().all(|k| *k == root as i16),
                "lane A's note: {ons:?}"
            );
            assert!(
                played.iter().any(|e| e.header.type_ == CLAP_EVENT_NOTE_OFF),
                "and the gate closes"
            );

            // …and the lane's note is a parameter too: `SEQ_GRID + track`.
            let mut moved = grid.clone();
            moved.push((SEQ_GRID as u32, 72.0 / 127.0));
            let played = run_generator("org.choz.gen.seq", &moved, &[], 400);
            assert!(
                played
                    .iter()
                    .filter(|e| e.header.type_ == CLAP_EVENT_NOTE_ON)
                    .all(|e| e.key == 72),
                "the lane plays the note the host set"
            );
        }
    }

    /// The arpeggiator answers a held key with a pattern — notes in, notes out,
    /// which is the one thing an effect could never be.
    #[test]
    fn the_exported_arpeggiator_answers_a_held_key() {
        let _guard = guard();
        unsafe {
            // Nothing held: nothing to arpeggiate.
            let quiet = run_generator("org.choz.gen.arp", &[], &[], 400);
            assert!(quiet.is_empty(), "silence with no keys down: {quiet:?}");

            // Two keys down and never released, over two octaves.
            let held = [(CLAP_EVENT_NOTE_ON, 60i16), (CLAP_EVENT_NOTE_ON, 64)];
            let played = run_generator("org.choz.gen.arp", &[], &held, 400);
            let ons: Vec<i16> = played
                .iter()
                .filter(|e| e.header.type_ == CLAP_EVENT_NOTE_ON)
                .map(|e| e.key)
                .collect();
            assert!(ons.len() > 2, "it stepped through the chord: {ons:?}");
            assert!(
                ons.contains(&60) && ons.contains(&64),
                "both held keys are in it: {ons:?}"
            );
            assert!(
                ons.iter().all(|k| *k == 60 || *k == 64),
                "and nothing that was not held: {ons:?}"
            );
        }
    }

    fn events_list(events: &[*const clap_event_header]) -> clap_sys::events::clap_input_events {
        unsafe extern "C" fn size(list: *const clap_sys::events::clap_input_events) -> u32 {
            unsafe { (*((*list).ctx as *const Vec<*const clap_event_header>)).len() as u32 }
        }
        unsafe extern "C" fn get(
            list: *const clap_sys::events::clap_input_events,
            index: u32,
        ) -> *const clap_event_header {
            unsafe {
                let events = &*((*list).ctx as *const Vec<*const clap_event_header>);
                events[index as usize]
            }
        }
        // Leaked: it lives as long as the test, which is what the host promises
        // for the duration of one `process` call anyway.
        let ctx: &'static Vec<*const clap_event_header> = Box::leak(Box::new(events.to_vec()));
        clap_sys::events::clap_input_events {
            ctx: ctx as *const _ as *mut c_void,
            size: Some(size),
            get: Some(get),
        }
    }
}
