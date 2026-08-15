#!/bin/sh
# Install (or upgrade, or remove) choz for people who don't use .deb/.rpm.
#
#   ./packaging/install.sh              build with cargo, install into ~/.local
#   ./packaging/install.sh --prefix /usr/local
#   ./packaging/install.sh --binary target/release/choz     skip the build
#   ./packaging/install.sh --skip-deps-check      install without checking ALSA
#   ./packaging/install.sh --uninstall
#
# What it will never touch, install or uninstall: ~/.local/state/choz. The
# projects, the plugin paths and the settings are the user's, not the package's.
set -eu

PREFIX="${PREFIX:-$HOME/.local}"
BINARY=""
UNINSTALL=0
SKIP_DEPS=0
# Where an older copy may be hiding, whatever prefix is asked for now.
# `CHOZ_SEARCH_BINS` narrows that (the test suite sets it empty): this list is
# the one thing here that reaches outside `--prefix`, and deleting a binary
# outside the prefix has to be something the caller can turn off.
KNOWN_BINS="${CHOZ_SEARCH_BINS-$HOME/.local/bin/choz /usr/local/bin/choz /usr/bin/choz}"

say() { printf '%s\n' "$*"; }
die() { printf 'install.sh: %s\n' "$*" >&2; exit 1; }

while [ $# -gt 0 ]; do
    case "$1" in
        --prefix) PREFIX="${2:?--prefix needs a directory}"; shift 2 ;;
        --binary) BINARY="${2:?--binary needs a path}"; shift 2 ;;
        --uninstall) UNINSTALL=1; shift ;;
        --skip-deps-check) SKIP_DEPS=1; shift ;;
        -h|--help) sed -n '2,12p' "$0"; exit 0 ;;
        *) die "unknown option: $1" ;;
    esac
done

BIN_DIR="$PREFIX/bin"
APP_DIR="$PREFIX/share/applications"
ICON_DIR="$PREFIX/share/icons/hicolor/scalable/apps"
# The raster sizes. **Not a nicety**: GTK 3 loads theme icons through
# gdk-pixbuf, and librsvg 2.61 dropped the gdk-pixbuf SVG loader — so on a
# current desktop a scalable-only icon does not render and the menu draws its
# generic cog. The .svg stays for GTK 4 and Qt, which rasterise it themselves.
ICON_SIZES="16x16 24x24 32x32 48x48 64x64 128x128 256x256"
MIME_DIR="$PREFIX/share/mime/packages"
HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

remove_installed() {
    # Every copy on the known paths, plus this prefix's — an upgrade that leaves
    # the old binary earlier on PATH installs nothing the user can see.
    for bin in $KNOWN_BINS "$BIN_DIR/choz"; do
        [ -e "$bin" ] || continue
        version=$("$bin" --version 2>/dev/null || echo "unknown version")
        if [ -w "$(dirname "$bin")" ]; then
            rm -f "$bin" && say "removed $bin ($version)"
        else
            say "cannot remove $bin ($version): no write permission — try sudo"
        fi
    done
    for f in "$BIN_DIR/choz-launcher" "$APP_DIR/choz.desktop" \
             "$ICON_DIR/choz.svg" "$MIME_DIR/choz-project.xml"; do
        [ -e "$f" ] && rm -f "$f" && say "removed $f"
    done
    for size in $ICON_SIZES; do
        f="$PREFIX/share/icons/hicolor/$size/apps/choz.png"
        [ -e "$f" ] && rm -f "$f" && say "removed $f"
    done
    return 0
}

refresh_caches() {
    # Without these the menu entry and the *.choz.yml association only appear
    # (or linger) until the next login. Neither exists in a container, and
    # neither failing is fatal.
    command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$APP_DIR" 2>/dev/null || true
    command -v update-mime-database >/dev/null 2>&1 && update-mime-database "$PREFIX/share/mime" 2>/dev/null || true
    # GTK reads its icons out of a cache, and an icon that is not in it is a
    # menu entry with a blank square next to it.
    command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -qtf "$PREFIX/share/icons/hicolor" 2>/dev/null || true
    return 0
}

if [ "$UNINSTALL" = 1 ]; then
    remove_installed
    refresh_caches
    say "left alone: ${XDG_STATE_HOME:-$HOME/.local/state}/choz (projects and settings)"
    exit 0
fi

# What choz needs on the machine that runs it. Checked rather than assumed: the
# binary links ALSA and glibc, and *dlopens* libjack at runtime — so JACK and
# PipeWire are optional, and a box without them still gets audio through ALSA.
#
# A missing ALSA is fatal, because the result would be a choz that starts and
# then opens no audio device — a bug report waiting to happen rather than an
# install. `--skip-deps-check` is for the one case where continuing is right:
# staging an install for a machine that is not this one.
check_runtime_deps() {
    if [ "$SKIP_DEPS" = 1 ]; then
        say "skipping the runtime dependency check (--skip-deps-check)"
        return 0
    fi
    # No ldconfig at all (a container, a musl box): say so and carry on rather
    # than refuse over a question that could not be asked.
    if ! command -v ldconfig >/dev/null 2>&1; then
        say "note: no ldconfig here, so the runtime libraries were not checked"
        return 0
    fi
    if ! ldconfig -p 2>/dev/null | grep -q 'libasound\.so\.2'; then
        say "libasound.so.2 (ALSA) is missing — choz would start and open no audio device."
        say "  Debian/Ubuntu: sudo apt install libasound2t64   (or libasound2)"
        say "  Arch:          sudo pacman -S alsa-lib"
        say "  Fedora:        sudo dnf install alsa-lib"
        say "Install it and run this again, or pass --skip-deps-check to install anyway"
        say "(right when you are staging this for another machine)."
        exit 1
    fi
    if ! ldconfig -p 2>/dev/null | grep -q 'libjack\.so'; then
        say "note: libjack is not installed — choz will use ALSA."
        say "      For JACK/PipeWire routing: apt install libjack-jackd2-0 | pacman -S jack2 | dnf install jack-audio-connection-kit"
    fi
    return 0
}

# In a release tarball the binary sits right next to this script; building it
# again there needs a Rust toolchain the user has no reason to have.
if [ -z "$BINARY" ] && [ -x "$HERE/choz" ]; then
    BINARY="$HERE/choz"
    say "using the binary shipped next to this script ($BINARY)"
fi

if [ -z "$BINARY" ]; then
    command -v cargo >/dev/null 2>&1 || die "no cargo and no --binary; nothing to install"
    say "building (release)…"
    ( cd "$HERE/.." && cargo build --release --bin choz )
    BINARY="$HERE/../target/release/choz"
fi
[ -x "$BINARY" ] || die "$BINARY is not an executable"

new_version=$("$BINARY" --version 2>/dev/null || echo "choz (unknown)")
check_runtime_deps

# Upgrade means replacing what is there, not installing alongside it.
remove_installed

mkdir -p "$BIN_DIR" "$APP_DIR" "$ICON_DIR" "$MIME_DIR"
install -m 755 "$BINARY" "$BIN_DIR/choz"
install -m 755 "$HERE/desktop/choz-launcher" "$BIN_DIR/choz-launcher"
install -m 644 "$HERE/desktop/choz.desktop" "$APP_DIR/choz.desktop"
install -m 644 "$HERE/desktop/choz.svg" "$ICON_DIR/choz.svg"
for size in $ICON_SIZES; do
    install -d "$PREFIX/share/icons/hicolor/$size/apps"
    install -m 644 "$HERE/desktop/icons/$size/choz.png" \
        "$PREFIX/share/icons/hicolor/$size/apps/choz.png"
done
install -m 644 "$HERE/desktop/choz-project.xml" "$MIME_DIR/choz-project.xml"
say "installed $new_version into $PREFIX"

refresh_caches

case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) say "note: $BIN_DIR is not on your PATH" ;;
esac
