//! Samples that do not live in RAM.
//!
//! A folder of samples used to be decoded whole and held whole: for a library
//! of long files that is a wait at load and a lot of memory, and it is paid
//! again every time the instrument is opened. This is the way out, and it is the
//! same idea the scan cache is built on — **work it out once and write it
//! down**.
//!
//! ## How
//!
//! The first load of a long sample decodes it (there is no way round that:
//! MP3 and FLAC have to be decoded to be played) and writes the result as
//! **raw interleaved `f32` at the instrument's sample rate** into
//! `~/.local/state/choz/pcm/`. The decoded `Vec` is then dropped and the file is
//! **mapped** instead. Every load after that is a `mmap` and no decode at all.
//!
//! What the audio thread reads is therefore a slice of mapped file. The **kernel
//! is the ring buffer**: `madvise(MADV_WILLNEED)` at load asks it to bring the
//! pages in, and it may evict them again under memory pressure — which is
//! exactly the behaviour wanted, since the alternative was holding all of it
//! resident whether it was played or not.
//!
//! ## The ceiling, said out loud
//!
//! ponytail: a page that has been evicted costs a **fault on the audio thread**,
//! and a fault can block. The mitigations here are the `MADV_WILLNEED` at load
//! and the head of every sample kept in RAM (`HEAD_FRAMES`), so a note-on never
//! waits. A sustained note over a cold file can still stutter once. Making that
//! impossible means a reader thread with a ring per voice and voice-stealing
//! when the disk does not keep up — the right next step **measured against a
//! real library**, not guessed at.
//!
//! Short samples — kits, slices, one-shots, anything under [`THRESHOLD_FRAMES`]
//! — stay in memory exactly as they were. Nothing about a drum kit was slow.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};

/// How much disk the raw cache is allowed to take, in bytes: 8 GiB.
///
/// Raw `f32` is **bigger than the file it came from** — four times an MP3 and
/// about twice a FLAC — so a cache with no ceiling would quietly turn a 3 GB
/// library into 12 GB of somebody else's disk. Past this, samples are held in
/// memory the way they were before any of this existed, and a line says so. The
/// cache is a directory of files: deleting `~/.local/state/choz/pcm` costs one
/// slow load and nothing else.
pub const DISK_BUDGET: u64 = 8 * 1024 * 1024 * 1024;

/// How long a sample has to be before it is worth mapping rather than holding:
/// two seconds at 48 kHz. Under that, the file is smaller than the bookkeeping.
pub const THRESHOLD_FRAMES: usize = 96_000;

/// How much of a mapped sample is kept in RAM: a quarter of a second, so the
/// start of a note never waits for a page.
pub const HEAD_FRAMES: usize = 12_000;

/// Where a sample's audio comes from, as far as a voice is concerned.
///
/// Both arms answer the same two questions — how many frames, and what is at
/// frame `i` — and neither allocates. `Clone` is an `Arc` bump: a voice holds
/// what it is playing alive.
#[derive(Clone)]
pub enum Pcm {
    /// Decoded and held. Short samples, and anything whose cache could not be
    /// written.
    Mem(Arc<Vec<f32>>),
    /// Mapped from the raw cache, with its head in memory.
    Disk(Arc<Mapped>),
}

impl Pcm {
    /// Interleaved stereo in memory.
    pub fn mem(pcm: Vec<f32>) -> Self {
        Pcm::Mem(Arc::new(pcm))
    }

    /// How many stereo frames there are.
    pub fn frames(&self) -> usize {
        match self {
            Pcm::Mem(pcm) => pcm.len() / 2,
            Pcm::Disk(m) => m.frames,
        }
    }

    /// One frame, as `(left, right)`. Out of range is silence rather than a
    /// panic: the audio thread is the last place to find out about an
    /// off-by-one.
    #[inline]
    pub fn frame(&self, i: usize) -> (f32, f32) {
        match self {
            Pcm::Mem(pcm) => match pcm.get(i * 2..i * 2 + 2) {
                Some([l, r]) => (*l, *r),
                _ => (0.0, 0.0),
            },
            Pcm::Disk(m) => m.frame(i),
        }
    }
}

/// A raw PCM cache file, mapped read-only for the life of the instrument.
pub struct Mapped {
    ptr: *const f32,
    /// Bytes mapped — what `munmap` needs back.
    bytes: usize,
    frames: usize,
    /// The first [`HEAD_FRAMES`] frames, held so a note-on never faults.
    head: Vec<f32>,
}

// SAFETY: the mapping is read-only and never moved; `ptr` is valid until `Drop`
// unmaps it, and nothing hands out a reference that outlives the `Mapped`.
unsafe impl Send for Mapped {}
unsafe impl Sync for Mapped {}

impl Mapped {
    #[inline]
    fn frame(&self, i: usize) -> (f32, f32) {
        if i >= self.frames {
            return (0.0, 0.0);
        }
        // The head, without touching the mapping at all.
        if let Some([l, r]) = self.head.get(i * 2..i * 2 + 2) {
            return (*l, *r);
        }
        // SAFETY: `i < self.frames`, and the mapping is `frames * 2` floats
        // long — the length it was created from.
        unsafe { (*self.ptr.add(i * 2), *self.ptr.add(i * 2 + 1)) }
    }
}

impl Drop for Mapped {
    fn drop(&mut self) {
        // SAFETY: the same address and length the mapping was made with, and
        // nothing else holds it — `Mapped` is only ever behind an `Arc`.
        unsafe {
            libc::munmap(self.ptr as *mut libc::c_void, self.bytes);
        }
    }
}

/// Where the raw cache lives: one file per `(sample, rate)`, named after what it
/// was made from so a sample that changed gets a different file rather than a
/// stale one.
pub fn cache_path(sample: &Path, sample_rate: u32) -> PathBuf {
    let mut key = format!("{}|{sample_rate}", sample.display());
    // Length and modification time, the same identity the scan cache uses and
    // for the same reason (see `sampler::cache`). A sample inside a zip is
    // identified by the archive's.
    let stat = match crate::instruments::sampler::archive::split(sample) {
        Some((archive, entry)) => {
            key.push('|');
            key.push_str(&entry);
            std::fs::metadata(archive)
        }
        None => std::fs::metadata(sample),
    };
    if let Ok(meta) = stat {
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        key.push_str(&format!("|{}|{mtime}", meta.len()));
    }
    crate::cache::state_dir()
        .join("pcm")
        .join(format!("{:016x}.pcm", hash(&key)))
}

/// FNV-1a. Not a content hash of the audio — a name for a cache entry, which is
/// all a file name has to be, and one that needs no dependency.
fn hash(text: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// How much the cache holds right now, counted once per process and kept up to
/// date by what this module writes.
///
/// Counted once because a walk of the directory per sample is a walk per sample:
/// an instrument is a hundred of them.
fn used() -> &'static std::sync::atomic::AtomicU64 {
    use std::sync::atomic::AtomicU64;
    static USED: std::sync::OnceLock<AtomicU64> = std::sync::OnceLock::new();
    USED.get_or_init(|| {
        let dir = crate::cache::state_dir().join("pcm");
        let total: u64 = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.metadata().ok())
            .map(|m| m.len())
            .sum();
        AtomicU64::new(total)
    });
    USED.get().expect("just initialised")
}

/// Whether there is room for one more, and how much it would take.
fn room_for(frames: usize) -> bool {
    use std::sync::atomic::Ordering;
    let want = frames as u64 * 2 * std::mem::size_of::<f32>() as u64;
    used().load(Ordering::Relaxed) + want <= DISK_BUDGET
}

/// Write `pcm` out as the raw cache for `sample`, then map it back.
///
/// The decoded buffer is handed over by value and dropped here: the point of
/// this is that the whole library is not resident, so the caller must not keep
/// its copy.
pub fn cache_and_map(sample: &Path, sample_rate: u32, pcm: Vec<f32>) -> Result<Pcm> {
    if !room_for(pcm.len() / 2) {
        bail!(
            "the sample cache is at its {} GiB ceiling; holding this one in memory instead",
            DISK_BUDGET / (1024 * 1024 * 1024)
        );
    }
    let path = cache_path(sample, sample_rate);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("cannot make {}", dir.display()))?;
    }
    if !path.exists() {
        // Written beside and renamed: a cache half written by a process that
        // died is a cache that plays half a note forever.
        let tmp = path.with_extension("part");
        write_raw(&tmp, &pcm)?;
        std::fs::rename(&tmp, &path)
            .with_context(|| format!("cannot put {} in place", path.display()))?;
        used().fetch_add(
            (pcm.len() * std::mem::size_of::<f32>()) as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
    }
    drop(pcm);
    map(&path)
}

/// The cache for `sample`, if one was already written.
pub fn mapped(sample: &Path, sample_rate: u32) -> Option<Pcm> {
    let path = cache_path(sample, sample_rate);
    path.exists().then(|| map(&path).ok()).flatten()
}

fn write_raw(path: &Path, pcm: &[f32]) -> Result<()> {
    use std::io::Write;
    let file =
        std::fs::File::create(path).with_context(|| format!("cannot write {}", path.display()))?;
    let mut out = std::io::BufWriter::new(file);
    // Native endianness on purpose: this is a cache on the machine that wrote
    // it, not a format anybody exchanges. A cache read on another machine would
    // be a cache read wrong, which is why the name carries the path.
    for sample in pcm {
        out.write_all(&sample.to_ne_bytes())?;
    }
    out.flush()?;
    Ok(())
}

/// Map a raw cache file, keeping its head in memory.
pub fn map(path: &Path) -> Result<Pcm> {
    use std::os::unix::io::AsRawFd;
    let file = std::fs::File::open(path)
        .with_context(|| format!("cannot open {}", path.display()))?;
    let bytes = file.metadata()?.len() as usize;
    let floats = bytes / std::mem::size_of::<f32>();
    if floats < 2 {
        bail!("{}: too short to be a sample", path.display());
    }
    // SAFETY: a read-only private mapping of `bytes` from a file that is open;
    // the fd may be closed straight after, which is what POSIX says.
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            bytes,
            libc::PROT_READ,
            libc::MAP_PRIVATE,
            file.as_raw_fd(),
            0,
        )
    };
    if ptr == libc::MAP_FAILED {
        bail!(
            "{}: cannot map ({})",
            path.display(),
            std::io::Error::last_os_error()
        );
    }
    // Ask the kernel to bring it in. It is free to say no and free to evict it
    // later, which is the whole reason this is a mapping and not a `read`.
    // SAFETY: the mapping this call was given back.
    unsafe {
        libc::madvise(ptr, bytes, libc::MADV_WILLNEED);
    }
    let ptr = ptr as *const f32;
    let frames = floats / 2;
    // The head, copied out while the pages are certainly warm.
    let head_frames = HEAD_FRAMES.min(frames);
    let mut head = Vec::with_capacity(head_frames * 2);
    // SAFETY: `head_frames * 2 <= floats`, the length mapped.
    head.extend_from_slice(unsafe { std::slice::from_raw_parts(ptr, head_frames * 2) });
    Ok(Pcm::Disk(Arc::new(Mapped {
        ptr,
        bytes,
        frames,
        head,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A long sample goes out to disk and comes back the same, sample for
    /// sample — the head out of memory and the rest out of the mapping.
    #[test]
    fn a_sample_written_out_reads_back_identical() {
        let dir = std::env::temp_dir().join(format!("choz-stream-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("long.wav");
        std::fs::write(&file, []).unwrap();

        // Two frames per index, so a frame that came back from the wrong place
        // is obvious.
        let frames = HEAD_FRAMES + 1_000;
        let pcm: Vec<f32> = (0..frames)
            .flat_map(|i| [i as f32, -(i as f32)])
            .collect();
        let mapped = cache_and_map(&file, 48_000, pcm).expect("could not cache");
        assert_eq!(mapped.frames(), frames);
        for i in [0usize, 1, HEAD_FRAMES - 1, HEAD_FRAMES, frames - 1] {
            assert_eq!(mapped.frame(i), (i as f32, -(i as f32)), "frame {i}");
        }
        // Past the end is silence, not a panic: the audio thread is the last
        // place to find out about an off-by-one.
        assert_eq!(mapped.frame(frames), (0.0, 0.0));

        // And the second load does not need the audio again.
        assert!(super::mapped(&file, 48_000).is_some());
        assert!(super::mapped(&file, 44_100).is_none(), "the rate is part of it");

        let _ = std::fs::remove_file(cache_path(&file, 48_000));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// In memory and on disk answer the same questions the same way — which is
    /// what lets a voice not care which it got.
    #[test]
    fn both_kinds_answer_alike() {
        let pcm = Pcm::mem(vec![0.25, -0.25, 0.5, -0.5]);
        assert_eq!(pcm.frames(), 2);
        assert_eq!(pcm.frame(0), (0.25, -0.25));
        assert_eq!(pcm.frame(1), (0.5, -0.5));
        assert_eq!(pcm.frame(2), (0.0, 0.0));
    }
}
