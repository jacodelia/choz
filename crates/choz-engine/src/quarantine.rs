//! Try a plugin in a child process before letting it near the audio thread.
//!
//! Scanning already runs out of process, so a plugin that blows up while being
//! *probed* costs only itself. Loading one into a rack slot did not: Carla's
//! plugin-host wrappers corrupt the heap the moment they are instantiated, and
//! that took choz down with them.
//!
//! So the first time a plugin is loaded, choz runs it in a child — instantiate,
//! process a couple of blocks, drop — and remembers the verdict. Plugins that
//! die on the way in are refused with a message instead of a segfault.
//!
//! ponytail: this is half of "hosting out of process". The plugin still *plays*
//! in choz's own process, so one that crashes after minutes of use still takes
//! the app with it. Fixing that needs the audio itself to cross a process
//! boundary through shared memory — a different, much larger piece of work.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::paths::PluginFormat;

/// How a plugin behaved when it was tried on its own.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    /// Instantiated, processed and tore down cleanly.
    #[default]
    Ok,
    /// Died before it ever produced audio. choz refuses to load these.
    CrashesOnLoad,
    /// Played, then died while being destroyed. Loading is allowed: the crash
    /// only comes when the slot is dropped, and the plugin is leaked instead.
    CrashesOnTeardown,
}

impl Verdict {
    pub fn loadable(self) -> bool {
        self != Verdict::CrashesOnLoad
    }

    /// How bad this verdict is. Used to keep the worst of several probes: a
    /// plugin that crashes *sometimes* has to be treated as one that crashes.
    fn severity(self) -> u8 {
        match self {
            Verdict::Ok => 0,
            Verdict::CrashesOnTeardown => 1,
            Verdict::CrashesOnLoad => 2,
        }
    }
}

/// How many times a plugin is probed before it is believed to be fine.
///
/// One probe is not enough, and this was measured rather than guessed:
/// `padthv1` segfaults on teardown in roughly two runs out of three — its Qt
/// thread racing `cleanup` — so a single sample says `Ok` often enough to cache
/// the wrong answer, leave the plugin un-sandboxed, and take choz down when the
/// tab is closed. Three probes cut that to about one in thirty.
///
/// Only ever paid once per plugin (the verdict is cached), and a bad verdict
/// stops the loop early. `CHOZ_PROBE_RUNS` overrides it — a slower machine may
/// want more samples, and a probe-heavy first run may want fewer.
const PROBE_RUNS: usize = 3;

fn probe_runs() -> usize {
    std::env::var("CHOZ_PROBE_RUNS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(PROBE_RUNS)
        .max(1)
}

/// Argument that turns the choz binary into a load-probe worker.
pub const PROBE_WORKER_FLAG: &str = "--choz-probe-worker";

/// How far the child got. Written to the file the parent named, so a crash
/// leaves behind the last stage that completed.
const STAGE_STARTED: &str = "started";
const STAGE_LOADED: &str = "loaded";
const STAGE_DONE: &str = "done";

/// Appended to a stage once the child knows the plugin has a window. The probe
/// only *asks* for the handle — opening the thing is what is dangerous, and the
/// whole point of the answer is that choz never has to do it in its own process.
const GUI_MARK: &str = " gui";

/// What one probe found out about a plugin: how it behaved, and whether it has
/// a window at all.
///
/// The verdict alone stopped being enough once the editor mattered: a plugin
/// whose GUI segfaults processes audio perfectly, so the probe calls it `Ok` and
/// nothing isolates it — until the user presses `GUI` and takes choz with it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub verdict: Verdict,
    /// The plugin offers its own window.
    #[serde(default)]
    pub editor: bool,
}

fn cache_path() -> PathBuf {
    crate::cache::state_dir().join("plugin-verdicts.json")
}

/// `format|path|id` — the plugin's identity in the cache file.
fn key(format: PluginFormat, path: &Path, id: &str) -> String {
    format!("{}|{}|{id}", format.label(), path.display())
}

/// A file written before [`Report`] existed holds bare verdicts and no longer
/// parses; it is dropped whole and every plugin is probed once more, which is
/// what has to happen anyway to learn about its window.
fn load_cache() -> HashMap<String, Report> {
    std::fs::read(cache_path())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_cache(cache: &HashMap<String, Report>) {
    let path = cache_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_vec_pretty(cache) {
        let _ = std::fs::write(path, json);
    }
}

/// The verdict for one plugin, probing it if this is the first time.
///
/// Never fails: if a child can't be spawned the answer is [`Verdict::Ok`], which
/// is exactly the behaviour choz had before any of this existed.
pub fn check(format: PluginFormat, path: &Path, id: &str) -> Report {
    let k = key(format, path, id);
    let mut cache = load_cache();
    if let Some(r) = cache.get(&k) {
        return *r;
    }
    // Worst of N: the crashes worth catching are races, and a single sample
    // that comes back clean is what silently un-sandboxes a plugin that kills
    // the app two out of three times.
    let mut report = Report::default();
    for _ in 0..probe_runs() {
        let r = probe(format, path, id);
        if r.verdict.severity() > report.verdict.severity() {
            report.verdict = r.verdict;
        }
        // A run that died before loading saw no window; one that got there did.
        report.editor |= r.editor;
        // Nothing worse to find, and the answer is already the strictest one.
        if report.verdict == Verdict::CrashesOnLoad
            || NOT_A_WORKER.load(std::sync::atomic::Ordering::Relaxed)
        {
            break;
        }
    }
    if report.verdict != Verdict::Ok {
        eprintln!(
            "choz: {} {} is quarantined: {:?}",
            format.label(),
            path.display(),
            report.verdict
        );
    }
    cache.insert(k, report);
    save_cache(&cache);
    report
}

/// Forget every verdict, so the next load probes again. For the user who has
/// just replaced a broken plugin with a fixed one.
pub fn clear() {
    let _ = std::fs::remove_file(cache_path());
}

// ─── Sandbox on request ─────────────────────────────────────────────────────
//
// The automatic policy only isolates what the probe caught dying. A plugin that
// misbehaves later — minutes in, or only with a particular patch — is invisible
// to a two-block probe, so the user gets to say "always run this one on its
// own". The choice is per plugin rather than per rack tab: it is a property of
// the plugin, and it should survive reloading the project.

fn forced_path() -> PathBuf {
    crate::cache::state_dir().join("plugin-sandbox.json")
}

fn load_forced() -> Vec<String> {
    std::fs::read(forced_path())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Whether the user asked for this plugin to always run in its own process.
pub fn forced(format: PluginFormat, path: &Path, id: &str) -> bool {
    load_forced().contains(&key(format, path, id))
}

/// Turn "always sandbox this plugin" on or off. Takes effect the next time the
/// plugin is instantiated.
pub fn set_forced(format: PluginFormat, path: &Path, id: &str, on: bool) {
    let k = key(format, path, id);
    let mut list = load_forced();
    match (on, list.iter().position(|e| *e == k)) {
        (true, None) => list.push(k),
        (false, Some(i)) => {
            list.remove(i);
        }
        _ => return,
    }
    let p = forced_path();
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_vec_pretty(&list) {
        let _ = std::fs::write(p, json);
    }
}

/// Whether this plugin should be hosted out of process: because the user said
/// so, because the probe saw it die on the way out, or because it has a window.
///
/// A window is a reason on its own. Plugin GUIs are the least trustworthy code
/// choz runs — guitarix's 31 X11 UIs segfault whatever loads them, and the
/// deny-list that hides them only exists because there was nowhere safe to open
/// them. There is now: the plugin that owns the window also owns the process
/// that dies with it, and the supervisor puts a new one back.
///
/// `CHOZ_SANDBOX_GUI=0` turns this half off for whoever would rather pay the
/// crash than the extra process.
pub fn wants_sandbox(format: PluginFormat, path: &Path, id: &str) -> bool {
    if forced(format, path, id) {
        return true;
    }
    let report = check(format, path, id);
    report.verdict == Verdict::CrashesOnTeardown || (report.editor && sandbox_guis())
}

fn sandbox_guis() -> bool {
    std::env::var("CHOZ_SANDBOX_GUI")
        .map(|v| v != "0")
        .unwrap_or(true)
}

/// Set once a spawned child turns out not to understand the probe flag, so a
/// binary that doesn't call [`crate::worker_main`] pays for exactly one failed
/// spawn instead of one per plugin.
static NOT_A_WORKER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Split what the child left behind into the stage it reached and whether it
/// had found a window by then. A crash before loading leaves no mark and the
/// answer is "no window seen", which is the truth.
fn read_stage(written: &str) -> (&str, bool) {
    match written.strip_suffix(GUI_MARK) {
        Some(stage) => (stage, true),
        None => (written, false),
    }
}

fn probe(format: PluginFormat, path: &Path, id: &str) -> Report {
    // A worker never probes: it *is* the probe, or the sandbox that already
    // knows what it is loading.
    if crate::is_worker() || NOT_A_WORKER.load(std::sync::atomic::Ordering::Relaxed) {
        return Report::default();
    }
    let Ok(exe) = std::env::current_exe() else {
        return Report::default();
    };
    // The child writes its progress here, so the directory has to exist before
    // it starts: a failed write looks exactly like "this binary is not a probe
    // worker", and every plugin would come back Ok.
    let dir = crate::cache::state_dir();
    let _ = std::fs::create_dir_all(&dir);
    let out = dir.join(format!("probe-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&out);
    let spawned = std::process::Command::new(exe)
        .env(crate::WORKER_ENV, "1")
        .arg(PROBE_WORKER_FLAG)
        .arg(format.label())
        .arg(path)
        .arg(id)
        .arg(&out)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::inherit())
        .status();
    let written = std::fs::read_to_string(&out).unwrap_or_default();
    let _ = std::fs::remove_file(&out);
    let (stage, editor) = read_stage(&written);
    let verdict = match spawned {
        // The child never wrote anything: it isn't a probe worker at all (a
        // test binary, say), so assume the plugin is fine and stop asking.
        Ok(_) if stage.is_empty() => {
            NOT_A_WORKER.store(true, std::sync::atomic::Ordering::Relaxed);
            Verdict::Ok
        }
        Ok(status) if status.success() && stage == STAGE_DONE => Verdict::Ok,
        Ok(_) if stage == STAGE_LOADED => Verdict::CrashesOnTeardown,
        Ok(_) => Verdict::CrashesOnLoad,
        Err(_) => Verdict::Ok,
    };
    Report { verdict, editor }
}

/// The child side: load the named plugin, run it, drop it, recording how far it
/// got. Returns `false` when the arguments aren't a probe invocation.
///
/// Call it from `main` next to [`crate::scan_worker_main`], before anything
/// touches the terminal or the audio device.
pub fn probe_worker_main() -> bool {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 6 || args[1] != PROBE_WORKER_FLAG {
        return false;
    }
    let Some(format) = PluginFormat::from_label(&args[2]) else {
        return true;
    };
    let (path, id, out) = (Path::new(&args[3]), args[4].as_str(), &args[5]);
    // Asking whether a UI exists is not loading it, and the answer is what puts
    // the dangerous ones behind a process boundary. Hiding them here would keep
    // the very plugins the deny-list is about out of the sandbox.
    choz_plugin_lv2::allow_denied_uis(true);
    let gui = std::cell::Cell::new(false);
    let mark = |stage: &str| {
        let text = if gui.get() {
            format!("{stage}{GUI_MARK}")
        } else {
            stage.to_string()
        };
        let _ = std::fs::write(out, text);
    };

    mark(STAGE_STARTED);
    // Both halves of what a slot does: an instrument source or an FX processor.
    // Whichever this plugin is, the dangerous part is the same — instantiate,
    // run, destroy.
    let (sr, block) = (48_000u32, 256u32);
    let mut source = crate::engine::build_instrument(format, path, id, sr, block).ok();
    let mut buf = vec![0.0f32; block as usize * 2];
    if source.is_none() {
        // Not an instrument: try it as an effect before calling it broken.
        let mut fx = crate::fx_chain::build_plugin_fx_in_process(
            &crate::fx_chain::PluginFxRef {
                format,
                path: path.to_path_buf(),
                id: id.to_string(),
            },
            sr,
            block,
        );
        if let Some(fx) = fx.as_mut() {
            gui.set(fx.editor().is_some());
            mark(STAGE_LOADED);
            for _ in 0..2 {
                fx.process_block(&mut buf, sr);
            }
        }
        drop(fx);
        mark(STAGE_DONE);
        return true;
    }
    gui.set(source.as_ref().is_some_and(|s| s.editor().is_some()));
    mark(STAGE_LOADED);
    if let Some(src) = source.as_mut() {
        src.note_on(60, 100);
        for _ in 0..2 {
            src.render(&mut buf, sr);
        }
        src.note_off(60);
    }
    drop(source);
    mark(STAGE_DONE);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_crashing_verdict_is_not_loadable() {
        assert!(Verdict::Ok.loadable());
        assert!(
            Verdict::CrashesOnTeardown.loadable(),
            "it plays; only the drop hurts"
        );
        assert!(!Verdict::CrashesOnLoad.loadable());
    }

    /// The whole point of probing more than once: one clean run does not clear
    /// a plugin that died in another.
    #[test]
    fn the_worst_of_several_probes_is_the_one_that_counts() {
        let worst = |vs: &[Verdict]| {
            vs.iter().copied().fold(Verdict::Ok, |acc, v| {
                if v.severity() > acc.severity() {
                    v
                } else {
                    acc
                }
            })
        };
        assert_eq!(worst(&[Verdict::Ok, Verdict::Ok, Verdict::Ok]), Verdict::Ok);
        assert_eq!(
            worst(&[Verdict::Ok, Verdict::CrashesOnTeardown, Verdict::Ok]),
            Verdict::CrashesOnTeardown,
            "a teardown crash seen once is a teardown crash"
        );
        assert_eq!(
            worst(&[Verdict::CrashesOnTeardown, Verdict::CrashesOnLoad]),
            Verdict::CrashesOnLoad,
        );
    }

    /// The child says two things in one file, and a crash truncates it after
    /// whichever mark it had reached.
    #[test]
    fn the_stage_file_carries_the_window_answer_with_the_stage() {
        assert_eq!(read_stage("done"), ("done", false));
        assert_eq!(read_stage("done gui"), ("done", true));
        assert_eq!(
            read_stage("loaded gui"),
            ("loaded", true),
            "died on teardown, has a UI"
        );
        assert_eq!(
            read_stage("started"),
            ("started", false),
            "never got far enough to know"
        );
        assert_eq!(read_stage(""), ("", false), "not a probe worker at all");
    }

    /// The verdict is not the only reason to isolate a plugin: a window is one
    /// on its own, and the cache has to remember it for the next load.
    #[test]
    fn a_report_survives_the_cache_with_its_window_flag() {
        let r = Report {
            verdict: Verdict::Ok,
            editor: true,
        };
        let json = serde_json::to_vec(&r).unwrap();
        assert_eq!(serde_json::from_slice::<Report>(&json).unwrap(), r);
        // Written before the flag existed: no field, and nothing is claimed.
        let old: Report = serde_json::from_str(r#"{"verdict":"Ok"}"#).unwrap();
        assert!(!old.editor);
    }

    #[test]
    fn the_cache_key_separates_plugins_inside_one_file() {
        let path = Path::new("/usr/lib/ladspa/multi.so");
        assert_ne!(
            key(PluginFormat::Ladspa, path, "delay"),
            key(PluginFormat::Ladspa, path, "reverb"),
        );
        assert_ne!(
            key(PluginFormat::Ladspa, path, "delay"),
            key(PluginFormat::Dssi, path, "delay"),
        );
    }
}
