//! Filesystem discovery for plugin formats: LADSPA, DSSI, LV2, SFZ, SF2, JSFX.
//!
//! Walks the filesystem recognizing each format by its on-disk convention.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::plugin_types::{PluginDescriptor, PluginHostPort, PluginKind};

#[derive(Debug, Clone)]
enum ScanRule {
    BundleDir(&'static str),
    Files(&'static [&'static str]),
}

const fn dynlib_ext() -> &'static [&'static str] {
    #[cfg(target_os = "windows")]
    { &["dll"] }
    #[cfg(target_os = "macos")]
    { &["dylib", "so"] }
    #[cfg(all(unix, not(target_os = "macos")))]
    { &["so"] }
}

fn rule_for(kind: &PluginKind) -> ScanRule {
    match kind {
        PluginKind::Lv2 => ScanRule::BundleDir("lv2"),
        PluginKind::Sfz => ScanRule::Files(&["sfz"]),
        PluginKind::Sf2 => ScanRule::Files(&["sf2", "sf3"]),
        PluginKind::Jsfx => ScanRule::Files(&["jsfx"]),
        PluginKind::Ladspa | PluginKind::Dssi => ScanRule::Files(dynlib_ext()),
        _ => ScanRule::Files(&[]),
    }
}

fn ext_matches(path: &Path, exts: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| exts.iter().any(|w| e.eq_ignore_ascii_case(w)))
        .unwrap_or(false)
}

const MAX_SCAN_DEPTH: usize = 6;

fn is_pruned_dir(name: &str) -> bool {
    matches!(
        name,
        ".git" | ".svn" | ".hg"
            | "target" | "build" | "node_modules"
            | ".cargo" | ".rustup" | ".cache" | "__pycache__"
    )
}

fn scan_directory(dir: &Path, rule: &ScanRule) -> Vec<PathBuf> {
    let mut out = Vec::new();
    scan_recursive(dir, rule, 0, &mut out);
    out
}

fn scan_recursive(dir: &Path, rule: &ScanRule, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > MAX_SCAN_DEPTH { return; }
    let rd = match std::fs::read_dir(dir) { Ok(r) => r, Err(_) => return };
    for entry in rd.flatten() {
        let path = entry.path();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let is_file = entry.file_type().map(|t| t.is_file()).unwrap_or(false);
        let recurse_into = |path: &Path, out: &mut Vec<PathBuf>| {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !is_pruned_dir(name) {
                scan_recursive(path, rule, depth + 1, out);
            }
        };
        match rule {
            ScanRule::BundleDir(ext) => {
                if is_dir {
                    if ext_matches(&path, &[ext]) { out.push(path); }
                    else { recurse_into(&path, out); }
                }
            }
            ScanRule::Files(exts) => {
                if is_file {
                    if ext_matches(&path, exts) { out.push(path); }
                } else if is_dir {
                    recurse_into(&path, out);
                }
            }
        }
    }
}

pub fn default_search_paths(kind: &PluginKind) -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut p = Vec::new();
    macro_rules! home_join {
        ($sub:expr) => {
            if let Some(h) = &home { p.push(h.join($sub)); }
        };
    }

    match kind {
        PluginKind::Lv2 => {
            #[cfg(all(unix, not(target_os = "macos")))]
            {
                home_join!(".lv2");
                p.push(PathBuf::from("/usr/lib/lv2"));
                p.push(PathBuf::from("/usr/local/lib/lv2"));
            }
        }
        PluginKind::Ladspa => {
            #[cfg(all(unix, not(target_os = "macos")))]
            {
                home_join!(".ladspa");
                p.push(PathBuf::from("/usr/lib/ladspa"));
                p.push(PathBuf::from("/usr/local/lib/ladspa"));
            }
        }
        PluginKind::Dssi => {
            #[cfg(all(unix, not(target_os = "macos")))]
            {
                home_join!(".dssi");
                p.push(PathBuf::from("/usr/lib/dssi"));
                p.push(PathBuf::from("/usr/local/lib/dssi"));
            }
        }
        PluginKind::Sf2 => {
            #[cfg(all(unix, not(target_os = "macos")))]
            {
                home_join!(".sounds/sf2");
                p.push(PathBuf::from("/usr/share/sounds/sf2"));
                p.push(PathBuf::from("/usr/share/soundfonts"));
            }
        }
        PluginKind::Sfz => {
            #[cfg(all(unix, not(target_os = "macos")))]
            {
                home_join!(".sfz");
                p.push(PathBuf::from("/usr/share/sounds/sfz"));
            }
        }
        PluginKind::Jsfx => {
            #[cfg(all(unix, not(target_os = "macos")))]
            { home_join!(".config/REAPER/Effects"); }
        }
        _ => {}
    }
    p
}

#[allow(dead_code)]
pub struct FileScanHost {
    kind: PluginKind,
    plugins: Vec<PluginDescriptor>,
    instances: HashMap<u64, ()>,
    next_id: u64,
}

#[allow(dead_code)]
impl FileScanHost {
    pub fn new(kind: PluginKind) -> Self {
        Self { kind, plugins: Vec::new(), instances: HashMap::new(), next_id: 0 }
    }

    #[allow(dead_code)]
    pub fn scan_default_paths(&mut self) -> Vec<PluginDescriptor> {
        let mut all = Vec::new();
        for dir in default_search_paths(&self.kind.clone()) {
            if let Ok(found) = self.scan(&dir) {
                all.extend(found);
            }
        }
        all
    }

    fn descriptor(&self, path: &Path) -> PluginDescriptor {
        let name = path
            .file_stem().map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Unknown".into());
        let (is_effect, is_instrument) = match self.kind {
            PluginKind::Ladspa | PluginKind::Jsfx => (true, false),
            PluginKind::Dssi => (false, true),
            PluginKind::Lv2 => (true, true),
            _ => (false, true),
        };
        PluginDescriptor {
            id: path.to_string_lossy().into_owned(),
            name,
            vendor: String::new(),
            version: String::new(),
            kind: self.kind.clone(),
            path: path.to_path_buf(),
            is_instrument,
            is_effect,
        }
    }
}

impl PluginHostPort for FileScanHost {
    fn scan(&mut self, dir: &Path) -> anyhow::Result<Vec<PluginDescriptor>> {
        let rule = rule_for(&self.kind);
        let found: Vec<PluginDescriptor> = scan_directory(dir, &rule)
            .iter().map(|p| self.descriptor(p)).collect();
        for d in &found {
            if !self.plugins.iter().any(|p| p.id == d.id) {
                self.plugins.push(d.clone());
            }
        }
        Ok(found)
    }

    fn list_plugins(&self) -> &[PluginDescriptor] { &self.plugins }

    fn instantiate(&mut self, plugin_id: &str, _sr: u32, _block: u32) -> anyhow::Result<u64> {
        if !self.plugins.iter().any(|p| p.id == plugin_id) {
            anyhow::bail!("{} plugin not found: {plugin_id}", self.kind.label());
        }
        self.next_id += 1;
        let id = self.next_id;
        self.instances.insert(id, ());
        Ok(id)
    }

    fn destroy(&mut self, instance_id: u64) { self.instances.remove(&instance_id); }

    fn process(&mut self, _instance_id: u64, _input: &[f32], output: &mut [f32]) -> anyhow::Result<()> {
        output.fill(0.0);
        Ok(())
    }
}
