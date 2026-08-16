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
    run_with_path(prefix, home, args, None)
}

fn run_with_path(prefix: &Path, home: &Path, args: &[&str], path_prefix: Option<&Path>) -> String {
    let (out, ok) = try_run(prefix, home, args, path_prefix);
    assert!(ok, "install.sh failed: {out}");
    out
}

/// The output and whether it succeeded — a refusal is a result here, not a bug.
fn try_run(
    prefix: &Path,
    home: &Path,
    args: &[&str],
    path_prefix: Option<&Path>,
) -> (String, bool) {
    let mut cmd = Command::new("sh");
    cmd.arg(script())
        .arg("--prefix")
        .arg(prefix)
        .args(args)
        .env("HOME", home)
        .env("CHOZ_SEARCH_BINS", "");
    if let Some(dir) = path_prefix {
        let path = std::env::var("PATH").unwrap_or_default();
        cmd.env("PATH", format!("{}:{path}", dir.display()));
    }
    let out = cmd.output().expect("sh is installed");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    (text, out.status.success())
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
    assert!(
        out.contains("installed choz "),
        "it reports the version it put there: {out}"
    );
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
    assert!(
        out.contains("left alone"),
        "and it says what it did not remove"
    );
    assert!(
        project.exists(),
        "the user's projects are not part of the package"
    );
    assert_eq!(std::fs::read(&project).unwrap(), b"slots: []\n");

    let _ = std::fs::remove_dir_all(&tmp);
}

/// An installer decides whether to replace what is on disk by asking it, so the
/// binary has to answer `--version` without starting a terminal.
#[test]
fn the_binary_answers_version_and_help_on_stdout() {
    let out = Command::new(env!("CARGO_BIN_EXE_choz"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.starts_with("choz "), "got {text:?}");
    assert!(
        text.trim()
            .split(' ')
            .nth(1)
            .is_some_and(|v| v.contains('.')),
        "a version number"
    );

    let out = Command::new(env!("CARGO_BIN_EXE_choz"))
        .arg("-h")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("--osc-port"));
}

/// choz needs ALSA on the machine that runs it, and only *dlopens* libjack — so
/// a missing ALSA is fatal and a missing JACK is a note.
///
/// Refusing matters more than warning: a choz installed without ALSA starts,
/// opens no audio device, and looks like a bug in choz. The escape hatch exists
/// for the one case where continuing is right — staging an install for a
/// machine that is not this one.
#[test]
fn the_installer_refuses_when_a_critical_library_is_missing() {
    let tmp = std::env::temp_dir().join(format!("choz_deps_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let (prefix, home, fake_bin) = (tmp.join("prefix"), tmp.join("home"), tmp.join("bin"));
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&fake_bin).unwrap();

    // An `ldconfig` that reports no libraries at all: the machine has neither.
    let ldconfig = fake_bin.join("ldconfig");
    std::fs::write(&ldconfig, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&ldconfig, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let (out, ok) = try_run(
        &prefix,
        &home,
        &["--binary", env!("CARGO_BIN_EXE_choz")],
        Some(&fake_bin),
    );
    assert!(!ok, "a missing ALSA has to fail, not warn: {out}");
    assert!(
        out.contains("libasound.so.2 (ALSA) is missing"),
        "and say which one: {out}"
    );
    assert!(out.contains("apt install"), "and how to fix it: {out}");
    assert!(
        !prefix.join("bin/choz").exists(),
        "nothing is installed when it refuses"
    );

    // The escape hatch installs, and says it skipped the check rather than
    // pretending it passed.
    let out = run_with_path(
        &prefix,
        &home,
        &["--binary", env!("CARGO_BIN_EXE_choz"), "--skip-deps-check"],
        Some(&fake_bin),
    );
    assert!(
        out.contains("skipping the runtime dependency check"),
        "{out}"
    );
    assert!(prefix.join("bin/choz").exists(), "and it installs");

    let _ = std::fs::remove_dir_all(&tmp);
}

/// In a release tarball the script sits next to the binary it should install.
/// Without this, a user who downloaded a `.tar.gz` gets `cargo build` — a
/// toolchain they have no reason to have — instead of the binary in their hand.
#[test]
fn the_installer_uses_the_binary_shipped_beside_it() {
    let tmp = std::env::temp_dir().join(format!("choz_tarball_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let (prefix, home, dist) = (tmp.join("prefix"), tmp.join("home"), tmp.join("dist"));
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&dist).unwrap();

    // The tarball layout the release workflow builds: script, binary, desktop/.
    let packaging = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging");
    std::fs::copy(script(), dist.join("install.sh")).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_choz"), dist.join("choz")).unwrap();
    copy_tree(&packaging.join("desktop"), &dist.join("desktop"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dist.join("choz"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
    }

    let out = Command::new("/bin/sh")
        .arg(dist.join("install.sh"))
        .arg("--prefix")
        .arg(&prefix)
        .arg("--skip-deps-check")
        .env("HOME", &home)
        .env("CHOZ_SEARCH_BINS", "")
        .output()
        .expect("sh is installed");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "install.sh failed: {text}");
    assert!(
        text.contains("shipped next to this script"),
        "it says where the binary came from: {text}"
    );
    assert!(prefix.join("bin/choz").exists(), "and installs it: {text}");

    let _ = std::fs::remove_dir_all(&tmp);
}

/// choz's own effects go where other hosts look, **by default**, and
/// `--uninstall` takes them away again.
///
/// They are choz's own DSP rather than somebody else's plugin, so a default
/// install ships them; `--no-clap` is for whoever does not want them. The
/// bundle is prebuilt here — building it inside a test would cost a release
/// compile of the whole engine.
#[test]
fn the_clap_bundle_is_installed_by_default_and_removable() {
    let tmp = std::env::temp_dir().join(format!("choz_clap_install_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let (prefix, home) = (tmp.join("prefix"), tmp.join("home"));
    std::fs::create_dir_all(&home).unwrap();
    // Stand in for the built cdylib: the script only copies it.
    let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/release");
    std::fs::create_dir_all(&target).unwrap();
    let bundle = target.join("libchoz_plugin_clap_export.so");
    let existed = bundle.exists();
    if !existed {
        std::fs::write(&bundle, b"not really a plugin").unwrap();
    }

    let out = run(
        &prefix,
        &home,
        &["--binary", env!("CARGO_BIN_EXE_choz"), "--skip-deps-check"],
    );
    let installed = home.join(".clap/choz.clap");
    assert!(installed.exists(), "the bundle is installed: {out}");
    assert!(out.contains("45 effects"), "and it says so: {out}");
    // And the wallpapers, which is what a fresh install opens with.
    let wallpaper = prefix.join("share/choz/wallpapers/wallpaper.jpg");
    assert!(wallpaper.exists(), "the wallpapers ship too: {out}");

    let out = run(&prefix, &home, &["--uninstall"]);
    assert!(!installed.exists(), "and uninstall takes it away: {out}");
    assert!(!wallpaper.exists(), "wallpapers too: {out}");

    // Whoever does not want the plugin says so.
    let out = run(
        &prefix,
        &home,
        &[
            "--binary",
            env!("CARGO_BIN_EXE_choz"),
            "--skip-deps-check",
            "--no-clap",
        ],
    );
    assert!(!installed.exists(), "--no-clap means no plugin: {out}");

    if !existed {
        let _ = std::fs::remove_file(&bundle);
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        if src.is_dir() {
            copy_tree(&src, &dst);
        } else {
            std::fs::copy(&src, &dst).unwrap();
        }
    }
}

/// JACK is the other half of the same check and must **not** stop anything: it
/// is `dlopen`ed, so without it choz runs on ALSA.
#[test]
fn a_missing_jack_is_a_note_not_a_refusal() {
    let tmp = std::env::temp_dir().join(format!("choz_jack_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let (prefix, home, fake_bin) = (tmp.join("prefix"), tmp.join("home"), tmp.join("bin"));
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&fake_bin).unwrap();

    // An `ldconfig` that reports ALSA and nothing else.
    let ldconfig = fake_bin.join("ldconfig");
    std::fs::write(
        &ldconfig,
        "#!/bin/sh\necho '\tlibasound.so.2 (libc6,x86-64) => /usr/lib/libasound.so.2'\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&ldconfig, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let out = run_with_path(
        &prefix,
        &home,
        &["--binary", env!("CARGO_BIN_EXE_choz")],
        Some(&fake_bin),
    );
    assert!(
        out.contains("libjack is not installed"),
        "no JACK note: {out}"
    );
    assert!(
        out.contains("will use ALSA"),
        "and it says what happens instead: {out}"
    );
    assert!(
        prefix.join("bin/choz").exists(),
        "JACK is optional, so this installs"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
