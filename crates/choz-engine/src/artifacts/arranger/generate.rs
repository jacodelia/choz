//! The generators: a progression and a style in, a part in notes out.
//!
//! Everything here runs **once**, when the text or a setting changes, and never
//! from the audio thread: what it produces is a baked list of notes that
//! [`super::Arranger::tick`] only reads. Same progression, same style and same
//! seed give the same list, note for note — an interpretation you liked is one
//! you can get back, and a bug is one you can reproduce.

use super::chord::{Chord, Progression};
use super::style::Style;

/// General MIDI percussion, which is what the drum part speaks. The kit is the
/// instrument's business — a tab with a GM SoundFont on channel 10, or a drum
/// sampler mapped the same way.
pub mod gm {
    pub const KICK: u8 = 36;
    pub const SNARE: u8 = 38;
    pub const HAT_CLOSED: u8 = 42;
    /// Not hit by anything yet: the styles that need one — a shuffle that
    /// opens the hat on the and of four — come with phase 3.
    #[allow(dead_code)]
    pub const HAT_OPEN: u8 = 46;
    pub const CRASH: u8 = 49;
    pub const RIDE: u8 = 51;
    pub const TOM_LO: u8 = 45;
    pub const TOM_MID: u8 = 47;
    pub const TOM_HI: u8 = 50;
}

/// Which musician this instance is. One role per instance, because an artifact
/// lives in a tab and plays *one* instrument: the band is a tab per player,
/// all reading the same progression with the same seed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum Role {
    #[default]
    Bass,
    Drums,
    /// `Melody` and `Solo` are what a project written before those roles went
    /// says, and they read back as this one: a saved rack must not stop opening
    /// because a role did.
    #[serde(alias = "Melody", alias = "Solo")]
    Piano,
    /// The same chords with a guitar's voicings and a strum across them, and
    /// the rhythm the piano is *not* playing — see [`counter_hold`].
    Guitar,
}

impl Role {
    /// New roles go on the **end**: the variant's position is what a saved
    /// project wrote and what [`Rng::new`] seeds from, so moving one changes
    /// both the role a project reopens with and what it plays.
    /// **A band, not a whole arrangement.** The tune and the solo were here and
    /// went: what the arranger is for is the part nobody in the room is going
    /// to play, and the melody is the thing the player *is* playing. What they
    /// left behind — a lead's register, its menu of durations — went with the
    /// hand-written styles: what plays now is measured off real rhythms, and
    /// there is nothing in a measurement for a role nobody has.
    pub const ALL: [Role; 4] = [Role::Bass, Role::Drums, Role::Piano, Role::Guitar];

    pub fn name(self) -> &'static str {
        match self {
            Role::Bass => "BASS",
            Role::Drums => "DRUMS",
            Role::Piano => "PIANO",
            Role::Guitar => "GUITAR",
        }
    }
}

/// A note in the arrangement. Positions are in beats from the top of the
/// progression, which is the only timeline the roles share.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Note {
    pub note: u8,
    pub vel: u8,
    pub start: f64,
    pub len: f64,
    /// Who plays it. Carried on the note because a band in one tab is several
    /// parts on one timeline, and where a note goes — which zone of the tab's
    /// instrument holds the kit, the bass, the horn — is not something the
    /// pitch can say: a bass and a piano share notes.
    pub role: Role,
}

/// xorshift32: a sequence rather than a surprise, and the same one the
/// sequencer's `RANDOM` uses. It carries the role's feel because every roll
/// that humanises a note is made through it, and threading one more number
/// through six generators to say the same thing would be six signatures wider.
pub struct Rng {
    state: u32,
    /// How loose this role plays: the style's `human` times what the role
    /// itself is worth. `1.0` is what a blues drummer does.
    feel: f32,
}

impl Rng {
    /// Seeded per role, so moving the bass's dice does not re-roll the drums'.
    pub fn new(seed: u32, role: Role, feel: f32) -> Self {
        Self {
            state: (seed ^ (0x9e37_79b9u32.wrapping_mul(role as u32 + 1))).max(1),
            feel,
        }
    }

    fn next(&mut self) -> u32 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 17;
        self.state ^= self.state << 5;
        self.state
    }

    /// 0.0..1.0.
    fn f(&mut self) -> f32 {
        self.next() as f32 / u32::MAX as f32
    }

    fn chance(&mut self, p: f32) -> bool {
        self.f() < p
    }

    fn pick(&mut self, n: usize) -> usize {
        match n {
            0 => 0,
            n => self.next() as usize % n,
        }
    }
}

/// What the band is playing *into*: the tempo, the bar, and the three knobs a
/// player turns by ear.
///
/// Passed in rather than read off the transport, so baking stays a function of
/// its arguments — see the module's own promise about the seed.
#[derive(Debug, Clone, Copy)]
pub struct Bake<'a> {
    /// What it will be played at. Only the drums read it: what a kit can play
    /// at 130 and what it can play at 230 are different parts.
    pub bpm: f32,
    /// On top of the style's own swing, as a share of the division it counts
    /// in. The sequencer's knob, on the same scale, because there is one swing
    /// in this program.
    pub swing: f32,
    /// **How far** a note may stray from what was generated, 0..1 — the size of
    /// the deviation, in velocity and in time.
    pub random: f32,
    /// **How often** that deviation is applied, 0..1. Rolled once a note: at 0
    /// the part plays exactly as it was generated.
    pub prob: f32,
    /// How the bar is grouped — `3+2+2` for a 7/8 counted the way the metronome
    /// clicks it. Empty is a bar that is not grouped, which is most of them.
    /// The group each one starts on is where the accent goes.
    pub groups: &'a [u8],
}

impl Bake<'_> {
    /// Everything at its default, at `bpm` — what the tests and a plain tab
    /// bake with.
    pub fn at(bpm: f32) -> Self {
        Self {
            bpm,
            swing: 0.0,
            random: 0.0,
            prob: 0.0,
            groups: &[],
        }
    }

    /// The beat each group of the bar starts on.
    fn accents(&self, groups: &[u8], beats_per_bar: f64, unit: f64) -> Vec<f64> {
        let mut out = Vec::new();
        let mut at = 0.0;
        for g in groups.iter().copied() {
            if at >= beats_per_bar - 1e-9 {
                break;
            }
            out.push(at);
            at += g as f64 * unit;
        }
        out
    }
}

/// The whole of a role's part. Every note comes out stamped with who plays it
/// — see [`Note::role`].
pub fn bake(prog: &Progression, style: &Style, role: Role, seed: u32, ctx: &Bake) -> Vec<Note> {
    // The style, with the knobs on it: the swing is the style's plus the tab's,
    // because a style is a feel and a knob is a player leaning on it.
    let style = &Style {
        swing: (style.swing + ctx.swing).clamp(0.0, 0.9),
        ..*style
    };
    let mut rng = Rng::new(seed, role, style.human * role_feel(role));
    let mut out = Vec::new();
    match role {
        Role::Bass => bass(prog, style, &mut rng, &mut out),
        Role::Drums => drums(prog, style, &mut rng, &mut out, ctx),
        Role::Piano => comp(
            prog,
            style,
            &mut rng,
            &mut out,
            style.comp,
            0.0,
            true,
            PIANO_NOTES,
        ),
        // The guitar plays against the piano rather than beside it: whatever
        // the piano is holding, this is the other length — see
        // [`counter_hold`]. Two chord instruments playing the same rhythm is
        // one chord instrument with a chorus on it.
        Role::Guitar => {
            let mut c = style.guitar.unwrap_or_else(|| guitar_of(style.comp));
            c.hold = counter_hold(style.comp.hold, style.beats_per_bar);
            comp(
                prog,
                style,
                &mut rng,
                &mut out,
                c,
                style.strum.max(0.0),
                false,
                GUITAR_NOTES,
            )
        }
    }
    // Stamped here rather than in six generators: whoever is baking is the
    // role that was asked for, and a generator that has to remember to say so
    // is a generator that will forget.
    for n in out.iter_mut() {
        n.role = role;
    }
    // The two knobs that are not the style's: how far a note may stray and how
    // often it does. The sequencer's pair, with the same meanings — what makes
    // a part that goes round twelve bars stop sounding like it is going round
    // twelve bars.
    vary(&mut out, &mut rng, ctx.random, ctx.prob);
    // The accents the bar's grouping asks for: a 7/8 counted 3+2+2 leans on the
    // beat each group starts on, which is what makes it that 7/8 and not the
    // other one.
    accent(&mut out, ctx, prog, style.beats_per_bar);
    // A chord instrument lets go when it plays again: whatever the style says a
    // chord is held for, the next strum is what ends it.
    if matches!(role, Role::Piano | Role::Guitar) {
        damp(&mut out);
    }
    let total = prog.beats(style.beats_per_bar);
    out.retain(|n| n.start < total && n.len > 0.0);
    // Nothing hangs past the end: the loop comes back round to the downbeat and
    // a note still sounding there would be a note the next chorus did not play.
    for n in &mut out {
        n.len = n.len.min(total - n.start);
    }
    out.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.note.cmp(&b.note)));
    balance(&mut out, role);
    out
}

/// The band's own mix, applied last: what a role is worth against the others,
/// and what a chord is worth against a single note.
///
/// A style's `vel` is how hard the *kit* hits — it is measured off the drums —
/// and handing the same number to four players is four players fighting for
/// the same room. Worse, a chord instrument puts five notes where the bass puts
/// one, so a comp at the bass's velocity is five times the bass's energy and
/// the line disappears under it. Both of those are mixing and neither belongs
/// in a generator, so they happen here, once, to whatever came out.
///
/// The fader in the box is on top of this — see [`super::Arranger::rebake`].
fn balance(out: &mut [Note], role: Role) {
    // The bass is the reference. Everything else sits under it, because what a
    // backing band is for is the part nobody in the room is playing, and the
    // one you have to hear is the one holding the harmony down.
    let level = match role {
        Role::Bass => 1.0,
        Role::Drums => 0.82,
        Role::Piano => 0.62,
        Role::Guitar => 0.55,
    };
    // Notes struck together share the room they are given. Not `1/n` — that is
    // a chord quieter than one note of it, which is not what a piano does —
    // but the square root, which keeps the *sum* roughly level.
    let mut i = 0;
    while i < out.len() {
        let mut j = i + 1;
        while j < out.len() && (out[j].start - out[i].start).abs() < 1e-6 {
            j += 1;
        }
        let share = level / ((j - i) as f32).sqrt();
        for n in &mut out[i..j] {
            n.vel = (n.vel as f32 * share).round().clamp(1.0, 127.0) as u8;
        }
        i = j;
    }
}

/// How far an off-division is pushed late, in beats. The sequencer's reading
/// of the same knob: a third of an eighth is the triplet shuffle a blues wants,
/// and the same fraction of a sixteenth is the one a funk does.
///
/// `div` is what the style counts in — see [`Style::swing_div`].
fn swung(pos: f64, style: &Style) -> f64 {
    let div = match style.swing_div > 0.0 {
        true => style.swing_div,
        false => 0.5,
    };
    let step = (pos / div).round();
    match (step as i64).rem_euclid(2) == 1 && (pos / div - step).abs() < 1e-6 {
        true => pos + div * style.swing as f64,
        false => pos,
    }
}

/// Move what the dice say to move: `random` is how far, `prob` is how often.
///
/// Velocity and time, never pitch — a note in another place is a mistake, and a
/// note at another weight is a player. Deterministic, like everything else
/// here: the same seed gives the same variation.
fn vary(part: &mut [Note], rng: &mut Rng, random: f32, prob: f32) {
    if random <= 0.0 || prob <= 0.0 {
        return;
    }
    for n in part.iter_mut() {
        if !rng.chance(prob) {
            continue;
        }
        let v = n.vel as f32 * (1.0 - random * 0.5 * rng.f());
        n.vel = v.clamp(1.0, 127.0) as u8;
        // A twentieth of a beat at the widest, either way: past that it is not
        // a player pushing, it is a note in the wrong place.
        let shift = (rng.f() as f64 - 0.5) * 0.1 * random as f64;
        n.start = (n.start + shift).max(0.0);
        n.len = (n.len * (1.0 - random as f64 * 0.3 * rng.f() as f64)).max(0.05);
    }
}

/// Lean on the beat each group of the bar starts on.
///
/// Only where the bar says it is grouped: `3+2+2` is not seven equal eighths,
/// and the difference between it and `2+2+3` is exactly this.
///
/// **Each bar is grouped on its own.** A bar written `| Cm:3 F:2 Bb:2 |` says
/// where its own accents are; the tab's grouping — the metronome's, or the one
/// the arranger was given — is what a bar that said nothing is counted in.
fn accent(part: &mut [Note], ctx: &Bake, prog: &Progression, beats_per_bar: f64) {
    if beats_per_bar <= 0.0 {
        return;
    }
    let bars = prog.bars.len();
    for n in part.iter_mut() {
        let bar = (n.start / beats_per_bar).floor();
        if bar < 0.0 || bar >= bars as f64 {
            continue;
        }
        // The bar's own weights when it was written with more than one slot,
        // the tab's grouping otherwise.
        let own = prog.bars[bar as usize].groups();
        let groups: &[u8] = match own.len() >= 2 {
            true => &own,
            false => ctx.groups,
        };
        if groups.len() < 2 {
            continue;
        }
        let cells: f64 = groups.iter().map(|g| *g as f64).sum();
        if cells <= 0.0 {
            continue;
        }
        // What one group-count is worth, in beats: seven eighths in a bar of
        // 3.5 beats is half a beat each.
        let unit = beats_per_bar / cells;
        let starts = ctx.accents(groups, beats_per_bar, unit);
        let into = n.start - bar * beats_per_bar;
        // The downbeat is already the loudest thing in the bar; this is about
        // the ones inside it.
        if starts
            .iter()
            .skip(1)
            .any(|s| (into - s).abs() < unit * 0.25)
        {
            n.vel = n.vel.saturating_add(10).min(127);
        }
    }
}

/// How loose a role is allowed to be, against the style's own looseness. The
/// drummer is the clock and the soloist is the one allowed to lean on it;
/// putting the same jitter on both is what makes a generated band sound like
/// one player with six hands.
fn role_feel(role: Role) -> f32 {
    match role {
        Role::Drums => 0.6,
        Role::Bass => 0.8,
        Role::Piano | Role::Guitar => 1.0,
    }
}

/// Timing and velocity, moved by a hair and never the pitch. Deterministic:
/// the humanisation is part of the interpretation the seed names. `spread` is
/// what the part is worth on its own; the role's and the style's feel scale it.
fn human(rng: &mut Rng, note: &mut Note, spread: f64) {
    let feel = rng.feel.max(0.0) as f64;
    let jitter = (rng.f() as f64 - 0.5) * spread * feel;
    note.start = (note.start + jitter).max(0.0);
    let swing = (16.0 * feel) as f32;
    let v = note.vel as i32 + (rng.f() * swing) as i32 - (swing / 2.0) as i32;
    note.vel = v.clamp(1, 127) as u8;
}

/// Fold a pitch class into a register.
fn in_range(pc: u8, low: u8, high: u8, near: u8) -> u8 {
    let mut best = None;
    let mut dist = i32::MAX;
    for oct in 0..11 {
        let n = oct * 12 + pc as i32;
        if n < low as i32 || n > high as i32 {
            continue;
        }
        let d = (n - near as i32).abs();
        if d < dist {
            dist = d;
            best = Some(n as u8);
        }
    }
    // A register narrower than an octave can have no answer at all; the nearest
    // edge is better than dropping the note.
    best.unwrap_or_else(|| ((pc as i32).clamp(low as i32, high as i32)) as u8)
}

// ─── Bass ───────────────────────────────────────────────────────────────────

/// A walking bass: the root where the chord lands, chord tones through it, and
/// a chromatic approach into the next one. `C C C C / F F F F` is the failure,
/// not the minimum.
fn bass(prog: &Progression, style: &Style, rng: &mut Rng, out: &mut Vec<Note>) {
    let bpb = style.beats_per_bar;
    let b = style.bass;
    let total = prog.beats(bpb);
    let mut prev = in_range(0, b.low, b.high, (b.low + b.high) / 2);
    let mut prev_chord: Option<super::chord::Chord> = None;
    // Which way the line last moved, so a passing note carries on that way.
    let mut last_move: i32 = 1;
    let mut beat = 0.0;
    while beat < total - 1e-9 {
        let Some((chord, left)) = prog.at(beat, bpb) else {
            break;
        };
        // "The chord has just landed" is "it is not the one that was sounding a
        // beat ago" — asking the bar how many slots it has got this wrong the
        // moment a chord was written held across two of them.
        let lands = prev_chord.as_ref() != Some(chord);
        prev_chord = Some(chord.clone());
        let leaves = left <= 1.0 + 1e-6;
        let next = prog.next_after(beat, bpb);

        let note = if lands {
            // The chord's own root, where the chord arrives: the one note that
            // is never in question.
            in_range(chord.root, b.low, b.high, prev)
        } else if !b.walking {
            // Not walking: root and fifth, and `density` decides which of the
            // off-beats speak at all. A rock or a bossa bass holds the chord
            // down; it does not go looking for a way out of it.
            in_range(chord.tone(2), b.low, b.high, prev)
        } else if leaves && rng.chance(b.approach) {
            approach(next.unwrap_or(chord), prev, &b, rng)
        } else if rng.chance(b.passing) {
            // A step through the scale, in the direction the line was already
            // going: what joins two chord tones up instead of jumping between
            // them.
            passing(chord, prev, last_move, &b)
        } else {
            chord_tone(chord, prev, &b, rng)
        };
        let note = match !lands && rng.chance(b.octave_jump) {
            true => match note + 12 <= b.high {
                true => note + 12,
                false => note.saturating_sub(12).max(b.low),
            },
            false => note,
        };
        last_move = note as i32 - prev as i32;
        prev = note;

        // The beat the chord lands on always speaks: `density` thins the line,
        // it does not swallow the note that says which chord this is.
        if lands || rng.chance(b.density) {
            // The anticipation: a chord arrived at an eighth early and held
            // over the beat it belongs to. Bar one gets it as the pickup.
            let early = lands && beat > 0.0 && rng.chance(b.pickup);
            let mut n = Note {
                note,
                vel: style.vel,
                start: match early {
                    true => swung(beat - 0.5, style),
                    false => swung(beat, style),
                },
                // Walking: long enough to join up, short enough to articulate.
                len: match early {
                    true => 1.4,
                    false => 0.9,
                },
                // Who plays it is stamped by `bake`, which knows.
                ..Default::default()
            };
            // What came before has to get out of the way of an anticipation,
            // or the two overlap and the same note is asked to stop twice.
            if early {
                if let Some(last) = out.last_mut() {
                    last.len = last.len.min((n.start - last.start).max(0.05));
                }
            }
            human(rng, &mut n, 0.03);
            out.push(n);
        }
        beat += 1.0;
    }
}

/// One step of the chord's scale from where the line is, in the direction it
/// was already going — the passing note. The edges of the register turn it
/// round rather than dropping it.
fn passing(chord: &Chord, prev: u8, last_move: i32, b: &super::style::Bass) -> u8 {
    let up = match (prev >= b.high.saturating_sub(1), prev <= b.low + 1) {
        (true, _) => false,
        (_, true) => true,
        _ => last_move >= 0,
    };
    let mut best = prev;
    let mut dist = i32::MAX;
    for pc in chord
        .scale
        .iter()
        .map(|s| ((chord.root as i32 + s).rem_euclid(12)) as u8)
    {
        for oct in 0..11u8 {
            let Some(note) = oct.checked_mul(12).and_then(|o| o.checked_add(pc)) else {
                continue;
            };
            if note < b.low || note > b.high {
                continue;
            }
            let d = note as i32 - prev as i32;
            if (up && d <= 0) || (!up && d >= 0) {
                continue;
            }
            if d.abs() < dist {
                dist = d.abs();
                best = note;
            }
        }
    }
    best
}

/// A semitone either side of the next chord's root, chosen so the line keeps
/// going the way it was already going.
fn approach(next: &Chord, prev: u8, b: &super::style::Bass, rng: &mut Rng) -> u8 {
    let target = in_range(next.root, b.low, b.high, prev);
    let below = target.saturating_sub(1);
    let above = (target + 1).min(127);
    let from_below = match (below >= b.low, above <= b.high) {
        (true, false) => true,
        (false, true) => false,
        // Free to choose: keep the direction, and let the dice break a tie.
        _ => target > prev || (target == prev && rng.chance(0.5)),
    };
    match from_below {
        true => below,
        false => above,
    }
}

/// A chord tone that is not the one just played — a repeated note is what makes
/// a bass line sound like a metronome.
fn chord_tone(chord: &Chord, prev: u8, b: &super::style::Bass, rng: &mut Rng) -> u8 {
    let pcs = chord.pitch_classes();
    for _ in 0..4 {
        let pc = pcs[rng.pick(pcs.len())];
        let note = in_range(pc, b.low, b.high, prev);
        if note != prev {
            return note;
        }
    }
    in_range(pcs[0], b.low, b.high, prev)
}

// ─── Drums ──────────────────────────────────────────────────────────────────

/// Kick, snare, a cymbal keeping time, ghost notes, and a fill where the form
/// turns. The sounds are the instrument's; this is only what is hit and when.
fn drums(prog: &Progression, style: &Style, rng: &mut Rng, out: &mut Vec<Note>, ctx: &Bake) {
    let bpm = ctx.bpm;
    let bpb = style.beats_per_bar;
    let d = style.drums;
    // What the tempo leaves room for. A style's numbers are written for the
    // feel and not for the clock, so a funk counting sixteenths on the hat is
    // right at 100 and is a buzz at 230 — the hands thin out, which is what a
    // drummer does and what the generated part used not to.
    let fast = bpm >= FAST_BPM;
    let very_fast = bpm >= VERY_FAST_BPM;
    let cymbal = match (fast, very_fast) {
        (_, true) => d.cymbal.max(1.0),
        (true, _) => d.cymbal.max(0.5),
        _ => d.cymbal,
    };
    // The ghosts are the first thing to go: they live between the hands, and at
    // this tempo there is nothing between them.
    let ghost = match (fast, very_fast) {
        (_, true) => 0.0,
        (true, _) => d.ghost * 0.35,
        _ => d.ghost,
    };
    let grid = fill_grid(bpm);
    // The fill played last, so the next one is a different one.
    let mut last_fill: Option<usize> = None;
    for bar in 0..prog.bars.len() {
        let at = bar as f64 * bpb;
        // **The fill is chosen before the groove is laid**, because how much of
        // the bar it takes is what says where the groove stops: a two-beat
        // turnaround that started while the hat was still counting would be two
        // drummers. `None` is a bar with no fill in it at all, and then nothing
        // stops.
        let wanted = d.fill_every > 0 && (bar + 1) % d.fill_every == 0;
        // The turnaround: the bar that hands the form back to the top asks for
        // two beats instead of one.
        let turnaround = wanted && bar + 1 == prog.bars.len() && bpb >= 3.0;
        let chosen = match wanted {
            true => pick_fill(rng, if turnaround { 2.0 } else { 1.0 }, grid, last_fill),
            false => None,
        };
        if chosen.is_some() {
            last_fill = chosen;
        }
        let fill = chosen.is_some();
        let last_beat = match chosen {
            Some(i) => bpb - FILLS[i].beats,
            None => bpb,
        };

        for pos in d.kick.iter().copied() {
            push_hit(out, rng, gm::KICK, at + swung(pos, style), style.vel, 0.25);
        }
        for pos in d.snare.iter().copied() {
            if fill && pos >= last_beat {
                continue;
            }
            push_hit(out, rng, gm::SNARE, at + swung(pos, style), style.vel, 0.25);
        }
        // The cymbal: a ride swings its off-beats, a hat does not, and both
        // stop where a fill starts.
        let drum = match d.ride {
            true => gm::RIDE,
            false => gm::HAT_CLOSED,
        };
        let mut pos = 0.0;
        while pos < bpb - 1e-9 {
            if !(fill && pos >= last_beat) {
                let on_beat = (pos.fract()).abs() < 1e-6;
                let vel = match on_beat {
                    true => style.vel,
                    false => style.vel.saturating_sub(18),
                };
                push_hit(out, rng, drum, at + swung(pos, style), vel, 0.2);
            }
            pos += cymbal.max(0.125);
        }
        // Ghosts: quiet snares between the backbeats, which is the difference
        // between a drum machine and someone playing one.
        //
        // On the division the style counts in, not on every half beat: a funk
        // ghosts in sixteenths and a shuffle in swung eighths, and putting both
        // on the same grid was what made the two sound like the same drummer.
        // Never on a beat that already has something on it, and never on a bare
        // downbeat — a ghost is what fills the gap, not what marks the bar.
        let step = cymbal.max(0.125);
        let mut g = step;
        while g < bpb - 1e-9 {
            let taken = d
                .kick
                .iter()
                .chain(d.snare.iter())
                .any(|p| (p - g).abs() < 1e-6)
                || g.fract().abs() < 1e-6;
            if !taken && !(fill && g >= last_beat) && rng.chance(ghost) {
                push_hit(
                    out,
                    rng,
                    gm::SNARE,
                    at + swung(g, style),
                    style.vel.saturating_sub(45).max(1),
                    0.15,
                );
            }
            g += step;
        }
        // The fill: one played by somebody, off the table — a beat of it where
        // the form is only turning a corner, two where it goes back to the top.
        // Never the same one twice running, and never one the tempo cannot
        // carry: see [`pick_fill`].
        if let Some(i) = chosen {
            let f = &FILLS[i];
            // Laid so it *ends* on the barline, which is where the groove picks
            // the bar back up.
            let from = at + bpb - f.beats;
            for (pos, note, vel) in f.hits.iter().copied() {
                let vel = (style.vel as f32 * vel).clamp(1.0, 127.0) as u8;
                push_hit(out, rng, note, from + pos, vel, 0.2);
            }
        }
        // The crash marks where the form comes back round, downbeat of the bar
        // after a fill and the top of the arrangement.
        if bar == 0 || (d.fill_every > 0 && bar % d.fill_every == 0 && bar > 0) {
            push_hit(out, rng, gm::CRASH, at, style.vel, 0.5);
        }
    }
}

/// A fill, as it was played: where each hit falls inside it, which drum it is,
/// and how hard against the style's own velocity.
///
/// # Where these come from
///
/// The `Drums/` folder of a 52,000-file MIDI collection — 196 loops, 232 bars
/// of them fills — folded onto a sixteenth grid and counted. What is here is
/// the shapes that came up most, kept as they were played: the tom that
/// answers itself with the kick under it, the single-stroke roll that builds
/// into the downbeat, the paradiddle with the kick on the second stroke, the
/// open hat that gets choked by the snare. A generated fill is only as good as
/// the fills it was copied from.
pub struct Fill {
    /// How much of the bar it takes, in beats. One beat is an ordinary fill and
    /// two is what a turnaround gets.
    pub beats: f64,
    /// `(beats into the fill, drum, share of the style's velocity)`.
    pub hits: &'static [(f64, u8, f32)],
}

impl Fill {
    /// The shortest step in it, in beats — what says whether a tempo can carry
    /// it. A sixteenth at 220 is 68 ms, and what a kit plays at 68 ms is a
    /// single-stroke roll, not four separate drums.
    fn finest(&self) -> f64 {
        let mut finest = self.beats;
        for w in self.hits.windows(2) {
            let step = w[1].0 - w[0].0;
            if step > 1e-6 && step < finest {
                finest = step;
            }
        }
        finest
    }
}

/// The fills. One beat and two, sixteenths and eighths — the eighth ones are
/// what a fast tempo is left with, and they are real fills and not the others
/// with hits taken out.
pub const FILLS: &[Fill] = &[
    // ── A beat of sixteenths ────────────────────────────────────────────
    // Down the toms, which is the fill everybody knows.
    Fill {
        beats: 1.0,
        hits: &[
            (0.0, gm::TOM_HI, 0.92),
            (0.25, gm::TOM_MID, 0.95),
            (0.5, gm::TOM_LO, 1.0),
            (0.75, gm::TOM_LO, 1.05),
        ],
    },
    // The single-stroke roll, building into the downbeat.
    Fill {
        beats: 1.0,
        hits: &[
            (0.0, gm::SNARE, 0.8),
            (0.25, gm::SNARE, 0.88),
            (0.5, gm::SNARE, 0.96),
            (0.75, gm::SNARE, 1.05),
        ],
    },
    // The one with the kick under the first stroke — 7 bars of the library.
    Fill {
        beats: 1.0,
        hits: &[
            (0.0, gm::KICK, 1.0),
            (0.0, gm::SNARE, 0.85),
            (0.25, gm::SNARE, 0.9),
            (0.5, gm::SNARE, 0.95),
            (0.75, gm::SNARE, 1.05),
        ],
    },
    // The open hat choked by the snare, straight out of the funk loops.
    Fill {
        beats: 1.0,
        hits: &[
            (0.0, gm::HAT_OPEN, 0.8),
            (0.25, gm::SNARE, 0.85),
            (0.5, gm::KICK, 1.0),
            (0.75, gm::SNARE, 1.0),
        ],
    },
    // The paradiddle: kick, kick, snare, kick.
    Fill {
        beats: 1.0,
        hits: &[
            (0.0, gm::KICK, 1.0),
            (0.25, gm::KICK, 0.9),
            (0.5, gm::SNARE, 0.95),
            (0.75, gm::KICK, 1.0),
        ],
    },
    // Snare and mid tom answering each other.
    Fill {
        beats: 1.0,
        hits: &[
            (0.0, gm::TOM_MID, 0.9),
            (0.25, gm::SNARE, 0.9),
            (0.5, gm::SNARE, 0.95),
            (0.75, gm::TOM_MID, 1.05),
        ],
    },
    // ── A beat of eighths — what a fast tempo can actually play ─────────
    // The most common bar in the whole library: floor tom, then the kick and
    // the floor tom together.
    Fill {
        beats: 1.0,
        hits: &[
            (0.0, gm::TOM_LO, 0.95),
            (0.5, gm::KICK, 1.0),
            (0.5, gm::TOM_LO, 1.05),
        ],
    },
    Fill {
        beats: 1.0,
        hits: &[(0.0, gm::SNARE, 0.9), (0.5, gm::TOM_LO, 1.05)],
    },
    Fill {
        beats: 1.0,
        hits: &[(0.0, gm::TOM_HI, 0.9), (0.5, gm::TOM_LO, 1.05)],
    },
    Fill {
        beats: 1.0,
        hits: &[(0.0, gm::KICK, 1.0), (0.5, gm::SNARE, 1.05)],
    },
    // ── Two beats: the turnaround ───────────────────────────────────────
    // The library's most common two-beat shape, in eighths, so it survives a
    // fast tempo as well.
    Fill {
        beats: 2.0,
        hits: &[
            (0.0, gm::KICK, 0.95),
            (1.0, gm::TOM_LO, 0.95),
            (1.5, gm::KICK, 1.0),
            (1.5, gm::TOM_LO, 1.05),
        ],
    },
    Fill {
        beats: 2.0,
        hits: &[
            (0.0, gm::KICK, 0.95),
            (0.5, gm::TOM_LO, 0.9),
            (1.0, gm::SNARE, 1.0),
            (1.5, gm::TOM_LO, 1.05),
        ],
    },
    // Down the kit: high tom twice, then the floor tom with the kick.
    Fill {
        beats: 2.0,
        hits: &[
            (0.0, gm::KICK, 0.95),
            (0.5, gm::TOM_HI, 0.88),
            (0.75, gm::TOM_HI, 0.92),
            (1.0, gm::TOM_MID, 0.96),
            (1.5, gm::KICK, 1.0),
            (1.5, gm::TOM_LO, 1.05),
        ],
    },
    // Two beats of single strokes, which is what a rock turnaround is.
    Fill {
        beats: 2.0,
        hits: &[
            (0.0, gm::SNARE, 0.72),
            (0.25, gm::SNARE, 0.76),
            (0.5, gm::SNARE, 0.8),
            (0.75, gm::SNARE, 0.84),
            (1.0, gm::SNARE, 0.88),
            (1.25, gm::SNARE, 0.92),
            (1.5, gm::SNARE, 0.98),
            (1.75, gm::SNARE, 1.05),
        ],
    },
    // The open-hat one, stretched over two beats.
    Fill {
        beats: 2.0,
        hits: &[
            (0.0, gm::HAT_OPEN, 0.8),
            (0.25, gm::SNARE, 0.85),
            (0.5, gm::SNARE, 0.9),
            (0.75, gm::KICK, 0.95),
            (1.0, gm::HAT_OPEN, 0.85),
            (1.25, gm::SNARE, 0.95),
            (1.5, gm::KICK, 1.0),
            (1.75, gm::SNARE, 1.05),
        ],
    },
];

/// Above this the sixteenths go: at 190 one is 79 ms, and four drums 79 ms
/// apart is a blur however well they are played. The hats thin to eighths, the
/// ghosts mostly go, and the fills come out of the eighth-note half of
/// [`FILLS`].
pub const FAST_BPM: f32 = 190.0;

/// And above this the eighths go too — the cymbal counts the beat, which is
/// what a drummer does at a tempo nobody can ride at.
pub const VERY_FAST_BPM: f32 = 240.0;

/// The shortest step a fill may have at this tempo.
fn fill_grid(bpm: f32) -> f64 {
    match bpm >= FAST_BPM {
        true => 0.5,
        false => 0.25,
    }
}

/// One fill that fits: no longer than `beats`, and nothing in it faster than
/// `grid`. `avoid` is the one played last, because the same fill twice running
/// is what gives a generated drummer away in one chorus.
fn pick_fill(rng: &mut Rng, beats: f64, grid: f64, avoid: Option<usize>) -> Option<usize> {
    let fits: Vec<usize> = (0..FILLS.len())
        .filter(|i| FILLS[*i].beats <= beats + 1e-9 && FILLS[*i].finest() >= grid - 1e-9)
        .collect();
    // The longest that fit, when any do: a turnaround asked for two beats.
    let longest = fits
        .iter()
        .map(|i| FILLS[*i].beats)
        .fold(0.0f64, |a, b| a.max(b));
    let best: Vec<usize> = fits
        .iter()
        .copied()
        .filter(|i| (FILLS[*i].beats - longest).abs() < 1e-9)
        .collect();
    let pool: Vec<usize> = match best.iter().any(|i| Some(*i) != avoid) {
        true => best.iter().copied().filter(|i| Some(*i) != avoid).collect(),
        false => best,
    };
    pool.get(rng.pick(pool.len())).copied()
}

fn push_hit(out: &mut Vec<Note>, rng: &mut Rng, note: u8, start: f64, vel: u8, len: f64) {
    let mut n = Note {
        note,
        vel,
        start,
        len,
        // Who plays it is stamped by `bake`, which knows.
        ..Default::default()
    };
    human(rng, &mut n, 0.02);
    out.push(n);
}

// ─── Comping ────────────────────────────────────────────────────────────────

/// Chords, voiced and led: the third and the seventh with whatever tension the
/// chord carries, placed at the octave nearest where the last voicing sat. A
/// comp that re-stacks from the root every bar jumps an octave for no reason
/// and stops sounding like hands.
#[allow(clippy::too_many_arguments)]
fn comp(
    prog: &Progression,
    style: &Style,
    rng: &mut Rng,
    out: &mut Vec<Note>,
    c: super::style::Comp,
    strum: f64,
    shell: bool,
    max: usize,
) {
    let bpb = style.beats_per_bar;
    let mut prev_top = (c.low + c.high) / 2;
    // Which stroke this is: the hand alternates over the hits it actually
    // plays, not over the ones the density threw away.
    let mut hits = 0usize;
    for bar in 0..prog.bars.len() {
        for hit in c.hits.iter().copied() {
            let beat = bar as f64 * bpb + hit;
            let Some((chord, left)) = prog.at(beat, bpb) else {
                continue;
            };
            if !rng.chance(c.density) {
                continue;
            }
            let voicing = voice(chord, prev_top, c.low, c.high, shell, max);
            if let Some(top) = voicing.last() {
                prev_top = *top;
            }
            let start = swung(beat, style);
            let len = c.hold.min(left.max(0.25));
            // One roll of the dice for the whole voicing: four notes each
            // straying on their own is four chords, not one played loosely.
            let jitter = (rng.f() as f64 - 0.5) * 0.03 * rng.feel.max(0.0) as f64;
            // The hand goes down and comes back up. An upstroke catches the
            // top strings first, hits fewer of them and is damped by the palm
            // on the way back: quieter and shorter, which is what stops a
            // strum from sounding like the same chord played twice.
            let up = strum > 0.0 && hits % 2 == 1;
            hits += 1;
            let vel = match up {
                true => style.vel.saturating_sub(26),
                false => style.vel.saturating_sub(10),
            };
            let len = match up {
                true => (len * 0.5).max(0.15),
                false => len,
            };
            let n = voicing.len();
            for (i, note) in voicing.into_iter().enumerate() {
                // The strum: the pick crosses the strings, it does not land on
                // them all at once. Zero for the piano, whose hands do.
                let across = match up {
                    true => (n - 1 - i) as f64,
                    false => i as f64,
                };
                let mut n = Note {
                    note,
                    vel,
                    start: (start + jitter + across * strum).max(0.0),
                    len,
                    // Who plays it is stamped by `bake`, which knows.
                    ..Default::default()
                };
                // Velocity alone, so the chord still lands together.
                let v = n.vel as i32 + (rng.f() * 10.0) as i32 - 5;
                n.vel = v.clamp(1, 127) as u8;
                out.push(n);
            }
        }
    }
}

/// The notes of one voicing, ascending, at the octave that moves least from
/// `prev_top`.
/// How far apart two notes have to start to be **different** chords, in beats.
/// A strum is one chord crossing the strings — a few hundredths of a beat — and
/// the hits of even the busiest comping are a sixteenth apart.
const HIT_WINDOW: f64 = 0.1;

/// Let go of a chord when the same instrument plays the next one.
///
/// `Comp::hold` is how long a chord *wants* to ring, and on a busy pattern it is
/// longer than the gap to the next hit: the notes pile up, four strums deep, and
/// what comes out is not a chord but every chord of the bar at once. That is the
/// wall an extended chart turned the accompaniment into — twenty-two voices
/// sounding together where a guitarist has six strings.
///
/// A little overlap is left on purpose: a hand comes off the strings as the next
/// one lands, and cutting exactly on the beat is a staccato nobody played.
fn damp(part: &mut [Note]) {
    /// How much of a beat a chord is allowed to ring into the next one.
    const LEGATO: f64 = 0.02;
    let mut starts: Vec<f64> = part.iter().map(|n| n.start).collect();
    starts.sort_by(f64::total_cmp);
    for n in part.iter_mut() {
        // The next hit of this instrument: the first start far enough along to
        // be another chord rather than the rest of this strum.
        let after = n.start + HIT_WINDOW;
        let at = starts.partition_point(|s| *s <= after);
        if let Some(next) = starts.get(at) {
            n.len = n.len.min((next - n.start + LEGATO).max(0.05));
        }
    }
}

/// How many notes one hand plays. A pianist voicing a thirteenth chord plays
/// four or five notes, not the eight the symbol spells: the rest are the bass
/// player's, or simply not played. Without this an extended chart handed the
/// comping every tone it could name — seven-note strums, twenty-two voices
/// sounding at once, and an accompaniment that was a wall.
const PIANO_NOTES: usize = 4;
const GUITAR_NOTES: usize = 5;

/// The tones worth keeping when there are more than a hand can play, most
/// characteristic first: the third says major or minor, the seventh says what
/// kind of seventh, the top tension is why the chord was written extended at
/// all, and an altered fifth is never decoration. The root and the plain fifth
/// go last — a bass player is playing the root.
fn voicing_priority(tone: i32) -> u8 {
    match tone.rem_euclid(12) {
        3 | 4 => 0,         // the third
        10 | 11 => 1,       // the seventh
        1 | 2 | 6 | 8 => 2, // the altered tensions and the altered fifth
        9 => 3,             // the sixth, or the thirteenth
        5 => 4,             // the eleventh
        0 => 5,             // the root
        _ => 6,             // the fifth
    }
}

/// The tones a hand of `max` notes plays, in the order they sound.
fn hand(tones: &[i32], max: usize) -> Vec<i32> {
    if tones.len() <= max {
        return tones.to_vec();
    }
    let mut kept: Vec<i32> = tones.to_vec();
    // Stable by priority, so two tones worth the same keep the order the chord
    // spelled them in.
    kept.sort_by_key(|t| (voicing_priority(*t), *t));
    kept.truncate(max);
    kept.sort_unstable();
    kept
}

fn voice(chord: &Chord, prev_top: u8, low: u8, high: u8, shell: bool, max: usize) -> Vec<u8> {
    // What is voiced: the tones that say what the chord *is*. With a seventh
    // there, the root and the fifth are the bass player's and are left out —
    // for the piano. A guitar keeps them: six strings under one hand is a
    // shape, and the shape has a root in it.
    let tones: Vec<i32> = match shell && chord.tones.len() >= 4 {
        true => chord
            .tones
            .iter()
            .copied()
            .filter(|t| !matches!(t.rem_euclid(12), 0 | 7))
            .collect(),
        false => chord.tones.clone(),
    };
    let tones = match tones.is_empty() {
        true => chord.tones.clone(),
        false => tones,
    };
    // A hand, not a spelling: an extended chord is voiced with the notes that
    // say what it is.
    let tones = hand(&tones, max.max(2));
    let pcs: Vec<u8> = tones
        .iter()
        .map(|t| ((chord.root as i32 + t).rem_euclid(12)) as u8)
        .collect();

    // Every inversion, at every octave that fits: the one whose top note moves
    // least from the last voicing wins. That is what turns `C7 (E Bb)` into
    // `F7 (Eb A)` instead of stacking both from their root and jumping an
    // octave whenever the bass note does.
    let mut best: Vec<u8> = Vec::new();
    let mut dist = i32::MAX;
    for rot in 0..pcs.len() {
        let order: Vec<u8> = (0..pcs.len()).map(|i| pcs[(i + rot) % pcs.len()]).collect();
        for base in (low as i32)..=(high as i32) {
            if base.rem_euclid(12) != order[0] as i32 {
                continue;
            }
            let mut notes = vec![base];
            for pc in order.iter().skip(1) {
                let last = *notes.last().unwrap();
                let mut n = (last / 12) * 12 + *pc as i32;
                while n <= last {
                    n += 12;
                }
                notes.push(n);
            }
            if notes.last().is_some_and(|n| *n > high as i32) {
                continue;
            }
            let d = (notes.last().unwrap() - prev_top as i32).abs();
            if d < dist {
                dist = d;
                best = notes.iter().map(|n| *n as u8).collect();
            }
        }
    }
    // Nothing fits between `low` and `high`: play what there is room for rather
    // than nothing.
    match best.is_empty() {
        true => vec![in_range(pcs[0], low, high, prev_top)],
        false => best,
    }
}

/// The length the guitar holds a chord for, against the piano's.
///
/// **Short against long, long against short.** Two chord instruments holding
/// the same length are one chord instrument played twice: what makes a rhythm
/// section out of a piano and a guitar is that one of them is ringing while the
/// other is chopping. So the piano's `hold` decides the guitar's, and the
/// dividing line is the beat — under it the piano is comping short and the
/// guitar rings the bar out; over it the piano is sustaining and the guitar
/// answers with the chop.
///
/// The bar is the ceiling either way: a chord held past the next one is a
/// chord playing over the change.
pub fn counter_hold(piano: f64, beats_per_bar: f64) -> f64 {
    let bar = beats_per_bar.max(1.0);
    match piano <= 1.0 {
        true => (bar - piano).max(1.5).min(bar),
        false => (piano * 0.25).clamp(0.2, 0.75),
    }
}

/// The guitar's register, from the style's piano comp.
///
/// What a style that did not write its own [`Style::guitar`] gets: the piano's
/// rhythm played on a guitar's strings.
fn guitar_of(c: super::style::Comp) -> super::style::Comp {
    super::style::Comp {
        // Open position: the low E to the top of the first few frets, which is
        // where a chord shape actually sits.
        low: 40,
        high: 72,
        // Shorter and busier than the piano's: a strummed chord rings and gets
        // damped, it is not held down.
        hold: (c.hold * 0.8).max(0.25),
        ..c
    }
}
