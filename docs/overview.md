# choz overview

What choz does today, in more detail than the README's feature table.

**1.3.18.** The FX engine, the rack and the TUI are real and working, **CLAP, LV2,
LADSPA, DSSI, VST2, VST3 and Pure Data patches are really hosted** — instruments
and audio effects, with their own parameters and their own windows — choz's own
56 effects and all four artifacts — the arpeggiator, the step sequencer, the
metronome and the arranger — are
published as CLAP plugins for other hosts, and choz installs as a
`.deb`, an `.rpm` or a script, with an entry in the desktop menu.

choz is a **citizen of the JACK/PipeWire graph**, not only of ALSA:
`choz:midi_in` and `choz:midi_out` are ports on the client it already had, so a
DAW on the same graph plays a rack tab, sends it the clock and receives what the
arpeggiator puts out — no `a2jmidid`, which bridges the other way. On ALSA it
**publishes a port of its own**, `choz MIDI IN`: a DAW picks it out of its MIDI
output list and plays a tab with no loopback module and no graph at all. And a tab can
**leave the master mix through a port of its own**: one direct out per tab, at a
fixed place and as wide as the tab is, which is what a DAW records track by
track. A mono jack stays one channel all the way through — one fader on the
mixer, one port on the graph — until something in its chain has a reason to pull
the two sides apart.

Plus **56 built-in DSP effects** — including a real-time pitch corrector, a pitch shifter, a Dattorro plate reverb, a Moog ladder filter and a three-band compressor on a Linkwitz-Riley crossover — and WAV playback as a rack source.

Plugin windows embed into a real X11 window on choz's editor thread — no suil,
no Steinberg SDK. Verified by counting the parent window's actual X11 children,
not by trusting return values: **20 of 20 CLAP** and **20 of 21 VST3** plugins
installed here open at the size they ask for (Surge XT included), and **254 of
259 LV2 editors** in a full sweep with no crashes (the other 5 do not
instantiate at all — sequencers with no audio output).

Whatever a plugin's window can do, choz can do without opening it: every
parameter is a knob in the RACK, and MIDI learn binds to those knobs directly.
What is drawn comes from the plugin, including the two things it is easy to get
wrong: a parameter it says cannot be automated is **not** a knob (Surge XT
publishes 191 `MIDI CC` rows that do nothing), and a parameter whose whole range
only ever reads as two words is a **switch**, not a fader, even when the plugin
reports no steps at all — which Surge does for all 800 of its parameters.

A parameter whose positions have **names** opens its list rather than counting.
LV2, CLAP, VST3 and VST2 report them; LADSPA and DSSI cannot — the ABI has no
call that says how a value reads — so choz reads them where every other host
does, from the `.rdf` installed beside the plugin. That is `tap_reverb`'s 43
reverb types, caps' 25 cabinets and nine tonestacks, as names instead of
numbers.

A synth with more parameters than the box can show (Surge XT has hundreds) gets
`◀` `▶` on the box's top edge, and **the CCs already learned move with the
box**: the fader on the first knob of one page is on the first knob of the next,
so eight faders reach every parameter the plugin has instead of eight of them
for good. That happens however the box moved — the arrows, `PgUp` / `PgDn`, a
CC bound to either (they are learn targets like every other button), the cursor
walking off the edge, a resize. A plugin whose patches are **files** rather than
programs has them found by name: the bank button opens straight onto the
categories its own window shows — `Basses`, `Leads`, `Pads` for Surge XT's 637
`.fxp`, `01 Basses`, `02 Leads` for TyrellN6's 669 `.h2p` (u-he's text patches
*are* the plugin's state, so they load like any other) — and any other folder is
one pick away, saved with the project. A plugin that publishes 128 slots called
`Program 0` is treated as publishing nothing, because it is.
Parameters moved *inside* the plugin's window are followed too (VST3
`IComponentHandler`, VST2 `audioMasterAutomate`, CLAP output events, the LV2 UI
write callback), so "move that knob, then move a fader" is a complete binding.

Projects save what a parameter list cannot: the plugin's **own state** — the
patch picked in its browser — through VST2 chunks, VST3 `IComponent::getState`,
`clap.state` and LV2 `state#interface`. And what no parameter can hold at all:
the looper's **takes**, written as WAVs into `<project>.loops/` beside the file,
so moving a project is moving the `.yml` and its directory. A take recorded at
another sample rate is resampled to the device's on load, keeping its pitch and
its length in seconds. The project stays a YAML you can read, diff and commit;
the audio lives next to it.

Playing rather than patching: a **MIXER** tab at the bottom shows every rack tab
at once as channel strips — **one vertical fader per output channel** with a
link between them (tied by default, broken to trim one side against the other),
pan, and an `O M S` row under the fader — where the tab sums, mute, solo — each
editable where it is drawn instead of one tab at a time, moved by the wheel or
the arrows in the same step the RACK's `VOL` uses, and paging with `◀ ▶` when
the rack is wider than the panel. **The arrows walk the whole desk**: past the
last tab come the four groups and the main, so a machine that is played rather
than pointed at can reach every fader; a **metronome** beside the LIVE/MULTI switch clicks off the same
transport every synced plugin reads (tempo, time signature, three sounds), and
it keeps counting with the transport stopped, which is when a metronome is
wanted; every tab carries a **step sequencer built like an Alesis MMT-8** —
eight tracks, sixteen steps, eight parts and a song chain, drawn above the
instrument because that is the order the notes travel in, with `REC` writing what
you play quantised to the step the playhead is on, and every step handed to the
tab's arpeggiator when it has one running; and the arpeggiator's **HOLD** works
the way a Keystep's does — let go
and the chord keeps playing, and the next key pressed with nothing down starts a
new one rather than piling onto the old.
