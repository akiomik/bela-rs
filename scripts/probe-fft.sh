#!/bin/sh
# Ask a board what its NE10 FFT does, by running
# `bela-sys/examples/ne10_probe` on it and keeping the output.
#
# Usage: scripts/probe-fft.sh [user@host]
#   host defaults to root@bela.local
#
# BELA_SYSROOT must point at a synced sysroot (scripts/sync-sysroot.sh)
# and BELA_CC at a cross compiler; see docs/cross-compile.md.
#
# This is an experiment, not a check, the way scripts/probe-io.sh is:
# it has no pass and no fail, and its findings belong in docs/fft.md.
# What it answers is which behaviour this build of libNE10 has — does a
# transform write into its input, does the inverse scale, which lengths
# work, does alignment matter, how many bins are written — and those
# answers decide the shape of the safe API on top (issue #138).
#
# Unlike the other probe scripts it creates no audio system: NE10 is a
# plain C library and the probe calls nothing else. So `bela_daemon` is
# left running, no board state is touched, and a failure leaves nothing
# behind. The run is still bounded, because an unbounded remote command
# is how a script waits forever on a board that stopped answering.
#
# Re-run it after a board image update: the answers describe one build
# of libNE10, identified in bela-sys/vendor/ne10/SOURCE, and
# `cargo xtask check-vendor --board` is what notices that build has
# changed.
set -eu

HOST="${1:-root@bela.local}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET="aarch64-unknown-linux-gnu"
BIN_DIR="${CARGO_TARGET_DIR:-$ROOT/target}/$TARGET/release/examples"
REMOTE_DIR="/tmp/bela-rs-probe-fft"
# Long enough for the length sweep, which allocates and transforms
# every power of two up to 65536; short enough that a board that
# stopped answering is noticed rather than waited on.
RUN_TIMEOUT=120

echo "Building the probe for $TARGET..."
cargo build -p bela-sys --release --target "$TARGET" --example ne10_probe

if [ ! -x "$BIN_DIR/ne10_probe" ]; then
  echo "not built at $BIN_DIR/ne10_probe" >&2
  exit 2
fi

echo "Copying it to $HOST..."
ssh -o ConnectTimeout=10 "$HOST" "mkdir -p $REMOTE_DIR"
scp -q -o ConnectTimeout=10 "$BIN_DIR/ne10_probe" "$HOST:$REMOTE_DIR/ne10_probe"

echo "Running it..."
echo
ssh -o ConnectTimeout=10 "$HOST" \
  "chmod +x $REMOTE_DIR/ne10_probe && timeout -s INT -k 5 $RUN_TIMEOUT $REMOTE_DIR/ne10_probe 2>&1"
status=$?

echo
if [ "$status" -eq 0 ]; then
  echo "The board answered. Record the findings in docs/fft.md, with the"
  echo "libNE10 build id from bela-sys/vendor/ne10/SOURCE beside them."
else
  echo "The probe exited $status: it could not ask, rather than getting a"
  echo "surprising answer. Read the output above." >&2
fi
exit "$status"
