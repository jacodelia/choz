//! The generators: a progression and a style in, a part in notes out.
//!
//! Everything here runs **once**, when the text or a setting changes, and never
//! from the audio thread: what it produces is a baked list of notes that
//! [`super::Arranger::tick`] only reads. Same progression, same style and same
//! seed give the same list, note for note — an interpretation you liked is one
//! you can get back, and a bug is one you can reproduce.

use super::chord::{Chord, Progression};
use super::style::{Lead, Style};

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
    Piano,
    /// The same chords with a guitar's voicings and a strum across them.
    Guitar,
    /// The tune: one line, in motifs, over the same progression.
    Melody,
    /// The same shape played busier and leaning on the chord changes.
    Solo,
}

impl Role {
    /// New roles go on the **end**: the variant's position is what a saved
    /// project wrote and what [`Rng::new`] seeds from, so moving one changes
    /// both the role a project reopens with and what it plays.
    pub const ALL: [Role; 6] = [
        Role::Bass,
        Role::Drums,
        Role::Piano,
        Role::Melody,
        Role::Solo,
        Role::Guitar,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Role::Bass => "BASS",
            Role::Drums => "DRUMS",
            Role::Piano => "PIANO",
            Role::Guitar => "GUITAR",
            Role::Melody => "MELODY",
            Role::Solo => "SOLO",
        }
    }
}

/// A note in the arrangement. Positions are in beats from the top of the
/// progression, which is the only timeline the roles share.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Note {
    pub note: u8,
    pub vel: u8,
    pub start: f64,
    pub len: f64,
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

/// How near two notes have to be in time to count as the same moment: an
/// eighth at the tempo everything defaults to.
const UNISON: f64 = 0.25;

/// Move what lands on the same note at the same moment as something in
/// `others` — up a step, or down when there is no room.
///
/// A unison between two lines is not a chord, it is one line with a thicker
/// tone, and the seed is all that was keeping them apart.
fn avoid(part: &mut [Note], others: &[Note], low: u8, high: u8) {
    for n in part.iter_mut() {
        let clash = others
            .iter()
            .any(|o| o.note == n.note && (o.start - n.start).abs() < UNISON);
        if clash {
            // A step up, or down when the top of the register is in the way.
            // Never out of the register: the line it belongs to is the line it
            // has to stay in.
            n.note = match n.note.saturating_add(2) <= high {
                true => n.note + 2,
                false => n.note.saturating_sub(2).max(low),
            };
        }
    }
}

/// The whole of a role's part.
pub fn bake(prog: &Progression, style: &Style, role: Role, seed: u32) -> Vec<Note> {
    let mut rng = Rng::new(seed, role, style.human * role_feel(role));
    let mut out = Vec::new();
    match role {
        Role::Bass => bass(prog, style, &mut rng, &mut out),
        Role::Drums => drums(prog, style, &mut rng, &mut out),
        Role::Piano => comp(prog, style, &mut rng, &mut out, style.comp, 0.0, true),
        Role::Guitar => comp(
            prog,
            style,
            &mut rng,
            &mut out,
            style.guitar.unwrap_or_else(|| guitar_of(style.comp)),
            style.strum.max(0.0),
            false,
        ),
        Role::Melody => lead(prog, style, &mut rng, &mut out, style.lead),
        Role::Solo => {
            lead(
                prog,
                style,
                &mut rng,
                &mut out,
                style.solo.unwrap_or_else(|| solo_of(style.lead)),
            );
            // The soloist hears the tune. Every role bakes on its own — that is
            // what lets one tab play one part — but the parts are a function of
            // the text and the seed, so the solo can bake the melody and get
            // out of its way. It is the only pair where landing on the same
            // note at the same time is heard as a mistake.
            let l = style.solo.unwrap_or_else(|| solo_of(style.lead));
            let melody = bake(prog, style, Role::Melody, seed);
            avoid(&mut out, &melody, l.low, l.high);
        }
    }
    let total = prog.beats(style.beats_per_bar);
    out.retain(|n| n.start < total && n.len > 0.0);
    // Nothing hangs past the end: the loop comes back round to the downbeat and
    // a note still sounding there would be a note the next chorus did not play.
    for n in &mut out {
        n.len = n.len.min(total - n.start);
    }
    out.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.note.cmp(&b.note)));
    out
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

/// How loose a role is allowed to be, against the style's own looseness. The
/// drummer is the clock and the soloist is the one allowed to lean on it;
/// putting the same jitter on both is what makes a generated band sound like
/// one player with six hands.
fn role_feel(role: Role) -> f32 {
    match role {
        Role::Drums => 0.6,
        Role::Bass => 0.8,
        Role::Piano | Role::Guitar => 1.0,
        Role::Melody => 1.2,
        Role::Solo => 1.4,
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
fn drums(prog: &Progression, style: &Style, rng: &mut Rng, out: &mut Vec<Note>) {
    let bpb = style.beats_per_bar;
    let d = style.drums;
    for bar in 0..prog.bars.len() {
        let at = bar as f64 * bpb;
        let fill = d.fill_every > 0 && (bar + 1) % d.fill_every == 0;
        // The turnaround: the bar that hands the form back to the top gets two
        // beats of fill instead of one. Everything else keeps its last beat.
        let turnaround = fill && bar + 1 == prog.bars.len() && bpb >= 3.0;
        let last_beat = match turnaround {
            true => bpb - 2.0,
            false => bpb - 1.0,
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
        let cymbal = match d.ride {
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
                push_hit(out, rng, cymbal, at + swung(pos, style), vel, 0.2);
            }
            pos += d.cymbal.max(0.125);
        }
        // Ghosts: quiet snares between the backbeats, which is the difference
        // between a drum machine and someone playing one.
        let mut g = 0.5;
        while g < bpb - 1e-9 {
            if !(fill && g >= last_beat) && rng.chance(d.ghost) {
                push_hit(
                    out,
                    rng,
                    gm::SNARE,
                    at + swung(g, style),
                    style.vel.saturating_sub(45).max(1),
                    0.15,
                );
            }
            g += 1.0;
        }
        // A fill is a table of four sixteenths, and which of the tables it is
        // comes off the dice: the same fill every four bars is the thing that
        // gives a generated drummer away in one chorus.
        if fill {
            // Two tables on the turnaround, one anywhere else — and never the
            // same table twice in a row, which is the whole point of there
            // being four of them.
            let first = rng.pick(FILLS.len());
            let shapes: Vec<[u8; 4]> = match turnaround {
                true => vec![FILLS[first], FILLS[(first + 1 + rng.pick(FILLS.len() - 1)) % FILLS.len()]],
                false => vec![FILLS[first]],
            };
            let hits: Vec<u8> = shapes.concat();
            let n = hits.len();
            for (i, note) in hits.into_iter().enumerate() {
                let pos = at + last_beat + i as f64 * 0.25;
                // Fills build: the last sixteenth is the one that hands the bar
                // back, and it is the loudest.
                let step = (12 * (n - 1 - i) / (n - 1).max(1)) as u8;
                push_hit(out, rng, note, pos, style.vel.saturating_sub(step), 0.2);
            }
        }
        // The crash marks where the form comes back round, downbeat of the bar
        // after a fill and the top of the arrangement.
        if bar == 0 || (d.fill_every > 0 && bar % d.fill_every == 0 && bar > 0) {
            push_hit(out, rng, gm::CRASH, at, style.vel, 0.5);
        }
    }
}

/// The fills, as tables: down the toms, a roll on the snare, and the one that
/// answers itself in pairs. Four sixteenths on the last beat, which is the
/// fill that fits in every style here.
const FILLS: &[[u8; 4]] = &[
    [gm::SNARE, gm::TOM_HI, gm::TOM_MID, gm::TOM_LO],
    [gm::SNARE, gm::SNARE, gm::SNARE, gm::SNARE],
    [gm::SNARE, gm::SNARE, gm::TOM_LO, gm::TOM_LO],
    [gm::TOM_HI, gm::TOM_HI, gm::TOM_MID, gm::SNARE],
];

fn push_hit(out: &mut Vec<Note>, rng: &mut Rng, note: u8, start: f64, vel: u8, len: f64) {
    let mut n = Note {
        note,
        vel,
        start,
        len,
    };
    human(rng, &mut n, 0.02);
    out.push(n);
}

// ─── Comping ────────────────────────────────────────────────────────────────

/// Chords, voiced and led: the third and the seventh with whatever tension the
/// chord carries, placed at the octave nearest where the last voicing sat. A
/// comp that re-stacks from the root every bar jumps an octave for no reason
/// and stops sounding like hands.
fn comp(
    prog: &Progression,
    style: &Style,
    rng: &mut Rng,
    out: &mut Vec<Note>,
    c: super::style::Comp,
    strum: f64,
    shell: bool,
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
            let voicing = voice(chord, prev_top, c.low, c.high, shell);
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
fn voice(chord: &Chord, prev_top: u8, low: u8, high: u8, shell: bool) -> Vec<u8> {
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

// ─── Melody and solo ────────────────────────────────────────────────────────

/// One bar of shape, before it knows what chord it is over.
///
/// A motif is what makes a melody a melody rather than an arpeggiator with
/// dice: the same shape comes back changed — transposed, pushed late, resolved
/// — instead of being rolled again.
#[derive(Clone)]
struct Motif {
    /// `(start in the bar, length in beats, degree of the scale relative to the
    /// phrase's first note)`.
    notes: Vec<(f64, f64, i32)>,
}

/// The durations a solo is built from: the melody's menu with the long notes
/// taken out, which is most of what makes one sound like the other played
/// busier.
const SOLO_NOTES: &[f64] = &[0.5, 0.5, 0.25, 0.75, 1.0];

/// The solo, from the style's melody.
///
/// What a style that did not write its own [`Style::solo`] gets: the melody
/// played busier and a little higher.
fn solo_of(l: Lead) -> Lead {
    Lead {
        notes: SOLO_NOTES,
        rest: l.rest * 0.4,
        chromatic: (l.chromatic * 2.0).min(0.8),
        // A soloist sits above the tune, and both stay clear of the comping.
        low: l.low.saturating_add(5).min(120),
        high: l.high.saturating_add(5).min(127),
        ..l
    }
}

/// A bar's worth of shape: durations off the style's menu, and a walk up and
/// down the scale in steps.
fn motif(rng: &mut Rng, l: &Lead, bpb: f64) -> Motif {
    let mut notes = Vec::new();
    let mut at = 0.0;
    let mut degree = 0i32;
    while at < bpb - 1e-9 {
        let len = l.notes[rng.pick(l.notes.len())].min(bpb - at);
        if !rng.chance(l.rest) {
            notes.push((at, len, degree));
            // Mostly by step, sometimes by a third, and on a leash: a random
            // walk with nothing holding it climbs out of the register and
            // never comes back.
            let step = match rng.chance(0.75) {
                true => 1,
                false => 2,
            };
            let step = match rng.chance(0.5) {
                true => step,
                false => -step,
            };
            degree = (degree + step).clamp(-7, 7);
        }
        at += len;
    }
    // A bar of nothing but rests is a bar the form loses; the dice do not get
    // to delete a phrase.
    if notes.is_empty() {
        notes.push((0.0, 1.0, 0));
    }
    Motif { notes }
}

/// The same figure, worked: one of its notes broken in two, and the walk it
/// makes widened by a step.
///
/// This is what "the motif grows" means and all it means — the shape is still
/// recognisably the one the phrase opened with, which is the whole point of
/// there being a motif at all.
fn develop(m: &Motif, l: &Lead, rng: &mut Rng, bpb: f64) -> Motif {
    let mut notes = m.notes.clone();
    if notes.is_empty() {
        return Motif { notes };
    }
    // Break the longest note in two: the busier version of the same shape.
    let at = notes
        .iter()
        .enumerate()
        .max_by(|a, b| a.1 .1.total_cmp(&b.1 .1))
        .map(|(i, _)| i)
        .unwrap_or(0);
    let (start, len, degree) = notes[at];
    // Only where both halves still land on a sixteenth: a dotted eighth broken
    // in two is two dotted sixteenths, which is not a figure anybody plays.
    let half = len / 2.0;
    if len >= 0.5 && ((half * 4.0) - (half * 4.0).round()).abs() < 1e-9 {
        let step = match rng.chance(0.5) {
            true => 1,
            false => -1,
        };
        notes[at] = (start, half, degree);
        notes.insert(at + 1, (start + half, half, degree + step));
    }
    // …and one of them says a little more than it did.
    let lift = rng.pick(notes.len());
    let (start, len, degree) = notes[lift];
    notes[lift] = (start, len, (degree + 1).clamp(-7, 7));
    // A rest opens where the dice say, so the figure does not fill up.
    if rng.chance(l.rest) && notes.len() > 2 {
        let drop = rng.pick(notes.len());
        notes.remove(drop);
    }
    notes.retain(|(s, len, _)| *s < bpb - 1e-9 && *len > 0.0);
    let _ = (start, len, degree);
    match notes.is_empty() {
        true => m.clone(),
        false => Motif { notes },
    }
}

/// A degree of the chord's scale, as a pitch.
///
/// This is the only place anything reads [`Chord::scale`]: the chord says what
/// may be played over it, and the line picks a rung of that ladder. Counted
/// from the scale's root at the octave nearest the register's middle, and
/// folded back inside when the walk leaves it.
fn scale_note(chord: &Chord, degree: i32, anchor: u8, low: u8, high: u8) -> u8 {
    let n = chord.scale.len().max(1) as i32;
    let semis = match chord.scale.is_empty() {
        true => 0,
        false => chord.scale[degree.rem_euclid(n) as usize] + 12 * degree.div_euclid(n),
    };
    let mut note = in_range(chord.root, low, high, anchor) as i32 + semis;
    while note > high as i32 {
        note -= 12;
    }
    while note < low as i32 {
        note += 12;
    }
    note.clamp(0, 127) as u8
}

/// A single line, in four-bar phrases: `A A' B A''`.
///
/// `A'` answers `A` a third up the scale and pushed late, `B` is the other
/// shape, and `A''` comes back down and lands on a chord tone — which is what
/// a phrase ending is. The melody and the solo are the same function with
/// different numbers: see [`solo_of`].
fn lead(prog: &Progression, style: &Style, rng: &mut Rng, out: &mut Vec<Note>, l: Lead) {
    let bpb = style.beats_per_bar;
    let a = motif(rng, &l, bpb);
    let b = motif(rng, &l, bpb);
    let anchor = (l.low / 2) + (l.high / 2);
    // The phrase the last four bars grew into. A figure that comes back exactly
    // as it left is an exercise; one that comes back busier is a tune being
    // played by somebody.
    let mut grown = a.clone();
    for bar in 0..prog.bars.len() {
        // Each time round the four bars, the figure is worked a little more.
        if bar > 0 && bar % 4 == 0 {
            grown = develop(&grown, &l, rng, bpb);
        }
        let (m, transpose, shift, resolve) = match bar % 4 {
            0 => (&grown, 0, 0.0, false),
            1 => (&grown, 2, l.displace, false),
            2 => (&b, 0, 0.0, false),
            _ => (&grown, -1, 0.0, true),
        };
        let last = m.notes.len() - 1;
        for (i, (start, len, degree)) in m.notes.iter().copied().enumerate() {
            let beat = bar as f64 * bpb + start + shift;
            let Some((chord, left)) = prog.at(beat, bpb) else {
                continue;
            };
            let mut note = scale_note(chord, degree + transpose, anchor, l.low, l.high);
            // The end of a phrase is where a line says something: land on a
            // chord tone rather than wherever the walk had got to.
            if i == last && resolve {
                note = chord.nearest_tone(note).clamp(l.low, l.high);
            }
            // …and the note before a chord change leans into the next chord
            // from a semitone away, which is the whole of why a line sounds
            // like it knew the change was coming.
            if left <= len + 1e-6 && rng.chance(l.chromatic) {
                if let Some(next) = prog.next_after(beat, bpb) {
                    let target = next.nearest_tone(note);
                    note = match target >= note {
                        true => target.saturating_sub(1),
                        false => target.saturating_add(1),
                    }
                    .clamp(l.low, l.high);
                }
            }
            let mut n = Note {
                note,
                vel: style.vel.saturating_sub(4),
                start: swung(beat, style),
                // Articulated rather than legato: a line that ties every note
                // into the next is one nobody can hear the rhythm of.
                len: len * 0.9,
            };
            human(rng, &mut n, 0.02);
            out.push(n);
        }
    }
}
