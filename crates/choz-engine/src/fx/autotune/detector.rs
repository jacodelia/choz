//! F0 estimation for a monophonic voice: YIN over a ring, once per hop.
//!
//! The arithmetic is [`crate::pitch::yin`] — the same one the `A→M` button uses,
//! written once. The windowing is the same too, and for the same measured
//! reason: **YIN at the device's rate does not fit in the callback.**
//!
//! At 48 kHz with a window long enough for 60 Hz, one analysis is ~2.2 million
//! operations, and a 256-sample hop asks for 187 of them a second: 410 M
//! operations per second, for one voice. Decimating to 16 kHz — a box average,
//! which is both the downsample and its anti-alias filter — cuts the window and
//! the lag range together, and the same analysis costs ~117 k. Thirty times
//! less, and nothing is lost: a guitar's top note is 1.3 kHz and a voice's is
//! lower, so there is nothing above 8 kHz that says anything about the period.
//!
//! The *frequency* keeps its precision because the dip is interpolated, and the
//! shifter is handed the period back at the full rate as `sample_rate / f0`.

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
    pub const SILENT: Self =
        PitchEstimate { frequency_hz: 0.0, confidence: 0.0, voiced: false };
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

pub struct PitchDetector {
    sample_rate: f32,
    /// Samples averaged into one, and the rate that leaves.
    decim: usize,
    work_rate: f32,
    /// Partial average waiting for its last few samples.
    acc: f32,
    acc_n: usize,
    window: Vec<f32>,
    write: usize,
    filled: usize,
    since: usize,
    diff: Vec<f32>,
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
            window: vec![0.0; WINDOW],
            write: 0,
            filled: 0,
            since: 0,
            diff: vec![0.0; WINDOW / 2],
            last: PitchEstimate::SILENT,
            // -50 dBFS. A sung note is far above it; room tone is not.
            gate: 0.0032,
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
        self.reset();
    }

    pub fn reset(&mut self) {
        self.acc = 0.0;
        self.acc_n = 0;
        self.window.fill(0.0);
        self.write = 0;
        self.filled = 0;
        self.since = 0;
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
            self.acc += if x.is_finite() { x } else { 0.0 };
            self.acc_n += 1;
            if self.acc_n < self.decim {
                continue;
            }
            self.window[self.write] = self.acc / self.acc_n as f32;
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
            return PitchEstimate::SILENT;
        }
        let half = WINDOW / 2;
        let min_lag = (self.work_rate / self.max_hz.max(1.0)) as usize;
        let max_lag = (self.work_rate / self.min_hz.max(1.0)) as usize;
        let Some((period, clarity)) =
            crate::pitch::yin(&self.window, self.write, half, min_lag, max_lag, &mut self.diff)
        else {
            self.previous = 0.0;
            return PitchEstimate::SILENT;
        };
        if !period.is_finite() || period <= 0.0 {
            return PitchEstimate::SILENT;
        }
        let mut hz = self.work_rate / period;
        if !(self.min_hz..=self.max_hz).contains(&hz) {
            self.previous = 0.0;
            return PitchEstimate::SILENT;
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
        let voiced = clarity >= self.voiced_threshold;
        if voiced {
            self.previous = hz;
        }
        PitchEstimate { frequency_hz: hz, confidence: clarity, voiced }
    }
}

fn rms(w: &[f32]) -> f32 {
    let sum: f64 = w.iter().map(|s| (*s as f64) * (*s as f64)).sum();
    (sum / w.len().max(1) as f64).sqrt() as f32
}
