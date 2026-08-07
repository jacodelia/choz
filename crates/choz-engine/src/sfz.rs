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
use std::sync::Arc;

use anyhow::{Context, Result, bail};

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
}

impl SfzRegion {
    fn matches(&self, note: u8, vel: u8) -> bool {
        (self.lo_key..=self.hi_key).contains(&note) && (self.lo_vel..=self.hi_vel).contains(&vel)
    }

    /// Playback rate that transposes the sample to `note`.
    fn rate_for_note(&self, note: u8) -> f32 {
        let semitones = note as i32 - self.pitch_key_center as i32;
        2.0_f32.powf(semitones as f32 / 12.0)
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
}

impl Default for Defaults {
    fn default() -> Self {
        Self { lo_key: 0, hi_key: 127, pkc: 60, lo_vel: 0, hi_vel: 127, gain: 1.0 }
    }
}

/// Parse an `.sfz` file. Sample paths come back absolute.
pub fn parse_file(path: &Path) -> Result<Vec<SfzRegion>> {
    let base = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read {}", path.display()))?;
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
                    let Some((key, val)) = t.split_once('=') else { continue };
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
            !key.is_empty()
                && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
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

/// Decode a sample to interleaved stereo at `target_sr`.
///
/// ponytail: linear interpolation for the rate conversion, both here and in the
/// voice. A sampler that needs better than that needs a real resampler, and
/// that is a different piece of work.
fn decode(path: &Path, target_sr: u32) -> Result<Vec<f32>> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let file = std::fs::File::open(path)
        .with_context(|| format!("cannot open sample {}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
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
    let track = format.default_track().context("sample has no audio track")?;
    let track_id = track.id;
    let channels = track.codec_params.channels.map(|c| c.count()).unwrap_or(1).max(1);
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
    pcm: Arc<Vec<f32>>,
}

struct Voice {
    note: u8,
    gain: f32,
    rate: f64,
    pcm: Arc<Vec<f32>>,
    /// Read head, in frames.
    pos: f64,
}

/// An SFZ instrument in a rack slot: notes in, interleaved stereo out.
pub struct SfzSampler {
    regions: Vec<Loaded>,
    voices: Vec<Voice>,
    name: String,
}

impl SfzSampler {
    /// Parse `path` and decode every sample it references to `sample_rate`.
    /// Regions whose sample is missing or undecodable are dropped with a
    /// message; the instrument still loads if anything is left.
    pub fn build(path: &Path, sample_rate: u32) -> Result<Self> {
        let parsed = parse_file(path)?;
        // One decode per distinct file: regions of the same instrument share
        // samples far more often than not.
        let mut cache: HashMap<PathBuf, Option<Arc<Vec<f32>>>> = HashMap::new();
        let mut regions = Vec::new();
        for region in parsed {
            let pcm = cache
                .entry(region.sample.clone())
                .or_insert_with(|| match decode(&region.sample, sample_rate) {
                    Ok(pcm) if !pcm.is_empty() => Some(Arc::new(pcm)),
                    Ok(_) => None,
                    Err(e) => {
                        eprintln!("choz: SFZ {}: {e}", region.sample.display());
                        None
                    }
                })
                .clone();
            if let Some(pcm) = pcm {
                regions.push(Loaded { region, pcm });
            }
        }
        if regions.is_empty() {
            bail!("{}: none of its samples could be loaded", path.display());
        }
        Ok(Self {
            regions,
            voices: Vec::with_capacity(MAX_VOICES),
            name: path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "SFZ".into()),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

impl AudioSource for SfzSampler {
    fn render(&mut self, out: &mut [f32], _sample_rate: u32) -> usize {
        let frames = out.len() / 2;
        out.fill(0.0);
        for voice in &mut self.voices {
            let last = voice.pcm.len() / 2 - 1;
            for f in 0..frames {
                let i0 = voice.pos as usize;
                if i0 >= last {
                    voice.pos = f64::INFINITY;
                    break;
                }
                let t = (voice.pos - i0 as f64) as f32;
                let l = voice.pcm[i0 * 2] + t * (voice.pcm[(i0 + 1) * 2] - voice.pcm[i0 * 2]);
                let r =
                    voice.pcm[i0 * 2 + 1] + t * (voice.pcm[(i0 + 1) * 2 + 1] - voice.pcm[i0 * 2 + 1]);
                out[f * 2] += l * voice.gain;
                out[f * 2 + 1] += r * voice.gain;
                voice.pos += voice.rate;
            }
        }
        // Finished voices go here, not in the loop above: retain never
        // allocates, so this stays RT-safe.
        self.voices.retain(|v| v.pos.is_finite());
        frames
    }

    fn note_on(&mut self, note: u8, velocity: u8) {
        if velocity == 0 {
            self.note_off(note);
            return;
        }
        let Some(hit) = self.regions.iter().find(|r| r.region.matches(note, velocity)) else {
            return;
        };
        if self.voices.len() == MAX_VOICES {
            // Steal the oldest rather than grow: `push` past the capacity would
            // allocate on the audio thread.
            self.voices.remove(0);
        }
        self.voices.push(Voice {
            note,
            gain: hit.region.gain * (velocity as f32 / 127.0),
            rate: hit.region.rate_for_note(note) as f64,
            pcm: Arc::clone(&hit.pcm),
            pos: 0.0,
        });
    }

    fn note_off(&mut self, note: u8) {
        self.voices.retain(|v| v.note != note);
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
        assert!((regions[0].gain - 0.5011872).abs() < 1e-4, "-6 dB: {}", regions[0].gain);
        // `key` sets range and root pitch at once.
        assert_eq!((regions[1].lo_key, regions[1].hi_key, regions[1].pitch_key_center), (38, 38, 38));
    }

    /// Sample paths with spaces are the norm in commercial libraries, and the
    /// value runs until the next opcode.
    #[test]
    fn a_sample_path_may_contain_spaces() {
        let sfz = "<region> sample=Saw Samples/Saw_C-3.flac lokey=36 hikey=47";
        let regions = parse_text(sfz, Path::new("/lib"));
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].sample, Path::new("/lib/Saw Samples/Saw_C-3.flac"));
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
        let pcm = Arc::new(vec![0.5f32; 8]); // 4 stereo frames
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
                },
                pcm,
            }],
            voices: Vec::with_capacity(MAX_VOICES),
            name: "test".into(),
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
        assert!(s.voices.is_empty(), "note-off stops it");
    }
}
