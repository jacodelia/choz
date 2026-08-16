# CHOZ

A terminal-based audio plugin host for the terminal.

"Choz" can mean a few different things depending on the context:

* Tagalog Slang (Philippines): It is an informal spelling or variation of chos (from echos), meaning "just kidding," "joking," or used to brush off a statement as a joke.
* Urban/Regional Slang: In some British regional dialects, it has historically been used to mean something "good" or "brilliant".
* Spanish (Colloquial/Archaic): According to the Diccionario de la lengua española (RAE), choz can mean a sudden surprise, blow, or a state of amazement.
* Zoning (Urban Planning): It can stand for an acronym like Central Heber Overlay Zone (a specific municipal planning term).
* Toys/Media: It is frequently used as shorthand for "Cho-Z" (Super Z) series parts or spinning tops in franchises like Beyblade Burst.

Choose the meaning that suits you best!

Built with Rust, ratatui and cpal. Provides a TUI for managing note inputs, instruments and real-time FX chains.

---

## Status

**1.1.0.** The FX engine, the rack and the TUI are real and working, **CLAP, LV2,
LADSPA, DSSI, VST2, VST3 and Pure Data patches are really hosted** — instruments
and audio effects, with their own parameters and their own windows — choz's own
45 effects are published as a CLAP plugin for other hosts, and choz installs as a
`.deb`, an `.rpm` or a script, with an entry in the desktop menu.

### Plugin formats

| Format | Scan | Instrument | Effect | Native window |
|---|---|---|---|---|
| **LV2**    | ✅ | ✅ | ✅ | ✅ `ui:X11UI`, no suil |
| **VST2**   | ✅ | ✅ | ✅ | ✅ `effEditOpen` |
| **CLAP**   | ✅ | ✅ | ✅ | ✅ `clap.gui` + host timer |
| **VST3**   | ✅ | ✅ | ✅ | ✅ `IPlugView` + Linux run loop |
| **LADSPA** | ✅ | — | ✅ | ❌ (format has no GUI) |
| **DSSI**   | ✅ | ✅ | ✅ | ❌ |
| **SFZ**    | ✅ | ✅ | — | — |
| **SF2**    | ✅ | ✅ (oxisynth) | — | — |

Plus **35 built-in DSP effects** — including a real-time pitch corrector — and WAV playback as a rack source.

Plugin windows embed into a real X11 window on choz's editor thread — no suil,
no Steinberg SDK. Verified by counting the parent window's actual X11 children,
not by trusting return values: **20 of 20 CLAP** and **20 of 21 VST3** plugins
installed here open at the size they ask for (Surge XT included), and **254 of
259 LV2 editors** in a full sweep with no crashes (the other 5 do not
instantiate at all — sequencers with no audio output).

Whatever a plugin's window can do, choz can do without opening it: every
parameter is a knob in the RACK, and MIDI learn binds to those knobs directly.
Parameters moved *inside* the plugin's window are followed too (VST3
`IComponentHandler`, VST2 `audioMasterAutomate`, CLAP output events, the LV2 UI
write callback), so "move that knob, then move a fader" is a complete binding.

Projects save what a parameter list cannot: the plugin's **own state** — the
patch picked in its browser — through VST2 chunks, VST3 `IComponent::getState`,
`clap.state` and LV2 `state#interface`.

### Safety net

Plugins are third-party C code, and some of it crashes. choz handles that in
three layers, all measured against what is installed here:

- **Scanning runs out of process** — one child per (format, directory); if it
  dies, the parent retries entry by entry, so only the broken plugin is lost.
- **Quarantine** — the first time a plugin is loaded it is tried in a child
  process, and the verdict is cached. Dying on *load* means choz refuses it;
  dying on *teardown* means it is loaded and then leaked on purpose.
- **Sandbox** — a plugin can run in its own process, exchanging audio over shared
  memory with a deadline. If the child dies mid-note, a supervisor thread
  restarts it: a click instead of a dead tab. **Its window opens in that process
  too**, which is where third-party code crashes most.

**Having a window is reason enough to be sandboxed**, whatever the probe saw:
plugin GUIs are the least trustworthy code choz runs (on this machine all 31
guitarix UIs segfault whatever loads them), and the probe asks each plugin
whether it has one without ever opening it. `CHOZ_SANDBOX_GUI=0` turns that half
of the policy off.

### The controls follow the parameter

A plugin does not only have knobs, and choz does not draw only knobs. What each
parameter *is* comes from the plugin — never from its name — and decides the
control: `lv2:portProperty`/`scalePoint` and `units:unit` in LV2, `stepCount` and
`units` in VST3, `IS_STEPPED` in CLAP, port hints in LADSPA/DSSI.

| The plugin says | choz draws |
|---|---|
| two positions | a switch (`[ ON ]` / `[ OFF ]`), a checkbox in long lists |
| named steps | `◀ Sine ▶` with `1/3` under it |
| a time, a percentage | a horizontal fader — a travel, not a setting |
| three or more of those in a row, same unit | a bank of vertical bars: an ADSR or an EQ read as a profile |
| anything else | the knob's arc, at eight positions per cell |

Arrows and the wheel move a stepped parameter **one position**, and every control
is clickable and MIDI-learnable, bank or not.

### Any jack to any jack

An interface's channels are not glued together in pairs, so choz stops pretending
they are. Both drawers list **one row per channel**, and a tab's channels go on
and off one at a time:

| | |
|---|---|
| `Enter` or `Space` | put this channel on the tab, or take it off |
| left click | put it on |
| right click | take it off |

A tab holds up to two channels — the first is its left, the second its right, and
the rows say `L`, `R` or `L+R` so the routing reads off the panel. A new tab
starts on 1 and 2, so clicking **3** and then **9** leaves it playing out of 3
and 9. Taking the last channel off an input puts the tab back on its instrument;
an output always keeps one, because a tab has to come out somewhere.

Assigning an input **starts a rack tab** if there is none, the same way binding a
MIDI port does — a guitar has no port to bind, and an empty rack has nothing to
assign to.

**Every capture jack in the system is listed**, grouped by the card that owns it:
a UMC1820's eight inputs, the laptop's microphone and the second card all at
once. There is no "input device" to choose — choz wires them all and you pick a
channel. `r` in the IN drawer re-reads the graph, for a card plugged in after
choz started.

### One wash, every panel

Every section — IN/OUT, RACK, FX, TRANSPORT, the monitor — shares one
translucent coloured background, set in `Settings → THEME`:

- **Panel colour** — the scheme's own by default, so a washed UI still looks
  like the theme rather than like a filter over it. `←`/`→` walk the palette.
- **Panel opacity** — 0 % leaves the desktop untouched, 100 % hides it.

A terminal cell background has no alpha, so "semi transparent" is resolved to a
real colour before it is painted: over an image each cell blends with what the
picture shows there, over a flat colour it is computed once. On the terminal's
own background neither row appears — choz cannot read that colour, so there is
nothing to blend with.

### AutoTune, built in

A real-time pitch corrector as a built-in FX, not a wrapper: `a → PITCH →
AUTO-TUNE`. `Preset`, `Key`, `Scale` and `Mode` open a picker on Enter or a
click — they are lists, not knobs. YIN finds the pitch, a key and scale decide the note it should be,
and a **variable-rate delay reader** moves it there — zita-at1's method: it
walks the line at the correction ratio and jumps whole pitch periods with a
crossfade, so the note keeps its length. The output is a blend of two reads of
the input, so it can never come out louder than it went in.

`Retune` is the glide (0–1000 ms), `Correct` how much of the error is taken,
`Humanize` stops held notes converging along the same mechanical curve, and
`Mode` picks Natural or the Hard Tune effect. Five presets, from *Natural Vocal*
to *Robot Voice*. Under the knobs it shows what it hears — the note, the target,
the error in cents, and a trace of where that error has been.

**Monophonic**, 33 ms of latency at 48 kHz, and about a tenth of a core per
voice — measured, along with **zero allocations** in the audio callback, by
`cargo run --release --example autotune_bench -p choz-engine`. Full write-up in
[`docs/autotune.md`](docs/autotune.md).

### A guitar into a synth

A rack tab fed by an audio input passes that audio through its FX. Press **`A→M`**
on the instrument line — the button only appears where there *is* an input — and
the tab listens instead: the pitch it hears becomes note-ons for its own
instrument, so a guitar, a bass or a voice plays Surge XT like a keyboard.

It is **monophonic**, which is what pitch tracking can do honestly: one period is
one frequency, and a chord has several. The detector is YIN, decimated to 16 kHz
and run once every 8 ms — no extra latency, no allocation, and a small enough
slice of the audio callback that the plugin it drives still gets its own.

The conversion is **Csound's `ftom` with `irnd` non-zero** — *"the result is
rounded to the nearest integer"*. One jack in, one exact note out, the way a
keyboard sends it. Rounding alone would not do: a pitch resting on a semitone
boundary rounds up and down as it wobbles, and every flip would be a note-on. So
a note only changes when the new one is clearly the nearest (20 cents past the
halfway point) *and* has held for three readings — a singer's vibrato is one
note held, not a run of them.

The button says what it is hearing — ` A→M● E2-14`, the note and how many cents
off it is, or the input level in dB when nothing is sounding. That is the number
**`SENS`** is set against; the cents are a display, what reaches the plugin is
the note. And `A→M` plays the tab's **instrument**, not its FX chain: with no
instrument loaded the rack says so rather than going quietly silent.

A tab fed by audio has its own **`IN`** trim and **`SENS`** — how hard the
instrument has to be hit before `A→M` calls it a note (-70 to -20 dBFS). A guitar
through a preamp is nowhere near a synth's level, so both are knobs on the mixer
strip: scroll them, `<`/`>` and `;`/`:`, bind them to a CC, automate them.

### Ten sliders and eighteen presets

**GRAPHIC EQ** is tanu's ten-band Winamp EQ, with Winamp's own preset list — Rock, Techno, Full Bass, all eighteen. Here
each band is a choz parameter, so a CC can ride a single band and any band can be
automated; the preset picker is a knob like the rest, and a band moved after a
preset wins over it.

### A clock of its own

choz has a transport — position, tempo, time signature, bar, play state — and hands it
to every format that asks: VST2 `audioMasterGetTime`, VST3 `processContext`,
CLAP's transport event, and for LV2 a `time:Position` object written into the
plugin's atom port (LV2 has no callback for this; the host has to build it). A
tempo-synced delay or arpeggiator follows Settings → AUDIO → **Tempo** (20–300
BPM) and **Time signature** instead of guessing 120 and 4/4. The position only
moves while the transport rolls, and only what choz actually knows is flagged
valid — no cycle, no SMPTE. The bar position is offered because it is real: choz
has no arrangement, so bars are counted from the last transport reset, and the
*phase* is what a plugin syncing a pattern to bar starts is actually after.

### Five ways to look at one panel

The **MIDI IN** panel has tabs (`F5`, or click them). **MIDI** is the messages as
they arrive; **KEYS** is a piano keyboard lit by them; **ROLL** is the same notes
falling towards that keyboard; **WAVE** is the shape of what came out; and
**ACTIVITY** is peak and RMS in dB, with a clip warning. The last two come from a
lock-free meter the audio callback publishes — "did the note arrive" and "did
anything come out" are the same question asked twice, and the second one needs no
MIDI at all.

`C` cycles what colours a lit key: **CHANNEL** (each MIDI channel its own hue —
in MULTI a channel is a tab), **INSTRUMENT** (the tab that is actually playing
it, for when two ports share a channel) or **VELOCITY** (how hard it was
played). Controllers never light a key: pitch bend, the modulation wheel and the
last pedals seen get their own row underneath. A note-on with velocity 0 is
treated as the release it is, and `PANIC` clears the picture along with the
notes — the keyboard is never left insisting on a chord nothing is playing.

### An arpeggiator, per tab

`A` turns it on, and the `ARP` line in the RACK is one switch until it is —
then its controls follow: pattern (**UP**, **DOWN**, **INCL**, **EXCL**,
**RANDOM**, **ORDER**, **UP×2**, **DN×2**), division from `1/4` to `1/32T` with
real triplets, its own tempo or the transport's (`SYNC`), `TAP`, gate, swing,
octaves, latch and a chord mode where one key plays a memorised shape. Held keys
go to it instead of to the instrument, and its own clock plays them.

A tab can also play **out of choz**: the OUT drawer lists the MIDI ports under
their own heading, and binding one sends everything the tab plays — keys and
arpeggiator alike — to that port as well as to its own instrument. `PANIC`
reaches it, note by note, because a synth on the other end of a cable is exactly
what that button is for.

That clock can be somebody else's: `SYNC` counts the steps off choz's transport
rather than a tempo of its own, and the transport itself follows an outside MIDI
clock when `CLK EXT` is switched on in the TRANSPORT panel — `START` from the
top, `CONTINUE` from where it stopped, and a tempo averaged over each quarter of
pulses in the port's own callback, where the timestamps are still honest.

The controls are drawn as the knob box the FX and the instrument use, without
its frame where the panel is short, and as a wrapping row of buttons where even
that would leave no FX chain — the same controls in all three. Everything is
reachable without a mouse: `k` hands it the arrows, Enter opens the list of a
control that has names, and `w`/`s` move the ones that are numbers. On/off, tap
and latch are MIDI-learnable, because they are what a player needs with both
hands busy.

It is not an FX and cannot be one: an FX processes interleaved audio and has
nowhere to put a note. It lives where routing is decided, and `tick` is handed
the current instant rather than reading a clock, so the sample-exact version
against the transport is a change of driver, not a rewrite. Every note it starts
it stops — `PANIC` and switching it off release what was sounding.

### A note as a control (`mtof`)

`M→P` on the instrument line arms the pointer; click any knob and from then on
this tab's notes drive it — keys and the arpeggiator alike. The value goes
through the same function a MIDI CC does.

What gets written depends on what the target says it is. A plugin parameter with
a `Hz` unit and a declared range gets the note's real frequency, placed
**logarithmically**, so an octave is always the same distance. Anything else is
key-tracked across the playable note range: a built-in effect's parameter
declares neither range nor unit, so writing "440" into it would be a guess, not a
conversion — and "the filter follows the keyboard" is what this is for anyway.

### Automation

`R` in TRANSPORT arms it, `X` clears it. What the user moves while the transport
rolls is written down against the beat and moved again on the next pass through
the loop; the lanes are saved with the project.

The addresses are the ones MIDI learn uses, so anything bindable is automatable:
tab volume and pan, the input trim and sensitivity, any instrument parameter, any
parameter of the selected FX. `◀ LOOP n ▶` in the TRANSPORT sets how long the
loop is, in bars of whatever the time signature says.
Recording **samples** rather than intercepting — the UI ticks faster than a hand
moves — and plays back as steps, because what was recorded is where the control
was, not a curve through it.

### Two jobs, one switch

The switch in the top-right corner (`F4`) says which one choz is doing, because
the two pull the routing in opposite directions:

- **LIVE** — one tab sounds at a time. Tabs are the songs or the patches of a
  set, and a program change from a controller's buttons steps through them.
- **MULTI** — every tab sounds at once, each answering **its own MIDI channel**:
  a multi-timbral module for a DAW's orchestral template (Reaper → choz, the way
  Kontakt is used).

In LIVE, several tabs can share one port — `+` on the tab bar gives another patch
on the same controller. They answer **any** channel by default, so the tab on
screen is the one that plays; give one a channel (`CH` on the instrument line)
and that channel reaches it whatever is on screen, which turns one port into a
split.

`PANIC` (in TRANSPORT, or `P` from anywhere) kills every sounding note: a real
note-off for each note choz knows is down, then the broadcast — the note-offs
are what a VST3 plugin actually receives, since *all notes off* is a MIDI CC.

---

## Build

### System dependencies

To **build** (the `-dev` headers; see below for what a *built* choz needs):

```bash
# Debian / Ubuntu
sudo apt install build-essential pkg-config libasound2-dev libjack-jackd2-dev

# Arch
sudo pacman -S base-devel alsa-lib jack2

# Fedora
sudo dnf install @development-tools alsa-lib-devel jack-audio-connection-kit-devel
```

`libjack`'s headers are what the native JACK backend is compiled against — it
works against PipeWire's JACK layer too, which is the usual setup. **No X11
headers**: the plugin windows go through `x11rb`, which speaks the protocol
itself and links no C library.

### Compile

```bash
git clone git@github.com:jacodelia/choz.git
cd choz
cargo build --release
```

Every plugin host is compiled in — there are no feature flags to remember.

### What it needs at runtime

Different from what it needs to *build*. The binary links two things and opens a
third by hand:

| Library | Needed? | If missing |
|---|---|---|
| `libc` | yes | nothing runs |
| `libasound.so.2` (ALSA) | yes | choz starts but opens no audio device |
| `libjack.so.0` | **optional** — `dlopen`ed at runtime | choz uses ALSA; no JACK/PipeWire routing, no per-channel outputs |
| `libpd` (Pure Data) | **optional** — linked only by `choz-pd-host` | choz installs and runs; Pure Data patches cannot be hosted |
| X11 | not linked | plugin windows go through `x11rb`, which speaks the protocol itself |

That is why the `.deb` declares only `libasound2t64` and `libc6`: JACK is a
runtime choice, not a build-time dependency. **The packages refuse to install
without ALSA**, and that is read off the built packages rather than intended:
`dpkg-deb -f` shows `Depends: libasound2t64 (>= 1.0.29), libc6 (>= 2.43)`, and
the `.rpm` requires `libasound.so.2()(64bit)` down to its `ALSA_0.9` symbol
versions, so `apt` and `rpm -i` both stop. JACK is a `Recommends` in both.

`install.sh` checks all three before it copies anything. **A missing ALSA stops
the install** — a choz that starts and then opens no device looks like a bug in
choz, not a missing package — and it prints the command for your distribution. A
missing JACK is only a note; a missing libpd is a note **and** it decides what
gets built: without it choz installs without the Pure Data half rather than
failing over it. `--skip-deps-check` installs anyway, which is right when you are
staging an install for a machine that is not this one.

### Install

**From a release** — no toolchain needed. Every tag publishes a `.tar.gz` per
architecture (x86-64, aarch64, armv7), a `.deb`, an `.rpm` and a `PKGBUILD` for
Arch, plus `SHA256SUMS.txt`:

```bash
tar xzf choz-1.1.0-x86_64-unknown-linux-gnu.tar.gz
cd choz-1.1.0-x86_64-unknown-linux-gnu
./install.sh            # uses the binary shipped beside it — no cargo involved
```

The tarball carries the binary, the launcher, the desktop entry, every icon size,
the MIME type, the wallpapers, choz's own effects as a CLAP plugin and the Pure
Data host — the same set the `.deb` installs. On ARM, remember that **plugins are
native binaries**: a Raspberry Pi loads plugins built for ARM, not the x86 ones.

**What an install puts down besides choz itself:**

| What | Where | Why |
|---|---|---|
| `choz.clap` | `~/.clap` (script) or `/usr/lib/clap` (packages) | choz's own 45 effects, usable from Bitwig, Reaper, Carla or any CLAP host. `--no-clap` skips it. |
| Wallpapers | `<prefix>/share/choz/wallpapers` | A fresh install opens on the image choz ships with, and the picker starts there. |
| `choz-pd-host` | next to `choz` | The only binary that links libpd — installed when libpd is present. |

**From a checkout** — the same script builds first:

```bash
./packaging/install.sh                    # build, then install into ~/.local
./packaging/install.sh --prefix /usr/local
./packaging/install.sh --binary target/release/choz   # skip the build
./packaging/install.sh --skip-deps-check   # install without checking ALSA
./packaging/install.sh --no-clap          # skip choz's effects as a CLAP plugin
./packaging/install.sh --uninstall
```

The script replaces an older copy before putting the new one down — it looks in
`~/.local/bin`, `/usr/local/bin` and `/usr/bin`, and asks each one its
`choz --version`. It also installs the desktop entry, the icon and the
`*.choz.yml` file association, so choz shows up in the menu — under multimedia,
beside the other audio applications — and a project opens with a double click.

**What no uninstall ever removes: `~/.local/state/choz`.** The projects, the
plugin paths and the settings are yours, not the package's.

For distributions, `.deb` and `.rpm` are built from the same assets and replace
the previous version by package name:

```bash
cargo build --release --bin choz          # both read target/release/choz
cargo deb -p choz-ui --no-build           # → target/debian/choz_*.deb
cargo generate-rpm -p crates/choz-ui      # → target/generate-rpm/choz-*.rpm
```

Because choz is a TUI, the desktop entry runs `choz-launcher`, which opens the
first terminal it finds — **kitty first**, since that is where the wallpaper is
drawn at real pixel resolution — at 120×40 cells. Below about 100×30 the RACK
does not fit.

---

## Run

```bash
cargo run --release --bin choz            # needs a real terminal (tty)
./target/release/choz                     # same thing, after a build
```

For live playing use the **release** binary: the debug one does not have the CPU
headroom for plugin DSP at small buffer sizes.

```bash
./target/release/choz project.yml         # open a saved project
./target/release/choz instrument.sf2      # load a file straight into a tab
./target/release/choz --osc-port 9000     # pin the OSC listener
```

### Keys to get started

| Key | Where | Action |
|---|---|---|
| `F2` / `F3` | anywhere | IN and OUT drawers (note inputs, audio devices) |
| `Enter` / `Space` | a drawer's channel row | put that channel on the active tab, or take it off (right click also takes it off) |
| `F10` | anywhere | menu bar (EDIT → Settings… → THEME) |
| `[` / `]` | rack | switch tab |
| `1` or `i` | rack | change the tab's instrument |
| `a` | rack | add an FX to the chain |
| `g` / `G` | rack | plugin window: instrument / selected FX |
| `x` / `X` | rack | run that plugin sandboxed |
| `l` | rack | MIDI learn (or click a knob after pressing `MIDI LEARN`) |
| `k` | rack | move the cursor between the instrument knobs and the FX ones |
| `p` | rack | full parameter list of the tab's plugin |
| `P` | anywhere | panic — kill every sounding note |
| `F4` | anywhere | LIVE ↔ MULTI |
| `F5` | anywhere | MIDI IN panel: MIDI / WAVE / ACTIVITY (the tabs are clickable) |
| `<` `>` / `;` `:` | rack | input trim / `A→M` sensitivity of a tab fed by audio |
| `←` `→` | TRANSPORT | length of the automation loop, in bars |
| `m` / `S` | rack | mute / solo the tab |
| `c` / `r` | IN drawer | connect-disconnect a port / rescan inputs |

A controller plugged in while choz is running is picked up on its own — the port
list is polled every couple of seconds.

Log: `~/.local/state/choz/choz.log` — plugin stdout lands there too, so it never
paints over the TUI.

---

## Architecture

```
choz/                      11 crates, version 1.1.0
├── crates/
│   ├── choz-ports/         RT-safe traits every host implements: AudioSource,
│   │                       FxProcessor, PluginEditor, PluginParam, SandboxStatus
│   ├── choz-engine/        Audio thread, rack, mixer, FX chain, MIDI/OSC input,
│   │                       plugin scan cache, quarantine, sandbox policy
│   │   ├── engine.rs       RT callback, slots, EngineCommand ring
│   │   ├── jack_backend.rs Native JACK client — one port per device channel
│   │   ├── fx/             45 built-in DSP effects
│   │   ├── chord.rs        The chord being held, for the harmoniser's MIDI in
│   │   ├── feedback.rs     Catches a microphone that starts to howl
│   │   ├── maxpat.rs       Reads a Max/MSP patch and says what can be kept
│   │   ├── sources.rs      WAV, SF2 (oxisynth)
│   │   ├── sfz.rs          SFZ parser + 32-voice sampler
│   │   ├── paths.rs        Per-format search paths, Carla-style
│   │   ├── quarantine.rs   Probe a plugin in a child before trusting it
│   │   └── sandboxed.rs    AudioSource/FxProcessor that talk to a child process
│   ├── choz-plugin-clap/   CLAP host (clack-host)
│   ├── choz-plugin-lv2/    LV2 host — own Turtle parser + the C ABI, no lilv
│   ├── choz-plugin-ladspa/ LADSPA + DSSI (they share a descriptor)
│   ├── choz-plugin-vst2/   VST2 host — the published binary interface, no SDK
│   ├── choz-plugin-vst3/   VST3 host — pure-Rust COM bindings, no Steinberg SDK
│   ├── choz-plugin-pd/     Pure Data patches as effects; `choz-pd-host` is the
│   │                       only binary that links libpd (feature `pd`)
│   ├── choz-plugin-clap-export/ choz's own 45 effects, published as one `.clap`
│   ├── choz-plugin-sandbox/ Shared-memory transport for out-of-process hosting
│   │                       (audio blocks and the plugin's window)
│   └── choz-ui/            The `choz` binary: TUI, rack, modals, drawers,
│                           projects, settings, i18n, plugin windows
├── packaging/              install.sh, the desktop entry, the icon, the MIME type
├── examples/
│   └── esp32s3-touch/      A touchscreen control surface that drives choz over OSC
└── docs/
    ├── architecture.md     How the pieces fit
    ├── roadmap.md          What is still missing, and the gotchas worth knowing
    ├── audio-latency.md    PipeWire/JACK tuning and how to verify it
    └── usb-xhci-crash.md   The USB controller incident, and what avoids it
```

### Realtime contract

The audio callback allocates nothing, takes no locks, and never blocks.
Commands reach it over an `rtrb` ring; dropped objects go back over a second
ring so they are freed off the RT thread.

---

## Version

| | |
|---|---|
| choz | **1.1.0** |
| Rust edition | 2021 (`choz-plugin-lv2` is 2024) |
| Toolchain tested | rustc 1.97.1 |
| Platform | Linux. ALSA/JACK/PipeWire. Released for x86-64, aarch64 and armv7 |

See [`CHANGELOG.md`](CHANGELOG.md) for what has landed so far.

---

## Tests

```bash
cargo test --workspace              # 395 tests
cargo clippy --workspace --all-targets -- -D warnings
```

| Crate | Tests | Covers |
|---|---|---|
| `choz-engine` | 183 | 35 FX processors, mixer, sources, SFZ parser, plugin paths, scan cache, quarantine, sandbox, OSC socket |
| `choz-ui` | 163 | Rack layout, parameter controls, modals, mouse hit-testing, MIDI learn, note routing in both modes, project save/load, i18n, themes, background rendering, the installer script |
| `choz-plugin-lv2` | 16 | TTL parsing, hosting installed effects, `worker#schedule`, X11 editor discovery, state round-trip |
| `choz-plugin-clap` | 11 | Effect and instrument runtime against installed plugins, window feed |
| `choz-plugin-ladspa` | 7 | LADSPA + DSSI descriptors and runtime |
| `choz-plugin-sandbox` | 5 | Shared-memory handshake, deadline behaviour, window request |
| `choz-plugin-vst2` | 4 | Host callback transport, automation feed, runtime |
| `choz-plugin-vst3` | 5 | Factory info, parameter changes reaching the processor, run loop, runtime |

Four suites use `harness = false`, because the test binary itself has to be able
to act as a worker process: `quarantine`, `sandboxed_plugin`, `scan_isolation`
(choz-engine) and `across_a_process` (choz-plugin-sandbox).

Runtime tests run against whatever plugins are installed on the machine and skip
themselves when a format has none, so a plugin-less CI stays green.

### Long sweeps

Hosting *every* installed plugin of a format is `#[ignore]`d — it takes minutes:

```bash
cargo test --release -p choz-plugin-lv2 -- --ignored
cargo test --release -p choz-plugin-ladspa -- --ignored
```

### Diagnostic examples

Not tests — small programs that measure something against the real machine:

```bash
cargo run -p choz-plugin-lv2  --example ui_probe    # open every LV2 X11 editor
cargo run -p choz-plugin-clap --example gui_probe   # same for CLAP
cargo run -p choz-engine      --example latency_probe
cargo run -p choz-engine      --example devlist
```

---

## Environment variables

| Variable | Effect |
|---|---|
| `PIPEWIRE_LATENCY` / `PIPEWIRE_QUANTUM` | Set by choz from the configured buffer size before opening the JACK client. See [`docs/audio-latency.md`](docs/audio-latency.md). |
| `CHOZ_CLAP_STRICT_TEARDOWN=1` | Destroy CLAP plugins properly instead of leaking the ones known to crash. For debugging. |
| `CHOZ_LV2_STRICT_TEARDOWN=1` | Same for LV2 — this is how the quarantine probe finds out in the first place. |
| `CHOZ_KITTY_BG=0` | Draw the wallpaper as cell colours instead of using kitty's graphics protocol. |
| `CHOZ_SANDBOX_GUI=0` | Stop isolating a plugin just because it has a window. Cheaper, and a crashing GUI takes choz with it. |
| `CHOZ_PROBE_RUNS=N` | How many times a plugin is probed before it is believed to be safe (default 3 — some crashes are races). |
| `CHOZ_VST2_DIR=<dir>` | Extra directory for the VST2 runtime tests, where the machine's instruments live. |
| `LV2_PATH`, `VST_PATH`, `VST3_PATH`, `CLAP_PATH`, `LADSPA_PATH`, `DSSI_PATH`, `SF2_PATH`, `SFZ_PATH` | Override the search path for that format. |

State lives in `~/.local/state/choz/`: `choz.log`, `plugins.json` (scan cache),
`plugin-paths.json`, `plugin-verdicts.json`, `plugin-sandbox.json`, `ui.json`.

---
### Layout

![The choz rack, inputs and monitor](docs/layout.png)
---

## Credits

- **Jorge Codelia** — author & maintainer

---
## License

MIT — see [`LICENSE`](LICENSE).
