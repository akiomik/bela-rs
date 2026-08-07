#!/bin/sh
# Measure what libbela does with the standard command-line options when
# they are given something it cannot use, by running
# `bela/examples/command_line` on a board one case per process and
# recording where each one fails: the parse, `Bela_initAudio`,
# `Bela_startAudio`, or nowhere at all.
#
# Usage: scripts/probe-command-line.sh [user@host] [case...]
#   host defaults to root@bela.local
#   with no case named, every one of them runs
#
# BELA_SYSROOT must point at a synced sysroot (scripts/sync-sysroot.sh);
# see docs/cross-compile.md.
#
# This is an experiment, not a check: it asks where each option is
# caught, and there is no answer it is expecting. That is why it is
# separate from scripts/smoke-test.sh, which is a pass/fail gate. Its
# findings belong in docs/board-facts.md.
#
# It answers #84, which asks which options this crate owes a check of
# its own. What matters about a case is not that it fails but *where*:
# a parse-time refusal costs the caller an error, while a refusal inside
# `Bela_initAudio` costs the process its ability to build another audio
# system (see "Audio thread" in docs/board-facts.md), and a case that is
# caught nowhere costs whatever the board does about it.
#
# One case per process for that same reason. Nothing here is
# destructive: every case is an audio system that either comes up or
# does not, and the board is left as it was found.
#
# `--high-performance-mode` is deliberately not among the cases: it
# gives the audio thread enough CPU that the Linux side may stop
# responding, which is not a state to leave a board in unattended.
#
# `bela_daemon` is stopped for the duration and restarted afterwards.
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET="aarch64-unknown-linux-gnu"
BIN_DIR="${CARGO_TARGET_DIR:-$ROOT/target}/$TARGET/release/examples"
REMOTE_DIR="/tmp/bela-rs-probe-command-line"

# Kept on one line per assignment: the membership tests below match on
# " $case " against " $ALL_CASES ", which a folded line would break for
# whichever case ends up next to the newline.
ALL_CASES="unknown-option missing-value receive-port transmit-port"
ALL_CASES="$ALL_CASES server-name json-file-missing json-string-malformed"
ALL_CASES="$ALL_CASES analog-3 analog-0 analog-100 digital-100 digital-0"
ALL_CASES="$ALL_CASES period-0 period-3 rate-text rate-negative"
ALL_CASES="$ALL_CASES analog-flag-2 uniform-5 pru-number-5 board-mismatch"
ALL_CASES="$ALL_CASES board-unknown stop-pin-9999 disabled-digitals-all"
ALL_CASES="$ALL_CASES pru-file-missing codec-mode-garbage mux-1 mux-8"
ALL_CASES="$ALL_CASES mux-no-analog expander-inputs"

# The arguments each case passes, kept next to the names so that the
# summary and the command are never out of step.
case_arguments() {
  case "$1" in
  # Options that are not options: the first two are what getopt itself
  # rejects, the next three are printed by `Bela_usage` — and so by
  # this crate's `print_usage` — without being implemented.
  unknown-option) echo "--nonsense" ;;
  missing-value) echo "--period" ;;
  receive-port) echo "--receive-port 9998" ;;
  transmit-port) echo "--transmit-port 9999" ;;
  server-name) echo "--server-name 127.0.0.1" ;;
  # The two options that read a file or a string of their own.
  json-file-missing) echo "--json-file /nonexistent.json" ;;
  json-string-malformed) echo "--json-string {" ;;
  # Channel counts libbela reshapes rather than refuses.
  analog-3) echo "-C 3" ;;
  analog-0) echo "-C 0" ;;
  analog-100) echo "-C 100" ;;
  digital-100) echo "-B 100" ;;
  digital-0) echo "-B 0" ;;
  # A period size the hardware cannot keep up with. 0 is clamped to 1
  # by the parser, so both of these ask what happens below the floor.
  period-0) echo "-p 0" ;;
  period-3) echo "-p 3" ;;
  # `atof` on something that is not a number gives 0, and the parser
  # clamps a negative rate to the same 0.
  rate-text) echo "-r abc" ;;
  rate-negative) echo "-r -5" ;;
  # Options documented as booleans, given something else.
  analog-flag-2) echo "-N 2" ;;
  uniform-5) echo "-U 5" ;;
  # Hardware the board does not have or cannot use.
  pru-number-5) echo "--pru-number 5" ;;
  board-mismatch) echo "--board BelaMini" ;;
  board-unknown) echo "--board nonsense" ;;
  stop-pin-9999) echo "--stop-button-pin 9999" ;;
  disabled-digitals-all) echo "--disabled-digital-channels 65535" ;;
  pru-file-missing) echo "--pru-file /nonexistent" ;;
  codec-mode-garbage) echo "--codec-mode garbage" ;;
  # The Capelet settings, whose hardware cannot be attached to a Gem at
  # all (see "The Multiplexer Capelet" in docs/board-facts.md).
  mux-1) echo "-X 1" ;;
  mux-8) echo "-X 8" ;;
  mux-no-analog) echo "-X 8 -N 0" ;;
  expander-inputs) echo "-Y 0,1" ;;
  *) return 1 ;;
  esac
}

# The first argument is the host, unless it names a case.
HOST=root@bela.local
if [ $# -gt 0 ]; then
  case " $ALL_CASES " in
  *" $1 "*) ;;
  *)
    HOST="$1"
    shift
    ;;
  esac
fi
CASES="${*:-$ALL_CASES}"

# A case that comes up runs until this interrupts it, so this is the
# length of a successful case rather than a ceiling for a stuck one.
RUN_TIMEOUT=4

daemon_was_active=no
board_prepared=no
LOG_DIR="$(mktemp -d)"
RESULTS="$LOG_DIR/results"
: > "$RESULTS"

restore() {
  status=$?
  if [ "$board_prepared" = yes ]; then
    undo="pkill -9 -x command_line; rm -rf $REMOTE_DIR"
    if [ "$daemon_was_active" = yes ]; then
      undo="$undo; systemctl start bela_daemon"
    fi
    # shellcheck disable=SC2029 # the remote path is meant to expand here
    ssh -o ConnectTimeout=10 "$HOST" "$undo" 2>/dev/null ||
      echo "WARNING: could not restore $HOST — check for a leftover" \
        "command_line process, $REMOTE_DIR, and bela_daemon" >&2
  fi
  rm -rf "$LOG_DIR"
  exit "$status"
}
trap 'restore' EXIT INT TERM

if [ -z "${BELA_SYSROOT:-}" ]; then
  echo "BELA_SYSROOT is not set; see docs/cross-compile.md" >&2
  exit 2
fi

for name in $CASES; do
  case " $ALL_CASES " in
  *" $name "*) ;;
  *)
    echo "unknown case '$name'; known: $ALL_CASES" >&2
    exit 2
    ;;
  esac
done

# The image greets every ssh session with a login banner and a locale
# warning on stderr, which would bury the output being parsed.
remote() {
  # shellcheck disable=SC2029 # callers build the command on this side on purpose
  if ! ssh -n -o ConnectTimeout=10 "$HOST" "$1" 2> "$LOG_DIR/ssh.err"; then
    sed 's/^/        /' "$LOG_DIR/ssh.err" >&2
    return 1
  fi
}

echo "Building the probe for $TARGET..."
cargo build -p bela --release --target "$TARGET" --example command_line

if [ ! -x "$BIN_DIR/command_line" ]; then
  echo "not built at $BIN_DIR/command_line" >&2
  exit 2
fi

echo "Preparing $HOST..."
if ssh -o ConnectTimeout=10 "$HOST" "systemctl is-active --quiet bela_daemon" 2>/dev/null; then
  daemon_was_active=yes
fi
board_prepared=yes
remote "systemctl stop bela_daemon; mkdir -p $REMOTE_DIR"

# The remote half. `timeout` reports 124 for a case it had to interrupt,
# which is how a case that came up and kept running is told from one
# that gave up on its own.
cat > "$LOG_DIR/run-remote.sh" <<'REMOTE'
#!/bin/sh
# usage: run-remote.sh <timeout-seconds> <probe-arguments...>
set -u
seconds="$1"
shift
cd "$(dirname "$0")"
chmod +x ./command_line
timeout -s INT -k 5 "$seconds" ./command_line "$@" > out.log 2>&1
echo "probe-exit=$?"
# The usage message is dumped whenever the command line is refused, and
# it is the same 30 lines every time.
grep -v '^   --' out.log | grep -v '^Usage:' | grep -v '^ `changains`'
REMOTE
scp -q -o ConnectTimeout=10 "$BIN_DIR/command_line" "$HOST:$REMOTE_DIR/command_line"
scp -q -o ConnectTimeout=10 "$LOG_DIR/run-remote.sh" "$HOST:$REMOTE_DIR/run-remote.sh"

for name in $CASES; do
  echo
  echo "Case: $name ($(case_arguments "$name"))"
  log="$LOG_DIR/$name.log"
  # The arguments go over as one string and are split by the shell on
  # the board, which is where they have to be words.
  remote "sh $REMOTE_DIR/run-remote.sh $RUN_TIMEOUT $(case_arguments "$name")" > "$log" ||
    echo "probe-exit=ssh-failed" >> "$log"
  sed 's/^/    /' "$log"

  exit_code="$(sed -n 's/^probe-exit=//p' "$log" | tail -1)"
  # Where it was caught, in the order the run would reach them. `setup`
  # having run at all is the interesting part of the last two: the
  # application was already live when the board gave up.
  if grep -q '^Error: an argument is not one of' "$log"; then
    where="parse"
  elif grep -q 'Bela_initAudio failed' "$log"; then
    where="initAudio"
  elif grep -q 'Bela_startAudio failed' "$log"; then
    where="startAudio (setup had run)"
  elif grep -q 'McASP error, abort' "$log"; then
    where="killed by libbela (setup had run)"
  elif [ "$exit_code" = 134 ]; then
    # 128 + SIGABRT: a C++ exception nobody caught, thrown while
    # parsing, so before any of the three calls above was reached.
    where="aborted in the parse"
  elif [ "$exit_code" = 124 ]; then
    where="nowhere: it ran"
  else
    where="unclassified"
  fi
  detail="$where (exit $exit_code)"
  printf '%s\t%s\n' "$name" "$detail" >> "$RESULTS"
  echo "  -> $detail"
done

echo
echo "Results"
echo "-------"
while IFS="$(printf '\t')" read -r name detail; do
  printf '  %-22s %s\n' "$name" "$detail"
done < "$RESULTS"
echo
echo "Nothing here passes or fails; record what it says in docs/board-facts.md."
