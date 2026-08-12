//! Emits the `bela_device` cfg for device targets, and relays the
//! device link arguments `bela-sys` publishes.

use std::env;

include!("link_args.rs");

// The parts of the crate that call into libbela can only link on the board.
// Gate them behind a custom `bela_device` cfg so that host builds (unit
// tests, clippy) and non-linking cross checks stay warning-free.
fn main() {
    println!("cargo::rustc-check-cfg=cfg(bela_device)");
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if arch == "aarch64" && os == "linux" {
        println!("cargo::rustc-cfg=bela_device");
    }
    relay_link_args();
}

// `links` metadata reaches only an immediate dependent: bela-sys's
// `DEP_BELA_LINK_ARGS_*` (see bela-sys/link_args.rs) is visible here,
// not to an application three crates away. Apply the arguments to
// this crate's own examples and tests, and republish them under
// `bela`'s own `links` name (`bela_relay`) so a dependent's build
// script can read `DEP_BELA_RELAY_LINK_ARGS_*` and do the same. See
// docs/cross-compile.md.
fn relay_link_args() {
    let args = decode_link_args(|key| env::var(key).ok(), "DEP_BELA");
    for arg in &args {
        println!("cargo::rustc-link-arg={arg}");
    }
    for (key, value) in encode_link_args(&args) {
        println!("cargo::metadata={key}={value}");
    }
}
