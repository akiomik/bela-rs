#!/bin/sh
# Build the examples, run them on a board and check they actually
# rendered audio — a single pass/fail answer for "does this still work
# on hardware".
#
# Usage: scripts/smoke-test.sh [user@host] [seconds]
#   host defaults to root@bela.local, seconds to 5
#
# BELA_SYSROOT must point at a synced sysroot (scripts/sync-sysroot.sh);
# see docs/cross-compile.md.
#
# Needs a board, so it cannot run in CI. Run it before releasing, after
# updating the board image, and whenever a change touches the device
# path.
#
# `bela_daemon` is stopped for the duration and restarted afterwards,
# including when a check fails or the script is interrupted.
set -eu

HOST="${1:-root@bela.local}"
DURATION="${2:-5}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET="aarch64-unknown-linux-gnu"
BIN_DIR="${CARGO_TARGET_DIR:-$ROOT/target}/$TARGET/release/examples"
REMOTE_DIR="/tmp/bela-rs-smoke"
# `print` carries the numeric checks; the other two only have to start,
# keep running and stop cleanly.
EXAMPLES="print sine passthrough"
# How much of the run may be spent starting audio up rather than
# rendering (measured at about 0.6 s; rounded up for headroom).
STARTUP_ALLOWANCE_SECONDS=1.5
# Shorter runs are all startup and the block count check stops meaning
# anything.
MIN_DURATION=3

failures=0
daemon_was_active=no
LOG_DIR="$(mktemp -d)"

pass() { printf '  ok    %s\n' "$1"; }
fail() {
  printf '  FAIL  %s\n' "$1"
  failures=$((failures + 1))
}

# The image greets every ssh session with a login banner and a locale
# warning on stderr, which would bury the checks. Hold the remote stderr
# back and show it only when the command actually fails.
remote() {
  # shellcheck disable=SC2029 # callers build the command on this side on purpose
  if ! ssh -o ConnectTimeout=10 "$HOST" "$1" 2> "$LOG_DIR/ssh.err"; then
    sed 's/^/        /' "$LOG_DIR/ssh.err" >&2
    return 1
  fi
}

if [ -z "${BELA_SYSROOT:-}" ]; then
  echo "BELA_SYSROOT is not set; see docs/cross-compile.md" >&2
  exit 2
fi

if [ "$DURATION" -lt "$MIN_DURATION" ]; then
  echo "seconds must be at least $MIN_DURATION" >&2
  exit 2
fi

# Leave the board as it was found, whether the checks passed, one of
# them failed, or the run was interrupted.
restore() {
  status=$?
  # shellcheck disable=SC2029 # the remote path is meant to expand here
  ssh -o ConnectTimeout=10 "$HOST" "rm -rf $REMOTE_DIR" 2>/dev/null || true
  if [ "$daemon_was_active" = yes ]; then
    ssh -o ConnectTimeout=10 "$HOST" "systemctl start bela_daemon" 2>/dev/null ||
      echo "WARNING: could not restart bela_daemon on $HOST" >&2
  fi
  rm -rf "$LOG_DIR"
  exit "$status"
}
trap 'restore' EXIT INT TERM

echo "Building the examples for $TARGET..."
cargo build -p bela --release --target "$TARGET" --examples

echo "Preparing $HOST..."
# Only restart the daemon afterwards if it was running to begin with.
if ssh -o ConnectTimeout=10 "$HOST" "systemctl is-active --quiet bela_daemon" 2>/dev/null; then
  daemon_was_active=yes
fi
remote "systemctl stop bela_daemon; mkdir -p $REMOTE_DIR"

# The remote half: run one example for a while, interrupt it, and
# report how it went. Kept on the board so the quoting stays readable.
cat > "$LOG_DIR/run-remote.sh" <<'REMOTE'
#!/bin/sh
# usage: run-remote.sh <binary> <seconds>
set -eu
binary="$1"
seconds="$2"
cd "$(dirname "$0")"
chmod +x "./$binary"

"./$binary" > "$binary.log" 2>&1 &
pid=$!
sleep "$seconds"

if ! kill -0 "$pid" 2>/dev/null; then
  wait "$pid" || true
  echo "state=exited-early"
  exit 0
fi

# A clean shutdown on SIGINT is part of what is being tested: the
# signal handler asks the audio system to stop, cleanup runs, and the
# process exits 0.
kill -INT "$pid"
waited=0
while kill -0 "$pid" 2>/dev/null; do
  if [ "$waited" -ge 50 ]; then
    kill -9 "$pid" 2>/dev/null || true
    echo "state=hung"
    exit 0
  fi
  sleep 0.1
  waited=$((waited + 1))
done
if wait "$pid"; then
  code=0
else
  code=$?
fi
echo "state=stopped exit=$code"
REMOTE
scp -q -o ConnectTimeout=10 "$LOG_DIR/run-remote.sh" "$HOST:$REMOTE_DIR/run-remote.sh"

for example in $EXAMPLES; do
  echo "Running $example for ${DURATION}s on $HOST..."
  if [ ! -x "$BIN_DIR/$example" ]; then
    fail "$example: not built at $BIN_DIR/$example"
    continue
  fi
  scp -q -o ConnectTimeout=10 "$BIN_DIR/$example" "$HOST:$REMOTE_DIR/$example"
  result="$(remote "sh $REMOTE_DIR/run-remote.sh $example $DURATION" || echo state=ssh-failed)"
  remote "cat $REMOTE_DIR/$example.log" > "$LOG_DIR/$example.log" 2>/dev/null || true

  case "$result" in
  "state=stopped exit=0") pass "$example: ran for ${DURATION}s and exited 0 on SIGINT" ;;
  state=exited-early)
    fail "$example: exited before it was interrupted"
    sed 's/^/        /' "$LOG_DIR/$example.log" >&2
    continue
    ;;
  state=hung)
    fail "$example: still running 5s after SIGINT"
    continue
    ;;
  *)
    fail "$example: $result"
    sed 's/^/        /' "$LOG_DIR/$example.log" >&2
    continue
    ;;
  esac
done

# The numeric part. `print` reports the audio configuration from setup,
# a heartbeat from render and the total from cleanup, which is enough to
# tell a callback that runs at the right rate from one that runs once,
# late, or not at all.
log="$LOG_DIR/print.log"
if [ ! -s "$log" ]; then
  fail "print: produced no output"
else
  # setup: 44100 Hz, 16 frames per block, 2 in / 2 out audio channels, thread 0/1
  sample_rate="$(awk '/^setup:/ { print $2; exit }' "$log")"
  block_size="$(awk '/^setup:/ { print $4; exit }' "$log")"
  # render: 2756 blocks, 44080 frames elapsed, 0 underruns
  blocks="$(awk '/^render:/ { blocks = $2 } END { print blocks + 0 }' "$log")"
  frames="$(awk '/^render:/ { frames = $4 } END { print frames + 0 }' "$log")"
  underruns="$(awk '/^render:/ { underruns = $6 } END { print underruns + 0 }' "$log")"
  # cleanup: 12075 blocks rendered
  total="$(awk '/^cleanup:/ { print $2; exit }' "$log")"

  if [ -z "$sample_rate" ] || [ -z "$block_size" ]; then
    fail "print: no setup line"
  else
    pass "print: setup reported ${sample_rate} Hz, $block_size frames per block"
  fi

  if [ "$blocks" -eq 0 ]; then
    fail "print: render never reported a block"
  else
    # Every reporting block prints the frame count as it was at the
    # start of that block, so this identity is exact — not an estimate.
    expected_frames=$(( (blocks - 1) * block_size ))
    if [ "$frames" -eq "$expected_frames" ]; then
      pass "print: $blocks blocks match $frames elapsed frames at $block_size frames per block"
    else
      fail "print: $blocks blocks imply $expected_frames elapsed frames, got $frames"
    fi

    # Blocks per second must follow the sample rate: a callback that
    # runs at half speed, or stalls partway through, shows up here.
    #
    # The window is asymmetric on purpose. Audio cannot run ahead of the
    # wall clock, but it starts late — loading the PRU firmware and
    # bringing up the codec took about 0.6 s on the reference board — so
    # the count is compared against the run time minus a startup
    # allowance at the bottom and the full run time at the top.
    min_blocks="$(awk -v r="$sample_rate" -v b="$block_size" \
      -v s="$DURATION" -v a="$STARTUP_ALLOWANCE_SECONDS" \
      'BEGIN { printf "%d", r / b * (s - a) }')"
    max_blocks="$(awk -v r="$sample_rate" -v b="$block_size" -v s="$DURATION" \
      'BEGIN { printf "%d", r / b * s }')"
    if [ "$total" -ge "$min_blocks" ] && [ "$total" -le "$max_blocks" ]; then
      pass "print: $total blocks in ${DURATION}s, within $min_blocks..$max_blocks"
    else
      fail "print: $total blocks in ${DURATION}s, outside $min_blocks..$max_blocks"
    fi
  fi

  if [ "$underruns" -eq 0 ]; then
    pass "print: no underruns"
  else
    fail "print: $underruns underruns"
  fi

  if [ -z "$total" ]; then
    fail "print: cleanup did not run"
  else
    pass "print: cleanup ran after $total blocks"
  fi
fi

echo
if [ "$failures" -eq 0 ]; then
  echo "smoke test passed"
else
  echo "smoke test failed ($failures check(s))"
  exit 1
fi
