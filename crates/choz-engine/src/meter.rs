//! What the output actually sounds like, in numbers the interface can draw.
//!
//! The audio callback is the only place that sees the mixed signal, and it can
//! neither allocate nor block — so it writes a handful of atomics and the UI
//! reads them whenever it redraws. Nothing is synchronised beyond "relaxed",
//! because a meter that is one block stale is a meter that is right.
//!
//! Two things are published, which is what a monitor needs:
//!
//! * **level** — peak and RMS of the last block, for a VU strip;
//! * **a waveform window** — a few dozen samples, decimated, so the interface
//!   can draw the shape of the sound rather than only its size.

use std::sync::atomic::{AtomicI32, AtomicU32, AtomicUsize, Ordering};

/// How many points the waveform window holds. Wide enough for a panel, small
/// enough that writing it costs nothing on the audio thread.
pub const WAVE_POINTS: usize = 128;

/// How many **undecimated** samples the spectrum window holds.
///
/// A power of two, because the analyser on the other end is a radix-2 FFT. At
/// 48 kHz this is 43 ms of sound and 23 Hz per bin — enough to separate two
/// notes in the bass, which is where a coarser window stops being a spectrum
/// and starts being a shape.
pub const SPECTRUM_POINTS: usize = 2048;

/// The shared meter. One per process, like the transport: there is one output.
pub struct Meter {
    peak: AtomicU32,
    rms: AtomicU32,
    /// Blocks whose peak went past full scale. The device clips those, and a
    /// hard clip at the device is the worst-sounding failure there is —
    /// counted so the interface can say it rather than leaving it to be
    /// guessed at.
    clipped: AtomicU32,
    /// Ring of recent samples (mono, `f32` bits), and where the next one goes.
    wave: [AtomicU32; WAVE_POINTS],
    write: AtomicUsize,
    /// The same signal **undecimated**, for anything that needs to measure
    /// frequency rather than draw a shape. The wave ring keeps one sample per
    /// slice of a block, which is a picture of the envelope and nothing an FFT
    /// can be run on.
    spectrum: [AtomicU32; SPECTRUM_POINTS],
    spectrum_write: AtomicUsize,
}

pub fn meter() -> &'static Meter {
    static METER: Meter = Meter::new();
    &METER
}

impl Meter {
    #[allow(clippy::declare_interior_mutable_const)]
    const ZERO: AtomicU32 = AtomicU32::new(0);

    const fn new() -> Self {
        Self {
            peak: AtomicU32::new(0),
            rms: AtomicU32::new(0),
            clipped: AtomicU32::new(0),
            wave: [Self::ZERO; WAVE_POINTS],
            write: AtomicUsize::new(0),
            spectrum: [Self::ZERO; SPECTRUM_POINTS],
            spectrum_write: AtomicUsize::new(0),
        }
    }

    /// Publish one block. Called from the audio callback with the interleaved
    /// stereo mix, so it walks the buffer once and stores four numbers plus a
    /// handful of decimated points.
    pub fn publish(&self, buf: &[f32]) {
        if buf.is_empty() {
            return;
        }
        let mut peak = 0.0f32;
        let mut sum = 0.0f64;
        for frame in buf.chunks_exact(2) {
            let mono = (frame[0] + frame[1]) * 0.5;
            peak = peak.max(mono.abs());
            sum += (mono as f64) * (mono as f64);
        }
        let frames = (buf.len() / 2).max(1);
        let rms = (sum / frames as f64).sqrt() as f32;
        self.peak.store(peak.to_bits(), Ordering::Relaxed);
        self.rms.store(rms.to_bits(), Ordering::Relaxed);
        if peak > 1.0 {
            self.clipped.fetch_add(1, Ordering::Relaxed);
        }

        // A window of the shape, not of every sample: one point per slice of the
        // block, so the ring holds roughly a second at any block size.
        let step = frames.div_ceil(WAVE_POINTS.min(frames).max(1));
        let mut i = 0;
        let mut w = self.write.load(Ordering::Relaxed);
        while i < frames {
            let f = i * 2;
            let mono = (buf[f] + buf[f + 1]) * 0.5;
            self.wave[w % WAVE_POINTS].store(mono.to_bits(), Ordering::Relaxed);
            w = w.wrapping_add(1);
            i += step.max(1);
        }
        self.write.store(w, Ordering::Relaxed);

        // Every frame, in order: one relaxed store per frame, which is what a
        // block of 256 costs and what an FFT needs to exist at all.
        let mut sw = self.spectrum_write.load(Ordering::Relaxed);
        for frame in buf.chunks_exact(2) {
            let mono = (frame[0] + frame[1]) * 0.5;
            self.spectrum[sw % SPECTRUM_POINTS].store(mono.to_bits(), Ordering::Relaxed);
            sw = sw.wrapping_add(1);
        }
        self.spectrum_write.store(sw, Ordering::Relaxed);
    }

    pub fn peak(&self) -> f32 {
        f32::from_bits(self.peak.load(Ordering::Relaxed))
    }

    pub fn rms(&self) -> f32 {
        f32::from_bits(self.rms.load(Ordering::Relaxed))
    }

    /// Blocks that went past full scale on the way out. Non-zero means the
    /// device is clipping the mix, which no amount of tuning inside choz will
    /// fix — it is a level, and the mixer is where it is set.
    pub fn clipping(&self) -> u32 {
        self.clipped.load(Ordering::Relaxed)
    }

    /// The waveform window, oldest first.
    pub fn wave(&self) -> [f32; WAVE_POINTS] {
        let start = self.write.load(Ordering::Relaxed);
        let mut out = [0.0f32; WAVE_POINTS];
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = f32::from_bits(self.wave[(start + i) % WAVE_POINTS].load(Ordering::Relaxed));
        }
        out
    }

    /// The undecimated window, oldest first. What the spectrum analyser reads.
    ///
    /// Torn against a running callback by construction — the newest samples may
    /// be a block old and the oldest may already be overwritten. That is the
    /// price of not locking the audio thread, and for a picture that is redrawn
    /// twenty times a second it costs a smear at one edge of one frame.
    pub fn spectrum_window(&self, out: &mut [f32; SPECTRUM_POINTS]) {
        let start = self.spectrum_write.load(Ordering::Relaxed);
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = f32::from_bits(
                self.spectrum[(start + i) % SPECTRUM_POINTS].load(Ordering::Relaxed),
            );
        }
    }

    /// Forget everything — a stream that stopped should not leave a frozen
    /// picture of the last thing it played.
    pub fn clear(&self) {
        self.peak.store(0, Ordering::Relaxed);
        self.rms.store(0, Ordering::Relaxed);
        self.clipped.store(0, Ordering::Relaxed);
        for cell in self.wave.iter() {
            cell.store(0, Ordering::Relaxed);
        }
        for cell in self.spectrum.iter() {
            cell.store(0, Ordering::Relaxed);
        }
    }
}

/// How loud each tab is **before its fader**, since the last reset.
///
/// The master meter says the mix clipped; it cannot say which tab pushed it
/// there, and on a rack of eight that is the whole question. This is the same
/// two numbers per slot, taken where the tab's audio is finished and the strip
/// has not touched it yet — so the reading answers "how loud is this plugin",
/// not "how loud did I leave the fader".
///
/// Both are **sticky maxima**: the audio thread only ever raises them, so a
/// UI reading twenty times a second cannot miss the one block that clipped.
/// Whoever wants a fresh window calls [`SlotLevels::reset`].
pub struct SlotLevels {
    peaks: [AtomicU32; MAX_SLOTS],
    rms: [AtomicU32; MAX_SLOTS],
    /// The **last** block's peak, not the loudest one. What a sidechain reads:
    /// a gate opened by a kick drum needs to know what the drum is doing right
    /// now, and the sticky maxima above answer a different question.
    live: [AtomicU32; MAX_SLOTS],
}

/// As many tabs as the engine will build. Kept here because the meter is the
/// one place both sides agree on how many there can be.
pub const MAX_SLOTS: usize = 32;

pub fn slot_levels() -> &'static SlotLevels {
    static L: SlotLevels = SlotLevels::new();
    &L
}

impl SlotLevels {
    #[allow(clippy::declare_interior_mutable_const)]
    const ZERO: AtomicU32 = AtomicU32::new(0);

    const fn new() -> Self {
        Self {
            peaks: [Self::ZERO; MAX_SLOTS],
            rms: [Self::ZERO; MAX_SLOTS],
            live: [Self::ZERO; MAX_SLOTS],
        }
    }

    /// Publish one block of one tab, interleaved stereo, pre-fader. Called from
    /// the audio callback: one pass over the buffer and two relaxed max-stores.
    pub fn publish(&self, slot: usize, buf: &[f32]) {
        if slot >= MAX_SLOTS || buf.is_empty() {
            return;
        }
        let mut peak = 0.0f32;
        let mut sum = 0.0f64;
        for frame in buf.chunks_exact(2) {
            let mono = (frame[0] + frame[1]) * 0.5;
            peak = peak.max(mono.abs());
            sum += (mono as f64) * (mono as f64);
        }
        let rms = (sum / (buf.len() / 2).max(1) as f64).sqrt() as f32;
        // A plugin that has gone to NaN would otherwise store a bit pattern
        // that no later block can beat, and the tab reads "infinitely loud"
        // for the rest of the session.
        if !peak.is_finite() || !rms.is_finite() {
            return;
        }
        // `fetch_max` on the bits works because both are non-negative and
        // finite: for those, f32 bit order is value order.
        self.peaks[slot].fetch_max(peak.to_bits(), Ordering::Relaxed);
        self.rms[slot].fetch_max(rms.to_bits(), Ordering::Relaxed);
        self.live[slot].store(peak.to_bits(), Ordering::Relaxed);
    }

    /// The loudest block this tab has played since the last reset, as
    /// `(peak, RMS)`, linear. Reading does not clear it — two readers (the
    /// health log and auto-trim) want the same window.
    pub fn read(&self, slot: usize) -> (f32, f32) {
        match slot < MAX_SLOTS {
            true => (
                f32::from_bits(self.peaks[slot].load(Ordering::Relaxed)),
                f32::from_bits(self.rms[slot].load(Ordering::Relaxed)),
            ),
            false => (0.0, 0.0),
        }
    }

    /// What this tab did in the **last block**, linear. The sidechain reading:
    /// see [`SlotLevels::live`].
    pub fn live(&self, slot: usize) -> f32 {
        match slot < MAX_SLOTS {
            true => f32::from_bits(self.live[slot].load(Ordering::Relaxed)),
            false => 0.0,
        }
    }

    /// Start this tab's window again — a new instrument is not the old one's
    /// levels.
    pub fn reset(&self, slot: usize) {
        if slot < MAX_SLOTS {
            self.peaks[slot].store(0, Ordering::Relaxed);
            self.rms[slot].store(0, Ordering::Relaxed);
            self.live[slot].store(0, Ordering::Relaxed);
        }
    }

    pub fn reset_all(&self) {
        for i in 0..MAX_SLOTS {
            self.reset(i);
        }
    }
}

#[cfg(test)]
mod slot_level_tests {
    use super::*;

    /// It keeps the loudest block, not the last one — a fader solved from
    /// whatever happened to be playing when the UI looked is a fader that
    /// changes every time you press the key.
    #[test]
    fn a_slot_keeps_its_loudest_block_until_reset() {
        let l = slot_levels();
        l.reset(3);
        assert_eq!(l.read(3), (0.0, 0.0));

        // Full-scale square, both channels: peak and RMS are both 1.
        l.publish(3, &[1.0, 1.0, -1.0, -1.0]);
        assert_eq!(l.read(3), (1.0, 1.0));

        // A quiet block after it changes nothing.
        l.publish(3, &[0.1, 0.1, -0.1, -0.1]);
        assert_eq!(l.read(3), (1.0, 1.0), "the loud block still stands");

        // A plugin gone to NaN must not stick a reading nothing can beat.
        l.publish(3, &[f32::NAN, f32::NAN]);
        assert_eq!(l.read(3), (1.0, 1.0));

        l.reset(3);
        assert_eq!(l.read(3), (0.0, 0.0));
        // Slots do not read each other, and past the cap is silence, not a panic.
        l.publish(3, &[0.5, 0.5]);
        assert_eq!(l.read(4), (0.0, 0.0));
        l.publish(MAX_SLOTS, &[1.0, 1.0]);
        assert_eq!(l.read(MAX_SLOTS), (0.0, 0.0));
        l.reset_all();
        assert_eq!(l.read(3), (0.0, 0.0));
    }
}

/// How loud each capture channel is, right now.
///
/// The one reading that separates the three ways live audio goes missing:
/// **nothing arrives** (the jack is not wired, or the device is not open —
/// every channel reads silence), **it arrives but is not routed** (the channel
/// reads a level and the tab is still quiet), and **it is routed and the effect
/// is the problem**. Without it all three look identical from the outside, and
/// the only tool left is guessing.
///
/// Written from the audio callback: one pass over the capture buffers, one
/// relaxed store per channel.
pub struct CaptureLevels {
    peaks: [AtomicU32; MAX_CAPTURE],
    channels: AtomicUsize,
}

/// As many as the backends register — the JACK client caps at 32 jacks.
pub const MAX_CAPTURE: usize = 32;

pub fn capture_levels() -> &'static CaptureLevels {
    static L: CaptureLevels = CaptureLevels::new();
    &L
}

impl CaptureLevels {
    #[allow(clippy::declare_interior_mutable_const)]
    const ZERO: AtomicU32 = AtomicU32::new(0);

    const fn new() -> Self {
        Self {
            peaks: [Self::ZERO; MAX_CAPTURE],
            channels: AtomicUsize::new(0),
        }
    }

    /// Publish one block's peak per channel.
    pub fn publish(&self, capture: &[Vec<f32>], frames: usize) {
        self.channels
            .store(capture.len().min(MAX_CAPTURE), Ordering::Relaxed);
        for (ch, buf) in capture.iter().take(MAX_CAPTURE).enumerate() {
            let n = frames.min(buf.len());
            let peak = buf[..n].iter().fold(0.0f32, |m, s| m.max(s.abs()));
            self.peaks[ch].store(peak.to_bits(), Ordering::Relaxed);
        }
    }

    /// Peak of channel `ch` in the last block, linear.
    pub fn peak(&self, ch: usize) -> f32 {
        match ch < MAX_CAPTURE {
            true => f32::from_bits(self.peaks[ch].load(Ordering::Relaxed)),
            false => 0.0,
        }
    }

    /// How many channels the backend last published.
    pub fn channels(&self) -> usize {
        self.channels.load(Ordering::Relaxed)
    }

    pub fn clear(&self) {
        for p in self.peaks.iter() {
            p.store(0, Ordering::Relaxed);
        }
        self.channels.store(0, Ordering::Relaxed);
    }
}

/// How the live input is holding up, when it comes from its own stream.
///
/// The capture stream and the playback stream run on **different clocks** — the
/// microphone's and the speakers' — and two real clocks drift. Neither end can
/// stop that; what `RtState::drain_capture` can do is decide what to give up,
/// and this counts how often it had to:
///
/// * **late** — a block where the input had not produced enough yet, filled
///   with silence. A handful at start-up is normal; a steady trickle is the
///   input running behind, heard as a tick.
/// * **dropped** — samples thrown away because the input was running ahead and
///   the backlog was turning into latency.
///
/// Both counters only move on the cpal backends: the native JACK client hands
/// capture and playback to one callback, so there is nothing to drift against.
/// Without this the answer to "does it drift on my machine" was "play it for an
/// hour and see" — which is exactly the kind of question hardware has to be in
/// front of you to answer, and now it is a number on screen.
pub struct CaptureHealth {
    late: AtomicU32,
    dropped: AtomicU32,
    /// Blocks where the input trim pushed the signal past full scale.
    clipped: AtomicU32,
    /// How much the feedback guard is holding the input down, in dB, as `f32`
    /// bits. Zero when it is holding nothing.
    guard_db: AtomicU32,
}

pub fn capture_health() -> &'static CaptureHealth {
    static H: CaptureHealth = CaptureHealth {
        late: AtomicU32::new(0),
        dropped: AtomicU32::new(0),
        clipped: AtomicU32::new(0),
        guard_db: AtomicU32::new(0),
    };
    &H
}

impl CaptureHealth {
    /// Called from the audio callback: two relaxed adds, and only when
    /// something actually went wrong.
    pub fn late_block(&self) {
        self.late.fetch_add(1, Ordering::Relaxed);
    }

    pub fn dropped_samples(&self, n: usize) {
        self.dropped.fetch_add(n as u32, Ordering::Relaxed);
    }

    /// One block of input arrived past full scale and was limited.
    pub fn clipped_block(&self) {
        self.clipped.fetch_add(1, Ordering::Relaxed);
    }

    /// Blocks the input trim has had to limit. Non-zero means the trim is set
    /// too high, which is heard as saturation and read by the pitch detector as
    /// a waveform that is not the one that was played.
    pub fn clipping(&self) -> u32 {
        self.clipped.load(Ordering::Relaxed)
    }

    /// Called once a block while live audio is coming in: how much the
    /// feedback guard is currently pulling the input down.
    pub fn guard(&self, db: f32) {
        let db = if db.is_finite() { db.min(0.0) } else { 0.0 };
        self.guard_db.store(db.to_bits(), Ordering::Relaxed);
    }

    /// What the guard is holding down right now, in dB. `0.0` means it is not
    /// holding anything — see [`crate::feedback`] for what it does and what it
    /// deliberately does not.
    pub fn guard_db(&self) -> f32 {
        f32::from_bits(self.guard_db.load(Ordering::Relaxed))
    }

    /// `(late blocks, dropped samples)` since the last clear.
    pub fn counts(&self) -> (u32, u32) {
        (
            self.late.load(Ordering::Relaxed),
            self.dropped.load(Ordering::Relaxed),
        )
    }

    /// Start counting again — done when the stream is rebuilt, so the numbers
    /// belong to the device that is open now.
    pub fn clear(&self) {
        self.late.store(0, Ordering::Relaxed);
        self.dropped.store(0, Ordering::Relaxed);
        self.clipped.store(0, Ordering::Relaxed);
        self.guard_db.store(0, Ordering::Relaxed);
    }
}

/// What `A→M` is hearing right now, for the interface to draw.
///
/// The whole reason this exists: without it, a tracker that plays nothing and a
/// tracker that plays the wrong thing look identical from the outside, and the
/// only way to set `SENS` is to guess. With it, the rack says `A→M ● A2 -38dB`
/// and the knob has something to aim at.
///
/// One per process, like the meter: two tabs converting at once would fight
/// over it, and a rack doing that has a stranger problem than this display.
pub struct PitchMeter {
    /// The sounding note plus one; zero is "nothing".
    note: AtomicU32,
    /// How far off that note the heard pitch is, in cents. A display only:
    /// what goes to the plugin is the note, exactly, the way a keyboard sends
    /// it — this is how the player sees the tracker is locked on.
    cents: AtomicI32,
    /// RMS of the window the tracker last looked at (`f32` bits).
    level: AtomicU32,
}

pub fn pitch_meter() -> &'static PitchMeter {
    static M: PitchMeter = PitchMeter {
        note: AtomicU32::new(0),
        cents: AtomicI32::new(0),
        level: AtomicU32::new(0),
    };
    &M
}

impl PitchMeter {
    /// Called from the audio callback: three relaxed stores and nothing else.
    pub fn publish(&self, note: Option<u8>, cents: i32, level: f32) {
        self.note
            .store(note.map_or(0, |n| n as u32 + 1), Ordering::Relaxed);
        self.cents.store(cents, Ordering::Relaxed);
        self.level.store(level.to_bits(), Ordering::Relaxed);
    }

    pub fn note(&self) -> Option<u8> {
        match self.note.load(Ordering::Relaxed) {
            0 => None,
            n => Some((n - 1) as u8),
        }
    }

    pub fn cents(&self) -> i32 {
        self.cents.load(Ordering::Relaxed)
    }

    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }

    pub fn clear(&self) {
        self.publish(None, 0, 0.0);
    }
}

/// How long the audio callback takes against how long it has.
///
/// The one number that separates "a plugin is broken" from "this machine cannot
/// render this rack in time": at 96 kHz with 128-frame blocks a callback has
/// **1.33 ms**, and everything it does — every synth voice, every effect, every
/// round trip to a sandboxed plugin — comes out of that. Past 100 % the device
/// gets a hole instead of a block, which is heard as the sound breaking up and
/// then going away, and which nothing in the interface could say before this.
///
/// Written from the audio thread with relaxed atomics and one `Instant` pair
/// per block, which is a clock read, not a syscall.
pub struct Load {
    /// Microseconds the last block took, and the worst since the last read.
    last_us: AtomicU32,
    /// The same, smoothed: a decaying average of the last few hundred blocks.
    ///
    /// **The readout reads this, not `last_us`.** One block out of the ~190 a
    /// second is a sample of nothing: `elapsed()` is wall-clock, so a block
    /// that happened to be preempted reads as a rack that costs 40 % when the
    /// thread's own CPU time says 4 %. Which is exactly what "the number keeps
    /// climbing the longer choz is open" looked like. The peak is still kept
    /// separately, because a deadline is missed by peaks and not by averages.
    avg_us: AtomicU32,
    peak_us: AtomicU32,
    /// Microseconds the block *had*, from frames and sample rate.
    budget_us: AtomicU32,
    /// Blocks rendered, and blocks that took longer than they had.
    blocks: AtomicU32,
    over: AtomicU32,
    /// The slot that cost the most in the worst block, and what it cost in
    /// microseconds. Without this "the callback ran out of time" names no
    /// culprit, and the tab that is actually expensive is the one thing the
    /// person playing can change.
    worst_slot: AtomicU32,
    worst_slot_us: AtomicU32,
    /// The worst block's own **CPU** time, against the wall time in
    /// `peak_us`. See [`cpu_micros`].
    peak_cpu_us: AtomicU32,
}

/// This thread's own CPU time, in microseconds.
///
/// `CLOCK_THREAD_CPUTIME_ID`: it advances only while the thread is on a CPU, so
/// subtracting two readings gives the work done and not the time passed. One
/// `clock_gettime` per block on the audio thread — the same vDSO call
/// `Instant::now` already makes there, so it costs a second one and no more.
/// Zero where the platform has no such clock, which reads as "we cannot tell"
/// rather than as "it was free".
pub fn cpu_micros() -> u64 {
    #[cfg(target_os = "linux")]
    {
        let mut ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // Safe: `ts` is a valid, owned `timespec` and the call only writes it.
        if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) } == 0 {
            return ts.tv_sec as u64 * 1_000_000 + ts.tv_nsec as u64 / 1_000;
        }
    }
    0
}

pub fn load() -> &'static Load {
    static L: Load = Load {
        last_us: AtomicU32::new(0),
        avg_us: AtomicU32::new(0),
        peak_us: AtomicU32::new(0),
        budget_us: AtomicU32::new(0),
        blocks: AtomicU32::new(0),
        over: AtomicU32::new(0),
        worst_slot: AtomicU32::new(0),
        worst_slot_us: AtomicU32::new(0),
        peak_cpu_us: AtomicU32::new(0),
    };
    &L
}

impl Load {
    /// Called at the end of every block with what it took on the wall clock,
    /// what it took of this thread's own CPU, and what it had.
    ///
    /// **The two are not the same number and the difference is the diagnosis.**
    /// Wall time says the block was late; CPU time says whether choz was busy
    /// or simply not running. A rack costing 20 µs of CPU inside a 1400 µs wall
    /// block was preempted — by a browser, by an IRQ, by anything — and no
    /// amount of making choz faster would have helped. The old line reported
    /// the wall figure as "the tab costs 1.12 ms", which sent people optimising
    /// a rack that was already thirty times inside its budget.
    pub fn publish(
        &self,
        took: std::time::Duration,
        cpu: std::time::Duration,
        budget: std::time::Duration,
    ) {
        let us = took.as_micros().min(u32::MAX as u128) as u32;
        let cpu_us = cpu.as_micros().min(u32::MAX as u128) as u32;
        // The pair is kept together: the CPU figure only means anything beside
        // the wall figure it belongs to.
        if us >= self.peak_us.load(Ordering::Relaxed) {
            self.peak_cpu_us.store(cpu_us, Ordering::Relaxed);
        }
        let budget_us = budget.as_micros().min(u32::MAX as u128) as u32;
        self.last_us.store(us, Ordering::Relaxed);
        // A 1/16 exponential average: ~a tenth of a second at any block size
        // anyone plays at, which is slow enough to stop flickering and fast
        // enough that turning a rack on is seen immediately. Integer maths, on
        // the audio thread.
        let prev = self.avg_us.load(Ordering::Relaxed);
        self.avg_us
            .store(prev - prev / 16 + us / 16, Ordering::Relaxed);
        self.budget_us.store(budget_us, Ordering::Relaxed);
        self.peak_us.fetch_max(us, Ordering::Relaxed);
        self.blocks.fetch_add(1, Ordering::Relaxed);
        if budget_us > 0 && us > budget_us {
            self.over.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Called per slot per block, from the audio thread: which tab this was and
    /// how long its source took. Only the worst since the last read is kept.
    pub fn publish_slot(&self, slot: usize, took: std::time::Duration) {
        let us = took.as_micros().min(u32::MAX as u128) as u32;
        if us > self.worst_slot_us.load(Ordering::Relaxed) {
            self.worst_slot_us.store(us, Ordering::Relaxed);
            self.worst_slot.store(slot as u32, Ordering::Relaxed);
        }
    }

    /// `(tab, milliseconds)` of the most expensive source since the last read.
    pub fn take_worst_slot(&self) -> (usize, f32) {
        let us = self.worst_slot_us.swap(0, Ordering::Relaxed);
        (
            self.worst_slot.load(Ordering::Relaxed) as usize,
            us as f32 / 1000.0,
        )
    }

    /// Microseconds the last block took. The raw sample; the readout wants
    /// [`Self::last`], which is the average.
    pub fn last_block_us(&self) -> u32 {
        self.last_us.load(Ordering::Relaxed)
    }

    /// Fraction of the budget a block is using, averaged. 1.0 is the edge of a
    /// hole.
    pub fn last(&self) -> f32 {
        let (us, budget) = (
            self.avg_us.load(Ordering::Relaxed) as f32,
            self.budget_us.load(Ordering::Relaxed) as f32,
        );
        if budget <= 0.0 {
            0.0
        } else {
            us / budget
        }
    }

    /// `(peak fraction, blocks, blocks over budget)` since the last
    /// [`Self::take`], which is what the health poller reports and resets.
    pub fn take(&self) -> (f32, u32, u32) {
        let budget = self.budget_us.load(Ordering::Relaxed) as f32;
        let peak = self.peak_us.swap(0, Ordering::Relaxed) as f32;
        let blocks = self.blocks.swap(0, Ordering::Relaxed);
        let over = self.over.swap(0, Ordering::Relaxed);
        let peak = if budget <= 0.0 { 0.0 } else { peak / budget };
        (peak, blocks, over)
    }

    /// What the worst block actually spent on a CPU, as a share of its budget.
    ///
    /// Read after [`Self::take`], which is what resets the pair. Well under the
    /// wall figure means the thread was off the CPU, not slow.
    pub fn peak_cpu(&self) -> f32 {
        let budget = self.budget_us.load(Ordering::Relaxed) as f32;
        let cpu = self.peak_cpu_us.swap(0, Ordering::Relaxed) as f32;
        match budget <= 0.0 {
            true => 0.0,
            false => cpu / budget,
        }
    }

    /// Microseconds a block has, for a message that wants the real numbers.
    pub fn budget_us(&self) -> u32 {
        self.budget_us.load(Ordering::Relaxed)
    }

    pub fn clear(&self) {
        self.last_us.store(0, Ordering::Relaxed);
        self.avg_us.store(0, Ordering::Relaxed);
        self.worst_slot_us.store(0, Ordering::Relaxed);
        self.peak_us.store(0, Ordering::Relaxed);
        self.peak_cpu_us.store(0, Ordering::Relaxed);
        self.blocks.store(0, Ordering::Relaxed);
        self.over.store(0, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Wall time and CPU time are two different answers and the load meter
    /// has to keep both.**
    ///
    /// A block that ran long on the wall clock while spending almost no CPU was
    /// not slow — the thread was off the CPU when the deadline went past. The
    /// meter reported only the wall figure, so a rack costing 20 µs inside a
    /// 1400 µs block was written down as "this tab costs 1.4 ms" and the fix
    /// people went looking for was in the wrong process.
    #[test]
    fn the_load_meter_separates_being_slow_from_not_being_scheduled() {
        use std::time::Duration;
        // A local one: the readings are process-wide, and a test that reset
        // them would fight every other test in this file.
        let load = Load {
            last_us: AtomicU32::new(0),
            avg_us: AtomicU32::new(0),
            peak_us: AtomicU32::new(0),
            budget_us: AtomicU32::new(0),
            blocks: AtomicU32::new(0),
            over: AtomicU32::new(0),
            worst_slot: AtomicU32::new(0),
            worst_slot_us: AtomicU32::new(0),
            peak_cpu_us: AtomicU32::new(0),
        };
        let budget = Duration::from_micros(1333);

        // A healthy block, then one that overran the deadline without doing any
        // more work: preempted, not slow.
        load.publish(Duration::from_micros(20), Duration::from_micros(19), budget);
        load.publish(
            Duration::from_micros(1400),
            Duration::from_micros(21),
            budget,
        );
        let (peak, blocks, over) = load.take();
        let cpu = load.peak_cpu();
        assert_eq!((blocks, over), (2, 1));
        assert!(peak > 1.0, "the wall clock says it was late: {peak}");
        assert!(cpu < 0.05, "and the CPU says it was not busy: {cpu}");

        // The other way round: a block that really did the work is not excused.
        load.publish(
            Duration::from_micros(1400),
            Duration::from_micros(1390),
            budget,
        );
        let (peak, ..) = load.take();
        assert!(
            peak > 1.0 && load.peak_cpu() > 1.0,
            "that one was this rack"
        );

        // The pair belongs together: the CPU figure kept is the worst *wall*
        // block's, not the worst CPU block's, or the two would describe
        // different blocks and the comparison would mean nothing.
        load.publish(
            Duration::from_micros(2000),
            Duration::from_micros(30),
            budget,
        );
        load.publish(
            Duration::from_micros(100),
            Duration::from_micros(95),
            budget,
        );
        load.take();
        assert!(
            load.peak_cpu() < 0.05,
            "the CPU reading follows the worst wall block"
        );
    }

    #[test]
    fn a_block_becomes_a_level_and_a_shape() {
        let _g = crate::test_locks::meter();
        let m = meter();
        m.clear();
        assert_eq!(m.peak(), 0.0);

        // A half-scale sine: peak is 0.5, RMS is 0.5/√2.
        let frames = 512;
        let buf: Vec<f32> = (0..frames)
            .flat_map(|i| {
                let s = 0.5 * (2.0 * std::f32::consts::PI * i as f32 / 64.0).sin();
                [s, s]
            })
            .collect();
        m.publish(&buf);
        assert!((m.peak() - 0.5).abs() < 0.01, "peak {}", m.peak());
        assert!((m.rms() - 0.3536).abs() < 0.02, "rms {}", m.rms());

        let wave = m.wave();
        assert_eq!(wave.len(), WAVE_POINTS);
        assert!(
            wave.iter().any(|s| s.abs() > 0.1),
            "the shape is not all zeros"
        );
        assert!(
            wave.iter().all(|s| s.abs() <= 0.51),
            "and none of it is out of range"
        );

        // Silence after a signal reads as silence, not as the last thing heard.
        m.publish(&vec![0.0; 1024]);
        assert_eq!(m.peak(), 0.0);
        m.clear();
    }

    /// Past full scale the device clips, and nothing inside choz can fix a
    /// level. Counting it is what turns "it sounds saturated" into a reading.
    #[test]
    fn going_past_full_scale_is_counted() {
        let _g = crate::test_locks::meter();
        let m = meter();
        m.clear();
        m.publish(&vec![0.5f32; 256]);
        assert_eq!(m.clipping(), 0, "half scale is not clipping");
        m.publish(&vec![1.4f32; 256]);
        assert_eq!(m.clipping(), 1, "past full scale is");
        m.clear();
        assert_eq!(m.clipping(), 0);
    }

    /// An empty block is what a stopped stream hands over; it must not panic or
    /// divide by zero.
    #[test]
    fn an_empty_block_is_harmless() {
        let m = meter();
        m.publish(&[]);
        m.clear();
    }
}
