//! **AutoTune — real-time pitch correction**, built in.
//!
//! Audio in, the same audio in tune, at the rack's own latency. Not a wrapper
//! around anything: the detector, the quantiser, the smoother and the shifter
//! are all here, and each is its own file because each is replaceable on its
//! own.
//!
//! ```text
//! in ─► pre ─► [detector] F0, confidence, voiced
//!        │          │
//!        │          ▼
//!        │     [quantizer] key + scale ─► target note ─► target Hz
//!        │          │
//!        │          ▼
//!        │     [corrector] retune speed, correction, humanise ─► ratio
//!        │          │
//!        │          ▼
//!        └────► [shifter] PSOLA, one per channel ────────────► out
//! ```
//!
//! ## What it is for, and what it is not
//!
//! **Monophonic sources only** — a voice, a bass, a lead line. A chord has more
//! than one pitch and this reports one; that is not a bug to be fixed later, it
//! is what a monophonic tracker is. Polyphonic correction is a different
//! algorithm and a different effect.
//!
//! ## Realtime contract
//!
//! Every buffer is allocated in [`AutoTune::new`], sized for the longest period
//! (60 Hz) at the highest supported rate (96 kHz). `process_block` allocates
//! nothing, locks nothing and blocks on nothing, including when the sample rate
//! changes underneath it: the buffers are already big enough, so a rate change
//! costs a `reset` and no more.

pub mod corrector;
pub mod detector;
pub mod meter;
pub mod quantizer;
pub mod shifter;

#[cfg(test)]
mod tests;

use choz_ports::{FxParam, FxProcessor};

pub use corrector::{AutoTuneMode, PitchCorrector};
pub use detector::{PitchDetector, PitchEstimate};
pub use meter::AutoTuneMeter;
pub use quantizer::{NoteQuantizer, PitchTarget, Scale, ScaleType, NOTE_NAMES};
pub use shifter::{PitchShifter, RetuneShifter};

/// Longest block the mono scratch is sized for. The engine's own callback
/// buffer is capped well under this; a longer one is processed in chunks.
const MAX_BLOCK: usize = 8192;

/// Presets, as parameter sets: `(name, retune ms, correction, humanize, mode)`
/// — the four knobs that make one sound different from another. Key, scale and
/// the frequency range belong to the song, not to the preset, so a preset
/// leaves them alone.
pub const PRESETS: &[(&str, f32, f32, f32, AutoTuneMode)] = &[
    ("Natural Vocal", 120.0, 0.85, 0.25, AutoTuneMode::Natural),
    ("Fast Vocal", 35.0, 1.0, 0.10, AutoTuneMode::Natural),
    ("Hard Auto-Tune", 1.0, 1.0, 0.0, AutoTuneMode::HardTune),
    ("Subtle Correction", 300.0, 0.5, 0.4, AutoTuneMode::Natural),
    ("Robot Voice", 4.0, 1.0, 0.0, AutoTuneMode::HardTune),
];

/// Which preset a knob position picks. 0 is the first, which is also what a
/// knob left alone means.
pub fn preset_index(norm: f32) -> usize {
    (norm.clamp(0.0, 1.0) * (PRESETS.len() - 1) as f32).round() as usize
}

/// Everything the user can move. Kept as one struct so a preset is an
/// assignment rather than a list of setters that can be forgotten.
#[derive(Debug, Clone, Copy)]
pub struct AutoTuneParameters {
    pub input_gain_db: f32,
    pub retune_speed_ms: f32,
    pub correction: f32,
    pub humanize: f32,
    pub output_gain_db: f32,
    pub key: u8,
    pub scale: ScaleType,
    pub mode: AutoTuneMode,
    pub reference_hz: f32,
    pub min_frequency: f32,
    pub max_frequency: f32,
}

impl Default for AutoTuneParameters {
    fn default() -> Self {
        Self {
            input_gain_db: 0.0,
            retune_speed_ms: 80.0,
            correction: 1.0,
            humanize: 0.0,
            output_gain_db: 0.0,
            key: 0,
            scale: ScaleType::Chromatic,
            mode: AutoTuneMode::Natural,
            reference_hz: 440.0,
            min_frequency: 70.0,
            max_frequency: 1200.0,
        }
    }
}

pub struct AutoTune {
    detector: PitchDetector,
    quantizer: NoteQuantizer,
    corrector: PitchCorrector,
    /// One shifter per channel, driven by the same control signal — the pitch
    /// is decided on the mono sum, so the stereo image survives.
    shifters: [RetuneShifter; 2],
    sample_rate: f32,
    /// Mono sum of the block, for the detector.
    mono: Vec<f32>,
    /// Per-channel scratch, in and out.
    chan_in: [Vec<f32>; 2],
    chan_out: [Vec<f32>; 2],
    /// The dry signal, delayed to meet the wet one. Mixing an undelayed dry
    /// with a wet that is 33 ms late is a comb filter, not a mix.
    dry: [Vec<f32>; 2],
    dry_write: usize,
    detected_hz: f32,
    target_hz: f32,
    wet: f32,
    pub params: AutoTuneParameters,
}

impl Default for AutoTune {
    fn default() -> Self {
        Self::new(48_000.0)
    }
}

impl AutoTune {
    pub fn new(sample_rate: f32) -> Self {
        let sr = sample_rate.max(1.0);
        let delay = 2 * detector::MAX_PERIOD + 1;
        let mut me = Self {
            detector: PitchDetector::new(sr),
            quantizer: NoteQuantizer::default(),
            corrector: PitchCorrector::new(sr),
            shifters: [RetuneShifter::new(sr), RetuneShifter::new(sr)],
            sample_rate: sr,
            mono: vec![0.0; MAX_BLOCK],
            chan_in: [vec![0.0; MAX_BLOCK], vec![0.0; MAX_BLOCK]],
            chan_out: [vec![0.0; MAX_BLOCK], vec![0.0; MAX_BLOCK]],
            dry: [vec![0.0; delay], vec![0.0; delay]],
            dry_write: 0,
            detected_hz: 0.0,
            target_hz: 0.0,
            wet: 1.0,
            params: AutoTuneParameters::default(),
        };
        me.apply_params();
        me
    }

    /// Samples the output runs behind the input.
    pub fn latency_samples(&self) -> usize {
        self.shifters[0].latency_samples()
    }

    /// Load one of [`PRESETS`]. Out of range leaves everything alone.
    pub fn set_preset(&mut self, index: usize) {
        let Some(&(_, retune, correction, humanize, mode)) = PRESETS.get(index) else {
            return;
        };
        self.params.retune_speed_ms = retune;
        self.params.correction = correction;
        self.params.humanize = humanize;
        self.params.mode = mode;
        self.apply_params();
    }

    /// Push the parameter block into the parts that hold their own copy.
    pub fn apply_params(&mut self) {
        let p = &self.params;
        self.detector.min_hz = p.min_frequency.clamp(detector::MIN_SUPPORTED_HZ, 800.0);
        self.detector.max_hz = p.max_frequency.clamp(self.detector.min_hz + 20.0, 2000.0);
        self.quantizer.scale = Scale::new(p.key, p.scale);
        self.quantizer.reference_hz = p.reference_hz.clamp(400.0, 480.0);
        self.corrector.retune_ms = p.retune_speed_ms;
        self.corrector.correction = p.correction;
        self.corrector.humanize = p.humanize;
        self.corrector.mode = p.mode;
    }

    /// The last reading, for tests and for the meter.
    pub fn reading(&self) -> AutoTuneMeter {
        let e = self.detector.estimate();
        let cents = if self.detected_hz > 0.0 && self.target_hz > 0.0 {
            1200.0 * (self.detected_hz / self.target_hz).log2()
        } else {
            0.0
        };
        AutoTuneMeter {
            detected_frequency: self.detected_hz,
            target_frequency: self.target_hz,
            pitch_error_cents: cents,
            confidence: e.confidence,
            voiced: e.voiced,
            level: 0.0,
        }
    }

    /// One chunk, at most [`MAX_BLOCK`] frames. `buf` is interleaved stereo.
    fn process_chunk(&mut self, buf: &mut [f32]) {
        let frames = buf.len() / 2;
        if frames == 0 {
            return;
        }
        let in_gain = db_to_lin(self.params.input_gain_db);
        let out_gain = db_to_lin(self.params.output_gain_db);

        // ── De-interleave, and the mono sum the analysis works on ───────────
        let mut sum_sq = 0.0f64;
        for f in 0..frames {
            let l = sanitise(buf[f * 2]) * in_gain;
            let r = sanitise(buf[f * 2 + 1]) * in_gain;
            self.chan_in[0][f] = l;
            self.chan_in[1][f] = r;
            let m = (l + r) * 0.5;
            self.mono[f] = m;
            sum_sq += (m as f64) * (m as f64);
        }
        let level = (sum_sq / frames as f64).sqrt() as f32;

        // ── Detect, quantise, decide the ratio ──────────────────────────────
        let est = self.detector.process(&self.mono[..frames]);
        self.detected_hz = if est.voiced { est.frequency_hz } else { 0.0 };
        let target = est
            .voiced
            .then(|| self.quantizer.target_hz(est.frequency_hz))
            .flatten();
        self.target_hz = target.unwrap_or(0.0);

        // Unvoiced, or a reading choz does not believe: aim at no correction at
        // all and let the smoother walk back there. Snapping to 1.0 would be a
        // click on every consonant.
        let error_semitones = match target {
            Some(t) if est.frequency_hz > 0.0 => 12.0 * (t / est.frequency_hz).log2(),
            _ => 0.0,
        };
        let ratio = self.corrector.advance(error_semitones, frames);

        // The period the grains are cut on is the **detected** one; it is what
        // the input actually contains. The ratio moves their spacing.
        let period = if est.voiced && est.frequency_hz > 0.0 {
            self.sample_rate / est.frequency_hz
        } else {
            0.0
        };

        for ch in 0..2 {
            let (input, output) = (
                &self.chan_in[ch][..frames],
                &mut self.chan_out[ch][..frames],
            );
            self.shifters[ch].process(input, output, ratio, period);
        }

        // ── Delay the dry, then mix ─────────────────────────────────────────
        let latency = self.latency_samples();
        let dry_len = self.dry[0].len();
        for f in 0..frames {
            let slot = (self.dry_write + f) % dry_len;
            let read = (self.dry_write + f + dry_len - latency % dry_len) % dry_len;
            for ch in 0..2 {
                let delayed = self.dry[ch][read];
                self.dry[ch][slot] = self.chan_in[ch][f];
                let wet = self.chan_out[ch][f];
                let y = (delayed * (1.0 - self.wet) + wet * self.wet) * out_gain;
                buf[f * 2 + ch] = sanitise(y);
            }
        }
        self.dry_write = (self.dry_write + frames) % dry_len;

        meter::meter().publish(AutoTuneMeter {
            level,
            ..self.reading()
        });
    }
}

impl FxProcessor for AutoTune {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        let sr = sample_rate.max(1) as f32;
        if (sr - self.sample_rate).abs() > 0.5 {
            self.sample_rate = sr;
            self.detector.set_sample_rate(sr);
            self.corrector.set_sample_rate(sr);
            for s in self.shifters.iter_mut() {
                s.set_sample_rate(sr);
            }
            self.reset();
        }
        // A block longer than the scratch is walked in pieces rather than
        // reallocating: the audio thread may not allocate, and a host that
        // hands over a huge buffer is a host to survive, not to argue with.
        for chunk in buf.chunks_mut(MAX_BLOCK * 2) {
            self.process_chunk(chunk);
        }
    }

    fn reset(&mut self) {
        self.detector.reset();
        self.corrector.reset();
        for s in self.shifters.iter_mut() {
            s.reset();
        }
        for d in self.dry.iter_mut() {
            d.fill(0.0);
        }
        self.dry_write = 0;
        self.detected_hz = 0.0;
        self.target_hz = 0.0;
    }

    fn set_mix(&mut self, wet: f32) {
        self.wet = wet.clamp(0.0, 1.0);
    }

    fn name(&self) -> &str {
        "AUTO-TUNE"
    }

    /// The shifter's window, which is the one place the signal is held back.
    /// Reported so the rack can say where its 30-odd milliseconds went.
    fn latency_samples(&self) -> u32 {
        AutoTune::latency_samples(self) as u32
    }

    fn params(&self) -> Vec<FxParam> {
        let p = &self.params;
        vec![
            FxParam::new("Preset", 0.0, 0.0, (PRESETS.len() - 1) as f32, ""),
            FxParam::new(
                "Retune",
                norm(p.retune_speed_ms, 0.0, 1000.0),
                0.0,
                1000.0,
                "ms",
            ),
            FxParam::new("Correct", p.correction, 0.0, 100.0, "%"),
            FxParam::new("Key", p.key as f32 / 11.0, 0.0, 11.0, ""),
            FxParam::new("Scale", scale_norm(p.scale), 0.0, 5.0, ""),
            FxParam::new(
                "Mode",
                (p.mode == AutoTuneMode::HardTune) as u8 as f32,
                0.0,
                1.0,
                "",
            ),
            FxParam::new("Human", p.humanize, 0.0, 100.0, "%"),
            FxParam::new("A4", norm(p.reference_hz, 430.0, 450.0), 430.0, 450.0, "Hz"),
            FxParam::new(
                "MinHz",
                norm(p.min_frequency, 60.0, 400.0),
                60.0,
                400.0,
                "Hz",
            ),
            FxParam::new(
                "MaxHz",
                norm(p.max_frequency, 400.0, 1200.0),
                400.0,
                1200.0,
                "Hz",
            ),
            FxParam::new(
                "InGain",
                norm(p.input_gain_db, -24.0, 24.0),
                -24.0,
                24.0,
                "dB",
            ),
            FxParam::new(
                "OutGain",
                norm(p.output_gain_db, -24.0, 24.0),
                -24.0,
                24.0,
                "dB",
            ),
        ]
    }

    /// The order is [`Self::params`]'s, and it is frozen: a CC learned on
    /// "Retune" must stay on Retune.
    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => {
                let i = (v * (PRESETS.len() - 1) as f32).round() as usize;
                self.set_preset(i);
                return;
            }
            1 => self.params.retune_speed_ms = denorm(v, 0.0, 1000.0),
            2 => self.params.correction = v,
            3 => self.params.key = (v * 11.0).round() as u8,
            4 => {
                let i = (v * (ScaleType::ALL.len() - 1) as f32).round() as usize;
                self.params.scale = ScaleType::ALL[i.min(ScaleType::ALL.len() - 1)];
            }
            5 => {
                self.params.mode = if v >= 0.5 {
                    AutoTuneMode::HardTune
                } else {
                    AutoTuneMode::Natural
                }
            }
            6 => self.params.humanize = v,
            7 => self.params.reference_hz = denorm(v, 430.0, 450.0),
            8 => self.params.min_frequency = denorm(v, 60.0, 400.0),
            9 => self.params.max_frequency = denorm(v, 400.0, 1200.0),
            10 => self.params.input_gain_db = denorm(v, -24.0, 24.0),
            11 => self.params.output_gain_db = denorm(v, -24.0, 24.0),
            _ => return,
        }
        self.apply_params();
    }
}

fn norm(v: f32, lo: f32, hi: f32) -> f32 {
    ((v - lo) / (hi - lo)).clamp(0.0, 1.0)
}

fn denorm(v: f32, lo: f32, hi: f32) -> f32 {
    lo + v.clamp(0.0, 1.0) * (hi - lo)
}

fn scale_norm(s: ScaleType) -> f32 {
    ScaleType::ALL.iter().position(|x| *x == s).unwrap_or(0) as f32
        / (ScaleType::ALL.len() - 1) as f32
}

fn db_to_lin(db: f32) -> f32 {
    if db.is_finite() {
        10f32.powf(db / 20.0)
    } else {
        1.0
    }
}

/// Nothing leaves this effect that is not a number. A NaN in an audio buffer
/// spreads: it poisons the next FX, the mix bus and the device.
#[inline]
fn sanitise(x: f32) -> f32 {
    if x.is_finite() {
        x.clamp(-8.0, 8.0)
    } else {
        0.0
    }
}
