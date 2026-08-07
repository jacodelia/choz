//! Minimal LADSPA + DSSI C ABI — just what hosting needs, no SDK headers.
//!
//! References: `ladspa.h` (v1.1), `dssi.h` (v1.0), ALSA `seq_event.h`.

#![allow(non_camel_case_types)]

use std::os::raw::{c_char, c_int, c_uint, c_ulong, c_void};

pub type LADSPA_Data = f32;
pub type LADSPA_Handle = *mut c_void;
pub type LADSPA_PortDescriptor = c_int;
pub type LADSPA_PortRangeHintDescriptor = c_int;

pub const LADSPA_PORT_INPUT: c_int = 0x1;
pub const LADSPA_PORT_OUTPUT: c_int = 0x2;
pub const LADSPA_PORT_CONTROL: c_int = 0x4;
pub const LADSPA_PORT_AUDIO: c_int = 0x8;

pub const HINT_BOUNDED_BELOW: c_int = 0x1;
pub const HINT_BOUNDED_ABOVE: c_int = 0x2;
pub const HINT_TOGGLED: c_int = 0x4;
pub const HINT_SAMPLE_RATE: c_int = 0x8;
pub const HINT_LOGARITHMIC: c_int = 0x10;
pub const HINT_INTEGER: c_int = 0x20;
pub const HINT_DEFAULT_MASK: c_int = 0x3C0;
pub const HINT_DEFAULT_NONE: c_int = 0x0;
pub const HINT_DEFAULT_MINIMUM: c_int = 0x40;
pub const HINT_DEFAULT_LOW: c_int = 0x80;
pub const HINT_DEFAULT_MIDDLE: c_int = 0xC0;
pub const HINT_DEFAULT_HIGH: c_int = 0x100;
pub const HINT_DEFAULT_MAXIMUM: c_int = 0x140;
pub const HINT_DEFAULT_0: c_int = 0x200;
pub const HINT_DEFAULT_1: c_int = 0x240;
pub const HINT_DEFAULT_100: c_int = 0x280;
pub const HINT_DEFAULT_440: c_int = 0x2C0;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct LADSPA_PortRangeHint {
    pub hint_descriptor: LADSPA_PortRangeHintDescriptor,
    pub lower_bound: LADSPA_Data,
    pub upper_bound: LADSPA_Data,
}

#[repr(C)]
pub struct LADSPA_Descriptor {
    pub unique_id: c_ulong,
    pub label: *const c_char,
    pub properties: c_int,
    pub name: *const c_char,
    pub maker: *const c_char,
    pub copyright: *const c_char,
    pub port_count: c_ulong,
    pub port_descriptors: *const LADSPA_PortDescriptor,
    pub port_names: *const *const c_char,
    pub port_range_hints: *const LADSPA_PortRangeHint,
    pub implementation_data: *mut c_void,
    pub instantiate:
        Option<unsafe extern "C" fn(d: *const LADSPA_Descriptor, sample_rate: c_ulong) -> LADSPA_Handle>,
    pub connect_port:
        Option<unsafe extern "C" fn(h: LADSPA_Handle, port: c_ulong, data: *mut LADSPA_Data)>,
    pub activate: Option<unsafe extern "C" fn(h: LADSPA_Handle)>,
    pub run: Option<unsafe extern "C" fn(h: LADSPA_Handle, sample_count: c_ulong)>,
    pub run_adding: Option<unsafe extern "C" fn(h: LADSPA_Handle, sample_count: c_ulong)>,
    pub set_run_adding_gain: Option<unsafe extern "C" fn(h: LADSPA_Handle, gain: LADSPA_Data)>,
    pub deactivate: Option<unsafe extern "C" fn(h: LADSPA_Handle)>,
    pub cleanup: Option<unsafe extern "C" fn(h: LADSPA_Handle)>,
}

pub type LadspaDescriptorFn = unsafe extern "C" fn(index: c_ulong) -> *const LADSPA_Descriptor;
pub const LADSPA_DESCRIPTOR_SYM: &[u8] = b"ladspa_descriptor";

// ─── DSSI ───────────────────────────────────────────────────────────────────

pub type DssiDescriptorFn = unsafe extern "C" fn(index: c_ulong) -> *const DSSI_Descriptor;
pub const DSSI_DESCRIPTOR_SYM: &[u8] = b"dssi_descriptor";

/// `DSSI_Program_Descriptor` — one selectable program (bank + program + name).
#[repr(C)]
pub struct DSSI_Program_Descriptor {
    pub bank: c_ulong,
    pub program: c_ulong,
    pub name: *const c_char,
}

#[repr(C)]
pub struct DSSI_Descriptor {
    pub api_version: c_int,
    pub ladspa: *const LADSPA_Descriptor,
    pub configure: Option<
        unsafe extern "C" fn(h: LADSPA_Handle, key: *const c_char, value: *const c_char) -> *mut c_char,
    >,
    pub get_program:
        Option<unsafe extern "C" fn(h: LADSPA_Handle, index: c_ulong) -> *const DSSI_Program_Descriptor>,
    pub select_program: Option<unsafe extern "C" fn(h: LADSPA_Handle, bank: c_ulong, program: c_ulong)>,
    pub get_midi_controller_for_port:
        Option<unsafe extern "C" fn(h: LADSPA_Handle, port: c_ulong) -> c_int>,
    pub run_synth: Option<
        unsafe extern "C" fn(
            h: LADSPA_Handle,
            sample_count: c_ulong,
            events: *mut snd_seq_event_t,
            event_count: c_ulong,
        ),
    >,
    pub run_synth_adding: Option<
        unsafe extern "C" fn(
            h: LADSPA_Handle,
            sample_count: c_ulong,
            events: *mut snd_seq_event_t,
            event_count: c_ulong,
        ),
    >,
    pub run_multiple_synths: Option<
        unsafe extern "C" fn(
            instance_count: c_ulong,
            handles: *mut LADSPA_Handle,
            sample_count: c_ulong,
            events: *mut *mut snd_seq_event_t,
            event_counts: *mut c_ulong,
        ),
    >,
    pub run_multiple_synths_adding: Option<
        unsafe extern "C" fn(
            instance_count: c_ulong,
            handles: *mut LADSPA_Handle,
            sample_count: c_ulong,
            events: *mut *mut snd_seq_event_t,
            event_counts: *mut c_ulong,
        ),
    >,
}

// ─── ALSA sequencer events (what DSSI's run_synth takes) ────────────────────

pub const SND_SEQ_EVENT_NOTEON: u8 = 6;
pub const SND_SEQ_EVENT_NOTEOFF: u8 = 7;
pub const SND_SEQ_EVENT_CONTROLLER: u8 = 10;
pub const SND_SEQ_EVENT_PGMCHANGE: u8 = 11;
pub const SND_SEQ_EVENT_PITCHBEND: u8 = 13;

/// `snd_seq_ev_note_t`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct snd_seq_ev_note_t {
    pub channel: u8,
    pub note: u8,
    pub velocity: u8,
    pub off_velocity: u8,
    pub duration: c_uint,
}

/// `snd_seq_ev_ctrl_t`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct snd_seq_ev_ctrl_t {
    pub channel: u8,
    pub unused: [u8; 3],
    pub param: c_uint,
    pub value: c_int,
}

/// The 12-byte data union of `snd_seq_event_t`, as raw bytes: the two variants
/// we emit (note, ctrl) are written into it.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct snd_seq_ev_data_t {
    pub raw: [u8; 12],
}

/// `snd_seq_timestamp_t` — we always use the tick variant (DSSI reads it as the
/// frame offset inside the block).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct snd_seq_timestamp_t {
    pub tick_or_sec: c_uint,
    pub nsec: c_uint,
}

/// `snd_seq_addr_t`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct snd_seq_addr_t {
    pub client: u8,
    pub port: u8,
}

/// `snd_seq_event_t` — 28 bytes on every platform ALSA supports.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct snd_seq_event_t {
    pub type_: u8,
    pub flags: u8,
    pub tag: u8,
    pub queue: u8,
    pub time: snd_seq_timestamp_t,
    pub source: snd_seq_addr_t,
    pub dest: snd_seq_addr_t,
    pub data: snd_seq_ev_data_t,
}

impl snd_seq_event_t {
    fn with_data<T: Copy>(type_: u8, frame: u32, body: T) -> Self {
        let mut ev = snd_seq_event_t { type_, ..Default::default() };
        ev.time.tick_or_sec = frame;
        assert!(std::mem::size_of::<T>() <= 12, "event body must fit the union");
        // SAFETY: `body` is a plain-old-data ALSA event struct that fits the
        // 12-byte union, copied byte for byte.
        unsafe {
            std::ptr::copy_nonoverlapping(
                &body as *const T as *const u8,
                ev.data.raw.as_mut_ptr(),
                std::mem::size_of::<T>(),
            );
        }
        ev
    }

    /// Build one event from a raw 3-byte MIDI message, at `frame` in the block.
    /// Returns `None` for messages DSSI has no event type for.
    pub fn from_midi(data: [u8; 3], frame: u32) -> Option<Self> {
        let channel = data[0] & 0x0F;
        let note = |velocity: u8, type_: u8| {
            Some(Self::with_data(
                type_,
                frame,
                snd_seq_ev_note_t { channel, note: data[1] & 0x7F, velocity, ..Default::default() },
            ))
        };
        match data[0] & 0xF0 {
            0x80 => note(data[2] & 0x7F, SND_SEQ_EVENT_NOTEOFF),
            // A note-on at velocity 0 is a note-off; DSSI synths expect the
            // real event type, not the shorthand.
            0x90 if data[2] & 0x7F == 0 => note(0, SND_SEQ_EVENT_NOTEOFF),
            0x90 => note(data[2] & 0x7F, SND_SEQ_EVENT_NOTEON),
            0xB0 => Some(Self::with_data(
                SND_SEQ_EVENT_CONTROLLER,
                frame,
                snd_seq_ev_ctrl_t {
                    channel,
                    param: (data[1] & 0x7F) as c_uint,
                    value: (data[2] & 0x7F) as c_int,
                    ..Default::default()
                },
            )),
            0xC0 => Some(Self::with_data(
                SND_SEQ_EVENT_PGMCHANGE,
                frame,
                snd_seq_ev_ctrl_t {
                    channel,
                    value: (data[1] & 0x7F) as c_int,
                    ..Default::default()
                },
            )),
            0xE0 => Some(Self::with_data(
                SND_SEQ_EVENT_PITCHBEND,
                frame,
                snd_seq_ev_ctrl_t {
                    channel,
                    // ALSA carries pitch bend centred on zero.
                    value: (((data[2] as i32) << 7 | data[1] as i32) - 8192) as c_int,
                    ..Default::default()
                },
            )),
            _ => None,
        }
    }
}

/// The default value a control port should start at, from its range hints.
/// `sample_rate` matters for ports whose bounds are relative to it.
pub fn default_for(hint: &LADSPA_PortRangeHint, sample_rate: u32) -> f32 {
    let scale = if hint.hint_descriptor & HINT_SAMPLE_RATE != 0 { sample_rate as f32 } else { 1.0 };
    let lower = hint.lower_bound * scale;
    let upper = hint.upper_bound * scale;
    let log = hint.hint_descriptor & HINT_LOGARITHMIC != 0;
    let between = |a: f32, b: f32| {
        if log && a > 0.0 && b > 0.0 {
            (a.ln() * (1.0 - 0.5) + b.ln() * 0.5).exp()
        } else {
            a * 0.5 + b * 0.5
        }
    };
    match hint.hint_descriptor & HINT_DEFAULT_MASK {
        HINT_DEFAULT_MINIMUM => lower,
        HINT_DEFAULT_MAXIMUM => upper,
        HINT_DEFAULT_LOW => {
            if log && lower > 0.0 && upper > 0.0 {
                (lower.ln() * 0.75 + upper.ln() * 0.25).exp()
            } else {
                lower * 0.75 + upper * 0.25
            }
        }
        HINT_DEFAULT_MIDDLE => between(lower, upper),
        HINT_DEFAULT_HIGH => {
            if log && lower > 0.0 && upper > 0.0 {
                (lower.ln() * 0.25 + upper.ln() * 0.75).exp()
            } else {
                lower * 0.25 + upper * 0.75
            }
        }
        HINT_DEFAULT_0 => 0.0,
        HINT_DEFAULT_1 => 1.0,
        HINT_DEFAULT_100 => 100.0,
        HINT_DEFAULT_440 => 440.0,
        // No hint: the low end of whatever range is known, else silence.
        _ => {
            if hint.hint_descriptor & HINT_BOUNDED_BELOW != 0 {
                lower
            } else {
                0.0
            }
        }
    }
}

/// Sane display bounds for a control port, used for the UI's 0..1 knob.
pub fn bounds(hint: &LADSPA_PortRangeHint, sample_rate: u32) -> (f32, f32) {
    let scale = if hint.hint_descriptor & HINT_SAMPLE_RATE != 0 { sample_rate as f32 } else { 1.0 };
    let lower = if hint.hint_descriptor & HINT_BOUNDED_BELOW != 0 {
        hint.lower_bound * scale
    } else {
        0.0
    };
    let upper = if hint.hint_descriptor & HINT_BOUNDED_ABOVE != 0 {
        hint.upper_bound * scale
    } else {
        (lower + 1.0).max(1.0)
    };
    if upper > lower { (lower, upper) } else { (lower, lower + 1.0) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The struct the plugin reads must be exactly ALSA's 28-byte event.
    #[test]
    fn seq_event_has_the_alsa_layout() {
        assert_eq!(std::mem::size_of::<snd_seq_event_t>(), 28);
        assert_eq!(std::mem::size_of::<snd_seq_ev_note_t>(), 8);
        assert_eq!(std::mem::size_of::<snd_seq_ev_ctrl_t>(), 12);
    }

    #[test]
    fn midi_maps_to_the_right_event_types() {
        let on = snd_seq_event_t::from_midi([0x90, 60, 100], 0).unwrap();
        assert_eq!(on.type_, SND_SEQ_EVENT_NOTEON);
        assert_eq!(on.data.raw[1], 60);
        assert_eq!(on.data.raw[2], 100);
        // Note-on at velocity 0 is a note-off.
        let off = snd_seq_event_t::from_midi([0x90, 60, 0], 0).unwrap();
        assert_eq!(off.type_, SND_SEQ_EVENT_NOTEOFF);
        assert_eq!(snd_seq_event_t::from_midi([0x80, 60, 0], 0).unwrap().type_, SND_SEQ_EVENT_NOTEOFF);
        assert_eq!(
            snd_seq_event_t::from_midi([0xB0, 7, 64], 0).unwrap().type_,
            SND_SEQ_EVENT_CONTROLLER
        );
        assert!(snd_seq_event_t::from_midi([0xF0, 0, 0], 0).is_none());
    }

    #[test]
    fn defaults_follow_the_range_hints() {
        let h = |d: i32, lo: f32, hi: f32| LADSPA_PortRangeHint {
            hint_descriptor: d,
            lower_bound: lo,
            upper_bound: hi,
        };
        assert_eq!(default_for(&h(HINT_DEFAULT_MINIMUM | HINT_BOUNDED_BELOW, -6.0, 6.0), 48_000), -6.0);
        assert_eq!(default_for(&h(HINT_DEFAULT_MIDDLE, 0.0, 10.0), 48_000), 5.0);
        assert_eq!(default_for(&h(HINT_DEFAULT_1, 0.0, 10.0), 48_000), 1.0);
        // Sample-rate-relative bounds scale with the rate.
        assert_eq!(
            default_for(&h(HINT_DEFAULT_MAXIMUM | HINT_SAMPLE_RATE, 0.0, 0.5), 48_000),
            24_000.0
        );
        // No default hint and no lower bound: silence, not a NaN.
        assert_eq!(default_for(&h(HINT_DEFAULT_NONE, 0.0, 0.0), 48_000), 0.0);
    }
}
