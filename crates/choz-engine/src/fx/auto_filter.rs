//! Auto-filter: a state-variable filter with its cutoff moved by an LFO, by
//! the input level, or by both.
//!
//! The existing [`super::filter::Svf`] is a filter you set. This is the same
//! topology with something moving it — which is a different effect, not a knob
//! on that one: a static filter has no rate, no depth and no envelope, and
//! hanging five dead knobs on it to reach one moving one is how a simple
//! processor stops being simple.
//!
//! # Real-time
//!
//! The SVF coefficients need a `tan()`, so they are recomputed **every 16
//! frames** rather than every sample. At the rates an LFO runs (≤20 Hz) that is
//! a third of a millisecond of staleness — inaudible — for a sixteenth of the
//! transcendental work.
// ponytail: 16-frame coefficient updates. Per-sample the day someone modulates
// a filter at audio rate, which this LFO cannot do anyway.

use super::lfo::{Lfo, Wave};
use super::smooth::Smoothed;
use std::f32::consts::PI;

/// How often the coefficients catch up with the modulation, in frames.
const COEFF_INTERVAL: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FilterMode {
    #[default]
    Lowpass,
    Bandpass,
    Highpass,
}

impl FilterMode {
    pub const ALL: [FilterMode; 3] = [
        FilterMode::Lowpass,
        FilterMode::Bandpass,
        FilterMode::Highpass,
    ];

    pub fn label(self) -> &'static str {
        match self {
            FilterMode::Lowpass => "LP",
            FilterMode::Bandpass => "BP",
            FilterMode::Highpass => "HP",
        }
    }

    pub fn to_norm(self) -> f32 {
        Self::ALL.iter().position(|m| *m == self).unwrap_or(0) as f32 / (Self::ALL.len() - 1) as f32
    }

    pub fn from_norm(v: f32) -> Self {
        let n = Self::ALL.len();
        let i = (v.clamp(0.0, 1.0) * (n - 1) as f32).round() as usize;
        Self::ALL[i.min(n - 1)]
    }
}

/// One channel of Simper's topology-preserving SVF, with its own coefficients
/// so the two channels can sit at different cutoffs — which is the whole point
/// of a stereo spread.
#[derive(Clone, Copy, Default)]
struct SvfChannel {
    ic1: f32,
    ic2: f32,
    a1: f32,
    a2: f32,
    a3: f32,
    k: f32,
}

impl SvfChannel {
    fn set(&mut self, cutoff_hz: f32, resonance: f32, sr: f32) {
        // Never past Nyquist: `tan` goes to infinity there and takes the filter
        // with it.
        let hz = cutoff_hz.clamp(20.0, sr * 0.45);
        let g = (PI * hz / sr).tan();
        let k = 2.0 - 2.0 * resonance.clamp(0.0, 0.98);
        self.k = k;
        self.a1 = 1.0 / (1.0 + g * (g + k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
    }

    #[inline]
    fn process(&mut self, x: f32, mode: FilterMode) -> f32 {
        let v3 = x - self.ic2;
        let v1 = self.a1 * self.ic1 + self.a2 * v3;
        let v2 = self.ic2 + self.a2 * self.ic1 + self.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        match mode {
            FilterMode::Lowpass => v2,
            FilterMode::Bandpass => v1,
            FilterMode::Highpass => x - self.k * v1 - v2,
        }
    }
}

pub struct AutoFilter {
    mode: FilterMode,
    /// Where the cutoff sits with nothing modulating it, in Hz.
    base_hz: f32,
    resonance: f32,
    wave: Wave,
    rate_hz: f32,
    /// LFO depth in octaves, ±.
    depth_oct: f32,
    /// How far the right channel's LFO runs behind the left, in cycles.
    spread: f32,
    /// How far the input level pushes the cutoff, in octaves. Signed: a
    /// negative amount is a filter that closes as it is played harder.
    env_oct: f32,
    env: Smoothed,
    lfo: Lfo,
    left: SvfChannel,
    right: SvfChannel,
    countdown: usize,
    mix: f32,
    sample_rate: f32,
}

impl AutoFilter {
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate.max(8000) as f32;
        let mut f = Self {
            mode: FilterMode::Lowpass,
            base_hz: 800.0,
            resonance: 0.4,
            wave: Wave::Sine,
            rate_hz: 1.0,
            depth_oct: 2.0,
            spread: 0.0,
            env_oct: 0.0,
            // 30 ms: fast enough to follow a note, slow enough not to follow
            // the waveform itself, which would be distortion and not tracking.
            env: Smoothed::new(0.0, 30.0, sr),
            lfo: Lfo::new(),
            left: SvfChannel::default(),
            right: SvfChannel::default(),
            countdown: 0,
            mix: 1.0,
            sample_rate: sr,
        };
        f.left.set(f.base_hz, f.resonance, sr);
        f.right.set(f.base_hz, f.resonance, sr);
        f
    }

    /// Build from the rack's knob positions: freq, res, mode, rate, depth,
    /// shape, spread, env.
    pub fn with_params(sample_rate: u32, p: &[f32]) -> Self {
        let get = |i: usize, d: f32| p.get(i).copied().unwrap_or(d);
        let mut f = Self::new(sample_rate);
        f.set_freq(get(0, 0.5));
        f.resonance = get(1, 0.4) * 0.98;
        f.mode = FilterMode::from_norm(get(2, 0.0));
        f.set_rate(get(3, 0.35));
        f.depth_oct = get(4, 0.5) * 4.0;
        f.wave = Wave::from_norm(get(5, 0.0));
        f.spread = get(6, 0.0);
        f.env_oct = (get(7, 0.5) - 0.5) * 8.0;
        f
    }

    /// 0..1 → 20 Hz–20 kHz, logarithmic. An octave is the same distance
    /// wherever the knob is; linear, three quarters of the travel is above
    /// where anyone puts a cutoff.
    pub fn set_freq(&mut self, v: f32) {
        self.base_hz = 20.0 * 1000.0f32.powf(v.clamp(0.0, 1.0));
    }

    pub fn set_rate(&mut self, v: f32) {
        self.rate_hz = 0.02 * 1000.0f32.powf(v.clamp(0.0, 1.0));
    }

    /// Where the two channels' cutoffs are right now, for a test or a meter.
    pub fn cutoffs(&self) -> (f32, f32) {
        let m = [self.lfo_value(0), self.lfo_value(1)];
        let e = self.env.value() * self.env_oct;
        (
            self.base_hz * 2.0f32.powf(m[0] * self.depth_oct + e),
            self.base_hz * 2.0f32.powf(m[1] * self.depth_oct + e),
        )
    }

    fn lfo_value(&self, ch: usize) -> f32 {
        // Read-only peek at the shape, for `cutoffs`; the audio path uses the
        // ticking value so the two cannot be asked to agree on a phase.
        let ph = (self.lfo.phase() + if ch == 1 { self.spread } else { 0.0 }).fract();
        match self.wave {
            Wave::Sine => (std::f32::consts::TAU * ph).sin(),
            _ => 0.0,
        }
    }
}

impl super::FxProcessor for AutoFilter {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        let sr = sample_rate.max(8000) as f32;
        if (sr - self.sample_rate).abs() > 0.5 {
            self.sample_rate = sr;
            self.env.set_sample_rate(sr);
            self.countdown = 0;
        }
        let (mode, wave, rate, spread) = (self.mode, self.wave, self.rate_hz, self.spread);
        let mix = self.mix;

        for frame in buf.as_chunks_mut::<2>().0 {
            let (dry_l, dry_r) = (frame[0], frame[1]);
            let m = self.lfo.tick(wave, rate, sr, spread);
            // Envelope of what is coming in, before the filter: the follower
            // must not hear its own filtering, or a closing filter closes
            // further and the effect eats itself.
            self.env.set_target(dry_l.abs().max(dry_r.abs()).min(1.0));
            let e = self.env.tick() * self.env_oct;

            if self.countdown == 0 {
                self.left.set(
                    self.base_hz * 2.0f32.powf(m[0] * self.depth_oct + e),
                    self.resonance,
                    sr,
                );
                self.right.set(
                    self.base_hz * 2.0f32.powf(m[1] * self.depth_oct + e),
                    self.resonance,
                    sr,
                );
                self.countdown = COEFF_INTERVAL;
            }
            self.countdown -= 1;

            let mut wl = self.left.process(dry_l, mode);
            let mut wr = self.right.process(dry_r, mode);
            // A resonant filter handed a non-finite sample stays non-finite
            // for ever: clear it and pass the dry signal rather than poison
            // the bus.
            if !wl.is_finite() || !wr.is_finite() {
                self.left = SvfChannel::default();
                self.right = SvfChannel::default();
                self.countdown = 0;
                wl = dry_l;
                wr = dry_r;
            }
            frame[0] = dry_l + mix * (wl - dry_l);
            frame[1] = dry_r + mix * (wr - dry_r);
        }
    }

    fn reset(&mut self) {
        self.lfo.reset();
        self.env.snap(0.0);
        self.left = SvfChannel::default();
        self.right = SvfChannel::default();
        self.countdown = 0;
    }

    fn set_mix(&mut self, wet: f32) {
        self.mix = wet.clamp(0.0, 1.0);
    }

    fn name(&self) -> &str {
        "AutoFilter"
    }

    fn params(&self) -> Vec<crate::fx::FxParam> {
        use crate::fx::FxParam;
        vec![
            FxParam::new(
                "Freq",
                (self.base_hz / 20.0).log(1000.0).clamp(0.0, 1.0),
                20.0,
                20000.0,
                "Hz",
            ),
            FxParam::new("Res", self.resonance / 0.98, 0.0, 1.0, ""),
            FxParam::new("Mode", self.mode.to_norm(), 0.0, 1.0, ""),
            FxParam::new(
                "Rate",
                (self.rate_hz / 0.02).log(1000.0).clamp(0.0, 1.0),
                0.02,
                20.0,
                "Hz",
            ),
            FxParam::new("Depth", self.depth_oct / 4.0, 0.0, 4.0, "oct"),
            FxParam::new("Shape", self.wave.to_norm(), 0.0, 1.0, ""),
            FxParam::new("Spread", self.spread, 0.0, 1.0, ""),
            FxParam::new("Env", self.env_oct / 8.0 + 0.5, -4.0, 4.0, "oct"),
            FxParam::new("Wet", self.mix, 0.0, 1.0, ""),
        ]
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => self.set_freq(v),
            1 => self.resonance = v * 0.98,
            2 => self.mode = FilterMode::from_norm(v),
            3 => self.set_rate(v),
            4 => self.depth_oct = v * 4.0,
            5 => self.wave = Wave::from_norm(v),
            6 => self.spread = v,
            7 => self.env_oct = (v - 0.5) * 8.0,
            8 => self.mix = v,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::FxProcessor;

    fn tone(hz: f32, sr: u32, frames: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let s = (2.0 * PI * hz * i as f32 / sr as f32).sin() * 0.5;
                [s, s]
            })
            .collect()
    }

    fn rms(buf: &[f32]) -> f32 {
        (buf.iter().map(|s| s * s).sum::<f32>() / buf.len().max(1) as f32).sqrt()
    }

    /// With the LFO stopped it is just a filter, and a low-pass has to cut what
    /// is above it and pass what is below.
    #[test]
    fn standing_still_it_is_a_filter() {
        let run = |probe: f32| {
            let mut f = AutoFilter::new(48000);
            f.rate_hz = 0.0;
            f.depth_oct = 0.0;
            f.env_oct = 0.0;
            f.set_freq(0.5); // ~630 Hz
            let mut buf = tone(probe, 48000, 8192);
            f.process_block(&mut buf, 48000);
            rms(&buf[8192..])
        };
        let low = run(100.0);
        let high = run(8000.0);
        assert!(low > 0.2, "100 Hz should pass a 630 Hz low-pass: {low}");
        assert!(high < low * 0.2, "8 kHz should not: {high} vs {low}");
    }

    /// The cutoff has to actually move: the same tone comes and goes as the
    /// filter sweeps over it.
    #[test]
    fn the_lfo_sweeps_the_cutoff() {
        let mut f = AutoFilter::new(48000);
        f.set_freq(0.35);
        f.depth_oct = 3.0;
        f.set_rate(0.75);
        f.env_oct = 0.0;
        let mut buf = tone(2000.0, 48000, 48000);
        f.process_block(&mut buf, 48000);
        // Look at the envelope of the output in 10 ms windows.
        let windows: Vec<f32> = buf.chunks(960).map(rms).collect();
        let lo = windows.iter().cloned().fold(f32::MAX, f32::min);
        let hi = windows.iter().cloned().fold(f32::MIN, f32::max);
        assert!(
            hi > lo * 4.0,
            "the sweep should open and close on a steady tone: {lo}..{hi}"
        );
    }

    /// Envelope tracking: a loud input opens the filter, a quiet one does not.
    #[test]
    fn the_input_level_moves_the_cutoff_too() {
        let run = |amp: f32| {
            let mut f = AutoFilter::new(48000);
            f.rate_hz = 0.0;
            f.depth_oct = 0.0;
            f.set_freq(0.2); // ~180 Hz, well below the probe
            f.env_oct = 4.0;
            let mut buf: Vec<f32> = tone(2000.0, 48000, 24000).iter().map(|s| s * amp).collect();
            f.process_block(&mut buf, 48000);
            // Normalise by the input level, so this measures the filter and
            // not the amplitude it was handed.
            rms(&buf[24000..]) / amp
        };
        let quiet = run(0.02);
        let loud = run(2.0);
        assert!(
            loud > quiet * 3.0,
            "a hotter input should open the filter: {loud} vs {quiet}"
        );
    }

    /// A spread puts the two channels at different cutoffs — which is the only
    /// reason each has its own coefficients.
    #[test]
    fn a_spread_takes_the_two_channels_apart() {
        let mut f = AutoFilter::new(48000);
        f.set_freq(0.4);
        f.depth_oct = 3.0;
        f.set_rate(0.6);
        f.spread = 0.5;
        f.env_oct = 0.0;
        let mut buf = tone(1500.0, 48000, 24000);
        f.process_block(&mut buf, 48000);
        let l: Vec<f32> = buf.iter().step_by(2).copied().collect();
        let r: Vec<f32> = buf.iter().skip(1).step_by(2).copied().collect();
        let biggest = l
            .iter()
            .zip(r.iter())
            .fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
        assert!(biggest > 0.05, "the channels stayed together: {biggest}");
    }

    #[test]
    fn it_survives_silence_extremes_shapes_and_a_rate_change() {
        for wave in Wave::ALL {
            for mode in FilterMode::ALL {
                let mut f = AutoFilter::new(48000);
                f.wave = wave;
                f.mode = mode;
                f.resonance = 0.98;
                f.set_rate(1.0);
                f.depth_oct = 4.0;
                let mut buf = vec![0.0f32; 512];
                f.process_block(&mut buf, 48000);
                assert!(buf.iter().all(|s| s.is_finite()));
                let mut hot = vec![8.0f32; 2048];
                f.process_block(&mut hot, 96000);
                assert!(
                    hot.iter().all(|s| s.is_finite()),
                    "{} / {} went non-finite",
                    wave.label(),
                    mode.label()
                );
                f.process_block(&mut [], 96000);
                f.process_block(&mut [1.0], 96000);
                f.reset();
            }
        }
    }
}
