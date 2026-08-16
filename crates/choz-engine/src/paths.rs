//! Plugin search paths and the multi-format scan that walks them.
//!
//! choz looks in the same places Carla does by default (plus the usual
//! environment variables), and the user can edit the list per format in
//! Settings → Plugin paths. The result is persisted as JSON next to the scan
//! cache, so an edited list survives restarts.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A plugin/soundbank format choz knows how to look for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum PluginFormat {
    Ladspa,
    Dssi,
    Lv2,
    Vst2,
    Vst3,
    Clap,
    Sf2,
    Sfz,
    /// Pure Data patches. Not a binary plugin: a text file Pd runs, hosted in
    /// a process of its own because libpd allows exactly one Pd per process.
    Pd,
}

impl PluginFormat {
    /// Every format, in the order the settings modal lists them.
    pub const ALL: &'static [PluginFormat] = &[
        PluginFormat::Ladspa,
        PluginFormat::Dssi,
        PluginFormat::Lv2,
        PluginFormat::Vst2,
        PluginFormat::Vst3,
        PluginFormat::Clap,
        PluginFormat::Sf2,
        PluginFormat::Sfz,
        PluginFormat::Pd,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PluginFormat::Ladspa => "LADSPA",
            PluginFormat::Dssi => "DSSI",
            PluginFormat::Lv2 => "LV2",
            PluginFormat::Vst2 => "VST2",
            PluginFormat::Vst3 => "VST3",
            PluginFormat::Clap => "CLAP",
            PluginFormat::Sf2 => "SF2",
            PluginFormat::Sfz => "SFZ",
            PluginFormat::Pd => "PD",
        }
    }

    pub fn from_label(s: &str) -> Option<PluginFormat> {
        PluginFormat::ALL
            .iter()
            .copied()
            .find(|f| f.label().eq_ignore_ascii_case(s))
    }

    /// Environment variable that overrides this format's search path, if the
    /// format has the conventional one.
    fn env_var(self) -> Option<&'static str> {
        match self {
            PluginFormat::Ladspa => Some("LADSPA_PATH"),
            PluginFormat::Dssi => Some("DSSI_PATH"),
            PluginFormat::Lv2 => Some("LV2_PATH"),
            PluginFormat::Vst2 => Some("VST_PATH"),
            PluginFormat::Vst3 => Some("VST3_PATH"),
            PluginFormat::Clap => Some("CLAP_PATH"),
            PluginFormat::Sf2 => Some("SF2_PATH"),
            PluginFormat::Sfz => Some("SFZ_PATH"),
            PluginFormat::Pd => Some("PD_PATH"),
        }
    }

    /// What a plugin of this format looks like on disk: file extensions, and
    /// whether an entry is a bundle *directory* rather than a file.
    fn matcher(self) -> (&'static [&'static str], bool) {
        match self {
            PluginFormat::Ladspa | PluginFormat::Dssi | PluginFormat::Vst2 => (&["so"], false),
            PluginFormat::Lv2 => (&["lv2"], true),
            PluginFormat::Vst3 => (&["vst3"], true),
            PluginFormat::Clap => (&["clap"], false),
            PluginFormat::Sf2 => (&["sf2", "sf3"], false),
            PluginFormat::Sfz => (&["sfz"], false),
            PluginFormat::Pd => (&["pd"], false),
        }
    }

    /// True for real plugin formats — SF2/SFZ are soundbanks, loaded as files.
    pub fn is_plugin(self) -> bool {
        !matches!(self, PluginFormat::Sf2 | PluginFormat::Sfz)
    }

    /// True for formats choz can actually load today.
    pub fn is_hosted(self) -> bool {
        matches!(
            self,
            PluginFormat::Clap
                | PluginFormat::Lv2
                | PluginFormat::Ladspa
                | PluginFormat::Dssi
                | PluginFormat::Vst2
                | PluginFormat::Vst3
                | PluginFormat::Sf2
                | PluginFormat::Sfz
                | PluginFormat::Pd
        )
    }

    /// Carla-style defaults for this format, plus `$FORMAT_PATH` when set.
    pub fn default_dirs(self) -> Vec<PathBuf> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let user = |rel: &str| home.as_ref().map(|h| h.join(rel));
        let sys = |rel: &str| {
            ["/usr/lib", "/usr/local/lib", "/usr/lib64"]
                .iter()
                .map(|p| PathBuf::from(p).join(rel))
                .collect::<Vec<_>>()
        };

        let mut dirs: Vec<PathBuf> = match self {
            PluginFormat::Ladspa => sys("ladspa"),
            PluginFormat::Dssi => sys("dssi"),
            PluginFormat::Lv2 => sys("lv2"),
            PluginFormat::Vst2 => sys("vst").into_iter().chain(sys("lxvst")).collect(),
            PluginFormat::Vst3 => sys("vst3"),
            PluginFormat::Clap => sys("clap"),
            PluginFormat::Sf2 => vec![
                PathBuf::from("/usr/share/sounds/sf2"),
                PathBuf::from("/usr/share/sounds/sf3"),
                PathBuf::from("/usr/share/soundfonts"),
            ],
            PluginFormat::Sfz => vec![PathBuf::from("/usr/share/sounds/sfz")],
            // Pd has no installed-patch convention the way plugins do: a patch
            // is a document. These are where people keep them.
            PluginFormat::Pd => vec![PathBuf::from("/usr/share/pd/patches")],
        };
        let user_dirs: Vec<Option<PathBuf>> = match self {
            PluginFormat::Ladspa => vec![user(".ladspa")],
            PluginFormat::Dssi => vec![user(".dssi")],
            PluginFormat::Lv2 => vec![user(".lv2")],
            PluginFormat::Vst2 => vec![user(".vst"), user(".lxvst")],
            PluginFormat::Vst3 => vec![user(".vst3")],
            PluginFormat::Clap => vec![user(".clap")],
            PluginFormat::Sf2 => vec![user(".local/share/sounds/sf2"), user(".sounds")],
            PluginFormat::Sfz => vec![user(".sfz")],
            PluginFormat::Pd => vec![user("pd"), user(".local/share/pd"), user("Documents/Pd")],
        };
        dirs.extend(user_dirs.into_iter().flatten());

        // The environment wins over the built-in list, as in every other host.
        if let Some(var) = self.env_var() {
            if let Some(val) = std::env::var_os(var) {
                let env_dirs: Vec<PathBuf> = std::env::split_paths(&val).collect();
                if !env_dirs.is_empty() {
                    dirs = env_dirs;
                }
            }
        }
        dirs.sort();
        dirs.dedup();
        dirs
    }
}

/// One search directory and whether it's scanned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchDir {
    pub path: PathBuf,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

/// The whole per-format search-path configuration.
///
/// Deserialising goes through [`StoredPaths`], which drops entries naming a
/// format this build does not know instead of failing the whole file. Without
/// that, a config written by a build with one more format than this one would
/// fail to parse as a whole and silently reset the user's hand-edited
/// directories back to `Default`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(into = "StoredPaths")]
pub struct PluginPaths {
    pub entries: Vec<(PluginFormat, Vec<SearchDir>)>,
}

/// On-disk shape: the format is its label, not the enum variant name, so an
/// unknown one is just a string to skip. `from_label` is case-insensitive, which
/// is what makes files written before this change (`"Ladspa"`) still load.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredPaths {
    entries: Vec<(String, Vec<SearchDir>)>,
}

impl From<PluginPaths> for StoredPaths {
    fn from(p: PluginPaths) -> Self {
        Self {
            entries: p
                .entries
                .into_iter()
                .map(|(f, dirs)| (f.label().to_string(), dirs))
                .collect(),
        }
    }
}

impl<'de> Deserialize<'de> for PluginPaths {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let stored = StoredPaths::deserialize(d)?;
        let mut entries: Vec<(PluginFormat, Vec<SearchDir>)> = stored
            .entries
            .into_iter()
            .filter_map(|(label, dirs)| Some((PluginFormat::from_label(&label)?, dirs)))
            .collect();
        // **A format the file has never heard of gets its defaults**, not an
        // empty list.
        //
        // Every `plugin-paths.json` predates whichever format was added last —
        // and until this, that format arrived with nowhere to look, which reads
        // as "choz cannot find my patches" and is answered by typing a path by
        // hand. Found exactly that way: a `PD` section that did not exist,
        // because the file was written before Pure Data was a format.
        //
        // Only the formats the file does *not* mention are filled in: a user
        // who has edited a list, including down to nothing, has said something
        // and it stands.
        let defaults = PluginPaths::default();
        for (format, dirs) in defaults.entries {
            if !entries.iter().any(|(f, _)| *f == format) {
                entries.push((format, dirs));
            }
        }
        entries.sort_by_key(|(f, _)| *f);
        Ok(PluginPaths { entries })
    }
}

impl Default for PluginPaths {
    fn default() -> Self {
        Self {
            entries: PluginFormat::ALL
                .iter()
                .map(|&f| {
                    let dirs = f
                        .default_dirs()
                        .into_iter()
                        .map(|path| SearchDir {
                            path,
                            enabled: true,
                        })
                        .collect();
                    (f, dirs)
                })
                .collect(),
        }
    }
}

impl PluginPaths {
    pub fn dirs(&self, format: PluginFormat) -> &[SearchDir] {
        self.entries
            .iter()
            .find(|(f, _)| *f == format)
            .map(|(_, d)| d.as_slice())
            .unwrap_or(&[])
    }

    pub fn dirs_mut(&mut self, format: PluginFormat) -> &mut Vec<SearchDir> {
        if !self.entries.iter().any(|(f, _)| *f == format) {
            self.entries.push((format, Vec::new()));
        }
        self.entries
            .iter_mut()
            .find(|(f, _)| *f == format)
            .map(|(_, d)| d)
            .expect("just inserted")
    }

    /// Every enabled directory of every format, for cache-freshness checks.
    pub fn all_enabled(&self) -> Vec<PathBuf> {
        self.entries
            .iter()
            .flat_map(|(_, dirs)| dirs.iter().filter(|d| d.enabled).map(|d| d.path.clone()))
            .collect()
    }

    pub fn load() -> Self {
        let path = config_path();
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Where the config lives. The scan cache watches it, so editing the paths
    /// invalidates a cache written before the edit.
    pub fn config_file() -> PathBuf {
        config_path()
    }

    pub fn save(&self) {
        let path = config_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_string_pretty(self) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    eprintln!("choz: cannot write {}: {e}", path.display());
                }
            }
            Err(e) => eprintln!("choz: cannot serialize plugin paths: {e}"),
        }
    }
}

fn config_path() -> PathBuf {
    crate::cache::state_dir().join("plugin-paths.json")
}

/// One plugin/soundbank found on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoundPlugin {
    pub format: PluginFormat,
    /// Display name (plugin descriptor name for CLAP, file stem otherwise).
    pub name: String,
    pub path: PathBuf,
    /// Plugin id within the file; empty for formats where the file *is* the
    /// plugin (SF2, SFZ).
    pub id: String,
    /// True when it makes sound on its own (instrument/soundbank) rather than
    /// processing audio.
    pub is_instrument: bool,
}

/// Files of `format` under `dir` (recursing, but never descending into a
/// bundle). Bundle formats (LV2, VST3) yield the bundle directory itself.
pub fn scan_dir(dir: &Path, format: PluginFormat) -> Vec<FoundPlugin> {
    let (exts, bundles) = format.matcher();
    let mut out = Vec::new();
    scan_into(dir, format, exts, bundles, 0, &mut out);
    if format == PluginFormat::Pd {
        out.retain(pd_effect);
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out.dedup_by(|a, b| a.path == b.path);
    out
}

/// A `.pd` file is a document, not a plugin: half of them are a GUI, a helper
/// or an abstraction with nothing to connect audio to. The file says which it
/// is — it is text — so the scan reads it instead of listing patches that
/// cannot be an effect.
fn pd_effect(found: &FoundPlugin) -> bool {
    match choz_plugin_pd::read_patch(&found.path) {
        Ok(info) => info.role() == choz_plugin_pd::PatchRole::Effect,
        Err(e) => {
            eprintln!("choz: cannot read {}: {e}", found.path.display());
            false
        }
    }
}

/// The single-path counterpart of [`scan_dir`]: describe `path` itself when it
/// is a file (or bundle) of this format, instead of looking inside it. Used by
/// the per-entry retry after a scan crash.
pub fn scan_path(path: &Path, format: PluginFormat) -> Vec<FoundPlugin> {
    let (exts, bundles) = format.matcher();
    let matches = path
        .extension()
        .is_some_and(|e| exts.iter().any(|x| e.eq_ignore_ascii_case(x)));
    if matches && path.is_dir() == bundles {
        return vec![FoundPlugin {
            format,
            name: stem(path),
            path: path.to_path_buf(),
            id: String::new(),
            is_instrument: matches!(format, PluginFormat::Sf2 | PluginFormat::Sfz),
        }];
    }
    scan_dir(path, format)
}

fn scan_into(
    dir: &Path,
    format: PluginFormat,
    exts: &[&str],
    bundles: bool,
    depth: usize,
    out: &mut Vec<FoundPlugin>,
) {
    // Deep trees exist (VST collections); stop before they cost real time.
    if depth > 4 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let matches = path
            .extension()
            .is_some_and(|e| exts.iter().any(|x| e.eq_ignore_ascii_case(x)));
        let is_dir = path.is_dir();
        if matches && (is_dir == bundles) {
            out.push(FoundPlugin {
                format,
                name: stem(&path),
                path,
                id: String::new(),
                is_instrument: matches!(format, PluginFormat::Sf2 | PluginFormat::Sfz),
            });
        } else if is_dir {
            scan_into(&path, format, exts, bundles, depth + 1, out);
        }
    }
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

/// Formats whose files sit directly in `dir`, with how many of each. Used to
/// tell the user "this directory holds 73 SF2 files" when they added it under
/// the wrong format and nothing showed up.
pub fn formats_present(dir: &Path) -> Vec<(PluginFormat, usize)> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut counts: Vec<(PluginFormat, usize)> = Vec::new();
    for entry in rd.flatten() {
        let path = entry.path();
        let Some(ext) = path.extension() else {
            continue;
        };
        for &fmt in PluginFormat::ALL {
            let (exts, bundles) = fmt.matcher();
            if path.is_dir() != bundles {
                continue;
            }
            if exts.iter().any(|x| ext.eq_ignore_ascii_case(x)) {
                match counts.iter_mut().find(|(f, _)| *f == fmt) {
                    Some((_, n)) => *n += 1,
                    None => counts.push((fmt, 1)),
                }
            }
        }
    }
    counts.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    counts
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory of `.pd` files is a directory of documents: only the ones
    /// that actually process audio are effects. The file says so in text, so
    /// this holds on a machine with no Pure Data at all.
    #[test]
    fn only_the_patches_that_process_audio_are_listed_as_effects() {
        let dir = std::env::temp_dir().join("choz-paths-pd");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("gain.pd"),
            "#N canvas 0 0 1 1 12;\n#X obj 0 0 adc~;\n#X obj 0 1 dac~;\n",
        )
        .unwrap();
        // Notes out: an input algorithm, not an effect — it belongs next to the
        // arpeggiator, and offering it here would be offering silence.
        std::fs::write(
            dir.join("arp.pd"),
            "#N canvas 0 0 1 1 12;\n#X obj 0 0 notein;\n#X obj 0 1 noteout;\n",
        )
        .unwrap();
        // Neither: nothing to wire it to.
        std::fs::write(
            dir.join("gui.pd"),
            "#N canvas 0 0 1 1 12;\n#X obj 0 0 bng 15 250 50 0;\n",
        )
        .unwrap();

        let found = scan_dir(&dir, PluginFormat::Pd);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].name, "gain");
        assert!(!found[0].is_instrument, "a patch is an effect, not a synth");
        let _ = std::fs::remove_dir_all(&dir);
    }


    /// A saved file written before a format existed still gets that format's
    /// default directories.
    ///
    /// Every `plugin-paths.json` predates whichever format was added last. With
    /// an empty list that format simply never finds anything, and the only
    /// clue is the absence of results — which is how "I added my Pd folder and
    /// it did not work" started: there was no `PD` section to add it to, and
    /// the path went in by hand with a typo in it.
    #[test]
    fn a_format_the_saved_file_never_heard_of_gets_its_defaults() {
        // A file from before Pure Data was a format: it knows LV2 and nothing
        // else, and its LV2 list is the user's own.
        let json = r#"{"entries":[["LV2",[{"path":"/opt/my-lv2","enabled":true}]]]}"#;
        let paths: PluginPaths = serde_json::from_str(json).unwrap();

        // What the user said stands, exactly as they said it.
        let lv2 = paths.dirs(PluginFormat::Lv2);
        assert_eq!(lv2.len(), 1);
        assert_eq!(lv2[0].path, Path::new("/opt/my-lv2"));

        // And everything the file never mentioned has somewhere to look.
        for &format in PluginFormat::ALL {
            assert!(
                !paths.dirs(format).is_empty(),
                "{} would search nowhere",
                format.label()
            );
        }
        assert!(paths
            .dirs(PluginFormat::Pd)
            .iter()
            .any(|d| d.path.ends_with("pd")));
    }

    #[test]
    fn defaults_cover_the_carla_locations() {
        let dirs = PluginFormat::Lv2.default_dirs();
        assert!(
            dirs.iter().any(|d| d == Path::new("/usr/lib/lv2")),
            "the system LV2 dir must be searched: {dirs:?}"
        );
        let clap = PluginFormat::Clap.default_dirs();
        assert!(clap.iter().any(|d| d == Path::new("/usr/lib/clap")));
    }

    /// A config written by a build that knew one more format must still load,
    /// keeping every directory the user edited by hand. Dropping the whole file
    /// would silently reset their paths to the defaults.
    ///
    /// The formats it *does* know are filled in from the defaults — see
    /// [`a_format_the_saved_file_never_heard_of_gets_its_defaults`] — so what
    /// this checks is that the unknown one leaves no trace and the edited ones
    /// come through untouched.
    #[test]
    fn an_unknown_format_is_skipped_instead_of_failing_the_file() {
        let json = r#"{"entries":[
            ["Lv2",[{"path":"/home/me/.lv2","enabled":true}]],
            ["Fictional",[{"path":"/usr/share/fictional","enabled":true}]],
            ["Vst2",[{"path":"/home/me/repo","enabled":true}]]
        ]}"#;
        let cfg: PluginPaths = serde_json::from_str(json).expect("unknown format must not fail");
        assert_eq!(
            cfg.entries.len(),
            PluginFormat::ALL.len(),
            "the unknown one is dropped and the known ones are all there"
        );
        assert_eq!(
            cfg.dirs(PluginFormat::Vst2)[0].path,
            PathBuf::from("/home/me/repo")
        );
        assert_eq!(
            cfg.dirs(PluginFormat::Lv2)[0].path,
            PathBuf::from("/home/me/.lv2")
        );
    }

    #[test]
    fn config_round_trips_and_keeps_edits() {
        let mut cfg = PluginPaths::default();
        cfg.dirs_mut(PluginFormat::Vst2).push(SearchDir {
            path: "/opt/plugins".into(),
            enabled: false,
        });
        let json = serde_json::to_string(&cfg).unwrap();
        let back: PluginPaths = serde_json::from_str(&json).unwrap();
        assert_eq!(back, cfg);
        let vst2 = back.dirs(PluginFormat::Vst2);
        assert!(vst2
            .iter()
            .any(|d| d.path == Path::new("/opt/plugins") && !d.enabled));
        // A disabled dir is not offered to the scanner.
        assert!(!back.all_enabled().contains(&PathBuf::from("/opt/plugins")));
    }

    /// A directory added under the wrong format still reports what it holds.
    #[test]
    fn formats_present_reports_what_a_directory_holds() {
        let tmp = std::env::temp_dir().join(format!("choz_fmt_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("a.sf2"), b"x").unwrap();
        std::fs::write(tmp.join("B.SF2"), b"x").unwrap();
        std::fs::write(tmp.join("c.sfz"), b"x").unwrap();
        std::fs::write(tmp.join("readme.txt"), b"x").unwrap();

        let found = formats_present(&tmp);
        assert_eq!(
            found[0],
            (PluginFormat::Sf2, 2),
            "case-insensitive: {found:?}"
        );
        assert!(found.contains(&(PluginFormat::Sfz, 1)));
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn scan_finds_files_and_bundles() {
        let tmp = std::env::temp_dir().join(format!("choz_paths_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("nested")).unwrap();
        std::fs::write(tmp.join("nested/Piano.sf2"), b"x").unwrap();
        std::fs::write(tmp.join("notes.txt"), b"x").unwrap();
        std::fs::create_dir_all(tmp.join("Amp.lv2")).unwrap();
        std::fs::write(tmp.join("Amp.lv2/manifest.ttl"), b"x").unwrap();

        let sf2 = scan_dir(&tmp, PluginFormat::Sf2);
        assert_eq!(sf2.len(), 1, "recurses into subdirectories: {sf2:?}");
        assert_eq!(sf2[0].name, "Piano");
        assert!(sf2[0].is_instrument);

        let lv2 = scan_dir(&tmp, PluginFormat::Lv2);
        assert_eq!(lv2.len(), 1, "a .lv2 bundle is one entry, not its contents");
        assert_eq!(lv2[0].name, "Amp");
        assert!(lv2[0].path.is_dir());

        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
