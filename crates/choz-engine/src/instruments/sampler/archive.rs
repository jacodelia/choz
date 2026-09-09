//! Sample packs that arrive as one zip per instrument, read where they sit.
//!
//! Philharmonia ships `violin.zip`, `cello.zip`, twenty of them, and every
//! other free library does the same. Making somebody unpack 350 MB before choz
//! will look at it is a step that exists for no reason: a zip's central
//! directory lists every file in it, and pulling one entry out is a seek and a
//! decompress.
//!
//! ## The path of something inside a zip
//!
//! A sample in an archive is named the obvious way —
//! `~/Samples/violin.zip/violin_A3_15_forte_arco-normal.mp3` — a path that no
//! `open` will ever accept and that everything else here can carry unchanged.
//! [`split`] takes it apart again when the bytes are actually wanted. That is
//! what keeps the archive out of the rest of the sampler: the scanner, the
//! name parser, the mapper and the regions never learn that zips exist.
//!
//! ## What this deliberately does not do
//!
//! No unpacking to a cache directory, anywhere, ever. The alternative design —
//! extract on load, then treat it as a folder — is less code here and hundreds
//! of megabytes of somebody else's disk, silently, for a library they already
//! have. Reading the entry costs a decompress that the decode dwarfs.

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Whether this path names an archive rather than a plain file.
pub fn is_archive(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
}

/// Take a path apart into the archive it lives in and the entry inside it, or
/// `None` when the path is an ordinary file.
///
/// Walks from the root rather than looking at the last `.zip` in the string:
/// an entry inside an archive can perfectly well be named `something.zip`, and
/// the outermost archive is the one that can be opened.
pub fn split(path: &Path) -> Option<(PathBuf, String)> {
    let mut archive = PathBuf::new();
    let mut components = path.components();
    for component in components.by_ref() {
        archive.push(component);
        if is_archive(&archive) {
            let entry: PathBuf = components.collect();
            let entry = entry.to_string_lossy().into_owned();
            return (!entry.is_empty()).then_some((archive, entry));
        }
    }
    None
}

/// Every entry in `archive` that looks like a sample, as the path it will be
/// known by and the size it decompresses to. With a `prefix`, only what lives
/// under that folder inside the archive.
///
/// Errors are swallowed on purpose: an unreadable or truncated zip in a folder
/// of good ones is one instrument missing, not a scan that stops.
pub fn samples_under(archive: &Path, prefix: Option<&str>) -> Vec<(PathBuf, u64)> {
    let Ok(file) = std::fs::File::open(archive) else {
        return Vec::new();
    };
    let Ok(mut zip) = zip::ZipArchive::new(std::io::BufReader::new(file)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for index in 0..zip.len() {
        let Ok(entry) = zip.by_index(index) else {
            continue;
        };
        if !entry.is_file() {
            continue;
        }
        // `enclosed_name` is the zip crate's answer to entries that spell their
        // way out of the archive with `..`. A pack that does that is hostile;
        // this drops the entry rather than building a path out of it.
        let Some(name) = entry.enclosed_name() else {
            continue;
        };
        if !super::is_sample(name) {
            continue;
        }
        if let Some(prefix) = prefix {
            if !name.starts_with(prefix) {
                continue;
            }
        }
        out.push((archive.join(name), entry.size()));
    }
    out
}

/// Every sample in the archive.
pub fn samples(archive: &Path) -> Vec<(PathBuf, u64)> {
    samples_under(archive, None)
}

/// What this archive offers to play: itself, when the samples are lying loose
/// inside it, or the folders in it when they are sorted.
///
/// The same rule a directory gets, one level in. Philharmonia's
/// `percussion.zip` is 39 instruments in 39 folders — read as one it is a kit
/// of 148 sounds on 92 keys, with 56 of them dropped off the end of the
/// keyboard for want of somewhere to go.
pub fn instruments(archive: &Path) -> Vec<PathBuf> {
    let samples = samples(archive);
    if samples.is_empty() {
        return Vec::new();
    }
    // Loose at the top: the archive is the instrument.
    let inside = |path: &Path| -> Option<PathBuf> {
        path.strip_prefix(archive)
            .ok()?
            .parent()
            .map(Path::to_path_buf)
    };
    if samples
        .iter()
        .any(|(p, _)| inside(p).is_some_and(|d| d.as_os_str().is_empty()))
    {
        return vec![archive.to_path_buf()];
    }
    let mut folders: Vec<PathBuf> = samples
        .iter()
        .filter_map(|(p, _)| Some(archive.join(inside(p)?.components().next()?)))
        .collect();
    folders.sort();
    folders.dedup();
    folders
}

/// True when the archive holds at least one sample — what makes it an
/// instrument rather than a zip of something else.
///
/// ponytail: opens the archive and reads its central directory, which is one
/// seek and a few KB even for a thousand entries. Caching the answer would
/// matter if a scan met hundreds of zips; a sample library is twenty.
pub fn holds_samples(archive: &Path) -> bool {
    !samples(archive).is_empty()
}

/// One entry's bytes.
pub fn read(archive: &Path, entry: &str) -> Result<Vec<u8>> {
    let file = std::fs::File::open(archive)
        .with_context(|| format!("cannot open {}", archive.display()))?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file))
        .with_context(|| format!("{} is not a readable zip", archive.display()))?;
    let mut found = zip
        .by_name(entry)
        .with_context(|| format!("{} has no {entry}", archive.display()))?;
    let mut bytes = Vec::with_capacity(found.size() as usize);
    found.read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_comes_apart_at_the_archive() {
        let (archive, entry) = split(Path::new("/lib/violin.zip/violin_C4.mp3")).unwrap();
        assert_eq!(archive, Path::new("/lib/violin.zip"));
        assert_eq!(entry, "violin_C4.mp3");

        // Folders inside the archive stay with the entry.
        let (_, entry) = split(Path::new("/lib/violin.zip/staccato/C4.wav")).unwrap();
        assert_eq!(entry, "staccato/C4.wav");

        // The outermost archive wins, so an entry that is itself called `.zip`
        // is read out of the archive that holds it rather than opened.
        let (archive, entry) = split(Path::new("/lib/pack.zip/inner.zip")).unwrap();
        assert_eq!(archive, Path::new("/lib/pack.zip"));
        assert_eq!(entry, "inner.zip");

        assert!(split(Path::new("/lib/violin/C4.wav")).is_none());
        // An archive with nothing after it is a file, not an entry in itself.
        assert!(split(Path::new("/lib/violin.zip")).is_none());
    }
}
