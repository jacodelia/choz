//! DC blocking.
//!
//! An asymmetric curve (a tape or a groove, not a plain tanh on a plain sine)
//! leaves a bias behind, and a bias costs headroom in every effect that comes
//! after it in the chain. One pole at ~10 Hz: low enough not to touch a bass
//! note, high enough that a change in bias settles in a few tens of
//! milliseconds instead of sagging.

/// A one-pole high pass that only removes the offset.
#[derive(Clone, Copy)]
pub struct DcBlock {
    x1: f32,
    y1: f32,
    r: f32,
}

impl DcBlock {
    pub fn new(sr: f32) -> Self {
        Self {
            x1: 0.0,
            y1: 0.0,
            r: 1.0 - (2.0 * std::f32::consts::PI * 10.0 / sr.max(8000.0)),
        }
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = x - self.x1 + self.r * self.y1;
        self.x1 = x;
        self.y1 = y;
        y
    }

    pub fn reset(&mut self) {
        self.x1 = 0.0;
        self.y1 = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_steady_offset_decays_to_nothing() {
        let mut dc = DcBlock::new(48000.0);
        let mut last = 0.0;
        for _ in 0..48000 {
            last = dc.process(0.5);
        }
        assert!(last.abs() < 1e-3, "offset survived: {last}");
    }

    #[test]
    fn a_bass_note_goes_through() {
        let mut dc = DcBlock::new(48000.0);
        let mut peak: f32 = 0.0;
        for i in 0..48000 {
            let s = (std::f32::consts::TAU * 60.0 * i as f32 / 48000.0).sin();
            let y = dc.process(s);
            if i > 4800 {
                peak = peak.max(y.abs());
            }
        }
        assert!(peak > 0.95, "60 Hz lost too much: {peak}");
    }
}
