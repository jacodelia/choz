//! Project files: everything choz is currently set up to do, as YAML.
//!
//! One file holds both halves of the state — the sound (rack tabs, their
//! instruments, FX chains with every knob, the mixer, MIDI-learn bindings) and
//! the app configuration (plugin search paths, interface settings, OSC port).
//! Both directions are wired: `save` writes the file, `load` reads it back and
//! `App::apply_project` rebuilds the rack from it.

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
    /// Recorded parameter moves. Added later, hence the default: a project
    /// written before automation existed simply has none.
    #[serde(default)]
    pub automation: crate::automation::Automation,
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
    /// MIDI channel this tab answers in MULTI mode, 1..16. Added later, so a
    /// project written before the mode existed still loads — channel 1 for a
    /// single tab is the same thing it always did.
    #[serde(default = "default_channel")]
    pub channel: u8,
    pub instrument: Instrument,
    pub mixer: Mixer,
    pub fx: Vec<Fx>,
    /// MIDI-learn bindings that target this tab.
    pub midi_learn: Vec<Binding>,
    /// The tab's arpeggiator. Added later, so `default` (off) keeps every
    /// project written before it loadable and sounding the same.
    /// A MIDI port this tab also plays to, by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub midi_out: Option<String>,
    #[serde(default)]
    pub arp: crate::arp::ArpSettings,
}

fn default_channel() -> u8 {
    1
}

/// One MIDI-learn binding. `target` is what gets restored; `label` is written
/// only so the file reads like the UI does, and is ignored on load.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Binding {
    pub cc: u8,
    pub target: crate::LearnTarget,
    #[serde(default)]
    pub label: String,
    /// Which controller the CC has to come from, `"MIDI:<port>"` / `"OSC"`.
    /// Absent in projects written before bindings knew, and absent for a
    /// binding learned from anything but a port — both mean "any source".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mixer {
    pub gain: f32,
    /// The right channel's own level, and whether the two move together.
    /// Added later, so a project written before the strips had two faders loads
    /// with them linked — which is what it sounded like.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gain_r: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<bool>,
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
    /// Device output channels this tab plays out of, 0-based. Absent in
    /// projects saved before per-slot routing existed, which is the first pair.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out_pair: Option<(usize, usize)>,
    /// Device *input* channels feeding this tab, when it runs on live audio
    /// instead of its instrument.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_pair: Option<(usize, usize)>,
    /// **The names of those two jacks**, which is what actually identifies
    /// them.
    ///
    /// The pair above is an index into a flat list of every capture port in the
    /// system, and that list moves: unplug an interface and every index after
    /// it shifts by two, so a project reopened without the card was quietly
    /// listening to somebody else's microphone. The names are matched first and
    /// the indices are only the fallback for projects written before this
    /// existed — same reasoning as `midi_out`, which has stored a name all
    /// along.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_ports: Option<(String, String)>,
    /// Audio in, notes out. Added later, hence the default.
    #[serde(default)]
    pub pitch_to_midi: bool,
    /// How much of a converting tab is the instrument rather than the audio
    /// that drove it. Added later; `None` means "all instrument", which is
    /// what it did before there was a choice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pitch_mix: Option<f32>,
    /// Trim on the audio input, and the level `A→M` calls a note. Both added
    /// later; `None` means "whatever the defaults are now".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_gain: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_gate: Option<f32>,
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
    /// The plugin's own state, base64. Everything about the sound that is not a
    /// parameter: the patch picked in its browser, a wavetable, a sample path.
    /// Written as text so the project stays a readable YAML file.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub state: String,
    /// The folder of preset **files** this tab uses as its bank, for a plugin
    /// that publishes no programs of its own (Surge XT's VST3 build reports
    /// none and keeps its patches as `.fxp`). Without it the bank has to be
    /// pointed at again every time the project opens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bank_dir: Option<PathBuf>,
    /// DSSI only: the `configure` key/values the plugin was given. Saving these
    /// is what the DSSI convention asks of a host, and without them a project
    /// with FluidSynth-DSSI in it reopens with no SoundFont and no sound.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub config: Vec<(String, String)>,
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
    /// The plugin's own state, base64 — see [`Instrument::state`].
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub state: String,
}

impl Project {
    /// Read a project back. `path` may name the file or the directory holding
    /// the default one.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let file = if path.is_dir() {
            path.join(DEFAULT_NAME)
        } else {
            path.to_path_buf()
        };
        let text = std::fs::read_to_string(&file)
            .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", file.display()))?;
        serde_yaml::from_str(&text)
            .map_err(|e| anyhow::anyhow!("{} is not a choz project: {e}", file.display()))
    }

    /// Write the project to `path`, or to `path/choz-project.yml` when `path`
    /// is a directory.
    pub fn save(&self, path: &Path) -> anyhow::Result<PathBuf> {
        let file = if path.is_dir() {
            path.join(DEFAULT_NAME)
        } else {
            path.to_path_buf()
        };
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
            automation: crate::automation::Automation::default(),
            version: 1,
            audio: Audio {
                sample_rate: 48_000,
                buffer_size: 256,
                backend: "JACK".into(),
                output_device: Some("default".into()),
                osc_port: Some(9000),
                disabled_midi_inputs: vec!["Midi Through".into()],
            },
            interface: Interface {
                text_color: (240, 180, 90),
                language: "es".into(),
            },
            plugin_paths: choz_engine::PluginPaths::default(),
            rack: vec![Slot {
                input: Some("MIDI:Keystation".into()),
                channel: 3,
                instrument: Instrument {
                    kind: "sf2".into(),
                    path: Some("/usr/share/sounds/sf2/FluidR3_GM.sf2".into()),
                    id: None,
                    name: None,
                    bank: Some(0),
                    preset: Some(4),
                    params: Vec::new(),
                    state: String::new(),
                    bank_dir: None,
                    config: Vec::new(),
                },
                mixer: Mixer {
                    gain: 0.8,
                    gain_r: None,
                    link: None,
                    pan: -0.25,
                    mute: false,
                    solo: false,
                    out_pair: Some((2, 3)),
                    in_pair: None,
                    in_ports: None,
                    pitch_to_midi: false,
                    pitch_mix: None,
                    in_gain: None,
                    in_gate: None,
                },
                fx: vec![Fx {
                    kind: "amberfang".into(),
                    enabled: true,
                    wet: 1.0,
                    params: vec![0.6, 0.4, 0.7, 1.0],
                    plugin_path: None,
                    plugin_id: None,
                    state: String::new(),
                }],
                midi_learn: vec![Binding {
                    source: None,
                    cc: 74,
                    target: crate::LearnTarget::Gain(0),
                    label: "tab 1 \u{00b7} VOL".into(),
                }],
                midi_out: Some("Some Synth:in 0".into()),
                arp: crate::arp::ArpSettings {
                    on: true,
                    mode: crate::arp::ArpMode::UpDown,
                    div: crate::arp::TimeDiv::EighthTriplet,
                    bpm: 96.0,
                    sync: true,
                    gate: 0.4,
                    swing: 0.1,
                    octaves: 2,
                    latch: true,
                    chord: true,
                },
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

        // And what was written loads back identical, by file or by directory.
        assert_eq!(Project::load(&named).unwrap(), sample());
        assert_eq!(Project::load(&dir).unwrap(), sample());

        std::fs::remove_dir_all(&dir).unwrap();
    }
}

/// The plugin state blob as it goes into the YAML file.
///
/// Base64 rather than a binary side-car: a project is one file the user can
/// read, copy and diff, and a patch is a few kilobytes.
pub fn encode_state(data: &[u8]) -> String {
    base64_simd::STANDARD.encode_to_string(data)
}

pub fn decode_state(text: &str) -> Option<Vec<u8>> {
    (!text.is_empty()).then(|| base64_simd::STANDARD.decode_to_vec(text).ok())?
}

#[cfg(test)]
mod state_tests {
    use super::*;

    /// The blob has to survive the round trip through the file exactly: a patch
    /// that comes back one byte short is a plugin that refuses to load it.
    #[test]
    fn a_state_blob_round_trips_through_text() {
        let blob: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
        let text = encode_state(&blob);
        assert!(!text.contains('\n'), "one line, so the YAML stays tidy");
        assert_eq!(decode_state(&text).unwrap(), blob);
        assert_eq!(decode_state(""), None);
        assert_eq!(decode_state("not base64 !!"), None);
    }
}
