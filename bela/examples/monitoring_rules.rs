//! Hardware checks for the audio system rules that cannot be tested on
//! the host, because they need a real audio system — and, in one case,
//! because only libbela knows the answer.
//!
//! Run by `scripts/smoke-test.sh`, which invokes it once per check and
//! asserts on what that run prints. It covers five things:
//!
//! - **the period size limit is the right one** (`fifo-probe`). Above a
//!   hardware-dependent period size libbela renders on a thread of its
//!   own (`fifoLoop`) while the CPU counters stay with the core audio
//!   thread, which is why `Settings::cpu_monitoring` is refused past
//!   `MAX_MONITORED_PERIOD_SIZE`. That constant is a measured board
//!   fact, so it is measured again here: libbela prints `gFifoFactor`
//!   under `Settings::verbose`, and it has to be 1 at the limit and
//!   more than 1 just above it. Without this the limit could drift from
//!   the hardware and nothing would notice — the refusal itself would
//!   keep passing, since it only consults the constant.
//! - **only one audio system exists at a time** (`second-new`). A
//!   second `Bela::new` has to be refused, because setting an audio
//!   system up reaches into libbela's globals — including the CPU
//!   counters, before `Bela_initAudio` gets a chance to object.
//! - **an unset `cpu_monitoring` means off** (`monitoring`), not
//!   "whatever the last audio system left behind". Stale counters would
//!   report a reading nobody asked for, and would skip the period size
//!   check that goes with asking for one.
//! - **a failed initialisation refuses the next one** (`poisoned`).
//!   `Bela_initAudio` failing leaves libbela believing an audio system
//!   is up, with no call that puts it back, so `Bela::new` gives up on
//!   the process rather than letting the next attempt segfault inside
//!   libbela. Only a board can tell the refusal from the segfault it
//!   replaces: on the host there is no `Bela_initAudio` to fail.
//! - **a configuration the application refuses costs it nothing**
//!   (`validate-settings`). `BelaApplication::validate_settings` is
//!   asked before `Bela_initAudio`, so declining there has to leave the
//!   process able to build an audio system that does run — which is the
//!   whole difference from declining in `setup`, the check above. Only a
//!   board can show the second half of that: on the host there is no
//!   audio system to bring up afterwards.
//!
//! # One check per run
//!
//! Each invocation brings up at most one audio system and exits, with
//! `validate-settings` the exception that proves the rule: it is about
//! two attempts in one process, and it is only able to make them
//! because the first was refused before libbela was asked anything.
//! Three of the other checks abort the initialisation from `setup`, and
//! a failed `Bela_initAudio` leaves libbela's globals in that process
//! still believing the audio system is up, with no call that puts them
//! back (`docs/board-facts.md`). `Bela::new` gives up on such a
//! process, so a second check sharing it would be refused rather than
//! run.
//!
//! That refusal is why the arrangement is now only an arrangement.
//! Before it, the second check was the segfault itself, and running one
//! per process was what kept the suite alive. Separate processes also
//! make the output unambiguous, since libbela's C `printf` and Rust's
//! `println!` buffer independently.
//!
//! Cross-compile and run on the board (see docs/cross-compile.md):
//!
//! ```sh
//! cargo build -p bela --release --target aarch64-unknown-linux-gnu --example monitoring_rules
//! ```

#![cfg_attr(
    not(bela_device),
    allow(
        dead_code,
        reason = "only the fallback main is reachable off-device; the application code should still compile and lint"
    )
)]

use core::num::NonZeroU32;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::process::ExitCode;
use std::sync::Arc;

use bela::{
    BelaApplication, BlockContext, RenderContext, ResolvedSettings, SetupContext, ThreadInfo,
};

const MEASUREMENTS_PER_CYCLE: u32 = 2000;

/// How many render threads the application below insists on, which is
/// more than Bela's default of one, so that the same program is
/// refused for one configuration and runs for another.
const REQUIRED_THREADS: NonZeroU32 =
    NonZeroU32::new(4).expect("the required thread count is a non-zero constant");

/// What it says when it is not given them.
const WRONG_THREAD_COUNT: &str = "this application renders on four threads";

/// How long the accepted audio system is left rendering, which only has
/// to be long enough for blocks to have been counted.
const RENDER_MILLIS: u64 = 500;

const fn cycle() -> NonZeroU32 {
    NonZeroU32::new(MEASUREMENTS_PER_CYCLE).expect("the cycle length is a non-zero constant")
}

/// Records whether monitoring was on when `setup` ran — the earliest a
/// callback can look — and then stops the audio system from coming up,
/// since the answer is already in hand.
struct Observe {
    monitored: Arc<AtomicBool>,
}

impl BelaApplication for Observe {
    type RenderState = ();

    fn setup(&mut self, context: &SetupContext) -> bool {
        self.monitored
            .store(context.cpu_usage().is_some(), Ordering::Relaxed);
        false
    }

    fn create_render_state(&mut self, _thread: ThreadInfo, _context: &SetupContext) {}

    fn render(&self, _state: &mut (), _context: &mut RenderContext) {}
}

/// Stops an audio system from coming up, for the probe that only needs
/// libbela to have got as far as choosing a `gFifoFactor`.
struct Abort;

impl BelaApplication for Abort {
    type RenderState = ();

    fn setup(&mut self, _context: &SetupContext) -> bool {
        false
    }

    fn create_render_state(&mut self, _thread: ThreadInfo, _context: &SetupContext) {}

    fn render(&self, _state: &mut (), _context: &mut RenderContext) {}
}

/// Will only run on [`REQUIRED_THREADS`] render threads, and says so
/// from `validate_settings` rather than from `setup`.
///
/// Counts the blocks it renders, so that a run which was accepted can
/// be told from one that came up and did nothing.
struct NeedsFourThreads {
    blocks: Arc<AtomicU32>,
}

impl BelaApplication for NeedsFourThreads {
    type RenderState = ();

    fn validate_settings(&self, settings: &ResolvedSettings<'_>) -> Result<(), &'static str> {
        if settings.thread_count() == REQUIRED_THREADS.get() as usize {
            Ok(())
        } else {
            Err(WRONG_THREAD_COUNT)
        }
    }

    fn create_render_state(&mut self, _thread: ThreadInfo, _context: &SetupContext) {}

    fn render_pre(&mut self, _states: &mut [()], _context: &mut BlockContext) {
        self.blocks.fetch_add(1, Ordering::Relaxed);
    }

    fn render(&self, _state: &mut (), _context: &mut RenderContext) {}
}

/// Does nothing, for the check that only needs an audio system to
/// exist.
struct Idle;

impl BelaApplication for Idle {
    type RenderState = ();

    fn create_render_state(&mut self, _thread: ThreadInfo, _context: &SetupContext) {}

    fn render(&self, _state: &mut (), _context: &mut RenderContext) {}
}

#[cfg(bela_device)]
mod checks {
    use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use core::time::Duration;
    use std::sync::Arc;
    use std::thread;

    use bela::{Bela, Error, Settings};

    use super::{
        Abort, Idle, NeedsFourThreads, Observe, RENDER_MILLIS, REQUIRED_THREADS,
        WRONG_THREAD_COUNT, cycle,
    };

    /// Starts initialising audio at `period_size` with libbela's own
    /// logging on, so that it prints the `gFifoFactor` it chose, and
    /// aborts in `setup` once it has.
    ///
    /// Deliberately without `cpu_monitoring`: asking for it above the
    /// limit would be refused before `Bela_initAudio` ran, and libbela
    /// would never print the number being checked.
    ///
    /// Prints nothing itself — the reading is libbela's own output, and
    /// this run is the only thing producing it.
    pub(crate) fn fifo_probe(period_size: u32) {
        let settings = Settings::new().period_size(period_size).verbose(true);
        // `Err` is expected: `Abort` refuses in `setup`.
        drop(Bela::new(Abort, &settings));
    }

    /// A second audio system must be refused while the first is alive.
    pub(crate) fn second_new() {
        let outcome = match Bela::new(Idle, &Settings::new()) {
            Err(error) => format!("first-new-failed-{error}"),
            Ok(first) => {
                let second = match Bela::new(Idle, &Settings::new()) {
                    Err(Error::AudioSystemExists) => "refused",
                    Err(_) => "failed-otherwise",
                    Ok(_) => "created",
                };
                // Only now, so the second attempt really did overlap it.
                drop(first);
                second.to_owned()
            }
        };
        println!("rules: second-new={outcome}");
    }

    /// Reports whether `setup` saw monitoring on, with it either asked
    /// for or left unset.
    pub(crate) fn monitoring(requested: bool) {
        let settings = if requested {
            Settings::new().cpu_monitoring(cycle())
        } else {
            Settings::new()
        };
        let monitored = Arc::new(AtomicBool::new(false));
        let app = Observe {
            monitored: Arc::clone(&monitored),
        };
        // `Observe` aborts in `setup`, so a failed init is the expected
        // outcome and means the observation was taken.
        let observed = match Bela::new(app, &settings) {
            Err(Error::Init(_)) => true,
            Err(_) => false,
            Ok(bela) => {
                drop(bela);
                true
            }
        };
        let seen = if !observed {
            "setup-not-reached"
        } else if monitored.load(Ordering::Relaxed) {
            "some"
        } else {
            "none"
        };
        println!("rules: monitoring={seen}");
    }

    /// An application that refuses a configuration must be able to run
    /// under another one in the same process.
    ///
    /// The first `Bela::new` is given Bela's default single render
    /// thread, which `NeedsFourThreads` declines; the second asks for
    /// the four it wants and is then started, so what this reports is a
    /// refusal followed by rendered blocks. Both configurations are
    /// ones libbela runs, which is deliberate: a hook that wrongly
    /// accepted the first would produce a working audio system here
    /// rather than a poisoned process, and the two are told apart by
    /// the refusal being missing rather than by the run dying.
    pub(crate) fn validate_settings() {
        let blocks = Arc::new(AtomicU32::new(0));
        let refusing = NeedsFourThreads {
            blocks: Arc::clone(&blocks),
        };
        let refused = match Bela::new(refusing, &Settings::new()) {
            Err(Error::SettingsRefused(reason)) if reason == WRONG_THREAD_COUNT => "refused",
            Err(Error::SettingsRefused(_)) => "refused-with-another-reason",
            Err(_) => "failed-otherwise",
            Ok(bela) => {
                drop(bela);
                "created"
            }
        };

        // The point of the refusal above having cost nothing: this is
        // the same process, and it has to get a working audio system.
        let accepting = NeedsFourThreads {
            blocks: Arc::clone(&blocks),
        };
        let settings = Settings::new().thread_count(REQUIRED_THREADS);
        let ran = match Bela::new(accepting, &settings) {
            Err(error) => format!("failed-{error}"),
            Ok(mut bela) => {
                if let Err(error) = bela.start() {
                    format!("start-failed-{error}")
                } else {
                    thread::sleep(Duration::from_millis(RENDER_MILLIS));
                    bela.stop();
                    format!("blocks-{}", blocks.load(Ordering::Relaxed))
                }
            }
        };
        println!("rules: settings-refusal={refused} then-audio={ran}");
    }

    /// Once an initialisation has failed, every later one in the same
    /// process must be refused rather than attempted.
    ///
    /// This is the check the refusal exists for. Without it the second
    /// `Bela::new` is the segfault recorded in `docs/board-facts.md`,
    /// so a run that gets as far as printing its line has already
    /// demonstrated most of what is being asserted.
    pub(crate) fn poisoned() {
        // `Abort` refuses in `setup`, which fails `Bela_initAudio`
        // after libbela has brought the hardware up — the state there
        // is no way back from.
        let first = match Bela::new(Abort, &Settings::new()) {
            Err(Error::Init(_)) => "failed",
            Err(_) => "failed-otherwise",
            Ok(bela) => {
                drop(bela);
                "created"
            }
        };
        let second = match Bela::new(Idle, &Settings::new()) {
            Err(Error::AudioSystemPoisoned) => "refused",
            Err(_) => "failed-otherwise",
            Ok(bela) => {
                drop(bela);
                "created"
            }
        };
        println!("rules: first-init={first} poisoned-new={second}");
    }
}

#[cfg(bela_device)]
fn main() -> ExitCode {
    use std::env::args;

    let arguments: Vec<String> = args().skip(1).collect();
    let check: Vec<&str> = arguments.iter().map(String::as_str).collect();
    match check.as_slice() {
        ["fifo-probe", frames] => {
            let Ok(frames) = frames.parse() else {
                eprintln!("fifo-probe takes a period size in frames, not {frames:?}");
                return ExitCode::FAILURE;
            };
            checks::fifo_probe(frames);
        }
        ["second-new"] => checks::second_new(),
        ["monitoring", "on"] => checks::monitoring(true),
        ["monitoring", "off"] => checks::monitoring(false),
        ["poisoned"] => checks::poisoned(),
        ["validate-settings"] => checks::validate_settings(),
        _ => {
            eprintln!(
                "usage: monitoring_rules (fifo-probe <frames> | second-new | monitoring on|off\n\
                 \x20                       | poisoned | validate-settings)\n\
                 one check per run: three of these abort from `setup`, which makes \
                 `Bela::new` give up on it"
            );
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}

#[cfg(not(bela_device))]
fn main() -> ExitCode {
    eprintln!("This example must be cross-compiled for Bela Gem (aarch64-unknown-linux-gnu).");
    ExitCode::FAILURE
}
