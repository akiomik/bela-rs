# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

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

[Unreleased]: https://github.com/akiomik/bela-rs/commits/main
