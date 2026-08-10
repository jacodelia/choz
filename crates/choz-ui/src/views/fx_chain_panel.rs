//! RACK panel — tabs, mixer strip, instrument line and the insert FX chain.
//!
//! The panel computes its own click rects while it draws and hands them back in
//! a [`RackLayout`]: there is exactly one place that decides where a control
//! sits, so the hit test can't drift from the pixels the way hand-mirrored
//! offsets used to.

use ratatui::{
    layout::Rect, style::{Color, Modifier, Style}, text::{Line, Span},
    widgets::{Block, Borders, Paragraph}, Frame,
};

use crate::i18n::t;
use crate::source::{AudioFxEntry, MAX_FX, ParamShape};
use crate::views::theme::{border as ui_border, text as ui_text};

const HEADER: Color = Color::Rgb(240, 136, 62);
const LABEL: Color = Color::Rgb(120, 132, 155);
const RULE: Color = Color::Rgb(38, 44, 54);
const KNOB: Color = Color::Rgb(100, 160, 220);
/// In tune. The one colour in the AutoTune strip that means "nothing to do".
const IN_TUNE: Color = Color::Rgb(56, 200, 100);
const SEL: Color = Color::Yellow;
/// A switch reads as on or off at a glance, which is the whole reason it is not
/// drawn as an arc.
const ON_COLOUR: Color = Color::Rgb(86, 200, 120);
const OFF_COLOUR: Color = Color::Rgb(96, 104, 118);

pub const FX_CELL_W: u16 = 13;

/// Width of the close button at the right edge of a tab (the ✕ plus the pad
/// cell after it), in columns.
pub const TAB_CLOSE_W: u16 = 2;

/// Width of the `+` button that follows the last tab, in columns.
pub const TAB_ADD_W: u16 = 3;

/// Buttons on the RACK's instrument line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RackButton {
    /// The tab's MIDI channel, shown only in MULTI mode. Clicking steps it.
    Channel,
    /// Open the source/synth picker.
    Source,
    /// Open the SF2 bank/preset picker (SoundFont tabs only).
    Preset,
    /// Arm MIDI learn.
    Learn,
    /// Listen to the tab's audio input and play its instrument from the pitch.
    PitchToMidi,
    /// Open (or close) the plugin's own window.
    Gui,
    /// Ask for (or stop asking for) this plugin to run in its own process.
    Sandbox,
    /// Previous / next program of the loaded SoundFont.
    PresetPrev,
    PresetNext,
}

/// Every clickable area of the panel, filled in as it draws.
#[derive(Default, Clone)]
pub struct RackLayout {
    pub tabs: Vec<(usize, Rect)>,
    pub tab_close: Vec<(usize, Rect)>,
    /// The `+` after the last tab: another configuration on the same input.
    pub tab_add: Option<Rect>,
    pub gain: Option<Rect>,
    pub pan: Option<Rect>,
    /// Trim on the tab's audio input, and how loud that input has to be before
    /// `A→M` calls it a note. Both only exist on a tab fed by audio.
    pub in_gain: Option<Rect>,
    pub in_gate: Option<Rect>,
    pub mute: Option<Rect>,
    pub solo: Option<Rect>,
    pub buttons: Vec<(RackButton, Rect)>,
    /// (FX index, rect) for the chain buttons — they wrap onto further lines
    /// when the chain is wider than the panel.
    pub fx_slots: Vec<(usize, Rect)>,
    pub fx_add: Option<Rect>,
    /// (parameter index, rect) for the knob cells currently on screen.
    pub params: Vec<(usize, Rect)>,
    /// Same, for the instrument's own parameters — the generic panel every
    /// plugin gets whether or not it has a window.
    pub instr_knobs: Vec<(usize, Rect)>,
    pub on_off: Option<Rect>,
    pub move_left: Option<Rect>,
    pub move_right: Option<Rect>,
    pub del: Option<Rect>,
    /// The selected FX's own window button, when it's a plugin that has one.
    pub fx_gui: Option<Rect>,
    /// The selected FX's sandbox toggle, when it's a hosted plugin.
    pub fx_sandbox: Option<Rect>,
}

/// How a plugin stands with respect to running in its own process. `on` is what
/// the user asked for; `live` is what is actually happening right now — they
/// differ between asking and the next load, and `live` is also true for a
/// plugin the crash probe isolated on its own.
#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub struct SbxState {
    /// This is a hosted plugin, so the toggle means something.
    pub available: bool,
    pub on: bool,
    pub live: bool,
    /// Blocks the child missed — audible gaps — and crash restarts.
    pub missed: u64,
    pub restarts: u64,
}

/// Button text for a sandbox toggle: the state, and what it has cost.
pub fn sbx_label(s: SbxState) -> String {
    if !s.live {
        return if s.on { " SBX \u{25CF} (reload) ".into() } else { " SBX \u{25CB} ".into() };
    }
    let mut label = String::from(" SBX \u{25CF}");
    if s.missed > 0 {
        label.push_str(&format!(" {} lost", s.missed));
    }
    if s.restarts > 0 {
        label.push_str(&format!(" {}\u{21BB}", s.restarts));
    }
    label.push(' ');
    label
}

pub fn knob_indicator(val: f32) -> char {
    match (val.clamp(0.0, 1.0) * 7.99) as usize {
        0 => '\u{2199}', 1 => '\u{2190}', 2 => '\u{2196}', 3 => '\u{2191}',
        4 => '\u{2197}', 5 => '\u{2192}', 6 => '\u{2198}',
        _ => '\u{2193}',
    }
}

/// The knob's arc, at eight positions per cell instead of one.
///
/// A terminal cell is the coarsest unit there is, so an eight-cell bar could
/// only ever show eight positions — a filter cutoff moved by a hair looked
/// exactly the same. The eighth-block glyphs (`▏▎▍▌▋▊▉█`) split each cell into
/// eight, which is 64 positions in the same width and the closest a terminal
/// gets to the angular resolution of a real knob.
pub fn knob_arc(val: f32, width: usize) -> String {
    const EIGHTHS: [char; 8] = ['\u{258F}', '\u{258E}', '\u{258D}', '\u{258C}', '\u{258B}', '\u{258A}', '\u{2589}', '\u{2588}'];
    let eighths = (val.clamp(0.0, 1.0) * (width * 8) as f32).round() as usize;
    let full = eighths / 8;
    let rest = eighths % 8;
    let mut out = String::with_capacity(width * 3);
    for _ in 0..full.min(width) {
        out.push('\u{2588}');
    }
    if full < width && rest > 0 {
        out.push(EIGHTHS[rest - 1]);
    }
    let drawn = full.min(width) + usize::from(full < width && rest > 0);
    for _ in drawn..width {
        out.push('\u{2591}');
    }
    out
}

/// The cells of a **group**: three or more faders in a row sharing one unit.
///
/// That is an ADSR (four times), an EQ's band gains, a set of send levels — the
/// case where seeing the *profile* beats reading four numbers. The grouping
/// comes from the plugin too: the unit says these are the same kind of thing,
/// and the order is the plugin's own. No name is read, so a plugin that calls
/// its envelope A/D/S/R and one that calls it Attack/Decay/… group the same.
///
/// Returns one flag per parameter: draw it as a vertical bar, or as itself.
pub fn fader_groups(shapes: &[ParamShape]) -> Vec<bool> {
    const MIN_GROUP: usize = 3;
    let mut grouped = vec![false; shapes.len()];
    let mut i = 0;
    while i < shapes.len() {
        let ParamShape::Fader(unit) = &shapes[i] else {
            i += 1;
            continue;
        };
        let mut j = i + 1;
        while j < shapes.len() && matches!(&shapes[j], ParamShape::Fader(u) if u == unit) {
            j += 1;
        }
        if j - i >= MIN_GROUP {
            grouped[i..j].fill(true);
        }
        i = j;
    }
    grouped
}

/// One column of a vertical fader bank, two rows tall: `(top, bottom)`.
///
/// Two cells of eighth-blocks are sixteen levels, and the point is not the
/// number — it is that the bars next to each other draw the shape of the
/// envelope (or the curve of the EQ) in one look.
pub fn vertical_bar(val: f32, width: usize) -> (String, String) {
    const EIGHTHS: [char; 8] = ['\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}', '\u{2585}', '\u{2586}', '\u{2587}', '\u{2588}'];
    let eighths = (val.clamp(0.0, 1.0) * 16.0).round() as usize;
    let cell = |e: usize| -> char {
        match e {
            0 => ' ',
            n if n >= 8 => '\u{2588}',
            n => EIGHTHS[n - 1],
        }
    };
    // The bar grows upward: the bottom cell fills first.
    let bottom = cell(eighths.min(8));
    let top = cell(eighths.saturating_sub(8));
    (top.to_string().repeat(width), bottom.to_string().repeat(width))
}

/// A horizontal fader: the whole travel, with the handle where the value is.
///
/// The difference from the arc is what it says about the parameter — a mix or a
/// delay time is a distance covered, not a setting dialled in — so the track is
/// drawn end to end and only the handle moves.
pub fn fader_track(val: f32, width: usize) -> String {
    let width = width.max(2);
    let pos = (val.clamp(0.0, 1.0) * (width - 1) as f32).round() as usize;
    (0..width)
        .map(|i| if i == pos { '\u{25AE}' } else { '\u{2500}' })
        .collect()
}

/// Active slot's mixer strip: gain (linear), pan (-1..1), mute, solo.
pub type MixStrip = (f32, f32, bool, bool);

/// Max linear slot gain, mirrors `MAX_GAIN` in main.rs — only used to scale the
/// gain bar.
const MAX_GAIN: f32 = 2.0;

/// The `A→M` gate as a knob position and as a reading.
///
/// A gate is a level, so it is drawn in dB like every other level: the knob
/// spans -70 dBFS (anything plays) to -20 dBFS (only a hard note does), which
/// is the range a pickup actually lives in.
pub const GATE_MIN_DB: f32 = -70.0;
pub const GATE_MAX_DB: f32 = -20.0;

pub fn gate_norm(gate: f32) -> f32 {
    let db = if gate > 1e-6 { 20.0 * gate.log10() } else { GATE_MIN_DB };
    ((db - GATE_MIN_DB) / (GATE_MAX_DB - GATE_MIN_DB)).clamp(0.0, 1.0)
}

pub fn gate_from_norm(norm: f32) -> f32 {
    let db = GATE_MIN_DB + norm.clamp(0.0, 1.0) * (GATE_MAX_DB - GATE_MIN_DB);
    10f32.powf(db / 20.0)
}

fn gate_db(gate: f32) -> String {
    let db = if gate > 1e-6 { 20.0 * gate.log10() } else { GATE_MIN_DB };
    format!("{db:.0}")
}

/// Instrument-line button labels.
pub const BTN_SOURCE: &str = " SOURCE ";
pub const BTN_PRESET: &str = " BANK/PRESET ";
pub const BTN_LEARN: &str = " MIDI LEARN ";
/// Audio in → notes out. Only offered on a tab fed by a capture pair.
pub const BTN_A2M: &str = " A\u{2192}M ";

/// What `A→M` is hearing, short enough to sit inside its own button:
/// ` E2+14` — the note and how many cents off it is — or the input level in dB
/// when nothing is sounding, which is the number `SENS` is set against.
fn heard() -> String {
    let m = choz_engine::meter::pitch_meter();
    match m.note() {
        Some(note) => format!(" {}{:+}", note_name(note), m.cents()),
        None => {
            let level = m.level();
            let db = if level > 1e-6 { 20.0 * level.log10() } else { -99.0 };
            format!(" {db:.0}dB")
        }
    }
}

/// `60` is `C4`, the way every tracker and DAW writes it.
fn note_name(note: u8) -> String {
    const NAMES: [&str; 12] =
        ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    format!("{}{}", NAMES[note as usize % 12], note as i32 / 12 - 1)
}
pub const BTN_GUI: &str = " GUI ";
pub const BTN_PREV: &str = " \u{25C0} ";
pub const BTN_NEXT: &str = " \u{25B6} ";

pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max.saturating_sub(1)).chain(['\u{2026}']).collect()
    }
}

/// Pan position as a small slider, e.g. "L--|-o-R".
pub fn pan_slider(pan: f32) -> String {
    const W: usize = 7;
    let idx = (((pan.clamp(-1.0, 1.0) + 1.0) / 2.0) * (W - 1) as f32).round() as usize;
    let mut s: Vec<char> = "\u{2500}".repeat(W).chars().collect();
    s[W / 2] = '\u{253C}';
    s[idx] = '\u{25CF}';
    format!("L{}R", s.into_iter().collect::<String>())
}

/// Human-readable pan label: "C", "L64", "R30".
pub fn pan_label(pan: f32) -> String {
    let p = (pan.clamp(-1.0, 1.0) * 100.0).round() as i32;
    match p {
        0 => "C".to_string(),
        n if n < 0 => format!("L{}", -n),
        n => format!("R{n}"),
    }
}

/// How many knob columns fit in `width`, and how many rows `n` knobs need.
/// Exposed so the parameter cursor logic and the tests agree with the drawing.
/// How many rows of instrument knobs the RACK gives up before scrolling. The
/// FX chain needs the rest of the panel, and a plugin with a hundred
/// parameters would otherwise eat the whole thing.
const INSTR_KNOB_ROWS: usize = 2;

pub fn param_grid(width: u16, n: usize) -> (usize, usize) {
    let cols = (width.saturating_sub(2) / FX_CELL_W).max(1) as usize;
    let rows = n.div_ceil(cols);
    (cols, rows)
}

/// Draw a bordered box of knobs — one cell per parameter, wrapping onto as many
/// rows as fit and scrolling to keep the cursor visible.
///
/// Shared by the selected FX and by the instrument's own parameters, which is
/// the point: a plugin's knobs look and behave the same wherever they come
/// from, so a CC can be learned on any of them without opening a window.
///
/// Returns the click rects (parameter index → cell) and the first row below the
/// box.
#[allow(clippy::too_many_arguments)]
fn draw_knob_box(
    f: &mut Frame,
    inner: Rect,
    y: u16,
    title: &str,
    values: &[f32],
    names: &[String],
    // One shape per value, or empty for "all knobs" — which is what every
    // built-in FX is, and what a plugin that reports nothing gets.
    shapes: &[ParamShape],
    cursor: usize,
    focused: bool,
    max_rows: usize,
    reserve_below: u16,
) -> (Vec<(usize, Rect)>, u16) {
    let bg = super::theme::panel_style();
    let n = values.len();
    let mut rects = Vec::new();
    if n == 0 || y + 5 > inner.y + inner.height {
        return (rects, y);
    }
    let (cols, rows_needed) = param_grid(inner.width, n);
    // 3 rows per knob row, plus the box border.
    let room = (inner.y + inner.height).saturating_sub(y + reserve_below).max(3) as usize;
    let rows_shown = (room / 3).clamp(1, rows_needed.max(1)).min(max_rows.max(1));
    let cursor_row = cursor / cols.max(1);
    let first_row = cursor_row.saturating_sub(rows_shown.saturating_sub(1));

    let box_h = (rows_shown * 3) as u16 + 2;
    let box_rect = Rect::new(inner.x + 1, y, inner.width.saturating_sub(2), box_h.min(inner.height));
    let more = if rows_needed > rows_shown {
        format!(" ({}/{} rows) ", first_row + rows_shown, rows_needed)
    } else {
        String::new()
    };
    let block = Block::default()
        .title(format!(" {title}{more} "))
        .title_style(Style::default().fg(HEADER).add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if focused { SEL } else { RULE }))
        .style(bg);
    let param_inner = block.inner(box_rect);
    f.render_widget(block, box_rect);
    // Runs of same-unit faders read as one instrument (an ADSR, a set of band
    // gains), so they are drawn as a bank of vertical bars side by side.
    let grouped = fader_groups(shapes);

    for row in 0..rows_shown {
        let ry = param_inner.y + (row * 3) as u16;
        if ry + 2 > param_inner.y + param_inner.height {
            break;
        }
        let (mut knob_spans, mut val_spans, mut name_spans) = (Vec::new(), Vec::new(), Vec::new());
        for col in 0..cols {
            let pi = (first_row + row) * cols + col;
            if pi >= n {
                break;
            }
            let val = values[pi];
            let is_p = pi == cursor && focused;
            rects.push((pi, Rect::new(param_inner.x + (col as u16) * FX_CELL_W, ry, FX_CELL_W, 3)));
            let shape = shapes.get(pi).unwrap_or(&ParamShape::Continuous);
            let cell = FX_CELL_W as usize;
            // The control follows what the parameter is: a switch reads as on
            // or off, a named step reads as its name, and everything else is
            // the arc choz always drew.
            let (top, bottom, top_colour) = match (shape, shape.step_at(val)) {
                (ParamShape::Toggle, Some((k, _))) => {
                    let on = k == 1;
                    (
                        format!("[  {}  ]", if on { " ON" } else { "OFF" }),
                        String::new(),
                        if on { ON_COLOUR } else { OFF_COLOUR },
                    )
                }
                (ParamShape::Named(_), Some((k, n))) => (
                    format!("\u{25C0}{}\u{25B6}", truncate(shape.label(k).unwrap_or("?"), cell - 3)),
                    format!(" {}/{n}", k + 1),
                    KNOB,
                ),
                // In a group the bar is vertical, and the two rows above the
                // name are its height — the profile is what the eye reads.
                (ParamShape::Fader(_), _) if grouped.get(pi).copied().unwrap_or(false) => {
                    let (top, bottom) = vertical_bar(val, 5);
                    (format!(" {top}"), format!(" {bottom} {val:4.2}"), KNOB)
                }
                (ParamShape::Fader(_), _) => (
                    fader_track(val, 10),
                    format!(" {val:4.2}"),
                    KNOB,
                ),
                _ => (
                    format!("[{}]", knob_arc(val, 8)),
                    format!(" {}{val:4.2}", knob_indicator(val)),
                    KNOB,
                ),
            };
            knob_spans.push(Span::styled(
                format!("{top:<cell$}"),
                Style::default().fg(if is_p { SEL } else { top_colour }),
            ));
            val_spans.push(Span::styled(
                format!("{bottom:<cell$}"),
                Style::default()
                    .fg(if is_p { SEL } else { ui_text() })
                    .add_modifier(if is_p { Modifier::BOLD } else { Modifier::empty() }),
            ));
            let name = names.get(pi).map(|s| s.as_str()).unwrap_or("?");
            name_spans.push(Span::styled(
                format!(" {:<width$}", truncate(name, FX_CELL_W as usize - 2), width = FX_CELL_W as usize - 1),
                Style::default().fg(if is_p { SEL } else { LABEL }),
            ));
        }
        for (spans, yy) in [(knob_spans, ry), (val_spans, ry + 1), (name_spans, ry + 2)] {
            f.render_widget(
                Paragraph::new(Line::from(spans)).style(bg),
                Rect::new(param_inner.x, yy, param_inner.width, 1),
            );
        }
    }
    (rects, box_rect.y + box_rect.height)
}

/// Draw the RACK panel and return its click rects.
#[allow(clippy::too_many_arguments)]
pub fn draw_fx_chain_panel(
    f: &mut Frame,
    area: Rect,
    chain: &[AudioFxEntry],
    fx_slot: usize,
    fx_param: usize,
    focused: bool,
    tabs: &[String],
    active_slot: usize,
    mix: Option<MixStrip>,
    instrument: &str,
    // `preset` is the current bank:preset name, when the tab holds a SoundFont.
    preset: Option<&str>,
    // Whether the tab's instrument has a native window to open.
    has_gui: bool,
    // Same for the selected FX in the chain.
    fx_has_gui: bool,
    // Out-of-process state of the instrument and of the selected FX.
    sandbox: SbxState,
    fx_sandbox: SbxState,
    // The tab's MIDI channel (1..16), `None` in LIVE mode where it means
    // nothing: there, a tab is chosen by its input and by which one is active.
    channel: Option<u8>,
    // Audio-in state of the tab: `Some(on)` when it is fed by a capture pair —
    // the only case where turning its pitch into notes means anything.
    pitch_to_midi: Option<bool>,
    // The instrument's own parameters: name and 0..1 position, in plugin order.
    // Carla's "generic UI": every plugin gets knobs whether or not it has a
    // window, so a CC can be learned without opening one.
    instr_params: &[(String, f32, ParamShape)],
    instr_cursor: usize,
    // Which of the two knob boxes the arrows and the highlight belong to.
    instr_focused: bool,
    // `(trim, gate)` of the tab's audio input, or `None` when it plays its own
    // instrument and there is nothing coming in to trim.
    in_trim: Option<(f32, f32)>,
    // The AutoTune reading and its recent pitch error, when that is the FX the
    // cursor is on. `None` for every other effect.
    at_view: Option<(choz_engine::fx::autotune::AutoTuneMeter, &[f32])>,
) -> RackLayout {
    let has_presets = preset.is_some();
    let mut layout = RackLayout::default();
    let border_style = if focused {
        Style::default().fg(SEL).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(ui_border())
    };

    let block = Block::default()
        .title(format!(" {} ", if focused { format!("{} [ACTIVE]", t("RACK")) } else { t("RACK").to_string() }))
        .title_style(Style::default().fg(HEADER).add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_style(border_style)
        .style(super::theme::panel_style());

    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return layout;
    }

    let bg = super::theme::panel_style();
    let put = |f: &mut Frame, line: Line, y: u16| {
        if y < inner.y + inner.height {
            f.render_widget(Paragraph::new(line).style(bg), Rect::new(inner.x, y, inner.width, 1));
        }
    };
    let rule = |f: &mut Frame, label: &str, y: u16| {
        if y >= inner.y + inner.height {
            return;
        }
        let text = format!("\u{2500}\u{2500} {label} ");
        let pad = (inner.width as usize).saturating_sub(text.chars().count());
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(text, Style::default().fg(LABEL).add_modifier(Modifier::BOLD)),
                Span::styled("\u{2500}".repeat(pad), Style::default().fg(RULE)),
            ]))
            .style(bg),
            Rect::new(inner.x, y, inner.width, 1),
        );
    };

    let mut y = inner.y;

    // ── Tabs ───────────────────────────────────────────────────────────────
    let mut tab_line: Vec<Span> = vec![Span::raw(" ")];
    if tabs.is_empty() {
        tab_line.push(Span::styled(
            " (empty rack \u{2014} bind an input on the left) ",
            super::theme::panel_style().fg(Color::DarkGray),
        ));
    } else {
        let mut x = inner.x + 1;
        for (i, label) in tabs.iter().enumerate() {
            let w = label.chars().count() as u16 + 2;
            layout.tabs.push((i, Rect::new(x, y, w, 1)));
            layout.tab_close.push((i, Rect::new(x + w - TAB_CLOSE_W, y, TAB_CLOSE_W, 1)));
            let st = if i == active_slot {
                Style::default().fg(Color::Black).bg(HEADER).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Rgb(200, 205, 215)).bg(Color::Rgb(40, 46, 56))
            };
            tab_line.push(Span::styled(format!(" {label} "), st));
            tab_line.push(Span::raw(" "));
            x += w + 1;
        }
        // `+`: a second configuration for the same input as the active tab.
        if x + TAB_ADD_W <= inner.x + inner.width {
            layout.tab_add = Some(Rect::new(x, y, TAB_ADD_W, 1));
            tab_line.push(Span::styled(
                " + ",
                Style::default().fg(Color::Black).bg(Color::Rgb(90, 170, 110)).add_modifier(Modifier::BOLD),
            ));
        }
    }
    put(f, Line::from(tab_line), y);
    y += 1;

    if tabs.is_empty() {
        return layout;
    }

    // ── Mixer strip ────────────────────────────────────────────────────────
    let (gain, pan, mute, solo) = mix.unwrap_or((1.0, 0.0, false, false));
    let flag = |on: bool, on_bg: Color| {
        if on {
            Style::default().fg(Color::Black).bg(on_bg).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Rgb(110, 115, 125)).bg(Color::Rgb(32, 38, 47))
        }
    };
    let label_style = Style::default().fg(LABEL).add_modifier(Modifier::BOLD);
    // (text, style, which rect it feeds) — laid out left to right, so the rect
    // of each cell falls out of the widths instead of being re-derived.
    let cells: Vec<(String, Style, Option<usize>)> = vec![
        (format!("{} ", t("VOL")), label_style, None),
        (format!("[{}] {gain:4.2}  ", knob_arc(gain / MAX_GAIN, 8)), Style::default().fg(KNOB), Some(0)),
        (format!("{} ", t("PAN")), label_style, None),
        (format!("{} {:<4}  ", pan_slider(pan), pan_label(pan)), Style::default().fg(KNOB), Some(1)),
        (format!(" {} ", t("MUTE")), flag(mute, Color::Rgb(200, 80, 80)), Some(2)),
        (" ".to_string(), bg, None),
        (format!(" {} ", t("SOLO")), flag(solo, Color::Rgb(220, 190, 70)), Some(3)),
    ];
    // A guitar is nowhere near the level of a synth, so a tab fed by audio gets
    // its own trim — and the sensitivity `A→M` listens with, which is the same
    // knob from the player's side: how hard you have to hit it to make a note.
    let mut cells = cells;
    if let Some((in_gain, gate)) = in_trim {
        cells.push(("  ".to_string(), bg, None));
        cells.push((format!("{} ", t("IN")), label_style, None));
        cells.push((
            format!("[{}] {in_gain:4.2}  ", knob_arc(in_gain / MAX_GAIN, 8)),
            Style::default().fg(KNOB),
            Some(4),
        ));
        cells.push((format!("{} ", t("SENS")), label_style, None));
        cells.push((
            format!("[{}] {:>3}", knob_arc(gate_norm(gate), 8), gate_db(gate)),
            Style::default().fg(KNOB),
            Some(5),
        ));
    }
    let mut x = inner.x + 2;
    let mut mix_line: Vec<Span> = vec![Span::raw("  ")];
    for (text, style, target) in cells {
        let w = text.chars().count() as u16;
        let rect = Rect::new(x, y, w, 1);
        match target {
            Some(0) => layout.gain = Some(rect),
            Some(1) => layout.pan = Some(rect),
            Some(2) => layout.mute = Some(rect),
            Some(3) => layout.solo = Some(rect),
            Some(4) => layout.in_gain = Some(rect),
            Some(5) => layout.in_gate = Some(rect),
            _ => {}
        }
        mix_line.push(Span::styled(text, style));
        x += w;
    }
    put(f, Line::from(mix_line), y);
    y += 1;

    // ── Instrument line ────────────────────────────────────────────────────
    let btn_style = Style::default().fg(Color::Black).bg(KNOB).add_modifier(Modifier::BOLD);
    // A plugin actually running elsewhere gets its own colour: it is the one
    // thing on this line that says the audio is crossing a process boundary.
    let sbx_style = Style::default()
        .fg(Color::Black)
        .bg(Color::Rgb(56, 200, 100))
        .add_modifier(Modifier::BOLD);
    let mut instr_line: Vec<Span> = vec![
        Span::styled(format!("  {} ", t("INSTR")), Style::default().fg(LABEL).add_modifier(Modifier::BOLD)),
        Span::styled(format!("{:<18}", truncate(instrument, 18)), Style::default().fg(ui_text())),
    ];
    let mut bx = inner.x + 2 + 8 + 18;
    for (btn, text) in [
        (RackButton::Source, Some(BTN_SOURCE.to_string())),
        // Bank/preset only exists while the tab holds a SoundFont.
        (RackButton::Preset, has_presets.then(|| BTN_PRESET.to_string())),
        (RackButton::Learn, Some(BTN_LEARN.to_string())),
        // A guitar into a synth: only offered where there is audio coming in.
        (
            RackButton::PitchToMidi,
            pitch_to_midi.map(|on| {
                // With it on, the button says what the tracker is hearing —
                // the note and how far off it is. Without that, "nothing
                // happens" and "the wrong note" look the same and `SENS` has
                // nothing to aim at.
                if on {
                    format!("{}\u{25CF}{} ", BTN_A2M.trim_end(), heard())
                } else {
                    format!("{}\u{25CB} ", BTN_A2M.trim_end())
                }
            }),
        ),
        // In MULTI the channel is what decides whether this tab sounds at all,
        // so it sits on the same line as the instrument it selects.
        // Channel 0 is "any": a tab that takes whatever its port sends.
        (
            RackButton::Channel,
            channel.map(|c| if c == 0 { " CH ANY ".to_string() } else { format!(" CH {c:>2} ") }),
        ),
        // Only plugins with a native editor get the button.
        (RackButton::Gui, has_gui.then(|| BTN_GUI.to_string())),
        // Only a hosted plugin can be moved into a process of its own.
        (RackButton::Sandbox, sandbox.available.then(|| sbx_label(sandbox))),
    ] {
        let Some(text) = text else { continue };
        let w = text.chars().count() as u16;
        layout.buttons.push((btn, Rect::new(bx, y, w, 1)));
        let style = if btn == RackButton::Sandbox && sandbox.live {
            sbx_style
        } else if btn == RackButton::PitchToMidi && pitch_to_midi == Some(true) {
            // On, and it changes what the tab does with its input, so it says so
            // the way the sandbox button does.
            Style::default().fg(Color::Black).bg(ON_COLOUR).add_modifier(Modifier::BOLD)
        } else {
            btn_style
        };
        instr_line.push(Span::styled(text, style));
        instr_line.push(Span::raw(" "));
        bx += w + 1;
    }
    put(f, Line::from(instr_line), y);
    y += 1;

    // ── Bank / preset line (SoundFont tabs only) ───────────────────────────
    if let Some(name) = preset {
        let mut px = inner.x + 2 + 8;
        let mut line: Vec<Span> = vec![
            Span::styled(format!("  {}  ", t("BANK")), Style::default().fg(LABEL).add_modifier(Modifier::BOLD)),
        ];
        for (btn, text) in [(RackButton::PresetPrev, BTN_PREV), (RackButton::PresetNext, BTN_NEXT)] {
            let w = text.chars().count() as u16;
            layout.buttons.push((btn, Rect::new(px, y, w, 1)));
            line.push(Span::styled(text, btn_style));
            line.push(Span::raw(" "));
            px += w + 1;
            if btn == RackButton::PresetPrev {
                // The name sits between the two arrows.
                let w = 30u16;
                line.push(Span::styled(
                    format!("{:<30}", truncate(name, 30)),
                    Style::default().fg(ui_text()),
                ));
                line.push(Span::raw(" "));
                px += w + 1;
            }
        }
        put(f, Line::from(line), y);
        y += 1;
    }

    // ── Instrument parameters ──────────────────────────────────────────────
    if !instr_params.is_empty() {
        let values: Vec<f32> = instr_params.iter().map(|(_, v, _)| *v).collect();
        let names: Vec<String> = instr_params.iter().map(|(n, _, _)| n.clone()).collect();
        let shapes: Vec<ParamShape> = instr_params.iter().map(|(_, _, s)| s.clone()).collect();
        let (rects, next) = draw_knob_box(
            f,
            inner,
            y,
            // The title carries the key that hands it the arrows: two knob
            // boxes on one panel need to say which one is live.
            &format!(
                "{} \u{00B7} {}{}",
                t("INSTRUMENT"),
                truncate(instrument, 18),
                if focused && instr_focused { "" } else { "  [k]" }
            ),
            &values,
            &names,
            &shapes,
            instr_cursor,
            focused && instr_focused,
            INSTR_KNOB_ROWS,
            // Leave the FX chain its rule, its buttons and a knob row.
            9,
        );
        layout.instr_knobs = rects;
        y = next;
    }

    // ── FX chain ───────────────────────────────────────────────────────────
    rule(f, t("FX CHAIN"), y);
    y += 1;

    // Chain buttons wrap onto further lines instead of running off the panel.
    let mut cx = inner.x + 2;
    let right = inner.x + inner.width;
    let mut chain_line: Vec<Span> = vec![Span::raw("  ")];
    let flush = |f: &mut Frame, line: &mut Vec<Span>, y: u16| {
        put(f, Line::from(std::mem::replace(line, vec![Span::raw("  ")])), y);
    };
    for (i, entry) in chain.iter().enumerate() {
        let text = format!(" {}:{} ", i + 1, entry.label());
        let w = text.chars().count() as u16;
        if cx + w > right && cx > inner.x + 2 {
            flush(f, &mut chain_line, y);
            y += 1;
            cx = inner.x + 2;
        }
        let st = if i == fx_slot && focused {
            Style::default().fg(Color::Black).bg(SEL).add_modifier(Modifier::BOLD)
        } else if entry.enabled {
            Style::default().fg(Color::Rgb(56, 200, 100)).bg(Color::Rgb(30, 40, 34))
        } else {
            Style::default()
                .fg(Color::Rgb(90, 95, 105))
                .bg(Color::Rgb(30, 34, 40))
                .add_modifier(Modifier::CROSSED_OUT)
        };
        layout.fx_slots.push((i, Rect::new(cx, y, w, 1)));
        chain_line.push(Span::styled(text, st));
        chain_line.push(Span::raw(" "));
        cx += w + 1;
    }
    if chain.len() < MAX_FX {
        let add = " + ADD ";
        let w = add.chars().count() as u16;
        if cx + w > right && cx > inner.x + 2 {
            flush(f, &mut chain_line, y);
            y += 1;
            cx = inner.x + 2;
        }
        layout.fx_add = Some(Rect::new(cx, y, w, 1));
        chain_line.push(Span::styled(
            add,
            Style::default().fg(Color::Black).bg(Color::Rgb(100, 160, 220)),
        ));
    }
    flush(f, &mut chain_line, y);
    y += 1;

    let Some(entry) = chain.get(fx_slot) else {
        if chain.is_empty() {
            put(
                f,
                Line::from(Span::styled(
                    "   no FX yet \u{2014} press 'a' or click [+ ADD]",
                    Style::default().fg(Color::DarkGray),
                )),
                y,
            );
        }
        return layout;
    };

    // ── Selected FX: the same knob box, from the same helper ──────────────
    let descs = entry.param_descs();
    let names: Vec<String> = descs.iter().map(|d| d.name.to_string()).collect();
    let shapes: Vec<ParamShape> = descs.iter().map(|d| d.shape.clone()).collect();

    // A graphic EQ is ten sliders, and ten arcs cannot be read as a curve. It
    // gets tanu's drawing — a column per band, the zero line through the middle
    // — and the knobs that are *not* bands (preamp, preset, wet) follow below.
    let mut drawn = false;
    let eq_bands = (entry.plugin.is_none() && entry.kind == crate::source::AudioFxKind::GraphicEq)
        .then_some(choz_engine::fx::EQ_BANDS)
        .filter(|n| entry.params.len() > *n);
    if let Some(n) = eq_bands {
        let labels: Vec<&str> = names[..n].iter().map(|s| s.as_str()).collect();
        let title = format!("{}:{}", fx_slot + 1, entry.label());
        let (band_rects, after) = draw_eq_bank(
            f,
            inner,
            y,
            &entry.params[..n],
            &labels,
            fx_param.min(n - 1),
            focused && !instr_focused && fx_param < n,
            &title,
            bg,
        );
        if !band_rects.is_empty() {
            layout.params = band_rects;
            y = after;
            let (rest, next) = draw_knob_box(
                f,
                inner,
                y,
                "",
                &entry.params[n..],
                &names[n..],
                &shapes[n..],
                fx_param.saturating_sub(n),
                focused && !instr_focused && fx_param >= n,
                usize::MAX,
                5,
            );
            // The tail box numbers its knobs from zero; the chain does not.
            layout.params.extend(rest.into_iter().map(|(i, r)| (i + n, r)));
            y = next;
            drawn = true;
        }
    }

    let (rects, next) = if drawn {
        (Vec::new(), y)
    } else {
        draw_knob_box(
        f,
        inner,
        y,
        &format!("{}:{}", fx_slot + 1, entry.label()),
        &entry.params,
        &names,
        &shapes,
        fx_param,
        focused && !instr_focused,
        usize::MAX,
        5,
        )
    };
    if !drawn {
        layout.params = rects;
    }
    y = next;

    // ── What AutoTune is hearing ──────────────────────────────────────────
    //
    // The knobs already show every parameter, so what is added here is the one
    // thing they cannot: the live reading. Two rows, because a pitch corrector
    // that cannot be seen working can only be trusted or not.
    if let Some((m, trace)) = at_view {
        y = draw_autotune_readout(f, inner, y, m, trace, bg);
    }

    // ── Slot controls, in their own box, one blank line below the knobs ────
    if y + 2 < inner.y + inner.height {
        y += 1;
        let ctrl_h = 3u16;
        let ctrl_rect = Rect::new(inner.x + 1, y, inner.width.saturating_sub(2), ctrl_h);
        let ctrl_block = Block::default()
            .title(format!(" {} ", t("SLOT")))
            .title_style(Style::default().fg(LABEL))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(RULE))
            .style(bg);
        let ctrl_inner = ctrl_block.inner(ctrl_rect);
        f.render_widget(ctrl_block, ctrl_rect);

        let mut cx = ctrl_inner.x + 1;
        let mut spans: Vec<Span> = vec![Span::raw(" ")];
        let mut button = |spans: &mut Vec<Span<'static>>, text: String, style: Style, rect: &mut Option<Rect>| {
            let w = text.chars().count() as u16;
            *rect = Some(Rect::new(cx, ctrl_inner.y, w, 1));
            spans.push(Span::styled(text, style));
            // Buttons are spaced out so they don't read as one bar.
            spans.push(Span::raw("   "));
            cx += w + 3;
        };
        let (on_lbl, on_style) = if entry.enabled {
            ("  ON  ", Style::default().fg(Color::Black).bg(Color::Rgb(56, 200, 100)))
        } else {
            (" OFF  ", Style::default().fg(Color::Rgb(180, 185, 195)).bg(Color::Rgb(40, 46, 56)))
        };
        button(&mut spans, on_lbl.into(), on_style.add_modifier(Modifier::BOLD), &mut layout.on_off);
        let mv = Style::default().fg(Color::Black).bg(Color::Rgb(150, 195, 245));
        let mv_off = Style::default().fg(Color::Rgb(70, 78, 92)).bg(Color::Rgb(32, 38, 47));
        // Disabled ends of the chain still draw a (greyed) button, they just
        // don't get a click rect.
        let can_left = fx_slot > 0;
        let can_right = fx_slot + 1 < chain.len();
        let mut left = None;
        let mut right = None;
        button(&mut spans, " \u{25C0} MOVE ".into(), if can_left { mv } else { mv_off }, &mut left);
        button(&mut spans, " MOVE \u{25B6} ".into(), if can_right { mv } else { mv_off }, &mut right);
        layout.move_left = can_left.then_some(left).flatten();
        layout.move_right = can_right.then_some(right).flatten();
        button(&mut spans, " DEL ".into(),
               Style::default().fg(Color::White).bg(Color::Rgb(170, 50, 50)).add_modifier(Modifier::BOLD),
               &mut layout.del);
        // Plugin FX get their own window button; built-ins have nothing to show.
        if fx_has_gui {
            button(&mut spans, BTN_GUI.into(),
                   Style::default().fg(Color::Black).bg(KNOB).add_modifier(Modifier::BOLD),
                   &mut layout.fx_gui);
        }
        // Same toggle as the instrument's, for a plugin effect.
        if fx_sandbox.available {
            let style = if fx_sandbox.live {
                Style::default().fg(Color::Black).bg(Color::Rgb(56, 200, 100))
            } else {
                Style::default().fg(Color::Black).bg(KNOB)
            };
            button(&mut spans, sbx_label(fx_sandbox), style.add_modifier(Modifier::BOLD),
                   &mut layout.fx_sandbox);
        }
        f.render_widget(
            Paragraph::new(Line::from(spans)).style(bg),
            Rect::new(ctrl_inner.x, ctrl_inner.y, ctrl_inner.width, 1),
        );
        y = ctrl_rect.y + ctrl_rect.height;
    }

    // ── Hint, last line of the panel ───────────────────────────────────────
    let hint_y = (inner.y + inner.height).saturating_sub(1).max(y);
    let hint = if focused {
        "  1=source 2=bank/preset 3=learn 4=plugin window p=instr x/X=sandbox \u{00B7} a=add d=del \u{2190}\u{2192}=FX \u{2191}\u{2193}=param wheel=value \u{00B7} -/+=vol ,/.=pan m=mute S=solo"
    } else {
        "  Tab=enter the rack"
    };
    put(f, Line::from(Span::styled(truncate(hint, inner.width as usize), Style::default().fg(ui_border()))), hint_y);

    layout
}

/// The AutoTune strip: level, the note heard, the note aimed at, the error, and
/// where that error has been.
///
/// Everything here is read from a lock-free meter the audio thread publishes —
/// no work crosses back the other way, which is why a graph of the pitch costs
/// the callback nothing.
fn draw_autotune_readout(
    f: &mut Frame,
    inner: Rect,
    mut y: u16,
    m: choz_engine::fx::autotune::AutoTuneMeter,
    trace: &[f32],
    bg: Style,
) -> u16 {
    if y + 2 >= inner.y + inner.height || inner.width < 30 {
        return y;
    }
    let label = Style::default().fg(LABEL);
    let value = Style::default().fg(KNOB);
    let dim = Style::default().fg(RULE);

    // Row 1: level, the note heard → the note aimed at, and the error.
    let db = if m.level > 1e-6 { 20.0 * m.level.log10() } else { -99.0 };
    let bars = (((db + 60.0) / 60.0).clamp(0.0, 1.0) * 8.0).round() as usize;
    let meter: String = std::iter::repeat_n('\u{2588}', bars)
        .chain(std::iter::repeat_n('\u{2591}', 8 - bars))
        .collect();
    let heard = note_label(m.detected_frequency);
    let aimed = note_label(m.target_frequency);
    let err = if m.voiced { format!("{:+.0}\u{00A2}", m.pitch_error_cents) } else { "  \u{00B7} ".into() };
    let err_style = if !m.voiced {
        dim
    } else if m.pitch_error_cents.abs() < 10.0 {
        Style::default().fg(IN_TUNE)
    } else {
        Style::default().fg(KNOB)
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("  IN ", label),
            Span::styled(meter, value),
            Span::styled(format!(" {db:>5.1}dB   "), dim),
            Span::styled(format!("{heard:>10}"), value),
            Span::styled(" \u{2192} ", dim),
            Span::styled(format!("{aimed:<10}"), Style::default().fg(HEADER)),
            Span::styled(err, err_style),
        ]))
        .style(bg),
        Rect::new(inner.x, y, inner.width, 1),
    );
    y += 1;

    // Row 2: where the error has been. The centre line is in tune; the trace is
    // ±50 cents around it, which is the range a listener calls "out".
    let cols = (inner.width as usize).saturating_sub(10).min(trace.len());
    if cols > 4 {
        let spans: Vec<Span> = trace[trace.len() - cols..]
            .iter()
            .map(|&c| {
                if !c.is_finite() {
                    return Span::styled("\u{00B7}", dim);
                }
                let n = (c / 50.0).clamp(-1.0, 1.0);
                let (ch, st) = match n {
                    n if n > 0.35 => ('\u{2594}', Style::default().fg(KNOB)),
                    n if n > 0.08 => ('\u{2500}', Style::default().fg(KNOB)),
                    n if n < -0.35 => ('\u{2581}', Style::default().fg(KNOB)),
                    n if n < -0.08 => ('\u{2582}', Style::default().fg(KNOB)),
                    _ => ('\u{2500}', Style::default().fg(IN_TUNE)),
                };
                Span::styled(ch.to_string(), st)
            })
            .collect();
        let mut line = vec![Span::styled("  0\u{00A2} ", dim)];
        line.extend(spans);
        f.render_widget(
            Paragraph::new(Line::from(line)).style(bg),
            Rect::new(inner.x, y, inner.width, 1),
        );
        y += 1;
    }
    y
}

/// `233.4 Hz` as `A#3 233`, or a dash when there is nothing to name.
fn note_label(hz: f32) -> String {
    if !hz.is_finite() || hz <= 0.0 {
        return "\u{2014}".to_string();
    }
    let note = (69.0 + 12.0 * (hz / 440.0).log2()).round().clamp(0.0, 127.0) as i32;
    let name = choz_engine::fx::autotune::NOTE_NAMES[(note as usize) % 12];
    format!("{name}{} {hz:.0}", note / 12 - 1)
}

/// The graphic EQ as tanu draws it: a column per band, a knob on the track, and
/// the zero line straight through the middle.
///
/// A row of arcs says what each band is set to; this says what the **curve**
/// is, which is the only question anyone asks an EQ. Ten arcs cannot be read as
/// a shape, and a shape is the whole reason the control is ten sliders and not
/// one number.
///
/// Returns a click rect per band, so a click lands on the band under the mouse.
#[allow(clippy::too_many_arguments)]
pub fn draw_eq_bank(
    f: &mut Frame,
    inner: Rect,
    y: u16,
    values: &[f32],
    labels: &[&str],
    cursor: usize,
    focused: bool,
    title: &str,
    bg: Style,
) -> (Vec<(usize, Rect)>, u16) {
    let bands = values.len().min(labels.len());
    let mut rects = Vec::new();
    // Six rows of track plus the labels and the frame: under that there is no
    // curve to see and the knob grid is the better drawing.
    let height = 10u16.min(inner.height.saturating_sub(y - inner.y));
    if bands == 0 || height < 7 || inner.width < bands as u16 * 4 {
        return (rects, y);
    }
    let rect = Rect::new(inner.x + 1, y, inner.width.saturating_sub(2), height);
    let block = Block::default()
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(if focused { SEL } else { LABEL }).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if focused { SEL } else { RULE }))
        .style(bg);
    let area = block.inner(rect);
    f.render_widget(block, rect);

    let w = area.width as usize;
    let track_h = (area.height - 1) as usize;
    let centre = track_h / 2;
    // Where the knob sits: 1.0 is the top of the track, 0.5 the zero line.
    let knob_row = |v: f32| -> usize {
        (((1.0 - v.clamp(0.0, 1.0)) * (track_h - 1) as f32).round() as usize).min(track_h - 1)
    };
    let band_mid = |b: usize| (b * w / bands + (b + 1) * w / bands) / 2;

    for b in 0..bands {
        let x0 = area.x + (b * w / bands) as u16;
        let x1 = area.x + ((b + 1) * w / bands) as u16;
        rects.push((b, Rect::new(x0, area.y, x1.saturating_sub(x0).max(1), area.height)));
    }

    for row in 0..track_h {
        let spans: Vec<Span> = (0..w)
            .map(|col| {
                let b = (col * bands / w).min(bands - 1);
                let mid = band_mid(b);
                let sel = b == cursor && focused;
                let kr = knob_row(values[b]);
                let (ch, colour) = if col == mid && row == kr {
                    // Above the zero line is a boost, below it is a cut, and
                    // the colour says which without reading the number.
                    ('\u{2588}', if values[b] >= 0.5 { IN_TUNE } else { Color::Rgb(230, 120, 120) })
                } else if row == centre {
                    ('\u{2500}', RULE)
                } else if col == mid {
                    ('\u{2502}', RULE)
                } else {
                    (' ', RULE)
                };
                let style = if sel && ch != ' ' && ch != '\u{2588}' {
                    Style::default().fg(SEL)
                } else {
                    Style::default().fg(colour)
                };
                Span::styled(ch.to_string(), style)
            })
            .collect();
        f.render_widget(
            Paragraph::new(Line::from(spans)).style(bg),
            Rect::new(area.x, area.y + row as u16, area.width, 1),
        );
    }

    // Band labels, centred under their own column.
    let mut label_line = vec![Span::raw(" ".repeat(0))];
    let mut at = 0usize;
    for (b, label) in labels.iter().enumerate().take(bands) {
        let mid = band_mid(b);
        let text = truncate(label, 4);
        let start = mid.saturating_sub(text.chars().count() / 2);
        if start > at {
            label_line.push(Span::raw(" ".repeat(start - at)));
            at = start;
        }
        let style = if b == cursor && focused {
            Style::default().fg(SEL).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(LABEL)
        };
        at += text.chars().count();
        label_line.push(Span::styled(text.to_string(), style));
    }
    f.render_widget(
        Paragraph::new(Line::from(label_line)).style(bg),
        Rect::new(area.x, area.y + track_h as u16, area.width, 1),
    );
    (rects, rect.y + rect.height)
}

#[cfg(test)]
mod tests {
    /// The `A→M` button says what it is hearing, because "nothing happens" and
    /// "the wrong note" look identical otherwise — and `SENS` is set against
    /// exactly this reading.
    #[test]
    fn the_a_to_m_button_reports_what_it_heard() {
        assert_eq!(note_name(60), "C4", "middle C is C4, the way a DAW writes it");
        assert_eq!(note_name(69), "A4");
        assert_eq!(note_name(40), "E2", "a guitar's low E");

        let m = choz_engine::meter::pitch_meter();
        // Nothing sounding: the reading is the input level, which is the number
        // the sensitivity is set against.
        m.publish(None, 0, 0.01);
        assert!(heard().contains("-40dB"), "{}", heard());

        // Sounding, and 40 cents flat of it.
        m.publish(Some(40), -40, 0.2);
        let text = heard();
        assert!(text.contains("E2"), "{text}");
        assert!(text.contains("-40"), "and how far off it is: {text}");
        m.clear();
    }

    use super::*;

    #[test]
    fn params_wrap_onto_more_rows_when_they_dont_fit() {
        // 80 columns of panel → 6 knob columns of 13.
        assert_eq!(param_grid(80, 6), (6, 1));
        assert_eq!(param_grid(80, 7), (6, 2), "a 7th knob starts a second row");
        assert_eq!(param_grid(80, 16), (6, 3), "Z5 Texture's 16 params take 3 rows");
        assert_eq!(param_grid(10, 4), (1, 4), "a narrow panel stacks them");
        assert_eq!(param_grid(80, 0), (6, 0));
    }
}
