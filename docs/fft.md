# FFT

What this workspace wraps to do an FFT on a Bela Gem, why it is NE10
rather than Bela's `Fft` class, and what a board had to be asked
because no header answers it.

Everything about Bela and NE10 here is **read from the sources and
binaries on the board** (Bela 1.18.0, Debian Bookworm image
2026-03-25), as synced into the sysroot by `scripts/sync-sysroot.sh`;
line numbers are into that copy. Where something was disassembled or
run, it says so. Those sources are not upstream's — the board's
checkout is `fb362a5` with an image overlay (see
[board-facts.md](board-facts.md)) — and NE10's are Debian's package
rather than anything Bela publishes.

## Summary

- The FFT comes from **NE10** (`libNE10.so.10`, BSD-3-Clause), called
  directly from [`bela-sys/src/ne10.rs`](../bela-sys/src/ne10.rs).
- It does **not** come from Bela's `Fft` class in `libbelaextra`, which
  wraps these same NE10 calls and little else.
- Nothing is compiled for it: no shim, no bindgen. What that costs is a
  hand-written `extern` block, and [Pinning the
  ABI](#pinning-the-abi) is how it is kept honest.
- What the transforms do to their arguments — whether they write into
  their inputs, whether the inverse scales, which lengths work — is
  **not settled by the headers**. `scripts/probe-fft.sh` asked a
  board, and [the answers](#what-the-board-says-the-transforms-do)
  are why the safe API will start at 8 points: at 2 and 4 the NEON
  kernels write outside every buffer they are given.

## NE10 rather than Bela's `Fft` class

`docs/midi.md` wraps Bela's `Midi` class rather than talking to ALSA,
and gives the reason: the class carries real-time design worth reusing
— an input thread, a parser with a 100-message ring, device recovery,
an `AuxTaskNonRT` for output — and "writing them again to avoid a C++
boundary is the wrong trade for this crate, which exists to make Bela
reachable from Rust rather than to replace it."

That reasoning does not reach `Fft`. `libraries/Fft/` is two files, and
between `ne10_fft_alloc_r2c_float32` and the two `_neon` calls the
class contributes two `malloc`s, a power-of-two check, four unchecked
accessors and two `std::vector` overloads that copy:

- `setup(length)` (`Fft.cpp:66`) refuses a non-power-of-two, then
  allocates a time-domain buffer of `length` floats (`Fft.cpp:76`), a
  frequency-domain buffer of `length` **complex** values
  (`Fft.cpp:77` — the transform writes `length / 2 + 1`), and a plan
  (`Fft.cpp:78`).
- `fft()` (`Fft.cpp:117`) is one call to
  `ne10_fft_r2c_1d_float32_neon`; `ifft()` (`Fft.cpp:136`) one call to
  `ne10_fft_c2r_1d_float32_neon`.
- `fdr`, `fdi`, `td` (`Fft.h:39`, `:45`, `:60`) hand out `float&` per
  index, unchecked; `fda` (`Fft.h:52-56`) is `sqrtf_neon` over two of
  them.

So calling NE10 directly is not reimplementing Bela's work: it is
making the same four calls, from Rust instead of from C++, and it is
less code than a shim would be. Against that, wrapping the class would
cost:

- **A C++ shim and its build path**, with the C++ ABI question
  `docs/board-facts.md` had to settle for `Midi` by measuring
  `sizeof` on two toolchains. There is no C++ here at all.
- **An LGPL 3.0 condition.** `libraries/Fft/lib.metadata` declares the
  class LGPL 3.0, where these crates are MIT OR Apache-2.0 — the
  arrangement `docs/midi.md` documents for `Midi`, paid a second time
  for something NE10 gives under BSD-3-Clause out of the same library
  the class itself calls. (`libraries/ne10/lib.metadata` also says LGPL
  3.0; that covers Bela's own two-line convenience header, not NE10,
  whose licence is at the top of every header it ships —
  `NE10_dsp.h:1-26`.)
- **A buffer model to hide.** The class owns its buffers, so a wrapper
  either exposes one-sample-at-a-time accessors or copies in and out
  of every transform. Calling NE10 lets a transform read the caller's
  own window where it already is.
- **Four defects to guard.** `Fft()` (`Fft.cpp:55`) leaves `length`
  uninitialised (`Fft.h:66`) while `td`/`fdr` are unchecked;
  `setup` sets `length` before allocating and, on failure, leaves it
  non-zero with null buffers (`Fft.cpp:74`, `:89-93`); `Fft(size_t)`
  (`Fft.cpp:56-58`) discards `setup`'s `-1`, so `Fft x(3);` yields an
  object with a null plan and no way to know; and `ifft()` runs the
  inverse over the object's own spectrum, which NE10 writes into.

## What is on the board

`ne10_fft_alloc_r2c_float32(nfft)` returns a pointer to a struct
(`NE10_types.h:272-290`) holding the twiddles, the factors **and a
scratch buffer** (`NE10_types.h:274`) — which is what makes a
transform allocation-free, and what makes a plan single-user. Its
layout is conditional on `NE10_UNROLL_LEVEL` (`NE10_types.h:275-289`),
so nothing here may depend on it; as an opaque pointer, nothing does.

**The plan is one allocation.** Disassembling the board's
`libNE10.so.10`: `ne10_fft_alloc_r2c_float32` makes a single
`malloc@plt` call (at `+0xa75c`; the other six `bl`s are `ne10_factor`,
the twiddle generators and `sincos`), and
`ne10_fft_destroy_r2c_float32` is one instruction, `b free@plt` at
`0x43c4`. So the twiddles and the scratch are offsets inside that
block, destroying a plan is one `free`, and a null plan is safe to
destroy.

`ne10_fft_cpx_float32_t` (`NE10_types.h:230-234`) is `{ float r; float
i; }`, unconditional. `ne10_int32_t` is `signed int`
(`NE10_types.h:76`) — the name promises 32 bits and the typedef says
`int`, which is why `ne10.rs` spells it `c_int`.

The transforms have two spellings. `ne10_fft_r2c_1d_float32` is a
function pointer that stays null until `ne10_init` runs — `nm -D`
shows it in BSS — while `ne10_fft_r2c_1d_float32_neon` is a defined
text symbol. Bela calls the `_neon` symbols directly (`Fft.cpp:120`,
`:139`) and never calls `ne10_init`; so does this crate. The target is
ARMv8 with NEON, so there is nothing to dispatch.

`NE10_MALLOC` is plain `malloc` (`NE10_macros.h:53`), so NE10's own
buffers are aligned only as `malloc` aligns them — which is why
question 5 below was worth asking rather than assuming.

## Pinning the ABI

`bela-sys/src/ne10.rs` is written by hand, so no build regenerates when
a board image moves NE10: a changed typedef or parameter type would
link, run and go wrong. `xtask/src/check_vendor.rs` exists for exactly
that failure mode for Bela's headers, and NE10's are outside the
directory it looked at. Two layers now cover it, and they catch
different things:

1. **`bela-sys/vendor/ne10/`** holds `NE10_dsp.h` and `NE10_types.h` —
   the include closure of the assertions below — as a drift baseline.
   Nothing generates from them, unlike `vendor/bela/`; they exist so
   that `cargo xtask check-vendor --board` can diff them against a
   board. `SOURCE` beside them records the library's own identity:

   ```
   library: /usr/lib/aarch64-linux-gnu/libNE10.so.10
   build-id: 2abed00810b18c216f992a4a79c5228605e58b1a
   sha256: 186da1ff9a854d996e19546000681bb65b083903ef3b6f01c4898e03ec69f6e9
   ```

   The soname identifies nothing — every build calls itself
   `libNE10.so.10` — and a rebuilt library with unchanged headers is
   exactly the case where the measurements below have to be taken
   again. `check-vendor` compares both values and reports a difference
   as drift.
2. **`bela-sys/abi/ne10_abi.c`** asserts, at build time, that the
   headers still describe what `ne10.rs` declares: the typedefs, the
   complex struct's size, alignment, offsets and field types, and all
   four function signatures. Every assertion is written in C
   primitives rather than in NE10's own typedefs — `ne10_float32_t` on
   both sides of a comparison proves nothing if the typedef is what
   moved, and what Rust names is `f32`, `c_int` and "pointer to an
   opaque struct". It needs no board, so unlike `check-vendor` it runs
   in CI with a synced sysroot, and it runs on a native build on the
   board as well, where the headers are the system's own.

Neither layer can say what a transform *does*. That is the next
section.

## What the board says the transforms do

`bela-sys/examples/ne10_probe.rs`, run by `scripts/probe-fft.sh`, asks
a board what the headers cannot say. It creates no audio system — NE10
is a plain C library — so the length sweep fits in one run per length
and a failure leaves nothing holding the audio device.

**Measured 2026-09-07** on a Bela Gem Stereo (Bela 1.18.0, Debian
Bookworm image 2026-03-25, NE10 0.9.10) against `libNE10.so.10` build
id `2abed00810b18c216f992a4a79c5228605e58b1a`, which is the one
`bela-sys/vendor/ne10/SOURCE` records — `cargo xtask check-vendor
--board` reported both headers and both identity values matching on
the same day. A different build can answer
differently: `cargo xtask check-vendor --board` is what notices, and
then this section has to be measured again.

| Question | Answer |
|---|---|
| 1. Does the inverse write into its spectrum? | No, it is preserved — at 16 and at every supported length |
| 2. Does the forward write into its signal? | No, likewise |
| 3. Is the inverse scaled? | Yes: a unit cosine comes back at 1.000000, so NE10 applies the `1/N` itself |
| 4. Which lengths work? | **8 to 65536.** 2 and 4 corrupt the heap — see below |
| 5. Does alignment matter? | No: a window offset by one `f32` gives the same spectrum |
| 6. How many bins does the forward write? | Exactly `N / 2 + 1`; the canary past the last bin survives |

Both inputs being preserved is the useful surprise. Upstream's
`NE10_rfft_float32.neonintrinsic.c` assigns to `fin[0]` and NE10's
parameters are not `const`, so the safe API was designed expecting to
take both by `&mut`; on this build it does not have to. The sweep
checks it at every length rather than at one, because the code path
changes with the length — which question 4 is the proof of.

### Below 8 points, the transforms write outside every buffer

At 2 and 4 points the NEON kernels write past *and before* the buffers
they are given. Measured with each buffer placed in the middle of a
much larger allocation, so that the overrun lands in canaries instead
of in the allocator's bookkeeping:

| Length | Forward, into the spectrum | Inverse, into the signal |
|---|---|---|
| 2 | 3 bins before bin 0, 3 bins past bin 1 | 2 floats before, 26 past |
| 4 | 2 bins before bin 0, 2 bins past bin 2 | 24 floats past |
| 8 | nothing outside | nothing outside |
| 1024 | nothing outside | nothing outside |

With ordinary buffers the process dies rather than returning a wrong
answer: `free(): invalid pointer` on the spectrum, which is glibc
finding the chunk header before the buffer overwritten, and once a `Fatal
glibc error: malloc assertion failure in sysmalloc`. The input
buffers are untouched in every case; it is the output side that
overruns, in both directions.

Neither the headers nor Bela's `Fft` class says anything about this.
`Fft::setup` accepts any power of two (`Fft.cpp:66-71`), and its
frequency-domain buffer is `length` complex values rather than
`length / 2 + 1` (`Fft.cpp:77`), which is four times the room needed at
`length = 2` — so a program using the class survives the same overrun
by accident, on the output side, and its `ifft()` writes into a
`length`-float buffer that the inverse overruns by 26 floats at that
size.

**So the safe API's `FftLength::MIN` is 8**, and it is a hard floor
rather than a preference: 2 and 4 are lengths where a wrapper cannot
make the call safe, because the damage is outside every buffer the
caller owns. 65536 is the top of what was swept and what
`FftLength::MAX` will be set from; both are decisions recorded against
this measurement rather than limits NE10 states.

### Still to measure

What a transform costs on the audio thread, per length, with
`CpuSection` from inside `render`. That one needs an audio system, so
it belongs with the safe API rather than with this probe.

## Status

The safe API (`FftLength`, `FftBin`, `RealFft`) is issue #138. This
document and the `bela-sys` layer are its first step: the declarations,
the ABI checks, the probe — and now the answers the probe gave, which
are what the API is shaped against. `FftLength::MIN` is 8 because of
them.
