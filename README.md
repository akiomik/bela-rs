# bela-rs

Rust bindings for the [Bela](https://bela.io) core API, targeting
**Bela Gem** on PocketBeagle 2 (`aarch64-unknown-linux-gnu`).

> **Status: early development.** The hardware has not arrived yet;
> nothing here has run on a board. Linking against `libbela` is not
> wired up, so the published 0.0.x crates compile but cannot produce a
> runnable device binary yet. Version 0.1.0 is reserved for the first
> hardware-validated release:
> [milestone v0.1.0 — sound on Bela Gem](https://github.com/akiomik/bela-rs/milestone/1).

## Crates

| Crate | Description |
|-------|-------------|
| [`bela-sys`](bela-sys) | Raw FFI bindings to `libbela` (bindgen, C core API only) |
| [`bela`](bela) | Safe API: settings builder, real-time render trait, RAII lifecycle |

The scope is intentionally the C core API (`BelaContext`,
`setup`/`render`/`cleanup`, `Bela_initAudio`/`Bela_startAudio`/...).
The C++ libraries (Scope, Trill, Fft, Gui, Midi) are out of scope for
now and may be added incrementally.

The integration model is the officially supported one: a standalone
binary that defines the render callbacks and links `libbela`,
cross-compiled on a host machine and copied to the board.

## Documentation

- [Cross-compilation setup](docs/cross-compile.md)
- [Board facts](docs/board-facts.md) — measured values from the actual board
- [Changelog](CHANGELOG.md) / [Contributing](CONTRIBUTING.md)

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or
  <http://opensource.org/licenses/MIT>)

at your option.

Note: the Bela core software that final binaries link against is
licensed under the LGPL 3.0. That obligation applies to the linked
binary, not to these crates.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the
Apache-2.0 license, shall be dual licensed as above, without any
additional terms or conditions.
