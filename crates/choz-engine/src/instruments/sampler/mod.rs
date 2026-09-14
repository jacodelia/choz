//! A folder of samples, as an instrument you can play.
//!
//! Point this at `~/Samples/Philharmonia/violin` and it comes back with a
//! [`crate::instruments::sfz::SfzSampler`] — the same player an `.sfz` file gets, built from
//! what the files themselves say rather than from a map somebody wrote by hand.
//!
//! ## The order it asks in
//!
//! Cheapest and most trustworthy first, because the ones that follow are
//! guesses and a guess must never overrule a statement:
//!
//! 1. **The filename** — `violin_C4_mf_RR2.wav` says the note, the dynamic and
//!    the take, and says them exactly. See [`name`].
//! 2. **The folders above it** — a pack that keeps its articulations in
//!    directories has said the same thing one level up.
//! 3. **The audio** — [`analyze`], and only for what is still unknown. It is a
//!    decode plus a pitch detection per file, which is why it is last.
//!
//! ## What it costs, and what the cache is for
//!
//! Philharmonia's violin is some 1500 files. Decoding them to find pitches
//! nobody needed would be minutes of work at every load — so filenames answer
//! nearly all of it in microseconds, and whatever the audio had to answer is
//! written to [`cache`] and not asked again until the file changes.
//!
//! ## What this is not, yet
//!
//! One articulation plays at a time (see [`map`]), samples are decoded into
//! RAM when the instrument loads rather than streamed, and nothing here writes
//! to the sample folder — the originals are read, never renamed, moved or
//! converted.

pub mod analyze;
pub mod archive;
pub mod cache;
pub mod map;
pub mod name;
pub mod preset;

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

/// What the sampler is called, wherever a person sees it.
pub const NAME: &str = "choz-sampler";

/// The instrument id of a sampler that has not been given a folder yet.
///
/// Not a path and never confused for one: an empty sampler is what a tab holds
/// between being told to use the sampler and being told which samples.
pub const EMPTY_ID: &str = NAME;

/// Extensions a sample can have. WAV and FLAC first because that is what
/// multisampled packs ship; the compressed ones are here because Philharmonia
/// distributes MP3 and refusing to read it would mean refusing the library
/// this was built for.
pub const EXTENSIONS: &[&str] = &["wav", "wave", "flac", "aiff", "aif", "mp3", "ogg"];

/// How a folder is laid out on the keyboard, when the user disagrees with what
/// the sampler worked out.
///
/// [`map::regions`] decides for itself — named notes and a spread of pitches
/// mean an instrument, a heap of unpitched hits means a kit — and it is right
/// about the libraries it was built against. It cannot be right about all of
/// them: a pack of unnamed one-note synth stabs *is* meant to be played across
/// the keyboard, and a set of tuned toms is not. This is the override.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    /// Work it out from the samples. What everything does unless told.
    #[default]
    Auto,
    /// Stretch: every sample covers the keys around it, transposed.
    Stretch,
    /// Kit: one sample per key, each at the pitch it was recorded at.
    Kit,
    /// Slice: one sample cut into equal pieces, one piece per key.
    ///
    /// A break, a phrase, a field recording: the file is not a note and not a
    /// kit, it is a thing to be cut up and replayed in another order. Nothing
    /// is written — the pieces are ranges into the same decoded sample.
    Slice,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Auto => "AUTO",
            Mode::Stretch => "STRETCH",
            Mode::Kit => "KIT",
            Mode::Slice => "SLICE",
        }
    }

    /// Every mode, in the order the picker offers them.
    pub const ALL: &'static [Mode] = &[Mode::Auto, Mode::Stretch, Mode::Kit, Mode::Slice];

    /// How the mode rides along with the instrument, since a folder loaded
    /// from a project has to come back the way it was left.
    ///
    /// A scanned instrument is identified by its path, and this hangs off the
    /// end of that identity: `~/Samples/toms#kit`. The path itself stays
    /// clean — it is passed separately — so nothing ever tries to open a
    /// directory by this name.
    pub fn suffix(self) -> &'static str {
        match self {
            Mode::Auto => "",
            Mode::Stretch => "#stretch",
            Mode::Kit => "#kit",
            Mode::Slice => "#slice",
        }
    }

    /// Read one back off an instrument id. Anything else is `Auto`, including
    /// a path that happens to contain a `#`.
    ///
    /// `#slice` may carry how many pieces it was cut into — `#slice24` — which
    /// is still the slice mode. See [`slices_of_id`].
    pub fn of_id(id: &str) -> Mode {
        if slice_tail(id).is_some() {
            return Mode::Slice;
        }
        Mode::ALL
            .iter()
            .copied()
            .find(|m| !m.suffix().is_empty() && id.ends_with(m.suffix()))
            .unwrap_or(Mode::Auto)
    }

    /// The id with the mode taken off: the path it names.
    pub fn strip(id: &str) -> &str {
        if let Some((path, _)) = slice_tail(id) {
            return path;
        }
        let mode = Mode::of_id(id);
        id.strip_suffix(mode.suffix()).unwrap_or(id)
    }
}

/// How the recording was played, which decides whether two samples of the same
/// note are alternatives or different instruments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Articulation {
    Sustain,
    Short,
    Staccato,
    Long,
    Legato,
    Pizzicato,
    Tremolo,
    Marcato,
    Hit,
    Release,
    Muted,
    Open,
    /// The name said nothing. Not a failure: most packs never say.
    Unknown,
}

impl Articulation {
    /// One word of a filename, if it names an articulation.
    ///
    /// `normal` and `arco` are here because that is what Philharmonia calls an
    /// ordinary bowed note, and a library whose commonest articulation parsed
    /// as `Unknown` would sort its own default out of the way.
    pub fn from_word(word: &str) -> Option<Articulation> {
        Some(match word {
            "sustain" | "sustained" | "normal" | "arco" | "ord" => Articulation::Sustain,
            // `short` and `long` are deliberately absent: in the libraries
            // this reads they are the note's *length*, not how it was played —
            // `violin_A3_15_forte_arco-normal` and `guitar_C4_very-long_normal`
            // — and length already tells the groups apart by name. Reading them
            // as articulations made `very-long_normal` come out as `Long`
            // rather than an ordinary note, and an ordinary note is exactly
            // what a folder should open on.
            "staccato" | "stac" | "stacc" | "spiccato" => Articulation::Staccato,
            "tenuto" => Articulation::Long,
            "legato" | "leg" => Articulation::Legato,
            "pizzicato" | "pizz" => Articulation::Pizzicato,
            "tremolo" | "trem" => Articulation::Tremolo,
            "marcato" | "marc" | "martele" => Articulation::Marcato,
            "hit" | "oneshot" => Articulation::Hit,
            "release" | "rel" => Articulation::Release,
            "muted" | "mute" => Articulation::Muted,
            "open" => Articulation::Open,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Articulation::Sustain => "sustain",
            Articulation::Short => "short",
            Articulation::Staccato => "staccato",
            Articulation::Long => "long",
            Articulation::Legato => "legato",
            Articulation::Pizzicato => "pizzicato",
            Articulation::Tremolo => "tremolo",
            Articulation::Marcato => "marcato",
            Articulation::Hit => "hit",
            Articulation::Release => "release",
            Articulation::Muted => "muted",
            Articulation::Open => "open",
            Articulation::Unknown => "unknown",
        }
    }
}

/// One file, and everything known about it. The file itself is never touched: this
/// is the whole of what the sampler knows, and it lives in the cache.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    pub path: PathBuf,
    /// The pitch it was recorded at, or `None` for a drum, a noise bed or
    /// anything the detector would not commit to.
    pub root: Option<u8>,
    /// How far off that note it actually sits, in cents.
    pub cents: f32,
    /// The velocity its name claimed; `None` when the pack never said.
    pub velocity: Option<u8>,
    pub articulation: Articulation,
    pub round_robin: Option<u32>,
    /// The name with the note, the dynamic and the take removed — see
    /// [`name`]. Samples that share it are one instrument.
    pub group: String,
    /// The file's size. Not trivia: when two groups cover the same notes, the
    /// one whose files are bigger is the one whose notes are longer, and a
    /// second and a half of violin is a better instrument than a quarter of a
    /// second of it.
    pub bytes: u64,
    /// 1.0 when the name said the note outright, YIN's clarity when it did not.
    pub confidence: f32,
    /// True when the note came off the filename rather than the audio.
    pub from_name: bool,
    pub peak: f32,
    pub rms: f32,
    pub frames: u64,
    pub sample_rate: u32,
}

/// Every sample under `dir`, with its size, in a stable order. `dir` may also
/// be a `.zip`, in which case its entries are what comes back — see
/// [`archive`].
///
/// The size travels with the path because the two places that want it cannot
/// go back for it: an entry inside an archive has no `stat`, and asking the
/// archive again per file would re-read its directory once per sample.
///
/// ponytail: three levels down and no symlink following. Packs nest by
/// articulation and dynamic, not by ten levels of anything, and a scan that
/// wandered into `~` because someone linked it there is a worse bug than a
/// pack that needs a fourth level.
pub fn files(dir: &Path) -> Vec<(PathBuf, u64)> {
    let mut out = match archive::split(dir) {
        // A folder inside an archive: `percussion.zip/bass drum`.
        Some((zip, prefix)) => archive::samples_under(&zip, Some(&prefix)),
        None if archive::is_archive(dir) => archive::samples(dir),
        None => {
            let mut found = Vec::new();
            collect(dir, 0, &mut found);
            found
        }
    };
    out.sort();
    out
}

fn collect(dir: &Path, depth: usize, out: &mut Vec<(PathBuf, u64)>) {
    if depth > 3 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_symlink() {
            continue;
        }
        // Archives inside are **not** followed: each one is an instrument in
        // its own right, and a shelf of them is a shelf, not one instrument
        // with twenty violins in it.
        if path.is_dir() {
            collect(&path, depth + 1, out);
        } else if is_sample(&path) {
            let bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);
            out.push((path, bytes));
        }
    }
}

fn is_sample(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// True when `path` is an instrument rather than a shelf of them: a folder
/// holding sample files of its own, or an archive holding them.
/// `~/Samples/Philharmonia` is a shelf; the `violin` folder inside it and the
/// `violin.zip` next to it are each an instrument.
pub fn is_instrument_dir(path: &Path) -> bool {
    if archive::is_archive(path) || archive::split(path).is_some() {
        return !files(path).is_empty();
    }
    let Ok(rd) = std::fs::read_dir(path) else {
        return false;
    };
    rd.flatten().any(|e| is_sample(&e.path()))
}

/// The instruments `path` offers a scan: itself when it is one, the folders
/// inside it when it is an archive holding several, and nothing when it is a
/// shelf on disk — a shelf gets walked into instead.
pub fn instruments(path: &Path) -> Vec<PathBuf> {
    if archive::is_archive(path) {
        return archive::instruments(path);
    }
    match is_instrument_dir(path) {
        true => vec![path.to_path_buf()],
        false => Vec::new(),
    }
}

/// Read a folder: names first, the audio only for what the names left open,
/// and the cache in front of both.
pub fn describe(dir: &Path) -> Vec<Sample> {
    let files = files(dir);
    let mut cached = cache::read(dir);
    let mut out = Vec::with_capacity(files.len());
    let mut analysed = 0usize;
    for (path, bytes) in files {
        if let Some(hit) = cached.remove(&path) {
            out.push(hit);
            continue;
        }
        analysed += 1;
        out.push(describe_one(dir, &path, bytes));
    }
    if analysed > 0 {
        cache::write(dir, &out);
    }
    out
}

/// One file. Public so a UI can re-read a single sample the user just fixed
/// the name of, without rescanning the folder. `bytes` is its size, which the
/// caller has already paid for — see [`files`].
pub fn describe_one(root_dir: &Path, path: &Path, bytes: u64) -> Sample {
    // The folders between the instrument and the file say what the filename
    // says, for packs that sort by articulation instead of naming it.
    let mut parsed = name::Parsed::default();
    for component in path
        .strip_prefix(root_dir)
        .unwrap_or(path)
        .parent()
        .into_iter()
        .flat_map(|p| p.components())
    {
        let from_dir = name::parse(&component.as_os_str().to_string_lossy());
        parsed.articulation = parsed.articulation.or(from_dir.articulation);
        parsed.velocity = parsed.velocity.or(from_dir.velocity);
    }
    // The filename overrules the folders: it is the more specific statement.
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let from_name = name::parse(&stem);
    let note = from_name.note;
    let velocity = from_name.velocity.or(parsed.velocity);
    let articulation = from_name
        .articulation
        .or(parsed.articulation)
        .unwrap_or(Articulation::Unknown);

    // The audio, only for what is left. A named note is never second-guessed:
    // the person who made the pack knew, and a detector that disagreed with
    // them would be wrong more often than they were.
    let analysis = match note {
        Some(_) => None,
        None => analyze::analyze(path)
            .map_err(|e| eprintln!("choz: sampler {}: {e}", path.display()))
            .ok(),
    };
    Sample {
        path: path.to_path_buf(),
        group: from_name.group,
        bytes,
        root: note.or_else(|| analysis.and_then(|a| a.root)),
        cents: analysis.map(|a| a.cents).unwrap_or(0.0),
        velocity,
        articulation,
        round_robin: from_name.round_robin,
        confidence: match note {
            Some(_) => 1.0,
            None => analysis.map(|a| a.confidence).unwrap_or(0.0),
        },
        from_name: note.is_some(),
        peak: analysis.map(|a| a.peak).unwrap_or(0.0),
        rms: analysis.map(|a| a.rms).unwrap_or(0.0),
        frames: analysis.map(|a| a.frames).unwrap_or(0),
        sample_rate: analysis.map(|a| a.sample_rate).unwrap_or(0),
    }
}

/// How many pieces [`Mode::Slice`] cuts a sample into unless told otherwise:
/// two octaves of keys, which is what a break wants and what fits under two
/// hands.
pub const SLICES: usize = 16;

/// The counts the button steps through. A break with twenty hits in it has
/// nowhere to put four of them at sixteen, and past thirty-two the keys run
/// out of the part of the keyboard a kit lives on.
pub const SLICE_CHOICES: &[usize] = &[8, 16, 24, 32];

/// `("path", pieces)` when an id ends in a slice suffix, `None` otherwise. The
/// count is optional: plain `#slice` is [`SLICES`], which is what every project
/// written before the count existed says.
fn slice_tail(id: &str) -> Option<(&str, usize)> {
    let (path, tail) = id.rsplit_once("#slice")?;
    match tail.is_empty() {
        true => Some((path, SLICES)),
        false => tail.parse::<usize>().ok().map(|n| (path, n.max(1))),
    }
}

/// The suffix that says an id names a saved map rather than a folder — see
/// [`preset`]. The path beside it is the `.smpreset` file itself.
pub const PRESET_SUFFIX: &str = "#preset";

/// Whether this id names a saved map.
pub fn is_preset_id(id: &str) -> bool {
    id.ends_with(PRESET_SUFFIX)
}

/// How many pieces the id asks for, whatever it is: a non-slice id is the
/// default, because nothing is going to ask it.
pub fn slices_of_id(id: &str) -> usize {
    slice_tail(id).map(|(_, n)| n).unwrap_or(SLICES)
}

/// The suffix for a slice layout of `n` pieces. The default count writes the
/// plain `#slice` an older choz already reads.
pub fn slice_suffix(n: usize) -> String {
    match n == SLICES {
        true => Mode::Slice.suffix().to_string(),
        false => format!("#slice{n}"),
    }
}

/// The sampler's own knobs, in the order the rack draws and addresses them.
///
/// The same six [`crate::instruments::sfz::Knobs`] takes by index — this is
/// the description of them, for the panel and for the project file. Ranges are
/// real values, not 0..1: the rack sends the normalised position and shows the
/// number these give it.
pub fn params() -> Vec<choz_ports::PluginParam> {
    use crate::instruments::sfz::Knobs;
    let p = |name: &str, min: f32, max: f32, default: f32, unit: &str, group: &str| {
        choz_ports::PluginParam {
            name: name.to_string(),
            min: min as f64,
            max: max as f64,
            default: default as f64,
            unit: (!unit.is_empty()).then(|| unit.to_string()),
            group: Some(group.to_string()),
            ..Default::default()
        }
    };
    let mut out = vec![
        // Where in the sample a note starts: the scrub.
        p("START", 0.0, 100.0, 0.0, "%", "SAMPLE"),
        choz_ports::PluginParam {
            steps: 2,
            ..p("LOOP", 0.0, 1.0, 0.0, "", "SAMPLE")
        },
        p("ATTACK", 0.0, Knobs::MAX_ATTACK, 0.0, "s", "ENV"),
        p("DECAY", 0.0, Knobs::MAX_DECAY, 0.0, "s", "ENV"),
        p("SUSTAIN", 0.0, 100.0, 100.0, "%", "ENV"),
        p("RELEASE", 0.0, Knobs::MAX_RELEASE, 0.0, "s", "ENV"),
        // Appended, and appended on purpose: a project saved before these
        // existed reads its six values into the six that were here first.
        p("LOOP ST", 0.0, 100.0, 0.0, "%", "SAMPLE"),
        p("LOOP END", 0.0, 100.0, 100.0, "%", "SAMPLE"),
        // Ten milliseconds by default rather than nothing: a held note over a
        // loop that was cut where the waveform is not at zero clicks once a
        // cycle, and a click is a spike in a chord that is already summing.
        // Somebody who wants the butt join turns it down.
        p(
            "XFADE",
            0.0,
            Knobs::MAX_CROSSFADE * 1000.0,
            10.0,
            "ms",
            "SAMPLE",
        ),
        // Where the key stops playing, against the same span START measures.
        // Last in the list for the same reason the three above it are: a
        // project written before it reads its own knobs where it left them.
        p("END", 0.0, 100.0, 100.0, "%", "SAMPLE"),
    ];
    for (i, param) in out.iter_mut().enumerate() {
        param.id = i as u32;
    }
    out
}

/// What the panel needs to draw one sample: its shape and where `SLICE` would
/// cut it.
///
/// Both come out of the same decode, because they were two of them and a
/// folder is adopted often enough to feel it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Shape {
    /// `buckets` peak levels, 0..1 — the waveform.
    pub peaks: Vec<f32>,
    /// Where the cuts fall, as fractions of the file. Empty when nothing in it
    /// sounded like a hit, which is when the map falls back to equal pieces.
    pub cuts: Vec<f32>,
}

/// The shape of one sample and its cuts, from one decode.
///
/// Called once when a folder is adopted and never while anything is drawing:
/// it reads the file.
pub fn shape(path: &Path, buckets: usize, slices: usize) -> Shape {
    let Ok(pcm) = crate::instruments::sfz::decode(path, 44_100) else {
        return Shape::default();
    };
    Shape {
        peaks: peaks_of(&pcm, buckets),
        cuts: analyze::onsets(&pcm, 44_100, slices),
    }
}

/// The shape of one sample, as `buckets` peak levels 0..1 — what the panel
/// draws as a waveform.
pub fn peaks(path: &Path, buckets: usize) -> Vec<f32> {
    let Ok(pcm) = crate::instruments::sfz::decode(path, 44_100) else {
        return Vec::new();
    };
    peaks_of(&pcm, buckets)
}

/// The peaks of already-decoded interleaved stereo.
fn peaks_of(pcm: &[f32], buckets: usize) -> Vec<f32> {
    let frames = pcm.len() / 2;
    if frames == 0 || buckets == 0 {
        return Vec::new();
    }
    let per = frames.div_ceil(buckets).max(1);
    (0..buckets)
        .map(|b| {
            let from = (b * per).min(frames);
            let to = ((b + 1) * per).min(frames);
            pcm[from * 2..to * 2]
                .iter()
                .fold(0.0f32, |m, s| m.max(s.abs()))
        })
        .collect()
}

/// Where `SLICE` cuts a break: the onsets of the file, as fractions, or an
/// empty list when nothing in it sounded like a hit.
pub fn slice_points(path: &Path, max: usize) -> Vec<f32> {
    let Ok(pcm) = crate::instruments::sfz::decode(path, 44_100) else {
        return Vec::new();
    };
    analyze::onsets(&pcm, 44_100, max)
}

/// What a folder can be played with, and the lowest key that selects one:
/// `(names, switch base)`.
///
/// The same answer [`build_with`] gives the instrument, for an interface that
/// wants to draw the list without asking the audio thread. Backed by the same
/// cache the scan is, so this is a read of what is already known.
pub fn articulations(dir: &Path) -> (Vec<String>, Option<u8>) {
    let samples = describe(dir);
    if samples.is_empty() {
        return (Vec::new(), None);
    }
    let names = map::articulation_groups(&samples);
    let regions = map::regions(&samples);
    let base = map::switch_base(&regions, names.len());
    match names.len() > 1 {
        true => (names, base),
        false => (Vec::new(), None),
    }
}

/// Which sample a key plays, and the part of it that key gets.
///
/// `(file, start, end)` — the fractions are `0.0..1.0` for anything but a
/// slice. What the panel draws when the shape it shows is meant to be the shape
/// of what is sounding rather than of whichever file the map happened to put
/// first.
pub fn sample_for(
    dir: &Path,
    mode: Mode,
    slices: usize,
    articulation: usize,
    note: u8,
    velocity: u8,
) -> Option<(PathBuf, f32, f32)> {
    let samples = describe(dir);
    let regions = map::regions_with_slices(&samples, mode, slices);
    let hit = regions
        .iter()
        .find(|r| {
            r.articulation == articulation
                && (r.lo_key..=r.hi_key).contains(&note)
                && (r.lo_vel..=r.hi_vel).contains(&velocity)
        })
        .or_else(|| regions.first())?;
    Some((hit.sample.clone(), hit.start, hit.end))
}

/// The file a folder's waveform is drawn from: the one the map would reach
/// for first, which is the one the player hears when they press a key.
pub fn representative(dir: &Path) -> Option<PathBuf> {
    let samples = describe(dir);
    let regions = map::regions(&samples);
    regions.first().map(|r| r.sample.clone())
}

/// The whole trip: a folder in, a playable instrument out, laid out the way
/// the samples say.
pub fn build(dir: &Path, sample_rate: u32) -> Result<crate::instruments::sfz::SfzSampler> {
    build_with(dir, sample_rate, Mode::Auto)
}

/// A sampler with no folder: silent, and ready to be given one.
pub fn empty() -> crate::instruments::sfz::SfzSampler {
    crate::instruments::sfz::SfzSampler::empty(NAME.to_string())
}

/// Build from a saved map rather than from a scan — see [`preset`].
///
/// This is the load that does not read a hundred and ten file names and does
/// not analyse anything: the answer was worked out once and written down.
pub fn build_preset(
    preset: &preset::Preset,
    sample_rate: u32,
) -> Result<crate::instruments::sfz::SfzSampler> {
    let regions = preset.regions();
    if regions.is_empty() {
        bail!("{}: a preset with no regions in it", preset.name);
    }
    let mut instrument = crate::instruments::sfz::SfzSampler::from_regions(
        regions,
        sample_rate,
        preset.name.clone(),
    )?;
    instrument.set_articulations(preset.articulations.clone(), preset.switch_base);
    Ok(instrument)
}

/// Everything a folder builds, as a preset ready to write.
pub fn preset_of(dir: &Path, mode: Mode, slices: usize) -> Result<preset::Preset> {
    let samples = describe(dir);
    if samples.is_empty() {
        bail!("{}: no samples in it", dir.display());
    }
    let regions = map::regions_with_slices(&samples, mode, slices);
    if regions.is_empty() {
        bail!("{}: nothing in it could be mapped to a key", dir.display());
    }
    let names = map::articulation_groups(&samples);
    let base = map::switch_base(&regions, names.len());
    let name = match archive::is_archive(dir) {
        true => dir.file_stem(),
        false => dir.file_name(),
    }
    .map(|s| s.to_string_lossy().into_owned())
    .unwrap_or_else(|| "samples".into());
    let names = match names.len() > 1 {
        true => names,
        false => Vec::new(),
    };
    Ok(preset::Preset::of(dir, &name, &regions, &names, base))
}

/// The same, with the user overruling the layout — see [`Mode`].
pub fn build_with(
    dir: &Path,
    sample_rate: u32,
    mode: Mode,
) -> Result<crate::instruments::sfz::SfzSampler> {
    build_id(dir, sample_rate, mode, SLICES)
}

/// The same, told how many pieces `SLICE` is cutting into — which is what the
/// instrument id carries, so that a project comes back cut the way it was left.
pub fn build_id(
    dir: &Path,
    sample_rate: u32,
    mode: Mode,
    slices: usize,
) -> Result<crate::instruments::sfz::SfzSampler> {
    let samples = describe(dir);
    if samples.is_empty() {
        bail!("{}: no samples in it", dir.display());
    }
    let regions = map::regions_with_slices(&samples, mode, slices);
    if regions.is_empty() {
        bail!("{}: nothing in it could be mapped to a key", dir.display());
    }
    // `violin.zip` is the violin, not "violin.zip"; `percussion.zip/bass drum`
    // is the bass drum.
    let name = match archive::is_archive(dir) {
        true => dir.file_stem(),
        false => dir.file_name(),
    }
    .map(|s| s.to_string_lossy().into_owned())
    .unwrap_or_else(|| "samples".into());
    // What it can be played with, and the keys that select them. A pack with
    // one articulation says nothing and behaves exactly as it did.
    let names = map::articulation_groups(&samples);
    let base = map::switch_base(&regions, names.len());
    let mut instrument = crate::instruments::sfz::SfzSampler::from_regions(regions, sample_rate, name)?;
    instrument.set_articulations(names, base);
    Ok(instrument)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The folders count when the filenames stay quiet, and the filename wins
    /// when they disagree — the more specific statement is the one to believe.
    #[test]
    fn a_folder_can_say_what_the_filename_does_not() {
        let dir = Path::new("/lib/violin");
        let from_dir = describe_one(dir, Path::new("/lib/violin/staccato/C4.wav"), 0);
        assert_eq!(from_dir.articulation, Articulation::Staccato);
        assert_eq!(from_dir.root, Some(60));

        let from_file = describe_one(dir, Path::new("/lib/violin/staccato/C4_pizz.wav"), 0);
        assert_eq!(from_file.articulation, Articulation::Pizzicato);
    }

    /// A sampler with no folder is a real instrument: it goes in a tab, it
    /// answers notes, and what it answers with is silence until somebody says
    /// where the samples are.
    #[test]
    fn an_empty_sampler_is_silent_and_survives_being_played() {
        use crate::sources::AudioSource;

        let mut instrument = empty();
        let mut buf = vec![1.0f32; 256];
        instrument.note_on(60, 100);
        instrument.render(&mut buf, 48_000);
        assert!(
            buf.iter().all(|s| *s == 0.0),
            "an empty sampler made a sound"
        );
        instrument.note_off(60);
        instrument.all_notes_off();
    }

    /// The mode rides on the instrument's id, because that is what a project
    /// saves and what a rack reopens with.
    #[test]
    fn a_mode_travels_on_the_instrument_id() {
        let id = format!("/Samples/toms{}", Mode::Kit.suffix());
        assert_eq!(Mode::of_id(&id), Mode::Kit);
        assert_eq!(Mode::strip(&id), "/Samples/toms");

        assert_eq!(Mode::of_id("/Samples/toms"), Mode::Auto);
        assert_eq!(Mode::strip("/Samples/toms"), "/Samples/toms");
        // A path with a `#` in it is a path, not a mode.
        assert_eq!(Mode::of_id("/Samples/drum#2"), Mode::Auto);
        assert_eq!(Mode::strip("/Samples/drum#2"), "/Samples/drum#2");
    }

    /// How many pieces SLICE cut into rides on the id beside the layout, so a
    /// project comes back cut the way it was left — and a project written
    /// before the count existed says the default.
    #[test]
    fn the_id_carries_how_many_pieces() {
        for n in SLICE_CHOICES.iter().copied() {
            let id = format!("/Samples/break{}", slice_suffix(n));
            assert_eq!(Mode::of_id(&id), Mode::Slice, "{id}");
            assert_eq!(Mode::strip(&id), "/Samples/break", "{id}");
            assert_eq!(slices_of_id(&id), n, "{id}");
        }
        // The old spelling, and the ids that are not slicing at all.
        assert_eq!(slices_of_id("/Samples/break#slice"), SLICES);
        assert_eq!(Mode::of_id("/Samples/break#slice"), Mode::Slice);
        assert_eq!(slices_of_id("/Samples/break#kit"), SLICES);
        assert_eq!(Mode::of_id("/Samples/break#slicex"), Mode::Auto);
    }

    /// The whole trip, on files: three notes in a folder become an instrument
    /// that answers a key none of them was recorded at.
    ///
    /// This is the check that the pieces fit — the scan, the name parser, the
    /// mapper and the player each have their own tests, and none of them would
    /// notice if the folder never reached the player at all.
    #[test]
    fn a_folder_of_wavs_becomes_something_that_plays() {
        use crate::sources::AudioSource;

        let dir = std::env::temp_dir().join(format!("choz-sampler-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (name, freq) in [
            ("violin_C3", 130.81),
            ("violin_C4", 261.63),
            ("violin_C5", 523.25),
        ] {
            let spec = hound::WavSpec {
                channels: 1,
                sample_rate: 48_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            };
            let mut w = hound::WavWriter::create(dir.join(format!("{name}.wav")), spec).unwrap();
            for i in 0..24_000 {
                let t = i as f32 / 48_000.0;
                let s = (std::f32::consts::TAU * freq * t).sin() * 0.5;
                w.write_sample((s * i16::MAX as f32) as i16).unwrap();
            }
            w.finalize().unwrap();
        }

        let samples = describe(&dir);
        assert_eq!(samples.len(), 3);
        // Named notes: nothing here was decoded to find a pitch.
        assert!(samples.iter().all(|s| s.from_name));
        assert_eq!(map::regions(&samples).len(), 3);

        let mut instrument = build(&dir, 48_000).expect("the folder would not build");
        let mut buf = vec![0.0f32; 512];
        instrument.render(&mut buf, 48_000);
        assert!(buf.iter().all(|s| *s == 0.0), "sound before a note");
        // A key no sample was recorded at, inside C4's zone: it plays, because
        // the mapper gave that sample the keys around it.
        instrument.note_on(58, 100);
        instrument.render(&mut buf, 48_000);
        assert!(
            buf.iter().any(|s| s.abs() > 0.05),
            "the instrument stayed silent"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The same folder, zipped: one file on disk, an instrument all the same.
    /// Sample libraries arrive this way — Philharmonia is twenty zips — and
    /// making somebody unpack 350 MB before choz will look at it is a step
    /// with no reason behind it.
    #[test]
    fn a_zip_of_wavs_is_an_instrument_too() {
        use crate::sources::AudioSource;
        use std::io::Write;

        let dir = std::env::temp_dir().join(format!("choz-sampler-zip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let zip_path = dir.join("violin.zip");

        let mut zip = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
        for (name, freq) in [
            ("violin_C3", 130.81),
            ("violin_C4", 261.63),
            ("violin_C5", 523.25),
        ] {
            // A WAV built by hand: `hound` writes to a file, and what goes in
            // here is bytes.
            let frames: Vec<i16> = (0..24_000)
                .map(|i| {
                    let t = i as f32 / 48_000.0;
                    ((std::f32::consts::TAU * freq * t).sin() * 0.5 * i16::MAX as f32) as i16
                })
                .collect();
            let data: Vec<u8> = frames.iter().flat_map(|s| s.to_le_bytes()).collect();
            let mut wav = Vec::new();
            wav.extend_from_slice(b"RIFF");
            wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
            wav.extend_from_slice(b"WAVEfmt ");
            wav.extend_from_slice(&16u32.to_le_bytes());
            wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
            wav.extend_from_slice(&1u16.to_le_bytes()); // mono
            wav.extend_from_slice(&48_000u32.to_le_bytes());
            wav.extend_from_slice(&96_000u32.to_le_bytes()); // bytes per second
            wav.extend_from_slice(&2u16.to_le_bytes()); // block align
            wav.extend_from_slice(&16u16.to_le_bytes()); // bits
            wav.extend_from_slice(b"data");
            wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
            wav.extend_from_slice(&data);
            zip.start_file(format!("{name}.wav"), zip::write::FileOptions::default())
                .unwrap();
            zip.write_all(&wav).unwrap();
        }
        zip.finish().unwrap();

        // The shelf is not the instrument; the zip on it is.
        assert!(!is_instrument_dir(&dir), "the folder of zips is not one");
        assert!(is_instrument_dir(&zip_path), "the zip is");

        // And a scan of the shelf finds the instrument in it, under the name
        // the archive is called by rather than the file it is.
        let found = crate::paths::scan_dir(&dir, crate::PluginFormat::Samples);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].name, "violin");
        assert_eq!(found[0].path, zip_path);
        assert!(found[0].is_instrument);

        let samples = describe(&zip_path);
        assert_eq!(samples.len(), 3);
        assert!(
            samples.iter().all(|s| s.bytes > 0),
            "no size off the entries"
        );
        assert_eq!(map::regions(&samples).len(), 3);

        let mut instrument = build(&zip_path, 48_000).expect("the zip would not build");
        let mut buf = vec![0.0f32; 512];
        instrument.note_on(58, 100);
        instrument.render(&mut buf, 48_000);
        assert!(
            buf.iter().any(|s| s.abs() > 0.05),
            "nothing came out of the archive"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An archive with its samples in folders is a shelf, not an instrument:
    /// Philharmonia's `percussion.zip` holds 39 of them. Read as one it is a
    /// kit of 148 sounds fighting over 92 keys.
    #[test]
    fn an_archive_of_folders_is_an_instrument_per_folder() {
        use std::io::Write;

        let dir = std::env::temp_dir().join(format!("choz-sampler-shelf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let zip_path = dir.join("percussion.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
        for entry in [
            "bass drum/kick.wav",
            "bass drum/kick-soft.wav",
            "snare/hit.wav",
        ] {
            zip.start_file(entry, zip::write::FileOptions::default())
                .unwrap();
            // The bytes never get decoded here: what is under test is which
            // instruments the archive offers, not what they sound like.
            zip.write_all(b"not really a wav").unwrap();
        }
        zip.finish().unwrap();

        let mut found: Vec<String> = instruments(&zip_path)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        found.sort();
        assert_eq!(found, ["bass drum", "snare"]);

        // Each one is reachable on its own, and holds only its own samples.
        let drum = zip_path.join("bass drum");
        assert!(is_instrument_dir(&drum));
        assert_eq!(files(&drum).len(), 2);
        assert_eq!(files(&zip_path.join("snare")).len(), 1);

        let scanned = crate::paths::scan_dir(&dir, crate::PluginFormat::Samples);
        assert_eq!(scanned.len(), 2, "{scanned:?}");
        assert!(scanned.iter().any(|p| p.name == "bass drum"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A named note is not re-derived from the audio — which is also what
    /// makes a scan of a well-named pack instant, since nothing gets decoded.
    #[test]
    fn a_named_note_is_taken_at_its_word() {
        // The file does not exist; if this tried to decode it, it would say so
        // by coming back with no root at all.
        let s = describe_one(Path::new("/lib"), Path::new("/lib/violin_C4_mf.wav"), 0);
        assert_eq!(s.root, Some(60));
        assert_eq!(s.confidence, 1.0);
        assert!(s.from_name);
        assert_eq!(s.velocity, Some(76));
    }

    /// A held note loops **between the points it was given**, and the join is
    /// faded rather than butted together — which is what stops a sustain that
    /// does not close at zero from clicking once a second.
    #[test]
    fn a_loop_has_its_own_points_and_a_fade_across_the_join() {
        use crate::instruments::sfz::Knobs;
        use crate::sources::AudioSource;

        let dir = std::env::temp_dir().join(format!("choz-sampler-loop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        // A second of tone that ends on a step: the first half is loud and the
        // second half is silent, so a loop that ran past its end is audible as
        // silence and a join with no fade is audible as a click.
        let mut w = hound::WavWriter::create(dir.join("pad_C4.wav"), spec).unwrap();
        for i in 0..48_000 {
            let t = i as f32 / 48_000.0;
            let s = match i < 24_000 {
                true => (std::f32::consts::TAU * 220.0 * t).sin() * 0.5,
                false => 0.0,
            };
            w.write_sample((s * i16::MAX as f32) as i16).unwrap();
        }
        w.finalize().unwrap();

        let mut instrument = build(&dir, 48_000).expect("the folder would not build");
        // LOOP on, and the loop is the first quarter of the file: a note held
        // for a second must never reach the silent half.
        instrument.set_param(1, 1.0);
        instrument.set_param(6, 0.0);
        instrument.set_param(7, 0.25);
        // …and the release long enough that the envelope is not what is being
        // measured.
        instrument.set_param(5, 1.0);
        instrument.note_on(60, 100);
        let mut buf = vec![0.0f32; 2 * 48_000];
        instrument.render(&mut buf, 48_000);
        // The last tenth of the second still sounds: without loop points it
        // would be in the silent half of the file.
        let tail = &buf[buf.len() - 9_600..];
        assert!(
            tail.iter().any(|s| s.abs() > 0.05),
            "the note ran past its loop"
        );

        // The crossfade is a knob, and it is the last one.
        let mut k = Knobs::default();
        k.set(8, 1.0);
        assert!((k.crossfade - Knobs::MAX_CROSSFADE).abs() < 1e-6);
        k.set(6, 0.5);
        k.set(7, 0.75);
        assert!((k.loop_start - 0.5).abs() < 1e-6 && (k.loop_end - 0.75).abs() < 1e-6);
        // The six that were here first did not move: a project saved before
        // these existed reads its values into the same knobs.
        assert_eq!(params()[0].name, "START");
        assert_eq!(params()[5].name, "RELEASE");
        assert_eq!(params().len(), 10);
        assert_eq!(params()[9].name, "END");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A pack with two ways of playing the note gets both, and the keys under
    /// the map choose between them: the keyswitch, which is what made a pack
    /// with sustain and staccato in it half a pack.
    #[test]
    fn two_articulations_are_both_in_the_map_and_a_key_chooses() {
        use crate::sources::AudioSource;

        let dir = std::env::temp_dir().join(format!("choz-sampler-ks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        // The sustain rings for half a second; the staccato is a tenth of it.
        // Both cover the same three notes, which is what makes them two
        // articulations of one instrument rather than an odd file.
        for (art, frames) in [("sustain", 24_000usize), ("staccato", 2_400)] {
            for (name, freq) in [("C3", 130.81), ("C4", 261.63), ("C5", 523.25)] {
                let file = dir.join(format!("viola_{name}_{art}.wav"));
                let mut w = hound::WavWriter::create(file, spec).unwrap();
                for i in 0..frames {
                    let t = i as f32 / 48_000.0;
                    let s = (std::f32::consts::TAU * freq * t).sin() * 0.5;
                    w.write_sample((s * i16::MAX as f32) as i16).unwrap();
                }
                w.finalize().unwrap();
            }
        }

        let samples = describe(&dir);
        let groups = map::articulation_groups(&samples);
        assert_eq!(groups.len(), 2, "{groups:?}");
        let regions = map::regions(&samples);
        assert!(
            regions.iter().any(|r| r.articulation == 1),
            "the second articulation is not in the map"
        );

        let mut instrument = build(&dir, 48_000).expect("the folder would not build");
        assert_eq!(instrument.articulations().len(), 2);
        let base = instrument.switch_base().expect("no keyswitches");
        // Under everything that was recorded. A map stretched to the bottom of
        // the keyboard leaves no gap, and then they take the bottom keys —
        // which is what `map::switch_base` says and why this is `<=`.
        let lowest_root = samples.iter().filter_map(|s| s.root).min().unwrap();
        assert!(base < lowest_root, "{base} is not under the instrument");

        // The switch selects and does not sound.
        let mut buf = vec![0.0f32; 512];
        instrument.note_on(base + 1, 100);
        instrument.render(&mut buf, 48_000);
        assert!(buf.iter().all(|s| *s == 0.0), "a keyswitch made a sound");
        assert_eq!(instrument.articulation(), 1);

        // …and what plays now is the short one: half a second in, the staccato
        // has been over for a long time.
        let mut long = vec![0.0f32; 2 * 24_000];
        instrument.note_on(60, 100);
        instrument.render(&mut long, 48_000);
        let tail = &long[long.len() - 4_800..];
        assert!(
            tail.iter().all(|s| s.abs() < 0.01),
            "the sustain played under a staccato keyswitch"
        );

        // Back to the first, and the same note rings.
        instrument.note_off(60);
        instrument.note_on(base, 100);
        assert_eq!(instrument.articulation(), 0);
        instrument.note_on(60, 100);
        let mut long = vec![0.0f32; 2 * 24_000];
        instrument.render(&mut long, 48_000);
        let tail = &long[long.len() - 4_800..];
        assert!(
            tail.iter().any(|s| s.abs() > 0.01),
            "the sustain did not come back"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A long sample is **not** held in memory: the first load decodes it once
    /// into the raw cache and maps it, and every load after that is a map and no
    /// decode at all. A short one is held exactly as it was — nothing about a
    /// drum kit was slow.
    #[test]
    fn a_long_sample_goes_out_to_disk_and_plays_from_there() {
        use crate::instruments::stream;
        use crate::sources::AudioSource;

        let dir = std::env::temp_dir().join(format!("choz-sampler-stream-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        // Three seconds — past `stream::THRESHOLD_FRAMES` — and a second file
        // short enough to stay in memory.
        for (name, frames) in [
            ("cello_C3", stream::THRESHOLD_FRAMES + 48_000),
            ("cello_C5", 4_800),
        ] {
            let mut w = hound::WavWriter::create(dir.join(format!("{name}.wav")), spec).unwrap();
            for i in 0..frames {
                let t = i as f32 / 48_000.0;
                let s = (std::f32::consts::TAU * 220.0 * t).sin() * 0.5;
                w.write_sample((s * i16::MAX as f32) as i16).unwrap();
            }
            w.finalize().unwrap();
        }
        let long = dir.join("cello_C3.wav");
        let short = dir.join("cello_C5.wav");
        // Nothing cached before the first load.
        let _ = std::fs::remove_file(stream::cache_path(&long, 48_000));
        assert!(stream::mapped(&long, 48_000).is_none());

        let mut instrument = build(&dir, 48_000).expect("the folder would not build");
        // The long one is out on disk now; the short one was never written.
        assert!(
            stream::mapped(&long, 48_000).is_some(),
            "the long sample was not cached"
        );
        assert!(
            stream::mapped(&short, 48_000).is_none(),
            "a short sample does not need a cache"
        );

        // …and it plays from there, past the head that is kept in memory.
        instrument.note_on(48, 100);
        let mut buf = vec![0.0f32; 2 * (stream::HEAD_FRAMES + 24_000)];
        instrument.render(&mut buf, 48_000);
        let tail = &buf[2 * stream::HEAD_FRAMES..];
        assert!(
            tail.iter().any(|s| s.abs() > 0.05),
            "silent past the head — the mapping is not being read"
        );

        // The second load reads the cache and does not decode: the proof is a
        // file whose audio has been made unreadable — the same name, the same
        // length and the same mtime, so the cache still answers to it, but
        // nothing in it can be decoded any more.
        let bytes = std::fs::metadata(&long).unwrap().len() as usize;
        let meta = std::fs::File::open(&long).unwrap().metadata().unwrap();
        std::fs::write(&long, vec![0u8; bytes]).unwrap();
        let file = std::fs::File::options().write(true).open(&long).unwrap();
        file.set_modified(meta.modified().unwrap()).unwrap();
        drop(file);
        let mut again = build(&dir, 48_000).expect("the second load failed");
        again.note_on(48, 100);
        // Two seconds. Only the long sample can still be sounding a second in:
        // the short one is a tenth of a second even transposed down two
        // octaves.
        let mut buf = vec![0.0f32; 2 * 96_000];
        again.render(&mut buf, 48_000);
        assert!(
            buf[2 * 48_000..].iter().any(|s| s.abs() > 0.05),
            "a second in there is nothing: the cache was not used"
        );

        let _ = std::fs::remove_file(stream::cache_path(&long, 48_000));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// START and END are a window on the sample: what is outside them is not
    /// played, which is what the two marks on the panel mean.
    #[test]
    fn start_and_end_are_the_window_a_key_plays() {
        use crate::sources::AudioSource;

        let dir = std::env::temp_dir().join(format!("choz-sampler-window-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        // A second: loud for the first half, silent for the second.
        let mut w = hound::WavWriter::create(dir.join("pad_C4.wav"), spec).unwrap();
        for i in 0..48_000 {
            let t = i as f32 / 48_000.0;
            let s = match i < 24_000 {
                true => (std::f32::consts::TAU * 220.0 * t).sin() * 0.5,
                false => 0.0,
            };
            w.write_sample((s * i16::MAX as f32) as i16).unwrap();
        }
        w.finalize().unwrap();

        let played = |start: f32, end: f32| -> usize {
            let mut instrument = build(&dir, 48_000).expect("the folder would not build");
            instrument.set_param(0, start);
            instrument.set_param(9, end);
            // No release tail in the way of counting frames.
            instrument.set_param(5, 0.0);
            instrument.note_on(60, 100);
            let mut buf = vec![0.0f32; 2 * 48_000];
            instrument.render(&mut buf, 48_000);
            buf.chunks(2).filter(|f| f[0].abs() > 0.01).count()
        };

        // The whole loud half.
        let all = played(0.0, 1.0);
        assert!(all > 20_000, "{all} frames of a half-second tone");
        // Stopped a quarter in: a quarter of the file, not half.
        let quarter = played(0.0, 0.25);
        assert!(
            (10_000..14_000).contains(&quarter),
            "END at 25% played {quarter} frames"
        );
        // …and started halfway, where the file goes quiet: nothing.
        assert_eq!(played(0.5, 1.0), 0, "the silent half sounded");
        // END under START is nothing said, not silence.
        assert_eq!(played(0.0, 0.0), all, "END at zero silenced the note");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The library on this machine, when it is here: twenty `.zip`s in one
    /// folder, which is the shape a sample library actually arrives in. Skipped
    /// where it is not — a test that needs somebody's disk is a test that says
    /// so and gets out of the way.
    #[test]
    #[ignore]
    fn the_real_library_reads_as_one_instrument_per_archive() {
        let dir = std::path::Path::new("/home/jorge/repo/philharmonia/all-samples");
        if !dir.is_dir() {
            return;
        }
        let packs: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| archive::is_archive(p))
            .collect();
        assert!(!packs.is_empty(), "no archives in {}", dir.display());
        for pack in packs.iter().take(3) {
            let found = instruments(pack);
            assert!(!found.is_empty(), "{} holds no instrument", pack.display());
            println!("{} -> {} instrument(s)", pack.display(), found.len());
        }
    }
}
