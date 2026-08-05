#!/bin/sh
# Measure how a board configures its analog and digital I/O, by running
# `bela/examples/io_config` on it one configuration per process and
# recording the numbers it reports back.
#
# Usage: scripts/probe-io.sh [user@host] [run...]
#   host defaults to root@bela.local
#   with no run named, every one of them runs
#
# BELA_SYSROOT must point at a synced sysroot (scripts/sync-sysroot.sh);
# see docs/cross-compile.md.
#
# This is an experiment, not a check: it asks the board what shape its
# blocks are, and there is no answer it is expecting. That is why it is
# separate from scripts/smoke-test.sh, which is a pass/fail gate. Its
# findings belong in docs/board-facts.md.
#
# It answers the half of #11 that needs no wiring. Whether a pin does
# what the accessor says needs a voltage on it and a meter, and is not
# this script.
#
# Nothing here is destructive — every run is an audio system brought up
# and torn down, which the board does all day — so there is no oracle
# between runs the way scripts/probe-init-failure.sh has one. A
# configuration libbela refuses is a finding, and it is confined to the
# process that asked for it (see "Audio thread" in docs/board-facts.md).
#
# `bela_daemon` is stopped for the duration and restarted afterwards.
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET="aarch64-unknown-linux-gnu"
BIN_DIR="${CARGO_TARGET_DIR:-$ROOT/target}/$TARGET/release/examples"
REMOTE_DIR="/tmp/bela-rs-probe-io"

# What the board says it is comes first: every run after it is an audio
# system configured against that hardware, and a log that opens with
# the wrong board makes the rest of it worthless.
# Kept on one line per assignment: the membership tests below match on
# " $run " against " $ALL_RUNS ", which a folded line would break for
# whichever run ends up next to the newline.
ALL_RUNS="hardware defaults uniform-off no-analog no-digital analog-out"
ALL_RUNS="$ALL_RUNS channels-4 channels-2 uniform-off-4 uniform-off-2"
ALL_RUNS="$ALL_RUNS period-16 period-256"

# The arguments each run passes to the probe. Kept next to the names so
# that the summary and the command are never out of step.
run_arguments() {
  case "$1" in
  hardware) echo "hardware" ;;
  defaults) echo "context" ;;
  # `uniformSampleRate` is the claim: with it off, the analog frame
  # count should stop following the audio one.
  uniform-off) echo "context --uniform off" ;;
  no-analog) echo "context --analog off" ;;
  no-digital) echo "context --digital off" ;;
  # Asking for analog outputs a Gem Stereo may not have. Bela's
  # specifications give it none — expanded outputs are the Gem Multi's
  # — so this is where "the analog outputs are the audio outputs with a
  # +2 offset" either survives or does not.
  analog-out) echo "context --analog-out 2" ;;
  # What `uniformSampleRate` exists to remove. Bela's migration guide
  # puts the old analog-to-audio frame ratio at 0.5 for 8 channels, 1
  # for 4 and 2 for 2, which is three measurements, not one — and the
  # count has to be set on both directions at once, because asking for
  # a different number of each is refused.
  channels-4) echo "context --analog-in 4 --analog-out 4" ;;
  channels-2) echo "context --analog-in 2 --analog-out 2" ;;
  uniform-off-4) echo "context --uniform off --analog-in 4 --analog-out 4" ;;
  uniform-off-2) echo "context --uniform off --analog-in 2 --analog-out 2" ;;
  # Either side of the 128-frame boundary where libbela splits `render`
  # onto a second thread (docs/board-facts.md). The analog frame count
  # is worth having from both. 16 is also the default period, so this
  # run doubles as a check that asking for what the board would have
  # chosen anyway changes nothing.
  period-16) echo "context --period 16" ;;
  period-256) echo "context --period 256" ;;
  *) return 1 ;;
  esac
}

# The first argument is the host, unless it names a run: probing the
# default board should not need the host spelled out.
HOST=root@bela.local
if [ $# -gt 0 ]; then
  case " $ALL_RUNS " in
  *" $1 "*) ;;
  *)
    HOST="$1"
    shift
    ;;
  esac
fi
RUNS="${*:-$ALL_RUNS}"

# A run is about 0.7 s of startup, 1 s of rendering and a teardown, so
# this is a ceiling for one that hangs rather than an expected time.
RUN_TIMEOUT=40

daemon_was_active=no
# Whether the board has been touched yet. Until it has there is nothing
# to put back, and an early exit should not spend a connection timeout
# finding that out.
board_prepared=no
LOG_DIR="$(mktemp -d)"
RESULTS="$LOG_DIR/results"
: > "$RESULTS"

# Leave the board as it was found, whether the run finished or the
# operator interrupted. Installed straight after the directory it
# removes, so that the argument checks below cannot leak one by exiting
# before it.
restore() {
  status=$?
  if [ "$board_prepared" = yes ]; then
    # One connection, because an unreachable board makes each of these
    # cost a full ConnectTimeout. The kill comes first: a run still
    # holding the audio device would make the next thing to run here
    # fail for a reason of its own.
    undo="pkill -9 -x io_config; rm -rf $REMOTE_DIR"
    if [ "$daemon_was_active" = yes ]; then
      undo="$undo; systemctl start bela_daemon"
    fi
    # shellcheck disable=SC2029 # the remote path is meant to expand here
    ssh -o ConnectTimeout=10 "$HOST" "$undo" 2>/dev/null ||
      echo "WARNING: could not restore $HOST — check for a leftover" \
        "io_config process, $REMOTE_DIR, and bela_daemon" >&2
  fi
  rm -rf "$LOG_DIR"
  exit "$status"
}
trap 'restore' EXIT INT TERM

if [ -z "${BELA_SYSROOT:-}" ]; then
  echo "BELA_SYSROOT is not set; see docs/cross-compile.md" >&2
  exit 2
fi

for run in $RUNS; do
  case " $ALL_RUNS " in
  *" $run "*) ;;
  *)
    echo "unknown run '$run'; known: $ALL_RUNS" >&2
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

# The last value a run reported for one key.
field() {
  sed -n "s/^io-config: $2=//p" "$1" | tail -1
}

status_of() {
  sed -n 's/^probe-exit=//p' "$1" | tail -1
}

record() {
  printf '%s\t%s\n' "$1" "$2" >> "$RESULTS"
}

summarise() {
  echo "Results"
  echo "-------"
  while IFS="$(printf '\t')" read -r name detail; do
    printf '  %-14s %s\n' "$name" "$detail"
  done < "$RESULTS"
}

echo "Building the probe for $TARGET..."
cargo build -p bela --release --target "$TARGET" --example io_config

if [ ! -x "$BIN_DIR/io_config" ]; then
  echo "not built at $BIN_DIR/io_config" >&2
  exit 2
fi

echo "Preparing $HOST..."
if ssh -o ConnectTimeout=10 "$HOST" "systemctl is-active --quiet bela_daemon" 2>/dev/null; then
  daemon_was_active=yes
fi
# From here on there is something to put back.
board_prepared=yes
remote "systemctl stop bela_daemon; mkdir -p $REMOTE_DIR"

# The remote half. Bounded, because a run that hangs holds the audio
# device and every later one would fail for that reason instead of
# reporting its own configuration.
cat > "$LOG_DIR/run-remote.sh" <<'REMOTE'
#!/bin/sh
# usage: run-remote.sh <timeout-seconds> <probe-arguments...>
set -u
seconds="$1"
shift
cd "$(dirname "$0")"
chmod +x ./io_config
# -k: a run that misses the interrupt is killed outright rather than
# left holding the audio device.
timeout -s INT -k 5 "$seconds" ./io_config "$@" 2>&1
echo "probe-exit=$?"
REMOTE
scp -q -o ConnectTimeout=10 "$BIN_DIR/io_config" "$HOST:$REMOTE_DIR/io_config"
scp -q -o ConnectTimeout=10 "$LOG_DIR/run-remote.sh" "$HOST:$REMOTE_DIR/run-remote.sh"

for run in $RUNS; do
  echo
  echo "Run: $run ($(run_arguments "$run"))"
  log="$LOG_DIR/$run.log"
  # The arguments go over as one string and are split by the shell on
  # the board, which is where they have to be words.
  remote "sh $REMOTE_DIR/run-remote.sh $RUN_TIMEOUT $(run_arguments "$run")" > "$log" ||
    echo "probe-exit=ssh-failed" >> "$log"
  sed 's/^/    /' "$log"

  case "$run" in
  hardware)
    detail="$(field "$log" detect-hw) config[$(field "$log" hw-config)]"
    detail="$detail defaults-analog[$(field "$log" defaults-analog)]"
    detail="$detail defaults-digital[$(field "$log" defaults-digital)]"
    ;;
  *)
    init="$(field "$log" init)"
    if [ "$init" != created ]; then
      detail="init=${init:-no-output}"
    else
      detail="analog[$(field "$log" setup-analog)]"
      detail="$detail digital[$(field "$log" setup-digital)]"
      detail="$detail audio[$(field "$log" setup-audio)]"
      # The block is only worth a line of its own when it disagrees
      # with `setup`; that disagreement is the reason both are asked.
      # A block line that never arrived is a third thing — a run that
      # did not get that far, or output that was lost — and reporting
      # it as a disagreement would invent a finding.
      for domain in audio analog digital; do
        block="$(field "$log" "block-$domain")"
        if [ -z "$block" ]; then
          detail="$detail BLOCK-${domain}[missing]"
        elif [ "$(field "$log" "setup-$domain")" != "$block" ]; then
          detail="$detail BLOCK-${domain}[$block]"
        fi
      done
    fi
    ;;
  esac
  detail="$detail (exit $(status_of "$log"))"
  echo "  -> $detail"
  record "$run" "$detail"
done

echo
summarise
echo
echo "Nothing here passes or fails; record what it says in docs/board-facts.md."
