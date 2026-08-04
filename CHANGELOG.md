# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `AuxiliaryTask` in the `bela` crate: a safe wrapper over
  `Bela_createAuxiliaryTask` / `Bela_scheduleAuxiliaryTask`, so work
  that must not happen in `render` (I/O, allocation, long
  calculations) can be moved to a lower-priority thread that `render`
  triggers with a real-time safe `schedule()`. The callback is a
  `'static` closure that owns its state — it cannot borrow from the
  application, which the audio thread holds by `&mut` while the task
  runs — and shares with `render` through atomics or a lock-free
  queue. `AUDIO_PRIORITY` is exposed to pick a priority below the
  audio thread. Handles stop working once the audio system is stopped,
  since that deletes every task; scheduling from `cleanup` would
  otherwise have been a use-after-free reachable from safe code.
  Measured on the board: a request that arrives while the callback is
  still running is silently lost, and the C return value does not
  report it, so `schedule()` returns nothing and the documentation
  says to count invocations in the callback when it matters
- `bela/examples/aux_task.rs`, reporting from a task scheduled once a
  second by `render`, including work that allocates

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
