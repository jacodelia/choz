//! Minimal file browser modal.
//!
//! Lists the parent dir, subdirectories, and the files under `dir` whose
//! extension matches. Arrow keys move, Enter descends into a dir or picks a
//! file. Several extensions can be offered at once (images are `png`/`jpg`/…).

use std::path::{Path, PathBuf};

pub struct Entry {
    pub label: String,
    pub path: PathBuf,
    pub is_dir: bool,
}

pub struct FileBrowser {
    pub dir: PathBuf,
    pub entries: Vec<Entry>,
    pub cursor: usize,
    pub scroll: usize,
    /// File extensions to list (lowercase, no dot), e.g. `["wav"]` or the image
    /// set. Empty is never useful; use [`DIR_PICK`] to pick a directory instead.
    exts: &'static [&'static str],
}

/// What pressing Enter on the current entry resolves to.
pub enum Action {
    /// Descend into a directory (browser stays open, re-scanned).
    EnterDir(PathBuf),
    /// A WAV file was chosen.
    PickFile(PathBuf),
}

impl FileBrowser {
    /// `exts` are the file extensions to list (lowercase, no dot).
    pub fn open(start: &Path, exts: &'static [&'static str]) -> Self {
        let dir = start.to_path_buf();
        let entries = scan(&dir, exts);
        Self {
            dir,
            entries,
            cursor: 0,
            scroll: 0,
            exts,
        }
    }

    /// Resolve the current selection. Returns `None` if the list is empty.
    pub fn select(&self) -> Option<Action> {
        let e = self.entries.get(self.cursor)?;
        if e.is_dir {
            Some(Action::EnterDir(e.path.clone()))
        } else {
            Some(Action::PickFile(e.path.clone()))
        }
    }

    /// Re-scan after entering a new directory.
    pub fn set_dir(&mut self, dir: PathBuf) {
        self.entries = scan(&dir, self.exts);
        self.dir = dir;
        self.cursor = 0;
        self.scroll = 0;
    }
}

/// What says "this set picks a directory". Not an extension anything has.
const DIR_MARK: &str = "<dir>";

/// Extension set that means "pick a directory, not a file" — the browser then
/// offers the current directory itself as the first entry.
pub const DIR_PICK: &[&str] = &[DIR_MARK];

/// The same, for the sampler: a folder, one of the `.zip`s in it, **or a
/// single recording** — the most ordinary thing to hand a sampler. The audio
/// extensions are `sampler::EXTENSIONS`, spelled out because a `const` cannot
/// be concatenated.
///
/// A sample library arrives as one archive per instrument — Philharmonia's is
/// twenty of them in one directory — and choz reads an archive where it sits.
/// The picker that could only answer "this directory" made the user unpack
/// them first, which is the step the whole archive reader exists to avoid.
pub const DIR_OR_ZIP: &[&str] = &[
    DIR_MARK, "zip", "wav", "wave", "flac", "aiff", "aif", "mp3", "ogg",
];

/// What counts as a background image. Decoding is `image`'s problem; this is
/// only what the browser lists.
pub const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "bmp", "gif", "webp"];

fn is_dir_pick(exts: &[&str]) -> bool {
    exts.first() == Some(&DIR_MARK)
}

/// Directories first (with `..` on top), then matching files, each alphabetical.
fn scan(dir: &Path, exts: &[&str]) -> Vec<Entry> {
    let mut dirs: Vec<Entry> = Vec::new();
    let mut files: Vec<Entry> = Vec::new();

    if is_dir_pick(exts) {
        // Picking a directory needs a way to say "this one": it is a file entry
        // so `select()` resolves it to PickFile rather than descending.
        dirs.push(Entry {
            label: format!("[use {}]", dir.display()),
            path: dir.to_path_buf(),
            is_dir: false,
        });
    }

    if let Some(parent) = dir.parent() {
        dirs.push(Entry {
            label: "../".to_string(),
            path: parent.to_path_buf(),
            is_dir: true,
        });
    }

    if let Ok(rd) = std::fs::read_dir(dir) {
        for ent in rd.flatten() {
            let path = ent.path();
            let name = ent.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue; // skip hidden
            }
            if path.is_dir() {
                if is_bundle(&path) {
                    // A plugin bundle is a binary in a directory suit. Asked
                    // where a synth's patches are, `/usr/lib/lv2` answered with
                    // 260 `.lv2` folders and none of them was one.
                    continue;
                }
                dirs.push(Entry {
                    label: format!("{name}/"),
                    path,
                    is_dir: true,
                });
            // The marker is not an extension: a dir-pick set may still name
            // real ones beside it, and then those files are listed and
            // pickable — a folder of sample packs is a folder of `.zip`s, and
            // "pick a folder" with nothing in the list is a dead end.
            } else if exts.iter().any(|e| *e != DIR_MARK && has_ext(&path, e)) {
                files.push(Entry {
                    label: name,
                    path,
                    is_dir: false,
                });
            }
        }
    }

    // `..` (and the "use this directory" entry) stay first.
    let split = usize::from(dir.parent().is_some()) + usize::from(is_dir_pick(exts));
    dirs[split..].sort_by(|a, b| a.label.cmp(&b.label));
    files.sort_by(|a, b| a.label.cmp(&b.label));

    dirs.extend(files);
    dirs
}

/// Directory extensions that are a plugin, not a folder of patches.
const BUNDLE_EXTS: &[&str] = &["lv2", "vst3", "clap", "vst", "component"];

fn is_bundle(path: &Path) -> bool {
    BUNDLE_EXTS.iter().any(|e| has_ext(path, e))
}

fn has_ext(path: &Path, ext: &str) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_lists_dirs_then_wavs_with_parent_first() {
        let base = std::env::temp_dir().join(format!("choz_fb_{}", std::process::id()));
        let sub = base.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(base.join("b.wav"), b"x").unwrap();
        std::fs::write(base.join("a.wav"), b"x").unwrap();
        std::fs::write(base.join("note.txt"), b"x").unwrap(); // ignored

        // A plugin bundle is not a folder anyone is browsing for.
        std::fs::create_dir_all(base.join("Synth.lv2")).unwrap();

        let labels: Vec<_> = scan(&base, &["wav"]).into_iter().map(|e| e.label).collect();
        assert_eq!(labels, vec!["../", "sub/", "a.wav", "b.wav"]);

        std::fs::remove_dir_all(&base).unwrap();
    }

    /// Picking a folder of sample packs has to show the packs: a library is one
    /// `.zip` per instrument in one directory, and a picker that only offered
    /// "use this directory" made the user unpack them first — the step the
    /// whole archive reader exists to avoid.
    #[test]
    fn the_sampler_picker_lists_folders_and_archives() {
        let base = std::env::temp_dir().join(format!("choz_fb_zip_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("loose")).unwrap();
        std::fs::write(base.join("violin.zip"), b"x").unwrap();
        std::fs::write(base.join("cello.ZIP"), b"x").unwrap();
        std::fs::write(base.join("notes.txt"), b"x").unwrap();
        std::fs::write(base.join("take.wav"), b"x").unwrap();

        let b = FileBrowser::open(&base, DIR_OR_ZIP);
        let labels: Vec<&str> = b.entries.iter().map(|e| e.label.as_str()).collect();
        assert!(labels[0].starts_with("[use "), "{labels:?}");
        assert!(labels.contains(&"loose/"), "{labels:?}");
        // Both archives, whatever case they spell their extension in.
        assert!(labels.contains(&"violin.zip"), "{labels:?}");
        assert!(labels.contains(&"cello.ZIP"), "{labels:?}");
        // A single recording is an instrument too.
        assert!(labels.contains(&"take.wav"), "{labels:?}");
        // …and nothing else.
        assert!(!labels.contains(&"notes.txt"), "{labels:?}");

        // A zip resolves to "picked", not to "descend into".
        let at = b
            .entries
            .iter()
            .position(|e| e.label == "violin.zip")
            .unwrap();
        let mut b = b;
        b.cursor = at;
        assert!(matches!(b.select(), Some(Action::PickFile(p)) if p.ends_with("violin.zip")));

        // The plain directory picker is what it was: folders only.
        let plain = FileBrowser::open(&base, DIR_PICK);
        assert!(
            !plain.entries.iter().any(|e| e.label.ends_with(".zip")),
            "the directory picker grew files"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// The picker offers exactly the audio the sampler reads: a format added to
    /// one list and not the other is a file that cannot be picked, or one that
    /// is picked and then refused.
    #[test]
    fn the_sampler_picker_offers_every_format_the_sampler_reads() {
        let offered: Vec<&str> = DIR_OR_ZIP
            .iter()
            .copied()
            .filter(|e| *e != DIR_MARK && *e != "zip")
            .collect();
        assert_eq!(
            offered,
            choz_engine::instruments::sampler::EXTENSIONS,
            "the picker and the sampler disagree about what a sample is"
        );
    }
}
