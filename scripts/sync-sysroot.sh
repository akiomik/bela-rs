#!/bin/sh
# Sync the parts of a Bela board's filesystem needed to cross-link
# device binaries (the equivalent of Bela's own SyncBelaSysroot), plus
# the sources that answer what the headers do not.
#
# The two are not the same thing. Everything under /usr, /root/Bela/lib
# and /root/Bela/include is compiled or linked against, and so is
# /root/Bela/libraries, which is where <libraries/<Name>/<Name>.h>
# resolves to. /root/Bela/core is neither: nothing here builds it, and
# it is carried because what a call into libbela costs the audio thread
# is written in the .cpp and nowhere else.
#
# Usage: scripts/sync-sysroot.sh [destination] [user@host]
#   destination defaults to ./bela-sysroot, host to root@bela.local
#
# Point BELA_SYSROOT at the destination when building; see
# docs/cross-compile.md.
set -eu

DEST="${1:-bela-sysroot}"
HOST="${2:-root@bela.local}"

# Paths recorded in docs/board-facts.md: the Bela headers, the sources
# a program reaches as <libraries/<Name>/<Name>.h>, the sources of the
# real-time plumbing underneath them, the Bela shared objects, the EVL
# real-time runtime, seasocks, and the Debian headers/libraries they
# all depend on.
#
# /root/Bela/libraries is 3 MB against the sysroot's 820, and without it
# the only trace of a class like Midi is the forwarding shim left in
# include/legacy — which says where the header is but does not carry it.
#
# /root/Bela/core is 564 KB and answers the question the headers cannot:
# include/ declares AuxTaskNonRT, RtNonRtMsgFifo and CircularBuffer,
# while what a call through them costs the audio thread — whether it
# blocks, what it does when a pipe is full, which side of the EVL
# boundary it runs on — is written in the .cpp.
PATHS="/root/Bela/include /root/Bela/libraries /root/Bela/core \
/root/Bela/lib /usr/evl /usr/local/lib /usr/include \
/usr/lib/aarch64-linux-gnu /usr/lib/gcc"

mkdir -p "$DEST"
# -R keeps the absolute paths, -l/-K preserve the many symlinks Debian
# uses without following them out of the tree.
#
# No -z: the board reaches the host over USB 2.0 either way, so gzip on
# its Cortex-A53 is the bottleneck rather than the link. Dropping it cut
# a full sync from 163 s to about 40 s (see docs/board-network.md).
#
# --chmod=ug-s drops setuid/setgid bits, which a sysroot never needs.
# Keeping them makes rsync fail on files like utempter, whose setgid
# bit an unprivileged user on the host cannot reproduce; that failure
# aborts this script before the symlinks below are created.
rsync -aR --chmod=ug-s --delete --links --keep-dirlinks "$HOST:$PATHS" "$DEST/"

# Debian merges /lib into /usr/lib, and the linker resolves the ELF
# interpreter and the linker scripts through that path.
ln -sfn usr/lib "$DEST/lib"
ln -sfn aarch64-linux-gnu/ld-linux-aarch64.so.1 \
  "$DEST/usr/lib/ld-linux-aarch64.so.1"

echo "Synced $HOST into $DEST ($(du -sh "$DEST" | cut -f1))"
