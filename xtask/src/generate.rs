//! `bindgen`: regenerates `bela-sys/src/bindings.rs` from the vendored
//! Bela headers.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use bindgen::callbacks::ParseCallbacks;

const TARGET: &str = "aarch64-unknown-linux-gnu";

/// The package the generated file belongs to, and so the one whose
/// edition decides how it is formatted.
const PACKAGE: &str = "bela-sys";

// The defines the Bela build system compiles with on the Bela Gem
// (captured from a verbose on-board build; see docs/board-facts.md).
// They gate parts of the header surface, so bindgen must see them too.
const BELA_DEFINES: &[&str] = &[
    "-DBELA_USE_POLL",
    "-DENABLE_PRU_UIO=0",
    "-DENABLE_PRU_RPROC=1",
    "-DIS_AM62_PB2",
    "-DIS_AM62",
    "-DBELA_HAS_GPIO",
    "-DBELA_HAS_PRU_AND_MCASP",
    "-DBELA_RT_WRAP(call)=call",
    "-DBELA_EVL",
    "-DNDEBUG",
];

/// Drops one comment bindgen would otherwise attach to a function.
///
/// `GPIOcontrol.h` puts a banner over the block of `gpio_*`
/// declarations — a heading for the thirteen, not a description of the
/// first of them — and bindgen lifts it onto `gpio_setup`, which then
/// carries `gpio_functions` as its summary line on docs.rs while its
/// twelve neighbours carry nothing. Every other comment is passed
/// through untouched.
#[derive(Debug)]
struct DropFamilyBanner;

impl ParseCallbacks for DropFamilyBanner {
    fn process_comment(&self, comment: &str) -> Option<String> {
        comment.trim().eq("gpio_functions").then(String::new)
    }
}

/// Whether the generated `gpio_setup` carries a doc comment.
///
/// This is the condition worth asserting rather than the absence of
/// the banner's text: a banner reworded in the vendored header stops
/// matching `DropFamilyBanner` and is attached under its new name,
/// which a search for the old one would pass. It also stays true of
/// the one benign way the callback can go unused — a bindgen that
/// stops lifting a block comment onto the declaration after it, which
/// is the result the callback exists to produce.
///
/// Everything between the declaration and the `unsafe extern` opening
/// its block belongs to that declaration, so that is what it looks in.
/// Anchoring on the keyword rather than on the block's `{` matters: a
/// banner reworded to contain a brace would move the start of the
/// region past its own text, and the guard would pass on precisely
/// the case it exists to catch.
///
/// It runs before `format`, on bindgen's own token output, which
/// spells an attribute `# [doc = "..."]` with the space — hence
/// matching on `[doc` rather than the `#[doc` the formatted file ends
/// up with.
fn gpio_setup_is_documented(generated: &str) -> bool {
    generated
        .split_once("pub fn gpio_setup")
        .and_then(|(before, _)| before.rsplit_once("unsafe extern"))
        .is_some_and(|(_, attrs)| attrs.contains("[doc"))
}

pub(crate) fn generate(root: &Path, sysroot: Option<PathBuf>) {
    let vendor = root.join("bela-sys/vendor/bela");
    let source = fs::read_to_string(vendor.join("SOURCE")).expect("read vendor SOURCE file");
    let source = source.trim();
    let out = root.join("bela-sys/src/bindings.rs");

    let mut builder = bindgen::Builder::default()
        .header(root.join("bela-sys/wrapper.h").display().to_string())
        .clang_arg(format!("--target={TARGET}"))
        .clang_arg(format!("-I{}", vendor.join("include").display()))
        .clang_args(BELA_DEFINES)
        .use_core()
        .allowlist_type("Bela.*")
        .allowlist_type("AuxiliaryTask")
        .allowlist_function("Bela_.*")
        .allowlist_function("rt_.*")
        // The sysfs GPIO and LED family from `GPIOcontrol.h`, which
        // `Bela.h` includes and `libbela` exports. It is the only way
        // this crate offers to reach a pin that is not one of the
        // sixteen digital channels a `BelaContext` carries, or to
        // reach any pin at all outside a render callback; libbela's
        // own `Gpio` reaches one through the registers instead, which
        // is a different thing and not bound here. `PIN_DIRECTION`
        // and `PIN_VALUE` are the vocabulary its `out_flag` and
        // `value` arguments are written in, so they come with the
        // functions.
        .allowlist_function("gpio_.*")
        .allowlist_function("led_set_trigger")
        .allowlist_type("PIN_.*")
        .allowlist_var("BELA_.*")
        .allowlist_var("DEFAULT_.*")
        .prepend_enum_name(false)
        // The FILE* / va_list printf variants would drag glibc internals
        // into the bindings and are not usable from Rust anyway.
        .blocklist_function("(rt|Bela)_v?fprintf")
        .blocklist_function("(rt|Bela)_vprintf")
        .blocklist_type("^FILE$")
        .blocklist_type("^_IO_FILE$")
        .blocklist_type("^_IO_marker$")
        .blocklist_type("^_IO_codecvt$")
        .blocklist_type("^_IO_wide_data$")
        .blocklist_type("^_IO_lock_t$")
        .blocklist_type("^__off_t$")
        .blocklist_type("^__off64_t$")
        .blocklist_type("^va_list$")
        .blocklist_type("^__gnuc_va_list$")
        .blocklist_type("^__BindgenOpaqueArray$")
        .derive_default(true)
        .parse_callbacks(Box::new(DropFamilyBanner))
        // Formatting is left to `cargo fmt`; see `format`.
        .formatter(bindgen::Formatter::None)
        .raw_line(format!(
            "//! Bindings to the Bela core API, generated by `cargo xtask bindgen`\n\
             //! from Bela headers vendored from: {source}. Do not edit by hand."
        ));
    if let Some(sysroot) = sysroot {
        builder = builder.clang_arg(format!("--sysroot={}", sysroot.display()));
    }

    let bindings = builder.generate().expect("bindgen failed");
    // `DropFamilyBanner` goes wrong silently — bindgen consults only
    // the *last* registered `parse_callbacks` for comments, so one
    // added after it wins, and the match is on the banner's exact
    // text, so rewording it in the vendored header is enough. Neither
    // is an error to bindgen, and no CI job regenerates this file to
    // notice. See `gpio_setup_is_documented` for why that is the
    // condition tested rather than the banner's text.
    let generated = bindings.to_string();
    assert!(
        generated.contains("pub fn gpio_setup"),
        "the gpio_* family is not being generated; the allowlist above \
         is what puts it in"
    );
    assert!(
        !gpio_setup_is_documented(&generated),
        "gpio_setup came out with a doc comment, which is GPIOcontrol.h's \
         banner for the whole family: DropFamilyBanner has been displaced \
         by a later parse_callbacks, or the banner was reworded"
    );
    bindings.write_to_file(&out).expect("write bindings.rs");
    format(root);
    println!("wrote {}", out.display());
}

/// Formats the generated file the way the rest of the workspace is
/// formatted.
///
/// bindgen can run rustfmt itself, and used to. It picks the edition to
/// format for from its own idea of the current Rust release rather than
/// from this workspace, and that idea stops at the newest release the
/// bindgen release knows about — 1.82 here, whose latest edition is
/// 2021. The two editions disagree about where a wrapped return type
/// goes, so `cargo xtask bindgen` and `cargo fmt` each rewrote the
/// other's output, and a rerun of the task reported a diff that said
/// nothing about the headers it had read.
///
/// Deferring to `cargo fmt` leaves one authority on formatting, the one
/// CI already checks, and it reads the edition from `Cargo.toml`
/// instead of from a second copy kept here.
fn format(root: &Path) {
    // `CARGO` is set for anything cargo runs, which is how this task is
    // run; the fallback is for a directly invoked binary.
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = Command::new(cargo)
        .args(["fmt", "--package", PACKAGE, "--manifest-path"])
        .arg(root.join("Cargo.toml"))
        .status()
        .expect("run cargo fmt");
    assert!(status.success(), "cargo fmt failed: {status}");
}
