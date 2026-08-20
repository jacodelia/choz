//! A click, made in the audio callback.
//!
//! The tempo and the time signature are the transport's — the same ones every
//! synced plugin reads, so the click and a tempo-synced delay cannot disagree.
//! What lives here is only whether it sounds, how loud, and what it sounds
//! like.
//!
//! **It keeps its own clock.** The transport advances only while it is rolling,
//! and a metronome is for practising: it has to tick with the transport
//! stopped, which is exactly when it is most wanted. That clock is one counter
//! advanced per block from the audio thread and read nowhere else.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// How much weight a beat carries in the bar. Three levels, because two cannot
/// express what a grouping is for: 7/8 as 2+2+3 is *heard* as one downbeat and
/// two lesser accents, and a click that only knows "first beat or not" leaves
/// the player counting seven identical taps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stress {
    /// Beat one.
    Down,
    /// The first beat of a group inside the bar — the 3 in 2+2+3.
    Group,
    /// Everything else.
    Weak,
}

/// Every way `n` beats can be grouped into twos and threes, plus the ungrouped
/// bar itself first.
///
/// The compositions musicians actually write: 7 gives 2+2+3, 2+3+2 and 3+2+2,
/// and 12 gives twelve of them. Parts of 4 and 5 are deliberately not offered —
/// 5 is heard as 2+3 or 3+2 with one accent dropped, and admitting them turns
/// 12/8 from twelve choices into fifty-seven.
///
/// ponytail: recursive, and bounded by the `n > 16` guard; the count grows like
/// the Padovan sequence, so a bar longer than that is a list nobody can pick
/// from anyway.
pub fn musical_groupings(n: u8) -> Vec<Vec<u8>> {
    let n = n.max(1);
    if n > 16 {
        return vec![vec![n]];
    }
    fn compose(rem: u8, cur: &mut Vec<u8>, out: &mut Vec<Vec<u8>>) {
        if rem == 0 {
            out.push(cur.clone());
            return;
        }
        for p in [2u8, 3u8] {
            if p <= rem {
                cur.push(p);
                compose(rem - p, cur, out);
                cur.pop();
            }
        }
    }
    let mut comps = Vec::new();
    compose(n, &mut Vec::new(), &mut comps);
    comps.sort();
    comps.dedup();
    comps.retain(|g| g.as_slice() != [n]);
    let mut out = vec![vec![n]];
    out.extend(comps);
    out
}

/// What the click sounds like.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ClickStyle {
    /// A short sine pip — a studio talkback beep.
    #[default]
    Beep,
    /// A hard, clicky transient: the sound of a stick on a rim.
    Click,
    /// A woodblock: two partials and a fast decay.
    Wood,
}

impl ClickStyle {
    pub const ALL: [ClickStyle; 3] = [ClickStyle::Beep, ClickStyle::Click, ClickStyle::Wood];

    pub fn label(self) -> &'static str {
        match self {
            ClickStyle::Beep => "BEEP",
            ClickStyle::Click => "CLICK",
            ClickStyle::Wood => "WOOD",
        }
    }

    pub fn next(self) -> Self {
        match self {
            ClickStyle::Beep => ClickStyle::Click,
            ClickStyle::Click => ClickStyle::Wood,
            ClickStyle::Wood => ClickStyle::Beep,
        }
    }

    fn from_bits(b: u32) -> Self {
        *Self::ALL.get(b as usize).unwrap_or(&ClickStyle::Beep)
    }
}

/// The one metronome, like the transport and the meter: there is one output to
/// put a click into.
pub fn metronome() -> &'static Metronome {
    static M: Metronome = Metronome::new();
    &M
}

pub struct Metronome {
    on: AtomicBool,
    /// Linear, 0..1.
    gain: AtomicU32,
    style: AtomicU32,
    /// Frames since it was switched on, advanced by the audio callback.
    pos: AtomicU64,
    /// How the bar is grouped, as up to sixteen 4-bit parts, least significant
    /// first; a zero nibble ends the list. Packed into one atomic because the
    /// reader is the audio callback, and a `Vec` there is not an option.
    ///
    /// Ignored unless the parts add up to the bar — a grouping left over from
    /// 7/8 means nothing in 4/4, and the safe reading of a stale one is "no
    /// grouping at all".
    groups: AtomicU64,
    /// Where the click lands, as [`crate::engine::Dest::index`]. Its own
    /// setting because the click is the one thing in the rack that often must
    /// **not** go where the music goes: the player wants it in their wedge and
    /// the room does not want it at all.
    dest: AtomicU32,
}

impl Metronome {
    const fn new() -> Self {
        Self {
            on: AtomicBool::new(false),
            gain: AtomicU32::new(0x3f00_0000), // 0.5
            style: AtomicU32::new(0),
            pos: AtomicU64::new(0),
            groups: AtomicU64::new(0),
            dest: AtomicU32::new(0),
        }
    }

    pub fn dest(&self) -> crate::engine::Dest {
        crate::engine::Dest::from_index(self.dest.load(Ordering::Relaxed) as usize)
    }

    pub fn set_dest(&self, dest: crate::engine::Dest) {
        self.dest.store(dest.index() as u32, Ordering::Relaxed);
    }

    /// Group the bar: `&[2, 2, 3]` for a 7/8 counted 2+2+3. An empty slice, or
    /// one that does not add up to the bar, is no grouping.
    pub fn set_groups(&self, groups: &[u8]) {
        let mut packed = 0u64;
        for (i, g) in groups.iter().take(16).enumerate() {
            packed |= (u64::from(*g).min(15)) << (i * 4);
        }
        self.groups.store(packed, Ordering::Relaxed);
    }

    /// The grouping as the interface wants it. Allocates — never called from
    /// the audio thread, which reads [`Self::stress_at`] instead.
    pub fn groups(&self) -> Vec<u8> {
        let packed = self.groups.load(Ordering::Relaxed);
        (0..16)
            .map(|i| ((packed >> (i * 4)) & 0xF) as u8)
            .take_while(|g| *g > 0)
            .collect()
    }

    /// What beat `beat_in_bar` of a `beats_per_bar` bar is worth. Reads one
    /// atomic and walks at most sixteen nibbles: no allocation, no branch on
    /// anything the audio thread does not already know.
    fn stress_at(&self, beat_in_bar: u64, beats_per_bar: u64) -> Stress {
        if beat_in_bar == 0 {
            return Stress::Down;
        }
        let packed = self.groups.load(Ordering::Relaxed);
        let (mut sum, mut boundary) = (0u64, false);
        for i in 0..16 {
            let part = (packed >> (i * 4)) & 0xF;
            if part == 0 {
                break;
            }
            // The boundary is where a group *starts*, so it is the running sum
            // before the group is added — never the end of the bar.
            boundary |= sum == beat_in_bar;
            sum += part;
        }
        // A grouping that does not add up to the bar is a leftover from another
        // signature; the honest reading of it is no grouping at all.
        match sum == beats_per_bar && boundary {
            true => Stress::Group,
            false => Stress::Weak,
        }
    }

    pub fn on(&self) -> bool {
        self.on.load(Ordering::Relaxed)
    }

    /// Switching it on restarts the count, so the first beat is the beat you
    /// switched it on for rather than wherever a free-running counter had got
    /// to — the difference between counting a band in and joining it.
    pub fn set_on(&self, on: bool) {
        if on {
            self.pos.store(0, Ordering::Relaxed);
        }
        self.on.store(on, Ordering::Relaxed);
    }

    pub fn gain(&self) -> f32 {
        f32::from_bits(self.gain.load(Ordering::Relaxed))
    }

    pub fn set_gain(&self, g: f32) {
        self.gain
            .store(g.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    pub fn style(&self) -> ClickStyle {
        ClickStyle::from_bits(self.style.load(Ordering::Relaxed))
    }

    pub fn set_style(&self, s: ClickStyle) {
        let bits = ClickStyle::ALL.iter().position(|x| *x == s).unwrap_or(0);
        self.style.store(bits as u32, Ordering::Relaxed);
    }

    /// Mix `frames` of click into an interleaved stereo block. Called from the
    /// audio thread: no allocation, no locks, and it returns immediately when
    /// the metronome is off.
    pub fn render(&self, out: &mut [f32], frames: usize, sample_rate: u32) {
        if !self.on() || sample_rate == 0 {
            return;
        }
        let bpm = choz_ports::transport().bpm().clamp(20.0, 300.0);
        let (num, den) = choz_ports::transport().time_signature();
        // **The denominator names the beat.** A tempo is quarter notes, so a
        // bar of 7/8 is seven *eighths* — clicking seven quarters is a
        // different piece of music, and it was what choz did before compound
        // meters were selectable at all.
        let quarter = (sample_rate as f64 * 60.0 / bpm as f64).max(1.0);
        let beat = (quarter * 4.0 / den.max(1) as f64).max(1.0);
        let beats_per_bar = num.max(1) as u64;
        let gain = self.gain();
        let style = self.style();
        let start = self.pos.fetch_add(frames as u64, Ordering::Relaxed);

        for f in 0..frames.min(out.len() / 2) {
            let pos = (start + f as u64) as f64;
            let beat_index = (pos / beat) as u64;
            let t = (pos - beat_index as f64 * beat) / sample_rate as f64;
            // The downbeat is a different note, not a louder one: on a busy
            // stage "louder" is the first thing the room takes away. The
            // grouping's accents sit between the two, which is how 2+2+3 is
            // told apart from seven of the same tap.
            let stress = self.stress_at(beat_index % beats_per_bar, beats_per_bar);
            let s = click(style, t as f32, stress) * gain;
            out[f * 2] += s;
            out[f * 2 + 1] += s;
        }
    }
}

/// One click, `t` seconds after its beat. Silent once the envelope has run out,
/// which is what makes the per-frame call above cheap between beats.
fn click(style: ClickStyle, t: f32, stress: Stress) -> f32 {
    // A ratio, not three tables: the group accent sits between the downbeat and
    // the rest, which is exactly what it is musically.
    let lift = match stress {
        Stress::Down => 1.6,
        Stress::Group => 1.25,
        Stress::Weak => 1.0,
    };
    let (freq, decay, len) = match style {
        ClickStyle::Beep => (1000.0 * lift, 60.0, 0.06),
        ClickStyle::Click => (2000.0 * lift, 220.0, 0.02),
        ClickStyle::Wood => (900.0 * lift, 120.0, 0.04),
    };
    if t < 0.0 || t > len {
        return 0.0;
    }
    let env = (-t * decay).exp();
    let phase = std::f32::consts::TAU * freq * t;
    match style {
        ClickStyle::Beep => phase.sin() * env,
        // A transient, not a tone: the odd harmonics are what makes it read as
        // a stick rather than a pip.
        ClickStyle::Click => (phase.sin() + 0.5 * (phase * 3.0).sin()) * env * 0.7,
        ClickStyle::Wood => (phase.sin() + 0.6 * (phase * 2.76).sin()) * env * 0.7,
    }
}

impl Metronome {
    /// Put the click's own clock somewhere, for the test below. Not public
    /// beyond the crate: nothing else has any business moving it.
    #[cfg(test)]
    fn set_pos_for_test(&self, pos: u64) {
        self.pos.store(pos, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A grouping accents where the groups start, and a grouping that does not
    /// fit the bar is ignored rather than half-applied.
    #[test]
    fn a_bar_is_accented_where_its_groups_start() {
        let m = metronome();

        // 7/8 counted 2+2+3: beats 0, 2 and 4 carry weight, the rest do not.
        m.set_groups(&[2, 2, 3]);
        assert_eq!(m.groups(), vec![2, 2, 3]);
        let stress: Vec<Stress> = (0..7).map(|b| m.stress_at(b, 7)).collect();
        assert_eq!(
            stress,
            vec![
                Stress::Down,
                Stress::Weak,
                Stress::Group,
                Stress::Weak,
                Stress::Group,
                Stress::Weak,
                Stress::Weak
            ]
        );

        // The same seven beats grouped 3+2+2 accent somewhere else — which is
        // the whole reason the setting exists.
        m.set_groups(&[3, 2, 2]);
        assert_eq!(m.stress_at(3, 7), Stress::Group);
        assert_eq!(m.stress_at(2, 7), Stress::Weak);

        // Left over from another signature: it adds up to seven, not four, so
        // 4/4 is counted plainly instead of accented on nonsense.
        assert!((1..4).all(|b| m.stress_at(b, 4) == Stress::Weak));
        assert_eq!(
            m.stress_at(0, 4),
            Stress::Down,
            "beat one is always beat one"
        );

        m.set_groups(&[]);
        assert!(m.groups().is_empty());
        assert_eq!(m.stress_at(2, 7), Stress::Weak, "no grouping, no accents");
    }

    /// The compositions offered are the ones a bar is actually written in.
    #[test]
    fn the_groupings_offered_are_twos_and_threes() {
        assert_eq!(musical_groupings(4), vec![vec![4], vec![2, 2]]);
        let seven = musical_groupings(7);
        assert_eq!(seven[0], vec![7], "the ungrouped bar comes first");
        for g in [vec![2, 2, 3], vec![2, 3, 2], vec![3, 2, 2]] {
            assert!(seven.contains(&g), "7/8 must offer {g:?}");
        }
        assert_eq!(seven.len(), 4);
        // Every composition adds up, or the metronome would throw it away.
        for g in musical_groupings(12) {
            assert_eq!(g.iter().map(|p| *p as u16).sum::<u16>(), 12);
        }
        // A bar too long to group is left alone rather than exploded.
        assert_eq!(musical_groupings(17), vec![vec![17]]);
    }

    /// Off it is silence; on it puts a click on the beat and nothing between
    /// them — and the first beat of the bar is not the same sound as the rest.
    #[test]
    fn it_clicks_on_the_beat_and_only_there() {
        let m = metronome();
        let sr = 48_000u32;
        choz_ports::transport().set_bpm(120.0); // half a second a beat
        choz_ports::transport().set_time_signature(4, 4);

        let mut buf = vec![0.0f32; 64];
        m.set_on(false);
        m.render(&mut buf, 32, sr);
        assert!(buf.iter().all(|s| *s == 0.0), "off means silent");

        m.set_on(true);
        m.render(&mut buf, 32, sr);
        let downbeat = buf.iter().cloned().fold(0.0f32, |a, b| a.max(b.abs()));
        assert!(downbeat > 0.1, "the first beat sounds: {downbeat}");

        // A quarter of the way into the beat there is nothing left.
        let quiet_at = (sr / 4) as u64;
        let mut buf2 = vec![0.0f32; 64];
        m.set_pos_for_test(quiet_at);
        m.render(&mut buf2, 32, sr);
        assert!(
            buf2.iter().all(|s| s.abs() < 1e-6),
            "between beats it is silent"
        );

        // Beat two, half a second in: a click again, and a different one.
        let mut buf3 = vec![0.0f32; 64];
        m.set_pos_for_test(sr as u64 / 2);
        m.render(&mut buf3, 32, sr);
        let beat2 = buf3.iter().cloned().fold(0.0f32, |a, b| a.max(b.abs()));
        assert!(beat2 > 0.1, "beat two sounds: {beat2}");
        assert!(
            (beat2 - downbeat).abs() > 1e-6 || buf3 != buf,
            "the downbeat has to be distinguishable from the others"
        );
        m.set_on(false);
    }
}
