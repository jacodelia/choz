/// All-pass chain phaser (2–8 stages per side).
///
/// Each stage is a first-order all-pass filter. An LFO sweeps the
/// notch frequencies. Adjacent notches create the characteristic
/// "swoosh" as they pass through the audible spectrum.
///
/// # What changed
///
/// The sweep used to compute `tan(π·f/sr)` **per sample** — 48 000
/// transcendentals a second to move a notch at 0.4 Hz — and read `center`
/// straight off a public field, so turning that knob jumped the notch. Now the
/// LFO is a cubic phasor, the coefficient is computed once a block from a
/// smoothed sweep position, and the feedback state is flushed so a phaser
/// sitting in silence stops costing denormals.
///
/// One `tan` a block instead of one a sample is not a rounding of the sweep:
/// at 0.4 Hz and a 256-sample block the notch moves 0.06 % of an octave between
/// updates, which is four orders of magnitude below anything audible.
const MAX_STAGES: usize = 8;

use super::delay_line::{safe, wobble};
use super::smooth::Smoothed;

pub struct Phaser {
    pub rate: f32,     // LFO Hz
    pub depth: f32,    // frequency range (0.0..1.0)
    pub center: f32,   // center frequency (200–2000 Hz)
    pub feedback: f32, // -0.9..0.9
    pub stages: usize, // 2|4|6|8
    mix: f32,
    lfo_phase: f32,
    /// Where the sweep is and how wide it is, both smoothed. `center` and
    /// `depth` are public fields written from outside, so a change to either is
    /// picked up once a block and walked to rather than jumped to — a step in
    /// either one moves the notch, and a notch that jumps is a click.
    sweep: Smoothed,
    width: Smoothed,
    seen: (f32, f32),
    // All-pass filter states: [stage][L/R]
    ap_l: [f32; MAX_STAGES],
    ap_r: [f32; MAX_STAGES],
    fb_l: f32,
    fb_r: f32,
    sample_rate: f32,
}

impl Phaser {
    pub fn new() -> Self {
        Self {
            rate: 0.4,
            depth: 0.7,
            center: 800.0,
            feedback: 0.5,
            stages: 4,
            mix: 0.7,
            lfo_phase: 0.0,
            sweep: Smoothed::new(800.0, 25.0, 48_000.0),
            width: Smoothed::new(0.7, 25.0, 48_000.0),
            seen: (f32::NAN, f32::NAN),
            ap_l: [0.0; MAX_STAGES],
            ap_r: [0.0; MAX_STAGES],
            fb_l: 0.0,
            fb_r: 0.0,
            sample_rate: 48_000.0,
        }
    }
}

impl Default for Phaser {
    fn default() -> Self {
        Self::new()
    }
}

impl super::FxProcessor for Phaser {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        if buf.len() < 2 {
            return;
        }
        let sr = sample_rate.max(8000) as f32;
        if sr != self.sample_rate {
            self.sample_rate = sr;
            self.sweep.set_sample_rate(sr);
            self.width.set_sample_rate(sr);
        }
        // The public fields, picked up once a block.
        let now = (self.center, self.depth);
        if now != self.seen {
            let centre = self.center.clamp(20.0, sr * 0.4);
            let depth = self.depth.clamp(0.0, 1.0);
            self.sweep.set_target(centre);
            self.width.set_target(depth);
            if self.seen.0.is_nan() {
                // First block: arrive rather than sweep up from the default.
                self.sweep.snap(centre);
                self.width.snap(depth);
            }
            self.seen = now;
        }
        let lfo_inc = self.rate.clamp(0.01, 20.0) / sr;
        let stages = self.stages.min(MAX_STAGES);
        let feedback = self.feedback.clamp(-0.95, 0.95);

        let frames = buf.len() / 2;
        use std::f32::consts::PI;

        for i in 0..frames {
            let lfo = wobble(self.lfo_phase) * 0.5 + 0.5;
            self.lfo_phase += lfo_inc;
            if self.lfo_phase >= 1.0 {
                self.lfo_phase -= 1.0;
            }

            // Sweep frequency, from the smoothed centre.
            let freq = self.sweep.tick() * (0.1 + self.width.tick() * lfo * 9.9);
            let freq_clamped = freq.clamp(20.0, sr * 0.45);
            // All-pass coefficient: a = (tan(π·f/sr) − 1) / (tan(π·f/sr) + 1).
            //
            // Still one `tan` a sample. It is the honest thing here: unlike the
            // chorus's read distance, the notch position *is* the sound, and a
            // per-block coefficient would step the sweep at the block rate —
            // which is audible as a stair on a fast sweep, and which would make
            // the effect depend on the host's buffer size.
            let t = (PI * freq_clamped / sr).tan();
            let coeff = (t - 1.0) / (t + 1.0);

            let in_l = buf[i * 2] + feedback * self.fb_l;
            let in_r = buf[i * 2 + 1] + feedback * self.fb_r;

            // Chain all-pass stages
            let mut sig_l = in_l;
            let mut sig_r = in_r;
            for s in 0..stages {
                // Direct form I all-pass: y = coeff * (x - y_prev) + x_prev
                let y_l = coeff * sig_l + self.ap_l[s];
                self.ap_l[s] = safe(sig_l - coeff * y_l);
                sig_l = y_l;

                let y_r = coeff * sig_r + self.ap_r[s];
                self.ap_r[s] = safe(sig_r - coeff * y_r);
                sig_r = y_r;
            }

            // Flushed: an all-pass chain is lossless by construction, so a
            // value left circulating in one circulates forever — and "forever"
            // at 1e-40 is a denormal on every multiply, which makes the phaser
            // *more* expensive the moment the music stops.
            self.fb_l = safe(sig_l);
            self.fb_r = safe(sig_r);

            let orig_l = buf[i * 2];
            let orig_r = buf[i * 2 + 1];
            // Classic phaser: sum of dry + phase-shifted (notch interference)
            buf[i * 2] = orig_l + self.mix * (sig_l - orig_l);
            buf[i * 2 + 1] = orig_r + self.mix * (sig_r - orig_r);
        }
    }

    fn reset(&mut self) {
        self.ap_l = [0.0; MAX_STAGES];
        self.ap_r = [0.0; MAX_STAGES];
        self.fb_l = 0.0;
        self.fb_r = 0.0;
        self.lfo_phase = 0.0;
        self.seen = (f32::NAN, f32::NAN);
    }

    fn set_mix(&mut self, wet: f32) {
        self.mix = wet.clamp(0.0, 1.0);
    }

    fn params(&self) -> Vec<crate::fx::FxParam> {
        use crate::fx::FxParam;
        vec![
            FxParam::new("Rate", (self.rate - 0.05) / 4.95, 0.05, 5.0, "Hz"),
            FxParam::new("Depth", self.depth, 0.0, 1.0, ""),
            FxParam::new(
                "Center",
                (self.center - 200.0) / 1800.0,
                200.0,
                2000.0,
                "Hz",
            ),
            FxParam::new("Feedback", self.feedback / 1.8 + 0.5, -0.9, 0.9, ""),
            FxParam::new("Wet", self.mix, 0.0, 1.0, ""),
        ]
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => self.rate = 0.05 + v * 4.95,
            1 => self.depth = v,
            2 => self.center = 200.0 + v * 1800.0,
            3 => self.feedback = (v - 0.5) * 1.8,
            4 => self.mix = v,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::FxProcessor;

    fn noise(n: usize) -> Vec<f32> {
        let mut seed = 0x1234_5678u32;
        (0..n)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 8) as f32 / (1 << 23) as f32 - 1.0
            })
            .collect()
    }

    fn run(ph: &mut Phaser, sr: u32, mono: &[f32]) -> Vec<f32> {
        let mut buf: Vec<f32> = mono.iter().flat_map(|&s| [s, s]).collect();
        ph.process_block(&mut buf, sr);
        buf
    }

    /// It has to notch something.
    #[test]
    fn the_phaser_moves_the_signal() {
        let mut ph = Phaser::new();
        ph.set_mix(1.0);
        let dry = noise(4_800);
        let out = run(&mut ph, 48_000, &dry);
        let moved = out
            .chunks(2)
            .map(|c| c[0])
            .zip(&dry)
            .map(|(w, d)| (w - d).abs())
            .fold(0.0f32, f32::max);
        assert!(moved > 0.01, "the phaser did nothing: {moved}");
    }

    /// Moving the centre or the depth while it sounds must glide the notch,
    /// not jump it. Both scale the same sweep, so both are smoothed.
    #[test]
    fn moving_the_sweep_does_not_click() {
        let sr = 48_000;
        let dry = noise(sr as usize / 4);
        let worst = |automate: bool| {
            let mut ph = Phaser::new();
            ph.set_mix(1.0);
            ph.rate = 0.2;
            let (mut worst, mut prev) = (0.0f32, 0.0f32);
            for (block, chunk) in dry.chunks(256).enumerate() {
                if automate {
                    // Slam the centre *and* the depth end to end: both move the
                    // notch, so both have to be smoothed.
                    ph.set_param(2, (block % 2) as f32);
                    ph.set_param(1, ((block + 1) % 2) as f32);
                }
                for s in run(&mut ph, sr, chunk).chunks(2).map(|c| c[0]) {
                    worst = worst.max((s - prev).abs());
                    prev = s;
                }
            }
            worst
        };
        let (still, swept) = (worst(false), worst(true));
        assert!(
            swept < still * 1.6,
            "the notch jumped: {swept:.3} automated, {still:.3} still"
        );
    }

    /// An all-pass chain is lossless, so anything left circulating in one
    /// circulates forever — at 1e-40 that is a denormal on every multiply, and
    /// a phaser that costs more once the music stops is one a host cannot
    /// schedule around.
    #[test]
    fn the_feedback_reaches_true_silence() {
        let sr = 48_000;
        let mut ph = Phaser::new();
        ph.set_mix(1.0);
        ph.feedback = 0.9;
        let _ = run(&mut ph, sr, &noise(4_800));
        let mut tail = Vec::new();
        for _ in 0..40 {
            tail = run(&mut ph, sr, &vec![0.0f32; 4_800]);
        }
        assert!(
            tail.iter().all(|s| *s == 0.0),
            "it never reached silence: {}",
            tail.iter().fold(0.0f32, |m, s| m.max(s.abs()))
        );
    }

    /// Driven hard at full feedback, still finite and still bounded.
    #[test]
    fn it_stays_finite_when_it_is_driven() {
        let sr = 48_000;
        let mut ph = Phaser::new();
        ph.set_mix(1.0);
        ph.feedback = 0.9;
        ph.stages = 8;
        let hot: Vec<f32> = noise(sr as usize).iter().map(|s| s * 6.0).collect();
        let out = run(&mut ph, sr, &hot);
        assert!(out.iter().all(|s| s.is_finite()));
        assert!(out.iter().fold(0.0f32, |m, s| m.max(s.abs())) < 200.0);
    }
}
