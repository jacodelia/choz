//! Envelope — an ADHSR contour re-imposed on whatever comes in.
//!
//! The chain already has effects that *follow* an envelope (the gate, the
//! compressor, the auto-filter). This one **writes** it: the input's own level
//! only decides *when* a note starts, and from there the shape is the five
//! knobs' and not the source's. That is what turns a sustained pad into a
//! plucked one, a piano into a swell, or a loop into something that breathes on
//! a shape you drew rather than the one it arrived with.
//!
//! **Not a transient designer.** A transient designer scales the attack the
//! signal already has; this replaces it. The difference shows on a sound with
//! no transient at all — a held organ note — where a designer has nothing to
//! work on and this one can still pluck it.
//!
//! Retriggered by a fast peak follower, which is what makes it usable on a
//! whole mix: it is the note starting, not a sidechain input choz does not have
//! here.
//!
//! **The gate closes relative to the note's own peak, not at a level in dB.**
//! An absolute threshold has to be set for the material and then set again the
//! moment anything upstream changes gain — and set too low (which -40 dB is,
//! for anything real) the gate never closes at all, so the release stage is
//! only ever reached after the sound has already gone and the Release knob does
//! nothing anybody can hear. `Length` instead says how far below its own peak
//! the note has to fall before the release takes over, which means the same
//! thing at any input level.

use super::{FxParam, FxProcessor};

/// Where the contour is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Nothing has crossed the threshold yet, and the gain sits at the floor.
    Idle,
    Attack,
    Hold,
    Decay,
    Sustain,
    Release,
}

pub struct Envelope {
    attack_ms: f32,
    hold_ms: f32,
    decay_ms: f32,
    /// Where the contour holds while the input stays above the threshold.
    sustain: f32,
    release_ms: f32,
    /// How far below the note's own peak the input has to fall for the release
    /// to take over, as a fraction of that peak. Small = the contour holds on
    /// for most of the note; large = it lets go almost at once.
    fall_to: f32,
    /// How much of the contour is applied: 0 passes the input untouched, 1
    /// hands over the level entirely.
    depth: f32,
    mix: f32,

    stage: Stage,
    /// The contour's current value, 0..1.
    env: f32,
    /// Samples left in `Hold`, which is the one stage that is a wait.
    hold_left: usize,
    /// Fast peak follower on the input — the trigger, not the shape.
    follow: f32,
    /// The loudest the follower has been since the gate opened. What `fall_to`
    /// is a fraction of.
    peak: f32,
    /// The quietest the follower has been since the gate closed. What a new
    /// note has to rise above to trigger the contour again.
    floor: f32,
    open: bool,
    sample_rate: f32,
}

/// The longest each stage can be asked for, in milliseconds. Release is the
/// long one on purpose: it is what a swell is made of.
const MAX_ATTACK_MS: f32 = 1000.0;
const MAX_HOLD_MS: f32 = 500.0;
const MAX_DECAY_MS: f32 = 2000.0;
const MAX_RELEASE_MS: f32 = 4000.0;

/// How fast the trigger follower falls, in milliseconds. Fast enough to let a
/// second note retrigger, slow enough not to retrigger inside one waveform —
/// which at 50 Hz would be forty times a second.
const FOLLOW_RELEASE_MS: f32 = 40.0;

/// Below this the input is noise, not a note, and nothing triggers. Fixed
/// rather than a knob: it exists to stop the contour firing on a silent
/// channel's own hiss, which is not a musical decision.
const FLOOR_DB: f32 = -60.0;

/// What `Length` runs between, as a fraction of the note's own peak. At the
/// short end the contour lets go as soon as the note is a hair off its peak; at
/// the long end it holds until the note is all but gone.
const LENGTH_MIN: f32 = 0.02;
const LENGTH_MAX: f32 = 0.90;

/// How far the input has to rise above where it has settled before the contour
/// fires again — 6 dB, which is a note being played and not the one before it
/// still ringing.
///
/// **Without this the effect eats its own release.** The gate closes on a note
/// that is still perfectly audible (that is the point of `Length`), and a
/// contour that re-armed on level alone would find the input still above the
/// floor and retrigger on the next sample — an endless sawtooth, and a Release
/// knob that could never be heard because nothing ever finished releasing.
const REARM: f32 = 2.0;

impl Envelope {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            attack_ms: 5.0,
            hold_ms: 0.0,
            decay_ms: 200.0,
            sustain: 0.7,
            release_ms: 300.0,
            fall_to: LENGTH_MAX - 0.6 * (LENGTH_MAX - LENGTH_MIN),
            depth: 1.0,
            mix: 1.0,
            stage: Stage::Idle,
            env: 0.0,
            hold_left: 0,
            follow: 0.0,
            peak: 0.0,
            // **At the noise floor, not at full scale.** Started high, the
            // floor was pulled down to the first sample's own level before the
            // rise could be seen, and a signal that begins at once — every test
            // tone, and a channel unmuted mid-note — never triggered at all.
            floor: db_to_lin(FLOOR_DB),
            open: false,
            sample_rate: sample_rate.max(8000) as f32,
        }
    }

    /// Build from the rack's knob positions, in `set_param` order.
    pub fn with_params(sample_rate: u32, p: &[f32]) -> Self {
        let mut e = Self::new(sample_rate);
        for (i, v) in p.iter().enumerate() {
            e.set_param(i, *v);
        }
        e
    }

    /// Milliseconds as a per-sample step over the whole 0..1 travel. A stage
    /// asked for zero moves in one sample rather than dividing by nothing.
    fn step(&self, ms: f32) -> f32 {
        let samples = (ms * 0.001 * self.sample_rate).max(1.0);
        1.0 / samples
    }

    /// One sample of contour. Split out so a test can run it without audio.
    fn tick(&mut self, level: f32) -> f32 {
        // The trigger follower: instant up, slow down.
        let fall = (-1.0 / (FOLLOW_RELEASE_MS * 0.001 * self.sample_rate)).exp();
        self.follow = level.max(self.follow * fall);

        // Opening starts the contour from where it is rather than from zero: a
        // note retriggered inside the tail of the last one must not click.
        if !self.open {
            // Tested before the floor is pulled down, or a step up would be
            // swallowed by the same sample that should have triggered on it.
            if self.follow > self.floor * REARM {
                self.open = true;
                self.peak = self.follow;
                self.stage = Stage::Attack;
            } else {
                // While it is closed the floor tracks the input down, so a note
                // dying away never retriggers and the next one always does.
                self.floor = self.floor.min(self.follow).max(db_to_lin(FLOOR_DB));
            }
        } else {
            self.peak = self.peak.max(self.follow);
            if self.follow < self.peak * self.fall_to {
                self.open = false;
                self.floor = self.follow;
                self.stage = Stage::Release;
            }
        }

        match self.stage {
            Stage::Idle => self.env = 0.0,
            Stage::Attack => {
                self.env += self.step(self.attack_ms);
                if self.env >= 1.0 {
                    self.env = 1.0;
                    self.hold_left = (self.hold_ms * 0.001 * self.sample_rate) as usize;
                    self.stage = Stage::Hold;
                }
            }
            Stage::Hold => match self.hold_left.checked_sub(1) {
                Some(left) => self.hold_left = left,
                None => self.stage = Stage::Decay,
            },
            Stage::Decay => {
                // The decay falls from full scale to the sustain over the time
                // asked for, whatever that distance happens to be — a decay
                // that took the same time to fall 3 dB as to fall 30 would be
                // a different control at every sustain setting.
                self.env -= self.step(self.decay_ms) * (1.0 - self.sustain);
                if self.env <= self.sustain {
                    self.env = self.sustain;
                    self.stage = Stage::Sustain;
                }
            }
            Stage::Sustain => self.env = self.sustain,
            Stage::Release => {
                self.env -= self.step(self.release_ms) * self.sustain.max(1e-3);
                if self.env <= 0.0 {
                    self.env = 0.0;
                    self.stage = Stage::Idle;
                }
            }
        }
        self.env.clamp(0.0, 1.0)
    }
}

fn db_to_lin(db: f32) -> f32 {
    10.0f32.powf(db / 20.0)
}

impl FxProcessor for Envelope {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        let sr = sample_rate.max(8000) as f32;
        if (sr - self.sample_rate).abs() > 0.5 {
            self.sample_rate = sr;
        }
        let (depth, mix) = (self.depth, self.mix);
        for frame in buf.chunks_exact_mut(2) {
            let (dry_l, dry_r) = (frame[0], frame[1]);
            let env = self.tick(dry_l.abs().max(dry_r.abs()));
            // `depth` fades between the input's own level and the contour's, so
            // the knob is "how much of this shape", not a second output gain.
            let g = 1.0 - depth + depth * env;
            frame[0] = dry_l + mix * (dry_l * g - dry_l);
            frame[1] = dry_r + mix * (dry_r * g - dry_r);
        }
    }

    fn reset(&mut self) {
        self.stage = Stage::Idle;
        self.env = 0.0;
        self.follow = 0.0;
        self.peak = 0.0;
        self.floor = db_to_lin(FLOOR_DB);
        self.open = false;
        self.hold_left = 0;
    }

    fn set_mix(&mut self, wet: f32) {
        self.mix = wet.clamp(0.0, 1.0);
    }

    fn name(&self) -> &str {
        "Envelope"
    }

    fn params(&self) -> Vec<FxParam> {
        vec![
            FxParam::new(
                "Attack",
                self.attack_ms / MAX_ATTACK_MS,
                0.0,
                MAX_ATTACK_MS,
                "ms",
            ),
            FxParam::new("Hold", self.hold_ms / MAX_HOLD_MS, 0.0, MAX_HOLD_MS, "ms"),
            FxParam::new(
                "Decay",
                self.decay_ms / MAX_DECAY_MS,
                0.0,
                MAX_DECAY_MS,
                "ms",
            ),
            FxParam::new("Sustain", self.sustain, 0.0, 1.0, ""),
            FxParam::new(
                "Release",
                self.release_ms / MAX_RELEASE_MS,
                0.0,
                MAX_RELEASE_MS,
                "ms",
            ),
            FxParam::new(
                "Length",
                (LENGTH_MAX - self.fall_to) / (LENGTH_MAX - LENGTH_MIN),
                0.0,
                1.0,
                "",
            ),
            FxParam::new("Depth", self.depth, 0.0, 1.0, ""),
            FxParam::new("Wet", self.mix, 0.0, 1.0, ""),
        ]
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => self.attack_ms = v * MAX_ATTACK_MS,
            1 => self.hold_ms = v * MAX_HOLD_MS,
            2 => self.decay_ms = v * MAX_DECAY_MS,
            3 => self.sustain = v,
            4 => self.release_ms = v * MAX_RELEASE_MS,
            // Up is longer, so the knob and the fraction run opposite ways.
            5 => self.fall_to = LENGTH_MAX - v * (LENGTH_MAX - LENGTH_MIN),
            6 => self.depth = v,
            7 => self.mix = v,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    /// A block of steady full-scale signal.
    fn tone(frames: usize) -> Vec<f32> {
        (0..frames * 2).map(|_| 0.8).collect()
    }

    fn peak(buf: &[f32]) -> f32 {
        buf.iter().fold(0.0f32, |m, s| m.max(s.abs()))
    }

    /// The whole point: a signal that arrives flat leaves with the shape the
    /// knobs drew. A slow attack has to start quiet and a fast one must not.
    #[test]
    fn a_flat_input_comes_out_with_the_contour_on_it() {
        let mut e = Envelope::new(SR);
        e.set_param(0, 0.2); // 200 ms attack
        e.set_param(6, 1.0); // full depth

        // The first ten milliseconds of a 200 ms attack are nearly silent.
        let mut head = tone(480);
        e.process_block(&mut head, SR);
        assert!(
            peak(&head) < 0.1,
            "the attack has barely started, got {}",
            peak(&head)
        );

        // Half a second in, the contour is up and the signal is through.
        let mut body = tone(SR as usize / 2);
        e.process_block(&mut body, SR);
        let tail = &body[body.len() - 200..];
        assert!(peak(tail) > 0.4, "the contour opened, got {}", peak(tail));

        // A fast attack passes the same input straight away.
        let mut e = Envelope::new(SR);
        e.set_param(0, 0.0);
        e.set_param(6, 1.0);
        let mut head = tone(480);
        e.process_block(&mut head, SR);
        assert!(peak(&head) > 0.5, "a 0 ms attack is open at once");
    }

    /// Depth is the mix between the input's own level and the contour's, so at
    /// zero the effect is a wire — the setting anything in a chain must have.
    #[test]
    fn at_zero_depth_it_is_a_wire() {
        let mut e = Envelope::new(SR);
        e.set_param(0, 1.0); // the slowest attack there is
        e.set_param(6, 0.0);
        let mut buf = tone(512);
        let before = buf.clone();
        e.process_block(&mut buf, SR);
        for (a, b) in buf.iter().zip(before.iter()) {
            assert!((a - b).abs() < 1e-6, "{a} != {b}");
        }
    }

    /// It retriggers on a new note and lets go when the input does. A contour
    /// that only ever fires once is a fade-in, not an envelope.
    #[test]
    fn it_opens_on_a_note_and_releases_when_the_input_stops() {
        let mut e = Envelope::new(SR);
        e.set_param(0, 0.0); // instant attack
        e.set_param(2, 0.0); // instant decay
        e.set_param(3, 1.0); // hold at full
        e.set_param(4, 0.01); // 40 ms release
        e.set_param(6, 1.0);

        let mut buf = tone(2048);
        e.process_block(&mut buf, SR);
        assert!(peak(&buf) > 0.5, "the note opened it");

        // Silence: the follower falls, the contour releases, and it lands idle.
        let mut quiet = vec![0.0f32; SR as usize];
        e.process_block(&mut quiet, SR);
        assert_eq!(e.stage, Stage::Idle, "it let go");

        // …and the next note opens it again.
        let mut again = tone(2048);
        e.process_block(&mut again, SR);
        assert!(peak(&again) > 0.5, "it retriggered");
    }

    /// **The contour has to finish before it fires again.**
    ///
    /// It re-armed on level alone at first, and the gate closes while the note
    /// is still perfectly audible — that is what `Length` is for — so the input
    /// was still above the floor on the next sample and the contour retriggered
    /// straight into another attack. The output was a sawtooth, and Release
    /// could never be heard because nothing ever finished releasing: every
    /// setting of it sounded identical, which is exactly how it was reported.
    #[test]
    fn a_note_that_is_still_ringing_does_not_retrigger_it() {
        // A plucked note: it decays but never stops, which is the case that
        // broke.
        let n = SR as usize * 3;
        let note: Vec<f32> = (0..n)
            .flat_map(|i| {
                let t = i as f32 / SR as f32;
                let s = (t * 220.0 * std::f32::consts::TAU).sin() * (-t * 1.2).exp() * 0.8;
                [s, s]
            })
            .collect();

        let shaped = |release: f32| -> Vec<f32> {
            let mut e = Envelope::new(SR);
            e.set_param(4, release);
            e.set_param(5, 0.6); // the default length
            e.set_param(6, 1.0);
            let mut buf = note.clone();
            e.process_block(&mut buf, SR);
            buf
        };
        /// Where the output last rose — a contour that fired once has one.
        fn rises(buf: &[f32]) -> usize {
            let win = SR as usize / 20 * 2;
            let mut last = f32::MAX;
            let mut n = 0;
            for c in buf.chunks(win) {
                let p = c.iter().fold(0.0f32, |m, v| m.max(v.abs()));
                if p > last * 1.5 && p > 0.01 {
                    n += 1;
                }
                last = p;
            }
            n
        }
        assert_eq!(rises(&shaped(0.3)), 0, "one note, one contour");

        // And with that fixed, the release is audible: a long one leaves the
        // note ringing well past where a short one has cut it.
        let last_sound = |buf: &[f32]| -> usize {
            buf.chunks(SR as usize / 20 * 2)
                .rposition(|c| c.iter().any(|v| v.abs() > 0.01))
                .unwrap_or(0)
        };
        let (short, long) = (shaped(0.0), shaped(1.0));
        assert!(
            last_sound(&long) > last_sound(&short) + 4,
            "release must be heard: {} vs {}",
            last_sound(&long),
            last_sound(&short)
        );
    }

    /// Every knob round-trips through `params`, which is what the rack, a saved
    /// project and the CLAP export all read it back through.
    #[test]
    fn the_knobs_read_back_where_they_were_put() {
        let mut e = Envelope::new(SR);
        let want = [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8];
        for (i, v) in want.iter().enumerate() {
            e.set_param(i, *v);
        }
        for (i, p) in e.params().iter().enumerate() {
            assert!(
                (p.value - want[i]).abs() < 0.02,
                "{} came back at {} not {}",
                p.name,
                p.value,
                want[i]
            );
        }
        assert_eq!(e.params().len(), want.len(), "one knob per parameter");
    }
}
