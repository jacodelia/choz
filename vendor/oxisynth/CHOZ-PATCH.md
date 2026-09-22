# oxisynth 0.1.0, patched for choz

Upstream: https://github.com/PolyMeilex/oxisynth (LGPL-2.1, see `LICENSE`).
This directory is the crates.io release 0.1.0 with one addition, and nothing
else changed. It is used through `[patch.crates-io]` in the workspace
`Cargo.toml`.

## The change

oxisynth already renders every voice into the audio group of its MIDI channel
(`channel % audio_groups`, `core/write/mod.rs::write_voices`), but its public
API only ever reads group 0 — so with more than one group, every channel past
the first is rendered and thrown away. Two public calls expose them:

- `Synth::read_next_groups(&mut self, out: &mut [(f32, f32)])` — the next
  sample of every group, reverb and chorus returns mixed into group 0 exactly
  as `read_next` does.
- `Synth::audio_groups(&self) -> usize`.

choz builds its SoundFont synth with one group per MIDI channel it uses, and
sums the groups itself — through a gain per channel, which is what puts each
musician of the arranger's band on a mixer strip of their own.
