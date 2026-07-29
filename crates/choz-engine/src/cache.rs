//! On-disk cache of the plugin scan.
//!
//! Scanning dlopens every `.clap` file on the system, which costs a noticeable
//! chunk of startup. The result is cached as JSON in the state dir and reused
//! until a search directory looks newer than the cache.

use std::path::PathBuf;

use crate::paths::FoundPlugin;

/// `$XDG_STATE_HOME/choz`, else `~/.local/state/choz`, else `$TMPDIR/choz`.
/// Shared with the UI's log file so both land in the same place.
pub fn state_dir() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")));
    match base {
        Some(b) => b.join("choz"),
        None => std::env::temp_dir().join("choz"),
    }
}

fn cache_path() -> PathBuf {
    state_dir().join("plugins.json")
}

fn modified(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

/// True when the cache exists and no search directory has been touched since it
/// was written.
///
/// ponytail: only the search directories themselves are stat'ed, not the whole
/// tree — installing a plugin touches its parent dir, which is the case that
/// matters. Nested edits need an explicit rescan.
fn cache_is_fresh(dirs: &[PathBuf]) -> bool {
    let Some(cached_at) = modified(&cache_path()) else { return false };
    dirs.iter()
        .filter_map(|d| modified(d))
        .all(|dir_at| dir_at <= cached_at)
}

/// The cache file. `hosted` records whether the build that wrote it had real
/// CLAP hosting: without the feature a scan only gets filename-derived
/// metadata (every plugin looks like an instrument), which must never be served
/// to a build that can do better — or vice versa.
#[derive(serde::Serialize, serde::Deserialize)]
struct CacheFile {
    hosted: bool,
    plugins: Vec<FoundPlugin>,
}

fn read_cache() -> Option<Vec<FoundPlugin>> {
    let data = std::fs::read_to_string(cache_path()).ok()?;
    let cached: CacheFile = serde_json::from_str(&data).ok()?;
    (cached.hosted == choz_plugin_clap::clap_supported()).then_some(cached.plugins)
}

fn write_cache(plugins: &[FoundPlugin]) {
    let path = cache_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let file = CacheFile {
        hosted: choz_plugin_clap::clap_supported(),
        plugins: plugins.to_vec(),
    };
    match serde_json::to_string_pretty(&file) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                eprintln!("choz: cannot write plugin cache {}: {e}", path.display());
            }
        }
        Err(e) => eprintln!("choz: cannot serialize plugin cache: {e}"),
    }
}

/// Plugins from the cache when it's fresh, otherwise a full scan (which is then
/// cached). `scan` is the real scanner, injected so this module stays testable
/// and doesn't care where the plugins come from.
pub fn cached_or_scan(dirs: &[PathBuf], scan: impl FnOnce() -> Vec<FoundPlugin>) -> Vec<FoundPlugin> {
    if cache_is_fresh(dirs) {
        // A cache written by the other kind of build reads as `None` here and
        // falls through to a real scan.
        if let Some(plugins) = read_cache() {
            return plugins;
        }
    }
    let plugins = scan();
    write_cache(&plugins);
    plugins
}

/// Force a scan and refresh the cache.
pub fn rescan(scan: impl FnOnce() -> Vec<FoundPlugin>) -> Vec<FoundPlugin> {
    let plugins = scan();
    write_cache(&plugins);
    plugins
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<FoundPlugin> {
        vec![FoundPlugin {
            format: crate::paths::PluginFormat::Clap,
            path: "/usr/lib/clap/Thing.clap".into(),
            name: "Thing".into(),
            id: "com.acme.thing".into(),
            is_instrument: true,
        }]
    }

    #[test]
    fn cache_round_trips_through_json() {
        let file = CacheFile { hosted: true, plugins: sample() };
        let json = serde_json::to_string(&file).unwrap();
        let back: CacheFile = serde_json::from_str(&json).unwrap();
        assert_eq!(back.plugins.len(), 1);
        assert_eq!(back.plugins[0].id, "com.acme.thing");
        assert!(back.plugins[0].is_instrument);
        assert!(back.hosted, "the build flavour is part of the cache");
    }

    /// A missing cache is never fresh; once written it's served without
    /// re-running the (expensive) scan.
    #[test]
    fn cache_is_written_then_served_without_rescanning() {
        let tmp = std::env::temp_dir().join(format!("choz_cache_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        // Point the state dir at an empty temp dir for this check. This is
        // process-global: keep state-dir use out of other tests in this crate.
        let prev = std::env::var_os("XDG_STATE_HOME");
        unsafe { std::env::set_var("XDG_STATE_HOME", &tmp) };
        assert!(!cache_is_fresh(std::slice::from_ref(&tmp)));

        // First call scans and writes the cache...
        let mut scans = 0;
        let dirs: Vec<PathBuf> = Vec::new();
        let got = cached_or_scan(&dirs, || { scans += 1; sample() });
        assert_eq!((got.len(), scans), (1, 1));
        assert!(cache_is_fresh(&dirs), "cache file exists now");

        // ...the second is served from disk without touching the scanner.
        let got = cached_or_scan(&dirs, || panic!("must not rescan a fresh cache"));
        assert_eq!(got[0].id, "com.acme.thing");

        // An explicit rescan always runs and rewrites.
        let got = rescan(|| { scans += 1; sample() });
        assert_eq!((got.len(), scans), (1, 2));

        // A cache written by the other build flavour holds filename-derived
        // metadata (everything looks like an instrument), so it's ignored and
        // rescanned. Same test to keep the XDG_STATE_HOME override in one place.
        let wrong = CacheFile { hosted: !choz_plugin_clap::clap_supported(), plugins: sample() };
        std::fs::write(cache_path(), serde_json::to_string(&wrong).unwrap()).unwrap();
        assert!(read_cache().is_none(), "foreign cache ignored");
        cached_or_scan(&dirs, || { scans += 1; sample() });
        assert_eq!(scans, 3, "a rejected cache forces a rescan");
        assert!(read_cache().is_some(), "and the rescan rewrites it in our flavour");

        std::fs::remove_dir_all(&tmp).unwrap();
        match prev {
            Some(v) => unsafe { std::env::set_var("XDG_STATE_HOME", v) },
            None => unsafe { std::env::remove_var("XDG_STATE_HOME") },
        }
    }
}
