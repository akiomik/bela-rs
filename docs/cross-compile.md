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

The transfer takes whichever path `bela.local` resolves to, the USB
gadget network by default. Attaching a USB Ethernet adapter is an
option — see [board-network.md](board-network.md) — but it is not a
speedup: the board is USB 2.0 on either path.

## 4. Build, deploy and run

Set `BELA_SYSROOT` when building; `bela-sys/build.rs` derives the
library search paths from it, and the linker wrapper uses it too.

```sh
export BELA_SYSROOT="$PWD/bela-sysroot"
cargo build -p bela --release --target aarch64-unknown-linux-gnu --example sine
scp target/aarch64-unknown-linux-gnu/release/examples/sine root@bela.local:
ssh -t root@bela.local 'systemctl stop bela_daemon && ./sine'
```

Rust binaries cannot be built by the Bela IDE, hence the scp + ssh
workflow. `bela_daemon` runs the IDE's own program and holds the audio
hardware, so stop it first; start it again with
`systemctl start bela_daemon`.

**Use `ssh -t`.** Without it ssh allocates no terminal, so Ctrl-C only
kills the local ssh client while the program keeps running on the
board — you then have to log in and `pkill -f ./sine` to stop it. With
`-t`, Ctrl-C reaches the program and `Bela::run` shuts down cleanly
(it handles SIGINT, SIGTERM and SIGHUP, so `systemctl stop` and a
dropped connection are clean too).

Note that Bela renames the process, so `pgrep -x sine` does not match
it; use `pgrep -f './sine'`.

## Updating the vendored headers

The Bela Gem image ships a Bela version that is not published upstream
(see [board-facts.md](board-facts.md)), so pin the headers to the
board:

```sh
scripts/update-vendor.sh --board
cargo xtask bindgen --sysroot "$BELA_SYSROOT"
```

Nothing in the build notices when the board moves ahead of the pin:
`bela-sys/src/bindings.rs` is committed, so it keeps describing the
ABI of the vendored headers while the `libbela` it links against is a
different one — which can shift `BelaContext` field offsets underneath
running code. After updating the board image, ask:

```sh
cargo xtask check-vendor --board   # or --board user@host
```

It compares every vendored file with the board's copy, prints the
`BELA_*_VERSION` macros on both sides and a diff of whatever differs,
and exits non-zero on drift. It needs a board, so CI cannot run it.
