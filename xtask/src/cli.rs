//! The command line: which task `cargo xtask` was asked to run, with
//! its arguments resolved.
//!
//! Declared rather than parsed by hand, so that the grammar and the
//! `--help` describing it are the same thing. They used to be two: a
//! parser, and a block of `eprintln!` claiming what the parser did.
//! Nothing tied them together, and only the parser was tested.
//!
//! Deciding stays separate from doing. `Cli::try_parse_from` answers
//! what the arguments mean without a board, without a sysroot and
//! without regenerating anything, leaving `main` to act on the answer.
//! `BELA_SYSROOT` is applied there rather than here (clap can read the
//! environment itself) so that what parsing returns depends on nothing
//! but the arguments it was given.

// The doc comments below are not rustdoc prose: clap renders them as
// the `--help` text, where backticks would be shown to the reader as
// backticks.
#![allow(
    clippy::doc_markdown,
    reason = "doc comments on the grammar are the help output"
)]

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::check_vendor;

/// Repository tasks.
#[derive(Parser, Debug, PartialEq, Eq)]
#[command(bin_name = "cargo xtask", about, long_about = None)]
pub struct Cli {
    /// The task to run.
    #[command(subcommand)]
    pub task: Task,
}

/// A task named on the command line.
#[derive(Subcommand, Debug, PartialEq, Eq)]
pub enum Task {
    /// Regenerate bela-sys/src/bindings.rs from the vendored headers.
    Bindgen {
        /// Sysroot providing the aarch64-linux libc headers Bela.h
        /// includes. Defaults to $BELA_SYSROOT. See bela-sys/README.md.
        // Overriding itself is how clap spells "the last one wins",
        // which is what a shell user repeating the argument means by
        // it; without this clap rejects the repetition as a conflict.
        #[arg(long, value_name = "DIR", overrides_with = "sysroot")]
        sysroot: Option<PathBuf>,
    },

    /// Compare the vendored headers with the ones on a board.
    ///
    /// Exits non-zero on drift. A board is the only source worth
    /// checking against: the headers are pinned to one, not to an
    /// upstream ref, which is why naming it is not optional.
    CheckVendor {
        /// Board to compare against. Defaults to root@bela.local.
        //
        // The flag is required because the field is not an `Option`;
        // `num_args` and `default_missing_value` are what make the host
        // after it optional, not the flag itself. clap leaves
        // `default_missing_value` out of the help, so the default is
        // spelled out above by hand and a test holds the two together.
        #[arg(
            long,
            num_args = 0..=1,
            default_missing_value = check_vendor::DEFAULT_HOST,
            value_name = "USER@HOST",
        )]
        board: String,
    },
}

#[cfg(test)]
mod tests {
    use std::iter;

    use clap::CommandFactory;
    use clap::error::ErrorKind;

    use super::*;

    fn parse(args: &[&str]) -> Result<Task, ErrorKind> {
        Cli::try_parse_from(iter::once("xtask").chain(args.iter().copied()))
            .map(|cli| cli.task)
            .map_err(|error| error.kind())
    }

    /// Why a line was rejected, for the tests that only care about that.
    fn rejection(args: &[&str]) -> ErrorKind {
        parse(args).expect_err("expected a usage error")
    }

    fn bindgen(sysroot: Option<&str>) -> Task {
        Task::Bindgen {
            sysroot: sysroot.map(PathBuf::from),
        }
    }

    fn compare_against(board: &str) -> Task {
        Task::CheckVendor {
            board: board.to_owned(),
        }
    }

    /// clap checks its own definition: colliding names, defaults that
    /// cannot be parsed, and the like would otherwise only show up as
    /// a panic on the first run.
    #[test]
    fn the_declared_grammar_is_well_formed() {
        Cli::command().debug_assert();
    }

    /// Naming no task at all is answered with the help rather than an
    /// error about it, which is what someone running `cargo xtask` to
    /// see what it offers is after.
    #[test]
    fn a_line_that_names_no_task_shows_the_help() {
        assert_eq!(
            rejection(&[]),
            ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        );
    }

    #[test]
    fn a_line_that_names_something_else_is_rejected() {
        assert_eq!(rejection(&["bogus"]), ErrorKind::InvalidSubcommand);
        // The task comes first: these name one, but only after an
        // argument that was never asked for.
        assert_eq!(
            rejection(&["--sysroot", "/opt/bela"]),
            ErrorKind::UnknownArgument
        );
        assert_eq!(
            rejection(&["--board", "check-vendor"]),
            ErrorKind::UnknownArgument
        );
    }

    #[test]
    fn help_is_answered_rather_than_rejected() {
        for args in [
            vec!["--help"],
            vec!["-h"],
            vec!["bindgen", "--help"],
            vec!["check-vendor", "--help"],
        ] {
            assert_eq!(rejection(&args), ErrorKind::DisplayHelp, "{args:?}");
        }
    }

    /// The help is generated from the same declaration the parsing
    /// comes from, so it cannot describe a grammar that is not there.
    #[test]
    fn the_help_names_both_tasks_and_their_arguments() {
        let mut command = Cli::command();
        let help = command.render_long_help().to_string();
        for expected in ["bindgen", "check-vendor"] {
            assert!(help.contains(expected), "{expected} missing from:\n{help}");
        }

        let board = command
            .find_subcommand_mut("check-vendor")
            .expect("check-vendor is declared")
            .render_long_help()
            .to_string();
        assert!(
            board.contains(check_vendor::DEFAULT_HOST),
            "the default board is not in:\n{board}"
        );
    }

    #[test]
    fn bindgen_takes_a_sysroot_or_none() {
        assert_eq!(parse(&["bindgen"]), Ok(bindgen(None)));
        assert_eq!(
            parse(&["bindgen", "--sysroot", "/opt/bela"]),
            Ok(bindgen(Some("/opt/bela")))
        );
    }

    /// `--sysroot=<dir>` is the form the hand-rolled parser rejected.
    #[test]
    fn a_sysroot_may_be_attached_to_the_flag() {
        assert_eq!(
            parse(&["bindgen", "--sysroot=/opt/bela"]),
            Ok(bindgen(Some("/opt/bela")))
        );
    }

    #[test]
    fn the_last_sysroot_wins() {
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
    fn a_sysroot_without_a_directory_is_rejected() {
        assert_eq!(
            rejection(&["bindgen", "--sysroot"]),
            ErrorKind::InvalidValue
        );
    }

    #[test]
    fn bindgen_takes_no_other_arguments() {
        assert_eq!(
            rejection(&["bindgen", "--board"]),
            ErrorKind::UnknownArgument
        );
        assert_eq!(
            rejection(&["bindgen", "/opt/bela"]),
            ErrorKind::UnknownArgument
        );
        assert_eq!(
            rejection(&["bindgen", "--sysroot", "/opt/bela", "extra"]),
            ErrorKind::UnknownArgument
        );
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
        assert_eq!(
            parse(&["check-vendor", "--board=root@bela-gem.local"]),
            Ok(compare_against("root@bela-gem.local"))
        );
    }

    /// `--board` says a board is the source, which is the only source
    /// this compares against. Without it there is nothing to compare
    /// with, and defaulting to one would make a plain `check-vendor`
    /// reach for the network unasked.
    #[test]
    fn check_vendor_needs_the_board_flag() {
        assert_eq!(
            rejection(&["check-vendor"]),
            ErrorKind::MissingRequiredArgument
        );
        // The host reads as the board's name, so it follows the flag
        // rather than standing in for it.
        assert_eq!(
            rejection(&["check-vendor", "root@bela.local"]),
            ErrorKind::UnknownArgument
        );
        assert_eq!(
            rejection(&["check-vendor", "root@bela.local", "--board"]),
            ErrorKind::UnknownArgument
        );
    }

    #[test]
    fn check_vendor_compares_against_one_board() {
        assert_eq!(
            rejection(&["check-vendor", "--board", "one.local", "two.local"]),
            ErrorKind::UnknownArgument
        );
    }
}
