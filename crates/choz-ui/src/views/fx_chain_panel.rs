//! RACK panel — tabs, mixer strip, instrument line and the insert FX chain.
//!
//! The panel computes its own click rects while it draws and hands them back in
//! a [`RackLayout`]: there is exactly one place that decides where a control
//! sits, so the hit test can't drift from the pixels the way hand-mirrored
//! offsets used to.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::i18n::t;
use crate::source::{AudioFxEntry, ParamShape, MAX_FX};
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
    /// How much of a converting tab's output is the instrument and how much is
    /// the audio that drove it.
    PitchMix,
    /// Open (or close) the plugin's own window.
    Gui,
    /// Ask for (or stop asking for) this plugin to run in its own process.
    Sandbox,
    /// Previous / next program of the loaded SoundFont.
    PresetPrev,
    PresetNext,
    /// Previous / next page of the instrument's own parameters. A synth like
    /// Surge XT has hundreds; the box shows a few rows of them, and these are
    /// how the rest are reached.
    InstrPagePrev,
    InstrPageNext,
    /// The tab's arpeggiator. `ArpOn` is the only one drawn while it is off —
    /// a box of settings for something switched off is six rows of nothing in a
    /// panel that is already tight.
    ArpOn,
    ArpMode,
    ArpDiv,
    ArpRateDown,
    ArpRateUp,
    ArpGate,
    ArpOctaves,
    ArpLatch,
    ArpSwing,
    /// Follow choz's transport instead of the arpeggiator's own tempo.
    ArpSync,
    /// One key plays the memorised chord.
    ArpChord,
    ArpTap,
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
    /// The factory-preset picker, for a built-in that ships any.
    pub fx_preset: Option<Rect>,
    /// (knob index, rect) of the arpeggiator's own box, when it is drawn as
    /// one. Empty on a screen too short for it, where the controls are buttons.
    pub arp_knobs: Vec<(usize, Rect)>,
}

/// What the SLOT box says about the selected effect beyond its buttons: what is
/// going through it, what it delays, and whether it has presets to offer.
#[derive(Default, Clone, Copy)]
pub struct FxSlotInfo {
    /// Peak in and out of the last block, linear. `None` for a slot that is not
    /// running — an empty chain, or a rack with no engine behind it.
    pub peaks: Option<(f32, f32)>,
    /// What the whole chain delays the signal by, in milliseconds. Shown on the
    /// chain, not on the effect: it is the number the player feels.
    pub latency_ms: f32,
    /// This effect ships factory presets.
    pub presets: bool,
}

/// A linear peak as the dB the meter shows. Anything under -60 is "nothing",
/// which is shorter to read than `-inf` and means the same to a player.
fn db_label(peak: f32) -> String {
    if peak <= 0.001 {
        return "  -\u{221E}".into();
    }
    format!("{:5.1}", 20.0 * peak.log10())
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
        return if s.on {
            " SBX \u{25CF} (reload) ".into()
        } else {
            " SBX \u{25CB} ".into()
        };
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
        0 => '\u{2199}',
        1 => '\u{2190}',
        2 => '\u{2196}',
        3 => '\u{2191}',
        4 => '\u{2197}',
        5 => '\u{2192}',
        6 => '\u{2198}',
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
    const EIGHTHS: [char; 8] = [
        '\u{258F}', '\u{258E}', '\u{258D}', '\u{258C}', '\u{258B}', '\u{258A}', '\u{2589}',
        '\u{2588}',
    ];
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
    const EIGHTHS: [char; 8] = [
        '\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}', '\u{2585}', '\u{2586}', '\u{2587}',
        '\u{2588}',
    ];
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
    (
        top.to_string().repeat(width),
        bottom.to_string().repeat(width),
    )
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

/// Max linear **input** trim, mirrors `MAX_IN_GAIN` in main.rs. An input is
/// coming off whatever the preamp was set to, so it gets far more range than
/// a slot's output does.
const MAX_IN_GAIN: f32 = 16.0;

/// Max linear slot gain, mirrors `MAX_GAIN` in main.rs — only used to scale the
/// gain bar.
pub const MAX_GAIN: f32 = 2.0;

/// The `A→M` gate as a knob position and as a reading.
///
/// A gate is a level, so it is drawn in dB like every other level: the knob
/// spans -70 dBFS (anything plays) to -20 dBFS (only a hard note does), which
/// is the range a pickup actually lives in.
pub const GATE_MIN_DB: f32 = -70.0;
pub const GATE_MAX_DB: f32 = -20.0;

pub fn gate_norm(gate: f32) -> f32 {
    let db = if gate > 1e-6 {
        20.0 * gate.log10()
    } else {
        GATE_MIN_DB
    };
    ((db - GATE_MIN_DB) / (GATE_MAX_DB - GATE_MIN_DB)).clamp(0.0, 1.0)
}

pub fn gate_from_norm(norm: f32) -> f32 {
    let db = GATE_MIN_DB + norm.clamp(0.0, 1.0) * (GATE_MAX_DB - GATE_MIN_DB);
    10f32.powf(db / 20.0)
}

fn gate_db(gate: f32) -> String {
    let db = if gate > 1e-6 {
        20.0 * gate.log10()
    } else {
        GATE_MIN_DB
    };
    format!("{db:.0}")
}

/// Instrument-line button labels.
///
/// Keys, not finished text: all three have translations that were never
/// reached because the buttons drew the constant directly. `btn()` puts them
/// back through `t()` and re-adds the padding the layout expects.
pub const BTN_SOURCE: &str = "SOURCE";
pub const BTN_PRESET: &str = "BANK/PRESET";
pub const BTN_LEARN: &str = "MIDI LEARN";

/// A button label, translated and padded to sit inside its box.
pub fn btn(key: &str) -> String {
    format!(" {} ", crate::i18n::t(key))
}
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
            let db = if level > 1e-6 {
                20.0 * level.log10()
            } else {
                -99.0
            };
            format!(" {db:.0}dB")
        }
    }
}

/// `60` is `C4`, the way every tracker and DAW writes it.
fn note_name(note: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!("{}{}", NAMES[note as usize % 12], note as i32 / 12 - 1)
}
pub const BTN_GUI: &str = " GUI ";
/// How many cells a run of spans takes on screen — the only honest way to know
/// where the next button lands once any of the text before it is translated.
fn line_width(spans: &[Span]) -> u16 {
    spans.iter().map(|s| s.width() as u16).sum()
}

pub const BTN_PREV: &str = " \u{25C0} ";
pub const BTN_NEXT: &str = " \u{25B6} ";

/// A row of buttons that **wraps** onto the next line when the panel runs out
/// of columns, handing back the rect of everything it draws.
///
/// The arpeggiator's row is what forced this: with the sequencer on, its
/// switches ran past 120 columns and the last ones were simply not there — and
/// the RACK cannot afford to answer that with another bordered box, which is
/// the same reason the row collapses to a single switch when it is off.
///
/// Each button is rendered where it is measured, so a translated label moves
/// the ones after it instead of leaving their click rects behind. That bug has
/// been fixed twice on hand-computed offsets; this is the shape that cannot
/// have it.
struct ButtonRow {
    x: u16,
    y: u16,
    inner: Rect,
    bg: Style,
    /// Columns the wrapped lines start at, so a continuation reads as one.
    indent: u16,
}

impl ButtonRow {
    fn new(inner: Rect, bg: Style, y: u16, indent: u16) -> Self {
        Self {
            x: inner.x + indent,
            y,
            inner,
            bg,
            indent,
        }
    }

    /// Draw `text`, wrapping first if it does not fit, and leave `gap` columns
    /// after it. Returns where it landed.
    fn draw(&mut self, f: &mut Frame, text: String, style: Style, gap: u16) -> Rect {
        let w = Span::raw(text.as_str()).width() as u16;
        let right = self.inner.x + self.inner.width;
        if self.x + w > right && self.x > self.inner.x + self.indent {
            self.y += 1;
            self.x = self.inner.x + self.indent;
        }
        // Wrapping cannot save a label that is wider than the whole panel, and
        // ratatui's answer to a rect that leaves the buffer is a panic — the
        // application gone because somebody made the window narrow. What fits
        // is drawn, and the rect handed back is the part that is really there,
        // so the mouse and the drawing keep agreeing.
        let visible = w.min(right.saturating_sub(self.x));
        let rect = Rect::new(self.x, self.y, visible, 1);
        if self.y < self.inner.y + self.inner.height && visible > 0 {
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(text, style))).style(self.bg),
                rect,
            );
        }
        self.x += w + gap;
        rect
    }

    /// Text that takes room but answers no clicks.
    fn label(&mut self, f: &mut Frame, text: String, style: Style) {
        self.draw(f, text, style, 0);
    }

    /// A button, spaced from the next so they don't read as one bar.
    fn button(&mut self, f: &mut Frame, text: String, style: Style) -> Rect {
        self.draw(f, text, style, 1)
    }

    /// The first line below the row.
    fn finish(self) -> u16 {
        self.y + 1
    }
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars()
            .take(max.saturating_sub(1))
            .chain(['\u{2026}'])
            .collect()
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

/// Rows of arpeggiator knobs the RACK gives up: two is every control it has at
/// any usable width, and a third would come out of the FX chain.
const ARP_KNOB_ROWS: usize = 2;

/// What one row of knobs costs: the arc, the value and the name.
const ARP_KNOBS_ROWS: u16 = 3;

/// The row the TAP button sits on. It is never a knob — tapping a tempo is a
/// gesture, and a gesture has no position to be turned to.
const ARP_TAP_ROW: u16 = 1;

/// Which of the three shapes the arpeggiator's controls take, decided by the
/// rows the panel has left rather than by a setting: the same controls either
/// way, and nothing to get wrong when a window is resized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArpShape {
    /// Not running: one switch, and the rest of the panel keeps its rows.
    Off,
    /// Knobs in a bordered, titled box, like the FX and the instrument get.
    Boxed,
    /// The same knobs with no frame — two rows cheaper, which is the difference
    /// between having them and not on a five-inch screen.
    Strip,
    /// No room for knobs at all: the row of buttons, wrapping.
    Buttons,
}

/// Rows the FX chain cannot do without: its title rule, the row of units and a
/// knob box for the selected one. The SLOT box below them is already drawn only
/// where it fits, so it is not counted here — this is the floor, not the wish.
///
/// The arpeggiator only takes a knob shape when this much is left after it,
/// which is what keeps a five-inch screen showing an FX chain at all.
const FX_CHAIN_ROWS: u16 = 7;

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
    // A border costs two rows and buys a title. On a panel that has the rows it
    // is worth it; on a five-inch screen those two rows are the difference
    // between knobs and no knobs, so the box loses its frame instead of the
    // user losing the controls.
    bordered: bool,
) -> (Vec<(usize, Rect)>, u16) {
    let bg = super::theme::panel_style();
    let n = values.len();
    let mut rects = Vec::new();
    let frame = if bordered { 2 } else { 0 };
    if n == 0 || y + 3 + frame > inner.y + inner.height {
        return (rects, y);
    }
    let (cols, rows_needed) = param_grid(inner.width, n);
    // 3 rows per knob row, plus the box border.
    let room = (inner.y + inner.height)
        .saturating_sub(y + reserve_below)
        .max(3) as usize;
    let rows_shown = (room / 3).clamp(1, rows_needed.max(1)).min(max_rows.max(1));
    let cursor_row = cursor / cols.max(1);
    let first_row = cursor_row.saturating_sub(rows_shown.saturating_sub(1));

    let box_h = (rows_shown * 3) as u16 + frame;
    let box_rect = Rect::new(
        inner.x + 1,
        y,
        inner.width.saturating_sub(2),
        box_h.min(inner.height),
    );
    let more = if rows_needed > rows_shown {
        format!(" ({}/{} rows) ", first_row + rows_shown, rows_needed)
    } else {
        String::new()
    };
    let param_inner = if bordered {
        let block = Block::default()
            .title(format!(" {title}{more} "))
            .title_style(Style::default().fg(HEADER).add_modifier(Modifier::BOLD))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(if focused { SEL } else { RULE }))
            .style(bg);
        let param_inner = block.inner(box_rect);
        f.render_widget(block, box_rect);
        param_inner
    } else {
        box_rect
    };
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
            rects.push((
                pi,
                Rect::new(param_inner.x + (col as u16) * FX_CELL_W, ry, FX_CELL_W, 3),
            ));
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
                    format!(
                        "\u{25C0}{}\u{25B6}",
                        truncate(shape.label(k).unwrap_or("?"), cell - 3)
                    ),
                    format!(" {}/{n}", k + 1),
                    KNOB,
                ),
                // In a group the bar is vertical, and the two rows above the
                // name are its height — the profile is what the eye reads.
                (ParamShape::Fader(_), _) if grouped.get(pi).copied().unwrap_or(false) => {
                    let (top, bottom) = vertical_bar(val, 5);
                    (format!(" {top}"), format!(" {bottom} {val:4.2}"), KNOB)
                }
                (ParamShape::Fader(_), _) => (fader_track(val, 10), format!(" {val:4.2}"), KNOB),
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
                    .add_modifier(if is_p {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ));
            let name = names.get(pi).map(|s| s.as_str()).unwrap_or("?");
            name_spans.push(Span::styled(
                format!(
                    " {:<width$}",
                    truncate(name, FX_CELL_W as usize - 2),
                    width = FX_CELL_W as usize - 1
                ),
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
    // How much of a converting tab is the instrument: 1 = only the instrument.
    pitch_mix: f32,
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
    // The tab's arpeggiator: settings plus where its sequencer is. Drawn as one
    // line while it is off, two when it is on.
    arp: crate::arp::ArpView<'_>,
    // Which input algorithm the tab runs, and the knobs of the ALGO box — the
    // picker first, then whatever the running algorithm owns. Both come from
    // the interface rather than being worked out again here: a box whose knobs
    // are not the knobs being edited moves the wrong control.
    // Where this tab's notes are pointed, when they are pointed anywhere, and
    // over how much of that parameter's range.
    // Meter, latency and presets of the selected FX — everything the SLOT box
    // knows that is not a button.
    fx_info: FxSlotInfo,
) -> RackLayout {
    let has_presets = preset.is_some();
    let mut layout = RackLayout::default();
    let border_style = if focused {
        Style::default().fg(SEL).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(ui_border())
    };

    let block = Block::default()
        .title(format!(
            " {} ",
            if focused {
                format!("{} [ACTIVE]", t("RACK"))
            } else {
                t("RACK").to_string()
            }
        ))
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
            f.render_widget(
                Paragraph::new(line).style(bg),
                Rect::new(inner.x, y, inner.width, 1),
            );
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
                Span::styled(
                    text,
                    Style::default().fg(LABEL).add_modifier(Modifier::BOLD),
                ),
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
            layout
                .tab_close
                .push((i, Rect::new(x + w - TAB_CLOSE_W, y, TAB_CLOSE_W, 1)));
            let st = if i == active_slot {
                Style::default()
                    .fg(Color::Black)
                    .bg(HEADER)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(Color::Rgb(200, 205, 215))
                    .bg(Color::Rgb(40, 46, 56))
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
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Rgb(90, 170, 110))
                    .add_modifier(Modifier::BOLD),
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
            Style::default()
                .fg(Color::Black)
                .bg(on_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(Color::Rgb(110, 115, 125))
                .bg(Color::Rgb(32, 38, 47))
        }
    };
    let label_style = Style::default().fg(LABEL).add_modifier(Modifier::BOLD);
    // (text, style, which rect it feeds) — laid out left to right, so the rect
    // of each cell falls out of the widths instead of being re-derived.
    let cells: Vec<(String, Style, Option<usize>)> = vec![
        (format!("{} ", t("VOL")), label_style, None),
        (
            format!("[{}] {gain:4.2}  ", knob_arc(gain / MAX_GAIN, 8)),
            Style::default().fg(KNOB),
            Some(0),
        ),
        (format!("{} ", t("PAN")), label_style, None),
        (
            format!("{} {:<4}  ", pan_slider(pan), pan_label(pan)),
            Style::default().fg(KNOB),
            Some(1),
        ),
        (
            format!(" {} ", t("MUTE")),
            flag(mute, Color::Rgb(200, 80, 80)),
            Some(2),
        ),
        (" ".to_string(), bg, None),
        (
            format!(" {} ", t("SOLO")),
            flag(solo, Color::Rgb(220, 190, 70)),
            Some(3),
        ),
    ];
    // A guitar is nowhere near the level of a synth, so a tab fed by audio gets
    // its own trim — and the sensitivity `A→M` listens with, which is the same
    // knob from the player's side: how hard you have to hit it to make a note.
    let mut cells = cells;
    if let Some((in_gain, gate)) = in_trim {
        cells.push(("  ".to_string(), bg, None));
        cells.push((format!("{} ", t("IN")), label_style, None));
        cells.push((
            // In dB, not as a bare multiplier: "×8.30" is not a number anyone
            // sets a microphone by, and the useful part of a 24 dB range is
            // all bunched up at the bottom of a linear reading.
            //
            // `CLIP` is the reading that matters more than the number: a trim
            // past full scale saturates what comes out **and** hands the pitch
            // detector a square wave, and neither of those says which knob did
            // it. Turn it down until this goes away.
            format!(
                "[{}] {:>+3.0}dB{}  ",
                knob_arc(in_gain / MAX_IN_GAIN, 8),
                if in_gain > 1e-4 {
                    20.0 * in_gain.log10()
                } else {
                    -60.0
                },
                if choz_engine::meter::capture_health().clipping() > 0 {
                    " CLIP"
                } else {
                    ""
                }
            ),
            if choz_engine::meter::capture_health().clipping() > 0 {
                Style::default()
                    .fg(super::theme::WARN)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(KNOB)
            },
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
    let btn_style = Style::default()
        .fg(Color::Black)
        .bg(KNOB)
        .add_modifier(Modifier::BOLD);
    // A plugin actually running elsewhere gets its own colour: it is the one
    // thing on this line that says the audio is crossing a process boundary.
    let sbx_style = Style::default()
        .fg(Color::Black)
        .bg(Color::Rgb(56, 200, 100))
        .add_modifier(Modifier::BOLD);
    // The row wraps: it already carries eight buttons on a tab with audio in, a
    // window and a sandbox, and the label of the learn one grows with the
    // parameter it points at.
    let mut instr_row = ButtonRow::new(inner, bg, y, 2);
    instr_row.label(
        f,
        format!("{} ", t("INSTR")),
        Style::default().fg(LABEL).add_modifier(Modifier::BOLD),
    );
    instr_row.label(
        f,
        format!("{:<18} ", truncate(instrument, 18)),
        Style::default().fg(ui_text()),
    );
    for (btn, text) in [
        (RackButton::Source, Some(btn(BTN_SOURCE))),
        // Bank/preset only exists while the tab holds a SoundFont.
        (RackButton::Preset, has_presets.then(|| btn(BTN_PRESET))),
        (RackButton::Learn, Some(btn(BTN_LEARN))),
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
        // How much of the input comes back with the instrument. Only worth a
        // control once the converter is on — off, there is nothing to mix.
        (
            RackButton::PitchMix,
            (pitch_to_midi == Some(true)).then(|| {
                format!(
                    " WET {}{:>3.0}% ",
                    knob_arc(pitch_mix, 6),
                    pitch_mix * 100.0
                )
            }),
        ),
        // In MULTI the channel is what decides whether this tab sounds at all,
        // so it sits on the same line as the instrument it selects.
        // Channel 0 is "any": a tab that takes whatever its port sends.
        (
            RackButton::Channel,
            channel.map(|c| {
                if c == 0 {
                    " CH ANY ".to_string()
                } else {
                    format!(" CH {c:>2} ")
                }
            }),
        ),
        // Only plugins with a native editor get the button.
        (RackButton::Gui, has_gui.then(|| BTN_GUI.to_string())),
        // Only a hosted plugin can be moved into a process of its own.
        (
            RackButton::Sandbox,
            sandbox.available.then(|| sbx_label(sandbox)),
        ),
    ] {
        let Some(text) = text else { continue };
        let style = if btn == RackButton::Sandbox && sandbox.live {
            sbx_style
        } else if btn == RackButton::PitchToMidi && pitch_to_midi == Some(true) {
            // On, and it changes what the tab does with its input, so it says so
            // the way the sandbox button does.
            Style::default()
                .fg(Color::Black)
                .bg(ON_COLOUR)
                .add_modifier(Modifier::BOLD)
        } else {
            btn_style
        };
        let rect = instr_row.button(f, text, style);
        layout.buttons.push((btn, rect));
    }
    y = instr_row.finish();

    // ── Bank / preset line (SoundFont tabs only) ───────────────────────────
    if let Some(name) = preset {
        let line_start: Vec<Span> = vec![Span::styled(
            format!("  {}  ", t("BANK")),
            Style::default().fg(LABEL).add_modifier(Modifier::BOLD),
        )];
        // Same rule as the instrument line: the arrows are where the label
        // leaves them, not where an English label would leave them.
        let mut px = inner.x + line_width(&line_start);
        let mut line = line_start;
        for (btn, text) in [
            (RackButton::PresetPrev, BTN_PREV),
            (RackButton::PresetNext, BTN_NEXT),
        ] {
            let w = Span::raw(text).width() as u16;
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

    // ── Arpeggiator ────────────────────────────────────────────────────────
    //
    // Off, it is a single switch: a bordered box for something most tabs never
    // turn on would cost the RACK rows it does not have. On, it is the same
    // knob box the FX and the instrument get — BPM, GATE and SWING are values,
    // and a value belongs on the arc this program draws every other value with.
    //
    // **Unless the screen is too small for it.** On a 5-inch panel the rack has
    // a handful of rows, and five of them spent on a bordered box would leave
    // no FX chain — so there the controls stay a row of buttons, wrapping. Same
    // controls either way; only the shape changes.
    let arp_boxed;
    {
        let s = arp.settings;
        // Which control the arrows are on, as buttons rather than as a knob:
        // on a panel too short for the box the row **is** the box, and a
        // cursor nobody can see is a cursor nobody can use.
        let knobs = arp.knobs();
        let cursor_btns: &[RackButton] = if arp.focused && focused {
            match knobs.get(arp.cursor).map(|(p, ..)| *p) {
                Some(crate::arp::ArpParam::On) => &[RackButton::ArpOn],
                Some(crate::arp::ArpParam::Sync) => &[RackButton::ArpSync],
                Some(crate::arp::ArpParam::Mode) => &[RackButton::ArpMode],
                Some(crate::arp::ArpParam::Div) => &[RackButton::ArpDiv],
                // The tempo is two buttons, and both are the same control.
                Some(crate::arp::ArpParam::Bpm) => {
                    &[RackButton::ArpRateDown, RackButton::ArpRateUp]
                }
                Some(crate::arp::ArpParam::Gate) => &[RackButton::ArpGate],
                Some(crate::arp::ArpParam::Swing) => &[RackButton::ArpSwing],
                Some(crate::arp::ArpParam::Octaves) => &[RackButton::ArpOctaves],
                Some(crate::arp::ArpParam::Latch) => &[RackButton::ArpLatch],
                Some(crate::arp::ArpParam::Chord) => &[RackButton::ArpChord],
                None => &[],
            }
        } else {
            &[]
        };
        let button = |row: &mut ButtonRow,
                      f: &mut Frame,
                      layout: &mut RackLayout,
                      btn: RackButton,
                      text: String,
                      on: bool| {
            let style = if cursor_btns.contains(&btn) {
                Style::default()
                    .fg(Color::Black)
                    .bg(SEL)
                    .add_modifier(Modifier::BOLD)
            } else if on {
                Style::default()
                    .fg(Color::Black)
                    .bg(ON_COLOUR)
                    .add_modifier(Modifier::BOLD)
            } else {
                btn_style
            };
            let rect = row.button(f, text, style);
            layout.buttons.push((btn, rect));
        };

        // What shape the controls take is decided by the rows left, and only by
        // them: a bordered box where the panel is tall, the same knobs without
        // their frame where two rows matter (a five-inch screen gives the rack
        // about eleven), and the row of buttons where even that would leave no
        // FX chain.
        let room = (inner.y + inner.height).saturating_sub(y);
        let shape = if !s.on {
            ArpShape::Off
        } else if room >= ARP_KNOBS_ROWS + 2 + ARP_TAP_ROW + FX_CHAIN_ROWS {
            ArpShape::Boxed
        } else if room >= ARP_KNOBS_ROWS + ARP_TAP_ROW + FX_CHAIN_ROWS {
            ArpShape::Strip
        } else {
            ArpShape::Buttons
        };
        let boxed = matches!(shape, ArpShape::Boxed | ArpShape::Strip);
        arp_boxed = boxed;

        // The switch is a knob inside the box when there is one — the header
        // row it used to live on is a row, and a row is what is scarce. TAP is
        // never a knob: tapping a tempo is a gesture, not a position.
        let mut row = ButtonRow::new(inner, bg, y, 2);
        if !boxed {
            row.label(
                f,
                format!("{}   ", t("ARP")),
                Style::default().fg(LABEL).add_modifier(Modifier::BOLD),
            );
            button(
                &mut row,
                f,
                &mut layout,
                RackButton::ArpOn,
                format!(
                    " {} {} ",
                    t("ARP"),
                    if s.on { "\u{25CF}" } else { "\u{25CB}" }
                ),
                s.on,
            );
        }
        // TAP only rides the button row in the shapes that *are* a button row.
        // With a box, it goes on the box's own top edge — a gesture that
        // belongs to the arpeggiator should not be floating above it.
        if s.on && !boxed {
            button(
                &mut row,
                f,
                &mut layout,
                RackButton::ArpTap,
                format!(" TAP {:>3.0} ", s.tempo()),
                false,
            );
        }
        if matches!(shape, ArpShape::Buttons) {
            // Every control a button, on the row that wraps: the shape for a
            // panel with no room for knobs.
            button(
                &mut row,
                f,
                &mut layout,
                RackButton::ArpMode,
                format!(" {} ", s.mode.label()),
                false,
            );
            button(
                &mut row,
                f,
                &mut layout,
                RackButton::ArpDiv,
                format!(" {} ", s.div.label()),
                false,
            );
            button(
                &mut row,
                f,
                &mut layout,
                RackButton::ArpRateDown,
                BTN_PREV.into(),
                false,
            );
            row.label(
                f,
                // The tempo it is actually counting at, which is the
                // transport's while SYNC is on.
                format!("{:>3.0} BPM ", s.tempo()),
                Style::default().fg(ui_text()),
            );
            button(
                &mut row,
                f,
                &mut layout,
                RackButton::ArpRateUp,
                BTN_NEXT.into(),
                false,
            );
            button(
                &mut row,
                f,
                &mut layout,
                RackButton::ArpSync,
                format!(" SYNC {} ", if s.sync { "\u{25CF}" } else { "\u{25CB}" }),
                s.sync,
            );
            button(
                &mut row,
                f,
                &mut layout,
                RackButton::ArpGate,
                format!(" GATE {:>3.0}% ", s.gate * 100.0),
                false,
            );
            button(
                &mut row,
                f,
                &mut layout,
                RackButton::ArpSwing,
                format!(" SWING {:>2.0}% ", s.swing * 100.0),
                s.swing > 0.0,
            );
            button(
                &mut row,
                f,
                &mut layout,
                RackButton::ArpOctaves,
                format!(" OCT {} ", s.octaves),
                false,
            );
            button(
                &mut row,
                f,
                &mut layout,
                RackButton::ArpLatch,
                format!(" HOLD {} ", if s.latch { "\u{25CF}" } else { "\u{25CB}" }),
                s.latch,
            );
            button(
                &mut row,
                f,
                &mut layout,
                RackButton::ArpChord,
                // On, it says how many notes one key will play: a chord mode
                // with nothing memorised looks identical to one that works
                // until you press a key.
                if s.chord {
                    format!(" CHORD \u{25CF}{} ", arp.chord.len())
                } else {
                    " CHORD \u{25CB} ".to_string()
                },
                s.chord,
            );
        }
        y = row.finish();

        if boxed {
            let box_top = y;
            let values: Vec<f32> = knobs.iter().map(|(_, _, v, _)| *v).collect();
            let names: Vec<String> = knobs.iter().map(|(_, n, _, _)| n.to_string()).collect();
            let shapes: Vec<ParamShape> = knobs.iter().map(|(_, _, _, s)| s.clone()).collect();
            let (rects, next) = draw_knob_box(
                f,
                inner,
                y,
                // Same convention as the other two boxes: the title says which
                // key hands it the arrows.
                &format!("{} [k]", t("ARP")),
                &values,
                &names,
                &shapes,
                arp.cursor.min(values.len().saturating_sub(1)),
                focused && arp.focused,
                ARP_KNOB_ROWS,
                FX_CHAIN_ROWS,
                matches!(shape, ArpShape::Boxed),
            );
            layout.arp_knobs = rects;
            // On the box's own top edge, right-aligned: the same place the
            // SLOT box carries its meter, and the reason this is not a knob is
            // that tapping a tempo is a gesture, not a position.
            if matches!(shape, ArpShape::Boxed) {
                let label = format!(" TAP {:>3.0} ", s.tempo());
                let w = label.chars().count() as u16;
                let right = inner.x + inner.width.saturating_sub(1);
                if right > inner.x + w + 2 {
                    let rect = Rect::new(right.saturating_sub(w + 1), box_top, w, 1);
                    let style = if cursor_btns.contains(&RackButton::ArpTap) {
                        Style::default()
                            .fg(Color::Black)
                            .bg(SEL)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        btn_style
                    };
                    f.render_widget(Paragraph::new(Span::styled(label, style)), rect);
                    layout.buttons.push((RackButton::ArpTap, rect));
                }
            }
            y = next;
        }
    }

    // Whichever box has the arrows is the one drawn live: with three boxes on
    // the panel, "not the instrument's" stopped being the same as "the FX's".
    let fx_focused = focused && !instr_focused && !(arp_boxed && arp.focused);

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
                if focused && instr_focused {
                    ""
                } else {
                    "  [k]"
                }
            ),
            &values,
            &names,
            &shapes,
            instr_cursor,
            focused && instr_focused,
            INSTR_KNOB_ROWS,
            // Leave the FX chain its rule, its buttons and a knob row.
            9,
            true,
        );
        // More parameters than fit: page through them. The arrows sit on the
        // box's own top edge, right-aligned, where the arpeggiator's TAP sits —
        // and they are learn targets like any other button, because a synth
        // whose knobs are on page 4 is no use to someone with both hands busy.
        if rects.len() < instr_params.len() {
            let (px, pn) = (BTN_PREV, BTN_NEXT);
            let w = (Span::raw(px).width() + Span::raw(pn).width() + 1) as u16;
            let right = inner.x + inner.width.saturating_sub(1);
            if right > inner.x + w + 2 {
                let mut x = right.saturating_sub(w + 1);
                for (btn, text) in [
                    (RackButton::InstrPagePrev, px),
                    (RackButton::InstrPageNext, pn),
                ] {
                    let bw = Span::raw(text).width() as u16;
                    let rect = Rect::new(x, y, bw, 1);
                    f.render_widget(Paragraph::new(Span::styled(text, btn_style)), rect);
                    layout.buttons.push((btn, rect));
                    x += bw;
                }
            }
        }
        layout.instr_knobs = rects;
        y = next;
    }

    // ── FX chain ───────────────────────────────────────────────────────────
    rule(f, t("FX CHAIN"), y);
    y += 1;

    // Chain buttons wrap onto further lines instead of running off the panel —
    // the same row as the arpeggiator's, from the same helper.
    let mut row = ButtonRow::new(inner, bg, y, 2);
    for (i, entry) in chain.iter().enumerate() {
        let st = if i == fx_slot && focused {
            Style::default()
                .fg(Color::Black)
                .bg(SEL)
                .add_modifier(Modifier::BOLD)
        } else if entry.enabled {
            Style::default()
                .fg(Color::Rgb(56, 200, 100))
                .bg(Color::Rgb(30, 40, 34))
        } else {
            Style::default()
                .fg(Color::Rgb(90, 95, 105))
                .bg(Color::Rgb(30, 34, 40))
                .add_modifier(Modifier::CROSSED_OUT)
        };
        let rect = row.button(f, format!(" {}:{} ", i + 1, entry.label()), st);
        layout.fx_slots.push((i, rect));
    }
    if chain.len() < MAX_FX {
        layout.fx_add = Some(
            row.button(
                f,
                " + ADD ".to_string(),
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Rgb(100, 160, 220)),
            ),
        );
    }
    y = row.finish();

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
    //
    // The waveshaper's eight points are the same drawing for the same reason:
    // a transfer curve is a shape, and eight arcs are eight numbers. The bank
    // already carries its own click rects and its own cursor, so drawing the
    // curve *is* editing it — there is no editor to write.
    let mut drawn = false;
    let bank_labels: Vec<String>;
    let eq_bands = match entry.kind {
        _ if entry.plugin.is_some() => None,
        crate::source::AudioFxKind::GraphicEq => Some(choz_engine::fx::EQ_BANDS),
        crate::source::AudioFxKind::WaveShaper => Some(choz_engine::fx::saturator::TABLE_POINTS),
        _ => None,
    }
    .filter(|n| entry.params.len() > *n);
    if let Some(n) = eq_bands {
        // The waveshaper's columns are input levels, not band names: "P3" says
        // nothing, "-.43" says where on the curve the point sits.
        let labels: Vec<&str> = if entry.kind == crate::source::AudioFxKind::WaveShaper {
            bank_labels = (0..n)
                .map(|i| {
                    let x = choz_engine::fx::saturator::Table::input_at(i);
                    format!("{x:+.1}")
                })
                .collect();
            bank_labels.iter().map(|s| s.as_str()).collect()
        } else {
            names[..n].iter().map(|s| s.as_str()).collect()
        };
        let title = format!("{}:{}", fx_slot + 1, entry.label());
        let (band_rects, after) = draw_eq_bank(
            f,
            inner,
            y,
            &entry.params[..n],
            &labels,
            fx_param.min(n - 1),
            fx_focused && fx_param < n,
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
                fx_focused && fx_param >= n,
                usize::MAX,
                5,
                true,
            );
            // The tail box numbers its knobs from zero; the chain does not.
            layout
                .params
                .extend(rest.into_iter().map(|(i, r)| (i + n, r)));
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
            fx_focused,
            usize::MAX,
            5,
            true,
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

    // ── What the parametric EQ is doing to the signal ─────────────────────
    // Drawn at 48 kHz whatever the device runs at: bilinear warping only moves
    // the curve near Nyquist, and the panel does not get a knob's worth of
    // plumbing for a difference nobody can see at this many pixels per octave.
    if entry.plugin.is_none() && entry.kind == crate::source::AudioFxKind::ParamEq {
        y = draw_eq_curve(f, inner, y, &entry.params, 48_000, bg);
    }

    // ── Slot controls, in their own box, one blank line below the knobs ────
    if y + 2 < inner.y + inner.height {
        y += 1;
        let ctrl_h = 3u16;
        let ctrl_rect = Rect::new(inner.x + 1, y, inner.width.saturating_sub(2), ctrl_h);
        // The title carries the meter: the box is one row of buttons, and a
        // second row for two numbers would cost a line of knobs.
        let mut title = format!(" {} ", t("SLOT"));
        if let Some((pin, pout)) = fx_info.peaks {
            title.push_str(&format!(
                "\u{00B7} IN {} OUT {} dB ",
                db_label(pin),
                db_label(pout)
            ));
        }
        if fx_info.latency_ms >= 0.05 {
            title.push_str(&format!("\u{00B7} LAT {:.1} ms ", fx_info.latency_ms));
        }
        let ctrl_block = Block::default()
            .title(title)
            .title_style(Style::default().fg(LABEL))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(RULE))
            .style(bg);
        let ctrl_inner = ctrl_block.inner(ctrl_rect);
        f.render_widget(ctrl_block, ctrl_rect);

        let mut cx = ctrl_inner.x + 1;
        let mut spans: Vec<Span> = vec![Span::raw(" ")];
        let mut button = |spans: &mut Vec<Span<'static>>,
                          text: String,
                          style: Style,
                          rect: &mut Option<Rect>| {
            let w = text.chars().count() as u16;
            *rect = Some(Rect::new(cx, ctrl_inner.y, w, 1));
            spans.push(Span::styled(text, style));
            // Buttons are spaced out so they don't read as one bar.
            spans.push(Span::raw("   "));
            cx += w + 3;
        };
        let (on_lbl, on_style) = if entry.enabled {
            (
                "  ON  ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Rgb(56, 200, 100)),
            )
        } else {
            (
                " OFF  ",
                Style::default()
                    .fg(Color::Rgb(180, 185, 195))
                    .bg(Color::Rgb(40, 46, 56)),
            )
        };
        button(
            &mut spans,
            on_lbl.into(),
            on_style.add_modifier(Modifier::BOLD),
            &mut layout.on_off,
        );
        let mv = Style::default()
            .fg(Color::Black)
            .bg(Color::Rgb(150, 195, 245));
        let mv_off = Style::default()
            .fg(Color::Rgb(70, 78, 92))
            .bg(Color::Rgb(32, 38, 47));
        // Disabled ends of the chain still draw a (greyed) button, they just
        // don't get a click rect.
        let can_left = fx_slot > 0;
        let can_right = fx_slot + 1 < chain.len();
        let mut left = None;
        let mut right = None;
        button(
            &mut spans,
            " \u{25C0} MOVE ".into(),
            if can_left { mv } else { mv_off },
            &mut left,
        );
        button(
            &mut spans,
            " MOVE \u{25B6} ".into(),
            if can_right { mv } else { mv_off },
            &mut right,
        );
        layout.move_left = can_left.then_some(left).flatten();
        layout.move_right = can_right.then_some(right).flatten();
        button(
            &mut spans,
            " DEL ".into(),
            Style::default()
                .fg(Color::White)
                .bg(Color::Rgb(170, 50, 50))
                .add_modifier(Modifier::BOLD),
            &mut layout.del,
        );
        // Factory presets, for the built-ins that ship them.
        if fx_info.presets {
            button(
                &mut spans,
                format!(" {} ", t("PRESET")),
                Style::default()
                    .fg(Color::Black)
                    .bg(KNOB)
                    .add_modifier(Modifier::BOLD),
                &mut layout.fx_preset,
            );
        }
        // Plugin FX get their own window button; built-ins have nothing to show.
        if fx_has_gui {
            button(
                &mut spans,
                BTN_GUI.into(),
                Style::default()
                    .fg(Color::Black)
                    .bg(KNOB)
                    .add_modifier(Modifier::BOLD),
                &mut layout.fx_gui,
            );
        }
        // Same toggle as the instrument's, for a plugin effect.
        if fx_sandbox.available {
            let style = if fx_sandbox.live {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Rgb(56, 200, 100))
            } else {
                Style::default().fg(Color::Black).bg(KNOB)
            };
            button(
                &mut spans,
                sbx_label(fx_sandbox),
                style.add_modifier(Modifier::BOLD),
                &mut layout.fx_sandbox,
            );
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
        "  1=source 2=bank/preset 3=learn 4=plugin window k=box p=instr P=fx preset x/X=sandbox \u{00B7} a=add d=del \u{2190}\u{2192}=FX \u{2191}\u{2193}=param wheel=value \u{00B7} -/+=vol ,/.=pan m=mute S=solo"
    } else {
        "  Tab=enter the rack"
    };
    put(
        f,
        Line::from(Span::styled(
            truncate(hint, inner.width as usize),
            Style::default().fg(ui_border()),
        )),
        hint_y,
    );

    layout
}

/// The AutoTune strip: level, the note heard, the note aimed at, the error, and
/// where that error has been.
///
/// Everything here is read from a lock-free meter the audio thread publishes —
/// no work crosses back the other way, which is why a graph of the pitch costs
/// the callback nothing.
/// The parametric EQ's response, 20 Hz–20 kHz on a log axis, ±18 dB.
///
/// Four knobs and a Q do not read as a shape; the curve does. It is computed
/// from the same coefficients the processor runs, through
/// `ParametricEq::from_params`, so it cannot claim a curve that is not there.
/// Drawn only when there are rows to spare — the knobs are what has to fit.
fn draw_eq_curve(
    f: &mut Frame,
    inner: Rect,
    y: u16,
    params: &[f32],
    sample_rate: u32,
    bg: Style,
) -> u16 {
    let rows = inner.y + inner.height - y.min(inner.y + inner.height);
    let height = 6u16;
    if rows < height || inner.width < 24 {
        return y;
    }
    let eq = choz_engine::fx::ParametricEq::from_params(params, sample_rate);
    let area = Rect::new(inner.x + 1, y, inner.width.saturating_sub(2), height);
    let w = area.width as usize;
    let track_h = (height - 1) as usize;
    let centre = track_h / 2;
    // ±18 dB across the box: the range the knobs can actually reach.
    let row_of = |db: f32| -> usize {
        let t = (1.0 - (db / 18.0).clamp(-1.0, 1.0)) * 0.5;
        ((t * (track_h - 1) as f32).round() as usize).min(track_h - 1)
    };
    // Log frequency: an octave is the same distance everywhere.
    let freq_of = |col: usize| 20.0f32 * 1000.0f32.powf(col as f32 / (w.max(2) - 1) as f32);

    let curve: Vec<usize> = (0..w)
        .map(|c| row_of(eq.response_db(freq_of(c), sample_rate)))
        .collect();

    for row in 0..track_h {
        let spans: Vec<Span> = (0..w)
            .map(|col| {
                if curve[col] == row {
                    let db = eq.response_db(freq_of(col), sample_rate);
                    let colour = if db > 0.5 {
                        IN_TUNE
                    } else if db < -0.5 {
                        Color::Rgb(230, 120, 120)
                    } else {
                        KNOB
                    };
                    Span::styled("\u{2501}".to_string(), Style::default().fg(colour))
                } else if row == centre {
                    Span::styled("\u{2500}".to_string(), Style::default().fg(RULE))
                } else {
                    Span::raw(" ")
                }
            })
            .collect();
        f.render_widget(
            Paragraph::new(Line::from(spans)).style(bg),
            Rect::new(area.x, area.y + row as u16, area.width, 1),
        );
    }

    // Decade marks, so the axis is readable without a legend.
    let mut axis: Vec<Span> = Vec::new();
    let mut at = 0usize;
    for (hz, text) in [(100.0f32, "100"), (1000.0, "1k"), (10000.0, "10k")] {
        let col = ((hz / 20.0).log(1000.0) * (w.max(2) - 1) as f32).round() as usize;
        let start = col
            .saturating_sub(text.len() / 2)
            .min(w.saturating_sub(text.len()));
        if start < at {
            continue;
        }
        axis.push(Span::raw(" ".repeat(start - at)));
        axis.push(Span::styled(text, Style::default().fg(LABEL)));
        at = start + text.len();
    }
    f.render_widget(
        Paragraph::new(Line::from(axis)).style(bg),
        Rect::new(area.x, area.y + track_h as u16, area.width, 1),
    );
    y + height
}

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
    let db = if m.level > 1e-6 {
        20.0 * m.level.log10()
    } else {
        -99.0
    };
    let bars = (((db + 60.0) / 60.0).clamp(0.0, 1.0) * 8.0).round() as usize;
    let meter: String = std::iter::repeat_n('\u{2588}', bars)
        .chain(std::iter::repeat_n('\u{2591}', 8 - bars))
        .collect();
    let heard = note_label(m.detected_frequency);
    let aimed = note_label(m.target_frequency);
    let err = if m.voiced {
        format!("{:+.0}\u{00A2}", m.pitch_error_cents)
    } else {
        "  \u{00B7} ".into()
    };
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
    let note = (69.0 + 12.0 * (hz / 440.0).log2())
        .round()
        .clamp(0.0, 127.0) as i32;
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
            Style::default()
                .fg(if focused { SEL } else { LABEL })
                .add_modifier(Modifier::BOLD),
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
        rects.push((
            b,
            Rect::new(x0, area.y, x1.saturating_sub(x0).max(1), area.height),
        ));
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
                    (
                        '\u{2588}',
                        if values[b] >= 0.5 {
                            IN_TUNE
                        } else {
                            Color::Rgb(230, 120, 120)
                        },
                    )
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
        assert_eq!(
            note_name(60),
            "C4",
            "middle C is C4, the way a DAW writes it"
        );
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

    /// The curve has to *move* with the knobs, not decorate the panel: a boost
    /// draws above the zero line where the band is and nowhere else.
    #[test]
    fn the_eq_curve_follows_the_knobs() {
        use ratatui::{backend::TestBackend, Terminal};
        let render = |params: &[f32]| -> Vec<String> {
            let mut term = Terminal::new(TestBackend::new(60, 8)).unwrap();
            term.draw(|f| {
                draw_eq_curve(f, f.area(), 0, params, 48_000, Style::default());
            })
            .unwrap();
            let buf = term.backend().buffer().clone();
            (0..8)
                .map(|y| (0..60).map(|x| buf[(x, y)].symbol()).collect::<String>())
                .collect()
        };

        // Everything flat: the curve sits on the zero line and nowhere else.
        let flat = [0.5, 0.5, 0.5, 0.5, 0.3, 0.7, 0.3, 0.0, 0.0, 1.0];
        let rows = render(&flat);
        let above: usize = rows[..2]
            .iter()
            .map(|r| r.matches('\u{2501}').count())
            .sum();
        assert_eq!(
            above, 0,
            "a flat EQ has nothing to draw above the line:\n{rows:?}"
        );

        // A full low-shelf boost: the curve leaves the line on the left.
        let mut boosted = flat;
        boosted[0] = 1.0;
        let rows = render(&boosted);
        let count = |r: &String, range: std::ops::Range<usize>| {
            r.chars()
                .skip(range.start)
                .take(range.end - range.start)
                .filter(|c| *c == '\u{2501}')
                .count()
        };
        let top_left: usize = rows[..2].iter().map(|r| count(r, 0..20)).sum();
        assert!(
            top_left > 0,
            "a +18 dB low shelf should draw high on the left:\n{rows:?}"
        );
        let top_right: usize = rows[..2].iter().map(|r| count(r, 40..60)).sum();
        assert_eq!(top_right, 0, "and not on the right:\n{rows:?}");

        // The axis labels are the legend.
        assert!(
            rows[5].contains("1k"),
            "the frequency axis is marked: {}",
            rows[5]
        );
    }

    /// The waveshaper's points are drawn as the bank, so the curve is the
    /// editor: the identity climbs left to right, and each column is clickable.
    #[test]
    fn the_waveshaper_draws_its_curve_as_the_bank() {
        use choz_engine::fx::saturator::TABLE_POINTS;
        use ratatui::{backend::TestBackend, Terminal};
        let entry = crate::source::AudioFxEntry::new(crate::source::AudioFxKind::WaveShaper);
        assert_eq!(
            entry.params.len(),
            TABLE_POINTS + 5,
            "eight points plus drive, tone, output, oversampling and wet"
        );

        let labels: Vec<String> = (0..TABLE_POINTS)
            .map(|i| format!("{:+.1}", choz_engine::fx::saturator::Table::input_at(i)))
            .collect();
        let refs: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
        let mut term = Terminal::new(TestBackend::new(60, 12)).unwrap();
        let mut rects = Vec::new();
        term.draw(|f| {
            let (r, _) = draw_eq_bank(
                f,
                f.area(),
                0,
                &entry.params[..TABLE_POINTS],
                &refs,
                0,
                true,
                "1:WAVESHAPER",
                Style::default(),
            );
            rects = r;
        })
        .unwrap();
        assert_eq!(rects.len(), TABLE_POINTS, "one click rect per point");

        let buf = term.backend().buffer().clone();
        let rows: Vec<String> = (0..12)
            .map(|y| (0..60).map(|x| buf[(x, y)].symbol()).collect())
            .collect();
        // The identity curve rises: the first point is at the bottom of the
        // track and the last at the top.
        let row_of = |col_range: std::ops::Range<usize>| {
            rows.iter()
                .position(|r| {
                    r.chars()
                        .skip(col_range.start)
                        .take(col_range.end - col_range.start)
                        .any(|c| c == '\u{2588}')
                })
                .unwrap_or(99)
        };
        let first = row_of(0..7);
        let last = row_of(50..60);
        assert!(
            first > last,
            "the identity should climb left to right: {first} vs {last}\n{}",
            rows.join("\n")
        );
        assert!(
            rows.iter().any(|r| r.contains("-1.0")),
            "the axis is input level"
        );
    }

    /// A panel with no rows left draws no curve, rather than over the knobs.
    #[test]
    fn the_eq_curve_gives_up_before_it_overflows() {
        use ratatui::{backend::TestBackend, Terminal};
        let mut term = Terminal::new(TestBackend::new(60, 4)).unwrap();
        let mut after = 99;
        term.draw(|f| {
            after = draw_eq_curve(f, f.area(), 0, &[0.5; 10], 48_000, Style::default());
        })
        .unwrap();
        assert_eq!(after, 0, "no room, no drawing, and no rows consumed");
    }

    #[test]
    fn params_wrap_onto_more_rows_when_they_dont_fit() {
        // 80 columns of panel → 6 knob columns of 13.
        assert_eq!(param_grid(80, 6), (6, 1));
        assert_eq!(param_grid(80, 7), (6, 2), "a 7th knob starts a second row");
        assert_eq!(
            param_grid(80, 16),
            (6, 3),
            "Z5 Texture's 16 params take 3 rows"
        );
        assert_eq!(param_grid(10, 4), (1, 4), "a narrow panel stacks them");
        assert_eq!(param_grid(80, 0), (6, 0));
    }
}
