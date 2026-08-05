//! The command line: which task `cargo xtask` was asked to run, with
//! its arguments resolved.
//!
//! Deciding is separated from doing. What the arguments mean is a
//! question about the arguments alone, so `Task::parse` answers it
//! without a board, without a sysroot, and without regenerating
//! anything — leaving `main` to act on the answer.

use std::path::PathBuf;
use std::process;

use crate::check_vendor;

/// A task named on the command line.
#[derive(Debug, PartialEq, Eq)]
pub enum Task {
    /// Regenerate `bela-sys/src/bindings.rs` from the vendored headers.
    Bindgen {
        /// Where bindgen takes the aarch64 libc headers from, if it was
        /// told. Unset means neither `--sysroot` nor `BELA_SYSROOT`
        /// named one, which bindgen is left to complain about.
        sysroot: Option<PathBuf>,
    },
    /// Compare the vendored headers with the ones on a board.
    CheckVendor {
        /// The board to compare against.
        host: String,
    },
}

/// The command line named no task this crate runs, or named one with
/// arguments it cannot make sense of. There is nothing to distinguish:
/// every one of them is answered with the usage text.
#[derive(Debug, PartialEq, Eq)]
pub struct UsageError;

impl Task {
    /// The task `args` — the arguments after the program name — asks
    /// for.
    ///
    /// `sysroot` is the default for `bindgen`, which the caller reads
    /// from `BELA_SYSROOT` and `--sysroot` overrides. It is passed in
    /// rather than read here so that what this returns depends on
    /// nothing but its arguments.
    pub fn parse(
        args: impl IntoIterator<Item = String>,
        sysroot: Option<PathBuf>,
    ) -> Result<Self, UsageError> {
        let mut args = args.into_iter();
        match args.next().as_deref() {
            Some("bindgen") => Self::parse_bindgen(args, sysroot),
            Some("check-vendor") => Self::parse_check_vendor(args),
            _ => Err(UsageError),
        }
    }

    /// `bindgen [--sysroot <dir>]`. A repeated `--sysroot` keeps the
    /// last one, the way a shell user overriding an earlier argument
    /// would expect.
    fn parse_bindgen(
        mut args: impl Iterator<Item = String>,
        mut sysroot: Option<PathBuf>,
    ) -> Result<Self, UsageError> {
        while let Some(arg) = args.next() {
            if arg == "--sysroot" {
                sysroot = Some(PathBuf::from(args.next().ok_or(UsageError)?));
            } else {
                return Err(UsageError);
            }
        }
        Ok(Self::Bindgen { sysroot })
    }

    /// `check-vendor --board [user@host]`. A board is the only source
    /// worth checking against — the headers are pinned to one, not to
    /// an upstream ref — so `--board` is required and reads as the
    /// choice of source, with the host it may be followed by naming
    /// which board.
    fn parse_check_vendor(args: impl Iterator<Item = String>) -> Result<Self, UsageError> {
        let mut board = false;
        let mut host = None;
        for arg in args {
            match arg.as_str() {
                "--board" if !board => board = true,
                _ if board && host.is_none() && !arg.starts_with('-') => host = Some(arg),
                _ => return Err(UsageError),
            }
        }
        if !board {
            return Err(UsageError);
        }
        Ok(Self::CheckVendor {
            host: host.unwrap_or_else(|| check_vendor::DEFAULT_HOST.to_owned()),
        })
    }
}

/// Print what the tasks are and exit non-zero.
#[allow(
    clippy::exit,
    reason = "a usage error is reported to the caller as exit code 2"
)]
pub fn usage() -> ! {
    eprintln!("usage:");
    eprintln!("  cargo xtask bindgen [--sysroot <dir>]");
    eprintln!("      Regenerate bela-sys/src/bindings.rs from the vendored headers.");
    eprintln!("      The sysroot must provide aarch64-linux libc headers (defaults to");
    eprintln!("      the BELA_SYSROOT environment variable). See bela-sys/README.md.");
    eprintln!();
    eprintln!("  cargo xtask check-vendor --board [user@host]");
    eprintln!("      Compare the vendored headers with the ones on a board");
    eprintln!(
        "      (default {}). Exits non-zero on drift.",
        check_vendor::DEFAULT_HOST
    );
    process::exit(2);
}
