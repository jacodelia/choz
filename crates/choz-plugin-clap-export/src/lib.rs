//! choz's own effects, published as CLAP plugins.
//!
//! The other direction from `choz-plugin-clap`: instead of choz loading
//! somebody else's plugin, this lets somebody else's host — Bitwig, Reaper,
//! Carla — load choz's 45 effects. One `.clap` file publishes all of them,
//! because a CLAP factory answers with as many plugins as it likes.
//!
//! # What travels and what does not
//!
//! **The DSP travels.** [`choz_ports::FxProcessor`] is already the shape of a
//! plugin — `process_block`, `params`, `set_param`, `reset` — and the effects
//! are built from an array of normalised `f32` and know nothing about racks or
//! tabs. That is why this crate is small.
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
    clap_event_header, clap_event_param_value, CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_PARAM_VALUE,
};
use clap_sys::ext::audio_ports::{
    clap_audio_port_info, clap_plugin_audio_ports, CLAP_AUDIO_PORT_IS_MAIN, CLAP_EXT_AUDIO_PORTS,
    CLAP_PORT_STEREO,
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

/// One exported effect: what the host is told about it, and what its knobs are.
struct Exported {
    /// The id [`choz_engine::fx_chain::build_processor`] answers to.
    kind: &'static str,
    /// Its knobs, in order, as the effect itself reports them. The dry/wet is
    /// not in here — it is appended, and it is the only parameter this crate
    /// invents.
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
        choz_engine::fx_chain::BUILT_IN_KINDS
            .iter()
            .filter_map(|&(kind, name)| {
                let proc = choz_engine::fx_chain::build_processor(kind, &defaults, 48_000)?;
                let features: &'static [*const c_char] = Box::leak(
                    vec![forever("audio-effect"), forever("stereo"), std::ptr::null()]
                        .into_boxed_slice(),
                );
                Some(Exported {
                    kind,
                    params: proc.params(),
                    descriptor: clap_plugin_descriptor {
                        clap_version: CLAP_VERSION,
                        id: forever(&format!("org.choz.fx.{kind}")),
                        // The catalogue's name, not `proc.name()`: half the
                        // effects never override it, and "FX" is not a plugin
                        // anybody can find in a list of four hundred.
                        name: forever(name),
                        vendor: forever("choz"),
                        url: forever("https://github.com/jcodelia/choz"),
                        manual_url: std::ptr::null(),
                        support_url: std::ptr::null(),
                        version: forever(env!("CARGO_PKG_VERSION")),
                        description: forever("A choz effect, outside choz"),
                        features: features.as_ptr(),
                    },
                })
            })
            .collect()
    })
}

/// How many knobs a host sees: the effect's own, plus the dry/wet.
fn param_count(e: &Exported) -> usize {
    e.params.len() + 1
}

// ─── One running instance ───────────────────────────────────────────────────

/// The plugin handed to the host. `plugin` is first so the two can be cast into
/// each other, which is the usual C idiom and what `plugin_data` is for anyway.
#[repr(C)]
struct Instance {
    plugin: clap_plugin,
    index: usize,
    /// Built at `activate`, when the sample rate is finally known. `None`
    /// between `deactivate` and the next `activate`.
    processor: Option<Box<dyn FxProcessor>>,
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
            if header.space_id != CLAP_CORE_EVENT_SPACE_ID || header.type_ != CLAP_EVENT_PARAM_VALUE
            {
                continue;
            }
            let event: &clap_event_param_value =
                unsafe { &*(header as *const clap_event_header as *const clap_event_param_value) };
            self.apply(event.param_id as usize, event.value as f32);
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
    // The processor is built from the knob positions the host has been setting
    // all along, so activating twice does not reset the sound.
    let Some(mut proc) =
        choz_engine::fx_chain::build_processor(inst.exported().kind, &inst.values, inst.sample_rate)
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
    if id == CLAP_EXT_AUDIO_PORTS {
        return &AUDIO_PORTS as *const _ as *const c_void;
    }
    if id == CLAP_EXT_PARAMS {
        return &PARAMS as *const _ as *const c_void;
    }
    std::ptr::null()
}

unsafe extern "C" fn plugin_on_main_thread(_plugin: *const clap_plugin) {}

// ─── Audio ports ────────────────────────────────────────────────────────────

unsafe extern "C" fn ports_count(_plugin: *const clap_plugin, _is_input: bool) -> u32 {
    1
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
        let factory = entry_get_factory(CLAP_PLUGIN_FACTORY_ID.as_ptr()) as *const clap_plugin_factory;
        assert!(!factory.is_null(), "the plugin factory is what a host asks for");
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
            assert!(((*plugin).activate.unwrap())(plugin, 48_000.0, 32, FRAMES as u32));
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
            assert!(((*plugin).activate.unwrap())(plugin, 48_000.0, 32, FRAMES as u32));

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

    /// The bundle publishes every built-in, each one with a name, an id and a
    /// parameter list a host can walk — including the dry/wet choz normally
    /// keeps in the chain.
    #[test]
    fn the_factory_publishes_every_built_in_effect() {
        let _guard = guard();
        unsafe {
            let factory =
                entry_get_factory(CLAP_PLUGIN_FACTORY_ID.as_ptr()) as *const clap_plugin_factory;
            let count = ((*factory).get_plugin_count.unwrap())(factory) as usize;
            assert_eq!(
                count,
                choz_engine::fx_chain::BUILT_IN_KINDS.len(),
                "every built-in travels"
            );

            for i in 0..count {
                let desc = ((*factory).get_plugin_descriptor.unwrap())(factory, i as u32);
                assert!(!desc.is_null());
                let id = CStr::from_ptr((*desc).id).to_string_lossy().into_owned();
                assert!(id.starts_with("org.choz.fx."), "{id}");
                assert!(!CStr::from_ptr((*desc).name).to_bytes().is_empty(), "{id}");

                let plugin = open(&id);
                let params = ((*plugin).get_extension.unwrap())(plugin, CLAP_EXT_PARAMS.as_ptr())
                    as *const clap_plugin_params;
                assert!(!params.is_null(), "{id} has no parameters extension");
                let n = ((*params).count.unwrap())(plugin);
                assert!(n >= 1, "{id} publishes at least the dry/wet");

                // The last one is the mix, and every one of them reads back.
                let mut info = std::mem::zeroed::<clap_param_info>();
                assert!(((*params).get_info.unwrap())(plugin, n - 1, &mut info));
                let name = CStr::from_ptr(info.name.as_ptr()).to_string_lossy().into_owned();
                assert_eq!(name, MIX_NAME, "{id}");
                let mut value = 0.0f64;
                assert!(((*params).get_value.unwrap())(plugin, n - 1, &mut value));
                assert_eq!(value, 1.0, "{id} starts fully wet");

                ((*plugin).destroy.unwrap())(plugin);
            }
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
            let text = CStr::from_ptr(buffer.as_ptr()).to_string_lossy().into_owned();
            assert!(text.contains("ms"), "the delay time is a time: {text}");
            ((*plugin).destroy.unwrap())(plugin);
        }
    }

    /// The event list a host hands over, as a `clap_input_events`.
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
