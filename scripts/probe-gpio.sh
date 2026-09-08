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
# `--destructive` adds one question to the second pass — what
# unexporting one of libbela's pins does to the run holding it — and
# widens another, letting the probe write a digital channel whose
# direction reads `out`. Both contend with a live run, which is why
# they are opt-in.
#
# What is put back: `bela_daemon`, the remote directory, the run, and
# the two pins the probe can leave exported — `gpio_probe --release`,
# called after each pass and from the handler. It does not ask which
# pins this run actually took, and it declines outright where a pin a
# run would hold is claimed; the probe's own documentation says why
# both, and what they cost. What is not put back is an LED trigger, if
# the probe was killed between setting one and restoring it, or a pin
# left where the release declined — the handler says so in both
# cases.
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
# Set by the INT/TERM traps, because `$?` at the moment a signal lands
# is whatever the last command left — `0` if it arrived between two
# `echo`s. Without this an interrupted run exits `0` and suppresses its
# own "an LED trigger may be left" advice, which is written for exactly
# that run.
INTERRUPTED=no
# Whether the probe has been started on the board at all. The advice
# about LED triggers is about question 6, so before that there is
# nothing to have left behind and telling an operator to go and check
# four files is noise.
PROBE_RAN=no
# Whether the handler has already run. On a signal it runs, exits, and
# the EXIT trap runs it again; without this the second pass opens
# another restore ssh — whose failure would report pins and a daemon
# the first pass had already put back — and repeats the advice below.
CLEANED=no

cleanup() {
  status=$?
  if [ "$INTERRUPTED" = yes ] && [ "$status" -eq 0 ]; then
    status=130
  fi
  if [ "$CLEANED" = yes ]; then
    exit "$status"
  fi
  CLEANED=yes
  if [ "$BOARD_PREPARED" = yes ]; then
    # One connection, because an unreachable board makes each of these
    # cost a full ConnectTimeout, and one WARNING naming everything
    # that may be left rather than three swallowed failures.
    #
    # The kill comes first, by pid — both pids — and it is graceful
    # *and* bounded. Both, because `run.pid` exists for the case where
    # the wrapper died without taking the run with it, and that is
    # exactly the case where `sine.pid` has been removed: aiming `-9`
    # straight at the run there would skip libbela's teardown and
    # orphan all twenty-two of its exports, which is what the rest of
    # this comment is about avoiding.
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
    undo="$undo; alive() { [ -n \"\$1\" ] && [ -d /proc/\$1 ]; }"
    undo="$undo; for t in \$p \$r; do kill -INT \$t 2>/dev/null; done"
    undo="$undo; n=0"
    undo="$undo; while { alive \$p || alive \$r; } && [ \$n -lt 6 ]"
    undo="$undo; do sleep 1; n=\$((n+1)); done"
    undo="$undo; for t in \$p \$r; do"
    undo="$undo if alive \$t; then kill -9 \$t 2>/dev/null; fi; done"
    # And give back every pin the probe can claim. After the kill
    # above, so no run is holding one. This does not ask which pins
    # this invocation actually took — see the probe's own notes on why
    # not, and what that costs.
    #
    # Its stdout is dropped, because on the ordinary path it is three
    # lines saying nothing was exported. Its stderr and its status are
    # not: the kill above escalates to `-9` when the graceful signal
    # did not take within six seconds, and a run killed that way never
    # runs libbela's teardown, so all twenty-two of its pins are still
    # exported and this declines. That is the case an operator has to
    # be told about, and it was the case this silenced.
    #
    # Onto *stdout*, and captured rather than streamed. The `ssh` below
    # discards remote stderr — inherited from probe-io.sh, where the
    # undo produces none worth keeping — so a refusal written there
    # goes nowhere, which is what happened to the first version of this
    # warning. Stdout is not discarded, so that is where it goes.
    #
    # Guarded on the binary, because `BOARD_PREPARED` is set before the
    # daemon is stopped and so before this is copied: without the guard
    # an interrupt in that window reports pins it never claimed, and so
    # does the second run of this handler, after `$REMOTE_DIR` is gone.
    undo="$undo; if [ -x $REMOTE_DIR/gpio_probe ]; then"
    undo="$undo out=\$(timeout -s INT -k 5 15 $REMOTE_DIR/gpio_probe --release 2>&1)"
    undo="$undo || { echo 'WARNING: pins were NOT released:'; echo \"\$out\"; }; fi"
    undo="$undo; rm -rf $REMOTE_DIR"
    if [ "$DAEMON_WAS_RUNNING" -eq 1 ]; then
      undo="$undo; systemctl start bela_daemon"
    fi
    # shellcheck disable=SC2029 # the remote paths are meant to expand here
    ssh -o ConnectTimeout=10 "$HOST" "$undo" 2>/dev/null ||
      echo "WARNING: could not restore $HOST — check for a leftover sine" \
        "process, an exported gpio584, gpio585 or gpio637," \
        "$REMOTE_DIR, and bela_daemon" >&2
  fi
  # Question 6 sets an LED trigger to `none` and puts it back a line
  # later, and nothing here can cover a probe killed in between — the
  # handler reaches the GPIO exports, the remote directory and the
  # daemon, not the triggers. So say it here, on the path an
  # interrupted run actually takes, rather than after the checks that
  # a failed or interrupted run never reaches.
  if [ "$status" -ne 0 ] && [ "$PROBE_RAN" = yes ]; then
    echo "If this run was interrupted, an LED trigger may be left at none." >&2
    echo "Check with:" >&2
    echo "  ssh $HOST 'grep -o \"\\[[a-z0-9-]*\\]\" /sys/class/leds/beaglebone:green:usr*/trigger'" >&2
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
trap cleanup EXIT
trap 'INTERRUPTED=yes; cleanup' INT TERM

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
# `run.pid` behind — and a handler acting on a stale pid would signal
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
PROBE_RAN=yes
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
  # Question 8 leaves one pin exported on purpose, and the listing
  # above is its answer. Give back everything the probe can claim, so
  # that pass 2 starts from the board's resting state. The probe
  # refuses this where a run is up, which is where two of the three
  # would be libbela's.
  release_status=0
  timeout -s INT -k 5 15 ./gpio_probe --release || release_status=\$?
  # The probe's own status first, then the tidy-up's. Letting the
  # second be overwritten by the first is how a failed release lets
  # pass 2 start with gpio585 still exported — which it would then
  # report as a pin libbela is holding, the distinction every answer in
  # that pass turns on.
  if [ \$probe_status -ne 0 ]; then exit \$probe_status; fi
  exit \$release_status
" || alone_status=$?

if [ "$alone_status" -ne 0 ]; then
  echo
  # 255 is ssh's own, and says nothing about whether the probe ran —
  # the distinction scripts/probe-fft.sh keeps for the same reason.
  if [ "$alone_status" -eq 255 ]; then
    echo "Pass 1's ssh failed (255): a transport failure, which says nothing" >&2
    echo "about whether the probe ran or what it left." >&2
  else
    echo "Pass 1 exited $alone_status: the probe could not ask." >&2
  fi
  echo "Pass 2 is not run either way: a pass 1 that stopped part way can" >&2
  echo "leave a pin of its own exported, and pass 2 would then report it as" >&2
  echo "one libbela is holding — the distinction its answers turn on. The" >&2
  echo "handler gives back whatever the probe was still holding." >&2
  exit 1
fi

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
  # ppid is the second field after the closing parenthesis — state,
  # then ppid — which is why the cut below takes -f2. Counting from the
  # start of the line instead would be shifted by any comm containing a
  # space, and libbela renames the run to one containing colons.
  for c in /proc/[0-9]*; do
    ppid=\$(sed 's/.*) //' \$c/stat 2>/dev/null | cut -d' ' -f2)
    if [ \"\$ppid\" = \"\$sine_pid\" ]; then
      basename \$c > run.pid
    fi
  done
  if ! alive \$sine_pid; then
    echo 'sine did not stay up; its output was:'
    cat sine.log
    # \$sine_pid is this shell's child and is about to be reaped, so
    # its number is about to be reusable. \$run.pid names the
    # grandchild, which this shell never reaps — and which can outlive
    # a wrapper that was killed rather than exiting — so it is dropped
    # only once the run is really gone. Nothing else can find it:
    # libbela renames the process.
    rm -f sine.pid
    r=\$(cat run.pid 2>/dev/null)
    if [ -z \"\$r\" ] || [ ! -d /proc/\$r ]; then rm -f run.pid; fi
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
  # After the listing, so question 13 answers before anything is
  # tidied, and after the wait, so the run is gone and the release is
  # not looking at libbela's pins.
  release_status=0
  timeout -s INT -k 5 15 ./gpio_probe --release || release_status=\$?
  if [ \$probe_status -ne 0 ]; then exit \$probe_status; fi
  exit \$release_status
" || with_run_status=$?

echo
# Pass 1's own failure exits above, at the point where continuing
# would corrupt pass 2, so only pass 2's status can reach here.
if [ "$with_run_status" -ne 0 ]; then
  # The remote block returns the probe's status where that is non-zero
  # and the release's otherwise, so `2` here is the release declining
  # after every question was asked and printed. That transcript is a
  # measurement; what failed is the tidy-up.
  if [ "$with_run_status" -eq 255 ]; then
    echo "Pass 2's ssh failed (255): a transport failure, which says nothing" >&2
    echo "about whether the probe ran or what it left." >&2
  elif [ "$with_run_status" -eq 2 ]; then
    echo "Pass 2 asked its questions — the transcript above stands — but the" >&2
    echo "release that follows them declined, so a pin may be left exported." >&2
    echo "See its message above." >&2
  else
    echo "Pass 2 exited $with_run_status: it could not ask, rather than" >&2
    echo "getting a surprising answer." >&2
  fi
  exit 1
fi
echo "The board answered. Record the findings in docs/board-facts.md."
echo
echo "If a pin is still exported above that was not before, this probe"
echo "left it there: that is question 13's answer and not a tidy-up the"
echo "script forgot."
