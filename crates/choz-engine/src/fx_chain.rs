//! Build realtime FX processor chains from specs.

use crate::fx;

pub struct FxSpec {
    pub kind: String,
    pub enabled: bool,
    pub wet: f32,
    pub params: Vec<f32>,
    /// Set for CLAP audio effects: the `.clap` file and plugin id to host in
    /// this slot instead of a built-in FX. `kind` is then ignored.
    pub plugin: Option<ClapFxRef>,
}

/// Which CLAP plugin an FX slot hosts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClapFxRef {
    pub path: std::path::PathBuf,
    pub id: String,
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
            // Map 0..1 into the filter's 0..~0.98 resonance range (1.0 self-oscillates).
            let res  = p(1) * 0.98;
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
        // Creative time/texture FX imported from seqterm: these take their
        // parameters normalised, in the same order `params()` reports them.
        "protocosmos" => Box::new(fx::Protocosmos::new(
            sample_rate, p(0), p(1), p(2), p(3), p(4), p(5), p(6),
        )),
        "spaceecho" => Box::new(fx::SpaceEcho::new(
            sample_rate, p(0), p(1), p(2), p(3), p(4), p(5), p(6),
        )),
        "reversedelay" => Box::new(fx::ReverseDelay::new(sample_rate, p(0), p(1))),
        // Stompbox distortions: knobs are normalised, in `params()` order.
        "amberfang" => {
            let mut d = fx::AmberFang::new(sample_rate);
            d.dist = p(0);
            d.tone = p(1);
            d.level = p(2);
            Box::new(d)
        }
        "velvetfuzz" => {
            let mut d = fx::VelvetFuzz::new(sample_rate);
            d.sustain = p(0);
            d.tone = p(1);
            d.level = p(2);
            Box::new(d)
        }
        "z5texture" => Box::new(fx::Z5Texture::with_params(sample_rate, params)),
        _ => return None,
    };
    Some(proc)
}

/// Build a CLAP audio effect, when the `clap` feature is on. Loading happens
/// here (UI thread), never on the RT thread.
#[cfg(feature = "clap")]
fn build_clap_fx(r: &ClapFxRef, sample_rate: u32, max_block: u32) -> Option<Box<dyn fx::FxProcessor>> {
    let eff = choz_plugin_clap::host::ClapEffect::build(&r.path, &r.id, sample_rate, max_block)?;
    Some(Box::new(eff))
}

#[cfg(not(feature = "clap"))]
fn build_clap_fx(_r: &ClapFxRef, _sample_rate: u32, _max_block: u32) -> Option<Box<dyn fx::FxProcessor>> {
    None
}

pub fn build_chain_from_specs(
    specs: &[FxSpec],
    sample_rate: u32,
    max_block: u32,
) -> Vec<Box<dyn fx::FxProcessor>> {
    specs.iter()
        .filter(|s| s.enabled)
        .filter_map(|s| {
            let mut proc = match &s.plugin {
                Some(r) => {
                    // A hosted plugin keeps its own parameters; hand it the
                    // values the UI is showing so a rebuild doesn't reset them.
                    let mut p = build_clap_fx(r, sample_rate, max_block)?;
                    for (i, v) in s.params.iter().enumerate() {
                        p.set_param(i, *v);
                    }
                    p
                }
                None => build_processor(&s.kind, &s.params, sample_rate)?,
            };
            proc.set_mix(s.wet);
            Some(proc)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every FX id the UI can build (kept in sync with `choz-ui`'s FX kinds).
    const FX_IDS: &[&str] = &[
        "delay", "reverb", "grandelay", "compressor", "limiter", "gate",
        "expander", "parameq", "filter", "filterbank", "chorus", "flanger",
        "phaser", "bitcrusher", "vinyl", "cassette", "softclip", "tubesat",
        "widener", "isolator", "gain", "phaseinvert", "monomaker", "pan",
        "looper", "sidechain", "protocosmos", "spaceecho", "reversedelay",
        "z5texture", "amberfang", "velvetfuzz",
    ];

    fn spec(kind: &str, plugin: Option<ClapFxRef>) -> FxSpec {
        FxSpec { kind: kind.into(), enabled: true, wet: 1.0, params: vec![0.5; 8], plugin }
    }

    /// A CLAP effect that can't be loaded (missing file, or the `clap` feature
    /// off) is dropped from the chain — the built-ins around it still build.
    #[test]
    fn unloadable_clap_fx_is_skipped() {
        let specs = vec![
            spec("gain", None),
            spec("", Some(ClapFxRef {
                path: "/nonexistent/nope.clap".into(),
                id: "com.example.nope".into(),
            })),
            spec("reverb", None),
        ];
        assert_eq!(build_chain_from_specs(&specs, 48_000, 256).len(), 2);
    }

    /// Every FX, built with a mid-range param set, must stay numerically sane
    /// (no NaN/Inf, no runaway) across several blocks of a -6 dBFS sine.
    #[test]
    fn all_fx_process_cleanly() {
        let sr = 48_000;
        // build_processor reads only the params it needs; a uniform 0.5 vector
        // covers every kind's parameter count.
        let params = [0.5f32; 8];
        for id in FX_IDS {
            let mut proc = build_processor(id, &params, sr)
                .unwrap_or_else(|| panic!("build_processor returned None for {id}"));

            let mut phase = 0.0f32;
            // ~10 blocks of 256 frames — enough to exercise delay/feedback state.
            for _ in 0..10 {
                let mut buf = [0.0f32; 512];
                for f in 0..256 {
                    let s = (2.0 * std::f32::consts::PI * phase).sin() * 0.5;
                    phase = (phase + 220.0 / sr as f32) % 1.0;
                    buf[f * 2] = s;
                    buf[f * 2 + 1] = s;
                }
                proc.process_block(&mut buf, sr);
                for (i, &s) in buf.iter().enumerate() {
                    assert!(s.is_finite(), "{id} produced non-finite at sample {i}");
                    assert!(s.abs() < 100.0, "{id} ran away to {s} at sample {i}");
                }
            }
        }
    }
}
