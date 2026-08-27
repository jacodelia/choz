//! Factory presets for the built-in effects: a starting point per FX, by name.
//!
//! # Why they are here and not in the DSP
//!
//! A preset is a set of **knob positions**, and the knobs are this crate's
//! ([`fx_param_descs`]): the processors take normalised values in an order the
//! UI defines, and it is the UI's parameter array that the project saves and
//! that a rebuild reads. A preset table next to the DSP would be a second
//! description of that order, free to drift from it.
//!
//! # Why the values are keyed by parameter *name*
//!
//! An index is a position in a list that grows: the day a knob is inserted in
//! the middle, every preset below it silently means something else. A name is
//! what the user reads on the knob, and a preset that names a parameter which
//! no longer exists is caught by the test at the bottom of this file rather
//! than by somebody wondering why "Dub" sounds like a slapback.
//!
//! Anything a preset does not mention keeps whatever the knob had — presets are
//! a starting point, not a reset. Only the ones an effect actually needs are
//! listed: three good ones beat twelve that differ in the third decimal.
//!
//! Three effects already had a preset **knob** of their own (the graphic EQ's
//! Winamp list, AutoTune's five, and the saturator's curve): those stay where
//! they are, because they are automatable positions of a parameter, not a
//! separate menu.

use crate::source::{fx_param_descs, AudioFxEntry, AudioFxKind};

/// One factory preset: a name and the knobs it moves.
#[derive(Debug, Clone, Copy)]
pub struct FxPreset {
    pub name: &'static str,
    /// `(parameter name, normalised 0..1 position)`.
    pub values: &'static [(&'static str, f32)],
}

/// The factory presets for `kind`. Empty for anything without a set worth
/// shipping — a Gain knob does not need presets.
pub fn presets(kind: AudioFxKind) -> &'static [FxPreset] {
    use AudioFxKind::*;
    match kind {
        Delay => &[
            FxPreset {
                name: "Slapback",
                // 100 ms, barely any tail: the doubling on a rockabilly vocal.
                values: &[
                    ("Time", 0.091),
                    ("Feedback", 0.15),
                    ("Damping", 0.40),
                    ("PingPong", 0.0),
                    ("Wet", 0.35),
                ],
            },
            FxPreset {
                name: "Eighth Note",
                // 250 ms — an eighth at 120 BPM, which is where the transport is
                // until something moves it.
                values: &[
                    ("Time", 0.242),
                    ("Feedback", 0.35),
                    ("Damping", 0.30),
                    ("Wet", 0.35),
                ],
            },
            FxPreset {
                name: "Dub",
                values: &[
                    ("Time", 0.394),
                    ("Feedback", 0.75),
                    ("Damping", 0.60),
                    ("PingPong", 1.0),
                    ("Wet", 0.45),
                ],
            },
        ],
        // Each of these sets `Character` as well as the knobs: the character is
        // what decides the *balance* between reflections and tail, and a hall
        // built out of a room's balance is a room with a long tail.
        Reverb => &[
            FxPreset {
                name: "Studio Room",
                values: &[
                    ("Character", 0.00),
                    ("Size", 0.28),
                    ("Decay", 0.22),
                    ("PreDelay", 0.03),
                    ("Diffusion", 0.55),
                    ("Damping", 0.60),
                    ("Tone", 0.50),
                    ("Modulation", 0.15),
                    ("LowCut", 0.30),
                    ("HighCut", 0.70),
                    ("Wet", 0.22),
                ],
            },
            FxPreset {
                name: "Concert Hall",
                values: &[
                    ("Character", 0.25),
                    ("Size", 0.70),
                    ("Decay", 0.55),
                    ("PreDelay", 0.10),
                    ("Diffusion", 0.80),
                    ("Damping", 0.42),
                    ("Tone", 0.48),
                    ("Modulation", 0.30),
                    ("LowCut", 0.22),
                    ("HighCut", 0.72),
                    ("Wet", 0.38),
                ],
            },
            FxPreset {
                name: "Chamber",
                values: &[
                    ("Character", 0.50),
                    ("Size", 0.45),
                    ("Decay", 0.38),
                    ("PreDelay", 0.05),
                    ("Diffusion", 0.85),
                    ("Damping", 0.50),
                    ("Tone", 0.55),
                    ("Modulation", 0.20),
                    ("LowCut", 0.28),
                    ("HighCut", 0.78),
                    ("Wet", 0.30),
                ],
            },
            FxPreset {
                name: "Plate",
                values: &[
                    ("Character", 0.75),
                    ("Size", 0.40),
                    ("Decay", 0.42),
                    ("PreDelay", 0.02),
                    ("Diffusion", 0.95),
                    ("Damping", 0.25),
                    ("Tone", 0.68),
                    ("Modulation", 0.20),
                    ("LowCut", 0.35),
                    ("HighCut", 0.85),
                    ("Wet", 0.32),
                ],
            },
            FxPreset {
                name: "Cathedral",
                values: &[
                    ("Character", 0.25),
                    ("Size", 1.00),
                    ("Decay", 0.85),
                    ("PreDelay", 0.16),
                    ("Diffusion", 0.90),
                    ("Damping", 0.55),
                    ("Tone", 0.40),
                    ("Modulation", 0.35),
                    ("LowCut", 0.30),
                    ("HighCut", 0.55),
                    ("Wet", 0.45),
                ],
            },
            FxPreset {
                name: "Ambient Wash",
                values: &[
                    ("Character", 1.00),
                    ("Size", 0.85),
                    ("Decay", 0.80),
                    ("PreDelay", 0.40),
                    ("Diffusion", 1.00),
                    ("Damping", 0.35),
                    ("Tone", 0.45),
                    ("Modulation", 0.60),
                    ("LowCut", 0.40),
                    ("HighCut", 0.65),
                    ("Wet", 0.55),
                ],
            },
        ],
        Compressor => &[
            // Threshold is -(1-p)·60 dB and ratio 1+p·19, so these read as
            // -18 dB at 4:1, -12 at 6:1, and so on.
            FxPreset {
                name: "Vocal Glue",
                values: &[
                    ("Thresh", 0.70),
                    ("Ratio", 0.158),
                    ("Attack", 0.099),
                    ("Release", 0.141),
                    ("Makeup", 0.167),
                    ("Knee", 0.50),
                ],
            },
            FxPreset {
                name: "Drum Punch",
                // Slow attack on purpose: the transient goes through, the body
                // is what gets held down.
                values: &[
                    ("Thresh", 0.80),
                    ("Ratio", 0.263),
                    ("Attack", 0.249),
                    ("Release", 0.071),
                    ("Makeup", 0.125),
                    ("Knee", 0.167),
                ],
            },
            FxPreset {
                name: "Bus Glue",
                values: &[
                    ("Thresh", 0.60),
                    ("Ratio", 0.053),
                    ("Attack", 0.299),
                    ("Release", 0.293),
                    ("Makeup", 0.083),
                    ("Knee", 0.667),
                ],
            },
            FxPreset {
                name: "Squash",
                values: &[
                    ("Thresh", 0.50),
                    ("Ratio", 0.579),
                    ("Attack", 0.009),
                    ("Release", 0.051),
                    ("Makeup", 0.333),
                    ("Knee", 0.0),
                ],
            },
        ],
        Limiter => &[
            FxPreset {
                name: "Transparent",
                values: &[("Thresh", 0.917), ("Release", 0.246)],
            },
            FxPreset {
                name: "Loud",
                values: &[("Thresh", 0.75), ("Release", 0.096)],
            },
            FxPreset {
                name: "Brickwall",
                values: &[("Thresh", 0.975), ("Release", 0.020)],
            },
        ],
        Gate => &[
            FxPreset {
                name: "Noise Gate",
                values: &[
                    ("Thresh", 0.4375),
                    ("Attack", 0.018),
                    ("Hold", 0.098),
                    ("Release", 0.091),
                    ("Floor", 0.0),
                ],
            },
            FxPreset {
                name: "Drum Gate",
                values: &[
                    ("Thresh", 0.6875),
                    ("Attack", 0.004),
                    ("Hold", 0.038),
                    ("Release", 0.051),
                    ("Floor", 0.0),
                ],
            },
            FxPreset {
                name: "Gentle",
                // A floor of -20 dB rather than silence: what is under the
                // threshold is ducked, not cut, which is what keeps a room from
                // breathing on and off behind a voice.
                values: &[
                    ("Thresh", 0.3125),
                    ("Attack", 0.098),
                    ("Hold", 0.238),
                    ("Release", 0.242),
                    ("Floor", 0.75),
                ],
            },
        ],
        Filter => &[
            FxPreset {
                name: "Warm Lowpass",
                values: &[("Cutoff", 0.059), ("Res", 0.20)],
            },
            FxPreset {
                name: "Telephone",
                values: &[("Cutoff", 0.124), ("Res", 0.50)],
            },
            FxPreset {
                name: "Dub Sweep",
                values: &[("Cutoff", 0.029), ("Res", 0.75)],
            },
        ],
        ParamEq => &[
            FxPreset {
                name: "Air",
                values: &[("High", 0.65), ("HiFreq", 0.85)],
            },
            FxPreset {
                name: "Warmth",
                values: &[("Low", 0.60), ("LowFreq", 0.25)],
            },
            FxPreset {
                name: "Presence",
                values: &[("HiMid", 0.65), ("MidQ", 0.35)],
            },
            FxPreset {
                name: "Mud Cut",
                values: &[("Low", 0.38), ("LowMid", 0.40)],
            },
        ],
        Saturator => &[
            // `Curve` is a named position: eight curves, so index/7. Writing the
            // number rather than the name is the one place this table has to
            // know how the picker is laid out.
            FxPreset {
                name: "Tape Warmth",
                values: &[
                    ("Drive", 0.35),
                    ("Curve", 0.4286),
                    ("Tone", 0.75),
                    ("Output", 0.50),
                    ("Oversamp", 0.3333),
                ],
            },
            FxPreset {
                name: "Tube Drive",
                values: &[
                    ("Drive", 0.55),
                    ("Curve", 0.2857),
                    ("Bias", 0.58),
                    ("Tone", 0.60),
                    ("Output", 0.45),
                    ("Oversamp", 0.6667),
                ],
            },
            FxPreset {
                name: "Fuzz Fold",
                values: &[
                    ("Drive", 0.80),
                    ("Curve", 0.5714),
                    ("Tone", 0.50),
                    ("Output", 0.35),
                    ("Oversamp", 1.0),
                ],
            },
            FxPreset {
                name: "Clean Glue",
                values: &[
                    ("Drive", 0.15),
                    ("Curve", 0.0),
                    ("Tone", 1.0),
                    ("Output", 0.50),
                    ("Oversamp", 0.3333),
                ],
            },
        ],
        TubeSat => &[
            FxPreset {
                name: "Warm",
                values: &[("Drive", 0.20), ("Tone", 0.35), ("Wet", 0.50)],
            },
            FxPreset {
                name: "Crunch",
                values: &[("Drive", 0.60), ("Tone", 0.50), ("Wet", 0.80)],
            },
            FxPreset {
                name: "Overdrive",
                values: &[("Drive", 0.85), ("Tone", 0.60), ("Wet", 1.0)],
            },
        ],
        BitCrusher => &[
            FxPreset {
                name: "Lo-Fi",
                values: &[("Bits", 0.50), ("Rate", 0.50)],
            },
            FxPreset {
                name: "Telephone",
                values: &[("Bits", 0.60), ("Rate", 0.25)],
            },
            FxPreset {
                name: "Destroy",
                values: &[("Bits", 0.15), ("Rate", 0.12)],
            },
        ],
        Chorus => &[
            FxPreset {
                name: "Subtle",
                values: &[("Rate", 0.12), ("Depth", 0.18), ("Wet", 0.30)],
            },
            FxPreset {
                name: "Lush",
                values: &[("Rate", 0.25), ("Depth", 0.45), ("Wet", 0.55)],
            },
            FxPreset {
                name: "Detune",
                values: &[("Rate", 0.05), ("Depth", 0.12), ("Wet", 0.50)],
            },
        ],
        Flanger => &[
            FxPreset {
                name: "Jet",
                values: &[("Rate", 0.20), ("Depth", 0.60), ("Feedback", 0.80)],
            },
            FxPreset {
                name: "Slow Sweep",
                values: &[("Rate", 0.05), ("Depth", 0.50), ("Feedback", 0.60)],
            },
            FxPreset {
                name: "Metallic",
                values: &[("Rate", 0.35), ("Depth", 0.25), ("Feedback", 0.90)],
            },
        ],
        Phaser => &[
            FxPreset {
                name: "Vintage",
                values: &[
                    ("Rate", 0.15),
                    ("Depth", 0.60),
                    ("Center", 0.35),
                    ("Feedback", 0.50),
                ],
            },
            FxPreset {
                name: "Slow Sweep",
                values: &[
                    ("Rate", 0.08),
                    ("Depth", 0.90),
                    ("Center", 0.50),
                    ("Feedback", 0.70),
                ],
            },
            FxPreset {
                name: "Fast",
                values: &[
                    ("Rate", 0.45),
                    ("Depth", 0.50),
                    ("Center", 0.40),
                    ("Feedback", 0.40),
                ],
            },
        ],
        GranDelay => &[
            FxPreset {
                name: "Shimmer",
                values: &[
                    ("Size", 0.35),
                    ("Density", 0.60),
                    ("Pitch", 0.75),
                    ("Feedback", 0.40),
                    ("Wet", 0.50),
                ],
            },
            FxPreset {
                name: "Octave Down",
                values: &[
                    ("Size", 0.45),
                    ("Density", 0.50),
                    ("Pitch", 0.25),
                    ("Feedback", 0.35),
                    ("Wet", 0.50),
                ],
            },
            FxPreset {
                name: "Cloud",
                values: &[
                    ("Size", 0.70),
                    ("Density", 0.85),
                    ("Pitch", 0.50),
                    ("Feedback", 0.60),
                    ("Wet", 0.70),
                ],
            },
        ],
        SpaceEcho => &[
            FxPreset {
                name: "Tape Echo",
                values: &[
                    ("Time", 0.30),
                    ("Feedback", 0.40),
                    ("Wow", 0.25),
                    ("Flutter", 0.20),
                    ("Age", 0.35),
                    ("Spring", 0.20),
                    ("Wet", 0.40),
                ],
            },
            FxPreset {
                name: "Dub Chamber",
                values: &[
                    ("Time", 0.50),
                    ("Feedback", 0.70),
                    ("Age", 0.60),
                    ("Spring", 0.50),
                    ("Wet", 0.50),
                ],
            },
            FxPreset {
                name: "Old Machine",
                values: &[
                    ("Time", 0.20),
                    ("Feedback", 0.30),
                    ("Wow", 0.60),
                    ("Flutter", 0.55),
                    ("Age", 0.85),
                    ("Tone", 0.35),
                ],
            },
        ],
        AmberFang => &[
            FxPreset {
                name: "Classic",
                values: &[("Dist", 0.50), ("Tone", 0.50), ("Level", 0.70)],
            },
            FxPreset {
                name: "Scooped",
                values: &[("Dist", 0.70), ("Tone", 0.30), ("Level", 0.65)],
            },
            FxPreset {
                name: "Boost",
                values: &[("Dist", 0.20), ("Tone", 0.60), ("Level", 0.80)],
            },
        ],
        VelvetFuzz => &[
            FxPreset {
                name: "Big Fuzz",
                values: &[("Sustain", 0.70), ("Tone", 0.50), ("Level", 0.60)],
            },
            FxPreset {
                name: "Wall",
                values: &[("Sustain", 0.90), ("Tone", 0.35), ("Level", 0.55)],
            },
            FxPreset {
                name: "Light",
                values: &[("Sustain", 0.35), ("Tone", 0.60), ("Level", 0.65)],
            },
        ],
        Widener => &[
            FxPreset {
                name: "Narrow",
                values: &[("Width", 0.30)],
            },
            FxPreset {
                name: "Wide",
                values: &[("Width", 0.80)],
            },
        ],
        _ => &[],
    }
}

/// Where parameter `name` sits in `entry`'s knob list, or `None` when this
/// effect has no such knob.
///
/// Built-ins only: a hosted plugin brings its own names and its own presets,
/// and guessing that its "Drive" is choz's would be writing values into a
/// stranger's parameter.
pub fn param_index(entry: &AudioFxEntry, name: &str) -> Option<usize> {
    if entry.plugin.is_some() {
        return None;
    }
    fx_param_descs(entry.kind)
        .iter()
        .position(|d| d.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every name in every preset has to be a knob that exists. This is the
    /// whole reason the tables are keyed by name: a renamed parameter fails
    /// here, loudly, instead of leaving a preset that quietly does less than it
    /// says.
    #[test]
    fn every_preset_names_knobs_that_exist() {
        // Every built-in, from the one list the ADD FX modal uses: a copy of
        // it here would go stale the day an effect is added, which is exactly
        // the day this test is worth having.
        let kinds = crate::source::ALL_FX_KINDS;
        let mut with_presets = 0;
        for kind in kinds.iter().copied() {
            let descs = fx_param_descs(kind);
            let set = presets(kind);
            if !set.is_empty() {
                with_presets += 1;
            }
            for preset in set {
                assert!(
                    !preset.values.is_empty(),
                    "{kind:?}/{} moves nothing",
                    preset.name
                );
                for (name, value) in preset.values {
                    assert!(
                        descs.iter().any(|d| d.name == *name),
                        "{kind:?}/{}: no knob called {name}",
                        preset.name
                    );
                    assert!(
                        (0.0..=1.0).contains(value),
                        "{kind:?}/{}: {name} is {value}, and knobs are 0..1",
                        preset.name
                    );
                }
            }
        }
        assert!(
            with_presets >= 15,
            "only {with_presets} effects have presets"
        );
    }

    /// A preset resolves to knob positions on the entry it belongs to, and to
    /// nothing at all on a hosted plugin — whose "Drive" is not this "Drive".
    #[test]
    fn a_preset_resolves_against_the_entry_it_is_for() {
        let entry = AudioFxEntry::new(AudioFxKind::Delay);
        assert_eq!(param_index(&entry, "Feedback"), Some(1));
        assert_eq!(param_index(&entry, "Sustain"), None);

        let plugin = AudioFxEntry::new_plugin(crate::source::PluginFx {
            format: choz_engine::PluginFormat::Clap,
            path: "/nowhere.clap".into(),
            id: "x".into(),
            name: "X".into(),
            params: Vec::new(),
        });
        assert_eq!(param_index(&plugin, "Feedback"), None);
    }
}
