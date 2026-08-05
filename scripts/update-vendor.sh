#!/bin/sh
# Update the vendored Bela headers in bela-sys/vendor/bela.
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

# The include closure of wrapper.h. Extend this list if Bela.h grows new
# includes (the bindgen run will fail on a missing header).
HEADERS="Bela.h GPIOcontrol.h Utilities.h"

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
echo "Next: regenerate the bindings with \`cargo xtask bindgen\`"
