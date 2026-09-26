# CHOZ

A terminal-based audio plugin host for the terminal.

![A terminal-based audio plugin host for the terminal.](docs/choz.png)

Built with Rust, ratatui and cpal. Provides a TUI for managing note inputs, instruments and real-time FX chains.

---

## Plugin formats

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

More on what choz does: [`docs/overview.md`](docs/overview.md).

---

## Build

### System dependencies

To **build** (the `-dev` headers; see below for what a *built* choz needs):

```bash
# Debian / Ubuntu
sudo apt install build-essential pkg-config libasound2-dev libjack-jackd2-dev

# Arch
sudo pacman -S base-devel alsa-lib pipewire-jack

# Fedora
sudo dnf install @development-tools alsa-lib-devel pipewire-jack-audio-connection-kit-devel
```

The build needs JACK's **headers and `jack.pc`**, nothing else: libjack itself
is `dlopen`ed. Every current desktop runs **PipeWire with its JACK layer
(`pipewire-jack`)** rather than jack2, and on Arch and Fedora that package is
where the headers come from too. Debian and Ubuntu package PipeWire's JACK
without headers, so there `libjack-jackd2-dev` supplies them — it installs
beside `pipewire-jack` and does not replace it at runtime. **No X11
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

### Install

```bash
./packaging/install.sh
```

Runtime dependencies, release tarballs, `.deb` / `.rpm` and every install flag: [`docs/install.md`](docs/install.md).

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
| `↑` `↓` / wheel | MIXER | that strip's level, one step (`Tab` focuses the MIXER while it is showing, or click a strip) |
| `←` `→` | MIXER | the strip beside it — the tabs, then the four groups, then the main |
| `l` / `k` | MIXER | link the strip's two channels / pick which side the arrows move |
| `C` | rack (FX) | which keyboard the selected effect takes its chord from |
| `v` | rack | split the keyboard: which saved sound each octave plays |
| `c` | rack (FX) | gate the selected effect from another tab — its level or the notes played into it, moving the effect's dry/wet or one named knob |
| `/` | rack (instrument) | find a knob by name, on a plugin whose list runs to hundreds |
| `n` / `N` | rack, MIXER | level the tab / the whole rack again, from what it has played since it was loaded |
| `c` / `r` | IN drawer | connect-disconnect a port / rescan inputs |

---

## Architecture

```
choz/
├── crates/                     The Cargo workspace: 11 crates
│   ├── choz-ports/             RT-safe traits every host implements
│   ├── choz-engine/            Audio thread, rack, mixer, MIDI/OSC input, scan, sandbox policy
│   │   └── src/
│   │       ├── fx/             The 56 built-in DSP effects
│   │       ├── artifacts/      Arpeggiator, step sequencer, metronome, arranger
│   │       └── instruments/    Built-in sources: SF2, SFZ sampler, streamed audio
│   ├── choz-plugin-clap/       CLAP host
│   ├── choz-plugin-lv2/        LV2 host (own Turtle parser, no lilv)
│   ├── choz-plugin-ladspa/     LADSPA + DSSI host
│   ├── choz-plugin-vst2/       VST2 host
│   ├── choz-plugin-vst3/       VST3 host
│   ├── choz-plugin-pd/         Pure Data patches as effects
│   │   └── src/bin/            choz-pd-host, the only binary that links libpd
│   ├── choz-plugin-clap-export/ choz's effects and artifacts as one .clap
│   ├── choz-clap/              choz itself as a CLAP instrument
│   ├── choz-plugin-sandbox/    Shared-memory transport for out-of-process plugins
│   └── choz-ui/                The choz binary: TUI, modals, projects, settings
│       └── src/views/          The panels drawn on screen
├── packaging/                  install.sh, desktop entry, icon, MIME type
├── assets/                     Wallpapers and sample .chord charts
├── examples/
│   └── esp32s3-touch/          Touchscreen control surface that drives choz over OSC
├── tools/                      Scripts that build the arranger's styles from MIDI files
├── vendor/
│   └── oxisynth/               Patched SoundFont synth (see License)
└── docs/                       Architecture, install, testing, roadmap, FX audit, manual
```

- **`crates/`** holds all the code. `choz-ports` defines the contracts, `choz-engine` runs the audio, one `choz-plugin-*` crate per plugin format hosts third-party plugins, and `choz-ui` is the application on top.
- **`packaging/`** turns a build into an installed app.
- **`assets/`** is the data choz ships with.
- **`examples/`** has programs that talk to choz from outside.
- **`tools/`** has offline scripts. They are not part of the build.
- **`vendor/`** holds dependencies that carry local changes.
- **`docs/`** is the long-form documentation. How the pieces fit together is in [`docs/architecture.md`](docs/architecture.md).

---

## Version

| | |
|---|---|
| choz | **1.3.18** |
| Rust edition | 2021 (`choz-plugin-lv2` is 2024) |
| Toolchain tested | rustc 1.97.1 |
| Platform | Linux. ALSA/JACK/PipeWire. Released for x86-64, aarch64 and armv7 |

See [`CHANGELOG.md`](CHANGELOG.md) for what has landed so far.

---

## Tests

```bash
cargo test --workspace
```

Per-crate coverage, long sweeps and diagnostic examples: [`docs/testing.md`](docs/testing.md). Environment variables: [`docs/environment.md`](docs/environment.md).

---
### Layout

![The choz rack, inputs and monitor](docs/layout.png)
---

## Credits

- **Jorge Codelia** — author & maintainer

---
## License

MIT — see [`LICENSE`](LICENSE).

**One vendored dependency keeps its own licence.** `vendor/oxisynth` is
[oxisynth](https://github.com/PolyMeilex/oxisynth) 0.1.0 (LGPL-2.1, its
`LICENSE` beside it) with one addition — `Synth::read_next_groups`, which
reads every audio group instead of only the first, so each musician of the
arranger's band can have a mixer strip of its own. The change is described in
`vendor/oxisynth/CHOZ-PATCH.md` and wired in through `[patch.crates-io]`.
