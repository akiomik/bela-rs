//! Hardware checks for the audio system rules that cannot be tested on
//! the host, because they need a real audio system — and, in one case,
//! because only libbela knows the answer.
//!
//! Run by `scripts/smoke-test.sh`, which invokes it once per check and
//! asserts on what that run prints. It covers four things:
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
//!
//! # One check per run
//!
//! Each invocation brings up at most one audio system and exits. Three
//! of these checks abort the initialisation from `setup`, and a failed
//! `Bela_initAudio` leaves libbela's globals in that process still
//! believing the audio system is up, with no call that puts them back
//! (`docs/board-facts.md`). `Bela::new` gives up on such a process, so
//! a second check sharing it would be refused rather than run.
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
use core::sync::atomic::{AtomicBool, Ordering};
use std::process::ExitCode;
use std::sync::Arc;

use bela::{BelaApplication, Context};

const MEASUREMENTS_PER_CYCLE: u32 = 2000;

const fn cycle() -> NonZeroU32 {
    NonZeroU32::new(MEASUREMENTS_PER_CYCLE).expect("the cycle length is a non-zero constant")
}

/// Records whether monitoring was on when `setup` ran — the earliest a
/// callback can look — and then stops the audio system from coming up,
/// since the answer is already in hand.
struct Observe {
    monitored: Arc<AtomicBool>,
}

unsafe impl BelaApplication for Observe {
    fn setup(&mut self, context: &mut Context) -> bool {
        self.monitored
            .store(context.cpu_usage().is_some(), Ordering::Relaxed);
        false
    }

    fn render(&mut self, _context: &mut Context) {}
}

/// Stops an audio system from coming up, for the probe that only needs
/// libbela to have got as far as choosing a `gFifoFactor`.
struct Abort;

unsafe impl BelaApplication for Abort {
    fn setup(&mut self, _context: &mut Context) -> bool {
        false
    }

    fn render(&mut self, _context: &mut Context) {}
}

/// Does nothing, for the check that only needs an audio system to
/// exist.
struct Idle;

unsafe impl BelaApplication for Idle {
    fn render(&mut self, _context: &mut Context) {}
}

#[cfg(bela_device)]
mod checks {
    use core::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use bela::{Bela, Error, Settings};

    use super::{Abort, Idle, Observe, cycle};

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
    pub fn fifo_probe(period_size: u32) {
        let settings = Settings::new().period_size(period_size).verbose(true);
        // `Err` is expected: `Abort` refuses in `setup`.
        drop(Bela::new(Abort, &settings));
    }

    /// A second audio system must be refused while the first is alive.
    pub fn second_new() {
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
    pub fn monitoring(requested: bool) {
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

    /// Once an initialisation has failed, every later one in the same
    /// process must be refused rather than attempted.
    ///
    /// This is the check the refusal exists for. Without it the second
    /// `Bela::new` is the segfault recorded in `docs/board-facts.md`,
    /// so a run that gets as far as printing its line has already
    /// demonstrated most of what is being asserted.
    pub fn poisoned() {
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
        _ => {
            eprintln!(
                "usage: monitoring_rules (fifo-probe <frames> | second-new | monitoring on|off\n\
                 \x20                       | poisoned)\n\
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
