//! Audio source types — what generates or processes audio for a channel.
//!
//! Based on the PATTERN view's SOURCE and FX functionalities.

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


impl AudioSource {
    pub fn kind_label(&self) -> &'static str {
        match self {
            AudioSource::Midi => "MIDI",
            AudioSource::Sf2 { .. } => "SF2",
            AudioSource::AudioFile { .. } => "AUDIO",
            AudioSource::Plugin { .. } => "SYNTH",
        }
    }
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
}

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
        }
    }

    #[allow(dead_code)]
    pub fn from_id(id: &str) -> Option<Self> {
        ALL_FX_KINDS.iter().copied().find(|k| k.id() == id)
    }
}

/// Descriptor for a single FX parameter.
#[derive(Debug, Clone, Copy)]
pub struct FxParamDesc {
    pub name: &'static str,
    pub default: f32,
}

macro_rules! pd { ($n:literal, $d:literal) => { FxParamDesc { name: $n, default: $d } } }

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
    }
}

/// One entry in the FX chain.
#[derive(Debug, Clone)]
pub struct AudioFxEntry {
    pub kind: AudioFxKind,
    pub wet: f32,
    pub enabled: bool,
    pub params: Vec<f32>,
}

impl AudioFxEntry {
    pub fn new(kind: AudioFxKind) -> Self {
        let descs = fx_param_descs(kind);
        let params: Vec<f32> = descs.iter().map(|d| d.default).collect();
        let wet = descs.last().filter(|d| d.name == "Wet").map(|d| d.default).unwrap_or(1.0);
        Self { kind, wet, enabled: true, params }
    }

    pub fn to_spec(&self) -> crate::fx_chain::FxSpec {
        crate::fx_chain::FxSpec {
            kind: self.kind.id().to_string(),
            enabled: self.enabled,
            wet: self.wet,
            params: self.params.clone(),
        }
    }

    #[allow(dead_code)]
    pub fn from_spec(spec: &crate::fx_chain::FxSpec) -> Option<Self> {
        let kind = AudioFxKind::from_id(&spec.kind)?;
        let descs = fx_param_descs(kind);
        let mut params: Vec<f32> = descs.iter().map(|d| d.default).collect();
        for (i, v) in spec.params.iter().enumerate() {
            if let Some(slot) = params.get_mut(i) { *slot = *v; }
        }
        Some(Self { kind, wet: spec.wet, enabled: spec.enabled, params })
    }
}
