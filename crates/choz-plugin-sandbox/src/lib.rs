//! Hosting a plugin in a process of its own.
//!
//! Scanning and load-probing already run in a child (`choz-engine`'s
//! `scan_worker_main` / `quarantine`), so a plugin that dies on the way in
//! costs only itself. This crate is the other half: the audio path, so a plugin
//! that dies *while playing* costs a glitch instead of the app.
//!
//! Two pieces, and they are deliberately separate:
//!
//! - [`shm`] maps a block of memory both processes can see.
//! - [`bridge`] is the protocol over those bytes — one block out, one block
//!   back, with a deadline. It never allocates and never maps anything, so the
//!   whole handshake is testable in one process, on a plain `Vec<u8>`.
//!
//! Linux/macOS only: `shm_open`, `mmap`. Windows would need
//! `CreateFileMapping`, and choz is a JACK/ALSA program anyway.

pub mod bridge;
#[cfg(unix)]
pub mod shm;

pub use bridge::{region_bytes, Host, Sandbox};
