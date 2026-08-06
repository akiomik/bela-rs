#!/bin/sh
# Build the examples, run them on a board and check they actually
# rendered audio — a single pass/fail answer for "does this still work
# on hardware".
#
# Usage: scripts/smoke-test.sh [user@host] [seconds]
#   host defaults to root@bela.local, seconds to 5
#
# BELA_SYSROOT must point at a synced sysroot (scripts/sync-sysroot.sh);
# see docs/cross-compile.md.
#
# Needs a board, so it cannot run in CI. Run it before releasing, after
# updating the board image, and whenever a change touches the device
# path.
#
# `bela_daemon` is stopped for the duration and restarted afterwards,
# including when a check fails or the script is interrupted.
set -eu

HOST="${1:-root@bela.local}"
DURATION="${2:-5}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET="aarch64-unknown-linux-gnu"
BIN_DIR="${CARGO_TARGET_DIR:-$ROOT/target}/$TARGET/release/examples"
REMOTE_DIR="/tmp/bela-rs-smoke"
# `print` carries the numeric checks, `task_lifecycle` the auxiliary
# task ones, `cpu` the CPU monitoring ones, `command_line` the
# command-line option ones and `levels` the codec level ones; the others
# only have to start, keep running and stop cleanly. `monitoring_rules`
# is not here: it answers one question per run and exits, so it is
# driven separately below rather than run for the duration. Neither is
# `parallel`, which is run once per thread count.
EXAMPLES="print sine passthrough aux_task task_lifecycle cpu command_line levels"
# Thread counts `parallel` is run at, lowest first: the last one has to
# spread the same work over more cores than the first.
THREAD_COUNTS="1 2 4"
# How much of the run may be spent starting audio up rather than
# rendering (measured at about 0.6 s; rounded up for headroom).
STARTUP_ALLOWANCE_SECONDS=1.5
# Shorter runs are all startup and the block count check stops meaning
# anything.
MIN_DURATION=3

failures=0
daemon_was_active=no
LOG_DIR="$(mktemp -d)"

pass() { printf '  ok    %s\n' "$1"; }
fail() {
  printf '  FAIL  %s\n' "$1"
  failures=$((failures + 1))
}

# The image greets every ssh session with a login banner and a locale
# warning on stderr, which would bury the checks. Hold the remote stderr
# back and show it only when the command actually fails.
remote() {
  # shellcheck disable=SC2029 # callers build the command on this side on purpose
  if ! ssh -o ConnectTimeout=10 "$HOST" "$1" 2> "$LOG_DIR/ssh.err"; then
    sed 's/^/        /' "$LOG_DIR/ssh.err" >&2
    return 1
  fi
}

if [ -z "${BELA_SYSROOT:-}" ]; then
  echo "BELA_SYSROOT is not set; see docs/cross-compile.md" >&2
  exit 2
fi

if [ "$DURATION" -lt "$MIN_DURATION" ]; then
  echo "seconds must be at least $MIN_DURATION" >&2
  exit 2
fi

# Leave the board as it was found, whether the checks passed, one of
# them failed, or the run was interrupted.
restore() {
  status=$?
  # shellcheck disable=SC2029 # the remote path is meant to expand here
  ssh -o ConnectTimeout=10 "$HOST" "rm -rf $REMOTE_DIR" 2>/dev/null || true
  if [ "$daemon_was_active" = yes ]; then
    ssh -o ConnectTimeout=10 "$HOST" "systemctl start bela_daemon" 2>/dev/null ||
      echo "WARNING: could not restart bela_daemon on $HOST" >&2
  fi
  rm -rf "$LOG_DIR"
  exit "$status"
}
trap 'restore' EXIT INT TERM

echo "Building the examples for $TARGET..."
cargo build -p bela --release --target "$TARGET" --examples

echo "Preparing $HOST..."
# Only restart the daemon afterwards if it was running to begin with.
if ssh -o ConnectTimeout=10 "$HOST" "systemctl is-active --quiet bela_daemon" 2>/dev/null; then
  daemon_was_active=yes
fi
remote "systemctl stop bela_daemon; mkdir -p $REMOTE_DIR"

# The remote half: run one example for a while, interrupt it, and
# report how it went. Kept on the board so the quoting stays readable.
cat > "$LOG_DIR/run-remote.sh" <<'REMOTE'
#!/bin/sh
# usage: run-remote.sh <binary> <seconds> [arguments...]
set -eu
binary="$1"
seconds="$2"
shift 2
cd "$(dirname "$0")"
chmod +x "./$binary"

"./$binary" "$@" > "$binary.log" 2>&1 &
pid=$!
sleep "$seconds"

if ! kill -0 "$pid" 2>/dev/null; then
  wait "$pid" || true
  echo "state=exited-early"
  exit 0
fi

# A clean shutdown on SIGINT is part of what is being tested: the
# signal handler asks the audio system to stop, cleanup runs, and the
# process exits 0.
kill -INT "$pid"
waited=0
while kill -0 "$pid" 2>/dev/null; do
  if [ "$waited" -ge 50 ]; then
    kill -9 "$pid" 2>/dev/null || true
    echo "state=hung"
    exit 0
  fi
  sleep 0.1
  waited=$((waited + 1))
done
if wait "$pid"; then
  code=0
else
  code=$?
fi
echo "state=stopped exit=$code"
REMOTE
scp -q -o ConnectTimeout=10 "$LOG_DIR/run-remote.sh" "$HOST:$REMOTE_DIR/run-remote.sh"

for example in $EXAMPLES; do
  echo "Running $example for ${DURATION}s on $HOST..."
  if [ ! -x "$BIN_DIR/$example" ]; then
    fail "$example: not built at $BIN_DIR/$example"
    continue
  fi
  scp -q -o ConnectTimeout=10 "$BIN_DIR/$example" "$HOST:$REMOTE_DIR/$example"
  result="$(remote "sh $REMOTE_DIR/run-remote.sh $example $DURATION" || echo state=ssh-failed)"
  remote "cat $REMOTE_DIR/$example.log" > "$LOG_DIR/$example.log" 2>/dev/null || true

  case "$result" in
  "state=stopped exit=0") pass "$example: ran for ${DURATION}s and exited 0 on SIGINT" ;;
  state=exited-early)
    fail "$example: exited before it was interrupted"
    sed 's/^/        /' "$LOG_DIR/$example.log" >&2
    continue
    ;;
  state=hung)
    fail "$example: still running 5s after SIGINT"
    continue
    ;;
  *)
    fail "$example: $result"
    sed 's/^/        /' "$LOG_DIR/$example.log" >&2
    continue
    ;;
  esac
done

# The numeric part. `print` reports the audio configuration from setup,
# a heartbeat from render and the total from cleanup, which is enough to
# tell a callback that runs at the right rate from one that runs once,
# late, or not at all.
log="$LOG_DIR/print.log"
if [ ! -s "$log" ]; then
  fail "print: produced no output"
else
  # setup: 44100 Hz, 16 frames per block, 2 in / 2 out audio channels, 1 render thread(s)
  sample_rate="$(awk '/^setup:/ { print $2; exit }' "$log")"
  block_size="$(awk '/^setup:/ { print $4; exit }' "$log")"
  # render: 2756 blocks, 44080 frames elapsed, 0 underruns
  blocks="$(awk '/^render:/ { blocks = $2 } END { print blocks + 0 }' "$log")"
  frames="$(awk '/^render:/ { frames = $4 } END { print frames + 0 }' "$log")"
  underruns="$(awk '/^render:/ { underruns = $6 } END { print underruns + 0 }' "$log")"
  # cleanup: 12075 blocks rendered
  total="$(awk '/^cleanup:/ { print $2; exit }' "$log")"

  if [ -z "$sample_rate" ] || [ -z "$block_size" ]; then
    fail "print: no setup line"
  else
    pass "print: setup reported ${sample_rate} Hz, $block_size frames per block"
  fi

  if [ "$blocks" -eq 0 ]; then
    fail "print: render never reported a block"
  else
    # Every reporting block prints the frame count as it was at the
    # start of that block, so this identity is exact — not an estimate.
    expected_frames=$(( (blocks - 1) * block_size ))
    if [ "$frames" -eq "$expected_frames" ]; then
      pass "print: $blocks blocks match $frames elapsed frames at $block_size frames per block"
    else
      fail "print: $blocks blocks imply $expected_frames elapsed frames, got $frames"
    fi

    # Blocks per second must follow the sample rate: a callback that
    # runs at half speed, or stalls partway through, shows up here.
    #
    # The window is asymmetric on purpose. Audio cannot run ahead of the
    # wall clock, but it starts late — loading the PRU firmware and
    # bringing up the codec took about 0.6 s on the reference board — so
    # the count is compared against the run time minus a startup
    # allowance at the bottom and the full run time at the top.
    min_blocks="$(awk -v r="$sample_rate" -v b="$block_size" \
      -v s="$DURATION" -v a="$STARTUP_ALLOWANCE_SECONDS" \
      'BEGIN { printf "%d", r / b * (s - a) }')"
    max_blocks="$(awk -v r="$sample_rate" -v b="$block_size" -v s="$DURATION" \
      'BEGIN { printf "%d", r / b * s }')"
    if [ "$total" -ge "$min_blocks" ] && [ "$total" -le "$max_blocks" ]; then
      pass "print: $total blocks in ${DURATION}s, within $min_blocks..$max_blocks"
    else
      fail "print: $total blocks in ${DURATION}s, outside $min_blocks..$max_blocks"
    fi
  fi

  if [ "$underruns" -eq 0 ]; then
    pass "print: no underruns"
  else
    fail "print: $underruns underruns"
  fi

  if [ -z "$total" ]; then
    fail "print: cleanup did not run"
  else
    pass "print: cleanup ran after $total blocks"
  fi
fi

# The auxiliary task lifecycle rules, which the host tests cannot reach
# because they need a real audio system. See the example's header for
# what each field means.
log="$LOG_DIR/task_lifecycle.log"
summary="$(awk '/^lifecycle: stale-runs=/ { print; exit }' "$log" 2>/dev/null || true)"
if [ -z "$summary" ]; then
  fail "task_lifecycle: no summary line"
else
  stale="$(echo "$summary" | sed -n 's/.*stale-runs=\([0-9]*\).*/\1/p')"
  fresh="$(echo "$summary" | sed -n 's/.*fresh-runs=\([0-9]*\).*/\1/p')"
  cleanup_create="$(echo "$summary" | sed -n 's/.*cleanup-create=\([a-z-]*\).*/\1/p')"

  if [ "$stale" = 0 ]; then
    pass "task_lifecycle: a handle from a dropped audio system never ran"
  else
    fail "task_lifecycle: a retired handle ran its task $stale time(s)"
  fi

  if [ "${fresh:-0}" -gt 0 ]; then
    pass "task_lifecycle: the running audio system's own task ran $fresh time(s)"
  else
    fail "task_lifecycle: the running audio system's task never ran"
  fi

  if [ "$cleanup_create" = rejected ]; then
    pass "task_lifecycle: creating a task from cleanup was refused"
  else
    fail "task_lifecycle: creating a task from cleanup was $cleanup_create"
  fi
fi

# CPU monitoring. The numbers themselves depend on the board and on
# what else it is doing, so the checks are on the relationships that
# have to hold whatever they are: both readings are real percentages,
# and the measured section is part of the audio thread it runs on.
log="$LOG_DIR/cpu.log"
# cleanup: audio thread 8.4% busy, averaged over 2000 measurements
thread="$(awk '/^cleanup: audio thread/ { print $4 + 0; exit }' "$log" 2>/dev/null || true)"
# cleanup: oscillators 5.1% busy, averaged over 2000 measurements; 12075 blocks rendered
section="$(awk '/^cleanup: oscillators/ { print $3 + 0; exit }' "$log" 2>/dev/null || true)"
reports="$(grep -c '^cpu: ' "$log" 2>/dev/null || true)"

if [ -z "$thread" ] || [ -z "$section" ]; then
  fail "cpu: no cleanup summary"
else
  if awk -v t="$thread" 'BEGIN { exit !(t > 0 && t <= 100) }'; then
    pass "cpu: the audio thread reported ${thread}% busy"
  else
    fail "cpu: the audio thread reported ${thread}%, which is not a percentage of a block"
  fi

  # A percentage of the same block period, so it cannot exceed the
  # thread's — allowing for the two cycles not being aligned and for
  # the one decimal place the report prints.
  if awk -v s="$section" -v t="$thread" 'BEGIN { exit !(s > 0 && s <= t + 1) }'; then
    pass "cpu: the measured section reported ${section}%, within the thread's ${thread}%"
  else
    fail "cpu: the measured section reported ${section}%, against the thread's ${thread}%"
  fi
fi

if [ "${reports:-0}" -gt 0 ]; then
  pass "cpu: the reporting task ran $reports time(s)"
else
  fail "cpu: the reporting task never ran"
fi

# Above a period size libbela can render natively, it moves `render` to
# its own FIFO thread while the counters stay with the core audio
# thread, so monitoring has to be refused rather than read across the
# two. Only the board can confirm it: the split is internal to libbela.
guard="$(sed -n 's/^fifo-guard: //p' "$log" 2>/dev/null | head -1)"
case "$guard" in
"refused at "*) pass "cpu: monitoring $guard, where render moves off the measured thread" ;;
"") fail "cpu: no fifo-guard line" ;;
*) fail "cpu: monitoring was $guard at a period size that moves render off the measured thread" ;;
esac

# The codec's levels and gain, which only the board can accept or
# refuse: off-device there is no codec to talk to. What the levels do to
# the sound needs an ear, so the checks are on the answers — every call
# for a channel the board has succeeded, and one for a channel it does
# not have was refused rather than silently ignored.
log="$LOG_DIR/levels.log"
# levels: line-out=ok ... missing-channel=refused not-a-number=refused
summary="$(awk '/^levels: / { print; exit }' "$log" 2>/dev/null || true)"
if [ -z "$summary" ]; then
  fail "levels: no summary line"
else
  # The summary is one line, so this answers "did any call fail".
  any_failed="$(echo "$summary" | grep -c 'failed(' || true)"
  missing="$(echo "$summary" | sed -n 's/.*missing-channel=\([a-z-]*\).*/\1/p')"
  not_a_number="$(echo "$summary" | sed -n 's/.*not-a-number=\([a-z-]*\).*/\1/p')"

  if [ "$any_failed" = 0 ]; then
    pass "levels: the line out, headphone, input gain and unmute calls all succeeded"
  else
    fail "levels: a call the board should accept failed ($summary)"
  fi

  if [ "$missing" = refused ]; then
    pass "levels: a channel the codec does not have was refused"
  else
    fail "levels: a channel the codec does not have was ${missing:-not reported}"
  fi

  # The guard has to sit in front of the FFI call in the shipped
  # binary, not only in the host tests: libbela casts a level to `int`,
  # which is undefined for a NaN, and no clamp on the C side catches it.
  if [ "$not_a_number" = refused ]; then
    pass "levels: a level libbela could not convert was refused before the call"
  else
    fail "levels: a level libbela could not convert was ${not_a_number:-not reported}"
  fi
fi

# Bela's standard command-line options. Only the board can answer
# whether they arrived: what `setup` reports is the configuration
# libbela brought the hardware up with, not what was asked for. The run
# in the loop above passed no options, so it shows the application's own
# `Settings` default; this one passes `--period` to show the command
# line overriding it.
default_period="$(awk '/^const PERIOD_SIZE/ { print $NF + 0 }' \
  "$ROOT/bela/examples/command_line.rs" | head -1)"
override_period=$((default_period * 2))
# setup: 44100 Hz, 32 frames per block, 2 in / 2 out audio channels, ...
reported_period="$(awk '/^setup:/ { print $4; exit }' "$LOG_DIR/command_line.log" 2>/dev/null || true)"
if [ "$reported_period" = "$default_period" ]; then
  pass "command_line: the application's own default of $default_period frames per block was used"
else
  fail "command_line: expected the application's default of $default_period frames per block, \
got ${reported_period:-nothing}"
fi

# Long enough for setup to report — this run is not about rendering.
echo "Running command_line with --period $override_period on $HOST..."
result="$(remote "sh $REMOTE_DIR/run-remote.sh command_line 2 --period $override_period" ||
  echo state=ssh-failed)"
remote "cat $REMOTE_DIR/command_line.log" > "$LOG_DIR/command_line-period.log" 2>/dev/null || true
reported_period="$(awk '/^setup:/ { print $4; exit }' "$LOG_DIR/command_line-period.log" \
  2>/dev/null || true)"
if [ "$result" != "state=stopped exit=0" ]; then
  fail "command_line: --period $override_period: $result"
  sed 's/^/        /' "$LOG_DIR/command_line-period.log" >&2
elif [ "$reported_period" = "$override_period" ]; then
  pass "command_line: --period $override_period overrode the application's default"
else
  fail "command_line: --period $override_period was configured as \
${reported_period:-nothing} frames per block"
fi

# The usage text comes from libbela, so it is also a check that
# `Bela_usage` is reachable at all. No audio system is involved.
help_output="$(remote "cd $REMOTE_DIR && ./command_line --help 2>&1" || echo run-failed)"
case "$help_output" in
*--period*) pass "command_line: --help printed Bela's standard options" ;;
*) fail "command_line: --help printed no usage information ($help_output)" ;;
esac

# An option Bela does not know has to be refused rather than ignored,
# and refused before any hardware is touched.
status="$(remote "cd $REMOTE_DIR && ./command_line --not-an-option > /dev/null 2>&1; echo \$?")"
if [ "$status" = 0 ]; then
  fail "command_line: an unrecognised option was accepted"
else
  pass "command_line: an unrecognised option was refused (exit $status)"
fi

# What the board says it is, and which libbela said it. No audio system
# is involved, so this is driven directly rather than through the loop
# above, and it is the one check here that would still answer on a board
# whose audio never comes up.
if [ ! -x "$BIN_DIR/board_info" ]; then
  fail "board_info: not built at $BIN_DIR/board_info"
else
  scp -q -o ConnectTimeout=10 "$BIN_DIR/board_info" "$HOST:$REMOTE_DIR/board_info"
  remote "chmod +x $REMOTE_DIR/board_info"
  info="$(remote "cd $REMOTE_DIR && ./board_info 2>&1" || echo "board: run-failed")"
  board="$(echo "$info" | sed -n 's/^board: //p' | head -1)"
  version="$(echo "$info" | sed -n 's/^version: //p' | head -1)"

  case "$board" in
  "" | run-failed) fail "board_info: no board was reported ($info)" ;;
  NoHw) fail "board_info: libbela detected no hardware, on a board that is running this" ;;
  # A board the vendored headers do not name. Not a failure of the
  # binding — it is the case `Board::Unrecognised` exists for — but on
  # this board it means the image moved and the headers have not.
  unrecognised*) fail "board_info: the board reports as $board, which these headers do not name" ;;
  *) pass "board_info: the board reports as $board" ;;
  esac

  # The example names both versions only when they differ, so a bare
  # number is the agreement. The vendored headers are what the committed
  # bindings describe, so a disagreement means the bindings may not
  # describe the libbela they just linked against (see
  # `cargo xtask check-vendor`).
  case "$version" in
  "") fail "board_info: no version was reported ($info)" ;;
  *"built against"*) fail "board_info: $version" ;;
  *) pass "board_info: libbela $version, which is what the vendored headers say" ;;
  esac

  # Every detect mode answers. They need not agree — a board with no
  # `~/.bela/belaconfig` has `user-only` answering `NoHw`, which is the
  # mode doing its job — so the check is that each one reported at all.
  #
  # `--all-modes` ends with the scan, which is the one mode that writes
  # `/run/bela/belaconfig`. What it writes is what the daemon writes, so
  # the file is saved first and put back if the scan changed it: this
  # script is meant to leave the board as it found it, and "it would
  # have written the same thing" is the claim being checked rather than
  # an assumption to run on.
  BELACONFIG=/run/bela/belaconfig
  before="$(remote "cat $BELACONFIG 2>/dev/null" || true)"
  all_modes="$(remote "cd $REMOTE_DIR && ./board_info --all-modes 2>&1" || true)"
  after="$(remote "cat $BELACONFIG 2>/dev/null" || true)"
  modes="$(echo "$all_modes" | grep -c '^board\[' || true)"
  if [ "$modes" = 5 ]; then
    pass "board_info: all five detect modes answered"
  else
    fail "board_info: $modes of five detect modes answered"
    echo "$all_modes" | sed 's/^/        /' >&2
  fi

  # The scan ran after the modes that read the cache, so this compares
  # what the daemon had left with what a fresh scan of the buses found.
  if [ "$before" = "$after" ]; then
    pass "board_info: the scan agreed with the cache the daemon had written"
  else
    fail "board_info: the scan rewrote $BELACONFIG ('$before' became '$after'); restoring"
    # Put back what was found, so the next thing to read the cache sees
    # what the board booted with rather than what this run left.
    remote "printf '%s\n' '$before' > $BELACONFIG" || true
  fi
fi

# The audio system rules that only a board can answer. One audio system
# per run and no `Bela::run`, so these are driven directly rather than
# through the loop above: three of these checks abort the initialisation
# from `setup`, which makes `Bela::new` give up on the process they run
# in, so a second check sharing one would be refused rather than run.
# See the example's header.
if [ ! -x "$BIN_DIR/monitoring_rules" ]; then
  fail "monitoring_rules: not built at $BIN_DIR/monitoring_rules"
else
  scp -q -o ConnectTimeout=10 "$BIN_DIR/monitoring_rules" "$HOST:$REMOTE_DIR/monitoring_rules"
  remote "chmod +x $REMOTE_DIR/monitoring_rules"

  # Runs one check and returns what it printed.
  rules() {
    remote "cd $REMOTE_DIR && ./monitoring_rules $1 2>&1" || echo "rules: run-failed"
  }

  # That the refusal checked above uses the *right* limit. libbela
  # prints the gFifoFactor it picked under `verbose`: 1 while `render`
  # still runs on the thread the counters belong to, more once it does
  # not. Each probe is its own process, so its output is its own.
  limit="$(awk '/^pub const MAX_MONITORED_PERIOD_SIZE/ { print $NF + 0 }' \
    "$ROOT/bela/src/cpu.rs" | head -1)"
  at_limit="$(rules "fifo-probe $limit" | sed -n 's/^gFifoFactor: //p' | head -1)"
  above_limit="$(rules "fifo-probe $((limit * 2))" | sed -n 's/^gFifoFactor: //p' | head -1)"

  if [ -z "$at_limit" ] || [ -z "$above_limit" ]; then
    fail "monitoring_rules: libbela reported no gFifoFactor (got '$at_limit' / '$above_limit')"
  elif [ "$at_limit" != 1 ]; then
    fail "monitoring_rules: gFifoFactor is $at_limit at the limit of $limit frames, so render already \
runs off the measured thread there"
  elif [ "$above_limit" = 1 ]; then
    fail "monitoring_rules: gFifoFactor is still 1 at $((limit * 2)) frames, so the limit of $limit is \
lower than the hardware needs"
  else
    pass "monitoring_rules: gFifoFactor is 1 at the limit of $limit frames and $above_limit above it"
  fi

  second_new="$(rules second-new | sed -n 's/^rules: second-new=//p' | head -1)"
  if [ "$second_new" = refused ]; then
    pass "monitoring_rules: a second audio system was refused"
  else
    fail "monitoring_rules: a second audio system was ${second_new:-not reported}"
  fi

  requested="$(rules "monitoring on" | sed -n 's/^rules: monitoring=//p' | head -1)"
  unset_to="$(rules "monitoring off" | sed -n 's/^rules: monitoring=//p' | head -1)"
  if [ "$requested" = some ] && [ "$unset_to" = none ]; then
    pass "monitoring_rules: monitoring was on when asked for and off when not"
  else
    fail "monitoring_rules: expected some/none when asked for and not, got \
${requested:-nothing}/${unset_to:-nothing}"
  fi

  # The refusal after a failed initialisation. Only the board can tell
  # it from what it replaces: without it the second `Bela::new` here is
  # a segfault inside libbela, so a run that prints its line at all has
  # already shown most of the answer.
  poisoned="$(rules poisoned | sed -n 's/^rules: //p' | head -1)"
  case "$poisoned" in
  "first-init=failed poisoned-new=refused")
    pass "monitoring_rules: a Bela::new after a failed initialisation was refused"
    ;;
  run-failed | "")
    # What the failure this guards against looks like from here: the
    # check reaches its second `Bela::new`, libbela segfaults, and the
    # non-zero exit reaches `rules` before any line is printed.
    fail "monitoring_rules: the poisoned check did not survive to report, which is what \
the segfault it guards against looks like"
    ;;
  *) fail "monitoring_rules: poisoned check reported '$poisoned'" ;;
  esac
fi

# Multithreaded rendering, which only a board can answer: whether the
# extra render threads exist, whether they land on different cores, and
# whether they divided the block rather than each rendering all of it.
# One audio system per process, so each thread count is its own run.
# See the example's header for what each field means.
if [ ! -x "$BIN_DIR/parallel" ]; then
  fail "parallel: not built at $BIN_DIR/parallel"
else
  scp -q -o ConnectTimeout=10 "$BIN_DIR/parallel" "$HOST:$REMOTE_DIR/parallel"
  single_section=
  for threads in $THREAD_COUNTS; do
    echo "Running parallel with $threads render thread(s) for ${DURATION}s on $HOST..."
    log="$LOG_DIR/parallel-$threads.log"
    result="$(remote "sh $REMOTE_DIR/run-remote.sh parallel $DURATION $threads" ||
      echo state=ssh-failed)"
    remote "cat $REMOTE_DIR/parallel.log" > "$log" 2>/dev/null || true
    if [ "$result" != "state=stopped exit=0" ]; then
      fail "parallel: $threads thread(s): $result"
      sed 's/^/        /' "$log" >&2
      continue
    fi

    # parallel: thread=0 tid=78455 cpu=3 range=0..4 calls=14705 frames=58820 section=10.5%
    reported="$(grep -c '^parallel: thread=' "$log" 2>/dev/null || true)"
    tids="$(sed -n 's/^parallel: thread=[0-9]* tid=\([0-9-]*\).*/\1/p' "$log" | sort -u | wc -l)"
    cpus="$(sed -n 's/^parallel: thread=[0-9]* tid=[0-9-]* cpu=\([0-9-]*\).*/\1/p' "$log" |
      sort -u | wc -l)"
    # parallel: faults=0 — no callback was refused for arriving where
    # the crate could not serve it safely, which is the guard the whole
    # parallel path rests on and the only check for it that runs on a
    # board.
    faults="$(sed -n 's/^parallel: faults=\([0-9]*\).*/\1/p' "$log" | head -1)"
    # parallel: blocks=14705 frames=16 rendered=235280 expected=235280
    summary="$(awk '/^parallel: blocks=/ { print; exit }' "$log" 2>/dev/null || true)"
    block_frames="$(echo "$summary" | sed -n 's/.*frames=\([0-9]*\).*/\1/p')"
    rendered="$(echo "$summary" | sed -n 's/.*rendered=\([0-9]*\).*/\1/p')"
    expected="$(echo "$summary" | sed -n 's/.*expected=\([0-9]*\).*/\1/p')"
    # bela: 1 callback(s) were refused while stopping, ...
    # The crate's own account of the same event, from `until_stopped`.
    stopping_faults="$(sed -n 's/^bela: \([0-9]*\) callback(s) were refused while stopping.*/\1/p' \
      "$log" | head -1)"
    # parallel: uncovered=0 abandoned=0 unfinished=0
    coverage="$(awk '/^parallel: uncovered=/ { print; exit }' "$log" 2>/dev/null || true)"
    uncovered="$(echo "$coverage" | sed -n 's/.*uncovered=\([0-9]*\).*/\1/p')"
    abandoned="$(echo "$coverage" | sed -n 's/.*abandoned=\([0-9]*\).*/\1/p')"
    unfinished="$(echo "$coverage" | sed -n 's/.*unfinished=\([0-9]*\).*/\1/p')"
    # The busiest thread's share of the block, which is what has to fall
    # as threads are added: they render at the same time, so the block
    # is only finished when the last of them is.
    section="$(sed -n 's/.*section=\([0-9.]*\)%.*/\1/p' "$log" | sort -rn | head -1)"

    if [ "$reported" != "$threads" ]; then
      fail "parallel: $threads thread(s): ${reported:-0} reported for themselves"
      continue
    fi
    pass "parallel: $threads thread(s): all $threads reported for themselves"

    if [ "$(echo "$tids" | tr -d ' ')" = "$threads" ] &&
      [ "$(echo "$cpus" | tr -d ' ')" = "$threads" ]; then
      pass "parallel: $threads thread(s): $threads distinct Linux thread id(s), on $threads core(s)"
    else
      fail "parallel: $threads thread(s): $(echo "$tids" | tr -d ' ') distinct thread id(s) on \
$(echo "$cpus" | tr -d ' ') core(s), so the work was not spread"
    fi

    if [ "$faults" = 0 ]; then
      pass "parallel: $threads thread(s): no callback was refused"
    else
      fail "parallel: $threads thread(s): ${faults:-no} callback fault(s) reported"
    fi

    # Every frame was written once or not at all. A frame written twice
    # would push `rendered` past `expected` without lowering
    # `uncovered`, so a negative shortfall is the duplication check; a
    # shortfall of up to one block is the stop landing mid-block, which
    # the example's header describes in both of its shapes. `abandoned`
    # is one of them and `unfinished` the other, and they are mutually
    # exclusive, so their sum is "blocks cut short on the way out".
    if [ -z "$rendered" ] || [ -z "$expected" ] || [ -z "$abandoned" ] ||
      [ -z "$unfinished" ] || [ -z "$block_frames" ]; then
      fail "parallel: $threads thread(s): no summary line"
    else
      shortfall=$((expected - rendered - uncovered))
      cut_short=$((abandoned + unfinished))
      if [ "$shortfall" -lt 0 ]; then
        # Either a frame was written twice, or a block was rendered
        # without being counted — `expected` comes from `render_pre`,
        # so a refused `render_pre` would take a block out of it while
        # leaving the frames in `rendered`.
        fail "parallel: $threads thread(s): $rendered rendered + $uncovered uncovered is more \
than the $expected frames of the run — a frame was written twice, or a render_pre was refused"
      elif [ "$shortfall" -gt "$block_frames" ] || [ "$cut_short" -gt 1 ]; then
        fail "parallel: $threads thread(s): $shortfall frames over $cut_short block(s) went \
unaccounted for, which is more than the one block a stop can cut short"
      elif [ "$cut_short" -eq 0 ]; then
        pass "parallel: $threads thread(s): $rendered frames rendered for $expected, \
every frame accounted for exactly once"
      else
        # The frames of that block that were never rendered show up as
        # `uncovered` when its `render_post` ran and as the shortfall
        # when it did not, so the two together are what was lost.
        pass "parallel: $threads thread(s): $rendered frames rendered for $expected, with the \
last block cut short by the stop ($((uncovered + shortfall)) frames)"
      fi

      # The example and the crate count the same shutdown from opposite
      # sides: a block left unfinished is a `render_post` the crate
      # refused, so every one of them has to appear in the crate's own
      # tally. The reverse does not hold — a late `render` turned away
      # by a `render_post` that did run is refused too, and finishes
      # its block — so this is a floor, not an equality.
      if [ "$unfinished" -eq 0 ]; then
        pass "parallel: $threads thread(s): no block was left unfinished"
      elif [ "${stopping_faults:-0}" -ge "$unfinished" ]; then
        pass "parallel: $threads thread(s): $unfinished unfinished block(s), and the crate \
reported ${stopping_faults} refusal(s) while stopping to match"
      else
        fail "parallel: $threads thread(s): $unfinished block(s) were left unfinished but the \
crate reported ${stopping_faults:-no} refusal(s) while stopping — the two do not agree"
      fi
    fi

    if [ -z "$section" ]; then
      fail "parallel: $threads thread(s): no section measurement"
    elif [ -z "$single_section" ]; then
      single_section="$section"
      pass "parallel: $threads thread(s): the busiest thread used ${section}% of the block"
    else
      # Never as good as dividing by the thread count — the threads are
      # woken and waited for — so this only asks that adding cores
      # helped at all, with room for the noise of a short run.
      wanted="$(awk -v s="$single_section" -v t="$threads" 'BEGIN { printf "%.1f", s / t * 1.5 }')"
      if awk -v a="$section" -v b="$wanted" 'BEGIN { exit !(a <= b) }'; then
        pass "parallel: $threads thread(s): the busiest thread used ${section}% of the block, \
down from ${single_section}% on one"
      else
        fail "parallel: $threads thread(s): the busiest thread used ${section}% of the block \
against ${single_section}% on one, which is not a share of the work"
      fi
    fi
  done
fi

echo
if [ "$failures" -eq 0 ]; then
  echo "smoke test passed"
else
  echo "smoke test failed ($failures check(s))"
  exit 1
fi
