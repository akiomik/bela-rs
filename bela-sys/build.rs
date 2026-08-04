//! Emits the `libbela` link flags for device targets.

use std::path::PathBuf;
use std::{env, fs};

// Library locations captured on the board; see docs/board-facts.md.
const LIB_DIRS: &[&str] = &[
    "/root/Bela/lib",
    "/usr/evl/lib/aarch64-linux-gnu",
    "/usr/local/lib",
    "/usr/lib/aarch64-linux-gnu",
];

// `libbela.so` is C++ and pulls in the EVL real-time runtime and the
// seasocks web server. Rust does not link a C++ runtime by itself, and
// the transitive dependencies are not resolved automatically when
// cross-linking, so name them explicitly.
const LIBS: &[&str] = &["bela", "seasocks", "evl", "stdc++"];

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-env-changed=BELA_SYSROOT");

    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if !(arch == "aarch64" && os == "linux") {
        return;
    }

    // Cross builds set BELA_SYSROOT to a copy of the board's
    // filesystem (see docs/cross-compile.md); native builds on the
    // board itself leave it unset and use the absolute paths.
    let sysroot = env::var("BELA_SYSROOT").unwrap_or_default();
    for dir in LIB_DIRS {
        println!("cargo::rustc-link-search=native={sysroot}{dir}");
    }
    if let Some(dir) = gcc_lib_dir(&sysroot) {
        println!("cargo::rustc-link-search=native={}", dir.display());
    }
    for lib in LIBS {
        println!("cargo::rustc-link-lib=dylib={lib}");
    }
}

// Debian ships the `libstdc++.so` linker symlink under a
// gcc-version-specific directory rather than the multiarch one.
fn gcc_lib_dir(sysroot: &str) -> Option<PathBuf> {
    let base = PathBuf::from(format!("{sysroot}/usr/lib/gcc/aarch64-linux-gnu"));
    fs::read_dir(base)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.join("libstdc++.so").exists())
        // Highest version directory wins if several are installed.
        .max()
}
