# Cross-compilation setup

How to build for Bela Gem (`aarch64-unknown-linux-gnu`) from a host
machine (instructions assume macOS).

> **Status: draft.** The board has not arrived yet; everything from the
> sysroot section onwards is unverified. This document is updated as
> steps are confirmed on real hardware.

## 1. Rust target

`rust-toolchain.toml` declares the `aarch64-unknown-linux-gnu` target,
so under rustup it is installed automatically the first time cargo runs
inside the repository.

Non-linking checks work with nothing else installed:

```sh
cargo check --workspace --all-targets --target aarch64-unknown-linux-gnu
```

## 2. Cross-linker (macOS)

Needed once binaries are actually linked. On macOS,
[messense/homebrew-macos-cross-toolchains](https://github.com/messense/homebrew-macos-cross-toolchains)
is the easiest route:

```sh
brew tap messense/macos-cross-toolchains
brew install aarch64-unknown-linux-gnu
```

Then uncomment the `linker` setting in `.cargo/config.toml`.

## 3. Sysroot (requires the board)

Sync `libbela`, the Xenomai wrapper libraries and their headers from
the board (the equivalent of `SyncBelaSysroot` in Bela's official
cross-build environment):

```sh
# Example, assuming the board is reachable as bela.local
rsync -avz --delete \
  --include-from=<list of required paths> \
  root@bela.local:/ ./bela-sysroot/
```

The paths to sync (`/usr/lib`, `/usr/include`, `/root/Bela`, ...) are
collected during the board fact-finding phase and recorded in
`board-facts.md` before this list is finalised.

Configuration points:

- `rustflags` with `--sysroot` in `.cargo/config.toml`
- `BINDGEN_EXTRA_CLANG_ARGS_aarch64_unknown_linux_gnu` with `--sysroot`

## 4. Deploy and run (requires the board)

Rust binaries cannot be built by the Bela IDE, so the workflow is
scp + ssh:

```sh
cargo build -p bela --release --target aarch64-unknown-linux-gnu --example sine
scp target/aarch64-unknown-linux-gnu/release/examples/sine root@bela.local:
ssh root@bela.local ./sine
```

How to stop the Bela IDE's default program (`systemctl stop bela` or
similar) will be confirmed on the board and documented here.
