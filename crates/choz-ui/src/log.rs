//! Point the process's own output at a log file before the TUI takes over the
//! terminal, so nothing can scribble on the display.
//!
//! stderr (fd 2) carries every `eprintln!` and panic message. **stdout (fd 1)
//! matters just as much**: hosted plugins print their own banners and warnings
//! there — u-he's synths, fluidsynth, guitarix all do — and fd 1 is exactly
//! where ratatui draws. So choz keeps a duplicate of the real terminal to draw
//! through, and hands fd 1 itself to the log.

use std::fs::{File, OpenOptions};
use std::os::fd::{AsRawFd, FromRawFd};
use std::path::PathBuf;
use std::time::Duration;

/// Log-file location: `<state dir>/choz.log`, next to the plugin cache.
pub fn log_path() -> PathBuf {
    choz_engine::cache::state_dir().join("choz.log")
}

/// Past this size the log is cut back to empty rather than left to grow.
///
/// A hosted plugin stuck retriggering a note can flood its own engine's
/// logging (fluidsynth's "Ringbuffer full" warning, printed once per failed
/// voice steal) fast enough to write tens of gigabytes in minutes — disk fill
/// on top of the CPU the stuck note is already burning. 32 MiB is generous for
/// any real session's worth of `eprintln!` and still cheap to lose.
const MAX_LOG_BYTES: u64 = 32 * 1024 * 1024;

/// Watch the log file for the life of the process and truncate it once it
/// passes [`MAX_LOG_BYTES`].
///
/// Both [`redirect_stderr`] and [`take_terminal`] hand fd 1/2 to the file with
/// `O_APPEND`, and an `O_APPEND` writer seeks to the current end of file on
/// every write rather than remembering an offset — so truncating the path out
/// from under it is enough to cap it. Nothing on either end has to know this
/// thread exists.
///
/// ponytail: a poll every few seconds, not a byte-counted write wrapper — nothing
/// here is on the audio path, and the failure mode this guards is a flood, not
/// a slow leak, so a few seconds of slack before the cut costs nothing that a
/// tighter check wouldn't already have lost to the first burst.
pub fn spawn_log_watchdog() {
    std::thread::Builder::new()
        .name("choz log watchdog".into())
        .spawn(|| loop {
            std::thread::sleep(Duration::from_secs(5));
            truncate_if_oversized();
        })
        .ok();
}

fn truncate_if_oversized() {
    truncate_if_over(&log_path(), MAX_LOG_BYTES);
}

fn truncate_if_over(path: &std::path::Path, limit: u64) {
    let Ok(meta) = std::fs::metadata(path) else {
        return;
    };
    if meta.len() <= limit {
        return;
    }
    if let Ok(f) = OpenOptions::new().write(true).open(path) {
        let _ = f.set_len(0);
    }
}

/// Point fd 2 (stderr) at the log file. Returns the path on success so the
/// caller can tell the user where to look. Best-effort: on any failure stderr
/// is left untouched and `None` is returned.
pub fn redirect_stderr() -> Option<PathBuf> {
    let path = log_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()?;

    // SAFETY: dup2 duplicates the file descriptor onto STDERR_FILENO; both are
    // valid open fds. We leak `file` so its fd stays open for the process life.
    let rc = unsafe { libc::dup2(file.as_raw_fd(), libc::STDERR_FILENO) };
    if rc < 0 {
        return None;
    }
    std::mem::forget(file); // keep the underlying fd alive

    eprintln!("\n─── choz started {} ───", timestamp());
    Some(path)
}

/// Hand fd 1 to the log file and return a duplicate of the real terminal for
/// the TUI to draw through. Call it *after* any startup `println!`.
///
/// On failure the terminal is returned untouched, so choz still draws — it just
/// keeps sharing fd 1 with whatever a plugin decides to print.
pub fn take_terminal() -> std::io::Result<File> {
    // SAFETY: dup(1) returns a fresh fd for the same open file description;
    // wrapping it in a File gives it an owner that closes it on drop.
    let dup = unsafe { libc::dup(libc::STDOUT_FILENO) };
    if dup < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let terminal = unsafe { File::from_raw_fd(dup) };

    if let Ok(file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path())
    {
        // SAFETY: both are valid open fds; the log file is leaked so its fd
        // stays alive for the life of the process.
        if unsafe { libc::dup2(file.as_raw_fd(), libc::STDOUT_FILENO) } >= 0 {
            std::mem::forget(file);
        }
    }
    Ok(terminal)
}

/// Seconds since the Unix epoch — a dependency-free timestamp for log separators.
fn timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A log past the cap is cut back to empty; one still under it is left
    /// alone. `O_APPEND` is what makes this safe to do out from under a
    /// writer that already holds the fd open — the point of the whole thing.
    #[test]
    fn oversized_log_is_truncated_undersized_is_left_alone() {
        let dir = std::env::temp_dir().join(format!(
            "choz-log-watchdog-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let big = dir.join("big.log");
        let small = dir.join("small.log");
        std::fs::write(&big, vec![b'x'; 100]).unwrap();
        std::fs::write(&small, vec![b'x'; 10]).unwrap();

        truncate_if_over(&big, 50);
        truncate_if_over(&small, 50);

        assert_eq!(std::fs::metadata(&big).unwrap().len(), 0, "over the cap");
        assert_eq!(std::fs::metadata(&small).unwrap().len(), 10, "under the cap");

        // An O_APPEND writer keeps writing after the cut — the mechanism the
        // watchdog actually relies on, not just the truncate call itself.
        let mut appender = OpenOptions::new().append(true).open(&big).unwrap();
        appender.write_all(b"fresh").unwrap();
        assert_eq!(std::fs::read(&big).unwrap(), b"fresh");

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
