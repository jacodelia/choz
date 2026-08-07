//! Audio source types — what generates or processes audio for a channel.
//!
//! Based on the PATTERN view's SOURCE and FX functionalities.

use std::borrow::Cow;
use std::path::PathBuf;
use serde::{Deserialize, Serialize};

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
    #[default] Delay,
    Reverb, GranDelay,
    Compressor, Limiter, Gate, Expander,
    ParamEq, Filter, FilterBank,
    Chorus, Flanger, Phaser,
    BitCrusher, Vinyl, Cassette, SoftClip, TubeSat,
    Widener, Isolator,
    Gain, PhaseInvert, MonoMaker, Pan,
    Looper, SidechainDuck,
    // Creative time/texture, imported from seqterm.
    Protocosmos, SpaceEcho, ReverseDelay, Z5Texture,
    // Stompbox distortions.
    AmberFang, VelvetFuzz,
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
        } else if has(&["dist", "drive", "sat", "fuzz", "crush", "clip", "tube", "amp"]) {
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
            Reverb => FxCategory::Reverb,
            Compressor | Limiter | Gate | Expander | SidechainDuck => FxCategory::Dynamics,
            ParamEq | Filter | FilterBank | Isolator => FxCategory::EqFilter,
            Chorus | Flanger | Phaser => FxCategory::Modulation,
            BitCrusher | Vinyl | Cassette | SoftClip | TubeSat | AmberFang | VelvetFuzz => {
                FxCategory::Distortion
            }
            Widener | Pan => FxCategory::Spatial,
            Protocosmos | Z5Texture | Looper => FxCategory::Texture,
            Gain | PhaseInvert | MonoMaker => FxCategory::Utility,
        }
    }
}

/// Max FX per rack slot (matches seqterm's chain length).
pub const MAX_FX: usize = 5;

pub const ALL_FX_KINDS: &[AudioFxKind] = &[
    AudioFxKind::Delay, AudioFxKind::Reverb, AudioFxKind::GranDelay,
    AudioFxKind::Compressor, AudioFxKind::Limiter, AudioFxKind::Gate, AudioFxKind::Expander,
    AudioFxKind::ParamEq, AudioFxKind::Filter, AudioFxKind::FilterBank,
    AudioFxKind::Chorus, AudioFxKind::Flanger, AudioFxKind::Phaser,
    AudioFxKind::BitCrusher, AudioFxKind::Vinyl, AudioFxKind::Cassette,
    AudioFxKind::SoftClip, AudioFxKind::TubeSat,
    AudioFxKind::Widener, AudioFxKind::Isolator,
    AudioFxKind::Gain, AudioFxKind::Pan, AudioFxKind::PhaseInvert, AudioFxKind::MonoMaker,
    AudioFxKind::Looper, AudioFxKind::SidechainDuck,
    AudioFxKind::Protocosmos, AudioFxKind::SpaceEcho, AudioFxKind::ReverseDelay,
    AudioFxKind::Z5Texture,
    AudioFxKind::AmberFang, AudioFxKind::VelvetFuzz,
];

impl AudioFxKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Delay => "DELAY", Self::Reverb => "REVERB", Self::GranDelay => "GRANDELAY",
            Self::Compressor => "COMPRESSOR", Self::Limiter => "LIMITER", Self::Gate => "GATE",
            Self::Expander => "EXPANDER",
            Self::ParamEq => "PARAM EQ", Self::Filter => "FILTER", Self::FilterBank => "FILTERBANK",
            Self::Chorus => "CHORUS", Self::Flanger => "FLANGER", Self::Phaser => "PHASER",
            Self::BitCrusher => "BITCRUSH", Self::Vinyl => "VINYL", Self::Cassette => "CASSETTE",
            Self::SoftClip => "SOFTCLIP", Self::TubeSat => "TUBE SAT",
            Self::Widener => "WIDENER", Self::Isolator => "ISOLATOR",
            Self::Gain => "GAIN", Self::PhaseInvert => "PHASE INV", Self::MonoMaker => "MONO",
            Self::Pan => "PAN",
            Self::Looper => "LOOPER", Self::SidechainDuck => "SIDECHAIN",
            Self::Protocosmos => "PROTOCOSMOS", Self::SpaceEcho => "SPACE ECHO",
            Self::ReverseDelay => "REVERSE", Self::Z5Texture => "Z5 TEXTURE",
            Self::AmberFang => "AMBER FANG", Self::VelvetFuzz => "VELVET FUZZ",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Delay => "delay", Self::Reverb => "reverb", Self::GranDelay => "grandelay",
            Self::Compressor => "compressor", Self::Limiter => "limiter", Self::Gate => "gate",
            Self::Expander => "expander",
            Self::ParamEq => "parameq", Self::Filter => "filter", Self::FilterBank => "filterbank",
            Self::Chorus => "chorus", Self::Flanger => "flanger", Self::Phaser => "phaser",
            Self::BitCrusher => "bitcrusher", Self::Vinyl => "vinyl", Self::Cassette => "cassette",
            Self::SoftClip => "softclip", Self::TubeSat => "tubesat",
            Self::Widener => "widener", Self::Isolator => "isolator",
            Self::Gain => "gain", Self::PhaseInvert => "phaseinvert", Self::MonoMaker => "monomaker",
            Self::Pan => "pan",
            Self::Looper => "looper", Self::SidechainDuck => "sidechain",
            Self::Protocosmos => "protocosmos", Self::SpaceEcho => "spaceecho",
            Self::ReverseDelay => "reversedelay", Self::Z5Texture => "z5texture",
            Self::AmberFang => "amberfang", Self::VelvetFuzz => "velvetfuzz",
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
}

macro_rules! pd {
    ($n:literal, $d:literal) => { FxParamDesc { name: Cow::Borrowed($n), default: $d } };
}

/// Static parameter table per FX kind.
pub fn fx_param_descs(kind: AudioFxKind) -> &'static [FxParamDesc] {
    use AudioFxKind::*;
    static DELAY:    &[FxParamDesc] = &[pd!("Time",0.30),pd!("Feedback",0.40),pd!("Damping",0.30),pd!("PingPong",0.00),pd!("Wet",1.00)];
    static REVERB:   &[FxParamDesc] = &[pd!("Room",0.50),pd!("Damping",0.50),pd!("Width",1.00),pd!("Wet",0.35)];
    static GRNDLY:   &[FxParamDesc] = &[pd!("Size",0.40),pd!("Density",0.50),pd!("Pitch",0.50),pd!("Feedback",0.30),pd!("Wet",0.80)];
    static COMP:     &[FxParamDesc] = &[pd!("Thresh",0.70),pd!("Ratio",0.18),pd!("Attack",0.10),pd!("Release",0.15),pd!("Makeup",0.00),pd!("Knee",0.50),pd!("Wet",1.00)];
    static LIMIT:    &[FxParamDesc] = &[pd!("Thresh",0.95),pd!("Release",0.25),pd!("Wet",1.00)];
    static GATE:     &[FxParamDesc] = &[pd!("Thresh",0.50),pd!("Attack",0.02),pd!("Hold",0.10),pd!("Release",0.20),pd!("Floor",0.00),pd!("Wet",1.00)];
    static PARAMEQ:  &[FxParamDesc] = &[pd!("Low",0.50),pd!("LowMid",0.50),pd!("HiMid",0.50),pd!("High",0.50),pd!("LowFreq",0.30),pd!("HiFreq",0.70),pd!("MidQ",0.30),pd!("Wet",1.00)];
    static FILTER:   &[FxParamDesc] = &[pd!("Cutoff",0.70),pd!("Res",0.20),pd!("Wet",1.00)];
    static FILTERBNK:&[FxParamDesc] = &[pd!("Low",0.50),pd!("Mid",0.50),pd!("High",0.50),pd!("Wet",1.00)];
    static CHORUS:   &[FxParamDesc] = &[pd!("Rate",0.20),pd!("Depth",0.30),pd!("Delay",0.30),pd!("Feedback",0.55),pd!("Wet",0.50)];
    static FLANGER:  &[FxParamDesc] = &[pd!("Rate",0.15),pd!("Depth",0.35),pd!("Delay",0.25),pd!("Feedback",0.70),pd!("Wet",0.70)];
    static PHASER:   &[FxParamDesc] = &[pd!("Rate",0.18),pd!("Depth",0.70),pd!("Center",0.40),pd!("Feedback",0.70),pd!("Wet",0.70)];
    static CRUSH:    &[FxParamDesc] = &[pd!("Bits",0.70),pd!("Rate",1.00),pd!("Wet",1.00)];
    static VINYL:    &[FxParamDesc] = &[pd!("Wow",0.20),pd!("Flutter",0.15),pd!("Crackle",0.10),pd!("Wet",1.00)];
    static CASSETTE: &[FxParamDesc] = &[pd!("Drive",0.40),pd!("Wet",1.00)];
    static SOFTCLIP: &[FxParamDesc] = &[pd!("Drive",0.25),pd!("Wet",1.00)];
    static TUBESAT:  &[FxParamDesc] = &[pd!("Drive",0.15),pd!("Tone",0.30),pd!("Wet",0.60)];
    static WIDENER:  &[FxParamDesc] = &[pd!("Width",0.50),pd!("Wet",1.00)];
    static ISOLATOR: &[FxParamDesc] = &[pd!("Low",0.50),pd!("Mid",0.50),pd!("High",0.50),pd!("Wet",1.00)];
    static GAIN:     &[FxParamDesc] = &[pd!("Gain",0.50),pd!("Wet",1.00)];
    static PHASEINV: &[FxParamDesc] = &[pd!("InvertL",1.00),pd!("InvertR",0.00)];
    static MONO:     &[FxParamDesc] = &[pd!("Wet",1.00)];
    static LOOPER:   &[FxParamDesc] = &[pd!("Length",0.50),pd!("Feedback",0.70),pd!("Wet",1.00)];
    static SIDECHAIN:&[FxParamDesc] = &[pd!("Amount",0.80),pd!("Release",0.30),pd!("Wet",1.00)];

    match kind {
        Delay => DELAY, Reverb => REVERB, GranDelay => GRNDLY,
        Compressor => COMP, Limiter => LIMIT, Gate => GATE,
        ParamEq => PARAMEQ, Filter => FILTER, FilterBank => FILTERBNK,
        Chorus => CHORUS, Flanger => FLANGER, Phaser => PHASER,
        BitCrusher => CRUSH, Vinyl => VINYL, Cassette => CASSETTE,
        SoftClip => SOFTCLIP, TubeSat => TUBESAT,
        Widener => WIDENER, Isolator => ISOLATOR,
        Gain => GAIN, PhaseInvert => PHASEINV, MonoMaker => MONO,
        Looper => LOOPER, SidechainDuck => SIDECHAIN,
        Expander => &[pd!("Thresh",0.50),pd!("Ratio",0.25),pd!("Attack",0.20),pd!("Release",0.30),pd!("Range",0.75)],
        Pan => &[pd!("Pan",0.50),pd!("ConstPwr",1.00)],
        // Imported from seqterm: parameters are normalised and in the same
        // order the processors report them from `params()`.
        Protocosmos => &[pd!("Size",0.40),pd!("Density",0.50),pd!("Pitch",0.50),pd!("Spray",0.30),
                         pd!("Reverse",0.00),pd!("Freeze",0.00),pd!("Diffuse",0.40),pd!("Wet",0.60)],
        SpaceEcho => &[pd!("Time",0.35),pd!("Feedback",0.35),pd!("Wow",0.20),pd!("Flutter",0.20),
                       pd!("Age",0.30),pd!("Spring",0.25),pd!("Tone",0.50),pd!("Wet",0.50)],
        ReverseDelay => &[pd!("Time",0.35),pd!("Feedback",0.30),pd!("Wet",0.60)],
        // Stompboxes: knob names as they read on the pedal.
        AmberFang => &[pd!("Dist",0.50),pd!("Tone",0.50),pd!("Level",0.70),pd!("Wet",1.00)],
        VelvetFuzz => &[pd!("Sustain",0.60),pd!("Tone",0.50),pd!("Level",0.60),pd!("Wet",1.00)],
        Z5Texture => &[pd!("Size",0.40),pd!("Density",0.60),pd!("Spray",0.50),pd!("Overlap",0.50),
                       pd!("Pitch",0.50),pd!("RndPitch",0.30),pd!("Reverse",0.20),pd!("Spread",0.60),
                       pd!("Freeze",0.00),pd!("Feedbk",0.40),pd!("Stretch",0.50),pd!("Position",0.50),
                       pd!("Drift",0.20),pd!("Blur",0.20),pd!("BufLen",1.00),pd!("Wet",0.60)],
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

/// How many plugin parameters the FX panel shows. The knob row is the limit,
/// not the plugin: everything past this needs the plugin's own GUI.
/// ponytail: raise it (or add paging) if someone actually needs more.
pub const MAX_PLUGIN_PARAMS: usize = 7;

/// One entry in the FX chain: either a built-in FX or a hosted plugin effect.
#[derive(Debug, Clone)]
pub struct AudioFxEntry {
    pub kind: AudioFxKind,
    pub wet: f32,
    pub enabled: bool,
    pub params: Vec<f32>,
    /// `Some` when this slot hosts a plugin effect; `kind` is then unused.
    pub plugin: Option<PluginFx>,
}

impl AudioFxEntry {
    pub fn new(kind: AudioFxKind) -> Self {
        let descs = fx_param_descs(kind);
        let params: Vec<f32> = descs.iter().map(|d| d.default).collect();
        let wet = descs.last().filter(|d| d.name == "Wet").map(|d| d.default).unwrap_or(1.0);
        Self { kind, wet, enabled: true, params, plugin: None }
    }

    pub fn new_plugin(plugin: PluginFx) -> Self {
        // Knob positions start where the plugin says its defaults are; the
        // trailing knob is choz's own dry/wet.
        let mut params: Vec<f32> = plugin
            .params
            .iter()
            .take(MAX_PLUGIN_PARAMS)
            .map(|p| p.normalised(p.default) as f32)
            .collect();
        params.push(1.0);
        Self { kind: AudioFxKind::default(), wet: 1.0, enabled: true, params, plugin: Some(plugin) }
    }

    /// True when knob `index` is choz's dry/wet rather than a plugin parameter.
    pub fn is_mix_param(&self, index: usize) -> bool {
        match &self.plugin {
            Some(c) => index == c.params.len().min(MAX_PLUGIN_PARAMS),
            None => false,
        }
    }

    /// Display label: the plugin name for hosted effects, the FX name otherwise.
    pub fn label(&self) -> &str {
        match &self.plugin {
            Some(c) => &c.name,
            None => self.kind.label(),
        }
    }

    /// Parameters this entry exposes: the plugin's own (capped) plus dry/wet
    /// for a hosted effect, or the static table for a built-in.
    pub fn param_descs(&self) -> Vec<FxParamDesc> {
        match &self.plugin {
            Some(c) => c
                .params
                .iter()
                .take(MAX_PLUGIN_PARAMS)
                .map(|p| FxParamDesc {
                    name: Cow::Owned(p.name.clone()),
                    default: p.normalised(p.default) as f32,
                })
                .chain(std::iter::once(pd!("Wet", 1.00)))
                .collect(),
            None => fx_param_descs(self.kind).to_vec(),
        }
    }

    pub fn to_spec(&self) -> choz_engine::fx_chain::FxSpec {
        choz_engine::fx_chain::FxSpec {
            kind: self.kind.id().to_string(),
            enabled: self.enabled,
            wet: self.wet,
            params: self.params.clone(),
            plugin: self.plugin.as_ref().map(|c| choz_engine::fx_chain::PluginFxRef {
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
            if let Some(slot) = params.get_mut(i) { *slot = *v; }
        }
        Some(Self { kind, wet: spec.wet, enabled: spec.enabled, params, plugin: None })
    }
}
