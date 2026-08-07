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

/// The C++ compiler to build the shim with, from the two variables
/// that can name one.
///
/// `BELA_CXX` wins. Failing that, `BELA_CC` — which
/// `scripts/aarch64-bela-linker.sh` reads, so following it is what
/// keeps the compiler and the linker in one toolchain — answers for
/// both when it ends in `gcc`, which covers the two cases
/// `docs/cross-compile.md` documents: `aarch64-linux-gnu-gcc` on
/// Debian and plain `gcc` on the board itself. With neither set, the
/// tap's name.
///
/// `Err` carries a message for a `BELA_CC` no C++ name follows from.
/// Guessing there would mix toolchains, which is how a binary ends up
/// asking the board for `libstdc++` symbols it does not have.
fn shim_compiler_from(bela_cxx: &str, bela_cc: &str) -> Result<String, String> {
    if !bela_cxx.is_empty() {
        return Ok(bela_cxx.to_owned());
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
