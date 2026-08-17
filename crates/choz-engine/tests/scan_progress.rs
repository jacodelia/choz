//! `scan_all_with_progress` has to report one step per directory it is about
//! to walk, in order, counting from zero — that is what the UI's progress bar
//! divides by.
//!
//! Every directory here is an empty temp dir, so nothing is ever dlopened and
//! this file cannot collide with the real scan in `plugin_scan.rs`.

use choz_engine::{scan_all_with_progress, PluginFormat, PluginPaths, ScanStep, SearchDir};

/// Empty search dirs, `n` of them enabled plus `disabled` that are switched off.
fn paths(n: usize, disabled: usize) -> (PluginPaths, Vec<std::path::PathBuf>) {
    let root = std::env::temp_dir().join(format!("choz_progress_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let mut made = Vec::new();
    let mut paths = PluginPaths {
        entries: Vec::new(),
    };
    for i in 0..(n + disabled) {
        let dir = root.join(format!("d{i}"));
        std::fs::create_dir_all(&dir).unwrap();
        made.push(dir.clone());
        paths.dirs_mut(PluginFormat::Lv2).push(SearchDir {
            path: dir,
            enabled: i < n,
        });
    }
    (paths, made)
}

#[test]
fn every_enabled_directory_reports_one_step_and_disabled_ones_report_none() {
    let (p, _dirs) = paths(4, 2);
    let mut steps: Vec<(usize, usize)> = Vec::new();
    let mut seen: Vec<std::path::PathBuf> = Vec::new();
    scan_all_with_progress(&p, |s: ScanStep<'_>| {
        steps.push((s.done, s.total));
        seen.push(s.dir.to_path_buf());
    });

    assert_eq!(
        steps,
        vec![(0, 4), (1, 4), (2, 4), (3, 4)],
        "four enabled dirs, counting from zero, total excluding the disabled two"
    );
    assert_eq!(seen.len(), 4);
    // The report comes *before* the directory is walked, so the last one is
    // `done == total - 1`; `done == total` is never sent.
    assert!(steps.iter().all(|(d, t)| d < t));

    let _ = std::fs::remove_dir_all(
        std::env::temp_dir().join(format!("choz_progress_{}", std::process::id())),
    );
}

#[test]
fn percent_runs_from_zero_and_a_run_with_no_directories_is_already_done() {
    let dir = std::path::Path::new("/nonexistent");
    let at = |done, total| {
        ScanStep {
            done,
            total,
            format: PluginFormat::Lv2,
            dir,
        }
        .percent()
    };
    assert_eq!(at(0, 4), 0);
    assert_eq!(at(2, 4), 50);
    assert_eq!(at(3, 4), 75);
    // Nothing to scan is 100 %, not a divide by zero.
    assert_eq!(at(0, 0), 100);
}

#[test]
fn a_scan_with_nothing_enabled_reports_no_steps_at_all() {
    let (p, _dirs) = paths(0, 3);
    let mut n = 0;
    scan_all_with_progress(&p, |_| n += 1);
    assert_eq!(n, 0, "nothing enabled means no work and no progress");
    let _ = std::fs::remove_dir_all(
        std::env::temp_dir().join(format!("choz_progress_{}", std::process::id())),
    );
}
