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
# This leaves the board's GPIO in an arbitrary state, and then reboots
# it. Exports, directions, levels and LED triggers all survive an
# unexport, and a `kill -9` runs none of the probe's own restore code —
# so no amount of restoring here could be complete, while a reboot is,
# and costs no code. The board is away for about forty seconds at the
# end of every run, including a Ctrl-C.
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
# a second, and the wait for it to claim a pin is bounded at eight, so
# 8 + 20 + 5 is what has to fit inside 40. Every command that *runs* something
# is bounded by `timeout`; the short ones carry only `ConnectTimeout`, so
# a board that answers and then stops answering will hang them.
RUN_SECONDS=40
PROBE_TIMEOUT=60
WITH_RUN_TIMEOUT=20

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
    # A reboot, not a tidy-up. Everything this probe changes — the
    # exports, the directions, the levels, the LED triggers — survives an
    # unexport, and a `kill -9` runs none of the probe's own restore
    # code, so no amount of it can put the board back. A reboot puts all
    # of it back, and it also ends any run still holding the audio
    # device, which is what the kill ladder here used to be for.
    #
    # What it costs: about forty seconds, and the board is gone for them.
    # This script already stops `bela_daemon` and runs two audio programs
    # back to back, so it is not something to run beside other work; and
    # a board left with an arbitrary GPIO state is worse than a board
    # that is briefly away. `bela_daemon` comes back on its own if it is
    # enabled, which is why nothing here records whether it was running.
    #
    # `rm -rf` first, in the same call, because `/tmp` is not guaranteed
    # to be a tmpfs on every image.
    echo "Rebooting $HOST: this probe leaves its GPIO in an arbitrary state." >&2
    # shellcheck disable=SC2029 # the remote path is meant to expand here
    # `--no-block`, so systemctl queues the job and returns instead of
    # taking sshd down under the connection and handing back 255 on
    # every successful run.
    # Its stderr kept and its status not read as "unreachable": the
    # remote command's own status comes back here too, so a board that
    # answered and refused the reboot — inhibited, unit unavailable —
    # would otherwise be reported as unreachable with the reason thrown
    # away, on the one step whose failure leaves the GPIO as this left
    # it.
    # shellcheck disable=SC2029 # the remote path is meant to expand here
    if ! why=$(ssh -o ConnectTimeout=10 "$HOST" \
      "rm -rf $REMOTE_DIR; systemctl --no-block reboot" 2>&1); then
      echo "WARNING: $HOST was not rebooted, so its GPIO is as this left it and" >&2
      echo "bela_daemon is still stopped — the reboot is what starts it again." >&2
      echo "$why" >&2
      # And into the exit status, or `probe-gpio.sh && next-step` walks
      # onto that board. 7 rather than 1: the questions above were
      # answered, and what failed was the step after them.
      if [ "$status" -eq 0 ]; then
        status=7
      fi
    fi
  fi
  # A caught signal in POSIX sh runs the handler and then *resumes*, so
  # without this a Ctrl-C during pass 1 would tidy up and walk into
  # pass 2 with the directory deleted.
  exit "$status"
}

# 255 is ssh's own and says nothing about whether the probe ran or what
# it left; scripts/probe-fft.sh keeps the same distinction.
pass_failed() {
  if [ "$2" -eq 5 ]; then
    # The remote block exits 5 only where the probe returned 0, so the
    # transcript above it stands and what failed came after the answers.
    echo "$1 asked its questions; what follows them did not finish. Its own" >&2
    echo "lines above say which." >&2
  elif [ "$2" -eq 255 ]; then
    echo "$1's ssh failed (255): a transport failure. Nothing above says what" >&2
    echo "ran." >&2
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
# A fresh directory, so a failed reboot from an earlier run cannot leave
# this one running its binaries.
# shellcheck disable=SC2029 # the remote path is meant to expand here
ssh -o ConnectTimeout=10 "$HOST" "rm -rf $REMOTE_DIR"
# Armed before the stop, as the sibling scripts do: an interrupt during
# that call can leave the daemon stopped.
BOARD_PREPARED=yes
# shellcheck disable=SC2029
# `||` and not `&&`: the four sibling scripts use `;` here and carry on,
# and a unit that is masked or not loaded at all exits 5 — which `&&`
# turned into `set -e` ending the probe. Not swallowed either: a daemon
# that would not stop holds the audio device, and pass 2 would report
# that as no run being there to ask beside.
# shellcheck disable=SC2029
ssh -o ConnectTimeout=10 "$HOST" \
  "systemctl stop bela_daemon || echo 'WARNING: bela_daemon would not stop'
   mkdir -p $REMOTE_DIR"
for binary in gpio_probe sine; do
  scp -q -o ConnectTimeout=10 "$BIN_DIR/$binary" "$HOST:$REMOTE_DIR/$binary"
done
# shellcheck disable=SC2029 # the remote path is meant to expand here
ssh -o ConnectTimeout=10 "$HOST" "chmod +x $REMOTE_DIR/gpio_probe $REMOTE_DIR/sine"

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
  # Only where the probe reached it. A pass that refused at its entry
  # guard never asked question 8, and this listing under that heading
  # would be a run's twenty-two pins copied into docs/board-facts.md as
  # the answer to a question nobody put. The refusal is on stderr, which
  # interleaves separately from this.
  if [ \$probe_status -eq 0 ]; then
    echo '-- question 8: what is claimed now the probe has exited --'
    claimed=\$(ls /sys/class/gpio) || claimed='<unreadable>'
    echo \"\$claimed\" | grep -vE 'gpiochip|^export\$|^unexport\$' | tr '\n' ' '
    echo
  fi
  # Question 8 leaves gpio585 exported on purpose and the listing above
  # is its answer. Pass 2 has to start without it, or it reports the
  # probe's own pin as one libbela is holding — the distinction its
  # answers turn on. Nothing else needs giving back: the reboot at the
  # end covers the rest.
  #
  # Only where the probe reached question 8. A pass that refused at its
  # entry guard did so because something else is holding pins, and this
  # would then take gpio585 from that run — the act the guard exists to
  # prevent.
  #
  # The write's status, not whether gpio585 is gone afterwards: an
  # existence test cannot tell a pin given back from a pin that was
  # never this number, and this 585 is tied by hand to the probe's
  # BANK0 + 46. Writing an
  # un-exported number to the unexport attribute fails, measured.
  if [ \$probe_status -eq 0 ]; then
    # Its stderr kept: the write being rejected and the attribute not
    # opening at all both land here, and the shell's own message is what
    # tells them apart.
    if ! echo 585 > /sys/class/gpio/unexport; then
      echo 'gpio585 could not be given back. If the write was rejected, question 8'
      echo 'left some other pin and the 585 here has come apart from the probe;'
      echo 'the line above says which.'
      exit 5
    fi
  fi
  exit \$probe_status
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
  # This shell's own business: the wrapper it waits on. Nothing is
  # written down for the handler, which reboots rather than killing.
  #
  # Wait for the run to have claimed a pin, not for a fixed four
  # seconds: \$sine_pid is the timeout wrapper, so its being alive says
  # nothing about whether PRU::initialise has exported the digital
  # channels. A start that ran long — cold cache, or the daemon slow to
  # let go of the audio device — had the probe refuse as though nothing
  # were rendering, which is a start-up race reported as a settings
  # problem, and it costs the reboot. Eight seconds at most, and it
  # usually returns in two or three. (No quotes in here: this comment is
  # inside the double-quoted ssh string.)
  for _ in 1 2 3 4 5 6 7 8; do
    [ -e /sys/class/gpio/gpio637 ] && break
    sleep 1
  done
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
  # Only where the probe reached it, as pass 1 does with question 8: a
  # pass that refused at its entry guard, or was killed part way, never
  # asked question 13, and this listing under that heading is a board at
  # rest — or the probe's own leftover — copied into the record as the
  # answer to a question nobody put.
  if [ \$probe_status -eq 0 ]; then
    echo '-- question 13: what is claimed after both processes exited --'
    claimed=\$(ls /sys/class/gpio) || claimed='<unreadable>'
    echo \"\$claimed\" | grep -vE 'gpiochip|^export\$|^unexport\$' | tr '\n' ' '
    echo
  fi
  exit \$probe_status
" || with_run_status=$?

echo
if [ "$with_run_status" -ne 0 ]; then
  pass_failed "Pass 2" "$with_run_status"
  if [ "$with_run_status" -eq 4 ]; then
    echo "No run was there to ask beside; sine's output is above." >&2
  fi
  exit 1
fi
echo "The board answered. Record the findings in docs/board-facts.md."
echo
echo "If a pin is still exported above that was not before, this probe"
echo "left it there: that is question 13's answer and not a tidy-up the"
echo "script forgot."
