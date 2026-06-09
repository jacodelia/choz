use super::FxProcessor;

const NUM_COMBS: usize = 8;
const NUM_ALLPASS: usize = 4;

const COMB_TUNINGS_44K: [usize; NUM_COMBS] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASS_TUNINGS_44K: [usize; NUM_ALLPASS] = [556, 441, 341, 225];
const STEREO_SPREAD: usize = 23;

struct CombFilter {
    buf: Vec<f32>,
    pos: usize,
    feedback: f32,
    damp1: f32,
    damp2: f32,
    filter_store: f32,
}

impl CombFilter {
    fn new(size: usize) -> Self {
        Self {
            buf: vec![0.0; size],
            pos: 0,
            feedback: 0.84,
            damp1: 0.2,
            damp2: 0.8,
            filter_store: 0.0,
        }
    }

    #[inline]
    fn process(&mut self, input: f32) -> f32 {
        let cap = self.buf.len();
        let output = self.buf[self.pos];
        self.filter_store = output * self.damp2 + self.filter_store * self.damp1;
        self.buf[self.pos] = input + self.filter_store * self.feedback;
        self.pos = (self.pos + 1) % cap;
        output
    }
}

struct AllpassFilter {
    buf: Vec<f32>,
    pos: usize,
}

impl AllpassFilter {
    fn new(size: usize) -> Self {
        Self { buf: vec![0.0; size], pos: 0 }
    }

    #[inline]
    fn process(&mut self, input: f32) -> f32 {
        let cap = self.buf.len();
        let buffered = self.buf[self.pos];
        self.buf[self.pos] = input + buffered * 0.5;
        self.pos = (self.pos + 1) % cap;
        -input + buffered
    }
}

pub struct Reverb {
    combs_l: [CombFilter; NUM_COMBS],
    combs_r: [CombFilter; NUM_COMBS],
    allpass_l: [AllpassFilter; NUM_ALLPASS],
    allpass_r: [AllpassFilter; NUM_ALLPASS],
    room_size: f32,
    damp: f32,
    wet: f32,
}

fn scale_delay(tuning: usize, sr: u32) -> usize {
    ((tuning as f64 * sr as f64 / 44100.0) as usize).max(4)
}

fn make_combs(sr: u32) -> [CombFilter; NUM_COMBS] {
    let s: Vec<CombFilter> = COMB_TUNINGS_44K.iter()
        .map(|&t| CombFilter::new(scale_delay(t, sr)))
        .collect();
    s.try_into().ok().unwrap()
}

fn make_combs_r(sr: u32) -> [CombFilter; NUM_COMBS] {
    let s: Vec<CombFilter> = COMB_TUNINGS_44K.iter()
        .map(|&t| CombFilter::new(scale_delay(t + STEREO_SPREAD, sr)))
        .collect();
    s.try_into().ok().unwrap()
}

fn make_allpass(sr: u32) -> [AllpassFilter; NUM_ALLPASS] {
    let s: Vec<AllpassFilter> = ALLPASS_TUNINGS_44K.iter()
        .map(|&t| AllpassFilter::new(scale_delay(t, sr)))
        .collect();
    s.try_into().ok().unwrap()
}

fn make_allpass_r(sr: u32) -> [AllpassFilter; NUM_ALLPASS] {
    let s: Vec<AllpassFilter> = ALLPASS_TUNINGS_44K.iter()
        .map(|&t| AllpassFilter::new(scale_delay(t + STEREO_SPREAD, sr)))
        .collect();
    s.try_into().ok().unwrap()
}

impl Reverb {
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate.max(8000);
        let mut r = Self {
            combs_l:   make_combs(sr),
            combs_r:   make_combs_r(sr),
            allpass_l: make_allpass(sr),
            allpass_r: make_allpass_r(sr),
            room_size: 0.5,
            damp: 0.5,
            wet: 0.3,
        };
        r.update_params();
        r
    }

    pub fn set_room_size(&mut self, size: f32) {
        self.room_size = size.clamp(0.0, 1.0);
        self.update_params();
    }

    pub fn set_damp(&mut self, d: f32) {
        self.damp = d.clamp(0.0, 1.0);
        self.update_params();
    }

    fn update_params(&mut self) {
        let feedback = self.room_size * 0.28 + 0.7;
        let damp1 = self.damp * 0.4;
        let damp2 = 1.0 - damp1;
        for c in self.combs_l.iter_mut().chain(self.combs_r.iter_mut()) {
            c.feedback = feedback;
            c.damp1    = damp1;
            c.damp2    = damp2;
        }
    }

    #[inline]
    fn process_channel(
        combs: &mut [CombFilter; NUM_COMBS],
        allpass: &mut [AllpassFilter; NUM_ALLPASS],
        input: f32,
    ) -> f32 {
        let mut out = 0.0_f32;
        for c in combs.iter_mut() {
            out += c.process(input);
        }
        let mut x = out;
        for ap in allpass.iter_mut() {
            x = ap.process(x);
        }
        x
    }
}

impl FxProcessor for Reverb {
    fn process_block(&mut self, buf: &mut [f32], _sample_rate: u32) {
        let frames = buf.len() / 2;
        for i in 0..frames {
            let dry_l = buf[i * 2];
            let dry_r = buf[i * 2 + 1];
            let input = (dry_l + dry_r) * 0.015;
            let wet_l = Self::process_channel(&mut self.combs_l, &mut self.allpass_l, input);
            let wet_r = Self::process_channel(&mut self.combs_r, &mut self.allpass_r, input);
            buf[i * 2]     = dry_l + self.wet * wet_l;
            buf[i * 2 + 1] = dry_r + self.wet * wet_r;
        }
    }

    fn reset(&mut self) {
        for c in self.combs_l.iter_mut().chain(self.combs_r.iter_mut()) {
            c.buf.fill(0.0);
            c.filter_store = 0.0;
            c.pos = 0;
        }
        for ap in self.allpass_l.iter_mut().chain(self.allpass_r.iter_mut()) {
            ap.buf.fill(0.0);
            ap.pos = 0;
        }
    }

    fn set_mix(&mut self, wet: f32) { self.wet = wet.clamp(0.0, 1.0); }
    fn name(&self) -> &str { "Reverb" }

    fn params(&self) -> Vec<crate::fx::FxParam> {
        use crate::fx::FxParam;
        vec![
            FxParam::new("Room",    self.room_size, 0.0, 1.0, ""),
            FxParam::new("Damping", self.damp, 0.0, 1.0, ""),
            FxParam::new("Wet",     self.wet, 0.0, 1.0, ""),
        ]
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match index {
            0 => self.set_room_size(v),
            1 => { self.damp = v; self.update_params(); }
            2 => {} // width — not applied
            3 => self.wet = v,
            _ => {}
        }
    }
}
