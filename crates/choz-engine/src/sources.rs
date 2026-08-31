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
                    out[i * 2..].fill(0.0);
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
    /// Which sound zone each octave of the keyboard plays, `None` for the
    /// tab's own program. See [`AudioSource::set_split`].
    split: [Option<u8>; choz_ports::SPLIT_OCTAVES],
    /// Zones that have been given a program of their own. A zone nobody has
    /// pointed anywhere plays the tab's program, so it must not be routed to an
    /// empty channel and go silent.
    zone_set: [bool; ZONES],
    /// Pre-allocated de-interleaved render scratch (oxisynth writes L/R planar).
    buf_l: Vec<f32>,
    buf_r: Vec<f32>,
}

/// Max render block, in frames. cpal blocks are far smaller (256).
const SF2_MAX_FRAMES: usize = 4096;

/// How many keyboard zones can sound at once.
///
/// **One MIDI channel each, one font.** A split used to be a program change as
/// the hand crossed the join, so two hands in two zones fought over the one
/// patch and only the last note's sound survived — you could not hold a bass
/// note and play a pad over it. oxisynth has sixteen channels and the
/// SoundFont is loaded once for all of them, so a zone costs a channel and no
/// memory at all. Channel 0 stays the tab's own program, for every octave with
/// no zone on it.
const ZONES: usize = 8;

/// The MIDI channel zone `z` plays on. Zone 0 is channel 1: channel 0 belongs
/// to the tab itself.
fn zone_channel(zone: u8) -> u8 {
    (zone as usize % ZONES) as u8 + 1
}

impl Sf2Synth {
    /// The channel a note plays on: its octave's zone when that zone has been
    /// given a program, and the tab's own channel otherwise.
    fn channel_for(&self, note: u8) -> u8 {
        self.split
            .get(note as usize / 12)
            .copied()
            .flatten()
            .filter(|z| self.zone_set.get(*z as usize % ZONES) == Some(&true))
            .map(zone_channel)
            .unwrap_or(0)
    }
}

impl Sf2Synth {
    /// Load an SF2 file and select `bank`/`preset` on channel 0. Non-RT (file I/O).
    pub fn load(path: &Path, bank: u8, preset: u8, sample_rate: u32) -> Result<Self> {
        use oxisynth::{MidiEvent, SoundFont, Synth, SynthDescriptor};

        // 0.2 is oxisynth's (and FluidSynth's) default for a reason: a voice
        // peaks around -6 dBFS on its own, so anything above ~0.3 clips as soon
        // as a chord is held — measured at 1.0 a four-note chord already hit
        // 1.15 and a two-handed one 2.7. Loudness belongs to the slot's VOL,
        // which has a fader; clipping inside the synth has no way back.
        // **Polyphony is a CPU budget, not a musical limit.** oxisynth's
        // default is 256 voices, and a SoundFont preset commonly layers two or
        // three per key — so a hand on the keyboard with the sustain pedal down
        // walks the voice pool up until it is full and *keeps it there*, since
        // nothing is released. Measured with the pedal down and a chord a
        // second: a block cost 233 µs a minute in and 330 µs once the pool had
        // filled, a 40 % rise on one tab alone, which is where "it saturates
        // when I press the sustain" comes from — those are dropouts, not
        // clipping. Sixty-four still covers both hands sustained on a layered
        // preset (three layers × twenty-one held notes) and bounds what the
        // pedal can cost.
        const POLYPHONY: u16 = 64;
        let desc = SynthDescriptor {
            sample_rate: sample_rate as f32,
            gain: 0.2,
            polyphony: POLYPHONY,
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
            split: [None; choz_ports::SPLIT_OCTAVES],
            zone_set: [false; ZONES],
            buf_l: vec![0.0; SF2_MAX_FRAMES],
            buf_r: vec![0.0; SF2_MAX_FRAMES],
        })
    }
}

/// The parameters an SF2 slot shows in the instrument editor.
///
/// Two switches and eleven knobs. The switches came first: oxisynth runs a
/// reverb **and** a chorus of its own, on by default, fed by each preset's send
/// amounts, and stacked under choz's FX chain that is two reverbs and a chorus
/// nobody asked for.
///
/// The rest is the SoundFont editor — the envelope, the filter, the tuning and
/// the output, from [`crate::sf2_patch::EDITS`]. They are ordinary slot
/// parameters on purpose: that is what makes them movable by mouse, by arrow
/// key and by a learned CC, and what puts them in the project file, without a
/// second copy of any of that machinery.
pub fn sf2_params() -> Vec<choz_ports::PluginParam> {
    use crate::sf2_patch::{EDITS, NEUTRAL};
    let sends = ["SF2 Reverb", "SF2 Chorus"]
        .iter()
        .map(|name| choz_ports::PluginParam {
            name: (*name).to_string(),
            min: 0.0,
            max: 1.0,
            default: 1.0,
            steps: 2,
            group: Some("SENDS".to_string()),
            ..Default::default()
        });
    let edits = EDITS.iter().map(|e| choz_ports::PluginParam {
        name: e.name.to_string(),
        min: 0.0,
        max: 1.0,
        // The middle is the SoundFont as written; a knob has to start there or
        // loading a font would change how it sounds.
        default: NEUTRAL as f64,
        unit: e.unit.map(str::to_string),
        group: Some(e.group.to_string()),
        ..Default::default()
    });
    sends
        .chain(edits)
        .enumerate()
        .map(|(i, p)| choz_ports::PluginParam { id: i as u32, ..p })
        .collect()
}

/// An SF2 generator number as oxisynth names it.
///
/// Only the ones [`crate::sf2_patch::EDITS`] uses: the numbering is the
/// specification's and shared, but oxisynth's enum is not `repr`-convertible
/// from a `u16`, and a wrong transmute here would move the wrong generator.
fn sf2_generator(gen: u16) -> Option<oxisynth::GeneratorType> {
    use oxisynth::GeneratorType as G;
    Some(match gen {
        8 => G::FilterFc,
        9 => G::FilterQ,
        17 => G::Pan,
        34 => G::VolEnvAttack,
        35 => G::VolEnvHold,
        36 => G::VolEnvDecay,
        37 => G::VolEnvSustain,
        38 => G::VolEnvRelease,
        48 => G::Attenuation,
        51 => G::CoarseTune,
        52 => G::FineTune,
        _ => return None,
    })
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
            channel: self.channel_for(note),
            key: note,
            vel: velocity,
        });
    }

    fn note_off(&mut self, note: u8) {
        // **The same channel the note went to.** The split can be re-drawn
        // while a key is held, so this is the one place it must not be
        // consulted again — but a note-off on the wrong channel is a hung note,
        // and sending it to every channel costs nothing and cannot miss.
        for channel in 0..=ZONES as u8 {
            let _ = self
                .synth
                .send_event(oxisynth::MidiEvent::NoteOff { channel, key: note });
        }
    }

    fn layers_zones(&self) -> bool {
        true
    }

    fn set_zone_program(&mut self, zone: u8, bank: u8, preset: u8) {
        let Some(seen) = self.zone_set.get_mut(zone as usize % ZONES) else {
            return;
        };
        *seen = true;
        let channel = zone_channel(zone);
        // RT-safe: this looks the preset up in the already-loaded font and
        // Arc-clones it into the channel.
        if self
            .synth
            .select_program(channel, self.font_id, bank as u32, preset)
            .is_err()
        {
            let _ = self.synth.select_program(channel, self.font_id, 0, 0);
        }
        // GM channel volume, as channel 0 gets on the way in — without it the
        // zone plays at whatever the channel happened to be left at.
        let _ = self.synth.send_event(oxisynth::MidiEvent::ControlChange {
            channel,
            ctrl: 7,
            value: 100,
        });
    }

    fn set_split(&mut self, split: [Option<u8>; choz_ports::SPLIT_OCTAVES]) {
        self.split = split;
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

    /// Pedals and wheels reach every zone: the sustain pedal holds the whole
    /// keyboard, not the half of it the last note happened to be in.
    fn control_change(&mut self, cc: u8, value: u8) {
        for channel in 1..=ZONES as u8 {
            let _ = self.synth.send_event(oxisynth::MidiEvent::ControlChange {
                channel,
                ctrl: cc,
                value,
            });
        }
        let _ = self.synth.send_event(oxisynth::MidiEvent::ControlChange {
            channel: 0,
            ctrl: cc,
            value,
        });
    }

    fn pitch_bend(&mut self, value: u16) {
        for channel in 0..=ZONES as u8 {
            let _ = self.synth.send_event(oxisynth::MidiEvent::PitchBend {
                channel,
                value: value.min(16383),
            });
        }
    }

    /// `0` = the SoundFont's own reverb send, `1` = its chorus send, both as
    /// on/off; everything after them is one of [`crate::sf2_patch::EDITS`].
    ///
    /// RT-safe: `set_gen` writes a channel generator offset and re-derives the
    /// live voices from it. **Not** `set_chorus_params`, which rebuilds the
    /// chorus modulation table — 4.3 ms measured, an xrun every toggle.
    fn set_param(&mut self, index: usize, value: f32) {
        if let Some((gen, offset)) = crate::sf2_patch::offset_of(index, value) {
            if let Some(g) = sf2_generator(gen) {
                // Every zone: the editor shapes the instrument, not whichever
                // half of the keyboard is being played at the time.
                for channel in 0..=ZONES {
                    let _ = self.synth.set_gen(channel, g, offset);
                }
            }
            return;
        }
        let gen = match index {
            0 => oxisynth::GeneratorType::ReverbSend,
            1 => oxisynth::GeneratorType::ChorusSend,
            _ => return,
        };
        // The offset is additive on top of what the preset asks for, and the
        // send is clamped at 0: -1000 (=-100%) zeroes any preset, whatever it
        // set. Anything else leaves the SoundFont's own amount alone.
        let offset = if value >= 0.5 { 0.0 } else { -1000.0 };
        for channel in 0..=ZONES {
            let _ = self.synth.set_gen(channel, gen, offset);
        }
    }

    fn program_change(&mut self, bank: u8, preset: u8) {
        // RT-safe: this only looks the preset up in the already-loaded font and
        // Arc-clones it into the channel.
        let _ = self
            .synth
            .select_program(0, self.font_id, bank as u32, preset);
        // **The same GM volume every zone gets**, and for the same reason.
        //
        // Without this, channel 0 was the one channel whose volume was set once
        // at load and never again, while every zone channel had it re-asserted
        // by `set_zone_program` on each `push_split`. Since `control_change`
        // forwards an incoming CC 7 to *all* of them, a controller with a
        // volume slider left the tab's own sound stuck at whatever the slider
        // said and snapped every zone back to full — measured at **15.2 dB**
        // between the same note on the same preset, which reads as "one of my
        // sounds is quieter than the others".
        let _ = self.synth.send_event(oxisynth::MidiEvent::ControlChange {
            channel: 0,
            ctrl: 7,
            value: 100,
        });
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

    /// RMS of a rendered block, for comparing two levels rather than telling
    /// sound from silence.
    fn rms_of(s: &mut Sf2Synth, frames: usize) -> f32 {
        let mut buf = vec![0.0f32; frames * 2];
        s.render(&mut buf, 48_000);
        (buf.iter().map(|x| x * x).sum::<f32>() / buf.len() as f32).sqrt()
    }

    /// **The tab's own sound is not quieter than the ones on split zones.**
    ///
    /// Reported from a rack with four SoundFont sounds on one tab: three played
    /// at one level and the piano at another. The three were on split zones —
    /// channels 1..8 — and the piano was the tab's own program, on channel 0.
    ///
    /// The cause was a channel that nobody re-initialised. `set_zone_program`
    /// sends the GM channel volume every time `push_split` runs, which is on
    /// almost every interaction; channel 0 got it once, when the file loaded.
    /// `control_change` forwards an incoming CC to *all* the channels, so the
    /// first CC 7 from a keyboard's volume slider stuck on channel 0 and was
    /// wiped from every zone at the next push — **15.2 dB** between the same
    /// note, on the same preset, at the same velocity.
    #[test]
    fn a_volume_cc_does_not_leave_the_tabs_own_sound_behind() {
        let path = std::path::Path::new("/usr/share/sounds/sf2/FluidR3_GM.sf2");
        if !path.exists() {
            return;
        }
        // The same note and the same preset throughout: the only thing that
        // changes is which channel it lands on.
        let level = |after_cc: bool| {
            let mut s = Sf2Synth::load(path, 0, 0, 48_000).expect("load SF2");
            s.set_zone_program(0, 0, 0);
            let mut split = [None; choz_ports::SPLIT_OCTAVES];
            split[5] = Some(0);
            s.set_split(split);
            if after_cc {
                // The keyboard's volume slider, and then the interaction that
                // re-pushes both programs — a sound button, a preset pick.
                s.control_change(7, 40);
                s.set_zone_program(0, 0, 0);
                s.program_change(0, 0);
            }
            s.note_on(60, 100);
            let zone = rms_of(&mut s, 24_000);
            s.all_notes_off();
            let _ = rms_of(&mut s, 8_000);
            // The same key, with nothing pointing at a zone: channel 0.
            s.set_split([None; choz_ports::SPLIT_OCTAVES]);
            s.note_on(60, 100);
            let own = rms_of(&mut s, 24_000);
            (zone, own)
        };

        for after_cc in [false, true] {
            let (zone, own) = level(after_cc);
            assert!(zone > 1e-4 && own > 1e-4, "both have to sound");
            let db = 20.0 * (zone / own).log10();
            assert!(
                db.abs() < 1.0,
                "a zone and the tab's own sound differ by {db:+.1} dB (CC sent: {after_cc})"
            );
        }
    }

    /// **A split has to layer, not choose.**
    ///
    /// The rack used to answer a note in a split zone by changing the tab's
    /// program, so holding a bass note and playing a pad over it left one
    /// sound: whichever note landed last won, and the one already down changed
    /// timbre under the finger. A SoundFont has no reason to work that way —
    /// the file is loaded once and the engine has sixteen channels to point at
    /// different programs in it, so a zone costs a channel and no memory.
    #[test]
    fn two_split_zones_sound_together_with_different_programs() {
        let path = std::path::Path::new("/usr/share/sounds/sf2/FluidR3_GM.sf2");
        if !path.exists() {
            return;
        }
        const SR: u32 = 48_000;
        let mut s = Sf2Synth::load(path, 0, 0, SR).expect("load SF2");
        assert!(s.layers_zones(), "a SoundFont layers its zones");

        // Zone 0 is a bass in the bottom two octaves, zone 1 a pad up top.
        // Programs picked by number rather than by name: any two different GM
        // programs prove the point.
        s.set_zone_program(0, 0, 33); // Electric Bass
        s.set_zone_program(1, 0, 89); // Warm Pad
        let mut split = [None; choz_ports::SPLIT_OCTAVES];
        split[2] = Some(0);
        split[5] = Some(1);
        s.set_split(split);

        // The two zones really are two channels.
        assert_eq!(s.channel_for(2 * 12 + 4), zone_channel(0));
        assert_eq!(s.channel_for(5 * 12 + 4), zone_channel(1));
        // …and an octave with no zone on it still plays the tab's own program.
        assert_eq!(s.channel_for(7 * 12), 0);

        /// Peak of `secs` of rendering.
        fn run(s: &mut Sf2Synth, secs: f32) -> f32 {
            let mut buf = vec![0.0f32; (SR as f32 * secs) as usize * 2];
            let mut at = 0;
            while at < buf.len() {
                let end = (at + 1024).min(buf.len());
                s.render(&mut buf[at..end], SR);
                at = end;
            }
            buf.iter().fold(0.0f32, |m, v| m.max(v.abs()))
        }

        // Both hands down at once, and neither goes quiet. Measured against a
        // second synth playing only the bass over the *same* window — a note's
        // own decay makes any comparison across two windows meaningless.
        let (low, high) = (2 * 12 + 4, 5 * 12 + 4);
        let mut alone = Sf2Synth::load(path, 0, 0, SR).expect("load SF2");
        alone.set_zone_program(0, 0, 33);
        alone.set_zone_program(1, 0, 89);
        alone.set_split(split);

        for synth in [&mut s, &mut alone] {
            synth.note_on(low, 100);
        }
        run(&mut s, 0.3);
        run(&mut alone, 0.3);
        s.note_on(high, 100);
        let both = run(&mut s, 0.5);
        let one = run(&mut alone, 0.5);
        assert!(one > 0.001, "the bass zone sounds: {one}");
        assert!(
            both > one * 1.5,
            "the pad joins it rather than replacing it: {both} vs {one}"
        );

        // Letting one go leaves the other ringing.
        s.note_off(high);
        assert!(run(&mut s, 0.3) > 0.001, "the held note survived");
        s.note_off(low);

        // A zone nobody has given a program to falls back to the tab's own
        // rather than playing an empty channel and going silent.
        let mut s = Sf2Synth::load(path, 0, 0, SR).expect("load SF2");
        let mut split = [None; choz_ports::SPLIT_OCTAVES];
        split[5] = Some(3);
        s.set_split(split);
        assert_eq!(s.channel_for(5 * 12), 0, "an unset zone is not a hole");
        s.note_on(5 * 12, 100);
        assert!(run(&mut s, 0.3) > 0.001, "and it still sounds");
    }

    /// **Every envelope knob has to reach the sound, and the right way round.**
    ///
    /// The first version could not. The offsets went ±2400 timecents and ±480
    /// centibels, which is polite next to a sampled piano's own 8 ms attack and
    /// 1000 cB sustain — the knobs moved and nothing was audible, which is the
    /// worst way for a control to fail. And two of them were backwards: the SF2
    /// generators behind `Sustain` and `Volume` are *attenuations*, so turning
    /// them up made the note quieter.
    ///
    /// Measured on a real SoundFont because that is the only place the question
    /// exists: an offset is only big enough relative to what the file says.
    #[test]
    fn every_envelope_knob_changes_the_sound_in_the_direction_it_reads() {
        use crate::sf2_patch::{NEUTRAL, SENDS};
        let path = std::path::Path::new("/usr/share/sounds/sf2/FluidR3_GM.sf2");
        if !path.exists() {
            return;
        }
        const SR: u32 = 48_000;
        /// Loudest sample in the last 200 ms of what was rendered — the level a
        /// long stage *arrives* at. Peak over the whole window would be the
        /// note's own attack, which no release setting can change.
        fn tail(s: &mut Sf2Synth, secs: f32) -> f32 {
            let frames = (SR as f32 * secs) as usize;
            let mut buf = vec![0.0f32; frames * 2];
            let mut at = 0;
            while at < buf.len() {
                let end = (at + 1024).min(buf.len());
                s.render(&mut buf[at..end], SR);
                at = end;
            }
            let last = SR as usize / 5 * 2;
            buf[buf.len().saturating_sub(last)..]
                .iter()
                .fold(0.0f32, |m, v| m.max(v.abs()))
        }
        /// `(level in the first 50 ms, held for 4 s, 3 s after the note-off)`
        /// with one parameter moved off centre.
        fn probe(path: &std::path::Path, param: usize, value: f32) -> (f32, f32, f32) {
            let mut s = Sf2Synth::load(path, 0, 0, SR).expect("load SF2");
            if param != usize::MAX {
                s.set_param(param, value);
            }
            s.note_on(60, 100);
            let head = tail(&mut s, 0.05);
            let held = tail(&mut s, 4.0);
            s.note_off(60);
            (head, held, tail(&mut s, 3.0))
        }

        let (head, held, after) = probe(path, usize::MAX, NEUTRAL);
        assert!(head > 0.01, "the SoundFont makes a sound at all");

        // Attack up: the note fades in, so the first 50 ms are far quieter.
        let (slow_head, ..) = probe(path, SENDS, 1.0);
        assert!(
            slow_head < head / 4.0,
            "a full attack must be audible: {slow_head} vs {head}"
        );
        // …and down, it is at least as immediate as the file itself.
        let (fast_head, ..) = probe(path, SENDS, 0.0);
        assert!(fast_head >= head * 0.9, "{fast_head} vs {head}");

        // Sustain up holds the note higher four seconds in. It cannot hold it
        // for ever — a piano *sample* runs out, and no envelope puts back what
        // was never recorded — but it must be plainly louder than the file's.
        let (_, loud_hold, _) = probe(path, SENDS + 3, 1.0);
        assert!(
            loud_hold > held * 2.0,
            "sustain up must sustain: {loud_hold} vs {held}"
        );

        // Release up leaves something ringing three seconds after the key.
        let (.., long_tail) = probe(path, SENDS + 4, 1.0);
        assert!(
            long_tail > after,
            "release up must ring on: {long_tail} vs {after}"
        );

        // Volume: up is louder and down is quieter. The generator is an
        // attenuation, so this is the pair that catches the sign.
        let (up, ..) = probe(path, SENDS + 10, 1.0);
        let (down, ..) = probe(path, SENDS + 10, 0.0);
        assert!(
            up > head && down < head,
            "up {up}, file {head}, down {down}"
        );

        // Cutoff down takes the top off it.
        let (dark, ..) = probe(path, SENDS + 5, 0.0);
        assert!(dark < head, "cutoff down must be heard: {dark} vs {head}");
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

    /// A held chord must stay inside the converter. The synth renders into a
    /// mixer, an FX chain and a fader that all assume \u{00B1}1.0; a SoundFont
    /// that leaves no headroom is distortion nothing downstream can undo.
    #[test]
    fn a_held_chord_leaves_headroom() {
        let path = std::path::Path::new("/usr/share/sounds/sf2/FluidR3_GM.sf2");
        if !path.exists() {
            return; // ponytail: no bundled SF2 to test against, skip rather than fail.
        }
        let mut synth = Sf2Synth::load(path, 0, 0, 48_000).expect("load SF2");
        // Two hands' worth, hard.
        for i in 0..12u8 {
            synth.note_on(48 + i * 4, 110);
        }
        let mut loudest = 0.0f32;
        for _ in 0..40 {
            loudest = loudest.max(peak(&mut synth, 512));
        }
        assert!(loudest > 0.05, "the chord has to sound at all: {loudest}");
        assert!(loudest < 1.0, "a chord must not clip the synth: {loudest}");
    }

    /// The SF2 reverb / chorus switches have to actually reach the synth: off
    /// must change what comes out for a preset that sends to them, and the
    /// toggle must stay cheap enough for the audio thread.
    #[test]
    fn the_sf2_send_switches_change_the_sound() {
        let path = std::path::Path::new("/usr/share/sounds/sf2/FluidR3_GM.sf2");
        if !path.exists() {
            return; // ponytail: no bundled SF2 to test against, skip rather than fail.
        }
        let render = |off: bool| {
            let mut s = Sf2Synth::load(path, 0, 0, 48_000).expect("load SF2");
            if off {
                s.set_param(0, 0.0); // reverb send
                s.set_param(1, 0.0); // chorus send
            }
            s.note_on(60, 110);
            let mut out = vec![0.0f32; 4096];
            let mut all = Vec::new();
            for _ in 0..30 {
                s.render(&mut out, 48_000);
                all.extend_from_slice(&out);
            }
            all
        };
        let (wet, dry) = (render(false), render(true));
        let diff: f32 = wet
            .iter()
            .zip(&dry)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max);
        assert!(
            diff > 1e-4,
            "switching the sends off must be audible: {diff}"
        );

        // …and back on again is the sound we started from.
        let mut s = Sf2Synth::load(path, 0, 0, 48_000).expect("load SF2");
        s.set_param(0, 0.0);
        s.set_param(1, 0.0);
        s.set_param(0, 1.0);
        s.set_param(1, 1.0);
        s.note_on(60, 110);
        let mut out = vec![0.0f32; 4096];
        let mut back = Vec::new();
        for _ in 0..30 {
            s.render(&mut out, 48_000);
            back.extend_from_slice(&out);
        }
        assert_eq!(back, wet, "on is the SoundFont's own amount, unchanged");
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
