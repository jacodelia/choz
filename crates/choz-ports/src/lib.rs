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
    pub const fn new(
        name: &'static str,
        value: f32,
        min: f32,
        max: f32,
        unit: &'static str,
    ) -> Self {
        Self {
            name,
            value,
            min,
            max,
            unit,
        }
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
    fn name(&self) -> &str {
        "FX"
    }

    /// Return automatable parameter list.
    fn params(&self) -> Vec<FxParam> {
        Vec::new()
    }

    /// Set a parameter by index to a normalised 0.0–1.0 value.
    fn set_param(&mut self, _index: usize, _value: f32) {}

    /// Handle to the plugin's own window, when it has one. Taken once, before
    /// the processor moves to the RT thread. Default `None`: built-in FX have
    /// no native editor.
    fn editor(&self) -> Option<EditorHandle> {
        None
    }

    /// Parameters the user moves inside the plugin's own window, when the
    /// format can report them. Captured at the same moment as [`Self::editor`].
    fn param_touch(&self) -> Option<TouchHandle> {
        None
    }

    /// The plugin's opaque state, for projects that must reopen sounding the
    /// same. Captured at the same moment as [`Self::editor`].
    fn state(&self) -> Option<StateHandle> {
        None
    }

    /// Live counters when this processor is a plugin running in its own
    /// process. Taken once, next to [`FxProcessor::editor`]. Default `None`:
    /// everything else runs in choz's own process.
    fn sandbox(&self) -> Option<SandboxStatus> {
        None
    }

    /// Peak in and out of the last block, when this processor publishes them.
    /// Taken once, next to [`FxProcessor::editor`] — after that the processor
    /// belongs to the RT thread. Default `None`: an effect that meters nothing.
    fn meter(&self) -> Option<FxMeter> {
        None
    }

    /// How many samples of delay this processor adds to the signal.
    ///
    /// Anything with lookahead or an FFT window has some, and it is a constant
    /// of the algorithm, not of the block size — so it is asked once, off the
    /// RT thread, where the editor and the meter are taken. Default `0`: most
    /// effects answer the same block they were given.
    fn latency_samples(&self) -> u32 {
        0
    }
}

// ─── Meters ─────────────────────────────────────────────────────────────────

/// Peak in and out of one effect's last block, for a meter the interface can
/// draw.
///
/// The processor belongs to the audio thread the moment it is handed over, so
/// the numbers travel the way [`SandboxStatus`]'s do: shared atomics, written
/// relaxed from the callback, read whenever the UI redraws. A reading one block
/// stale is a reading that is right.
#[derive(Clone)]
pub struct FxMeter {
    /// `[input, output]` peak, as `f32` bits.
    peaks: std::sync::Arc<[std::sync::atomic::AtomicU32; 2]>,
}

impl Default for FxMeter {
    fn default() -> Self {
        Self {
            peaks: std::sync::Arc::new([
                std::sync::atomic::AtomicU32::new(0),
                std::sync::atomic::AtomicU32::new(0),
            ]),
        }
    }
}

impl std::fmt::Debug for FxMeter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (i, o) = self.peaks();
        write!(f, "FxMeter({i:.3} -> {o:.3})")
    }
}

impl FxMeter {
    /// Publish one block's peaks. Two relaxed stores, nothing else.
    pub fn publish(&self, input: f32, output: f32) {
        self.peaks[0].store(input.to_bits(), std::sync::atomic::Ordering::Relaxed);
        self.peaks[1].store(output.to_bits(), std::sync::atomic::Ordering::Relaxed);
    }

    /// `(input, output)` peak of the last block, linear.
    pub fn peaks(&self) -> (f32, f32) {
        let read =
            |i: usize| f32::from_bits(self.peaks[i].load(std::sync::atomic::Ordering::Relaxed));
        (read(0), read(1))
    }

    /// Forget the last block: a chain that stopped should not leave a frozen
    /// needle behind. Called from `reset`, which is not the RT path.
    pub fn clear(&self) {
        self.publish(0.0, 0.0);
    }

    /// Peak of an interleaved block. The one pass every metering effect makes,
    /// written once here — and it skips anything non-finite, because a plugin
    /// that emits a NaN must not freeze the meter at NaN forever.
    pub fn peak_of(buf: &[f32]) -> f32 {
        buf.iter()
            .filter(|s| s.is_finite())
            .fold(0.0f32, |m, s| m.max(s.abs()))
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

    /// Stop every note this source is playing, right now — the panic button.
    ///
    /// The default is the two MIDI messages every synth understands: `all
    /// sound off` (CC 120) and `all notes off` (CC 123). A source that can do
    /// better (a sampler that owns its voices, a SoundFont engine with its own
    /// reset) should override this: a plugin that ignores the CCs is exactly
    /// the case the button exists for.
    ///
    /// Called from the audio thread, so it must not allocate: the two messages
    /// go through the same queues a note does.
    fn all_notes_off(&mut self) {
        self.control_change(120, 0);
        self.control_change(123, 0);
    }

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

    /// Parameters the user moves inside the plugin's own window, when the
    /// format can report them. Captured at the same moment as [`Self::editor`].
    fn param_touch(&self) -> Option<TouchHandle> {
        None
    }

    /// The plugin's opaque state, for projects that must reopen sounding the
    /// same. Captured at the same moment as [`Self::editor`].
    fn state(&self) -> Option<StateHandle> {
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

/// What the plugin's own window reports back: the parameter the user just
/// grabbed in it.
///
/// The point is MIDI learn. With the native editor open the keyboard and mouse
/// belong to the plugin, not to the TUI, so "bind the control I am touching"
/// can only work if the plugin says which one that is. Every format has a way
/// of telling the host (VST3 `IComponentHandler::performEdit`, VST2
/// `audioMasterAutomate`, CLAP's output event stream, an LV2 UI's write
/// callback); this is the one shape choz reads them through.
pub trait ParamTouch: Send + Sync {
    /// The last parameter the user moved and its new normalised value, or
    /// `None`. **Reading clears it**, so an old gesture cannot capture a CC
    /// that arrives much later — and the value is what lets choz keep its own
    /// knobs (and the saved project) in step with edits made in the plugin's
    /// window.
    fn take_touched(&self) -> Option<(u32, f32)>;
}

pub type TouchHandle = std::sync::Arc<dyn ParamTouch>;

/// A plugin's own opaque state — everything about its sound that is **not** a
/// parameter value.
///
/// Saving the parameter list is not enough: a patch picked in the plugin's
/// browser, an internal preset, a wavetable, a sample path… none of those are
/// automatable parameters, and all of them vanish when the tab is rebuilt.
/// Every format has a blob for exactly this (VST2 chunks, VST3
/// `IComponent::getState`, `clap.state`), and this is the one shape choz stores
/// it in.
///
/// The handle is captured where [`PluginEditor`] is, and reaches the plugin
/// through the same shared cell — so it stops working, quietly, once the
/// instance is gone.
pub trait PluginState: Send + Sync {
    /// The plugin's state, or `None` when it has none to give.
    fn save(&self) -> Option<Vec<u8>>;

    /// Restore a blob produced by [`Self::save`] on this same plugin.
    fn restore(&self, data: &[u8]);
}

pub type StateHandle = std::sync::Arc<dyn PluginState>;

// ─── Hosted plugins ─────────────────────────────────────────────────────────

/// One automatable parameter of a hosted plugin (CLAP param, LV2 control port,
/// LADSPA control port…). Names are dynamic, so this is the descriptor the UI
/// shows instead of [`FxParam`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PluginParam {
    /// Format-specific identifier: CLAP param id, LV2/LADSPA port index.
    pub id: u32,
    pub name: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    /// How many distinct positions the parameter has: `0` continuous, `2` an
    /// on/off switch, `n` an enumeration of n steps.
    ///
    /// **Only ever what the plugin said.** Guessing a switch from a name that
    /// happens to read like one is how a filter cutoff ends up as a checkbox;
    /// a host that does not report this leaves it 0 and gets a knob.
    pub steps: u32,
    /// Unit for display (`"Hz"`, `"dB"`, `"%"`), when the plugin gives one.
    pub unit: Option<String>,
    /// Named positions — `(value, label)` — for a parameter whose steps have
    /// names: waveform, filter type, mode. Empty when there are none.
    pub points: Vec<(f64, String)>,
}

impl PluginParam {
    /// A parameter with nothing but the numbers, which is all most hosts give.
    pub fn plain_range(id: u32, name: String, min: f64, max: f64, default: f64) -> Self {
        Self {
            id,
            name,
            min,
            max,
            default,
            ..Self::default()
        }
    }

    /// `true` when the parameter is an on/off switch.
    pub fn is_toggle(&self) -> bool {
        self.steps == 2
    }

    /// The label for `plain`, when the parameter has named steps: the nearest
    /// point at or below the value, so a slider between two names reads as the
    /// one it has reached.
    pub fn label_for(&self, plain: f64) -> Option<&str> {
        if self.points.is_empty() {
            return None;
        }
        self.points
            .iter()
            .min_by(|a, b| {
                (a.0 - plain)
                    .abs()
                    .partial_cmp(&(b.0 - plain).abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(_, label)| label.as_str())
    }
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

// ─── Transport ──────────────────────────────────────────────────────────────

/// The host's clock: where in the song choz is, and how fast.
///
/// A plugin that syncs anything — a tempo delay, an LFO, an arpeggiator — asks
/// the host for this on **every block**, from the audio thread, and several
/// dereference the answer without checking it (u-he's VST2s segfault on a null
/// one). So it is a handful of atomics: readable from the callback, writable
/// from the UI, and never a lock.
///
/// It is a **process-global** ([`transport`]) on purpose. There is one clock, and
/// the place that needs it most is a C callback — VST2's `audioMasterGetTime` —
/// which is handed a plugin pointer and no host context at all. Threading a
/// per-host instance down to there would mean a registry keyed by plugin
/// pointer, to answer the same question with the same number.
#[derive(Debug)]
pub struct Transport {
    /// Frames played since the stream started.
    samples: std::sync::atomic::AtomicU64,
    /// Beats per minute, as `f32` bits.
    bpm: std::sync::atomic::AtomicU32,
    sample_rate: std::sync::atomic::AtomicU32,
    playing: std::sync::atomic::AtomicBool,
    /// Time signature, packed as `numerator << 16 | denominator`. One atomic
    /// because the two are only ever meaningful together — a plugin reading
    /// 3 over 4 halfway through a change to 6/8 would be reading a bar that
    /// never existed.
    time_sig: std::sync::atomic::AtomicU32,
}

/// The one clock. See [`Transport`] for why it is global.
pub fn transport() -> &'static Transport {
    static TRANSPORT: Transport = Transport::new();
    &TRANSPORT
}

impl Transport {
    pub const DEFAULT_BPM: f32 = 120.0;
    /// The range the UI offers, and what any setter clamps to.
    pub const MIN_BPM: f32 = 20.0;
    pub const MAX_BPM: f32 = 300.0;

    const fn new() -> Self {
        Self {
            samples: std::sync::atomic::AtomicU64::new(0),
            bpm: std::sync::atomic::AtomicU32::new(Self::DEFAULT_BPM.to_bits()),
            sample_rate: std::sync::atomic::AtomicU32::new(48_000),
            playing: std::sync::atomic::AtomicBool::new(false),
            time_sig: std::sync::atomic::AtomicU32::new((4 << 16) | 4),
        }
    }

    /// Move the clock on by one block. Called from the audio callback, so:
    /// relaxed, and nothing else.
    pub fn advance(&self, frames: usize) {
        self.samples
            .fetch_add(frames as u64, std::sync::atomic::Ordering::Relaxed);
    }

    /// Back to the top. A new stream starts at zero, or the user rewinds.
    pub fn rewind(&self) {
        self.samples.store(0, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn samples(&self) -> u64 {
        self.samples.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn bpm(&self) -> f32 {
        f32::from_bits(self.bpm.load(std::sync::atomic::Ordering::Relaxed))
    }

    pub fn set_bpm(&self, bpm: f32) {
        let bpm = bpm.clamp(Self::MIN_BPM, Self::MAX_BPM);
        self.bpm
            .store(bpm.to_bits(), std::sync::atomic::Ordering::Relaxed);
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
            .load(std::sync::atomic::Ordering::Relaxed)
            .max(1)
    }

    /// Told by the engine when the stream opens. Rewinds: a position in frames
    /// means nothing once the frames are a different length.
    pub fn set_sample_rate(&self, sr: u32) {
        self.sample_rate
            .store(sr.max(1), std::sync::atomic::Ordering::Relaxed);
        self.rewind();
    }

    pub fn playing(&self) -> bool {
        self.playing.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn set_playing(&self, playing: bool) {
        self.playing
            .store(playing, std::sync::atomic::Ordering::Relaxed);
    }

    /// Beats per bar and the note value that gets the beat: `(4, 4)`, `(6, 8)`.
    pub fn time_signature(&self) -> (u16, u16) {
        let packed = self.time_sig.load(std::sync::atomic::Ordering::Relaxed);
        ((packed >> 16) as u16, packed as u16)
    }

    /// Set the time signature. Both halves are clamped to something a bar can
    /// be made of; a denominator that is not a power of two is not a note
    /// value, and a plugin handed one has no way to interpret it.
    pub fn set_time_signature(&self, numerator: u16, denominator: u16) {
        let numerator = numerator.clamp(1, 32);
        let denominator = match denominator {
            1 | 2 | 4 | 8 | 16 | 32 => denominator,
            _ => 4,
        };
        self.time_sig.store(
            ((numerator as u32) << 16) | denominator as u32,
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    /// Position in quarter notes, which is what every plugin format asks for
    /// (VST2 `ppqPos`, VST3 `projectTimeMusic`, CLAP's beat position).
    pub fn ppq(&self) -> f64 {
        self.samples() as f64 / self.sample_rate() as f64 * (self.bpm() as f64 / 60.0)
    }

    /// A bar's length in quarter notes. 4/4 is four, 6/8 is three, 7/8 is 3.5 —
    /// the numerator counts notes of `1/denominator`, and a quarter is four of
    /// the denominator's own units.
    pub fn bar_quarters(&self) -> f64 {
        let (num, den) = self.time_signature();
        num as f64 * 4.0 / den.max(1) as f64
    }

    /// Where the bar containing the playhead started, in quarter notes, and
    /// which bar that is counting from 1.
    ///
    /// choz has no arrangement, so "bar 1" is simply where the transport was
    /// last reset. That is still worth publishing: a plugin that draws a bar
    /// counter or syncs a pattern to bar starts needs the *phase*, and the phase
    /// is real even when the number is only a count.
    pub fn bar_position(&self) -> (i32, f64) {
        let quarters = self.bar_quarters();
        if quarters <= 0.0 {
            return (1, 0.0);
        }
        let ppq = self.ppq();
        let bars = (ppq / quarters).floor();
        (bars as i32 + 1, bars * quarters)
    }
}

#[cfg(test)]
mod transport_tests {
    use super::*;

    /// A bar is the time signature read as quarter notes, and the playhead's
    /// bar is where the phase is — which is the half of it a plugin can use
    /// even though choz has no arrangement to number bars against.
    #[test]
    fn a_bar_is_the_time_signature_read_in_quarter_notes() {
        let t = transport();
        t.set_sample_rate(48_000);
        t.set_bpm(120.0);
        t.rewind();
        t.set_time_signature(4, 4);
        assert_eq!(t.bar_quarters(), 4.0);
        assert_eq!(t.bar_position(), (1, 0.0), "the start is bar 1 at 0");

        // 120 BPM: a quarter note is half a second. Five quarters in is the
        // second bar, and it began at 4.
        t.advance(24_000 * 5);
        assert_eq!(t.bar_position(), (2, 4.0));

        // 6/8 is six eighths, which is three quarters — so the same playhead
        // sits in a different bar with a different start.
        t.set_time_signature(6, 8);
        assert_eq!(t.bar_quarters(), 3.0);
        assert_eq!(t.bar_position(), (2, 3.0));

        // 7/8 is three and a half quarters, which is not a whole number and is
        // exactly why this is computed rather than counted in beats.
        t.set_time_signature(7, 8);
        assert_eq!(t.bar_quarters(), 3.5);
        assert_eq!(t.bar_position(), (2, 3.5));

        t.set_time_signature(4, 4);
        t.set_bpm(Transport::DEFAULT_BPM);
        t.rewind();
    }
}
