//! Trying a plugin in a child process tells us how it fails.
//!
//! Custom harness (`harness = false`), same reason as `scan_isolation.rs`: the
//! probe re-runs *this* executable, so it has to answer `probe_worker_main`
//! before anything else.

use choz_engine::PluginFormat;
use choz_engine::quarantine::{Verdict, check};

fn main() {
    // Any of the three worker roles: the engine re-runs this binary for all
    // of them, and answering only one means the others re-enter the test.
    if choz_engine::worker_main() {
        return;
    }
    // Keep the verdict cache (and any temp file) out of the real state dir.
    let state = std::env::temp_dir().join(format!("choz_quarantine_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&state);
    std::fs::create_dir_all(&state).unwrap();
    unsafe { std::env::set_var("XDG_STATE_HOME", &state) };

    let good = std::path::Path::new("/usr/lib/lv2/amp.lv2");
    if good.exists() {
        let v = check(PluginFormat::Lv2, good, "urn:ardour:a-amplifier").verdict;
        assert_ne!(v, Verdict::CrashesOnLoad, "a working plugin must stay loadable");
    }

    // padthv1 plays fine and then segfaults in its own Qt thread while being
    // destroyed. What the probe must never do is call that a *load* failure:
    // the difference is what decides between refusing the plugin and leaking it.
    //
    // It is deliberately not asserted that the verdict *is* `CrashesOnTeardown`.
    // Measured over repeated runs, the same probe comes back `Ok` about one time
    // in three: the crash is a race between padthv1's Qt thread and `cleanup`,
    // and the child sometimes wins it. `LEAKY_URIS` starts empty in every child,
    // so the instance really is destroyed each time — the non-determinism is the
    // plugin's, not the probe's. See "Pendiente" in docs/roadmap.md: a probe
    // that only samples a racy crash once will sometimes clear a plugin that
    // can still take the app down.
    let padthv1 = std::path::Path::new("/usr/lib/lv2/padthv1.lv2");
    if padthv1.exists() {
        let v = check(PluginFormat::Lv2, padthv1, "http://padthv1.sourceforge.net/lv2").verdict;
        assert_ne!(v, Verdict::CrashesOnLoad, "padthv1 dies on the way out, not in");
        assert!(v.loadable(), "it still plays, so it is allowed");
        // Second call comes from the cache: no second child, same answer.
        assert_eq!(check(PluginFormat::Lv2, padthv1, "http://padthv1.sourceforge.net/lv2").verdict, v);
    } else {
        eprintln!("padthv1 not installed; skipping the teardown-crash check");
    }

    let _ = std::fs::remove_dir_all(&state);
    println!("test a_crashing_plugin_is_classified_by_where_it_died ... ok");
}
