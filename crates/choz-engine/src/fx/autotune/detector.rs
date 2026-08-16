//! F0 estimation for a monophonic voice: YIN over a ring, once per hop.
//!
//! The arithmetic is [`crate::pitch::yin`] — the same one the `A→M` button uses,
//! written once. The windowing is the same too, and for the same measured
//! reason: **YIN at the device's rate does not fit in the callback.**
//!
//! At 48 kHz with a window long enough for 60 Hz, one analysis is ~2.2 million
//! operations, and a 256-sample hop asks for 187 of them a second: 410 M
//! operations per second, for one voice. Decimating to 16 kHz cuts the window
//! and the lag range together, and the same analysis costs ~117 k. Thirty times
//! less, and nothing is lost: a guitar's top note is 1.3 kHz and a voice's is
//! lower, so there is nothing above 8 kHz that says anything about the period.
//!
//! The *frequency* keeps its precision because the dip is interpolated, and the
//! shifter is handed the period back at the full rate as `sample_rate / f0`.
//!
//! ## What a microphone in a room actually sends
//!
//! This detector shipped with the same three holes `A→M` shipped with, and they
//! were found there first — on a guitar and a voice, not on a test tone:
//!
//! * **The decimation was a box average**, i.e. its own anti-alias filter, and a
//!   box filter leaks. A voice has far more energy above 8 kHz than a synthetic
//!   tone does — sibilance, breath, room hiss — and all of it folded back down
//!   on top of the note. The detector then finds *a* period, just not the sung
//!   one, and AutoTune corrects towards a note nobody sang. Now the input is
//!   low-passed at [`ANTIALIAS_HZ`] **before** it is averaged.
//! * **Nothing took the room out from underneath.** Under the lowest note there
//!   is always a desk, a fan, a preamp, feet; handed a 40 Hz rumble a period
//!   detector finds the rumble. Now the decimated signal is high-passed at
//!   [`RUMBLE_HZ`], two sections, 24 dB per octave — the rumble is often louder
//!   than the note, and a gentle slope leaves it louder still.
//! * **Every hop's answer went straight out.** One bad window — a consonant, a
//!   door — moved the correction ratio for that block, which is heard as a
//!   warble. Now the reported frequency is the **median of the last three**
//!   analyses, which costs 16 ms of lag on a real glide and removes the
//!   single-window outlier entirely; and a window that briefly loses confidence
//!   [holds][`UNVOICED_HOLD`] the last note instead of dropping the correction
//!   to nothing, because that drop is a click.
//!
//! Filtering happens on the **analysis copy only**. What comes out of the effect
//! is the shifter's output, and the shifter still reads the untouched input.

/// What the detector decided about the signal it has heard so far.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PitchEstimate {
    pub frequency_hz: f32,
    /// How periodic the window is, 0..1. YIN's dip depth, not a probability.
    pub confidence: f32,
    /// Whether there is a pitch here at all. A consonant, a breath and silence
    /// are all unvoiced, and correcting them is how a tuner makes a robot.
    pub voiced: bool,
}

impl PitchEstimate {
    pub const SILENT: Self = PitchEstimate {
        frequency_hz: 0.0,
        confidence: 0.0,
        voiced: false,
    };
}

/// Highest sample rate the buffers are sized for. Above this the detector still
/// runs — the window is simply shorter in wall-clock terms.
pub const MAX_SAMPLE_RATE: u32 = 96_000;

/// The lowest F0 the analysis window is sized for. Everything downstream —
/// window length, shifter latency — falls out of this number.
pub const MIN_SUPPORTED_HZ: f32 = 60.0;

/// Longest period the buffers must hold, in samples.
pub const MAX_PERIOD: usize = (MAX_SAMPLE_RATE as f32 / MIN_SUPPORTED_HZ) as usize + 1;

/// Rate the detector actually works at, after decimation.
const WORK_RATE: f32 = 16_000.0;

/// Analysis window, in decimated samples: 64 ms at 16 kHz. YIN compares `half`
/// samples at lags up to one period, so it needs the longest period (60 Hz is
/// 267 samples here) to fit alongside the half it compares.
pub const WINDOW: usize = 1024;

/// Decimated samples between analyses — 8 ms. A voice cannot change note
/// inside that, and analysing more often only costs the callback.
const HOP: usize = 128;

/// Where the input is low-passed **before** it is decimated, in Hz. Above the
/// highest note anyone sings, below the working rate's Nyquist (8 kHz).
const ANTIALIAS_HZ: f32 = 3_500.0;

/// Where the decimated signal is high-passed, in Hz: just under the lowest note
/// this detector supports (60 Hz).
const RUMBLE_HZ: f32 = 55.0;

/// Analyses whose answers are kept for the median. Three is the smallest number
/// that can outvote one bad window, and the lag it costs (16 ms) is under the
/// shortest retune anybody uses.
const MEDIAN_ANALYSES: usize = 3;

/// Analyses a note is held through after the confidence drops, before the
/// detector calls the signal unvoiced.
///
/// A voice dips through low clarity at every consonant and every vibrato
/// turning point. Dropping the correction there and picking it up 8 ms later is
/// the pitch jumping to and from where it was, which is the very artefact this
/// effect exists to not make.
const UNVOICED_HOLD: u32 = 2;

pub struct PitchDetector {
    sample_rate: f32,
    /// Samples averaged into one, and the rate that leaves.
    decim: usize,
    work_rate: f32,
    /// Partial average waiting for its last few samples.
    acc: f32,
    acc_n: usize,
    /// Takes what is above the notes off before decimating, so it cannot fold
    /// back down on top of them. Runs at the **input** rate.
    antialias: [crate::fx::utility::Biquad; 2],
    /// Takes the room out from under the signal, after decimating.
    rumble: [crate::fx::utility::Biquad; 2],
    window: Vec<f32>,
    write: usize,
    filled: usize,
    since: usize,
    diff: Vec<f32>,
    /// The ring, straightened out. Copying it once an analysis buys YIN's
    /// difference loop with no wrap in it — see [`crate::pitch::yin`], which
    /// takes the fast path only when the window starts at zero.
    linear: Vec<f32>,
    /// The last few accepted frequencies, newest last. The median of these is
    /// what leaves the detector.
    recent: [f32; MEDIAN_ANALYSES],
    recent_n: usize,
    /// Analyses since the confidence last held up.
    unvoiced_for: u32,
    /// Last answer, so a block between hops still has something to say.
    last: PitchEstimate,
    /// RMS below which there is nothing to detect. A window quieter than this
    /// is unvoiced whatever YIN thinks of it.
    pub gate: f32,
    /// Search range, in Hz. Narrower is both faster and safer: half the octave
    /// errors a detector makes are outside the range the singer can reach.
    pub min_hz: f32,
    pub max_hz: f32,
    /// Confidence under which the estimate is called unvoiced.
    pub voiced_threshold: f32,
    /// Previous accepted frequency, for the octave check.
    previous: f32,
}

impl PitchDetector {
    /// Every buffer is sized here, for the worst case the parameters allow, and
    /// never again: [`Self::process`] runs on the audio thread.
    pub fn new(sample_rate: f32) -> Self {
        let sr = sample_rate.max(1.0);
        let decim = ((sr / WORK_RATE).round() as usize).max(1);
        Self {
            sample_rate: sr,
            decim,
            work_rate: sr / decim as f32,
            acc: 0.0,
            acc_n: 0,
            antialias: [crate::fx::utility::Biquad::lowpass(ANTIALIAS_HZ, sr, 0.707); 2],
            rumble: [crate::fx::utility::Biquad::highpass(
                RUMBLE_HZ,
                sr / decim as f32,
                0.707,
            ); 2],
            window: vec![0.0; WINDOW],
            write: 0,
            filled: 0,
            since: 0,
            diff: vec![0.0; WINDOW / 2],
            linear: vec![0.0; WINDOW],
            recent: [0.0; MEDIAN_ANALYSES],
            recent_n: 0,
            unvoiced_for: 0,
            last: PitchEstimate::SILENT,
            // -56 dBFS, and lower than it looks it should be **on purpose**:
            // the level this is compared against is measured after the
            // anti-alias and rumble filters, and those take real energy out of
            // a voice. A gate above the signal reads as "the effect does
            // nothing", which is the worse of the two failures — the same
            // lesson `A→M` learned at -61 dBFS.
            gate: 0.0016,
            min_hz: 70.0,
            max_hz: 1200.0,
            voiced_threshold: 0.55,
            previous: 0.0,
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        let sr = sample_rate.max(1.0);
        if (sr - self.sample_rate).abs() < 0.5 {
            return;
        }
        self.sample_rate = sr;
        self.decim = ((sr / WORK_RATE).round() as usize).max(1);
        self.work_rate = sr / self.decim as f32;
        // Both filters are cut for a rate; a new rate is new coefficients, not
        // just a flushed state.
        self.antialias = [crate::fx::utility::Biquad::lowpass(ANTIALIAS_HZ, sr, 0.707); 2];
        self.rumble = [crate::fx::utility::Biquad::highpass(RUMBLE_HZ, self.work_rate, 0.707); 2];
        self.reset();
    }

    pub fn reset(&mut self) {
        self.acc = 0.0;
        self.acc_n = 0;
        self.window.fill(0.0);
        self.write = 0;
        self.filled = 0;
        self.since = 0;
        self.recent = [0.0; MEDIAN_ANALYSES];
        self.recent_n = 0;
        self.unvoiced_for = 0;
        self.last = PitchEstimate::SILENT;
        self.previous = 0.0;
    }

    pub fn estimate(&self) -> PitchEstimate {
        self.last
    }

    /// Feed a block of **mono** samples. The answer only changes on a hop
    /// boundary; in between, the last one stands.
    pub fn process(&mut self, mono: &[f32]) -> PitchEstimate {
        for &x in mono {
            // Band-limit first, average second. The other way round is a box
            // filter, and a box filter is why a voice folded.
            let mut x = if x.is_finite() { x } else { 0.0 };
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
            self.filled = (self.filled + 1).min(WINDOW);
            self.since += 1;
        }
        if self.filled < WINDOW || self.since < HOP {
            return self.last;
        }
        self.since = 0;
        self.last = self.analyse();
        self.last
    }

    fn analyse(&mut self) -> PitchEstimate {
        let rms = rms(&self.window);
        if !rms.is_finite() || rms <= self.gate {
            self.previous = 0.0;
            self.recent_n = 0;
            self.unvoiced_for = 0;
            return PitchEstimate::SILENT;
        }
        let half = WINDOW / 2;
        let min_lag = (self.work_rate / self.max_hz.max(1.0)) as usize;
        let max_lag = (self.work_rate / self.min_hz.max(1.0)) as usize;
        // Straightened out first: YIN's inner loop has no wrap in it when the
        // window starts at zero, and that loop is the whole cost of an
        // analysis.
        let (tail, head) = self.window.split_at(self.write);
        self.linear[..head.len()].copy_from_slice(head);
        self.linear[head.len()..].copy_from_slice(tail);
        let Some((period, clarity)) =
            crate::pitch::yin(&self.linear, 0, half, min_lag, max_lag, &mut self.diff)
        else {
            self.previous = 0.0;
            return self.hold_or_silence();
        };
        if !period.is_finite() || period <= 0.0 {
            return self.hold_or_silence();
        }
        let mut hz = self.work_rate / period;
        if !(self.min_hz..=self.max_hz).contains(&hz) {
            self.previous = 0.0;
            return self.hold_or_silence();
        }
        // The one error YIN still makes on a voice: locking to the octave when
        // the second partial is the loudest thing in the window. If the reading
        // is within a few cents of double or half the last accepted one, the
        // continuous answer is the likelier one — a singer does not jump an
        // octave between two 5 ms hops and land exactly in tune.
        if self.previous > 0.0 {
            for factor in [0.5f32, 2.0] {
                let candidate = hz * factor;
                if (candidate / self.previous).log2().abs() < 0.03
                    && (hz / self.previous).log2().abs() > 0.4
                    && (self.min_hz..=self.max_hz).contains(&candidate)
                {
                    hz = candidate;
                    break;
                }
            }
        }
        if clarity < self.voiced_threshold {
            return self.hold_or_silence();
        }
        self.previous = hz;
        self.unvoiced_for = 0;
        PitchEstimate {
            frequency_hz: self.median_of(hz),
            confidence: clarity,
            voiced: true,
        }
    }

    /// One window's answer, smoothed by the two beside it.
    ///
    /// A median, not an average: an average of a good reading and an octave
    /// error is a note that was never there, while a median of three throws the
    /// octave error away and keeps a real glide moving. What it costs is 16 ms
    /// of lag on a slide, which is under the shortest retune anybody sets.
    fn median_of(&mut self, hz: f32) -> f32 {
        self.recent.rotate_left(1);
        self.recent[MEDIAN_ANALYSES - 1] = hz;
        self.recent_n = (self.recent_n + 1).min(MEDIAN_ANALYSES);
        if self.recent_n < MEDIAN_ANALYSES {
            // Not enough history to outvote anything yet: the reading stands,
            // which is what the note's first few milliseconds need anyway.
            return hz;
        }
        let mut sorted = self.recent;
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        sorted[MEDIAN_ANALYSES / 2]
    }

    /// A window this one does not believe. The last note stands for a couple of
    /// analyses before the effect gives up on it — see [`UNVOICED_HOLD`].
    fn hold_or_silence(&mut self) -> PitchEstimate {
        self.recent_n = 0;
        if !self.last.voiced {
            return PitchEstimate::SILENT;
        }
        self.unvoiced_for += 1;
        if self.unvoiced_for > UNVOICED_HOLD {
            self.previous = 0.0;
            return PitchEstimate::SILENT;
        }
        self.last
    }
}

fn rms(w: &[f32]) -> f32 {
    let sum: f64 = w.iter().map(|s| (*s as f64) * (*s as f64)).sum();
    (sum / w.len().max(1) as f64).sqrt() as f32
}
