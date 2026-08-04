//! Reports how much of the block deadline the audio thread uses, and
//! how much of that goes into one measured section of `render`.
//!
//! `render` synthesises a bank of sine oscillators, which is there to
//! use a visible and adjustable amount of CPU. A `CpuMonitor` covers
//! the whole audio thread — Bela's own bracketing of the block — and a
//! `CpuTimer` covers just the oscillator bank, so the two numbers can
//! be compared: the difference is what the rest of the audio thread
//! costs.
//!
//! The reporting happens on an auxiliary task, since printing is not
//! work for the audio thread. The section percentage is handed over
//! through an atomic, the whole-thread one is read straight from
//! libbela by the task.
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
    AUDIO_PRIORITY, AuxiliaryTask, BelaApplication, Context, CpuMonitor, CpuTimer, rt_println,
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
const TASK_PRIORITY: i32 = AUDIO_PRIORITY - 20;

struct Load {
    /// Bela's own measurement of the whole audio thread.
    monitor: Option<CpuMonitor>,
    /// This application's measurement of the oscillator bank alone.
    timer: CpuTimer,
    /// The section percentage, as `f32::to_bits`, published by `render`
    /// and read by the task.
    section: Arc<AtomicU32>,
    task: Option<AuxiliaryTask>,
    phases: [f32; OSCILLATORS],
    phase_increments: [f32; OSCILLATORS],
    blocks: u64,
    blocks_per_report: u64,
}

impl Load {
    fn new() -> Self {
        Self {
            monitor: None,
            timer: CpuTimer::new(
                NonZeroU32::new(MEASUREMENTS_PER_CYCLE).expect("the cycle length is a constant"),
            ),
            section: Arc::new(AtomicU32::new(0)),
            task: None,
            phases: [0.0; OSCILLATORS],
            phase_increments: [0.0; OSCILLATORS],
            blocks: 0,
            // Replaced in setup, once the block size is known.
            blocks_per_report: 1,
        }
    }
}

// Safety: render does arithmetic, writes to the context buffers, reads
// a clock through the CPU timer and stores to an atomic — no
// allocation, blocking, system calls or panicking code paths. The
// printing happens on the task's thread.
unsafe impl BelaApplication for Load {
    fn setup(&mut self, context: &mut Context) -> bool {
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

        // Before audio starts, which is what enabling requires: `setup`
        // runs inside Bela_initAudio, and the audio thread only decides
        // whether monitoring is on when Bela_startAudio spawns it.
        let cycle =
            NonZeroU32::new(MEASUREMENTS_PER_CYCLE).expect("the cycle length is a constant");
        match CpuMonitor::enable(cycle) {
            Ok(monitor) => self.monitor = Some(monitor),
            Err(error) => {
                rt_println!("setup: could not enable CPU monitoring: {error}");
                return false;
            }
        }

        // The task owns what it prints: the monitor is a copyable
        // token, and the section percentage arrives through the atomic.
        let monitor = self.monitor;
        let section = Arc::clone(&self.section);
        let task = AuxiliaryTask::new("bela-rs-cpu", TASK_PRIORITY, move || {
            let section = f32::from_bits(section.load(Ordering::Relaxed));
            if let Some(monitor) = monitor {
                rt_println!(
                    "cpu: audio thread {}; oscillators {:.1}%",
                    monitor.usage(),
                    section
                );
            }
        });

        match task {
            Ok(task) => {
                self.task = Some(task);
                rt_println!(
                    "setup: {OSCILLATORS} oscillators, reporting every {} blocks",
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

    fn render(&mut self, context: &mut Context) {
        {
            // Measures until the guard is dropped at the end of this
            // scope. Entered on every block, so the period each
            // measurement is a fraction of is the block period.
            let _oscillators = self.timer.measure();

            let channels = context.audio_out_channels();
            for frame in 0..context.audio_frames() {
                let mut sample = 0.0;
                for (phase, increment) in self.phases.iter_mut().zip(&self.phase_increments) {
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

        self.blocks += 1;
        if self.blocks % self.blocks_per_report != 0 {
            return;
        }
        self.section
            .store(self.timer.usage().percentage().to_bits(), Ordering::Relaxed);
        if let Some(task) = &self.task {
            task.schedule(context);
        }
    }

    fn cleanup(&mut self, _context: &mut Context) {
        let section = self.timer.usage();
        if let Some(monitor) = self.monitor {
            rt_println!("cleanup: audio thread {}", monitor.usage());
        }
        rt_println!(
            "cleanup: oscillators {section}; {} blocks rendered",
            self.blocks
        );
    }
}

#[cfg(bela_device)]
fn main() -> Result<(), bela::Error> {
    bela::Bela::run(Load::new(), &bela::Settings::new())
}

#[cfg(not(bela_device))]
fn main() -> ExitCode {
    eprintln!("This example must be cross-compiled for Bela Gem (aarch64-unknown-linux-gnu).");
    ExitCode::FAILURE
}
