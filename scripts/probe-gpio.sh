#!/bin/sh
# Ask a board what the sysfs GPIO family does, by running
# `bela-sys/examples/gpio_probe` on it and keeping the output.
#
# Usage: scripts/probe-gpio.sh [user@host] [--destructive]
#   host defaults to root@bela.local
#
# BELA_SYSROOT must point at a synced sysroot (scripts/sync-sysroot.sh)
# and BELA_CC at a cross compiler; see docs/cross-compile.md.
#
# This is an experiment, not a check, the way scripts/probe-io.sh is:
# it has no pass and no fail, and its findings belong in
# docs/board-facts.md. What it answers is what an application gets when
# it reaches for a pin — one that is free, and one libbela is holding —
# and those answers decide the shape of the safe API on top (issue
# #156).
#
# It runs in two passes, because the second question can only be asked
# of a board that is rendering:
#
#   1. the probe alone, on a board where nothing has claimed anything;
#   2. the probe beside `bela/examples/sine`, which holds the audio
#      device and libbela's own pins while the probe reaches for them.
#
# The probe itself never creates an audio system, which is what makes
# the second pass possible at all: libbela refuses a second one in
# another process, so a probe that brought its own could not ask.
#
# `--destructive` adds one more question to the second pass — what
# unexporting one of libbela's pins does to the run holding it — and is
# opt-in because the answer may be a stopped audio system.
#
# `bela_daemon` is stopped for the duration and restarted afterwards,
# as in scripts/smoke-test.sh: it would otherwise take the audio device
# and the pins this is asking about. Every remote run is bounded, an
# unbounded one being how a script waits forever on a board that
# stopped answering.
set -eu

HOST="root@bela.local"
DESTRUCTIVE=""
for arg in "$@"; do
  case "$arg" in
  --destructive) DESTRUCTIVE="--destructive" ;;
  -*)
    echo "unknown option: $arg (only --destructive is one)" >&2
    exit 2
    ;;
  *) HOST="$arg" ;;
  esac
done

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET="aarch64-unknown-linux-gnu"
BIN_DIR="${CARGO_TARGET_DIR:-$ROOT/target}/$TARGET/release/examples"
REMOTE_DIR="/tmp/bela-rs-probe-gpio"
# The second pass needs the run to outlive the probe, and by a margin:
# a probe that outlived it would measure a board with nothing running
# and the script would report both `ALREADY GONE` and `the undisturbed
# end`, which are the two conclusions this must never confuse. The
# probe takes well under a second, so 30 against a 20-second bound
# leaves the with-run questions ~26 seconds of run to happen inside.
# The probe checks for itself that the run was still up when it
# finished, which is what actually rules the confusion out.
RUN_SECONDS=30
PROBE_TIMEOUT=60
WITH_RUN_TIMEOUT=20

DAEMON_WAS_RUNNING=0
# The pin question 8 leaves exported on purpose. Pass 1 clears it
# itself on the happy path; this is so that an interrupted run, or one
# whose probe died before it could say, gives the pin back too. The
# probe is asked for the number rather than it being written here: it
# derives it from bank bases that are measured and can move.
LEFT_EXPORTED=""

cleanup() {
  status=$?
  # Stop any run still holding the audio device before the daemon is
  # started again, or it comes up unable to claim it. Matched by exact
  # process name: the remote cmdline is `./sine`, so a pattern built
  # from $REMOTE_DIR would match nothing and report success.
  ssh -o ConnectTimeout=10 "$HOST" "pkill -INT -x sine" 2>/dev/null || true
  if [ -n "$LEFT_EXPORTED" ]; then
    ssh -o ConnectTimeout=10 "$HOST" \
      "echo $LEFT_EXPORTED > /sys/class/gpio/unexport" 2>/dev/null || true
  fi
  ssh -o ConnectTimeout=10 "$HOST" "rm -rf $REMOTE_DIR" 2>/dev/null || true
  if [ "$DAEMON_WAS_RUNNING" -eq 1 ]; then
    ssh -o ConnectTimeout=10 "$HOST" "systemctl start bela_daemon" 2>/dev/null ||
      echo "WARNING: could not restart bela_daemon on $HOST" >&2
  fi
  # A caught signal in POSIX sh runs the handler and then *resumes*, so
  # without this a Ctrl-C during pass 1 would tidy up and then walk into
  # pass 2 with the directory deleted and the daemon holding the audio
  # device. Every other script in scripts/ ends its handler this way.
  exit "$status"
}

echo "Building the probe and an audio example for $TARGET..."
cargo build -p bela-sys --release --target "$TARGET" --example gpio_probe
cargo build -p bela --release --target "$TARGET" --example sine

for binary in gpio_probe sine; do
  if [ ! -x "$BIN_DIR/$binary" ]; then
    echo "not built at $BIN_DIR/$binary" >&2
    exit 2
  fi
done

# Only now: until the build has succeeded this invocation has no
# business touching a board, and a trap set earlier would answer a
# cross-compile failure by stopping whatever the board was running.
trap cleanup EXIT INT TERM

echo "Preparing $HOST..."
if ssh -o ConnectTimeout=10 "$HOST" "systemctl is-active --quiet bela_daemon" 2>/dev/null; then
  DAEMON_WAS_RUNNING=1
fi
ssh -o ConnectTimeout=10 "$HOST" "systemctl stop bela_daemon; mkdir -p $REMOTE_DIR"
for binary in gpio_probe sine; do
  scp -q -o ConnectTimeout=10 "$BIN_DIR/$binary" "$HOST:$REMOTE_DIR/$binary"
done
# shellcheck disable=SC2029 # the remote path is meant to expand here
ssh -o ConnectTimeout=10 "$HOST" "chmod +x $REMOTE_DIR/gpio_probe $REMOTE_DIR/sine"

# Ask before running anything, so that the handler can give the pin back
# even if pass 1 never reaches the line that clears it.
# shellcheck disable=SC2029
LEFT_EXPORTED="$(ssh -o ConnectTimeout=10 "$HOST" "$REMOTE_DIR/gpio_probe --will-leave" 2>/dev/null || true)"
case "$LEFT_EXPORTED" in
[0-9]*) echo "Question 8 will leave gpio$LEFT_EXPORTED exported; it will be cleared." ;;
*)
  echo "WARNING: the probe did not say which pin it leaves; an interrupted" >&2
  echo "pass 1 will leave one exported. Its output said: $LEFT_EXPORTED" >&2
  LEFT_EXPORTED=""
  ;;
esac

echo
echo "=============================================================="
echo "Pass 1: the probe alone"
echo "=============================================================="
# Not `status=$?` on the line after: `set -e` would end the script on a
# failing ssh before the assignment ran, and the report at the bottom
# would be unreachable code.
alone_status=0
# shellcheck disable=SC2029
ssh -o ConnectTimeout=10 "$HOST" "
  cd $REMOTE_DIR
  probe_out=\$(timeout -s INT -k 5 $PROBE_TIMEOUT ./gpio_probe 2>&1)
  probe_status=\$?
  echo \"\$probe_out\"
  echo
  echo '-- question 8: what is claimed now the probe has exited --'
  ls /sys/class/gpio | grep -vE 'gpiochip|^export\$|^unexport\$' | tr '\n' ' '
  echo
  # Question 8 leaves one behind on purpose. Clear it, so that pass 2
  # starts from the board's resting state rather than from this. The
  # pin comes from the probe rather than from a literal here: it
  # derives it from bank bases that are measured and can move.
  left=\$(echo \"\$probe_out\" | sed -n 's/^leaving-exported: //p')
  if [ -n \"\$left\" ]; then
    echo \"(clearing gpio\$left, which question 8 left on purpose)\"
    echo \"\$left\" > /sys/class/gpio/unexport 2>/dev/null || true
  else
    echo '(the probe named no pin to clear)'
  fi
  exit \$probe_status
" || alone_status=$?

# Disarm before pass 2 whatever pass 1 did. Pass 1 clears the pin as
# its last act, and its failure paths return before question 8 exports
# anything at all — but neither of those is what decides it. What does
# is that pass 2 has a live run holding that same pin, so a handler
# still armed there would unexport it out from under the run, which is
# the one act this script gates behind --destructive.
LEFT_EXPORTED=""

echo
echo "=============================================================="
echo "Pass 2: the probe beside a run${DESTRUCTIVE:+ (destructive)}"
echo "=============================================================="
# `sine` is backgrounded with its PID captured rather than matched on
# later: a cmdline that does not contain the path is how a pkill
# reports success and kills nothing. `timeout -s INT` bounds it even if
# the kill is missed, and the sleep is for `Bela_startAudio` to have
# claimed the pins before the probe looks at them.
with_run_status=0
# shellcheck disable=SC2029
ssh -o ConnectTimeout=10 "$HOST" "
  cd $REMOTE_DIR
  timeout -s INT -k 5 $RUN_SECONDS ./sine > sine.log 2>&1 &
  sine_pid=\$!
  sleep 4
  if ! kill -0 \$sine_pid 2>/dev/null; then
    echo 'sine did not stay up; its output was:'
    cat sine.log
    exit 3
  fi
  timeout -s INT -k 5 $WITH_RUN_TIMEOUT ./gpio_probe --with-run $DESTRUCTIVE
  probe_status=\$?
  echo
  echo '-- what the run did while that happened --'
  if kill -0 \$sine_pid 2>/dev/null; then
    echo '   still up at the moment the probe finished'
  else
    echo '   ALREADY GONE before the probe finished'
  fi
  # The status, not just liveness: a run that aborts a second after the
  # probe touched its pin is still alive for the check above, and
  # throwing the status away is how that gets recorded as a run which
  # did not notice. 124 is timeout ending it, the undisturbed end here.
  wait \$sine_pid
  sine_status=\$?
  case \$sine_status in
  124) echo '   ended at 124: its own timeout, which is the undisturbed end' ;;
  0) echo '   ended at 0' ;;
  *) echo '   ended at' \$sine_status '- NOT the undisturbed end; see sine.log' ;;
  esac
  echo '-- the run has now ended; its last lines --'
  tail -5 sine.log
  echo
  echo '-- question 13: what is claimed after both processes exited --'
  ls /sys/class/gpio | grep -vE 'gpiochip|^export\$|^unexport\$' | tr '\n' ' '
  echo
  exit \$probe_status
" || with_run_status=$?

echo
if [ "$alone_status" -ne 0 ] || [ "$with_run_status" -ne 0 ]; then
  echo "A pass exited non-zero (alone $alone_status, with-run $with_run_status):" >&2
  echo "it could not ask, rather than getting a surprising answer." >&2
  exit 1
fi
echo "The board answered. Record the findings in docs/board-facts.md."
echo
echo "If a pin is still exported above that was not before, this probe"
echo "left it there: that is question 13's answer and not a tidy-up the"
echo "script forgot."
echo
echo "One thing is not covered by any of this. Question 6 sets an LED"
echo "trigger to \`none\` and puts it back a line later; a probe killed"
echo "in between — by its timeout, or by a signal — leaves that LED"
echo "changed, and the handler here covers the GPIO export, the remote"
echo "directory and the daemon rather than the triggers. If a run was"
echo "interrupted, check:"
echo "  ssh $HOST 'grep -o \"\\[[a-z0-9-]*\\]\" /sys/class/leds/beaglebone:green:usr*/trigger'"
