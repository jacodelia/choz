//! Note input identity.
//!
//! Every note event carries where it came from, so the UI can route it to the
//! rack slots bound to that input. The RT engine never sees this — it only gets
//! per-slot note commands.

/// Where a note event came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputSource {
    /// Hardware MIDI input, by index into the connected-port list.
    Midi(usize),
    /// The OSC listener.
    Osc,
    /// The computer keyboard (QWERTY piano). Always plays the active rack tab.
    Keyboard,
}

/// A parsed note event plus its origin. Velocity 0 with `on = false` is a
/// note-off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoteMsg {
    pub source: InputSource,
    pub on: bool,
    pub note: u8,
    pub vel: u8,
}

/// A MIDI control-change message, used for MIDI learn (rack faders).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CcMsg {
    pub source: InputSource,
    pub cc: u8,
    pub value: u8,
}

/// A remote-control message (OSC only): change something the user could also
/// change in the UI. `tab` and `fx` are 1-based, as they read on screen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ControlMsg {
    Gain { tab: usize, value: f32 },
    Pan { tab: usize, value: f32 },
    Mute { tab: usize, on: bool },
    FxParam { tab: usize, fx: usize, param: usize, value: f32 },
}

/// Anything an input thread can hand the UI loop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InputEvent {
    Note(NoteMsg),
    Cc(CcMsg),
    Control(ControlMsg),
}
