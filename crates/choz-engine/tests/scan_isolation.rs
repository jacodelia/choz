//! A plugin that segfaults on load must cost only itself, not the scan.
//!
//! Custom harness (`harness = false`): the point of the test is that
//! `scan_all` re-runs *this executable* as a scan worker, which means the
//! binary has to answer [`choz_engine::scan_worker_main`] before doing anything
//! else — exactly like the real `choz` binary does.

use choz_engine::{PluginFormat, PluginPaths, SearchDir, scan_all};

fn main() {
    // Any of the three worker roles: the engine re-runs this binary for all
    // of them, and answering only one means the others re-enter the test.
    if choz_engine::worker_main() {
        return;
    }

    // The bad "plugin" is a shared object whose constructor dereferences null,
    // dropped next to a real one. Skipped without cc or without a VST2 plugin.
    let good = std::path::Path::new("/usr/lib/vst/ZamComp-vst.so");
    if !good.exists() {
        eprintln!("no VST2 plugin to pair with; skipping");
        return;
    }
    let dir = std::env::temp_dir().join(format!("choz_badscan_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("bad.c");
    std::fs::write(
        &src,
        "__attribute__((constructor)) static void boom(void){ volatile int*p=0; *p=1; }",
    )
    .unwrap();
    let built = std::process::Command::new("cc")
        .args(["-shared", "-fPIC", "-o"])
        .arg(dir.join("bad.so"))
        .arg(&src)
        .status()
        .is_ok_and(|s| s.success());
    if !built {
        eprintln!("no working cc; skipping");
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    std::fs::copy(good, dir.join("good.so")).unwrap();

    let mut paths = PluginPaths { entries: Vec::new() };
    paths
        .dirs_mut(PluginFormat::Vst2)
        .push(SearchDir { path: dir.clone(), enabled: true });
    let found = scan_all(&paths);
    let _ = std::fs::remove_dir_all(&dir);

    // Reaching this line at all is half the test: in-process, `bad.so` would
    // have taken the whole run down with it.
    assert_eq!(found.len(), 1, "expected only the good plugin: {found:?}");
    assert!(found[0].path.ends_with("good.so"), "{found:?}");
    println!("test a_plugin_that_crashes_on_load_only_costs_itself ... ok");
}
