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

    /// The interface's end of a looper deck, taken at the same moment as
    /// [`Self::editor`] and for the same reason: it is the last instant anybody
    /// but the audio thread can reach the processor.
    ///
    /// `&mut`, unlike the handles around it, because there is exactly one of
    /// these and it is moved out — two ends of one ring cannot be cloned.
    fn loopdeck(&mut self) -> Option<LoopHandle> {
        None
    }

    /// Whether this processor is a looper deck — asked **after** the handle has
    /// been taken, which is why it cannot just be `loopdeck().is_some()`.
    ///
    /// The audio thread uses it to carry a deck across a chain rebuild: adding
    /// a reverb builds a whole new chain, and a deck rebuilt from its spec is a
    /// deck with no takes in it. Minutes of playing, gone because the player
    /// reached for another effect.
    fn is_loop_deck(&self) -> bool {
        false
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

    /// The plugin's own preset browser, when the format lets choz read it.
    /// Captured at the same moment as [`Self::editor`]; `None` means the tab's
    /// BANK key has nothing to show.
    fn presets(&self) -> Option<PresetsHandle> {
        None
    }

    /// Controls the plugin keeps **outside** its parameters, addressed by name.
    ///
    /// ZynAddSubFX is the reason this exists: its ports are sixteen numbered
    /// slots, and everything that shapes the sound — including the 128
    /// harmonics of an oscillator — lives behind an OSC server addressed by
    /// path. A view that edits those needs to reach them one by one, which no
    /// flat parameter list can express.
    fn paths(&self) -> Option<PathsHandle> {
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

// ─── The looper deck's thread bridge ────────────────────────────────────────

/// One second of interleaved stereo audio, as the looper stores it.
///
/// **`i16`, not `f32`.** Five minutes of stereo `f32` at 48 kHz is 110 MiB a
/// track, and eight of those is most of a gigabyte; `i16` halves that and is
/// what the exported WAV is written as anyway, so nothing is lost twice.
///
/// **An `Arc`, so the interface can read what the audio thread is playing.**
/// The UI allocates it and hands it over; while the audio thread holds the only
/// reference it writes through `Arc::get_mut`, which is allocation-free. Once
/// the chunk is full it sends a *clone* home — a refcount bump, nothing more —
/// and from then on `get_mut` fails, which is exactly the guarantee wanted: a
/// chunk anybody else can see is never written again.
pub type LoopChunk = std::sync::Arc<[i16]>;

/// Frames one chunk holds. A second at 48 kHz, which is 187.5 KiB — small
/// enough that a track costs what it recorded, big enough that the ring is
/// touched once a second rather than once a block.
pub const LOOP_CHUNK_SECS: usize = 1;

/// A chunk that came home: which track it belongs to, where in that track, and
/// the audio. The interface keeps these to export from — see [`LoopHandle`].
pub struct LoopFilled {
    pub track: usize,
    pub index: usize,
    pub chunk: LoopChunk,
}

/// What a deck is doing, for the panel to draw. Atomics, published from the
/// callback and read whenever the UI redraws — the same contract [`FxMeter`]
/// has, for the same reason.
#[derive(Default)]
pub struct LoopState {
    /// Per track: 0 idle, 1 recording, 2 playing, 3 paused. Packed four bits a
    /// track so the whole deck is one load.
    tracks: std::sync::atomic::AtomicU64,
    /// Peak of what is **arriving** at the deck, as the panel's dB monitor
    /// reads it. One number and not one a track: a deck hears one source, so
    /// every channel's input is the same input.
    in_peak: std::sync::atomic::AtomicU32,
    /// The length track 1 froze, in frames. 0 until it does.
    frames: std::sync::atomic::AtomicUsize,
    /// Where the single playhead is.
    pos: std::sync::atomic::AtomicUsize,
    /// Frames the deck has recorded but not yet frozen — what the panel draws
    /// while track 1 is still growing.
    recorded: std::sync::atomic::AtomicUsize,
    /// Per channel, what that channel is putting out this block: the peak and
    /// the RMS, linear. What a strip's activity monitor draws — the input meter
    /// answers "is the deck hearing anything", and this answers "is *this*
    /// channel doing anything", which is the question a player with four
    /// strips in front of them is actually asking.
    ///
    /// A recording channel reports what it is taking in, so a strip lights up
    /// while the take is being made rather than only once it plays.
    track_peak: [std::sync::atomic::AtomicU32; LOOP_TRACKS],
    track_rms: [std::sync::atomic::AtomicU32; LOOP_TRACKS],
    /// The audio thread ran out of chunks and closed the loop rather than lose
    /// audio. Sticky until the interface reads it.
    starved: std::sync::atomic::AtomicBool,
}

/// What one track of a deck is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoopTrackState {
    #[default]
    Idle,
    Recording,
    Playing,
    /// Silent, but still the deck's: the shared playhead keeps running, so a
    /// track let back in comes back **in time with the others** rather than at
    /// the top of the loop. That is the whole difference between PAUSE and
    /// STOP on a deck with one playhead.
    Paused,
}

impl LoopTrackState {
    fn code(self) -> u64 {
        match self {
            LoopTrackState::Idle => 0,
            LoopTrackState::Recording => 1,
            LoopTrackState::Playing => 2,
            LoopTrackState::Paused => 3,
        }
    }

    fn of(code: u64) -> Self {
        match code {
            1 => LoopTrackState::Recording,
            2 => LoopTrackState::Playing,
            3 => LoopTrackState::Paused,
            _ => LoopTrackState::Idle,
        }
    }
}

/// Tracks one deck holds. Fixed, so the track array never grows on the RT
/// thread — and eight is what fits on the panel at a readable size.
pub const LOOP_TRACKS: usize = 8;

impl LoopState {
    pub fn set_track(&self, track: usize, state: LoopTrackState) {
        if track >= LOOP_TRACKS {
            return;
        }
        use std::sync::atomic::Ordering::Relaxed;
        let shift = track * 4;
        let mut now = self.tracks.load(Relaxed);
        now &= !(0xF << shift);
        now |= state.code() << shift;
        self.tracks.store(now, Relaxed);
    }

    pub fn track(&self, track: usize) -> LoopTrackState {
        use std::sync::atomic::Ordering::Relaxed;
        if track >= LOOP_TRACKS {
            return LoopTrackState::Idle;
        }
        LoopTrackState::of((self.tracks.load(Relaxed) >> (track * 4)) & 0xF)
    }

    /// The level arriving at the deck, as a linear peak.
    pub fn set_in_peak(&self, peak: f32) {
        self.in_peak
            .store(peak.to_bits(), std::sync::atomic::Ordering::Relaxed);
    }
    pub fn in_peak(&self) -> f32 {
        f32::from_bits(self.in_peak.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// One channel's own activity: peak and RMS of what it put out, linear.
    pub fn set_track_level(&self, track: usize, peak: f32, rms: f32) {
        use std::sync::atomic::Ordering::Relaxed;
        if track >= LOOP_TRACKS {
            return;
        }
        self.track_peak[track].store(peak.to_bits(), Relaxed);
        self.track_rms[track].store(rms.to_bits(), Relaxed);
    }

    /// `(peak, rms)`, both linear. Silence for a channel out of range.
    pub fn track_level(&self, track: usize) -> (f32, f32) {
        use std::sync::atomic::Ordering::Relaxed;
        if track >= LOOP_TRACKS {
            return (0.0, 0.0);
        }
        (
            f32::from_bits(self.track_peak[track].load(Relaxed)),
            f32::from_bits(self.track_rms[track].load(Relaxed)),
        )
    }

    pub fn any_recording(&self) -> bool {
        (0..LOOP_TRACKS).any(|t| self.track(t) == LoopTrackState::Recording)
    }

    pub fn set_frames(&self, frames: usize) {
        self.frames
            .store(frames, std::sync::atomic::Ordering::Relaxed);
    }
    pub fn frames(&self) -> usize {
        self.frames.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn set_pos(&self, pos: usize) {
        self.pos.store(pos, std::sync::atomic::Ordering::Relaxed);
    }
    pub fn pos(&self) -> usize {
        self.pos.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn set_recorded(&self, frames: usize) {
        self.recorded
            .store(frames, std::sync::atomic::Ordering::Relaxed);
    }
    pub fn recorded(&self) -> usize {
        self.recorded.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn set_starved(&self, on: bool) {
        self.starved.store(on, std::sync::atomic::Ordering::Relaxed);
    }
    /// Whether the deck ran out of chunks, clearing the flag as it is read.
    pub fn take_starved(&self) -> bool {
        self.starved
            .swap(false, std::sync::atomic::Ordering::Relaxed)
    }
}

/// The interface's end of a looper deck.
///
/// Taken out of the processor at build time — the way `meter()` and `editor()`
/// already are, in the one moment `AudioEngine::set_slot_fx` calls "the last
/// chance to reach the processors". [`FxProcessor::process_block`] gets a
/// buffer and a sample rate and nothing else: a processor has no handle to the
/// host, no channel, and no way to ask for memory. A looper that records five
/// minutes needs all three, and this is where they come from.
pub struct LoopHandle {
    /// Empty chunks going out to the audio thread.
    supply: rtrb::Producer<LoopChunk>,
    /// Full ones coming back, so the interface can export what was recorded.
    home: rtrb::Consumer<LoopFilled>,
    state: std::sync::Arc<LoopState>,
    /// Every chunk this handle ever handed out, per track, in order.
    ///
    /// Kept — not dropped — for two reasons: it is what EXPORT writes, and it
    /// is what keeps the audio thread's own `Arc::drop` a refcount decrement
    /// rather than a free. Cleared on the interface thread when a track is.
    takes: Vec<Vec<LoopChunk>>,
    /// Frames in one chunk, fixed when the deck was built.
    chunk_frames: usize,
    sample_rate: u32,
}

impl LoopHandle {
    /// Build both ends. The processor keeps what this does not.
    ///
    /// `ring` is how many chunks may be in flight; the interface tops the
    /// supply back up every time it redraws, so this only has to cover the gap
    /// between two frames of the UI — a handful of seconds is plenty.
    pub fn pair(
        sample_rate: u32,
        ring: usize,
    ) -> (
        LoopHandle,
        rtrb::Consumer<LoopChunk>,
        rtrb::Producer<LoopFilled>,
        std::sync::Arc<LoopState>,
    ) {
        let (supply, supply_rx) = rtrb::RingBuffer::new(ring.max(2));
        let (home_tx, home) = rtrb::RingBuffer::new(ring.max(2) * LOOP_TRACKS);
        let state = std::sync::Arc::new(LoopState::default());
        let handle = LoopHandle {
            supply,
            home,
            state: state.clone(),
            takes: vec![Vec::new(); LOOP_TRACKS],
            chunk_frames: LOOP_CHUNK_SECS * sample_rate.max(1) as usize,
            sample_rate,
        };
        (handle, supply_rx, home_tx, state)
    }

    pub fn state(&self) -> &LoopState {
        &self.state
    }

    /// The same, owned — so the panel can hold it while the engine is borrowed
    /// somewhere else.
    pub fn shared_state(&self) -> std::sync::Arc<LoopState> {
        self.state.clone()
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn chunk_frames(&self) -> usize {
        self.chunk_frames
    }

    /// Bytes this deck is holding, counting each chunk once.
    pub fn bytes(&self) -> usize {
        self.takes
            .iter()
            .flat_map(|t| t.iter())
            .map(|c| c.len() * std::mem::size_of::<i16>())
            .sum()
    }

    /// The audio of one track, in order, for EXPORT to write.
    pub fn take(&self, track: usize) -> &[LoopChunk] {
        self.takes.get(track).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Collect what the audio thread has filled since the last call, and top the
    /// supply back up to `want` chunks. Called once per redraw.
    ///
    /// `budget_left` is how many more bytes the whole program may hold; the
    /// interface is the only side that allocates, so it is also the only side
    /// that has to be told when to stop. Returns the bytes it allocated.
    pub fn pump(&mut self, want: usize, budget_left: usize) -> usize {
        while let Ok(filled) = self.home.pop() {
            if let Some(track) = self.takes.get_mut(filled.track) {
                if track.len() <= filled.index {
                    track.resize(filled.index + 1, filled.chunk.clone());
                }
                track[filled.index] = filled.chunk;
            }
        }
        let each = self.chunk_frames * 2 * std::mem::size_of::<i16>();
        let mut spent = 0;
        while self.supply.slots() > 0 && spent + each <= budget_left {
            let chunk: LoopChunk = vec![0i16; self.chunk_frames * 2].into();
            if self.supply.push(chunk).is_err() {
                break;
            }
            spent += each;
            if self.supply.slots() == 0 {
                break;
            }
            // `want` is a target, not the ring's whole capacity: handing over
            // every slot at once would allocate seconds of audio for a deck
            // nobody has armed.
            if (self.supply.buffer().capacity() - self.supply.slots()) >= want {
                break;
            }
        }
        spent
    }

    /// Forget one track's audio. The interface thread is where these are freed:
    /// the audio thread only ever decrements.
    pub fn forget(&mut self, track: usize) {
        if let Some(t) = self.takes.get_mut(track) {
            t.clear();
        }
    }

    /// Throw one track's audio away and slide the ones above it down — the
    /// interface's half of the deck's own `Remove`, so the takes stay lined up
    /// with the channels EXPORT names.
    pub fn remove(&mut self, track: usize) {
        if track < self.takes.len() {
            self.takes.remove(track);
            self.takes.push(Vec::new());
        }
    }

    pub fn forget_all(&mut self) {
        for t in self.takes.iter_mut() {
            t.clear();
        }
    }
}

impl std::fmt::Debug for LoopHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "LoopHandle({} bytes)", self.bytes())
    }
}

// ─── Sources ────────────────────────────────────────────────────────────────

/// A source of interleaved stereo `f32` audio. `render` is called from the
/// audio callback — must be realtime-safe (no alloc, no locks, no I/O).
/// Octaves a keyboard split is expressed in — MIDI note 127 is in octave 10,
/// so eleven covers the whole range.
pub const SPLIT_OCTAVES: usize = 11;

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

    /// Whether this source can play several *different* sounds at once, one per
    /// keyboard zone.
    ///
    /// A SoundFont can: the file is loaded once and the engine has sixteen MIDI
    /// channels to point at different programs in it, so a split costs nothing
    /// but channels. A hosted plugin cannot — it has one patch — so the rack
    /// falls back to switching that patch as the hand moves, which is one sound
    /// at a time.
    fn layers_zones(&self) -> bool {
        false
    }

    /// Point keyboard zone `zone` at a program of the loaded file.
    ///
    /// Zones are the rack's sound buttons, numbered from 0. Ignored by a source
    /// that answers `false` to [`AudioSource::layers_zones`].
    fn set_zone_program(&mut self, _zone: u8, _bank: u8, _preset: u8) {}

    /// Which zone each octave of the keyboard plays, `None` for the source's
    /// own current program.
    ///
    /// A fixed array rather than a slice because this crosses to the audio
    /// thread inside a command, and a command that allocates is a command that
    /// can stall a block.
    fn set_split(&mut self, _split: [Option<u8>; SPLIT_OCTAVES]) {}

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

    /// The plugin's own preset browser, when the format lets choz read it.
    /// Captured at the same moment as [`Self::editor`]; `None` means the tab's
    /// BANK key has nothing to show.
    fn presets(&self) -> Option<PresetsHandle> {
        None
    }

    /// Controls the plugin keeps **outside** its parameters, addressed by name.
    ///
    /// ZynAddSubFX is the reason this exists: its ports are sixteen numbered
    /// slots, and everything that shapes the sound — including the 128
    /// harmonics of an oscillator — lives behind an OSC server addressed by
    /// path. A view that edits those needs to reach them one by one, which no
    /// flat parameter list can express.
    fn paths(&self) -> Option<PathsHandle> {
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

    /// Whether the plugin puts up a **window of its own** rather than embedding
    /// into the one the host offers (LV2's `ui:showInterface`: Yoshimi,
    /// ZynAddSubFX). The host must then create no window at all — one it made
    /// would sit there empty beside the plugin's.
    fn owns_window(&self) -> bool {
        false
    }

    /// False once such a window has been closed. Only meaningful for an editor
    /// that owns its window: there is no host window for the window manager to
    /// report the close on, so the plugin is the one that says so.
    fn is_open(&self) -> bool {
        true
    }

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

/// One preset a plugin offers: what to show, and what to say back to load it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PresetEntry {
    /// What the user reads — the patch name the plugin gave.
    pub name: String,
    /// The group the plugin filed it under (CLAP collection, VST2 bank, an
    /// SF2's bank number). Empty when the format has no grouping; the picker
    /// turns the distinct values into filter chips.
    pub category: String,
    /// What [`PluginPresets::load`] needs to get this one back: a CLAP
    /// `load_key`, a VST2 program index as text, a preset URI. Opaque to
    /// choz — only the runtime that produced it reads it.
    pub key: String,
}

/// The presets a plugin can list and load by itself — its own patch browser,
/// flattened.
///
/// Deliberately **not** on the RT traits: no format loads a preset without
/// allocating, reading files or talking to the plugin's main thread, so this
/// is captured where [`PluginEditor`] and [`PluginState`] are and called from
/// the UI thread. Parameters and state stay as they are; a preset is the
/// coarse move that replaces both.
pub trait PluginPresets: Send + Sync {
    /// Everything the plugin offers, in the order it offered it. Empty when it
    /// has no browser of its own — which is most effects, and is not an error.
    fn list(&self) -> Vec<PresetEntry>;

    /// Load the preset with this [`PresetEntry::key`]. Silent no-op when the
    /// key is not one of ours or the instance is already gone.
    fn load(&self, key: &str);

    /// The key of the preset the plugin says it is on, when the format can be
    /// asked. Default `None`: the picker then opens where it last was, which
    /// is what every format that cannot answer leaves it doing anyway.
    fn current(&self) -> Option<String> {
        None
    }
}

pub type PresetsHandle = std::sync::Arc<dyn PluginPresets>;

/// A plugin's own controls, addressed by path rather than by index.
///
/// Reads are **asked for and collected later**: the plugin answers when it
/// answers, and a UI that blocked on each of 128 harmonics would stutter for a
/// second every time it drew. [`Self::ask`] sends the question, [`Self::value`]
/// returns the last answer there was, and a fresh view simply has none yet.
/// The harmonics of a plugin's oscillator, as a set of paths.
///
/// A synth that draws its sound as a row of bars — ZynAddSubFX is the one here
/// — keeps one control per harmonic. Which paths those are is the plugin's
/// business; what the view needs is how many there are and what a bar means.
pub struct HarmonicSet {
    /// One path per harmonic, in order: the first is the fundamental.
    pub magnitude: Vec<String>,
    /// The phase of each, when the plugin has one per harmonic. Empty when it
    /// does not, and then the view shows magnitudes alone.
    pub phase: Vec<String>,
    pub min: f32,
    pub max: f32,
    /// The value that means *silent*. Zyn's bars sit at 64 of 0..127 and grow
    /// either way from there, which is why this is not simply `min`.
    pub zero: f32,
}

pub trait PluginPaths: Send + Sync {
    /// Move the control at `path`.
    fn set(&self, path: &str, value: f32);
    /// Ask what it holds. The answer turns up in [`Self::value`].
    fn ask(&self, path: &str);
    /// The last value the plugin reported, if it has reported one.
    fn value(&self, path: &str) -> Option<f32>;

    /// The harmonics of the sound, when the plugin has a set of them to draw.
    fn harmonics(&self) -> Option<HarmonicSet> {
        None
    }

    /// The path behind each of the plugin's **parameters**, in the order they
    /// are reported. Empty for a plugin whose parameters are not paths.
    ///
    /// This is what lets the knobs show what the plugin is actually holding: a
    /// patch loaded inside it moves controls the host never touched, and a
    /// panel that only ever shows what *it* last sent is a panel that lies
    /// after the first preset.
    fn param_paths(&self) -> Vec<String> {
        Vec::new()
    }
}

pub type PathsHandle = std::sync::Arc<dyn PluginPaths>;

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
    /// Which section of the plugin this belongs to, **when the plugin says so**
    /// — CLAP's `module`, and nothing invented here.
    ///
    /// A synth with three hundred parameters is unreadable as one list: Surge
    /// XT's are called things like "Filter 1 Cutoff", and in a thirteen-column
    /// cell that is "Filter 1 C…", which names neither the section nor the
    /// control. With the section known, the cell can drop the part that repeats
    /// and show `Cutoff` under a heading that says `Filter 1`.
    pub group: Option<String>,
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

    /// Put the clock at `beats` quarter notes, which is where a **host's**
    /// transport says we are.
    ///
    /// choz's own clock is counted in frames and only ever moves forward, so
    /// this is the one door for a timeline somebody else owns: the exported
    /// CLAP plugins follow the DAW they are loaded into. Converting through
    /// frames rather than storing beats keeps a single source of position —
    /// two of them would disagree the moment the tempo changed.
    pub fn set_position_beats(&self, beats: f64) {
        let per_beat = self.sample_rate() as f64 * 60.0 / self.bpm().max(1.0) as f64;
        let samples = (beats.max(0.0) * per_beat) as u64;
        self.samples
            .store(samples, std::sync::atomic::Ordering::Relaxed);
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
