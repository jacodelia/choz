//! Audio in, notes out: playing a plugin with a guitar.
//!
//! A rack tab fed by a capture channel normally passes that audio through its
//! FX. With `A→M` on, the audio is listened to instead: its pitch becomes
//! note-ons for the tab's instrument, so a guitar (or a voice, or a bass) can
//! drive Surge XT like a keyboard would.
//!
//! **Monophonic, and that is not a shortcut to be fixed later** — it is what
//! pitch tracking can do honestly. One period is one frequency; a chord has
//! several and picking one of them is a guess. Guitar synths have worked this
//! way (one string, one converter) for forty years.
//!
//! ## What it costs, which is the whole design
//!
//! YIN is O(window × lags), and the first version ran it **at the device's
//! sample rate on every block**: 872 lags over 2048 samples, 187 times a
//! second, on the audio thread. That is ~340 million operations a second for
//! one guitar, and the result was not "a bit late" — it was xruns, a starved
//! plugin, and notes that looked random because the callback was missing its
//! deadline.
//!
//! Two things fix it, and both are about doing less:
//!
//! * **Decimate to ~16 kHz.** A guitar's top note is 1.3 kHz; nothing above
//!   8 kHz tells you anything about its period. Averaging `D` samples into one
//!   is both the downsample and the anti-alias filter.
//! * **Analyse on a hop, not on a block.** A note cannot start twice in 8 ms,
//!   so that is how often the window is looked at, whatever the block size.
//!
//! Together: ~150k operations every 8 ms, some 30× less work, and the callback
//! keeps its deadline.
//!
//! ## `ftom`, rounded — one exact note
//!
//! The conversion is Csound's `ftom(ifreq, irnd)` with **`irnd` non-zero**:
//! *"if non-zero the result is rounded to the nearest integer"*. The input is
//! one mono jack and the output is one MIDI note — a keyboard, not a
//! controller ribbon. [`freq_to_note_exact`] is `ftom` with the default
//! `irnd = 0` (the fractional note) and [`freq_to_note`] is the rounded one
//! that actually gets played.
//!
//! Rounding on its own is not enough, and this is the part that has to be got
//! right: a pitch sitting on a semitone boundary rounds up and down as it
//! wobbles, and each flip would be a note-on. So a note only changes when the
//! new one is **clearly** the nearest ([`HYSTERESIS`] past the halfway point)
//! *and* has held for [`STEADY_ANALYSES`] readings. Vibrato inside a semitone
//! is one note held, which is what a keyboard would have sent.

/// What the tracker decided about one block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PitchEvent {
    /// A new note started (or the pitch moved far enough to be a new one).
    On { note: u8, velocity: u8 },
    /// The signal fell below the gate: whatever was sounding stops.
    Off { note: u8 },
}

/// How far past the halfway point a new note must be before it replaces the
/// one sounding, in semitones.
///
/// Without it a pitch resting on a boundary — which is where vibrato and a
/// slightly out-of-tune string both live — rounds up and down forever, and
/// every flip is a note-on nobody played. A fifth of a semitone is 20 cents:
/// wider than a singer's wobble, narrower than any note anyone means.
pub const HYSTERESIS: f32 = 0.2;

/// Lowest and highest note the tracker will report.
///
/// A guitar's low E is 82 Hz (E2, note 40) and the 24th fret of its high E is
/// 1319 Hz (E6, **note 88** — 659 Hz is E5, note 76, which is worth writing down
/// because getting it wrong reads as a detector bug). Lower needs a longer window
/// than a block has; higher is where autocorrelation starts finding harmonics.
pub const MIN_NOTE: u8 = 33; // A1, 55 Hz — a bass's low string
pub const MAX_NOTE: u8 = 96; // C7

/// Where the input is high-passed before it is measured, in Hz.
///
/// Just under [`MIN_NOTE`] (55 Hz). **A microphone in a room is not a signal
/// generator**: under the lowest note anyone plays there is always a desk, a
/// fan, a preamp, feet — and a period detector handed a 40 Hz rumble finds the
/// rumble's period, which is a note an octave and a half below what was
/// played. Measured on a 220 Hz tone with a rumble 5 dB louder than it: without
/// this the tracker reports **nothing at all**, because the mixture is not
/// periodic at either period.
///
/// Two sections, so it is 24 dB per octave rather than 12: the rumble is often
/// louder than the note, and a gentle slope leaves it louder still.
const RUMBLE_HZ: f32 = 60.0;

/// Rate the detector works at. Everything above 8 kHz is harmonics.
const WORK_RATE: f32 = 16_000.0;

/// Where the input is low-passed **before** it is decimated, in Hz.
///
/// Comfortably above [`MAX_NOTE`] (2093 Hz) and comfortably below the working
/// rate's Nyquist (8 kHz), so it takes nothing from the notes and everything
/// from what would fold on top of them.
///
/// Averaging `decim` samples — which is all the decimation used to be — is a
/// box filter, and a box filter leaks. A voice carries far more energy above
/// 8 kHz than a string does (sibilance, breath, a room's hiss), and all of it
/// came back down on top of the note: the detector then finds *a* period, just
/// not the one that was sung. Measured on a 220 Hz tone with a 9.5 kHz hiss
/// over it: without this the tracker never settles on the note at all.
const ANTIALIAS_HZ: f32 = 3_500.0;

/// Decimated samples the window holds: 64 ms at 16 kHz, three and a half
/// periods of the lowest note the tracker will report.
const WINDOW: usize = 1024;

/// How many new decimated samples between analyses — 8 ms at 16 kHz. Shorter
/// buys nothing (a note cannot start twice inside it) and costs the callback.
const HOP: usize = 128;

/// A monophonic pitch-to-MIDI converter.
pub struct PitchTracker {
    sample_rate: u32,
    /// Samples averaged into one: the downsample and its anti-alias filter.
    decim: usize,
    /// Rate the ring actually holds, after decimation.
    work_rate: f32,
    /// Partial average being accumulated for the next decimated sample.
    acc: f32,
    acc_n: usize,
    /// Takes what is above the notes off before decimating, so it cannot fold
    /// back down on top of them. Runs at the **input** rate.
    antialias: [crate::fx::utility::Biquad; 2],
    /// Takes the room out from under the signal, after decimating. The other
    /// end of the same idea.
    rumble: [crate::fx::utility::Biquad; 2],
    /// Ring of recent decimated samples: the window the detector works on.
    window: Vec<f32>,
    write: usize,
    filled: bool,
    /// New samples since the last analysis; one lands every `HOP`.
    since: usize,
    /// Scratch for the difference function, sized once.
    diff: Vec<f32>,
    /// The ring, straightened out. Copying 1024 floats once an analysis buys a
    /// difference-function loop with no wrap in it — see `yin`.
    linear: Vec<f32>,
    /// The note currently sounding, if any.
    sounding: Option<u8>,
    /// How far the heard pitch is from the note being played, in cents. Never
    /// sent anywhere — it is what the interface draws so the player can see the
    /// tracker is locked on and not merely close.
    cents: i32,
    /// A note seen but not yet believed: `(note, how many analyses in a row)`.
    candidate: Option<(u8, u32)>,
    /// Analyses since the note now sounding started. What stops the output
    /// from moving faster than a note lasts.
    held_for: u32,
    /// Analyses the signal has been under the gate; a few in a row end the note.
    quiet: u32,
    /// How loud the last analysed window was, for velocity.
    level: f32,
    /// Below this RMS there is no note. Adjustable: a single-coil through an
    /// amp has a noise floor a synthetic test tone does not.
    pub gate: f32,
}

/// How many analyses in a row must agree before a pitch becomes a note.
///
/// At an 8 ms hop that is 24 ms of agreement — under a guitarist's own attack,
/// and enough to sit out the slide between two notes.
const STEADY_ANALYSES: u32 = 3;

/// How long a note must have been sounding before anything is allowed to
/// replace it, in analyses. At an 8 ms hop, ~130 ms.
///
/// This is the difference between a converter and a controller ribbon. The
/// checks above decide *whether a reading is a note*; this one decides **how
/// often the output is allowed to change at all**, which is the thing a player
/// actually hears: a synth re-triggered every 30 ms is a buzz whatever the
/// notes were, and no amount of per-reading cleverness fixes that.
///
/// It costs exactly what it says: a real run of notes faster than this comes
/// out as fewer notes. That is the trade a monophonic converter is for — one
/// voice, held — and it is why this is a floor and not a knob: a player who
/// wants every semitone of a fast run wants a keyboard.
// ponytail: one constant. A setting the day someone wants the trade moved,
// not before — and then it belongs next to `SENS`, which is the other one.
const MIN_NOTE_ANALYSES: u32 = 16;

/// The narrowest velocity range, in dB. Only reached if the gate is set
/// absurdly high; below this the scale would be a switch rather than a range.
const MIN_VELOCITY_RANGE_DB: f32 = 24.0;

/// How clean the reading must be to start a note, and to replace one.
const ONSET_CLARITY: f32 = 0.85;
const CHANGE_CLARITY: f32 = 0.90;

/// Default gate, -61 dBFS.
///
/// Lower than it looks it should be, on purpose: the level this is compared
/// against is measured **after** the anti-alias and rumble filters, and those
/// take real energy out of a voice — a signal that reads -50 dBFS on the input
/// meter is quieter than that by the time the detector sees it. A gate above
/// the signal reads as "it does nothing at all", which is the worse failure of
/// the two. `SENS` on the mixer strip goes from -70 to -20 dBFS, so a noisy
/// pickup can put it back where it needs to be.
pub const DEFAULT_GATE: f32 = 0.0009;

impl PitchTracker {
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate.max(1);
        let decim = ((sr as f32 / WORK_RATE).round() as usize).max(1);
        Self {
            sample_rate: sr,
            decim,
            work_rate: sr as f32 / decim as f32,
            acc: 0.0,
            acc_n: 0,
            antialias: [crate::fx::utility::Biquad::lowpass(ANTIALIAS_HZ, sr as f32, 0.707); 2],
            rumble: [crate::fx::utility::Biquad::highpass(
                RUMBLE_HZ,
                sr as f32 / decim as f32,
                0.707,
            ); 2],
            window: vec![0.0; WINDOW],
            write: 0,
            filled: false,
            since: 0,
            diff: vec![0.0; WINDOW / 2],
            linear: vec![0.0; WINDOW],
            sounding: None,
            cents: 0,
            candidate: None,
            held_for: 0,
            quiet: 0,
            level: 0.0,
            gate: DEFAULT_GATE,
        }
    }

    /// The note the tracker believes is sounding.
    pub fn sounding(&self) -> Option<u8> {
        self.sounding
    }

    /// How loud the last window was (0..1), for the meter.
    pub fn level(&self) -> f32 {
        self.level
    }

    /// Stop whatever is sounding, e.g. when the feature is switched off.
    pub fn release(&mut self) -> Option<PitchEvent> {
        self.candidate = None;
        self.held_for = 0;
        self.cents = 0;
        self.sounding.take().map(|note| PitchEvent::Off { note })
    }

    /// How far off the note the heard pitch is, in cents — for the display, not
    /// for the synth.
    pub fn cents(&self) -> i32 {
        self.cents
    }

    /// Feed one interleaved stereo block and get back what changed.
    ///
    /// Returns at most two events, and in the order they must be sent: a note
    /// that replaces another sends the off first, or the new note is cut by the
    /// old one's release.
    pub fn process(&mut self, buf: &[f32], sample_rate: u32) -> ([Option<PitchEvent>; 2], usize) {
        if sample_rate != self.sample_rate {
            let gate = self.gate;
            *self = Self::new(sample_rate);
            self.gate = gate;
        }
        // **The input is one jack.** A tab fed by a single channel has the same
        // signal on both sides, and one fed by two different ones has two
        // different microphones — summing those is phase cancellation and two
        // pitches at once, neither of which is a note. So: the left side, which
        // is the channel the user assigned first.
        for frame in buf.chunks_exact(2) {
            // Band-limit first, average second. The other way round is what a
            // box filter is, and a box filter is why a voice folded.
            let mut x = frame[0];
            for section in self.antialias.iter_mut() {
                x = section.process(x);
            }
            self.acc += x;
            self.acc_n += 1;
            if self.acc_n < self.decim {
                continue;
            }
            let mut decimated = self.acc / self.acc_n as f32;
            for section in self.rumble.iter_mut() {
                decimated = section.process(decimated);
            }
            self.window[self.write] = decimated;
            self.acc = 0.0;
            self.acc_n = 0;
            self.write = (self.write + 1) % WINDOW;
            self.since += 1;
            if self.write == 0 {
                self.filled = true;
            }
        }
        if !self.filled || self.since < HOP {
            return ([None, None], 0);
        }
        self.since = 0;
        self.analyse()
    }

    /// One look at the window. Everything expensive lives here, and it runs
    /// once a hop rather than once a block.
    fn analyse(&mut self) -> ([Option<PitchEvent>; 2], usize) {
        const NONE: [Option<PitchEvent>; 2] = [None, None];
        let rms = rms_of(&self.window);
        self.level = rms.min(1.0);
        if rms < self.gate {
            self.quiet += 1;
            // Two quiet analyses, not one: a plucked string dips through zero
            // every period, and cutting the note there would stutter it.
            self.candidate = None;
            if self.quiet >= 2 {
                if let Some(note) = self.sounding.take() {
                    self.cents = 0;
                    self.held_for = 0;
                    return ([Some(PitchEvent::Off { note }), None], 1);
                }
            }
            return (NONE, 0);
        }
        self.quiet = 0;
        self.held_for = self.held_for.saturating_add(1);

        let Some((freq, clarity)) = self.detect() else {
            return (NONE, 0);
        };
        // While the window still holds the previous note, the dip is shallow —
        // that mixture is what produced a spurious semitone on the way from one
        // note to the next. Changing note asks for a cleaner reading than
        // starting one from silence does.
        let needed = if self.sounding.is_some() {
            CHANGE_CLARITY
        } else {
            ONSET_CLARITY
        };
        if clarity < needed {
            return (NONE, 0);
        }
        // `ftom` with `irnd = 0` — the pitch as a number — and then the rounded
        // note that is actually played, which is `irnd = 1`.
        let exact = freq_to_note_exact(freq);
        let note = exact.round().clamp(0.0, 127.0) as u8;
        if !(MIN_NOTE..=MAX_NOTE).contains(&note) {
            return (NONE, 0);
        }
        // The distance from the note being played says whether this is the same
        // note wobbling or a different one. Half a semitone is the boundary;
        // the hysteresis is how far past it the pitch has to go to count, so a
        // singer's vibrato does not flip the note back and forth.
        if let Some(playing) = self.sounding {
            let drift = (exact - playing as f32).abs();
            if drift <= 0.5 + HYSTERESIS {
                self.cents = ((exact - playing as f32) * 100.0).round() as i32;
                self.candidate = None;
                return (NONE, 0);
            }
            // A note that has only just started does not get replaced. Every
            // check before this one asks "is this reading a note?"; this one
            // asks "has the last one lasted long enough to be one?", which is
            // the question a listener is actually asking.
            if self.held_for < MIN_NOTE_ANALYSES {
                self.candidate = None;
                return (NONE, 0);
            }
        }
        // A pitch has to hold before it becomes a note. Without this, sliding
        // into a note machine-guns the synth: the window still holds the old
        // tone while the new one arrives, so the detector walks up a semitone at
        // a time and each step would be a note-on. Measured on a slide: eight
        // notes where there should be two.
        let steady = match self.candidate {
            Some((n, count)) if n == note => count + 1,
            _ => 1,
        };
        self.candidate = Some((note, steady));
        if steady < STEADY_ANALYSES {
            return (NONE, 0);
        }
        self.candidate = None;
        // Louder signal, harder note — measured **in dB above the gate**, and
        // that is the whole trick.
        //
        // The first version was `sqrt(rms) * 3`, which pins at 127 for anything
        // above -19 dBFS. Once the input trim is up far enough for the detector
        // to hear a microphone, *every* note is 127, and a piano played at 127
        // for a whole take is the saturated mush this was reported as. Reading
        // it against the gate means the softest thing that counts as a note is
        // a soft note, whatever the trim happens to be set to, and the range
        // above it is the player's dynamics rather than the preamp's.
        // The scale runs from the gate to full scale, so it needs no constant
        // of its own and it follows `SENS`: whatever counts as the softest note
        // is velocity 1, and 0 dBFS is 127.
        let db = 20.0 * rms.max(1e-9).log10();
        let gate_db = 20.0 * self.gate.max(1e-9).log10();
        let span = (-gate_db).max(MIN_VELOCITY_RANGE_DB);
        let t = ((db - gate_db) / span).clamp(0.0, 1.0);
        let velocity = (1.0 + t * 126.0) as u8;
        self.cents = ((exact - note as f32) * 100.0).round() as i32;
        // The off goes first: the other order lets the old note's release cut
        // the new one short.
        self.held_for = 0;
        let off = self
            .sounding
            .replace(note)
            .map(|old| PitchEvent::Off { note: old });
        let on = Some(PitchEvent::On { note, velocity });
        match off {
            Some(_) => ([off, on], 2),
            None => ([on, None], 1),
        }
    }

    /// The fundamental in the window, by YIN's cumulative mean normalised
    /// difference.
    ///
    /// Plain autocorrelation is not enough and the first attempt here proved it:
    /// a squared difference alone dips at *every* short lag on a smooth signal,
    /// so a guitar's low E came out as a note an octave and a half up. YIN
    /// divides each lag by the running mean of the ones before it, which flattens
    /// those and leaves the real period as the first dip under the threshold.
    fn detect(&mut self) -> Option<(f32, f32)> {
        let half = WINDOW / 2;
        let min_lag = (self.work_rate / note_to_freq(MAX_NOTE)) as usize;
        let max_lag = (self.work_rate / note_to_freq(MIN_NOTE)) as usize;
        // Oldest first, in order, so the seam cannot fall inside a period and
        // the search below never has to wrap.
        let (a, b) = self.window.split_at(self.write);
        self.linear[..b.len()].copy_from_slice(b);
        self.linear[b.len()..].copy_from_slice(a);
        let (period, clarity) = yin(&self.linear, 0, half, min_lag, max_lag, &mut self.diff)?;
        Some((self.work_rate / period, clarity))
    }
}

/// YIN's cumulative mean normalised difference, over a ring buffer.
///
/// `window` is read oldest-first from `start`, so a period never straddles the
/// seam. `half` samples are compared at each lag, `diff` is the caller's
/// scratch (no allocation here — this runs on the audio thread), and the
/// answer is `(period in samples, clarity 0..1)`.
///
/// Plain autocorrelation is not enough and the first attempt proved it: a
/// squared difference alone dips at *every* short lag on a smooth signal, so a
/// guitar's low E came out an octave and a half up. YIN divides each lag by the
/// running mean of the ones before it, which flattens those and leaves the real
/// period as the first dip under the threshold.
///
/// Shared with the AutoTune FX: one detector, two callers. They window
/// differently — one decimates to 16 kHz for note events, the other tracks a
/// voice at the device's own rate — but the arithmetic in the middle is the
/// same and only wants writing once.
pub fn yin(
    window: &[f32],
    start: usize,
    half: usize,
    min_lag: usize,
    max_lag: usize,
    diff: &mut [f32],
) -> Option<(f32, f32)> {
    let n = window.len();
    let max_lag = max_lag
        .min(half.saturating_sub(1))
        .min(diff.len().saturating_sub(1));
    let min_lag = min_lag.max(1);
    if n == 0 || half == 0 || min_lag + 2 >= max_lag {
        return None;
    }
    let at = |i: usize| window[(start + i) % n];

    // d(τ), then normalised by the running mean of everything before it.
    //
    // The inner loop runs `half` times for every lag — 150k iterations for one
    // analysis — so what it does *per iteration* is the whole cost. Reading
    // through the ring meant two `%` per sample, three hundred thousand of them
    // per analysis, inside the audio callback. A caller that hands the window
    // in order (`start == 0`) gets the loop over two plain slices instead,
    // which has no wrap in it at all and vectorises.
    let mut running = 0.0f64;
    diff[0] = 1.0;
    for (lag, slot) in diff.iter_mut().enumerate().take(max_lag + 1).skip(1) {
        let mut sum = 0.0f64;
        if start == 0 {
            // `lag + half <= n` holds: `max_lag < half` and `half <= n / 2`.
            // Accumulated in `f32` and widened once: the terms are squares of
            // differences of samples, a few hundred of them, nowhere near
            // where single precision starts losing the sum — and the widening
            // was happening twice per iteration, which is most of a loop this
            // tight. `running` stays `f64`: that one grows across every lag.
            let mut acc = 0.0f32;
            for (x, y) in window[..half].iter().zip(window[lag..lag + half].iter()) {
                let d = x - y;
                acc += d * d;
            }
            sum = acc as f64;
        } else {
            for i in 0..half {
                let d = at(i) as f64 - at(i + lag) as f64;
                sum += d * d;
            }
        }
        running += sum;
        *slot = if running > 0.0 {
            (sum * lag as f64 / running) as f32
        } else {
            1.0
        };
    }

    // The first dip under the threshold, not the deepest: the deepest is
    // usually an octave down.
    const THRESHOLD: f32 = 0.15;
    let mut best = None;
    let mut lag = min_lag + 1;
    while lag < max_lag {
        if diff[lag] < THRESHOLD {
            // Walk to the bottom of this dip rather than stopping on its edge.
            while lag + 1 < max_lag && diff[lag + 1] < diff[lag] {
                lag += 1;
            }
            best = Some(lag);
            break;
        }
        lag += 1;
    }
    let lag = best.or_else(|| {
        // Nothing convincing: the clearest dip, and only if it is a dip.
        let (i, v) = (min_lag..max_lag)
            .map(|l| (l, diff[l]))
            .fold(
                (0usize, f32::MAX),
                |acc, x| if x.1 < acc.1 { x } else { acc },
            );
        (v < 0.4).then_some(i)
    })?;
    if lag == 0 || lag + 1 >= max_lag {
        return None;
    }

    // Parabolic interpolation around the dip. A whole lag is a **tone** at the
    // top of the range, so this is not polish — without it the high notes land
    // on the wrong one.
    let (y0, y1, y2) = (diff[lag - 1], diff[lag], diff[lag + 1]);
    let denom = 2.0 * (2.0 * y1 - y0 - y2);
    let shift = if denom.abs() > 1e-9 {
        (y2 - y0) / denom
    } else {
        0.0
    };
    let period = lag as f32 + shift.clamp(-1.0, 1.0);
    // How convincing the dip is: 1 is a perfectly periodic window.
    let clarity = (1.0 - diff[lag]).clamp(0.0, 1.0);
    (period > 0.0).then_some((period, clarity))
}

fn rms_of(window: &[f32]) -> f32 {
    let sum: f64 = window.iter().map(|s| (*s as f64) * (*s as f64)).sum();
    (sum / window.len().max(1) as f64).sqrt() as f32
}

/// Csound's `ftom` with the default `irnd = 0`: the MIDI note number for a
/// frequency, **fraction and all**. A4 = 440 Hz = note 69.
pub fn freq_to_note_exact(freq: f32) -> f32 {
    if freq <= 0.0 {
        return 0.0;
    }
    69.0 + 12.0 * (freq / 440.0).log2()
}

/// Csound's `ftom` with `irnd` non-zero — *"the result is rounded to the
/// nearest integer"*. This is the note that gets played: one jack in, one note
/// out.
pub fn freq_to_note(freq: f32) -> u8 {
    if freq <= 0.0 {
        return 0;
    }
    let n = 69.0 + 12.0 * (freq / 440.0).log2();
    n.round().clamp(0.0, 127.0) as u8
}

pub fn note_to_freq(note: u8) -> f32 {
    440.0 * 2f32.powf((note as f32 - 69.0) / 12.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tone that keeps its phase across blocks.
    ///
    /// The first version of this restarted at zero every block, which put a
    /// **256-sample discontinuity** into the signal — and the tracker duly
    /// reported the note that period belongs to. The detector was right and the
    /// test was wrong, which is worth remembering before blaming the detector.
    struct Tone {
        phase: f32,
    }

    impl Tone {
        fn new() -> Self {
            Self { phase: 0.0 }
        }

        fn block(&mut self, hz: f32, sr: u32, frames: usize, amp: f32) -> Vec<f32> {
            self.harmonics(&[(1.0, 1.0)], hz, sr, frames, amp)
        }

        /// A tone with harmonics, which is what a string actually makes:
        /// `(multiple, relative amplitude)`.
        fn harmonics(
            &mut self,
            parts: &[(f32, f32)],
            hz: f32,
            sr: u32,
            frames: usize,
            amp: f32,
        ) -> Vec<f32> {
            let step = 2.0 * std::f32::consts::PI * hz / sr as f32;
            (0..frames)
                .flat_map(|_| {
                    let s: f32 = parts
                        .iter()
                        .map(|(m, a)| a * (self.phase * m).sin())
                        .sum::<f32>()
                        * amp;
                    self.phase = (self.phase + step) % (2.0 * std::f32::consts::PI);
                    [s, s]
                })
                .collect()
        }
    }

    /// Collect every event a run of blocks produces.
    fn drain(
        t: &mut PitchTracker,
        tone: &mut Tone,
        hz: f32,
        sr: u32,
        blocks: usize,
        amp: f32,
    ) -> Vec<PitchEvent> {
        let mut out = Vec::new();
        for _ in 0..blocks {
            let (ev, n) = t.process(&tone.block(hz, sr, 256, amp), sr);
            out.extend(ev.iter().take(n).flatten().copied());
        }
        out
    }

    /// `ftom` both ways: the fractional note Csound gives with `irnd = 0`, and
    /// the rounded one it gives with `irnd` non-zero. What reaches the plugin
    /// is always the rounded one — one jack in, one note out.
    #[test]
    fn ftom_is_the_conversion_csound_documents() {
        assert!(
            (freq_to_note_exact(440.0) - 69.0).abs() < 1e-4,
            "A4 = 440 Hz = note 69"
        );
        assert!(
            (freq_to_note_exact(880.0) - 81.0).abs() < 1e-4,
            "an octave is twelve"
        );
        // A quarter tone up is half a semitone: `irnd = 0` says so, and
        // `irnd = 1` rounds it to a real note.
        let quarter_up = 440.0 * 2f32.powf(0.5 / 12.0);
        assert!((freq_to_note_exact(quarter_up) - 69.5).abs() < 1e-3);
        assert_eq!(
            freq_to_note(quarter_up),
            70,
            "rounded to the nearest integer"
        );
        assert_eq!(
            freq_to_note(440.0 * 2f32.powf(0.49 / 12.0)),
            69,
            "and down when nearer"
        );
        assert_eq!(freq_to_note_exact(0.0), 0.0, "silence is not a note");
    }

    /// Two notes traded faster than a note lasts. A converter that follows
    /// every one re-triggers the synth twenty times a second, and that is a
    /// buzz whatever the notes were. One voice, held, is what has to come out.
    #[test]
    fn the_output_cannot_change_faster_than_a_note_lasts() {
        let sr = 48_000;
        let mut t = PitchTracker::new(sr);
        let mut tone = Tone::new();
        let mut ons = 0usize;
        // A4 and C5 swapped every ~85 ms, each held long enough to be heard as
        // a note on its own.
        for i in 0..24 {
            let hz = if i % 2 == 0 { 440.0 } else { 523.25 };
            for e in drain(&mut t, &mut tone, hz, sr, 16, 0.4) {
                if matches!(e, PitchEvent::On { .. }) {
                    ons += 1;
                }
            }
        }
        // Following every swap would be 24 note-ons over ~2 s. The floor on how
        // long a note must last before anything replaces it cuts that down.
        assert!(
            ons <= 14,
            "the output re-triggered faster than a note lasts: {ons} note-ons"
        );
    }

    /// A microphone in a room is not a signal generator. Under the lowest note
    /// the tracker will report there is always something — a desk, a fan, a
    /// preamp, feet — and a period detector handed a 40 Hz rumble finds the
    /// rumble's period, which is a note an octave and a half below what was
    /// played. The input has to be cleaned before it is measured.
    #[test]
    fn rumble_under_the_lowest_note_does_not_become_the_note() {
        let sr = 48_000;
        let mut t = PitchTracker::new(sr);
        let mut tone = Tone::new();
        let mut rumble = Tone::new();
        let mut ons: Vec<u8> = Vec::new();
        for _ in 0..90 {
            // A3 played into a mic that is also hearing the room.
            let note = tone.block(220.0, sr, 256, 0.25);
            let low = rumble.block(41.0, sr, 256, 0.45);
            let mixed: Vec<f32> = note.iter().zip(low.iter()).map(|(a, b)| a + b).collect();
            let (ev, n) = t.process(&mixed, sr);
            for e in ev.iter().take(n).flatten() {
                if let PitchEvent::On { note, .. } = e {
                    ons.push(*note);
                }
            }
        }
        assert_eq!(
            t.sounding(),
            Some(57),
            "A3 is what was played; the rumble is not a note. Reported: {ons:?}"
        );
    }

    /// **No microtonality.** A quarter tone is not a note, and a converter that
    /// tried to be exact about it would have to send pitch bend — which is a
    /// second stream of MIDI moving all the time, the opposite of what a note
    /// is for. What comes out is a whole note; how far off it the singer was
    /// stays in the display, where it costs nobody anything.
    #[test]
    fn a_quarter_tone_still_comes_out_as_one_whole_note() {
        let sr = 48_000;
        let mut t = PitchTracker::new(sr);
        let mut tone = Tone::new();
        // A4 plus 50 cents: exactly between two notes.
        let hz = 440.0 * 2f32.powf(0.5 / 12.0);
        let events = drain(&mut t, &mut tone, hz, sr, 90, 0.4);
        let ons: Vec<u8> = events
            .iter()
            .filter_map(|e| match e {
                PitchEvent::On { note, .. } => Some(*note),
                _ => None,
            })
            .collect();
        assert_eq!(ons.len(), 1, "one note, not a hunt between two: {ons:?}");
        assert!(
            matches!(t.sounding(), Some(69) | Some(70)),
            "and it is a whole note: {:?}",
            t.sounding()
        );
        // The fraction is reported, never sent: `PitchEvent` has no variant
        // that carries one, which is the strongest way to promise it.
        assert!(t.cents().abs() <= 50, "the display keeps the fraction");
    }

    /// A voice is not a guitar: it carries a lot of energy above the band a
    /// note lives in — sibilance, breath, a room's hiss — and the decimation
    /// that takes 48 kHz down to 16 folds all of it back down on top of the
    /// note. A period detector handed that finds *a* period, but not the one
    /// that was sung, which is exactly "it turns the noise into a frequency
    /// and not into a note".
    #[test]
    fn sibilance_does_not_fold_down_on_top_of_the_note() {
        let sr = 48_000;
        let mut t = PitchTracker::new(sr);
        let mut tone = Tone::new();
        let mut hiss = Tone::new();
        let mut ons: Vec<u8> = Vec::new();
        for _ in 0..90 {
            // A3 sung, with the "s" of a consonant on top of it: energy above
            // the working rate's Nyquist, which is where folding happens.
            let note = tone.block(220.0, sr, 256, 0.3);
            let sibilance = hiss.block(9_500.0, sr, 256, 0.35);
            let mixed: Vec<f32> = note
                .iter()
                .zip(sibilance.iter())
                .map(|(a, b)| a + b)
                .collect();
            let (ev, n) = t.process(&mixed, sr);
            for e in ev.iter().take(n).flatten() {
                if let PitchEvent::On { note, .. } = e {
                    ons.push(*note);
                }
            }
        }
        assert_eq!(
            t.sounding(),
            Some(57),
            "A3 was sung; the sibilance is not a note. Reported: {ons:?}"
        );
    }

    /// Velocity is the player's dynamics, not the preamp's setting. Read from
    /// full scale it pins at 127 for anything above -19 dBFS — and once the
    /// trim is up far enough to hear a microphone, that is every note. A piano
    /// played at 127 for a whole take is not a piano.
    #[test]
    fn velocity_is_read_from_the_gate_not_from_full_scale() {
        let sr = 48_000;
        let vel_of = |amp: f32| -> u8 {
            let mut t = PitchTracker::new(sr);
            let mut tone = Tone::new();
            drain(&mut t, &mut tone, 220.0, sr, 90, amp)
                .into_iter()
                .find_map(|e| match e {
                    PitchEvent::On { velocity, .. } => Some(velocity),
                    _ => None,
                })
                .expect("a note")
        };
        let soft = vel_of(0.02);
        let normal = vel_of(0.2);
        let loud = vel_of(0.9);
        assert!(soft < normal && normal < loud, "{soft} < {normal} < {loud}");
        assert!(
            normal < 120,
            "a normal signal must leave headroom above it: {normal}"
        );
        assert!(soft < 60, "and a quiet one has to be quiet: {soft}");
    }

    /// The output is a keyboard: **one note, held**. A singer's vibrato inside
    /// a semitone must not flip it back and forth — every flip would be a
    /// note-on nobody played, which is what makes a tracker unusable.
    #[test]
    fn a_wobbling_pitch_is_one_note_held_not_a_run_of_them() {
        let sr = 48_000;
        let mut t = PitchTracker::new(sr);
        let mut tone = Tone::new();
        drain(&mut t, &mut tone, 196.0, sr, 60, 0.5);
        assert_eq!(t.sounding(), Some(55), "open G to start with");

        // ±35 cents of wobble, straddling nothing but sitting well inside the
        // note. Not one event should come out of it.
        let mut events = Vec::new();
        for cents in [20.0, 35.0, 10.0, -25.0, -35.0, 0.0f32] {
            let hz = 196.0 * 2f32.powf(cents / 1200.0);
            events.extend(drain(&mut t, &mut tone, hz, sr, 12, 0.5));
        }
        assert!(events.is_empty(), "one note held, not {events:?}");
        assert_eq!(t.sounding(), Some(55));
        // The display still follows the pitch, even though the synth does not.
        assert!(
            t.cents().abs() < 20,
            "the reading tracks it: {} cents",
            t.cents()
        );

        // A real semitone up **is** a new note.
        let events = drain(&mut t, &mut tone, 207.65, sr, 60, 0.5);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, PitchEvent::On { note: 56, .. })),
            "a semitone up is a new note: {events:?}"
        );
        assert_eq!(t.sounding(), Some(56));
    }

    /// The input is one jack. Two different channels are two different
    /// microphones, and summing them is phase cancellation plus two pitches at
    /// once — so the tracker listens to the side the user assigned first.
    #[test]
    fn the_input_is_mono_and_the_second_channel_is_ignored() {
        let sr = 48_000;
        let mut t = PitchTracker::new(sr);
        // Left is an open G; right is a fifth above it, and inverted so a sum
        // would both cancel and confuse.
        let (mut l, mut r) = (0.0f32, 0.0f32);
        let mut got = None;
        for _ in 0..60 {
            let buf: Vec<f32> = (0..256)
                .flat_map(|_| {
                    let a = 0.5 * l.sin();
                    let b = -0.5 * r.sin();
                    l = (l + 2.0 * std::f32::consts::PI * 196.0 / sr as f32)
                        % std::f32::consts::TAU;
                    r = (r + 2.0 * std::f32::consts::PI * 294.0 / sr as f32)
                        % std::f32::consts::TAU;
                    [a, b]
                })
                .collect();
            let (ev, n) = t.process(&buf, sr);
            for e in ev.iter().take(n).flatten() {
                if let PitchEvent::On { note, .. } = e {
                    got = Some(*note);
                }
            }
        }
        assert_eq!(got, Some(55), "the left jack is the input, and it is a G");
    }

    #[test]
    fn a_note_and_a_frequency_are_the_same_thing() {
        assert_eq!(freq_to_note(440.0), 69, "A4");
        assert_eq!(freq_to_note(82.41), 40, "a guitar's low E");
        assert_eq!(freq_to_note(261.63), 60, "middle C");
        assert!((note_to_freq(69) - 440.0).abs() < 0.01);
        assert_eq!(freq_to_note(0.0), 0, "silence is not a note");
    }

    /// The whole point: a steady tone in, the right note out.
    #[test]
    fn a_guitar_string_comes_out_as_its_note() {
        let sr = 48_000;
        // Low E of a guitar, A of a bass, open G, concert A, high E, and two
        // octaves above that — the range this is for.
        for (hz, want) in [
            (82.41, 40u8),
            (110.0, 45),
            (196.0, 55),
            (440.0, 69),
            (659.26, 76),
            (1318.5, 88),
        ] {
            let mut t = PitchTracker::new(sr);
            let mut tone = Tone::new();
            let mut got = None;
            // Enough blocks to fill the window and clear the steady count.
            for _ in 0..60 {
                let (events, n) = t.process(&tone.block(hz, sr, 256, 0.5), sr);
                for e in events.iter().take(n).flatten() {
                    if let PitchEvent::On { note, .. } = e {
                        got = Some(*note);
                    }
                }
            }
            assert_eq!(got, Some(want), "{hz} Hz should read as note {want}");
        }
    }

    /// A plucked string is not a sine: the harmonics are often louder than the
    /// fundamental, which is exactly where a naive detector reports the octave.
    #[test]
    fn a_string_with_harmonics_still_reads_as_its_fundamental() {
        let sr = 48_000;
        // Fundamental quieter than the second and third partials, as a bridge
        // pickup hears it.
        let parts = [(1.0, 0.4), (2.0, 1.0), (3.0, 0.7), (4.0, 0.3)];
        for (hz, want) in [(82.41, 40u8), (196.0, 55), (440.0, 69)] {
            let mut t = PitchTracker::new(sr);
            let mut tone = Tone::new();
            let mut got = None;
            for _ in 0..60 {
                let (events, n) = t.process(&tone.harmonics(&parts, hz, sr, 256, 0.25), sr);
                for e in events.iter().take(n).flatten() {
                    if let PitchEvent::On { note, .. } = e {
                        got = Some(*note);
                    }
                }
            }
            assert_eq!(
                got,
                Some(want),
                "{hz} Hz with harmonics should still be note {want}"
            );
        }
    }

    /// Silence ends the note, and it takes more than one quiet analysis — a
    /// plucked string crosses zero every period.
    #[test]
    fn silence_releases_the_note_but_not_on_the_first_dip() {
        let sr = 48_000;
        let mut t = PitchTracker::new(sr);
        let mut tone = Tone::new();
        for _ in 0..60 {
            t.process(&tone.block(196.0, sr, 256, 0.5), sr);
        }
        assert_eq!(t.sounding(), Some(55));

        // The gate looks at the whole window, so the release lags by up to one
        // window's worth of silence — a fifteenth of a second, and the price of
        // not stuttering on every dip of the waveform.
        let quiet = vec![0.0f32; 512];
        let (events, n) = t.process(&quiet, sr);
        assert_eq!(n, 0, "one quiet block is nowhere near the end of a note");
        assert!(events.iter().all(|e| e.is_none()));

        let mut off = None;
        for _ in 0..80 {
            let (events, n) = t.process(&quiet, sr);
            if n > 0 {
                off = events[0];
                break;
            }
        }
        assert_eq!(off, Some(PitchEvent::Off { note: 55 }));
        assert_eq!(t.sounding(), None);
    }

    /// A new pitch replaces the old one, and the off comes first — the other
    /// order leaves the new note cut short by the old note's release.
    #[test]
    fn a_new_pitch_stops_the_old_note_before_starting_its_own() {
        let sr = 48_000;
        let mut t = PitchTracker::new(sr);
        let mut tone = Tone::new();
        for _ in 0..60 {
            t.process(&tone.block(196.0, sr, 256, 0.5), sr);
        }
        assert_eq!(t.sounding(), Some(55));

        let mut seen = Vec::new();
        for _ in 0..80 {
            let (events, n) = t.process(&tone.block(246.94, sr, 256, 0.5), sr);
            for e in events.iter().take(n).flatten() {
                seen.push(*e);
            }
        }
        assert_eq!(
            seen.first().copied(),
            Some(PitchEvent::Off { note: 55 }),
            "the old note goes first: {seen:?}"
        );
        assert!(
            matches!(seen.get(1), Some(PitchEvent::On { note: 59, .. })),
            "then the new one: {seen:?}"
        );
        // And **only** those two: the slide between them is not a run of notes.
        assert_eq!(
            seen.len(),
            2,
            "one note change, not a semitone chain: {seen:?}"
        );
    }

    /// Under the gate there is no note at all, however clean the signal is —
    /// and the gate moves, because a pickup's noise floor is not a test tone's.
    #[test]
    fn a_signal_below_the_gate_plays_nothing() {
        let sr = 48_000;
        let mut t = PitchTracker::new(sr);
        let mut tone = Tone::new();
        for _ in 0..60 {
            let (events, n) = t.process(&tone.block(196.0, sr, 256, 0.0005), sr);
            assert_eq!(n, 0, "{events:?}");
        }
        assert_eq!(t.sounding(), None);

        // Raise the gate and a signal that *was* loud enough no longer is.
        let mut t = PitchTracker::new(sr);
        t.gate = 0.5;
        let mut tone = Tone::new();
        for _ in 0..60 {
            t.process(&tone.block(196.0, sr, 256, 0.2), sr);
        }
        assert_eq!(t.sounding(), None, "a gate above the signal keeps it quiet");
    }

    /// The reason for the rewrite: the work per second has to fit in the audio
    /// callback. One analysis per hop, on a decimated window — not one full-rate
    /// correlation per block, which is what starved the plugin.
    #[test]
    fn the_detector_runs_once_a_hop_not_once_a_block() {
        let sr = 48_000;
        let t = PitchTracker::new(sr);
        assert_eq!(t.decim, 3, "48 kHz decimates by 3 to work at 16 kHz");
        assert!((t.work_rate - 16_000.0).abs() < 1.0);

        // A 256-frame block is 85 decimated samples, so an analysis lands
        // roughly every other block rather than on every one.
        let blocks_per_analysis = HOP as f32 / (256.0 / t.decim as f32);
        assert!(
            blocks_per_analysis > 1.0,
            "{blocks_per_analysis} blocks between analyses"
        );

        // And the correlation itself is bounded by the window, not the device.
        let max_lag = (t.work_rate / note_to_freq(MIN_NOTE)) as usize;
        assert!(
            max_lag * (WINDOW / 2) < 200_000,
            "{max_lag} lags is the whole budget"
        );
    }
}

#[cfg(test)]
mod bench {
    use super::*;

    /// What one block of tracking costs against the time the callback has.
    ///
    /// **This is not a nicety.** The tracker runs inside the audio callback, so
    /// whatever it takes comes out of the same budget as the instrument and the
    /// whole FX chain — and a callback that misses its deadline does not glitch
    /// choz, it glitches **the graph**: every other application on the machine
    /// stutters with it. That was reported as "turning on A→M makes the browser
    /// sound bad", and it was true.
    ///
    /// Measured: 15.7 % of a 128-frame callback before the difference-function
    /// loop stopped reading through the ring (two `%` per sample, 300k of them
    /// per analysis) and stopped widening to `f64` twice per iteration. After:
    /// 1.6 %.
    ///
    /// Ignored by default because it is a clock, and a clock on a busy machine
    /// is a flaky test. Run it when the detector is touched:
    ///
    /// ```text
    /// cargo test --release -p choz-engine --lib -- --ignored --nocapture what_one_block
    /// ```
    #[test]
    #[ignore = "timing; run it deliberately, and in release"]
    fn what_one_block_of_tracking_costs() {
        let sr = 48_000u32;
        for frames in [128usize, 256, 512] {
            let mut t = PitchTracker::new(sr);
            let buf: Vec<f32> = (0..frames)
                .flat_map(|i| {
                    let s = (2.0 * std::f32::consts::PI * 220.0 * i as f32 / sr as f32).sin() * 0.4;
                    [s, s]
                })
                .collect();
            for _ in 0..40 {
                t.process(&buf, sr);
            }
            let n = 2000;
            let start = std::time::Instant::now();
            for _ in 0..n {
                t.process(&buf, sr);
            }
            let per_block = start.elapsed().as_secs_f64() / n as f64;
            let budget = frames as f64 / sr as f64;
            let share = per_block / budget * 100.0;
            eprintln!(
                "frames={frames}: {:.3} ms/block, budget {:.3} ms => {share:.1}% of the callback",
                per_block * 1000.0,
                budget * 1000.0,
            );
            // Generous, because it is a clock: what it is really guarding
            // against is a change that puts the analysis back to a third of the
            // callback, not a percentage point either way.
            if !cfg!(debug_assertions) {
                assert!(
                    share < 8.0,
                    "the tracker is eating the callback again: {share:.1}% at {frames} frames"
                );
            }
        }
    }
}
