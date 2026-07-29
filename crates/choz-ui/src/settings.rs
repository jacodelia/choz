//! choz settings: audio engine, OSC and interface.
//!
//! Same shape as seqterm's AUDIO SETTINGS (Engine / Plugin Paths / OSC), minus
//! the options choz has no engine for. Stored next to the plugin paths in the
//! state dir, so everything survives restarts. The plugin search paths live in
//! their own file (`choz_engine::PluginPaths`) because the engine reads them.

use ratatui::style::Color;

use crate::i18n::Lang;

/// Text colours offered in Settings → Text color.
pub const PALETTE: &[(&str, (u8, u8, u8))] = &[
    ("Default", (220, 226, 240)),
    ("White", (255, 255, 255)),
    ("Amber", (240, 180, 90)),
    ("Green", (120, 220, 150)),
    ("Cyan", (110, 210, 220)),
    ("Blue", (130, 170, 240)),
    ("Magenta", (215, 140, 220)),
    ("Red", (230, 120, 120)),
    ("Grey", (170, 178, 195)),
];

/// Audio-engine settings. `backend` and `sample_rate`/`buffer_size` only take
/// effect on the next start — the stream is built from them at launch.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AudioSettings {
    /// `AUTO`, `JACK`, `PIPEWIRE` or `ALSA`.
    pub backend: String,
    /// Output device name, or empty for the system default.
    pub device: String,
    pub sample_rate: u32,
    pub buffer_size: u32,
    /// SF2 synthesis engine. choz only builds `oxisynth`; kept so a project
    /// file written here means the same thing in seqterm.
    pub sf2_engine: String,
    /// PipeWire quantum override, 0 = leave it to the system.
    pub pipewire_quantum: u32,
    /// ALSA hardware device (`hw:0,0`…), empty = default.
    pub alsa_hw_device: String,
    /// JACK server name, empty = default.
    pub jack_server_name: String,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            backend: "AUTO".into(),
            device: String::new(),
            sample_rate: 48_000,
            buffer_size: 256,
            sf2_engine: "oxisynth".into(),
            pipewire_quantum: 0,
            alsa_hw_device: String::new(),
            jack_server_name: String::new(),
        }
    }
}

impl AudioSettings {
    /// Round-trip latency of one buffer, in milliseconds.
    pub fn latency_ms(&self) -> f32 {
        self.buffer_size as f32 / self.sample_rate.max(1) as f32 * 1000.0
    }
}

/// Sample rates offered by the Engine tab.
pub const SAMPLE_RATES: &[u32] = &[44_100, 48_000, 88_200, 96_000, 176_400, 192_000];
/// Buffer sizes offered by the Engine tab.
pub const BUFFER_SIZES: &[u32] = &[32, 64, 128, 256, 512, 1024, 2048];
/// Backends offered by the Engine tab.
pub const BACKENDS: &[&str] = &["AUTO", "JACK", "PIPEWIRE", "ALSA"];

/// How the OSC listener picks its port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum OscPortMode {
    Specific,
    Random,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OscSettings {
    pub enabled: bool,
    pub port_mode: OscPortMode,
    pub udp_port: u16,
    /// Stored for compatibility with seqterm's settings; choz's server is
    /// UDP-only, so nothing listens on it.
    pub tcp_port: u16,
}

impl Default for OscSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            port_mode: OscPortMode::Specific,
            udp_port: choz_engine::osc::DEFAULT_PORT,
            tcp_port: 9001,
        }
    }
}

impl OscSettings {
    /// The port to bind: the configured one, or 0 to let the OS choose.
    pub fn bind_port(&self) -> u16 {
        match self.port_mode {
            OscPortMode::Specific => self.udp_port,
            OscPortMode::Random => 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UiSettings {
    /// Text colour, as RGB.
    pub text_color: (u8, u8, u8),
    pub language: Lang,
    /// Sections added later; `default` keeps older files loadable.
    #[serde(default)]
    pub audio: AudioSettings,
    #[serde(default)]
    pub osc: OscSettings,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            text_color: PALETTE[0].1,
            language: Lang::from_env(),
            audio: AudioSettings::default(),
            osc: OscSettings::default(),
        }
    }
}

impl UiSettings {
    pub fn color(&self) -> Color {
        let (r, g, b) = self.text_color;
        Color::Rgb(r, g, b)
    }

    /// Index into [`PALETTE`] of the current colour, if it's one of them.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn palette_index(&self) -> Option<usize> {
        PALETTE.iter().position(|(_, rgb)| *rgb == self.text_color)
    }

    fn path() -> std::path::PathBuf {
        choz_engine::cache::state_dir().join("ui.json")
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_string_pretty(self) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    eprintln!("choz: cannot write {}: {e}", path.display());
                }
            }
            Err(e) => eprintln!("choz: cannot serialize the UI settings: {e}"),
        }
    }

    /// Push the settings into the places the drawing code reads them from.
    pub fn apply(&self) {
        crate::i18n::set_language(self.language);
        crate::views::theme::set_text_color(self.color());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file written before the audio/OSC sections existed still loads.
    #[test]
    fn older_settings_files_still_load() {
        let old = r#"{"text_color":[255,255,255],"language":"Es"}"#;
        let s: UiSettings = serde_json::from_str(old).unwrap();
        assert_eq!(s.language, Lang::Es);
        assert_eq!(s.audio, AudioSettings::default(), "missing sections take defaults");
        assert_eq!(s.osc.udp_port, choz_engine::osc::DEFAULT_PORT);
    }

    #[test]
    fn audio_and_osc_settings_have_sane_shapes() {
        let a = AudioSettings::default();
        assert!((a.latency_ms() - 5.333).abs() < 0.01, "256 @ 48k is ~5.3 ms");
        assert!(BACKENDS.contains(&a.backend.as_str()));
        assert!(SAMPLE_RATES.contains(&a.sample_rate));
        assert!(BUFFER_SIZES.contains(&a.buffer_size));

        let mut osc = OscSettings::default();
        assert_eq!(osc.bind_port(), choz_engine::osc::DEFAULT_PORT);
        osc.port_mode = OscPortMode::Random;
        assert_eq!(osc.bind_port(), 0, "random = let the OS pick");
    }

    #[test]
    fn settings_round_trip_and_map_to_the_palette() {
        let s = UiSettings {
            text_color: PALETTE[2].1,
            language: Lang::Ru,
            ..UiSettings::default()
        };
        let back: UiSettings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
        assert_eq!(back.palette_index(), Some(2));
        assert_eq!(back.color(), Color::Rgb(240, 180, 90));
    }
}
