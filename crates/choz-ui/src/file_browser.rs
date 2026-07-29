//! Minimal WAV file browser modal.
//!
//! Lists the parent dir, subdirectories, and `*.wav` files under `dir`.
//! Arrow keys move, Enter descends into a dir or picks a file.

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
    /// File extension to list (lowercase, no dot), e.g. "wav" or "sf2".
    ext: &'static str,
}

/// What pressing Enter on the current entry resolves to.
pub enum Action {
    /// Descend into a directory (browser stays open, re-scanned).
    EnterDir(PathBuf),
    /// A WAV file was chosen.
    PickFile(PathBuf),
}

impl FileBrowser {
    /// `ext` is the file extension to list (lowercase, no dot).
    pub fn open(start: &Path, ext: &'static str) -> Self {
        let dir = start.to_path_buf();
        let entries = scan(&dir, ext);
        Self { dir, entries, cursor: 0, scroll: 0, ext }
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
        self.entries = scan(&dir, self.ext);
        self.dir = dir;
        self.cursor = 0;
        self.scroll = 0;
    }
}

/// Extension that means "pick a directory, not a file" — the browser then
/// offers the current directory itself as the first entry.
pub const DIR_PICK: &str = "<dir>";

/// Directories first (with `..` on top), then matching files, each alphabetical.
fn scan(dir: &Path, ext: &str) -> Vec<Entry> {
    let mut dirs: Vec<Entry> = Vec::new();
    let mut files: Vec<Entry> = Vec::new();

    if ext == DIR_PICK {
        // Picking a directory needs a way to say "this one": it is a file entry
        // so `select()` resolves it to PickFile rather than descending.
        dirs.push(Entry {
            label: format!("[use {}]", dir.display()),
            path: dir.to_path_buf(),
            is_dir: false,
        });
    }

    if let Some(parent) = dir.parent() {
        dirs.push(Entry { label: "../".to_string(), path: parent.to_path_buf(), is_dir: true });
    }

    if let Ok(rd) = std::fs::read_dir(dir) {
        for ent in rd.flatten() {
            let path = ent.path();
            let name = ent.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue; // skip hidden
            }
            if path.is_dir() {
                dirs.push(Entry { label: format!("{name}/"), path, is_dir: true });
            } else if ext != DIR_PICK && has_ext(&path, ext) {
                files.push(Entry { label: name, path, is_dir: false });
            }
        }
    }

    // `..` (and the "use this directory" entry) stay first.
    let split = usize::from(dir.parent().is_some()) + usize::from(ext == DIR_PICK);
    dirs[split..].sort_by(|a, b| a.label.cmp(&b.label));
    files.sort_by(|a, b| a.label.cmp(&b.label));

    dirs.extend(files);
    dirs
}

fn has_ext(path: &Path, ext: &str) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext))
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

        let labels: Vec<_> = scan(&base, "wav").into_iter().map(|e| e.label).collect();
        assert_eq!(labels, vec!["../", "sub/", "a.wav", "b.wav"]);

        std::fs::remove_dir_all(&base).unwrap();
    }
}
