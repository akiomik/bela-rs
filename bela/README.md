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

See [`examples/`](examples) for runnable versions and the
[repository README](../README.md) for project status and
cross-compilation instructions.

[Bela Gem]: https://bela.io
