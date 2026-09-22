//! What the scan already worked out, kept so it is worked out once.
//!
//! Analysing a sample means decoding it and running a pitch detector over it.
//! For a library the size of Philharmonia's that is minutes, and it would be
//! spent again at every load, in a DAW, while somebody waits.
//!
//! Each entry remembers the file's **length and modification time**. If either
//! moved, the entry is dropped and the file is analysed again.
//!
//! ponytail: length + mtime, not a content hash. Hashing every sample means
//! reading the whole library off disk to find out whether it needs reading off
//! disk. The case this misses is an edit that preserves both, which is a
//! `touch -r` away from impossible by accident — and the fix for it is the
//! rescan button, not a slower load for everyone. A content hash earns its
//! place when relinking arrives, which needs to identify a file that *moved*.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::Sample;

/// Bumped when [`Sample`] changes shape, so an old cache is ignored rather
/// than half-read.
const VERSION: u32 = 2;

#[derive(Serialize, Deserialize)]
struct Entry {
    sample: Sample,
    len: u64,
    mtime: u64,
}

#[derive(Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    dir: PathBuf,
    entries: Vec<Entry>,
    /// The octave the pack's note names are off by — see
    /// `super::octave_of_names`. Absent in caches written before it existed,
    /// which then work it out once.
    #[serde(default)]
    octave: Option<i8>,
}

/// `$XDG_STATE_HOME/choz/samples/<folder>-<id>.json`. The folder's name is in
/// there for the benefit of whoever goes looking; the id is what makes it
/// unique.
fn path_for(dir: &Path) -> PathBuf {
    let name = dir
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "samples".into());
    let name: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .take(40)
        .collect();
    // FNV-1a over the full path: short, stable across runs, and no dependency.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in dir.to_string_lossy().as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    crate::cache::state_dir()
        .join("samples")
        .join(format!("{name}-{hash:016x}.json"))
}

/// The file's length and modification time. A sample inside an archive has
/// neither of its own, so it takes the archive's: a zip that changed is a zip
/// whose entries all have to be looked at again, which is exactly right.
fn fingerprint(path: &Path) -> Option<(u64, u64)> {
    let on_disk = super::archive::split(path).map(|(a, _)| a);
    let meta = std::fs::metadata(on_disk.as_deref().unwrap_or(path)).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some((meta.len(), mtime))
}

/// The samples of `dir` that are still exactly as they were analysed, and the
/// octave correction worked out for their names, if one was.
pub fn read(dir: &Path) -> (HashMap<PathBuf, Sample>, Option<i8>) {
    let Ok(text) = std::fs::read_to_string(path_for(dir)) else {
        return (HashMap::new(), None);
    };
    let Ok(cached) = serde_json::from_str::<CacheFile>(&text) else {
        return (HashMap::new(), None);
    };
    if cached.version != VERSION {
        return (HashMap::new(), None);
    }
    let samples = cached
        .entries
        .into_iter()
        .filter(|e| fingerprint(&e.sample.path) == Some((e.len, e.mtime)))
        .map(|e| (e.sample.path.clone(), e.sample))
        .collect();
    (samples, cached.octave)
}

/// Write the folder's analysis back. A cache that cannot be written is not an
/// error worth stopping a load for — it costs the next scan its time, nothing
/// more.
pub fn write(dir: &Path, samples: &[Sample], octave: Option<i8>) {
    let entries: Vec<Entry> = samples
        .iter()
        .filter_map(|s| {
            let (len, mtime) = fingerprint(&s.path)?;
            Some(Entry {
                sample: s.clone(),
                len,
                mtime,
            })
        })
        .collect();
    let file = CacheFile {
        version: VERSION,
        dir: dir.to_path_buf(),
        entries,
        octave,
    };
    let path = path_for(dir);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match serde_json::to_string(&file) {
        Ok(text) => {
            let _ = std::fs::write(&path, text);
        }
        Err(e) => eprintln!("choz: sampler cache: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rewritten sample is not served from the cache. The test writes the
    /// file twice with different contents, which moves the length.
    #[test]
    fn an_edited_sample_is_dropped_from_the_cache() {
        let dir = std::env::temp_dir().join(format!("choz-sampler-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wav = dir.join("thing_C4.wav");
        std::fs::write(&wav, b"one").unwrap();

        let sample = super::super::describe_one(&dir, &wav, 3);
        write(&dir, std::slice::from_ref(&sample), Some(-1));
        assert!(read(&dir).0.contains_key(&wav), "not cached");
        assert_eq!(read(&dir).1, Some(-1), "the octave correction is kept");

        std::fs::write(&wav, b"a different length entirely").unwrap();
        assert!(!read(&dir).0.contains_key(&wav), "stale entry survived");

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(path_for(&dir));
    }
}
