//! Audio source types — what generates or processes audio for a channel.
//!
//! Based on the PATTERN view's SOURCE and FX functionalities.

use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::path::PathBuf;

// ─── Audio Source ──────────────────────────────────────────────────────────────

/// Defines what generates audio for a channel. Based on PatternSource.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[derive(Default)]
pub enum AudioSource {
    /// Pass-through / silence.
    #[default]
    Midi,
    /// SF2 SoundFont synthesis (via oxisynth or fluidsynth).
    Sf2 {
        #[serde(default)]
        path: PathBuf,
        #[serde(default)]
        bank: u8,
        #[serde(default)]
        preset: u8,
    },
    /// Audio file playback (WAV, etc.).
    AudioFile {
        #[serde(default)]
        path: PathBuf,
        #[serde(default)]
        looping: bool,
    },
    /// External synth plugin.
    Plugin {
        id: String,
        format: String,
        name: String,
    },
}

// ─── FX Types ──────────────────────────────────────────────────────────────────

/// FX processor types available for the audio chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AudioFxKind {
    #[default]
    Delay,
    Reverb,
    GranDelay,
    Compressor,
    Limiter,
    Gate,
    Expander,
    ParamEq,
    GraphicEq,
    Filter,
    FilterBank,
    Chorus,
    Flanger,
    Phaser,
    /// Level moved by an LFO; the same processor as [`AudioFxKind::AutoPan`].
    Tremolo,
    /// The balance moved by the same LFO.
    AutoPan,
    /// A filter with an LFO and an envelope follower on its cutoff.
    AutoFilter,
    /// A slice of the bar, caught and looped on choz's own transport.
    BeatRepeat,
    /// Up to eight transposed voices, in the key.
    Harmonizer,
    /// One sound wearing another's mouth — and a talkbox with `INPUT R`.
    Vocoder,
    /// Every partial moved by the same number of Hz (one sideband).
    FreqShifter,
    /// The same carrier, both sidebands.
    RingMod,
    /// A reverb whose tail climbs, because the shifter is inside its loop.
    Shimmer,
    BitCrusher,
    Vinyl,
    Cassette,
    SoftClip,
    /// The general waveshaper: eight curves, bias, tone and 1x–8x oversampling.
    Saturator,
    /// The same processor with the curve drawn instead of computed.
    WaveShaper,
    TubeSat,
    Widener,
    Isolator,
    Gain,
    PhaseInvert,
    MonoMaker,
    Pan,
    Looper,
    SidechainDuck,
    // Creative time/texture, imported from seqterm.
    Protocosmos,
    SpaceEcho,
    ReverseDelay,
    Z5Texture,
    // Stompbox distortions.
    AmberFang,
    VelvetFuzz,
    // Pitch.
    AutoTune,
}

/// What an effect does, for grouping the ADD FX list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FxCategory {
    Delay,
    Reverb,
    Dynamics,
    EqFilter,
    Modulation,
    Distortion,
    Spatial,
    Texture,
    /// Anything that moves the pitch itself rather than the timbre around it.
    Pitch,
    Utility,
    /// A hosted plugin whose name gives nothing away.
    Other,
}

impl FxCategory {
    /// Display order of the section headers.
    pub const ALL: &'static [FxCategory] = &[
        FxCategory::Delay,
        FxCategory::Reverb,
        FxCategory::Dynamics,
        FxCategory::EqFilter,
        FxCategory::Modulation,
        FxCategory::Distortion,
        FxCategory::Spatial,
        FxCategory::Texture,
        FxCategory::Pitch,
        FxCategory::Utility,
        FxCategory::Other,
    ];

    pub fn label(self) -> &'static str {
        match self {
            FxCategory::Delay => "DELAY",
            FxCategory::Reverb => "REVERB",
            FxCategory::Dynamics => "DYNAMICS",
            FxCategory::EqFilter => "EQ / FILTER",
            FxCategory::Modulation => "MODULATION",
            FxCategory::Distortion => "DISTORTION",
            FxCategory::Spatial => "SPATIAL",
            FxCategory::Texture => "TEXTURE",
            FxCategory::Pitch => "PITCH",
            FxCategory::Utility => "UTILITY",
            FxCategory::Other => "OTHER",
        }
    }

    /// Best guess for a hosted plugin, from its name. Plugin formats don't all
    /// expose a category (CLAP features and LV2 classes would need loading or
    /// TTL parsing), so the name is what there is.
    pub fn guess(name: &str) -> FxCategory {
        let n = name.to_lowercase();
        let has = |words: &[&str]| words.iter().any(|w| n.contains(w));
        if has(&["delay", "echo", "tape"]) {
            FxCategory::Delay
        } else if has(&["reverb", "verb", "room", "hall", "plate"]) {
            FxCategory::Reverb
        } else if has(&["comp", "limit", "gate", "expand", "duck", "maxim", "level"]) {
            FxCategory::Dynamics
        } else if has(&["eq", "filter", "cut", "lowpass", "highpass", "band", "tilt"]) {
            FxCategory::EqFilter
        } else if has(&["chorus", "flang", "phas", "trem", "vibr", "mod", "rotary"]) {
            FxCategory::Modulation
        } else if has(&[
            "dist", "drive", "sat", "fuzz", "crush", "clip", "tube", "amp",
        ]) {
            FxCategory::Distortion
        } else if has(&["pan", "width", "stereo", "spatial", "ambi", "binaural"]) {
            FxCategory::Spatial
        } else if has(&["gran", "texture", "freeze", "cosmos"]) {
            FxCategory::Texture
        } else if has(&["gain", "mono", "utility", "invert", "meter", "tuner"]) {
            FxCategory::Utility
        } else {
            FxCategory::Other
        }
    }
}

impl AudioFxKind {
    /// Which section of the ADD FX list this built-in belongs to.
    pub fn category(self) -> FxCategory {
        use AudioFxKind::*;
        match self {
            Delay | GranDelay | ReverseDelay | SpaceEcho => FxCategory::Delay,
            Reverb | Shimmer => FxCategory::Reverb,
            Compressor | Limiter | Gate | Expander | SidechainDuck => FxCategory::Dynamics,
            ParamEq | GraphicEq | Filter | FilterBank | Isolator => FxCategory::EqFilter,
            Chorus | Flanger | Phaser | Tremolo | AutoPan | AutoFilter | FreqShifter | RingMod => {
                FxCategory::Modulation
            }
            BitCrusher | Vinyl | Cassette | SoftClip | Saturator | WaveShaper | TubeSat
            | AmberFang | VelvetFuzz => FxCategory::Distortion,
            Widener | Pan => FxCategory::Spatial,
            Protocosmos | Z5Texture | Looper | BeatRepeat => FxCategory::Texture,
            AutoTune | Harmonizer | Vocoder => FxCategory::Pitch,
            Gain | PhaseInvert | MonoMaker => FxCategory::Utility,
        }
    }
}

/// Max FX per rack slot.
///
/// Twelve, not seqterm's five: a guitar chain is a tuner, a gate, a compressor,
/// a drive, a modulation and two delays before anyone has thought about reverb.
/// The chain row wraps onto further lines on its own, so the only thing this
/// number costs is the DSP the user asked for.
pub const MAX_FX: usize = 12;

pub const ALL_FX_KINDS: &[AudioFxKind] = &[
    AudioFxKind::Delay,
    AudioFxKind::Reverb,
    AudioFxKind::GranDelay,
    AudioFxKind::Compressor,
    AudioFxKind::Limiter,
    AudioFxKind::Gate,
    AudioFxKind::Expander,
    AudioFxKind::ParamEq,
    AudioFxKind::GraphicEq,
    AudioFxKind::Filter,
    AudioFxKind::FilterBank,
    AudioFxKind::Chorus,
    AudioFxKind::Flanger,
    AudioFxKind::Phaser,
    AudioFxKind::Tremolo,
    AudioFxKind::AutoPan,
    AudioFxKind::AutoFilter,
    AudioFxKind::BeatRepeat,
    AudioFxKind::Harmonizer,
    AudioFxKind::Vocoder,
    AudioFxKind::FreqShifter,
    AudioFxKind::RingMod,
    AudioFxKind::Shimmer,
    AudioFxKind::BitCrusher,
    AudioFxKind::Vinyl,
    AudioFxKind::Cassette,
    AudioFxKind::SoftClip,
    AudioFxKind::Saturator,
    AudioFxKind::WaveShaper,
    AudioFxKind::TubeSat,
    AudioFxKind::Widener,
    AudioFxKind::Isolator,
    AudioFxKind::Gain,
    AudioFxKind::Pan,
    AudioFxKind::PhaseInvert,
    AudioFxKind::MonoMaker,
    AudioFxKind::Looper,
    AudioFxKind::SidechainDuck,
    AudioFxKind::Protocosmos,
    AudioFxKind::SpaceEcho,
    AudioFxKind::ReverseDelay,
    AudioFxKind::Z5Texture,
    AudioFxKind::AmberFang,
    AudioFxKind::VelvetFuzz,
    AudioFxKind::AutoTune,
];

impl AudioFxKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Delay => "DELAY",
            Self::Reverb => "REVERB",
            Self::GranDelay => "GRANDELAY",
            Self::Compressor => "COMPRESSOR",
            Self::Limiter => "LIMITER",
            Self::Gate => "GATE",
            Self::Expander => "EXPANDER",
            Self::ParamEq => "PARAM EQ",
            Self::GraphicEq => "GRAPHIC EQ",
            Self::Filter => "FILTER",
            Self::FilterBank => "FILTERBANK",
            Self::Chorus => "CHORUS",
            Self::Flanger => "FLANGER",
            Self::Phaser => "PHASER",
            Self::Tremolo => "TREMOLO",
            Self::AutoPan => "AUTO PAN",
            Self::AutoFilter => "AUTO FILTER",
            Self::BeatRepeat => "BEAT REPEAT",
            Self::Harmonizer => "HARMONIZER",
            Self::Vocoder => "VOCODER",
            Self::FreqShifter => "FREQ SHIFT",
            Self::RingMod => "RING MOD",
            Self::Shimmer => "SHIMMER",
            Self::BitCrusher => "BITCRUSH",
            Self::Vinyl => "VINYL",
            Self::Cassette => "CASSETTE",
            Self::SoftClip => "SOFTCLIP",
            Self::Saturator => "SATURATOR",
            Self::WaveShaper => "WAVESHAPER",
            Self::TubeSat => "TUBE SAT",
            Self::Widener => "WIDENER",
            Self::Isolator => "ISOLATOR",
            Self::Gain => "GAIN",
            Self::PhaseInvert => "PHASE INV",
            Self::MonoMaker => "MONO",
            Self::Pan => "PAN",
            Self::Looper => "LOOPER",
            Self::SidechainDuck => "SIDECHAIN",
            Self::Protocosmos => "PROTOCOSMOS",
            Self::SpaceEcho => "SPACE ECHO",
            Self::ReverseDelay => "REVERSE",
            Self::Z5Texture => "Z5 TEXTURE",
            Self::AmberFang => "AMBER FANG",
            Self::VelvetFuzz => "VELVET FUZZ",
            Self::AutoTune => "AUTO-TUNE",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Delay => "delay",
            Self::Reverb => "reverb",
            Self::GranDelay => "grandelay",
            Self::Compressor => "compressor",
            Self::Limiter => "limiter",
            Self::Gate => "gate",
            Self::Expander => "expander",
            Self::ParamEq => "parameq",
            Self::GraphicEq => "graphiceq",
            Self::Filter => "filter",
            Self::FilterBank => "filterbank",
            Self::Chorus => "chorus",
            Self::Flanger => "flanger",
            Self::Phaser => "phaser",
            Self::Tremolo => "tremolo",
            Self::AutoPan => "autopan",
            Self::AutoFilter => "autofilter",
            Self::BeatRepeat => "beatrepeat",
            Self::Harmonizer => "harmonizer",
            Self::Vocoder => "vocoder",
            Self::FreqShifter => "freqshifter",
            Self::RingMod => "ringmod",
            Self::Shimmer => "shimmer",
            Self::BitCrusher => "bitcrusher",
            Self::Vinyl => "vinyl",
            Self::Cassette => "cassette",
            Self::SoftClip => "softclip",
            Self::Saturator => "saturator",
            Self::WaveShaper => "waveshaper",
            Self::TubeSat => "tubesat",
            Self::Widener => "widener",
            Self::Isolator => "isolator",
            Self::Gain => "gain",
            Self::PhaseInvert => "phaseinvert",
            Self::MonoMaker => "monomaker",
            Self::Pan => "pan",
            Self::Looper => "looper",
            Self::SidechainDuck => "sidechain",
            Self::Protocosmos => "protocosmos",
            Self::SpaceEcho => "spaceecho",
            Self::ReverseDelay => "reversedelay",
            Self::Z5Texture => "z5texture",
            Self::AmberFang => "amberfang",
            Self::VelvetFuzz => "velvetfuzz",
            Self::AutoTune => "autotune",
        }
    }

    #[allow(dead_code)]
    pub fn from_id(id: &str) -> Option<Self> {
        ALL_FX_KINDS.iter().copied().find(|k| k.id() == id)
    }
}

/// Descriptor for a single FX parameter. Built-ins use static names; a hosted
/// plugin's names come from the plugin, hence the `Cow`.
#[derive(Debug, Clone)]
pub struct FxParamDesc {
    pub name: Cow<'static, str>,
    pub default: f32,
    /// What kind of control the parameter is. Built-in FX are all continuous;
    /// a hosted plugin says for itself.
    pub shape: ParamShape,
}

/// The control a parameter deserves, decided by what the parameter *is*.
///
/// Never guessed from the name — that is the mistake `FxCategory::guess` gets
/// away with because a wrong category only misfiles a row in a list, while a
/// cutoff drawn as a switch is unusable. A host that reports nothing leaves
/// everything [`ParamShape::Continuous`], which is what choz always drew.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum ParamShape {
    /// A knob: any value in the range.
    #[default]
    Continuous,
    /// On or off. Nothing in between exists, so an arc at 0.00 is a lie.
    Toggle,
    /// Named positions in order, each with the 0..1 place it sits at.
    ///
    /// The positions are **not** evenly spaced in general: Ardour's a-delay
    /// names ten note divisions at 1, 2, 4, 6, 8, 12, 16, 24, 32 and 48 over a
    /// range of 1..48. Assuming a uniform grid there shows the wrong name and
    /// steps to values the plugin never offered.
    Named(Vec<(f32, String)>),
    /// A travel rather than a rotation: a mix, a pan, a time. Same values a
    /// knob would take, drawn as the distance covered because that is how the
    /// parameter reads.
    ///
    /// Carries the plugin's unit, which is both why it is a fader and how a run
    /// of them is recognised as one group — an ADSR is four consecutive times.
    Fader(String),
}

/// Units that mean "a distance along something" — a time, a share, a position.
///
/// This is the plugin's own `units:unit`, not a guess at its name: `FxCategory`
/// guesses from names and gets away with it because a wrong category only
/// misfiles a row, while a control that does not match the parameter is used
/// wrong. A plugin that reports no unit keeps the knob.
/// `pc` is LV2's name for percent (`units:pc`), and after inline definitions it
/// and `ms` are the two most common units in the 261 bundles installed here.
const FADER_UNITS: &[&str] = &[
    "s", "ms", "sec", "seconds", "%", "pc", "percent", "cent", "cents",
];

impl ParamShape {
    /// The shape a hosted plugin's parameter reports.
    pub fn of(p: &choz_engine::PluginParam) -> Self {
        if p.is_toggle() {
            return ParamShape::Toggle;
        }
        // Named steps only when every step has a name: a partial list would
        // draw "3/8" for the ones the plugin skipped and lie about the rest.
        if !p.points.is_empty() && p.points.len() as u32 == p.steps {
            return ParamShape::Named(
                p.points
                    .iter()
                    .map(|(v, l)| (p.normalised(*v) as f32, l.clone()))
                    .collect(),
            );
        }
        // A time, a share or a position is read as how far along it is; a
        // frequency or a gain is read as a setting. The unit is the only thing
        // the plugin says about which of the two this is.
        if p.unit.as_deref().is_some_and(|u| {
            let u = u.trim().to_lowercase();
            FADER_UNITS.contains(&u.as_str())
        }) {
            return ParamShape::Fader(p.unit.clone().unwrap_or_default());
        }
        ParamShape::Continuous
    }

    /// Where one press of `←`/`→` lands.
    ///
    /// A stepped parameter moves one position, not one twentieth of its range:
    /// a switch nudged by 0.05 needs twenty presses to flip and spends the
    /// other nineteen in places it has no name for.
    pub fn nudge(&self, current: f32, delta: f32) -> f32 {
        let Some((k, n)) = self.step_at(current) else {
            return (current + delta).clamp(0.0, 1.0);
        };
        let dir: i64 = if delta >= 0.0 { 1 } else { -1 };
        let next = (k as i64 + dir).clamp(0, n as i64 - 1) as usize;
        self.position_of(next)
    }

    /// The 0..1 value of step `k`.
    fn position_of(&self, k: usize) -> f32 {
        match self {
            ParamShape::Continuous | ParamShape::Fader(_) => 0.0,
            ParamShape::Toggle => k.min(1) as f32,
            ParamShape::Named(points) => points.get(k).map(|(v, _)| *v).unwrap_or(0.0),
        }
    }

    /// Index of the position `norm` (0..1) selects, and how many there are.
    /// `None` for a continuous parameter, which has neither.
    pub fn step_at(&self, norm: f32) -> Option<(usize, usize)> {
        let norm = norm.clamp(0.0, 1.0);
        match self {
            // Neither has positions: they take any value in the range.
            ParamShape::Continuous | ParamShape::Fader(_) => None,
            ParamShape::Toggle => Some((usize::from(norm >= 0.5), 2)),
            ParamShape::Named(points) if points.is_empty() => None,
            // Nearest, not rounded onto a grid: the positions can sit anywhere.
            ParamShape::Named(points) => {
                let k = points
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| (a.0 - norm).abs().total_cmp(&(b.0 - norm).abs()))
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                Some((k, points.len()))
            }
        }
    }

    /// The label of step `k`, when it has one.
    pub fn label(&self, k: usize) -> Option<&str> {
        match self {
            ParamShape::Named(points) => points.get(k).map(|(_, l)| l.as_str()),
            _ => None,
        }
    }
}

macro_rules! pd {
    ($n:literal, $d:literal) => {
        FxParamDesc {
            name: Cow::Borrowed($n),
            default: $d,
            shape: ParamShape::Continuous,
        }
    };
}

/// Static parameter table per FX kind.
pub fn fx_param_descs(kind: AudioFxKind) -> &'static [FxParamDesc] {
    use AudioFxKind::*;
    /// `Wet` is not last here: from index 4 on, the knobs are the processor's
    /// own `set_param` order, and moving them would move a saved project's
    /// values to a different knob.
    static DELAY: &[FxParamDesc] = &[
        pd!("Time", 0.30),
        pd!("Feedback", 0.40),
        pd!("Damping", 0.30),
        pd!("PingPong", 0.00),
        pd!("Wet", 1.00),
        pd!("Cross", 0.00),
        pd!("ModRate", 0.03),
        pd!("ModDepth", 0.00),
    ];
    static REVERB: &[FxParamDesc] = &[
        pd!("Room", 0.50),
        pd!("Damping", 0.50),
        pd!("Width", 1.00),
        pd!("Wet", 0.35),
    ];
    static GRNDLY: &[FxParamDesc] = &[
        pd!("Size", 0.40),
        pd!("Density", 0.50),
        pd!("Pitch", 0.50),
        pd!("Feedback", 0.30),
        pd!("Wet", 0.80),
    ];
    /// The order is the processor's own `set_param` order — the compressor
    /// takes values live, so a knob that moves here writes there by index.
    /// `Detect` is a list of names, filled in by `param_descs` from the DSP.
    static COMP: &[FxParamDesc] = &[
        pd!("Thresh", 0.70),
        pd!("Ratio", 0.18),
        pd!("Attack", 0.10),
        pd!("Release", 0.15),
        pd!("Makeup", 0.00),
        pd!("Knee", 0.50),
        pd!("Detect", 0.00),
        pd!("Link", 1.00),
        pd!("SC HPF", 0.00),
        pd!("Wet", 1.00),
    ];
    static LIMIT: &[FxParamDesc] = &[
        pd!("Thresh", 0.95),
        pd!("Release", 0.25),
        pd!("Look", 0.20),
        pd!("Link", 1.00),
        pd!("Wet", 1.00),
    ];
    static GATE: &[FxParamDesc] = &[
        pd!("Thresh", 0.50),
        pd!("Attack", 0.02),
        pd!("Hold", 0.10),
        pd!("Release", 0.20),
        pd!("Floor", 0.00),
        pd!("Hyst", 0.25),
        pd!("Wet", 1.00),
    ];
    // Ten Winamp bands, a preamp and the preset list — each one a knob, so a CC
    // can ride a single band. The preset is drawn as a named step.
    static GRAPHICEQ: &[FxParamDesc] = &[
        pd!("70", 0.50),
        pd!("180", 0.50),
        pd!("320", 0.50),
        pd!("600", 0.50),
        pd!("1k", 0.50),
        pd!("3k", 0.50),
        pd!("6k", 0.50),
        pd!("12k", 0.50),
        pd!("14k", 0.50),
        pd!("16k", 0.50),
        pd!("Preamp", 0.50),
        FxParamDesc {
            name: Cow::Borrowed("Preset"),
            default: 0.0,
            shape: ParamShape::Continuous,
        },
        pd!("Wet", 1.00),
    ];
    // AUTO-TUNE. Named steps where the value is a name — a key is C or it is
    // not, and drawing "0.36" for D would be a knob that means nothing.
    static AUTOTUNE: &[FxParamDesc] = &[
        FxParamDesc {
            name: Cow::Borrowed("Preset"),
            default: 0.0,
            shape: ParamShape::Continuous,
        },
        pd!("Retune", 0.08),
        pd!("Correct", 1.00),
        FxParamDesc {
            name: Cow::Borrowed("Key"),
            default: 0.0,
            shape: ParamShape::Continuous,
        },
        FxParamDesc {
            name: Cow::Borrowed("Scale"),
            default: 0.0,
            shape: ParamShape::Continuous,
        },
        FxParamDesc {
            name: Cow::Borrowed("Mode"),
            default: 0.0,
            shape: ParamShape::Continuous,
        },
        pd!("Human", 0.00),
        pd!("A4", 0.50),
        pd!("MinHz", 0.03),
        pd!("MaxHz", 1.00),
        pd!("Sens", 0.50),
        pd!("OutGain", 0.50),
        pd!("Wet", 1.00),
    ];
    static PARAMEQ: &[FxParamDesc] = &[
        pd!("Low", 0.50),
        pd!("LowMid", 0.50),
        pd!("HiMid", 0.50),
        pd!("High", 0.50),
        pd!("LowFreq", 0.30),
        pd!("HiFreq", 0.70),
        pd!("MidQ", 0.30),
        pd!("Mode", 0.00),
        pd!("Solo", 0.00),
        pd!("Wet", 1.00),
    ];
    static FILTER: &[FxParamDesc] = &[pd!("Cutoff", 0.70), pd!("Res", 0.20), pd!("Wet", 1.00)];
    static FILTERBNK: &[FxParamDesc] = &[
        pd!("Low", 0.50),
        pd!("Mid", 0.50),
        pd!("High", 0.50),
        pd!("Wet", 1.00),
    ];
    static CHORUS: &[FxParamDesc] = &[
        pd!("Rate", 0.20),
        pd!("Depth", 0.30),
        pd!("Delay", 0.30),
        pd!("Feedback", 0.55),
        pd!("Wet", 0.50),
    ];
    static FLANGER: &[FxParamDesc] = &[
        pd!("Rate", 0.15),
        pd!("Depth", 0.35),
        pd!("Delay", 0.25),
        pd!("Feedback", 0.70),
        pd!("Wet", 0.70),
    ];
    static PHASER: &[FxParamDesc] = &[
        pd!("Rate", 0.18),
        pd!("Depth", 0.70),
        pd!("Center", 0.40),
        pd!("Feedback", 0.70),
        pd!("Wet", 0.70),
    ];
    /// Tremolo and auto-pan share their knobs because they share their
    /// processor: only what the LFO is pointed at differs.
    static TREMOLO: &[FxParamDesc] = &[
        pd!("Rate", 0.35),
        pd!("Depth", 0.50),
        pd!("Shape", 0.00),
        pd!("Spread", 0.00),
        pd!("Wet", 1.00),
    ];
    static AUTOFILTER: &[FxParamDesc] = &[
        pd!("Freq", 0.50),
        pd!("Res", 0.40),
        pd!("Mode", 0.00),
        pd!("Rate", 0.35),
        pd!("Depth", 0.50),
        pd!("Shape", 0.00),
        pd!("Spread", 0.00),
        pd!("Env", 0.50),
        pd!("Wet", 1.00),
    ];
    static BEATREPEAT: &[FxParamDesc] = &[
        pd!("Interval", 0.75),
        pd!("Grain", 0.40),
        pd!("Chance", 1.00),
        pd!("Decay", 1.00),
        pd!("Wet", 1.00),
    ];
    /// The shifter and the ring modulator share their knobs the way the
    /// tremolo and the auto-pan do: one processor, one carrier, two uses.
    static FREQSHIFT: &[FxParamDesc] = &[pd!("Freq", 0.50), pd!("Spread", 0.00), pd!("Wet", 1.00)];
    static SHIMMER: &[FxParamDesc] = &[
        pd!("Size", 0.85),
        pd!("PreDelay", 0.25),
        pd!("Shift", 1.00),
        pd!("Feedback", 0.50),
        pd!("Damping", 0.50),
        pd!("Width", 0.50),
        pd!("Wet", 0.40),
    ];
    /// Voices, shape, key and scale are lists of names; the rest are knobs.
    static HARMONIZER: &[FxParamDesc] = &[
        pd!("Voices", 0.334),
        pd!("Shape", 0.00),
        pd!("Key", 0.00),
        pd!("Scale", 0.20),
        pd!("Detune", 0.32),
        pd!("Delay", 0.36),
        pd!("Env", 0.50),
        pd!("Width", 1.00),
        pd!("Wet", 0.50),
        // Follow a keyboard instead of the shape and key: a switch, and the
        // channel it listens on. The channel's list of names is built where the
        // other named shapes are — a `static` cannot hold one.
        FxParamDesc {
            name: Cow::Borrowed("MIDI"),
            default: 0.0,
            shape: ParamShape::Toggle,
        },
        pd!("Ch", 0.00),
    ];
    /// Bands and carrier are lists of names; the rest are knobs.
    static VOCODER: &[FxParamDesc] = &[
        pd!("Bands", 0.50),
        pd!("Carrier", 0.00),
        pd!("Pitch", 0.35),
        pd!("Res", 0.36),
        pd!("Speed", 0.20),
        pd!("Shift", 0.50),
        pd!("Wet", 1.00),
    ];
    static CRUSH: &[FxParamDesc] = &[pd!("Bits", 0.70), pd!("Rate", 1.00), pd!("Wet", 1.00)];
    static VINYL: &[FxParamDesc] = &[
        pd!("Wow", 0.20),
        pd!("Flutter", 0.15),
        pd!("Crackle", 0.10),
        pd!("Wet", 1.00),
    ];
    static CASSETTE: &[FxParamDesc] = &[pd!("Drive", 0.40), pd!("Wet", 1.00)];
    static SOFTCLIP: &[FxParamDesc] = &[pd!("Drive", 0.25), pd!("Wet", 1.00)];
    /// `Curve` and `Oversamp` are lists of names, not knobs — the shapes are
    /// filled in by `param_descs`, from the DSP's own enums, so a label can
    /// never drift from what the processor does.
    static SATURATOR: &[FxParamDesc] = &[
        pd!("Drive", 0.30),
        pd!("Curve", 0.00),
        pd!("Bias", 0.50),
        pd!("Tone", 1.00),
        pd!("Output", 0.50),
        pd!("Oversamp", 0.3333),
        pd!("Wet", 1.00),
    ];
    /// Eight points of a curve, then what happens around it. The points come
    /// first because that is the order `Saturator::waveshaper` reads them and
    /// the order the panel draws them as a bank.
    static WAVESHAPER: &[FxParamDesc] = &[
        pd!("P1", 0.00),
        pd!("P2", 0.143),
        pd!("P3", 0.286),
        pd!("P4", 0.429),
        pd!("P5", 0.571),
        pd!("P6", 0.714),
        pd!("P7", 0.857),
        pd!("P8", 1.00),
        pd!("Drive", 0.00),
        pd!("Tone", 1.00),
        pd!("Output", 0.50),
        pd!("Oversamp", 0.6667),
        pd!("Wet", 1.00),
    ];
    static TUBESAT: &[FxParamDesc] = &[pd!("Drive", 0.15), pd!("Tone", 0.30), pd!("Wet", 0.60)];
    static WIDENER: &[FxParamDesc] = &[pd!("Width", 0.50), pd!("Wet", 1.00)];
    static ISOLATOR: &[FxParamDesc] = &[
        pd!("Low", 0.50),
        pd!("Mid", 0.50),
        pd!("High", 0.50),
        pd!("Wet", 1.00),
    ];
    static GAIN: &[FxParamDesc] = &[pd!("Gain", 0.50), pd!("Wet", 1.00)];
    static PHASEINV: &[FxParamDesc] = &[pd!("InvertL", 1.00), pd!("InvertR", 0.00)];
    static MONO: &[FxParamDesc] = &[pd!("Wet", 1.00)];
    static LOOPER: &[FxParamDesc] = &[pd!("Length", 0.50), pd!("Feedback", 0.70), pd!("Wet", 1.00)];
    static SIDECHAIN: &[FxParamDesc] =
        &[pd!("Amount", 0.80), pd!("Release", 0.30), pd!("Wet", 1.00)];

    match kind {
        Delay => DELAY,
        Reverb => REVERB,
        GranDelay => GRNDLY,
        Compressor => COMP,
        Limiter => LIMIT,
        Gate => GATE,
        AutoTune => AUTOTUNE,
        ParamEq => PARAMEQ,
        GraphicEq => GRAPHICEQ,
        Filter => FILTER,
        FilterBank => FILTERBNK,
        Chorus => CHORUS,
        Flanger => FLANGER,
        Phaser => PHASER,
        Tremolo | AutoPan => TREMOLO,
        AutoFilter => AUTOFILTER,
        BeatRepeat => BEATREPEAT,
        Harmonizer => HARMONIZER,
        Vocoder => VOCODER,
        FreqShifter | RingMod => FREQSHIFT,
        Shimmer => SHIMMER,
        BitCrusher => CRUSH,
        Vinyl => VINYL,
        Cassette => CASSETTE,
        SoftClip => SOFTCLIP,
        Saturator => SATURATOR,
        WaveShaper => WAVESHAPER,
        TubeSat => TUBESAT,
        Widener => WIDENER,
        Isolator => ISOLATOR,
        Gain => GAIN,
        PhaseInvert => PHASEINV,
        MonoMaker => MONO,
        Looper => LOOPER,
        SidechainDuck => SIDECHAIN,
        Expander => &[
            pd!("Thresh", 0.50),
            pd!("Ratio", 0.25),
            pd!("Attack", 0.20),
            pd!("Release", 0.30),
            pd!("Range", 0.75),
        ],
        Pan => &[pd!("Pan", 0.50), pd!("ConstPwr", 1.00)],
        // Imported from seqterm: parameters are normalised and in the same
        // order the processors report them from `params()`.
        Protocosmos => &[
            pd!("Size", 0.40),
            pd!("Density", 0.50),
            pd!("Pitch", 0.50),
            pd!("Spray", 0.30),
            pd!("Reverse", 0.00),
            pd!("Freeze", 0.00),
            pd!("Diffuse", 0.40),
            pd!("Wet", 0.60),
        ],
        SpaceEcho => &[
            pd!("Time", 0.35),
            pd!("Feedback", 0.35),
            pd!("Wow", 0.20),
            pd!("Flutter", 0.20),
            pd!("Age", 0.30),
            pd!("Spring", 0.25),
            pd!("Tone", 0.50),
            pd!("Wet", 0.50),
        ],
        ReverseDelay => &[pd!("Time", 0.35), pd!("Feedback", 0.30), pd!("Wet", 0.60)],
        // Stompboxes: knob names as they read on the pedal.
        AmberFang => &[
            pd!("Dist", 0.50),
            pd!("Tone", 0.50),
            pd!("Level", 0.70),
            pd!("Wet", 1.00),
        ],
        VelvetFuzz => &[
            pd!("Sustain", 0.60),
            pd!("Tone", 0.50),
            pd!("Level", 0.60),
            pd!("Wet", 1.00),
        ],
        Z5Texture => &[
            pd!("Size", 0.40),
            pd!("Density", 0.60),
            pd!("Spray", 0.50),
            pd!("Overlap", 0.50),
            pd!("Pitch", 0.50),
            pd!("RndPitch", 0.30),
            pd!("Reverse", 0.20),
            pd!("Spread", 0.60),
            pd!("Freeze", 0.00),
            pd!("Feedbk", 0.40),
            pd!("Stretch", 0.50),
            pd!("Position", 0.50),
            pd!("Drift", 0.20),
            pd!("Blur", 0.20),
            pd!("BufLen", 1.00),
            pd!("Wet", 0.60),
        ],
    }
}

/// A hosted plugin audio effect (CLAP, LV2): which plugin it is, what to call
/// it, and the parameters it exposes (read once when the FX is added).
#[derive(Debug, Clone)]
pub struct PluginFx {
    pub format: choz_engine::PluginFormat,
    /// The plugin file, or the bundle directory for LV2.
    pub path: PathBuf,
    /// Plugin id inside that file: CLAP id, LV2 URI.
    pub id: String,
    pub name: String,
    pub params: Vec<choz_engine::PluginParam>,
}

/// One entry in the FX chain: either a built-in FX or a hosted plugin effect.
#[derive(Debug, Clone)]
pub struct AudioFxEntry {
    pub kind: AudioFxKind,
    pub wet: f32,
    pub enabled: bool,
    pub params: Vec<f32>,
    /// `Some` when this slot hosts a plugin effect; `kind` is then unused.
    pub plugin: Option<PluginFx>,
    /// The plugin's own state (its patch), as the project stores it. Empty for
    /// built-ins, which have nothing beyond their parameters.
    pub state: Vec<u8>,
}

impl AudioFxEntry {
    pub fn new(kind: AudioFxKind) -> Self {
        let descs = fx_param_descs(kind);
        let params: Vec<f32> = descs.iter().map(|d| d.default).collect();
        // By name, not by position: a built-in whose knobs must be in the
        // processor's `set_param` order cannot always put `Wet` last.
        let wet = descs
            .iter()
            .find(|d| d.name == "Wet")
            .map(|d| d.default)
            .unwrap_or(1.0);
        Self {
            kind,
            wet,
            enabled: true,
            params,
            plugin: None,
            state: Vec::new(),
        }
    }

    pub fn new_plugin(plugin: PluginFx) -> Self {
        // Knob positions start where the plugin says its defaults are; the
        // trailing knob is choz's own dry/wet.
        // Every parameter the plugin has, not a prefix of them: a value that is
        // not stored can be neither edited nor saved with the project. The knob
        // grid wraps onto as many rows as it needs and scrolls with the cursor,
        // so there is nothing to cap here.
        let mut params: Vec<f32> = plugin
            .params
            .iter()
            .map(|p| p.normalised(p.default) as f32)
            .collect();
        params.push(1.0);
        Self {
            kind: AudioFxKind::default(),
            wet: 1.0,
            enabled: true,
            params,
            plugin: Some(plugin),
            state: Vec::new(),
        }
    }

    /// Whether the built-in processor takes parameter changes **live**.
    ///
    /// This is the difference between turning a knob and losing the sound: a
    /// built-in that does not implement `set_param` only picks a value up when
    /// the chain is rebuilt, and a rebuild replaces **every** processor in the
    /// chain — so nudging a Gain knob throws away the reverb's tail, the
    /// delay's buffer and the looper's recording. Everything with state is on
    /// this list, and the ones that are not are stateless: rebuilding them is
    /// inaudible.
    pub fn takes_live_params(kind: AudioFxKind) -> bool {
        use AudioFxKind::*;
        matches!(
            kind,
            Delay
                | Reverb
                | GranDelay
                | Compressor
                | Limiter
                | Gate
                | Expander
                | GraphicEq
                | Filter
                | Chorus
                | Flanger
                | Phaser
                | Tremolo
                | AutoPan
                | AutoFilter
                | BeatRepeat
                | FreqShifter
                | RingMod
                | Shimmer
                | Harmonizer
                | Vocoder
                | BitCrusher
                | Vinyl
                | Widener
                | Pan
                | Protocosmos
                | SpaceEcho
                | ReverseDelay
                | Z5Texture
                | AmberFang
                | VelvetFuzz
                | AutoTune
        )
    }

    /// A preset knob that fills in the knobs below it.
    ///
    /// AutoTune's presets are five sets of five values, and the parameter array
    /// here is the state everything else reads: the project saves it and the
    /// chain is rebuilt from it. So picking a preset has to *write* those
    /// values, not just tell the processor — otherwise the rebuild reads the
    /// array, finds the defaults, and the preset lasts exactly until the next
    /// knob is touched.
    ///
    /// Returns true when it changed something, so the caller knows to rebuild.
    pub fn apply_preset(&mut self, index: usize) -> bool {
        if self.plugin.is_some() {
            return false;
        }
        // The graphic EQ's preset is the last-but-one knob, not the first.
        if self.kind == AudioFxKind::GraphicEq {
            use choz_engine::fx::{graphic_eq, EQ_BANDS, EQ_PRESETS};
            let Some(slot) = self.param_descs().iter().position(|d| d.name == "Preset") else {
                return false;
            };
            if index != slot {
                return false;
            }
            let pick = graphic_eq::preset_index(self.params.get(slot).copied().unwrap_or(0.0));
            let Some((_, gains)) = EQ_PRESETS.get(pick) else {
                return false;
            };
            // **Write the bands.** The processor was already told, but the
            // sliders draw from this array and the project saves it — a preset
            // the sliders do not show is a preset that vanishes the next time
            // anything is rebuilt.
            for (b, db) in gains.iter().enumerate().take(EQ_BANDS) {
                if let Some(p) = self.params.get_mut(b) {
                    *p = graphic_eq::db_to_norm(*db);
                }
            }
            return true;
        }
        if self.kind != AudioFxKind::AutoTune || index != 0 {
            return false;
        }
        use choz_engine::fx::autotune;
        let pick = autotune::preset_index(self.params.first().copied().unwrap_or(0.0));
        let Some(&(_, retune, correction, humanize, mode)) = autotune::PRESETS.get(pick) else {
            return false;
        };
        let set = |ps: &mut Vec<f32>, i: usize, v: f32| {
            if let Some(slot) = ps.get_mut(i) {
                *slot = v.clamp(0.0, 1.0);
            }
        };
        set(&mut self.params, 1, retune / 1000.0);
        set(&mut self.params, 2, correction);
        set(
            &mut self.params,
            5,
            (mode == autotune::AutoTuneMode::HardTune) as u8 as f32,
        );
        set(&mut self.params, 6, humanize);
        true
    }

    /// True when knob `index` is choz's dry/wet rather than a parameter of the
    /// processor itself.
    ///
    /// A hosted plugin gets one appended after its own; a built-in declares it
    /// as a knob called `Wet`. **Both have to answer yes**: the dry/wet lives
    /// in `entry.wet`, and that is what a rebuild re-applies — a built-in that
    /// answered "no" here kept its mix in `params` alone and lost it the next
    /// time anything rebuilt the chain.
    pub fn is_mix_param(&self, index: usize) -> bool {
        match &self.plugin {
            Some(c) => index == c.params.len(),
            None => fx_param_descs(self.kind)
                .get(index)
                .is_some_and(|d| d.name == "Wet"),
        }
    }

    /// Display label: the plugin name for hosted effects, the FX name otherwise.
    pub fn label(&self) -> &str {
        match &self.plugin {
            Some(c) => &c.name,
            None => self.kind.label(),
        }
    }

    /// Parameters this entry exposes: all of the plugin's own plus dry/wet for
    /// a hosted effect, or the static table for a built-in.
    pub fn param_descs(&self) -> Vec<FxParamDesc> {
        match &self.plugin {
            Some(c) => c
                .params
                .iter()
                .map(|p| FxParamDesc {
                    name: Cow::Owned(p.name.clone()),
                    default: p.normalised(p.default) as f32,
                    shape: ParamShape::of(p),
                })
                .chain(std::iter::once(pd!("Wet", 1.00)))
                .collect(),
            None => {
                let mut descs = fx_param_descs(self.kind).to_vec();
                // The preset row is a list of names, not a number: the same
                // control a plugin's enumerated parameter gets.
                // Same for the saturator's curve and oversampling factor:
                // there is nothing between two curves, so they are named
                // positions rather than a knob with a number on it.
                if self.kind == AudioFxKind::Saturator || self.kind == AudioFxKind::WaveShaper {
                    use choz_engine::fx::oversample::Factor;
                    use choz_engine::fx::saturator::Curve;
                    if let Some(d) = descs.iter_mut().find(|d| d.name == "Curve") {
                        d.shape = ParamShape::Named(
                            Curve::ALL
                                .iter()
                                .map(|c| (c.to_norm(), c.label().to_string()))
                                .collect(),
                        );
                    }
                    if let Some(d) = descs.iter_mut().find(|d| d.name == "Oversamp") {
                        d.shape = ParamShape::Named(
                            Factor::ALL
                                .iter()
                                .map(|f| (f.to_norm(), f.label().to_string()))
                                .collect(),
                        );
                    }
                }
                // The detector mode, the EQ's target and the soloed band are
                // positions with names, not numbers: "0.5" says nothing about
                // whether the detector is following peaks or loudness.
                // A modulation shape, a filter mode and a grid division are
                // all names. The lists come from the DSP's own enums, so a
                // label cannot drift from what the processor does.
                if matches!(
                    self.kind,
                    AudioFxKind::Tremolo | AudioFxKind::AutoPan | AudioFxKind::AutoFilter
                ) {
                    use choz_engine::fx::lfo::Wave;
                    if let Some(d) = descs.iter_mut().find(|d| d.name == "Shape") {
                        d.shape = ParamShape::Named(
                            Wave::ALL
                                .iter()
                                .map(|w| (w.to_norm(), w.label().to_string()))
                                .collect(),
                        );
                    }
                }
                if self.kind == AudioFxKind::AutoFilter {
                    use choz_engine::fx::auto_filter::FilterMode;
                    if let Some(d) = descs.iter_mut().find(|d| d.name == "Mode") {
                        d.shape = ParamShape::Named(
                            FilterMode::ALL
                                .iter()
                                .map(|m| (m.to_norm(), m.label().to_string()))
                                .collect(),
                        );
                    }
                }
                // A voice count, a chord shape, a key and a scale are all
                // names. A knob at 0.4 does not say "four voices in D minor".
                if self.kind == AudioFxKind::Harmonizer {
                    use choz_engine::fx::autotune::{ScaleType, NOTE_NAMES};
                    use choz_engine::fx::harmonizer::{Shape, VOICE_COUNTS};
                    let named = |d: &mut FxParamDesc, items: Vec<String>| {
                        let last = items.len().saturating_sub(1).max(1) as f32;
                        d.shape = ParamShape::Named(
                            items
                                .into_iter()
                                .enumerate()
                                .map(|(i, n)| (i as f32 / last, n))
                                .collect(),
                        );
                    };
                    for d in descs.iter_mut() {
                        match d.name.as_ref() {
                            "Voices" => {
                                named(d, VOICE_COUNTS.iter().map(|c| c.to_string()).collect())
                            }
                            "Shape" => named(
                                d,
                                Shape::ALL.iter().map(|s| s.label().to_string()).collect(),
                            ),
                            "Key" => {
                                named(d, NOTE_NAMES.iter().map(|n| (*n).to_string()).collect())
                            }
                            "Scale" => named(
                                d,
                                ScaleType::ALL
                                    .iter()
                                    .map(|s| s.label().to_string())
                                    .collect(),
                            ),
                            // Sixteen channels are a list, not a knob: nudging
                            // through them one arrow press at a time is what a
                            // picker is for.
                            "Ch" => named(d, (1..=16).map(|c| c.to_string()).collect()),
                            _ => {}
                        }
                    }
                }
                if self.kind == AudioFxKind::Vocoder {
                    use choz_engine::fx::vocoder::{Carrier, BAND_COUNTS};
                    if let Some(d) = descs.iter_mut().find(|d| d.name == "Bands") {
                        let last = (BAND_COUNTS.len() - 1) as f32;
                        d.shape = ParamShape::Named(
                            BAND_COUNTS
                                .iter()
                                .enumerate()
                                .map(|(i, c)| (i as f32 / last, c.to_string()))
                                .collect(),
                        );
                    }
                    if let Some(d) = descs.iter_mut().find(|d| d.name == "Carrier") {
                        d.shape = ParamShape::Named(
                            Carrier::ALL
                                .iter()
                                .map(|c| (c.to_norm(), c.label().to_string()))
                                .collect(),
                        );
                    }
                }
                if self.kind == AudioFxKind::BeatRepeat {
                    use choz_engine::fx::beat_repeat::{GRAINS, INTERVALS};
                    let named = |list: &[(f32, &str)]| {
                        let last = (list.len() - 1) as f32;
                        ParamShape::Named(
                            list.iter()
                                .enumerate()
                                .map(|(i, (_, name))| (i as f32 / last, (*name).to_string()))
                                .collect(),
                        )
                    };
                    if let Some(d) = descs.iter_mut().find(|d| d.name == "Interval") {
                        d.shape = named(&INTERVALS);
                    }
                    if let Some(d) = descs.iter_mut().find(|d| d.name == "Grain") {
                        d.shape = named(&GRAINS);
                    }
                }
                if self.kind == AudioFxKind::Compressor {
                    use choz_engine::fx::compressor::Detect;
                    if let Some(d) = descs.iter_mut().find(|d| d.name == "Detect") {
                        d.shape = ParamShape::Named(
                            Detect::ALL
                                .iter()
                                .map(|m| (m.to_norm(), m.label().to_string()))
                                .collect(),
                        );
                    }
                }
                if self.kind == AudioFxKind::ParamEq {
                    use choz_engine::fx::parametric_eq::EqMode;
                    if let Some(d) = descs.iter_mut().find(|d| d.name == "Mode") {
                        d.shape = ParamShape::Named(
                            EqMode::ALL
                                .iter()
                                .map(|m| (m.to_norm(), m.label().to_string()))
                                .collect(),
                        );
                    }
                    if let Some(d) = descs.iter_mut().find(|d| d.name == "Solo") {
                        d.shape = ParamShape::Named(vec![
                            (0.0, "off".to_string()),
                            (0.25, "Low".to_string()),
                            (0.5, "LowMid".to_string()),
                            (0.75, "HiMid".to_string()),
                            (1.0, "High".to_string()),
                        ]);
                    }
                }
                if self.kind == AudioFxKind::GraphicEq {
                    let last = choz_engine::fx::EQ_PRESETS.len().saturating_sub(1).max(1) as f32;
                    let points = choz_engine::fx::EQ_PRESETS
                        .iter()
                        .enumerate()
                        .map(|(i, (name, _))| (i as f32 / last, (*name).to_string()))
                        .collect();
                    if let Some(d) = descs.iter_mut().find(|d| d.name == "Preset") {
                        d.shape = ParamShape::Named(points);
                    }
                }
                // Same for AutoTune: the preset, the key, the scale and the
                // mode are all names. A knob at 0.36 does not say "D".
                if self.kind == AudioFxKind::AutoTune {
                    use choz_engine::fx::autotune;
                    let named = |d: &mut FxParamDesc, items: Vec<String>| {
                        let last = items.len().saturating_sub(1).max(1) as f32;
                        d.shape = ParamShape::Named(
                            items
                                .into_iter()
                                .enumerate()
                                .map(|(i, n)| (i as f32 / last, n))
                                .collect(),
                        );
                    };
                    for d in descs.iter_mut() {
                        match d.name.as_ref() {
                            "Preset" => named(
                                d,
                                autotune::PRESETS.iter().map(|p| p.0.to_string()).collect(),
                            ),
                            "Key" => named(
                                d,
                                autotune::NOTE_NAMES
                                    .iter()
                                    .map(|n| (*n).to_string())
                                    .collect(),
                            ),
                            "Scale" => named(
                                d,
                                autotune::ScaleType::ALL
                                    .iter()
                                    .map(|s| s.label().to_string())
                                    .collect(),
                            ),
                            "Mode" => {
                                named(d, vec!["Natural".to_string(), "Hard Tune".to_string()])
                            }
                            _ => {}
                        }
                    }
                }
                descs
            }
        }
    }

    pub fn to_spec(&self) -> choz_engine::fx_chain::FxSpec {
        choz_engine::fx_chain::FxSpec {
            kind: self.kind.id().to_string(),
            enabled: self.enabled,
            wet: self.wet,
            params: self.params.clone(),
            plugin: self
                .plugin
                .as_ref()
                .map(|c| choz_engine::fx_chain::PluginFxRef {
                    format: c.format,
                    path: c.path.clone(),
                    id: c.id.clone(),
                }),
        }
    }

    #[allow(dead_code)]
    pub fn from_spec(spec: &choz_engine::fx_chain::FxSpec) -> Option<Self> {
        let kind = AudioFxKind::from_id(&spec.kind)?;
        let descs = fx_param_descs(kind);
        let mut params: Vec<f32> = descs.iter().map(|d| d.default).collect();
        for (i, v) in spec.params.iter().enumerate() {
            if let Some(slot) = params.get_mut(i) {
                *slot = *v;
            }
        }
        Some(Self {
            kind,
            wet: spec.wet,
            enabled: spec.enabled,
            params,
            plugin: None,
            state: Vec::new(),
        })
    }
}
