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

#[cfg(test)]
mod tests {
    use super::*;

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
