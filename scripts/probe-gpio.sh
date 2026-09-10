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
# would measure a board with nothing running. The probe takes well
# under a second, so 30 against a 20-second bound leaves the with-run
# questions ~26 seconds of run to happen inside. Every remote command is
# bounded; an unbounded one is how a script waits forever on a board
# that stopped answering.
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
    # The kill comes first, by pid, and it is graceful *and* bounded.
    # By pid because nothing else works: libbela renames the process —
    # `comm` becomes `sine:<pid>:<n>`, measured — so `pkill -x sine`
    # matches nothing and reports success, and `pkill -f ./sine` matches
    # the ssh command running this script. Graceful because `-9` skips
    # libbela's teardown and orphans its twenty-two exports; bounded
    # because `-INT` returns as soon as the signal is queued, and the
    # daemon below would start while the run still held the device.
    undo="p=\$(cat $REMOTE_DIR/sine.pid 2>/dev/null)"
    undo="$undo; r=\$(cat $REMOTE_DIR/run.pid 2>/dev/null)"
    # The wrapper, kept apart because it is the only pid here that
    # leads a process group.
    undo="$undo; w=\$p"
    undo="$undo; alive() { [ -n \"\$1\" ] && [ -d /proc/\$1 ] &&"
    undo="$undo ! grep -qE '^State:[[:space:]]*Z' /proc/\$1/status 2>/dev/null; }"
    # A fallback for the windows no pid file covers: pass 2 writes
    # `sine.pid` on the line after backgrounding the run, and a wrapper
    # killed before the settle orphans the run with `run.pid` never
    # written. So the test is liveness, not the files. Found by exe,
    # which finds the run rather than its `timeout` wrapper.
    undo="$undo; if ! alive \$p && ! alive \$r; then"
    undo="$undo for c in /proc/[0-9]*; do"
    undo="$undo case \"\$(readlink \$c/exe 2>/dev/null)\" in"
    undo="$undo $REMOTE_DIR/sine*) p=\${c#/proc/} ;;"
    undo="$undo esac; done; fi"
    # The group first: `timeout` leads one with the run in it —
    # measured, wrapper 18246 pgid 18246, run 18248 pgid 18246 — so it
    # reaches the run before `run.pid` exists. Only `$w`, and only
    # while alive: a group signal to a pid leading no group is an ESRCH
    # no-op, but a recycled pid would make it signal strangers.
    undo="$undo; if alive \$w; then kill -INT -\$w 2>/dev/null; fi"
    undo="$undo; for t in \$p \$r; do"
    undo="$undo if alive \$t; then kill -INT \$t 2>/dev/null; fi; done"
    undo="$undo; n=0"
    undo="$undo; while { alive \$p || alive \$r; } && [ \$n -lt 6 ]"
    undo="$undo; do sleep 1; n=\$((n+1)); done"
    undo="$undo; if alive \$w; then kill -9 -\$w 2>/dev/null; fi"
    undo="$undo; for t in \$p \$r; do"
    undo="$undo if alive \$t; then kill -9 \$t 2>/dev/null; fi; done"
    # And any probe of ours still running, or the release below would
    # give back pins it then exports again. Matched on the executable:
    # exact, and needs no pid written down.
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
  echo "What this probe can leave behind, and nothing here puts back:"
  echo "  - an LED trigger at none, if it was killed inside question 6"
  echo "  - gpio584 or gpio637 driven, if it was killed between a write"
  echo "    and the restore a line later"
  echo "Both survive an unexport. With nothing running, check and clear with:"
  echo "  ssh $HOST 'grep -o \"\\[[a-z0-9-]*\\]\" /sys/class/leds/beaglebone:green:usr*/trigger'"
  echo "  ssh $HOST 'for p in 584 637; do echo \$p > /sys/class/gpio/export;"
  echo "    cat /sys/class/gpio/gpio\$p/value; echo 0 > /sys/class/gpio/gpio\$p/value;"
  echo "    echo \$p > /sys/class/gpio/unexport; done'"
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
# allowed to fail, so an earlier run can have left `sine.pid` behind,
# and a handler acting on a stale pid would signal whatever has since
# been given that number. `systemctl stop` can take seconds, which is
# long enough for an interrupt to land inside the guarded window.
# shellcheck disable=SC2029 # the remote path is meant to expand here
ssh -o ConnectTimeout=10 "$HOST" "rm -rf $REMOTE_DIR"
# Armed before the stop, as the sibling scripts do: an interrupt during
# that call can leave the daemon stopped.
BOARD_PREPARED=yes
# shellcheck disable=SC2029
ssh -o ConnectTimeout=10 "$HOST" "systemctl stop bela_daemon; mkdir -p $REMOTE_DIR"
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
  cd $REMOTE_DIR
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
  echo "Pass 1 did not finish (exit $alone_status); the transcript above says" >&2
  echo "where it stopped. Pass 2 is not run: a pass 1 that stopped part way" >&2
  echo "can leave a pin of its own exported, and pass 2 would report it as one" >&2
  echo "libbela is holding — the distinction its answers turn on." >&2
  leftovers
  exit 1
fi

echo
echo "=============================================================="
echo "Pass 2: the probe beside a run${DESTRUCTIVE:+ (destructive)}"
echo "=============================================================="
with_run_status=0
# shellcheck disable=SC2029
ssh -o ConnectTimeout=10 "$HOST" "
  cd $REMOTE_DIR
  timeout -s INT -k 5 $RUN_SECONDS ./sine > sine.log 2>&1 &
  sine_pid=\$!
  # For the handler, which runs in another connection and cannot see
  # this shell's job table. Both pids: \$sine_pid is the timeout
  # wrapper, SIGKILL is not forwarded to its child, and an escalation
  # aimed only at the wrapper would orphan the run.
  echo \$sine_pid > sine.pid
  sleep 4
  # Through /proc rather than a signal-0: under a shell which reaps only
  # at wait, a run that died a second ago is still a zombie a signal-0
  # succeeds on.
  alive() { [ -n \"\$1\" ] && [ -d /proc/\$1 ] &&
    ! grep -qE '^State:[[:space:]]*Z' /proc/\$1/status 2>/dev/null; }
  # After the sleep: at the moment this shell forks the wrapper, that
  # wrapper has still to execve and fork, so the run has no /proc entry
  # yet. The ppid is the second field after the closing parenthesis, so
  # the cut takes -f2; counting from the start of the line would be
  # shifted by a comm containing spaces, and libbela renames the run to
  # one containing colons.
  for c in /proc/[0-9]*; do
    ppid=\$(sed 's/.*) //' \$c/stat 2>/dev/null | cut -d' ' -f2)
    if [ \"\$ppid\" = \"\$sine_pid\" ]; then basename \$c > run.pid; fi
  done
  if ! alive \$sine_pid; then
    echo 'sine did not stay up; its output was:'
    cat sine.log
    # \$sine_pid is about to be reaped and its number reusable. run.pid
    # names the grandchild, which this shell never reaps and which can
    # outlive a killed wrapper, so it is dropped only once that is gone.
    rm -f sine.pid
    r=\$(cat run.pid 2>/dev/null)
    if ! alive \$r; then rm -f run.pid; fi
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
  # The pids are reaped now, so the files must stop naming them.
  rm -f sine.pid run.pid
  case \$sine_status in
  124) echo '   ended at 124: its own timeout, the undisturbed end' ;;
  *) echo '   ended at' \$sine_status '- NOT the undisturbed end; see sine.log' ;;
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
  echo "Pass 2 did not finish (exit $with_run_status); the transcript above says" >&2
  echo "where it stopped. 4 means no run was there to ask beside." >&2
  leftovers
  exit 1
fi
echo "The board answered. Record the findings in docs/board-facts.md."
echo
echo "If a pin is still exported above that was not before, this probe"
echo "left it there: that is question 13's answer and not a tidy-up the"
echo "script forgot."
leftovers
