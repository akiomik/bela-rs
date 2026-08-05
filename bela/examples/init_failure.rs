//! Hardware probe for what a failed `Bela_initAudio` leaves behind.
//!
//! Written to answer #30: what a failed `Bela_initAudio` leaves behind,
//! and whether handing it back is possible. It is not, so `Bela::new`
//! now refuses every attempt after a failed one rather than segfaulting
//! — and this probe stays as the instrument, because the answers are
//! board facts and a board image can change them.
//! `scripts/probe-init-failure.sh` drives it, and what it found is
//! recorded under "Audio thread" in `docs/board-facts.md`.
//!
//! It is not a check with a right answer, so it is not part of
//! `scripts/smoke-test.sh`: it deliberately puts the board into the
//! state that crashes, and a pass/fail gate should not depend on how
//! that goes.
//!
//! # What each run does
//!
//! - `render-check [seconds]` — one full cycle: bring an audio system
//!   up, render, tear it down, report the block count. Used as the
//!   oracle, in a process of its own, to ask whether the board is still
//!   usable after something else has run — and, with `seconds`, as the
//!   process that holds the device for `busy-probe`.
//! - `abort` — fail an initialisation from `setup`, which is the
//!   reachable way to fail it *after* the hardware is up.
//! - `abort-cleanup` — the same, then call `Bela_cleanupAudio` directly.
//!   This is the call `Bela::new` does not make, and the one question
//!   the fix turns on: does handing the state back work, or is the call
//!   itself what crashes? From outside the crate it necessarily happens
//!   after `Bela::new` has freed the application, which is not the
//!   order a fix would use.
//! - `raw-cleanup` — the same question in the order a fix *would* use:
//!   straight through the C API, mirroring `Bela::new` up to the
//!   failing `Bela_initAudio`, then `Bela_cleanupAudio` with the
//!   application still alive and a `cleanup` callback that reaches into
//!   it. Without this, `abort-cleanup` alone cannot tell a call that
//!   crashes from an arrangement that does.
//! - `abort-then-new` — fail an initialisation, then try a full cycle
//!   in the same process. This is the sequence `Bela::new` now refuses:
//!   it reports `failed-AudioSystemPoisoned` and exits 0 where it used
//!   to segfault. Kept as the record of that, and as the thing to run
//!   against a new board image.
//! - `abort-cleanup-then-new` — fail an initialisation, hand the state
//!   back, then try a full cycle. This was the candidate fix. It never
//!   reaches the cycle: handing the state back is itself the crash,
//!   which is what ruled it out.
//! - `busy-probe <seconds>` — try a cycle while another process is
//!   using the audio device, wait for it to go, and try again. Not
//!   every `Error::Init` need be an abort from `setup`; "the hardware
//!   is already in use" is the other one the API documents, and whether
//!   *it* poisons the process would have decided whether the refusal
//!   has to be unconditional. This board does not produce that failure
//!   at all — it does not refuse a second process — so the refusal is
//!   unconditional for want of a second route to measure.
//! - `cycles <count>` — bring an audio system all the way up, render,
//!   and tear it down again, `count` times.
//! - `init-cycles <count>` — build an audio system and drop it again
//!   without ever starting audio, `count` times. An earlier board note
//!   put a bus error at four or five of these; this is the shape it
//!   described, and it did not reproduce.
//!
//! # One process per run
//!
//! Each invocation reports what it managed, line by line and flushed as
//! it goes, because it may not survive to the end — a process that dies
//! inside `Bela_cleanupAudio` still has to have said that it got there.
//! For the same reason the interesting question is often asked of the
//! *next* process rather than this one. That was #30's leading
//! hypothesis — that what breaks an audio system is what the process
//! before it left behind — and asking it this way is what refuted it:
//! the next process is always fine.
//!
//! Cross-compile and run on the board (see docs/cross-compile.md):
//!
//! ```sh
//! cargo build -p bela --release --target aarch64-unknown-linux-gnu --example init_failure
//! ```

#![cfg_attr(
    not(bela_device),
    allow(
        dead_code,
        reason = "only the fallback main is reachable off-device; the probe code should still compile and lint"
    )
)]

use core::sync::atomic::{AtomicU32, Ordering};
use std::process::ExitCode;
use std::sync::Arc;

use bela::{BelaApplication, Context};

/// Refuses to start, failing `Bela_initAudio` from inside itself —
/// after libbela has brought the hardware up.
struct Abort;

unsafe impl BelaApplication for Abort {
    fn setup(&mut self, _context: &mut Context) -> bool {
        false
    }

    fn render(&mut self, _context: &mut Context) {}
}

/// Does nothing, for the cycles that never start audio at all.
struct Idle;

unsafe impl BelaApplication for Idle {
    fn render(&mut self, _context: &mut Context) {}
}

/// Counts the blocks it renders, so that an audio system which really
/// ran can be told from one that only came up.
struct Count {
    blocks: Arc<AtomicU32>,
}

unsafe impl BelaApplication for Count {
    fn render(&mut self, _context: &mut Context) {
        self.blocks.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(bela_device)]
mod probes {
    use core::ffi::c_void;
    use core::sync::atomic::{AtomicU32, Ordering};
    use core::time::Duration;
    use std::io::{self, Write};
    use std::sync::Arc;
    use std::thread;

    use bela::bela_sys::{
        Bela_InitSettings_alloc, Bela_InitSettings_free, Bela_cleanupAudio, Bela_defaultSettings,
        Bela_initAudio, BelaContext,
    };
    use bela::{Bela, Error, Settings};

    use super::{Abort, Count, Idle};

    /// How long a cycle renders before it is torn down. Long enough
    /// that a working audio system reports thousands of blocks, so that
    /// "came up but never rendered" cannot be mistaken for it.
    const RENDER_TIME: Duration = Duration::from_secs(1);

    /// Reports one step, on the spot.
    ///
    /// Flushed, because stdout is a pipe when the driving script is
    /// listening and the next call may be the one that kills the
    /// process: what it managed to do has to be out of the buffer
    /// before it tries.
    fn report(key: &str, value: &str) {
        println!("init-failure: {key}={value}");
        let _ = io::stdout().flush();
    }

    /// Brings an audio system up, renders for `render_time`, tears it
    /// down, and reports how many blocks it saw.
    fn cycle_for(render_time: Duration) -> Result<u32, Error> {
        let blocks = Arc::new(AtomicU32::new(0));
        let app = Count {
            blocks: Arc::clone(&blocks),
        };
        let mut bela = Bela::new(app, &Settings::new())?;
        bela.start()?;
        thread::sleep(render_time);
        drop(bela);
        Ok(blocks.load(Ordering::Relaxed))
    }

    /// A cycle of the default length, which is what every probe but the
    /// device holder wants.
    fn cycle() -> Result<u32, Error> {
        cycle_for(RENDER_TIME)
    }

    /// How a cycle went, in one word: the script matches on these, so
    /// none of them contains a space.
    fn outcome(result: Result<u32, Error>) -> String {
        match result {
            Ok(0) => "up-but-silent".to_owned(),
            Ok(blocks) => format!("rendered-{blocks}"),
            Err(error) => format!("failed-{error:?}"),
        }
    }

    /// Fails an initialisation from `setup` and says how it failed.
    fn abort_init() -> String {
        match Bela::new(Abort, &Settings::new()) {
            Err(error) => format!("failed-{error:?}"),
            Ok(bela) => {
                // Not what this probe is for, but the run should not
                // leave an audio system behind on the way out.
                drop(bela);
                "created".to_owned()
            }
        }
    }

    /// Hands whatever the failed initialisation took back to libbela.
    ///
    /// The call `Bela::new` does not make. Reported either side, so a
    /// process that dies inside it is distinguishable from one that
    /// survives it.
    ///
    /// From out here this necessarily happens *after* `Bela::new` has
    /// freed the application, which is not the order a fix inside it
    /// would use. [`raw_cleanup`] measures that order instead.
    fn hand_back() {
        report("cleanup", "calling");
        unsafe { Bela_cleanupAudio() };
        report("cleanup", "returned");
    }

    /// The oracle: is an audio system still possible in a fresh
    /// process?
    ///
    /// `render_time` is also how long the device stays held, which is
    /// what makes this the holder for `busy-probe`.
    pub fn render_check(render_time: Option<Duration>) {
        report(
            "cycle",
            &outcome(cycle_for(render_time.unwrap_or(RENDER_TIME))),
        );
    }

    /// One aborted initialisation and nothing else.
    pub fn abort() {
        report("abort", &abort_init());
    }

    /// An aborted initialisation, then the call that might have undone
    /// it — from outside the crate, so after the application is gone.
    pub fn abort_cleanup() {
        report("abort", &abort_init());
        hand_back();
    }

    /// An aborted initialisation, then another audio system in the same
    /// process with nothing handed back.
    pub fn abort_then_new() {
        report("abort", &abort_init());
        report("second", &outcome(cycle()));
    }

    /// An aborted initialisation, the state handed back, then another
    /// audio system in the same process.
    pub fn abort_cleanup_then_new() {
        report("abort", &abort_init());
        hand_back();
        report("second", &outcome(cycle()));
    }

    /// An application with a real allocation behind it and a `cleanup`
    /// that reaches into it.
    ///
    /// [`raw_cleanup`] needs both. `Abort` is zero-sized with the
    /// default do-nothing `cleanup`, so a callback fired over it would
    /// touch no memory and report nothing — exactly the case that
    /// cannot fail, which is not the one worth measuring.
    struct RawApp {
        marker: [u8; 64],
        cleaned: bool,
    }

    #[expect(
        clippy::missing_const_for_fn,
        reason = "a function pointer handed to C, which cannot be const-evaluated"
    )]
    unsafe extern "C" fn raw_setup(_context: *mut BelaContext, _user_data: *mut c_void) -> bool {
        false
    }

    #[expect(
        clippy::missing_const_for_fn,
        reason = "a function pointer handed to C, which cannot be const-evaluated"
    )]
    unsafe extern "C" fn raw_render(_context: *mut BelaContext, _user_data: *mut c_void) {}

    unsafe extern "C" fn raw_cleanup_callback(_context: *mut BelaContext, user_data: *mut c_void) {
        let app = unsafe { &mut *user_data.cast::<RawApp>() };
        app.cleaned = true;
        report("cleanup-callback", &format!("ran-marker-{}", app.marker[0]));
    }

    /// `Bela_cleanupAudio` after a failed `Bela_initAudio`, in the order
    /// a fix inside `Bela::new` would use it.
    ///
    /// [`abort_cleanup`] can only make that call from outside the
    /// crate, which is necessarily after `Bela::new` has freed the
    /// application — so if the callback `Bela_cleanupAudio` fires were
    /// the problem, that probe would be measuring its own arrangement
    /// rather than the call. This one goes through the C API directly,
    /// mirroring what `Bela::new` does up to and including the failing
    /// `Bela_initAudio`, and hands the state back with the application
    /// still alive.
    pub fn raw_cleanup() {
        let app = Box::into_raw(Box::new(RawApp {
            marker: [7; 64],
            cleaned: false,
        }));
        let ret = unsafe {
            let raw = Bela_InitSettings_alloc();
            Bela_defaultSettings(raw);
            (*raw).setup = Some(raw_setup);
            (*raw).render = Some(raw_render);
            (*raw).cleanup = Some(raw_cleanup_callback);
            let ret = Bela_initAudio(raw, app.cast::<c_void>());
            Bela_InitSettings_free(raw);
            ret
        };
        report("raw-init", &format!("returned-{ret}"));
        report("cleanup", "calling");
        unsafe { Bela_cleanupAudio() };
        report("cleanup", "returned");
        // Only now: everything above ran with the application alive,
        // which is the whole point of the probe.
        let app = unsafe { Box::from_raw(app) };
        report(
            "cleanup-callback",
            if app.cleaned { "ran" } else { "never-ran" },
        );
    }

    /// A cycle attempted while another process holds the audio device,
    /// and another once `wait` has given that process time to go.
    pub fn busy_probe(wait: Duration) {
        report("busy-first", &outcome(cycle()));
        thread::sleep(wait);
        report("busy-second", &outcome(cycle()));
    }

    /// Full cycles, one after another, in one process.
    pub fn cycles(count: u32) {
        for index in 1..=count {
            report(&format!("cycle-{index}"), &outcome(cycle()));
        }
        report("cycles", "completed");
    }

    /// Audio systems built and dropped without ever being started,
    /// which is the cycle the board notes put the bus error on — a
    /// different one from [`cycles`], where audio actually runs.
    pub fn init_cycles(count: u32) {
        for index in 1..=count {
            let outcome = match Bela::new(Idle, &Settings::new()) {
                Err(error) => format!("failed-{error:?}"),
                Ok(bela) => {
                    drop(bela);
                    "built-and-dropped".to_owned()
                }
            };
            report(&format!("init-cycle-{index}"), &outcome);
        }
        report("init-cycles", "completed");
    }
}

#[cfg(bela_device)]
fn main() -> ExitCode {
    use core::time::Duration;
    use std::env::args;

    let arguments: Vec<String> = args().skip(1).collect();
    let probe: Vec<&str> = arguments.iter().map(String::as_str).collect();
    match probe.as_slice() {
        ["render-check"] => probes::render_check(None),
        ["render-check", seconds] => {
            let Ok(seconds) = seconds.parse() else {
                eprintln!("render-check takes a number of seconds, not {seconds:?}");
                return ExitCode::FAILURE;
            };
            probes::render_check(Some(Duration::from_secs(seconds)));
        }
        ["abort"] => probes::abort(),
        ["abort-cleanup"] => probes::abort_cleanup(),
        ["raw-cleanup"] => probes::raw_cleanup(),
        ["abort-then-new"] => probes::abort_then_new(),
        ["abort-cleanup-then-new"] => probes::abort_cleanup_then_new(),
        ["busy-probe", seconds] => {
            let Ok(seconds) = seconds.parse() else {
                eprintln!("busy-probe takes a number of seconds, not {seconds:?}");
                return ExitCode::FAILURE;
            };
            probes::busy_probe(Duration::from_secs(seconds));
        }
        ["cycles", count] => {
            let Ok(count) = count.parse() else {
                eprintln!("cycles takes a count, not {count:?}");
                return ExitCode::FAILURE;
            };
            probes::cycles(count);
        }
        ["init-cycles", count] => {
            let Ok(count) = count.parse() else {
                eprintln!("init-cycles takes a count, not {count:?}");
                return ExitCode::FAILURE;
            };
            probes::init_cycles(count);
        }
        _ => {
            eprintln!(
                "usage: init_failure (render-check [seconds] | abort | abort-cleanup\n\
                 \x20                   | raw-cleanup | abort-then-new\n\
                 \x20                   | abort-cleanup-then-new | busy-probe <seconds>\n\
                 \x20                   | cycles <count> | init-cycles <count>)\n\
                 one probe per run: what is being measured is partly what the previous process left"
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
