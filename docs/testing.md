# Testing choz

The README only gives the command to run the tests. This page covers what each crate tests, why some tests flake, the long sweeps and the diagnostic examples.

```bash
cargo test --workspace              # 894 tests
cargo clippy --workspace --all-targets -- -D warnings
```

| Crate | Tests | Covers |
|---|---|---|
| `choz-engine` | 450 | 56 FX processors, mixer, sources, SFZ parser, preset files and where a plugin keeps them, the transport and the metronome, the looper deck and its takes on disk, plugin paths, scan cache, quarantine, sandbox, OSC socket |
| `choz-ui` | 330 | Rack layout, parameter controls, modals, mouse hit-testing, MIDI learn (including the knob box paging under it), the mixer strips, note routing in both modes, project save/load, i18n, themes, background rendering, drawing at every terminal size, the installer script |
| `choz-plugin-lv2` | 34 | TTL parsing, hosting installed effects, `worker#schedule`, X11 editor discovery, state round-trip, `patch:Set` atoms from the UI |
| `choz-plugin-ladspa` | 14 | LADSPA + DSSI descriptors and runtime, step names from the `.rdf` sidecar |
| `choz-plugin-clap` | 13 | Effect and instrument runtime against installed plugins, window feed |
| `choz-clap` | 10 | choz as an instrument: sixteen stereo outs offered to the host, the track's audio reaching the rack's input, an empty rack rendering silence, the rack saved and restored as the host's state, the X11 window with the panels drawn in it, keys, the pointer and colours crossing from X, no child of the host ever spawned, and a real host scanning the built bundle |
| `choz-plugin-clap-export` | 9 | The bundle choz publishes: catalogue, parameters, a real host loading it |
| `choz-plugin-vst3` | 9 | Factory info, parameter changes reaching the processor, run loop, runtime |
| `choz-ports` | 8 | The shared types: parameter ranges, meters, the loop chunk |
| `choz-plugin-sandbox` | 6 | Shared-memory handshake, deadline behaviour, window request |
| `choz-plugin-vst2` | 6 | Host callback transport, automation feed, runtime |
| `choz-plugin-pd` | 4 | Pure Data patch discovery and hosting |

**Globals are why a test flakes.** The harness runs a crate's tests in parallel
in one process, and the transport, the meters and `capture_health` are
singletons by design. `choz-engine::test_locks` has **one lock per global** and
a test that needs two takes them in the same order as everything else; in
`choz-ui` the pair is `ui_guard()` and `UiRestore`, because loading a project
applies its language and colour process-wide. A test that reads a global to
check something about *its own* object is written wrong — ask the object.

Four suites use `harness = false`, because the test binary itself has to be able
to act as a worker process: `quarantine`, `sandboxed_plugin`, `scan_isolation`
(choz-engine) and `across_a_process` (choz-plugin-sandbox).

Runtime tests run against whatever plugins are installed on the machine and skip
themselves when a format has none, so a plugin-less CI stays green.

## Long sweeps

Hosting *every* installed plugin of a format is `#[ignore]`d — it takes minutes:

```bash
cargo test --release -p choz-plugin-lv2 -- --ignored
cargo test --release -p choz-plugin-ladspa -- --ignored
```

## Diagnostic examples

Not tests — small programs that measure something against the real machine:

```bash
cargo run -p choz-plugin-lv2  --example ui_probe    # open every LV2 X11 editor
cargo run -p choz-plugin-clap --example clap_gui_probe   # same for CLAP
cargo run -p choz-engine      --example latency_probe
cargo run -p choz-engine      --example devlist
cargo run --release -p choz-engine --example sf2_voices -- <sf2> 96000 128   # what a pedalful of notes costs
cargo run --release -p choz-engine --example pedal_bench -- <sf2> <vst3> [sandbox|inproc] [busy threads]
cargo run --release -p choz-engine --example param_shapes -- <vst3>          # what a plugin says vs what choz draws
```
