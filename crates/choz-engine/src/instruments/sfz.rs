//! SFZ instruments: a text file mapping key ranges to sample files.
//!
//! ```text
//! <group> lovel=64 hivel=127
//! <region> sample=Saw_C-3.flac lokey=36 hikey=47 pitch_keycenter=36
//! ```
//!
//! Two halves: a parser for the subset of SFZ that freely-available
//! instruments actually use, and [`SfzSampler`], an [`AudioSource`] that plays
//! them. Ported from seqterm's `seqterm-sfz`, with one change that matters:
//! **every sample is decoded when the instrument loads**, not on note-on.
//! seqterm's version reads and decodes the file inside `note_on`, which choz
//! calls from the audio thread.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::sources::AudioSource;

/// Simultaneous voices. Past this the oldest is stolen, so `note_on` never
/// grows the vector — it is called from the audio thread.
const MAX_VOICES: usize = 32;

// ─── Parsing ────────────────────────────────────────────────────────────────

/// One region as written in the file: a sample path plus its key/velocity range.
#[derive(Debug, Clone, PartialEq)]
pub struct SfzRegion {
    pub sample: PathBuf,
    pub lo_key: u8,
    pub hi_key: u8,
    /// The pitch the sample was recorded at, for transposition.
    pub pitch_key_center: u8,
    pub lo_vel: u8,
    pub hi_vel: u8,
    /// Linear gain, from the `volume` opcode in dB.
    pub gain: f32,
    /// Fine tuning in cents, from the `tune` opcode — and from the sampler's
    /// own pitch detection, which is how a sample recorded 30 cents flat is
    /// made to play in tune without touching the file.
    pub tune_cents: f32,
    /// The slice of the file this region plays, as fractions of it. `0.0..1.0`
    /// is the whole sample; anything narrower is a slice — see
    /// [`crate::instruments::sampler::Mode::Slice`].
    pub start: f32,
    pub end: f32,
    /// Which articulation this region belongs to, as an index into the
    /// instrument's list. `0` is the one an instrument that never heard of
    /// articulations plays — which is every SFZ file and every kit.
    pub articulation: usize,
}

impl SfzRegion {
    fn matches(&self, note: u8, vel: u8) -> bool {
        (self.lo_key..=self.hi_key).contains(&note) && (self.lo_vel..=self.hi_vel).contains(&vel)
    }

    /// Playback rate that transposes the sample to `note`, tuning included.
    fn rate_for_note(&self, note: u8) -> f32 {
        let semitones = note as f32 - self.pitch_key_center as f32;
        2.0_f32.powf(semitones / 12.0 + self.tune_cents / 1200.0)
    }
}

/// Opcode values that a `<group>` hands down to the `<region>`s under it.
#[derive(Clone, Copy)]
struct Defaults {
    lo_key: u8,
    hi_key: u8,
    pkc: u8,
    lo_vel: u8,
    hi_vel: u8,
    gain: f32,
    tune: f32,
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            lo_key: 0,
            hi_key: 127,
            pkc: 60,
            lo_vel: 0,
            hi_vel: 127,
            gain: 1.0,
            tune: 0.0,
        }
    }
}

/// Parse an `.sfz` file. Sample paths come back absolute.
pub fn parse_file(path: &Path) -> Result<Vec<SfzRegion>> {
    let base = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let text =
        std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let regions = parse_text(&text, &base);
    if regions.is_empty() {
        bail!("{} has no regions", path.display());
    }
    Ok(regions)
}

/// The parser proper. Unknown opcodes are ignored: SFZ has hundreds and this
/// handles the handful that decide which sample plays.
pub fn parse_text(text: &str, base: &Path) -> Vec<SfzRegion> {
    let mut regions = Vec::new();
    let mut group = Defaults::default();
    let mut cur = Defaults::default();
    let mut sample: Option<PathBuf> = None;
    let mut in_region = false;

    let flush = |sample: &mut Option<PathBuf>, d: &Defaults, out: &mut Vec<SfzRegion>| {
        if let Some(s) = sample.take() {
            out.push(SfzRegion {
                sample: s,
                lo_key: d.lo_key,
                hi_key: d.hi_key,
                pitch_key_center: d.pkc,
                lo_vel: d.lo_vel,
                hi_vel: d.hi_vel,
                gain: d.gain,
                tune_cents: d.tune,
                start: 0.0,
                end: 1.0,
                articulation: 0,
            });
        }
    };

    for raw in text.lines() {
        // `//` starts a comment; SFZ has no block comments.
        let line = match raw.find("//") {
            Some(i) => &raw[..i],
            None => raw,
        };
        // Headers and opcodes can share a line, so walk the tokens in order.
        let mut tokens = line.split_whitespace().peekable();
        while let Some(token) = tokens.next() {
            match token {
                "<group>" => {
                    flush(&mut sample, &cur, &mut regions);
                    in_region = false;
                    group = Defaults::default();
                    cur = group;
                }
                "<region>" => {
                    flush(&mut sample, &cur, &mut regions);
                    in_region = true;
                    cur = group;
                }
                // Sections choz doesn't model (control, curve, effect…) — their
                // opcodes are harmless where they land.
                t if t.starts_with('<') => {}
                t => {
                    let Some((key, val)) = t.split_once('=') else {
                        continue;
                    };
                    let target = if in_region { &mut cur } else { &mut group };
                    match key.to_ascii_lowercase().as_str() {
                        "sample" => {
                            // Sample paths may contain spaces ("Saw Samples/…"),
                            // and the value runs to the end of the line or to
                            // the next opcode. Whitespace collapses to one
                            // space, which is what every other host does too.
                            let mut val = val.to_string();
                            while tokens.peek().is_some_and(|t| !is_opcode(t)) {
                                val.push(' ');
                                val.push_str(tokens.next().unwrap_or_default());
                            }
                            let p = PathBuf::from(val.replace('\\', "/"));
                            sample = Some(if p.is_absolute() { p } else { base.join(p) });
                        }
                        "lokey" => target.lo_key = note_value(val).unwrap_or(0),
                        "hikey" => target.hi_key = note_value(val).unwrap_or(127),
                        "key" => {
                            let n = note_value(val).unwrap_or(60);
                            target.lo_key = n;
                            target.hi_key = n;
                            target.pkc = n;
                        }
                        "pitch_keycenter" => target.pkc = note_value(val).unwrap_or(60),
                        // Cents, and SFZ allows a whole semitone either way.
                        "tune" | "pitch" => {
                            target.tune = val.parse::<f32>().unwrap_or(0.0).clamp(-1200.0, 1200.0)
                        }
                        "lovel" => target.lo_vel = val.parse().unwrap_or(0),
                        "hivel" => target.hi_vel = val.parse().unwrap_or(127),
                        "volume" => {
                            let db: f32 = val.parse().unwrap_or(0.0);
                            target.gain = 10.0_f32.powf(db / 20.0);
                        }
                        _ => {}
                    }
                    // A group's opcodes only reach regions opened after them.
                    if !in_region {
                        cur = group;
                    }
                }
            }
        }
    }
    flush(&mut sample, &cur, &mut regions);
    regions
}

/// Whether a token starts a new opcode (`lokey=36`) rather than continuing the
/// value of the previous one. Only the characters SFZ uses in opcode names
/// count, so a path fragment like `C-3.flac` is not mistaken for one.
fn is_opcode(token: &str) -> bool {
    match token.split_once('=') {
        Some((key, _)) => {
            !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        None => token.starts_with('<'),
    }
}

/// A MIDI note as a number (`60`) or a name (`C4`, `a#3`, `Db-1`).
fn note_value(s: &str) -> Option<u8> {
    if let Ok(n) = s.parse::<u8>() {
        return Some(n);
    }
    let s = s.to_ascii_lowercase();
    let mut chars = s.chars().peekable();
    let semitone = match chars.next()? {
        'c' => 0,
        'd' => 2,
        'e' => 4,
        'f' => 5,
        'g' => 7,
        'a' => 9,
        'b' => 11,
        _ => return None,
    };
    let mut accidental: i32 = 0;
    match chars.peek() {
        Some('#') => {
            accidental = 1;
            chars.next();
        }
        Some('b') => {
            accidental = -1;
            chars.next();
        }
        _ => {}
    }
    let octave: i32 = chars.collect::<String>().parse().ok()?;
    let midi = (octave + 1) * 12 + semitone + accidental;
    u8::try_from(midi).ok().filter(|n| *n <= 127)
}

// ─── Sample decoding ────────────────────────────────────────────────────────

/// One sample's audio, from wherever it can be had most cheaply.
///
/// A cache already written is mapped and nothing is decoded. Otherwise the file
/// is decoded, and if it is long enough to be worth it
/// ([`stream::THRESHOLD_FRAMES`]) the result goes out to the raw cache and the
/// decoded buffer is dropped — so the peak memory of building an instrument is
/// one sample, not the library. Short ones stay in memory exactly as they were.
fn load_pcm(
    sample: &Path,
    bytes: Option<Vec<u8>>,
    sample_rate: u32,
) -> Option<crate::instruments::stream::Pcm> {
    use crate::instruments::stream;
    if let Some(mapped) = stream::mapped(sample, sample_rate) {
        return Some(mapped);
    }
    let pcm = match decode_from(sample, bytes, sample_rate) {
        Ok(pcm) if !pcm.is_empty() => pcm,
        Ok(_) => return None,
        Err(e) => {
            eprintln!("choz: SFZ {}: {e}", sample.display());
            return None;
        }
    };
    if pcm.len() / 2 < stream::THRESHOLD_FRAMES {
        return Some(stream::Pcm::mem(pcm));
    }
    match stream::cache_and_map(sample, sample_rate, pcm) {
        Ok(mapped) => Some(mapped),
        // A cache that cannot be written is not a reason not to play: decode
        // again and hold it, which is what every load did before this existed.
        Err(e) => {
            eprintln!("choz: sample cache {}: {e:#}", sample.display());
            decode(sample, sample_rate).ok().map(stream::Pcm::mem)
        }
    }
}

/// Every archived sample of an instrument, read archive by archive.
///
/// Keyed by the entry name, which is unique inside one instrument: a folder
/// inside a zip is one instrument, and two archives never share a build.
fn archive_bytes(regions: &[SfzRegion]) -> HashMap<String, Vec<u8>> {
    let mut wanted: HashMap<PathBuf, Vec<String>> = HashMap::new();
    for region in regions {
        if let Some((archive, entry)) = crate::instruments::sampler::archive::split(&region.sample)
        {
            let list = wanted.entry(archive).or_default();
            if !list.contains(&entry) {
                list.push(entry);
            }
        }
    }
    let mut out = HashMap::new();
    for (archive, entries) in wanted {
        out.extend(crate::instruments::sampler::archive::read_many(
            &archive, &entries,
        ));
    }
    out
}

/// The shortest a loop is allowed to be, in frames: about a millisecond and a
/// half at 48 kHz.
///
/// Two marks dragged on top of each other is a loop a few samples long, which
/// is not a loop — it is an oscillator at whatever frequency those samples are,
/// running at the level of the sample and never ending. The knee below keeps it
/// inside full scale; this keeps it from happening.
const MIN_LOOP_FRAMES: f64 = 64.0;

/// Where the soft knee starts. Under this nothing is touched at all, and the
/// curve above it is asymptotic to full scale — so the sampler's own output
/// cannot leave `-1..1` however many voices are in it.
///
/// High on purpose: a single voice of a sample normalised to full scale comes
/// out 0.2 dB down, which is the whole price. What it is for is the sum of a
/// chord, which is where the numbers get interesting.
const KNEE: f32 = 0.9;

/// A soft knee over [`KNEE`], so a chord that went past full scale bends
/// instead of squaring off.
///
/// ponytail: a plain `tanh` above the knee, not oversampled. What it is for is
/// the peak of a held chord, which is a slow thing; a waveshaper for *tone*
/// would need the oversampling the utility effect does — see `fx::utility`.
#[inline]
fn soft_knee(x: f32) -> f32 {
    let over = x.abs() - KNEE;
    if over <= 0.0 {
        return x;
    }
    let shaped = KNEE + (1.0 - KNEE) * (over / (1.0 - KNEE)).tanh();
    shaped * x.signum()
}

/// Decode a sample to interleaved stereo at `target_sr`.
///
/// ponytail: linear interpolation for the rate conversion, both here and in the
/// voice. A sampler that needs better than that needs a real resampler, and
/// that is a different piece of work.
pub(crate) fn decode(path: &Path, target_sr: u32) -> Result<Vec<f32>> {
    decode_from(path, None, target_sr)
}

/// The same, with the bytes already in hand — which is how an instrument built
/// out of one zip decodes a hundred regions without opening the archive a
/// hundred times. `None` reads the file (or the entry) itself.
pub(crate) fn decode_from(
    path: &Path,
    bytes: Option<Vec<u8>>,
    target_sr: u32,
) -> Result<Vec<f32>> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    // A sample can live inside a zip, and then there is no file to open: the
    // bytes are pulled out of the archive and decoded from memory. Everything
    // downstream — the probe, the hint, the decoder — is identical.
    let source: Box<dyn symphonia::core::io::MediaSource> = match bytes {
        Some(bytes) => Box::new(std::io::Cursor::new(bytes)),
        None => match crate::instruments::sampler::archive::split(path) {
            Some((archive, entry)) => Box::new(std::io::Cursor::new(
                crate::instruments::sampler::archive::read(&archive, &entry)?,
            )),
            None => Box::new(
                std::fs::File::open(path)
                    .with_context(|| format!("cannot open sample {}", path.display()))?,
            ),
        },
    };
    let mss = MediaSourceStream::new(source, Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe().format(
        &hint,
        mss,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    )?;
    let mut format = probed.format;
    let track = format
        .default_track()
        .context("sample has no audio track")?;
    let track_id = track.id;
    let channels = track
        .codec_params
        .channels
        .map(|c| c.count())
        .unwrap_or(1)
        .max(1);
    let file_sr = track.codec_params.sample_rate.unwrap_or(target_sr);
    let mut decoder =
        symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())?;

    let mut raw: Vec<f32> = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(p) if p.track_id() == track_id => p,
            Ok(_) => continue,
            // End of stream, or a truncated file: keep what decoded.
            Err(_) => break,
        };
        if let Ok(decoded) = decoder.decode(&packet) {
            let spec = *decoded.spec();
            let mut buf: SampleBuffer<f32> = SampleBuffer::new(decoded.capacity() as u64, spec);
            buf.copy_interleaved_ref(decoded);
            raw.extend_from_slice(buf.samples());
        }
    }

    let mut stereo = Vec::with_capacity(raw.len() / channels * 2);
    for frame in raw.chunks_exact(channels) {
        let l = frame[0];
        let r = if channels > 1 { frame[1] } else { l };
        stereo.push(l);
        stereo.push(r);
    }
    if file_sr == target_sr || stereo.is_empty() {
        return Ok(stereo);
    }
    Ok(resample(&stereo, file_sr, target_sr))
}

fn resample(stereo: &[f32], from: u32, to: u32) -> Vec<f32> {
    let ratio = from as f64 / to as f64;
    let in_frames = stereo.len() / 2;
    let mut out = Vec::with_capacity((in_frames as f64 / ratio) as usize * 2 + 2);
    let mut pos = 0.0f64;
    while pos < in_frames as f64 - 1.0 {
        let i0 = pos as usize;
        let i1 = i0 + 1;
        let t = (pos - i0 as f64) as f32;
        out.push(stereo[i0 * 2] + t * (stereo[i1 * 2] - stereo[i0 * 2]));
        out.push(stereo[i0 * 2 + 1] + t * (stereo[i1 * 2 + 1] - stereo[i0 * 2 + 1]));
        pos += ratio;
    }
    out
}

// ─── Sampler ────────────────────────────────────────────────────────────────

/// A region with its sample already decoded.
struct Loaded {
    region: SfzRegion,
    pcm: crate::instruments::stream::Pcm,
}

struct Voice {
    note: u8,
    gain: f32,
    rate: f64,
    pcm: crate::instruments::stream::Pcm,
    /// Read head, in frames.
    pos: f64,
    /// Release envelope, 1.0 while the key is down.
    env: f32,
    /// True once the key came up: the voice fades instead of stopping dead.
    releasing: bool,
    /// Where the region ends for this voice, in frames.
    to: f64,
    /// Where a held note turns round, in frames — the LOOP START and LOOP END
    /// knobs read against this voice's region. Equal to `from`/`to` when
    /// nothing was asked for.
    loop_from: f64,
    loop_to: f64,
    /// Seconds since the note started, and since it was released. The
    /// envelope is a function of these two and of the knobs, so a knob moved
    /// mid-note is heard on the next block.
    age: f32,
    since_off: f32,
    /// The envelope at the moment the key came up: release falls from there,
    /// not from 1.0, or letting go during the attack jumps to full volume.
    off_env: f32,
}

impl Voice {
    /// The amplitude envelope at this instant.
    fn envelope(&self, k: &Knobs) -> f32 {
        if self.releasing {
            let r = k.release.max(RELEASE_SECONDS);
            return self.off_env * (1.0 - self.since_off / r).max(0.0);
        }
        if self.age < k.attack {
            return self.age / k.attack.max(1e-6);
        }
        let d = self.age - k.attack;
        if d < k.decay {
            return 1.0 - (1.0 - k.sustain) * (d / k.decay.max(1e-6));
        }
        k.sustain
    }
}

/// The sampler's own knobs, as the rack sends them: normalised 0..1, in the
/// order [`crate::instruments::sampler::params`] lists them.
///
/// One set for the whole instrument rather than one per region: this is the
/// player's hand on the sound in front of them, not a per-sample edit — the
/// files are never touched.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Knobs {
    /// Where in the sample (or in the slice) a note starts, 0..1.
    pub start: f32,
    /// …and where it stops, 0..1 of the same span. `1.0` is the end of the
    /// region, which is what a sample with nothing said about it plays to.
    /// At or below `start` it is ignored: a window of no length is not a
    /// window, it is silence nobody asked for.
    pub end: f32,
    pub looping: bool,
    /// Where a held note goes back to and where it turns round, as fractions of
    /// the region — **not** of the file, so a slice loops inside itself.
    /// `loop_end` at or below `loop_start` means "to the end of the region",
    /// which is what a loop with nothing said about it used to do.
    pub loop_start: f32,
    pub loop_end: f32,
    /// How long the turn-round is faded across, in seconds. Zero is the butt
    /// join that clicks on anything that does not close at zero.
    pub crossfade: f32,
    /// Amplitude envelope, seconds and a level.
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
}

impl Default for Knobs {
    fn default() -> Self {
        Self {
            start: 0.0,
            end: 1.0,
            looping: false,
            loop_start: 0.0,
            loop_end: 1.0,
            crossfade: 0.0,
            attack: 0.0,
            decay: 0.0,
            sustain: 1.0,
            release: RELEASE_SECONDS,
        }
    }
}

impl Knobs {
    /// The longest a knob can ask for, per parameter. The rack shows these as
    /// the range of each knob, so the two have to agree — see
    /// [`crate::instruments::sampler::params`].
    pub const MAX_ATTACK: f32 = 2.0;
    pub const MAX_DECAY: f32 = 4.0;
    pub const MAX_RELEASE: f32 = 4.0;
    /// The longest turn-round fade. Past a quarter of a second it is not a
    /// loop join any more, it is a second voice.
    pub const MAX_CROSSFADE: f32 = 0.25;

    /// Take one normalised value from the rack. Index is the parameter's.
    pub fn set(&mut self, index: usize, v: f32) {
        let v = v.clamp(0.0, 1.0);
        match index {
            0 => self.start = v,
            1 => self.looping = v >= 0.5,
            2 => self.attack = v * Self::MAX_ATTACK,
            3 => self.decay = v * Self::MAX_DECAY,
            4 => self.sustain = v,
            // Never zero: a note cut off at an arbitrary sample clicks.
            5 => self.release = (v * Self::MAX_RELEASE).max(RELEASE_SECONDS),
            // Appended after the six that were here first, so a project saved
            // before they existed still finds its envelope where it left it.
            6 => self.loop_start = v,
            7 => self.loop_end = v,
            8 => self.crossfade = v * Self::MAX_CROSSFADE,
            9 => self.end = v,
            _ => {}
        }
    }
}

/// How long a released note takes to fade out.
///
/// Not a feature — the absence of one is a **click**. Cutting a bowed or
/// sustained sample off at an arbitrary sample leaves a step in the waveform,
/// and a step is a broadband transient: every note-off popped. Fifteen
/// milliseconds is under a player's notice and well over the discontinuity's.
const RELEASE_SECONDS: f32 = 0.015;

/// An SFZ instrument in a rack slot: notes in, interleaved stereo out.
pub struct SfzSampler {
    regions: Vec<Loaded>,
    voices: Vec<Voice>,
    name: String,
    /// How many times each key has been struck, for round robins. A plain
    /// array because the alternative — a map keyed by note — would allocate on
    /// the audio thread the first time a new note was played.
    strikes: [u32; 128],
    knobs: Knobs,
    /// Where the polyphony headroom got to at the end of the last block — see
    /// [`AudioSource::render`]. `0.0` is "nothing rendered yet".
    headroom: f32,
    /// The ways this instrument can be played, when its pack ships more than
    /// one — `sustain`, `staccato`, `pizzicato`. Empty otherwise, which is
    /// every SFZ file and every kit.
    articulations: Vec<String>,
    /// Which of them is sounding.
    articulation: usize,
    /// The lowest key that selects an articulation rather than playing one.
    /// The switches run up from here, one per articulation, below everything
    /// the map plays — see `sampler::map::regions_with_slices`.
    switch_base: Option<u8>,
}

impl SfzSampler {
    /// Parse `path` and decode every sample it references to `sample_rate`.
    /// Regions whose sample is missing or undecodable are dropped with a
    /// message; the instrument still loads if anything is left.
    pub fn build(path: &Path, sample_rate: u32) -> Result<Self> {
        let parsed = parse_file(path)?;
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "SFZ".into());
        Self::from_regions(parsed, sample_rate, name)
            .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))
    }

    /// An instrument with nothing in it: answers notes with silence, and takes
    /// a folder later.
    ///
    /// The sampler is loaded into a tab *before* anybody has said where the
    /// samples are — picking it is what puts it there, pointing it at a folder
    /// is a separate thing done afterwards — so there has to be something to
    /// put in the tab in the meantime.
    pub fn empty(name: String) -> Self {
        Self {
            regions: Vec::new(),
            voices: Vec::with_capacity(MAX_VOICES),
            name,
            strikes: [0; 128],
            knobs: Knobs::default(),
            headroom: 0.0,
            articulations: Vec::new(),
            articulation: 0,
            switch_base: None,
        }
    }

    /// What the keyswitches select, in the order their keys run. Empty for an
    /// instrument with one articulation, which is nearly all of them.
    pub fn articulations(&self) -> &[String] {
        &self.articulations
    }

    /// Which one is playing, and the lowest key that selects one.
    pub fn articulation(&self) -> usize {
        self.articulation
    }

    pub fn switch_base(&self) -> Option<u8> {
        self.switch_base
    }

    /// Name the articulations and put their switches under `base`. Called by
    /// the folder sampler, which is the only thing that knows a pack has more
    /// than one way of playing a note.
    pub fn set_articulations(&mut self, names: Vec<String>, base: Option<u8>) {
        self.articulation = 0;
        self.switch_base = base.filter(|_| names.len() > 1);
        self.articulations = names;
    }

    /// Pick one by hand — what the panel's button does.
    pub fn set_articulation(&mut self, index: usize) {
        if index < self.articulations.len().max(1) {
            self.articulation = index;
        }
    }

    /// The same instrument from regions somebody else worked out — the smart
    /// sampler builds its map from a folder of samples rather than from a text
    /// file, and the player has no reason to know the difference.
    pub fn from_regions(parsed: Vec<SfzRegion>, sample_rate: u32, name: String) -> Result<Self> {
        // One decode per distinct file: regions of the same instrument share
        // samples far more often than not.
        let mut cache: HashMap<PathBuf, Option<crate::instruments::stream::Pcm>> = HashMap::new();
        let mut regions = Vec::new();
        // Everything that lives inside a zip, read in one pass per archive:
        // opening it per region is the central directory again a hundred times
        // for one instrument.
        let mut bytes = archive_bytes(&parsed);
        for region in parsed {
            let entry = crate::instruments::sampler::archive::split(&region.sample)
                .and_then(|(_, entry)| bytes.remove(&entry));
            let pcm = cache
                .entry(region.sample.clone())
                .or_insert_with(|| load_pcm(&region.sample, entry, sample_rate))
                .clone();
            if let Some(pcm) = pcm {
                regions.push(Loaded { region, pcm });
            }
        }
        if regions.is_empty() {
            bail!("none of its samples could be loaded");
        }
        Ok(Self {
            regions,
            voices: Vec::with_capacity(MAX_VOICES),
            name,
            strikes: [0; 128],
            knobs: Knobs::default(),
            headroom: 0.0,
            articulations: Vec::new(),
            articulation: 0,
            switch_base: None,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Where the knobs are, for whoever draws them.
    pub fn knobs(&self) -> Knobs {
        self.knobs
    }
}

impl AudioSource for SfzSampler {
    fn render(&mut self, out: &mut [f32], sample_rate: u32) -> usize {
        let frames = out.len() / 2;
        out.fill(0.0);
        let dt = 1.0 / sample_rate as f32;
        let knobs = self.knobs;
        // How loud the sum of what is sounding is allowed to be.
        //
        // A chord of long notes is the case: six voices of the same sample add
        // up to six times one note, and the tab's fader was set by a probe that
        // played **one**. Sixteen voices of a held pad went past full scale
        // before the fader even saw them.
        //
        // `1/sqrt(n)` is the equal-power answer: one note is untouched, a chord
        // grows as the square root of what is in it — louder, which is what a
        // chord is, and not six times louder. Smoothed over the block so a note
        // starting is not a step in the level of everything already sounding.
        let sounding = self.voices.iter().filter(|v| v.pos.is_finite()).count();
        let target = 1.0 / (sounding.max(1) as f32).sqrt();
        let from = match self.headroom {
            // First block: start where the level already is rather than sliding
            // up to it from nothing.
            h if h <= 0.0 => target,
            h => h,
        };
        self.headroom = target;
        for voice in &mut self.voices {
            // Where this voice turns round, and how long the fade across the
            // join is. A loop shorter than the fade is a loop the fade would
            // read past the start of, so it is the fade that gives way.
            let looping = knobs.looping && !voice.releasing;
            let end = match looping {
                true => voice.loop_to,
                false => voice.to,
            };
            let span = (voice.loop_to - voice.loop_from).max(1.0);
            let fade = match looping {
                true => ((knobs.crossfade.max(0.0) as f64) * sample_rate as f64).min(span * 0.5),
                false => 0.0,
            };
            for f in 0..frames {
                let i0 = voice.pos as usize;
                // Past its end — the whole file, or the slice it was given.
                if voice.pos >= end || i0 + 1 >= voice.pcm.frames() {
                    // A held loop goes back to where it was told to; anything
                    // else is finished.
                    if looping {
                        voice.pos = voice.loop_from;
                    } else {
                        voice.pos = f64::INFINITY;
                        break;
                    }
                }
                let i0 = voice.pos as usize;
                voice.env = voice.envelope(&knobs);
                // Gone: stop mixing it rather than adding zeros for the rest
                // of the sample.
                if voice.releasing && voice.env <= 0.0 {
                    voice.pos = f64::INFINITY;
                    break;
                }
                let t = (voice.pos - i0 as f64) as f32;
                let (l0, r0) = voice.pcm.frame(i0);
                let (l1, r1) = voice.pcm.frame(i0 + 1);
                let mut l = l0 + t * (l1 - l0);
                let mut r = r0 + t * (r1 - r0);
                // The turn-round, faded: the last `fade` frames before the loop
                // end are mixed with the same distance before the loop start,
                // which is the join a sustained note needs not to click.
                let left = end - voice.pos;
                if fade > 0.0 && left < fade {
                    let ahead = voice.loop_from - left;
                    let j0 = ahead.max(0.0) as usize;
                    if ahead >= 0.0 && j0 + 1 < voice.pcm.frames() {
                        let u = (ahead - j0 as f64) as f32;
                        let (al0, ar0) = voice.pcm.frame(j0);
                        let (al1, ar1) = voice.pcm.frame(j0 + 1);
                        let al = al0 + u * (al1 - al0);
                        let ar = ar0 + u * (ar1 - ar0);
                        // Equal power, so the join does not dip in the middle.
                        let x = ((left / fade) as f32).clamp(0.0, 1.0);
                        let (a, b) = ((x * std::f32::consts::FRAC_PI_2).sin(), (x * std::f32::consts::FRAC_PI_2).cos());
                        l = l * a + al * b;
                        r = r * a + ar * b;
                    }
                }
                let gain = voice.gain * voice.env;
                out[f * 2] += l * gain;
                out[f * 2 + 1] += r * gain;
                voice.pos += voice.rate;
                voice.age += dt;
                if voice.releasing {
                    voice.since_off += dt;
                }
            }
        }
        // Finished voices go here, not in the loop above: retain never
        // allocates, so this stays RT-safe.
        self.voices.retain(|v| v.pos.is_finite());
        // The headroom, slid across the block, and a soft knee over it.
        //
        // The knee is the last guard and nothing else: under `KNEE` it is
        // exactly the signal it was given, and above it the curve bends rather
        // than the sample squaring off. A key held down under a wide-open
        // sustain, sixteen voices deep, cannot hard-clip the tab.
        for f in 0..frames {
            let t = match frames {
                0 | 1 => 1.0,
                n => f as f32 / (n - 1) as f32,
            };
            let g = from + (self.headroom - from) * t;
            for ch in 0..2 {
                let x = out[f * 2 + ch] * g;
                out[f * 2 + ch] = soft_knee(x);
            }
        }
        frames
    }

    /// The rack's knobs: see [`Knobs`]. Live, on the audio thread — moving one
    /// is heard on the notes already sounding, which is what makes the
    /// envelope worth having.
    fn set_param(&mut self, index: usize, value: f32) {
        self.knobs.set(index, value);
    }

    fn note_on(&mut self, note: u8, velocity: u8) {
        if velocity == 0 {
            self.note_off(note);
            return;
        }
        // A keyswitch selects and does not sound: the keys under the map are
        // how a player says "now staccato" without letting go of anything.
        if let Some(base) = self.switch_base {
            let top = base as usize + self.articulations.len();
            if (base as usize..top).contains(&(note as usize)) {
                self.articulation = note as usize - base as usize;
                return;
            }
        }
        // Every region that answers this note and velocity, and then the one
        // whose turn it is. Two takes of a note are alternatives — playing the
        // first one every time is what makes a sampled instrument sound like a
        // machine gun on a repeated note.
        let playing = self.articulation;
        let matching = self
            .regions
            .iter()
            .filter(|r| r.region.articulation == playing && r.region.matches(note, velocity))
            .count();
        if matching == 0 {
            return;
        }
        let turn = self.strikes[note as usize] as usize % matching;
        self.strikes[note as usize] = self.strikes[note as usize].wrapping_add(1);
        let Some(hit) = self
            .regions
            .iter()
            .filter(|r| r.region.articulation == playing && r.region.matches(note, velocity))
            .nth(turn)
        else {
            return;
        };
        if self.voices.len() == MAX_VOICES {
            // Steal the oldest rather than grow: `push` past the capacity would
            // allocate on the audio thread.
            self.voices.remove(0);
        }
        // The region says which part of the file it is (the whole of it,
        // unless the folder was sliced) and the START knob says where inside
        // that part the note begins.
        let last = hit.pcm.frames().saturating_sub(1) as f64;
        let from = (hit.region.start.clamp(0.0, 1.0) as f64 * last).min(last);
        let to = (hit.region.end.clamp(0.0, 1.0) as f64 * last).max(from);
        // START and END are the window inside the region the key plays: the
        // two marks the panel draws over the waveform. END at or below START is
        // nothing said, and the note runs to the end of the region.
        let span = to - from;
        let pos = from + self.knobs.start.clamp(0.0, 1.0) as f64 * span;
        let to = match self.knobs.end > self.knobs.start {
            true => from + self.knobs.end.clamp(0.0, 1.0) as f64 * span,
            false => to,
        };
        // The loop points, read against the region and not against the file: a
        // slice loops inside itself. Nothing said — the end knob at or below
        // the start — means "to the end of the region", which is what a loop
        // did before it had points of its own.
        let k = &self.knobs;
        let loop_from = from + k.loop_start.clamp(0.0, 1.0) as f64 * (to - from);
        let loop_to = match k.loop_end > k.loop_start {
            true => from + k.loop_end.clamp(0.0, 1.0) as f64 * (to - from),
            false => to,
        };
        let mut voice = Voice {
            note,
            gain: hit.region.gain * (velocity as f32 / 127.0),
            rate: hit.region.rate_for_note(note) as f64,
            pcm: hit.pcm.clone(),
            pos,
            env: 0.0,
            releasing: false,
            to,
            loop_from,
            loop_to: loop_to.max(loop_from + MIN_LOOP_FRAMES),
            age: 0.0,
            since_off: 0.0,
            off_env: 0.0,
        };
        // Where the envelope starts, so a note released inside the same block
        // it began fades from a real level rather than from silence.
        voice.env = voice.envelope(&self.knobs);
        self.voices.push(voice);
    }

    /// The key came up: fade, do not cut. See [`RELEASE_SECONDS`].
    fn note_off(&mut self, note: u8) {
        for voice in self
            .voices
            .iter_mut()
            .filter(|v| !v.releasing && v.note == note)
        {
            voice.releasing = true;
            voice.since_off = 0.0;
            // From wherever the envelope had got to: letting go during the
            // attack fades from there, it does not jump to full volume first.
            voice.off_env = voice.env;
        }
    }

    /// The sampler owns its voices, so panic just drops them. `clear` keeps the
    /// Vec's buffer, which is what keeps this RT-safe.
    fn all_notes_off(&mut self) {
        self.voices.clear();
    }

    fn plays_on_transport_stop(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_names_and_numbers_both_parse() {
        assert_eq!(note_value("C4"), Some(60));
        assert_eq!(note_value("c4"), Some(60));
        assert_eq!(note_value("A#3"), Some(58));
        assert_eq!(note_value("Db4"), Some(61));
        assert_eq!(note_value("60"), Some(60));
        assert_eq!(note_value("0"), Some(0));
        assert_eq!(note_value("127"), Some(127));
        assert_eq!(note_value("C-1"), Some(0));
        assert_eq!(note_value("nonsense"), None);
    }

    #[test]
    fn regions_take_their_key_range_and_group_defaults() {
        let sfz = "// a comment\n\
                   <group> lovel=64 hivel=127 volume=-6\n\
                   <region> sample=hard.wav lokey=36 hikey=47 pitch_keycenter=36\n\
                   <region> sample=snare.wav key=38";
        let regions = parse_text(sfz, Path::new("/kit"));
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[0].sample, Path::new("/kit/hard.wav"));
        assert_eq!((regions[0].lo_key, regions[0].hi_key), (36, 47));
        assert_eq!(regions[0].pitch_key_center, 36);
        assert_eq!(regions[0].lo_vel, 64, "group defaults reach the region");
        assert!(
            (regions[0].gain - 0.5011872).abs() < 1e-4,
            "-6 dB: {}",
            regions[0].gain
        );
        // `key` sets range and root pitch at once.
        assert_eq!(
            (
                regions[1].lo_key,
                regions[1].hi_key,
                regions[1].pitch_key_center
            ),
            (38, 38, 38)
        );
    }

    /// Sample paths with spaces are the norm in commercial libraries, and the
    /// value runs until the next opcode.
    #[test]
    fn a_sample_path_may_contain_spaces() {
        let sfz = "<region> sample=Saw Samples/Saw_C-3.flac lokey=36 hikey=47";
        let regions = parse_text(sfz, Path::new("/lib"));
        assert_eq!(regions.len(), 1);
        assert_eq!(
            regions[0].sample,
            Path::new("/lib/Saw Samples/Saw_C-3.flac")
        );
        assert_eq!((regions[0].lo_key, regions[0].hi_key), (36, 47));
    }

    #[test]
    fn a_region_only_answers_inside_its_ranges() {
        let r = SfzRegion {
            sample: PathBuf::new(),
            lo_key: 36,
            hi_key: 47,
            pitch_key_center: 36,
            lo_vel: 64,
            hi_vel: 127,
            gain: 1.0,
            tune_cents: 0.0,
            start: 0.0,
            end: 1.0,
            articulation: 0,
        };
        assert!(r.matches(40, 100));
        assert!(!r.matches(48, 100), "above the key range");
        assert!(!r.matches(40, 20), "below the velocity range");
        // An octave up plays at double speed.
        assert!((r.rate_for_note(48) - 2.0).abs() < 1e-5);
    }

    /// The voice mixer, on a sample built by hand — no files involved.
    #[test]
    fn a_note_plays_its_sample_and_stops_at_the_end() {
        let pcm = crate::instruments::stream::Pcm::mem(vec![0.5f32; 8]); // 4 stereo frames
        let mut s = SfzSampler {
            regions: vec![Loaded {
                region: SfzRegion {
                    sample: PathBuf::new(),
                    lo_key: 0,
                    hi_key: 127,
                    pitch_key_center: 60,
                    lo_vel: 0,
                    hi_vel: 127,
                    gain: 1.0,
                    tune_cents: 0.0,
                    start: 0.0,
                    end: 1.0,
                    articulation: 0,
                },
                pcm,
            }],
            voices: Vec::with_capacity(MAX_VOICES),
            name: "test".into(),
            strikes: [0; 128],
            knobs: Knobs::default(),
            headroom: 0.0,
            articulations: Vec::new(),
            articulation: 0,
            switch_base: None,
        };

        let mut buf = vec![0.0f32; 4];
        s.render(&mut buf, 48_000);
        assert!(buf.iter().all(|v| *v == 0.0), "silent until a note arrives");

        s.note_on(60, 127);
        s.render(&mut buf, 48_000);
        assert!(buf[0] > 0.4, "the sample is heard: {buf:?}");

        // Four frames of sample, two blocks of two: the voice is done.
        s.render(&mut buf, 48_000);
        assert!(s.voices.is_empty(), "a finished voice is dropped");

        s.note_on(60, 127);
        assert_eq!(s.voices.len(), 1);
        s.note_off(60);
        assert_eq!(s.voices.len(), 1, "note-off fades rather than cutting");
        // At 48 kHz the fade is 720 frames, so what ends this voice is the end
        // of its four-frame sample: two more blocks of two.
        s.render(&mut buf, 48_000);
        s.render(&mut buf, 48_000);
        assert!(s.voices.is_empty(), "and the faded voice is dropped");
    }

    /// A tuned region plays faster or slower by exactly the cents asked for,
    /// which is how a sample recorded flat is made to play in tune.
    #[test]
    fn tuning_moves_the_playback_rate() {
        let r = SfzRegion {
            sample: PathBuf::new(),
            lo_key: 0,
            hi_key: 127,
            pitch_key_center: 60,
            lo_vel: 0,
            hi_vel: 127,
            gain: 1.0,
            tune_cents: 100.0,
            start: 0.0,
            end: 1.0,
            articulation: 0,
        };
        // A semitone of tuning is the same as playing a semitone up.
        assert!((r.rate_for_note(60) - 2f32.powf(1.0 / 12.0)).abs() < 1e-5);
    }

    /// Two regions answering the same note are takes of it, and a repeated
    /// note has to alternate between them — the sampled-instrument machine gun
    /// is what happens when it does not.
    #[test]
    fn a_repeated_note_walks_its_round_robins() {
        let region = |gain: f32| Loaded {
            region: SfzRegion {
                sample: PathBuf::new(),
                lo_key: 0,
                hi_key: 127,
                pitch_key_center: 60,
                lo_vel: 0,
                hi_vel: 127,
                gain,
                tune_cents: 0.0,
                start: 0.0,
                end: 1.0,
                articulation: 0,
            },
            // Loud enough to tell apart, long enough to survive a block.
            pcm: crate::instruments::stream::Pcm::mem(vec![1.0f32; 64]),
        };
        let mut s = SfzSampler {
            // The two takes differ only in gain, which is what the test reads.
            regions: vec![region(0.25), region(1.0)],
            voices: Vec::with_capacity(MAX_VOICES),
            name: "test".into(),
            strikes: [0; 128],
            knobs: Knobs::default(),
            headroom: 0.0,
            articulations: Vec::new(),
            articulation: 0,
            switch_base: None,
        };
        let strike = |s: &mut SfzSampler| {
            s.all_notes_off();
            s.note_on(60, 127);
            let mut buf = vec![0.0f32; 4];
            s.render(&mut buf, 48_000);
            buf[0]
        };
        let first = strike(&mut s);
        let second = strike(&mut s);
        let third = strike(&mut s);
        assert!(first < second, "the second strike took the same sample");
        assert!(
            (third - first).abs() < 1e-6,
            "it never came back round: {first} then {third}"
        );
    }

    /// A sampler over one flat sample, at a sample rate that makes the
    /// arithmetic readable: 1000 frames a second, so a knob asking for 0.1 s
    /// is asking for 100 frames.
    fn bench(pcm: Vec<f32>) -> SfzSampler {
        SfzSampler {
            regions: vec![Loaded {
                region: SfzRegion {
                    sample: PathBuf::new(),
                    lo_key: 0,
                    hi_key: 127,
                    pitch_key_center: 60,
                    lo_vel: 0,
                    hi_vel: 127,
                    gain: 1.0,
                    tune_cents: 0.0,
                    start: 0.0,
                    end: 1.0,
                    articulation: 0,
                },
                pcm: crate::instruments::stream::Pcm::mem(pcm),
            }],
            voices: Vec::with_capacity(MAX_VOICES),
            name: "test".into(),
            strikes: [0; 128],
            knobs: Knobs::default(),
            headroom: 0.0,
            articulations: Vec::new(),
            articulation: 0,
            switch_base: None,
        }
    }

    /// The envelope knobs shape the note: an attack ramps it in from silence,
    /// and a release fades it out over the time it was given rather than the
    /// fifteen milliseconds that only exist to stop a click.
    #[test]
    fn the_envelope_knobs_shape_the_note() {
        let mut s = bench(vec![1.0; 2000]);
        // ATTACK is knob 2, of two seconds: 0.05 of it is 0.1 s = 100 frames.
        s.set_param(2, 0.05);
        s.note_on(60, 127);
        let mut buf = vec![0.0f32; 200];
        s.render(&mut buf, 1000);
        assert!(
            buf[0].abs() < 0.05,
            "it did not start from silence: {}",
            buf[0]
        );
        assert!(buf[80] > buf[20], "the attack did not ramp up");
        assert!((buf[198] - 1.0).abs() < 0.05, "it never reached the top");

        // RELEASE is knob 5, of four seconds: 0.05 is 0.2 s = 200 frames, so
        // the voice is still sounding a hundred frames after the key came up.
        s.set_param(5, 0.05);
        s.note_off(60);
        let mut buf = vec![0.0f32; 200];
        s.render(&mut buf, 1000);
        assert!(buf[0] > 0.5, "the release cut the note off: {}", buf[0]);
        assert!(buf[100] < buf[0], "the release did not fade");
        assert!(!s.voices.is_empty(), "the voice was dropped mid-release");
        // Two hundred frames of release, a hundred frames to a block.
        s.render(&mut buf, 1000);
        s.render(&mut buf, 1000);
        assert!(s.voices.is_empty(), "the release never ended");
    }

    /// LOOP keeps a held note going round the sample; without it the same note
    /// is over as soon as the file is.
    #[test]
    fn loop_keeps_a_held_note_going_past_the_end() {
        let mut s = bench(vec![1.0; 20]); // 10 frames
        s.note_on(60, 127);
        let mut buf = vec![0.0f32; 200];
        s.render(&mut buf, 1000);
        assert!(s.voices.is_empty(), "it played past its own end");

        s.set_param(1, 1.0);
        s.note_on(60, 127);
        s.render(&mut buf, 1000);
        assert_eq!(s.voices.len(), 1, "the loop ended anyway");
        assert!(buf[190] > 0.5, "it went silent instead of coming round");
        // And letting go still ends it, or a loop would be a note that never
        // stops.
        s.note_off(60);
        s.render(&mut buf, 1000);
        assert!(s.voices.is_empty(), "a released loop kept going");
    }

    /// START moves where a note begins inside the sample — the scrub. On a
    /// ramp, starting halfway through is heard as starting at half level.
    #[test]
    fn the_start_knob_moves_the_read_head() {
        let ramp: Vec<f32> = (0..1000).flat_map(|i| [i as f32 / 1000.0; 2]).collect();
        let mut s = bench(ramp);
        let mut buf = vec![0.0f32; 2];

        s.note_on(60, 127);
        s.render(&mut buf, 1000);
        assert!(buf[0] < 0.01, "it did not start at the start: {}", buf[0]);

        s.all_notes_off();
        s.set_param(0, 0.5);
        s.note_on(60, 127);
        s.render(&mut buf, 1000);
        assert!(
            (buf[0] - 0.5).abs() < 0.02,
            "the read head did not move: {}",
            buf[0]
        );
    }

    /// A sliced region plays its own piece of the file and stops at the end of
    /// it, not at the end of the sample.
    #[test]
    fn a_slice_plays_only_its_own_piece() {
        let mut s = bench(vec![1.0; 200]); // 100 frames
        s.regions[0].region.start = 0.5;
        s.regions[0].region.end = 0.75;
        s.note_on(60, 127);
        let mut buf = vec![0.0f32; 40]; // 20 frames
        s.render(&mut buf, 1000);
        assert_eq!(s.voices.len(), 1, "the quarter was over in twenty frames");
        s.render(&mut buf, 1000);
        assert!(s.voices.is_empty(), "the slice ran past its own end");
    }

    /// **A chord of long notes does not saturate.**
    ///
    /// Reported from a run whose log says it: the tab's fader is set by a probe
    /// that plays *one* note — `tab 1 trimmed to 2.00 — peaking 0.13` — and
    /// then six held notes summed to six times what the probe measured. With
    /// `LOOP` on there is no end to them either: they sustain until the key
    /// comes up.
    ///
    /// Two things answer it, and this checks both: the sum is scaled by
    /// `1/sqrt(voices)`, so a chord is louder than one note and nowhere near
    /// six times louder; and the knee above it means the instrument's own
    /// output cannot leave `-1..1` whatever is held down.
    #[test]
    fn a_chord_of_long_notes_does_not_saturate() {
        let peak = |s: &mut SfzSampler, frames: usize| -> f32 {
            let mut buf = vec![0.0f32; frames * 2];
            s.render(&mut buf, 1000);
            buf.iter().fold(0.0f32, |m, v| m.max(v.abs()))
        };

        // One voice of a sample that sits at full scale: what the auto-level
        // probe hears, and what it must keep hearing.
        let mut one = bench(vec![1.0; 8000]);
        one.set_param(1, 1.0); // LOOP on: long notes, no end to them
        one.note_on(60, 127);
        let alone = peak(&mut one, 400);
        assert!(
            (alone - 1.0).abs() < 0.05,
            "one note is not itself any more: {alone}"
        );

        // Six of them, held. Without the headroom this is 6.0 and the tab's
        // fader multiplies it.
        let mut chord = bench(vec![1.0; 8000]);
        chord.set_param(1, 1.0);
        for note in [48, 52, 55, 59, 62, 67] {
            chord.note_on(note, 127);
        }
        let together = peak(&mut chord, 400);
        assert!(
            together <= 1.0,
            "a six-note chord left full scale: {together}"
        );
        assert!(
            together > alone,
            "a chord has to be louder than one note: {together} vs {alone}"
        );
        // Held, not decaying: the same a second later, and still inside.
        let held = peak(&mut chord, 2_000);
        assert!(held <= 1.0, "it saturated once it settled: {held}");

        // …and the worst case the sampler allows: every voice it has.
        let mut full = bench(vec![1.0; 8000]);
        full.set_param(1, 1.0);
        for note in 40..40 + MAX_VOICES as u8 {
            full.note_on(note, 127);
        }
        let all = peak(&mut full, 400);
        assert!(all <= 1.0, "{} voices reached {all}", MAX_VOICES);
    }

    /// Two loop marks on top of each other is not a loop: it is an oscillator
    /// at whatever those few samples are, at the level of the sample and with
    /// no end. The window is opened to [`MIN_LOOP_FRAMES`] instead.
    #[test]
    fn a_loop_cannot_be_shorter_than_a_loop() {
        let mut s = bench(vec![1.0; 8000]);
        s.set_param(1, 1.0); // LOOP
        s.set_param(6, 0.5); // LOOP ST
        s.set_param(7, 0.5001); // …and LOOP END on top of it
        s.note_on(60, 100);
        let voice = s.voices.first().expect("no voice");
        assert!(
            voice.loop_to - voice.loop_from >= MIN_LOOP_FRAMES,
            "a {}-frame loop",
            voice.loop_to - voice.loop_from
        );
    }
}
