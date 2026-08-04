//! Moves non-real-time work off the audio thread with an auxiliary
//! task: `render` counts blocks and, once a second, asks a
//! lower-priority task to report — including work that allocates,
//! which `render` itself must never do.
//!
//! Cross-compile and run on the board (see docs/cross-compile.md):
//!
//! ```sh
//! cargo build -p bela --release --target aarch64-unknown-linux-gnu --example aux_task
//! ```

#![cfg_attr(
    not(bela_device),
    allow(
        dead_code,
        reason = "only the fallback main is reachable off-device; the application code should still compile and lint"
    )
)]

use core::sync::atomic::{AtomicU64, Ordering};
#[cfg(not(bela_device))]
use std::process::ExitCode;
use std::sync::Arc;

use bela::{AUDIO_PRIORITY, AuxiliaryTask, BelaApplication, Context, rt_println};

/// The task runs below the audio thread, so a slow report can never
/// delay rendering.
const TASK_PRIORITY: i32 = AUDIO_PRIORITY - 20;

struct Report {
    task: Option<AuxiliaryTask>,
    /// Written by `render`, read by the task: the only thing the two
    /// threads share.
    blocks: Arc<AtomicU64>,
    /// How many blocks between reports; set in `setup`.
    interval: u64,
    /// Counted by the task, so `cleanup` can compare it with the
    /// number of requests: a request that arrives while the task is
    /// still running is silently lost.
    runs: Arc<AtomicU64>,
    /// Counted in `render`, so it needs no synchronisation.
    requests: u64,
}

impl Report {
    fn new() -> Self {
        Self {
            task: None,
            blocks: Arc::new(AtomicU64::new(0)),
            interval: 1,
            runs: Arc::new(AtomicU64::new(0)),
            requests: 0,
        }
    }
}

// Safety: render only counts, stores to an atomic and schedules the
// task — no allocation, blocking, system calls or panicking code paths.
// The reporting itself happens on the task's thread.
unsafe impl BelaApplication for Report {
    fn setup(&mut self, context: &mut Context) -> bool {
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the sample rate is a small positive number"
        )]
        let sample_rate_hz = context.audio_sample_rate() as u64;
        self.interval = (sample_rate_hz / context.audio_frames().max(1) as u64).max(1);

        // The callback owns everything it touches: it cannot borrow
        // from the application, which the audio thread holds by &mut
        // while the task runs.
        let blocks = Arc::clone(&self.blocks);
        let runs = Arc::clone(&self.runs);
        let task = AuxiliaryTask::new("bela-rs-report", TASK_PRIORITY, move || {
            runs.fetch_add(1, Ordering::Relaxed);
            let count = blocks.load(Ordering::Relaxed);
            // Allocating here is the point of the exercise: this is a
            // normal thread, so it may do what `render` may not.
            let bar = "#".repeat((count / 10_000) as usize + 1);
            rt_println!("task: {count} blocks {bar}");
        });

        match task {
            Ok(task) => {
                self.task = Some(task);
                rt_println!("setup: reporting every {} blocks", self.interval);
                true
            }
            Err(error) => {
                rt_println!("setup: could not create the task: {error}");
                false
            }
        }
    }

    fn render(&mut self, context: &mut Context) {
        let blocks = self.blocks.fetch_add(1, Ordering::Relaxed) + 1;
        if blocks % self.interval != 0 {
            return;
        }
        if let Some(task) = &self.task {
            task.schedule(context);
            self.requests += 1;
        }
    }

    fn cleanup(&mut self, _context: &mut Context) {
        rt_println!(
            "cleanup: {} blocks, {} requests, {} task runs",
            self.blocks.load(Ordering::Relaxed),
            self.requests,
            self.runs.load(Ordering::Relaxed)
        );
    }
}

#[cfg(bela_device)]
fn main() -> Result<(), bela::Error> {
    bela::Bela::run(Report::new(), &bela::Settings::new())
}

#[cfg(not(bela_device))]
fn main() -> ExitCode {
    eprintln!("This example must be cross-compiled for Bela Gem (aarch64-unknown-linux-gnu).");
    ExitCode::FAILURE
}
