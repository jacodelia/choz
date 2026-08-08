//! Minimal LV2 C ABI — just the pieces needed to host a plugin without the
//! official LV2 SDK headers. Mirrors `lv2.h`, `urid.h`, `atom.h`, `midi.h`.
//!
//! References: <https://lv2plug.in/ns/>

#![allow(non_camel_case_types)]

use std::os::raw::{c_char, c_void};

// ─── Core (lv2core) ─────────────────────────────────────────────────────────

/// Opaque plugin instance handle (`LV2_Handle`).
pub type LV2_Handle = *mut c_void;

/// A host feature passed to `instantiate` (`LV2_Feature`).
#[repr(C)]
pub struct LV2_Feature {
    /// Feature URI (NUL-terminated).
    pub uri: *const c_char,
    /// Feature-specific data (e.g. a `*const LV2_URID_Map`).
    pub data: *mut c_void,
}

/// The plugin descriptor returned by `lv2_descriptor(index)` (`LV2_Descriptor`).
#[repr(C)]
pub struct LV2_Descriptor {
    /// Plugin URI (NUL-terminated) — matches the subject in the bundle TTL.
    pub uri: *const c_char,
    pub instantiate: Option<
        unsafe extern "C" fn(
            descriptor: *const LV2_Descriptor,
            sample_rate: f64,
            bundle_path: *const c_char,
            features: *const *const LV2_Feature,
        ) -> LV2_Handle,
    >,
    pub connect_port:
        Option<unsafe extern "C" fn(instance: LV2_Handle, port: u32, data_location: *mut c_void)>,
    pub activate: Option<unsafe extern "C" fn(instance: LV2_Handle)>,
    pub run: Option<unsafe extern "C" fn(instance: LV2_Handle, sample_count: u32)>,
    pub deactivate: Option<unsafe extern "C" fn(instance: LV2_Handle)>,
    pub cleanup: Option<unsafe extern "C" fn(instance: LV2_Handle)>,
    pub extension_data: Option<unsafe extern "C" fn(uri: *const c_char) -> *const c_void>,
}

/// Signature of the bundle entry point: `const LV2_Descriptor* lv2_descriptor(uint32_t index)`.
pub type Lv2DescriptorFn = unsafe extern "C" fn(index: u32) -> *const LV2_Descriptor;

/// The symbol name the entry point is exported under.
pub const LV2_DESCRIPTOR_SYM: &[u8] = b"lv2_descriptor";

// ─── URID extension (ext/urid) ──────────────────────────────────────────────

pub const LV2_URID_MAP_URI: &str = "http://lv2plug.in/ns/ext/urid#map";
pub const LV2_URID_UNMAP_URI: &str = "http://lv2plug.in/ns/ext/urid#unmap";

pub type LV2_URID = u32;
pub type LV2_URID_Map_Handle = *mut c_void;
pub type LV2_URID_Unmap_Handle = *mut c_void;

#[repr(C)]
pub struct LV2_URID_Map {
    pub handle: LV2_URID_Map_Handle,
    /// Map a URI string to an integer URID (never 0 on success).
    pub map: Option<unsafe extern "C" fn(handle: LV2_URID_Map_Handle, uri: *const c_char) -> LV2_URID>,
}

#[repr(C)]
pub struct LV2_URID_Unmap {
    pub handle: LV2_URID_Unmap_Handle,
    pub unmap:
        Option<unsafe extern "C" fn(handle: LV2_URID_Unmap_Handle, urid: LV2_URID) -> *const c_char>,
}

// ─── Atom + MIDI (ext/atom, ext/midi) ───────────────────────────────────────

pub const LV2_ATOM_SEQUENCE_URI: &str = "http://lv2plug.in/ns/ext/atom#Sequence";
pub const LV2_ATOM_CHUNK_URI: &str = "http://lv2plug.in/ns/ext/atom#Chunk";
pub const LV2_MIDI_EVENT_URI: &str = "http://lv2plug.in/ns/ext/midi#MidiEvent";

/// `LV2_Atom` — header common to every atom (`{ size, type }`).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct LV2_Atom {
    pub size: u32,
    pub type_: u32,
}

/// `LV2_Atom_Sequence_Body` — `{ unit, pad }` following the sequence atom header.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct LV2_Atom_Sequence_Body {
    pub unit: u32,
    pub pad: u32,
}

/// `LV2_Atom_Sequence` — header for an atom port carrying timed events.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct LV2_Atom_Sequence {
    pub atom: LV2_Atom,
    pub body: LV2_Atom_Sequence_Body,
}

/// `LV2_Atom_Event` — `{ int64 frames; LV2_Atom body; <body bytes…> }`.
/// We use the audio-frame timestamp variant (not beats).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct LV2_Atom_Event {
    pub frames: i64,
    pub body: LV2_Atom,
    // followed by `body.size` bytes of event payload (raw MIDI for MidiEvent)
}

/// Round `size` up to the next 8-byte boundary (atoms are 64-bit aligned).
#[inline]
pub fn pad8(size: usize) -> usize {
    (size + 7) & !7
}

// ─── Options + buffer size (ext/options, ext/buf-size, ext/parameters) ──────

pub const LV2_OPTIONS_URI: &str = "http://lv2plug.in/ns/ext/options#options";
pub const LV2_BUF_SIZE_BOUNDED_URI: &str =
    "http://lv2plug.in/ns/ext/buf-size#boundedBlockLength";
pub const LV2_BUF_SIZE_MIN_BLOCK_URI: &str = "http://lv2plug.in/ns/ext/buf-size#minBlockLength";
pub const LV2_BUF_SIZE_MAX_BLOCK_URI: &str = "http://lv2plug.in/ns/ext/buf-size#maxBlockLength";
pub const LV2_BUF_SIZE_NOMINAL_BLOCK_URI: &str =
    "http://lv2plug.in/ns/ext/buf-size#nominalBlockLength";
pub const LV2_PARAM_SAMPLE_RATE_URI: &str = "http://lv2plug.in/ns/ext/parameters#sampleRate";
pub const LV2_ATOM_INT_URI: &str = "http://lv2plug.in/ns/ext/atom#Int";
pub const LV2_ATOM_FLOAT_URI: &str = "http://lv2plug.in/ns/ext/atom#Float";

/// `LV2_Options_Context::LV2_OPTIONS_INSTANCE`.
pub const LV2_OPTIONS_INSTANCE: u32 = 0;

/// `LV2_Options_Option` — one key/value the host offers the plugin. The array
/// passed as the feature data is terminated by an all-zero entry.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct LV2_Options_Option {
    pub context: u32,
    pub subject: u32,
    pub key: LV2_URID,
    pub size: u32,
    pub type_: LV2_URID,
    pub value: *const c_void,
}

// ─── Worker extension (ext/worker) ──────────────────────────────────────────

/// Feature the host offers so the plugin can hand off non-realtime work
/// (loading a file, resizing a buffer) from `run()`.
pub const LV2_WORKER_SCHEDULE_URI: &str = "http://lv2plug.in/ns/ext/worker#schedule";
/// Extension the plugin exposes through `extension_data` to do that work.
pub const LV2_WORKER_INTERFACE_URI: &str = "http://lv2plug.in/ns/ext/worker#interface";

/// `LV2_Worker_Status::LV2_WORKER_SUCCESS`.
pub const LV2_WORKER_SUCCESS: i32 = 0;
/// `LV2_Worker_Status::LV2_WORKER_ERR_UNKNOWN`.
pub const LV2_WORKER_ERR_UNKNOWN: i32 = 1;

/// Host callback the plugin's `work()` uses to send its answer back.
pub type LV2_Worker_Respond_Function = Option<
    unsafe extern "C" fn(handle: *mut c_void, size: u32, data: *const c_void) -> i32,
>;

/// The `worker#schedule` feature data (`LV2_Worker_Schedule`).
#[repr(C)]
pub struct LV2_Worker_Schedule {
    pub handle: *mut c_void,
    pub schedule_work: Option<
        unsafe extern "C" fn(handle: *mut c_void, size: u32, data: *const c_void) -> i32,
    >,
}

/// What `extension_data(worker#interface)` returns (`LV2_Worker_Interface`).
#[repr(C)]
pub struct LV2_Worker_Interface {
    pub work: Option<
        unsafe extern "C" fn(
            instance: LV2_Handle,
            respond: LV2_Worker_Respond_Function,
            handle: *mut c_void,
            size: u32,
            data: *const c_void,
        ) -> i32,
    >,
    pub work_response:
        Option<unsafe extern "C" fn(instance: LV2_Handle, size: u32, body: *const c_void) -> i32>,
    pub end_run: Option<unsafe extern "C" fn(instance: LV2_Handle) -> i32>,
}

// ─── UI extension (extensions/ui) ───────────────────────────────────────────

pub const LV2_UI_X11UI_URI: &str = "http://lv2plug.in/ns/extensions/ui#X11UI";
pub const LV2_UI_PARENT_URI: &str = "http://lv2plug.in/ns/extensions/ui#parent";
pub const LV2_UI_IDLE_INTERFACE_URI: &str = "http://lv2plug.in/ns/extensions/ui#idleInterface";

/// The symbol every UI binary exports: `const LV2UI_Descriptor* lv2ui_descriptor(uint32_t)`.
pub const LV2UI_DESCRIPTOR_SYM: &[u8] = b"lv2ui_descriptor";

pub type LV2UI_Handle = *mut c_void;
pub type LV2UI_Widget = *mut c_void;
pub type LV2UI_Controller = *mut c_void;

/// How a UI pushes a control change back at the host.
pub type LV2UI_Write_Function = Option<
    unsafe extern "C" fn(
        controller: LV2UI_Controller,
        port_index: u32,
        buffer_size: u32,
        port_protocol: u32,
        buffer: *const c_void,
    ),
>;

/// `LV2UI_Descriptor` from `ui.h`.
#[repr(C)]
pub struct LV2UI_Descriptor {
    /// UI URI (NUL-terminated) — the subject typed `ui:X11UI` in the bundle TTL.
    pub uri: *const c_char,
    pub instantiate: Option<
        unsafe extern "C" fn(
            descriptor: *const LV2UI_Descriptor,
            plugin_uri: *const c_char,
            bundle_path: *const c_char,
            write_function: LV2UI_Write_Function,
            controller: LV2UI_Controller,
            widget: *mut LV2UI_Widget,
            features: *const *const LV2_Feature,
        ) -> LV2UI_Handle,
    >,
    pub cleanup: Option<unsafe extern "C" fn(ui: LV2UI_Handle)>,
    pub port_event: Option<
        unsafe extern "C" fn(
            ui: LV2UI_Handle,
            port_index: u32,
            buffer_size: u32,
            format: u32,
            buffer: *const c_void,
        ),
    >,
    pub extension_data: Option<unsafe extern "C" fn(uri: *const c_char) -> *const c_void>,
}

pub type Lv2UiDescriptorFn = unsafe extern "C" fn(index: u32) -> *const LV2UI_Descriptor;

/// What `extension_data(ui:idleInterface)` returns. Returning non-zero means
/// the UI asked to be closed.
#[repr(C)]
pub struct LV2UI_Idle_Interface {
    pub idle: Option<unsafe extern "C" fn(ui: LV2UI_Handle) -> i32>,
}

// ─── State (ext/state) ──────────────────────────────────────────────────────

pub const LV2_STATE_INTERFACE_URI: &str = "http://lv2plug.in/ns/ext/state#interface";

/// `LV2_State_Status`. Anything non-zero is a failure.
pub const LV2_STATE_SUCCESS: i32 = 0;

pub type LV2_State_Handle = *mut c_void;

/// What the plugin calls to hand the host one piece of its state
/// (`LV2_State_Store_Function`). `key` and `type_` are URIDs; the host is
/// expected to copy the value.
pub type LV2_State_Store_Function = unsafe extern "C" fn(
    handle: LV2_State_Handle,
    key: u32,
    value: *const c_void,
    size: usize,
    type_: u32,
    flags: u32,
) -> i32;

/// The other direction (`LV2_State_Retrieve_Function`): the host returns a
/// pointer it keeps alive for the duration of `restore`.
pub type LV2_State_Retrieve_Function = unsafe extern "C" fn(
    handle: LV2_State_Handle,
    key: u32,
    size: *mut usize,
    type_: *mut u32,
    flags: *mut u32,
) -> *const c_void;

/// What `extension_data(state#interface)` returns.
#[repr(C)]
pub struct LV2_State_Interface {
    pub save: Option<
        unsafe extern "C" fn(
            instance: LV2_Handle,
            store: LV2_State_Store_Function,
            handle: LV2_State_Handle,
            flags: u32,
            features: *const *const LV2_Feature,
        ) -> i32,
    >,
    pub restore: Option<
        unsafe extern "C" fn(
            instance: LV2_Handle,
            retrieve: LV2_State_Retrieve_Function,
            handle: LV2_State_Handle,
            flags: u32,
            features: *const *const LV2_Feature,
        ) -> i32,
    >,
}

