//! `packaging/install.sh` end to end, into a throwaway prefix.
//!
//! The script deletes binaries, so it is exactly the kind of thing that must be
//! run before it is trusted. `CHOZ_SEARCH_BINS` is set empty here so nothing
//! outside the temporary prefix is ever looked at, let alone removed.

use std::path::{Path, PathBuf};
use std::process::Command;

fn script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/install.sh")
}

fn run(prefix: &Path, home: &Path, args: &[&str]) -> String {
    let out = Command::new("sh")
        .arg(script())
        .arg("--prefix")
        .arg(prefix)
        .args(args)
        .env("HOME", home)
        .env("CHOZ_SEARCH_BINS", "")
        .output()
        .expect("sh is installed");
    assert!(
        out.status.success(),
        "install.sh failed: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn the_installer_upgrades_in_place_and_never_touches_the_user_state() {
    let tmp = std::env::temp_dir().join(format!("choz_install_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let (prefix, home) = (tmp.join("prefix"), tmp.join("home"));
    std::fs::create_dir_all(&home).unwrap();

    // The one thing an uninstall must leave alone.
    let state = home.join(".local/state/choz");
    std::fs::create_dir_all(&state).unwrap();
    let project = state.join("my-song.choz.yml");
    std::fs::write(&project, b"slots: []\n").unwrap();

    let binary = env!("CARGO_BIN_EXE_choz");
    let installed = prefix.join("bin/choz");
    let files = [
        "bin/choz",
        "bin/choz-launcher",
        "share/applications/choz.desktop",
        "share/icons/hicolor/scalable/apps/choz.svg",
        "share/mime/packages/choz-project.xml",
    ];

    let out = run(&prefix, &home, &["--binary", binary]);
    assert!(out.contains("installed choz "), "it reports the version it put there: {out}");
    for f in files {
        assert!(prefix.join(f).exists(), "{f} was not installed");
    }

    // Installing again is an upgrade: the old copy goes before the new one
    // lands, rather than the two sitting side by side.
    let out = run(&prefix, &home, &["--binary", binary]);
    assert!(
        out.contains(&format!("removed {}", installed.display())),
        "an upgrade removes the previous binary first: {out}"
    );
    assert!(installed.exists(), "and puts the new one back");

    let out = run(&prefix, &home, &["--uninstall"]);
    for f in files {
        assert!(!prefix.join(f).exists(), "{f} survived the uninstall");
    }
    assert!(out.contains("left alone"), "and it says what it did not remove");
    assert!(project.exists(), "the user's projects are not part of the package");
    assert_eq!(std::fs::read(&project).unwrap(), b"slots: []\n");

    let _ = std::fs::remove_dir_all(&tmp);
}

/// An installer decides whether to replace what is on disk by asking it, so the
/// binary has to answer `--version` without starting a terminal.
#[test]
fn the_binary_answers_version_and_help_on_stdout() {
    let out = Command::new(env!("CARGO_BIN_EXE_choz")).arg("--version").output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.starts_with("choz "), "got {text:?}");
    assert!(text.trim().split(' ').nth(1).is_some_and(|v| v.contains('.')), "a version number");

    let out = Command::new(env!("CARGO_BIN_EXE_choz")).arg("-h").output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("--osc-port"));
}
