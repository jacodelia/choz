//! The arranger and Standard MIDI Files, both ways.
//!
//! **Out**: the part the band plays, as a type-1 file a DAW opens — a
//! conductor track with the tempo, the bar and a marker at every chord change,
//! then a track a musician on the channel a GM player expects (the kit on 10).
//! The chart itself travels in the conductor as a text event, so a file choz
//! wrote opens back as exactly the chart that wrote it.
//!
//! **In**: that text when it is there; otherwise the chords are *heard* off the
//! notes, a bar at a time — see [`chart_of_notes`].

use super::chord::{self, pitch_class_name_flat};
use super::generate::Role;
use super::Arranger;
use anyhow::{bail, Result};

/// Ticks a quarter note. 480 is what every DAW writes, and it divides into
/// every swing and every tuplet the generators play.
const PPQ: u16 = 480;

/// What the conductor's text event starts with when it carries a chart.
const CHART_TAG: &str = "choz chord chart\n";

/// The channel a role plays on: GM's kit on 10, the others in `Role::ALL`
/// order.
fn channel(role: Role) -> u8 {
    match role {
        Role::Drums => 9,
        Role::Bass => 0,
        Role::Piano => 1,
        Role::Guitar => 2,
    }
}

/// The average velocity each musician is brought to in the file.
///
/// **A tab is not a mixer.** Inside choz the whole band plays one instrument,
/// so the generators keep the comping quiet under the bass and the kit: a
/// shuffle's piano averages under 20. On a GM player every musician has a
/// channel of their own, and at 20 the piano is simply not there. So each part
/// is scaled to a level of its own, accents kept — the chord players a little
/// under the rest, because three to six notes at once already sound as more.
fn target_velocity(role: Role) -> f64 {
    match role {
        Role::Drums => 96.0,
        Role::Bass => 92.0,
        Role::Piano => 80.0,
        Role::Guitar => 74.0,
    }
}

/// Channel volume (CC 7) at a fader of 1.0: GM's own default, so a file at
/// full faders sounds the way a player expects it to.
const FULL_VOLUME: f32 = 100.0;

fn vlq(mut n: u32, out: &mut Vec<u8>) {
    let mut bytes = vec![(n & 0x7F) as u8];
    n >>= 7;
    while n > 0 {
        bytes.push((n & 0x7F) as u8 | 0x80);
        n >>= 7;
    }
    out.extend(bytes.iter().rev());
}

/// One track: `(tick, bytes)` events, sorted and delta-timed.
fn track(mut events: Vec<(u32, Vec<u8>)>) -> Vec<u8> {
    // A stable sort, and the note-offs were pushed first: a note ending on the
    // tick another starts must let go before it is struck again.
    events.sort_by_key(|(t, _)| *t);
    let mut body = Vec::new();
    let mut now = 0;
    for (t, bytes) in events {
        vlq(t - now, &mut body);
        body.extend(bytes);
        now = t;
    }
    body.extend([0x00, 0xFF, 0x2F, 0x00]);
    let mut out = b"MTrk".to_vec();
    out.extend((body.len() as u32).to_be_bytes());
    out.extend(body);
    out
}

fn meta(kind: u8, data: &[u8]) -> Vec<u8> {
    let mut out = vec![0xFF, kind];
    vlq(data.len() as u32, &mut out);
    out.extend(data);
    out
}

fn ticks(beats: f64) -> u32 {
    (beats.max(0.0) * PPQ as f64).round() as u32
}

/// The arranger's part as a type-1 Standard MIDI File, one pass of the form.
pub fn write(arr: &Arranger, bpm: f32) -> Vec<u8> {
    let prog = chord::parse_progression(&arr.settings.text).ok();
    // The first bar's signature: in a chart that changes meter that is not
    // necessarily the one the playhead is in.
    let (num, den) = match prog.as_ref().filter(|p| p.heterometric()) {
        Some(p) => p.bars[0].meter.unwrap_or_else(|| arr.default_meter()),
        None => arr.meter(),
    };
    let bpb = arr.view().beats_per_bar;
    let mut conductor = vec![
        (0, meta(0x03, b"choz arranger")),
        (
            0,
            meta(0x51, &(60_000_000 / bpm.max(1.0) as u32).to_be_bytes()[1..]),
        ),
        (
            0,
            meta(
                0x58,
                &[num.min(255) as u8, den.max(1).trailing_zeros() as u8, 24, 8],
            ),
        ),
        (
            0,
            meta(0x01, format!("{CHART_TAG}{}", arr.settings.text).as_bytes()),
        ),
    ];
    if let Some(prog) = prog {
        for (at, symbol) in prog.changes(bpb) {
            conductor.push((ticks(at), meta(0x06, symbol.as_bytes())));
        }
        // A chart that changes meter says so where it does, the way a score
        // does: a DAW's grid then has the bars the band played.
        for i in 1..prog.bars.len() {
            let (was, now) = (prog.bars[i - 1].meter, prog.bars[i].meter);
            if now != was {
                let (num, den) = now.unwrap_or_else(|| arr.default_meter());
                conductor.push((
                    ticks(prog.bar_start(i, bpb)),
                    meta(
                        0x58,
                        &[num.min(255) as u8, den.max(1).trailing_zeros() as u8, 24, 8],
                    ),
                ));
            }
        }
    }
    let mut tracks = vec![track(conductor)];
    let band = arr.settings.band();
    for role in Role::ALL {
        let notes: Vec<_> = arr.part().iter().filter(|n| n.role == role).collect();
        if notes.is_empty() {
            continue;
        }
        let ch = channel(role);
        // The velocities are the playing, the fader is the mix: the part is
        // brought to its level here, and how loud the player set it goes on
        // the channel's volume, where a DAW's mixer can find it.
        let mean = notes.iter().map(|n| n.vel as f64).sum::<f64>() / notes.len() as f64;
        let scale = target_velocity(role) / mean.max(1.0);
        let gain = band
            .iter()
            .find(|(r, _)| *r == role)
            .map(|(_, g)| *g)
            .unwrap_or(1.0);
        let mut events = vec![
            (0, meta(0x03, role.name().as_bytes())),
            (
                0,
                vec![
                    0xB0 | ch,
                    7,
                    (FULL_VOLUME * gain).round().clamp(0.0, 127.0) as u8,
                ],
            ),
        ];
        if let Some(program) = arr.style().programs(role).first() {
            events.push((0, vec![0xC0 | ch, *program & 0x7F]));
        }
        for n in notes {
            let on = ticks(n.start);
            let off = ticks(n.start + n.len).max(on + 1);
            let vel = (n.vel as f64 * scale).round().clamp(1.0, 127.0) as u8;
            events.push((off, vec![0x80 | ch, n.note & 0x7F, 0]));
            events.push((on, vec![0x90 | ch, n.note & 0x7F, vel]));
        }
        tracks.push(track(events));
    }
    let mut out = b"MThd".to_vec();
    out.extend(6u32.to_be_bytes());
    out.extend(1u16.to_be_bytes());
    out.extend((tracks.len() as u16).to_be_bytes());
    out.extend(PPQ.to_be_bytes());
    for t in tracks {
        out.extend(t);
    }
    out
}

/// A note read out of a file, in quarter-note beats.
#[derive(Debug, Clone, Copy)]
struct Heard {
    key: u8,
    channel: u8,
    /// What the balance test reads; hearing chords needs only durations.
    #[cfg_attr(not(test), allow(dead_code))]
    vel: u8,
    start: f64,
    end: f64,
}

/// What a file says, as far as a chart is concerned.
struct Read {
    notes: Vec<Heard>,
    /// `(channel, CC 7)` in the order they came.
    volumes: Vec<(u8, u8)>,
    meter: (u16, u16),
    chart: Option<String>,
}

fn read(bytes: &[u8]) -> Result<Read> {
    let mut at = 0usize;
    let take = |n: usize, at: &mut usize| -> Result<&[u8]> {
        let Some(s) = bytes.get(*at..*at + n) else {
            bail!("the file ends in the middle of itself");
        };
        *at += n;
        Ok(s)
    };
    if take(4, &mut at)? != b"MThd" {
        bail!("not a MIDI file");
    }
    let len = u32::from_be_bytes(take(4, &mut at)?.try_into()?) as usize;
    let head = take(len, &mut at)?;
    if head.len() < 6 {
        bail!("a MIDI header too short to read");
    }
    let tracks = u16::from_be_bytes([head[2], head[3]]);
    let division = u16::from_be_bytes([head[4], head[5]]);
    if division & 0x8000 != 0 || division == 0 {
        bail!("a MIDI file timed in SMPTE frames has no bars to read");
    }
    let ppq = division as f64;
    let mut out = Read {
        notes: Vec::new(),
        volumes: Vec::new(),
        meter: (4, 4),
        chart: None,
    };
    let mut meter_seen = false;
    for _ in 0..tracks {
        if at + 8 > bytes.len() {
            break;
        }
        let id = take(4, &mut at)?.to_vec();
        let len = u32::from_be_bytes(take(4, &mut at)?.try_into()?) as usize;
        let body = take(len.min(bytes.len() - at), &mut at)?;
        if id != b"MTrk" {
            continue;
        }
        read_track(body, ppq, &mut out, &mut meter_seen)?;
    }
    Ok(out)
}

fn read_vlq(body: &[u8], i: &mut usize) -> Result<u32> {
    let mut n = 0u32;
    for _ in 0..4 {
        let Some(b) = body.get(*i) else {
            bail!("a track ends in the middle of a number");
        };
        *i += 1;
        n = (n << 7) | (b & 0x7F) as u32;
        if b & 0x80 == 0 {
            return Ok(n);
        }
    }
    bail!("a number longer than MIDI allows");
}

fn read_track(body: &[u8], ppq: f64, out: &mut Read, meter_seen: &mut bool) -> Result<()> {
    let mut i = 0usize;
    let mut tick = 0u64;
    let mut status = 0u8;
    // What is held, by channel and key: when it was struck, and how hard.
    let mut held: std::collections::HashMap<(u8, u8), Vec<(u64, u8)>> = Default::default();
    let beat = |t: u64| t as f64 / ppq;
    while i < body.len() {
        tick += read_vlq(body, &mut i)? as u64;
        let Some(&first) = body.get(i) else { break };
        if first & 0x80 != 0 {
            status = first;
            i += 1;
        } else if status == 0 {
            bail!("a data byte with no status before it");
        }
        match status {
            0xFF => {
                let kind = *body.get(i).unwrap_or(&0);
                i += 1;
                let len = read_vlq(body, &mut i)? as usize;
                let data = body.get(i..i + len).unwrap_or_default();
                i += len;
                match kind {
                    0x58 if data.len() >= 2 && !*meter_seen => {
                        out.meter = (data[0].max(1) as u16, 1u16 << data[1].min(4));
                        *meter_seen = true;
                    }
                    0x01 => {
                        if let Some(chart) = std::str::from_utf8(data)
                            .ok()
                            .and_then(|t| t.strip_prefix(CHART_TAG))
                        {
                            out.chart = Some(chart.to_string());
                        }
                    }
                    0x2F => break,
                    _ => {}
                }
                // A meta event cancels running status.
                status = 0;
            }
            0xF0 | 0xF7 => {
                let len = read_vlq(body, &mut i)? as usize;
                i += len;
                status = 0;
            }
            s => {
                let n = match s & 0xF0 {
                    0xC0 | 0xD0 => 1,
                    _ => 2,
                };
                let Some(data) = body.get(i..i + n) else {
                    break;
                };
                i += n;
                let ch = s & 0x0F;
                match (s & 0xF0, data) {
                    (0x90, [key, vel]) if *vel > 0 => {
                        held.entry((ch, *key)).or_default().push((tick, *vel))
                    }
                    (0x80 | 0x90, [key, _]) => {
                        if let Some((start, vel)) = held.get_mut(&(ch, *key)).and_then(|v| v.pop())
                        {
                            out.notes.push(Heard {
                                key: *key,
                                channel: ch,
                                vel,
                                start: beat(start),
                                end: beat(tick),
                            });
                        }
                    }
                    (0xB0, [7, value]) => out.volumes.push((ch, *value)),
                    _ => {}
                }
            }
        }
    }
    Ok(())
}

/// The qualities a chord is heard as, simplest first: at a tie the triad wins
/// over the seventh that needs a note nobody played.
const QUALITIES: [&str; 12] = [
    "", "m", "7", "maj7", "m7", "sus4", "sus2", "6", "m6", "m7b5", "dim", "aug",
];

/// Below this a note is the bass's: what the root of the chord is heard from.
const BASS_BELOW: u8 = 52;

/// The chord that best explains `weights` — how long each pitch class sounds
/// — with `low` the same for the bass register, and how well it does.
fn best_chord(weights: &[f64; 12], low: &[f64; 12]) -> Option<(String, f64)> {
    let total: f64 = weights.iter().sum();
    let low_total: f64 = low.iter().sum();
    if total <= 0.0 {
        return None;
    }
    let mut best: Option<(String, f64)> = None;
    for root in 0..12u8 {
        for q in QUALITIES {
            let symbol = format!("{}{q}", pitch_class_name_flat(root));
            let Ok(chord) = chord::parse(&symbol, None) else {
                continue;
            };
            let pcs = chord.pitch_classes();
            let inside: f64 = pcs.iter().map(|pc| weights[*pc as usize]).sum();
            let missing = pcs
                .iter()
                .filter(|pc| weights[**pc as usize] <= 0.0)
                .count();
            // A tone past the triad has to be *played*, not passed through:
            // a walking bass touches the sixth of every major bar.
            let extra = pcs.len().saturating_sub(3) as f64;
            let mut score = (2.0 * inside - total) / total - 0.12 * missing as f64 - 0.12 * extra;
            if low_total > 0.0 {
                score += 0.3 * low[root as usize] / low_total;
            }
            if best.as_ref().is_none_or(|(_, s)| score > *s + 1e-9) {
                best = Some((symbol, score));
            }
        }
    }
    best
}

/// How long each pitch class sounds between `from` and `to`: everywhere, and
/// in the bass register.
fn listen(notes: &[Heard], from: f64, to: f64) -> ([f64; 12], [f64; 12]) {
    let (mut w, mut low) = ([0.0; 12], [0.0; 12]);
    for n in notes {
        let d = n.end.min(to) - n.start.max(from);
        if d > 1e-9 {
            w[(n.key % 12) as usize] += d;
            if n.key < BASS_BELOW {
                low[(n.key % 12) as usize] += d;
            }
        }
    }
    (w, low)
}

/// A chart heard off the notes: one chord a bar, or two when the halves of the
/// bar are plainly different chords. The kit (channel 10) is not harmony.
fn chart_of_notes(notes: &[Heard], meter: (u16, u16)) -> Result<String> {
    let notes: Vec<Heard> = notes.iter().copied().filter(|n| n.channel != 9).collect();
    let end = notes.iter().map(|n| n.end).fold(0.0, f64::max);
    if end <= 0.0 {
        bail!("no notes in the file to hear chords in");
    }
    let bpb = (meter.0 as f64 * 4.0 / meter.1 as f64).max(0.25);
    let bars = (end / bpb).ceil() as usize;
    let mut cells: Vec<String> = Vec::new();
    let mut last: Option<String> = None;
    for b in 0..bars {
        let from = b as f64 * bpb;
        let mid = from + bpb / 2.0;
        let to = from + bpb;
        let (w, bass) = listen(&notes, from, to);
        let Some((whole, score)) = best_chord(&w, &bass) else {
            // A bar of silence holds what came before; before anything, there
            // is nothing to hold and no bar to write.
            if last.is_some() {
                cells.push("%".into());
            }
            continue;
        };
        let halves = {
            let (w1, b1) = listen(&notes, from, mid);
            let (w2, b2) = listen(&notes, mid, to);
            best_chord(&w1, &b1).zip(best_chord(&w2, &b2))
        };
        let cell = match halves {
            // ponytail: a fixed margin over the whole bar; a melody full of
            // passing notes can still split a bar that is one chord.
            Some(((a, sa), (c, sc))) if a != c && (sa + sc) / 2.0 > score + 0.15 => {
                last = Some(c.clone());
                format!("{a} {c}")
            }
            _ => {
                last = Some(whole.clone());
                whole
            }
        };
        cells.push(cell);
    }
    let mut text = String::new();
    if meter != (4, 4) {
        text.push_str(&format!("meter = {}/{}\n", meter.0, meter.1));
    }
    for row in cells.chunks(4) {
        text.push_str(&format!("| {} |\n", row.join(" | ")));
    }
    Ok(text)
}

/// How many musicians a file may have for the arranger to read it: its own
/// band is four, and one instrument is a melody or a loop, not a backing.
pub const INSTRUMENTS: std::ops::RangeInclusive<usize> = 2..=4;

/// A MIDI file as a chart the arranger reads: the chart choz wrote into it, or
/// the chords heard off its notes.
///
/// **Only a band's worth of instruments** — a channel with notes on it is one,
/// the kit on 10 included. A full GM arrangement or a single piano line is not
/// what the arranger plays, and reading chords off one would load something
/// nobody could recognise as the file they picked.
pub fn chart_of(bytes: &[u8]) -> Result<String> {
    let read = read(bytes)?;
    let mut channels: Vec<u8> = read.notes.iter().map(|n| n.channel).collect();
    channels.sort_unstable();
    channels.dedup();
    if !INSTRUMENTS.contains(&channels.len()) {
        bail!(
            "{} instrument(s) in the file: the arranger reads {} to {} (bass, drums, piano, guitar)",
            channels.len(),
            INSTRUMENTS.start(),
            INSTRUMENTS.end()
        );
    }
    match read.chart {
        Some(chart) => Ok(chart),
        None => chart_of_notes(&read.notes, read.meter),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifacts::arranger::ArrangerSettings;

    fn arranger(text: &str) -> Arranger {
        let mut arr = Arranger::new(ArrangerSettings {
            on: true,
            text: text.into(),
            ..Default::default()
        });
        for role in Role::ALL {
            arr.set_gain(role, 1.0);
        }
        arr
    }

    /// What choz writes, choz reads back as the same chart — and the file is
    /// one a DAW reads: a header, a conductor, a track a musician with notes.
    #[test]
    fn a_written_file_opens_as_the_chart_that_wrote_it() {
        let _g = crate::test_locks::transport();
        let arr = arranger(super::super::DEFAULT_TEXT);
        let bytes = write(&arr, 120.0);
        assert_eq!(&bytes[..4], b"MThd");
        let read = read(&bytes).unwrap();
        assert_eq!(read.chart.as_deref(), Some(arr.settings.text.as_str()));
        assert_eq!(chart_of(&bytes).unwrap(), arr.settings.text);
        let written: usize = read.notes.len();
        assert_eq!(written, arr.part().len(), "every note of the part is in it");
        assert!(
            read.notes.iter().any(|n| n.channel == 9),
            "the kit is on 10"
        );
    }

    /// A file somebody else wrote: the chords are heard off the notes, a bar at
    /// a time, and the kit is not mistaken for harmony.
    #[test]
    fn the_chords_are_heard_off_a_file_with_no_chart() {
        let _g = crate::test_locks::transport();
        let mut arr = arranger("| C | Am | Dm7 G7 | Cmaj7 |");
        arr.settings.text = String::new(); // no chart travels with it
        let mut bytes = write(&arr, 120.0);
        // Drop the text event by rewriting its tag: the notes stay.
        let at = bytes
            .windows(CHART_TAG.len())
            .position(|w| w == CHART_TAG.as_bytes())
            .unwrap();
        bytes[at] = b'x';
        let chart = chart_of(&bytes).unwrap();
        let prog = chord::parse_progression(&chart).unwrap();
        assert_eq!(prog.bars.len(), 4, "{chart}");
        let pcs = |b: usize, i: usize| {
            let mut p = prog.bars[b].chords[i].pitch_classes();
            p.sort();
            p
        };
        assert_eq!(pcs(0, 0), vec![0, 4, 7], "{chart}");
        assert_eq!(pcs(1, 0), vec![0, 4, 9], "{chart}");
        assert_eq!(pcs(3, 0), vec![0, 4, 7, 11], "{chart}");
    }

    /// **Balanced on any GM player.** Every style bakes its comping far under
    /// its bass for the one instrument a tab has; in the file each musician is
    /// brought to a level of its own, the accents keep their order, and the
    /// fader goes on the channel's volume instead of into the playing.
    #[test]
    fn every_musician_is_heard_in_the_file() {
        let _g = crate::test_locks::transport();
        for style in ["shfblues", "strtrock", "16_beat"] {
            let mut arr = arranger(&format!("key = C\nstyle = {style}\n| C7 | F7 | C7 | G7 |"));
            arr.set_gain(Role::Guitar, 0.5);
            let read = read(&write(&arr, 120.0)).unwrap();
            for role in Role::ALL {
                let ch = channel(role);
                let vels: Vec<f64> = read
                    .notes
                    .iter()
                    .filter(|n| n.channel == ch)
                    .map(|n| n.vel as f64)
                    .collect();
                assert!(!vels.is_empty(), "{style}: {} is silent", role.name());
                let mean = vels.iter().sum::<f64>() / vels.len() as f64;
                let want = target_velocity(role);
                assert!(
                    (mean - want).abs() < 6.0,
                    "{style}: {} averages {mean:.0}, wants {want}",
                    role.name()
                );
                // The accents survive: the loudest note in the part is still
                // louder than its average.
                let peak = vels.iter().cloned().fold(0.0, f64::max);
                let baked: Vec<u8> = arr
                    .part()
                    .iter()
                    .filter(|n| n.role == role)
                    .map(|n| n.vel)
                    .collect();
                if baked.iter().min() != baked.iter().max() {
                    assert!(peak > mean, "{style}: {} lost its accents", role.name());
                }
            }
            let volume = |ch: u8| read.volumes.iter().find(|(c, _)| *c == ch).map(|(_, v)| *v);
            assert_eq!(volume(channel(Role::Bass)), Some(100), "{style}");
            assert_eq!(
                volume(channel(Role::Guitar)),
                Some(50),
                "{style}: the fader"
            );
        }
    }

    /// Two to four instruments, or the file is not one the arranger reads: a
    /// band of one is a melody, a whole GM song is not a backing.
    #[test]
    fn a_file_needs_a_bands_worth_of_instruments() {
        let _g = crate::test_locks::transport();
        let mut arr = arranger("| C | F |");
        arr.settings.parts = vec![1.0, 0.0, 0.0, 0.0]; // the bass alone
        arr.rebake();
        let err = chart_of(&write(&arr, 120.0)).unwrap_err().to_string();
        assert!(err.contains("1 instrument"), "{err}");
        arr.set_gain(Role::Drums, 1.0);
        assert!(chart_of(&write(&arr, 120.0)).is_ok(), "two is a band");

        // Five channels of notes: hand-built, the way another program writes.
        let mut events = Vec::new();
        for ch in 0..5u8 {
            events.push((0, vec![0x90 | ch, 60, 100]));
            events.push((480, vec![0x80 | ch, 60, 0]));
        }
        let mut bytes = b"MThd".to_vec();
        bytes.extend(6u32.to_be_bytes());
        bytes.extend(0u16.to_be_bytes());
        bytes.extend(1u16.to_be_bytes());
        bytes.extend(PPQ.to_be_bytes());
        bytes.extend(track(events));
        let err = chart_of(&bytes).unwrap_err().to_string();
        assert!(err.contains("5 instrument"), "{err}");
    }

    /// A chart that changes meter exports its changes: a time signature at
    /// every bar that changes, so a DAW's grid has the bars the band played.
    #[test]
    fn a_heterometric_chart_exports_its_meters() {
        let _g = crate::test_locks::transport();
        let arr = arranger("style = tarkus\n| 5/4 Fm | 3/4 Db | Eb | 7/8 Fm |");
        let bytes = write(&arr, 120.0);
        let sigs: Vec<(u8, u8)> = (0..bytes.len().saturating_sub(5))
            .filter(|&i| bytes[i] == 0xFF && bytes[i + 1] == 0x58 && bytes[i + 2] == 4)
            .map(|i| (bytes[i + 3], bytes[i + 4]))
            .collect();
        assert_eq!(
            sigs,
            vec![(5, 2), (3, 2), (7, 3)],
            "5/4, then 3/4 at bar 2, then 7/8"
        );
    }

    #[test]
    fn what_is_not_a_midi_file_says_so() {
        assert!(chart_of(b"key = C\n| C |").is_err());
        assert!(chart_of(b"MThd").is_err());
    }
}
