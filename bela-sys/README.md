# bela-sys

Raw FFI bindings to the Bela core API (`libbela`) for Bela Gem on
PocketBeagle 2 (`aarch64-unknown-linux-gnu`).

## Vendored headers

`vendor/bela/` contains `Bela.h`, `GPIOcontrol.h` and `Utilities.h`
(the include closure of `wrapper.h`) plus the upstream `LICENSE`. The
provenance is recorded in `vendor/bela/SOURCE`. These files are LGPL
3.0 (see `vendor/bela/LICENSE`); the rest of the crate is MIT OR
Apache-2.0.

The headers are taken **from the board**, not from
[BelaPlatform/Bela]: the Bela Gem image ships Bela 1.18.0, which is
newer than any published branch (see
[docs/board-facts.md](../docs/board-facts.md)).

```sh
scripts/update-vendor.sh --board          # from root@bela.local
scripts/update-vendor.sh <branch|commit>  # from upstream git
```

Because the pin follows a board image rather than a released version,
a new image moves the headers on the board without changing anything
here. After updating one, compare the two:

```sh
cargo xtask check-vendor --board          # against root@bela.local
```

It reports the `BELA_*_VERSION` macros on both sides and diffs every
vendored file against the board's copy, exiting non-zero on drift —
which is the signal to re-run the update script and regenerate the
bindings below. It needs a board, so CI cannot run it.

### Why vendored files instead of a git submodule

- The include closure is three files (~70 KB); a submodule would drag
  in the whole upstream repository (IDE, examples, PRU firmware,
  history) for every clone and CI run.
- `src/bindings.rs` is committed, and vendoring keeps "these headers"
  and "the bindings generated from them" atomic in one commit — a
  submodule can drift ahead of the generated code, and its bumps show
  up as opaque hash changes instead of reviewable header diffs.
- The pin tracks the exact Bela version shipped on the board, which
  does not correspond to any published upstream commit. File copies
  can come from anywhere; a submodule can only point at upstream
  commits.
- A plain `git clone` always builds — no `--recursive`, no submodule
  initialisation failure modes.

The trade-off is that provenance rests on `scripts/update-vendor.sh`
recording the source in `vendor/bela/SOURCE`, rather than on git
itself.

[BelaPlatform/Bela]: https://github.com/BelaPlatform/Bela

## Regenerating the bindings

`src/bindings.rs` is generated but **committed**, so building this
crate requires neither libclang nor an aarch64 sysroot. Regenerate it
after updating the vendored headers:

```sh
cargo xtask bindgen --sysroot <dir>   # or set BELA_SYSROOT
```

The sysroot is the one synced from the board (see
[docs/cross-compile.md](../docs/cross-compile.md)); bindgen needs it
for the libc headers `Bela.h` includes.

## Linking

`build.rs` emits the link flags for `libbela` on device targets:
the library search paths from `docs/board-facts.md` (prefixed with
`BELA_SYSROOT` when cross-compiling) and `-lbela` plus the C++ runtime
and transitive dependencies (`seasocks`, `evl`, `stdc++`) that Rust
does not link on its own.

On non-device targets it emits nothing, so host builds and `cargo
check`/`clippy` for the target work without a sysroot.
