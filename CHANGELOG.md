# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `Board`, `DetectMode` and `Version`: what a program is running on.
  `Board::detect` wraps `Bela_detectHw` and takes the detect mode as an
  argument, because the modes differ in which of the two `belaconfig`
  files they trust and whether they fall back to scanning;
  `Version::running` wraps `Bela_getVersion` and `Version::HEADERS` is
  what the vendored headers said, so a binary can report both and name
  a board image that is not the one it was built against. Neither call
  needs an audio system, so a program that was only ever measured
  against one board can say so and decline before it brings one up. A
  board the vendored headers do not name is kept as
  `Board::Unrecognised` with the number libbela returned, rather than
  being read as a board this crate does know. Both types convert to
  their C spelling through `to_sys` and `From` — `BelaHw::from(board)`,
  `BelaHwDetectMode::from(mode)` — matching the `as_sys` already on the
  context types; the way back is `Board::from_sys` rather than a `From`
  impl, because `BelaHw` is an alias for `c_int` and the impl would
  claim that every `i32` is a board.
- `examples/board_info`, which prints the detected board and the Bela
  version and nothing else. It brings no audio system up and touches no
  audio hardware, so it answers on a board that is already doing
  something else — the first thing to ask for from anyone reporting a
  problem. With `--all-modes` it reports what each detect mode says
  instead, asking the scan last: it writes the file the other modes
  read, so going first would leave them reporting what that same run
  had just written.
- `examples/io_config`, a hardware probe for how a board configures its
  analog and digital I/O. It brings one audio system up per process
  and reports the `BelaContext` that `setup` and the first block see —
  the channel counts, frame counts and sample rates of all three
  domains — for a configuration given on its command line, alongside
  the board it detects, the version of the library that detected it and
  what `Bela_defaultSettings` says before any audio system exists.
  Nothing is wired to the board for it: it asks what shape the block
  is, which is what the accessors on the context types index against.
- `Debug` for `Bela` and for the four context types, which were the
  only public types without it. `Bela` prints whether audio is started
  and both callback fault counts, and no application type has to be
  `Debug` for it — nothing of the application is printed. A context
  prints the audio configuration its accessors report and not the
  buffers: a block is thousands of samples, and a context can be
  reached from a callback. `RenderContext` adds the three frame ranges
  it may write, which are what separate it from `BlockContext` and the
  first thing worth seeing from a `render` writing somewhere it did
  not mean to.
- `Default` for `PinMode`, which is `Input` — what a pin is until
  something says otherwise, as the type's documentation already said
  without the type being able to.
- `Hash` for `PinMode`, `Channel`, `ThreadInfo`, `Settings` and
  `Error`. `Board`, `DetectMode` and `Version` already had it, so this
  is every public type that can be a key rather than some of them —
  a run keyed by its settings, or faults counted per error, no longer
  depends on which type the crate happened to derive it for.
- `bela_midi_*` in `bela-sys`: raw bindings to a C surface over Bela's
  `Midi` class, which is C++ in `libbelaextra` and so out of reach of
  the generated bindings. The C is this crate's own (`shim/midi.cpp`,
  compiled by `build.rs`), because Bela's — `libraries/Midi/Midi_c.h` —
  covers input only, cannot report a port that failed to open, and
  enables the input parser after starting the thread that reads it.
  A port that does not exist is reported as `BELA_MIDI_NO_SUCH_PORT`
  and a second open of the same direction as `BELA_MIDI_ALREADY_OPEN`,
  both far outside the `errno` range so that neither collides with an
  ALSA failure passed through as `-errno`.
  Ports are listed with `bela_midi_list_ports`, which exists because
  Bela's port names are not the ones `amidi -l` prints: `hw:0,0,0`
  against `hw:0,0`, and only the first opens anything. Device builds
  therefore link `libbelaextra` as well as `libbela`, and cross builds
  need a C++ compiler from the same toolchain as the linker's — named
  by `BELA_CXX`, or derived from `BELA_CC`. A safe API is still to
  come; `docs/midi.md` records what the class does on the audio thread
  and what the safe API will be shaped by.

### Changed

- `Settings::new` is a `const fn`. All thirteen builder methods
  already were, so the only thing keeping a configuration out of a
  `const` was where it started: `const SETTINGS: Settings =
  Settings::new().period_size(64);` now compiles. `Settings::default`
  is the same value, and is written in terms of `new` rather than
  derived so that there is one place saying what empty means.

### Fixed

- The Bela Gem documentation on the context types and `Settings` said
  the analog outputs are part of the audio outputs, reachable as
  `audio_write` with the channel offset by +2. Measured on a Gem
  Stereo, that board has no analog outputs at all:
  `analog_out_channels()` is 0 for every channel count it accepts, and
  `audio_out_channels()` stays 2 rather than 2 plus them. The +2 offset
  belongs to a Gem Multi, which has the outputs, and is now documented
  as the unmeasured claim it is. `RenderContext` says the same as
  `BlockContext`, since what a board reports does not depend on which
  callback is asking.
- `Settings::num_analog_out_channels` did not say that libbela refuses
  a number different from `num_analog_in_channels`, which fails
  `Bela_initAudio` — and a failed initialisation costs the process its
  ability to build another audio system.
- `Settings::uniform_sample_rate` now says what it does rather than
  only when it is on: with it off the analog frame count follows the
  analog channel count instead of the audio block, measured at 8
  channels giving half the audio frames, 4 giving the same number and 2
  giving twice.
- The example on `Bela::set_line_out_level` could not compile: it named
  a `Context` type that does not exist and an `unsafe impl
  BelaApplication` without the `RenderState` the trait has carried
  since 0.3.0 replaced both. Nothing caught it, because a doc example
  on a device-gated item is compiled by neither of the two things that
  could — the host doc tests never see it, and the device documentation
  build only renders it. The example is gone rather than corrected: it
  repeated the one on `Bela::until_stopped`, which is where it now
  points, alongside `examples/levels.rs` — a whole program, and one CI
  does compile for the device.
- The three examples marked `ignore` — the crate's own, the one on
  `Board` and `Version`, and the `audio_out` loop on `RenderContext` —
  are compiled doc tests now. `ignore` compiles nothing at all, which
  is how the example above came to rot. The `RenderContext` one is
  checked in full. The crate's own no longer shows the `Bela::run` call
  inside the code block, so what remains of it is checked in full too.
  What is left unchecked is the three lines on `Board` and `Version`
  that need a board: they sit behind a `bela_device` cfg, and unlike
  the files in `examples/`, which CI compiles for the device, no job
  compiles a doc test for it — running one means linking, so it needs a
  cross-linker and a sysroot CI does not have.

## [0.3.0] - 2026-08-06

### Added

- Multithreaded rendering. `Settings::thread_count` above 1 is served
  again, and a Bela Gem's four cores can be used for one block. Every
  application is written for it, whatever the thread count: `render`
  takes `&self` and one `RenderState` of its own, and the whole of what
  changes with a second thread is that there are two states and two
  frame ranges instead of one.
  The crate partitions the block, which Bela does not: it hands every
  thread the same buffers and leaves the splitting to the application.
  `RenderContext` reads the whole block but writes only
  `audio_frame_range()` and its analog and digital counterparts —
  contiguous ranges that tile the block exactly across the threads — so
  two threads cannot reach the same output sample. The digital words
  are bounded on the way in as well, since they carry the outputs too.
  None of it is trusted to libbela. A callback that arrives where the
  references it needs cannot be handed out safely — a second `render`
  on one thread number, a `render_post` overlapping a `render`, which a
  stop requested mid-block can produce — is refused, and the audio
  system asked to stop, without any user code running.
  `Bela::callback_faults` counts those refusals, and
  `Bela::until_stopped` — with the `run` methods built on it — fails
  with the new `Error::CallbackFaults` rather than reporting `Ok(())`
  for a run the crate itself asked to stop.
  A refusal during the shutdown is counted apart from one during a live
  run and does not fail anything: libbela abandons the block it is in
  when a stop arrives, which can leave a `render_post` overlapping a
  `render` still finishing, and refusing that is the guard working
  rather than a symptom. Those are reported on the console and counted
  by `Bela::callback_faults_while_stopping`, and mean the last block of
  a multithreaded run may be short — see
  `docs/multithreaded-rendering.md`.
- `examples/parallel`, which splits a bank of 192 sine oscillators
  across the render threads and measures that the work was divided
  rather than duplicated: per-thread frame counts that account for
  every frame exactly once, and a Linux thread id and core for each.
  Measured on a Bela Gem, the busiest thread's share of the block falls
  from 41.4% on one thread to 10.6% on four; see
  `docs/multithreaded-rendering.md` for the whole table, for why the
  audio thread's own figure falls by less, and for the one thing the
  example turned up that reading libbela's sources had only suggested —
  a stop requested mid-block can leave the final block partly
  unrendered.
- `examples/init_failure`, a hardware probe for what a failed
  `Bela_initAudio` leaves behind. It runs one question per process —
  fail an initialisation and stop; fail one and try another; fail one
  and call `Bela_cleanupAudio`, both from outside the crate and through
  the C API in the order a fix would use; build and drop several audio
  systems in a row — so that the answers can be attributed to the run
  that produced them.

### Changed

- Breaking: `BelaApplication` is a different trait. It is safe to
  implement rather than `unsafe`, it carries a `RenderState` associated
  type, `render` takes `&self` and one state, and there are two new
  callbacks — `create_render_state`, which builds one state per render
  thread before audio starts, and `render_pre` / `render_post`, which
  bracket the parallel section on the main audio thread with the whole
  block and every state to themselves. Implementors must be `Sync` as
  well as `Send`.
  Every existing application has to be rewritten: what `render` used to
  mutate through `&mut self` moves into the `RenderState`, and work
  that is one thing per block rather than per thread — an oscillator's
  phase, a block counter — moves into `render_pre` / `render_post`.
  `examples/sine` shows the first and `examples/print` the second.
  The trait is no longer `unsafe` because the invariants that had to
  hold are now enforced by the crate rather than promised by the
  implementor. The real-time rules — no allocating, no blocking, no
  panicking on the audio thread — are unchanged and just as important,
  but breaking them costs dropouts rather than undefined behaviour, and
  that is not what `unsafe` is for.
- Breaking: `Context` is replaced by four types, one per callback
  phase, because the phases do not have the same rights over the block.
  `SetupContext` and `CleanupContext` describe the audio configuration
  and expose no buffers, since there is no block in flight when they
  run. `BlockContext` is the whole block, for `render_pre` and
  `render_post`. `RenderContext` is what `render` gets: the whole block
  to read, this thread's frames to write, and no `as_sys_mut` — the way
  back to the whole output buffer is the aliasing being avoided.
  `cpu_usage` is on the first three and not on `RenderContext`: above
  one render thread, `render` runs on threads other than the one
  libbela's counters belong to, and reading them from there would be
  the data race the accessor exists to prevent. Read it in `render_pre`
  or `render_post` and hand the number on, as `examples/cpu` does.
  `this_thread` and `thread_count` return `usize` rather than `u32`,
  like the other counts, and `thread_count` reports the number of
  threads that actually render, so libbela's two spellings of one
  thread — 0 and 1 — both come back as 1.
- Breaking: `AuxiliaryTask` is `Sync`. An application is shared across
  the render threads, so a handle it holds is reachable from all of
  them, and `Bela_scheduleAuxiliaryTask` serialises on the task's own
  mutex. `schedule` takes any of the four contexts, through the new
  `CallbackContext` trait, rather than a `Context`.
- Breaking: `Bela::new` refuses every attempt made after one of its own
  has failed, returning the new `Error::AudioSystemPoisoned` instead of
  trying. A `Bela_initAudio` that fails partway through leaves libbela
  believing an audio system is up and offers no call that puts it back —
  `Bela_cleanupAudio` segfaults on that path — so the second attempt was
  never going to work: on a board it segfaulted inside libbela with
  `Mcasp::start() called while already running` rather than returning
  anything. Where a program used to crash it now gets an error, and one
  that retried after `Error::Init` and happened to survive is now
  refused. Only the process is affected, and the board is untouched, so
  a new process gets a working audio system straight away.
  `Bela::new`, `Bela::run`, `BelaApplication::setup` and `Error::Init`
  now say all of this: `Error::Init` is a reason to exit, and a `setup`
  callback returning `false` — the reachable way to get there, since it
  fails the initialisation after the hardware is up — is for ending a
  program rather than for trying different settings.

### Removed

- Breaking: `Error::ThreadCountUnsupported`, which 0.2.0 added to
  refuse a `Settings::thread_count` above 1. There is nothing left to
  refuse: every positive thread count goes through the same
  `Bela::new`.

## [0.2.0] - 2026-08-05

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

[Unreleased]: https://github.com/akiomik/bela-rs/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/akiomik/bela-rs/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/akiomik/bela-rs/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/akiomik/bela-rs/compare/v0.0.1...v0.1.0
[0.0.1]: https://github.com/akiomik/bela-rs/releases/tag/v0.0.1
