//! Build realtime FX processor chains from specs.

use crate::fx;

pub struct FxSpec {
    pub kind: String,
    pub enabled: bool,
    pub wet: f32,
    pub params: Vec<f32>,
}

pub fn build_processor(kind: &str, params: &[f32], sample_rate: u32) -> Option<Box<dyn fx::FxProcessor>> {
    let p = |i: usize| params.get(i).copied().unwrap_or(0.0);

    let proc: Box<dyn fx::FxProcessor> = match kind {
        "delay" => {
            let delay_ms = 10.0 + p(0) * 990.0;
            let feedback = p(1);
            let damping  = p(2);
            let mut d = fx::DelayLine::new(delay_ms, feedback, damping);
            d.set_ping_pong(p(3) > 0.5);
            Box::new(d)
        }
        "reverb" => {
            let mut r = fx::Reverb::new(sample_rate);
            r.set_room_size(p(0));
            r.set_damp(p(1));
            Box::new(r)
        }
        "grandelay" => Box::new(fx::GranularDelay::new(
            20.0 + p(0) * 980.0, p(1), (p(2) - 0.5) * 24.0, 1.0 + p(3) * 31.0,
        )),
        "compressor" => {
            let mut c = fx::Compressor::new();
            c.threshold_db = -(1.0 - p(0)) * 60.0;
            c.ratio        = 1.0 + p(1) * 19.0;
            c.attack_ms    = 0.1 + p(2) * 99.9;
            c.release_ms   = 10.0 + p(3) * 990.0;
            c.makeup_db    = p(4) * 24.0;
            c.knee_db      = p(5) * 12.0;
            Box::new(c)
        }
        "limiter" => {
            let mut lim = fx::Compressor::limiter();
            lim.threshold_db = -(1.0 - p(0)) * 12.0;
            lim.release_ms   = 1.0 + p(1) * 199.0;
            Box::new(lim)
        }
        "gate" => {
            let mut g = fx::Gate::new();
            g.threshold_db = -(1.0 - p(0)) * 80.0;
            g.attack_ms    = 0.1 + p(1) * 49.9;
            g.hold_ms      = 1.0 + p(2) * 499.0;
            g.release_ms   = 10.0 + p(3) * 990.0;
            g.floor_db     = -(1.0 - p(4)) * 80.0;
            Box::new(g)
        }
        "parameq" => {
            let mut eq = fx::ParametricEq::new();
            eq.bands[1].gain_db = (p(0) - 0.5) * 36.0;
            eq.bands[2].gain_db = (p(1) - 0.5) * 36.0;
            eq.bands[3].gain_db = (p(2) - 0.5) * 36.0;
            eq.bands[3].kind    = fx::EqBandKind::HighShelf;
            eq.bands[3].gain_db = (p(3) - 0.5) * 36.0;
            eq.bands[1].freq    = 20.0 * (800.0f32 / 20.0).powf(p(4));
            eq.bands[3].freq    = 1000.0 * 20.0f32.powf(p(5));
            eq.bands[2].q       = 0.1 + p(6) * 9.9;
            Box::new(eq)
        }
        "filter" => {
            let freq = 20.0 + p(0) * 19980.0;
            let res  = p(1) * 4.0 + 0.5;
            Box::new(fx::Svf::new(fx::SvfMode::Lowpass, freq, res))
        }
        "filterbank" => Box::new(fx::FilterBankFx::new(sample_rate)),
        "chorus" => {
            let mut c = fx::Chorus::new();
            c.rate     = 0.05 + p(0) * 4.95;
            c.depth    = 0.5  + p(1) * 9.5;
            c.delay_ms = 5.0  + p(2) * 25.0;
            c.feedback = (p(3) - 0.5) * 1.8;
            Box::new(c)
        }
        "flanger" => {
            let mut f = fx::Flanger::new();
            f.rate     = 0.05 + p(0) * 4.95;
            f.depth    = p(1) * 7.0;
            f.delay_ms = 0.5  + p(2) * 9.5;
            f.feedback = (p(3) - 0.5) * 1.9;
            Box::new(f)
        }
        "phaser" => {
            let mut ph = fx::Phaser::new();
            ph.rate     = 0.05 + p(0) * 4.95;
            ph.depth    = p(1);
            ph.center   = 200.0 + p(2) * 1800.0;
            ph.feedback = (p(3) - 0.5) * 1.8;
            Box::new(ph)
        }
        "bitcrusher" => {
            let mut b = fx::Bitcrusher::new();
            b.set_bits((1.0 + p(0) * 15.0) as u8);
            b.set_hold((1.0 + p(1) * 15.0) as u32);
            Box::new(b)
        }
        "vinyl" => {
            let mut v = fx::VinylSim::new();
            v.set_wow(p(0) * 0.1);
            v.set_flutter(p(1) * 0.05);
            v.set_crackle(p(2));
            Box::new(v)
        }
        "cassette" => Box::new(fx::Cassette::new()),
        "softclip" => {
            let mut s = fx::SoftClipper::new();
            s.drive = 1.0 + p(0) * 9.0;
            Box::new(s)
        }
        "tubesat" => {
            let mut t = fx::TubeSaturation::new();
            t.drive = 1.0 + p(0) * 19.0;
            t.tone  = p(1);
            Box::new(t)
        }
        "widener" => {
            let mut w = fx::StereoWidener::new();
            w.width = p(0) * 2.0;
            Box::new(w)
        }
        "isolator" => Box::new(fx::Isolator::new()),
        "gain" => {
            let mut g = fx::Gain::new();
            g.gain_db = (p(0) - 0.5) * 48.0;
            Box::new(g)
        }
        "phaseinvert" => Box::new(fx::PhaseInvert { invert_l: p(0) > 0.5, invert_r: p(1) > 0.5 }),
        "monomaker" => Box::new(fx::MonoMaker::new()),
        "looper" => Box::new(fx::Looper::new(sample_rate)),
        "sidechain" => Box::new(fx::SidechainDuck::new()),
        "expander" => {
            let mut exp = fx::Expander::new();
            exp.threshold_db = -(1.0 - p(0)) * 80.0;
            exp.ratio        = 1.0 + p(1) * 9.0;
            exp.attack_ms    = 0.1 + p(2) * 49.9;
            exp.release_ms   = 10.0 + p(3) * 990.0;
            exp.range_db     = p(4) * 80.0;
            Box::new(exp)
        }
        "pan" => {
            let mut pan = fx::Pan::new();
            pan.pan            = (p(0) - 0.5) * 2.0;
            pan.constant_power = p(1) > 0.5;
            Box::new(pan)
        }
        _ => return None,
    };
    Some(proc)
}

pub fn build_chain_from_specs(specs: &[FxSpec], sample_rate: u32) -> Vec<Box<dyn fx::FxProcessor>> {
    specs.iter()
        .filter(|s| s.enabled)
        .filter_map(|s| {
            let mut proc = build_processor(&s.kind, &s.params, sample_rate)?;
            proc.set_mix(s.wet);
            Some(proc)
        })
        .collect()
}
