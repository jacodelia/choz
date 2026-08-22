//! Feed-forward compressor and lookahead limiter.
//!
//! Both share the same gain-computer topology; `is_limiter = true` forces
//! ratio to infinity, snaps attack to 0.1 ms and delays the signal by the
//! lookahead window so the gain is already down when the peak arrives.

/// What the detector listens to.
///
/// Peak catches every transient; RMS follows loudness and lets short spikes
/// through; the short RMS sits in between — the setting that decides whether a
/// snare is a peak or part of the level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detect {
    Peak,
    Rms,
    RmsFast,
}

impl Detect {
    pub const ALL: [Detect; 3] = [Detect::Peak, Detect::Rms, Detect::RmsFast];

    pub fn label(self) -> &'static str {
        match self {
            Detect::Peak => "Peak",
            Detect::Rms => "RMS",
            Detect::RmsFast => "RMS fast",
        }
    }

    /// Averaging window of the RMS detector, in ms (unused for `Peak`).
    fn window_ms(self) -> f32 {
        match self {
            Detect::Peak => 0.0,
            Detect::Rms => 30.0,
            Detect::RmsFast => 3.0,
        }
    }

    pub fn to_norm(self) -> f32 {
        Self::ALL.iter().position(|d| *d == self).unwrap_or(0) as f32 / (Self::ALL.len() - 1) as f32
    }

    pub fn from_norm(v: f32) -> Self {
        let n = Self::ALL.len();
        let i = (v.clamp(0.0, 1.0) * (n - 1) as f32).round() as usize;
        Self::ALL[i.min(n - 1)]
    }
}

/// Lookahead ceiling: 10 ms at 192 kHz. Allocated once, by the limiter only.
const MAX_LOOKAHEAD_FRAMES: usize = 1920;

pub struct Compressor {
    pub threshold_db: f32, // -60..0
    pub ratio: f32,        // 1.0..100.0 (ignored when is_limiter=true)
    pub attack_ms: f32,
    pub release_ms: f32,
    pub makeup_db: f32, // 0..24
    pub knee_db: f32,   // 0..12  (soft knee width)
    pub detect: Detect,
    /// 0 = each channel compresses on its own, 1 = both follow the loudest.
    pub stereo_link: f32,
    /// High-pass on the detector only, in Hz. 20 = off (nothing to remove).
    pub sc_hpf_hz: f32,
    /// Lookahead, in ms. Only the limiter allocates the buffer it needs.
    pub lookahead_ms: f32,
    pub is_limiter: bool,
    mix: f32,
    // RT state
    env: [f32; 2],
    sq: [f32; 2],
    gain_smooth: [f32; 2],
    hpf_z: [f32; 2],
    /// Interleaved delay ring for the lookahead; empty when there is none.
    look_buf: Vec<f32>,
    look_pos: usize,
    look_frames: usize,
    sr: f32,
}

impl Compressor {
    pub fn new() -> Self {
        Self {
            threshold_db: -12.0,
            ratio: 4.0,
            attack_ms: 10.0,
            release_ms: 100.0,
            makeup_db: 0.0,
            knee_db: 6.0,
            detect: Detect::Peak,
            stereo_link: 1.0,
            sc_hpf_hz: 20.0,
            lookahead_ms: 0.0,
            is_limiter: false,
            mix: 1.0,
            env: [0.0; 2],
            sq: [0.0; 2],
            gain_smooth: [1.0; 2],
            hpf_z: [0.0; 2],
            look_buf: Vec::new(),
            look_pos: 0,
            look_frames: 0,
            sr: 44100.0,
        }
    }

    pub fn limiter(sample_rate: u32) -> Self {
        let mut c = Self::new();
        c.threshold_db = -0.3;
        c.ratio = 100.0;
        c.attack_ms = 0.1;
        c.release_ms = 50.0;
        c.knee_db = 0.0;
        c.detect = Detect::Peak;
        c.lookahead_ms = 2.0;
        c.is_limiter = true;
        c.look_buf = vec![0.0; MAX_LOOKAHEAD_FRAMES * 2];
        c.set_sample_rate(sample_rate as f32);
        c
    }

    fn set_sample_rate(&mut self, sr: f32) {
        self.sr = sr;
        let cap = self.look_buf.len() / 2;
        let want = (self.lookahead_ms * 0.001 * sr).round() as usize;
        self.look_frames = want.min(cap);
        self.look_pos = 0;
        self.look_buf.fill(0.0);
    }

    fn gain_reduction_db(&self, level_db: f32) -> f32 {
        let thr = self.threshold_db;
        let ratio = if self.is_limiter { 1000.0 } else { self.ratio };
        let knee = self.knee_db;

        if knee > 0.0 {
            let diff = level_db - thr;
            let half_knee = knee * 0.5;
            if diff < -half_knee {
                0.0
            } else if diff < half_knee {
                let t = (diff + half_knee) / knee;
                (1.0 / ratio - 1.0) * t * t * knee * 0.5
            } else {
                (level_db - thr) * (1.0 / ratio - 1.0)
            }
        } else if level_db > thr {
            (level_db - thr) * (1.0 / ratio - 1.0)
        } else {
            0.0
        }
    }
}

impl Default for Compressor {
    fn default() -> Self {
        Self::new()
    }
}

impl super::FxProcessor for Compressor {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        if buf.len() < 2 {
            return;
        }
        let sr = sample_rate as f32;
        // The lookahead window is in samples, so both a new sample rate and a
        // new setting have to re-measure it.
        let want = ((self.lookahead_ms * 0.001 * sr).round() as usize).min(self.look_buf.len() / 2);
        if sr != self.sr || want != self.look_frames {
            self.lookahead_ms = self.lookahead_ms.max(0.0);
            self.set_sample_rate(sr);
        }

        let attack_coeff = (-1.0 / (self.attack_ms.max(0.01) * 0.001 * sr)).exp();
        let release_coeff = (-1.0 / (self.release_ms.max(0.01) * 0.001 * sr)).exp();
        let makeup_linear = db_to_linear(self.makeup_db);
        // One-pole high-pass on the detector: above 20 Hz there is something to
        // remove, at 20 Hz there is not.
        let hpf_on = self.sc_hpf_hz > 20.5;
        let hpf_coeff = if hpf_on {
            let x = (-2.0 * std::f32::consts::PI * self.sc_hpf_hz / sr).exp();
            x.clamp(0.0, 0.9999)
        } else {
            0.0
        };
        let rms_coeff = if self.detect.window_ms() > 0.0 {
            (-1.0 / (self.detect.window_ms() * 0.001 * sr)).exp()
        } else {
            0.0
        };
        let link = self.stereo_link.clamp(0.0, 1.0);
        let look = self.look_frames;

        let frames = buf.len() / 2;
        for i in 0..frames {
            let inp = [buf[i * 2], buf[i * 2 + 1]];

            // ── Detector ──────────────────────────────────────────────────
            let mut det = [0.0f32; 2];
            for ch in 0..2 {
                let mut d = inp[ch];
                if hpf_on {
                    // y = x - lowpass(x)
                    self.hpf_z[ch] = d + hpf_coeff * (self.hpf_z[ch] - d);
                    d -= self.hpf_z[ch];
                }
                det[ch] = if rms_coeff > 0.0 {
                    self.sq[ch] = d * d + rms_coeff * (self.sq[ch] - d * d);
                    self.sq[ch].max(0.0).sqrt()
                } else {
                    d.abs()
                };
            }
            let loudest = det[0].max(det[1]);

            // ── Envelope + gain, per channel ──────────────────────────────
            let mut gain = [1.0f32; 2];
            for ch in 0..2 {
                let target_det = det[ch] + link * (loudest - det[ch]);
                let mut e = self.env[ch];
                if target_det > e {
                    e = attack_coeff * (e - target_det) + target_det;
                } else {
                    e = release_coeff * (e - target_det) + target_det;
                }
                self.env[ch] = e;
                let gr_db = self.gain_reduction_db(linear_to_db(e.max(1e-10)));
                let target_gain = db_to_linear(gr_db) * makeup_linear;
                let mut g = self.gain_smooth[ch];
                if target_gain < g {
                    g = attack_coeff * (g - target_gain) + target_gain;
                } else {
                    g = release_coeff * (g - target_gain) + target_gain;
                }
                self.gain_smooth[ch] = g;
                gain[ch] = g;
            }

            // ── Lookahead: the gain rides the sample the detector saw N
            //    frames ago, so the peak arrives with the gain already down.
            let dry = if look > 0 {
                let p = self.look_pos * 2;
                let out = [self.look_buf[p], self.look_buf[p + 1]];
                self.look_buf[p] = inp[0];
                self.look_buf[p + 1] = inp[1];
                self.look_pos = (self.look_pos + 1) % look;
                out
            } else {
                inp
            };

            for ch in 0..2 {
                let wet = dry[ch] * gain[ch];
                buf[i * 2 + ch] = dry[ch] + self.mix * (wet - dry[ch]);
            }
        }
    }

    fn reset(&mut self) {
        self.env = [0.0; 2];
        self.sq = [0.0; 2];
        self.gain_smooth = [1.0; 2];
        self.hpf_z = [0.0; 2];
        self.look_pos = 0;
        self.look_buf.fill(0.0);
    }

    fn set_mix(&mut self, wet: f32) {
        self.mix = wet.clamp(0.0, 1.0);
    }

    fn name(&self) -> &str {
        if self.is_limiter {
            "Limiter"
        } else {
            "Compressor"
        }
    }

    fn latency_samples(&self) -> u32 {
        self.look_frames as u32
    }

    fn params(&self) -> Vec<crate::fx::FxParam> {
        use crate::fx::FxParam;
        if self.is_limiter {
            return vec![
                FxParam::new(
                    "Thresh",
                    (self.threshold_db + 12.0) / 12.0,
                    -12.0,
                    0.0,
                    "dB",
                ),
                FxParam::new(
                    "Release",
                    (self.release_ms / 200.0).clamp(0.0, 1.0),
                    1.0,
                    200.0,
                    "ms",
                ),
                FxParam::new(
                    "Look",
                    (self.lookahead_ms / 10.0).clamp(0.0, 1.0),
                    0.0,
                    10.0,
                    "ms",
                ),
                FxParam::new("Link", self.stereo_link, 0.0, 1.0, ""),
                FxParam::new("Wet", self.mix, 0.0, 1.0, ""),
            ];
        }
        vec![
            FxParam::new(
                "Threshold",
                (self.threshold_db + 60.0) / 60.0,
                -60.0,
                0.0,
                "dB",
            ),
            FxParam::new(
                "Ratio",
                ((self.ratio - 1.0) / 99.0).clamp(0.0, 1.0),
                1.0,
                100.0,
                ":1",
            ),
            FxParam::new(
                "Attack",
                (self.attack_ms / 200.0).clamp(0.0, 1.0),
                0.0,
                200.0,
                "ms",
            ),
            FxParam::new(
                "Release",
                (self.release_ms / 2000.0).clamp(0.0, 1.0),
                0.0,
                2000.0,
                "ms",
            ),
            FxParam::new(
                "Makeup",
                (self.makeup_db / 24.0).clamp(0.0, 1.0),
                0.0,
                24.0,
                "dB",
            ),
            FxParam::new(
                "Knee",
                (self.knee_db / 12.0).clamp(0.0, 1.0),
                0.0,
                12.0,
                "dB",
            ),
            FxParam::new("Detect", self.detect.to_norm(), 0.0, 1.0, ""),
            FxParam::new("Link", self.stereo_link, 0.0, 1.0, ""),
            FxParam::new(
                "SC HPF",
                ((self.sc_hpf_hz - 20.0) / 480.0).clamp(0.0, 1.0),
                20.0,
                500.0,
                "Hz",
            ),
            FxParam::new("Wet", self.mix, 0.0, 1.0, ""),
        ]
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        if self.is_limiter {
            match index {
                0 => self.threshold_db = -12.0 + v * 12.0,
                1 => self.release_ms = 1.0 + v * 199.0,
                2 => self.lookahead_ms = v * 10.0,
                3 => self.stereo_link = v,
                4 => self.mix = v,
                _ => {}
            }
            return;
        }
        match index {
            0 => self.threshold_db = -60.0 + v * 60.0,
            1 => self.ratio = 1.0 + v * 19.0,
            2 => self.attack_ms = 0.1 + v * 99.9,
            3 => self.release_ms = 10.0 + v * 990.0,
            4 => self.makeup_db = v * 24.0,
            5 => self.knee_db = v * 12.0,
            6 => self.detect = Detect::from_norm(v),
            7 => self.stereo_link = v,
            8 => self.sc_hpf_hz = 20.0 + v * 480.0,
            9 => self.mix = v,
            _ => {}
        }
    }
}

#[inline]
fn db_to_linear(db: f32) -> f32 {
    10.0f32.powf(db / 20.0)
}
#[inline]
fn linear_to_db(lin: f32) -> f32 {
    20.0 * lin.max(1e-10).log10()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::FxProcessor;

    #[test]
    fn unity_gain_below_threshold() {
        let mut c = Compressor::new();
        c.threshold_db = -6.0;
        c.ratio = 4.0;
        c.attack_ms = 0.0;
        c.release_ms = 0.0;
        c.makeup_db = 0.0;
        c.knee_db = 0.0;
        // Signal well below threshold: gain reduction should be ~0.
        let mut buf = vec![0.01f32; 256]; // 256 interleaved samples (128 frames)
        let before = buf[0];
        c.process_block(&mut buf, 48000);
        // Output should be close to input (minimal compression below threshold).
        assert!(
            (buf[0] - before).abs() < 0.01,
            "expected ~unity below threshold"
        );
    }

    #[test]
    fn gain_reduction_above_threshold() {
        let mut c = Compressor::new();
        c.threshold_db = -20.0;
        c.ratio = 10.0;
        c.attack_ms = 0.1;
        c.release_ms = 10.0;
        c.makeup_db = 0.0;
        c.knee_db = 0.0;
        // Hot signal well above threshold.
        let mut buf = vec![0.5f32; 1024];
        c.process_block(&mut buf, 48000);
        // After enough frames the gain should be significantly reduced.
        let last = buf[1020].abs();
        assert!(last < 0.45, "expected gain reduction, got {}", last);
    }

    #[test]
    fn limiter_prevents_overshoot() {
        let mut lim = Compressor::limiter(48000);
        let mut buf = vec![2.0f32; 1024];
        lim.process_block(&mut buf, 48000);
        // Limiter should bring the signal down to ~threshold.
        let last = buf[1020].abs();
        assert!(last < 1.2, "limiter should prevent overshoot, got {}", last);
    }

    /// The point of lookahead: a peak that arrives out of silence is already
    /// held down, instead of being let through while the envelope catches up.
    #[test]
    fn lookahead_catches_the_first_peak() {
        let mut with = Compressor::limiter(48000);
        let mut without = Compressor::limiter(48000);
        without.lookahead_ms = 0.0;
        // Silence, then a sudden full-scale burst.
        let mut a = vec![0.0f32; 4096];
        for s in a.iter_mut().skip(1024) {
            *s = 1.0;
        }
        let mut b = a.clone();
        with.process_block(&mut a, 48000);
        without.process_block(&mut b, 48000);
        // The first sample of the burst, delayed by the lookahead window.
        let look = with.latency_samples() as usize;
        assert!(look > 0, "the limiter must report its lookahead");
        let peak_with = a[(1024 + look * 2)..(1024 + look * 2 + 8)]
            .iter()
            .fold(0.0f32, |m, s| m.max(s.abs()));
        let peak_without = b[1024..1032].iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(
            peak_with < peak_without,
            "lookahead should tame the attack: {peak_with} vs {peak_without}"
        );
    }

    #[test]
    fn rms_lets_a_short_spike_through_that_peak_catches() {
        let spike = |c: &mut Compressor| {
            let mut buf = vec![0.05f32; 2048];
            // One short burst, ~1 ms.
            for s in buf.iter_mut().skip(1024).take(96) {
                *s = 1.0;
            }
            c.process_block(&mut buf, 48000);
            buf[1024..1120].iter().fold(0.0f32, |m, s| m.max(s.abs()))
        };
        let mut peak = Compressor::new();
        peak.threshold_db = -20.0;
        peak.ratio = 20.0;
        peak.attack_ms = 0.1;
        peak.detect = Detect::Peak;
        let mut rms = Compressor::new();
        rms.threshold_db = -20.0;
        rms.ratio = 20.0;
        rms.attack_ms = 0.1;
        rms.detect = Detect::Rms;

        let p = spike(&mut peak);
        let r = spike(&mut rms);
        assert!(
            r > p,
            "RMS should let more of a 1 ms spike through: {r} vs {p}"
        );
    }

    /// Unlinked, a loud left channel does not pull the right one down: that is
    /// the whole difference, and it is what moves the image when it is wrong.
    #[test]
    fn stereo_link_decides_whether_one_channel_ducks_the_other() {
        let run = |link: f32| {
            let mut c = Compressor::new();
            c.threshold_db = -30.0;
            c.ratio = 20.0;
            c.attack_ms = 0.1;
            c.release_ms = 50.0;
            c.stereo_link = link;
            let mut buf = vec![0.0f32; 4096];
            for i in 0..2048 {
                buf[i * 2] = 0.9; // hot left
                buf[i * 2 + 1] = 0.02; // quiet right
            }
            c.process_block(&mut buf, 48000);
            buf[4095].abs() // right channel, last frame
        };
        let linked = run(1.0);
        let dual = run(0.0);
        assert!(
            dual > linked * 2.0,
            "unlinked right should survive: dual={dual} linked={linked}"
        );
    }

    /// A kick under the sidechain HPF stops driving the gain.
    #[test]
    fn sidechain_hpf_deafens_the_detector_to_bass() {
        let run = |hpf: f32| {
            let mut c = Compressor::new();
            c.threshold_db = -30.0;
            c.ratio = 20.0;
            c.attack_ms = 1.0;
            c.release_ms = 100.0;
            c.sc_hpf_hz = hpf;
            // 40 Hz sine, well above threshold.
            let frames = 8192;
            let mut buf = vec![0.0f32; frames * 2];
            for i in 0..frames {
                let s = 0.8 * (2.0 * std::f32::consts::PI * 40.0 * i as f32 / 48000.0).sin();
                buf[i * 2] = s;
                buf[i * 2 + 1] = s;
            }
            c.process_block(&mut buf, 48000);
            buf[(frames * 2 - 512)..]
                .iter()
                .fold(0.0f32, |m, s| m.max(s.abs()))
        };
        let off = run(20.0);
        let on = run(500.0);
        assert!(
            on > off * 1.5,
            "with the detector high-passed the bass should pass: on={on} off={off}"
        );
    }

    #[test]
    fn survives_silence_extremes_and_sample_rate_changes() {
        let mut c = Compressor::limiter(48000);
        let mut buf = vec![0.0f32; 512];
        c.process_block(&mut buf, 48000);
        c.process_block(&mut buf, 96000);
        c.set_param(2, 1.0); // longest lookahead
        let mut hot = vec![4.0f32; 512];
        c.process_block(&mut hot, 96000);
        assert!(hot.iter().all(|s| s.is_finite()));
        c.process_block(&mut [], 96000);
        c.reset();
    }
}
