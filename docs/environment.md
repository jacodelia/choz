# Environment variables

| Variable | Effect |
|---|---|
| `PIPEWIRE_LATENCY` / `PIPEWIRE_QUANTUM` | Set by choz from the configured buffer size before opening the JACK client — the placement matters, they are read when the client opens. |
| `CHOZ_CLAP_STRICT_TEARDOWN=1` | Destroy CLAP plugins properly instead of leaking the ones known to crash. For debugging. |
| `CHOZ_LV2_STRICT_TEARDOWN=1` | Same for LV2 — this is how the quarantine probe finds out in the first place. |
| `CHOZ_KITTY_BG=0` | Draw the wallpaper as cell colours instead of using kitty's graphics protocol. |
| `CHOZ_SANDBOX_GUI=1` | Isolate every plugin that has a window, not only the ones that crashed. Safer against a GUI segfault, and it costs most of an audio block per plugin — which is what used to break the sound up. |
| `CHOZ_PROBE_RUNS=N` | How many times a plugin is probed before it is believed to be safe (default 3 — some crashes are races). |
| `CHOZ_VST2_DIR=<dir>` | Extra directory for the VST2 runtime tests, where the machine's instruments live. |
| `LV2_PATH`, `VST_PATH`, `VST3_PATH`, `CLAP_PATH`, `LADSPA_PATH`, `DSSI_PATH`, `SF2_PATH`, `SFZ_PATH` | Override the search path for that format. |

State lives in `~/.local/state/choz/`: `choz.log`, `plugins.json` (scan cache),
`plugin-paths.json`, `plugin-verdicts.json`, `plugin-sandbox.json`, `ui.json`.
