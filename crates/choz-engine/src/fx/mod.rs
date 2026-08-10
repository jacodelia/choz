//! Real-time FX processors (zero allocation in the audio callback).
//!
//! All processors implement [`FxProcessor`]: a single `process_block` call
//! transforms a stereo (interleaved L/R) buffer in place.

// ── Original ──────────────────────────────────────────────────────────────────
pub mod bitcrusher;
pub mod cassette;
pub mod delay;
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
pub mod parametric_eq;
// ── Modulation ───────────────────────────────────────────────────────────────
pub mod chorus;
pub mod flanger;
pub mod phaser;
// ── Spatial ──────────────────────────────────────────────────────────────────
pub mod widener;
// ── Utility ──────────────────────────────────────────────────────────────────
pub mod utility;
// ── New processors ───────────────────────────────────────────────────────────
pub mod expander;
pub mod pan;
// ── Creative time/texture ─────────────────────────────────────────────────────
pub mod pedal;
pub mod protocosmos;
pub mod z5_texture;
pub mod reverse;
pub mod space_echo;

// ── Re-exports ────────────────────────────────────────────────────────────────
pub use bitcrusher::Bitcrusher;
pub use cassette::Cassette;
pub use delay::DelayLine;
pub use filter::{Svf, SvfMode};
pub use filterbank::FilterBankFx;
pub use gran_delay::GranularDelay;
pub use isolator::Isolator;
pub use looper::{Looper, LooperState};
pub use reverb::Reverb;
pub use sidechain::SidechainDuck;
pub use vinyl::VinylSim;
// Dynamics
pub use compressor::Compressor;
pub use gate::Gate;
// Pitch
pub use autotune::{AutoTune, AutoTuneMode, AutoTuneParameters, ScaleType};
// EQ
pub use graphic_eq::{GraphicEq, EQ_BANDS, EQ_FREQS, PRESETS as EQ_PRESETS};
pub use parametric_eq::{EqBandKind, ParametricEq};
// Modulation
pub use chorus::Chorus;
pub use flanger::Flanger;
pub use phaser::Phaser;
// Spatial
pub use widener::StereoWidener;
// Utility
pub use utility::{Gain, MonoMaker, PhaseInvert, SoftClipper, TubeSaturation};
// New
pub use expander::Expander;
pub use pan::Pan;
// Creative time/texture
pub use pedal::{AmberFang, VelvetFuzz};
pub use protocosmos::Protocosmos;
pub use reverse::ReverseDelay;
pub use z5_texture::{Z5Texture, Z5Meter, Z5_WAVE_BINS};
pub use space_echo::SpaceEcho;

// The FX processor trait and param descriptor live in `choz-ports`; re-exported
// here so every `super::FxProcessor` / `crate::fx::FxParam` path across the fx/
// modules keeps resolving unchanged.
pub use choz_ports::{FxParam, FxProcessor};
