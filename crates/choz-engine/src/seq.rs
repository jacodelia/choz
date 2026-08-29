//! Step sequencer: an MMT-8 in the RACK.
//!
//! # What it is
//!
//! Eight tracks, sixteen steps, eight parts and a song chain — the shape of an
//! Alesis MMT-8, which is a *multitrack recorder* rather than a drum machine:
//! a track is a note pointed at the tab's own instrument, a part is a pattern
//! of them, and the song is the order the parts play in.
//!
//! # Why it lives beside the arpeggiator
//!
//! For the same reason [`crate::arp`] does: a note is not audio, so there is no
//! slot in `FxProcessor::process_block` to put one in. Both are note
//! generators, so this one emits [`ArpEvent`] rather than a type of its own —
//! the rack already knows how to send those, and a step that reaches a tab with
//! its arpeggiator on is handed to the arpeggiator instead of to the
//! instrument. That is the whole integration: the sequencer plays the keys, the
//! arpeggiator does to them what it does to a player's.
//!
//! # What that costs
//!
//! The UI loop is the clock, as it is for the arpeggiator. Following a rolling
//! transport it counts steps off the song position, so nothing accumulates; on
//! its own clock a step lands within one pass of the loop.
//!
//! ponytail: no per-step velocity and no lookahead scheduling. Both are the
//! arpeggiator's shape (`ArpEvent::at` already carries a sample), so the
//! upgrade path is to schedule against the grid the way `Arp::next_grid_step`
//! does rather than to change anything here.

use crate::arp::{ArpEvent, TimeDiv};
use std::time::{Duration, Instant};

/// Tracks a part holds — the MMT-8's eight.
pub const TRACKS: usize = 8;
/// Steps a part is long. Sixteen: one bar of sixteenths, and what fits in the
/// columns the RACK has.
pub const STEPS: usize = 16;
/// Parts a project holds, `A`..`H`.
pub const PARTS: usize = 8;

/// How hard a step hits when nothing has strayed from what was written.
const STEP_VEL: u8 = 100;

/// How long a step is held, as a share of the step — the arpeggiator's default
/// gate: long enough to be a note, short enough that two steps on one track
/// retrigger.
const GATE: f32 = 0.5;

/// What [`SeqSettings::random`] at 1 is worth, in each of the things it moves.
///
/// **Never the pitch.** An octave jump was the first thing tried here and it is
/// the one deviation this box must not have: a lane's note is a choice, and a
/// sequencer that answers a written C2 with a C3 is playing something nobody
/// selected. What strays instead is *when* and *how often* — a note repeated
/// inside its own step, a hit nudged off the grid, a hit on a step nothing was
/// written on. Every one of them plays a note that is already in the pattern.
const VEL_RANGE: f32 = 55.0;
const GATE_RANGE: f32 = 0.8;
/// How likely an altered step is to repeat itself rather than hit once, and how
/// many times it may repeat at the top of the knob.
const RATCHET_CHANCE: f32 = 0.45;
const RATCHET_MAX: u32 = 4;
/// How far off its own boundary a hit may land, as a share of the step. Kept
/// under a third: past that it stops reading as *this* step played loosely and
/// starts reading as the next one played early.
const NUDGE_RANGE: f32 = 0.3;
/// How likely a step with nothing written on it is to speak anyway — the ghost
/// note, which is the whole of "at times other than the ones selected".
const GHOST_CHANCE: f32 = 0.4;

/// A gate under this is a click rather than a note; over it, two steps on one
/// track stop retriggering.
const MIN_GATE: f32 = 0.1;
const MAX_GATE: f32 = 0.95;

/// A part: one bitmask per track, a bit per step.
///
/// A mask rather than `[bool; 16]` so a project writes eight numbers per part
/// instead of a hundred and twenty-eight booleans.
pub type Pattern = [u16; TRACKS];

/// A part's letter, `A`..`H`.
pub fn part_name(part: usize) -> char {
    (b'A' + (part % PARTS) as u8) as char
}

/// A track's letter, `A`..`H`.
///
/// The grid names its rows by position rather than by the note they play: a
/// track is a *lane* — the note on it is a setting, and one changed while
/// reading a pattern renamed the row under the reader's eye. The note is a
/// click away, on the letter itself.
pub fn track_name(track: usize) -> char {
    (b'A' + (track % TRACKS) as u8) as char
}

/// Past this the off-beat swallows the on-beat. Same ceiling the arpeggiator's
/// swing has, for the same reason.
pub const MAX_SWING: f32 = 0.75;

/// `60` is `C4`, as everywhere else in choz.
pub fn note_name(note: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!("{}{}", NAMES[note as usize % 12], note as i32 / 12 - 1)
}

/// What the sequencer is, as the project stores it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SeqSettings {
    /// Drawn and playable at all. Off, a tab behaves as it did before this
    /// existed — and off is what every tab starts as.
    pub on: bool,
    /// How long a step lasts. The arpeggiator's divisions, because there is one
    /// clock in this program and it counts in these.
    pub div: TimeDiv,
    /// The part being edited, and the one that plays when there is no song.
    pub part: usize,
    /// The order the parts play in. Empty — the usual — loops the part being
    /// edited, which is what a box does while you are writing one.
    pub song: Vec<usize>,
    /// The note each track plays.
    pub notes: [u8; TRACKS],
    pub parts: [Pattern; PARTS],
    /// How far the off-beats are pushed back, 0..[`MAX_SWING`], as a share of
    /// one step. The whole of what makes a written grid stop sounding like a
    /// grid — and the same number the arpeggiator's SWING is, so the two
    /// controls on this panel swing by the same amount.
    #[serde(default)]
    pub swing: f32,
    /// **How far** a step is allowed to stray from what was written, 0..1.
    ///
    /// The size of the deviation, not how often it happens — velocity, how long
    /// the gate is held, and how far the pitch jumps. At 0 there is no
    /// deviation at all whatever [`Self::prob`] says; at 1 the range is the
    /// widest the box offers.
    #[serde(default)]
    pub random: f32,
    /// **How often** that deviation is applied, 0..1.
    ///
    /// Rolled once per step: at 0 the pattern plays exactly as written, and as
    /// it comes up the steps [`Self::random`] is allowed to alter get more
    /// frequent — which is what lets a sixteen-step loop breathe instead of
    /// repeating.
    #[serde(default)]
    pub prob: f32,
}

impl Default for SeqSettings {
    fn default() -> Self {
        Self {
            on: false,
            div: TimeDiv::Sixteenth,
            part: 0,
            song: Vec::new(),
            // C2 up a scale's worth of semitones: eight tracks that are eight
            // different notes on whatever the tab is holding, so a new
            // sequencer plays something recognisable the moment steps go in.
            notes: [36, 38, 40, 41, 43, 45, 47, 48],
            parts: [[0; TRACKS]; PARTS],
            swing: 0.0,
            random: 0.0,
            prob: 0.0,
        }
    }
}

impl SeqSettings {
    /// How many steps one bar is, in this sequencer's own resolution: the
    /// transport's time signature read at the step length.
    ///
    /// This is what makes the meter mean something rather than decorate the
    /// display — a part in 3/4 at 1/16 loops after twelve steps, and the four
    /// past the end are not in the bar at all. Capped at [`STEPS`], which is
    /// what the grid holds: a 7/4 bar at 1/16 is twenty-eight steps, and the
    /// honest answer there is "as much of it as there is room for".
    pub fn bar_steps(&self) -> usize {
        let (num, den) = choz_ports::transport().time_signature();
        // A bar in quarter notes: four beats of an eighth is two quarters.
        let quarters = num.max(1) as f32 * 4.0 / den.max(1) as f32;
        let steps = (quarters / self.div.quarters()).round() as usize;
        steps.clamp(1, STEPS)
    }

    pub fn step_on(&self, track: usize, step: usize) -> bool {
        self.parts[self.part.min(PARTS - 1)][track.min(TRACKS - 1)] & (1 << step.min(STEPS - 1))
            != 0
    }

    /// Steps written across every track of every part — what the display calls
    /// the event count, and what says whether ERASE has anything to do.
    pub fn events(&self) -> u32 {
        self.parts
            .iter()
            .flat_map(|p| p.iter())
            .map(|t| t.count_ones())
            .sum()
    }
}

/// One tab's sequencer: the pattern, where the playhead is, and where the
/// cursor is.
#[derive(Debug, Clone)]
pub struct Seq {
    pub settings: SeqSettings,
    /// Rolling. Not saved: a project that loaded playing would start making
    /// noise before anybody asked it to.
    playing: bool,
    /// Writing what is played into the pattern.
    rec: bool,
    /// The step that last fired, so the display and the recorder agree on where
    /// "now" is.
    step: usize,
    /// Where in the song chain the playhead is.
    song_pos: usize,
    /// The step the editing cursor is on, as `(track, step)`.
    pub cursor: (usize, usize),
    /// When the next step is due, and when the notes out now are released.
    next_step: Option<Instant>,
    off_at: Option<Instant>,
    /// The notes this sequencer started and has not stopped yet.
    sounding: Vec<u8>,
    /// The transport step last fired while following one, so the same one is
    /// never played twice.
    grid: Option<i64>,
    /// xorshift, so RANDOM and PROB are a sequence rather than a surprise —
    /// the same generator the arpeggiator's `Random` mode runs on.
    rng: u32,
    /// Hits this step still owes: a ratchet's repeats, and the first hit itself
    /// when RANDOM nudged it off the boundary.
    ///
    /// A queue rather than one `at` per event, because both clocks have to work
    /// the same way: the transport-following one could carry a sample on
    /// `ArpEvent::at`, and the free-running one has no timeline to carry.
    /// Reused, never re-allocated — this runs on a host's audio thread now.
    pending: Vec<Repeat>,
    /// The notes those repeats play. One list for the step: a ratchet repeats
    /// the chord, it does not pick a different note each time.
    pending_notes: Vec<u8>,
}

/// One hit a step owes: when it starts and how long it is held.
#[derive(Debug, Clone, Copy)]
struct Repeat {
    at: Instant,
    hold: Duration,
    vel: u8,
}

impl Default for Seq {
    fn default() -> Self {
        Self {
            settings: SeqSettings::default(),
            playing: false,
            rec: false,
            step: 0,
            song_pos: 0,
            cursor: (0, 0),
            next_step: None,
            off_at: None,
            sounding: Vec::new(),
            grid: None,
            // Any seed but zero: xorshift started at zero stays there, and a
            // PROB knob that never fires is worse than one that is not here.
            rng: 0x9E37_79B9,
            pending: Vec::new(),
            pending_notes: Vec::new(),
        }
    }
}

/// The snapshot the panel draws from. `Copy`-cheap except for the song, which
/// is borrowed.
#[derive(Debug, Clone, Copy)]
pub struct SeqView<'a> {
    pub settings: &'a SeqSettings,
    pub playing: bool,
    pub rec: bool,
    /// Where the playhead is, drawn only while it is rolling.
    pub step: usize,
    pub cursor: (usize, usize),
    /// How many of the sixteen cells are inside the bar — see
    /// [`SeqSettings::bar_steps`]. The rest are drawn, and never played.
    pub bar: usize,
    /// Whether the arrows are on this box at all.
    pub focused: bool,
}

impl Seq {
    pub fn new(settings: SeqSettings) -> Self {
        Self {
            settings,
            ..Self::default()
        }
    }

    pub fn is_on(&self) -> bool {
        self.settings.on
    }

    /// Whether it has anything to play — what makes the event loop come back
    /// sooner, the same way the arpeggiator does.
    pub fn running(&self) -> bool {
        self.settings.on && self.playing
    }

    pub fn view(&self) -> SeqView<'_> {
        SeqView {
            settings: &self.settings,
            playing: self.playing,
            rec: self.rec,
            step: self.step,
            cursor: self.cursor,
            bar: self.settings.bar_steps(),
            focused: false,
        }
    }

    // ── Transport ──────────────────────────────────────────────────────────

    pub fn play(&mut self) {
        if !self.settings.on {
            self.settings.on = true;
        }
        self.playing = true;
        // One before the top: the clock fires the step *after* this one, so
        // starting at 0 would skip the downbeat — the one step a pattern is
        // most likely to have something on.
        self.step = self.settings.bar_steps() - 1;
        self.song_pos = 0;
        self.grid = None;
        self.next_step = Some(Instant::now());
        if let Some(first) = self.settings.song.first() {
            self.settings.part = (*first).min(PARTS - 1);
        }
    }

    /// Stop, and take the notes out with it. Recording stops too: a REC left
    /// armed writes the next thing played into a pattern nobody was looking at.
    pub fn stop(&mut self, out: &mut Vec<ArpEvent>) {
        self.playing = false;
        self.rec = false;
        self.step = 0;
        self.song_pos = 0;
        self.grid = None;
        self.next_step = None;
        self.silence_all(out);
    }

    pub fn toggle_play(&mut self, out: &mut Vec<ArpEvent>) {
        if self.playing {
            self.stop(out);
        } else {
            self.play();
        }
    }

    /// Arm (or disarm) recording. Arming starts the transport: a recorder that
    /// is armed and not rolling records nothing, which is a button that lies.
    pub fn toggle_rec(&mut self) {
        self.rec = !self.rec;
        if self.rec && !self.playing {
            self.play();
        }
    }

    /// Everything up, and nothing left running — PANIC, and a tab whose
    /// instrument changed under it.
    pub fn reset(&mut self, out: &mut Vec<ArpEvent>) {
        self.stop(out);
    }

    pub fn silence(&mut self, out: &mut Vec<ArpEvent>) {
        for note in self.sounding.drain(..) {
            out.push(ArpEvent::Off { note, at: 0 });
        }
        self.off_at = None;
    }

    /// Everything sounding, and everything about to: what STOP and a tab whose
    /// instrument changed under it both need. A repeat left in the queue would
    /// otherwise speak one tick after the transport was stopped.
    fn silence_all(&mut self, out: &mut Vec<ArpEvent>) {
        self.pending.clear();
        self.pending_notes.clear();
        self.silence(out);
    }

    // ── Editing ────────────────────────────────────────────────────────────

    /// Move the cursor, wrapping the way a grid does: past the last step is the
    /// first step of the next track, which is how a pattern is walked.
    pub fn move_cursor(&mut self, dt: isize, ds: isize) {
        let (t, s) = self.cursor;
        let track = (t as isize + dt).rem_euclid(TRACKS as isize) as usize;
        let step = (s as isize + ds).rem_euclid(STEPS as isize) as usize;
        self.cursor = (track, step);
    }

    pub fn toggle_step(&mut self, track: usize, step: usize) {
        if track >= TRACKS || step >= STEPS {
            return;
        }
        let part = self.settings.part.min(PARTS - 1);
        self.settings.parts[part][track] ^= 1 << step;
    }

    /// Take a step off, whether or not it was on — the right button's gesture.
    ///
    /// Not `toggle_step`: dragging the right button across a run of steps with
    /// a toggle behind it writes the gaps back in, which is the opposite of
    /// erasing. Clearing is idempotent, so a drag erases what it crosses.
    pub fn clear_step(&mut self, track: usize, step: usize) {
        if track >= TRACKS || step >= STEPS {
            return;
        }
        let part = self.settings.part.min(PARTS - 1);
        self.settings.parts[part][track] &= !(1 << step);
    }

    /// Point a track at a note. What the grid's letter opens a keyboard for.
    pub fn set_note(&mut self, track: usize, note: u8) {
        if track < TRACKS {
            self.settings.notes[track] = note;
        }
    }

    /// How hard an altered step hits and how long it is held, as a share of
    /// whatever slice of the step it gets.
    ///
    /// Both scale to nothing at `random` 0 and to the box's widest at 1, so the
    /// knob reads as one range and not as two controls sharing a number. The
    /// pitch is not here and never will be — see [`VEL_RANGE`].
    fn deviation(&mut self, random: f32) -> (u8, f32) {
        // Both ways from what was written: a deviation that only ever went one
        // way is a setting, not a variation.
        let bipolar = |r: f32| (r - 0.5) * 2.0;
        let vel = STEP_VEL as f32 + bipolar(self.rand()) * random * VEL_RANGE;
        let vel = (vel.round().clamp(1.0, 127.0)) as u8;
        let gate = GATE * (1.0 + bipolar(self.rand()) * random * GATE_RANGE);
        (vel, gate.clamp(MIN_GATE, MAX_GATE))
    }

    /// The next number out of the xorshift, as 0..1.
    fn rand(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x >> 8) as f32 / (1 << 24) as f32
    }

    /// Transpose the cursor's track. A track *is* a note here, so this is the
    /// only tuning the box has.
    pub fn transpose(&mut self, semitones: i16) {
        let track = self.cursor.0.min(TRACKS - 1);
        let note = self.settings.notes[track] as i16 + semitones;
        self.settings.notes[track] = note.clamp(0, 127) as u8;
    }

    /// Append the part to the song chain — the MMT-8's way of writing one:
    /// press the parts in the order they should play.
    pub fn chain(&mut self, part: usize) {
        if self.settings.song.len() < 64 {
            self.settings.song.push(part.min(PARTS - 1));
        }
    }

    /// Wipe the part being edited, or the song chain when there is one. The
    /// destructive button is the one thing here that asks what mode it is in:
    /// on a box with a chain, ERASE nearly always means the chain.
    pub fn erase(&mut self) {
        if !self.settings.song.is_empty() {
            self.settings.song.clear();
            return;
        }
        let part = self.settings.part.min(PARTS - 1);
        self.settings.parts[part] = [0; TRACKS];
    }

    /// A note played while REC is armed goes into the pattern, quantised to the
    /// step the playhead is on — the MMT-8's auto-correct, which is on because
    /// software timing on the UI thread is not worth recording unquantised.
    ///
    /// Which track it lands on: the one already tuned to that note, so playing
    /// the tracks back writes to where they came from. Failing that, the
    /// cursor's track — retuned to the note when it is empty, so a fresh
    /// pattern takes whatever is played into it, and left alone when it is not,
    /// so a take never retunes work that is already there.
    pub fn record(&mut self, note: u8) -> bool {
        if !self.rec || !self.playing {
            return false;
        }
        let track = match self.settings.notes.iter().position(|n| *n == note) {
            Some(t) => t,
            None => {
                let t = self.cursor.0.min(TRACKS - 1);
                let part = self.settings.part.min(PARTS - 1);
                if self.settings.parts[part][t] == 0 {
                    self.settings.notes[t] = note;
                }
                t
            }
        };
        let part = self.settings.part.min(PARTS - 1);
        self.settings.parts[part][track] |= 1 << self.step.min(STEPS - 1);
        true
    }

    // ── The clock ──────────────────────────────────────────────────────────

    /// Advance to `now`, appending what has to be sent.
    ///
    /// Takes the instant rather than reading the clock, so the whole thing is
    /// testable without sleeping — and so the engine can drive it the day the
    /// note generators move off the UI thread.
    pub fn tick(&mut self, now: Instant, out: &mut Vec<ArpEvent>) {
        if !self.settings.on || !self.playing {
            self.silence(out);
            return;
        }
        // The gate closes on its own schedule: a step with nothing on the next
        // one still has to let go of its notes.
        if self.off_at.is_some_and(|at| now >= at) {
            self.silence(out);
        }
        // Then whatever this step still owes — a ratchet's repeats, or a first
        // hit RANDOM pushed off the boundary. Before the step logic, because
        // both belong to the step that is already running.
        self.play_pending(now, out);
        // Following the transport, the step number comes from the song position
        // rather than from counting durations — so a stall skips a step instead
        // of firing a burst to catch up, and nothing drifts.
        if let Some((index, frac)) = self.grid_step() {
            if self.grid == Some(index) {
                return;
            }
            // SWING, against a grid whose steps are counted rather than timed:
            // the off-beats are simply not due until they are `swing` of a step
            // late. Nothing is dropped — the step still fires, further in.
            if index.rem_euclid(2) == 1 && frac < self.swing_offset() as f64 {
                return;
            }
            let first = self.grid.is_none();
            self.grid = Some(index);
            let step = index.rem_euclid(self.settings.bar_steps() as i64) as usize;
            // A wrap of the bar is a wrap of the part, whoever is counting —
            // but landing on step 0 because that is where the transport already
            // was is not a wrap, it is where the sequencer came in.
            if step == 0 && !first {
                self.advance_song();
            }
            self.fire(step, now, out);
            return;
        }
        // Its own clock: a step is due when enough time has gone by.
        self.grid = None;
        let Some(due) = self.next_step else {
            self.next_step = Some(now);
            return;
        };
        if now < due {
            return;
        }
        let step = (self.step + 1) % self.settings.bar_steps();
        if step == 0 {
            self.advance_song();
        }
        // SWING: the step about to play decides how long it is. An even step
        // sits on the beat and holds on longer, the odd one after it makes the
        // time back — so a pair always adds up to two steps and the bar keeps
        // its length however far the swing is pushed.
        let sw = self.swing_offset();
        let len = self.step_len().mul_f32(if step.is_multiple_of(2) {
            1.0 + sw
        } else {
            1.0 - sw
        });
        // From `due`, not from `now`: a late tick must not push the grid, or a
        // busy UI thread would drag the tempo down over time — and never so far
        // behind that the next tick fires a burst to catch up.
        self.next_step = Some(if due + len < now {
            now + len
        } else {
            due + len
        });
        self.fire(step, now, out);
    }

    /// Play step `step` of the current part.
    fn fire(&mut self, step: usize, now: Instant, out: &mut Vec<ArpEvent>) {
        self.silence(out);
        self.pending.clear();
        self.step = step;
        let part = self.settings.part.min(PARTS - 1);
        // The two knobs are one gesture: PROB decides **whether** this step is
        // played as written, RANDOM decides **how far** it goes if it is not.
        // Rolled once for the step and not once per note, so a chord strays
        // together — three notes each wandering off on their own is not a
        // variation on a chord, it is three chords.
        let random = self.settings.random.clamp(0.0, 1.0);
        let prob = self.settings.prob.clamp(0.0, 1.0);
        let altered = random > 0.0 && prob > 0.0 && self.rand() < prob;

        // What this step plays. Written first; failing that, and only on an
        // altered step, one note borrowed from elsewhere in the pattern — which
        // is how a hit lands at a time nobody selected without ever sounding a
        // note nobody selected.
        self.pending_notes.clear();
        for track in 0..TRACKS {
            if self.settings.parts[part][track] & (1 << step) != 0 {
                self.pending_notes.push(self.settings.notes[track]);
            }
        }
        let ghost = self.pending_notes.is_empty();
        if ghost {
            if !altered || self.rand() >= random * GHOST_CHANCE {
                self.off_at = None;
                return;
            }
            let Some(note) = self.pick_written(part) else {
                self.off_at = None;
                return;
            };
            self.pending_notes.push(note);
        }

        let (vel, gate) = match altered {
            true => self.deviation(random),
            false => (STEP_VEL, GATE),
        };
        let len = self.step_len();
        // A ratchet divides the step; a nudge moves the whole of it. Neither
        // touches which notes play.
        let reps = if altered { self.ratchet(random) } else { 1 };
        let nudge = match altered && reps == 1 {
            true => len.mul_f32(self.rand() * random * NUDGE_RANGE),
            false => Duration::ZERO,
        };
        let sub = len / reps;
        let hold = sub.mul_f32(gate);
        for r in 0..reps {
            self.pending.push(Repeat {
                at: now + nudge + sub * r,
                hold,
                vel,
            });
        }
        // Nothing is played here: the first hit goes out through the same queue
        // as its repeats, so a nudged step is late by the same code path that
        // makes a ratchet's second hit late.
        self.off_at = None;
        self.play_pending(now, out);
    }

    /// Play whatever is due, and let go of what its hold has run out on.
    fn play_pending(&mut self, now: Instant, out: &mut Vec<ArpEvent>) {
        while self.pending.first().is_some_and(|r| now >= r.at) {
            let hit = self.pending.remove(0);
            // The previous repeat, if its hold outlasted the gap.
            self.silence(out);
            for note in self.pending_notes.iter().copied() {
                out.push(ArpEvent::On {
                    note,
                    vel: hit.vel,
                    at: 0,
                });
                self.sounding.push(note);
            }
            // What `GateSource::Seq` listens to — published on the hits that
            // speak, so an effect wired to the sequencer opens on the pattern
            // rather than on the clock behind it.
            if !self.pending_notes.is_empty() {
                crate::fx_chain::seq_hit();
            }
            self.off_at = Some(hit.at + hit.hold);
        }
    }

    /// How many times an altered step hits. One most of the time: a sequencer
    /// that ratcheted every altered step would be a drum roll with a pattern
    /// behind it.
    fn ratchet(&mut self, random: f32) -> u32 {
        if self.rand() >= random * RATCHET_CHANCE {
            return 1;
        }
        let most = 2 + ((RATCHET_MAX - 2) as f32 * random).round() as u32;
        2 + (self.rand() * (most - 1) as f32) as u32 % (most - 1)
    }

    /// A note this part already plays, drawn from the lanes that have something
    /// written on them. `None` for an empty part, which has nothing to borrow.
    ///
    /// This is the rule the whole of RANDOM now obeys: it may move *when* a
    /// note sounds and *how often*, never *which*.
    fn pick_written(&mut self, part: usize) -> Option<u8> {
        let lanes = (0..TRACKS)
            .filter(|t| self.settings.parts[part][*t] != 0)
            .count();
        if lanes == 0 {
            return None;
        }
        let k = (self.rand() * lanes as f32) as usize % lanes;
        (0..TRACKS)
            .filter(|t| self.settings.parts[part][*t] != 0)
            .nth(k)
            .map(|t| self.settings.notes[t])
    }

    /// Move to the next part of the song chain, if there is one.
    fn advance_song(&mut self) {
        if self.settings.song.is_empty() {
            return;
        }
        self.song_pos = (self.song_pos + 1) % self.settings.song.len();
        self.settings.part = self.settings.song[self.song_pos].min(PARTS - 1);
    }

    fn step_len(&self) -> Duration {
        let bpm = choz_ports::transport().bpm().max(1.0);
        Duration::from_secs_f32(60.0 / bpm * self.settings.div.quarters())
    }

    /// How far into a step the off-beats are pushed, as a share of one step.
    ///
    /// The knob itself, on the arpeggiator's scale — `Arp::next_grid_step`
    /// shifts an odd step by `step * swing` and nothing else. It was half that
    /// here, which is why the same reading of the same control was audible on
    /// one box and not on the other.
    fn swing_offset(&self) -> f32 {
        self.settings.swing.clamp(0.0, MAX_SWING)
    }

    /// Which step of the transport's grid the playhead is on and how far into
    /// it the transport is, or `None` when there is no transport rolling to
    /// follow.
    fn grid_step(&self) -> Option<(i64, f64)> {
        let transport = choz_ports::transport();
        if !transport.playing() {
            return None;
        }
        let step_q = self.settings.div.quarters() as f64;
        if step_q <= 0.0 {
            return None;
        }
        let pos = transport.ppq() / step_q;
        Some((pos.floor() as i64, pos - pos.floor()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(seq: &Seq, track: usize, step: usize) -> bool {
        seq.settings.step_on(track, step)
    }

    /// A step written on track 0 plays its note, and the gate closes it.
    #[test]
    fn a_step_plays_its_note_and_lets_go() {
        let _g = crate::test_locks::transport();
        let mut seq = Seq::new(SeqSettings {
            on: true,
            ..Default::default()
        });
        seq.toggle_step(0, 1);
        seq.play();
        let note = seq.settings.notes[0];

        let t0 = Instant::now();
        let mut out = Vec::new();
        // Step 0 is empty, so the first due step says nothing…
        seq.tick(t0 + Duration::from_secs(1), &mut out);
        assert!(out.is_empty(), "step 0 has nothing on it: {out:?}");
        // …and the next one plays the note that is there.
        seq.tick(t0 + Duration::from_secs(2), &mut out);
        assert_eq!(
            out,
            vec![ArpEvent::On {
                note,
                vel: 100,
                at: 0
            }]
        );

        out.clear();
        seq.tick(t0 + Duration::from_secs(3), &mut out);
        assert!(
            out.contains(&ArpEvent::Off { note, at: 0 }),
            "the gate has to close: {out:?}"
        );
    }

    /// Stopping takes every sounding note with it — a sequencer that forgets a
    /// note-off is a stuck note no amount of stopping will clear.
    #[test]
    fn stop_releases_what_is_sounding() {
        let _g = crate::test_locks::transport();
        let mut seq = Seq::new(SeqSettings {
            on: true,
            ..Default::default()
        });
        seq.toggle_step(0, 1);
        seq.play();
        let mut out = Vec::new();
        let t0 = Instant::now();
        seq.tick(t0 + Duration::from_secs(1), &mut out);
        seq.tick(t0 + Duration::from_secs(2), &mut out);
        out.clear();
        seq.stop(&mut out);
        assert_eq!(out.len(), 1, "one note out, one note off: {out:?}");
        assert!(matches!(out[0], ArpEvent::Off { .. }));
        assert!(!seq.rec, "STOP disarms REC");
    }

    /// A note played while armed lands on the track tuned to it, at the step
    /// the playhead is on.
    #[test]
    fn record_writes_the_playhead_step() {
        let mut seq = Seq::new(SeqSettings {
            on: true,
            ..Default::default()
        });
        seq.play();
        seq.toggle_rec();
        seq.step = 5;
        let note = seq.settings.notes[3];
        assert!(seq.record(note));
        assert!(on(&seq, 3, 5), "the track tuned to that note takes it");

        // A note no track is tuned to takes the cursor's track, and tunes it
        // because that track is empty.
        seq.cursor = (6, 0);
        assert!(seq.record(21));
        assert_eq!(seq.settings.notes[6], 21);
        assert!(on(&seq, 6, 5));

        // …but a track with steps already on it is not retuned under them.
        seq.cursor = (3, 0);
        assert!(seq.record(99));
        assert_eq!(seq.settings.notes[3], note, "track 3 keeps its note");
    }

    /// Disarmed, nothing is written: REC is the difference between playing
    /// along and recording.
    #[test]
    fn record_does_nothing_while_disarmed() {
        let mut seq = Seq::new(SeqSettings {
            on: true,
            ..Default::default()
        });
        seq.play();
        assert!(!seq.record(60));
        assert_eq!(seq.settings.events(), 0);
    }

    /// The song chain moves the part on every wrap of the bar, and loops.
    #[test]
    fn the_song_walks_its_parts() {
        let mut seq = Seq::new(SeqSettings {
            on: true,
            song: vec![2, 5],
            ..Default::default()
        });
        seq.play();
        assert_eq!(seq.settings.part, 2, "PLAY starts at the top of the song");
        seq.advance_song();
        assert_eq!(seq.settings.part, 5);
        seq.advance_song();
        assert_eq!(seq.settings.part, 2, "and it loops");
    }

    /// ERASE clears the chain first and the pattern second — the chain is what
    /// it nearly always means once there is one.
    #[test]
    fn erase_takes_the_chain_before_the_pattern() {
        let mut seq = Seq::default();
        seq.toggle_step(0, 0);
        seq.chain(1);
        seq.erase();
        assert!(seq.settings.song.is_empty());
        assert!(on(&seq, 0, 0), "the pattern survives the first ERASE");
        seq.erase();
        assert!(!on(&seq, 0, 0));
    }

    /// The time signature is what a bar is long, in the sequencer's own
    /// resolution — the whole point of choosing one.
    #[test]
    fn the_bar_is_the_time_signature_at_the_step_length() {
        // The signature is one global, and every other test that reads it takes
        // this lock: without it the two move under each other.
        let _g = crate::test_locks::transport();
        let t = choz_ports::transport();
        let mut seq = Seq::default();

        t.set_time_signature(4, 4);
        assert_eq!(
            seq.settings.bar_steps(),
            16,
            "4/4 at 1/16 is the whole grid"
        );
        t.set_time_signature(3, 4);
        assert_eq!(seq.settings.bar_steps(), 12);
        t.set_time_signature(7, 8);
        assert_eq!(seq.settings.bar_steps(), 14);

        // A bar longer than the grid is as much of it as there is room for,
        // rather than a wrap nobody can see.
        t.set_time_signature(7, 4);
        assert_eq!(seq.settings.bar_steps(), STEPS);

        // And the quantisation moves it too: eighths are half the steps.
        t.set_time_signature(4, 4);
        seq.settings.div = TimeDiv::Eighth;
        assert_eq!(seq.settings.bar_steps(), 8);
        t.set_time_signature(4, 4);
    }

    /// The playhead wraps on the bar, not on the width of the grid: the steps
    /// past the end of a short bar are never played.
    #[test]
    fn the_playhead_wraps_on_the_bar() {
        let _g = crate::test_locks::transport();
        let t = choz_ports::transport();
        t.set_time_signature(3, 4);
        let mut seq = Seq::new(SeqSettings {
            on: true,
            ..Default::default()
        });
        // A step past the end of a 12-step bar, and one inside it.
        seq.toggle_step(0, 13);
        seq.toggle_step(1, 2);
        seq.play();

        let t0 = Instant::now();
        let mut out = Vec::new();
        let mut played: Vec<u8> = Vec::new();
        for i in 1..40 {
            out.clear();
            seq.tick(t0 + Duration::from_millis(i * 200), &mut out);
            played.extend(out.iter().filter_map(|e| match e {
                ArpEvent::On { note, .. } => Some(*note),
                _ => None,
            }));
        }
        let notes = seq.settings.notes;
        assert!(played.contains(&notes[1]), "the step inside the bar plays");
        assert!(
            !played.contains(&notes[0]),
            "the step past the bar never does: {played:?}"
        );
        t.set_time_signature(4, 4);
    }

    /// The right button erases rather than toggling: dragging it across a run
    /// of steps has to clear the run, not stencil its gaps back in.
    #[test]
    fn clearing_a_step_is_idempotent() {
        let mut seq = Seq::default();
        seq.toggle_step(2, 3);
        seq.clear_step(2, 3);
        assert!(!on(&seq, 2, 3));
        seq.clear_step(2, 3);
        assert!(!on(&seq, 2, 3), "twice over is still off, not back on");
    }

    /// Every written step still plays whatever PROB says: it is how often a
    /// step *strays*, not how often it sounds. A pattern that silently dropped
    /// notes would be a mute, not a variation.
    #[test]
    fn probability_never_drops_a_written_step() {
        let _g = crate::test_locks::transport();
        let count = |random: f32, prob: f32| {
            let mut seq = Seq::new(SeqSettings {
                on: true,
                random,
                prob,
                ..Default::default()
            });
            for step in 0..STEPS {
                seq.toggle_step(0, step);
            }
            seq.play();
            let t0 = Instant::now();
            let mut out = Vec::new();
            let mut n = 0;
            for i in 1..200 {
                out.clear();
                seq.tick(t0 + Duration::from_millis(i * 200), &mut out);
                n += out
                    .iter()
                    .filter(|e| matches!(e, ArpEvent::On { .. }))
                    .count();
            }
            n
        };
        let plain = count(0.0, 0.0);
        assert!(plain > 50, "a full pattern plays every step: {plain}");
        // No range to stray over: PROB has nothing to apply, whatever it says.
        assert_eq!(count(0.0, 1.0), plain, "PROB alone changes nothing");
        // With a range, hits are *added* — repeats inside a step, and steps
        // nothing was written on. Never taken away: that is the mute this knob
        // must not be.
        assert!(
            count(1.0, 1.0) >= plain,
            "wide open it adds hits and drops none: {} vs {plain}",
            count(1.0, 1.0)
        );
        assert!(count(1.0, 0.5) >= plain);
    }

    /// RAND is how far a step strays and PROB is how often. Either at zero is
    /// the pattern exactly as written.
    ///
    /// **And it never plays a note that is not already in the pattern.** That
    /// is the one rule the whole knob obeys: it moves *when* a note sounds and
    /// *how often*, never *which*.
    #[test]
    fn randomness_never_sounds_a_note_that_was_not_selected() {
        let _g = crate::test_locks::transport();
        let notes = |random: f32, prob: f32| {
            let mut seq = Seq::new(SeqSettings {
                on: true,
                random,
                prob,
                ..Default::default()
            });
            // Two lanes with something on them; the other six are untouched, so
            // any note but these two came from somewhere it should not have.
            seq.toggle_step(0, 0);
            seq.toggle_step(0, 4);
            seq.toggle_step(3, 8);
            seq.play();
            let t0 = Instant::now();
            let mut out = Vec::new();
            let mut played: Vec<(u8, u8)> = Vec::new();
            for i in 1..600 {
                out.clear();
                seq.tick(t0 + Duration::from_millis(i * 20), &mut out);
                played.extend(out.iter().filter_map(|e| match e {
                    ArpEvent::On { note, vel, .. } => Some((*note, *vel)),
                    _ => None,
                }));
            }
            played
        };
        let d = SeqSettings::default().notes;
        let (a, b) = (d[0], d[3]);
        let selected = |v: &[(u8, u8)]| v.iter().all(|(n, _)| *n == a || *n == b);
        let straight = |v: &[(u8, u8)]| {
            v.iter()
                .all(|(n, vel)| (*n == a || *n == b) && *vel == STEP_VEL)
        };

        assert!(straight(&notes(0.0, 0.0)), "both at zero: as written");
        assert!(
            straight(&notes(0.0, 1.0)),
            "PROB alone has nothing to apply — no range, no deviation"
        );
        assert!(
            straight(&notes(1.0, 0.0)),
            "RAND alone is never applied — a range nothing reaches for"
        );

        let wild = notes(1.0, 1.0);
        assert!(!wild.is_empty(), "wide open, it still plays");
        assert!(
            selected(&wild),
            "wide open it still only sounds the two lanes that were written: {:?}",
            wild.iter().map(|(n, _)| *n).collect::<Vec<_>>()
        );
        assert!(
            wild.iter().any(|(_, vel)| *vel != STEP_VEL),
            "and the velocity strays: {wild:?}"
        );
        assert!(wild.iter().all(|(_, vel)| *vel > 0), "never to silence");
    }

    /// Wide open, RANDOM plays more hits than the grid holds — repeats inside a
    /// step, and hits on steps nothing was written on. That is the whole of
    /// "repeated notes, at other times".
    #[test]
    fn randomness_repeats_notes_and_plays_off_the_written_steps() {
        let _g = crate::test_locks::transport();
        let hits = |random: f32, prob: f32| {
            let mut seq = Seq::new(SeqSettings {
                on: true,
                random,
                prob,
                ..Default::default()
            });
            seq.toggle_step(0, 0);
            seq.toggle_step(0, 8);
            seq.play();
            let t0 = Instant::now();
            let mut out = Vec::new();
            let mut n = 0;
            // Finer than a step, so a repeat *inside* one is visible at all.
            for i in 1..4000 {
                out.clear();
                seq.tick(t0 + Duration::from_millis(i * 3), &mut out);
                n += out
                    .iter()
                    .filter(|e| matches!(e, ArpEvent::On { .. }))
                    .count();
            }
            n
        };
        let written = hits(0.0, 0.0);
        assert!(written > 4, "the written steps play: {written}");
        let wild = hits(1.0, 1.0);
        assert!(
            wild > written,
            "wide open there are more hits than steps written: {wild} vs {written}"
        );
    }

    /// A ghost has to borrow: an empty part has nothing to play on the steps
    /// nobody wrote, and inventing one there is inventing a note.
    #[test]
    fn an_empty_pattern_stays_silent_however_wild_the_knobs() {
        let _g = crate::test_locks::transport();
        let mut seq = Seq::new(SeqSettings {
            on: true,
            random: 1.0,
            prob: 1.0,
            ..Default::default()
        });
        seq.play();
        let t0 = Instant::now();
        let mut out = Vec::new();
        for i in 1..400 {
            seq.tick(t0 + Duration::from_millis(i * 20), &mut out);
        }
        assert!(
            !out.iter().any(|e| matches!(e, ArpEvent::On { .. })),
            "nothing was written, so there is nothing to borrow: {out:?}"
        );
    }

    /// STOP takes the repeats with it. One left in the queue speaks a tick
    /// after the transport was stopped, which is a note nothing will end.
    #[test]
    fn stopping_drops_the_repeats_that_were_queued() {
        let _g = crate::test_locks::transport();
        let mut seq = Seq::new(SeqSettings {
            on: true,
            random: 1.0,
            prob: 1.0,
            ..Default::default()
        });
        seq.toggle_step(0, 0);
        seq.play();
        let t0 = Instant::now();
        let mut out = Vec::new();
        seq.tick(t0 + Duration::from_millis(20), &mut out);
        seq.stop(&mut out);
        assert!(seq.pending.is_empty(), "the queue is empty");
        out.clear();
        // Rolling again would be the only thing that speaks; a stopped
        // sequencer ticked is silent.
        for i in 1..40 {
            seq.tick(t0 + Duration::from_millis(20 + i * 20), &mut out);
        }
        assert!(
            !out.iter().any(|e| matches!(e, ArpEvent::On { .. })),
            "stopped is stopped: {out:?}"
        );
    }

    /// SWING pushes the off-beats back and makes the time up on the next one,
    /// so a pair of steps still lasts two steps however far it is pushed.
    #[test]
    fn swing_lengthens_the_beat_and_shortens_the_off_beat() {
        let _g = crate::test_locks::transport();
        let mut seq = Seq::new(SeqSettings {
            on: true,
            swing: MAX_SWING,
            ..Default::default()
        });
        // Every step written, so the timing is the only thing being read.
        for step in 0..STEPS {
            seq.toggle_step(0, step);
        }
        seq.play();
        let t0 = Instant::now();
        let mut out = Vec::new();
        // When each step landed, in milliseconds from the start.
        let mut at: Vec<u64> = Vec::new();
        let mut last = None;
        for i in 1..400 {
            out.clear();
            seq.tick(t0 + Duration::from_millis(i * 5), &mut out);
            if out.iter().any(|e| matches!(e, ArpEvent::On { .. })) {
                if let Some(prev) = last {
                    at.push(i * 5 - prev);
                }
                last = Some(i * 5);
            }
        }
        assert!(at.len() > 4, "several steps went by: {at:?}");
        // The first interval is the run-up — `play` fires the downbeat on the
        // tick that starts the clock. The steady state is the pair after it:
        // on-beat to off-beat is the long half, off-beat to on-beat the short,
        // and the two together are two straight steps.
        let (long, short) = (at[2], at[3]);
        assert!(
            long > short * 2,
            "the off-beat is late and pays it back: {at:?}"
        );
    }

    /// Swing is the arpeggiator's number, on the arpeggiator's scale: an odd
    /// step is late by `swing` of a step, not by half of it.
    ///
    /// Against the transport's grid, which is the clock that runs once the rack
    /// is rolling — and the one where it was not being heard.
    #[test]
    fn swing_is_heard_on_the_transports_grid() {
        let _g = crate::test_locks::transport();
        let t = choz_ports::transport();
        t.set_time_signature(4, 4);
        t.set_bpm(120.0);
        t.set_playing(true);
        t.rewind();

        let mut seq = Seq::new(SeqSettings {
            on: true,
            swing: MAX_SWING,
            ..Default::default()
        });
        seq.toggle_step(0, 1);
        seq.play();

        // Sitting exactly on step 1's boundary: straight, it is due now.
        let step_q = seq.settings.div.quarters() as f64;
        t.set_position_beats(step_q);
        let mut out = Vec::new();
        seq.tick(Instant::now(), &mut out);
        assert!(
            out.is_empty(),
            "the off-beat is not due on the beat any more: {out:?}"
        );

        // …and it is, once the transport is `swing` of a step past it.
        t.set_position_beats(step_q * (1.0 + MAX_SWING as f64 + 0.01));
        seq.tick(Instant::now(), &mut out);
        assert!(
            out.iter().any(|e| matches!(e, ArpEvent::On { .. })),
            "and it plays late: {out:?}"
        );

        t.set_playing(false);
        t.rewind();
    }

    /// The cursor wraps rather than sticking at the edges.
    #[test]
    fn the_cursor_wraps() {
        let mut seq = Seq::default();
        seq.move_cursor(-1, -1);
        assert_eq!(seq.cursor, (TRACKS - 1, STEPS - 1));
        seq.move_cursor(1, 1);
        assert_eq!(seq.cursor, (0, 0));
    }
}
