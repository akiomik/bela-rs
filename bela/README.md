# bela

Safe Rust API for real-time audio on [Bela Gem] (PocketBeagle 2,
`aarch64-unknown-linux-gnu`), built on the raw FFI bindings in
[`bela-sys`](../bela-sys).

User code implements the `BelaApplication` trait — an `unsafe` trait,
because implementing it is a promise that `render` is real-time safe —
and hands an instance to `Bela::run`:

```rust,ignore
use bela::{Bela, BelaApplication, Context, Settings};

struct Passthrough;

unsafe impl BelaApplication for Passthrough {
    fn render(&mut self, context: &mut Context) {
        let channels = context
            .audio_in_channels()
            .min(context.audio_out_channels());
        for frame in 0..context.audio_frames() {
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
`Settings::cpu_monitoring`, which makes `Context::cpu_usage` report how
much of each block the audio thread uses, and by `CpuTimer`, which
measures one section of `render` at a time; see
[`examples/cpu.rs`](examples/cpu.rs). Without them the first sign of
running out of headroom is a dropout.

See [`examples/`](examples) for runnable versions and the
[repository README](../README.md) for project status and
cross-compilation instructions.

[Bela Gem]: https://bela.io
