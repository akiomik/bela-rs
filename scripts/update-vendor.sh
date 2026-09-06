#!/bin/sh
# Update the vendored headers in bela-sys/vendor.
#
# Two trees live there and they are not the same kind of thing.
# vendor/bela is what `cargo xtask bindgen` generates the bindings
# from. vendor/ne10 generates nothing: bela-sys/src/ne10.rs is written
# by hand, and those two headers are the baseline `check-vendor` diffs
# a board against, so that a board image moving NE10 is noticed rather
# than discovered at run time. See docs/fft.md.
#
# Usage:
#   scripts/update-vendor.sh <git-ref>       vendor from BelaPlatform/Bela
#   scripts/update-vendor.sh --board [host]  vendor from a Bela board
#                                            (default root@bela.local)
#
# The Bela Gem image ships Bela versions that are not published to the
# upstream repository (see docs/board-facts.md), so --board is the
# normal way to pin for this project.
#
# After updating, regenerate the bindings (see bela-sys/README.md):
#   cargo xtask bindgen --sysroot <aarch64-sysroot>
#
# To find out whether an update is due — after installing a new board
# image, above all — compare the two:
#   cargo xtask check-vendor --board [user@host]
set -eu

REPO_URL="https://github.com/BelaPlatform/Bela"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="$ROOT/bela-sys/vendor/bela"
NE10_DEST="$ROOT/bela-sys/vendor/ne10"

# The include closure of wrapper.h. Extend this list if Bela.h grows new
# includes (the bindgen run will fail on a missing header).
HEADERS="Bela.h GPIOcontrol.h Utilities.h"

# The include closure of bela-sys/abi/ne10_abi.c, other than the C
# library's own. NE10_dsp.h includes NE10_types.h and nothing else of
# NE10's.
NE10_HEADERS="NE10_dsp.h NE10_types.h"
NE10_INCLUDE_DIR="/usr/include/ne10"
NE10_LIB="/usr/lib/aarch64-linux-gnu/libNE10.so.10"

case "${1:-}" in
"")
  echo "usage: $0 <bela-git-ref> | --board [user@host]" >&2
  exit 2
  ;;
--board)
  HOST="${2:-root@bela.local}"
  mkdir -p "$DEST/include"
  for h in $HEADERS; do
    scp -q "$HOST:/root/Bela/include/$h" "$DEST/include/"
  done
  scp -q "$HOST:/root/Bela/LICENSE" "$DEST/LICENSE"
  VERSION="$(awk '/#define BELA_(MAJOR|MINOR|BUGFIX)_VERSION/ { printf "%s.", $3 }' \
    "$DEST/include/Bela.h" | sed 's/\.$//')"
  GIT_HEAD="$(ssh "$HOST" 'git -C /root/Bela rev-parse --short HEAD' 2>/dev/null || echo unknown)"
  printf 'board %s: Bela %s (git HEAD %s + image overlay)\n' \
    "$HOST" "$VERSION" "$GIT_HEAD" > "$DEST/SOURCE"

  # NE10 comes from the board too, and only from the board: it is
  # Debian's package rather than anything Bela publishes, so there is
  # no upstream ref to vendor from.
  mkdir -p "$NE10_DEST/include"
  for h in $NE10_HEADERS; do
    scp -q "$HOST:$NE10_INCLUDE_DIR/$h" "$NE10_DEST/include/"
  done
  # The library's own identity, which the headers do not carry: a
  # rebuilt libNE10 with unchanged headers is exactly the case where
  # what the FFT does to its arguments has to be measured again
  # (scripts/probe-fft.sh). The soname says nothing — every build calls
  # itself libNE10.so.10.
  # shellcheck disable=SC2029 # the remote paths are meant to expand here
  NE10_VERSION="$(ssh "$HOST" "awk '/#define VERSION_(MAJOR|MINOR|REVISION)/ { printf \"%s.\", \$3 }' \
    $NE10_INCLUDE_DIR/versionheader.h" 2>/dev/null | sed 's/\.$//')"
  # shellcheck disable=SC2029 # the remote path is meant to expand here
  NE10_BUILD_ID="$(ssh "$HOST" "readelf -n $NE10_LIB | awk '/Build ID:/ { print \$3 }'" \
    2>/dev/null || echo unknown)"
  # shellcheck disable=SC2029 # the remote path is meant to expand here
  NE10_SHA256="$(ssh "$HOST" "sha256sum $NE10_LIB | cut -d' ' -f1" 2>/dev/null || echo unknown)"
  {
    printf 'board %s: NE10 %s\n' "$HOST" "${NE10_VERSION:-unknown}"
    printf 'library: %s\n' "$NE10_LIB"
    printf 'build-id: %s\n' "${NE10_BUILD_ID:-unknown}"
    printf 'sha256: %s\n' "${NE10_SHA256:-unknown}"
  } > "$NE10_DEST/SOURCE"
  ;;
*)
  REF="$1"
  SHA="$(git ls-remote "$REPO_URL.git" "refs/heads/$REF" "refs/tags/$REF" | head -n1 | cut -f1)"
  [ -n "$SHA" ] || SHA="$REF"
  TMP="$(mktemp -d)"
  trap 'rm -rf "$TMP"' EXIT
  curl -sfL "$REPO_URL/archive/$SHA.tar.gz" | tar xz -C "$TMP" --strip-components=1
  mkdir -p "$DEST/include"
  for h in $HEADERS; do
    cp "$TMP/include/$h" "$DEST/include/"
  done
  cp "$TMP/LICENSE" "$DEST/LICENSE"
  printf 'git %s\n' "$SHA" > "$DEST/SOURCE"
  ;;
esac

cat "$DEST/SOURCE"
if [ -f "$NE10_DEST/SOURCE" ]; then
  cat "$NE10_DEST/SOURCE"
fi
echo "Next: regenerate the bindings with \`cargo xtask bindgen\`"
