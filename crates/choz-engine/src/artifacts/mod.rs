//! The things that make notes without being played: choz's own generators.
//!
//! An artifact is not an instrument and not an effect. It sits in a tab and
//! *produces* — the arpeggiator ([`arp`]) turns a held chord into a pattern,
//! the step sequencer ([`seq`]) plays one it was written, and the metronome
//! ([`metronome`]) counts the bar everything else is timed against. All three
//! run off the same transport, and two of them are published as CLAP plugins
//! alongside the effects (see `choz-plugin-clap-export`).

pub mod arp;
pub mod metronome;
pub mod seq;
