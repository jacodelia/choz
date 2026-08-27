//! The looper deck: several takes over one source.
//!
//! # What it is
//!
//! A guitar or a microphone comes into a tab, and this records take after take
//! of it — eight of them — which play together or not. It is the pedalboard
//! looper, not a mixer's recorder: it is an effect in **this tab's** chain and
//! hears **this tab's** audio, which is the whole reason it stays where the old
//! single-track looper already was.
//!
//! Where in the chain matters and is the player's choice. The deck records what
//! reaches its slot and sums its tracks into what leaves it, so effects *before*
//! it are printed into the take and effects *after* it colour the take every
//! time it plays.
//!
//! # The first track defines the length
//!
//! Track 1 grows until the player closes it or it reaches [`MAX_SECS`]. The
//! moment it closes, [`Looper::loop_frames`] is frozen and every other track is
//! exactly that long — a known size, so nothing after track 1 has to guess or
//! grow. That is a rule about musical form and it is also the memory strategy:
//! five minutes is a ceiling on **track 1 alone**, and a player looping an
//! eight-second phrase pays for eight seconds a track.
//!
//! # Why there is a thread bridge
//!
//! [`FxProcessor::process_block`] is handed a buffer and a sample rate. It has
//! no host handle, no channel and no way to ask for memory — and recording five
//! minutes needs all three. So the deck takes its chunks from a ring the
//! interface fills ([`LoopHandle`]), and never allocates, frees or blocks in the
//! callback. Running out of chunks closes the loop rather than losing audio in
//! silence.
//!
//! ponytail: no overdub. The old looper had one and nothing could reach it; with
//! eight tracks, stacking takes answers most of what it was for. Adding it back
//! means `f32` chunks and twice the memory, which is a decision for then.

use super::FxProcessor;
use choz_ports::{
    LoopChunk, LoopFilled, LoopHandle, LoopState, LoopTrackState, LOOP_CHUNK_SECS, LOOP_TRACKS,
};
use std::sync::Arc;

/// The ceiling on track 1, in seconds. Five minutes, which at 48 kHz in `i16`
/// is 54.9 MiB — the largest single thing choz will ever hold.
pub const MAX_SECS: usize = 300;

/// Chunks in flight between the interface and the callback. Four seconds of
/// slack against a UI that redraws thirty times a second is generous; the point
/// is that a stalled interface closes the loop instead of dropping audio.
const RING: usize = 4;

/// `i16` full scale, as the float side counts.
const SCALE: f32 = 32767.0;

/// What a closed loop's length is rounded to.
///
/// The floor is the point: a loop can never close shorter than one unit, so a
/// `REC` pressed twice by accident gives a bar of silence rather than a loop of
/// four frames that machine-guns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Quantise {
    #[default]
    Off,
    /// One bar of the transport's own time signature.
    Bar,
    /// One second, for a rig with no tempo worth following.
    Second,
}

impl Quantise {
    pub const ALL: [Quantise; 3] = [Quantise::Off, Quantise::Bar, Quantise::Second];

    pub fn label(self) -> &'static str {
        match self {
            Quantise::Off => "OFF",
            Quantise::Bar => "1 BAR",
            Quantise::Second => "1 SEC",
        }
    }

    pub fn next(self) -> Quantise {
        match self {
            Quantise::Off => Quantise::Bar,
            Quantise::Bar => Quantise::Second,
            Quantise::Second => Quantise::Off,
        }
    }

    /// The unit, in frames. `None` when there is nothing to round to.
    pub fn frames(self, sample_rate: u32) -> Option<usize> {
        let sr = sample_rate.max(1) as f64;
        match self {
            Quantise::Off => None,
            Quantise::Second => Some(sr as usize),
            Quantise::Bar => {
                let t = choz_ports::transport();
                let (num, den) = t.time_signature();
                let bpm = t.bpm().max(1.0) as f64;
                // A bar in quarter notes, then in seconds, then in frames.
                let quarters = num.max(1) as f64 * 4.0 / den.max(1) as f64;
                let secs = 60.0 / bpm * quarters;
                Some(((secs * sr).round() as usize).max(1))
            }
        }
    }

    /// `frames` rounded to the nearest whole unit, never below one.
    pub fn round(self, frames: usize, sample_rate: u32) -> usize {
        let Some(unit) = self.frames(sample_rate) else {
            return frames.max(1);
        };
        let units = ((frames as f64 / unit as f64).round() as usize).max(1);
        units * unit
    }
}

/// The transport, as parameters.
///
/// A looper's REC and PLAY *are* automatable controls — a host would want them
/// as such, and choz's MIDI learn already targets FX parameters. Modelling them
/// this way means the interface, a learned CC and the exported CLAP plugin all
/// reach the deck through `SetFxParam`, which already exists, instead of three
/// separate paths that could disagree.
///
/// One knob a track rather than a button each: a track is in exactly one state,
/// and two toggles that can both be on is a state nobody can draw.
const TRACK_PARAMS: [&str; LOOP_TRACKS] = ["T1", "T2", "T3", "T4", "T5", "T6", "T7", "T8"];

/// The name each block's eight parameters carry. `&'static str`, so they are
/// written out rather than formatted — which is also what a host reads.
const MUTE_PARAMS: [&str; LOOP_TRACKS] = [
    "T1 Mute", "T2 Mute", "T3 Mute", "T4 Mute", "T5 Mute", "T6 Mute", "T7 Mute", "T8 Mute",
];
const SOLO_PARAMS: [&str; LOOP_TRACKS] = [
    "T1 Solo", "T2 Solo", "T3 Solo", "T4 Solo", "T5 Solo", "T6 Solo", "T7 Solo", "T8 Solo",
];
const PAN_PARAMS: [&str; LOOP_TRACKS] = [
    "T1 Pan", "T2 Pan", "T3 Pan", "T4 Pan", "T5 Pan", "T6 Pan", "T7 Pan", "T8 Pan",
];
const VOL_PARAMS: [&str; LOOP_TRACKS] = [
    "T1 Vol", "T2 Vol", "T3 Vol", "T4 Vol", "T5 Vol", "T6 Vol", "T7 Vol", "T8 Vol",
];
const QUANT_PARAMS: [&str; LOOP_TRACKS] = [
    "T1 Quant", "T2 Quant", "T3 Quant", "T4 Quant", "T5 Quant", "T6 Quant", "T7 Quant", "T8 Quant",
];

/// Where a track's state knob sits: stopped, paused, playing, recording.
///
/// Four positions and not three, because PLAY is also PAUSE — see
/// [`LoopTrackState::Paused`].
pub const P_STOP: f32 = 0.0;
pub const P_PAUSE: f32 = 1.0 / 3.0;
pub const P_PLAY: f32 = 2.0 / 3.0;
pub const P_REC: f32 = 1.0;

/// The parameter list, in blocks of one per track. Everything a channel strip
/// **decides** is one of these, which is what makes it learnable, automatable
/// and saved with the project without a path of its own.
///
/// What a strip only *shows* is not here: the input monitor is a meter, and
/// METRO works choz's own metronome rather than a setting of its own.
pub const P_STATE: usize = 0;
pub const P_MUTE: usize = LOOP_TRACKS;
/// What a closed take rounds to. Per track: a channel is a take, and the button
/// the player reaches for is on the channel.
pub const P_QUANT: usize = LOOP_TRACKS * 2;
/// How many channel strips the deck offers. Not how many fit on screen — that
/// is the panel's business, and why there are page arrows.
pub const P_CHANS: usize = LOOP_TRACKS * 3;
/// Solo and pan, added after `P_CHANS` **on purpose**: a project saved before
/// they existed keeps every index it wrote, because none of the older blocks
/// moved.
pub const P_SOLO: usize = LOOP_TRACKS * 3 + 1;
pub const P_PAN: usize = LOOP_TRACKS * 4 + 1;
/// Which channel the `X` on a strip is throwing away, as `(track + 1) / 8`.
///
/// A gesture and not a state, so it is written and then written back to zero —
/// [`FxProcessor::set_param`] acts on edges, so both land. A parameter anyway,
/// because parameters are the only road from the interface to a processor.
pub const P_DEL: usize = LOOP_TRACKS * 5 + 1;
/// How loud one channel's take plays, `0..1` linear — the strip's fader.
pub const P_VOL: usize = LOOP_TRACKS * 5 + 2;
pub const LOOP_PARAMS: usize = LOOP_TRACKS * 6 + 2;

/// The `P_DEL` value that means "throw away channel `t`", and the channel one
/// means.
pub fn del_param(track: usize) -> f32 {
    (track.min(LOOP_TRACKS - 1) + 1) as f32 / LOOP_TRACKS as f32
}

pub fn del_of(value: f32) -> Option<usize> {
    match ((value.clamp(0.0, 1.0) * LOOP_TRACKS as f32).round() as usize).checked_sub(1) {
        Some(t) if t < LOOP_TRACKS => Some(t),
        _ => None,
    }
}

/// A pan parameter (`0..1`, centre at `0.5`) as the `-1..1` the mixer speaks.
pub fn pan_of(value: f32) -> f32 {
    value.clamp(0.0, 1.0) * 2.0 - 1.0
}

pub fn pan_param(pan: f32) -> f32 {
    (pan.clamp(-1.0, 1.0) + 1.0) / 2.0
}

/// The channel count a `P_CHANS` value means.
///
/// Zero is not one channel: it is what an unset parameter reads as — a fresh
/// deck, or a project saved before this existed — and the answer there is the
/// default four.
pub fn chans_of(v: f32) -> usize {
    match v < 0.05 {
        true => 4,
        false => ((v * LOOP_TRACKS as f32).round() as usize).clamp(1, LOOP_TRACKS),
    }
}

pub fn chans_param(n: usize) -> f32 {
    n.clamp(1, LOOP_TRACKS) as f32 / LOOP_TRACKS as f32
}

/// The knob position that says `state`.
pub fn param_of(state: LoopTrackState) -> f32 {
    match state {
        LoopTrackState::Idle => P_STOP,
        LoopTrackState::Paused => P_PAUSE,
        LoopTrackState::Playing => P_PLAY,
        LoopTrackState::Recording => P_REC,
    }
}

/// The state a knob at `value` means. Nearest of the four positions, so a wheel
/// that lands between two says the closer one rather than nothing.
pub fn state_of(value: f32) -> LoopTrackState {
    const ALL: [LoopTrackState; 4] = [
        LoopTrackState::Idle,
        LoopTrackState::Paused,
        LoopTrackState::Playing,
        LoopTrackState::Recording,
    ];
    let k = ((value.clamp(0.0, 1.0) * 3.0).round() as usize).min(3);
    ALL[k]
}

/// One take.
struct Track {
    /// The audio, a chunk a second. Sized once at construction so pushing a
    /// chunk on the audio thread is a write into a slot that is already there.
    chunks: Vec<Option<LoopChunk>>,
    state: LoopTrackState,
    /// Frames written so far. Frozen at the deck's length once it closes.
    frames: usize,
    muted: bool,
    /// Out of the mix while any other channel is soloed. Mute says "not this
    /// one"; solo says "only these", and a deck with none soloed is every
    /// channel as it was.
    solo: bool,
    /// Where the take sits between the speakers, `-1..1`.
    pan: f32,
    /// How loud it plays, linear. Unity until the fader is touched.
    gain: f32,
    /// What this channel's take rounds to when it closes.
    quantise: Quantise,
}

impl Track {
    fn new(max_chunks: usize) -> Self {
        Self {
            chunks: (0..max_chunks).map(|_| None).collect(),
            state: LoopTrackState::Idle,
            frames: 0,
            muted: false,
            solo: false,
            pan: 0.0,
            gain: 1.0,
            quantise: Quantise::default(),
        }
    }

    /// One frame of this track at `pos`, or silence where nothing was recorded.
    fn at(&self, pos: usize, chunk_frames: usize) -> (f32, f32) {
        if self.muted || pos >= self.frames {
            return (0.0, 0.0);
        }
        let Some(Some(chunk)) = self.chunks.get(pos / chunk_frames) else {
            return (0.0, 0.0);
        };
        let i = (pos % chunk_frames) * 2;
        // Balance, not a pan law: the same arithmetic the mixer's strips use,
        // so `L50` means the same thing on both panels.
        let (gl, gr) = (
            (1.0 - self.pan).min(1.0) * self.gain,
            (1.0 + self.pan).min(1.0) * self.gain,
        );
        match (chunk.get(i), chunk.get(i + 1)) {
            (Some(l), Some(r)) => (*l as f32 / SCALE * gl, *r as f32 / SCALE * gr),
            _ => (0.0, 0.0),
        }
    }

    fn clear(&mut self, retired: &mut Vec<LoopChunk>) {
        for slot in self.chunks.iter_mut() {
            if let Some(chunk) = slot.take() {
                retired.push(chunk);
            }
        }
        self.state = LoopTrackState::Idle;
        self.frames = 0;
    }
}

/// What the interface asks the deck to do. Applied at block boundaries, the way
/// the single-track looper already applied its own.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Cmd {
    Record(usize),
    Play(usize),
    Pause(usize),
    Stop(usize),
    /// Freeze the take and leave it playing — how a take ends when it was REC
    /// that ended it, and when the ceiling did.
    Close(usize),
    Clear(usize),
    Mute(usize, bool),
    Solo(usize, bool),
    Pan(usize, f32),
    Gain(usize, f32),
    /// Throw the channel away and close the gap: the strips above it slide
    /// down, audio and settings together.
    Remove(usize),
    ClearAll,
}

/// Eight takes over one source.
pub struct Looper {
    tracks: Vec<Track>,
    /// The length track 1 froze. `0` until it does.
    loop_frames: usize,
    /// One playhead for the whole deck: tracks are mutes, not transports, so
    /// they cannot drift apart and there is no per-track resync to write.
    pos: usize,
    /// Empty chunks arriving from the interface, and full ones going home.
    supply: Option<rtrb::Consumer<LoopChunk>>,
    home: Option<rtrb::Producer<LoopFilled>>,
    /// The chunk each recording track is filling, and how far into it.
    writing: Vec<Option<LoopChunk>>,
    state: Arc<LoopState>,
    /// Handed out once, at build time.
    handle: Option<LoopHandle>,
    pending: Vec<Cmd>,
    /// The last value each parameter was set to, so automation that repeats
    /// itself does not re-trigger a transport — see [`FxProcessor::set_param`].
    last_param: Vec<f32>,
    /// Chunks a cleared track gave back, sent home on the next block. Never
    /// dropped here: the interface holds a reference to every one of them, so a
    /// drop on this thread is a refcount decrement — but only while that holds.
    retired: Vec<LoopChunk>,
    chunk_frames: usize,
    max_frames: usize,
    sample_rate: u32,
    /// Channel strips on offer. Four unless the player asked for more.
    chans: usize,
    wet: f32,
}

impl Looper {
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate.max(1) as usize;
        let chunk_frames = LOOP_CHUNK_SECS * sr;
        let max_frames = MAX_SECS * sr;
        // One more than the ceiling needs, so the last partial second has a slot
        // to land in.
        let max_chunks = max_frames.div_ceil(chunk_frames) + 1;
        let (handle, supply, home, state) = LoopHandle::pair(sample_rate, RING);
        Self {
            tracks: (0..LOOP_TRACKS).map(|_| Track::new(max_chunks)).collect(),
            loop_frames: 0,
            pos: 0,
            supply: Some(supply),
            home: Some(home),
            writing: (0..LOOP_TRACKS).map(|_| None).collect(),
            state,
            handle: Some(handle),
            pending: Vec::with_capacity(LOOP_TRACKS * 2),
            last_param: vec![f32::NAN; LOOP_PARAMS],
            retired: Vec::with_capacity(max_chunks),
            chunk_frames,
            max_frames,
            sample_rate: sample_rate.max(1),
            chans: 4,
            wet: 1.0,
        }
    }

    // ── What the interface asks for ────────────────────────────────────────

    pub fn record(&mut self, track: usize) {
        self.pending.push(Cmd::Record(track));
    }
    pub fn play(&mut self, track: usize) {
        self.pending.push(Cmd::Play(track));
    }
    pub fn pause(&mut self, track: usize) {
        self.pending.push(Cmd::Pause(track));
    }
    pub fn stop(&mut self, track: usize) {
        self.pending.push(Cmd::Stop(track));
    }
    pub fn clear(&mut self, track: usize) {
        self.pending.push(Cmd::Clear(track));
    }
    pub fn set_muted(&mut self, track: usize, on: bool) {
        self.pending.push(Cmd::Mute(track, on));
    }
    pub fn set_solo(&mut self, track: usize, on: bool) {
        self.pending.push(Cmd::Solo(track, on));
    }
    pub fn set_pan(&mut self, track: usize, pan: f32) {
        self.pending.push(Cmd::Pan(track, pan.clamp(-1.0, 1.0)));
    }
    pub fn set_gain(&mut self, track: usize, gain: f32) {
        self.pending.push(Cmd::Gain(track, gain.clamp(0.0, 1.0)));
    }
    pub fn remove_track(&mut self, track: usize) {
        self.pending.push(Cmd::Remove(track));
    }
    pub fn clear_all(&mut self) {
        self.pending.push(Cmd::ClearAll);
    }

    /// REC on a track: arm it if it is idle, close it if it is recording.
    pub fn toggle_record(&mut self, track: usize) {
        match self.track_state(track) {
            LoopTrackState::Recording => self.pending.push(Cmd::Close(track)),
            _ => self.record(track),
        }
    }

    /// PLAY on a track, which is also its PAUSE: a playing take goes quiet and
    /// keeps its place in the loop; anything else starts playing. A recording
    /// one is closed first, which is what a player pressing PLAY means.
    pub fn toggle_play(&mut self, track: usize) {
        match self.track_state(track) {
            LoopTrackState::Playing => self.pause(track),
            LoopTrackState::Recording => self.pending.push(Cmd::Close(track)),
            LoopTrackState::Idle | LoopTrackState::Paused => self.play(track),
        }
    }

    pub fn track_state(&self, track: usize) -> LoopTrackState {
        self.tracks
            .get(track)
            .map(|t| t.state)
            .unwrap_or(LoopTrackState::Idle)
    }

    pub fn loop_frames(&self) -> usize {
        self.loop_frames
    }

    pub fn quantise(&self, track: usize) -> Quantise {
        self.tracks
            .get(track)
            .map(|t| t.quantise)
            .unwrap_or_default()
    }

    /// Set what a channel's closed take rounds to. Takes effect on the **next**
    /// take: a deck whose length is already frozen is a deck whose length is
    /// the answer.
    pub fn set_quantise(&mut self, track: usize, q: Quantise) {
        if let Some(t) = self.tracks.get_mut(track) {
            t.quantise = q;
        }
    }

    /// How many channel strips the deck offers.
    pub fn chans(&self) -> usize {
        self.chans
    }

    // ── The commands, at a block boundary ──────────────────────────────────

    fn apply(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Record(t) => self.start_record(t),
            Cmd::Play(t) => self.start_play(t),
            Cmd::Pause(t) => self.pause_track(t),
            Cmd::Stop(t) => self.stop_track(t),
            Cmd::Close(t) => self.close_track(t, true),
            Cmd::Clear(t) => {
                if let Some(track) = self.tracks.get_mut(t) {
                    track.clear(&mut self.retired);
                }
                if let Some(chunk) = self.writing.get_mut(t).and_then(|c| c.take()) {
                    self.retired.push(chunk);
                }
                // Track 1 owns the length: clearing it lets the next thing
                // recorded define one again.
                if t == 0 {
                    self.loop_frames = 0;
                    self.pos = 0;
                }
            }
            Cmd::Mute(t, on) => {
                if let Some(track) = self.tracks.get_mut(t) {
                    track.muted = on;
                }
            }
            Cmd::Solo(t, on) => {
                if let Some(track) = self.tracks.get_mut(t) {
                    track.solo = on;
                }
            }
            Cmd::Pan(t, pan) => {
                if let Some(track) = self.tracks.get_mut(t) {
                    track.pan = pan;
                }
            }
            Cmd::Gain(t, gain) => {
                if let Some(track) = self.tracks.get_mut(t) {
                    track.gain = gain;
                }
            }
            Cmd::Remove(t) => self.remove(t),
            Cmd::ClearAll => {
                for t in 0..self.tracks.len() {
                    self.apply(Cmd::Clear(t));
                }
            }
        }
    }

    /// Throw one channel away, and slide everything above it down.
    ///
    /// A rotate and not a `remove`: the audio thread may not allocate, and
    /// `Vec::rotate_left` moves the chunk handles without asking for a byte.
    /// The last strip is left empty, which is where the count that just shrank
    /// was pointing anyway.
    ///
    /// Every per-channel thing rotates together — the take, the state, the
    /// settings, and the last value each parameter was set to. Leaving that
    /// last one behind would make the interface's next press look like a repeat
    /// and get swallowed.
    fn remove(&mut self, t: usize) {
        if t >= self.tracks.len() || self.tracks.len() < 2 {
            return;
        }
        let mut retired = std::mem::take(&mut self.retired);
        if let Some(track) = self.tracks.get_mut(t) {
            track.clear(&mut retired);
            track.muted = false;
            track.solo = false;
            track.pan = 0.0;
            track.gain = 1.0;
        }
        self.retired = retired;
        if let Some(chunk) = self.writing.get_mut(t).and_then(|c| c.take()) {
            self.retired.push(chunk);
        }
        self.tracks[t..].rotate_left(1);
        self.writing[t..].rotate_left(1);
        for block in [P_STATE, P_MUTE, P_QUANT, P_SOLO, P_PAN, P_VOL] {
            let end = block + LOOP_TRACKS;
            if end <= self.last_param.len() {
                self.last_param[block + t..end].rotate_left(1);
            }
        }
        self.chans = self.chans.saturating_sub(1).max(1);
        // The length belongs to the deck, not to whichever channel is first
        // now: it only goes away when there is nothing left holding it.
        if !self.tracks.iter().any(|tr| tr.frames > 0) {
            self.loop_frames = 0;
            self.pos = 0;
        }
    }

    fn start_record(&mut self, t: usize) {
        if t >= self.tracks.len() {
            return;
        }
        // A track other than the first cannot record before there is a length
        // to record into: the deck has no idea how long it would be.
        if t != 0 && self.loop_frames == 0 {
            return;
        }
        let mut retired = std::mem::take(&mut self.retired);
        if let Some(track) = self.tracks.get_mut(t) {
            track.clear(&mut retired);
            track.state = LoopTrackState::Recording;
        }
        self.retired = retired;
        if let Some(chunk) = self.writing.get_mut(t).and_then(|c| c.take()) {
            self.retired.push(chunk);
        }
        // Track 1 starts the deck over; the others join the loop already running.
        if t == 0 {
            self.pos = 0;
        }
    }

    fn start_play(&mut self, t: usize) {
        // PLAY on a take still being recorded **closes** it — which is what
        // freezes the deck's length, sends the half-written chunk home and
        // leaves the take rolling. Without this the take never closes:
        // `loop_frames` stays zero, the playback branch never runs, and a
        // player who armed REC and pressed it again hears nothing at all.
        if self.track_state(t) == LoopTrackState::Recording {
            self.close_track(t, true);
            return;
        }
        if self.tracks.get(t).is_some_and(|tr| tr.frames > 0) {
            if let Some(track) = self.tracks.get_mut(t) {
                track.state = LoopTrackState::Playing;
            }
        }
    }

    /// Quiet, but still in the loop. The playhead belongs to the deck, so a
    /// paused take rejoins where the others are rather than at the top.
    fn pause_track(&mut self, t: usize) {
        if let Some(track) = self.tracks.get_mut(t) {
            if track.state == LoopTrackState::Playing {
                track.state = LoopTrackState::Paused;
            }
        }
    }

    /// Close whatever the track was doing.
    ///
    /// A recording track always freezes its length first — nothing here may
    /// lose a take. `then_play` is the difference between the two ways a take
    /// ends: REC's second press (and hitting the ceiling) leaves it playing,
    /// the way a looper pedal does, while STOP leaves it out of the loop and
    /// rewinds the deck once nothing is holding the playhead any more.
    fn close_track(&mut self, t: usize, then_play: bool) {
        let Some(track) = self.tracks.get_mut(t) else {
            return;
        };
        if track.state != LoopTrackState::Recording {
            track.state = LoopTrackState::Idle;
            if !self.deck_running() {
                self.pos = 0;
            }
            return;
        }
        // Whatever is half-written belongs to the take.
        if let Some(chunk) = self.writing.get_mut(t).and_then(|c| c.take()) {
            let index = track.frames.saturating_sub(1) / self.chunk_frames;
            if let Some(slot) = track.chunks.get_mut(index) {
                *slot = Some(chunk.clone());
            }
            if let Some(home) = self.home.as_mut() {
                let _ = home.push(LoopFilled {
                    track: t,
                    index,
                    chunk,
                });
            }
        }
        track.state = match then_play {
            true => LoopTrackState::Playing,
            false => LoopTrackState::Idle,
        };
        let frames = track.frames;
        if t == 0 && self.loop_frames == 0 {
            // Rounded here and nowhere else: the length track 1 freezes is the
            // length every other track will be, so quantising it once quantises
            // the whole deck. Never past the ceiling — a take that ran to five
            // minutes must not round to five minutes and a bar.
            let rounded = self.quantise(0).round(frames, self.sample_rate);
            self.loop_frames = rounded.min(self.max_frames).max(1);
            self.pos = 0;
        }
        // Every track is the deck's length: a later take that came up short is
        // silence to the end of the loop, not a loop of its own.
        if let Some(track) = self.tracks.get_mut(t) {
            track.frames = frames;
        }
        if !then_play && !self.deck_running() {
            self.pos = 0;
        }
    }

    /// STOP: out of the loop, and the deck back to the top if it was the last.
    fn stop_track(&mut self, t: usize) {
        self.close_track(t, false);
    }

    /// Whether anything still holds the playhead — a playing take, or a paused
    /// one waiting to come back in time with it.
    fn deck_running(&self) -> bool {
        self.tracks.iter().any(|t| {
            matches!(
                t.state,
                LoopTrackState::Playing | LoopTrackState::Recording | LoopTrackState::Paused
            )
        })
    }

    // ── The block ──────────────────────────────────────────────────────────

    /// Write one frame into whichever tracks are recording.
    ///
    /// Returns `false` when a track needed a chunk and there was none — the one
    /// case that closes the loop, because the alternative is audio that goes
    /// missing without saying so.
    fn record_frame(&mut self, t: usize, l: f32, r: f32) -> bool {
        let Some(track) = self.tracks.get(t) else {
            return true;
        };
        let frames = track.frames;
        // The ceiling. Track 1 has the whole of it; the rest have the deck's
        // length, whatever that turned out to be.
        let ceiling = match self.loop_frames {
            0 => self.max_frames,
            n => n,
        };
        if frames >= ceiling {
            return false;
        }
        let index = frames / self.chunk_frames;
        let into = (frames % self.chunk_frames) * 2;
        if into == 0 {
            // A chunk boundary: send the one just filled home and take a fresh
            // one. `clone` is a refcount bump — the interface reads the same
            // memory, and from here it is never written again.
            if let Some(full) = self.writing.get_mut(t).and_then(|c| c.take()) {
                let done = index.saturating_sub(1);
                if let Some(slot) = self.tracks[t].chunks.get_mut(done) {
                    *slot = Some(full.clone());
                }
                if let Some(home) = self.home.as_mut() {
                    let _ = home.push(LoopFilled {
                        track: t,
                        index: done,
                        chunk: full,
                    });
                }
            }
            let Some(fresh) = self.supply.as_mut().and_then(|s| s.pop().ok()) else {
                self.state.set_starved(true);
                return false;
            };
            self.writing[t] = Some(fresh);
        }
        let Some(chunk) = self.writing[t].as_mut() else {
            return false;
        };
        // Unique while the audio thread is filling it, so this is a plain
        // write — and it stops being unique the moment it goes home, which is
        // exactly when it must stop being written.
        let Some(audio) = Arc::get_mut(chunk) else {
            return false;
        };
        if let (Some(a), Some(b)) = (audio.get_mut(into), None::<&mut i16>) {
            let _ = (a, b);
        }
        let clip = |v: f32| (v.clamp(-1.0, 1.0) * SCALE) as i16;
        if into + 1 < audio.len() {
            audio[into] = clip(l);
            audio[into + 1] = clip(r);
        }
        self.tracks[t].frames = frames + 1;
        true
    }

    fn publish(&self) {
        for t in 0..self.tracks.len() {
            self.state.set_track(t, self.tracks[t].state);
        }
        self.state.set_frames(self.loop_frames);
        self.state.set_pos(self.pos);
        self.state
            .set_recorded(self.tracks.first().map(|t| t.frames).unwrap_or(0));
    }
}

impl FxProcessor for Looper {
    fn process_block(&mut self, buf: &mut [f32], _sample_rate: u32) {
        // Commands first, at the boundary — a transport that changed mid-block
        // would put half a frame in the wrong take.
        if !self.pending.is_empty() {
            let cmds = std::mem::take(&mut self.pending);
            for cmd in cmds.iter().copied() {
                self.apply(cmd);
            }
            self.pending = cmds;
            self.pending.clear();
        }
        // Cleared chunks go home to be freed on the interface thread.
        if !self.retired.is_empty() {
            while let Some(chunk) = self.retired.pop() {
                let Some(home) = self.home.as_mut() else {
                    break;
                };
                if home
                    .push(LoopFilled {
                        track: usize::MAX,
                        index: 0,
                        chunk,
                    })
                    .is_err()
                {
                    break;
                }
            }
        }

        let frames = buf.len() / 2;
        // What is arriving, before anything of this deck's is added to it —
        // which is what "incoming" has to mean for the panel's monitor to be
        // the number a player sets their gain against.
        let peak = buf.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        self.state.set_in_peak(peak);
        let recording: Vec<usize> = (0..self.tracks.len())
            .filter(|t| self.tracks[*t].state == LoopTrackState::Recording)
            .collect();
        let playing = self
            .tracks
            .iter()
            .any(|t| t.state == LoopTrackState::Playing);
        let soloed = self.tracks.iter().any(|t| t.solo);
        if recording.is_empty() && !playing {
            for t in 0..self.tracks.len() {
                self.state.set_track_level(t, 0.0, 0.0);
            }
            self.publish();
            return;
        }
        // What each channel is doing this block, for the strips' activity
        // monitors: `(peak, sum of squares)`, filled as the frames go by and
        // turned into an RMS at the end. A recording channel reports what it
        // is taking in and a playing one what it is putting out — which is, in
        // both cases, the answer to "is this strip doing anything".
        let mut level = [(0.0f32, 0.0f32); LOOP_TRACKS];
        let note = |lv: &mut (f32, f32), l: f32, r: f32| {
            lv.0 = lv.0.max(l.abs()).max(r.abs());
            lv.1 += (l * l + r * r) * 0.5;
        };

        for i in 0..frames {
            let (dry_l, dry_r) = (buf[i * 2], buf[i * 2 + 1]);

            // Record what arrives at the slot — not what leaves it, or a track
            // would re-record whatever the others are playing.
            //
            // The state is re-read every frame: a take that reached the deck's
            // length closed itself earlier in this same block, and asking it to
            // close a second time would take it from Playing to Idle.
            for &t in recording.iter() {
                if self.tracks[t].state != LoopTrackState::Recording {
                    continue;
                }
                if !self.record_frame(t, dry_l, dry_r) {
                    self.close_track(t, true);
                }
                if let Some(lv) = level.get_mut(t) {
                    note(lv, dry_l, dry_r);
                }
            }

            if playing && self.loop_frames > 0 {
                let (mut sum_l, mut sum_r) = (0.0f32, 0.0f32);
                for t in 0..self.tracks.len() {
                    if self.tracks[t].state != LoopTrackState::Playing {
                        continue;
                    }
                    // Solo is a mute of everything else, and only while
                    // something is soloed — no channel soloed is the deck as
                    // its mutes left it.
                    if soloed && !self.tracks[t].solo {
                        continue;
                    }
                    let (l, r) = self.tracks[t].at(self.pos, self.chunk_frames);
                    if let Some(lv) = level.get_mut(t) {
                        note(lv, l, r);
                    }
                    sum_l += l;
                    sum_r += r;
                }
                buf[i * 2] = dry_l + self.wet * sum_l;
                buf[i * 2 + 1] = dry_r + self.wet * sum_r;
                self.pos = (self.pos + 1) % self.loop_frames;
            }
        }
        let n = frames.max(1) as f32;
        for (t, (peak, sq)) in level.iter().enumerate().take(self.tracks.len()) {
            self.state.set_track_level(t, *peak, (sq / n).sqrt());
        }
        self.publish();
    }

    fn reset(&mut self) {
        self.clear_all();
    }

    fn set_mix(&mut self, wet: f32) {
        self.wet = wet.clamp(0.0, 1.0);
    }

    fn loopdeck(&mut self) -> Option<LoopHandle> {
        self.handle.take()
    }

    fn is_loop_deck(&self) -> bool {
        true
    }

    fn params(&self) -> Vec<choz_ports::FxParam> {
        // One block a control, one entry a channel, in the order `P_*` names
        // them. A parameter and not a field for every one of them: that is what
        // puts them in the project file, in the learn picker and in the
        // exported plugin at the same time.
        let mut out = Vec::with_capacity(LOOP_PARAMS);
        let each = |out: &mut Vec<choz_ports::FxParam>,
                    names: &[&'static str; LOOP_TRACKS],
                    unit: &'static str,
                    value: &dyn Fn(&Track) -> f32,
                    default: f32| {
            for (t, name) in names.iter().enumerate() {
                let v = self.tracks.get(t).map(value).unwrap_or(default);
                out.push(choz_ports::FxParam::new(name, v, 0.0, 1.0, unit));
            }
        };
        for (t, name) in TRACK_PARAMS.iter().enumerate() {
            out.push(choz_ports::FxParam::new(
                name,
                param_of(self.track_state(t)),
                0.0,
                1.0,
                "",
            ));
        }
        each(&mut out, &MUTE_PARAMS, "", &|tr| tr.muted as u8 as f32, 0.0);
        each(
            &mut out,
            &QUANT_PARAMS,
            "",
            &|tr| {
                Quantise::ALL
                    .iter()
                    .position(|q| *q == tr.quantise)
                    .unwrap_or(0) as f32
                    / (Quantise::ALL.len() - 1) as f32
            },
            0.0,
        );
        out.push(choz_ports::FxParam::new(
            "Chans",
            chans_param(self.chans),
            0.0,
            1.0,
            "",
        ));
        each(&mut out, &SOLO_PARAMS, "", &|tr| tr.solo as u8 as f32, 0.0);
        // Centre is `0.5`, so an unset one — a project written before this
        // block existed — reads as centred rather than as hard left.
        each(&mut out, &PAN_PARAMS, "", &|tr| pan_param(tr.pan), 0.5);
        out.push(choz_ports::FxParam::new("Del", 0.0, 0.0, 1.0, ""));
        // Unity by default, so a channel that was never faded plays at the
        // level it was recorded — and so does one from a project written
        // before this block existed.
        each(&mut out, &VOL_PARAMS, "", &|tr| tr.gain, 1.0);
        out
    }

    /// A knob moved, from wherever: the panel, a learned CC, or a host.
    ///
    /// **Edges, not levels.** A host re-sends the same automation value every
    /// block, and a REC that fired on every one of them would start the take
    /// over forty times a second. Only a change acts.
    fn set_param(&mut self, index: usize, value: f32) {
        let Some(last) = self.last_param.get_mut(index) else {
            return;
        };
        let value = value.clamp(0.0, 1.0);
        if (*last - value).abs() < 1e-4 {
            return;
        }
        *last = value;
        // Which block the index falls in, and which channel inside it. Named
        // bounds rather than `index / LOOP_TRACKS`: the blocks stopped being
        // eight-wide the moment `P_CHANS` sat between two of them.
        match index {
            i if i < P_MUTE => match state_of(value) {
                LoopTrackState::Recording => self.record(i - P_STATE),
                LoopTrackState::Playing => self.play(i - P_STATE),
                LoopTrackState::Paused => self.pause(i - P_STATE),
                LoopTrackState::Idle => self.stop(i - P_STATE),
            },
            i if i < P_QUANT => self.set_muted(i - P_MUTE, value >= 0.5),
            i if i < P_CHANS => {
                let n = Quantise::ALL.len();
                let k = ((value * (n - 1) as f32).round() as usize).min(n - 1);
                self.set_quantise(i - P_QUANT, Quantise::ALL[k]);
            }
            i if i == P_CHANS => self.chans = chans_of(value),
            i if i < P_PAN => self.set_solo(i - P_SOLO, value >= 0.5),
            i if i < P_DEL => self.set_pan(i - P_PAN, pan_of(value)),
            // Zero is the rest position this is written back to; only the
            // press does anything.
            i if i == P_DEL => {
                if let Some(t) = del_of(value) {
                    self.remove_track(t);
                }
            }
            i if i < LOOP_PARAMS => self.set_gain(i - P_VOL, value),
            _ => {}
        }
    }
}

/// The deck's state, for anything that reads it without owning it.
impl Looper {
    pub fn shared_state(&self) -> Arc<LoopState> {
        self.state.clone()
    }
}

// ─── Export ─────────────────────────────────────────────────────────────────

/// Write one track's take to `path` as a stereo WAV.
///
/// **Not on the audio thread.** It reads the chunks the deck sent home, which
/// the interface has been keeping all along — so exporting asks the callback for
/// nothing and can be done while the deck is still rolling: the frozen chunks
/// are not the ones being written into.
///
/// `i16` in and `i16` out: the take is copied, not converted, so nothing here
/// can clip what was recorded.
pub fn export_track(
    handle: &LoopHandle,
    track: usize,
    frames: usize,
    path: &std::path::Path,
) -> std::io::Result<usize> {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: handle.sample_rate(),
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec)
        .map_err(|e| std::io::Error::other(format!("{path:?}: {e}")))?;
    let mut written = 0usize;
    'chunks: for chunk in handle.take(track) {
        for frame in chunk.chunks_exact(2) {
            // The deck's length, not the chunk's: the last second of a take is
            // a whole chunk with silence after the end of the loop.
            if written >= frames {
                break 'chunks;
            }
            w.write_sample(frame[0])
                .and_then(|_| w.write_sample(frame[1]))
                .map_err(|e| std::io::Error::other(format!("{path:?}: {e}")))?;
            written += 1;
        }
    }
    w.finalize()
        .map_err(|e| std::io::Error::other(format!("{path:?}: {e}")))?;
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deck with its supply filled, as the interface would hand it over.
    fn deck(sr: u32, seconds: usize) -> (Looper, LoopHandle) {
        let mut lp = Looper::new(sr);
        let mut handle = lp.loopdeck().expect("a deck hands out its handle once");
        // Enough chunks for the whole take, so the tests are about the deck and
        // not about the interface keeping up.
        for _ in 0..seconds.max(1) + 2 {
            handle.pump(RING, usize::MAX);
        }
        (lp, handle)
    }

    fn ramp(frames: usize, from: f32) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let v = from + i as f32 * 1e-4;
                [v, v]
            })
            .collect()
    }

    /// The handle comes out once and only once: two ends of one ring cannot be
    /// cloned, and a second caller getting `None` is what says so.
    #[test]
    fn the_deck_hands_out_its_handle_once() {
        let mut lp = Looper::new(48_000);
        assert!(lp.loopdeck().is_some());
        assert!(lp.loopdeck().is_none(), "there is only one of them");
    }

    /// Record a take, close it, hear it back.
    #[test]
    fn a_take_records_and_plays() {
        let (mut lp, _h) = deck(8_000, 2);
        lp.record(0);
        let mut buf = ramp(4_000, 0.5);
        lp.process_block(&mut buf, 8_000);
        assert_eq!(lp.track_state(0), LoopTrackState::Recording);

        // REC's second press closes the take and leaves it playing, the way a
        // looper pedal does. STOP is the other ending — see the pause test.
        lp.toggle_record(0);
        // The loop is half a second long and it plays what went in.
        let mut out = vec![0.0f32; 2_000];
        lp.process_block(&mut out, 8_000);
        assert_eq!(lp.track_state(0), LoopTrackState::Playing);
        assert_eq!(lp.loop_frames(), 4_000, "track 1 froze the length");
        let energy: f32 = out.iter().map(|s| s * s).sum();
        assert!(energy > 0.0, "the take plays back: {energy}");
    }

    /// Nothing armed and nothing playing leaves the signal exactly as it came.
    #[test]
    fn an_idle_deck_is_a_wire() {
        let (mut lp, _h) = deck(8_000, 1);
        let mut buf = vec![0.5f32; 64];
        lp.process_block(&mut buf, 8_000);
        assert!(buf.iter().all(|s| *s == 0.5), "idle passes the signal");
    }

    /// The first track defines the length, and every track after it is exactly
    /// that long — the rule the whole memory design rests on.
    #[test]
    fn the_first_track_defines_the_length() {
        let (mut lp, _h) = deck(8_000, 3);
        lp.record(0);
        lp.process_block(&mut ramp(2_400, 0.3), 8_000);
        lp.stop(0);
        lp.process_block(&mut [0.0f32; 32], 8_000);
        assert_eq!(lp.loop_frames(), 2_400);

        // A second take runs past the deck's length and is cut there.
        lp.record(1);
        lp.process_block(&mut ramp(8_000, 0.9), 8_000);
        assert_eq!(
            lp.tracks[1].frames, 2_400,
            "a later take is the deck's length, not its own"
        );
        assert_eq!(
            lp.track_state(1),
            LoopTrackState::Playing,
            "reaching the length closes the take"
        );
    }

    /// A track cannot record before there is a length to record into.
    #[test]
    fn a_later_track_will_not_record_before_the_first() {
        let (mut lp, _h) = deck(8_000, 1);
        lp.record(3);
        lp.process_block(&mut ramp(800, 0.5), 8_000);
        assert_eq!(
            lp.track_state(3),
            LoopTrackState::Idle,
            "there is no length yet, so there is nothing to join"
        );
    }

    /// Clearing track 1 gives the deck its length back: the next thing recorded
    /// defines it again.
    #[test]
    fn clearing_the_first_track_frees_the_length() {
        let (mut lp, _h) = deck(8_000, 2);
        lp.record(0);
        lp.process_block(&mut ramp(1_600, 0.4), 8_000);
        lp.stop(0);
        lp.process_block(&mut [0.0f32; 32], 8_000);
        assert_eq!(lp.loop_frames(), 1_600);

        lp.clear(0);
        lp.process_block(&mut [0.0f32; 32], 8_000);
        assert_eq!(lp.loop_frames(), 0, "the deck has no length again");
        assert_eq!(lp.track_state(0), LoopTrackState::Idle);
    }

    /// Two takes sound together, and muting one takes it out without stopping
    /// it — a track is a mute, not a transport of its own.
    #[test]
    fn takes_play_together_and_a_mute_takes_one_out() {
        let (mut lp, _h) = deck(8_000, 3);
        lp.record(0);
        lp.process_block(&mut vec![0.5f32; 1_600], 8_000);
        lp.toggle_record(0);
        lp.process_block(&mut [0.0f32; 32], 8_000);

        lp.record(1);
        lp.process_block(&mut vec![0.5f32; 1_600], 8_000);
        lp.toggle_record(1);
        lp.process_block(&mut [0.0f32; 32], 8_000);

        let peak = |lp: &mut Looper| {
            let mut out = vec![0.0f32; 512];
            lp.process_block(&mut out, 8_000);
            out.iter().fold(0.0f32, |m, s| m.max(s.abs()))
        };
        let both = peak(&mut lp);
        lp.set_muted(1, true);
        let one = peak(&mut lp);
        assert!(
            both > one && one > 0.0,
            "two takes are louder than one, and one still sounds: {both} vs {one}"
        );
    }

    /// The panel's REC sends PLAY to end a take, and PLAY on a take still
    /// recording has to **close** it — freeze the length and leave it rolling.
    ///
    /// This is the bug the strips were unusable for: `Cmd::Play` used to set
    /// the state to Playing without closing, so `loop_frames` stayed at zero,
    /// the playback branch never ran, and pressing REC twice was silent.
    #[test]
    fn play_on_a_recording_take_closes_it_and_it_sounds() {
        let (mut lp, _h) = deck(8_000, 2);
        lp.set_param(P_STATE, P_REC);
        lp.process_block(&mut vec![0.5f32; 3_200], 8_000);
        assert_eq!(lp.track_state(0), LoopTrackState::Recording);

        lp.set_param(P_STATE, P_PLAY);
        let mut out = vec![0.0f32; 512];
        lp.process_block(&mut out, 8_000);
        assert_eq!(lp.loop_frames(), 1_600, "the take closed and froze it");
        assert_eq!(lp.track_state(0), LoopTrackState::Playing);
        assert!(
            out.iter().fold(0.0f32, |m, s| m.max(s.abs())) > 0.0,
            "and it is audible: {out:?}"
        );
    }

    /// A take of a known level, for the tests below to fade, pan and solo.
    fn take(lp: &mut Looper, track: usize, level: f32) {
        lp.record(track);
        lp.process_block(&mut vec![level; 1_600], 8_000);
        lp.toggle_record(track);
        lp.process_block(&mut [0.0f32; 32], 8_000);
    }

    /// The fader scales a channel, the pan moves it across, and solo takes
    /// everything that is not soloed out of the mix.
    #[test]
    fn a_channel_has_a_level_a_pan_and_a_solo() {
        let (mut lp, _h) = deck(8_000, 3);
        take(&mut lp, 0, 0.5);
        take(&mut lp, 1, 0.5);
        // Loud enough to tell apart, quiet enough not to clip the sum.
        let sides = |lp: &mut Looper| {
            let mut out = vec![0.0f32; 512];
            lp.process_block(&mut out, 8_000);
            let peak = |off: usize| {
                out.iter()
                    .skip(off)
                    .step_by(2)
                    .fold(0.0f32, |m, s| m.max(s.abs()))
            };
            (peak(0), peak(1))
        };

        let (full, _) = sides(&mut lp);
        lp.set_param(P_VOL, 0.5);
        let (half, _) = sides(&mut lp);
        assert!(
            half < full && half > 0.0,
            "the fader scales it: {full} -> {half}"
        );

        // Hard right on channel 1: its left goes away, and channel 2 is still
        // in the middle, so the left is quieter than the right.
        lp.set_param(P_PAN, pan_param(1.0));
        let (l, r) = sides(&mut lp);
        assert!(l < r, "panned right: {l} vs {r}");

        // Solo on the second channel: the first is out, whatever its fader
        // and pan said.
        lp.set_param(P_SOLO + 1, 1.0);
        let (l, r) = sides(&mut lp);
        assert!(
            (l - r).abs() < 1e-6 && l > 0.0,
            "only the centred take is left: {l} vs {r}"
        );
    }

    /// `X` on a strip throws that channel away and slides the ones above it
    /// down — the take that was on channel 2 answers as channel 1.
    #[test]
    fn removing_a_channel_slides_the_rest_down() {
        let (mut lp, _h) = deck(8_000, 3);
        take(&mut lp, 0, 0.25);
        take(&mut lp, 1, 0.25);
        lp.set_param(P_MUTE + 1, 1.0);
        assert_eq!(lp.chans(), 4);

        lp.set_param(P_DEL, del_param(0));
        lp.process_block(&mut [0.0f32; 32], 8_000);
        assert_eq!(lp.chans(), 3, "one strip fewer");
        assert_eq!(
            lp.track_state(0),
            LoopTrackState::Playing,
            "what was channel 2 is channel 1 now"
        );
        assert!(lp.tracks[0].muted, "and it brought its mute with it");
        assert_eq!(lp.track_state(1), LoopTrackState::Idle, "the top is empty");
        assert_eq!(lp.loop_frames(), 800, "a take still holds the length");
    }

    /// Running out of chunks closes the loop instead of losing audio quietly.
    #[test]
    fn a_starved_deck_closes_the_loop_rather_than_drop_audio() {
        let mut lp = Looper::new(8_000);
        let mut handle = lp.loopdeck().unwrap();
        // One pump only: a second of chunks, and the take asks for more.
        handle.pump(1, usize::MAX);
        lp.record(0);
        lp.process_block(&mut ramp(40_000, 0.5), 8_000);
        assert!(lp.state.take_starved(), "it said so");
        assert_ne!(
            lp.track_state(0),
            LoopTrackState::Recording,
            "and it stopped rather than write into nothing"
        );
    }

    /// Quantising rounds the closed loop to a whole unit, and never to nothing.
    #[test]
    fn quantise_rounds_and_has_a_floor() {
        let _g = crate::test_locks::transport();
        let t = choz_ports::transport();
        t.set_bpm(120.0);
        t.set_time_signature(4, 4);

        // A bar at 120 BPM in 4/4 is two seconds.
        let bar = Quantise::Bar.frames(48_000).unwrap();
        assert_eq!(bar, 96_000);

        // Just under a bar rounds up to one; well over rounds to two.
        assert_eq!(Quantise::Bar.round(91_000, 48_000), bar);
        assert_eq!(Quantise::Bar.round(180_000, 48_000), bar * 2);
        // And a stab shorter than anything is still a bar, not nothing.
        assert_eq!(Quantise::Bar.round(120, 48_000), bar);
        assert_eq!(Quantise::Second.round(120, 48_000), 48_000);

        // Off leaves the take alone, but never at zero.
        assert_eq!(Quantise::Off.round(1_234, 48_000), 1_234);
        assert_eq!(Quantise::Off.round(0, 48_000), 1);
    }

    /// The deck freezes the **quantised** length, so every later take is on the
    /// grid because the first one was.
    #[test]
    fn the_deck_freezes_the_quantised_length() {
        let _g = crate::test_locks::transport();
        let t = choz_ports::transport();
        t.set_bpm(120.0);
        t.set_time_signature(4, 4);

        // 8 kHz: a bar is 16 000 frames. Record a bit under one.
        let (mut lp, _h) = deck(8_000, 4);
        lp.set_quantise(0, Quantise::Bar);
        lp.record(0);
        lp.process_block(&mut ramp(15_000, 0.4), 8_000);
        lp.stop(0);
        lp.process_block(&mut [0.0f32; 32], 8_000);
        assert_eq!(lp.loop_frames(), 16_000, "rounded up to the bar");
    }

    /// A take goes out as a WAV of exactly the deck's length, at the rate it was
    /// recorded at.
    #[test]
    fn a_take_exports_as_a_wav() {
        let (mut lp, mut handle) = deck(8_000, 4);
        lp.record(0);
        lp.process_block(&mut vec![0.75f32; 2 * 12_000], 8_000);
        lp.stop(0);
        lp.process_block(&mut [0.0f32; 32], 8_000);
        handle.pump(RING, usize::MAX);

        let path = std::env::temp_dir().join("choz_test_loop_export.wav");
        let _ = std::fs::remove_file(&path);
        let frames = lp.loop_frames();
        let written = export_track(&handle, 0, frames, &path).expect("it writes");
        assert_eq!(written, frames, "exactly the loop, no more");

        let reader = hound::WavReader::open(&path).expect("it reads back");
        assert_eq!(reader.spec().channels, 2);
        assert_eq!(
            reader.spec().sample_rate,
            8_000,
            "the rate it was recorded at"
        );
        assert_eq!(reader.spec().bits_per_sample, 16);
        assert_eq!(reader.len() as usize, frames * 2);
        let peak = reader
            .into_samples::<i16>()
            .filter_map(|s| s.ok())
            .fold(0i16, |m, s| m.max(s.abs()));
        assert!(peak > 20_000, "and it carries the audio: {peak}");
        let _ = std::fs::remove_file(&path);
    }

    /// The interface can read what was recorded — which is what EXPORT needs.
    #[test]
    fn what_was_recorded_comes_home_for_export() {
        let (mut lp, mut handle) = deck(8_000, 3);
        lp.record(0);
        // Two full chunks and a bit, so the boundary path runs.
        lp.process_block(&mut vec![0.6f32; 2 * 17_000], 8_000);
        lp.stop(0);
        lp.process_block(&mut [0.0f32; 32], 8_000);
        handle.pump(RING, usize::MAX);

        let take = handle.take(0);
        assert!(take.len() >= 2, "both chunks came home: {}", take.len());
        let loud = take
            .iter()
            .flat_map(|c| c.iter())
            .filter(|s| **s != 0)
            .count();
        assert!(loud > 8_000, "and they carry the audio: {loud}");
    }
    /// PLAY is also PAUSE, and PAUSE is not STOP.
    ///
    /// The deck has one playhead, so the difference is what happens to it: a
    /// paused take goes quiet and the loop keeps running under it, so letting
    /// it back in puts it where the others are. STOP gives the take up, and
    /// once nothing is left holding the playhead the deck goes back to the top.
    #[test]
    fn play_pauses_in_place_and_stop_rewinds_the_deck() {
        let (mut lp, _h) = deck(8_000, 3);
        lp.record(0);
        lp.process_block(&mut vec![0.5f32; 2 * 1_600], 8_000);
        lp.toggle_record(0);
        // A second take, so there is something still playing to pause against.
        lp.record(1);
        lp.process_block(&mut vec![0.5f32; 2 * 1_600], 8_000);
        lp.toggle_record(1);
        lp.process_block(&mut [0.0f32; 64], 8_000);
        assert_eq!(lp.track_state(0), LoopTrackState::Playing);

        // PAUSE: quiet, but the loop under it keeps running.
        lp.toggle_play(0);
        lp.process_block(&mut [0.0f32; 2 * 400], 8_000);
        assert_eq!(lp.track_state(0), LoopTrackState::Paused);
        let paused_at = lp.state.pos();
        assert!(paused_at > 0, "the deck kept going: {paused_at}");

        // …and PLAY again takes it back, in time rather than at the top.
        lp.toggle_play(0);
        lp.process_block(&mut [0.0f32; 64], 8_000);
        assert_eq!(lp.track_state(0), LoopTrackState::Playing);
        assert!(
            lp.state.pos() >= paused_at,
            "resuming does not rewind: {} vs {paused_at}",
            lp.state.pos()
        );

        // STOP on the last one holding the playhead puts the deck back on top.
        lp.stop(0);
        lp.stop(1);
        lp.process_block(&mut [0.0f32; 64], 8_000);
        assert_eq!(lp.track_state(0), LoopTrackState::Idle);
        assert_eq!(
            lp.state.pos(),
            0,
            "nothing left running, so back to the top"
        );
    }

    /// The panel's dB monitor reads what is **arriving** at the deck — before
    /// anything the deck plays is added to it, or the number would rise just
    /// because a take is running.
    #[test]
    fn the_deck_publishes_the_level_arriving_at_it() {
        let (mut lp, _h) = deck(8_000, 2);
        lp.process_block(&mut vec![0.5f32; 64], 8_000);
        assert!(
            (lp.state.in_peak() - 0.5).abs() < 1e-6,
            "silence in, silence out: {}",
            lp.state.in_peak()
        );

        lp.record(0);
        lp.process_block(&mut vec![0.25f32; 2 * 800], 8_000);
        lp.toggle_record(0);
        lp.process_block(&mut vec![0.0f32; 64], 8_000);
        assert!(
            lp.state.in_peak() < 1e-6,
            "a take playing back is not something arriving: {}",
            lp.state.in_peak()
        );
    }
}
