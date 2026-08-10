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

/// Rate the detector works at. Everything above 8 kHz is harmonics.
const WORK_RATE: f32 = 16_000.0;

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
    /// Ring of recent decimated samples: the window the detector works on.
    window: Vec<f32>,
    write: usize,
    filled: bool,
    /// New samples since the last analysis; one lands every `HOP`.
    since: usize,
    /// Scratch for the difference function, sized once.
    diff: Vec<f32>,
    /// The note currently sounding, if any.
    sounding: Option<u8>,
    /// How far the heard pitch is from the note being played, in cents. Never
    /// sent anywhere — it is what the interface draws so the player can see the
    /// tracker is locked on and not merely close.
    cents: i32,
    /// A note seen but not yet believed: `(note, how many analyses in a row)`.
    candidate: Option<(u8, u32)>,
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

/// How clean the reading must be to start a note, and to replace one.
const ONSET_CLARITY: f32 = 0.85;
const CHANGE_CLARITY: f32 = 0.90;

/// Default gate, -55 dBFS. A headset microphone through a laptop's preamp is
/// far quieter than a guitar through a DI, and a gate above the signal reads
/// as "it does nothing at all" — which is the worse failure. `SENS` on the
/// mixer strip is there to put it back up where a noisy pickup needs it.
pub const DEFAULT_GATE: f32 = 0.0018;

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
            window: vec![0.0; WINDOW],
            write: 0,
            filled: false,
            since: 0,
            diff: vec![0.0; WINDOW / 2],
            sounding: None,
            cents: 0,
            candidate: None,
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
            self.acc += frame[0];
            self.acc_n += 1;
            if self.acc_n < self.decim {
                continue;
            }
            self.window[self.write] = self.acc / self.acc_n as f32;
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
                    return ([Some(PitchEvent::Off { note }), None], 1);
                }
            }
            return (NONE, 0);
        }
        self.quiet = 0;

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
        // Louder signal, harder note. The curve is gentle: a pickup's dynamic
        // range is nothing like a keyboard's velocity scale.
        let velocity = ((rms.sqrt() * 3.0).min(1.0) * 110.0) as u8 + 17;
        self.cents = ((exact - note as f32) * 100.0).round() as i32;
        // The off goes first: the other order lets the old note's release cut
        // the new one short.
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
        let (period, clarity) = yin(
            &self.window,
            self.write,
            half,
            min_lag,
            max_lag,
            &mut self.diff,
        )?;
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
    let mut running = 0.0f64;
    diff[0] = 1.0;
    for (lag, slot) in diff.iter_mut().enumerate().take(max_lag + 1).skip(1) {
        let mut sum = 0.0f64;
        for i in 0..half {
            let d = at(i) as f64 - at(i + lag) as f64;
            sum += d * d;
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
