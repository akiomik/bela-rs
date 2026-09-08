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
BOARD_PREPARED=no

# Modelled on scripts/probe-io.sh's `restore`, which had all of this
# right already.
cleanup() {
  status=$?
  if [ "$BOARD_PREPARED" = yes ]; then
    # One connection, because an unreachable board makes each of these
    # cost a full ConnectTimeout, and one WARNING naming everything
    # that may be left rather than three swallowed failures.
    #
    # The kill comes first, by pid, and it is graceful *and* bounded.
    #
    # By pid because nothing else works here. libbela renames the
    # process — `comm` becomes `sine:<pid>:<n>`, measured — so
    # `pkill -x sine` matches nothing and reports success, and
    # `pkill -f ./sine` matches the ssh command running this script
    # and kills the connection. Pass 2 writes the pid down for this.
    #
    # Graceful and bounded because neither half alone will do. `-INT`
    # returns as soon as the signal is queued, so the daemon below
    # would start while the run still held the audio device. `-9`
    # cannot be handled, so libbela's teardown never runs and the
    # twenty-two pins it exported stay exported — after which this
    # script's own pass 1 reads `gpio637` and aborts with "something
    # is rendering" when nothing is. So: ask, wait up to six seconds
    # (smoke-test.sh budgets five for the same teardown), then insist.
    # `timeout` relays the signal to the run it manages, measured.
    undo="p=\$(cat $REMOTE_DIR/sine.pid 2>/dev/null)"
    undo="$undo; r=\$(cat $REMOTE_DIR/run.pid 2>/dev/null)"
    undo="$undo; if [ -n \"\$p\" ]; then kill -INT \$p 2>/dev/null"
    undo="$undo; n=0"
    undo="$undo; while [ -d /proc/\$p ] && [ \$n -lt 6 ]"
    undo="$undo; do sleep 1; n=\$((n+1)); done"
    undo="$undo; kill -9 \$p 2>/dev/null; fi"
    undo="$undo; if [ -n \"\$r\" ]; then kill -9 \$r 2>/dev/null; fi"
    # Question 8 leaves one pin exported on purpose, and the probe
    # writes the number into `left.pin` at the moment it does. So the
    # file exists exactly when there is a pin of *ours* to give back:
    # not when the probe stopped before exporting one, and not when
    # some other run happens to be holding that pin. Pass 1 removes it
    # once it has cleared the pin itself, which is why nothing here
    # needs to know which pass is running.
    undo="$undo; l=\$(cat $REMOTE_DIR/left.pin 2>/dev/null)"
    undo="$undo; if [ -n \"\$l\" ]; then echo \$l > /sys/class/gpio/unexport 2>/dev/null; fi"
    # And the pins the with-run pass exported for itself, which it
    # records as it takes them and removes once it has given them back.
    undo="$undo; if [ -r $REMOTE_DIR/ours.pins ]; then"
    undo="$undo while read -r o; do echo \$o > /sys/class/gpio/unexport 2>/dev/null"
    undo="$undo; done < $REMOTE_DIR/ours.pins; fi"
    undo="$undo; rm -rf $REMOTE_DIR"
    if [ "$DAEMON_WAS_RUNNING" -eq 1 ]; then
      undo="$undo; systemctl start bela_daemon"
    fi
    # shellcheck disable=SC2029 # the remote paths are meant to expand here
    ssh -o ConnectTimeout=10 "$HOST" "$undo" 2>/dev/null ||
      echo "WARNING: could not restore $HOST — check for a leftover sine" \
        "process, a pin named by $REMOTE_DIR/left.pin still exported," \
        "$REMOTE_DIR, and bela_daemon" >&2
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
# Armed before the stop rather than after it, as the sibling scripts
# do: an interrupt or a dropped connection *during* this call can leave
# the daemon stopped, and a handler that had not been armed yet would
# exit silently without putting it back.
#
# The directory is removed rather than reused. `cleanup`'s single ssh
# is allowed to fail, so a previous run can have left `sine.pid` and
# `left.pin` behind — and a handler acting on a stale pid would signal
# whatever has since been given that number.
BOARD_PREPARED=yes
ssh -o ConnectTimeout=10 "$HOST" "systemctl stop bela_daemon; rm -rf $REMOTE_DIR; mkdir -p $REMOTE_DIR"
for binary in gpio_probe sine; do
  scp -q -o ConnectTimeout=10 "$BIN_DIR/$binary" "$HOST:$REMOTE_DIR/$binary"
done
# shellcheck disable=SC2029 # the remote path is meant to expand here
ssh -o ConnectTimeout=10 "$HOST" "chmod +x $REMOTE_DIR/gpio_probe $REMOTE_DIR/sine"

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
  left=\$(cat left.pin 2>/dev/null)
  if [ -n \"\$left\" ]; then
    echo \"(clearing gpio\$left, which question 8 left on purpose)\"
    echo \"\$left\" > /sys/class/gpio/unexport 2>/dev/null || true
    rm -f left.pin
  else
    echo '(the probe left no pin to clear)'
  fi
  exit \$probe_status
" || alone_status=$?

echo
echo "=============================================================="
echo "Pass 2: the probe beside a run${DESTRUCTIVE:+ (destructive)}"
echo "=============================================================="
# `sine` is backgrounded with its pid captured and written down, and
# everything that asks after it asks by that pid. Matching by name
# cannot work — libbela renames the process, measured in
# docs/board-facts.md — and matching by cmdline is worse, `./sine`
# appearing in the ssh command line of this script. `timeout -s INT`
# bounds the run even if a kill is missed, and the sleep is for
# `Bela_startAudio` to have claimed the pins before the probe looks.
with_run_status=0
# shellcheck disable=SC2029
ssh -o ConnectTimeout=10 "$HOST" "
  cd $REMOTE_DIR
  timeout -s INT -k 5 $RUN_SECONDS ./sine > sine.log 2>&1 &
  sine_pid=\$!
  # For the handler, which runs in another connection and cannot see
  # this shell's job table — and cannot find the run by name either.
  # Both pids: \$sine_pid is the timeout wrapper, and SIGKILL is not
  # forwarded, so an escalation aimed only at it would leave the run
  # orphaned and still holding the audio device. The run is the
  # wrapper's only child.
  echo \$sine_pid > sine.pid
  sleep 4
  # By pid, and through /proc rather than a signal-0: under a shell
  # which reaps only at wait, a run that died a second ago is still
  # a zombie that a signal-0 succeeds on. Not by name either: libbela
  # renames the process, so nothing here is called sine.
  alive() { [ -d /proc/\$1 ] && ! grep -qE '^State:[[:space:]]*Z' /proc/\$1/status 2>/dev/null; }
  # After the sleep, not before it: the glob is expanded once, and at
  # the moment the shell forks the wrapper that wrapper has still to
  # execve and fork, so the run has no /proc entry to find yet. The
  # ppid comes from /proc/<pid>/stat's fourth field counted from the
  # closing parenthesis, because a comm containing a space would shift
  # every field read positionally — and libbela renames the run.
  for c in /proc/[0-9]*; do
    ppid=\$(sed 's/.*) //' \$c/stat 2>/dev/null | cut -d' ' -f2)
    if [ \"\$ppid\" = \"\$sine_pid\" ]; then
      basename \$c > run.pid
    fi
  done
  if ! alive \$sine_pid; then
    echo 'sine did not stay up; its output was:'
    cat sine.log
    exit 3
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
  # probe touched its pin is still alive for the check above, and
  # throwing the status away is how that gets recorded as a run which
  # did not notice. 124 is timeout ending it, the undisturbed end here.
  wait \$sine_pid
  sine_status=\$?
  # The pids are dead and reaped now, so the files must stop naming
  # them: the handler runs on the normal exit path too, and a number
  # the kernel has since reissued is not one to signal.
  rm -f sine.pid run.pid
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
