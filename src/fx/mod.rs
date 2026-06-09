//! Real-time FX processors (zero allocation in the audio callback).
//!
//! All processors implement [`FxProcessor`]: a single `process_block` call
//! transforms a stereo (interleaved L/R) buffer in place.

pub mod bitcrusher;
pub mod cassette;
pub mod chorus;
pub mod compressor;
pub mod delay;
pub mod expander;
pub mod filter;
pub mod filterbank;
pub mod flanger;
pub mod gate;
pub mod gran_delay;
pub mod isolator;
pub mod looper;
pub mod pan;
pub mod parametric_eq;
pub mod phaser;
pub mod reverb;
pub mod sidechain;
pub mod utility;
pub mod vinyl;
pub mod widener;

pub use bitcrusher::Bitcrusher;
pub use cassette::Cassette;
pub use chorus::Chorus;
pub use compressor::Compressor;
pub use delay::DelayLine;
pub use expander::Expander;
pub use filter::{Svf, SvfMode};
pub use filterbank::FilterBankFx;
pub use flanger::Flanger;
pub use gate::Gate;
pub use gran_delay::GranularDelay;
pub use isolator::Isolator;
pub use looper::Looper;
pub use pan::Pan;
pub use parametric_eq::{EqBandKind, ParametricEq};
pub use phaser::Phaser;
pub use reverb::Reverb;
pub use sidechain::SidechainDuck;
pub use utility::{Gain, MonoMaker, PhaseInvert, SoftClipper, TubeSaturation};
pub use vinyl::VinylSim;
pub use widener::StereoWidener;

/// A single automatable parameter descriptor.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct FxParam {
    pub name: &'static str,
    pub value: f32,
    pub min: f32,
    pub max: f32,
    pub unit: &'static str,
}

#[allow(dead_code)]
impl FxParam {
    pub const fn new(name: &'static str, value: f32, min: f32, max: f32, unit: &'static str) -> Self {
        Self { name, value, min, max, unit }
    }

    pub fn native(&self) -> f32 {
        self.min + self.value * (self.max - self.min)
    }
}

/// Common interface for all FX processors.
#[allow(dead_code)]
pub trait FxProcessor: Send {
    /// Process one stereo block in place. `buf` is interleaved L/R.
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32);

    /// Reset internal state.
    fn reset(&mut self);

    /// Dry/wet mix (0.0 = dry, 1.0 = fully wet).
    fn set_mix(&mut self, wet: f32);

    /// Human-readable name.
    fn name(&self) -> &str { "FX" }

    /// Return automatable parameter list.
    fn params(&self) -> Vec<FxParam> { Vec::new() }

    /// Set a parameter by index to a normalised 0.0–1.0 value.
    fn set_param(&mut self, _index: usize, _value: f32) {}
}
