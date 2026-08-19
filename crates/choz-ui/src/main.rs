//! choz — A terminal-based audio plugin host inspired by Carla.
//!
//! Provides a TUI for managing audio sources (MIDI, SF2, audio files, plugins)
//! and FX chains, feeding a real-time audio engine via cpal.
//!
//! UI styling adapted from seqterm.

mod arp;
mod automation;
mod editor;
mod file_browser;
mod fx_presets;
mod i18n;
mod log;
mod logo;
mod menu;
mod project;
mod settings;
mod source;
mod spectrum;
mod views;

use choz_engine::fx_chain::FxSpec;
use choz_engine::{engine, midi, sources};

use std::cell::RefCell;
use std::io;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, MouseButton,
        MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame, Terminal,
};

use source::{AudioFxEntry, AudioSource, ALL_FX_KINDS};
use views::fx_chain_panel::{RackButton, RackLayout};
use views::splash::{draw_splash, is_active, SplashState};
use views::theme::*;

/// One rack slot: a source and its FX chain. Mirrors an engine slot at the same
/// index. `app.{source, fx_chain, fx_slot, fx_param}` are the live working copy
/// of the *active* slot; they're persisted back here on tab switch / removal.
/// A note input a rack tab can be bound to. MIDI ports are identified by name so
/// the binding survives a rescan (port indices don't).
#[derive(Clone, PartialEq, Eq, Debug)]
enum InputRef {
    Midi(String),
    Osc,
}

impl InputRef {
    fn kind(&self) -> &'static str {
        match self {
            InputRef::Midi(_) => "MIDI",
            InputRef::Osc => "OSC",
        }
    }

    fn name(&self) -> &str {
        match self {
            InputRef::Midi(n) => n,
            InputRef::Osc => "OSC",
        }
    }
}

/// One MIDI-learn assignment: which controller, which CC, what it moves.
///
/// **The controller is part of the binding.** Two keyboards on stage both send
/// CC 1; without the source, the KeyStep's mod wheel would drive whatever the
/// Keystation's was assigned to. `None` means "any source" — what a binding
/// learned before this existed says, and what the QWERTY piano and OSC use.
#[derive(Clone, Debug, PartialEq)]
struct CcBinding {
    source: Option<InputRef>,
    cc: u8,
    target: LearnTarget,
}

#[derive(Clone)]
struct RackSlot {
    /// MIDI channel this tab answers, 1..16 — **or 0, meaning any**.
    ///
    /// In MULTI it is what a tab *is*, and every tab has a number. In LIVE it is
    /// opt-in: a tab left on `ANY` behaves as it always did (the active one
    /// plays), and giving it a number turns one port into a split — a keyboard
    /// sending channel 3 reaches the tab that asked for 3, whether or not it is
    /// the tab on screen.
    channel: u8,
    /// Turn this tab's audio input into notes for its own instrument.
    pitch_to_midi: bool,
    /// Trim on the audio coming in, and how loud it has to be before `A→M`
    /// hears a note. A guitar into a preamp is nowhere near a synth's level,
    /// so without the trim the two are stuck wherever the interface left them.
    in_gain: f32,
    in_gate: f32,
    /// Which note input drives this tab. `None` = only the QWERTY piano (which
    /// always plays the active tab) reaches it.
    input: Option<InputRef>,
    /// A MIDI port this tab also plays to, by name. `None` — the usual — is a
    /// tab that ends at its own instrument.
    ///
    /// The name rather than an index: ports come and go, and an index into a
    /// list that changed while choz was closed points at somebody else's synth.
    midi_out: Option<String>,
    source: AudioSource,
    fx_chain: Vec<AudioFxEntry>,
    /// Mixer strip. `solo` is a UI-only concept: it's folded into the mute flag
    /// sent to the engine (any solo → everything else is muted).
    /// Output level, one per side. A stereo instrument that sits louder on one
    /// channel is trimmed here rather than by cheating with the pan; `link`
    /// keeps the two equal, which is what a tab wants nearly always and so is
    /// what it starts as.
    gain: f32,
    gain_r: f32,
    link: bool,
    pan: f32,
    mute: bool,
    solo: bool,
    /// Device output channels this tab plays out of, 0-based. `(0, 1)` is the
    /// first pair; anything else needs the native JACK backend.
    out_pair: (usize, usize),
    /// Device *input* channels feeding this tab, 0-based, or `None` when the
    /// tab plays its instrument. A tab fed by live audio ignores its
    /// instrument and runs the capture pair through its FX chain.
    in_pair: Option<(usize, usize)>,
    /// SF2 slots only: the programs in the loaded SoundFont, and the cursor into
    /// them. Empty for every other source kind.
    presets: Vec<sources::Sf2Preset>,
    preset_cursor: usize,
    /// DSSI instruments only: the `configure` key/values this tab was given.
    /// Applied when the plugin is built — see [`choz_engine::AudioEngine::load_dssi`].
    dssi_config: Vec<(String, String)>,
    /// Plugin-instrument slots only: the patches the plugin publishes through
    /// its own browser. Shares `preset_cursor` with the SoundFont list — a tab
    /// has one instrument, so only one of the two is ever populated.
    plugin_presets: Vec<choz_engine::PresetEntry>,
    /// The folder of preset **files** this tab is using as its bank, when the
    /// plugin has no browser of its own to ask. Surge XT's VST3 build reports
    /// no programs at all and keeps its 637 factory patches as `.fxp` files, so
    /// without this the tab has no bank and no way to reach any of them.
    preset_dir: Option<std::path::PathBuf>,
    /// Plugin-instrument slots only: what the plugin exposes, and the current
    /// knob positions (0..1, same order). Empty for every other source kind.
    instr_params: Vec<choz_engine::PluginParam>,
    instr_values: Vec<f32>,
    /// The window of instrument knobs the last draw put on screen, as
    /// `(first parameter, how many the plugin has)`. Compared against the next
    /// draw to re-address the CCs learned on that box when the window moves.
    /// `None` until this tab has been drawn once, and reset whenever the
    /// parameter list changes — a new instrument is not a window that moved.
    instr_window: Option<(usize, usize)>,
    /// The instrument plugin's own state (its patch), as the project stores it.
    /// Kept here so a tab survives everything that rebuilds engine slots — an
    /// output-device change, a project load — with the sound it had.
    instr_state: Vec<u8>,
    /// This tab's arpeggiator. Off by default, and when it is off a note passes
    /// through exactly as it did before it existed.
    arp: arp::Arp,
    /// With `A→M` on, how much of what comes out is the instrument rather than
    /// the audio that drove it. 1 = only the instrument, which is what the
    /// converter did before there was a choice.
    pitch_mix: f32,
}

impl RackSlot {
    fn new(source: AudioSource) -> Self {
        RackSlot {
            // Any channel until something says otherwise: `push_slot` numbers
            // the tabs in MULTI (an orchestral template's default layout), and
            // in LIVE a number is opt-in — it turns one port into a split.
            channel: ANY_CHANNEL,
            midi_out: None,
            pitch_to_midi: false,
            in_gain: 1.0,
            in_gate: choz_engine::pitch::DEFAULT_GATE,
            input: None,
            source,
            fx_chain: Vec::new(),
            gain: 1.0,
            gain_r: 1.0,
            link: true,
            pan: 0.0,
            mute: false,
            solo: false,
            out_pair: (0, 1),
            in_pair: None,
            presets: Vec::new(),
            preset_cursor: 0,
            dssi_config: Vec::new(),
            plugin_presets: Vec::new(),
            preset_dir: None,
            instr_params: Vec::new(),
            instr_values: Vec::new(),
            instr_window: None,
            instr_state: Vec::new(),
            arp: arp::Arp::default(),
            pitch_mix: 1.0,
        }
    }
}

/// A load the interface has promised but not run yet. See [`App::pending_load`].
#[derive(Debug, Clone)]
enum PendingLoad {
    Synth(usize),
    Source(std::path::PathBuf),
}

/// Which half of a strip a level change is aimed at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MixSide {
    Left,
    Right,
    /// Both, whatever the link says — what the RACK's own `VOL` knob does.
    Both,
}

/// A discovered synthesizer plugin.
#[derive(Debug, Clone)]
pub struct SynthEntry {
    pub id: String,
    pub format: choz_engine::PluginFormat,
    pub name: String,
    pub path: std::path::PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    /// The IN drawer (note inputs), only reachable while it is open.
    Source,
    FxChain,
    Transport,
    /// The OUT drawer (audio output devices), only while it is open.
    Output,
    /// The MIXER at the bottom, only while that tab is the one showing: the
    /// arrows are worth more there than anywhere else, because a level is the
    /// one thing you set while looking somewhere else entirely.
    Mixer,
}

/// What a row of the OUT drawer points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutTarget {
    /// A section header — not selectable.
    None,
    /// Index into `App::out_devices`.
    Device(usize),
    /// A MIDI port the active tab also plays to. Index into `midi_out_ports`.
    MidiOut(usize),
    /// One output channel of the device. A tab's two sides are picked
    /// separately, so "left out of 3, right out of 9" is a routing like any
    /// other — an interface's jacks are not glued together in pairs.
    Channel(usize),
}

/// What a gesture on a channel row does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Assign {
    /// Left click: put this channel on the tab.
    On,
    /// Right click: take it off.
    Off,
    /// Enter or Space: whichever of the two this channel is not already.
    Toggle,
}

/// A routing is one or two jacks: `(a, a)` is one, `(a, b)` is two.
fn channels_of(pair: (usize, usize)) -> Vec<usize> {
    if pair.0 == pair.1 {
        vec![pair.0]
    } else {
        vec![pair.0, pair.1]
    }
}

/// `pair` with `ch` added. Two jacks is all a tab has, so this is a queue of
/// two: the newcomer is the right side and the oldest falls off the left.
///
/// That is what makes the common gesture work. A new tab starts on 1 and 2, so
/// clicking 3 and then 9 leaves it on **3 and 9** — with the left side pinned
/// instead, clicking twice would have left channel 1 in there.
fn assign_channel(pair: (usize, usize), ch: usize) -> (usize, usize) {
    if channels_of(pair).contains(&ch) {
        return pair;
    }
    (pair.1, ch)
}

/// `pair` with `ch` removed. `None` when that was the only jack left — for an
/// input that means "back to the instrument", and for an output there is
/// nothing sensible to do, so the caller keeps what it had.
fn unassign_channel(pair: (usize, usize), ch: usize) -> Option<(usize, usize)> {
    match (pair.0 == ch, pair.1 == ch) {
        (true, true) => None,
        (true, false) => Some((pair.1, pair.1)),
        (false, true) => Some((pair.0, pair.0)),
        (false, false) => Some(pair),
    }
}

/// What a channel is to a routing: `"  L"`, `"  R"`, `"  L+R"` — or nothing at
/// all when the tab does not use it.
fn side_label(pair: Option<(usize, usize)>, ch: usize) -> &'static str {
    match pair {
        Some((l, r)) if l == ch && r == ch => "  L+R",
        Some((l, _)) if l == ch => "  L",
        Some((_, r)) if r == ch => "  R",
        _ => "",
    }
}

/// What Enter on an IN drawer row does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InTarget {
    /// A section header — not selectable.
    None,
    /// Index into `App::input_list()`: a note input (MIDI port or OSC).
    Note(usize),
    /// One capture channel of the device feeds the tab. Picked per channel, so
    /// a guitar on input 5 is one jack rather than half of a pair.
    Channel(usize),
    /// Back to the tab's own instrument.
    NoCapture,
}

/// Which picker the open modal is. Every one of them draws through
/// What the scanning thread sends back. The scan walks one directory per
/// child process, so `Step` lands between children and `Done` exactly once.
enum ScanMsg {
    Step {
        done: usize,
        total: usize,
        label: String,
    },
    Done(Vec<choz_engine::FoundPlugin>),
}

/// A plugin rescan running off the UI thread.
///
/// The scan spawns a child process per directory and can take tens of seconds
/// on a full plugin collection; doing it inline froze the whole TUI — no
/// redraw, no keys, and (worse) no arpeggiator clock — until it finished.
struct ScanJob {
    rx: std::sync::mpsc::Receiver<ScanMsg>,
    /// Last reported position, so the bar holds its value between messages
    /// instead of flickering back to zero on every frame.
    done: usize,
    total: usize,
    /// The directory being walked, already shortened for display.
    label: String,
}

/// `views::modal::draw_list_modal`, so they share the scrollbar, the
/// SELECT/CANCEL buttons and one set of mouse rects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModalKind {
    /// Instrument for the active rack tab, filtered by plugin/file format.
    Source,
    /// Add an FX to the active chain (built-ins, then CLAP effects).
    AddFx,
    /// SF2 bank/preset of the active tab.
    Preset,
    /// Pick which rack control the next MIDI CC will drive.
    Learn,
    /// Filesystem browser (SF2/WAV), navigable into directories.
    Browser,
    /// Live parameters of the active tab's plugin instrument.
    InstrParams,
    /// Settings \u{2192} Plugin paths: the per-format scan directories.
    PluginPaths,
    /// Directory picker that adds a path to `paths_format`.
    AddPath,
    /// Directory picker for File \u{2192} Save project.
    SaveProject,
    LoadProject,
    /// Tempo, time signature, sound and level of the metronome.
    Metronome,
    /// Folder picker for a plugin's bank of preset **files** — the tab has an
    /// instrument that reports no programs of its own, so its patches are
    /// `.fxp` / `.vstpreset` files on disk.
    Bank,
    /// Image picker for the desktop background (Settings \u{2192} THEME).
    Wallpaper,
    /// Factory presets of the selected built-in effect.
    FxPreset,
    /// The positions of one **named** arpeggiator knob: the mode, the division,
    /// which sequence, how long it plays.
    ArpChoice,
    /// Pick a Max/MSP patch to import into the active tab's chain.
    ImportMax,
    /// What that import kept, and — the half that matters — what it could not.
    MaxReport,
    /// The positions of one **named** FX parameter — a preset, a key, a scale,
    /// a mode. Stepping through eighteen Winamp presets with an arrow key is a
    /// list pretending to be a knob; this is the list.
    FxChoice,
}

/// Tabs of the Settings modal, in chip order.
const SETTINGS_TABS: &[&str] = &["AUDIO", "THEME", "LANGUAGE"];
/// What a row on the THEME tab does when it is picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThemeRow {
    /// Index into [`settings::THEMES`].
    Scheme(usize),
    /// Cycle terminal default → flat colour of the current scheme.
    Background,
    /// Stretch ↔ tile, only present while an image is set.
    Fit,
    /// How strongly the panel colour washes over the desktop, `←`/`→`. Absent
    /// on the terminal's own background: choz cannot read that colour, so
    /// there is nothing to be translucent against.
    Tint,
    /// Which colour that wash is. The scheme's own, or any palette entry —
    /// `←`/`→`, because it is judged by looking at it.
    PanelColor,
    /// Open the file browser on the project's `assets/`.
    PickImage,
    /// Back to the terminal's own background.
    Clear,
    /// Apply and leave, for anyone driving the modal from the keyboard: Enter
    /// on the other rows keeps it open on purpose, so that trying a few schemes
    /// does not mean reopening Settings each time.
    Done,
}

/// Below this peak an input jack reads `--` instead of a number: -90 dBFS,
/// which is under every converter's idle noise floor and over anything a player
/// can produce. A live ADC is never truly at zero.
const SILENCE: f32 = 3.163e-5;

/// Width of the tint slider's bar, in cells.
const TINT_BAR_WIDTH: usize = 20;
/// How much one press (or one click of the wheel) moves a level — the same
/// step the RACK's `VOL` knob uses, so the answer does not depend on where you
/// touched it.
const GAIN_STEP: f32 = 0.05;

/// How much one arrow press moves the tint.
const TINT_STEP: u8 = 5;

const TAB_AUDIO: usize = 0;
const TAB_THEME: usize = 1;
const TAB_LANG: usize = 2;

/// Sub-categories of the AUDIO tab, shown in the modal's sidebar — the same
/// split seqterm's AUDIO SETTINGS uses.
const AUDIO_SECTIONS: &[&str] = &["Engine", "Plugin Paths", "OSC"];
const SEC_ENGINE: usize = 0;
const SEC_PATHS: usize = 1;
const SEC_OSC: usize = 2;

/// Bank Select MSB/LSB. Never a MIDI-learn source: it only ever precedes a
/// program change.
const BANK_SELECT_CCS: [u8; 2] = [0, 32];

/// How many MIDI messages the monitor keeps. A few more than fit on screen, so
/// the panel still fills up when the terminal is tall.
const MIDI_LOG_MAX: usize = 64;

/// Editable rows of the Engine section, in display order.
const ENGINE_ROWS: &[&str] = &[
    "Backend",
    "Device",
    "Input",
    "Sample rate",
    "Buffer size",
    "Tempo",
    "Time signature",
    "Feedback guard",
    "SF2 engine",
];

/// Time signatures the Engine row cycles through. Every denominator here is a
/// note value; the plugin formats have no way to read anything else.
const TIME_SIGS: &[(u16, u16)] = &[
    (4, 4),
    (3, 4),
    (2, 4),
    (6, 8),
    (5, 4),
    (7, 8),
    (12, 8),
    (2, 2),
];

/// How much one arrow press moves the tempo.
const BPM_STEP: f32 = 1.0;
/// Editable rows of the OSC section.
const OSC_ROWS: &[&str] = &["Enable OSC", "Port mode", "UDP port", "TCP port"];

/// Format chips of the ADD FX modal.
const FX_FORMATS: &[&str] = &[
    "ALL", "BUILT-IN", "CLAP", "LV2", "VST2", "VST3", "LADSPA", "DSSI", "PD",
];

/// One offer in the ADD FX list: a built-in or a scanned plugin.
struct FxMenuEntry {
    /// `None` for choz's own DSP.
    format: Option<choz_engine::PluginFormat>,
    category: source::FxCategory,
    label: String,
    /// Whether choz can actually load it today.
    hosted: bool,
}

impl FxMenuEntry {
    fn matches_filter(&self, wanted: &str) -> bool {
        match (wanted, self.format) {
            ("ALL", _) => true,
            ("BUILT-IN", None) => true,
            (w, Some(f)) => f.label() == w,
            _ => false,
        }
    }
}

/// One entry of the SOURCE picker.
#[derive(Debug, Clone)]
struct SourceChoice {
    fmt: &'static str,
    label: String,
    action: SourceAction,
}

#[derive(Debug, Clone)]
enum SourceAction {
    Plugin {
        format: choz_engine::PluginFormat,
        /// Plugin file, or the bundle directory for LV2.
        path: std::path::PathBuf,
        id: String,
    },
    File(std::path::PathBuf),
    /// Open the file browser for this extension instead of loading directly.
    Browse(&'static [&'static str]),
    /// A format choz can find but not load yet.
    Unsupported(&'static str),
}

/// Formats the SOURCE picker can filter by. Only CLAP/SF2/WAV can actually be
/// loaded today; the rest are listed so it's obvious what choz doesn't host yet.
const SOURCE_FORMATS: &[&str] = &[
    "ALL", "CLAP", "SF2", "WAV", "SFZ", "LV2", "VST2", "VST3", "DSSI", "LADSPA",
];

/// A rack control a MIDI CC can drive, bound by MIDI learn — and the address an
/// automation lane is recorded against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LearnTarget {
    Gain(usize),
    Pan(usize),
    /// Trim on the tab's audio input, and the level `A→M` calls a note. Both
    /// are knobs like any other, so both are bindable and automatable — a
    /// guitarist wants the sensitivity on a pedal, not in a menu.
    InGain(usize),
    InGate(usize),
    FxParam {
        slot: usize,
        fx: usize,
        param: usize,
    },
    /// A parameter of the tab's own plugin instrument. Any parameter of any
    /// hosted plugin is bindable, not just the FX chain's.
    InstrParam {
        slot: usize,
        param: usize,
    },
    /// A button rather than a fader: fired by a CC crossing half-scale, so a
    /// pad, a footswitch or the top half of a fader all work.
    Trigger(TriggerAction),
}

/// The rack tab a target moves, or `None` for the rack-wide buttons.
fn target_slot(t: &LearnTarget) -> Option<usize> {
    match *t {
        LearnTarget::Gain(s)
        | LearnTarget::Pan(s)
        | LearnTarget::InGain(s)
        | LearnTarget::InGate(s) => Some(s),
        LearnTarget::FxParam { slot, .. } | LearnTarget::InstrParam { slot, .. } => Some(slot),
        LearnTarget::Trigger(_) => None,
    }
}

/// The FX unit a target belongs to, or `None` for rack-wide controls. Two
/// bindings on the same CC may coexist only when they live in different units.
fn fx_scope(t: &LearnTarget) -> Option<(usize, usize)> {
    match *t {
        LearnTarget::FxParam { slot, fx, .. } => Some((slot, fx)),
        _ => None,
    }
}

/// Rack buttons a CC can press. `DEL` is deliberately absent — nothing should
/// delete an FX because a fader was nudged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TriggerAction {
    PresetPrev,
    PresetNext,
    /// Page the instrument's knob box backwards / forwards. A plugin with
    /// hundreds of parameters (Surge XT has them) shows a few rows at a time,
    /// and the CCs already learned move with the page — see [`App::page_instr`].
    InstrPagePrev,
    InstrPageNext,
    FxToggle,
    FxMoveLeft,
    FxMoveRight,
    /// Select FX slot `n` of the active chain (the FX CHAIN row).
    FxSelect(usize),
    FxAdd,
    Mute,
    Solo,
    /// The tab's arpeggiator, from a footswitch or a pad: the controls a player
    /// needs with both hands busy. Everything else on the ARP line is a
    /// setting, and a setting is not worth a pedal.
    ArpToggle,
    ArpTap,
    ArpLatch,
    /// The sequencer's transport, from back when the arpeggiator had one.
    /// Kept so a project saved with these bindings still loads — an unknown
    /// variant is a parse error, and a parse error is a lost rack. They are not
    /// offered by the picker and do nothing.
    ArpPlayPause,
    ArpStop,
    ArpRecord,
}

impl TriggerAction {
    fn label(self) -> String {
        match self {
            TriggerAction::PresetPrev => "BANK \u{25C0}".to_string(),
            TriggerAction::PresetNext => "BANK \u{25B6}".to_string(),
            TriggerAction::InstrPagePrev => "PARAMS \u{25C0}".to_string(),
            TriggerAction::InstrPageNext => "PARAMS \u{25B6}".to_string(),
            TriggerAction::FxToggle => "FX ON/OFF".to_string(),
            TriggerAction::FxMoveLeft => "FX \u{25C0} MOVE".to_string(),
            TriggerAction::FxMoveRight => "FX MOVE \u{25B6}".to_string(),
            TriggerAction::FxSelect(i) => format!("select FX {}", i + 1),
            TriggerAction::FxAdd => "ADD FX".to_string(),
            TriggerAction::Mute => "MUTE".to_string(),
            TriggerAction::Solo => "SOLO".to_string(),
            TriggerAction::ArpToggle => "ARP ON/OFF".to_string(),
            TriggerAction::ArpTap => "ARP TAP".to_string(),
            TriggerAction::ArpLatch => "ARP HOLD".to_string(),
            TriggerAction::ArpPlayPause | TriggerAction::ArpStop | TriggerAction::ArpRecord => {
                "(retired)".to_string()
            }
        }
    }
}

/// One line of text being typed in a modal: the buffer and where the caret is.
/// Shared by the Plugin paths editor and the SAVE PROJECT name prompt.
#[derive(Debug, Clone, Default)]
struct TextEdit {
    buf: String,
    /// Caret position, in characters.
    cursor: usize,
}

impl TextEdit {
    fn new(buf: String) -> Self {
        let cursor = buf.chars().count();
        Self { buf, cursor }
    }

    /// The buffer with a block caret drawn at the cursor.
    fn caret(&self) -> String {
        let mut out: String = self.buf.chars().take(self.cursor).collect();
        out.push('\u{2588}');
        out.extend(self.buf.chars().skip(self.cursor));
        out
    }

    /// Apply a key to the buffer. Returns `Some(commit)` when the edit ends.
    fn key(&mut self, key: KeyCode) -> Option<bool> {
        match key {
            KeyCode::Char(c) => {
                let byte = self
                    .buf
                    .char_indices()
                    .nth(self.cursor)
                    .map(|(i, _)| i)
                    .unwrap_or(self.buf.len());
                self.buf.insert(byte, c);
                self.cursor += 1;
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    let byte = self
                        .buf
                        .char_indices()
                        .nth(self.cursor - 1)
                        .map(|(i, _)| i)?;
                    self.buf.remove(byte);
                    self.cursor -= 1;
                }
            }
            KeyCode::Delete => {
                if let Some((byte, _)) = self.buf.char_indices().nth(self.cursor) {
                    self.buf.remove(byte);
                }
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.buf.chars().count()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.buf.chars().count(),
            KeyCode::Enter => return Some(true),
            KeyCode::Esc => return Some(false),
            _ => {}
        }
        None
    }
}

/// A directory being typed in the Plugin paths modal. `dir` is the index of the
/// row being rewritten, or `None` when it's a brand new entry.
#[derive(Debug, Clone)]
struct PathEdit {
    fmt: choz_engine::PluginFormat,
    dir: Option<usize>,
    text: TextEdit,
}

impl PathEdit {
    fn new(fmt: choz_engine::PluginFormat, dir: Option<usize>, buf: String) -> Self {
        Self {
            fmt,
            dir,
            text: TextEdit::new(buf),
        }
    }

    /// The line the modal shows while editing, with a caret at the cursor and
    /// the format the path is being filed under (getting that wrong is the easy
    /// mistake: an SF2 folder under SFZ finds nothing).
    fn display(&self) -> String {
        format!("    \u{270E} [{}] {}", self.fmt.label(), self.text.caret())
    }
}

/// The file name being typed once SAVE PROJECT has a directory. Overwriting is
/// a separate keypress: a project is somebody's whole set, and the browser
/// picking a folder is not consent to replace what is already in it.
struct SaveName {
    dir: std::path::PathBuf,
    text: TextEdit,
    /// The target already exists and Enter is now the confirmation.
    confirm: bool,
    /// Why the last save failed, kept on screen instead of dying in stderr.
    error: Option<String>,
}

impl SaveName {
    /// `name` is what the prompt starts with — the project's own file name when
    /// it has one, so Save as suggests overwriting itself rather than a name
    /// nobody chose.
    fn new(dir: std::path::PathBuf, name: String) -> Self {
        Self {
            dir,
            text: TextEdit::new(name),
            confirm: false,
            error: None,
        }
    }

    /// Where Enter would write. A bare name gets `.yml` so the browser (and
    /// `Project::load`) still recognises it.
    fn target(&self) -> std::path::PathBuf {
        let mut name = self.text.buf.trim().to_string();
        if !name.contains('.') {
            name.push_str(".yml");
        }
        self.dir.join(name)
    }

    /// The modal's note line: the prompt, the overwrite question, or the error.
    fn note(&self) -> String {
        if let Some(e) = &self.error {
            return format!("  \u{26A0} {e}  \u{00B7}  Enter=retry  Esc=cancel");
        }
        if self.confirm {
            return format!(
                "  \u{26A0} {} \u{00B7} \u{2191}\u{2193} then Enter  \u{00B7}  Esc=rename",
                self.target().display()
            );
        }
        format!(
            "  name: {}  \u{00B7}  in {}  \u{00B7}  Enter=save  Esc=back",
            self.text.caret(),
            self.dir.display()
        )
    }
}

/// A port being typed in the OSC section. `row` is the OSC row it belongs to.
#[derive(Debug, Clone)]
struct PortEdit {
    row: usize,
    buf: String,
}

/// The open modal: what it picks, its list state, and the data behind it.
struct Modal {
    kind: ModalKind,
    list: views::modal::ListModal,
    sources: Vec<SourceChoice>,
    targets: Vec<LearnTarget>,
    browser: Option<file_browser::FileBrowser>,
    /// Which FX parameter a [`ModalKind::FxChoice`] is picking a position for.
    fx_param: usize,
}

impl Modal {
    fn new(kind: ModalKind, list: views::modal::ListModal) -> Self {
        Self {
            kind,
            list,
            sources: Vec::new(),
            targets: Vec::new(),
            browser: None,
            fx_param: 0,
        }
    }
}

#[derive(Default, Clone)]
struct UiLayout {
    source_area: Rect,
    fx_chain_area: Rect,
    transport_area: Rect,
    /// Everything inside the RACK panel, computed by the panel while it draws.
    rack: RackLayout,
    play_btn_rect: Rect,
    stop_btn_rect: Rect,
    /// Clickable areas of the open modal, whichever one it is.
    modal_rects: views::modal::ModalRects,
    /// Top menu-bar title rects, one per MenuKind.
    menu_bar_rects: Vec<Rect>,
    /// Open dropdown item rects: (real item index, rect).
    menu_item_rects: Vec<(usize, Rect)>,
    /// The OUT drawer, open or a handle.
    output_area: Rect,
    /// Close buttons of the open drawers (`None` while they are shut).
    in_close_rect: Option<Rect>,
    out_close_rect: Option<Rect>,
    /// Device-list rects of the open OUT drawer: (device index, rect).
    output_item_rects: Vec<(usize, Rect)>,
    /// Input-list rects: (input index, rect).
    input_item_rects: Vec<(usize, Rect)>,
    /// The connect/disconnect mark at the left of each input row.
    input_mark_rects: Vec<(usize, Rect)>,
    /// The INPUTS panel's rescan button.
    input_scan_rect: Option<Rect>,
    /// The OUT line in the transport panel (click = open the device picker).
    out_device_rect: Option<Rect>,
    /// The PANIC button in the TRANSPORT panel.
    panic_rect: Option<Rect>,
    /// The automation loop's length, in bars. Clickable: the left half of the
    /// cell shortens it, the right half lengthens it.
    loop_rect: Option<Rect>,
    /// The internal/external clock switch of the TRANSPORT panel.
    clock_rect: Option<Rect>,
    /// The metronome's switch and the `\u{25BE}` that opens its menu, drawn on the
    /// menu bar just left of LIVE/MULTI.
    met_rect: Option<Rect>,
    met_menu_rect: Option<Rect>,
    /// The LIVE/MULTI switch in the top-right corner of the menu bar.
    mode_switch_rect: Option<Rect>,
    /// About dialog close-button rect.
    about_close_rect: Option<Rect>,
    /// The MIDI monitor's tab strip: MIDI / WAVE / ACTIVITY.
    monitor_tabs: Vec<(views::midi_monitor::MonitorTab, Rect)>,
    /// The MIXER tab's controls, while it is the one showing.
    mixer_hits: Vec<views::midi_monitor::MixerRect>,
}

#[allow(dead_code)]
struct App {
    source: AudioSource,
    /// Every MIDI input port seen at the last scan (connected or not).
    midi_ports: Vec<String>,
    /// Decoded background image, rebuilt when the file or the terminal size
    /// changes. `None` until something needs it.
    wallpaper: Option<views::background::Wallpaper>,
    /// The kitty-protocol wallpaper currently on screen, when the terminal can
    /// draw one. `Some` means the picture lives under the text at real pixel
    /// resolution and `ui()` must not paint cell backgrounds over it.
    kitty_bg: Option<views::kitty_bg::Placement>,
    /// One colour per cell of the transmitted picture, for the panels to blend
    /// against — under the graphics protocol the buffer itself holds nothing.
    kitty_cells: Option<Vec<(u8, u8, u8)>>,
    /// The panel rectangles and how strongly each is washed, filled by `ui()`.
    /// Under the graphics protocol they become the translucent mask; the
    /// halfblocks path has already blended them into the cells by then.
    wash_rects: Vec<(ratatui::layout::Rect, f32)>,
    /// What the wash mask on screen was built for.
    kitty_mask: Option<views::kitty_bg::MaskState>,
    /// Ports actually connected, in the order `midi::connect_inputs` returned
    /// them — `InputSource::Midi(i)` indexes into this.
    midi_connected: Vec<String>,
    synths: Vec<SynthEntry>,
    /// Everything the last scan found, all formats. `synths`/the ADD FX list
    /// are views onto this.
    plugins: Vec<choz_engine::FoundPlugin>,
    /// Per-format search directories (Settings \u{2192} Plugin paths).
    plugin_paths: choz_engine::PluginPaths,
    synth_cursor: usize,
    /// Discovered CLAP *audio effects*, offered in the ADD FX modal after the
    /// built-ins.
    fx_plugins: Vec<source::PluginFx>,
    /// Whether a chord is currently being published for a harmoniser, so it is
    /// cleared once rather than on every frame that has nothing to say.
    chord_published: bool,

    /// Rack slots (one per source). Slot at index i mirrors engine slot i.
    slots: Vec<RackSlot>,
    /// Active tab. The fields below are its live working copy.
    active_slot: usize,
    fx_chain: Vec<AudioFxEntry>,
    fx_slot: usize,
    fx_param: usize,

    focus: Focus,
    /// The one open modal, if any (see [`ModalKind`]).
    modal: Option<Modal>,
    /// Open top-bar menu (None = closed).
    menu: Option<menu::MenuState>,
    /// About dialog visibility.
    about_open: bool,
    /// The running plugin rescan, if any. `Some` also means "the progress
    /// modal is up" — there is no separate flag to keep in step.
    scan: Option<ScanJob>,
    /// Pre-rendered logo image protocol (ratatui-image), built at startup.
    logo: Option<ratatui_image::protocol::Protocol>,
    /// Notes sounding right now and **the slots their note-on went to**.
    ///
    /// Routing is resolved per event and depends on which tab is active: the
    /// QWERTY piano always plays the active one, and several tabs on one MIDI
    /// port take turns. So switching tabs while a key was down sent the
    /// note-off to a *different* instrument and left the first one ringing —
    /// which is exactly how TyrellN6 ended up holding a note forever. A
    /// note-off now follows its note-on.
    sounding: Vec<(choz_engine::input::InputSource, u8, Vec<usize>)>,
    /// QWERTY-piano notes currently sounding: (midi_note, ticks_until_auto_off).
    active_notes: Vec<(u8, u8)>,
    /// Note events from every off-thread input (hardware MIDI, OSC). Created
    /// once at startup so reconnecting MIDI doesn't orphan the OSC listener.
    note_tx: flume::Sender<midi::InputEvent>,
    note_rx: flume::Receiver<midi::InputEvent>,
    /// Live MIDI connections (must be kept alive to keep receiving).
    _midi_conns: Vec<midir::MidiInputConnection<()>>,
    /// MIDI input ports the user switched off; skipped when (re)connecting.
    midi_disabled: Vec<String>,
    /// When the port list was last polled for hotplug. A controller plugged in
    /// after startup is invisible until we reconnect, which looked like a dead
    /// keyboard.
    midi_scan_at: Instant,
    /// Last messages seen on any input, oldest first — what the MIDI monitor
    /// shows. Capped at [`MIDI_LOG_MAX`]; older entries fall off the front.
    midi_log: std::collections::VecDeque<midi::InputEvent>,
    /// Cursor into the input list.
    input_cursor: usize,
    /// Side drawers: IN (left, note inputs) and OUT (right, audio devices).
    /// Both start closed so the RACK owns the whole body.
    in_open: bool,
    out_open: bool,
    /// Output devices as of the last time the OUT drawer was opened — cpal
    /// enumeration is far too slow to redo every frame.
    out_devices: Vec<String>,
    out_cursor: usize,
    /// Recent AutoTune pitch error, in cents, oldest first. Filled from the
    /// meter as the panel draws, so watching the pitch costs the audio thread
    /// nothing at all.
    autotune_trace: Vec<f32>,
    /// The graph port behind each input channel, cached from the engine: the
    /// IN drawer redraws every frame and this only changes when the client is
    /// rebuilt.
    in_ports: Vec<String>,
    /// Interface settings (text colour, language).
    ui: settings::UiSettings,
    /// Format whose directory list the AddPath browser is feeding.
    paths_format: Option<choz_engine::PluginFormat>,
    /// In-place path editor of the Plugin paths section.
    path_edit: Option<PathEdit>,
    /// The SAVE PROJECT name prompt, once a directory has been picked.
    save_name: Option<SaveName>,
    /// The file this project was last saved to or loaded from — what plain
    /// "Save project" rewrites without asking anything.
    project_file: Option<std::path::PathBuf>,
    /// In-place numeric editor for an OSC port.
    port_edit: Option<PortEdit>,
    /// Set when the search paths changed, so closing the modal rescans.
    paths_dirty: bool,
    /// Load only the rack from the next project, leaving choz's own settings
    /// (plugin paths, colour, language, audio, OSC) as they are.
    load_rack_only: bool,
    /// The plugin's own window, while one is open. At most one at a time:
    /// two plugin GUIs in a terminal app is more window than anyone asked for.
    editor: Option<editor::EditorWindow>,
    /// Cursor in the instrument's knob box.
    instr_param: usize,
    /// Which knob box the arrows drive.
    rack_focus: RackFocus,
    /// Cursor inside the arpeggiator's knob box.
    arp_param: usize,
    /// MIDI output ports as the OUT drawer lists them.
    midi_out_ports: Vec<String>,
    /// Open MIDI outputs, by port name. Shared: two tabs pointed at the same
    /// synth are one connection, because ALSA hands a port to one client and
    /// the second one would simply fail.
    midi_outs: std::collections::HashMap<String, midi::MidiOut>,
    /// Rack control waiting for a MIDI CC (MIDI learn armed).
    learn: Option<LearnTarget>,
    /// MIDI learn is waiting for the user to *click* the control to bind. While
    /// it is on, the terminal reports bare mouse motion and choz paints a `?`
    /// pointer; both are turned back off as soon as a CC lands or it's cancelled.
    learn_pick: bool,
    /// Same pointer gesture, for choosing which parameter this tab's notes
    /// drive. One click, one target — there is no CC to wait for afterwards.
    /// Last known mouse position, only tracked while `learn_pick` is on.
    mouse: (u16, u16),
    /// MIDI-learn bindings: CC number -> the rack control it drives.
    cc_bindings: Vec<CcBinding>,
    /// Same, for controller buttons that send program change: program number ->
    /// the rack button it presses.
    pc_bindings: Vec<(u8, LearnTarget)>,
    /// A MIDI/OSC-driven FX parameter changed and the chain has to be rebuilt.
    /// Coalesced: one rebuild per input drain, never one per message.
    fx_dirty: bool,
    /// Last value seen per CC, for the rising-edge test button bindings use.
    cc_last: [u8; 128],
    /// UDP port the OSC listener bound to, if it started.
    osc_port: Option<u16>,
    /// The running listener; dropping it frees the port.
    osc: Option<choz_engine::osc::OscHandle>,

    audio_engine: Option<engine::AudioEngine>,

    playing: bool,
    /// Recorded parameter moves, played back against the transport.
    automation: automation::Automation,
    /// When the health of the audio thread was last reported, and what the
    /// counters said then — see [`App::poll_health`].
    health_at: Instant,
    health_seen: (u32, u64, u64),
    /// A load asked for but not started yet, and the name to say while it runs.
    ///
    /// Instantiating a plugin is seconds of blocking work on this thread (Surge
    /// XT reads its whole factory library), and it used to happen inside the
    /// keypress that asked for it — so the interface froze with no explanation
    /// and the last thing drawn was the picker. Deferring it by one frame is
    /// what makes it possible to say "loading" *before* going quiet.
    pending_load: Option<PendingLoad>,
    loading: Option<String>,
    /// Which half of a strip the MIXER's arrows move. Only means anything on a
    /// strip whose sides are not linked.
    mix_side: MixSide,
    /// Which of the monitor's tabs is showing.
    monitor_tab: views::midi_monitor::MonitorTab,
    /// What the KEYS/ROLL tabs draw: which keys are down, and the wheels.
    keyboard: views::midi_monitor::KeyboardState,
    /// The spectrum analyser's state. It lives here because the peak hold has
    /// to survive between redraws, and it is updated only while its tab is on
    /// screen — an FFT nobody is looking at is an FFT nobody should pay for.
    spectrum: spectrum::Spectrum,
    /// The WAVE tab's stack of past traces. Same reasoning as `spectrum`: it is
    /// a history, so it cannot be rebuilt per frame.
    wave: views::midi_monitor::WaveHistory,
    quit: bool,

    layout: RefCell<UiLayout>,

    /// Splash screen state.
    splash: SplashState,
    /// Whether the splash screen has finished.
    splash_done: bool,
}

impl App {
    fn new() -> Self {
        let (note_tx, note_rx) = flume::unbounded();

        Self {
            source: AudioSource::Midi,
            midi_ports: Vec::new(),
            midi_connected: Vec::new(),
            synths: Vec::new(),
            plugins: Vec::new(),
            plugin_paths: choz_engine::PluginPaths::load(),
            synth_cursor: 0,
            fx_plugins: Vec::new(),
            chord_published: false,
            slots: Vec::new(),
            active_slot: 0,
            fx_chain: Vec::new(),
            fx_slot: 0,
            fx_param: 0,
            focus: Focus::FxChain,
            modal: None,
            menu: None,
            about_open: false,
            scan: None,
            logo: logo::build_logo(),
            active_notes: Vec::new(),
            note_tx,
            note_rx,
            _midi_conns: Vec::new(),
            wallpaper: None,
            kitty_bg: None,
            kitty_cells: None,
            wash_rects: Vec::new(),
            kitty_mask: None,
            midi_disabled: Vec::new(),
            midi_scan_at: Instant::now(),
            midi_log: std::collections::VecDeque::new(),
            input_cursor: 0,
            in_open: false,
            out_open: false,
            out_devices: Vec::new(),
            in_ports: Vec::new(),
            autotune_trace: vec![f32::NAN; AUTOTUNE_TRACE],
            out_cursor: 0,
            // Loaded here, applied by `main` — `apply()` sets process-wide
            // state (language, text colour) that tests must not inherit.
            ui: settings::UiSettings::load(),
            paths_format: None,
            path_edit: None,
            save_name: None,
            project_file: None,
            port_edit: None,
            paths_dirty: false,
            load_rack_only: false,
            editor: None,
            sounding: Vec::new(),
            instr_param: 0,
            rack_focus: RackFocus::default(),
            arp_param: 0,
            midi_out_ports: midi::list_output_ports(),
            midi_outs: std::collections::HashMap::new(),
            learn: None,
            learn_pick: false,
            mouse: (0, 0),
            cc_bindings: Vec::new(),
            pc_bindings: Vec::new(),
            fx_dirty: false,
            cc_last: [0; 128],
            osc_port: None,
            osc: None,
            audio_engine: None,
            playing: false,
            automation: automation::Automation::default(),
            health_at: Instant::now(),
            health_seen: (0, 0, 0),
            pending_load: None,
            loading: None,
            mix_side: MixSide::Both,
            monitor_tab: views::midi_monitor::MonitorTab::default(),
            keyboard: views::midi_monitor::KeyboardState::default(),
            spectrum: spectrum::Spectrum::new(),
            wave: views::midi_monitor::WaveHistory::default(),
            quit: false,
            layout: RefCell::new(UiLayout::default()),
            splash: SplashState::new(),
            splash_done: false,
        }
    }

    /// Scan every configured plugin directory (all formats). Called after the
    /// engine starts and whenever the user asks for a refresh; `force` skips the
    /// on-disk cache.
    fn discover_synths(&mut self, force: bool) {
        let Some(engine) = self.audio_engine.as_ref() else {
            return;
        };
        let paths = self.plugin_paths.clone();
        let found = if force {
            engine.rescan_plugins(&paths)
        } else {
            engine.cached_plugins(&paths)
        };
        self.apply_plugins(found);
    }

    /// Start a rescan on a background thread and put the progress modal up.
    /// A second call while one is running is ignored: two scans would fight
    /// over the same cache file for no gain.
    fn start_rescan(&mut self) {
        if self.scan.is_some() || self.audio_engine.is_none() {
            return;
        }
        let paths = self.plugin_paths.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        // The scan needs nothing from the engine — `rescan_plugins` only calls
        // the free `scan_all` — so the thread carries the paths and nothing
        // else, and no lock is shared with the audio side.
        std::thread::spawn(move || {
            let found = choz_engine::cache::rescan(|| {
                choz_engine::scan_all_with_progress(&paths, |step| {
                    // A closed channel means the UI is gone; the scan still
                    // finishes and writes its cache, which is the cheap and
                    // useful outcome, so the send result is deliberately
                    // dropped rather than used to bail out.
                    let _ = tx.send(ScanMsg::Step {
                        done: step.done,
                        total: step.total,
                        label: format!("{} {}", step.format.label(), step.dir.display()),
                    });
                })
            });
            let _ = tx.send(ScanMsg::Done(found));
        });
        self.scan = Some(ScanJob {
            rx,
            done: 0,
            total: 0,
            label: String::new(),
        });
    }

    /// Drain whatever the scanning thread has said since the last frame. Called
    /// once per event-loop turn, so the bar advances at the redraw rate.
    fn poll_scan(&mut self) {
        let Some(job) = self.scan.as_mut() else {
            return;
        };
        let mut finished = None;
        loop {
            match job.rx.try_recv() {
                Ok(ScanMsg::Step { done, total, label }) => {
                    job.done = done;
                    job.total = total;
                    job.label = label;
                }
                Ok(ScanMsg::Done(found)) => {
                    finished = Some(found);
                    break;
                }
                // A thread that died without sending `Done` must not leave the
                // modal up forever, so a broken channel closes the job too.
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
            }
        }
        self.scan = None;
        if let Some(found) = finished {
            self.apply_plugins(found);
            eprintln!("choz: rescanned plugin paths: {} found", self.plugins.len());
        }
    }

    /// Sort a finished scan into the lists the pickers read. Split out of
    /// [`Self::discover_synths`] because the background rescan produces the
    /// same `Vec` on another thread and has to land it the same way.
    fn apply_plugins(&mut self, found: Vec<choz_engine::FoundPlugin>) {
        self.plugins = found;
        self.synths = self
            .plugins
            .iter()
            .filter(|p| p.is_instrument)
            .map(|p| SynthEntry {
                id: p.id.clone(),
                format: p.format,
                name: p.name.clone(),
                path: p.path.clone(),
            })
            .collect();
        // Formats choz can host go in the chain; the rest are still listed in
        // ADD FX (with their format) so it's clear what was found.
        self.fx_plugins = self
            .plugins
            .iter()
            .filter(|p| !p.is_instrument && p.format.is_hosted() && p.format.is_plugin())
            // Parameters are read lazily when the effect is added — scanning
            // instantiates enough plugins already.
            .map(|p| source::PluginFx {
                format: p.format,
                path: p.path.clone(),
                id: p.id.clone(),
                name: p.name.clone(),
                params: Vec::new(),
            })
            .collect();
        self.synth_cursor = 0;
    }

    /// Effects offered by ADD FX after the built-ins: hosted ones first, then
    /// the formats choz found but can't load yet.
    fn plugin_fx_entries(&self) -> Vec<(choz_engine::PluginFormat, String, bool)> {
        let mut out: Vec<_> = self
            .fx_plugins
            .iter()
            .map(|p| (p.format, p.name.clone(), true))
            .collect();
        out.extend(
            self.plugins
                .iter()
                .filter(|p| !p.is_instrument && !(p.format.is_hosted() && p.format.is_plugin()))
                .map(|p| (p.format, p.name.clone(), false)),
        );
        out
    }

    /// Append the ADD FX entry at `i` to the active slot's chain.
    fn add_fx_at(&mut self, i: usize) {
        if self.fx_chain.len() >= source::MAX_FX {
            return;
        }
        let entry = match ALL_FX_KINDS.get(i) {
            Some(&kind) => AudioFxEntry::new(kind),
            None => match self.fx_plugins.get(i - ALL_FX_KINDS.len()) {
                Some(p) => {
                    let mut plugin = p.clone();
                    plugin.params =
                        choz_engine::read_plugin_params(plugin.format, &plugin.path, &plugin.id);
                    AudioFxEntry::new_plugin(plugin)
                }
                // Past the hosted CLAP effects are the formats choz can scan but
                // not yet load; say so instead of silently doing nothing.
                None => {
                    if let Some(e) = self.fx_menu_entries().get(i) {
                        let fmt = e.format.map(|f| f.label()).unwrap_or("?");
                        eprintln!("choz: {fmt} hosting is not implemented yet ({})", e.label);
                    }
                    return;
                }
            },
        };
        self.fx_chain.push(entry);
        self.rebuild_fx();
    }

    // ── Modals ────────────────────────────────────────────────────────────

    /// Instruments choz can offer for a rack tab: hosted CLAP instruments, the
    /// SoundFonts and WAVs it can find, and a "browse" entry per file format.
    /// Formats choz can't host yet contribute nothing, so their filter is empty.
    fn source_choices(&self) -> Vec<SourceChoice> {
        let mut out: Vec<SourceChoice> = self
            .plugins
            .iter()
            .filter(|p| p.is_instrument)
            .map(|p| {
                let hosted = p.format.is_hosted();
                let mark = if hosted {
                    String::new()
                } else {
                    "  (not hosted yet)".to_string()
                };
                SourceChoice {
                    fmt: p.format.label(),
                    label: format!("{}{mark}", p.name),
                    action: match p.format {
                        // SFZ isn't a plugin, but it loads through the same
                        // path: the engine builds its own sampler for it.
                        f if f.is_hosted() && f != choz_engine::PluginFormat::Sf2 => {
                            SourceAction::Plugin {
                                format: p.format,
                                path: p.path.clone(),
                                id: p.id.clone(),
                            }
                        }
                        choz_engine::PluginFormat::Sf2 => SourceAction::File(p.path.clone()),
                        _ => SourceAction::Unsupported(p.format.label()),
                    },
                }
            })
            .collect();

        for (fmt, ext, dirs) in [
            ("SF2", "sf2", sf2_dirs()),
            (
                "WAV",
                "wav",
                vec![std::env::current_dir().unwrap_or_else(|_| ".".into())],
            ),
        ] {
            out.push(SourceChoice {
                fmt,
                label: format!("Browse for a .{ext} file..."),
                action: SourceAction::Browse(match ext {
                    "sf2" => &["sf2", "sf3"],
                    _ => &["wav"],
                }),
            });
            for dir in dirs {
                for path in scan_files(&dir, ext) {
                    out.push(SourceChoice {
                        fmt,
                        label: path
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default(),
                        action: SourceAction::File(path),
                    });
                }
            }
        }
        out
    }

    fn open_source_modal(&mut self) {
        let mut modal = Modal::new(
            ModalKind::Source,
            views::modal::ListModal::new("CHANGE SOURCE / SYNTH", Vec::new())
                .with_filters(SOURCE_FORMATS),
        );
        modal.sources = self.source_choices();
        self.modal = Some(modal);
        self.refresh_modal();
    }

    fn open_add_fx_modal(&mut self) {
        let mut modal = Modal::new(
            ModalKind::AddFx,
            views::modal::ListModal::new(i18n::t("ADD FX"), Vec::new()).with_filters(FX_FORMATS),
        );
        // Start on the sidebar: picking a category first is the fast path.
        modal.list.sidebar_focused = true;
        modal.list.note = "  \u{2190}\u{2192}=category/list  Tab=format".to_string();
        self.modal = Some(modal);
        self.refresh_modal();
    }

    /// Open the picker for FX parameter `param` of the selected unit, when that
    /// parameter is a list of names. Returns false when it is not one — the
    /// caller then does whatever it did before.
    fn open_fx_choice(&mut self, param: usize) -> bool {
        let Some(entry) = self.fx_chain.get(self.fx_slot) else {
            return false;
        };
        let descs = entry.param_descs();
        let Some(desc) = descs.get(param) else {
            return false;
        };
        let source::ParamShape::Named(points) = &desc.shape else {
            return false;
        };
        if points.len() < 2 {
            return false;
        }
        let items: Vec<String> = points.iter().map(|(_, n)| n.clone()).collect();
        let here = desc
            .shape
            .step_at(entry.params.get(param).copied().unwrap_or(0.0));
        let title = format!("{} \u{00B7} {}", entry.label(), desc.name);
        let mut modal = Modal::new(
            ModalKind::FxChoice,
            views::modal::ListModal::new(title, items),
        );
        modal.list.cursor = here.map(|(k, _)| k).unwrap_or(0);
        modal.fx_param = param;
        self.modal = Some(modal);
        true
    }

    /// Open the list of positions for arpeggiator knob `index`, when that knob
    /// has names rather than a range. False for the rest — a toggle has two
    /// places and no list worth opening, and a tempo is a number.
    ///
    /// The same rule the FX knobs follow, and the reason it exists is the
    /// keyboard: walking eight modes with the wheel is fine for a mouse and
    /// hopeless without one. Enter opens this, arrows move it, Enter picks.
    fn open_arp_choice(&mut self, index: usize) -> bool {
        let knobs = self.arp_knobs();
        let Some((_, name, value, shape)) = knobs.get(index) else {
            return false;
        };
        let source::ParamShape::Named(points) = shape else {
            return false;
        };
        if points.len() < 2 {
            return false;
        }
        let items: Vec<String> = points.iter().map(|(_, n)| n.clone()).collect();
        let here = shape.step_at(*value);
        let mut modal = Modal::new(
            ModalKind::ArpChoice,
            views::modal::ListModal::new(format!("{} \u{00B7} {}", i18n::t("ARP"), name), items),
        );
        modal.list.cursor = here.map(|(k, _)| k).unwrap_or(0);
        modal.fx_param = index;
        self.modal = Some(modal);
        true
    }

    /// Open the factory-preset list for the selected effect. False when it
    /// ships none — a hosted plugin brings its own, and a Gain knob needs none.
    fn open_fx_presets(&mut self) -> bool {
        let Some(entry) = self.fx_chain.get(self.fx_slot) else {
            return false;
        };
        if entry.plugin.is_some() {
            return false;
        }
        let set = fx_presets::presets(entry.kind);
        if set.is_empty() {
            return false;
        }
        let items: Vec<String> = set.iter().map(|p| p.name.to_string()).collect();
        let title = format!("{} \u{00B7} {}", entry.label(), i18n::t("PRESET"));
        self.modal = Some(Modal::new(
            ModalKind::FxPreset,
            views::modal::ListModal::new(title, items),
        ));
        true
    }

    /// Load factory preset `index` into the selected effect.
    ///
    /// Every value goes through `set_fx_param`, which is what a knob, a CC and
    /// the picker all use: the live processor hears it, the working copy keeps
    /// it, and the rebuild flag is set when the effect needs one. A preset that
    /// wrote `params` directly would be a fourth path to get subtly wrong.
    fn load_fx_preset(&mut self, index: usize) {
        let Some(entry) = self.fx_chain.get(self.fx_slot) else {
            return;
        };
        let Some(preset) = fx_presets::presets(entry.kind).get(index) else {
            return;
        };
        let fx = self.fx_slot;
        for (name, value) in preset.values {
            let Some(param) = self
                .fx_chain
                .get(fx)
                .and_then(|e| fx_presets::param_index(e, name))
            else {
                continue;
            };
            self.set_fx_param(fx, param, *value);
            // A built-in's dry/wet is a knob like any other, but the rebuild
            // reads it from `entry.wet` — so a preset that sets Wet has to write
            // both, or it lasts until the next rebuild.
            if *name == "Wet" {
                if let Some(e) = self.fx_chain.get_mut(fx) {
                    e.wet = *value;
                }
            }
        }
        self.fx_dirty = true;
    }

    /// What Enter (or a click) on an OUT drawer row does.
    fn out_targets(&self) -> Vec<(OutTarget, views::drawer::OutRow)> {
        use views::drawer::OutRow;
        let live_device = self
            .audio_engine
            .as_ref()
            .and_then(|e| e.output_device())
            .map(|d| d.to_string());
        let mut rows = vec![(
            OutTarget::None,
            OutRow {
                label: i18n::t("DEVICE").to_string(),
                mark: ' ',
                header: true,
            },
        )];
        for (i, name) in self.out_devices.iter().enumerate() {
            let live = Some(name) == live_device.as_ref();
            rows.push((
                OutTarget::Device(i),
                OutRow {
                    label: name.clone(),
                    mark: if live { '\u{2713}' } else { '\u{00B7}' },
                    header: false,
                },
            ));
        }

        // Every output channel of the running device, one row each: the two
        // sides of a tab are separate choices, so 3 and 9 is a routing like 1
        // and 2 is.
        let channels = self
            .audio_engine
            .as_ref()
            .map(|e| e.output_channels())
            .unwrap_or(2);
        let active = self.slots.get(self.active_slot).map(|s| s.out_pair);
        rows.push((
            OutTarget::None,
            OutRow {
                label: format!("CHANNELS ({channels})"),
                mark: ' ',
                header: true,
            },
        ));
        for ch in 0..channels {
            // Which tabs already play out of this channel, so the routing of
            // the whole rack is visible at a glance.
            let tabs: Vec<String> = self
                .slots
                .iter()
                .enumerate()
                .filter(|(_, s)| s.out_pair.0 == ch || s.out_pair.1 == ch)
                .map(|(i, _)| format!("{}", i + 1))
                .collect();
            let used = if tabs.is_empty() {
                String::new()
            } else {
                format!("  \u{2190} tab {}", tabs.join(","))
            };
            let role = side_label(active, ch);
            rows.push((
                OutTarget::Channel(ch),
                OutRow {
                    label: format!("{}{role}{used}", ch + 1),
                    mark: if role.is_empty() {
                        '\u{00B7}'
                    } else {
                        '\u{2713}'
                    },
                    header: false,
                },
            ));
        }

        // Where a tab's notes go when they leave choz: the arpeggiator into a
        // desk of hardware, which is the case the whole routing section exists
        // for. Listed after the audio because it is the rarer choice.
        rows.push((
            OutTarget::None,
            OutRow {
                label: format!("MIDI OUT ({})", self.midi_out_ports.len()),
                mark: ' ',
                header: true,
            },
        ));
        let bound = self
            .slots
            .get(self.active_slot)
            .and_then(|s| s.midi_out.clone());
        for (i, name) in self.midi_out_ports.iter().enumerate() {
            let on = bound.as_deref() == Some(name.as_str());
            // Which tabs already play to it, the way the channel rows do.
            let tabs: Vec<String> = self
                .slots
                .iter()
                .enumerate()
                .filter(|(_, s)| s.midi_out.as_deref() == Some(name.as_str()))
                .map(|(i, _)| format!("{}", i + 1))
                .collect();
            let used = if tabs.is_empty() {
                String::new()
            } else {
                format!("  \u{2190} tab {}", tabs.join(","))
            };
            rows.push((
                OutTarget::MidiOut(i),
                OutRow {
                    label: format!("{name}{used}"),
                    mark: if on { '\u{2713}' } else { '\u{00B7}' },
                    header: false,
                },
            ));
        }
        rows
    }

    /// Act on the OUT drawer row under the cursor. `how` says what a channel
    /// row does: assign it, take it off, or toggle — which is how a tab ends up
    /// playing out of 3 and 9, two jacks that are not a pair.
    fn out_select_side(&mut self, row: usize, how: Assign) {
        let Some((target, _)) = self.out_targets().into_iter().nth(row) else {
            return;
        };
        match target {
            OutTarget::None => {}
            // Picking the device is not an assignment, so the right button
            // leaves it alone.
            OutTarget::Device(_) if how == Assign::Off => {}
            OutTarget::Device(i) => {
                if let Some(name) = self.out_devices.get(i).cloned() {
                    self.set_output_device(&name);
                    self.refresh_out_devices();
                }
            }
            // Routing is per rack tab: the channel applies to the active one.
            OutTarget::Channel(ch) => self.set_active_out(ch, how),
            OutTarget::MidiOut(i) => {
                let Some(name) = self.midi_out_ports.get(i).cloned() else {
                    return;
                };
                let idx = self.active_slot;
                let bound = self
                    .slots
                    .get(idx)
                    .is_some_and(|s| s.midi_out.as_deref() == Some(name.as_str()));
                let next = match how {
                    Assign::On => Some(name),
                    Assign::Off => None,
                    Assign::Toggle if bound => None,
                    Assign::Toggle => Some(name),
                };
                // Whatever the tab had sounding out there belongs to the port
                // it is leaving: nothing else would ever send those note-offs.
                self.silence_midi_outs();
                if let Some(slot) = self.slots.get_mut(idx) {
                    slot.midi_out = next;
                }
            }
        }
    }

    /// Enter (or a left click) on an OUT row.
    fn out_select(&mut self, row: usize) {
        self.out_select_side(row, Assign::Toggle);
    }

    /// Add or remove `ch` from the active tab's output.
    fn set_active_out(&mut self, ch: usize, how: Assign) {
        let idx = self.active_slot;
        let Some(slot) = self.slots.get_mut(idx) else {
            return;
        };
        let on = channels_of(slot.out_pair).contains(&ch);
        let next = match (how, on) {
            (Assign::On, _) | (Assign::Toggle, false) => Some(assign_channel(slot.out_pair, ch)),
            (Assign::Off, _) | (Assign::Toggle, true) => unassign_channel(slot.out_pair, ch),
        };
        // A tab has to come out somewhere: taking away its last channel would
        // leave the engine no channel to mix into, so that gesture does nothing.
        let Some(pair) = next else { return };
        slot.out_pair = pair;
        if let Some(ref mut engine) = self.audio_engine {
            engine.set_slot_out(idx, pair.0, pair.1);
        }
    }

    /// Push every tab's output routing to the engine — after a rack reload the
    /// engine slots are new and back on the default pair.
    fn apply_routing(&mut self) {
        let outs: Vec<(usize, usize)> = self.slots.iter().map(|s| s.out_pair).collect();
        let ins: Vec<Option<(usize, usize)>> = self.slots.iter().map(|s| s.in_pair).collect();
        // A tab that lost its audio input cannot convert a pitch it no longer
        // hears, so the flag follows the routing.
        let a2m: Vec<bool> = self
            .slots
            .iter()
            .map(|s| s.in_pair.is_some() && s.pitch_to_midi)
            .collect();
        let mixes: Vec<f32> = self.slots.iter().map(|s| s.pitch_mix).collect();
        let trims: Vec<(f32, f32)> = self.slots.iter().map(|s| (s.in_gain, s.in_gate)).collect();
        let Some(ref mut engine) = self.audio_engine else {
            return;
        };
        for (i, (l, r)) in outs.into_iter().enumerate() {
            engine.set_slot_out(i, l, r);
        }
        for (i, pair) in ins.into_iter().enumerate() {
            engine.set_slot_in(i, pair);
        }
        for (i, on) in a2m.into_iter().enumerate() {
            engine.set_slot_pitch_to_midi(i, on);
        }
        for (i, mix) in mixes.into_iter().enumerate() {
            engine.set_slot_pitch_mix(i, mix);
        }
        // The trim goes last: it carries the gate, and the tracker it sets the
        // gate on only exists once `set_slot_pitch_to_midi(true)` has run.
        for (i, (gain, gate)) in trims.into_iter().enumerate() {
            engine.set_slot_in_trim(i, gain, gate);
        }
    }

    /// Open/close the OUT drawer. Opening re-reads the device list (they come
    /// and go) and parks the cursor on the live one; closing hands focus back
    /// to the RACK so no panel is left focused off-screen.
    fn toggle_out_drawer(&mut self) {
        self.out_open = !self.out_open;
        if !self.out_open {
            if self.focus == Focus::Output {
                self.focus = Focus::FxChain;
            }
            return;
        }
        self.refresh_out_devices();
        self.out_cursor = self
            .out_targets()
            .iter()
            .position(|(t, _)| *t != OutTarget::None)
            .unwrap_or(0);
        self.focus = Focus::Output;
    }

    fn refresh_out_devices(&mut self) {
        let Some(engine) = self.audio_engine.as_ref() else {
            return;
        };
        self.out_devices = engine.output_devices();
        // Same moment, same reason: a synth plugged in since the drawer was
        // last opened should be in the list.
        self.midi_out_ports = midi::list_output_ports();
        let rows = self.out_targets().len();
        self.out_cursor = self.out_cursor.min(rows.saturating_sub(1));
    }

    /// Open/close the IN drawer, same focus rules as the OUT one.
    fn toggle_in_drawer(&mut self) {
        self.in_open = !self.in_open;
        if self.in_open {
            self.refresh_in_ports();
            self.input_cursor = self
                .in_targets()
                .iter()
                .position(|(t, _)| *t != InTarget::None)
                .unwrap_or(0);
        }
        self.focus = if self.in_open {
            Focus::Source
        } else if self.focus == Focus::Source {
            Focus::FxChain
        } else {
            self.focus
        };
    }

    /// Bank/preset picker for the active tab (RACK's `[BANK/PRESET]`): a
    /// SoundFont's programs, or the plugin instrument's own patches.
    fn open_preset_modal(&mut self) {
        let Some(slot) = self.slots.get(self.active_slot) else {
            return;
        };
        if slot.presets.is_empty() && slot.plugin_presets.is_empty() {
            // No programs to list. If the plugin takes a state blob its patches
            // are files somewhere, so ask where instead of refusing.
            if self.can_pick_bank() {
                self.open_bank_browser();
            } else {
                eprintln!("choz: the active tab's instrument has no presets to pick");
            }
            return;
        }
        // Where the plugin says it is, when it can be asked: opening the picker
        // on row 0 while the plugin plays program 12 is a lie the user has to
        // undo before it is useful.
        let at = self
            .audio_engine
            .as_ref()
            .and_then(|e| e.slot_current_preset(self.active_slot))
            .and_then(|key| {
                self.slots
                    .get(self.active_slot)?
                    .plugin_presets
                    .iter()
                    .position(|p| p.key == key)
            });
        if let (Some(at), Some(slot)) = (at, self.slots.get_mut(self.active_slot)) {
            slot.preset_cursor = at;
        }
        let slot = match self.slots.get(self.active_slot) {
            Some(s) => s,
            None => return,
        };
        let banks = self.preset_banks(self.active_slot);
        let mut list = views::modal::ListModal::new("BANK / PRESET", Vec::new());
        if !banks.is_empty() {
            // Tab cycles these; the first one is the whole list, so a plugin
            // with thousands of patches still opens on something familiar.
            let mut chips = vec![i18n::t("ALL BANKS").to_string()];
            chips.extend(banks.iter().cloned());
            list.filters = chips;
            list.note = "  Tab = bank".to_string();
        }
        let mut modal = Modal::new(ModalKind::Preset, list);
        modal.list.cursor = slot.preset_cursor;
        self.modal = Some(modal);
        self.refresh_modal();
    }

    /// MIDI-learn picker: which rack control the next CC should drive.
    fn open_learn_modal(&mut self) {
        let slot = self.active_slot;
        if self.slots.is_empty() {
            return;
        }
        let mut targets = vec![
            LearnTarget::Gain(slot),
            LearnTarget::Pan(slot),
            LearnTarget::Trigger(TriggerAction::Mute),
            LearnTarget::Trigger(TriggerAction::Solo),
            LearnTarget::Trigger(TriggerAction::PresetPrev),
            LearnTarget::Trigger(TriggerAction::PresetNext),
            LearnTarget::Trigger(TriggerAction::InstrPagePrev),
            LearnTarget::Trigger(TriggerAction::InstrPageNext),
            LearnTarget::Trigger(TriggerAction::FxToggle),
            LearnTarget::Trigger(TriggerAction::FxMoveLeft),
            LearnTarget::Trigger(TriggerAction::FxMoveRight),
            LearnTarget::Trigger(TriggerAction::FxAdd),
            LearnTarget::Trigger(TriggerAction::ArpToggle),
            LearnTarget::Trigger(TriggerAction::ArpTap),
            LearnTarget::Trigger(TriggerAction::ArpLatch),
        ];
        for fx in 0..self.fx_chain.len() {
            targets.push(LearnTarget::Trigger(TriggerAction::FxSelect(fx)));
        }
        for (fx, entry) in self.fx_chain.iter().enumerate() {
            for param in 0..entry.param_descs().len() {
                targets.push(LearnTarget::FxParam { slot, fx, param });
            }
        }
        let instr_params = self
            .slots
            .get(slot)
            .map(|s| s.instr_params.len())
            .unwrap_or(0);
        for param in 0..instr_params {
            targets.push(LearnTarget::InstrParam { slot, param });
        }
        let mut modal = Modal::new(
            ModalKind::Learn,
            views::modal::ListModal::new("MIDI LEARN", Vec::new()),
        );
        modal.list.note = "  pick a control, then move a fader on your controller".to_string();
        modal.targets = targets;
        self.modal = Some(modal);
        self.refresh_modal();
    }

    /// Can this tab take a folder of preset files as its bank? Any plugin that
    /// can be handed its own state back can: that is what a `.fxp` carries.
    fn can_pick_bank(&self) -> bool {
        matches!(
            self.slots.get(self.active_slot).map(|s| &s.source),
            Some(AudioSource::Plugin { .. })
        ) && self
            .audio_engine
            .as_ref()
            .is_some_and(|e| e.slot_has_state(self.active_slot))
    }

    /// Pick the folder a plugin's patches live in — its bank, when the plugin
    /// has no browser of its own to ask. Starts where this tab last looked,
    /// then where the plugin is installed.
    fn open_bank_browser(&mut self) {
        let start = self
            .slots
            .get(self.active_slot)
            .and_then(|s| s.preset_dir.clone())
            .or_else(|| {
                let id = match self.slots.get(self.active_slot).map(|s| &s.source) {
                    Some(AudioSource::Plugin { id, .. }) => id.clone(),
                    _ => return None,
                };
                self.synths
                    .iter()
                    .find(|s| s.id == id)
                    .and_then(|s| s.path.parent().map(|p| p.to_path_buf()))
            })
            .filter(|p| p.is_dir())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| "/".into()));
        let mut modal = Modal::new(
            ModalKind::Bank,
            views::modal::ListModal::new(i18n::t("BANK FOLDER"), Vec::new()),
        );
        // The folder itself, not a file in it: a patch library is filed by
        // category, and those sub-folders are the picker's bank chips. The
        // browser's first row is "[use this one]".
        modal.browser = Some(file_browser::FileBrowser::open(
            &start,
            file_browser::DIR_PICK,
        ));
        self.modal = Some(modal);
        self.refresh_modal();
    }

    /// Switch the click on or off. Also on `F6` and on the MET button.
    fn toggle_metronome(&mut self) {
        let m = choz_engine::metronome::metronome();
        m.set_on(!m.on());
    }

    /// The metronome's menu: tempo, time signature, sound, level. Every row
    /// steps on Enter or a click and the menu stays open, because setting a
    /// tempo means hearing it — closing after each step would make that four
    /// round trips instead of four presses.
    fn open_metronome_modal(&mut self) {
        let modal = Modal::new(
            ModalKind::Metronome,
            views::modal::ListModal::new(i18n::t("METRONOME"), Vec::new()),
        );
        self.modal = Some(modal);
        self.refresh_modal();
    }

    /// The time signatures the menu offers, in the order it steps them.
    const TIME_SIGS: [(u16, u16); 6] = [(4, 4), (3, 4), (2, 4), (6, 8), (5, 4), (7, 8)];

    fn metronome_rows(&self) -> Vec<String> {
        let m = choz_engine::metronome::metronome();
        let t = choz_ports::transport();
        let (num, den) = t.time_signature();
        vec![
            format!(
                "  {:<12} {}",
                i18n::t("CLICK"),
                if m.on() { "ON" } else { "OFF" }
            ),
            format!("  {:<12} {:>5.1} BPM", i18n::t("TEMPO"), t.bpm()),
            format!("  {:<12} {num}/{den}", i18n::t("SIGNATURE")),
            format!("  {:<12} {}", i18n::t("SOUND"), m.style().label()),
            format!("  {:<12} {:>4.0}%", i18n::t("LEVEL"), m.gain() * 100.0),
        ]
    }

    /// Move row `i` of the metronome menu by `delta` steps.
    ///
    /// Enter (or a click) steps forward; `←` `→` and the wheel go either way,
    /// which is what a tempo needs — 132 is four presses past 112 and forty
    /// presses back the other way if the only direction is forward.
    fn step_metronome_row(&mut self, i: usize, delta: isize) {
        if delta == 0 {
            return;
        }
        let m = choz_engine::metronome::metronome();
        let t = choz_ports::transport();
        let wrap = |at: usize, n: usize| -> usize {
            ((at as isize + delta).rem_euclid(n as isize)) as usize
        };
        match i {
            0 => m.set_on(!m.on()),
            1 => {
                // Five at a time, wrapping: the range is 20..300 and a menu is
                // not where anyone sets a tempo one BPM at a time.
                let bpm = t.bpm() + 5.0 * delta as f32;
                let (lo, hi) = (
                    choz_ports::Transport::MIN_BPM,
                    choz_ports::Transport::MAX_BPM,
                );
                t.set_bpm(if bpm > hi {
                    lo
                } else if bpm < lo {
                    hi
                } else {
                    bpm
                });
            }
            2 => {
                let now = t.time_signature();
                let at = Self::TIME_SIGS.iter().position(|s| *s == now).unwrap_or(0);
                let (n, d) = Self::TIME_SIGS[wrap(at, Self::TIME_SIGS.len())];
                t.set_time_signature(n, d);
            }
            3 => {
                let all = choz_engine::metronome::ClickStyle::ALL;
                let at = all.iter().position(|s| *s == m.style()).unwrap_or(0);
                m.set_style(all[wrap(at, all.len())]);
            }
            4 => {
                let g = m.gain() + 0.1 * delta as f32;
                m.set_gain(if g > 1.001 {
                    0.1
                } else if g < 0.05 {
                    1.0
                } else {
                    g
                });
            }
            _ => {}
        }
    }

    /// Take `dir` as the active tab's bank: every preset file under it becomes
    /// a patch in the same picker a plugin's own browser fills, filed by
    /// sub-folder. Loading one is [`App::apply_selected_preset`], which for a
    /// file bank is a state restore.
    fn set_bank_dir(&mut self, dir: std::path::PathBuf) {
        let bank = choz_engine::preset_files::list_bank(&dir);
        if bank.is_empty() {
            eprintln!("choz: no .fxp / .fxb / .vstpreset files under {}", dir.display());
            return;
        }
        if let Some(slot) = self.slots.get_mut(self.active_slot) {
            slot.preset_cursor = 0;
            slot.plugin_presets = bank;
            slot.preset_dir = Some(dir);
        }
        // Straight into the patch list: picking the folder was a means to it.
        self.open_preset_modal();
    }

    fn open_browser_modal(&mut self, exts: &'static [&'static str]) {
        let start = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let title = format!("OPEN .{}", exts.join(" / ."));
        let mut modal = Modal::new(
            ModalKind::Browser,
            views::modal::ListModal::new(title, Vec::new()),
        );
        modal.browser = Some(file_browser::FileBrowser::open(&start, exts));
        self.modal = Some(modal);
        self.refresh_modal();
    }

    /// Whether the active tab's instrument has a native window to open.
    fn has_editor(&self) -> bool {
        self.editor_handle(None).is_some()
    }

    /// Same for the selected FX of the active tab's chain.
    fn has_fx_editor(&self) -> bool {
        self.editor_handle(Some(self.fx_slot)).is_some()
    }

    /// What the SLOT box says beyond its buttons: what is going through the
    /// selected effect, what the chain delays, and whether the effect has
    /// factory presets.
    ///
    /// The meter is read from the engine, where it was captured off the
    /// processor before it went to the RT thread — this side only ever loads
    /// two atomics.
    fn fx_slot_info(&self) -> views::fx_chain_panel::FxSlotInfo {
        let mut info = views::fx_chain_panel::FxSlotInfo {
            presets: self
                .fx_chain
                .get(self.fx_slot)
                .is_some_and(|e| e.plugin.is_none() && !fx_presets::presets(e.kind).is_empty()),
            ..Default::default()
        };
        let Some(engine) = self.audio_engine.as_ref() else {
            return info;
        };
        // Latency is the chain's, not the effect's: it is what the player
        // feels, and a disabled effect delays nothing but is still selected.
        info.latency_ms = engine.slot_latency(self.active_slot) as f32 * 1000.0
            / engine.sample_rate.max(1) as f32;
        if let Some(fx) = self.engine_fx_index(self.fx_slot) {
            info.peaks = engine.fx_meter(self.active_slot, fx).map(|m| m.peaks());
        }
        info
    }

    /// Which plugin the active tab's instrument (`None`) or one of its FX
    /// (`Some(ui index)`) is, as format/path/id — the identity the sandbox
    /// preference and the crash verdicts are keyed by.
    fn plugin_ref(
        &self,
        fx: Option<usize>,
    ) -> Option<(choz_engine::PluginFormat, std::path::PathBuf, String)> {
        match fx {
            Some(i) => {
                let p = self.fx_chain.get(i)?.plugin.as_ref()?;
                Some((p.format, p.path.clone(), p.id.clone()))
            }
            None => match &self.source {
                // The tab only remembers the plugin's id; the scan list is what
                // knows where it lives.
                AudioSource::Plugin { id, .. } => {
                    let e = self.synths.iter().find(|s| s.id == *id)?;
                    Some((e.format, e.path.clone(), e.id.clone()))
                }
                _ => None,
            },
        }
    }

    /// Out-of-process state of the instrument (`None`) or one of the FX, as the
    /// RACK draws it.
    fn sbx_state(&self, fx: Option<usize>) -> views::fx_chain_panel::SbxState {
        let mut state = views::fx_chain_panel::SbxState::default();
        let Some((format, path, id)) = self.plugin_ref(fx) else {
            return state;
        };
        state.available = true;
        state.on = choz_engine::quarantine::forced(format, &path, &id);
        let live = self.audio_engine.as_ref().and_then(|e| match fx {
            None => e.slot_sandbox(self.active_slot),
            Some(i) => e.fx_sandbox(self.active_slot, self.engine_fx_index(i)?),
        });
        if let Some(s) = live {
            state.live = true;
            state.missed = s.missed();
            state.restarts = s.restarts();
        }
        state
    }

    /// Ask for (or stop asking for) this plugin to play in its own process, then
    /// reload it so the change is audible now rather than next session.
    fn toggle_sandbox(&mut self, fx: Option<usize>) {
        let Some((format, path, id)) = self.plugin_ref(fx) else {
            eprintln!("choz: only a hosted plugin can run in its own process");
            return;
        };
        let on = !choz_engine::quarantine::forced(format, &path, &id);
        choz_engine::quarantine::set_forced(format, &path, &id, on);
        eprintln!(
            "choz: {} will {}run in its own process",
            path.display(),
            if on { "" } else { "no longer " }
        );
        match fx {
            // Rebuilding the chain re-reads the policy for every plugin in it.
            Some(_) => self.rebuild_fx(),
            None => self.reload_instrument(),
        }
    }

    /// Re-instantiate the active tab's plugin instrument, keeping its knobs.
    /// Used when something about *how* it is hosted changed.
    /// Whether the active tab plays a DSSI plugin.
    fn active_is_dssi(&self) -> bool {
        self.plugin_ref(None)
            .is_some_and(|(format, _, _)| format == choz_engine::PluginFormat::Dssi)
    }

    /// Store one DSSI `configure` setting for the active tab and rebuild the
    /// instrument with it. Rebuilt rather than sent live because `configure` is
    /// not RT-safe and the audio thread owns the instance — see
    /// [`choz_engine::AudioEngine::load_dssi`].
    fn set_dssi_config(&mut self, key: &str, value: &str) {
        let slot = self.active_slot;
        if let Some(s) = self.slots.get_mut(slot) {
            s.dssi_config.retain(|(k, _)| k != key);
            s.dssi_config.push((key.to_string(), value.to_string()));
        }
        self.reload_instrument();
        // The plugin's programs come *from* what it was just configured with:
        // FluidSynth-DSSI has none until it has a SoundFont, and then it has
        // that SoundFont's.
        let presets = match self.audio_engine.as_ref() {
            Some(engine) => engine.slot_presets(slot),
            None => Vec::new(),
        };
        if let Some(s) = self.slots.get_mut(slot) {
            s.plugin_presets = presets;
            s.preset_cursor = 0;
        }
    }

    /// Load a plugin instrument into `slot`, sending a DSSI synth its stored
    /// `configure` settings on the way in — the only moment they can be sent,
    /// and what a FluidSynth-DSSI tab needs to come back with its SoundFont.
    fn load_plugin_into(
        engine: &mut choz_engine::AudioEngine,
        slot: usize,
        format: choz_engine::PluginFormat,
        path: &std::path::Path,
        id: &str,
        config: &[(String, String)],
    ) -> anyhow::Result<()> {
        if format == choz_engine::PluginFormat::Dssi {
            engine.load_dssi(slot, path, id, config)
        } else {
            engine.load_plugin(slot, format, path, id)
        }
    }

    fn reload_instrument(&mut self) {
        let Some((format, path, id)) = self.plugin_ref(None) else {
            return;
        };
        let slot = self.active_slot;
        let values = self
            .slots
            .get(slot)
            .map(|s| s.instr_values.clone())
            .unwrap_or_default();
        let config = self
            .slots
            .get(slot)
            .map(|s| s.dssi_config.clone())
            .unwrap_or_default();
        self.close_editor_for(Some(slot));
        let Some(ref mut engine) = self.audio_engine else {
            return;
        };
        if let Err(e) = Self::load_plugin_into(engine, slot, format, &path, &id, &config) {
            eprintln!("choz: reloading {}: {e}", path.display());
            return;
        }
        // A fresh instance is back at the plugin's own defaults.
        for (i, v) in values.iter().enumerate() {
            engine.set_slot_param(slot, i, *v);
        }
    }

    /// The window handle for the active tab's instrument (`None`) or one of its
    /// FX (`Some(ui index)`).
    fn editor_handle(&self, fx: Option<usize>) -> Option<choz_ports::EditorHandle> {
        let engine = self.audio_engine.as_ref()?;
        match fx {
            None => engine.slot_editor(self.active_slot),
            Some(i) => engine.fx_editor(self.active_slot, self.engine_fx_index(i)?),
        }
    }

    /// Open a plugin window, or close it if that same plugin's window is
    /// already up. Only one is open at a time: two plugin GUIs floating over a
    /// terminal is more window than anyone asked for.
    fn toggle_editor(&mut self, fx: Option<usize>) {
        let key = (self.active_slot, fx);
        if self.editor.as_ref().is_some_and(|w| w.key == key) {
            self.editor = None;
            return;
        }
        self.editor = None;
        let Some(handle) = self.editor_handle(fx) else {
            eprintln!("choz: this plugin has no window");
            return;
        };
        let name = match fx {
            None => self.instrument_label(),
            Some(i) => self
                .fx_chain
                .get(i)
                .map(|e| e.label().to_string())
                .unwrap_or_default(),
        };
        let title = format!("choz \u{00B7} {name}");
        self.editor = editor::EditorWindow::open(key, handle, title);
    }

    /// Drop the window once the user closed it from the window manager, so the
    /// `[GUI]` button opens a fresh one instead of toggling a dead handle.
    fn poll_editor(&mut self) {
        if self.editor.as_ref().is_some_and(|w| !w.is_open()) {
            self.editor = None;
        }
    }

    /// Move the cursor inside the instrument's knob box, clamped to its ends.
    fn step_instr_cursor(&mut self, delta: isize) {
        let n = self.instr_knob_count();
        if n == 0 {
            return;
        }
        let next = (self.instr_param as isize + delta).clamp(0, n as isize - 1);
        self.instr_param = next as usize;
    }

    /// Page the instrument's knob box. The learned CCs follow it, but not from
    /// here: [`App::sync_instr_window`] moves them after the draw, so they
    /// follow the window however it moved.
    fn page_instr(&mut self, delta: isize) {
        let n = self.instr_knob_count();
        // What is actually on screen, read back from the last draw: the panel
        // decides how many knobs fit, and pretending otherwise here is how the
        // page and the box disagree.
        let (start, page, cols) = {
            let layout = self.layout.borrow();
            let knobs = &layout.rack.instr_knobs;
            let Some(&(first, rect)) = knobs.first() else {
                return;
            };
            let cols = knobs.iter().filter(|(_, r)| r.y == rect.y).count().max(1);
            (first, knobs.len(), cols)
        };
        if page == 0 || page >= n {
            return;
        }
        let rows = page.div_ceil(cols);
        // The box scrolls to keep the cursor visible, with the cursor on the
        // *last* visible row — so putting the cursor on the last cell of the
        // page we want is how the window is moved. No second scroll state to
        // keep in step with the first.
        let wanted = (start as isize + delta * page as isize).max(0) as usize;
        let cursor = (wanted + page - 1).min(n - 1);
        let landed = (cursor / cols).saturating_sub(rows - 1) * cols;
        let shift = landed as isize - start as isize;
        if shift == 0 {
            return;
        }
        self.instr_param = cursor;
    }

    /// Re-address the CCs learned on the instrument's knob box to whatever the
    /// box is showing now.
    ///
    /// A plugin with three hundred parameters (Surge XT has them) shows a
    /// dozen at a time, so the box has a window into the list. Whoever moves
    /// that window — the `◀` `▶` buttons, `PgUp` / `PgDn`, a CC bound to
    /// either, the arrow keys walking off the edge, the wheel, a terminal
    /// resize — moves every CC bound to this tab's instrument by the same
    /// distance: the fader that sat on the first knob of the box still sits on
    /// the first knob of the box. Otherwise eight faders own eight of three
    /// hundred parameters for good.
    ///
    /// This runs after the draw, on the window the draw actually produced,
    /// because that is the only place the truth lives: the panel decides how
    /// many knobs fit. Nothing that scrolls the box has to remember to call
    /// anything.
    fn sync_instr_window(&mut self) {
        let slot = self.active_slot;
        let n = self.instr_knob_count();
        let start = self
            .layout
            .borrow()
            .rack
            .instr_knobs
            .first()
            .map(|&(first, _)| first);
        let prev = self.slots.get(slot).and_then(|s| s.instr_window);
        if let Some(s) = self.slots.get_mut(slot) {
            s.instr_window = start.map(|start| (start, n));
        }
        let (Some(start), Some((was, was_n))) = (start, prev) else {
            return;
        };
        // A different parameter list is not a window that moved: a tab that
        // just loaded a plugin adopts its window and shifts nothing.
        if was_n != n || start == was {
            return;
        }
        let shift = start as isize - was as isize;
        for b in self.cc_bindings.iter_mut() {
            if let LearnTarget::InstrParam { slot: s, param } = &mut b.target {
                // ponytail: clamped, so a binding near the end of the list
                // lands on the last parameter instead of falling off the rack.
                // Two of them can then share one parameter, which is visible
                // and undoable; a silently dropped binding is neither.
                if *s == slot {
                    *param = (*param as isize + shift).clamp(0, n as isize - 1) as usize;
                }
            }
        }
    }

    /// Columns in the instrument's knob box, read back from what was drawn:
    /// the panel decides how many fit, and up/down should move a whole row.
    fn instr_cols(&self) -> usize {
        let layout = self.layout.borrow();
        let knobs = &layout.rack.instr_knobs;
        let Some((_, first)) = knobs.first() else {
            return 1;
        };
        knobs.iter().filter(|(_, r)| r.y == first.y).count().max(1)
    }

    /// Swap the arrows between the two knob boxes. Only useful when the
    /// instrument has knobs at all.
    fn toggle_rack_focus(&mut self) {
        // Only boxes that are on screen: `k` on a rack whose arpeggiator is off
        // (or whose screen is too short for its knobs) must not land the arrows
        // on something invisible.
        let instr = self.instr_knob_count() > 0;
        // Whether the arpeggiator has controls, not whether they came out as a
        // box: on a panel too short for one they are drawn as buttons, and the
        // keyboard has to reach them there too — that is the shape a five-inch
        // screen gets.
        let arp = self
            .slots
            .get(self.active_slot)
            .is_some_and(|s| s.arp.is_on());
        self.rack_focus = match self.rack_focus {
            RackFocus::Fx if instr => RackFocus::Instrument,
            RackFocus::Fx if arp => RackFocus::Arp,
            RackFocus::Instrument if arp => RackFocus::Arp,
            _ => RackFocus::Fx,
        };
    }

    /// The arpeggiator's knobs of the active tab, in the order they are drawn.
    fn arp_knobs(&self) -> Vec<(arp::ArpParam, &'static str, f32, source::ParamShape)> {
        self.slots
            .get(self.active_slot)
            .map(|s| s.arp.view().knobs())
            .unwrap_or_default()
    }

    /// Where a control sits in the arpeggiator's knob list, when it is there at
    /// all — `MODE` is not, in the sequencer.
    fn arp_knob_index(&self, param: arp::ArpParam) -> Option<usize> {
        self.arp_knobs().iter().position(|(p, ..)| *p == param)
    }

    /// Move the cursor inside the arpeggiator's knob box.
    fn step_arp_cursor(&mut self, delta: isize) {
        let n = self.arp_knobs().len();
        if n == 0 {
            return;
        }
        self.arp_param = (self.arp_param as isize + delta).clamp(0, n as isize - 1) as usize;
    }

    /// Columns in the arpeggiator's box, read back from what was drawn — the
    /// panel decides how many fit, and up/down move a whole row.
    fn arp_cols(&self) -> usize {
        let layout = self.layout.borrow();
        let knobs = &layout.rack.arp_knobs;
        let Some((_, first)) = knobs.first() else {
            return 1;
        };
        knobs.iter().filter(|(_, r)| r.y == first.y).count().max(1)
    }

    /// Set arpeggiator knob `index` to a 0..1 position.
    fn set_arp_knob(&mut self, index: usize, value: f32) {
        let Some(param) = self.arp_knobs().get(index).map(|(p, ..)| *p) else {
            return;
        };
        // Memorising is the gesture: the chord held when the switch goes on is
        // the chord it will play.
        if param == arp::ArpParam::Chord && value >= 0.5 {
            if let Some(slot) = self.slots.get_mut(self.active_slot) {
                slot.arp.memorise_chord();
            }
        }
        self.edit_arp(ArpEdit::Knob { param, value });
    }

    /// Nudge a knob the way the arrows and the wheel do: a stepped control moves
    /// one position, a continuous one moves by `delta`.
    fn nudge_arp_knob(&mut self, index: usize, delta: f32) {
        let Some((_, _, value, shape)) = self.arp_knobs().get(index).cloned() else {
            return;
        };
        self.set_arp_knob(index, shape.nudge(value, delta));
    }

    /// What Enter does to an arpeggiator knob: **flip a switch**, step anything
    /// else on by one.
    ///
    /// The difference matters on the very first knob, which is the
    /// arpeggiator's own on/off. Enter used to nudge it *up*, and a switch
    /// nudged up when it is already on stays on — so Enter could start the
    /// arpeggiator and never stop it.
    fn press_arp_knob(&mut self, index: usize) {
        let Some((_, _, value, shape)) = self.arp_knobs().get(index).cloned() else {
            return;
        };
        match shape {
            source::ParamShape::Toggle => {
                let flipped = if value >= 0.5 { 0.0 } else { 1.0 };
                self.set_arp_knob(index, flipped);
            }
            _ => self.set_arp_knob(index, shape.nudge(value, 1.0)),
        }
    }

    /// The active tab's instrument parameters as knobs: name and 0..1 position.
    ///
    /// This is the panel Carla shows for every plugin — the point being that a
    /// CC can be learned on any parameter without opening the plugin's own
    /// window, which for many plugins is the slow part.
    fn instr_knobs(&self) -> Vec<(String, f32, source::ParamShape)> {
        let Some(slot) = self.slots.get(self.active_slot) else {
            return Vec::new();
        };
        slot.instr_params
            .iter()
            .enumerate()
            .map(|(i, p)| {
                (
                    p.name.clone(),
                    slot.instr_values.get(i).copied().unwrap_or(0.0),
                    source::ParamShape::of(p),
                )
            })
            .collect()
    }

    /// Whether the active tab is fed by audio, and whether it is converting it
    /// to notes. `None` when there is no audio coming in: the button would be a
    /// switch for something that is not happening.
    fn pitch_to_midi_state(&self) -> Option<bool> {
        let slot = self.slots.get(self.active_slot)?;
        slot.in_pair.map(|_| slot.pitch_to_midi)
    }

    /// The automation loop's length in bars of the current time signature —
    /// what the UI shows, because "16 beats" means nothing in 6/8.
    fn automation_loop_bars(&self) -> u32 {
        let per_bar = choz_ports::transport().bar_quarters().max(1.0) as f32;
        ((self.automation.loop_beats() / per_bar).round() as u32).max(1)
    }

    /// Lengthen or shorten the loop by `d` bars. A lane is a position in a
    /// loop, so this is the one number that decides what a lane means.
    fn nudge_automation_loop(&mut self, d: i32) {
        let per_bar = choz_ports::transport().bar_quarters().max(1.0) as f32;
        let bars = (self.automation_loop_bars() as i32 + d).clamp(1, 64) as f32;
        self.automation.loop_beats = bars * per_bar;
    }

    /// The active tab's input trim and `A→M` sensitivity, or `None` when it
    /// plays its own instrument and there is nothing coming in to trim.
    fn in_trim_state(&self) -> Option<(f32, f32)> {
        let slot = self.slots.get(self.active_slot)?;
        slot.in_pair.map(|_| (slot.in_gain, slot.in_gate))
    }

    /// Move the input trim (`gain`) or the sensitivity (`gate`) of the active
    /// tab, both as a delta on the knob's own 0..1 travel.
    fn adjust_in_trim(&mut self, d_gain: f32, d_gate: f32) {
        let idx = self.active_slot;
        let Some(s) = self.slots.get_mut(idx) else {
            return;
        };
        if s.in_pair.is_none() {
            return;
        }
        if d_gain != 0.0 {
            s.in_gain = (s.in_gain + d_gain * MAX_IN_GAIN).clamp(0.0, MAX_IN_GAIN);
        }
        if d_gate != 0.0 {
            let norm = views::fx_chain_panel::gate_norm(s.in_gate) + d_gate;
            s.in_gate = views::fx_chain_panel::gate_from_norm(norm);
        }
        let (gain, gate) = (s.in_gain, s.in_gate);
        if let Some(engine) = self.audio_engine.as_mut() {
            engine.set_slot_in_trim(idx, gain, gate);
        }
    }

    /// Set them outright, which is what MIDI learn and automation do: a CC is a
    /// position on the knob, not a nudge.
    fn set_in_trim(&mut self, gain: Option<f32>, gate_norm: Option<f32>) {
        let idx = self.active_slot;
        let Some(s) = self.slots.get_mut(idx) else {
            return;
        };
        if let Some(g) = gain {
            s.in_gain = g.clamp(0.0, MAX_IN_GAIN);
        }
        if let Some(n) = gate_norm {
            s.in_gate = views::fx_chain_panel::gate_from_norm(n);
        }
        let (gain, gate) = (s.in_gain, s.in_gate);
        if let Some(engine) = self.audio_engine.as_mut() {
            engine.set_slot_in_trim(idx, gain, gate);
        }
    }

    /// A guitar into a synth, or back to passing the audio through.
    fn toggle_pitch_to_midi(&mut self) {
        let slot = self.active_slot;
        let Some(s) = self.slots.get_mut(slot) else {
            return;
        };
        if s.in_pair.is_none() {
            return;
        }
        s.pitch_to_midi = !s.pitch_to_midi;
        let on = s.pitch_to_midi;
        if let Some(engine) = self.audio_engine.as_mut() {
            engine.set_slot_pitch_to_midi(slot, on);
        }
        eprintln!(
            "choz: tab {} audio\u{2192}MIDI {}",
            slot + 1,
            if on { "on" } else { "off" }
        );
        // **Independent on purpose.** `A→M` turns the tab's audio into notes
        // inside the callback; the ALGO list decides what happens to notes on
        // the way to the instrument. They are different questions, so this
        // switch retires nothing and nothing retires it — a tab can convert a
        // guitar and arpeggiate the result.
    }

    /// How much of a converting tab's output is the instrument.
    ///
    /// `wrap` is the click — the button walks 100 → 75 → 50 → 25 → 0 → 100,
    /// the same idiom the arpeggiator's GATE uses. The wheel clamps instead:
    /// turning it past the end and landing back at the other one is not what a
    /// wheel means.
    fn step_pitch_mix(&mut self, delta: f32, wrap: bool) {
        let slot = self.active_slot;
        let Some(s) = self.slots.get_mut(slot) else {
            return;
        };
        let next = s.pitch_mix + delta;
        s.pitch_mix = if wrap && next < -0.001 {
            1.0
        } else {
            next.clamp(0.0, 1.0)
        };
        let mix = s.pitch_mix;
        if let Some(engine) = self.audio_engine.as_mut() {
            engine.set_slot_pitch_mix(slot, mix);
        }
    }

    /// What control the instrument's parameter `i` is, so the arrows step it
    /// the way it is drawn.
    fn instr_param_shape(&self, i: usize) -> source::ParamShape {
        self.slots
            .get(self.active_slot)
            .and_then(|s| s.instr_params.get(i))
            .map(source::ParamShape::of)
            .unwrap_or_default()
    }

    /// How many knobs that box has, for cursor movement.
    fn instr_knob_count(&self) -> usize {
        self.slots
            .get(self.active_slot)
            .map(|s| s.instr_params.len())
            .unwrap_or(0)
    }

    /// Publish what the panels should blend against: one colour per cell of the
    /// picture behind them, plus the theme colour and how strongly it washes.
    ///
    /// Both drawing paths feed it. Under the kitty protocol the buffer holds no
    /// picture at all (it is under the cells), so the grid is the only thing the
    /// panels can know about what is behind them.
    fn publish_backdrop(&mut self, area: ratatui::layout::Rect) {
        let cells = match (&self.ui.background, &self.wallpaper, &self.kitty_cells) {
            // Halfblocks: the picture is in the buffer, and the cache knows it.
            (settings::Background::Image { .. }, Some(w), _) if self.kitty_bg.is_none() => {
                Some(views::background::backdrop_cells(w, area))
            }
            // kitty: the grid was computed when the image was transmitted.
            (settings::Background::Image { .. }, _, Some(c)) if self.kitty_bg.is_some() => {
                Some(c.clone())
            }
            // A flat colour has no picture in it; the panels are tinted over
            // it once, below, instead of cell by cell.
            _ => None,
        };
        let graphics = self.kitty_bg.is_some();
        let has_cells = cells.is_some();
        views::theme::set_backdrop(cells.map(|cells| views::theme::Backdrop {
            cols: area.width,
            rows: area.height,
            cells,
            tint: self.ui.tint(),
            graphics,
        }));

        // With a picture behind them the panels are washed per cell, so they
        // paint nothing of their own. Otherwise the translucency is resolved
        // once: the desktop's own colour with the tint mixed in.
        let (tint, alpha) = self.ui.tint();
        views::theme::set_panel_fill(match (&self.ui.background, has_cells) {
            (_, true) => None,
            (settings::Background::Color(base), _) => Some(views::theme::blend(*base, tint, alpha)),
            // The terminal's own background: choz cannot read it, so it blends
            // against the colour it would have painted there anyway.
            _ => Some(views::theme::blend(
                views::theme::rgb_of(views::theme::APP_BG),
                tint,
                alpha,
            )),
        });
    }

    /// Follow the knobs the user moves **inside the plugin's own window**.
    ///
    /// Two things depend on this, and both were impossible while the plugin's
    /// GUI had the mouse: keeping choz's copy of the values (and therefore the
    /// saved project) in step with what the user did in there, and MIDI learn —
    /// "bind the knob I am touching" needs the plugin to say which one it is.
    ///
    /// Only the plugin whose window is open can report anything, so that is the
    /// only one polled.
    fn poll_plugin_touch(&mut self) {
        let Some((slot, fx)) = self.editor.as_ref().map(|w| w.key) else {
            return;
        };
        // A drag produces a stream of edits; a handful per frame is plenty to
        // stay current without spinning here.
        for _ in 0..16 {
            let touched = match (self.audio_engine.as_ref(), fx) {
                (Some(e), None) => e.slot_touched_param(slot),
                (Some(e), Some(ui_fx)) => match self.engine_fx_index(ui_fx) {
                    Some(engine_fx) => e.fx_touched_param(slot, engine_fx),
                    None => return,
                },
                (None, _) => None,
            };
            let Some((id, value)) = touched else { return };
            self.record_plugin_edit(slot, fx, id, value);
        }
    }

    /// One parameter edit that came from the plugin's window: store the value
    /// where the project will find it, and finish arming MIDI learn if it is
    /// waiting for a control.
    ///
    /// `index` is a **position in the parameter list**, not the plugin's own
    /// id: each format's host translates before reporting, because that is the
    /// only thing the knobs, the learn targets and the saved project speak. A
    /// CLAP id, an LV2 port number and a VST3 `ParamID` are all arbitrary.
    fn record_plugin_edit(&mut self, slot: usize, fx: Option<usize>, index: u32, value: f32) {
        let index = index as usize;
        let known = match fx {
            None => self
                .slots
                .get(slot)
                .is_some_and(|s| index < s.instr_params.len()),
            Some(ui_fx) => self
                .fx_chain
                .get(ui_fx)
                .and_then(|e| e.plugin.as_ref())
                .is_some_and(|p| index < p.params.len()),
        };
        if !known {
            return;
        }

        match fx {
            None => {
                if let Some(v) = self
                    .slots
                    .get_mut(slot)
                    .and_then(|s| s.instr_values.get_mut(index))
                {
                    *v = value;
                }
            }
            Some(ui_fx) => {
                if let Some(v) = self
                    .fx_chain
                    .get_mut(ui_fx)
                    .and_then(|e| e.params.get_mut(index))
                {
                    *v = value;
                }
            }
        }

        // Learn was armed and is waiting for a control: the knob the user just
        // grabbed in the plugin's window is the answer.
        if self.learn_pick && self.learn.is_none() {
            self.learn = Some(match fx {
                None => LearnTarget::InstrParam { slot, param: index },
                Some(ui_fx) => LearnTarget::FxParam {
                    slot,
                    fx: ui_fx,
                    param: index,
                },
            });
            self.learn_pick = false;
        }
    }

    /// Parameter editor for the active tab's instrument — a hosted plugin's own
    /// parameters, or a SoundFont's built-in reverb / chorus switches.
    fn open_instr_modal(&mut self) {
        let Some(slot) = self.slots.get(self.active_slot) else {
            return;
        };
        if slot.instr_values.is_empty() {
            return;
        }
        let mut modal = Modal::new(
            ModalKind::InstrParams,
            views::modal::ListModal::new(
                format!("INSTRUMENT \u{00B7} {}", slot_label(&slot.source)),
                Vec::new(),
            ),
        );
        modal.list.note =
            "  \u{2190}\u{2192} change the selected value \u{00B7} l = learn a CC for it"
                .to_string();
        self.modal = Some(modal);
        self.refresh_modal();
    }

    /// Close whatever modal is open. Editing the search paths changes what a
    /// scan would find, so leaving that modal always rescans — whichever way it
    /// was closed (Esc, the CANCEL button, or a click outside).
    fn close_modal(&mut self) {
        let kind = self.modal.take().map(|m| m.kind);
        self.path_edit = None;
        self.save_name = None;
        if kind == Some(ModalKind::PluginPaths) && self.paths_dirty {
            self.paths_dirty = false;
            self.discover_synths(true);
        }
    }

    /// Settings \u{2192} Plugin paths. One row per format header, then its
    /// directories; the same list handles enable/disable, add and remove.
    fn open_paths_modal(&mut self) {
        let mut modal = Modal::new(
            ModalKind::PluginPaths,
            views::modal::ListModal::new("SETTINGS \u{00B7} PLUGIN PATHS", Vec::new()),
        );
        modal.list.filters = SETTINGS_TABS
            .iter()
            .map(|t| i18n::t(t).to_string())
            .collect();
        // Start on the section list, like every other sidebar modal.
        modal.list.sidebar_focused = true;
        self.modal = Some(modal);
        self.refresh_modal();
    }

    /// What a search directory contributed to the last scan, and — when that is
    /// nothing — what it actually holds. This is the cue that was missing when a
    /// folder full of SoundFonts was added under the wrong format.
    fn dir_hint(&self, fmt: choz_engine::PluginFormat, dir: &choz_engine::SearchDir) -> String {
        if !dir.enabled {
            return "   (off)".to_string();
        }
        let found = self
            .plugins
            .iter()
            .filter(|p| p.format == fmt && p.path.starts_with(&dir.path))
            .count();
        if found > 0 {
            return format!("   ({found})");
        }
        if !dir.path.is_dir() {
            return "   (missing)".to_string();
        }
        match choz_engine::paths::formats_present(&dir.path).first() {
            Some(&(other, n)) if other != fmt => {
                format!(
                    "   (0 \u{2014} holds {n} {} file(s), move it to {})",
                    other.label(),
                    other.label()
                )
            }
            _ => "   (0)".to_string(),
        }
    }

    /// The THEME tab: a colour scheme picker, then the desktop background.
    ///
    /// The two halves are separated by a header row, the same trick the plugin
    /// paths list uses: headers are labels, not options.
    /// The rows and what each one does, built together.
    ///
    /// One list rather than two functions doing the same arithmetic from
    /// opposite ends: with the schemes now numbering in the hundreds, an index
    /// map maintained by hand is a bug waiting for the next row to be inserted.
    ///
    /// The desktop controls come **first**. They used to sit under the scheme
    /// list, which was fine with eleven schemes and unreachable with 372.
    fn theme_layout(&self) -> Vec<(String, Option<ThemeRow>)> {
        let mut rows: Vec<(String, Option<ThemeRow>)> =
            Vec::with_capacity(settings::THEMES.len() + 8);
        rows.push(("DESKTOP".to_string(), None));
        rows.push((
            format!("  {:<18} {}", "Background", self.ui.background.label()),
            Some(ThemeRow::Background),
        ));
        if let settings::Background::Image { fit, .. } = &self.ui.background {
            rows.push((
                format!("  {:<18} {}   (Enter cycles)", "Fit", fit.label()),
                Some(ThemeRow::Fit),
            ));
        }
        // The panel wash: every section — IN/OUT, RACK, FX, TRANSPORT, the
        // monitor — gets the same colour at the same strength, so the desktop
        // reads through all of them equally. Absent on the terminal's own
        // background, which choz cannot read and therefore cannot blend with.
        if !matches!(self.ui.background, settings::Background::Terminal) {
            // A slider, because the useful value is "as much as it takes to read
            // the knobs" and that is judged by eye, not typed.
            let pct = self.ui.background_tint.min(100);
            let filled = (pct as usize * TINT_BAR_WIDTH).div_ceil(100);
            let bar: String = std::iter::repeat_n('\u{2588}', filled)
                .chain(std::iter::repeat_n('\u{2591}', TINT_BAR_WIDTH - filled))
                .collect();
            rows.push((
                format!(
                    "  {:<18} {bar} {pct:>3}%   (\u{2190}\u{2192})",
                    "Panel opacity"
                ),
                Some(ThemeRow::Tint),
            ));
            rows.push((
                format!(
                    "  {:<18} {}   (\u{2190}\u{2192})",
                    "Panel colour",
                    self.ui.panel_tint_label()
                ),
                Some(ThemeRow::PanelColor),
            ));
        }
        rows.push((
            format!("  {:<18} {}", "Pick an image...", "Enter opens the browser"),
            Some(ThemeRow::PickImage),
        ));
        rows.push((
            format!(
                "  {:<18} {}",
                "Clear background", "back to the terminal's own"
            ),
            Some(ThemeRow::Clear),
        ));
        rows.push((
            format!(
                "  {:<18} {}",
                "Apply and close", "keeps theme and background"
            ),
            Some(ThemeRow::Done),
        ));

        rows.push(("COLOUR SCHEME".to_string(), None));
        for (k, t) in settings::THEMES.iter().enumerate() {
            let active = t.text == self.ui.text_color && Some(t.border) == self.ui.border_color;
            let mark = if active { "\u{25CF}" } else { "\u{25CB}" };
            let swatch = "\u{2588}\u{2588}";
            rows.push((
                format!(
                    "  {mark} {:<24} {swatch} text  {swatch} frame  {}",
                    t.name,
                    match t.desktop {
                        Some(_) => format!("{swatch} desktop"),
                        None => "   (terminal)".to_string(),
                    }
                ),
                Some(ThemeRow::Scheme(k)),
            ));
        }
        rows
    }

    fn theme_rows(&self) -> Vec<String> {
        self.theme_layout()
            .into_iter()
            .map(|(label, _)| label)
            .collect()
    }

    /// Row index → what it means on the THEME tab. `None` for the headers.
    fn theme_row(&self, i: usize) -> Option<ThemeRow> {
        self.theme_layout().get(i).and_then(|(_, row)| *row)
    }

    /// Enter on a THEME row. Returns whether the modal should close.
    ///
    /// Picking a scheme applies and saves at once, like the language tab did —
    /// but the modal stays open, because the whole point of a theme picker is
    /// trying a few.
    fn theme_select(&mut self, i: usize) -> bool {
        let Some(row) = self.theme_row(i) else {
            return false;
        };
        match row {
            ThemeRow::Scheme(k) => {
                if let Some(theme) = settings::THEMES.get(k) {
                    self.ui.apply_theme(theme);
                    self.apply_ui_settings();
                }
            }
            ThemeRow::Background => {
                // Only meaningful as a toggle between "no background" and the
                // current scheme's desktop colour; the image comes from the
                // browser row below.
                self.ui.background = match self.ui.background {
                    settings::Background::Terminal => {
                        let rgb = settings::THEMES
                            .iter()
                            .find(|t| t.name == self.ui.theme_name)
                            .and_then(|t| t.desktop)
                            .unwrap_or(self.ui.text_color);
                        settings::Background::Color(rgb)
                    }
                    _ => settings::Background::Terminal,
                };
                self.apply_ui_settings();
            }
            ThemeRow::Fit => {
                if let settings::Background::Image { path, fit } = &self.ui.background {
                    self.ui.background = settings::Background::Image {
                        path: path.clone(),
                        fit: fit.next(),
                    };
                    self.apply_ui_settings();
                }
            }
            ThemeRow::Tint => {
                // Enter steps it too, so the row does something for anyone who
                // has not noticed the arrows.
                self.step_tint(TINT_STEP as i16);
            }
            ThemeRow::PanelColor => {
                self.ui.panel_tint = {
                    self.ui.step_panel_tint(1);
                    self.ui.panel_tint
                };
                self.ui.save();
                self.apply_ui_settings();
            }
            ThemeRow::PickImage => {
                self.open_wallpaper_browser();
                return false;
            }
            ThemeRow::Clear => {
                self.ui.background = settings::Background::Terminal;
                self.apply_ui_settings();
            }
            // Everything is applied and saved as it is picked, so there is
            // nothing left to commit — just leave.
            ThemeRow::Done => return true,
        }
        self.refresh_modal();
        false
    }

    /// Move the panel opacity by `delta` percent and apply it at once, so the
    /// slider is judged against the real screen.
    ///
    /// Nothing about the picture changes — the wash lives in the panels, not in
    /// the image — so this is a redraw, not a rebuild. That is the difference
    /// between a slider that follows the key and one that stutters: the first
    /// version baked the tint into the image and paid a decode, a Lanczos
    /// rescale and (under kitty) a multi-megabyte transfer on every press.
    fn step_tint(&mut self, delta: i16) {
        let next = (self.ui.background_tint as i16 + delta).clamp(0, 100) as u8;
        if next == self.ui.background_tint {
            return;
        }
        self.ui.background_tint = next;
        self.apply_ui_settings();
        self.refresh_modal();
    }

    /// The image browser, started in the project's `assets/` when it exists —
    /// that is where the sample wallpapers live — and in the working directory
    /// otherwise.
    fn open_wallpaper_browser(&mut self) {
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        // Where the shipped images are on an installed copy, the repository's
        // `assets/` in a checkout, and the working directory when neither is
        // there.
        let start = settings::wallpaper_dir().unwrap_or(cwd);
        let mut modal = Modal::new(
            ModalKind::Wallpaper,
            views::modal::ListModal::new("BACKGROUND IMAGE", Vec::new()),
        );
        modal.browser = Some(file_browser::FileBrowser::open(
            &start,
            file_browser::IMAGE_EXTS,
        ));
        self.modal = Some(modal);
        self.refresh_modal();
    }

    /// Which AUDIO sub-category is showing (Engine / Plugin Paths / OSC).
    fn audio_section(&self) -> usize {
        self.modal
            .as_ref()
            .map(|m| m.list.sidebar_cursor)
            .unwrap_or(SEC_ENGINE)
    }

    /// Engine section: the same rows seqterm shows, with the ones choz can't
    /// change marked read-only rather than hidden.
    fn engine_rows(&self) -> Vec<String> {
        let a = &self.ui.audio;
        let engine = self.audio_engine.as_ref();
        // With AUTO, say what the engine actually picked.
        let backend = match (a.backend.as_str(), engine) {
            ("AUTO", Some(e)) => format!("AUTO \u{2192} {}", e.backend.label()),
            ("AUTO", None) => "AUTO".to_string(),
            (b, _) => b.to_string(),
        };
        let device = engine
            .and_then(|e| e.output_device().map(|d| d.to_string()))
            .or_else(|| (!a.device.is_empty()).then(|| a.device.clone()))
            .unwrap_or_else(|| "(default)".to_string());
        let running = engine.map(|e| (e.sample_rate, e.buffer_size));
        let pending = |field: u32, live: Option<u32>| match live {
            Some(l) if l != field => format!("{field}  (restart: running {l})"),
            _ => field.to_string(),
        };
        // What the live input is, which is the difference between "choz is
        // deaf" and "the effect does nothing". Under the native JACK client
        // there is no device to pick: every capture jack in the graph is
        // already wired, and the IN drawer picks a *channel*.
        let input = match engine {
            Some(e) if e.backend == choz_engine::AudioBackend::Jack => {
                format!("(JACK: {} channels wired)", e.input_channels())
            }
            Some(e) => match e.input_device() {
                Some(d) if e.input_enabled() => d.to_string(),
                _ => "(off)".to_string(),
            },
            None => match a.input_device.as_deref() {
                Some(d) if !d.is_empty() => d.to_string(),
                Some(_) => "(default)".to_string(),
                None => "(off)".to_string(),
            },
        };
        let mut rows = vec![
            format!("  {:>14}  {}", "Backend", backend),
            format!("  {:>14}  {}", "Device", device),
            format!("  {:>14}  {}", "Input", input),
            format!(
                "  {:>14}  {} Hz",
                "Sample rate",
                pending(a.sample_rate, running.map(|r| r.0))
            ),
            format!(
                "  {:>14}  {} samples",
                "Buffer size",
                pending(a.buffer_size, running.map(|r| r.1))
            ),
            // The host clock every tempo-synced plugin reads. Shown from the
            // transport itself, which is the thing plugins actually see.
            format!(
                "  {:>14}  {:.1} BPM",
                "Tempo",
                choz_ports::transport().bpm()
            ),
            {
                let (num, den) = choz_ports::transport().time_signature();
                format!("  {:>14}  {num}/{den}", "Time signature")
            },
            // What it is doing right now, not just whether it is armed: a
            // guard that has caught something is the one thing worth seeing
            // here, and the IN drawer says it too.
            {
                let db = choz_engine::meter::capture_health().guard_db();
                let state = match (a.feedback_guard, db < -0.1) {
                    (false, _) => "OFF".to_string(),
                    (true, false) => "ON".to_string(),
                    (true, true) => format!("ON  (holding {db:.0} dB)"),
                };
                format!("  {:>14}  {state}", "Feedback guard")
            },
            // choz only builds oxisynth; the row exists so the setting matches
            // seqterm's file, not to pretend there is a choice.
            format!(
                "  {:>14}  {} (only engine built in)",
                "SF2 engine", a.sf2_engine
            ),
            format!("  {:>14}  {:.1} ms", "Latency", a.latency_ms()),
        ];
        // Backend-specific extras, read-only (edit them in the config file).
        match a.backend.to_uppercase().as_str() {
            "ALSA" => rows.push(format!(
                "  {:>14}  {}",
                "ALSA hw dev",
                if a.alsa_hw_device.is_empty() {
                    "(default)"
                } else {
                    &a.alsa_hw_device
                }
            )),
            "JACK" => rows.push(format!(
                "  {:>14}  {}",
                "JACK server",
                if a.jack_server_name.is_empty() {
                    "(default)"
                } else {
                    &a.jack_server_name
                }
            )),
            _ => rows.push(format!(
                "  {:>14}  {}",
                "PW quantum",
                if a.pipewire_quantum == 0 {
                    "system".to_string()
                } else {
                    a.pipewire_quantum.to_string()
                }
            )),
        }
        rows
    }

    /// OSC section rows, plus the live server status.
    fn osc_rows(&self) -> Vec<String> {
        let o = &self.ui.osc;
        let port_field = |row: usize, value: String| match &self.port_edit {
            Some(e) if e.row == row => format!("{}\u{2588}", e.buf),
            _ => value,
        };
        let mode = match o.port_mode {
            settings::OscPortMode::Specific => "Specific",
            settings::OscPortMode::Random => "Random",
        };
        let live = match self.osc_port {
            Some(p) => format!("UDP :{p} \u{25CF} listening"),
            None => "\u{25CB} stopped".to_string(),
        };
        vec![
            format!(
                "  {:>12}  {}",
                "Enable OSC",
                if o.enabled { "On" } else { "Off" }
            ),
            format!("  {:>12}  {mode}", "Port mode"),
            format!(
                "  {:>12}  {}",
                "UDP port",
                port_field(2, o.udp_port.to_string())
            ),
            format!(
                "  {:>12}  {}",
                "TCP port",
                port_field(
                    3,
                    format!("{}  (stored — the server is UDP-only)", o.tcp_port)
                )
            ),
            String::new(),
            format!("  {:>12}  {live}", "server"),
        ]
    }

    /// Which Settings tab is showing.
    fn settings_tab(&self) -> usize {
        self.modal
            .as_ref()
            .map(|m| m.list.filter)
            .unwrap_or(TAB_AUDIO)
    }

    /// Save the interface settings and push them into the drawing code.
    fn apply_ui_settings(&mut self) {
        self.ui.apply();
        self.ui.save();
        // The tab labels are themselves translated.
        if let Some(m) = self.modal.as_mut() {
            m.list.filters = SETTINGS_TABS
                .iter()
                .map(|t| i18n::t(t).to_string())
                .collect();
            m.list.title = format!(
                "{} \u{00B7} {}",
                i18n::t("SETTINGS"),
                i18n::t("PLUGIN PATHS")
            );
        }
        self.refresh_modal();
    }

    /// (format, index into that format's dirs) for each row of the paths modal.
    /// `None` on a format header row.
    fn path_rows(&self) -> Vec<(choz_engine::PluginFormat, Option<usize>)> {
        let mut rows = Vec::new();
        for &fmt in choz_engine::PluginFormat::ALL {
            rows.push((fmt, None));
            for i in 0..self.plugin_paths.dirs(fmt).len() {
                rows.push((fmt, Some(i)));
            }
        }
        rows
    }

    /// Keys of the AUDIO tab's Engine and OSC sections: `←→` change a value,
    /// `Enter` toggles or opens the port editor. Returns true when handled.
    fn audio_settings_key(&mut self, key: KeyCode) -> bool {
        let Some(m) = self.modal.as_ref() else {
            return false;
        };
        if m.kind != ModalKind::PluginPaths || m.list.filter != TAB_AUDIO {
            return false;
        }
        let section = m.list.sidebar_cursor;
        let row = m.list.cursor;
        if m.list.sidebar_focused {
            return false;
        }

        // A port being typed swallows everything until Enter/Esc.
        if let Some(mut edit) = self.port_edit.take() {
            match key {
                KeyCode::Char(c) if c.is_ascii_digit() && edit.buf.len() < 5 => edit.buf.push(c),
                KeyCode::Backspace => {
                    edit.buf.pop();
                }
                KeyCode::Enter => {
                    if let Ok(port) = edit.buf.parse::<u16>() {
                        match edit.row {
                            2 => self.ui.osc.udp_port = port,
                            3 => self.ui.osc.tcp_port = port,
                            _ => {}
                        }
                        self.ui.save();
                        if edit.row == 2 {
                            self.apply_osc_settings();
                        }
                    }
                    self.refresh_modal();
                    return true;
                }
                KeyCode::Esc => {
                    self.refresh_modal();
                    return true;
                }
                _ => {}
            }
            self.port_edit = Some(edit);
            self.refresh_modal();
            return true;
        }

        let step = match key {
            KeyCode::Left => -1,
            KeyCode::Right => 1,
            KeyCode::Enter => 0,
            _ => return false,
        };
        let cycle = |list: &[&str], cur: &str, step: isize| -> String {
            let n = list.len() as isize;
            let i = list.iter().position(|v| *v == cur).unwrap_or(0) as isize;
            list[(((i + step) % n + n) % n) as usize].to_string()
        };
        let cycle_num = |list: &[u32], cur: u32, step: isize| -> u32 {
            let n = list.len() as isize;
            let i = list.iter().position(|v| *v == cur).unwrap_or(0) as isize;
            list[(((i + step) % n + n) % n) as usize]
        };

        match (section, row) {
            (SEC_ENGINE, 0) if step != 0 => {
                self.ui.audio.backend = cycle(settings::BACKENDS, &self.ui.audio.backend, step);
            }
            // The device is the one place a change applies immediately.
            (SEC_ENGINE, 1) => {
                let devices = match self.audio_engine.as_ref() {
                    Some(e) => e.output_devices(),
                    None => return true,
                };
                if devices.is_empty() {
                    return true;
                }
                let cur = self
                    .audio_engine
                    .as_ref()
                    .and_then(|e| e.output_device().map(|d| d.to_string()))
                    .unwrap_or_default();
                let i = devices.iter().position(|d| *d == cur).unwrap_or(0) as isize;
                let n = devices.len() as isize;
                let next = devices[(((i + step.max(-1)) % n + n) % n) as usize].clone();
                if step != 0 {
                    self.ui.audio.device = next.clone();
                    self.set_output_device(&next);
                }
            }
            // The capture device, and it applies immediately like the output.
            // "(off)" is the first entry rather than a separate toggle: a
            // multi-effect that grabs the microphone on start-up is a host
            // nobody asked for, and turning it off is the same gesture.
            (SEC_ENGINE, 2) if step != 0 => {
                let mut devices = vec![String::new()];
                devices.extend(
                    self.audio_engine
                        .as_ref()
                        .map(|e| e.input_devices())
                        .unwrap_or_default(),
                );
                let cur = self
                    .audio_engine
                    .as_ref()
                    .filter(|e| e.input_enabled())
                    .and_then(|e| e.input_device().map(|d| d.to_string()))
                    .unwrap_or_default();
                let i = devices.iter().position(|d| *d == cur).unwrap_or(0) as isize;
                let n = devices.len() as isize;
                let next = devices[(((i + step) % n + n) % n) as usize].clone();
                self.ui.audio.input_device = (!next.is_empty()).then(|| next.clone());
                self.set_input_device((!next.is_empty()).then_some(next));
            }
            (SEC_ENGINE, 3) if step != 0 => {
                self.ui.audio.sample_rate =
                    cycle_num(settings::SAMPLE_RATES, self.ui.audio.sample_rate, step);
            }
            (SEC_ENGINE, 4) if step != 0 => {
                self.ui.audio.buffer_size =
                    cycle_num(settings::BUFFER_SIZES, self.ui.audio.buffer_size, step);
            }
            // The tempo applies at once: it is a number a plugin reads on the
            // next block, not something the stream has to be rebuilt for.
            (SEC_ENGINE, 5) if step != 0 => {
                let t = choz_ports::transport();
                t.set_bpm(t.bpm() + step as f32 * BPM_STEP);
                self.ui.audio.bpm = t.bpm();
                self.ui.save();
            }
            // Cycles through the signatures a bar can actually be written in.
            (SEC_ENGINE, 6) if step != 0 => {
                let t = choz_ports::transport();
                let cur = t.time_signature();
                let i = TIME_SIGS.iter().position(|s| *s == cur).unwrap_or(0) as isize;
                let n = TIME_SIGS.len() as isize;
                let (num, den) = TIME_SIGS[(((i + step) % n + n) % n) as usize];
                t.set_time_signature(num, den);
                self.ui.audio.time_sig = (num, den);
                self.ui.save();
            }
            // The guard is a switch, so either arrow — or Enter — flips it.
            (SEC_ENGINE, 7) => {
                self.ui.audio.feedback_guard = !self.ui.audio.feedback_guard;
                choz_engine::feedback::arm(self.ui.audio.feedback_guard);
                self.ui.save();
                self.refresh_modal();
                return true;
            }
            // SF2 engine and the read-only rows below it take no input.
            (SEC_ENGINE, _) => return true,

            (SEC_OSC, 0) => {
                self.ui.osc.enabled = !self.ui.osc.enabled;
                self.ui.save();
                self.apply_osc_settings();
                self.refresh_modal();
                return true;
            }
            (SEC_OSC, 1) => {
                self.ui.osc.port_mode = match self.ui.osc.port_mode {
                    settings::OscPortMode::Specific => settings::OscPortMode::Random,
                    settings::OscPortMode::Random => settings::OscPortMode::Specific,
                };
                self.ui.save();
                self.apply_osc_settings();
                self.refresh_modal();
                return true;
            }
            (SEC_OSC, r @ (2 | 3)) => {
                let cur = if r == 2 {
                    self.ui.osc.udp_port
                } else {
                    self.ui.osc.tcp_port
                };
                if step == 0 {
                    self.port_edit = Some(PortEdit {
                        row: r,
                        buf: cur.to_string(),
                    });
                } else {
                    let next = (cur as i32 + step as i32).clamp(1, 65_535) as u16;
                    if r == 2 {
                        self.ui.osc.udp_port = next;
                    } else {
                        self.ui.osc.tcp_port = next;
                    }
                }
            }
            _ => return false,
        }
        self.ui.save();
        self.refresh_modal();
        true
    }

    /// Apply a key that only the paths modal understands. Returns true when it
    /// was handled.
    fn paths_modal_key(&mut self, key: KeyCode) -> bool {
        let Some(m) = self.modal.as_ref() else {
            return false;
        };
        // Only the Plugin Paths section of the AUDIO tab takes these keys.
        if m.kind != ModalKind::PluginPaths
            || m.list.filter != TAB_AUDIO
            || m.list.sidebar_cursor != SEC_PATHS
        {
            return false;
        }
        // While a path is being typed, every key belongs to the editor.
        if let Some(mut edit) = self.path_edit.take() {
            if let Some(m) = self.modal.as_mut() {
                let fmt = edit.fmt.label();
                m.list.note = format!(
                    "  typing a {fmt} path \u{00B7} Enter=save  Esc=cancel  (empty = remove)"
                );
            }
            match edit.text.key(key) {
                Some(true) => self.commit_path_edit(edit),
                Some(false) => {}
                None => {
                    self.path_edit = Some(edit);
                    self.refresh_modal();
                    return true;
                }
            }
            if let Some(m) = self.modal.as_mut() {
                m.list.note = "  Enter=on/off  \u{00B7}  the buttons below act on the selected row"
                    .to_string();
            }
            self.refresh_modal();
            return true;
        }

        let cursor = m.list.cursor;
        let Some(&(fmt, dir)) = self.path_rows().get(cursor) else {
            return false;
        };
        match key {
            // Enter toggles a directory on/off (a header row has nothing to do).
            KeyCode::Enter => {
                if let Some(i) = dir {
                    if let Some(d) = self.plugin_paths.dirs_mut(fmt).get_mut(i) {
                        d.enabled = !d.enabled;
                    }
                }
            }
            // Type a path in place: `e` rewrites the selected one, `a` starts a
            // new (empty) one under the format the cursor is in.
            KeyCode::Char('e') => {
                if let Some(i) = dir {
                    let current = self.plugin_paths.dirs(fmt)[i].path.display().to_string();
                    self.path_edit = Some(PathEdit::new(fmt, Some(i), current));
                    self.refresh_modal();
                    return true;
                }
            }
            KeyCode::Char('a') => {
                self.path_edit = Some(PathEdit::new(fmt, None, String::new()));
                self.refresh_modal();
                return true;
            }
            // `b` picks a directory with the browser instead of typing it.
            KeyCode::Char('b') => {
                self.paths_format = Some(fmt);
                self.open_dir_picker();
                return true;
            }
            KeyCode::Char('d') => {
                if let Some(i) = dir {
                    self.plugin_paths.dirs_mut(fmt).remove(i);
                }
            }
            KeyCode::Char('r') => {
                let defaults = choz_engine::PluginPaths::default();
                *self.plugin_paths.dirs_mut(fmt) = defaults.dirs(fmt).to_vec();
            }
            _ => return false,
        }
        self.plugin_paths.save();
        self.paths_dirty = true;
        self.refresh_modal();
        true
    }

    /// Store a typed path: replaces the row it came from, or appends a new one.
    /// An empty buffer means "forget it" (and deletes the row when editing).
    fn commit_path_edit(&mut self, edit: PathEdit) {
        let text = edit.text.buf.trim().to_string();
        let dirs = self.plugin_paths.dirs_mut(edit.fmt);
        match (edit.dir, text.is_empty()) {
            (Some(i), true) => {
                dirs.remove(i);
            }
            (Some(i), false) => {
                if let Some(d) = dirs.get_mut(i) {
                    d.path = text.into();
                }
            }
            (None, false) => dirs.push(choz_engine::SearchDir {
                path: text.into(),
                enabled: true,
            }),
            (None, true) => {}
        }
        self.plugin_paths.save();
        self.paths_dirty = true;
    }

    /// Directory picker used by the paths modal's `b` key.
    fn open_dir_picker(&mut self) {
        let start = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("/"));
        let mut modal = Modal::new(
            ModalKind::AddPath,
            views::modal::ListModal::new("ADD SEARCH DIRECTORY", Vec::new()),
        );
        modal.browser = Some(file_browser::FileBrowser::open(
            &start,
            file_browser::DIR_PICK,
        ));
        self.modal = Some(modal);
        self.refresh_modal();
    }

    /// Rebuild the open modal's visible item list from its data and filter.
    /// Called on open, on filter change, and after anything that changes the
    /// underlying data (browsing into a directory, turning a knob).
    fn refresh_modal(&mut self) {
        let Some(kind) = self.modal.as_ref().map(|m| m.kind) else {
            return;
        };
        // Set by the Plugin paths arm when a path is being typed, so the caret
        // row stays selected (and therefore visible) while the text changes.
        let mut edit_row: Option<usize> = None;
        let items: Vec<String> = match kind {
            ModalKind::Source => {
                let m = self.modal.as_ref().unwrap();
                let fmt = SOURCE_FORMATS[m.list.filter];
                m.sources
                    .iter()
                    .filter(|c| fmt == "ALL" || c.fmt == fmt)
                    .map(|c| format!("[{}] {}", c.fmt, c.label))
                    .collect()
            }
            ModalKind::AddFx => {
                let sidebar: Vec<(String, usize)> = self
                    .fx_categories()
                    .into_iter()
                    .map(|(cat, n)| (cat.map(|c| c.label()).unwrap_or("ALL").to_string(), n))
                    .collect();
                if let Some(m) = self.modal.as_mut() {
                    let last = sidebar.len().saturating_sub(1);
                    m.list.sidebar_cursor = m.list.sidebar_cursor.min(last);
                    m.list.sidebar = sidebar;
                }
                self.fx_menu_rows()
                    .into_iter()
                    .map(|(_, label)| label)
                    .collect()
            }
            ModalKind::Metronome => self.metronome_rows(),
            ModalKind::Preset => self
                .preset_rows()
                .into_iter()
                .map(|(_, label)| label)
                .collect(),
            // The list was built when the modal opened and the parameter has
            // not moved since; rebuilding it here would only re-read the same
            // names.
            ModalKind::FxChoice | ModalKind::FxPreset | ModalKind::ArpChoice => return,
            // Built when the import ran; there is nothing to re-read.
            ModalKind::MaxReport => return,
            ModalKind::Learn => {
                let targets = self.modal.as_ref().unwrap().targets.clone();
                targets
                    .iter()
                    .map(|t| {
                        let bound = self
                            .cc_bindings
                            .iter()
                            .find(|b| b.target == *t)
                            .map(|b| match &b.source {
                                // Which keyboard it answers to, because with two
                                // of them "CC 74" alone does not say.
                                Some(src) => format!("   [CC {} \u{00B7} {}]", b.cc, src.name()),
                                None => format!("   [CC {}]", b.cc),
                            })
                            .or_else(|| {
                                self.pc_bindings
                                    .iter()
                                    .find(|(_, b)| b == t)
                                    .map(|(p, _)| format!("   [PC {p}]"))
                            })
                            .unwrap_or_default();
                        format!("{}{}", self.learn_label(t), bound)
                    })
                    .collect()
            }
            ModalKind::Browser | ModalKind::Wallpaper | ModalKind::Bank => self
                .modal
                .as_ref()
                .unwrap()
                .browser
                .as_ref()
                .map(|b| b.entries.iter().map(|e| e.label.clone()).collect())
                .unwrap_or_default(),
            ModalKind::PluginPaths
                if self.settings_tab() == TAB_AUDIO && self.audio_section() == SEC_ENGINE =>
            {
                self.engine_rows()
            }
            ModalKind::PluginPaths
                if self.settings_tab() == TAB_AUDIO && self.audio_section() == SEC_OSC =>
            {
                self.osc_rows()
            }
            ModalKind::PluginPaths if self.settings_tab() == TAB_THEME => self.theme_rows(),
            ModalKind::PluginPaths if self.settings_tab() == TAB_LANG => i18n::Lang::ALL
                .iter()
                .map(|l| {
                    let mark = if *l == self.ui.language {
                        "\u{25CF}"
                    } else {
                        "\u{25CB}"
                    };
                    format!("  {mark} {}   ({})", l.label(), l.code())
                })
                .collect(),
            ModalKind::PluginPaths => {
                let mut rows: Vec<String> = self
                    .path_rows()
                    .into_iter()
                    .map(|(fmt, dir)| match dir {
                        None => fmt.label().to_string(),
                        Some(i) => {
                            let d = &self.plugin_paths.dirs(fmt)[i];
                            let mark = if d.enabled { "\u{2713}" } else { "\u{00B7}" };
                            format!("    {mark} {}{}", d.path.display(), self.dir_hint(fmt, d))
                        }
                    })
                    .collect();
                // The editor replaces the row it rewrites, or is inserted right
                // under its format header when it's a new entry.
                if let Some(edit) = self.path_edit.as_ref() {
                    let at = self
                        .path_rows()
                        .iter()
                        .position(|&(f, d)| f == edit.fmt && d == edit.dir)
                        .or_else(|| {
                            self.path_rows()
                                .iter()
                                .rposition(|&(f, _)| f == edit.fmt)
                                .map(|i| i + 1)
                        });
                    let row = match (at, edit.dir) {
                        (Some(i), Some(_)) if i < rows.len() => {
                            rows[i] = edit.display();
                            i
                        }
                        (Some(i), _) => {
                            let i = i.min(rows.len());
                            rows.insert(i, edit.display());
                            i
                        }
                        (None, _) => {
                            rows.push(edit.display());
                            rows.len() - 1
                        }
                    };
                    edit_row = Some(row);
                }
                rows
            }
            ModalKind::AddPath
            | ModalKind::SaveProject
            | ModalKind::LoadProject
            | ModalKind::ImportMax => self
                .modal
                .as_ref()
                .unwrap()
                .browser
                .as_ref()
                .map(|b| b.entries.iter().map(|e| e.label.clone()).collect())
                .unwrap_or_default(),
            ModalKind::InstrParams => self
                .slots
                .get(self.active_slot)
                .map(|s| {
                    s.instr_params
                        .iter()
                        .enumerate()
                        .map(|(i, p)| {
                            instr_param_row(p, s.instr_values.get(i).copied().unwrap_or(0.0))
                        })
                        .collect()
                })
                .unwrap_or_default(),
        };
        if kind == ModalKind::PluginPaths {
            let tab = self.settings_tab();
            let section = self.audio_section();
            let mut title = format!(
                "{} \u{00B7} {}",
                i18n::t("SETTINGS"),
                i18n::t(SETTINGS_TABS[tab.min(2)])
            );
            // The AUDIO tab splits into Engine / Plugin Paths / OSC down the
            // side, the way seqterm's AUDIO SETTINGS does.
            let sidebar: Vec<(String, usize)> = if tab == TAB_AUDIO {
                title = format!("{title} \u{00B7} {}", AUDIO_SECTIONS[section.min(2)]);
                AUDIO_SECTIONS
                    .iter()
                    .enumerate()
                    .map(|(i, name)| {
                        let n = match i {
                            SEC_ENGINE => ENGINE_ROWS.len(),
                            SEC_PATHS => self.plugin_paths.all_enabled().len(),
                            _ => OSC_ROWS.len(),
                        };
                        (name.to_string(), n)
                    })
                    .collect()
            } else {
                Vec::new()
            };
            let (note, actions) = match (tab, section) {
                (TAB_AUDIO, SEC_ENGINE) => (
                    "  \u{2190}\u{2192}=change  \u{00B7}  backend, sample rate and buffer apply on the next start"
                        .to_string(),
                    Vec::new(),
                ),
                (TAB_AUDIO, SEC_OSC) => (
                    "  \u{2190}\u{2192}=change  Enter=toggle / type a port  \u{00B7}  applies immediately"
                        .to_string(),
                    Vec::new(),
                ),
                (TAB_THEME, _) => (
                    "  Enter / SELECT applies the colour to text and borders, and closes"
                        .to_string(),
                    Vec::new(),
                ),
                (TAB_LANG, _) => (
                    "  Enter / SELECT switches the interface language and closes".to_string(),
                    Vec::new(),
                ),
                // Plugin Paths.
                _ => (
                    "  Enter=on/off  \u{00B7}  the buttons below act on the selected row".to_string(),
                    vec![
                        (i18n::t("EDIT").to_string(), 'e'),
                        (i18n::t("ADD").to_string(), 'a'),
                        (i18n::t("BROWSE").to_string(), 'b'),
                        (i18n::t("REMOVE").to_string(), 'd'),
                        (i18n::t("DEFAULTS").to_string(), 'r'),
                    ],
                ),
            };
            if let Some(m) = self.modal.as_mut() {
                m.list.title = title;
                // The path editor sets its own note while it is typing.
                if self.path_edit.is_none() {
                    m.list.note = note;
                }
                m.list.actions = actions;
                let last = sidebar.len().saturating_sub(1);
                m.list.sidebar_cursor = m.list.sidebar_cursor.min(last);
                m.list.sidebar = sidebar;
            }
        }

        if let Some(m) = self.modal.as_mut() {
            if let Some(row) = edit_row {
                m.list.cursor = row;
            }
            // …unless the overwrite question is up, whose two rows own the
            // cursor: taking the browser's back would put it on "overwrite".
            let confirming = self.save_name.as_ref().is_some_and(|n| n.confirm);
            if let (
                ModalKind::Browser
                | ModalKind::Bank
                | ModalKind::AddPath
                | ModalKind::SaveProject
                | ModalKind::LoadProject,
                Some(b),
            ) = (m.kind, m.browser.as_ref())
            {
                m.list.note = format!("  {}", b.dir.display());
                if !confirming {
                    m.list.cursor = b.cursor;
                }
            }
            // The name prompt owns the note line while it is open, and an
            // overwrite replaces the listing outright: a question with two
            // answers, not a hint under a file browser nobody re-reads.
            if let (ModalKind::SaveProject, Some(n)) = (m.kind, self.save_name.as_ref()) {
                m.list.note = n.note();
                if n.confirm {
                    m.list.title = i18n::t("OVERWRITE PROJECT?").to_string();
                    m.list.items = vec![
                        format!(
                            "  {} \u{2014} {}",
                            i18n::t("OVERWRITE"),
                            n.target().display()
                        ),
                        format!("  {}", i18n::t("RENAME INSTEAD")),
                    ];
                    m.list.cursor = m.list.cursor.min(1);
                    m.list.scroll = 0;
                    return;
                }
                m.list.title = "SAVE PROJECT".to_string();
            }
            m.list.items = items;
            let last = m.list.items.len().saturating_sub(1);
            m.list.cursor = m.list.cursor.min(last);
        }
    }

    /// Human label for a learn target (also used by the INPUTS banner).
    fn learn_label(&self, t: &LearnTarget) -> String {
        match *t {
            LearnTarget::Gain(s) => format!("tab {} \u{00B7} VOL", s + 1),
            LearnTarget::Pan(s) => format!("tab {} \u{00B7} PAN", s + 1),
            LearnTarget::InGain(s) => format!("tab {} \u{00B7} IN", s + 1),
            LearnTarget::InGate(s) => format!("tab {} \u{00B7} SENS", s + 1),
            LearnTarget::Trigger(action) => action.label(),
            LearnTarget::InstrParam { slot, param } => {
                let name = self
                    .slots
                    .get(slot)
                    .and_then(|s| s.instr_params.get(param))
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| format!("p{}", param + 1));
                format!("INSTR \u{00B7} {name}")
            }
            LearnTarget::FxParam { slot, fx, param } => {
                let (label, pname) = self
                    .slots
                    .get(slot)
                    .and_then(|s| s.fx_chain.get(fx).or_else(|| self.fx_chain.get(fx)))
                    .map(|e| {
                        let p = e
                            .param_descs()
                            .get(param)
                            .map(|d| d.name.to_string())
                            .unwrap_or_else(|| format!("p{}", param + 1));
                        (e.label().to_string(), p)
                    })
                    .unwrap_or_else(|| ("FX".to_string(), format!("p{}", param + 1)));
                format!("tab {} \u{00B7} {}:{} {}", slot + 1, fx + 1, label, pname)
            }
        }
    }

    /// Apply the modal's current selection. Returns true when the modal should
    /// close (browsing into a directory keeps it open).
    fn modal_select(&mut self) -> bool {
        let Some(m) = self.modal.as_ref() else {
            return true;
        };
        let i = m.list.cursor;
        match m.kind {
            ModalKind::Source => {
                let fmt = SOURCE_FORMATS[m.list.filter];
                let choice = m
                    .sources
                    .iter()
                    .filter(|c| fmt == "ALL" || c.fmt == fmt)
                    .nth(i)
                    .cloned();
                match choice.map(|c| c.action) {
                    Some(SourceAction::Plugin { format, path, id }) => {
                        self.load_plugin_source(format, &path, &id);
                        true
                    }
                    Some(SourceAction::File(path)) => {
                        self.request_load_source(path);
                        true
                    }
                    Some(SourceAction::Browse(ext)) => {
                        self.open_browser_modal(ext);
                        false
                    }
                    Some(SourceAction::Unsupported(fmt)) => {
                        eprintln!("choz: {fmt} hosting is not implemented yet");
                        true
                    }
                    None => true,
                }
            }
            ModalKind::AddFx => match self.fx_menu_rows().get(i).and_then(|(idx, _)| *idx) {
                Some(idx) => {
                    self.add_fx_at(idx);
                    true
                }
                None => false,
            },
            ModalKind::Preset => {
                // The row is a position in the filtered view; what gets applied
                // is the preset it points at.
                let Some((index, _)) = self.preset_rows().into_iter().nth(i) else {
                    return false;
                };
                if let Some(slot) = self.slots.get_mut(self.active_slot) {
                    slot.preset_cursor = index;
                }
                self.apply_selected_preset();
                true
            }
            ModalKind::Metronome => {
                self.step_metronome_row(i, 1);
                // Stays open: the point is to hear the change and step again.
                false
            }
            ModalKind::Learn => {
                self.learn = m.targets.get(i).copied();
                true
            }
            ModalKind::FxPreset => {
                self.load_fx_preset(i);
                true
            }
            ModalKind::ArpChoice => {
                let index = m.fx_param;
                // The list is the knob's own positions, so the value is where
                // the chosen one sits — not `i / len`, which would be a grid
                // the knob never had.
                let value = self
                    .arp_knobs()
                    .get(index)
                    .and_then(|(_, _, _, shape)| match shape {
                        source::ParamShape::Named(points) => points.get(i).map(|(v, _)| *v),
                        _ => None,
                    });
                if let Some(v) = value {
                    self.set_arp_knob(index, v);
                }
                true
            }
            ModalKind::FxChoice => {
                let param = m.fx_param;
                // The list is the parameter's own positions, so the value is
                // wherever the chosen one sits — not `i / len`, which would be
                // a uniform grid the parameter never had.
                let value = self
                    .fx_chain
                    .get(self.fx_slot)
                    .and_then(|e| e.param_descs().get(param).cloned())
                    .and_then(|d| match d.shape {
                        source::ParamShape::Named(points) => points.get(i).map(|(v, _)| *v),
                        _ => None,
                    });
                if let Some(v) = value {
                    self.set_fx_param(self.fx_slot, param, v);
                }
                true
            }
            ModalKind::Browser | ModalKind::Wallpaper | ModalKind::Bank => {
                let wallpaper = m.kind == ModalKind::Wallpaper;
                let bank = m.kind == ModalKind::Bank;
                let action = self.modal.as_mut().and_then(|m| {
                    let b = m.browser.as_mut()?;
                    b.cursor = i;
                    b.select()
                });
                match action {
                    Some(file_browser::Action::EnterDir(d)) => {
                        if let Some(b) = self.modal.as_mut().and_then(|m| m.browser.as_mut()) {
                            b.set_dir(d);
                        }
                        self.refresh_modal();
                        false
                    }
                    Some(file_browser::Action::PickFile(path)) if wallpaper => {
                        // Keep whatever fit was already chosen, so swapping the
                        // picture does not silently reset tile back to stretch.
                        let fit = match &self.ui.background {
                            settings::Background::Image { fit, .. } => *fit,
                            _ => settings::ImageFit::default(),
                        };
                        self.ui.background = settings::Background::Image { path, fit };
                        self.apply_ui_settings();
                        // Straight back to the theme tab, where the result shows.
                        self.open_paths_modal();
                        if let Some(m) = self.modal.as_mut() {
                            m.list.filter = TAB_THEME;
                            m.list.sidebar_focused = false;
                        }
                        self.refresh_modal();
                        false
                    }
                    Some(file_browser::Action::PickFile(path)) if bank => {
                        self.set_bank_dir(path);
                        // `set_bank_dir` opens the patch list on top; closing
                        // here would take that with it.
                        false
                    }
                    Some(file_browser::Action::PickFile(path)) => {
                        self.request_load_source(path);
                        true
                    }
                    None => true,
                }
            }
            ModalKind::PluginPaths => {
                if self.settings_tab() == TAB_AUDIO {
                    // Handled by `audio_settings_key` / `paths_modal_key`; the
                    // modal stays open so several values can be set at once.
                    return false;
                }
                match self.settings_tab() {
                    TAB_THEME => self.theme_select(i),
                    TAB_LANG => {
                        if let Some(&lang) = i18n::Lang::ALL.get(i) {
                            self.ui.language = lang;
                            self.apply_ui_settings();
                        }
                        true
                    }
                    // Enter on the paths tab is handled by `paths_modal_key`
                    // (it toggles a directory), so the modal stays open.
                    _ => false,
                }
            }
            ModalKind::LoadProject => {
                let picked = self.modal.as_mut().and_then(|m| {
                    let b = m.browser.as_mut()?;
                    b.cursor = i;
                    b.select()
                });
                match picked {
                    Some(file_browser::Action::EnterDir(d)) => {
                        if let Some(b) = self.modal.as_mut().and_then(|m| m.browser.as_mut()) {
                            b.set_dir(d);
                        }
                        self.refresh_modal();
                        false
                    }
                    Some(file_browser::Action::PickFile(file)) => {
                        self.load_project_from(&file);
                        true
                    }
                    None => true,
                }
            }
            ModalKind::ImportMax => {
                let picked = self.modal.as_mut().and_then(|m| {
                    let b = m.browser.as_mut()?;
                    b.cursor = i;
                    b.select()
                });
                match picked {
                    Some(file_browser::Action::EnterDir(d)) => {
                        if let Some(b) = self.modal.as_mut().and_then(|m| m.browser.as_mut()) {
                            b.set_dir(d);
                        }
                        self.refresh_modal();
                        false
                    }
                    Some(file_browser::Action::PickFile(file)) => {
                        self.import_maxpat(&file);
                        // The report replaces this modal rather than closing it:
                        // what an import could **not** do is the half worth
                        // reading, and a log line is not where anybody looks.
                        false
                    }
                    None => true,
                }
            }
            // Nothing to select: it is a list of what happened.
            ModalKind::MaxReport => true,
            ModalKind::SaveProject => {
                // While the name is being typed the list is only a backdrop:
                // a stray click must not re-pick the directory under it. The
                // overwrite question, though, *is* the list.
                if let Some(n) = self.save_name.as_ref() {
                    if n.confirm {
                        if let Some(m) = self.modal.as_mut() {
                            m.list.cursor = i;
                        }
                        self.save_name_key(KeyCode::Enter);
                        // Answering either way leaves the modal where it is:
                        // saved closes it already, renaming goes on typing.
                        return false;
                    }
                    return false;
                }
                let picked = self.modal.as_mut().and_then(|m| {
                    let b = m.browser.as_mut()?;
                    b.cursor = i;
                    b.select()
                });
                match picked {
                    Some(file_browser::Action::EnterDir(d)) => {
                        if let Some(b) = self.modal.as_mut().and_then(|m| m.browser.as_mut()) {
                            b.set_dir(d);
                        }
                        self.refresh_modal();
                        false
                    }
                    // Picking the directory only asks for the name; the save
                    // itself is `save_name_key`'s Enter.
                    Some(file_browser::Action::PickFile(dir)) => {
                        let name = self
                            .project_file
                            .as_ref()
                            .and_then(|f| f.file_name())
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| project::DEFAULT_NAME.to_string());
                        self.save_name = Some(SaveName::new(dir, name));
                        self.refresh_modal();
                        false
                    }
                    None => true,
                }
            }
            ModalKind::AddPath => {
                let picked = self.modal.as_mut().and_then(|m| {
                    let b = m.browser.as_mut()?;
                    b.cursor = i;
                    b.select()
                });
                match picked {
                    Some(file_browser::Action::EnterDir(d)) => {
                        if let Some(b) = self.modal.as_mut().and_then(|m| m.browser.as_mut()) {
                            b.set_dir(d);
                        }
                        self.refresh_modal();
                        false
                    }
                    Some(file_browser::Action::PickFile(dir)) => {
                        if let Some(fmt) = self.paths_format.take() {
                            self.plugin_paths
                                .dirs_mut(fmt)
                                .push(choz_engine::SearchDir {
                                    path: dir,
                                    enabled: true,
                                });
                            self.plugin_paths.save();
                            self.paths_dirty = true;
                        }
                        self.open_paths_modal();
                        false
                    }
                    None => true,
                }
            }
            // Nothing to "select": the value is edited in place with the arrows.
            ModalKind::InstrParams => true,
        }
    }

    /// Everything ADD FX can offer, in one flat list: built-ins first (in
    /// `ALL_FX_KINDS` order), then the scanned plugins — the index of an entry
    /// is what [`Self::add_fx_at`] takes.
    fn fx_menu_entries(&self) -> Vec<FxMenuEntry> {
        ALL_FX_KINDS
            .iter()
            .map(|k| FxMenuEntry {
                format: None,
                category: k.category(),
                label: k.label().to_string(),
                hosted: true,
            })
            .chain(
                self.plugin_fx_entries()
                    .into_iter()
                    .map(|(fmt, name, hosted)| FxMenuEntry {
                        format: Some(fmt),
                        category: source::FxCategory::guess(&name),
                        label: name,
                        hosted,
                    }),
            )
            .collect()
    }

    /// Categories that have anything in them under the current format chip,
    /// with their counts — this is the ADD FX sidebar. "ALL" comes first.
    fn fx_categories(&self) -> Vec<(Option<source::FxCategory>, usize)> {
        let wanted = self.fx_format_filter();
        let entries = self.fx_menu_entries();
        let matching: Vec<&FxMenuEntry> = entries
            .iter()
            .filter(|e| e.matches_filter(wanted))
            .collect();
        let mut out = vec![(None, matching.len())];
        for &cat in source::FxCategory::ALL {
            let n = matching.iter().filter(|e| e.category == cat).count();
            if n > 0 {
                out.push((Some(cat), n));
            }
        }
        out
    }

    fn fx_format_filter(&self) -> &'static str {
        let filter = self.modal.as_ref().map(|m| m.list.filter).unwrap_or(0);
        FX_FORMATS.get(filter).copied().unwrap_or("ALL")
    }

    /// The ADD FX list as it is shown: the entries of the category selected in
    /// the sidebar, under the current format chip. The `usize` is the index
    /// into [`Self::fx_menu_entries`].
    fn fx_menu_rows(&self) -> Vec<(Option<usize>, String)> {
        let wanted = self.fx_format_filter();
        let section = self
            .modal
            .as_ref()
            .map(|m| m.list.sidebar_cursor)
            .and_then(|i| self.fx_categories().get(i).map(|(c, _)| *c))
            .unwrap_or(None);
        self.fx_menu_entries()
            .into_iter()
            .enumerate()
            .filter(|(_, e)| e.matches_filter(wanted) && section.is_none_or(|c| e.category == c))
            .map(|(i, e)| {
                let fmt = e.format.map(|f| f.label()).unwrap_or("BUILT-IN");
                let mark = if e.hosted { "" } else { "  (not hosted yet)" };
                (Some(i), format!(" [{fmt}] {}{mark}", e.label))
            })
            .collect()
    }

    /// Load a plugin instrument by path+id into the active tab (SOURCE picker).
    fn load_plugin_source(
        &mut self,
        format: choz_engine::PluginFormat,
        path: &std::path::Path,
        id: &str,
    ) {
        if let Some(i) = self
            .synths
            .iter()
            .position(|s| s.id == id && s.path == path && s.format == format)
        {
            self.request_load_synth(i);
        }
    }

    /// Everything choz is set up to do right now, ready to be written out.
    fn project_snapshot(&mut self) -> project::Project {
        self.persist_active();
        let engine = self.audio_engine.as_ref();
        let rack = self
            .slots
            .iter()
            .enumerate()
            .map(|(idx, slot)| {
                let instrument = match &slot.source {
                    AudioSource::Midi => project::Instrument {
                        kind: "none".into(),
                        path: None,
                        id: None,
                        name: None,
                        bank: None,
                        preset: None,
                        params: Vec::new(),
                        state: String::new(),
                        bank_dir: None,
                        config: Vec::new(),
                    },
                    AudioSource::Sf2 { path, bank, preset } => project::Instrument {
                        kind: "sf2".into(),
                        path: Some(path.clone()),
                        id: None,
                        name: None,
                        bank: Some(*bank),
                        preset: Some(*preset),
                        params: Vec::new(),
                        state: String::new(),
                        bank_dir: None,
                        config: Vec::new(),
                    },
                    AudioSource::AudioFile { path, .. } => project::Instrument {
                        kind: "wav".into(),
                        path: Some(path.clone()),
                        id: None,
                        name: None,
                        bank: None,
                        preset: None,
                        params: Vec::new(),
                        state: String::new(),
                        bank_dir: None,
                        config: Vec::new(),
                    },
                    AudioSource::Plugin { id, name, .. } => project::Instrument {
                        kind: "plugin".into(),
                        path: self
                            .synths
                            .iter()
                            .find(|s| s.id == *id)
                            .map(|s| s.path.clone()),
                        id: Some(id.clone()),
                        name: Some(name.clone()),
                        bank: None,
                        preset: None,
                        params: slot.instr_values.clone(),
                        // The patch itself: what the parameter list cannot say.
                        // Asked of the live plugin, so a tab that has been
                        // edited in its own window saves what is actually
                        // sounding.
                        state: engine
                            .and_then(|e| e.slot_state(idx))
                            .or_else(|| {
                                (!slot.instr_state.is_empty()).then(|| slot.instr_state.clone())
                            })
                            .map(|b| project::encode_state(&b))
                            .unwrap_or_default(),
                        bank_dir: slot.preset_dir.clone(),
                        config: slot.dssi_config.clone(),
                    },
                };
                let fx = slot
                    .fx_chain
                    .iter()
                    .enumerate()
                    .map(|(ui_fx, e)| {
                        let spec = e.to_spec();
                        // The engine only holds the *enabled* entries, so the
                        // index of this one there is how many enabled ones come
                        // before it — the same arithmetic `engine_fx_index`
                        // does for the active tab.
                        let engine_fx = e
                            .enabled
                            .then(|| slot.fx_chain[..ui_fx].iter().filter(|x| x.enabled).count());
                        project::Fx {
                            // Hosted effects record their host format instead
                            // of a built-in FX id.
                            kind: match &e.plugin {
                                Some(p) => p.format.label().to_lowercase(),
                                None => spec.kind,
                            },
                            enabled: spec.enabled,
                            wet: spec.wet,
                            params: spec.params,
                            plugin_path: e.plugin.as_ref().map(|c| c.path.clone()),
                            plugin_id: e.plugin.as_ref().map(|c| c.id.clone()),
                            state: engine
                                .zip(engine_fx)
                                .and_then(|(eng, i)| eng.fx_state(idx, i))
                                .or_else(|| (!e.state.is_empty()).then(|| e.state.clone()))
                                .map(|b| project::encode_state(&b))
                                .unwrap_or_default(),
                        }
                    })
                    .collect();
                // Only the bindings that point at this tab.
                let midi_learn = self
                    .cc_bindings
                    .iter()
                    .filter(|b| match &b.target {
                        LearnTarget::Gain(s)
                        | LearnTarget::Pan(s)
                        | LearnTarget::InGain(s)
                        | LearnTarget::InGate(s) => *s == idx,
                        LearnTarget::FxParam { slot, .. }
                        | LearnTarget::InstrParam { slot, .. } => *slot == idx,
                        LearnTarget::Trigger(_) => idx == self.active_slot,
                    })
                    .map(|b| project::Binding {
                        cc: b.cc,
                        target: b.target,
                        label: self.learn_label(&b.target),
                        // Which controller it was learned from, written the way
                        // a tab's own input is.
                        source: b.source.as_ref().map(|i| match i {
                            InputRef::Midi(name) => format!("MIDI:{name}"),
                            InputRef::Osc => "OSC".to_string(),
                        }),
                    })
                    .collect();
                project::Slot {
                    channel: slot.channel,
                    input: slot.input.as_ref().map(|i| match i {
                        InputRef::Midi(name) => format!("MIDI:{name}"),
                        InputRef::Osc => "OSC".to_string(),
                    }),
                    instrument,
                    mixer: project::Mixer {
                        pitch_to_midi: slot.pitch_to_midi,
                        pitch_mix: Some(slot.pitch_mix),
                        in_gain: Some(slot.in_gain),
                        in_gate: Some(slot.in_gate),
                        gain: slot.gain,
                        gain_r: (!slot.link).then_some(slot.gain_r),
                        link: (!slot.link).then_some(false),
                        pan: slot.pan,
                        mute: slot.mute,
                        solo: slot.solo,
                        out_pair: Some(slot.out_pair),
                        in_pair: slot.in_pair,
                        // The jacks by name, which is what survives an
                        // interface being unplugged.
                        in_ports: slot.in_pair.and_then(|(l, r)| {
                            Some((self.in_ports.get(l)?.clone(), self.in_ports.get(r)?.clone()))
                        }),
                    },
                    fx,
                    midi_learn,
                    midi_out: slot.midi_out.clone(),
                    arp: slot.arp.settings,
                }
            })
            .collect();

        project::Project {
            version: 1,
            automation: self.automation.clone(),
            audio: project::Audio {
                // Settings, not the live engine: sample rate, buffer and backend
                // only take effect on the next start, so the engine still runs
                // the *previous* values. Saving those threw away the change the
                // user just made. The device is the other way round — it moves
                // live, so the engine knows the real one.
                sample_rate: self.ui.audio.sample_rate,
                buffer_size: self.ui.audio.buffer_size,
                backend: self.ui.audio.backend.clone(),
                output_device: engine.and_then(|e| e.output_device().map(|d| d.to_string())),
                osc_port: self.osc_port,
                disabled_midi_inputs: self.midi_disabled.clone(),
            },
            interface: project::Interface {
                text_color: self.ui.text_color,
                language: self.ui.language.code().to_string(),
            },
            plugin_paths: self.plugin_paths.clone(),
            rack,
        }
    }

    /// Rebuild everything from a project file: settings first (they decide what
    /// the rack is rebuilt *into*), then the tabs.
    ///
    /// Missing pieces are reported and skipped, never fatal — a project written
    /// on another machine will be missing plugins, and losing one tab beats
    /// refusing to open the file.
    fn apply_project(&mut self, p: project::Project) {
        // ── Configuration ────────────────────────────────────────────────
        // Skipped when the user asked for the rack alone: a project written on
        // another machine carries plugin paths and interface settings that are
        // right *there*, and taking them over is rarely what you want when you
        // only came for the sound.
        if self.load_rack_only {
            self.apply_project_rack(p);
            return;
        }
        self.plugin_paths = p.plugin_paths.clone();
        self.plugin_paths.save();
        self.discover_synths(false);

        self.ui.text_color = p.interface.text_color;
        if let Some(lang) = i18n::Lang::from_code(&p.interface.language) {
            self.ui.language = lang;
        }
        self.ui.apply();
        self.ui.save();

        self.midi_disabled = p.audio.disabled_midi_inputs.clone();
        self.connect_midi();
        if p.audio.osc_port != self.osc_port {
            match p.audio.osc_port {
                Some(port) => self.start_osc(port),
                None => self.stop_osc(),
            }
        }

        self.apply_project_rack(p);
    }

    /// The sound half of a project: tabs, instruments, FX, mixer, routing and
    /// MIDI-learn bindings. Nothing here touches choz's own configuration.
    fn apply_project_rack(&mut self, p: project::Project) {
        // The lanes address slots and FX by index, so they belong to the rack
        // half of a project and travel with it.
        self.automation = p.automation.clone();
        self.automation.recording = false;
        // Drop the live rack first: `rebuild_rack` appends engine slots and
        // assumes it starts from nothing.
        while !self.slots.is_empty() {
            self.remove_slot(self.slots.len() - 1);
        }
        self.cc_bindings.clear();
        self.pc_bindings.clear();

        for (idx, slot) in p.rack.iter().enumerate() {
            let mut rack = RackSlot::new(AudioSource::Midi);
            rack.input = slot.input.as_deref().and_then(parse_input_ref);
            rack.source = self.project_source(&slot.instrument, idx);
            if let AudioSource::Sf2 { path, .. } = &rack.source {
                rack.presets = sources::list_sf2_presets(path).unwrap_or_default();
                rack.preset_cursor = slot
                    .instrument
                    .preset
                    .map(|pr| {
                        rack.presets
                            .iter()
                            .position(|x| x.preset == pr)
                            .unwrap_or(0)
                    })
                    .unwrap_or(0);
            }
            if matches!(rack.source, AudioSource::Sf2 { .. }) {
                rack.instr_params = sources::sf2_params();
                // A project saved before the sends were switchable has no
                // values stored: on is what it sounded like.
                rack.instr_values = slot.instrument.params.clone();
                rack.instr_values.resize(rack.instr_params.len(), 1.0);
            }
            rack.dssi_config = slot.instrument.config.clone();
            if let AudioSource::Plugin { id, .. } = &rack.source {
                if let Some(entry) = self.synths.iter().find(|s| s.id == *id) {
                    rack.instr_params =
                        choz_engine::read_plugin_params(entry.format, &entry.path, &entry.id);
                }
                rack.instr_values = slot.instrument.params.clone();
            }
            // The patch the plugin was on, kept even when the instrument itself
            // could not be resolved on this machine — throwing it away would
            // turn a missing plugin into a lost sound the next time the project
            // is saved. Restored by `rebuild_rack` once an instance exists; a
            // blob that no longer decodes is dropped, not fatal.
            rack.instr_state = project::decode_state(&slot.instrument.state).unwrap_or_default();
            // The bank is a folder on disk; re-reading it is how the patch list
            // comes back. A folder that has since gone leaves the tab with the
            // patch it saved and no list, which is what it would have anyway.
            if let Some(dir) = slot.instrument.bank_dir.clone() {
                rack.plugin_presets = choz_engine::preset_files::list_bank(&dir);
                rack.preset_dir = Some(dir);
            }
            rack.fx_chain = slot
                .fx
                .iter()
                .filter_map(|f| {
                    let mut entry = self.project_fx(f)?;
                    entry.state = project::decode_state(&f.state).unwrap_or_default();
                    Some(entry)
                })
                .collect();
            rack.gain = slot.mixer.gain;
            rack.link = slot.mixer.link.unwrap_or(true);
            rack.gain_r = slot.mixer.gain_r.unwrap_or(slot.mixer.gain);
            rack.pan = slot.mixer.pan;
            rack.mute = slot.mixer.mute;
            rack.solo = slot.mixer.solo;
            rack.out_pair = slot.mixer.out_pair.unwrap_or((0, 1));
            rack.in_pair = resolve_in_pair(&self.in_ports, &slot.mixer);
            rack.pitch_to_midi = slot.mixer.pitch_to_midi;
            rack.pitch_mix = slot.mixer.pitch_mix.unwrap_or(1.0).clamp(0.0, 1.0);
            rack.in_gain = slot.mixer.in_gain.unwrap_or(1.0).clamp(0.0, MAX_IN_GAIN);
            rack.in_gate = slot
                .mixer
                .in_gate
                .unwrap_or(choz_engine::pitch::DEFAULT_GATE);
            rack.channel = slot.channel.clamp(1, 16);
            rack.arp = arp::Arp::new(slot.arp);
            // A patch that is no longer on this machine is said out loud and
            // dropped, like a missing plugin: the tab loads without it rather
            // than the project failing.
            rack.midi_out = slot.midi_out.clone();
            self.slots.push(rack);

            for b in &slot.midi_learn {
                self.cc_bindings.push(CcBinding {
                    source: b.source.as_deref().and_then(parse_input_ref),
                    cc: b.cc,
                    target: b.target,
                });
            }
        }

        // The working copy has to point at a real tab before anything draws.
        self.active_slot = 0;
        self.source = self
            .slots
            .first()
            .map(|s| s.source.clone())
            .unwrap_or(AudioSource::Midi);
        self.fx_chain = self
            .slots
            .first()
            .map(|s| s.fx_chain.clone())
            .unwrap_or_default();
        self.fx_slot = 0;
        self.fx_param = 0;

        self.rebuild_rack();
        eprintln!("choz: loaded {} tab(s)", self.slots.len());
    }

    /// The instrument of a loaded tab. Anything that can't be found any more
    /// leaves the tab empty rather than failing the whole load.
    fn project_source(&self, instr: &project::Instrument, tab: usize) -> AudioSource {
        match instr.kind.as_str() {
            "sf2" => match &instr.path {
                Some(path) => AudioSource::Sf2 {
                    path: path.clone(),
                    bank: instr.bank.unwrap_or(0),
                    preset: instr.preset.unwrap_or(0),
                },
                None => AudioSource::Midi,
            },
            "wav" => match &instr.path {
                Some(path) => AudioSource::AudioFile {
                    path: path.clone(),
                    looping: true,
                },
                None => AudioSource::Midi,
            },
            "plugin" => {
                let id = instr.id.clone().unwrap_or_default();
                match self.synths.iter().find(|s| s.id == id) {
                    Some(entry) => AudioSource::Plugin {
                        id,
                        format: entry.format.label().to_string(),
                        name: entry.name.clone(),
                    },
                    None => {
                        eprintln!("choz: tab {}: plugin {id} is not installed", tab + 1);
                        AudioSource::Midi
                    }
                }
            }
            _ => AudioSource::Midi,
        }
    }

    /// One FX of a loaded chain, built-in or hosted. `None` drops the entry.
    fn project_fx(&self, fx: &project::Fx) -> Option<AudioFxEntry> {
        let mut entry = match (&fx.plugin_path, &fx.plugin_id) {
            (Some(path), Some(id)) => {
                let Some(p) = self
                    .fx_plugins
                    .iter()
                    .find(|p| p.id == *id && p.path == *path)
                    .or_else(|| self.fx_plugins.iter().find(|p| p.id == *id))
                else {
                    eprintln!("choz: FX plugin {id} is not installed");
                    return None;
                };
                let mut plugin = p.clone();
                plugin.params =
                    choz_engine::read_plugin_params(plugin.format, &plugin.path, &plugin.id);
                AudioFxEntry::new_plugin(plugin)
            }
            _ => match source::AudioFxKind::from_id(&fx.kind) {
                Some(kind) => AudioFxEntry::new(kind),
                None => {
                    eprintln!("choz: unknown FX '{}'", fx.kind);
                    return None;
                }
            },
        };
        entry.enabled = fx.enabled;
        entry.wet = fx.wet;
        // Knob count is decided by the plugin/built-in, not the file: an old
        // project with fewer (or more) values keeps the rest at their defaults.
        for (i, v) in fx.params.iter().enumerate() {
            if let Some(slot) = entry.params.get_mut(i) {
                *slot = *v;
            }
        }
        Some(entry)
    }

    /// File \u{2192} Open project: pick the file, then rebuild from it.
    fn open_load_project(&mut self) {
        let start = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let mut modal = Modal::new(
            ModalKind::LoadProject,
            views::modal::ListModal::new("OPEN PROJECT", Vec::new()),
        );
        modal.browser = Some(file_browser::FileBrowser::open(&start, &["yml", "yaml"]));
        modal.list.actions = vec![(i18n::t("RACK ONLY").to_string(), 'k')];
        modal.list.note =
            "  a project also carries plugin paths, colour, language and audio settings"
                .to_string();
        self.modal = Some(modal);
        self.load_rack_only = false;
        self.refresh_modal();
    }

    /// File > Import Max patch: pick the `.maxpat`, then keep what can be kept.
    fn open_import_max(&mut self) {
        let start = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let mut modal = Modal::new(
            ModalKind::ImportMax,
            views::modal::ListModal::new("IMPORT MAX PATCH", Vec::new()),
        );
        modal.browser = Some(file_browser::FileBrowser::open(&start, &["maxpat"]));
        modal.list.note =
            "  Max cannot be run here - what has an equivalent is kept, the rest is named"
                .to_string();
        self.modal = Some(modal);
        self.refresh_modal();
    }

    /// Read a Max patch into the active tab's FX chain, then show what happened.
    ///
    /// **There is no Max runtime**, here or anywhere embeddable, so this is an
    /// import and not a host: the objects with a real equivalent among choz's
    /// own effects become effects, and every object without one is named. A
    /// patch that comes back as two effects and nine names is telling the
    /// truth about itself.
    fn import_maxpat(&mut self, path: &std::path::Path) {
        let import = match choz_engine::maxpat::read_maxpat(path) {
            Ok(import) => import,
            Err(e) => {
                eprintln!("choz: {}: {e}", path.display());
                self.modal = None;
                return;
            }
        };
        let mut lines: Vec<String> = Vec::new();
        let mut added = 0usize;
        let mut over = 0usize;
        for spec in &import.chain {
            let Some(kind) = source::AudioFxKind::from_id(&spec.kind) else {
                continue;
            };
            if self.fx_chain.len() >= source::MAX_FX {
                over += 1;
                continue;
            }
            self.fx_chain.push(AudioFxEntry::new(kind));
            added += 1;
            lines.push(format!("  + {}", kind.label()));
        }
        if added > 0 {
            self.rebuild_fx();
        }
        if over > 0 {
            lines.push(format!(
                "  \u{00B7} {over} more did not fit: a chain holds {}",
                source::MAX_FX
            ));
        }
        for object in &import.dropped {
            lines.push(format!("  \u{2014} no equivalent: {object}"));
        }
        if !import.followed_cords {
            lines.push("  \u{00B7} read in file order: no adc~ or plugin~ to follow".to_string());
        }
        if lines.is_empty() {
            lines.push("  nothing in this patch has an equivalent here".to_string());
        }
        eprintln!("choz: {}", import.summary());

        let mut modal = Modal::new(
            ModalKind::MaxReport,
            views::modal::ListModal::new(format!("IMPORTED {}", import.name), lines),
        );
        modal.list.note = "  Max patches are not run here; this is what could be kept".to_string();
        self.modal = Some(modal);
    }

    fn load_project_from(&mut self, path: &std::path::Path) {
        match project::Project::load(path) {
            Ok(p) => {
                self.apply_project(p);
                // Where "Save project" will write from now on. A directory was
                // a valid thing to open, so store the file it resolved to.
                self.project_file = Some(if path.is_dir() {
                    path.join(project::DEFAULT_NAME)
                } else {
                    path.to_path_buf()
                });
            }
            Err(e) => eprintln!("choz: {e}"),
        }
    }

    /// File \u{2192} Save project: straight back to the file this project came
    /// from. Without one (nothing opened, nothing saved yet) there is nowhere
    /// to write silently, so it is Save as.
    fn save_project(&mut self) {
        match self.project_file.clone() {
            Some(file) => self.save_project_to(&file),
            None => self.open_save_project(),
        }
    }

    /// File \u{2192} Save project as: pick the directory, then the name. Starts
    /// where the current project lives, which is the folder the next one
    /// usually belongs in too.
    fn open_save_project(&mut self) {
        let start = self
            .project_file
            .as_ref()
            .and_then(|f| f.parent())
            .filter(|d| d.is_dir())
            .map(|d| d.to_path_buf())
            .unwrap_or_else(|| {
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
            });
        let mut modal = Modal::new(
            ModalKind::SaveProject,
            views::modal::ListModal::new("SAVE PROJECT", Vec::new()),
        );
        modal.browser = Some(file_browser::FileBrowser::open(
            &start,
            file_browser::DIR_PICK,
        ));
        self.modal = Some(modal);
        self.refresh_modal();
    }

    /// Write the project to `path`, closing the modal when it worked and
    /// leaving the error on screen when it did not.
    fn save_project_to(&mut self, path: &std::path::Path) {
        let project = self.project_snapshot();
        match project.save(path) {
            Ok(file) => {
                eprintln!("choz: project saved to {}", file.display());
                self.project_file = Some(file);
                self.close_modal();
            }
            Err(e) => {
                if let Some(n) = self.save_name.as_mut() {
                    n.confirm = false;
                    n.error = Some(e.to_string());
                }
            }
        }
    }

    /// Keys of the SAVE PROJECT name prompt: typing the file name, and the
    /// separate Enter that agrees to overwrite. Returns true when handled.
    fn save_name_key(&mut self, key: KeyCode) -> bool {
        if self.modal.as_ref().map(|m| m.kind) != Some(ModalKind::SaveProject) {
            return false;
        }
        let Some(mut edit) = self.save_name.take() else {
            return false;
        };

        // An error owns the keys until it is dismissed: Enter goes back to the
        // name (which is what needs changing when a directory is read-only or
        // the name is not writable), Esc drops the save.
        if edit.error.is_some() {
            match key {
                KeyCode::Enter => {
                    edit.error = None;
                    self.save_name = Some(edit);
                }
                KeyCode::Esc => {}
                _ => self.save_name = Some(edit),
            }
            self.refresh_modal();
            return true;
        }

        // The overwrite question is the list itself (row 0 replaces, row 1
        // keeps): only Enter and Esc answer it, everything else — the arrows,
        // the wheel — still moves between the two rows.
        if edit.confirm {
            match key {
                KeyCode::Enter => {
                    let overwrite = self.modal.as_ref().is_some_and(|m| m.list.cursor == 0);
                    let target = edit.target();
                    edit.confirm = overwrite;
                    self.save_name = Some(edit);
                    if overwrite {
                        self.save_project_to(&target);
                    } else {
                        // Back to the name, which is the other way out of a
                        // collision: save it as something else.
                        if let Some(n) = self.save_name.as_mut() {
                            n.confirm = false;
                        }
                    }
                }
                // Esc backs out of the overwrite, not out of the save.
                KeyCode::Esc => {
                    edit.confirm = false;
                    self.save_name = Some(edit);
                }
                _ => {
                    self.save_name = Some(edit);
                    return false;
                }
            }
            self.refresh_modal();
            return true;
        }

        match edit.text.key(key) {
            Some(true) => {
                // An empty name has nothing to write: keep typing.
                if edit.text.buf.trim().is_empty() {
                    self.save_name = Some(edit);
                } else {
                    let target = edit.target();
                    if target.exists() {
                        edit.confirm = true;
                        self.save_name = Some(edit);
                        // Start on "rename": Enter twice in a row must not be
                        // how somebody's set gets replaced.
                        if let Some(m) = self.modal.as_mut() {
                            m.list.cursor = 1;
                        }
                    } else {
                        self.save_name = Some(edit);
                        self.save_project_to(&target);
                    }
                }
            }
            // Esc leaves the name and returns to the directory browser.
            Some(false) => {}
            None => self.save_name = Some(edit),
        }
        self.refresh_modal();
        true
    }

    // ── MIDI learn ────────────────────────────────────────────────────────

    /// Arm pointer learn: the next click on a rack control picks what to bind.
    /// `EnableMouseCapture` already turns on any-motion reporting (mode 1003),
    /// so learn touches no terminal mode of its own — sending `?1003l` on exit
    /// killed mouse reporting outright on terminals that keep one mouse-mode.
    fn start_learn_pick(&mut self) {
        if self.slots.is_empty() {
            return;
        }
        self.learn_pick = true;
        self.learn = None;
    }

    /// Leave learn entirely (bound, or cancelled).
    fn end_learn(&mut self) {
        self.learn_pick = false;
        self.learn = None;
    }

    /// What the INPUTS panel should say about learn, if anything.
    fn learn_banner(&self) -> Option<String> {
        match (self.learn_pick, self.learn) {
            (_, Some(t)) => Some(format!("move a fader \u{2192} {}", self.learn_label(&t))),
            (true, None) if self.editor.is_some() => {
                Some("click a fader, or move a knob in the plugin's window".to_string())
            }
            (true, None) => Some("click a fader or button".to_string()),
            (false, None) => None,
        }
    }

    /// The rack control under `pos`, for pointer learn. Only the controls a CC
    /// can drive are offered — clicking anything else cancels.
    fn learn_target_at(&self, pos: ratatui::layout::Position) -> Option<LearnTarget> {
        let layout = self.layout.borrow();
        let rack = &layout.rack;
        let slot = self.active_slot;
        // The instrument's own knobs are learn targets too — that is the whole
        // reason the box is there: bind a CC to any plugin parameter without
        // opening the plugin's window.
        for &(pi, rect) in rack.instr_knobs.iter() {
            if rect.contains(pos) {
                return Some(LearnTarget::InstrParam { slot, param: pi });
            }
        }
        if rack.gain.is_some_and(|r| r.contains(pos)) {
            return Some(LearnTarget::Gain(slot));
        }
        if rack.pan.is_some_and(|r| r.contains(pos)) {
            return Some(LearnTarget::Pan(slot));
        }
        if rack.in_gain.is_some_and(|r| r.contains(pos)) {
            return Some(LearnTarget::InGain(slot));
        }
        if rack.in_gate.is_some_and(|r| r.contains(pos)) {
            return Some(LearnTarget::InGate(slot));
        }
        // Buttons: everything in SLOT except DEL, the FX CHAIN row, the mixer
        // flags and the bank arrows.
        let trigger = [
            (rack.mute, TriggerAction::Mute),
            (rack.solo, TriggerAction::Solo),
            (rack.on_off, TriggerAction::FxToggle),
            (rack.move_left, TriggerAction::FxMoveLeft),
            (rack.move_right, TriggerAction::FxMoveRight),
            (rack.fx_add, TriggerAction::FxAdd),
        ]
        .into_iter()
        .find(|(r, _)| r.is_some_and(|r| r.contains(pos)))
        .map(|(_, a)| a)
        .or_else(|| {
            rack.buttons
                .iter()
                .find(|(_, r)| r.contains(pos))
                .and_then(|&(b, _)| match b {
                    RackButton::PresetPrev => Some(TriggerAction::PresetPrev),
                    RackButton::PresetNext => Some(TriggerAction::PresetNext),
                    RackButton::InstrPagePrev => Some(TriggerAction::InstrPagePrev),
                    RackButton::InstrPageNext => Some(TriggerAction::InstrPageNext),
                    _ => None,
                })
        })
        .or_else(|| {
            rack.fx_slots
                .iter()
                .find(|(_, r)| r.contains(pos))
                .map(|&(i, _)| TriggerAction::FxSelect(i))
        });
        if let Some(action) = trigger {
            return Some(LearnTarget::Trigger(action));
        }
        rack.params
            .iter()
            .find(|(_, r)| r.contains(pos))
            .map(|&(param, _)| LearnTarget::FxParam {
                slot,
                fx: self.fx_slot,
                param,
            })
    }

    /// Run a button binding. Same code paths the mouse and keys use.
    fn fire_trigger(&mut self, action: TriggerAction) {
        match action {
            TriggerAction::PresetPrev => self.step_preset(-1),
            TriggerAction::PresetNext => self.step_preset(1),
            TriggerAction::InstrPagePrev => self.page_instr(-1),
            TriggerAction::InstrPageNext => self.page_instr(1),
            TriggerAction::Mute => self.with_active_mix(|s| s.mute = !s.mute),
            TriggerAction::Solo => self.with_active_mix(|s| s.solo = !s.solo),
            TriggerAction::ArpToggle => self.edit_arp(ArpEdit::Toggle),
            // The sequencer's, kept only so old projects load.
            TriggerAction::ArpPlayPause | TriggerAction::ArpStop | TriggerAction::ArpRecord => {}

            TriggerAction::ArpTap => self.edit_arp(ArpEdit::Tap),

            TriggerAction::ArpLatch => self.edit_arp(ArpEdit::Latch),
            TriggerAction::FxToggle => {
                if let Some(entry) = self.fx_chain.get_mut(self.fx_slot) {
                    entry.enabled = !entry.enabled;
                    self.rebuild_fx();
                }
            }
            TriggerAction::FxMoveLeft => {
                if self.fx_slot > 0 {
                    self.fx_chain.swap(self.fx_slot, self.fx_slot - 1);
                    self.fx_slot -= 1;
                    self.fx_param = 0;
                    self.rebuild_fx();
                }
            }
            TriggerAction::FxMoveRight => {
                if self.fx_slot + 1 < self.fx_chain.len() {
                    self.fx_chain.swap(self.fx_slot, self.fx_slot + 1);
                    self.fx_slot += 1;
                    self.fx_param = 0;
                    self.rebuild_fx();
                }
            }
            TriggerAction::FxSelect(i) => {
                if i < self.fx_chain.len() {
                    self.fx_slot = i;
                    self.fx_param = 0;
                }
            }
            TriggerAction::FxAdd => self.open_add_fx_modal(),
        }
    }

    /// The [`InputRef`] a message's source names, or `None` for the QWERTY
    /// piano (and for a port that is no longer connected).
    fn source_ref(&self, source: choz_engine::input::InputSource) -> Option<InputRef> {
        use choz_engine::input::InputSource as S;
        match source {
            S::Midi(i) => self.midi_connected.get(i).cloned().map(InputRef::Midi),
            S::Osc => Some(InputRef::Osc),
            S::Keyboard => None,
        }
    }

    /// Every tab bound to `input`, in tab order.
    fn tabs_on_input(&self, input: &InputRef) -> Vec<usize> {
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.input.as_ref() == Some(input))
            .map(|(i, _)| i)
            .collect()
    }

    /// A CC as if it came from the QWERTY piano — no source, so it drives every
    /// binding. Tests that are not about routing use this.
    #[cfg(test)]
    fn feed_cc(&mut self, cc: u8, value: u8) {
        self.apply_cc(choz_engine::input::InputSource::Keyboard, 0, cc, value);
    }

    /// `(cc, target)` for every binding, for the tests that predate sources.
    #[cfg(test)]
    fn cc_pairs(&self) -> Vec<(u8, LearnTarget)> {
        self.cc_bindings.iter().map(|b| (b.cc, b.target)).collect()
    }

    /// A control change arrived: bind it if learn is armed, otherwise drive
    /// every rack control bound to that CC **from that controller**.
    ///
    /// Two keyboards is the normal case on stage, and each one drives its own
    /// tabs: a CC from the KeyStep never moves what the Keystation's fader was
    /// assigned to, whichever tab happens to be on screen. The active tab only
    /// wins where it has to — when several tabs listen to the *same* port, in
    /// which case the port has one owner at a time, exactly as its notes do.
    fn apply_cc(
        &mut self,
        source: choz_engine::input::InputSource,
        channel: u8,
        cc: u8,
        value: u8,
    ) {
        // Bank Select rides along with every program change a controller's
        // buttons send, so it is the first CC to arrive after arming learn —
        // and it would steal the binding from the fader the user is moving.
        // Nothing is ever assigned to it: it carries no position of its own.
        if BANK_SELECT_CCS.contains(&cc) {
            return;
        }
        if let Some(target) = self.learn.take() {
            // FX bindings are scoped to their FX unit: the same fader can drive
            // 1:REVERB Room *and* 2:TUBE Drive, and the selected unit decides
            // which one it moves. Everything else (VOL, PAN, buttons) is one CC
            // one control, so a new binding evicts the old.
            // Two bindings may share a CC when they cannot both be meant: one
            // per **tab** (a shared port has one owner at a time, so the fader
            // moves whichever tab is playing), and one per FX unit within a tab
            // (the same fader drives 1:REVERB Room *and* 2:TUBE Drive). Anything
            // else is one CC, one control, and the new binding evicts the old.
            let coexists = |t: &LearnTarget| match (target_slot(t), target_slot(&target)) {
                (Some(a), Some(b)) if a != b => true,
                _ => match (fx_scope(t), fx_scope(&target)) {
                    (Some(a), Some(b)) => a != b,
                    _ => false,
                },
            };
            // Learned from this controller. A binding evicts an old one only
            // when it is the same CC **from the same place**: the two keyboards
            // both sending CC 1 are two bindings, not one being overwritten.
            let from = self.source_ref(source);
            self.cc_bindings.retain(|b| {
                (b.cc != cc || b.source != from || coexists(&b.target)) && b.target != target
            });
            self.cc_bindings.push(CcBinding {
                source: from,
                cc,
                target,
            });
            eprintln!("choz: CC {cc} -> {}", self.learn_label(&target));
            // Assignment done: back to the normal pointer and event flow.
            self.end_learn();
            return;
        }
        let v = value as f32 / 127.0;
        // A button fires when its CC crosses half-scale upwards, so holding a
        // fader high doesn't retrigger every message.
        let rising = value >= 64 && self.cc_last[cc as usize] < 64;
        self.cc_last[cc as usize] = value;
        let from = self.source_ref(source);
        // Which tab this port is playing right now — the channel first, then the
        // active tab. Only consulted when the port really is shared.
        let owners = from.as_ref().map(|i| self.tabs_on_input(i));
        let playing = self.targets_for(source, channel);
        for binding in self.cc_bindings.clone() {
            if binding.cc != cc {
                continue;
            }
            // A binding with no source is from before they had one (or from the
            // QWERTY piano): it answers anything, as it always did.
            if binding.source.is_some() && binding.source != from {
                continue;
            }
            if let (Some(slot), Some(owners)) = (target_slot(&binding.target), owners.as_ref()) {
                // Shared port: only the tab that owns it right now answers.
                // One tab on the port (or a tab elsewhere) is not a conflict —
                // the binding already names what it moves.
                if owners.len() > 1 && owners.contains(&slot) && !playing.contains(&slot) {
                    continue;
                }
            }
            self.apply_target(binding.target, v, rising);
        }
    }

    /// Move one control, wherever the instruction came from — a CC, an
    /// automation lane, a click. One place, so a control that can be learned can
    /// also be automated without being wired up twice.
    fn apply_target(&mut self, target: LearnTarget, v: f32, rising: bool) {
        match target {
            LearnTarget::Trigger(action) => {
                if rising {
                    self.fire_trigger(action);
                }
            }
            LearnTarget::Gain(slot) => {
                if let Some(s) = self.slots.get_mut(slot) {
                    s.gain = v * MAX_GAIN;
                }
                self.push_mix();
            }
            LearnTarget::Pan(slot) => {
                if let Some(s) = self.slots.get_mut(slot) {
                    s.pan = v * 2.0 - 1.0;
                }
                self.push_mix();
            }
            LearnTarget::InGain(slot) => {
                if slot == self.active_slot {
                    self.set_in_trim(Some(v * MAX_IN_GAIN), None);
                }
            }
            LearnTarget::InGate(slot) => {
                if slot == self.active_slot {
                    self.set_in_trim(None, Some(v));
                }
            }
            LearnTarget::InstrParam { slot, param } => {
                self.set_slot_instr_param(slot, param, v);
            }
            LearnTarget::FxParam { slot, fx, param } => {
                if slot != self.active_slot {
                    // Only the active tab has a live working copy of its chain.
                    return;
                }
                if fx != self.fx_slot {
                    // The selected FX unit owns the fader; the same CC bound
                    // to another unit stays quiet until that unit is picked.
                    return;
                }
                self.set_fx_param(fx, param, v);
            }
        }
    }

    /// The controls an automation lane can address on this tab, and where they
    /// stand right now. The active tab only: a lane belongs to the rack, but the
    /// FX chain of an inactive tab is not live to be read or moved.
    fn automatable(&self) -> Vec<(LearnTarget, f32)> {
        let slot = self.active_slot;
        let mut out = Vec::new();
        if let Some(s) = self.slots.get(slot) {
            out.push((LearnTarget::Gain(slot), (s.gain / MAX_GAIN).clamp(0.0, 1.0)));
            out.push((LearnTarget::Pan(slot), (s.pan + 1.0) / 2.0));
            // Only where there is audio coming in: a lane for a control that
            // is not on screen would play back into nothing.
            if s.in_pair.is_some() {
                out.push((
                    LearnTarget::InGain(slot),
                    (s.in_gain / MAX_IN_GAIN).clamp(0.0, 1.0),
                ));
                out.push((
                    LearnTarget::InGate(slot),
                    views::fx_chain_panel::gate_norm(s.in_gate),
                ));
            }
            for (param, value) in s.instr_values.iter().enumerate() {
                out.push((LearnTarget::InstrParam { slot, param }, *value));
            }
        }
        let fx = self.fx_slot;
        if let Some(entry) = self.fx_chain.get(fx) {
            for (param, value) in entry.params.iter().enumerate() {
                out.push((LearnTarget::FxParam { slot, fx, param }, *value));
            }
        }
        out
    }

    /// One pass of the automation: write down what moved, or move what was
    /// written down. Called once per UI tick, which is faster than a hand.
    ///
    /// Nothing happens while the transport is stopped — a lane is a position in
    /// a loop, and with no clock running there is no position.
    fn tick_automation(&mut self) {
        if !self.playing {
            return;
        }
        let beat = self.automation.position(choz_ports::transport().ppq());
        if self.automation.recording {
            for (target, value) in self.automatable() {
                self.automation.record(target, beat, value);
            }
            return;
        }
        for (target, value) in self.automation.values_at(beat) {
            // Only what actually differs: `apply_target` pushes to the engine,
            // and re-sending an unchanged value every tick would flood the ring
            // the notes travel on.
            let current = self.automatable().into_iter().find(|(t, _)| *t == target);
            if current.is_some_and(|(_, v)| (v - value).abs() < 1e-4) {
                continue;
            }
            self.apply_target(target, value, false);
        }
    }

    // ── Inputs ────────────────────────────────────────────────────────────

    /// Every note input, in list order: MIDI ports first, then OSC.
    fn input_list(&self) -> Vec<InputRef> {
        let mut list: Vec<InputRef> = self
            .midi_ports
            .iter()
            .cloned()
            .map(InputRef::Midi)
            .collect();
        list.push(InputRef::Osc);
        list
    }

    fn input_is_connected(&self, input: &InputRef) -> bool {
        match input {
            InputRef::Midi(name) => !self.midi_disabled.contains(name),
            InputRef::Osc => self.osc_port.is_some(),
        }
    }

    /// The rack tab bound to `input`, if any.
    fn bound_tab(&self, input: &InputRef) -> Option<usize> {
        self.slots
            .iter()
            .position(|s| s.input.as_ref() == Some(input))
    }

    /// Every row of the IN drawer: the note inputs first, then the audio
    /// capture pairs of the running device — the same shape the OUT drawer uses
    /// for playback pairs.
    fn in_targets(&self) -> Vec<(InTarget, views::source_panel::InputRow)> {
        use views::source_panel::InputRow;
        let row = |kind, name: String, connected, bound_tab| InputRow {
            kind,
            name,
            connected,
            bound_tab,
            header: false,
        };
        let header = |name: &str| InputRow {
            kind: "",
            name: name.to_string(),
            connected: false,
            bound_tab: None,
            header: true,
        };
        let mut rows = vec![(InTarget::None, header(i18n::t("NOTE IN")))];
        for (i, input) in self.input_list().iter().enumerate() {
            rows.push((
                InTarget::Note(i),
                row(
                    input.kind(),
                    input.name().to_string(),
                    self.input_is_connected(input),
                    self.bound_tab(input),
                ),
            ));
        }

        // Where the live audio comes from, and there are two shapes of it.
        //
        // On the native JACK client **every capture jack in the system is
        // listed**, grouped under the card that owns it: an eight-input
        // interface, the laptop microphone and the second card are all here at
        // once, because choz wires them all.
        //
        // On ALSA / PulseAudio / PipeWire there is one capture *device*, chosen
        // in Settings, and these are its channels. Empty means no input is
        // open, and the header says where to open one: "nothing here" and
        // "nothing works" look the same otherwise.
        let ports = &self.in_ports;
        let title = if ports.is_empty() {
            format!(
                "{} (0) \u{2014} EDIT > Settings > AUDIO > Input",
                i18n::t("AUDIO IN")
            )
        } else {
            // Two clocks drift, and this is where it shows. Silent while it is
            // behaving — a counter at zero is noise on a panel this narrow —
            // and a number the moment it is not, which is the difference
            // between "my microphone crackles" and something to point at.
            let health = choz_engine::meter::capture_health();
            let (late, dropped) = health.counts();
            let drift = match (late, dropped) {
                (0, 0) => String::new(),
                (l, 0) => format!("  \u{00B7} {l} late"),
                (0, d) => format!("  \u{00B7} {d} dropped"),
                (l, d) => format!("  \u{00B7} {l} late, {d} dropped"),
            };
            // And what the feedback guard is holding down, while it is holding
            // it: a duck nobody can see is indistinguishable from the room
            // having gone quiet by itself, and the player needs to know it was
            // choz that pulled the microphone down.
            let guard = match health.guard_db() {
                db if db < -0.1 => format!("  \u{00B7} GUARD {db:.0} dB"),
                _ => String::new(),
            };
            format!(
                "{} ({}){}{}",
                i18n::t("AUDIO IN"),
                ports.len(),
                drift,
                guard
            )
        };
        rows.push((InTarget::None, header(&title)));
        let active = self.slots.get(self.active_slot).and_then(|s| s.in_pair);
        rows.push((
            InTarget::NoCapture,
            row(
                "AUDIO",
                i18n::t("(instrument)").to_string(),
                active.is_none(),
                None,
            ),
        ));
        let mut card = String::new();
        for (ch, port) in ports.iter().enumerate() {
            let (owner, jack) = port.rsplit_once(':').unwrap_or(("", port.as_str()));
            if owner != card {
                card = owner.to_string();
                rows.push((InTarget::None, header(owner)));
            }
            let tab = self
                .slots
                .iter()
                .position(|s| s.in_pair.is_some_and(|(l, r)| l == ch || r == ch));
            let role = side_label(active, ch);
            // The level on the jack itself, before any tab claims it. This is
            // the reading that ends the guessing: a channel that stays at
            // `--` is a channel nothing is arriving on, whatever the routing
            // says, and no effect downstream can fix that.
            let peak = choz_engine::meter::capture_levels().peak(ch);
            // Gated at -90 dBFS, not at the old -100. A converter with nothing
            // plugged in still converts its own noise floor: the UMC-1820 and
            // the H340 both idle around -98 dB, which the old gate let through
            // and printed as a live-looking number on a dead jack — exactly the
            // confusion the `--` is here to prevent. Nothing anyone plays sits
            // below -90.
            let level = match peak {
                p if p > SILENCE => format!("  {:>4.0}dB", 20.0 * p.log10()),
                _ => "    --  ".to_string(),
            };
            rows.push((
                InTarget::Channel(ch),
                row(
                    "AUDIO",
                    format!("{}  {jack}{level}{role}", ch + 1),
                    !role.is_empty(),
                    tab,
                ),
            ));
        }
        rows
    }

    /// Act on the IN drawer row under the cursor. Channel rows work exactly
    /// like the OUT drawer's: assign, unassign, or toggle. Taking away a tab's
    /// last input channel puts it back on its instrument, which is the same
    /// thing the `(instrument)` row does.
    fn in_select_side(&mut self, row: usize, how: Assign) {
        let Some((target, _)) = self.in_targets().into_iter().nth(row) else {
            return;
        };
        match target {
            InTarget::None => {}
            // A note input is bound, not assigned to a side: the right button
            // has nothing to take off it.
            InTarget::Note(_) if how == Assign::Off => {}
            InTarget::Note(_) => self.bind_selected_input(),
            InTarget::Channel(ch) => {
                // Assigning an audio input starts a tab if the rack is empty,
                // the same way binding a MIDI port does. Without this a
                // guitarist is stuck: there is no note input to bind, so the
                // rack stays empty and the assignment lands on no slot at all.
                if how != Assign::Off && self.ensure_slot().is_none() {
                    return;
                }
                let current = self.slots.get(self.active_slot).and_then(|s| s.in_pair);
                let on = current.is_some_and(|p| channels_of(p).contains(&ch));
                let next = match (how, current, on) {
                    // Nothing captured yet: the first pick is the whole input.
                    (Assign::Off, _, _) if !on => return,
                    (_, None, _) => Some((ch, ch)),
                    (Assign::On, Some(p), _) | (Assign::Toggle, Some(p), false) => {
                        Some(assign_channel(p, ch))
                    }
                    (Assign::Off, Some(p), _) | (Assign::Toggle, Some(p), true) => {
                        unassign_channel(p, ch)
                    }
                };
                self.set_active_capture(next);
            }
            InTarget::NoCapture => self.set_active_capture(None),
        }
    }

    /// Enter (or a left click) on an IN row.
    fn in_select(&mut self, row: usize) {
        self.in_select_side(row, Assign::Toggle);
    }

    /// Re-read the graph's capture ports, so a card plugged in while choz runs
    /// shows up. The JACK client is rebuilt, so the rack is recreated exactly
    /// like an output change.
    fn rescan_capture(&mut self) {
        self.persist_active();
        match self.audio_engine.as_mut().map(|e| e.rescan_inputs()) {
            Some(Ok(true)) => self.rebuild_rack(),
            Some(Ok(false)) => {}
            Some(Err(e)) => eprintln!("choz: {e}"),
            None => {}
        }
        self.refresh_in_ports();
    }

    /// Re-read the capture ports the engine wired up. Cheap, and only worth
    /// doing when the client can have changed — the drawer redraws far more
    /// often than the graph moves.
    fn refresh_in_ports(&mut self) {
        if let Some(engine) = self.audio_engine.as_ref() {
            self.in_ports = engine.input_ports().to_vec();
        }
    }

    /// Feed the active tab from a capture pair (or put it back on its own
    /// instrument). A tab can only have one source of sound, so this is a swap.
    fn set_active_capture(&mut self, pair: Option<(usize, usize)>) {
        let idx = self.active_slot;
        let Some(slot) = self.slots.get_mut(idx) else {
            return;
        };
        slot.in_pair = pair;
        if let Some(ref mut engine) = self.audio_engine {
            engine.set_slot_in(idx, pair);
        }
    }

    /// The note input the cursor sits on, if it's on one at all.
    fn selected_input(&self) -> Option<InputRef> {
        match self.in_targets().get(self.input_cursor)?.0 {
            InTarget::Note(i) => self.input_list().get(i).cloned(),
            _ => None,
        }
    }

    /// Enter on the input list: jump to the tab already bound to this input, or
    /// create a new (instrument-less) tab bound to it.
    fn bind_selected_input(&mut self) {
        let Some(input) = self.selected_input() else {
            return;
        };
        if let Some(tab) = self.bound_tab(&input) {
            self.switch_slot(tab);
            self.focus = Focus::FxChain;
            return;
        }
        let added = match self.audio_engine.as_mut() {
            Some(engine) => engine.add_silent(),
            None => return,
        };
        if added.is_none() {
            eprintln!("choz: rack full");
            return;
        }
        self.push_slot(AudioSource::Midi);
        if let Some(slot) = self.slots.last_mut() {
            slot.input = Some(input);
        }
        self.focus = Focus::FxChain;
    }

    /// `c` on the input list: connect/disconnect that input.
    fn toggle_selected_input(&mut self) {
        let Some(input) = self.selected_input() else {
            return;
        };
        match input {
            InputRef::Midi(name) => {
                match self.midi_disabled.iter().position(|n| *n == name) {
                    Some(i) => {
                        self.midi_disabled.remove(i);
                    }
                    None => self.midi_disabled.push(name),
                }
                self.connect_midi();
            }
            // OSC has a single listener bound at startup; nothing to toggle.
            InputRef::Osc => eprintln!("choz: OSC listener is always on when it could bind"),
        }
    }

    // ── Instruments (loaded into the active tab) ──────────────────────────

    /// Index of the active slot, creating an instrument-less tab first if the
    /// rack is empty (so `choz file.wav` and the File menu still work).
    fn ensure_slot(&mut self) -> Option<usize> {
        if self.slots.is_empty() {
            self.audio_engine.as_mut()?.add_silent()?;
            self.push_slot(AudioSource::Midi);
        }
        Some(self.active_slot)
    }

    /// Load the plugin instrument at `i` into the active tab.
    /// Ask for a plugin to be loaded after the next frame, with its name on
    /// screen while it happens. Everything the user can click goes through
    /// here; a project load calls [`App::load_synth`] directly, because there
    /// is no frame to wait for in the middle of rebuilding a rack.
    fn request_load_synth(&mut self, i: usize) {
        self.loading = self.synths.get(i).map(|s| s.name.clone());
        self.pending_load = Some(PendingLoad::Synth(i));
    }

    fn request_load_source(&mut self, path: std::path::PathBuf) {
        self.loading = Some(
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
        );
        self.pending_load = Some(PendingLoad::Source(path));
    }

    /// Say in the log when the audio thread could not keep up.
    ///
    /// The symptom the user reports is always the same sentence — "the sound
    /// saturates and then disappears, and comes back when I stop playing" — and
    /// from the outside a rack that runs out of CPU, a sandboxed plugin missing
    /// its deadline and an output that is clipping look identical. They are not
    /// the same problem and they have different fixes, so the log says which:
    /// the block budget, the worst block in the last second, how many went over
    /// it, how many blocks a sandboxed child failed to answer, and whether the
    /// mix clipped. Once a second, and only when something actually went wrong.
    fn poll_health(&mut self) {
        if self.health_at.elapsed() < std::time::Duration::from_secs(1) {
            return;
        }
        self.health_at = Instant::now();
        let (peak, blocks, over) = choz_engine::meter::load().take();
        if blocks == 0 {
            return;
        }
        let clips = choz_engine::meter::meter().clipping();
        let (missed, restarts) = self
            .audio_engine
            .as_ref()
            .map(|e| e.sandbox_health())
            .unwrap_or((0, 0));
        let (seen_clips, seen_missed, seen_restarts) = self.health_seen;
        self.health_seen = (clips, missed, restarts);
        let new_missed = missed.saturating_sub(seen_missed);
        let new_clips = clips.saturating_sub(seen_clips);
        let new_restarts = restarts.saturating_sub(seen_restarts);
        if over == 0 && new_missed == 0 && new_clips == 0 && new_restarts == 0 {
            return;
        }
        let budget = choz_engine::meter::load().budget_us();
        let (tab, tab_ms) = choz_engine::meter::load().take_worst_slot();
        // A second where more than a handful of blocks went over is not a
        // hiccup, it is a rack this machine cannot render at this block size —
        // and the fix is one setting, so say it rather than leaving it to be
        // deduced from microseconds.
        if over > blocks / 20 {
            let frames = self
                .audio_engine
                .as_ref()
                .map(|e| e.buffer_size)
                .unwrap_or(0);
            if frames > 0 {
                eprintln!(
                    "choz[{}]: …that is heard as breaking up. A block of {} frames instead of {} \
                     doubles the room (Settings -> AUDIO), and costs {:.1} ms more latency.",
                    std::process::id(),
                    frames * 2,
                    frames,
                    budget as f32 / 1000.0,
                );
            }
        }
        eprintln!(
            "choz[{pid}]: audio short of time — block {:.2} ms, worst {:.2} ms ({:.0}% of it), \
             dearest tab {} ({}) {tab_ms:.2} ms, {over}/{blocks} blocks over budget \
             (device wants {want}/s), {new_missed} sandbox blocks missed, \
             {new_clips} clipped, {new_restarts} plugin restarts",
            budget as f32 / 1000.0,
            peak * budget as f32 / 1000.0,
            peak * 100.0,
            tab + 1,
            self.slots
                .get(tab)
                .map(|s| slot_label(&s.source))
                .unwrap_or_else(|| "?".into()),
            pid = std::process::id(),
            want = 1_000_000u32.checked_div(budget).unwrap_or(0),
        );
    }

    /// Run whatever the last frame promised. Called from the run loop right
    /// after the draw that put "loading" on screen.
    fn run_pending_load(&mut self) {
        match self.pending_load.take() {
            Some(PendingLoad::Synth(i)) => self.load_synth(i),
            Some(PendingLoad::Source(p)) => self.load_source(p),
            None => return,
        }
        self.loading = None;
    }

    fn load_synth(&mut self, i: usize) {
        let Some(entry) = self.synths.get(i).cloned() else {
            return;
        };
        let Some(slot) = self.ensure_slot() else {
            return;
        };
        // A tab that already had DSSI settings keeps them; a new instrument
        // starts with none, which is what every other format has anyway.
        let config = self
            .slots
            .get(slot)
            .map(|s| s.dssi_config.clone())
            .unwrap_or_default();
        // The instrument in this tab is about to be replaced; its window cannot
        // outlive it.
        self.close_editor_for(Some(slot));
        let loaded = match self.audio_engine.as_mut() {
            Some(engine) => {
                Self::load_plugin_into(engine, slot, entry.format, &entry.path, &entry.id, &config)
            }
            None => return,
        };
        match loaded {
            Ok(()) => {
                self.set_active_source(AudioSource::Plugin {
                    id: entry.id.clone(),
                    format: entry.format.label().to_string(),
                    name: entry.name.clone(),
                });
                // Read the plugin's own parameters so the INSTR editor can show
                // them; knobs start where the plugin says its defaults are.
                let params = choz_engine::read_plugin_params(entry.format, &entry.path, &entry.id);
                let values = params
                    .iter()
                    .map(|p| p.normalised(p.default) as f32)
                    .collect();
                // …and its own patch browser, for the BANK key. A plugin whose
                // format cannot report presets simply hands back nothing.
                let presets = self
                    .audio_engine
                    .as_ref()
                    .map(|e| e.slot_presets(slot))
                    .unwrap_or_default();
                if let Some(s) = self.slots.get_mut(slot) {
                    s.instr_params = params;
                    s.instr_values = values;
                    s.plugin_presets = presets;
                    s.preset_cursor = 0;
                    // A new instrument, so the folder the last one used is not
                    // this one's bank.
                    s.preset_dir = None;
                }
                self.adopt_bank(slot);
            }
            Err(e) => eprintln!("choz: {e}"),
        }
    }

    /// Set instrument parameter `index` of the active tab to `value` (0..1) and
    /// push it to the live plugin — no reload, like the FX knobs.
    fn set_instr_param(&mut self, index: usize, value: f32) {
        self.set_slot_instr_param(self.active_slot, index, value);
    }

    /// Same, for a named tab — MIDI learn can drive a plugin on any tab, not
    /// only the one on screen.
    fn set_slot_instr_param(&mut self, slot: usize, index: usize, value: f32) {
        let value = value.clamp(0.0, 1.0);
        match self
            .slots
            .get_mut(slot)
            .and_then(|s| s.instr_values.get_mut(index))
        {
            Some(v) => *v = value,
            None => return,
        }
        if let Some(ref mut engine) = self.audio_engine {
            engine.set_slot_param(slot, index, value);
        }
    }

    /// Point the active tab's working copy (and its stored slot) at `source`.
    fn set_active_source(&mut self, source: AudioSource) {
        self.source = source.clone();
        if let Some(slot) = self.slots.get_mut(self.active_slot) {
            slot.source = source;
            slot.presets.clear();
            slot.plugin_presets.clear();
            slot.preset_cursor = 0;
            slot.instr_params.clear();
            slot.instr_values.clear();
        }
    }

    // ── Note routing ──────────────────────────────────────────────────────

    /// Which rack slots a note from `source` should reach. The QWERTY piano
    /// always plays the active tab; hardware inputs only reach the tabs bound
    /// to them.
    fn note_targets(&self, source: choz_engine::input::InputSource) -> Vec<usize> {
        self.targets_for(source, 0)
    }

    /// Where an event goes, given the rack's mode. `channel` is the MIDI
    /// channel it arrived on (0-based); it only matters in MULTI.
    fn targets_for(&self, source: choz_engine::input::InputSource, channel: u8) -> Vec<usize> {
        if self.ui.rack_mode == settings::RackMode::Multi {
            let channels: Vec<u8> = self.slots.iter().map(|s| s.channel).collect();
            return multi_targets(&channels, self.active_slot, source, channel);
        }
        let bindings: Vec<Option<&InputRef>> =
            self.slots.iter().map(|s| s.input.as_ref()).collect();
        let channels: Vec<u8> = self.slots.iter().map(|s| s.channel).collect();
        note_targets(
            &bindings,
            &channels,
            &self.midi_connected,
            self.active_slot,
            source,
            channel,
        )
    }

    /// Where a note-on should go, remembered so its note-off can follow it.
    fn start_note(
        &mut self,
        source: choz_engine::input::InputSource,
        channel: u8,
        note: u8,
    ) -> Vec<usize> {
        let targets = self.targets_for(source, channel);
        // A retrigger without an off (a controller repeating, a held key)
        // replaces the old entry rather than stacking another.
        self.sounding
            .retain(|(s, n, _)| !(*s == source && *n == note));
        if !targets.is_empty() {
            self.sounding.push((source, note, targets.clone()));
        }
        targets
    }

    /// Where a note-off should go: wherever its note-on went. Falls back to the
    /// current routing for a note choz never saw start (a controller plugged in
    /// mid-note, a stuck key from before a reload).
    fn end_note(
        &mut self,
        source: choz_engine::input::InputSource,
        channel: u8,
        note: u8,
    ) -> Vec<usize> {
        match self
            .sounding
            .iter()
            .position(|(s, n, _)| *s == source && *n == note)
        {
            Some(i) => self.sounding.swap_remove(i).2,
            None => self.targets_for(source, channel),
        }
    }

    /// The active tab's MIDI channel, or `None` in LIVE mode where it means
    /// nothing.
    /// The channel button, when the channel means something.
    ///
    /// Always in MULTI, where a tab *is* a channel. In LIVE only when another
    /// tab shares this one's input: that is the case where the channel picks
    /// between them, and showing it on a lone tab would offer a setting that
    /// changes nothing.
    fn tab_channel(&self) -> Option<u8> {
        let slot = self.slots.get(self.active_slot)?;
        if self.ui.rack_mode == settings::RackMode::Multi {
            return Some(slot.channel);
        }
        let input = slot.input.as_ref()?;
        let shared = self
            .slots
            .iter()
            .enumerate()
            .filter(|(i, s)| *i != self.active_slot && s.input.as_ref() == Some(input))
            .count();
        (shared > 0).then_some(slot.channel)
    }

    /// Step the active tab's MIDI channel, wrapping 16 → ANY → 1.
    fn step_channel(&mut self, delta: i8) {
        // Notes already sounding were routed by the old channel; leaving them
        // would strand their note-offs.
        self.panic();
        let live = self.ui.rack_mode == settings::RackMode::Live;
        if let Some(slot) = self.slots.get_mut(self.active_slot) {
            // MULTI has no "any": a tab there is a channel. LIVE cycles through
            // ANY as the seventeenth position, which is how a split is turned
            // off again.
            slot.channel = if live {
                (slot.channel as i8 + delta).rem_euclid(17) as u8
            } else {
                ((slot.channel.max(1) as i8 - 1 + delta).rem_euclid(16) + 1) as u8
            };
        }
    }

    /// Flip between the live rig and the multi-timbral module.
    ///
    /// Everything sounding is cut first: the two modes route notes to different
    /// tabs, so anything held across the switch would never receive its
    /// note-off — the same trap that left a note ringing when tabs changed.
    fn toggle_rack_mode(&mut self) {
        self.panic();
        self.ui.rack_mode = self.ui.rack_mode.next();
        // MULTI has no "any": a tab there *is* a channel, and one left on ANY
        // would answer nothing at all. Tabs that came from LIVE get numbered on
        // the way in, consecutively, which is the layout a DAW sends.
        if self.ui.rack_mode == settings::RackMode::Multi {
            for (i, slot) in self.slots.iter_mut().enumerate() {
                if slot.channel == ANY_CHANNEL {
                    slot.channel = (i % 16) as u8 + 1;
                }
            }
        }
        self.ui.save();
        eprintln!("choz: rack mode {}", self.ui.rack_mode.label());
    }

    /// Play one note on `slot`: its own instrument, and the MIDI port the tab
    /// is pointed at when it is pointed at one.
    ///
    /// Every note choz plays goes through here — the keys, the arpeggiator, the
    /// panic that stops them. That is the point: a second destination that some
    /// paths know about and others do not is a synth left droning by whichever
    /// path was forgotten.
    fn send_note(&mut self, slot: usize, on: bool, note: u8, vel: u8) {
        self.send_note_at(slot, on, note, vel, 0);
    }

    /// The same funnel, for a sender that knows **when** the note is for.
    ///
    /// `at` is an absolute transport sample; `0` is "now". Only the
    /// arpeggiator's synced clock sends anything else, because it is the only
    /// thing here that knows where its next step lands before it gets there.
    ///
    /// **MIDI OUT is still sent immediately**, and it has to be: ALSA sends
    /// when it is told, so a note scheduled for a sample in the future would
    /// leave the building before it sounded inside. The instrument in the tab
    /// gets the accurate one; an external synth gets it when the interface
    /// noticed, which is what it got before any of this.
    fn send_note_at(&mut self, slot: usize, on: bool, note: u8, vel: u8, at: u64) {
        if let Some(ref mut engine) = self.audio_engine {
            match (on, at) {
                (true, 0) => engine.note_on(slot, note, vel),
                (false, 0) => engine.note_off(slot, note),
                (true, at) => engine.note_on_at(slot, note, vel, at),
                (false, at) => engine.note_off_at(slot, note, at),
            }
        }
        let Some((port, channel)) = self
            .slots
            .get(slot)
            .and_then(|s| s.midi_out.as_ref().map(|p| (p.clone(), s.channel)))
        else {
            return;
        };
        // Channel 0 is the rack's "any", which is not a channel to send on.
        let wire = channel.saturating_sub(1).min(15);
        if let Some(out) = self.midi_out(&port) {
            if on {
                out.note_on(wire, note, vel);
            } else {
                out.note_off(wire, note);
            }
        }
    }

    /// The open connection to `port`, opening it the first time it is asked
    /// for. A port that will not open is remembered as absent by simply not
    /// being in the map — the next note tries again, which is what makes
    /// plugging the synth in mid-set work.
    fn midi_out(&mut self, port: &str) -> Option<&mut midi::MidiOut> {
        if !self.midi_outs.contains_key(port) {
            let out = midi::MidiOut::open(port)?;
            self.midi_outs.insert(port.to_string(), out);
        }
        self.midi_outs.get_mut(port)
    }

    /// Stop everything every open port has sounding. `PANIC`, and losing a
    /// destination: those notes are choz's, and nothing else will end them.
    fn silence_midi_outs(&mut self) {
        for out in self.midi_outs.values_mut() {
            out.all_notes_off(0);
        }
    }

    /// Follow what an outside clock just said.
    ///
    /// `START` is the one that also rewinds: it means "from the top", which is
    /// the difference between it and `CONTINUE`. A tempo reading is written
    /// straight through — the sender is the clock now, and second-guessing it
    /// with a smoother here would put choz a beat behind whatever it is
    /// playing with.
    fn apply_midi_clock(&mut self, msg: midi::ClockMsg) {
        let transport = choz_ports::transport();
        match msg {
            midi::ClockMsg::Tempo(bpm) => transport.set_bpm(bpm),
            midi::ClockMsg::Start => {
                transport.rewind();
                self.playing = true;
            }
            midi::ClockMsg::Continue => self.playing = true,
            midi::ClockMsg::Stop => self.playing = false,
        }
        if !matches!(msg, midi::ClockMsg::Tempo(_)) {
            if let Some(ref engine) = self.audio_engine {
                engine.set_playing(self.playing);
            }
        }
    }

    /// Turn following an outside clock on or off, and remember it.
    fn set_midi_clock(&mut self, on: bool) {
        self.ui.midi_clock = on;
        self.ui.save();
    }

    /// Whether an outside clock is being followed. Read from the settings
    /// rather than copied into a field of its own: one place to be right
    /// about, and it is the place that is saved.
    fn midi_clock(&self) -> bool {
        self.ui.midi_clock
    }

    /// Change the active tab's arpeggiator.
    ///
    /// Turning it **off** has to stop what it was holding: its notes are its
    /// own, and nothing else will ever send their note-offs.
    fn edit_arp(&mut self, edit: ArpEdit) {
        let Some(slot_index) = (self.active_slot < self.slots.len()).then_some(self.active_slot)
        else {
            return;
        };
        let mut stop = Vec::new();
        {
            let slot = &mut self.slots[slot_index];
            let s = &mut slot.arp.settings;
            match edit {
                ArpEdit::Toggle => s.on = !s.on,
                // The knob box writes through here so that everything a change
                // drags with it — stopping what the old play mode was holding,
                // dropping a latched chord — happens exactly once, in the place
                // that already knew how.
                ArpEdit::Knob { param, value } => {
                    let mode_changed = s.set_norm(param, value);
                    let unlatched = param == arp::ArpParam::Latch && !s.latch;
                    if mode_changed || unlatched {
                        slot.arp.reset(&mut stop);
                    }
                }
                // Wraps rather than sticking at the ends: one button, both
                // directions, and the range is small enough to walk.
                ArpEdit::Bpm(d) => {
                    let bpm = s.bpm + d;
                    s.bpm = if bpm > 300.0 {
                        20.0
                    } else if bpm < 20.0 {
                        300.0
                    } else {
                        bpm
                    };
                }
                ArpEdit::Gate => {
                    s.gate = if s.gate >= 0.95 { 0.1 } else { s.gate + 0.1 };
                }
                ArpEdit::Sync => s.sync = !s.sync,
                ArpEdit::Swing => {
                    // Past 75 % the off-beat swallows the on-beat, so it wraps
                    // back to straight instead of going further.
                    s.swing = if s.swing >= 0.74 { 0.0 } else { s.swing + 0.15 };
                }
                ArpEdit::Tap => slot.arp.tap(std::time::Instant::now()),
                // Switching it on memorises whatever is held: that is the
                // gesture, on the hardware and here.
                ArpEdit::Chord => {
                    s.chord = !s.chord;
                    if s.chord {
                        slot.arp.memorise_chord();
                    }
                }
                ArpEdit::Latch => {
                    s.latch = !s.latch;
                    if !s.latch {
                        // Un-latching drops the chord it was holding for you.
                        slot.arp.reset(&mut stop);
                    }
                }
            }
            if !slot.arp.is_on() {
                slot.arp.reset(&mut stop);
            }
        }
        for event in stop {
            if let arp::ArpEvent::Off { note, .. } = event {
                self.send_note(slot_index, false, note, 0);
            }
        }
    }

    /// Advance every tab's arpeggiator and send what it asks for.
    ///
    /// Called from the event loop, which is the clock — see `arp.rs` for why
    /// that is where this lives today, and what it costs.
    fn tick_arps(&mut self) {
        let now = std::time::Instant::now();
        let mut events: Vec<(usize, arp::ArpEvent)> = Vec::new();
        for (i, slot) in self.slots.iter_mut().enumerate() {
            let mut out = Vec::new();
            slot.arp.tick(now, &mut out);
            events.extend(out.into_iter().map(|e| (i, e)));
        }
        for (slot, event) in events {
            match event {
                arp::ArpEvent::On { note, vel, at } => self.send_note_at(slot, true, note, vel, at),
                arp::ArpEvent::Off { note, at } => self.send_note_at(slot, false, note, 0, at),
            }
        }
    }

    /// Whether any tab is arpeggiating, which is what makes the event loop come
    /// back sooner: a step landing within 50 ms is audibly late.
    fn arps_running(&self) -> bool {
        self.slots.iter().any(|s| s.arp.running())
    }

    fn panic(&mut self) {
        self.sounding.clear();
        self.active_notes.clear();
        // A generator that keeps its held keys through a PANIC starts playing
        // again the moment it is ticked, which is not what the button promises.
        for slot in self.slots.iter_mut() {
            let mut out = Vec::new();
            slot.arp.reset(&mut out);
        }
        // The visualizer is showing what the rack believes; PANIC is the
        // moment both stop believing it.
        self.keyboard.clear();
        // Whatever is out on a MIDI port is choz's too: PANIC is the one button
        // that has to reach a synth on the other end of a cable.
        self.silence_midi_outs();
        if let Some(ref mut engine) = self.audio_engine {
            engine.panic();
        }
        eprintln!("choz: panic \u{2014} all notes off");
    }

    /// Push the working copy of the active slot back into `slots`.
    fn persist_active(&mut self) {
        if let Some(slot) = self.slots.get_mut(self.active_slot) {
            slot.source = self.source.clone();
            slot.fx_chain = self.fx_chain.clone();
        }
    }

    /// Make slot `i` active, loading it into the working copy.
    fn switch_slot(&mut self, i: usize) {
        if i >= self.slots.len() {
            return;
        }
        self.persist_active();
        self.active_slot = i;
        self.source = self.slots[i].source.clone();
        self.fx_chain = self.slots[i].fx_chain.clone();
        self.fx_slot = 0;
        self.fx_param = 0;
    }

    /// Append a new slot (mirroring an engine slot just added) and activate it.
    fn push_slot(&mut self, source: AudioSource) {
        self.persist_active();
        let mut slot = RackSlot::new(source.clone());
        // In MULTI a tab is a channel, and consecutive is the layout a DAW's
        // orchestral template sends by default (past 16 they wrap; a rack that
        // big is sharing channels on purpose). In LIVE a new tab answers **any**
        // channel: it is another patch on the same port, not a split, and the
        // user opts into a split by giving it a number.
        slot.channel = match self.ui.rack_mode {
            settings::RackMode::Multi => (self.slots.len() % 16) as u8 + 1,
            settings::RackMode::Live => ANY_CHANNEL,
        };
        self.slots.push(slot);
        self.active_slot = self.slots.len() - 1;
        self.source = source;
        self.fx_chain = Vec::new();
        self.fx_slot = 0;
        self.fx_param = 0;
    }

    /// The `+` on the tab bar: a second configuration for the input the active
    /// tab already listens to. Both tabs stay bound to that port, but only the
    /// active one is fed (see [`note_targets`]), so switching tabs switches the
    /// whole instrument + FX + MIDI-learn setup on the same controller.
    fn add_slot_on_active_input(&mut self) {
        let input = self
            .slots
            .get(self.active_slot)
            .and_then(|s| s.input.clone());
        if let Some(engine) = self.audio_engine.as_mut() {
            if engine.add_silent().is_none() {
                eprintln!("choz: rack full");
                return;
            }
        }
        self.push_slot(AudioSource::Midi);
        if let Some(slot) = self.slots.last_mut() {
            slot.input = input;
        }
    }

    /// Remove the active slot from the rack and the engine.
    fn remove_active_slot(&mut self) {
        self.remove_slot(self.active_slot);
    }

    /// Remove slot `idx` from the rack and the engine, then reload the working
    /// copy from whichever slot ends up active.
    /// Give tab `slot` a bank of patch **files** when its plugin publishes no
    /// programs worth showing.
    ///
    /// Surge XT's VST3 reports zero programs and ships 637 `.fxp` under
    /// `/usr/share/surge-xt/patches_factory`, filed by the same categories its
    /// own window shows. Finding them by name means the BANK button opens on
    /// `Leads · Butter` instead of on a folder picker. A tab that already knows
    /// its folder (a project, or a folder the user picked) keeps it, and
    /// anything the guess cannot find is still one pick away.
    fn adopt_bank(&mut self, slot: usize) {
        let Some(s) = self.slots.get(slot) else {
            return;
        };
        // A browser of its own wins — but only if it says anything. TyrellN6's
        // VST3 publishes 128 slots called `Program 0`, `Program 1`… : a list
        // with no information in it, standing between the player and the 669
        // patches u-he actually installed. A bank that names nothing is not a
        // bank.
        if s.preset_dir.is_none() && s.plugin_presets.iter().any(|p| !is_placeholder(&p.name)) {
            return;
        }
        let (id, name) = match &s.source {
            AudioSource::Plugin { id, name, .. } => (id.clone(), name.clone()),
            _ => return,
        };
        let dir = s
            .preset_dir
            .clone()
            .filter(|d| d.is_dir())
            .or_else(|| {
                let path = self.synths.iter().find(|e| e.id == id).map(|e| e.path.clone())?;
                choz_engine::preset_files::guess_bank_dir(&name, &path)
            });
        let Some(dir) = dir else {
            return;
        };
        let files = choz_engine::preset_files::list_bank(&dir);
        if files.is_empty() {
            return;
        }
        eprintln!("choz: {} patches for {name} in {}", files.len(), dir.display());
        if let Some(s) = self.slots.get_mut(slot) {
            s.preset_cursor = s.preset_cursor.min(files.len() - 1);
            s.plugin_presets = files;
            s.preset_dir = Some(dir);
        }
    }

    /// Shut a plugin's window **before** its plugin is dropped or replaced.
    ///
    /// The editor thread lives inside the plugin: it calls `idle()` on it every
    /// 30 ms, and for a JUCE-based one (Surge XT) that runs the plugin's own
    /// message loop. Dropping the instrument out from under that thread is a
    /// deadlock — the destructor takes locks the editor thread is already
    /// holding — and it is what froze choz when a tab was closed with the
    /// window open: the interface waiting on a thread that was waiting on the
    /// plugin. Dropping [`editor::EditorWindow`] closes the window and joins the
    /// thread first, which is the order that works.
    ///
    /// `slot` of `None` means "whatever is open": removing a tab renumbers
    /// every slot after it, so an editor keyed on the old index is pointing at
    /// somebody else's plugin either way.
    fn close_editor_for(&mut self, slot: Option<usize>) {
        let open = self.editor.as_ref().map(|w| w.key.0);
        if open.is_some_and(|s| slot.is_none_or(|want| s == want)) {
            self.editor = None;
        }
    }

    fn remove_slot(&mut self, idx: usize) {
        if idx >= self.slots.len() {
            return;
        }
        // Its plugin is about to be dropped, and every slot after it is about
        // to be renumbered.
        self.close_editor_for(None);
        // The working copy belongs to the active slot; flush it before the Vec
        // shifts, or edits made since the last tab switch are lost.
        self.persist_active();
        if let Some(ref mut engine) = self.audio_engine {
            engine.remove_slot(idx);
        }
        self.slots.remove(idx);
        if self.slots.is_empty() {
            self.active_slot = 0;
            self.source = AudioSource::Midi;
            self.fx_chain = Vec::new();
        } else {
            // Removing a slot before the active one shifts it down by one.
            let new = if idx < self.active_slot {
                self.active_slot - 1
            } else {
                self.active_slot.min(self.slots.len() - 1)
            };
            self.active_slot = new;
            self.source = self.slots[new].source.clone();
            self.fx_chain = self.slots[new].fx_chain.clone();
        }
        self.fx_slot = 0;
        self.fx_param = 0;
        // Solo/mute are index-based on the engine side; re-push after the shift.
        self.push_mix();
    }

    /// Push every slot's mixer strip to the engine. Solo is resolved here: when
    /// any slot is soloed, every non-soloed slot is muted.
    fn push_mix(&mut self) {
        let any_solo = self.slots.iter().any(|s| s.solo);
        let strips: Vec<(f32, f32, f32, bool)> = self
            .slots
            .iter()
            .map(|s| {
                (
                    s.gain,
                    if s.link { s.gain } else { s.gain_r },
                    s.pan,
                    s.mute || (any_solo && !s.solo),
                )
            })
            .collect();
        if let Some(ref mut engine) = self.audio_engine {
            for (i, (gain, gain_r, pan, mute)) in strips.into_iter().enumerate() {
                engine.set_slot_mix(i, gain, gain_r, pan, mute);
            }
        }
    }

    /// Move one side of a tab's fader, or both when the strip is linked.
    ///
    /// `delta` is in the same step the RACK's `VOL` knob uses, so a click of the
    /// wheel means the same thing wherever the level is touched.
    fn nudge_gain(&mut self, tab: usize, side: MixSide, delta: f32) {
        let Some(s) = self.slots.get(tab) else {
            return;
        };
        let (linked, l, r) = (s.link, s.gain, s.gain_r);
        let clamp = |v: f32| v.clamp(0.0, views::fx_chain_panel::MAX_GAIN);
        self.with_mix(tab, |s| match (linked, side) {
            (true, _) | (_, MixSide::Both) => {
                s.gain = clamp(l + delta);
                s.gain_r = clamp(r + delta);
            }
            (false, MixSide::Left) => s.gain = clamp(l + delta),
            (false, MixSide::Right) => s.gain_r = clamp(r + delta),
        });
    }

    /// Put one side of a tab's fader at `value` (0..1 of the range), or both
    /// when the strip is linked — what a click on the track means.
    fn set_gain_side(&mut self, tab: usize, side: MixSide, value: f32) {
        let linked = self.slots.get(tab).is_some_and(|s| s.link);
        let v = value.clamp(0.0, 1.0) * views::fx_chain_panel::MAX_GAIN;
        self.with_mix(tab, |s| match (linked, side) {
            (true, _) | (_, MixSide::Both) => {
                s.gain = v;
                s.gain_r = v;
            }
            (false, MixSide::Left) => s.gain = v,
            (false, MixSide::Right) => s.gain_r = v,
        });
    }

    /// Tie a strip's two sides together, or let them go. Linking takes the
    /// louder of the two, because the quiet side was the one being trimmed.
    fn toggle_link(&mut self, tab: usize) {
        let Some(s) = self.slots.get(tab) else {
            return;
        };
        let (link, l, r) = (s.link, s.gain, s.gain_r);
        self.with_mix(tab, |s| {
            s.link = !link;
            if s.link {
                let both = l.max(r);
                s.gain = both;
                s.gain_r = both;
            }
        });
    }

    /// Apply the preset under the active slot's cursor (SF2 program change).
    fn apply_selected_preset(&mut self) {
        let idx = self.active_slot;
        let Some(slot) = self.slots.get_mut(idx) else {
            return;
        };
        // A plugin loads its patch through its own browser; there is no bank
        // and no program number to send, and the state blob follows from it.
        if slot.presets.is_empty() {
            let key = slot
                .plugin_presets
                .get(slot.preset_cursor)
                .map(|p| p.key.clone());
            // A bank of files: the key is the path, and the patch inside it is
            // the same blob the plugin's own state call produces.
            if slot.preset_dir.is_some() {
                let Some(key) = key else { return };
                match choz_engine::preset_files::read_state(std::path::Path::new(&key)) {
                    Ok(state) => {
                        // Kept on the tab too, so the patch survives everything
                        // that rebuilds engine slots — and gets saved with the
                        // project like any other.
                        slot.instr_state = state.clone();
                        if let Some(engine) = self.audio_engine.as_ref() {
                            engine.set_slot_state(idx, &state);
                        }
                    }
                    Err(e) => eprintln!("choz: {e:#}"),
                }
                return;
            }
            if let (Some(key), Some(engine)) = (key, self.audio_engine.as_ref()) {
                engine.load_slot_preset(idx, &key);
            }
            return;
        }
        let Some(p) = slot.presets.get(slot.preset_cursor).cloned() else {
            return;
        };
        slot.source = match &slot.source {
            AudioSource::Sf2 { path, .. } => AudioSource::Sf2 {
                path: path.clone(),
                bank: p.bank,
                preset: p.preset,
            },
            other => other.clone(),
        };
        self.source = slot.source.clone();
        if let Some(ref mut engine) = self.audio_engine {
            engine.set_slot_program(idx, p.bank, p.preset);
        }
    }

    /// A controller button that sends program change instead of CC. The program
    /// number is the button's identity: learn binds it to a trigger, a bound
    /// button fires that trigger, and every other program change is ignored —
    /// the controller does not get to pick the preset on its own.
    fn apply_program_button(&mut self, program: u8) {
        // Only buttons (triggers) bind here; a fader target stays armed, so a
        // stray program change can't steal the binding the user is aiming at.
        if let Some(target @ LearnTarget::Trigger(_)) = self.learn {
            self.pc_bindings
                .retain(|(p, t)| *p != program && *t != target);
            self.pc_bindings.push((program, target));
            eprintln!("choz: PC {program} -> {}", self.learn_label(&target));
            self.end_learn();
            return;
        }
        let mut fired = false;
        for (_, target) in self
            .pc_bindings
            .clone()
            .iter()
            .filter(|(p, _)| *p == program)
        {
            if let LearnTarget::Trigger(action) = *target {
                self.fire_trigger(action);
                fired = true;
            }
        }
        // Unbound program changes select a tab, which is what a live rig does
        // with them: PC 0 is tab 1. Only in LIVE — in MULTI the tabs sound
        // together and there is nothing to select. A binding wins, so anyone
        // who mapped a button to something else keeps it.
        if !fired && self.ui.rack_mode == settings::RackMode::Live {
            let tab = program as usize;
            if tab < self.slots.len() && tab != self.active_slot {
                self.panic();
                self.switch_slot(tab);
            }
        }
    }

    /// Move audio output to `name`, rebuilding the rack when the switch had to
    /// tear the stream down (every slot goes with it, so they are recreated here
    /// from the UI's own model and instruments are reloaded from disk). On JACK
    /// the engine just re-patches ports and there is nothing to rebuild.
    fn set_output_device(&mut self, name: &str) {
        self.persist_active();
        match self
            .audio_engine
            .as_mut()
            .map(|e| e.set_output_device(name))
        {
            Some(Ok(true)) => {}
            Some(Ok(false)) => return,
            Some(Err(e)) => {
                eprintln!("choz: {e}");
                return;
            }
            None => return,
        }
        self.rebuild_rack();
    }

    /// Choose the capture device (or turn live input off), then put the rack
    /// back: the stream is rebuilt, and a rebuilt stream has no slots.
    fn set_input_device(&mut self, name: Option<String>) {
        self.persist_active();
        match self.audio_engine.as_mut().map(|e| e.set_input_device(name)) {
            Some(Ok(true)) => {}
            Some(Ok(false)) => {
                self.ui.save();
                return;
            }
            Some(Err(e)) => {
                eprintln!("choz: {e}");
                return;
            }
            None => return,
        }
        self.rebuild_rack();
        self.refresh_in_ports();
        self.ui.save();
    }

    /// Create one engine slot per rack tab and fill it from the UI model:
    /// instrument, FX chain with its knobs, mixer and routing. The engine side
    /// is assumed empty — this runs after something dropped it (an output
    /// device change) or after a project was loaded.
    fn rebuild_rack(&mut self) {
        // Every instrument in the rack is about to be replaced by a fresh one.
        self.close_editor_for(None);
        let slots = self.slots.clone();
        for (i, slot) in slots.iter().enumerate() {
            let Some(ref mut engine) = self.audio_engine else {
                return;
            };
            if engine.add_silent().is_none() {
                break;
            }
            let loaded = match &slot.source {
                AudioSource::Midi => Ok(()),
                AudioSource::Sf2 { path, bank, preset } => engine.load_sf2(i, path, *bank, *preset),
                AudioSource::AudioFile { path, looping } => engine.load_wav(i, path, *looping),
                AudioSource::Plugin { id, .. } => match self.synths.iter().find(|s| s.id == *id) {
                    Some(entry) => {
                        let (fmt, path, id) = (entry.format, entry.path.clone(), entry.id.clone());
                        Self::load_plugin_into(engine, i, fmt, &path, &id, &slot.dssi_config)
                    }
                    None => Err(anyhow::anyhow!("plugin {id} is no longer available")),
                },
            };
            if let Err(e) = loaded {
                eprintln!("choz: reloading tab {}: {e}", i + 1);
            }
            let specs: Vec<FxSpec> = slot.fx_chain.iter().map(|e| e.to_spec()).collect();
            if let Some(ref mut engine) = self.audio_engine {
                engine.set_slot_fx(i, specs);
                // A reloaded plugin is back at its own defaults. The patch
                // goes first: restoring state moves every parameter, so the
                // knob values have to be applied on top of it, not under it.
                if !slot.instr_state.is_empty() {
                    engine.set_slot_state(i, &slot.instr_state);
                }
                for (p, v) in slot.instr_values.iter().enumerate() {
                    engine.set_slot_param(i, p, *v);
                }
                for (ui_fx, entry) in slot.fx_chain.iter().enumerate() {
                    if entry.state.is_empty() || !entry.enabled {
                        continue;
                    }
                    let engine_fx = slot.fx_chain[..ui_fx].iter().filter(|x| x.enabled).count();
                    engine.set_fx_state(i, engine_fx, &entry.state);
                }
            }
        }
        // The preset handles belong to the instances that were just built, so
        // the lists have to come back with them — a project load lands here.
        for i in 0..self.slots.len() {
            let presets = match self.audio_engine.as_ref() {
                Some(engine) => engine.slot_presets(i),
                None => Vec::new(),
            };
            if let Some(slot) = self.slots.get_mut(i) {
                // A bank of files is the tab's own, not the instance's: keep it
                // rather than replacing it with the nothing this plugin
                // publishes.
                if slot.preset_dir.is_none() {
                    slot.plugin_presets = presets;
                }
                let last = slot.plugin_presets.len().saturating_sub(1);
                if !slot.plugin_presets.is_empty() {
                    slot.preset_cursor = slot.preset_cursor.min(last);
                }
            }
            self.adopt_bank(i);
        }
        self.push_mix();
        // Engine slots are new after a reload, so they start on the default
        // pair — put every tab back where the user routed it.
        self.apply_routing();
        // A reload is the one thing that can have rebuilt the JACK client, so
        // it is where the capture ports can have changed under us.
        self.refresh_in_ports();
    }

    /// One-line summary of the active tab: position, instrument and bound input.
    fn active_tab_label(&self) -> String {
        let Some(slot) = self.slots.get(self.active_slot) else {
            return "empty rack".to_string();
        };
        let input = match &slot.input {
            Some(i) => format!(" \u{2190} {}", i.name()),
            None => String::new(),
        };
        format!(
            "{}/{} {}{}",
            self.active_slot + 1,
            self.slots.len(),
            slot_label(&self.source),
            input,
        )
    }

    /// What the BANK key lists for tab `slot`: a SoundFont's programs, or a
    /// plugin's own patches. One instrument per tab, so at most one is filled.
    fn preset_labels(&self, slot: usize) -> Vec<String> {
        let Some(s) = self.slots.get(slot) else {
            return Vec::new();
        };
        if !s.presets.is_empty() {
            return s.presets.iter().map(|p| p.label()).collect();
        }
        s.plugin_presets
            .iter()
            .map(|p| {
                if p.category.is_empty() {
                    p.name.clone()
                } else {
                    format!("{} \u{00B7} {}", p.category, p.name)
                }
            })
            .collect()
    }

    /// The bank a preset belongs to: the first level of its category, which is
    /// what a plugin's own browser calls a bank ("A.Liv", "Factory"). Surge XT
    /// files its 3008 patches under 314 two-level categories, and only the top
    /// level is few enough to be a row of chips.
    fn preset_bank(entry: &choz_engine::PresetEntry) -> &str {
        entry
            .category
            .split_once(" / ")
            .map(|(top, _)| top)
            .unwrap_or(&entry.category)
    }

    /// The banks of tab `slot`, in list order, without repeats. Empty when the
    /// instrument files nothing under a bank — an SF2, or a flat plugin list.
    fn preset_banks(&self, slot: usize) -> Vec<String> {
        let Some(s) = self.slots.get(slot) else {
            return Vec::new();
        };
        let mut banks: Vec<String> = Vec::new();
        for entry in &s.plugin_presets {
            let bank = Self::preset_bank(entry);
            if !bank.is_empty() && !banks.iter().any(|b| b == bank) {
                banks.push(bank.to_string());
            }
        }
        banks
    }

    /// The rows the BANK picker shows: `(index into the full list, label)`,
    /// narrowed to the selected bank chip. The index is what selecting a row
    /// applies, so the filter never has to be undone anywhere else.
    fn preset_rows(&self) -> Vec<(usize, String)> {
        let labels = self.preset_labels(self.active_slot);
        let banks = self.preset_banks(self.active_slot);
        // Chip 0 is "every bank"; the rest are the banks in order.
        let chip = self.modal.as_ref().map(|m| m.list.filter).unwrap_or(0);
        let wanted = chip.checked_sub(1).and_then(|i| banks.get(i));
        let Some(slot) = self.slots.get(self.active_slot) else {
            return Vec::new();
        };
        labels
            .into_iter()
            .enumerate()
            .filter(|(i, _)| match wanted {
                Some(bank) => slot
                    .plugin_presets
                    .get(*i)
                    .is_some_and(|e| Self::preset_bank(e) == bank),
                None => true,
            })
            .collect()
    }

    /// The active tab's current program, as the picker writes it.
    ///
    /// A plugin that reports no programs but can be handed a state blob gets a
    /// standing invitation instead of nothing: its patches are files on disk,
    /// and the bank button is the only way to say where they are. Surge XT's
    /// VST3 build is exactly that case — 637 factory patches, zero programs.
    fn active_preset_label(&self) -> Option<String> {
        let cursor = self.slots.get(self.active_slot)?.preset_cursor;
        self.preset_labels(self.active_slot)
            .into_iter()
            .nth(cursor)
            .or_else(|| self.can_pick_bank().then(|| i18n::t("PICK BANK").to_string()))
    }

    /// Step the active tab's program by `delta` and apply it. This is what the
    /// RACK's `\u{25C0}` / `\u{25B6}` buttons (and their MIDI bindings) do.
    fn step_preset(&mut self, delta: isize) {
        let last = self.preset_labels(self.active_slot).len() as isize - 1;
        if last < 0 {
            return;
        }
        // Inside the bank, not across the whole list: stepping out of "Factory"
        // into somebody's third-party pack is not what the next patch means,
        // and with 3008 of them it is not even findable again.
        let bank: Option<String> = self.slots.get(self.active_slot).and_then(|s| {
            s.plugin_presets
                .get(s.preset_cursor)
                .map(|e| Self::preset_bank(e).to_string())
        });
        if let Some(slot) = self.slots.get_mut(self.active_slot) {
            let mut at = slot.preset_cursor as isize;
            match &bank {
                Some(bank) if !bank.is_empty() => {
                    let step = delta.signum();
                    let mut left = delta.abs();
                    while left > 0 {
                        let next = at + step;
                        if next < 0 || next > last {
                            break;
                        }
                        at = next;
                        if slot
                            .plugin_presets
                            .get(at as usize)
                            .is_some_and(|e| Self::preset_bank(e) == bank)
                        {
                            left -= 1;
                        }
                    }
                    // Landing outside the bank means the edge was reached: stay.
                    if !slot
                        .plugin_presets
                        .get(at as usize)
                        .is_some_and(|e| Self::preset_bank(e) == bank)
                    {
                        at = slot.preset_cursor as isize;
                    }
                }
                _ => at = (at + delta).clamp(0, last),
            }
            slot.preset_cursor = at.clamp(0, last) as usize;
        }
        self.apply_selected_preset();
    }

    /// What the active tab plays, for the RACK's instrument line.
    fn instrument_label(&self) -> String {
        if self.slots.is_empty() {
            return "(no rack tab)".to_string();
        }
        // A tab fed by live audio ignores its instrument, so say so instead of
        // naming a plugin that isn't being heard.
        if let Some((l, r)) = self.slots.get(self.active_slot).and_then(|s| s.in_pair) {
            // One jack is one number: "AUDIO IN 5", not "5/5".
            let jacks = match l == r {
                true => format!("{} {}", i18n::t("AUDIO IN"), l + 1),
                false => format!("{} {}/{}", i18n::t("AUDIO IN"), l + 1, r + 1),
            };
            // `A→M` plays the tab's **instrument**, not its FX chain. A tab with
            // no instrument converts the pitch perfectly and then has nothing to
            // play it on, which from the outside is indistinguishable from a
            // tracker that does not work — so it says so.
            let a2m = self
                .slots
                .get(self.active_slot)
                .is_some_and(|s| s.pitch_to_midi);
            if a2m && matches!(self.source, AudioSource::Midi) {
                return format!("{jacks} \u{2192} A\u{2192}M needs an instrument [1]");
            }
            return jacks;
        }
        match &self.source {
            AudioSource::Midi => "(none)".to_string(),
            other => slot_label(other),
        }
    }

    /// Mutate the active slot's mixer strip, then push it to the engine.
    /// Every tab as the MIXER tab draws it.
    fn mixer_strips(&self) -> Vec<views::midi_monitor::MixerStrip> {
        self.slots
            .iter()
            .enumerate()
            .map(|(i, s)| views::midi_monitor::MixerStrip {
                label: slot_label(&s.source),
                gain: s.gain,
                gain_r: if s.link { s.gain } else { s.gain_r },
                link: s.link,
                pan: s.pan,
                mute: s.mute,
                solo: s.solo,
                active: i == self.active_slot,
                // Only the tab the keyboard is on has a lit side, and only
                // while the MIXER is the panel with the focus.
                side: (i == self.active_slot && self.focus == Focus::Mixer).then_some(
                    match self.mix_side {
                        MixSide::Left => views::midi_monitor::MixerSide::Left,
                        MixSide::Right => views::midi_monitor::MixerSide::Right,
                        MixSide::Both => views::midi_monitor::MixerSide::Both,
                    },
                ),
            })
            .collect()
    }

    /// Same as [`App::with_active_mix`] for a tab that is not the active one —
    /// the MIXER edits all of them, and every strip has to reach the engine.
    fn with_mix(&mut self, i: usize, f: impl FnOnce(&mut RackSlot)) {
        let Some(slot) = self.slots.get_mut(i) else {
            return;
        };
        f(slot);
        self.push_mix();
    }

    fn with_active_mix(&mut self, f: impl FnOnce(&mut RackSlot)) {
        let Some(slot) = self.slots.get_mut(self.active_slot) else {
            return;
        };
        f(slot);
        self.push_mix();
    }

    /// Send one live parameter change to the FX at UI index `fx_idx` of the
    /// active slot. Disabled entries aren't in the engine's chain, so the index
    /// has to be translated.
    /// Move FX `fx`'s parameter `param` to `value`, and everything that
    /// follows from it: the working copy, the dry/wet, a preset's knock-on
    /// values, the live processor, and the rebuild flag.
    ///
    /// One place, because there are three callers — a CC, a click, and the
    /// picker modal — and each of them getting this subtly wrong in its own way
    /// is how a knob ends up moving in the UI and not in the audio.
    fn set_fx_param(&mut self, fx: usize, param: usize, value: f32) {
        let Some(entry) = self.fx_chain.get_mut(fx) else {
            return;
        };
        let Some(p) = entry.params.get_mut(param) else {
            return;
        };
        *p = value;
        let is_mix = entry.is_mix_param(param);
        if is_mix {
            entry.wet = value;
        }
        // A preset knob fills in the knobs below it, and those are what the
        // rebuild and the project read.
        let preset = entry.apply_preset(param);
        let kind = entry.kind;
        let native = entry.plugin.is_none();
        let idx = if is_mix {
            choz_engine::FX_MIX_PARAM
        } else {
            param
        };
        self.set_live_fx_param(fx, idx, value);
        // Only rebuild when the processor cannot take the value live. A rebuild
        // replaces **every** processor in the chain, so it throws away the
        // reverb's tail and the delay's buffer — nudging one knob used to cut
        // the sound of everything else in the slot. The fader also sends ~100
        // CCs a second, and rebuilding per message floods the command ring
        // (note-offs get dropped: notes hang), so when it is needed it is
        // marked and done once per drain.
        if preset || (native && !source::AudioFxEntry::takes_live_params(kind)) {
            self.fx_dirty = true;
        }
    }

    fn set_live_fx_param(&mut self, fx_idx: usize, param: usize, value: f32) {
        let Some(engine_fx) = self.engine_fx_index(fx_idx) else {
            return;
        };
        let slot = self.active_slot;
        if let Some(ref mut engine) = self.audio_engine {
            engine.set_fx_param(slot, engine_fx, param, value);
        }
    }

    /// Position of UI FX `fx_idx` in the engine's chain, which only holds the
    /// enabled entries. `None` when that entry is disabled (nothing to update).
    fn engine_fx_index(&self, fx_idx: usize) -> Option<usize> {
        if !self.fx_chain.get(fx_idx)?.enabled {
            return None;
        }
        Some(self.fx_chain[..fx_idx].iter().filter(|e| e.enabled).count())
    }

    /// Push the active slot's FX chain to its engine slot.
    fn rebuild_fx(&mut self) {
        if self.slots.is_empty() {
            return;
        }
        let idx = self.active_slot;
        if let Some(ref mut engine) = self.audio_engine {
            let specs: Vec<FxSpec> = self.fx_chain.iter().map(|e| e.to_spec()).collect();
            engine.set_slot_fx(idx, specs);
        }
    }

    /// Load `path` into the active rack tab, dispatching on file extension.
    fn load_source(&mut self, path: std::path::PathBuf) {
        let is_sf2 = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("sf2"));
        // A SoundFont dropped on a DSSI tab is not a new instrument: it is what
        // that instrument was missing. FluidSynth-DSSI has no sound at all
        // until it is given one, and `load` is the key it takes.
        if is_sf2 && self.active_is_dssi() {
            self.set_dssi_config("load", &path.to_string_lossy());
            return;
        }
        let Some(slot) = self.ensure_slot() else {
            return;
        };
        self.close_editor_for(Some(slot));
        let loaded = match self.audio_engine.as_mut() {
            Some(engine) if is_sf2 => engine.load_sf2(slot, &path, 0, 0),
            Some(engine) => engine.load_wav(slot, &path, true),
            None => return,
        };
        match loaded {
            Ok(()) if is_sf2 => {
                let presets = sources::list_sf2_presets(&path).unwrap_or_else(|e| {
                    eprintln!("choz: cannot list SF2 presets: {e}");
                    Vec::new()
                });
                self.set_active_source(AudioSource::Sf2 {
                    path,
                    bank: 0,
                    preset: 0,
                });
                if let Some(slot) = self.slots.get_mut(self.active_slot) {
                    slot.presets = presets;
                    // A SoundFont has no plugin parameters, but it does have
                    // oxisynth's own reverb and chorus, and those are switches.
                    slot.instr_params = sources::sf2_params();
                    slot.instr_values = vec![1.0; slot.instr_params.len()];
                }
            }
            Ok(()) => self.set_active_source(AudioSource::AudioFile {
                path,
                looping: true,
            }),
            Err(e) => eprintln!("choz: {e}"),
        }
    }

    /// Trigger a piano note on the active tab's instrument, scheduling an auto
    /// note-off. (Terminals don't deliver reliable key-release, so notes are
    /// fixed-length.)
    fn piano_note_on(&mut self, note: u8) {
        let targets = self.start_note(choz_engine::input::InputSource::Keyboard, 0, note);
        // A tab with its arpeggiator on gets the key the same way it gets a
        // MIDI one: through the arpeggiator. Without this the computer keyboard
        // was the one input that could neither arpeggiate nor type a step, and
        // typing a step is the whole point of being able to pick one.
        let now = std::time::Instant::now();
        let (arped, direct): (Vec<usize>, Vec<usize>) = targets
            .into_iter()
            .partition(|slot| self.slots.get(*slot).is_some_and(|s| s.arp.is_on()));
        for slot in arped {
            if let Some(s) = self.slots.get_mut(slot) {
                s.arp.note_on(note, 100, now);
            }
        }
        for slot in direct {
            self.send_note(slot, true, note, 100);
        }
        const SUSTAIN_TICKS: u8 = 10; // ~500ms at the 50ms poll cadence
        if let Some(slot) = self.active_notes.iter_mut().find(|(n, _)| *n == note) {
            slot.1 = SUSTAIN_TICKS; // retrigger: refresh sustain
        } else {
            self.active_notes.push((note, SUSTAIN_TICKS));
        }
    }

    /// (Re)connect every enabled hardware MIDI input port and refresh the port
    /// lists. Any plugged-in controller then drives the synth (Carla-style).
    /// Dropping the previous connections here is what makes a port toggle off.
    fn connect_midi(&mut self) {
        self._midi_conns.clear();
        let (connected, conns) = midi::connect_inputs(self.note_tx.clone(), &self.midi_disabled);
        self._midi_conns = conns;
        // Show every input port seen, not only the ones that connected.
        let mut inputs = midi::list_input_ports();
        if inputs.is_empty() {
            inputs.clone_from(&connected);
        }
        self.midi_ports = inputs;
        // `InputSource::Midi(i)` indexes this list, so it must stay exactly as
        // `connect_inputs` returned it.
        self.midi_connected = connected;
        self.input_cursor = self
            .input_cursor
            .min(self.in_targets().len().saturating_sub(1));
    }

    /// Reconnect when a controller is plugged in or unplugged while choz runs.
    /// ponytail: polling the port list beats a udev/ALSA-monitor thread — the
    /// scan is an ALSA client open, cheap at 0.5 Hz.
    fn poll_midi_hotplug(&mut self) {
        if self.audio_engine.is_none() || self.midi_scan_at.elapsed() < Duration::from_secs(2) {
            return;
        }
        self.midi_scan_at = Instant::now();
        if midi::list_input_ports() != self.midi_ports {
            self.connect_midi();
        }
    }

    /// Start the OSC listener on `port` (0 = let the OS pick). Failure is
    /// non-fatal — choz just runs without OSC input.
    fn start_osc(&mut self, port: u16) {
        self.stop_osc();
        match choz_engine::osc::listen(port, self.note_tx.clone()) {
            Ok(handle) => {
                eprintln!("choz: OSC listening on udp/{}", handle.port());
                self.osc_port = Some(handle.port());
                self.osc = Some(handle);
            }
            Err(e) => eprintln!("choz: OSC disabled: {e}"),
        }
    }

    fn stop_osc(&mut self) {
        // Dropping the handle stops the thread and frees the port.
        self.osc = None;
        self.osc_port = None;
    }

    /// (Re)start OSC to match the settings — called after the OSC tab changes.
    fn apply_osc_settings(&mut self) {
        if self.ui.osc.enabled {
            self.start_osc(self.ui.osc.bind_port());
        } else {
            self.stop_osc();
            eprintln!("choz: OSC stopped");
        }
    }

    /// Forward any received off-thread input (MIDI, OSC): notes go to the rack
    /// tabs bound to their input (real note-offs, so no auto-off tracking
    /// needed), control messages are applied like the equivalent UI action.
    /// Performance data that rides along with the notes: pedals and wheels.
    /// Resolved to slots before the engine borrow, like the notes themselves.
    fn drain_midi(&mut self) {
        enum Expr {
            Cc(u8, u8),
            Bend(u16),
        }

        let events: Vec<midi::InputEvent> = self.note_rx.try_iter().collect();
        if events.is_empty() {
            return;
        }
        let mut controls = Vec::new();
        let mut ccs = Vec::new();
        // Resolve routing first: note_targets borrows self immutably. Pedals,
        // the modulation wheel and the bend wheel follow the notes they belong
        // to, so they go to the same slots.
        let mut routed = Vec::new();
        let mut expression = Vec::new();
        // Program numbers — the buttons on a controller keyboard. They act on
        // the rack (via learn bindings), not on the slot's preset, so the input
        // routing doesn't matter here.
        let mut programs: Vec<u8> = Vec::new();
        // Only the last one of the drain matters: a tempo reading supersedes
        // the one before it, and START after STOP is where it ended up.
        let mut clock: Option<midi::ClockMsg> = None;
        for event in events {
            // Log first, so the monitor shows everything that arrived, including
            // messages no slot is bound to.
            if self.midi_log.len() == MIDI_LOG_MAX {
                self.midi_log.pop_front();
            }
            self.midi_log.push_back(event);
            match event {
                midi::InputEvent::Note(msg) => {
                    // On and off resolve differently on purpose: see `end_note`.
                    let targets = if msg.on {
                        self.start_note(msg.source, msg.channel, msg.note)
                    } else {
                        self.end_note(msg.source, msg.channel, msg.note)
                    };
                    // The visualizer is fed *after* routing, so a key can be
                    // coloured by the tab that is actually playing it.
                    self.keyboard.feed(&event, targets.first().copied());
                    routed.push((targets, msg));
                }
                midi::InputEvent::Cc(c) => {
                    self.keyboard.feed(&event, None);
                    expression.push((
                        self.targets_for(c.source, c.channel),
                        Expr::Cc(c.cc, c.value),
                    ));
                    ccs.push(c);
                }
                midi::InputEvent::Bend(b) => {
                    self.keyboard.feed(&event, None);
                    expression.push((self.note_targets(b.source), Expr::Bend(b.value)));
                }
                // An outside clock, when the user asked for one. It moves the
                // transport, and the transport is what everything synced reads
                // — the arpeggiator, a tempo delay, a plugin. There is one
                // clock, so this is the only place it is written from.
                midi::InputEvent::Clock(msg) => {
                    if self.midi_clock() {
                        clock = Some(msg);
                    }
                }
                midi::InputEvent::Program(p) => programs.push(p.program),
                midi::InputEvent::Control(c) => controls.push(c),
            }
        }
        if let Some(msg) = clock {
            self.apply_midi_clock(msg);
        }
        // A tab with its arpeggiator on does not get the key: it gets told a
        // key is held, and its own clock decides what sounds. Split here rather
        // than inside the engine loop so a tab without one is byte-for-byte the
        // path it always had.
        let now = std::time::Instant::now();
        let arped: Vec<(usize, midi::NoteMsg)> = routed
            .iter()
            .flat_map(|(targets, msg)| targets.iter().map(move |&slot| (slot, *msg)))
            .filter(|(slot, _)| self.slots.get(*slot).is_some_and(|s| s.arp.is_on()))
            .collect();
        for (slot, msg) in arped {
            let Some(s) = self.slots.get_mut(slot) else {
                continue;
            };
            if msg.on {
                s.arp.note_on(msg.note, msg.vel, now);
            } else {
                s.arp.note_off(msg.note);
            }
        }
        // Every other tab gets the key itself, through the one funnel that also
        // feeds MIDI OUT.
        let direct: Vec<(usize, midi::NoteMsg)> = routed
            .iter()
            .flat_map(|(targets, msg)| targets.iter().map(move |&slot| (slot, *msg)))
            .filter(|(slot, _)| !self.slots.get(*slot).is_some_and(|s| s.arp.is_on()))
            .collect();
        for (slot, msg) in direct {
            self.send_note(slot, msg.on, msg.note, msg.vel);
        }
        // Pedals and wheels go where their notes went — resolved before the
        // engine borrow, like the notes themselves.
        if let Some(ref mut engine) = self.audio_engine {
            for (targets, e) in expression {
                for slot in targets {
                    match e {
                        Expr::Cc(cc, value) => engine.control_change(slot, cc, value),
                        Expr::Bend(value) => engine.pitch_bend(slot, value),
                    }
                }
            }
        }

        for program in programs {
            self.apply_program_button(program);
        }
        // A CC reaching the instrument does not stop it from also driving a
        // MIDI-learn binding: the same sustain pedal can hold notes *and* be
        // assigned to a rack control.
        for c in ccs {
            self.apply_cc(c.source, c.channel, c.cc, c.value);
        }
        for c in controls {
            self.apply_control(c);
        }
        // One rebuild for the whole batch, whatever it took to dirty the chain.
        if self.fx_dirty {
            self.fx_dirty = false;
            self.rebuild_fx();
        }
    }

    /// Apply a remote-control message (OSC). Indices are 1-based on the wire.
    fn apply_control(&mut self, msg: choz_engine::input::ControlMsg) {
        use choz_engine::input::ControlMsg as C;
        let (tab, slot_of) = match msg {
            C::Gain { tab, .. }
            | C::Pan { tab, .. }
            | C::Mute { tab, .. }
            | C::FxParam { tab, .. } => (tab, tab.checked_sub(1)),
        };
        let Some(slot_idx) = slot_of.filter(|i| *i < self.slots.len()) else {
            eprintln!("choz: OSC targets tab {tab}, which doesn't exist");
            return;
        };
        match msg {
            C::Gain { value, .. } => {
                if let Some(s) = self.slots.get_mut(slot_idx) {
                    s.gain = value;
                }
                self.push_mix();
            }
            C::Pan { value, .. } => {
                if let Some(s) = self.slots.get_mut(slot_idx) {
                    s.pan = value;
                }
                self.push_mix();
            }
            C::Mute { on, .. } => {
                if let Some(s) = self.slots.get_mut(slot_idx) {
                    s.mute = on;
                }
                self.push_mix();
            }
            C::FxParam {
                fx, param, value, ..
            } => {
                // Only the active tab has a live working copy of its chain, so
                // remote FX tweaks apply there; other tabs would need their
                // stored chain rebuilt, which a rebuild would do anyway.
                if slot_idx != self.active_slot {
                    eprintln!("choz: OSC FX control only applies to the active tab");
                    return;
                }
                let (Some(fx_idx), Some(p_idx)) = (fx.checked_sub(1), param.checked_sub(1)) else {
                    return;
                };
                let Some(entry) = self.fx_chain.get_mut(fx_idx) else {
                    return;
                };
                let Some(v) = entry.params.get_mut(p_idx) else {
                    return;
                };
                *v = value.clamp(0.0, 1.0);
                let is_mix = entry.is_mix_param(p_idx);
                if is_mix {
                    entry.wet = value;
                }
                entry.apply_preset(p_idx);
                let kind = entry.kind;
                let native = entry.plugin.is_none();
                let param = if is_mix {
                    choz_engine::FX_MIX_PARAM
                } else {
                    p_idx
                };
                self.set_live_fx_param(fx_idx, param, value);
                if native && !source::AudioFxEntry::takes_live_params(kind) {
                    self.fx_dirty = true;
                }
            }
        }
    }

    /// Publish the chord the harmoniser follows, if any harmoniser is asking.
    ///
    /// Read from the keys the monitor already tracks — the notes **held**, not
    /// the events that arrived — and narrowed to the tab on screen, which is
    /// what "the active tab is the reference" means. Nothing is published when
    /// no harmoniser has its MIDI switch on, so the effect stays exactly as it
    /// was for everybody else.
    ///
    /// **Off in MULTI**: there, every tab answers its own channel and a single
    /// process-wide chord would be one keyboard deciding another tab's harmony.
    /// The switch says so on the panel rather than doing something surprising.
    fn publish_chord(&mut self) {
        let channel = match self.ui.rack_mode == settings::RackMode::Multi {
            true => None,
            false => self.harmonizer_midi_channel(),
        };
        let Some(channel) = channel else {
            // Only clear it once, so this is not a store per frame forever.
            if self.chord_published {
                choz_engine::chord::chord().clear();
                self.chord_published = false;
            }
            return;
        };
        let held = self
            .keyboard
            .held_on_channel(channel, Some(self.active_slot));
        choz_engine::chord::chord().set(&held);
        self.chord_published = true;
    }

    /// The channel the active tab's harmoniser is listening to, if it has one
    /// and its switch is on. `Ch` is the last parameter and `MIDI` the one
    /// before it — see the harmoniser's `params`.
    fn harmonizer_midi_channel(&self) -> Option<u8> {
        const MIDI: usize = 9;
        const CHANNEL: usize = 10;
        self.fx_chain
            .iter()
            .filter(|e| e.enabled && e.kind == source::AudioFxKind::Harmonizer)
            .find(|e| e.params.get(MIDI).copied().unwrap_or(0.0) >= 0.5)
            .map(|e| {
                let v = e.params.get(CHANNEL).copied().unwrap_or(0.0);
                1 + (v.clamp(0.0, 1.0) * 15.0).round() as u8
            })
    }

    /// Called once per UI tick: age active notes and send note-off at expiry.
    fn tick_notes(&mut self) {
        if self.active_notes.is_empty() {
            return;
        }
        let mut expired = Vec::new();
        for (note, ticks) in &mut self.active_notes {
            *ticks -= 1;
            if *ticks == 0 {
                expired.push(*note);
            }
        }
        if !expired.is_empty() {
            // Each note goes home to the tab that started it, even if the
            // active tab changed while it was sounding.
            let ends: Vec<(u8, Vec<usize>)> = expired
                .iter()
                .map(|n| {
                    (
                        *n,
                        self.end_note(choz_engine::input::InputSource::Keyboard, 0, *n),
                    )
                })
                .collect();
            for (n, targets) in &ends {
                for slot in targets {
                    if let Some(s) = self.slots.get_mut(*slot) {
                        if s.arp.is_on() {
                            // Its own note-off; the arpeggiator releases what
                            // it started, not what the key did.
                            s.arp.note_off(*n);
                        }
                    }
                }
            }
            // Resolved before the engine borrow: whether a tab runs an
            // algorithm is a question about `self`, and the engine is `self`
            // too.
            let direct: Vec<(u8, usize)> = ends
                .iter()
                .flat_map(|(n, targets)| targets.iter().map(move |slot| (*n, *slot)))
                .filter(|(_, slot)| !self.slots.get(*slot).is_some_and(|s| s.arp.is_on()))
                .collect();
            if let Some(ref mut engine) = self.audio_engine {
                for (n, slot) in direct {
                    engine.note_off(slot, n);
                }
            }
            self.active_notes.retain(|(n, _)| !expired.contains(n));
        }
    }
}

/// Which capture jacks a saved tab meant.
///
/// The pair in the file is an index into a flat list of every capture port in
/// the system, and that list moves the moment an interface is unplugged: every
/// index past it shifts, and a project reopened without the card was listening
/// to somebody else's microphone without saying so. So the **names** win when
/// they are there and still exist.
///
/// When they are there and do *not* exist, the routing is dropped rather than
/// guessed: the tab comes back playing its instrument, which is obvious, while
/// the wrong jack is not. Projects written before the names existed still fall
/// back to the index, because that is all they say.
fn resolve_in_pair(ports: &[String], mixer: &project::Mixer) -> Option<(usize, usize)> {
    let Some((left, right)) = mixer.in_ports.as_ref() else {
        return mixer.in_pair;
    };
    let find = |name: &String| ports.iter().position(|p| p == name);
    match (find(left), find(right)) {
        (Some(l), Some(r)) => Some((l, r)),
        _ => {
            eprintln!(
                "choz: capture jack not here any more, tab loads without audio in: {left} / {right}"
            );
            None
        }
    }
}

/// Command line: `choz [--osc-port N] [file.wav|file.sf2]`.
/// ponytail: two options don't need a parser dependency.
struct Cli {
    osc_port: u16,
    /// Whether `--osc-port` was actually passed (it overrides the settings).
    osc_port_given: bool,
    file: Option<std::path::PathBuf>,
    /// A project file to open at startup.
    project: Option<std::path::PathBuf>,
}

/// What `--version` prints, and what an installer compares to decide whether the
/// copy already on disk is older than the one it is about to put there.
pub fn version_line() -> String {
    format!("choz {}", env!("CARGO_PKG_VERSION"))
}

const USAGE: &str = "\
choz — a terminal audio plugin host

USAGE:
    choz [OPTIONS] [FILE]

ARGS:
    FILE            a .wav or .sf2 to load as the first instrument, or a
                    .yml/.yaml choz project to open

OPTIONS:
    --osc-port N    listen for OSC on port N (overrides the saved setting)
    -V, --version   print the version and exit
    -h, --help      print this help and exit
";

impl Cli {
    fn from_args() -> Self {
        Cli::parse(std::env::args().skip(1)).0
    }

    /// Parse arguments. The second half is what to print before exiting —
    /// `--version` and `--help` answer and stop, which is what a package's
    /// post-install check and an installer both rely on.
    fn parse(args: impl Iterator<Item = String>) -> (Self, Option<String>) {
        let mut cli = Cli {
            osc_port: choz_engine::osc::DEFAULT_PORT,
            osc_port_given: false,
            file: None,
            project: None,
        };
        let mut args = args;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-V" | "--version" => return (cli, Some(version_line())),
                "-h" | "--help" => return (cli, Some(USAGE.to_string())),
                "--osc-port" => match args.next().and_then(|v| v.parse().ok()) {
                    Some(port) => {
                        cli.osc_port = port;
                        cli.osc_port_given = true;
                    }
                    None => eprintln!("choz: --osc-port needs a port number"),
                },
                _ => {
                    let path = std::path::PathBuf::from(&arg);
                    let ext = path
                        .extension()
                        .map(|e| e.to_string_lossy().to_lowercase())
                        .unwrap_or_default();
                    match ext.as_str() {
                        "wav" | "sf2" => cli.file = Some(path),
                        "yml" | "yaml" => cli.project = Some(path),
                        _ => eprintln!(
                            "choz: ignoring '{arg}' (expected a .wav, .sf2 or .yml project)"
                        ),
                    }
                }
            }
        }
        (cli, None)
    }
}

fn main() -> Result<()> {
    // Scan worker: a child of this same binary, probing one plugin directory so
    // a plugin that segfaults doesn't take the app with it. Must come before
    // anything touches the terminal, the log or the audio device — its stdout
    // is the result.
    if choz_engine::worker_main() {
        return Ok(());
    }

    // `--version` and `--help` answer on stdout and stop — before the log
    // redirect below takes fd 1 away, which is where the answer would have
    // silently ended up.
    if let (_, Some(text)) = Cli::parse(std::env::args().skip(1)) {
        println!("{text}");
        return Ok(());
    }

    // Send stderr (all eprintln! + panics) to a log file so it never corrupts
    // the TUI. Tell the user where it is before we grab the terminal.
    if let Some(path) = log::redirect_stderr() {
        println!("choz: logging to {}", path.display());
    }

    enable_raw_mode()?;
    // Draw through a private duplicate of the terminal; fd 1 now belongs to the
    // log, so a chatty plugin can't paint over the UI.
    let mut screen = io::BufWriter::new(log::take_terminal()?);
    execute!(screen, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(screen);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
    app.ui.apply();

    let result = run_app(&mut terminal, &mut app);

    disable_raw_mode()?;
    // The wallpaper is the terminal's, not the buffer's: leaving the alternate
    // screen would not remove it.
    let _ = views::kitty_bg::clear_mask(terminal.backend_mut());
    let _ = views::kitty_bg::clear(terminal.backend_mut());
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

type Screen = io::BufWriter<std::fs::File>;

fn run_app(terminal: &mut Terminal<CrosstermBackend<Screen>>, app: &mut App) -> Result<()> {
    let splash_start = Instant::now();
    let splash_deadline = splash_start + std::time::Duration::from_secs(3);

    while !app.quit {
        // Before drawing: hand the wallpaper to the terminal itself when it can
        // composite one under the text. Cheap after the first time — `sync`
        // re-transmits only when the file, the fit or the size changes.
        let area = terminal
            .size()
            .map(|s| Rect::new(0, 0, s.width, s.height))
            .unwrap_or_default();
        let placed = views::kitty_bg::sync(
            terminal.backend_mut(),
            &app.ui.background,
            area,
            &mut app.kitty_bg,
            &mut app.kitty_cells,
        );
        if !placed {
            app.kitty_bg = None;
        }
        // The wash for those panels, as a translucent image over the picture —
        // painting cells here would hide it. One frame behind the layout, which
        // nobody can see, and skipped entirely when nothing moved.
        if placed {
            let (color, alpha) = app.ui.tint();
            let rects: Vec<_> = app
                .wash_rects
                .iter()
                .map(|(r, s)| (*r, s * alpha))
                .collect();
            views::kitty_bg::sync_mask(
                terminal.backend_mut(),
                area,
                color,
                &rects,
                &mut app.kitty_mask,
            );
        } else if app.kitty_mask.take().is_some() {
            let _ = views::kitty_bg::clear_mask(terminal.backend_mut());
        }
        terminal.draw(|f| ui(f, app))?;

        // Handle splash screen lifecycle
        if !app.splash_done {
            app.splash.tick += 1;
            if !app.splash.ready && Instant::now() >= splash_deadline {
                app.splash.dismiss();
                // Start audio engine after splash is ready
                let audio = app.ui.audio.clone();
                let mut eng = engine::AudioEngine::new(audio.sample_rate, audio.buffer_size);
                // Armed before a single block is rendered: the run-away it
                // exists for can happen on the first one.
                choz_engine::feedback::arm(audio.feedback_guard);
                eng.set_backend_preference(&audio.backend);
                if !audio.device.is_empty() {
                    eng.set_output_device_preference(&audio.device);
                }
                // The capture device the user picked last time. `None` is the
                // default and means no live input: a host that opens the
                // microphone by itself is a host nobody asked for.
                eng.set_input_device_preference(audio.input_device.as_deref());
                // 0 is "system": ask for a period, never force one on the whole
                // graph. Forcing is what made every other application on the
                // machine sound resampled while choz was running.
                eng.set_force_quantum(audio.pipewire_quantum);
                if eng.start().is_ok() {
                    app.audio_engine = Some(eng);
                    app.connect_midi();
                    let cli = Cli::from_args();
                    // An explicit --osc-port wins over the saved settings.
                    if cli.osc_port_given {
                        app.ui.osc.enabled = true;
                        app.ui.osc.port_mode = settings::OscPortMode::Specific;
                        app.ui.osc.udp_port = cli.osc_port;
                    }
                    app.apply_osc_settings();
                    app.discover_synths(false);
                    app.refresh_in_ports();
                    // A project rebuilds the whole rack, so it goes first and a
                    // file argument still lands on the tab that ends up active.
                    if let Some(path) = cli.project {
                        app.load_project_from(&path);
                    }
                    if let Some(path) = cli.file {
                        app.load_source(path);
                    }
                }
            }
            if !is_active(&app.splash) {
                app.splash_done = true;
            }
        }

        // The frame above is the one that says "loading": the work below blocks
        // this thread, so it has to come after a draw and not before one.
        app.run_pending_load();

        handle_events(app)?;
        app.poll_scan();
        app.poll_midi_hotplug();
        app.drain_midi();
        app.tick_arps();
        app.tick_notes();
        app.publish_chord();
        app.poll_editor();
        app.poll_plugin_touch();
        app.poll_health();
        app.tick_automation();
    }
    Ok(())
}

fn handle_events(app: &mut App) -> Result<()> {
    // The event poll is also the arpeggiator's clock. Idle, waking twenty times
    // a second is plenty; with a pattern running, a step has to land closer to
    // where it was asked for than that.
    let wait = if app.arps_running() { 5 } else { 50 };
    if event::poll(std::time::Duration::from_millis(wait))? {
        match event::read()? {
            Event::Key(key)
                if (key.kind == KeyEventKind::Press || key.kind == KeyEventKind::Repeat) =>
            {
                if !app.splash_done {
                    // Any key dismisses splash
                    if is_active(&app.splash) {
                        app.splash.dismiss();
                    }
                    if !is_active(&app.splash) {
                        app.splash_done = true;
                    }
                    return Ok(());
                }
                handle_key(app, key.code);
            }
            Event::Mouse(mouse) if app.splash_done => {
                handle_mouse(app, mouse);
            }
            _ => {}
        }
    }
    Ok(())
}

fn handle_key(app: &mut App, key: KeyCode) {
    if key == KeyCode::Esc && (app.learn_pick || app.learn.is_some()) {
        app.end_learn();
        return;
    }
    if key == KeyCode::Char('q') && app.modal.is_none() {
        app.quit = true;
        return;
    }
    // Panic is global on purpose: a note stuck under a modal, in a drawer or
    // with the plugin's window focused still has to be killable from the TUI.
    if key == KeyCode::Char('P') && app.modal.is_none() {
        app.panic();
        return;
    }
    // The click, from anywhere and with one key: a metronome you have to open
    // a menu to silence is one you play over.
    if key == KeyCode::F(6) && app.modal.is_none() {
        app.toggle_metronome();
        return;
    }
    // The monitor's tabs, from anywhere: the panel has no focus of its own.
    if key == KeyCode::F(5) && app.modal.is_none() {
        app.monitor_tab = app.monitor_tab.next();
        return;
    }
    // Cycle what colours a lit key. Upper-case on purpose: lower-case `c`
    // connects an input in the IN drawer, and the keyboard has no focus to
    // disambiguate it with.
    // The arpeggiator of the active tab. Upper case, like the monitor's colour
    // key: lower-case `a` adds an FX.
    if key == KeyCode::Char('A') && app.modal.is_none() {
        app.edit_arp(ArpEdit::Toggle);
        return;
    }
    if key == KeyCode::Char('C') && app.modal.is_none() && app.monitor_tab.is_keyboard() {
        app.ui.key_colour = app.ui.key_colour.next();
        app.ui.save();
        return;
    }
    // The rack mode switch, from anywhere: it changes what every note does.
    if key == KeyCode::F(4) && app.modal.is_none() {
        app.toggle_rack_mode();
        return;
    }

    // About dialog swallows keys until closed.
    if app.about_open {
        if matches!(key, KeyCode::Esc | KeyCode::Enter | KeyCode::Char(' ')) {
            app.about_open = false;
        }
        return;
    }

    // F10 opens the menu bar.
    if key == KeyCode::F(10) && app.menu.is_none() {
        app.menu = Some(menu::MenuState::open(menu::MenuKind::File));
        return;
    }
    // Menu navigation while open.
    if let Some(mut state) = app.menu {
        match key {
            KeyCode::Esc => {
                app.menu = None;
            }
            KeyCode::Left => {
                state.cycle_menu(false);
                app.menu = Some(state);
            }
            KeyCode::Right => {
                state.cycle_menu(true);
                app.menu = Some(state);
            }
            KeyCode::Up => {
                state.move_up();
                app.menu = Some(state);
            }
            KeyCode::Down => {
                state.move_down();
                app.menu = Some(state);
            }
            KeyCode::Enter => {
                let action = state.current_action();
                app.menu = None;
                apply_menu_action(app, action);
            }
            _ => {}
        }
        return;
    }

    // One key path for every modal (see `ModalKind`).
    if app.modal.is_some() {
        handle_modal_key(app, key);
        return;
    }

    // Drawers: F2 for IN (left), F3 for OUT (right).
    if key == KeyCode::F(2) {
        app.toggle_in_drawer();
        return;
    }
    if key == KeyCode::F(3) {
        app.toggle_out_drawer();
        return;
    }

    if key == KeyCode::Tab {
        app.focus = next_focus(
            app.focus,
            app.in_open,
            app.out_open,
            app.monitor_tab == views::midi_monitor::MonitorTab::Mixer,
        );
        return;
    }

    match app.focus {
        Focus::Source => handle_source_keys(app, key),
        Focus::FxChain => handle_fx_keys(app, key),
        Focus::Transport => handle_transport_keys(app, key),
        Focus::Output => handle_output_keys(app, key),
        Focus::Mixer => handle_mixer_keys(app, key),
    }
}

/// Keys for the MIXER: the strips of every tab, side by side.
///
/// `↑` `↓` move the level in the same step the RACK's `VOL` uses, `←` `→` walk
/// the desk (which pages it), `l` ties a strip's two sides together or lets
/// them go, and `k` picks which side the arrows move on a strip that is not
/// linked. Mute and solo are the same letters as everywhere else.
fn handle_mixer_keys(app: &mut App, key: KeyCode) {
    let tab = app.active_slot;
    match key {
        KeyCode::Up => app.nudge_gain(tab, app.mix_side, GAIN_STEP),
        KeyCode::Down => app.nudge_gain(tab, app.mix_side, -GAIN_STEP),
        // A tenth at a time, for getting there rather than for settling.
        KeyCode::PageUp => app.nudge_gain(tab, app.mix_side, GAIN_STEP * 2.0),
        KeyCode::PageDown => app.nudge_gain(tab, app.mix_side, -GAIN_STEP * 2.0),
        KeyCode::Left if tab > 0 => app.switch_slot(tab - 1),
        KeyCode::Right if tab + 1 < app.slots.len() => app.switch_slot(tab + 1),
        KeyCode::Char('l') => app.toggle_link(tab),
        KeyCode::Char('k') => {
            app.mix_side = match app.mix_side {
                MixSide::Both => MixSide::Left,
                MixSide::Left => MixSide::Right,
                MixSide::Right => MixSide::Both,
            }
        }
        KeyCode::Char(',') => app.with_active_mix(|s| s.pan = (s.pan - 0.05).clamp(-1.0, 1.0)),
        KeyCode::Char('.') => app.with_active_mix(|s| s.pan = (s.pan + 0.05).clamp(-1.0, 1.0)),
        KeyCode::Char('m') => app.with_active_mix(|s| s.mute = !s.mute),
        KeyCode::Char('S') => app.with_active_mix(|s| s.solo = !s.solo),
        _ => {}
    }
}

/// Tab order, skipping whichever drawer is shut — a closed drawer has nothing
/// to focus, so Tab must not park on it.
fn next_focus(focus: Focus, in_open: bool, out_open: bool, mixer: bool) -> Focus {
    let order = [
        Focus::Source,
        Focus::FxChain,
        Focus::Transport,
        Focus::Mixer,
        Focus::Output,
    ];
    let open = |f: Focus| match f {
        Focus::Source => in_open,
        Focus::Output => out_open,
        // Nothing to focus when the bottom panel is showing something else.
        Focus::Mixer => mixer,
        _ => true,
    };
    let at = order.iter().position(|f| *f == focus).unwrap_or(1);
    (1..=order.len())
        .map(|step| order[(at + step) % order.len()])
        .find(|f| open(*f))
        .unwrap_or(Focus::FxChain)
}

/// Keys for whichever modal is open. Navigation is the same everywhere;
/// only Enter (and the value arrows of the instrument editor) differ per kind.
fn handle_modal_key(app: &mut App, key: KeyCode) {
    let Some(kind) = app.modal.as_ref().map(|m| m.kind) else {
        return;
    };
    let cursor = app.modal.as_ref().map(|m| m.list.cursor).unwrap_or(0);
    // Enable/disable, add and remove live in the Plugin Paths section; the
    // Engine and OSC sections have their own value editing.
    if app.paths_modal_key(key) || app.audio_settings_key(key) || app.save_name_key(key) {
        return;
    }
    match key {
        KeyCode::Esc => {
            app.close_modal();
            return;
        }
        KeyCode::Up => {
            if let Some(m) = app.modal.as_mut() {
                if m.list.sidebar_focused {
                    m.list.move_section(-1)
                } else {
                    m.list.move_cursor(-1)
                }
            }
        }
        KeyCode::Down => {
            if let Some(m) = app.modal.as_mut() {
                if m.list.sidebar_focused {
                    m.list.move_section(1)
                } else {
                    m.list.move_cursor(1)
                }
            }
        }
        KeyCode::PageUp => {
            if let Some(m) = app.modal.as_mut() {
                m.list.move_cursor(-10);
            }
        }
        KeyCode::PageDown => {
            if let Some(m) = app.modal.as_mut() {
                m.list.move_cursor(10);
            }
        }
        // In a modal with a sidebar the arrows move between the two panes.
        KeyCode::Left | KeyCode::Right
            if app
                .modal
                .as_ref()
                .is_some_and(|m| !m.list.sidebar.is_empty()) =>
        {
            if let Some(m) = app.modal.as_mut() {
                m.list.sidebar_focused = key == KeyCode::Left;
                if !m.list.sidebar_focused {
                    m.list.cursor = 0;
                    m.list.scroll = 0;
                }
            }
        }
        // Tab cycles the format chips of a sidebar modal.
        KeyCode::Tab
            if app
                .modal
                .as_ref()
                .is_some_and(|m| !m.list.filters.is_empty()) =>
        {
            if let Some(m) = app.modal.as_mut() {
                m.list.cycle_filter(1);
                m.list.sidebar_cursor = 0;
            }
        }
        // The tint slider owns the arrows while the cursor is on it; without
        // this they would flip to the next Settings tab, which is what the rest
        // of the rows want.
        KeyCode::Left | KeyCode::Right
            if kind == ModalKind::PluginPaths
                && app
                    .modal
                    .as_ref()
                    .is_some_and(|m| m.list.filter == TAB_THEME)
                && matches!(
                    app.theme_row(cursor),
                    Some(ThemeRow::Tint) | Some(ThemeRow::PanelColor)
                ) =>
        {
            let up = key == KeyCode::Right;
            if app.theme_row(cursor) == Some(ThemeRow::Tint) {
                app.step_tint(if up {
                    TINT_STEP as i16
                } else {
                    -(TINT_STEP as i16)
                });
            } else {
                app.ui.step_panel_tint(if up { 1 } else { -1 });
                app.ui.save();
                app.apply_ui_settings();
                app.refresh_modal();
            }
        }
        // The metronome's rows are values, not choices: the arrows move them
        // either way, and the menu stays open so the change can be heard.
        KeyCode::Left | KeyCode::Right if kind == ModalKind::Metronome => {
            app.step_metronome_row(cursor, if key == KeyCode::Right { 1 } else { -1 });
            app.refresh_modal();
        }
        // Value arrows in the instrument editor; filter chips everywhere else.
        KeyCode::Left | KeyCode::Right => {
            let delta = if key == KeyCode::Right { 1 } else { -1 };
            if kind == ModalKind::InstrParams {
                let v = app
                    .slots
                    .get(app.active_slot)
                    .and_then(|s| s.instr_values.get(cursor))
                    .copied()
                    .unwrap_or(0.0);
                let shape = app.instr_param_shape(cursor);
                app.set_instr_param(cursor, shape.nudge(v, delta as f32 * INSTR_STEP));
            } else if let Some(m) = app.modal.as_mut() {
                m.list.cycle_filter(delta);
            }
        }
        KeyCode::Enter => {
            // On the sidebar, Enter means "show me this category".
            if app.modal.as_ref().is_some_and(|m| m.list.sidebar_focused) {
                if let Some(m) = app.modal.as_mut() {
                    m.list.sidebar_focused = false;
                    m.list.cursor = 0;
                    m.list.scroll = 0;
                }
                app.refresh_modal();
                return;
            }
            if app.modal_select() {
                app.close_modal();
                app.focus = Focus::FxChain;
            }
            return;
        }
        // Take the rack out of the project and leave the settings alone.
        KeyCode::Char('k') if kind == ModalKind::LoadProject => {
            app.load_rack_only = !app.load_rack_only;
            if let Some(m) = app.modal.as_mut() {
                m.list.note = if app.load_rack_only {
                    "  RACK ONLY: choz keeps its own paths, colour, language and audio".to_string()
                } else {
                    "  a project also carries plugin paths, colour, language and audio settings"
                        .to_string()
                };
            }
            return;
        }
        // Learn the parameter under the cursor: any parameter of any hosted
        // plugin can be driven by a CC, not just the FX chain's.
        KeyCode::Char('l') if kind == ModalKind::InstrParams => {
            app.learn = Some(LearnTarget::InstrParam {
                slot: app.active_slot,
                param: cursor,
            });
            app.close_modal();
            return;
        }
        // Rescan plugins from the SOURCE / ADD FX pickers.
        KeyCode::Char('r') if matches!(kind, ModalKind::Source | ModalKind::AddFx) => {
            app.discover_synths(true);
            if let Some(m) = app.modal.as_mut() {
                m.sources.clear();
            }
            if kind == ModalKind::Source {
                let sources = app.source_choices();
                if let Some(m) = app.modal.as_mut() {
                    m.sources = sources;
                }
            }
        }
        _ => {}
    }
    // The browser's own cursor mirrors the list cursor, and the instrument
    // editor's rows show live values, so rebuild the items after every key.
    if let Some(m) = app.modal.as_mut() {
        if let Some(b) = m.browser.as_mut() {
            b.cursor = m.list.cursor;
        }
    }
    app.refresh_modal();
}

/// Keys for the INPUTS panel: the list of note inputs (MIDI ports + OSC).
/// Presets moved out to the RACK's `[2:BANK/PRESET]` modal.
fn handle_source_keys(app: &mut App, key: KeyCode) {
    match key {
        KeyCode::Up => app.input_cursor = in_step(app, -1),
        KeyCode::Down => app.input_cursor = in_step(app, 1),
        // Bind a note input to a rack tab, or take a capture channel on and off
        // the tab. Enter and Space are the same gesture, which is what makes a
        // stereo pair out of any two jacks: press on each of them.
        KeyCode::Enter | KeyCode::Char(' ') => app.in_select(app.input_cursor),
        KeyCode::Char('c') => app.toggle_selected_input(),
        // Rescan: the note inputs *and* the graph's capture ports, because a
        // card plugged in after start-up is the same kind of surprise.
        KeyCode::Char('r') => {
            app.connect_midi();
            app.rescan_capture();
        }
        KeyCode::Esc => app.toggle_in_drawer(),
        // The QWERTY piano plays the active tab from any panel.
        _ => {
            if let Some(note) = qwerty_note(key) {
                app.piano_note_on(note);
            }
        }
    }
}

/// Keys for the OUT drawer: pick which device the engine plays through.
fn handle_output_keys(app: &mut App, key: KeyCode) {
    match key {
        KeyCode::Up => app.out_cursor = out_step(app, -1),
        KeyCode::Down => app.out_cursor = out_step(app, 1),
        // One channel at a time: Enter (or Space) on 3 and again on 9 is a tab
        // playing out of two jacks that are not a pair.
        KeyCode::Enter | KeyCode::Char(' ') => app.out_select(app.out_cursor),
        KeyCode::Char('r') => app.refresh_out_devices(),
        KeyCode::Esc => app.toggle_out_drawer(),
        // The QWERTY piano plays the active tab from any panel.
        _ => {
            if let Some(note) = qwerty_note(key) {
                app.piano_note_on(note);
            }
        }
    }
}

/// Move a drawer cursor by `step`, stepping over the section headers (which do
/// nothing when selected) and stopping at the ends of the list.
fn row_step<T: PartialEq>(rows: &[T], header: &T, start: usize, step: isize) -> usize {
    let mut i = start as isize;
    loop {
        let next = i + step;
        // Ran off the list with only headers in between: stay put rather than
        // parking the cursor on a header.
        if next < 0 || next as usize >= rows.len() {
            return start;
        }
        i = next;
        if rows[i as usize] != *header {
            return i as usize;
        }
    }
}

fn out_step(app: &App, step: isize) -> usize {
    let rows: Vec<OutTarget> = app.out_targets().into_iter().map(|(t, _)| t).collect();
    row_step(&rows, &OutTarget::None, app.out_cursor, step)
}

fn in_step(app: &App, step: isize) -> usize {
    let rows: Vec<InTarget> = app.in_targets().into_iter().map(|(t, _)| t).collect();
    row_step(&rows, &InTarget::None, app.input_cursor, step)
}

/// Rack slots a note from `source` should reach.
///
/// The QWERTY piano always plays the active tab (it has no port of its own);
/// hardware inputs reach exactly the tabs bound to them, which is what replaced
/// the old omni broadcast.
/// Which tabs a note reaches in **MULTI** mode: every tab whose MIDI channel
/// matches, all sounding at once.
///
/// The channel is what selects here, not the port and not which tab is active —
/// that is the whole difference between driving a rig by hand and being a
/// multi-timbral module for a DAW. A note from OSC or the QWERTY piano has no
/// channel of its own, so it goes to the active tab as it always did.
fn multi_targets(
    channels: &[u8],
    active_slot: usize,
    source: choz_engine::input::InputSource,
    channel: u8,
) -> Vec<usize> {
    use choz_engine::input::InputSource as S;
    match source {
        S::Midi(_) => channels
            .iter()
            .enumerate()
            .filter(|(_, ch)| **ch == channel + 1)
            .map(|(i, _)| i)
            .collect(),
        _ => {
            if active_slot < channels.len() {
                vec![active_slot]
            } else {
                Vec::new()
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn note_targets(
    bindings: &[Option<&InputRef>],
    // Each tab's MIDI channel, 1-based, as MULTI uses it.
    channels: &[u8],
    midi_connected: &[String],
    active_slot: usize,
    source: choz_engine::input::InputSource,
    // The channel the note arrived on, 0-based off the wire.
    channel: u8,
) -> Vec<usize> {
    use choz_engine::input::InputSource as S;
    let input = match source {
        S::Keyboard => {
            return if active_slot < bindings.len() {
                vec![active_slot]
            } else {
                Vec::new()
            };
        }
        S::Osc => InputRef::Osc,
        S::Midi(i) => match midi_connected.get(i) {
            Some(name) => InputRef::Midi(name.clone()),
            None => return Vec::new(),
        },
    };
    let bound: Vec<usize> = bindings
        .iter()
        .enumerate()
        .filter(|(_, b)| **b == Some(&input))
        .map(|(i, _)| i)
        .collect();
    // Several tabs on one port are alternative configurations of it, not a
    // layer: exactly one of them answers a note.
    if bound.len() > 1 {
        // First ask the channel. A controller with a split keyboard, or two
        // sequencer tracks on one cable, sends different channels down the same
        // port — and a tab that says it listens to channel 3 should get channel
        // 3 whether or not it is the one on screen.
        let on_channel: Vec<usize> = bound
            .iter()
            .copied()
            .filter(|i| {
                channels
                    .get(*i)
                    .is_some_and(|c| *c != ANY_CHANNEL && *c - 1 == channel)
            })
            .collect();
        if !on_channel.is_empty() {
            return vec![if on_channel.contains(&active_slot) {
                active_slot
            } else {
                on_channel[0]
            }];
        }
        // Nobody claims it: the active tab answers, and if it is elsewhere the
        // first of the group does, so a port is never played by two at once.
        return vec![if bound.contains(&active_slot) {
            active_slot
        } else {
            bound[0]
        }];
    }
    bound
}

/// `"MIDI:<port>"` / `"OSC"` back into an [`InputRef`]. The one spelling, used
/// by a tab's own input and by a MIDI-learn binding's source.
fn parse_input_ref(text: &str) -> Option<InputRef> {
    match text.strip_prefix("MIDI:") {
        Some(name) => Some(InputRef::Midi(name.to_string())),
        None if text == "OSC" => Some(InputRef::Osc),
        None => None,
    }
}

/// A tab that answers every MIDI channel of its port. The default in LIVE,
/// where a tab is a patch rather than a part.
const ANY_CHANNEL: u8 = 0;

/// Directories scanned for SoundFonts in the SOURCE picker.
fn sf2_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = vec![
        std::path::PathBuf::from("/usr/share/sounds/sf2"),
        std::path::PathBuf::from("/usr/share/soundfonts"),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(std::path::Path::new(&home).join(".local/share/sounds/sf2"));
    }
    dirs.push(std::env::current_dir().unwrap_or_else(|_| ".".into()));
    dirs
}

/// Files directly under `dir` with extension `ext` (no recursion — the SOURCE
/// picker is a shortcut, the file browser is there for everything else).
fn scan_files(dir: &std::path::Path, ext: &str) -> Vec<std::path::PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<std::path::PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext)))
        .collect();
    out.sort();
    out
}

/// Short label for a rack tab, e.g. "SF2:piano" or "CLAP:Surge".
/// A preset name that names nothing: empty, a bare number, or `Program 12` —
/// the numbered slots a format hands out when the plugin filled none of them in.
fn is_placeholder(name: &str) -> bool {
    let n = name.trim();
    if n.is_empty() {
        return true;
    }
    let rest = n
        .strip_prefix("Program")
        .or_else(|| n.strip_prefix("program"))
        .or_else(|| n.strip_prefix("Preset"))
        .or_else(|| n.strip_prefix("preset"))
        .unwrap_or(n)
        .trim();
    rest.is_empty() || rest.chars().all(|c| c.is_ascii_digit())
}

fn slot_label(source: &AudioSource) -> String {
    match source {
        AudioSource::Midi => "(empty)".to_string(),
        AudioSource::Sf2 { path, .. } => format!("SF2:{}", file_stem(path)),
        AudioSource::AudioFile { path, .. } => format!("WAV:{}", file_stem(path)),
        // The format is whatever loaded it — hardcoding CLAP here labelled
        // every LV2/VST/DSSI/SFZ tab wrong once those formats landed.
        AudioSource::Plugin { name, format, .. } => format!("{format}:{name}"),
    }
}

/// Tab text for a rack slot: mute/solo marker, source label, and the close
/// button. Used for both drawing and click hit-testing, so the two can't drift.
fn tab_label(slot: &RackSlot) -> String {
    let mark = match (slot.mute, slot.solo) {
        (_, true) => "\u{25C9}", // soloed
        (true, _) => "\u{2298}", // muted
        _ => "",
    };
    format!("{mark}{} \u{2715}", slot_label(&slot.source))
}

fn file_stem(path: &std::path::Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().chars().take(10).collect())
        .unwrap_or_else(|| "?".to_string())
}

/// Map a QWERTY key to a MIDI note (one octave from C4=60), tracker-style.
fn qwerty_note(key: KeyCode) -> Option<u8> {
    let c = match key {
        KeyCode::Char(c) => c.to_ascii_lowercase(),
        _ => return None,
    };
    let n = match c {
        'a' => 60,
        'w' => 61,
        's' => 62,
        'e' => 63,
        'd' => 64,
        'f' => 65,
        't' => 66,
        'g' => 67,
        'y' => 68,
        'h' => 69,
        'u' => 70,
        'j' => 71,
        'k' => 72,
        _ => return None,
    };
    Some(n)
}

fn handle_fx_keys(app: &mut App, key: KeyCode) {
    match key {
        // Instrument, presets and MIDI learn for the active tab.
        KeyCode::Char('1') | KeyCode::Char('i') => app.open_source_modal(),
        KeyCode::Char('2') | KeyCode::Char('b') => app.open_preset_modal(),
        // `3` arms the pointer (click the fader to bind); `l` picks the target
        // from a list, for anyone driving choz from the keyboard only.
        KeyCode::Char('3') => app.start_learn_pick(),
        KeyCode::Char('l') => app.open_learn_modal(),
        // Rack tab navigation + slot removal.
        KeyCode::Char('[') => {
            if app.active_slot > 0 {
                app.switch_slot(app.active_slot - 1);
            }
        }
        KeyCode::Char(']') => {
            app.switch_slot(app.active_slot + 1);
        }
        KeyCode::Backspace => {
            app.remove_active_slot();
        }
        // `k` swaps between the instrument's knobs and the FX chain's; the
        // arrows and w/s then drive whichever box has the cursor.
        KeyCode::Char('k') => app.toggle_rack_focus(),
        // The instrument's pager, from the keyboard: the same two arrows the
        // panel draws, and the same remapping of whatever CCs are on the box.
        // Page up/down rather than `<`/`>` — those are the input trim, and a
        // guitarist reaching for the gain must not page the synth instead.
        KeyCode::PageUp => app.page_instr(-1),
        KeyCode::PageDown => app.page_instr(1),
        KeyCode::Left if app.rack_focus == RackFocus::Instrument => app.step_instr_cursor(-1),
        KeyCode::Right if app.rack_focus == RackFocus::Instrument => app.step_instr_cursor(1),
        KeyCode::Up if app.rack_focus == RackFocus::Instrument => {
            let cols = app.instr_cols() as isize;
            app.step_instr_cursor(-cols);
        }
        KeyCode::Down if app.rack_focus == RackFocus::Instrument => {
            let cols = app.instr_cols() as isize;
            app.step_instr_cursor(cols);
        }
        // The arpeggiator's box takes the arrows the same way the instrument's
        // does: sideways along the row, up and down a whole row at a time.
        KeyCode::Left if app.rack_focus == RackFocus::Arp => app.step_arp_cursor(-1),
        KeyCode::Right if app.rack_focus == RackFocus::Arp => app.step_arp_cursor(1),
        KeyCode::Up if app.rack_focus == RackFocus::Arp => {
            let cols = app.arp_cols() as isize;
            app.step_arp_cursor(-cols);
        }
        KeyCode::Down if app.rack_focus == RackFocus::Arp => {
            let cols = app.arp_cols() as isize;
            app.step_arp_cursor(cols);
        }
        KeyCode::Left => {
            app.fx_slot = app.fx_slot.saturating_sub(1);
            app.fx_param = 0;
        }
        KeyCode::Right if app.fx_slot + 1 < app.fx_chain.len() => {
            app.fx_slot += 1;
            app.fx_param = 0;
        }
        KeyCode::Up => {
            app.fx_param = app.fx_param.saturating_sub(1);
        }
        KeyCode::Down => {
            if let Some(entry) = app.fx_chain.get(app.fx_slot) {
                let max = entry.param_descs().len();
                if app.fx_param + 1 < max {
                    app.fx_param += 1;
                }
            }
        }
        // Parameters of the tab's own instrument: a plugin's list, or the
        // SoundFont's reverb / chorus switches.
        KeyCode::Char('p') => app.open_instr_modal(),
        // Factory presets of the selected effect.
        KeyCode::Char('P') => {
            app.open_fx_presets();
        }
        // A named parameter — a preset, a key, a scale, a mode — is a list, so
        // Enter opens the list. Stepping through eighteen Winamp presets with
        // an arrow key is a knob pretending to be a menu.
        KeyCode::Enter if app.rack_focus == RackFocus::Arp => {
            // A knob with names is a list; one with two places flips — **both
            // ways**, so Enter turns the arpeggiator off as readily as on.
            if !app.open_arp_choice(app.arp_param) {
                app.press_arp_knob(app.arp_param);
            }
        }
        KeyCode::Enter if app.rack_focus != RackFocus::Instrument => {
            app.open_fx_choice(app.fx_param);
        }
        // The plugin's own window, for plugins that have one.
        KeyCode::Char('4') | KeyCode::Char('g') => app.toggle_editor(None),
        KeyCode::Char('G') => app.toggle_editor(Some(app.fx_slot)),
        KeyCode::Char('x') => app.toggle_sandbox(None),
        KeyCode::Char('X') => app.toggle_sandbox(Some(app.fx_slot)),
        // `w`/`s` move whichever box has the arrows — the arpeggiator's knobs
        // included, or its box would be the only one that can be walked but not
        // turned from the keyboard.
        KeyCode::Char('w') | KeyCode::Char('W') if app.rack_focus == RackFocus::Arp => {
            app.nudge_arp_knob(app.arp_param, 0.05)
        }
        KeyCode::Char('s') if app.rack_focus == RackFocus::Arp => {
            app.nudge_arp_knob(app.arp_param, -0.05)
        }
        KeyCode::Char('w') | KeyCode::Char('W') => adjust_fx_param(app, 0.05),
        KeyCode::Char('s') => adjust_fx_param(app, -0.05),
        // Mixer strip of the active slot.
        KeyCode::Char('-') => adjust_gain(app, -0.05),
        KeyCode::Char('+') | KeyCode::Char('=') => adjust_gain(app, 0.05),
        KeyCode::Char(',') => adjust_pan(app, -0.1),
        KeyCode::Char('.') => adjust_pan(app, 0.1),
        // The input strip, on the keys next to the volume ones: a guitar's
        // trim, and how hard it has to be hit before `A→M` calls it a note.
        KeyCode::Char('<') => app.adjust_in_trim(-0.05, 0.0),
        KeyCode::Char('>') => app.adjust_in_trim(0.05, 0.0),
        KeyCode::Char(';') => app.adjust_in_trim(0.0, -0.05),
        KeyCode::Char(':') => app.adjust_in_trim(0.0, 0.05),
        KeyCode::Char('m') => app.with_active_mix(|s| s.mute = !s.mute),
        KeyCode::Char('S') => app.with_active_mix(|s| s.solo = !s.solo),
        KeyCode::Char(' ') => {
            if let Some(entry) = app.fx_chain.get_mut(app.fx_slot) {
                entry.enabled = !entry.enabled;
                app.rebuild_fx();
            }
        }
        KeyCode::Char('a') => app.open_add_fx_modal(),
        KeyCode::Char('d') if !app.fx_chain.is_empty() => {
            app.fx_chain.remove(app.fx_slot);
            if app.fx_slot >= app.fx_chain.len() && app.fx_slot > 0 {
                app.fx_slot -= 1;
            }
            app.rebuild_fx();
        }
        _ => {}
    }
}

/// Max linear slot gain (+6 dB).
const MAX_GAIN: f32 = 2.0;

/// Ceiling of the **input** trim, linear. +24 dB.
///
/// Not the same number as [`MAX_GAIN`], and that was the bug: a slot's output
/// gain wants a little headroom over unity, while an input is coming off
/// whatever the preamp happened to be set to. At the old +6 dB a dynamic
/// microphone had to be sung into from two centimetres before `A→M` heard a
/// note at all — the trim ran out long before the signal was loud enough to
/// measure a period from.
const MAX_IN_GAIN: f32 = 16.0;

/// How much AutoTune history the strip under the knobs shows.
const AUTOTUNE_TRACE: usize = 240;

/// The RACK's `VOL`: the tab's level, both sides at once whatever the link
/// says — an unlinked strip keeps the trim between its sides while the whole
/// tab moves.
fn adjust_gain(app: &mut App, delta: f32) {
    app.nudge_gain(app.active_slot, MixSide::Both, delta);
}

fn adjust_pan(app: &mut App, delta: f32) {
    app.with_active_mix(|s| s.pan = (s.pan + delta).clamp(-1.0, 1.0));
}

fn adjust_fx_param(app: &mut App, delta: f32) {
    // The knob keys follow the cursor: with the instrument's box focused they
    // move an instrument parameter instead of an FX one.
    if app.rack_focus == RackFocus::Instrument {
        let param = app.instr_param;
        let v = app
            .slots
            .get(app.active_slot)
            .and_then(|s| s.instr_values.get(param))
            .copied()
            .unwrap_or(0.0);
        let shape = app.instr_param_shape(param);
        app.set_instr_param(param, shape.nudge(v, delta));
        return;
    }
    let (fx_idx, param) = (app.fx_slot, app.fx_param);
    let Some(entry) = app.fx_chain.get_mut(fx_idx) else {
        return;
    };
    // A switch or a named step moves one position per press; a knob moves by
    // `delta`.
    let shape = entry
        .param_descs()
        .get(param)
        .map(|d| d.shape.clone())
        .unwrap_or_default();
    let Some(v) = entry.params.get_mut(param) else {
        return;
    };
    *v = shape.nudge(*v, delta);
    let value = *v;
    let (is_plugin, is_mix) = (entry.plugin.is_some(), entry.is_mix_param(param));
    if is_mix {
        entry.wet = value;
    }
    let kind = entry.kind;
    let preset = entry.apply_preset(param);

    // **Never rebuild for a knob that can be moved live.** A rebuild replaces
    // every processor in the chain, and a replaced processor has no buffer: a
    // delay loses its echoes, a space echo its tape, a granular cloud its
    // grains. Nudging one knob used to cut the sound of the whole slot, which
    // is what "the slider cuts the audio" was. A hosted plugin must never be
    // rebuilt either — that re-instantiates it.
    app.set_live_fx_param(
        fx_idx,
        if is_mix {
            choz_engine::FX_MIX_PARAM
        } else {
            param
        },
        value,
    );
    // What is left: a preset (which moves the knobs below it) and the handful
    // of built-ins that are configured at construction and have no live path.
    // Marked rather than done, so dragging a knob across the panel is one
    // rebuild at the end of the drain and not one per step.
    if preset || (!is_plugin && !source::AudioFxEntry::takes_live_params(kind)) {
        app.fx_dirty = true;
    }
}

fn handle_transport_keys(app: &mut App, key: KeyCode) {
    match key {
        KeyCode::Char(' ') => toggle_play(app),
        KeyCode::Char('s') => stop_play(app),
        KeyCode::Char('p') => app.panic(),
        // The automation loop's length, in bars.
        KeyCode::Left => app.nudge_automation_loop(-1),
        KeyCode::Right => app.nudge_automation_loop(1),
        // Arming is a toggle, and it starts the transport if it is stopped:
        // recording into a clock that is not running records one instant.
        KeyCode::Char('r') | KeyCode::Char('R') => {
            app.automation.recording = !app.automation.recording;
            if app.automation.recording && !app.playing {
                toggle_play(app);
            }
        }
        KeyCode::Char('x') | KeyCode::Char('X') => {
            app.automation.clear(None);
            app.automation.recording = false;
        }
        KeyCode::Char('o') => app.toggle_out_drawer(),
        _ => {}
    }
}

fn toggle_play(app: &mut App) {
    app.playing = !app.playing;
    if let Some(ref engine) = app.audio_engine {
        engine.set_playing(app.playing);
    }
}

fn stop_play(app: &mut App) {
    app.playing = false;
    if let Some(ref engine) = app.audio_engine {
        engine.set_playing(false);
    }
}

// ─── Mouse Handling ────────────────────────────────────────────────────────────

/// One edit to the active tab's arpeggiator. Clicking a button cycles it; the
/// same actions are what a key binding would drive.
#[derive(Debug, Clone, Copy, PartialEq)]
enum ArpEdit {
    Toggle,
    /// One knob of the box, as a 0..1 position.
    Knob {
        param: arp::ArpParam,
        value: f32,
    },
    Bpm(f32),
    Gate,
    Swing,
    Latch,
    /// Follow the transport instead of the arpeggiator's own tempo.
    Sync,
    /// Set the tempo by tapping it.
    Tap,
    /// One key plays the memorised chord.
    Chord,
}

enum MouseAction {
    None,
    ArpEdit(ArpEdit),
    /// Arm the pointer to choose what this tab's notes drive.
    FocusPanel(Focus),
    FxSlot(usize),
    FxParam(usize),
    FxParamAdjust(usize, f32),
    /// A knob in the instrument's own box: select it (and move the cursor
    /// there), or turn it with the wheel.
    InstrParamSel(usize),
    InstrParamAdjust(usize, f32),
    ArpParamSel(usize),
    ArpParamAdjust(usize, f32),
    /// Open the list of positions of one arpeggiator control.
    ArpPick(arp::ArpParam),
    /// Follow an outside MIDI clock, or go back to choz's own.
    ToggleMidiClock,
    FxAdd,
    FxToggle,
    FxDelete,
    FxMoveLeft,
    FxMoveRight,
    TransportPlay,
    TransportStop,
    /// Kill every sounding note, everywhere.
    Panic,
    /// Turn the active tab's audio input into notes for its instrument.
    TogglePitchToMidi,
    /// Step the `A→M` dry/wet through its quarters, or nudge it with the wheel.
    PitchMixCycle,
    PitchMixAdjust(f32),
    /// Show one of the monitor's tabs.
    MonitorTab(views::midi_monitor::MonitorTab),
    ToggleInDrawer,
    ToggleOutDrawer,
    OutputDevice(usize),
    InputBind(usize),
    /// Right click on a drawer row: take that channel off the active tab.
    OutputUnassign(usize),
    InputUnassign(usize),
    InputToggle(usize),
    OpenSourcePicker,
    ScanInputs,
    PresetStep(isize),
    /// Page the instrument's knob box, taking the CCs learned on it along.
    InstrPage(isize),
    /// Step the active tab's MIDI channel (MULTI mode).
    ChannelStep(i8),
    OpenPresetPicker,
    OpenLearnPicker,
    ToggleEditor,
    ToggleFxEditor,
    OpenFxPresets,
    /// Move a drawer's cursor, which is also what scrolls its list.
    InputStep(isize),
    OutputStep(isize),
    ToggleSandbox,
    ToggleFxSandbox,
    RackTab(usize),
    RackTabClose(usize),
    RackTabAdd,
    MixGain(f32),
    MixPan(f32),
    /// `(input trim, A→M sensitivity)`, each a delta on that knob's travel.
    InTrim(f32, f32),
    /// Lengthen or shorten the automation loop, in bars.
    AutomationLoop(i32),
    MixMute,
    MixSolo,
    /// A MIXER strip: set that tab's level or pan from where the click landed
    /// on the track, or flip one of its two flags.
    MixerSet(usize, views::midi_monitor::MixerHit, f32),
    MixerToggle(usize, views::midi_monitor::MixerHit),
    MixerPage(isize),
    /// The wheel over a strip: move that side by a step.
    MixerNudge(usize, MixSide, f32),
    MixerPan(usize, f32),
}

fn mouse_action(col: u16, row: u16, layout: &UiLayout, kind: MouseEventKind) -> MouseAction {
    let pos: ratatui::layout::Position = (col, row).into();

    match kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if layout.play_btn_rect.contains(pos) {
                return MouseAction::TransportPlay;
            }
            if layout.stop_btn_rect.contains(pos) {
                return MouseAction::TransportStop;
            }

            if layout.in_close_rect.is_some_and(|r| r.contains(pos)) {
                return MouseAction::ToggleInDrawer;
            }
            if layout.out_close_rect.is_some_and(|r| r.contains(pos)) {
                return MouseAction::ToggleOutDrawer;
            }

            if layout.source_area.contains(pos) {
                // Shut, the whole strip is the handle: clicking it opens IN.
                if layout.input_item_rects.is_empty() && layout.input_scan_rect.is_none() {
                    return MouseAction::ToggleInDrawer;
                }
                if layout.input_scan_rect.is_some_and(|r| r.contains(pos)) {
                    return MouseAction::ScanInputs;
                }
                // The connect mark sits inside the input row, so test it first.
                for &(ii, rect) in layout.input_mark_rects.iter() {
                    if rect.contains(pos) {
                        return MouseAction::InputToggle(ii);
                    }
                }
                for &(ii, rect) in layout.input_item_rects.iter() {
                    if rect.contains(pos) {
                        return MouseAction::InputBind(ii);
                    }
                }
                return MouseAction::FocusPanel(Focus::Source);
            }

            for &(tab, rect) in layout.monitor_tabs.iter() {
                if rect.contains(pos) {
                    return MouseAction::MonitorTab(tab);
                }
            }
            for &(hit, rect) in layout.mixer_hits.iter() {
                if !rect.contains(pos) {
                    continue;
                }
                use views::midi_monitor::MixerHit;
                return match hit {
                    // Where in the control it was clicked *is* the value: a
                    // fader you can only nudge is a fader that takes ten clicks
                    // to cross. The level is a **vertical** fader — top is
                    // loud — and the pan reads across.
                    MixerHit::Gain(i) | MixerHit::GainR(i) => {
                        let h = rect.height.max(1) as f32;
                        let at = (rect.y + rect.height - 1 - pos.y) as f32 / (h - 1.0).max(1.0);
                        MouseAction::MixerSet(i, hit, at.clamp(0.0, 1.0))
                    }
                    MixerHit::Link(i) => MouseAction::MixerToggle(i, hit),
                    MixerHit::Pan(i) => {
                        let w = rect.width.max(1) as f32;
                        let at = (pos.x - rect.x) as f32 / w;
                        MouseAction::MixerSet(i, hit, at.clamp(0.0, 1.0))
                    }
                    MixerHit::Mute(i) | MixerHit::Solo(i) | MixerHit::Select(i) => {
                        MouseAction::MixerToggle(i, hit)
                    }
                    // The pager walks the active tab, which is what the window
                    // follows — there is no separate scroll to move.
                    MixerHit::Page(d) => MouseAction::MixerPage(d),
                };
            }

            if layout.fx_chain_area.contains(pos) {
                let rack = &layout.rack;
                for &(btn, rect) in rack.buttons.iter() {
                    if rect.contains(pos) {
                        return match btn {
                            RackButton::Channel => MouseAction::ChannelStep(1),
                            RackButton::Source => MouseAction::OpenSourcePicker,
                            RackButton::Preset => MouseAction::OpenPresetPicker,
                            RackButton::Learn => MouseAction::OpenLearnPicker,
                            RackButton::PitchToMidi => MouseAction::TogglePitchToMidi,
                            RackButton::PitchMix => MouseAction::PitchMixCycle,
                            RackButton::Gui => MouseAction::ToggleEditor,
                            RackButton::Sandbox => MouseAction::ToggleSandbox,
                            RackButton::PresetPrev => MouseAction::PresetStep(-1),
                            RackButton::PresetNext => MouseAction::PresetStep(1),
                            RackButton::InstrPagePrev => MouseAction::InstrPage(-1),
                            RackButton::InstrPageNext => MouseAction::InstrPage(1),
                            RackButton::ArpOn => MouseAction::ArpEdit(ArpEdit::Toggle),
                            // The ones with names open their list instead of
                            // walking it: on a panel too short for the knob
                            // box these buttons are the only way in, and
                            // clicking eight times to reach RANDOM is not one.
                            RackButton::ArpMode => MouseAction::ArpPick(arp::ArpParam::Mode),
                            RackButton::ArpDiv => MouseAction::ArpPick(arp::ArpParam::Div),
                            RackButton::ArpRateDown => MouseAction::ArpEdit(ArpEdit::Bpm(-5.0)),
                            RackButton::ArpRateUp => MouseAction::ArpEdit(ArpEdit::Bpm(5.0)),
                            RackButton::ArpGate => MouseAction::ArpEdit(ArpEdit::Gate),
                            RackButton::ArpOctaves => MouseAction::ArpPick(arp::ArpParam::Octaves),
                            RackButton::ArpLatch => MouseAction::ArpEdit(ArpEdit::Latch),
                            RackButton::ArpSwing => MouseAction::ArpEdit(ArpEdit::Swing),
                            RackButton::ArpSync => MouseAction::ArpEdit(ArpEdit::Sync),
                            RackButton::ArpChord => MouseAction::ArpEdit(ArpEdit::Chord),
                            RackButton::ArpTap => MouseAction::ArpEdit(ArpEdit::Tap),
                        };
                    }
                }
                // Close button first — it sits inside the tab rect.
                for &(si, rect) in rack.tab_close.iter() {
                    if rect.contains(pos) {
                        return MouseAction::RackTabClose(si);
                    }
                }
                for &(si, rect) in rack.tabs.iter() {
                    if rect.contains(pos) {
                        return MouseAction::RackTab(si);
                    }
                }
                if rack.tab_add.is_some_and(|r| r.contains(pos)) {
                    return MouseAction::RackTabAdd;
                }
                if rack.mute.is_some_and(|r| r.contains(pos)) {
                    return MouseAction::MixMute;
                }
                if rack.solo.is_some_and(|r| r.contains(pos)) {
                    return MouseAction::MixSolo;
                }
                for &(i, rect) in rack.fx_slots.iter() {
                    if rect.contains(pos) {
                        return MouseAction::FxSlot(i);
                    }
                }
                for &(pi, rect) in rack.arp_knobs.iter() {
                    if rect.contains(pos) {
                        return MouseAction::ArpParamSel(pi);
                    }
                }
                for &(pi, rect) in rack.instr_knobs.iter() {
                    if rect.contains(pos) {
                        return MouseAction::InstrParamSel(pi);
                    }
                }
                for &(pi, rect) in rack.params.iter() {
                    if rect.contains(pos) {
                        return MouseAction::FxParam(pi);
                    }
                }
                for (rect, action) in [
                    (rack.fx_add, MouseAction::FxAdd),
                    (rack.on_off, MouseAction::FxToggle),
                    (rack.del, MouseAction::FxDelete),
                    (rack.move_left, MouseAction::FxMoveLeft),
                    (rack.move_right, MouseAction::FxMoveRight),
                    (rack.fx_preset, MouseAction::OpenFxPresets),
                    (rack.fx_gui, MouseAction::ToggleFxEditor),
                    (rack.fx_sandbox, MouseAction::ToggleFxSandbox),
                ] {
                    if rect.is_some_and(|r| r.contains(pos)) {
                        return action;
                    }
                }
                return MouseAction::FocusPanel(Focus::FxChain);
            }

            if layout.transport_area.contains(pos) {
                if layout.panic_rect.is_some_and(|r| r.contains(pos)) {
                    return MouseAction::Panic;
                }
                if layout.clock_rect.is_some_and(|r| r.contains(pos)) {
                    return MouseAction::ToggleMidiClock;
                }
                if let Some(r) = layout.loop_rect {
                    if r.contains(pos) {
                        // Left half shortens, right half lengthens — the two
                        // arrows the cell is drawn with.
                        let d = if pos.x < r.x + r.width / 2 { -1 } else { 1 };
                        return MouseAction::AutomationLoop(d);
                    }
                }
                if layout.out_device_rect.is_some_and(|r| r.contains(pos)) {
                    return MouseAction::ToggleOutDrawer;
                }
                return MouseAction::FocusPanel(Focus::Transport);
            }

            if layout.output_area.contains(pos) {
                if layout.output_item_rects.is_empty() {
                    return MouseAction::ToggleOutDrawer;
                }
                for &(di, rect) in layout.output_item_rects.iter() {
                    if rect.contains(pos) {
                        return MouseAction::OutputDevice(di);
                    }
                }
                return MouseAction::FocusPanel(Focus::Output);
            }

            MouseAction::None
        }
        // The right button only means one thing, and only over the two drawers:
        // take this channel off the tab. Everywhere else it does nothing.
        MouseEventKind::Down(MouseButton::Right) => {
            if layout.source_area.contains(pos) {
                for &(ii, rect) in layout.input_item_rects.iter() {
                    if rect.contains(pos) {
                        return MouseAction::InputUnassign(ii);
                    }
                }
            }
            if layout.output_area.contains(pos) {
                for &(di, rect) in layout.output_item_rects.iter() {
                    if rect.contains(pos) {
                        return MouseAction::OutputUnassign(di);
                    }
                }
            }
            MouseAction::None
        }
        MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
            let dir = if matches!(kind, MouseEventKind::ScrollUp) {
                1.0
            } else {
                -1.0
            };
            // The drawers scroll by moving their cursor — there is no separate
            // offset to move (see `drawer::list_scroll`).
            let step = if dir > 0.0 { -1 } else { 1 };
            if layout.source_area.contains(pos) {
                return MouseAction::InputStep(step);
            }
            if layout.output_area.contains(pos) {
                return MouseAction::OutputStep(step);
            }
            for &(tab, rect) in layout.monitor_tabs.iter() {
                if rect.contains(pos) {
                    return MouseAction::MonitorTab(tab);
                }
            }
            // The wheel over a MIXER control moves it, in the step the RACK's
            // own VOL knob uses: a fader four rows tall cannot be *set* finely
            // with a click, and this is what makes it playable anyway.
            for &(hit, rect) in layout.mixer_hits.iter() {
                if !rect.contains(pos) {
                    continue;
                }
                use views::midi_monitor::MixerHit;
                return match hit {
                    MixerHit::Gain(i) => MouseAction::MixerNudge(i, MixSide::Left, dir * GAIN_STEP),
                    MixerHit::GainR(i) => {
                        MouseAction::MixerNudge(i, MixSide::Right, dir * GAIN_STEP)
                    }
                    MixerHit::Pan(i) => MouseAction::MixerPan(i, dir * 0.05),
                    _ => MouseAction::None,
                };
            }

            if layout.fx_chain_area.contains(pos) {
                let rack = &layout.rack;
                if rack.gain.is_some_and(|r| r.contains(pos)) {
                    return MouseAction::MixGain(dir * 0.05);
                }
                if rack.pan.is_some_and(|r| r.contains(pos)) {
                    return MouseAction::MixPan(dir * 0.1);
                }
                if rack.in_gain.is_some_and(|r| r.contains(pos)) {
                    return MouseAction::InTrim(dir * 0.05, 0.0);
                }
                // The wheel over the converter's dry/wet nudges it, the way it
                // does over every other level on this panel.
                for &(btn, rect) in rack.buttons.iter() {
                    if btn == views::fx_chain_panel::RackButton::PitchMix && rect.contains(pos) {
                        return MouseAction::PitchMixAdjust(dir * 0.05);
                    }
                }
                if rack.in_gate.is_some_and(|r| r.contains(pos)) {
                    return MouseAction::InTrim(0.0, dir * 0.05);
                }
                for &(pi, rect) in rack.arp_knobs.iter() {
                    if rect.contains(pos) {
                        return MouseAction::ArpParamAdjust(pi, dir * 0.03);
                    }
                }
                for &(pi, rect) in rack.instr_knobs.iter() {
                    if rect.contains(pos) {
                        return MouseAction::InstrParamAdjust(pi, dir * 0.03);
                    }
                }
                for &(pi, rect) in rack.params.iter() {
                    if rect.contains(pos) {
                        return MouseAction::FxParamAdjust(pi, dir * 0.03);
                    }
                }
            }
            MouseAction::None
        }
        _ => MouseAction::None,
    }
}

fn apply_menu_action(app: &mut App, action: menu::MenuAction) {
    use menu::MenuAction as A;
    match action {
        A::None => {}
        A::OpenWav => app.open_browser_modal(&["wav"]),
        A::OpenSf2 => app.open_browser_modal(&["sf2", "sf3"]),
        A::Quit => app.quit = true,
        A::PluginPaths => app.open_paths_modal(),
        A::SaveProject => app.save_project(),
        A::SaveProjectAs => app.open_save_project(),
        A::LoadProject => app.open_load_project(),
        A::ImportMax => app.open_import_max(),
        A::RescanPlugins => app.start_rescan(),
        A::About => app.about_open = true,
    }
}

fn handle_mouse(app: &mut App, mouse: MouseEvent) {
    let pos: ratatui::layout::Position = (mouse.column, mouse.row).into();
    let left = matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left));

    // ── MIDI learn pointer ─────────────────────────────────────────────────
    if app.learn_pick {
        app.mouse = (mouse.column, mouse.row);
        if left {
            match app.learn_target_at(pos) {
                // Picked: keep the `?` pointer up (choz is now listening for a
                // MIDI fader); clicking another control just re-targets.
                Some(target) => app.learn = Some(target),
                // A click anywhere else cancels, rather than doing the normal
                // action behind the `?` pointer.
                None => app.end_learn(),
            }
        }
        return;
    }

    // ── Modal / menu mouse handling (needs app state) ──────────────────────
    if app.about_open {
        if left {
            app.about_open = false;
        }
        return;
    }

    if let Some(state) = app.menu {
        if left {
            let (item_hit, title_hit) = {
                let l = app.layout.borrow();
                (
                    l.menu_item_rects
                        .iter()
                        .find(|(_, r)| r.contains(pos))
                        .map(|(i, _)| *i),
                    l.menu_bar_rects.iter().position(|r| r.contains(pos)),
                )
            };
            if let Some(idx) = item_hit {
                let action = state.kind.items()[idx].action;
                app.menu = None;
                apply_menu_action(app, action);
            } else if let Some(ti) = title_hit {
                app.menu = Some(menu::MenuState::open(menu::MenuKind::ALL[ti]));
            } else {
                app.menu = None; // click-away dismiss
            }
        }
        return;
    }

    // Menu closed: a click on a bar title opens it.
    if left {
        let switch = app.layout.borrow().mode_switch_rect;
        if switch.is_some_and(|r| r.contains(pos)) {
            app.toggle_rack_mode();
            return;
        }
        let (met, met_menu) = {
            let l = app.layout.borrow();
            (l.met_rect, l.met_menu_rect)
        };
        if met.is_some_and(|r| r.contains(pos)) {
            app.toggle_metronome();
            return;
        }
        if met_menu.is_some_and(|r| r.contains(pos)) {
            app.open_metronome_modal();
            return;
        }
        let title_hit = app
            .layout
            .borrow()
            .menu_bar_rects
            .iter()
            .position(|r| r.contains(pos));
        if let Some(ti) = title_hit {
            app.menu = Some(menu::MenuState::open(menu::MenuKind::ALL[ti]));
            return;
        }
    }

    // Any modal: rows select, chips filter, the wheel scrolls, SELECT/CANCEL
    // are buttons, and a click outside the popup cancels.
    if app.modal.is_some() {
        handle_modal_mouse(app, mouse);
        return;
    }

    // ── Base UI ────────────────────────────────────────────────────────────
    let action = {
        let layout = app.layout.borrow();
        mouse_action(mouse.column, mouse.row, &layout, mouse.kind)
    };

    match action {
        MouseAction::None => {}
        MouseAction::FocusPanel(f) => {
            app.focus = f;
        }
        MouseAction::FxSlot(i) => {
            app.focus = Focus::FxChain;
            app.fx_slot = i;
            app.fx_param = 0;
        }
        MouseAction::FxParam(pi) => {
            // A click on a named parameter opens its list rather than nudging
            // it: there is nothing to nudge, only positions to choose.
            if app.fx_param == pi && app.open_fx_choice(pi) {
                return;
            }
            app.focus = Focus::FxChain;
            app.rack_focus = RackFocus::Fx;
            app.fx_param = pi;
        }
        MouseAction::ArpParamSel(pi) => {
            app.focus = Focus::FxChain;
            // A second click on the knob already under the cursor opens its
            // list, or flips it when there is no list to open — the same
            // gesture the FX knobs answer to.
            if app.rack_focus == RackFocus::Arp && app.arp_param == pi && !app.open_arp_choice(pi) {
                app.nudge_arp_knob(pi, 1.0);
            }
            app.rack_focus = RackFocus::Arp;
            app.arp_param = pi;
        }
        MouseAction::ArpParamAdjust(pi, delta) => app.nudge_arp_knob(pi, delta),
        MouseAction::ArpPick(param) => {
            app.focus = Focus::FxChain;
            if let Some(index) = app.arp_knob_index(param) {
                app.rack_focus = RackFocus::Arp;
                app.arp_param = index;
                if !app.open_arp_choice(index) {
                    app.nudge_arp_knob(index, 1.0);
                }
            }
        }
        MouseAction::InstrParamSel(pi) => {
            app.focus = Focus::FxChain;
            app.rack_focus = RackFocus::Instrument;
            app.instr_param = pi;
        }
        MouseAction::InstrParamAdjust(pi, delta) => {
            let v = app
                .slots
                .get(app.active_slot)
                .and_then(|s| s.instr_values.get(pi))
                .copied()
                .unwrap_or(0.0);
            // The wheel steps a switch or a named position exactly like the
            // arrows do — 0.03 of a range that only has two places in it lands
            // between them.
            let shape = app.instr_param_shape(pi);
            app.set_instr_param(pi, shape.nudge(v, delta));
        }
        MouseAction::FxParamAdjust(pi, delta) => {
            let old_slot = app.fx_slot;
            let old_param = app.fx_param;
            app.fx_param = pi;
            adjust_fx_param(app, delta);
            app.fx_slot = old_slot;
            app.fx_param = old_param;
        }
        MouseAction::TogglePitchToMidi => app.toggle_pitch_to_midi(),
        MouseAction::PitchMixCycle => app.step_pitch_mix(-0.25, true),
        MouseAction::PitchMixAdjust(d) => app.step_pitch_mix(d, false),
        MouseAction::MonitorTab(tab) => app.monitor_tab = tab,
        MouseAction::Panic => app.panic(),
        MouseAction::ChannelStep(d) => app.step_channel(d),
        MouseAction::FxAdd => app.open_add_fx_modal(),
        MouseAction::FxToggle => {
            if let Some(entry) = app.fx_chain.get_mut(app.fx_slot) {
                entry.enabled = !entry.enabled;
                app.rebuild_fx();
            }
        }
        MouseAction::FxDelete => {
            if !app.fx_chain.is_empty() {
                app.fx_chain.remove(app.fx_slot);
                if app.fx_slot >= app.fx_chain.len() && app.fx_slot > 0 {
                    app.fx_slot -= 1;
                }
                app.rebuild_fx();
            }
        }
        MouseAction::FxMoveLeft => {
            if app.fx_slot > 0 {
                app.fx_chain.swap(app.fx_slot, app.fx_slot - 1);
                app.fx_slot -= 1;
                app.fx_param = 0;
                app.rebuild_fx();
            }
        }
        MouseAction::FxMoveRight => {
            if app.fx_slot + 1 < app.fx_chain.len() {
                app.fx_chain.swap(app.fx_slot, app.fx_slot + 1);
                app.fx_slot += 1;
                app.fx_param = 0;
                app.rebuild_fx();
            }
        }
        MouseAction::TransportPlay => {
            app.focus = Focus::Transport;
            toggle_play(app);
        }
        MouseAction::TransportStop => {
            app.focus = Focus::Transport;
            stop_play(app);
        }
        MouseAction::ToggleInDrawer => app.toggle_in_drawer(),
        MouseAction::ToggleOutDrawer => app.toggle_out_drawer(),
        MouseAction::OutputDevice(i) => {
            app.focus = Focus::Output;
            app.out_cursor = i;
            // The left button assigns. It is not a toggle, so clicking a
            // channel twice is not the same as clicking it and taking it back —
            // that is what the right button is for.
            app.out_select_side(i, Assign::On);
        }
        MouseAction::OutputUnassign(i) => {
            app.focus = Focus::Output;
            app.out_cursor = i;
            app.out_select_side(i, Assign::Off);
        }
        MouseAction::InputBind(i) => {
            app.focus = Focus::Source;
            app.input_cursor = i;
            app.in_select_side(i, Assign::On);
        }
        MouseAction::InputUnassign(i) => {
            app.focus = Focus::Source;
            app.input_cursor = i;
            app.in_select_side(i, Assign::Off);
        }
        MouseAction::InputToggle(i) => {
            app.focus = Focus::Source;
            app.input_cursor = i;
            app.toggle_selected_input();
        }
        MouseAction::OpenSourcePicker => app.open_source_modal(),
        MouseAction::ScanInputs => {
            app.focus = Focus::Source;
            app.connect_midi();
            app.rescan_capture();
        }
        MouseAction::PresetStep(d) => app.step_preset(d),
        MouseAction::InstrPage(d) => app.page_instr(d),
        MouseAction::ArpEdit(edit) => app.edit_arp(edit),
        MouseAction::OpenPresetPicker => app.open_preset_modal(),
        MouseAction::OpenLearnPicker => app.start_learn_pick(),
        MouseAction::ToggleEditor => app.toggle_editor(None),
        MouseAction::ToggleFxEditor => app.toggle_editor(Some(app.fx_slot)),
        MouseAction::OpenFxPresets => {
            app.open_fx_presets();
        }
        MouseAction::InputStep(d) => app.input_cursor = in_step(app, d),
        MouseAction::OutputStep(d) => app.out_cursor = out_step(app, d),
        MouseAction::ToggleSandbox => app.toggle_sandbox(None),
        MouseAction::ToggleFxSandbox => app.toggle_sandbox(Some(app.fx_slot)),
        MouseAction::RackTab(i) => {
            app.focus = Focus::FxChain;
            app.switch_slot(i);
        }
        MouseAction::RackTabClose(i) => {
            app.focus = Focus::FxChain;
            app.remove_slot(i);
        }
        MouseAction::RackTabAdd => {
            app.focus = Focus::FxChain;
            app.add_slot_on_active_input();
        }
        MouseAction::MixGain(d) => adjust_gain(app, d),
        MouseAction::MixPan(d) => adjust_pan(app, d),
        MouseAction::InTrim(g, s) => app.adjust_in_trim(g, s),
        MouseAction::ToggleMidiClock => app.set_midi_clock(!app.midi_clock()),
        MouseAction::AutomationLoop(d) => {
            app.focus = Focus::Transport;
            app.nudge_automation_loop(d);
        }
        MouseAction::MixMute => app.with_active_mix(|s| s.mute = !s.mute),
        MouseAction::MixSolo => app.with_active_mix(|s| s.solo = !s.solo),
        MouseAction::MixerSet(i, hit, v) => match hit {
            views::midi_monitor::MixerHit::Gain(_) => app.set_gain_side(i, MixSide::Left, v),
            views::midi_monitor::MixerHit::GainR(_) => app.set_gain_side(i, MixSide::Right, v),
            views::midi_monitor::MixerHit::Pan(_) => {
                app.with_mix(i, |s| s.pan = (v * 2.0 - 1.0).clamp(-1.0, 1.0))
            }
            _ => {}
        },
        MouseAction::MixerNudge(i, side, d) => app.nudge_gain(i, side, d),
        MouseAction::MixerPan(i, d) => {
            app.with_mix(i, |s| s.pan = (s.pan + d).clamp(-1.0, 1.0));
        }
        MouseAction::MixerPage(d) => {
            // A page is however many strips fit, which only the panel knows —
            // so it is read back from what it drew, like every other window in
            // choz. Moving the active tab moves the window with it.
            let per_page = app
                .layout
                .borrow()
                .mixer_hits
                .iter()
                .filter(|(h, _)| matches!(h, views::midi_monitor::MixerHit::Select(_)))
                .count()
                .max(1) as isize;
            let at = app.active_slot as isize + d * per_page;
            let last = app.slots.len().saturating_sub(1) as isize;
            app.switch_slot(at.clamp(0, last) as usize);
        }
        MouseAction::MixerToggle(i, hit) => match hit {
            views::midi_monitor::MixerHit::Mute(_) => app.with_mix(i, |s| s.mute = !s.mute),
            views::midi_monitor::MixerHit::Solo(_) => app.with_mix(i, |s| s.solo = !s.solo),
            views::midi_monitor::MixerHit::Link(_) => app.toggle_link(i),
            views::midi_monitor::MixerHit::Select(_) => {
                // Clicking a strip is how the MIXER takes the keyboard: from
                // then on the arrows are on levels.
                app.focus = Focus::Mixer;
                app.switch_slot(i);
            }
            _ => {}
        },
    }
}

/// Mouse inside an open modal. All modals share `layout.modal_rects`, so this
/// works the same for the source picker, ADD FX, devices, presets and browser.
fn handle_modal_mouse(app: &mut App, mouse: MouseEvent) {
    let pos: ratatui::layout::Position = (mouse.column, mouse.row).into();
    let rects = app.layout.borrow().modal_rects.clone();

    match mouse.kind {
        // The metronome's menu has five rows and no scrolling: the wheel over
        // one of them is worth more as the value than as a cursor.
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
            if app.modal.as_ref().is_some_and(|m| m.kind == ModalKind::Metronome) =>
        {
            if let Some(&(row, _)) = rects.rows.iter().find(|(_, r)| r.contains(pos)) {
                if let Some(m) = app.modal.as_mut() {
                    m.list.cursor = row;
                }
                let delta = if matches!(mouse.kind, MouseEventKind::ScrollUp) {
                    1
                } else {
                    -1
                };
                app.step_metronome_row(row, delta);
                app.refresh_modal();
            }
        }
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            if rects.list.is_some_and(|r| r.contains(pos))
                || rects.scrollbar.is_some_and(|r| r.contains(pos))
            {
                // Three rows a notch: one is a crawl on a list of hundreds.
                let delta = if matches!(mouse.kind, MouseEventKind::ScrollUp) {
                    -3
                } else {
                    3
                };
                if let Some(m) = app.modal.as_mut() {
                    m.list.move_cursor(delta);
                    if let Some(b) = m.browser.as_mut() {
                        b.cursor = m.list.cursor;
                    }
                }
                // **No `refresh_modal` here.** It rebuilds every row string from
                // scratch (and for ADD FX re-walks the categories), which on a
                // few hundred plugins is what made the wheel feel like treacle.
                // Nothing it rebuilds depends on the cursor — only the filter
                // and the sidebar do, and neither moved.
            }
        }
        // Dragging the scrollbar, and clicking anywhere on its track, jump the
        // cursor there. Without this the bar is decoration: it draws, but the
        // mouse falls through to the rows behind it.
        MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Drag(MouseButton::Left)
            if rects.scrollbar.is_some_and(|r| r.contains(pos)) =>
        {
            let track = rects.scrollbar.unwrap();
            if let Some(m) = app.modal.as_mut() {
                m.list.drag_to(track, mouse.row);
                if let Some(b) = m.browser.as_mut() {
                    b.cursor = m.list.cursor;
                }
            }
        }
        MouseEventKind::Down(MouseButton::Left) => {
            if rects.cancel.is_some_and(|r| r.contains(pos)) {
                app.close_modal();
                return;
            }
            if let Some(&(key, _)) = rects.actions.iter().find(|(_, r)| r.contains(pos)) {
                handle_modal_key(app, KeyCode::Char(key));
                return;
            }
            // Sidebar: clicking a section shows it.
            if let Some(&(i, _)) = rects.sidebar.iter().find(|(_, r)| r.contains(pos)) {
                if let Some(m) = app.modal.as_mut() {
                    m.list.sidebar_cursor = i;
                    m.list.sidebar_focused = true;
                    m.list.cursor = 0;
                    m.list.scroll = 0;
                }
                app.refresh_modal();
                return;
            }
            if let Some((i, _)) = rects.filters.iter().find(|(_, r)| r.contains(pos)) {
                if let Some(m) = app.modal.as_mut() {
                    m.list.filter = *i;
                    m.list.cursor = 0;
                    m.list.scroll = 0;
                    m.list.sidebar_cursor = 0;
                }
                app.refresh_modal();
                return;
            }
            // A click on a row selects it; SELECT (or a second click on the same
            // row) confirms — the same feel as a double click, without timing.
            let row = rects
                .rows
                .iter()
                .find(|(_, r)| r.contains(pos))
                .map(|(i, _)| *i);
            let confirm = match row {
                Some(i) => {
                    let same = app
                        .modal
                        .as_ref()
                        .is_some_and(|m| m.list.cursor == i && !m.list.sidebar_focused);
                    if let Some(m) = app.modal.as_mut() {
                        m.list.sidebar_focused = false;
                        m.list.cursor = i;
                        if let Some(b) = m.browser.as_mut() {
                            b.cursor = i;
                        }
                    }
                    app.refresh_modal();
                    same
                }
                None => {
                    if rects.select.is_some_and(|r| r.contains(pos)) {
                        // The button means "done": apply what is under the
                        // cursor and leave, even on tabs where Enter keeps the
                        // modal open so several things can be set at once.
                        app.modal_select();
                        app.close_modal();
                        app.focus = Focus::FxChain;
                        return;
                    }
                    // Click outside the popup dismisses it.
                    if !rects.area.is_some_and(|r| r.contains(pos)) {
                        app.close_modal();
                    }
                    return;
                }
            };
            if confirm && app.modal_select() {
                app.close_modal();
                app.focus = Focus::FxChain;
            }
        }
        _ => {}
    }
}

/// Knob step of the instrument-parameter editor (left/right arrows).
const INSTR_STEP: f32 = 0.05;

/// One row of the INSTRUMENT parameter list: name, control, value.
///
/// The list is the long form of the same three controls the RACK's knob box
/// draws — which is where a **checkbox** belongs rather than a button: a switch
/// among forty rows should cost one column, not a box. The value carries the
/// plugin's unit when it gave one, because "0.42" and "0.42 Hz" are different
/// amounts of information.
fn instr_param_row(p: &choz_engine::PluginParam, v: f32) -> String {
    use source::ParamShape;
    let name = views::fx_chain_panel::truncate(&p.name, 22);
    let shape = ParamShape::of(p);
    match (&shape, shape.step_at(v)) {
        (ParamShape::Toggle, Some((k, _))) => {
            let on = k == 1;
            format!(
                "{name:<22} [{}] {}",
                if on { "x" } else { " " },
                if on { "ON" } else { "OFF" }
            )
        }
        (ParamShape::Named(_), Some((k, n))) => format!(
            "{name:<22} \u{25C0} {:<16} \u{25B6}  {}/{n}",
            views::fx_chain_panel::truncate(shape.label(k).unwrap_or("?"), 16),
            k + 1,
        ),
        (ParamShape::Fader(_), _) => format!(
            "{name:<22} {} {:>10.3}{}",
            views::fx_chain_panel::fader_track(v, 10),
            p.plain(v as f64),
            match &p.unit {
                Some(u) => format!(" {u}"),
                None => String::new(),
            }
        ),
        _ => format!(
            "{name:<22} [{}] {:>10.3}{}",
            views::fx_chain_panel::knob_arc(v, 8),
            p.plain(v as f64),
            match &p.unit {
                Some(u) => format!(" {u}"),
                None => String::new(),
            }
        ),
    }
}

/// Which of the RACK's two knob boxes the arrows and the highlight belong to.
///
/// Carla shows a plugin's parameters as knobs next to the button that opens its
/// real window; choz does the same, so the RACK now holds two boxes — the
/// instrument's own parameters and the selected FX's — and one of them has the
/// cursor. `i` swaps, and clicking a knob in either box moves the focus there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum RackFocus {
    #[default]
    Fx,
    Instrument,
    /// The arpeggiator's knob box, when the screen is tall enough to draw one.
    Arp,
}

// ─── UI Render ─────────────────────────────────────────────────────────────────

fn ui(f: &mut Frame, app: &mut App) {
    let area = f.area();

    // Behind everything, before anything else touches the buffer: the panels
    // above set foreground colours and symbols but leave `bg` alone, so
    // whatever is painted here shows through.
    // …unless the terminal is already showing the picture underneath at real
    // pixel resolution (kitty graphics, `z=-1`), in which case painting cells
    // would cover it.
    if app.kitty_bg.is_none() {
        views::background::render(f.buffer_mut(), area, &app.ui.background, &mut app.wallpaper);
    }
    // What the panels blend their colour with, so they read as translucent over
    // the picture. Cheap to publish (a colour per cell) and independent of the
    // opacity, which is why moving that slider costs nothing.
    app.publish_backdrop(area);

    // ─── Splash Screen ──────────────────────────────────────────────────
    if !app.splash_done {
        draw_splash(f, &app.splash, area);
        return;
    }

    // Top: menu bar (1 row) · middle: body · bottom: status bar (1 row).
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);
    let menubar_area = root[0];
    let body = root[1];
    let status_area = root[2];

    draw_menu_bar(f, app, menubar_area);

    // IN and OUT are drawers: a 3-wide handle each while closed, a real panel
    // once open. The RACK takes whatever is left, which is everything when
    // both are shut.
    let in_w = views::drawer::drawer_width(app.in_open, body.width, 40, 24);
    // The right one measures itself against what the left one left behind, so
    // two open drawers can never squeeze the RACK out of existence.
    let out_w = views::drawer::drawer_width(app.out_open, body.width.saturating_sub(in_w), 34, 24);
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(in_w),
            Constraint::Min(10),
            Constraint::Length(out_w),
        ])
        .split(body);

    // The MIDI monitor takes whatever is left once the rack and the transport
    // have theirs, up to 8 rows. Below 3 (a border plus one message) it is not
    // worth the space and disappears entirely.
    let monitor_height = monitor_rows(body.height);
    let right_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(10),
            Constraint::Length(7),
            Constraint::Length(monitor_height),
        ])
        .split(chunks[1]);

    let source_area = chunks[0];
    let output_area = chunks[2];
    let fx_chain_area = right_chunks[0];
    let transport_area = right_chunks[1];
    let monitor_area = right_chunks[2];

    // Each box gets the theme's colour washed over the picture behind it, so
    // its labels and knobs read while the wallpaper still shows through. Done
    // here, before the panels draw, because they leave `bg` alone once a
    // desktop is set — so the wash survives underneath them.
    app.wash_rects.clear();
    for a in [
        source_area,
        output_area,
        fx_chain_area,
        transport_area,
        monitor_area,
    ] {
        views::theme::wash(f.buffer_mut(), a);
        app.wash_rects.push((a, 1.0));
    }
    // The bars top and bottom are part of the frame, not of a box: a lighter
    // wash keeps them legible without turning into two solid stripes.
    for a in [menubar_area, status_area] {
        views::theme::wash_weak(f.buffer_mut(), a);
        app.wash_rects.push((a, views::theme::WEAK_WASH));
    }

    if app.in_open {
        let rows: Vec<views::source_panel::InputRow> =
            app.in_targets().into_iter().map(|(_, r)| r).collect();
        let learn = app.learn_banner();
        let scan_rect = views::source_panel::draw_input_panel(
            f,
            source_area,
            app.focus == Focus::Source,
            &rows,
            app.input_cursor,
            &app.active_tab_label(),
            learn.as_deref(),
        );
        let close = views::drawer::draw_close_button(f, source_area);
        let mut layout = app.layout.borrow_mut();
        layout.input_scan_rect = scan_rect;
        layout.in_close_rect = close;
    } else {
        views::drawer::draw_handle(f, source_area, "IN", false);
        let mut layout = app.layout.borrow_mut();
        layout.input_scan_rect = None;
        layout.in_close_rect = None;
    }

    if app.out_open {
        let rows: Vec<views::drawer::OutRow> =
            app.out_targets().into_iter().map(|(_, r)| r).collect();
        views::drawer::draw_output_panel(
            f,
            output_area,
            app.focus == Focus::Output,
            &rows,
            app.out_cursor,
        );
        let close = views::drawer::draw_close_button(f, output_area);
        app.layout.borrow_mut().out_close_rect = close;
    } else {
        views::drawer::draw_handle(f, output_area, "OUT", false);
        app.layout.borrow_mut().out_close_rect = None;
    }

    let tabs: Vec<String> = app.slots.iter().map(tab_label).collect();
    let mix = app
        .slots
        .get(app.active_slot)
        .map(|s| (s.gain, s.pan, s.mute, s.solo));
    // AutoTune draws a live reading under its knobs, and the reading is only
    // meaningful for the effect the cursor is on.
    let at_selected = app
        .fx_chain
        .get(app.fx_slot)
        .is_some_and(|e| e.plugin.is_none() && e.kind == source::AudioFxKind::AutoTune);
    let at_view = if at_selected {
        let m = choz_engine::fx::autotune::meter::meter().read();
        app.autotune_trace.remove(0);
        app.autotune_trace.push(if m.voiced {
            m.pitch_error_cents
        } else {
            f32::NAN
        });
        Some((m, app.autotune_trace.as_slice()))
    } else {
        None
    };
    let rack = views::fx_chain_panel::draw_fx_chain_panel(
        f,
        fx_chain_area,
        &app.fx_chain,
        app.fx_slot,
        app.fx_param,
        app.focus == Focus::FxChain,
        &tabs,
        app.active_slot,
        mix,
        &app.instrument_label(),
        app.active_preset_label().as_deref(),
        app.has_editor(),
        app.has_fx_editor(),
        app.sbx_state(None),
        app.sbx_state(Some(app.fx_slot)),
        app.tab_channel(),
        app.pitch_to_midi_state(),
        app.slots
            .get(app.active_slot)
            .map(|s| s.pitch_mix)
            .unwrap_or(1.0),
        &app.instr_knobs(),
        app.instr_param,
        app.rack_focus == RackFocus::Instrument,
        app.in_trim_state(),
        at_view,
        app.slots
            .get(app.active_slot)
            .map(|s| arp::ArpView {
                cursor: app.arp_param,
                focused: app.rack_focus == RackFocus::Arp,
                ..s.arp.view()
            })
            .unwrap_or_default(),
        app.fx_slot_info(),
    );
    app.layout.borrow_mut().rack = rack;
    // The knob box may have scrolled; the CCs learned on it go where it went.
    app.sync_instr_window();

    draw_transport(f, app, transport_area);

    if monitor_area.height > 0 {
        // What `A→M` is playing, on the keyboard with everything else. Those
        // notes are made in the audio callback and never travel as MIDI, so
        // this is the only place they can be seen — and a converter you cannot
        // watch is one you can only trust or not.
        let converting = app
            .slots
            .iter()
            .position(|s| s.in_pair.is_some() && s.pitch_to_midi);
        app.keyboard.feed_converted(
            views::midi_monitor::Converted::PitchToMidi,
            converting.and_then(|_| choz_engine::meter::pitch_meter().note()),
            converting,
        );
        // And what AutoTune is aiming at, for the same reason: it decides a
        // note in the callback, corrects towards it, and says so nowhere a
        // player can watch. The tab is the first one with an AutoTune enabled —
        // the meter is one per process, so with two of them the last one to run
        // is the one being shown, which is what its own readout does too.
        let tuning = app.slots.iter().position(|s| {
            s.fx_chain
                .iter()
                .any(|e| e.enabled && e.kind == source::AudioFxKind::AutoTune)
        });
        let tuned = tuning.and_then(|_| {
            let m = choz_engine::fx::autotune::meter::meter().read();
            (m.voiced && m.target_frequency > 0.0).then(|| {
                (69.0f32 + 12.0 * (m.target_frequency / 440.0).log2())
                    .round()
                    .clamp(0.0, 127.0) as u8
            })
        });
        app.keyboard
            .feed_converted(views::midi_monitor::Converted::AutoTune, tuned, tuning);
        // The FFT runs here, on the UI thread, and only while its tab is on
        // screen. The rate comes from the transport, which the engine sets when
        // the stream opens — the analyser has no other way to know what a bin
        // is worth in Hz.
        if app.monitor_tab.needs_spectrum() {
            app.spectrum
                .set_sample_rate(choz_ports::transport().sample_rate() as f32);
            app.spectrum.update();
        }
        // The stack only advances while it is being looked at. A history taken
        // off screen would scroll past unseen and be gone by the time the tab
        // came back, which is worse than starting empty.
        if app.monitor_tab == views::midi_monitor::MonitorTab::Wave {
            app.wave.tick();
        }
        let log: Vec<midi::InputEvent> = app.midi_log.iter().copied().collect();
        let (tabs, hits) = views::midi_monitor::draw_midi_monitor(
            f,
            monitor_area,
            &log,
            &app.midi_ports,
            app.monitor_tab,
            &app.keyboard,
            app.ui.key_colour,
            &app.spectrum,
            &app.wave,
            &app.mixer_strips(),
        );
        let mut layout = app.layout.borrow_mut();
        layout.monitor_tabs = tabs;
        layout.mixer_hits = hits;
    }

    if app.modal.is_some() {
        // The modal owns its scroll state, so it draws from a &mut borrow and
        // stores its hit rects where the mouse handler can find them.
        let mut modal = app.modal.take().unwrap();
        let pct = (70, 70);
        let rects = views::modal::draw_list_modal(f, &mut modal.list, area, pct);
        app.layout.borrow_mut().modal_rects = rects;
        app.modal = Some(modal);
    } else {
        app.layout.borrow_mut().modal_rects = views::modal::ModalRects::default();
    }

    // The MIDI-learn pointer rides above every panel.
    if app.learn_pick {
        let (mx, my) = app.mouse;
        if mx < area.width && my < area.height {
            f.render_widget(
                Paragraph::new(Span::styled(
                    "?",
                    Style::default()
                        .fg(Color::Black)
                        .bg(WARN)
                        .add_modifier(Modifier::BOLD),
                )),
                Rect::new(mx, my, 1, 1),
            );
        }
    }

    // Dropdown + About draw on top of everything.
    if let Some(ref m) = app.menu {
        draw_menu_dropdown(f, app, *m, menubar_area);
    }
    if app.about_open {
        draw_about(f, app, area);
    }
    // Above everything, including About: the scan blocks what those dialogs
    // would act on.
    if let Some(ref job) = app.scan {
        draw_scan_progress(f, job, area);
    }
    // …and above that, what this frame exists for: the next thing this thread
    // does is instantiate a plugin, which can take seconds with nothing moving.
    if let Some(name) = app.loading.clone() {
        draw_loading(f, &name, area);
    }

    // Status bar
    let backend_label = app
        .audio_engine
        .as_ref()
        .map(|e| e.backend.label())
        .unwrap_or("none");
    let play_icon = if app.playing { "\u{25B6}" } else { "\u{25A0}" };
    let play_state = if app.playing {
        i18n::t("PLAYING")
    } else {
        i18n::t("STOPPED")
    };

    // How much of the audio block the callback is using. The one number that
    // says "the machine cannot render this rack in time" while it is happening,
    // rather than after the sound has already gone.
    let dsp = choz_engine::meter::load().last() * 100.0;
    let status_text = format!(
        " choz v0.1 | {} backend | RACK: {} | FX: {} | DSP {dsp:.0}% | {play_icon} {play_state} | F2=IN F3=OUT F10=menu q=quit",
        backend_label,
        app.active_tab_label(),
        app.fx_chain.len(),
    );
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            status_text,
            Style::default().fg(STATUS_FG).bg(STATUS_BG),
        ))),
        status_area,
    );

    // ─── Compute layout for mouse hit-testing ──────────────────────────
    compute_layout(
        app,
        area,
        source_area,
        fx_chain_area,
        transport_area,
        output_area,
    );
}

/// Rows the MIDI monitor gets out of a body `height`, after the rack's minimum
/// (10) and the transport's fixed 7. Capped at 8; anything under 3 is dropped so
/// a squeezed terminal keeps the rack readable instead of showing a sliver.
fn monitor_rows(height: u16) -> u16 {
    let left = height.saturating_sub(10 + 7);
    if left < 3 {
        0
    } else {
        left.min(8)
    }
}

fn compute_layout(
    app: &App,
    _area: Rect,
    source_area: Rect,
    fx_chain_area: Rect,
    transport_area: Rect,
    output_area: Rect,
) {
    let mut layout = app.layout.borrow_mut();

    layout.source_area = source_area;
    layout.fx_chain_area = fx_chain_area;
    layout.transport_area = transport_area;
    layout.output_area = output_area;

    let inner = |a: Rect| {
        Rect::new(
            a.x + 1,
            a.y + 1,
            a.width.saturating_sub(2),
            a.height.saturating_sub(2),
        )
    };

    // Input rows. Line layout must match `draw_input_panel`: the list starts at
    // INPUT_LIST_TOP and the connect mark is the second column of each row.
    use views::source_panel as sp;
    layout.input_item_rects.clear();
    layout.input_mark_rects.clear();
    if app.in_open {
        let src_inner = inner(source_area);
        let list_y = src_inner.y + sp::INPUT_LIST_TOP as u16;
        let rows = app.in_targets().len();
        // The same window the panel draws: a row that is scrolled off screen
        // gets no rect, or a click lands on whatever is painted there instead.
        let (scroll, height) =
            sp::input_window(source_area, rows, app.input_cursor, app.learn.is_some());
        for i in scroll..(scroll + height).min(rows) {
            let y = list_y + (i - scroll) as u16;
            layout
                .input_item_rects
                .push((i, Rect::new(src_inner.x, y, src_inner.width, 1)));
            layout
                .input_mark_rects
                .push((i, Rect::new(src_inner.x, y, 2, 1)));
        }
    }

    // Device rows of the OUT drawer, same deal against `draw_output_panel`.
    layout.output_item_rects.clear();
    if app.out_open {
        let out_inner = inner(output_area);
        let list_y = out_inner.y + views::drawer::OUTPUT_LIST_TOP as u16;
        let rows = app.out_targets().len();
        let height = views::drawer::list_height(output_area, views::drawer::OUTPUT_LIST_TOP, 0);
        let scroll = views::drawer::list_scroll(app.out_cursor, rows, height);
        for i in scroll..(scroll + height).min(rows) {
            let y = list_y + (i - scroll) as u16;
            layout
                .output_item_rects
                .push((i, Rect::new(out_inner.x, y, out_inner.width, 1)));
        }
    }

    // Everything inside the RACK panel (tabs, mixer, buttons, FX slots,
    // knobs, slot controls) is recorded by `draw_fx_chain_panel` itself.

    let tr_inner = Rect::new(
        transport_area.x + 1,
        transport_area.y + 1,
        transport_area.width.saturating_sub(2),
        transport_area.height.saturating_sub(2),
    );
    let mut tr_x = tr_inner.x + 2;
    let play_w = 12u16;
    layout.play_btn_rect = Rect::new(tr_x, tr_inner.y + 1, play_w, 1);
    tr_x += play_w + 2;
    let stop_w = 10u16;
    layout.stop_btn_rect = Rect::new(tr_x, tr_inner.y + 1, stop_w, 1);
}

// ─── Transport ────────────────────────────────────────────────────────────────

fn draw_transport(f: &mut Frame, app: &App, area: Rect) {
    let is_focused = app.focus == Focus::Transport;

    let border_style = if is_focused {
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(views::theme::border())
    };

    let block = Block::default()
        .title(format!(" {} ", i18n::t("TRANSPORT")))
        .title_style(Style::default().fg(HEADER).add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_style(border_style)
        .style(views::theme::panel_style());

    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.height < 3 {
        return;
    }

    // Every button on this row sits at a fixed offset, and a narrow terminal
    // puts those offsets past the panel — which ratatui answers with a panic,
    // so a window dragged narrow took the whole application with it. What fits
    // is drawn; what does not is left out, and the rect handed back is the part
    // that is really on screen so the mouse agrees with the picture.
    let fit = move |r: Rect| -> Option<Rect> {
        let right = inner.x + inner.width;
        if r.x >= right || r.y >= inner.y + inner.height {
            return None;
        }
        Some(Rect::new(r.x, r.y, r.width.min(right - r.x), r.height))
    };

    // Row 1: buttons centered
    let btn_row = inner.y + 1;

    // Play button
    let play_bg = if app.playing {
        OK
    } else {
        Color::Rgb(20, 60, 30)
    };
    let play_fg = if app.playing { Color::Black } else { DIM };
    let play_label = "[ ▶ PLAY ]";

    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            play_label,
            Style::default()
                .fg(play_fg)
                .bg(play_bg)
                .add_modifier(Modifier::BOLD),
        )))
        .style(views::theme::panel_style()),
        match fit(Rect::new(inner.x + 2, btn_row, play_label.len() as u16, 1)) {
            Some(r) => r,
            None => return,
        },
    );

    // Stop button
    let stop_bg = if !app.playing {
        ERR
    } else {
        Color::Rgb(50, 20, 20)
    };
    let stop_fg = if !app.playing { Color::Black } else { DIM };
    let stop_label = " [ ■ STOP ]";

    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            stop_label,
            Style::default()
                .fg(stop_fg)
                .bg(stop_bg)
                .add_modifier(Modifier::BOLD),
        )))
        .style(views::theme::panel_style()),
        match fit(Rect::new(inner.x + 16, btn_row, stop_label.len() as u16, 1)) {
            Some(r) => r,
            None => return,
        },
    );

    // Automation: arm it here, because it only means anything while the
    // transport rolls — which is the thing this panel is about.
    let rec_on = app.automation.recording;
    let rec_label = if rec_on {
        " [ \u{25CF} REC ]"
    } else {
        " [ \u{25CB} REC ]"
    };
    let (rec_bg, rec_fg) = if rec_on {
        (ERR, Color::Black)
    } else if app.automation.is_empty() {
        (Color::Rgb(40, 28, 28), DIM)
    } else {
        // Lanes exist and are playing back: not armed, but not idle either.
        (Color::Rgb(60, 48, 20), Color::Rgb(230, 200, 120))
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            rec_label,
            Style::default()
                .fg(rec_fg)
                .bg(rec_bg)
                .add_modifier(Modifier::BOLD),
        )))
        .style(views::theme::panel_style()),
        match fit(Rect::new(inner.x + 28, btn_row, rec_label.chars().count() as u16, 1)) {
            Some(r) => r,
            None => return,
        },
    );

    // How long the automation loop is, in bars — the one number a lane is
    // measured against, and until now a constant nobody could reach.
    let bars = app.automation_loop_bars();
    let loop_label = format!(" [ \u{25C0} LOOP {bars:>2} \u{25B6} ]");
    let Some(loop_rect) = fit(Rect::new(
        inner.x + 40,
        btn_row,
        loop_label.chars().count() as u16,
        1,
    )) else {
        return;
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            loop_label,
            Style::default()
                .fg(Color::Rgb(230, 200, 120))
                .bg(Color::Rgb(40, 40, 48))
                .add_modifier(Modifier::BOLD),
        )))
        .style(views::theme::panel_style()),
        loop_rect,
    );
    app.layout.borrow_mut().loop_rect = Some(loop_rect);

    // Follow an outside MIDI clock. It lives here because this is the panel
    // about the clock, and it has to be **a switch**: a port that sends clock
    // all day would otherwise take the tempo over the moment it is plugged in,
    // which is not a thing to discover mid-set.
    let clk_label = if app.midi_clock() {
        " [ CLK EXT \u{25CF} ]"
    } else {
        " [ CLK INT \u{25CB} ]"
    };
    let Some(clk_rect) = fit(Rect::new(
        loop_rect.x + loop_rect.width,
        btn_row,
        clk_label.chars().count() as u16,
        1,
    )) else {
        return;
    };
    let (clk_bg, clk_fg) = if app.midi_clock() {
        (Color::Rgb(56, 200, 100), Color::Black)
    } else {
        (Color::Rgb(40, 40, 48), DIM)
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            clk_label,
            Style::default()
                .fg(clk_fg)
                .bg(clk_bg)
                .add_modifier(Modifier::BOLD),
        )))
        .style(views::theme::panel_style()),
        clk_rect,
    );
    app.layout.borrow_mut().clock_rect = Some(clk_rect);

    // Row 2: status text
    let status_y = btn_row + 1;
    let lanes = app
        .automation
        .lanes
        .iter()
        .filter(|l| !l.points.is_empty())
        .count();
    let auto = match (rec_on, lanes) {
        (true, _) => "  \u{25CF} REC".to_string(),
        (false, 0) => String::new(),
        (false, n) => format!("  {n} lane(s) [R]=rec [X]=clear"),
    };
    let state_text = if app.playing {
        format!("  \u{25B6} PLAYING  |  [Space]=pause  [S]=stop  [P]=panic{auto}")
    } else {
        format!("  \u{25A0} STOPPED  |  [Space]=play  [S]=stop  [P]=panic{auto}")
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            state_text,
            Style::default().fg(HINT),
        )))
        .style(views::theme::panel_style()),
        Rect::new(inner.x + 2, status_y, inner.width.saturating_sub(4), 1),
    );

    // Row 3: audio output device.
    if inner.height < 4 {
        return;
    }
    let device = app
        .audio_engine
        .as_ref()
        .and_then(|e| e.output_device())
        .unwrap_or("default");
    let out_rect = Rect::new(inner.x + 2, status_y + 1, inner.width.saturating_sub(4), 1);
    app.layout.borrow_mut().out_device_rect = Some(out_rect);
    // Which pair the active tab plays out of, so the routing is visible
    // without opening the drawer.
    let pair = app
        .slots
        .get(app.active_slot)
        .map(|s| format!("  \u{2192} {}/{}", s.out_pair.0 + 1, s.out_pair.1 + 1))
        .unwrap_or_default();
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!("  {}  ", i18n::t("OUT")),
                Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                device.to_string(),
                Style::default().fg(views::theme::text()),
            ),
            Span::styled(pair, Style::default().fg(HEADER)),
            // A mix that reaches no device is silence, and silence looks
            // exactly like every other thing that can be wrong. Said here
            // because this is the line that names the output.
            Span::styled(
                // No engine at all counts too: a stream that never opened is
                // the loudest version of "nothing is coming out".
                match app.audio_engine.as_ref() {
                    Some(e) if e.output_wired() => "",
                    _ => "  NOT CONNECTED",
                },
                Style::default().fg(WARN).add_modifier(Modifier::BOLD),
            ),
            // Past full scale the device clips, and a hard clip is the worst
            // sound a mixer can make. Nothing inside choz can fix a level, so
            // it says so instead of quietly making the best of it.
            Span::styled(
                if choz_engine::meter::meter().clipping() > 0 {
                    "  CLIP"
                } else {
                    ""
                },
                Style::default().fg(WARN).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  [o/F3=OUT]", Style::default().fg(HINT)),
        ]))
        .style(views::theme::panel_style()),
        out_rect,
    );
}

fn draw_menu_bar(f: &mut Frame, app: &App, area: Rect) {
    use menu::MenuKind;
    f.render_widget(Block::default().style(views::theme::panel_style()), area);

    let mut rects = Vec::new();
    let mut spans = Vec::new();
    let mut x = area.x;
    let open_kind = app.menu.map(|m| m.kind);
    for k in MenuKind::ALL {
        let label = k.label();
        let w = label.len() as u16;
        let style = if open_kind == Some(*k) {
            Style::default()
                .fg(Color::Black)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(HEADER)
                .bg(PANEL_BG)
                .add_modifier(Modifier::BOLD)
        };
        spans.push(Span::styled(label, style));
        rects.push(Rect::new(x, area.y, w, 1));
        x += w;
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);

    // The mode switch, hard right: it decides what every note in the rack does,
    // so it belongs where it is always visible rather than inside a menu.
    let mode = app.ui.rack_mode;
    let (live, multi) = (
        format!(" {} ", settings::RackMode::Live.label()),
        format!(" {} ", settings::RackMode::Multi.label()),
    );
    let w = (live.chars().count() + multi.chars().count()) as u16;
    if area.width > w + 2 {
        let sx = area.x + area.width - w - 1;
        let on = Style::default()
            .fg(Color::Black)
            .bg(ACCENT)
            .add_modifier(Modifier::BOLD);
        let off = Style::default()
            .fg(Color::Rgb(150, 155, 165))
            .bg(Color::Rgb(40, 46, 56));
        let is_live = mode == settings::RackMode::Live;
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(live.clone(), if is_live { on } else { off }),
                Span::styled(multi.clone(), if is_live { off } else { on }),
            ])),
            Rect::new(sx, area.y, w, 1),
        );
        app.layout.borrow_mut().mode_switch_rect = Some(Rect::new(sx, area.y, w, 1));

        // The click, next to the switch that decides what the rack does with a
        // note: both are things you reach for without looking. The arrow opens
        // its menu — tempo, time signature, sound — underneath it.
        let met_on = choz_engine::metronome::metronome().on();
        let met = format!(" \u{2669} {} ", i18n::t("MET"));
        let arrow = "\u{25BE} ";
        let mw = (met.chars().count() + arrow.chars().count()) as u16;
        if sx > area.x + mw {
            let mx = sx - mw;
            let style = if met_on {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Rgb(230, 200, 120))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(Color::Rgb(150, 155, 165))
                    .bg(Color::Rgb(40, 46, 56))
            };
            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(met.clone(), style),
                    Span::styled(arrow, style),
                ])),
                Rect::new(mx, area.y, mw, 1),
            );
            let mut layout = app.layout.borrow_mut();
            layout.met_rect = Some(Rect::new(mx, area.y, met.chars().count() as u16, 1));
            layout.met_menu_rect = Some(Rect::new(
                mx + met.chars().count() as u16,
                area.y,
                arrow.chars().count() as u16,
                1,
            ));
        }
    } else {
        let mut layout = app.layout.borrow_mut();
        layout.mode_switch_rect = None;
        layout.met_rect = None;
        layout.met_menu_rect = None;
    }
    app.layout.borrow_mut().menu_bar_rects = rects;
}

fn draw_menu_dropdown(f: &mut Frame, app: &App, state: menu::MenuState, menubar_area: Rect) {
    // Horizontal offset = sum of label widths before the open menu.
    let mut x = menubar_area.x;
    for k in menu::MenuKind::ALL {
        if *k == state.kind {
            break;
        }
        x += k.label().len() as u16;
    }
    let items = state.kind.items();
    let w = state.kind.width().max(12);
    let h = items.len() as u16 + 2;
    let popup = Rect::new(x, menubar_area.y + 1, w, h);
    draw_modal_shadow(f, popup, f.area());
    f.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .style(views::theme::overlay_style());
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let mut item_rects = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let y = inner.y + i as u16;
        if item.separator {
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "─".repeat(inner.width as usize),
                    Style::default().fg(DIM),
                ))),
                Rect::new(inner.x, y, inner.width, 1),
            );
            continue;
        }
        let selected = i == state.cursor;
        let st = if selected {
            Style::default()
                .fg(Color::Black)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White).bg(PANEL_BG)
        };
        // The bar's own labels were translated and the items under them were
        // not, so every dropdown opened in English whatever the language said.
        let label = i18n::t(item.label);
        let pad =
            (inner.width as usize).saturating_sub(label.chars().count() + item.shortcut.len() + 1);
        let text = format!(" {}{}{} ", label, " ".repeat(pad.max(1)), item.shortcut);
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(text, st))),
            Rect::new(inner.x, y, inner.width, 1),
        );
        item_rects.push((i, Rect::new(inner.x, y, inner.width, 1)));
    }
    app.layout.borrow_mut().menu_item_rects = item_rects;
}

/// The plugin rescan's progress box. Drawn from [`App::scan`], so it is up for
/// exactly as long as the thread is running.
/// "Loading <name>…", drawn on the frame before the load blocks the thread.
///
/// ponytail: a box and a name, no spinner. Nothing can animate it — the thread
/// that would tick it is the thread doing the loading — and a frozen spinner
/// says less than a sentence that was true when it was drawn.
fn draw_loading(f: &mut Frame, name: &str, area: Rect) {
    let text = format!(" {} {name}\u{2026} ", i18n::t("Loading"));
    let w = (text.chars().count() as u16 + 4).min(area.width);
    let h = 3u16.min(area.height);
    let popup = Rect::new(
        area.x + (area.width.saturating_sub(w)) / 2,
        area.y + (area.height.saturating_sub(h)) / 2,
        w,
        h,
    );
    draw_modal_shadow(f, popup, area);
    f.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .style(views::theme::overlay_style());
    let inner = block.inner(popup);
    f.render_widget(block, popup);
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            text,
            Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
        ))),
        inner,
    );
}

fn draw_scan_progress(f: &mut Frame, job: &ScanJob, area: Rect) {
    f.render_widget(Clear, area);
    f.render_widget(Block::default().style(Style::default().bg(BACKDROP)), area);

    let popup = centered_rect(60, 22, area);
    draw_modal_shadow(f, popup, area);
    f.render_widget(Clear, popup);
    let block = Block::default()
        .title(format!(" {} ", i18n::t("Rescanning plugin paths")))
        .title_style(Style::default().fg(HEADER).add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .style(views::theme::overlay_style());
    let inner = block.inner(popup);
    f.render_widget(block, popup);
    if inner.height < 3 || inner.width < 8 {
        return;
    }

    // `total` is 0 until the first message arrives; showing 0 % beats dividing
    // by it.
    let pct = match job.total {
        0 => 0,
        t => (job.done * 100 / t) as u16,
    };
    let bar_w = inner.width.saturating_sub(2) as usize;
    let filled = bar_w * pct as usize / 100;
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("\u{2588}".repeat(filled), Style::default().fg(ACCENT)),
            Span::styled("\u{2591}".repeat(bar_w - filled), Style::default().fg(DIM)),
        ])),
        Rect::new(inner.x + 1, inner.y, inner.width.saturating_sub(2), 1),
    );
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("{pct:>3}%   {} / {}", job.done, job.total),
            Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
        ))),
        Rect::new(inner.x + 1, inner.y + 1, inner.width.saturating_sub(2), 1),
    );
    // The directory in flight, tail-first: the end of a long path is the part
    // that says which one it is.
    if inner.height >= 3 {
        let w = inner.width.saturating_sub(2) as usize;
        let n = job.label.chars().count();
        let shown: String = match n > w {
            true => format!(
                "\u{2026}{}",
                job.label.chars().skip(n - w + 1).collect::<String>()
            ),
            false => job.label.clone(),
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(shown, Style::default().fg(DIM)))),
            Rect::new(inner.x + 1, inner.y + 2, inner.width.saturating_sub(2), 1),
        );
    }
}

fn draw_about(f: &mut Frame, app: &App, area: Rect) {
    f.render_widget(Clear, area);
    f.render_widget(Block::default().style(Style::default().bg(BACKDROP)), area);

    let popup = centered_rect(55, 65, area);
    draw_modal_shadow(f, popup, area);
    f.render_widget(Clear, popup);
    let block = Block::default()
        .title(" ABOUT choz ")
        .title_style(Style::default().fg(HEADER).add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .style(views::theme::overlay_style());
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    // Close button [×] + hit rect.
    let close_x = popup.x + popup.width.saturating_sub(4);
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "[×]",
            Style::default().fg(ERR).add_modifier(Modifier::BOLD),
        ))),
        Rect::new(close_x, popup.y, 3, 1),
    );
    app.layout.borrow_mut().about_close_rect = Some(Rect::new(close_x, popup.y, 3, 1));

    // Logo image (ratatui-image) fills the top; text below.
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(6)])
        .split(inner);
    if let Some(ref protocol) = app.logo {
        f.render_widget(ratatui_image::Image::new(protocol), rows[0]);
    } else {
        f.render_widget(
            Paragraph::new("choz").style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            rows[0],
        );
    }
    let text = vec![
        Line::from(Span::styled(
            "choz v0.1",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "Terminal audio plugin host — Carla for the terminal.",
            Style::default().fg(DIM),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "FX chain · WAV/SF2 · CLAP · MIDI",
            Style::default().fg(Color::White),
        )),
        Line::from(Span::styled("Esc / [×] to close", Style::default().fg(DIM))),
    ];
    f.render_widget(
        Paragraph::new(text).style(views::theme::panel_style()),
        rows[1].inner(Margin {
            vertical: 0,
            horizontal: 1,
        }),
    );
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn draw_modal_shadow(f: &mut Frame, modal: Rect, screen: Rect) {
    let sx = (modal.x + 1).min(screen.x + screen.width.saturating_sub(1));
    let sy = (modal.y + 1).min(screen.y + screen.height.saturating_sub(1));
    let sw = modal.width.min(screen.width - (sx - screen.x));
    let sh = modal.height.min(screen.height - (sy - screen.y));

    if sw > 0 && sh > 0 {
        f.render_widget(
            Block::default().style(Style::default().bg(SHADOW)),
            Rect::new(sx, sy, sw, sh),
        );
    }
}

#[cfg(test)]
mod tests {

    /// The monitor must never eat into the rack, and must vanish rather than
    /// render a useless sliver.
    #[test]
    fn midi_monitor_only_takes_the_space_left_over() {
        assert_eq!(
            super::monitor_rows(19),
            0,
            "2 spare rows is not worth a panel"
        );
        assert_eq!(super::monitor_rows(20), 3, "border plus one message");
        assert_eq!(super::monitor_rows(23), 6);
        assert_eq!(super::monitor_rows(25), 8, "capped");
        assert_eq!(
            super::monitor_rows(60),
            8,
            "a tall terminal grows the rack, not the log"
        );
        assert_eq!(
            super::monitor_rows(0),
            0,
            "no underflow on a degenerate size"
        );
    }
    use super::*;
    use choz_engine::input::InputSource;
    use ratatui::{backend::TestBackend, Terminal};

    /// OSC mixer control, end to end up to the pixels: apply the message and
    /// render the RACK panel, because "the message arrived" was already proven
    /// but "the strip redraws" never was (a pty scrape can't see it — ratatui
    /// only repaints changed cells).
    #[test]
    fn osc_mix_control_shows_up_in_the_rendered_mixer_strip() {
        use choz_engine::input::ControlMsg;

        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.active_slot = 1;

        // 1-based on the wire: tab 2 is slot 1.
        app.apply_control(ControlMsg::Gain {
            tab: 2,
            value: 0.25,
        });
        app.apply_control(ControlMsg::Pan {
            tab: 2,
            value: -0.8,
        });
        app.apply_control(ControlMsg::Mute { tab: 2, on: true });
        assert_eq!(app.slots[0].gain, 1.0, "tab 1 is untouched");

        let mut term = Terminal::new(TestBackend::new(80, 12)).unwrap();
        term.draw(|f| {
            let mix = app
                .slots
                .get(app.active_slot)
                .map(|s| (s.gain, s.pan, s.mute, s.solo));
            let tabs: Vec<String> = app.slots.iter().map(tab_label).collect();
            views::fx_chain_panel::draw_fx_chain_panel(
                f,
                f.area(),
                &app.fx_chain,
                app.fx_slot,
                app.fx_param,
                true,
                &tabs,
                app.active_slot,
                mix,
                &app.instrument_label(),
                None,
                false,
                false,
                Default::default(),
                Default::default(),
                app.tab_channel(),
                app.pitch_to_midi_state(),
                app.slots
                    .get(app.active_slot)
                    .map(|s| s.pitch_mix)
                    .unwrap_or(1.0),
                &app.instr_knobs(),
                app.instr_param,
                app.rack_focus == RackFocus::Instrument,
                app.in_trim_state(),
                None,
                crate::arp::ArpView::default(),
                Default::default(),
            );
        })
        .unwrap();

        let screen: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(screen.contains("0.25"), "gain not drawn:\n{screen}");
        assert!(screen.contains("L80"), "pan not drawn:\n{screen}");
    }

    /// Language and text colour are process-wide (the draw code reads them from
    /// globals), so the test that switches them and the tests that render have
    /// to take turns.
    fn ui_guard() -> views::theme::UiGuard {
        // The lock lives in `theme` so the panels' own tests can take it too.
        views::theme::ui_guard()
    }

    /// Puts the global language, colour and desktop flag back to their defaults
    /// on drop —
    /// including when the test panics, which would otherwise leave every other
    /// rendering test reading a foreign language.
    struct UiRestore;

    impl Drop for UiRestore {
        fn drop(&mut self) {
            i18n::set_language(i18n::Lang::En);
            views::theme::set_has_desktop(false);
            views::theme::set_text_color(ratatui::style::Color::Rgb(
                settings::PALETTE[0].1 .0,
                settings::PALETTE[0].1 .1,
                settings::PALETTE[0].1 .2,
            ));
        }
    }

    /// Open Settings on AUDIO → Plugin Paths, with the cursor in the directory
    /// list (the modal itself opens on the Engine section).
    fn open_paths_tab(app: &mut App) {
        app.open_paths_modal();
        {
            let m = app.modal.as_mut().unwrap();
            m.list.filter = TAB_AUDIO;
            m.list.sidebar_cursor = SEC_PATHS;
            m.list.sidebar_focused = false;
        }
        app.refresh_modal();
    }

    /// Point the state dir at a temp directory: these tests save the plugin
    /// paths, which must never touch the user's real config.
    fn sandbox_state_dir() {
        let tmp = std::env::temp_dir().join(format!("choz_ui_state_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);
        unsafe { std::env::set_var("XDG_STATE_HOME", &tmp) };
        // …and start from defaults. The directory is per process, not per test,
        // and `App::new()` reads `ui.json` from it: one test that sets a desktop
        // image and saves leaves the next one starting with that image, which
        // is a failure that only shows up in some orders. The env var is
        // process-global, so a directory per test would not help either — the
        // fix is that a sandbox means "nothing carried over".
        let _ = std::fs::remove_file(tmp.join("ui.json"));
    }

    /// A rack tab holding a plugin instrument with two parameters.
    /// A plugin that publishes no programs of its own (Surge XT's VST3 build
    /// reports zero and keeps its 637 factory patches as `.fxp` files) gets its
    /// bank from a folder: point at the folder once, and every patch under it
    /// is in the same picker — and on the same `◀` `▶` buttons, and therefore
    /// on whatever CC is learned on them — as a plugin's own programs.
    #[test]
    fn a_folder_of_patch_files_becomes_the_tabs_bank() {
        // The round trip at the end applies the project's language and theme
        // process-wide, like a real load does.
        let _g = ui_guard();
        let _restore = UiRestore;
        let mut app = app_with_plugin_tab();
        assert!(
            app.slots[0].plugin_presets.is_empty(),
            "the plugin publishes no programs"
        );

        let dir = std::env::temp_dir().join(format!("choz_uibank_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Leads")).unwrap();
        let patch = b"the patch".to_vec();
        let mut fxp = vec![0u8; 60];
        fxp[0..4].copy_from_slice(b"CcnK");
        fxp[8..12].copy_from_slice(b"FPCh");
        fxp[56..60].copy_from_slice(&(patch.len() as u32).to_be_bytes());
        fxp.extend_from_slice(&patch);
        std::fs::write(dir.join("Init.fxp"), &fxp).unwrap();
        std::fs::write(dir.join("Leads/Tok.fxp"), &fxp).unwrap();

        // The browser picks the folder itself, not a file in it.
        app.open_bank_browser();
        assert_eq!(app.modal.as_ref().unwrap().kind, ModalKind::Bank);
        app.set_bank_dir(dir.clone());

        assert_eq!(
            app.preset_labels(0),
            vec!["Init".to_string(), "Leads · Tok".to_string()],
            "the whole tree is the bank, filed by sub-folder"
        );
        assert_eq!(
            app.modal.as_ref().map(|m| m.kind),
            Some(ModalKind::Preset),
            "and the patch list opens on top of the folder picker"
        );

        // Picking one restores its patch onto the tab — with no engine here,
        // what is checked is the blob the tab keeps (the same one the project
        // saves, and the one a rebuilt engine slot is handed back).
        app.slots[0].preset_cursor = 1;
        app.apply_selected_preset();
        assert_eq!(app.slots[0].instr_state, patch);
        assert_eq!(app.active_preset_label().as_deref(), Some("Leads · Tok"));

        // The bank survives a save/load round trip: it is a folder on disk, and
        // re-reading it is cheaper than storing 637 paths in the project.
        app.source = app.slots[0].source.clone();
        let saved = app.project_snapshot();
        let mut fresh = App::new();
        // The rack alone: applying the whole project would set the language and
        // the theme **process-wide**, and other tests are reading those while
        // this one runs.
        fresh.load_rack_only = true;
        fresh.apply_project(saved);
        assert_eq!(fresh.slots[0].preset_dir.as_deref(), Some(dir.as_path()));
        assert_eq!(fresh.preset_labels(0).len(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn app_with_plugin_tab() -> App {
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Plugin {
            id: "com.example.synth".into(),
            format: "CLAP".into(),
            name: "Example".into(),
        }));
        app.slots[0].instr_params = vec![
            choz_engine::PluginParam {
                id: 0,
                name: "Cutoff".into(),
                min: 20.0,
                max: 20_000.0,
                default: 20.0,
                ..Default::default()
            },
            choz_engine::PluginParam {
                id: 1,
                name: "Resonance".into(),
                min: 0.0,
                max: 1.0,
                default: 0.0,
                ..Default::default()
            },
        ];
        app.slots[0].instr_values = vec![0.0, 0.0];
        app.focus = Focus::FxChain;
        app
    }

    fn render_modal(app: &mut App, w: u16, h: u16) -> String {
        let _g = ui_guard();
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        let mut modal = app.modal.take().expect("a modal must be open");
        term.draw(|f| {
            let rects = views::modal::draw_list_modal(f, &mut modal.list, f.area(), (80, 80));
            app.layout.borrow_mut().modal_rects = rects;
        })
        .unwrap();
        app.modal = Some(modal);
        term.backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    /// The INSTR editor lists the plugin's own parameter names and values, and
    /// the arrows move the value of the row it points at.
    #[test]
    fn instrument_param_modal_draws_plugin_names_and_edits_values() {
        let mut app = app_with_plugin_tab();

        handle_fx_keys(&mut app, KeyCode::Char('p'));
        assert_eq!(
            app.modal.as_ref().map(|m| m.kind),
            Some(ModalKind::InstrParams)
        );

        // Right moves the selected parameter up by one step, and it clamps at 0.
        handle_modal_key(&mut app, KeyCode::Right);
        assert_eq!(app.slots[0].instr_values[0], INSTR_STEP);
        handle_modal_key(&mut app, KeyCode::Left);
        handle_modal_key(&mut app, KeyCode::Left);
        assert_eq!(app.slots[0].instr_values[0], 0.0, "values clamp at 0");
        app.set_instr_param(0, 0.5);
        app.refresh_modal();

        let screen = render_modal(&mut app, 80, 20);
        assert!(
            screen.contains("Cutoff"),
            "plugin param name missing:\n{screen}"
        );
        assert!(
            screen.contains("Resonance"),
            "second param missing:\n{screen}"
        );
        // Half of 20..20000 in plain units.
        assert!(screen.contains("10010"), "plain value missing:\n{screen}");
    }

    /// A tab is labelled with the format that actually loaded it.
    #[test]
    fn a_plugin_tab_is_labelled_with_its_own_format() {
        for (format, expect) in [("LV2", "LV2:Yoshimi"), ("SFZ", "SFZ:Yoshimi")] {
            let source = AudioSource::Plugin {
                id: "x".into(),
                format: format.into(),
                name: "Yoshimi".into(),
            };
            assert_eq!(slot_label(&source), expect);
        }
    }

    /// SFZ is not a plugin ABI, but it loads through the same picker entry —
    /// and must not be labelled "(not hosted yet)" any more.
    #[test]
    fn sfz_instruments_are_offered_as_loadable() {
        let mut app = App::new();
        app.plugins.push(choz_engine::FoundPlugin {
            format: choz_engine::PluginFormat::Sfz,
            id: String::new(),
            name: "Saw".into(),
            path: "/lib/Saw.sfz".into(),
            is_instrument: true,
        });
        let choice = app
            .source_choices()
            .into_iter()
            .find(|c| c.label.starts_with("Saw"))
            .expect("SFZ instrument missing from the picker");
        assert!(!choice.label.contains("not hosted"), "{}", choice.label);
        match choice.action {
            SourceAction::Plugin {
                format, ref path, ..
            } => {
                assert_eq!(format, choz_engine::PluginFormat::Sfz);
                assert_eq!(path, std::path::Path::new("/lib/Saw.sfz"));
            }
            other => panic!("SFZ should load like a plugin, got {other:?}"),
        }
    }

    /// The SOURCE picker filters by format, and every format choz can't host
    /// yet simply comes up empty instead of pretending.
    #[test]
    fn source_modal_filters_by_format() {
        let mut app = App::new();
        app.plugins.push(choz_engine::FoundPlugin {
            format: choz_engine::PluginFormat::Clap,
            id: "com.example.synth".into(),
            name: "Example Synth".into(),
            path: "/tmp/example.clap".into(),
            is_instrument: true,
        });
        app.open_source_modal();

        let screen = render_modal(&mut app, 90, 24);
        assert!(
            screen.contains("Example Synth"),
            "CLAP instrument missing:\n{screen}"
        );
        assert!(
            screen.contains("SELECT") && screen.contains("CANCEL"),
            "buttons missing"
        );

        // VST3 is offered as a filter but hosts nothing yet.
        let vst3 = SOURCE_FORMATS.iter().position(|f| *f == "VST3").unwrap();
        app.modal.as_mut().unwrap().list.filter = vst3;
        app.refresh_modal();
        assert!(
            app.modal.as_ref().unwrap().list.items.is_empty(),
            "VST3 isn't hosted yet"
        );

        // Back to CLAP: the instrument is there again.
        let clap = SOURCE_FORMATS.iter().position(|f| *f == "CLAP").unwrap();
        app.modal.as_mut().unwrap().list.filter = clap;
        app.refresh_modal();
        assert_eq!(app.modal.as_ref().unwrap().list.items.len(), 1);
        assert!(app.modal.as_ref().unwrap().list.items[0].contains("Example Synth"));
    }

    /// A click on a modal row selects it; clicking the same row again (or the
    /// SELECT button) confirms, and CANCEL closes without applying.
    #[test]
    fn modal_rows_and_buttons_respond_to_the_mouse() {
        let mut app = app_with_plugin_tab();
        app.open_instr_modal();
        render_modal(&mut app, 80, 20);

        let (row1, cancel) = {
            let l = app.layout.borrow();
            (l.modal_rects.rows[1].1, l.modal_rects.cancel.unwrap())
        };
        let click = |x: u16, y: u16| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y,
            modifiers: crossterm::event::KeyModifiers::NONE,
        };
        handle_modal_mouse(&mut app, click(row1.x + 1, row1.y));
        assert_eq!(
            app.modal.as_ref().unwrap().list.cursor,
            1,
            "clicking a row selects it"
        );

        handle_modal_mouse(&mut app, click(cancel.x + 1, cancel.y));
        assert!(app.modal.is_none(), "CANCEL closes the modal");
    }

    /// A plugin with more parameters than the box can show pages, and the CCs
    /// learned on the box page with it: the fader that was on the first knob of
    /// the page is on the first knob of the next one, driving a different
    /// parameter. Without that, a synth like Surge XT gives a desk of faders
    /// the first dozen of its three hundred parameters and nothing else.
    #[test]
    fn paging_the_instrument_box_carries_the_learned_ccs_with_it() {
        let mut app = app_with_plugin_tab();
        app.slots[0].instr_params = (0..60)
            .map(|i| choz_engine::PluginParam::plain_range(i, format!("P{i}"), 0.0, 1.0, 0.0))
            .collect();
        app.slots[0].instr_values = vec![0.0; 60];
        app.rack_focus = RackFocus::Instrument;

        let (_, layout) = render_rack(&mut app, 120, 30);
        let page = layout.instr_knobs.len();
        assert!(page > 0 && page < 60, "the box has to be paging: {page}");
        let first = layout.instr_knobs[0].0;

        // A fader on the first knob of the page.
        app.learn = Some(LearnTarget::InstrParam {
            slot: 0,
            param: first,
        });
        app.feed_cc(74, 100);
        assert_eq!(
            app.cc_pairs(),
            vec![(
                74,
                LearnTarget::InstrParam {
                    slot: 0,
                    param: first
                }
            )]
        );

        handle_fx_keys(&mut app, KeyCode::PageDown);
        let (_, layout) = render_rack(&mut app, 120, 30);
        let moved = layout.instr_knobs[0].0;
        assert!(moved > first, "the box did not page: {first} -> {moved}");
        assert_eq!(
            app.cc_pairs(),
            vec![(
                74,
                LearnTarget::InstrParam {
                    slot: 0,
                    param: moved
                }
            )],
            "the CC has to land on the first knob of the new page"
        );

        // And back, onto the parameter it started on.
        handle_fx_keys(&mut app, KeyCode::PageUp);
        render_rack(&mut app, 120, 30);
        assert_eq!(
            app.cc_pairs(),
            vec![(
                74,
                LearnTarget::InstrParam {
                    slot: 0,
                    param: first
                }
            )]
        );

        // The CC now moves the parameter it points at, not the one it learned.
        handle_fx_keys(&mut app, KeyCode::PageDown);
        render_rack(&mut app, 120, 30);
        app.feed_cc(74, 127);
        assert_eq!(app.slots[0].instr_values[moved], 1.0);
        assert_eq!(app.slots[0].instr_values[first], 0.0);
    }

    /// And the CCs follow the box **however** it moved. Paging is one way; the
    /// arrows walking the cursor off the bottom of the box scrolls it just the
    /// same, and a controller that only re-addressed on `PgUp` / `PgDn` would
    /// be pointing at the wrong parameters from the first arrow press.
    #[test]
    fn the_learned_ccs_follow_the_knob_box_however_it_scrolled() {
        let mut app = app_with_plugin_tab();
        app.slots[0].instr_params = (0..60)
            .map(|i| choz_engine::PluginParam::plain_range(i, format!("P{i}"), 0.0, 1.0, 0.0))
            .collect();
        app.slots[0].instr_values = vec![0.0; 60];
        app.rack_focus = RackFocus::Instrument;

        let (_, layout) = render_rack(&mut app, 120, 30);
        let page = layout.instr_knobs.len();
        assert!(page > 0 && page < 60, "the box has to be paging: {page}");
        let first = layout.instr_knobs[0].0;
        app.learn = Some(LearnTarget::InstrParam {
            slot: 0,
            param: first,
        });
        app.feed_cc(74, 100);

        // Walk the cursor down until the box scrolls — no paging involved.
        let moved = loop {
            handle_fx_keys(&mut app, KeyCode::Down);
            let (_, layout) = render_rack(&mut app, 120, 30);
            let start = layout.instr_knobs[0].0;
            if start != first {
                break start;
            }
            assert!(app.instr_param < 59, "the box never scrolled");
        };

        assert_eq!(
            app.cc_pairs(),
            vec![(
                74,
                LearnTarget::InstrParam {
                    slot: 0,
                    param: first + (moved - first)
                }
            )],
            "the CC has to move with the window, not only with the pager"
        );
        app.feed_cc(74, 127);
        assert_eq!(app.slots[0].instr_values[moved], 1.0);
        assert_eq!(app.slots[0].instr_values[first], 0.0);
    }

    /// MIDI learn: arm a target, and the next CC binds to it; after that the
    /// same CC drives the control.
    #[test]
    fn midi_learn_binds_a_cc_then_drives_the_fader() {
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.focus = Focus::FxChain;

        handle_fx_keys(&mut app, KeyCode::Char('l'));
        assert_eq!(app.modal.as_ref().map(|m| m.kind), Some(ModalKind::Learn));
        // First entry is the active tab's VOL.
        handle_modal_key(&mut app, KeyCode::Enter);
        assert_eq!(app.learn, Some(LearnTarget::Gain(0)));

        // The next CC binds; it must not also move the fader.
        app.feed_cc(74, 127);
        assert_eq!(app.cc_pairs(), vec![(74, LearnTarget::Gain(0))]);
        assert_eq!(
            app.slots[0].gain, 1.0,
            "the binding message itself doesn't move it"
        );
        assert!(app.learn.is_none(), "learn disarms after binding");

        // Now it drives the fader; an unbound CC does nothing.
        app.feed_cc(74, 64);
        assert!((app.slots[0].gain - 64.0 / 127.0 * MAX_GAIN).abs() < 1e-6);
        let gain = app.slots[0].gain;
        app.feed_cc(9, 0);
        assert_eq!(app.slots[0].gain, gain, "an unbound CC is ignored");
    }

    /// Any parameter of any hosted plugin is bindable, not only the FX chain's:
    /// `l` in the INSTRUMENT editor learns the row under the cursor.
    #[test]
    fn midi_learn_binds_a_cc_to_an_instrument_parameter() {
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.slots[0].instr_params = vec![
            choz_engine::PluginParam {
                id: 0,
                name: "Cutoff".into(),
                min: 0.0,
                max: 1.0,
                default: 0.0,
                ..Default::default()
            },
            choz_engine::PluginParam {
                id: 1,
                name: "Reso".into(),
                min: 0.0,
                max: 1.0,
                default: 0.0,
                ..Default::default()
            },
        ];
        app.slots[0].instr_values = vec![0.0, 0.0];

        app.open_instr_modal();
        handle_modal_key(&mut app, KeyCode::Down);
        handle_modal_key(&mut app, KeyCode::Char('l'));
        assert_eq!(
            app.learn,
            Some(LearnTarget::InstrParam { slot: 0, param: 1 })
        );
        assert!(app.modal.is_none(), "learning closes the editor");

        app.feed_cc(74, 127);
        assert_eq!(
            app.cc_pairs(),
            vec![(74, LearnTarget::InstrParam { slot: 0, param: 1 })]
        );
        assert_eq!(
            app.slots[0].instr_values[1], 0.0,
            "the binding message doesn't move it"
        );

        app.feed_cc(74, 64);
        assert!((app.slots[0].instr_values[1] - 64.0 / 127.0).abs() < 1e-6);
        assert_eq!(
            app.slots[0].instr_values[0], 0.0,
            "the other parameter is untouched"
        );
    }

    /// One fader, two FX units: the selected unit owns it, so the same CC can be
    /// re-learned per effect instead of the second binding stealing the first.
    #[test]
    fn the_same_fader_binds_per_fx_unit_and_only_the_selected_one_moves() {
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.fx_chain
            .push(AudioFxEntry::new(source::AudioFxKind::Reverb));
        app.fx_chain
            .push(AudioFxEntry::new(source::AudioFxKind::Delay));

        // CC 7 → FX 1 param 0, then the *same* CC → FX 2 param 0.
        app.fx_slot = 0;
        app.learn = Some(LearnTarget::FxParam {
            slot: 0,
            fx: 0,
            param: 0,
        });
        app.feed_cc(7, 0);
        app.fx_slot = 1;
        app.learn = Some(LearnTarget::FxParam {
            slot: 0,
            fx: 1,
            param: 0,
        });
        app.feed_cc(7, 0);
        assert_eq!(
            app.cc_bindings.len(),
            2,
            "the second unit stole the binding: {:?}",
            app.cc_bindings
        );

        // FX 2 is selected, so only FX 2 follows the fader.
        let untouched = app.fx_chain[0].params[0];
        app.feed_cc(7, 127);
        assert_eq!(
            app.fx_chain[0].params[0], untouched,
            "the unselected unit moved"
        );
        assert_eq!(app.fx_chain[1].params[0], 1.0);

        // Select FX 1 and the same fader now drives it instead.
        app.fire_trigger(TriggerAction::FxSelect(0));
        let held = app.fx_chain[1].params[0];
        app.feed_cc(7, 64);
        assert!((app.fx_chain[0].params[0] - 64.0 / 127.0).abs() < 1e-6);
        assert_eq!(app.fx_chain[1].params[0], held, "the unselected unit moved");

        // A non-FX target on that CC is still one-CC-one-control.
        app.learn = Some(LearnTarget::Gain(0));
        app.feed_cc(7, 0);
        assert_eq!(app.cc_pairs(), vec![(7, LearnTarget::Gain(0))]);
    }

    /// A bound CC arriving over the real MIDI channel has to move the drawn
    /// mixer strip, not just `slots[i].gain` — CCs are also forwarded to the
    /// instrument, so "it sounds louder" is not proof the fader followed.
    #[test]
    fn a_bound_cc_moves_the_drawn_fader() {
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.cc_bindings.push(CcBinding {
            source: None,
            cc: 74,
            target: LearnTarget::Gain(0),
        });

        app.note_tx
            .send(midi::InputEvent::Cc(choz_engine::input::CcMsg {
                channel: 0,
                source: InputSource::Keyboard,
                cc: 74,
                value: 32,
            }))
            .unwrap();
        app.drain_midi();

        let (screen, _) = render_rack(&mut app, 100, 30);
        assert!(
            screen.contains("0.50"),
            "the strip still shows the old gain:\n{screen}"
        );
    }

    /// What a Keystation button actually sends: bank select, then a program
    /// change. Neither may steal an armed *fader* learn, and an unbound program
    /// change must leave the preset alone — the controller doesn't pick sounds.
    #[test]
    fn an_unbound_controller_button_leaves_the_preset_alone() {
        use choz_engine::input::{CcMsg, ProgramMsg};

        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Sf2 {
            path: std::path::PathBuf::from("/nonexistent.sf2"),
            bank: 0,
            preset: 0,
        }));
        app.source = app.slots[0].source.clone();
        app.learn = Some(LearnTarget::Gain(0));

        let src = InputSource::Keyboard; // routes to the active slot, no port needed
        for e in [
            midi::InputEvent::Cc(CcMsg {
                source: src,
                channel: 0,
                cc: 32,
                value: 0,
            }),
            midi::InputEvent::Cc(CcMsg {
                source: src,
                channel: 0,
                cc: 0,
                value: 0,
            }),
            midi::InputEvent::Program(ProgramMsg {
                source: src,
                bank: 0,
                program: 13,
            }),
        ] {
            app.note_tx.send(e).unwrap();
        }
        app.drain_midi();

        assert!(
            app.cc_bindings.is_empty(),
            "bank select stole the binding: {:?}",
            app.cc_bindings
        );
        assert_eq!(
            app.learn,
            Some(LearnTarget::Gain(0)),
            "learn stays armed for the real fader"
        );
        assert!(
            matches!(
                app.slots[0].source,
                AudioSource::Sf2 {
                    preset: 0,
                    bank: 0,
                    ..
                }
            ),
            "the button picked a preset by itself: {:?}",
            app.slots[0].source,
        );

        // The fader that follows is what gets bound.
        app.note_tx
            .send(midi::InputEvent::Cc(CcMsg {
                source: src,
                channel: 0,
                cc: 74,
                value: 100,
            }))
            .unwrap();
        app.drain_midi();
        assert_eq!(app.cc_pairs(), vec![(74, LearnTarget::Gain(0))]);
    }

    /// Two buttons, learned by the user, step the preset; every other button on
    /// the controller stays silent.
    #[test]
    fn a_learned_program_change_button_steps_the_preset() {
        use choz_engine::input::ProgramMsg;

        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Sf2 {
            path: std::path::PathBuf::from("/nonexistent.sf2"),
            bank: 0,
            preset: 0,
        }));
        app.slots[0].presets = vec![
            choz_engine::sources::Sf2Preset {
                bank: 0,
                preset: 0,
                name: "a".into(),
            },
            choz_engine::sources::Sf2Preset {
                bank: 0,
                preset: 7,
                name: "b".into(),
            },
        ];
        app.source = app.slots[0].source.clone();

        let src = InputSource::Keyboard;
        let press = |app: &mut App, program| {
            app.note_tx
                .send(midi::InputEvent::Program(ProgramMsg {
                    source: src,
                    bank: 0,
                    program,
                }))
                .unwrap();
            app.drain_midi();
        };

        // Arm learn on BANK ▶, then press the button that should drive it.
        app.learn = Some(LearnTarget::Trigger(TriggerAction::PresetNext));
        press(&mut app, 13);
        assert_eq!(
            app.pc_bindings,
            vec![(13, LearnTarget::Trigger(TriggerAction::PresetNext))]
        );
        assert_eq!(app.learn, None);
        assert_eq!(
            app.slots[0].preset_cursor, 0,
            "binding must not press the button"
        );

        press(&mut app, 13);
        assert_eq!(
            app.slots[0].preset_cursor, 1,
            "the bound button steps the preset"
        );

        press(&mut app, 42);
        assert_eq!(
            app.slots[0].preset_cursor, 1,
            "an unbound button does nothing"
        );
    }

    /// Draw the RACK panel over a test backend and return (screen, rects).
    fn render_rack(app: &mut App, w: u16, h: u16) -> (String, RackLayout) {
        let _g = ui_guard();
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        let mut rack = RackLayout::default();
        let preset = app.active_preset_label();
        term.draw(|f| {
            let mix = app
                .slots
                .get(app.active_slot)
                .map(|s| (s.gain, s.pan, s.mute, s.solo));
            let tabs: Vec<String> = app.slots.iter().map(tab_label).collect();
            rack = views::fx_chain_panel::draw_fx_chain_panel(
                f,
                f.area(),
                &app.fx_chain,
                app.fx_slot,
                app.fx_param,
                true,
                &tabs,
                app.active_slot,
                mix,
                &app.instrument_label(),
                preset.as_deref(),
                app.has_editor(),
                app.has_fx_editor(),
                app.sbx_state(None),
                app.sbx_state(Some(app.fx_slot)),
                app.tab_channel(),
                app.pitch_to_midi_state(),
                app.slots
                    .get(app.active_slot)
                    .map(|s| s.pitch_mix)
                    .unwrap_or(1.0),
                &app.instr_knobs(),
                app.instr_param,
                app.rack_focus == RackFocus::Instrument,
                app.in_trim_state(),
                None,
                app.slots
                    .get(app.active_slot)
                    .map(|s| arp::ArpView {
                        cursor: app.arp_param,
                        focused: app.rack_focus == RackFocus::Arp,
                        ..s.arp.view()
                    })
                    .unwrap_or_default(),
                app.fx_slot_info(),
            );
        })
        .unwrap();
        let screen = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        let mut layout = app.layout.borrow_mut();
        layout.rack = rack.clone();
        // The mouse router checks the panel area before the rack rects.
        layout.fx_chain_area = ratatui::layout::Rect::new(0, 0, w, h);
        drop(layout);
        app.sync_instr_window();
        (screen, rack)
    }

    /// Three or more faders in a row sharing a unit are one thing — an ADSR,
    /// a bank of band gains — and get drawn as a bank of vertical bars, so the
    /// shape is read in one look instead of four numbers.
    #[test]
    fn a_run_of_faders_with_one_unit_is_drawn_as_a_bank() {
        use source::ParamShape;
        use views::fx_chain_panel::{fader_groups, vertical_bar};

        let ms = || ParamShape::Fader("ms".into());
        let adsr = vec![ms(), ms(), ms(), ms()];
        assert_eq!(fader_groups(&adsr), vec![true; 4]);

        // Two is not a bank, and a different unit ends the run.
        assert_eq!(fader_groups(&[ms(), ms()]), vec![false, false]);
        assert_eq!(
            fader_groups(&[ms(), ms(), ParamShape::Fader("%".into()), ms()]),
            vec![false, false, false, false],
        );
        // A knob in the middle splits it, whatever the units say.
        assert_eq!(
            fader_groups(&[ms(), ms(), ParamShape::Continuous, ms(), ms(), ms()]),
            vec![false, false, false, true, true, true],
        );

        // The bar grows upward and fills the bottom cell first, so a row of
        // them draws a profile.
        let (top, bottom) = vertical_bar(0.0, 3);
        assert_eq!((top.as_str(), bottom.as_str()), ("   ", "   "));
        let (top, bottom) = vertical_bar(0.25, 3);
        assert_eq!(top, "   ", "a quarter is all in the bottom cell");
        assert_ne!(bottom, "   ");
        let (top, bottom) = vertical_bar(1.0, 3);
        assert_eq!(
            (top.as_str(), bottom.as_str()),
            ("\u{2588}\u{2588}\u{2588}", "\u{2588}\u{2588}\u{2588}")
        );
        assert_eq!(
            vertical_bar(0.5, 3).1,
            "\u{2588}\u{2588}\u{2588}",
            "half fills the bottom cell"
        );
    }

    /// The arc has to resolve finer than a cell, or a knob nudged by a hair
    /// looks identical — which is what an eight-cell bar with one position per
    /// cell did.
    #[test]
    fn the_knob_arc_resolves_finer_than_a_whole_cell() {
        use views::fx_chain_panel::{fader_track, knob_arc};
        let arc = |v: f32| knob_arc(v, 8);
        assert_eq!(arc(0.0).chars().count(), 8, "always the same width");
        assert_eq!(arc(1.0), "\u{2588}".repeat(8), "full is full");
        // Eight cells at eight positions each: 65 distinct pictures instead of
        // the 9 a whole-cell bar could draw.
        let distinct: std::collections::HashSet<String> =
            (0..=64).map(|i| arc(i as f32 / 64.0)).collect();
        assert_eq!(distinct.len(), 65, "one picture per eighth of a cell");
        assert_ne!(
            arc(0.50),
            arc(0.52),
            "a nudge inside one cell still moves it"
        );

        // The fader is a travel: the track never changes, only the handle.
        assert_eq!(fader_track(0.0, 10).chars().next(), Some('\u{25AE}'));
        assert_eq!(fader_track(1.0, 10).chars().last(), Some('\u{25AE}'));
        assert_eq!(
            fader_track(0.5, 10)
                .chars()
                .filter(|c| *c == '\u{25AE}')
                .count(),
            1
        );
        assert_eq!(fader_track(0.5, 10).chars().count(), 10);
    }

    /// The long form of the same three controls: a checkbox where the RACK
    /// draws a button (one column instead of a box, which is what a list of
    /// forty rows can afford), the step's name where it draws arrows, and the
    /// plugin's own unit next to a continuous value.
    #[test]
    fn the_instrument_list_draws_a_checkbox_a_name_and_a_unit() {
        use source::ParamShape;
        let toggle = choz_engine::PluginParam {
            id: 0,
            name: "Sync".into(),
            min: 0.0,
            max: 1.0,
            default: 0.0,
            steps: 2,
            ..Default::default()
        };
        assert!(instr_param_row(&toggle, 0.0).contains("[ ] OFF"));
        assert!(instr_param_row(&toggle, 1.0).contains("[x] ON"));

        let wave = choz_engine::PluginParam {
            id: 1,
            name: "Wave".into(),
            min: 0.0,
            max: 2.0,
            default: 0.0,
            steps: 3,
            points: vec![
                (0.0, "Sine".into()),
                (1.0, "Saw".into()),
                (2.0, "Square".into()),
            ],
            ..Default::default()
        };
        let row = instr_param_row(&wave, 1.0);
        assert!(
            row.contains("Square"),
            "the last step at the top of the range: {row}"
        );
        assert!(row.contains("3/3"), "and which one it is: {row}");

        // A time is a distance covered, so it gets a fader — and that comes
        // from the plugin's unit, not from its name.
        let time = choz_engine::PluginParam {
            id: 3,
            name: "Delay".into(),
            min: 0.0,
            max: 2.0,
            default: 0.0,
            unit: Some("s".into()),
            ..Default::default()
        };
        assert_eq!(ParamShape::of(&time), ParamShape::Fader("s".into()));
        let row = instr_param_row(&time, 0.5);
        assert!(
            row.contains('\u{25AE}'),
            "the handle sits on the track: {row}"
        );
        assert!(row.contains("1.000 s"));

        // A continuous parameter keeps the arc, and says what the number means.
        let cutoff = choz_engine::PluginParam {
            id: 2,
            name: "Cutoff".into(),
            min: 20.0,
            max: 20_000.0,
            default: 20.0,
            unit: Some("Hz".into()),
            ..Default::default()
        };
        let row = instr_param_row(&cutoff, 0.0);
        assert!(
            row.contains("20.000 Hz"),
            "value in the plugin's own units: {row}"
        );
        assert!(
            !instr_param_row(&toggle, 0.0).contains("Hz"),
            "no unit, nothing invented"
        );
    }

    /// The bank on screen: four times in a row draw as vertical bars, and every
    /// one of them keeps its own click rect — the mouse and MIDI learn work off
    /// those, so a new control that forgets them is a control nobody can bind.
    #[test]
    fn an_envelope_draws_as_a_bank_and_stays_clickable() {
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        let time = |id: u32, name: &str| choz_engine::PluginParam {
            id,
            name: name.into(),
            min: 0.0,
            max: 1000.0,
            default: 0.0,
            unit: Some("ms".into()),
            ..Default::default()
        };
        app.slots[0].instr_params = vec![
            time(0, "Attack"),
            time(1, "Decay"),
            time(2, "Sustain"),
            time(3, "Release"),
        ];
        app.slots[0].instr_values = vec![0.1, 0.4, 0.9, 0.6];
        app.rack_focus = RackFocus::Instrument;

        let (screen, layout) = render_rack(&mut app, 120, 30);
        assert!(
            screen.contains('\u{2588}'),
            "a full bar for the loud one: {screen}"
        );
        assert!(
            screen.contains("Attack") && screen.contains("Release"),
            "names stay"
        );
        assert_eq!(
            layout.instr_knobs.len(),
            4,
            "one rect per parameter, bank or not"
        );

        // Clicking the third bar selects the third parameter.
        let rect = layout
            .instr_knobs
            .iter()
            .find(|(i, _)| *i == 2)
            .map(|(_, r)| *r)
            .unwrap();
        let layout = app.layout.borrow().clone();
        let action = mouse_action(
            rect.x + 1,
            rect.y + 1,
            &layout,
            MouseEventKind::Down(MouseButton::Left),
        );
        assert!(
            matches!(action, MouseAction::InstrParamSel(2)),
            "clicking the third bar has to select the third parameter"
        );
    }

    /// A parameter is drawn as what it *is*: a switch reads on or off, a
    /// parameter with named steps reads its name, and anything the plugin says
    /// nothing about stays the arc it always was.
    #[test]
    fn a_switch_and_a_named_step_are_not_drawn_as_knobs() {
        use source::ParamShape;

        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.slots[0].instr_params = vec![
            choz_engine::PluginParam {
                id: 0,
                name: "Sync".into(),
                min: 0.0,
                max: 1.0,
                default: 0.0,
                steps: 2,
                ..Default::default()
            },
            choz_engine::PluginParam {
                id: 1,
                name: "Wave".into(),
                min: 0.0,
                max: 2.0,
                default: 0.0,
                steps: 3,
                points: vec![
                    (0.0, "Sine".into()),
                    (1.0, "Saw".into()),
                    (2.0, "Square".into()),
                ],
                ..Default::default()
            },
            choz_engine::PluginParam {
                id: 2,
                name: "Cutoff".into(),
                min: 0.0,
                max: 1.0,
                default: 0.0,
                ..Default::default()
            },
        ];
        app.slots[0].instr_values = vec![0.0, 0.0, 0.5];
        app.rack_focus = RackFocus::Instrument;

        let (screen, _) = render_rack(&mut app, 120, 30);
        assert!(screen.contains("OFF"), "a switch at 0 says so: {screen}");
        assert!(
            screen.contains("\u{25C0}Sine\u{25B6}"),
            "the step's own name, with arrows"
        );
        assert!(screen.contains("1/3"), "and which of them it is");
        assert!(screen.contains("\u{2591}"), "the knob keeps its arc");

        // The arrows move a stepped parameter one position, not one twentieth
        // of its range.
        adjust_fx_param(&mut app, 0.05);
        assert_eq!(
            app.slots[0].instr_values[0], 1.0,
            "one press flips the switch"
        );
        adjust_fx_param(&mut app, 0.05);
        assert_eq!(app.slots[0].instr_values[0], 1.0, "and it stops there");
        let (screen, _) = render_rack(&mut app, 120, 30);
        assert!(screen.contains(" ON"), "which the panel shows");

        app.instr_param = 1;
        adjust_fx_param(&mut app, 0.05);
        assert_eq!(
            app.slots[0].instr_values[1], 0.5,
            "the middle of three steps"
        );
        let (screen, _) = render_rack(&mut app, 120, 30);
        assert!(screen.contains("\u{25C0}Saw\u{25B6}"), "the second name");

        // A continuous parameter is nudged, not stepped.
        app.instr_param = 2;
        adjust_fx_param(&mut app, 0.05);
        assert!((app.slots[0].instr_values[2] - 0.55).abs() < 1e-6);

        // And what the shape says about a plugin's parameter is only ever what
        // the plugin reported.
        assert_eq!(
            ParamShape::of(&app.slots[0].instr_params[0]),
            ParamShape::Toggle
        );
        assert_eq!(
            ParamShape::of(&app.slots[0].instr_params[1]),
            ParamShape::Named(vec![
                (0.0, "Sine".into()),
                (0.5, "Saw".into()),
                (1.0, "Square".into()),
            ]),
        );
        assert_eq!(
            ParamShape::of(&app.slots[0].instr_params[2]),
            ParamShape::Continuous
        );
    }

    /// An FX with more knobs than fit across the panel wraps onto further rows,
    /// and every knob drawn is clickable at the position it was drawn.
    #[test]
    fn wide_fx_wraps_its_knobs_onto_more_rows() {
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        // Z5 Texture has 16 parameters — far more than one row of knobs.
        app.fx_chain
            .push(AudioFxEntry::new(source::AudioFxKind::Z5Texture));
        let n = app.fx_chain[0].param_descs().len();
        assert!(n > 6, "this test needs a wide FX, got {n} params");

        let (screen, rack) = render_rack(&mut app, 100, 30);
        assert!(
            rack.params.len() > 7,
            "only {} knobs drawn",
            rack.params.len()
        );
        // Knobs on the second row sit strictly below the first row's.
        let first_y = rack.params[0].1.y;
        assert!(
            rack.params.iter().any(|(_, r)| r.y > first_y),
            "nothing wrapped:\n{screen}"
        );
        // The slot controls kept their own box below the knobs.
        assert!(
            screen.contains("SLOT") && screen.contains("DEL"),
            "slot box missing:\n{screen}"
        );
        let del = rack.del.expect("DEL is clickable");
        assert!(del.y > first_y, "DEL must sit below the knob grid");
    }

    /// The RACK offers "run this plugin in its own process", the click sticks,
    /// and the button says which way it is set.
    #[test]
    fn the_sandbox_button_toggles_the_plugin_preference() {
        // `set_forced` writes the real state dir otherwise.
        sandbox_state_dir();
        let mut app = app_with_plugin_tab();
        app.source = app.slots[0].source.clone();
        app.synths.push(SynthEntry {
            id: "com.example.synth".into(),
            format: choz_engine::PluginFormat::Clap,
            name: "Example".into(),
            path: std::path::PathBuf::from("/tmp/example.clap"),
        });
        let (format, path, id) = app.plugin_ref(None).expect("the tab holds a plugin");
        choz_engine::quarantine::set_forced(format, &path, &id, false);

        let (screen, rack) = render_rack(&mut app, 120, 30);
        assert!(screen.contains("SBX"), "no sandbox button drawn:\n{screen}");
        let btn = rack
            .buttons
            .iter()
            .find(|(b, _)| *b == RackButton::Sandbox)
            .map(|(_, r)| *r)
            .expect("the sandbox button is clickable");

        click(&mut app, btn.x, btn.y);
        assert!(
            choz_engine::quarantine::forced(format, &path, &id),
            "clicking it must ask for the sandbox"
        );
        // Asking is not the same as running: nothing reloads without an engine.
        let state = app.sbx_state(None);
        assert!(state.on && !state.live);
        assert!(views::fx_chain_panel::sbx_label(state).contains("reload"));

        click(&mut app, btn.x, btn.y);
        assert!(
            !choz_engine::quarantine::forced(format, &path, &id),
            "clicking again must take it back"
        );

        // A tab with no plugin has no button at all.
        let mut plain = App::new();
        plain.slots.push(RackSlot::new(AudioSource::Midi));
        let (_, rack) = render_rack(&mut plain, 120, 30);
        assert!(!rack.buttons.iter().any(|(b, _)| *b == RackButton::Sandbox));
    }

    /// The chain buttons wrap instead of running off the right edge.
    #[test]
    fn fx_chain_buttons_wrap_to_the_next_line() {
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        for kind in [
            source::AudioFxKind::Protocosmos,
            source::AudioFxKind::SpaceEcho,
            source::AudioFxKind::ReverseDelay,
            source::AudioFxKind::Z5Texture,
            source::AudioFxKind::Compressor,
        ] {
            app.fx_chain.push(AudioFxEntry::new(kind));
        }
        let (_, rack) = render_rack(&mut app, 46, 32);
        let rows: std::collections::BTreeSet<u16> =
            rack.fx_slots.iter().map(|(_, r)| r.y).collect();
        assert!(
            rows.len() > 1,
            "a narrow panel must wrap the chain onto more lines"
        );
        for &(_, r) in rack.fx_slots.iter() {
            assert!(
                r.x + r.width <= 46,
                "a chain button ran off the panel: {r:?}"
            );
        }
    }

    /// MIDI learn by pointer: arm it, click a fader, then the next CC binds and
    /// the pointer mode ends.
    #[test]
    fn pointer_learn_picks_the_clicked_control_then_binds_the_cc() {
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.fx_chain
            .push(AudioFxEntry::new(source::AudioFxKind::Delay));
        let (_, rack) = render_rack(&mut app, 100, 30);

        app.learn_pick = true; // start_learn_pick without touching the terminal
        let gain = rack.gain.expect("the VOL cell is clickable");
        let click = |x: u16, y: u16| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y,
            modifiers: crossterm::event::KeyModifiers::NONE,
        };
        handle_mouse(&mut app, click(gain.x + 1, gain.y));
        assert_eq!(app.learn, Some(LearnTarget::Gain(0)));
        assert!(
            app.learn_pick,
            "the ? pointer stays up while it listens for a CC"
        );
        assert_eq!(
            app.slots[0].gain, 1.0,
            "the pick click must not move the fader"
        );

        app.feed_cc(21, 127);
        assert_eq!(app.cc_pairs(), vec![(21, LearnTarget::Gain(0))]);
        assert!(
            !app.learn_pick && app.learn.is_none(),
            "learn ends once bound"
        );

        // A knob can be picked the same way.
        app.learn_pick = true;
        let (_, param_rect) = rack.params[1];
        handle_mouse(&mut app, click(param_rect.x + 1, param_rect.y + 1));
        assert_eq!(
            app.learn,
            Some(LearnTarget::FxParam {
                slot: 0,
                fx: 0,
                param: 1
            })
        );
    }

    /// The bank arrows step the SoundFont program and the current one is named
    /// on screen; a CC bound to the arrow does exactly the same thing.
    #[test]
    fn bank_arrows_change_the_preset_and_can_be_midi_bound() {
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Sf2 {
            path: "/tmp/x.sf2".into(),
            bank: 0,
            preset: 0,
        }));
        app.slots[0].presets = vec![
            sources::Sf2Preset {
                bank: 0,
                preset: 0,
                name: "Grand Piano".into(),
            },
            sources::Sf2Preset {
                bank: 0,
                preset: 1,
                name: "Bright Piano".into(),
            },
        ];
        app.source = app.slots[0].source.clone();

        let (screen, rack) = render_rack(&mut app, 100, 30);
        assert!(
            screen.contains("Grand Piano"),
            "the current bank must be named:\n{screen}"
        );
        let next = rack
            .buttons
            .iter()
            .find(|(b, _)| *b == RackButton::PresetNext)
            .map(|&(_, r)| r)
            .expect("the ▶ button is drawn for a SoundFont tab");

        let click = |x: u16, y: u16| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y,
            modifiers: crossterm::event::KeyModifiers::NONE,
        };
        handle_mouse(&mut app, click(next.x + 1, next.y));
        assert_eq!(app.slots[0].preset_cursor, 1, "▶ steps to the next program");
        assert_eq!(
            app.active_preset_label().as_deref(),
            Some("000:001 Bright Piano")
        );

        // The same button is MIDI-learnable: pick it with the pointer, bind a CC.
        app.learn_pick = true;
        handle_mouse(&mut app, click(next.x + 1, next.y));
        assert_eq!(
            app.learn,
            Some(LearnTarget::Trigger(TriggerAction::PresetNext))
        );
        assert_eq!(
            app.slots[0].preset_cursor, 1,
            "picking must not press the button"
        );
        app.feed_cc(30, 127);
        assert_eq!(
            app.cc_pairs(),
            vec![(30, LearnTarget::Trigger(TriggerAction::PresetNext))]
        );

        // Buttons fire on the rising edge only.
        app.slots[0].preset_cursor = 0;
        app.feed_cc(30, 10);
        assert_eq!(
            app.slots[0].preset_cursor, 0,
            "below half-scale does nothing"
        );
        app.feed_cc(30, 127);
        assert_eq!(
            app.slots[0].preset_cursor, 1,
            "crossing half-scale presses it"
        );
        app.feed_cc(30, 120);
        assert_eq!(app.slots[0].preset_cursor, 1, "held high doesn't retrigger");
    }

    /// The arpeggiator takes the keys instead of the instrument, and its own
    /// clock plays them. Off, a note reaches the tab exactly as it always did —
    /// which is the property that makes it safe to have at all.
    #[test]
    fn the_arpeggiator_takes_the_held_keys_and_plays_a_pattern() {
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.active_slot = 0;

        // Off: nothing is held by the arpeggiator, and ticking it is silent.
        assert!(!app.slots[0].arp.is_on());
        assert!(!app.arps_running());

        app.edit_arp(ArpEdit::Toggle);
        assert!(app.slots[0].arp.is_on(), "A turns it on");

        let t = std::time::Instant::now();
        for n in [60, 64, 67] {
            app.slots[0].arp.note_on(n, 100, t);
        }
        assert!(app.arps_running(), "the event loop has to come back sooner");

        // The pattern is the chord, in order, on its own clock.
        let mut played = Vec::new();
        for i in 0..3 {
            let mut out = Vec::new();
            app.slots[0]
                .arp
                .tick(t + std::time::Duration::from_millis(125 * i), &mut out);
            played.extend(out.into_iter().filter_map(|e| match e {
                arp::ArpEvent::On { note, .. } => Some(note),
                _ => None,
            }));
        }
        assert_eq!(played, vec![60, 64, 67]);

        // Turning it off releases what it was holding rather than leaving a
        // note sounding that nothing else would ever stop.
        app.edit_arp(ArpEdit::Toggle);
        let mut out = Vec::new();
        app.slots[0]
            .arp
            .tick(t + std::time::Duration::from_millis(500), &mut out);
        assert!(out.is_empty(), "already stopped by the toggle: {out:?}");
    }

    /// **Enter turns the arpeggiator off as well as on.**
    ///
    /// Its switch is the first knob of the box, and Enter used to *nudge* that
    /// knob up — which on something with two positions means "on" and then
    /// "on" again. Reported as exactly that: no way to stop it from the
    /// keyboard. A switch is pressed, not nudged; the other knobs still step.
    #[test]
    fn enter_flips_the_arpeggiators_switch_both_ways() {
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.active_slot = 0;
        app.rack_focus = RackFocus::Arp;
        // The switch is the first knob; the cursor starts there.
        app.arp_param = 0;
        assert_eq!(app.arp_knobs()[0].1, "ARP");
        assert!(!app.slots[0].arp.is_on());

        handle_fx_keys(&mut app, KeyCode::Enter);
        assert!(app.slots[0].arp.is_on(), "Enter starts it");

        handle_fx_keys(&mut app, KeyCode::Enter);
        assert!(!app.slots[0].arp.is_on(), "and Enter stops it");

        // And nothing else about Enter moved: on a knob whose positions have
        // names it still opens the list rather than flipping anything.
        app.edit_arp(ArpEdit::Toggle);
        let mode = app
            .arp_knobs()
            .iter()
            .position(|(p, ..)| *p == arp::ArpParam::Mode)
            .expect("the mode knob is in the box");
        app.arp_param = mode;
        handle_fx_keys(&mut app, KeyCode::Enter);
        assert_eq!(
            app.modal.as_ref().map(|m| m.kind),
            Some(ModalKind::ArpChoice),
            "a list of names opens as a list"
        );
    }

    /// The buttons on the ARP line are drawn where they are clicked, and only
    /// the switch is drawn while it is off.
    #[test]
    fn the_arp_line_shows_its_settings_only_when_it_is_on() {
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.active_slot = 0;

        let (screen, rack) = render_rack(&mut app, 120, 14);
        assert!(screen.contains("ARP"), "the switch is always there");
        let arp_buttons = |rack: &RackLayout| {
            rack.buttons
                .iter()
                .filter(|(b, _)| {
                    !matches!(
                        b,
                        RackButton::Channel
                            | RackButton::Source
                            | RackButton::Preset
                            | RackButton::Learn
                            | RackButton::PitchToMidi
                            | RackButton::Gui
                            | RackButton::Sandbox
                            | RackButton::PresetPrev
                            | RackButton::PresetNext
                    )
                })
                .count()
        };
        assert_eq!(arp_buttons(&rack), 1, "off, it is one switch");

        app.edit_arp(ArpEdit::Toggle);
        // Short on rows, so the controls are buttons: on a panel with room they
        // are knobs instead, which is what
        // `the_arp_controls_take_the_shape_the_screen_can_afford` covers.
        let (screen, rack) = render_rack(&mut app, 120, 14);
        assert!(arp_buttons(&rack) > 5, "on, the settings are on the line");
        assert!(screen.contains("1/16"), "the division: {screen}");
        assert!(screen.contains("120 BPM"), "the rate: {screen}");

        // And clicking one of them changes the setting it names.
        let div = rack
            .buttons
            .iter()
            .find(|(b, _)| *b == RackButton::ArpDiv)
            .map(|&(_, r)| r)
            .expect("the division button is drawn");
        handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: div.x + 1,
                row: div.y,
                modifiers: crossterm::event::KeyModifiers::NONE,
            },
        );
        // It opens the list of divisions rather than walking them: eight
        // positions is a menu, and on this shape the button is the only way in.
        let modal = app.modal.as_ref().expect("the division list opened");
        assert_eq!(modal.kind, ModalKind::ArpChoice);
        assert_eq!(modal.list.items.len(), arp::TimeDiv::ALL.len());
        app.modal.as_mut().unwrap().list.cursor = 5;
        app.modal_select();
        app.close_modal();
        assert_eq!(
            app.slots[0].arp.settings.div,
            arp::TimeDiv::SixteenthTriplet
        );
    }

    /// Every cell of a bank arrow presses it, in every language. The rects used
    /// to start at a hardcoded offset that only matched the English label, so in
    /// Spanish (`BANCO`) the arrows answered one column to the left of where
    /// they were painted — half the button was dead and the cell next to it
    /// fired.
    #[test]
    fn the_whole_bank_arrow_is_clickable_after_the_label_is_translated() {
        // The language is process-global, so hold the UI lock across the whole
        // test — `ui_guard` is reentrant, `render_rack` takes it again.
        let _g = ui_guard();
        let _restore = UiRestore;
        i18n::set_language(i18n::Lang::Es);

        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Sf2 {
            path: "/tmp/x.sf2".into(),
            bank: 0,
            preset: 0,
        }));
        app.slots[0].presets = (0..4)
            .map(|i| sources::Sf2Preset {
                bank: 0,
                preset: i,
                name: format!("Preset {i}"),
            })
            .collect();
        app.source = app.slots[0].source.clone();

        let (_, rack) = render_rack(&mut app, 100, 30);
        let next = rack
            .buttons
            .iter()
            .find(|(b, _)| *b == RackButton::PresetNext)
            .map(|&(_, r)| r)
            .expect("the ▶ button is drawn for a SoundFont tab");

        for dx in 0..next.width {
            app.slots[0].preset_cursor = 0;
            handle_mouse(
                &mut app,
                MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: next.x + dx,
                    row: next.y,
                    modifiers: crossterm::event::KeyModifiers::NONE,
                },
            );
            assert_eq!(
                app.slots[0].preset_cursor, 1,
                "column {dx} of the ▶ button did not press it"
            );
        }
    }

    /// The SLOT box offers factory presets for a built-in that ships them, the
    /// button opens the list, and picking one moves the knobs it names — through
    /// `set_fx_param`, so the dry/wet and the rebuild flag come along.
    #[test]
    fn a_factory_preset_is_a_click_away_and_moves_the_knobs_it_names() {
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.fx_chain
            .push(AudioFxEntry::new(source::AudioFxKind::Delay));
        let (_, rack) = render_rack(&mut app, 110, 32);
        let button = rack
            .fx_preset
            .expect("the delay ships presets, so the button is drawn");

        handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: button.x + 1,
                row: button.y,
                modifiers: crossterm::event::KeyModifiers::NONE,
            },
        );
        let modal = app.modal.as_ref().expect("the picker opened");
        assert_eq!(modal.kind, ModalKind::FxPreset);
        let dub = modal
            .list
            .items
            .iter()
            .position(|i| i == "Dub")
            .expect("Dub is one of the delay's presets");

        app.modal.as_mut().unwrap().list.cursor = dub;
        app.modal_select();
        let entry = &app.fx_chain[0];
        let feedback = fx_presets::param_index(entry, "Feedback").unwrap();
        assert!(
            (entry.params[feedback] - 0.75).abs() < 1e-6,
            "feedback stayed at {}",
            entry.params[feedback]
        );
        assert!((entry.wet - 0.45).abs() < 1e-6, "wet is {}", entry.wet);
        assert!(app.fx_dirty, "the chain has to be rebuilt with the values");

        // A hosted plugin's knobs are its own: no button, and the picker refuses
        // to open on one.
        app.fx_chain[0] = AudioFxEntry::new_plugin(source::PluginFx {
            format: choz_engine::PluginFormat::Clap,
            path: "/nowhere.clap".into(),
            id: "x".into(),
            name: "X".into(),
            params: Vec::new(),
        });
        let (_, rack) = render_rack(&mut app, 110, 32);
        assert!(rack.fx_preset.is_none());
        assert!(!app.open_fx_presets());
    }

    /// SLOT buttons (bar DEL) are learnable, and DEL deliberately isn't.
    #[test]
    fn slot_buttons_are_learnable_except_delete() {
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.fx_chain
            .push(AudioFxEntry::new(source::AudioFxKind::Delay));
        app.fx_chain
            .push(AudioFxEntry::new(source::AudioFxKind::Reverb));
        let (_, rack) = render_rack(&mut app, 100, 32);

        let at = |app: &App, r: ratatui::layout::Rect| app.learn_target_at((r.x + 1, r.y).into());
        assert_eq!(
            at(&app, rack.on_off.unwrap()),
            Some(LearnTarget::Trigger(TriggerAction::FxToggle))
        );
        assert_eq!(
            at(&app, rack.move_right.unwrap()),
            Some(LearnTarget::Trigger(TriggerAction::FxMoveRight))
        );
        assert_eq!(
            at(&app, rack.del.unwrap()),
            None,
            "DEL must never be bindable"
        );
        // The FX CHAIN row selects slots.
        let (i, chain_rect) = rack.fx_slots[1];
        assert_eq!(
            at(&app, chain_rect),
            Some(LearnTarget::Trigger(TriggerAction::FxSelect(i)))
        );

        // Firing the toggle really flips the FX.
        app.fire_trigger(TriggerAction::FxToggle);
        assert!(!app.fx_chain[0].enabled);
    }

    /// The chain row no longer repeats the FX names as an IN → … → OUT diagram.
    #[test]
    fn the_routing_diagram_is_gone() {
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.fx_chain
            .push(AudioFxEntry::new(source::AudioFxKind::Delay));
        let (screen, _) = render_rack(&mut app, 100, 30);
        assert_eq!(
            screen.matches("DELAY").count(),
            2,
            "chain button + param box title only"
        );
        assert!(
            !screen.contains("OUT"),
            "the routing line is gone:\n{screen}"
        );
    }

    /// The two stompboxes are offered under DISTORTION and build for real.
    #[test]
    fn the_stompbox_distortions_are_offered_and_build() {
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.open_add_fx_modal();
        // The sidebar has a DISTORTION section; select it and both pedals are
        // in the list it shows.
        let dist = app
            .modal
            .as_ref()
            .unwrap()
            .list
            .sidebar
            .iter()
            .position(|(label, _)| label == "DISTORTION")
            .expect("DISTORTION section missing");
        for name in ["AMBER FANG", "VELVET FUZZ"] {
            app.modal.as_mut().unwrap().list.sidebar_cursor = dist;
            app.refresh_modal();
            let rows = app.fx_menu_rows();
            let pos = rows
                .iter()
                .position(|(_, l)| l.contains(name))
                .unwrap_or_else(|| panic!("{name} missing from the DISTORTION section"));
            app.modal.as_mut().unwrap().list.cursor = pos;
            assert!(app.modal_select());
            app.open_add_fx_modal();
        }
        assert_eq!(app.fx_chain.len(), 2);
        // The engine can build both from their specs.
        for entry in app.fx_chain.iter() {
            let spec = entry.to_spec();
            assert!(
                choz_engine::fx_chain::build_processor(&spec.kind, &spec.params, 48_000).is_some(),
                "{} has no engine-side processor",
                spec.kind
            );
        }
    }

    /// Settings is three tabs: audio, theme and language. Picking a theme sets
    /// text, frame and desktop together, and the panels draw with all three.
    #[test]
    fn settings_tabs_switch_colour_and_language() {
        use i18n::Lang;
        let _g = ui_guard();
        let _restore = UiRestore;
        sandbox_state_dir();
        let mut app = App::new();
        app.open_paths_modal();
        let tabs = app.modal.as_ref().unwrap().list.filters.clone();
        assert_eq!(tabs, vec!["AUDIO", "THEME", "LANGUAGE"]);

        // The starting background is stated rather than assumed: the settings
        // file is per process, not per test, so an image left behind by another
        // one both survives `apply_theme` (deliberately) and adds two rows to
        // the DESKTOP block, which moves every scheme row below it. It has to be
        // set *before* the rows are read, or the indices are off by two.
        app.ui.background = settings::Background::Terminal;

        // Theme tab: the desktop controls first, then the scheme list — which
        // runs to hundreds of rows, so anything below it is unreachable.
        app.modal.as_mut().unwrap().list.filter = TAB_THEME;
        app.refresh_modal();
        let items = app.modal.as_ref().unwrap().list.items.clone();
        assert_eq!(items[0], "DESKTOP", "headers label the two halves");
        let schemes = items
            .iter()
            .position(|i| i == "COLOUR SCHEME")
            .expect("second header");
        assert!(
            items[schemes + 1].contains("choz (default)"),
            "choz's own themes lead"
        );
        assert!(
            items.iter().any(|i| i.contains("Gruvbox Dark")),
            "and Gogh's follow: {} rows in all",
            items.len()
        );

        // The row right after the first scheme is the second one.
        let theme = &settings::THEMES[1];
        app.modal.as_mut().unwrap().list.cursor = schemes + 2;
        assert!(
            !app.modal_select(),
            "the modal stays open so several can be tried"
        );
        assert_eq!(app.ui.text_color, theme.text);
        assert_eq!(app.ui.border_color, Some(theme.border));
        assert_eq!(views::theme::text(), app.ui.color(), "panels draw with it");
        assert_eq!(
            views::theme::border(),
            app.ui.border(),
            "and so do the frames"
        );
        // A scheme with a desktop colour sets the background too.
        assert_eq!(
            app.ui.background,
            settings::Background::Color(theme.desktop.unwrap())
        );
        // The marker moves to the chosen row. The scheme block itself moved:
        // picking a desktop colour added the wash rows above it, so the header
        // has to be found again rather than remembered.
        app.refresh_modal();
        let items = app.modal.as_ref().unwrap().list.items.clone();
        let schemes = items
            .iter()
            .position(|i| i == "COLOUR SCHEME")
            .expect("second header");
        assert!(items[schemes + 2].contains('\u{25CF}'));

        // Back to a scheme without a desktop: the background goes with it.
        app.modal.as_mut().unwrap().list.cursor = schemes + 1;
        app.modal_select();
        assert_eq!(app.ui.background, settings::Background::Terminal);

        // Language tab: every shipped language is listed, Enter switches.
        app.modal.as_mut().unwrap().list.filter = TAB_LANG;
        app.refresh_modal();
        let items = app.modal.as_ref().unwrap().list.items.clone();
        assert_eq!(items.len(), Lang::ALL.len());
        let es = Lang::ALL.iter().position(|l| *l == Lang::Es).unwrap();
        app.modal.as_mut().unwrap().list.cursor = es;
        assert!(
            app.modal_select(),
            "SELECT switches the language and closes"
        );
        assert_eq!(app.ui.language, Lang::Es);
        assert_eq!(i18n::t("SETTINGS"), "AJUSTES", "the interface follows");
        // The tab labels themselves are translated now.
        assert_eq!(app.modal.as_ref().unwrap().list.filters[2], "IDIOMA");
        assert_eq!(
            app.modal.as_ref().unwrap().list.filters[1],
            i18n::t("THEME")
        );
        // `_restore` puts English (and the default colour) back on the way out.
    }

    /// The desktop rows: toggle a flat colour, pick an image through the file
    /// browser, cycle its fit, and clear it again.
    #[test]
    fn the_theme_tab_sets_a_desktop_image_through_the_browser() {
        let _g = ui_guard();
        let _restore = UiRestore;
        sandbox_state_dir();
        let mut app = App::new();
        app.open_paths_modal();
        app.modal.as_mut().unwrap().list.filter = TAB_THEME;
        app.refresh_modal();

        // Find the rows by meaning, not by a hard-coded index.
        let row_of = |app: &App, want: ThemeRow| -> usize {
            (0..app.modal.as_ref().unwrap().list.items.len())
                .find(|&i| app.theme_row(i) == Some(want))
                .unwrap_or_else(|| panic!("no row for {want:?}"))
        };

        // "Pick an image..." opens the browser, started in assets/ when it is there.
        let pick = row_of(&app, ThemeRow::PickImage);
        app.modal.as_mut().unwrap().list.cursor = pick;
        app.modal_select();
        assert_eq!(app.modal.as_ref().unwrap().kind, ModalKind::Wallpaper);
        let dir = app
            .modal
            .as_ref()
            .unwrap()
            .browser
            .as_ref()
            .unwrap()
            .dir
            .clone();
        if std::path::Path::new("assets").is_dir() {
            assert!(
                dir.ends_with("assets"),
                "started in the sample directory: {dir:?}"
            );
        }

        // Only images are listed — no .rs, no .toml.
        let entries: Vec<String> = app
            .modal
            .as_ref()
            .unwrap()
            .browser
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .filter(|e| !e.is_dir)
            .map(|e| e.label.clone())
            .collect();
        assert!(
            entries.iter().all(|e| file_browser::IMAGE_EXTS
                .iter()
                .any(|x| e.to_lowercase().ends_with(x))),
            "only image files: {entries:?}"
        );

        // Picking one sets it as the background and returns to the theme tab.
        let Some(img) = app
            .modal
            .as_ref()
            .unwrap()
            .browser
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .position(|e| !e.is_dir)
        else {
            eprintln!("no images in assets/; skipping the rest");
            return;
        };
        app.modal.as_mut().unwrap().list.cursor = img;
        app.modal_select();
        let settings::Background::Image { path, fit } = app.ui.background.clone() else {
            panic!("expected an image, got {:?}", app.ui.background);
        };
        assert_eq!(fit, settings::ImageFit::Stretch, "the default fit");
        assert!(path.is_file(), "{path:?}");
        assert_eq!(
            app.modal.as_ref().unwrap().kind,
            ModalKind::PluginPaths,
            "back to settings"
        );
        assert_eq!(app.settings_tab(), TAB_THEME);

        // The fit row only exists once there is an image, and Enter cycles it.
        let fit_row = row_of(&app, ThemeRow::Fit);
        app.modal.as_mut().unwrap().list.cursor = fit_row;
        app.modal_select();
        assert!(matches!(
            app.ui.background,
            settings::Background::Image {
                fit: settings::ImageFit::Tile,
                ..
            }
        ));

        // And clearing puts the terminal's own background back.
        let clear = row_of(&app, ThemeRow::Clear);
        app.modal.as_mut().unwrap().list.cursor = clear;
        app.modal_select();
        assert_eq!(app.ui.background, settings::Background::Terminal);
        assert!(app.theme_row(row_of(&app, ThemeRow::PickImage)).is_some());
    }

    /// The flow the user asked for: tick a theme, then pick a wallpaper, then
    /// leave — and the system keeps *both*.
    #[test]
    fn a_theme_and_a_wallpaper_survive_leaving_the_modal_together() {
        let _g = ui_guard();
        let _restore = UiRestore;
        sandbox_state_dir();
        let mut app = App::new();
        app.open_paths_modal();
        app.modal.as_mut().unwrap().list.filter = TAB_THEME;
        app.refresh_modal();

        let row_of = |app: &App, want: ThemeRow| -> usize {
            (0..app.modal.as_ref().unwrap().list.items.len())
                .find(|&i| app.theme_row(i) == Some(want))
                .unwrap_or_else(|| panic!("no row for {want:?}"))
        };

        // 1. Tick Obsidian. The modal stays open — it is a checkbox, not a door.
        let obsidian = settings::THEMES
            .iter()
            .position(|t| t.name == "Obsidian")
            .unwrap();
        app.modal.as_mut().unwrap().list.cursor = row_of(&app, ThemeRow::Scheme(obsidian));
        assert!(!app.modal_select(), "picking a scheme keeps the modal open");
        assert_eq!(app.ui.text_color, settings::THEMES[obsidian].text);

        // 2. Pick a wallpaper. Back on the theme tab afterwards.
        app.modal.as_mut().unwrap().list.cursor = row_of(&app, ThemeRow::PickImage);
        app.modal_select();
        assert_eq!(app.modal.as_ref().unwrap().kind, ModalKind::Wallpaper);
        let img = app
            .modal
            .as_ref()
            .unwrap()
            .browser
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .position(|e| !e.is_dir);
        let Some(img) = img else {
            eprintln!("no images in assets/; skipping");
            return;
        };
        app.modal.as_mut().unwrap().list.cursor = img;
        app.modal_select();
        assert_eq!(app.settings_tab(), TAB_THEME, "back where we were");

        // 3. "Apply and close" leaves, and the theme did not eat the wallpaper
        //    nor the wallpaper the theme.
        app.modal.as_mut().unwrap().list.cursor = row_of(&app, ThemeRow::Done);
        assert!(app.modal_select(), "SELECT closes the modal");

        assert_eq!(
            app.ui.text_color,
            settings::THEMES[obsidian].text,
            "theme kept"
        );
        assert_eq!(app.ui.border_color, Some(settings::THEMES[obsidian].border));
        let settings::Background::Image { fit, .. } = &app.ui.background else {
            panic!("wallpaper lost: {:?}", app.ui.background);
        };
        assert_eq!(*fit, settings::ImageFit::Stretch);

        // And it is on disk, so the next start comes up the same.
        let saved = settings::UiSettings::load();
        assert_eq!(saved.text_color, settings::THEMES[obsidian].text);
        assert!(matches!(
            saved.background,
            settings::Background::Image { .. }
        ));

        // Re-ticking the theme afterwards must not throw the wallpaper away.
        app.open_paths_modal();
        app.modal.as_mut().unwrap().list.filter = TAB_THEME;
        app.refresh_modal();
        app.modal.as_mut().unwrap().list.cursor = row_of(&app, ThemeRow::Scheme(obsidian));
        app.modal_select();
        assert!(
            matches!(app.ui.background, settings::Background::Image { .. }),
            "a scheme with its own desktop colour still leaves a chosen image alone"
        );
    }

    /// ADD FX has a category sidebar on the left and format chips on top: the
    /// sidebar picks what the list shows, the chips narrow both.
    #[test]
    fn add_fx_sidebar_picks_the_category_and_chips_the_format() {
        use choz_engine::PluginFormat;
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        // Two hosted plugins of different formats: the sidebar has to sort them
        // into categories guessed from their names, and the format chips have to
        // narrow both list and sidebar.
        app.fx_plugins.push(source::PluginFx {
            format: PluginFormat::Lv2,
            name: "Calf Reverb".into(),
            path: "/usr/lib/lv2/calf.lv2".into(),
            id: "urn:calf:reverb".into(),
            params: Vec::new(),
        });
        app.fx_plugins.push(source::PluginFx {
            format: PluginFormat::Ladspa,
            name: "TapeDelay".into(),
            path: "/usr/lib/ladspa/tape.so".into(),
            id: "tape".into(),
            params: Vec::new(),
        });
        app.open_add_fx_modal();

        // The sidebar lists ALL plus every non-empty category, with counts.
        let sidebar = app.modal.as_ref().unwrap().list.sidebar.clone();
        assert_eq!(sidebar[0].0, "ALL");
        assert_eq!(
            sidebar[0].1,
            app.fx_menu_entries().len(),
            "ALL counts everything"
        );
        for cat in ["DELAY", "REVERB", "DISTORTION"] {
            assert!(
                sidebar.iter().any(|(l, n)| l == cat && *n > 0),
                "{cat} missing: {sidebar:?}"
            );
        }

        // Selecting REVERB shows only reverbs — including the LV2 one, whose
        // category was guessed from its name.
        let reverb = sidebar.iter().position(|(l, _)| l == "REVERB").unwrap();
        app.modal.as_mut().unwrap().list.sidebar_cursor = reverb;
        app.refresh_modal();
        let items = app.modal.as_ref().unwrap().list.items.clone();
        assert!(
            items.iter().any(|i| i.contains("[LV2] Calf Reverb")),
            "{items:#?}"
        );
        assert!(
            !items.iter().any(|i| i.contains("DELAY")),
            "only reverbs now: {items:#?}"
        );
        // The LADSPA one is named delay, so it sits in DELAY instead.
        let delay = sidebar.iter().position(|(l, _)| l == "DELAY").unwrap();
        app.modal.as_mut().unwrap().list.sidebar_cursor = delay;
        app.refresh_modal();
        assert!(app
            .modal
            .as_ref()
            .unwrap()
            .list
            .items
            .iter()
            .any(|i| i.contains("TapeDelay")));

        // The format chips narrow the sidebar too: under LV2 only the reverb
        // section survives.
        let lv2 = FX_FORMATS.iter().position(|f| *f == "LV2").unwrap();
        app.modal.as_mut().unwrap().list.filter = lv2;
        app.modal.as_mut().unwrap().list.sidebar_cursor = 0;
        app.refresh_modal();
        let sidebar = app.modal.as_ref().unwrap().list.sidebar.clone();
        assert_eq!(
            sidebar,
            vec![("ALL".to_string(), 1), ("REVERB".to_string(), 1)]
        );
        let items = app.modal.as_ref().unwrap().list.items.clone();
        assert!(
            items.iter().all(|i| i.contains("[LV2]")),
            "only LV2 now:\n{items:#?}"
        );

        // ← / → move between the two panes; Enter on the sidebar jumps into the
        // list, and Enter there adds the FX.
        app.modal.as_mut().unwrap().list.filter = 0;
        app.refresh_modal();
        handle_modal_key(&mut app, KeyCode::Left);
        assert!(app.modal.as_ref().unwrap().list.sidebar_focused);
        handle_modal_key(&mut app, KeyCode::Down);
        assert_eq!(
            app.modal.as_ref().unwrap().list.sidebar_cursor,
            1,
            "↓ moves the sidebar"
        );
        handle_modal_key(&mut app, KeyCode::Enter);
        assert!(
            !app.modal.as_ref().unwrap().list.sidebar_focused,
            "Enter enters the list"
        );
        assert!(app.fx_chain.is_empty(), "and adds nothing on the way");
        assert!(app.modal_select());
        assert_eq!(app.fx_chain.len(), 1);
    }

    /// Settings → AUDIO has seqterm's three sections, and Engine/OSC rows are
    /// editable with the arrows.
    #[test]
    fn audio_settings_has_engine_paths_and_osc_sections() {
        let _g = ui_guard();
        let _restore = UiRestore;
        sandbox_state_dir();
        let mut app = App::new();
        app.open_paths_modal();

        // The sidebar is the sub-category list, and it opens on Engine.
        let sections: Vec<String> = app
            .modal
            .as_ref()
            .unwrap()
            .list
            .sidebar
            .iter()
            .map(|(l, _)| l.clone())
            .collect();
        assert_eq!(sections, vec!["Engine", "Plugin Paths", "OSC"]);
        assert_eq!(app.audio_section(), SEC_ENGINE);

        // Engine rows: the same ones seqterm shows.
        let rows = app.modal.as_ref().unwrap().list.items.join("\n");
        for label in [
            "Backend",
            "Device",
            "Sample rate",
            "Buffer size",
            "Tempo",
            "Time signature",
            "SF2 engine",
            "Latency",
            // The capture device: this is the row that makes choz a
            // multi-effect on a box without JACK.
            "Input",
        ] {
            assert!(rows.contains(label), "{label} missing:\n{rows}");
        }
        assert!(rows.contains("5.3 ms"), "latency is computed: {rows}");

        // → on the backend row cycles it; sample rate and buffer likewise.
        app.modal.as_mut().unwrap().list.sidebar_focused = false;
        app.modal.as_mut().unwrap().list.cursor = 0;
        assert!(app.audio_settings_key(KeyCode::Right));
        assert_eq!(app.ui.audio.backend, "JACK");
        app.modal.as_mut().unwrap().list.cursor = 3;
        app.audio_settings_key(KeyCode::Right);
        assert_eq!(app.ui.audio.sample_rate, 88_200);
        app.modal.as_mut().unwrap().list.cursor = 4;
        app.audio_settings_key(KeyCode::Left);
        assert_eq!(app.ui.audio.buffer_size, 128);

        // The tempo is the one engine row that applies at once — a plugin reads
        // it on the next block, nothing has to be rebuilt.
        let transport = choz_ports::transport();
        transport.set_bpm(choz_ports::Transport::DEFAULT_BPM);
        app.modal.as_mut().unwrap().list.cursor = 5;
        app.audio_settings_key(KeyCode::Right);
        assert_eq!(transport.bpm(), 121.0);
        assert_eq!(app.ui.audio.bpm, 121.0, "and it is remembered");
        app.refresh_modal();
        assert!(
            app.modal.as_ref().unwrap().list.items[5].contains("121.0 BPM"),
            "the row shows the clock plugins actually see"
        );
        transport.set_bpm(choz_ports::Transport::DEFAULT_BPM);

        // The time signature is the row under it, and cycles.
        transport.set_time_signature(4, 4);
        app.modal.as_mut().unwrap().list.cursor = 6;
        app.audio_settings_key(KeyCode::Right);
        assert_eq!(transport.time_signature(), (3, 4));
        app.refresh_modal();
        assert!(
            app.modal.as_ref().unwrap().list.items[6].contains("3/4"),
            "the row follows"
        );
        app.audio_settings_key(KeyCode::Left);
        assert_eq!(transport.time_signature(), (4, 4), "and it goes back");

        // Those need a restart, and the rows say so.
        app.audio_engine = None;
        app.refresh_modal();

        // OSC section: enable, port mode and the two ports.
        app.modal.as_mut().unwrap().list.sidebar_cursor = SEC_OSC;
        app.refresh_modal();
        let rows = app.modal.as_ref().unwrap().list.items.join("\n");
        for label in ["Enable OSC", "Port mode", "UDP port", "TCP port", "server"] {
            assert!(rows.contains(label), "{label} missing:\n{rows}");
        }
        app.modal.as_mut().unwrap().list.cursor = 1;
        assert!(app.audio_settings_key(KeyCode::Enter));
        assert_eq!(app.ui.osc.port_mode, settings::OscPortMode::Random);
        // Enter on a port row opens a numeric editor; digits land in it.
        app.modal.as_mut().unwrap().list.cursor = 3;
        assert!(app.audio_settings_key(KeyCode::Enter));
        assert!(app.port_edit.is_some());
        for _ in 0..5 {
            app.audio_settings_key(KeyCode::Backspace);
        }
        for c in "9100".chars() {
            app.audio_settings_key(KeyCode::Char(c));
        }
        assert!(
            app.modal.as_ref().unwrap().list.items[3].contains("9100\u{2588}"),
            "caret shown"
        );
        app.audio_settings_key(KeyCode::Enter);
        assert_eq!(app.ui.osc.tcp_port, 9100);
        assert!(app.port_edit.is_none());

        // Everything was persisted.
        let saved = settings::UiSettings::load();
        assert_eq!(saved.audio.backend, "JACK");
        assert_eq!(saved.osc.tcp_port, 9100);
        assert_eq!(saved.audio.bpm, 121.0);
    }

    /// Clicking a category in the ADD FX sidebar shows that category — through
    /// The tint slider: it only exists with an image set, the arrows move it,
    /// and moving it drops the cached picture — a wash baked into the image is
    /// only visible once the image is rebuilt.
    #[test]
    fn the_panel_opacity_slider_moves_without_rebuilding_the_picture() {
        let _g = ui_guard();
        let _restore = UiRestore;
        sandbox_state_dir();
        let mut app = App::new();

        // The terminal's own background: choz cannot read that colour, so there
        // is nothing to be translucent against and neither row is offered.
        app.ui.background = settings::Background::Terminal;
        assert!(
            (0..app.theme_rows().len()).all(|i| {
                !matches!(
                    app.theme_row(i),
                    Some(ThemeRow::Tint) | Some(ThemeRow::PanelColor)
                )
            }),
            "nothing to blend with on the terminal's own background"
        );

        // A flat colour is something to see through, so both rows appear.
        app.ui.background = settings::Background::Color((1, 2, 3));
        assert!(
            (0..app.theme_rows().len()).any(|i| app.theme_row(i) == Some(ThemeRow::Tint)),
            "a flat desktop can be washed too"
        );
        assert!(
            (0..app.theme_rows().len()).any(|i| app.theme_row(i) == Some(ThemeRow::PanelColor)),
            "and the colour of that wash is a choice"
        );

        app.ui.background = settings::Background::Image {
            path: "/tmp/none.png".into(),
            fit: settings::ImageFit::Stretch,
        };
        let row = (0..app.theme_rows().len())
            .find(|&i| app.theme_row(i) == Some(ThemeRow::Tint))
            .expect("the slider is offered once an image is set");
        assert!(app.theme_rows()[row].contains('%'));

        app.ui.background_tint = 50;
        app.step_tint(5);
        assert_eq!(app.ui.background_tint, 55);

        // And it stops at the ends instead of wrapping.
        app.ui.background_tint = 98;
        app.step_tint(5);
        assert_eq!(app.ui.background_tint, 100);
        app.step_tint(-500);
        assert_eq!(app.ui.background_tint, 0);
    }

    /// The wash is what makes a panel readable over a photo *and* keeps the
    /// photo visible: the cell ends up between the picture's colour and the
    /// theme's, never at either end unless the slider is.
    #[test]
    fn panels_blend_the_theme_colour_over_the_picture() {
        let _g = ui_guard();
        let _restore = UiRestore;
        let area = ratatui::layout::Rect::new(0, 0, 4, 2);
        let mut buf = ratatui::buffer::Buffer::empty(area);

        views::theme::set_backdrop(Some(views::theme::Backdrop {
            cols: 4,
            rows: 2,
            cells: vec![(200, 200, 200); 8],
            tint: ((0, 0, 0), 0.5),
            graphics: false,
        }));
        views::theme::wash(&mut buf, ratatui::layout::Rect::new(0, 0, 2, 1));

        assert_eq!(
            buf[(0, 0)].bg,
            ratatui::style::Color::Rgb(100, 100, 100),
            "half of the picture, half of the theme"
        );
        assert_eq!(
            buf[(3, 1)].bg,
            ratatui::style::Color::Reset,
            "outside the rect, untouched"
        );

        // Opacity 0 leaves the picture alone; 100 hides it.
        views::theme::set_backdrop(Some(views::theme::Backdrop {
            cols: 4,
            rows: 2,
            cells: vec![(200, 200, 200); 8],
            tint: ((10, 20, 30), 1.0),
            graphics: false,
        }));
        views::theme::wash(&mut buf, area);
        assert_eq!(buf[(3, 1)].bg, ratatui::style::Color::Rgb(10, 20, 30));

        views::theme::set_backdrop(None);
    }

    /// The whole point of the wash, checked through the real `ui()`: with a
    /// photo behind it, a panel's cells carry a colour that came from *both*
    /// the picture and the theme — so the labels read and the wallpaper is
    /// still visible, and it is not one flat rectangle either.
    #[test]
    fn panels_over_a_photo_are_translucent_not_flat() {
        let _g = ui_guard();
        let _restore = UiRestore;
        sandbox_state_dir();
        let Some(photo) = ["assets/wallpaper2.jpg", "assets/wallpaper.png"]
            .into_iter()
            .map(std::path::PathBuf::from)
            .find(|p| p.exists())
        else {
            return; // no sample image in this checkout
        };

        let mut app = App::new();
        app.splash_done = true;
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.ui.background = settings::Background::Image {
            path: photo,
            fit: settings::ImageFit::Stretch,
        };
        app.ui.background_tint = 50;
        app.ui.theme_name = "Ruby Blue".to_string();
        views::theme::set_has_desktop(true);

        let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
        term.draw(|f| ui(f, &mut app)).unwrap();
        let buf = term.backend().buffer();

        // Inside the RACK, well away from the borders.
        let inside: Vec<ratatui::style::Color> = (10..40).map(|x| buf[(x, 12)].bg).collect();
        assert!(
            inside.iter().all(|c| *c != ratatui::style::Color::Reset),
            "a panel over a photo must not leave cells at the terminal default"
        );
        let distinct: std::collections::HashSet<_> = inside.iter().collect();
        assert!(
            distinct.len() > 1,
            "the picture has to show through: the panel is one flat colour instead"
        );

        let (tint, _) = app.ui.tint();
        assert!(
            inside
                .iter()
                .all(|c| *c != ratatui::style::Color::Rgb(tint.0, tint.1, tint.2)),
            "at 50% nothing should land on the pure theme colour"
        );

        views::theme::set_backdrop(None);
    }

    /// A plugin's patch has to survive the project file, and the *values* have
    /// to land on top of it: restoring state moves every parameter, so applying
    /// the knobs first would leave the tab sounding like the saved patch and
    /// looking like the saved knobs.
    #[test]
    fn a_plugin_patch_goes_into_the_project_and_comes_back() {
        let _g = ui_guard();
        let _restore = UiRestore;
        sandbox_state_dir();
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Plugin {
            format: "VST3".into(),
            id: "test:one".into(),
            name: "Tester".into(),
        }));
        app.slots[0].instr_values = vec![0.25];
        app.slots[0].instr_state = b"a patch, opaque to choz".to_vec();
        app.slots[0].fx_chain.push({
            let mut e = source::AudioFxEntry::new(source::AudioFxKind::Delay);
            e.state = b"fx patch".to_vec();
            e
        });

        // The snapshot persists the working copy first, so it has to match the
        // tab — same as anywhere else that builds a rack by hand.
        app.source = app.slots[0].source.clone();
        app.fx_chain = app.slots[0].fx_chain.clone();

        let snap = app.project_snapshot();
        assert!(
            !snap.rack[0].instrument.state.is_empty(),
            "the patch is written out"
        );
        assert_eq!(
            project::decode_state(&snap.rack[0].instrument.state).unwrap(),
            b"a patch, opaque to choz",
        );
        assert_eq!(
            project::decode_state(&snap.rack[0].fx[0].state).unwrap(),
            b"fx patch"
        );

        // Round trip through YAML and back into a fresh app.
        let text = serde_yaml::to_string(&snap).unwrap();
        let parsed: project::Project = serde_yaml::from_str(&text).unwrap();
        let mut fresh = App::new();
        fresh.apply_project_rack(parsed);
        assert_eq!(fresh.slots[0].instr_state, b"a patch, opaque to choz");
        assert_eq!(fresh.slots[0].fx_chain[0].state, b"fx patch");
        // The knob values belong to a plugin that is not installed here, so the
        // tab comes back without an instrument — and the patch is kept anyway,
        // which is the point: a missing plugin must not quietly erase the sound
        // from the file the next time it is saved.
        assert!(fresh.slots[0].instr_values.is_empty());
    }

    /// The switch itself: drawn hard right on the menu bar, clickable, and it
    /// saves — the mode is how this machine is set up, not a per-session whim.
    #[test]
    fn the_mode_switch_is_in_the_corner_and_clickable() {
        let _g = ui_guard();
        let _restore = UiRestore;
        sandbox_state_dir();
        let mut app = App::new();
        app.splash_done = true;
        app.slots.push(RackSlot::new(AudioSource::Midi));

        let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
        term.draw(|f| ui(f, &mut app)).unwrap();
        let screen: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            screen.contains("LIVE") && screen.contains("MULTI"),
            "no switch drawn"
        );

        let rect = app
            .layout
            .borrow()
            .mode_switch_rect
            .expect("the switch is hit-testable");
        assert!(
            rect.x + rect.width >= 118,
            "it belongs in the top-right corner, not at x={}",
            rect.x
        );
        assert_eq!(rect.y, 0);

        handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: rect.x + 1,
                row: rect.y,
                modifiers: crossterm::event::KeyModifiers::NONE,
            },
        );
        assert_eq!(app.ui.rack_mode, settings::RackMode::Multi);
        // F4 does the same from the keyboard.
        handle_key(&mut app, KeyCode::F(4));
        assert_eq!(app.ui.rack_mode, settings::RackMode::Live);
    }

    /// The two jobs choz does, and the switch between them.
    ///
    /// LIVE: one tab sounds, a program change steps through them — a rig on
    /// stage. MULTI: every tab answers its own MIDI channel and they all sound
    /// at once — a multi-timbral module for a DAW's orchestral template.
    #[test]
    fn live_picks_one_tab_and_multi_answers_every_channel() {
        let _g = ui_guard();
        let _restore = UiRestore;
        sandbox_state_dir();
        let mut app = App::new();
        // Three tabs on the same MIDI port. Numbered in MULTI, where a tab *is*
        // a channel; in LIVE they would land on ANY.
        app.ui.rack_mode = settings::RackMode::Multi;
        for _ in 0..3 {
            app.push_slot(AudioSource::Midi);
        }
        app.midi_connected = vec!["Keystation".into()];
        for slot in app.slots.iter_mut() {
            slot.input = Some(InputRef::Midi("Keystation".into()));
        }
        assert_eq!(
            app.slots.iter().map(|s| s.channel).collect::<Vec<_>>(),
            vec![1, 2, 3],
            "new tabs land on consecutive channels"
        );

        let midi = choz_engine::input::InputSource::Midi(0);

        // LIVE: one tab answers a note, and among tabs sharing a port the
        // channel is what picks which — a split keyboard, or two sequencer
        // tracks down one cable, land on different tabs.
        app.ui.rack_mode = settings::RackMode::Live;
        app.active_slot = 1;
        assert_eq!(
            app.targets_for(midi, 0),
            vec![0],
            "channel 1 → the tab set to 1"
        );
        assert_eq!(app.targets_for(midi, 1), vec![1]);
        assert_eq!(
            app.targets_for(midi, 2),
            vec![2],
            "even though tab 1 is the active one"
        );
        // A channel nobody claims falls back to the tab in front of the user.
        assert_eq!(
            app.targets_for(midi, 9),
            vec![1],
            "no tab listens to channel 10"
        );
        // Two tabs on the same channel are still one voice in LIVE: the active
        // one if it is among them, otherwise the first — never both.
        app.slots[2].channel = 2;
        assert_eq!(app.targets_for(midi, 1), vec![1]);
        app.active_slot = 0;
        assert_eq!(
            app.targets_for(midi, 1),
            vec![1],
            "the first of the two, not both"
        );
        app.slots[2].channel = 3;
        app.active_slot = 1;

        // …and an unbound program change selects a tab, like a live rig.
        app.apply_program_button(2);
        assert_eq!(app.active_slot, 2);

        // MULTI: the channel decides, and the active tab is irrelevant.
        app.ui.rack_mode = settings::RackMode::Multi;
        assert_eq!(
            app.targets_for(midi, 0),
            vec![0],
            "channel 1 → the tab set to 1"
        );
        assert_eq!(app.targets_for(midi, 2), vec![2]);
        assert_eq!(
            app.targets_for(midi, 9),
            Vec::<usize>::new(),
            "no tab on channel 10"
        );

        // Two tabs sharing a channel sound together — that is the layering the
        // live mode deliberately refuses.
        app.slots[2].channel = 1;
        assert_eq!(app.targets_for(midi, 0), vec![0, 2]);

        // The QWERTY piano has no channel of its own, so it still plays the
        // tab in front of the user.
        app.active_slot = 1;
        assert_eq!(
            app.targets_for(choz_engine::input::InputSource::Keyboard, 0),
            vec![1]
        );

        // And a program change no longer steals a tab: they all sound anyway.
        app.apply_program_button(0);
        assert_eq!(app.active_slot, 1);
    }

    /// The stuck-note bug: routing depends on which tab is active, so a note
    /// released *after* switching tabs used to send its note-off to the wrong
    /// instrument and leave the first one ringing forever.
    #[test]
    fn a_note_off_goes_to_the_tab_that_started_the_note() {
        let _g = ui_guard();
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.slots.push(RackSlot::new(AudioSource::Midi));

        // Played on tab 1 (the QWERTY piano always follows the active tab)…
        app.active_slot = 0;
        let on = app.start_note(choz_engine::input::InputSource::Keyboard, 0, 60);
        assert_eq!(on, vec![0]);

        // …released after switching to tab 2.
        app.active_slot = 1;
        assert_eq!(
            app.note_targets(choz_engine::input::InputSource::Keyboard),
            vec![1],
            "the routing really did move"
        );
        assert_eq!(
            app.end_note(choz_engine::input::InputSource::Keyboard, 0, 60),
            vec![0],
            "the note-off has to go back to the tab that is sounding it"
        );

        // A note choz never saw start still gets the best guess rather than
        // nothing — a controller plugged in mid-note, say.
        assert_eq!(
            app.end_note(choz_engine::input::InputSource::Keyboard, 0, 72),
            vec![1]
        );

        // Panic forgets everything it was tracking.
        app.start_note(choz_engine::input::InputSource::Keyboard, 0, 64);
        app.active_notes.push((64, 5));
        app.panic();
        assert!(app.sounding.is_empty() && app.active_notes.is_empty());
    }

    /// Carla's generic panel: every plugin gets its parameters as knobs in the
    /// RACK, so a CC can be learned on any of them **without opening the
    /// plugin's window** — which for a big synth is the slow part.
    #[test]
    fn instrument_knobs_are_drawn_clickable_and_learnable() {
        let _g = ui_guard();
        let _restore = UiRestore;
        sandbox_state_dir();
        let mut app = App::new();
        app.splash_done = true;
        app.slots.push(RackSlot::new(AudioSource::Plugin {
            format: "VST2".into(),
            id: "test:one".into(),
            name: "Tyrell".into(),
        }));
        app.slots[0].instr_params = vec![
            choz_engine::PluginParam {
                id: 9,
                name: "Cutoff".into(),
                min: 0.0,
                max: 1.0,
                default: 0.0,
                ..Default::default()
            },
            choz_engine::PluginParam {
                id: 4,
                name: "Volume".into(),
                min: 0.0,
                max: 1.0,
                default: 0.5,
                ..Default::default()
            },
        ];
        app.slots[0].instr_values = vec![0.2, 0.6];
        app.source = app.slots[0].source.clone();

        let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
        term.draw(|f| ui(f, &mut app)).unwrap();
        let screen: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            screen.contains("Cutoff"),
            "the instrument's knobs are not drawn:\n{screen}"
        );
        assert!(screen.contains("Volume"));

        let knobs = app.layout.borrow().rack.instr_knobs.clone();
        assert_eq!(knobs.len(), 2, "one clickable cell per parameter");

        // Clicking a knob moves the cursor there and hands the arrows to that
        // box instead of the FX chain's.
        let (_, rect) = knobs[1];
        handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: rect.x + 1,
                row: rect.y,
                modifiers: crossterm::event::KeyModifiers::NONE,
            },
        );
        assert_eq!(app.rack_focus, RackFocus::Instrument);
        assert_eq!(app.instr_param, 1);

        // And `w` turns *that* knob, not an FX one.
        handle_key(&mut app, KeyCode::Char('w'));
        assert!(app.slots[0].instr_values[1] > 0.6);

        // Pointer learn picks it without the plugin's window being open.
        app.start_learn_pick();
        let target = app.learn_target_at(ratatui::layout::Position {
            x: rect.x + 1,
            y: rect.y,
        });
        assert_eq!(target, Some(LearnTarget::InstrParam { slot: 0, param: 1 }));
    }

    /// The whole point of learn-from-the-plugin's-window, end to end in the UI:
    /// a knob moved inside the plugin picks the target, the next CC binds to
    /// it, and every CC after that drives that parameter.
    ///
    /// The plugin half (a real `audioMasterAutomate` / `performEdit`) is tested
    /// in the format crates; what is checked here is that choz turns a reported
    /// touch into a binding, which is the part that did nothing before.
    #[test]
    fn a_knob_touched_in_the_plugin_window_is_what_midi_learn_binds() {
        let _g = ui_guard();
        let _restore = UiRestore;
        sandbox_state_dir();
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        // Two parameters, as `read_params` would report them.
        // Ids that are deliberately not positions: a CLAP id, an LV2 port and a
        // VST3 ParamID are all arbitrary, and the UI must work off the index the
        // host translated to.
        app.slots[0].instr_params = vec![
            choz_engine::PluginParam {
                id: 4100,
                name: "Cutoff".into(),
                min: 0.0,
                max: 1.0,
                default: 0.0,
                ..Default::default()
            },
            choz_engine::PluginParam {
                id: 77,
                name: "Volume".into(),
                min: 0.0,
                max: 1.0,
                default: 0.5,
                ..Default::default()
            },
        ];
        app.slots[0].instr_values = vec![0.0, 0.5];

        app.start_learn_pick();
        assert!(
            app.learn.is_none(),
            "nothing is chosen until something is touched"
        );

        // The plugin says: the user just moved parameter 1 to 0.8.
        app.record_plugin_edit(0, None, 1, 0.8);
        assert_eq!(
            app.learn,
            Some(LearnTarget::InstrParam { slot: 0, param: 1 }),
            "the knob touched in the plugin's window is the one learn takes"
        );
        assert_eq!(
            app.slots[0].instr_values[1], 0.8,
            "and choz's own copy follows it, so the project saves what was done in there"
        );

        // The next CC binds; the ones after it drive the parameter.
        app.feed_cc(74, 100);
        assert!(app.learn.is_none(), "armed no longer");
        assert_eq!(app.cc_bindings.len(), 1);
        app.feed_cc(74, 127);
        assert_eq!(app.slots[0].instr_values[1], 1.0);
        app.feed_cc(74, 0);
        assert_eq!(app.slots[0].instr_values[1], 0.0);

        // A touch with learn disarmed only tracks the value; it must not
        // silently re-bind anything.
        app.record_plugin_edit(0, None, 0, 0.25);
        assert_eq!(app.learn, None);
        assert_eq!(app.slots[0].instr_values[0], 0.25);
    }

    /// With a desktop background in play, nothing may punch a hole in it.
    ///
    /// A hole is a cell left at `Color::Reset` — SGR 49, the terminal's own
    /// background, which is not transparent and reads as a grey rectangle over
    /// the wallpaper. The background renderer paints every cell of the area, so
    /// any `Reset` left afterwards is a widget that set its own.
    #[test]
    fn nothing_punches_a_hole_in_the_desktop() {
        let _g = ui_guard();
        let _restore = UiRestore;

        // Every state that draws over the body, because those are the ones that
        // call `Clear` — which resets cells instead of tinting them.
        type Setup = fn(&mut App);
        let states: Vec<(&str, Setup)> = vec![
            ("plain", |_| {}),
            ("drawers open", |a: &mut App| {
                a.in_open = true;
                a.out_open = true;
            }),
            ("add fx modal", |a: &mut App| a.open_add_fx_modal()),
            ("about", |a: &mut App| a.about_open = true),
            ("menu open", |a: &mut App| {
                a.menu = Some(menu::MenuState::open(menu::MenuKind::File))
            }),
        ];

        // The real terminal this was measured on, not a token 80x24: the holes
        // that mattered showed up at full size.
        let mut term = Terminal::new(TestBackend::new(170, 45)).unwrap();
        for (name, setup) in states {
            let mut app = App::new();
            app.splash_done = true;
            app.slots.push(RackSlot::new(AudioSource::Midi));
            app.ui.background = settings::Background::Color((10, 20, 30));
            views::theme::set_has_desktop(true);
            setup(&mut app);

            term.draw(|f| ui(f, &mut app)).unwrap();
            let buf = term.backend().buffer();
            let holes: Vec<(u16, u16)> = (0..45)
                .flat_map(|y| (0..170).map(move |x| (x, y)))
                .filter(|&(x, y)| buf[(x, y)].bg == ratatui::style::Color::Reset)
                .collect();
            assert!(
                holes.is_empty(),
                "{name}: {} cells reset the background, first at {:?}",
                holes.len(),
                &holes[..holes.len().min(8)],
            );
        }
    }

    /// the real mouse path, rects and all.
    #[test]
    fn add_fx_categories_are_clickable() {
        let _g = ui_guard();
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.open_add_fx_modal();

        // Draw it so the sidebar gets its rects, exactly as `ui()` does.
        let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
        let mut modal = app.modal.take().unwrap();
        term.draw(|f| {
            let rects = views::modal::draw_list_modal(f, &mut modal.list, f.area(), (80, 80));
            app.layout.borrow_mut().modal_rects = rects;
        })
        .unwrap();
        app.modal = Some(modal);

        let sidebar = app.layout.borrow().modal_rects.sidebar.clone();
        assert!(!sidebar.is_empty(), "the sidebar must be hit-testable");
        let labels = app.modal.as_ref().unwrap().list.sidebar.clone();
        let reverb = labels.iter().position(|(l, _)| l == "REVERB").unwrap();
        let (_, rect) = sidebar
            .iter()
            .find(|(i, _)| *i == reverb)
            .expect("REVERB row drawn");

        handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: rect.x + 1,
                row: rect.y,
                modifiers: crossterm::event::KeyModifiers::NONE,
            },
        );
        assert_eq!(
            app.modal.as_ref().unwrap().list.sidebar_cursor,
            reverb,
            "the click selected it"
        );
        let items = app.modal.as_ref().unwrap().list.items.clone();
        assert!(items.iter().any(|i| i.contains("REVERB")), "{items:#?}");
        assert!(
            !items.iter().any(|i| i.contains("DELAY")),
            "only reverbs now: {items:#?}"
        );
    }

    /// RACK ONLY takes the sound and leaves choz's own settings alone — a
    /// project written on another machine points at plugin paths that only
    /// exist there.
    #[test]
    fn loading_rack_only_keeps_the_local_settings() {
        let _g = ui_guard();
        let _restore = UiRestore;
        sandbox_state_dir();

        let mut source = App::new();
        source.slots.push(RackSlot::new(AudioSource::Midi));
        source
            .plugin_paths
            .dirs_mut(choz_engine::PluginFormat::Clap)
            .clear();
        source
            .plugin_paths
            .dirs_mut(choz_engine::PluginFormat::Clap)
            .push(choz_engine::SearchDir {
                path: "/somewhere/else".into(),
                enabled: true,
            });
        source.ui.language = i18n::Lang::Ja;
        let saved = source.project_snapshot();

        let mut app = App::new();
        let mine = app.plugin_paths.clone();
        let my_lang = app.ui.language;
        app.load_rack_only = true;
        app.apply_project(saved);

        assert_eq!(app.slots.len(), 1, "the rack still comes across");
        assert_eq!(app.plugin_paths, mine, "plugin paths are untouched");
        assert_eq!(app.ui.language, my_lang, "so is the language");
    }

    /// choz as a multi-effect on a box without JACK: the Settings row that
    /// opens a capture device, and the IN drawer saying where it is when there
    /// is none. Without this pair, "the effects do nothing on my microphone"
    /// looks exactly like a broken effect.
    #[test]
    fn the_engine_section_offers_a_capture_device() {
        let _g = ui_guard();
        let _restore = UiRestore;
        sandbox_state_dir();
        let mut app = App::new();
        app.open_paths_modal();
        app.modal.as_mut().unwrap().list.sidebar_cursor = SEC_ENGINE;
        app.refresh_modal();
        let rows = app.modal.as_ref().unwrap().list.items.clone();
        assert!(
            rows[2].contains("Input") && rows[2].contains("(off)"),
            "the input row is under the device and starts off: {:?}",
            rows[2]
        );

        // No engine in a test, so the row cannot be cycled — what has to hold
        // is that the setting is the one that persists, and that it starts
        // empty: choz must not open a microphone nobody asked for.
        assert_eq!(app.ui.audio.input_device, None);

        // And the IN drawer says where to switch it on rather than showing an
        // empty section with no explanation.
        let title = app
            .in_targets()
            .into_iter()
            .map(|(_, row)| row.name)
            .find(|n| n.contains("AUDIO IN"))
            .expect("the AUDIO IN header is drawn");
        assert!(
            title.contains("Settings") && title.contains("Input"),
            "an empty input section has to say where to open one: {title}"
        );
    }

    /// A mix that reaches no device is silence, and silence looks like every
    /// other thing that can be wrong. The transport line says it out loud.
    #[test]
    fn the_transport_says_when_the_output_reaches_nothing() {
        use ratatui::{backend::TestBackend, Terminal};
        let _g = ui_guard();
        let _restore = UiRestore;
        let app = App::new();
        // No engine at all: nothing is wired, and the line has to say so
        // rather than showing a device name and going quiet.
        let mut term = Terminal::new(TestBackend::new(120, 6)).unwrap();
        term.draw(|f| {
            draw_transport(f, &app, f.area());
        })
        .unwrap();
        let buf = term.backend().buffer().clone();
        let screen: String = (0..6)
            .map(|y| (0..120).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            screen.contains("NOT CONNECTED"),
            "an unwired output has to say so:\n{screen}"
        );
    }

    /// `TAP` belongs to the arpeggiator, so it is drawn on the arpeggiator's
    /// own box rather than floating on the row above it.
    #[test]
    fn tap_sits_on_the_arp_box_not_above_it() {
        let _g = ui_guard();
        let _restore = UiRestore;
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.active_slot = 0;
        app.edit_arp(ArpEdit::Toggle);

        // A tall panel: the arpeggiator gets its bordered box.
        let (_, rack) = render_rack(&mut app, 140, 40);
        let tap = rack
            .buttons
            .iter()
            .find(|(b, _)| *b == RackButton::ArpTap)
            .map(|&(_, r)| r)
            .expect("TAP is drawn");
        let knob = rack
            .arp_knobs
            .first()
            .map(|&(_, r)| r)
            .expect("the box has knobs");
        // The knobs are inside the box; TAP is on its top border, one row
        // above them — not on some earlier row of the panel.
        assert!(
            tap.y < knob.y && knob.y <= tap.y + 2,
            "TAP at row {} should be on the box the knobs at {} live in",
            tap.y,
            knob.y
        );
        assert!(
            tap.x > 40,
            "and right-aligned on that edge, not at the left margin: {}",
            tap.x
        );
    }

    /// The level on each capture jack, in the drawer that lists them.
    ///
    /// This is the reading that separates "nothing is arriving" from "it
    /// arrives and something downstream drops it" — three ways live audio goes
    /// missing that look identical without it.
    #[test]
    fn the_in_drawer_shows_the_level_on_each_capture_jack() {
        let _g = ui_guard();
        let _restore = UiRestore;
        let levels = choz_engine::meter::capture_levels();
        levels.clear();

        let mut app = App::new();
        app.in_ports = vec!["H340:capture_1".into(), "H340:capture_2".into()];
        let jack_rows = |app: &App| -> Vec<String> {
            app.in_targets()
                .into_iter()
                .filter(|(t, _)| matches!(t, InTarget::Channel(_)))
                .map(|(_, r)| r.name)
                .collect()
        };
        // Nothing arriving: the row says so instead of leaving it blank, which
        // would read as "no meter" rather than as "no signal".
        let quiet = jack_rows(&app);
        assert_eq!(quiet.len(), 2);
        assert!(
            quiet[0].contains("--"),
            "a silent jack is marked: {}",
            quiet[0]
        );

        // Half scale on the first jack, nothing on the second.
        levels.publish(&[vec![0.5; 64], vec![0.0; 64]], 64);
        let live = jack_rows(&app);
        assert!(
            live[0].contains("-6dB"),
            "the level shows on the jack it arrived on: {}",
            live[0]
        );
        assert!(
            live[1].contains("--"),
            "and not on the one it did not: {}",
            live[1]
        );
        levels.clear();
    }

    /// The drift between the capture clock and the playback clock, on screen.
    ///
    /// Silent while it behaves, a number the moment it does not — which is the
    /// difference between "my microphone crackles sometimes" and something a
    /// person can point at and measure.
    #[test]
    fn the_in_drawer_reports_capture_drift_only_when_there_is_some() {
        let _g = ui_guard();
        let _restore = UiRestore;
        let health = choz_engine::meter::capture_health();
        health.clear();

        let mut app = App::new();
        app.in_ports = vec!["H340:in_1".into(), "H340:in_2".into()];
        let header = |app: &App| {
            app.in_targets()
                .into_iter()
                .map(|(_, row)| row.name)
                .find(|n| n.contains("AUDIO IN"))
                .expect("the AUDIO IN header is drawn")
        };
        let clean = header(&app);
        assert!(clean.contains("(2)"), "the channel count: {clean}");
        assert!(
            !clean.contains("late") && !clean.contains("dropped"),
            "a behaving input says nothing about drift: {clean}"
        );

        health.late_block();
        health.dropped_samples(512);
        let drifting = header(&app);
        assert!(
            drifting.contains("1 late") && drifting.contains("512 dropped"),
            "both counts show once they move: {drifting}"
        );
        health.clear();
    }

    /// Sample rate, buffer and backend only apply on the next start, so the
    /// running engine still holds the previous ones. Saving from the engine
    /// threw away the change the user had just made in the Engine tab.
    #[test]
    fn saving_keeps_the_pending_audio_settings_not_the_running_ones() {
        let _g = ui_guard();
        sandbox_state_dir();
        let mut app = App::new();
        app.ui.audio.buffer_size = 128;
        app.ui.audio.sample_rate = 96_000;
        app.ui.audio.backend = "JACK".into();

        let p = app.project_snapshot();
        assert_eq!(p.audio.buffer_size, 128);
        assert_eq!(p.audio.sample_rate, 96_000);
        assert_eq!(p.audio.backend, "JACK");
    }

    /// And it loads back: a saved project rebuilds the same rack, knobs, mixer,
    /// routing and MIDI-learn bindings.
    #[test]
    fn a_saved_project_loads_back_into_the_same_rack() {
        let _g = ui_guard();
        // Loading applies the project's colour and language process-wide, so
        // this test has to put them back like the other global-state ones.
        let _restore = UiRestore;
        sandbox_state_dir();
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Sf2 {
            path: "/usr/share/sounds/sf2/FluidR3_GM.sf2".into(),
            bank: 0,
            preset: 4,
        }));
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.slots[0].input = Some(InputRef::Midi("Keystation".into()));
        app.slots[0].gain = 0.8;
        app.slots[0].pan = -0.25;
        app.slots[0].mute = true;
        app.slots[1].out_pair = (2, 3);
        app.slots[1].in_pair = Some((4, 5));
        app.source = app.slots[0].source.clone();
        let mut fx = AudioFxEntry::new(source::AudioFxKind::AmberFang);
        fx.params[0] = 0.75;
        fx.enabled = false;
        app.fx_chain.push(fx);
        app.cc_bindings.push(CcBinding {
            source: None,
            cc: 74,
            target: LearnTarget::Gain(0),
        });

        let saved = app.project_snapshot();

        let mut loaded = App::new();
        loaded.apply_project(saved);

        assert_eq!(loaded.slots.len(), 2);
        assert_eq!(loaded.slots[0].source, app.slots[0].source);
        assert_eq!(
            loaded.slots[0].input,
            Some(InputRef::Midi("Keystation".into()))
        );
        assert_eq!(loaded.slots[0].gain, 0.8);
        assert_eq!(loaded.slots[0].pan, -0.25);
        assert!(loaded.slots[0].mute);
        assert_eq!(loaded.slots[1].out_pair, (2, 3));
        assert_eq!(loaded.slots[1].in_pair, Some((4, 5)));

        let chain = &loaded.slots[0].fx_chain;
        assert_eq!(chain.len(), 1, "the FX chain came back");
        assert_eq!(chain[0].kind, source::AudioFxKind::AmberFang);
        assert_eq!(chain[0].params[0], 0.75, "knob positions survive");
        assert!(!chain[0].enabled, "so does the ON/OFF state");
        assert_eq!(loaded.cc_pairs(), vec![(74, LearnTarget::Gain(0))]);
        // The working copy has to follow the first tab, or the RACK draws stale.
        assert_eq!(loaded.source, app.slots[0].source);
        assert_eq!(loaded.fx_chain.len(), 1);
    }

    /// File → Save project writes one YAML with the sound *and* the config.
    #[test]
    fn saving_a_project_writes_everything_as_yaml() {
        let _g = ui_guard();
        sandbox_state_dir();
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Sf2 {
            path: "/usr/share/sounds/sf2/FluidR3_GM.sf2".into(),
            bank: 0,
            preset: 4,
        }));
        app.slots[0].input = Some(InputRef::Midi("Keystation".into()));
        app.slots[0].gain = 0.8;
        app.slots[0].pan = -0.25;
        app.source = app.slots[0].source.clone();
        app.fx_chain
            .push(AudioFxEntry::new(source::AudioFxKind::AmberFang));
        app.cc_bindings.push(CcBinding {
            source: None,
            cc: 74,
            target: LearnTarget::Gain(0),
        });
        app.midi_disabled.push("Midi Through".into());

        let dir = std::env::temp_dir().join(format!("choz_save_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        app.save_project_to(&dir);

        let yaml = std::fs::read_to_string(dir.join(project::DEFAULT_NAME)).unwrap();
        // Sound settings…
        assert!(yaml.contains("kind: sf2"), "{yaml}");
        assert!(yaml.contains("preset: 4"));
        assert!(
            yaml.contains("kind: amberfang"),
            "the FX chain and its knobs are in there"
        );
        assert!(yaml.contains("gain: 0.8"));
        assert!(yaml.contains("MIDI:Keystation"));
        assert!(yaml.contains("cc: 74"), "MIDI-learn bindings are saved");
        // …and choz's own configuration.
        assert!(yaml.contains("plugin_paths:"));
        assert!(yaml.contains(&format!("language: {}", app.ui.language.code())));
        assert!(yaml.contains("disabled_midi_inputs:"));
        assert!(yaml.contains("Midi Through"));

        // It parses back as the same project.
        let back: project::Project = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(back.rack.len(), 1);
        assert_eq!(back.rack[0].fx[0].kind, "amberfang");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A directory that yields nothing says so, and names the format it really
    /// holds — the cue that was missing when a folder of SoundFonts ended up
    /// under SFZ and the list stayed empty.
    #[test]
    fn a_directory_with_the_wrong_format_says_what_it_holds() {
        use choz_engine::{PluginFormat, SearchDir};
        sandbox_state_dir();
        let tmp = std::env::temp_dir().join(format!("choz_wrongfmt_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("piano.sf2"), b"x").unwrap();
        std::fs::write(tmp.join("organ.SF2"), b"x").unwrap();

        let mut app = App::new();
        app.plugin_paths = choz_engine::PluginPaths::default();
        // Filed under SFZ by mistake.
        app.plugin_paths
            .dirs_mut(PluginFormat::Sfz)
            .push(SearchDir {
                path: tmp.clone(),
                enabled: true,
            });
        open_paths_tab(&mut app);
        let row = app
            .modal
            .as_ref()
            .unwrap()
            .list
            .items
            .iter()
            .find(|i| i.contains(&tmp.display().to_string()))
            .expect("the added directory is listed")
            .clone();
        assert!(row.contains("(0"), "it contributed nothing: {row}");
        assert!(
            row.contains("SF2"),
            "and it says what it really holds: {row}"
        );

        // Filed correctly (and scanned), it shows its count instead.
        app.plugin_paths.dirs_mut(PluginFormat::Sfz).pop();
        app.plugin_paths
            .dirs_mut(PluginFormat::Sf2)
            .push(SearchDir {
                path: tmp.clone(),
                enabled: true,
            });
        app.plugins = choz_engine::scan_all(&app.plugin_paths);
        app.refresh_modal();
        let row = app
            .modal
            .as_ref()
            .unwrap()
            .list
            .items
            .iter()
            .find(|i| i.contains(&tmp.display().to_string()))
            .expect("still listed")
            .clone();
        assert!(row.contains("(2)"), "both SoundFonts counted: {row}");

        // A disabled directory is called out as off.
        app.plugin_paths
            .dirs_mut(PluginFormat::Sf2)
            .last_mut()
            .unwrap()
            .enabled = false;
        app.refresh_modal();
        let row = app
            .modal
            .as_ref()
            .unwrap()
            .list
            .items
            .iter()
            .find(|i| i.contains(&tmp.display().to_string()))
            .expect("still listed")
            .clone();
        assert!(row.contains("(off)"), "{row}");

        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// Closing the paths modal any way at all triggers a rescan when the list
    /// changed, so a freshly added directory is never ignored.
    #[test]
    fn editing_paths_marks_them_for_a_rescan_on_close() {
        sandbox_state_dir();
        let mut app = App::new();
        app.plugin_paths = choz_engine::PluginPaths::default();
        open_paths_tab(&mut app);
        assert!(!app.paths_dirty);

        let idx = app
            .path_rows()
            .iter()
            .position(|(_, d)| d.is_some())
            .unwrap();
        app.modal.as_mut().unwrap().list.cursor = idx;
        app.paths_modal_key(KeyCode::Enter); // toggle a directory off
        assert!(app.paths_dirty, "an edit marks the list dirty");

        // No engine here, so the rescan is a no-op; what matters is the flag
        // being consumed exactly when the modal closes.
        app.close_modal();
        assert!(app.modal.is_none());
        assert!(!app.paths_dirty, "closing consumes the rescan");
    }

    /// The paths modal's buttons drive the same keys as the keyboard, and the
    /// INPUTS panel's SCAN button rescans the MIDI ports.
    #[test]
    fn modal_action_buttons_and_the_inputs_scan_button_are_clickable() {
        let _g = ui_guard();
        sandbox_state_dir();
        let mut app = App::new();
        open_paths_tab(&mut app);

        // Draw the modal so its buttons get rects.
        let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
        let mut modal = app.modal.take().unwrap();
        term.draw(|f| {
            let rects = views::modal::draw_list_modal(f, &mut modal.list, f.area(), (80, 80));
            app.layout.borrow_mut().modal_rects = rects;
        })
        .unwrap();
        app.modal = Some(modal);
        let screen: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        for label in ["EDIT", "ADD", "BROWSE", "REMOVE", "DEFAULTS"] {
            assert!(screen.contains(label), "{label} button missing:\n{screen}");
        }

        // Put the cursor on a real directory row, then click EDIT.
        let idx = app
            .path_rows()
            .iter()
            .position(|(_, d)| d.is_some())
            .unwrap();
        app.modal.as_mut().unwrap().list.cursor = idx;
        let edit = app
            .layout
            .borrow()
            .modal_rects
            .actions
            .iter()
            .find(|(k, _)| *k == 'e')
            .map(|&(_, r)| r)
            .expect("EDIT is clickable");
        handle_modal_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: edit.x + 1,
                row: edit.y,
                modifiers: crossterm::event::KeyModifiers::NONE,
            },
        );
        assert!(
            app.path_edit.is_some(),
            "the EDIT button opens the path editor"
        );
    }

    /// "Save project" rewrites the file the project came from without a single
    /// question; "Save project as" always asks, and starts from that file's
    /// folder and name.
    #[test]
    fn save_rewrites_the_current_file_and_save_as_always_asks() {
        sandbox_state_dir();
        let dir = std::env::temp_dir().join(format!("choz_save_as_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let mut app = App::new();
        // Nothing saved yet: Save falls back to the picker.
        app.save_project();
        assert_eq!(
            app.modal.as_ref().map(|m| m.kind),
            Some(ModalKind::SaveProject),
            "without a file, Save is Save as"
        );
        app.close_modal();

        // With a file, it writes straight there and opens nothing.
        let file = dir.join("set-a.yml");
        app.project_file = Some(file.clone());
        app.save_project();
        assert!(app.modal.is_none(), "Save asks nothing");
        assert!(file.exists());

        // Save as starts in that file's folder, suggesting its name.
        app.open_save_project();
        let browser_dir = app
            .modal
            .as_ref()
            .and_then(|m| m.browser.as_ref())
            .map(|b| b.dir.clone());
        assert_eq!(browser_dir.as_deref(), Some(dir.as_path()));
        // The first entry of a DIR_PICK browser is "use this directory".
        assert!(!app.modal_select(), "picking the dir only prompts");
        assert_eq!(app.save_name.as_ref().unwrap().text.buf, "set-a.yml");

        // Typing a new name leaves the old file alone and moves the project.
        for _ in 0.."set-a.yml".chars().count() {
            app.save_name_key(KeyCode::Backspace);
        }
        for c in "set-b".chars() {
            app.save_name_key(KeyCode::Char(c));
        }
        app.save_name_key(KeyCode::Enter);
        assert!(dir.join("set-b.yml").exists());
        assert_eq!(
            app.project_file.as_deref(),
            Some(dir.join("set-b.yml").as_path())
        );

        // …and loading a project points Save at it too.
        app.load_project_from(&file);
        assert_eq!(app.project_file.as_deref(), Some(file.as_path()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A SoundFont dropped on a FluidSynth-DSSI tab is a `configure` call, not
    /// a new instrument — and after it the tab has the SoundFont's programs.
    ///
    /// Skipped without the plugin (or without a SoundFont) installed. It is the
    /// one DSSI synth here that starts empty, which is exactly what makes it
    /// the case worth testing: it had been loading and staying silent.
    #[test]
    fn a_soundfont_on_a_dssi_tab_configures_it_instead_of_replacing_it() {
        sandbox_state_dir();
        let plugin = std::path::Path::new("/usr/lib/dssi/fluidsynth-dssi.so");
        let sf2 = std::path::Path::new("/usr/share/sounds/sf2/FluidR3_GM.sf2");
        if !plugin.exists() || !sf2.exists() {
            eprintln!("no fluidsynth-dssi (or no SoundFont) here; skipping");
            return;
        }

        let mut app = App::new();
        app.audio_engine = Some(choz_engine::AudioEngine::new(48_000, 256));
        app.synths.push(SynthEntry {
            id: "FluidSynth-DSSI".into(),
            format: choz_engine::PluginFormat::Dssi,
            name: "FluidSynth-DSSI".into(),
            path: plugin.to_path_buf(),
        });
        app.slots.push(RackSlot::new(AudioSource::Plugin {
            id: "FluidSynth-DSSI".into(),
            format: "DSSI".into(),
            name: "FluidSynth-DSSI".into(),
        }));
        app.source = app.slots[0].source.clone();
        let engine = app.audio_engine.as_mut().unwrap();
        engine.add_silent().unwrap();
        engine
            .load_dssi(0, plugin, "FluidSynth-DSSI", &[])
            .expect("the plugin loads");
        assert!(app.active_is_dssi());
        assert!(
            app.audio_engine
                .as_ref()
                .unwrap()
                .slot_presets(0)
                .is_empty(),
            "no SoundFont, no programs"
        );

        // The SF2 picker lands here. The tab must still be the same plugin.
        app.load_source(sf2.to_path_buf());
        assert!(
            matches!(app.slots[0].source, AudioSource::Plugin { .. }),
            "the DSSI instrument was replaced instead of configured"
        );
        assert_eq!(
            app.slots[0].dssi_config,
            [("load".to_string(), sf2.to_string_lossy().into_owned())]
        );
        assert!(
            app.slots[0].plugin_presets.len() > 100,
            "the SoundFont's programs: {}",
            app.slots[0].plugin_presets.len()
        );

        // …and the settings travel with the project.
        let saved = app.project_snapshot();
        assert_eq!(saved.rack[0].instrument.config, app.slots[0].dssi_config);
    }

    /// A plugin with thousands of patches is only usable if the picker narrows
    /// them: the chips are its banks, a row picks the preset it points at (not
    /// the position it sits in), and the arrows stay inside the bank.
    #[test]
    fn a_plugins_patches_are_picked_by_bank() {
        sandbox_state_dir();
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Plugin {
            id: "org.surge-synth-team.surge-xt".into(),
            format: "CLAP".into(),
            name: "Surge XT".into(),
        }));
        app.source = app.slots[0].source.clone();
        // Shaped like the real scan: two levels of directory per patch.
        let entry = |cat: &str, name: &str| choz_engine::PresetEntry {
            name: name.to_string(),
            category: cat.to_string(),
            key: format!("/patches/{cat}/{name}.fxp"),
        };
        app.slots[0].plugin_presets = vec![
            entry("A.Liv / Basses", "808er"),
            entry("A.Liv / Keys", "Rhodes"),
            entry("Factory / Basses", "Sub"),
            entry("Factory / Leads", "Saw"),
            entry("Factory / Leads", "Square"),
        ];

        // The banks are the first level, deduplicated and in list order.
        assert_eq!(app.preset_banks(0), ["A.Liv", "Factory"]);

        app.open_preset_modal();
        let chips = app.modal.as_ref().unwrap().list.filters.clone();
        assert_eq!(chips.len(), 3, "every bank, plus one chip for all of them");
        assert_eq!(app.preset_rows().len(), 5, "chip 0 hides nothing");

        // Second chip = the second bank: three of the five patches.
        app.modal.as_mut().unwrap().list.filter = 2;
        let rows = app.preset_rows();
        assert_eq!(rows.iter().map(|(i, _)| *i).collect::<Vec<_>>(), [2, 3, 4]);
        assert!(rows[0].1.contains("Sub"), "{rows:?}");

        // Selecting row 1 of that view applies preset 3, not preset 1.
        app.modal.as_mut().unwrap().list.cursor = 1;
        app.modal_select();
        assert_eq!(app.slots[0].preset_cursor, 3);

        // The arrows stay inside "Factory": forward lands on Square…
        app.step_preset(1);
        assert_eq!(app.slots[0].preset_cursor, 4);
        // …and the end of the bank is where they stop, not "A.Liv".
        app.step_preset(1);
        assert_eq!(app.slots[0].preset_cursor, 4);
        app.step_preset(-1);
        app.step_preset(-1);
        assert_eq!(app.slots[0].preset_cursor, 2, "the first patch of the bank");
        app.step_preset(-1);
        assert_eq!(app.slots[0].preset_cursor, 2, "and it does not leave it");
    }

    /// SAVE PROJECT asks for a name and refuses to overwrite on a single
    /// keypress: picking the directory only opens the prompt, Enter writes the
    /// typed file, and a second save onto the same name has to be confirmed.
    #[test]
    fn saving_a_project_asks_for_the_name_and_for_the_overwrite() {
        sandbox_state_dir();
        let dir = std::env::temp_dir().join(format!("choz_save_name_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let mut app = App::new();
        app.open_save_project();
        // Picking the directory opens the name prompt instead of saving.
        app.save_name = Some(SaveName::new(
            dir.clone(),
            project::DEFAULT_NAME.to_string(),
        ));
        app.refresh_modal();
        assert!(
            !dir.join(project::DEFAULT_NAME).exists(),
            "nothing written yet"
        );

        // Retype the suggested name, then save.
        let suggested = project::DEFAULT_NAME.chars().count();
        for _ in 0..suggested {
            app.save_name_key(KeyCode::Backspace);
        }
        for c in "my-set".chars() {
            assert!(app.save_name_key(KeyCode::Char(c)));
        }
        assert!(app.save_name_key(KeyCode::Enter));
        let file = dir.join("my-set.yml");
        assert!(file.exists(), "Enter writes the typed name (with .yml)");
        assert!(app.modal.is_none(), "a saved project closes the modal");

        // Same name again: Enter asks first, and Esc backs out without writing.
        let before = std::fs::metadata(&file).unwrap().len();
        std::fs::write(&file, "clobbered").unwrap();
        app.open_save_project();
        app.save_name = Some(SaveName::new(
            dir.clone(),
            project::DEFAULT_NAME.to_string(),
        ));
        for _ in 0..project::DEFAULT_NAME.chars().count() {
            app.save_name_key(KeyCode::Backspace);
        }
        for c in "my-set".chars() {
            app.save_name_key(KeyCode::Char(c));
        }
        app.save_name_key(KeyCode::Enter);
        assert!(
            app.save_name.as_ref().unwrap().confirm,
            "an existing file asks before it is replaced"
        );
        // The question is the modal itself: two rows, starting on the harmless
        // one, naming the file that is about to go.
        let m = app.modal.as_ref().unwrap();
        assert_eq!(m.list.items.len(), 2, "overwrite / rename");
        assert_eq!(m.list.cursor, 1, "Enter twice does not clobber anything");
        assert!(m.list.items[0].contains("my-set.yml"), "{:?}", m.list.items);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "clobbered");

        // The rename row backs out, and so does Esc.
        app.save_name_key(KeyCode::Enter);
        assert!(!app.save_name.as_ref().unwrap().confirm, "rename backs out");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "clobbered");
        app.save_name_key(KeyCode::Enter);
        app.save_name_key(KeyCode::Esc);
        assert!(
            !app.save_name.as_ref().unwrap().confirm,
            "Esc backs out of the overwrite"
        );
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "clobbered");

        // Confirming on the OVERWRITE row does replace it.
        app.save_name_key(KeyCode::Enter);
        app.modal.as_mut().unwrap().list.cursor = 0;
        app.save_name_key(KeyCode::Enter);
        assert!(std::fs::metadata(&file).unwrap().len() > before / 2);
        assert_ne!(std::fs::read_to_string(&file).unwrap(), "clobbered");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A path can be typed in place: `e` loads the row into the editor, the
    /// keys land in the buffer, Enter stores it and Esc leaves it alone.
    #[test]
    fn plugin_paths_can_be_typed_in_the_modal() {
        use choz_engine::PluginFormat;
        sandbox_state_dir();
        let mut app = App::new();
        app.plugin_paths = choz_engine::PluginPaths::default();
        open_paths_tab(&mut app);

        let rows = app.path_rows();
        let (idx, &(fmt, dir)) = rows
            .iter()
            .enumerate()
            .find(|(_, (f, d))| *f == PluginFormat::Vst2 && d.is_some())
            .expect("VST2 has default directories");
        let i = dir.unwrap();
        app.modal.as_mut().unwrap().list.cursor = idx;

        // `e` opens the editor pre-filled with the current path.
        assert!(app.paths_modal_key(KeyCode::Char('e')));
        let before = app.plugin_paths.dirs(fmt)[i].path.clone();
        assert_eq!(
            app.path_edit.as_ref().unwrap().text.buf,
            before.display().to_string()
        );
        // The row being typed is what the modal shows, caret and all.
        let shown = &app.modal.as_ref().unwrap().list.items[idx];
        assert!(shown.contains('\u{2588}'), "the caret is drawn: {shown}");

        // Retype it: clear, then type a new path.
        for _ in 0..before.display().to_string().chars().count() {
            app.paths_modal_key(KeyCode::Backspace);
        }
        for c in "/opt/vst".chars() {
            app.paths_modal_key(KeyCode::Char(c));
        }
        assert!(app.path_edit.is_some(), "still editing until Enter");
        app.paths_modal_key(KeyCode::Enter);
        assert!(app.path_edit.is_none(), "Enter ends the edit");
        assert_eq!(
            app.plugin_paths.dirs(fmt)[i].path,
            std::path::Path::new("/opt/vst")
        );

        // Esc discards, and `a` types a brand new entry.
        assert!(app.paths_modal_key(KeyCode::Char('e')));
        app.paths_modal_key(KeyCode::Char('X'));
        app.paths_modal_key(KeyCode::Esc);
        assert_eq!(
            app.plugin_paths.dirs(fmt)[i].path,
            std::path::Path::new("/opt/vst")
        );
        assert!(app.modal.is_some(), "Esc leaves the edit, not the modal");

        let count = app.plugin_paths.dirs(fmt).len();
        assert!(app.paths_modal_key(KeyCode::Char('a')));
        for c in "/srv/plugins".chars() {
            app.paths_modal_key(KeyCode::Char(c));
        }
        app.paths_modal_key(KeyCode::Enter);
        assert_eq!(app.plugin_paths.dirs(fmt).len(), count + 1);
        assert!(app
            .plugin_paths
            .dirs(fmt)
            .iter()
            .any(|d| d.path == std::path::Path::new("/srv/plugins")));

        // Emptying an existing row removes it.
        app.modal.as_mut().unwrap().list.cursor = idx;
        assert!(app.paths_modal_key(KeyCode::Char('e')));
        for _ in 0.."/opt/vst".len() {
            app.paths_modal_key(KeyCode::Backspace);
        }
        app.paths_modal_key(KeyCode::Enter);
        assert_eq!(app.plugin_paths.dirs(fmt).len(), count);
    }

    /// Settings → Plugin paths lists every format, and edits persist into the
    /// config the scanner reads.
    #[test]
    fn plugin_paths_modal_lists_formats_and_edits_dirs() {
        sandbox_state_dir();
        let mut app = App::new();
        app.plugin_paths = choz_engine::PluginPaths::default();
        open_paths_tab(&mut app);
        let items = app.modal.as_ref().unwrap().list.items.clone();
        for fmt in choz_engine::PluginFormat::ALL {
            assert!(
                items.iter().any(|i| i == fmt.label()),
                "{} missing",
                fmt.label()
            );
        }

        // Put the cursor on the first LV2 directory and switch it off.
        let rows = app.path_rows();
        let (idx, &(fmt, dir)) = rows
            .iter()
            .enumerate()
            .find(|(_, (f, d))| *f == choz_engine::PluginFormat::Lv2 && d.is_some())
            .expect("LV2 has default directories");
        app.modal.as_mut().unwrap().list.cursor = idx;
        assert!(
            app.paths_modal_key(KeyCode::Enter),
            "Enter toggles a directory"
        );
        let i = dir.unwrap();
        assert!(!app.plugin_paths.dirs(fmt)[i].enabled);
        assert!(
            !app.plugin_paths
                .all_enabled()
                .contains(&app.plugin_paths.dirs(fmt)[i].path),
            "a disabled directory is not scanned"
        );

        // `d` removes it, `r` restores the format's defaults.
        let before = app.plugin_paths.dirs(fmt).len();
        assert!(app.paths_modal_key(KeyCode::Char('d')));
        assert_eq!(app.plugin_paths.dirs(fmt).len(), before - 1);
        assert!(app.paths_modal_key(KeyCode::Char('r')));
        assert_eq!(app.plugin_paths.dirs(fmt).len(), before);
        assert!(app.plugin_paths.dirs(fmt).iter().all(|d| d.enabled));
    }

    /// The `+` on the tab bar adds a second tab on the same input, activates it,
    /// and leaves it with its own (empty) chain — the first tab's setup is
    /// untouched and stays silent while the new one is active.
    #[test]
    fn the_plus_button_adds_another_tab_on_the_same_input() {
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.slots[0].input = Some(InputRef::Midi("Keystation".into()));
        app.fx_chain
            .push(AudioFxEntry::new(source::AudioFxKind::Delay));
        let (_, rack) = render_rack(&mut app, 100, 30);
        let plus = rack.tab_add.expect("the + button is drawn and clickable");

        handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: plus.x + 1,
                row: plus.y,
                modifiers: crossterm::event::KeyModifiers::NONE,
            },
        );

        assert_eq!(app.slots.len(), 2, "the + appended a tab");
        assert_eq!(app.active_slot, 1, "the new tab takes over");
        assert_eq!(
            app.slots[1].input, app.slots[0].input,
            "same input as the tab it came from"
        );
        assert!(
            app.fx_chain.is_empty(),
            "the new tab starts with its own empty chain"
        );
        assert_eq!(app.slots[0].fx_chain.len(), 1, "the first tab kept its FX");

        // One port, two configurations: only the active tab is fed. Both land
        // on ANY, because in LIVE a new tab is another patch and not a split.
        assert_eq!(app.slots[1].channel, ANY_CHANNEL);
        let connected = vec!["Keystation".to_string()];
        let bindings: Vec<Option<&InputRef>> = app.slots.iter().map(|s| s.input.as_ref()).collect();
        let channels: Vec<u8> = app.slots.iter().map(|s| s.channel).collect();
        assert_eq!(
            note_targets(&bindings, &channels, &connected, 1, InputSource::Midi(0), 0),
            vec![1]
        );
        assert_eq!(
            note_targets(&bindings, &channels, &connected, 0, InputSource::Midi(0), 0),
            vec![0]
        );
    }

    /// The loop's length is the one number a lane is measured against, and it
    /// is shown in **bars** because "16 beats" means nothing in 6/8.
    #[test]
    fn the_automation_loop_is_as_long_as_the_user_says() {
        let _g = ui_guard();
        let clock = choz_ports::transport();
        clock.set_time_signature(4, 4);
        let mut app = App::new();

        assert_eq!(
            app.automation_loop_bars(),
            4,
            "four bars of four by default"
        );
        app.nudge_automation_loop(4);
        assert_eq!(app.automation_loop_bars(), 8);
        assert_eq!(
            app.automation.loop_beats, 32.0,
            "and eight bars of four is 32 beats"
        );
        app.nudge_automation_loop(-100);
        assert_eq!(
            app.automation_loop_bars(),
            1,
            "one bar is as short as a loop gets"
        );

        // A bar is what the time signature says it is: eight bars of 6/8 is 24
        // quarter notes, not 32.
        clock.set_time_signature(6, 8);
        app.nudge_automation_loop(7);
        assert_eq!(app.automation_loop_bars(), 8);
        assert_eq!(app.automation.loop_beats, 24.0);
        clock.set_time_signature(4, 4);
    }

    /// Record a fader move against the clock, then let it move on its own.
    ///
    /// The whole loop, through the app: arm, roll the transport, move a control,
    /// disarm, wind the clock forward and watch the value come back.
    #[test]
    fn a_recorded_move_plays_itself_back_on_the_next_pass() {
        let _g = ui_guard();
        let _restore = UiRestore;
        sandbox_state_dir();
        let mut app = App::new();
        app.push_slot(AudioSource::Midi);

        let clock = choz_ports::transport();
        clock.set_sample_rate(48_000);
        clock.set_bpm(120.0); // one beat every half second
        clock.set_time_signature(4, 4);
        clock.rewind();
        app.automation.loop_beats = 4.0;
        app.playing = true;

        // Arm and move the fader a beat in.
        app.automation.recording = true;
        app.tick_automation();
        clock.advance(24_000); // half a second = one beat
        app.slots[0].gain = 1.5;
        app.tick_automation();
        app.automation.recording = false;

        let lane = app
            .automation
            .lanes
            .iter()
            .find(|l| l.target == LearnTarget::Gain(0))
            .expect("the fader that moved has a lane");
        assert_eq!(lane.points.len(), 2, "where it started and where it went");
        assert!(
            (lane.points[1].0 - 1.0).abs() < 0.01,
            "one beat in: {:?}",
            lane.points
        );

        // Wind on to the same place in the next pass, with the fader put back:
        // the lane moves it again.
        app.slots[0].gain = 1.0;
        clock.advance(24_000 * 6); // three more beats, then one into the next loop
        app.tick_automation();
        assert!(
            (app.slots[0].gain - 1.5).abs() < 0.05,
            "the lane replayed the move, got {}",
            app.slots[0].gain
        );

        // Stopped, nothing moves — a lane is a position in a loop and there is
        // no position without a clock.
        app.playing = false;
        app.slots[0].gain = 1.0;
        app.tick_automation();
        assert_eq!(app.slots[0].gain, 1.0);

        // And clearing forgets it.
        app.automation.clear(None);
        assert!(app.automation.is_empty());
        clock.rewind();
        clock.set_bpm(choz_ports::Transport::DEFAULT_BPM);
    }

    /// The `A→M` button: only where there is audio coming in, and it survives a
    /// project round trip.
    #[test]
    fn audio_to_midi_is_offered_only_on_a_tab_fed_by_audio() {
        let _g = ui_guard();
        let _restore = UiRestore;
        sandbox_state_dir();
        let mut app = App::new();
        app.push_slot(AudioSource::Midi);

        // No capture pair: nothing to listen to, so no button.
        assert_eq!(app.pitch_to_midi_state(), None);
        app.toggle_pitch_to_midi();
        assert!(!app.slots[0].pitch_to_midi, "and toggling it does nothing");

        // Fed by a capture pair: the button appears, off.
        app.slots[0].in_pair = Some((0, 1));
        assert_eq!(app.pitch_to_midi_state(), Some(false));
        let (screen, rack) = render_rack(&mut app, 120, 30);
        assert!(
            screen.contains('\u{2192}'),
            "the A→M button is drawn: {screen}"
        );
        let rect = rack
            .buttons
            .iter()
            .find(|(b, _)| *b == views::fx_chain_panel::RackButton::PitchToMidi)
            .map(|(_, r)| *r)
            .expect("and it is clickable");

        // Clicking turns it on.
        handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: rect.x + 1,
                row: rect.y,
                modifiers: crossterm::event::KeyModifiers::NONE,
            },
        );
        assert_eq!(app.pitch_to_midi_state(), Some(true));

        // It travels with the project, because it is part of how the tab sounds.
        let saved = app.project_snapshot();
        assert!(saved.rack[0].mixer.pitch_to_midi);
    }

    /// One port, two tabs, two channels: the split the roadmap asked for.
    ///
    /// Two controllers, two tabs: **each keyboard drives its own tab**, whatever
    /// is on screen. This is the stage case — a Keystation on the e-piano, a
    /// KeyStep on Surge — and before bindings knew where a CC came from, the
    /// KeyStep's mod wheel moved whatever the Keystation's had been assigned to.
    #[test]
    fn each_controller_drives_its_own_tabs_learned_controls() {
        let _g = ui_guard();
        let _restore = UiRestore;
        sandbox_state_dir();
        let mut app = App::new();
        for _ in 0..2 {
            app.push_slot(AudioSource::Midi);
        }
        app.midi_connected = vec!["Keystation".into(), "KeyStep".into()];
        app.slots[0].input = Some(InputRef::Midi("Keystation".into()));
        app.slots[1].input = Some(InputRef::Midi("KeyStep".into()));
        let keystation = choz_engine::input::InputSource::Midi(0);
        let keystep = choz_engine::input::InputSource::Midi(1);

        // CC 74 learned on each keyboard, each pointing at its own tab's VOL.
        app.learn = Some(LearnTarget::Gain(0));
        app.apply_cc(keystation, 0, 74, 100);
        app.learn = Some(LearnTarget::Gain(1));
        app.apply_cc(keystep, 0, 74, 100);
        assert_eq!(
            app.cc_bindings.len(),
            2,
            "same CC from two controllers is two bindings: {:?}",
            app.cc_bindings
        );

        // Tab 2 is on screen. The Keystation's fader still moves tab 1 — its
        // own tab — and leaves tab 2 alone.
        app.active_slot = 1;
        app.slots[0].gain = 0.0;
        app.slots[1].gain = 0.0;
        app.apply_cc(keystation, 0, 74, 127);
        assert!(app.slots[0].gain > 0.0, "the Keystation moved its own tab");
        assert_eq!(app.slots[1].gain, 0.0, "and not the tab on screen");

        // …and the KeyStep's moves tab 2 while tab 1 is on screen.
        app.active_slot = 0;
        app.slots[0].gain = 0.0;
        app.apply_cc(keystep, 0, 74, 127);
        assert!(app.slots[1].gain > 0.0, "the KeyStep moved its own tab");
        assert_eq!(app.slots[0].gain, 0.0);
    }

    /// The one case where the tab on screen decides: **both tabs on the same
    /// port**. A port has one owner at a time — the same rule its notes follow —
    /// so its fader moves the tab that is actually playing.
    #[test]
    fn a_shared_port_gives_its_ccs_to_the_tab_on_screen() {
        let _g = ui_guard();
        let _restore = UiRestore;
        sandbox_state_dir();
        let mut app = App::new();
        for _ in 0..2 {
            app.push_slot(AudioSource::Midi);
        }
        app.midi_connected = vec!["Keystation".into()];
        for slot in app.slots.iter_mut() {
            slot.input = Some(InputRef::Midi("Keystation".into()));
        }
        let midi = choz_engine::input::InputSource::Midi(0);

        // The same fader learned twice on the same port, once per tab.
        app.active_slot = 0;
        app.learn = Some(LearnTarget::Gain(0));
        app.apply_cc(midi, 0, 74, 100);
        app.active_slot = 1;
        app.learn = Some(LearnTarget::Gain(1));
        app.apply_cc(midi, 0, 74, 100);

        // Tab 2 on screen: only tab 2 moves.
        app.slots[0].gain = 0.0;
        app.slots[1].gain = 0.0;
        app.apply_cc(midi, 0, 74, 127);
        assert_eq!(app.slots[0].gain, 0.0, "the tab off screen stays put");
        assert!(app.slots[1].gain > 0.0);

        // Switch tabs and the same fader moves the other one.
        app.active_slot = 0;
        app.slots[1].gain = 0.0;
        app.apply_cc(midi, 0, 74, 127);
        assert!(app.slots[0].gain > 0.0);
        assert_eq!(app.slots[1].gain, 0.0);

        // A channel claim still wins over the tab on screen, as it does for
        // notes: tab 2 asks for channel 3 and gets channel 3's fader.
        app.slots[1].channel = 3;
        app.slots[0].gain = 0.0;
        app.slots[1].gain = 0.0;
        app.apply_cc(midi, 2, 74, 127);
        assert!(
            app.slots[1].gain > 0.0,
            "channel 3 reaches the tab that asked"
        );
        assert_eq!(app.slots[0].gain, 0.0);
    }

    /// It has to be opt-in, and this is why: pressing `+` gives another patch on
    /// the same controller, and if that tab claimed a channel the controller
    /// already sends, the patch on screen would go silent. So a tab answers
    /// **any** channel until someone gives it a number.
    #[test]
    fn a_channel_splits_one_port_between_tabs_in_live() {
        let _g = ui_guard();
        let _restore = UiRestore;
        sandbox_state_dir();
        let mut app = App::new();
        for _ in 0..2 {
            app.push_slot(AudioSource::Midi);
        }
        app.midi_connected = vec!["Keystation".into()];
        for slot in app.slots.iter_mut() {
            slot.input = Some(InputRef::Midi("Keystation".into()));
        }
        assert_eq!(app.ui.rack_mode, settings::RackMode::Live);
        assert_eq!(
            app.slots.iter().map(|s| s.channel).collect::<Vec<_>>(),
            vec![ANY_CHANNEL, ANY_CHANNEL],
            "a new tab in LIVE is another patch, not a part"
        );

        let midi = choz_engine::input::InputSource::Midi(0);
        // With both on ANY nothing changed: the tab on screen plays.
        app.active_slot = 1;
        assert_eq!(app.targets_for(midi, 0), vec![1]);
        assert_eq!(app.targets_for(midi, 5), vec![1]);

        // Give the lower tab channel 3 and the port is split: channel 3 reaches
        // it even while the other tab is the active one, and everything else
        // still goes where it went.
        app.slots[0].channel = 3;
        assert_eq!(app.targets_for(midi, 2), vec![0], "channel 3 is claimed");
        assert_eq!(app.targets_for(midi, 0), vec![1], "the rest is unchanged");

        // The button says which it is, and stepping past 16 gets back to ANY.
        app.active_slot = 0;
        assert_eq!(
            app.tab_channel(),
            Some(3),
            "shown, because another tab shares the port"
        );
        app.slots[0].channel = 16;
        app.step_channel(1);
        assert_eq!(app.slots[0].channel, ANY_CHANNEL, "16 → ANY");
        app.step_channel(1);
        assert_eq!(app.slots[0].channel, 1, "and on to 1");

        // A lone tab has nothing to split, so the button is not offered.
        app.remove_slot(1);
        assert_eq!(app.tab_channel(), None);
    }

    #[test]
    fn notes_reach_only_the_tabs_bound_to_their_input() {
        let keys = InputRef::Midi("Keystation".to_string());
        let other = InputRef::Midi("Midi Through".to_string());
        // tab 0 ← Keystation, tab 1 ← OSC, tab 2 ← Keystation, tab 3 unbound.
        let bindings = vec![Some(&keys), Some(&InputRef::Osc), Some(&keys), None];
        let channels = vec![1u8, 1, 1, 1];
        let connected = vec!["Keystation".to_string(), "Midi Through".to_string()];

        // Tabs 0 and 2 are two configurations of the same port: one plays at a
        // time, the active one when it's among them, the first otherwise.
        assert_eq!(
            note_targets(&bindings, &channels, &connected, 2, InputSource::Midi(0), 0),
            vec![2]
        );
        assert_eq!(
            note_targets(&bindings, &channels, &connected, 0, InputSource::Midi(0), 0),
            vec![0]
        );
        assert_eq!(
            note_targets(&bindings, &channels, &connected, 3, InputSource::Midi(0), 0),
            vec![0]
        );
        assert_eq!(
            note_targets(&bindings, &channels, &connected, 3, InputSource::Osc, 0),
            vec![1]
        );
        assert!(
            note_targets(&bindings, &channels, &connected, 3, InputSource::Midi(1), 0).is_empty(),
            "no tab is bound to {other:?}"
        );
        assert!(
            note_targets(&bindings, &channels, &connected, 3, InputSource::Midi(9), 0).is_empty(),
            "unknown port index is dropped, not panicked on"
        );
    }

    #[test]
    fn the_qwerty_piano_always_plays_the_active_tab() {
        let osc = InputRef::Osc;
        let bindings = vec![Some(&osc), None];
        let channels = vec![1u8, 1];
        assert_eq!(
            note_targets(&bindings, &channels, &[], 1, InputSource::Keyboard, 0),
            vec![1]
        );
        assert_eq!(
            note_targets(&bindings, &channels, &[], 0, InputSource::Keyboard, 0),
            vec![0],
            "even a bound tab is playable from the keyboard"
        );
        assert!(
            note_targets(&[], &[], &[], 0, InputSource::Keyboard, 0).is_empty(),
            "empty rack"
        );
    }

    /// The IN drawer holds both kinds of input: note sources first, then the
    /// audio capture pairs that can feed a tab instead of its instrument.
    #[test]
    fn in_drawer_lists_note_inputs_then_audio_capture() {
        let _g = ui_guard();
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.midi_ports = vec!["Keystation".to_string()];

        let rows = app.in_targets();
        let targets: Vec<InTarget> = rows.iter().map(|(t, _)| *t).collect();
        let names: Vec<String> = rows.iter().map(|(_, r)| r.name.clone()).collect();
        assert_eq!(
            targets[0],
            InTarget::None,
            "section titles are not selectable"
        );
        assert_eq!(targets[1], InTarget::Note(0));
        assert_eq!(names[1], "Keystation");
        // Note inputs, then the AUDIO IN title, then "play the instrument".
        let audio = targets
            .iter()
            .position(|t| *t == InTarget::NoCapture)
            .expect("no audio row");
        assert!(
            rows[audio].1.connected,
            "a tab with no capture is on its instrument"
        );

        // Down from the last note input hops over the AUDIO IN header onto the
        // first row under it. No engine in a test, so that row is the
        // "(instrument)" one — there are no capture ports to list.
        let last_note = targets
            .iter()
            .rposition(|t| matches!(t, InTarget::Note(_)))
            .unwrap();
        app.input_cursor = last_note;
        assert_eq!(
            in_step(&app, 1),
            audio,
            "the cursor must not park on a header"
        );

        // Selecting a capture pair swaps the tab onto live audio, and the RACK
        // says so instead of naming an instrument that isn't heard.
        app.set_active_capture(Some((2, 3)));
        assert_eq!(app.slots[0].in_pair, Some((2, 3)));
        assert!(
            app.instrument_label().contains("3/4"),
            "{}",
            app.instrument_label()
        );
        app.input_cursor = audio;
        app.in_select(audio);
        assert_eq!(app.slots[0].in_pair, None, "back to the instrument");
    }

    /// The OUT drawer's row model: devices, then **one row per channel** of the
    /// running device, each saying what it is to the active tab.
    #[test]
    fn out_drawer_lists_devices_then_single_channels() {
        // Reads translated text, so it has to hold the same lock the tests
        // that change the language hold.
        let _g = ui_guard();
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.out_devices = vec!["UMC1820".into(), "Headset".into()];
        app.slots[1].out_pair = (0, 1);
        app.active_slot = 1;

        let labels: Vec<String> = app
            .out_targets()
            .iter()
            .map(|(_, r)| r.label.clone())
            .collect();
        assert_eq!(labels[0], "DEVICE");
        assert_eq!(labels[1], "UMC1820");
        assert!(
            labels[3].starts_with("CHANNELS"),
            "channel section: {labels:?}"
        );
        // No engine in a test, so the device is the stereo default: two rows.
        assert!(
            labels[4].starts_with("1  L"),
            "channel 1 is the tab's left: {labels:?}"
        );
        assert!(
            labels[5].starts_with("2  R"),
            "and channel 2 its right: {labels:?}"
        );
        assert!(
            labels[4].contains("tab 1"),
            "both tabs are on it: {labels:?}"
        );

        let targets: Vec<OutTarget> = app.out_targets().iter().map(|(t, _)| *t).collect();
        assert_eq!(targets[0], OutTarget::None, "headers are not selectable");
        assert_eq!(targets[1], OutTarget::Device(0));
        assert_eq!(targets[4], OutTarget::Channel(0));
        assert_eq!(targets[5], OutTarget::Channel(1));
    }

    /// Enter on a channel row routes the *active* tab there, leaving the others
    /// alone — that is the whole "[MIDI] → plugin → out 3" gesture.
    #[test]
    fn selecting_a_channel_routes_only_the_active_tab() {
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.active_slot = 1;

        let row = app
            .out_targets()
            .iter()
            .position(|(t, _)| *t == OutTarget::Channel(0))
            .expect("a channel row is drawn");
        app.slots[1].out_pair = (8, 9);
        app.out_select(row);

        assert_eq!(
            app.slots[1].out_pair,
            (9, 0),
            "channel 1 joined, the oldest fell off"
        );
        assert_eq!(app.slots[0].out_pair, (0, 1), "the other tab is untouched");
    }

    /// Channels go on and off one at a time, which is the point: a tab can play
    /// out of 3 and 9, jacks that are not a pair and never were.
    #[test]
    fn channels_go_on_and_off_one_at_a_time() {
        assert_eq!(
            assign_channel((2, 2), 8),
            (2, 8),
            "a second jack is the right side"
        );
        assert_eq!(
            assign_channel((2, 8), 8),
            (2, 8),
            "assigning what is already on is a no-op"
        );
        assert_eq!(
            assign_channel((2, 8), 4),
            (8, 4),
            "a third pushes the oldest off"
        );
        assert_eq!(
            unassign_channel((2, 8), 2),
            Some((8, 8)),
            "what is left goes mono"
        );
        assert_eq!(
            unassign_channel((2, 8), 5),
            Some((2, 8)),
            "a channel it never had"
        );
        assert_eq!(
            unassign_channel((2, 2), 2),
            None,
            "the last one leaves nothing"
        );

        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        // A new tab is on 1 and 2. Click 3, click 9, and it plays out of **3
        // and 9** — the routing the pairs could not express.
        assert_eq!(app.slots[0].out_pair, (0, 1));
        app.set_active_out(2, Assign::On);
        app.set_active_out(8, Assign::On);
        assert_eq!(app.slots[0].out_pair, (2, 8));

        // The right button takes one off and the other goes mono.
        app.set_active_out(2, Assign::Off);
        assert_eq!(app.slots[0].out_pair, (8, 8));
        // And a tab has to come out somewhere: the last one cannot be removed.
        app.set_active_out(8, Assign::Off);
        assert_eq!(app.slots[0].out_pair, (8, 8), "a tab always has an output");

        // Enter is the toggle: on when off, off when on.
        app.set_active_out(2, Assign::Toggle);
        assert_eq!(app.slots[0].out_pair, (8, 2));
        app.set_active_out(2, Assign::Toggle);
        assert_eq!(app.slots[0].out_pair, (8, 8));

        // And the rows say what each channel carries.
        assert_eq!(side_label(Some((2, 8)), 2), "  L");
        assert_eq!(side_label(Some((2, 8)), 8), "  R");
        assert_eq!(side_label(Some((2, 8)), 5), "", "a channel it does not use");
        assert_eq!(side_label(Some((4, 4)), 4), "  L+R");
    }

    /// `A→M` plays the tab's instrument, not its FX chain. A tab with no
    /// instrument tracks the pitch perfectly and has nothing to play it on,
    /// which looks exactly like a broken tracker — so the rack says which it is.
    #[test]
    fn audio_to_midi_says_when_there_is_no_instrument_to_play() {
        // Reads translated text, so it has to hold the same lock the tests
        // that change the language hold.
        let _g = ui_guard();
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.slots[0].in_pair = Some((4, 4));
        assert_eq!(
            app.instrument_label(),
            "AUDIO IN 5",
            "passing audio through is fine"
        );

        app.slots[0].pitch_to_midi = true;
        assert!(
            app.instrument_label().contains("needs an instrument"),
            "{}",
            app.instrument_label()
        );

        // With one loaded it goes back to naming the input.
        app.source = AudioSource::Sf2 {
            path: "x.sf2".into(),
            bank: 0,
            preset: 0,
        };
        assert_eq!(app.instrument_label(), "AUDIO IN 5");
    }

    /// The graphic EQ draws as a bank of sliders, tanu's way: a column per
    /// band with the zero line through the middle. Ten arcs cannot be read as a
    /// curve, and the curve is the only question anyone asks an EQ.
    #[test]
    fn the_graphic_eq_draws_as_a_slider_bank() {
        // Taken here *and* inside `render_rack`; the guard is reentrant on one
        // thread, which is what stops that from being a deadlock.
        let _g = ui_guard();
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.fx_chain
            .push(AudioFxEntry::new(source::AudioFxKind::GraphicEq));
        app.fx_slot = 0;
        // A shape only a bank can show: boost the bottom, cut the top.
        for (i, v) in [0.9f32, 0.85, 0.7, 0.55, 0.5, 0.45, 0.3, 0.2, 0.15, 0.1]
            .iter()
            .enumerate()
        {
            app.fx_chain[0].params[i] = *v;
        }
        let (screen, rack) = render_rack(&mut app, 110, 34);

        // The zero line, the tracks and the knobs are all there.
        assert!(screen.contains('\u{2588}'), "no slider knobs:\n{screen}");
        assert!(screen.contains('\u{2502}'), "no slider tracks:\n{screen}");
        // The band labels are the frequencies, not "p1 p2 p3".
        for label in ["70", "180", "1k", "16k"] {
            assert!(screen.contains(label), "band {label} unlabelled:\n{screen}");
        }
        // One click rect per band, plus the knobs that are not bands.
        assert!(
            rack.params.len() > choz_engine::fx::EQ_BANDS,
            "{} rects",
            rack.params.len()
        );
        let band_rects: Vec<_> = rack
            .params
            .iter()
            .filter(|(i, _)| *i < choz_engine::fx::EQ_BANDS)
            .collect();
        assert_eq!(
            band_rects.len(),
            choz_engine::fx::EQ_BANDS,
            "a rect per band"
        );
        // Side by side, and each one tall enough to be a slider.
        assert!(band_rects[1].1.x > band_rects[0].1.x, "left to right");
        assert!(band_rects[0].1.height >= 6, "a slider, not a row");
    }

    /// A named parameter is a list, so Enter and a click open the list. Walking
    /// eighteen Winamp presets with an arrow key is a knob pretending to be a
    /// menu.
    #[test]
    fn a_named_fx_parameter_opens_a_picker() {
        let _g = ui_guard();
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.fx_chain
            .push(AudioFxEntry::new(source::AudioFxKind::GraphicEq));
        app.fx_slot = 0;

        // A band is a knob: there is nothing to list.
        assert!(!app.open_fx_choice(0));
        assert!(app.modal.is_none());

        // The preset is a list of names, and every Winamp preset is in it.
        let preset = app.fx_chain[0]
            .param_descs()
            .iter()
            .position(|d| d.name == "Preset")
            .expect("the EQ has a preset");
        assert!(app.open_fx_choice(preset));
        let m = app.modal.as_ref().expect("a modal opened");
        assert_eq!(m.kind, ModalKind::FxChoice);
        assert_eq!(m.list.items.len(), choz_engine::fx::EQ_PRESETS.len());
        assert!(m.list.items.iter().any(|i| i == "Rock"));

        // Choosing one moves the parameter to that position and fills the bands.
        let rock = m.list.items.iter().position(|i| i == "Rock").unwrap();
        app.modal.as_mut().unwrap().list.cursor = rock;
        assert!(app.modal_select(), "picking closes it");
        let v = app.fx_chain[0].params[preset];
        let expect = rock as f32 / (choz_engine::fx::EQ_PRESETS.len() - 1) as f32;
        assert!((v - expect).abs() < 1e-4, "{v} vs {expect}");

        // AutoTune's scale and mode are lists too, by the same route.
        app.fx_chain
            .push(AudioFxEntry::new(source::AudioFxKind::AutoTune));
        app.fx_slot = 1;
        for name in ["Preset", "Key", "Scale", "Mode"] {
            let i = app.fx_chain[1]
                .param_descs()
                .iter()
                .position(|d| d.name == name)
                .unwrap();
            assert!(app.open_fx_choice(i), "{name} should open a list");
            app.modal = None;
        }
    }

    /// A preset has to move the **sliders**, not just the processor. The array
    /// is what the panel draws and what the project saves: a preset that only
    /// reaches the DSP vanishes the next time anything is rebuilt, and until
    /// then the sliders say something that is not true.
    #[test]
    fn an_eq_preset_moves_the_sliders() {
        use choz_engine::fx::{graphic_eq, EQ_BANDS, EQ_PRESETS};
        let mut entry = source::AudioFxEntry::new(source::AudioFxKind::GraphicEq);
        let slot = entry
            .param_descs()
            .iter()
            .position(|d| d.name == "Preset")
            .unwrap();
        assert!(
            entry.params[..EQ_BANDS]
                .iter()
                .all(|v| (*v - 0.5).abs() < 1e-6),
            "flat to start"
        );

        let rock = EQ_PRESETS.iter().position(|(n, _)| *n == "Rock").unwrap();
        entry.params[slot] = rock as f32 / (EQ_PRESETS.len() - 1) as f32;
        assert!(
            entry.apply_preset(slot),
            "the preset knob changed the bands"
        );

        let gains = EQ_PRESETS[rock].1;
        for (b, db) in gains.iter().enumerate() {
            let want = graphic_eq::db_to_norm(*db);
            assert!(
                (entry.params[b] - want).abs() < 1e-4,
                "band {b}: slider at {} but the preset says {want}",
                entry.params[b]
            );
        }
        // Rock is a smile curve, so the sliders are not all in one place.
        assert!(entry.params[0] > 0.55 && entry.params[3] < 0.45);

        // Any other knob is not the preset.
        assert!(!entry.apply_preset(0));
    }

    /// Turning a knob must not rebuild the chain. A rebuild replaces **every**
    /// processor in the slot, so nudging one control threw away the reverb's
    /// tail, the delay's buffer and the looper's recording — which is heard as
    /// the sound cutting out.
    #[test]
    fn moving_a_knob_does_not_rebuild_the_chain() {
        use source::{AudioFxEntry, AudioFxKind};
        // Everything with state takes its parameters live.
        for kind in [
            AudioFxKind::Reverb,
            AudioFxKind::Delay,
            AudioFxKind::SpaceEcho,
            AudioFxKind::Protocosmos,
            AudioFxKind::Z5Texture,
            AudioFxKind::GranDelay,
            AudioFxKind::Vinyl,
            AudioFxKind::Filter,
            AudioFxKind::GraphicEq,
            AudioFxKind::AutoTune,
        ] {
            assert!(
                AudioFxEntry::takes_live_params(kind),
                "{kind:?} would cut the sound"
            );
        }

        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.fx_chain.push(AudioFxEntry::new(AudioFxKind::SpaceEcho));
        app.fx_dirty = false;
        app.set_fx_param(0, 1, 0.7);
        assert!(!app.fx_dirty, "a space echo knob rebuilt the chain");
        assert!(
            (app.fx_chain[0].params[1] - 0.7).abs() < 1e-6,
            "and the value did land"
        );

        // One that cannot take them live still asks for the rebuild, because
        // otherwise the knob would do nothing at all.
        app.fx_chain.push(AudioFxEntry::new(AudioFxKind::Cassette));
        app.fx_dirty = false;
        app.set_fx_param(1, 0, 0.3);
        assert!(
            app.fx_dirty,
            "cassette has no live path, so it must rebuild"
        );
    }

    /// The interface's list of built-ins and the engine's are the same list.
    ///
    /// They are written down twice — the interface needs a category, a label
    /// and a parameter layout the engine has no use for — so what keeps them
    /// honest is this: an effect the interface offers and the engine cannot
    /// build is a dead row, and one the engine has that the interface never
    /// lists is an effect nobody can reach (and, since the CLAP export walks
    /// the engine's list, one that exists *outside* choz but not inside it).
    #[test]
    fn the_interface_and_the_engine_agree_on_which_effects_exist() {
        let ui: std::collections::BTreeSet<&str> =
            source::ALL_FX_KINDS.iter().map(|k| k.id()).collect();
        let engine: std::collections::BTreeSet<&str> = choz_engine::fx_chain::BUILT_IN_KINDS
            .iter()
            .map(|(id, _)| *id)
            .collect();
        assert_eq!(
            ui.difference(&engine).collect::<Vec<_>>(),
            Vec::<&&str>::new(),
            "the interface offers effects the engine cannot build"
        );
        assert_eq!(
            engine.difference(&ui).collect::<Vec<_>>(),
            Vec::<&&str>::new(),
            "the engine has effects nothing in the interface lists"
        );
    }

    /// A saved tab finds its jacks by **name**, because the index moves.
    ///
    /// Unplug an interface and every capture index past it shifts by two, so a
    /// project reopened without the card used to listen to somebody else's
    /// microphone and say nothing. Names first; and when the jack is really
    /// gone the routing is dropped rather than guessed, because a tab playing
    /// its instrument is obvious and the wrong microphone is not.
    #[test]
    fn a_saved_tab_finds_its_capture_jacks_by_name() {
        let mixer = |ports: Option<(&str, &str)>| project::Mixer {
            gain: 1.0,
            gain_r: None,
            link: None,
            pan: 0.0,
            mute: false,
            solo: false,
            out_pair: Some((0, 1)),
            in_pair: Some((0, 1)),
            in_ports: ports.map(|(l, r)| (l.to_string(), r.to_string())),
            pitch_to_midi: false,
            pitch_mix: None,
            in_gain: None,
            in_gate: None,
        };
        // The card that was first when this was saved is now second: the
        // indices say 0/1 and the names say otherwise. The names win.
        let now = [
            "onboard:capture_1".to_string(),
            "onboard:capture_2".to_string(),
            "UMC1820:capture_1".to_string(),
            "UMC1820:capture_2".to_string(),
        ];
        assert_eq!(
            resolve_in_pair(
                &now,
                &mixer(Some(("UMC1820:capture_1", "UMC1820:capture_2")))
            ),
            Some((2, 3))
        );

        // The card is not plugged in at all: no audio in, rather than the
        // laptop's microphone pretending to be it.
        assert_eq!(
            resolve_in_pair(&now, &mixer(Some(("MOTU:capture_7", "MOTU:capture_8")))),
            None
        );

        // A project written before the names existed still says what it can.
        assert_eq!(resolve_in_pair(&now, &mixer(None)), Some((0, 1)));
    }

    /// The harmoniser's MIDI input: a switch, a channel, the active tab as the
    /// reference — and nothing at all in MULTI.
    #[test]
    fn the_harmonizer_follows_the_keyboard_only_when_it_is_asked_to() {
        use views::midi_monitor::Converted;
        let chord = choz_engine::chord::chord();
        chord.clear();

        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.active_slot = 0;
        app.fx_chain
            .push(AudioFxEntry::new(source::AudioFxKind::Harmonizer));

        // A chord held on channel 3, routed to the tab on screen.
        for note in [60u8, 64, 67] {
            app.keyboard.feed(
                &midi::InputEvent::Note(midi::NoteMsg {
                    source: choz_engine::input::InputSource::Midi(0),
                    channel: 2,
                    on: true,
                    note,
                    vel: 100,
                }),
                Some(0),
            );
        }
        let _ = Converted::PitchToMidi;

        // The switch is off: nothing is published, whatever is held.
        app.publish_chord();
        let mut out = [0u8; choz_engine::chord::MAX_NOTES];
        assert_eq!(chord.read(&mut out), 0, "off means off");

        // On, and pointed at channel 3 (the knob is 0..1 across sixteen).
        app.fx_chain[0].params[9] = 1.0;
        app.fx_chain[0].params[10] = 2.0 / 15.0;
        app.publish_chord();
        assert_eq!(chord.read(&mut out), 3, "the chord reaches the effect");
        assert_eq!(&out[..3], &[60, 64, 67]);

        // Another channel: the same keys are somebody else's.
        app.fx_chain[0].params[10] = 0.0;
        app.publish_chord();
        assert_eq!(chord.read(&mut out), 0, "channel 1 is not channel 3");

        // MULTI turns the whole thing off: there, every tab answers its own
        // channel and one process-wide chord would be the wrong keyboard.
        app.fx_chain[0].params[10] = 2.0 / 15.0;
        app.publish_chord();
        assert_eq!(chord.read(&mut out), 3);
        app.ui.rack_mode = settings::RackMode::Multi;
        app.publish_chord();
        assert_eq!(chord.read(&mut out), 0, "no chord in MULTI");
        chord.clear();
    }

    /// A Max patch imports into the chain, and the report names what it could
    /// not bring — which is the half of an import that is worth reading.
    #[test]
    fn a_max_patch_imports_what_it_can_and_names_what_it_cannot() {
        let dir = std::env::temp_dir().join("choz-ui-maxpat");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let patch = dir.join("guitar.maxpat");
        std::fs::write(
            &patch,
            r#"{"patcher":{"boxes":[
                {"box":{"id":"obj-1","maxclass":"newobj","text":"adc~"}},
                {"box":{"id":"obj-2","maxclass":"newobj","text":"overdrive~"}},
                {"box":{"id":"obj-3","maxclass":"newobj","text":"gizmo~ 2048"}},
                {"box":{"id":"obj-4","maxclass":"newobj","text":"freeverb~"}},
                {"box":{"id":"obj-5","maxclass":"newobj","text":"dac~"}}
            ],"lines":[
                {"patchline":{"source":["obj-1",0],"destination":["obj-2",0]}},
                {"patchline":{"source":["obj-2",0],"destination":["obj-3",0]}},
                {"patchline":{"source":["obj-3",0],"destination":["obj-4",0]}},
                {"patchline":{"source":["obj-4",0],"destination":["obj-5",0]}}
            ]}}"#,
        )
        .unwrap();

        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.active_slot = 0;
        app.import_maxpat(&patch);

        let kinds: Vec<&str> = app.fx_chain.iter().map(|e| e.kind.label()).collect();
        assert_eq!(kinds, vec!["SATURATOR", "REVERB"], "in signal order");

        let modal = app.modal.as_ref().expect("the report opens by itself");
        assert_eq!(modal.kind, ModalKind::MaxReport);
        let report = modal.list.items.join("\n");
        assert!(
            report.contains("gizmo~"),
            "it names what it dropped:\n{report}"
        );
        assert!(report.contains("SATURATOR"), "and what it kept:\n{report}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The feedback guard is a switch in Settings and a reading in the IN
    /// drawer, and both say the same thing.
    ///
    /// A guard that is working invisibly is indistinguishable from the room
    /// having gone quiet on its own — which is exactly the moment a player
    /// needs to know it was choz that pulled the microphone down.
    #[test]
    fn the_feedback_guard_is_switchable_and_says_when_it_is_holding() {
        let _guard = ui_guard();
        sandbox_state_dir();
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.in_ports = vec!["capture_1".to_string(), "capture_2".to_string()];

        // Armed by default, and the row says so.
        assert!(app.ui.audio.feedback_guard);
        let rows = app.engine_rows();
        let row = rows
            .iter()
            .find(|r| r.contains("Feedback guard"))
            .expect("the row is in the Engine section");
        assert!(row.contains("ON"), "{row}");

        // Holding something down: both the row and the drawer say how much.
        choz_engine::meter::capture_health().guard(-18.0);
        let rows = app.engine_rows();
        let row = rows.iter().find(|r| r.contains("Feedback guard")).unwrap();
        assert!(row.contains("holding"), "{row}");
        let drawer = app.in_targets();
        assert!(
            drawer.iter().any(|(_, row)| row.name.contains("GUARD")),
            "the drawer says it too: {:?}",
            drawer.iter().map(|(_, r)| &r.name).collect::<Vec<_>>()
        );

        // Off is off, everywhere: the engine's own switch follows the setting.
        app.ui.audio.feedback_guard = false;
        choz_engine::feedback::arm(false);
        assert!(!choz_engine::feedback::armed());
        let rows = app.engine_rows();
        let row = rows.iter().find(|r| r.contains("Feedback guard")).unwrap();
        assert!(row.contains("OFF"), "{row}");

        choz_engine::feedback::arm(true);
        choz_engine::meter::capture_health().clear();
    }

    /// AutoTune is a built-in like any other: it is in the ADD FX list, under a
    /// category of its own, and it builds into a real processor.
    #[test]
    fn autotune_is_one_of_the_built_in_effects() {
        use source::{AudioFxKind, FxCategory};
        assert!(source::ALL_FX_KINDS.contains(&AudioFxKind::AutoTune));
        assert_eq!(AudioFxKind::AutoTune.label(), "AUTO-TUNE");
        assert_eq!(AudioFxKind::AutoTune.id(), "autotune");
        assert_eq!(AudioFxKind::AutoTune.category(), FxCategory::Pitch);
        assert!(
            FxCategory::ALL.contains(&FxCategory::Pitch),
            "and the section exists"
        );
        assert_eq!(
            AudioFxKind::from_id("autotune"),
            Some(AudioFxKind::AutoTune)
        );

        // The knobs are named, not numbered: a key is C or it is not.
        let entry = source::AudioFxEntry::new(AudioFxKind::AutoTune);
        let descs = entry.param_descs();
        let named = |n: &str| {
            descs
                .iter()
                .find(|d| d.name == n)
                .map(|d| matches!(d.shape, source::ParamShape::Named(_)))
                .unwrap_or(false)
        };
        assert!(named("Preset") && named("Key") && named("Scale") && named("Mode"));
        // No formant switch: the shifter resamples, so the envelope moves with
        // the pitch and there is nothing to switch. A control that does nothing
        // is worse than no control.
        assert!(!descs.iter().any(|d| d.name == "Formant"));

        // And it builds, which is what the rack actually does with it.
        let params: Vec<f32> = descs.iter().map(|d| d.default).collect();
        assert!(choz_engine::fx_chain::build_processor("autotune", &params, 48_000).is_some());
    }

    /// Picking a preset writes the knobs it stands for. Without that the rebuild
    /// reads the parameter array, finds the defaults, and the preset lasts until
    /// the next knob is touched.
    #[test]
    fn an_autotune_preset_fills_in_the_knobs_below_it() {
        let mut entry = source::AudioFxEntry::new(source::AudioFxKind::AutoTune);
        let before = entry.params.clone();
        // Preset 2 is Hard Auto-Tune: immediate retune, full correction.
        entry.params[0] = 2.0 / (choz_engine::fx::autotune::PRESETS.len() - 1) as f32;
        assert!(entry.apply_preset(0), "the preset knob changed something");
        assert!(
            entry.params[1] < before[1] + 0.01,
            "retune went to the floor"
        );
        assert_eq!(entry.params[5], 1.0, "and the mode is Hard Tune");

        // Any other knob is not a preset.
        assert!(!entry.apply_preset(3));
        // Neither is a preset knob on an effect that has none.
        let mut delay = source::AudioFxEntry::new(source::AudioFxKind::Delay);
        assert!(!delay.apply_preset(0));
    }

    /// Every section gets the same wash: one colour, one opacity, resolved to
    /// a real colour because a terminal cell background has no alpha.
    #[test]
    fn one_colour_washes_every_panel_at_the_configured_opacity() {
        let _g = ui_guard();
        let _restore = UiRestore;
        sandbox_state_dir();
        let mut app = App::new();
        let area = ratatui::layout::Rect::new(0, 0, 80, 24);

        // Follows the scheme by default, which is what keeps a washed UI
        // looking like the theme instead of like a filter over it.
        assert_eq!(app.ui.panel_tint, None);
        assert_eq!(app.ui.panel_tint_label(), "theme's own");

        // A flat desktop: the panels are that colour with the tint mixed in.
        app.ui.background = settings::Background::Color((100, 100, 100));
        app.ui.panel_tint = Some((0, 0, 0));
        app.ui.background_tint = 50;
        app.publish_backdrop(area);
        assert_eq!(
            views::theme::panel_fill(),
            Some(ratatui::style::Color::Rgb(50, 50, 50)),
            "half way to the tint"
        );
        // And every panel paints it — the drawers, the rack, the transport and
        // the monitor all go through this one style.
        assert_eq!(
            views::theme::panel_style().bg,
            Some(ratatui::style::Color::Rgb(50, 50, 50))
        );

        // Opacity 0 is the desktop untouched; 100 hides it entirely.
        app.ui.background_tint = 0;
        app.publish_backdrop(area);
        assert_eq!(
            views::theme::panel_fill(),
            Some(ratatui::style::Color::Rgb(100, 100, 100))
        );
        app.ui.background_tint = 100;
        app.publish_backdrop(area);
        assert_eq!(
            views::theme::panel_fill(),
            Some(ratatui::style::Color::Rgb(0, 0, 0))
        );

        // The colour steps through the palette and wraps back to the theme's.
        app.ui.panel_tint = None;
        app.ui.step_panel_tint(1);
        assert_eq!(app.ui.panel_tint, Some(settings::PALETTE[0].1));
        app.ui.step_panel_tint(-1);
        assert_eq!(
            app.ui.panel_tint, None,
            "and back round to the scheme's own"
        );

        views::theme::set_panel_fill(None);
    }

    /// The `A→M` dry/wet appears with the converter and not before: off, there
    /// is nothing to mix. It starts at 100 % — the instrument alone, which is
    /// what the converter did when there was no choice — and the click walks it
    /// down and round.
    #[test]
    fn the_converter_has_a_wet_control_once_it_is_on() {
        let _g = ui_guard();
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.active_slot = 0;
        app.slots[0].in_pair = Some((0, 1));

        let wet_button = |app: &mut App| {
            let (_, rack) = render_rack(app, 140, 30);
            rack.buttons
                .iter()
                .find(|(b, _)| *b == RackButton::PitchMix)
                .map(|&(_, r)| r)
        };
        assert!(
            wet_button(&mut app).is_none(),
            "no converter, nothing to mix"
        );

        app.toggle_pitch_to_midi();
        assert!(
            wet_button(&mut app).is_some(),
            "the wet control comes with the converter"
        );
        assert_eq!(app.slots[0].pitch_mix, 1.0, "and starts on the instrument");

        // The click walks it down in quarters and wraps back to the top.
        app.step_pitch_mix(-0.25, true);
        assert!((app.slots[0].pitch_mix - 0.75).abs() < 1e-6);
        for _ in 0..3 {
            app.step_pitch_mix(-0.25, true);
        }
        assert!(
            app.slots[0].pitch_mix.abs() < 1e-6,
            "down to the input alone"
        );
        app.step_pitch_mix(-0.25, true);
        assert!(
            (app.slots[0].pitch_mix - 1.0).abs() < 1e-6,
            "and round again"
        );

        // The wheel clamps rather than wrapping.
        app.step_pitch_mix(0.5, false);
        assert!((app.slots[0].pitch_mix - 1.0).abs() < 1e-6);
    }

    /// Moving a knob must not rebuild the chain for an effect that can take the
    /// value live. A rebuild replaces every processor in the slot, and a
    /// replaced processor has no buffer: the delay loses its echoes, the space
    /// echo its tape, the granular clouds their grains. That is what "the
    /// slider cuts the audio" was.
    #[test]
    fn a_knob_does_not_rebuild_an_effect_that_takes_it_live() {
        let _g = ui_guard();
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.active_slot = 0;

        // The effects the user reported, and they all take values live.
        for kind in [
            source::AudioFxKind::Delay,
            source::AudioFxKind::SpaceEcho,
            source::AudioFxKind::Z5Texture,
            source::AudioFxKind::Protocosmos,
            source::AudioFxKind::Reverb,
        ] {
            app.fx_chain = vec![source::AudioFxEntry::new(kind)];
            app.fx_slot = 0;
            app.fx_param = 0;
            app.fx_dirty = false;
            adjust_fx_param(&mut app, 0.05);
            assert!(
                !app.fx_dirty,
                "{kind:?} takes values live, so a knob must not rebuild the chain"
            );
        }

        // And the ones that really are built at construction still say so.
        app.fx_chain = vec![source::AudioFxEntry::new(source::AudioFxKind::FilterBank)];
        app.fx_slot = 0;
        app.fx_param = 0;
        app.fx_dirty = false;
        adjust_fx_param(&mut app, 0.05);
        assert!(
            app.fx_dirty,
            "a built-at-construction effect still needs its rebuild"
        );
    }

    /// A tab fed by audio gets a trim and a sensitivity, and neither exists on
    /// a tab playing its own instrument — there is nothing coming in to trim.
    #[test]
    fn an_audio_input_brings_its_own_trim_and_sensitivity() {
        use views::fx_chain_panel::{gate_from_norm, gate_norm};
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        assert_eq!(app.in_trim_state(), None, "no input, no trim");
        app.adjust_in_trim(0.5, 0.5);
        assert_eq!(app.slots[0].in_gain, 1.0, "and the knobs do nothing");

        app.slots[0].in_pair = Some((4, 4));
        assert_eq!(
            app.in_trim_state(),
            Some((1.0, choz_engine::pitch::DEFAULT_GATE))
        );

        // A guitar is quieter than a synth and a microphone is quieter still,
        // so the trim has real range: a quarter turn is a quarter of +24 dB.
        app.adjust_in_trim(0.25, 0.0);
        assert_eq!(app.slots[0].in_gain, 1.0 + 0.25 * MAX_IN_GAIN);
        app.adjust_in_trim(2.0, 0.0);
        assert_eq!(app.slots[0].in_gain, MAX_IN_GAIN, "and it stops at the top");

        // The gate is a level, so the knob is in dB and the ends are the ends.
        let before = gate_norm(app.slots[0].in_gate);
        app.adjust_in_trim(0.0, 0.1);
        assert!(gate_norm(app.slots[0].in_gate) > before, "a stiffer gate");
        app.adjust_in_trim(0.0, -5.0);
        assert!(gate_norm(app.slots[0].in_gate) < 1e-6, "and it bottoms out");
        assert!(
            (gate_norm(gate_from_norm(0.42)) - 0.42).abs() < 1e-4,
            "the knob round-trips"
        );

        // Both are learn targets, so both are automatable and both survive a
        // project round trip.
        app.set_in_trim(Some(1.25), Some(0.5));
        let auto = app.automatable();
        assert!(auto.iter().any(|(t, _)| *t == LearnTarget::InGain(0)));
        assert!(auto.iter().any(|(t, _)| *t == LearnTarget::InGate(0)));
        let saved = app.project_snapshot();
        assert_eq!(saved.rack[0].mixer.in_gain, Some(1.25));
        assert_eq!(saved.rack[0].mixer.in_gate, Some(gate_from_norm(0.5)));
    }

    /// The whole way from a drawer row to the rack: the capture ports are
    /// listed under the card that owns them, and picking one lands on the tab.
    ///
    /// This is the path that was broken — the rows were drawn and nothing
    /// happened when they were picked, because nothing here was ever exercised
    /// without a JACK client.
    #[test]
    fn picking_a_capture_row_feeds_the_active_tab() {
        let _g = ui_guard();
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.in_ports = vec![
            "alsa_input.usb-UMC1820:capture_AUX0".into(),
            "alsa_input.usb-UMC1820:capture_AUX1".into(),
            "alsa_input.pci-HDA:capture_FL".into(),
        ];

        let rows = app.in_targets();
        let cards: Vec<&str> = rows
            .iter()
            .filter(|(t, r)| *t == InTarget::None && r.header)
            .map(|(_, r)| r.name.as_str())
            .collect();
        assert!(
            cards.contains(&"alsa_input.usb-UMC1820") && cards.contains(&"alsa_input.pci-HDA"),
            "one header per card: {cards:?}"
        );
        let ch2 = rows
            .iter()
            .position(|(t, _)| *t == InTarget::Channel(1))
            .expect("channel 2");
        assert!(
            rows[ch2].1.name.starts_with("2  capture_AUX1"),
            "{}",
            rows[ch2].1.name
        );

        // Enter on it feeds the tab, and the RACK stops naming an instrument
        // that is not being heard.
        app.in_select(ch2);
        assert_eq!(app.slots[0].in_pair, Some((1, 1)));
        assert!(
            app.instrument_label().contains("AUDIO IN 2"),
            "{}",
            app.instrument_label()
        );

        // A second channel makes it stereo, and Enter again takes it off.
        let ch3 = app
            .in_targets()
            .iter()
            .position(|(t, _)| *t == InTarget::Channel(2))
            .unwrap();
        app.in_select(ch3);
        assert_eq!(app.slots[0].in_pair, Some((1, 2)));
        app.in_select(ch3);
        assert_eq!(app.slots[0].in_pair, Some((1, 1)));
        // And the last one puts the tab back on its instrument.
        app.in_select_side(ch2, Assign::Off);
        assert_eq!(app.slots[0].in_pair, None);
    }

    /// Synced, the arpeggiator counts the transport's grid instead of its own
    /// clock: a step is due when the song position crosses one, not when a
    /// duration has gone by. That is what keeps two tabs in phase with each
    /// other and with a tempo-synced plugin, and what a busy UI thread cannot
    /// drag off the beat.
    #[test]
    fn a_synced_arp_follows_the_transport_grid_and_falls_back_when_it_stops() {
        use arp::{ArpEvent, ArpSettings, TimeDiv};
        // The transport is process-global, so this test and every other one
        // that touches it take turns.
        let _g = ui_guard();
        let clock = choz_ports::transport();
        clock.set_sample_rate(48_000);
        clock.set_bpm(120.0);
        clock.rewind();
        clock.set_playing(true);

        let mut a = arp::Arp::new(ArpSettings {
            on: true,
            sync: true,
            div: TimeDiv::Quarter,
            ..Default::default()
        });
        // It counts the transport's tempo, whatever its own number says.
        assert_eq!(a.bpm(), 120.0);

        let now = std::time::Instant::now();
        a.note_on(60, 100, now);
        let mut out = Vec::new();
        a.tick(now, &mut out);
        assert!(
            out.iter().any(|e| matches!(e, ArpEvent::On { .. })),
            "the first step is where the playhead already is"
        );

        // Time on the wall passing is not a step: the grid has not moved.
        out.clear();
        a.tick(now + std::time::Duration::from_millis(400), &mut out);
        assert!(
            !out.iter().any(|e| matches!(e, ArpEvent::On { .. })),
            "a step fired without the transport moving: {out:?}"
        );

        // Half a second at 120 BPM is one quarter note, so now it is due — and
        // it is due at the same instant, because the beat says so.
        clock.advance(24_000);
        a.tick(now, &mut out);
        assert!(
            out.iter().any(|e| matches!(e, ArpEvent::On { .. })),
            "the grid moved and nothing fired"
        );

        // The tempo follows the transport while synced.
        clock.set_bpm(200.0);
        assert_eq!(a.bpm(), 200.0);

        // Stopped, there is no grid to lock to — somebody holding a chord still
        // wants to hear it, so the arpeggiator free-runs at that tempo.
        clock.set_playing(false);
        out.clear();
        a.tick(now, &mut out);
        a.tick(now + std::time::Duration::from_secs(1), &mut out);
        assert!(
            out.iter().any(|e| matches!(e, ArpEvent::On { .. })),
            "a stopped transport must not silence the keys"
        );

        clock.set_bpm(choz_ports::Transport::DEFAULT_BPM);
        clock.rewind();
    }

    /// A tab can end somewhere other than its own instrument: the OUT drawer
    /// lists the MIDI ports, picking one binds the active tab, and picking it
    /// again unbinds it. That is the destination the arpeggiator exists for —
    /// a desk of hardware with no arpeggiator of its own.
    #[test]
    fn a_tab_can_be_pointed_at_a_midi_port() {
        let _g = ui_guard();
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.slots.push(RackSlot::new(AudioSource::Midi));
        // The machine running the tests may have no MIDI ports at all, so the
        // list is stubbed: what is under test is the routing, not midir.
        app.midi_out_ports = vec!["Synth A".to_string(), "Synth B".to_string()];

        let rows = app.out_targets();
        let port_row = rows
            .iter()
            .position(|(t, _)| *t == OutTarget::MidiOut(1))
            .expect("the ports are listed");
        assert!(
            rows.iter()
                .any(|(_, r)| r.header && r.label.starts_with("MIDI OUT")),
            "under a heading of their own"
        );

        app.out_select(port_row);
        assert_eq!(app.slots[0].midi_out.as_deref(), Some("Synth B"));
        // The row says so, and says which tab — the same way a channel row does.
        let rows = app.out_targets();
        assert_eq!(rows[port_row].1.mark, '\u{2713}');
        assert!(rows[port_row].1.label.contains("tab 1"));

        // The second tab can share it, and each is remembered on its own.
        app.active_slot = 1;
        app.out_select(port_row);
        assert_eq!(app.slots[1].midi_out.as_deref(), Some("Synth B"));
        assert_eq!(app.slots[0].midi_out.as_deref(), Some("Synth B"));
        assert!(app.out_targets()[port_row].1.label.contains("tab 1,2"));

        // Picking it again is how it is taken off: there is no other gesture.
        app.out_select(port_row);
        assert_eq!(app.slots[1].midi_out, None);
        assert_eq!(app.slots[0].midi_out.as_deref(), Some("Synth B"));
    }

    /// An outside MIDI clock moves choz's transport — but only when it was
    /// asked to. A port that sends clock all day must not take the tempo over
    /// the moment it is plugged in.
    #[test]
    fn an_outside_clock_drives_the_transport_only_when_it_is_switched_on() {
        let _g = ui_guard();
        let clock = choz_ports::transport();
        clock.set_bpm(120.0);
        clock.set_playing(false);
        clock.rewind();

        let mut app = App::new();
        app.ui.midi_clock = false;

        // Off: the wire talks and nothing listens.
        let feed = |app: &mut App, msg: midi::ClockMsg| {
            app.note_tx.send(midi::InputEvent::Clock(msg)).unwrap();
            app.drain_midi();
        };

        feed(&mut app, midi::ClockMsg::Tempo(90.0));
        assert_eq!(clock.bpm(), 120.0);
        assert!(!app.playing);

        app.ui.midi_clock = true;
        feed(&mut app, midi::ClockMsg::Tempo(90.0));
        assert_eq!(clock.bpm(), 90.0);

        // START is the one that also rewinds: it means "from the top", which is
        // the whole difference between it and CONTINUE.
        clock.advance(48_000);
        feed(&mut app, midi::ClockMsg::Start);
        assert!(app.playing);
        assert_eq!(clock.samples(), 0);

        feed(&mut app, midi::ClockMsg::Stop);
        assert!(!app.playing);
        clock.advance(48_000);
        feed(&mut app, midi::ClockMsg::Continue);
        assert!(app.playing);
        assert_eq!(clock.samples(), 48_000, "CONTINUE keeps the position");

        clock.set_bpm(choz_ports::Transport::DEFAULT_BPM);
        clock.set_playing(false);
        clock.rewind();
    }

    /// TAP sets the tempo by playing it, which is the one control that cannot
    /// be a knob: a tap is a gesture, and a gesture has no position to turn to.
    #[test]
    fn tapping_four_times_sets_the_tempo() {
        use views::fx_chain_panel::RackButton as B;
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.edit_arp(ArpEdit::Toggle);

        let (_, rack) = render_rack(&mut app, 120, 24);
        let tap = rack
            .buttons
            .iter()
            .find(|(b, _)| *b == B::ArpTap)
            .expect("TAP is drawn whenever the arpeggiator is on")
            .1;

        // Four taps a fifth of a second apart is 300 BPM — the top of the
        // range, and a number nothing else in this test could have written.
        for _ in 0..4 {
            handle_mouse(
                &mut app,
                MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: tap.x + 1,
                    row: tap.y,
                    modifiers: crossterm::event::KeyModifiers::NONE,
                },
            );
        }
        let bpm = app.slots[0].arp.settings.bpm;
        assert!(bpm > 120.0, "tapping did not move the tempo: {bpm}");
    }

    /// Every arpeggiator control is reachable and changeable **without a
    /// mouse**: `k` hands it the arrows, the arrows walk it, Enter opens the
    /// list of a knob that has names, and the list is picked with the arrows
    /// too. The wheel was the only way in, which on a machine without one meant
    /// no way in at all.
    #[test]
    fn the_arp_is_driven_from_the_keyboard_alone() {
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.edit_arp(ArpEdit::Toggle);
        app.focus = Focus::FxChain;
        render_rack(&mut app, 120, 24);

        // `k` cycles the boxes; with no instrument knobs the next one is the
        // arpeggiator's.
        handle_fx_keys(&mut app, KeyCode::Char('k'));
        assert_eq!(app.rack_focus, RackFocus::Arp);

        // Walk to MODE and open its list.
        let mode = app
            .arp_knobs()
            .iter()
            .position(|(_, n, ..)| *n == "MODE")
            .unwrap();
        app.arp_param = 0;
        for _ in 0..mode {
            handle_fx_keys(&mut app, KeyCode::Right);
        }
        assert_eq!(app.arp_param, mode);
        handle_fx_keys(&mut app, KeyCode::Enter);
        let modal = app.modal.as_ref().expect("Enter opens the list");
        assert_eq!(modal.kind, ModalKind::ArpChoice);
        assert_eq!(modal.list.items.len(), arp::ArpMode::ALL.len());

        // Pick the fourth mode with the arrows and Enter.
        for _ in 0..3 {
            app.modal.as_mut().unwrap().list.move_cursor(1);
        }
        app.modal_select();
        app.close_modal();
        assert_eq!(app.slots[0].arp.settings.mode, arp::ArpMode::ALL[3]);

        // The octave range is a list too — four positions, and the list is how
        // they are reached without a wheel.
        let oct = app
            .arp_knobs()
            .iter()
            .position(|(_, n, ..)| *n == "OCT")
            .unwrap();
        app.arp_param = oct;
        handle_fx_keys(&mut app, KeyCode::Enter);
        let modal = app.modal.as_ref().expect("OCT opens its list");
        assert_eq!(modal.list.items, vec!["1", "2", "3", "4"]);
        app.modal.as_mut().unwrap().list.cursor = 2;
        app.modal_select();
        app.close_modal();
        assert_eq!(app.slots[0].arp.settings.octaves, 3);

        // A knob that is a number, not a list, moves with `w` and `s` instead.
        let gate = app
            .arp_knobs()
            .iter()
            .position(|(_, n, ..)| *n == "GATE")
            .unwrap();
        app.arp_param = gate;
        let before = app.slots[0].arp.settings.gate;
        handle_fx_keys(&mut app, KeyCode::Char('w'));
        assert!(app.slots[0].arp.settings.gate > before);
        handle_fx_keys(&mut app, KeyCode::Char('s'));
        assert!((app.slots[0].arp.settings.gate - before).abs() < 1e-6);

        // A switch has no list worth opening: Enter flips it.
        let latch = app
            .arp_knobs()
            .iter()
            .position(|(_, n, ..)| *n == "HOLD")
            .unwrap();
        app.arp_param = latch;
        handle_fx_keys(&mut app, KeyCode::Enter);
        assert!(app.modal.is_none(), "a toggle does not open a list");
        assert!(app.slots[0].arp.settings.latch);

        // And on a panel too short for the knob box the keyboard still gets
        // there — the controls are buttons, but they are the same controls.
        let (_, rack) = render_rack(&mut app, 100, 12);
        assert!(rack.arp_knobs.is_empty(), "this is the button shape");
        app.rack_focus = RackFocus::Fx;
        handle_fx_keys(&mut app, KeyCode::Char('k'));
        assert_eq!(app.rack_focus, RackFocus::Arp);
    }

    /// The arpeggiator's controls take the shape the screen can afford: a
    /// bordered knob box where there are rows for it, the same knobs without
    /// their frame two rows cheaper, and the row of buttons on a panel that has
    /// neither. Nothing is ever missing — only its shape changes.
    #[test]
    fn the_arp_controls_take_the_shape_the_screen_can_afford() {
        use views::fx_chain_panel::RackButton as B;
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.edit_arp(ArpEdit::Toggle);

        // A rack with rows to spare: knobs, and no buttons standing in for
        // them. (The panel here is the whole frame, so these are the rows the
        // RACK itself gets, not the terminal's.)
        let (screen, rack) = render_rack(&mut app, 120, 24);
        assert_eq!(
            rack.arp_knobs.len(),
            app.arp_knobs().len(),
            "every control is a knob:\n{screen}"
        );
        assert!(
            !rack.buttons.iter().any(|(b, _)| *b == B::ArpGate),
            "the buttons the knobs replace are gone"
        );
        assert!(screen.contains("GATE"), "the knobs are named:\n{screen}");
        assert!(
            screen.contains("ARP [k]"),
            "the bordered box says which key hands it the arrows:\n{screen}"
        );

        // A five-inch screen — an 800x480 panel at a readable font leaves the
        // rack a dozen-odd rows — still has the knobs, without their frame.
        let (screen, small) = render_rack(&mut app, 100, 16);
        assert!(!small.arp_knobs.is_empty(), "{screen}");
        // Only one row of them fits there, so the box scrolls with the cursor —
        // the same way the FX and instrument boxes do. Whatever the arrows are
        // on is on screen, which is what makes the ones below reachable.
        let last = app.arp_knobs().len() - 1;
        app.arp_param = last;
        let (screen, scrolled) = render_rack(&mut app, 100, 16);
        assert!(
            scrolled.arp_knobs.iter().any(|(i, _)| *i == last),
            "the last knob is out of reach:\n{screen}"
        );
        app.arp_param = 0;

        // And a squeezed one falls back to the buttons rather than dropping
        // controls on the floor.
        let (_, tiny) = render_rack(&mut app, 100, 12);
        assert!(tiny.arp_knobs.is_empty());
        assert!(tiny.buttons.iter().any(|(b, _)| *b == B::ArpGate));

        // Whatever the shape, nothing is drawn off the panel.
        for r in tiny
            .buttons
            .iter()
            .map(|(_, r)| r)
            .chain(small.arp_knobs.iter().map(|(_, r)| r))
        {
            assert!(r.x + r.width <= 100, "drawn past the panel: {r:?}");
        }
    }

    /// A knob is turned the way every other knob in the rack is: click to take
    /// the cursor, wheel to move it, and a second click steps a named position.
    #[test]
    fn the_arp_knobs_turn_with_the_mouse() {
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.edit_arp(ArpEdit::Toggle);
        let (_, rack) = render_rack(&mut app, 120, 24);

        let knobs = app.arp_knobs();
        let index = |name: &str| {
            knobs
                .iter()
                .position(|(_, n, ..)| *n == name)
                .unwrap_or_else(|| panic!("no {name} knob"))
        };
        let (i_bpm, i_mode, i_gate) = (index("BPM"), index("MODE"), index("GATE"));
        let rect = |rack: &RackLayout, i: usize| {
            rack.arp_knobs
                .iter()
                .find(|(k, _)| *k == i)
                .expect("knob is on screen")
                .1
        };

        // The wheel over BPM moves the tempo and nothing else.
        let bpm = rect(&rack, i_bpm);
        let before = app.slots[0].arp.settings.bpm;
        handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::ScrollUp,
                column: bpm.x + 1,
                row: bpm.y,
                modifiers: crossterm::event::KeyModifiers::NONE,
            },
        );
        assert!(app.slots[0].arp.settings.bpm > before);

        // Clicking a knob takes the arrows; clicking it again steps it — which
        // for PLAY means the sequencer, the same as the button did.
        let play = rect(&rack, i_mode);
        let click = |app: &mut App| {
            handle_mouse(
                app,
                MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: play.x + 1,
                    row: play.y,
                    modifiers: crossterm::event::KeyModifiers::NONE,
                },
            );
        };
        // `w` and `s` turn whichever knob the arrows are on. Before the play
        // mode is touched: in the sequencer there is no MODE knob, so every
        // index after it means a different control — which is exactly why the
        // panel addresses them by what they are.
        app.rack_focus = RackFocus::Arp;
        app.arp_param = i_gate;
        let gate = app.slots[0].arp.settings.gate;
        handle_fx_keys(&mut app, KeyCode::Char('w'));
        assert!(app.slots[0].arp.settings.gate > gate);

        click(&mut app);
        assert_eq!(app.rack_focus, RackFocus::Arp);
        assert_eq!(app.slots[0].arp.settings.mode, arp::ArpMode::Up);

        // A second click on the one under the cursor opens its list rather than
        // walking it: eight modes with a wheel is fine, eight modes with a
        // keyboard is not.
        click(&mut app);
        let modal = app.modal.as_ref().expect("the picker opened");
        assert_eq!(modal.kind, ModalKind::ArpChoice);
        assert_eq!(modal.list.items.len(), arp::ArpMode::ALL.len());
        app.modal.as_mut().unwrap().list.cursor = 1;
        app.modal_select();
        assert_eq!(app.slots[0].arp.settings.mode, arp::ArpMode::ALL[1]);
        app.close_modal();
    }

    /// With everything switched on, the arpeggiator's row no longer runs off
    /// the panel: it wraps, and every button is still on screen and clickable.
    ///
    /// This is the roadmap's own note — the ARP line stopped fitting in 120
    /// columns once the sequencer arrived, and a button drawn past the right
    /// edge is a button that does not exist.
    #[test]
    fn the_arp_row_wraps_instead_of_running_off_the_panel() {
        use views::fx_chain_panel::RackButton as B;
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.edit_arp(ArpEdit::Toggle);
        app.edit_arp(ArpEdit::Swing);
        assert!(app.slots[0].arp.settings.on);

        // 80 columns and few rows: the shape with no room for knobs, which is
        // the one that has to wrap.
        let (_, rack) = render_rack(&mut app, 80, 14);
        let arp: Vec<(B, ratatui::layout::Rect)> = rack
            .buttons
            .iter()
            .copied()
            .filter(|(b, _)| {
                matches!(
                    b,
                    B::ArpTap
                        | B::ArpMode
                        | B::ArpDiv
                        | B::ArpRateDown
                        | B::ArpRateUp
                        | B::ArpSync
                        | B::ArpGate
                        | B::ArpSwing
                        | B::ArpOctaves
                        | B::ArpLatch
                        | B::ArpChord
                )
            })
            .collect();
        // Eleven: the arpeggiator's own controls. Its on/off is not among them
        // any more — that switch is the button on the ALGO row above.
        assert_eq!(arp.len(), 11, "every switch is drawn: {arp:?}");
        for (b, r) in &arp {
            assert!(r.x + r.width <= 80, "{b:?} runs past the panel: {r:?}");
        }
        assert!(
            arp.iter()
                .map(|(_, r)| r.y)
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                > 1,
            "the row did not wrap, so something is being drawn off screen"
        );

        // Clicking the wrapped ones still reaches them.
        let latch = arp.iter().find(|(b, _)| *b == B::ArpLatch).unwrap().1;
        let before = app.slots[0].arp.settings.latch;
        handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: latch.x + 1,
                row: latch.y,
                modifiers: crossterm::event::KeyModifiers::NONE,
            },
        );
        assert_ne!(app.slots[0].arp.settings.latch, before);
    }

    /// The IN drawer scrolls with its cursor, and the click rects come from the
    /// same window the panel paints.
    ///
    /// This is the gotcha the roadmap flagged: the drawer listed every capture
    /// port of every card with no way to move, so on a short terminal the last
    /// rows were painted off the panel and there was nothing to click.
    #[test]
    fn the_in_drawer_scrolls_and_its_rects_follow_what_is_painted() {
        use views::source_panel as sp;
        let _g = ui_guard();
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.midi_ports = (0..20).map(|i| format!("Port {i}")).collect();
        app.in_open = true;

        // 12 rows tall: 10 inside the borders, 3 of them headers → 7 list rows.
        let area = ratatui::layout::Rect::new(0, 0, 34, 12);
        let height = views::drawer::list_height(area, sp::INPUT_LIST_TOP, 0);
        assert_eq!(height, 7);
        let rows = app.in_targets();
        assert!(rows.len() > height, "the list has to overflow to be a test");

        let recompute = |app: &App| {
            compute_layout(
                app,
                area,
                area,
                ratatui::layout::Rect::new(34, 0, 40, 12),
                ratatui::layout::Rect::new(34, 12, 40, 4),
                ratatui::layout::Rect::new(74, 0, 6, 12),
            );
            app.layout.borrow().input_item_rects.clone()
        };

        // At the top: the first screenful, and nothing for what is below it.
        let rects = recompute(&app);
        assert_eq!(rects.len(), height);
        assert_eq!(rects.first().unwrap().0, 0);
        assert_eq!(rects.last().unwrap().0, height - 1);

        // At the bottom: the last row is reachable, on the last line of the
        // list — which is what was impossible before.
        app.input_cursor = rows.len() - 1;
        let rects = recompute(&app);
        assert_eq!(rects.last().unwrap().0, rows.len() - 1);
        let bottom = rects.last().unwrap().1;
        assert_eq!(bottom.y as usize, 1 + sp::INPUT_LIST_TOP + height - 1);

        // And the panel paints that row where the rect says it is. The two used
        // to be computed apart, which is how a click lands on the wrong port.
        let mut term = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
        let labels: Vec<sp::InputRow> = rows
            .iter()
            .map(|(_, r)| sp::InputRow {
                kind: r.kind,
                name: r.name.clone(),
                connected: r.connected,
                bound_tab: r.bound_tab,
                header: r.header,
            })
            .collect();
        term.draw(|f| {
            sp::draw_input_panel(f, area, true, &labels, app.input_cursor, "tab 1", None);
        })
        .unwrap();
        let buf = term.backend().buffer();
        let line: String = (bottom.x..bottom.x + bottom.width)
            .map(|x| buf[(x, bottom.y)].symbol().to_string())
            .collect();
        assert!(
            line.contains(&labels[rows.len() - 1].name),
            "the bottom rect is not on the row it answers for: {line:?}"
        );
    }

    /// The two mouse buttons are the two halves of the gesture: left puts a
    /// channel on the tab, right takes it off. Nowhere else does the right
    /// button mean anything.
    #[test]
    fn the_right_button_takes_a_channel_off() {
        let layout = UiLayout {
            output_area: Rect::new(60, 0, 20, 10),
            output_item_rects: vec![(3, Rect::new(61, 4, 18, 1))],
            source_area: Rect::new(0, 0, 20, 10),
            input_item_rects: vec![(2, Rect::new(1, 5, 18, 1))],
            ..Default::default()
        };

        let down = |btn| MouseEventKind::Down(btn);
        assert!(matches!(
            mouse_action(62, 4, &layout, down(MouseButton::Left)),
            MouseAction::OutputDevice(3)
        ));
        assert!(matches!(
            mouse_action(62, 4, &layout, down(MouseButton::Right)),
            MouseAction::OutputUnassign(3)
        ));
        assert!(matches!(
            mouse_action(2, 5, &layout, down(MouseButton::Right)),
            MouseAction::InputUnassign(2)
        ));
        // Off the rows, the right button does nothing at all.
        assert!(matches!(
            mouse_action(62, 9, &layout, down(MouseButton::Right)),
            MouseAction::None
        ));
        assert!(matches!(
            mouse_action(40, 4, &layout, down(MouseButton::Right)),
            MouseAction::None
        ));
    }

    /// One input jack is one input: a guitar in 5 feeds the tab in mono, and
    /// taking the last one off puts the tab back on its instrument.
    #[test]
    fn an_input_channel_can_be_picked_on_its_own() {
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));

        app.set_active_capture(Some((4, 4)));
        assert_eq!(app.slots[0].in_pair, Some((4, 4)));
        assert!(
            app.instrument_label().contains("AUDIO IN 5"),
            "one jack is one number: {}",
            app.instrument_label()
        );
        assert!(!app.instrument_label().contains("5/5"));

        // A second jack makes it a stereo capture of 5 and 7.
        app.set_active_capture(Some(assign_channel(app.slots[0].in_pair.unwrap(), 6)));
        assert_eq!(app.slots[0].in_pair, Some((4, 6)));
        assert!(
            app.instrument_label().contains("5/7"),
            "{}",
            app.instrument_label()
        );

        // Off they come, and with the last one the tab is on its instrument.
        app.set_active_capture(unassign_channel(app.slots[0].in_pair.unwrap(), 6));
        assert_eq!(app.slots[0].in_pair, Some((4, 4)));
        app.set_active_capture(unassign_channel(app.slots[0].in_pair.unwrap(), 4));
        assert_eq!(app.slots[0].in_pair, None, "back to the instrument");
    }

    /// The cursor never lands on a section header.
    #[test]
    fn the_out_cursor_steps_over_headers() {
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.out_devices = vec!["UMC1820".into()];
        app.out_cursor = 1; // the device row
        let next = out_step(&app, 1);
        assert_eq!(
            app.out_targets()[next].0,
            OutTarget::Channel(0),
            "stepping down skips the CHANNELS header"
        );
        assert_eq!(out_step(&app, -1), 1, "and stops at the first row going up");
    }

    /// Both open drawers carry an ✕ on their outer edge, and clicking it shuts
    /// that drawer — the mouse-only way back to a full-width RACK.
    #[test]
    fn the_close_button_shuts_its_drawer() {
        let _g = ui_guard();
        let mut app = App::new();
        app.splash_done = true;
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.in_open = true;
        app.out_open = true;

        let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
        term.draw(|f| ui(f, &mut app)).unwrap();
        let (in_x, out_x) = {
            let l = app.layout.borrow();
            (
                l.in_close_rect.expect("IN draws a close button"),
                l.out_close_rect.expect("OUT draws a close button"),
            )
        };
        // On the outer edge of each drawer, not floating in the middle.
        // Top-right corner of each drawer, clear of the panel title.
        assert_eq!(
            in_x.y,
            app.layout.borrow().source_area.y,
            "on the top border"
        );
        assert!(
            in_x.x > app.layout.borrow().source_area.x + 4,
            "IN's ✕ is at its right"
        );
        assert!(
            out_x.x > app.layout.borrow().output_area.x + 4,
            "OUT's ✕ is at its right"
        );

        click(&mut app, in_x.x, in_x.y);
        assert!(!app.in_open, "clicking ✕ shut IN");

        // Shutting IN re-flows the body, so OUT's button has moved: read it
        // from the fresh layout rather than the stale rect.
        term.draw(|f| ui(f, &mut app)).unwrap();
        let out_x = app
            .layout
            .borrow()
            .out_close_rect
            .expect("OUT still has its ✕");
        click(&mut app, out_x.x, out_x.y);
        assert!(!app.out_open, "clicking ✕ shut OUT");
    }

    fn click(app: &mut App, x: u16, y: u16) {
        handle_mouse(
            app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: x,
                row: y,
                modifiers: crossterm::event::KeyModifiers::NONE,
            },
        );
    }

    /// Loading a plugin blocks this thread for as long as the plugin takes
    /// (Surge XT reads its whole factory library), so what is asked for is
    /// promised, drawn, and only then run: the frame that says "loading" has to
    /// reach the terminal before the thread goes quiet.
    #[test]
    fn a_load_says_so_on_screen_before_it_blocks_the_thread() {
        let _g = ui_guard();
        let mut app = App::new();
        app.splash_done = true;
        app.slots.push(RackSlot::new(AudioSource::Midi));

        app.request_load_source("/nowhere/Grand Piano.sf2".into());
        assert!(app.pending_load.is_some(), "the load is only promised");

        let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
        term.draw(|f| ui(f, &mut app)).unwrap();
        let screen: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            screen.contains("Grand Piano.sf2"),
            "the name of what is loading is on screen:\n{screen}"
        );

        // Only now does the work happen — and the message goes with it.
        app.run_pending_load();
        assert!(app.pending_load.is_none());
        assert!(app.loading.is_none());
    }

    /// The metronome: a switch on the menu bar beside LIVE/MULTI, and a menu
    /// under it for tempo, time signature, sound and level. The click itself is
    /// made in the audio callback off the transport's tempo, so it and a
    /// tempo-synced delay cannot disagree.
    #[test]
    fn the_metronome_switches_and_its_menu_steps_every_setting() {
        let _g = ui_guard();
        let m = choz_engine::metronome::metronome();
        m.set_on(false);
        let mut app = App::new();
        app.splash_done = true;
        app.slots.push(RackSlot::new(AudioSource::Midi));

        let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
        term.draw(|f| ui(f, &mut app)).unwrap();

        let (met, menu) = {
            let l = app.layout.borrow();
            (l.met_rect.expect("MET button"), l.met_menu_rect.unwrap())
        };
        let switch = app.layout.borrow().mode_switch_rect.unwrap();
        assert!(
            met.x + met.width <= switch.x,
            "the click sits beside LIVE/MULTI, not on top of it"
        );

        click(&mut app, met.x + 1, met.y);
        assert!(m.on(), "the button switches the click on");
        handle_key(&mut app, KeyCode::F(6));
        assert!(!m.on(), "and F6 does it from anywhere");

        // The menu steps each row and stays open — setting a tempo means
        // hearing it, and a menu that closes makes that four round trips.
        click(&mut app, menu.x, menu.y);
        assert_eq!(app.modal.as_ref().map(|m| m.kind), Some(ModalKind::Metronome));
        let bpm = choz_ports::transport().bpm();
        app.modal.as_mut().unwrap().list.cursor = 1;
        app.modal_select();
        assert!(
            (choz_ports::transport().bpm() - (bpm + 5.0)).abs() < 0.01,
            "the tempo row steps the transport itself"
        );
        assert!(app.modal.is_some(), "the menu stays open");

        let sig = choz_ports::transport().time_signature();
        app.modal.as_mut().unwrap().list.cursor = 2;
        app.modal_select();
        assert_ne!(choz_ports::transport().time_signature(), sig);

        // The arrows move a row either way — a tempo four presses up is forty
        // presses back the other way if the only direction is forward.
        app.modal.as_mut().unwrap().list.cursor = 1;
        let bpm = choz_ports::transport().bpm();
        handle_modal_key(&mut app, KeyCode::Left);
        assert!(
            (choz_ports::transport().bpm() - (bpm - 5.0)).abs() < 0.01,
            "left takes the tempo down: {}",
            choz_ports::transport().bpm()
        );
        handle_modal_key(&mut app, KeyCode::Right);
        assert!((choz_ports::transport().bpm() - bpm).abs() < 0.01, "and right puts it back");
        assert!(app.modal.is_some(), "still open");

        // …and so does the wheel over the row it is pointing at.
        let row = app
            .layout
            .borrow()
            .modal_rects
            .rows
            .iter()
            .find(|(i, _)| *i == 1)
            .map(|(_, r)| *r);
        if let Some(r) = row {
            handle_modal_mouse(
                &mut app,
                MouseEvent {
                    kind: MouseEventKind::ScrollUp,
                    column: r.x + 1,
                    row: r.y,
                    modifiers: crossterm::event::KeyModifiers::NONE,
                },
            );
            assert!(
                (choz_ports::transport().bpm() - (bpm + 5.0)).abs() < 0.01,
                "the wheel over the tempo row is the tempo, not the cursor"
            );
        }

        let style = m.style();
        app.modal.as_mut().unwrap().list.cursor = 3;
        app.modal_select();
        assert_eq!(m.style(), style.next());

        m.set_on(false);
        choz_ports::transport().set_bpm(120.0);
        choz_ports::transport().set_time_signature(4, 4);
    }

    /// Every size the terminal can be, drawn. A panel that computes a rect from
    /// a width it did not check writes outside the buffer, and ratatui's answer
    /// to that is a panic — which is the whole application gone, mid-set,
    /// because somebody resized a window.
    #[test]
    fn the_interface_draws_at_every_size_without_writing_off_the_buffer() {
        let _g = ui_guard();
        let mut app = App::new();
        app.splash_done = true;
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.slots.push(RackSlot::new(AudioSource::Plugin {
            id: "x".into(),
            format: "VST3".into(),
            name: "Tester".into(),
        }));
        app.slots[1].instr_params = (0..40)
            .map(|i| choz_engine::PluginParam::plain_range(i, format!("P{i}"), 0.0, 1.0, 0.0))
            .collect();
        app.slots[1].instr_values = vec![0.0; 40];

        // The edges, not the whole grid: a terminal one column narrower than a
        // fixed offset is the whole bug, and each of these draws costs an FFT.
        for (i, (w, h)) in [(20u16, 8u16), (20, 24), (40, 12), (60, 38), (80, 24), (213, 58)]
            .into_iter()
            .enumerate()
        {
            for tab in views::midi_monitor::MonitorTab::ALL {
                app.monitor_tab = tab;
                let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
                term.draw(|f| ui(f, &mut app))
                    .unwrap_or_else(|e| panic!("{w}x{h} on {tab:?}: {e}"));
            }
            let _ = i;
        }
    }

    /// A list of numbered slots is not a bank. TyrellN6's VST3 publishes 128
    /// of them (`Program 0`, `Program 1`…), which stood between the player and
    /// the 669 patches u-he had installed on the same machine.
    #[test]
    fn a_bank_that_names_nothing_does_not_count_as_one() {
        assert!(is_placeholder("Program 0"));
        assert!(is_placeholder("program 127"));
        assert!(is_placeholder("Preset 12"));
        assert!(is_placeholder("  "));
        assert!(is_placeholder("7"));
        assert!(!is_placeholder("Angry Dog Bass"));
        assert!(!is_placeholder("Program Material"), "a name that starts with the word is a name");
        assert!(!is_placeholder("Grand Piano"));
    }

    /// The MIXER tab: every rack tab's strip at once, and each one editable
    /// where it is drawn. The RACK only ever shows the active tab's level and
    /// pan, so balancing a set of them meant switching tabs to hear the change
    /// you just made somewhere else.
    #[test]
    fn the_mixer_tab_edits_every_strip_where_it_is_drawn() {
        let _g = ui_guard();
        let mut app = App::new();
        app.splash_done = true;
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.monitor_tab = views::midi_monitor::MonitorTab::Mixer;

        let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
        term.draw(|f| ui(f, &mut app)).unwrap();

        let hits = app.layout.borrow().mixer_hits.clone();
        assert!(!hits.is_empty(), "the MIXER tab draws controls");
        let find = |want: views::midi_monitor::MixerHit| {
            hits.iter()
                .find(|(h, _)| *h == want)
                .map(|(_, r)| *r)
                .unwrap_or_else(|| panic!("no rect for {want:?} in {hits:?}"))
        };

        // The second tab, muted from the mixer while the RACK is still on the
        // first one — the whole point of the panel.
        let m = find(views::midi_monitor::MixerHit::Mute(1));
        click(&mut app, m.x + 1, m.y);
        assert!(app.slots[1].mute, "the strip of tab 2 mutes tab 2");
        assert!(!app.slots[0].mute, "and leaves tab 1 alone");
        assert_eq!(app.active_slot, 0, "muting is not switching tabs");

        // The level track takes the value from where it was clicked: a fader
        // that only nudges is ten clicks wide.
        let g = find(views::midi_monitor::MixerHit::Gain(1));
        click(&mut app, g.x + g.width - 1, g.y);
        assert!(
            app.slots[1].gain > 1.5,
            "clicking the right end of the track is the top of the range: {}",
            app.slots[1].gain
        );
        let p = find(views::midi_monitor::MixerHit::Pan(1));
        click(&mut app, p.x, p.y);
        assert!(app.slots[1].pan < -0.5, "hard left: {}", app.slots[1].pan);

        // And the name switches to that tab, so the mixer is also a tab strip.
        let n = find(views::midi_monitor::MixerHit::Select(1));
        click(&mut app, n.x + 1, n.y);
        assert_eq!(app.active_slot, 1);

        // The strips are columns: the level reads bottom-up, so clicking the
        // top of the track is full and the bottom is off.
        let g = find(views::midi_monitor::MixerHit::Gain(1));
        click(&mut app, g.x + 1, g.y + g.height - 1);
        assert!(app.slots[1].gain < 0.1, "the bottom of a fader is off: {}", app.slots[1].gain);
    }

    /// A tab plays out of two channels, so its strip has two faders and a link
    /// between them: linked they move together (which is what a tab wants
    /// nearly always), broken they trim one side against the other. And the
    /// level moves in the same step wherever it is touched — the wheel over a
    /// fader, the arrows with the MIXER focused, the RACK's own VOL.
    #[test]
    fn a_strip_has_a_fader_per_channel_and_a_link_between_them() {
        let _g = ui_guard();
        let mut app = App::new();
        app.splash_done = true;
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.monitor_tab = views::midi_monitor::MonitorTab::Mixer;

        let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
        term.draw(|f| ui(f, &mut app)).unwrap();
        let find = |app: &App, want: views::midi_monitor::MixerHit| {
            app.layout
                .borrow()
                .mixer_hits
                .iter()
                .find(|(h, _)| *h == want)
                .map(|(_, r)| *r)
                .unwrap_or_else(|| panic!("no rect for {want:?}"))
        };

        // Linked (the default): the wheel over one side moves both.
        let l = find(&app, views::midi_monitor::MixerHit::Gain(1));
        wheel(&mut app, l.x, l.y + 1, true);
        assert!(app.slots[1].gain > 1.0 && app.slots[1].link);
        assert_eq!(
            app.slots[1].gain, app.slots[1].gain_r,
            "linked, the two sides are one fader"
        );
        assert_eq!(app.slots[1].gain, 1.0 + GAIN_STEP, "one click, one step");

        // Break the link and the right side moves on its own.
        let link = find(&app, views::midi_monitor::MixerHit::Link(1));
        click(&mut app, link.x, link.y);
        assert!(!app.slots[1].link);
        term.draw(|f| ui(f, &mut app)).unwrap();
        let r = find(&app, views::midi_monitor::MixerHit::GainR(1));
        let (before_l, before_r) = (app.slots[1].gain, app.slots[1].gain_r);
        wheel(&mut app, r.x, r.y + 1, false);
        assert_eq!(app.slots[1].gain, before_l, "the left side stayed put");
        assert!(
            (app.slots[1].gain_r - (before_r - GAIN_STEP)).abs() < 1e-6,
            "and the right came down one step"
        );

        // The RACK's VOL still moves the whole tab, trim and all.
        let offset = app.slots[1].gain - app.slots[1].gain_r;
        app.active_slot = 1;
        adjust_gain(&mut app, GAIN_STEP);
        assert!(
            (app.slots[1].gain - app.slots[1].gain_r - offset).abs() < 1e-6,
            "an unlinked strip keeps its trim when the tab's own level moves"
        );

        // Focused, the arrows are levels — that is what the MIXER is for.
        app.focus = Focus::Mixer;
        let before = app.slots[1].gain;
        handle_key(&mut app, KeyCode::Up);
        assert!((app.slots[1].gain - (before + GAIN_STEP)).abs() < 1e-6);
        handle_key(&mut app, KeyCode::Down);
        assert!((app.slots[1].gain - before).abs() < 1e-6);

        // Linking again takes the louder of the two: the quiet side was the
        // one being trimmed.
        let loud = app.slots[1].gain.max(app.slots[1].gain_r);
        click(&mut app, link.x, link.y);
        assert!(app.slots[1].link);
        assert_eq!(app.slots[1].gain, loud);
        assert_eq!(app.slots[1].gain_r, loud);
    }

    fn wheel(app: &mut App, x: u16, y: u16, up: bool) {
        handle_mouse(
            app,
            MouseEvent {
                kind: if up {
                    MouseEventKind::ScrollUp
                } else {
                    MouseEventKind::ScrollDown
                },
                column: x,
                row: y,
                modifiers: crossterm::event::KeyModifiers::NONE,
            },
        );
    }

    /// More tabs than fit across the panel: the mixer pages, and the window
    /// follows the active tab rather than keeping a scroll of its own.
    #[test]
    fn the_mixer_pages_when_the_rack_is_wider_than_the_panel() {
        let _g = ui_guard();
        let mut app = App::new();
        app.splash_done = true;
        for _ in 0..12 {
            app.slots.push(RackSlot::new(AudioSource::Midi));
        }
        app.monitor_tab = views::midi_monitor::MonitorTab::Mixer;

        let mut term = Terminal::new(TestBackend::new(80, 30)).unwrap();
        term.draw(|f| ui(f, &mut app)).unwrap();
        let shown = |app: &App| -> Vec<usize> {
            app.layout
                .borrow()
                .mixer_hits
                .iter()
                .filter_map(|(h, _)| match h {
                    views::midi_monitor::MixerHit::Select(i) => Some(*i),
                    _ => None,
                })
                .collect()
        };
        let first = shown(&app);
        assert!(
            !first.is_empty() && first.len() < 12,
            "a 80-column panel cannot show twelve strips: {first:?}"
        );
        assert!(first.contains(&0), "it starts on the active tab");

        // The pager is drawn, and it walks the window.
        let next = app
            .layout
            .borrow()
            .mixer_hits
            .iter()
            .find(|(h, _)| matches!(h, views::midi_monitor::MixerHit::Page(1)))
            .map(|(_, r)| *r)
            .expect("the pager is on screen when the strips do not fit");
        click(&mut app, next.x, next.y);
        term.draw(|f| ui(f, &mut app)).unwrap();
        let after = shown(&app);
        assert!(
            after.first() > first.first(),
            "the window moved on: {first:?} -> {after:?}"
        );
        assert!(
            after.contains(&app.active_slot),
            "and the tab it made active is one of the strips being shown"
        );
    }

    /// The drawers: shut, both edges are handles and the RACK owns the body;
    /// F2/F3 open them and Tab only ever lands on an open one.
    #[test]
    fn side_drawers_open_close_and_take_focus() {
        let _g = ui_guard();
        let mut app = App::new();
        app.splash_done = true;
        app.slots.push(RackSlot::new(AudioSource::Midi));

        let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
        let draw = |app: &mut App, term: &mut Terminal<TestBackend>| {
            term.draw(|f| ui(f, app)).unwrap();
            term.backend()
                .buffer()
                .content()
                .iter()
                .map(|c| c.symbol())
                .collect::<String>()
        };

        let closed = draw(&mut app, &mut term);
        assert!(!closed.contains("INPUTS"), "IN starts shut:\n{closed}");
        assert_eq!(
            app.layout.borrow().source_area.width,
            views::drawer::HANDLE_W
        );
        assert_eq!(
            app.layout.borrow().output_area.width,
            views::drawer::HANDLE_W
        );
        let rack_full = app.layout.borrow().fx_chain_area.width;

        handle_key(&mut app, KeyCode::F(2));
        let open_in = draw(&mut app, &mut term);
        assert!(open_in.contains("INPUTS"), "F2 opens IN:\n{open_in}");
        assert_eq!(app.focus, Focus::Source);
        assert!(
            app.layout.borrow().fx_chain_area.width < rack_full,
            "the RACK gives way"
        );

        handle_key(&mut app, KeyCode::F(3));
        let open_out = draw(&mut app, &mut term);
        assert!(open_out.contains("OUT"), "F3 opens OUT:\n{open_out}");
        assert_eq!(app.focus, Focus::Output);

        // Esc shuts the focused drawer and hands focus back to the RACK.
        handle_key(&mut app, KeyCode::Esc);
        assert!(!app.out_open);
        assert_eq!(app.focus, Focus::FxChain);
    }

    #[test]
    fn tab_never_parks_on_a_closed_drawer() {
        assert_eq!(next_focus(Focus::FxChain, false, false, false), Focus::Transport);
        assert_eq!(next_focus(Focus::Transport, false, false, false), Focus::FxChain);
        assert_eq!(next_focus(Focus::Transport, false, true, false), Focus::Output);
        assert_eq!(next_focus(Focus::Transport, true, false, false), Focus::Source);
        assert_eq!(next_focus(Focus::Output, true, true, false), Focus::Source);
    }
}
