//! What the audio says, for the samples whose name would not say it.
//!
//! One job: the recorded pitch. Everything else here (peak, RMS, length) comes
//! free from the decode and is worth keeping because the mapper and the
//! inspector both want it.
//!
//! **Where in the sample it listens matters more than the algorithm.** A
//! violin's first 50 ms is bow noise and a piano's is a hammer — run a detector
//! over the attack and it reports the transient's period, which is not a note.
//! So the window is taken from the loudest part of the *sustain*: find the
//! peak, walk past it, and analyse there.
//!
//! The detector is [`crate::pitch::yin`], the same one the `A→M` converter and
//! AutoTune use. It runs here at a tenth of the effort it costs them, because
//! this happens once per file in a background scan rather than every 8 ms on
//! the audio thread.

use std::path::Path;

use anyhow::Result;

/// The rate the analysis runs at. YIN costs `half × lags`, both of which scale
/// with the rate, so halving it quarters the work — and nothing above 11 kHz
/// tells you anything about the period of a note a sampler can play.
const ANALYSIS_RATE: u32 = 22_050;

/// Samples in the analysis window. Two periods of the lowest note have to fit
/// in `half`, and the lowest note here is 25 Hz.
const WINDOW: usize = 8192;

/// Below this, the sample is called unpitched rather than guessed at. A drum
/// hit run through a pitch detector always produces *a* number; the whole
/// point of a confidence is to refuse it.
pub const MIN_CLARITY: f32 = 0.55;

/// What one file turned out to be.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Analysis {
    /// The detected pitch as a MIDI note, or `None` when nothing convincing
    /// was found — a drum, a noise bed, a chord.
    pub root: Option<u8>,
    /// How far the detected pitch sits from that note, in cents. The sampler
    /// tunes it back out, so a sample recorded 30 cents flat plays in tune.
    pub cents: f32,
    /// YIN's clarity for the window that was analysed, 0..1.
    pub confidence: f32,
    pub peak: f32,
    pub rms: f32,
    pub frames: u64,
    pub sample_rate: u32,
}

/// Decode `path` and listen to it. Slow — a decode plus a few million
/// multiply-adds — and therefore never on the audio thread.
pub fn analyze(path: &Path) -> Result<Analysis> {
    let stereo = crate::instruments::sfz::decode(path, ANALYSIS_RATE)?;
    Ok(of_stereo(&stereo, ANALYSIS_RATE))
}

/// The analysis proper, over interleaved stereo. Split out so a test can hand
/// it a tone it built itself rather than a file it had to write.
pub fn of_stereo(stereo: &[f32], sample_rate: u32) -> Analysis {
    let frames = stereo.len() / 2;
    let mono: Vec<f32> = stereo
        .as_chunks::<2>()
        .0
        .iter()
        .map(|f| (f[0] + f[1]) * 0.5)
        .collect();
    let peak = mono.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let rms = (mono.iter().map(|s| (*s as f64) * (*s as f64)).sum::<f64>()
        / mono.len().max(1) as f64)
        .sqrt() as f32;

    let empty = Analysis {
        root: None,
        cents: 0.0,
        confidence: 0.0,
        peak,
        rms,
        frames: frames as u64,
        sample_rate,
    };
    let Some(start) = sustain_start(&mono) else {
        return empty;
    };
    let window = &mono[start..(start + WINDOW).min(mono.len())];
    if window.len() < WINDOW / 4 {
        return empty;
    }

    let half = window.len() / 2;
    // 25 Hz to 4.2 kHz: below is a room mode, above is the top of a piccolo.
    let min_lag = (sample_rate as usize / 4200).max(2);
    let max_lag = (sample_rate as usize / 25).min(half - 1);
    let mut diff = vec![0.0f32; max_lag + 2];
    let Some((period, clarity)) = crate::pitch::yin(window, 0, half, min_lag, max_lag, &mut diff)
    else {
        return empty;
    };
    if clarity < MIN_CLARITY || period <= 0.0 {
        return Analysis {
            confidence: clarity,
            ..empty
        };
    }
    let freq = sample_rate as f32 / period;
    let exact = crate::pitch::freq_to_note_exact(freq);
    let note = exact.round().clamp(0.0, 127.0);
    Analysis {
        root: Some(note as u8),
        cents: (exact - note) * 100.0,
        confidence: clarity,
        ..empty
    }
}

/// Where the note has settled: past the attack, still loud.
///
/// The peak of a struck or bowed sample is inside its transient, so the window
/// starts a fixed distance after it rather than on it. A sample too short to
/// have a sustain (a click, a truncated file) has none, and says so.
fn sustain_start(mono: &[f32]) -> Option<usize> {
    if mono.len() < WINDOW / 4 {
        return None;
    }
    let (peak_at, _) = mono
        .iter()
        .enumerate()
        .fold((0usize, 0.0f32), |best, (i, s)| {
            let a = s.abs();
            if a > best.1 {
                (i, a)
            } else {
                best
            }
        });
    // A tenth of a second past the loudest point, which is past the hammer,
    // the pick and the first of the bow — and back off when the file is too
    // short to give it.
    let want = peak_at + WINDOW / 2;
    Some(want.min(mono.len().saturating_sub(WINDOW / 4)))
}

/// Where the hits are, as fractions of the file, `0.0` first and ascending.
///
/// This is what `SLICE` cuts on. Sixteen equal pieces land between the golpes
/// of any break that was not played to a grid, and a slice that starts halfway
/// through a snare is a slice nobody can use.
///
/// ponytail: an energy detector — the rise in RMS from one 10 ms hop to the
/// next — rather than spectral flux. It hears a drum hit, which is what a
/// break is made of; the day someone slices a legato pad, that is the upgrade
/// and it is a different function, not a flag on this one.
///
/// Fewer than two onsets means it heard nothing it could call a hit, and the
/// caller falls back to equal cuts: an empty answer is "I don't know", never
/// "the file is silent".
pub fn onsets(stereo: &[f32], sample_rate: u32, max: usize) -> Vec<f32> {
    let frames = stereo.len() / 2;
    // 10 ms hops: shorter than the gap between two hits anybody plays, longer
    // than one period of anything in a kick.
    let hop = (sample_rate as usize / 100).max(1);
    if frames < hop * 4 || max < 2 {
        return Vec::new();
    }
    let env: Vec<f32> = (0..frames / hop)
        .map(|h| {
            let block = &stereo[h * hop * 2..((h + 1) * hop * 2).min(stereo.len())];
            let sum: f64 = block.iter().map(|s| (*s as f64) * (*s as f64)).sum();
            (sum / block.len().max(1) as f64).sqrt() as f32
        })
        .collect();
    // The rise, and only the rise: energy falling away is the tail of the hit
    // before, not a new one.
    let flux: Vec<f32> = env
        .windows(2)
        .map(|w| (w[1] - w[0]).max(0.0))
        .collect();
    let mean = flux.iter().sum::<f32>() / flux.len().max(1) as f32;
    // Half again over the average rise, and never on a file that is all noise
    // floor: a threshold of nothing finds a hit every hop.
    let floor = (mean * 1.5).max(1e-4);
    // 50 ms: two hits closer than that are one hit with a flam on it.
    let gap = 5usize;
    let mut hits: Vec<(usize, f32)> = Vec::new();
    for (i, f) in flux.iter().copied().enumerate() {
        let rising_peak = f > floor
            && flux.get(i.wrapping_sub(1)).is_none_or(|p| f >= *p)
            && flux.get(i + 1).is_none_or(|n| f > *n);
        if !rising_peak {
            continue;
        }
        match hits.last() {
            // Inside the gap: keep whichever of the two was the bigger hit.
            Some((at, was)) if i - at < gap => {
                if f > *was {
                    *hits.last_mut().unwrap() = (i, f);
                }
            }
            _ => hits.push((i, f)),
        }
    }
    if hits.len() < 2 {
        return Vec::new();
    }
    // More hits than there are keys: keep the loudest, in time order. A break
    // with forty hits in it plays its sixteen biggest, which are the ones
    // somebody would have cut by hand.
    if hits.len() > max {
        hits.sort_by(|a, b| b.1.total_cmp(&a.1));
        hits.truncate(max);
        hits.sort_by_key(|h| h.0);
    }
    let mut out: Vec<f32> = hits
        .iter()
        .map(|(h, _)| (*h * hop) as f32 / frames as f32)
        .collect();
    // The first slice is the top of the file whatever the detector said: the
    // audio before the first hit belongs to it, not to nobody.
    out[0] = 0.0;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tone with an attack in front of it, the way a recorded note has one.
    fn note(freq: f32, sample_rate: u32, seconds: f32) -> Vec<f32> {
        let frames = (sample_rate as f32 * seconds) as usize;
        let mut out = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            let t = i as f32 / sample_rate as f32;
            // 20 ms of noise-ish transient, then the tone. Without something
            // in front of it this would not be testing where the window lands.
            let s = if t < 0.02 {
                ((i * 2654435761) as f32 / u32::MAX as f32 - 0.5) * 0.8
            } else {
                (std::f32::consts::TAU * freq * t).sin() * 0.5
            };
            out.push(s);
            out.push(s);
        }
        out
    }

    /// A break: silence with hits in it, at positions nothing would land on
    /// by cutting the file in sixteen.
    fn hits(at: &[f32], sample_rate: u32, seconds: f32) -> Vec<f32> {
        let frames = (sample_rate as f32 * seconds) as usize;
        let mut out = vec![0.0f32; frames * 2];
        for fraction in at {
            let start = (frames as f32 * fraction) as usize;
            // 80 ms of noise dying away, which is a drum as far as an energy
            // detector is concerned.
            let hit = sample_rate as usize / 12;
            for i in 0..hit {
                let Some(f) = start.checked_add(i).filter(|f| *f < frames) else {
                    break;
                };
                let decay = 1.0 - i as f32 / hit as f32;
                let s = ((i * 2654435761) as f32 / u32::MAX as f32 - 0.5) * decay;
                out[f * 2] = s;
                out[f * 2 + 1] = s;
            }
        }
        out
    }

    /// `SLICE` cuts where the hits are. Sixteen equal pieces of a break nobody
    /// played to a grid start halfway through a snare, which is the failure
    /// this replaces.
    #[test]
    fn the_onsets_are_where_the_hits_are() {
        let at = [0.0, 0.17, 0.41, 0.66, 0.83];
        let found = onsets(&hits(&at, 44_100, 4.0), 44_100, 16);
        assert_eq!(found.len(), at.len(), "{found:?}");
        for (want, got) in at.iter().zip(&found) {
            assert!((want - got).abs() < 0.02, "wanted {at:?}, found {found:?}");
        }
    }

    /// Silence has no hits in it, and saying so is what makes the caller fall
    /// back to equal pieces rather than cut a file into one.
    #[test]
    fn silence_has_no_onsets() {
        assert!(onsets(&vec![0.0f32; 44_100 * 2], 44_100, 16).is_empty());
        // And a file too short to have two hops in it is not a break.
        assert!(onsets(&[0.5f32; 16], 44_100, 16).is_empty());
    }

    /// More hits than there are keys: the loudest survive, in time order.
    #[test]
    fn a_busy_break_keeps_its_biggest_hits() {
        let mut at: Vec<f32> = (0..24).map(|i| i as f32 / 24.0).collect();
        at.remove(0);
        let mut audio = hits(&at, 44_100, 6.0);
        // One hit the detector cannot miss, late in the file: it has to come
        // back, and it has to come back last.
        for s in audio.iter_mut().skip(44_100 * 2 * 5) {
            *s *= 2.0;
        }
        let found = onsets(&audio, 44_100, 8);
        assert_eq!(found.len(), 8, "{found:?}");
        assert!(found.windows(2).all(|w| w[0] < w[1]), "out of order: {found:?}");
        assert_eq!(found[0], 0.0, "the first slice is the top of the file");
    }

    #[test]
    fn a_tone_is_named_by_its_note() {
        for (freq, expected) in [(440.0, 69), (261.626, 60), (523.251, 72), (110.0, 45)] {
            let a = of_stereo(&note(freq, 22_050, 1.0), 22_050);
            assert_eq!(a.root, Some(expected), "{freq} Hz");
            assert!(a.confidence > MIN_CLARITY, "{freq} Hz: {}", a.confidence);
            assert!(
                a.cents.abs() < 12.0,
                "{freq} Hz landed {} cents off",
                a.cents
            );
        }
    }

    /// Half a semitone flat is still that note, and the tuning is what makes
    /// it play in tune rather than being rounded away.
    #[test]
    fn a_detuned_tone_keeps_its_note_and_reports_the_offset() {
        // 40 cents below A4.
        let a = of_stereo(&note(440.0 * 2f32.powf(-0.40 / 12.0), 22_050, 1.0), 22_050);
        assert_eq!(a.root, Some(69));
        assert!(
            (a.cents + 40.0).abs() < 10.0,
            "expected about -40 cents, got {}",
            a.cents
        );
    }

    /// Noise has no period, and a detector that answers anyway is worse than
    /// one that shrugs: it would map a kick drum onto a key and transpose it.
    #[test]
    fn noise_is_left_unpitched() {
        let mut stereo = Vec::new();
        let mut state = 12345u32;
        for _ in 0..22_050 {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            let s = (state >> 8) as f32 / 8_388_608.0 - 1.0;
            stereo.push(s);
            stereo.push(s);
        }
        assert_eq!(of_stereo(&stereo, 22_050).root, None);
    }
}
