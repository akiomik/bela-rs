//! Reports the audio configuration from `setup` and a heartbeat from
//! `render`, using the real-time safe printing macros.
//!
//! Prints roughly once a second rather than once a block: formatting
//! costs time on the audio thread.
//!
//! Cross-compile and run on the board (see docs/cross-compile.md):
//!
//! ```sh
//! cargo build -p bela --release --target aarch64-unknown-linux-gnu --example print
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

use bela::{BelaApplication, Context, rt_println};

struct Heartbeat {
    blocks: u64,
    blocks_per_report: u64,
}

impl Heartbeat {
    const fn new() -> Self {
        Self {
            blocks: 0,
            // Replaced in setup, once the block size is known.
            blocks_per_report: 1,
        }
    }
}

// Safety: render counts blocks and prints through the real-time safe
// path — no allocation, blocking, system calls or panicking code paths.
unsafe impl BelaApplication for Heartbeat {
    fn setup(&mut self, context: &mut Context) -> bool {
        let frames = context.audio_frames();
        let sample_rate = context.audio_sample_rate();
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the sample rate is a small positive number"
        )]
        let sample_rate_hz = sample_rate as u64;
        // `max(1)` on both sides: a zero would divide by zero or make
        // every block a reporting block.
        self.blocks_per_report = (sample_rate_hz / frames.max(1) as u64).max(1);

        rt_println!(
            "setup: {sample_rate} Hz, {frames} frames per block, \
             {} in / {} out audio channels, thread {}/{}",
            context.audio_in_channels(),
            context.audio_out_channels(),
            context.this_thread(),
            context.thread_count()
        );
        true
    }

    fn render(&mut self, context: &mut Context) {
        self.blocks += 1;
        if self.blocks % self.blocks_per_report == 0 {
            rt_println!(
                "render: {} blocks, {} frames elapsed, {} underruns",
                self.blocks,
                context.audio_frames_elapsed(),
                context.underrun_count()
            );
        }
    }

    fn cleanup(&mut self, _context: &mut Context) {
        rt_println!("cleanup: {} blocks rendered", self.blocks);
    }
}

#[cfg(bela_device)]
fn main() -> Result<(), bela::Error> {
    bela::Bela::run(Heartbeat::new(), &bela::Settings::new())
}

#[cfg(not(bela_device))]
fn main() -> ExitCode {
    eprintln!("This example must be cross-compiled for Bela Gem (aarch64-unknown-linux-gnu).");
    ExitCode::FAILURE
}
