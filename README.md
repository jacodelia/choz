# choz

A terminal-based audio plugin host — like [Carla](https://kx.studio/Applications:Carla) for the terminal.

Built with Rust, ratatui, and cpal. Provides a TUI for managing audio sources and real-time FX chains.

## Features

- **Terminal UI** with mouse and keyboard support
- **Audio sources**: MIDI, SoundFonts (SF2/SF3), audio files (WAV), synthesizer plugins
- **27 FX processors**: delay, reverb, compressor, limiter, gate, expander, EQ, filters, chorus, flanger, phaser, bitcrusher, vinyl sim, cassette sim, tube saturation, stereo widener, isolator, looper, sidechain ducking, panner, and more
- **Up to 8 FX slots** per chain with parameter automation
- **Real-time audio** via cpal (ALSA, WASAPI, CoreAudio)
- **Plugin scanning**: LADSPA, DSSI, SFZ, SF2, JSFX discovery
- **Low latency**: fixed buffer size, zero-allocation audio callback

## Screenshot

```
┌─ SOURCE [ACTIVE] ───────────┐ ┌─ FX CHAIN ───────────────────────────────────────┐
│ Now: MIDI                     │ │  ←→=select FX  ↑↓=param  wheel=value  a=add d=del │
│                               │ │   1:DELAY   2:REVERB   [+ ADD]                   │
│  MIDI   SFZ   AUDIO   SYNTH   │ │                                                  │
│                               │ │  ROUTING: IN -> 1:DELAY -> 2:REVERB -> OUT       │
│ Available MIDI ports:         │ │  [████████]  [████    ]                          │
│   0: default                  │ │   ↑0.95         ↙0.35                            │
│                               │ │   Feedback      Wet                              │
│                               │ │                                                  │
│ Output: MIDI passthrough      │ │   ON  <-MOVE  DEL                               │
└───────────────────────────────┘ └──────────────────────────────────────────────────┘
┌─ TRANSPORT ───────────────────────────────────────────────────────────────────────┐
│  [SPACE]   [S] STOP   PLAYING                                                     │
└────────────────────────────────────────────────────────────────────────────────────┘
 choz v0.1 | SOURCE: MIDI | FX: 2 slots | ▶ PLAYING | Tab=switch q=quit
```

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
| `←` `→` | Source | Select category / FX slot |
| `↑` `↓` | Source / FX | Navigate ports / parameters |
| `1` `2` `3` `4` | Source | Select category (MIDI/SF2/AUDIO/SYNTH) |
| `↑` `↓` | FX Chain | Select parameter |
| `w` / `s` | FX Chain | Increase / decrease parameter |
| `Space` | FX Chain | Toggle FX enabled |
| `a` | FX Chain | Add new FX (opens selector) |
| `d` | FX Chain | Delete selected FX |
| `Esc` | FX Selector | Close selector modal |
| `Enter` | FX Selector | Confirm FX selection |
| `Space` | Transport | Toggle play/stop |
| `s` | Transport | Stop |

### Mouse Controls

| Action | Result |
|--------|--------|
| Click panel | Focus that panel |
| Click source category tab | Select category |
| Click FX slot | Select slot + show parameters |
| Click parameter knob | Select parameter |
| Scroll on parameter | Adjust value ±0.03 |
| Click `[+ ADD]` | Open FX selector |
| Click `ON`/`OFF` | Toggle FX enabled |
| Click `<-MOVE` / `MOVE->` | Reorder FX |
| Click `DEL` | Delete FX |
| Click `[SPACE]` / `[S] STOP` | Transport control |
| Click FX selector item | Choose FX kind |
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
    README.md ARCHITECTURE.md

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

See [ARCHITECTURE.md](ARCHITECTURE.md) for detailed diagrams (Mermaid) covering:
- Module dependency graph
- Thread architecture (UI + audio callback)
- Data flow (input → state → render → audio)
- FX processing pipeline
- Plugin registry pattern
- Mouse interaction map

## License

MIT or Apache-2.0 (at your option)
