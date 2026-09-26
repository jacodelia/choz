//! A recording, turned into a folder the sampler already knows how to play.
//!
//! `REC` on the sampler records the tab's input; this cuts the take at its
//! silences and writes one WAV per sound. **Nothing else is new**: the folder
//! is loaded like any other, so the sampler's own analysis finds each file's
//! pitch and how far off it sits ([`super::analyze`]), [`super::map`] spreads
//! pitched sounds across the keyboard and lays unpitched ones out as a kit, and
//! two takes of the same note are a round robin.
//!
//! That is why the files carry no note in their name. `rec_take3` says only
//! which take it is — spelled, so [`super::name`] reads it as a round robin
//! and every file stays in the one group `rec` — and the pitch comes from the
//! audio, cents and all, which a name could not say.
//!
//! ## Where a sound ends
//!
//! Silence is measured against the take's own loudest moment, not a fixed
//! level: a microphone across a room and a guitar into a preamp differ by
//! 30 dB, and both have a noise floor well under what was played. A 10 ms
//! window is quiet when it is 40 dB under the loudest one (and never louder
//! than -60 dBFS); a gap shorter than 60 ms is a breath inside one sound, not
//! the end of it; and anything shorter than 80 ms is a click, not a note.
//!
//! ## Kept, or not
//!
//! A take is **not kept until it is saved**. `REC` writes into this process's
//! own scratch folder ([`unsaved_dir`]); `SAVE` moves it into
//! [`recordings_dir`], which is where it stays. One that is never saved is
//! deleted as soon as nothing plays it — another instrument, `CLEAR`, the tab
//! closed — and whatever is left when choz exits goes with it. A choz that
//! crashed leaves its folder behind, and the next one to start removes it.

use std::path::{Path, PathBuf};

/// The longest take `REC` keeps, in seconds. A minute is a dozen notes with
/// room to breathe between them, and 23 MB of buffer at 48 kHz.
pub const MAX_SECS: usize = 60;

const WINDOW_MS: usize = 10;
/// A window this far under the loudest one is silence.
const SILENCE_BELOW_PEAK_DB: f32 = -40.0;
/// …and one this quiet always is, however soft the whole take was.
const SILENCE_FLOOR: f32 = 0.001; // -60 dBFS
/// Gaps shorter than this stay inside the sound.
const MIN_GAP_MS: usize = 60;
/// Sounds shorter than this are dropped.
const MIN_SOUND_MS: usize = 80;
/// Fades that keep a cut from clicking.
const FADE_IN_MS: usize = 2;
const FADE_OUT_MS: usize = 10;

/// Where every **saved** take lives: one folder per recording under
/// `~/.local/state/choz/recordings`. A search path of its own, so a project
/// that played one finds it again and SOURCE lists them all.
pub fn recordings_dir() -> PathBuf {
    crate::cache::state_dir().join("recordings")
}

/// Takes nobody has saved yet, one folder per running choz — so two of them
/// never clear each other's.
fn unsaved_root() -> PathBuf {
    crate::cache::state_dir().join("recordings-unsaved")
}

/// This process's unsaved takes.
pub fn unsaved_dir() -> PathBuf {
    unsaved_root().join(std::process::id().to_string())
}

/// Whether `dir` is a take that has not been saved.
pub fn is_unsaved(dir: &Path) -> bool {
    // `starts_with` compares components, so `<pid>/../..` would pass it: a path
    // read from a project file must not climb out of the scratch folder.
    dir.starts_with(unsaved_dir())
        && dir != unsaved_dir()
        && !dir
            .components()
            .any(|c| c == std::path::Component::ParentDir)
}

/// Save a take: move it out of the scratch folder into [`recordings_dir`].
/// Returns where it is now.
pub fn keep(dir: &Path) -> std::io::Result<PathBuf> {
    let name = dir
        .file_name()
        .ok_or_else(|| std::io::Error::other("a take has a folder name"))?;
    std::fs::create_dir_all(recordings_dir())?;
    let to = recordings_dir().join(name);
    std::fs::rename(dir, &to)?;
    super::cache::forget(dir);
    Ok(to)
}

/// Delete an unsaved take and what the sampler worked out about it. Anything
/// outside the scratch folder is refused: this deletes a directory.
pub fn discard(dir: &Path) {
    if !is_unsaved(dir) {
        return;
    }
    let _ = std::fs::remove_dir_all(dir);
    super::cache::forget(dir);
}

/// Every unsaved take of this process, for a clean exit.
pub fn discard_all() {
    for take in takes(&unsaved_dir()) {
        discard(&take);
    }
    let _ = std::fs::remove_dir(unsaved_dir());
}

/// The takes in a scratch folder.
pub fn takes(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect()
}

/// What a choz that did not exit cleanly left behind: every scratch folder
/// whose process is gone.
pub fn discard_stale() {
    for dir in takes(&unsaved_root()) {
        let pid = dir
            .file_name()
            .and_then(|n| n.to_str()?.parse::<u32>().ok());
        let alive = pid.is_some_and(|p| Path::new(&format!("/proc/{p}")).exists());
        if !alive {
            for take in takes(&dir) {
                super::cache::forget(&take);
            }
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

/// The sounds in an interleaved stereo take, as frame ranges `start..end`.
pub fn sounds(stereo: &[f32], sample_rate: u32) -> Vec<(usize, usize)> {
    let window = (sample_rate as usize * WINDOW_MS / 1000).max(1);
    let frames = stereo.len() / 2;
    let levels: Vec<f32> = (0..frames.div_ceil(window))
        .map(|w| {
            let (a, b) = (w * window, ((w + 1) * window).min(frames));
            let sum: f32 = (a..b)
                .map(|f| {
                    let m = (stereo[f * 2] + stereo[f * 2 + 1]) * 0.5;
                    m * m
                })
                .sum();
            (sum / (b - a) as f32).sqrt()
        })
        .collect();
    let peak = levels.iter().copied().fold(0.0f32, f32::max);
    let threshold = (peak * 10f32.powf(SILENCE_BELOW_PEAK_DB / 20.0)).max(SILENCE_FLOOR);
    if peak < threshold {
        return Vec::new();
    }

    let min_gap = MIN_GAP_MS / WINDOW_MS;
    let min_sound = MIN_SOUND_MS / WINDOW_MS;
    // Runs of loud windows, in windows.
    let mut runs: Vec<(usize, usize)> = Vec::new();
    for (w, level) in levels.iter().enumerate() {
        if *level < threshold {
            continue;
        }
        match runs.last_mut() {
            Some(last) if w - last.1 <= min_gap => last.1 = w + 1,
            _ => runs.push((w, w + 1)),
        }
    }
    runs.into_iter()
        .filter(|(a, b)| b - a >= min_sound)
        // One window of pre-roll: an attack starts inside the window before
        // the first one loud enough to count.
        .map(|(a, b)| (a.saturating_sub(1) * window, (b * window).min(frames)))
        .collect()
}

/// Cut `stereo` at its silences and write each sound into `dir` as
/// `rec_take<n>.wav`, 32-bit float. Returns how many were written — 0 when the
/// take was silence, and then nothing is created at all.
pub fn save(stereo: &[f32], sample_rate: u32, dir: &Path) -> std::io::Result<usize> {
    let found = sounds(stereo, sample_rate);
    if found.is_empty() {
        return Ok(0);
    }
    std::fs::create_dir_all(dir)?;
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let fade_in = sample_rate as usize * FADE_IN_MS / 1000;
    let fade_out = sample_rate as usize * FADE_OUT_MS / 1000;
    for (n, (a, b)) in found.iter().enumerate() {
        let path = dir.join(format!("rec_take{}.wav", n + 1));
        let mut w = hound::WavWriter::create(&path, spec)
            .map_err(|e| std::io::Error::other(format!("{path:?}: {e}")))?;
        let len = b - a;
        for i in 0..len {
            let gain = (i as f32 / fade_in.max(1) as f32)
                .min((len - i) as f32 / fade_out.max(1) as f32)
                .min(1.0);
            for ch in 0..2 {
                w.write_sample(stereo[(a + i) * 2 + ch] * gain)
                    .map_err(std::io::Error::other)?;
            }
        }
        w.finalize().map_err(std::io::Error::other)?;
    }
    Ok(found.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    /// `(seconds of tone at `hz`, or of silence when `hz` is 0)`, stereo.
    fn take(parts: &[(f32, f32)]) -> Vec<f32> {
        let mut out = Vec::new();
        for &(secs, hz) in parts {
            for i in 0..(secs * SR as f32) as usize {
                let s = match hz {
                    0.0 => 0.0002 * ((i * 7919 % 101) as f32 / 50.0 - 1.0), // a noise floor
                    _ => 0.5 * (std::f32::consts::TAU * hz * i as f32 / SR as f32).sin(),
                };
                out.extend([s, s]);
            }
        }
        out
    }

    /// Three notes with silence between them are three sounds, each cut
    /// where it is, and a short breath inside a note does not split it.
    #[test]
    fn a_take_is_cut_at_its_silences() {
        let audio = take(&[
            (0.3, 0.0),
            (0.5, 220.0),
            (0.03, 0.0), // a breath: shorter than a gap
            (0.5, 220.0),
            (0.4, 0.0),
            (0.6, 330.0),
            (0.4, 0.0),
            (0.04, 440.0), // a click: shorter than a sound
            (0.4, 0.0),
            (0.5, 440.0),
            (0.2, 0.0),
        ]);
        let found = sounds(&audio, SR);
        assert_eq!(found.len(), 3, "{found:?}");
        let secs = |f: usize| f as f32 / SR as f32;
        assert!((secs(found[0].0) - 0.3).abs() < 0.02, "{found:?}");
        assert!((secs(found[0].1) - 1.33).abs() < 0.02, "{found:?}");
        assert!((secs(found[1].0) - 1.73).abs() < 0.02, "{found:?}");
        assert!((secs(found[2].1) - 3.67).abs() < 0.02, "{found:?}");
        assert!(
            sounds(&take(&[(1.0, 0.0)]), SR).is_empty(),
            "silence is no sound"
        );
    }

    /// End to end with the sampler's own reading of the folder: each file is
    /// found at the pitch that was played, and every one is in the same group,
    /// so the map plays them all as one instrument.
    #[test]
    fn the_files_read_back_as_one_instrument_at_the_notes_played() {
        let dir = std::env::temp_dir().join(format!("choz_capture_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // A3 and E4.
        let audio = take(&[
            (0.2, 0.0),
            (0.8, 220.0),
            (0.3, 0.0),
            (0.8, 329.63),
            (0.2, 0.0),
        ]);
        assert_eq!(save(&audio, SR, &dir).unwrap(), 2);

        let mut samples = crate::instruments::sampler::describe(&dir);
        samples.sort_by_key(|s| s.root);
        let roots: Vec<Option<u8>> = samples.iter().map(|s| s.root).collect();
        assert_eq!(roots, vec![Some(57), Some(64)]);
        assert!(samples.iter().all(|s| s.group == samples[0].group));
        assert_eq!(
            crate::instruments::sampler::map::regions(&samples).len(),
            2,
            "both notes are on the keyboard"
        );
        assert_eq!(
            save(&take(&[(0.5, 0.0)]), SR, &dir.join("quiet")).unwrap(),
            0
        );
        assert!(!dir.join("quiet").exists(), "a silent take writes nothing");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A take lives in the scratch folder until it is saved; saved, it moves
    /// to the recordings; discarded, it and its analysis are gone — and
    /// nothing outside the scratch folder can be deleted by mistake.
    #[test]
    fn an_unsaved_take_is_kept_or_thrown_away() {
        let audio = take(&[(0.2, 0.0), (0.5, 220.0), (0.2, 0.0)]);
        let a = unsaved_dir().join("rec-test-a");
        let b = unsaved_dir().join("rec-test-b");
        for d in [&a, &b] {
            assert_eq!(save(&audio, SR, d).unwrap(), 1);
            crate::instruments::sampler::describe(d); // writes its analysis
            assert!(is_unsaved(d));
        }

        let kept = keep(&a).unwrap();
        assert!(!a.exists() && kept.join("rec_take1.wav").exists());
        assert!(!is_unsaved(&kept));

        discard(&b);
        assert!(!b.exists(), "discarded");
        discard(&kept);
        assert!(kept.exists(), "a saved take is never deleted by discard");
        assert!(
            !is_unsaved(&unsaved_dir().join("x/../..")),
            "nor anything a `..` climbs to"
        );
        std::fs::remove_dir_all(&kept).unwrap();
    }
}
