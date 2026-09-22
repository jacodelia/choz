# The arranger's rhythms

One type-1 MIDI file per accompaniment, and one [`Style`](../style.rs) measured
off each of them. The file name is the style name: `shfblues.mid` is the style
a chart writes as `style = shfblues`.

choz does not read this folder — `styles.rs` is generated from it and that
table is what ships. The folder is here so the numbers have a source: a style
that sounds wrong is a measurement to fix, and the recording it was measured
from is the thing to listen to first.

## What a file has to look like

- **Type 1, any division.** 96 ticks per quarter note is what these use.
- **A tempo and a track name**, on the first track.
- **A marker at the start of every section**, reading `<name> <bars>bar
  <num>/<den>` — `Var1 4bar 4/4`. The measuring reads `Var1`, then `Var2`, then
  `Intro`, whichever it finds first, and a `Fill` anywhere in the file is what
  says the style takes fills at all.
- **A track name per track**, `<section> <part> [flags…]`, where the part is
  one of `Percussion`, `Drum`, `Bass`, `Chord1`…`Chord5`. Anything after the
  part is ignored by the measuring and is there to be read by a person — except
  `2bar` on the drum track (`Var1 Drum 2bar`): the groove is two bars long, the
  even bars are its first and the odd bars its answer.
- **Sections laid end to end**, so the file plays through as an arrangement.
- **A heterometric rhythm** — one that changes meter — adds a section per
  signature it changes to, marked `Meter<n> <bars>bar <num>/<den>` (`Meter3
  4bar 7/8`) with its own `Meter3 Drum`, `Meter3 Chord1`… tracks. Each one is
  measured into the style's `meters` table: how it plays a bar of that
  signature when a chart changes to it (`| 7/8 Dm |`).
- **Drums on MIDI channel 10**, General MIDI numbering, and the melodic parts
  with a program change each — the program is what the style asks a SoundFont
  for.

## Adding one

Drop the file in, then:

```sh
tools/mid_to_styles.py crates/choz-engine/src/artifacts/arranger/rhythms \
    crates/choz-engine/src/artifacts/arranger/styles.rs
cargo fmt -p choz-engine
```

Two rhythms that measure the same are one style; two that only share a name
are two, and the second one gets a number.
