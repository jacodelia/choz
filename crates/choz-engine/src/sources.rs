//! Audio sources — what *generates* the audio that feeds the FX chain.
//!
//! Trait shape copied from seqterm-audio-engine's `AudioSource`: the render
//! method runs inside the RT callback, so it must be allocation-free and
//! lock-free. Concrete sources (built here, off the RT thread) are handed to
//! the engine over a ring and swapped in by the callback.

use anyhow::{Context, Result};
use std::path::Path;

// The AudioSource trait lives in `choz-ports`; re-exported so `crate::sources::
// AudioSource` (used by engine.rs and the impls below) keeps resolving.
pub use choz_ports::AudioSource;

/// The default source: a 440 Hz sine. Endless.
pub struct TestTone {
    phase: f32,
    freq: f32,
    amp: f32,
}

impl TestTone {
    pub fn new() -> Self {
        Self {
            phase: 0.0,
            freq: 440.0,
            amp: 0.3,
        }
    }
}

impl Default for TestTone {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioSource for TestTone {
    fn render(&mut self, out: &mut [f32], sample_rate: u32) -> usize {
        let sr = sample_rate as f32;
        let frames = out.len() / 2;
        for i in 0..frames {
            let s = (2.0 * std::f32::consts::PI * self.phase).sin() * self.amp;
            self.phase = (self.phase + self.freq / sr) % 1.0;
            out[i * 2] = s;
            out[i * 2 + 1] = s;
        }
        frames
    }
}

/// A rack slot with no instrument yet: renders silence, ignores notes. Keeps UI
/// slots and engine slots index-aligned before an instrument is chosen.
pub struct Silence;

impl AudioSource for Silence {
    fn render(&mut self, out: &mut [f32], _sample_rate: u32) -> usize {
        out.fill(0.0);
        out.len() / 2
    }

    fn plays_on_transport_stop(&self) -> bool {
        true
    }
}

/// Plays a WAV file (decoded up-front into memory) as a stereo source.
///
/// Sample-rate mismatch between the file and the engine is handled with linear
/// interpolation.
/// ponytail: linear resampling — fine for playback; swap for a windowed-sinc
/// resampler only if audible artifacts matter.
pub struct WavPlayer {
    /// Interleaved stereo samples at `file_rate`.
    samples: Vec<f32>,
    file_rate: u32,
    looping: bool,
    /// Fractional read position, in frames.
    pos: f64,
    finished: bool,
}

impl WavPlayer {
    /// Decode a WAV file into memory. Non-RT: does file I/O and allocation.
    pub fn load(path: &Path, looping: bool) -> Result<Self> {
        let mut reader = hound::WavReader::open(path)
            .with_context(|| format!("cannot open WAV: {}", path.display()))?;
        let spec = reader.spec();
        let channels = spec.channels.max(1) as usize;

        // Read every sample as f32, normalizing integer formats to -1.0..1.0.
        let mono_or_multi: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => {
                reader.samples::<f32>().map(|s| s.unwrap_or(0.0)).collect()
            }
            hound::SampleFormat::Int => {
                let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
                reader
                    .samples::<i32>()
                    .map(|s| s.unwrap_or(0) as f32 / max)
                    .collect()
            }
        };

        // Fold to interleaved stereo: mono → duplicate, >2ch → take first two.
        let frames = mono_or_multi.len() / channels;
        let mut samples = Vec::with_capacity(frames * 2);
        for f in 0..frames {
            let base = f * channels;
            let l = mono_or_multi[base];
            let r = if channels >= 2 {
                mono_or_multi[base + 1]
            } else {
                l
            };
            samples.push(l);
            samples.push(r);
        }

        let finished = samples.len() < 2;
        Ok(Self {
            samples,
            file_rate: spec.sample_rate,
            looping,
            pos: 0.0,
            finished,
        })
    }

    fn frame(&self, idx: usize) -> (f32, f32) {
        let i = idx * 2;
        (self.samples[i], self.samples[i + 1])
    }

    fn total_frames(&self) -> usize {
        self.samples.len() / 2
    }
}

impl AudioSource for WavPlayer {
    fn render(&mut self, out: &mut [f32], sample_rate: u32) -> usize {
        let total = self.total_frames();
        if self.finished || total == 0 {
            out.fill(0.0);
            return 0;
        }
        let step = self.file_rate as f64 / sample_rate as f64;
        let frames = out.len() / 2;

        for i in 0..frames {
            if self.pos >= total as f64 {
                if self.looping {
                    self.pos %= total as f64;
                } else {
                    // Zero-fill the rest of the block and stop.
                    for s in &mut out[i * 2..] {
                        *s = 0.0;
                    }
                    self.finished = true;
                    return i;
                }
            }
            let i0 = self.pos.floor() as usize;
            let frac = (self.pos - i0 as f64) as f32;
            let (l0, r0) = self.frame(i0);
            let (l1, r1) = self.frame((i0 + 1) % total);
            out[i * 2] = l0 + (l1 - l0) * frac;
            out[i * 2 + 1] = r0 + (r1 - r0) * frac;
            self.pos += step;
        }
        frames
    }
}

/// SoundFont (SF2) synth, backed by the pure-Rust oxisynth engine.
///
/// Loaded off the RT thread; `render`/`note_on`/`note_off` are RT-safe (oxisynth
/// pre-allocates its voice pool). Single MIDI channel 0. Pattern adapted from
/// seqterm-audio-engine's `SoundFontSynth`.
pub struct Sf2Synth {
    synth: oxisynth::Synth,
    /// Font handle, kept so program changes can re-select on the same font.
    font_id: oxisynth::SoundFontId,
    /// Pre-allocated de-interleaved render scratch (oxisynth writes L/R planar).
    buf_l: Vec<f32>,
    buf_r: Vec<f32>,
}

/// Max render block, in frames. cpal blocks are far smaller (256).
const SF2_MAX_FRAMES: usize = 4096;

impl Sf2Synth {
    /// Load an SF2 file and select `bank`/`preset` on channel 0. Non-RT (file I/O).
    pub fn load(path: &Path, bank: u8, preset: u8, sample_rate: u32) -> Result<Self> {
        use oxisynth::{MidiEvent, SoundFont, Synth, SynthDescriptor};

        let desc = SynthDescriptor {
            sample_rate: sample_rate as f32,
            gain: 1.0,
            ..Default::default()
        };
        let mut synth =
            Synth::new(desc).map_err(|e| anyhow::anyhow!("oxisynth init failed: {e:?}"))?;

        let data =
            std::fs::read(path).with_context(|| format!("cannot read SF2: {}", path.display()))?;
        let sf = SoundFont::load(&mut std::io::Cursor::new(data))
            .map_err(|e| anyhow::anyhow!("SF2 parse error: {e:?}"))?;
        let font_id = synth.add_font(sf, true);

        // Fall back to bank 0 / preset 0 if the requested program is missing.
        if synth
            .select_program(0, font_id, bank as u32, preset)
            .is_err()
        {
            let _ = synth.select_program(0, font_id, 0, 0);
        }
        // GM channel volume so notes are audible.
        let _ = synth.send_event(MidiEvent::ControlChange {
            channel: 0,
            ctrl: 7,
            value: 100,
        });

        Ok(Self {
            synth,
            font_id,
            buf_l: vec![0.0; SF2_MAX_FRAMES],
            buf_r: vec![0.0; SF2_MAX_FRAMES],
        })
    }
}

/// One selectable program in a SoundFont.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sf2Preset {
    pub bank: u8,
    pub preset: u8,
    pub name: String,
}

impl Sf2Preset {
    pub fn label(&self) -> String {
        format!("{:03}:{:03} {}", self.bank, self.preset, self.name)
    }
}

/// List the programs in an SF2 file, sorted by bank then preset. Non-RT (reads
/// and parses the file). Uses `soundfont` directly — oxisynth keeps its preset
/// list private.
pub fn list_sf2_presets(path: &Path) -> Result<Vec<Sf2Preset>> {
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("cannot read SF2: {}", path.display()))?;
    let sf2 = soundfont::SoundFont2::load(&mut file)
        .map_err(|e| anyhow::anyhow!("SF2 parse error: {e:?}"))?;

    let mut presets: Vec<Sf2Preset> = sf2
        .presets
        .iter()
        // The hydra preset list is terminated by a sentinel record named "EOP".
        .filter(|p| p.header.name != "EOP")
        .map(|p| Sf2Preset {
            bank: p.header.bank.min(255) as u8,
            preset: p.header.preset.min(255) as u8,
            name: p.header.name.clone(),
        })
        .collect();
    presets.sort_by_key(|p| (p.bank, p.preset));
    Ok(presets)
}

impl AudioSource for Sf2Synth {
    fn render(&mut self, out: &mut [f32], _sample_rate: u32) -> usize {
        let frames = (out.len() / 2).min(SF2_MAX_FRAMES);
        {
            let l = &mut self.buf_l[..frames];
            let r = &mut self.buf_r[..frames];
            self.synth.write((l, r));
        }
        for i in 0..frames {
            out[i * 2] = self.buf_l[i];
            out[i * 2 + 1] = self.buf_r[i];
        }
        frames
    }

    fn note_on(&mut self, note: u8, velocity: u8) {
        let _ = self.synth.send_event(oxisynth::MidiEvent::NoteOn {
            channel: 0,
            key: note,
            vel: velocity,
        });
    }

    fn note_off(&mut self, note: u8) {
        let _ = self.synth.send_event(oxisynth::MidiEvent::NoteOff {
            channel: 0,
            key: note,
        });
    }

    /// The SoundFont engine can cut its own voices, which is more than the two
    /// CCs of the default: `AllSoundOff` kills the tails too, and a panic
    /// button that leaves a reverb tail ringing has not really panicked.
    fn all_notes_off(&mut self) {
        for channel in 0..16 {
            let _ = self
                .synth
                .send_event(oxisynth::MidiEvent::AllSoundOff { channel });
        }
    }

    fn control_change(&mut self, cc: u8, value: u8) {
        let _ = self.synth.send_event(oxisynth::MidiEvent::ControlChange {
            channel: 0,
            ctrl: cc,
            value,
        });
    }

    fn pitch_bend(&mut self, value: u16) {
        let _ = self.synth.send_event(oxisynth::MidiEvent::PitchBend {
            channel: 0,
            value: value.min(16383),
        });
    }

    fn program_change(&mut self, bank: u8, preset: u8) {
        // RT-safe: this only looks the preset up in the already-loaded font and
        // Arc-clones it into the channel.
        let _ = self
            .synth
            .select_program(0, self.font_id, bank as u32, preset);
    }

    fn plays_on_transport_stop(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Peak level of a rendered block — enough to tell "sounding" from "silent".
    fn peak(s: &mut Sf2Synth, frames: usize) -> f32 {
        let mut buf = vec![0.0f32; frames * 2];
        s.render(&mut buf, 48_000);
        buf.iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }

    /// The sustain pedal has to actually sustain: without it a note-off kills
    /// the note, with it held the note keeps ringing past the note-off.
    #[test]
    fn sustain_pedal_holds_notes_past_note_off() {
        let path = std::path::Path::new("/usr/share/sounds/sf2/FluidR3_GM.sf2");
        if !path.exists() {
            return; // ponytail: no bundled SF2 to test against, skip rather than fail.
        }
        let mut synth = Sf2Synth::load(path, 0, 0, 48_000).expect("load SF2");

        // Baseline: note-off with no pedal, the tail dies out.
        synth.note_on(60, 110);
        assert!(peak(&mut synth, 4096) > 0.01, "note must sound");
        synth.note_off(60);
        let mut dry = 0.0;
        for _ in 0..12 {
            dry = peak(&mut synth, 4096);
        }
        assert!(dry < 0.01, "released note should have decayed, peak {dry}");

        // Same again with sustain down: the note is still ringing.
        synth.control_change(64, 127);
        synth.note_on(60, 110);
        assert!(peak(&mut synth, 4096) > 0.01);
        synth.note_off(60);
        let mut held = 0.0;
        for _ in 0..12 {
            held = peak(&mut synth, 4096);
        }
        assert!(
            held > dry * 10.0,
            "sustain must hold the note: held {held} vs dry {dry}"
        );

        // Lifting the pedal releases it.
        synth.control_change(64, 0);
        let mut after = 0.0;
        for _ in 0..12 {
            after = peak(&mut synth, 4096);
        }
        assert!(
            after < held * 0.5,
            "lifting the pedal releases: {after} vs {held}"
        );
    }

    #[test]
    fn pitch_bend_shifts_the_rendered_tone() {
        let path = std::path::Path::new("/usr/share/sounds/sf2/FluidR3_GM.sf2");
        if !path.exists() {
            return;
        }
        let mut synth = Sf2Synth::load(path, 0, 0, 48_000).expect("load SF2");

        // Same note, centred vs bent fully up, compared by zero-crossing count:
        // a raised pitch crosses zero more often over the same window.
        let crossings = |s: &mut Sf2Synth| {
            let mut buf = vec![0.0f32; 8192];
            s.render(&mut buf, 48_000); // discard the attack
            s.render(&mut buf, 48_000);
            buf.chunks(2)
                .map(|f| f[0])
                .collect::<Vec<_>>()
                .windows(2)
                .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
                .count()
        };

        synth.pitch_bend(8192); // centre
        synth.note_on(60, 110);
        let centred = crossings(&mut synth);
        synth.note_off(60);

        synth.pitch_bend(16383); // fully up
        synth.note_on(60, 110);
        let bent = crossings(&mut synth);

        assert!(centred > 0, "note must sound to be measurable");
        assert!(
            bent > centred,
            "bending up must raise the pitch: {bent} vs {centred}"
        );
    }

    #[test]
    fn test_tone_fills_block() {
        let mut t = TestTone::new();
        let mut buf = [0.0f32; 16];
        let n = t.render(&mut buf, 48_000);
        assert_eq!(n, 8);
        assert!(buf.iter().any(|&s| s != 0.0), "tone must produce signal");
        assert!(buf.iter().all(|&s| s.abs() <= 0.3001), "amp bounded");
    }

    #[test]
    fn wav_player_plays_then_stops() {
        // Build a tiny 3-frame stereo source at the engine rate (no resample).
        let mut p = WavPlayer {
            samples: vec![1.0, -1.0, 0.5, -0.5, 0.25, -0.25],
            file_rate: 48_000,
            looping: false,
            pos: 0.0,
            finished: false,
        };
        let mut buf = [9.0f32; 8]; // 4 frames, source only has 3
        let n = p.render(&mut buf, 48_000);
        assert_eq!(n, 3, "renders available frames then stops");
        assert_eq!(&buf[0..6], &[1.0, -1.0, 0.5, -0.5, 0.25, -0.25]);
        assert_eq!(&buf[6..8], &[0.0, 0.0], "tail zero-filled");
        assert!(p.finished, "one-shot source stops at end");
    }

    #[test]
    fn wav_player_loops() {
        let mut p = WavPlayer {
            samples: vec![1.0, 1.0, 2.0, 2.0],
            file_rate: 48_000,
            looping: true,
            pos: 0.0,
            finished: false,
        };
        let mut buf = [0.0f32; 12]; // 6 frames over a 2-frame loop
        let n = p.render(&mut buf, 48_000);
        assert_eq!(n, 6);
        assert!(!p.finished, "looping source never finishes");
    }

    #[test]
    fn load_decodes_mono_wav_to_stereo() {
        // Write a 4-frame 16-bit mono WAV, then load it back.
        let path = std::env::temp_dir().join("choz_test_mono.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for &v in &[i16::MAX, 0, i16::MIN, 0] {
            w.write_sample(v).unwrap();
        }
        w.finalize().unwrap();

        let mut p = WavPlayer::load(&path, false).unwrap();
        assert_eq!(p.file_rate, 48_000);
        assert_eq!(p.total_frames(), 4, "mono folded to 4 stereo frames");

        let mut buf = [0.0f32; 8];
        let n = p.render(&mut buf, 48_000);
        assert_eq!(n, 4);
        // Full-scale positive ≈ +1.0, mono duplicated to both channels.
        assert!((buf[0] - 1.0).abs() < 0.001 && (buf[1] - buf[0]).abs() < 1e-6);
        let _ = std::fs::remove_file(&path);
    }
}
