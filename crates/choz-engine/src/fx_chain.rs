//! Build realtime FX processor chains from specs.

use crate::fx;

#[derive(Debug, Clone, PartialEq)]
pub struct FxSpec {
    pub kind: String,
    pub enabled: bool,
    pub wet: f32,
    pub params: Vec<f32>,
    /// Set for hosted plugin effects: which plugin to load in this slot
    /// instead of a built-in FX. `kind` is then ignored.
    pub plugin: Option<PluginFxRef>,
}

/// Which plugin an FX slot hosts: the file (or LV2 bundle directory) and the
/// id inside it (CLAP plugin id, LV2 URI).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginFxRef {
    pub format: crate::PluginFormat,
    pub path: std::path::PathBuf,
    pub id: String,
}

/// Every effect choz brings with it: the id [`build_processor`] answers to, and
/// the name to show it under.
///
/// The match below is the other half of this list, and the two drifting apart
/// is what a test here catches: an id nobody can build, or an effect nothing
/// can reach. Anything that wants to walk the built-ins — the interface's ADD
/// FX list, the CLAP export — walks this.
pub const BUILT_IN_KINDS: &[(&str, &str)] = &[
    ("delay", "Delay"),
    ("reverb", "Reverb"),
    ("grandelay", "Granular Delay"),
    ("compressor", "Compressor"),
    ("limiter", "Limiter"),
    ("gate", "Gate"),
    ("parameq", "Parametric EQ"),
    ("graphiceq", "Graphic EQ"),
    ("filter", "Filter"),
    ("autotune", "Auto-Tune"),
    ("filterbank", "Filter Bank"),
    ("tremolo", "Tremolo"),
    ("autopan", "Auto Pan"),
    ("autofilter", "Auto Filter"),
    ("freqshifter", "Frequency Shifter"),
    ("ringmod", "Ring Modulator"),
    ("shimmer", "Shimmer"),
    ("harmonizer", "Harmonizer"),
    ("vocoder", "Vocoder"),
    ("beatrepeat", "Beat Repeat"),
    ("chorus", "Chorus"),
    ("flanger", "Flanger"),
    ("phaser", "Phaser"),
    ("bitcrusher", "Bitcrusher"),
    ("vinyl", "Vinyl"),
    ("cassette", "Cassette"),
    ("saturator", "Saturator"),
    ("waveshaper", "Wave Shaper"),
    ("softclip", "Soft Clip"),
    ("tubesat", "Tube Saturator"),
    ("widener", "Widener"),
    ("isolator", "Isolator"),
    ("gain", "Gain"),
    ("phaseinvert", "Phase Invert"),
    ("monomaker", "Mono Maker"),
    ("looper", "Looper"),
    ("sidechain", "Sidechain Duck"),
    ("expander", "Expander"),
    ("pan", "Pan"),
    ("protocosmos", "Protocosmos"),
    ("spaceecho", "Space Echo"),
    ("reversedelay", "Reverse Delay"),
    ("amberfang", "Amber Fang"),
    ("velvetfuzz", "Velvet Fuzz"),
    ("z5texture", "Z5 Texture"),
];

pub fn build_processor(
    kind: &str,
    params: &[f32],
    sample_rate: u32,
) -> Option<Box<dyn fx::FxProcessor>> {
    let p = |i: usize| params.get(i).copied().unwrap_or(0.0);

    let proc: Box<dyn fx::FxProcessor> = match kind {
        "delay" => {
            let delay_ms = 10.0 + p(0) * 990.0;
            let feedback = p(1);
            let damping = p(2);
            let mut d = fx::DelayLine::new(delay_ms, feedback, damping);
            d.set_ping_pong(p(3) > 0.5);
            // 4 is the dry/wet, which the chain applies itself.
            d.set_crossfeed(p(5));
            d.set_mod_rate(p(6) * 10.0);
            d.set_mod_depth_ms(p(7) * 50.0);
            Box::new(d)
        }
        "reverb" => {
            let mut r = fx::Reverb::new(sample_rate);
            r.set_room_size(p(0));
            r.set_damp(p(1));
            Box::new(r)
        }
        "grandelay" => Box::new(fx::GranularDelay::new(
            20.0 + p(0) * 980.0,
            p(1),
            (p(2) - 0.5) * 24.0,
            1.0 + p(3) * 31.0,
        )),
        "compressor" => {
            let mut c = fx::Compressor::new();
            c.threshold_db = -(1.0 - p(0)) * 60.0;
            c.ratio = 1.0 + p(1) * 19.0;
            c.attack_ms = 0.1 + p(2) * 99.9;
            c.release_ms = 10.0 + p(3) * 990.0;
            c.makeup_db = p(4) * 24.0;
            c.knee_db = p(5) * 12.0;
            c.detect = fx::compressor::Detect::from_norm(p(6));
            c.stereo_link = p(7);
            c.sc_hpf_hz = 20.0 + p(8) * 480.0;
            Box::new(c)
        }
        "limiter" => {
            let mut lim = fx::Compressor::limiter(sample_rate);
            lim.threshold_db = -(1.0 - p(0)) * 12.0;
            lim.release_ms = 1.0 + p(1) * 199.0;
            lim.lookahead_ms = p(2) * 10.0;
            lim.stereo_link = p(3);
            Box::new(lim)
        }
        "gate" => {
            let mut g = fx::Gate::new();
            g.threshold_db = -(1.0 - p(0)) * 80.0;
            g.attack_ms = 0.1 + p(1) * 49.9;
            g.hold_ms = 1.0 + p(2) * 499.0;
            g.release_ms = 10.0 + p(3) * 990.0;
            g.floor_db = -(1.0 - p(4)) * 80.0;
            g.hysteresis_db = p(5) * 24.0;
            Box::new(g)
        }
        // The interface builds the same EQ to draw its curve, so the mapping
        // lives with the processor rather than here.
        "parameq" => Box::new(fx::ParametricEq::from_params(params, sample_rate)),
        // Ten bands and a preamp, all of them knobs — so a CC can ride one
        // band. `p(11)` picks a Winamp preset, which fills the bands unless the
        // user has moved them (a preset is a starting point, not a lock).
        "graphiceq" => {
            let mut eq = fx::GraphicEq::new();
            eq.set_preset(fx::graphic_eq::preset_index(p(11)));
            // A band left in the middle keeps whatever the preset put there; one
            // the user has moved wins, because it is the later decision. The
            // first version let the preset win outright, which made every band
            // knob dead as soon as a preset was picked.
            for band in 0..fx::EQ_BANDS {
                if (p(band) - 0.5).abs() > 1e-4 {
                    eq.set_band_db(band, fx::graphic_eq::norm_to_db(p(band)));
                }
            }
            eq.set_preamp_db(fx::graphic_eq::norm_to_db(p(10)));
            Box::new(eq)
        }
        "filter" => {
            let freq = 20.0 + p(0) * 19980.0;
            // Map 0..1 into the filter's 0..~0.98 resonance range (1.0 self-oscillates).
            let res = p(1) * 0.98;
            Box::new(fx::Svf::new(fx::SvfMode::Lowpass, freq, res))
        }
        // AutoTune reads its whole parameter block at once — a preset sets
        // several of them, so applying them one by one would fight itself.
        "autotune" => {
            use fx::FxProcessor as _;
            let mut at = fx::AutoTune::new(sample_rate as f32);
            // The preset is **not** applied here. Picking one writes its values
            // into the parameter array itself (`AudioFxEntry::apply_preset`),
            // and that array is what the project saves and this rebuilds from —
            // applying the preset again would fight whatever the user moved
            // afterwards.
            for i in 1..13 {
                at.set_param(i, p(i));
            }
            Box::new(at)
        }
        "filterbank" => Box::new(fx::FilterBankFx::new(sample_rate)),
        // One processor, two effects: the LFO points at level or at balance.
        "tremolo" => Box::new(fx::Tremolo::with_params(
            fx::ModTarget::Tremolo,
            sample_rate,
            params,
        )),
        "autopan" => Box::new(fx::Tremolo::with_params(
            fx::ModTarget::AutoPan,
            sample_rate,
            params,
        )),
        "autofilter" => Box::new(fx::AutoFilter::with_params(sample_rate, params)),
        // One carrier, two uses: one sideband or both.
        "freqshifter" => Box::new(fx::FreqShift::with_params(
            fx::Carrier::Shift,
            sample_rate,
            params,
        )),
        "ringmod" => Box::new(fx::FreqShift::with_params(
            fx::Carrier::Ring,
            sample_rate,
            params,
        )),
        "shimmer" => Box::new(fx::ShimmerReverb::with_params(sample_rate, params)),
        "harmonizer" => Box::new(fx::Harmonizer::with_params(sample_rate, params)),
        "vocoder" => Box::new(fx::Vocoder::with_params(sample_rate, params)),
        "beatrepeat" => Box::new(fx::BeatRepeat::with_params(sample_rate, params)),
        "chorus" => {
            let mut c = fx::Chorus::new();
            c.rate = 0.05 + p(0) * 4.95;
            c.depth = 0.5 + p(1) * 9.5;
            c.delay_ms = 5.0 + p(2) * 25.0;
            c.feedback = (p(3) - 0.5) * 1.8;
            Box::new(c)
        }
        "flanger" => {
            let mut f = fx::Flanger::new();
            f.rate = 0.05 + p(0) * 4.95;
            f.depth = p(1) * 7.0;
            f.delay_ms = 0.5 + p(2) * 9.5;
            f.feedback = (p(3) - 0.5) * 1.9;
            Box::new(f)
        }
        "phaser" => {
            let mut ph = fx::Phaser::new();
            ph.rate = 0.05 + p(0) * 4.95;
            ph.depth = p(1);
            ph.center = 200.0 + p(2) * 1800.0;
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
        // The general waveshaper: curve and oversampling are its own knobs.
        "saturator" => Box::new(fx::Saturator::with_params(sample_rate, params)),
        // The same processor: the curve is drawn instead of computed.
        "waveshaper" => Box::new(fx::Saturator::waveshaper(sample_rate, params)),
        "softclip" => {
            let mut s = fx::SoftClipper::new();
            s.drive = 1.0 + p(0) * 9.0;
            Box::new(s)
        }
        "tubesat" => {
            let mut t = fx::TubeSaturation::new();
            t.drive = 1.0 + p(0) * 19.0;
            t.tone = p(1);
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
        "phaseinvert" => Box::new(fx::PhaseInvert {
            invert_l: p(0) > 0.5,
            invert_r: p(1) > 0.5,
        }),
        "monomaker" => Box::new(fx::MonoMaker::new()),
        "looper" => Box::new(fx::Looper::new(sample_rate)),
        "sidechain" => Box::new(fx::SidechainDuck::new()),
        "expander" => {
            let mut exp = fx::Expander::new();
            exp.threshold_db = -(1.0 - p(0)) * 80.0;
            exp.ratio = 1.0 + p(1) * 9.0;
            exp.attack_ms = 0.1 + p(2) * 49.9;
            exp.release_ms = 10.0 + p(3) * 990.0;
            exp.range_db = p(4) * 80.0;
            Box::new(exp)
        }
        "pan" => {
            let mut pan = fx::Pan::new();
            pan.pan = (p(0) - 0.5) * 2.0;
            pan.constant_power = p(1) > 0.5;
            Box::new(pan)
        }
        // Creative time/texture FX imported from seqterm: these take their
        // parameters normalised, in the same order `params()` reports them.
        "protocosmos" => Box::new(fx::Protocosmos::new(
            sample_rate,
            p(0),
            p(1),
            p(2),
            p(3),
            p(4),
            p(5),
            p(6),
        )),
        "spaceecho" => Box::new(fx::SpaceEcho::new(
            sample_rate,
            p(0),
            p(1),
            p(2),
            p(3),
            p(4),
            p(5),
            p(6),
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

/// Build a hosted plugin effect. Loading happens here (UI thread), never on
/// the RT thread. `None` for a format this build can't host, or a plugin that
/// refuses to load.
pub(crate) fn build_plugin_fx(
    r: &PluginFxRef,
    sample_rate: u32,
    max_block: u32,
) -> Option<Box<dyn fx::FxProcessor>> {
    // Same policy as instruments: what the load probe caught dying on teardown
    // goes in its own process, so removing the effect costs a child — and so
    // does anything the user asked for by hand.
    //
    // A Pure Data patch has no other way in: choz does not link libpd, and one
    // process holds one Pd, so "in-process" is not a fallback that exists.
    if r.format == crate::PluginFormat::Pd
        || crate::quarantine::wants_sandbox(r.format, &r.path, &r.id)
    {
        match crate::sandboxed::SandboxedEffect::build(
            r.format,
            &r.path,
            &r.id,
            sample_rate,
            max_block,
        ) {
            Ok(fx) => {
                eprintln!("choz: hosting {} in its own process", r.path.display());
                return Some(Box::new(fx));
            }
            Err(e) if r.format == crate::PluginFormat::Pd => {
                eprintln!("choz: cannot run {}: {e}", r.path.display());
                return None;
            }
            Err(e) => eprintln!(
                "choz: sandbox for {} failed ({e}); hosting in-process",
                r.path.display()
            ),
        }
    }
    build_plugin_fx_in_process(r, sample_rate, max_block)
}

/// Load a plugin effect in this process. The sandbox child calls exactly this.
pub(crate) fn build_plugin_fx_in_process(
    r: &PluginFxRef,
    sample_rate: u32,
    max_block: u32,
) -> Option<Box<dyn fx::FxProcessor>> {
    match r.format {
        crate::PluginFormat::Clap => build_clap_fx(r, sample_rate, max_block),
        crate::PluginFormat::Lv2 => Some(Box::new(choz_plugin_lv2::Lv2Effect::build(
            &r.path,
            &r.id,
            sample_rate,
            max_block,
        )?)),
        crate::PluginFormat::Ladspa | crate::PluginFormat::Dssi => Some(Box::new(
            choz_plugin_ladspa::LadspaEffect::build(&r.path, &r.id, sample_rate, max_block)?,
        )),
        crate::PluginFormat::Vst2 => Some(Box::new(choz_plugin_vst2::Vst2Effect::build(
            &r.path,
            sample_rate,
            max_block,
        )?)),
        crate::PluginFormat::Vst3 => Some(Box::new(choz_plugin_vst3::Vst3Effect::build(
            &r.path,
            sample_rate,
            max_block,
        )?)),
        _ => None,
    }
}

fn build_clap_fx(
    r: &PluginFxRef,
    sample_rate: u32,
    max_block: u32,
) -> Option<Box<dyn fx::FxProcessor>> {
    let eff = choz_plugin_clap::host::ClapEffect::build(&r.path, &r.id, sample_rate, max_block)?;
    Some(Box::new(eff))
}

/// Every effect in a chain, metered — without a single effect knowing about it.
///
/// The alternative was a pair of peak fields inside each processor (which is
/// what the `Saturator` grew first, and what the roadmap called out): thirty-odd
/// copies of the same two lines, and no meter at all for a hosted plugin, which
/// is exactly where "is anything even reaching this?" gets asked. Here the peak
/// is taken on the way in and on the way out of a `process_block` that has no
/// idea it is being watched.
///
/// Cost: two passes over the block per effect. A peak is a compare per sample
/// over a few hundred of them — next to the effect it wraps, nothing.
///
/// Everything else is forwarded. A wrapper that swallowed `editor()` would take
/// a plugin's window away, which is the trap with this shape.
struct Metered {
    inner: Box<dyn fx::FxProcessor>,
    meter: choz_ports::FxMeter,
}

impl fx::FxProcessor for Metered {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        let input = choz_ports::FxMeter::peak_of(buf);
        self.inner.process_block(buf, sample_rate);
        self.meter.publish(input, choz_ports::FxMeter::peak_of(buf));
    }

    fn reset(&mut self) {
        self.inner.reset();
        self.meter.clear();
    }

    fn set_mix(&mut self, wet: f32) {
        self.inner.set_mix(wet);
    }

    fn name(&self) -> &str {
        self.inner.name()
    }

    fn params(&self) -> Vec<choz_ports::FxParam> {
        self.inner.params()
    }

    fn set_param(&mut self, index: usize, value: f32) {
        self.inner.set_param(index, value);
    }

    fn editor(&self) -> Option<choz_ports::EditorHandle> {
        self.inner.editor()
    }

    fn param_touch(&self) -> Option<choz_ports::TouchHandle> {
        self.inner.param_touch()
    }

    fn state(&self) -> Option<choz_ports::StateHandle> {
        self.inner.state()
    }

    fn sandbox(&self) -> Option<choz_ports::SandboxStatus> {
        self.inner.sandbox()
    }

    fn meter(&self) -> Option<choz_ports::FxMeter> {
        Some(self.meter.clone())
    }

    fn latency_samples(&self) -> u32 {
        self.inner.latency_samples()
    }
}

pub fn build_chain_from_specs(
    specs: &[FxSpec],
    sample_rate: u32,
    max_block: u32,
) -> Vec<Box<dyn fx::FxProcessor>> {
    specs
        .iter()
        .filter(|s| s.enabled)
        .filter_map(|s| {
            let mut proc = match &s.plugin {
                Some(r) => {
                    // A hosted plugin keeps its own parameters; hand it the
                    // values the UI is showing so a rebuild doesn't reset them.
                    let mut p = build_plugin_fx(r, sample_rate, max_block)?;
                    for (i, v) in s.params.iter().enumerate() {
                        p.set_param(i, *v);
                    }
                    p
                }
                None => build_processor(&s.kind, &s.params, sample_rate)?,
            };
            proc.set_mix(s.wet);
            Some(Box::new(Metered {
                inner: proc,
                meter: choz_ports::FxMeter::default(),
            }) as Box<dyn fx::FxProcessor>)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The list under test is the one everything else walks — see
    /// [`BUILT_IN_KINDS`]. There used to be a second copy here, which is
    /// exactly the drift it was supposed to catch.
    fn fx_ids() -> Vec<&'static str> {
        BUILT_IN_KINDS.iter().map(|(id, _)| *id).collect()
    }

    fn spec(kind: &str, plugin: Option<PluginFxRef>) -> FxSpec {
        FxSpec {
            kind: kind.into(),
            enabled: true,
            wet: 1.0,
            params: vec![0.5; 8],
            plugin,
        }
    }

    /// A CLAP effect that can't be loaded (missing file) is dropped from the
    /// chain — the built-ins around it still build.
    #[test]
    fn unloadable_clap_fx_is_skipped() {
        let specs = vec![
            spec("gain", None),
            spec(
                "",
                Some(PluginFxRef {
                    format: crate::PluginFormat::Clap,
                    path: "/nonexistent/nope.clap".into(),
                    id: "com.example.nope".into(),
                }),
            ),
            spec("reverb", None),
        ];
        assert_eq!(build_chain_from_specs(&specs, 48_000, 256).len(), 2);
    }

    /// Every effect in a built chain is metered, whatever it is — that is the
    /// whole point of doing it in the wrapper instead of inside each processor.
    /// And a chain reports the delay its effects add.
    #[test]
    fn a_built_chain_meters_every_effect_and_reports_its_latency() {
        // Gain at 0.5 is a knob position, not a factor; what matters is that
        // both sides of it read something and neither is stuck at zero.
        let mut chain = build_chain_from_specs(&[spec("gain", None)], 48_000, 256);
        let meter = chain[0].meter().expect("the wrapper meters everything");
        assert_eq!(meter.peaks(), (0.0, 0.0), "nothing has gone through yet");

        let mut buf = [0.0f32; 512];
        for (i, s) in buf.iter_mut().enumerate() {
            *s = 0.5 * (2.0 * std::f32::consts::PI * (i / 2) as f32 / 64.0).sin();
        }
        chain[0].process_block(&mut buf, 48_000);
        let (input, output) = meter.peaks();
        assert!((input - 0.5).abs() < 0.02, "input peak: {input}");
        assert!(output > 0.0 && output.is_finite(), "output peak: {output}");

        // Silence after signal reads as silence: a needle that stays up is a
        // needle nobody believes.
        chain[0].process_block(&mut [0.0f32; 512], 48_000);
        assert_eq!(meter.peaks(), (0.0, 0.0));

        // A gain stage delays nothing; AutoTune holds a shifter window, and the
        // wrapper must pass that number through rather than answer for it.
        assert_eq!(chain[0].latency_samples(), 0);
        let tuned = build_chain_from_specs(&[spec("autotune", None)], 48_000, 256);
        assert!(
            tuned[0].latency_samples() > 0,
            "AutoTune reports its shifter window"
        );
    }

    /// Every FX, built with a mid-range param set, must stay numerically sane
    /// (no NaN/Inf, no runaway) across several blocks of a -6 dBFS sine.
    #[test]
    fn all_fx_process_cleanly() {
        let sr = 48_000;
        // build_processor reads only the params it needs; a uniform 0.5 vector
        // covers every kind's parameter count.
        let params = [0.5f32; 8];
        for id in fx_ids() {
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
