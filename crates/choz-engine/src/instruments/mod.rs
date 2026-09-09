//! The things that make notes into sound, without being somebody else's plugin.
//!
//! choz hosts six plugin formats, and these are what it does *not* need a
//! plugin for: a SoundFont ([`sf2_patch`], with the synth itself in
//! [`crate::sources`]), an SFZ instrument ([`sfz`]), and a folder of samples
//! read into an instrument on its own ([`sampler`]).
//!
//! They share more than a shelf. [`sampler`] builds its map out of
//! [`sfz::SfzRegion`]s and plays it with [`sfz::SfzSampler`] — one player, two
//! ways of deciding what it plays — and both of them decode through the same
//! path.

pub mod sampler;
pub mod sf2_patch;
pub mod sfz;
