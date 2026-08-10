//! Recording what the user moves, and moving it again on the next pass.
//!
//! The addresses are the ones MIDI learn already uses ([`LearnTarget`]): a lane
//! is "this control, over one loop". Nothing new has to be made bindable, and a
//! lane means the same thing in a project file as a CC binding does.
//!
//! **Recording samples, it does not intercept.** The alternative — a hook in
//! every setter (`adjust_fx_param`, `set_instr_param`, the mixer, the CC path,
//! the plugin's own window) — is five places that must never be forgotten. The
//! UI already ticks tens of times a second, faster than a hand moves a knob, so
//! each tick asks what the values are and writes down the ones that changed.
//! What comes out is what a fader move looks like anyway.
//!
//! None of this is on the audio thread. Values reach the engine the way every
//! other UI change does, over the command ring.

use serde::{Deserialize, Serialize};

use crate::LearnTarget;

/// One control's movements over a loop, in beats from its start.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lane {
    pub target: LearnTarget,
    /// `(beat within the loop, value 0..1)`, kept sorted by beat.
    pub points: Vec<(f32, f32)>,
}

impl Lane {
    /// The value at `beat`, holding the last point until the next one.
    ///
    /// A step, not a ramp: what was recorded is where the control *was*, and
    /// inventing motion between two samples would play back something the user
    /// never did. A knob left alone between two moves stays put.
    pub fn value_at(&self, beat: f32) -> Option<f32> {
        let mut current = None;
        for (at, value) in &self.points {
            if *at > beat {
                break;
            }
            current = Some(*value);
        }
        // Before the first point, the loop wraps to the last one: the lane is a
        // circle, and the value at the top of the bar is where it ended.
        current.or_else(|| self.points.last().map(|(_, v)| *v))
    }
}

/// Every lane, plus where the loop ends and what the transport is doing to it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Automation {
    pub lanes: Vec<Lane>,
    /// Loop length in beats. Zero means "not set yet" and is treated as
    /// [`Automation::DEFAULT_BEATS`].
    #[serde(default)]
    pub loop_beats: f32,
    /// Writing down what the user moves.
    #[serde(skip)]
    pub recording: bool,
    /// Playing back what was written down.
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

impl Default for Automation {
    /// Playback on: a project that carries lanes should play them without the
    /// user having to find a switch. `#[serde(default)]` is not enough — it only
    /// speaks for a field a file leaves out.
    fn default() -> Self {
        Self {
            lanes: Vec::new(),
            loop_beats: 0.0,
            recording: false,
            enabled: true,
        }
    }
}

impl Automation {
    /// Four bars of four, which is the length most things loop at, and the one
    /// the user can change without having to know it exists.
    pub const DEFAULT_BEATS: f32 = 16.0;

    pub fn loop_beats(&self) -> f32 {
        if self.loop_beats > 0.0 {
            self.loop_beats
        } else {
            Self::DEFAULT_BEATS
        }
    }

    /// Where in the loop the transport's beat position falls.
    pub fn position(&self, beats: f64) -> f32 {
        let len = self.loop_beats() as f64;
        (beats.rem_euclid(len)) as f32
    }

    pub fn is_empty(&self) -> bool {
        self.lanes.iter().all(|l| l.points.is_empty())
    }

    /// Write down that `target` was at `value` at `beat`.
    ///
    /// A point that repeats the previous value is dropped: a knob nobody touched
    /// would otherwise fill a lane with the same number sixty times a second.
    /// Points arriving out of order (the loop wrapped) start the lane again, so
    /// a second pass replaces the first rather than interleaving with it.
    pub fn record(&mut self, target: LearnTarget, beat: f32, value: f32) {
        let lane = match self.lanes.iter_mut().position(|l| l.target == target) {
            Some(i) => &mut self.lanes[i],
            None => {
                self.lanes.push(Lane {
                    target,
                    points: Vec::new(),
                });
                self.lanes.last_mut().expect("just pushed")
            }
        };
        if let Some((last_beat, last_value)) = lane.points.last().copied() {
            if (last_value - value).abs() < f32::EPSILON {
                return;
            }
            if beat < last_beat {
                // The loop came round: this pass overwrites the previous one.
                lane.points.clear();
            }
        }
        lane.points.push((beat, value));
    }

    /// What every lane says at `beat`. Empty while recording: the user is
    /// driving, and playing back over their hand would fight it.
    pub fn values_at(&self, beat: f32) -> Vec<(LearnTarget, f32)> {
        if self.recording || !self.enabled {
            return Vec::new();
        }
        self.lanes
            .iter()
            .filter_map(|l| l.value_at(beat).map(|v| (l.target, v)))
            .collect()
    }

    /// Forget one control's lane, or all of them.
    pub fn clear(&mut self, target: Option<&LearnTarget>) {
        match target {
            Some(t) => self.lanes.retain(|l| l.target != *t),
            None => self.lanes.clear(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gain(slot: usize) -> LearnTarget {
        LearnTarget::Gain(slot)
    }

    #[test]
    fn a_lane_holds_its_value_until_the_next_point() {
        let lane = Lane {
            target: gain(0),
            points: vec![(0.0, 0.2), (2.0, 0.8)],
        };
        assert_eq!(lane.value_at(0.0), Some(0.2));
        assert_eq!(lane.value_at(1.9), Some(0.2), "held, not ramped");
        assert_eq!(lane.value_at(2.0), Some(0.8));
        assert_eq!(lane.value_at(15.0), Some(0.8));

        // Before the first point the loop wraps to the last: the lane is a
        // circle, so the top of the bar continues where the pass ended.
        let late = Lane {
            target: gain(0),
            points: vec![(4.0, 0.5)],
        };
        assert_eq!(late.value_at(0.0), Some(0.5));

        assert_eq!(
            Lane {
                target: gain(0),
                points: vec![]
            }
            .value_at(0.0),
            None
        );
    }

    #[test]
    fn recording_drops_repeats_and_a_second_pass_replaces_the_first() {
        let mut a = Automation::default();
        a.record(gain(0), 0.0, 0.5);
        a.record(gain(0), 0.5, 0.5);
        a.record(gain(0), 1.0, 0.5);
        assert_eq!(
            a.lanes[0].points.len(),
            1,
            "a knob nobody touched writes one point"
        );

        a.record(gain(0), 2.0, 0.9);
        assert_eq!(a.lanes[0].points.len(), 2);

        // The loop came round: the new pass is the lane now.
        a.record(gain(0), 0.1, 0.3);
        assert_eq!(a.lanes[0].points, vec![(0.1, 0.3)]);

        // Another control is another lane, not another point.
        a.record(LearnTarget::Pan(1), 0.2, 0.7);
        assert_eq!(a.lanes.len(), 2);
    }

    #[test]
    fn the_loop_wraps_and_playback_stops_while_recording() {
        let mut a = Automation {
            loop_beats: 4.0,
            ..Automation::default()
        };
        assert_eq!(a.position(0.0), 0.0);
        assert_eq!(a.position(4.0), 0.0, "one loop on");
        assert_eq!(a.position(5.5), 1.5);
        // A default-length loop is four bars of four.
        assert_eq!(
            Automation::default().loop_beats(),
            Automation::DEFAULT_BEATS
        );

        a.record(gain(0), 0.0, 0.25);
        assert_eq!(a.values_at(1.0), vec![(gain(0), 0.25)]);

        a.recording = true;
        assert!(
            a.values_at(1.0).is_empty(),
            "the user's hand wins while recording"
        );
        a.recording = false;
        a.enabled = false;
        assert!(
            a.values_at(1.0).is_empty(),
            "and nothing plays when it is off"
        );

        a.enabled = true;
        a.clear(Some(&gain(0)));
        assert!(a.is_empty());
    }
}
