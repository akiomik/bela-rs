//! Raw FFI bindings to the Bela core API (`libbela`) for [Bela Gem].
//!
//! This crate exposes the C surface of the Bela core API (`Bela.h`):
//! `BelaContext`, `BelaInitSettings`, the `Bela_*` lifecycle and
//! auxiliary-task functions, and `rt_printf`. Bindings are generated
//! from vendored headers (see `vendor/bela/COMMIT` for the pinned
//! upstream commit) with `cargo xtask bindgen`; see the crate README
//! for how to regenerate them.
//!
//! It also exposes the sysfs GPIO and LED family that `Bela.h`
//! includes from `GPIOcontrol.h`: the twelve [`gpio_*`](gpio_setup)
//! functions and [`led_set_trigger`]. It is a different mechanism
//! from the digital channels of a [`BelaContext`] rather than a
//! second spelling of them, and the only path here to a pin that is
//! not one of those sixteen — or to any pin at all outside the
//! moment a block is being rendered. It is also file I/O under
//! `/sys/class/gpio` and `/sys/class/leds`, most of the calls opening
//! and closing a file to move a single bit; `gpio_read` and
//! `gpio_write` are the pair that work on a descriptor already open.
//! So it belongs where a Bela program puts file I/O — in `setup`, in
//! `cleanup`, in an [`AuxiliaryTask`] or on a thread of its own — and
//! never in `render`. libbela claims some of these pins for itself
//! while a run is up; `docs/board-facts.md` in the repository records
//! which, and `docs/scope.md` records what a safe wrapper over these
//! is still waiting on.
//!
//! Eight things about the family are easy to get wrong:
//!
//! - **The constants are the wrong integer type.** [`PIN_DIRECTION`]
//!   and [`PIN_VALUE`] name the values its arguments take, but they
//!   are `c_uint` where every parameter that consumes one is `c_int`.
//!   `gpio_set_dir(pin, OUTPUT_PIN as c_int)` is the spelling that
//!   compiles, and so is `gpio_write(fd, HIGH as c_int)`.
//! - **`writeFlag` is not a flag.** `gpio_fd_open`'s second argument
//!   is the second argument of `open(2)`; `gpio_setup` passes
//!   `O_RDWR`, which is `2` on this board and on Linux generally.
//!   This crate is `no_std` and depends on no `libc`, so the constant
//!   is the caller's to bring — `libc::O_RDWR`, or the literal. Both
//!   `gpio_fd_open` and `gpio_setup` return a descriptor, where the
//!   rest of the family returns `0` for success and a negative value
//!   for failure.
//! - **`gpio_read` needs the descriptor rewound, and `gpio_write`
//!   moves it.** The two share one file offset and neither resets it,
//!   and a sysfs `value` file is two bytes — `"0\n"` or `"1\n"`. On a
//!   descriptor nothing has written to, the first `gpio_read` answers,
//!   the second reads the newline — which is not `'0'`, so it reports
//!   the pin *high* whatever the pin is doing — and every one after
//!   that reads nothing and returns `-1`. After a `gpio_write`, which
//!   writes two bytes, the offset is already at the end, so the very
//!   next `gpio_read` is the one that returns `-1`: writing a pin and
//!   reading it back on the descriptor `gpio_setup` gave you does not
//!   work at all. Either way the remedy is the caller's, an `lseek`
//!   back to 0 before each read. Both measured on a board;
//!   `docs/board-facts.md` in the repository has the transcripts.
//!   `gpio_get_value` has neither problem, opening and closing the
//!   file around each reading.
//! - **A reading is written only on success.** `gpio_get_value` and
//!   `gpio_read` leave their `*mut c_uint` untouched on every failure
//!   path, so it is not sound to hand either an uninitialised
//!   location and assume the pointee afterwards. The test is `< 0`
//!   rather than `== -1`: every failure in this family happens to be
//!   `-1`, the paths that pass one on getting it from a failed
//!   `open`, but a negative value is what the family promises.
//! - **`gpio_dismiss` returns `0` whatever happens.** It closes the
//!   descriptor and unexports the pin and discards what either of
//!   them said, so a pin that failed to unexport is reported as one
//!   that did not. It unexports whether or not this process was what
//!   exported the pin, too, which on one libbela holds is how a
//!   program takes an LED or the stop button away from a live run.
//! - **A failed `gpio_setup` can leave the pin exported.** It exports,
//!   sets the direction and then opens; if either of the last two
//!   fails it returns a negative value with the export already done
//!   and no descriptor to hand `gpio_dismiss`. Undoing that takes a
//!   `gpio_unexport` from the caller — but only where the export was
//!   this program's, which `gpio_export` cannot say, succeeding just
//!   as readily on a pin somebody else had already exported. So the
//!   cleanup that looks obvious here is the same call that takes
//!   libbela's pin away from a run. Skipping it instead leaves the
//!   pin in `/sys/class/gpio` after the process exits. One process
//!   does not get that success: the test is `if(fd > 0)`, so a
//!   program whose standard input is closed can be handed descriptor
//!   0 by the probe, miss the fast path, leak that descriptor and
//!   fail the export with `EBUSY` — reporting failure for a pin that
//!   is exported and perfectly usable.
//! - **The two string arguments have to be NUL-terminated.**
//!   `gpio_set_edge` and `led_set_trigger` both write `strlen(s) + 1`
//!   bytes, so a pointer into a Rust `&str` sends `strlen` off the end
//!   of it and puts whatever followed into the sysfs file. Pass a
//!   [`CStr`](core::ffi::CStr): `led_set_trigger(1,
//!   c"heartbeat".as_ptr())`, and — because `GPIOcontrol.h` declares
//!   the other one `char *` where it means `const char *` —
//!   `gpio_set_edge(pin, c"rising".as_ptr().cast_mut())`. Neither
//!   writes through the pointer.
//! - **`led_set_trigger`'s `lednum` starts at 1 on a Gem.** It builds
//!   `/sys/class/leds/beaglebone:green:usr%d/trigger`, a path written
//!   for a `BeagleBone`, whose user LEDs are `usr0` to `usr3`. This
//!   board has `usr1` to `usr4`, so `0` — the obvious first guess, and
//!   the right one on the hardware the path names — reaches no file
//!   and comes back `-1` with a `perror`. Measured;
//!   `docs/board-facts.md` has the inventory.
//!
//! Nor does a failure always announce itself. `gpio_setup` prints to
//! stdout, and every function that cannot open its sysfs file calls
//! `perror` — but once a file is open a failed `read` or `write` only
//! becomes a `-1`, and `gpio_read` and `gpio_write`, which are handed
//! a descriptor rather than opening one, never print at all. And
//! `gpio_unexport`'s `perror` is labelled `gpio/export`, its
//! neighbour's label, so a pin that would not go away reports itself
//! as one that would not arrive.
//!
//! Two things here are neither the core API nor generated, and they
//! are two different kinds of thing:
//!
//! - The [`bela_midi_*`](bela_midi_new) functions are a C surface this
//!   crate compiles (`shim/midi.cpp`) over Bela's `Midi` class in
//!   `libbelaextra`. MIDI is what a Bela program reaches for first
//!   after audio, and the class is C++ with only half a C surface of
//!   its own.
//! - The [`ne10_fft_*`](ne10_fft_alloc_r2c_float32) functions are
//!   plain C on the board, in `libNE10.so.10`, declared here by hand.
//!   They are the real-to-complex FFT, reached directly rather than
//!   through Bela's `Fft` class, which wraps these same calls and
//!   little else; `docs/fft.md` in the repository records why. Nothing
//!   is compiled for them — the library is already on the board — but
//!   `abi/ne10_abi.c` asserts at build time that its headers still
//!   describe what is declared here.
//!
//! Bela's own higher-level C++ libraries (Scope, Trill, Fft, Gui)
//! remain out of scope.
//!
//! The `setup` / `render` / `cleanup` callbacks are not bound: they are
//! either provided to `Bela_initAudio` via [`BelaInitSettings`] or
//! defined as `#[unsafe(no_mangle)]` symbols by the linking crate.
//!
//! Target platform is Bela Gem on `PocketBeagle` 2
//! (`aarch64-unknown-linux-gnu`). For a safe API, use the `bela`
//! crate instead.
//!
//! [Bela Gem]: https://bela.io
#![no_std]

#[allow(
    missing_docs,
    nonstandard_style,
    unsafe_op_in_unsafe_fn,
    unused,
    clippy::all,
    clippy::pedantic,
    clippy::nursery,
    clippy::restriction,
    rustdoc::all,
    reason = "generated by bindgen; regenerate with `cargo xtask bindgen`"
)]
mod bindings;
mod midi;
mod ne10;

pub use bindings::*;
pub use midi::{
    BELA_MIDI_ALREADY_OPEN, BELA_MIDI_MESSAGE_MAX, BELA_MIDI_NO_SUCH_PORT, BelaMidi,
    bela_midi_available_messages, bela_midi_delete, bela_midi_get_message, bela_midi_list_ports,
    bela_midi_new, bela_midi_read_from, bela_midi_write_output, bela_midi_write_to,
};
pub use ne10::{
    ne10_fft_alloc_r2c_float32, ne10_fft_c2r_1d_float32_neon, ne10_fft_cpx_float32_t,
    ne10_fft_destroy_r2c_float32, ne10_fft_r2c_1d_float32_neon, ne10_fft_r2c_state_float32_t,
};

// The build script's toolchain logic, tested where a build script
// cannot be: `cargo test` builds this crate, not `build.rs`. See
// ../shim_compiler.rs.
#[cfg(test)]
mod shim_compiler {
    extern crate std;

    use std::borrow::ToOwned;
    use std::format;
    use std::string::String;

    include!("../shim_compiler.rs");

    #[test]
    fn bela_cxx_is_taken_as_it_stands() {
        assert_eq!(
            shim_compiler_from(
                "clang++",
                "aarch64-unknown-linux-gnu-gcc",
                "aarch64-linux-gnu-gcc"
            ),
            Ok("clang++".to_owned()),
            "an explicit C++ compiler outranks anything derived, even a resolved linker"
        );
    }

    #[test]
    fn a_c_compiler_ending_in_gcc_answers_for_both() {
        // The two cases docs/cross-compile.md documents, driven
        // through the legacy BELA_CC path (no linker resolved).
        assert_eq!(
            shim_compiler_from("", "", "aarch64-linux-gnu-gcc"),
            Ok("aarch64-linux-gnu-g++".to_owned())
        );
        assert_eq!(shim_compiler_from("", "", "gcc"), Ok("g++".to_owned()));
    }

    #[test]
    fn neither_set_is_the_tap_default() {
        assert_eq!(
            shim_compiler_from("", "", ""),
            Ok(DEFAULT_CXX.to_owned()),
            "the same default scripts/aarch64-bela-linker.sh has"
        );
    }

    #[test]
    fn a_c_compiler_nothing_follows_from_is_refused() {
        // Deriving `ar`, or a C++ name, from this would mix
        // toolchains silently, which the build script fails on
        // instead.
        let error = shim_compiler_from("", "", "clang").unwrap_err();
        assert!(
            error.contains("BELA_CXX"),
            "the message should say what to set, got: {error}"
        );
    }

    #[test]
    fn a_resolved_gcc_linker_answers_for_the_shim_too() {
        // The direct-linker path (docs/cross-compile.md): Cargo
        // resolved a compiler driver directly, so no BELA_CC is
        // needed at all.
        assert_eq!(
            shim_compiler_from("", "aarch64-unknown-linux-gnu-gcc", ""),
            Ok("aarch64-unknown-linux-gnu-g++".to_owned())
        );
        assert_eq!(shim_compiler_from("", "gcc", ""), Ok("g++".to_owned()));
    }

    #[test]
    fn a_resolved_linker_outranks_a_stale_bela_cc() {
        // RUSTC_LINKER reflects the toolchain that will actually link
        // the binary; a leftover BELA_CC from before migrating off the
        // wrapper must not silently win and build the shim with a
        // different one.
        assert_eq!(
            shim_compiler_from("", "aarch64-unknown-linux-gnu-gcc", "gcc"),
            Ok("aarch64-unknown-linux-gnu-g++".to_owned())
        );
    }

    #[test]
    fn the_wrapper_as_the_resolved_linker_falls_through_to_bela_cc() {
        // .cargo/config.toml still names the wrapper: RUSTC_LINKER is
        // set, but to something that names no C++ compiler on its own,
        // so BELA_CC answers as it always has.
        assert_eq!(
            shim_compiler_from("", "scripts/aarch64-bela-linker.sh", "gcc"),
            Ok("g++".to_owned())
        );
        assert_eq!(
            shim_compiler_from(
                "",
                "/Users/dev/bela-rs/scripts/aarch64-bela-linker.sh",
                "aarch64-linux-gnu-gcc"
            ),
            Ok("aarch64-linux-gnu-g++".to_owned()),
            "an absolute path still matches by its last segment"
        );
        assert_eq!(
            shim_compiler_from("", "scripts/aarch64-bela-linker.sh", ""),
            Ok(DEFAULT_CXX.to_owned()),
            "and with BELA_CC unset too, the tap default"
        );
    }

    #[test]
    fn the_abi_check_takes_the_c_compiler_as_it_stands() {
        // BELA_CC already names a C compiler, and the assertions in
        // abi/ne10_abi.c are C: nothing to derive.
        assert_eq!(
            abi_compiler_from("aarch64-linux-gnu-gcc", "", "clang++"),
            Some("aarch64-linux-gnu-gcc".to_owned())
        );
    }

    #[test]
    fn the_abi_check_follows_the_resolved_linker() {
        // The direct-linker path: what Cargo resolved is a compiler
        // driver, which is what should read the headers.
        assert_eq!(
            abi_compiler_from("", "aarch64-unknown-linux-gnu-gcc", ""),
            Some("aarch64-unknown-linux-gnu-gcc".to_owned())
        );
        assert_eq!(
            abi_compiler_from("gcc", "scripts/aarch64-bela-linker.sh", ""),
            Some("gcc".to_owned()),
            "the wrapper names no compiler of its own, so BELA_CC answers"
        );
    }

    #[test]
    fn the_abi_check_derives_a_c_compiler_from_a_cxx_one() {
        // Only BELA_CXX is set, which is the shim's variable: the C
        // compiler beside it reads the same headers with the same
        // defines.
        assert_eq!(
            abi_compiler_from("", "", "aarch64-linux-gnu-g++"),
            Some("aarch64-linux-gnu-gcc".to_owned())
        );
        assert_eq!(
            abi_compiler_from("", "", "clang++"),
            Some("clang".to_owned())
        );
        assert_eq!(
            abi_compiler_from("", "", ""),
            Some("aarch64-unknown-linux-gnu-gcc".to_owned()),
            "with nothing set, the tap's C compiler beside DEFAULT_CXX"
        );
    }

    #[test]
    fn a_cxx_compiler_nothing_follows_from_skips_the_abi_check() {
        // No error: the check is a guard against a board image moving
        // NE10, and a build that cannot run it links exactly as it did
        // before the check existed. build.rs warns instead.
        assert_eq!(abi_compiler_from("", "", "my-cross-compiler"), None);
    }

    #[test]
    fn the_abi_archiver_follows_a_gcc_name() {
        assert_eq!(
            abi_archiver("aarch64-linux-gnu-gcc"),
            Some("aarch64-linux-gnu-ar".to_owned())
        );
        assert_eq!(abi_archiver("gcc"), Some("ar".to_owned()));
        assert_eq!(
            abi_archiver("clang"),
            None,
            "clang wants llvm-ar, not an ar beside it; cc resolves that"
        );
    }

    #[test]
    fn a_resolved_linker_nothing_follows_from_is_refused() {
        // A directly configured non-GNU linker (clang, lld, mold, ...)
        // names no C++ compiler to derive, and BELA_CC is the legacy
        // path's variable, not this one's — guessing here would risk
        // the same toolchain mismatch BELA_CC guards against.
        let error = shim_compiler_from("", "clang", "").unwrap_err();
        assert!(
            error.contains("BELA_CXX"),
            "the message should say what to set, got: {error}"
        );
    }

    #[test]
    fn the_archiver_follows_the_compiler_it_belongs_to() {
        assert_eq!(
            shim_archiver("aarch64-unknown-linux-gnu-g++"),
            Some("aarch64-unknown-linux-gnu-ar".to_owned()),
            "cc would otherwise look for one named after the target triple"
        );
        assert_eq!(shim_archiver("g++"), Some("ar".to_owned()));
    }

    #[test]
    fn an_archiver_that_does_not_follow_is_left_to_cc() {
        // `clang++` wants llvm-ar, and it also ends in the letters
        // `g++`: deriving from it would name `clanar`. AR is the way
        // out for those, and cc reads it.
        assert_eq!(shim_archiver("clang++"), None);
        assert_eq!(shim_archiver("aarch64-linux-gnu-clang++"), None);
    }

    #[test]
    fn a_compiler_named_by_its_path_is_still_one() {
        // docs/cross-compile.md allows an absolute path, and
        // /usr/bin/gcc is the board's own compiler.
        assert_eq!(
            shim_compiler_from("", "", "/usr/bin/gcc"),
            Ok("/usr/bin/g++".to_owned())
        );
        assert_eq!(
            shim_compiler_from("", "", "/opt/tc/bin/aarch64-linux-gnu-gcc"),
            Ok("/opt/tc/bin/aarch64-linux-gnu-g++".to_owned())
        );
        assert_eq!(
            shim_archiver("/usr/bin/g++"),
            Some("/usr/bin/ar".to_owned()),
            "and the archiver beside it"
        );
    }

    #[test]
    fn a_compiler_that_merely_ends_in_gcc_is_not_one() {
        // Same trap on the compiler side: only a bare `gcc` or a
        // `<triple>-gcc` names a toolchain to follow.
        assert!(shim_compiler_from("", "", "notgcc").is_err());
        assert_eq!(shim_archiver("notg++"), None);
        assert_eq!(
            shim_archiver("/usr/bin/clang++"),
            None,
            "a path does not make it one"
        );
    }
}

// The metadata encoding build.rs publishes so `bela` can relay device
// link arguments to its own dependents; tested here for the same
// reason as shim_compiler above. See link_args.rs and bela/link_args.rs.
#[cfg(test)]
mod link_args {
    extern crate std;

    use std::borrow::ToOwned;
    use std::format;
    use std::string::{String, ToString};
    use std::vec;
    use std::vec::Vec;

    include!("../link_args.rs");

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn no_arguments_still_publishes_a_zero_count() {
        assert_eq!(
            encode_link_args(&[]),
            vec![("LINK_ARGS_COUNT".to_owned(), "0".to_owned())],
            "a dependent has to see a count of zero, not an absent key, \
             to tell \"nothing to add\" apart from \"never ran\""
        );
    }

    #[test]
    fn arguments_are_indexed_from_zero_in_order() {
        assert_eq!(
            encode_link_args(&args(&["--sysroot=/opt/bela", "-Bfoo"])),
            vec![
                ("LINK_ARGS_COUNT".to_owned(), "2".to_owned()),
                ("LINK_ARGS_0".to_owned(), "--sysroot=/opt/bela".to_owned()),
                ("LINK_ARGS_1".to_owned(), "-Bfoo".to_owned()),
            ]
        );
    }

    #[test]
    fn whitespace_in_an_argument_survives_uninterpreted() {
        // The reason for a count-plus-index encoding over one joined
        // string: a BELA_SYSROOT with a space in it must not need a
        // shell-style parser on the reading side.
        let value = "--sysroot=/Volumes/Bela Sysroot";
        assert_eq!(
            encode_link_args(&args(&[value])),
            vec![
                ("LINK_ARGS_COUNT".to_owned(), "1".to_owned()),
                ("LINK_ARGS_0".to_owned(), value.to_owned()),
            ]
        );
    }
}

// The shim's header and the declarations mirroring it are edited by
// hand, and a value that drifts between them is not a compile error —
// it is a safe API that reads one failure as another. This is the
// cheap half of the guard: the numbers.
#[cfg(test)]
mod shim_header {
    extern crate std;

    use std::format;

    /// `shim/midi.h`, read at compile time from the crate this
    /// declares the shim for.
    const HEADER: &str = include_str!("../shim/midi.h");

    /// The value of `#define <name> ...`, with one level of
    /// parentheses taken off — the header writes negative constants as
    /// `(-1000)`, as a C header should.
    fn defined(name: &str) -> i64 {
        let line = HEADER
            .lines()
            .find(|line| line.starts_with(&format!("#define {name} ")))
            .unwrap_or_else(|| panic!("{name} is not defined in shim/midi.h"));
        let value = line.split_whitespace().nth(2).expect("a value");
        value
            .trim_start_matches('(')
            .trim_end_matches(')')
            .parse()
            .expect("a number")
    }

    #[test]
    fn the_constants_match_the_header() {
        assert_eq!(
            defined("BELA_MIDI_MESSAGE_MAX"),
            i64::try_from(super::BELA_MIDI_MESSAGE_MAX).expect("a small buffer size")
        );
        assert_eq!(
            defined("BELA_MIDI_NO_SUCH_PORT"),
            i64::from(super::BELA_MIDI_NO_SUCH_PORT)
        );
        assert_eq!(
            defined("BELA_MIDI_ALREADY_OPEN"),
            i64::from(super::BELA_MIDI_ALREADY_OPEN)
        );
    }
}
