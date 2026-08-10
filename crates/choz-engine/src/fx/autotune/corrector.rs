//! How fast the pitch is dragged to where it should be.
//!
//! Everything here happens in the **log domain** — semitones, not hertz.
//! Smoothing a frequency linearly slides through the wrong notes on the way: an
//! octave is a doubling, so the same "half way there" in Hz is a different
//! interval depending on where you started. In semitones, half way is half way.

/// Which way the effect leans.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AutoTuneMode {
    /// Follow the singer: the retune time is what the user set, and the
    /// spectral envelope is left where it was.
    #[default]
    Natural,
    /// The creative effect: snap, and snap immediately.
    HardTune,
}

impl AutoTuneMode {
    pub fn label(self) -> &'static str {
        match self {
            AutoTuneMode::Natural => "Natural",
            AutoTuneMode::HardTune => "Hard Tune",
        }
    }
}

/// Turns "you are 40 cents flat" into a pitch ratio that moves like a voice.
pub struct PitchCorrector {
    sample_rate: f32,
    /// Where the shift is now, in semitones. The state that must not jump.
    current_semitones: f32,
    /// Phase of the humanise drift, in radians.
    lfo: f32,
    /// 0 ms — 1000 ms.
    pub retune_ms: f32,
    /// 0..1. How much of the error is taken out at all.
    pub correction: f32,
    /// 0..1. How much the retune time wanders, so held notes do not all
    /// converge with the same mechanical curve.
    pub humanize: f32,
    pub mode: AutoTuneMode,
}

/// The shortest retune the smoother will use. Zero would be a step, and a step
/// in pitch ratio is a click — Hard Tune is fast, not discontinuous.
const MIN_RETUNE_MS: f32 = 1.0;

/// Beyond this much error, the reading is not believed.
///
/// A singer is out by a semitone, not by a fifth. An error that big is the
/// detector having found a harmonic or a noise, and correcting it is the one
/// thing that turns a voice into rubbish — the pitch shifter is asked for a
/// ratio it cannot make cleanly, and it obliges.
const MAX_BELIEVABLE_SEMITONES: f32 = 3.0;

/// How far humanise moves the retune time, at full, and how slowly.
const HUMANIZE_DEPTH: f32 = 0.6;
const HUMANIZE_HZ: f32 = 0.7;

impl PitchCorrector {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate: sample_rate.max(1.0),
            current_semitones: 0.0,
            lfo: 0.0,
            retune_ms: 80.0,
            correction: 1.0,
            humanize: 0.0,
            mode: AutoTuneMode::Natural,
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
    }

    pub fn reset(&mut self) {
        self.current_semitones = 0.0;
        self.lfo = 0.0;
    }

    /// The shift currently applied, in semitones.
    pub fn semitones(&self) -> f32 {
        self.current_semitones
    }

    /// Advance by `frames` samples towards `target_semitones` and return the
    /// pitch ratio to use for that block.
    ///
    /// `target_semitones` is the *whole* error; `correction` decides how much of
    /// it is taken, which is what makes 50 % a singer who is nearly in tune
    /// rather than a singer who is corrected half the time.
    pub fn advance(&mut self, target_semitones: f32, frames: usize) -> f32 {
        let target = match target_semitones {
            t if !t.is_finite() => 0.0,
            // Not clamped — *ignored*. Clamping a wild reading still bends the
            // voice by three semitones towards a note nobody sang; treating it
            // as "no correction" leaves the singer alone until the detector
            // agrees with itself again.
            t if t.abs() > MAX_BELIEVABLE_SEMITONES => 0.0,
            t => t,
        };
        let (retune, correction) = match self.mode {
            // Hard Tune ignores both knobs on purpose: it is one sound, and a
            // Hard Tune that can be set to 400 ms is just Natural with a
            // confusing name.
            AutoTuneMode::HardTune => (MIN_RETUNE_MS, 1.0),
            AutoTuneMode::Natural => (self.retune_ms.clamp(0.0, 1000.0), self.correction.clamp(0.0, 1.0)),
        };
        let wanted = target * correction;

        // Humanise wanders the retune time rather than the pitch: a note still
        // arrives where it should, it just does not arrive along the same curve
        // every time. Modulating the pitch itself would be a vibrato the singer
        // did not sing.
        let depth = self.humanize.clamp(0.0, 1.0) * HUMANIZE_DEPTH;
        let retune = (retune * (1.0 + depth * self.lfo.sin())).max(MIN_RETUNE_MS);
        self.lfo += std::f32::consts::TAU * HUMANIZE_HZ * frames as f32 / self.sample_rate;
        if self.lfo > std::f32::consts::TAU {
            self.lfo -= std::f32::consts::TAU;
        }

        // One pole, per block: `a` is how much of the gap is left after this
        // many samples. `retune_ms` is the time constant, so ~63 % of the way
        // there in that time — which is what a retune time means everywhere.
        let tau = retune * 0.001 * self.sample_rate;
        let a = if tau > 0.5 { (-(frames as f32) / tau).exp() } else { 0.0 };
        self.current_semitones = wanted + (self.current_semitones - wanted) * a;
        if !self.current_semitones.is_finite() {
            self.current_semitones = 0.0;
        }
        let ratio = (2.0f32).powf(self.current_semitones / 12.0);
        if ratio.is_finite() { ratio.clamp(0.25, 4.0) } else { 1.0 }
    }
}
