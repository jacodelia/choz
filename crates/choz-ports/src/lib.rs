//! Realtime-safe port traits shared across choz crates.
//!
//! RULE: methods called from the audio callback (`FxProcessor::process_block`,
//! `AudioSource::render` / `note_*`) must be allocation-free, lock-free, and
//! non-blocking. Everything else (construction, file loading) is non-RT.

// ─── FX ─────────────────────────────────────────────────────────────────────

/// A single automatable parameter descriptor.
#[derive(Debug, Clone)]
pub struct FxParam {
    pub name: &'static str,
    pub value: f32,
    pub min: f32,
    pub max: f32,
    pub unit: &'static str,
}

impl FxParam {
    pub const fn new(name: &'static str, value: f32, min: f32, max: f32, unit: &'static str) -> Self {
        Self { name, value, min, max, unit }
    }

    pub fn native(&self) -> f32 {
        self.min + self.value * (self.max - self.min)
    }
}

/// Common interface for all FX processors.
pub trait FxProcessor: Send {
    /// Process one stereo block in place. `buf` is interleaved L/R.
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32);

    /// Reset internal state.
    fn reset(&mut self);

    /// Dry/wet mix (0.0 = dry, 1.0 = fully wet).
    fn set_mix(&mut self, wet: f32);

    /// Human-readable name.
    fn name(&self) -> &str { "FX" }

    /// Return automatable parameter list.
    fn params(&self) -> Vec<FxParam> { Vec::new() }

    /// Set a parameter by index to a normalised 0.0–1.0 value.
    fn set_param(&mut self, _index: usize, _value: f32) {}

    /// Handle to the plugin's own window, when it has one. Taken once, before
    /// the processor moves to the RT thread. Default `None`: built-in FX have
    /// no native editor.
    fn editor(&self) -> Option<EditorHandle> {
        None
    }

    /// Live counters when this processor is a plugin running in its own
    /// process. Taken once, next to [`FxProcessor::editor`]. Default `None`:
    /// everything else runs in choz's own process.
    fn sandbox(&self) -> Option<SandboxStatus> {
        None
    }
}

// ─── Sources ────────────────────────────────────────────────────────────────

/// A source of interleaved stereo `f32` audio. `render` is called from the
/// audio callback — must be realtime-safe (no alloc, no locks, no I/O).
pub trait AudioSource: Send {
    /// Fill `out` (interleaved stereo) with the next block. Returns frames
    /// written; a short/zero return means the source has finished.
    fn render(&mut self, out: &mut [f32], sample_rate: u32) -> usize;

    /// MIDI note-on. Default no-op: only playable synths (SF2) react.
    fn note_on(&mut self, _note: u8, _velocity: u8) {}

    /// MIDI note-off. Default no-op.
    fn note_off(&mut self, _note: u8) {}

    /// MIDI control change — pedals (sustain 64, sostenuto 66, soft 67),
    /// expression (11), volume (7) and the modulation wheel (1) all arrive
    /// here. Default no-op: only playable synths react. Called on the RT
    /// thread, so implementations must not allocate or block.
    fn control_change(&mut self, _cc: u8, _value: u8) {}

    /// MIDI pitch bend, as the raw 14-bit wire value: 0..16383, centred at
    /// 8192. Default no-op. RT thread, same constraints as `control_change`.
    fn pitch_bend(&mut self, _value: u16) {}

    /// Select a bank/preset (program change). Default no-op: only multi-preset
    /// sources (SF2) react. Called on the RT thread, so implementations must not
    /// allocate or block.
    fn program_change(&mut self, _bank: u8, _preset: u8) {}

    /// Set a plugin parameter by index to a normalised 0.0–1.0 value. Default
    /// no-op: only hosted plugins expose parameters. Called on the RT thread,
    /// so implementations must not allocate or block.
    fn set_param(&mut self, _index: usize, _value: f32) {}

    /// Whether this source should keep rendering while transport is stopped.
    /// Synths return true (key presses / envelope tails must sound); generators
    /// gated by the play button (tone, WAV) return false.
    fn plays_on_transport_stop(&self) -> bool {
        false
    }

    /// Handle to the plugin's own window, when it has one. Taken once, before
    /// the source moves to the RT thread. Default `None`: built-in sources have
    /// no native editor.
    fn editor(&self) -> Option<EditorHandle> {
        None
    }

    /// Live counters when this source is a plugin running in its own process.
    /// Taken once, next to [`AudioSource::editor`]. Default `None`: everything
    /// else runs in choz's own process.
    fn sandbox(&self) -> Option<SandboxStatus> {
        None
    }
}

// ─── Out-of-process plugins ─────────────────────────────────────────────────

/// What the UI can see of a plugin hosted in a child process, without ever
/// touching the instance itself — which belongs to the RT thread.
///
/// Both counters are written from the audio thread (a plain relaxed store) and
/// read by the UI, so the shared handles are the whole point.
#[derive(Clone, Default)]
pub struct SandboxStatus {
    /// Blocks the child failed to answer in time. Each one is silence the user
    /// heard.
    pub missed: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// How many times the plugin crashed and was restarted.
    pub restarts: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl SandboxStatus {
    pub fn missed(&self) -> u64 {
        self.missed.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn restarts(&self) -> u64 {
        self.restarts.load(std::sync::atomic::Ordering::Relaxed)
    }
}

// ─── Native plugin editors ──────────────────────────────────────────────────

/// The plugin side of a native editor window. Every method runs on the GUI
/// thread that owns the window, never on the RT thread, and the implementation
/// is responsible for staying safe if the plugin is dropped meanwhile (calls
/// then become no-ops).
pub trait PluginEditor: Send + Sync {
    /// Embed the editor into the native window `parent` (an X11 Window XID on
    /// Linux). Returns the size the plugin asks for, if it reports one.
    fn open(&self, parent: u64) -> Option<(u16, u16)>;

    /// Pump the plugin's idle callback (~30 ms while the window is open).
    /// VST2 GUIs freeze without it.
    fn idle(&self) {}

    /// Tear the editor down. Safe to call more than once.
    fn close(&self);
}

pub type EditorHandle = std::sync::Arc<dyn PluginEditor>;

// ─── Hosted plugins ─────────────────────────────────────────────────────────

/// One automatable parameter of a hosted plugin (CLAP param, LV2 control port,
/// LADSPA control port…). Names are dynamic, so this is the descriptor the UI
/// shows instead of [`FxParam`].
#[derive(Debug, Clone, PartialEq)]
pub struct PluginParam {
    /// Format-specific identifier: CLAP param id, LV2/LADSPA port index.
    pub id: u32,
    pub name: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
}

impl PluginParam {
    /// Plain value for a normalised 0..1 knob position.
    pub fn plain(&self, norm: f64) -> f64 {
        self.min + norm.clamp(0.0, 1.0) * (self.max - self.min)
    }

    /// Knob position for a plain value.
    pub fn normalised(&self, plain: f64) -> f64 {
        if self.max <= self.min {
            return 0.0;
        }
        ((plain - self.min) / (self.max - self.min)).clamp(0.0, 1.0)
    }
}
