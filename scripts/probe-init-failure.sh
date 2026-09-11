#!/bin/sh
# Measure what a failed `Bela_initAudio` leaves behind, by running
# `bela/examples/init_failure` on a board one probe per process and
# asking, from a fresh process each time, whether the board still works.
#
# Usage: scripts/probe-init-failure.sh [user@host] [probe...]
#   host defaults to root@bela.local
#   with no probe named, every one of them runs, least destructive first
#
# BELA_SYSROOT must point at a synced sysroot (scripts/sync-sysroot.sh);
# see docs/cross-compile.md.
#
# This is an experiment, not a check: it deliberately puts the board
# into the state that crashes, and there is no answer it is expecting.
# That is why it is separate from scripts/smoke-test.sh, which is a
# pass/fail gate and must not depend on how this goes. Its findings
# belong in docs/board-facts.md.
#
# Each probe is measured the same way:
#
#   oracle (must pass) -> probe -> oracle
#
# The oracle is a full audio cycle in a process of its own. The one in
# front establishes that the board was working before the probe ran, so
# that a failure afterwards can be attributed to the probe rather than
# to whatever came before it; the one behind is the measurement. Every
# probe therefore runs on a board that a successful audio program has
# just used, which is the condition #30 saw the crashes under.
#
# A board freshly out of a reboot is a different condition and this
# script cannot produce it, since its own preflight oracle warms the
# board up. To measure that, reboot and run one probe by name.
#
# `bela_daemon` is stopped for the duration and restarted afterwards.
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET="aarch64-unknown-linux-gnu"
BIN_DIR="${CARGO_TARGET_DIR:-$ROOT/target}/$TARGET/release/examples"
REMOTE_DIR="/tmp/bela-rs-probe"

# Least destructive first, so that a probe which wedges the board takes
# as few unmeasured ones with it as possible.
ALL_PROBES="abort abort-then-new abort-cleanup raw-cleanup abort-cleanup-then-new cycles init-cycles busy"

# The first argument is the host, unless it names a probe: running one
# probe against the default board should not need the host spelled out.
HOST=root@bela.local
if [ $# -gt 0 ]; then
  case " $ALL_PROBES " in
  *" $1 "*) ;;
  *)
    HOST="$1"
    shift
    ;;
  esac
fi
PROBES="${*:-$ALL_PROBES}"

# A cycle is about 0.7 s of startup, 1 s of rendering and a teardown, so
# these are ceilings for a probe that hangs rather than expected times.
ORACLE_TIMEOUT=25
PROBE_TIMEOUT=40
# How many audio systems `cycles` and `init-cycles` build in one
# process. An earlier board note put a bus error at four or five, which
# did not reproduce; overridable, because finding out whether there is
# a limit at all means pushing the number up.
CYCLE_COUNT="${CYCLE_COUNT:-5}"
# Derived, so that raising the count does not silently run into the
# ceiling: a cut-short run reports fewer cycles than were asked for,
# which is exactly what a crash looks like.
CYCLES_TIMEOUT=$((CYCLE_COUNT * 6 + 30))
# How long the holder keeps the audio device, how long after it the
# probe starts, and how long the probe then waits. The wait has to see
# the holder out — the four is a cycle's startup and teardown either
# side of the hold — because the oracle behind the probe would be
# refused by a holder still up (#167).
HOLD_SECONDS=6
HOLDER_HEAD_START=2
BUSY_WAIT_SECONDS=$((HOLD_SECONDS + 4 - HOLDER_HEAD_START))
BUSY_TIMEOUT=$((BUSY_WAIT_SECONDS + 30))
HOLDER_CEILING=$((HOLD_SECONDS + ORACLE_TIMEOUT))
# `remote` has no terminal, so Ctrl-C ends the local ssh and leaves the
# board-side run to whichever of these ceilings it was given, plus the
# five seconds probe-remote.sh allows for the kill after it.
LONGEST_LEFTOVER=0
for t in "$ORACLE_TIMEOUT" "$CYCLES_TIMEOUT" "$PROBE_TIMEOUT" \
  "$BUSY_TIMEOUT" "$HOLDER_CEILING"; do
  [ "$t" -le "$LONGEST_LEFTOVER" ] || LONGEST_LEFTOVER=$t
done
LONGEST_LEFTOVER=$((LONGEST_LEFTOVER + 5))
# How long to wait before asking again — whether the board is back, or
# whether the holder has reported.
RECOVERY_WAIT=2

daemon_was_active=no
# Whether the board has been touched yet. Until it has there is nothing
# to put back, and an early exit should not spend three connection
# timeouts finding that out.
board_prepared=no
LOG_DIR="$(mktemp -d)"
RESULTS="$LOG_DIR/results"
: > "$RESULTS"

# Leave the board as it was found, whether the run finished, a probe
# wedged it, or the operator interrupted. Installed straight after the
# directory it removes, so that the argument checks below cannot leak
# one by exiting before it.
restore() {
  status=$?
  if [ "$board_prepared" = yes ]; then
    # One connection, because an unreachable board makes each of these
    # cost a full ConnectTimeout.
    # Nothing here kills what an interrupt leaves running: a ceiling
    # ends it, and the preflight says so when a later run meets it.
    undo="rm -rf $REMOTE_DIR"
    if [ "$daemon_was_active" = yes ]; then
      undo="$undo; systemctl start bela_daemon"
    fi
    # shellcheck disable=SC2029 # the remote path is meant to expand here
    ssh -o ConnectTimeout=10 "$HOST" "$undo" 2>/dev/null ||
      echo "WARNING: could not restore $HOST — check for a leftover" \
        "init_failure process, $REMOTE_DIR, and bela_daemon" >&2
  fi
  rm -rf "$LOG_DIR"
  exit "$status"
}
trap 'restore' EXIT INT TERM

if [ -z "${BELA_SYSROOT:-}" ]; then
  echo "BELA_SYSROOT is not set; see docs/cross-compile.md" >&2
  exit 2
fi

for probe in $PROBES; do
  case " $ALL_PROBES " in
  *" $probe "*) ;;
  *)
    echo "unknown probe '$probe'; known: $ALL_PROBES" >&2
    exit 2
    ;;
  esac
done

# The image greets every ssh session with a login banner and a locale
# warning on stderr, which would bury the output being parsed. Held back
# and shown only when the command actually fails, so that a bad host or
# a key problem does not become a silent exit under `set -e`.
remote() {
  # shellcheck disable=SC2029 # callers build the command on this side on purpose
  if ! ssh -n -o ConnectTimeout=10 "$HOST" "$1" 2> "$LOG_DIR/ssh.err"; then
    sed 's/^/        /' "$LOG_DIR/ssh.err" >&2
    return 1
  fi
}

# Runs one probe process on the board and echoes what it printed,
# followed by the exit status the shell saw. A segfault shows up as 139
# and a bus error as 135, which is the difference this script exists to
# record.
run_probe() {
  timeout="$1"
  shift
  remote "sh $REMOTE_DIR/probe-remote.sh $timeout $*" || echo "probe-exit=ssh-failed"
}

# The last value a probe reported for one key. Keys are repeated when a
# step is reported before and after a call that might not return.
field() {
  sed -n "s/^init-failure: $2=//p" "$1" | tail -1
}

status_of() {
  sed -n 's/^probe-exit=//p' "$1" | tail -1
}

# Whether a run was turned away because something already holds the
# board. On the message rather than the code: `Bela_initAudio` returns
# -1 from nineteen places, and only this one says so.
refused() {
  grep -q "already running in another process" "$1"
}

# A run cut short by the clock is missing the same fields a crashed one
# is. Say which it was.
timed_out() {
  case "$(status_of "$1")" in
  124 | 137) echo " - TIMED OUT at the ceiling, not a crash" ;;
  *) ;;
  esac
}

# A full audio cycle in a fresh process: the only question is whether
# the board still gives one.
oracle() {
  run_probe "$ORACLE_TIMEOUT" render-check > "$LOG_DIR/oracle.log"
  case "$(field "$LOG_DIR/oracle.log" cycle)" in
  rendered-*) return 0 ;;
  *) return 1 ;;
  esac
}

# What an oracle run said, for the record — the block count when it
# worked, and how it failed when it did not.
oracle_detail() {
  cycle="$(field "$LOG_DIR/oracle.log" cycle)"
  echo "${cycle:-no-output} (exit $(status_of "$LOG_DIR/oracle.log"))"
}

record() {
  printf '%s\t%s\n' "$1" "$2" >> "$RESULTS"
}

wedged() {
  echo
  echo "The board no longer gives an audio system, and a second look a"
  echo "few seconds later did not find it back. The last line below is"
  echo "where it was noticed."
  advice="Reboot before probing again:"
  if refused "$LOG_DIR/oracle.log"; then
    echo
    echo "That last look was refused rather than broken: something holds"
    echo "the board. After the busy probe it is likely this run's own"
    echo "holder, which goes within $((HOLDER_CEILING + 5)) seconds of"
    echo "starting — $LONGEST_LEFTOVER for the longest-lived probe here."
    advice="Wait it out, and reboot only if it is still refused:"
  fi
  echo
  echo "$advice"
  echo
  echo "    ssh $HOST reboot"
  echo
  summarise
  exit 1
}

summarise() {
  echo "Results"
  echo "-------"
  while IFS="$(printf '\t')" read -r name detail; do
    printf '  %-24s %s\n' "$name" "$detail"
  done < "$RESULTS"
}

echo "Building the probe for $TARGET..."
cargo build -p bela --release --target "$TARGET" --example init_failure

if [ ! -x "$BIN_DIR/init_failure" ]; then
  echo "not built at $BIN_DIR/init_failure" >&2
  exit 2
fi

echo "Preparing $HOST..."
if ssh -o ConnectTimeout=10 "$HOST" "systemctl is-active --quiet bela_daemon" 2>/dev/null; then
  daemon_was_active=yes
fi
# From here on there is something to put back.
board_prepared=yes
remote "systemctl stop bela_daemon; mkdir -p $REMOTE_DIR"

# The remote half. Bounded, for two reasons: `remote` runs a probe in
# the foreground with no clock of its own, so one that does not end is
# one this script waits on forever; and a probe that hangs keeps the
# board for its whole life, so every later run is refused (measured,
# see docs/board-facts.md).
cat > "$LOG_DIR/probe-remote.sh" <<'REMOTE'
#!/bin/sh
# usage: probe-remote.sh <timeout-seconds> <probe-arguments...>
set -u
seconds="$1"
shift
cd "$(dirname "$0")"
chmod +x ./init_failure
# -k: a probe that misses the interrupt is killed outright rather
# than left holding the audio device.
timeout -s INT -k 5 "$seconds" ./init_failure "$@" 2>&1
echo "probe-exit=$?"
REMOTE
scp -q -o ConnectTimeout=10 "$BIN_DIR/init_failure" "$HOST:$REMOTE_DIR/init_failure"
scp -q -o ConnectTimeout=10 "$LOG_DIR/probe-remote.sh" "$HOST:$REMOTE_DIR/probe-remote.sh"

echo "Checking the board is working to begin with..."
if ! oracle; then
  echo "the board does not give an audio system before any probe has run:" >&2
  echo "  $(oracle_detail)" >&2
  if refused "$LOG_DIR/oracle.log"; then
    echo "  that is another process holding the board, not a broken one." >&2
    echo "  An interrupted run of this script leaves whatever it was" >&2
    echo "  running for up to $LONGEST_LEFTOVER seconds; otherwise look" >&2
    echo "  for a project or another operator's run." >&2
  fi
  sed 's/^/        /' "$LOG_DIR/oracle.log" >&2
  exit 1
fi
record "preflight" "$(oracle_detail)"
echo "  $(oracle_detail)"

# Makes sure the next probe starts from a board that works, and says so
# when it cannot. A second oracle after a wait, which asks whether the
# board comes back on its own.
require_healthy() {
  oracle && return 0
  echo "  board unhealthy before the probe; waiting and retrying"
  sleep "$RECOVERY_WAIT"
  oracle
}

for probe in $PROBES; do
  echo
  echo "Probe: $probe"
  if ! require_healthy; then
    record "$probe" "not run: board already unhealthy"
    wedged
  fi

  log="$LOG_DIR/$probe.log"
  case "$probe" in
  cycles)
    run_probe "$CYCLES_TIMEOUT" cycles "$CYCLE_COUNT" > "$log"
    completed=0
    index=1
    while [ "$index" -le "$CYCLE_COUNT" ]; do
      case "$(field "$log" "cycle-$index")" in
      rendered-*) completed=$index ;;
      *) break ;;
      esac
      index=$((index + 1))
    done
    detail="$completed/$CYCLE_COUNT cycles rendered (exit $(status_of "$log"))"
    detail="$detail$(timed_out "$log")"
    ;;
  init-cycles)
    run_probe "$CYCLES_TIMEOUT" init-cycles "$CYCLE_COUNT" > "$log"
    completed=0
    index=1
    while [ "$index" -le "$CYCLE_COUNT" ]; do
      case "$(field "$log" "init-cycle-$index")" in
      built-and-dropped) completed=$index ;;
      *) break ;;
      esac
      index=$((index + 1))
    done
    detail="$completed/$CYCLE_COUNT built and dropped (exit $(status_of "$log"))"
    detail="$detail$(timed_out "$log")"
    ;;
  raw-cleanup)
    # Reports on the C API rather than on `Bela::new`, so it has its own
    # keys. `cleanup-callback` is the one that says whether the user
    # callback ran before the crash, which is what tells a call that
    # cannot work from one this probe arranged badly.
    run_probe "$PROBE_TIMEOUT" raw-cleanup > "$log"
    detail="init=$(field "$log" raw-init) cleanup=$(field "$log" cleanup)"
    detail="$detail callback=$(field "$log" cleanup-callback)"
    detail="$detail (exit $(status_of "$log"))"
    ;;
  busy)
    # A holder keeps the audio device while the probe tries to take it
    # and is refused. The probe's second attempt measures nothing: the
    # first poisoned its own claim (#167). Nothing ends the holder but
    # its ceiling.
    remote "setsid sh $REMOTE_DIR/probe-remote.sh \
      $HOLDER_CEILING render-check $HOLD_SECONDS \
      > $REMOTE_DIR/holder.log 2>&1 & echo started" > /dev/null
    sleep "$HOLDER_HEAD_START"
    run_probe "$BUSY_TIMEOUT" busy-probe "$BUSY_WAIT_SECONDS" > "$log"
    # The holder reports after its teardown, which can be later than
    # the probe returns. A log that is missing rather than silent means
    # nothing was launched, and there is nothing to wait for.
    waited=0
    while remote "cat $REMOTE_DIR/holder.log" > "$LOG_DIR/holder.log" \
      2>/dev/null &&
      [ -z "$(field "$LOG_DIR/holder.log" cycle)" ] &&
      [ -z "$(status_of "$LOG_DIR/holder.log")" ] &&
      [ "$waited" -lt "$HOLDER_CEILING" ]; do
      sleep "$RECOVERY_WAIT"
      waited=$((waited + RECOVERY_WAIT))
    done
    # A holder that never reported has its reason in that log, and
    # nothing else would show it.
    case "$(field "$LOG_DIR/holder.log" cycle)$(status_of "$LOG_DIR/holder.log")" in
    "") sed 's/^/    holder: /' "$LOG_DIR/holder.log" >&2 ;;
    esac
    detail="first=$(field "$log" busy-first) second=$(field "$log" busy-second)"
    # Both halves: the refusal says somebody held the board, the
    # holder's cycle says it was this run's.
    if ! refused "$log"; then
      detail="$detail - CONTENTION NOT SHOWN: the probe was not refused"
    else
      case "$(field "$LOG_DIR/holder.log" cycle)" in
      rendered-* | up-but-silent) ;;
      *)
        detail="$detail - CONTENTION NOT SHOWN: the probe was refused, but"
        detail="$detail nothing here says the holder is what refused it"
        ;;
      esac
    fi
    detail="$detail (holder $(field "$LOG_DIR/holder.log" cycle), \
probe exit $(status_of "$log"))$(timed_out "$log")"
    ;;
  *)
    run_probe "$PROBE_TIMEOUT" "$probe" > "$log"
    detail="abort=$(field "$log" abort)"
    case "$probe" in
    *cleanup*) detail="$detail cleanup=$(field "$log" cleanup)" ;;
    esac
    case "$probe" in
    *then-new) detail="$detail second=$(field "$log" second)" ;;
    esac
    detail="$detail (exit $(status_of "$log"))"
    ;;
  esac

  sed 's/^/    /' "$log"
  echo "  -> $detail"

  # A holder that overran its hold is still on the board here, and
  # this reads that as damage rather than as held: #167.
  if oracle; then
    after="board still works: $(oracle_detail)"
  else
    after="BOARD BROKEN AFTERWARDS: $(oracle_detail)"
  fi
  echo "  -> $after"
  record "$probe" "$detail; $after"

  case "$after" in
  BOARD*)
    # Distinguish damage that a wait undoes from damage that outlives
    # one, which is the whole question.
    sleep "$RECOVERY_WAIT"
    if oracle; then
      record "$probe (after a wait)" "recovered: $(oracle_detail)"
      echo "  -> recovered on a second look"
    else
      record "$probe (after a wait)" "still broken: $(oracle_detail)"
      wedged
    fi
    ;;
  esac
done

echo
summarise
echo
echo "Nothing here passes or fails; record what it says in docs/board-facts.md."
