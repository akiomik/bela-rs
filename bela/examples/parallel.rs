//! Splits a block of frames across Bela's render threads and measures
//! that the work was spread rather than duplicated.
//!
//! The load is a bank of sine oscillators evaluated per frame, which is
//! divisible by frame and heavy enough to see. Each render thread
//! renders its own share of the block, keeps its own oscillator
//! phases — seeded per block by `render_pre` so the tone is the same
//! whatever the split — and records what it did. `cleanup` prints one
//! line per thread and a summary.
//!
//! The number of render threads is this program's own first argument,
//! defaulting to 4:
//!
//! ```sh
//! ./parallel 1
//! ./parallel 4
//! ```
//!
//! Comparing the two runs is the measurement. Four things say the work
//! was divided:
//!
//! - every thread reports calls of its own, on a Linux thread id of its
//!   own, running on a different core;
//! - the frames the threads rendered add up to exactly one block per
//!   block, rather than one block per thread;
//! - `uncovered` is 0: `render_pre` stamps every frame with a sentinel
//!   and `render_post` finds none of them left, so between them the
//!   threads wrote the whole block;
//! - the audio thread's own busy percentage falls as threads are added,
//!   for the same number of oscillators.
//!
//! Reading the thread id and the core costs two system calls, which
//! `render` must not normally make; this example makes them on its
//! first block only, because they are the measurement.
//!
//! Cross-compile and run on the board (see docs/cross-compile.md):
//!
//! ```sh
//! cargo build -p bela --release --target aarch64-unknown-linux-gnu --example parallel
//! ```

#![cfg_attr(
    not(bela_device),
    allow(
        dead_code,
        reason = "only the fallback main is reachable off-device; the application code should still compile and lint"
    )
)]

use core::f32::consts::TAU;
use core::num::NonZeroU32;
#[cfg(not(bela_device))]
use std::process::ExitCode;

use bela::{
    BelaApplication, BlockContext, CleanupContext, CpuTimer, RenderContext, SetupContext,
    ThreadInfo, rt_println,
};

/// Enough oscillators for one thread to be busy and four to be visibly
/// less so.
const OSCILLATORS: usize = 192;
const BASE_FREQUENCY: f32 = 55.0;
const AMPLITUDE: f32 = 0.2;

/// Render threads to use when the command line does not say.
const DEFAULT_THREADS: u32 = 4;

const MEASUREMENTS_PER_CYCLE: u32 = 2000;

/// Written into every frame by `render_pre` and gone by the end of the
/// block if the threads between them covered it. Not a value any
/// oscillator sum can produce.
const UNWRITTEN: f32 = f32::NAN;

const fn cycle() -> NonZeroU32 {
    NonZeroU32::new(MEASUREMENTS_PER_CYCLE).expect("the cycle length is a non-zero constant")
}

/// The Linux thread id of the calling thread, or -1 off-device.
#[cfg(bela_device)]
fn thread_id() -> i64 {
    // Safety: gettid takes no arguments and cannot fail.
    i64::from(unsafe { libc::gettid() })
}

#[cfg(not(bela_device))]
const fn thread_id() -> i64 {
    -1
}

/// The core the calling thread is running on, or -1 off-device.
#[cfg(bela_device)]
fn current_cpu() -> i32 {
    // Safety: sched_getcpu takes no arguments and reports -1 on
    // failure, which is the value used for "not known" anyway.
    unsafe { libc::sched_getcpu() }
}

#[cfg(not(bela_device))]
const fn current_cpu() -> i32 {
    -1
}

struct Parallel {
    /// The phases the *block* starts at, advanced once per block.
    phases: [f32; OSCILLATORS],
    phase_increments: [f32; OSCILLATORS],
    blocks: u64,
    /// Frames no thread wrote, over the whole run.
    uncovered: u64,
}

/// One render thread's oscillator bank and its record of the run.
struct Voice {
    thread: usize,
    /// The frames this thread owns, worked out once and the same for
    /// every block.
    first_frame: usize,
    last_frame: usize,
    phases: [f32; OSCILLATORS],
    calls: u64,
    frames: u64,
    thread_id: i64,
    cpu: i32,
    timer: CpuTimer,
}

impl Parallel {
    const fn new() -> Self {
        Self {
            phases: [0.0; OSCILLATORS],
            phase_increments: [0.0; OSCILLATORS],
            blocks: 0,
            uncovered: 0,
        }
    }
}

impl BelaApplication for Parallel {
    type RenderState = Voice;

    fn setup(&mut self, context: &SetupContext) -> bool {
        let sample_rate = context.audio_sample_rate();
        for (index, increment) in self.phase_increments.iter_mut().enumerate() {
            #[allow(
                clippy::cast_precision_loss,
                reason = "the oscillator index is far below f32's exact integer range"
            )]
            let harmonic = index as f32 + 1.0;
            *increment = TAU * BASE_FREQUENCY * harmonic / sample_rate;
        }
        rt_println!(
            "parallel: setup threads={} frames={} oscillators={OSCILLATORS} rate={sample_rate}",
            context.thread_count(),
            context.audio_frames()
        );
        true
    }

    fn create_render_state(&mut self, thread: ThreadInfo, context: &SetupContext) -> Voice {
        // The same split `RenderContext::audio_frame_range` makes.
        let frames = context.audio_frames();
        Voice {
            thread: thread.index(),
            first_frame: frames * thread.index() / thread.count(),
            last_frame: frames * (thread.index() + 1) / thread.count(),
            phases: [0.0; OSCILLATORS],
            calls: 0,
            frames: 0,
            thread_id: -1,
            cpu: -1,
            timer: CpuTimer::new(cycle()),
        }
    }

    // Real-time safe: arithmetic on values the states already hold,
    // plus one store per frame of the sentinel.
    fn render_pre(&mut self, states: &mut [Voice], context: &mut BlockContext) {
        for state in states.iter_mut() {
            #[allow(
                clippy::cast_precision_loss,
                reason = "a frame index within a block is far below f32's exact integer range"
            )]
            let offset = state.first_frame as f32;
            for (phase, (block_phase, increment)) in state
                .phases
                .iter_mut()
                .zip(self.phases.iter().zip(&self.phase_increments))
            {
                *phase = block_phase + offset * increment;
            }
        }

        // Stamped now, looked for again in `render_post`: a frame that
        // still carries it is a frame no thread claimed.
        let channels = context.audio_out_channels();
        for sample in context.audio_out().iter_mut().step_by(channels.max(1)) {
            *sample = UNWRITTEN;
        }
    }

    // Real-time safe from the second block on; the first also reads
    // the thread id and the core, which is the measurement.
    fn render(&self, state: &mut Voice, context: &mut RenderContext) {
        if state.calls == 0 {
            state.thread_id = thread_id();
            state.cpu = current_cpu();
        }
        state.calls += 1;

        let _oscillators = state.timer.measure();
        let channels = context.audio_out_channels();
        for frame in context.audio_frame_range() {
            let mut sample = 0.0;
            for (phase, increment) in state.phases.iter_mut().zip(&self.phase_increments) {
                sample += phase.sin();
                *phase += increment;
                if *phase >= TAU {
                    *phase -= TAU;
                }
            }
            #[allow(
                clippy::cast_precision_loss,
                reason = "the oscillator count is far below f32's exact integer range"
            )]
            let sample = AMPLITUDE * sample / OSCILLATORS as f32;
            for channel in 0..channels {
                context.audio_write(frame, channel, sample);
            }
            state.frames += 1;
        }
    }

    // Real-time safe: arithmetic and one read per frame.
    fn render_post(&mut self, _states: &mut [Voice], context: &mut BlockContext) {
        #[allow(
            clippy::cast_precision_loss,
            reason = "a block's frame count is far below f32's exact integer range"
        )]
        let frames = context.audio_frames() as f32;
        for (phase, increment) in self.phases.iter_mut().zip(&self.phase_increments) {
            *phase = (*phase + frames * increment) % TAU;
        }

        let channels = context.audio_out_channels();
        let mut uncovered = 0;
        for sample in context.audio_out().iter_mut().step_by(channels.max(1)) {
            if sample.is_nan() {
                // Nobody wrote this frame; silence it rather than
                // sending a NaN to the codec.
                *sample = 0.0;
                uncovered += 1;
            }
        }
        self.uncovered += uncovered;
        self.blocks += 1;
    }

    fn cleanup(&mut self, states: &mut [Voice], context: &CleanupContext) {
        let mut rendered = 0;
        for state in states.iter() {
            rt_println!(
                "parallel: thread={} tid={} cpu={} range={}..{} calls={} frames={} section={:.1}%",
                state.thread,
                state.thread_id,
                state.cpu,
                state.first_frame,
                state.last_frame,
                state.calls,
                state.frames,
                state.timer.usage().percentage()
            );
            rendered += state.frames;
        }
        let expected = self.blocks * context.audio_frames() as u64;
        rt_println!(
            "parallel: blocks={} rendered={rendered} expected={expected} uncovered={}",
            self.blocks,
            self.uncovered
        );
        let busy = context.cpu_usage().map_or(0.0, |usage| usage.percentage());
        rt_println!("parallel: audio-thread={busy:.1}%");
    }
}

/// The render thread count from this program's own first argument.
fn requested_threads() -> u32 {
    use std::env;

    env::args()
        .nth(1)
        .and_then(|argument| argument.parse().ok())
        .unwrap_or(DEFAULT_THREADS)
}

#[cfg(bela_device)]
fn main() -> Result<(), bela::Error> {
    let settings = bela::Settings::new()
        .thread_count(requested_threads())
        .cpu_monitoring(cycle());
    bela::Bela::run(Parallel::new(), &settings)
}

#[cfg(not(bela_device))]
fn main() -> ExitCode {
    eprintln!("This example must be cross-compiled for Bela Gem (aarch64-unknown-linux-gnu).");
    ExitCode::FAILURE
}
