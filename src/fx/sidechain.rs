use super::FxProcessor;

pub struct SidechainDuck {
    depth: f32,
    release_secs: f32,
    hold_secs: f32,
    rate_hz: f32,
    wet: f32,
    env: f32,
    lfo_phase: f32,
    hold_frames: usize,
    triggered: std::sync::atomic::AtomicBool,
    sample_rate: u32,
}

#[allow(dead_code)]
impl SidechainDuck {
    pub fn new() -> Self {
        Self {
            depth:        0.8,
            release_secs: 0.15,
            hold_secs:    0.0,
            rate_hz:      2.0,
            wet:          1.0,
            env:          1.0,
            lfo_phase:    0.0,
            hold_frames:  0,
            triggered:    std::sync::atomic::AtomicBool::new(false),
            sample_rate:  48000,
        }
    }

    pub fn set_depth(&mut self, d: f32) { self.depth = d.clamp(0.0, 1.0); }
    pub fn set_release(&mut self, secs: f32) { self.release_secs = secs.clamp(0.001, 4.0); }
    pub fn set_hold(&mut self, secs: f32) { self.hold_secs = secs.clamp(0.0, 1.0); }
    pub fn set_rate(&mut self, hz: f32) { self.rate_hz = hz.clamp(0.0, 20.0); }

    pub fn trigger(&self) {
        self.triggered.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    #[inline]
    fn release_coef(&self) -> f32 {
        (-1.0 / (self.release_secs * self.sample_rate as f32)).exp()
    }
}

impl Default for SidechainDuck { fn default() -> Self { Self::new() } }

impl FxProcessor for SidechainDuck {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        self.sample_rate = sample_rate;
        let sr = sample_rate as f32;
        let rel_coef = self.release_coef();
        let frames   = buf.len() / 2;

        let ext_trigger = self.triggered.swap(false, std::sync::atomic::Ordering::Relaxed);

        for i in 0..frames {
            let lfo_trigger = if self.rate_hz > 0.0 {
                let prev = self.lfo_phase;
                self.lfo_phase = (self.lfo_phase + self.rate_hz / sr).fract();
                prev > self.lfo_phase
            } else {
                false
            };

            let fire = lfo_trigger || (ext_trigger && i == 0);
            if fire {
                self.env = 1.0 - self.depth;
                self.hold_frames = (self.hold_secs * sr) as usize;
            }

            if self.hold_frames > 0 {
                self.hold_frames -= 1;
            } else {
                self.env = 1.0 - rel_coef * (1.0 - self.env);
                if self.env > 0.9999 { self.env = 1.0; }
            }

            let gain = 1.0 - self.wet * self.depth * (1.0 - self.env);
            buf[i * 2]     *= gain;
            buf[i * 2 + 1] *= gain;
        }
    }

    fn reset(&mut self) {
        self.env        = 1.0;
        self.lfo_phase  = 0.0;
        self.hold_frames = 0;
        self.triggered.store(false, std::sync::atomic::Ordering::Relaxed);
    }

    fn set_mix(&mut self, wet: f32) { self.wet = wet.clamp(0.0, 1.0); }
}
