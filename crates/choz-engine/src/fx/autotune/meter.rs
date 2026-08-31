//! What AutoTune is hearing, for the interface to draw.
//!
//! Same contract as [`crate::meter`]: the audio thread stores a handful of
//! atomics and the UI reads them when it redraws. No locks, because the writer
//! is the audio callback; no channel, because a reading one block stale is a
//! reading that is right.
//!
//! One per process. Two AutoTunes in a rack share it and the last one to run
//! wins — a display, not a routing matrix.

use std::sync::atomic::{AtomicU32, Ordering};

/// A copyable snapshot, which is what the UI actually wants.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AutoTuneMeter {
    pub detected_frequency: f32,
    pub target_frequency: f32,
    pub pitch_error_cents: f32,
    pub confidence: f32,
    pub voiced: bool,
    /// Input RMS, for the level bar.
    pub level: f32,
}

pub struct SharedMeter {
    detected: AtomicU32,
    target: AtomicU32,
    cents: AtomicU32,
    confidence: AtomicU32,
    level: AtomicU32,
    voiced: AtomicU32,
}

static METER: SharedMeter = SharedMeter::new();

pub fn meter() -> &'static SharedMeter {
    &METER
}

impl SharedMeter {
    /// An empty one.
    ///
    /// Public so a test can hold its own rather than reach for [`meter()`]:
    /// the process-wide one is written by every `AutoTune` that runs, and a
    /// test that reads it while another test's effect is processing is a test
    /// that fails for a reason that has nothing to do with it.
    pub const fn new() -> Self {
        Self {
            detected: AtomicU32::new(0),
            target: AtomicU32::new(0),
            cents: AtomicU32::new(0),
            confidence: AtomicU32::new(0),
            level: AtomicU32::new(0),
            voiced: AtomicU32::new(0),
        }
    }

    /// Called from the audio callback: six relaxed stores, no more.
    pub fn publish(&self, m: AutoTuneMeter) {
        self.detected
            .store(m.detected_frequency.to_bits(), Ordering::Relaxed);
        self.target
            .store(m.target_frequency.to_bits(), Ordering::Relaxed);
        self.cents
            .store(m.pitch_error_cents.to_bits(), Ordering::Relaxed);
        self.confidence
            .store(m.confidence.to_bits(), Ordering::Relaxed);
        self.level.store(m.level.to_bits(), Ordering::Relaxed);
        self.voiced.store(m.voiced as u32, Ordering::Relaxed);
    }

    pub fn read(&self) -> AutoTuneMeter {
        let f = |a: &AtomicU32| f32::from_bits(a.load(Ordering::Relaxed));
        AutoTuneMeter {
            detected_frequency: f(&self.detected),
            target_frequency: f(&self.target),
            pitch_error_cents: f(&self.cents),
            confidence: f(&self.confidence),
            level: f(&self.level),
            voiced: self.voiced.load(Ordering::Relaxed) != 0,
        }
    }

    pub fn clear(&self) {
        self.publish(AutoTuneMeter::default());
    }
}

impl Default for SharedMeter {
    fn default() -> Self {
        Self::new()
    }
}
