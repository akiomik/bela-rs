//! Measures what the sysfs GPIO family does on a board.
//!
//! An instrument, not a check: nothing here passes or fails, and it is
//! run by a person reading the transcript. `scripts/probe-gpio.sh`
//! builds it, copies it over and runs it in both modes; the answers go
//! in `docs/board-facts.md`.
//!
//! It creates **no audio system**, which is what lets the second mode
//! exist: libbela refuses a second one in another process, so a probe
//! that brought its own could not ask what an application gets while a
//! run is up. The script starts `bela/examples/sine` for it to reach past.
//!
//! `--with-run` needs something rendering; `--destructive` can stop it.
//!
//! Alone: 1 export twice; 2 `gpio_setup`; 3 repeated `gpio_read` on one
//! descriptor; 4 `gpio_write` then `gpio_read`; 5 `gpio_dismiss` and a
//! second unexport; 6 `led_set_trigger` numbers; 7 a pin no chip
//! covers; 8 does an export outlive the process.
//!
//! With a run up: 9 claiming one of libbela's pins; 10 reading one; 11
//! writing a PRU-driven channel; 12 (`--destructive`) unexporting one
//! out from under the run; 13 what is claimed once both have gone.
//!
//! 8 and 13 are answered by the script, which lists `/sys/class/gpio`
//! once this process has exited — the only place they can be seen from.
//!
//! It leaves the board's GPIO in whatever state its last question left:
//! exports, directions and levels all survive an unexport, and a probe
//! killed mid-question leaves whatever it was holding. Putting that back
//! from here cannot be done — a `kill -9` runs none of this code — so
//! nothing here tries. `scripts/probe-gpio.sh` reboots the board
//! instead, which puts all of it back and needs no code at all.

fn main() {
    imp::main();
}

#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
mod imp {
    use std::ffi::{c_char, c_int, c_void};
    use std::{env, fs, process, ptr};

    unsafe extern "C" {
        /// Question 3's control. `gpio_read` misbehaves because it
        /// never rewinds, so rewinding for it is the only evidence for
        /// *why* rather than *that*. Declared here because this crate
        /// has no `libc` dependency.
        fn lseek(fd: c_int, offset: i64, whence: c_int) -> i64;

        /// C's own `stdout`, a different stream from the one `println!`
        /// writes to and buffered on its own terms.
        static mut stdout: *mut c_void;
        fn setvbuf(stream: *mut c_void, buf: *mut c_char, mode: c_int, size: usize) -> c_int;
    }

    const SEEK_SET: c_int = 0;
    /// `_IONBF`, read from the board's `/usr/include/stdio.h`.
    const IONBF: c_int = 2;

    use bela_sys::{
        PIN_VALUE, gpio_dismiss, gpio_export, gpio_fd_close, gpio_fd_open, gpio_get_value,
        gpio_read, gpio_set_dir, gpio_set_edge, gpio_set_value, gpio_setup, gpio_unexport,
        gpio_write, led_set_trigger,
    };

    /// Bank 0 is `600000.gpio` and bank 1 `601000.gpio`, both measured
    /// in "The board LEDs" in `docs/board-facts.md`; `GPIOn_m` is
    /// `BASE_n + m`.
    const BANK0: u32 = 539;
    const BANK1: u32 = 631;

    /// `GPIO0_45`, claimed for a run unless `enable_led` is off.
    const LED_RUNNING: u32 = BANK0 + 45;
    /// `GPIO0_46`, claimed with the one above.
    const LED_UNDERRUN: u32 = BANK0 + 46;
    /// `GPIO0_47`. libbela opens it with `unexport = false`, so it is
    /// exported before any run and stays — the board's resting state.
    const STOP_BUTTON: u32 = BANK0 + 47;
    /// `GPIO1_6`, header `P1_21`, from `digital_gpio_mapping.h`.
    const DIGITAL_D0: u32 = BANK1 + 6;
    const NO_SUCH_PIN: u32 = 99_999;

    /// The `PIN_DIRECTION`/`PIN_VALUE` constants as the `c_int` every
    /// parameter taking one actually wants — the first trap `bela_sys`
    /// documents.
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

    /// A `BeagleBone` has `usr0`..`usr3`; this board is measured to
    /// have `usr1`..`usr4`, so the sweep covers both.
    const LED_NUMBERS: &[u32] = &[0, 1, 2, 3, 4];

    fn trigger_path(lednum: u32) -> String {
        format!("/sys/class/leds/beaglebone:green:usr{lednum}/trigger")
    }

    /// The selected trigger, which sysfs marks with brackets among the
    /// ones it offers.
    fn current_trigger(lednum: u32) -> Option<String> {
        let contents = fs::read_to_string(trigger_path(lednum)).ok()?;
        let start = contents.find('[')? + 1;
        let end = contents[start..].find(']')? + start;
        Some(contents[start..end].to_owned())
    }

    fn exported(pin: u32) -> bool {
        fs::metadata(format!("/sys/class/gpio/gpio{pin}")).is_ok()
    }

    fn direction(pin: u32) -> String {
        fs::read_to_string(format!("/sys/class/gpio/gpio{pin}/direction"))
            .map_or_else(|e| format!("<{e}>"), |s| s.trim().to_owned())
    }

    /// What `/sys/class/gpio` holds, less the `gpiochip*` directories
    /// and the two attribute files, which are always there and say
    /// nothing: what is left is the set of claimed pins.
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

    /// Exported pins other than the four this probe asks about.
    ///
    /// An error, not an empty `Vec`: a caller reads that as a board at
    /// rest and goes on to unexport pins out from under a live run.
    fn other_pins() -> Result<Vec<u32>, String> {
        let entries = fs::read_dir("/sys/class/gpio")
            .map_err(|e| format!("/sys/class/gpio could not be read ({e})"))?;
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

    /// Whether anything looks like a run in progress: any exported pin
    /// other than the two LEDs, which this pass claims itself, and the
    /// stop button, which outlives every run.
    ///
    /// A run with `use_analog` and `use_digital` both off exports
    /// nothing this can see. sysfs offers no ownership, so two exported
    /// LEDs are the same bytes whoever left them; `bela/examples/sine`,
    /// which the script runs, has both settings on.
    fn a_run_is_up() -> Result<bool, String> {
        Ok(!other_pins()?.is_empty() || exported(DIGITAL_D0))
    }

    /// Unexports `pin` and then asks the pin, `gpio_unexport` being the
    /// call question 5 measures refusing silently.
    fn give_back(pin: u32) {
        let ret = unsafe { gpio_unexport(pin) };
        if exported(pin) {
            eprintln!(
                "LEFT CHANGED: gpio{pin} is still exported after gpio_unexport ({ret}); \
                 a reboot is what gives it back."
            );
        } else {
            println!("  gpio_unexport({pin}) = {ret}, and it is gone");
        }
    }

    pub(crate) fn main() {
        // Before anything is printed. libbela reports `gpio_setup`'s two
        // failures with C `printf`, and C stdio block-buffers to a pipe,
        // which is what ssh makes of remote stdout — so those lines
        // would be flushed at exit, away from the question behind them.
        unsafe { setvbuf(stdout, ptr::null_mut(), IONBF, 0) };

        let args: Vec<String> = env::args().skip(1).collect();
        if let Some(bad) = args
            .iter()
            .find(|a| !matches!(a.as_str(), "--with-run" | "--destructive"))
        {
            eprintln!("unknown argument: {bad}");
            process::exit(2);
        }

        let with_run = args.iter().any(|a| a == "--with-run");
        let destructive = args.iter().any(|a| a == "--destructive");
        if destructive && !with_run {
            // Not a warning: going on would run the alone pass — which
            // rewrites LED triggers and exports pins — in place of the
            // one asked for, and the unknown-argument check above exists
            // to stop exactly that substitution.
            eprintln!("--destructive is a question the with-run pass asks; add --with-run");
            process::exit(2);
        }

        let asked = if with_run {
            with_run_questions(destructive)
        } else {
            alone_questions()
        };
        if let Err(why) = asked {
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
        // This pass dismisses and unexports `LED_RUNNING`, which sysfs
        // grants whoever asks — the one act the probe gates.
        if a_run_is_up()? {
            return Err(format!(
                "a pin other than the two LEDs and the stop button is exported, so \
                 either a run is up and every answer below would be about a board that \
                 was rendering, or something else is holding pins: {}",
                listing()
            ));
        }
        for (what, pin) in [("running LED", LED_RUNNING), ("underrun LED", LED_UNDERRUN)] {
            if exported(pin) {
                return Err(format!(
                    "gpio{pin} ({what}) is exported and no pin a run would hold is, so an \
                     earlier probe left it. Reboot the board: nothing here gives a \
                     pin back across invocations, and a reboot gives back all of them"
                ));
            }
        }
        println!("at rest, /sys/class/gpio holds: {}", listing());

        println!("\n-- 1. export, twice --");
        println!(
            "gpio_export({LED_RUNNING}) = {}, exported now: {}",
            unsafe { gpio_export(LED_RUNNING) },
            exported(LED_RUNNING)
        );
        println!("gpio_export({LED_RUNNING}) again = {}", unsafe {
            gpio_export(LED_RUNNING)
        });
        // Question 2's heading depends on this: a pin still claimed
        // sends `gpio_setup` down `gpio_export`'s already-exported fast
        // path, and the row that reaches `docs/board-facts.md` says "on
        // a board where nothing had claimed anything".
        give_back(LED_RUNNING);
        if exported(LED_RUNNING) {
            return Err(format!(
                "gpio{LED_RUNNING} would not unexport, so question 2 would measure \
                 gpio_setup on a claimed pin under a heading that says a free one"
            ));
        }

        println!("\n-- 2. gpio_setup --");
        let fd = unsafe { gpio_setup(LED_RUNNING, arg::OUTPUT) };
        println!("gpio_setup({LED_RUNNING}, OUTPUT_PIN) = {fd}");
        println!("  direction file now: {}", direction(LED_RUNNING));
        if fd < 0 {
            // `gpio_setup` exports before it opens, so a failure here
            // can leave the pin claimed — the trap this probe exists to
            // document.
            give_back(LED_RUNNING);
            return Err(format!("gpio_setup on a free pin returned {fd}"));
        }

        println!("\n-- 3. gpio_read on one descriptor --");
        for n in 1..=3 {
            let mut value: PIN_VALUE = 0xdead_beef;
            let ret = unsafe { gpio_read(fd, &raw mut value) };
            println!("  read {n}: ret {ret}, *value {value:#x}");
        }
        let mut value: PIN_VALUE = 0xdead_beef;
        let ret = unsafe { gpio_get_value(LED_RUNNING, &raw mut value) };
        println!("gpio_get_value (opens its own file) = {ret}, *value {value:#x}");
        println!("  the same three, with an lseek back to 0 before each:");
        for n in 1..=3 {
            // Printed, not discarded: this row is the control that makes
            // the missing rewind the cause rather than a guess that
            // fits, so a seek that failed has to be visible in it.
            let sought = unsafe { lseek(fd, 0, SEEK_SET) };
            let mut value: PIN_VALUE = 0xdead_beef;
            let ret = unsafe { gpio_read(fd, &raw mut value) };
            println!("    read {n}: lseek {sought}, ret {ret}, *value {value:#x}");
        }
        println!("gpio_fd_close(fd) = {}", unsafe { gpio_fd_close(fd) });

        println!("\n-- 4. gpio_write, then gpio_read on the same descriptor --");
        let fd2 = unsafe { gpio_setup(LED_RUNNING, arg::OUTPUT) };
        println!("a fresh descriptor: {fd2}");
        if fd2 < 0 {
            give_back(LED_RUNNING);
            return Err(format!("gpio_setup for question 4 returned {fd2}"));
        }
        println!("gpio_write(fd2, HIGH) = {}", unsafe {
            gpio_write(fd2, arg::HIGH)
        });
        let mut value: PIN_VALUE = 0xdead_beef;
        let ret = unsafe { gpio_read(fd2, &raw mut value) };
        println!("  the very next gpio_read: ret {ret}, *value {value:#x}");
        // An unexport keeps the level, so a HIGH that stayed would stay
        // after this pass ended.
        // Reported like question 6's and question 11's: an unexport
        // keeps the level, and setting `in` does not pull the line down,
        // so a HIGH that stayed is a lit LED for the rest of the session.
        let put_back = unsafe { gpio_write(fd2, arg::LOW) };
        println!("  gpio_write(fd2, LOW) = {put_back}");
        if put_back != 0 {
            eprintln!(
                "LEFT CHANGED: gpio{LED_RUNNING} was written HIGH and would not go back \
                 ({put_back}); the reboot at the end of the script is what clears it"
            );
        }

        println!("\n-- 5. gpio_dismiss, then unexport again --");
        println!("gpio_dismiss = {}", unsafe {
            gpio_dismiss(fd2, LED_RUNNING)
        });
        println!("  still exported: {}", exported(LED_RUNNING));
        println!("gpio_unexport (second time) = {}", unsafe {
            gpio_unexport(LED_RUNNING)
        });

        println!("\n-- 6. led_set_trigger --");
        // The unreadable-file arm below does not call it, so a pass in
        // which every number took that arm would be evidence for twelve
        // of the thirteen while exiting 0 — which the block after this
        // one refuses for its own four.
        let mut trigger_called = false;
        for &n in LED_NUMBERS {
            // Read first, and only write what can be put back: a file
            // that exists but will not say what it holds would
            // otherwise be set to `none` and left there.
            let Some(before) = current_trigger(n) else {
                if fs::metadata(trigger_path(n)).is_ok() {
                    println!("  lednum {n}: not asked — its trigger could not be read");
                } else {
                    let ret = unsafe { led_set_trigger(n, c"none".as_ptr()) };
                    trigger_called = true;
                    println!("  lednum {n}: ret {ret} (no such file to begin with)");
                }
                continue;
            };
            let ret = unsafe { led_set_trigger(n, c"none".as_ptr()) };
            trigger_called = true;
            // `led_set_trigger` answers -1 for a failed `open` and a
            // failed `write` alike, so ask the file: a write that failed
            // after the attribute took `none` would otherwise be skipped
            // as nothing to restore.
            if ret != 0 && current_trigger(n).as_ref() == Some(&before) {
                println!("  lednum {n}: ret {ret}, still [{before}] — unchanged");
                continue;
            }
            println!("  lednum {n}: ret {ret} (was [{before}], restoring)");
            if let Err(e) = fs::write(trigger_path(n), &before) {
                let now = current_trigger(n).unwrap_or_else(|| "unreadable".to_owned());
                eprintln!(
                    "LEFT CHANGED: usr{n} reads `{now}` and was `{before}` ({e}); put it \
                     back with: echo {before} > {}",
                    trigger_path(n)
                );
                // Stop asking: whatever refused this is as likely to
                // hold for the next number, and going on would leave
                // four triggers changed rather than one.
                break;
            }
        }

        if !trigger_called {
            return Err(
                "every lednum named a file whose trigger could not be read, so \
                 led_set_trigger went uncalled and this pass is not evidence for it"
                    .to_owned(),
            );
        }

        // So that a probe which links and runs is evidence for all
        // thirteen symbols, not the nine this pass needs.
        println!("\n-- the remaining four --");
        let fd3 = unsafe { gpio_setup(LED_RUNNING, arg::OUTPUT) };
        if fd3 < 0 {
            give_back(LED_RUNNING);
            return Err(format!(
                "gpio_setup({LED_RUNNING}) returned {fd3}, so four of the thirteen went \
                 uncalled and this pass is not evidence for them"
            ));
        }
        println!("  gpio_set_dir(INPUT) = {}", unsafe {
            gpio_set_dir(LED_RUNNING, arg::INPUT)
        });
        println!("  gpio_set_edge(\"none\") = {}", unsafe {
            gpio_set_edge(LED_RUNNING, c"none".as_ptr().cast_mut())
        });
        let ro = unsafe { gpio_fd_open(LED_RUNNING, 0) };
        println!("  gpio_fd_open(O_RDONLY) = {ro}");
        if ro >= 0 {
            println!("  gpio_fd_close = {}", unsafe { gpio_fd_close(ro) });
        }
        // The pin is an input by now, so this cannot take; the point is
        // the link, and `-1` on an input is what question 11 measures.
        println!("  gpio_set_value(LOW) = {}", unsafe {
            gpio_set_value(LED_RUNNING, arg::LOW)
        });
        // `gpio_dismiss` returns 0 whatever happened, so ask the pin.
        let _ = unsafe { gpio_dismiss(fd3, LED_RUNNING) };
        if exported(LED_RUNNING) {
            println!("  gpio_dismiss left it exported");
            give_back(LED_RUNNING);
            if exported(LED_RUNNING) {
                return Err(format!(
                    "gpio{LED_RUNNING} survived both, so question 8's listing would show \
                     two pins and mean one"
                ));
            }
        }

        println!("\n-- 7. a pin nothing can honour --");
        println!("gpio_export({NO_SUCH_PIN}) = {}", unsafe {
            gpio_export(NO_SUCH_PIN)
        });
        if exported(NO_SUCH_PIN) {
            println!("  it took after all");
            give_back(NO_SUCH_PIN);
        }

        println!("\n-- 8. does an export outlive the process that made it? --");
        // Whether the export is *ours* cannot be read off the return:
        // `gpio_export` answers 0 for a pin it merely found.
        let was_exported = exported(LED_UNDERRUN);
        println!("gpio_export({LED_UNDERRUN}) = {}", unsafe {
            gpio_export(LED_UNDERRUN)
        });
        if was_exported || !exported(LED_UNDERRUN) {
            return Err(format!(
                "gpio{LED_UNDERRUN} was {}, so question 8 could not be put — and the \
                 listing that answers it is the same either way",
                if was_exported {
                    "already exported"
                } else {
                    "left unexported by the call"
                }
            ));
        }
        println!("  exiting now WITHOUT unexporting it, on purpose");
        println!("\nleaving /sys/class/gpio at: {}", listing());
        Ok(())
    }

    /// Questions 9 and 10. Returns the pins that went from free to
    /// exported while it asked — the probe's own, to be given back;
    /// `enable_led` off is what makes that reachable.
    fn claim_and_read(claimed: &[(&str, u32)]) -> Vec<u32> {
        let mut ours: Vec<u32> = Vec::new();
        for &(name, pin) in claimed {
            println!("\n-- 9 and 10. {name} (gpio{pin}) --");
            // One read, used twice: the run can end between two of
            // them, and then the line printed and the fact `ours` is
            // built from would disagree about who held the pin.
            let was_exported = exported(pin);
            println!("  exported before we ask: {was_exported}");
            println!("  direction: {}", direction(pin));
            println!("  gpio_export = {}", unsafe { gpio_export(pin) });
            // The stop button is left exported whoever claimed it: that
            // is the resting state `docs/board-facts.md` records.
            if !was_exported && exported(pin) && pin != STOP_BUTTON {
                ours.push(pin);
            }
            let mut value: PIN_VALUE = 0xdead_beef;
            let ret = unsafe { gpio_get_value(pin, &raw mut value) };
            println!("  gpio_get_value = {ret}, *value {value:#x}");
        }
        ours
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one question per block, in the order the module doc numbers them"
    )]
    fn with_run_questions(destructive: bool) -> Result<(), String> {
        println!("== with a run up: something else must be holding the audio device ==");
        println!("/sys/class/gpio holds: {}", listing());

        if !exported(DIGITAL_D0) {
            return Err(format!(
                "gpio{DIGITAL_D0} (digital D0) is not exported: either nothing is \
                 rendering, or a run is up with `use_digital` off, and these questions \
                 need the pins a run holds"
            ));
        }
        // Read while the run is provably up and before this pass exports
        // anything. Which pins a run takes depends on the board and the
        // settings, so the set is read, not named.
        let was = other_pins()?;
        if was.is_empty() {
            return Err(format!(
                "gpio{DIGITAL_D0} is exported but no other pin is, so there is nothing \
                 here that only a run would hold"
            ));
        }

        let ours = claim_and_read(&[
            ("running LED", LED_RUNNING),
            ("underrun LED", LED_UNDERRUN),
            ("stop button", STOP_BUTTON),
            ("digital D0", DIGITAL_D0),
        ]);

        println!("\n-- 11. writing gpio{DIGITAL_D0} while the PRU drives it --");
        let d0_direction = direction(DIGITAL_D0);
        println!("  its direction is {d0_direction}");
        if d0_direction == "out" && !destructive {
            println!("  not attempted: writing an output pin contends with its driver,");
            println!("  which is what --destructive is for");
        } else if d0_direction != "in" && d0_direction != "out" {
            println!("  not attempted: its direction could not be read");
        } else {
            let mut before: PIN_VALUE = 0xdead_beef;
            let read_before = unsafe { gpio_get_value(DIGITAL_D0, &raw mut before) };
            if read_before != 0 {
                // Question 6's rule: only write what can be put back.
                println!("  not attempted: gpio_get_value returned {read_before}, so the");
                println!("  level to put back is not known");
            } else {
                let wrote = unsafe { gpio_set_value(DIGITAL_D0, arg::HIGH) };
                println!("  gpio_set_value(HIGH) = {wrote}");
                let mut value: PIN_VALUE = 0xdead_beef;
                let ret = unsafe { gpio_get_value(DIGITAL_D0, &raw mut value) };
                println!("  reads back: ret {ret}, *value {value:#x}");
                // Whether the write took decides whether a restore is
                // owed, not what it reads back as.
                if wrote == 0 {
                    let restore = if before == 0 { arg::LOW } else { arg::HIGH };
                    let put_back = unsafe { gpio_set_value(DIGITAL_D0, restore) };
                    println!("  restoring to what it held ({before}): {put_back}");
                    if put_back != 0 {
                        eprintln!(
                            "LEFT CHANGED: gpio{DIGITAL_D0} was written HIGH and would not \
                             go back ({put_back}); it drives against the PRU until the run \
                             ends"
                        );
                    }
                } else {
                    println!("  the write did not take, so there is nothing to put back");
                }
            }
        }

        if destructive {
            println!("\n-- 12. DESTRUCTIVE: unexporting the running LED under the run --");
            destructive_question(&ours);
        } else {
            println!("\n(skipping question 12; pass --destructive for it)");
        }

        if ours.is_empty() {
            println!("\n(no pin went from free to exported while this pass asked)");
        } else {
            println!("\n-- giving back the pins libbela had not exported --");
            for pin in &ours {
                give_back(*pin);
            }
        }

        println!("\n-- was a run still up when these finished? --");
        if let Some(pin) = was.iter().find(|pin| exported(**pin)) {
            println!("  gpio{pin} is still exported, as it was when these began. A run");
            println!("  killed without libbela's teardown leaves the same pins, so how");
            println!("  the run ended is the script's to report.");
        } else {
            println!("  NO — the run is gone. If question 12 was asked, this may be its");
            println!("  answer; the script reports how the run ended, and 124 is the");
            println!("  undisturbed end. Otherwise the answers above are not all about a");
            println!("  board that was rendering.");
            // A run tearing down between a pin's `was_exported` read and
            // its `gpio_export` leaves the export the probe's own with
            // `ours` not recording it. All three pins this pass claims go
            // through that window — the LEDs first, so they are the more
            // exposed — and question 13's listing is what pays for it.
            // Reached only where the run's own pins are gone, which is
            // teardown having run rather than a hard kill, so anything
            // still here is this probe's.
            for pin in [LED_RUNNING, LED_UNDERRUN, DIGITAL_D0] {
                if exported(pin) {
                    println!("  gpio{pin} outlived it, so it is this probe's");
                    give_back(pin);
                }
            }
        }

        println!("\nleaving /sys/class/gpio at: {}", listing());
        Ok(())
    }

    /// Question 12.
    fn destructive_question(ours: &[u32]) {
        // The pin, not the bookkeeping: `ours` is empty both when
        // libbela had the pin already and when the probe's own
        // `gpio_export` failed on a free one.
        if !exported(LED_RUNNING) || ours.contains(&LED_RUNNING) {
            println!("  NOT asked: gpio{LED_RUNNING} is not a pin libbela is holding");
            return;
        }
        // As the direction it already holds: `gpio_setup` needs only
        // *a* direction to open a descriptor, and an unexport keeps the
        // one it finds, so asking as an input would latch the run's LED
        // pin as one for the rest of the run.
        let before = direction(LED_RUNNING);
        let ask_as = if before == "in" {
            arg::INPUT
        } else {
            arg::OUTPUT
        };
        let fd = unsafe { gpio_setup(LED_RUNNING, ask_as) };
        println!("  gpio_setup = {fd} (its direction was {before})");
        if fd < 0 {
            println!("  NOT proceeding: what follows would be a bare unexport");
            if !exported(LED_RUNNING) {
                println!("  and the pin is gone, so there is nothing to put back");
                return;
            }
            let now = direction(LED_RUNNING);
            if now != before && (before == "in" || before == "out") {
                let back = if before == "out" {
                    arg::OUTPUT
                } else {
                    arg::INPUT
                };
                println!("  restoring its direction to {before}: {}", unsafe {
                    gpio_set_dir(LED_RUNNING, back)
                });
            } else if now == before {
                // Not untouched. libbela's `gpio_set_dir` compares its
                // argument against a `read(fd, buf, 4)` that leaves the
                // buffer unterminated, so the `strcmp` never matches and
                // it always writes — and writing a direction drives the
                // line low. The PRU drives this pin every block and
                // takes it back, which is why this is a line of
                // transcript and not a LEFT CHANGED.
                println!("  its direction still reads {before}, but gpio_setup wrote it:");
                println!("  that drives the line low, and the PRU takes this pin back");
            } else {
                println!("  its direction reads {now} and was {before}, which is not a");
                println!("  direction, so there is nothing to put it back to");
            }
            return;
        }
        println!("  gpio_dismiss = {}", unsafe {
            gpio_dismiss(fd, LED_RUNNING)
        });
        println!("  still exported: {}", exported(LED_RUNNING));
        if exported(LED_RUNNING) {
            give_back(LED_RUNNING);
        }
        println!("  what this did to the run is the script's to report");
    }
}

#[cfg(not(all(target_arch = "aarch64", target_os = "linux")))]
mod imp {
    use std::process;

    pub(crate) fn main() {
        eprintln!("this probe only runs on the board; see scripts/probe-gpio.sh");
        process::exit(2);
    }
}
