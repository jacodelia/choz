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

/// The looper strip's palette: a button's colour is how a symbol says which
/// button it is. `M` and `S` are the same shape; white and amber are not.
///
/// Each has a banked-down twin for the same button switched off, so a strip
/// reads the same whether anything is engaged on it or not.
const WHITE: Color = Color::Rgb(236, 238, 244);
const DIM_WHITE: Color = Color::Rgb(58, 62, 72);
const AMBER: Color = Color::Rgb(230, 200, 120);
const DIM_AMBER: Color = Color::Rgb(62, 54, 30);
const BLUE: Color = Color::Rgb(96, 156, 236);
const DIM_BLUE: Color = Color::Rgb(28, 44, 72);
const RED: Color = Color::Rgb(224, 88, 84);
const DIM_RED: Color = Color::Rgb(72, 28, 28);
const DIM_GREEN: Color = Color::Rgb(26, 60, 38);
const OFF_COLOUR: Color = Color::Rgb(96, 104, 118);

/// Which of the tab's two note generators the RACK is showing.
///
/// They are drawn one at a time, as tabs: both boxes at once cost the panel
/// nine rows it does not have, and a player is setting up one of them at a
/// time. The switches stay visible on the tab row either way, so a running
/// arpeggiator is never hidden behind a sequencer — only its controls are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GenTab {
    #[default]
    Arp,
    Seq,
}

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
    /// Listen to the tab's audio input and play its instrument from the pitch.
    PitchToMidi,
    /// How much of a converting tab's output is the instrument and how much is
    /// the audio that drove it.
    PitchMix,
    /// Open (or close) the plugin's own window.
    Gui,
    /// Open the harmonics of the instrument's oscillator — for a synth that
    /// keeps them out of its parameter list.
    Harmonics,
    /// Ask for (or stop asking for) this plugin to run in its own process.
    Sandbox,
    /// Previous / next program of the loaded SoundFont.
    PresetPrev,
    PresetNext,
    /// One of the tab's saved sounds. Left recalls it, right saves what the
    /// tab is playing into it.
    Sound(usize),
    /// One more sound button.
    SoundAdd,
    /// Which saved sound each octave of the keyboard plays.
    Split,
    /// Previous / next page of the instrument's own parameters. A synth like
    /// Surge XT has hundreds; the box shows a few rows of them, and these are
    /// how the rest are reached.
    InstrPagePrev,
    InstrPageNext,
    /// Type a few letters and land on the knob that has them in its name. A
    /// plugin with eight hundred parameters is a plugin whose `Cutoff` is
    /// twenty-six pages in, and paging to it is not finding it.
    InstrSearch,
    /// Put the instrument's own knobs back where the file opened them. Drawn
    /// only for a SoundFont, whose knobs are choz's own generator offsets and
    /// whose "default" therefore means something exact — the SF2 as written.
    /// A hosted plugin's defaults are the plugin's, and wiping a patch someone
    /// spent an hour on is not a button choz should offer by accident.
    InstrReset,
    /// The tab's arpeggiator. `ArpOn` is the only one drawn while it is off —
    /// a box of settings for something switched off is six rows of nothing in a
    /// panel that is already tight — and it sits on one row with `SeqOn`: the
    /// two ways a tab makes notes are switched on side by side.
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
    /// One key plays the memorised chord.
    ArpChord,
    ArpTap,
    /// The tab's step sequencer. `SeqOn` is the only one drawn while it is
    /// off, for the same reason `ArpOn` is: a grid for something switched off
    /// is rows this panel does not have. Drawn beside `ArpOn`, in the same
    /// shape.
    SeqOn,
    SeqPlay,
    SeqRec,
    /// Step the part being edited, `A`..`H`. In a box with a song chain it is
    /// also what appends to it — see [`crate::seq::Seq::chain`].
    SeqPart,
    SeqChain,
    SeqErase,
    /// How long a step is — the sequencer's quantisation. Opens the list, the
    /// way every other named value in this panel does.
    SeqQuant,
    /// The transport's time signature, which is what a bar of the pattern is
    /// long. Opens the list too.
    SeqTimeSig,
    /// What opens or ducks the selected effect: another tab, the external
    /// clock, or the internal metronome's tap.
    FxGate,
    /// Which keyboard the selected effect takes its chord from. Only the
    /// harmoniser has one, so only the harmoniser draws it.
    FxChord,
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
    /// What part of the instrument's list its box is drawing: `(first, one
    /// past the last, whether it is a bank)`.
    ///
    /// The list is not one flat run of knobs — a bank of sliders is drawn whole
    /// and on its own — so the box shows **one segment at a time** and the page
    /// keys walk between them. Without that the grid drew the bank's members
    /// too, on a page the arrows could never land on.
    pub instr_segment: Option<(usize, usize, bool)>,
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
    /// ((track, step), rect) of the sequencer's grid cells that are on screen.
    /// Empty while it is off, or on a panel with no rows to draw a grid in.
    pub seq_steps: Vec<((usize, usize), Rect)>,
    /// (track, rect) of the grid's row letters. Clicking one opens the keyboard
    /// that says which note the lane plays.
    pub seq_tracks: Vec<(usize, Rect)>,
    /// The FX CHAIN box itself. A click anywhere inside it hands it the arrows,
    /// which is what turns its border yellow — the section is a box now, so it
    /// says whether it is the live one the way every other box does.
    pub fx_area: Option<Rect>,
    /// `((track, button), rect)` for the looper deck's per-track transport.
    /// Empty for every effect that is not one.
    pub loop_hits: Vec<((usize, LoopBtn), Rect)>,
    /// The deck's own row: paging, `+`, clear, export.
    pub loop_deck: Vec<(LoopBtn, Rect)>,
    /// `((channel, which), bar rect)` of a strip's two sliders — the pan and
    /// the level. The rect is the bar alone, so where a click lands in it *is*
    /// the value, the same contract the sequencer's sliders have.
    pub loop_sliders: Vec<((usize, LoopBtn), Rect)>,
    /// `(page, pages)` the deck was drawn with. The panel is the only side that
    /// knows how many strips fit in the width, so it is the side that says how
    /// many pages there are.
    pub loop_pages: (usize, usize),
    /// (slider, **bar** rect) of the sequencer's three variation sliders. The
    /// rect is the bar alone and not the whole label, so where a click lands in
    /// it *is* the value — see [`seq_slider_at`].
    pub seq_sliders: Vec<(SeqSlider, Rect)>,
}

/// The sequencer's three variation sliders: what turns a written grid into a
/// part that is played rather than repeated.
///
/// RAND and PROB are one gesture in two halves — **how far** a step may stray
/// from what was written, and **how often** it is allowed to. Either at zero is
/// the pattern exactly as it was typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeqSlider {
    /// How far the off-beats are pushed back.
    Swing,
    /// How wide the deviation is when a step takes one.
    Random,
    /// How often a step takes one.
    Prob,
}

/// A button on one looper channel strip, or on the deck's row under them.
///
/// Every one of these is a control the player can reach three ways — mouse,
/// keyboard, or a learned CC — which is the point of a looper: whoever is
/// recording has both hands on a guitar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopBtn {
    /// Arm the channel, and close the take on the second press.
    Rec,
    /// Start the take — and, on a take already going, pause it in place. The
    /// deck has one playhead, so a paused take comes back in time with the
    /// others instead of at the top.
    Play,
    /// Give the take up. Where PAUSE keeps its place, this loses it, and the
    /// deck rewinds once nothing is holding the playhead.
    Stop,
    Clear,
    /// Out of the mix, this one.
    Mute,
    /// Only this one — a mute of everything else, and only while something is
    /// soloed.
    Solo,
    /// Throw the channel away. On the strip's top-right corner, where a window
    /// puts its close box.
    Del,
    /// Where the take sits between the speakers. A slider: what it answers is
    /// **where** it was clicked, not that it was.
    Pan,
    /// How loud the take plays. A slider, like the pan.
    Vol,
    /// What this channel's closed take rounds to.
    Quant,
    /// choz's own metronome, from the strip. Not a setting of the deck's: it is
    /// the same click every other part of the program hears, on the same
    /// transport the quantise rounds to.
    Metro,
    Export,
    /// One more channel strip, up to [`choz_ports::LOOP_TRACKS`].
    AddChan,
    PagePrev,
    PageNext,
}

impl LoopBtn {
    /// The buttons of one strip, top to bottom, as the keyboard walks them.
    /// The input monitor is not among them — it is a reading, not a control.
    /// ponytail: the two sliders are not on this walk — they are grabbed, and
    /// they are parameters, so a learned CC reaches them. Give them a keyboard
    /// step if anyone asks for one.
    pub const STRIP: [LoopBtn; 8] = [
        LoopBtn::Metro,
        LoopBtn::Quant,
        LoopBtn::Mute,
        LoopBtn::Solo,
        LoopBtn::Stop,
        LoopBtn::Play,
        LoopBtn::Rec,
        LoopBtn::Del,
    ];

    /// The deck's own row, under the strips — one more stop on the same walk,
    /// so `+`, the page arrows, CLEAR and EXPORT are all reachable without a
    /// mouse.
    pub const DECK: [LoopBtn; 5] = [
        LoopBtn::PagePrev,
        LoopBtn::PageNext,
        LoopBtn::AddChan,
        LoopBtn::Clear,
        LoopBtn::Export,
    ];

    /// Whether this button belongs to the row under the strips.
    pub fn on_deck(self) -> bool {
        Self::DECK.contains(&self)
    }
}

/// Cells the bar of a slider is drawn in.
pub const SEQ_BAR_W: u16 = 8;

/// A slider as it is drawn: the whole label, and how many columns come before
/// its bar. The two are built together so the drawing and the mouse cannot
/// disagree about where the bar is.
fn seq_slider_label(name: &str, v: f32) -> (String, u16) {
    let v = v.clamp(0.0, 1.0);
    let filled = (v * SEQ_BAR_W as f32).round() as usize;
    let bar: String = (0..SEQ_BAR_W as usize)
        .map(|i| if i < filled { '\u{2588}' } else { '\u{2591}' })
        .collect();
    let head = format!(" {name} ");
    let prefix = head.chars().count() as u16;
    (format!("{head}{bar} {:>3.0}% ", v * 100.0), prefix)
}

/// The value a click at column `x` on a slider's bar means, 0..1.
pub fn seq_slider_at(bar: Rect, x: u16) -> f32 {
    let span = bar.width.saturating_sub(1).max(1) as f32;
    (x.saturating_sub(bar.x) as f32 / span).clamp(0.0, 1.0)
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

/// The run of bank-drawn sliders `cursor` is inside, as `(from, to)`.
///
/// Long enough to be worth the whole box: below that the grid says more, since
/// a handful of faders still fit as cells with their names and numbers under
/// them. Thirty-two harmonics are a spectrum and eight cells are a page.
const BANK_MIN: usize = 8;

/// The part of the list the box should draw for a cursor at `cursor`:
/// `(first, one past the last, whether it is a bank)`.
///
/// A bank run is a segment of its own; everything between two of them is
/// another. That is what keeps a slider out of the knob grid: it is drawn in
/// the bank it belongs to, and nowhere else.
pub fn instr_segment(shapes: &[ParamShape], cursor: usize) -> (usize, usize, bool) {
    if let Some((from, to)) = bank_run(shapes, cursor) {
        return (from, to, true);
    }
    let banked = |i: usize| bank_run(shapes, i).is_some();
    let mut from = cursor;
    while from > 0 && !banked(from - 1) {
        from -= 1;
    }
    let mut to = (cursor + 1).min(shapes.len());
    while to < shapes.len() && !banked(to) {
        to += 1;
    }
    (from, to, false)
}

pub fn bank_run(shapes: &[ParamShape], cursor: usize) -> Option<(usize, usize)> {
    let ParamShape::Fader(unit) = shapes.get(cursor)? else {
        return None;
    };
    let same = |i: usize| matches!(&shapes[i], ParamShape::Fader(u) if u == unit);
    let mut from = cursor;
    while from > 0 && same(from - 1) {
        from -= 1;
    }
    let mut to = cursor + 1;
    while to < shapes.len() && same(to) {
        to += 1;
    }
    (to - from >= BANK_MIN).then_some((from, to))
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
pub const BTN_HARMONICS: &str = " HARM ";
/// How many cells a run of spans takes on screen — the only honest way to know
/// where the next button lands once any of the text before it is translated.
fn line_width(spans: &[Span]) -> u16 {
    spans.iter().map(|s| s.width() as u16).sum()
}

pub const BTN_RESET: &str = " RESET ";
/// Opens the knob search on the instrument box.
pub const BTN_SEARCH: &str = " \u{2315} ";
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

    /// Whether anything was drawn on it. A row nobody used costs a line the
    /// RACK does not have — which is what the arpeggiator's row became once
    /// its switch moved up beside the sequencer's.
    fn used(&self) -> bool {
        self.x > self.inner.x + self.indent
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

/// A gate source in the width a chain button can spare: the tab's number, or
/// the two clock sources by name. The full label is in the gate's own picker,
/// which is where there is room for one.
fn gate_source_mark(source: choz_engine::fx_chain::GateSource) -> String {
    use choz_engine::fx_chain::GateSource;
    match source {
        GateSource::Tab(i) => (i + 1).to_string(),
        // The tab's number with a note on it: the button has room for two
        // cells, and which of the two readings it is has to be one of them.
        GateSource::Note(i) => format!("{}\u{266A}", i + 1),
        GateSource::Clock => "CLK".into(),
        GateSource::Metronome => "MET".into(),
        GateSource::Seq => "SEQ".into(),
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
/// A knob shape costs the arpeggiator no row of its own: its switch is on the
/// row of switches above, beside the sequencer's, and TAP rides the box's top
/// edge. Only the row-of-buttons shape spends a line, and it spends its own.
///
/// The arpeggiator only takes a knob shape when this much is left after it,
/// which is what keeps a five-inch screen showing an FX chain at all.
///
/// Still seven now the section draws itself as a box: the frame costs a row
/// more than the rule it replaced, and taking that row from this floor instead
/// is what put a five-inch screen back to buttons where it used to have knobs.
/// The chain gives up the row; the generator above it keeps its shape.
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
    // **The rows on screen, not the last of them.** It read `2/3` while rows
    // one and two were both up, so the first row looked like a page nobody
    // could reach — it was the one being looked at.
    let more = if rows_needed > rows_shown {
        format!(
            " ({}\u{2013}{}/{} rows) ",
            first_row + 1,
            first_row + rows_shown,
            rows_needed
        )
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

/// The tab's saved sounds, as the RACK draws them.
pub struct SoundsView<'a> {
    /// One entry per button: the name it holds, or `None` for an empty one.
    pub names: &'a [Option<String>],
    /// The button last recalled, drawn lit.
    pub active: Option<usize>,
    /// Whether there is room for another button.
    pub can_add: bool,
    /// Whether any octave of the keyboard is set to one of these sounds.
    pub split: bool,
}

/// Rows the sequencer's grid needs before it is worth drawing one: the box's
/// two border rows, its step ruler, and a single track.
const SEQ_BOX_ROWS: u16 = 4;

/// Columns the grid needs: the note label, sixteen cells, the three gaps
/// between the four groups of four, and the border.
const SEQ_BOX_COLS: u16 = 26;

/// Draw the tab's step sequencer above the instrument — the order the signal
/// runs in, which is the order the panel is read in: the notes are made here,
/// the instrument plays them, the chain colours them.
///
/// One row of buttons while it is off, and the grid on top of that row when it
/// is on and the panel has the rows for it. Same rule the arpeggiator follows,
/// for the same reason: the RACK is short of rows before it is short of
/// anything else.
#[allow(clippy::too_many_arguments)]
fn draw_seq_box(
    f: &mut Frame,
    inner: Rect,
    mut y: u16,
    seq: crate::seq::SeqView<'_>,
    focused: bool,
    bg: Style,
    btn_style: Style,
    layout: &mut RackLayout,
) -> u16 {
    use crate::seq::{part_name, STEPS, TRACKS};

    let s = seq.settings;
    let lit = Style::default()
        .fg(Color::Black)
        .bg(ON_COLOUR)
        .add_modifier(Modifier::BOLD);
    let armed = Style::default()
        .fg(Color::Black)
        .bg(Color::Rgb(210, 80, 76))
        .add_modifier(Modifier::BOLD);

    // The switch is not here: it sits beside the arpeggiator's, on the row of
    // switches above both of them. Off, this box is nothing at all.
    if !s.on {
        return y;
    }
    let mut row = ButtonRow::new(inner, bg, y, 2);
    let mut button = |row: &mut ButtonRow, f: &mut Frame, btn, text: String, style| {
        let rect = row.button(f, text, style);
        layout.buttons.push((btn, rect));
    };
    button(
        &mut row,
        f,
        RackButton::SeqPlay,
        format!(" {} ", t(if seq.playing { "STOP" } else { "PLAY" })),
        if seq.playing { lit } else { btn_style },
    );
    // REC arms the recorder; what it writes is whatever is played into the tab,
    // quantised to the step the playhead is on.
    button(
        &mut row,
        f,
        RackButton::SeqRec,
        " REC ".to_string(),
        if seq.rec { armed } else { btn_style },
    );
    button(
        &mut row,
        f,
        RackButton::SeqPart,
        format!(" PART {} ", part_name(s.part)),
        btn_style,
    );
    // Quantisation and metre: the step length, and how many steps a bar of it
    // is. Both open a list rather than cycling — stepping eight divisions with
    // a button to reach 1/16T is a knob pretending to be a menu.
    button(
        &mut row,
        f,
        RackButton::SeqQuant,
        format!(" QUANT {} ", s.div.label()),
        btn_style,
    );
    let (num, den) = choz_ports::transport().time_signature();
    button(
        &mut row,
        f,
        RackButton::SeqTimeSig,
        format!(" {num}/{den} ", num = num, den = den),
        btn_style,
    );
    // The chain is written the way an MMT-8 writes one: the part being edited,
    // appended in the order the parts should play.
    button(
        &mut row,
        f,
        RackButton::SeqChain,
        if s.song.is_empty() {
            " SONG \u{2013} ".to_string()
        } else {
            format!(
                " SONG {} ",
                s.song
                    .iter()
                    .map(|p| part_name(*p).to_string())
                    .collect::<Vec<_>>()
                    .join("")
            )
        },
        if s.song.is_empty() { btn_style } else { lit },
    );
    button(
        &mut row,
        f,
        RackButton::SeqErase,
        " ERASE ".to_string(),
        btn_style,
    );
    // The three variations, as sliders: a grid is a rhythm nobody played, and
    // these are what make it one that somebody might have. They wrap onto the
    // next line with the rest of the row when the panel is narrow.
    for (slider, name, value) in [
        (SeqSlider::Swing, "SWING", s.swing / crate::seq::MAX_SWING),
        (SeqSlider::Random, "RAND", s.random),
        (SeqSlider::Prob, "PROB", s.prob),
    ] {
        let (label, prefix) = seq_slider_label(name, value);
        let rect = row.button(
            f,
            label,
            if value > 0.0 {
                Style::default()
                    .fg(ON_COLOUR)
                    .bg(bg.bg.unwrap_or(Color::Reset))
            } else {
                btn_style
            },
        );
        // Only the bar answers the mouse, and only as far as it was really
        // drawn: a row that wrapped mid-label hands back a short rect.
        let bar_x = rect.x + prefix;
        let bar_w = SEQ_BAR_W.min((rect.x + rect.width).saturating_sub(bar_x));
        if bar_w > 0 {
            layout
                .seq_sliders
                .push((slider, Rect::new(bar_x, rect.y, bar_w, 1)));
        }
    }
    y = row.finish();

    // The grid, when there are rows left for one after the FX chain has its
    // floor. Fewer tracks than eight is not a failure: the window follows the
    // cursor, so every track is reachable on a screen that can only show two.
    let room = (inner.y + inner.height).saturating_sub(y);
    if room < SEQ_BOX_ROWS + FX_CHAIN_ROWS || inner.width < SEQ_BOX_COLS {
        return y;
    }
    let rows = (room - FX_CHAIN_ROWS - 3).min(TRACKS as u16).max(1) as usize;
    // Keep the cursor's track on screen, and show the tracks that follow it.
    let first = seq
        .cursor
        .0
        .saturating_sub(rows - 1)
        .min(TRACKS.saturating_sub(rows));

    // The cursor's lane says which note it plays: the rows are letters now, so
    // this is where the note went — on the one row somebody is looking at.
    let title = format!(
        " {} \u{00B7} {} {} \u{00B7} {} {} \u{00B7} {} {}{} ",
        t("SEQ"),
        t("PART"),
        part_name(s.part),
        s.events(),
        t("EVENTS"),
        crate::seq::track_name(seq.cursor.0),
        crate::seq::note_name(s.notes[seq.cursor.0.min(TRACKS - 1)]),
        if focused && seq.focused {
            String::new()
        } else {
            "  [K]".to_string()
        }
    );
    let height = rows as u16 + 3;
    let area = Rect::new(inner.x, y, inner.width, height);
    let block = Block::default()
        .title(title)
        .title_style(Style::default().fg(HEADER).add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_style(if focused && seq.focused {
            Style::default().fg(SEL).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(ui_border())
        });
    let grid = block.inner(area);
    f.render_widget(block, area);

    // Where the grid breaks, and what the ruler counts.
    //
    // **The bar, not four cells.** The gaps and the beat numbers used to fall
    // every fourth step whatever the signature was, so 7/8 was drawn as four
    // beats of four — a picture of a bar the sequencer was not playing. A beat
    // is the signature's own unit read at the step length (an eighth in 7/8,
    // two cells at 1/16), and where a bar is *grouped* — 3+2+2, as the click
    // accents it — the break follows the groups instead.
    let beat = seq.beat.max(1);
    let stops = &seq.stops;
    let breaks = |step: usize| step > 0 && stops.contains(&step);
    // At one cell to a beat a gap between every pair of cells is not a grid any
    // more, so the ruler carries the beat and the cells stay together.
    let gapped = beat >= 2 || stops.len() < seq.bar;
    let gaps_before = |step: usize| match gapped {
        true => stops.iter().filter(|s| **s > 0 && **s <= step).count() as u16,
        false => 0,
    };
    let cell_x = |step: usize| grid.x + SEQ_LABEL_W + step as u16 + gaps_before(step);

    // The ruler: the beat numbers, and the playhead over them.
    let mut ruler: Vec<Span> = vec![Span::styled(
        " ".repeat(SEQ_LABEL_W as usize),
        Style::default().fg(LABEL),
    )];
    for step in 0..STEPS {
        if gapped && breaks(step) {
            ruler.push(Span::raw(" "));
        }
        let here = seq.playing && seq.step == step;
        // The beat this step is on, counted from the start of the bar — which
        // is what a player reads to find "the three of the second bar".
        let on_beat = step.is_multiple_of(beat);
        let starts_group = stops.contains(&step);
        ruler.push(Span::styled(
            if step >= seq.bar {
                " ".to_string()
            } else if on_beat {
                // Two digits do not fit a cell: past nine the beat is marked
                // rather than numbered, which is all a long bar needs.
                match step / beat + 1 {
                    n @ 1..=9 => n.to_string(),
                    _ => "\u{00B7}".to_string(),
                }
            } else {
                "\u{00B7}".to_string()
            },
            if here {
                Style::default().fg(Color::Black).bg(SEL)
            } else if starts_group && step > 0 {
                // Where a group starts — the accent the click plays.
                Style::default().fg(HEADER).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(LABEL)
            },
        ));
    }
    f.render_widget(
        Paragraph::new(Line::from(ruler)).style(bg),
        Rect::new(grid.x, grid.y, grid.width, 1),
    );

    for (i, track) in (first..first + rows).enumerate() {
        let row_y = grid.y + 1 + i as u16;
        // The lane's letter, not its note: a row named after a setting is a
        // row that renames itself while the pattern is being read. The note is
        // one click away — the letter opens a keyboard.
        let label = format!(" {}  ", crate::seq::track_name(track));
        let mut spans: Vec<Span> = vec![Span::styled(
            label,
            Style::default().fg(if seq.cursor.0 == track { SEL } else { LABEL }),
        )];
        layout
            .seq_tracks
            .push((track, Rect::new(grid.x, row_y, SEQ_LABEL_W, 1)));
        for step in 0..STEPS {
            if gapped && breaks(step) {
                spans.push(Span::raw(" "));
            }
            let on = s.step_on(track, step);
            let cursor = focused && seq.focused && seq.cursor == (track, step);
            let head = seq.playing && seq.step == step;
            // Past the end of the bar: drawn, so the metre is visible as the
            // shape of the pattern, and never played.
            let outside = step >= seq.bar;
            let style = if cursor {
                Style::default().fg(Color::Black).bg(SEL)
            } else if on {
                Style::default().fg(if outside { OFF_COLOUR } else { ON_COLOUR })
            } else if head {
                Style::default().fg(RULE).bg(Color::Rgb(48, 56, 68))
            } else {
                Style::default().fg(RULE)
            };
            spans.push(Span::styled(
                if on {
                    "\u{25A0}"
                } else if outside {
                    " "
                } else {
                    "\u{00B7}"
                }
                .to_string(),
                style,
            ));
            layout
                .seq_steps
                .push(((track, step), Rect::new(cell_x(step), row_y, 1, 1)));
        }
        f.render_widget(
            Paragraph::new(Line::from(spans)).style(bg),
            Rect::new(grid.x, row_y, grid.width, 1),
        );
    }
    area.y + area.height
}

/// Columns the note name of a track takes, plus the space after it.
const SEQ_LABEL_W: u16 = 4;

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
    // Whether it has a set of harmonics choz can draw.
    has_harmonics: bool,
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
    // Which section of the plugin the cursor is in, when it has sections. The
    // cells show the short name; this is the heading that gives it back.
    instr_section: Option<String>,
    instr_cursor: usize,
    // Which of the two knob boxes the arrows and the highlight belong to.
    instr_focused: bool,
    // Whether to offer RESET on the instrument box — see
    // [`RackButton::InstrReset`].
    instr_reset: bool,
    // The knob search, when it is open: the letters, which match it is on, and
    // how many there are. `None` draws the button that opens it.
    instr_search: Option<(&str, usize, usize)>,
    // `(trim, gate)` of the tab's audio input, or `None` when it plays its own
    // instrument and there is nothing coming in to trim.
    in_trim: Option<(f32, f32)>,
    // The AutoTune reading and its recent pitch error, when that is the FX the
    // cursor is on. `None` for every other effect.
    at_view: Option<(choz_engine::fx::autotune::AutoTuneMeter, &[f32])>,
    // The tab's arpeggiator: settings plus where its sequencer is. Drawn as one
    // line while it is off, two when it is on.
    arp: crate::arp::ArpView<'_>,
    // The tab's step sequencer, drawn above the instrument because that is the
    // order the notes travel in. `None` for a rack with no tab at all.
    seq: Option<crate::seq::SeqView<'_>>,
    // Which generator's controls are on screen. The other one keeps running;
    // only its box is put away.
    gen_tab: GenTab,
    // Which input algorithm the tab runs, and the knobs of the ALGO box — the
    // picker first, then whatever the running algorithm owns. Both come from
    // the interface rather than being worked out again here: a box whose knobs
    // are not the knobs being edited moves the wrong control.
    // Where this tab's notes are pointed, when they are pointed anywhere, and
    // over how much of that parameter's range.
    // Meter, latency and presets of the selected FX — everything the SLOT box
    // knows that is not a button.
    fx_info: FxSlotInfo,
    // The tab's sound buttons: a footswitch's worth of patches.
    sounds: SoundsView<'_>,
    // The looper deck in the selected slot, when that is what it is. Read from
    // the handle's atomics, so what is drawn is what the callback published.
    deck: Option<LoopView<'_>>,
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
        // …and only a synth whose harmonics can be reached gets that one.
        (
            RackButton::Harmonics,
            has_harmonics.then(|| BTN_HARMONICS.to_string()),
        ),
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

    // ── Saved sounds ───────────────────────────────────────────────────────
    //
    // A row of buttons holding the tab's own patches — not the plugin's preset
    // list, which is the BANK line above: these are the sound *as the player
    // left it*, and the reason they are on the panel rather than in a menu is
    // that they are reached mid-song.
    if !sounds.names.is_empty() {
        let mut row = ButtonRow::new(inner, bg, y, 2);
        row.label(
            f,
            format!("{}  ", t("SOUNDS")),
            Style::default().fg(LABEL).add_modifier(Modifier::BOLD),
        );
        for (i, name) in sounds.names.iter().enumerate() {
            let filled = name.is_some();
            let style = match (Some(i) == sounds.active && filled, filled) {
                (true, _) => Style::default()
                    .fg(Color::Black)
                    .bg(ON_COLOUR)
                    .add_modifier(Modifier::BOLD),
                (false, true) => Style::default().fg(ui_text()).bg(Color::Rgb(40, 46, 56)),
                // An empty button is drawn all the same: it is where a sound
                // goes, and a row that grows as you save is a row you cannot
                // aim at.
                (false, false) => Style::default().fg(Color::Rgb(90, 95, 105)),
            };
            let label = match name {
                Some(n) => format!(" {}:{} ", i + 1, truncate(n, 8)),
                None => format!(" {} ", i + 1),
            };
            let rect = row.button(f, label, style);
            layout.buttons.push((RackButton::Sound(i), rect));
        }
        if sounds.can_add {
            let rect = row.button(f, " + ".to_string(), btn_style);
            layout.buttons.push((RackButton::SoundAdd, rect));
        }
        // The split lives on this row because it is about these buttons: it is
        // which of them each octave of the keyboard plays. Lit while the tab
        // has one set, because a split is the setting that explains a keyboard
        // that changes sound halfway up.
        let rect = row.button(
            f,
            format!(
                " {} {} ",
                t("SPLIT"),
                if sounds.split { "\u{25CF}" } else { "\u{25CB}" }
            ),
            match sounds.split {
                true => Style::default()
                    .fg(Color::Black)
                    .bg(ON_COLOUR)
                    .add_modifier(Modifier::BOLD),
                false => btn_style,
            },
        );
        layout.buttons.push((RackButton::Split, rect));
        y = row.finish();
    }

    // ── The two note generators, as tabs ───────────────────────────────────
    //
    // A tab can make notes two ways — a pattern of steps and an arpeggiator —
    // and they share one row and one strip of rows below it: the tabs pick
    // which set of controls is on screen, and the `●`/`○` on each says whether
    // that one is running whether or not it is the one showing. Both boxes at
    // once cost nine rows the RACK does not have, and nobody is dialling in
    // two generators in the same second.
    //
    // Clicking a tab that is not showing brings it up; clicking the one that
    // already is switches it on or off — the same "select, then act" the knobs
    // in this panel answer a second click with.
    {
        let mut row = ButtonRow::new(inner, bg, y, 2);
        // The knob box addresses its controls by what they are, so the switch
        // is highlighted from the cursor's *parameter*, not from its position.
        let arp_selected = focused
            && arp.focused
            && arp
                .knobs()
                .get(arp.cursor)
                .map(|(p, ..)| *p)
                .is_some_and(|p| p == crate::arp::ArpParam::On);
        let switch = |f: &mut Frame,
                      row: &mut ButtonRow,
                      layout: &mut RackLayout,
                      btn: RackButton,
                      label: &str,
                      on: bool,
                      selected: bool,
                      showing: bool| {
            let style = if selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(SEL)
                    .add_modifier(Modifier::BOLD)
            } else if showing {
                // The tab whose controls are on screen wears the panel's own
                // header colour, exactly as the rack's slot tabs do — one tab
                // idiom, not two.
                Style::default()
                    .fg(Color::Black)
                    .bg(if on { ON_COLOUR } else { HEADER })
                    .add_modifier(Modifier::BOLD)
            } else if on {
                Style::default()
                    .fg(ON_COLOUR)
                    .bg(Color::Rgb(40, 46, 56))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(Color::Rgb(200, 205, 215))
                    .bg(Color::Rgb(40, 46, 56))
            };
            let text = format!(" {} {} ", label, if on { "\u{25CF}" } else { "\u{25CB}" });
            let rect = row.button(f, text, style);
            layout.buttons.push((btn, rect));
        };
        switch(
            f,
            &mut row,
            &mut layout,
            RackButton::ArpOn,
            t("ARP"),
            arp.settings.on,
            arp_selected,
            gen_tab == GenTab::Arp,
        );
        // Drawn even for a rack with no tab: an empty rack shows the controls
        // it would have, and a switch that appears only sometimes is a switch
        // nobody learns where to find.
        switch(
            f,
            &mut row,
            &mut layout,
            RackButton::SeqOn,
            t("SEQ"),
            seq.as_ref().is_some_and(|v| v.settings.on),
            false,
            gen_tab == GenTab::Seq,
        );
        y = row.finish();
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
    let mut arp_boxed = false;
    if gen_tab == GenTab::Arp {
        let s = arp.settings;
        // Which control the arrows are on, as buttons rather than as a knob:
        // on a panel too short for the box the row **is** the box, and a
        // cursor nobody can see is a cursor nobody can use.
        let knobs = arp.knobs();
        let cursor_btns: &[RackButton] = if arp.focused && focused {
            match knobs.get(arp.cursor).map(|(p, ..)| *p) {
                Some(crate::arp::ArpParam::On) => &[RackButton::ArpOn],
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
        } else if room >= ARP_KNOBS_ROWS + 2 + FX_CHAIN_ROWS {
            ArpShape::Boxed
        } else if room >= ARP_KNOBS_ROWS + FX_CHAIN_ROWS {
            ArpShape::Strip
        } else {
            ArpShape::Buttons
        };
        let boxed = matches!(shape, ArpShape::Boxed | ArpShape::Strip);
        arp_boxed = boxed;

        // The switch is on the row of switches above, beside the sequencer's:
        // the two generators a tab has are turned on in the same place, in the
        // same shape, whatever shape their controls then take. TAP is never a
        // knob either way: tapping a tempo is a gesture, not a position.
        let mut row = ButtonRow::new(inner, bg, y, 2);
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
        y = if row.used() { row.finish() } else { y };

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
            // TAP rides the box's top edge, right-aligned: a gesture that
            // belongs to the arpeggiator should not be floating above it, and
            // the edge costs no row — which is what this panel never has. Not
            // the left edge, because that is where the box says which key
            // hands it the arrows.
            if matches!(shape, ArpShape::Boxed) {
                let labels = [(
                    RackButton::ArpTap,
                    format!(" TAP {:>3.0} ", s.tempo()),
                    false,
                )];
                let total: u16 = labels
                    .iter()
                    .map(|(_, l, _)| l.chars().count() as u16)
                    .sum();
                let right = inner.x + inner.width.saturating_sub(1);
                if right > inner.x + total + 2 {
                    let mut x = right.saturating_sub(total + 1);
                    for (btn, label, lit) in labels {
                        let w = label.chars().count() as u16;
                        let style = if cursor_btns.contains(&btn) {
                            Style::default()
                                .fg(Color::Black)
                                .bg(SEL)
                                .add_modifier(Modifier::BOLD)
                        } else if lit {
                            Style::default()
                                .fg(Color::Black)
                                .bg(ON_COLOUR)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            btn_style
                        };
                        let rect = Rect::new(x, box_top, w, 1);
                        f.render_widget(Paragraph::new(Span::styled(label, style)), rect);
                        layout.buttons.push((btn, rect));
                        x += w;
                    }
                }
            }
            y = next;
        }
    }

    // ── Sequencer ──────────────────────────────────────────────────────────
    //
    // Above the instrument: the sequencer makes the notes, the instrument plays
    // them. Reading the panel downwards is reading the signal in order.
    let seq_focused = gen_tab == GenTab::Seq && seq.as_ref().is_some_and(|v| v.focused);
    if let (GenTab::Seq, Some(view)) = (gen_tab, seq) {
        y = draw_seq_box(f, inner, y, view, focused, bg, btn_style, &mut layout);
    }

    // Whichever box has the arrows is the one drawn live: with four boxes on
    // the panel, "not the instrument's" stopped being the same as "the FX's".
    let fx_focused = focused
        && !instr_focused
        && !(gen_tab == GenTab::Arp && arp_boxed && arp.focused)
        && !seq_focused;

    // ── Instrument parameters ──────────────────────────────────────────────
    if !instr_params.is_empty() {
        let values: Vec<f32> = instr_params.iter().map(|(_, v, _)| *v).collect();
        let names: Vec<String> = instr_params.iter().map(|(n, _, _)| n.clone()).collect();
        let shapes: Vec<ParamShape> = instr_params.iter().map(|(_, _, s)| s.clone()).collect();
        let cursor = instr_cursor.min(shapes.len().saturating_sub(1));
        let title = format!(
            "{} \u{00B7} {}{}{}",
            t("INSTRUMENT"),
            truncate(instrument, 18),
            match &instr_section {
                Some(g) => format!(" \u{00B7} {}", truncate(g, 18)),
                None => String::new(),
            },
            // `[k]` is the key that hands it the arrows, so it is shown to
            // whoever does not have them.
            if focused && instr_focused {
                ""
            } else {
                "  [k]"
            }
        );
        // The box draws **one segment** of the list: a long run of sliders is a
        // shape — a spectrum, a curve — and it is drawn whole, as the same bank
        // the graphic EQ uses; everything between two such runs is a segment of
        // knobs. That is what keeps a slider out of the grid, where it was
        // being drawn a second time on a page the arrows could never reach.
        let (from, to, is_bank) = instr_segment(&shapes, cursor);
        let banked = is_bank
            .then(|| {
                let labels: Vec<&str> = names[from..to].iter().map(|s| s.as_str()).collect();
                let (band_rects, after) = draw_eq_bank(
                    f,
                    inner,
                    y,
                    &values[from..to],
                    &labels,
                    cursor - from,
                    focused && instr_focused,
                    &title,
                    bg,
                );
                // Too narrow or too short for a bank: the grid draws it
                // instead, and it is a segment of knobs like any other.
                (!band_rects.is_empty()).then(|| {
                    let rects: Vec<(usize, Rect)> =
                        band_rects.into_iter().map(|(i, r)| (i + from, r)).collect();
                    (rects, after)
                })
            })
            .flatten();
        let drew_bank = banked.is_some();
        let (rects, next) = match banked {
            Some(pair) => pair,
            None => {
                let (rects, next) = draw_knob_box(
                    f,
                    inner,
                    y,
                    // The title carries the key that hands it the arrows: two
                    // knob boxes on one panel need to say which one is live.
                    // Built above, because the bank draws under the same one.
                    &title,
                    &values[from..to],
                    &names[from..to],
                    &shapes[from..to],
                    // Clamped here, where the box is drawn: the cursor belongs
                    // to the rack and this list to the tab, and a cursor past
                    // the end scrolls the window off the list — an empty box
                    // for a tab whose knobs are right there. The arpeggiator's
                    // box has always done this.
                    cursor.saturating_sub(from).min(to - from - 1),
                    focused && instr_focused,
                    INSTR_KNOB_ROWS,
                    // Leave the FX chain its rule, its buttons and a knob row.
                    9,
                    true,
                );
                // The box numbers its cells from the start of the segment; the
                // tab does not.
                (
                    rects.into_iter().map(|(i, r)| (i + from, r)).collect(),
                    next,
                )
            }
        };
        layout.instr_segment = Some((from, to, drew_bank));
        // The box's own top edge, right-aligned, where the arpeggiator's TAP
        // sits: RESET, then the page arrows when there is more than one page.
        // They are learn targets like any other button — a synth whose knobs
        // are on page 4 is no use to someone with both hands busy, and neither
        // is an undo you have to find with a mouse.
        let mut edge: Vec<(RackButton, &str)> = Vec::new();
        if instr_reset {
            edge.push((RackButton::InstrReset, BTN_RESET));
        }
        // Beside the pager, because it is the other way through the same list:
        // one walks it, this one jumps.
        let search = match instr_search {
            // The count, because a search that matched nothing has to say so:
            // without it a word the plugin does not use looks exactly like a
            // cursor that moved somewhere off screen.
            Some((q, at, n)) => format!(" \u{2315} {q}\u{2588} {at}/{n} "),
            None => BTN_SEARCH.to_string(),
        };
        if rects.len() < instr_params.len() || instr_search.is_some() {
            edge.push((RackButton::InstrSearch, &search));
            edge.push((RackButton::InstrPagePrev, BTN_PREV));
            edge.push((RackButton::InstrPageNext, BTN_NEXT));
        }
        if !edge.is_empty() {
            let w = edge
                .iter()
                .map(|(_, t)| Span::raw(*t).width() as u16)
                .sum::<u16>()
                + edge.len() as u16
                - 1;
            let right = inner.x + inner.width.saturating_sub(1);
            if right > inner.x + w + 2 {
                let mut x = right.saturating_sub(w + 1);
                for (btn, text) in edge {
                    let bw = Span::raw(text).width() as u16;
                    let rect = Rect::new(x, y, bw, 1);
                    // The search wears the accent while it is taking letters:
                    // every key is going into it and nowhere else, which is
                    // the one thing the player has to be able to see.
                    let style = match btn == RackButton::InstrSearch && instr_search.is_some() {
                        true => Style::default()
                            .fg(Color::Black)
                            .bg(ON_COLOUR)
                            .add_modifier(Modifier::BOLD),
                        false => btn_style,
                    };
                    f.render_widget(Paragraph::new(Span::styled(text, style)), rect);
                    layout.buttons.push((btn, rect));
                    x += bw;
                }
            }
        }
        layout.instr_knobs = rects;
        y = next;
    }

    // ── FX chain ───────────────────────────────────────────────────────────
    //
    // Its own box, and not a rule with everything after it floating on the
    // panel: the chain's buttons, the effect's knobs and the SLOT controls are
    // one section, and a divider said so in the wrong place — everything below
    // it, including the hint, read as part of the chain. The INSTRUMENT above
    // already draws itself as a box, and this is the same section drawn the
    // same way.
    let chain_area = Rect::new(
        inner.x,
        y,
        inner.width,
        (inner.y + inner.height).saturating_sub(y + 1),
    );
    if chain_area.height < 3 {
        return layout;
    }
    let chain_block = Block::default()
        .title(format!(" {} ", t("FX CHAIN")))
        .title_style(Style::default().fg(LABEL).add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_style(if fx_focused {
            Style::default().fg(SEL).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(RULE)
        })
        .style(bg);
    let fx_box = chain_block.inner(chain_area);
    f.render_widget(chain_block, chain_area);
    layout.fx_area = Some(chain_area);
    y = fx_box.y;

    // Chain buttons wrap onto further lines instead of running off the panel —
    // the same row as the arpeggiator's, from the same helper.
    let mut row = ButtonRow::new(fx_box, bg, y, 2);
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
        // A gated effect says so on its own button: what it is wired to is not
        // in any of its knobs, so an effect that goes quiet between kicks would
        // otherwise look like an effect that is broken.
        let mark = match entry.gate {
            Some(g) => format!("\u{2301}{} ", gate_source_mark(g.source)),
            None => String::new(),
        };
        let rect = row.button(f, format!(" {}:{} {mark}", i + 1, entry.label()), st);
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
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    " no FX yet \u{2014} press 'a' or click [+ ADD]",
                    Style::default().fg(Color::DarkGray),
                )))
                .style(bg),
                Rect::new(fx_box.x, y, fx_box.width, 1),
            );
        }
        return layout;
    };

    // The two wirings an effect can have that are not knobs: what opens it,
    // and — for the harmoniser — which keyboard it takes its chord from.
    // Buttons rather than only the `c` / `C` keys: a wiring nothing on the
    // panel mentions is a wiring nobody finds.
    {
        let on = |lit: bool| match lit {
            true => Style::default()
                .fg(Color::Black)
                .bg(ON_COLOUR)
                .add_modifier(Modifier::BOLD),
            false => btn_style,
        };
        let mut row = ButtonRow::new(fx_box, bg, y, 2);
        let rect = row.button(
            f,
            match entry.gate {
                Some(g) => format!(" {} {} ", t("GATE"), gate_source_mark(g.source)),
                None => format!(" {} \u{25CB} ", t("GATE")),
            },
            on(entry.gate.is_some()),
        );
        layout.buttons.push((RackButton::FxGate, rect));
        if entry.kind == crate::source::AudioFxKind::Harmonizer {
            let rect = row.button(
                f,
                match &entry.chord_port {
                    Some(p) => format!(" {} {} ", t("CHORD"), truncate(p, 10)),
                    None => format!(" {} {} ", t("CHORD"), t("ANY")),
                },
                on(entry.chord_port.is_some()),
            );
            layout.buttons.push((RackButton::FxChord, rect));
        }
        y = row.finish();
    }

    // ── Selected FX: the same knob box, from the same helper ──────────────
    let descs = entry.param_descs();
    // A hosted effect's parameters are the plugin's own names, and a plugin
    // with sections repeats them in every one — the same thing that made an
    // instrument's cells unreadable. Built-ins name their knobs in one word
    // and come back unchanged.
    let sections = match &entry.plugin {
        Some(p) => crate::source::param_sections(&p.params),
        None => Vec::new(),
    };
    let names: Vec<String> = descs
        .iter()
        .enumerate()
        .map(|(i, d)| match sections.get(i) {
            Some((_, short)) => short.clone(),
            None => d.name.to_string(),
        })
        .collect();
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
            fx_box,
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
                fx_box,
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

    // The deck draws in place of the knob grid — eight three-position knobs is
    // sixteen arcs reading "0.50", and a transport is read as buttons. The SLOT
    // box below still belongs to it: it is an effect in a chain like any other.
    let deck_drawn = match deck {
        Some(view) => {
            y = draw_loop_deck(f, fx_box, y, view, fx_focused, bg, btn_style, &mut layout);
            true
        }
        None => false,
    };

    let (rects, next) = if drawn || deck_drawn {
        (Vec::new(), y)
    } else {
        draw_knob_box(
            f,
            fx_box,
            y,
            &format!(
                "{}:{}{}",
                fx_slot + 1,
                entry.label(),
                // Same as the instrument's box: the cells dropped the words
                // every knob around them repeats, and the heading is where
                // they went.
                match sections.get(fx_param).and_then(|(g, _)| g.as_ref()) {
                    Some(g) => format!(" \u{00B7} {}", truncate(g, 18)),
                    None => String::new(),
                }
            ),
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
        y = draw_autotune_readout(f, fx_box, y, m, trace, bg);
    }

    // ── What the parametric EQ is doing to the signal ─────────────────────
    // Drawn at 48 kHz whatever the device runs at: bilinear warping only moves
    // the curve near Nyquist, and the panel does not get a knob's worth of
    // plumbing for a difference nobody can see at this many pixels per octave.
    if entry.plugin.is_none() && entry.kind == crate::source::AudioFxKind::ParamEq {
        y = draw_eq_curve(f, fx_box, y, &entry.params, 48_000, bg);
    }

    // ── Slot controls, in their own box, one blank line below the knobs ────
    if y + 2 < fx_box.y + fx_box.height {
        y += 1;
        let ctrl_h = 3u16;
        let ctrl_rect = Rect::new(fx_box.x, y, fx_box.width, ctrl_h);
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
    let hint = if focused && deck_drawn {
        "  k=box \u{2190}\u{2192}=channel \u{2191}\u{2193}=button enter=press \u{00B7} PLAY is also PAUSE \u{00B7} \u{2669} is choz's own click \u{00B7} [\u{00D7}] drops a channel \u{00B7} the pan and the level are grabbed, not walked"
    } else if focused {
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

/// What the panel needs to draw a deck. Read from the handle's atomics, which
/// the audio thread publishes every block — the same contract the meters have.
#[derive(Clone, Copy)]
pub struct LoopView<'a> {
    /// What the audio thread is publishing, when there is an audio thread.
    ///
    /// Optional on purpose: everything a strip **draws** comes from the
    /// effect's parameters, and only the transport lights, the input monitor
    /// and the deck's length come from here. A rack with no engine running
    /// still draws its channels rather than falling back to a grid of eight
    /// knobs called `T1`…`T8`.
    pub state: Option<&'a choz_ports::LoopState>,
    pub sample_rate: u32,
    /// Bytes this deck holds, and the ceiling every deck shares.
    pub held: usize,
    pub budget: usize,
    /// What each channel's take rounds to when it closes. From the effect's
    /// parameters rather than the atomics: a setting the project saves, not
    /// something the callback decides.
    pub quant: [&'static str; choz_ports::LOOP_TRACKS],
    /// The rest of what a strip decides, from the same parameters: out of the
    /// mix, only this one, where it sits, and how loud it plays.
    pub mute: [bool; choz_ports::LOOP_TRACKS],
    pub solo: [bool; choz_ports::LOOP_TRACKS],
    /// `-1..1`, the way the mixer says pan.
    pub pan: [f32; choz_ports::LOOP_TRACKS],
    /// `0..1`, linear.
    pub vol: [f32; choz_ports::LOOP_TRACKS],
    /// Whether choz's metronome is running. One state for the whole program,
    /// shown on every strip, because that is what it is.
    pub metro: bool,
    /// Strips the deck offers, and which page of them is on screen.
    pub chans: usize,
    pub page: usize,
    /// Where the keyboard is, when the deck has the arrows.
    pub cursor: (usize, LoopBtn),
}

/// The narrowest a channel strip is allowed to get, and the rows it needs with
/// and without the rules between its sections.
///
/// The strips **share the width**: as many as fit at `CH_MIN` are drawn, and
/// then they are widened to fill the row, so four channels on a wide panel are
/// four wide strips on one line rather than four narrow ones and a gap.
const CH_MIN: u16 = 18;
const CH_FULL: u16 = 9;
const CH_TIGHT: u16 = 7;

/// One channel's activity, drawn the way a desk draws it: the RMS as a filled
/// bar, the peak as a single cell riding on top of it.
///
/// Both are what that **channel** is doing — what it is taking in while it
/// records, what it is putting out while it plays — so four strips side by side
/// say which of them is the one making the noise.
fn level_bar(rms_db: f32, peak_db: f32, cells: usize) -> String {
    // -60 dB is the floor: under it a channel reads as silent, which is what a
    // player means by it.
    let frac = |db: f32| ((db + 60.0) / 60.0).clamp(0.0, 1.0);
    let fill = (frac(rms_db) * cells as f32).round() as usize;
    let mark = ((frac(peak_db) * cells as f32).round() as usize).min(cells.saturating_sub(1));
    (0..cells)
        .map(|i| match i {
            i if peak_db > -60.0 && i == mark => '\u{2588}',
            i if i < fill => '\u{2593}',
            _ => '\u{2591}',
        })
        .collect()
}

/// The colour a level is read at: red at the top, green where it is working,
/// dim where there is nothing.
fn level_colour(peak_db: f32) -> Color {
    match peak_db {
        d if d > -1.0 => Color::Rgb(210, 80, 76),
        d if d > -12.0 => ON_COLOUR,
        d if d > -60.0 => Color::Rgb(120, 160, 120),
        _ => LABEL,
    }
}

/// The same reading with no room for its name.
fn db_short(db: f32) -> String {
    match db.is_finite() {
        true => format!("{db:+.1}"),
        false => "-inf".to_string(),
    }
}

/// A linear peak as dB.
fn peak_db(peak: f32) -> f32 {
    match peak {
        p if p <= 1e-5 => f32::NEG_INFINITY,
        p => 20.0 * p.log10(),
    }
}

/// Draw the looper as channel strips, side by side.
///
/// In place of the knob grid, not beside it. Eight tracks of four-position
/// knobs is sixteen arcs saying "0.50", and a transport is read as buttons —
/// which is also what a player with both hands on a guitar can hit.
///
/// How many strips are drawn is the width's answer, not a setting: the deck
/// offers `v.chans` of them and the panel shows the ones that fit, with page
/// arrows when they do not all.
#[allow(clippy::too_many_arguments)]
fn draw_loop_deck(
    f: &mut Frame,
    inner: Rect,
    mut y: u16,
    v: LoopView<'_>,
    focused: bool,
    bg: Style,
    btn_style: Style,
    layout: &mut RackLayout,
) -> u16 {
    use choz_ports::{LoopTrackState, LOOP_TRACKS};

    let bottom = inner.y + inner.height;
    let lit = Style::default()
        .fg(Color::Black)
        .bg(ON_COLOUR)
        .add_modifier(Modifier::BOLD);
    // A strip needs its rules dropped before it needs to disappear: the rows
    // that carry controls are the ones worth the height.
    let rows = bottom.saturating_sub(y);
    let (ch_h, ruled) = match rows {
        r if r > CH_FULL => (CH_FULL, true),
        r if r > CH_TIGHT => (CH_TIGHT, false),
        _ => return y,
    };
    // At least one strip, however narrow the panel is — a deck that draws
    // nothing because the window shrank is worse than one that scrolls.
    let chans = v.chans.clamp(1, LOOP_TRACKS);
    let per_page = ((inner.width / CH_MIN).max(1) as usize).min(chans);
    let cell_w = inner.width / per_page as u16;
    // A strip needs its rules    // Every button on a strip is a symbol. Words were what made a strip's rows
    // depend on the language and on how wide the panel happened to be, and a
    // transport of five symbols is read faster than one of five words anyway.
    // What is left of `wide` is the two sliders, which are labels and not
    // buttons: they spell their name where there is room for it.
    let wide = cell_w >= (t("LEVEL").chars().count() as u16 + 18);
    let pages = chans.div_ceil(per_page);
    let page = v.page.min(pages - 1);
    layout.loop_pages = (page, pages);
    let first = page * per_page;
    let shown = per_page.min(chans - first);

    for i in 0..shown {
        let track = first + i;
        let x = inner.x + i as u16 * cell_w;
        let w = cell_w.min((inner.x + inner.width).saturating_sub(x));
        if w < 4 {
            break;
        }
        let state = v
            .state
            .map(|s| s.track(track))
            .unwrap_or(LoopTrackState::Idle);
        let here = focused && v.cursor.0 == track;
        let block = Block::default()
            .title(format!(" {} {} ", t("CHANNEL"), track + 1))
            .title_style(
                Style::default()
                    .fg(match state {
                        LoopTrackState::Idle => HEADER,
                        _ => ON_COLOUR,
                    })
                    .add_modifier(Modifier::BOLD),
            )
            .borders(Borders::ALL)
            .border_style(match here {
                true => Style::default().fg(SEL).add_modifier(Modifier::BOLD),
                false => Style::default().fg(ui_border()),
            })
            .style(bg);
        let area = Rect::new(x, y, w, ch_h);
        let box_inner = block.inner(area);
        f.render_widget(block, area);
        if box_inner.width == 0 || box_inner.height == 0 {
            continue;
        }

        // ── The close box, on the corner a window puts one ─────────────────
        // Drawn over the top border rather than inside the box: the strip has
        // its rows full already, and the corner is the one place a "throw this
        // away" reads without being reached for by accident.
        if w >= 6 {
            let del = Rect::new(area.x + w - 4, area.y, 3, 1);
            let on_del = here && v.cursor.1 == LoopBtn::Del;
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "[\u{00D7}]",
                    match on_del {
                        true => Style::default()
                            .fg(Color::Black)
                            .bg(SEL)
                            .add_modifier(Modifier::BOLD),
                        false => Style::default().fg(Color::Rgb(200, 110, 105)),
                    },
                )))
                .style(bg),
                del,
            );
            layout.loop_hits.push(((track, LoopBtn::Del), del));
        }

        // A button in its own colour: what it *is*, banked down while it is
        // off and lit while it is on — and the cursor still wins, because
        // where the keyboard is has to read differently from what is engaged.
        //
        // Colour is how a symbol says which button it is. `M` and `S` are the
        // same shape; white and amber are not.
        let paint = |btn: LoopBtn, on: bool, colour: Color, dim: Color| -> Style {
            match (here && v.cursor.1 == btn, on) {
                (true, _) => Style::default()
                    .fg(Color::Black)
                    .bg(SEL)
                    .add_modifier(Modifier::BOLD),
                (false, true) => Style::default()
                    .fg(Color::Black)
                    .bg(colour)
                    .add_modifier(Modifier::BOLD),
                (false, false) => Style::default()
                    .fg(colour)
                    .bg(dim)
                    .add_modifier(Modifier::BOLD),
            }
        };
        let mut hit = |f: &mut Frame, row: &mut ButtonRow, btn, text: String, style| {
            let rect = row.button(f, text, style);
            layout.loop_hits.push(((track, btn), rect));
        };
        let rule = |f: &mut Frame, ry: u16| {
            f.render_widget(
                Paragraph::new(Span::styled(
                    "\u{2500}".repeat(box_inner.width as usize),
                    Style::default().fg(ui_border()),
                ))
                .style(bg),
                Rect::new(box_inner.x, ry, box_inner.width, 1),
            );
        };

        let mut ry = box_inner.y;
        // ── What this channel is doing: RMS filled, peak riding on it ──────
        let (peak, rms) = v.state.map(|s| s.track_level(track)).unwrap_or((0.0, 0.0));
        let (peak, rms) = (peak_db(peak), peak_db(rms));
        let mut row = ButtonRow::new(box_inner, bg, ry, 0);
        let cells = match wide {
            true => 8,
            false => 4,
        };
        row.label(
            f,
            format!(" {} {:>5} ", level_bar(rms, peak, cells), db_short(peak)),
            Style::default().fg(level_colour(peak)),
        );
        row.finish();
        ry += 1;
        if ruled {
            rule(f, ry);
            ry += 1;
        }

        // ── The four switches, one row ─────────────────────────────────────
        // The metronome's own glyph from the bar above, because it is that
        // metronome; then what the take rounds to, out of the mix, and only
        // this one.
        let mut row = ButtonRow::new(box_inner, bg, ry, 0);
        hit(
            f,
            &mut row,
            LoopBtn::Metro,
            " \u{2669} ".to_string(),
            paint(LoopBtn::Metro, v.metro, AMBER, DIM_AMBER),
        );
        hit(
            f,
            &mut row,
            LoopBtn::Quant,
            " Q ".to_string(),
            paint(LoopBtn::Quant, v.quant[track] != "OFF", BLUE, DIM_BLUE),
        );
        hit(
            f,
            &mut row,
            LoopBtn::Mute,
            " M ".to_string(),
            paint(LoopBtn::Mute, v.mute[track], WHITE, DIM_WHITE),
        );
        hit(
            f,
            &mut row,
            LoopBtn::Solo,
            " S ".to_string(),
            paint(LoopBtn::Solo, v.solo[track], AMBER, DIM_AMBER),
        );
        row.finish();
        ry += 1;

        // ── How loud the take plays, and where it sits ─────────────────────
        // The level first and the pan under it, the way a strip is read: how
        // much, then where.
        let mut row = ButtonRow::new(box_inner, bg, ry, 0);
        let (label, prefix) = seq_slider_label(
            match wide {
                true => t("LEVEL"),
                false => "V",
            },
            v.vol[track],
        );
        let whole = row.draw(f, label, Style::default().fg(KNOB), 0);
        layout.loop_sliders.push((
            (track, LoopBtn::Vol),
            Rect::new(
                whole.x + prefix,
                whole.y,
                whole.width.saturating_sub(prefix).min(SEQ_BAR_W),
                1,
            ),
        ));
        row.finish();
        ry += 1;

        let mut row = ButtonRow::new(box_inner, bg, ry, 0);
        let pan_rect = row.draw(
            f,
            format!(" {} ", pan_slider(v.pan[track])),
            Style::default().fg(KNOB),
            1,
        );
        row.label(f, pan_label(v.pan[track]), Style::default().fg(LABEL));
        // The bar alone answers the mouse — the space either side of it is not
        // a value, and clicking it would be one.
        layout.loop_sliders.push((
            (track, LoopBtn::Pan),
            Rect::new(
                pan_rect.x + 1,
                pan_rect.y,
                pan_rect.width.saturating_sub(2),
                1,
            ),
        ));
        row.finish();
        ry += 1;
        if ruled {
            rule(f, ry);
            ry += 1;
        }

        // ── The transport: three buttons, one gesture each ─────────────────
        // Each in its own colour, on and off: red is what REC *is*, and green
        // is what PLAY is. A transport painted like the rest of the panel is
        // one nobody finds with a guitar in their hands.
        let mut row = ButtonRow::new(box_inner, bg, ry, 0);
        hit(
            f,
            &mut row,
            LoopBtn::Stop,
            " \u{25A0} ".to_string(),
            paint(LoopBtn::Stop, false, LABEL, Color::Rgb(38, 42, 52)),
        );
        // PLAY is also PAUSE: it shows which one the next press would be, and
        // amber says "held" where green says "running".
        let paused = state == LoopTrackState::Paused;
        let playing = state == LoopTrackState::Playing;
        hit(
            f,
            &mut row,
            LoopBtn::Play,
            match playing {
                true => " \u{23F8} ".to_string(),
                false => " \u{25B6} ".to_string(),
            },
            match paused {
                true => paint(LoopBtn::Play, true, AMBER, DIM_AMBER),
                false => paint(LoopBtn::Play, playing, ON_COLOUR, DIM_GREEN),
            },
        );
        let recording = state == LoopTrackState::Recording;
        hit(
            f,
            &mut row,
            LoopBtn::Rec,
            " \u{25CF} ".to_string(),
            paint(LoopBtn::Rec, recording, RED, DIM_RED),
        );
        row.finish();
    }
    y += ch_h;

    // ── The deck's own row: paging, one more strip, and what it holds ──────
    if y >= bottom {
        return y;
    }
    let mut row = ButtonRow::new(inner, bg, y, 0);
    let mut hit = |f: &mut Frame, row: &mut ButtonRow, btn, text: String, style| {
        let rect = row.button(f, text, style);
        layout.loop_deck.push((btn, rect));
    };
    if pages > 1 {
        hit(
            f,
            &mut row,
            LoopBtn::PagePrev,
            " \u{25C0} ".to_string(),
            btn_style,
        );
        row.label(
            f,
            format!(" {}/{} ", page + 1, pages),
            Style::default().fg(LABEL),
        );
        hit(
            f,
            &mut row,
            LoopBtn::PageNext,
            " \u{25B6} ".to_string(),
            btn_style,
        );
    }
    if chans < LOOP_TRACKS {
        hit(
            f,
            &mut row,
            LoopBtn::AddChan,
            " + ".to_string(),
            // Lit, not tinted: `+` in a slightly greener grey on a grey button
            // was a button nobody could see.
            lit,
        );
    }
    hit(
        f,
        &mut row,
        LoopBtn::Clear,
        format!(" {} ", t("CLEAR")),
        btn_style,
    );
    hit(
        f,
        &mut row,
        LoopBtn::Export,
        format!(" {} ", t("EXPORT")),
        btn_style,
    );
    // The length the deck froze and what it is holding: the two questions a
    // looper gets asked that are not a button.
    let frames = v.state.map(|s| s.frames()).unwrap_or(0);
    let secs = |n: usize| n as f32 / v.sample_rate.max(1) as f32;
    row.label(
        f,
        match frames {
            0 => match v.state.map(|s| s.recorded()).unwrap_or(0) {
                0 => format!(" {} ", t("EMPTY")),
                n => format!(" {:.1}s\u{2026} ", secs(n)),
            },
            n => format!(
                " {:.2}s \u{00B7} {:.0} MiB / {:.0} ",
                secs(n),
                v.held as f32 / (1 << 20) as f32,
                v.budget as f32 / (1 << 20) as f32,
            ),
        },
        Style::default().fg(LABEL),
    );
    row.finish();
    y + 1
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
    // Two columns a band is the floor: one for the track, one to tell it from
    // the next. Ten EQ bands were never near it; thirty-two harmonics are
    // exactly what it is for — packed side by side is the whole point of a
    // bank, and four columns each would fit eight of them.
    if bands == 0 || height < 7 || inner.width < bands as u16 * 2 {
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

    /// The deck draws a strip a channel, four of them unless asked for more,
    /// and pages rather than dropping the ones that do not fit.
    ///
    /// Every button a strip carries is in `loop_hits` keyed by the channel it
    /// belongs to — which is what the mouse hit-tests and what the keyboard
    /// walks, so a strip drawn with no hits is a strip nobody can press.
    #[test]
    fn the_looper_draws_a_strip_a_channel_and_pages_the_rest() {
        use ratatui::{backend::TestBackend, Terminal};

        // It reads the labels off the screen, so it cannot run while another
        // test has the interface in Spanish: "CHANNEL 4" is "CANAL 4" there.
        let _g = super::super::theme::ui_guard();

        /// Wide enough for the spelled-out tier in any language the table has.
        const CH_STRIP: u16 = 30;
        let state = choz_ports::LoopState::default();
        let view = |chans: usize, page: usize| LoopView {
            state: Some(&state),
            sample_rate: 48_000,
            held: 0,
            budget: 512 << 20,
            quant: ["OFF"; choz_ports::LOOP_TRACKS],
            mute: [false; choz_ports::LOOP_TRACKS],
            solo: [false; choz_ports::LOOP_TRACKS],
            pan: [0.0; choz_ports::LOOP_TRACKS],
            vol: [1.0; choz_ports::LOOP_TRACKS],
            metro: false,
            chans,
            page,
            cursor: (0, LoopBtn::Play),
        };
        let draw = |w: u16, chans: usize, page: usize| {
            let mut term = Terminal::new(TestBackend::new(w, 12)).unwrap();
            let mut layout = RackLayout::default();
            term.draw(|f| {
                let area = f.area();
                draw_loop_deck(
                    f,
                    area,
                    area.y,
                    view(chans, page),
                    true,
                    Style::default(),
                    Style::default(),
                    &mut layout,
                );
            })
            .unwrap();
            let screen = term
                .backend()
                .to_string()
                .lines()
                .map(|r| r.trim_matches('"').to_string())
                .collect::<Vec<_>>()
                .join("\n");
            (screen, layout)
        };

        // Wide enough for four strips: four strips, one page, no arrows.
        let (screen, layout) = draw(CH_STRIP * 4, 4, 0);
        for n in 1..=4 {
            assert!(
                screen.contains(&format!("CHANNEL {n}")),
                "channel {n} is missing:\n{screen}"
            );
        }
        assert_eq!(layout.loop_pages, (0, 1), "four in four fits on one page");
        // All four on the same line, filling the width — the bug this replaced
        // was four strips paged two at a time on a panel with room for four.
        let tops: Vec<u16> = (0..4)
            .map(|t| {
                layout
                    .loop_hits
                    .iter()
                    .find(|((c, b), _)| *c == t && *b == LoopBtn::Play)
                    .expect("every channel has a transport")
                    .1
                    .y
            })
            .collect();
        assert!(
            tops.windows(2).all(|w| w[0] == w[1]),
            "the strips are side by side, not stacked: {tops:?}"
        );
        assert!(
            !layout
                .loop_deck
                .iter()
                .any(|(b, _)| matches!(b, LoopBtn::PagePrev | LoopBtn::PageNext)),
            "nothing to page through, so no arrows"
        );
        assert!(
            layout.loop_deck.iter().any(|(b, _)| *b == LoopBtn::AddChan),
            "four of eight, so there is one more to offer"
        );
        // Every button of every strip answers the mouse.
        for track in 0..4 {
            for btn in LoopBtn::STRIP {
                assert!(
                    layout
                        .loop_hits
                        .iter()
                        .any(|((t, b), r)| *t == track && *b == btn && r.width > 0),
                    "channel {track} has no {btn:?} to press"
                );
            }
        }

        // Eight channels in the width of three: three at a time, three pages,
        // and the arrows to walk them.
        let (screen, layout) = draw(CH_MIN * 3, 8, 1);
        assert_eq!(layout.loop_pages, (1, 3));
        assert!(
            screen.contains("CHANNEL 4") && !screen.contains("CHANNEL 1"),
            "the second page starts at channel 4:\n{screen}"
        );
        for want in [LoopBtn::PagePrev, LoopBtn::PageNext] {
            assert!(
                layout.loop_deck.iter().any(|(b, _)| *b == want),
                "{want:?} is not on the deck row"
            );
        }

        // A full deck has nothing left to add, so it does not offer to.
        let (_, layout) = draw(CH_STRIP * 4, choz_ports::LOOP_TRACKS, 0);
        assert!(
            !layout.loop_deck.iter().any(|(b, _)| *b == LoopBtn::AddChan),
            "eight is the ceiling"
        );

        // A page that no longer exists lands on the last one rather than
        // drawing nothing.
        let (screen, layout) = draw(CH_STRIP * 4, 4, 9);
        assert_eq!(layout.loop_pages, (0, 1));
        assert!(screen.contains("CHANNEL 1"), "{screen}");
    }

    /// The strip's shape, row by row: the trim on top, the two settings that
    /// decide how a take starts and ends, then the transport and the level it
    /// comes back at.
    #[test]
    fn the_strip_looks_like_a_channel_strip() {
        use ratatui::{backend::TestBackend, Terminal};
        const CH_STRIP: u16 = 30;
        let state = choz_ports::LoopState::default();
        let mut term = Terminal::new(TestBackend::new(CH_STRIP * 2, 10)).unwrap();
        let mut layout = RackLayout::default();
        term.draw(|f| {
            let area = f.area();
            draw_loop_deck(
                f,
                area,
                area.y,
                LoopView {
                    state: Some(&state),
                    sample_rate: 48_000,
                    held: 0,
                    budget: 512 << 20,
                    quant: ["1 BAR"; choz_ports::LOOP_TRACKS],
                    mute: [false; choz_ports::LOOP_TRACKS],
                    solo: [false; choz_ports::LOOP_TRACKS],
                    pan: [0.0; choz_ports::LOOP_TRACKS],
                    vol: [1.0; choz_ports::LOOP_TRACKS],
                    metro: true,
                    chans: 2,
                    page: 0,
                    cursor: (0, LoopBtn::Play),
                },
                true,
                Style::default(),
                Style::default(),
                &mut layout,
            );
        })
        .unwrap();
        let rows: Vec<String> = term
            .backend()
            .to_string()
            .lines()
            .map(|r| r.trim_matches('"').to_string())
            .collect();
        let want = [
            "CHANNEL 1",
            "\u{2591}",
            "\u{2669}",
            "LEVEL",
            "L\u{2500}",
            "\u{25A0}",
        ];
        let mut at = 0;
        for label in want {
            let found = rows
                .iter()
                .skip(at)
                .position(|r| r.contains(label))
                .unwrap_or_else(|| {
                    panic!(
                        "{label} is not below the row before it:\n{}",
                        rows.join("\n")
                    )
                });
            at += found;
        }
        // Not one word inside a button: the four switches are one row of
        // symbols, and so is the transport.
        assert!(
            rows[3].contains("\u{2669}")
                && rows[3].contains(" Q ")
                && rows[3].contains(" M ")
                && rows[3].contains(" S "),
            "the four switches share a row:\n{}",
            rows.join("\n")
        );
        // The level, then the pan under it — how much, then where.
        assert!(
            rows[4].contains("LEVEL") && rows[5].contains("L\u{2500}"),
            "the pan sits under the level:\n{}",
            rows.join("\n")
        );
        // The transport is three buttons, on the last row of the strip.
        assert!(
            rows[7].contains('\u{25A0}')
                && rows[7].contains('\u{25B6}')
                && rows[7].contains('\u{25CF}'),
            "stop, play and rec are separate:\n{}",
            rows.join("\n")
        );
        assert!(
            !rows
                .iter()
                .any(|r| r.contains("STOP") || r.contains("MUTE")),
            "no words inside a button:\n{}",
            rows.join("\n")
        );
        // The close box is on the corner of every strip, and the mouse knows
        // where each one is.
        assert!(
            rows[0].matches("[\u{00D7}]").count() == 2,
            "{}",
            rows.join("\n")
        );
        assert_eq!(
            layout
                .loop_hits
                .iter()
                .filter(|((_, b), _)| *b == LoopBtn::Del)
                .count(),
            2,
            "one close box a strip"
        );
        // And both sliders answer the mouse, on both strips.
        assert_eq!(layout.loop_sliders.len(), 4, "a pan and a level a strip");
        assert!(
            !rows.iter().any(|r| r.contains("PLAY/REC")),
            "the combined button is gone:\n{}",
            rows.join("\n")
        );
        // The deck's row, under every strip.
        assert!(
            rows.iter().any(|r| r.contains("EXPORT") && r.contains('+')),
            "the deck row is missing:\n{}",
            rows.join("\n")
        );
    }
}
