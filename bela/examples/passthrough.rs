//! Copies the audio inputs straight to the audio outputs.
//!
//! Frame-independent work, so it divides across render threads without
//! anything having to be carried between them: run it with
//! `Settings::thread_count(NonZeroU32::new(4).expect("4 is not zero"))`
//! and each thread copies a quarter of the block.
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

use bela::{BelaApplication, RenderContext, SetupContext, ThreadInfo};

struct Passthrough;

impl BelaApplication for Passthrough {
    /// Nothing to carry from one block to the next: every output
    /// sample depends only on the input sample beside it.
    type RenderState = ();

    fn create_render_state(&mut self, _thread: ThreadInfo, _context: &SetupContext) {}

    // Real-time safe: only reads and writes of the context buffers —
    // no allocation, blocking, system calls or panicking code paths.
    fn render(&self, _state: &mut (), context: &mut RenderContext) {
        let channels = context
            .audio_in_channels()
            .min(context.audio_out_channels());
        // This thread's share of the block. With one render thread
        // that is all of it; with four, the four ranges tile it.
        for frame in context.audio_frame_range() {
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
