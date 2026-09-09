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
        .chunks_exact(2)
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
