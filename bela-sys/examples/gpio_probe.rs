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
//! Three things it does not cover, said rather than left to be found:
//!
//! - An LED trigger. Question 6 sets one to `none` and puts it back a
//!   line later; a probe killed in between leaves it changed, and
//!   nothing here restores triggers. The script says so and gives the
//!   command to check.
//! - A pin's level. An unexport keeps the level and the direction
//!   alike, measured, so what the last question set is what the pin
//!   holds afterwards. The direction is put back where it can be — the
//!   two calls made only to link `gpio_set_dir` undo each other, and
//!   question 12 asks as the direction the pin already holds — but the
//!   direction a pin held before this probe ran is not readable through
//!   an unexported pin, so what is restored is the pass's own starting
//!   point, not the board's. The level is the part nothing here puts
//!   back: questions 4 and 11 write one and put it back a line later —
//!   the running LED's pin and a digital channel — and a probe killed
//!   in between leaves that line high, or the channel driving against
//!   the PRU for the rest of the run. Whether a high line lights the
//!   indicator is not something sysfs answers, so nothing here says it
//!   does. Question 12's failure branch restores a direction, which is
//!   a write that drives the pin low and cannot carry a level back
//!   with it — but the PRU drives that pin every block and takes it
//!   back, so what lasts there is a direction that would not go back,
//!   an input being one the PRU cannot drive. `--release` unexports
//!   pins; nothing here restores a *value*, and the run's own end is
//!   what clears it.
//! - A pin exported by something else. `--release` will unexport one
//!   of its two whoever claimed it — Bela's own two LEDs, which
//!   nothing else takes on an idle board. It declines outright where
//!   any pin *other than those two and the stop button* is exported,
//!   that being what a run looks like — the stop button is exempt
//!   because it is the resting state of a board that has ever run
//!   Bela — and it asks about those rather than about the LEDs so that
//!   the LEDs can always be given back.

fn main() {
    imp::main();
}

#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
mod imp {
    use std::ffi::{c_char, c_int, c_void};
    use std::{env, fs, process, ptr};

    // The control for question 3. `gpio_read` misbehaves because it
    // never rewinds, so rewinding for it ought to make it behave, and
    // that is the only evidence for *why* rather than *that*. Declared
    // here rather than pulled from a dependency: this crate has no
    // `libc`, and one function is not a reason to acquire one.
    unsafe extern "C" {
        fn lseek(fd: c_int, offset: i64, whence: c_int) -> i64;

        /// C's own `stdout`, which is a different stream from the one
        /// `println!` writes to and is buffered on its own terms.
        static mut stdout: *mut c_void;
        fn setvbuf(stream: *mut c_void, buf: *mut c_char, mode: c_int, size: usize) -> c_int;
    }

    /// `SEEK_SET`, the whence `lseek` is given to rewind.
    const SEEK_SET: c_int = 0;

    /// `_IONBF`, read from the board's `/usr/include/stdio.h`.
    const IONBF: c_int = 2;

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
    /// here, and `release_all` would then take them from it.
    /// `PRU::prepareGPIO` gates the analog chip selects on
    /// `analogFrames` and the digital channels on `digitalFrames`, so
    /// `useAnalog = 0` with `useDigital = 0` and `enableLed` on reaches
    /// exactly that — not through this script, which runs
    /// `bela/examples/sine` with both on, but reachable. What it costs
    /// is measured in `docs/board-facts.md`: a run does not notice
    /// losing an LED export.
    fn a_run_is_up() -> Result<bool, String> {
        let entries = fs::read_dir("/sys/class/gpio").map_err(|e| {
            // Declining is still right — nothing here can be trusted —
            // but saying "pins are exported" would be a claim about a
            // directory that could not be read at all. Said without the
            // caller's framing, both callers having their own.
            format!(
                "/sys/class/gpio could not be read ({e}), so whether anything is \
                 holding a pin is unknown"
            )
        })?;
        Ok(entries.filter_map(Result::ok).any(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.strip_prefix("gpio")
                .and_then(|rest| rest.parse::<u32>().ok())
                .is_some_and(|pin| !matches!(pin, LED_RUNNING | LED_UNDERRUN | STOP_BUTTON))
        }))
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
        // Asked of pins this does *not* release, so that the two it
        // does can always be given back. Guarding on `LED_RUNNING`
        // instead — which is where this started — made the backstop
        // unable to return the pin the alone pass is likeliest to
        // leak, since every question from 1 to 7 claims it. And
        // guarding on `DIGITAL_D0` alone, which came before that,
        // missed a run with digital I/O off; `a_run_is_up` sees one,
        // that run still exporting the ADC reset and the SPI DAC chip
        // select among the twenty.
        if a_run_is_up().map_err(|e| {
            format!("NOT released: {e}. This declines rather than guess, and nothing is known to be left exported.")
        })? {
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

    /// Unexports `pin` and then asks the pin, `gpio_unexport` being the
    /// call question 5 measures refusing silently.
    ///
    /// An export that survives is one `--release` cannot give back
    /// either — it is the same call — so it is `LEFT CHANGED` and the
    /// caller's pass fails, rather than an `Err` that `main` would
    /// render as exit 2 and the script would read as a pass that left
    /// nothing. Every give-back of a pin this probe claimed goes
    /// through here, so that hardening one does not leave its sibling
    /// behind, which is how three of these came to differ.
    fn give_back(pin: u32, left_changed: &mut bool) -> Result<(), String> {
        let ret = unsafe { gpio_unexport(pin) };
        if !exported(pin) {
            println!("  gpio_unexport({pin}) = {ret}, and it is gone");
            return Ok(());
        }
        eprintln!(
            "LEFT CHANGED: gpio{pin} is exported and would not unexport ({ret}). \
             `--release` cannot give back a pin that refuses one. By hand: \
             echo {pin} > /sys/class/gpio/unexport"
        );
        *left_changed = true;
        Err(format!("gpio{pin} survived gpio_unexport"))
    }

    fn direction(pin: u32) -> String {
        fs::read_to_string(format!("/sys/class/gpio/gpio{pin}/direction"))
            .map_or_else(|e| format!("<{e}>"), |s| s.trim().to_owned())
    }

    pub(crate) fn main() {
        // Before anything is printed. libbela reports some failures
        // with C `printf` — `gpio_setup`'s two are in
        // `core/GPIOcontrol.cpp` — and C stdio block-buffers when its
        // output is a pipe, which is what the script's
        // `probe_out=$(...)` makes it. Those lines would then be
        // flushed at exit and land together at the bottom, detached
        // from the question that produced them, in a transcript whose
        // whole product is the order things happened in. Rust's own
        // prints go to a different, line-buffered stream, so only the C
        // side needs this. Its other failures use `perror`, which is
        // stderr and unbuffered, and already interleave.
        unsafe { setvbuf(stdout, ptr::null_mut(), IONBF, 0) };

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
        //
        // `3`, not `2`, when it declines. `2` is "could not ask", and
        // a caller that cannot tell the two apart has to guess whether
        // the transcript above it is a measurement — which the script
        // did, and told the operator the opposite of what happened,
        // whichever way round it was.
        if args.iter().any(|a| a == "--release") {
            if let Err(why) = release_all() {
                eprintln!("{why}");
                process::exit(3);
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

        // Held here rather than returned, because the two are
        // independent: a pass can leave something changed and *then*
        // fail to ask a later question, and returning the flag made
        // every `Err` after the change drop it. It goes true where a
        // pass could not put back one of the things nothing else here
        // reaches — an LED trigger (question 6), a pin's level
        // (questions 11 and 12).
        let mut left_changed = false;
        let could_ask = if with_run {
            with_run_questions(destructive, &mut left_changed)
        } else {
            alone_questions(&mut left_changed)
        };

        // Non-zero means the questions could not be put, or something
        // was left changed, never that an answer was surprising.
        if let Err(why) = could_ask {
            eprintln!("could not ask: {why}");
            if !left_changed {
                process::exit(2);
            }
        }
        // 6 takes precedence over 2, for the same reason 3 is not 2: a
        // pin or a trigger left changed is the thing that needs a hand,
        // and reporting only that the probe could not ask sends an
        // operator past it. The message above still prints, so a pass
        // that did both says so.
        if left_changed {
            process::exit(6);
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one question per block, in the order the module doc numbers them"
    )]
    /// Sets `left_changed` where question 6's LED trigger would not go
    /// back, which is a status of its own rather than a failure to ask.
    fn alone_questions(left_changed: &mut bool) -> Result<(), String> {
        println!("== alone: nothing else should be running ==");
        // Two checks, because two different things disqualify this
        // pass, and one used to stand in for both.
        //
        // First: is anything else holding pins at all. `a_run_is_up`
        // answers that, and `release_all` has always used it for the
        // same question — a run with digital I/O off *and* `enable_led`
        // off exports neither of the two pins below while still
        // exporting the ADC reset and the SPI DAC chip select among the
        // twenty. Nothing there is this pass's to take, which is why
        // checking those two alone looked like enough; but its exports
        // would be in question 8's answer and in the closing listing,
        // under a heading that says nothing else should be running.
        if a_run_is_up()? {
            return Err(
                "a pin outside the ones this probe can give back is exported, so \
                 something else is holding pins; every answer below would be about a \
                 board that was rendering, under a heading saying it was not"
                    .to_owned(),
            );
        }
        // And then the two this pass claims and releases — it dismisses
        // and unexports `LED_RUNNING`, which sysfs grants whoever asks,
        // and that is the one act this probe gates. Reaching here means
        // nothing else is exported, so either is a pin a previous probe
        // left behind rather than one a run is holding: `--release`
        // gives both back, and the script runs it before this pass.
        for (what, pin) in [
            ("the running LED", LED_RUNNING),
            ("the underrun LED", LED_UNDERRUN),
        ] {
            if exported(pin) {
                return Err(format!(
                    "gpio{pin} ({what}) is exported and no pin a run would hold is, so a \
                     previous probe left it behind; `--release` gives it back, and this \
                     script runs that before each pass"
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
        // Asked, not discarded: an export that outlives this question
        // makes question 2 measure `gpio_setup` on a claimed pin under
        // a heading that says a free one, and question 5's dismiss the
        // second export layer rather than the pin.
        give_back(LED_RUNNING, left_changed)?;

        // 2. What gpio_setup hands back.
        println!("\n-- 2. gpio_setup --");
        let fd = unsafe { gpio_setup(LED_RUNNING, arg::OUTPUT) };
        println!("gpio_setup({LED_RUNNING}, OUTPUT_PIN) = {fd}");
        println!("  direction file now: {}", direction(LED_RUNNING));
        if fd < 0 {
            // `gpio_setup` exports before it opens, so a failure here
            // can leave the pin claimed — the trap this probe exists
            // to document. Give it back before reporting.
            give_back(LED_RUNNING, left_changed)?;
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
        println!("gpio_get_value (opens and closes its own file) = {ret}, *value {value:#x}");
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
            give_back(LED_RUNNING, left_changed)?;
            return Err(format!("gpio_setup for question 4 returned {fd2}"));
        }
        println!("gpio_write(fd2, HIGH) = {}", unsafe {
            gpio_write(fd2, arg::HIGH)
        });
        let mut value: PIN_VALUE = 0xdead_beef;
        let ret = unsafe { gpio_read(fd2, &raw mut value) };
        println!("  the very next gpio_read: ret {ret}, *value {value:#x}");
        // The same rule the rest of the file follows: a restore that
        // refused is a thing left changed, not a line to drop. Question
        // 5 unexports this pin two lines down, and an unexport keeps
        // both the direction and the level — measured, and in
        // docs/board-facts.md — so a `HIGH` that stayed would stay
        // after the pass ended, on an idle board, with the transcript
        // silent about it.
        let put_back = unsafe { gpio_write(fd2, arg::LOW) };
        println!("  putting it back to LOW: {put_back}");
        if put_back != 0 {
            // Try the other way in before reporting: `gpio_write` uses
            // the descriptor question 4 opened, and `gpio_set_value`
            // opens its own, so a descriptor that has gone bad is not
            // the same as a pin that will not take a level.
            println!("  gpio_set_value(LOW) instead = {}", unsafe {
                gpio_set_value(LED_RUNNING, arg::LOW)
            });
            let mut level: PIN_VALUE = 0xdead_beef;
            let read = unsafe { gpio_get_value(LED_RUNNING, &raw mut level) };
            println!("  it now reads: ret {read}, *value {level:#x}");
            // The pin, not the calls. Said as a level rather than as a
            // lit LED: sysfs reads the line, and whether that lights
            // the indicator is not something this can see.
            if read != 0 || level != 0 {
                eprintln!(
                    "LEFT CHANGED: gpio{LED_RUNNING} was written HIGH and would not go \
                     back; the line is not known to be low, and an unexport keeps \
                     whatever it holds"
                );
                *left_changed = true;
            }
        }

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
        // The branch below that finds an unreadable file does not call
        // it, so a pass in which every number took that branch would be
        // evidence for twelve of the thirteen while exiting 0.
        let mut trigger_called = false;
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
                    trigger_called = true;
                    println!("  lednum {n}: ret {ret} (no such file to begin with)");
                }
                continue;
            };
            let ret = unsafe { led_set_trigger(n, c"none".as_ptr()) };
            trigger_called = true;
            if ret == 0 {
                println!("  lednum {n}: ret {ret} (was [{before}], restoring)");
            } else {
                // `led_set_trigger` answers `-1` for a failed `open` and
                // for a failed `write` alike (`GPIOcontrol.cpp:338,341`),
                // and only the first is measured here — usr0, ENOENT. So
                // ask the file rather than the return: a write that
                // failed after the attribute had taken `none` would
                // otherwise be skipped as nothing to restore, which is
                // the one outcome question 6 has to catch.
                match current_trigger(n) {
                    Some(now) if now == before => {
                        println!("  lednum {n}: ret {ret}, still [{before}] — unchanged");
                        continue;
                    }
                    Some(now) => {
                        println!("  lednum {n}: ret {ret} but it reads [{now}] — restoring");
                    }
                    None => {
                        // Writing back what it held is the only thing
                        // that can help; a failure is reported below.
                        println!("  lednum {n}: ret {ret} and it no longer reads — restoring");
                    }
                }
            }
            if let Err(e) = fs::write(trigger_path(n), &before) {
                // Loudly, and naming both, because nothing else will
                // put it back: the script's handler covers the GPIO
                // export, the remote directory and the daemon, not this.
                // What it holds now is read rather than assumed: two
                // of the three ways here are entered *because* it does
                // not read `none` — some third value, or nothing.
                let now = current_trigger(n).unwrap_or_else(|| "unreadable".to_owned());
                eprintln!(
                    "LEFT CHANGED: usr{n} reads `{now}` and was `{before}`; \
                     restore it by hand with: echo {before} > {}",
                    trigger_path(n)
                );
                eprintln!("  the write failed with: {e}");
                *left_changed = true;
                // And stop asking. Whatever made this restore refuse is
                // as likely to hold for the next number, and going on
                // would leave four triggers at `none` rather than one.
                break;
            }
        }

        if !trigger_called {
            return Err(
                "every lednum named a file whose current trigger could not be read, so \
                 led_set_trigger went uncalled and this pass is not evidence for it"
                    .to_owned(),
            );
        }

        // The four no question of this pass reaches on the way it
        // goes. `gpio_fd_close` is not among them, question 3 having
        // closed its descriptor; `gpio_set_value` is, the with-run
        // pass reaching it under a condition and question 4 only where
        // its restore refused — so without it here an operator whose
        // pass 1 went well would have linked twelve.
        // Nothing here is a question: they are called so that a probe
        // which links and runs is evidence for all thirteen symbols
        // rather than for the nine this pass needs.
        println!("\n-- the remaining four, called only to link them --");
        let fd3 = unsafe { gpio_setup(LED_RUNNING, arg::OUTPUT) };
        if fd3 >= 0 {
            println!("  gpio_set_dir(INPUT) = {}", unsafe {
                gpio_set_dir(LED_RUNNING, arg::INPUT)
            });
            // And back, the pin still being exported and its direction
            // still readable. An unexport keeps what it finds, so
            // without this every pass 1 leaves gpio584 latched as an
            // input on an idle board — for a call made only to link a
            // symbol.
            println!("  gpio_set_dir(OUTPUT), putting it back = {}", unsafe {
                gpio_set_dir(LED_RUNNING, arg::OUTPUT)
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
            // The point is the link, but it takes now that the
            // direction has been put back — and low is where question 4
            // left the line, so it changes nothing.
            println!("  gpio_set_value(LOW) = {}", unsafe {
                gpio_set_value(LED_RUNNING, arg::LOW)
            });
            // `gpio_dismiss` returns `0` whatever happened, so the
            // pin is what to check, not the call. Left exported it
            // would show up in question 8's listing as a second leak
            // and be read as part of the one that is deliberate.
            let _ = unsafe { gpio_dismiss(fd3, LED_RUNNING) };
            if exported(LED_RUNNING) {
                // Until this pin is free question 8 cannot be asked:
                // its answer is a listing, and a second pin in it reads
                // as part of the one that is deliberate.
                println!("  gpio_dismiss left it exported");
                give_back(LED_RUNNING, left_changed)?;
            }
        } else {
            // Two of `gpio_setup`'s three failure paths leave the pin
            // exported with no descriptor, which is the trap question 2
            // handles. Give it back, or question 8's listing reports
            // two leaked pins rather than the one it means.
            println!("  skipped: gpio_setup returned {fd3}");
            give_back(LED_RUNNING, left_changed)?;
            // And fail the pass, rather than print `skipped:` and go on
            // to exit 0. These four are called for one reason — so that
            // a probe which links and runs is evidence for all thirteen
            // symbols — and a pass that skipped them is evidence for
            // nine. docs/board-facts.md says flatly that this pass
            // calls every one, and a green run is what stands behind
            // that sentence.
            return Err(format!(
                "gpio_setup({LED_RUNNING}) returned {fd3}, so four of the thirteen went \
                 uncalled and this pass is not evidence for them"
            ));
        }

        // 7. A pin number no chip covers.
        println!("\n-- 7. a pin nothing can honour --");
        println!("gpio_export({NO_SUCH_PIN}) = {}", unsafe {
            gpio_export(NO_SUCH_PIN)
        });
        // The return is not the answer here either. On this board no
        // chip covers 99999 — the highest is 631 plus its lines — so
        // the write fails and there is nothing to give back. Asked
        // anyway because `--release` cannot: `NO_SUCH_PIN` is not in
        // `RELEASABLE`, and one exported pin outside the three
        // `a_run_is_up` exempts makes every later `--release` decline,
        // after which nothing in this tree can give back the LEDs.
        if exported(NO_SUCH_PIN) {
            println!("  it took after all");
            if let Err(why) = give_back(NO_SUCH_PIN, left_changed) {
                // The one pin whose survival is worse than the message
                // `give_back` prints: it is not in `RELEASABLE`, so
                // every later `--release` declines while it is there
                // and nothing in this tree can give back the LEDs.
                eprintln!(
                    "  and gpio{NO_SUCH_PIN} is not one `--release` acts on, so every \
                     later release declines while it is exported"
                );
                return Err(why);
            }
        }

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
            // And fail the pass, for the same reason the "remaining
            // four" block does: the listing the script prints after
            // this process exits is the same either way, so a pass that
            // could not ask this and still returned 0 would be read as
            // one that answered it.
            return Err(format!(
                "gpio{LED_UNDERRUN} was already exported, so question 8 could not be put"
            ));
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

    /// Says whether a run was still up when the with-run questions
    /// finished, and fails the pass where it was not: answers gathered
    /// after a run ended are answers about an idle board under a
    /// heading that says otherwise.
    ///
    /// Asked of the pins the run held at entry, not of `DIGITAL_D0`,
    /// which the probe exports itself where it finds it free — so a
    /// `D0` that is exported now is not evidence of anything.
    fn still_a_run(was: &[u32], left_changed: &mut bool) -> Result<(), String> {
        println!("\n-- was a run still up when these finished? --");
        if let Some(pin) = was.iter().find(|pin| exported(**pin)) {
            println!("  yes: gpio{pin} is still exported, as it was when these began");
            return Ok(());
        }
        // Each pin's `was_exported` is read before the `gpio_export`
        // two lines under it, so a run that tore down in that window
        // leaves the export the probe's own with `ours` not recording
        // it. For the LEDs that costs nothing — `--release` gives both
        // back — but `DIGITAL_D0` is outside `RELEASABLE` *and* outside
        // `a_run_is_up`'s exemptions, so one left here makes every later
        // `--release` decline, after which nothing in this tree can give
        // back the LEDs. Only where the run's own pins are gone, which
        // is teardown having run: a run killed hard leaves all
        // twenty-two, and `D0` is then one of those rather than ours.
        if exported(DIGITAL_D0) {
            println!("  the run is gone and gpio{DIGITAL_D0} is not, so it is this probe's");
            if let Err(why) = give_back(DIGITAL_D0, left_changed) {
                eprintln!("  {why}, and it is the one pin `--release` cannot act on");
            }
        }
        println!("  NO — the run ended part way through, and the answers");
        println!("  above are not all about a board that was rendering");
        Err(format!(
            "none of the {} pins the run held when these began is exported now, so it \
             ended part way through and these are not answers about a board that was \
             rendering",
            was.len()
        ))
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one question per block, in the order the module doc numbers them"
    )]
    /// Sets `left_changed` where question 11's write or question 12's
    /// direction restore would not go back, which is a status of its
    /// own rather than a failure to ask.
    fn with_run_questions(destructive: bool, left_changed: &mut bool) -> Result<(), String> {
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
        // Taken now, while the run is provably up and before this pass
        // has exported anything, so that the closing check reads the
        // run's own pins rather than the probe's.
        let was = run_pins()?;

        // 9 and 10: claiming and reading pins libbela is holding.
        // Whatever we exported that libbela had not — which happens
        // with `enable_led` off, where it claims neither LED — is ours
        // to give back, or the closing listing reports our own leak as
        // an answer.
        let mut ours: Vec<u32> = Vec::new();
        for (name, pin) in claimed {
            println!("\n-- {name} (gpio{pin}) --");
            // One read, used twice. The run can end between two of
            // them — `sine` reaching its own timeout while this is on
            // the fourth pin — and then the line printed and the fact
            // `ours` is built from would disagree about who held the
            // pin, which is the distinction every answer here turns on.
            let was_exported = exported(pin);
            println!("  exported before we ask: {was_exported}");
            println!("  direction: {}", direction(pin));
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
            if read_before != 0 {
                // Question 6's rule, and for the same reason: only write
                // what can be put back. Restoring to `LOW` here — which
                // is what this did — puts back a level nothing measured,
                // on a channel the PRU may have been driving high.
                println!("  not attempted: gpio_get_value returned {read_before}, so the");
                println!("  level to put back is not known and a write could not be undone");
            } else {
                let wrote = unsafe { gpio_set_value(DIGITAL_D0, arg::HIGH) };
                println!("  gpio_set_value(HIGH) = {wrote}");
                let mut value: PIN_VALUE = 0xdead_beef;
                let ret = unsafe { gpio_get_value(DIGITAL_D0, &raw mut value) };
                println!("  reads back: ret {ret}, *value {value:#x}");
                // What decides whether a restore is owed is whether the
                // write took, not what the pin reads back as: a write that
                // succeeded and then read back wrong would otherwise leave a
                // PRU channel driven for the rest of the run.
                if wrote == 0 {
                    let restore = if before == 0 { arg::LOW } else { arg::HIGH };
                    let put_back = unsafe { gpio_set_value(DIGITAL_D0, restore) };
                    println!("  restoring to what it held ({before}): {put_back}");
                    if put_back != 0 {
                        // Reachable only under `--destructive` on a board
                        // where D0 is an output — this one has it as an
                        // input, where the write above cannot take. There
                        // the channel is now driven against the PRU until
                        // the run ends, which is the same class of thing as
                        // question 6's trigger and is treated the same way:
                        // loudly, and the pass fails. Not here, though —
                        // the pins below are still owed back first.
                        eprintln!(
                            "LEFT CHANGED: gpio{DIGITAL_D0} was written HIGH and would not go \
                         back ({put_back}); it is driving against the PRU until the run ends"
                        );
                        *left_changed = true;
                    }
                } else {
                    println!("  the write did not take, so there is nothing to put back");
                }
            }
        }

        // 12: the destructive one.
        if destructive {
            println!("\n-- DESTRUCTIVE: unexporting the running LED out from under the run --");
            // What has to hold is that libbela is holding the pin
            // *now*, and `ours` does not say that. It is empty both
            // when libbela had the pin already and when the probe's own
            // `gpio_export` failed on a free one — a run with
            // `enable_led` off claims neither LED, so that pairing is
            // reachable. In the second case the branch below would
            // export the pin fresh and dismiss it: the probe taking a
            // pin from itself, printed under a heading that says it
            // took one from the run. So ask the pin, not the bookkeeping.
            if !exported(LED_RUNNING) || ours.contains(&LED_RUNNING) {
                println!("  NOT asked: gpio{LED_RUNNING} is not a pin libbela is holding");
                println!(
                    "  — it is {}, so there is nothing here to take from the run",
                    if ours.contains(&LED_RUNNING) {
                        "exported because this probe exported it"
                    } else {
                        "not exported at all"
                    }
                );
            } else {
                // Read before the call, because `gpio_setup` sets the
                // direction before it opens: the failure branch below
                // has to put back what the pin held, and cannot know
                // that afterwards.
                let direction_before = direction(LED_RUNNING);
                // As the direction it already holds. `gpio_setup` needs
                // *a* direction to open a descriptor, and the question
                // is the claim and the unexport, not the direction —
                // but an unexport keeps whatever it finds, measured, so
                // asking as an input would latch the run's LED pin as
                // one for the rest of the run, which is a state the PRU
                // cannot drive at all. That is what the failure branch
                // below calls LEFT CHANGED; the success path must not
                // do it silently.
                let ask_as = if direction_before == "in" {
                    arg::INPUT
                } else {
                    arg::OUTPUT
                };
                let fd = unsafe { gpio_setup(LED_RUNNING, ask_as) };
                println!("  gpio_setup = {fd} (its direction was {direction_before})");
                if fd < 0 {
                    // `gpio_dismiss` would unexport the pin anyway, so
                    // the destructive act would still happen — but as a
                    // bare unexport rather than a claim then a release,
                    // and the transcript would not say which.
                    println!("  NOT proceeding: what follows would be a bare unexport,");
                    println!("  which is a different thing from taking a pin we held");
                    // `gpio_setup` sets the direction before it opens,
                    // and it is asked for the one the pin already had,
                    // so the ordinary way here leaves the direction as
                    // it was. The other two branches are for a
                    // `gpio_setup` that failed at the export, before
                    // touching direction, and for a pin that has since
                    // stopped reading.
                    let now = direction(LED_RUNNING);
                    let readable = |d: &str| d == "in" || d == "out";
                    if now == direction_before {
                        println!("  its direction is unchanged, so nothing to put back");
                        // The direction, not the level. libbela's "only
                        // write if it has changed" guard never fires: it
                        // compares `out` against an unterminated
                        // `read(fd, buf, 4)`, so `strcmp` is never zero
                        // (`GPIOcontrol.cpp:138-147`). Writing a
                        // direction drives the line low, measured — and
                        // the PRU drives this pin every block and takes
                        // it back, which is why this is a line of
                        // transcript rather than a LEFT CHANGED.
                        println!("  the write still happened, and writing a direction drives");
                        println!("  the line low; the PRU takes this pin back within a block");
                    } else if !readable(&now) {
                        // `direction` reports a read failure as its own
                        // message, so `now` is not a direction here and
                        // the branch below would call `gpio_set_dir` on
                        // a pin it cannot address, then describe the
                        // failure as an input. Not knowing is not the
                        // same as nothing having changed: `gpio_setup`
                        // sets the direction before it opens.
                        println!("  its direction no longer reads as one ({now}), so whether");
                        println!("  it was changed cannot be established, let alone put back");
                        eprintln!(
                            "LEFT CHANGED: gpio{LED_RUNNING} was {direction_before} and its \
                             direction cannot be read now ({now}); it is not known to have \
                             been put back"
                        );
                        *left_changed = true;
                    } else if readable(&direction_before) {
                        let back = if direction_before == "out" {
                            arg::OUTPUT
                        } else {
                            arg::INPUT
                        };
                        let put_back = unsafe { gpio_set_dir(LED_RUNNING, back) };
                        println!("  restoring its direction to {direction_before}: {put_back}");
                        if put_back != 0 {
                            // Said first and instead: the message below
                            // opens with "is an output again", which
                            // would tell an operator the direction is
                            // fine and only the level is wrong.
                            eprintln!(
                                "LEFT CHANGED: gpio{LED_RUNNING}'s direction would not go back \
                                 ({put_back}); it is an input for the rest of the run, and the \
                                 run drives it as an output"
                            );
                            *left_changed = true;
                        } else if direction_before == "out" {
                            // Writing `out` to the `direction`
                            // attribute drives the pin low — measured,
                            // and in docs/board-facts.md — since only
                            // `high` and `low` carry a level and
                            // `gpio_set_dir` writes neither. So the
                            // direction is back and the level is not.
                            // Not LEFT CHANGED, though: "The board
                            // LEDs" records the PRU blinking this pin
                            // by writing the GPIO bank directly, ten
                            // reads at 100 ms giving `1011010101`, so
                            // an output it drives is one it takes back
                            // within a block. The branch above, where
                            // the direction itself would not go back,
                            // is the one that lasts — the PRU cannot
                            // drive an input.
                            println!("  its level is not restored with it: writing `out`");
                            println!("  drives the pin low, and only the direction came back.");
                            println!("  The PRU drives this pin every block, so it takes it");
                            println!("  back; see \"The board LEDs\" in docs/board-facts.md.");
                        }
                    } else {
                        // `direction` reports a read failure as its own
                        // message rather than a direction, and there is
                        // nothing to restore to. Saying so beats
                        // setting it to a guess and calling that a
                        // restore.
                        println!("  its direction could not be read before this, so there");
                        println!("  is nothing to put it back to; it now reads {now}");
                        // Not knowing what was changed is not the same
                        // as nothing having been: `gpio_setup` sets the
                        // direction before it opens, so the reachable
                        // way here is a pre-read that failed, a
                        // `gpio_set_dir` that took and an open that did
                        // not. Its two sibling branches call exactly
                        // that state LEFT CHANGED.
                        eprintln!(
                            "LEFT CHANGED: gpio{LED_RUNNING} reads {now} and what it held \
                             before could not be read, so it is not restored; the run \
                             drives it as an output"
                        );
                        *left_changed = true;
                    }
                } else {
                    println!("  gpio_dismiss = {}", unsafe {
                        gpio_dismiss(fd, LED_RUNNING)
                    });
                    // `gpio_dismiss` returns `0` whatever happened, so
                    // the pin says whether it took and the call does
                    // not — question 5 measures the silent unexport
                    // failure inside it. Still exported here is not the
                    // destruction this question asks about: it is the
                    // run's LED claimed *and* forced to an input by the
                    // `gpio_setup` above, while the run drives it as an
                    // output. So act on the answer rather than print
                    // it, as the two sibling dismiss sites do.
                    let mut survived = exported(LED_RUNNING);
                    println!("  still exported: {survived}");
                    if survived {
                        println!("  gpio_unexport = {}", unsafe {
                            gpio_unexport(LED_RUNNING)
                        });
                        survived = exported(LED_RUNNING);
                        println!("  still exported after that: {survived}");
                    }
                    if survived {
                        eprintln!(
                            "LEFT CHANGED: gpio{LED_RUNNING} survived gpio_dismiss and \
                             gpio_unexport both; it is exported and an input for the rest \
                             of the run, which drives it as an output"
                        );
                        *left_changed = true;
                    }
                    println!("  what this did to the run is the script's to report");
                }
            }
        } else {
            println!("\n(skipping the destructive question; pass --destructive for it)");
        }

        if ours.is_empty() {
            // What `ours` records is "went from free to exported while
            // this pass asked", and nothing more. It is also empty
            // where the probe's own `gpio_export` failed on a free pin,
            // which `enable_led` off makes reachable — so "libbela had
            // already exported every pin", which this used to print, is
            // a conclusion the bookkeeping does not carry. The
            // destructive question above asks the pin for exactly this
            // reason.
            println!("\n(no pin went from free to exported while this pass asked,");
            println!(" so there is nothing here of the probe's to give back)");
        } else {
            println!("\n-- giving back the pins libbela had not exported --");
            for pin in &ours {
                // Reported, not returned: the questions are all asked by
                // now, and a pin that survives is one `--release` will
                // meet again. What it costs is question 13's listing,
                // which the script says holds only what the probe left
                // on purpose — so say which pin is in it and why.
                if let Err(why) = give_back(*pin, left_changed) {
                    eprintln!("  {why}, so question 13's listing shows it");
                }
            }
        }

        // The entry check said a run was up. Say whether one still is,
        // so that a probe which outlived the run cannot have its
        // answers read as answers about a board that was rendering.
        still_a_run(&was, left_changed)?;

        println!("\nleaving /sys/class/gpio at: {}", listing());
        Ok(())
    }

    /// What `/sys/class/gpio` holds, with the `gpiochip*` directories
    /// and the `export`/`unexport` attribute files left out: all of
    /// them are always there and none says who claimed what. What is
    /// left is exactly the set of claimed pins, so two listings can be
    /// compared directly.
    /// The pins a run is holding right now, minus the four this probe
    /// asks about — so a later reading of the same set answers "is that
    /// run still up" without the answer depending on what the probe
    /// itself exported.
    ///
    /// Read rather than named. Which pins a run takes depends on the
    /// board and the settings: `PRU::prepareGPIO` gates the analog
    /// chip selects on `analogFrames` and the digital channels on
    /// `digitalFrames`, and the ADC reset is opened only for a
    /// `BelaMini` or a Gem Stereo with analog input on
    /// (`core/PRU.cpp:402-411`). Any pin named here in advance would
    /// therefore be absent on some run, and reading "the run is gone"
    /// off that is how a live run's channel gets unexported.
    fn run_pins() -> Result<Vec<u32>, String> {
        // Not an empty `Vec` on a read failure: downstream that is
        // indistinguishable from a run holding nothing, and the caller
        // reads *that* as the run having ended — after which it
        // unexports a channel the PRU is driving. `a_run_is_up` treats
        // the same failure as an error for the same reason.
        let entries = fs::read_dir("/sys/class/gpio").map_err(|e| {
            format!(
                "/sys/class/gpio could not be read ({e}), so which pins the run holds \
                 is unknown"
            )
        })?;
        Ok(entries
            .filter_map(Result::ok)
            .filter_map(|e| {
                e.file_name()
                    .to_string_lossy()
                    .strip_prefix("gpio")
                    .and_then(|rest| rest.parse::<u32>().ok())
            })
            .filter(|pin| !matches!(*pin, LED_RUNNING | LED_UNDERRUN | STOP_BUTTON | DIGITAL_D0))
            .collect())
    }

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
