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

/// Log-file location: `<state dir>/choz.log`, next to the plugin cache.
pub fn log_path() -> PathBuf {
    choz_engine::cache::state_dir().join("choz.log")
}

/// Point fd 2 (stderr) at the log file. Returns the path on success so the
/// caller can tell the user where to look. Best-effort: on any failure stderr
/// is left untouched and `None` is returned.
pub fn redirect_stderr() -> Option<PathBuf> {
    let path = log_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let file = OpenOptions::new().create(true).append(true).open(&path).ok()?;

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

    if let Ok(file) = OpenOptions::new().create(true).append(true).open(log_path()) {
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
