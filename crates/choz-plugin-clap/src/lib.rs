//! CLAP (CLever Audio Plugin) hosting for choz.
//!
//! With the `clap` feature, real hosting is provided via the safe `clack-host`
//! crate (see [`host`]): factory-accurate discovery and live instrument audio
//! (note events → stereo output). Without the feature, scanning falls back to
//! filename-only metadata and instantiation returns `None`.
//!
//! Ported (simplified: single MIDI channel, no MPE/expression/state) from
//! seqterm's `seqterm-plugin-clap`.

use std::path::{Path, PathBuf};

/// Metadata about a discovered CLAP plugin.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ClapPluginInfo {
    /// Absolute path to the `.clap` shared library.
    pub path: PathBuf,
    /// Plugin name from the descriptor.
    pub name: String,
    /// Vendor string from the descriptor.
    pub vendor: String,
    /// Unique plugin ID (reverse-DNS style, e.g. `com.example.my-plugin`).
    pub id: String,
    /// True if the plugin declares the `instrument` CLAP feature.
    pub is_instrument: bool,
}

pub use choz_ports::PluginParam;

/// Parameters of the plugin at `path`.
pub fn read_params(path: &Path, plugin_id: &str) -> Vec<PluginParam> {
    host::read_params(path, plugin_id)
}

pub mod editor;
pub mod host;
pub mod state;

/// Scan a directory tree for `.clap` plugins.
pub fn scan_directory(dir: &Path) -> Vec<ClapPluginInfo> {
    let mut found = Vec::new();
    scan_recursive(dir, &mut found);
    found
}

fn scan_recursive(dir: &Path, out: &mut Vec<ClapPluginInfo>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_file() && path.extension().is_some_and(|e| e == "clap") {
            out.extend(describe(&path));
        } else if path.is_dir() {
            scan_recursive(&path, out);
        }
    }
}

/// Describe every plugin inside a `.clap` file: the real factory when the
/// library loads, otherwise a single filename-derived entry.
pub fn describe(path: &Path) -> Vec<ClapPluginInfo> {
    let real = host::read_descriptors(path);
    if !real.is_empty() {
        return real;
    }
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Unknown".into());
    vec![ClapPluginInfo {
        path: path.to_path_buf(),
        name,
        vendor: String::new(),
        id: path.to_string_lossy().into_owned(),
        is_instrument: true,
    }]
}

/// Default CLAP search paths for the current platform.
pub fn default_search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    #[cfg(target_os = "linux")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            paths.push(PathBuf::from(home).join(".clap"));
        }
        paths.push(PathBuf::from("/usr/lib/clap"));
        paths.push(PathBuf::from("/usr/local/lib/clap"));
    }
    #[cfg(target_os = "macos")]
    if let Some(home) = std::env::var_os("HOME") {
        paths.push(PathBuf::from(home).join("Library/Audio/Plug-Ins/CLAP"));
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_finds_clap_files_by_extension() {
        let base = std::env::temp_dir().join(format!("choz_clap_{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("Synth.clap"), b"not-a-real-plugin").unwrap();
        std::fs::write(base.join("readme.txt"), b"x").unwrap();

        let found = scan_directory(&base);
        // A bogus file still yields the filename-derived fallback entry, and
        // never the .txt.
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "Synth");

        std::fs::remove_dir_all(&base).unwrap();
    }
}
