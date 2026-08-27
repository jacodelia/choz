//! Unit tests for the parts, then DSP tests for the whole thing.
//!
//! The DSP tests are the ones that matter: a pitch corrector that passes every
//! unit test and still warbles is a pitch corrector that does not work.

use super::*;
use choz_ports::FxProcessor;

/// A tone that keeps its phase across blocks. Restarting the phase every block
/// puts a discontinuity in the signal, and a detector that is working will
/// faithfully report the period *that* has.
struct Tone {
    phase: f32,
}

impl Tone {
    fn new() -> Self {
        Self { phase: 0.0 }
    }

    /// One interleaved stereo block of a sine at `hz`.
    fn block(&mut self, hz: f32, sr: f32, frames: usize, amp: f32) -> Vec<f32> {
        let step = std::f32::consts::TAU * hz / sr;
        (0..frames)
            .flat_map(|_| {
                let s = amp * self.phase.sin();
                self.phase = (self.phase + step) % std::f32::consts::TAU;
                [s, s]
            })
            .collect()
    }

    /// A sound with harmonics, which is what a voice actually is — and where a
    /// detector that only finds the loudest partial reports the octave.
    fn voice(&mut self, hz: f32, sr: f32, frames: usize, amp: f32) -> Vec<f32> {
        let step = std::f32::consts::TAU * hz / sr;
        (0..frames)
            .flat_map(|_| {
                let p = self.phase;
                let s = amp
                    * (0.5 * p.sin()
                        + 1.0 * (2.0 * p).sin()
                        + 0.6 * (3.0 * p).sin()
                        + 0.25 * (4.0 * p).sin());
                self.phase = (self.phase + step) % std::f32::consts::TAU;
                [s, s]
            })
            .collect()
    }
}

// ─── Phase 1: the detector ──────────────────────────────────────────────────

/// Run a tone through the detector until it has an answer.
fn detect(hz: f32, sr: f32, harmonics: bool) -> PitchEstimate {
    let mut d = PitchDetector::new(sr);
    let mut t = Tone::new();
    let mut last = PitchEstimate::SILENT;
    for _ in 0..120 {
        let stereo = if harmonics {
            t.voice(hz, sr, 256, 0.2)
        } else {
            t.block(hz, sr, 256, 0.5)
        };
        let mono: Vec<f32> = stereo
            .as_chunks::<2>()
            .0
            .iter()
            .map(|f| (f[0] + f[1]) * 0.5)
            .collect();
        last = d.process(&mono);
    }
    last
}

#[test]
fn the_detector_finds_the_fundamental_of_a_steady_tone() {
    for sr in [44_100.0f32, 48_000.0, 96_000.0] {
        for hz in [110.0f32, 220.0, 261.626, 329.628, 440.0, 523.251] {
            let e = detect(hz, sr, false);
            assert!(e.voiced, "{hz} Hz at {sr} should be voiced");
            let cents = 1200.0 * (e.frequency_hz / hz).log2();
            assert!(
                cents.abs() < 20.0,
                "{hz} Hz at {sr}: off by {cents:.1} cents"
            );
        }
    }
}

#[test]
fn a_fundamental_quieter_than_its_harmonics_is_still_the_fundamental() {
    for hz in [98.0f32, 146.83, 220.0] {
        let e = detect(hz, 48_000.0, true);
        assert!(e.voiced, "{hz} Hz with harmonics should be voiced");
        let cents = 1200.0 * (e.frequency_hz / hz).log2();
        assert!(
            cents.abs() < 30.0,
            "{hz} Hz: off by {cents:.1} cents — an octave error is 1200"
        );
    }
}

#[test]
fn silence_and_noise_are_unvoiced() {
    let mut d = PitchDetector::new(48_000.0);
    for _ in 0..120 {
        d.process(&[0.0; 256]);
    }
    let e = d.estimate();
    assert!(!e.voiced, "silence has no pitch");
    assert_eq!(e.frequency_hz, 0.0);

    // White-ish noise: loud enough to pass the gate, periodic enough for
    // nothing. It must not be called voiced.
    let mut d = PitchDetector::new(48_000.0);
    let mut seed = 12345u32;
    for _ in 0..160 {
        let block: Vec<f32> = (0..256)
            .map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                ((seed >> 8) as f32 / 8_388_608.0 - 1.0) * 0.4
            })
            .collect();
        d.process(&block);
    }
    assert!(d.estimate().confidence < 0.9, "noise is not a note");
}

#[test]
fn rubbish_in_does_not_crash_the_detector() {
    let mut d = PitchDetector::new(48_000.0);
    let bad = [
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        0.0,
        -0.0,
        1e30,
        -1e30,
    ];
    for _ in 0..200 {
        d.process(&bad);
    }
    let e = d.estimate();
    assert!(e.frequency_hz.is_finite(), "{e:?}");
    assert!(e.confidence.is_finite());
}

// ─── Phase 2: notes and scales ──────────────────────────────────────────────

#[test]
fn frequency_and_midi_note_convert_both_ways() {
    let q = NoteQuantizer::default();
    assert!((q.hz_to_note(440.0) - 69.0).abs() < 1e-4);
    assert!((q.hz_to_note(880.0) - 81.0).abs() < 1e-4);
    assert!((q.hz_to_note(220.0) - 57.0).abs() < 1e-4);
    assert!((q.note_to_hz(69.0) - 440.0).abs() < 1e-3);
    assert!((q.note_to_hz(81.0) - 880.0).abs() < 1e-3);
    assert!((q.note_to_hz(57.0) - 220.0).abs() < 1e-3);
    // Nothing sensible comes of a frequency that is not one.
    assert_eq!(q.hz_to_note(0.0), 0.0);
    assert_eq!(q.hz_to_note(-5.0), 0.0);
    assert_eq!(q.hz_to_note(f32::NAN), 0.0);
}

#[test]
fn the_reference_pitch_moves_every_note_with_it() {
    let mut q = NoteQuantizer {
        reference_hz: 432.0,
        ..Default::default()
    };
    assert!(
        (q.hz_to_note(432.0) - 69.0).abs() < 1e-4,
        "A4 is wherever it is put"
    );
    assert!((q.note_to_hz(69.0) - 432.0).abs() < 1e-3);
    q.reference_hz = 442.0;
    assert!((q.note_to_hz(69.0) - 442.0).abs() < 1e-3);
}

#[test]
fn a_scale_holds_the_notes_it_should_and_no_others() {
    // C major: the white keys.
    let c_major = Scale::new(0, ScaleType::Major);
    for (note, want) in [
        (60, true),
        (61, false),
        (62, true),
        (63, false),
        (64, true),
        (65, true),
    ] {
        assert_eq!(c_major.contains(note), want, "note {note} in C major");
    }
    // A minor is the same pitch classes, a different root.
    let a_minor = Scale::new(9, ScaleType::Minor);
    for n in [57, 59, 60, 62, 64, 65, 67] {
        assert!(a_minor.contains(n), "note {n} is in A minor");
    }
    assert!(!a_minor.contains(61), "C# is not");

    // C minor has E flat, not E.
    let c_minor = Scale::new(0, ScaleType::Minor);
    assert!(c_minor.contains(63) && !c_minor.contains(64));

    // Pentatonic drops the two notes that make it a scale of seven.
    let c_pent = Scale::new(0, ScaleType::PentatonicMajor);
    assert!(c_pent.contains(60) && c_pent.contains(62) && c_pent.contains(64));
    assert!(!c_pent.contains(65) && !c_pent.contains(71));

    // Blues has the flat five.
    let c_blues = Scale::new(0, ScaleType::Blues);
    assert!(c_blues.contains(66), "F# is the blue note");
    assert!(!c_blues.contains(62));

    // Chromatic holds everything, which is what makes it the default: it
    // corrects tuning without deciding what key the song is in.
    let chrom = Scale::new(0, ScaleType::Chromatic);
    assert!((0..24).all(|n| chrom.contains(n)));
}

#[test]
fn the_nearest_note_is_the_nearest_one_in_the_scale() {
    let c_major = Scale::new(0, ScaleType::Major);
    // 40 cents sharp of F is still nearer F than G.
    assert_eq!(c_major.nearest(65.4), 65);
    // A note that is not in the scale goes to the closer neighbour, not up.
    assert_eq!(
        c_major.nearest(66.0),
        65,
        "F# is as near F as G; the lower wins"
    );
    assert_eq!(c_major.nearest(66.4), 67, "past halfway it is G");
    // C# is exactly a semitone from both C and D; the documented tie-break is
    // the lower note, the same choice `round` makes.
    assert_eq!(c_major.nearest(61.0), 60, "a tie goes down");
    assert_eq!(c_major.nearest(61.4), 62, "past the middle it is D");
    // Across an octave boundary the answer is still the nearest note.
    assert_eq!(c_major.nearest(71.6), 72);
    let c_pent = Scale::new(0, ScaleType::PentatonicMinor);
    assert_eq!(
        c_pent.nearest(64.0),
        63,
        "E in C minor pentatonic goes down to Eb"
    );
}

#[test]
fn the_target_frequency_is_the_note_the_singer_meant() {
    let q = NoteQuantizer {
        scale: Scale::new(0, ScaleType::Chromatic),
        reference_hz: 440.0,
        target: PitchTarget::AutomaticScale,
    };
    // 445 Hz is a sharp A: the target is A itself.
    let t = q.target_hz(445.0).unwrap();
    assert!((t - 440.0).abs() < 0.01, "got {t}");
    // 450 Hz is nearer A# (466.16) than A? No — it is 38 cents sharp of A.
    assert!((q.target_hz(450.0).unwrap() - 440.0).abs() < 0.01);
    // 460 Hz is 76 cents sharp of A, so A# is nearer.
    assert!((q.target_hz(460.0).unwrap() - 466.164).abs() < 0.01);
    assert_eq!(q.target_hz(0.0), None);
    assert_eq!(q.target_hz(f32::NAN), None);

    // A fixed MIDI target ignores the scale entirely, which is the hook the
    // MIDI routing will use.
    let q = NoteQuantizer {
        target: PitchTarget::MidiNote(60),
        ..q
    };
    assert!((q.target_hz(445.0).unwrap() - 261.626).abs() < 0.01);
}

// ─── Phase 3: correction and smoothing ──────────────────────────────────────

#[test]
fn a_retune_time_is_a_glide_and_not_a_jump() {
    let sr = 48_000.0;
    let mut c = PitchCorrector::new(sr);
    c.retune_ms = 100.0;
    c.correction = 1.0;

    // One semitone of error, asked for in 64-sample blocks.
    let first = c.advance(1.0, 64);
    assert!(
        first > 1.0 && first < 1.002,
        "one block is a nudge, not the answer: {first}"
    );
    let mut ratio = first;
    // Ten time constants: a one-pole is asymptotic, so "arrived" needs saying.
    for _ in 0..1000 {
        ratio = c.advance(1.0, 64);
    }
    // A semitone is 2^(1/12) ≈ 1.0595. After many time constants it is there.
    assert!((ratio - 1.0595).abs() < 0.002, "it does arrive: {ratio}");

    // And it comes back the same way when the error goes.
    let back = c.advance(0.0, 64);
    assert!(back < ratio && back > 1.0, "released, not snapped: {back}");
}

#[test]
fn correction_decides_how_much_of_the_error_is_taken() {
    let sr = 48_000.0;
    let settle = |correction: f32| {
        let mut c = PitchCorrector::new(sr);
        c.retune_ms = 5.0;
        c.correction = correction;
        let mut r = 1.0;
        for _ in 0..400 {
            r = c.advance(1.0, 64);
        }
        r
    };
    assert!((settle(0.0) - 1.0).abs() < 1e-4, "nothing at all at 0 %");
    let half = settle(0.5);
    assert!(
        (half - 1.0293).abs() < 0.002,
        "half a semitone at 50 %: {half}"
    );
    assert!((settle(1.0) - 1.0595).abs() < 0.002);
}

#[test]
fn hard_tune_arrives_far_sooner_than_natural() {
    let sr = 48_000.0;
    let blocks_to_arrive = |mode: AutoTuneMode, retune: f32| {
        let mut c = PitchCorrector::new(sr);
        c.retune_ms = retune;
        c.correction = 1.0;
        c.mode = mode;
        for n in 1..2000 {
            if c.advance(1.0, 64) > 1.055 {
                return n;
            }
        }
        i32::MAX
    };
    let hard = blocks_to_arrive(AutoTuneMode::HardTune, 500.0);
    let natural = blocks_to_arrive(AutoTuneMode::Natural, 500.0);
    assert!(
        hard < natural / 10,
        "hard {hard} blocks vs natural {natural}"
    );
    assert!(hard < 10, "and it is immediate by ear: {hard} blocks");
}

#[test]
fn humanize_moves_the_curve_without_moving_the_note() {
    let sr = 48_000.0;
    let mut plain = PitchCorrector::new(sr);
    plain.retune_ms = 120.0;
    let mut human = PitchCorrector::new(sr);
    human.retune_ms = 120.0;
    human.humanize = 1.0;

    let mut differed = false;
    let (mut a, mut b) = (1.0, 1.0);
    for _ in 0..1500 {
        a = plain.advance(1.0, 64);
        b = human.advance(1.0, 64);
        if (a - b).abs() > 1e-4 {
            differed = true;
        }
    }
    assert!(differed, "humanise has to actually do something");
    // …and both still land on the note. Humanising the destination would be
    // humanising the tuning, which is the one thing it must not do.
    assert!((a - b).abs() < 0.001, "same note in the end: {a} vs {b}");
}

/// A singer is out by a semitone, not by a fifth. An error that big is the
/// detector having found a harmonic, and bending the voice to meet it is how a
/// pitch corrector turns a voice into noise.
#[test]
fn an_unbelievable_error_is_ignored_rather_than_obeyed() {
    let mut c = PitchCorrector::new(48_000.0);
    c.retune_ms = 5.0;
    let mut r = 1.0;
    for _ in 0..400 {
        r = c.advance(7.0, 64); // a fifth: the detector is wrong, not the singer
    }
    assert!(
        (r - 1.0).abs() < 1e-3,
        "left alone, not dragged a fifth: {r}"
    );

    // A believable one is still corrected.
    let mut c = PitchCorrector::new(48_000.0);
    c.retune_ms = 5.0;
    for _ in 0..400 {
        r = c.advance(1.0, 64);
    }
    assert!((r - 1.0595).abs() < 0.002, "a semitone is a singer: {r}");
}

#[test]
fn the_corrector_survives_rubbish() {
    let mut c = PitchCorrector::new(48_000.0);
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1e30, -1e30] {
        let r = c.advance(bad, 64);
        assert!(r.is_finite() && r > 0.0, "{bad} gave {r}");
    }
}

// ─── Phase 4: the shifter ───────────────────────────────────────────────────

/// How much of `hz` is in `x`, by a single-bin DFT.
///
/// Zero-crossing counting and autocorrelation both mislead here — the first is
/// quantised to whole cycles, the second scores a lag near an integer above the
/// true one. Asking the spectrum directly is the only estimator that answers
/// the question being asked.
fn goertzel(x: &[f32], hz: f32, sr: f32) -> f32 {
    let n = x.len();
    if n < 16 {
        return 0.0;
    }
    let w = std::f32::consts::TAU * hz / sr;
    let (mut re, mut im) = (0.0f32, 0.0f32);
    for (i, &v) in x.iter().enumerate() {
        // Hann, so the neighbouring bins do not leak into the answer.
        let win = 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / (n - 1) as f32).cos();
        let a = w * i as f32;
        re += v * win * a.cos();
        im -= v * win * a.sin();
    }
    (re * re + im * im).sqrt() / n as f32
}

/// Estimate a frequency by counting zero crossings — deliberately not YIN, so
/// the test does not agree with the detector by construction.
///
/// The count is an integer, so the resolution is one cycle over the whole
/// buffer: at 220 Hz over 4096 samples that is ±90 cents, which is wider than
/// anything being asserted. Long buffers only.
fn zero_crossing_hz(x: &[f32], sr: f32) -> f32 {
    let mut crossings = 0;
    let mut first = None;
    let mut last = 0usize;
    for (i, w) in x.windows(2).enumerate() {
        if w[0] <= 0.0 && w[1] > 0.0 {
            crossings += 1;
            first.get_or_insert(i);
            last = i;
        }
    }
    // Measure between the first and last crossing rather than over the whole
    // buffer: the partial cycles at each end are most of the error.
    match first {
        Some(f) if crossings > 1 && last > f => (crossings - 1) as f32 * sr / (last - f) as f32,
        _ => 0.0,
    }
}

#[test]
fn the_shifter_moves_the_pitch_and_leaves_the_length_alone() {
    let sr = 48_000.0;
    let hz = 220.0;
    let period = sr / hz;
    for ratio in [1.0f32, 1.0595, 0.9439, 1.5] {
        let mut s = RetuneShifter::new(sr);
        let mut phase = 0.0f32;
        let frames = 4096;
        let mut out = vec![0.0f32; frames];
        let mut tail: Vec<f32> = Vec::new();
        for pass in 0..10 {
            let input: Vec<f32> = (0..frames)
                .map(|_| {
                    let v = phase.sin();
                    phase = (phase + std::f32::consts::TAU * hz / sr) % std::f32::consts::TAU;
                    v
                })
                .collect();
            s.process(&input, &mut out, ratio, ratio, period);
            // The output is the same length as the input. That is the whole
            // reason this is not a resampler.
            assert_eq!(out.len(), input.len());
            if pass >= 6 {
                tail.extend_from_slice(&out);
            }
        }
        // The target has to be *there*, and the original has to be gone: a
        // shifter that adds the new pitch on top of the old one is a
        // harmoniser, and this is not one.
        let at_target = goertzel(&tail, hz * ratio, sr);
        let at_input = goertzel(&tail, hz, sr);
        assert!(
            at_target > 0.05,
            "ratio {ratio}: nothing at {:.1} Hz ({at_target:.4})",
            hz * ratio
        );
        if (ratio - 1.0).abs() > 0.01 {
            assert!(
                at_target > at_input * 4.0,
                "ratio {ratio}: {:.1} Hz is {at_target:.4} but {hz} Hz is still {at_input:.4}",
                hz * ratio
            );
        }
    }
}

#[test]
fn an_unvoiced_block_passes_through_the_shifter() {
    let mut s = RetuneShifter::new(48_000.0);
    let frames = 2048;
    let input: Vec<f32> = (0..frames)
        .map(|i| ((i % 97) as f32 / 97.0) - 0.5)
        .collect();
    let mut out = vec![0.0; frames];
    // Long enough for the latency to clear.
    for _ in 0..4 {
        s.process(&input, &mut out, 1.5, 1.5, 0.0);
    }
    // With no period there is nothing to cut grains on, so the input is what
    // comes back — delayed, but itself.
    let energy: f32 = out.iter().map(|x| x * x).sum();
    let want: f32 = input.iter().map(|x| x * x).sum();
    assert!(
        energy > want * 0.5,
        "unvoiced audio must not be swallowed: {energy} vs {want}"
    );
    assert!(out.iter().all(|x| x.is_finite()));
}

#[test]
fn the_shifter_never_emits_a_nan() {
    let mut s = RetuneShifter::new(48_000.0);
    let mut out = vec![0.0; 512];
    let nasty = vec![f32::NAN; 512];
    s.process(&nasty, &mut out, f32::NAN, f32::NAN, f32::NAN);
    assert!(
        out.iter().all(|x| x.is_finite()),
        "NaN in must not be NaN out"
    );
    let big = vec![1e30f32; 512];
    s.process(&big, &mut out, 1e30, 1e30, 1e30);
    assert!(out.iter().all(|x| x.is_finite()));
}

// ─── Phase 5: the effect ────────────────────────────────────────────────────

/// Run a tone through a whole AutoTune and hand back the last block.
fn run(at: &mut AutoTune, hz: f32, sr: f32, blocks: usize) -> Vec<f32> {
    let mut t = Tone::new();
    let mut last = Vec::new();
    for _ in 0..blocks {
        let mut buf = t.voice(hz, sr, 512, 0.2);
        at.process_block(&mut buf, sr as u32);
        last = buf;
    }
    last
}

/// The same, on a **sine**, keeping the tail as one long mono buffer.
///
/// A harmonic-rich signal crosses zero several times per period, so counting
/// crossings on it measures the loudest partial rather than the pitch. The
/// detector is tested on the harmonic signal; the *output frequency* is
/// measured on a sine, where the estimator means what it says.
fn run_sine(at: &mut AutoTune, hz: f32, sr: f32, blocks: usize) -> Vec<f32> {
    let mut t = Tone::new();
    let mut tail = Vec::new();
    for i in 0..blocks {
        let mut buf = t.block(hz, sr, 512, 0.5);
        at.process_block(&mut buf, sr as u32);
        if i >= blocks / 2 {
            tail.extend(buf.as_chunks::<2>().0.iter().map(|f| (f[0] + f[1]) * 0.5));
        }
    }
    tail
}

#[test]
fn a_sharp_note_is_pulled_to_the_note_it_should_be() {
    let sr = 48_000.0;
    let mut at = AutoTune::new(sr);
    at.params.retune_speed_ms = 5.0;
    at.params.scale = ScaleType::Chromatic;
    at.apply_params();

    // 445 Hz: a little sharp of A4.
    let mono = run_sine(&mut at, 445.0, sr, 60);
    let m = at.reading();
    assert!(m.voiced, "a sung A is voiced");
    assert!(
        (m.target_frequency - 440.0).abs() < 0.5,
        "target {m}",
        m = m.target_frequency
    );
    assert!(
        m.pitch_error_cents > 5.0,
        "it knows it is sharp: {} cents",
        m.pitch_error_cents
    );

    let got = zero_crossing_hz(&mono, sr);
    // In tune is 440; uncorrected is 445. It has to land nearer the first.
    assert!(
        (got - 440.0).abs() < (got - 445.0).abs(),
        "corrected towards A, not left at 445: {got:.2} Hz"
    );
    assert!((got - 440.0).abs() < 2.0, "and close to it: {got:.2} Hz");
}

/// Corrected, and **clean**. A pitch corrector that lands on the right note
/// while spraying the spectrum with the grain rate is not usable, and that is
/// exactly what an unnormalised overlap-add and a jumping analysis grid sound
/// like.
#[test]
fn a_corrected_note_is_clean_and_no_louder_than_it_arrived() {
    let sr = 48_000.0;
    let mut at = AutoTune::new(sr);
    at.params.retune_speed_ms = 5.0;
    at.apply_params();
    let mono = run_sine(&mut at, 445.0, sr, 60);

    let target = goertzel(&mono, 440.0, sr);
    // Everything a grain rate would put there: the old pitch, and the
    // sidebands a jumping grid throws either side of it.
    let junk: f32 = [415.0f32, 430.0, 445.0, 460.0, 466.0, 880.0, 1320.0]
        .iter()
        .map(|f| goertzel(&mono, *f, sr))
        .fold(0.0, f32::max);
    assert!(
        target > junk * 6.0,
        "target {target:.4} vs the loudest junk {junk:.4}"
    );

    // And it did not get louder on the way. A Hann of 2P overlapped at P/ratio
    // sums to `ratio`, so without normalising, correcting *up* turns the effect
    // up — which is heard as clipping.
    let peak = mono.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!(
        peak < 0.75,
        "input peaked at 0.5; output peaks at {peak:.3}"
    );
    assert!(peak > 0.25, "and it is not swallowed either: {peak:.3}");
}

#[test]
fn correction_towards_a_neighbouring_note_lands_on_it() {
    let sr = 48_000.0;
    let mut at = AutoTune::new(sr);
    at.params.retune_speed_ms = 5.0;
    // D major has no A#, so tell it in the key where A# lives.
    at.params.key = 10; // A#
    at.params.scale = ScaleType::Major;
    at.apply_params();

    // 460 Hz sits between A and A#; in A# major the nearest member is A# 466.16.
    run(&mut at, 460.0, sr, 40);
    let m = at.reading();
    assert!(
        (m.target_frequency - 466.164).abs() < 1.0,
        "aimed at A#4, got {}",
        m.target_frequency
    );
}

#[test]
fn the_key_and_scale_decide_where_a_note_goes() {
    let sr = 48_000.0;
    // 315 Hz is between D#4 (311.1) and E4 (329.6) — nearer D#.
    let target_in = |root: u8, kind: ScaleType| {
        let mut at = AutoTune::new(sr);
        at.params.key = root;
        at.params.scale = kind;
        at.apply_params();
        run(&mut at, 315.0, sr, 40);
        at.reading().target_frequency
    };
    // Chromatic: the nearest semitone, D#4.
    assert!((target_in(0, ScaleType::Chromatic) - 311.127).abs() < 1.0);
    // C major has no D#, so E is where it goes.
    assert!((target_in(0, ScaleType::Major) - 329.628).abs() < 1.0);
}

#[test]
fn mix_at_zero_is_the_input_back() {
    let sr = 48_000.0;
    let mut at = AutoTune::new(sr);
    at.set_mix(0.0);
    at.params.retune_speed_ms = 1.0;
    at.apply_params();
    let mut t = Tone::new();
    // Warm up past the latency, then compare a block to the one that went in
    // that many samples earlier.
    let mut history: Vec<f32> = Vec::new();
    let mut out = Vec::new();
    for _ in 0..20 {
        let block = t.block(440.0, sr, 512, 0.5);
        history.extend_from_slice(&block);
        let mut buf = block.clone();
        at.process_block(&mut buf, sr as u32);
        out = buf;
    }
    let latency = at.latency_samples();
    let start = history.len() - out.len() - latency * 2;
    let expect = &history[start..start + out.len()];
    let err: f32 = out
        .iter()
        .zip(expect)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(
        err < 1e-3,
        "dry at 0 % mix must be the input, delayed: max error {err}"
    );
}

#[test]
fn silence_in_is_silence_out() {
    let sr = 48_000.0;
    let mut at = AutoTune::new(sr);
    for _ in 0..20 {
        let mut buf = vec![0.0f32; 1024];
        at.process_block(&mut buf, sr as u32);
        assert!(buf.iter().all(|x| *x == 0.0), "silence must stay silent");
    }
    assert!(!at.reading().voiced);
}

#[test]
fn nothing_that_leaves_this_effect_is_a_nan() {
    let sr = 48_000.0;
    let mut at = AutoTune::new(sr);
    for pattern in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1e30, -1e30, 0.0] {
        let mut buf = vec![pattern; 1024];
        at.process_block(&mut buf, sr as u32);
        assert!(
            buf.iter().all(|x| x.is_finite()),
            "{pattern} produced a non-finite output"
        );
    }
}

#[test]
fn it_runs_at_every_sample_rate_and_every_block_size() {
    for sr in [44_100u32, 48_000, 88_200, 96_000] {
        let mut at = AutoTune::new(sr as f32);
        let mut t = Tone::new();
        for frames in [64usize, 128, 256, 512, 1024, 37] {
            for _ in 0..8 {
                let mut buf = t.voice(220.0, sr as f32, frames, 0.2);
                at.process_block(&mut buf, sr);
                assert!(buf.iter().all(|x| x.is_finite()), "sr {sr} frames {frames}");
            }
        }
        // And it found the note despite the block size changing under it.
        assert!(at.reading().detected_frequency > 0.0, "sr {sr}");
    }
}

#[test]
fn a_sample_rate_change_mid_stream_is_survivable() {
    let mut at = AutoTune::new(48_000.0);
    let mut t = Tone::new();
    for _ in 0..10 {
        let mut buf = t.voice(220.0, 48_000.0, 256, 0.2);
        at.process_block(&mut buf, 48_000);
    }
    for _ in 0..10 {
        let mut buf = t.voice(220.0, 96_000.0, 256, 0.2);
        at.process_block(&mut buf, 96_000);
        assert!(buf.iter().all(|x| x.is_finite()));
    }
}

#[test]
fn changing_the_note_does_not_click() {
    let sr = 48_000.0;
    let mut at = AutoTune::new(sr);
    at.params.retune_speed_ms = 40.0;
    at.apply_params();
    let mut t = Tone::new();
    // Warm up on one note, then walk to another and watch the sample-to-sample
    // jump. A click is a discontinuity, so that is what is measured.
    for _ in 0..20 {
        let mut buf = t.voice(220.0, sr, 512, 0.2);
        at.process_block(&mut buf, sr as u32);
    }
    let mut worst: f32 = 0.0;
    let mut prev = 0.0f32;
    for hz in [233.08f32, 246.94, 261.63] {
        for _ in 0..10 {
            let mut buf = t.voice(hz, sr, 512, 0.2);
            at.process_block(&mut buf, sr as u32);
            for f in buf.as_chunks::<2>().0 {
                worst = worst.max((f[0] - prev).abs());
                prev = f[0];
            }
        }
    }
    // The signal itself steps by up to ~0.06 between samples at these
    // frequencies; anything near full scale would be a click.
    assert!(worst < 0.5, "biggest sample-to-sample step {worst}");
}

// ─── Parameters and presets ─────────────────────────────────────────────────

#[test]
fn every_parameter_is_reachable_and_survives_the_round_trip() {
    let mut at = AutoTune::new(48_000.0);
    let names: Vec<String> = at.params().iter().map(|p| p.name.to_string()).collect();
    assert_eq!(
        names[0], "Preset",
        "the order is frozen: a CC learned here stays here"
    );
    assert_eq!(names[1], "Retune");
    assert_eq!(names[2], "Correct");

    at.set_param(1, 0.5);
    assert!((at.params.retune_speed_ms - 500.0).abs() < 1.0);
    at.set_param(2, 0.25);
    assert!((at.params.correction - 0.25).abs() < 1e-6);
    at.set_param(3, 1.0);
    assert_eq!(at.params.key, 11);
    at.set_param(4, 1.0);
    assert_eq!(at.params.scale, ScaleType::Blues);
    at.set_param(5, 1.0);
    assert_eq!(at.params.mode, AutoTuneMode::HardTune);
    at.set_param(7, 0.0);
    assert!((at.params.reference_hz - 430.0).abs() < 0.01);
    at.set_param(10, 0.0);
    assert!(
        (at.params.input_gain_db + 24.0).abs() < 0.01,
        "InGain is param 10 now"
    );
    // An index nobody has is not a panic.
    at.set_param(99, 0.5);
}

#[test]
fn the_presets_are_five_different_sounds() {
    assert_eq!(PRESETS.len(), 5);
    let mut at = AutoTune::new(48_000.0);
    at.set_preset(2); // Hard Auto-Tune
    assert_eq!(at.params.mode, AutoTuneMode::HardTune);
    assert!(at.params.retune_speed_ms <= 1.0);
    at.set_preset(3); // Subtle Correction
    assert_eq!(at.params.mode, AutoTuneMode::Natural);
    assert!(at.params.correction < 1.0 && at.params.retune_speed_ms > 100.0);
    at.set_preset(4); // Robot Voice
    assert_eq!(
        at.params.mode,
        AutoTuneMode::HardTune,
        "the snap is the point of this one"
    );
    // Out of range changes nothing.
    let before = at.params.retune_speed_ms;
    at.set_preset(99);
    assert_eq!(at.params.retune_speed_ms, before);

    // And the preset knob reaches them.
    at.set_param(0, 0.0);
    assert!(
        (at.params.retune_speed_ms - 120.0).abs() < 1.0,
        "the first is Natural Vocal"
    );
}

/// **The output can never be louder than the input.** Two readers crossfaded is
/// a convex combination of two samples of the signal, so this holds for any
/// ratio and any pitch — which is the whole reason the overlap-add went.
#[test]
fn the_shifter_cannot_make_the_signal_louder() {
    let sr = 48_000.0;
    for ratio in [0.6f32, 0.94, 1.0, 1.06, 1.5] {
        let mut s = RetuneShifter::new(sr);
        let hz = 196.0;
        let mut phase = 0.0f32;
        let frames = 4096;
        let mut out = vec![0.0f32; frames];
        let mut peak = 0.0f32;
        for pass in 0..10 {
            let input: Vec<f32> = (0..frames)
                .map(|_| {
                    // Full scale, and with harmonics, so the peak is a real one.
                    let p = phase;
                    let v = 0.55 * p.sin() + 0.3 * (2.0 * p).sin() + 0.15 * (3.0 * p).sin();
                    phase = (phase + std::f32::consts::TAU * hz / sr) % std::f32::consts::TAU;
                    v
                })
                .collect();
            let in_peak = input.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            s.process(&input, &mut out, ratio, ratio, sr / hz);
            if pass >= 3 {
                peak = peak.max(out.iter().fold(0.0f32, |m, x| m.max(x.abs())));
                assert!(
                    peak <= in_peak + 1e-3,
                    "ratio {ratio}: input peaks at {in_peak:.3}, output at {peak:.3}"
                );
            }
        }
        assert!(
            peak > 0.2,
            "ratio {ratio}: and it is not swallowed either ({peak:.3})"
        );
    }
}

#[test]
fn the_meter_carries_what_the_ui_needs() {
    let sr = 48_000.0;
    meter::meter().clear();
    let mut at = AutoTune::new(sr);
    run(&mut at, 445.0, sr, 40);
    let m = meter::meter().read();
    assert!(m.voiced && m.detected_frequency > 400.0, "{m:?}");
    assert!(m.target_frequency > 0.0 && m.level > 0.0);
    assert!(m.pitch_error_cents.is_finite());
    meter::meter().clear();
    assert_eq!(meter::meter().read(), AutoTuneMeter::default());
}

#[test]
fn latency_is_reported_and_constant() {
    let at = AutoTune::new(48_000.0);
    let l = at.latency_samples();
    assert!(l > 0, "PSOLA cannot be free");
    // Two periods of 60 Hz **at this rate** — 33 ms, not the 67 ms that sizing
    // it for 96 kHz would have cost.
    assert_eq!(
        l,
        2 * (48_000.0f32 / detector::MIN_SUPPORTED_HZ).ceil() as usize
    );
    assert!(
        (l as f32 / 48.0) < 40.0,
        "{:.1} ms is the budget",
        l as f32 / 48.0
    );
    // It does not move with the note being sung, which is what stops a change
    // of note from being a jump in time.
    let mut at = AutoTune::new(48_000.0);
    run(&mut at, 220.0, 48_000.0, 20);
    assert_eq!(at.latency_samples(), l);
}

// ─── Phase 5: what a microphone in a room actually sends ────────────────────

/// A voice with a rumble under it is still a voice.
///
/// The same failure `A→M` had, found on a guitar first: under the lowest note
/// anyone sings there is a desk, a fan, a preamp — and a period detector handed
/// a 40 Hz rumble finds the rumble's period, which is a note an octave and a
/// half below what was sung. AutoTune then corrects *towards* that note, which
/// is the worst thing this effect can do.
#[test]
fn a_rumble_under_the_voice_is_not_the_note() {
    let sr = 48_000.0f32;
    let mut d = PitchDetector::new(sr);
    let (mut voice, mut rumble) = (Tone::new(), Tone::new());
    let mut last = PitchEstimate::SILENT;
    for _ in 0..120 {
        // The rumble is **louder than the note**, which is what a cheap stand
        // on a wooden floor actually does.
        let note = voice.voice(220.0, sr, 256, 0.15);
        let low = rumble.block(41.0, sr, 256, 0.45);
        let mono: Vec<f32> = note
            .as_chunks::<2>()
            .0
            .iter()
            .zip(low.as_chunks::<2>().0)
            .map(|(n, l)| (n[0] + l[0]) * 0.5)
            .collect();
        last = d.process(&mono);
    }
    assert!(last.voiced, "a sung note with a rumble under it is a note");
    let cents = 1200.0 * (last.frequency_hz / 220.0).log2();
    assert!(
        cents.abs() < 50.0,
        "220 Hz was sung; the detector heard {:.1} Hz ({cents:+.0} cents)",
        last.frequency_hz
    );
}

/// And a voice with hiss over it is still a voice.
///
/// The decimation used to be a plain average, i.e. its own anti-alias filter,
/// and a box filter leaks: sibilance and room hiss folded back down on top of
/// the note, and the detector locked onto the mixture.
#[test]
fn hiss_above_the_notes_does_not_fold_onto_them() {
    let sr = 48_000.0f32;
    let mut d = PitchDetector::new(sr);
    let (mut voice, mut hiss) = (Tone::new(), Tone::new());
    let mut last = PitchEstimate::SILENT;
    for _ in 0..120 {
        let note = voice.voice(220.0, sr, 256, 0.2);
        // 9.5 kHz: above everything the detector cares about, and exactly what
        // folds onto the notes when the only filter is an average.
        let above = hiss.block(9_500.0, sr, 256, 0.4);
        let mono: Vec<f32> = note
            .as_chunks::<2>()
            .0
            .iter()
            .zip(above.as_chunks::<2>().0)
            .map(|(n, h)| (n[0] + h[0]) * 0.5)
            .collect();
        last = d.process(&mono);
    }
    assert!(last.voiced, "hiss over a note does not un-sing it");
    let cents = 1200.0 * (last.frequency_hz / 220.0).log2();
    assert!(
        cents.abs() < 50.0,
        "220 Hz was sung; the detector heard {:.1} Hz ({cents:+.0} cents)",
        last.frequency_hz
    );
}

/// One bad window does not move the correction.
///
/// Every hop's answer used to go straight out, so a single window that found a
/// harmonic — one consonant, one door — bent the voice for that block. The
/// median of three throws it away and keeps the note.
#[test]
fn a_single_bad_window_does_not_become_the_note() {
    let sr = 48_000.0f32;
    let mut d = PitchDetector::new(sr);
    let mut t = Tone::new();
    for _ in 0..80 {
        let stereo = t.voice(220.0, sr, 256, 0.2);
        let mono: Vec<f32> = stereo.as_chunks::<2>().0.iter().map(|f| f[0]).collect();
        d.process(&mono);
    }
    let settled = d.estimate();
    assert!(settled.voiced);

    // One window of something else entirely, then straight back to the note.
    let mut other = Tone::new();
    for _ in 0..2 {
        let stereo = other.voice(330.0, sr, 256, 0.2);
        let mono: Vec<f32> = stereo.as_chunks::<2>().0.iter().map(|f| f[0]).collect();
        d.process(&mono);
    }
    let after = d.estimate();
    let moved = 1200.0 * (after.frequency_hz / settled.frequency_hz).log2();
    assert!(
        moved.abs() < 120.0,
        "one interrupted window moved the answer by {moved:+.0} cents"
    );
}

/// The pitch moves *through* a block, not at the end of one.
///
/// The corrector steps once per block. Holding its answer for the whole block
/// and then jumping is a staircase in pitch — flat, step, flat — and on a fast
/// retune it is heard as the effect being dirty rather than as the singer
/// arriving.
#[test]
fn the_shift_is_walked_across_the_block_and_not_stepped() {
    const BLOCK: usize = 512;
    let sr = 48_000.0f32;
    let hz = 220.0;
    let period = sr / hz;
    // **One continuous sine**, sliced into blocks. Restarting the phase every
    // block puts a corner in the *input*, and the shifter faithfully reproduces
    // it a latency later — which looks exactly like the click being hunted.
    let signal: Vec<f32> = (0..BLOCK * 40)
        .map(|i| 0.4 * (std::f32::consts::TAU * hz * i as f32 / sr).sin())
        .collect();

    let mut ramped = RetuneShifter::new(sr);
    let mut stepped = RetuneShifter::new(sr);
    let up = 2.0f32.powf(1.0 / 12.0);
    let (mut a, mut b) = (vec![0.0; BLOCK], vec![0.0; BLOCK]);
    // Past the latency with no shift at all, so the block under test is the one
    // where the ratio moves.
    for chunk in signal.as_chunks::<BLOCK>().0.iter().take(20) {
        ramped.process(chunk, &mut a, 1.0, 1.0, period);
        stepped.process(chunk, &mut b, 1.0, 1.0, period);
    }
    let block = &signal[BLOCK * 20..BLOCK * 21];
    ramped.process(block, &mut a, 1.0, up, period);
    stepped.process(block, &mut b, up, up, period);

    // The ramp is real: reading at a rate that grows through the block cannot
    // land on the same samples as reading at the final rate from the first one.
    assert!(
        a.iter().zip(b.iter()).any(|(x, y)| (x - y).abs() > 1e-4),
        "the ratio was not walked across the block"
    );

    // And nothing in it is a click. One sample of this sine moves by at most
    // 0.4 · 2π · 220/48000 ≈ 0.012, so anything an order of magnitude past that
    // is a corner rather than a wave — which is what a staircase in pitch, or a
    // crossfade that does not line up, sounds like.
    let biggest = |v: &[f32]| {
        v.windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max)
    };
    assert!(
        biggest(&a) < 0.05,
        "a step of {:.4} between two samples is a click",
        biggest(&a)
    );
    assert!(a.iter().all(|s| s.is_finite()));
}

/// The whole effect, on the signal a microphone in a room actually sends: a
/// voice that is flat, with a rumble under it and hiss over it.
///
/// This is the test that says the two filters are worth their cost. Without
/// them the detector locks onto the rumble or onto the folded hiss, and
/// AutoTune corrects a voice towards a note nobody sang — which is not "a bit
/// noisy", it is the effect actively making things worse.
#[test]
fn a_voice_in_a_room_is_still_corrected_to_the_note_it_meant() {
    let sr = 48_000.0f32;
    let mut at = AutoTune::new(sr);
    at.params.retune_speed_ms = 10.0;
    at.params.scale = ScaleType::Chromatic;
    at.apply_params();

    // 445 Hz — a little sharp of A4 — under a 41 Hz rumble louder than it and
    // over a 9.5 kHz hiss.
    let (mut voice, mut rumble, mut hiss) = (Tone::new(), Tone::new(), Tone::new());
    for _ in 0..80 {
        let note = voice.block(445.0, sr, 512, 0.25);
        let low = rumble.block(41.0, sr, 512, 0.45);
        let high = hiss.block(9_500.0, sr, 512, 0.2);
        let mut buf: Vec<f32> = note
            .iter()
            .zip(low.iter())
            .zip(high.iter())
            .map(|((n, l), h)| n + l + h)
            .collect();
        at.process_block(&mut buf, sr as u32);
        assert!(buf.iter().all(|s| s.is_finite()));
    }

    let m = at.reading();
    assert!(m.voiced, "a sung note in a room is still a note");
    assert!(
        (m.detected_frequency - 445.0).abs() < 10.0,
        "445 Hz was sung; it heard {:.1} Hz",
        m.detected_frequency
    );
    assert!(
        (m.target_frequency - 440.0).abs() < 0.5,
        "and aims at A4, not at whatever the room was doing: {:.1} Hz",
        m.target_frequency
    );
}

/// **The input gain is the detector's, not the signal's.**
///
/// Reported from a real microphone: a voice that tracked well came out
/// saturated, because the only knob that got the detector over its gate was
/// also multiplying the audio. Now `Sens` lifts the analysis and leaves the
/// sound where it was; `OutGain` is the level control, and it is the only one.
#[test]
fn the_sensitivity_lifts_the_analysis_and_not_the_output() {
    let sr = 48_000.0f32;
    let peak = |db: f32| -> f32 {
        let mut at = AutoTune::new(sr);
        at.params.input_gain_db = db;
        at.apply_params();
        let mut t = Tone::new();
        let mut worst = 0.0f32;
        for i in 0..60 {
            let mut buf = t.block(440.0, sr, 512, 0.2);
            at.process_block(&mut buf, sr as u32);
            // The first blocks are the shifter filling its delay line.
            if i > 40 {
                worst = worst.max(buf.iter().fold(0.0f32, |a, s| a.max(s.abs())));
            }
        }
        worst
    };

    let flat = peak(0.0);
    let lifted = peak(18.0);
    assert!(flat > 0.05, "there is a signal to compare: {flat}");
    assert!(
        (lifted - flat).abs() < 0.02,
        "18 dB of sensitivity moved the output: {lifted} vs {flat}"
    );

    // And it does reach the detector: a voice under the gate is heard once the
    // sensitivity is up, which is the whole point of the knob.
    let voiced_at = |db: f32| -> bool {
        let mut at = AutoTune::new(sr);
        at.params.input_gain_db = db;
        at.apply_params();
        let mut t = Tone::new();
        for _ in 0..80 {
            // Well under the detector's gate on its own.
            let mut buf = t.voice(220.0, sr, 512, 0.0006);
            at.process_block(&mut buf, sr as u32);
        }
        at.reading().voiced
    };
    assert!(!voiced_at(0.0), "that quiet, it is under the gate");
    assert!(voiced_at(24.0), "and the sensitivity is what gets it over");
}
