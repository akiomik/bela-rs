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
# An experiment, not a check, the way scripts/probe-io.sh is: no pass
# and no fail, read by a person, and its findings belong in
# docs/board-facts.md. Two passes, because the second question can only
# be asked of a board that is rendering: the probe alone, then the
# probe beside `bela/examples/sine`, which holds the audio device and
# libbela's own pins while the probe reaches for them.
#
# `--destructive` adds question 12 and widens question 11; both contend
# with a live run, which is why they are opt-in.
#
# What is put back: `bela_daemon`, the remote directory, the run, and
# the two LED pins — `gpio_probe --release`, before pass 1, after each
# pass and from the handler. What is not put back is printed at the end
# of every run, along with how to check it.
set -eu

HOST="root@bela.local"
HOST_GIVEN=""
DESTRUCTIVE=""
for arg in "$@"; do
  case "$arg" in
  --destructive) DESTRUCTIVE="--destructive" ;;
  -*)
    echo "unknown option: $arg (only --destructive is one)" >&2
    exit 2
    ;;
  *)
    # One positional. A second used to overwrite the first silently,
    # and `probe-gpio.sh destructive` became `ssh destructive`.
    if [ -n "$HOST_GIVEN" ]; then
      echo "two hosts given: $HOST and $arg" >&2
      exit 2
    fi
    HOST="$arg"
    HOST_GIVEN=yes
    ;;
  esac
done

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET="aarch64-unknown-linux-gnu"
BIN_DIR="${CARGO_TARGET_DIR:-$ROOT/target}/$TARGET/release/examples"
REMOTE_DIR="/tmp/bela-rs-probe-gpio"

# The run must outlive the probe by a margin: a probe that outlived it
# would measure a board with nothing running. The probe takes well under
# a second, so 30 against a 20-second bound leaves the with-run questions
# ~26 seconds of run to happen inside. Every command that *runs* something
# is bounded by `timeout`; the short ones carry only `ConnectTimeout`, so
# a board that answers and then stops answering will hang them.
RUN_SECONDS=30
PROBE_TIMEOUT=60
WITH_RUN_TIMEOUT=20

DAEMON_WAS_RUNNING=0
BOARD_PREPARED=no
# Set by the traps to the status that signal owes, because `$?` when a
# signal lands is whatever the last command left — 0 if it arrived
# between two `echo`s.
INTERRUPTED=0
# On a signal the handler runs, exits, and the EXIT trap runs it again;
# without this the second pass opens another restore ssh.
CLEANED=no

cleanup() {
  status=$?
  if [ "$INTERRUPTED" -ne 0 ] && [ "$status" -eq 0 ]; then
    status=$INTERRUPTED
  fi
  if [ "$CLEANED" = yes ]; then
    exit "$status"
  fi
  CLEANED=yes
  if [ "$BOARD_PREPARED" = yes ]; then
    # One connection: an unreachable board makes each of these cost a
    # full ConnectTimeout.
    #
    # The kill comes first. By executable, not by a pid written down:
    # libbela renames the process — `comm` becomes `sine:<pid>:<n>`,
    # measured — so `pkill -x sine` matches nothing and reports success,
    # and `pkill -f ./sine` matches the ssh running this script, while
    # `/proc/<pid>/exe` still points at the binary. A pid file needs
    # writing, removing when the number is reaped, and reading back
    # under the assumption that neither the write nor the removal was
    # interrupted; this needs none of that and cannot signal a recycled
    # number. The trailing glob matches a deleted binary, which
    # `readlink` marks.
    #
    # What it costs: a run that has not `execve`d yet is invisible here,
    # where a pid file would have named it. That window ends before
    # `Bela_startAudio` — so the run is not holding the audio device or
    # any pin inside it, and if `rm -rf` below removes the binary first,
    # the wrapper fails to start it at all. Both outcomes are the one
    # this is for.
    #
    # Graceful and bounded: `-9` skips libbela's teardown and orphans
    # its twenty-two exports, and `-INT` returns as soon as the signal
    # is queued, so the daemon below would start while the run still
    # held the device. The `timeout` wrapper is left alone — it exits
    # when its child does.
    undo="alive() { [ -n \"\$1\" ] && [ -d /proc/\$1 ] &&"
    undo="$undo ! grep -qE '^State:[[:space:]]*Z' /proc/\$1/status 2>/dev/null; }"
    undo="$undo; p="
    undo="$undo; for c in /proc/[0-9]*; do"
    undo="$undo case \"\$(readlink \$c/exe 2>/dev/null)\" in"
    undo="$undo $REMOTE_DIR/sine*) p=\${c#/proc/} ;;"
    undo="$undo esac; done"
    undo="$undo; if alive \$p; then kill -INT \$p 2>/dev/null; n=0"
    undo="$undo; while alive \$p && [ \$n -lt 6 ]; do sleep 1; n=\$((n+1)); done"
    undo="$undo; if alive \$p; then kill -9 \$p 2>/dev/null; fi; fi"
    # And any probe of ours still running, or the release below would
    # give back pins it then exports again.
    undo="$undo; for c in /proc/[0-9]*; do"
    undo="$undo case \"\$(readlink \$c/exe 2>/dev/null)\" in"
    undo="$undo $REMOTE_DIR/gpio_probe*) kill -9 \${c#/proc/} 2>/dev/null ;;"
    undo="$undo esac; done"
    # Then the pins. Its stderr onto stdout, because the ssh below
    # discards remote stderr and a refusal written there would go
    # nowhere. Guarded on the binary: BOARD_PREPARED is set before it
    # is copied.
    undo="$undo; if [ -x $REMOTE_DIR/gpio_probe ]; then"
    undo="$undo out=\$(timeout -s INT -k 5 15 $REMOTE_DIR/gpio_probe --release 2>&1)"
    undo="$undo || { echo 'WARNING: pins were NOT released:'; echo \"\$out\"; }; fi"
    undo="$undo; rm -rf $REMOTE_DIR"
    if [ "$DAEMON_WAS_RUNNING" -eq 1 ]; then
      undo="$undo; systemctl start bela_daemon || echo 'WARNING: bela_daemon did not start'"
    fi
    # Exporting a pin that is already exported fails and, worse, the
# unexport after it would remove an export the reader did not make — so
# the check looks before it claims. A probe killed between a change and
# its restore leaves it, and an
    # unexport keeps a pin's level and direction alike. Printed on every
    # exit that got as far as touching the board, which is where an
    # interrupt can have left something — and before the tidy-up, whose
    # last act is restarting `bela_daemon`: the snippet exports and
    # unexports pins, and would take them from whatever the daemon
    # started.
    leftovers
    # shellcheck disable=SC2029 # the remote paths are meant to expand here
    ssh -o ConnectTimeout=10 "$HOST" "$undo" 2>/dev/null ||
      echo "WARNING: could not reach $HOST to restore it; nothing above says what ran" >&2
  fi
  # A caught signal in POSIX sh runs the handler and then *resumes*, so
  # without this a Ctrl-C during pass 1 would tidy up and walk into
  # pass 2 with the directory deleted.
  exit "$status"
}

leftovers() {
  echo
  echo "If it was killed part way, look at what it had changed. With nothing"
  echo "running:"
  echo "  ssh $HOST 'grep -o \"\\[[a-z0-9-]*\\]\" /sys/class/leds/beaglebone:green:usr*/trigger'"
  echo "  ssh $HOST 'for p in 584 637; do d=/sys/class/gpio/gpio\$p;"
  echo "    if [ -d \$d ]; then cat \$d/direction \$d/value;"
  echo "    else echo \$p > /sys/class/gpio/export; cat \$d/direction \$d/value;"
  echo "      echo \$p > /sys/class/gpio/unexport; fi; done'"
  echo "The direction to put back is in the transcript above, on the line"
  echo "reading \"its direction before any of this\": an unexport keeps a"
  echo "direction, and only a pass that reached its end restores one."
}

# 255 is ssh's own and says nothing about whether the probe ran or what
# it left; scripts/probe-fft.sh keeps the same distinction.
pass_failed() {
  if [ "$2" -eq 255 ]; then
    echo "$1's ssh failed (255): a transport failure. Nothing above says what" >&2
    echo "ran, and the run may still be up on the board." >&2
  else
    echo "$1 did not finish (exit $2); the transcript above says where it" >&2
    echo "stopped." >&2
  fi
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
# business touching a board.
trap cleanup EXIT
trap 'INTERRUPTED=130; cleanup' INT
trap 'INTERRUPTED=143; cleanup' TERM

echo "Preparing $HOST..."
if ssh -o ConnectTimeout=10 "$HOST" "systemctl is-active --quiet bela_daemon" 2>/dev/null; then
  DAEMON_WAS_RUNNING=1
fi
# The directory goes before the flag is armed: `cleanup`'s ssh is
# allowed to fail, so an earlier run can have left the binaries behind,
# and the handler's exe scan would find one of those rather than
# nothing. `systemctl stop` can take seconds, which is long enough for
# an interrupt to land inside the guarded window.
# shellcheck disable=SC2029 # the remote path is meant to expand here
ssh -o ConnectTimeout=10 "$HOST" "rm -rf $REMOTE_DIR"
# Armed before the stop, as the sibling scripts do: an interrupt during
# that call can leave the daemon stopped.
BOARD_PREPARED=yes
# shellcheck disable=SC2029
ssh -o ConnectTimeout=10 "$HOST" "systemctl stop bela_daemon && mkdir -p $REMOTE_DIR"
for binary in gpio_probe sine; do
  scp -q -o ConnectTimeout=10 "$BIN_DIR/$binary" "$HOST:$REMOTE_DIR/$binary"
done
# shellcheck disable=SC2029 # the remote path is meant to expand here
ssh -o ConnectTimeout=10 "$HOST" "chmod +x $REMOTE_DIR/gpio_probe $REMOTE_DIR/sine"

# Before the questions, not only after them: question 8 answers by
# leaving gpio585 exported, and it can only answer where the pin was
# free to begin with. Where this declines a run is up, and the probe's
# own guard names the pin a moment later.
echo
echo "-- giving back anything an earlier invocation left --"
pre_release=0
# shellcheck disable=SC2029
ssh -o ConnectTimeout=10 "$HOST" "cd $REMOTE_DIR &&
  timeout -s INT -k 5 15 ./gpio_probe --release" || pre_release=$?
if [ "$pre_release" -ne 0 ]; then
  echo "That release did not finish (exit $pre_release); its message says why." >&2
fi

echo
echo "=============================================================="
echo "Pass 1: the probe alone"
echo "=============================================================="
# Not `status=$?` on the next line: `set -e` would end the script on a
# failing ssh before the assignment ran.
alone_status=0
# shellcheck disable=SC2029
ssh -o ConnectTimeout=10 "$HOST" "
  cd $REMOTE_DIR || { echo 'the remote directory is gone'; exit 1; }
  timeout -s INT -k 5 $PROBE_TIMEOUT ./gpio_probe
  probe_status=\$?
  echo
  echo '-- question 8: what is claimed now the probe has exited --'
  ls /sys/class/gpio | grep -vE 'gpiochip|^export\$|^unexport\$' | tr '\n' ' '
  echo
  # Question 8 leaves one pin exported on purpose and the listing above
  # is its answer. Give it back, so pass 2 starts from the resting
  # state; the probe declines this where a run is up.
  #
  # Its status counts, unlike pass 2's: a gpio585 still here is one
  # pass 2 would report as libbela's.
  release_status=0
  timeout -s INT -k 5 15 ./gpio_probe --release || release_status=\$?
  if [ \$probe_status -ne 0 ]; then exit \$probe_status; fi
  exit \$release_status
" || alone_status=$?

if [ "$alone_status" -ne 0 ]; then
  echo
  pass_failed "Pass 1" "$alone_status"
  echo "Pass 2 is not run: a pass 1 that stopped part way can leave a pin of" >&2
  echo "its own exported, and pass 2 would report it as one libbela is" >&2
  echo "holding — the distinction its answers turn on." >&2
  exit 1
fi

echo
echo "=============================================================="
echo "Pass 2: the probe beside a run${DESTRUCTIVE:+ (destructive)}"
echo "=============================================================="
with_run_status=0
# shellcheck disable=SC2029
ssh -o ConnectTimeout=10 "$HOST" "
  cd $REMOTE_DIR || { echo 'the remote directory is gone'; exit 1; }
  timeout -s INT -k 5 $RUN_SECONDS ./sine > sine.log 2>&1 &
  sine_pid=\$!
  # Nothing is written down for the handler: it finds the run by its
  # executable, which needs no file to be correct. This pid is this
  # shell's own business: the wrapper it waits on.
  sleep 4
  # Through /proc rather than a signal-0: under a shell which reaps only
  # at wait, a run that died a second ago is still a zombie a signal-0
  # succeeds on.
  alive() { [ -n \"\$1\" ] && [ -d /proc/\$1 ] &&
    ! grep -qE '^State:[[:space:]]*Z' /proc/\$1/status 2>/dev/null; }
  if ! alive \$sine_pid; then
    echo 'sine did not stay up; its output was:'
    cat sine.log
    exit 4
  fi
  timeout -s INT -k 5 $WITH_RUN_TIMEOUT ./gpio_probe --with-run $DESTRUCTIVE
  probe_status=\$?
  echo
  echo '-- what the run did while that happened --'
  if alive \$sine_pid; then
    echo '   still up at the moment the probe finished'
  else
    echo '   ALREADY GONE before the probe finished'
  fi
  # The status, not just liveness: a run that aborts a second after the
  # probe touched its pin is still alive for the check above. 124 is
  # timeout ending it, which is the undisturbed end here.
  wait \$sine_pid
  sine_status=\$?
  case \$sine_status in
  124) echo '   ended at 124: its own timeout, the undisturbed end' ;;
  *) echo '   ended at' \$sine_status '- NOT the undisturbed end' ;;
  esac
  echo '-- the run has now ended; its last lines --'
  tail -5 sine.log
  echo
  echo '-- question 13: what is claimed after both processes exited --'
  ls /sys/class/gpio | grep -vE 'gpiochip|^export\$|^unexport\$' | tr '\n' ' '
  echo
  # After the listing, so question 13 answers before anything is
  # tidied, and after the wait, so the release is not looking at
  # libbela's pins.
  timeout -s INT -k 5 15 ./gpio_probe --release || true
  exit \$probe_status
" || with_run_status=$?

echo
if [ "$with_run_status" -ne 0 ]; then
  pass_failed "Pass 2" "$with_run_status"
  echo "4 means no run was there to ask beside." >&2
  exit 1
fi
echo "The board answered. Record the findings in docs/board-facts.md."
echo
echo "If a pin is still exported above that was not before, this probe"
echo "left it there: that is question 13's answer and not a tidy-up the"
echo "script forgot."
