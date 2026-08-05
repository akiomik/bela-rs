//! Repository tasks:
//!
//! - `bindgen` regenerates `bela-sys/src/bindings.rs` from the vendored
//!   Bela headers.
//! - `check-vendor` compares those headers with the ones on a board.

mod check_vendor;
mod generate;

use std::path::PathBuf;
use std::{env, process};

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives one level below the repository root")
        .to_path_buf();

    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("bindgen") => {
            let mut sysroot = env::var_os("BELA_SYSROOT").map(PathBuf::from);
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--sysroot" => {
                        let value = args.next().unwrap_or_else(|| usage());
                        sysroot = Some(PathBuf::from(value));
                    }
                    _ => usage(),
                }
            }
            generate::generate(&root, sysroot);
        }
        Some("check-vendor") => {
            // A board is the only source worth checking against: the
            // headers are pinned to one, not to an upstream ref.
            let mut board = false;
            let mut host = None;
            for arg in args {
                match arg.as_str() {
                    "--board" if !board => board = true,
                    _ if board && host.is_none() && !arg.starts_with('-') => host = Some(arg),
                    _ => usage(),
                }
            }
            if !board {
                usage();
            }
            check_vendor::check(&root, host.as_deref().unwrap_or(check_vendor::DEFAULT_HOST));
        }
        _ => usage(),
    }
}

#[allow(
    clippy::exit,
    reason = "a usage error is reported to the caller as exit code 2"
)]
fn usage() -> ! {
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
