//! Project files: everything choz is currently set up to do, as YAML.
//!
//! One file holds both halves of the state — the sound (rack tabs, their
//! instruments, FX chains with every knob, the mixer, MIDI-learn bindings) and
//! the app configuration (plugin search paths, interface settings, OSC port).
//! Saving is what's implemented; the structs derive `Deserialize` too, so
//! loading is a matter of wiring, not of format.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Default file name when the user picks a directory rather than a file.
pub const DEFAULT_NAME: &str = "choz-project.yml";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    /// Format version, so a future loader can tell what it's reading.
    pub version: u32,
    pub audio: Audio,
    pub interface: Interface,
    pub plugin_paths: choz_engine::PluginPaths,
    pub rack: Vec<Slot>,
}

/// Engine-side settings that aren't per-slot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Audio {
    pub sample_rate: u32,
    pub buffer_size: u32,
    pub backend: String,
    pub output_device: Option<String>,
    pub osc_port: Option<u16>,
    /// MIDI ports the user switched off.
    pub disabled_midi_inputs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Interface {
    pub text_color: (u8, u8, u8),
    pub language: String,
}

/// One rack tab.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Slot {
    /// Note input bound to this tab, `"MIDI:<port>"` / `"OSC"` / none.
    pub input: Option<String>,
    pub instrument: Instrument,
    pub mixer: Mixer,
    pub fx: Vec<Fx>,
    /// MIDI-learn bindings that target this tab: `(cc, what it drives)`.
    pub midi_learn: Vec<(u8, String)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mixer {
    pub gain: f32,
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
}

/// What a tab plays. `kind` is `none` / `sf2` / `wav` / `plugin`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Instrument {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// SF2 only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bank: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset: Option<u8>,
    /// Plugin instruments: knob positions, 0..1, in the plugin's order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<f32>,
}

/// One FX in a chain, with every knob as the UI shows it (0..1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fx {
    /// Built-in FX id (`delay`, `amberfang`, …) or `clap` for a hosted plugin.
    pub kind: String,
    pub enabled: bool,
    pub wet: f32,
    pub params: Vec<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_id: Option<String>,
}

impl Project {
    /// Write the project to `path`, or to `path/choz-project.yml` when `path`
    /// is a directory.
    pub fn save(&self, path: &Path) -> anyhow::Result<PathBuf> {
        let file = if path.is_dir() { path.join(DEFAULT_NAME) } else { path.to_path_buf() };
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let yaml = serde_yaml::to_string(self)?;
        std::fs::write(&file, yaml)?;
        Ok(file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Project {
        Project {
            version: 1,
            audio: Audio {
                sample_rate: 48_000,
                buffer_size: 256,
                backend: "JACK".into(),
                output_device: Some("default".into()),
                osc_port: Some(9000),
                disabled_midi_inputs: vec!["Midi Through".into()],
            },
            interface: Interface { text_color: (240, 180, 90), language: "es".into() },
            plugin_paths: choz_engine::PluginPaths::default(),
            rack: vec![Slot {
                input: Some("MIDI:Keystation".into()),
                instrument: Instrument {
                    kind: "sf2".into(),
                    path: Some("/usr/share/sounds/sf2/FluidR3_GM.sf2".into()),
                    id: None,
                    name: None,
                    bank: Some(0),
                    preset: Some(4),
                    params: Vec::new(),
                },
                mixer: Mixer { gain: 0.8, pan: -0.25, mute: false, solo: false },
                fx: vec![Fx {
                    kind: "amberfang".into(),
                    enabled: true,
                    wet: 1.0,
                    params: vec![0.6, 0.4, 0.7, 1.0],
                    plugin_path: None,
                    plugin_id: None,
                }],
                midi_learn: vec![(74, "tab 1 \u{00b7} VOL".into())],
            }],
        }
    }

    /// Everything that matters survives the round trip, and the file really is
    /// YAML (not JSON-in-disguise).
    #[test]
    fn a_project_round_trips_through_yaml() {
        let p = sample();
        let yaml = serde_yaml::to_string(&p).unwrap();
        assert!(yaml.starts_with("version: 1\n"), "not YAML:\n{yaml}");
        assert!(yaml.contains("kind: amberfang"));
        assert!(yaml.contains("language: es"));
        assert!(yaml.contains("- 0.6"), "FX knobs are written out:\n{yaml}");

        let back: Project = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn saving_into_a_directory_uses_the_default_name() {
        let dir = std::env::temp_dir().join(format!("choz_proj_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let file = sample().save(&dir).unwrap();
        assert_eq!(file, dir.join(DEFAULT_NAME));
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("sample_rate: 48000"));

        // An explicit file name is honoured as-is.
        let named = dir.join("my-set.yml");
        assert_eq!(sample().save(&named).unwrap(), named);

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
