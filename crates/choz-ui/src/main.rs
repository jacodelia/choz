//! choz — A terminal-based audio plugin host inspired by Carla.
//!
//! Provides a TUI for managing audio sources (MIDI, SF2, audio files, plugins)
//! and FX chains, feeding a real-time audio engine via cpal.
//!
//! UI styling adapted from seqterm.

mod i18n;
mod project;
mod settings;
mod source;
mod file_browser;
mod menu;
mod logo;
mod log;
mod views;

use choz_engine::{engine, midi, sources};
use choz_engine::fx_chain::FxSpec;
use choz_engine::registry::PluginRegistry;

use std::io;
use std::cell::RefCell;
use std::time::Instant;

use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind,
            MouseButton, MouseEvent, MouseEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect, Margin},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame, Terminal, backend::CrosstermBackend,
};

use source::{AudioSource, AudioFxEntry, ALL_FX_KINDS};
use views::fx_chain_panel::{RackButton, RackLayout};
use views::theme::*;
use views::splash::{SplashState, draw_splash, is_active};

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

#[derive(Clone)]
struct RackSlot {
    /// Which note input drives this tab. `None` = only the QWERTY piano (which
    /// always plays the active tab) reaches it.
    input: Option<InputRef>,
    source: AudioSource,
    fx_chain: Vec<AudioFxEntry>,
    /// Mixer strip. `solo` is a UI-only concept: it's folded into the mute flag
    /// sent to the engine (any solo → everything else is muted).
    gain: f32,
    pan: f32,
    mute: bool,
    solo: bool,
    /// SF2 slots only: the programs in the loaded SoundFont, and the cursor into
    /// them. Empty for every other source kind.
    presets: Vec<sources::Sf2Preset>,
    preset_cursor: usize,
    /// Plugin-instrument slots only: what the plugin exposes, and the current
    /// knob positions (0..1, same order). Empty for every other source kind.
    instr_params: Vec<choz_engine::ClapParamInfo>,
    instr_values: Vec<f32>,
}

impl RackSlot {
    fn new(source: AudioSource) -> Self {
        RackSlot {
            input: None, source, fx_chain: Vec::new(),
            gain: 1.0, pan: 0.0, mute: false, solo: false,
            presets: Vec::new(), preset_cursor: 0,
            instr_params: Vec::new(), instr_values: Vec::new(),
        }
    }
}

/// A discovered synthesizer plugin.
#[derive(Debug, Clone)]
pub struct SynthEntry {
    pub id: String,
    pub format: String,
    pub name: String,
    pub path: std::path::PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Source,
    FxChain,
    Transport,
}

/// Which picker the open modal is. Every one of them draws through
/// `views::modal::draw_list_modal`, so they share the scrollbar, the
/// SELECT/CANCEL buttons and one set of mouse rects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModalKind {
    /// Instrument for the active rack tab, filtered by plugin/file format.
    Source,
    /// Add an FX to the active chain (built-ins, then CLAP effects).
    AddFx,
    /// Audio output device.
    Device,
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
}

/// Tabs of the Settings modal, in chip order.
const SETTINGS_TABS: &[&str] = &["AUDIO", "COLOR", "LANGUAGE"];
const TAB_AUDIO: usize = 0;
const TAB_COLOR: usize = 1;
const TAB_LANG: usize = 2;

/// Sub-categories of the AUDIO tab, shown in the modal's sidebar — the same
/// split seqterm's AUDIO SETTINGS uses.
const AUDIO_SECTIONS: &[&str] = &["Engine", "Plugin Paths", "OSC"];
const SEC_ENGINE: usize = 0;
const SEC_PATHS: usize = 1;
const SEC_OSC: usize = 2;

/// Editable rows of the Engine section, in display order.
const ENGINE_ROWS: &[&str] = &["Backend", "Device", "Sample rate", "Buffer size", "SF2 engine"];
/// Editable rows of the OSC section.
const OSC_ROWS: &[&str] = &["Enable OSC", "Port mode", "UDP port", "TCP port"];

/// Format chips of the ADD FX modal.
const FX_FORMATS: &[&str] =
    &["ALL", "BUILT-IN", "CLAP", "LV2", "VST2", "VST3", "LADSPA", "DSSI", "JSFX"];

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
    Clap { path: std::path::PathBuf, id: String },
    File(std::path::PathBuf),
    /// Open the file browser for this extension instead of loading directly.
    Browse(&'static str),
    /// A format choz can find but not load yet.
    Unsupported(&'static str),
}

/// Formats the SOURCE picker can filter by. Only CLAP/SF2/WAV can actually be
/// loaded today; the rest are listed so it's obvious what choz doesn't host yet.
const SOURCE_FORMATS: &[&str] = &[
    "ALL", "CLAP", "SF2", "WAV", "SFZ", "LV2", "VST2", "VST3", "DSSI", "LADSPA", "JSFX",
];

/// A rack control a MIDI CC can drive, bound by MIDI learn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LearnTarget {
    Gain(usize),
    Pan(usize),
    FxParam { slot: usize, fx: usize, param: usize },
    /// A button rather than a fader: fired by a CC crossing half-scale, so a
    /// pad, a footswitch or the top half of a fader all work.
    Trigger(TriggerAction),
}

/// Rack buttons a CC can press. `DEL` is deliberately absent — nothing should
/// delete an FX because a fader was nudged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TriggerAction {
    PresetPrev,
    PresetNext,
    FxToggle,
    FxMoveLeft,
    FxMoveRight,
    /// Select FX slot `n` of the active chain (the FX CHAIN row).
    FxSelect(usize),
    FxAdd,
    Mute,
    Solo,
}

impl TriggerAction {
    fn label(self) -> String {
        match self {
            TriggerAction::PresetPrev => "BANK \u{25C0}".to_string(),
            TriggerAction::PresetNext => "BANK \u{25B6}".to_string(),
            TriggerAction::FxToggle => "FX ON/OFF".to_string(),
            TriggerAction::FxMoveLeft => "FX \u{25C0} MOVE".to_string(),
            TriggerAction::FxMoveRight => "FX MOVE \u{25B6}".to_string(),
            TriggerAction::FxSelect(i) => format!("select FX {}", i + 1),
            TriggerAction::FxAdd => "ADD FX".to_string(),
            TriggerAction::Mute => "MUTE".to_string(),
            TriggerAction::Solo => "SOLO".to_string(),
        }
    }
}

/// A directory being typed in the Plugin paths modal. `dir` is the index of the
/// row being rewritten, or `None` when it's a brand new entry.
#[derive(Debug, Clone)]
struct PathEdit {
    fmt: choz_engine::PluginFormat,
    dir: Option<usize>,
    buf: String,
    /// Caret position, in characters.
    cursor: usize,
}

impl PathEdit {
    fn new(fmt: choz_engine::PluginFormat, dir: Option<usize>, buf: String) -> Self {
        let cursor = buf.chars().count();
        Self { fmt, dir, buf, cursor }
    }

    /// The line the modal shows while editing, with a caret at the cursor and
    /// the format the path is being filed under (getting that wrong is the easy
    /// mistake: an SF2 folder under SFZ finds nothing).
    fn display(&self) -> String {
        let mut out: String = self.buf.chars().take(self.cursor).collect();
        out.push('\u{2588}');
        out.extend(self.buf.chars().skip(self.cursor));
        format!("    \u{270E} [{}] {out}", self.fmt.label())
    }

    /// Apply a key to the buffer. Returns `Some(commit)` when the edit ends.
    fn key(&mut self, key: KeyCode) -> Option<bool> {
        match key {
            KeyCode::Char(c) => {
                let byte = self.buf.char_indices().nth(self.cursor).map(|(i, _)| i)
                    .unwrap_or(self.buf.len());
                self.buf.insert(byte, c);
                self.cursor += 1;
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    let byte = self.buf.char_indices().nth(self.cursor - 1).map(|(i, _)| i)?;
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
    devices: Vec<String>,
    targets: Vec<LearnTarget>,
    browser: Option<file_browser::FileBrowser>,
}

impl Modal {
    fn new(kind: ModalKind, list: views::modal::ListModal) -> Self {
        Self { kind, list, sources: Vec::new(), devices: Vec::new(), targets: Vec::new(), browser: None }
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
    /// Input-list rects: (input index, rect).
    input_item_rects: Vec<(usize, Rect)>,
    /// The connect/disconnect mark at the left of each input row.
    input_mark_rects: Vec<(usize, Rect)>,
    /// The INPUTS panel's rescan button.
    input_scan_rect: Option<Rect>,
    /// The OUT line in the transport panel (click = open the device picker).
    out_device_rect: Option<Rect>,
    /// About dialog close-button rect.
    about_close_rect: Option<Rect>,
}

#[allow(dead_code)]
struct App {
    source: AudioSource,
    /// Every MIDI input port seen at the last scan (connected or not).
    midi_ports: Vec<String>,
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
    fx_plugins: Vec<source::ClapFx>,

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
    /// Pre-rendered logo image protocol (ratatui-image), built at startup.
    logo: Option<ratatui_image::protocol::Protocol>,
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
    /// Cursor into the input list.
    input_cursor: usize,
    /// Interface settings (text colour, language).
    ui: settings::UiSettings,
    /// Format whose directory list the AddPath browser is feeding.
    paths_format: Option<choz_engine::PluginFormat>,
    /// In-place path editor of the Plugin paths section.
    path_edit: Option<PathEdit>,
    /// In-place numeric editor for an OSC port.
    port_edit: Option<PortEdit>,
    /// Set when the search paths changed, so closing the modal rescans.
    paths_dirty: bool,
    /// Rack control waiting for a MIDI CC (MIDI learn armed).
    learn: Option<LearnTarget>,
    /// MIDI learn is waiting for the user to *click* the control to bind. While
    /// it is on, the terminal reports bare mouse motion and choz paints a `?`
    /// pointer; both are turned back off as soon as a CC lands or it's cancelled.
    learn_pick: bool,
    /// Last known mouse position, only tracked while `learn_pick` is on.
    mouse: (u16, u16),
    /// MIDI-learn bindings: CC number -> the rack control it drives.
    cc_bindings: Vec<(u8, LearnTarget)>,
    /// Last value seen per CC, for the rising-edge test button bindings use.
    cc_last: [u8; 128],
    /// UDP port the OSC listener bound to, if it started.
    osc_port: Option<u16>,
    /// The running listener; dropping it frees the port.
    osc: Option<choz_engine::osc::OscHandle>,

    registry: PluginRegistry,
    audio_engine: Option<engine::AudioEngine>,

    playing: bool,
    quit: bool,

    layout: RefCell<UiLayout>,

    /// Splash screen state.
    splash: SplashState,
    /// Whether the splash screen has finished.
    splash_done: bool,
}

impl App {
    fn new() -> Self {
        let registry = PluginRegistry::with_default_adapters();
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
            slots: Vec::new(),
            active_slot: 0,
            fx_chain: Vec::new(),
            fx_slot: 0,
            fx_param: 0,
            focus: Focus::Source,
            modal: None,
            menu: None,
            about_open: false,
            logo: logo::build_logo(),
            active_notes: Vec::new(),
            note_tx,
            note_rx,
            _midi_conns: Vec::new(),
            midi_disabled: Vec::new(),
            input_cursor: 0,
            // Loaded here, applied by `main` — `apply()` sets process-wide
            // state (language, text colour) that tests must not inherit.
            ui: settings::UiSettings::load(),
            paths_format: None,
            path_edit: None,
            port_edit: None,
            paths_dirty: false,
            learn: None,
            learn_pick: false,
            mouse: (0, 0),
            cc_bindings: Vec::new(),
            cc_last: [0; 128],
            osc_port: None,
            osc: None,
            registry,
            audio_engine: None,
            playing: false,
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
        let Some(engine) = self.audio_engine.as_ref() else { return };
        let paths = self.plugin_paths.clone();
        self.plugins = if force { engine.rescan_plugins(&paths) } else { engine.cached_plugins(&paths) };
        self.synths = self
            .plugins
            .iter()
            .filter(|p| p.is_instrument)
            .map(|p| SynthEntry {
                id: p.id.clone(),
                format: p.format.label().to_string(),
                name: p.name.clone(),
                path: p.path.clone(),
            })
            .collect();
        // Only CLAP effects can be hosted in the chain today; the rest are still
        // listed in ADD FX (with their format) so it's clear what was found.
        self.fx_plugins = self
            .plugins
            .iter()
            .filter(|p| !p.is_instrument && p.format == choz_engine::PluginFormat::Clap)
            // Parameters are read lazily when the effect is added — scanning
            // instantiates enough plugins already.
            .map(|p| source::ClapFx {
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
            .map(|p| (choz_engine::PluginFormat::Clap, p.name.clone(), true))
            .collect();
        out.extend(
            self.plugins
                .iter()
                .filter(|p| !p.is_instrument && p.format != choz_engine::PluginFormat::Clap)
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
                    plugin.params = choz_engine::read_clap_params(&plugin.path, &plugin.id);
                    AudioFxEntry::new_clap(plugin)
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
                let mark = if hosted { String::new() } else { "  (not hosted yet)".to_string() };
                SourceChoice {
                    fmt: p.format.label(),
                    label: format!("{}{mark}", p.name),
                    action: match p.format {
                        choz_engine::PluginFormat::Clap => {
                            SourceAction::Clap { path: p.path.clone(), id: p.id.clone() }
                        }
                        choz_engine::PluginFormat::Sf2 => SourceAction::File(p.path.clone()),
                        _ => SourceAction::Unsupported(p.format.label()),
                    },
                }
            })
            .collect();

        for (fmt, ext, dirs) in [
            ("SF2", "sf2", sf2_dirs()),
            ("WAV", "wav", vec![std::env::current_dir().unwrap_or_else(|_| ".".into())]),
        ] {
            out.push(SourceChoice {
                fmt,
                label: format!("Browse for a .{ext} file..."),
                action: SourceAction::Browse(ext),
            });
            for dir in dirs {
                for path in scan_files(&dir, ext) {
                    out.push(SourceChoice {
                        fmt,
                        label: path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
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

    fn open_device_modal(&mut self) {
        let Some(engine) = self.audio_engine.as_ref() else { return };
        let devices = engine.output_devices();
        if devices.is_empty() {
            eprintln!("choz: no output devices to choose from");
            return;
        }
        let cursor = engine
            .output_device()
            .and_then(|cur| devices.iter().position(|d| d == cur))
            .unwrap_or(0);
        let mut modal = Modal::new(
            ModalKind::Device,
            views::modal::ListModal::new("AUDIO OUTPUT", Vec::new()),
        );
        modal.list.note = "  switching reloads every rack tab".to_string();
        modal.devices = devices;
        modal.list.cursor = cursor;
        self.modal = Some(modal);
        self.refresh_modal();
    }

    /// SF2 bank/preset picker for the active tab (RACK's `[BANK/PRESET]`).
    fn open_preset_modal(&mut self) {
        let Some(slot) = self.slots.get(self.active_slot) else { return };
        if slot.presets.is_empty() {
            eprintln!("choz: the active tab has no SoundFont");
            return;
        }
        let mut modal = Modal::new(
            ModalKind::Preset,
            views::modal::ListModal::new("BANK / PRESET", Vec::new()),
        );
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
            LearnTarget::Trigger(TriggerAction::FxToggle),
            LearnTarget::Trigger(TriggerAction::FxMoveLeft),
            LearnTarget::Trigger(TriggerAction::FxMoveRight),
            LearnTarget::Trigger(TriggerAction::FxAdd),
        ];
        for fx in 0..self.fx_chain.len() {
            targets.push(LearnTarget::Trigger(TriggerAction::FxSelect(fx)));
        }
        for (fx, entry) in self.fx_chain.iter().enumerate() {
            for param in 0..entry.param_descs().len() {
                targets.push(LearnTarget::FxParam { slot, fx, param });
            }
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

    fn open_browser_modal(&mut self, ext: &'static str) {
        let start = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let mut modal = Modal::new(
            ModalKind::Browser,
            views::modal::ListModal::new(format!("OPEN .{ext}"), Vec::new()),
        );
        modal.browser = Some(file_browser::FileBrowser::open(&start, ext));
        self.modal = Some(modal);
        self.refresh_modal();
    }

    /// Parameter editor for the active tab's plugin instrument.
    fn open_instr_modal(&mut self) {
        let Some(slot) = self.slots.get(self.active_slot) else { return };
        if slot.instr_values.is_empty() {
            return;
        }
        let mut modal = Modal::new(
            ModalKind::InstrParams,
            views::modal::ListModal::new(format!("INSTRUMENT \u{00B7} {}", slot_label(&slot.source)), Vec::new()),
        );
        modal.list.note = "  \u{2190}\u{2192} change the selected value".to_string();
        self.modal = Some(modal);
        self.refresh_modal();
    }

    /// Close whatever modal is open. Editing the search paths changes what a
    /// scan would find, so leaving that modal always rescans — whichever way it
    /// was closed (Esc, the CANCEL button, or a click outside).
    fn close_modal(&mut self) {
        let kind = self.modal.take().map(|m| m.kind);
        self.path_edit = None;
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
        modal.list.filters = SETTINGS_TABS.iter().map(|t| i18n::t(t).to_string()).collect();
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
                format!("   (0 \u{2014} holds {n} {} file(s), move it to {})", other.label(), other.label())
            }
            _ => "   (0)".to_string(),
        }
    }

    /// Which AUDIO sub-category is showing (Engine / Plugin Paths / OSC).
    fn audio_section(&self) -> usize {
        self.modal.as_ref().map(|m| m.list.sidebar_cursor).unwrap_or(SEC_ENGINE)
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
        let mut rows = vec![
            format!("  {:>14}  {}", "Backend", backend),
            format!("  {:>14}  {}", "Device", device),
            format!("  {:>14}  {} Hz", "Sample rate", pending(a.sample_rate, running.map(|r| r.0))),
            format!("  {:>14}  {} samples", "Buffer size", pending(a.buffer_size, running.map(|r| r.1))),
            // choz only builds oxisynth; the row exists so the setting matches
            // seqterm's file, not to pretend there is a choice.
            format!("  {:>14}  {} (only engine built in)", "SF2 engine", a.sf2_engine),
            format!("  {:>14}  {:.1} ms", "Latency", a.latency_ms()),
        ];
        // Backend-specific extras, read-only (edit them in the config file).
        match a.backend.to_uppercase().as_str() {
            "ALSA" => rows.push(format!(
                "  {:>14}  {}",
                "ALSA hw dev",
                if a.alsa_hw_device.is_empty() { "(default)" } else { &a.alsa_hw_device }
            )),
            "JACK" => rows.push(format!(
                "  {:>14}  {}",
                "JACK server",
                if a.jack_server_name.is_empty() { "(default)" } else { &a.jack_server_name }
            )),
            _ => rows.push(format!(
                "  {:>14}  {}",
                "PW quantum",
                if a.pipewire_quantum == 0 { "system".to_string() } else { a.pipewire_quantum.to_string() }
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
            format!("  {:>12}  {}", "Enable OSC", if o.enabled { "On" } else { "Off" }),
            format!("  {:>12}  {mode}", "Port mode"),
            format!("  {:>12}  {}", "UDP port", port_field(2, o.udp_port.to_string())),
            format!(
                "  {:>12}  {}",
                "TCP port",
                port_field(3, format!("{}  (stored — the server is UDP-only)", o.tcp_port))
            ),
            String::new(),
            format!("  {:>12}  {live}", "server"),
        ]
    }

    /// Which Settings tab is showing.
    fn settings_tab(&self) -> usize {
        self.modal.as_ref().map(|m| m.list.filter).unwrap_or(TAB_AUDIO)
    }

    /// Save the interface settings and push them into the drawing code.
    fn apply_ui_settings(&mut self) {
        self.ui.apply();
        self.ui.save();
        // The tab labels are themselves translated.
        if let Some(m) = self.modal.as_mut() {
            m.list.filters = SETTINGS_TABS.iter().map(|t| i18n::t(t).to_string()).collect();
            m.list.title = format!("{} \u{00B7} {}", i18n::t("SETTINGS"), i18n::t("PLUGIN PATHS"));
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
        let Some(m) = self.modal.as_ref() else { return false };
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
            (SEC_ENGINE, 2) if step != 0 => {
                self.ui.audio.sample_rate =
                    cycle_num(settings::SAMPLE_RATES, self.ui.audio.sample_rate, step);
            }
            (SEC_ENGINE, 3) if step != 0 => {
                self.ui.audio.buffer_size =
                    cycle_num(settings::BUFFER_SIZES, self.ui.audio.buffer_size, step);
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
                let cur = if r == 2 { self.ui.osc.udp_port } else { self.ui.osc.tcp_port };
                if step == 0 {
                    self.port_edit = Some(PortEdit { row: r, buf: cur.to_string() });
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
        let Some(m) = self.modal.as_ref() else { return false };
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
                m.list.note =
                    format!("  typing a {fmt} path \u{00B7} Enter=save  Esc=cancel  (empty = remove)");
            }
            match edit.key(key) {
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
        let Some(&(fmt, dir)) = self.path_rows().get(cursor) else { return false };
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
        let text = edit.buf.trim().to_string();
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
            (None, false) => dirs.push(choz_engine::SearchDir { path: text.into(), enabled: true }),
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
        modal.browser = Some(file_browser::FileBrowser::open(&start, file_browser::DIR_PICK));
        self.modal = Some(modal);
        self.refresh_modal();
    }

    /// Rebuild the open modal's visible item list from its data and filter.
    /// Called on open, on filter change, and after anything that changes the
    /// underlying data (browsing into a directory, turning a knob).
    fn refresh_modal(&mut self) {
        let Some(kind) = self.modal.as_ref().map(|m| m.kind) else { return };
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
                    .map(|(cat, n)| {
                        (cat.map(|c| c.label()).unwrap_or("ALL").to_string(), n)
                    })
                    .collect();
                if let Some(m) = self.modal.as_mut() {
                    let last = sidebar.len().saturating_sub(1);
                    m.list.sidebar_cursor = m.list.sidebar_cursor.min(last);
                    m.list.sidebar = sidebar;
                }
                self.fx_menu_rows().into_iter().map(|(_, label)| label).collect()
            }
            ModalKind::Device => self.modal.as_ref().unwrap().devices.clone(),
            ModalKind::Preset => self
                .slots
                .get(self.active_slot)
                .map(|s| s.presets.iter().map(|p| p.label()).collect())
                .unwrap_or_default(),
            ModalKind::Learn => {
                let targets = self.modal.as_ref().unwrap().targets.clone();
                targets
                    .iter()
                    .map(|t| {
                        let bound = self
                            .cc_bindings
                            .iter()
                            .find(|(_, b)| b == t)
                            .map(|(cc, _)| format!("   [CC {cc}]"))
                            .unwrap_or_default();
                        format!("{}{}", self.learn_label(t), bound)
                    })
                    .collect()
            }
            ModalKind::Browser => self
                .modal
                .as_ref()
                .unwrap()
                .browser
                .as_ref()
                .map(|b| b.entries.iter().map(|e| e.label.clone()).collect())
                .unwrap_or_default(),
            ModalKind::PluginPaths if self.settings_tab() == TAB_AUDIO
                && self.audio_section() == SEC_ENGINE =>
            {
                self.engine_rows()
            }
            ModalKind::PluginPaths if self.settings_tab() == TAB_AUDIO
                && self.audio_section() == SEC_OSC =>
            {
                self.osc_rows()
            }
            ModalKind::PluginPaths if self.settings_tab() == TAB_COLOR => settings::PALETTE
                .iter()
                .map(|(name, rgb)| {
                    let mark = if *rgb == self.ui.text_color { "\u{25CF}" } else { "\u{25CB}" };
                    format!("  {mark} {name}   \u{2588}\u{2588}\u{2588}\u{2588}  rgb({},{},{})", rgb.0, rgb.1, rgb.2)
                })
                .collect(),
            ModalKind::PluginPaths if self.settings_tab() == TAB_LANG => i18n::Lang::ALL
                .iter()
                .map(|l| {
                    let mark = if *l == self.ui.language { "\u{25CF}" } else { "\u{25CB}" };
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
            ModalKind::AddPath | ModalKind::SaveProject => self
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
                            let v = s.instr_values.get(i).copied().unwrap_or(0.0);
                            format!(
                                "{:<22} [{}] {:>10.3}",
                                views::fx_chain_panel::truncate(&p.name, 22),
                                views::fx_chain_panel::knob_arc(v, 8),
                                p.plain(v as f64)
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
        };
        if kind == ModalKind::PluginPaths {
            let tab = self.settings_tab();
            let section = self.audio_section();
            let mut title =
                format!("{} \u{00B7} {}", i18n::t("SETTINGS"), i18n::t(SETTINGS_TABS[tab.min(2)]));
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
                (TAB_COLOR, _) => (
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
            if let (ModalKind::Browser | ModalKind::AddPath | ModalKind::SaveProject, Some(b)) =
                (m.kind, m.browser.as_ref())
            {
                m.list.note = format!("  {}", b.dir.display());
                m.list.cursor = b.cursor;
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
            LearnTarget::Trigger(action) => action.label(),
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
        let Some(m) = self.modal.as_ref() else { return true };
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
                    Some(SourceAction::Clap { path, id }) => {
                        self.load_clap_source(&path, &id);
                        true
                    }
                    Some(SourceAction::File(path)) => {
                        self.load_source(path);
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
            ModalKind::Device => {
                if let Some(name) = m.devices.get(i).cloned() {
                    self.set_output_device(&name);
                }
                true
            }
            ModalKind::Preset => {
                if let Some(slot) = self.slots.get_mut(self.active_slot) {
                    slot.preset_cursor = i;
                }
                self.apply_selected_preset();
                true
            }
            ModalKind::Learn => {
                self.learn = m.targets.get(i).copied();
                true
            }
            ModalKind::Browser => {
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
                    Some(file_browser::Action::PickFile(path)) => {
                        self.load_source(path);
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
                    TAB_COLOR => {
                        if let Some(&(_, rgb)) = settings::PALETTE.get(i) {
                            self.ui.text_color = rgb;
                            self.apply_ui_settings();
                        }
                        // The choice is applied and saved: SELECT means done.
                        true
                    }
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
            ModalKind::SaveProject => {
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
                        self.save_project_to(&dir);
                        true
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
                            self.plugin_paths.dirs_mut(fmt).push(choz_engine::SearchDir {
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
            .chain(self.plugin_fx_entries().into_iter().map(|(fmt, name, hosted)| FxMenuEntry {
                format: Some(fmt),
                category: source::FxCategory::guess(&name),
                label: name,
                hosted,
            }))
            .collect()
    }

    /// Categories that have anything in them under the current format chip,
    /// with their counts — this is the ADD FX sidebar. "ALL" comes first.
    fn fx_categories(&self) -> Vec<(Option<source::FxCategory>, usize)> {
        let wanted = self.fx_format_filter();
        let entries = self.fx_menu_entries();
        let matching: Vec<&FxMenuEntry> =
            entries.iter().filter(|e| e.matches_filter(wanted)).collect();
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

    /// Load a CLAP instrument by path+id into the active tab (SOURCE picker).
    fn load_clap_source(&mut self, path: &std::path::Path, id: &str) {
        if let Some(i) = self.synths.iter().position(|s| s.id == id && s.path == path) {
            self.load_synth(i);
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
                        path: None, id: None, name: None, bank: None, preset: None,
                        params: Vec::new(),
                    },
                    AudioSource::Sf2 { path, bank, preset } => project::Instrument {
                        kind: "sf2".into(),
                        path: Some(path.clone()),
                        id: None,
                        name: None,
                        bank: Some(*bank),
                        preset: Some(*preset),
                        params: Vec::new(),
                    },
                    AudioSource::AudioFile { path, .. } => project::Instrument {
                        kind: "wav".into(),
                        path: Some(path.clone()),
                        id: None, name: None, bank: None, preset: None,
                        params: Vec::new(),
                    },
                    AudioSource::Plugin { id, name, .. } => project::Instrument {
                        kind: "plugin".into(),
                        path: self.synths.iter().find(|s| s.id == *id).map(|s| s.path.clone()),
                        id: Some(id.clone()),
                        name: Some(name.clone()),
                        bank: None,
                        preset: None,
                        params: slot.instr_values.clone(),
                    },
                };
                let fx = slot
                    .fx_chain
                    .iter()
                    .map(|e| {
                        let spec = e.to_spec();
                        project::Fx {
                            kind: if e.clap.is_some() { "clap".into() } else { spec.kind },
                            enabled: spec.enabled,
                            wet: spec.wet,
                            params: spec.params,
                            plugin_path: e.clap.as_ref().map(|c| c.path.clone()),
                            plugin_id: e.clap.as_ref().map(|c| c.id.clone()),
                        }
                    })
                    .collect();
                // Only the bindings that point at this tab.
                let midi_learn = self
                    .cc_bindings
                    .iter()
                    .filter(|(_, t)| match t {
                        LearnTarget::Gain(s) | LearnTarget::Pan(s) => *s == idx,
                        LearnTarget::FxParam { slot, .. } => *slot == idx,
                        LearnTarget::Trigger(_) => idx == self.active_slot,
                    })
                    .map(|(cc, t)| (*cc, self.learn_label(t)))
                    .collect();
                project::Slot {
                    input: slot.input.as_ref().map(|i| match i {
                        InputRef::Midi(name) => format!("MIDI:{name}"),
                        InputRef::Osc => "OSC".to_string(),
                    }),
                    instrument,
                    mixer: project::Mixer {
                        gain: slot.gain,
                        pan: slot.pan,
                        mute: slot.mute,
                        solo: slot.solo,
                    },
                    fx,
                    midi_learn,
                }
            })
            .collect();

        project::Project {
            version: 1,
            audio: project::Audio {
                sample_rate: engine.map(|e| e.sample_rate).unwrap_or(48_000),
                buffer_size: engine.map(|e| e.buffer_size).unwrap_or(256),
                backend: engine.map(|e| e.backend.label().to_string()).unwrap_or_default(),
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

    /// File \u{2192} Save project: pick the directory, then write the YAML.
    fn open_save_project(&mut self) {
        let start = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let mut modal = Modal::new(
            ModalKind::SaveProject,
            views::modal::ListModal::new("SAVE PROJECT", Vec::new()),
        );
        modal.browser = Some(file_browser::FileBrowser::open(&start, file_browser::DIR_PICK));
        self.modal = Some(modal);
        self.refresh_modal();
    }

    fn save_project_to(&mut self, dir: &std::path::Path) {
        let project = self.project_snapshot();
        match project.save(dir) {
            Ok(file) => eprintln!("choz: project saved to {}", file.display()),
            Err(e) => eprintln!("choz: cannot save the project: {e}"),
        }
    }

    // ── MIDI learn ────────────────────────────────────────────────────────

    /// Arm pointer learn: the next click on a rack control picks what to bind.
    /// Bare mouse motion has to be requested from the terminal (crossterm only
    /// enables drag reporting), so choz turns mode 1003 on here and off again
    /// when learn ends — outside learn the mouse behaves exactly as before.
    fn start_learn_pick(&mut self) {
        if self.slots.is_empty() {
            return;
        }
        self.learn_pick = true;
        self.learn = None;
        print!("\u{1b}[?1003h");
        let _ = io::Write::flush(&mut io::stdout());
    }

    /// Leave learn entirely (bound, or cancelled): motion reporting off, normal
    /// mouse behaviour back.
    fn end_learn(&mut self) {
        self.learn_pick = false;
        self.learn = None;
        print!("\u{1b}[?1003l");
        let _ = io::Write::flush(&mut io::stdout());
    }

    /// What the INPUTS panel should say about learn, if anything.
    fn learn_banner(&self) -> Option<String> {
        match (self.learn_pick, self.learn) {
            (_, Some(t)) => Some(format!("move a fader \u{2192} {}", self.learn_label(&t))),
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
        if rack.gain.is_some_and(|r| r.contains(pos)) {
            return Some(LearnTarget::Gain(slot));
        }
        if rack.pan.is_some_and(|r| r.contains(pos)) {
            return Some(LearnTarget::Pan(slot));
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
            rack.buttons.iter().find(|(_, r)| r.contains(pos)).and_then(|&(b, _)| match b {
                RackButton::PresetPrev => Some(TriggerAction::PresetPrev),
                RackButton::PresetNext => Some(TriggerAction::PresetNext),
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
            .map(|&(param, _)| LearnTarget::FxParam { slot, fx: self.fx_slot, param })
    }

    /// Run a button binding. Same code paths the mouse and keys use.
    fn fire_trigger(&mut self, action: TriggerAction) {
        match action {
            TriggerAction::PresetPrev => self.step_preset(-1),
            TriggerAction::PresetNext => self.step_preset(1),
            TriggerAction::Mute => self.with_active_mix(|s| s.mute = !s.mute),
            TriggerAction::Solo => self.with_active_mix(|s| s.solo = !s.solo),
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

    /// A control change arrived: bind it if learn is armed, otherwise drive
    /// every rack control already bound to that CC.
    fn apply_cc(&mut self, cc: u8, value: u8) {
        if let Some(target) = self.learn.take() {
            self.cc_bindings.retain(|(c, t)| *c != cc && *t != target);
            self.cc_bindings.push((cc, target));
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
        for (_, target) in self.cc_bindings.clone().iter().filter(|(c, _)| *c == cc) {
            match *target {
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
                LearnTarget::FxParam { slot, fx, param } => {
                    if slot != self.active_slot {
                        // Only the active tab has a live working copy of its chain.
                        continue;
                    }
                    let Some(entry) = self.fx_chain.get_mut(fx) else { continue };
                    let Some(p) = entry.params.get_mut(param) else { continue };
                    *p = v;
                    let is_mix = entry.is_mix_param(param);
                    if is_mix {
                        entry.wet = v;
                    }
                    if entry.clap.is_some() {
                        let idx = if is_mix { choz_engine::FX_MIX_PARAM } else { param };
                        self.set_live_fx_param(fx, idx, v);
                    } else {
                        self.rebuild_fx();
                    }
                }
            }
        }
    }

    // ── Inputs ────────────────────────────────────────────────────────────

    /// Every note input, in list order: MIDI ports first, then OSC.
    fn input_list(&self) -> Vec<InputRef> {
        let mut list: Vec<InputRef> = self.midi_ports.iter().cloned().map(InputRef::Midi).collect();
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
        self.slots.iter().position(|s| s.input.as_ref() == Some(input))
    }

    /// Enter on the input list: jump to the tab already bound to this input, or
    /// create a new (instrument-less) tab bound to it.
    fn bind_selected_input(&mut self) {
        let Some(input) = self.input_list().get(self.input_cursor).cloned() else { return };
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
        let Some(input) = self.input_list().get(self.input_cursor).cloned() else { return };
        match input {
            InputRef::Midi(name) => {
                match self.midi_disabled.iter().position(|n| *n == name) {
                    Some(i) => { self.midi_disabled.remove(i); }
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

    /// Load the CLAP instrument at `i` into the active tab.
    fn load_synth(&mut self, i: usize) {
        let Some(entry) = self.synths.get(i).cloned() else { return };
        let Some(slot) = self.ensure_slot() else { return };
        let loaded = match self.audio_engine.as_mut() {
            Some(engine) => engine.load_clap(slot, &entry.path, &entry.id),
            None => return,
        };
        match loaded {
            Ok(()) => {
                self.set_active_source(AudioSource::Plugin {
                    id: entry.id.clone(),
                    format: entry.format,
                    name: entry.name,
                });
                // Read the plugin's own parameters so the INSTR editor can show
                // them; knobs start where the plugin says its defaults are.
                let params = choz_engine::read_clap_params(&entry.path, &entry.id);
                let values = params.iter().map(|p| p.normalised(p.default) as f32).collect();
                if let Some(s) = self.slots.get_mut(slot) {
                    s.instr_params = params;
                    s.instr_values = values;
                }
            }
            Err(e) => eprintln!("choz: {e}"),
        }
    }

    /// Set instrument parameter `index` of the active tab to `value` (0..1) and
    /// push it to the live plugin — no reload, like the FX knobs.
    fn set_instr_param(&mut self, index: usize, value: f32) {
        let slot = self.active_slot;
        let value = value.clamp(0.0, 1.0);
        match self.slots.get_mut(slot).and_then(|s| s.instr_values.get_mut(index)) {
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
        let bindings: Vec<Option<&InputRef>> = self.slots.iter().map(|s| s.input.as_ref()).collect();
        note_targets(&bindings, &self.midi_connected, self.active_slot, source)
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
        self.slots.push(RackSlot::new(source.clone()));
        self.active_slot = self.slots.len() - 1;
        self.source = source;
        self.fx_chain = Vec::new();
        self.fx_slot = 0;
        self.fx_param = 0;
    }

    /// Remove the active slot from the rack and the engine.
    fn remove_active_slot(&mut self) {
        self.remove_slot(self.active_slot);
    }

    /// Remove slot `idx` from the rack and the engine, then reload the working
    /// copy from whichever slot ends up active.
    fn remove_slot(&mut self, idx: usize) {
        if idx >= self.slots.len() {
            return;
        }
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
        let strips: Vec<(f32, f32, bool)> = self
            .slots
            .iter()
            .map(|s| (s.gain, s.pan, s.mute || (any_solo && !s.solo)))
            .collect();
        if let Some(ref mut engine) = self.audio_engine {
            for (i, (gain, pan, mute)) in strips.into_iter().enumerate() {
                engine.set_slot_mix(i, gain, pan, mute);
            }
        }
    }

    /// Apply the preset under the active slot's cursor (SF2 program change).
    fn apply_selected_preset(&mut self) {
        let idx = self.active_slot;
        let Some(slot) = self.slots.get_mut(idx) else { return };
        let Some(p) = slot.presets.get(slot.preset_cursor).cloned() else { return };
        slot.source = match &slot.source {
            AudioSource::Sf2 { path, .. } => {
                AudioSource::Sf2 { path: path.clone(), bank: p.bank, preset: p.preset }
            }
            other => other.clone(),
        };
        self.source = slot.source.clone();
        if let Some(ref mut engine) = self.audio_engine {
            engine.set_slot_program(idx, p.bank, p.preset);
        }
    }

    /// Move audio output to `name` and rebuild the rack on the new stream.
    /// Switching devices tears the old stream down, so every slot is recreated
    /// here from the UI's own model (instruments are reloaded from disk).
    fn set_output_device(&mut self, name: &str) {
        self.persist_active();
        if let Some(ref mut engine) = self.audio_engine {
            if let Err(e) = engine.set_output_device(name) {
                eprintln!("choz: {e}");
                return;
            }
        } else {
            return;
        }
        let slots = self.slots.clone();
        for (i, slot) in slots.iter().enumerate() {
            let Some(ref mut engine) = self.audio_engine else { return };
            if engine.add_silent().is_none() {
                break;
            }
            let loaded = match &slot.source {
                AudioSource::Midi => Ok(()),
                AudioSource::Sf2 { path, bank, preset } => engine.load_sf2(i, path, *bank, *preset),
                AudioSource::AudioFile { path, looping } => engine.load_wav(i, path, *looping),
                AudioSource::Plugin { id, .. } => {
                    match self.synths.iter().find(|s| s.id == *id) {
                        Some(entry) => {
                            let (path, id) = (entry.path.clone(), entry.id.clone());
                            engine.load_clap(i, &path, &id)
                        }
                        None => Err(anyhow::anyhow!("plugin {id} is no longer available")),
                    }
                }
            };
            if let Err(e) = loaded {
                eprintln!("choz: reloading tab {}: {e}", i + 1);
            }
            let specs: Vec<FxSpec> = slot.fx_chain.iter().map(|e| e.to_spec()).collect();
            if let Some(ref mut engine) = self.audio_engine {
                engine.set_slot_fx(i, specs);
                // A reloaded plugin is back at its own defaults.
                for (p, v) in slot.instr_values.iter().enumerate() {
                    engine.set_slot_param(i, p, *v);
                }
            }
        }
        self.push_mix();
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

    /// The active tab's current SoundFont program, as `bank:preset Name`.
    fn active_preset_label(&self) -> Option<String> {
        let slot = self.slots.get(self.active_slot)?;
        slot.presets.get(slot.preset_cursor).map(|p| p.label())
    }

    /// Step the active tab's SoundFont program by `delta` and apply it. This is
    /// what the RACK's `\u{25C0}` / `\u{25B6}` buttons (and their MIDI bindings) do.
    fn step_preset(&mut self, delta: isize) {
        let Some(slot) = self.slots.get(self.active_slot) else { return };
        if slot.presets.is_empty() {
            return;
        }
        if let Some(slot) = self.slots.get_mut(self.active_slot) {
            let last = slot.presets.len() as isize - 1;
            slot.preset_cursor =
                (slot.preset_cursor as isize + delta).clamp(0, last.max(0)) as usize;
        }
        self.apply_selected_preset();
    }

    /// What the active tab plays, for the RACK's instrument line.
    fn instrument_label(&self) -> String {
        if self.slots.is_empty() {
            return "(no rack tab)".to_string();
        }
        match &self.source {
            AudioSource::Midi => "(none)".to_string(),
            other => slot_label(other),
        }
    }

    /// Mutate the active slot's mixer strip, then push it to the engine.
    fn with_active_mix(&mut self, f: impl FnOnce(&mut RackSlot)) {
        let Some(slot) = self.slots.get_mut(self.active_slot) else { return };
        f(slot);
        self.push_mix();
    }

    /// Send one live parameter change to the FX at UI index `fx_idx` of the
    /// active slot. Disabled entries aren't in the engine's chain, so the index
    /// has to be translated.
    fn set_live_fx_param(&mut self, fx_idx: usize, param: usize, value: f32) {
        let Some(engine_fx) = self.engine_fx_index(fx_idx) else { return };
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
        let is_sf2 = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("sf2"));
        let Some(slot) = self.ensure_slot() else { return };
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
                self.set_active_source(AudioSource::Sf2 { path, bank: 0, preset: 0 });
                if let Some(slot) = self.slots.get_mut(self.active_slot) {
                    slot.presets = presets;
                }
            }
            Ok(()) => self.set_active_source(AudioSource::AudioFile { path, looping: true }),
            Err(e) => eprintln!("choz: {e}"),
        }
    }

    /// Trigger a piano note on the active tab's instrument, scheduling an auto
    /// note-off. (Terminals don't deliver reliable key-release, so notes are
    /// fixed-length.)
    fn piano_note_on(&mut self, note: u8) {
        let targets = self.note_targets(choz_engine::input::InputSource::Keyboard);
        if let Some(ref mut engine) = self.audio_engine {
            for slot in targets {
                engine.note_on(slot, note, 100);
            }
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
        self.input_cursor = self.input_cursor.min(self.input_list().len().saturating_sub(1));
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
    fn drain_midi(&mut self) {
        let events: Vec<midi::InputEvent> = self.note_rx.try_iter().collect();
        if events.is_empty() {
            return;
        }
        let mut controls = Vec::new();
        let mut ccs = Vec::new();
        // Resolve routing first: note_targets borrows self immutably.
        let mut routed = Vec::new();
        for event in events {
            match event {
                midi::InputEvent::Note(msg) => routed.push((self.note_targets(msg.source), msg)),
                midi::InputEvent::Cc(c) => ccs.push(c),
                midi::InputEvent::Control(c) => controls.push(c),
            }
        }
        if let Some(ref mut engine) = self.audio_engine {
            for (targets, msg) in routed {
                for slot in targets {
                    if msg.on {
                        engine.note_on(slot, msg.note, msg.vel);
                    } else {
                        engine.note_off(slot, msg.note);
                    }
                }
            }
        }
        for c in ccs {
            self.apply_cc(c.cc, c.value);
        }
        for c in controls {
            self.apply_control(c);
        }
    }

    /// Apply a remote-control message (OSC). Indices are 1-based on the wire.
    fn apply_control(&mut self, msg: choz_engine::input::ControlMsg) {
        use choz_engine::input::ControlMsg as C;
        let (tab, slot_of) = match msg {
            C::Gain { tab, .. } | C::Pan { tab, .. } | C::Mute { tab, .. } | C::FxParam { tab, .. } => {
                (tab, tab.checked_sub(1))
            }
        };
        let Some(slot_idx) = slot_of.filter(|i| *i < self.slots.len()) else {
            eprintln!("choz: OSC targets tab {tab}, which doesn't exist");
            return;
        };
        match msg {
            C::Gain { value, .. } => {
                if let Some(s) = self.slots.get_mut(slot_idx) { s.gain = value; }
                self.push_mix();
            }
            C::Pan { value, .. } => {
                if let Some(s) = self.slots.get_mut(slot_idx) { s.pan = value; }
                self.push_mix();
            }
            C::Mute { on, .. } => {
                if let Some(s) = self.slots.get_mut(slot_idx) { s.mute = on; }
                self.push_mix();
            }
            C::FxParam { fx, param, value, .. } => {
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
                let Some(entry) = self.fx_chain.get_mut(fx_idx) else { return };
                let Some(v) = entry.params.get_mut(p_idx) else { return };
                *v = value.clamp(0.0, 1.0);
                let is_mix = entry.is_mix_param(p_idx);
                if is_mix {
                    entry.wet = value;
                }
                if entry.clap.is_some() {
                    let param = if is_mix { choz_engine::FX_MIX_PARAM } else { p_idx };
                    self.set_live_fx_param(fx_idx, param, value);
                } else {
                    self.rebuild_fx();
                }
            }
        }
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
            let targets = self.note_targets(choz_engine::input::InputSource::Keyboard);
            if let Some(ref mut engine) = self.audio_engine {
                for n in &expired {
                    for slot in &targets {
                        engine.note_off(*slot, *n);
                    }
                }
            }
            self.active_notes.retain(|(n, _)| !expired.contains(n));
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
}

impl Cli {
    fn from_args() -> Self {
        let mut cli =
            Cli { osc_port: choz_engine::osc::DEFAULT_PORT, osc_port_given: false, file: None };
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--osc-port" => match args.next().and_then(|v| v.parse().ok()) {
                    Some(port) => {
                        cli.osc_port = port;
                        cli.osc_port_given = true;
                    }
                    None => eprintln!("choz: --osc-port needs a port number"),
                },
                _ => {
                    let path = std::path::PathBuf::from(&arg);
                    let ext_ok = path.extension().is_some_and(|e| {
                        e.eq_ignore_ascii_case("wav") || e.eq_ignore_ascii_case("sf2")
                    });
                    if ext_ok {
                        cli.file = Some(path);
                    } else {
                        eprintln!("choz: ignoring '{arg}' (expected a .wav or .sf2 file)");
                    }
                }
            }
        }
        cli
    }
}

fn main() -> Result<()> {
    // Send stderr (all eprintln! + panics) to a log file so it never corrupts
    // the TUI. Tell the user where it is before we grab the terminal.
    if let Some(path) = log::redirect_stderr() {
        println!("choz: logging to {}", path.display());
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
    app.ui.apply();

    let result = run_app(&mut terminal, &mut app);

    // Leave any-motion reporting off, whatever state learn was left in.
    print!("\u{1b}[?1003l");
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

fn run_app(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App) -> Result<()> {
    let splash_start = Instant::now();
    let splash_deadline = splash_start + std::time::Duration::from_secs(3);

    while !app.quit {
        terminal.draw(|f| ui(f, app))?;

        // Handle splash screen lifecycle
        if !app.splash_done {
            app.splash.tick += 1;
            if !app.splash.ready && Instant::now() >= splash_deadline {
                app.splash.dismiss();
                // Start audio engine after splash is ready
                let audio = app.ui.audio.clone();
                let mut eng = engine::AudioEngine::new(audio.sample_rate, audio.buffer_size);
                eng.set_backend_preference(&audio.backend);
                if !audio.device.is_empty() {
                    eng.set_output_device_preference(&audio.device);
                }
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
                    if let Some(path) = cli.file {
                        app.load_source(path);
                    }
                }
            }
            if !is_active(&app.splash) {
                app.splash_done = true;
            }
        }

        handle_events(app)?;
        app.drain_midi();
        app.tick_notes();
    }
    Ok(())
}

fn handle_events(app: &mut App) -> Result<()> {
    if event::poll(std::time::Duration::from_millis(50))? {
        match event::read()? {
            Event::Key(key)
                if (key.kind == KeyEventKind::Press || key.kind == KeyEventKind::Repeat) => {
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
            Event::Mouse(mouse)
                if app.splash_done => {
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
            KeyCode::Esc => { app.menu = None; }
            KeyCode::Left => { state.cycle_menu(false); app.menu = Some(state); }
            KeyCode::Right => { state.cycle_menu(true); app.menu = Some(state); }
            KeyCode::Up => { state.move_up(); app.menu = Some(state); }
            KeyCode::Down => { state.move_down(); app.menu = Some(state); }
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

    if key == KeyCode::Tab {
        app.focus = match app.focus {
            Focus::Source => Focus::FxChain,
            Focus::FxChain => Focus::Transport,
            Focus::Transport => Focus::Source,
        };
        return;
    }

    match app.focus {
        Focus::Source => handle_source_keys(app, key),
        Focus::FxChain => handle_fx_keys(app, key),
        Focus::Transport => handle_transport_keys(app, key),
    }
}

/// Keys for whichever modal is open. Navigation is the same everywhere;
/// only Enter (and the value arrows of the instrument editor) differ per kind.
fn handle_modal_key(app: &mut App, key: KeyCode) {
    let Some(kind) = app.modal.as_ref().map(|m| m.kind) else { return };
    let cursor = app.modal.as_ref().map(|m| m.list.cursor).unwrap_or(0);
    // Enable/disable, add and remove live in the Plugin Paths section; the
    // Engine and OSC sections have their own value editing.
    if app.paths_modal_key(key) || app.audio_settings_key(key) {
        return;
    }
    match key {
        KeyCode::Esc => {
            app.close_modal();
            return;
        }
        KeyCode::Up => {
            if let Some(m) = app.modal.as_mut() {
                if m.list.sidebar_focused { m.list.move_section(-1) } else { m.list.move_cursor(-1) }
            }
        }
        KeyCode::Down => {
            if let Some(m) = app.modal.as_mut() {
                if m.list.sidebar_focused { m.list.move_section(1) } else { m.list.move_cursor(1) }
            }
        }
        KeyCode::PageUp => {
            if let Some(m) = app.modal.as_mut() { m.list.move_cursor(-10); }
        }
        KeyCode::PageDown => {
            if let Some(m) = app.modal.as_mut() { m.list.move_cursor(10); }
        }
        // In a modal with a sidebar the arrows move between the two panes.
        KeyCode::Left | KeyCode::Right
            if app.modal.as_ref().is_some_and(|m| !m.list.sidebar.is_empty()) =>
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
        KeyCode::Tab if app.modal.as_ref().is_some_and(|m| !m.list.filters.is_empty()) => {
            if let Some(m) = app.modal.as_mut() {
                m.list.cycle_filter(1);
                m.list.sidebar_cursor = 0;
            }
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
                app.set_instr_param(cursor, v + delta as f32 * INSTR_STEP);
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
        // Rescan plugins from the SOURCE / ADD FX pickers.
        KeyCode::Char('r') if matches!(kind, ModalKind::Source | ModalKind::AddFx) => {
            app.discover_synths(true);
            if let Some(m) = app.modal.as_mut() {
                m.sources.clear();
            }
            if kind == ModalKind::Source {
                let sources = app.source_choices();
                if let Some(m) = app.modal.as_mut() { m.sources = sources; }
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
        KeyCode::Up => app.input_cursor = app.input_cursor.saturating_sub(1),
        KeyCode::Down => {
            app.input_cursor = (app.input_cursor + 1).min(app.input_list().len().saturating_sub(1));
        }
        // Bind the selected input to a rack tab (or jump to its tab).
        KeyCode::Enter => app.bind_selected_input(),
        KeyCode::Char('c') => app.toggle_selected_input(),
        KeyCode::Char('r') => app.connect_midi(),
        // The QWERTY piano plays the active tab from any panel.
        _ => {
            if let Some(note) = qwerty_note(key) {
                app.piano_note_on(note);
            }
        }
    }
}

/// Rack slots a note from `source` should reach.
///
/// The QWERTY piano always plays the active tab (it has no port of its own);
/// hardware inputs reach exactly the tabs bound to them, which is what replaced
/// the old omni broadcast.
fn note_targets(
    bindings: &[Option<&InputRef>],
    midi_connected: &[String],
    active_slot: usize,
    source: choz_engine::input::InputSource,
) -> Vec<usize> {
    use choz_engine::input::InputSource as S;
    let input = match source {
        S::Keyboard => {
            return if active_slot < bindings.len() { vec![active_slot] } else { Vec::new() };
        }
        S::Osc => InputRef::Osc,
        S::Midi(i) => match midi_connected.get(i) {
            Some(name) => InputRef::Midi(name.clone()),
            None => return Vec::new(),
        },
    };
    bindings
        .iter()
        .enumerate()
        .filter(|(_, b)| **b == Some(&input))
        .map(|(i, _)| i)
        .collect()
}


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
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<std::path::PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext)))
        .collect();
    out.sort();
    out
}

/// Short label for a rack tab, e.g. "SF2:piano" or "CLAP:Surge".
fn slot_label(source: &AudioSource) -> String {
    match source {
        AudioSource::Midi => "(empty)".to_string(),
        AudioSource::Sf2 { path, .. } => format!("SF2:{}", file_stem(path)),
        AudioSource::AudioFile { path, .. } => format!("WAV:{}", file_stem(path)),
        AudioSource::Plugin { name, .. } => format!("CLAP:{name}"),
    }
}

/// Tab text for a rack slot: mute/solo marker, source label, and the close
/// button. Used for both drawing and click hit-testing, so the two can't drift.
fn tab_label(slot: &RackSlot) -> String {
    let mark = match (slot.mute, slot.solo) {
        (_, true) => "\u{25C9}",  // soloed
        (true, _) => "\u{2298}",  // muted
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
    let c = match key { KeyCode::Char(c) => c.to_ascii_lowercase(), _ => return None };
    let n = match c {
        'a' => 60, 'w' => 61, 's' => 62, 'e' => 63, 'd' => 64, 'f' => 65,
        't' => 66, 'g' => 67, 'y' => 68, 'h' => 69, 'u' => 70, 'j' => 71,
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
        KeyCode::Char('[') => { if app.active_slot > 0 { app.switch_slot(app.active_slot - 1); } }
        KeyCode::Char(']') => { app.switch_slot(app.active_slot + 1); }
        KeyCode::Backspace => { app.remove_active_slot(); }
        KeyCode::Left => { app.fx_slot = app.fx_slot.saturating_sub(1); app.fx_param = 0; }
        KeyCode::Right
            if app.fx_slot + 1 < app.fx_chain.len() => {
                app.fx_slot += 1; app.fx_param = 0;
            }
        KeyCode::Up => { app.fx_param = app.fx_param.saturating_sub(1); }
        KeyCode::Down => {
            if let Some(entry) = app.fx_chain.get(app.fx_slot) {
                let max = entry.param_descs().len();
                if app.fx_param + 1 < max { app.fx_param += 1; }
            }
        }
        // Parameters of the tab's own instrument (plugin instruments only).
        KeyCode::Char('p') => app.open_instr_modal(),
        KeyCode::Char('w') | KeyCode::Char('W') => adjust_fx_param(app, 0.05),
        KeyCode::Char('s') => adjust_fx_param(app, -0.05),
        // Mixer strip of the active slot.
        KeyCode::Char('-') => adjust_gain(app, -0.05),
        KeyCode::Char('+') | KeyCode::Char('=') => adjust_gain(app, 0.05),
        KeyCode::Char(',') => adjust_pan(app, -0.1),
        KeyCode::Char('.') => adjust_pan(app, 0.1),
        KeyCode::Char('m') => app.with_active_mix(|s| s.mute = !s.mute),
        KeyCode::Char('S') => app.with_active_mix(|s| s.solo = !s.solo),
        KeyCode::Char(' ') => {
            if let Some(entry) = app.fx_chain.get_mut(app.fx_slot) {
                entry.enabled = !entry.enabled;
                app.rebuild_fx();
            }
        }
        KeyCode::Char('a') => app.open_add_fx_modal(),
        KeyCode::Char('d')
            if !app.fx_chain.is_empty() => {
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

fn adjust_gain(app: &mut App, delta: f32) {
    app.with_active_mix(|s| s.gain = (s.gain + delta).clamp(0.0, MAX_GAIN));
}

fn adjust_pan(app: &mut App, delta: f32) {
    app.with_active_mix(|s| s.pan = (s.pan + delta).clamp(-1.0, 1.0));
}

fn adjust_fx_param(app: &mut App, delta: f32) {
    let (fx_idx, param) = (app.fx_slot, app.fx_param);
    let Some(entry) = app.fx_chain.get_mut(fx_idx) else { return };
    let Some(v) = entry.params.get_mut(param) else { return };
    *v = (*v + delta).clamp(0.0, 1.0);
    let value = *v;
    let (is_plugin, is_mix) = (entry.clap.is_some(), entry.is_mix_param(param));
    if is_mix {
        entry.wet = value;
    }

    if !is_plugin {
        // Built-ins are configured at build time, so the chain is rebuilt.
        app.rebuild_fx();
        return;
    }
    // A hosted plugin must NOT be rebuilt per knob turn (that re-instantiates
    // it); the value is sent straight to the live processor instead.
    app.set_live_fx_param(fx_idx, if is_mix { choz_engine::FX_MIX_PARAM } else { param }, value);
}

fn handle_transport_keys(app: &mut App, key: KeyCode) {
    match key {
        KeyCode::Char(' ') => toggle_play(app),
        KeyCode::Char('s') => stop_play(app),
        KeyCode::Char('o') => app.open_device_modal(),
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

enum MouseAction {
    None,
    FocusPanel(Focus),
    FxSlot(usize),
    FxParam(usize),
    FxParamAdjust(usize, f32),
    FxAdd,
    FxToggle,
    FxDelete,
    FxMoveLeft,
    FxMoveRight,
    TransportPlay,
    TransportStop,
    OpenDevicePicker,
    InputBind(usize),
    InputToggle(usize),
    OpenSourcePicker,
    ScanInputs,
    PresetStep(isize),
    OpenPresetPicker,
    OpenLearnPicker,
    RackTab(usize),
    RackTabClose(usize),
    MixGain(f32),
    MixPan(f32),
    MixMute,
    MixSolo,
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

            if layout.source_area.contains(pos) {
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

            if layout.fx_chain_area.contains(pos) {
                let rack = &layout.rack;
                for &(btn, rect) in rack.buttons.iter() {
                    if rect.contains(pos) {
                        return match btn {
                            RackButton::Source => MouseAction::OpenSourcePicker,
                            RackButton::Preset => MouseAction::OpenPresetPicker,
                            RackButton::Learn => MouseAction::OpenLearnPicker,
                            RackButton::PresetPrev => MouseAction::PresetStep(-1),
                            RackButton::PresetNext => MouseAction::PresetStep(1),
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
                ] {
                    if rect.is_some_and(|r| r.contains(pos)) {
                        return action;
                    }
                }
                return MouseAction::FocusPanel(Focus::FxChain);
            }

            if layout.transport_area.contains(pos) {
                if layout.out_device_rect.is_some_and(|r| r.contains(pos)) {
                    return MouseAction::OpenDevicePicker;
                }
                return MouseAction::FocusPanel(Focus::Transport);
            }

            MouseAction::None
        }
        MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
            let dir = if matches!(kind, MouseEventKind::ScrollUp) { 1.0 } else { -1.0 };
            if layout.fx_chain_area.contains(pos) {
                let rack = &layout.rack;
                if rack.gain.is_some_and(|r| r.contains(pos)) {
                    return MouseAction::MixGain(dir * 0.05);
                }
                if rack.pan.is_some_and(|r| r.contains(pos)) {
                    return MouseAction::MixPan(dir * 0.1);
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
        A::OpenWav => app.open_browser_modal("wav"),
        A::OpenSf2 => app.open_browser_modal("sf2"),
        A::Quit => app.quit = true,
        A::PluginPaths => app.open_paths_modal(),
        A::SaveProject => app.open_save_project(),
        A::RescanPlugins => {
            app.discover_synths(true);
            eprintln!("choz: rescanned plugin paths: {} found", app.plugins.len());
        }
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
        if left { app.about_open = false; }
        return;
    }

    if let Some(state) = app.menu {
        if left {
            let (item_hit, title_hit) = {
                let l = app.layout.borrow();
                (
                    l.menu_item_rects.iter().find(|(_, r)| r.contains(pos)).map(|(i, _)| *i),
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
        let title_hit = app.layout.borrow().menu_bar_rects.iter().position(|r| r.contains(pos));
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
        MouseAction::FocusPanel(f) => { app.focus = f; }
        MouseAction::FxSlot(i) => {
            app.focus = Focus::FxChain;
            app.fx_slot = i;
            app.fx_param = 0;
        }
        MouseAction::FxParam(pi) => {
            app.focus = Focus::FxChain;
            app.fx_param = pi;
        }
        MouseAction::FxParamAdjust(pi, delta) => {
            let old_slot = app.fx_slot;
            let old_param = app.fx_param;
            app.fx_param = pi;
            adjust_fx_param(app, delta);
            app.fx_slot = old_slot;
            app.fx_param = old_param;
        }
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
        MouseAction::OpenDevicePicker => {
            app.focus = Focus::Transport;
            app.open_device_modal();
        }
        MouseAction::InputBind(i) => {
            app.focus = Focus::Source;
            app.input_cursor = i;
            app.bind_selected_input();
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
        }
        MouseAction::PresetStep(d) => app.step_preset(d),
        MouseAction::OpenPresetPicker => app.open_preset_modal(),
        MouseAction::OpenLearnPicker => app.start_learn_pick(),
        MouseAction::RackTab(i) => {
            app.focus = Focus::FxChain;
            app.switch_slot(i);
        }
        MouseAction::RackTabClose(i) => {
            app.focus = Focus::FxChain;
            app.remove_slot(i);
        }
        MouseAction::MixGain(d) => adjust_gain(app, d),
        MouseAction::MixPan(d) => adjust_pan(app, d),
        MouseAction::MixMute => app.with_active_mix(|s| s.mute = !s.mute),
        MouseAction::MixSolo => app.with_active_mix(|s| s.solo = !s.solo),
    }
}

/// Mouse inside an open modal. All modals share `layout.modal_rects`, so this
/// works the same for the source picker, ADD FX, devices, presets and browser.
fn handle_modal_mouse(app: &mut App, mouse: MouseEvent) {
    let pos: ratatui::layout::Position = (mouse.column, mouse.row).into();
    let rects = app.layout.borrow().modal_rects.clone();

    match mouse.kind {
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            if rects.list.is_some_and(|r| r.contains(pos)) {
                let delta = if matches!(mouse.kind, MouseEventKind::ScrollUp) { -1 } else { 1 };
                if let Some(m) = app.modal.as_mut() {
                    m.list.move_cursor(delta);
                    if let Some(b) = m.browser.as_mut() {
                        b.cursor = m.list.cursor;
                    }
                }
                app.refresh_modal();
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
            let row = rects.rows.iter().find(|(_, r)| r.contains(pos)).map(|(i, _)| *i);
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
                        true
                    } else {
                        // Click outside the popup dismisses it.
                        if !rects.area.is_some_and(|r| r.contains(pos)) {
                            app.close_modal();
                        }
                        return;
                    }
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

// ─── UI Render ─────────────────────────────────────────────────────────────────

fn ui(f: &mut Frame, app: &mut App) {
    let area = f.area();

    // ─── Splash Screen ──────────────────────────────────────────────────
    if !app.splash_done {
        draw_splash(f, &app.splash, area);
        return;
    }

    // Top: menu bar (1 row) · middle: body · bottom: status bar (1 row).
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)])
        .split(area);
    let menubar_area = root[0];
    let body = root[1];
    let status_area = root[2];

    draw_menu_bar(f, app, menubar_area);

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(body);

    let right_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(10), Constraint::Length(7)])
        .split(chunks[1]);

    let source_area = chunks[0];
    let fx_chain_area = right_chunks[0];
    let transport_area = right_chunks[1];

    let inputs = app.input_list();
    let rows: Vec<views::source_panel::InputRow> = inputs
        .iter()
        .map(|i| views::source_panel::InputRow {
            kind: i.kind(),
            name: i.name(),
            connected: app.input_is_connected(i),
            bound_tab: app.bound_tab(i),
        })
        .collect();
    let learn = app.learn_banner();
    let scan_rect = views::source_panel::draw_input_panel(
        f, source_area, app.focus == Focus::Source,
        &rows, app.input_cursor, &app.active_tab_label(), learn.as_deref(),
    );
    app.layout.borrow_mut().input_scan_rect = scan_rect;

    let tabs: Vec<String> = app.slots.iter().map(tab_label).collect();
    let mix = app.slots.get(app.active_slot).map(|s| (s.gain, s.pan, s.mute, s.solo));
    let rack = views::fx_chain_panel::draw_fx_chain_panel(
        f, fx_chain_area, &app.fx_chain,
        app.fx_slot, app.fx_param, app.focus == Focus::FxChain,
        &tabs, app.active_slot, mix, &app.instrument_label(),
        app.active_preset_label().as_deref(),
    );
    app.layout.borrow_mut().rack = rack;

    draw_transport(f, app, transport_area);

    if app.modal.is_some() {
        // The modal owns its scroll state, so it draws from a &mut borrow and
        // stores its hit rects where the mouse handler can find them.
        let mut modal = app.modal.take().unwrap();
        let pct = if modal.kind == ModalKind::Device { (60, 50) } else { (70, 70) };
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
                    Style::default().fg(Color::Black).bg(WARN).add_modifier(Modifier::BOLD),
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

    // Status bar
    let backend_label = app.audio_engine
        .as_ref()
        .map(|e| e.backend.label())
        .unwrap_or("none");
    let play_icon = if app.playing { "\u{25B6}" } else { "\u{25A0}" };
    let play_state = if app.playing { i18n::t("PLAYING") } else { i18n::t("STOPPED") };

    let status_text = format!(
        " choz v0.1 | {} backend | RACK: {} | FX: {} | {play_icon} {play_state} | F10=menu Tab=switch q=quit",
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
    compute_layout(app, area, source_area, fx_chain_area, transport_area);
}

fn compute_layout(app: &App, _area: Rect, source_area: Rect, fx_chain_area: Rect, transport_area: Rect) {
    let mut layout = app.layout.borrow_mut();

    layout.source_area = source_area;
    layout.fx_chain_area = fx_chain_area;
    layout.transport_area = transport_area;

    let src_inner = Rect::new(
        source_area.x + 1,
        source_area.y + 1,
        source_area.width.saturating_sub(2),
        source_area.height.saturating_sub(2),
    );
    // Input rows. Line layout must match `draw_input_panel`: the list starts at
    // INPUT_LIST_TOP and the connect mark is the second column of each row.
    use views::source_panel as sp;
    layout.input_item_rects.clear();
    layout.input_mark_rects.clear();
    let inputs = app.input_list();
    let list_y = src_inner.y + sp::INPUT_LIST_TOP as u16;
    for (i, _) in inputs.iter().enumerate() {
        let y = list_y + i as u16;
        layout.input_item_rects.push((i, Rect::new(src_inner.x, y, src_inner.width, 1)));
        layout.input_mark_rects.push((i, Rect::new(src_inner.x, y, 2, 1)));
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
        .title(" TRANSPORT ")
        .title_style(Style::default().fg(HEADER).add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_style(border_style)
        .style(Style::default().bg(PANEL_BG));

    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.height < 3 { return; }

    // Row 1: buttons centered
    let btn_row = inner.y + 1;

    // Play button
    let play_bg = if app.playing { OK } else { Color::Rgb(20, 60, 30) };
    let play_fg = if app.playing { Color::Black } else { DIM };
    let play_label = "[ ▶ PLAY ]";

    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            play_label,
            Style::default().fg(play_fg).bg(play_bg).add_modifier(Modifier::BOLD),
        )))
        .style(Style::default().bg(PANEL_BG)),
        Rect::new(inner.x + 2, btn_row, play_label.len() as u16, 1),
    );

    // Stop button
    let stop_bg = if !app.playing { ERR } else { Color::Rgb(50, 20, 20) };
    let stop_fg = if !app.playing { Color::Black } else { DIM };
    let stop_label = " [ ■ STOP ]";

    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            stop_label,
            Style::default().fg(stop_fg).bg(stop_bg).add_modifier(Modifier::BOLD),
        )))
        .style(Style::default().bg(PANEL_BG)),
        Rect::new(inner.x + 16, btn_row, stop_label.len() as u16, 1),
    );

    // Row 2: status text
    let status_y = btn_row + 1;
    let state_text = if app.playing {
        "  ▶ PLAYING  |  [Space]=pause  [S]=stop".to_string()
    } else {
        "  ■ STOPPED  |  [Space]=play  [S]=stop".to_string()
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            state_text,
            Style::default().fg(HINT),
        )))
        .style(Style::default().bg(PANEL_BG)),
        Rect::new(inner.x + 2, status_y, inner.width.saturating_sub(4), 1),
    );

    // Row 3: audio output device.
    if inner.height < 4 { return; }
    let device = app
        .audio_engine
        .as_ref()
        .and_then(|e| e.output_device())
        .unwrap_or("default");
    let out_rect = Rect::new(inner.x + 2, status_y + 1, inner.width.saturating_sub(4), 1);
    app.layout.borrow_mut().out_device_rect = Some(out_rect);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!("  {}  ", i18n::t("OUT")), Style::default().fg(HEADER).add_modifier(Modifier::BOLD)),
            Span::styled(device.to_string(), Style::default().fg(views::theme::text())),
            Span::styled("  [o=change]", Style::default().fg(HINT)),
        ]))
        .style(Style::default().bg(PANEL_BG)),
        out_rect,
    );
}

fn draw_menu_bar(f: &mut Frame, app: &App, area: Rect) {
    use menu::MenuKind;
    f.render_widget(Block::default().style(Style::default().bg(PANEL_BG)), area);

    let mut rects = Vec::new();
    let mut spans = Vec::new();
    let mut x = area.x;
    let open_kind = app.menu.map(|m| m.kind);
    for k in MenuKind::ALL {
        let label = k.label();
        let w = label.len() as u16;
        let style = if open_kind == Some(*k) {
            Style::default().fg(Color::Black).bg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(HEADER).bg(PANEL_BG).add_modifier(Modifier::BOLD)
        };
        spans.push(Span::styled(label, style));
        rects.push(Rect::new(x, area.y, w, 1));
        x += w;
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
    app.layout.borrow_mut().menu_bar_rects = rects;
}

fn draw_menu_dropdown(f: &mut Frame, app: &App, state: menu::MenuState, menubar_area: Rect) {
    // Horizontal offset = sum of label widths before the open menu.
    let mut x = menubar_area.x;
    for k in menu::MenuKind::ALL {
        if *k == state.kind { break; }
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
        .style(Style::default().bg(PANEL_BG));
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let mut item_rects = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let y = inner.y + i as u16;
        if item.separator {
            f.render_widget(
                Paragraph::new(Line::from(Span::styled("─".repeat(inner.width as usize), Style::default().fg(DIM)))),
                Rect::new(inner.x, y, inner.width, 1),
            );
            continue;
        }
        let selected = i == state.cursor;
        let st = if selected {
            Style::default().fg(Color::Black).bg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White).bg(PANEL_BG)
        };
        let pad = (inner.width as usize).saturating_sub(item.label.len() + item.shortcut.len() + 1);
        let text = format!(" {}{}{} ", item.label, " ".repeat(pad.max(1)), item.shortcut);
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(text, st))),
            Rect::new(inner.x, y, inner.width, 1),
        );
        item_rects.push((i, Rect::new(inner.x, y, inner.width, 1)));
    }
    app.layout.borrow_mut().menu_item_rects = item_rects;
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
        .style(Style::default().bg(PANEL_BG));
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    // Close button [×] + hit rect.
    let close_x = popup.x + popup.width.saturating_sub(4);
    f.render_widget(
        Paragraph::new(Line::from(Span::styled("[×]", Style::default().fg(ERR).add_modifier(Modifier::BOLD)))),
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
        Line::from(Span::styled("choz v0.1", Style::default().fg(Color::White).add_modifier(Modifier::BOLD))),
        Line::from(Span::styled("Terminal audio plugin host — Carla for the terminal.", Style::default().fg(DIM))),
        Line::from(""),
        Line::from(Span::styled("FX chain · WAV/SF2 · CLAP · MIDI", Style::default().fg(Color::White))),
        Line::from(Span::styled("Esc / [×] to close", Style::default().fg(DIM))),
    ];
    f.render_widget(
        Paragraph::new(text).style(Style::default().bg(PANEL_BG)),
        rows[1].inner(Margin { vertical: 0, horizontal: 1 }),
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
        app.apply_control(ControlMsg::Gain { tab: 2, value: 0.25 });
        app.apply_control(ControlMsg::Pan { tab: 2, value: -0.8 });
        app.apply_control(ControlMsg::Mute { tab: 2, on: true });
        assert_eq!(app.slots[0].gain, 1.0, "tab 1 is untouched");

        let mut term = Terminal::new(TestBackend::new(80, 12)).unwrap();
        term.draw(|f| {
            let mix = app.slots.get(app.active_slot).map(|s| (s.gain, s.pan, s.mute, s.solo));
            let tabs: Vec<String> = app.slots.iter().map(tab_label).collect();
            views::fx_chain_panel::draw_fx_chain_panel(
                f, f.area(), &app.fx_chain, app.fx_slot, app.fx_param, true,
                &tabs, app.active_slot, mix, &app.instrument_label(), None,
            );
        })
        .unwrap();

        let screen: String = term.backend().buffer().content().iter().map(|c| c.symbol()).collect();
        assert!(screen.contains("0.25"), "gain not drawn:\n{screen}");
        assert!(screen.contains("L80"), "pan not drawn:\n{screen}");
    }

    /// Language and text colour are process-wide (the draw code reads them from
    /// globals), so the test that switches them and the tests that render have
    /// to take turns.
    static UI_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn ui_guard() -> std::sync::MutexGuard<'static, ()> {
        UI_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Puts the global language and colour back to English/default on drop —
    /// including when the test panics, which would otherwise leave every other
    /// rendering test reading a foreign language.
    struct UiRestore;

    impl Drop for UiRestore {
        fn drop(&mut self) {
            i18n::set_language(i18n::Lang::En);
            views::theme::set_text_color(ratatui::style::Color::Rgb(
                settings::PALETTE[0].1.0,
                settings::PALETTE[0].1.1,
                settings::PALETTE[0].1.2,
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
    }

    /// A rack tab holding a plugin instrument with two parameters.
    fn app_with_plugin_tab() -> App {
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Plugin {
            id: "com.example.synth".into(),
            format: "CLAP".into(),
            name: "Example".into(),
        }));
        app.slots[0].instr_params = vec![
            choz_engine::ClapParamInfo {
                id: 0, name: "Cutoff".into(), min: 20.0, max: 20_000.0, default: 20.0,
            },
            choz_engine::ClapParamInfo {
                id: 1, name: "Resonance".into(), min: 0.0, max: 1.0, default: 0.0,
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
        term.backend().buffer().content().iter().map(|c| c.symbol()).collect()
    }

    /// The INSTR editor lists the plugin's own parameter names and values, and
    /// the arrows move the value of the row it points at.
    #[test]
    fn instrument_param_modal_draws_plugin_names_and_edits_values() {
        let mut app = app_with_plugin_tab();

        handle_fx_keys(&mut app, KeyCode::Char('p'));
        assert_eq!(app.modal.as_ref().map(|m| m.kind), Some(ModalKind::InstrParams));

        // Right moves the selected parameter up by one step, and it clamps at 0.
        handle_modal_key(&mut app, KeyCode::Right);
        assert_eq!(app.slots[0].instr_values[0], INSTR_STEP);
        handle_modal_key(&mut app, KeyCode::Left);
        handle_modal_key(&mut app, KeyCode::Left);
        assert_eq!(app.slots[0].instr_values[0], 0.0, "values clamp at 0");
        app.set_instr_param(0, 0.5);
        app.refresh_modal();

        let screen = render_modal(&mut app, 80, 20);
        assert!(screen.contains("Cutoff"), "plugin param name missing:\n{screen}");
        assert!(screen.contains("Resonance"), "second param missing:\n{screen}");
        // Half of 20..20000 in plain units.
        assert!(screen.contains("10010"), "plain value missing:\n{screen}");
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
        assert!(screen.contains("Example Synth"), "CLAP instrument missing:\n{screen}");
        assert!(screen.contains("SELECT") && screen.contains("CANCEL"), "buttons missing");

        // VST3 is offered as a filter but hosts nothing yet.
        let vst3 = SOURCE_FORMATS.iter().position(|f| *f == "VST3").unwrap();
        app.modal.as_mut().unwrap().list.filter = vst3;
        app.refresh_modal();
        assert!(app.modal.as_ref().unwrap().list.items.is_empty(), "VST3 isn't hosted yet");

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
        assert_eq!(app.modal.as_ref().unwrap().list.cursor, 1, "clicking a row selects it");

        handle_modal_mouse(&mut app, click(cancel.x + 1, cancel.y));
        assert!(app.modal.is_none(), "CANCEL closes the modal");
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
        app.apply_cc(74, 127);
        assert_eq!(app.cc_bindings, vec![(74, LearnTarget::Gain(0))]);
        assert_eq!(app.slots[0].gain, 1.0, "the binding message itself doesn't move it");
        assert!(app.learn.is_none(), "learn disarms after binding");

        // Now it drives the fader; an unbound CC does nothing.
        app.apply_cc(74, 64);
        assert!((app.slots[0].gain - 64.0 / 127.0 * MAX_GAIN).abs() < 1e-6);
        let gain = app.slots[0].gain;
        app.apply_cc(9, 0);
        assert_eq!(app.slots[0].gain, gain, "an unbound CC is ignored");
    }

    /// Draw the RACK panel over a test backend and return (screen, rects).
    fn render_rack(app: &mut App, w: u16, h: u16) -> (String, RackLayout) {
        let _g = ui_guard();
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        let mut rack = RackLayout::default();
        let preset = app.active_preset_label();
        term.draw(|f| {
            let mix = app.slots.get(app.active_slot).map(|s| (s.gain, s.pan, s.mute, s.solo));
            let tabs: Vec<String> = app.slots.iter().map(tab_label).collect();
            rack = views::fx_chain_panel::draw_fx_chain_panel(
                f, f.area(), &app.fx_chain, app.fx_slot, app.fx_param, true,
                &tabs, app.active_slot, mix, &app.instrument_label(), preset.as_deref(),
            );
        })
        .unwrap();
        let screen = term.backend().buffer().content().iter().map(|c| c.symbol()).collect();
        let mut layout = app.layout.borrow_mut();
        layout.rack = rack.clone();
        // The mouse router checks the panel area before the rack rects.
        layout.fx_chain_area = ratatui::layout::Rect::new(0, 0, w, h);
        drop(layout);
        (screen, rack)
    }

    /// An FX with more knobs than fit across the panel wraps onto further rows,
    /// and every knob drawn is clickable at the position it was drawn.
    #[test]
    fn wide_fx_wraps_its_knobs_onto_more_rows() {
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        // Z5 Texture has 16 parameters — far more than one row of knobs.
        app.fx_chain.push(AudioFxEntry::new(source::AudioFxKind::Z5Texture));
        let n = app.fx_chain[0].param_descs().len();
        assert!(n > 6, "this test needs a wide FX, got {n} params");

        let (screen, rack) = render_rack(&mut app, 100, 30);
        assert!(rack.params.len() > 7, "only {} knobs drawn", rack.params.len());
        // Knobs on the second row sit strictly below the first row's.
        let first_y = rack.params[0].1.y;
        assert!(rack.params.iter().any(|(_, r)| r.y > first_y), "nothing wrapped:\n{screen}");
        // The slot controls kept their own box below the knobs.
        assert!(screen.contains("SLOT") && screen.contains("DEL"), "slot box missing:\n{screen}");
        let del = rack.del.expect("DEL is clickable");
        assert!(del.y > first_y, "DEL must sit below the knob grid");
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
        let rows: std::collections::BTreeSet<u16> = rack.fx_slots.iter().map(|(_, r)| r.y).collect();
        assert!(rows.len() > 1, "a narrow panel must wrap the chain onto more lines");
        for &(_, r) in rack.fx_slots.iter() {
            assert!(r.x + r.width <= 46, "a chain button ran off the panel: {r:?}");
        }
    }

    /// MIDI learn by pointer: arm it, click a fader, then the next CC binds and
    /// the pointer mode ends.
    #[test]
    fn pointer_learn_picks_the_clicked_control_then_binds_the_cc() {
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.fx_chain.push(AudioFxEntry::new(source::AudioFxKind::Delay));
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
        assert!(app.learn_pick, "the ? pointer stays up while it listens for a CC");
        assert_eq!(app.slots[0].gain, 1.0, "the pick click must not move the fader");

        app.apply_cc(21, 127);
        assert_eq!(app.cc_bindings, vec![(21, LearnTarget::Gain(0))]);
        assert!(!app.learn_pick && app.learn.is_none(), "learn ends once bound");

        // A knob can be picked the same way.
        app.learn_pick = true;
        let (_, param_rect) = rack.params[1];
        handle_mouse(&mut app, click(param_rect.x + 1, param_rect.y + 1));
        assert_eq!(app.learn, Some(LearnTarget::FxParam { slot: 0, fx: 0, param: 1 }));
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
            sources::Sf2Preset { bank: 0, preset: 0, name: "Grand Piano".into() },
            sources::Sf2Preset { bank: 0, preset: 1, name: "Bright Piano".into() },
        ];
        app.source = app.slots[0].source.clone();

        let (screen, rack) = render_rack(&mut app, 100, 30);
        assert!(screen.contains("Grand Piano"), "the current bank must be named:\n{screen}");
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
        assert_eq!(app.active_preset_label().as_deref(), Some("000:001 Bright Piano"));

        // The same button is MIDI-learnable: pick it with the pointer, bind a CC.
        app.learn_pick = true;
        handle_mouse(&mut app, click(next.x + 1, next.y));
        assert_eq!(app.learn, Some(LearnTarget::Trigger(TriggerAction::PresetNext)));
        assert_eq!(app.slots[0].preset_cursor, 1, "picking must not press the button");
        app.apply_cc(30, 127);
        assert_eq!(app.cc_bindings, vec![(30, LearnTarget::Trigger(TriggerAction::PresetNext))]);

        // Buttons fire on the rising edge only.
        app.slots[0].preset_cursor = 0;
        app.apply_cc(30, 10);
        assert_eq!(app.slots[0].preset_cursor, 0, "below half-scale does nothing");
        app.apply_cc(30, 127);
        assert_eq!(app.slots[0].preset_cursor, 1, "crossing half-scale presses it");
        app.apply_cc(30, 120);
        assert_eq!(app.slots[0].preset_cursor, 1, "held high doesn't retrigger");
    }

    /// SLOT buttons (bar DEL) are learnable, and DEL deliberately isn't.
    #[test]
    fn slot_buttons_are_learnable_except_delete() {
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.fx_chain.push(AudioFxEntry::new(source::AudioFxKind::Delay));
        app.fx_chain.push(AudioFxEntry::new(source::AudioFxKind::Reverb));
        let (_, rack) = render_rack(&mut app, 100, 32);

        let at = |app: &App, r: ratatui::layout::Rect| {
            app.learn_target_at((r.x + 1, r.y).into())
        };
        assert_eq!(
            at(&app, rack.on_off.unwrap()),
            Some(LearnTarget::Trigger(TriggerAction::FxToggle))
        );
        assert_eq!(
            at(&app, rack.move_right.unwrap()),
            Some(LearnTarget::Trigger(TriggerAction::FxMoveRight))
        );
        assert_eq!(at(&app, rack.del.unwrap()), None, "DEL must never be bindable");
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
        app.fx_chain.push(AudioFxEntry::new(source::AudioFxKind::Delay));
        let (screen, _) = render_rack(&mut app, 100, 30);
        assert_eq!(screen.matches("DELAY").count(), 2, "chain button + param box title only");
        assert!(!screen.contains("OUT"), "the routing line is gone:\n{screen}");
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

    /// Settings is three tabs: paths, text colour and language. Picking a colour
    /// or a language applies it and is what the panels then draw with.
    #[test]
    fn settings_tabs_switch_colour_and_language() {
        use i18n::Lang;
        let _g = ui_guard();
        let _restore = UiRestore;
        sandbox_state_dir();
        let mut app = App::new();
        app.open_paths_modal();
        let tabs = app.modal.as_ref().unwrap().list.filters.clone();
        assert_eq!(tabs, vec!["AUDIO", "COLOR", "LANGUAGE"]);

        // Colour tab: rows are the palette, Enter applies.
        app.modal.as_mut().unwrap().list.filter = TAB_COLOR;
        app.refresh_modal();
        let items = app.modal.as_ref().unwrap().list.items.clone();
        assert_eq!(items.len(), settings::PALETTE.len());
        assert!(items[0].contains("Default"));
        app.modal.as_mut().unwrap().list.cursor = 2;
        assert!(app.modal_select(), "SELECT applies the colour and closes the modal");
        assert_eq!(app.ui.text_color, settings::PALETTE[2].1);
        assert_eq!(views::theme::text(), app.ui.color(), "panels draw with it");
        // Borders follow the same colour, dimmed.
        assert_ne!(views::theme::border(), views::theme::text());
        assert!(matches!(views::theme::border(), ratatui::style::Color::Rgb(r, _, _) if r > 0));
        // The marker moves to the chosen row.
        assert!(app.modal.as_ref().unwrap().list.items[2].contains('\u{25CF}'));

        // Language tab: every shipped language is listed, Enter switches.
        app.modal.as_mut().unwrap().list.filter = TAB_LANG;
        app.refresh_modal();
        let items = app.modal.as_ref().unwrap().list.items.clone();
        assert_eq!(items.len(), Lang::ALL.len());
        let es = Lang::ALL.iter().position(|l| *l == Lang::Es).unwrap();
        app.modal.as_mut().unwrap().list.cursor = es;
        assert!(app.modal_select(), "SELECT switches the language and closes");
        assert_eq!(app.ui.language, Lang::Es);
        assert_eq!(i18n::t("SETTINGS"), "AJUSTES", "the interface follows");
        // The tab labels themselves are translated now.
        assert_eq!(app.modal.as_ref().unwrap().list.filters[2], "IDIOMA");
        assert_eq!(app.modal.as_ref().unwrap().list.filters[1], i18n::t("COLOR"));
        // `_restore` puts English (and the default colour) back on the way out.
    }

    /// ADD FX has a category sidebar on the left and format chips on top: the
    /// sidebar picks what the list shows, the chips narrow both.
    #[test]
    fn add_fx_sidebar_picks_the_category_and_chips_the_format() {
        use choz_engine::{FoundPlugin, PluginFormat};
        let mut app = App::new();
        app.slots.push(RackSlot::new(AudioSource::Midi));
        app.plugins.push(FoundPlugin {
            format: PluginFormat::Lv2,
            name: "Calf Reverb".into(),
            path: "/usr/lib/lv2/calf.lv2".into(),
            id: String::new(),
            is_instrument: false,
        });
        app.plugins.push(FoundPlugin {
            format: PluginFormat::Vst2,
            name: "TapeDelay".into(),
            path: "/usr/lib/vst/tape.so".into(),
            id: String::new(),
            is_instrument: false,
        });
        app.open_add_fx_modal();

        // The sidebar lists ALL plus every non-empty category, with counts.
        let sidebar = app.modal.as_ref().unwrap().list.sidebar.clone();
        assert_eq!(sidebar[0].0, "ALL");
        assert_eq!(sidebar[0].1, app.fx_menu_entries().len(), "ALL counts everything");
        for cat in ["DELAY", "REVERB", "DISTORTION"] {
            assert!(sidebar.iter().any(|(l, n)| l == cat && *n > 0), "{cat} missing: {sidebar:?}");
        }

        // Selecting REVERB shows only reverbs — including the LV2 one, whose
        // category was guessed from its name.
        let reverb = sidebar.iter().position(|(l, _)| l == "REVERB").unwrap();
        app.modal.as_mut().unwrap().list.sidebar_cursor = reverb;
        app.refresh_modal();
        let items = app.modal.as_ref().unwrap().list.items.clone();
        assert!(items.iter().any(|i| i.contains("[LV2] Calf Reverb")), "{items:#?}");
        assert!(!items.iter().any(|i| i.contains("DELAY")), "only reverbs now: {items:#?}");
        // The VST2 name says delay, so it sits in DELAY instead.
        let delay = sidebar.iter().position(|(l, _)| l == "DELAY").unwrap();
        app.modal.as_mut().unwrap().list.sidebar_cursor = delay;
        app.refresh_modal();
        assert!(app.modal.as_ref().unwrap().list.items.iter().any(|i| i.contains("TapeDelay")));

        // The format chips narrow the sidebar too: under LV2 only the reverb
        // section survives.
        let lv2 = FX_FORMATS.iter().position(|f| *f == "LV2").unwrap();
        app.modal.as_mut().unwrap().list.filter = lv2;
        app.modal.as_mut().unwrap().list.sidebar_cursor = 0;
        app.refresh_modal();
        let sidebar = app.modal.as_ref().unwrap().list.sidebar.clone();
        assert_eq!(sidebar, vec![("ALL".to_string(), 1), ("REVERB".to_string(), 1)]);
        let items = app.modal.as_ref().unwrap().list.items.clone();
        assert!(items.iter().all(|i| i.contains("[LV2]")), "only LV2 now:\n{items:#?}");

        // ← / → move between the two panes; Enter on the sidebar jumps into the
        // list, and Enter there adds the FX.
        app.modal.as_mut().unwrap().list.filter = 0;
        app.refresh_modal();
        handle_modal_key(&mut app, KeyCode::Left);
        assert!(app.modal.as_ref().unwrap().list.sidebar_focused);
        handle_modal_key(&mut app, KeyCode::Down);
        assert_eq!(app.modal.as_ref().unwrap().list.sidebar_cursor, 1, "↓ moves the sidebar");
        handle_modal_key(&mut app, KeyCode::Enter);
        assert!(!app.modal.as_ref().unwrap().list.sidebar_focused, "Enter enters the list");
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
        for label in ["Backend", "Device", "Sample rate", "Buffer size", "SF2 engine", "Latency"] {
            assert!(rows.contains(label), "{label} missing:\n{rows}");
        }
        assert!(rows.contains("5.3 ms"), "latency is computed: {rows}");

        // → on the backend row cycles it; sample rate and buffer likewise.
        app.modal.as_mut().unwrap().list.sidebar_focused = false;
        app.modal.as_mut().unwrap().list.cursor = 0;
        assert!(app.audio_settings_key(KeyCode::Right));
        assert_eq!(app.ui.audio.backend, "JACK");
        app.modal.as_mut().unwrap().list.cursor = 2;
        app.audio_settings_key(KeyCode::Right);
        assert_eq!(app.ui.audio.sample_rate, 88_200);
        app.modal.as_mut().unwrap().list.cursor = 3;
        app.audio_settings_key(KeyCode::Left);
        assert_eq!(app.ui.audio.buffer_size, 128);
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
        assert!(app.modal.as_ref().unwrap().list.items[3].contains("9100\u{2588}"), "caret shown");
        app.audio_settings_key(KeyCode::Enter);
        assert_eq!(app.ui.osc.tcp_port, 9100);
        assert!(app.port_edit.is_none());

        // Everything was persisted.
        let saved = settings::UiSettings::load();
        assert_eq!(saved.audio.backend, "JACK");
        assert_eq!(saved.osc.tcp_port, 9100);
    }

    /// Clicking a category in the ADD FX sidebar shows that category — through
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
        let (_, rect) = sidebar.iter().find(|(i, _)| *i == reverb).expect("REVERB row drawn");

        handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: rect.x + 1,
                row: rect.y,
                modifiers: crossterm::event::KeyModifiers::NONE,
            },
        );
        assert_eq!(app.modal.as_ref().unwrap().list.sidebar_cursor, reverb, "the click selected it");
        let items = app.modal.as_ref().unwrap().list.items.clone();
        assert!(items.iter().any(|i| i.contains("REVERB")), "{items:#?}");
        assert!(!items.iter().any(|i| i.contains("DELAY")), "only reverbs now: {items:#?}");
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
        app.fx_chain.push(AudioFxEntry::new(source::AudioFxKind::AmberFang));
        app.cc_bindings.push((74, LearnTarget::Gain(0)));
        app.midi_disabled.push("Midi Through".into());

        let dir = std::env::temp_dir().join(format!("choz_save_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        app.save_project_to(&dir);

        let yaml = std::fs::read_to_string(dir.join(project::DEFAULT_NAME)).unwrap();
        // Sound settings…
        assert!(yaml.contains("kind: sf2"), "{yaml}");
        assert!(yaml.contains("preset: 4"));
        assert!(yaml.contains("kind: amberfang"), "the FX chain and its knobs are in there");
        assert!(yaml.contains("gain: 0.8"));
        assert!(yaml.contains("MIDI:Keystation"));
        assert!(yaml.contains("- 74"), "MIDI-learn bindings are saved");
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
        app.plugin_paths.dirs_mut(PluginFormat::Sfz).push(SearchDir {
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
        assert!(row.contains("SF2"), "and it says what it really holds: {row}");

        // Filed correctly (and scanned), it shows its count instead.
        app.plugin_paths.dirs_mut(PluginFormat::Sfz).pop();
        app.plugin_paths.dirs_mut(PluginFormat::Sf2).push(SearchDir {
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
        app.plugin_paths.dirs_mut(PluginFormat::Sf2).last_mut().unwrap().enabled = false;
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

        let idx = app.path_rows().iter().position(|(_, d)| d.is_some()).unwrap();
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
        let screen: String =
            term.backend().buffer().content().iter().map(|c| c.symbol()).collect();
        for label in ["EDIT", "ADD", "BROWSE", "REMOVE", "DEFAULTS"] {
            assert!(screen.contains(label), "{label} button missing:\n{screen}");
        }

        // Put the cursor on a real directory row, then click EDIT.
        let idx = app.path_rows().iter().position(|(_, d)| d.is_some()).unwrap();
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
        assert!(app.path_edit.is_some(), "the EDIT button opens the path editor");
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
        assert_eq!(app.path_edit.as_ref().unwrap().buf, before.display().to_string());
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
        assert_eq!(app.plugin_paths.dirs(fmt)[i].path, std::path::Path::new("/opt/vst"));

        // Esc discards, and `a` types a brand new entry.
        assert!(app.paths_modal_key(KeyCode::Char('e')));
        app.paths_modal_key(KeyCode::Char('X'));
        app.paths_modal_key(KeyCode::Esc);
        assert_eq!(app.plugin_paths.dirs(fmt)[i].path, std::path::Path::new("/opt/vst"));
        assert!(app.modal.is_some(), "Esc leaves the edit, not the modal");

        let count = app.plugin_paths.dirs(fmt).len();
        assert!(app.paths_modal_key(KeyCode::Char('a')));
        for c in "/srv/plugins".chars() {
            app.paths_modal_key(KeyCode::Char(c));
        }
        app.paths_modal_key(KeyCode::Enter);
        assert_eq!(app.plugin_paths.dirs(fmt).len(), count + 1);
        assert!(app.plugin_paths.dirs(fmt).iter().any(|d| d.path == std::path::Path::new("/srv/plugins")));

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
            assert!(items.iter().any(|i| i == fmt.label()), "{} missing", fmt.label());
        }

        // Put the cursor on the first LV2 directory and switch it off.
        let rows = app.path_rows();
        let (idx, &(fmt, dir)) = rows
            .iter()
            .enumerate()
            .find(|(_, (f, d))| *f == choz_engine::PluginFormat::Lv2 && d.is_some())
            .expect("LV2 has default directories");
        app.modal.as_mut().unwrap().list.cursor = idx;
        assert!(app.paths_modal_key(KeyCode::Enter), "Enter toggles a directory");
        let i = dir.unwrap();
        assert!(!app.plugin_paths.dirs(fmt)[i].enabled);
        assert!(
            !app.plugin_paths.all_enabled().contains(&app.plugin_paths.dirs(fmt)[i].path),
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

    #[test]
    fn notes_reach_only_the_tabs_bound_to_their_input() {
        let keys = InputRef::Midi("Keystation".to_string());
        let other = InputRef::Midi("Midi Through".to_string());
        // tab 0 ← Keystation, tab 1 ← OSC, tab 2 ← Keystation, tab 3 unbound.
        let bindings = vec![Some(&keys), Some(&InputRef::Osc), Some(&keys), None];
        let connected = vec!["Keystation".to_string(), "Midi Through".to_string()];

        assert_eq!(note_targets(&bindings, &connected, 3, InputSource::Midi(0)), vec![0, 2]);
        assert_eq!(note_targets(&bindings, &connected, 3, InputSource::Osc), vec![1]);
        assert!(
            note_targets(&bindings, &connected, 3, InputSource::Midi(1)).is_empty(),
            "no tab is bound to {other:?}"
        );
        assert!(
            note_targets(&bindings, &connected, 3, InputSource::Midi(9)).is_empty(),
            "unknown port index is dropped, not panicked on"
        );
    }

    #[test]
    fn the_qwerty_piano_always_plays_the_active_tab() {
        let osc = InputRef::Osc;
        let bindings = vec![Some(&osc), None];
        assert_eq!(note_targets(&bindings, &[], 1, InputSource::Keyboard), vec![1]);
        assert_eq!(note_targets(&bindings, &[], 0, InputSource::Keyboard), vec![0],
            "even a bound tab is playable from the keyboard");
        assert!(note_targets(&[], &[], 0, InputSource::Keyboard).is_empty(), "empty rack");
    }
}
