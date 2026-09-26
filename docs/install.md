# Installing choz

The README only gives the install command. This page covers what a built choz needs at runtime and every way to install it.

## What it needs at runtime

Different from what it needs to *build*. The binary links two things and opens a
third by hand:

| Library | Needed? | If missing |
|---|---|---|
| `libc` | yes | nothing runs |
| `libasound.so.2` (ALSA) | yes | choz starts but opens no audio device |
| `libjack.so.0` | **optional** — `dlopen`ed at runtime; normally PipeWire's (`pipewire-jack`), jack2's works the same | choz uses ALSA; no JACK/PipeWire routing, no per-channel outputs |
| `libpd` (Pure Data) | **optional** — linked only by `choz-pd-host`, from `libpd-dev` (not `puredata-dev`) | choz installs and runs; Pure Data patches cannot be hosted |
| X11 | not linked | plugin windows go through `x11rb`, which speaks the protocol itself |

That is why the `.deb` declares only `libasound2t64` and `libc6`: JACK is a
runtime choice, not a build-time dependency. **The packages refuse to install
without ALSA**, and that is read off the built packages rather than intended:
`dpkg-deb -f` shows `Depends: libasound2t64 (>= 1.0.29), libc6 (>= 2.43)`, and
the `.rpm` requires `libasound.so.2()(64bit)` down to its `ALSA_0.9` symbol
versions, so `apt` and `rpm -i` both stop. JACK is a `Recommends` in both:
`pipewire-jack` first on Debian/Ubuntu (jack2's library still satisfies it), and
the `libjack.so.0` soname on Fedora, which PipeWire's JACK provides. On Arch it
is an `optdepends` on `pipewire-jack`.

`install.sh` checks all three before it copies anything. **A missing ALSA stops
the install** — a choz that starts and then opens no device looks like a bug in
choz, not a missing package — and it prints the command for your distribution. A
missing JACK is only a note; a missing libpd is a note **and** it decides what
gets built: without it choz installs without the Pure Data half rather than
failing over it. `--skip-deps-check` installs anyway, which is right when you are
staging an install for a machine that is not this one.

## Install

**From a release** — no toolchain needed. Every tag publishes a `.tar.gz` per
architecture (x86-64, aarch64, armv7), a `.deb`, an `.rpm` and a `PKGBUILD` for
Arch, plus `SHA256SUMS.txt`:

```bash
tar xzf choz-1.3.6-x86_64-unknown-linux-gnu.tar.gz
cd choz-1.3.6-x86_64-unknown-linux-gnu
./install.sh            # uses the binary shipped beside it — no cargo involved
```

The tarball carries the binary, the launcher, the desktop entry, every icon size,
the MIME type, the wallpapers, choz's own effects and artifacts as a CLAP plugin
and the Pure
Data host — the same set the `.deb` installs. On ARM, remember that **plugins are
native binaries**: a Raspberry Pi loads plugins built for ARM, not the x86 ones.

**What an install puts down besides choz itself:**

| What | Where | Why |
|---|---|---|
| `choz.clap` | `~/.clap` (script) or `/usr/lib/clap` (packages) | choz's own 56 effects plus the arpeggiator and step sequencer as note effects, usable from Bitwig, Reaper, Carla or any CLAP host. Every effect publishes its full knob list, so all of them are automatable and saveable from the host. `--no-clap` skips it. |
| `choz-rack.clap` | the same places | **choz itself**, as a CLAP instrument: the whole rack on a track in the DAW, with its own window and **sixteen stereo outputs** — put a tab on pair 4 in the rack and it arrives on the host's fourth output, on its own track, the way a sampler's individual outs do. The track's own audio arrives as `host:in_1`/`in_2`, so a tab can process it and the rack is an effect chain too. The rack is saved with the host's session. It takes the host's notes and MIDI but **not its transport**: tempo, meter and play are choz's own. Ardour does not load CLAP — record choz there through JACK/PipeWire and the direct outs (manual, section 2.5). |
| Wallpapers | `<prefix>/share/choz/wallpapers` | A fresh install opens on the image choz ships with, and the picker starts there. |
| `choz-pd-host` | next to `choz` | The only binary that links libpd — installed when libpd is present. |

**From a checkout** — the same script builds first:

```bash
./packaging/install.sh                    # build, then install into ~/.local
./packaging/install.sh --prefix /usr/local
./packaging/install.sh --binary target/release/choz   # skip the build
./packaging/install.sh --skip-deps-check   # install without checking ALSA
./packaging/install.sh --no-clap          # skip the CLAP plugin (effects + artifacts)
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
