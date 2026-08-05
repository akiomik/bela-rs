# bela-rs

[![CI](https://github.com/akiomik/bela-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/akiomik/bela-rs/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/akiomik/bela-rs/branch/main/graph/badge.svg)](https://codecov.io/gh/akiomik/bela-rs)
![Crates.io Version](https://img.shields.io/crates/v/bela-sys?label=bela-sys)
![Crates.io Version](https://img.shields.io/crates/v/bela?label=bela)

Rust bindings for the [Bela](https://bela.io) core API, targeting
**Bela Gem** on PocketBeagle 2 (`aarch64-unknown-linux-gnu`).

> **Status: works on hardware, API not yet settled.** The examples
> cross-build on a host and produce sound on a Bela Gem Stereo. The
> scope is still the core audio API — the C++ libraries and most of
> the auxiliary-task surface are not wrapped yet, and the API may
> change in any 0.x release.

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

## Quick start

Implement `BelaApplication` and hand it to `Bela::run`. `render` must
be real-time safe: no allocation, blocking, system calls or panics.

```rust
use bela::{Bela, BelaApplication, RenderContext, Settings, SetupContext, ThreadInfo};

struct Passthrough;

impl BelaApplication for Passthrough {
    // Nothing to carry from one block to the next.
    type RenderState = ();

    fn create_render_state(&mut self, _thread: ThreadInfo, _context: &SetupContext) {}

    fn render(&self, _state: &mut (), context: &mut RenderContext) {
        let channels = context
            .audio_in_channels()
            .min(context.audio_out_channels());
        // This thread's share of the block; with one render thread,
        // all of it.
        for frame in context.audio_frame_range() {
            for channel in 0..channels {
                let sample = context.audio_read(frame, channel);
                context.audio_write(frame, channel, sample);
            }
        }
    }
}

fn main() -> Result<(), bela::Error> {
    Bela::run(Passthrough, &Settings::new())
}
```

The shape — an application shared as `&self`, one `RenderState` per
render thread, a context that writes only this thread's frames — is
what lets `Settings::thread_count` use all four of a Bela Gem's cores
for one block. It is the same code either way; see
[Multithreaded rendering](docs/multithreaded-rendering.md).

Building requires a sysroot synced from the board; see
[docs/cross-compile.md](docs/cross-compile.md) for the one-time setup.

```sh
export BELA_SYSROOT="$PWD/bela-sysroot"
cargo build --release --target aarch64-unknown-linux-gnu
scp target/aarch64-unknown-linux-gnu/release/my-app root@bela.local:
ssh -t root@bela.local 'systemctl stop bela_daemon && ./my-app'
```

Set `panic = "abort"` in the release profile: a panic crossing the
audio callback boundary aborts the process either way.

## Documentation

- [Cross-compilation setup](docs/cross-compile.md)
- [Board facts](docs/board-facts.md) — measured values from the actual board
- [Connecting the board over Ethernet](docs/board-network.md) — USB
  Ethernet adapter setup, and why it is not a transfer speedup
- [Multithreaded rendering](docs/multithreaded-rendering.md) — what
  `threadCount` does on the board, how the safe API divides a block
  across the render threads, and what it measurably buys
- [Release procedure](docs/release.md)
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
