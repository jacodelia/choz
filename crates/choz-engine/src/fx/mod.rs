//! Real-time FX processors (zero allocation in the audio callback).
//!
//! All processors implement [`FxProcessor`]: a single `process_block` call
//! transforms a stereo (interleaved L/R) buffer in place.

// ── Original ──────────────────────────────────────────────────────────────────
pub mod bitcrusher;
pub mod cassette;
pub mod delay;
pub mod delay_line;
pub mod filter;
pub mod filterbank;
pub mod gran_delay;
pub mod isolator;
pub mod looper;
pub mod reverb;
pub mod sidechain;
pub mod vinyl;
// ── Dynamics ─────────────────────────────────────────────────────────────────
pub mod compressor;
pub mod gate;
// ── Pitch ────────────────────────────────────────────────────────────────────
pub mod autotune;
// ── EQ ───────────────────────────────────────────────────────────────────────
pub mod graphic_eq;
pub mod harmonizer;
pub mod parametric_eq;
// ── Modulation ───────────────────────────────────────────────────────────────
pub mod auto_filter;
pub mod beat_repeat;
pub mod chorus;
pub mod envelope;
pub mod flanger;
pub mod freq_shift;
pub mod lfo;
pub mod phaser;
pub mod tremolo;
// ── Spatial ──────────────────────────────────────────────────────────────────
pub mod widener;
// ── Utility ──────────────────────────────────────────────────────────────────
pub mod oversample;
pub mod saturator;
pub mod smooth;
pub mod utility;
pub mod vocoder;
// ── New processors ───────────────────────────────────────────────────────────
pub mod expander;
pub mod pan;
// ── Creative time/texture ─────────────────────────────────────────────────────
pub mod pedal;
pub mod protocosmos;
pub mod reverse;
pub mod shift;
pub mod shimmer;
pub mod space_echo;
pub mod z5_texture;

// ── Re-exports ────────────────────────────────────────────────────────────────
pub use bitcrusher::Bitcrusher;
pub use cassette::Cassette;
pub use delay::DelayLine;
pub use filter::{Svf, SvfMode};
pub use filterbank::FilterBankFx;
pub use gran_delay::GranularDelay;
pub use isolator::Isolator;
pub use looper::{Looper, MAX_SECS as LOOP_MAX_SECS};
pub use reverb::Reverb;
pub use sidechain::SidechainDuck;
pub use vinyl::VinylSim;
// Dynamics
pub use compressor::Compressor;
pub use gate::Gate;
pub use saturator::{Curve, Saturator};
// Pitch
pub use autotune::{AutoTune, AutoTuneMode, AutoTuneParameters, ScaleType};
// EQ
pub use graphic_eq::{GraphicEq, EQ_BANDS, EQ_FREQS, PRESETS as EQ_PRESETS};
pub use harmonizer::Harmonizer;
pub use parametric_eq::{EqBandKind, ParametricEq};
// Modulation
pub use auto_filter::{AutoFilter, FilterMode};
pub use beat_repeat::BeatRepeat;
pub use chorus::Chorus;
pub use envelope::Envelope;
pub use flanger::Flanger;
pub use freq_shift::{Carrier, FreqShift};
pub use lfo::{Lfo, Wave};
pub use phaser::Phaser;
pub use tremolo::{ModTarget, Tremolo};
// Spatial
pub use widener::StereoWidener;
// Utility
pub use utility::{Gain, MonoMaker, PhaseInvert, SoftClipper, TubeSaturation};
pub use vocoder::Vocoder;
// New
pub use expander::Expander;
pub use pan::Pan;
// Creative time/texture
pub use pedal::{AmberFang, VelvetFuzz};
pub use protocosmos::Protocosmos;
pub use reverse::ReverseDelay;
pub use shift::VoiceShifter;
pub use shimmer::ShimmerReverb;
pub use space_echo::SpaceEcho;
pub use z5_texture::{Z5Meter, Z5Texture, Z5_WAVE_BINS};

// The FX processor trait and param descriptor live in `choz-ports`; re-exported
// here so every `super::FxProcessor` / `crate::fx::FxParam` path across the fx/
// modules keeps resolving unchanged.
pub use choz_ports::{FxParam, FxProcessor};
