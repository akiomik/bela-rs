# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Cargo workspace scaffolding with the `bela-sys` (raw FFI) and `bela-rs`
  (safe wrapper) crates, targeting Bela Gem on PocketBeagle 2
  (`aarch64-unknown-linux-gnu`)
- CI running rustfmt, clippy and tests on the host, plus `cargo check`
  for `aarch64-unknown-linux-gnu`
- Dual MIT / Apache-2.0 licensing
- Documentation: feasibility study and roadmap, cross-compilation setup
  draft, and a board-facts template for on-device measurements

[Unreleased]: https://github.com/akiomik/bela-rs/commits/main
