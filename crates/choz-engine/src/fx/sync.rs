//! Tempo sync for the effects that have a time: a knob that steps through
//! note values, with "off" at the bottom so a project from before the knob
//! existed — which has nothing there, and gets 0 — keeps its free time.
//!
//! The clock is choz's own transport: its playhead while it rolls, the free
//! clock while it is stopped. Both run at the session's tempo, so a synced
//! echo is on the beat whether or not anything is playing.

use choz_ports::transport;

/// Note values, in quarter notes. Index 0 is "off".
pub const DIVISIONS: [(f32, &str); 14] = [
    (0.0, "FREE"),
    (0.125, "1/32"),
    (1.0 / 6.0, "1/16T"),
    (0.25, "1/16"),
    (1.0 / 3.0, "1/8T"),
    (0.375, "1/16."),
    (0.5, "1/8"),
    (2.0 / 3.0, "1/4T"),
    (0.75, "1/8."),
    (1.0, "1/4"),
    (4.0 / 3.0, "1/2T"),
    (1.5, "1/4."),
    (2.0, "1/2"),
    (4.0, "1 bar"),
];

/// Which division a 0..1 knob sits on.
pub fn index_of(v: f32) -> usize {
    ((v.clamp(0.0, 1.0) * (DIVISIONS.len() - 1) as f32).round() as usize).min(DIVISIONS.len() - 1)
}

/// The knob position a division sits at.
pub fn norm_of(index: usize) -> f32 {
    index.min(DIVISIONS.len() - 1) as f32 / (DIVISIONS.len() - 1) as f32
}

/// The division in quarter notes, or `None` for "off".
pub fn quarters(index: usize) -> Option<f32> {
    match index {
        0 => None,
        i => Some(DIVISIONS[i.min(DIVISIONS.len() - 1)].0),
    }
}

/// `quarters` at the session tempo, in samples.
pub fn samples(quarters: f32, sample_rate: f32) -> f32 {
    quarters * 60.0 / transport().bpm().max(1.0) * sample_rate
}

/// Where the grid is, in quarter notes: the playhead while rolling, the free
/// clock while stopped.
pub fn position() -> f64 {
    let t = transport();
    match t.playing() {
        true => t.ppq(),
        false => t.free_ppq(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bottom_of_the_knob_is_off_and_the_rest_are_notes() {
        assert_eq!(quarters(index_of(0.0)), None);
        assert_eq!(quarters(index_of(1.0)), Some(4.0));
        for i in 0..DIVISIONS.len() {
            assert_eq!(index_of(norm_of(i)), i);
        }
        let mut last = 0.0;
        for (q, _) in DIVISIONS.iter().skip(1) {
            assert!(*q > last, "divisions rise");
            last = *q;
        }
    }
}
