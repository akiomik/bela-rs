# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Bela's standard command-line options, wrapping `Bela_getopt_long` and
  `Bela_usage`. `Bela::run_with_args` and `Bela::new_with_args` take the
  argument list a program was started with and apply `--period`,
  `--verbose`, `--use-analog` and the rest of the options every other
  way of writing a Bela program accepts, so a built binary can be
  reconfigured without rebuilding it; `print_usage` prints the list for
  a program's own `--help`. They are applied on top of `Settings`, which
  is in turn applied on top of `Bela_defaultSettings()` — so an
  application keeps the defaults it was built with, and the command line
  still wins over them. Options of the program's own are not handed to
  Bela: parse them first with whatever argument parser the program
  already uses and pass on what is left, which keeps `getopt`'s globals
  and its argv permutation out of the API and makes anything Bela does
  not recognise an error rather than something quietly ignored. See
  `examples/command_line.rs`

- `Bela` now enforces the "only one at a time" rule it documented:
  the process-wide claim on the audio system is taken atomically by
  `Bela::new`, and a second one fails with `Error::AudioSystemExists`
  instead of reaching into libbela's globals — from this thread or any
  other. That matters beyond tidiness now that setting an audio system
  up touches the CPU monitoring counters before `Bela_initAudio` gets
  a chance to refuse, which a second setup would race against the
  first audio system's thread. The claim is released when the `Bela`
  is dropped, including when construction fails partway through or a
  panic unwinds through it

- CPU monitoring in the `bela` crate, wrapping `Bela_cpuMonitoringInit`
  / `Bela_cpuMonitoringGet` / `Bela_cpuTic` / `Bela_cpuToc`. It answers
  whether `render` fits within its block deadline, which until now only
  showed up as dropouts and `Context::underrun_count` after the fact.
  `Settings::cpu_monitoring` turns on the monitoring libbela does for
  the whole audio thread and `Context::cpu_usage` reads it;
  `CpuTimer` measures a section of `render` with counters the
  application owns, bracketed either by `tic`/`toc` or by the
  `measure()` guard, both real-time safe. Both are read as a
  `CpuUsage`, which implements `Display`.
  Those two shapes are a soundness requirement rather than a
  preference. The audio thread's counters are one unsynchronised
  structure inside libbela: reading them from another thread while it
  runs is a data race, which is undefined behaviour no matter how
  sensible the values look, and turning monitoring on resets that same
  structure. So enabling is a setting, applied by `Bela::new` before
  `Bela_initAudio` — early enough that `setup` already sees it, and
  long before an audio thread exists — and reading needs the
  `&Context` only a Bela callback has: `setup` runs before the audio
  thread starts, `render` runs on it, and `cleanup` runs after libbela
  has joined it. To report from an auxiliary task, publish the reading
  from `render` through an atomic.
  That `render` runs on the measured thread stops being true at large
  period sizes, where libbela keeps the counters on the core audio
  thread and moves `render` to a FIFO thread of its own. Nothing in the
  context distinguishes the two arrangements, so monitoring is refused
  above `MAX_MONITORED_PERIOD_SIZE` with
  `Error::CpuMonitoringPeriodSize` rather than read across threads.
  Measured on the board: `gFifoFactor` is 1 at 16, 32, 64 and 128
  frames and 2 at 256, recorded in `docs/board-facts.md`
  The cycle length is a `NonZeroU32`, which is also how the API avoids
  the count that the C documentation says disables monitoring:
  `Bela_cpuMonitoringInit(0)` returns without doing anything, so
  turning it off clears the count directly, and leaving
  `Settings::cpu_monitoring` unset means off rather than "whatever the
  last audio system left behind". A cycle too large for the C `int`
  libbela takes is refused with `Error::CpuMonitoringCycle` instead of
  being saturated into a different cycle than the one asked for.
  Bela measures each period from the previous tic, and a first tic has
  only a zeroed timestamp to measure from — the whole monotonic clock.
  `CpuTimer` throws its first measurement away, so every reading it
  gives counts; the audio thread's cannot, because libbela takes that
  tic, so its first cycle also counts the audio system starting up.
  Measured on the board: a first reading of 9.8% against a steady 19.0%
- `bela/examples/cpu.rs`, running a bank of 64 sine oscillators with
  monitoring over the whole audio thread and a `CpuTimer` over the
  oscillators alone, both read in `render` and reported once a second
  from an auxiliary task through atomics — the pattern to copy.
  `scripts/smoke-test.sh` runs it and checks that both readings are
  percentages of a block, that the measured section stays within the
  thread that runs it, and that monitoring is refused at a period size
  where `render` would move off that thread — a rule only the board can
  confirm, since the split happens inside libbela
- `bela/examples/monitoring_rules.rs`, the hardware checks for the
  monitoring rules the host cannot reach, driven one per run by
  `scripts/smoke-test.sh`: that `MAX_MONITORED_PERIOD_SIZE` is the
  limit the *hardware* has rather than one that drifted from it (the
  refusal alone would keep passing, since it only consults the
  constant), that a second `Bela::new` is refused, and that leaving
  `cpu_monitoring` unset really means off rather than whatever the last
  audio system left behind.
  One check per process, because the board does not survive several
  audio systems in one: bringing four or five up and tearing them down
  again ends in a bus error, and an initialisation aborted from `setup`
  leaves libbela holding hardware it will not take back, so the next
  one fails with `Mcasp::start() called while already running` and then
  segfaults

- `AuxiliaryTask` in the `bela` crate: a safe wrapper over
  `Bela_createAuxiliaryTask` / `Bela_scheduleAuxiliaryTask`, so work
  that must not happen in `render` (I/O, allocation, long
  calculations) can be moved to a lower-priority thread that `render`
  triggers with a real-time safe `schedule()`. The callback is a
  `'static` closure that owns its state — it cannot borrow from the
  application, which the audio thread holds by `&mut` while the task
  runs — and shares with `render` through atomics or a lock-free
  queue. `AUDIO_PRIORITY` is exposed to pick a priority below the
  audio thread. `schedule` takes a `&Context`, which is a witness that
  the caller is inside a Bela callback: stopping the audio system
  deletes every task, and libbela joins the render thread before doing
  so, so a schedule made from a callback can never be in flight while
  the task behind it is freed. Handles also record which audio system
  they belong to, so one that outlives its audio system stays retired
  even if a later audio system creates tasks of its own — including
  when that audio system was initialised but never started. Creating a
  task while an audio system is being torn down fails with
  `Error::TaskCreateWhileStopping`, which is also what a `cleanup`
  callback gets, since it runs inside that teardown.
  Measured on the board: a request that arrives while the callback is
  still running is silently lost, and the C return value does not
  report it, so `schedule()` returns nothing and the documentation
  says to count invocations in the callback when it matters
- `bela/examples/aux_task.rs`, reporting from a task scheduled once a
  second by `render`, including work that allocates
- `bela/examples/task_lifecycle.rs`, a hardware check for the task
  lifecycle rules the host tests cannot reach — a handle from an audio
  system that was dropped without ever starting never runs, the running
  system's own task does, and creating a task from `cleanup` is
  refused. `scripts/smoke-test.sh` asserts on all three

- `scripts/smoke-test.sh`: builds the examples, runs each of them on a
  board and gives a single pass/fail answer. The checks are numeric
  rather than "it did not crash" — the reported block count has to
  match the elapsed frame count exactly and land in the window the
  sample rate implies for the run, with no underruns — plus a clean
  exit on SIGINT after `cleanup` ran. `bela_daemon` is stopped for the
  duration and restarted afterwards, including on failure or Ctrl-C.
  Needs a board, so it stays outside CI; it is now a step in
  `docs/release.md`
- `rt_print!` and `rt_println!` in the `bela` crate: `format!`-style
  printing that is usable from `render`. Arguments are formatted into a
  fixed-size buffer on the stack (`MESSAGE_CAPACITY`, 256 bytes) and
  passed to Bela's real-time print function as the argument of a
  literal `%s`, so nothing allocates and text containing `%` is not
  treated as a format string. Messages that do not fit are truncated on
  a `char` boundary and end with `...`. Off-device the same bytes go to
  stdout, so application code that prints still compiles and behaves on
  the host. `print_args` / `println_args` are the underlying functions
  for callers that already have `format_args!`
- `bela/examples/print.rs`, printing the audio configuration from
  `setup` and a once-a-second heartbeat from `render`
- `docs/multithreaded-rendering.md`, recording what `threadCount` does
  on the board: `render` runs concurrently on every thread, for the
  same block, with the same user data and the same (unpartitioned)
  buffers

### Fixed

- `rt_print!` and `rt_println!` stop formatting as soon as the message
  fills the buffer, instead of running the formatting machinery to the
  end and discarding what no longer fits. Padding is written one
  `char` at a time, so `rt_println!("{:width$}", "", width = 65_535)`
  was 65_535 calls into the writer on the audio thread — bounded
  memory, but not bounded time, and enough to miss the render
  deadline. The output is unchanged: the message is still truncated on
  a `char` boundary and marked with `...`

- `Bela::new` rejects a `Settings::thread_count` above 1 with the new
  `Error::ThreadCountUnsupported` instead of initialising an unsound
  setup. Bela calls `render` on all render threads at once with the
  same user data, so the trampoline would have handed out several
  `&mut T` to one application — reachable from safe code. A trait
  shaped for concurrent rendering is still to be designed

- `scripts/sync-sysroot.sh` no longer aborts partway through. `rsync`
  cannot reproduce the setgid bit of files like
  `/usr/lib/aarch64-linux-gnu/utempter/utempter` as an unprivileged
  user on the host, and the resulting error stopped the script before
  it created the `lib` and `ld-linux-aarch64.so.1` symlinks that
  cross-linking needs. The sysroot now drops setuid/setgid bits, which
  it never needs

### Changed

- `scripts/sync-sysroot.sh` no longer compresses the transfer: gzip on
  the board's Cortex-A53 was the bottleneck rather than the link, and
  dropping `-z` cuts a full sync from 163 s to about 40 s

## [0.1.0] - 2026-08-05

First hardware-validated release. The examples cross-build on a host
and produce sound on a Bela Gem Stereo (PocketBeagle 2, EVL real-time
core).

### Added

- `bela-sys`: `build.rs` emits the `libbela` link flags for device
  targets, so device binaries link and run. Cross-linking is driven by
  `BELA_SYSROOT` together with `scripts/sync-sysroot.sh` and the linker
  wrapper in `scripts/aarch64-bela-linker.sh`
- `bela`: `Context::this_thread` / `thread_count` and
  `Settings::thread_count` for the multithreaded rendering added in
  Bela 1.15
- `bela`: `Bela::run` also handles SIGHUP, so a dropped ssh connection
  shuts the audio system down cleanly instead of killing the process
  outright

### Changed

- `bela-sys`: bindings regenerated from the headers shipped on a Bela
  Gem (Bela 1.18.0), which is newer than any published upstream branch.
  `BelaContext` and `BelaInitSettings` gained fields, and the bindings
  now cover `Bela_initRtBackend`, `Bela_clock_gettime` and friends.
  `scripts/update-vendor.sh --board` vendors from a board, and the
  provenance file is `vendor/bela/SOURCE` (was `COMMIT`)

## [0.0.1] - 2026-07-27

Initial release: pre-hardware. Linking against `libbela` is not wired
up yet, so the crates compile (host and `aarch64-unknown-linux-gnu`)
but cannot produce a runnable device binary until the board arrives.

### Added

- Release workflow publishing to crates.io on version tags via Trusted
  Publishing (the initial 0.0.1 publish is manual, as crates.io
  requires for new crates)
- CI job verifying the MSRV (`rust-version` in Cargo.toml), and a
  pinned development toolchain in `rust-toolchain.toml` so that a
  moving `stable` cannot break builds
- Workspace-wide Clippy configuration (pedantic, nursery, cargo and
  selected restriction lints) with the codebase cleaned up to pass it;
  CI now also lints the device-only code for the aarch64 target
- `bela`: `passthrough` and `sine` examples written against the safe
  API only, with `panic = "abort"` in the workspace release profile;
  `Bela::run` now installs SIGINT/SIGTERM handlers that request a clean
  stop, mirroring the C example templates
- `bela`: safe `Context` accessors following Bela Gem semantics —
  frame/channel/sample-rate metadata, interleaved buffer slices, indexed
  audio/analog/digital I/O with bounds checking (Rust ports of the
  `Bela.h` inline helpers, including within-block persistence of
  `analog_write` / `digital_write` and the digital direction/value bit
  layout), plus the `map` and `constrain` utilities
- `bela`: safe wrapper core — the `unsafe` real-time trait
  `BelaApplication` (setup/render/cleanup), `extern "C"` trampolines
  bridging the C callbacks via `userData`, the `Settings` builder
  applying overrides on top of `Bela_defaultSettings()`, and the `Bela`
  RAII lifecycle (init/start/stop/cleanup, device target only behind
  the `bela_device` cfg)
- `bela-sys`: FFI bindings to the Bela core C API (`BelaContext`,
  `BelaInitSettings`, `Bela_*` lifecycle and auxiliary-task functions,
  `rt_printf`), generated with bindgen from vendored headers and
  committed so that builds need neither libclang nor a sysroot
- Vendored Bela headers pinned to the upstream `dev` branch
  (Gem-era API), with `scripts/update-vendor.sh` to move the pin
- `cargo xtask bindgen` task for regenerating the bindings
- Cargo workspace scaffolding with the `bela-sys` (raw FFI) and `bela`
  (safe wrapper) crates, targeting Bela Gem on PocketBeagle 2
  (`aarch64-unknown-linux-gnu`)
- CI running rustfmt, clippy and tests on the host, plus `cargo check`
  for `aarch64-unknown-linux-gnu`
- Dual MIT / Apache-2.0 licensing
- Documentation: cross-compilation setup draft and a board-facts
  template for on-device measurements

[Unreleased]: https://github.com/akiomik/bela-rs/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/akiomik/bela-rs/compare/v0.0.1...v0.1.0
[0.0.1]: https://github.com/akiomik/bela-rs/releases/tag/v0.0.1
