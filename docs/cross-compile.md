# Cross-compilation setup

How to build for Bela Gem (`aarch64-unknown-linux-gnu`) from a host
machine. The macOS path is verified end to end: examples cross-built
that way run on the board. Cross-building from an x86_64 Linux host is
not — it differs only in which compiler the linker wrapper calls, but
nothing built that way has been run on hardware. An arm64 Linux host
differs by more than the compiler, and is untried as well.

## 1. Rust target

`rust-toolchain.toml` declares the `aarch64-unknown-linux-gnu` target,
so under rustup it is installed automatically the first time cargo runs
inside the repository.

Non-linking checks need nothing else installed:

```sh
cargo check --workspace --all-targets --target aarch64-unknown-linux-gnu
```

## 2. Linker wrapper

`.cargo/config.toml` points the linker at
[`scripts/aarch64-bela-linker.sh`](../scripts/aarch64-bela-linker.sh),
a wrapper that adds the sysroot-specific flags Debian needs (see the
comments in the script for why each is required). The wrapper calls a
compiler, which has to be installed, and `BELA_CC` says which one. A
cross toolchain is named after the triple it was built for, so the name
depends on where the toolchain came from.

On macOS, from the [messense/macos-cross-toolchains] tap:

```sh
brew tap messense/macos-cross-toolchains
brew trust messense/macos-cross-toolchains
brew install aarch64-unknown-linux-gnu
```

That one is the wrapper's default, so `BELA_CC` can stay unset.

On Debian or Ubuntu, from the distribution:

```sh
sudo apt install gcc-aarch64-linux-gnu
export BELA_CC=aarch64-linux-gnu-gcc
```

Any other aarch64 Linux compiler is used the same way: install it and
name it in `BELA_CC`. The value is a name to find on `PATH` or an
absolute path — the wrapper runs it as a program, so it cannot carry
arguments or a prefix command like `ccache`.

A **C++** compiler from the same toolchain is needed as well, because
`bela-sys` compiles a small shim over Bela's `Midi` class (see
[midi.md](midi.md)). `bela-sys/build.rs` picks it in this order:

1. `BELA_CXX`, if set.
2. Otherwise a name derived from `BELA_CC`, if that is set and ends in
   `gcc`: `aarch64-linux-gnu-gcc` gives `aarch64-linux-gnu-g++`,
   `/usr/bin/gcc` gives `/usr/bin/g++`, plain `gcc` gives `g++`.
3. Otherwise — `BELA_CC` unset as well — the tap's
   `aarch64-unknown-linux-gnu-g++`.

A `BELA_CC` that is set and does not end in `gcc` **fails the build**
rather than falling back to the default, because the fallback would
compile the shim with a toolchain the linker is not using. `BELA_CXX`
is the answer for those:

```sh
export BELA_CXX=aarch64-linux-gnu-g++
```

`BELA_CXX` and `BELA_CC` are what this reads, and `CXX` and
`CXX_aarch64-unknown-linux-gnu` — which `cc` would otherwise honour —
are not consulted: the compiler has to match the one the linker
wrapper calls, and that wrapper knows only `BELA_CC`. The archiver
follows the compiler's name (`aarch64-linux-gnu-g++` implies
`aarch64-linux-gnu-ar`), except for names nothing follows from, such as
a `clang++`; `AR` and `AR_aarch64-unknown-linux-gnu` are read first
either way, so a toolchain that needs a different one can say so.

Both have to come from the same toolchain. The shim allocates a class
whose methods live in `libbelaextra.so`, so it has to agree with it
about layout — measured equal between the tap's g++ 15.2.0 and the
board's own `clang++`, and recorded in
[board-facts.md](board-facts.md). Compiling with one toolchain and
linking with another is also how a binary ends up asking the board for
`libstdc++` or `libgcc_s` symbols it does not have, which is a failure
that waits until the program runs.

Not every build that comes through the wrapper is a cross build. It is
attached to the target in `.cargo/config.toml`, not to cross-compiling,
so building on the board itself goes through it as well — with no
sysroot to add, and the board's own compiler to call:

```sh
export BELA_CC=gcc
```

An arm64 Linux host, building for the board with a sysroot, is the case
nobody here has tried. Host and target triple are equal there, and
whether `[target.<triple>]` also governs what cargo compiles to run on
the host is `target-applies-to-host`, which is on and can only be
turned off on nightly — so the board's sysroot may end up linked into
programs meant to run on the host machine. Expect to find that out by a
build script failing to run.

What the wrapper asks of a toolchain is that it targets aarch64 Linux
and honours `--sysroot`. The C library, its headers and the startup
files the `-B` covers (`Scrt1.o`, `crti.o`, `crtn.o`) then come from
the board's sysroot rather than the toolchain, so its own copies of
those need not match the board. Its `libgcc` does not follow that rule:
`crtbeginS.o`, `crtendS.o` and the `libgcc` the compiler links come
from the toolchain's own directory, and a toolchain much newer than the
board's gcc can leave a binary asking for `GCC_x.y` symbols the board's
`libgcc_s.so.1` does not have. That failure appears when the binary
runs, not when it links, which is the other reason to run the smoke
test on a board after changing toolchains.

[messense/macos-cross-toolchains]: https://github.com/messense/macos-cross-toolchains

## 3. Sysroot

Linking needs a copy of the board's filesystem — `libbela` plus the EVL
runtime and the Debian libraries they depend on:

```sh
scripts/sync-sysroot.sh            # ./bela-sysroot from root@bela.local
```

The result is about 820 MB and is git-ignored. Re-run it after
updating the board image, and after a change to the script's own list
of paths: a sysroot synced by an earlier version has only what that
version copied, and nothing in a build notices what is missing unless
it is something the build needs.

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
