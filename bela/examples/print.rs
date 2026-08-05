//! Reports the audio configuration from `setup` and a heartbeat from
//! `render_post`, using the real-time safe printing macros.
//!
//! Prints roughly once a second rather than once a block: formatting
//! costs time on the audio thread.
//!
//! The heartbeat is counted in `render_post` rather than in `render`
//! because a block is one block however many threads rendered it —
//! `render_post` runs once per block, on the main audio thread, with
//! `&mut self`, which is exactly what a per-block counter wants.
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

use bela::{
    BelaApplication, BlockContext, CleanupContext, RenderContext, SetupContext, ThreadInfo,
    rt_println,
};

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

impl BelaApplication for Heartbeat {
    type RenderState = ();

    fn setup(&mut self, context: &SetupContext) -> bool {
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
             {} in / {} out audio channels, {} render thread(s)",
            context.audio_in_channels(),
            context.audio_out_channels(),
            context.thread_count()
        );
        true
    }

    fn create_render_state(&mut self, _thread: ThreadInfo, _context: &SetupContext) {}

    fn render(&self, _state: &mut (), _context: &mut RenderContext) {}

    // Real-time safe: a counter, and a formatted line once a second.
    fn render_post(&mut self, _states: &mut [()], context: &mut BlockContext) {
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

    fn cleanup(&mut self, _states: &mut [()], _context: &CleanupContext) {
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
