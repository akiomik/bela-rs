//! Emits the `libbela` link flags for device targets, and compiles the
//! MIDI shim when the sysroot carries what it is written against.

use std::path::{Path, PathBuf};
use std::{env, fs};

// Shared with the crate's tests: a build script is not a target
// `cargo test` builds, so the string work lives in a file both can
// include. See shim_compiler.rs.
include!("shim_compiler.rs");

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
//
// `libbelaextra.so` holds the higher-level classes, which is where
// `Midi` is; its own dependencies (`libasound.so.2`, `libNE10.so.10`)
// resolve through the search paths above.
//
// It comes before `bela` because it needs it and does not say so:
// `readelf -d libbelaextra.so` lists no `libbela.so`, while its
// `RtThread`, `SchedulableTask` and `IoUtils` symbols are defined
// there. rustc links with `--as-needed`, so a `libbela` named earlier
// than the library that needs it is dropped as unused and then the
// link fails on those symbols.
const LIBS: &[&str] = &["belaextra", "bela", "seasocks", "evl", "stdc++"];

// The C surface this crate compiles over Bela's `Midi` class. See
// shim/midi.h for what it exports and docs/midi.md for why.
const SHIM_SOURCES: &[&str] = &["shim/midi.cpp", "shim/midi.h"];

// What the shim includes, relative to the sysroot. The first is where
// `Bela.h` and the real-time headers are; the second is what makes
// `<libraries/Midi/Midi.h>` resolve, the same way the board's own
// Makefile puts `/root/Bela` on the include path.
const SHIM_INCLUDE_DIRS: &[&str] = &["/root/Bela/include", "/root/Bela"];

// Every spelling cc accepts for the archiver, in its own order of
// preference (cc-1.4.0, `env_tool`).
const AR_ENV: &[&str] = &[
    "AR_aarch64-unknown-linux-gnu",
    "AR_aarch64_unknown_linux_gnu",
    "TARGET_AR",
    "AR",
];

// The header that says whether a sysroot is one the shim can be built
// against. `scripts/sync-sysroot.sh` has carried it since the commit
// that added `/root/Bela/libraries`; sysroots synced before that have
// `include` and not this.
const SHIM_PROBE: &str = "/root/Bela/libraries/Midi/Midi.h";

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    for source in SHIM_SOURCES {
        println!("cargo::rerun-if-changed={source}");
    }
    println!("cargo::rerun-if-env-changed=BELA_SYSROOT");
    // Named for the same reason as BELA_CC below: it chooses a
    // compiler, and changing it has to rebuild what that compiler made.
    println!("cargo::rerun-if-env-changed=BELA_CXX");
    // What cc reads for the archiver. Named here so that setting one
    // rebuilds the shim, and read in build_shim so that setting one
    // still wins over what this script would pick.
    for name in AR_ENV {
        println!("cargo::rerun-if-env-changed={name}");
    }
    // Nothing here reads BELA_CC — scripts/aarch64-bela-linker.sh
    // does, and cargo cannot see into a linker it was handed as a path.
    // Declaring it makes changing the compiler rebuild this crate, and
    // so relink whatever links it, instead of leaving a binary built by
    // the previous one in place.
    println!("cargo::rerun-if-env-changed=BELA_CC");

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
    // Ahead of the libraries below on purpose: the shim is a static
    // archive calling into `libbelaextra`, and a static archive has to
    // reach the linker before whatever resolves it.
    build_shim(&sysroot);
    for lib in LIBS {
        println!("cargo::rustc-link-lib=dylib={lib}");
    }
}

// Compiles the MIDI shim, when the sysroot carries the sources it is
// written against.
//
// Skipping is the right answer rather than an error, because a build
// without a sysroot is a normal thing to run: `cargo check` and
// `cargo clippy` for the device target never link, and that is what CI
// does, having no board to sync one from. A build that does link
// without a sysroot fails either way — at `-lbela` if not here.
#[allow(
    clippy::panic,
    reason = "a build script reports a misconfiguration by failing the build"
)]
fn build_shim(sysroot: &str) {
    let probe = format!("{sysroot}{SHIM_PROBE}");
    // Declared whether or not it is there. A missing path re-runs this
    // script on every build, which is what makes the warning below
    // recoverable: syncing a sysroot into a path that was already
    // named by BELA_SYSROOT changes no file cargo would otherwise be
    // watching, and the shim would stay uncompiled with nothing said.
    println!("cargo::rerun-if-changed={probe}");
    if !Path::new(&probe).exists() {
        let where_ = if sysroot.is_empty() {
            "BELA_SYSROOT is unset and this is not a board".to_owned()
        } else {
            format!("{sysroot}{SHIM_PROBE} is missing")
        };
        println!(
            "cargo::warning=MIDI shim not compiled ({where_}); \
             linking a device binary will fail on bela_midi_* until \
             scripts/sync-sysroot.sh has run"
        );
        return;
    }

    let compiler = match shim_compiler_from(
        &env::var("BELA_CXX").unwrap_or_default(),
        &env::var("BELA_CC").unwrap_or_default(),
    ) {
        Ok(compiler) => compiler,
        Err(message) => panic!("{message}"),
    };
    let mut build = cc::Build::new();
    build
        .cpp(true)
        // What Bela compiles its own C++ with (docs/board-facts.md).
        // The shim allocates a `Midi`, so it has to agree with
        // `libbelaextra.so` about that class's layout; measured equal
        // between this toolchain and the board's clang++, and pinning
        // the standard is one fewer way for that to drift.
        .std("c++14")
        .file("shim/midi.cpp")
        .compiler(&compiler);
    // cc resolves the archiver from the target triple rather than from
    // the compiler, so it has to be told; an AR already in the
    // environment is left to win, as it would without this.
    if !AR_ENV.iter().any(|name| env::var_os(name).is_some()) {
        if let Some(archiver) = shim_archiver(&compiler) {
            build.archiver(archiver);
        }
    }
    for dir in SHIM_INCLUDE_DIRS {
        build.include(format!("{sysroot}{dir}"));
    }
    if !sysroot.is_empty() {
        build.flag(format!("--sysroot={sysroot}"));
        // Debian keeps its architecture-specific headers here, and a
        // toolchain built for a different triple —
        // aarch64-unknown-linux-gnu against Debian's
        // aarch64-linux-gnu — does not look for them on its own. Same
        // reason scripts/aarch64-bela-linker.sh passes -B.
        build.include(format!("{sysroot}/usr/include/aarch64-linux-gnu"));
    }
    build.compile("bela_midi_shim");
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
