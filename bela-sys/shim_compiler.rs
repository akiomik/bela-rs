// Which toolchain builds the MIDI shim, as pure string work.
//
// Included by build.rs, which is where it runs, and by src/lib.rs
// under cfg(test), which is where it is tested: a build script is not
// a target `cargo test` builds, and getting this wrong compiles the
// shim with one toolchain and links it with another. Included rather
// than imported because those two are different crates.
//
// This header is `//` rather than `//!` because the file is pasted
// into the middle of two others, where an inner doc comment is a
// syntax error. The items below keep their `///`.

/// The C++ compiler assumed when nothing names one: the macOS tap's,
/// matching the default in `scripts/aarch64-bela-linker.sh`.
const DEFAULT_CXX: &str = "aarch64-unknown-linux-gnu-g++";

/// The name `RUSTC_LINKER` carries when Cargo resolved the
/// compatibility wrapper rather than a compiler driver directly. It
/// names no C++ compiler by itself — only `BELA_CC` does, for that
/// path — so it falls through to that instead of being read as an
/// unsupported linker.
const WRAPPER_LINKER_NAME: &str = "aarch64-bela-linker.sh";

/// The C++ compiler to build the shim with, so it agrees with
/// whatever compiler drives the final link.
///
/// In order:
///
/// 1. `BELA_CXX`, when set, always wins.
/// 2. Otherwise `RUSTC_LINKER` — the compiler driver Cargo resolved
///    for the target, present when `.cargo/config.toml` or
///    `CARGO_TARGET_*_LINKER` names one directly (`docs/cross-compile.md`),
///    which is the case a direct linker setting is meant to cover: no
///    `BELA_CC` required. A name ending in `gcc` answers for the C++
///    compiler beside it; the wrapper's own name is treated as
///    "nothing resolved" and falls through to `BELA_CC`; anything else
///    is refused, because guessing would risk building the shim with
///    one toolchain and linking it with another.
/// 3. Otherwise `BELA_CC` — the legacy wrapper reads this and calls it
///    as the linker, so following it here keeps the shim and the link
///    in one toolchain. Covers the two cases `docs/cross-compile.md`
///    documents: `aarch64-linux-gnu-gcc` on Debian and plain `gcc` on
///    the board itself.
/// 4. With none of the three set, the tap's name.
///
/// `Err` carries a message for a compiler name no C++ compiler follows
/// from. Guessing there would mix toolchains, which is how a binary
/// ends up asking the board for `libstdc++` symbols it does not have.
fn shim_compiler_from(bela_cxx: &str, rustc_linker: &str, bela_cc: &str) -> Result<String, String> {
    if !bela_cxx.is_empty() {
        return Ok(bela_cxx.to_owned());
    }
    if !rustc_linker.is_empty() && !is_wrapper_linker(rustc_linker) {
        return gnu_prefix(rustc_linker, "gcc").map_or_else(
            || {
                Err(format!(
                    "the configured linker is `{rustc_linker}`, and no C++ compiler name \
                     follows from it; set BELA_CXX to the matching C++ compiler"
                ))
            },
            |prefix| Ok(format!("{prefix}g++")),
        );
    }
    if bela_cc.is_empty() {
        return Ok(DEFAULT_CXX.to_owned());
    }
    if let Some(prefix) = gnu_prefix(bela_cc, "gcc") {
        return Ok(format!("{prefix}g++"));
    }
    Err(format!(
        "BELA_CC is `{bela_cc}`, and no C++ compiler name follows from it; \
         set BELA_CXX to the matching C++ compiler"
    ))
}

/// Whether `rustc_linker` names the compatibility wrapper rather than
/// a compiler driver, checked against the last path segment so an
/// absolute path (Cargo resolves config-file linker paths against the
/// workspace root) still matches.
fn is_wrapper_linker(rustc_linker: &str) -> bool {
    rustc_linker.rsplit('/').next().unwrap_or(rustc_linker) == WRAPPER_LINKER_NAME
}

/// The archiver that belongs to `compiler`, or [`None`] when nothing
/// follows from its name.
///
/// `cc` resolves the archiver from a table of target prefixes rather
/// than from the compiler it was given, so a toolchain whose prefix is
/// not the target triple — `aarch64-unknown-linux-gnu-g++` against a
/// target Debian spells `aarch64-linux-gnu` — gets an archiver from
/// somewhere else, and on a host that has none, the host's own. An
/// `ar` that does not understand aarch64 ELF produces a broken archive
/// rather than an error.
///
/// Only a name ending in `g++` is derived from: `clang++` wants
/// `llvm-ar` rather than an `ar` beside it, and stripping the suffix
/// would name the host's. Those keep `cc`'s own resolution, which
/// `AR` and `AR_<target>` still override — as they do here too, since
/// `build.rs` only offers this when neither is set.
fn shim_archiver(compiler: &str) -> Option<String> {
    let prefix = gnu_prefix(compiler, "g++")?;
    Some(format!("{prefix}ar"))
}

/// Everything before the tool in a GNU tool name — `aarch64-linux-gnu-`
/// for `aarch64-linux-gnu-gcc`, `/usr/bin/` for `/usr/bin/gcc`, and the
/// empty string for a bare `gcc`.
///
/// [`None`] for a name that merely ends in those letters: `clang++`
/// ends in `g++` without being one, and deriving `clanar` from it is
/// how a fallback becomes a wrong answer instead of no answer.
fn gnu_prefix<'a>(name: &'a str, tool: &str) -> Option<&'a str> {
    if name == tool {
        return Some("");
    }
    name.strip_suffix(tool).filter(|prefix| {
        // A triple ends in `-`, and a path in `/`: both
        // aarch64-linux-gnu-gcc and /usr/bin/gcc name a toolchain,
        // and docs/cross-compile.md allows either spelling. What is
        // ruled out is a longer word that merely ends in these
        // letters.
        prefix.ends_with('-') || prefix.ends_with('/')
    })
}
