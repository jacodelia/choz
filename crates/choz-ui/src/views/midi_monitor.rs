//! MIDI monitor panel — the last few messages that reached choz.
//!
//! Answers "is the keyboard actually talking to choz?" without leaving the app:
//! notes, pedals and wheels show up here the moment they arrive. Only real
//! input traffic (MIDI ports, OSC) passes through — the QWERTY piano drives the
//! engine directly and is deliberately not logged as MIDI.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use choz_engine::input::{InputEvent, InputSource};

use super::theme::{self, ACCENT, DIM, HEADER, OK, WARN};

/// Note names, sharps only — flats would need a key signature to choose.
const NOTE_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// Scientific pitch: MIDI 60 is C4, so octave = note/12 - 1.
pub fn note_name(note: u8) -> String {
    format!("{}{}", NOTE_NAMES[note as usize % 12], note as i16 / 12 - 1)
}

/// The common controllers, so the monitor says "SUSTAIN" instead of "CC 64"
/// when the user steps on a pedal.
fn cc_name(cc: u8) -> Option<&'static str> {
    Some(match cc {
        0 => "BANK MSB",
        32 => "BANK LSB",
        1 => "MOD WHEEL",
        7 => "VOLUME",
        11 => "EXPRESSION",
        64 => "SUSTAIN",
        66 => "SOSTENUTO",
        67 => "SOFT",
        _ => return None,
    })
}

/// Where the message came from, short enough to sit in a narrow panel.
fn source_label(source: InputSource, ports: &[String]) -> String {
    match source {
        InputSource::Midi(i) => ports
            .get(i)
            .map(|n| n.chars().take(14).collect())
            .unwrap_or_else(|| format!("port {i}")),
        InputSource::Osc => "OSC".to_string(),
        InputSource::Keyboard => "keyboard".to_string(),
    }
}

/// One log line: `<source>  <what>  <detail>`, coloured by message type.
fn line(event: &InputEvent, ports: &[String]) -> Line<'static> {
    let (source, what, detail, colour) = match event {
        InputEvent::Note(m) if m.on => (
            m.source,
            "NOTE ON".to_string(),
            format!("{:<4} vel {}", note_name(m.note), m.vel),
            OK,
        ),
        InputEvent::Note(m) => (m.source, "NOTE OFF".to_string(), note_name(m.note), DIM),
        // The clock has no port of its own in this log: it is the wire itself
        // talking, and there is one of it.
        InputEvent::Clock(c) => (
            InputSource::Osc,
            "CLOCK".to_string(),
            match c {
                choz_engine::input::ClockMsg::Start => "START".to_string(),
                choz_engine::input::ClockMsg::Continue => "CONTINUE".to_string(),
                choz_engine::input::ClockMsg::Stop => "STOP".to_string(),
                choz_engine::input::ClockMsg::Tempo(bpm) => format!("{bpm:.1} BPM"),
            },
            ACCENT,
        ),
        InputEvent::Cc(m) => (
            m.source,
            cc_name(m.cc)
                .map(str::to_string)
                .unwrap_or_else(|| format!("CC {}", m.cc)),
            m.value.to_string(),
            ACCENT,
        ),
        InputEvent::Program(m) => (
            m.source,
            "PROGRAM".to_string(),
            format!("{} bank {}", m.program, m.bank),
            OK,
        ),
        InputEvent::Bend(m) => (
            m.source,
            "PITCH BEND".to_string(),
            // Shown relative to the centre detent, which is how it reads on a wheel.
            format!("{:+}", m.value as i32 - 8192),
            WARN,
        ),
        // OSC control messages are remote UI actions, not performance data.
        InputEvent::Control(_) => (
            InputSource::Osc,
            "CONTROL".to_string(),
            String::new(),
            theme::HINT,
        ),
    };

    Line::from(vec![
        Span::styled(
            format!("{:<15}", source_label(source, ports)),
            Style::default().fg(DIM),
        ),
        Span::styled(
            format!("{what:<11}"),
            Style::default().fg(colour).add_modifier(Modifier::BOLD),
        ),
        Span::styled(detail, Style::default().fg(theme::text())),
    ])
}

/// What the monitor is showing: the messages, or the sound they made.
///
/// seqterm has these as sidebar tabs; here they share the MIDI panel, because
/// "did the note arrive" and "did anything come out" are the same question asked
/// twice, and answering the second one needs no MIDI at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MonitorTab {
    #[default]
    Midi,
    /// A piano keyboard lit by what is arriving.
    Keys,
    /// The same notes as bars falling towards that keyboard.
    Roll,
    /// The output's shape, as a travelling window.
    Wave,
    /// How loud it is: peak and RMS, held and decaying.
    Activity,
    /// What frequencies are in it: an FFT, logarithmic, with peak hold.
    Spectrum,
}

impl MonitorTab {
    pub const ALL: [MonitorTab; 6] = [
        MonitorTab::Midi,
        MonitorTab::Keys,
        MonitorTab::Roll,
        MonitorTab::Wave,
        MonitorTab::Spectrum,
        MonitorTab::Activity,
    ];

    pub fn label(self) -> &'static str {
        match self {
            MonitorTab::Midi => "MIDI",
            MonitorTab::Keys => "KEYS",
            MonitorTab::Roll => "ROLL",
            MonitorTab::Wave => "WAVE",
            MonitorTab::Activity => "ACTIVITY",
            MonitorTab::Spectrum => "SPEC",
        }
    }

    /// Whether this tab draws the keyboard, and so answers the colour key.
    pub fn is_keyboard(self) -> bool {
        matches!(self, MonitorTab::Keys | MonitorTab::Roll)
    }

    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|t| *t == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
}

/// What decides the colour of a lit key.
///
/// Three questions a player actually asks, one mode each: *which channel is
/// this* (MULTI, where a channel is a tab), *which instrument is sounding*
/// (two ports on one channel), and *how hard am I playing*.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum KeyColor {
    #[default]
    Channel,
    Instrument,
    Velocity,
}

impl KeyColor {
    pub const ALL: [KeyColor; 3] = [KeyColor::Channel, KeyColor::Instrument, KeyColor::Velocity];

    pub fn label(self) -> &'static str {
        match self {
            KeyColor::Channel => "CHANNEL",
            KeyColor::Instrument => "INSTRUMENT",
            KeyColor::Velocity => "VELOCITY",
        }
    }

    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|c| *c == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
}

/// A note currently held down.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeyLit {
    pub channel: u8,
    pub vel: u8,
    /// Which rack tab it was routed to, when it was routed anywhere.
    pub slot: Option<usize>,
}

/// One note in the falling-notes view: when it started, and when it stopped.
#[derive(Debug, Clone, Copy)]
struct RollNote {
    note: u8,
    channel: u8,
    slot: Option<usize>,
    start: Instant,
    /// `None` while the key is still down.
    end: Option<Instant>,
}

/// How far back the roll looks. Longer needs a taller panel to say anything.
const ROLL_WINDOW: Duration = Duration::from_secs(4);
/// Fixed budget: the oldest note is overwritten rather than the buffer growing.
const ROLL_MAX: usize = 256;
/// How many controllers are shown under the keyboard.
const CC_SHOWN: usize = 3;

/// What the keyboard tabs draw: which keys are down, and what the wheels and
/// pedals are doing.
///
/// Deliberately **not** `App.sounding`: that one indexes slots and exists to
/// send note-offs where their note-on went. Mixing the two is how notes get
/// stuck.
///
/// Lives on the UI thread and is fed from the same drained MIDI the monitor log
/// gets, so the audio callback never touches it.
#[derive(Debug)]
pub struct KeyboardState {
    keys: [Option<KeyLit>; 128],
    roll: VecDeque<RollNote>,
    /// Last few controllers seen, newest first: `(cc, value)`.
    ccs: Vec<(u8, u8)>,
    bend: u16,
    modulation: u8,
    /// The notes made *inside* choz rather than played into it, so each can be
    /// put out when it changes. Kept apart from `keys` because nothing sends
    /// their note-offs. Indexed by [`Converted`].
    converted: [Option<u8>; Converted::COUNT],
}

/// Where a note choz made itself came from.
///
/// Both of these are decided in the audio callback and never travel as MIDI, so
/// this monitor is the only place they can be watched — and one of them is the
/// note an effect is *correcting towards*, which is the number that says
/// whether AutoTune is aiming where the singer meant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Converted {
    /// `A→M`: the tab's audio input, played as notes.
    PitchToMidi,
    /// AutoTune's target note — what it is pulling the voice onto.
    AutoTune,
}

impl Converted {
    pub const COUNT: usize = 2;

    fn index(self) -> usize {
        match self {
            Converted::PitchToMidi => 0,
            Converted::AutoTune => 1,
        }
    }
}

impl Default for KeyboardState {
    fn default() -> Self {
        Self {
            keys: [None; 128],
            roll: VecDeque::with_capacity(ROLL_MAX),
            ccs: Vec::with_capacity(CC_SHOWN),
            bend: 8192,
            converted: [None; Converted::COUNT],
            modulation: 0,
        }
    }
}

impl KeyboardState {
    /// Feed one arrived message. `slot` is where the routing sent it, when it
    /// was sent anywhere — that is what the `INSTRUMENT` colour mode reads.
    /// Light (or put out) a note choz made itself: `A→M`'s, or the one
    /// AutoTune is correcting towards.
    ///
    /// Those notes are made in the audio callback and never travel as MIDI, so
    /// nothing else here would ever see them — and a converter you cannot watch
    /// is one you can only trust or not. Called once per redraw with whatever
    /// each source is on now, so they light up and go out on their own.
    pub fn feed_converted(&mut self, source: Converted, note: Option<u8>, slot: Option<usize>) {
        let i = source.index();
        if self.converted[i] == note {
            return;
        }
        if let Some(old) = self.converted[i].take() {
            self.release(old);
        }
        self.converted[i] = note;
        if let Some(n) = note.filter(|n| (*n as usize) < 128) {
            self.keys[n as usize] = Some(KeyLit {
                channel: 0,
                vel: 100,
                slot,
            });
            if self.roll.len() == ROLL_MAX {
                self.roll.pop_front();
            }
            self.roll.push_back(RollNote {
                note: n,
                channel: 0,
                slot,
                start: Instant::now(),
                end: None,
            });
        }
    }

    /// The notes held on one MIDI channel, low to high.
    ///
    /// What the harmoniser's MIDI input reads: the chord under the hand right
    /// now, not the stream of events that built it. `slot` narrows it to the
    /// notes that were routed to one tab, which is what "the active tab is the
    /// reference" means.
    ///
    /// **`channel` is 1..16**, the way a musician and the panel say it; on the
    /// wire and in [`KeyLit`] it is 0-based.
    pub fn held_on_channel(&self, channel: u8, slot: Option<usize>) -> Vec<u8> {
        let wire = channel.saturating_sub(1);
        self.keys
            .iter()
            .enumerate()
            .filter_map(|(note, lit)| {
                let lit = lit.as_ref()?;
                let same_tab = slot.is_none() || lit.slot.is_none() || lit.slot == slot;
                (lit.channel == wire && same_tab).then_some(note as u8)
            })
            .collect()
    }

    /// Which notes are lit right now. For a test, and for anything that wants
    /// to know what the keyboard is showing without redrawing it.
    #[cfg(test)]
    pub(crate) fn drawn_keys(&self) -> Vec<u8> {
        self.keys
            .iter()
            .enumerate()
            .filter_map(|(i, k)| k.map(|_| i as u8))
            .collect()
    }

    /// Put a note out: the key and whatever bar is still falling for it.
    fn release(&mut self, note: u8) {
        if (note as usize) < 128 {
            self.keys[note as usize] = None;
        }
        for r in self.roll.iter_mut().rev() {
            if r.note == note && r.end.is_none() {
                r.end = Some(Instant::now());
                break;
            }
        }
    }

    pub fn feed(&mut self, event: &InputEvent, slot: Option<usize>) {
        match event {
            // **A note-on with velocity 0 is a note-off.** Plenty of hardware
            // says it that way, and taking it literally leaves keys lit
            // forever.
            InputEvent::Note(m) if m.on && m.vel > 0 => {
                let note = m.note as usize;
                if note >= 128 {
                    return;
                }
                self.keys[note] = Some(KeyLit {
                    channel: m.channel,
                    vel: m.vel,
                    slot,
                });
                if self.roll.len() == ROLL_MAX {
                    self.roll.pop_front();
                }
                self.roll.push_back(RollNote {
                    note: m.note,
                    channel: m.channel,
                    slot,
                    start: Instant::now(),
                    end: None,
                });
            }
            InputEvent::Note(m) => {
                if (m.note as usize) < 128 {
                    self.keys[m.note as usize] = None;
                }
                // Close the newest open bar for that note.
                if let Some(r) = self
                    .roll
                    .iter_mut()
                    .rev()
                    .find(|r| r.note == m.note && r.end.is_none())
                {
                    r.end = Some(Instant::now());
                }
            }
            InputEvent::Cc(m) => {
                if m.cc == 1 {
                    self.modulation = m.value;
                }
                self.ccs.retain(|(cc, _)| *cc != m.cc);
                self.ccs.insert(0, (m.cc, m.value));
                self.ccs.truncate(CC_SHOWN);
            }
            InputEvent::Bend(m) => self.bend = m.value,
            InputEvent::Program(_) | InputEvent::Control(_) | InputEvent::Clock(_) => {}
        }
    }

    /// Everything up. Called by `PANIC` — the same button that sends the real
    /// note-offs — so a keyboard that got out of step with the rack is one
    /// keypress from telling the truth again.
    ///
    /// There is deliberately **no timeout**: a pad held for a minute is a held
    /// note, not a stuck one, and a visualizer that drops it is lying about the
    /// easy case to be clever about the rare one.
    pub fn clear(&mut self) {
        self.keys = [None; 128];
        self.roll.clear();
        self.bend = 8192;
        self.modulation = 0;
    }

    /// Which keys are down, for tests and for the roll's colouring.
    pub fn lit(&self, note: u8) -> Option<KeyLit> {
        self.keys.get(note as usize).copied().flatten()
    }

    /// The lowest and highest notes held, if any.
    fn sounding_range(&self) -> Option<(u8, u8)> {
        let mut range: Option<(u8, u8)> = None;
        for (n, k) in self.keys.iter().enumerate() {
            if k.is_some() {
                let n = n as u8;
                range = Some(match range {
                    None => (n, n),
                    Some((lo, hi)) => (lo.min(n), hi.max(n)),
                });
            }
        }
        range
    }
}

/// Default window: the range `pitch.rs` tracks, which also covers a 61-key
/// controller. Anything outside scrolls the view rather than being dropped.
const DEFAULT_LO: u8 = 36; // C2
const DEFAULT_HI: u8 = 96; // C7

fn is_black(note: u8) -> bool {
    matches!(note % 12, 1 | 3 | 6 | 8 | 10)
}

/// The colour a lit key gets, by mode.
///
/// Channels and slots are hues off one wheel, so sixteen of them stay apart
/// without a hand-written palette; velocity is the theme's own text colour
/// scaled, so it reads as "the same colour, harder".
fn key_colour(mode: KeyColor, key: &KeyLit) -> Color {
    match mode {
        KeyColor::Channel => hue_of(key.channel as u32),
        KeyColor::Instrument => match key.slot {
            Some(s) => hue_of(s as u32 + 3),
            // Not routed anywhere: it arrived, but nothing is playing it.
            None => DIM,
        },
        KeyColor::Velocity => {
            let (r, g, b) = theme::rgb_of(theme::text());
            let k = 0.25 + 0.75 * (key.vel as f32 / 127.0);
            Color::Rgb(
                (r as f32 * k) as u8,
                (g as f32 * k) as u8,
                (b as f32 * k) as u8,
            )
        }
    }
}

fn hue_of(n: u32) -> Color {
    // 16 steps around the wheel, offset so channel 1 is not pure red (which
    // reads as an error everywhere else in the UI).
    let (r, g, b) = crate::logo::hsv_to_rgb((n % 16) as f32 * 22.5 + 30.0, 0.75, 0.95);
    Color::Rgb(r, g, b)
}

/// Which note each column of the keyboard belongs to.
///
/// Two columns per white key, and the black key sits on the **second** column
/// of the white key below it — which is where it sits on a real keyboard, and
/// is what makes the 2-3 grouping readable without drawing any borders.
fn key_columns(lo: u8, hi: u8, width: usize) -> Vec<(Option<u8>, u8)> {
    let mut cols: Vec<(Option<u8>, u8)> = Vec::with_capacity(width);
    let mut note = lo;
    while note <= hi && cols.len() < width {
        if is_black(note) {
            note += 1;
            continue;
        }
        let black = (note < hi && is_black(note + 1)).then_some(note + 1);
        cols.push((None, note));
        if cols.len() < width {
            cols.push((black, note));
        }
        note += 1;
    }
    cols
}

/// The range to draw: the default window, widened to whatever is sounding
/// outside it, and clamped to what fits.
fn visible_range(state: &KeyboardState, width: usize) -> (u8, u8) {
    let (mut lo, mut hi) = (DEFAULT_LO, DEFAULT_HI);
    if let Some((slo, shi)) = state.sounding_range() {
        lo = lo.min(slo);
        hi = hi.max(shi);
    }
    // Two columns per white key; drop octaves from the top until it fits.
    let whites = |lo: u8, hi: u8| (lo..=hi).filter(|n| !is_black(*n)).count();
    while whites(lo, hi) * 2 > width && hi > lo + 12 {
        hi -= 12;
    }
    while whites(lo, hi) * 2 > width && lo + 12 < hi {
        lo += 12;
    }
    (lo, hi)
}

/// The piano keyboard, lit by what is arriving.
fn draw_keys(f: &mut Frame, area: Rect, state: &KeyboardState, mode: KeyColor) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let mut lines = keyboard_lines(state, mode, area.width as usize);
    // Below the keys: the wheels and the last few controllers. They are what
    // a player checks after "did the note arrive" — and CCs never light a key.
    if area.height as usize > lines.len() {
        lines.push(controller_line(state));
    }
    if area.height as usize > lines.len() {
        lines.push(Line::from(Span::styled(
            format!("  colour: {}  [C]", mode.label()),
            Style::default().fg(DIM),
        )));
    }
    f.render_widget(
        Paragraph::new(lines).style(super::theme::panel_style()),
        area,
    );
}

/// The keyboard itself: black keys on top, white key bodies below, and the C
/// octave labels under them when there is room.
fn keyboard_lines(state: &KeyboardState, mode: KeyColor, width: usize) -> Vec<Line<'static>> {
    let (lo, hi) = visible_range(state, width);
    let cols = key_columns(lo, hi, width);

    let mut blacks: Vec<Span> = Vec::with_capacity(cols.len());
    let mut whites: Vec<Span> = Vec::with_capacity(cols.len());
    let mut labels = String::new();
    for &(black, white) in &cols {
        match black {
            Some(n) => {
                let (ch, colour) = match state.lit(n) {
                    Some(k) => ('\u{2584}', key_colour(mode, &k)),
                    None => ('\u{2584}', Color::Rgb(40, 44, 52)),
                };
                blacks.push(Span::styled(ch.to_string(), Style::default().fg(colour)));
            }
            None => blacks.push(Span::raw(" ")),
        }
        let colour = match state.lit(white) {
            Some(k) => key_colour(mode, &k),
            None => Color::Rgb(150, 158, 172),
        };
        whites.push(Span::styled(
            "\u{2588}".to_string(),
            Style::default().fg(colour),
        ));
        // One label per C, written across the two columns of that key.
        if white % 12 == 0 {
            labels.push_str(&note_name(white));
        } else if labels.len() < whites.len() {
            labels.push(' ');
        }
    }
    vec![
        Line::from(blacks),
        Line::from(whites),
        Line::from(Span::styled(labels, Style::default().fg(DIM))),
    ]
}

/// Wheels first, then the last controllers seen — the two that get looked at
/// while playing are bend and modulation, so they are always drawn.
fn controller_line(state: &KeyboardState) -> Line<'static> {
    let bar = |v: f32| {
        let filled = (v.clamp(0.0, 1.0) * 6.0).round() as usize;
        format!(
            "{}{}",
            "\u{2593}".repeat(filled),
            "\u{2591}".repeat(6 - filled)
        )
    };
    let bend = state.bend as i32 - 8192;
    let mut spans = vec![
        Span::styled("  BEND ", Style::default().fg(HEADER)),
        Span::styled(
            bar((bend as f32 / 8192.0 + 1.0) / 2.0),
            Style::default().fg(if bend == 0 { DIM } else { WARN }),
        ),
        Span::styled(format!(" {bend:+6}  "), Style::default().fg(theme::text())),
        Span::styled("MOD ", Style::default().fg(HEADER)),
        Span::styled(
            bar(state.modulation as f32 / 127.0),
            Style::default().fg(if state.modulation == 0 { DIM } else { ACCENT }),
        ),
        Span::styled(
            format!(" {:>3}  ", state.modulation),
            Style::default().fg(theme::text()),
        ),
    ];
    for (cc, value) in state.ccs.iter().filter(|(cc, _)| *cc != 1) {
        spans.push(Span::styled(
            format!(
                "{} {}  ",
                cc_name(*cc)
                    .map(str::to_string)
                    .unwrap_or(format!("CC {cc}")),
                value
            ),
            Style::default().fg(DIM),
        ));
    }
    Line::from(spans)
}

/// Falling notes: time runs down the panel, newest at the top, landing on the
/// keyboard drawn at the bottom.
fn draw_roll(f: &mut Frame, area: Rect, state: &KeyboardState, mode: KeyColor) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let width = area.width as usize;
    let keyboard = keyboard_lines(state, mode, width);
    let roll_rows = (area.height as usize).saturating_sub(keyboard.len());
    let (lo, hi) = visible_range(state, width);
    let cols = key_columns(lo, hi, width);

    let now = Instant::now();
    let mut lines: Vec<Line> = Vec::with_capacity(area.height as usize);
    for row in 0..roll_rows {
        // Row 0 is the oldest edge of the window, the last row the present, so
        // a held note grows downward towards its key.
        let age = ROLL_WINDOW.mul_f32(1.0 - (row as f32 + 0.5) / roll_rows.max(1) as f32);
        let spans: Vec<Span> = cols
            .iter()
            .map(|&(black, white)| {
                let note = black.unwrap_or(white);
                let hit = state.roll.iter().rev().find(|r| {
                    r.note == note
                        && now.saturating_duration_since(r.start) >= age
                        && r.end
                            .is_none_or(|e| now.saturating_duration_since(e) <= age)
                });
                match hit {
                    Some(r) => Span::styled(
                        "\u{2588}".to_string(),
                        Style::default().fg(key_colour(
                            mode,
                            &KeyLit {
                                channel: r.channel,
                                vel: 100,
                                slot: r.slot,
                            },
                        )),
                    ),
                    None => Span::raw(" "),
                }
            })
            .collect();
        lines.push(Line::from(spans));
    }
    lines.extend(keyboard);
    f.render_widget(
        Paragraph::new(lines).style(super::theme::panel_style()),
        area,
    );
}

/// Draw the monitor. `events` is oldest-first; the newest ones are shown at the
/// bottom, so the eye follows new arrivals downward like a terminal log.
#[allow(clippy::too_many_arguments)]
pub fn draw_midi_monitor(
    f: &mut Frame,
    area: Rect,
    events: &[InputEvent],
    ports: &[String],
    tab: MonitorTab,
    keyboard: &KeyboardState,
    key_colour_mode: KeyColor,
    // The analyser keeps its peak hold between frames, so it is owned by the
    // application and lent here rather than built per redraw.
    spectrum: &crate::spectrum::Spectrum,
) -> Vec<(MonitorTab, Rect)> {
    let block = Block::default()
        .title(" MIDI IN ")
        .title_style(Style::default().fg(HEADER).add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::border()))
        .style(super::theme::panel_style());

    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 {
        return Vec::new();
    }

    // The tab strip takes the first row; the rest is whichever tab is showing.
    let mut rects = Vec::new();
    let mut spans: Vec<Span> = Vec::new();
    let mut x = inner.x;
    for t in MonitorTab::ALL {
        let text = format!(" {} ", t.label());
        let w = text.chars().count() as u16;
        rects.push((t, Rect::new(x, inner.y, w, 1)));
        let style = if t == tab {
            Style::default()
                .fg(ratatui::style::Color::Black)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(HEADER)
        };
        spans.push(Span::styled(text, style));
        spans.push(Span::styled("\u{2502}", Style::default().fg(DIM)));
        x += w + 1;
    }
    f.render_widget(
        Paragraph::new(Line::from(spans)).style(super::theme::panel_style()),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    let inner = Rect::new(
        inner.x,
        inner.y + 1,
        inner.width,
        inner.height.saturating_sub(1),
    );
    if inner.height == 0 {
        return rects;
    }

    match tab {
        MonitorTab::Keys => {
            draw_keys(f, inner, keyboard, key_colour_mode);
            return rects;
        }
        MonitorTab::Roll => {
            draw_roll(f, inner, keyboard, key_colour_mode);
            return rects;
        }
        MonitorTab::Wave => {
            draw_wave(f, inner);
            return rects;
        }
        MonitorTab::Activity => {
            draw_activity(f, inner);
            return rects;
        }
        MonitorTab::Spectrum => {
            draw_spectrum(f, inner, spectrum);
            return rects;
        }
        MonitorTab::Midi => {}
    }

    let rows = inner.height as usize;
    let lines: Vec<Line> = if events.is_empty() {
        vec![Line::from(Span::styled(
            "waiting for MIDI…",
            Style::default().fg(DIM).add_modifier(Modifier::ITALIC),
        ))]
    } else {
        events[events.len().saturating_sub(rows)..]
            .iter()
            .map(|e| line(e, ports))
            .collect()
    };

    f.render_widget(
        Paragraph::new(lines).style(super::theme::panel_style()),
        inner,
    );
    rects
}

/// The output's shape: a window of the mixed signal, oldest on the left.
///
/// Half-blocks, so one row of cells carries two rows of resolution — the same
/// trick the wallpaper uses, and the reason this reads as a wave rather than as
/// a bar chart.
fn draw_wave(f: &mut Frame, area: Rect) {
    let wave = choz_engine::meter::meter().wave();
    let rows = area.height as usize;
    let cols = area.width as usize;
    if rows == 0 || cols == 0 {
        return;
    }
    // Auto-gain so a quiet signal is still a picture, with a floor so silence
    // does not become noise magnified to full scale.
    let peak = wave.iter().fold(0.02f32, |m, s| m.max(s.abs()));
    let mid = (rows * 2) as f32 / 2.0;

    let mut lines: Vec<Line> = Vec::with_capacity(rows);
    for row in 0..rows {
        let spans: Vec<Span> = (0..cols)
            .map(|col| {
                let s = wave[col * wave.len() / cols.max(1)] / peak;
                let y = mid - s * (mid - 1.0);
                // The two half-cells this character covers.
                let (top, bottom) = ((row * 2) as f32, (row * 2 + 1) as f32);
                let hit = |cell: f32| (y - cell).abs() < 1.0;
                let ch = match (hit(top), hit(bottom)) {
                    (true, true) => '\u{2588}',
                    (true, false) => '\u{2580}',
                    (false, true) => '\u{2584}',
                    // The centre line, so a silent signal is still a signal.
                    _ if (row * 2 + 1) as f32 == mid.floor() => '\u{2500}',
                    _ => ' ',
                };
                let colour = if ch == '\u{2500}' { DIM } else { ACCENT };
                Span::styled(ch.to_string(), Style::default().fg(colour))
            })
            .collect();
        lines.push(Line::from(spans));
    }
    f.render_widget(
        Paragraph::new(lines).style(super::theme::panel_style()),
        area,
    );
}

/// The spectrum: a column per terminal cell, logarithmic in frequency, with
/// the peak hold sitting above each bar.
///
/// Half-blocks again, so a panel eight rows tall carries sixteen steps of
/// level — which is the difference between a bar chart and a curve. The bottom
/// row is the frequency axis: without `100 / 1k / 10k` written on it, a
/// logarithmic scale is a picture of nothing in particular.
fn draw_spectrum(f: &mut Frame, area: Rect, spectrum: &crate::spectrum::Spectrum) {
    let (cols, rows) = (area.width as usize, area.height as usize);
    if cols == 0 || rows < 2 {
        return;
    }
    // One row goes to the axis; the rest is the display.
    let plot_rows = rows - 1;
    let steps = plot_rows * 2;
    let columns = spectrum.columns(cols);

    let mut lines: Vec<Line> = Vec::with_capacity(plot_rows);
    for row in 0..plot_rows {
        let spans: Vec<Span> = columns
            .iter()
            .map(|(level, peak)| {
                let filled = (level * steps as f32).round() as usize;
                let peak_step = (peak * steps as f32).round() as usize;
                // Half-cells of this row, counted from the bottom.
                let bottom = (plot_rows - row - 1) * 2;
                let top = bottom + 1;
                let lit = |cell: usize| cell < filled;
                let is_peak = |cell: usize| peak_step > 0 && cell + 1 == peak_step;
                // The held peak wins the cell: it is the reading that is
                // hardest to catch and the one worth drawing over the bar.
                if is_peak(top) || is_peak(bottom) {
                    let ch = if is_peak(top) && is_peak(bottom) {
                        '\u{2588}'
                    } else if is_peak(top) {
                        '\u{2580}'
                    } else {
                        '\u{2584}'
                    };
                    return Span::styled(ch.to_string(), Style::default().fg(HEADER));
                }
                let ch = match (lit(top), lit(bottom)) {
                    (true, true) => '\u{2588}',
                    (false, true) => '\u{2584}',
                    // A bar cannot have a lit top and a dark bottom.
                    _ => ' ',
                };
                Span::styled(ch.to_string(), Style::default().fg(ACCENT))
            })
            .collect();
        lines.push(Line::from(spans));
    }

    // The axis: a tick under each decade, labelled where the label fits.
    let mut axis = vec![' '; cols];
    let mut labels: Vec<(usize, &str)> = Vec::new();
    for (hz, text) in [(100.0f32, "100"), (1000.0, "1k"), (10_000.0, "10k")] {
        if let Some(col) = spectrum.marker_col(hz, cols) {
            axis[col] = '\u{2534}';
            labels.push((col, text));
        }
    }
    let mut axis_line: String = axis.into_iter().collect();
    for (col, text) in labels {
        let start = col
            .saturating_sub(text.len() / 2)
            .min(cols.saturating_sub(text.len()));
        // Written over the ticks, which is why the ticks are drawn first: the
        // label is the tick once there is room for it.
        axis_line.replace_range(char_range(&axis_line, start, text.chars().count()), text);
    }
    lines.push(Line::from(Span::styled(
        axis_line,
        Style::default().fg(DIM),
    )));

    f.render_widget(
        Paragraph::new(lines).style(super::theme::panel_style()),
        area,
    );
}

/// Byte range of `len` characters starting at character `start`.
fn char_range(s: &str, start: usize, len: usize) -> std::ops::Range<usize> {
    let mut it = s
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(s.len()));
    let from = it.clone().nth(start).unwrap_or(s.len());
    let to = it.nth(start + len).unwrap_or(s.len());
    from..to.max(from)
}

/// How loud it is: peak and RMS as two bars, with the numbers in dB.
fn draw_activity(f: &mut Frame, area: Rect) {
    let m = choz_engine::meter::meter();
    let (peak, rms) = (m.peak(), m.rms());
    // Six columns for the label, ten for the reading, and the bar gets the rest
    // — the first version gave the bar too much and pushed the dB off the edge.
    let width = area.width.saturating_sub(17) as usize;
    let bar = |v: f32| -> String {
        // dBFS over 60 dB, because a linear meter is all top and no bottom.
        let db = if v > 1e-6 { 20.0 * v.log10() } else { -60.0 };
        let filled = (((db + 60.0) / 60.0).clamp(0.0, 1.0) * width as f32).round() as usize;
        format!(
            "{}{}",
            "\u{2588}".repeat(filled),
            "\u{2591}".repeat(width.saturating_sub(filled))
        )
    };
    let db_text = |v: f32| -> String {
        // Always in dB, silence included: "-inf dB" is a reading, "-inf" alone
        // looks like a missing unit.
        if v > 1e-6 {
            format!("{:>6.1} dB", 20.0 * v.log10())
        } else {
            "  -inf dB".to_string()
        }
    };
    let colour = |v: f32| {
        if v >= 0.99 {
            WARN
        } else if v > 0.7 {
            OK
        } else {
            ACCENT
        }
    };

    let lines = vec![
        Line::from(vec![
            Span::styled(" PEAK ", Style::default().fg(HEADER)),
            Span::styled(bar(peak), Style::default().fg(colour(peak))),
            Span::styled(db_text(peak), Style::default().fg(theme::text())),
        ]),
        Line::from(vec![
            Span::styled(" RMS  ", Style::default().fg(HEADER)),
            Span::styled(bar(rms), Style::default().fg(colour(rms))),
            Span::styled(db_text(rms), Style::default().fg(theme::text())),
        ]),
        Line::from(Span::styled(
            if peak >= 0.99 { "  CLIPPING" } else { "" },
            Style::default().fg(WARN).add_modifier(Modifier::BOLD),
        )),
    ];
    f.render_widget(
        Paragraph::new(lines).style(super::theme::panel_style()),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use choz_engine::input::{BendMsg, CcMsg, NoteMsg};

    #[test]
    fn note_names_follow_scientific_pitch() {
        assert_eq!(note_name(60), "C4", "middle C");
        assert_eq!(note_name(69), "A4", "concert A");
        assert_eq!(note_name(0), "C-1", "lowest MIDI note");
        assert_eq!(note_name(127), "G9", "highest MIDI note");
        assert_eq!(note_name(61), "C#4");
    }

    /// The panel is only useful if a glance tells you *which* pedal moved.
    #[test]
    fn lines_name_the_pedals_and_centre_the_bend() {
        let ports = vec!["Keystation Pro 88".to_string()];
        let src = InputSource::Midi(0);
        let text = |e: InputEvent| {
            line(&e, &ports)
                .spans
                .iter()
                .map(|s| s.content.to_string())
                .collect::<String>()
        };

        let sustain = text(InputEvent::Cc(CcMsg {
            source: src,
            channel: 0,
            cc: 64,
            value: 127,
        }));
        assert!(sustain.contains("SUSTAIN"), "got {sustain:?}");
        assert!(sustain.contains("Keystation"), "port is named: {sustain:?}");

        let unknown = text(InputEvent::Cc(CcMsg {
            source: src,
            channel: 0,
            cc: 23,
            value: 5,
        }));
        assert!(
            unknown.contains("CC 23"),
            "unnamed controllers fall back to a number"
        );

        // A wheel at rest reads 0, not 8192.
        let centre = text(InputEvent::Bend(BendMsg {
            source: src,
            value: 8192,
        }));
        assert!(centre.contains("+0"), "got {centre:?}");
        let down = text(InputEvent::Bend(BendMsg {
            source: src,
            value: 0,
        }));
        assert!(down.contains("-8192"), "got {down:?}");

        let note = text(InputEvent::Note(NoteMsg {
            source: src,
            channel: 0,
            on: true,
            note: 60,
            vel: 100,
        }));
        assert!(
            note.contains("C4") && note.contains("vel 100"),
            "got {note:?}"
        );
    }

    /// An unknown port index must not panic the draw path.
    #[test]
    fn missing_port_names_degrade_to_an_index() {
        let label = source_label(InputSource::Midi(7), &[]);
        assert_eq!(label, "port 7");
    }

    fn render(events: &[InputEvent], ports: &[String], w: u16, h: u16) -> String {
        render_tab(events, ports, w, h, MonitorTab::Midi)
    }

    /// Draw a keyboard tab over a test backend and return the screen.
    fn render_keys(
        state: &KeyboardState,
        mode: KeyColor,
        tab: MonitorTab,
        w: u16,
        h: u16,
    ) -> String {
        use ratatui::{backend::TestBackend, Terminal};
        let spec = crate::spectrum::Spectrum::new();
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| {
            draw_midi_monitor(f, f.area(), &[], &[], tab, state, mode, &spec);
        })
        .unwrap();
        let buf = term.backend().buffer().clone();
        (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The colour of every cell that is lit with `colour`, for counting.
    fn cells_coloured(state: &KeyboardState, mode: KeyColor, w: u16, h: u16) -> Vec<Color> {
        use ratatui::{backend::TestBackend, Terminal};
        let spec = crate::spectrum::Spectrum::new();
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| {
            draw_midi_monitor(f, f.area(), &[], &[], MonitorTab::Keys, state, mode, &spec);
        })
        .unwrap();
        let buf = term.backend().buffer().clone();
        (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .map(|(x, y)| buf[(x, y)].style().fg.unwrap_or(Color::Reset))
            .collect()
    }

    fn note_on(note: u8, channel: u8, vel: u8) -> InputEvent {
        InputEvent::Note(NoteMsg {
            source: InputSource::Midi(0),
            channel,
            on: true,
            note,
            vel,
        })
    }

    fn note_off(note: u8) -> InputEvent {
        InputEvent::Note(NoteMsg {
            source: InputSource::Midi(0),
            channel: 0,
            on: false,
            note,
            vel: 0,
        })
    }

    /// The whole point of the tab: a key goes down, the key lights; it comes
    /// up, the light goes out.
    #[test]
    fn a_note_lights_its_key_until_the_note_off() {
        let mut k = KeyboardState::default();
        k.feed(&note_on(60, 0, 100), Some(0));
        assert_eq!(k.lit(60).map(|l| l.vel), Some(100));
        assert!(k.lit(61).is_none(), "only the key that was played");

        k.feed(&note_off(60), Some(0));
        assert!(k.lit(60).is_none());
    }

    /// **A note-on with velocity 0 is a note-off.** Plenty of hardware only
    /// ever says it that way, and reading it literally leaves the key lit for
    /// the rest of the session.
    #[test]
    fn a_note_on_with_velocity_zero_releases_the_key() {
        let mut k = KeyboardState::default();
        k.feed(&note_on(64, 0, 90), None);
        assert!(k.lit(64).is_some());

        k.feed(&note_on(64, 0, 0), None);
        assert!(k.lit(64).is_none(), "velocity 0 is a release");
    }

    /// PANIC exists because the rack and the keyboard can get out of step — a
    /// cable pulled mid-chord leaves note-ons with no note-off. It has to clear
    /// the picture too, or the visualizer keeps insisting on a chord nothing is
    /// playing.
    #[test]
    fn panic_puts_every_key_back_up() {
        let mut k = KeyboardState::default();
        for n in [60, 64, 67] {
            k.feed(&note_on(n, 0, 100), Some(0));
        }
        k.clear();
        assert!(
            (0..128).all(|n| k.lit(n).is_none()),
            "nothing survives a panic"
        );
    }

    /// Two channels have to be told apart at a glance — that is what the mode
    /// is for. Velocity mode instead separates two strengths on one channel.
    #[test]
    fn the_colour_modes_separate_what_they_claim_to() {
        let (mut a, mut b) = (KeyboardState::default(), KeyboardState::default());
        a.feed(&note_on(60, 0, 100), Some(0));
        b.feed(&note_on(60, 5, 100), Some(1));
        let (ca, cb) = (
            key_colour(KeyColor::Channel, &a.lit(60).unwrap()),
            key_colour(KeyColor::Channel, &b.lit(60).unwrap()),
        );
        assert_ne!(ca, cb, "two channels, two colours");

        let (mut soft, mut hard) = (KeyboardState::default(), KeyboardState::default());
        soft.feed(&note_on(60, 0, 20), Some(0));
        hard.feed(&note_on(60, 0, 127), Some(0));
        assert_ne!(
            key_colour(KeyColor::Velocity, &soft.lit(60).unwrap()),
            key_colour(KeyColor::Velocity, &hard.lit(60).unwrap()),
            "how hard it was played has to show"
        );

        // Instrument mode answers "which tab is playing this", so the same note
        // on two tabs must differ.
        assert_ne!(
            key_colour(KeyColor::Instrument, &a.lit(60).unwrap()),
            key_colour(KeyColor::Instrument, &b.lit(60).unwrap()),
        );
    }

    /// A lit key has to actually change what is drawn, not just what is stored.
    #[test]
    fn the_drawn_keyboard_changes_when_a_key_goes_down() {
        let mut k = KeyboardState::default();
        let before = cells_coloured(&k, KeyColor::Channel, 60, 5);
        k.feed(&note_on(60, 0, 100), Some(0));
        let after = cells_coloured(&k, KeyColor::Channel, 60, 5);
        assert_ne!(before, after, "middle C lights up");

        // And a black key lights the row above the white bodies.
        let mut sharp = KeyboardState::default();
        sharp.feed(&note_on(61, 0, 100), Some(0));
        assert_ne!(
            cells_coloured(&sharp, KeyColor::Channel, 60, 5),
            before,
            "C# lights too"
        );
    }

    /// The panel is drawn at whatever size the terminal leaves it, including
    /// sizes where a full keyboard cannot fit.
    #[test]
    fn the_keyboard_survives_a_narrow_panel() {
        let mut k = KeyboardState::default();
        k.feed(&note_on(60, 0, 100), Some(0));
        for (w, h) in [(40, 6), (20, 4), (12, 3), (60, 12)] {
            let screen = render_keys(&k, KeyColor::Channel, MonitorTab::Keys, w, h);
            // The tab strip is only legible while it fits; below that it is
            // truncated like every other panel, which is not this test's point.
            if w >= 40 {
                assert!(screen.contains("KEYS"), "the tab strip is still there");
            }
            assert!(
                screen.lines().all(|l| l.chars().count() == w as usize),
                "no row overflows at {w}x{h}"
            );
        }
    }

    /// A note outside the default window still has to be visible: the view
    /// follows what is being played.
    #[test]
    fn the_view_follows_notes_below_the_default_window() {
        let mut k = KeyboardState::default();
        k.feed(&note_on(24, 0, 100), Some(0)); // C1, below C2
        let (lo, _hi) = visible_range(&k, 200);
        assert!(lo <= 24, "the window opened downwards, got {lo}");
    }

    /// The roll draws the same notes as bars and lands them on the keyboard.
    #[test]
    fn the_roll_draws_the_keyboard_under_the_bars() {
        let mut k = KeyboardState::default();
        k.feed(&note_on(60, 0, 100), Some(0));
        let screen = render_keys(&k, KeyColor::Channel, MonitorTab::Roll, 60, 12);
        assert!(screen.contains("ROLL"));
        assert!(
            screen.lines().rev().take(4).any(|l| l.contains('\u{2588}')),
            "the keyboard is at the bottom:\n{screen}"
        );
    }

    /// Controllers never light a key — they get their own row, and the two a
    /// player watches while playing are always on it.
    #[test]
    fn controllers_go_under_the_keys_and_light_nothing() {
        let mut k = KeyboardState::default();
        k.feed(
            &InputEvent::Cc(CcMsg {
                source: InputSource::Midi(0),
                channel: 0,
                cc: 64,
                value: 127,
            }),
            None,
        );
        assert!(
            (0..128).all(|n| k.lit(n).is_none()),
            "a pedal is not a note"
        );

        k.feed(
            &InputEvent::Bend(BendMsg {
                source: InputSource::Midi(0),
                value: 0,
            }),
            None,
        );
        let screen = render_keys(&k, KeyColor::Channel, MonitorTab::Keys, 70, 7);
        assert!(screen.contains("BEND"), "{screen}");
        assert!(
            screen.contains("-8192"),
            "the wheel reads centred on 0: {screen}"
        );
        assert!(screen.contains("SUSTAIN"), "the pedal is named: {screen}");
    }

    /// The note `A→M` is converting shows on the keyboard like any other. It
    /// never travels as MIDI — it is made in the audio callback — so without
    /// this the one view that is supposed to say what is arriving would be the
    /// one place it cannot be seen.
    #[test]
    fn the_converted_note_lights_a_key_and_goes_out_by_itself() {
        let mut state = KeyboardState::default();
        assert!(!state.drawn_keys().contains(&60), "nothing yet");

        state.feed_converted(Converted::PitchToMidi, Some(60), Some(0));
        assert!(
            state.drawn_keys().contains(&60),
            "the converted note lights"
        );

        // The tracker moves to another note: nothing sends a note-off for the
        // old one, so putting it out is this function's job.
        state.feed_converted(Converted::PitchToMidi, Some(64), Some(0));
        let lit = state.drawn_keys();
        assert!(lit.contains(&64) && !lit.contains(&60), "lit: {lit:?}");

        // And silence puts the last one out.
        state.feed_converted(Converted::PitchToMidi, None, Some(0));
        assert!(state.drawn_keys().is_empty(), "and it goes out");
    }

    /// AutoTune's target note shows too, and the two sources do not put each
    /// other out.
    ///
    /// The note an effect is *correcting towards* is decided in the callback
    /// and says so nowhere else — so a singer watching this panel is the only
    /// way to see that AutoTune is aiming where they meant.
    #[test]
    fn autotune_lights_the_note_it_is_aiming_at() {
        let mut state = KeyboardState::default();
        state.feed_converted(Converted::PitchToMidi, Some(60), Some(0));
        state.feed_converted(Converted::AutoTune, Some(69), Some(1));
        let lit = state.drawn_keys();
        assert!(lit.contains(&60) && lit.contains(&69), "lit: {lit:?}");

        // Each source puts out its own note and only its own.
        state.feed_converted(Converted::AutoTune, Some(71), Some(1));
        let lit = state.drawn_keys();
        assert!(
            lit.contains(&60) && lit.contains(&71) && !lit.contains(&69),
            "lit: {lit:?}"
        );

        state.feed_converted(Converted::AutoTune, None, None);
        let lit = state.drawn_keys();
        assert_eq!(lit, vec![60], "the converter's note is still its own");
    }

    /// The spectrum tab draws where the tone is, marks the decades, and does
    /// not fall over in a panel two rows tall.
    #[test]
    fn the_spectrum_draws_a_tone_where_it_belongs() {
        use ratatui::{backend::TestBackend, Terminal};
        let mut spec = crate::spectrum::Spectrum::new();
        spec.set_sample_rate(48_000.0);
        let tone: Vec<f32> = (0..choz_engine::meter::SPECTRUM_POINTS)
            .map(|i| (std::f32::consts::TAU * 1000.0 * i as f32 / 48_000.0).sin())
            .collect();
        spec.analyse(&tone);

        let (w, h) = (64u16, 10u16);
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| {
            draw_midi_monitor(
                f,
                f.area(),
                &[],
                &[],
                MonitorTab::Spectrum,
                &KeyboardState::default(),
                KeyColor::default(),
                &spec,
            );
        })
        .unwrap();
        let buf = term.backend().buffer().clone();
        let rows: Vec<String> = (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect())
            .collect();
        let screen = rows.join("\n");
        assert!(screen.contains("SPEC"), "the tab is in the strip: {screen}");
        assert!(screen.contains("1k"), "the decades are marked: {screen}");

        // The tallest column is the one 1 kHz belongs to. The panel has a
        // border, so the plot starts one column in.
        let plot_w = (w - 2) as usize;
        let expect = spec.marker_col(1000.0, plot_w).unwrap();
        let height_of = |col: usize| {
            rows.iter()
                .filter(|r| {
                    r.chars()
                        .nth(col + 1)
                        .is_some_and(|c| c == '\u{2588}' || c == '\u{2584}' || c == '\u{2580}')
                })
                .count()
        };
        let tallest = (0..plot_w).max_by_key(|c| height_of(*c)).unwrap();
        assert!(
            tallest.abs_diff(expect) <= 1,
            "the 1 kHz tone should be tallest at column {expect}, it is at {tallest}\n{screen}"
        );
        assert!(height_of(tallest) > 0, "nothing was drawn at all");
        // Far away there is nothing to draw.
        let quiet = spec.marker_col(60.0, plot_w).unwrap();
        assert_eq!(height_of(quiet), 0, "silence should stay empty:\n{screen}");
    }

    /// Two rows is a panel with no room for a plot; it must not panic.
    #[test]
    fn the_spectrum_survives_a_panel_with_no_room() {
        use ratatui::{backend::TestBackend, Terminal};
        let spec = crate::spectrum::Spectrum::new();
        for h in [1u16, 2, 3] {
            let mut term = Terminal::new(TestBackend::new(20, h)).unwrap();
            term.draw(|f| {
                draw_midi_monitor(
                    f,
                    f.area(),
                    &[],
                    &[],
                    MonitorTab::Spectrum,
                    &KeyboardState::default(),
                    KeyColor::default(),
                    &spec,
                );
            })
            .unwrap();
        }
    }

    fn render_tab(
        events: &[InputEvent],
        ports: &[String],
        w: u16,
        h: u16,
        tab: MonitorTab,
    ) -> String {
        use ratatui::{backend::TestBackend, Terminal};
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| {
            draw_midi_monitor(
                f,
                f.area(),
                events,
                ports,
                tab,
                &KeyboardState::default(),
                KeyColor::default(),
                &crate::spectrum::Spectrum::new(),
            );
        })
        .unwrap();
        // One string per row, so assertions can talk about lines.
        let buf = term.backend().buffer().clone();
        (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn shows_the_newest_messages_and_drops_the_oldest() {
        let ports = vec!["Keystation Pro 88".to_string()];
        let src = InputSource::Midi(0);
        // More messages than the 6 inner rows of an 8-row panel.
        let events: Vec<InputEvent> = (0..20)
            .map(|i| {
                InputEvent::Note(NoteMsg {
                    source: src,
                    channel: 0,
                    on: true,
                    note: 40 + i,
                    vel: 100,
                })
            })
            .collect();

        let screen = render(&events, &ports, 50, 8);
        assert!(screen.contains("MIDI IN"), "panel is titled:\n{screen}");
        assert!(
            screen.contains(&note_name(59)),
            "newest message is shown:\n{screen}"
        );
        assert!(
            !screen.contains(&note_name(40)),
            "oldest scrolled off:\n{screen}"
        );
    }

    #[test]
    fn says_it_is_waiting_when_nothing_has_arrived() {
        let screen = render(&[], &[], 40, 8);
        assert!(screen.contains("waiting for MIDI"), "got:\n{screen}");
    }

    /// A panel too short for a single row must not panic or overflow.
    #[test]
    fn survives_being_squeezed_to_nothing() {
        let src = InputSource::Midi(0);
        let e = [InputEvent::Note(NoteMsg {
            source: src,
            channel: 0,
            on: true,
            note: 60,
            vel: 100,
        })];
        for h in 0..4 {
            render(&e, &[], 30, h);
        }
    }

    /// Three tabs, and each one shows its own thing. WAVE and ACTIVITY read the
    /// engine's meter, so with no audio running they draw an empty picture
    /// rather than nothing at all — "no signal" is information.
    #[test]
    fn the_monitor_has_three_tabs_and_each_draws_its_own() {
        choz_engine::meter::meter().clear();
        let midi = render_tab(&[], &[], 60, 10, MonitorTab::Midi);
        assert!(midi.contains("MIDI") && midi.contains("WAVE") && midi.contains("ACTIVITY"));
        assert!(
            midi.contains("waiting for MIDI"),
            "the MIDI tab is the messages"
        );

        let wave = render_tab(&[], &[], 60, 10, MonitorTab::Wave);
        assert!(
            !wave.contains("waiting for MIDI"),
            "a different tab, different content"
        );
        assert!(
            wave.contains('\u{2500}'),
            "silence still draws its centre line"
        );

        // A block through the meter and the wave has something in it.
        let buf: Vec<f32> = (0..512)
            .flat_map(|i| {
                let s = 0.8 * (2.0 * std::f32::consts::PI * i as f32 / 32.0).sin();
                [s, s]
            })
            .collect();
        choz_engine::meter::meter().publish(&buf);
        let wave = render_tab(&[], &[], 60, 10, MonitorTab::Wave);
        assert!(
            wave.chars()
                .any(|c| c == '\u{2588}' || c == '\u{2580}' || c == '\u{2584}'),
            "the shape of the sound: {wave}"
        );

        let activity = render_tab(&[], &[], 60, 10, MonitorTab::Activity);
        assert!(
            activity.contains("PEAK") && activity.contains("RMS"),
            "{activity}"
        );
        assert!(
            activity.contains("dB"),
            "levels in dB, not in fractions: {activity}"
        );
        choz_engine::meter::meter().clear();
    }

    /// The tabs are clickable, which means they have to hand back where they
    /// were drawn.
    #[test]
    fn the_tab_strip_reports_its_rects() {
        use ratatui::{backend::TestBackend, Terminal};
        let mut term = Terminal::new(TestBackend::new(60, 10)).unwrap();
        let mut rects = Vec::new();
        term.draw(|f| {
            rects = draw_midi_monitor(
                f,
                f.area(),
                &[],
                &[],
                MonitorTab::Midi,
                &KeyboardState::default(),
                KeyColor::default(),
                &crate::spectrum::Spectrum::new(),
            );
        })
        .unwrap();
        assert_eq!(rects.len(), MonitorTab::ALL.len());
        assert_eq!(rects[0].0, MonitorTab::Midi);
        assert!(rects[1].1.x > rects[0].1.x, "left to right");
        assert!(rects.iter().all(|(_, r)| r.height == 1));

        assert_eq!(MonitorTab::Midi.next(), MonitorTab::Keys);
        assert_eq!(MonitorTab::Activity.next(), MonitorTab::Midi, "it wraps");
    }
}
