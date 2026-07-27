//! Copies the audio inputs straight to the audio outputs.
//!
//! Cross-compile and run on the board (see docs/cross-compile.md):
//!
//! ```sh
//! cargo build -p bela --release --target aarch64-unknown-linux-gnu --example passthrough
//! ```

#![cfg_attr(
    not(bela_device),
    allow(
        dead_code,
        reason = "only the fallback main is reachable off-device; the application code should still compile and lint"
    )
)]

#[cfg(not(bela_device))]
use std::process::ExitCode;

use bela::{BelaApplication, Context};

struct Passthrough;

// Safety: render only touches the context buffers — no allocation,
// blocking, system calls or panicking code paths.
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

#[cfg(bela_device)]
fn main() -> Result<(), bela::Error> {
    bela::Bela::run(Passthrough, &bela::Settings::new())
}

#[cfg(not(bela_device))]
fn main() -> ExitCode {
    eprintln!("This example must be cross-compiled for Bela Gem (aarch64-unknown-linux-gnu).");
    ExitCode::FAILURE
}
