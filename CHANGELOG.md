# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- The codec's levels and gain in the `bela` crate, wrapping
  `Bela_setLineOutLevel`, `Bela_setHpLevel`, `Bela_setAudioInputGain`
  and `Bela_muteSpeakers`. `Bela::set_line_out_level`,
  `Bela::set_headphone_level`, `Bela::set_audio_input_gain` and
  `Bela::mute_speakers` take a `Channel` — one channel or all of them —
  and report what the codec made of the call. They are the analogue
  volume controls, so they change what the hardware does with the
  signal without `render` knowing anything about it: attenuating the
  line out, or turning the preamplifier ahead of the ADC up for a quiet
  source rather than scaling in software, which would only amplify the
  noise the ADC already digitised. The deprecated halves of the C API
  (`Bela_setDacLevel`, `Bela_setAdcLevel`, `Bela_setPgaGain`,
  `Bela_setHeadphoneLevel` and their all-channel spellings) are not
  wrapped; each is a call to one of the four above.
  They live on the `Bela` handle rather than in `Settings`, which the
  `BelaChannelGainArray` fields of `BelaInitSettings` might suggest.
  Measured on the board: libbela applies those arrays by calling
  exactly these functions from inside `Bela_initAudio`, and the codec
  only writes its registers once audio starts — so a call between
  `Bela::new` and `Bela::start` reaches the hardware in the same state
  and at the same moment, while a settings-time copy would only add
  storage and a second way to say it. Being on the handle also keeps
  them away from `render`, where an I²C write has no place.
  A level has to be a finite number of decibels of at most
  `MAX_DECIBELS` in magnitude, or the call fails with
  `Error::Decibels` before reaching libbela: libbela converts decibels
  into register values with a C cast to `int`, which is undefined
  behaviour for a NaN or a value that does not fit, and every clamp on
  the C side is a comparison a NaN slips through. That limit is far
  outside any codec's range — what the codec cannot do it clamps, as
  before. See `examples/levels.rs`
- `Bela::until_stopped`, which was the second half of `Bela::run` and
  is now public: it starts the audio system, blocks until a stop is
  requested and shuts down, so a program that has something to say
  between `Bela::new` and the run loop — setting a level, above all —
  no longer has to reimplement the loop and its signal handling to get
  that window
- `Settings::begin_muted`, wrapping the `beginMuted` init setting: the
  one level control that cannot be a call, since `Bela_startAudio`
  unmutes the speaker amplifiers unless it was asked not to. A Bela Gem
  Stereo has no amplifier mute pin (measured: `ampMutePin` is -1), so
  neither this nor `Bela::mute_speakers` has any effect there; both are
  wrapped for the Bela hardware that does have one, and say so
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
  from an auxiliary task through atomics — the pattern to copy
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

### Changed

- Breaking: `Bela::new` refuses a `Settings::thread_count` above 1 with
  the new `Error::ThreadCountUnsupported`, where 0.1.0 accepted it and
  initialised the audio system. Multithreaded rendering is the one
  thing 0.1.0 offered that this release does not, and it is why the
  version goes to 0.2.0. It could not stay: Bela calls `render` on all
  render threads at once with the same user data, so the trampoline
  handed out several `&mut T` to one application — reachable from safe
  code, and undefined behaviour whatever the render function then did
  with them. A trait shaped for concurrent rendering is still to be
  designed; until it exists, `thread_count(1)` and leaving it unset are
  the configurations this crate serves

### Fixed

- `rt_print!` and `rt_println!` stop formatting as soon as the message
  fills the buffer, instead of running the formatting machinery to the
  end and discarding what no longer fits. Padding is written one
  `char` at a time, so `rt_println!("{:width$}", "", width = 65_535)`
  was 65_535 calls into the writer on the audio thread — bounded
  memory, but not bounded time, and enough to miss the render
  deadline. The output is unchanged: the message is still truncated on
  a `char` boundary and marked with `...`

## [0.1.0] - 2026-08-05

### Added

- `bela-sys`'s `build.rs` emits the `libbela` link flags for device
  targets, so device binaries link and run. Cross-linking is driven by
  `BELA_SYSROOT` together with `scripts/sync-sysroot.sh` and the linker
  wrapper in `scripts/aarch64-bela-linker.sh`
- `Context::this_thread` / `thread_count` and `Settings::thread_count`
  for the multithreaded rendering added in Bela 1.15
- `Bela::run` also handles SIGHUP, so a dropped ssh connection shuts
  the audio system down cleanly instead of killing the process outright

### Changed

- The bindings are regenerated from the headers shipped on a Bela Gem
  (Bela 1.18.0), which is newer than any published upstream branch.
  `BelaContext` and `BelaInitSettings` gained fields, and the bindings
  now cover `Bela_initRtBackend`, `Bela_clock_gettime` and friends

## [0.0.1] - 2026-07-27

### Added

- `passthrough` and `sine` examples written against the safe API only,
  with `panic = "abort"` in the workspace release profile; `Bela::run`
  now installs SIGINT/SIGTERM handlers that request a clean stop,
  mirroring the C example templates
- Safe `Context` accessors following Bela Gem semantics —
  frame/channel/sample-rate metadata, interleaved buffer slices, indexed
  audio/analog/digital I/O with bounds checking (Rust ports of the
  `Bela.h` inline helpers, including within-block persistence of
  `analog_write` / `digital_write` and the digital direction/value bit
  layout), plus the `map` and `constrain` utilities
- The safe wrapper core — the `unsafe` real-time trait
  `BelaApplication` (setup/render/cleanup), `extern "C"` trampolines
  bridging the C callbacks via `userData`, the `Settings` builder
  applying overrides on top of `Bela_defaultSettings()`, and the `Bela`
  RAII lifecycle (init/start/stop/cleanup, device target only behind
  the `bela_device` cfg)
- FFI bindings to the Bela core C API (`BelaContext`,
  `BelaInitSettings`, `Bela_*` lifecycle and auxiliary-task functions,
  `rt_printf`), generated with bindgen from vendored headers pinned to
  the upstream `dev` branch (the Gem-era API) and committed, so that
  builds need neither libclang nor a sysroot
- Cargo workspace scaffolding with the `bela-sys` (raw FFI) and `bela`
  (safe wrapper) crates, targeting Bela Gem on PocketBeagle 2
  (`aarch64-unknown-linux-gnu`). Linking against `libbela` is not
  wired up yet, so the crates compile for host and device but cannot
  produce a runnable device binary
- Dual MIT / Apache-2.0 licensing
- A draft of the cross-compilation setup in `docs/cross-compile.md`

[Unreleased]: https://github.com/akiomik/bela-rs/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/akiomik/bela-rs/compare/v0.0.1...v0.1.0
[0.0.1]: https://github.com/akiomik/bela-rs/releases/tag/v0.0.1
