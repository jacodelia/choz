//! The things that make notes without being played: choz's own generators.
//!
//! An artifact is not an instrument and not an effect. It sits in a tab and
//! *produces* — the arpeggiator ([`arp`]) turns a held chord into a pattern,
//! the step sequencer ([`seq`]) plays one it was written, the arranger
//! ([`arranger`]) plays a part of a chord progression, and the metronome
//! ([`metronome`]) counts the bar everything else is timed against. All of them
//! run off the same transport, and the generators are published as CLAP plugins
//! alongside the effects (see `choz-plugin-clap-export`).

pub mod arp;
pub mod arranger;
pub mod metronome;
pub mod seq;
