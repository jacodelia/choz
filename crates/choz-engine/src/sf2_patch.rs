//! The editable half of a SoundFont, in one table.
//!
//! A SoundFont's sound is a pile of *generators*: signed numbers in timecents,
//! centibels and absolute cents. oxisynth takes an **offset** on any of them at
//! run time (`set_gen`), which is how a SoundFont player changes a patch
//! without touching the file — and that is what choz's SF2 slot exposes as
//! knobs.
//!
//! One table, three readers: [`crate::sources::sf2_params`] builds the slot's
//! parameter list from it, `Sf2Synth::set_param` turns a knob position into the
//! offset, and the interface learns CCs to the same indices. A second copy of
//! any of that is a second answer to what `Attack` means.

/// One editable generator of an SF2 slot.
///
/// The SoundFont itself is never written: every one of these rides on top of
/// what the file says, so a project stores eleven numbers rather than a copy of
/// the instrument.
///
/// **The spans are what makes these audible.** A generator offset is added to
/// what the instrument already asks for, so a span that is small next to a real
/// patch's own value is a knob that moves and changes nothing.
pub struct Edit {
    pub name: &'static str,
    /// Display unit, and the thing that decides the control: the run of five
    /// sharing one unit is what the RACK draws as a bank of vertical faders.
    /// See `views::fx_chain_panel::fader_groups`.
    pub unit: Option<&'static str>,
    pub group: &'static str,
    /// SF2 generator number (specification 8.1.3), which oxisynth numbers the
    /// same way.
    pub gen: u16,
    /// What the far end of the knob is worth, in that generator's own units.
    /// The middle is neutral: `offset = (v - 0.5) * 2 * span`.
    pub span: f32,
}

/// The parameters an SF2 slot offers past its two sends.
///
/// The order is fixed and only ever appended to: a project stores these by
/// position, so re-ordering them would re-point every saved edit.
pub const EDITS: &[Edit] = &[
    // The amplitude envelope, in timecents. **The whole range the format
    // allows**, and not a polite slice of it: these are offsets on top of what
    // the file already says, and a sampled piano says 8 ms of attack and 900 ms
    // of release. A span of ±2400 (four times either way) moved that to 32 ms
    // and 3.6 s — which is a piano with a slightly soft edge, not the pad
    // somebody was trying to make. ±8000 is what oxisynth clamps to, so it is
    // as far as the knob can usefully go, and it reaches a two-second swell
    // from a piano.
    edit("Attack", Some("%"), "AMP ENV", 34, 8000.0),
    edit("Hold", Some("%"), "AMP ENV", 35, 8000.0),
    edit("Decay", Some("%"), "AMP ENV", 36, 8000.0),
    // Centibels of attenuation at the sustain, over the format's full 0–1440:
    // a piano sustains at 1000 (it dies away), and anything less than the full
    // range could not bring it back up to a held note. **Negative**, like
    // `Volume` below and for the same reason — the generator is attenuation, so
    // a knob marked `Sustain` has to *remove* it as it goes up.
    edit("Sustain", Some("%"), "AMP ENV", 37, -1440.0),
    edit("Release", Some("%"), "AMP ENV", 38, 8000.0),
    // Absolute cents: ±4 octaves of cutoff, and ±24 dB of resonance.
    edit("Cutoff", None, "FILTER", 8, 4800.0),
    edit("Reso", None, "FILTER", 9, 240.0),
    edit("Coarse", None, "PITCH", 51, 24.0),
    edit("Fine", Some("cents"), "PITCH", 52, 100.0),
    // Tenths of a percent left/right.
    edit("Pan", None, "OUT", 17, 500.0),
    // **Negative on purpose.** The generator is *attenuation* in centibels, so
    // more of it is quieter — and a knob labelled `Volume` that turns the sound
    // down as it goes up is a knob nobody can use. The span is what the top of
    // the travel is worth, and here that is 24 dB of attenuation removed.
    edit("Volume", None, "OUT", 48, -240.0),
];

const fn edit(
    name: &'static str,
    unit: Option<&'static str>,
    group: &'static str,
    gen: u16,
    span: f32,
) -> Edit {
    Edit {
        name,
        unit,
        group,
        gen,
        span,
    }
}

/// Parameters before the first editable generator: the two sends, which were
/// there first and are switches rather than knobs.
pub const SENDS: usize = 2;

/// Neutral position of every editable generator — the middle of its travel,
/// where the offset is zero and the SoundFont plays as written.
pub const NEUTRAL: f32 = 0.5;

/// The generator and offset a slot parameter stands for, or `None` for the two
/// sends and for anything past the end of the list.
pub fn offset_of(param: usize, value: f32) -> Option<(u16, f32)> {
    let e = EDITS.get(param.checked_sub(SENDS)?)?;
    Some((e.gen, (value.clamp(0.0, 1.0) - NEUTRAL) * 2.0 * e.span))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every editable generator has to be neutral in the middle, distinct from
    /// its neighbours, and signed the way the label reads. A table where two
    /// entries share a generator number would silently move one control with
    /// another, and one whose middle is not zero would change the sound of a
    /// SoundFont the moment it loaded.
    #[test]
    fn the_editable_generators_are_neutral_in_the_middle_and_all_different() {
        let mut seen = std::collections::HashSet::new();
        for (i, e) in EDITS.iter().enumerate() {
            let param = SENDS + i;
            assert!(seen.insert(e.gen), "{} repeats generator {}", e.name, e.gen);
            assert_eq!(
                offset_of(param, NEUTRAL),
                Some((e.gen, 0.0)),
                "{} is neutral in the middle",
                e.name
            );
            let (_, hi) = offset_of(param, 1.0).unwrap();
            let (_, lo) = offset_of(param, 0.0).unwrap();
            // Both ways from the middle, whichever way round the generator
            // reads: `Volume` is an *attenuation*, so its span is negative and
            // the top of its travel is the loud end.
            assert!(hi * lo < 0.0, "{} runs both ways", e.name);
            assert_eq!(hi, e.span, "{} reaches its span", e.name);
        }
        // The two sends are switches, not generators with a span.
        assert!(offset_of(0, 1.0).is_none() && offset_of(1, 1.0).is_none());
        assert!(offset_of(SENDS + EDITS.len(), 1.0).is_none());
    }
}
