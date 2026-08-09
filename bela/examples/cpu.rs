//! Reports how much of the block deadline the audio thread uses, and
//! how much of that goes into one measured section of `render`.
//!
//! `render` synthesises a bank of sine oscillators, which is there to
//! use a visible and adjustable amount of CPU. `Settings::cpu_monitoring`
//! turns on Bela's own bracketing of the whole audio thread, read back
//! with `BlockContext::cpu_usage`, and a `CpuTimer` covers just the
//! oscillator bank, so the two numbers can be compared: the difference
//! is what the rest of the audio thread costs.
//!
//! The `CpuTimer` lives in the render state rather than in the
//! application, because it is the section of *this thread's* `render`
//! that it measures, and its counters are its own. With more than one
//! render thread there is one per thread, and the report reads the
//! first of them.
//!
//! The printing happens on an auxiliary task, since it is not work for
//! the audio thread. Both percentages are read in `render_post` — the
//! audio thread's counters may only be read from a callback that runs
//! on that thread — and handed to the task through atomics, which is
//! the pattern to copy.
//!
//! Cross-compile and run on the board (see docs/cross-compile.md):
//!
//! ```sh
//! cargo build -p bela --release --target aarch64-unknown-linux-gnu --example cpu
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
use core::sync::atomic::{AtomicU32, Ordering};
#[cfg(not(bela_device))]
use std::process::ExitCode;
use std::sync::Arc;

use bela::{
    AuxiliaryTask, BelaApplication, BlockContext, CleanupContext, CpuTimer, Priority,
    RenderContext, SetupContext, ThreadInfo, rt_println,
};

/// Enough oscillators to be measurable, few enough to leave headroom.
const OSCILLATORS: usize = 64;
const BASE_FREQUENCY: f32 = 110.0;
const AMPLITUDE: f32 = 0.2;

/// Blocks per acquisition cycle. At 44.1 kHz and 16 frames per block
/// that is a fresh reading roughly every 0.7 s, so a one-second report
/// always has one.
const MEASUREMENTS_PER_CYCLE: u32 = 2000;

/// The report runs below the audio thread, so it can never delay it.
const TASK_PRIORITY: Priority = Priority::new(75).expect("75 is within Bela's priority range");

/// The two percentages the audio thread publishes for the task to
/// print, each as `f32::to_bits`.
#[derive(Debug, Default)]
struct Published {
    thread: AtomicU32,
    section: AtomicU32,
}

struct Load {
    published: Arc<Published>,
    task: Option<AuxiliaryTask>,
    /// The phases the *block* starts at, advanced once per block in
    /// `render_post`, so the tone does not depend on how the block was
    /// split. See `examples/sine.rs` for the pattern on its own.
    phases: [f32; OSCILLATORS],
    phase_increments: [f32; OSCILLATORS],
    blocks: u64,
    blocks_per_report: u64,
}

/// One render thread's oscillator bank, and its measurement of it.
struct Bank {
    /// The first frame this thread writes, so that `render_pre` can
    /// seed the phases its share of the block starts at.
    first_frame: usize,
    phases: [f32; OSCILLATORS],
    /// This application's measurement of the oscillator bank alone.
    timer: CpuTimer,
}

impl Load {
    fn new() -> Self {
        Self {
            published: Arc::new(Published::default()),
            task: None,
            phases: [0.0; OSCILLATORS],
            phase_increments: [0.0; OSCILLATORS],
            blocks: 0,
            // Replaced in setup, once the block size is known.
            blocks_per_report: 1,
        }
    }
}

const fn cycle() -> NonZeroU32 {
    NonZeroU32::new(MEASUREMENTS_PER_CYCLE).expect("the cycle length is a non-zero constant")
}

impl BelaApplication for Load {
    type RenderState = Bank;

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

        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the sample rate is a small positive number"
        )]
        let sample_rate_hz = sample_rate as u64;
        self.blocks_per_report = (sample_rate_hz / context.audio_frames().max(1) as u64).max(1);

        if context.cpu_usage().is_none() {
            rt_println!("setup: CPU monitoring is off; run with Settings::cpu_monitoring");
            return false;
        }

        // The callback owns everything it touches: the percentages
        // arrive through the atomics, because the audio thread's
        // counters cannot be read from this thread.
        let published = Arc::clone(&self.published);
        let task = AuxiliaryTask::new("bela-rs-cpu", TASK_PRIORITY, move || {
            let thread = f32::from_bits(published.thread.load(Ordering::Relaxed));
            let section = f32::from_bits(published.section.load(Ordering::Relaxed));
            rt_println!("cpu: audio thread {thread:.1}%; oscillators {section:.1}%");
        });

        match task {
            Ok(task) => {
                self.task = Some(task);
                rt_println!(
                    "setup: {OSCILLATORS} oscillators over {} render thread(s), \
                     reporting every {} blocks",
                    context.thread_count(),
                    self.blocks_per_report
                );
                true
            }
            Err(error) => {
                rt_println!("setup: could not create the task: {error}");
                false
            }
        }
    }

    fn create_render_state(&mut self, thread: ThreadInfo, context: &SetupContext) -> Bank {
        Bank {
            first_frame: thread.frame_range(context.audio_frames()).start,
            phases: [0.0; OSCILLATORS],
            timer: CpuTimer::new(cycle()),
        }
    }

    // Real-time safe: arithmetic on values the states already hold.
    fn render_pre(&mut self, states: &mut [Bank], _context: &mut BlockContext) {
        for state in states {
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
    }

    // Real-time safe: arithmetic, writes to this thread's frames, and
    // a clock read through the CPU timer.
    fn render(&self, state: &mut Bank, context: &mut RenderContext) {
        // Measures until the guard is dropped at the end of this
        // scope. Entered on every block, so the period each
        // measurement is a fraction of is the block period.
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
        }
    }

    // Real-time safe: arithmetic, atomic stores and a schedule.
    fn render_post(&mut self, states: &mut [Bank], context: &mut BlockContext) {
        #[allow(
            clippy::cast_precision_loss,
            reason = "a block's frame count is far below f32's exact integer range"
        )]
        let frames = context.audio_frames() as f32;
        for (phase, increment) in self.phases.iter_mut().zip(&self.phase_increments) {
            *phase = (*phase + frames * increment) % TAU;
        }

        self.blocks += 1;
        if self.blocks % self.blocks_per_report != 0 {
            return;
        }
        // Read here, on the main audio thread, and handed to the task
        // as plain numbers.
        let thread = context.cpu_usage().map_or(0.0, |usage| usage.percentage());
        let section = states
            .first()
            .map_or(0.0, |bank| bank.timer.usage().percentage());
        self.published
            .thread
            .store(thread.to_bits(), Ordering::Relaxed);
        self.published
            .section
            .store(section.to_bits(), Ordering::Relaxed);
        if let Some(task) = &self.task {
            task.schedule(context);
        }
    }

    fn cleanup(&mut self, states: &mut [Bank], context: &CleanupContext) {
        // Sound from `cleanup` too: libbela has joined the audio thread
        // by the time this runs, so nothing is writing the counters.
        if let Some(usage) = context.cpu_usage() {
            rt_println!("cleanup: audio thread {usage}");
        }
        if let Some(bank) = states.first() {
            rt_println!(
                "cleanup: oscillators {}; {} blocks rendered",
                bank.timer.usage(),
                self.blocks
            );
        }
    }
}

/// Checks that monitoring is refused at a period size where libbela
/// would run `render` on its FIFO thread, away from the thread that
/// updates the counters. Only the board can tell: the split happens
/// inside libbela, and nothing in the context reveals it.
#[cfg(bela_device)]
fn report_fifo_guard() {
    let settings = bela::Settings::new()
        .cpu_monitoring(cycle())
        .period_size(bela::MAX_MONITORED_PERIOD_SIZE * 2);
    let outcome = match bela::Bela::new(Load::new(), &settings) {
        Err(bela::Error::CpuMonitoringPeriodSize(frames)) => format!("refused at {frames} frames"),
        Err(error) => format!("other-error {error}"),
        // Dropped immediately, which tears the audio system down again.
        Ok(_) => "accepted".to_owned(),
    };
    println!("fifo-guard: {outcome}");
}

#[cfg(bela_device)]
fn main() -> Result<(), bela::Error> {
    report_fifo_guard();
    bela::Bela::run(Load::new(), &bela::Settings::new().cpu_monitoring(cycle()))
}

#[cfg(not(bela_device))]
fn main() -> ExitCode {
    eprintln!("This example must be cross-compiled for Bela Gem (aarch64-unknown-linux-gnu).");
    ExitCode::FAILURE
}
