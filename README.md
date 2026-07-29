# choz

A terminal-based audio plugin host — like [Carla](https://kx.studio/Applications:Carla) for the terminal.

Built with Rust, ratatui, and cpal. Provides a TUI for managing audio sources and real-time FX chains.

## Status

choz is early-stage. The FX engine and TUI are real and working; audio
**sources** are still being built out. See the tables below for what works today
versus what is planned.

## Features

### Working today

- **Terminal UI** with full mouse and keyboard support, incl. a top **menu bar** (`F10` or click: File / Source / FX / Transport / Help) — every action is reachable by mouse
- **About dialog** with an in-terminal image logo rendered via [ratatui-image](https://github.com/benjajaja/ratatui-image) (Help → About)
- **27 FX processors**: delay, reverb, compressor, limiter, gate, expander, EQ, filters, chorus, flanger, phaser, bitcrusher, vinyl sim, cassette sim, tube saturation, stereo widener, isolator, looper, sidechain ducking, panner, and more
- **INPUTS → RACK model**: the left panel lists note inputs (every MIDI port, plus OSC). `Enter` on one creates a rack tab bound to it; pick its instrument (SF2 / WAV / CLAP synth) from the tab's `INSTR` line. Switch tabs with `[` / `]` or by clicking; `✕` (or `Backspace`) removes one.
- **Per-input routing**: notes from an input reach only the tabs bound to that input, so two controllers can drive two different instruments at once. The QWERTY piano always plays the active tab.
- **Selectable audio output**: `o` in the transport panel lists the backend's output devices; switching reloads the rack onto the new stream.
- **Per-slot FX**: up to 5 FX per source with live parameter tweaking
- **Per-slot mixer**: gain, constant-power pan, mute and solo on every rack slot (`-`/`+`, `,`/`.`, `m`, `S`, or scroll/click the strip)
- **Real-time audio** via cpal (ALSA / JACK, auto-detected through PipeWire)
- **RT-safe engine**: lock-free, zero-allocation audio callback (chains built on the UI thread, handed over an `rtrb` ring)
- **WAV file playback** as an audio source (`choz path/to/file.wav`, or browse in the SOURCE panel)
- **SF2 SoundFont synthesis** (via [oxisynth](https://github.com/PolyMeilex/OxiSynth)) — browse an `.sf2` in the SOURCE panel, then play it with the computer keyboard (`a w s e d f t g y h u j k`, one octave from C4)
- **SF2 preset selection**: the INPUTS panel's lower half lists every program in the active tab's SoundFont (`→` to focus it, `↑↓`, `Enter` to switch)
- **Hardware MIDI input** (via [midir](https://github.com/Boddlnagg/midir)) — connects every MIDI input port at startup; toggle individual ports with `c` in the INPUTS panel (`r` rescans)
- **OSC input** over UDP (port 9000, or `--osc-port N`): notes (`/note <note> <vel>`, `/note/on`, `/note/off`) as an input like any other, plus remote control — `/mix/<tab>/gain|pan|mute` and `/fx/<tab>/<fx>/<param>` (all indices 1-based, as drawn)
- **CLAP hosting** (via [clack-host](https://github.com/prokopyl/clack), behind `--features clap`) — instruments from the tab's `3:SYNTH` picker; CLAP *audio effects* at the bottom of the ADD FX list, **with their own parameters** on the knobs (names and ranges read from the plugin; changes go straight to the running plugin)
- **Cached plugin scan**: the CLAP scan (~236 ms for 20 plugins here) is cached in `~/.local/state/choz/plugins.json` and reused until a plugin directory changes; `r` in the SYNTH panel forces a rescan
- **Plugin *scanning*** for other formats (discovery only): LADSPA, DSSI, SFZ, SF2, JSFX

### Planned (not yet implemented)

- **Hosting for non-CLAP formats** (LADSPA/DSSI/LV2/VST) — scanned but not instantiated
- **Plugin GUIs** — parameters are edited on choz's knobs (first 7 of an effect); CLAP *instruments* don't expose theirs yet
- **Parameter automation**
- **Per-channel routing inside one port** — routing is per input port, not per MIDI channel

> The default source is a 440 Hz test tone until you load a WAV or SF2.

## Screenshot

```
 FILE  RACK  FX  TRANSPORT  HELP
┌─ INPUTS [ACTIVE] ───────────┐ ┌─ RACK ───────────────────────────────────────────┐
│ TAB: 1/2 SF2:FluidR3 ← LPK25│ │  SF2:FluidR3 ✕   ⊘WAV:loop ✕                     │
│ ↑↓ · Enter=bind tab · c=on/…│ │  VOL [▓▓▓▓░░░░] 1.00  PAN L───●───R C   MUTE SOLO │
│ ✓ MIDI LPK25    → tab 1     │ │  INSTR SF2:FluidR3          1:SF2  2:WAV  3:SYNTH │
│ · MIDI Midi Through         │ │  ←→=FX ↑↓=param wheel=value a=add d=del -/+=vol   │
│ ✓ OSC  OSC      → tab 2     │ │   1:DELAY  2:REVERB  [+ ADD]                      │
│                             │ │  ROUTING: IN -> 1:DELAY -> 2:REVERB -> OUT        │
│ PRESETS (→ to select)       │ │  [████████]  [████    ]                           │
│ 000:000 Yamaha Grand Piano  │ │   ↑0.95         ↙0.35                             │
│ 000:001 Bright Yamaha Grand │ │   Feedback      Wet                               │
│ 000:002 Electric Piano      │ │   ON  <-MOVE  DEL                                 │
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

# Build in debug mode
cargo build

# Run
cargo run
```

### Release Build (Optimized)

```bash
# Build with optimizations
cargo build --release

# The binary will be at:
#   target/release/choz
```

### Cross-compilation

```bash
# For a specific target
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
```

## Execution

```bash
# Run from source
cargo run

# Run the release binary directly
./target/release/choz

# With logging
RUST_LOG=info cargo run
RUST_LOG=debug cargo run
```

### Keyboard Controls

| Key | Context | Action |
|-----|---------|--------|
| `Tab` | Global | Cycle focus: Source → FX Chain → Transport |
| `q` | Global | Quit |
| `←` `→` | Inputs | Switch between the input list and the preset list |
| `↑` `↓` | Inputs / FX | Navigate the list / parameters |
| `↑` `↓` | FX Chain | Select parameter |
| `w` / `s` | FX Chain | Increase / decrease parameter |
| `Space` | FX Chain | Toggle FX enabled |
| `a` | FX Chain | Add new FX (opens selector) |
| `d` | FX Chain | Delete selected FX |
| `[` `]` | RACK | Previous / next source tab |
| `Backspace` | RACK | Remove the active source (or click the tab's `✕`) |
| `-` / `+` | RACK | Slot gain down / up |
| `,` / `.` | RACK | Pan left / right |
| `m` / `S` | RACK | Mute / solo the slot |
| `Enter` | Inputs | Bind the selected input to a rack tab (or jump to its tab) |
| `c` | Inputs | Connect / disconnect the selected input |
| `r` | Inputs | Rescan and reconnect MIDI ports |
| `Enter` | Inputs · presets | Load the selected SoundFont program |
| `1` `2` `3` | RACK | Set the tab's instrument: SF2 / WAV / synth |
| `r` | Synth picker | Rescan plugins (bypasses the cache) |
| `o` | Transport | Choose the audio output device |
| `Esc` | FX Selector | Close selector modal |
| `Enter` | FX Selector | Confirm FX selection |
| `Space` | Transport | Toggle play/stop |
| `s` | Transport | Stop |

### Mouse Controls

| Action | Result |
|--------|--------|
| Click panel | Focus that panel |
| Click an input | Bind it to a rack tab |
| Click an input's `✓`/`·` | Connect / disconnect it |
| Click `1:SF2` / `2:WAV` / `3:SYNTH` | Choose the tab's instrument |
| Click the `OUT` line | Choose the audio output device |
| Click FX slot | Select slot + show parameters |
| Click parameter knob | Select parameter |
| Scroll on parameter | Adjust value ±0.03 |
| Click `[+ ADD]` | Open FX selector |
| Click `ON`/`OFF` | Toggle FX enabled |
| Click `<-MOVE` / `MOVE->` | Reorder FX |
| Click `DEL` | Delete FX |
| Click `[SPACE]` / `[S] STOP` | Transport control |
| Click FX selector item | Choose FX kind |
| Click RACK tab | Switch active source |
| Click `✕` on a RACK tab | Remove that source |
| Scroll on `VOL` / `PAN` | Adjust slot gain / pan |
| Click `MUTE` / `SOLO` | Toggle slot mute / solo |
| Click an SF2 preset | Load that program |
| Click outside modal | Dismiss |

## Generating a Release

### Binary Release

```bash
# Build optimized binary
cargo build --release

# Strip debug symbols (Linux)
strip target/release/choz

# Check binary size
ls -lh target/release/choz

# Create tarball
VERSION=$(grep '^version' Cargo.toml | head -1 | cut -d'"' -f2)
tar -czf choz-${VERSION}-x86_64-linux.tar.gz \
    -C target/release choz \
    README.md docs/architecture.md

# Or use cargo-binstall compatible packaging
cargo package --no-verify
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

### CI/CD (GitHub Actions)

```yaml
name: Release

on:
  push:
    tags: ['v*']

jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: sudo apt install libasound2-dev libjack-dev
      - run: cargo build --release
      - run: strip target/release/choz
      - uses: softprops/action-gh-release@v1
        with:
          files: target/release/choz
```

## Project Structure

```
src/
├── main.rs           # App state, event loop, UI, mouse/keyboard
├── engine.rs         # Real-time audio engine (cpal callback)
├── fx_chain.rs       # FX chain builder from specs
├── source.rs         # Audio source types, FX kinds, params
├── registry.rs       # Plugin registry (scan/load/process)
├── scanner.rs        # Filesystem plugin discovery
├── plugin_types.rs   # Plugin format definitions
├── views/
│   ├── source_panel.rs   # Source selection panel
│   └── fx_chain_panel.rs # FX chain panel with knobs
└── fx/               # 27 DSP processors
    ├── delay.rs, reverb.rs, compressor.rs, gate.rs, ...
    └── utility.rs    # Gain, PhaseInvert, SoftClip, etc.
```

## Architecture

See [docs/architecture.md](docs/architecture.md) for detailed diagrams (Mermaid) covering:
- Module dependency graph
- Thread architecture (UI + audio callback)
- Data flow (input → state → render → audio)
- FX processing pipeline
- Plugin registry pattern
- Mouse interaction map

## License

MIT or Apache-2.0 (at your option)
