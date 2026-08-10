//! What actually ends up in the `.deb` and the `.rpm`.
//!
//! This exists because of a bug that produced no error anywhere. The `assets`
//! list had drifted into `[package.metadata.deb.variants.arm]`, and a variant
//! inherits from the base table rather than the other way round — so the
//! ordinary x86_64 package contained the binary and nothing else. No desktop
//! entry, no icon, no launcher. cargo-deb said nothing, the package installed
//! cleanly, and choz simply never appeared in the application menu. The only
//! way to see it was `dpkg-deb -c` on the built package.
//!
//! Cheap to check here, so it is checked here: the manifest is read and every
//! declared source is required to exist, in the table it is required to be in.
//!
//! There is no TOML parser in this workspace and this test is not worth adding
//! one for. It reads the manifest as text, split on section headers — enough to
//! answer "which table is this line in", which is the entire question.

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

/// The body of `[header]`, up to the next section header.
fn section(manifest: &str, header: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for line in manifest.lines() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            inside = t == format!("[{header}]");
            continue;
        }
        if inside {
            out.push_str(line);
            out.push('\n');
        }
    }
    assert!(!out.is_empty(), "[{header}] is missing or empty");
    out
}

fn manifest() -> String {
    std::fs::read_to_string(root().join("Cargo.toml")).expect("read Cargo.toml")
}

/// The files that make choz an application rather than a binary, by their
/// destination. A package missing any of these installs and disappears.
/// The 48px raster is in here on purpose. GTK 3 loads theme icons through
/// gdk-pixbuf, and librsvg 2.61 dropped the gdk-pixbuf SVG loader, so a package
/// carrying only `scalable/` installs an icon that does not render — the menu
/// draws its generic cog and everything looks correctly installed.
const REQUIRED_DESTINATIONS: [&str; 5] = [
    "usr/bin/",
    "usr/share/applications/",
    "usr/share/icons/hicolor/scalable/apps/",
    "usr/share/icons/hicolor/48x48/apps/",
    "usr/share/mime/packages/",
];

#[test]
fn the_deb_carries_the_desktop_files_in_the_base_table() {
    // In the *base* table. Declared only in a variant these reach the ARM
    // package and no other, which is how the bug hid.
    let deb = section(&manifest(), "package.metadata.deb");
    assert!(
        deb.contains("assets = ["),
        "assets belong in [package.metadata.deb], not in a variant",
    );

    for dest in REQUIRED_DESTINATIONS {
        assert!(
            deb.contains(&format!("\"{dest}")),
            "the .deb would install nothing into {dest}",
        );
    }

    let sources = sources(&deb);
    assert!(
        sources.iter().any(|s| s == "target/release/choz"),
        "the .deb does not carry the binary",
    );

    for source in sources {
        // The binary is the one path cargo-deb resolves itself: written exactly
        // like this it is recognised as the Cargo target dir and rewritten under
        // `--target`, which is what keeps the host binary out of an ARM package.
        // It only exists after a release build, so it is not checked for here.
        if source == "target/release/choz" {
            continue;
        }
        assert!(
            root().join(&source).exists(),
            "{source} is declared in the .deb and does not exist",
        );
    }
}

/// Every asset source in a section: the first quoted string on a line inside an
/// `assets` list, in either the `["src", "dest", "mode"]` or
/// `{ source = "src", … }` spelling.
fn sources(section: &str) -> Vec<String> {
    section
        .lines()
        .filter_map(|line| {
            let t = line.trim();
            let rest = t
                .strip_prefix('[')
                .or_else(|| t.strip_prefix("{ source = "))?;
            let rest = rest.strip_prefix('"')?;
            Some(rest.split('"').next()?.to_string())
        })
        .collect()
}

#[test]
fn the_rpm_carries_them_too() {
    let rpm = section(&manifest(), "package.metadata.generate-rpm");
    for dest in REQUIRED_DESTINATIONS {
        assert!(
            rpm.contains(&format!("\"/{dest}")),
            "the .rpm would install nothing into {dest}",
        );
    }
    for source in sources(&rpm) {
        if source.ends_with("target/release/choz") {
            continue;
        }
        assert!(
            root().join(&source).exists(),
            "{source} is declared in the .rpm and does not exist",
        );
    }
}

/// Both formats have to refresh the desktop, MIME and icon caches after
/// unpacking. Debian ships triggers that usually do it, and "usually" is how an
/// application installs and never appears in the menu.
#[test]
fn both_formats_refresh_the_caches_after_install() {
    let manifest = manifest();
    let deb = section(&manifest, "package.metadata.deb");

    let dir = deb
        .lines()
        .find_map(|l| l.trim().strip_prefix("maintainer-scripts = "))
        .expect("maintainer-scripts")
        .trim()
        .trim_matches('"')
        .to_string();
    for script in ["postinst", "postrm"] {
        let path = root().join(&dir).join(script);
        let body =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert!(
            body.contains("update-desktop-database"),
            "{script} does not refresh the menu",
        );
    }

    let rpm = section(&manifest, "package.metadata.generate-rpm");
    for key in ["post_install_script", "post_uninstall_script"] {
        assert!(rpm.contains(key), "the .rpm has no {key}");
    }
    assert!(
        rpm.matches("update-desktop-database").count() >= 2,
        "the .rpm's scriptlets do not refresh the menu",
    );
}

/// The menu entry itself: the categories decide *where* it lands, and the user
/// asked for the multimedia/synthesiser section.
#[test]
fn the_desktop_entry_lands_under_audio() {
    let entry = std::fs::read_to_string(root().join("../../packaging/desktop/choz.desktop"))
        .expect("read choz.desktop");
    let categories = entry
        .lines()
        .find_map(|l| l.strip_prefix("Categories="))
        .expect("Categories=");
    // AudioVideo is the top-level section every menu implementation reads; Audio
    // is what puts it under multimedia rather than loose beside it.
    for required in ["AudioVideo", "Audio"] {
        assert!(
            categories.split(';').any(|c| c == required),
            "Categories is missing {required}: {categories}",
        );
    }
    assert_eq!(
        entry.lines().find_map(|l| l.strip_prefix("Icon=")),
        Some("choz"),
        "the icon name has to match the installed choz.svg",
    );
}
