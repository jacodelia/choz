//! Plate reverb: the Dattorro tank.
//!
//! choz's [`super::reverb`] is a room — early reflections, a size, walls. A
//! plate is not a room: it is a sheet of steel under tension with a driver at
//! one end and pickups at the other, and what it does to a snare or a vocal is
//! the reason it survived the room simulations that were supposed to replace
//! it. Dense from the first millisecond, no early reflections at all, and a
//! bright metallic tail that no room has.
//!
//! The structure is Jon Dattorro's (JAES 1997): an input diffuser of four
//! all-passes, then a figure-of-eight tank of two halves, each a pair of
//! all-passes around a delay with a damping filter, cross-coupled so energy
//! circulates. The outputs are taps at fixed points of both halves — that is
//! what makes the two channels different without a stereo input.
//!
//! Delay lengths are Dattorro's, in samples at 29.761 kHz, scaled to whatever
//! rate the device runs at.

use super::delay_line::DelayLine as Line;
use choz_ports::{FxParam, FxProcessor};

/// The rate Dattorro's numbers are in samples at.
const REF_SR: f32 = 29_761.0;

/// A Schroeder all-pass: a delay with its own output fed back and forwards.
struct Allpass {
    line: Line,
    delay: f32,
    gain: f32,
}

impl Allpass {
    fn new(samples: f32, gain: f32) -> Self {
        Self {
            line: Line::with_samples(samples.ceil() as usize + 4),
            delay: samples,
            gain,
        }
    }

    fn set_delay(&mut self, samples: f32) {
        self.delay = samples.clamp(1.0, self.line.capacity() as f32 - 4.0);
    }

    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let delayed = self.line.read(self.delay);
        let v = x + delayed * -self.gain;
        self.line.write(v);
        delayed + v * self.gain
    }

    fn clear(&mut self) {
        self.line.clear();
    }
}

/// A plain delay with a one-pole damping filter on its output.
struct Damped {
    line: Line,
    delay: f32,
    z: f32,
}

impl Damped {
    fn new(samples: f32) -> Self {
        Self {
            line: Line::with_samples(samples.ceil() as usize + 4),
            delay: samples,
            z: 0.0,
        }
    }

    fn set_delay(&mut self, samples: f32) {
        self.delay = samples.clamp(1.0, self.line.capacity() as f32 - 4.0);
    }

    #[inline]
    fn tick(&mut self, x: f32, damp: f32) -> f32 {
        self.line.write(x);
        let out = self.line.read(self.delay);
        self.z = out + damp * (self.z - out);
        self.z
    }

    fn clear(&mut self) {
        self.line.clear();
        self.z = 0.0;
    }
}

/// Dattorro's lengths, in samples at [`REF_SR`].
const IN_AP: [f32; 4] = [142.0, 107.0, 379.0, 277.0];
const TANK_AP: [f32; 4] = [672.0, 1800.0, 908.0, 2656.0];
const TANK_DELAY: [f32; 4] = [4453.0, 3720.0, 4217.0, 3163.0];
/// Where the output is tapped, as `(which tank delay, how far back)` in samples
/// at [`REF_SR`].
///
/// The **first** delay of each half, not the second: the second only starts
/// carrying anything once the first has run out, which is 150 ms at 48 kHz —
/// tapped there the plate was silent for a sixth of a second and then arrived
/// all at once, which is the one thing a plate never does.
const TAPS_L: [(usize, f32); 3] = [(0, 266.0), (0, 2974.0), (2, 1913.0)];
const TAPS_R: [(usize, f32); 3] = [(2, 353.0), (2, 3627.0), (0, 1228.0)];

pub struct PlateReverb {
    /// Pre-delay: the only thing between the source and a tank with no early
    /// reflections in it.
    pre: Line,
    pre_ms: f32,
    input: [Allpass; 4],
    tank_ap: [Allpass; 4],
    tank_delay: [Damped; 4],
    /// How much of the tank goes back round: the decay time, in the only terms
    /// a feedback loop has.
    decay: f32,
    damp: f32,
    /// Bandwidth into the tank: a plate driven through a dull amplifier.
    bandwidth: f32,
    bw_z: [f32; 2],
    wet: f32,
    /// The two halves' last outputs, which is what the other half is fed.
    cross: [f32; 2],
    sample_rate: f32,
}

impl PlateReverb {
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate.max(8000) as f32;
        let k = sr / REF_SR;
        let mut p = Self {
            pre: Line::with_ms(200.0),
            pre_ms: 10.0,
            input: [
                Allpass::new(IN_AP[0] * k, 0.75),
                Allpass::new(IN_AP[1] * k, 0.75),
                Allpass::new(IN_AP[2] * k, 0.625),
                Allpass::new(IN_AP[3] * k, 0.625),
            ],
            tank_ap: [
                Allpass::new(TANK_AP[0] * k, 0.7),
                Allpass::new(TANK_AP[1] * k, 0.5),
                Allpass::new(TANK_AP[2] * k, 0.7),
                Allpass::new(TANK_AP[3] * k, 0.5),
            ],
            tank_delay: [
                Damped::new(TANK_DELAY[0] * k),
                Damped::new(TANK_DELAY[1] * k),
                Damped::new(TANK_DELAY[2] * k),
                Damped::new(TANK_DELAY[3] * k),
            ],
            decay: 0.65,
            damp: 0.35,
            bandwidth: 0.85,
            bw_z: [0.0; 2],
            wet: 0.3,
            cross: [0.0; 2],
            sample_rate: sr,
        };
        p.retune(sr);
        p
    }

    pub fn with_params(sample_rate: u32, params: &[f32]) -> Self {
        let mut p = Self::new(sample_rate);
        for (i, v) in params.iter().enumerate() {
            <Self as FxProcessor>::set_param(&mut p, i, *v);
        }
        p
    }

    /// Put every length back where it belongs for `sr`. The lines are built for
    /// the rate the plate was created at; a device that opens faster keeps the
    /// same *sound* only if the numbers move with it.
    fn retune(&mut self, sr: f32) {
        let k = (sr / REF_SR).min(self.headroom());
        for (i, ap) in self.input.iter_mut().enumerate() {
            ap.set_delay(IN_AP[i] * k);
        }
        for (i, ap) in self.tank_ap.iter_mut().enumerate() {
            ap.set_delay(TANK_AP[i] * k);
        }
        for (i, d) in self.tank_delay.iter_mut().enumerate() {
            d.set_delay(TANK_DELAY[i] * k);
        }
        self.sample_rate = sr;
    }

    /// How far the lines allocated at build time can be stretched. A plate
    /// built at 48 kHz and run at 192 cannot grow its buffers on the audio
    /// thread, so the tank shortens instead of allocating.
    fn headroom(&self) -> f32 {
        let longest = self.tank_delay[0].line.capacity() as f32 - 8.0;
        longest / TANK_DELAY[0]
    }
}

impl FxProcessor for PlateReverb {
    fn name(&self) -> &str {
        "Plate Reverb"
    }

    fn params(&self) -> Vec<FxParam> {
        vec![
            FxParam::new("PreDelay", self.pre_ms / 200.0, 0.0, 200.0, "ms"),
            FxParam::new("Decay", self.decay, 0.0, 1.0, ""),
            FxParam::new("Damping", self.damp, 0.0, 1.0, ""),
            FxParam::new("Tone", self.bandwidth, 0.0, 1.0, ""),
            FxParam::new("Wet", self.wet, 0.0, 1.0, ""),
        ]
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => self.pre_ms = v * 200.0,
            // Stops short of 1: at 1 the tank never loses anything and the
            // plate is an oscillator with a long memory.
            1 => self.decay = v * 0.98,
            2 => self.damp = v,
            3 => self.bandwidth = v,
            4 => self.wet = v,
            _ => {}
        }
    }

    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        let sr = sample_rate.max(8000) as f32;
        if (sr - self.sample_rate).abs() > f32::EPSILON {
            self.retune(sr);
        }
        let k = (sr / REF_SR).min(self.headroom());
        let pre = (self.pre_ms * 0.001 * sr).clamp(1.0, self.pre.capacity() as f32 - 4.0);
        // Dattorro's "excursion" is left out: the tank modulation is what stops
        // a plate ringing on one note, and it costs an interpolated read per
        // sample per all-pass. The damping does the same job for a fraction of
        // it — this is a plate, not a museum piece.
        for frame in buf.as_chunks_mut::<2>().0 {
            let dry = [frame[0], frame[1]];
            let mono = (dry[0] + dry[1]) * 0.5;
            self.pre.write(mono);
            let mut x = self.pre.read(pre);
            // Bandwidth: one pole, on the way in.
            self.bw_z[0] = x + (1.0 - self.bandwidth) * (self.bw_z[0] - x);
            x = self.bw_z[0];
            for ap in &mut self.input {
                x = ap.tick(x);
            }
            // The figure of eight: each half takes the input plus what came out
            // of the other one.
            let mut a = self.tank_ap[0].tick(x + self.cross[1] * self.decay);
            a = self.tank_delay[0].tick(a, self.damp);
            a = self.tank_ap[1].tick(a * self.decay);
            let out_a = self.tank_delay[1].tick(a, self.damp);

            let mut b = self.tank_ap[2].tick(x + self.cross[0] * self.decay);
            b = self.tank_delay[2].tick(b, self.damp);
            b = self.tank_ap[3].tick(b * self.decay);
            let out_b = self.tank_delay[3].tick(b, self.damp);

            self.cross = [out_a * self.decay, out_b * self.decay];

            // The taps, which is where the two channels come from.
            let read = |i: usize, at: f32, delays: &[Damped; 4]| -> f32 {
                delays[i]
                    .line
                    .read(at.min(delays[i].line.capacity() as f32 - 4.0))
            };
            let mut l = 0.0;
            for (i, at) in TAPS_L {
                l += read(i, at * k, &self.tank_delay);
            }
            let mut r = 0.0;
            for (i, at) in TAPS_R {
                r += read(i, at * k, &self.tank_delay);
            }
            let wet = [l * 0.5, r * 0.5];
            for ch in 0..2 {
                frame[ch] = dry[ch] + self.wet * (wet[ch] - dry[ch]);
            }
        }
    }

    fn reset(&mut self) {
        self.pre.clear();
        for ap in self.input.iter_mut().chain(self.tank_ap.iter_mut()) {
            ap.clear();
        }
        for d in &mut self.tank_delay {
            d.clear();
        }
        self.cross = [0.0; 2];
        self.bw_z = [0.0; 2];
    }

    fn set_mix(&mut self, wet: f32) {
        self.wet = wet.clamp(0.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A click in has to come back as a tail that lasts, is dense from the
    /// start (no gap where a room's early reflections would be), and is not the
    /// same in both channels — a mono plate is a plate nobody would use.
    #[test]
    fn a_click_becomes_a_dense_stereo_tail() {
        let sr = 48_000u32;
        let mut fx = PlateReverb::new(sr);
        fx.set_param(0, 0.0); // no pre-delay
        fx.set_param(1, 0.8); // a long tail
        fx.set_param(4, 1.0); // fully wet

        let mut buf = vec![0.0f32; sr as usize * 2]; // one second, stereo
        buf[0] = 1.0;
        buf[1] = 1.0;
        fx.process_block(&mut buf, sr);

        let energy = |from_ms: f32, to_ms: f32| {
            let a = (from_ms * 0.001 * sr as f32) as usize * 2;
            let b = (to_ms * 0.001 * sr as f32) as usize * 2;
            buf[a..b].iter().map(|s| s * s).sum::<f32>()
        };
        assert!(energy(5.0, 20.0) > 1e-4, "the tank is not filling");
        assert!(
            energy(300.0, 400.0) > 1e-5,
            "the tail died in a third of a second"
        );

        let diff: f32 = buf[..sr as usize]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|f| (f[0] - f[1]).abs())
            .sum();
        assert!(diff > 1.0, "both channels came out the same: {diff}");
        assert!(
            buf.iter().all(|s| s.is_finite() && s.abs() < 8.0),
            "the tank ran away"
        );
    }

    /// The same plate at a different device rate keeps its decay: the lengths
    /// are in seconds, not in samples.
    #[test]
    fn the_tail_is_the_same_length_at_any_rate() {
        let tail_at = |sr: u32| {
            let mut fx = PlateReverb::new(sr);
            fx.set_param(1, 0.8);
            fx.set_param(4, 1.0);
            let mut buf = vec![0.0f32; sr as usize * 2];
            buf[0] = 1.0;
            buf[1] = 1.0;
            fx.process_block(&mut buf, sr);
            // Where the energy has fallen to a thousandth of the first 50 ms.
            let win = (sr as usize / 100) * 2; // 10 ms windows
            let first: f32 = buf[..win * 5].iter().map(|s| s * s).sum();
            let mut last = 0;
            for (i, w) in buf.chunks(win).enumerate() {
                if w.iter().map(|s| s * s).sum::<f32>() > first * 1e-3 {
                    last = i;
                }
            }
            last as f32 * 10.0 // ms
        };
        let a = tail_at(44_100);
        let b = tail_at(96_000);
        assert!(
            (a - b).abs() < a.max(b) * 0.25,
            "the tail is {a} ms at 44.1k and {b} ms at 96k"
        );
    }
}
