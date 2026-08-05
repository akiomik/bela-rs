//! Repository tasks:
//!
//! - `bindgen` regenerates `bela-sys/src/bindings.rs` from the vendored
//!   Bela headers.
//! - `check-vendor` compares those headers with the ones on a board.
//!
//! Run `cargo xtask --help` for the arguments each of them takes.

mod check_vendor;
mod cli;
mod generate;

use std::env;
use std::path::PathBuf;

use clap::Parser;

use crate::cli::{Cli, Task};

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives one level below the repository root")
        .to_path_buf();

    match Cli::parse().task {
        // `--sysroot` overrides `BELA_SYSROOT`, which is the standing
        // setting. Applied here rather than by clap so that parsing
        // answers for the arguments alone.
        Task::Bindgen { sysroot } => {
            let sysroot = sysroot.or_else(|| env::var_os("BELA_SYSROOT").map(PathBuf::from));
            generate::generate(&root, sysroot);
        }
        Task::CheckVendor { board } => check_vendor::check(&root, &board),
    }
}
