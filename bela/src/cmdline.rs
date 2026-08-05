//! Bela's standard command-line options.
//!
//! Every other way of writing a Bela program accepts the same set of
//! options — `--period`, `--verbose`, `--use-analog` and the rest —
//! parsed by `Bela_getopt_long`, so a binary can be reconfigured
//! without rebuilding it.
//! [`Bela::new_with_args`](crate::Bela::new_with_args) and
//! [`Bela::run_with_args`](crate::Bela::run_with_args) apply them here,
//! and [`print_usage`] prints the list.
//!
//! # What ends up in the settings
//!
//! Three layers, each on top of the previous one:
//!
//! 1. `Bela_defaultSettings()`, which is more than a table of
//!    constants: it also applies the `CL=` line from the board's
//!    `~/.bela/belaconfig` and calls the weakly linked
//!    `Bela_userSettings()` hook, if the program defines one.
//! 2. [`Settings`](crate::Settings) — the defaults the application was
//!    built with.
//! 3. the command line, which therefore wins. That is the point of it:
//!    a program keeps sensible defaults of its own and stays
//!    configurable from outside.
//!
//! # Options of the program's own
//!
//! `Bela_getopt_long` can be given extra options to parse, returning
//! the ones it does not recognise the way `getopt_long` would. This
//! crate passes none, and a program that has options of its own parses
//! them itself — with whatever argument parser it already uses — and
//! hands on what is left. Nothing about `getopt`'s globals, its argv
//! permutation or its `optarg` then has to appear in a safe API, and
//! anything Bela does not recognise is an error rather than something
//! quietly ignored.

use core::ffi::{c_char, c_int};
use std::ffi::{CString, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::ptr;

use crate::error::Error;

/// An argument list in the layout the C side needs.
///
/// `Bela_getopt_long` takes `argc` and `argv` as a C `main` receives
/// them: NUL-terminated strings, addressed through an array of
/// pointers that outlives the call. `getopt` also reorders that array
/// as it goes — non-options are moved to the end — so the array is
/// owned here rather than borrowed from the caller.
#[cfg_attr(
    not(bela_device),
    allow(
        dead_code,
        reason = "only the device-gated audio system parses arguments; still unit-tested on the host"
    )
)]
pub struct Arguments {
    /// Owns the argument bytes. Never mutated once `argv` points into
    /// it: each `CString` keeps its own allocation, so the pointers
    /// stay valid as long as this vector is neither changed nor
    /// dropped.
    storage: Vec<CString>,
    /// One pointer per argument, plus the terminating null pointer a C
    /// `main` would have.
    argv: Vec<*mut c_char>,
}

#[cfg_attr(
    not(bela_device),
    allow(
        dead_code,
        reason = "only the device-gated audio system parses arguments; still unit-tested on the host"
    )
)]
impl Arguments {
    /// Copies `args` into the C layout.
    ///
    /// The first argument is the program name, as
    /// [`std::env::args_os()`] yields it; `getopt` starts at the second
    /// one.
    ///
    /// # Errors
    /// Returns [`Error::CommandLineNul`] when an argument contains a
    /// NUL byte, which a C string cannot carry. Arguments a process was
    /// started with never do — the kernel hands them over
    /// NUL-terminated — so this only concerns hand-built lists.
    pub fn new<I, S>(args: I) -> Result<Self, Error>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let storage = args
            .into_iter()
            .map(|arg| CString::new(arg.as_ref().as_bytes()).map_err(|_| Error::CommandLineNul))
            .collect::<Result<Vec<_>, Error>>()?;
        let mut argv: Vec<*mut c_char> = storage
            .iter()
            // The C side reads the strings and reorders the pointers to
            // them; neither `Bela_getopt_long` nor `getopt` writes
            // through one.
            .map(|arg| arg.as_ptr().cast_mut())
            .collect();
        argv.push(ptr::null_mut());
        Ok(Self { storage, argv })
    }

    /// The number of arguments, excluding the null terminator.
    fn argc(&self) -> c_int {
        // An argument list that long cannot be built: the kernel caps
        // it far below this. Saturate rather than wrap if one ever is.
        c_int::try_from(self.storage.len()).unwrap_or(c_int::MAX)
    }

    /// The pointer array, for the length [`argc`](Arguments::argc)
    /// reports.
    ///
    /// Takes `&mut self` because the call reorders the array.
    fn as_argv(&mut self) -> *const *mut c_char {
        self.argv.as_mut_ptr()
    }
}

/// Applies Bela's standard options from `arguments` on top of `raw`.
///
/// # Errors
/// Returns [`Error::CommandLine`] when an argument is not one of the
/// standard options, is missing its value, or is rejected by libbela.
#[cfg(bela_device)]
pub fn parse(arguments: &mut Arguments, raw: &mut bela_sys::BelaInitSettings) -> Result<(), Error> {
    // `Bela_getopt_long` handles the standard options itself and
    // returns only what is left over, exactly as `getopt_long` would.
    // With no options of our own added, the only thing it can return
    // that is not an error is the end of the argument list.
    //
    // Its cursor is `getopt`'s process-wide `optind`, and it starts
    // where `Bela_defaultSettings` left it: that function runs the
    // board's configured `CL=` options through this same parser and
    // resets the cursor afterwards, which is what makes a program's own
    // parse begin at the first argument. `Bela::new` calls it every
    // time, so a second audio system in the same process starts from
    // the front too.
    //
    // Passing an empty string for the custom short options and a null
    // pointer for the custom long ones is how libbela itself calls this
    // when it has none to add.
    let ret = unsafe {
        bela_sys::Bela_getopt_long(
            arguments.argc(),
            arguments.as_argv(),
            c"".as_ptr(),
            ptr::null(),
            raw,
        )
    };
    if ret < 0 {
        Ok(())
    } else {
        Err(Error::CommandLine(ret))
    }
}

/// Prints Bela's standard options to standard error.
///
/// This is the usage text every Bela program shares; a program with
/// options of its own prints them around it.
///
/// Only available on the device target (`aarch64-unknown-linux-gnu`).
#[cfg(bela_device)]
pub fn print_usage() {
    unsafe { bela_sys::Bela_usage() }
}

#[cfg(test)]
mod tests {
    use core::ffi::CStr;

    use super::*;

    /// Reads the arguments back the way the C side sees them.
    fn as_c_sees_them(arguments: &Arguments) -> Vec<Vec<u8>> {
        arguments
            .argv
            .iter()
            .take_while(|arg| !arg.is_null())
            // Safety: every non-null pointer in the array points at one
            // of the NUL-terminated strings `storage` still owns.
            .map(|arg| unsafe { CStr::from_ptr(*arg) }.to_bytes().to_vec())
            .collect()
    }

    #[test]
    fn arguments_are_copied_in_order() {
        let mut arguments =
            Arguments::new(["my-app", "--period", "64", "-v"]).expect("no NUL bytes");

        assert_eq!(arguments.argc(), 4);
        assert_eq!(
            as_c_sees_them(&arguments),
            [
                b"my-app".to_vec(),
                b"--period".to_vec(),
                b"64".to_vec(),
                b"-v".to_vec()
            ]
        );
        assert!(
            !arguments.as_argv().is_null(),
            "the array has to be addressable"
        );
    }

    #[test]
    fn the_pointer_array_is_null_terminated() {
        let arguments = Arguments::new(["my-app", "-v"]).expect("no NUL bytes");

        assert_eq!(
            arguments.argv.len(),
            3,
            "one pointer per argument plus the terminator a C main has"
        );
        assert!(
            arguments.argv.last().is_some_and(|last| last.is_null()),
            "the array must end the way a C argv does"
        );
    }

    #[test]
    fn arguments_that_are_not_utf8_survive() {
        // A file name does not have to be UTF-8, and `--json-file`
        // takes one.
        let path = OsStr::from_bytes(b"settings-\xff.json");
        let arguments = Arguments::new([OsStr::new("my-app"), OsStr::new("--json-file"), path])
            .expect("no NUL bytes");

        assert_eq!(
            as_c_sees_them(&arguments)[2],
            b"settings-\xff.json".to_vec(),
            "the bytes have to reach C unchanged"
        );
    }

    #[test]
    fn an_argument_containing_a_nul_is_refused() {
        assert_eq!(
            Arguments::new(["my-app", "--period\0 64"]).err(),
            Some(Error::CommandLineNul),
            "a C string cannot carry it, and truncating would change what was asked for"
        );
    }

    #[test]
    fn an_empty_list_is_empty() {
        let arguments = Arguments::new(Vec::<String>::new()).expect("no NUL bytes");

        assert_eq!(arguments.argc(), 0);
        assert_eq!(as_c_sees_them(&arguments), Vec::<Vec<u8>>::new());
    }
}
