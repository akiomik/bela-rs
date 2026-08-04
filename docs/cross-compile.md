# Cross-compilation setup

How to build for Bela Gem (`aarch64-unknown-linux-gnu`) from a host
machine (instructions assume macOS). Verified end to end: examples
cross-built this way run on the board.

## 1. Rust target

`rust-toolchain.toml` declares the `aarch64-unknown-linux-gnu` target,
so under rustup it is installed automatically the first time cargo runs
inside the repository.

Non-linking checks need nothing else installed:

```sh
cargo check --workspace --all-targets --target aarch64-unknown-linux-gnu
```

## 2. Cross-linker

From the [messense/macos-cross-toolchains] tap:

```sh
brew tap messense/macos-cross-toolchains
brew trust messense/macos-cross-toolchains
brew install aarch64-unknown-linux-gnu
```

`.cargo/config.toml` points the linker at
[`scripts/aarch64-bela-linker.sh`](../scripts/aarch64-bela-linker.sh),
a wrapper that adds the sysroot-specific flags Debian needs (see the
comments in the script for why each is required).

[messense/macos-cross-toolchains]: https://github.com/messense/macos-cross-toolchains

## 3. Sysroot

Linking needs a copy of the board's filesystem — `libbela` plus the EVL
runtime and the Debian libraries they depend on:

```sh
scripts/sync-sysroot.sh            # ./bela-sysroot from root@bela.local
```

The result is about 850 MB and is git-ignored. Re-run it after
updating the board image.

## 4. Build, deploy and run

Set `BELA_SYSROOT` when building; `bela-sys/build.rs` derives the
library search paths from it, and the linker wrapper uses it too.

```sh
export BELA_SYSROOT="$PWD/bela-sysroot"
cargo build -p bela --release --target aarch64-unknown-linux-gnu --example sine
scp target/aarch64-unknown-linux-gnu/release/examples/sine root@bela.local:
ssh root@bela.local 'systemctl stop bela_daemon && ./sine'
```

Rust binaries cannot be built by the Bela IDE, hence the scp + ssh
workflow. `bela_daemon` runs the IDE's own program and holds the audio
hardware, so stop it first; start it again with
`systemctl start bela_daemon`.

Press Ctrl-C (or send SIGTERM) to stop: `Bela::run` installs handlers
that request a clean shutdown.

## Updating the vendored headers

The Bela Gem image ships a Bela version that is not published upstream
(see [board-facts.md](board-facts.md)), so pin the headers to the
board:

```sh
scripts/update-vendor.sh --board
cargo xtask bindgen --sysroot "$BELA_SYSROOT"
```
