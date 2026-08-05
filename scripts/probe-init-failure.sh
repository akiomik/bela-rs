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

HOST="${1:-root@bela.local}"
[ $# -gt 0 ] && shift
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET="aarch64-unknown-linux-gnu"
BIN_DIR="${CARGO_TARGET_DIR:-$ROOT/target}/$TARGET/release/examples"
REMOTE_DIR="/tmp/bela-rs-probe"

# Least destructive first, so that a probe which wedges the board takes
# as few unmeasured ones with it as possible.
ALL_PROBES="abort abort-then-new abort-cleanup abort-cleanup-then-new cycles init-cycles busy"
PROBES="${*:-$ALL_PROBES}"

# A cycle is about 0.7 s of startup, 1 s of rendering and a teardown, so
# these are ceilings for a probe that hangs rather than expected times.
ORACLE_TIMEOUT=25
PROBE_TIMEOUT=40
CYCLES_TIMEOUT=90
# How many audio systems `cycles` and `init-cycles` build in one
# process. The board notes put the bus error at four or five;
# overridable, because finding out where it actually is means pushing
# the number up.
CYCLE_COUNT="${CYCLE_COUNT:-5}"
# How long the holder process keeps the audio device for the busy probe,
# and how long the probe waits before trying again. The wait has to
# outlast the holder from later than it started.
HOLD_SECONDS=6
BUSY_WAIT_SECONDS=8

daemon_was_active=no
LOG_DIR="$(mktemp -d)"
RESULTS="$LOG_DIR/results"
: > "$RESULTS"

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

# Leave the board as it was found, whether the run finished, a probe
# wedged it, or the operator interrupted.
restore() {
  status=$?
  ssh -o ConnectTimeout=10 "$HOST" "rm -rf $REMOTE_DIR" 2>/dev/null || true
  if [ "$daemon_was_active" = yes ]; then
    ssh -o ConnectTimeout=10 "$HOST" "systemctl start bela_daemon" 2>/dev/null ||
      echo "WARNING: could not restart bela_daemon on $HOST" >&2
  fi
  rm -rf "$LOG_DIR"
  exit "$status"
}
trap 'restore' EXIT INT TERM

# The image greets every ssh session with a login banner and a locale
# warning on stderr, which would bury the output being parsed.
remote() {
  # shellcheck disable=SC2029 # callers build the command on this side on purpose
  ssh -n -o ConnectTimeout=10 "$HOST" "$1" 2>/dev/null
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
  echo "The board no longer gives an audio system, and killing leftover"
  echo "processes did not bring it back. The probe that did it is the last"
  echo "one below. Reboot before probing again:"
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
remote "systemctl stop bela_daemon; mkdir -p $REMOTE_DIR"

# The remote half. Bounded, because a probe that hangs holds the audio
# device and every later run would fail for that reason instead of the
# one being measured.
cat > "$LOG_DIR/probe-remote.sh" <<'REMOTE'
#!/bin/sh
# usage: probe-remote.sh <timeout-seconds> <probe-arguments...>
set -u
seconds="$1"
shift
cd "$(dirname "$0")"
chmod +x ./init_failure
timeout -s INT "$seconds" ./init_failure "$@" 2>&1
echo "probe-exit=$?"
REMOTE
scp -q -o ConnectTimeout=10 "$BIN_DIR/init_failure" "$HOST:$REMOTE_DIR/init_failure"
scp -q -o ConnectTimeout=10 "$LOG_DIR/probe-remote.sh" "$HOST:$REMOTE_DIR/probe-remote.sh"

echo "Checking the board is working to begin with..."
if ! oracle; then
  echo "the board does not give an audio system before any probe has run:" >&2
  echo "  $(oracle_detail)" >&2
  sed 's/^/        /' "$LOG_DIR/oracle.log" >&2
  exit 1
fi
record "preflight" "$(oracle_detail)"
echo "  $(oracle_detail)"

# Kills anything the last probe left running. Matched on the process
# name rather than the command line: `pkill -f init_failure` also
# matches the shell running this very command, so it would kill itself
# before reaching whatever came next.
kill_leftovers() {
  remote "pkill -9 -x init_failure; sleep 2" > /dev/null 2>&1 || true
}

# Makes sure the next probe starts from a board that works, and says so
# when it cannot. A probe that timed out still holds the audio device,
# and that would be measured as damage it did not do.
require_healthy() {
  oracle && return 0
  echo "  board unhealthy before the probe; killing leftovers and retrying"
  kill_leftovers
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
    ;;
  busy)
    # A holder keeps the audio device while the probe tries to take it,
    # then goes; the probe waits it out and tries again in the same
    # process. Detached, so the ssh call returns immediately.
    remote "cd $REMOTE_DIR && nohup ./init_failure render-check $HOLD_SECONDS \
      > holder.log 2>&1 & echo started" > /dev/null
    sleep 2
    run_probe "$PROBE_TIMEOUT" busy-probe "$BUSY_WAIT_SECONDS" > "$log"
    remote "cat $REMOTE_DIR/holder.log" > "$LOG_DIR/holder.log" 2>/dev/null || true
    detail="first=$(field "$log" busy-first) second=$(field "$log" busy-second)"
    detail="$detail (holder $(field "$LOG_DIR/holder.log" cycle), \
exit $(status_of "$log"))"
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

  if oracle; then
    after="board still works: $(oracle_detail)"
  else
    after="BOARD BROKEN AFTERWARDS: $(oracle_detail)"
  fi
  echo "  -> $after"
  record "$probe" "$detail; $after"

  case "$after" in
  BOARD*)
    # Distinguish damage that a leftover process explains from damage
    # that outlives one, which is the whole question.
    kill_leftovers
    if oracle; then
      record "$probe (after pkill)" "recovered: $(oracle_detail)"
      echo "  -> recovered once leftover processes were killed"
    else
      record "$probe (after pkill)" "still broken: $(oracle_detail)"
      wedged
    fi
    ;;
  esac
done

echo
summarise
echo
echo "Nothing here passes or fails; record what it says in docs/board-facts.md."
