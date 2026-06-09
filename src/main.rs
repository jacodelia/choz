//! choz — A terminal-based audio plugin host inspired by Carla.
//!
//! Provides a TUI for managing audio sources (MIDI, SF2, audio files, plugins)
//! and FX chains, feeding a real-time audio engine via cpal.
//!
//! UI styling adapted from seqterm.

mod fx;
mod fx_chain;
mod plugin_types;
mod scanner;
mod registry;
mod source;
mod engine;
mod views;

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
use fx_chain::FxSpec;
use registry::PluginRegistry;
use views::{SOURCE_CATEGORIES, FX_CELL_W};
use views::theme::*;
use views::splash::{SplashState, draw_splash, is_active};

/// A discovered synthesizer plugin.
#[derive(Debug, Clone)]
pub struct SynthEntry {
    pub id: String,
    pub format: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Source,
    FxChain,
    FxKindSelector,
    Transport,
}

struct FxKindSelector {
    cursor: usize,
    scroll: usize,
}

#[derive(Default, Clone)]
struct UiLayout {
    source_area: Rect,
    fx_chain_area: Rect,
    transport_area: Rect,
    source_cat_rects: [Rect; 4],
    fx_slot_rects: Vec<Rect>,
    fx_param_rects: Vec<Rect>,
    fx_add_rect: Option<Rect>,
    fx_on_off_rect: Option<Rect>,
    fx_del_rect: Option<Rect>,
    fx_move_left_rect: Option<Rect>,
    fx_move_right_rect: Option<Rect>,
    play_btn_rect: Rect,
    stop_btn_rect: Rect,
    fx_sel_area: Option<Rect>,
    fx_sel_items: Vec<Rect>,
    fx_sel_close_rect: Option<Rect>,
}

#[allow(dead_code)]
struct App {
    source: AudioSource,
    source_cat: usize,
    midi_ports: Vec<String>,
    synths: Vec<SynthEntry>,

    fx_chain: Vec<AudioFxEntry>,
    fx_slot: usize,
    fx_param: usize,

    focus: Focus,
    fx_selector: Option<FxKindSelector>,

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
        let mut registry = PluginRegistry::with_default_adapters();
        let synths = Self::discover_synths(&mut registry);

        Self {
            source: AudioSource::Midi,
            source_cat: 0,
            midi_ports: vec!["default".to_string()],
            synths,
            fx_chain: Vec::new(),
            fx_slot: 0,
            fx_param: 0,
            focus: Focus::Source,
            fx_selector: None,
            registry,
            audio_engine: None,
            playing: false,
            quit: false,
            layout: RefCell::new(UiLayout::default()),
            splash: SplashState::new(),
            splash_done: false,
        }
    }

    fn discover_synths(registry: &mut PluginRegistry) -> Vec<SynthEntry> {
        registry.scan_default_locations(&[]);
        let plugins: Vec<_> = registry.list_plugins().into_iter().cloned().collect();
        plugins.into_iter().map(|p| SynthEntry {
            id: p.id, format: p.kind.label().to_string(), name: p.name,
        }).collect()
    }

    fn rebuild_fx(&mut self) {
        if let Some(ref engine) = self.audio_engine {
            let specs: Vec<FxSpec> = self.fx_chain.iter().map(|e| e.to_spec()).collect();
            engine.rebuild_fx_chain(specs);
        }
    }
}

fn main() -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();

    let result = run_app(&mut terminal, &mut app);

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
                let mut eng = engine::AudioEngine::new(48000, 256);
                if eng.start().is_ok() {
                    app.audio_engine = Some(eng);
                }
            }
            if !is_active(&app.splash) {
                app.splash_done = true;
            }
        }

        handle_events(app)?;
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
    match key {
        KeyCode::Char('q') => { app.quit = true; return; }
        KeyCode::Esc
            if app.fx_selector.is_some() => {
                app.fx_selector = None;
                app.focus = Focus::FxChain;
                return;
            }
        _ => {}
    }

    if let Some(ref mut sel) = app.fx_selector {
        match key {
            KeyCode::Up => { sel.cursor = sel.cursor.saturating_sub(1); }
            KeyCode::Down => { sel.cursor = (sel.cursor + 1).min(ALL_FX_KINDS.len() - 1); }
            KeyCode::Enter => {
                let kind = ALL_FX_KINDS[sel.cursor];
                if app.fx_chain.len() < 8 {
                    app.fx_chain.push(AudioFxEntry::new(kind));
                    app.rebuild_fx();
                }
                app.fx_selector = None;
                app.focus = Focus::FxChain;
            }
            _ => {}
        }
        return;
    }

    if key == KeyCode::Tab {
        app.focus = match app.focus {
            Focus::Source => Focus::FxChain,
            Focus::FxChain => Focus::Transport,
            Focus::Transport => Focus::Source,
            _ => Focus::Source,
        };
        return;
    }

    match app.focus {
        Focus::Source => handle_source_keys(app, key),
        Focus::FxChain => handle_fx_keys(app, key),
        Focus::Transport => handle_transport_keys(app, key),
        Focus::FxKindSelector => {}
    }
}

fn handle_source_keys(app: &mut App, key: KeyCode) {
    match key {
        KeyCode::Left | KeyCode::Right => {
            app.source_cat = match key {
                KeyCode::Left => app.source_cat.saturating_sub(1),
                _ => (app.source_cat + 1).min(3),
            };
            if app.source_cat <= 3 { app.source_cat = app.source_cat.min(3); }
        }
        KeyCode::Down => {
            app.source_cat = 0;
            app.source = AudioSource::Midi;
        }
        KeyCode::Up => {
            app.source_cat = 3;
            if !app.synths.is_empty() {
                app.source = AudioSource::Plugin {
                    id: app.synths[0].id.clone(),
                    format: app.synths[0].format.clone(),
                    name: app.synths[0].name.clone(),
                };
            }
        }
        KeyCode::Char('1') => { app.source_cat = 0; app.source = AudioSource::Midi; }
        KeyCode::Char('2') => { app.source_cat = 1; }
        KeyCode::Char('3') => { app.source_cat = 2; }
        KeyCode::Char('4') => { app.source_cat = 3; }
        _ => {}
    }
}

fn handle_fx_keys(app: &mut App, key: KeyCode) {
    match key {
        KeyCode::Left => { app.fx_slot = app.fx_slot.saturating_sub(1); app.fx_param = 0; }
        KeyCode::Right
            if app.fx_slot + 1 < app.fx_chain.len() => {
                app.fx_slot += 1; app.fx_param = 0;
            }
        KeyCode::Up => { app.fx_param = app.fx_param.saturating_sub(1); }
        KeyCode::Down => {
            if let Some(entry) = app.fx_chain.get(app.fx_slot) {
                let max = source::fx_param_descs(entry.kind).len();
                if app.fx_param + 1 < max { app.fx_param += 1; }
            }
        }
        KeyCode::Char('w') | KeyCode::Char('W') => adjust_fx_param(app, 0.05),
        KeyCode::Char('s') | KeyCode::Char('S') => adjust_fx_param(app, -0.05),
        KeyCode::Char(' ') => {
            if let Some(entry) = app.fx_chain.get_mut(app.fx_slot) {
                entry.enabled = !entry.enabled;
                app.rebuild_fx();
            }
        }
        KeyCode::Char('a') => {
            app.fx_selector = Some(FxKindSelector { cursor: 0, scroll: 0 });
            app.focus = Focus::FxKindSelector;
        }
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

fn adjust_fx_param(app: &mut App, delta: f32) {
    if let Some(entry) = app.fx_chain.get_mut(app.fx_slot) {
        if let Some(v) = entry.params.get_mut(app.fx_param) {
            *v = (*v + delta).clamp(0.0, 1.0);
        }
        app.rebuild_fx();
    }
}

fn handle_transport_keys(app: &mut App, key: KeyCode) {
    match key {
        KeyCode::Char(' ') => toggle_play(app),
        KeyCode::Char('s') => stop_play(app),
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
    SourceCat(usize),
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
    FxSelItem(usize),
    FxSelClose,
}

fn mouse_action(col: u16, row: u16, layout: &UiLayout, kind: MouseEventKind) -> MouseAction {
    let pos: ratatui::layout::Position = (col, row).into();

    match kind {
        MouseEventKind::Down(MouseButton::Left) => {
            // FX selector modal — highest priority
            if let Some(ref sel_area) = layout.fx_sel_area {
                if let Some(ref close_rect) = layout.fx_sel_close_rect {
                    if close_rect.contains(pos) {
                        return MouseAction::FxSelClose;
                    }
                }
                if sel_area.contains(pos) {
                    for (i, item_rect) in layout.fx_sel_items.iter().enumerate() {
                        if item_rect.contains(pos) {
                            return MouseAction::FxSelItem(i);
                        }
                    }
                    return MouseAction::None;
                }
                return MouseAction::FxSelClose;
            }

            if layout.play_btn_rect.contains(pos) {
                return MouseAction::TransportPlay;
            }
            if layout.stop_btn_rect.contains(pos) {
                return MouseAction::TransportStop;
            }

            if layout.source_area.contains(pos) {
                for (i, cat_rect) in layout.source_cat_rects.iter().enumerate() {
                    if cat_rect.contains(pos) {
                        return MouseAction::SourceCat(i);
                    }
                }
                return MouseAction::FocusPanel(Focus::Source);
            }

            if layout.fx_chain_area.contains(pos) {
                for (i, slot_rect) in layout.fx_slot_rects.iter().enumerate() {
                    if slot_rect.contains(pos) {
                        return MouseAction::FxSlot(i);
                    }
                }
                for (pi, param_rect) in layout.fx_param_rects.iter().enumerate() {
                    if param_rect.contains(pos) {
                        return MouseAction::FxParam(pi);
                    }
                }
                if let Some(r) = layout.fx_add_rect {
                    if r.contains(pos) { return MouseAction::FxAdd; }
                }
                if let Some(r) = layout.fx_on_off_rect {
                    if r.contains(pos) { return MouseAction::FxToggle; }
                }
                if let Some(r) = layout.fx_del_rect {
                    if r.contains(pos) { return MouseAction::FxDelete; }
                }
                if let Some(r) = layout.fx_move_left_rect {
                    if r.contains(pos) { return MouseAction::FxMoveLeft; }
                }
                if let Some(r) = layout.fx_move_right_rect {
                    if r.contains(pos) { return MouseAction::FxMoveRight; }
                }
                return MouseAction::FocusPanel(Focus::FxChain);
            }

            if layout.transport_area.contains(pos) {
                return MouseAction::FocusPanel(Focus::Transport);
            }

            MouseAction::None
        }
        MouseEventKind::ScrollDown => {
            if layout.fx_chain_area.contains(pos) {
                for (pi, param_rect) in layout.fx_param_rects.iter().enumerate() {
                    if param_rect.contains(pos) {
                        return MouseAction::FxParamAdjust(pi, -0.03);
                    }
                }
            }
            MouseAction::None
        }
        MouseEventKind::ScrollUp => {
            if layout.fx_chain_area.contains(pos) {
                for (pi, param_rect) in layout.fx_param_rects.iter().enumerate() {
                    if param_rect.contains(pos) {
                        return MouseAction::FxParamAdjust(pi, 0.03);
                    }
                }
            }
            MouseAction::None
        }
        _ => MouseAction::None,
    }
}

fn handle_mouse(app: &mut App, mouse: MouseEvent) {
    let action = {
        let layout = app.layout.borrow();
        mouse_action(mouse.column, mouse.row, &layout, mouse.kind)
    };

    match action {
        MouseAction::None => {}
        MouseAction::FocusPanel(f) => { app.focus = f; }
        MouseAction::SourceCat(i) => {
            app.focus = Focus::Source;
            app.source_cat = i;
            if i == 0 {
                app.source = AudioSource::Midi;
            } else if i == 3 && !app.synths.is_empty() {
                app.source = AudioSource::Plugin {
                    id: app.synths[0].id.clone(),
                    format: app.synths[0].format.clone(),
                    name: app.synths[0].name.clone(),
                };
            }
        }
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
        MouseAction::FxAdd => {
            app.fx_selector = Some(FxKindSelector { cursor: 0, scroll: 0 });
            app.focus = Focus::FxKindSelector;
        }
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
        MouseAction::FxSelItem(i) => {
            if let Some(ref mut sel) = app.fx_selector {
                sel.cursor = i + sel.scroll;
            }
        }
        MouseAction::FxSelClose => {
            app.fx_selector = None;
            app.focus = Focus::FxChain;
        }
    }
}

// ─── UI Render ─────────────────────────────────────────────────────────────────

fn ui(f: &mut Frame, app: &App) {
    let area = f.area();

    // ─── Splash Screen ──────────────────────────────────────────────────
    if !app.splash_done {
        draw_splash(f, &app.splash, area);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    let right_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(10), Constraint::Length(7)])
        .split(chunks[1]);

    let source_area = chunks[0];
    let fx_chain_area = right_chunks[0];
    let transport_area = right_chunks[1];

    views::source_panel::draw_source_panel(
        f, source_area, &app.source, app.focus == Focus::Source,
        app.source_cat, &app.midi_ports, &app.synths,
    );

    views::fx_chain_panel::draw_fx_chain_panel(
        f, fx_chain_area, &app.fx_chain,
        app.fx_slot, app.fx_param, app.focus == Focus::FxChain,
    );

    draw_transport(f, app, transport_area);

    if let Some(ref sel) = app.fx_selector {
        draw_fx_selector(f, sel, area);
    }

    // Status bar
    let status_area = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area)[1];

    let backend_label = app.audio_engine
        .as_ref()
        .map(|e| e.backend.label())
        .unwrap_or("none");
    let play_icon = if app.playing { "\u{25B6}" } else { "\u{25A0}" };
    let play_state = if app.playing { "PLAYING" } else { "STOPPED" };

    let status_text = format!(
        " choz v0.1 | {} backend | SOURCE: {} | FX: {} | {play_icon} {play_state} | Tab=switch q=quit",
        backend_label,
        app.source.kind_label(),
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

fn compute_layout(app: &App, area: Rect, source_area: Rect, fx_chain_area: Rect, transport_area: Rect) {
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
    let cat_y = src_inner.y + 2;
    let mut cat_x = src_inner.x;
    for (i, &cat) in SOURCE_CATEGORIES.iter().enumerate() {
        let w = cat.len() as u16 + 2;
        layout.source_cat_rects[i] = Rect::new(cat_x, cat_y, w, 1);
        cat_x += w;
    }

    let fx_inner = Rect::new(
        fx_chain_area.x + 1,
        fx_chain_area.y + 1,
        fx_chain_area.width.saturating_sub(2),
        fx_chain_area.height.saturating_sub(2),
    );
    let fx_cy = fx_inner.y;

    let slot_y = fx_cy + 1;
    layout.fx_slot_rects.clear();
    let mut slot_x = fx_inner.x + 2;
    for (i, entry) in app.fx_chain.iter().enumerate() {
        let text = format!(" {}:{} ", i + 1, entry.kind.label());
        let w = text.len() as u16;
        layout.fx_slot_rects.push(Rect::new(slot_x, slot_y, w, 1));
        slot_x += w;
    }
    if app.fx_chain.len() < 5 {
        let add_text = " [+ ADD] ";
        layout.fx_add_rect = Some(Rect::new(slot_x, slot_y, add_text.len() as u16, 1));
    } else {
        layout.fx_add_rect = None;
    }

    layout.fx_param_rects.clear();
    if let Some(entry) = app.fx_chain.get(app.fx_slot) {
        let descs = source::fx_param_descs(entry.kind);
        let n = descs.len();
        let avail = fx_inner.width.saturating_sub(2) as usize;
        let visible = (avail / FX_CELL_W as usize).max(1);
        let focused = app.focus == Focus::FxChain;
        let start = if focused && app.fx_param >= visible { app.fx_param + 1 - visible } else { 0 };
        let end = (start + visible).min(n);
        let param_y_base = fx_cy + 4;

        for pi in start..end {
            let col = pi - start;
            let x = fx_inner.x + 2 + (col as u16) * FX_CELL_W;
            layout.fx_param_rects.push(Rect::new(x, param_y_base, FX_CELL_W, 3));
        }
    }

    let ctrl_y = fx_cy + 7;
    layout.fx_on_off_rect = None;
    layout.fx_move_left_rect = None;
    layout.fx_move_right_rect = None;
    layout.fx_del_rect = None;

    if let Some(entry) = app.fx_chain.get(app.fx_slot) {
        let mut ctrl_x = fx_inner.x + 2;
        let on_off = if entry.enabled { " ON " } else { " OFF " };
        layout.fx_on_off_rect = Some(Rect::new(ctrl_x, ctrl_y, on_off.len() as u16, 1));
        ctrl_x += on_off.len() as u16 + 1;

        if app.fx_slot > 0 {
            let mv_text = " <-MOVE ";
            layout.fx_move_left_rect = Some(Rect::new(ctrl_x, ctrl_y, mv_text.len() as u16, 1));
            ctrl_x += mv_text.len() as u16 + 1;
        }
        if app.fx_slot + 1 < app.fx_chain.len() {
            let mv_text = " MOVE-> ";
            layout.fx_move_right_rect = Some(Rect::new(ctrl_x, ctrl_y, mv_text.len() as u16, 1));
            ctrl_x += mv_text.len() as u16 + 1;
        }
        let del_text = " DEL ";
        layout.fx_del_rect = Some(Rect::new(ctrl_x, ctrl_y, del_text.len() as u16, 1));
    }

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

    if let Some(ref sel) = app.fx_selector {
        let popup = centered_rect(50, 60, area);
        let sel_inner = popup.inner(Margin { vertical: 2, horizontal: 2 });
        layout.fx_sel_area = Some(popup);
        // Close button: [×] at top-right of popup
        layout.fx_sel_close_rect = Some(Rect::new(
            popup.x + popup.width - 4,
            popup.y,
            3,
            1,
        ));
        layout.fx_sel_items.clear();

        let visible = sel_inner.height as usize;
        let scroll = if sel.cursor >= sel.scroll + visible {
            sel.cursor + 1 - visible
        } else if sel.cursor < sel.scroll {
            sel.cursor
        } else {
            sel.scroll
        };

        for (i, _kind) in ALL_FX_KINDS.iter().enumerate().skip(scroll).take(visible) {
            let line = i - scroll;
            let item_rect = Rect::new(sel_inner.x, sel_inner.y + line as u16, sel_inner.width, 1);
            layout.fx_sel_items.push(item_rect);
        }
    } else {
        layout.fx_sel_area = None;
        layout.fx_sel_close_rect = None;
        layout.fx_sel_items.clear();
    }
}

// ─── Transport ────────────────────────────────────────────────────────────────

fn draw_transport(f: &mut Frame, app: &App, area: Rect) {
    let is_focused = app.focus == Focus::Transport;

    let border_style = if is_focused {
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(BORDER)
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
}

// ─── FX Selector Modal ────────────────────────────────────────────────────────

fn draw_fx_selector(f: &mut Frame, sel: &FxKindSelector, area: Rect) {
    // Backdrop
    f.render_widget(Clear, area);
    f.render_widget(
        Block::default().style(Style::default().bg(BACKDROP)),
        area,
    );

    let popup_area = centered_rect(50, 60, area);

    // Shadow
    draw_modal_shadow(f, popup_area, area);

    // Modal background
    f.render_widget(Clear, popup_area);

    let block = Block::default()
        .title(" ADD FX ")
        .title_style(Style::default().fg(HEADER).add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .style(Style::default().bg(PANEL_BG));
    let inner = block.inner(popup_area);
    f.render_widget(block, popup_area);

    // Close button [×]
    let close_x = popup_area.x + popup_area.width - 4;
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "[×]",
            Style::default().fg(ERR).add_modifier(Modifier::BOLD),
        ))),
        Rect::new(close_x, popup_area.y, 3, 1),
    );

    let content_inner = inner.inner(Margin { vertical: 0, horizontal: 1 });
    let mut lines: Vec<Line> = Vec::new();

    let visible = content_inner.height as usize;
    let scroll = if sel.cursor >= sel.scroll + visible {
        sel.cursor + 1 - visible
    } else if sel.cursor < sel.scroll {
        sel.cursor
    } else {
        sel.scroll
    };

    for (i, kind) in ALL_FX_KINDS.iter().enumerate().skip(scroll).take(visible) {
        let st = if i == sel.cursor {
            Style::default().fg(Color::Black).bg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White).bg(PANEL_BG)
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {:>2} ", i + 1),
                Style::default().fg(DIM).bg(PANEL_BG),
            ),
            Span::styled(kind.label(), st),
        ]));
    }

    f.render_widget(
        Paragraph::new(lines).style(Style::default().bg(PANEL_BG)),
        content_inner,
    );

    // Help line at the bottom
    let help_y = content_inner.y + content_inner.height.min(2);
    let help = " \u{2191}\u{2193}=select  Enter=confirm  Esc=cancel  click=select";
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            help,
            Style::default().fg(DIM),
        )))
        .style(Style::default().bg(PANEL_BG)),
        Rect::new(inner.x + 2, help_y, inner.width.saturating_sub(4), 1),
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
