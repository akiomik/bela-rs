//! Repository tasks:
//!
//! - `bindgen` regenerates `bela-sys/src/bindings.rs` from the vendored
//!   Bela headers.
//! - `check-vendor` compares those headers with the ones on a board.

mod check_vendor;
mod cli;
mod generate;

use std::env;
use std::path::PathBuf;

use crate::cli::{Task, UsageError};

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives one level below the repository root")
        .to_path_buf();

    let sysroot = env::var_os("BELA_SYSROOT").map(PathBuf::from);
    match Task::parse(env::args().skip(1), sysroot) {
        Ok(Task::Bindgen { sysroot }) => generate::generate(&root, sysroot),
        Ok(Task::CheckVendor { host }) => check_vendor::check(&root, &host),
        Err(UsageError) => cli::usage(),
    }
}
