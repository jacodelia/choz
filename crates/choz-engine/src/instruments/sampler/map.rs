//! Samples in, playable regions out.
//!
//! The rules are the ones a person would use looking at the same list of
//! files, written down:
//!
//! * **One group at a time.** A folder holding sustains, staccatos and
//!   pizzicatos of the same notes has three instruments in it, and stacking
//!   them on one keyboard means every note plays whichever the scan listed
//!   first. The group that covers the most of the keyboard wins — see
//!   [`name`](super::name) for what a group is — and the rest wait in the map
//!   for the keyswitches a later version will hang them on.
//! * **Coverage first, then length.** Philharmonia's violin ships its ordinary
//!   bowed note at four lengths, and all four cover the same 49 pitches; the
//!   one with the *most files* is the quarter-second one, which would make a
//!   sampler nobody could play a chord on. So a tie on coverage goes to the
//!   group with the biggest files, which is the group with the longest notes.
//! * **Boundaries at the midpoint.** A sample stretched up sounds thin and one
//!   stretched down sounds slow, equally, so the split between two roots goes
//!   halfway between them: C3 (48) and C4 (60) meet at 54.
//! * **Velocity layers split the same way**, by the midpoint between the
//!   velocities the names stated.
//! * **Same note, same layer, more than one file** is a round robin, whether
//!   or not the name spelled it `RR`. Two takes of one note *are* alternatives;
//!   nothing else they could be would be worth playing only one of.

use crate::instruments::sfz::SfzRegion;

use super::{Articulation, Mode, Sample};

/// The first key on a drum map, which is where every hardware sampler and
/// every GM kit puts the kick.
const DRUM_BASE: u8 = 36;

/// Build the playable map: a keyboard of transposed multisamples, or a kit of
/// one-shots, depending on what the folder turns out to be.
pub fn regions(samples: &[Sample]) -> Vec<SfzRegion> {
    regions_with(samples, Mode::Auto)
}

/// The same, with the layout forced. `Stretch` on a folder where nothing has a
/// pitch still comes out a kit: there is no root note to transpose away from,
/// and stretching silence across the keyboard is not a thing that can be done.
pub fn regions_with(samples: &[Sample], mode: Mode) -> Vec<SfzRegion> {
    if mode == Mode::Slice {
        return slice_map(samples);
    }
    let pitched = match mode {
        Mode::Auto => looks_pitched(samples),
        Mode::Stretch => samples.iter().any(|s| s.root.is_some()),
        Mode::Kit | Mode::Slice => false,
    };
    if !pitched {
        // A kit is not grouped: there are no variants of one note to choose
        // between, only different sounds, and every one of them wants a key.
        return drum_map(&samples.iter().collect::<Vec<_>>());
    }
    let group = primary_group(samples);
    let chosen: Vec<&Sample> = samples
        .iter()
        .filter(|s| s.group == group && s.root.is_some())
        .collect();
    pitched_map(&chosen)
}

/// Whether this folder is an instrument to be played across a keyboard, or a
/// box of sounds to be laid out one per key.
///
/// **A named note is a statement and a detected one is a guess**, and the
/// difference decides this. Philharmonia's bass drum is the case that taught
/// it: the detector finds a period in a bass drum, because a bass drum has one
/// — around 37 Hz, note 25, and it is not wrong. But two hits that both come
/// out at note 25 are not a multisample of anything, and stretching them over
/// the keyboard makes a kick that plays chords.
///
/// So a folder is pitched when its notes were *named*. Failing that it has to
/// look like an instrument twice over: **nearly every file has a pitch**, and
/// those pitches **span at least an octave**. A drum folder fails the first —
/// half of Philharmonia's bass drums come back unpitched, because half of them
/// are — and it was the file that only checked the second which mapped a kick
/// drum across the keyboard.
fn looks_pitched(samples: &[Sample]) -> bool {
    let found: Vec<u8> = samples.iter().filter_map(|s| s.root).collect();
    if found.is_empty() {
        return false;
    }
    if samples
        .iter()
        .filter(|s| s.root.is_some())
        .all(|s| s.from_name)
    {
        return true;
    }
    // Four out of five. An instrument pack has a pitch in every file; the odd
    // miss is a detector that shrugged at one bad recording, not a kit.
    if found.len() * 5 < samples.len() * 4 {
        return false;
    }
    let (lo, hi) = found
        .iter()
        .fold((127u8, 0u8), |(lo, hi), r| (lo.min(*r), hi.max(*r)));
    hi.saturating_sub(lo) >= 12
}

/// Which set of samples this folder is going to play.
///
/// Three questions, in order, and the order is what the real libraries taught:
///
/// 1. **Which reaches most of the keyboard?** A set with holes in it is a set
///    whose holes get filled by stretching its neighbours.
/// 2. **Within a tenth of that, which is an ordinary note?** Philharmonia's
///    mandolin ships `normal` and `tremolo` at the same 39 pitches, and a
///    library that opened on the tremolo would be a surprise every time.
/// 3. **And of those, which has the biggest files?** Bigger means longer.
///    Its saxophone covers 41 notes at a quarter-second and 40 at a second and
///    a half; strict coverage buys that one extra note for samples nobody can
///    play a phrase with, which is why the first question has a tolerance.
pub fn primary_group(samples: &[Sample]) -> String {
    let mut names: Vec<&str> = samples.iter().map(|s| s.group.as_str()).collect();
    names.sort_unstable();
    names.dedup();

    // ponytail: a pass over the samples per distinct group, so O(groups × n).
    // A folder is a few thousand files and a few dozen groups, once, off the
    // audio thread. Bucketing into a map first is faster and longer.
    let stats: Vec<(&str, usize, bool, u64)> = names
        .iter()
        .map(|group| {
            let members: Vec<&Sample> = samples.iter().filter(|s| s.group == *group).collect();
            let mut notes: Vec<u8> = members.iter().filter_map(|s| s.root).collect();
            notes.sort_unstable();
            notes.dedup();
            let ordinary = members
                .first()
                .is_some_and(|s| s.articulation == Articulation::Sustain);
            // Mean, not total: a set of many short files would otherwise beat
            // a set of few long ones, which is the tie this exists to break.
            let mean = members.iter().map(|s| s.bytes).sum::<u64>() / members.len().max(1) as u64;
            (*group, notes.len(), ordinary, mean)
        })
        .collect();

    let widest = stats.iter().map(|s| s.1).max().unwrap_or(0);
    let mut best: Option<&(&str, usize, bool, u64)> = None;
    for candidate in stats.iter().filter(|s| s.1 * 10 >= widest * 9) {
        // Strictly greater, over a list already in name order: ties keep the
        // first, so the same folder always builds the same instrument.
        if best.is_none_or(|b| (candidate.2, candidate.3) > (b.2, b.3)) {
            best = Some(candidate);
        }
    }
    best.map(|b| b.0.to_string()).unwrap_or_default()
}

/// One file per key, from C2 up, played at the pitch it was recorded at.
///
/// ponytail: alphabetical, so `kick` `snare` `hat` land in that order rather
/// than the drummer's. Naming the parts (kick → 36, snare → 38, the GM map) is
/// worth doing when someone asks for it; until then the order is at least the
/// one the folder listing shows.
fn drum_map(samples: &[&Sample]) -> Vec<SfzRegion> {
    let mut sorted: Vec<&&Sample> = samples.iter().collect();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));
    // From C2, where every hardware kit puts its kick — unless there are more
    // sounds than there are keys above it, and then from the bottom of the
    // keyboard, because a sound with no key is a sound that cannot be played
    // at all. Philharmonia's whole `percussion.zip` read as one kit is 148 of
    // them; this is what keeps 128 rather than 92.
    let base = match sorted.len() > (128 - DRUM_BASE as usize) {
        true => 0,
        false => DRUM_BASE,
    };
    sorted
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            let key = base.checked_add(u8::try_from(i).ok()?)?;
            (key <= 127).then(|| SfzRegion {
                sample: s.path.clone(),
                lo_key: key,
                hi_key: key,
                // Its own key, so a one-shot plays at the speed it was
                // recorded: a kick transposed is a different drum.
                pitch_key_center: key,
                lo_vel: 0,
                hi_vel: 127,
                gain: 1.0,
                tune_cents: 0.0,
                start: 0.0,
                end: 1.0,
            })
        })
        .collect()
}

/// One sample cut into [`super::SLICES`] equal pieces, one per key from
/// [`DRUM_BASE`] up.
///
/// The longest file in the folder, because that is the break: a folder that
/// also holds one-shots would otherwise get sliced on whichever came first.
/// Each piece plays at the speed it was recorded — a slice transposed is a
/// different drum, same as a kit.
fn slice_map(samples: &[Sample]) -> Vec<SfzRegion> {
    let Some(sample) = samples.iter().max_by_key(|s| s.bytes) else {
        return Vec::new();
    };
    let n = super::SLICES;
    (0..n)
        .filter_map(|i| {
            let key = DRUM_BASE.checked_add(u8::try_from(i).ok()?)?;
            (key <= 127).then(|| SfzRegion {
                sample: sample.path.clone(),
                lo_key: key,
                hi_key: key,
                pitch_key_center: key,
                lo_vel: 0,
                hi_vel: 127,
                gain: 1.0,
                tune_cents: 0.0,
                start: i as f32 / n as f32,
                end: (i + 1) as f32 / n as f32,
            })
        })
        .collect()
}

fn pitched_map(samples: &[&Sample]) -> Vec<SfzRegion> {
    let layers = velocity_layers(samples);
    let mut out = Vec::new();
    for (velocity, lo_vel, hi_vel) in &layers {
        let in_layer: Vec<&&Sample> = samples
            .iter()
            .filter(|s| s.velocity.unwrap_or(*velocity) == *velocity)
            .collect();
        let mut roots: Vec<u8> = in_layer.iter().filter_map(|s| s.root).collect();
        roots.sort_unstable();
        roots.dedup();
        for sample in &in_layer {
            let Some(root) = sample.root else { continue };
            let (lo_key, hi_key) = key_zone(root, &roots);
            out.push(SfzRegion {
                sample: sample.path.clone(),
                lo_key,
                hi_key,
                pitch_key_center: root,
                lo_vel: *lo_vel,
                hi_vel: *hi_vel,
                gain: 1.0,
                // What the detector found the sample to be *off* by, taken back
                // out: a note recorded 30 cents flat plays in tune.
                tune_cents: -sample.cents,
                start: 0.0,
                end: 1.0,
            });
        }
    }
    out
}

/// Where one root's zone starts and ends, given every root in its layer.
/// Halfway to each neighbour; the outermost roots take the rest of the
/// keyboard, because a key with no sample under it is a dead key.
fn key_zone(root: u8, roots: &[u8]) -> (u8, u8) {
    let below = roots.iter().copied().filter(|r| *r < root).max();
    let above = roots.iter().copied().filter(|r| *r > root).min();
    let lo = match below {
        Some(b) => midpoint(b, root) + 1,
        None => 0,
    };
    let hi = match above {
        Some(a) => midpoint(root, a),
        None => 127,
    };
    (lo, hi)
}

fn midpoint(a: u8, b: u8) -> u8 {
    a + (b - a) / 2
}

/// The velocity layers, as `(the layer's own velocity, lo, hi)`.
///
/// A pack that never says a velocity has one layer covering the keyboard —
/// which is not the same as no layers, and a sampler that answered nothing to
/// a note played at 30 would be silent for half of what a player does.
fn velocity_layers(samples: &[&Sample]) -> Vec<(u8, u8, u8)> {
    let mut stated: Vec<u8> = samples.iter().filter_map(|s| s.velocity).collect();
    stated.sort_unstable();
    stated.dedup();
    if stated.is_empty() {
        return vec![(127, 0, 127)];
    }
    stated
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let lo = match i {
                0 => 0,
                _ => midpoint(stated[i - 1], *v) + 1,
            };
            let hi = match stated.get(i + 1) {
                Some(next) => midpoint(*v, *next),
                None => 127,
            };
            (*v, lo, hi)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn sample(name: &str, root: Option<u8>, velocity: Option<u8>) -> Sample {
        // The group is what the parser would have left over; the tests that
        // care set it themselves.
        Sample {
            path: PathBuf::from(name),
            group: String::new(),
            bytes: 1000,
            root,
            cents: 0.0,
            velocity,
            articulation: Articulation::Unknown,
            round_robin: None,
            confidence: 1.0,
            from_name: true,
            peak: 1.0,
            rms: 0.5,
            frames: 1000,
            sample_rate: 48_000,
        }
    }

    /// Three roots an octave apart cover the whole keyboard, and the splits
    /// land halfway between them — 48 and 60 meet at 54.
    #[test]
    fn key_zones_meet_at_the_midpoint_and_leave_no_dead_keys() {
        let samples = [
            sample("a_C3.wav", Some(48), None),
            sample("a_C4.wav", Some(60), None),
            sample("a_C5.wav", Some(72), None),
        ];
        let regions = regions(&samples);
        let zone = |root: u8| {
            let r = regions
                .iter()
                .find(|r| r.pitch_key_center == root)
                .expect("no region for that root");
            (r.lo_key, r.hi_key)
        };
        assert_eq!(zone(48), (0, 54));
        assert_eq!(zone(60), (55, 66));
        assert_eq!(zone(72), (67, 127));
    }

    /// One sample plays everywhere, transposed. A sampler that mapped it only
    /// to its own key would be a one-note instrument.
    #[test]
    fn a_lone_sample_covers_the_keyboard() {
        let regions = regions(&[sample("piano_C4.wav", Some(60), None)]);
        assert_eq!(regions.len(), 1);
        assert_eq!((regions[0].lo_key, regions[0].hi_key), (0, 127));
        assert_eq!(regions[0].pitch_key_center, 60);
    }

    #[test]
    fn velocity_layers_split_between_the_velocities_the_names_stated() {
        let samples = [
            sample("p_C4_v40.wav", Some(60), Some(40)),
            sample("p_C4_v80.wav", Some(60), Some(80)),
            sample("p_C4_v127.wav", Some(60), Some(127)),
        ];
        let regions = regions(&samples);
        let range = |name: &str| {
            let r = regions
                .iter()
                .find(|r| r.sample.to_string_lossy().contains(name))
                .expect("no region for that sample");
            (r.lo_vel, r.hi_vel)
        };
        assert_eq!(range("v40"), (0, 60));
        assert_eq!(range("v80"), (61, 103));
        assert_eq!(range("v127"), (104, 127));
        // And every layer still covers the whole keyboard.
        assert!(regions.iter().all(|r| (r.lo_key, r.hi_key) == (0, 127)));
    }

    /// Two takes of one note, same layer: both regions, identical ranges. The
    /// player is what alternates between them.
    #[test]
    fn takes_of_one_note_all_become_regions() {
        let mut a = sample("v_C4_RR1.wav", Some(60), None);
        a.round_robin = Some(1);
        let mut b = sample("v_C4_RR2.wav", Some(60), None);
        b.round_robin = Some(2);
        let regions = regions(&[a, b]);
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[0].lo_key, regions[1].lo_key);
        assert_eq!(regions[0].hi_key, regions[1].hi_key);
    }

    /// The group that reaches more of the keyboard is the one that plays; the
    /// others do not get stacked on top of it.
    #[test]
    fn only_the_group_that_covers_most_of_the_keyboard_is_mapped() {
        let grouped = |name: &str, root: u8, group: &str| {
            let mut s = sample(name, Some(root), None);
            s.group = group.into();
            s
        };
        let regions = regions(&[
            grouped("v_C4_sustain.wav", 60, "v_sustain"),
            grouped("v_C5_sustain.wav", 72, "v_sustain"),
            grouped("v_C4_staccato.wav", 60, "v_staccato"),
        ]);
        assert_eq!(regions.len(), 2);
        assert!(regions
            .iter()
            .all(|r| r.sample.to_string_lossy().contains("sustain")));
    }

    /// Philharmonia's saxophone, in miniature: the short set reaches one more
    /// pitch than the long one, and buying that pitch with three quarters of
    /// every note's length is a bad trade.
    #[test]
    fn one_extra_note_does_not_buy_the_shorter_samples() {
        let of = |name: &str, root: u8, group: &str, bytes: u64| {
            let mut s = sample(name, Some(root), None);
            s.group = group.into();
            s.bytes = bytes;
            s
        };
        let mut samples = vec![
            of("s_C4_15.wav", 60, "s_15", 14_000),
            of("s_C5_15.wav", 72, "s_15", 14_000),
            of("s_D5_15.wav", 74, "s_15", 14_000),
            of("s_E5_15.wav", 76, "s_15", 14_000),
            of("s_F5_15.wav", 77, "s_15", 14_000),
            of("s_G5_15.wav", 79, "s_15", 14_000),
            of("s_A5_15.wav", 81, "s_15", 14_000),
            of("s_B5_15.wav", 83, "s_15", 14_000),
            of("s_C6_15.wav", 84, "s_15", 14_000),
            of("s_D6_15.wav", 86, "s_15", 14_000),
        ];
        // The same ten pitches plus one, at a fifth of the length.
        for (i, root) in [60, 72, 74, 76, 77, 79, 81, 83, 84, 86, 88]
            .iter()
            .enumerate()
        {
            samples.push(of(&format!("s_{i}_025.wav"), *root, "s_025", 5_000));
        }
        let regions = regions(&samples);
        assert!(
            regions
                .iter()
                .all(|r| r.sample.to_string_lossy().contains("_15")),
            "the short set won on one note"
        );
    }

    /// Two sets covering exactly the same pitches: the ordinary one plays.
    /// Philharmonia's mandolin is `normal` and `tremolo` at 39 pitches each,
    /// and an instrument that opened on the tremolo would be a surprise.
    #[test]
    fn a_dead_tie_goes_to_the_ordinary_articulation() {
        let of = |name: &str, root: u8, group: &str, art: Articulation, bytes: u64| {
            let mut s = sample(name, Some(root), None);
            s.group = group.into();
            s.articulation = art;
            s.bytes = bytes;
            s
        };
        let regions = regions(&[
            of(
                "m_C4_trem.wav",
                60,
                "m_tremolo",
                Articulation::Tremolo,
                40_000,
            ),
            of(
                "m_C5_trem.wav",
                72,
                "m_tremolo",
                Articulation::Tremolo,
                40_000,
            ),
            // Slightly smaller, which a size-only tie-break would lose on.
            of("m_C4.wav", 60, "m_normal", Articulation::Sustain, 38_000),
            of("m_C5.wav", 72, "m_normal", Articulation::Sustain, 38_000),
        ]);
        assert!(regions
            .iter()
            .all(|r| !r.sample.to_string_lossy().contains("_trem")));
    }

    /// A tie on coverage between two sets of the same kind goes to the longer
    /// samples, which is what the bigger files mean.
    #[test]
    fn a_tie_on_coverage_goes_to_the_longer_samples() {
        let of = |name: &str, root: u8, group: &str, bytes: u64| {
            let mut s = sample(name, Some(root), None);
            s.group = group.into();
            s.bytes = bytes;
            s
        };
        let mut samples = vec![
            of("v_C4_15.wav", 60, "v_15", 23_000),
            of("v_C5_15.wav", 72, "v_15", 23_000),
        ];
        // Three times as many files, each a fraction of the length.
        for i in 0..3 {
            samples.push(of(&format!("v_C4_025_{i}.wav"), 60, "v_025", 12_000));
            samples.push(of(&format!("v_C5_025_{i}.wav"), 72, "v_025", 12_000));
        }
        let regions = regions(&samples);
        assert_eq!(regions.len(), 2, "the short set was mapped");
        assert!(regions
            .iter()
            .all(|r| r.sample.to_string_lossy().contains("_15")));
    }

    /// The user overrules the guess. A pack of unnamed one-note stabs is meant
    /// to be played across the keyboard even though nothing about it says so,
    /// and a set of tuned toms is not even though everything about it does.
    #[test]
    fn the_mode_overrules_what_the_samples_look_like() {
        let hit = |name: &str, root: Option<u8>| {
            let mut s = sample(name, root, None);
            s.from_name = false;
            s
        };
        // Left alone this is a kit: unnamed, detected, all in a heap.
        let kit_shaped = [
            hit("a.wav", Some(25)),
            hit("b.wav", Some(26)),
            hit("c.wav", None),
        ];
        assert!(regions(&kit_shaped).iter().all(|r| r.lo_key == r.hi_key));

        // STRETCH spreads the two that have a pitch over the keyboard.
        let stretched = regions_with(&kit_shaped, Mode::Stretch);
        assert_eq!(stretched.len(), 2, "the unpitched one cannot be stretched");
        assert_eq!((stretched[0].lo_key, stretched[1].hi_key), (0, 127));

        // And KIT flattens an instrument back to one key per sample.
        let instrument = [
            sample("a_C3.wav", Some(48), None),
            sample("a_C4.wav", Some(60), None),
        ];
        let flat = regions_with(&instrument, Mode::Kit);
        assert_eq!(flat.len(), 2);
        assert!(flat.iter().all(|r| r.lo_key == r.hi_key));
        assert!(flat.iter().all(|r| r.lo_key == r.pitch_key_center));
    }

    /// More sounds than there are keys above C2: the kit starts at the bottom
    /// of the keyboard instead of dropping the ones that fall off the top.
    #[test]
    fn a_kit_too_big_for_c2_starts_at_the_bottom() {
        let samples: Vec<Sample> = (0..120)
            .map(|i| sample(&format!("hit-{i:03}.wav"), None, None))
            .collect();
        let regions = regions(&samples);
        assert_eq!(regions.len(), 120, "sounds were dropped off the end");
        assert_eq!(regions[0].lo_key, 0);
        assert_eq!(regions[119].lo_key, 119);
    }

    /// A detector that finds the same low note in every file has not found an
    /// instrument. Philharmonia's bass drum folder is exactly this: real
    /// periods, all within a semitone of each other, none of them named.
    #[test]
    fn detected_pitches_all_in_a_heap_are_a_kit_not_an_instrument() {
        let hit = |name: &str, root: u8| {
            let mut s = sample(name, Some(root), None);
            s.from_name = false;
            s.confidence = 0.7;
            s
        };
        // And two that the detector would not commit to at all, which is the
        // other half of what a real kit folder looks like.
        let mut quiet = sample("bass-drum_thud.mp3", None, None);
        quiet.from_name = false;
        let mut quieter = sample("bass-drum_soft.mp3", None, None);
        quieter.from_name = false;
        let regions = regions(&[
            hit("bass-drum_mallet.mp3", 25),
            hit("bass-drum_rute.mp3", 26),
            // Junk an octave off, which is what a detector does with a drum:
            // the spread alone must not be enough to call this an instrument.
            hit("bass-drum_rhythm.mp3", 44),
            quiet,
            quieter,
        ]);
        assert_eq!(regions.len(), 5);
        // One key each, at its own pitch: a kit.
        assert!(regions.iter().all(|r| r.lo_key == r.hi_key));
        assert!(regions.iter().all(|r| r.lo_key == r.pitch_key_center));
    }

    /// The same guesses, an octave apart, are an instrument — that spread is
    /// something a drum cannot produce and a real instrument cannot avoid.
    #[test]
    fn detected_pitches_spread_over_an_octave_are_an_instrument() {
        let of = |name: &str, root: u8| {
            let mut s = sample(name, Some(root), None);
            s.from_name = false;
            s
        };
        let regions = regions(&[of("a.wav", 48), of("b.wav", 60), of("c.wav", 72)]);
        assert_eq!(regions.len(), 3);
        assert!(regions.iter().any(|r| r.hi_key > r.lo_key + 1));
    }

    /// Nothing in the folder has a pitch: it is a drum kit, and a drum kit is
    /// one hit per key at the speed it was recorded — not one hit stretched
    /// across the keyboard.
    #[test]
    fn a_folder_with_no_pitches_becomes_a_drum_map() {
        let regions = regions(&[
            sample("hat.wav", None, None),
            sample("kick.wav", None, None),
            sample("snare.wav", None, None),
        ]);
        assert_eq!(regions.len(), 3);
        assert_eq!(regions[0].sample.to_string_lossy(), "hat.wav");
        assert_eq!((regions[0].lo_key, regions[0].hi_key), (36, 36));
        assert_eq!(regions[0].pitch_key_center, 36);
        assert_eq!((regions[2].lo_key, regions[2].hi_key), (38, 38));
    }

    /// SLICE cuts the longest file into a piece per key, and the pieces cover
    /// the whole sample end to end with no gap and no overlap.
    #[test]
    fn slice_cuts_one_sample_into_a_piece_per_key() {
        let mut long = sample("break.wav", None, None);
        long.bytes = 500_000;
        let samples = [sample("stab.wav", None, None), long];
        let regions = regions_with(&samples, Mode::Slice);
        assert_eq!(regions.len(), super::super::SLICES);
        assert!(regions
            .iter()
            .all(|r| r.sample == Path::new("break.wav")));
        // Consecutive keys from the drum base, each playing the next piece.
        for (i, r) in regions.iter().enumerate() {
            assert_eq!(r.lo_key, DRUM_BASE + i as u8);
            assert_eq!(r.lo_key, r.hi_key);
            // Its own key: a slice transposed is a different sound.
            assert_eq!(r.pitch_key_center, r.lo_key);
            assert!((r.start - i as f32 / super::super::SLICES as f32).abs() < 1e-6);
            assert!((r.end - r.start - 1.0 / super::super::SLICES as f32).abs() < 1e-6);
        }
        assert_eq!(regions.last().map(|r| r.end), Some(1.0));
    }
}
