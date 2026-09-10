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

    /// The two pins `--release` gives back. The stop button is excluded
    /// because unexporting it would change the resting state a later
    /// run measures.
    const RELEASABLE: &[u32] = &[LED_RUNNING, LED_UNDERRUN];

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
    fn other_pins() -> Vec<u32> {
        let Ok(entries) = fs::read_dir("/sys/class/gpio") else {
            return Vec::new();
        };
        entries
            .filter_map(Result::ok)
            .filter_map(|e| {
                e.file_name()
                    .to_string_lossy()
                    .strip_prefix("gpio")
                    .and_then(|rest| rest.parse::<u32>().ok())
            })
            .filter(|pin| !matches!(*pin, LED_RUNNING | LED_UNDERRUN | STOP_BUTTON | DIGITAL_D0))
            .collect()
    }

    /// Whether anything looks like a run in progress. The LEDs cannot be
    /// the signal — they are what `--release` gives back — and the stop
    /// button outlives every run.
    fn a_run_is_up() -> bool {
        !other_pins().is_empty() || exported(DIGITAL_D0)
    }

    /// Unexports `pin` and then asks the pin, `gpio_unexport` being the
    /// call question 5 measures refusing silently.
    fn give_back(pin: u32) {
        let ret = unsafe { gpio_unexport(pin) };
        if exported(pin) {
            eprintln!(
                "LEFT CHANGED: gpio{pin} would not unexport ({ret}); `--release` is the \
                 same call, so it cannot give it back either"
            );
        } else {
            println!("  gpio_unexport({pin}) = {ret}, and it is gone");
        }
    }

    /// Gives back the two LED pins whether or not this invocation
    /// claimed them. Tracking which were ours needs a ledger that an
    /// interrupt can still land inside; three attempts at one were the
    /// substance of eight review rounds.
    fn release_all() -> Result<(), String> {
        println!("== releasing the two LED pins, claimed or not ==");
        if a_run_is_up() {
            return Err(format!(
                "NOT released: a pin other than gpio{LED_RUNNING} and gpio{LED_UNDERRUN} \
                 is exported, so either a run is up and the LEDs are its, or one was \
                 killed hard enough to skip libbela's teardown. Either way they are not \
                 this probe's to take back. /sys/class/gpio holds: {}",
                listing()
            ));
        }
        let mut held = Vec::new();
        for &pin in RELEASABLE {
            let was = exported(pin);
            let ret = unsafe { gpio_unexport(pin) };
            println!(
                "  gpio_unexport({pin}) = {ret} (was {}, now {})",
                if was { "exported" } else { "free" },
                if exported(pin) { "exported" } else { "free" }
            );
            if exported(pin) {
                held.push(pin.to_string());
            }
        }
        // The pin, not the return: `gpio_unexport` refuses silently,
        // which question 5 measures.
        if held.is_empty() {
            return Ok(());
        }
        Err(format!(
            "NOT released: gpio{} would not unexport, silently",
            held.join(", gpio")
        ))
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
            .find(|a| !matches!(a.as_str(), "--with-run" | "--destructive" | "--release"))
        {
            eprintln!("unknown argument: {bad}");
            process::exit(2);
        }

        if args.iter().any(|a| a == "--release") {
            if let Some(other) = args.iter().find(|a| *a != "--release") {
                eprintln!("--release does its work and stops; {other} cannot come with it");
                process::exit(2);
            }
            if let Err(why) = release_all() {
                eprintln!("{why}");
                process::exit(2);
            }
            return;
        }

        let with_run = args.iter().any(|a| a == "--with-run");
        let destructive = args.iter().any(|a| a == "--destructive");
        if destructive && !with_run {
            eprintln!("--destructive only applies to --with-run, and is being ignored");
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
        if a_run_is_up() {
            return Err(format!(
                "a pin a run would hold is exported, so every answer below would be \
                 about a board that was rendering: {}",
                listing()
            ));
        }
        for (what, pin) in [("running LED", LED_RUNNING), ("underrun LED", LED_UNDERRUN)] {
            if exported(pin) {
                return Err(format!(
                    "gpio{pin} ({what}) is exported and no pin a run would hold is, so an \
                     earlier probe left it; `--release` gives it back"
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
            unsafe { lseek(fd, 0, SEEK_SET) };
            let mut value: PIN_VALUE = 0xdead_beef;
            let ret = unsafe { gpio_read(fd, &raw mut value) };
            println!("    read {n}: ret {ret}, *value {value:#x}");
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
        // after this pass ended. The link-only block below writes it
        // low again and reads it back, which is what settles this.
        println!("  putting it back to LOW: {}", unsafe {
            gpio_write(fd2, arg::LOW)
        });

        println!("\n-- 5. gpio_dismiss, then unexport again --");
        println!("gpio_dismiss = {}", unsafe {
            gpio_dismiss(fd2, LED_RUNNING)
        });
        println!("  still exported: {}", exported(LED_RUNNING));
        println!("gpio_unexport (second time) = {}", unsafe {
            gpio_unexport(LED_RUNNING)
        });

        println!("\n-- 6. led_set_trigger --");
        for &n in LED_NUMBERS {
            // Read first, and only write what can be put back: a file
            // that exists but will not say what it holds would
            // otherwise be set to `none` and left there.
            let Some(before) = current_trigger(n) else {
                if fs::metadata(trigger_path(n)).is_ok() {
                    println!("  lednum {n}: not asked — its trigger could not be read");
                } else {
                    let ret = unsafe { led_set_trigger(n, c"none".as_ptr()) };
                    println!("  lednum {n}: ret {ret} (no such file to begin with)");
                }
                continue;
            };
            let ret = unsafe { led_set_trigger(n, c"none".as_ptr()) };
            // `led_set_trigger` answers -1 for a failed `open` and a
            // failed `write` alike (`GPIOcontrol.cpp:338,341`), so ask
            // the file: a write that failed after the attribute took
            // `none` would otherwise be skipped as nothing to restore.
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

        // So that a probe which links and runs is evidence for all
        // thirteen symbols, not the nine this pass needs.
        println!("\n-- the remaining four, called only to link them --");
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
        // Back again, the pin still being exported: an unexport keeps
        // the direction it finds, so without this every pass leaves
        // gpio584 an input for a call made only to link a symbol.
        println!("  gpio_set_dir(OUTPUT), putting it back = {}", unsafe {
            gpio_set_dir(LED_RUNNING, arg::OUTPUT)
        });
        println!("  gpio_set_edge(\"none\") = {}", unsafe {
            gpio_set_edge(LED_RUNNING, c"none".as_ptr().cast_mut())
        });
        let ro = unsafe { gpio_fd_open(LED_RUNNING, 0) };
        println!("  gpio_fd_open(O_RDONLY) = {ro}");
        if ro >= 0 {
            println!("  gpio_fd_close = {}", unsafe { gpio_fd_close(ro) });
        }
        println!("  gpio_set_value(LOW) = {}", unsafe {
            gpio_set_value(LED_RUNNING, arg::LOW)
        });
        let mut level: PIN_VALUE = 0xdead_beef;
        let read = unsafe { gpio_get_value(LED_RUNNING, &raw mut level) };
        println!("  the line now reads: ret {read}, *value {level:#x}");
        if read != 0 || level != 0 {
            eprintln!(
                "LEFT CHANGED: gpio{LED_RUNNING} is not known to be low after three writes \
                 that reported success, and an unexport keeps what the line holds"
            );
        }
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
                "gpio{DIGITAL_D0} (digital D0) is not exported, so nothing is rendering"
            ));
        }
        // Read while the run is provably up and before this pass exports
        // anything. Which pins a run takes depends on the board and the
        // settings, so the set is read, not named.
        let was = other_pins();
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
            println!("  yes: gpio{pin} is still exported, as it was when these began");
        } else {
            println!("  NO — the run is gone. If question 12 was asked, this may be its");
            println!("  answer; the script reports how the run ended, and 124 is the");
            println!("  undisturbed end. Otherwise the answers above are not all about a");
            println!("  board that was rendering.");
            // A run tearing down between a pin's `was_exported` read and
            // its `gpio_export` leaves D0 the probe's own with `ours` not
            // recording it. Only where the run's own pins are gone, which
            // is teardown having run rather than a hard kill.
            if exported(DIGITAL_D0) {
                println!("  gpio{DIGITAL_D0} outlived it, so it is this probe's");
                give_back(DIGITAL_D0);
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
            } else {
                println!("  its direction reads {now}, and was {before}");
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
