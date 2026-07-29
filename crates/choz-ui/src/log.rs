//! Redirect process stderr (fd 2) to a log file before the TUI takes over the
//! terminal. Every `eprintln!` in the app + any panic message then lands in the
//! file instead of corrupting the ratatui display.

use std::fs::OpenOptions;
use std::os::fd::AsRawFd;
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

/// Seconds since the Unix epoch — a dependency-free timestamp for log separators.
fn timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
