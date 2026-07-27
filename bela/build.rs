use std::env;

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
}
