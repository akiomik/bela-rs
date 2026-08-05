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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Task, UsageError> {
        Task::parse(args.iter().map(|arg| (*arg).to_owned()), None)
    }

    fn parse_with_sysroot(args: &[&str], sysroot: &str) -> Result<Task, UsageError> {
        Task::parse(
            args.iter().map(|arg| (*arg).to_owned()),
            Some(PathBuf::from(sysroot)),
        )
    }

    fn bindgen(sysroot: Option<&str>) -> Task {
        Task::Bindgen {
            sysroot: sysroot.map(PathBuf::from),
        }
    }

    fn compare_against(host: &str) -> Task {
        Task::CheckVendor {
            host: host.to_owned(),
        }
    }

    #[test]
    fn a_line_that_names_no_task_is_a_usage_error() {
        for args in [
            vec![],
            vec![""],
            vec!["bogus"],
            vec!["--sysroot", "/opt/bela"],
            // The task has to come first: these read as one, but only
            // after an argument that was never asked for.
            vec!["--board", "check-vendor"],
            vec!["cargo", "xtask", "bindgen"],
        ] {
            assert_eq!(parse(&args), Err(UsageError), "{args:?}");
        }
    }

    #[test]
    fn bindgen_falls_back_to_the_sysroot_it_was_given() {
        assert_eq!(parse(&["bindgen"]), Ok(bindgen(None)));
        assert_eq!(
            parse_with_sysroot(&["bindgen"], "/opt/bela"),
            Ok(bindgen(Some("/opt/bela")))
        );
    }

    /// `BELA_SYSROOT` is a standing setting and `--sysroot` is the
    /// argument overriding it for one run, so the argument wins.
    #[test]
    fn bindgen_prefers_the_sysroot_argument() {
        assert_eq!(
            parse_with_sysroot(&["bindgen", "--sysroot", "/tmp/other"], "/opt/bela"),
            Ok(bindgen(Some("/tmp/other")))
        );
    }

    #[test]
    fn the_last_sysroot_argument_wins() {
        assert_eq!(
            parse(&[
                "bindgen",
                "--sysroot",
                "/tmp/first",
                "--sysroot",
                "/tmp/last"
            ]),
            Ok(bindgen(Some("/tmp/last")))
        );
    }

    /// A `--sysroot` with nothing after it asked for a sysroot and
    /// named none. Falling back to `BELA_SYSROOT` would regenerate the
    /// bindings from something other than what was asked for.
    #[test]
    fn a_sysroot_argument_without_a_directory_is_a_usage_error() {
        assert_eq!(
            parse_with_sysroot(&["bindgen", "--sysroot"], "/opt/bela"),
            Err(UsageError)
        );
    }

    #[test]
    fn bindgen_takes_no_other_arguments() {
        for args in [
            vec!["bindgen", "--board"],
            vec!["bindgen", "/opt/bela"],
            vec!["bindgen", "--sysroot", "/opt/bela", "extra"],
        ] {
            assert_eq!(parse(&args), Err(UsageError), "{args:?}");
        }
    }

    #[test]
    fn check_vendor_defaults_to_the_usual_board() {
        assert_eq!(
            parse(&["check-vendor", "--board"]),
            Ok(compare_against(check_vendor::DEFAULT_HOST))
        );
    }

    #[test]
    fn check_vendor_takes_the_board_that_follows_the_flag() {
        assert_eq!(
            parse(&["check-vendor", "--board", "root@bela-gem.local"]),
            Ok(compare_against("root@bela-gem.local"))
        );
    }

    /// `--board` says a board is the source, which is the only source
    /// this compares against. Without it there is nothing to compare
    /// with, and defaulting to one would make a plain `check-vendor`
    /// reach for the network unasked.
    #[test]
    fn check_vendor_needs_the_board_flag() {
        for args in [
            vec!["check-vendor"],
            vec!["check-vendor", "root@bela.local"],
            // The host reads as the board's name, so it follows the
            // flag rather than standing in for it.
            vec!["check-vendor", "root@bela.local", "--board"],
        ] {
            assert_eq!(parse(&args), Err(UsageError), "{args:?}");
        }
    }

    #[test]
    fn check_vendor_compares_against_one_board() {
        for args in [
            vec!["check-vendor", "--board", "--board"],
            vec!["check-vendor", "--board", "one.local", "--board"],
            vec!["check-vendor", "--board", "one.local", "two.local"],
        ] {
            assert_eq!(parse(&args), Err(UsageError), "{args:?}");
        }
    }

    /// A host is a host, but an unknown flag is a mistake worth
    /// reporting rather than a board named `--sysroot`.
    #[test]
    fn a_flag_is_never_taken_for_a_board() {
        assert_eq!(
            parse(&["check-vendor", "--board", "--sysroot"]),
            Err(UsageError)
        );
    }
}
