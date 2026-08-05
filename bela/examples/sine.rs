//! Plays a 440 Hz sine tone on all audio output channels.
//!
//! A phase is state the *block* carries, not state a thread carries, so
//! this is the example of the pattern for that: the application holds
//! the phase the block starts at, `render_pre` hands each thread the
//! phase its own share of the block begins with, `render` advances its
//! own copy, and `render_post` moves the block's phase on. The output
//! is the same tone whatever `Settings::thread_count` says.
//!
//! Cross-compile and run on the board (see docs/cross-compile.md):
//!
//! ```sh
//! cargo build -p bela --release --target aarch64-unknown-linux-gnu --example sine
//! ```

#![cfg_attr(
    not(bela_device),
    allow(
        dead_code,
        reason = "only the fallback main is reachable off-device; the application code should still compile and lint"
    )
)]

use core::f32::consts::TAU;
#[cfg(not(bela_device))]
use std::process::ExitCode;

use bela::{BelaApplication, BlockContext, RenderContext, SetupContext, ThreadInfo};

const FREQUENCY: f32 = 440.0;
const AMPLITUDE: f32 = 0.3;

struct Sine {
    /// Where the next block starts. Advanced once per block, in
    /// `render_post`, so it never depends on how the block was split.
    phase: f32,
    phase_increment: f32,
}

impl Sine {
    const fn new() -> Self {
        Self {
            phase: 0.0,
            phase_increment: 0.0,
        }
    }
}

/// One thread's running phase, seeded per block by `render_pre`.
struct Phase {
    /// The first frame this thread will write, so that `render_pre`
    /// can work out the phase that frame starts at.
    first_frame: usize,
    phase: f32,
}

impl BelaApplication for Sine {
    type RenderState = Phase;

    fn setup(&mut self, context: &SetupContext) -> bool {
        self.phase_increment = TAU * FREQUENCY / context.audio_sample_rate();
        true
    }

    fn create_render_state(&mut self, thread: ThreadInfo, context: &SetupContext) -> Phase {
        // The same split `RenderContext::audio_frame_range` makes. It
        // does not change from block to block, so it is worked out once
        // here rather than on every one of them.
        let frames = context.audio_frames();
        Phase {
            first_frame: frames * thread.index() / thread.count(),
            phase: 0.0,
        }
    }

    // Real-time safe: arithmetic on values the states already hold.
    fn render_pre(&mut self, states: &mut [Phase], _context: &mut BlockContext) {
        for state in states {
            #[allow(
                clippy::cast_precision_loss,
                reason = "a frame index within a block is far below f32's exact integer range"
            )]
            let offset = state.first_frame as f32 * self.phase_increment;
            state.phase = self.phase + offset;
        }
    }

    // Real-time safe: arithmetic and writes to this thread's frames —
    // no allocation, blocking, system calls or panicking code paths.
    fn render(&self, state: &mut Phase, context: &mut RenderContext) {
        for frame in context.audio_frame_range() {
            let sample = AMPLITUDE * state.phase.sin();
            for channel in 0..context.audio_out_channels() {
                context.audio_write(frame, channel, sample);
            }
            state.phase += self.phase_increment;
        }
    }

    // Real-time safe: one multiplication and a wrap.
    fn render_post(&mut self, _states: &mut [Phase], context: &mut BlockContext) {
        #[allow(
            clippy::cast_precision_loss,
            reason = "a block's frame count is far below f32's exact integer range"
        )]
        let advanced = context.audio_frames() as f32 * self.phase_increment;
        self.phase = (self.phase + advanced) % TAU;
    }
}

#[cfg(bela_device)]
fn main() -> Result<(), bela::Error> {
    bela::Bela::run(Sine::new(), &bela::Settings::new())
}

#[cfg(not(bela_device))]
fn main() -> ExitCode {
    eprintln!("This example must be cross-compiled for Bela Gem (aarch64-unknown-linux-gnu).");
    ExitCode::FAILURE
}
