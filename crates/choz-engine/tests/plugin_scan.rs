//! The multi-format scan against whatever is installed on this machine.
//! Skips silently when a format has nothing installed.
//!
//! **One test function on purpose.** A test binary is not a scan worker, so
//! `worker_available()` says no and `scan_all` falls back to scanning in
//! process. Two tests calling it at once means two threads dlopening the same
//! plugins, and the JUCE/VST3 ones do global init on load — that segfaulted the
//! whole test binary (signal 11) about one run in three. The harness
//! parallelises by function, so one function is what serialises it. It also
//! halves the runtime: `scan_all` costs ~6 s and now runs once.

use choz_engine::{FoundPlugin, PluginFormat, PluginPaths, scan_all};

#[test]
fn the_scan_reports_real_metadata_for_every_installed_format() {
    let found = scan_all(&PluginPaths::default());
    real_lv2_metadata(&found);
    hosted_formats_report_what_is_installed(&found);
}

fn real_lv2_metadata(found: &[FoundPlugin]) {
    let lv2: Vec<_> = found.iter().filter(|p| p.format == PluginFormat::Lv2).collect();
    if lv2.is_empty() {
        eprintln!("no LV2 plugins installed; skipping");
        return;
    }
    // Real metadata means a URI as the id (not the empty string the
    // filename-only scanner leaves behind) and a bundle directory as the path.
    for p in &lv2 {
        assert!(!p.id.is_empty(), "{} has no URI", p.name);
        assert!(
            p.path.extension().is_some_and(|e| e == "lv2"),
            "{} is not a bundle dir: {}",
            p.name,
            p.path.display()
        );
    }
    assert!(
        lv2.iter().any(|p| p.is_instrument),
        "expected at least one LV2 instrument among {} plugins",
        lv2.len()
    );
}

/// Every format choz claims to host must actually produce entries when plugins
/// of that format are installed — this is the check that would have caught
/// "I don't see my LV2/VST/DSSI plugins".
fn hosted_formats_report_what_is_installed(found: &[FoundPlugin]) {
    for &fmt in PluginFormat::ALL.iter().filter(|f| f.is_hosted() && f.is_plugin()) {
        let of_format: Vec<_> = found.iter().filter(|p| p.format == fmt).collect();
        if of_format.is_empty() {
            eprintln!("no {} plugins installed; skipping", fmt.label());
            continue;
        }
        assert!(
            of_format.iter().all(|p| !p.name.is_empty()),
            "{} entries must be named",
            fmt.label()
        );
        assert!(
            of_format.iter().all(|p| p.path.exists()),
            "{} entries must point at a real file",
            fmt.label()
        );
    }
}
