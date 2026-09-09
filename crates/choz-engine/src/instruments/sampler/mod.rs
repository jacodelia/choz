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
    pub fn of_id(id: &str) -> Mode {
        Mode::ALL
            .iter()
            .copied()
            .find(|m| !m.suffix().is_empty() && id.ends_with(m.suffix()))
            .unwrap_or(Mode::Auto)
    }

    /// The id with the mode taken off: the path it names.
    pub fn strip(id: &str) -> &str {
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

/// How many pieces [`Mode::Slice`] cuts a sample into: two octaves of keys,
/// which is what a break wants and what fits under two hands.
pub const SLICES: usize = 16;

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
    ];
    for (i, param) in out.iter_mut().enumerate() {
        param.id = i as u32;
    }
    out
}

/// The shape of one sample, as `buckets` peak levels 0..1 — what the panel
/// draws as a waveform.
///
/// The file is decoded to do it, which is why this is called once when a
/// folder is adopted and not while anything is drawing.
pub fn peaks(path: &Path, buckets: usize) -> Vec<f32> {
    let Ok(pcm) = crate::instruments::sfz::decode(path, 44_100) else {
        return Vec::new();
    };
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

/// The same, with the user overruling the layout — see [`Mode`].
pub fn build_with(
    dir: &Path,
    sample_rate: u32,
    mode: Mode,
) -> Result<crate::instruments::sfz::SfzSampler> {
    let samples = describe(dir);
    if samples.is_empty() {
        bail!("{}: no samples in it", dir.display());
    }
    let regions = map::regions_with(&samples, mode);
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
    crate::instruments::sfz::SfzSampler::from_regions(regions, sample_rate, name)
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
}
