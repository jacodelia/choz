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

/// Where a sound with no pitch of its own plays at its recorded speed when
/// STRETCH spreads it over the keyboard: middle C.
const STRETCH_ROOT: u8 = 60;

/// Build the playable map: a keyboard of transposed multisamples, or a kit of
/// one-shots, depending on what the folder turns out to be.
pub fn regions(samples: &[Sample]) -> Vec<SfzRegion> {
    regions_with(samples, Mode::Auto)
}

/// The same, with the layout forced. `Stretch` on a folder where nothing has a
/// pitch plays it across the keyboard from middle C — see [`STRETCH_ROOT`].
pub fn regions_with(samples: &[Sample], mode: Mode) -> Vec<SfzRegion> {
    regions_with_slices(samples, mode, super::SLICES)
}

/// The same, told how many pieces `SLICE` cuts into — see
/// [`super::slices_of_id`]. Every other layout ignores it.
pub fn regions_with_slices(samples: &[Sample], mode: Mode, slices: usize) -> Vec<SfzRegion> {
    if mode == Mode::Slice {
        return slice_map(samples, slices.max(1));
    }
    let pitched = match mode {
        Mode::Auto => looks_pitched(samples),
        Mode::Stretch => true,
        Mode::Kit | Mode::Slice => false,
    };
    // **STRETCH on sounds with no pitch at all** — a cabasa, a shaker, a noise
    // — plays them across the keyboard anyway, as recorded at middle C: the
    // player asked for every key, and a sound transposed is still a sound.
    // A folder where only some files have a pitch stretches those, as before.
    let unpitched: Vec<Sample>;
    let samples = match mode == Mode::Stretch && samples.iter().all(|s| s.root.is_none()) {
        true => {
            unpitched = samples
                .iter()
                .cloned()
                .map(|mut s| {
                    s.root = Some(STRETCH_ROOT);
                    s
                })
                .collect();
            &unpitched[..]
        }
        false => samples,
    };
    if !pitched {
        // A kit is not grouped: there are no variants of one note to choose
        // between, only different sounds, and every one of them wants a key.
        return drum_map(&samples.iter().collect::<Vec<_>>());
    }
    // Every way of playing the note, not just the most recorded one: the
    // primary group opens the instrument and the others sit behind keyswitches
    // — see [`articulation_groups`]. A pack with sustain and staccato in it was
    // half a pack while only one of them was in the map.
    let groups = articulation_groups(samples);
    let mut out = Vec::new();
    for (index, group) in groups.iter().enumerate() {
        let chosen: Vec<&Sample> = samples
            .iter()
            .filter(|s| s.group == *group && s.root.is_some())
            .collect();
        for mut region in pitched_map(&chosen) {
            region.articulation = index;
            out.push(region);
        }
    }
    out
}

/// The groups an instrument can be played with, the primary one first.
///
/// Only groups that cover enough of the keyboard to be an articulation of the
/// same instrument rather than an odd file or two: a third of what the primary
/// group reaches. A pack with one way of playing a note gives one group, which
/// is what every kit and nearly every free library is.
pub fn articulation_groups(samples: &[Sample]) -> Vec<String> {
    let primary = primary_group(samples);
    let notes = |group: &str| -> usize {
        let mut roots: Vec<u8> = samples
            .iter()
            .filter(|s| s.group == group && s.root.is_some())
            .filter_map(|s| s.root)
            .collect();
        roots.sort_unstable();
        roots.dedup();
        roots.len()
    };
    let widest = notes(&primary);
    let mut rest: Vec<String> = samples
        .iter()
        .map(|s| s.group.clone())
        .filter(|g| *g != primary && notes(g) * 3 >= widest)
        .collect();
    rest.sort();
    rest.dedup();
    let mut out = vec![primary];
    out.extend(rest);
    out
}

/// Where the keyswitches go: the octave under the lowest key anything is
/// mapped to, and `None` when there is no room or nothing to switch.
///
/// Under the map rather than over it, because a library's top octave is where
/// its highest notes are and its bottom one is usually empty — and because
/// under is where every sample library in the world puts them.
pub fn switch_base(regions: &[SfzRegion], count: usize) -> Option<u8> {
    if count < 2 {
        return None;
    }
    let lowest = regions.iter().map(|r| r.lo_key).min()?;
    // The map is usually stretched to the bottom of the keyboard, so there is
    // no gap to put them in: then they take the bottom keys and shadow them.
    // Nothing was recorded down there — what is on those keys is the lowest
    // sample transposed into a growl — and a switch you cannot reach is worse
    // than three keys you would not play.
    Some(lowest.saturating_sub(count as u8))
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
    // **One recording with a pitch is an instrument.** There is no octave of
    // roots to span and no second file to compare it with — and what anyone
    // hands a sampler a single melodic sample for is to play it across the
    // keyboard. A single hit with no pitch is still a kit of one.
    if samples.len() == 1 {
        return true;
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

/// What a drum is called, and which General MIDI key that is.
///
/// Longest match wins, so `open hat` is not read as `hat` and `ride bell` is
/// not read as `ride`; the table is walked in order and the first hit stops
/// it. Written the way sample packs name files — `BD`, `SD`, `CHH`, `OHH` are
/// as common as the words.
///
/// ponytail: `contains` on the stem, not a tokeniser. `kick_01.wav`,
/// `01-Kick.wav` and `Kick Drum Hard.wav` all say kick, and none of them needs
/// a grammar to say it.
const GM_NAMES: &[(&[&str], u8)] = &[
    (&["pedal hat", "pedalhat", "foot hat", "phh"], 44),
    (&["open hat", "openhat", "hat open", "ohh"], 46),
    (&["closed hat", "closedhat", "hat closed", "chh"], 42),
    (&["hihat", "hi-hat", "hi hat", "hat", "hh"], 42),
    (&["side stick", "sidestick", "rimshot", "rim"], 37),
    (&["bass drum", "bassdrum", "kick", "bd"], 36),
    (&["clap"], 39),
    (&["snare", "sd"], 38),
    (&["ride bell", "ridebell"], 53),
    (&["ride"], 51),
    (&["crash"], 49),
    (&["china"], 52),
    (&["splash"], 55),
    (&["floor tom", "floortom"], 43),
    (&["tom"], 45),
    (&["cowbell"], 56),
    (&["tambourine", "tamb"], 54),
    (&["shaker", "cabasa"], 69),
    (&["maraca"], 70),
    (&["conga"], 63),
    (&["bongo"], 60),
    (&["timbale"], 65),
    (&["agogo"], 67),
    (&["clave"], 75),
    (&["woodblock", "wood block"], 76),
    (&["guiro"], 73),
    (&["triangle"], 81),
    (&["whistle"], 71),
    (&["vibraslap"], 58),
    (&["cuica"], 78),
];

/// The GM key a file's name asks for, if it asks for one.
fn gm_key(sample: &Sample) -> Option<u8> {
    let stem = sample
        .path
        .file_stem()?
        .to_string_lossy()
        .to_ascii_lowercase()
        // `kick-01`, `kick_01` and `kick 01` are the same name; so is
        // `01.kick`.
        .replace(['_', '-', '.'], " ");
    GM_NAMES
        .iter()
        .find(|(names, _)| names.iter().any(|n| stem.contains(n)))
        .map(|(_, key)| *key)
}

/// One file per key, played at the pitch it was recorded at: **the General MIDI
/// key its name asks for**, and the first free key for whatever the table does
/// not recognise.
///
/// A kit whose kick is not on 36 is a kit that plays wrong under every drum
/// pattern written for one — choz's own arranger included, which speaks GM and
/// nothing else. Alphabetical order put `hat` `kick` `snare` on 36, 37, 38,
/// which is three wrong sounds rather than none.
///
/// Two files asking for the same key —`kick_hard`, `kick_soft`— is a velocity
/// layer the name did not spell out, and this is not the place to guess: the
/// first one alphabetically keeps the GM key and the rest take free keys, so
/// both are playable and neither is lost.
fn drum_map(samples: &[&Sample]) -> Vec<SfzRegion> {
    let mut sorted: Vec<&&Sample> = samples.iter().collect();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));
    // From C2, where every hardware sampler and every GM kit puts its kick —
    // unless there are more sounds than there are keys above it, and then from
    // the bottom of the keyboard, because a sound with no key is a sound that
    // cannot be played at all. Philharmonia's whole `percussion.zip` read as
    // one kit is 148 of them; this is what keeps 128 rather than 92.
    let crowded = sorted.len() > (128 - DRUM_BASE as usize);
    let base = match crowded {
        true => 0,
        false => DRUM_BASE,
    };
    let mut taken = [false; 128];
    // The named ones first, so an unnamed file cannot sit on the key a kick
    // was going to ask for.
    let mut keys: Vec<Option<u8>> = sorted
        .iter()
        .map(|s| match crowded {
            // No room to be picky: a hundred and fifty sounds do not fit on a
            // GM kit, and every one of them still wants a key.
            true => None,
            false => match gm_key(s) {
                // First come keeps the GM key; the second `kick` takes a free
                // one below.
                Some(k) if !taken[k as usize] => {
                    taken[k as usize] = true;
                    Some(k)
                }
                _ => None,
            },
        })
        .collect();
    let mut free = base as usize;
    for key in keys.iter_mut().filter(|k| k.is_none()) {
        while taken.get(free) == Some(&true) {
            free += 1;
        }
        // Past the top of the keyboard: what is left has nowhere to go, and a
        // region on no key is worse than no region.
        if free > 127 {
            break;
        }
        taken[free] = true;
        *key = Some(free as u8);
    }
    sorted
        .iter()
        .zip(keys)
        .filter_map(|(s, key)| {
            let key = key?;
            Some(SfzRegion {
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
                articulation: 0,
            })
        })
        .collect()
}

/// One sample cut **where it is hit**, one piece per key from [`DRUM_BASE`] up.
///
/// The longest file in the folder, because that is the break: a folder that
/// also holds one-shots would otherwise get sliced on whichever came first.
/// Each piece plays at the speed it was recorded — a slice transposed is a
/// different drum, same as a kit.
///
/// The cuts come from [`super::slice_points`], which listens for the onsets;
/// As many equal pieces as were asked for are what is left when it hears nothing it
/// can call a hit, and they are what a break played to a grid wants anyway.
fn slice_map(samples: &[Sample], slices: usize) -> Vec<SfzRegion> {
    let Some(sample) = samples.iter().max_by_key(|s| s.bytes) else {
        return Vec::new();
    };
    let cuts = super::slice_points(&sample.path, slices);
    slice_regions(&sample.path, &cuts, slices)
}

/// The regions for one sliced file, given the cuts. Split out from
/// [`slice_map`] so a test can hand it the cuts rather than a file to decode.
fn slice_regions(path: &std::path::Path, cuts: &[f32], slices: usize) -> Vec<SfzRegion> {
    let n = slices.max(1);
    let equal: Vec<f32> = (0..n).map(|i| i as f32 / n as f32).collect();
    let cuts = match cuts.len() >= 2 {
        true => cuts,
        false => &equal,
    };
    cuts.iter()
        .copied()
        .enumerate()
        .filter_map(|(i, start)| {
            let key = DRUM_BASE.checked_add(u8::try_from(i).ok()?)?;
            // The last piece runs to the end of the file: whatever is after
            // the final hit is its tail.
            let end = cuts.get(i + 1).copied().unwrap_or(1.0);
            (key <= 127 && end > start).then(|| SfzRegion {
                sample: path.to_path_buf(),
                lo_key: key,
                hi_key: key,
                pitch_key_center: key,
                lo_vel: 0,
                hi_vel: 127,
                gain: 1.0,
                tune_cents: 0.0,
                start,
                end,
                articulation: 0,
            })
        })
        .collect()
}

fn pitched_map(samples: &[&Sample]) -> Vec<SfzRegion> {
    let samples = &dense_layers(samples);
    let layers = velocity_layers(samples);
    // Where the instrument itself was recorded, whichever layer recorded it.
    let (low, high) = samples
        .iter()
        .filter_map(|s| s.root)
        .fold((u8::MAX, 0u8), |(lo, hi), r| (lo.min(r), hi.max(r)));
    let mut out: Vec<Vec<SfzRegion>> = Vec::new();
    for (velocity, lo_vel, hi_vel) in &layers {
        let in_layer: Vec<&&Sample> = samples
            .iter()
            .filter(|s| s.velocity.unwrap_or(*velocity) == *velocity)
            .collect();
        let mut roots: Vec<u8> = in_layer.iter().filter_map(|s| s.root).collect();
        roots.sort_unstable();
        roots.dedup();
        let mut layer = Vec::new();
        for sample in &in_layer {
            let Some(root) = sample.root else { continue };
            let (mut lo_key, mut hi_key) = key_zone(root, &roots);
            // **A layer stretches past its own last note only where the
            // instrument ends.** Oboe fortissimo stops at E6 while every other
            // dynamic goes on to G6: stretched to the top, a hard F6 or G6
            // played the E6 sample transposed, and E6 itself changed colour
            // with how hard the sweep happened to hit it. Those keys are
            // filled from a layer that recorded them — see below.
            if lo_key == 0 && root > low {
                lo_key = root;
            }
            if hi_key == 127 && root < high {
                hi_key = root;
            }
            layer.push(SfzRegion {
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
                articulation: 0,
            });
        }
        out.push(layer);
    }
    // **Every key plays a recorded note when any layer has one.** Per layer,
    // per key: the layer's own region when its note is within the layer's
    // `reach`; otherwise the nearest layer — in velocity — that recorded the
    // key within that reach; otherwise the layer's own stretch, or, for a key it
    // does not reach at all, the nearest layer that does. A soft trombone
    // missing four notes in its low octave played them six semitones
    // transposed while every other dynamic had them, so a chromatic sweep
    // changed colour on those keys with how hard each one was hit.
    let distance = |r: &SfzRegion, key: usize| (key as i32 - r.pitch_key_center as i32).unsigned_abs();
    let covering = |layer: &[SfzRegion], key: usize| -> Option<usize> {
        layer
            .iter()
            .enumerate()
            .filter(|(_, r)| (r.lo_key as usize..=r.hi_key as usize).contains(&key))
            .min_by_key(|(_, r)| distance(r, key))
            .map(|(k, _)| k)
    };
    // How far a layer's own stretch is its normal: half the gap between its
    // recorded notes. A chromatic layer plays its own note or one a semitone
    // off; a pack recorded every minor third reaches a semitone either way on
    // its own and borrows nothing; one recorded every fifth reaches three.
    // Only a key further than that is a hole.
    let reach: Vec<u32> = out
        .iter()
        .map(|layer| {
            let mut roots: Vec<u8> = layer.iter().map(|r| r.pitch_key_center).collect();
            roots.sort_unstable();
            roots.dedup();
            let mut gaps: Vec<u32> = roots.windows(2).map(|w| (w[1] - w[0]) as u32).collect();
            gaps.sort_unstable();
            gaps.get(gaps.len() / 2).map(|g| g.div_ceil(2)).unwrap_or(1).max(1)
        })
        .collect();
    let mut regions = Vec::new();
    for (i, (_, lo_vel, hi_vel)) in layers.iter().enumerate() {
        let mut by_velocity: Vec<usize> = (0..out.len()).filter(|j| *j != i).collect();
        by_velocity.sort_by_key(|j| j.abs_diff(i));
        // Which layer's region plays each key, as (layer, region).
        let chosen: Vec<Option<(usize, usize)>> = (0..128usize)
            .map(|key| {
                let own = covering(&out[i], key).map(|k| (i, k));
                if own.is_some_and(|(_, k)| distance(&out[i][k], key) <= reach[i]) {
                    return own;
                }
                let recorded = by_velocity.iter().find_map(|&j| {
                    covering(&out[j], key)
                        .filter(|k| distance(&out[j][*k], key) <= reach[i])
                        .map(|k| (j, k))
                });
                recorded
                    .or(own)
                    .or_else(|| by_velocity.iter().find_map(|&j| covering(&out[j], key).map(|k| (j, k))))
            })
            .collect();
        // Runs of keys played by the same note of the same layer become one
        // region per take of that note: takes are round robins, and choosing
        // "a region" must not choose one take and drop the others.
        let note_of = |c: Option<(usize, usize)>| c.map(|(j, k)| (j, out[j][k].pitch_key_center));
        let mut key = 0usize;
        while key < 128 {
            let Some((j, root)) = note_of(chosen[key]) else {
                key += 1;
                continue;
            };
            let from = key;
            while key < 128 && note_of(chosen[key]) == Some((j, root)) {
                key += 1;
            }
            for take in out[j].iter().filter(|r| r.pitch_key_center == root) {
                regions.push(SfzRegion {
                    lo_key: from as u8,
                    hi_key: (key - 1) as u8,
                    lo_vel: *lo_vel,
                    hi_vel: *hi_vel,
                    ..take.clone()
                });
            }
        }
    }
    regions
}

/// The samples of the velocity layers that can carry the keyboard.
///
/// **A layer with a handful of notes is not a layer.** Philharmonia's flute has
/// 42 forte notes and 2 fortissimo ones; as a layer of its own the fortissimo
/// took every note played at 105 and up, and played those two samples
/// stretched across all 88 keys — the bottom octaves three octaves down, the
/// level off the top of the scale. So a layer needs a third of the notes of
/// the widest one, the rule [`articulation_groups`] uses, and a thinner one is
/// left out: its velocities go to the layers beside it, which have a note near
/// every key.
///
/// ponytail: left out whole. Keeping a sparse layer on the keys near its own
/// roots and falling back elsewhere is the finer answer, and wants a zone per
/// key per layer rather than per root.
fn dense_layers<'a>(samples: &[&'a Sample]) -> Vec<&'a Sample> {
    let notes = |velocity: u8| -> usize {
        let mut roots: Vec<u8> = samples
            .iter()
            .filter(|s| s.velocity == Some(velocity))
            .filter_map(|s| s.root)
            .collect();
        roots.sort_unstable();
        roots.dedup();
        roots.len()
    };
    let widest = samples
        .iter()
        .filter_map(|s| s.velocity)
        .map(notes)
        .max()
        .unwrap_or(0);
    samples
        .iter()
        .copied()
        .filter(|s| s.velocity.is_none_or(|v| notes(v) * 3 >= widest))
        .collect()
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

    /// A layer that stops short of the instrument's top note does not stretch
    /// its last note over the keys another layer recorded: those keys play the
    /// nearest layer that has them, at this layer's velocities.
    #[test]
    fn a_layer_that_ends_early_borrows_the_keys_above_it() {
        let mut samples: Vec<Sample> = (84..=91)
            .map(|n| sample(&format!("o_{n}_forte.wav"), Some(n), Some(96)))
            .collect();
        samples.extend((84..=88).map(|n| sample(&format!("o_{n}_ff.wav"), Some(n), Some(112))));
        let regions = regions(&samples);
        let at = |key: u8, vel: u8| {
            let hits: Vec<&SfzRegion> = regions
                .iter()
                .filter(|r| (r.lo_key..=r.hi_key).contains(&key) && (r.lo_vel..=r.hi_vel).contains(&vel))
                .collect();
            assert_eq!(hits.len(), 1, "key {key} vel {vel}: {hits:?}");
            (hits[0].pitch_key_center, hits[0].sample.to_string_lossy().into_owned())
        };
        // E6 hard is its own fortissimo, and nothing else.
        assert_eq!(at(88, 120), (88, "o_88_ff.wav".into()));
        // F6 and G6 hard are recorded notes, from the forte layer — not E6
        // fortissimo transposed.
        assert_eq!(at(89, 120), (89, "o_89_forte.wav".into()));
        assert_eq!(at(91, 120), (91, "o_91_forte.wav".into()));
        // Past the instrument's own top, the top note still stretches.
        assert_eq!(at(100, 120).0, 91);
        // And every key answers at every velocity.
        for key in 0..=127u8 {
            for vel in [1u8, 100, 127] {
                at(key, vel);
            }
        }
    }

    /// A pack recorded every four semitones stretches each layer its usual
    /// two either way, and does not trade its dynamic for another layer's
    /// note just because that layer was recorded at different pitches.
    #[test]
    fn a_sparse_pack_keeps_its_own_dynamic_within_its_reach() {
        let mut samples: Vec<Sample> = [48u8, 52, 56, 60]
            .iter()
            .map(|n| sample(&format!("s_{n}_forte.wav"), Some(*n), Some(96)))
            .collect();
        samples.extend(
            [50u8, 54, 58]
                .iter()
                .map(|n| sample(&format!("s_{n}_piano.wav"), Some(*n), Some(48))),
        );
        let regions = regions(&samples);
        let at = |key: u8, vel: u8| {
            let hits: Vec<&SfzRegion> = regions
                .iter()
                .filter(|r| (r.lo_key..=r.hi_key).contains(&key) && (r.lo_vel..=r.hi_vel).contains(&vel))
                .collect();
            assert_eq!(hits.len(), 1, "key {key} vel {vel}: {hits:?}");
            hits[0].sample.to_string_lossy().into_owned()
        };
        // Loud at 50: two semitones from the forte 48 or 52 — its own layer.
        assert!(at(50, 120).contains("forte"), "{}", at(50, 120));
        assert!(at(56, 20).contains("piano"), "{}", at(56, 20));
    }

    /// A hole in the middle of a layer is filled from a layer that recorded
    /// the note, not by transposing the layer's neighbour three semitones.
    #[test]
    fn a_hole_in_a_layer_plays_the_note_another_layer_recorded() {
        let mut samples: Vec<Sample> = (40..=50)
            .map(|n| sample(&format!("t_{n}_forte.wav"), Some(n), Some(96)))
            .collect();
        samples.extend(
            [40u8, 41, 42, 48, 49, 50]
                .iter()
                .map(|n| sample(&format!("t_{n}_piano.wav"), Some(*n), Some(48))),
        );
        let regions = regions(&samples);
        let at = |key: u8, vel: u8| {
            let hits: Vec<&SfzRegion> = regions
                .iter()
                .filter(|r| (r.lo_key..=r.hi_key).contains(&key) && (r.lo_vel..=r.hi_vel).contains(&vel))
                .collect();
            assert_eq!(hits.len(), 1, "key {key} vel {vel}: {hits:?}");
            hits[0].pitch_key_center
        };
        // The soft layer's own notes stay its own, and so does a semitone off.
        assert_eq!(at(40, 20), 40);
        assert_eq!(at(43, 20), 42);
        // Deep in the hole it plays the recorded note.
        assert_eq!(at(45, 20), 45);
        assert_eq!(at(46, 20), 46);
    }

    /// A velocity layer with two notes in it does not take the top of the
    /// velocity range and stretch those two across the keyboard: its range
    /// goes to the full layer beside it.
    #[test]
    fn a_thin_velocity_layer_gives_its_range_to_the_full_one() {
        let mut samples: Vec<Sample> = (60..72)
            .map(|n| sample(&format!("f_{n}_forte.wav"), Some(n), Some(96)))
            .collect();
        samples.push(sample("f_84_ff.wav", Some(84), Some(112)));
        samples.push(sample("f_86_ff.wav", Some(86), Some(112)));
        let regions = regions(&samples);
        assert!(
            regions.iter().all(|r| !r.sample.to_string_lossy().contains("ff")),
            "the two-note layer is still in the map"
        );
        assert!(
            regions.iter().all(|r| (r.lo_vel, r.hi_vel) == (0, 127)),
            "the full layer does not take every velocity"
        );
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
        // The sustain opens the instrument — articulation 0 — and the staccato
        // is in the map behind a keyswitch rather than dropped.
        let opening: Vec<&SfzRegion> = regions.iter().filter(|r| r.articulation == 0).collect();
        assert_eq!(opening.len(), 2);
        assert!(opening
            .iter()
            .all(|r| r.sample.to_string_lossy().contains("sustain")));
        assert!(
            regions
                .iter()
                .any(|r| r.articulation == 1 && r.sample.to_string_lossy().contains("staccato")),
            "the staccato was dropped instead of switched to"
        );
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
                .filter(|r| r.articulation == 0)
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
        // The normal set opens it; the tremolo is behind a keyswitch.
        assert!(regions
            .iter()
            .filter(|r| r.articulation == 0)
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
        let opening: Vec<&SfzRegion> = regions.iter().filter(|r| r.articulation == 0).collect();
        assert_eq!(opening.len(), 2, "the short set opens the instrument");
        assert!(opening
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

        // A folder where nothing has a pitch — a cabasa — still stretches when
        // asked: every key plays, at its recorded speed on middle C.
        let shaker = [hit("cabasa_1.wav", None), hit("cabasa_2.wav", None)];
        assert!(regions(&shaker).iter().all(|r| r.lo_key == r.hi_key), "AUTO is a kit");
        let spread = regions_with(&shaker, Mode::Stretch);
        assert!(!spread.is_empty());
        for key in 0..=127u8 {
            assert!(
                spread.iter().any(|r| (r.lo_key..=r.hi_key).contains(&key)),
                "key {key} is silent under STRETCH"
            );
        }
        assert!(spread.iter().all(|r| r.pitch_key_center == 60));

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
    /// across the keyboard. **And the keys are the General MIDI ones**: a kick
    /// that is not on 36 plays wrong under every pattern written for a kit.
    #[test]
    fn a_folder_with_no_pitches_becomes_a_drum_map() {
        let regions = regions(&[
            sample("hat.wav", None, None),
            sample("kick.wav", None, None),
            sample("snare.wav", None, None),
        ]);
        assert_eq!(regions.len(), 3);
        // Alphabetical in the list, GM on the keyboard.
        assert_eq!(regions[0].sample.to_string_lossy(), "hat.wav");
        assert_eq!((regions[0].lo_key, regions[0].hi_key), (42, 42));
        assert_eq!(regions[0].pitch_key_center, 42);
        assert_eq!(regions[1].lo_key, 36);
        assert_eq!(regions[2].lo_key, 38);
    }

    /// The names a pack actually uses, and the ones that would be read as the
    /// wrong drum by a shorter match.
    #[test]
    fn the_names_a_pack_uses_land_on_their_gm_keys() {
        for (file, key) in [
            ("01-Kick.wav", 36),
            ("BD_hard.wav", 36),
            ("Snare Drum.wav", 38),
            ("CHH.wav", 42),
            ("hihat_closed_02.wav", 42),
            // The longer name wins: `open hat` is not a hat, and `ride bell`
            // is not a ride.
            ("OHH.wav", 46),
            ("hat-open.wav", 46),
            ("ride_bell.wav", 53),
            ("Ride.wav", 51),
            ("Floor Tom.wav", 43),
            ("tom3.wav", 45),
            ("crash_cymbal.wav", 49),
            ("rimshot.wav", 37),
            ("clap 01.wav", 39),
            ("cowbell.wav", 56),
        ] {
            let regions = regions(&[sample(file, None, None)]);
            assert_eq!(regions[0].lo_key, key, "{file}");
        }
        // Nothing the table knows: the first free key from the drum base up.
        let regions = regions(&[sample("blorp.wav", None, None)]);
        assert_eq!(regions[0].lo_key, DRUM_BASE);
    }

    /// Two kicks are a velocity layer nobody spelled out. The first keeps 36
    /// and the second gets a key of its own — both playable, neither lost.
    #[test]
    fn two_files_asking_for_one_key_both_get_played() {
        let regions = regions(&[
            sample("kick_hard.wav", None, None),
            sample("kick_soft.wav", None, None),
        ]);
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[0].lo_key, 36);
        assert_ne!(regions[1].lo_key, 36);
        assert_eq!(regions[1].sample.to_string_lossy(), "kick_soft.wav");
    }

    /// More sounds than there are keys: GM goes out of the window, because a
    /// hundred and fifty of them do not fit on a kit and a sound with no key
    /// cannot be played at all.
    #[test]
    fn a_crowded_folder_falls_back_to_the_whole_keyboard() {
        let many: Vec<Sample> = (0..140)
            .map(|i| sample(&format!("hit{i:03}.wav"), None, None))
            .collect();
        let regions = regions(&many);
        assert_eq!(regions.len(), 128);
        assert_eq!(regions[0].lo_key, 0);
        assert_eq!(regions[127].lo_key, 127);
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

    /// And when the detector *did* hear the hits, the pieces are where they
    /// are: a slice per onset, joined end to end, the last one running out to
    /// the end of the file.
    #[test]
    fn slice_cuts_on_the_onsets_when_there_are_any() {
        let cuts = [0.0, 0.17, 0.41, 0.66, 0.83];
        let regions = slice_regions(Path::new("break.wav"), &cuts, super::super::SLICES);
        assert_eq!(regions.len(), cuts.len());
        for (i, r) in regions.iter().enumerate() {
            assert_eq!(r.lo_key, DRUM_BASE + i as u8);
            assert!((r.start - cuts[i]).abs() < 1e-6);
            let end = cuts.get(i + 1).copied().unwrap_or(1.0);
            assert!((r.end - end).abs() < 1e-6);
        }
        // One cut is not a slicing: back to equal pieces, which is what a
        // break played to a grid wanted anyway.
        assert_eq!(
            slice_regions(Path::new("break.wav"), &[0.0], super::super::SLICES).len(),
            super::super::SLICES
        );
        // And how many equal pieces is whatever was asked for: the count is a
        // number the id carries, not a constant any more.
        assert_eq!(
            slice_regions(Path::new("break.wav"), &[0.0], 24).len(),
            24
        );
    }
}
