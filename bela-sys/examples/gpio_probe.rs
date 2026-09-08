//! Measures what the sysfs GPIO family does on a board.
//!
//! Run it with `scripts/probe-gpio.sh`, which builds it, copies it
//! over, runs it in both modes and keeps the output. The findings
//! belong in `docs/board-facts.md`, beside the ones the crate
//! documentation for `bela_sys` already cites.
//!
//! This is an experiment, not a check, the way `scripts/probe-io.sh`
//! is: nothing here passes or fails. It exits non-zero only when it
//! could not ask, or could not put something back — a pin it could not
//! claim at all, a trigger it changed and then could not restore. A
//! trigger it could not *read* first is skipped rather than an error,
//! precisely so that it is never changed without a way back.
//!
//! It creates **no audio system**. That is the point of it rather than
//! an economy: the interesting questions are what an application gets
//! when it reaches for a pin *while libbela is running in another
//! process*, and a probe that brought its own audio system up could
//! not ask them — libbela refuses a second one ("Bela is already
//! running in another process"). So `scripts/probe-gpio.sh` starts an
//! ordinary example on the board and runs this beside it. The "one
//! audio system per process" rule is not in play here, and a crash
//! leaves no process holding the audio device.
//!
//! Two modes:
//!
//! - alone (no arguments), which asks what the family does on a board
//!   where nothing has claimed anything;
//! - `--with-run`, which asks what it does to pins libbela is holding,
//!   and is meaningless unless something else is running.
//!
//! `--destructive` is separate because what it allows may stop a
//! running audio system. It adds question 12 — unexporting a pin
//! libbela still holds — and it also widens question 11: without it,
//! that question is asked only where the channel's direction reads
//! `in` and a write cannot take, which is the harmless case and the
//! one this board answers. With it, a channel that reads `out` is
//! written too, which is contending with whatever drives it.
//!
//! What it answers, alone:
//!
//! 1. What does exporting a free pin return, and what does exporting
//!    an already-exported one return?
//! 2. What does `gpio_setup` hand back, and what direction does the
//!    pin end up with?
//! 3. How many `gpio_read` calls on one descriptor are correct?
//! 4. What does a `gpio_write` do to the next `gpio_read`?
//! 5. Does `gpio_dismiss` remove the export, and what does a second
//!    `gpio_unexport` say?
//! 6. Which `led_set_trigger` numbers name a file on this board?
//! 7. What does a pin number no chip covers do?
//! 8. Does an export outlive the process that made it? Asked by
//!    leaving one behind on purpose, and answered by the script.
//!
//! With a run up:
//!
//! 9. What does claiming one of libbela's own pins return?
//! 10. What can be read from one?
//! 11. Does a write to a PRU-driven channel reach the pin?
//! 12. (`--destructive`) What does unexporting one of libbela's pins
//!     do to the run holding it?
//! 13. What is still exported once both processes have gone?
//!
//! The two lists share one numbering, so question 8 is the alone pass
//! and question 13 the with-run one. Both are the same question —
//! what is claimed once this process has gone — and the script answers
//! them, by listing `/sys/class/gpio` after the probe exits, which is
//! the only place it can be seen from. It clears what 8 leaves.
//!
//! # What is put back, and what is not
//!
//! `--release` unexports every pin this probe can claim, and the
//! script calls it after each pass and from its handler. It does not
//! ask whether *this* invocation claimed them, and that is deliberate:
//! knowing would need a ledger written before each export and unwound
//! after it, and an interrupt could still land between two
//! instructions. What keeps a pin from leaking is the layering the
//! script already has — `timeout -s INT` bounds every run, a bounded
//! run ends normally, and a normal end gives back what it held — with
//! `--release` as the backstop. That is the shape
//! `scripts/probe-io.sh` has always had, and it is enough.
//!
//! Two things it does not cover, said rather than left to be found:
//!
//! - An LED trigger. Question 6 sets one to `none` and puts it back a
//!   line later; a probe killed in between leaves it changed, and
//!   nothing here restores triggers. The script says so and gives the
//!   command to check.
//! - A pin exported by something else. `--release` will unexport one
//!   of its two whoever claimed it — Bela's own two LEDs, which
//!   nothing else takes on an idle board. It declines outright where
//!   any *other* pin is exported, that being what a run looks like,
//!   and it asks about those rather than about the LEDs so that the
//!   LEDs can always be given back.

fn main() {
    imp::main();
}

#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
mod imp {
    use std::ffi::c_int;
    use std::{env, fs, process};

    // The control for question 3. `gpio_read` misbehaves because it
    // never rewinds, so rewinding for it ought to make it behave, and
    // that is the only evidence for *why* rather than *that*. Declared
    // here rather than pulled from a dependency: this crate has no
    // `libc`, and one function is not a reason to acquire one.
    unsafe extern "C" {
        fn lseek(fd: c_int, offset: i64, whence: c_int) -> i64;
    }

    /// `SEEK_SET`, the whence `lseek` is given to rewind.
    const SEEK_SET: c_int = 0;

    use bela_sys::{
        PIN_VALUE, gpio_dismiss, gpio_export, gpio_fd_close, gpio_fd_open, gpio_get_value,
        gpio_read, gpio_set_dir, gpio_set_edge, gpio_set_value, gpio_setup, gpio_unexport,
        gpio_write, led_set_trigger,
    };

    /// Bank 0 is `600000.gpio`; bank 1 is `601000.gpio`. Both bases are
    /// measured, in "The board LEDs" in `docs/board-facts.md`, and a
    /// `GPIOn_m` is `BASE_n + m`.
    const BANK0: u32 = 539;
    const BANK1: u32 = 631;

    /// The blue running LED (`GPIO0_45`), which libbela claims for the
    /// duration of a run unless `enable_led` is off.
    const LED_RUNNING: u32 = BANK0 + 45;
    /// The red underrun LED (`GPIO0_46`), claimed with the one above.
    const LED_UNDERRUN: u32 = BANK0 + 46;
    /// The stop button (`GPIO0_47`), which libbela opens with
    /// `unexport = false` and so leaves exported after a run.
    const STOP_BUTTON: u32 = BANK0 + 47;
    /// Digital channel D0 (`GPIO1_6`, header `P1_21`), from
    /// `digital_gpio_mapping.h`. libbela exports all sixteen for the
    /// PRU while a run is up.
    const DIGITAL_D0: u32 = BANK1 + 6;

    /// A number past the end of every chip on this board, for the
    /// question about arguments nothing can honour.
    const NO_SUCH_PIN: u32 = 99_999;

    /// The four `PIN_DIRECTION` / `PIN_VALUE` constants as the
    /// `c_int` that every parameter taking one actually wants. That
    /// mismatch is the first of the traps `bela_sys` documents, and
    /// casting once here keeps the questions below reading as calls.
    #[allow(
        clippy::cast_possible_wrap,
        reason = "INPUT_PIN, OUTPUT_PIN, LOW and HIGH are 0 and 1"
    )]
    mod arg {
        use std::ffi::c_int;

        pub(super) const INPUT: c_int = bela_sys::INPUT_PIN as c_int;
        pub(super) const OUTPUT: c_int = bela_sys::OUTPUT_PIN as c_int;
        pub(super) const LOW: c_int = bela_sys::LOW as c_int;
        pub(super) const HIGH: c_int = bela_sys::HIGH as c_int;
    }

    /// The user LEDs `led_set_trigger` builds a path for. A
    /// `BeagleBone` has `usr0`..`usr3`; this board is measured to have
    /// `usr1`..`usr4`, so the sweep covers both and reports which
    /// answered.
    const LED_NUMBERS: &[u32] = &[0, 1, 2, 3, 4];

    /// Where `led_set_trigger` writes, so that the probe can read a
    /// trigger back before changing it and put it there afterwards.
    fn trigger_path(lednum: u32) -> String {
        format!("/sys/class/leds/beaglebone:green:usr{lednum}/trigger")
    }

    /// The trigger currently selected, which the sysfs file marks with
    /// brackets among all the ones it offers.
    fn current_trigger(lednum: u32) -> Option<String> {
        let contents = fs::read_to_string(trigger_path(lednum)).ok()?;
        let start = contents.find('[')? + 1;
        let end = contents[start..].find(']')? + start;
        Some(contents[start..end].to_owned())
    }

    /// Every pin this probe can leave exported, and so every pin
    /// `--release` gives back.
    ///
    /// `STOP_BUTTON` is deliberately not here. libbela opens it with
    /// `unexport = false` and it is exported before this probe ever
    /// runs and expected to stay — `docs/board-facts.md` records that
    /// as the board's resting state, and unexporting it would change
    /// what a later run measures.
    /// `DIGITAL_D0` is not here either, for a different reason from
    /// the stop button's: `release_all` returns before the loop
    /// whenever that pin is exported, so the loop could only ever be
    /// reached with it already free. Neither pass can leak it — the
    /// alone pass never touches it, and the with-run pass refuses to
    /// start unless libbela has already exported it — so listing it
    /// would only print `was free, now free` on every release, in a
    /// transcript whose point is telling claimed pins from free ones.
    const RELEASABLE: &[u32] = &[LED_RUNNING, LED_UNDERRUN];

    /// Whether anything on the board looks like a run in progress.
    ///
    /// Any exported pin other than the two LEDs and the stop button.
    /// The LEDs cannot be the signal — they are what `--release` gives
    /// back, so guarding on them would mean never giving them back —
    /// and the stop button is exported before any run and stays. Every
    /// other pin of the twenty-two libbela takes for a run is there
    /// only while one is up; `docs/board-facts.md` lists them.
    ///
    /// A run that exported nothing but the LEDs would not be seen
    /// here. No configuration measured on this board does that: with
    /// `enable_led` off it exports the other twenty and neither LED,
    /// and with the LEDs on it exports those twenty as well.
    fn a_run_is_up() -> bool {
        let Ok(entries) = fs::read_dir("/sys/class/gpio") else {
            // Unreadable: assume the worse of the two, which is that
            // something is holding pins.
            return true;
        };
        entries.filter_map(Result::ok).any(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.strip_prefix("gpio")
                .and_then(|rest| rest.parse::<u32>().ok())
                .is_some_and(|pin| !matches!(pin, LED_RUNNING | LED_UNDERRUN | STOP_BUTTON))
        })
    }

    /// Gives back every pin this probe can claim, whether or not this
    /// invocation claimed it.
    ///
    /// That bluntness is the trade-off, and it is the point. Tracking
    /// which pins were ours needed a ledger written before each export
    /// and unwound after it, and an interrupt could still land between
    /// two instructions; three attempts at it were the substance of
    /// eight review rounds. What actually keeps a pin from leaking is
    /// the layering the script already has — `timeout -s INT` bounds
    /// every run, a bounded run ends normally, and a normal end
    /// releases what it held — with this as the backstop, which is the
    /// same shape `scripts/probe-io.sh` has always had.
    ///
    /// The cost is that it will unexport one of its two even if
    /// something else exported it. It declines where any pin outside
    /// its own set and the stop button is claimed, which is what a run
    /// looks like; on a board with no run these two are Bela's own
    /// LEDs, which nothing else takes.
    fn release_all() -> Result<(), String> {
        println!("== releasing every pin this probe can claim ==");
        // The precondition, checked here rather than trusted to every
        // caller: two of these three are libbela's while a run is up,
        // and the alone pass refuses for the same reason. `DIGITAL_D0`
        // is the signal because libbela exports the sixteen channels
        // for the PRU and gives them back when the run ends. A run
        // with digital I/O off is not detected by it — the script's
        // own calls are made where it has just ended the run it
        // started, which is what covers that.
        // Asked of pins this does *not* release, so that the two it
        // does can always be given back. Guarding on `LED_RUNNING`
        // instead — which is where this started — made the backstop
        // unable to return the pin the alone pass is likeliest to
        // leak, since every question from 1 to 7 claims it.
        if a_run_is_up() {
            // Through `Err` rather than a printed line, because the
            // caller that most needs to know is the script's handler,
            // which runs this after a `kill -9` — where libbela's
            // teardown never ran and all twenty-two of its exports are
            // still there. Silently returning `0` from that is how a
            // board is left claimed with nobody told.
            return Err(format!(
                "NOT released: pins other than gpio{LED_RUNNING}, gpio{LED_UNDERRUN} \
                 and gpio{STOP_BUTTON} are exported, so either a run is up and the \
                 LEDs are its, or one was killed hard enough to skip libbela's \
                 teardown and its exports are orphaned. Either way the LEDs are not \
                 this probe's to take back; unexport by hand what is left."
            ));
        }
        let mut still_held = Vec::new();
        for &pin in RELEASABLE {
            let was = exported(pin);
            let ret = unsafe { gpio_unexport(pin) };
            let now = exported(pin);
            println!(
                "  gpio_unexport({pin}) = {ret} (was {}, now {})",
                if was { "exported" } else { "free" },
                if now { "exported" } else { "free" }
            );
            if now {
                still_held.push(pin);
            }
        }
        // The pin, not the return value: `gpio_unexport` opens the
        // `unexport` file and can then fail only in the write, which
        // it reports as `-1` and nothing else. A pin that stayed
        // claimed here is one the next run will abort on, and this is
        // the only place that can say which run left it.
        if still_held.is_empty() {
            return Ok(());
        }
        Err(format!(
            "still exported after being asked to go: {}",
            still_held
                .iter()
                .map(|pin| format!("gpio{pin}"))
                .collect::<Vec<_>>()
                .join(", ")
        ))
    }

    fn exported(pin: u32) -> bool {
        fs::metadata(format!("/sys/class/gpio/gpio{pin}")).is_ok()
    }

    fn direction(pin: u32) -> String {
        fs::read_to_string(format!("/sys/class/gpio/gpio{pin}/direction"))
            .map_or_else(|e| format!("<{e}>"), |s| s.trim().to_owned())
    }

    pub(crate) fn main() {
        let args: Vec<String> = env::args().skip(1).collect();
        // An unknown argument used to fall through to the alone pass,
        // so a mistyped `--with_run` ran the pass that takes pins.
        if let Some(bad) = args
            .iter()
            .find(|a| !matches!(a.as_str(), "--with-run" | "--destructive" | "--release"))
        {
            eprintln!("unknown argument: {bad}");
            process::exit(2);
        }
        // Not a question: the script's tidy-up, which it calls where
        // no run is up. Does its work and stops.
        if args.iter().any(|a| a == "--release") {
            if let Err(why) = release_all() {
                eprintln!("{why}");
                process::exit(2);
            }
            return;
        }

        let with_run = args.iter().any(|a| a == "--with-run");
        let destructive = args.iter().any(|a| a == "--destructive");

        if destructive && !with_run {
            eprintln!(
                "--destructive only applies to --with-run, and is being ignored: \
                 the question it adds is about a pin libbela is holding"
            );
        }

        let could_ask = if with_run {
            with_run_questions(destructive)
        } else {
            alone_questions()
        };

        // Non-zero means the questions could not be put, never that an
        // answer was surprising.
        if let Err(why) = could_ask {
            eprintln!("could not ask: {why}");
            process::exit(2);
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one question per block, in the order the module doc numbers them"
    )]
    fn alone_questions() -> Result<(), String> {
        println!("== alone: nothing else should be running ==");
        // The mirror of the check `with_run_questions` makes, and the
        // more important of the two: this pass dismisses and unexports
        // `LED_RUNNING`, which sysfs grants whoever asks. Run against a
        // live run it would take that pin with no `--destructive` and
        // no warning, which is the one act this probe gates.
        // Two pins, because neither alone covers every configuration: a
        // run with digital I/O off exports no channel, and one with
        // `enable_led` off exports no LED. Where neither is exported
        // libbela is not holding the running LED either, so there is
        // nothing for this pass to take.
        for (what, pin) in [("digital D0", DIGITAL_D0), ("the running LED", LED_RUNNING)] {
            if exported(pin) {
                return Err(format!(
                    "gpio{pin} ({what}) is exported, so something else is holding pins; \
                     these questions claim and release them and must not run beside a run"
                ));
            }
        }
        println!("at rest, /sys/class/gpio holds: {}", listing());

        // 1. Exporting a free pin, then the same pin again.
        println!("\n-- 1. export, twice --");
        let first = unsafe { gpio_export(LED_RUNNING) };
        println!(
            "gpio_export({LED_RUNNING}) = {first}, exported now: {}",
            exported(LED_RUNNING)
        );
        let second = unsafe { gpio_export(LED_RUNNING) };
        println!("gpio_export({LED_RUNNING}) again = {second}");
        let _ = unsafe { gpio_unexport(LED_RUNNING) };

        // 2. What gpio_setup hands back.
        println!("\n-- 2. gpio_setup --");
        let fd = unsafe { gpio_setup(LED_RUNNING, arg::OUTPUT) };
        println!("gpio_setup({LED_RUNNING}, OUTPUT_PIN) = {fd}");
        println!("  direction file now: {}", direction(LED_RUNNING));
        if fd < 0 {
            // `gpio_setup` exports before it opens, so a failure here
            // can leave the pin claimed — the trap this probe exists
            // to document. Give it back before reporting.
            unsafe { gpio_unexport(LED_RUNNING) };
            return Err(format!("gpio_setup on a free pin returned {fd}"));
        }

        // 3. How many reads on one descriptor are correct.
        println!("\n-- 3. gpio_read on one descriptor --");
        for n in 1..=3 {
            let mut value: PIN_VALUE = 0xdead_beef;
            let ret = unsafe { gpio_read(fd, &raw mut value) };
            println!("  read {n}: ret {ret}, *value {value:#x}");
        }
        let mut value: PIN_VALUE = 0xdead_beef;
        let ret = unsafe { gpio_get_value(LED_RUNNING, &raw mut value) };
        println!("gpio_get_value (opens and closes its own file) = {ret}, *value {value}");
        println!("  the same three, with an lseek back to 0 before each:");
        for n in 1..=3 {
            unsafe { lseek(fd, 0, SEEK_SET) };
            let mut value: PIN_VALUE = 0xdead_beef;
            let ret = unsafe { gpio_read(fd, &raw mut value) };
            println!("    read {n}: ret {ret}, *value {value:#x}");
        }
        // Question 4 opens its own; this one has been answered.
        println!("gpio_fd_close(fd) = {}", unsafe { gpio_fd_close(fd) });

        // 4. What a write does to the next read.
        println!("\n-- 4. gpio_write, then gpio_read on the same descriptor --");
        let fd2 = unsafe { gpio_setup(LED_RUNNING, arg::OUTPUT) };
        println!("a fresh descriptor: {fd2}");
        if fd2 < 0 {
            // Asking anyway would put `-1` into `write(2)`, and the
            // transcript would record `EBADF` on a bogus descriptor as
            // if it were what these functions do to a pin.
            unsafe { gpio_unexport(LED_RUNNING) };
            return Err(format!("gpio_setup for question 4 returned {fd2}"));
        }
        println!("gpio_write(fd2, HIGH) = {}", unsafe {
            gpio_write(fd2, arg::HIGH)
        });
        let mut value: PIN_VALUE = 0xdead_beef;
        let ret = unsafe { gpio_read(fd2, &raw mut value) };
        println!("  the very next gpio_read: ret {ret}, *value {value:#x}");
        let _ = unsafe { gpio_write(fd2, arg::LOW) };

        // 5. Teardown, and a second unexport.
        println!("\n-- 5. gpio_dismiss, then unexport again --");
        println!("gpio_dismiss = {}", unsafe {
            gpio_dismiss(fd2, LED_RUNNING)
        });
        println!("  still exported: {}", exported(LED_RUNNING));
        println!("gpio_unexport (second time) = {}", unsafe {
            gpio_unexport(LED_RUNNING)
        });

        // 6. Which LED numbers name a file.
        println!("\n-- 6. led_set_trigger --");
        for &n in LED_NUMBERS {
            // Read first, and only write what can be put back. A file
            // that exists but cannot be read for its `[selected]`
            // marker would otherwise be set to `none` and left there,
            // while the transcript said there was no file at all.
            let Some(before) = current_trigger(n) else {
                if fs::metadata(trigger_path(n)).is_ok() {
                    // The file is there but would not say what it holds,
                    // so a write could not be undone. Do not write.
                    println!("  lednum {n}: not asked — the file is there but its");
                    println!("    current trigger could not be read, so a write");
                    println!("    could not be put back");
                } else {
                    // Nothing to restore and nothing to break: this is
                    // the answer for a number naming no file, and it is
                    // what the `lednum` starting at 1 finding rests on.
                    let ret = unsafe { led_set_trigger(n, c"none".as_ptr()) };
                    println!("  lednum {n}: ret {ret} (no such file to begin with)");
                }
                continue;
            };
            let ret = unsafe { led_set_trigger(n, c"none".as_ptr()) };
            println!("  lednum {n}: ret {ret} (was [{before}], restoring)");
            if let Err(e) = fs::write(trigger_path(n), &before) {
                // Loudly, and naming both, because nothing else will
                // put it back: the script's handler covers the GPIO
                // export, the remote directory and the daemon, not this.
                eprintln!(
                    "LEFT CHANGED: usr{n} is now `none` and was `{before}`; \
                     restore it by hand with: echo {before} > {}",
                    trigger_path(n)
                );
                return Err(format!("could not restore usr{n} to {before}: {e}"));
            }
        }

        // The four this pass's questions never reach. `gpio_fd_close`
        // is not among them, question 3 having closed its descriptor;
        // `gpio_set_value` is, being reached only by the with-run
        // pass, and only there under a condition — so without it here
        // an operator who ran pass 1 alone would have linked twelve.
        // Nothing here is a question: they are called so that a probe
        // which links and runs is evidence for all thirteen symbols
        // rather than for the nine this pass needs.
        println!("\n-- the remaining four, called only to link them --");
        let fd3 = unsafe { gpio_setup(LED_RUNNING, arg::OUTPUT) };
        if fd3 >= 0 {
            println!("  gpio_set_dir(INPUT) = {}", unsafe {
                gpio_set_dir(LED_RUNNING, arg::INPUT)
            });
            println!("  gpio_set_edge(\"none\") = {}", unsafe {
                gpio_set_edge(LED_RUNNING, c"none".as_ptr().cast_mut())
            });
            let ro = unsafe { gpio_fd_open(LED_RUNNING, 0) };
            println!("  gpio_fd_open(O_RDONLY) = {ro}");
            if ro < 0 {
                // `close(-1)` is `EBADF`, and printing that as
                // `gpio_fd_close = -1` reads as a close that failed
                // rather than as one there was nothing to do.
                println!("  gpio_fd_close: not called, there is no descriptor");
            } else {
                println!("  gpio_fd_close = {}", unsafe { gpio_fd_close(ro) });
            }
            // An input, so this cannot take; the point is the link.
            println!("  gpio_set_value(LOW) = {}", unsafe {
                gpio_set_value(LED_RUNNING, arg::LOW)
            });
            // `gpio_dismiss` returns `0` whatever happened, so the
            // pin is what to check, not the call. Left exported it
            // would show up in question 8's listing as a second leak
            // and be read as part of the one that is deliberate.
            let _ = unsafe { gpio_dismiss(fd3, LED_RUNNING) };
            if exported(LED_RUNNING) {
                println!(
                    "  gpio_dismiss left it exported; gpio_unexport = {}",
                    unsafe { gpio_unexport(LED_RUNNING) }
                );
            }
        } else {
            // Two of `gpio_setup`'s three failure paths leave the pin
            // exported with no descriptor, which is the trap question 2
            // handles. Give it back, or question 8's listing reports
            // two leaked pins rather than the one it means.
            println!("  skipped: gpio_setup returned {fd3}");
            println!("  gpio_unexport = {}", unsafe {
                gpio_unexport(LED_RUNNING)
            });
        }

        // 7. A pin number no chip covers.
        println!("\n-- 7. a pin nothing can honour --");
        println!("gpio_export({NO_SUCH_PIN}) = {}", unsafe {
            gpio_export(NO_SUCH_PIN)
        });

        // 8. The question the wrapper's shape turns on. Everything
        // above tidied up after itself, which is exactly why none of
        // it can answer this one: an export is a change to a global
        // filesystem, not a resource the kernel reclaims when the
        // process that made it goes away — so a wrapper that drops
        // without unexporting leaves the pin claimed for whatever runs
        // next. Left deliberately; the script reports it and clears it.
        println!("\n-- 8. does an export outlive the process that made it? --");
        // Whether this export is *ours* cannot be read off the return
        // value: `gpio_export` answers `0` for a pin it merely found,
        // which is the finding this whole probe is about. So look
        // first, and only claim what was not there before.
        let was_exported = exported(LED_UNDERRUN);
        let ret = unsafe { gpio_export(LED_UNDERRUN) };
        println!("gpio_export({LED_UNDERRUN}) = {ret}");
        if was_exported {
            println!("  it was already exported before this probe asked, so there is");
            println!("  nothing of ours here to leave behind, and this question");
            println!("  goes unanswered rather than answered by somebody else's pin");
            println!("\nleaving /sys/class/gpio at: {}", listing());
            return Ok(());
        }
        if !exported(LED_UNDERRUN) {
            // The pin was free and is still not exported, so the call
            // failed. Saying "already exported" here would report the
            // question as answered when it was never asked.
            return Err(format!(
                "gpio_export({LED_UNDERRUN}) left the pin unexported, so question 8 \
                 could not be put"
            ));
        }
        println!("  exiting now WITHOUT unexporting it, on purpose");
        // Nothing records it: the script answers this question by
        // listing `/sys/class/gpio` once this process has gone, and
        // then calls `--release`, which gives back every pin this
        // probe can claim whether or not it claimed them.

        println!("\nleaving /sys/class/gpio at: {}", listing());
        Ok(())
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one question per block, in the order the module doc numbers them"
    )]
    fn with_run_questions(destructive: bool) -> Result<(), String> {
        println!("== with a run up: something else must be holding the audio device ==");
        println!("/sys/class/gpio holds: {}", listing());

        let claimed = [
            ("running LED", LED_RUNNING),
            ("underrun LED", LED_UNDERRUN),
            ("stop button", STOP_BUTTON),
            ("digital D0", DIGITAL_D0),
        ];

        // The stop button's export outlives a run, so it alone proves
        // nothing. The digital channels are exported for the PRU while
        // a run is up and gone afterwards, which is the signal that
        // something is actually rendering right now.
        if !exported(DIGITAL_D0) {
            return Err(format!(
                "gpio{DIGITAL_D0} (digital D0) is not exported, so nothing is \
                 rendering; these questions need a run to be up"
            ));
        }

        // 9 and 10: claiming and reading pins libbela is holding.
        // Whatever we exported that libbela had not — which happens
        // with `enable_led` off, where it claims neither LED — is ours
        // to give back, or the closing listing reports our own leak as
        // an answer.
        let mut ours: Vec<u32> = Vec::new();
        for (name, pin) in claimed {
            println!("\n-- {name} (gpio{pin}) --");
            println!("  exported before we ask: {}", exported(pin));
            println!("  direction: {}", direction(pin));
            let was_exported = exported(pin);
            println!("  gpio_export = {}", unsafe { gpio_export(pin) });
            if !was_exported && exported(pin) {
                ours.push(pin);
            }
            let mut value: PIN_VALUE = 0xdead_beef;
            let ret = unsafe { gpio_get_value(pin, &raw mut value) };
            println!("  gpio_get_value = {ret}, *value {value:#x}");
        }

        // 11: writing to a pin the PRU drives. On this board the pin
        // is an input and the write cannot take, which is the answer
        // and is harmless. On a board where it is an output the same
        // call would contend with whatever is driving it — the
        // loopback rig in "What a digital pin does" is one — so that
        // case needs the same opt-in the destructive question has.
        println!("\n-- writing gpio{DIGITAL_D0} while the PRU drives it --");
        let d0_direction = direction(DIGITAL_D0);
        println!("  its direction is {d0_direction}");
        if d0_direction != "in" && d0_direction != "out" {
            println!("  not attempted: its direction could not be read, so whether a");
            println!("  write would contend with a driver is not established");
        } else if d0_direction == "out" && !destructive {
            println!("  not attempted: writing an output pin contends with its driver,");
            println!("  which is what --destructive is for");
        } else {
            let mut before: PIN_VALUE = 0xdead_beef;
            let read_before = unsafe { gpio_get_value(DIGITAL_D0, &raw mut before) };
            let wrote = unsafe { gpio_set_value(DIGITAL_D0, arg::HIGH) };
            println!("  gpio_set_value(HIGH) = {wrote}");
            let mut value: PIN_VALUE = 0xdead_beef;
            let ret = unsafe { gpio_get_value(DIGITAL_D0, &raw mut value) };
            println!("  reads back: ret {ret}, *value {value:#x}");
            // What decides whether a restore is owed is whether the
            // write took, not whether the pin could be read first: a
            // write that succeeded after a failed read would otherwise
            // leave a PRU channel driven for the rest of the run.
            if wrote == 0 {
                let restore = if read_before == 0 && before != 0 {
                    arg::HIGH
                } else {
                    arg::LOW
                };
                let to = if read_before == 0 {
                    format!("what it held ({before})")
                } else {
                    "LOW, its value not having been readable first".to_owned()
                };
                println!("  restoring to {to}: {}", unsafe {
                    gpio_set_value(DIGITAL_D0, restore)
                });
            } else {
                println!("  the write did not take, so there is nothing to put back");
            }
        }

        // 12: the destructive one.
        if destructive {
            println!("\n-- DESTRUCTIVE: unexporting the running LED out from under the run --");
            if ours.contains(&LED_RUNNING) {
                // This export is the probe's own — a run with
                // `enable_led` off claims neither LED — so dismissing
                // it would measure the probe taking a pin from itself
                // while the transcript read as one taken from libbela.
                println!("  NOT asked: gpio{LED_RUNNING} was not exported until this");
                println!("  probe did it, so libbela is not holding it and there is");
                println!("  nothing here to take from the run");
            } else {
                // Read before the call, because `gpio_setup` sets the
                // direction before it opens: the failure branch below
                // has to put back what the pin held, and cannot know
                // that afterwards.
                let direction_before = direction(LED_RUNNING);
                let fd = unsafe { gpio_setup(LED_RUNNING, arg::INPUT) };
                println!("  gpio_setup = {fd} (its direction was {direction_before})");
                if fd < 0 {
                    // `gpio_dismiss` would unexport the pin anyway, so
                    // the destructive act would still happen — but as a
                    // bare unexport rather than a claim then a release,
                    // and the transcript would not say which.
                    println!("  NOT proceeding: what follows would be a bare unexport,");
                    println!("  which is a different thing from taking a pin we held");
                    // `gpio_setup` sets the direction before it opens,
                    // so this branch is reached with the run's LED
                    // already flipped to an input. Put it back.
                    // Only what it actually held, and only if that is
                    // not what it holds now: `gpio_setup` may have
                    // failed at the export, before touching direction.
                    let now = direction(LED_RUNNING);
                    if now == direction_before {
                        println!("  its direction is unchanged, so nothing to put back");
                    } else if direction_before == "out" || direction_before == "in" {
                        let back = if direction_before == "out" {
                            arg::OUTPUT
                        } else {
                            arg::INPUT
                        };
                        println!(
                            "  restoring its direction to {direction_before}: {}",
                            unsafe { gpio_set_dir(LED_RUNNING, back) }
                        );
                    } else {
                        // `direction` reports a read failure as its own
                        // message rather than a direction, and there is
                        // nothing to restore to. Saying so beats
                        // setting it to a guess and calling that a
                        // restore.
                        println!("  its direction could not be read before this, so there");
                        println!("  is nothing to put it back to; it now reads {now}");
                    }
                } else {
                    println!("  gpio_dismiss = {}", unsafe {
                        gpio_dismiss(fd, LED_RUNNING)
                    });
                    println!("  still exported: {}", exported(LED_RUNNING));
                    println!("  what this did to the run is the script's to report");
                }
            }
        } else {
            println!("\n(skipping the destructive question; pass --destructive for it)");
        }

        if ours.is_empty() {
            println!("\n(libbela had already exported every pin asked about)");
        } else {
            println!("\n-- giving back the pins libbela had not exported --");
            for pin in &ours {
                println!("  gpio_unexport({pin}) = {}", unsafe {
                    gpio_unexport(*pin)
                });
            }
        }

        // The entry check said a run was up. Say whether one still is,
        // so that a probe which outlived the run cannot have its
        // answers read as answers about a board that was rendering.
        println!("\n-- was a run still up when these finished? --");
        if exported(DIGITAL_D0) {
            println!("  yes: gpio{DIGITAL_D0} is still exported for the PRU");
        } else {
            println!("  NO — the run ended part way through, and the answers");
            println!("  above are not all about a board that was rendering");
        }

        println!("\nleaving /sys/class/gpio at: {}", listing());
        Ok(())
    }

    /// What `/sys/class/gpio` holds, with the `gpiochip*` directories
    /// and the `export`/`unexport` attribute files left out: all of
    /// them are always there and none says who claimed what. What is
    /// left is exactly the set of claimed pins, so two listings can be
    /// compared directly.
    fn listing() -> String {
        let Ok(entries) = fs::read_dir("/sys/class/gpio") else {
            return "<unreadable>".to_owned();
        };
        let mut names: Vec<String> = entries
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !n.starts_with("gpiochip") && n != "export" && n != "unexport")
            .collect();
        names.sort();
        names.join(" ")
    }
}

#[cfg(not(all(target_arch = "aarch64", target_os = "linux")))]
mod imp {
    use std::process;

    pub(crate) fn main() {
        eprintln!("gpio_probe only runs on a board (aarch64 linux)");
        process::exit(2);
    }
}
