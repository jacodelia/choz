//! The smart sampler against a real library, when there is one on the machine.
//!
//! Nothing here ships with a sample: the packs this is for are megabytes and
//! somebody else's to distribute. Point `CHOZ_SAMPLE_DIR` at a folder of them
//! — Philharmonia's `violin`, a drum kit, anything — and this reads it the way
//! the plugin would. With the variable unset it passes trivially, which is the
//! same bargain the plugin-hosting tests strike.
//!
//! ```bash
//! CHOZ_SAMPLE_DIR=~/Samples/Philharmonia/violin \
//!     cargo test -p choz-engine --release --test sample_folder -- --nocapture
//! ```

use choz_engine::instruments::sampler;

#[test]
fn a_real_library_maps_and_plays() {
    let Some(dir) = std::env::var_os("CHOZ_SAMPLE_DIR").map(std::path::PathBuf::from) else {
        eprintln!("CHOZ_SAMPLE_DIR unset; skipping");
        return;
    };
    // No `exists` check: a folder inside an archive —
    // `percussion.zip/bass drum` — is a path nothing on disk answers to, and
    // it is a perfectly good instrument.
    assert!(
        sampler::is_instrument_dir(&dir),
        "{}: nothing in it looks like a sample",
        dir.display()
    );

    let started = std::time::Instant::now();
    let samples = sampler::describe(&dir);
    let scan = started.elapsed();
    assert!(!samples.is_empty(), "the folder read as empty");

    let named = samples.iter().filter(|s| s.from_name).count();
    let unpitched = samples.iter().filter(|s| s.root.is_none()).count();
    let group = sampler::map::primary_group(&samples);
    let regions = sampler::map::regions(&samples);
    let notes = {
        let mut n: Vec<u8> = regions.iter().map(|r| r.pitch_key_center).collect();
        n.sort_unstable();
        n.dedup();
        n
    };
    let layers = {
        let mut l: Vec<(u8, u8)> = regions.iter().map(|r| (r.lo_vel, r.hi_vel)).collect();
        l.sort_unstable();
        l.dedup();
        l
    };
    eprintln!(
        "{}: {} samples in {scan:.1?}, {named} named, {unpitched} unpitched\n\
         group {group:?}: {} regions, {} notes {:?}..{:?}, {} velocity layers {layers:?}",
        dir.display(),
        samples.len(),
        regions.len(),
        notes.len(),
        notes.first(),
        notes.last(),
        layers.len(),
    );

    // A kit and an instrument are held to different promises. An instrument
    // must reach every key — a dead key in the middle of a keyboard is a bug.
    // A kit must not: its keys are its sounds, one each, at their own pitch.
    let kit = regions.iter().all(|r| r.lo_key == r.hi_key);
    if kit {
        eprintln!("read as a kit: one key per sound");
        assert!(
            regions.iter().all(|r| r.lo_key == r.pitch_key_center),
            "a kit plays its sounds at the pitch they were recorded at"
        );
    } else {
        for key in 0..=127u8 {
            assert!(
                regions.iter().any(|r| (r.lo_key..=r.hi_key).contains(&key)),
                "no sample answers key {key}"
            );
        }
    }

    let mut instrument = sampler::build(&dir, 48_000).expect("the folder would not build");
    let mut buf = vec![0.0f32; 4096];
    {
        use choz_engine::sources::AudioSource;
        instrument.render(&mut buf, 48_000);
        assert!(buf.iter().all(|s| *s == 0.0), "sound before a note");
        // A key in the middle of what was actually recorded, at a middling
        // velocity. Not key 60: a kit does not have one, and the bottom of an
        // instrument's range is a sample transposed four octaves down, which is
        // quiet and slow enough to look like silence.
        let key = notes[notes.len() / 2];
        instrument.note_on(key, 100);
        let mut peak = 0.0f32;
        for _ in 0..20 {
            instrument.render(&mut buf, 48_000);
            peak = peak.max(buf.iter().fold(0.0f32, |m, s| m.max(s.abs())));
        }
        assert!(peak > 0.01, "the instrument stayed silent: peak {peak}");
        eprintln!("note {key} at velocity 100 peaked at {peak:.3}");
    }
}
