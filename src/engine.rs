//! Audio engine — runs the real-time audio thread with cpal.
//!
//! Auto-detects the best available audio backend using the approach from
//! seqterm: tries JACK first when PipeWire is detected, falls back to ALSA.
//! Both ALSA and JACK work through PipeWire's compatibility layers.
//!
//! Processes audio through the source and FX chain, then outputs stereo.

use std::sync::Arc;
use std::cell::Cell;
use parking_lot::Mutex;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use anyhow::{Result, Context};

use crate::fx::FxProcessor;
use crate::fx_chain::{FxSpec, build_chain_from_specs};

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

/// Shared audio engine state.
pub struct AudioEngine {
    pub sample_rate: u32,
    pub buffer_size: u32,
    pub playing: Arc<Mutex<bool>>,
    pub fx_chain: Arc<Mutex<Vec<Box<dyn FxProcessor>>>>,
    pub pending_specs: Arc<Mutex<Option<Vec<FxSpec>>>>,
    pub backend: AudioBackend,
    _stream: Option<cpal::Stream>,
}

/// State shared with the real-time audio callback (lock-free where possible).
struct AudioState {
    playing: Arc<Mutex<bool>>,
    fx_chain: Arc<Mutex<Vec<Box<dyn FxProcessor>>>>,
    pending_specs: Arc<Mutex<Option<Vec<FxSpec>>>>,
    sample_rate: u32,
    phase: Cell<f32>,
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
        Self {
            sample_rate,
            buffer_size,
            playing: Arc::new(Mutex::new(false)),
            fx_chain: Arc::new(Mutex::new(Vec::new())),
            pending_specs: Arc::new(Mutex::new(None)),
            backend: AudioBackend::Alsa,
            _stream: None,
        }
    }

    /// Start the audio stream. Strategy (mirrors seqterm):
    ///
    /// 1. Detect PipeWire: if running, prefer JACK backend with PipeWire's
    ///    JACK compatibility layer (sets PIPEWIRE_QUANTUM for correct buffer).
    /// 2. Fallback: use ALSA (works through PipeWire's ALSA compat too).
    /// 3. If none work, prints installation hints.
    pub fn start(&mut self) -> Result<()> {
        let sound_server = detect_sound_server();

        // Build shared state for the callback
        let state = AudioState {
            playing: Arc::clone(&self.playing),
            fx_chain: Arc::clone(&self.fx_chain),
            pending_specs: Arc::clone(&self.pending_specs),
            sample_rate: self.sample_rate,
            phase: Cell::new(0.0),
        };

        // Prefer JACK when PipeWire is running (seqterm approach)
        if pipewire_is_running() {
            // Set PIPEWIRE_QUANTUM so PipeWire uses the exact buffer size
            // MUST be set before the JACK client is created
            let quantum_str = format!("{}/{}", self.buffer_size, self.sample_rate);
            unsafe {
                std::env::set_var("PIPEWIRE_QUANTUM", &quantum_str);
            }
            eprintln!(
                "choz: PipeWire detected — setting PIPEWIRE_QUANTUM={quantum_str}"
            );

            match open_jack_stream(&state, self.sample_rate, self.buffer_size) {
                Ok(stream) => {
                    self._stream = Some(stream);
                    self.backend = AudioBackend::Jack;
                    eprintln!(
                        "choz: using JACK backend via {sound_server} (sr={}, buf={})",
                        self.sample_rate, self.buffer_size,
                    );
                    return Ok(());
                }
                Err(e) => {
                    eprintln!("choz: JACK backend unavailable ({e}), trying ALSA...");
                }
            }
        }

        // Fallback: ALSA (works through PipeWire ALSA compat, or native ALSA)
        match open_alsa_stream(&state, self.sample_rate, self.buffer_size) {
            Ok(stream) => {
                self._stream = Some(stream);
                self.backend = AudioBackend::Alsa;
                eprintln!(
                    "choz: using ALSA backend via {sound_server} (sr={}, buf={})",
                    self.sample_rate, self.buffer_size,
                );
                Ok(())
            }
            Err(e) => {
                let mut msg = String::new();
                msg.push_str("No audio backend available.\n\n");
                msg.push_str(&format!("Sound server detected: {sound_server}\n\n"));
                msg.push_str(&format!("Last error: {e}\n\n"));
                msg.push_str("Installation hints:\n");
                msg.push_str("  Ubuntu/Debian:  sudo apt install libasound2-dev libjack-dev\n");
                msg.push_str("  Fedora:         sudo dnf install alsa-lib-devel jack-audio-connection-kit-devel\n");
                msg.push_str("  Arch:           sudo pacman -S alsa-lib jack2\n");
                msg.push_str("\nPipeWire users — install compatibility layers:\n");
                msg.push_str("  Ubuntu/Debian:  sudo apt install pipewire-alsa pipewire-jack\n");
                msg.push_str("  Fedora:         sudo dnf install pipewire-alsa pipewire-jack-audio-connection-kit\n");
                msg.push_str("  Arch:           sudo pacman -S pipewire-alsa pipewire-jack\n");
                Err(anyhow::anyhow!(msg))
            }
        }
    }

    fn audio_callback(buf: &mut [f32], state: &AudioState) {
        // Rebuild FX chain if specs changed (requested from non-RT thread)
        if let Some(specs) = state.pending_specs.lock().take() {
            let new_chain = build_chain_from_specs(&specs, state.sample_rate);
            *state.fx_chain.lock() = new_chain;
        }

        let playing = *state.playing.lock();

        if playing {
            let freq = 440.0;
            let sr = state.sample_rate as f32;
            let frames = buf.len() / 2;
            for i in 0..frames {
                let phase = state.phase.get();
                state.phase.set((phase + freq / sr) % 1.0);
                let s = (2.0 * std::f32::consts::PI * phase).sin() * 0.3;
                buf[i * 2]     = s;
                buf[i * 2 + 1] = s;
            }
        } else {
            buf.fill(0.0);
        }

        let mut chain = state.fx_chain.lock();
        for fx in chain.iter_mut() {
            fx.process_block(buf, state.sample_rate);
        }
    }

    pub fn rebuild_fx_chain(&self, specs: Vec<FxSpec>) {
        *self.pending_specs.lock() = Some(specs);
    }

    pub fn set_playing(&self, play: bool) {
        *self.playing.lock() = play;
    }
}

// ─── Backend open helpers ──────────────────────────────────────────────────

/// Open a JACK output stream. Uses JACK host from cpal.
fn open_jack_stream(
    state: &AudioState,
    sample_rate: u32,
    buffer_size: u32,
) -> Result<cpal::Stream> {
    let host = cpal::host_from_id(cpal::HostId::Jack)
        .context("JACK host not available (is libjack installed?)")?;

    let device = host
        .default_output_device()
        .context("no JACK output device found")?;

    let config = pick_config(&device, sample_rate, buffer_size)?;

    let stream = build_stream(&device, &config, state)?;
    stream.play().context("failed to start JACK stream")?;
    Ok(stream)
}

/// Open an ALSA output stream (or default cpal host).
fn open_alsa_stream(
    state: &AudioState,
    sample_rate: u32,
    buffer_size: u32,
) -> Result<cpal::Stream> {
    let host = cpal::default_host();

    let device = host
        .default_output_device()
        .context("no output audio device found")?;

    let config = pick_config(&device, sample_rate, buffer_size)?;

    let stream = build_stream(&device, &config, state)?;
    stream.play().context("failed to start stream")?;
    Ok(stream)
}

fn build_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    state: &AudioState,
) -> Result<cpal::Stream> {
    let playing = state.playing.clone();
    let fx_chain = state.fx_chain.clone();
    let pending_specs = state.pending_specs.clone();
    let sr = state.sample_rate;
    let phase = Cell::new(state.phase.get());

    device
        .build_output_stream(
            config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                let cb_state = AudioState {
                    playing: playing.clone(),
                    fx_chain: fx_chain.clone(),
                    pending_specs: pending_specs.clone(),
                    sample_rate: sr,
                    phase: Cell::new(phase.get()),
                };
                AudioEngine::audio_callback(data, &cb_state);
                phase.set(cb_state.phase.get());
            },
            |err| eprintln!("Audio error: {err}"),
            None,
        )
        .context("failed to build output stream")
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
