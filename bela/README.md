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

[Bela Gem]: https://bela.io
