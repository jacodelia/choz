# AutoTune — real-time pitch correction

A built-in FX, not a wrapper. Audio in, the same audio in tune, at the rack's
own latency plus 33 ms. Add it the way you add any other effect:

```text
a → PITCH → AUTO-TUNE
```

## What it is for, and what it is not

**Monophonic sources only** — a voice, a bass, a lead line. A chord has more
than one pitch and this reports one. That is not a limitation to be lifted
later: a monophonic tracker answering "which pitch" for a chord is guessing, and
polyphonic correction is a different algorithm and a different effect.

It corrects **tuning**, not timing, and it does not try to be a harmoniser: the
target pitch replaces the original rather than joining it.

## The chain

```text
in ─► [detector]  F0, confidence, voiced
         │
         ▼
    [quantizer]   key + scale ─► target note ─► target Hz
         │
         ▼
    [corrector]   retune speed, correction, humanise ─► pitch ratio
         │
         ▼
    [shifter]     PSOLA, one per channel ─────────────────► out
```

Each stage is a file under `crates/choz-engine/src/fx/autotune/`, because each
is replaceable on its own — the shifter especially, which is behind a
`PitchShifter` trait for exactly that reason.

## Pitch detection

**YIN** — the cumulative mean normalised difference — over a ring buffer, with
parabolic interpolation of the dip. It is `crate::pitch::yin`, the same function
the `A→M` button uses; written once, called twice.

Plain autocorrelation was tried first and is not enough: a squared difference
dips at *every* short lag on a smooth signal, and a guitar's low E came out an
octave and a half up. YIN divides each lag by the running mean of the ones
before it, which flattens those and leaves the real period as the first dip
under the threshold.

### Decimation, and why it is not optional

At 48 kHz with a window long enough for 60 Hz, one analysis is ~2.2 million
operations. A 256-sample hop asks for 187 of them a second: **410 million
operations per second, for one voice.** That is not "a bit slow" — it is xruns,
a starved plugin and a detector that looks broken because the callback is
missing its deadline. (Measured: it is exactly how the `A→M` tracker failed
before it was rewritten.)

The analysis therefore runs at **16 kHz**, reached by averaging `round(sr/16000)`
samples into one — which is both the downsample and its anti-alias filter. A
guitar's top note is 1.3 kHz and a voice's is lower, so nothing above 8 kHz says
anything about the period. The same analysis costs ~117 k operations: **thirty
times less**. Frequency precision survives because the dip is interpolated, and
the shifter is handed the period back at the full rate as `sample_rate / f0`.

### Voiced, unvoiced, and octaves

* An RMS **gate** (-50 dBFS by default) decides there is nothing to detect at
  all. Silence and room tone are unvoiced whatever YIN thinks of them.
* **Confidence** is the dip depth. Below the threshold the estimate is unvoiced,
  the correction target falls back to *no correction*, and the smoother walks
  back there rather than snapping — a consonant must not click.
* An **octave check**: if the reading is within a few cents of double or half
  the last accepted one, and far from it otherwise, the continuous answer is
  taken. A singer does not jump an octave between two 8 ms hops and land exactly
  in tune.

## Frequency and notes

`ftom`, the standard conversion:

```text
midi = 69 + 12·log2(f / A4)          f = A4·2^((midi − 69)/12)
```

`A4` is a parameter (430–450 Hz, 440 by default) because not everyone tunes to
440.

### Scales

`Chromatic`, `Major`, `Minor`, `Pentatonic Major`, `Pentatonic Minor`, `Blues`,
each as semitone offsets from the key's root. The target is the **nearest note
in the scale to the fractional note number** — fractional on purpose, because a
singer 40 cents sharp of F in C major is nearer F than G, and rounding first
would throw away the only information that says so. Ties go to the lower note.

Chromatic is the default: it corrects tuning without deciding what key the song
is in.

## Correction

Everything happens in the **log domain** — semitones, not hertz. Smoothing a
frequency linearly slides through the wrong notes on the way: an octave is a
doubling, so "half way there" in Hz is a different interval depending on where
you started.

* **Retune Speed** (0–1000 ms) is the time constant of a one-pole: ~63 % of the
  way there in that time. Never zero — a step in pitch ratio is a click, so the
  floor is 1 ms.
* **Correction** (0–100 %) is how much of the error is taken at all. 50 % is a
  singer who is nearly in tune, not a singer corrected half the time.
* **Humanize** (0–100 %) wanders the retune *time* with a slow LFO, so held
  notes do not all converge along the same mechanical curve. It deliberately
  does not move the pitch: that would be a vibrato the singer did not sing.
* **Mode** — `Natural` uses the knobs as set. `Hard Tune` ignores them: 1 ms and
  100 %, because it is one sound, and a Hard Tune that can be set to 400 ms is
  Natural with a confusing name.

## Pitch shifting — a variable-rate reader with period jumps

This is the shape **zita-at1** uses (Fons Adriaensen), and x42's `fat1.lv2`
after it. It was chosen after the first attempt — PSOLA — failed on a real
voice.

* One **read pointer** walks a delay line at `ratio` samples per output sample.
  That is the pitch shift, and it is exact.
* Walking at a rate other than 1 makes the pointer drift towards or away from
  the writer. When it leaves its window it **jumps by a whole number of pitch
  periods**, which lands it on the same phase of the waveform.
* The jump is a **crossfade** between the old position and the new one, over a
  raised cosine, about one period long. Nothing is discontinuous, so nothing
  clicks.
* Fractional positions are read with cubic (Catmull-Rom) interpolation.

```text
write ─────────────────────────────────────►  now
                   r1 ──►                     reads at `ratio`
       r2 ──►                                 …and during a jump, both,
       └── crossfade ──┘                      blended by a raised cosine
```

Reading at `ratio` moves the whole spectrum, which is why a resampler *alone*
would shorten the sound; here the time is put back by the periodic jumps, and
only the pitch is left moved.

### Why the overlap-add went

The first version was PSOLA: grains windowed with a Hann and summed. It measured
perfectly on a sine and misbehaved on a voice, in three ways that all come from
one place — **a sum of windowed copies has a gain**, and that gain depends on
the grain spacing, the window length and how well consecutive grains line up.
On a signal whose period is moving, all three wobble, and the output is both
dirty and *louder than the input*. Louder than the input is heard as the effect
clipping, and no amount of adjusting the window fixes the class of problem.

Two readers crossfaded cannot do it: the output is a convex combination of two
samples of the input, so **`|out| ≤ max |in|`**, for any ratio and any pitch.
That is a property of the method, not a tuning, and there is a test that asserts
it at five ratios.

Two bugs found and fixed inside PSOLA before it was replaced are worth keeping
in mind, because they are the kind a spectrum plot finds and an ear does not:

* The synthesis clock re-anchored itself whenever it fell "behind", and since
  the mark is fractional it is always behind by a fraction — so the analysis
  grid was re-anchored on *every* grain and the overlap-add reconstructed the
  input. A ratio of 1.5 produced 220 Hz from 220 Hz, spectrally pure, with the
  grain spacing perfectly correct.
* The analysis mark was snapped to a grid computed from the current `P`. `P`
  moves with the singer, so the grid moved, and the grain source jumped by up to
  half a period every time the pitch did.

### Formants

They move with the pitch: this is a resampler, so a shift of `r` moves the
spectral envelope by `r` too. At the ratios a *corrector* lives at — a semitone
is 6 % — that is inaudible, and it is what zita-at1 does.

There is **no formant switch**. A formant-preserving path would be a different
implementation of `PitchShifter`, which is what that trait is for; a switch that
does nothing is worse than no switch.

### Latency

A grain placed at `s` needs input up to `s + 2P`, so the output runs **two of
the longest periods** behind the input: 60 Hz at the running sample rate, which
is **1600 samples — 33.3 ms at 48 kHz**, reported by `AutoTune::latency_samples()`.

It is two periods at the *running* rate, not at the highest rate the buffers are
sized for; that mistake cost 67 ms at 48 kHz for nothing. And it does not follow
the note being sung: a latency that moved with the pitch would be a time
machine, and every change of note would click.

The dry signal is delayed to match before it is mixed. Mixing an undelayed dry
with a wet 33 ms late is a comb filter, not a mix.

## Stereo

The pitch is decided on the mono sum `(L + R) / 2`; each channel then gets its
own shifter driven by that one control signal, so the stereo image survives.

## Realtime safety

Every buffer is allocated in `AutoTune::new`, sized for the longest period
(60 Hz) at the highest supported rate (96 kHz). `process_block` allocates
nothing, locks nothing and blocks on nothing — including when the sample rate
changes underneath it: the buffers are already big enough, so a rate change
costs a `reset` and no more. A block longer than the scratch is walked in
chunks rather than reallocating.

This is **measured, not promised**. `examples/autotune_bench.rs` installs a
counting global allocator and takes the count around `process_block` alone:

```sh
cargo run --release --example autotune_bench -p choz-engine
```

## Benchmark

Ryzen laptop, release build, one voice, warm:

| Sample rate | Frames | µs / buffer | % of budget | Allocations |
|---|---|---|---|---|
| 44.1 kHz | 64 | 119 | 8.2 % | **0** |
| 44.1 kHz | 256 | 375 | 6.5 % | **0** |
| 48 kHz | 64 | 130 | 9.8 % | **0** |
| 48 kHz | 128 | 269 | 10.1 % | **0** |
| 48 kHz | 256 | 405 | 7.6 % | **0** |
| 48 kHz | 512 | 819 | 7.7 % | **0** |
| 96 kHz | 64 | 69 | 10.4 % | **0** |
| 96 kHz | 512 | 426 | 8.0 % | **0** |

Around a tenth of one core for one voice, at every rate and block size tested.
That is comfortably realtime and it is not fast: the grain loop is the cost, and
it is written plainly. Optimising it is a later job than getting it right.

## Parameters

| # | Name | Range | Notes |
|---|---|---|---|
| 0 | Preset | 5 named | Fills in the knobs below it |
| 1 | Retune | 0–1000 ms | Time constant of the glide |
| 2 | Correct | 0–100 % | How much of the error is taken |
| 3 | Key | C…B | Root of the scale |
| 4 | Scale | 6 named | Chromatic by default |
| 5 | Mode | Natural / Hard Tune | Hard ignores Retune and Correct |
| 6 | Human | 0–100 % | Wanders the retune time, not the pitch |
| 7 | A4 | 430–450 Hz | Reference |
| 8 | MinHz | 60–400 Hz | Detector range; narrower is safer and faster |
| 9 | MaxHz | 400–1200 Hz | |
| 10 | InGain | ±24 dB | |
| 11 | OutGain | ±24 dB | |
| — | Wet | 0–100 % | choz's own per-FX mix, not a second one |

The order is frozen: a CC learned on "Retune" has to stay on Retune.

## Presets

| Preset | Retune | Correct | Human | Mode |
|---|---|---|---|---|
| Natural Vocal | 120 ms | 85 % | 25 % | Natural |
| Fast Vocal | 35 ms | 100 % | 10 % | Natural |
| Hard Auto-Tune | 1 ms | 100 % | 0 % | Hard |
| Subtle Correction | 300 ms | 50 % | 40 % | Natural |
| Robot Voice | 4 ms | 100 % | 0 % | Hard |

Key, scale and the frequency range belong to the song rather than to the preset,
so a preset leaves them alone.

Picking a preset **writes** those values into the FX's parameter array, because
that array is what the project saves and what the chain is rebuilt from.
Telling only the processor would make the preset last exactly until the next
knob was touched.

## The readout

Under the knobs, when AutoTune is the selected FX:

```text
  IN ██████░░  -18.4dB      A#3 233 → B3  247  +42¢
  0¢ ────╱▔▔╲──────────────────────────────────────
```

Level, the note heard, the note aimed at, the error in cents, and where that
error has been. It comes from a lock-free meter the audio callback publishes
(`fx::autotune::meter`) — six relaxed stores and nothing else. Nothing crosses
back the other way, which is why a graph of the pitch costs the callback
nothing.

## MIDI targeting

`PitchTarget::MidiNote(u8)` exists and the quantiser honours it, so "sing into a
note held on a keyboard" is a routing change rather than a change to the shape
of anything. The routing itself is **not** implemented: choz has no path from a
note input into an FX slot today.

## Limitations, stated plainly

* Monophonic only.
* 33 ms of latency at 48 kHz, inherent to a PSOLA sized for 60 Hz.
* Formants move with the pitch. Inaudible at correction-sized ratios, wrong at
  wide ones — this is a corrector, not a harmoniser.
* Wide shifts in general are outside what this is for — it is a corrector, and
  the ratios it spends its life at are within a semitone or two of 1.
* Not yet tried against a real singer through a real microphone. Every test here
  is a synthetic signal, and synthetic signals are kinder than a room.
