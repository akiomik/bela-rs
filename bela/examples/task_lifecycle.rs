//! Hardware check for the auxiliary task lifecycle rules that cannot
//! be tested on the host, because they need a real audio system.
//!
//! Run by `scripts/smoke-test.sh`, which asserts on the line this
//! prints from `cleanup`. It covers three things:
//!
//! - a task created in the `setup` of an audio system that is then
//!   dropped without ever being started is retired with it, so
//!   scheduling its handle from a *later* audio system does nothing;
//! - a task belonging to the running audio system still works;
//! - creating a task from `cleanup` — which runs inside the teardown —
//!   fails instead of handing back a task that is about to be deleted.
//!
//! Cross-compile and run on the board (see docs/cross-compile.md):
//!
//! ```sh
//! cargo build -p bela --release --target aarch64-unknown-linux-gnu --example task_lifecycle
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
use std::sync::{Arc, Mutex, PoisonError};

use bela::{
    AuxiliaryTask, BelaApplication, BlockContext, CleanupContext, Error, Priority, RenderContext,
    SetupContext, ThreadInfo, rt_println,
};

const TASK_PRIORITY: Priority = Priority::new(50).expect("50 is within Bela's priority range");

/// Creates the task whose handle is then used after its audio system
/// is gone. Never started, only initialised and dropped.
struct Abandoned {
    runs: Arc<AtomicU64>,
    handle: Arc<Mutex<Option<AuxiliaryTask>>>,
}

impl BelaApplication for Abandoned {
    type RenderState = ();

    fn setup(&mut self, _context: &SetupContext) -> bool {
        let runs = Arc::clone(&self.runs);
        match AuxiliaryTask::new("bela-rs-abandoned", TASK_PRIORITY, move || {
            runs.fetch_add(1, Ordering::Relaxed);
        }) {
            Ok(task) => {
                // Hand the handle out, as a Send handle could be handed
                // to any other thread.
                *self.handle.lock().unwrap_or_else(PoisonError::into_inner) = Some(task);
                true
            }
            Err(error) => {
                rt_println!("lifecycle: could not create the abandoned task: {error}");
                false
            }
        }
    }

    fn create_render_state(&mut self, _thread: ThreadInfo, _context: &SetupContext) {}

    fn render(&self, _state: &mut (), _context: &mut RenderContext) {}
}

/// Runs for real, scheduling both the stale handle and one of its own.
struct Survivor {
    stale: Option<AuxiliaryTask>,
    stale_runs: Arc<AtomicU64>,
    fresh: Option<AuxiliaryTask>,
    fresh_runs: Arc<AtomicU64>,
    blocks: u64,
    interval: u64,
}

impl BelaApplication for Survivor {
    type RenderState = ();

    fn setup(&mut self, context: &SetupContext) -> bool {
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the sample rate is a small positive number"
        )]
        let sample_rate_hz = context.audio_sample_rate() as u64;
        self.interval = (sample_rate_hz / context.audio_frames().max(1) as u64).max(1);

        let runs = Arc::clone(&self.fresh_runs);
        self.fresh = AuxiliaryTask::new("bela-rs-survivor", TASK_PRIORITY, move || {
            runs.fetch_add(1, Ordering::Relaxed);
        })
        .ok();
        self.fresh.is_some()
    }

    fn create_render_state(&mut self, _thread: ThreadInfo, _context: &SetupContext) {}

    fn render(&self, _state: &mut (), _context: &mut RenderContext) {}

    // Real-time safe: a counter and two schedules, both of which
    // return immediately. Once per block, on the main audio thread.
    fn render_post(&mut self, _states: &mut [()], context: &mut BlockContext) {
        self.blocks += 1;
        if self.blocks % self.interval != 0 {
            return;
        }
        if let Some(stale) = &self.stale {
            stale.schedule(context);
        }
        if let Some(fresh) = &self.fresh {
            fresh.schedule(context);
        }
    }

    fn cleanup(&mut self, _states: &mut [()], _context: &CleanupContext) {
        // `cleanup` runs inside the teardown, so this must fail.
        let created = AuxiliaryTask::new("bela-rs-in-cleanup", TASK_PRIORITY, || {});
        let cleanup_create = match created {
            Err(Error::TaskCreateWhileStopping) => "rejected",
            Err(_) => "failed-otherwise",
            Ok(_) => "created",
        };
        rt_println!(
            "lifecycle: stale-runs={} fresh-runs={} cleanup-create={}",
            self.stale_runs.load(Ordering::Relaxed),
            self.fresh_runs.load(Ordering::Relaxed),
            cleanup_create
        );
    }
}

#[cfg(bela_device)]
fn main() -> Result<(), Error> {
    use bela::{Bela, Settings};

    let stale_runs = Arc::new(AtomicU64::new(0));
    let handle = Arc::new(Mutex::new(None));

    // Initialised, never started, then dropped — a teardown all the
    // same, and the task created in its setup goes with it.
    drop(Bela::new(
        Abandoned {
            runs: Arc::clone(&stale_runs),
            handle: Arc::clone(&handle),
        },
        &Settings::new(),
    )?);
    rt_println!("lifecycle: abandoned audio system dropped without starting");

    let stale = handle.lock().unwrap_or_else(PoisonError::into_inner).take();
    Bela::run(
        Survivor {
            stale,
            stale_runs,
            fresh: None,
            fresh_runs: Arc::new(AtomicU64::new(0)),
            blocks: 0,
            interval: 1,
        },
        &Settings::new(),
    )
}

#[cfg(not(bela_device))]
fn main() -> ExitCode {
    eprintln!("This example must be cross-compiled for Bela Gem (aarch64-unknown-linux-gnu).");
    ExitCode::FAILURE
}
