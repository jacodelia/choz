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

/// Chord shapes, as semitone steps up from the root paired with the suffix
/// that names them.
///
/// **Order matters.** The list is searched top to bottom, so the four-note
/// shapes come first: C-E-G-B contains C-E-G, and answering "C" to a hand
/// playing Cmaj7 is not wrong so much as useless. Within a size, the shapes a
/// player is likelier to be holding come first.
const CHORD_SHAPES: &[(&[u8], &str)] = &[
    (&[0, 4, 7, 11], "maj7"),
    (&[0, 4, 7, 10], "7"),
    (&[0, 3, 7, 10], "m7"),
    (&[0, 3, 7, 11], "m(maj7)"),
    (&[0, 3, 6, 10], "m7b5"),
    (&[0, 3, 6, 9], "dim7"),
    (&[0, 4, 7, 9], "6"),
    (&[0, 3, 7, 9], "m6"),
    (&[0, 4, 8, 10], "7#5"),
    (&[0, 4, 6, 10], "7b5"),
    (&[0, 2, 4, 7], "add9"),
    (&[0, 5, 7, 10], "7sus4"),
    (&[0, 4, 7], ""),
    (&[0, 3, 7], "m"),
    (&[0, 3, 6], "dim"),
    (&[0, 4, 8], "aug"),
    (&[0, 5, 7], "sus4"),
    (&[0, 2, 7], "sus2"),
    (&[0, 7], "5"),
];

/// Name the chord `notes` spells, or `None` when it does not spell one.
///
/// `notes` is MIDI numbers in any order; the **lowest** is the bass, which is
/// what decides whether the answer is `C` or `C/E`. Octaves and doublings are
/// folded away first, so a chord voiced across three octaves names the same as
/// the same chord under one hand.
///
/// Returns `None` rather than guessing: two notes that are not a fifth, or a
/// cluster matching no shape, has no name worth printing, and inventing one is
/// worse than a blank.
pub fn chord_name(notes: &[u8]) -> Option<String> {
    if notes.len() < 2 {
        return None;
    }
    let bass = *notes.iter().min()? % 12;
    let mut classes: Vec<u8> = notes.iter().map(|n| n % 12).collect();
    classes.sort_unstable();
    classes.dedup();

    // Try every note as the root. The bass is tried first so that a chord in
    // root position never comes back as an inversion of something else.
    let roots = std::iter::once(bass).chain(classes.iter().copied().filter(|c| *c != bass));
    for root in roots {
        let mut steps: Vec<u8> = classes.iter().map(|c| (c + 12 - root) % 12).collect();
        steps.sort_unstable();
        let Some((_, suffix)) = CHORD_SHAPES.iter().find(|(shape, _)| *shape == steps) else {
            continue;
        };
        let name = format!("{}{suffix}", NOTE_NAMES[root as usize]);
        // A root that is not in the bass is an inversion, and a player reading
        // the panel wants to see which one.
        return Some(match root == bass {
            true => name,
            false => format!("{name}/{}", NOTE_NAMES[bass as usize]),
        });
    }
    None
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
        InputEvent::Clock(_, c) => (
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
    /// The message log, the spectrum and the level meters, side by side.
    ///
    /// One tab rather than three because they answer one question between
    /// them — *did it arrive, what came out, how loud* — and reading it meant
    /// cycling tabs and holding the other two from memory.
    #[default]
    Monitor,
    /// A piano keyboard lit by what is arriving, and the chord it spells.
    Keys,
    /// The output's shape, stacked into a history.
    Wave,
    /// Every tab's strip at once: level, pan, mute, solo. The RACK shows the
    /// active tab's mixer and only that one; balancing a set means seeing them
    /// side by side.
    Mixer,
}

impl MonitorTab {
    pub const ALL: [MonitorTab; 4] = [
        MonitorTab::Monitor,
        MonitorTab::Keys,
        MonitorTab::Wave,
        MonitorTab::Mixer,
    ];

    pub fn label(self) -> &'static str {
        match self {
            MonitorTab::Monitor => "MONITOR",
            MonitorTab::Keys => "KEYS",
            MonitorTab::Wave => "WAVE",
            MonitorTab::Mixer => "MIXER",
        }
    }

    /// Whether this tab draws the keyboard, and so answers the colour key.
    pub fn is_keyboard(self) -> bool {
        matches!(self, MonitorTab::Keys)
    }

    /// Whether the FFT has to run this frame. It costs a 2048-point transform
    /// on the UI thread, so it only runs when something is drawing it.
    pub fn needs_spectrum(self) -> bool {
        matches!(self, MonitorTab::Monitor)
    }

    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|t| *t == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
}

/// What decides the colour of a lit key.
///
/// Four questions a player actually asks, one mode each: *which channel is
/// this* (MULTI, where a channel is a tab), *which keyboard did it come from*,
/// *which tab is sounding it*, and *how hard am I playing*.
///
/// **The last two are not the same question**, which is why they are two modes.
/// Two controllers can both play the same tab and one controller can be split
/// across two, so "where did this note come in" and "what is it playing" answer
/// different halves of a rig — and reading a rig means being able to ask each
/// of them on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum KeyColor {
    Channel,
    /// One colour per **MIDI input**: which keyboard, pad or sequencer the note
    /// arrived on. Notes choz made itself — the QWERTY piano, `A→M` — have no
    /// port and are drawn dim.
    Source,
    /// The default: one colour per rack tab. "Which tab is this note playing"
    /// is the question a rack of tabs raises, and the legend under the
    /// keyboard names the colours — which is what made the other modes
    /// unreadable before there was one.
    #[default]
    Instrument,
    Velocity,
}

impl KeyColor {
    pub const ALL: [KeyColor; 4] = [
        KeyColor::Channel,
        KeyColor::Source,
        KeyColor::Instrument,
        KeyColor::Velocity,
    ];

    pub fn label(self) -> &'static str {
        match self {
            KeyColor::Channel => "CHANNEL",
            KeyColor::Source => "INPUT",
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
    /// Which input it arrived on. `None` for a note choz made itself — the
    /// QWERTY piano, `A→M`. Kept because a chord the harmoniser follows has to
    /// be able to come from **one** keyboard: with two of them on a hub, the
    /// channel alone cannot tell them apart.
    pub source: Option<InputSource>,
}

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
                source: None,
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
    /// The same, narrowed to one input as well.
    ///
    /// `source` is which port the notes must have come from; `None` takes them
    /// from any. That is the difference between "the chord on channel 1" and
    /// "the chord on **this** keyboard" — with two controllers both sending
    /// channel 1, only the second question has an answer.
    pub fn held_from(
        &self,
        channel: u8,
        slot: Option<usize>,
        source: Option<InputSource>,
    ) -> Vec<u8> {
        let wire = channel.saturating_sub(1);
        self.keys
            .iter()
            .enumerate()
            .filter_map(|(note, lit)| {
                let lit = lit.as_ref()?;
                let same_tab = slot.is_none() || lit.slot.is_none() || lit.slot == slot;
                let same_port = match source {
                    None => true,
                    // A note choz made itself has no port and belongs to
                    // whoever asks: `A→M` feeding the harmony is the point of
                    // `A→M`.
                    Some(want) => lit.source.is_none_or(|s| s == want),
                };
                (lit.channel == wire && same_tab && same_port).then_some(note as u8)
            })
            .collect()
    }

    /// Which notes are lit right now, low to high, whatever channel they came
    /// on. This is what the chord readout names: a player holding a chord is
    /// not thinking about which channel each finger is on.
    pub fn held(&self) -> Vec<u8> {
        self.keys
            .iter()
            .enumerate()
            .filter_map(|(i, k)| k.map(|_| i as u8))
            .collect()
    }

    /// Which notes are lit right now. For a test, and for anything that wants
    /// to know what the keyboard is showing without redrawing it.
    #[cfg(test)]
    pub(crate) fn drawn_keys(&self) -> Vec<u8> {
        self.held()
    }

    /// Put a note out.
    fn release(&mut self, note: u8) {
        if (note as usize) < 128 {
            self.keys[note as usize] = None;
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
                    source: Some(m.source),
                });
            }
            InputEvent::Note(m) => {
                if (m.note as usize) < 128 {
                    self.keys[m.note as usize] = None;
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
            InputEvent::Program(_) | InputEvent::Control(_) | InputEvent::Clock(..) => {}
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
        self.bend = 8192;
        self.modulation = 0;
    }

    /// Which keys are down, and how they were played.
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
        // Offset past the tabs' wheel so a port and a tab of the same number
        // are not the same colour — the two modes are read one after the other,
        // and a colour that means two things is worse than no colour.
        KeyColor::Source => match key.source {
            Some(InputSource::Midi(i)) => hue_of(i as u32 + 9),
            Some(InputSource::Osc) => hue_of(8),
            // choz's own: the QWERTY piano and what `A→M` heard. They came from
            // no port, and saying so is the point of this mode.
            Some(InputSource::Keyboard) | None => DIM,
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

/// The full piano: A0 (MIDI 21) up to C8 (108). Eighty-eight keys, of which
/// fifty-two are white — and one column per white key is what makes the whole
/// instrument fit a panel narrower than a hundred cells.
const PIANO_LO: u8 = 21;
const PIANO_HI: u8 = 108;

/// Unlit key colours: ivory, and the dark the black keys sit at.
const WHITE_KEY: Color = Color::Rgb(214, 219, 228);
const BLACK_KEY: Color = Color::Rgb(28, 31, 38);

/// Every white key of the piano, low to high.
fn piano_whites() -> Vec<u8> {
    (PIANO_LO..=PIANO_HI).filter(|n| !is_black(*n)).collect()
}

/// The white keys to draw in `width` columns, one column each.
///
/// The whole piano when it fits, which it does from 52 columns up. Below that
/// the view is a window onto it, held over whatever is sounding — a keyboard
/// scrolled away from the notes being played answers nothing.
fn visible_whites(state: &KeyboardState, width: usize) -> Vec<u8> {
    let all = piano_whites();
    if width >= all.len() || width == 0 {
        return all;
    }
    // Centre the window on what is sounding, or on middle C when nothing is.
    let centre = match state.sounding_range() {
        Some((lo, hi)) => (lo as usize + hi as usize) / 2,
        None => 60,
    };
    let at = all
        .iter()
        .position(|w| *w as usize >= centre)
        .unwrap_or(all.len() / 2);
    let start = at.saturating_sub(width / 2).min(all.len() - width);
    all[start..start + width].to_vec()
}

/// The keyboard: black keys on the top row, white key bodies below, octave
/// numbers under those.
///
/// One column per white key, and the black keys are drawn with half-blocks
/// straddling the boundary they really sit on — the right half of the white
/// below them and the left half of the white above. A white key with a black
/// on each side (D, G, A) ends up fully covered, and *that* is what turns the
/// top row into the piano's 2-and-3 grouping instead of an even dotted line:
///
/// ```text
///  ▐█▌ ▐███▌ ▐█▌ ▐███▌
///  ███████████████████
///  1     2
/// ```
fn keyboard_lines(state: &KeyboardState, mode: KeyColor, width: usize) -> Vec<Line<'static>> {
    let whites = visible_whites(state, width);
    let colour_of = |note: u8, unlit: Color| match state.lit(note) {
        Some(k) => key_colour(mode, &k),
        None => unlit,
    };

    let mut blacks: Vec<Span> = Vec::with_capacity(whites.len());
    let mut bodies: Vec<Span> = Vec::with_capacity(whites.len());
    let mut labels: Vec<Span> = Vec::with_capacity(whites.len());
    for &w in &whites {
        let white = colour_of(w, WHITE_KEY);

        // The black keys touching this white key, if they are on the piano.
        let left = w.checked_sub(1).filter(|n| *n >= PIANO_LO && is_black(*n));
        let right = Some(w + 1).filter(|n| *n <= PIANO_HI && is_black(*n));
        let glyph = match (left.is_some(), right.is_some()) {
            (true, true) => '\u{2588}',
            (true, false) => '\u{258C}',
            (false, true) => '\u{2590}',
            (false, false) => ' ',
        };
        // One cell cannot colour its two halves apart, so a lit black key wins
        // the cell it shares: a key being played is the thing being looked for.
        let black = [left, right]
            .into_iter()
            .flatten()
            .find(|n| state.lit(*n).is_some())
            .map(|n| colour_of(n, BLACK_KEY))
            .unwrap_or(BLACK_KEY);
        blacks.push(Span::styled(
            glyph.to_string(),
            Style::default().fg(black).bg(white),
        ));
        bodies.push(Span::styled(
            "\u{2588}".to_string(),
            Style::default().fg(white),
        ));

        // Octave numbers under each C, one cell wide so they cannot collide
        // with the key next door. C4 is middle C, as everywhere else.
        let label = match w % 12 == 0 {
            true => char::from_digit((w as u32 / 12).saturating_sub(1), 10).unwrap_or(' '),
            false => ' ',
        };
        labels.push(Span::styled(
            label.to_string(),
            Style::default().fg(if label == ' ' { DIM } else { HEADER }),
        ));
    }
    vec![Line::from(blacks), Line::from(bodies), Line::from(labels)]
}

/// The piano keyboard, lit by what is arriving.
fn draw_keys(
    f: &mut Frame,
    area: Rect,
    state: &KeyboardState,
    mode: KeyColor,
    strips: &[MixerStrip],
    ports: &[String],
) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let height = area.height as usize;
    let mut lines = keyboard_lines(state, mode, area.width as usize);
    // The octave labels are the first thing to go when the panel is short: a
    // C2/C3/C4 ruler is a nicety, where the chord and the wheels below it are
    // readings. Without this, a seven-row panel spent its last row on the
    // ruler and pushed the pedals off the bottom.
    if height < lines.len() + 2 {
        lines.pop();
    }
    // The chord goes directly under the keys, before the controllers: it is
    // the thing being read while both hands are busy.
    if height > lines.len() {
        lines.push(chord_line(state));
    }
    // Below that: the wheels and the last few controllers. They are what
    // a player checks after "did the note arrive" — and CCs never light a key.
    if height > lines.len() {
        lines.push(controller_line(state));
    }
    // The key to the colours, in the colours: a keyboard lit in six hues says
    // nothing until something names them. What the legend lists is what the
    // mode colours by — one entry per tab, per channel, or the velocity ramp.
    if height > lines.len() {
        lines.push(colour_legend(mode, state, strips, ports));
    }
    f.render_widget(
        Paragraph::new(lines).style(super::theme::panel_style()),
        area,
    );
}

/// The colour key under the keyboard: which colour is which tab, channel or
/// velocity, spelled in the colour it stands for, and the `[C]` that cycles it.
fn colour_legend(
    mode: KeyColor,
    state: &KeyboardState,
    strips: &[MixerStrip],
    ports: &[String],
) -> Line<'static> {
    use crate::views::fx_chain_panel::truncate;
    let mut spans = vec![Span::styled(
        format!("  {} ", mode.label()),
        Style::default().fg(DIM).add_modifier(Modifier::BOLD),
    )];
    match mode {
        // One entry per rack tab, in the hue that tab's notes are drawn in —
        // `key_colour` offsets the slot by three, so the legend has to as well
        // or it would name the wrong colour with total confidence.
        KeyColor::Instrument => {
            for (i, st) in strips
                .iter()
                .filter(|s| s.kind == StripKind::Tab)
                .enumerate()
            {
                spans.push(Span::styled(
                    format!("{}:{} ", i + 1, truncate(&st.label, 8)),
                    Style::default().fg(hue_of(i as u32 + 3)),
                ));
            }
        }
        // Every input that is connected, whether or not it is playing right
        // now: this legend is also the answer to "is that keyboard even
        // plugged in", and a port that vanishes from the key when nothing is
        // held cannot answer it. `InputSource::Midi(i)` indexes this list.
        KeyColor::Source => {
            for (i, name) in ports.iter().enumerate() {
                spans.push(Span::styled(
                    format!("{} ", truncate(name, 12)),
                    Style::default().fg(hue_of(i as u32 + 9)),
                ));
            }
            // The notes with no port at all, named so the dim keys are not
            // read as a fault.
            spans.push(Span::styled(
                format!("{} ", crate::i18n::t("QWERTY")),
                Style::default().fg(DIM),
            ));
        }
        // Sixteen channels would be a legend nobody can read across a panel,
        // so it lists the ones actually arriving. Nothing held is not an empty
        // legend: it is the reminder that this colours by channel.
        KeyColor::Channel => {
            let mut seen: Vec<u8> = state
                .keys
                .iter()
                .filter_map(|k| k.as_ref().map(|l| l.channel))
                .collect();
            seen.sort_unstable();
            seen.dedup();
            if seen.is_empty() {
                spans.push(Span::styled(
                    "\u{2014} ".to_string(),
                    Style::default().fg(DIM),
                ));
            }
            for ch in seen {
                spans.push(Span::styled(
                    format!("CH{} ", ch + 1),
                    Style::default().fg(hue_of(ch as u32)),
                ));
            }
        }
        // A ramp rather than a list: velocity is continuous, and the thing to
        // read off it is which end is hard.
        KeyColor::Velocity => {
            for v in [1u8, 32, 64, 96, 127] {
                let lit = KeyLit {
                    channel: 0,
                    vel: v,
                    slot: None,
                    source: None,
                };
                spans.push(Span::styled(
                    format!("{v} "),
                    Style::default().fg(key_colour(KeyColor::Velocity, &lit)),
                ));
            }
        }
    }
    spans.push(Span::styled(" [C]", Style::default().fg(DIM)));
    Line::from(spans)
}

/// The chord under the hand: its name, then the notes that spell it.
///
/// A hand holding notes that name no chord still gets the note list — that is
/// the half that says the keys arrived, and it must not vanish just because
/// the shape has no name.
fn chord_line(state: &KeyboardState) -> Line<'static> {
    let held = state.held();
    if held.is_empty() {
        return Line::from(Span::styled(
            "  \u{2014}",
            Style::default().fg(DIM).add_modifier(Modifier::ITALIC),
        ));
    }
    let notes = held
        .iter()
        .map(|n| note_name(*n))
        .collect::<Vec<_>>()
        .join(" ");
    // The headline, in American notation throughout: the chord when the notes
    // spell one, the note itself when only one is down. A single key is the
    // commonest thing on this panel and it always has a name, so answering it
    // with a dash was the readout being pedantic at the player's expense.
    let (headline, style) = match (held.as_slice(), chord_name(&held)) {
        (_, Some(name)) => (name, Style::default().fg(OK).add_modifier(Modifier::BOLD)),
        ([one], None) => (
            note_name(*one),
            Style::default().fg(OK).add_modifier(Modifier::BOLD),
        ),
        // Two notes that are not a fifth are an interval, and more than that
        // with no match is a cluster. Neither is an error, so neither gets a
        // warning colour.
        _ => ("\u{2014}".to_string(), Style::default().fg(DIM)),
    };
    Line::from(vec![
        Span::styled("  ", Style::default()),
        Span::styled(format!("{headline:<10}"), style),
        Span::styled(notes, Style::default().fg(theme::text())),
    ])
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

/// Where the tab strip drew each tab, for the mouse.
pub type TabRect = (MonitorTab, Rect);
/// Where the MIXER drew each control, same reason.
pub type MixerRect = (MixerHit, Rect);

/// Draw the monitor. `events` is oldest-first; the newest ones are shown at the
/// bottom, so the eye follows new arrivals downward like a terminal log.
#[allow(clippy::too_many_arguments)]
pub fn draw_midi_monitor(
    f: &mut Frame,
    area: Rect,
    events: &[InputEvent],
    // The **connected** MIDI inputs, in the order `InputSource::Midi(i)`
    // indexes them. Only ever used to turn that index back into a name.
    ports: &[String],
    tab: MonitorTab,
    keyboard: &KeyboardState,
    key_colour_mode: KeyColor,
    // The analyser keeps its peak hold between frames, so it is owned by the
    // application and lent here rather than built per redraw.
    spectrum: &crate::spectrum::Spectrum,
    // Likewise a history, and for the same reason.
    wave: &WaveHistory,
    // One strip per rack tab, for the MIXER tab. Read from the tabs rather than
    // held here: the RACK edits them, and a second copy would drift.
    strips: &[MixerStrip],
) -> (Vec<TabRect>, Vec<MixerRect>) {
    let block = Block::default()
        .title(" MONITOR ")
        .title_style(Style::default().fg(HEADER).add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::border()))
        .style(super::theme::panel_style());

    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 {
        return (Vec::new(), Vec::new());
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
        return (rects, Vec::new());
    }

    let mut hits = Vec::new();
    match tab {
        MonitorTab::Keys => draw_keys(f, inner, keyboard, key_colour_mode, strips, ports),
        MonitorTab::Wave => draw_wave(f, inner, wave),
        MonitorTab::Monitor => draw_monitor_columns(f, inner, events, ports, spectrum),
        MonitorTab::Mixer => hits = draw_mixer(f, inner, strips),
    }
    (rects, hits)
}

/// What a strip stands for. The MIXER draws tabs, then the four subgroups,
/// then the main, in one run of strips — the way a desk is laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StripKind {
    Tab,
    /// A subgroup. Has a fader, a mute and an output pair, and none of the
    /// things that only mean something for a tab: no pan, no solo, no split
    /// sides — a group is already a sum.
    Bus,
    Main,
}

/// One strip of the MIXER: a rack tab, a subgroup, or the main.
pub struct MixerStrip {
    pub kind: StripKind,
    pub label: String,
    /// Linear, over the same range the RACK's VOL knob uses. One per output
    /// channel: a stereo tab can sit louder on one side, and `link` is what
    /// keeps the two together for the tabs where that is nonsense.
    pub gain: f32,
    pub gain_r: f32,
    pub link: bool,
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
    /// Drawn lit: the tab the RACK is on.
    pub active: bool,
    /// Which side the keyboard is pointed at, when the MIXER has the focus.
    pub side: Option<MixerSide>,
    /// Where a tab sums — `OUT`, or the letter of a group. `None` on the
    /// group and main strips, which are where things sum *to*.
    pub dest: Option<&'static str>,
}

/// A half of a strip, for the caller to say which one the arrows move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MixerSide {
    Left,
    Right,
    Both,
}

/// What the mouse can hit in the MIXER, by tab index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MixerHit {
    /// A level track, left or right. The value comes from where in the rect the
    /// click landed, so the caller needs the rect — which it has.
    Gain(usize),
    GainR(usize),
    /// Tie the two sides of a strip together, or let them go.
    Link(usize),
    Pan(usize),
    Mute(usize),
    Solo(usize),
    /// The tab's name: make it the active one.
    Select(usize),
    /// The destination cell: step it to the next group, or back to the device.
    Dest(usize),
    /// Walk the window of strips when the rack is wider than the panel.
    Page(isize),
}

/// Every tab as a channel strip, side by side, the way a desk is laid out —
/// the model is seqterm's mixer view: a bordered column per channel with a
/// vertical fader in it.
///
/// **It pages.** A rack of twelve tabs does not fit across a terminal, so the
/// strips that fit are drawn and `◀` `▶` on the top row walk the window, which
/// also follows the active tab so switching tabs never leaves the mixer showing
/// somewhere else.
fn draw_mixer(f: &mut Frame, area: Rect, strips: &[MixerStrip]) -> Vec<MixerRect> {
    let mut hits = Vec::new();
    if strips.is_empty() || area.height < 3 || area.width < STRIP_W {
        return hits;
    }
    let per_page = ((area.width / STRIP_W) as usize).max(1);
    let active = strips.iter().position(|s| s.active).unwrap_or(0);
    // The window follows the active tab, like every other list in choz: no
    // second piece of state to keep in step with the rack.
    let page = super::drawer::list_scroll(active, strips.len(), per_page);
    let paging = strips.len() > per_page;

    for (col, (i, st)) in strips
        .iter()
        .enumerate()
        .skip(page)
        .take(per_page)
        .enumerate()
    {
        let x = area.x + col as u16 * STRIP_W;
        let rect = Rect::new(x, area.y, STRIP_W - 1, area.height);
        hits.extend(draw_strip(f, rect, i, st));
    }

    // The pager sits on the right of the top row, over the last strip's border:
    // there is no row to spare in a panel that is eight lines at its tallest.
    if paging {
        let label = format!(" {page}+{per_page}/{} ", strips.len());
        let w = label.chars().count() as u16 + 2;
        if area.width > w {
            let x = area.x + area.width - w;
            let style = Style::default().fg(theme::text()).bg(theme::PANEL_BG);
            let prev = Rect::new(x, area.y, 1, 1);
            let next = Rect::new(x + w - 1, area.y, 1, 1);
            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled("\u{25C0}", Style::default().fg(ACCENT)),
                    Span::styled(label, style),
                    Span::styled("\u{25B6}", Style::default().fg(ACCENT)),
                ])),
                Rect::new(x, area.y, w, 1),
            );
            hits.push((MixerHit::Page(-1), prev));
            hits.push((MixerHit::Page(1), next));
        }
    }
    hits
}

/// Columns one channel strip takes, its one-column gutter included.
const STRIP_W: u16 = 9;

/// One channel: name, a vertical fader, its two flags and its pan.
fn draw_strip(f: &mut Frame, area: Rect, tab: usize, st: &MixerStrip) -> Vec<MixerRect> {
    use crate::views::fx_chain_panel::truncate;
    let mut hits = Vec::new();
    let name_style = if st.active {
        Style::default()
            .fg(Color::Black)
            .bg(ACCENT)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme::text())
    };
    let w = area.width as usize;
    let head = Rect::new(area.x, area.y, area.width, 1);
    // A tab is numbered because that is how it is addressed everywhere else; a
    // group and the main are named, because there is only one of each.
    let title = match st.kind {
        StripKind::Tab => format!("{} {}", tab + 1, truncate(&st.label, w.saturating_sub(2))),
        _ => truncate(&st.label, w),
    };
    let name_style = match st.kind {
        StripKind::Main => Style::default()
            .fg(Color::Black)
            .bg(HEADER)
            .add_modifier(Modifier::BOLD),
        StripKind::Bus => Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
        StripKind::Tab => name_style,
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(format!("{title:<w$}"), name_style)))
            .style(theme::panel_style()),
        head,
    );
    hits.push((MixerHit::Select(tab), head));

    // Two faders side by side — the tab's two output channels — with the link
    // between them. Everything between the name and the rows at the foot.
    let bottom = strip_bottom(area, st.kind);
    let fader_h = area.height.saturating_sub(1 + bottom);
    if fader_h > 0 {
        let y = area.y + 1;
        let colour = if st.mute { DIM } else { OK };
        // The lit side, when the keyboard is pointed at one of them.
        let lit = |side: MixerSide| {
            if st.link {
                return colour;
            }
            match st.side {
                Some(s) if s == side || s == MixerSide::Both => ACCENT,
                _ => colour,
            }
        };
        // A group is already a sum: one fader, the full width, and none of the
        // link machinery below. The **main** is not — it is a stereo output,
        // and it gets the same two faders a tab does.
        if st.kind == StripKind::Bus {
            let bar = Rect::new(area.x, y, area.width, fader_h);
            draw_fader(f, bar, st.gain, if st.mute { DIM } else { HEADER });
            hits.push((MixerHit::Gain(tab), bar));
            return finish_strip(f, area, tab, st, hits);
        }

        // `L` and `R` columns, one cell of gutter, then the link between them.
        let cols = ((area.width.saturating_sub(3)) / 2).max(1);
        let left = Rect::new(area.x, y, cols, fader_h);
        let right = Rect::new(area.x + cols + 1, y, cols, fader_h);
        draw_fader(f, left, st.gain, lit(MixerSide::Left));
        draw_fader(f, right, st.gain_r, lit(MixerSide::Right));
        hits.push((MixerHit::Gain(tab), left));
        hits.push((MixerHit::GainR(tab), right));

        // The link, drawn in the gutter down the middle: a chain when the two
        // move together, a break when they do not.
        let link_x = area.x + cols;
        let mid = y + fader_h / 2;
        for row in 0..fader_h {
            let on_link = y + row == mid;
            let (text, style) = if !on_link {
                (" ", Style::default().fg(DIM))
            } else if st.link {
                (
                    "\u{2261}",
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                )
            } else {
                ("\u{22EE}", Style::default().fg(DIM))
            };
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(text, style))).style(theme::panel_style()),
                Rect::new(link_x, y + row, 1, 1),
            );
        }
        hits.push((MixerHit::Link(tab), Rect::new(link_x, y, 1, fader_h)));
    }

    finish_strip(f, area, tab, st, hits)
}

/// The bottom of a strip: the flags row and the numbers row. Its own function
/// because a group leaves the fader early — one fader, no link — and still has
/// to have a mute and a level under it.
fn finish_strip(
    f: &mut Frame,
    area: Rect,
    tab: usize,
    st: &MixerStrip,
    mut hits: Vec<MixerRect>,
) -> Vec<MixerRect> {
    use crate::views::fx_chain_panel::pan_label;
    let w = area.width as usize;
    let bottom = strip_bottom(area, st.kind);
    let mut y = area.y + area.height - bottom;
    if bottom >= 2 {
        let flags = Rect::new(area.x, y, area.width, 1);
        // A group has no solo — soloing a sum is soloing the tabs in it, which
        // is what their own strips already do.
        let second = match st.kind {
            StripKind::Tab => Span::styled(" S ", flag(st.solo, Color::Rgb(220, 190, 70))),
            _ => Span::styled("   ", theme::panel_style()),
        };
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" M ", flag(st.mute, Color::Rgb(200, 80, 80))),
                Span::styled(" ", theme::panel_style()),
                second,
            ]))
            .style(theme::panel_style()),
            flags,
        );
        hits.push((MixerHit::Mute(tab), Rect::new(flags.x, y, 3, 1)));
        if st.kind == StripKind::Tab {
            hits.push((MixerHit::Solo(tab), Rect::new(flags.x + 4, y, 3, 1)));
        }
        y += 1;
    }
    // The level in numbers, and the pan. Two faders and one number: the number
    // is the side that is louder, which is the one you are setting.
    let row = Rect::new(area.x, y, area.width, 1);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!("{:<4.2}", st.gain.max(st.gain_r)),
                Style::default().fg(DIM),
            ),
            Span::styled(
                format!("{:>w$}", pan_label(st.pan), w = w.saturating_sub(4)),
                Style::default().fg(HEADER),
            ),
        ]))
        .style(theme::panel_style()),
        row,
    );
    if st.kind == StripKind::Tab {
        hits.push((MixerHit::Pan(tab), row));
    }
    y += 1;

    // Where the tab sums, when the panel is tall enough to say it. On a desk
    // with groups this is the setting that explains a tab nobody can hear —
    // and clicking it walks `OUT → A → B → C → D`.
    if bottom >= 3 {
        if let Some(d) = st.dest {
            let cell = Rect::new(area.x, y, area.width, 1);
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    format!("{:^w$}", format!("\u{25B8}{d}")),
                    Style::default()
                        .fg(Color::Black)
                        .bg(ACCENT)
                        .add_modifier(Modifier::BOLD),
                )))
                .style(theme::panel_style()),
                cell,
            );
            hits.push((MixerHit::Dest(tab), cell));
        }
    }
    hits
}

/// Rows at the foot of a strip: the flags and the numbers, plus the
/// destination when the panel is tall enough to show it and the strip is a tab
/// (a group is where things go; it has no destination of its own yet).
fn strip_bottom(area: Rect, kind: StripKind) -> u16 {
    let want = match kind == StripKind::Tab && area.height >= 7 {
        true => 3,
        false => 2,
    };
    want.min(area.height.saturating_sub(1))
}

/// One vertical fader, filled from the bottom, with the part-filled row drawn
/// in eighths — four rows of cells would otherwise be a four-position fader,
/// where the knob it stands for has forty.
fn draw_fader(f: &mut Frame, area: Rect, gain: f32, colour: Color) {
    use crate::views::fx_chain_panel::MAX_GAIN;
    let h = area.height;
    let filled = (gain / MAX_GAIN).clamp(0.0, 1.0) * h as f32;
    let lines: Vec<Line> = (0..h)
        .map(|row| {
            let from_bottom = h - row;
            let cell = if filled >= from_bottom as f32 {
                "\u{2588}"
            } else if filled > from_bottom as f32 - 1.0 {
                let eighths = ((filled - (from_bottom as f32 - 1.0)) * 8.0).round() as usize;
                [
                    "\u{2581}", "\u{2581}", "\u{2582}", "\u{2583}", "\u{2584}", "\u{2585}",
                    "\u{2586}", "\u{2587}", "\u{2588}",
                ][eighths.min(8)]
            } else {
                "\u{2591}"
            };
            Line::from(Span::styled(
                cell.repeat(area.width.max(1) as usize),
                Style::default().fg(colour),
            ))
        })
        .collect();
    f.render_widget(Paragraph::new(lines).style(theme::panel_style()), area);
}

/// A mixer flag: lit when it is on, an outline when it is not.
fn flag(on: bool, colour: Color) -> Style {
    if on {
        Style::default()
            .fg(Color::Black)
            .bg(colour)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(DIM)
    }
}

/// The message log, the spectrum and the meters, three columns across.
///
/// Below a width the columns stop being readable — a spectrum four cells wide
/// is not a spectrum — so a narrow panel drops back to the message log alone,
/// which is the one of the three that still says something at any size.
fn draw_monitor_columns(
    f: &mut Frame,
    area: Rect,
    events: &[InputEvent],
    ports: &[String],
    spectrum: &crate::spectrum::Spectrum,
) {
    /// Under this, three columns is worse than one.
    const MIN_FOR_COLUMNS: u16 = 60;
    if area.width < MIN_FOR_COLUMNS {
        draw_message_log(f, area, events, ports);
        return;
    }

    // The log gets the most: it is text and wraps badly, where the other two
    // are pictures that survive being squeezed.
    let log_w = area.width * 2 / 5;
    let spec_w = (area.width - log_w) / 2;
    let act_w = area.width - log_w - spec_w;

    let header = |f: &mut Frame, x: u16, w: u16, text: &str| {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("\u{2500}\u{2500} {text} "),
                Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
            )))
            .style(super::theme::panel_style()),
            Rect::new(x, area.y, w, 1),
        );
    };
    let body = |x: u16, w: u16| Rect::new(x, area.y + 1, w, area.height.saturating_sub(1));

    header(f, area.x, log_w, "MIDI");
    header(f, area.x + log_w, spec_w, "SPEC");
    header(f, area.x + log_w + spec_w, act_w, "ACTIVITY");
    if area.height < 2 {
        return;
    }

    // Vertical rules between the columns, so three pictures do not read as one
    // wide one.
    for col in [area.x + log_w - 1, area.x + log_w + spec_w - 1] {
        for row in area.y..area.y + area.height {
            f.render_widget(
                Paragraph::new(Span::styled("\u{2502}", Style::default().fg(DIM)))
                    .style(super::theme::panel_style()),
                Rect::new(col, row, 1, 1),
            );
        }
    }

    draw_message_log(f, body(area.x, log_w - 1), events, ports);
    draw_spectrum(f, body(area.x + log_w, spec_w - 1), spectrum);
    draw_activity(f, body(area.x + log_w + spec_w, act_w));
}

/// The message log on its own: newest at the bottom, like a terminal.
fn draw_message_log(f: &mut Frame, area: Rect, events: &[InputEvent], ports: &[String]) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let rows = area.height as usize;
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
        area,
    );
}

/// How many traces are kept. More than any panel is tall, so resizing taller
/// does not start from an empty stack.
const WAVE_TRACES: usize = 64;
/// How often a new trace is taken. Tied to the clock, not to the redraw, so
/// the stack drifts at the same speed whatever the frame rate is doing.
const WAVE_INTERVAL: Duration = Duration::from_millis(70);
/// How many rows a full-scale peak reaches above its own baseline. This is the
/// number that makes the picture: at 1 the traces never touch and it reads as a
/// bar chart, and too high turns the panel into a smear.
const WAVE_PEAK_ROWS: f32 = 5.0;
/// Sub-cell steps, filling from the bottom of a cell.
const WAVE_STEPS: [char; 8] = [
    '\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}', '\u{2585}', '\u{2586}', '\u{2587}', '\u{2588}',
];

/// The stack of past traces the WAVE tab draws.
///
/// Lives in the application rather than the view because it is a history: a
/// view that rebuilt it per frame would only ever have the present, which is
/// the one thing this picture is not about.
pub struct WaveHistory {
    /// Newest first. Each trace is `meter::WAVE_POINTS` magnitudes, 0..1.
    traces: VecDeque<Vec<f32>>,
    last: Instant,
}

impl Default for WaveHistory {
    fn default() -> Self {
        Self {
            traces: VecDeque::with_capacity(WAVE_TRACES),
            // Far enough back that the first frame takes a trace instead of
            // waiting out an interval on an empty panel.
            last: Instant::now() - WAVE_INTERVAL,
        }
    }
}

impl WaveHistory {
    /// Take a trace if enough time has passed. Called once per redraw.
    pub fn tick(&mut self) {
        self.tick_at(Instant::now())
    }

    fn tick_at(&mut self, now: Instant) {
        if now.duration_since(self.last) < WAVE_INTERVAL {
            return;
        }
        self.last = now;
        // Magnitude, not the signed sample: a ridge rises from its baseline,
        // and the sign of one decimated point is noise at this resolution.
        let trace: Vec<f32> = choz_engine::meter::meter()
            .wave()
            .iter()
            .map(|s| s.abs())
            .collect();
        if self.traces.len() == WAVE_TRACES {
            self.traces.pop_back();
        }
        self.traces.push_front(trace);
    }
}

/// The output's shape as a stack of traces: one ridge per moment, the newest
/// along the bottom, older ones climbing away from it.
///
/// Each ridge is drawn as a **line** with everything under it blanked, and the
/// ridges are painted back to front, so a loud moment hides the quiet ones
/// behind it. That occlusion is the whole effect — without it this is a pile of
/// overlapping curves, and with it the panel shows dynamics over time.
fn draw_wave(f: &mut Frame, area: Rect, history: &WaveHistory) {
    let (rows, cols) = (area.height as usize, area.width as usize);
    if rows == 0 || cols == 0 {
        return;
    }
    // Auto-gain across the whole stack, not per trace: normalising each trace
    // on its own would make silence as tall as a chord.
    let peak = history
        .traces
        .iter()
        .flat_map(|t| t.iter())
        .fold(0.02f32, |m, s| m.max(*s));

    let mut grid = vec![vec![(' ', DIM); cols]; rows];
    // Back to front: the oldest trace is furthest up and gets painted over.
    for (age, trace) in history.traces.iter().enumerate().take(rows).rev() {
        let baseline = rows - 1 - age;
        // Older traces recede. Not to the background — a trace that fades to
        // invisible takes its silhouette with it, and the silhouettes are what
        // give the stack depth.
        let shade = 1.0 - (age as f32 / rows as f32) * 0.75;
        let colour = Color::Rgb(
            (120.0 * shade) as u8,
            (200.0 * shade) as u8,
            (255.0 * shade) as u8,
        );
        for col in 0..cols {
            let amp = trace[col * trace.len() / cols.max(1)] / peak;
            let height = (amp.clamp(0.0, 1.0) * WAVE_PEAK_ROWS).min(baseline as f32);
            let full = height.floor() as usize;
            let top = baseline - full;
            // Everything between the ridge and its own baseline is blanked, so
            // whatever was drawn there by an older trace is hidden.
            for row in grid.iter_mut().take(baseline + 1).skip(top) {
                row[col] = (' ', DIM);
            }
            let step = ((height.fract() * 8.0) as usize).min(7);
            grid[top][col] = (WAVE_STEPS[step], colour);
        }
    }

    let lines: Vec<Line> = grid
        .into_iter()
        .map(|row| {
            Line::from(
                row.into_iter()
                    .map(|(ch, c)| Span::styled(ch.to_string(), Style::default().fg(c)))
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
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

    /// The colour key: it names what the mode colours by, in that colour, so a
    /// keyboard lit in six hues means something.
    #[test]
    fn the_keys_legend_names_the_colours() {
        let strips = |n: usize| -> Vec<MixerStrip> {
            (0..n)
                .map(|i| MixerStrip {
                    kind: StripKind::Tab,
                    label: format!("Synth{i}"),
                    gain: 1.0,
                    gain_r: 1.0,
                    link: true,
                    pan: 0.0,
                    mute: false,
                    solo: false,
                    active: false,
                    side: None,
                    dest: None,
                })
                .collect()
        };
        let mut k = KeyboardState::default();
        k.feed(
            &InputEvent::Note(NoteMsg {
                source: InputSource::Midi(0),
                channel: 2,
                on: true,
                note: 60,
                vel: 100,
            }),
            Some(1),
        );

        // One entry per tab, in that tab's own hue — the same offset
        // `key_colour` uses, or the legend would name the wrong colour.
        let line = colour_legend(KeyColor::Instrument, &k, &strips(2), &[]);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(
            text.contains("1:Synth0") && text.contains("2:Synth1"),
            "{text}"
        );
        assert_eq!(
            line.spans[1].style.fg,
            Some(hue_of(3)),
            "tab 1 is drawn in the colour its keys are"
        );

        // By channel it lists the ones actually arriving, not sixteen.
        let text: String = colour_legend(KeyColor::Channel, &k, &strips(2), &[])
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(text.contains("CH3") && !text.contains("CH1"), "{text}");

        // By input it lists every keyboard that is connected, playing or not —
        // the legend is also the answer to "is that thing plugged in".
        let ports = vec!["Keystation".to_string(), "Groovebox".to_string()];
        let line = colour_legend(KeyColor::Source, &k, &strips(2), &ports);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(
            text.contains("Keystation") && text.contains("Groovebox"),
            "{text}"
        );
        assert_eq!(
            line.spans[1].style.fg,
            Some(hue_of(9)),
            "the first port is drawn in the colour its notes are"
        );

        // The two modes must not answer with the same colour for the same
        // number, or reading one after the other means nothing.
        let lit = KeyLit {
            channel: 0,
            vel: 100,
            slot: Some(0),
            source: Some(InputSource::Midi(0)),
        };
        assert_ne!(
            key_colour(KeyColor::Source, &lit),
            key_colour(KeyColor::Instrument, &lit),
            "tab 1 and port 1 must not look alike"
        );

        // A note choz made itself came from no port, and says so.
        let own = KeyLit {
            channel: 0,
            vel: 100,
            slot: Some(0),
            source: None,
        };
        assert_eq!(key_colour(KeyColor::Source, &own), DIM);
    }

    /// An unknown port index must not panic the draw path.
    #[test]
    fn missing_port_names_degrade_to_an_index() {
        let label = source_label(InputSource::Midi(7), &[]);
        assert_eq!(label, "port 7");
    }

    fn render(events: &[InputEvent], ports: &[String], w: u16, h: u16) -> String {
        render_tab(events, ports, w, h, MonitorTab::Monitor)
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
        let wave = WaveHistory::default();
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| {
            draw_midi_monitor(f, f.area(), &[], &[], tab, state, mode, &spec, &wave, &[]);
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
        let wave = WaveHistory::default();
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| {
            draw_midi_monitor(
                f,
                f.area(),
                &[],
                &[],
                MonitorTab::Keys,
                state,
                mode,
                &spec,
                &wave,
                &[],
            );
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

    /// The black keys must land in the piano's 2-and-3 groups, or the row is a
    /// dotted line and not a keyboard. Checked on the glyphs, which is where
    /// the grouping actually comes from.
    #[test]
    fn the_black_keys_fall_into_two_and_three_groups() {
        let k = KeyboardState::default();
        let lines = keyboard_lines(&k, KeyColor::Channel, 200);
        let blacks: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();

        // One octave of the pattern, starting at C: C▐ D█ E▌ F▐ G█ A█ B▌.
        assert!(
            blacks.contains("\u{2590}\u{2588}\u{258C}\u{2590}\u{2588}\u{2588}\u{258C}"),
            "the 2-group then the 3-group: {blacks}"
        );
        // A white key with a black on each side is fully covered; one with a
        // black on neither side (E→F, B→C) leaves the gap that separates the
        // groups.
        assert_eq!(
            blacks.chars().filter(|c| *c == '\u{2588}').count(),
            21,
            "D, G and A of each full octave: {blacks}"
        );
    }

    /// Lit keys change colour — the whole point of the panel. Both colours of
    /// key, because a black key shares its cell with the white beneath it and
    /// that is exactly where a lit one could get lost.
    #[test]
    fn playing_a_key_changes_its_colour() {
        let idle = KeyboardState::default();
        let quiet = keyboard_lines(&idle, KeyColor::Channel, 200);

        // A white key: middle C.
        let mut k = KeyboardState::default();
        k.feed(&note_on(60, 0, 100), Some(0));
        let lit = keyboard_lines(&k, KeyColor::Channel, 200);
        let col = piano_whites().iter().position(|w| *w == 60).unwrap();
        assert_ne!(
            lit[1].spans[col].style.fg, quiet[1].spans[col].style.fg,
            "the white key body changed colour"
        );
        assert_ne!(
            lit[0].spans[col].style.bg, quiet[0].spans[col].style.bg,
            "and so did the part of it showing between the black keys"
        );

        // A black key: C#4. It shares its cells with C4 and D4, and must win
        // the foreground of at least one of them.
        let mut k = KeyboardState::default();
        k.feed(&note_on(61, 0, 100), Some(0));
        let lit = keyboard_lines(&k, KeyColor::Channel, 200);
        assert!(
            (col..=col + 1).any(|c| lit[0].spans[c].style.fg != quiet[0].spans[c].style.fg),
            "the lit black key took the cell it shares"
        );
    }

    /// Given the room, the whole piano is drawn — all 88 keys, A0 to C8.
    #[test]
    fn a_wide_panel_draws_the_whole_eighty_eight() {
        let whites = visible_whites(&KeyboardState::default(), 200);
        assert_eq!(whites.len(), 52, "52 white keys on an 88-key piano");
        assert_eq!(whites.first().copied(), Some(PIANO_LO), "starts at A0");
        assert_eq!(whites.last().copied(), Some(PIANO_HI), "ends at C8");
        // 88 keys means the 36 black ones are in range too.
        let blacks = (PIANO_LO..=PIANO_HI).filter(|n| is_black(*n)).count();
        assert_eq!(blacks + whites.len(), 88);
    }

    /// One column per white key, so the whole piano fits from 52 columns up —
    /// which is what makes an 80-column terminal enough.
    #[test]
    fn the_piano_fits_in_fifty_two_columns() {
        assert_eq!(visible_whites(&KeyboardState::default(), 52).len(), 52);
        assert_eq!(
            visible_whites(&KeyboardState::default(), 51).len(),
            51,
            "one short and it becomes a window, not a squeeze"
        );
    }

    /// Too narrow for the whole piano, the window holds over what is playing:
    /// a keyboard scrolled away from the notes answers nothing.
    #[test]
    fn a_narrow_keyboard_windows_onto_what_is_sounding() {
        let mut k = KeyboardState::default();
        k.feed(&note_on(24, 0, 100), Some(0)); // C1, near the bottom
        let whites = visible_whites(&k, 20);
        assert_eq!(whites.len(), 20);
        assert!(
            whites.contains(&24),
            "the note being played is on screen: {whites:?}"
        );

        let mut k = KeyboardState::default();
        k.feed(&note_on(103, 0, 100), Some(0)); // G7, near the top
        let whites = visible_whites(&k, 20);
        assert!(
            whites.contains(&103),
            "and so is one at the other end: {whites:?}"
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

    /// The spectrum draws where the tone is and marks the decades.
    ///
    /// Rendered straight into a known rect rather than through the tab: the
    /// spectrum is one of three columns now, and a test that has to re-derive
    /// the column arithmetic is testing the layout, not the analyser.
    #[test]
    fn the_spectrum_draws_a_tone_where_it_belongs() {
        use ratatui::{backend::TestBackend, Terminal};
        let mut spec = crate::spectrum::Spectrum::new();
        spec.set_sample_rate(48_000.0);
        let tone: Vec<f32> = (0..choz_engine::meter::SPECTRUM_POINTS)
            .map(|i| (std::f32::consts::TAU * 1000.0 * i as f32 / 48_000.0).sin())
            .collect();
        spec.analyse(&tone);

        let (w, h) = (62u16, 10u16);
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw_spectrum(f, f.area(), &spec)).unwrap();
        let buf = term.backend().buffer().clone();
        let rows: Vec<String> = (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect())
            .collect();
        let screen = rows.join("\n");
        assert!(screen.contains("1k"), "the decades are marked: {screen}");

        let plot_w = w as usize;
        let expect = spec.marker_col(1000.0, plot_w).unwrap();
        let height_of = |col: usize| {
            rows.iter()
                .filter(|r| {
                    r.chars()
                        .nth(col)
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

    /// Panels with no room for a plot must not panic — through the tab, which
    /// is where the column arithmetic can underflow.
    #[test]
    fn the_monitor_survives_a_panel_with_no_room() {
        use ratatui::{backend::TestBackend, Terminal};
        let spec = crate::spectrum::Spectrum::new();
        let wave = WaveHistory::default();
        for tab in MonitorTab::ALL {
            for h in [1u16, 2, 3, 4] {
                for w in [1u16, 4, 20, 59, 60, 61, 120] {
                    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
                    term.draw(|f| {
                        draw_midi_monitor(
                            f,
                            f.area(),
                            &[],
                            &[],
                            tab,
                            &KeyboardState::default(),
                            KeyColor::default(),
                            &spec,
                            &wave,
                            &[],
                        );
                    })
                    .unwrap();
                }
            }
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
                &WaveHistory::default(),
                &[],
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
        assert!(screen.contains("MONITOR"), "panel is titled:\n{screen}");
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

    /// Three tabs, and each one shows its own thing. MONITOR carries the log,
    /// the spectrum and the meters side by side, so all three headers are on
    /// the one screen.
    #[test]
    fn the_monitor_tab_carries_all_three_columns() {
        choz_engine::meter::meter().clear();
        let monitor = render_tab(&[], &[], 80, 10, MonitorTab::Monitor);
        for header in ["MIDI", "SPEC", "ACTIVITY"] {
            assert!(
                monitor.contains(header),
                "missing {header} column:\n{monitor}"
            );
        }
        assert!(
            monitor.contains("waiting for MIDI"),
            "the log column is still the messages:\n{monitor}"
        );
        assert!(
            monitor.contains("PEAK") && monitor.contains("RMS"),
            "the meters are on the same screen:\n{monitor}"
        );
        choz_engine::meter::meter().clear();
    }

    /// Too narrow for three columns, the log is what survives: a spectrum a few
    /// cells wide says nothing, and the messages still do.
    #[test]
    fn a_narrow_monitor_keeps_the_log_and_drops_the_columns() {
        let narrow = render_tab(&[], &[], 40, 10, MonitorTab::Monitor);
        assert!(
            narrow.contains("waiting for MIDI"),
            "the log survives:\n{narrow}"
        );
        assert!(
            !narrow.contains("ACTIVITY"),
            "the columns are gone, not squeezed:\n{narrow}"
        );
    }

    /// Chord naming: the shapes a player actually holds, including the ones
    /// that only differ by one note, and the inversions.
    #[test]
    fn chords_are_named_from_the_notes_held() {
        // Triads, in root position.
        assert_eq!(chord_name(&[60, 64, 67]).as_deref(), Some("C"));
        assert_eq!(chord_name(&[60, 63, 67]).as_deref(), Some("Cm"));
        assert_eq!(chord_name(&[60, 63, 66]).as_deref(), Some("Cdim"));
        assert_eq!(chord_name(&[60, 64, 68]).as_deref(), Some("Caug"));
        assert_eq!(chord_name(&[60, 65, 67]).as_deref(), Some("Csus4"));

        // A seventh must not be reported as the triad hiding inside it.
        assert_eq!(chord_name(&[60, 64, 67, 71]).as_deref(), Some("Cmaj7"));
        assert_eq!(chord_name(&[60, 64, 67, 70]).as_deref(), Some("C7"));
        assert_eq!(chord_name(&[60, 63, 67, 70]).as_deref(), Some("Cm7"));

        // Octaves and doublings fold away.
        assert_eq!(
            chord_name(&[36, 60, 64, 67, 72, 76]).as_deref(),
            Some("C"),
            "the same chord spread over three octaves is the same chord"
        );

        // Root not in the bass is an inversion, and says so.
        assert_eq!(chord_name(&[64, 67, 72]).as_deref(), Some("C/E"));
        assert_eq!(chord_name(&[67, 72, 76]).as_deref(), Some("C/G"));
    }

    /// A shape with no name gets no name. Guessing is worse than a blank.
    #[test]
    fn a_cluster_is_not_given_an_invented_name() {
        assert_eq!(chord_name(&[]), None, "nothing held");
        assert_eq!(chord_name(&[60]), None, "one note is not a chord");
        assert_eq!(chord_name(&[60, 61]), None, "a semitone is not a chord");
        assert_eq!(chord_name(&[60, 61, 62, 63]).as_deref(), None, "a cluster");
        // But a bare fifth is worth naming — it is what a hand plays.
        assert_eq!(chord_name(&[60, 67]).as_deref(), Some("C5"));
    }

    /// The chord readout is on the KEYS tab, and it names what is held.
    #[test]
    fn the_keys_tab_shows_the_chord_being_held() {
        let mut k = KeyboardState::default();
        for note in [60, 64, 67, 71] {
            k.feed(&note_on(note, 0, 100), Some(0));
        }
        let screen = render_keys(&k, KeyColor::Channel, MonitorTab::Keys, 70, 10);
        assert!(screen.contains("Cmaj7"), "the chord is named:\n{screen}");
        assert!(screen.contains("C4"), "and its notes are listed:\n{screen}");
    }

    /// One key down is named too, in American notation — it is the commonest
    /// thing on this panel and it always has a name.
    #[test]
    fn a_single_key_is_named_as_a_note() {
        for (note, expect) in [(60u8, "C4"), (69, "A4"), (61, "C#4"), (21, "A0")] {
            let mut k = KeyboardState::default();
            k.feed(&note_on(note, 0, 100), Some(0));
            let screen = render_keys(&k, KeyColor::Channel, MonitorTab::Keys, 70, 10);
            assert!(
                screen.contains(expect),
                "one key down should read {expect}:\n{screen}"
            );
        }
    }

    /// Nothing held is a dash, not a stale chord from the last thing played.
    #[test]
    fn the_chord_readout_empties_when_the_hands_come_off() {
        let mut k = KeyboardState::default();
        for note in [60, 64, 67] {
            k.feed(&note_on(note, 0, 100), Some(0));
        }
        assert!(render_keys(&k, KeyColor::Channel, MonitorTab::Keys, 70, 10).contains(" C "));
        for note in [60, 64, 67] {
            k.feed(&note_off(note), Some(0));
        }
        let screen = render_keys(&k, KeyColor::Channel, MonitorTab::Keys, 70, 10);
        assert!(
            !screen.contains("C4 E4 G4"),
            "the note list cleared:\n{screen}"
        );
    }

    /// The WAVE stack: a trace per interval, newest along the bottom, and the
    /// ridges occlude what is behind them.
    #[test]
    fn the_wave_stacks_traces_with_the_newest_at_the_bottom() {
        let mut wave = WaveHistory::default();
        let now = Instant::now();

        // Silence first, then a loud block: the loud trace must end up below
        // the quiet ones, because it arrived later.
        choz_engine::meter::meter().clear();
        wave.tick_at(now);
        let loud: Vec<f32> = (0..512).flat_map(|_| [0.9f32, 0.9]).collect();
        choz_engine::meter::meter().publish(&loud);
        for i in 1..4 {
            wave.tick_at(now + WAVE_INTERVAL * i);
        }
        assert_eq!(wave.traces.len(), 4, "one trace per interval");
        assert!(
            wave.traces[0][0] > wave.traces[3][0],
            "newest is first, and it is the loud one"
        );

        // A tick before the interval elapses takes nothing.
        wave.tick_at(now + WAVE_INTERVAL * 3);
        assert_eq!(wave.traces.len(), 4, "the interval gates the stack");

        choz_engine::meter::meter().clear();
    }

    /// The stack is bounded: a long session must not grow the buffer.
    #[test]
    fn the_wave_stack_is_a_fixed_budget() {
        let mut wave = WaveHistory::default();
        let now = Instant::now();
        for i in 0..(WAVE_TRACES * 2) {
            wave.tick_at(now + WAVE_INTERVAL * (i as u32 + 1));
        }
        assert_eq!(
            wave.traces.len(),
            WAVE_TRACES,
            "the oldest fall off the top"
        );
    }

    /// The ridges track amplitude and hide what is behind them — that
    /// occlusion is the picture.
    #[test]
    fn ridges_track_amplitude_and_occlude_what_is_behind() {
        let mut wave = WaveHistory::default();
        let now = Instant::now();
        // A ramp across the block, so the trace is quiet on the left and loud
        // on the right. A flat signal would draw a flat line and prove nothing.
        let ramp: Vec<f32> = (0..2048)
            .flat_map(|i| {
                let a = i as f32 / 2048.0;
                [a, a]
            })
            .collect();
        choz_engine::meter::meter().publish(&ramp);
        for i in 0..10 {
            wave.tick_at(now + WAVE_INTERVAL * (i + 1));
        }

        use ratatui::{backend::TestBackend, Terminal};
        let (w, h) = (40u16, 12u16);
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw_wave(f, f.area(), &wave)).unwrap();
        let buf = term.backend().buffer().clone();
        let rows: Vec<String> = (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect())
            .collect();
        let screen = rows.join("\n");

        // Topmost drawn cell of a column: the ridge of the trace in front.
        let ridge = |col: usize| {
            (0..h as usize).find(|&r| {
                rows[r]
                    .chars()
                    .nth(col)
                    .is_some_and(|c| WAVE_STEPS.contains(&c))
            })
        };
        let quiet = ridge(1).expect("a ridge on the quiet side");
        let loud = ridge(w as usize - 2).expect("a ridge on the loud side");
        assert!(
            loud < quiet,
            "the loud column rides higher (smaller row) than the quiet one: \
             loud={loud} quiet={quiet}\n{screen}"
        );

        // Under a ridge, down to the bottom, nothing from an older trace shows
        // through: that region belongs to the trace in front of it.
        for (row, text) in rows.iter().enumerate().skip(loud + 1) {
            let cell = text.chars().nth(w as usize - 2).unwrap();
            assert!(
                cell == ' ' || WAVE_STEPS.contains(&cell),
                "row {row} under the ridge should be blanked or a nearer \
                 ridge, got {cell:?}\n{screen}"
            );
        }
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
            (rects, _) = draw_midi_monitor(
                f,
                f.area(),
                &[],
                &[],
                MonitorTab::Monitor,
                &KeyboardState::default(),
                KeyColor::default(),
                &crate::spectrum::Spectrum::new(),
                &WaveHistory::default(),
                &[],
            );
        })
        .unwrap();
        assert_eq!(rects.len(), MonitorTab::ALL.len());
        assert_eq!(rects[0].0, MonitorTab::Monitor);
        assert!(rects[1].1.x > rects[0].1.x, "left to right");
        assert!(rects.iter().all(|(_, r)| r.height == 1));

        assert_eq!(MonitorTab::Monitor.next(), MonitorTab::Keys);
        assert_eq!(MonitorTab::Mixer.next(), MonitorTab::Monitor, "it wraps");
    }
}
