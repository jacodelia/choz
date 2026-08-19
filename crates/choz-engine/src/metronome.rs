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
}

impl Metronome {
    const fn new() -> Self {
        Self {
            on: AtomicBool::new(false),
            gain: AtomicU32::new(0x3f00_0000), // 0.5
            style: AtomicU32::new(0),
            pos: AtomicU64::new(0),
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
        self.gain.store(g.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
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
        let (num, _) = choz_ports::transport().time_signature();
        let beat = (sample_rate as f64 * 60.0 / bpm as f64).max(1.0);
        let beats_per_bar = num.max(1) as u64;
        let gain = self.gain();
        let style = self.style();
        let start = self.pos.fetch_add(frames as u64, Ordering::Relaxed);

        for f in 0..frames.min(out.len() / 2) {
            let pos = (start + f as u64) as f64;
            let beat_index = (pos / beat) as u64;
            let t = (pos - beat_index as f64 * beat) / sample_rate as f64;
            // The downbeat is a different note, not a louder one: on a busy
            // stage "louder" is the first thing the room takes away.
            let accent = beat_index.is_multiple_of(beats_per_bar);
            let s = click(style, t as f32, accent) * gain;
            out[f * 2] += s;
            out[f * 2 + 1] += s;
        }
    }
}

/// One click, `t` seconds after its beat. Silent once the envelope has run out,
/// which is what makes the per-frame call above cheap between beats.
fn click(style: ClickStyle, t: f32, accent: bool) -> f32 {
    let (freq, decay, len) = match style {
        ClickStyle::Beep => (if accent { 1600.0 } else { 1000.0 }, 60.0, 0.06),
        ClickStyle::Click => (if accent { 3000.0 } else { 2000.0 }, 220.0, 0.02),
        ClickStyle::Wood => (if accent { 1400.0 } else { 900.0 }, 120.0, 0.04),
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
