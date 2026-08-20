# CHOZ

![A terminal-based audio plugin host for the terminal.](docs/choz.png)

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

**1.3.0.** The FX engine, the rack and the TUI are real and working, **CLAP, LV2,
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
| **Pure Data** | ✅ `.pd` | — | ✅ | ❌ (patch has no embeddable window) |
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
What is drawn comes from the plugin, including the two things it is easy to get
wrong: a parameter it says cannot be automated is **not** a knob (Surge XT
publishes 191 `MIDI CC` rows that do nothing), and a parameter whose whole range
only ever reads as two words is a **switch**, not a fader, even when the plugin
reports no steps at all — which Surge does for all 800 of its parameters.

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
`clap.state` and LV2 `state#interface`.

Playing rather than patching: a **MIXER** tab at the bottom shows every rack tab
at once as channel strips — **one vertical fader per output channel** with a
link between them (tied by default, broken to trim one side against the other),
pan, mute and solo, each editable where it is drawn instead of one tab at a
time, moved by the wheel or the arrows in the same step the RACK's `VOL` uses,
and paging with `◀ ▶` when the rack is wider than the panel; a **metronome** beside the LIVE/MULTI switch clicks off the same
transport every synced plugin reads (tempo, time signature, three sounds), and
it keeps counting with the transport stopped, which is when a metronome is
wanted; and the arpeggiator's **HOLD** works the way a Keystep's does — let go
and the chord keeps playing, and the next key pressed with nothing down starts a
new one rather than piling onto the old.

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

### System requirements

| | Minimum | Notes |
|---|---|---|
| **OS** | Linux with glibc | The audio backends are ALSA and JACK, and the plugin sandbox re-runs the binary per directory. No macOS or Windows build. |
| **Architecture** | x86-64, aarch64 or armv7 | Exactly what the releases ship: `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu` (Pi 3/4/5), `armv7-unknown-linux-gnueabihf` (Pi 2, Zero 2 W). |
| **Audio** | ALSA (`libasound.so.2`) | Required — without it choz starts and opens no device. JACK/PipeWire is optional and `dlopen`ed. |
| **Terminal** | Any 24-bit-colour terminal | The wallpaper and logo render as halfblocks anywhere; kitty, Ghostty and WezTerm additionally get the graphics protocol. |
| **Terminal size** | 80×24 | Panels drop their optional rows as they shrink rather than breaking, but below this the rack and the monitor stop being readable together. |
| **Plugin windows** | X11 or XWayland | Only needed to *open* a plugin's own window. Every parameter is a knob in the RACK without it. |
| **Toolchain** (source only) | Rust stable, 2021 edition | Not needed for a release install. |

**Real-time privileges are optional but wanted.** choz runs without them; a
buffer small enough to play through needs them. What actually matters is
`rtprio` and `memlock` for your user — on a distribution with rtkit and
`@audio` set up, being in the `audio` group is usually the whole job:

```bash
ulimit -r -l          # want a non-zero rtprio and unlimited memlock
groups | grep audio   # the usual way to get them
```

Plugins are **native binaries**: an ARM install loads ARM plugins, not the x86
ones sitting in the same directory.

### Compile

```bash
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
| `libpd` (Pure Data) | **optional** — linked only by `choz-pd-host`, from `libpd-dev` (not `puredata-dev`) | choz installs and runs; Pure Data patches cannot be hosted |
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
tar xzf choz-1.3.0-x86_64-unknown-linux-gnu.tar.gz
cd choz-1.3.0-x86_64-unknown-linux-gnu
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
| `PgUp` / `PgDn` | rack | page the instrument's knob box (the `◀` `▶` on its top edge) — the learned CCs page with it |
| `p` | rack | parameters of the tab's instrument (a plugin's own list, or an SF2's reverb / chorus switches) |
| `P` | anywhere | panic — kill every sounding note |
| `F4` | anywhere | LIVE ↔ MULTI |
| `F5` | anywhere | bottom panel: MONITOR / KEYS / WAVE / MIXER (the tabs are clickable) |
| `F6` | anywhere | metronome on/off (the `▾` beside it opens tempo / signature / grouping / sound — arrows, Enter and the wheel move each row) |
| `<` `>` / `;` `:` | rack | input trim / `A→M` sensitivity of a tab fed by audio |
| `F7` / `F8` / `F9` | anywhere | roll-stop the rack / arm automation recording / panic (all-notes-off) — the same three buttons that sit on the menu bar |
| `m` / `S` | rack | mute / solo the tab |
| `↑` `↓` / wheel | MIXER | that tab's level, one step (`Tab` focuses the MIXER while it is showing, or click a strip) |
| `l` / `k` | MIXER | link the strip's two channels / pick which side the arrows move |
| `C` | rack (FX) | which keyboard the selected effect takes its chord from |
| `v` | rack | split the keyboard: which saved sound each octave plays |
| `c` | rack (FX) | gate the selected effect from another tab — the kick that opens it |
| `n` / `N` | rack, MIXER | level the tab / the whole rack again, from what it has played since it was loaded |
| `c` / `r` | IN drawer | connect-disconnect a port / rescan inputs |

### A plugin's knobs, readable

Surge XT calls its parameters `Filter 1 Cutoff`, `Filter 1 Resonance`,
`Filter 1 Type`. A cell in the rack is thirteen columns — eleven characters —
so those read `Filter 1 C…`, `Filter 1 R…`, `Filter 1 T…`: three knobs that
look the same and none you can name.

The part that repeats is now drawn **once**, as the box's heading
(`INSTRUMENT · Surge XT · Filter 1`), and each cell shows only what differs:
`Cutoff`, `Resonance`, `Type`. The section comes from the plugin when it gives
one — CLAP's `module` — and is read off the names when it does not, which is
every other format: a run of consecutive parameters that begin with the same
words *is* a section, because that is how plugins with sections write them.
Nothing is invented for a lone parameter; it keeps its whole name.

The long parameter list groups them the same way, without extra rows: the
cursor is the parameter index, so a heading row of its own would put every row
one out of step. The first row of a section names it and the rest sit indented
under it.

### The harmoniser and the vocoder are one effect

They answer the same question — what should the voice be sung *on* — and both
read the same held chord, so they are one effect with a `Mode`: **HARMONY**
pitch-shifts voices onto the chord, **VOCODER** puts the voice's shape on it.
The vocoder gained a carrier for exactly this: `CHORD`, a bank of saws at the
notes being held, so the keyboard decides what the machine says. Nothing held
is silence — a vocoder with no carrier says nothing.

The merge **appended**: every knob the harmoniser had is at the index it was, so
a project written when these were two effects opens with its controls
untouched. The old `vocoder` effect still loads and still works; it is only off
the ADD FX menu, because two entries for one sound is two places to look.

`C` (shift) picks **which keyboard** the chord comes from: any, or one by name.
The channel was always a knob; the port could not be, for the same reason the
clock's could not — a number into a list of ports means a different device the
moment one is unplugged.

### Four sounds on buttons, and the keyboard split between them

A tab with an instrument gets a `SOUNDS` row: four buttons and a `+` (up to
eight). **Left-click recalls, right-click saves** — what the tab is playing
*right now*, taken from the live plugin rather than from what choz last stored,
so a patch changed inside the plugin's own window is the one that gets kept.
The knobs and the program go with it.

They are MIDI-actionable: every button is in the learn picker, so a footswitch,
a pad or the program change a pedalboard sends when its patch changes can fire
one. This is not the BANK line above it — that lists what the *plugin* ships;
these are the sound as the player left it.

`v` opens the **split**: a row per MIDI octave, drawn as that octave's twelve
keys in the colour of the sound it plays, stepped with Enter, the arrows or the
wheel. A sound can be used in as many octaves as you like. A note arriving in
an octave brings its sound with it, and one arriving in an octave that has none
leaves the tab alone.

**It swaps the patch, it does not layer two.** One tab is one instrument: for a
SoundFont the swap is a program change and instant; for a plugin it is a state
restore, which costs what pressing the button costs. Two sounds at once is two
tabs, which is what MULTI is for.

### One tab's kick, another tab's filter

Any effect can be **gated from another tab**. Select it in the chain, press
`c`, and the dialogue asks the only four things a gate is: which tab drives it,
whether that tab **opens** the effect or **ducks** it, how much of the effect
the gate owns (depth), and what counts as a hit (threshold, plus a release).

The example it was built for: a drum kit on tab 1 and a keyboard on tab 2, with
the kick opening an auto-filter on the keyboard. The gated effect shows `⌁1` on
its button in the chain, because what an effect is wired to is in none of its
knobs — and an effect that goes quiet between kicks otherwise looks broken.

It works with **all forty-five effects and with hosted plugins**, without a
line of per-effect code: a gate rides the effect's dry/wet, and `set_mix` is
something the processor trait has required all along. The source is the tab's
own level in the last block, which the audio callback already publishes for the
meters — so a tab that renders after the gated one is one block late, which at
choz's block sizes is under three milliseconds.

### One clock, and you say whose

`CLK` on the menu bar opens a picker: **INTERNAL** (choz's own tempo), **ANY
PORT**, or one of the connected devices by name. Clock messages now travel
tagged with the port they arrived on, so a rig with a groovebox *and* a DAW on
the same hub can name which of them is the master — the other one is ignored
outright instead of fighting for the tempo.

It is one setting for the whole rack, deliberately: there is one transport,
every synced plugin reads it, and a clock per tab is a rack playing against
itself.

### A desk, not a pile of tabs

Under the tab strips the MIXER now carries the desk's own: **four subgroups**
(`A`–`D`) and a **MAIN**. A subgroup is a destination that is not a device —
tabs sum into it, its fader rides all of them together, its mute takes the
group out, and its output pair decides where the group lands. Every tab strip
shows where it sums (`▸OUT`, `▸A`…) and clicking that cell walks
`OUT → A → B → C → D`.

The MAIN is the last thing the **first** output pair passes through, which is
the pair everything calls "the output" and the one the meter reads. The other
pairs are separate outputs, left alone on purpose: a master fader that also
trimmed channels 7 and 8 is a master fader that silences a monitor send.

The metronome has its own destination (its menu's `OUTPUT` row): the click can
go to a group — a wedge — and nowhere else. It borrows that group's routing but
not its fader, because a reference that moves when somebody rides the group is
not a reference.

### One bar instead of a panel

Everything true of the whole rack lives on the top-right of the menu bar rather
than in a panel of its own: the metronome, the LIVE/MULTI switch, the transport
(`▶ ON` / `■ OFF`), the automation's `● REC` chip, the loop length `◀ 4 ▶`,
the `CLK INT/EXT` switch, `PANIC`, and what the audio callback is spending
(`DSP 12%`, ambered past 70% and red past 90%).

Every one is clickable; the loop's two arrows are the two halves of its button,
and the **right** button on `REC` throws the recorded lanes away. The keyboard
reaches the three that matter through `F7`, `F8` and `F9` — function keys
because every letter is already worth something to whichever section has the
focus, which is exactly why those controls used to be unreachable unless the
TRANSPORT panel was focused.

The panel itself is gone. It spent seven rows on five buttons and a device
name; the rows went to the RACK, and the device name — with `NOT CONNECTED`
and `CLIP` beside it — to the status bar, where clicking it still opens the
OUT drawer.

### Tabs that arrive level

A new instrument is measured before it reaches the audio thread: choz plays it
a middle C, listens to half a second of it, and sets the tab's fader so its
loudest passage sits at -18 dBFS RMS — never above full scale, whatever that
costs in loudness. A SoundFont, a Surge patch and a sampled piano therefore
land at roughly the same level instead of metres apart.

It happens **once, at load**. After that the strip is the player's: nothing
moves a fader again unless `n` or `N` is pressed, and a project reopens with
the levels it was saved with. A plugin that answers the probe with silence
(some need warming up) keeps the fader it had and says so in the log.

A controller plugged in while choz is running is picked up on its own — the port
list is polled every couple of seconds.

Log: `~/.local/state/choz/choz.log` — plugin stdout lands there too, so it never
paints over the TUI.

---

## Architecture

```
choz/                      11 crates, version 1.3.0
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
| choz | **1.3.0** |
| Rust edition | 2021 (`choz-plugin-lv2` is 2024) |
| Toolchain tested | rustc 1.97.1 |
| Platform | Linux. ALSA/JACK/PipeWire. Released for x86-64, aarch64 and armv7 |

See [`CHANGELOG.md`](CHANGELOG.md) for what has landed so far.

---

## Tests

```bash
cargo test --workspace              # 580 tests
cargo clippy --workspace --all-targets -- -D warnings
```

| Crate | Tests | Covers |
|---|---|---|
| `choz-engine` | 295 | 35 FX processors, mixer, sources, SFZ parser, preset files and where a plugin keeps them, the metronome, plugin paths, scan cache, quarantine, sandbox, OSC socket |
| `choz-ui` | 221 | Rack layout, parameter controls, modals, mouse hit-testing, MIDI learn (including the knob box paging under it), the mixer strips, note routing in both modes, project save/load, i18n, themes, background rendering, drawing at every terminal size, the installer script |
| `choz-plugin-lv2` | 17 | TTL parsing, hosting installed effects, `worker#schedule`, X11 editor discovery, state round-trip |
| `choz-plugin-clap` | 13 | Effect and instrument runtime against installed plugins, window feed |
| `choz-plugin-ladspa` | 7 | LADSPA + DSSI descriptors and runtime |
| `choz-plugin-sandbox` | 6 | Shared-memory handshake, deadline behaviour, window request |
| `choz-plugin-vst2` | 5 | Host callback transport, automation feed, runtime |
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
cargo run --release -p choz-engine --example sf2_voices -- <sf2> 96000 128   # what a pedalful of notes costs
cargo run --release -p choz-engine --example pedal_bench -- <sf2> <vst3> [sandbox|inproc] [busy threads]
cargo run --release -p choz-engine --example param_shapes -- <vst3>          # what a plugin says vs what choz draws
```

---

## Environment variables

| Variable | Effect |
|---|---|
| `PIPEWIRE_LATENCY` / `PIPEWIRE_QUANTUM` | Set by choz from the configured buffer size before opening the JACK client. See [`docs/audio-latency.md`](docs/audio-latency.md). |
| `CHOZ_CLAP_STRICT_TEARDOWN=1` | Destroy CLAP plugins properly instead of leaking the ones known to crash. For debugging. |
| `CHOZ_LV2_STRICT_TEARDOWN=1` | Same for LV2 — this is how the quarantine probe finds out in the first place. |
| `CHOZ_KITTY_BG=0` | Draw the wallpaper as cell colours instead of using kitty's graphics protocol. |
| `CHOZ_SANDBOX_GUI=1` | Isolate every plugin that has a window, not only the ones that crashed. Safer against a GUI segfault, and it costs most of an audio block per plugin — which is what used to break the sound up. |
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
