# bela-sys

Raw FFI bindings to the Bela core API (`libbela`) for Bela Gem on
PocketBeagle 2 (`aarch64-unknown-linux-gnu`).

## Vendored headers

`vendor/bela/` contains `Bela.h`, `GPIOcontrol.h` and `Utilities.h`
(the include closure of `wrapper.h`) plus the upstream `LICENSE`,
copied from [BelaPlatform/Bela] and pinned to the commit recorded in
`vendor/bela/COMMIT`. The pin currently tracks the `dev` branch, which
carries the Bela Gem-era API (e.g. `BelaInitSettings.uniformSampleRate`).
These files are LGPL 3.0 (see `vendor/bela/LICENSE`); the rest of the
crate is MIT OR Apache-2.0.

To move the pin (e.g. to the exact version shipped on the board):

```sh
scripts/update-vendor.sh <branch|tag|commit>
```

### Why vendored files instead of a git submodule

- The include closure is three files (~70 KB); a submodule would drag
  in the whole upstream repository (IDE, examples, PRU firmware,
  history) for every clone and CI run.
- `src/bindings.rs` is committed, and vendoring keeps "these headers"
  and "the bindings generated from them" atomic in one commit — a
  submodule can drift ahead of the generated code, and its bumps show
  up as opaque hash changes instead of reviewable header diffs.
- The pin will eventually move to the exact Bela version shipped on
  the board, which may not correspond to any published upstream
  commit (e.g. headers synced from the device image). File copies can
  come from anywhere; a submodule can only point at upstream commits.
- A plain `git clone` always builds — no `--recursive`, no submodule
  initialisation failure modes.

The trade-off is that provenance rests on `scripts/update-vendor.sh`
recording the resolved commit in `vendor/bela/COMMIT`, rather than on
git itself.

[BelaPlatform/Bela]: https://github.com/BelaPlatform/Bela

## Regenerating the bindings

`src/bindings.rs` is generated but **committed**, so building this
crate requires neither libclang nor an aarch64 sysroot. Regenerate it
after updating the vendored headers:

```sh
cargo xtask bindgen --sysroot <dir>   # or set BELA_SYSROOT
```

The sysroot only needs aarch64-linux libc headers. Until the board
arrives, they can be extracted from a Debian arm64 image:

```sh
docker run --platform linux/arm64 --name bela-hdrs debian:bookworm-slim \
  sh -c 'apt-get update -qq && apt-get install -y -qq --no-install-recommends libc6-dev linux-libc-dev'
mkdir -p aarch64-sysroot/usr
docker cp bela-hdrs:/usr/include aarch64-sysroot/usr/include
docker rm bela-hdrs
cargo xtask bindgen --sysroot aarch64-sysroot
```

Once the board is available, use the real sysroot synced from it
instead (see `docs/cross-compile.md`).

## Linking

Not wired up yet: `build.rs` will emit the `libbela` link flags once
they have been collected on the board (see `docs/board-facts.md`).
Until then, only non-linking builds (`cargo check`, `cargo test` on the
host) are supported.
