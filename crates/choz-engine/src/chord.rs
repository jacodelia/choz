//! The chord being held right now, for the one effect that asks for it.
//!
//! # Why this exists at all
//!
//! An [`crate::fx::FxProcessor`] is handed audio and nothing else. That is the
//! design and it stays: an FX chain carries a signal, and everything that reads
//! or writes *notes* lives in the interface, where the routing is. The
//! harmoniser is the exception that was asked for — "make the harmony follow
//! what I play on the piano" — and this is the smallest door that lets it in
//! without putting a note port on every effect in the program.
//!
//! So: one chord, published by the interface, read by whoever wants it, exactly
//! like [`choz_ports::transport`] is one clock. It carries **what is held**, not
//! what was pressed: a harmoniser needs the shape of the chord under the singer,
//! not a stream of events.
//!
//! # What it deliberately is not
//!
//! **One per process.** Two harmonisers in a rack read the same chord — which
//! is why the interface only ever publishes the notes of the **active tab**,
//! and why the whole feature is switched off in the multi-timbral rack mode
//! where every tab answers a different keyboard. A second chord would be a
//! routing matrix, and that is a bigger thing than this asked to be.
//!
//! Realtime-safe by construction: nine atomics, no allocation, no lock. The
//! writer is the interface thread and the reader is the audio callback.

use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};

/// Most notes a chord carries. Eight is the harmoniser's voice count, and a
/// hand holding more than eight notes is not asking for a harmony.
pub const MAX_NOTES: usize = 8;

pub struct Chord {
    notes: [AtomicU8; MAX_NOTES],
    count: AtomicU8,
    /// Bumped on every change, so a reader can tell "the same chord again" from
    /// "a new one that happens to look alike" without comparing arrays on the
    /// audio thread.
    generation: AtomicU32,
}

static CHORD: Chord = Chord {
    notes: [
        AtomicU8::new(0),
        AtomicU8::new(0),
        AtomicU8::new(0),
        AtomicU8::new(0),
        AtomicU8::new(0),
        AtomicU8::new(0),
        AtomicU8::new(0),
        AtomicU8::new(0),
    ],
    count: AtomicU8::new(0),
    generation: AtomicU32::new(0),
};

/// The one chord. See the module docs for why there is exactly one.
pub fn chord() -> &'static Chord {
    &CHORD
}

impl Chord {
    /// Publish what is held now. Called from the interface loop; `notes` is
    /// sorted low to high by the caller, because the lowest one is the root the
    /// harmony is measured from.
    pub fn set(&self, notes: &[u8]) {
        let n = notes.len().min(MAX_NOTES);
        for (slot, note) in self.notes.iter().zip(notes.iter().take(n)) {
            slot.store(*note, Ordering::Relaxed);
        }
        self.count.store(n as u8, Ordering::Release);
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    /// Nothing is held. Kept separate from `set(&[])` only for how it reads at
    /// the call site.
    pub fn clear(&self) {
        self.set(&[]);
    }

    /// Copy the held notes out. Returns how many were written.
    pub fn read(&self, out: &mut [u8; MAX_NOTES]) -> usize {
        let n = (self.count.load(Ordering::Acquire) as usize).min(MAX_NOTES);
        for (slot, note) in out.iter_mut().zip(self.notes.iter()).take(n) {
            *slot = note.load(Ordering::Relaxed);
        }
        n
    }

    /// How many times the chord has changed. A reader that has seen this number
    /// has seen this chord.
    pub fn generation(&self) -> u32 {
        self.generation.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_published_is_what_is_read_back() {
        let c = chord();
        c.set(&[60, 64, 67]);
        let mut out = [0u8; MAX_NOTES];
        assert_eq!(c.read(&mut out), 3);
        assert_eq!(&out[..3], &[60, 64, 67]);

        // A change is visible as a change, without comparing the notes.
        let before = c.generation();
        c.set(&[62, 65, 69]);
        assert_ne!(c.generation(), before);
        assert_eq!(c.read(&mut out), 3);
        assert_eq!(&out[..3], &[62, 65, 69]);

        c.clear();
        assert_eq!(c.read(&mut out), 0);

        // More notes than a chord carries: the first eight, not a panic.
        c.set(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        assert_eq!(c.read(&mut out), MAX_NOTES);
        assert_eq!(out[MAX_NOTES - 1], 8);
        c.clear();
    }
}
