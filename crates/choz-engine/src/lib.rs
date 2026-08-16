//! choz audio engine: RT audio thread, sources, FX chain, MIDI input, and the
//! plugin registry. Built on the RT-safe traits in `choz-ports`.

pub mod cache;
pub mod chord;
pub mod engine;
pub mod feedback;
pub mod fx;
pub mod fx_chain;
pub mod input;
mod jack_backend;
pub mod maxpat;
pub mod meter;
pub mod midi;
pub mod osc;
pub mod paths;
pub mod pitch;
pub mod quarantine;
pub mod sandboxed;
pub mod sfz;
pub mod sources;

pub use engine::{AudioBackend, AudioEngine};

/// Parameter index that means "choz's own dry/wet" in
/// [`AudioEngine::set_fx_param`], rather than one of the processor's params.
pub const FX_MIX_PARAM: usize = usize::MAX;
pub use fx_chain::FxSpec;

pub use paths::{FoundPlugin, PluginFormat, PluginPaths, SearchDir};

/// Scan every enabled directory of every format in `paths`.
///
/// Each directory is scanned **in a child process** ([`scan_worker_main`]), so
/// a plugin that segfaults while being probed costs that one directory instead
/// of the whole app. Falls back to scanning in-process when the child can't be
/// spawned at all (no `current_exe`, no fork).
pub fn scan_all(paths: &PluginPaths) -> Vec<FoundPlugin> {
    let mut out = Vec::new();
    for (format, dirs) in paths.entries.iter() {
        for dir in dirs.iter().filter(|d| d.enabled) {
            match scan_dir_out_of_process(*format, &dir.path) {
                Some(found) => out.extend(found),
                None => out.extend(scan_one(*format, &dir.path)),
            }
        }
    }
    out.sort_by_key(|p| (p.format, p.name.to_lowercase()));
    out.dedup_by(|a, b| a.path == b.path && a.id == b.id);
    out
}

/// Argument that turns the choz binary into a scan worker.
pub const SCAN_WORKER_FLAG: &str = "--choz-scan-worker";

/// Set in every child choz spawns. A worker never spawns workers of its own:
/// without this, a binary that forgets to answer one of the flags re-runs
/// itself forever (found the hard way, with a test that did exactly that).
pub const WORKER_ENV: &str = "CHOZ_WORKER";

/// True when this process is one of choz's own children.
pub fn is_worker() -> bool {
    std::env::var_os(WORKER_ENV).is_some()
}

/// Answer whichever worker role the arguments ask for: plugin scan, load probe
/// or audio sandbox. Returns `true` when this process was a worker and has done
/// its job, in which case the caller must exit without touching the terminal or
/// the audio device.
///
/// **Every binary that links choz-engine must call this first thing in `main`**,
/// including test binaries — the engine re-runs the current executable for each
/// of those roles.
pub fn worker_main() -> bool {
    scan_worker_main() || quarantine::probe_worker_main() || sandboxed::sandbox_worker_main()
}

/// Run one directory scan in a child process. `None` means "couldn't even
/// start one" — the caller then scans in-process. A child that dies takes its
/// directory's results with it and says so.
///
/// ponytail: one child per directory. A crash still loses the other plugins in
/// that directory, which is why the deny-lists for the Carla wrappers are still
/// there. Per-file isolation means the worker emitting a marker per candidate
/// and the parent resuming after it — do that when an unknown plugin actually
/// starts costing someone a directory. There is no timeout either: a plugin
/// that *hangs* still hangs the scan, which has not happened yet.
fn scan_dir_out_of_process(
    format: PluginFormat,
    dir: &std::path::Path,
) -> Option<Vec<FoundPlugin>> {
    if !worker_available() {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    // The result goes through a file, not the child's stdout: plugins print
    // banners and warnings to stdout while being probed (u-he, fluidsynth,
    // guitarix all do) and that noise would land in the middle of the JSON.
    let out_file = cache::state_dir().join(format!("scan-{}.json", std::process::id()));
    let status = std::process::Command::new(exe)
        .env(WORKER_ENV, "1")
        .arg(SCAN_WORKER_FLAG)
        .arg(format.label())
        .arg(dir)
        .arg(&out_file)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::inherit())
        .status()
        .ok()?;
    let found = std::fs::read(&out_file).ok();
    let _ = std::fs::remove_file(&out_file);
    if !status.success() {
        eprintln!(
            "choz: scanning {} for {} crashed ({status}); retrying one entry at a time",
            dir.display(),
            format.label(),
        );
        return Some(scan_dir_entrywise(format, dir));
    }
    match found
        .as_deref()
        .map(serde_json::from_slice::<Vec<FoundPlugin>>)
    {
        Some(Ok(found)) => Some(found),
        Some(Err(e)) => {
            eprintln!("choz: scan worker for {} returned junk: {e}", dir.display());
            Some(Vec::new())
        }
        None => {
            eprintln!("choz: scan worker for {} wrote nothing", dir.display());
            Some(Vec::new())
        }
    }
}

/// Whether re-running this executable actually gives us a scan worker.
///
/// Only the choz binary calls [`scan_worker_main`] at startup; a test harness
/// or any other embedder linking this crate would just re-run itself with
/// arguments it doesn't understand. So ask once, with a directory that cannot
/// exist: a real worker answers `[]`, anything else answers nothing and every
/// scan stays in-process.
fn worker_available() -> bool {
    static OK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OK.get_or_init(|| {
        if is_worker() {
            return false;
        }
        let Ok(exe) = std::env::current_exe() else {
            return false;
        };
        let probe = cache::state_dir().join(format!("scan-probe-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&probe);
        let ran = std::process::Command::new(exe)
            .env(WORKER_ENV, "1")
            .arg(SCAN_WORKER_FLAG)
            .arg("SF2")
            .arg(std::path::Path::new("/nonexistent/choz/scan/probe"))
            .arg(&probe)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        let answered = ran && std::fs::read(&probe).is_ok_and(|b| b.starts_with(b"["));
        let _ = std::fs::remove_file(&probe);
        if !answered {
            eprintln!("choz: no scan worker available; scanning in-process");
        }
        answered
    })
}

/// Second pass over a directory whose scan crashed: one child per entry, so
/// only the plugin that actually blows up is lost. Subdirectories are handed
/// over whole — a bundle *is* a directory, and a plain subdirectory that
/// crashes just recurses into this same split.
fn scan_dir_entrywise(format: PluginFormat, dir: &std::path::Path) -> Vec<FoundPlugin> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        match scan_dir_out_of_process(format, &path) {
            Some(found) => out.extend(found),
            // Can't spawn at all: scanning this entry in-process is the only
            // option left, and it is what the old behaviour did for everything.
            None => out.extend(scan_one(format, &path)),
        }
    }
    out
}

/// The child side of [`scan_all`]: scan one directory and print the result as
/// JSON on stdout. Returns `false` when the arguments aren't a worker
/// invocation, so `main` can carry on as normal.
///
/// Call this **first thing** in `main`, before any terminal or audio setup —
/// the worker must not draw anything or open a device. The result is written to
/// the file named by the last argument, because the plugins being probed print
/// to stdout themselves.
pub fn scan_worker_main() -> bool {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 5 || args[1] != SCAN_WORKER_FLAG {
        return false;
    }
    let found = match PluginFormat::from_label(&args[2]) {
        Some(format) => scan_one(format, std::path::Path::new(&args[3])),
        None => Vec::new(),
    };
    let json = serde_json::to_string(&found).unwrap_or_else(|_| "[]".into());
    if let Err(e) = std::fs::write(&args[4], json) {
        eprintln!("choz: scan worker cannot write {}: {e}", args[4]);
    }
    true
}

/// Scan a single directory for a single format. Hosted formats (CLAP, LV2, …)
/// get real metadata — plugin name, id/URI, instrument flag — from the plugin
/// itself; everything else is identified by file name.
pub fn scan_one(format: PluginFormat, dir: &std::path::Path) -> Vec<FoundPlugin> {
    // A single file (or a single `*.lv2` / `*.vst3` bundle) rather than a
    // directory to walk: that's the per-entry retry after a crash. A path that
    // doesn't exist at all is a search directory that was never created, and
    // must stay empty rather than be described as a plugin.
    if !dir.exists() {
        return Vec::new();
    }
    if !dir.is_dir() || is_bundle(dir) {
        return scan_item(format, dir);
    }
    match format {
        PluginFormat::Clap => choz_plugin_clap::scan_directory(dir)
            .into_iter()
            .map(|p| FoundPlugin {
                format: PluginFormat::Clap,
                name: p.name,
                path: p.path,
                id: p.id,
                is_instrument: p.is_instrument,
            })
            .collect(),
        PluginFormat::Lv2 => choz_plugin_lv2::scan_directory(dir)
            .into_iter()
            .map(|p| FoundPlugin {
                format: PluginFormat::Lv2,
                name: p.name,
                // The bundle directory + URI is what loading needs.
                path: p.bundle_dir,
                id: p.uri,
                is_instrument: p.is_instrument,
            })
            .collect(),
        PluginFormat::Ladspa | PluginFormat::Dssi => choz_plugin_ladspa::scan_directory(dir)
            .into_iter()
            .map(|p| FoundPlugin {
                format,
                name: p.name,
                path: p.path,
                // The LADSPA label is what identifies a plugin inside a `.so`
                // that exports several.
                id: p.label,
                is_instrument: p.is_instrument,
            })
            .collect(),
        PluginFormat::Vst2 => choz_plugin_vst2::scan_directory(dir)
            .into_iter()
            .map(|p| FoundPlugin {
                format: PluginFormat::Vst2,
                name: p.name,
                path: p.path,
                id: p.id,
                is_instrument: p.is_instrument,
            })
            .collect(),
        PluginFormat::Vst3 => choz_plugin_vst3::scan_directory(dir)
            .into_iter()
            .map(|p| FoundPlugin {
                format: PluginFormat::Vst3,
                name: p.name,
                path: p.path,
                id: String::new(),
                is_instrument: p.is_instrument,
            })
            .collect(),
        _ => paths::scan_dir(dir, format),
    }
}

/// A directory that is itself one plugin, not a place to look inside.
fn is_bundle(path: &std::path::Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("lv2") | Some("vst3")
    )
}

/// Scan exactly one plugin file or bundle. Used by the per-entry retry, where
/// pointing a directory walker at a single file would just find nothing.
fn scan_item(format: PluginFormat, path: &std::path::Path) -> Vec<FoundPlugin> {
    match format {
        PluginFormat::Clap => choz_plugin_clap::describe(path)
            .into_iter()
            .map(|p| FoundPlugin {
                format: PluginFormat::Clap,
                name: p.name,
                path: p.path,
                id: p.id,
                is_instrument: p.is_instrument,
            })
            .collect(),
        PluginFormat::Lv2 => choz_plugin_lv2::discovery::discover_bundle(path)
            .into_iter()
            .map(|p| FoundPlugin {
                format: PluginFormat::Lv2,
                name: p.name,
                path: p.bundle_dir,
                id: p.uri,
                is_instrument: p.is_instrument,
            })
            .collect(),
        PluginFormat::Ladspa | PluginFormat::Dssi => choz_plugin_ladspa::describe(path)
            .into_iter()
            .map(|p| FoundPlugin {
                format,
                name: p.name,
                path: p.path,
                id: p.label,
                is_instrument: p.is_instrument,
            })
            .collect(),
        PluginFormat::Vst2 => choz_plugin_vst2::describe(path)
            .into_iter()
            .map(|p| FoundPlugin {
                format: PluginFormat::Vst2,
                name: p.name,
                path: p.path,
                id: p.id,
                is_instrument: p.is_instrument,
            })
            .collect(),
        PluginFormat::Vst3 => {
            let p = choz_plugin_vst3::describe(path);
            vec![FoundPlugin {
                format: PluginFormat::Vst3,
                name: p.name,
                path: p.path,
                id: String::new(),
                is_instrument: p.is_instrument,
            }]
        }
        // File-based "plugins" (SF2, SFZ): the file itself is the entry.
        _ => paths::scan_path(path, format),
    }
}

pub use choz_plugin_clap::ClapPluginInfo;
pub use choz_ports::PluginParam;

/// Parameters exposed by a hosted plugin. Non-RT: CLAP loads the binary, LV2
/// reads the bundle TTL. Empty for formats choz can't host.
pub fn read_plugin_params(
    format: PluginFormat,
    path: &std::path::Path,
    id: &str,
) -> Vec<PluginParam> {
    match format {
        PluginFormat::Clap => choz_plugin_clap::read_params(path, id),
        PluginFormat::Lv2 => choz_plugin_lv2::read_params(path, id),
        PluginFormat::Ladspa | PluginFormat::Dssi => choz_plugin_ladspa::read_params(path, id),
        PluginFormat::Vst2 => choz_plugin_vst2::read_params(path, id),
        PluginFormat::Vst3 => choz_plugin_vst3::read_params(path, id),
        // A Pure Data patch's knobs are its own on-screen controls — the ones
        // that carry a receive symbol, because the rest cannot be moved from
        // outside the canvas. Read from the file, so this needs no Pd.
        PluginFormat::Pd => pd_params(path),
        _ => Vec::new(),
    }
}

/// The controls of a `.pd` patch, as parameters.
///
/// Also the place that says, once and by name, which controls **cannot** be
/// reached: a patch whose gain slider has no receive symbol sits at whatever
/// that slider was saved at — zero, for a fresh one — and is silent with no
/// error anywhere. Giving the slider a receive symbol in Pd is the fix, and
/// nobody guesses that from silence.
fn pd_params(path: &std::path::Path) -> Vec<PluginParam> {
    let Ok(info) = choz_plugin_pd::read_patch(path) else {
        return Vec::new();
    };
    let stuck = choz_plugin_pd::unreachable(&info);
    if !stuck.is_empty() {
        eprintln!(
            "choz: {} has {} control(s) choz cannot move ({}). Give them a receive \
             symbol in Pd (the slider's properties) and they become knobs here.",
            path.display(),
            stuck.len(),
            stuck.join(", ")
        );
    }
    choz_plugin_pd::addressable(&info)
        .into_iter()
        .enumerate()
        .map(|(i, c)| {
            let mut p = PluginParam::plain_range(
                i as u32,
                c.name.clone(),
                c.min as f64,
                c.max as f64,
                c.min as f64,
            );
            if c.toggle {
                p.steps = 2;
            }
            p
        })
        .collect()
}

/// Locks for the things that are global to the **process**, so tests that move
/// them do not move them under each other.
///
/// The transport and the meters are singletons by design — there is one
/// output and one clock — and `cargo test` runs a crate's tests in parallel in
/// one process. Two tests each rewinding the transport, or one clearing the
/// meter while another renders into it, is a failure that looks like a bug in
/// the code under test and is not one. **One lock per global**, in one place,
/// or they do not serialise against each other: that was the first attempt.
#[cfg(test)]
pub(crate) mod test_locks {
    use std::sync::{Mutex, MutexGuard};

    static TRANSPORT: Mutex<()> = Mutex::new(());
    static METER: Mutex<()> = Mutex::new(());

    /// Held while a test moves the transport (rewind, play, tempo).
    pub(crate) fn transport() -> MutexGuard<'static, ()> {
        TRANSPORT.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Held while a test reads or clears the output meter — which every
    /// `render` writes to.
    pub(crate) fn meter() -> MutexGuard<'static, ()> {
        METER.lock().unwrap_or_else(|e| e.into_inner())
    }
}
