#!/bin/sh
# Sync the parts of a Bela board's filesystem needed to cross-link
# device binaries (the equivalent of Bela's own SyncBelaSysroot).
#
# Usage: scripts/sync-sysroot.sh [destination] [user@host]
#   destination defaults to ./bela-sysroot, host to root@bela.local
#
# Point BELA_SYSROOT at the destination when building; see
# docs/cross-compile.md.
set -eu

DEST="${1:-bela-sysroot}"
HOST="${2:-root@bela.local}"

# Paths recorded in docs/board-facts.md: the Bela headers and
# libraries, the EVL real-time runtime, seasocks, and the Debian
# headers/libraries they all depend on.
PATHS="/root/Bela/include /root/Bela/lib /usr/evl /usr/local/lib \
/usr/include /usr/lib/aarch64-linux-gnu /usr/lib/gcc"

mkdir -p "$DEST"
# -R keeps the absolute paths, -l/-K preserve the many symlinks Debian
# uses without following them out of the tree.
rsync -azR --delete --links --keep-dirlinks "$HOST:$PATHS" "$DEST/"

# Debian merges /lib into /usr/lib, and the linker resolves the ELF
# interpreter and the linker scripts through that path.
ln -sfn usr/lib "$DEST/lib"
ln -sfn aarch64-linux-gnu/ld-linux-aarch64.so.1 \
  "$DEST/usr/lib/ld-linux-aarch64.so.1"

echo "Synced $HOST into $DEST ($(du -sh "$DEST" | cut -f1))"
