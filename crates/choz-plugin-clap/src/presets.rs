//! The plugin's own patch browser, through `clap.preset-discovery` and
//! `clap.preset-load`.
//!
//! Two halves that meet nowhere else:
//!
//! * **Listing** belongs to the *entry*, not to an instance: the discovery
//!   factory hangs off the `.clap` file and a provider tells the host which
//!   directories to index and which extensions count. So the scan runs once,
//!   on the UI thread, when the instrument is loaded — [`scan`].
//! * **Loading** belongs to the *instance*, through `clap.preset-load` and the
//!   same shared cell the editor and the state use.
//!
//! **The provider is asked for locations, not for metadata.** CLAP also lets
//! the host call `get_metadata` on every file to read a preset's real name and
//! tags, but that is one call into the plugin per file — 3008 of them for a
//! stock Surge XT — and what it would buy over the file name is the tags. The
//! file stem is the patch name in every preset format that puts one file per
//! preset, which is all of them here; a plugin that packs many presets into one
//! container needs `get_metadata` and its `load_key`, and is the reason this
//! module hands back an empty key rather than pretending.

use std::ffi::CString;
use std::path::{Path, PathBuf};

use choz_ports::PresetEntry;
use clap_sys::ext::preset_load::{
    clap_plugin_preset_load, CLAP_EXT_PRESET_LOAD, CLAP_EXT_PRESET_LOAD_COMPAT,
};
use clap_sys::factory::preset_discovery::CLAP_PRESET_DISCOVERY_LOCATION_FILE;
use clap_sys::plugin::clap_plugin;

use clack_extensions::preset_discovery::prelude::*;

use crate::editor::SharedGui;

/// Stop indexing here. A location is a directory the plugin named, and a plugin
/// that names `/` (or a tree with a loop in it) must not take the UI thread
/// with it. Surge XT's factory bank is ~3000.
const MAX_PRESETS: usize = 20_000;

/// How deep to walk under a declared location. Preset banks are `bank/patch`,
/// occasionally one deeper; anything past this is somebody's home directory.
const MAX_DEPTH: usize = 6;

/// What the provider declared: where to look, and what counts as a preset.
#[derive(Default)]
struct Indexer {
    dirs: Vec<PathBuf>,
    exts: Vec<String>,
}

/// Every preset the plugin at `path` publishes, or an empty list when it
/// publishes none (which is most plugins, and is not an error).
///
/// Runs on the UI thread: it loads the entry, talks to the provider and walks
/// directories. Never call it from the audio callback.
pub fn scan(path: &Path) -> Vec<PresetEntry> {
    use clack_host::prelude::PluginEntry;

    // SAFETY: loading an external library is inherently unsafe; clack handles
    // the ABI. Nothing is instantiated — only the discovery factory is asked.
    let Ok(entry) = (unsafe { PluginEntry::load(path) }) else {
        return Vec::new();
    };
    let Some(factory) = entry.get_factory::<PresetDiscoveryFactory>() else {
        return Vec::new();
    };

    let mut indexer = Indexer::default();
    for descriptor in factory.provider_descriptors() {
        let Some(id) = descriptor.id() else { continue };
        // `instantiate` runs the provider's `init`, which is where it declares
        // its locations and file types into our indexer. A provider that fails
        // to init is skipped: another one in the same entry may still work.
        match Provider::instantiate(&mut indexer, &entry, id, &crate::host::host_info()) {
            Ok(provider) => drop(provider),
            Err(e) => eprintln!(
                "choz: CLAP preset provider {} in {}: {e}",
                id.to_string_lossy(),
                path.display()
            ),
        }
    }
    presets_under(&indexer.dirs, &indexer.exts)
}

impl IndexerImpl for &mut Indexer {
    fn declare_filetype(
        &mut self,
        file_type: FileType,
    ) -> Result<(), clack_host::prelude::HostError> {
        if let Some(ext) = file_type.file_extension {
            let ext = ext.to_string_lossy().trim_start_matches('.').to_lowercase();
            if !ext.is_empty() && !self.exts.contains(&ext) {
                self.exts.push(ext);
            }
        }
        Ok(())
    }

    fn declare_location(
        &mut self,
        location: LocationInfo,
    ) -> Result<(), clack_host::prelude::HostError> {
        // `Location::Plugin` means the presets live inside the binary and are
        // only reachable through `get_metadata` — nothing to walk, so nothing
        // to list until this module learns that call.
        if let Location::File { path } = location.location {
            let path = PathBuf::from(path.to_string_lossy().into_owned());
            if !self.dirs.contains(&path) {
                self.dirs.push(path);
            }
        }
        Ok(())
    }

    fn declare_soundpack(
        &mut self,
        _soundpack: Soundpack,
    ) -> Result<(), clack_host::prelude::HostError> {
        // A soundpack is a label for a set of presets, not a place to look.
        Ok(())
    }
}

/// Turn what a provider declared into the preset list: every matching file
/// under every declared directory, named by its file stem and filed under the
/// directory it sits in.
///
/// Split out from the CLAP plumbing so the walk itself is testable without a
/// plugin — it is the part with the edge cases (no extensions declared, a file
/// declared instead of a directory, a bank nested one deeper).
fn presets_under(dirs: &[PathBuf], exts: &[String]) -> Vec<PresetEntry> {
    let mut out = Vec::new();
    for dir in dirs {
        // A location may name a single file as well as a directory.
        if dir.is_file() {
            push_preset(&mut out, dir, dir.parent().unwrap_or(dir), exts);
            continue;
        }
        walk(dir, dir, exts, 0, &mut out);
        if out.len() >= MAX_PRESETS {
            break;
        }
    }
    out.sort_by(|a, b| (&a.category, &a.name).cmp(&(&b.category, &b.name)));
    out
}

fn walk(root: &Path, dir: &Path, exts: &[String], depth: usize, out: &mut Vec<PresetEntry>) {
    if depth > MAX_DEPTH || out.len() >= MAX_PRESETS {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    // Sorted so the list is stable across runs: `read_dir` is not.
    let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            walk(root, &path, exts, depth + 1, out);
        } else {
            push_preset(out, &path, root, exts);
        }
        if out.len() >= MAX_PRESETS {
            return;
        }
    }
}

fn push_preset(out: &mut Vec<PresetEntry>, file: &Path, root: &Path, exts: &[String]) {
    // No declared extension means "every file", per the CLAP spec.
    if !exts.is_empty() {
        let ext = file
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if !exts.contains(&ext) {
            return;
        }
    }
    let Some(name) = file.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
        return;
    };
    // The bank is the path from the declared location down to the file's own
    // directory — "Keys/Electric", not the absolute path nobody can read.
    let category = file
        .parent()
        .and_then(|p| p.strip_prefix(root).ok())
        .map(|p| p.to_string_lossy().replace('/', " / "))
        .unwrap_or_default();
    out.push(PresetEntry {
        name,
        category,
        key: file.to_string_lossy().into_owned(),
    });
}

/// The list scanned at load time, plus the live instance to load one into.
///
/// The list is a snapshot: a patch the user drops into the bank while choz runs
/// shows up the next time the instrument is loaded, not by itself.
pub struct ClapPresets {
    shared: SharedGui,
    list: Vec<PresetEntry>,
}

impl ClapPresets {
    pub fn new(shared: SharedGui, list: Vec<PresetEntry>) -> Self {
        Self { shared, list }
    }

    /// The `clap.preset-load` vtable of a live plugin, draft id included: the
    /// extension was drafted for a long time and plugins built against the
    /// draft still answer only to that name.
    ///
    /// # Safety
    /// `plugin` must be a live `clap_plugin` whose `get_extension` is callable.
    unsafe fn extension(plugin: *const clap_plugin) -> Option<*const clap_plugin_preset_load> {
        let get = unsafe { (*plugin).get_extension }?;
        for id in [CLAP_EXT_PRESET_LOAD, CLAP_EXT_PRESET_LOAD_COMPAT] {
            let ext = unsafe { get(plugin, id.as_ptr()) } as *const clap_plugin_preset_load;
            if !ext.is_null() {
                return Some(ext);
            }
        }
        None
    }
}

impl choz_ports::PluginPresets for ClapPresets {
    fn list(&self) -> Vec<PresetEntry> {
        self.list.clone()
    }

    fn load(&self, key: &str) {
        let Ok(path) = CString::new(key) else { return };
        let guard = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        let Some(cell) = guard.as_ref() else { return };
        // SAFETY: the cell is `Some` only while the instance lives.
        let Some(ext) = (unsafe { Self::extension(cell.plugin) }) else {
            return;
        };
        let Some(from_location) = (unsafe { (*ext).from_location }) else {
            return;
        };
        // An empty load key is how the spec spells "the whole file is the
        // preset", which is what a scan without `get_metadata` can promise.
        let load_key = c"";
        let ok = unsafe {
            from_location(
                cell.plugin,
                CLAP_PRESET_DISCOVERY_LOCATION_FILE,
                path.as_ptr(),
                load_key.as_ptr(),
            )
        };
        if !ok {
            eprintln!("choz: the plugin refused the preset {key}");
        }
        // The call only *queues* the patch in every plugin tried so far: Surge
        // XT and TyrellN6 both apply it on their next block, so the state right
        // after this returns is still the old one. Nothing to wait for — the
        // sound changes on its own.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The walk is the half that has no plugin in it: extensions filter, the
    /// bank comes from the directory, and a declared file works as well as a
    /// declared directory.
    #[test]
    fn the_walk_lists_a_bank_the_way_the_user_sees_it() {
        let base = std::env::temp_dir().join(format!("choz_clap_presets_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("Keys/Electric")).unwrap();
        std::fs::create_dir_all(base.join("Bass")).unwrap();
        std::fs::write(base.join("Keys/Electric/Rhodes.fxp"), b"x").unwrap();
        std::fs::write(base.join("Bass/Sub.fxp"), b"x").unwrap();
        std::fs::write(base.join("Bass/notes.txt"), b"x").unwrap();
        std::fs::write(base.join("Loose.fxp"), b"x").unwrap();

        let dirs = vec![base.clone()];
        let found = presets_under(&dirs, &["fxp".to_string()]);
        let seen: Vec<(&str, &str)> = found
            .iter()
            .map(|p| (p.category.as_str(), p.name.as_str()))
            .collect();
        assert_eq!(
            seen,
            [
                ("", "Loose"),
                ("Bass", "Sub"),
                ("Keys / Electric", "Rhodes")
            ],
            "sorted by bank, and the .txt is not a preset"
        );
        assert!(found[2].key.ends_with("Keys/Electric/Rhodes.fxp"));

        // A location that names one file yields that file.
        let one = presets_under(&[base.join("Bass/Sub.fxp")], &["fxp".to_string()]);
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].name, "Sub");

        // No declared extension means every file counts.
        let all = presets_under(&[base.join("Bass")], &[]);
        assert_eq!(all.len(), 2, "{all:?}");

        let _ = std::fs::remove_dir_all(&base);
    }
}
