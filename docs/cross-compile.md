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

## 2. Linking

Three compiler-driver arguments, derived from the sysroot, are needed
at link time:

- `--sysroot=<BELA_SYSROOT>`, because Debian's linker scripts (e.g.
  `libc.so`) refer to absolute paths that must resolve inside it;
- `-B<BELA_SYSROOT>/usr/lib/aarch64-linux-gnu`, because Debian keeps
  the startup files (`Scrt1.o`, `crti.o`, `crtn.o`) in that multiarch
  directory, which a toolchain built for a different triple —
  `aarch64-unknown-linux-gnu` against Debian's `aarch64-linux-gnu` —
  does not search on its own;
- `-Wl,-rpath-link=...`, so the linker can resolve the dependencies
  *of* the Bela shared libraries at link time (`libbela` needs
  `libevl` and `libstdc++`, which in turn need `libbpf`, `libm` and
  others).

There are two ways to get them onto the link line. **An application
depending on the published `bela` and `bela-sys` crates uses the
direct linker below** — nothing to copy, no wrapper file, no
executable bit. This repository's own workspace still uses the
compatibility wrapper, kept for exactly that: a working path during
migration, and evidence that the two agree (`scripts/smoke-test.sh`
passes either way).

### Downstream: a direct linker and a small `build.rs`

`bela-sys/build.rs` derives the three arguments above from
`BELA_SYSROOT` and publishes them as `links` metadata. `bela`
(`links = "bela_relay"`) reads that and republishes it under its own
name, because Cargo passes `links` metadata only to an *immediate*
dependent (`bela-sys` → `bela`), not to `bela`'s own dependents — see
[the Cargo reference](https://doc.rust-lang.org/cargo/reference/build-scripts.html#the-links-manifest-key).
An application therefore needs a short build script of its own to read
what `bela` republished and turn it into link arguments for its own
binary:

```rust
// build.rs
fn main() {
    let Ok(count) = std::env::var("DEP_BELA_RELAY_LINK_ARGS_COUNT") else {
        return; // host build, or a native build with BELA_SYSROOT unset
    };
    let count: usize = count.parse().expect("DEP_BELA_RELAY_LINK_ARGS_COUNT is not a number");
    for index in 0..count {
        let key = format!("DEP_BELA_RELAY_LINK_ARGS_{index}");
        let arg = std::env::var(&key).unwrap_or_else(|_| panic!("{key} is missing"));
        println!("cargo::rustc-link-arg={arg}");
    }
}
```

and `.cargo/config.toml` names the compiler driver directly — no
wrapper in the path at all:

```toml
[target.aarch64-unknown-linux-gnu]
linker = "aarch64-unknown-linux-gnu-gcc"   # or aarch64-linux-gnu-gcc, gcc, ...
```

The compiler still has to be installed; see the toolchain
installation instructions below. An intermediary crate between `bela`
and an application — one more layer of wrapping — has to relay the
same way `bela` does: read `DEP_BELA_RELAY_LINK_ARGS_*`, apply it to
its own targets, and republish it under its own `links` name.

### This repository: the compatibility wrapper

`.cargo/config.toml` points the linker at
[`scripts/aarch64-bela-linker.sh`](../scripts/aarch64-bela-linker.sh),
a wrapper that adds the same three arguments (see the comments in the
script). The wrapper calls a compiler, which has to be installed, and
`BELA_CC` says which one. A cross toolchain is named after the triple
it was built for, so the name depends on where the toolchain came
from.

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

### The MIDI shim's compiler

A **C++** compiler from the same toolchain as the final link is needed
too, because `bela-sys` compiles a small shim over Bela's `Midi` class
(see [midi.md](midi.md)). `bela-sys/build.rs` picks it in this order:

1. `BELA_CXX`, if set, always wins.
2. Otherwise, the linker Cargo resolved for the target (`RUSTC_LINKER`,
   set when `.cargo/config.toml` or `CARGO_TARGET_*_LINKER` names one
   directly — the direct-linker path above): a name ending in `gcc`
   answers for the C++ compiler beside it the same way `BELA_CC` does
   below, so nothing more has to be set. The wrapper's own name is
   recognised and treated as "nothing resolved", so a workspace still
   using it falls through to step 3 instead of being read as an
   unsupported linker.
3. Otherwise `BELA_CC` — read here because the wrapper reads it too,
   so following it keeps the shim and the wrapped link in one
   toolchain: `aarch64-linux-gnu-gcc` gives `aarch64-linux-gnu-g++`,
   `/usr/bin/gcc` gives `/usr/bin/g++`, plain `gcc` gives `g++`.
4. With none of the three set, the tap's `aarch64-unknown-linux-gnu-g++`.

A resolved linker or a `BELA_CC` that does not end in `gcc` **fails the
build** rather than falling back to a default, because the fallback
would compile the shim with a toolchain the link is not using.
`BELA_CXX` is the answer for those:

```sh
export BELA_CXX=aarch64-linux-gnu-g++
```

`BELA_CXX`, `RUSTC_LINKER` and `BELA_CC` are what this reads; `CXX` and
`CXX_aarch64-unknown-linux-gnu` — which `cc` would otherwise honour —
are not consulted, because the compiler has to match whatever links
the binary, not a general-purpose default. The archiver follows the
compiler's name (`aarch64-linux-gnu-g++` implies `aarch64-linux-gnu-ar`),
except for names nothing follows from, such as a `clang++`; `AR` and
`AR_aarch64-unknown-linux-gnu` are read first either way, so a
toolchain that needs a different one can say so.

Both have to come from the same toolchain. The shim allocates a class
whose methods live in `libbelaextra.so`, so it has to agree with it
about layout — measured equal between the tap's g++ 15.2.0 and the
board's own `clang++`, and recorded in
[board-facts.md](board-facts.md). Compiling with one toolchain and
linking with another is also how a binary ends up asking the board for
`libstdc++` or `libgcc_s` symbols it does not have, which is a failure
that waits until the program runs.

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
