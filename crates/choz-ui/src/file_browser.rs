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

/// Extension set that means "pick a directory, not a file" — the browser then
/// offers the current directory itself as the first entry.
pub const DIR_PICK: &[&str] = &["<dir>"];

/// What counts as a background image. Decoding is `image`'s problem; this is
/// only what the browser lists.
pub const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "bmp", "gif", "webp"];

fn is_dir_pick(exts: &[&str]) -> bool {
    exts == DIR_PICK
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
                dirs.push(Entry {
                    label: format!("{name}/"),
                    path,
                    is_dir: true,
                });
            } else if !is_dir_pick(exts) && exts.iter().any(|e| has_ext(&path, e)) {
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

        let labels: Vec<_> = scan(&base, &["wav"]).into_iter().map(|e| e.label).collect();
        assert_eq!(labels, vec!["../", "sub/", "a.wav", "b.wav"]);

        std::fs::remove_dir_all(&base).unwrap();
    }
}
