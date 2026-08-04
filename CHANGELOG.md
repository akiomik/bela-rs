# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/akiomik/bela-rs/compare/v0.0.1...HEAD
[0.0.1]: https://github.com/akiomik/bela-rs/releases/tag/v0.0.1
