//! Catching the howl before the room does.
//!
//! A microphone in front of the speakers it is feeding is a loop, and a loop
//! whose gain passes one at any frequency runs away. Nothing about choz makes
//! that more likely than any other host — but the effects people reach for
//! make it *much* easier to cross the line: a distortion is a compressor with
//! makeup gain, so it lifts whatever is quietest towards unity, and a reverb
//! plus a delay adds copies of the signal back to itself for seconds after it
//! stopped. Both are asked for on purpose, and both push the loop gain up.
//!
//! ## What this is, and what it is not
//!
//! **It is a runaway catcher, not a feedback suppressor.** A real suppressor
//! finds the ringing frequency and notches it, keeping the rest of the signal;
//! that is a bank of tracking filters and a different project. This watches the
//! input for the one thing a howl always does — **grow, and keep growing** —
//! and pulls the input down when it sees it, smoothly, and lets it back up when
//! the room goes quiet.
//!
//! What it will not do:
//!
//! * **It will not catch a howl that is already steady.** Something that came up
//!   before choz started, or grew while this was off, looks like a loud held
//!   note. Growth is the whole signal.
//! * **It will not save a badly placed microphone.** It buys the seconds it
//!   takes to reach a fader; the fix is aiming, distance and gain.
//!
//! What it will not do *wrongly* matters as much: a struck note decays, and a
//! held one is flat. Only something that climbs for a fifth of a second, from
//! already-loud, is treated as a loop.
//!
//! ## Where it sits
//!
//! On the **input**, before the trim and before anything else, because that is
//! where the loop is closed. Ducking the output would leave the tab's own
//! reverb tail feeding the microphone; ducking the input stops the round trip.
//!
//! Realtime-safe: a handful of floats per sample, one branch, no allocation.
//! Global on/off (Settings → AUDIO), because a guard is a property of the room
//! rather than of a tab, and because a switch nobody can find in a panic is not
//! a switch.

use std::sync::atomic::{AtomicBool, Ordering};

/// Level under which nothing is ever treated as feedback, linear.
///
/// -26 dBFS. Below this a loop is not yet a problem, and quiet material that
/// happens to be swelling — a bowed note, a fade-in — must not be ducked.
const FLOOR: f32 = 0.05;

/// How much louder than it was **half a second ago** the signal has to be for
/// that stretch to count as growth.
///
/// Measured against the oldest reading in the history rather than against the
/// previous check, which is what makes this a rate and not a jump: a loop
/// climbing 6 dB a second gains only 0.4 dB between two 64 ms checks and the
/// per-check version never saw it at all, while the attack of a sung note
/// gains far more than that and tripped it every time. +3 dB over half a
/// second is ~4.5 dB/s, which is about as slow as a runaway gets before it
/// stops being one.
const GROWTH: f32 = 1.3;

/// Consecutive growing windows before the guard acts.
///
/// **This is the number that separates a singer from a loop, and it can only
/// be time.** Both grow; a note stops growing once it has arrived and a loop
/// does not. Sixteen checks is a second of *continuous* growth on top of the
/// half second the window itself covers — about 1.5 s in all, which no sung
/// entrance, bowed swell or crescendo reaches and which a howl passes without
/// noticing. The old value was three checks (~200 ms) measured per check, and
/// it pulled an ordinary 600 ms swell into a long note down by 18 dB and then
/// held it there for as long as the singer kept singing.
const GROWTH_CHECKS: u32 = 16;

/// Checks with no growth before a duck is let go again.
///
/// The other half of the same bug: the duck used to lift only when the room
/// went **quiet**, so a guard that fired on a singer stayed on until the singer
/// stopped — which is precisely the note it was ruining. A loop pulled 18 dB
/// down is a loop that is no longer running away, so "it stopped growing" is
/// the honest signal to open again. A real one climbs again and is caught
/// again, and that pumping is the guard telling the room something is wrong.
const CALM_CHECKS: u32 = 6;

/// How far the input is pulled down once a loop is recognised: -18 dB, which
/// takes any realistic loop gain back under one.
const DUCK: f32 = 0.126;

/// Seconds. The envelope's release, how often growth is looked at, how fast the
/// duck arrives, and how slowly it lets go.
///
/// The release is the one worth arguing about: too fast and it opens straight
/// back into the howl it just caught; too slow and a stage goes quiet for a
/// bar after one squeal. A second and a bit is long enough that the loop has to
/// build again from scratch, and short enough that nobody waits for it.
const ENV_RELEASE_S: f32 = 0.05;
const CHECK_S: f32 = 0.064;
const DUCK_ATTACK_S: f32 = 0.02;
const DUCK_RELEASE_S: f32 = 1.2;

/// Envelope readings kept: eight checks, half a second of history.
const HISTORY: usize = 8;

/// Whether the guard is armed at all. Global to the process: it is a property
/// of the room, and every capture path shares one room.
static ARMED: AtomicBool = AtomicBool::new(true);

pub fn armed() -> bool {
    ARMED.load(Ordering::Relaxed)
}

pub fn arm(on: bool) {
    ARMED.store(on, Ordering::Relaxed);
}

/// One capture path's runaway catcher.
pub struct FeedbackGuard {
    /// The rate every coefficient here was cut for.
    sample_rate: f32,
    /// Peak follower over the raw input.
    env: f32,
    env_release: f32,
    /// The envelope as it was at each of the last [`HISTORY`] checks.
    history: [f32; HISTORY],
    slot: usize,
    /// Samples until the next check.
    countdown: usize,
    check_every: usize,
    /// Consecutive checks that grew, and consecutive checks that did not.
    growing: u32,
    calm: u32,
    /// How many readings the history holds, so the oldest one is only compared
    /// against once it is a real reading rather than the zero it started as.
    filled: usize,
    /// Gain applied to the input right now, and where it is heading.
    gain: f32,
    target: f32,
    attack: f32,
    release: f32,
}

impl FeedbackGuard {
    pub fn new(sample_rate: f32) -> Self {
        let sr = sample_rate.max(1.0);
        Self {
            sample_rate: sr,
            env: 0.0,
            env_release: coefficient(ENV_RELEASE_S, sr),
            history: [0.0; HISTORY],
            slot: 0,
            countdown: (CHECK_S * sr) as usize,
            check_every: (CHECK_S * sr).max(1.0) as usize,
            growing: 0,
            calm: 0,
            filled: 0,
            gain: 1.0,
            target: 1.0,
            attack: coefficient(DUCK_ATTACK_S, sr),
            release: coefficient(DUCK_RELEASE_S, sr),
        }
    }

    /// Cheap enough to call every block: a rate that has not moved is a
    /// comparison and nothing else.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        let sr = sample_rate.max(1.0);
        if (sr - self.sample_rate).abs() < 0.5 {
            return;
        }
        *self = Self::new(sr);
    }

    /// How much the guard is holding the input down right now, in dB (0 when it
    /// is not holding anything).
    pub fn reduction_db(&self) -> f32 {
        if self.gain >= 0.999 {
            0.0
        } else {
            20.0 * self.gain.max(1e-6).log10()
        }
    }

    /// Whether it is doing anything at all.
    pub fn ducking(&self) -> bool {
        self.gain < 0.999
    }

    pub fn reset(&mut self) {
        self.env = 0.0;
        self.history = [0.0; HISTORY];
        self.slot = 0;
        self.countdown = self.check_every;
        self.growing = 0;
        self.calm = 0;
        self.filled = 0;
        self.gain = 1.0;
        self.target = 1.0;
    }

    /// Feed one input sample and get the gain to apply to it.
    ///
    /// The **raw** sample, before the trim: the loop out there is what is being
    /// watched, not what a knob in here has done to it.
    #[inline]
    pub fn step(&mut self, x: f32) -> f32 {
        if !armed() {
            // Let go of whatever it was holding, rather than freezing there.
            self.gain = 1.0;
            self.target = 1.0;
            return 1.0;
        }
        let level = if x.is_finite() { x.abs() } else { 0.0 };
        // Peak follower: instant up, slow down. A loop's growth is in its peaks.
        self.env = if level > self.env {
            level
        } else {
            self.env * self.env_release
        };

        if self.countdown == 0 {
            self.check();
            self.countdown = self.check_every;
        } else {
            self.countdown -= 1;
        }

        // One pole towards the target, fast down and slow up: a duck that let
        // go as fast as it arrived would pump in time with the howl it is
        // holding off.
        let coefficient = if self.target < self.gain {
            self.attack
        } else {
            self.release
        };
        self.gain = self.target + (self.gain - self.target) * coefficient;
        if !self.gain.is_finite() {
            self.gain = 1.0;
        }
        // A one-pole never quite arrives. Within a tenth of a dB of open it is
        // open, which is what lets the panel stop saying it is holding
        // something down when it no longer is.
        if self.target >= 1.0 && self.gain > 0.99 {
            self.gain = 1.0;
        }
        self.gain.clamp(DUCK * 0.5, 1.0)
    }

    /// One look at the history, once every [`CHECK_S`].
    fn check(&mut self) {
        // The oldest reading in the ring is half a second back, and that is
        // what growth is measured against — see [`GROWTH`].
        let then = self.history[self.slot];
        self.history[self.slot] = self.env;
        self.slot = (self.slot + 1) % HISTORY;
        self.filled = (self.filled + 1).min(HISTORY);

        let loud = self.env > FLOOR;
        let grew = loud && self.filled >= HISTORY && then > 0.0 && self.env > then * GROWTH;
        if grew {
            self.growing = self.growing.saturating_add(1);
            self.calm = 0;
        } else {
            self.growing = 0;
            self.calm = self.calm.saturating_add(1);
        }

        if self.growing >= GROWTH_CHECKS {
            self.target = DUCK;
            self.calm = 0;
        } else if self.target < 1.0 && (self.calm >= CALM_CHECKS || self.env < FLOOR * 0.7) {
            // It stopped climbing, or the room went quiet: let it back up.
            // Slowly, which is what `release` is for — a guard that opens the
            // instant a howl stops simply starts it again.
            self.target = 1.0;
            self.growing = 0;
        }
    }
}

/// One-pole coefficient for a time constant, at this rate.
fn coefficient(seconds: f32, sample_rate: f32) -> f32 {
    let n = seconds.max(1e-4) * sample_rate;
    (-1.0 / n).exp()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The guard's own clock is in samples, so a test is written in them too.
    const SR: f32 = 48_000.0;

    /// Arming is global to the process — that is the point of it — and
    /// `cargo test` runs these in one. One lock, held for the whole of each
    /// test, or the one that disarms does it under the others.
    fn armed_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        arm(true);
        guard
    }

    fn seconds(n: f32) -> usize {
        (n * SR) as usize
    }

    /// A signal that keeps climbing from already loud is a loop, and the guard
    /// pulls it down.
    #[test]
    fn a_growing_howl_is_pulled_down() {
        let _lock = armed_lock();
        let mut g = FeedbackGuard::new(SR);
        let mut last = 1.0;
        // A 1 kHz tone climbing 6 dB a second — about as slow as a runaway
        // gets — from something already audible. Two seconds of it, because
        // **length is what tells this from a singer**: a note stops growing
        // when it arrives, and this does not.
        for i in 0..seconds(2.5) {
            let t = i as f32 / SR;
            let phase = std::f32::consts::TAU * 1_000.0 * i as f32 / SR;
            let level = (0.06 * 2.0f32.powf(t)).min(1.0);
            last = g.step(level * phase.sin());
        }
        assert!(last < 0.5, "the guard let a howl through: gain {last}");
        assert!(g.ducking());
        assert!(g.reduction_db() < -6.0, "{} dB", g.reduction_db());
    }

    /// A held note is not a howl, however loud it is. This is the test that
    /// stops the guard from being a nuisance: an organ chord through a
    /// microphone is flat, not growing.
    #[test]
    fn a_loud_held_note_is_left_alone() {
        let _lock = armed_lock();
        let mut g = FeedbackGuard::new(SR);
        let mut last = 1.0;
        for i in 0..seconds(3.0) {
            let phase = std::f32::consts::TAU * 220.0 * i as f32 / SR;
            last = g.step(0.8 * phase.sin());
        }
        assert!((last - 1.0).abs() < 1e-3, "a held note was ducked: {last}");
        assert!(!g.ducking());
    }

    /// And a note that decays — every struck or plucked one — is not a howl
    /// either, even though it was loud a moment ago.
    #[test]
    fn a_decaying_note_is_left_alone() {
        let _lock = armed_lock();
        let mut g = FeedbackGuard::new(SR);
        let mut amp = 0.9f32;
        let mut last = 1.0;
        for i in 0..seconds(2.0) {
            let phase = std::f32::consts::TAU * 330.0 * i as f32 / SR;
            last = g.step(amp * phase.sin());
            amp *= 0.99995;
        }
        assert!(
            (last - 1.0).abs() < 1e-3,
            "a decaying note was ducked: {last}"
        );
    }

    /// Quiet material that swells is not a loop: a bowed note, a fade-in. Under
    /// the floor the guard does not look.
    #[test]
    fn something_quiet_that_swells_is_not_a_loop() {
        let _lock = armed_lock();
        let mut g = FeedbackGuard::new(SR);
        let mut last = 1.0;
        for i in 0..seconds(1.5) {
            let phase = std::f32::consts::TAU * 440.0 * i as f32 / SR;
            // 0 → -30 dBFS over the whole stretch, always under the floor.
            let amp = 0.03 * (i as f32 / seconds(1.5) as f32);
            last = g.step(amp * phase.sin());
        }
        assert!(
            (last - 1.0).abs() < 1e-3,
            "a quiet swell was ducked: {last}"
        );
    }

    /// **The one this was reported for.** A singer swelling into a long note is
    /// not a loop, and the note must survive being held: the guard used to duck
    /// an ordinary 600 ms entrance by 18 dB and then hold it there for as long
    /// as the singer kept singing, because the only thing that lifted the duck
    /// was silence.
    #[test]
    fn a_sung_note_swells_in_and_is_left_alone() {
        let _lock = armed_lock();
        for swell in [0.15f32, 0.6, 1.0] {
            let mut g = FeedbackGuard::new(SR);
            let mut worst = 1.0f32;
            for i in 0..seconds(4.0) {
                let t = i as f32 / SR;
                let phase = std::f32::consts::TAU * 220.0 * i as f32 / SR;
                // In over `swell` seconds, then held — with vibrato on it,
                // because a held human note is not a flat line.
                let amp = (t / swell).min(1.0)
                    * 0.35
                    * (1.0 + 0.25 * (t * 5.0 * std::f32::consts::TAU).sin());
                worst = worst.min(g.step(amp * phase.sin()));
            }
            assert!(
                worst > 0.999,
                "a {swell}s swell into a held note was ducked to {worst}"
            );
        }
    }

    /// And when it does fire on something that was not a loop — a long
    /// crescendo is genuinely indistinguishable from a slow one while it is
    /// still climbing — it lets go as soon as the climbing stops, without
    /// waiting for the room to fall silent.
    #[test]
    fn a_duck_lifts_when_the_growing_stops() {
        let _lock = armed_lock();
        let mut g = FeedbackGuard::new(SR);
        // Two seconds of continuous growth: long enough to be taken for a loop.
        for i in 0..seconds(2.5) {
            let t = i as f32 / SR;
            let phase = std::f32::consts::TAU * 440.0 * i as f32 / SR;
            g.step((0.06 + 0.3 * (t / 2.0).min(1.0)) * phase.sin());
        }
        assert!(g.ducking(), "it should be holding by now");

        // Now it stays loud but stops growing — which a runaway never does.
        // No silence anywhere, and it opens anyway.
        let mut last = 0.0;
        for i in 0..seconds(4.0) {
            let phase = std::f32::consts::TAU * 440.0 * i as f32 / SR;
            last = g.step(0.36 * phase.sin());
        }
        assert!(
            last > 0.8,
            "it held a signal that had stopped growing: gain {last}"
        );
    }

    /// Once the room goes quiet the guard lets go — slowly, because opening the
    /// instant a howl stops is how it starts again.
    #[test]
    fn it_lets_go_once_the_room_is_quiet() {
        let _lock = armed_lock();
        let mut g = FeedbackGuard::new(SR);
        for i in 0..seconds(2.5) {
            let t = i as f32 / SR;
            let phase = std::f32::consts::TAU * 1_000.0 * i as f32 / SR;
            let level = (0.06 * 2.0f32.powf(t)).min(1.0);
            g.step(level * phase.sin());
        }
        assert!(g.ducking(), "it should be holding by now");

        // Silence. Half a second in it is still holding most of it; after five
        // it is open again.
        for _ in 0..seconds(0.5) {
            g.step(0.0);
        }
        assert!(g.ducking(), "it let go too fast: {}", g.reduction_db());
        for _ in 0..seconds(6.0) {
            g.step(0.0);
        }
        assert!(!g.ducking(), "it never let go: {}", g.reduction_db());
    }

    /// Disarmed, it is a multiplication by one and nothing else — including
    /// while it was holding something down.
    #[test]
    fn disarmed_it_does_nothing_at_all() {
        let _lock = armed_lock();
        let mut g = FeedbackGuard::new(SR);
        for i in 0..seconds(2.5) {
            let t = i as f32 / SR;
            let phase = std::f32::consts::TAU * 1_000.0 * i as f32 / SR;
            let level = (0.06 * 2.0f32.powf(t)).min(1.0);
            g.step(level * phase.sin());
        }
        assert!(g.ducking());
        arm(false);
        let gain = g.step(0.5);
        assert_eq!(gain, 1.0, "disarmed and still holding");
        assert!(!g.ducking());
        arm(true);
    }
}
