//! What a sample's own name already says, which is most of what an analyser
//! would have to work out.
//!
//! Sample packs are named by people, for people, and they nearly all say the
//! note in the filename — `violin_C4_mf.wav`,
//! `violin_A3_15_mezzo-forte_arco-normal.mp3`. Reading that costs microseconds;
//! detecting the same pitch from the audio costs a decode plus a few million
//! multiply-adds. So this runs first and the detector only sees what it could
//! not name.
//!
//! ## How it reads a name
//!
//! Split on every separator a pack has ever used (`_`, `-`, `.`, space) and
//! classify each token on its own. Order is not assumed: `C4_violin_mf` and
//! `violin_mf_C4` say the same thing, and enough packs do it both ways that a
//! positional parser would be wrong half the time.
//!
//! **What is left over is the name of a group**, and that turns out to be the
//! most useful thing here. Take the note and the dynamic out of
//! `violin_A3_15_mezzo-forte_arco-normal` and what remains —
//! `violin_15_arco-normal` — names the set that file belongs to: bowed, normal,
//! a second and a half long. Every note of that set is one instrument, its
//! dynamics are that instrument's velocity layers, and a set with a different
//! length or a different bowing is a *different* instrument that happens to
//! live in the same folder. No vocabulary of articulations is needed to see it,
//! which is why this beats naming them: Philharmonia alone ships `arco-normal`,
//! `pizz-normal`, `arco-sul-ponticello`, `natural-harmonic` and twenty more.
//!
//! **A bare number is never a round robin.** Philharmonia writes the note's
//! length there — `violin_A4_15_forte_arco-normal` is a second and a half, not
//! take fifteen — so a round robin has to be spelled (`RR2`, `take3`, `var1`).
//! The cost of guessing wrong is four samples that never play.

use super::Articulation;

/// Everything a filename gave up.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Parsed {
    /// The recorded pitch, as a MIDI note.
    pub note: Option<u8>,
    /// A velocity the name stated outright (`v80`) or named musically (`mf`).
    pub velocity: Option<u8>,
    pub articulation: Option<Articulation>,
    /// Which take this is, when the name says so.
    pub round_robin: Option<u32>,
    /// The name with the note, the dynamic and the take taken out: what is
    /// left names the set this file belongs to. Two files with the same group
    /// are the same instrument at different pitches or different strengths.
    pub group: String,
}

/// Read a file stem. Directory names go through the same door — a pack that
/// says `violin/staccato/C4.wav` is saying what one that says
/// `violin_C4_staccato.wav` says, and neither should need its own parser.
pub fn parse(stem: &str) -> Parsed {
    let mut out = Parsed::default();
    // Split on the separators that hold words apart but **not** on the hyphen:
    // `C-1` is a note whose octave is -1, and `mezzo-forte` is one dynamic
    // rather than two. The hyphen comes apart further down, per token, where
    // `arco-normal` needs it and these two do not.
    //
    // **Unless the hyphen is all the name has.** `Cello-C3-f` and
    // `Piano-A0-v64-rr2` are how a large share of packs are named, and there
    // the hyphen *is* the separator: read whole, the note was never found.
    let separators: &[char] = match stem.contains(['_', ' ']) {
        true => &['_', '.', ' '],
        false => &['_', '.', ' ', '-'],
    };
    let tokens: Vec<&str> = stem
        .split(separators)
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect();
    let mut group: Vec<&str> = Vec::with_capacity(tokens.len());

    for (index, token) in tokens.iter().enumerate() {
        // The first token is the instrument's name, and two instruments are
        // spelled exactly like a dynamic: `piano_C4_v80` is not a quiet
        // anything, and `forte-piano_C4` is a make of piano. So the leading
        // token never counts as a dynamic — unless it is all there is.
        let named = index == 0 && tokens.len() > 1;
        let lower = token.to_ascii_lowercase();

        if out.note.is_none() {
            if let Some(n) = note_value(&lower) {
                out.note = Some(n);
                continue;
            }
        }
        // `mezzo-forte` is looked up whole, with the hyphen taken out: read as
        // two words it would be `mezzo` and land on the wrong layer, and
        // `mezzo-piano` would land on the same wrong layer as `mezzo-forte`.
        if let Some(v) = dynamic_value(&lower.replace('-', "")).filter(|_| !named) {
            out.velocity = out.velocity.or(Some(v));
            continue;
        }
        if let Some(v) = velocity_value(&lower) {
            out.velocity = out.velocity.or(Some(v));
            continue;
        }
        if let Some(rr) = round_robin_value(&lower) {
            out.round_robin = out.round_robin.or(Some(rr));
            continue;
        }
        // Whatever is left may still be several words: `arco-normal`,
        // `very-long`, `natural-harmonic`. They say what the set is, so the
        // token stays in the group name whole — only the articulation label is
        // read out of it.
        for word in lower.split('-') {
            if let Some(a) = Articulation::from_word(word) {
                // The specific beats the generic: `arco-spiccato` is a
                // spiccato, and `arco` on its own would have called it an
                // ordinary bowed note.
                if out.articulation.is_none() || a != Articulation::Sustain {
                    out.articulation = Some(a);
                }
            }
        }
        group.push(token);
    }
    out.group = group.join("_");
    out
}

/// A note name with an octave: `C4`, `c#3`, `Db-1`. **Not** a bare number —
/// `60` in a filename is a duration or a take far more often than a MIDI note.
fn note_value(token: &str) -> Option<u8> {
    let mut chars = token.chars();
    let semitone: i32 = match chars.next()? {
        'c' => 0,
        'd' => 2,
        'e' => 4,
        'f' => 5,
        'g' => 7,
        'a' => 9,
        'b' => 11,
        _ => return None,
    };
    let rest: String = chars.collect();
    let (accidental, rest) = match rest.strip_prefix('#').or_else(|| rest.strip_prefix('s')) {
        Some(r) => (1, r),
        None => match rest.strip_prefix('b') {
            Some(r) => (-1, r),
            None => (0, rest.as_str()),
        },
    };
    // The octave is the whole remainder, so `c4x` is a word, not a note.
    let octave: i32 = rest.parse().ok()?;
    let midi = (octave + 1) * 12 + semitone + accidental;
    u8::try_from(midi).ok().filter(|n| *n <= 127)
}

/// `v80`, `vel80`, `velocity80`.
fn velocity_value(token: &str) -> Option<u8> {
    let digits = ["velocity", "vel", "v"]
        .iter()
        .find_map(|p| token.strip_prefix(p))?;
    let n: u32 = digits.parse().ok()?;
    (n > 0 && n <= 127).then_some(n as u8)
}

/// The musical dynamics, as the velocity a player would hit to get them.
/// Centres, not edges: the layers get their boundaries from each other later,
/// so what a name has to give is one number inside its layer.
fn dynamic_value(token: &str) -> Option<u8> {
    Some(match token {
        "pppp" | "ppp" => 16,
        "pp" | "pianissimo" => 32,
        "p" | "piano" => 48,
        "mp" | "mezzopiano" => 60,
        "mf" | "mezzoforte" => 76,
        "f" | "forte" => 96,
        "ff" | "fortissimo" => 112,
        "fff" | "ffff" => 127,
        // How drum and one-shot packs say it.
        "soft" | "light" => 40,
        "med" | "medium" => 80,
        "hard" | "loud" => 120,
        _ => return None,
    })
}

/// A spelled take: `rr2`, `take3`, `var1`, `variation4`.
fn round_robin_value(token: &str) -> Option<u32> {
    let digits = ["variation", "var", "take", "rr"]
        .iter()
        .find_map(|p| token.strip_prefix(p))?;
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_gives_up_its_note() {
        assert_eq!(parse("C4").note, Some(60));
        assert_eq!(parse("piano_C4").note, Some(60));
        assert_eq!(parse("C#4").note, Some(61));
        assert_eq!(parse("Db4").note, Some(61));
        assert_eq!(parse("violin_A4_15_forte_arco-normal").note, Some(69));
        // The octave is part of the note, and `C-1` is MIDI 0.
        assert_eq!(parse("kick_C-1").note, Some(0));
    }

    /// A bare number is a length, a take, a serial — anything but a pitch.
    /// Philharmonia's own names have one in the middle of every file.
    #[test]
    fn a_bare_number_is_never_a_note_and_never_a_round_robin() {
        assert_eq!(parse("violin_15_forte").note, None);
        assert_eq!(parse("violin_A4_15_forte").round_robin, None);
        assert_eq!(parse("snare_01").note, None);
    }

    #[test]
    fn velocity_is_read_from_a_number_or_from_a_dynamic() {
        assert_eq!(parse("piano_C4_v80").velocity, Some(80));
        assert_eq!(parse("piano_C4_vel127").velocity, Some(127));
        assert_eq!(parse("piano_C4_velocity40").velocity, Some(40));
        assert!(parse("violin_C4_pp").velocity < parse("violin_C4_mf").velocity);
        assert!(parse("violin_C4_mf").velocity < parse("violin_C4_ff").velocity);
        assert_eq!(
            parse("banjo_A3_forte").velocity,
            parse("banjo_A3_f").velocity
        );
        // The instrument, not a dynamic: a piano library is not recorded at
        // velocity 48 because of what the instrument is called.
        assert_eq!(parse("piano_C4").velocity, None);
        assert_eq!(parse("piano_C4_v80").velocity, Some(80));
    }

    /// A name held together only by hyphens is read the same as one held
    /// together by underscores — and a hyphen inside an underscored name is
    /// still part of a word.
    #[test]
    fn a_hyphenated_name_gives_up_its_note_and_dynamic() {
        let p = parse("Cello-C3-f-rr2");
        assert_eq!((p.note, p.velocity, p.round_robin), (Some(48), Some(96), Some(2)));
        assert_eq!(parse("Piano-A0-v64").note, Some(21));
        assert_eq!(parse("Snare-Hard").velocity, Some(120));
        assert_eq!(parse("violin_A4_15_mezzo-forte_arco-normal").velocity, Some(76));
        assert_eq!(parse("kick_C-1").note, Some(0));
    }

    #[test]
    fn takes_are_only_the_spelled_ones() {
        assert_eq!(parse("violin_C4_RR2").round_robin, Some(2));
        assert_eq!(parse("violin_C4_take3").round_robin, Some(3));
        assert_eq!(parse("violin_C4_var1").round_robin, Some(1));
    }

    #[test]
    fn articulation_comes_off_the_name_in_either_spelling() {
        assert_eq!(
            parse("violin_C4_staccato").articulation,
            Some(Articulation::Staccato)
        );
        // Philharmonia hyphenates them, and the split has already happened.
        assert_eq!(
            parse("violin_A4_1_forte_pizz-normal").articulation,
            Some(Articulation::Pizzicato)
        );
        assert_eq!(parse("violin_C4_mf").articulation, None);
    }
}
