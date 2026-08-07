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

/// A MIDI control-change message. Drives MIDI learn (rack faders) *and* is
/// forwarded to the instruments bound to the same input, so pedals (sustain,
/// sostenuto, soft, expression) and the modulation wheel reach the synth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CcMsg {
    pub source: InputSource,
    pub cc: u8,
    pub value: u8,
}

/// A pitch-bend message. `value` is the raw 14-bit wire value: 0..16383,
/// centred at 8192.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BendMsg {
    pub source: InputSource,
    pub value: u16,
}

/// A program change, with the bank the last Bank Select (CC 0 / CC 32) chose.
/// Controllers send bank select and program change as a set, so they travel
/// together rather than as three unrelated messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProgramMsg {
    pub source: InputSource,
    pub bank: u8,
    pub program: u8,
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
    Program(ProgramMsg),
    Bend(BendMsg),
    Control(ControlMsg),
}
