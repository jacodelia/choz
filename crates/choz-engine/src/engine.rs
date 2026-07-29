//! Audio engine — runs the real-time audio thread with cpal.
//!
//! Auto-detects the best available audio backend using the approach from
//! seqterm: tries JACK first when PipeWire is detected, falls back to ALSA.
//! Both ALSA and JACK work through PipeWire's compatibility layers.
//!
//! RT-safety (patterns copied from seqterm-audio-engine): the audio callback
//! never locks, never allocates, and never builds an FX chain. The UI thread
//! builds the chain and hands the finished `Vec<Box<dyn FxProcessor>>` to the
//! callback over an `rtrb` lock-free ring; the callback swaps it in and ships
//! the old chain back over a second ring so its deallocation happens on the UI
//! thread, not in the RT context. Transport state is a plain `AtomicBool`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use anyhow::{Result, Context};

use crate::fx::FxProcessor;
use crate::fx_chain::{FxSpec, build_chain_from_specs};
use crate::sources::{AudioSource, Sf2Synth, WavPlayer};

type FxChain = Vec<Box<dyn FxProcessor>>;
type Source = Box<dyn AudioSource>;

/// One rack slot: an audio source plus its own post-source FX chain and mixer
/// strip. The RT callback mixes every slot's output together.
struct Slot {
    source: Source,
    fx: FxChain,
    /// Linear output gain (0.0..=2.0).
    gain: f32,
    /// Stereo position, -1.0 = hard left, 0.0 = center, 1.0 = hard right.
    pan: f32,
    mute: bool,
}

impl Slot {
    fn new(source: Source) -> Self {
        Slot { source, fx: Vec::new(), gain: 1.0, pan: 0.0, mute: false }
    }

    /// Constant-power pan law → (left, right) channel gains.
    fn channel_gains(&self) -> (f32, f32) {
        if self.mute {
            return (0.0, 0.0);
        }
        let theta = (self.pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
        (self.gain * theta.cos(), self.gain * theta.sin())
    }
}

/// Commands sent UI → RT over a lock-free ring. Notes name their target slot:
/// the UI resolves input→slot routing (it owns the input↔slot bindings), so the
/// RT side never has to know about MIDI ports or OSC.
enum EngineCommand {
    AddSlot(Source),
    RemoveSlot(usize),
    SetSlotSource { slot: usize, source: Source },
    SetSlotFx { slot: usize, fx: FxChain },
    SetSlotMix { slot: usize, gain: f32, pan: f32, mute: bool },
    SetSlotProgram { slot: usize, bank: u8, preset: u8 },
    /// Live parameter tweak for a slot's *instrument* (hosted plugin).
    SetSlotParam { slot: usize, index: usize, value: f32 },
    /// Live parameter tweak for one FX in a slot's chain — avoids rebuilding
    /// the chain (which, for a hosted plugin, means re-instantiating it).
    SetFxParam { slot: usize, fx: usize, index: usize, value: f32 },
    NoteOn { slot: usize, note: u8, vel: u8 },
    NoteOff { slot: usize, note: u8 },
}

/// Items retired by the RT thread, returned to the UI thread to be dropped
/// off the audio thread (avoids deallocation in the RT context). The payload is
/// never read — only dropped — which is the whole point.
#[allow(dead_code)]
enum Retired {
    Slot(Slot),
    Fx(FxChain),
    Source(Source),
}

/// Max rack slots before the slot Vec would reallocate on the RT thread.
/// ponytail: fixed cap keeps `AddSlot` alloc-free; raise it if anyone needs
/// more than this many simultaneous sources.
const MAX_SLOTS: usize = 32;

/// Which audio backend was selected at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioBackend {
    Alsa,
    Jack,
}

impl AudioBackend {
    pub fn label(&self) -> &'static str {
        match self {
            AudioBackend::Alsa => "ALSA",
            AudioBackend::Jack => "JACK",
        }
    }
}

/// Shared audio engine state (UI-thread side).
pub struct AudioEngine {
    pub sample_rate: u32,
    pub buffer_size: u32,
    pub backend: AudioBackend,
    playing: Arc<AtomicBool>,
    /// Number of rack slots the UI has created (kept in sync with the RT side).
    slot_count: usize,
    /// UI → RT: engine commands (add/remove slot, set fx, notes).
    cmd_tx: rtrb::Producer<EngineCommand>,
    /// RT → UI: retired slots/chains to drop off the audio thread.
    retired_rx: rtrb::Consumer<Retired>,
    /// Name of the output device in use (set once the stream is open).
    output_device: Option<String>,
    /// Backend the user asked for: `AUTO`, `JACK`, `PIPEWIRE` or `ALSA`.
    /// `AUTO` keeps the historical behaviour (JACK when PipeWire is up).
    backend_pref: String,
    /// RT endpoints, taken by `start()` and moved into the callback.
    rt_endpoints: Option<RtEndpoints>,
    _stream: Option<cpal::Stream>,
}

/// The consumer/producer ends the RT callback owns.
struct RtEndpoints {
    cmd_rx: rtrb::Consumer<EngineCommand>,
    retired_tx: rtrb::Producer<Retired>,
}

/// State owned by the real-time audio callback. No locks, no allocation.
struct RtState {
    playing: Arc<AtomicBool>,
    cmd_rx: rtrb::Consumer<EngineCommand>,
    retired_tx: rtrb::Producer<Retired>,
    slots: Vec<Slot>,
    /// Pre-allocated per-slot mix scratch (interleaved stereo).
    scratch: Vec<f32>,
    sample_rate: u32,
}

// ─── PipeWire / sound server detection ─────────────────────────────────────

/// Returns true when a PipeWire daemon is reachable on this session.
///
/// Checks `$XDG_RUNTIME_DIR/pipewire-0` (Linux) or fallback
/// `/run/user/<uid>/pipewire-0`. No client connection — filesystem check only.
fn pipewire_is_running() -> bool {
    let pipewire_socket = runtime_dir().join("pipewire-0");
    std::env::var("PIPEWIRE_REMOTE").is_ok()
        || pipewire_socket.exists()
}

fn runtime_dir() -> std::path::PathBuf {
    std::env::var("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let uid = std::process::id(); // not perfect but avoids libc
            std::path::PathBuf::from(format!("/run/user/{uid}"))
        })
}

/// Human-readable label of the detected sound server.
fn detect_sound_server() -> &'static str {
    if pipewire_is_running() {
        return "PipeWire";
    }
    if std::env::var("PULSE_RUNTIME_PATH").is_ok()
        || std::env::var("PULSE_SERVER").is_ok()
    {
        return "PulseAudio";
    }
    "ALSA"
}

impl AudioEngine {
    pub fn new(sample_rate: u32, buffer_size: u32) -> Self {
        // Small SPSC rings: a handful of pending rebuilds is plenty for a human
        // clicking knobs; a full ring just drops the excess without blocking RT.
        let (cmd_tx, cmd_rx) = rtrb::RingBuffer::new(128);
        let (retired_tx, retired_rx) = rtrb::RingBuffer::new(64);
        Self {
            sample_rate,
            buffer_size,
            backend: AudioBackend::Alsa,
            playing: Arc::new(AtomicBool::new(false)),
            slot_count: 0,
            cmd_tx,
            retired_rx,
            output_device: None,
            backend_pref: "AUTO".to_string(),
            rt_endpoints: Some(RtEndpoints { cmd_rx, retired_tx }),
            _stream: None,
        }
    }

    /// Start the audio stream. Strategy (mirrors seqterm):
    ///
    /// 1. Detect PipeWire: if running, prefer JACK backend with PipeWire's
    ///    JACK compatibility layer (sets PIPEWIRE_QUANTUM for correct buffer).
    /// 2. Fallback: use ALSA (works through PipeWire's ALSA compat too).
    /// 3. If none work, return installation hints.
    pub fn start(&mut self) -> Result<()> {
        let sound_server = detect_sound_server();

        // Pick backend + device + config BEFORE we touch the RT ring endpoints,
        // so a failed JACK probe can cleanly fall through to ALSA.
        let (device, config, backend) = self
            .pick_backend(sound_server, self.output_device.as_deref())
            .context(no_backend_hint(sound_server))?;

        let ep = self
            .rt_endpoints
            .take()
            .context("audio engine already started")?;

        let scratch_frames = (self.buffer_size.max(8192) as usize) * 2;
        let state = RtState {
            playing: Arc::clone(&self.playing),
            cmd_rx: ep.cmd_rx,
            retired_tx: ep.retired_tx,
            slots: Vec::with_capacity(MAX_SLOTS),
            scratch: vec![0.0; scratch_frames],
            sample_rate: self.sample_rate,
        };

        self.output_device = device.name().ok();
        let stream = build_stream(&device, &config, state)?;
        stream.play().context("failed to start audio stream")?;
        self._stream = Some(stream);
        self.backend = backend;
        eprintln!(
            "choz: using {} backend via {sound_server} (sr={}, buf={}, out={})",
            backend.label(), self.sample_rate, self.buffer_size,
            self.output_device.as_deref().unwrap_or("default"),
        );
        Ok(())
    }

    /// Output devices offered by the current backend, by name.
    pub fn output_devices(&self) -> Vec<String> {
        let host = if self.backend == AudioBackend::Jack {
            cpal::host_from_id(cpal::HostId::Jack).unwrap_or_else(|_| cpal::default_host())
        } else {
            cpal::default_host()
        };
        match host.output_devices() {
            Ok(devs) => devs.filter_map(|d| d.name().ok()).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Name of the device audio is currently going to.
    pub fn output_device(&self) -> Option<&str> {
        self.output_device.as_deref()
    }

    /// Move playback to `name`. The old stream is torn down, so **every rack
    /// slot is lost** — the caller must re-add them (the UI owns the rack model
    /// and reloads it). Returns an error and keeps the old stream if the device
    /// can't be opened.
    pub fn set_output_device(&mut self, name: &str) -> Result<()> {
        let sound_server = detect_sound_server();
        let (device, config, backend) = self
            .pick_backend(sound_server, Some(name))
            .with_context(|| format!("cannot open output device '{name}'"))?;

        // Fresh rings: the old ones are owned by the callback we're dropping.
        let (cmd_tx, cmd_rx) = rtrb::RingBuffer::new(128);
        let (retired_tx, retired_rx) = rtrb::RingBuffer::new(64);
        let scratch_frames = (self.buffer_size.max(8192) as usize) * 2;
        let state = RtState {
            playing: Arc::clone(&self.playing),
            cmd_rx,
            retired_tx,
            slots: Vec::with_capacity(MAX_SLOTS),
            scratch: vec![0.0; scratch_frames],
            sample_rate: self.sample_rate,
        };
        let stream = build_stream(&device, &config, state)?;
        stream.play().context("failed to start audio stream")?;

        self._stream = Some(stream); // drops the previous stream (and its slots)
        self.cmd_tx = cmd_tx;
        self.retired_rx = retired_rx;
        self.slot_count = 0;
        self.backend = backend;
        self.output_device = device.name().ok();
        Ok(())
    }

    /// Choose a working (device, config, backend) without consuming RT state.
    /// `want` names a specific output device; `None` takes the host default.
    /// Ask for a specific backend on the next [`Self::start`]. Values other
    /// than `AUTO`/`JACK`/`PIPEWIRE`/`ALSA` are ignored.
    pub fn set_backend_preference(&mut self, pref: &str) {
        let pref = pref.to_uppercase();
        if matches!(pref.as_str(), "AUTO" | "JACK" | "PIPEWIRE" | "ALSA") {
            self.backend_pref = pref;
        }
    }

    /// Ask for a specific output device on the next [`Self::start`].
    pub fn set_output_device_preference(&mut self, name: &str) {
        self.output_device = Some(name.to_string());
    }

    pub fn backend_preference(&self) -> &str {
        &self.backend_pref
    }

    fn pick_backend(
        &self,
        sound_server: &str,
        want: Option<&str>,
    ) -> Result<(cpal::Device, cpal::StreamConfig, AudioBackend)> {
        // An explicit ALSA choice skips the JACK probe entirely.
        if self.backend_pref == "ALSA" {
            let (dev, cfg) = self.alsa_device_config(want)?;
            return Ok((dev, cfg, AudioBackend::Alsa));
        }
        let force_jack = matches!(self.backend_pref.as_str(), "JACK" | "PIPEWIRE");
        if force_jack || pipewire_is_running() {
            // PIPEWIRE_QUANTUM must be set before the JACK client is created so
            // PipeWire uses our exact buffer size.
            let quantum_str = format!("{}/{}", self.buffer_size, self.sample_rate);
            unsafe { std::env::set_var("PIPEWIRE_QUANTUM", &quantum_str); }
            eprintln!("choz: PipeWire detected — setting PIPEWIRE_QUANTUM={quantum_str}");

            match self.jack_device_config(want) {
                Ok((dev, cfg)) => return Ok((dev, cfg, AudioBackend::Jack)),
                Err(e) => eprintln!(
                    "choz: JACK backend unavailable ({e}), trying ALSA via {sound_server}..."
                ),
            }
        }
        let (dev, cfg) = self.alsa_device_config(want)?;
        Ok((dev, cfg, AudioBackend::Alsa))
    }

    fn jack_device_config(&self, want: Option<&str>) -> Result<(cpal::Device, cpal::StreamConfig)> {
        let host = cpal::host_from_id(cpal::HostId::Jack)
            .context("JACK host not available (is libjack installed?)")?;
        let device = named_or_default(&host, want)
            .context("no JACK output device found")?;
        let config = pick_config(&device, self.sample_rate, self.buffer_size)?;
        Ok((device, config))
    }

    fn alsa_device_config(&self, want: Option<&str>) -> Result<(cpal::Device, cpal::StreamConfig)> {
        let device = named_or_default(&cpal::default_host(), want)
            .context("no output audio device found")?;
        let config = pick_config(&device, self.sample_rate, self.buffer_size)?;
        Ok((device, config))
    }

    /// Number of rack slots.
    pub fn slot_count(&self) -> usize {
        self.slot_count
    }

    fn drain_retired(&mut self) {
        while let Ok(retired) = self.retired_rx.pop() {
            drop(retired);
        }
    }

    fn send(&mut self, cmd: EngineCommand) {
        self.drain_retired();
        // A full ring means the RT thread is behind; drop rather than block.
        let _ = self.cmd_tx.push(cmd);
    }

    /// Append a source as a new rack slot. Returns its index. No-op past
    /// [`MAX_SLOTS`].
    fn add_slot(&mut self, source: Source) -> Option<usize> {
        if self.slot_count >= MAX_SLOTS {
            return None;
        }
        let idx = self.slot_count;
        self.send(EngineCommand::AddSlot(source));
        self.slot_count += 1;
        Some(idx)
    }

    /// Remove the slot at `slot`. Later slots shift down by one.
    pub fn remove_slot(&mut self, slot: usize) {
        if slot < self.slot_count {
            self.send(EngineCommand::RemoveSlot(slot));
            self.slot_count -= 1;
        }
    }

    /// Rebuild slot `slot`'s FX chain from specs (built off the RT thread).
    pub fn set_slot_fx(&mut self, slot: usize, specs: Vec<FxSpec>) {
        if slot >= self.slot_count {
            return;
        }
        let fx = build_chain_from_specs(&specs, self.sample_rate, self.buffer_size);
        self.send(EngineCommand::SetSlotFx { slot, fx });
    }

    /// Set slot `slot`'s mixer strip: linear `gain`, `pan` (-1 left .. 1 right)
    /// and `mute`.
    pub fn set_slot_mix(&mut self, slot: usize, gain: f32, pan: f32, mute: bool) {
        if slot >= self.slot_count {
            return;
        }
        self.send(EngineCommand::SetSlotMix { slot, gain, pan, mute });
    }

    /// Change one parameter of the FX at `fx` in slot `slot`'s chain, without
    /// rebuilding the chain. `value` is a normalised 0..1 knob position.
    pub fn set_fx_param(&mut self, slot: usize, fx: usize, index: usize, value: f32) {
        if slot >= self.slot_count {
            return;
        }
        self.send(EngineCommand::SetFxParam { slot, fx, index, value });
    }

    /// Change one parameter of slot `slot`'s instrument. `value` is a normalised
    /// 0..1 knob position; sources without parameters ignore it.
    pub fn set_slot_param(&mut self, slot: usize, index: usize, value: f32) {
        if slot >= self.slot_count {
            return;
        }
        self.send(EngineCommand::SetSlotParam { slot, index, value });
    }

    /// Select a bank/preset on slot `slot` (SF2 program change).
    pub fn set_slot_program(&mut self, slot: usize, bank: u8, preset: u8) {
        if slot >= self.slot_count {
            return;
        }
        self.send(EngineCommand::SetSlotProgram { slot, bank, preset });
    }

    /// Replace slot `slot`'s source. The old one is dropped off the RT thread.
    fn set_slot_source(&mut self, slot: usize, source: Source) {
        if slot >= self.slot_count {
            return;
        }
        self.send(EngineCommand::SetSlotSource { slot, source });
    }

    /// Add a slot with no instrument yet (renders silence). Used when a rack tab
    /// is created for an input before an instrument is chosen.
    pub fn add_silent(&mut self) -> Option<usize> {
        self.add_slot(Box::new(crate::sources::Silence))
    }

    /// Add a WAV-playback slot.
    pub fn add_wav(&mut self, path: &std::path::Path, looping: bool) -> Result<Option<usize>> {
        let player = WavPlayer::load(path, looping)?;
        Ok(self.add_slot(Box::new(player)))
    }

    /// Add an SF2 SoundFont instrument slot (MIDI-playable).
    pub fn add_sf2(&mut self, path: &std::path::Path, bank: u8, preset: u8) -> Result<Option<usize>> {
        let synth = Sf2Synth::load(path, bank, preset, self.sample_rate)?;
        Ok(self.add_slot(Box::new(synth)))
    }

    /// Load a WAV as slot `slot`'s source, replacing whatever was there.
    pub fn load_wav(&mut self, slot: usize, path: &std::path::Path, looping: bool) -> Result<()> {
        let player = WavPlayer::load(path, looping)?;
        self.set_slot_source(slot, Box::new(player));
        Ok(())
    }

    /// Load an SF2 as slot `slot`'s source, replacing whatever was there.
    pub fn load_sf2(&mut self, slot: usize, path: &std::path::Path, bank: u8, preset: u8) -> Result<()> {
        let synth = Sf2Synth::load(path, bank, preset, self.sample_rate)?;
        self.set_slot_source(slot, Box::new(synth));
        Ok(())
    }

    /// Scan every configured plugin directory, all formats.
    pub fn scan_plugins(&self, paths: &crate::PluginPaths) -> Vec<crate::FoundPlugin> {
        crate::scan_all(paths)
    }

    /// Discovered plugins, served from the on-disk cache when it's still fresh.
    /// Use this at startup; [`Self::rescan_plugins`] for an explicit refresh.
    pub fn cached_plugins(&self, paths: &crate::PluginPaths) -> Vec<crate::FoundPlugin> {
        // The config file counts as an input: editing the search paths must
        // invalidate a cache written before the edit.
        let mut dirs = paths.all_enabled();
        dirs.push(crate::PluginPaths::config_file());
        crate::cache::cached_or_scan(&dirs, || self.scan_plugins(paths))
    }

    /// Force a full scan and refresh the cache.
    pub fn rescan_plugins(&self, paths: &crate::PluginPaths) -> Vec<crate::FoundPlugin> {
        crate::cache::rescan(|| self.scan_plugins(paths))
    }

    /// Add a CLAP instrument slot. Requires the `clap` feature.
    #[cfg(feature = "clap")]
    pub fn add_clap(&mut self, path: &std::path::Path, plugin_id: &str) -> Result<Option<usize>> {
        let inst = choz_plugin_clap::host::ClapInstrument::build(
            path, plugin_id, self.sample_rate, self.buffer_size,
        )
        .ok_or_else(|| anyhow::anyhow!("failed to instantiate CLAP plugin: {plugin_id}"))?;
        Ok(self.add_slot(Box::new(inst)))
    }

    #[cfg(not(feature = "clap"))]
    pub fn add_clap(&mut self, _path: &std::path::Path, _plugin_id: &str) -> Result<Option<usize>> {
        anyhow::bail!("CLAP support not compiled in (rebuild with --features clap)")
    }

    /// Load a CLAP instrument as slot `slot`'s source. Requires the `clap` feature.
    #[cfg(feature = "clap")]
    pub fn load_clap(&mut self, slot: usize, path: &std::path::Path, plugin_id: &str) -> Result<()> {
        let inst = choz_plugin_clap::host::ClapInstrument::build(
            path, plugin_id, self.sample_rate, self.buffer_size,
        )
        .ok_or_else(|| anyhow::anyhow!("failed to instantiate CLAP plugin: {plugin_id}"))?;
        self.set_slot_source(slot, Box::new(inst));
        Ok(())
    }

    #[cfg(not(feature = "clap"))]
    pub fn load_clap(&mut self, _slot: usize, _path: &std::path::Path, _id: &str) -> Result<()> {
        anyhow::bail!("CLAP support not compiled in (rebuild with --features clap)")
    }

    /// Send a note-on to one slot. Input→slot routing lives in the UI.
    pub fn note_on(&mut self, slot: usize, note: u8, vel: u8) {
        self.send(EngineCommand::NoteOn { slot, note, vel });
    }

    pub fn note_off(&mut self, slot: usize, note: u8) {
        self.send(EngineCommand::NoteOff { slot, note });
    }

    pub fn set_playing(&self, play: bool) {
        self.playing.store(play, Ordering::Relaxed);
    }
}

/// Real-time callback body. Runs on the audio thread — no locks, no alloc.
fn audio_callback(buf: &mut [f32], state: &mut RtState) {
    // Apply pending commands to the slot list. Retired slots/chains go back over
    // retired_tx so they are freed on the UI thread, not here.
    while let Ok(cmd) = state.cmd_rx.pop() {
        match cmd {
            EngineCommand::AddSlot(source) => {
                if state.slots.len() < MAX_SLOTS {
                    state.slots.push(Slot::new(source));
                } else {
                    let _ = state.retired_tx.push(Retired::Slot(Slot::new(source)));
                }
            }
            EngineCommand::RemoveSlot(i) => {
                if i < state.slots.len() {
                    let old = state.slots.remove(i);
                    let _ = state.retired_tx.push(Retired::Slot(old));
                }
            }
            EngineCommand::SetSlotSource { slot, source } => {
                if let Some(s) = state.slots.get_mut(slot) {
                    let old = std::mem::replace(&mut s.source, source);
                    let _ = state.retired_tx.push(Retired::Source(old));
                } else {
                    let _ = state.retired_tx.push(Retired::Source(source));
                }
            }
            EngineCommand::SetSlotFx { slot, fx } => {
                if let Some(s) = state.slots.get_mut(slot) {
                    let old = std::mem::replace(&mut s.fx, fx);
                    let _ = state.retired_tx.push(Retired::Fx(old));
                } else {
                    let _ = state.retired_tx.push(Retired::Fx(fx));
                }
            }
            EngineCommand::SetSlotMix { slot, gain, pan, mute } => {
                if let Some(s) = state.slots.get_mut(slot) {
                    s.gain = gain;
                    s.pan = pan;
                    s.mute = mute;
                }
            }
            EngineCommand::SetFxParam { slot, fx, index, value } => {
                if let Some(p) = state.slots.get_mut(slot).and_then(|s| s.fx.get_mut(fx)) {
                    if index == crate::FX_MIX_PARAM {
                        p.set_mix(value);
                    } else {
                        p.set_param(index, value);
                    }
                }
            }
            EngineCommand::SetSlotParam { slot, index, value } => {
                if let Some(s) = state.slots.get_mut(slot) {
                    s.source.set_param(index, value);
                }
            }
            EngineCommand::SetSlotProgram { slot, bank, preset } => {
                if let Some(s) = state.slots.get_mut(slot) {
                    s.source.program_change(bank, preset);
                }
            }
            // Notes are addressed to one slot; the UI already decided which.
            EngineCommand::NoteOn { slot, note, vel } => {
                if let Some(s) = state.slots.get_mut(slot) {
                    s.source.note_on(note, vel);
                }
            }
            EngineCommand::NoteOff { slot, note } => {
                if let Some(s) = state.slots.get_mut(slot) {
                    s.source.note_off(note);
                }
            }
        }
    }

    buf.fill(0.0);
    let playing = state.playing.load(Ordering::Relaxed);
    let n = buf.len().min(state.scratch.len());
    let sr = state.sample_rate;

    // Mix every slot: render its source into scratch, run its FX, sum into buf.
    for slot in state.slots.iter_mut() {
        // Synths always render (envelope tails / live keys); generators (tone,
        // WAV) honor the transport play flag.
        if !playing && !slot.source.plays_on_transport_stop() {
            continue;
        }
        let sc = &mut state.scratch[..n];
        sc.fill(0.0);
        let written = slot.source.render(sc, sr);
        for s in sc[written * 2..].iter_mut() {
            *s = 0.0;
        }
        for fx in slot.fx.iter_mut() {
            fx.process_block(sc, sr);
        }
        // Muted slots still render (so envelopes/playheads keep moving) but sum
        // in at zero gain.
        let (gl, gr) = slot.channel_gains();
        for (i, (o, s)) in buf[..n].iter_mut().zip(sc.iter()).enumerate() {
            *o += *s * if i % 2 == 0 { gl } else { gr };
        }
    }
}

// ─── Stream / config helpers ───────────────────────────────────────────────

fn build_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut state: RtState,
) -> Result<cpal::Stream> {
    device
        .build_output_stream(
            config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                audio_callback(data, &mut state);
            },
            |err| eprintln!("Audio error: {err}"),
            None,
        )
        .context("failed to build output stream")
}

/// The named output device, or the host default when `want` is `None` or the
/// name isn't there any more (device disappeared between listing and opening).
fn named_or_default(host: &cpal::Host, want: Option<&str>) -> Option<cpal::Device> {
    if let Some(name) = want {
        let found = host
            .output_devices()
            .ok()?
            .find(|d| d.name().is_ok_and(|n| n == name));
        if let Some(dev) = found {
            return Some(dev);
        }
        eprintln!("choz: output device '{name}' not found, using the default");
    }
    host.default_output_device()
}

/// Pick the best supported stream config.
fn pick_config(
    device: &cpal::Device,
    target_rate: u32,
    target_buffer: u32,
) -> Result<cpal::StreamConfig> {
    let target_rate = cpal::SampleRate(target_rate);

    let supported = device
        .supported_output_configs()
        .context("cannot query supported configs")?;

    let mut best: Option<cpal::SupportedStreamConfig> = None;

    for cfg in supported {
        let range = cfg.min_sample_rate()..=cfg.max_sample_rate();
        if range.contains(&target_rate) {
            best = Some(cfg.with_sample_rate(target_rate));
            break;
        }
        if best.is_none() {
            best = Some(cfg.with_max_sample_rate());
        }
    }

    let supported = best.context("no compatible sample rate found")?;

    let buffer_size = match supported.buffer_size() {
        cpal::SupportedBufferSize::Range { min, max } => {
            if target_buffer >= *min && target_buffer <= *max {
                cpal::BufferSize::Fixed(target_buffer)
            } else {
                cpal::BufferSize::Default
            }
        }
        cpal::SupportedBufferSize::Unknown => cpal::BufferSize::Default,
    };

    Ok(cpal::StreamConfig {
        channels: 2,
        sample_rate: supported.sample_rate(),
        buffer_size,
    })
}

/// Multi-line install hint shown when no backend could be opened.
fn no_backend_hint(sound_server: &str) -> String {
    format!(
        "No audio backend available.\n\n\
         Sound server detected: {sound_server}\n\n\
         Installation hints:\n\
         \x20 Ubuntu/Debian:  sudo apt install libasound2-dev libjack-dev\n\
         \x20 Fedora:         sudo dnf install alsa-lib-devel jack-audio-connection-kit-devel\n\
         \x20 Arch:           sudo pacman -S alsa-lib jack2\n\
         \nPipeWire users — install compatibility layers:\n\
         \x20 Ubuntu/Debian:  sudo apt install pipewire-alsa pipewire-jack\n\
         \x20 Fedora:         sudo dnf install pipewire-alsa pipewire-jack-audio-connection-kit\n\
         \x20 Arch:           sudo pacman -S pipewire-alsa pipewire-jack"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Adds a constant to every sample — enough to prove the chain ran.
    struct AddFx(f32);
    impl FxProcessor for AddFx {
        fn process_block(&mut self, buf: &mut [f32], _sr: u32) {
            for s in buf.iter_mut() { *s += self.0; }
        }
        fn reset(&mut self) {}
        fn set_mix(&mut self, _wet: f32) {}
    }

    /// A source that renders a constant DC level — makes mixing observable.
    struct DcSource(f32);
    impl AudioSource for DcSource {
        fn render(&mut self, out: &mut [f32], _sr: u32) -> usize {
            out.fill(self.0);
            out.len() / 2
        }
        fn plays_on_transport_stop(&self) -> bool { true }
    }

    /// Records note events it receives; proves omni routing reaches it.
    struct RecordingSynth(std::sync::Arc<parking_lot::Mutex<Vec<(bool, u8)>>>);
    impl AudioSource for RecordingSynth {
        fn render(&mut self, out: &mut [f32], _sr: u32) -> usize {
            out.fill(0.0);
            out.len() / 2
        }
        fn note_on(&mut self, note: u8, _vel: u8) { self.0.lock().push((true, note)); }
        fn note_off(&mut self, note: u8) { self.0.lock().push((false, note)); }
        /// Recorded as note 200+preset so it can't be confused with a note event.
        fn program_change(&mut self, _bank: u8, preset: u8) { self.0.lock().push((true, 200 + preset)); }
        fn plays_on_transport_stop(&self) -> bool { true }
    }

    fn mk_state() -> (rtrb::Producer<EngineCommand>, rtrb::Consumer<Retired>, RtState) {
        let (cmd_tx, cmd_rx) = rtrb::RingBuffer::new(32);
        let (retired_tx, retired_rx) = rtrb::RingBuffer::new(32);
        let state = RtState {
            playing: Arc::new(AtomicBool::new(true)),
            cmd_rx,
            retired_tx,
            slots: Vec::with_capacity(MAX_SLOTS),
            scratch: vec![0.0; 64],
            sample_rate: 48_000,
        };
        (cmd_tx, retired_rx, state)
    }

    #[test]
    fn mixes_slots_and_applies_per_slot_fx() {
        let (mut cmd_tx, _retired, mut state) = mk_state();

        cmd_tx.push(EngineCommand::AddSlot(Box::new(DcSource(0.25)))).unwrap();
        cmd_tx.push(EngineCommand::AddSlot(Box::new(DcSource(0.25)))).unwrap();
        // Give slot 1 a +1.0 FX.
        cmd_tx
            .push(EngineCommand::SetSlotFx { slot: 1, fx: vec![Box::new(AddFx(1.0))] })
            .unwrap();

        let mut buf = [0.0f32; 8];
        audio_callback(&mut buf, &mut state);
        // slot0 = 0.25, slot1 = 0.25 + 1.0 = 1.25 → sum 1.5, then the default
        // centered constant-power pan (-3 dB) scales both channels.
        let expect = 1.5 * std::f32::consts::FRAC_1_SQRT_2;
        assert!(buf.iter().all(|&s| (s - expect).abs() < 1e-6), "got {buf:?}");
    }

    #[test]
    fn mixer_strip_applies_gain_pan_mute() {
        let (mut cmd_tx, _retired, mut state) = mk_state();
        cmd_tx.push(EngineCommand::AddSlot(Box::new(DcSource(1.0)))).unwrap();
        // Half gain, hard left.
        cmd_tx.push(EngineCommand::SetSlotMix { slot: 0, gain: 0.5, pan: -1.0, mute: false }).unwrap();

        let mut buf = [0.0f32; 8];
        audio_callback(&mut buf, &mut state);
        assert!(buf.iter().step_by(2).all(|&s| (s - 0.5).abs() < 1e-6), "left = gain, got {buf:?}");
        assert!(buf.iter().skip(1).step_by(2).all(|&s| s.abs() < 1e-6), "right silent, got {buf:?}");

        cmd_tx.push(EngineCommand::SetSlotMix { slot: 0, gain: 0.5, pan: -1.0, mute: true }).unwrap();
        audio_callback(&mut buf, &mut state);
        assert!(buf.iter().all(|&s| s == 0.0), "mute silences the slot, got {buf:?}");
    }

    #[test]
    fn notes_reach_only_their_target_slot() {
        let (mut cmd_tx, _retired, mut state) = mk_state();
        let a = std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
        let b = std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
        cmd_tx.push(EngineCommand::AddSlot(Box::new(RecordingSynth(a.clone())))).unwrap();
        cmd_tx.push(EngineCommand::AddSlot(Box::new(RecordingSynth(b.clone())))).unwrap();
        cmd_tx.push(EngineCommand::NoteOn { slot: 1, note: 60, vel: 100 }).unwrap();

        let mut buf = [0.0f32; 8];
        audio_callback(&mut buf, &mut state);
        assert!(a.lock().is_empty(), "slot 0 is bound to another input");
        assert_eq!(&*b.lock(), &[(true, 60)]);

        // Out-of-range targets are dropped, not panics.
        cmd_tx.push(EngineCommand::NoteOn { slot: 99, note: 62, vel: 100 }).unwrap();
        cmd_tx.push(EngineCommand::SetSlotProgram { slot: 1, bank: 0, preset: 42 }).unwrap();
        audio_callback(&mut buf, &mut state);
        assert!(a.lock().is_empty());
        assert_eq!(b.lock().last(), Some(&(true, 200 + 42)), "slot 1 got the program change");
    }

    #[test]
    fn set_slot_source_swaps_and_retires_the_old_one() {
        let (mut cmd_tx, mut retired_rx, mut state) = mk_state();
        cmd_tx.push(EngineCommand::AddSlot(Box::new(DcSource(0.5)))).unwrap();
        cmd_tx.push(EngineCommand::SetSlotSource { slot: 0, source: Box::new(DcSource(0.25)) }).unwrap();

        let mut buf = [0.0f32; 8];
        audio_callback(&mut buf, &mut state);
        let expect = 0.25 * std::f32::consts::FRAC_1_SQRT_2;
        assert!(buf.iter().all(|&s| (s - expect).abs() < 1e-6), "new source is live, got {buf:?}");
        assert!(matches!(retired_rx.pop(), Ok(Retired::Source(_))), "old source dropped off-RT");
    }

    #[test]
    fn remove_slot_returns_it_off_rt() {
        let (mut cmd_tx, mut retired_rx, mut state) = mk_state();
        cmd_tx.push(EngineCommand::AddSlot(Box::new(DcSource(0.5)))).unwrap();
        cmd_tx.push(EngineCommand::RemoveSlot(0)).unwrap();

        let mut buf = [0.0f32; 8];
        audio_callback(&mut buf, &mut state);
        assert!(state.slots.is_empty(), "slot removed");
        assert!(matches!(retired_rx.pop(), Ok(Retired::Slot(_))), "removed slot returned for off-RT drop");
        assert!(buf.iter().all(|&s| s == 0.0), "empty rack is silent");
    }
}

