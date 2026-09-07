# bela

Safe Rust API for real-time audio on [Bela Gem] (PocketBeagle 2,
`aarch64-unknown-linux-gnu`), built on the raw FFI bindings in
[`bela-sys`](../bela-sys).

User code implements the `BelaApplication` trait and hands an instance
to `Bela::run`:

```rust,ignore
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
        // This thread's share of the block.
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

What this wraps is Bela's core audio API, MIDI and an FFT. Its C++
libraries — the browser scope, Trill, the GUI and the rest — are not
wrapped.
[docs/scope.md](https://github.com/akiomik/bela-rs/blob/main/docs/scope.md)
lists what is in, what is left out on purpose and why, and what is
merely not written yet. It follows the repository rather than a
release, so what it calls wrapped is at least what this version has.

## One or four threads, one application model

Bela can render a block on all four of a Bela Gem's cores, and it does
so by calling `render` on every thread at once, for the same block,
over the same buffers — it partitions nothing itself. `BelaApplication`
is shaped for that, and a single render thread is the same shape with
one of everything:

- the application is shared as `&self` while rendering, so whatever
  `render` mutates lives in a `RenderState`, one per thread, built by
  `create_render_state` before audio starts;
- `RenderContext` reads the whole block but writes only
  `audio_frame_range()`, and the ranges tile the block exactly;
- `render_pre` and `render_post` bracket the parallel section on the
  main audio thread, with the whole block and every state to
  themselves — where per-block preparation and mixing down belong.

`Settings::thread_count` chooses how many threads; nothing else about
an application changes with it. See
[`examples/parallel.rs`](examples/parallel.rs), which measures that the
work really was divided, and
[`docs/multithreaded-rendering.md`](../docs/multithreaded-rendering.md)
for what Bela does and how it was measured.

Work that must not happen in `render` — file and network I/O,
expensive calculations, anything that allocates or blocks — goes into
an `AuxiliaryTask`, which `render` triggers with a real-time safe
`schedule()` call. The callback owns its state and shares with `render`
through atomics or a lock-free queue; see
[`examples/aux_task.rs`](examples/aux_task.rs).

Debugging output from the audio thread goes through `rt_println!`,
which formats into a fixed-size stack buffer and hands it to Bela's
real-time print function — `println!` allocates and blocks, and is
forbidden in `render`:

```rust,ignore
rt_println!("{} blocks, {} underruns", blocks, context.underrun_count());
```

Whether `render` fits within its block deadline is answered by
`Settings::cpu_monitoring`, which makes `BlockContext::cpu_usage`
report how much of each block the audio thread uses, and by `CpuTimer`, which
measures one section of `render` at a time; see
[`examples/cpu.rs`](examples/cpu.rs). Without them the first sign of
running out of headroom is a dropout.

A built binary stays reconfigurable through Bela's standard
command-line options — `--period`, `--verbose`, `--use-analog` and the
rest, the same set every other way of writing a Bela program accepts.
`Bela::run_with_args` applies them on top of `Settings`, so the
application keeps its own defaults, and `print_usage` prints the list
for a `--help` of your own:

```rust,ignore
fn main() -> Result<(), bela::Error> {
    let settings = Settings::new().period_size(32);
    Bela::run_with_args(Passthrough, &settings, std::env::args_os())
}
```

Options of the program's own are parsed by the program, which hands on
what is left; see [`examples/command_line.rs`](examples/command_line.rs).

See [`examples/`](examples) for runnable versions and the
[repository README](../README.md) for project status and
cross-compilation instructions.

## Testing DSP off the board

`RealFft::new` returns `Error::FftUnavailable` off the device target,
because NE10 is on the board and nowhere else. This crate ships no host
FFT backend, deliberately — one would verify a program's own arithmetic
while putting this crate's name on numbers that are not the board's.

What works is a trait the program owns, with one implementation over
`RealFft` and another over a host FFT crate. Two things that trait has
to carry, because no backend carries them: `forward` is **unscaled**
and `inverse` restores the original amplitudes, whoever applies the
`1 / length`; and the two backends **will not agree bit for bit**, so
host tests assert what the DSP means within a tolerance rather than
recorded values.

[`tests/off_board_fft.rs`](https://github.com/akiomik/bela-rs/blob/main/bela/tests/off_board_fft.rs)
is a worked version — the trait, both implementations, and tests that
run against whichever backend the target has.
[docs/fft.md](https://github.com/akiomik/bela-rs/blob/main/docs/fft.md#testing-dsp-off-the-board)
explains why it is shaped that way. Both links are absolute because
this file is read on crates.io, where the repository around it is not
there.

## Downstream setup

Building a device binary needs three compiler-driver arguments derived
from the Bela sysroot (`--sysroot`, `-B`, `-Wl,-rpath-link`; see
[docs/cross-compile.md](../docs/cross-compile.md) for what each is
for). `bela-sys` publishes them and this crate relays them, because
[`links` metadata reaches only an immediate dependent](https://doc.rust-lang.org/cargo/reference/build-scripts.html#the-links-manifest-key)
— an application depending on `bela` is not one of `bela-sys`'s. An
application therefore needs its own small `build.rs` to turn what
`bela` relayed into link arguments for its own binary:

```rust,ignore
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

and `.cargo/config.toml` names the compiler driver directly:

```toml
[target.aarch64-unknown-linux-gnu]
linker = "aarch64-unknown-linux-gnu-gcc"   # or aarch64-linux-gnu-gcc, gcc, ...
```

No file to copy from this repository, and no executable bit to
preserve. See [docs/cross-compile.md](../docs/cross-compile.md) for
compiler installation and the toolchain rules `bela-sys` uses to build
its MIDI shim with a compiler matching this linker.

[Bela Gem]: https://bela.io
