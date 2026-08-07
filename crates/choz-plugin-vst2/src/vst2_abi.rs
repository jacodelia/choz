//! Minimal VST2 ABI types.
//!
//! The VST2 SDK is no longer publicly available from Steinberg.
//! This module defines the minimal C structs needed to load and call VST2 plugins
//! using only the publicly-documented binary interface (same ABI as every DAW).
//!
//! References: VST 2.4 SDK documentation, kVstVersion = 2400.

#![allow(non_camel_case_types, non_snake_case, dead_code)]

use std::os::raw::{c_char, c_float, c_int, c_void};

pub const VST_MAGIC: i32 = 0x56737450; // 'VstP'
pub const VST_VERSION: i32 = 2400;

/// Opaque host/plugin communication pointer.
pub type AEffectPtr = *mut AEffect;

/// Host callback function signature.
pub type HostCallbackProc = unsafe extern "C" fn(
    effect: AEffectPtr,
    opcode: i32,
    index: i32,
    value: isize,
    ptr: *mut c_void,
    opt: c_float,
) -> isize;

/// Plugin dispatcher function signature.
pub type DispatcherProc = unsafe extern "C" fn(
    effect: AEffectPtr,
    opcode: i32,
    index: i32,
    value: isize,
    ptr: *mut c_void,
    opt: c_float,
) -> isize;

/// Plugin process (f32) function signature.
pub type ProcessProc = unsafe extern "C" fn(
    effect: AEffectPtr,
    inputs: *mut *mut c_float,
    outputs: *mut *mut c_float,
    sample_frames: c_int,
);

/// Plugin set-parameter function signature.
pub type SetParameterProc = unsafe extern "C" fn(effect: AEffectPtr, index: c_int, value: c_float);

/// Plugin get-parameter function signature.
pub type GetParameterProc = unsafe extern "C" fn(effect: AEffectPtr, index: c_int) -> c_float;

/// The core VST2 struct (`AEffect`).
#[repr(C)]
pub struct AEffect {
    pub magic: i32,
    pub dispatcher: Option<DispatcherProc>,
    pub _process_deprecated: Option<ProcessProc>,
    pub set_parameter: Option<SetParameterProc>,
    pub get_parameter: Option<GetParameterProc>,
    pub num_programs: i32,
    pub num_params: i32,
    pub num_inputs: i32,
    pub num_outputs: i32,
    pub flags: i32,
    pub _resvd1: isize,
    pub _resvd2: isize,
    pub initial_delay: i32,
    pub _real_qualities: i32,
    pub _off_qualities: i32,
    pub _io_ratio: c_float,
    pub object: *mut c_void,
    pub user: *mut c_void,
    pub unique_id: i32,
    pub version: i32,
    pub process_replacing: Option<ProcessProc>,
    pub process_double_replacing: *mut c_void,
    pub _future: [u8; 56],
}

/// Event type tag for [`VstMidiEvent`] (`kVstMidiType`).
pub const VST_MIDI_TYPE: i32 = 1;

/// Editor rectangle returned by `effEditGetRect` (coords are i16).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct ERect {
    pub top:    i16,
    pub left:   i16,
    pub bottom: i16,
    pub right:  i16,
}

/// A single MIDI event passed to the plugin via `effProcessEvents`.
#[repr(C)]
pub struct VstMidiEvent {
    /// Always [`VST_MIDI_TYPE`].
    pub event_type:        i32,
    /// `size_of::<VstMidiEvent>()`.
    pub byte_size:         i32,
    /// Sample offset within the current process block.
    pub delta_frames:      i32,
    pub flags:             i32,
    pub note_length:       i32,
    pub note_offset:       i32,
    /// Raw MIDI bytes (status, data1, data2, and a 4th for running-status align).
    pub midi_data:         [u8; 4],
    pub detune:            i8,
    pub note_off_velocity: u8,
    pub reserved1:         u8,
    pub reserved2:         u8,
}

/// Header for a batch of events. VST2 defines a flexible trailing pointer array;
/// callers build one with a fixed-capacity companion (see the vst2 crate's
/// `send_midi`). `events` here is a 2-slot stub matching the C layout prefix.
#[repr(C)]
pub struct VstEventsHeader {
    pub num_events: i32,
    pub reserved:   isize,
    // Followed in memory by `num_events` `*mut VstMidiEvent` pointers.
    pub events:     [*mut VstMidiEvent; 2],
}

/// AEffect flag bits.
pub mod flags {
    pub const HAS_EDITOR:      i32 = 1 << 0;
    pub const CAN_REPLACING:   i32 = 1 << 4;
    pub const PROGRAM_CHUNKS:  i32 = 1 << 5;
    pub const IS_SYNTH:        i32 = 1 << 8;
    pub const NO_SOUND_IN_STOP:i32 = 1 << 9;
    pub const CAN_DOUBLE_REPLACING: i32 = 1 << 12;
}

/// Dispatcher opcode: plugin-side.
pub mod opcode {
    pub const OPEN:              i32 = 0;
    pub const CLOSE:             i32 = 1;
    pub const SET_PROGRAM:       i32 = 2;
    pub const GET_PROGRAM:       i32 = 3;
    pub const SET_PROGRAM_NAME:  i32 = 4;
    pub const GET_PROGRAM_NAME:  i32 = 5;
    pub const GET_PARAM_LABEL:   i32 = 6;
    pub const GET_PARAM_DISPLAY: i32 = 7;
    pub const GET_PARAM_NAME:    i32 = 8;
    pub const SET_SAMPLE_RATE:   i32 = 10;
    pub const SET_BLOCK_SIZE:    i32 = 11;
    pub const MAIN_RESUME:       i32 = 12;  // suspend=0, resume=1 via value
    pub const EDIT_GET_RECT:     i32 = 13;
    pub const EDIT_OPEN:         i32 = 14;  // value/ptr = native parent window handle
    pub const EDIT_CLOSE:        i32 = 15;
    pub const EDIT_IDLE:         i32 = 19;
    pub const GET_CHUNK:         i32 = 23;
    pub const SET_CHUNK:         i32 = 24;
    pub const PROCESS_EVENTS:    i32 = 25;
    pub const CAN_BE_AUTOMATED:  i32 = 26;
    pub const VENDOR_SPECIFIC:   i32 = 48;
    pub const CAN_DO:            i32 = 51;
    pub const GET_VENDOR_STRING: i32 = 47;
    pub const GET_PRODUCT_STRING:i32 = 48;
    pub const GET_VENDOR_VERSION:i32 = 49;
    pub const GET_PLUGIN_NAME:   i32 = 45;
    pub const GET_VST_VERSION:   i32 = 58;
}

/// Transport and tempo, handed to the plugin on `audioMasterGetTime`. Plugins
/// that sync anything (LFOs, delays, arpeggiators) ask for this every block —
/// and a good few dereference the answer without checking it for null.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct VstTimeInfo {
    pub sample_pos: f64,
    pub sample_rate: f64,
    pub nano_seconds: f64,
    pub ppq_pos: f64,
    pub tempo: f64,
    pub bar_start_pos: f64,
    pub cycle_start_pos: f64,
    pub cycle_end_pos: f64,
    pub time_sig_numerator: i32,
    pub time_sig_denominator: i32,
    pub smpte_offset: i32,
    pub smpte_frame_rate: i32,
    pub samples_to_next_clock: i32,
    pub flags: i32,
}

/// `VstTimeInfo.flags`: which fields the host actually filled in.
pub mod time_flags {
    pub const TRANSPORT_PLAYING: i32 = 1 << 1;
    pub const PPQ_POS_VALID:     i32 = 1 << 9;
    pub const TEMPO_VALID:       i32 = 1 << 10;
    pub const TIME_SIG_VALID:    i32 = 1 << 13;
}

/// Host opcode (passed from plugin back to host via callback).
pub mod host_opcode {
    pub const VERSION:         i32 = 1;
    pub const CURRENT_ID:      i32 = 2;
    pub const IDLE:            i32 = 3;
    pub const WANT_MIDI:       i32 = 6;
    pub const GET_TIME:        i32 = 7;
    pub const GET_SAMPLE_RATE: i32 = 16;
    pub const GET_BLOCK_SIZE:  i32 = 17;
    pub const GET_VENDOR_STRING: i32 = 32;
    pub const GET_PRODUCT_STRING: i32 = 33;
    pub const GET_VENDOR_VERSION: i32 = 34;
    pub const PROCESS_LEVEL:   i32 = 23;
    pub const CAN_DO:          i32 = 37;
    pub const AUTOMATE:        i32 = 0;
}

/// Entry-point symbol exported by every VST2 plugin shared library.
pub type VstPluginMainFn = unsafe extern "C" fn(callback: HostCallbackProc) -> AEffectPtr;

/// Read a null-terminated C string from a fixed-size buffer.
pub fn c_str_from_buf(buf: &[c_char]) -> String {
    let bytes: Vec<u8> = buf.iter()
        .take_while(|&&b| b != 0)
        .map(|&b| b as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}
