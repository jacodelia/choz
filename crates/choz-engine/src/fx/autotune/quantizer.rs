//! Hz → note → the note it should have been.
//!
//! The conversion is the standard one, and the same one `A→M` uses:
//! `midi = 69 + 12·log2(f / A4)` and back. The reference is a parameter because
//! not everyone tunes to 440 — early music sits at 415, some orchestras at 442.

/// Scales the quantiser can snap to, as semitone offsets from the key's root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScaleType {
    #[default]
    Chromatic,
    Major,
    Minor,
    PentatonicMajor,
    PentatonicMinor,
    Blues,
}

impl ScaleType {
    pub const ALL: [ScaleType; 6] = [
        ScaleType::Chromatic,
        ScaleType::Major,
        ScaleType::Minor,
        ScaleType::PentatonicMajor,
        ScaleType::PentatonicMinor,
        ScaleType::Blues,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ScaleType::Chromatic => "Chromatic",
            ScaleType::Major => "Major",
            ScaleType::Minor => "Minor",
            ScaleType::PentatonicMajor => "Pent Maj",
            ScaleType::PentatonicMinor => "Pent Min",
            ScaleType::Blues => "Blues",
        }
    }

    /// Semitones above the root that belong to the scale.
    pub fn intervals(self) -> &'static [u8] {
        match self {
            ScaleType::Chromatic => &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
            ScaleType::Major => &[0, 2, 4, 5, 7, 9, 11],
            ScaleType::Minor => &[0, 2, 3, 5, 7, 8, 10],
            ScaleType::PentatonicMajor => &[0, 2, 4, 7, 9],
            ScaleType::PentatonicMinor => &[0, 3, 5, 7, 10],
            ScaleType::Blues => &[0, 3, 5, 6, 7, 10],
        }
    }
}

/// Note names, sharps only — the key is a pitch class, not a spelling.
pub const NOTE_NAMES: [&str; 12] =
    ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// A key and a scale: which of the twelve pitch classes a note may land on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scale {
    /// Root pitch class, 0 = C.
    pub root: u8,
    pub kind: ScaleType,
}

impl Default for Scale {
    fn default() -> Self {
        Self { root: 0, kind: ScaleType::Chromatic }
    }
}

impl Scale {
    pub fn new(root: u8, kind: ScaleType) -> Self {
        Self { root: root % 12, kind }
    }

    /// Whether a MIDI note belongs to the scale.
    pub fn contains(&self, note: i32) -> bool {
        let pc = (note - self.root as i32).rem_euclid(12) as u8;
        self.kind.intervals().contains(&pc)
    }

    /// The nearest note in the scale to a **fractional** note number.
    ///
    /// Fractional on purpose: a singer 40 cents sharp of F in C major is nearer
    /// to F than to G, and rounding to a note first would have thrown away the
    /// only information that says so. Ties go to the lower note, which is the
    /// same choice `round` makes and keeps the answer deterministic.
    pub fn nearest(&self, note: f32) -> i32 {
        let intervals = self.kind.intervals();
        let centre = note.round() as i32;
        let mut best = centre;
        let mut best_dist = f32::MAX;
        // A scale has at most twelve members per octave, so a semitone either
        // side of the octave the note sits in is every candidate there is.
        for octave in -1..=1 {
            let base = (centre - self.root as i32).div_euclid(12) + octave;
            for &iv in intervals {
                let candidate = self.root as i32 + base * 12 + iv as i32;
                let dist = (candidate as f32 - note).abs();
                if dist < best_dist - 1e-6 {
                    best_dist = dist;
                    best = candidate;
                }
            }
        }
        best
    }
}

/// Where the correction is aiming.
///
/// `MidiNote` is not wired to anything yet — there is no MIDI routing into an
/// FX in choz — but the quantiser takes it today so that adding the routing is
/// a change in one place rather than a change to the shape of this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PitchTarget {
    #[default]
    AutomaticScale,
    MidiNote(u8),
}

/// Hz → the note it should be, and that note's frequency.
#[derive(Debug, Clone, Copy)]
pub struct NoteQuantizer {
    pub scale: Scale,
    /// Reference pitch for A4, in Hz.
    pub reference_hz: f32,
    pub target: PitchTarget,
}

impl Default for NoteQuantizer {
    fn default() -> Self {
        Self { scale: Scale::default(), reference_hz: 440.0, target: PitchTarget::default() }
    }
}

impl NoteQuantizer {
    /// `midi = 69 + 12·log2(f / A4)`, fraction and all.
    pub fn hz_to_note(&self, hz: f32) -> f32 {
        if !hz.is_finite() || hz <= 0.0 {
            return 0.0;
        }
        69.0 + 12.0 * (hz / self.reference_hz).log2()
    }

    /// `f = A4·2^((midi − 69)/12)`.
    pub fn note_to_hz(&self, note: f32) -> f32 {
        self.reference_hz * (2.0f32).powf((note - 69.0) / 12.0)
    }

    /// The frequency the input should be corrected towards, or `None` when
    /// there is nothing to aim at.
    pub fn target_hz(&self, detected_hz: f32) -> Option<f32> {
        if !detected_hz.is_finite() || detected_hz <= 0.0 {
            return None;
        }
        let note = match self.target {
            PitchTarget::MidiNote(n) => n as f32,
            PitchTarget::AutomaticScale => self.scale.nearest(self.hz_to_note(detected_hz)) as f32,
        };
        let hz = self.note_to_hz(note);
        hz.is_finite().then_some(hz)
    }
}
