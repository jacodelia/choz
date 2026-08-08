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

choz is early-stage but usable. The FX engine, the rack and the TUI are real and
working, and **CLAP, LV2, LADSPA, DSSI, VST2 and VST3 plugins are really hosted**
— instruments and audio effects, with their own parameters.

### Plugin formats

| Format | Scan | Instrument | Effect | Native window |
|---|---|---|---|---|
| **LV2**    | ✅ | ✅ | ✅ | ✅ `ui:X11UI`, no suil |
| **VST2**   | ✅ | ✅ | ✅ | ✅ `effEditOpen` |
| **CLAP**   | ✅ | ✅ | ✅ | ✅ `clap.gui` + host timer |
| **VST3**   | ✅ | ✅ | ✅ | ❌ `IPlugView` not started |
| **LADSPA** | ✅ | — | ✅ | ❌ (format has no GUI) |
| **DSSI**   | ✅ | ✅ | ✅ | ❌ |
| **SFZ**    | ✅ | ✅ | — | — |
| **SF2**    | ✅ | ✅ (oxisynth) | — | — |

Plus **32 built-in DSP effects**, and WAV playback as a rack source.

Plugin windows embed into a real X11 window on choz's editor thread — no suil,
no Steinberg SDK. Verified by counting the parent window's actual X11 children,
not by trusting return values: **20 of 20 CLAP** plugins installed here open at
the size they ask for (Surge XT included), and **91 of 98 LV2 editors** swept
so far (1 opened without producing a window, 4 gave no editor at all).

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
  restarts it: a click instead of a dead tab.

---

## Build

### System dependencies

```bash
# Debian / Ubuntu
sudo apt install build-essential pkg-config libasound2-dev libjack-jackd2-dev libx11-dev

# Arch
sudo pacman -S base-devel alsa-lib jack2 libx11

# Fedora
sudo dnf install @development-tools alsa-lib-devel jack-audio-connection-kit-devel libX11-devel
```

`libjack` is what the native JACK backend links against — it works against
PipeWire's JACK layer too, which is the usual setup. `libX11` is only needed for
the native plugin windows.

### Compile

```bash
git clone git@github.com:jacodelia/choz.git
cd choz
cargo build --release
```

Every plugin host is compiled in — there are no feature flags to remember.

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
| `F10` | anywhere | menu bar (EDIT → Settings… → THEME) |
| `[` / `]` | rack | switch tab |
| `1` or `i` | rack | change the tab's instrument |
| `a` | rack | add an FX to the chain |
| `g` / `G` | rack | plugin window: instrument / selected FX |
| `x` / `X` | rack | run that plugin sandboxed |
| `l` | rack | MIDI learn |
| `m` / `S` | rack | mute / solo the tab |
| `c` / `r` | IN drawer | connect-disconnect a port / rescan inputs |

A controller plugged in while choz is running is picked up on its own — the port
list is polled every couple of seconds.

Log: `~/.local/state/choz/choz.log` — plugin stdout lands there too, so it never
paints over the TUI.

---

## Architecture

```
choz/                       9 crates, version 0.1.0
├── crates/
│   ├── choz-ports/         RT-safe traits every host implements: AudioSource,
│   │                       FxProcessor, PluginEditor, PluginParam, SandboxStatus
│   ├── choz-engine/        Audio thread, rack, mixer, FX chain, MIDI/OSC input,
│   │                       plugin scan cache, quarantine, sandbox policy
│   │   ├── engine.rs       RT callback, slots, EngineCommand ring
│   │   ├── jack_backend.rs Native JACK client — one port per device channel
│   │   ├── fx/             32 built-in DSP effects
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
│   ├── choz-plugin-sandbox/ Shared-memory transport for out-of-process hosting
│   └── choz-ui/            The `choz` binary: TUI, rack, modals, drawers,
│                           projects, settings, i18n, plugin windows
└── docs/
    ├── architecture.md     How the pieces fit
    ├── roadmap.md          What is done, what is not, and every session's notes
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
| choz | **0.1.0** (unreleased — no tags yet) |
| Rust edition | 2021 (`choz-plugin-lv2` is 2024) |
| Toolchain tested | rustc 1.97.1 |
| Platform | Linux (x86-64). ALSA/JACK/PipeWire |

See [`CHANGELOG.md`](CHANGELOG.md) for what has landed so far.

---

## Tests

```bash
cargo test --workspace              # 211 tests
cargo clippy --workspace --all-targets -- -D warnings
```

| Crate | Tests | Covers |
|---|---|---|
| `choz-engine` | 107 | 32 FX processors, mixer, sources, SFZ parser, plugin paths, scan cache, OSC socket |
| `choz-ui` | 76 | Rack layout, modals, mouse hit-testing, MIDI learn, project save/load, i18n, themes and background rendering |
| `choz-plugin-lv2` | 8 | TTL parsing, hosting installed effects, `worker#schedule`, X11 editor discovery |
| `choz-plugin-clap` | 8 | Effect runtime against installed plugins |
| `choz-plugin-ladspa` | 6 | LADSPA + DSSI descriptors and runtime |
| `choz-plugin-sandbox` | 3 | Shared-memory handshake, deadline behaviour |
| `choz-plugin-vst2` | 2 | Host callback transport, runtime |
| `choz-plugin-vst3` | 1 | Factory info, runtime |

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
| `LV2_PATH`, `VST_PATH`, `VST3_PATH`, `CLAP_PATH`, `LADSPA_PATH`, `DSSI_PATH`, `SF2_PATH`, `SFZ_PATH` | Override the search path for that format. |

State lives in `~/.local/state/choz/`: `choz.log`, `plugins.json` (scan cache),
`plugin-paths.json`, `plugin-verdicts.json`, `plugin-sandbox.json`, `ui.json`.

---
### Layout

![SeqTerm Pattern view](docs/layout.png)
---

## Credits

- **Jorge Codelia** — author & maintainer

---
## License

MIT — see [`LICENSE`](LICENSE).
