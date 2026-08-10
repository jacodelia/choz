//! One block of audio across a process boundary, and back.
//!
//! Layout of the shared region:
//!
//! ```text
//! [Header][input block: frames × channels f32][output block: same]
//! ```
//!
//! Every callback is a **rendezvous**, not a stream: the host writes its input
//! block, bumps `request`, and waits for `done` to catch up; the child wakes,
//! processes, writes the output block and bumps `done`. There is only ever one
//! block in flight, so there is no queue to get out of sync and no latency to
//! account for beyond the round trip itself.
//!
//! Waiting is what makes this safe to do from the audio thread:
//! [`Host::exchange`] gives the child a **deadline**. Miss it and the host
//! reads silence and carries on — a sandboxed plugin that hangs costs a glitch,
//! not the stream.
//!
//! ponytail: the wait is a bounded spin plus `sched_yield`, not a futex. It
//! keeps the region to plain atomics (no `sem_t` layout games, no init order to
//! get wrong) and the spin is short by construction — the child is answering a
//! block it already has. Swap in a futex if the busy-wait ever shows up in a
//! profile.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Bookkeeping at the head of the region. `repr(C)` because two independently
/// compiled processes read it.
#[repr(C)]
pub struct Header {
    /// Bumped by the host once its input block is written.
    pub request: AtomicU64,
    /// Bumped by the child once its output block is written. Equal to
    /// `request` means "the answer to that request is ready".
    pub done: AtomicU64,
    /// Set by the host when it wants the child to exit for good. A child that
    /// is merely being replaced never sees this.
    pub quit: AtomicU32,
    /// Frames in a block. Fixed for the life of the region.
    pub frames: AtomicU32,
    /// Interleaved channels per frame.
    pub channels: AtomicU32,
    pub sample_rate: AtomicU32,
    /// MIDI bytes queued for the coming block: `count` messages of 3 bytes.
    pub midi_count: AtomicU32,
    pub midi: [AtomicU32; MAX_MIDI],
    /// Parameter changes queued for the coming block: `count` pairs of
    /// (index, value as `f32::to_bits`).
    pub param_count: AtomicU32,
    pub param_index: [AtomicU32; MAX_PARAMS],
    pub param_value: [AtomicU32; MAX_PARAMS],

    // ── The plugin's window ────────────────────────────────────────────────
    //
    // The whole reason a plugin runs in its own process is that it can die
    // without taking choz with it — and a plugin's GUI is where third-party
    // code crashes most (every guitarix UI segfaults on this machine). So the
    // window is opened *by the child*, in the child's process, embedded into an
    // X11 window choz created: X11 window ids are valid across processes, which
    // is exactly what plugin bridges have always relied on.
    /// Bumped by the host to ask for something; the child answers by copying it
    /// into `editor_ack`.
    pub editor_seq: AtomicU32,
    /// What was asked: [`EDITOR_OPEN`] or [`EDITOR_CLOSE`].
    pub editor_cmd: AtomicU32,
    /// The X11 window the plugin should embed into.
    pub editor_parent: AtomicU64,
    /// Echo of `editor_seq` once the child has acted on it.
    pub editor_ack: AtomicU32,
    /// Size the plugin asked for, packed `width << 16 | height`. Zero when it
    /// did not report one (or has no editor at all).
    pub editor_size: AtomicU32,
    /// What the child found once the plugin was loaded: [`EDITOR_UNKNOWN`],
    /// [`EDITOR_NONE`] or [`EDITOR_PRESENT`]. Without it the host would offer a
    /// `GUI` button for every sandboxed plugin and open an empty frame for the
    /// ones that have no window — the host cannot see the plugin itself, and at
    /// the moment it captures the handle the child is still loading.
    pub editor_present: AtomicU32,
}

/// `editor_cmd` values.
pub const EDITOR_CLOSE: u32 = 0;
pub const EDITOR_OPEN: u32 = 1;

/// `editor_present` values.
pub const EDITOR_UNKNOWN: u32 = 0;
pub const EDITOR_NONE: u32 = 1;
pub const EDITOR_PRESENT: u32 = 2;

/// MIDI messages that fit in one block. Beyond this the newest are dropped —
/// the host side is realtime and must not grow anything.
pub const MAX_MIDI: usize = 64;

/// Parameter changes that fit in one block. A knob being dragged produces one
/// per frame at most; anything past that is the same knob, so dropping the
/// overflow only loses intermediate positions.
pub const MAX_PARAMS: usize = 32;

/// Bytes needed for a region carrying blocks of `frames × channels`.
pub fn region_bytes(frames: u32, channels: u32) -> usize {
    let block = frames as usize * channels as usize * std::mem::size_of::<f32>();
    std::mem::size_of::<Header>() + 2 * block
}

/// Shared plumbing for both ends. The pointers are into the shared mapping.
struct Region {
    header: *mut Header,
    input: *mut f32,
    output: *mut f32,
    samples: usize,
}

impl Region {
    /// # Safety
    /// `base` must point at [`region_bytes`] bytes, 8-byte aligned, valid for
    /// as long as this handle lives.
    unsafe fn new(base: *mut u8, frames: u32, channels: u32) -> Self {
        let samples = frames as usize * channels as usize;
        let audio = unsafe { base.add(std::mem::size_of::<Header>()) } as *mut f32;
        Self {
            header: base as *mut Header,
            input: audio,
            output: unsafe { audio.add(samples) },
            samples,
        }
    }

    fn header(&self) -> &Header {
        // SAFETY: the pointer comes from `new`, whose contract covers this.
        unsafe { &*self.header }
    }

    /// The input block. `&mut self` because the other process may be reading
    /// it: the SPSC discipline is per-side, so each side needs exclusive use of
    /// its own handle.
    fn input(&mut self) -> &mut [f32] {
        unsafe { std::slice::from_raw_parts_mut(self.input, self.samples) }
    }

    fn output(&mut self) -> &mut [f32] {
        unsafe { std::slice::from_raw_parts_mut(self.output, self.samples) }
    }
}

// SAFETY: the region is shared with another process on purpose. Which side may
// write what is the protocol above; each handle is used from one thread.
unsafe impl Send for Region {}

/// The choz side of the bridge.
pub struct Host {
    region: Region,
    /// Requests the child has answered late (or not at all).
    missed: u64,
}

impl Host {
    /// Set up a fresh region. Call once, before the child attaches.
    ///
    /// # Safety
    /// See [`Region::new`].
    pub unsafe fn create(base: *mut u8, frames: u32, channels: u32, sample_rate: u32) -> Self {
        // Zero first: `shm_open` gives zeroed pages, but a reused region might
        // not be, and a stale `done` would look like an answer.
        unsafe { std::ptr::write_bytes(base, 0, region_bytes(frames, channels)) };
        let region = unsafe { Region::new(base, frames, channels) };
        let h = region.header();
        h.frames.store(frames, Ordering::Relaxed);
        h.channels.store(channels, Ordering::Relaxed);
        h.sample_rate.store(sample_rate, Ordering::Relaxed);
        Self { region, missed: 0 }
    }

    /// Queue a MIDI message for the next exchange. Dropped when the block's
    /// buffer is full: the audio thread never waits and never allocates.
    pub fn push_midi(&mut self, msg: [u8; 3]) {
        let h = self.region.header();
        let n = h.midi_count.load(Ordering::Relaxed) as usize;
        if n >= MAX_MIDI {
            return;
        }
        let packed = u32::from(msg[0]) | u32::from(msg[1]) << 8 | u32::from(msg[2]) << 16;
        h.midi[n].store(packed, Ordering::Relaxed);
        h.midi_count.store(n as u32 + 1, Ordering::Relaxed);
    }

    /// Queue a parameter change for the next exchange. Dropped when the
    /// block's buffer is full, same as MIDI.
    pub fn push_param(&mut self, index: usize, value: f32) {
        let h = self.region.header();
        let n = h.param_count.load(Ordering::Relaxed) as usize;
        if n >= MAX_PARAMS || index > u32::MAX as usize {
            return;
        }
        h.param_index[n].store(index as u32, Ordering::Relaxed);
        h.param_value[n].store(value.to_bits(), Ordering::Relaxed);
        h.param_count.store(n as u32 + 1, Ordering::Relaxed);
    }

    /// Hand `input` to the child and fill `output` with what comes back.
    ///
    /// Returns `false` when the child missed the deadline; `output` is then
    /// silence and the block is simply lost. Realtime-safe: no allocation, and
    /// the wait is bounded by `deadline`.
    pub fn exchange(&mut self, input: &[f32], output: &mut [f32], deadline: Duration) -> bool {
        let n = self.region.samples.min(input.len()).min(output.len());
        self.region.input()[..n].copy_from_slice(&input[..n]);

        let h = self.region.header();
        let ticket = h.request.load(Ordering::Relaxed) + 1;
        // Release: the child must see the samples we just wrote, and the MIDI.
        h.request.store(ticket, Ordering::Release);

        let answered = wait_until(deadline, || h.done.load(Ordering::Acquire) >= ticket);
        // MIDI and parameters belong to the block just sent, answered or not.
        h.midi_count.store(0, Ordering::Relaxed);
        h.param_count.store(0, Ordering::Relaxed);
        if !answered {
            self.missed += 1;
            output[..n].fill(0.0);
            return false;
        }
        output[..n].copy_from_slice(&self.region.output()[..n]);
        true
    }

    /// How many blocks the child has failed to answer in time.
    /// A handle to just the window half of the region.
    ///
    /// The instance itself belongs to the audio thread; the `GUI` button is
    /// pressed on the UI thread. They never touch the same fields — the window
    /// requests are their own atomics — so this is a second view of the header
    /// rather than a second host.
    ///
    /// # Safety
    /// The caller must keep the shared mapping alive for as long as the link
    /// exists (`SandboxedPlugin` holds the `Shm` in an `Arc` for this).
    pub unsafe fn editor_link(&self) -> EditorLink {
        EditorLink {
            header: self.region.header,
        }
    }

    /// Ask the child to embed its plugin's window into `parent` (an X11 window
    /// id), or to close it when `parent` is `None`.
    ///
    /// Returns the size the plugin asked for, if it reported one before the
    /// deadline. Called from the UI thread, never from audio: it waits.
    pub fn editor(&self, parent: Option<u64>, patience: Duration) -> Option<(u16, u16)> {
        let h = self.region.header();
        h.editor_size.store(0, Ordering::Relaxed);
        h.editor_parent
            .store(parent.unwrap_or(0), Ordering::Relaxed);
        h.editor_cmd.store(
            if parent.is_some() {
                EDITOR_OPEN
            } else {
                EDITOR_CLOSE
            },
            Ordering::Relaxed,
        );
        let seq = h.editor_seq.fetch_add(1, Ordering::AcqRel) + 1;
        // The child picks the request up between blocks, so the wait is on the
        // order of one block — but a plugin building its UI can take much
        // longer, hence the caller's patience.
        if !wait_until(patience, || h.editor_ack.load(Ordering::Acquire) >= seq) {
            return None;
        }
        let packed = h.editor_size.load(Ordering::Acquire);
        (packed != 0).then_some(((packed >> 16) as u16, packed as u16))
    }

    pub fn missed(&self) -> u64 {
        self.missed
    }

    /// Whether the loaded plugin has a window, as reported by the child.
    /// `None` until it has answered its first block — after that it is settled.
    pub fn has_editor(&self) -> Option<bool> {
        match self.region.header().editor_present.load(Ordering::Acquire) {
            EDITOR_PRESENT => Some(true),
            EDITOR_NONE => Some(false),
            _ => None,
        }
    }

    /// Ask the child to exit at its next wake-up.
    pub fn stop(&self) {
        let h = self.region.header();
        h.quit.store(1, Ordering::Release);
        // A bumped request wakes a child parked on the wait below.
        let next = h.request.load(Ordering::Relaxed) + 1;
        h.request.store(next, Ordering::Release);
    }
}

/// One queued parameter change: index into the plugin's list, and a value.
pub type ParamChange = (usize, f32);

/// What the child does with a block: `(input, output, midi, params)`.
pub type Process<'a> = &'a mut dyn FnMut(&[f32], &mut [f32], &[[u8; 3]], &[ParamChange]);

/// The child side of the bridge.
pub struct Sandbox {
    region: Region,
    /// Last request answered.
    served: u64,
}

impl Sandbox {
    /// Attach to a region the host created.
    ///
    /// # Safety
    /// See [`Region::new`]. `frames`/`channels` must match what the host used.
    pub unsafe fn attach(base: *mut u8, frames: u32, channels: u32) -> Self {
        let region = unsafe { Region::new(base, frames, channels) };
        // Pick the count up at the last answered ticket. Zero for the first
        // child, so it answers the request already pending; for a replacement
        // it is wherever its predecessor got to, which skips the history it
        // never saw without the host having to tell it anything.
        let served = region.header().done.load(Ordering::Acquire);
        Self { region, served }
    }

    pub fn sample_rate(&self) -> u32 {
        self.region.header().sample_rate.load(Ordering::Relaxed)
    }

    /// Wait for the next block and process it with `f`, which is handed
    /// `(input, output, midi)`. Returns `false` once the host has asked to stop.
    ///
    /// `patience` bounds the wait so a host that dies without saying so doesn't
    /// leave the child spinning forever.
    /// A pending editor request, if the host made one since the last check.
    ///
    /// The child answers it on its own thread — opening a plugin's window can
    /// take hundreds of milliseconds, and the audio rendezvous must not wait
    /// for that. [`Self::editor_done`] reports back when it is finished.
    pub fn editor_request(&self) -> Option<(u32, Option<u64>)> {
        let h = self.region.header();
        let seq = h.editor_seq.load(Ordering::Acquire);
        if seq == h.editor_ack.load(Ordering::Acquire) {
            return None;
        }
        let parent = h.editor_parent.load(Ordering::Relaxed);
        let open = h.editor_cmd.load(Ordering::Relaxed) == EDITOR_OPEN;
        Some((seq, (open && parent != 0).then_some(parent)))
    }

    /// Tell the host whether the plugin it asked for has a window. Called once,
    /// right after loading and before the first block is served, so the host
    /// knows by the time `build` returns.
    pub fn set_editor_present(&self, present: bool) {
        self.region.header().editor_present.store(
            if present { EDITOR_PRESENT } else { EDITOR_NONE },
            Ordering::Release,
        );
    }

    /// Answer the request `seq`, with the size the plugin asked for.
    pub fn editor_done(&self, seq: u32, size: Option<(u16, u16)>) {
        let h = self.region.header();
        let packed = size.map_or(0, |(w, hgt)| ((w as u32) << 16) | hgt as u32);
        h.editor_size.store(packed, Ordering::Release);
        h.editor_ack.store(seq, Ordering::Release);
    }

    pub fn serve(&mut self, patience: Duration, f: Process<'_>) -> bool {
        let h = self.region.header();
        let want = self.served + 1;
        if !wait_until(patience, || h.request.load(Ordering::Acquire) >= want) {
            // Nothing asked of us: say we are still alive and let the caller
            // decide (it checks whether the host process is still there).
            return h.quit.load(Ordering::Acquire) == 0;
        }
        if h.quit.load(Ordering::Acquire) != 0 {
            return false;
        }

        let count = (h.midi_count.load(Ordering::Acquire) as usize).min(MAX_MIDI);
        let mut midi = [[0u8; 3]; MAX_MIDI];
        for (i, slot) in midi.iter_mut().enumerate().take(count) {
            let packed = h.midi[i].load(Ordering::Relaxed);
            *slot = [packed as u8, (packed >> 8) as u8, (packed >> 16) as u8];
        }
        let pcount = (h.param_count.load(Ordering::Acquire) as usize).min(MAX_PARAMS);
        let mut params = [(0usize, 0.0f32); MAX_PARAMS];
        for (i, slot) in params.iter_mut().enumerate().take(pcount) {
            *slot = (
                h.param_index[i].load(Ordering::Relaxed) as usize,
                f32::from_bits(h.param_value[i].load(Ordering::Relaxed)),
            );
        }

        // One `&mut` at a time: the blocks don't overlap, so split the borrow
        // through the raw pointers the region already holds.
        let (samples, in_ptr, out_ptr) =
            (self.region.samples, self.region.input, self.region.output);
        let input = unsafe { std::slice::from_raw_parts(in_ptr, samples) };
        let output = unsafe { std::slice::from_raw_parts_mut(out_ptr, samples) };
        output.fill(0.0);
        f(input, output, &midi[..count], &params[..pcount]);

        self.served = h.request.load(Ordering::Relaxed);
        // Release: the host must see the output samples once it sees `done`.
        h.done.store(self.served, Ordering::Release);
        true
    }
}

/// Spin, then yield, until `ready` or the deadline passes.
fn wait_until(deadline: Duration, ready: impl Fn() -> bool) -> bool {
    // A block the child already has takes microseconds; spinning first avoids
    // a syscall in the common case.
    for _ in 0..2_000 {
        if ready() {
            return true;
        }
        std::hint::spin_loop();
    }
    let start = Instant::now();
    while start.elapsed() < deadline {
        if ready() {
            return true;
        }
        std::thread::yield_now();
    }
    ready()
}

/// The window half of a live sandbox, usable from the UI thread.
pub struct EditorLink {
    header: *mut Header,
}

// SAFETY: only the editor atomics are touched through this, and atomics are
// what they are for.
unsafe impl Send for EditorLink {}
unsafe impl Sync for EditorLink {}

impl EditorLink {
    /// Same contract as [`Host::editor`].
    pub fn editor(&self, parent: Option<u64>, patience: Duration) -> Option<(u16, u16)> {
        // SAFETY: the mapping outlives the link by construction.
        let h = unsafe { &*self.header };
        h.editor_size.store(0, Ordering::Relaxed);
        h.editor_parent
            .store(parent.unwrap_or(0), Ordering::Relaxed);
        h.editor_cmd.store(
            if parent.is_some() {
                EDITOR_OPEN
            } else {
                EDITOR_CLOSE
            },
            Ordering::Relaxed,
        );
        let seq = h.editor_seq.fetch_add(1, Ordering::AcqRel) + 1;
        if !wait_until(patience, || h.editor_ack.load(Ordering::Acquire) >= seq) {
            return None;
        }
        let packed = h.editor_size.load(Ordering::Acquire);
        (packed != 0).then_some(((packed >> 16) as u16, packed as u16))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The window handshake, in one process on a plain buffer: the host asks,
    /// the child sees exactly one request, answers with a size, and the host
    /// reads it back. Nothing here maps memory or spawns anything — which is
    /// the point of keeping the protocol free of both.
    #[test]
    fn the_window_request_crosses_and_comes_back_with_a_size() {
        let (frames, channels) = (64u32, 2u32);
        let mut buf = vec![0u8; region_bytes(frames, channels)];
        // SAFETY: one buffer, both ends, as the other protocol tests do.
        let host = unsafe { Host::create(buf.as_mut_ptr(), frames, channels, 48_000) };
        let child = unsafe { Sandbox::attach(buf.as_mut_ptr(), frames, channels) };

        assert!(child.editor_request().is_none(), "nothing asked yet");

        // The host asks from another thread — which is where the `GUI` button
        // is pressed — while the child side stays here.
        let link = unsafe { host.editor_link() };
        let asked = std::thread::spawn(move || link.editor(Some(0xBEEF), Duration::from_secs(2)));

        // The child picks it up and answers.
        let (seq, parent) = loop {
            if let Some(req) = child.editor_request() {
                break req;
            }
            std::thread::yield_now();
        };
        assert_eq!(parent, Some(0xBEEF), "the X11 window id crosses intact");
        child.editor_done(seq, Some((640, 480)));
        assert_eq!(asked.join().unwrap(), Some((640, 480)));

        // Answered means answered: no second request appears out of nowhere.
        assert!(child.editor_request().is_none());

        // Closing is the same round trip with no parent.
        let link2 = unsafe { host.editor_link() };
        let closed = std::thread::spawn(move || link2.editor(None, Duration::from_secs(2)));
        let (seq, parent) = loop {
            if let Some(req) = child.editor_request() {
                break req;
            }
            std::thread::yield_now();
        };
        assert_eq!(parent, None, "a close carries no window");
        child.editor_done(seq, None);
        assert_eq!(closed.join().unwrap(), None);
    }

    /// A sandboxed plugin without a window must not get a `GUI` button, and the
    /// host has no way to look — only the child's answer.
    #[test]
    fn the_child_says_whether_its_plugin_has_a_window() {
        let (frames, channels) = (8u32, 2u32);
        let mut buf = vec![0u8; region_bytes(frames, channels)];
        let host = unsafe { Host::create(buf.as_mut_ptr(), frames, channels, 48_000) };
        let child = unsafe { Sandbox::attach(buf.as_mut_ptr(), frames, channels) };

        assert_eq!(host.has_editor(), None, "not loaded yet");
        child.set_editor_present(false);
        assert_eq!(host.has_editor(), Some(false));
        child.set_editor_present(true);
        assert_eq!(host.has_editor(), Some(true));
    }

    /// Both ends in one process, on a plain buffer: the algorithm doesn't care
    /// where the bytes live, which is what makes it testable without a child.
    #[test]
    fn a_block_goes_across_and_comes_back_changed() {
        let (frames, channels) = (4u32, 2u32);
        let mut buf = vec![0u8; region_bytes(frames, channels)];
        let base = buf.as_mut_ptr();
        let mut host = unsafe { Host::create(base, frames, channels, 48_000) };
        let mut child = unsafe { Sandbox::attach(base, frames, channels) };

        host.push_midi([0x90, 60, 100]);
        let input = vec![0.25f32; 8];
        let mut output = vec![0.0f32; 8];

        // The child answers before the host asks — same thread, so serve first
        // would block; instead post the request, then serve, then collect.
        let h = unsafe { &*(base as *const Header) };
        host.region.input().copy_from_slice(&input);
        h.request.store(1, Ordering::Release);

        let mut seen_midi = Vec::new();
        assert!(
            child.serve(Duration::from_millis(50), &mut |inp, out, midi, _params| {
                seen_midi.extend_from_slice(midi);
                for (o, i) in out.iter_mut().zip(inp) {
                    *o = i * 2.0;
                }
            })
        );
        assert_eq!(
            seen_midi,
            vec![[0x90, 60, 100]],
            "MIDI crosses with the block"
        );

        output.copy_from_slice(host.region.output());
        assert!(output.iter().all(|s| (*s - 0.5).abs() < 1e-6), "{output:?}");
    }

    /// A child that never answers costs one silent block, not the stream.
    #[test]
    fn a_silent_child_yields_silence_and_is_counted() {
        let (frames, channels) = (2u32, 2u32);
        let mut buf = vec![0u8; region_bytes(frames, channels)];
        let mut host = unsafe { Host::create(buf.as_mut_ptr(), frames, channels, 48_000) };

        let input = vec![1.0f32; 4];
        let mut output = vec![9.0f32; 4];
        assert!(!host.exchange(&input, &mut output, Duration::from_millis(2)));
        assert!(output.iter().all(|s| *s == 0.0), "{output:?}");
        assert_eq!(host.missed(), 1);
    }

    #[test]
    fn midi_beyond_the_block_limit_is_dropped_not_grown() {
        let mut buf = vec![0u8; region_bytes(1, 2)];
        let mut host = unsafe { Host::create(buf.as_mut_ptr(), 1, 2, 48_000) };
        for _ in 0..MAX_MIDI + 10 {
            host.push_midi([0x90, 60, 1]);
        }
        let h = unsafe { &*(buf.as_ptr() as *const Header) };
        assert_eq!(h.midi_count.load(Ordering::Relaxed) as usize, MAX_MIDI);
    }
}
