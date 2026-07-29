# choz

A terminal-based audio plugin host — like [Carla](https://kx.studio/Applications:Carla) for the terminal.

Built with Rust, ratatui and cpal. Provides a TUI for managing note inputs,
instruments and real-time FX chains.

## Status

choz is early-stage but usable. The FX engine, the rack and the TUI are real and
working; **CLAP** is the only plugin format that is actually hosted today (other
formats are scanned and listed, but not instantiated). See the tables below for
what works versus what is planned.

## Features

### Working today

- **Terminal UI** with full mouse and keyboard support, incl. a top **menu bar** (`F10` or click: File / Edit / Help) — every action is reachable by mouse
- **About dialog** with an in-terminal image logo rendered via [ratatui-image](https://github.com/benjajaja/ratatui-image) (Help → About)
- **32 built-in FX processors**: delay, granular delay, reverse delay, space echo, reverb, protocosmos, Z5 texture, compressor, limiter, gate, expander, parametric EQ, filter, filter bank, chorus, flanger, phaser, bitcrusher, vinyl sim, cassette sim, soft clip, tube saturation, two stompbox distortions (AMBER FANG, VELVET FUZZ), stereo widener, isolator, looper, sidechain ducking, panner, gain, phase invert, mono maker
- **INPUTS → RACK model**: the left panel lists note inputs (every MIDI port, plus OSC). `Enter` on one creates a rack tab bound to it; pick its instrument from the tab's `[1:SOURCE]` button. Switch tabs with `[` / `]` or by clicking; `✕` (or `Backspace`) removes one.
- **Per-input routing**: notes from an input reach only the tabs bound to that input, so two controllers can drive two different instruments at once. The QWERTY piano always plays the active tab.
- **Per-slot FX**: up to 5 FX per rack tab, reorderable, with live parameter tweaking. Parameters wrap onto more knob rows when an effect has many (Z5 Texture has 16).
- **Per-slot mixer**: gain, constant-power pan, mute and solo on every rack slot (`-`/`+`, `,`/`.`, `m`, `S`, or scroll/click the strip)
- **MIDI learn** for faders *and* buttons: press `MIDI LEARN` (or `3`), click the control with the pointer, then move a fader — the next CC is bound. Buttons (MUTE, SOLO, BANK ◀/▶, FX ON/OFF, MOVE, ADD FX, FX slot select) fire on the CC's rising edge. `l` opens the same picker for keyboard-only use.
- **Real-time audio** via cpal (ALSA / JACK, auto-detected through PipeWire)
- **RT-safe engine**: lock-free, zero-allocation audio callback (chains built on the UI thread, handed over an `rtrb` ring; dropped objects retire off the RT thread)
- **Selectable audio output**: `o` in the transport panel lists the backend's output devices; switching reloads the rack onto the new stream
- **WAV file playback** as an instrument (`choz path/to/file.wav`, or browse from `[1:SOURCE]`)
- **SF2 SoundFont synthesis** (via [oxisynth](https://github.com/PolyMeilex/OxiSynth)) — play it with the computer keyboard (`a w s e d f t g y h u j k`, one octave from C4)
- **SF2 bank/preset selection**: the `BANK ◀ 000:000 Name ▶` line steps through programs; `[2:BANK/PRESET]` opens the full list in a modal
- **Hardware MIDI input** (via [midir](https://github.com/Boddlnagg/midir)) — connects every MIDI input port at startup; toggle individual ports with `c` in the INPUTS panel (`r` or the `SCAN INPUTS` button rescans)
- **OSC input** over UDP (port 9000, or `--osc-port N`): notes (`/note <note> <vel>`, `/note/on`, `/note/off`) as an input like any other, plus remote control — `/mix/<tab>/gain|pan|mute` and `/fx/<tab>/<fx>/<param>` (all indices 1-based, as drawn). Port and enable/disable are changeable live from Settings → AUDIO → OSC.
- **CLAP hosting** (via [clack-host](https://github.com/prokopyl/clack), behind `--features clap`) — instruments from `[1:SOURCE]`, and CLAP *audio effects* in the ADD FX list, **with their own parameters** on the knobs (names and ranges read from the plugin; changes go straight to the running plugin). `p` opens a scrollable editor for a CLAP *instrument's* parameters.
- **Carla-style plugin paths**: per-format scan directories (LADSPA, DSSI, LV2, VST2, VST3, CLAP, SF2, SFZ, JSFX), respecting `LV2_PATH` / `VST_PATH` / `VST3_PATH` / `CLAP_PATH` …, editable in Settings → AUDIO → Plugin Paths (add / edit inline / browse / remove / restore defaults). Each directory reports what it contributed, and warns when it holds files of a *different* format.
- **Cached plugin scan**: results cached in `~/.local/state/choz/plugins.json` and reused until a scan directory or the path config changes; `r` in the SOURCE / ADD FX modal forces a rescan
- **ADD FX with a category sidebar**: format chips (`ALL / BUILT-IN / CLAP / LV2 / VST2 / VST3 / LADSPA / DSSI / JSFX`) plus a sidebar of categories (DELAY, REVERB, DYNAMICS, EQ / FILTER, MODULATION, DISTORTION, SPATIAL, TEXTURE, UTILITY, OTHER)
- **Audio settings**: backend (AUTO/JACK/PIPEWIRE/ALSA), device, sample rate, buffer size, latency readout. Device changes apply live; backend / sample rate / buffer apply on the next start (the row says so).
- **Themeable text and border color** (9-color palette) and **i18n** — English, Spanish, Portuguese, French, Italian, German, Russian, Japanese and Chinese, picked up from `$LC_ALL` / `$LC_MESSAGES` / `$LANG` on first run
- **Save project** (File → Save project…): writes `choz-project.yml` with both halves — sound (rack tabs, instrument + bank/preset or plugin params, full FX chain with every knob, mixer, bound MIDI input, MIDI-learn bindings) and configuration (plugin paths, color, language, audio settings, OSC port, disabled MIDI ports)

### Planned (not yet implemented)

- **Loading projects** — the YAML is written and the structs already `Deserialize`; rebuilding the rack from it is not wired yet
- **Hosting for non-CLAP formats** (LADSPA/DSSI/LV2/VST2/VST3) — scanned and listed, but marked `(not hosted yet)` and refused with a log line rather than failing silently
- **Plugin GUIs** — parameters are edited on choz's knobs / in the instrument modal
- **Knob paging for FX with more than 7 plugin parameters**
- **Parameter automation**
- **Per-channel routing inside one port** — routing is per input port, not per MIDI channel

> A rack tab starts empty (silent) until you give it an instrument from `[1:SOURCE]`.

## Screenshot

```
 FILE  EDIT  HELP
┌─ INPUTS [ACTIVE] ───────────┐ ┌─ RACK ───────────────────────────────────────────┐
│ [ SCAN INPUTS ]             │ │  SF2:FluidR3 ✕   ⊘WAV:loop ✕                     │
│ ↑↓ · Enter=bind tab · c=on/…│ │  VOL [▓▓▓▓░░░░] 1.00  PAN L───●───R C   MUTE SOLO │
│ ✓ MIDI LPK25    → tab 1     │ │  INSTR SF2:FluidR3   [1:SOURCE] [2:BANK] [3:LEARN]│
│ · MIDI Midi Through         │ │  BANK  ◀  000:000 Yamaha Grand Piano  ▶           │
│ ✓ OSC  OSC      → tab 2     │ │ ── FX CHAIN ─────────────────────────────────────│
│                             │ │   1:DELAY  2:REVERB  [+ ADD]                      │
│                             │ │ ┌ 1:DELAY ────────────────────────────────────────│
│                             │ │ │ [████████]  [████    ]                          │
│                             │ │ │  ↑0.95       ↙0.35                              │
│                             │ │ │  Feedback    Wet                                │
│                             │ │ ┌ SLOT ───────────────────────────────────────────│
│                             │ │ │  ON   ◀ MOVE   MOVE ▶   DEL                     │
└─────────────────────────────┘ └──────────────────────────────────────────────────┘
                                ┌─ TRANSPORT ──────────────────────────────────────┐
                                │  [ ▶ PLAY ]     [ ■ STOP ]                        │
                                │   ■ STOPPED  |  [Space]=play  [S]=stop            │
                                │   OUT  cpal_client_out  [o=change]                │
                                └──────────────────────────────────────────────────┘
 choz v0.1 | JACK backend | RACK: 1/2 SF2:FluidR3 ← LPK25 | FX: 2 | ■ STOPPED | F10=menu Tab=switch q=quit
```

### Known issues

- Some plugins crash inside their own teardown (`ZaMaximX2` segfaults in `deactivate`, reproducible with a bare CLAP host). choz therefore stops processing and **deliberately leaks** a plugin instead of destroying it; the memory comes back when choz exits. Set `CHOZ_CLAP_STRICT_TEARDOWN=1` to do the proper teardown instead.
- A plugin that emits NaN before its parameters are set (`ZamEQ2` does) is muted for that block rather than sent to the output device.
- `serde_yaml` 0.9 is deprecated upstream; project saving still uses it.

## Requirements

### System Dependencies

| Dependency | Ubuntu/Debian | Fedora | Arch |
|-----------|---------------|--------|------|
| ALSA dev | `libasound2-dev` | `alsa-lib-devel` | `alsa-lib` |
| JACK (optional) | `libjack-dev` | `jack-audio-connection-kit-devel` | `jack2` |

### Rust

- Rust 1.80+ (stable)
- Install via [rustup](https://rustup.rs)

## Compilation

### Development Build

```bash
# Clone the repository
git clone <repo-url>
cd choz

# Install system dependencies (Ubuntu/Debian)
sudo apt install libasound2-dev libjack-dev

# Build the whole workspace
cargo build --workspace

# Build with real CLAP plugin hosting
cargo build -p choz-ui --features clap

# Run (needs a real terminal)
cargo run --bin choz
```

### Release Build (Optimized)

```bash
# Build with optimizations
cargo build --release --workspace

# The binary will be at:
#   target/release/choz
```

### Tests and lints

```bash
cargo test --workspace                  # 128 tests
cargo test -p choz-plugin-clap --features clap
cargo clippy --workspace -- -D warnings
```

The CLAP runtime tests load, process and drop every `.clap` plugin installed on
the machine; they skip themselves when none are found. CI (`.github/workflows/ci.yml`)
runs build + test + clippy, plus a `--features clap` build and the CLAP tests.

### Cross-compilation

```bash
# For a specific target
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
```

## Execution

```bash
# Run from source
cargo run --bin choz

# Run the release binary directly
./target/release/choz

# Open a file straight away, and/or move the OSC listener
./target/release/choz --osc-port 9001 path/to/file.sf2

# Follow the log
tail -f ~/.local/state/choz/choz.log
```

State lives under `~/.local/state/choz/`: `choz.log`, `plugins.json` (scan cache),
`plugin-paths.json` (scan directories) and `ui.json` (color, language, audio + OSC settings).

### Keyboard Controls

| Key | Context | Action |
|-----|---------|--------|
| `Tab` | Global | Cycle focus: Inputs → Rack → Transport |
| `F10` | Global | Open the menu bar |
| `q` | Global | Quit |
| `↑` `↓` | Inputs | Move through the input list |
| `Enter` | Inputs | Bind the selected input to a rack tab (or jump to its tab) |
| `c` | Inputs | Connect / disconnect the selected input |
| `r` | Inputs | Rescan and reconnect MIDI ports |
| `1` / `i` | Rack | Open CHANGE SOURCE (instrument picker, filtered by format) |
| `2` / `b` | Rack | Open the SF2 bank/preset list |
| `3` | Rack | Arm MIDI learn with the pointer (click a control, then move a fader) |
| `l` | Rack | Pick a MIDI-learn target from a list (keyboard only) |
| `p` | Rack | Edit the CLAP instrument's parameters |
| `[` `]` | Rack | Previous / next tab |
| `Backspace` | Rack | Remove the active tab (or click the tab's `✕`) |
| `←` `→` | Rack | Select FX slot |
| `↑` `↓` | Rack | Select parameter |
| `w` / `s` | Rack | Increase / decrease the selected parameter |
| `Space` | Rack | Toggle the selected FX on/off |
| `a` | Rack | Add an FX (opens ADD FX) |
| `d` | Rack | Delete the selected FX |
| `-` / `+` | Rack | Slot gain down / up |
| `,` / `.` | Rack | Pan left / right |
| `m` / `S` | Rack | Mute / solo the slot |
| `Space` | Transport | Toggle play/stop |
| `s` | Transport | Stop |
| `o` | Transport | Choose the audio output device |
| `↑` `↓` / wheel | Any modal | Move the cursor / scroll |
| `←` `→` | Any modal | Switch panel (sidebar ↔ list) or change a value |
| `Tab` | Any modal | Cycle the filter chips |
| `Enter` | Any modal | Confirm |
| `Esc` | Any modal | Close (cancel) |
| `e` `a` `b` `d` `r` | Settings · plugin paths | Edit / add / browse / remove a path, restore defaults |
| `a w s e d f t g y h u j k` | Anywhere | QWERTY piano, one octave from C4 |

### Mouse Controls

| Action | Result |
|--------|--------|
| Click panel | Focus that panel |
| Click an input | Bind it to a rack tab |
| Click an input's `✓`/`·` | Connect / disconnect it |
| Click `SCAN INPUTS` | Rescan and reconnect MIDI ports |
| Click `[1:SOURCE]` | Choose the tab's instrument |
| Click `[2:BANK/PRESET]` / `BANK ◀ ▶` | Open the preset list / step programs |
| Click `[3:MIDI LEARN]` | Arm pointer learn, then click the control to bind |
| Click the `OUT` line | Choose the audio output device |
| Click FX slot | Select slot + show parameters |
| Click parameter knob | Select parameter |
| Scroll on parameter | Adjust value ±0.03 |
| Click `[+ ADD]` | Open ADD FX |
| Click `ON`/`OFF` | Toggle FX enabled |
| Click `◀ MOVE` / `MOVE ▶` | Reorder FX |
| Click `DEL` | Delete FX |
| Click `[ ▶ PLAY ]` / `[ ■ STOP ]` | Transport control |
| Click a rack tab | Switch active tab |
| Click `✕` on a rack tab | Remove that tab |
| Scroll on `VOL` / `PAN` | Adjust slot gain / pan |
| Click `MUTE` / `SOLO` | Toggle slot mute / solo |
| Click a modal row | Select it; click again (or `SELECT`) confirms |
| Click a modal sidebar section / chip | Filter the list |
| Click outside a modal | Dismiss |

## Generating a Release

### Binary Release

```bash
# Build optimized binary
cargo build --release --workspace

# Strip debug symbols (Linux)
strip target/release/choz

# Check binary size
ls -lh target/release/choz

# Create tarball
VERSION=$(grep '^version' Cargo.toml | head -1 | cut -d'"' -f2)
tar -czf choz-${VERSION}-x86_64-linux.tar.gz \
    -C target/release choz \
    README.md docs/architecture.md
```

### Using cargo-release (Automated)

```bash
# Install cargo-release
cargo install cargo-release

# Dry run
cargo release --dry-run

# Execute release (bump version, tag, publish)
cargo release patch   # 0.1.0 → 0.1.1
cargo release minor   # 0.1.0 → 0.2.0
cargo release major   # 0.1.0 → 1.0.0
```

### Static Binary (musl)

```bash
# Add musl target
rustup target add x86_64-unknown-linux-musl

# Install musl toolchain (Ubuntu)
sudo apt install musl-tools

# Build fully static binary
cargo build --release --target x86_64-unknown-linux-musl

# Verify it's static
file target/x86_64-unknown-linux-musl/release/choz
# Output: ... statically linked ...
```

## Project Structure

choz is a Cargo workspace of four crates:

```
crates/
├── choz-ports/        # RT-safe traits: FxProcessor, AudioSource
├── choz-engine/       # Audio engine, sources, 32 FX, MIDI, OSC, plugin scan/cache
├── choz-plugin-clap/  # CLAP hosting via clack-host (feature `clap`)
└── choz-ui/           # The `choz` binary: ratatui TUI, rack model, modals
```

See [docs/architecture.md](docs/architecture.md) for the module map, thread model,
data flow and plugin architecture, and [docs/roadmap.md](docs/roadmap.md) for the
session-by-session state and what is next.

## Layout

![Layout](docs/layout.png)

## License

MIT — 2026
