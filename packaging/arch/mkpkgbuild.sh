#!/bin/sh
# Fill packaging/arch/PKGBUILD.in from built release tarballs.
#
#   mkpkgbuild.sh <version> <dir-with-the-tarballs> > PKGBUILD
#
# The checksums have to come from the *published* files, so this runs after the
# tarballs exist — in the release workflow, or by hand against a directory of
# downloaded artifacts.
#
# A missing architecture is fatal rather than silently left as a placeholder: a
# PKGBUILD with `@SHA256_ARMV7@` in it fails at `makepkg` time on somebody
# else's machine, which is the worst place to find out.
set -eu

VER="${1:?usage: mkpkgbuild.sh <version> <dir>}"
DIR="${2:?usage: mkpkgbuild.sh <version> <dir>}"
HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

# Assigned, not interpolated straight into `sed`: a failure inside `$( )` used
# as an argument is swallowed, and the placeholder would ship empty.
sum() {
    file="$DIR/choz-$VER-$1.tar.gz"
    [ -f "$file" ] || { echo "mkpkgbuild.sh: missing $file" >&2; return 1; }
    sha256sum "$file" | cut -d' ' -f1
}

X86=$(sum x86_64-unknown-linux-gnu)
ARM64=$(sum aarch64-unknown-linux-gnu)
ARMV7=$(sum armv7-unknown-linux-gnueabihf)

sed -e "s|@VERSION@|$VER|g" \
    -e "s|@SHA256_X86_64@|$X86|" \
    -e "s|@SHA256_AARCH64@|$ARM64|" \
    -e "s|@SHA256_ARMV7@|$ARMV7|" \
    "$HERE/PKGBUILD.in"
