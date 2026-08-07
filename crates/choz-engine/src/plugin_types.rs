//! Plugin types and host port trait.
//! Defines the interface for discovering, loading, and processing plugins.

use std::path::PathBuf;
use anyhow::Result;

/// The plugin format.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginKind {
    Vst2,
    Vst3,
    Clap,
    Au,
    Ladspa,
    Dssi,
    Lv2,
    Sfz,
    Sf2,
    Internal,
}

impl PluginKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Vst2     => "VST2",
            Self::Vst3     => "VST3",
            Self::Clap     => "CLAP",
            Self::Au       => "AU",
            Self::Ladspa   => "LADSPA",
            Self::Dssi     => "DSSI",
            Self::Lv2      => "LV2",
            Self::Sfz      => "SFZ",
            Self::Sf2      => "SF2",
            Self::Internal => "FX",
        }
    }
}

/// Metadata describing a discovered plugin.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct PluginDescriptor {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
    pub kind: PluginKind,
    pub path: PathBuf,
    pub is_instrument: bool,
    pub is_effect: bool,
}

/// Port: plugin host — scan, instantiate, and communicate with plugins.
#[allow(dead_code)]
pub trait PluginHostPort: Send + Sync {
    /// Scan a directory for plugins.
    fn scan(&mut self, dir: &std::path::Path) -> Result<Vec<PluginDescriptor>>;

    /// List all known plugins from the last scan.
    fn list_plugins(&self) -> &[PluginDescriptor];

    /// Instantiate a plugin by ID.
    fn instantiate(&mut self, plugin_id: &str, sample_rate: u32, block_size: u32) -> Result<u64>;

    /// Destroy a plugin instance.
    fn destroy(&mut self, instance_id: u64);

    /// Process one audio block through a plugin instance.
    fn process(&mut self, instance_id: u64, input: &[f32], output: &mut [f32]) -> Result<()>;

    /// Return the number of automatable parameters.
    fn param_count(&self, _instance_id: u64) -> u32 { 0 }

    /// Get a parameter value (normalised 0.0–1.0).
    fn get_param(&self, _instance_id: u64, _param_id: u32) -> f32 { 0.0 }

    /// Set a parameter value (normalised 0.0–1.0).
    fn set_param(&mut self, _instance_id: u64, _param_id: u32, _value: f32) {}

    /// Human-readable parameter name.
    fn param_name(&self, _instance_id: u64, param_id: u32) -> String { format!("P{param_id}") }
}
