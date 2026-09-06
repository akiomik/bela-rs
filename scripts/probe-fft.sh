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
# Per run, and question 4 is one run per length: long enough for the
# longest transform the sweep asks for, short enough that a board that
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

# shellcheck disable=SC2029 # the remote path is meant to expand here
ssh -o ConnectTimeout=10 "$HOST" "chmod +x $REMOTE_DIR/ne10_probe"

echo "Asking questions 1, 2, 3, 5 and 6..."
echo
# Not `status=$?` on the line after: `set -e` would end the script on a
# failing ssh before the assignment ran, and the report at the bottom
# would be unreachable code.
status=0
# shellcheck disable=SC2029 # the remote path is meant to expand here
ssh -o ConnectTimeout=10 "$HOST" \
  "timeout -s INT -k 5 $RUN_TIMEOUT $REMOTE_DIR/ne10_probe 2>&1" || status=$?

# Question 4 runs one process per length, because a length that
# corrupts the heap ends the process it runs in — which is itself an
# answer, and one a single sweeping process could only give once.
#
# A probe that dies is a result; ssh failing to reach the board is not,
# and the two arrive the same way. ssh reports its own failures as 255
# and otherwise passes the remote command's status through, and the
# probe exits 0 to 3 or dies of a signal (134 for the abort a corrupt
# heap causes), so 255 is the transport and nothing else. Losing that
# distinction would print a transport failure as a DIED result and
# call a half-finished sweep a measurement.
echo
echo "4. which lengths plan and transform correctly? (one process each)"
echo
sweep_failed=0
length=2
while [ "$length" -le 65536 ]; do
  remote_status=0
  # shellcheck disable=SC2029 # the remote path and length expand here
  output="$(ssh -o ConnectTimeout=10 "$HOST" \
    "timeout -s INT -k 5 $RUN_TIMEOUT $REMOTE_DIR/ne10_probe $length 2>&1")" ||
    remote_status=$?
  if [ "$remote_status" -eq 255 ]; then
    # ssh has already printed why on this side; $output holds what the
    # board said, which for a connection that never opened is nothing.
    printf '  %-7s ssh failed (status 255); its reason is above\n' "$length" >&2
    sweep_failed="$length"
    break
  fi
  case "$output" in
  *"$length: ok"*) printf '  %-7s ok\n' "$length" ;;
  *"$length: "*) printf '  %-7s %s\n' "$length" "${output#*"$length": }" ;;
  *)
    # No line of its own: the process died before printing one.
    printf '  %-7s DIED: %s\n' "$length" "$(echo "$output" | tr '\n' ' ')"
    ;;
  esac
  length=$((length * 2))
done

echo
if [ "$status" -ne 0 ]; then
  echo "The first run exited $status: it could not ask, rather than getting" >&2
  echo "a surprising answer. Read the output above." >&2
  exit "$status"
fi
if [ "$sweep_failed" -ne 0 ]; then
  echo "The board stopped answering at $sweep_failed points: every length from" >&2
  echo "there on is unmeasured, and the answers above are a partial sweep." >&2
  exit 1
fi
echo "The board answered. Record the findings in docs/fft.md, with the"
echo "libNE10 build id from bela-sys/vendor/ne10/SOURCE beside them."
