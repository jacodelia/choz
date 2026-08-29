//! Two instances of one plugin, so a tab can be split between two sounds.
//!
//! A SoundFont splits for free: oxisynth has sixteen MIDI channels and one
//! loaded file, so a zone is a channel with its own program and the audio is
//! one synth's. A hosted plugin has **one** patch, and until now a split on a
//! plugin tab meant the rack swapped the patch as the keyboard crossed the
//! join — which is a switch, not a split: the note being held stopped.
//!
//! What makes it a split is two of them sounding at once, which means two
//! instances. That is the whole idea here, and the ceiling is **two**, on
//! purpose: a plugin costs what it costs, and somebody who wants four sounds at
//! once is asking for four tabs, which is what MULTI is for.
//!
//! # Real-time
//!
//! `render` mixes the second voice through a scratch buffer sized at
//! construction. Nothing here allocates once it is built.

use choz_ports::{AudioSource, SPLIT_OCTAVES};

/// How many zones one tab can layer. Two instruments, two sounds, one keyboard.
pub const MAX_ZONES: usize = 2;

pub struct Layered {
    voices: [Box<dyn AudioSource>; MAX_ZONES],
    /// Which zone each octave plays. `None` — and anything past the ceiling —
    /// is the first voice, which is the tab's own sound.
    split: [Option<u8>; SPLIT_OCTAVES],
    /// Where the second voice writes before being summed in.
    scratch: Vec<f32>,
}

impl Layered {
    /// `voices[0]` is the tab's instrument and the one every unassigned octave
    /// plays; `voices[1]` is the second zone.
    pub fn new(voices: [Box<dyn AudioSource>; MAX_ZONES], max_block: u32) -> Self {
        Self {
            voices,
            split: [None; SPLIT_OCTAVES],
            scratch: vec![0.0; max_block as usize * 2],
        }
    }

    /// The voice an octave plays.
    ///
    /// A zone past the ceiling plays the **first** voice — the tab's own sound
    /// — rather than the last one. A project can carry a split of four zones,
    /// painted on a SoundFont where four is free; opening it on a plugin, the
    /// honest reading of "there is no third instance" is that those octaves are
    /// not assigned, not that they belong to the second sound.
    fn zone_of(&self, note: u8) -> usize {
        self.split
            .get(note as usize / 12)
            .copied()
            .flatten()
            .map(|z| z as usize)
            .filter(|z| *z < MAX_ZONES)
            .unwrap_or(0)
    }

    /// The instance a zone is, for whoever has to reach into it — restoring the
    /// patch its button holds, mostly.
    pub fn voice(&self, zone: usize) -> Option<&dyn AudioSource> {
        self.voices.get(zone).map(|v| v.as_ref())
    }

    pub fn voice_mut(&mut self, zone: usize) -> Option<&mut Box<dyn AudioSource>> {
        self.voices.get_mut(zone)
    }
}

impl AudioSource for Layered {
    fn render(&mut self, out: &mut [f32], sample_rate: u32) -> usize {
        let wrote = self.voices[0].render(out, sample_rate);
        let n = out.len().min(self.scratch.len());
        let second = &mut self.scratch[..n];
        second.fill(0.0);
        let also = self.voices[1].render(second, sample_rate);
        for (o, s) in out.iter_mut().zip(second.iter()) {
            *o += *s;
        }
        // Whichever went further: a voice that finished does not end the tab
        // while the other one is still sounding.
        wrote.max(also)
    }

    fn note_on(&mut self, note: u8, velocity: u8) {
        let zone = self.zone_of(note);
        self.voices[zone].note_on(note, velocity);
    }

    fn note_off(&mut self, note: u8) {
        // **Both of them**, and not the one the split says now: the split can be
        // re-drawn while a key is held, and a note-off that goes to the wrong
        // instance is a note that never stops. A note-off for a note a plugin
        // is not playing is something every synth ignores.
        for v in self.voices.iter_mut() {
            v.note_off(note);
        }
    }

    /// Everything that is not a note goes to both: a pedal, a wheel or a bend
    /// belongs to the keyboard, not to a zone of it.
    fn control_change(&mut self, cc: u8, value: u8) {
        for v in self.voices.iter_mut() {
            v.control_change(cc, value);
        }
    }

    fn pitch_bend(&mut self, value: u16) {
        for v in self.voices.iter_mut() {
            v.pitch_bend(value);
        }
    }

    fn all_notes_off(&mut self) {
        for v in self.voices.iter_mut() {
            v.all_notes_off();
        }
    }

    fn program_change(&mut self, bank: u8, preset: u8) {
        self.voices[0].program_change(bank, preset);
    }

    fn layers_zones(&self) -> bool {
        true
    }

    fn set_split(&mut self, split: [Option<u8>; SPLIT_OCTAVES]) {
        self.split = split;
    }

    /// A zone's sound is a **patch**, not a program number: a plugin's is an
    /// opaque blob and the rack keeps it on the sound button. So this is the
    /// SoundFont's door and the first voice is the only one behind it — what
    /// puts a patch in the second zone is [`Layered::voice_mut`] and the state
    /// handle it hands over.
    fn set_zone_program(&mut self, zone: u8, bank: u8, preset: u8) {
        if let Some(v) = self.voices.get_mut(zone as usize) {
            v.program_change(bank, preset);
        }
    }

    /// The knobs belong to the tab, and the tab's instrument is the first
    /// voice. A second set of knobs for the second zone is a panel that does
    /// not exist — see the roadmap.
    fn set_param(&mut self, index: usize, value: f32) {
        self.voices[0].set_param(index, value);
    }

    fn plays_on_transport_stop(&self) -> bool {
        self.voices.iter().any(|v| v.plays_on_transport_stop())
    }

    /// Everything a panel reaches for is the first voice's: its window, its
    /// parameters, its patches. The second one is a copy of the same plugin
    /// carrying another patch, and a rack that offered two of each would be
    /// asking which one every time.
    fn editor(&self) -> Option<choz_ports::EditorHandle> {
        self.voices[0].editor()
    }

    fn param_touch(&self) -> Option<choz_ports::TouchHandle> {
        self.voices[0].param_touch()
    }

    fn state(&self) -> Option<choz_ports::StateHandle> {
        self.voices[0].state()
    }

    /// Each instance's own patch, which is what a zone's sound *is* for a
    /// hosted plugin. Zone 0 is the same handle [`AudioSource::state`] hands
    /// over — the tab's patch and the first zone's are one thing.
    fn zone_state(&self, zone: u8) -> Option<choz_ports::StateHandle> {
        self.voices.get(zone as usize)?.state()
    }

    fn presets(&self) -> Option<choz_ports::PresetsHandle> {
        self.voices[0].presets()
    }

    fn paths(&self) -> Option<choz_ports::PathsHandle> {
        self.voices[0].paths()
    }

    fn sandbox(&self) -> Option<choz_ports::SandboxStatus> {
        self.voices[0].sandbox()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A voice that says which one it is: a constant, and a note log.
    struct Spy {
        level: f32,
        on: std::sync::Arc<parking_lot::Mutex<Vec<(bool, u8)>>>,
    }

    impl AudioSource for Spy {
        fn render(&mut self, out: &mut [f32], _sr: u32) -> usize {
            out.fill(self.level);
            out.len() / 2
        }
        fn note_on(&mut self, note: u8, _v: u8) {
            self.on.lock().push((true, note));
        }
        fn note_off(&mut self, note: u8) {
            self.on.lock().push((false, note));
        }
    }

    type Log = std::sync::Arc<parking_lot::Mutex<Vec<(bool, u8)>>>;

    fn deck() -> (Layered, Log, Log) {
        let (a, b): (Log, Log) = Default::default();
        let voices: [Box<dyn AudioSource>; 2] = [
            Box::new(Spy {
                level: 0.25,
                on: a.clone(),
            }),
            Box::new(Spy {
                level: 0.5,
                on: b.clone(),
            }),
        ];
        (Layered::new(voices, 64), a, b)
    }

    /// The two of them sound **at once**, which is the whole difference from
    /// swapping a patch at the join.
    #[test]
    fn both_voices_are_heard_together() {
        let (mut l, _, _) = deck();
        let mut buf = vec![0.0f32; 32];
        let frames = l.render(&mut buf, 48_000);
        assert_eq!(frames, 16);
        assert!(
            buf.iter().all(|s| (s - 0.75).abs() < 1e-6),
            "0.25 and 0.5 sounding together is 0.75: {:?}",
            &buf[..4]
        );
    }

    /// A note goes to the zone its octave is painted with, and to no other.
    #[test]
    fn a_note_plays_the_zone_its_octave_is_painted_with() {
        let (mut l, first, second) = deck();
        let mut split = [None; SPLIT_OCTAVES];
        split[5] = Some(1); // C5..B5 on the second sound
        l.set_split(split);

        l.note_on(60, 100); // octave 5
        l.note_on(48, 100); // octave 4, unpainted
        assert_eq!(*first.lock(), vec![(true, 48)]);
        assert_eq!(*second.lock(), vec![(true, 60)]);
    }

    /// **The note-off reaches both.** The split can be re-drawn with a key
    /// held; sending it only where the split says *now* is how a note is left
    /// sounding forever.
    #[test]
    fn a_note_off_goes_to_both_however_the_split_moved() {
        let (mut l, first, second) = deck();
        let mut split = [None; SPLIT_OCTAVES];
        split[5] = Some(1);
        l.set_split(split);
        l.note_on(60, 100);
        // The player repaints the octave while the key is down.
        l.set_split([None; SPLIT_OCTAVES]);
        l.note_off(60);
        assert!(first.lock().contains(&(false, 60)));
        assert!(second.lock().contains(&(false, 60)));
    }

    /// A zone past the ceiling plays the tab's own sound.
    ///
    /// A project written on a SoundFont can carry four zones — free there —
    /// and be opened on a plugin, where there are two instances. Those octaves
    /// read as unassigned rather than as belonging to the second sound.
    #[test]
    fn a_zone_past_the_ceiling_plays_the_tabs_own_sound() {
        let (mut l, first, second) = deck();
        let mut split = [None; SPLIT_OCTAVES];
        split[3] = Some(2);
        l.set_split(split);
        l.note_on(36, 100);
        assert_eq!(*first.lock(), vec![(true, 36)], "the tab's own sound");
        assert_eq!(second.lock().len(), 0, "and not the second zone's");
    }
}
