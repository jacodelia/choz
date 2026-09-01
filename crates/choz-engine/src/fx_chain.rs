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
    /// Driven by another tab's level, when the user asked for it.
    pub gate: Option<GateSpec>,
    /// Takes to put back into a looper deck, as `(track, chunks)`.
    ///
    /// The one part of an effect's state that a parameter cannot carry: the
    /// audio somebody played into it. A project stores it as WAVs beside the
    /// file and hands it back here, so a deck built from a spec is a deck with
    /// its loops in it — see [`choz_ports::FxProcessor::load_loops`]. Empty for
    /// every effect that is not a looper, and for a deck that never recorded.
    pub loops: Vec<(usize, Vec<choz_ports::LoopChunk>)>,
    /// The deck's own loop length in frames, which the chunks do not say: the
    /// last chunk of a take runs past the end of the loop.
    pub loop_frames: usize,
}

/// An effect opened (or closed) by what **another tab** is playing.
///
/// The case it exists for: a drum kit on tab 1 and a keyboard on tab 2, and the
/// kick opening an auto-wah on the keyboard. Every host calls this a sidechain;
/// what makes it a *gate* here is that it moves the effect's dry/wet rather
/// than a compressor's gain, which is why it works with all forty-five of
/// choz's effects and with hosted plugins, without one line of per-effect code.
///
/// The source is read from [`crate::meter::SlotLevels::live`] — the tab's own
/// level in the last block, which the audio callback already publishes. A
/// source tab that renders *after* this one is therefore one block late; at
/// choz's block sizes that is under three milliseconds and nobody has ever
/// heard it, but it is the reason the order of tabs is not entirely arbitrary.
/// What drives a gate.
///
/// A tab is the sidechain everybody knows. The other two are the clock: an
/// effect that has to open **on the beat** rather than on a hit — a tremolo
/// that chops in time, a delay that only speaks between bars — has nothing to
/// listen to when nothing is playing it, and asking the player to keep a tab of
/// clicks running just to drive it is a workaround, not a feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateSource {
    /// Another tab's level, by rack index.
    Tab(usize),
    /// Another tab's **notes**, by rack index: velocity when one arrives,
    /// decaying like a beat.
    ///
    /// The level source answers "how loud is that tab", which is the right
    /// question for a kick drum and the wrong one for a pad holding a chord
    /// underneath everything — quiet on purpose, and never crossing a
    /// threshold. This one asks "is that tab being played", which does not
    /// care how loud the answer came out.
    Note(usize),
    /// The transport's beat, wherever the transport is getting it — choz's own
    /// clock or an external one. Silent metronome included: this is the count,
    /// not the click.
    Clock,
    /// The internal metronome's tap, as it actually sounds: off when the
    /// metronome is off, and accented on the downbeat the way the click is.
    Metronome,
    /// The step sequencer's own hits — the SEQ artifact, driving the gate the
    /// way a tab of clicks used to have to.
    ///
    /// A pattern *is* a rhythm, so a tremolo or a delay wired to this opens on
    /// the steps that were written rather than on whatever happens to be loud.
    /// One per rack, like the metronome's tap: it follows whichever sequencer
    /// fired last, which on a rig running one is the only one there is.
    Seq,
}

/// The transport sample the last sequencer step fired on, or `u64::MAX` for a
/// rack whose sequencers have not played anything yet.
static SEQ_HIT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(u64::MAX);

/// A sequencer step just played. Called from the interface, which is where the
/// step clock lives — see `choz-ui/src/seq.rs`.
pub fn seq_hit() {
    SEQ_HIT.store(
        choz_ports::transport().samples(),
        std::sync::atomic::Ordering::Relaxed,
    );
}

/// The envelope of that step, on the same 0..1 scale a tab's level is read on.
///
/// Same shape and length as [`crate::metronome::beat_pulse`], so a gate set up
/// against the clock reads the same wired to the sequencer.
pub fn seq_pulse() -> f32 {
    let last = SEQ_HIT.load(std::sync::atomic::Ordering::Relaxed);
    if last == u64::MAX {
        return 0.0;
    }
    let t = choz_ports::transport();
    let secs = t.samples().saturating_sub(last) as f32 / t.sample_rate().max(1) as f32;
    (-(secs / 0.125)).exp()
}

impl Default for GateSource {
    fn default() -> Self {
        GateSource::Tab(0)
    }
}

impl GateSource {
    /// The level this source is offering right now, on the same 0..1 scale a
    /// tab's is read on.
    pub fn level(self) -> f32 {
        match self {
            GateSource::Tab(i) => crate::meter::slot_levels().live(i),
            GateSource::Note(i) => crate::meter::note_levels().level(i),
            GateSource::Metronome => crate::metronome::metronome().tap_level(),
            GateSource::Clock => crate::metronome::beat_pulse(),
            GateSource::Seq => seq_pulse(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GateSpec {
    /// What drives this: another tab, the clock, or the metronome's tap.
    pub source: GateSource,
    pub mode: GateMode,
    /// How much of the effect the gate is allowed to move, 0..1. At 0 it does
    /// nothing; at 1 the effect is entirely the gate's.
    pub depth: f32,
    /// The source level that counts as fully open, linear. A kick peaks around
    /// 0.5, which is why that is the default rather than full scale.
    pub threshold: f32,
    /// How long the gate takes to fall back, in milliseconds. Rising is
    /// immediate: a gate that is late to open is a gate that missed the hit.
    pub release_ms: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GateMode {
    /// The source opens the effect: silence on the source means no effect.
    #[default]
    Open,
    /// The source closes it — the sidechain duck every mix engineer knows.
    Duck,
}

impl GateMode {
    pub const ALL: [GateMode; 2] = [GateMode::Open, GateMode::Duck];

    pub fn label(self) -> &'static str {
        match self {
            GateMode::Open => "OPEN",
            GateMode::Duck => "DUCK",
        }
    }
}

impl Default for GateSpec {
    fn default() -> Self {
        Self {
            source: GateSource::Tab(0),
            mode: GateMode::default(),
            depth: 1.0,
            threshold: 0.5,
            release_ms: 120.0,
        }
    }
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
    ("envelope", "Envelope"),
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
    ("pitchshifter", "Pitch Shifter"),
    ("vibrato", "Vibrato"),
    ("multitap", "Multi-tap Delay"),
    ("platereverb", "Plate Reverb"),
    ("moogladder", "Moog Ladder"),
    ("deesser", "De-esser"),
    ("transient", "Transient Shaper"),
    ("multiband", "Multiband Comp"),
    ("exciter", "Exciter"),
    ("bassenhance", "Bass Enhancer"),
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
            // Per-index defaults rather than the shared `p`, which answers 0.0
            // for anything a project did not write. A reverb saved before this
            // engine existed has four parameters; reading the other nine as
            // zero would open it with no decay, no diffusion and no tail —
            // the settings kept, the sound gone.
            let q = |i: usize, d: f32| params.get(i).copied().unwrap_or(d);
            let mut r = fx::Reverb::new(sample_rate);
            r.set_room_size(q(0, 0.50));
            r.set_damp(q(1, 0.50));
            r.set_width(q(2, 1.00));
            // 3 is the dry/wet, which the chain applies itself.
            r.set_decay(q(4, 0.45));
            r.set_predelay(q(5, 0.08));
            r.set_diffusion(q(6, 0.70));
            r.set_tone(q(7, 0.50));
            r.set_modulation(q(8, 0.25));
            r.set_low_cut(q(9, 0.15));
            r.set_high_cut(q(10, 0.80));
            r.set_character(fx::reverb::Character::from_norm(q(11, 0.25)));
            r.set_quality(fx::reverb::Quality::from_norm(q(12, 1.0)));
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
        // Low, mid and high were drawn and reached nothing: the bank was
        // built at its defaults and the knobs went nowhere.
        "filterbank" => {
            use fx::FxProcessor as _;
            let mut fb = fx::FilterBankFx::new(sample_rate);
            for i in 0..3 {
                fb.set_param(i, p(i));
            }
            Box::new(fb)
        }
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
        // The contour is written, not followed: see `fx::envelope`.
        "envelope" => Box::new(fx::Envelope::with_params(sample_rate, params)),
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
        "pitchshifter" => Box::new(fx::PitchShifter::with_params(sample_rate, params)),
        "vibrato" => Box::new(fx::Vibrato::with_params(sample_rate, params)),
        "multitap" => Box::new(fx::MultiTapDelay::with_params(sample_rate, params)),
        "platereverb" => Box::new(fx::PlateReverb::with_params(sample_rate, params)),
        "moogladder" => Box::new(fx::MoogLadder::with_params(sample_rate, params)),
        "deesser" => Box::new(fx::DeEsser::with_params(sample_rate, params)),
        "transient" => Box::new(fx::TransientShaper::with_params(sample_rate, params)),
        "multiband" => Box::new(fx::MultibandCompressor::with_params(sample_rate, params)),
        "exciter" => Box::new(fx::Exciter::with_params(sample_rate, params)),
        "bassenhance" => Box::new(fx::BassEnhancer::with_params(sample_rate, params)),
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
        "cassette" => {
            let mut c = fx::Cassette::new();
            c.set_drive(0.5 + p(0) * 7.5);
            Box::new(c)
        }
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
        "isolator" => {
            use fx::FxProcessor as _;
            let mut iso = fx::Isolator::new();
            for i in 0..3 {
                iso.set_param(i, params.get(i).copied().unwrap_or(0.5));
            }
            Box::new(iso)
        }
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
        "sidechain" => {
            let mut sc = fx::SidechainDuck::new();
            sc.set_depth(p(0));
            sc.set_release(0.01 + p(1) * 0.99);
            Box::new(sc)
        }
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
/// An effect whose dry/wet is ridden by another tab's level.
///
/// One wrapper for every effect, rather than a sidechain input on each: the
/// only thing it needs from the processor below it is `set_mix`, which the
/// trait has required all along.
struct Gated {
    inner: Box<dyn fx::FxProcessor>,
    gate: GateSpec,
    /// The mix the user set, which is what the gate moves *from*.
    base_wet: f32,
    /// The follower's state, 0..1. Rises to the source at once and falls by
    /// the release time — the shape of a gate, not of a filter.
    env: f32,
    /// What was last handed to the inner processor, so a block that does not
    /// move the gate does not set the same number again.
    sent: f32,
}

impl fx::FxProcessor for Gated {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        let frames = (buf.len() / 2).max(1) as f32;
        let level = self.gate.source.level();
        let open = (level / self.gate.threshold.max(1e-4)).clamp(0.0, 1.0);
        // Instant attack, exponential release, stepped once per block: a kick
        // is over in less time than a fader move, and the block is the grid the
        // dry/wet can be changed on anyway.
        if open >= self.env {
            self.env = open;
        } else {
            let secs = frames / sample_rate.max(1) as f32;
            let coeff = (-secs / (self.gate.release_ms.max(1.0) / 1000.0)).exp();
            self.env = open + (self.env - open) * coeff;
        }
        let g = match self.gate.mode {
            GateMode::Open => self.env,
            GateMode::Duck => 1.0 - self.env,
        };
        // Depth is how much of the effect the gate owns: the rest stays where
        // the user put it, so a gate at half depth is an effect that breathes
        // rather than one that switches.
        let d = self.gate.depth.clamp(0.0, 1.0);
        let wet = self.base_wet * (1.0 - d + d * g);
        if (wet - self.sent).abs() > 1e-4 {
            self.inner.set_mix(wet);
            self.sent = wet;
        }
        self.inner.process_block(buf, sample_rate);
    }

    fn reset(&mut self) {
        self.env = 0.0;
        self.sent = -1.0;
        self.inner.reset();
    }

    /// The user moving the dry/wet moves what the gate works from, not the
    /// gate's own output — otherwise the next block would overwrite the move.
    fn set_mix(&mut self, wet: f32) {
        self.base_wet = wet.clamp(0.0, 1.0);
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

    fn presets(&self) -> Option<choz_ports::PresetsHandle> {
        self.inner.presets()
    }

    fn sandbox(&self) -> Option<choz_ports::SandboxStatus> {
        self.inner.sandbox()
    }

    fn meter(&self) -> Option<choz_ports::FxMeter> {
        self.inner.meter()
    }

    fn loopdeck(&mut self) -> Option<choz_ports::LoopHandle> {
        self.inner.loopdeck()
    }

    fn load_loops(&mut self, takes: &[(usize, Vec<choz_ports::LoopChunk>)], frames: usize) {
        self.inner.load_loops(takes, frames)
    }

    fn is_loop_deck(&self) -> bool {
        self.inner.is_loop_deck()
    }

    fn latency_samples(&self) -> u32 {
        self.inner.latency_samples()
    }
}

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

    /// A deck's handle has exactly two ends, and the wrapper must not be where
    /// one of them stops. Everything the interface needs from a looper — the
    /// chunks it feeds it, the state it draws, the transport it drives — comes
    /// through here, so a wrapper that swallows it is a looper that cannot
    /// record and a panel with nothing to draw.
    fn loopdeck(&mut self) -> Option<choz_ports::LoopHandle> {
        self.inner.loopdeck()
    }

    fn load_loops(&mut self, takes: &[(usize, Vec<choz_ports::LoopChunk>)], frames: usize) {
        self.inner.load_loops(takes, frames)
    }

    fn is_loop_deck(&self) -> bool {
        self.inner.is_loop_deck()
    }

    fn latency_samples(&self) -> u32 {
        self.inner.latency_samples()
    }
}

/// A slot that is switched off, or one whose plugin would not load.
///
/// It stays in the chain rather than being dropped from it, because **every
/// handle the interface holds is addressed by position**: `fx_editors[slot][i]`,
/// `fx_loopers[slot][i]`, and the `fx` in a `SetFxParam` are all the index of
/// the effect *as the rack draws it*. Dropping a spec here shifted every effect
/// after it by one, so switching off an early effect quietly pointed the panel,
/// the meters and the learned CCs at their neighbours.
struct Bypass;

impl fx::FxProcessor for Bypass {
    fn process_block(&mut self, _buf: &mut [f32], _sample_rate: u32) {}
    fn reset(&mut self) {}
    fn set_mix(&mut self, _wet: f32) {}
    fn name(&self) -> &str {
        "off"
    }
}

/// Which tabs some effect is gated by, one bit each.
///
/// Read by the engine's render loop and written by whoever rebuilds a chain,
/// which is the interface thread — a relaxed load per block against a store
/// per edit.
static GATE_SOURCES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// The tabs that drive a gate somewhere in the rack, as a bitmask.
///
/// **What it is for: order.** A gate reads the source tab's level for the block
/// it is in, so a source rendered *after* the gated tab is read a block late —
/// audible as a rhythmic gate that lags its own kick by up to 5 ms. The render
/// loop takes these tabs first; everything else follows in its own order, and
/// the sum does not care which order it was added in.
pub fn gate_sources() -> u32 {
    GATE_SOURCES.load(std::sync::atomic::Ordering::Relaxed)
}

/// Publish the whole mask. Called with **every** chain's sources, not one
/// chain's: a bit that nobody drives any more has to go out.
pub fn set_gate_sources(mask: u32) {
    GATE_SOURCES.store(mask, std::sync::atomic::Ordering::Relaxed);
}

/// Wrap a processor in its gate, for a caller that has the two of them and no
/// spec to build from — the engine's own tests, which is the only place that
/// is true.
pub fn gated(
    inner: Box<dyn fx::FxProcessor>,
    gate: GateSpec,
    base_wet: f32,
) -> Box<dyn fx::FxProcessor> {
    Box::new(Gated {
        inner,
        gate,
        base_wet,
        env: 0.0,
        sent: -1.0,
    })
}

/// The tabs this chain's gates read, as a bitmask. The clock and the metronome
/// are nobody's tab and set no bit; a gate on the **notes** of a tab does not
/// either, because a note is published where it arrives and not where the tab
/// renders.
pub fn gate_sources_of(specs: &[FxSpec]) -> u32 {
    specs
        .iter()
        .filter_map(|s| s.gate)
        .filter_map(|g| match g.source {
            GateSource::Tab(i) => Some(i),
            _ => None,
        })
        .filter(|i| *i < 32)
        .fold(0u32, |mask, i| mask | (1 << i))
}

pub fn build_chain_from_specs(
    specs: &[FxSpec],
    sample_rate: u32,
    max_block: u32,
) -> Vec<Box<dyn fx::FxProcessor>> {
    specs
        .iter()
        .map(|s| {
            let built = match s.enabled {
                false => None,
                true => match &s.plugin {
                    Some(r) => {
                        // A hosted plugin keeps its own parameters; hand it the
                        // values the UI is showing so a rebuild doesn't reset
                        // them.
                        build_plugin_fx(r, sample_rate, max_block).map(|mut p| {
                            for (i, v) in s.params.iter().enumerate() {
                                p.set_param(i, *v);
                            }
                            p
                        })
                    }
                    None => build_processor(&s.kind, &s.params, sample_rate),
                },
            };
            let Some(mut proc) = built else {
                return Box::new(Bypass) as Box<dyn fx::FxProcessor>;
            };
            proc.set_mix(s.wet);
            // Before any wrapper and before the RT thread: the one moment a
            // deck can be handed the audio a project saved for it.
            if !s.loops.is_empty() {
                proc.load_loops(&s.loops, s.loop_frames);
            }
            // The gate wraps the processor and the meter wraps the gate, so
            // what the interface's meter shows is what actually came out —
            // gate and all.
            let proc: Box<dyn fx::FxProcessor> = match s.gate {
                Some(gate) => Box::new(Gated {
                    inner: proc,
                    gate,
                    base_wet: s.wet,
                    env: 0.0,
                    sent: -1.0,
                }),
                None => proc,
            };
            Box::new(Metered {
                inner: proc,
                meter: choz_ports::FxMeter::default(),
            }) as Box<dyn fx::FxProcessor>
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

    /// A gate driven by notes opens for a tab that makes no sound worth
    /// metering — which is the whole reason that source exists.
    ///
    /// The level source answers "how loud is that tab": right for a kick,
    /// wrong for a pad holding a chord underneath everything, which never
    /// crosses a threshold and so could never drive a gate at all.
    #[test]
    fn a_gate_can_be_opened_by_playing_rather_than_by_loudness() {
        let notes = crate::meter::note_levels();
        let levels = crate::meter::slot_levels();
        notes.reset_all();
        levels.reset(3);

        let quiet = GateSource::Tab(3);
        let played = GateSource::Note(3);
        assert!(quiet.level() < 1e-6, "nothing has been metered yet");
        assert!(played.level() < 1e-6, "and nothing has been played");

        // A note arrives on a tab that is not making a metered sound.
        notes.hit(3, 100);
        assert!(
            played.level() > 0.7,
            "the note opens it: {}",
            played.level()
        );
        assert!(
            quiet.level() < 1e-6,
            "while the level source still says nothing, which is the bug"
        );

        // And it falls: the source is a hit, not a switch.
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(
            played.level() < 0.3,
            "it decays after the note: {}",
            played.level()
        );
        notes.reset_all();
    }

    /// The dry/wet law, checked on **every** built-in at once: a wet of zero
    /// is a wire.
    ///
    /// It is one property, and it catches the whole family — an effect that
    /// adds its output instead of crossfading it, one that forgot to apply the
    /// mix at all, one whose output has a gain baked in. The looper adds on
    /// purpose (its takes play under what is being played) and still passes,
    /// because at zero there is nothing to add.
    ///
    /// Compared as levels rather than sample by sample: the compressor and
    /// Auto-Tune delay the dry to line it up with their own latency, which is
    /// right and which no sample-by-sample comparison would forgive.
    #[test]
    fn a_wet_of_zero_is_a_wire_in_every_built_in() {
        let sr = 48_000u32;
        let mut rng = 0x1234_5678u32;
        let mut lp = 0.0f32;
        let dry: Vec<f32> = (0..sr as usize)
            .flat_map(|i| {
                rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
                let white = (rng >> 8) as f32 / 8_388_608.0 - 1.0;
                lp += 0.05 * (white - lp);
                let tone = (std::f32::consts::TAU * 220.0 * i as f32 / sr as f32).sin();
                let s = 0.35 * tone + 0.25 * lp;
                [s, s]
            })
            .collect();
        let rms = |x: &[f32]| (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt();
        let dry_rms = rms(&dry);
        for kind in fx_ids() {
            let params = [0.5f32; 16];
            let Some(mut p) = build_processor(kind, &params, sr) else {
                panic!("{kind} is in the list and cannot be built");
            };
            p.set_mix(0.0);
            let mut buf = dry.clone();
            for block in buf.chunks_mut(512) {
                p.process_block(block, sr);
            }
            let db = 20.0 * (rms(&buf[buf.len() / 2..]) / dry_rms).log10();
            assert!(
                db.abs() < 0.5,
                "{kind} at a dry/wet of zero moved the level by {db:.2} dB"
            );
        }
    }

    fn spec(kind: &str, plugin: Option<PluginFxRef>) -> FxSpec {
        FxSpec {
            gate: None,
            kind: kind.into(),
            enabled: true,
            wet: 1.0,
            params: vec![0.5; 8],
            plugin,
            loops: Vec::new(),
            loop_frames: 0,
        }
    }

    /// The two clock gate sources: the metronome's tap says nothing while the
    /// metronome is off, and the transport's beat pulses whether it does or
    /// not — which is the point of having both.
    #[test]
    fn the_clock_can_drive_a_gate_with_no_tab_playing() {
        let m = crate::metronome::metronome();
        let t = choz_ports::transport();
        t.set_sample_rate(48_000);
        t.set_bpm(120.0);

        m.set_on(false);
        m.render(&mut [0.0f32; 128], 64, 48_000);
        assert_eq!(
            GateSource::Metronome.level(),
            0.0,
            "a metronome that is off taps nothing"
        );

        m.set_on(true);
        m.set_gain(1.0);
        // Switching it on rewinds its count, so the first block is the downbeat.
        m.render(&mut [0.0f32; 128], 64, 48_000);
        assert!(
            GateSource::Metronome.level() > 0.05,
            "and the click itself is the tap: {}",
            GateSource::Metronome.level()
        );
        m.set_on(false);

        // The beat pulse is the transport's, not the click's: full on the
        // boundary, well down a third of a beat later.
        t.rewind();
        let on_beat = GateSource::Clock.level();
        t.advance(8_000); // a third of a beat at 120 bpm
        let after = GateSource::Clock.level();
        assert!(on_beat > 0.9, "the boundary is the hit: {on_beat}");
        assert!(after < 0.3, "and it has decayed by the next third: {after}");
        t.rewind();
    }

    /// A gate is another tab's level riding this effect's dry/wet. The case it
    /// was built for: a kick on one tab opening a filter on another.
    #[test]
    fn a_gate_opens_an_effect_from_another_tab() {
        use crate::fx::FxProcessor;
        let levels = crate::meter::slot_levels();
        let drums = 7usize;

        // An effect that is trivially audible: full wet is silence, dry is the
        // signal. Anything measurable would do — the gate moves `set_mix`, and
        // that is the same call for all forty-five.
        struct Silencer {
            wet: f32,
        }
        impl FxProcessor for Silencer {
            fn process_block(&mut self, buf: &mut [f32], _sr: u32) {
                for s in buf.iter_mut() {
                    *s *= 1.0 - self.wet;
                }
            }
            fn reset(&mut self) {}
            fn set_mix(&mut self, wet: f32) {
                self.wet = wet.clamp(0.0, 1.0);
            }
        }

        let mut gated = Gated {
            inner: Box::new(Silencer { wet: 1.0 }),
            gate: GateSpec {
                source: GateSource::Tab(drums),
                mode: GateMode::Open,
                depth: 1.0,
                threshold: 0.5,
                release_ms: 20.0,
            },
            base_wet: 1.0,
            env: 0.0,
            sent: -1.0,
        };

        // The drum tab is silent: the gate is shut, so the effect is not
        // applied and the signal comes through untouched.
        levels.reset(drums);
        let mut buf = vec![1.0f32; 64];
        gated.process_block(&mut buf, 48_000);
        assert!(
            (buf[0] - 1.0).abs() < 1e-6,
            "a shut gate is no effect: {}",
            buf[0]
        );

        // A kick lands. The gate opens and the effect is fully in.
        levels.publish(drums, &vec![0.9f32; 64]);
        let mut buf = vec![1.0f32; 64];
        gated.process_block(&mut buf, 48_000);
        assert!(buf[0].abs() < 1e-6, "the kick opened it: {}", buf[0]);

        // Silence again: it falls back over the release rather than snapping,
        // which is what makes it sound like a gate and not like a switch.
        levels.publish(drums, &vec![0.0f32; 64]);
        let mut buf = vec![1.0f32; 64];
        gated.process_block(&mut buf, 48_000);
        assert!(
            buf[0] > 0.0 && buf[0] < 1.0,
            "one block into a 20 ms release: {}",
            buf[0]
        );

        // DUCK is the other way round: the same kick takes the effect *out*,
        // so the signal comes through where OPEN would have removed it.
        gated.gate.mode = GateMode::Duck;
        gated.env = 0.0;
        levels.publish(drums, &vec![0.9f32; 64]);
        let mut buf = vec![1.0f32; 64];
        gated.process_block(&mut buf, 48_000);
        assert!(
            (buf[0] - 1.0).abs() < 1e-6,
            "the kick ducked the effect out: {}",
            buf[0]
        );

        // Depth is how much of the effect the gate owns. At zero it owns none
        // of it, whatever the source is doing.
        gated.gate.mode = GateMode::Open;
        gated.gate.depth = 0.0;
        levels.reset(drums);
        let mut buf = vec![1.0f32; 64];
        gated.process_block(&mut buf, 48_000);
        assert!(
            buf[0].abs() < 1e-6,
            "depth 0 leaves the effect where the user put it: {}",
            buf[0]
        );
        levels.reset(drums);
    }

    /// A CLAP effect that can't be loaded (missing file) becomes a bypass, and
    /// the built-ins around it still build **and keep their positions**.
    ///
    /// Position is the whole contract: `fx_editors[slot][i]`, `fx_loopers`, and
    /// the `fx` of a `SetFxParam` are all the effect's index as the rack draws
    /// it. A chain that came back one short pointed every effect after the hole
    /// at its neighbour.
    #[test]
    fn an_unloadable_fx_becomes_a_bypass_and_keeps_everyone_in_place() {
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
        let chain = build_chain_from_specs(&specs, 48_000, 256);
        assert_eq!(chain.len(), 3, "one processor a spec, hole included");
        assert_eq!(chain[1].name(), "off", "the hole is a bypass");
        assert_ne!(chain[2].name(), "off", "and the reverb is still at 2");

        // A switched-off effect is the same story, and passes the signal.
        let mut off = vec![spec("gain", None), spec("reverb", None)];
        off[0].enabled = false;
        let mut chain = build_chain_from_specs(&off, 48_000, 256);
        assert_eq!(chain.len(), 2);
        let mut buf = vec![0.5f32; 64];
        chain[0].process_block(&mut buf, 48_000);
        assert!(
            buf.iter().all(|s| *s == 0.5),
            "an effect that is off is a wire"
        );
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
    /// A looper's handle survives the wrappers the chain builder puts around
    /// every effect.
    ///
    /// The deck reaches the interface through `loopdeck()` and nowhere else:
    /// the chunks it is fed, the state the panel draws, and the transport REC
    /// drives all ride on it. `Metered` wraps every effect and `Gated` wraps
    /// the ones with a gate, and while neither forwarded it the looper could
    /// be added to a chain, drawn, and pressed — and record nothing, because
    /// the interface had no end of the rings to hold.
    #[test]
    fn a_looper_hands_its_deck_through_the_chain_wrappers() {
        let bare = &mut build_chain_from_specs(&[spec("looper", None)], 48_000, 512)[0];
        assert!(
            bare.loopdeck().is_some(),
            "the meter wrapper swallowed the deck"
        );

        let mut gated = spec("looper", None);
        gated.gate = Some(GateSpec::default());
        let with_gate = &mut build_chain_from_specs(&[gated], 48_000, 512)[0];
        assert!(
            with_gate.loopdeck().is_some(),
            "the gate wrapper swallowed the deck"
        );

        // And it is handed out once: the rings have two ends, not three.
        assert!(
            with_gate.loopdeck().is_none(),
            "a second caller must not get a second end of the same rings"
        );

        // Nothing else in the chain claims to be one.
        let other = &mut build_chain_from_specs(&[spec("delay", None)], 48_000, 512)[0];
        assert!(other.loopdeck().is_none(), "a delay is not a deck");

        // And it says so after the handle is gone, which is what carrying a
        // deck across a rebuild asks it.
        assert!(with_gate.is_loop_deck(), "still a deck without its handle");
        assert!(!other.is_loop_deck());
    }
}
