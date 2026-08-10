//! choz settings: audio engine, OSC and interface.
//!
//! Same shape as seqterm's AUDIO SETTINGS (Engine / Plugin Paths / OSC), minus
//! the options choz has no engine for. Stored next to the plugin paths in the
//! state dir, so everything survives restarts. The plugin search paths live in
//! their own file (`choz_engine::PluginPaths`) because the engine reads them.

use ratatui::style::Color;

use crate::i18n::Lang;

/// A named colour scheme: text, frames and desktop, chosen together.
///
/// Same idea as Notepad++'s themes — the point is that the three read well
/// *as a set*, which is why they are picked together instead of one by one.
/// The individual colours stay editable afterwards; a theme is a starting
/// point, not a lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub name: &'static str,
    /// Ordinary interface text.
    pub text: (u8, u8, u8),
    /// Panel and modal frames.
    pub border: (u8, u8, u8),
    /// The desktop behind everything. `None` leaves the terminal's own
    /// background alone, which is what a terminal app should do by default.
    pub desktop: Option<(u8, u8, u8)>,
}

/// Every theme Settings → THEME offers: choz's own, then Gogh's.
pub static THEMES: std::sync::LazyLock<Vec<Theme>> =
    std::sync::LazyLock::new(|| BUILTIN.iter().copied().chain(gogh_themes()).collect());

/// Gogh's terminal colour schemes, trimmed to the three fields a theme here
/// needs. One line per scheme: `name|text RRGGBB|desktop RRGGBB`.
///
/// ponytail: a text table parsed once beats 361 `Theme` literals — same result,
/// 10 KB of data instead of 2000 lines of source, and re-generating it from
/// upstream is a `curl` and a script rather than a diff nobody can read.
const GOGH_DATA: &str = include_str!("gogh_themes.txt");

/// Gogh gives a terminal palette; choz needs a frame colour, which no terminal
/// scheme has. It is the midpoint between the text and the desktop: dimmer than
/// the text so frames don't shout, brighter than the background so they are
/// there at all. Works the same way for a light scheme, where "brighter" means
/// darker.
fn frame_between(text: (u8, u8, u8), desktop: (u8, u8, u8)) -> (u8, u8, u8) {
    let mix = |a: u8, b: u8| ((a as u16 + b as u16) / 2) as u8;
    (
        mix(text.0, desktop.0),
        mix(text.1, desktop.1),
        mix(text.2, desktop.2),
    )
}

fn hex(s: &str) -> Option<(u8, u8, u8)> {
    let s = s.trim();
    if s.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some(((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

/// Parse [`GOGH_DATA`]. A line that doesn't parse is skipped rather than fatal:
/// the table is data, and one bad row must not cost the user every theme.
fn gogh_themes() -> impl Iterator<Item = Theme> {
    GOGH_DATA
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .filter_map(|line| {
            let mut parts = line.split('|');
            let name = parts.next()?.trim();
            let text = hex(parts.next()?)?;
            let desktop = hex(parts.next()?)?;
            (!name.is_empty()).then_some(Theme {
                name,
                text,
                border: frame_between(text, desktop),
                desktop: Some(desktop),
            })
        })
}

/// choz's own themes, shown first: the default look plus the classic editor
/// schemes, with the desktop colour taken from the scheme's own editor
/// background.
const BUILTIN: &[Theme] = &[
    Theme {
        name: "choz (default)",
        text: (220, 226, 240),
        border: (99, 101, 108),
        desktop: None,
    },
    Theme {
        name: "Obsidian",
        text: (224, 226, 228),
        border: (91, 106, 114),
        desktop: Some((41, 49, 52)),
    },
    Theme {
        name: "Zenburn",
        text: (220, 220, 204),
        border: (110, 110, 100),
        desktop: Some((63, 63, 63)),
    },
    Theme {
        name: "Solarized Dark",
        text: (147, 161, 161),
        border: (88, 110, 117),
        desktop: Some((0, 43, 54)),
    },
    Theme {
        name: "Solarized Light",
        text: (88, 110, 117),
        border: (147, 161, 161),
        desktop: Some((253, 246, 227)),
    },
    Theme {
        name: "Monokai",
        text: (248, 248, 242),
        border: (117, 113, 94),
        desktop: Some((39, 40, 34)),
    },
    Theme {
        name: "Deep Black",
        text: (200, 200, 200),
        border: (70, 70, 70),
        desktop: Some((0, 0, 0)),
    },
    Theme {
        name: "Vibrant Ink",
        text: (255, 255, 255),
        border: (102, 102, 102),
        desktop: Some((20, 20, 20)),
    },
    Theme {
        name: "Ruby Blue",
        text: (255, 255, 255),
        border: (86, 118, 152),
        desktop: Some((17, 34, 51)),
    },
    Theme {
        name: "Bespin",
        text: (186, 174, 156),
        border: (124, 112, 100),
        desktop: Some((40, 33, 30)),
    },
    Theme {
        name: "Hello Kitty",
        text: (60, 40, 50),
        border: (200, 140, 170),
        desktop: Some((255, 228, 240)),
    },
];

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
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
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
    /// Tempo of choz's own transport, in BPM — what a tempo-synced plugin reads
    /// on every block. Added later, hence the default.
    #[serde(default = "default_bpm")]
    pub bpm: f32,
    /// Beats per bar and the note that gets the beat. Added later, hence the
    /// default.
    #[serde(default = "default_time_sig")]
    pub time_sig: (u16, u16),
}

fn default_time_sig() -> (u16, u16) {
    (4, 4)
}

fn default_bpm() -> f32 {
    choz_ports::Transport::DEFAULT_BPM
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
            bpm: default_bpm(),
            time_sig: default_time_sig(),
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

/// What choz is being used as.
///
/// The two jobs are genuinely different and pull the routing in opposite
/// directions, so they are a switch rather than a guess:
///
/// - **LIVE**: one instrument sounds at a time. Tabs are the songs or the
///   patches of a set, and a controller's buttons (program changes) step
///   through them. Several tabs can share one port — they are alternatives,
///   not layers.
/// - **MULTI**: every tab sounds at once, each answering its own **MIDI
///   channel**, the way a sampler answers a DAW's orchestral template. This is
///   the mode for driving choz from Reaper.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum RackMode {
    #[default]
    Live,
    Multi,
}

impl RackMode {
    pub fn label(self) -> &'static str {
        match self {
            RackMode::Live => "LIVE",
            RackMode::Multi => "MULTI",
        }
    }

    pub fn next(self) -> Self {
        match self {
            RackMode::Live => RackMode::Multi,
            RackMode::Multi => RackMode::Live,
        }
    }
}

/// How a background image fills the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ImageFit {
    /// Scale the image to cover the whole screen.
    #[default]
    Stretch,
    /// Repeat the image at its own aspect ratio.
    Tile,
}

impl ImageFit {
    pub fn label(self) -> &'static str {
        match self {
            ImageFit::Stretch => "STRETCH",
            ImageFit::Tile => "TILE",
        }
    }

    pub fn next(self) -> Self {
        match self {
            ImageFit::Stretch => ImageFit::Tile,
            ImageFit::Tile => ImageFit::Stretch,
        }
    }
}

/// What sits behind the whole UI.
///
/// The terminal's own background is the default: choz paints nothing and the
/// user's transparency, theme or wallpaper shows through, which is what a
/// terminal app should do unless asked otherwise.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum Background {
    #[default]
    Terminal,
    /// A flat colour under everything.
    Color((u8, u8, u8)),
    /// An image, rendered as per-cell background colours so it works on any
    /// terminal — no sixel or kitty protocol needed.
    Image {
        path: std::path::PathBuf,
        #[serde(default)]
        fit: ImageFit,
    },
}

impl Background {
    pub fn label(&self) -> String {
        match self {
            Background::Terminal => "terminal default".to_string(),
            Background::Color((r, g, b)) => format!("colour rgb({r},{g},{b})"),
            Background::Image { path, fit } => format!(
                "{}  [{}]",
                path.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                fit.label()
            ),
        }
    }
}

// No `Eq`: the tempo is an `f32`. Nothing compares settings for exact equality
// beyond the round-trip tests, which `PartialEq` covers.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UiSettings {
    /// Text colour, as RGB.
    pub text_color: (u8, u8, u8),
    pub language: Lang,
    /// Sections added later; `default` keeps older files loadable.
    #[serde(default)]
    pub audio: AudioSettings,
    #[serde(default)]
    pub osc: OscSettings,
    #[serde(default)]
    pub background: Background,
    /// Frame colour. Older files have none, and used to derive it by dimming the
    /// text colour — [`UiSettings::border`] keeps doing that when it is absent,
    /// so an old `ui.json` still looks the way it did.
    #[serde(default)]
    pub border_color: Option<(u8, u8, u8)>,
    /// Name of the theme the colours came from, for the UI to show which row is
    /// active. Editing a colour afterwards just leaves it stale, which is why
    /// the drawing code never reads it.
    #[serde(default)]
    pub theme_name: String,
    /// How strongly the theme's panel colour is washed over a background image,
    /// 0..100 %. A photo behind the UI is beautiful and unreadable; this is the
    /// knob that trades one for the other.
    ///
    /// It is blended **into the image**, not drawn as a layer, because a
    /// terminal cell background has no alpha: the only place a partial colour
    /// can exist is in the picture itself.
    #[serde(default = "default_tint")]
    pub background_tint: u8,
    /// The colour the panels are washed with. `None` follows the active
    /// scheme's own desktop colour, which is what keeps a tinted UI looking
    /// like the theme rather than like a filter over it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panel_tint: Option<(u8, u8, u8)>,
    /// Live rig or multi-timbral module. Saved, because it is a property of how
    /// this machine is set up, not of a single session.
    #[serde(default)]
    pub rack_mode: RackMode,
}

/// Enough wash to read knobs and labels over a busy photo, without hiding it.
fn default_tint() -> u8 {
    45
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            text_color: PALETTE[0].1,
            language: Lang::from_env(),
            audio: AudioSettings::default(),
            osc: OscSettings::default(),
            background: Background::default(),
            border_color: None,
            theme_name: THEMES[0].name.to_string(),
            background_tint: default_tint(),
            panel_tint: None,
            rack_mode: RackMode::default(),
        }
    }
}

impl UiSettings {
    /// The colour washed over a background image, and how strongly (0..1).
    ///
    /// It is the active theme's desktop colour — the one the scheme already
    /// uses behind everything — so a tinted photo reads as the same theme
    /// rather than as a grey filter. Falls back to a neutral dark when the
    /// scheme paints no desktop of its own.
    pub fn tint(&self) -> ((u8, u8, u8), f32) {
        let rgb = self.panel_tint.unwrap_or_else(|| {
            THEMES
                .iter()
                .find(|t| t.name == self.theme_name)
                .and_then(|t| t.desktop)
                .unwrap_or((16, 18, 24))
        });
        (rgb, self.background_tint.min(100) as f32 / 100.0)
    }

    /// Name of the panel colour for the settings row: the scheme's own, or the
    /// palette entry the user picked.
    pub fn panel_tint_label(&self) -> String {
        match self.panel_tint {
            None => "theme's own".to_string(),
            Some(rgb) => PALETTE
                .iter()
                .find(|(_, c)| *c == rgb)
                .map(|(n, _)| (*n).to_string())
                .unwrap_or_else(|| format!("rgb({},{},{})", rgb.0, rgb.1, rgb.2)),
        }
    }

    /// Step through "the theme's own" and then the palette, wrapping. One row,
    /// one key — the colour is a taste decision made by looking at it.
    pub fn step_panel_tint(&mut self, delta: i32) {
        let len = PALETTE.len() as i32 + 1;
        let now = match self.panel_tint {
            None => 0,
            Some(rgb) => PALETTE
                .iter()
                .position(|(_, c)| *c == rgb)
                .map(|i| i as i32 + 1)
                .unwrap_or(0),
        };
        let next = (now + delta).rem_euclid(len);
        self.panel_tint = match next {
            0 => None,
            i => Some(PALETTE[(i - 1) as usize].1),
        };
    }

    pub fn color(&self) -> Color {
        let (r, g, b) = self.text_color;
        Color::Rgb(r, g, b)
    }

    /// The frame colour: the stored one, or the historical fallback of the text
    /// colour at 45% brightness.
    pub fn border(&self) -> Color {
        match self.border_color {
            Some((r, g, b)) => Color::Rgb(r, g, b),
            None => {
                let (r, g, b) = self.text_color;
                let dim = |c: u8| ((c as u32 * 45) / 100) as u8;
                Color::Rgb(dim(r), dim(g), dim(b))
            }
        }
    }

    /// Apply a whole theme: text, frames and desktop together.
    ///
    /// A theme with no desktop colour clears an inherited *colour* background
    /// but leaves an image alone — someone who picked a wallpaper did not ask
    /// for it to vanish because they changed the text scheme.
    pub fn apply_theme(&mut self, theme: &Theme) {
        self.text_color = theme.text;
        self.border_color = Some(theme.border);
        self.theme_name = theme.name.to_string();
        match (theme.desktop, &self.background) {
            (Some(rgb), Background::Image { .. }) => {
                // Keep the picture; the colour would not be visible anyway.
                let _ = rgb;
            }
            (Some(rgb), _) => self.background = Background::Color(rgb),
            (None, Background::Image { .. }) => {}
            (None, _) => self.background = Background::Terminal,
        }
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
        choz_ports::transport().set_bpm(self.audio.bpm);
        choz_ports::transport().set_time_signature(self.audio.time_sig.0, self.audio.time_sig.1);
        crate::i18n::set_language(self.language);
        crate::views::theme::set_text_color(self.color());
        crate::views::theme::set_border_color(self.border());
        crate::views::theme::set_has_desktop(self.background != Background::Terminal);
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
        assert_eq!(
            s.audio,
            AudioSettings::default(),
            "missing sections take defaults"
        );
        assert_eq!(s.osc.udp_port, choz_engine::osc::DEFAULT_PORT);
    }

    #[test]
    fn audio_and_osc_settings_have_sane_shapes() {
        let a = AudioSettings::default();
        assert!(
            (a.latency_ms() - 5.333).abs() < 0.01,
            "256 @ 48k is ~5.3 ms"
        );
        assert!(BACKENDS.contains(&a.backend.as_str()));
        assert!(SAMPLE_RATES.contains(&a.sample_rate));
        assert!(BUFFER_SIZES.contains(&a.buffer_size));

        let mut osc = OscSettings::default();
        assert_eq!(osc.bind_port(), choz_engine::osc::DEFAULT_PORT);
        osc.port_mode = OscPortMode::Random;
        assert_eq!(osc.bind_port(), 0, "random = let the OS pick");
    }

    /// The Gogh table is data shipped inside the binary: a line that stopped
    /// parsing would silently cost the user a theme, and a colour read out of
    /// order would cost them a readable interface.
    #[test]
    fn the_gogh_table_parses_into_usable_themes() {
        let gogh: Vec<Theme> = gogh_themes().collect();
        assert!(
            gogh.len() > 300,
            "the upstream table has hundreds; got {}",
            gogh.len()
        );
        assert_eq!(
            THEMES.len(),
            BUILTIN.len() + gogh.len(),
            "choz's own come first"
        );
        assert_eq!(THEMES[0].name, BUILTIN[0].name);

        // A known scheme, byte for byte from `data/themes.json`.
        let gruvbox = gogh
            .iter()
            .find(|t| t.name == "Gruvbox Dark")
            .expect("a famous one");
        assert_eq!(gruvbox.text, (0xEB, 0xDB, 0xB2));
        assert_eq!(gruvbox.desktop, Some((0x28, 0x28, 0x28)));
        // The frame sits between the two, so it reads against both.
        assert_eq!(gruvbox.border, (0x89, 0x81, 0x6D));

        // Nothing empty, nothing duplicated: the picker shows names, and two
        // rows with the same one cannot be told apart.
        let mut names: Vec<&str> = THEMES.iter().map(|t| t.name).collect();
        assert!(names.iter().all(|n| !n.trim().is_empty()));
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(names.len(), before, "duplicate theme names");
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
