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

/// The shared meter. One per process, like the transport: there is one output.
pub struct Meter {
    peak: AtomicU32,
    rms: AtomicU32,
    /// Ring of recent samples (mono, `f32` bits), and where the next one goes.
    wave: [AtomicU32; WAVE_POINTS],
    write: AtomicUsize,
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
            wave: [Self::ZERO; WAVE_POINTS],
            write: AtomicUsize::new(0),
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
    }

    pub fn peak(&self) -> f32 {
        f32::from_bits(self.peak.load(Ordering::Relaxed))
    }

    pub fn rms(&self) -> f32 {
        f32::from_bits(self.rms.load(Ordering::Relaxed))
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

    /// Forget everything — a stream that stopped should not leave a frozen
    /// picture of the last thing it played.
    pub fn clear(&self) {
        self.peak.store(0, Ordering::Relaxed);
        self.rms.store(0, Ordering::Relaxed);
        for cell in self.wave.iter() {
            cell.store(0, Ordering::Relaxed);
        }
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
        self.note.store(note.map_or(0, |n| n as u32 + 1), Ordering::Relaxed);
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
        assert!(wave.iter().any(|s| s.abs() > 0.1), "the shape is not all zeros");
        assert!(wave.iter().all(|s| s.abs() <= 0.51), "and none of it is out of range");

        // Silence after a signal reads as silence, not as the last thing heard.
        m.publish(&vec![0.0; 1024]);
        assert_eq!(m.peak(), 0.0);
        m.clear();
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
