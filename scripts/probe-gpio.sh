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
  *)
    # One positional, and only one. A second used to overwrite the
    # first silently, and `probe-gpio.sh destructive` — the dashes
    # forgotten — became `ssh destructive`. The probe rejects unknown
    # arguments a level down for the same reason: a mistyped
    # `--with_run` ran the pass that takes pins.
    if [ -n "${HOST_GIVEN:-}" ]; then
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
# The second pass needs the run to outlive the probe, and by a margin:
# a probe that outlived it would measure a board with nothing running
# and the script would report both `ALREADY GONE` and `the undisturbed
# end`, which are the two conclusions this must never confuse. The
# probe takes well under a second, so 30 against a 20-second bound
# leaves the with-run questions ~26 seconds of run to happen inside.
# The probe asks whether the run was still up when it finished and
# fails the pass where it was not, which is what actually rules the
# confusion out: `ALREADY GONE` cannot then be reported under a status
# this script treats as an answer.
RUN_SECONDS=30
PROBE_TIMEOUT=60
WITH_RUN_TIMEOUT=20

DAEMON_WAS_RUNNING=0
BOARD_PREPARED=no

# Modelled on scripts/probe-io.sh's `restore`, which had all of this
# right already.
# Set by the INT/TERM traps to the status that signal owes, because
# `$?` at the moment a signal lands is whatever the last command left —
# `0` if it arrived between two `echo`s. Without this an interrupted run
# exits `0` and suppresses its own advice about what may be left, which
# is written for exactly that run. Two values rather than one: 130 is
# 128+SIGINT, and a caller that sent SIGTERM and read 130 would be told
# the wrong signal.
INTERRUPTED=0
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
  if [ "$INTERRUPTED" -ne 0 ] && [ "$status" -eq 0 ]; then
    status=$INTERRUPTED
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
    # The wrapper, kept apart because it is the only pid here that
    # leads a process group. The fallback below may overwrite `p`.
    undo="$undo; w=\$p"
    undo="$undo; alive() { [ -n \"\$1\" ] && [ -d /proc/\$1 ] &&"
    undo="$undo ! grep -qE '^State:[[:space:]]*Z' /proc/\$1/status 2>/dev/null; }"
    # And a fallback for the windows no pid file covers. Pass 2
    # backgrounds the run and writes `sine.pid` on the next line, so an
    # interrupt in between leaves both files absent; and a wrapper
    # SIGKILLed before the four-second settle orphans the run with
    # `run.pid` never written, leaving `sine.pid` naming a dead process.
    # Either way the ladder below has nothing live to aim at, after
    # which `rm -rf` removes the directory and the daemon starts while
    # the run still holds the audio device. So the test is liveness,
    # not the files: found the same way the probe is, one loop down,
    # and it finds the run itself rather than its `timeout` wrapper,
    # whose exe is not here, which is why it feeds the graceful ladder
    # rather than replacing it.
    undo="$undo; if ! alive \$p && ! alive \$r; then"
    undo="$undo for c in /proc/[0-9]*; do"
    undo="$undo case \"\$(readlink \$c/exe 2>/dev/null)\" in"
    undo="$undo $REMOTE_DIR/sine*) p=\${c#/proc/} ;;"
    undo="$undo esac; done; fi"
    # The group before the pids. `timeout` leads a process group with
    # the run in it — measured: wrapper pid 18246 with pgid 18246, run
    # 18248 with pgid 18246 — so signalling the group reaches the run
    # even in the first seconds of pass 2, before `run.pid` has been
    # written. Only `$w`, and not the other two: a group signal to a
    # pid that leads no group is an `ESRCH` no-op today, but a pid
    # recycled onto a group leader would make it signal strangers. And
    # on `$w` being alive, like its SIGKILL twin below — a wrapper that
    # exited on its own leaves nothing in that group, and the run it
    # may have orphaned is what the exe scan above is for.
    undo="$undo; if alive \$w; then kill -INT -\$w 2>/dev/null; fi"
    undo="$undo; for t in \$p \$r; do [ -n \"\$t\" ] || continue"
    undo="$undo; kill -INT \$t 2>/dev/null; done"
    undo="$undo; n=0"
    undo="$undo; while { alive \$p || alive \$r; } && [ \$n -lt 6 ]"
    undo="$undo; do sleep 1; n=\$((n+1)); done"
    # On `$w` being alive, not on either of the other two. A pgid is a
    # pid and is not reissued while the group has members, so this was
    # not reachable — but saying so takes an argument about the kernel,
    # and `alive \$w` says it in three characters. The run is reached by
    # the per-pid loop below either way; nothing here forks.
    undo="$undo; if alive \$w; then kill -9 -\$w 2>/dev/null; fi"
    undo="$undo; for t in \$p \$r; do"
    undo="$undo if alive \$t; then kill -9 \$t 2>/dev/null; fi; done"
    # And any probe of ours still running, or the release below would
    # give back pins it then exports again. Matched on the executable
    # rather than the name: exact, needs no pid written down, and does
    # not care that this is one of the few binaries here libbela does
    # not rename (it creates no audio system). A deleted binary makes
    # readlink append a suffix, hence the trailing match.
    undo="$undo; for c in /proc/[0-9]*; do"
    undo="$undo case \"\$(readlink \$c/exe 2>/dev/null)\" in"
    undo="$undo $REMOTE_DIR/gpio_probe*) kill -9 \${c#/proc/} 2>/dev/null ;;"
    undo="$undo esac; done"
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
    # And the level, for the same reason and on the same path. Question
    # 4 writes gpio584 high and writes it back two lines later, and an
    # unexport keeps whatever the line holds — measured, in
    # docs/board-facts.md — so the release this handler ran does not
    # undo it. Read through a fresh export, with nothing running: an
    # unexport while a run holds the pin would take it from the run.
    echo "The same window can leave gpio584 driven high. With nothing running," >&2
    echo "read and clear it with:" >&2
    echo "  ssh $HOST 'echo 584 > /sys/class/gpio/export;" >&2
    echo "    cat /sys/class/gpio/gpio584/value;" >&2
    echo "    echo 0 > /sys/class/gpio/gpio584/value;" >&2
    echo "    echo 584 > /sys/class/gpio/unexport'" >&2
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
trap 'INTERRUPTED=130; cleanup' INT
trap 'INTERRUPTED=143; cleanup' TERM

echo "Preparing $HOST..."
if ssh -o ConnectTimeout=10 "$HOST" "systemctl is-active --quiet bela_daemon" 2>/dev/null; then
  DAEMON_WAS_RUNNING=1
fi
# Armed before the stop rather than after it, as the sibling scripts
# do: an interrupt or a dropped connection *during* this call can leave
# the daemon stopped, and a handler that had not been armed yet would
# exit silently without putting it back.
#
# The directory is removed rather than reused, and *before* the flag is
# armed rather than in the same call. `cleanup`'s single ssh is allowed
# to fail, so a previous run can have left `sine.pid` and `run.pid`
# behind — and a handler acting on a stale pid would signal whatever has
# since been given that number. Inside the guarded window that is
# reachable: `systemctl stop` can take seconds while the daemon holds
# the audio device, and an interrupt in there would arm the ladder
# against the files this call had not reached yet.
# shellcheck disable=SC2029 # the remote path is meant to expand here
ssh -o ConnectTimeout=10 "$HOST" "rm -rf $REMOTE_DIR"
BOARD_PREPARED=yes
# shellcheck disable=SC2029
ssh -o ConnectTimeout=10 "$HOST" "systemctl stop bela_daemon; mkdir -p $REMOTE_DIR"
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
# Before the questions, not only after them. Question 8 answers by
# leaving gpio585 exported, and it can only answer where the pin was
# free to begin with — so a gpio585 left by an earlier invocation makes
# it report the question as unanswered while the listing pass 1 prints
# is byte-for-byte the one a run that answered produces. Its status is
# kept: the probe declines with 3 for two states, and only one of them
# is the one pass 1's guard goes on to explain — see below.
#
# In its own connection, and before PROBE_RAN is armed below, because it
# can take fifteen seconds: an interrupt inside it would otherwise send
# an operator to check four LED triggers that question 6 had not reached.
echo "-- giving back anything an earlier invocation left --"
pre_release=0
# shellcheck disable=SC2029
ssh -o ConnectTimeout=10 "$HOST" "cd $REMOTE_DIR &&
  timeout -s INT -k 5 15 ./gpio_probe --release" || pre_release=$?
# Not swallowed. The probe declines with 3 for two different states —
# a run is up, or a pin refused to unexport — and only the first is the
# one pass 1's guard goes on to explain. On the second, the advice
# below and in the probe, that `--release` gives a claimed pin back, is
# advice this release has just disproved.
if [ "$pre_release" -ne 0 ]; then
  echo "The release above did not finish (exit $pre_release); its message says" >&2
  echo "which state it found. Where it names a pin that would not unexport, the" >&2
  echo "advice that follows about --release giving a pin back does not apply to" >&2
  echo "that pin: the same call has already refused it." >&2
fi
echo

# Question 6 touches the LED triggers, and it is in this pass only. The
# window this over-covers is what is left of the one above: the connect,
# and the run up to question 6. Nothing here can see where the probe got
# to, and the advice is written as a conditional for that reason.
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
  # The probe's own status first, and the tidy-up's wherever the probe
  # returned 0 — which is the case this ordering is for. Discarding the
  # release's status there is how a failed release lets pass 2 start
  # with gpio585 still exported, which it would then report as a pin
  # libbela is holding, the distinction every answer in that pass turns
  # on. Under a probe that also failed the release's status is dropped,
  # and that is fine: its message is in the transcript, the pass after
  # this one does not run, and the handler releases again.
  # 5, not 124: the release below is wrapped in timeout too, so
  # letting the probe own 124 through would leave the two
  # indistinguishable, which is what 3 was before it was split out.
  # 137 as well as 124: measured on the board, GNU timeout 9.1 returns
  # 124 where its signal took and 137 where the -k SIGKILL had to
  # finish the job. Only the second can leave a pin claimed, but both
  # mean the same thing to a reader of the transcript.
  if [ \$probe_status -eq 124 ] || [ \$probe_status -eq 137 ]; then exit 5; fi
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
  elif [ "$alone_status" -eq 5 ]; then
    echo "Pass 1's probe hit its own timeout part way through: the transcript" >&2
    echo "above stops wherever it stopped, and is not a complete measurement." >&2
    echo "The tidy-up after it did run." >&2
  elif [ "$alone_status" -eq 6 ]; then
    # Named by the LEFT CHANGED line rather than here, for the reason
    # pass 2's branch gives: the probe has more than one way to reach 6
    # and a list here falls behind them.
    echo "Pass 1 could not put something back: see LEFT CHANGED above, which" >&2
    echo "names it and, where there is one, the command to put it back. Nothing" >&2
    echo "in this tree restores an LED trigger or a pin's level; --release" >&2
    echo "gives back an export and nothing else. A 'could not ask:' line above" >&2
    echo "it, if there is one, says the pass also stopped short." >&2
  elif [ "$alone_status" -eq 124 ] || [ "$alone_status" -eq 137 ] ||
    [ "$alone_status" -eq 3 ]; then
    # All three mean the probe itself returned 0 and only the release
    # after it went wrong, so question 6 put every trigger back — the
    # probe reports it otherwise. Clear the flag here as well as after a
    # clean pass, or the handler sends an operator to check four files
    # this run provably did not leave changed.
    PROBE_RAN=no
    if [ "$alone_status" -ne 3 ]; then
      echo "Pass 1's tidy-up hit its own timeout: the questions were asked and" >&2
      echo "the transcript above stands, but a pin may be left exported." >&2
      if [ "$alone_status" -eq 137 ]; then
        echo "It did not stop on the signal either and was killed, so it got no" >&2
        echo "further than wherever it was." >&2
      fi
    else
      echo "Pass 1 asked its questions — the transcript above stands — but the" >&2
      echo "release that follows them declined, so a pin may be left exported." >&2
      echo "See its message above." >&2
    fi
  elif [ "$alone_status" -eq 2 ]; then
    # A trigger left at `none` exits 6, never 2: the probe's own
    # precedence rule puts that first. So a 2 is provably a pass that
    # changed nothing, and the advice below would send an operator to
    # check four files this run did not touch.
    PROBE_RAN=no
    echo "Pass 1's probe declined to ask; see its message above. Nothing needs" >&2
    echo "a hand: a pin it had claimed is one --release gives back, and it" >&2
    echo "reports anything else as LEFT CHANGED and exits 6 rather than 2." >&2
  else
    # Not cleared here: an unexpected status is a probe that died
    # without saying so, and question 6 sets a trigger to `none` a line
    # before it restores it.
    echo "Pass 1 exited $alone_status: the probe could not ask." >&2
  fi
  echo "Pass 2 is not run either way: a pass 1 that stopped part way can" >&2
  echo "leave a pin of its own exported, and pass 2 would then report it as" >&2
  echo "one libbela is holding — the distinction its answers turn on. The" >&2
  echo "handler gives back whatever the probe was still holding." >&2
  exit 1
fi

# Pass 1 returned 0, so question 6 restored every trigger it changed —
# the probe fails the pass otherwise. Nothing after this point can
# leave one at `none`, so the advice below would send an operator to
# check four files this run cannot have touched.
PROBE_RAN=no

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
  alive() { [ -n \"\$1\" ] && [ -d /proc/\$1 ] &&
    ! grep -qE '^State:[[:space:]]*Z' /proc/\$1/status 2>/dev/null; }
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
    # Through the alive helper, like every other liveness test here: a
    # run that died a moment ago is a zombie this shell has not reaped,
    # and a bare /proc test would keep run.pid naming it, after which
    # the handler signals a number that may since have been reissued.
    # (No backticks or quotes in here: this is inside the double-quoted
    # ssh string.)
    if ! alive \$r; then rm -f run.pid; fi
    # 4, because 2 and 3 are the probe's: it could not ask, and its
    # release declined. Nothing here ran the probe at all.
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
  # 137 too; see pass 1.
  if [ \$probe_status -eq 124 ] || [ \$probe_status -eq 137 ]; then exit 5; fi
  if [ \$probe_status -ne 0 ]; then exit \$probe_status; fi
  exit \$release_status
" || with_run_status=$?

echo
# Pass 1's own failure exits above, at the point where continuing
# would corrupt pass 2, so only pass 2's status can reach here.
if [ "$with_run_status" -ne 0 ]; then
  # The probe exits 2 when it could not ask, 3 when only the release
  # declined and 6 when it asked everything and could not put a pin's
  # value back, so these are separable rather than guessed at: a 3 or a
  # 6 means the transcript above is a measurement and what failed came
  # after the answer.
  if [ "$with_run_status" -eq 255 ]; then
    echo "Pass 2's ssh failed (255): a transport failure, which says nothing" >&2
    echo "about whether the probe ran or what it left." >&2
  elif [ "$with_run_status" -eq 4 ]; then
    echo "Pass 2 found no run to ask beside: see sine's output above. No" >&2
    echo "question was put and nothing was tidied. If its wrapper died while" >&2
    echo "the run itself kept going, that branch keeps run.pid and the" >&2
    echo "handler will have signalled it — so something may have run after" >&2
    echo "all, and the exports above are the place to look." >&2
  elif [ "$with_run_status" -eq 6 ]; then
    # Not a list of the causes: there are four in the probe today, one
    # rewrite of this message has already fallen behind them, and the
    # LEFT CHANGED line names the one that happened anyway. What an
    # operator cannot read off that line is which of them the tidy-up
    # covers, so say only that.
    echo "Pass 2 could not put something back: see LEFT CHANGED above, which" >&2
    echo "names it and the state it is in. --release gives back an export and" >&2
    echo "nothing else — not a pin's level, not its direction — and the remote" >&2
    echo "block runs it a moment later either way. A 'could not ask:' line" >&2
    echo "above it, if there is one, says the pass also stopped short." >&2
  elif [ "$with_run_status" -eq 5 ]; then
    echo "Pass 2's probe hit its own timeout part way through: the transcript" >&2
    echo "above stops wherever it stopped, and is not a complete measurement." >&2
    echo "The tidy-up after it did run." >&2
  elif [ "$with_run_status" -eq 124 ] || [ "$with_run_status" -eq 137 ]; then
    echo "Pass 2's tidy-up hit its own timeout: the questions were asked and" >&2
    echo "the transcript above stands, but a pin may be left exported." >&2
    if [ "$with_run_status" -eq 137 ]; then
      echo "It did not stop on the signal either and was killed, so it got no" >&2
      echo "further than wherever it was." >&2
    fi
  elif [ "$with_run_status" -eq 3 ]; then
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
