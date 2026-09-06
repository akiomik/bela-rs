//! `check-vendor`: compares the vendored headers with the ones on a
//! board.
//!
//! Two trees are pinned to the board image, for the same reason and
//! against different failures:
//!
//! - `vendor/bela` is what `bela-sys/src/bindings.rs` is generated
//!   from (see `bela-sys/vendor/bela/SOURCE` and
//!   `docs/board-facts.md`). Updating the image changes the Bela
//!   version, and nothing in the build notices: the committed bindings
//!   would keep describing the ABI of the older headers while the
//!   `libbela` they link against has moved on, which can shift
//!   `BelaContext` field offsets underneath running code.
//! - `vendor/ne10` generates nothing. `bela-sys/src/ne10.rs` is
//!   written by hand, so the two headers there are a baseline for
//!   noticing that a board image moved NE10 (`docs/fft.md`).
//!   `bela-sys/abi/ne10_abi.c` catches the layout and signature half
//!   of that at build time; what only a board can answer is whether
//!   the *library* was rebuilt, which is why `vendor/ne10/SOURCE`
//!   records its build ID and hash and this task compares them.
//!
//! This needs a board, so it cannot run in CI. It is the check a human
//! runs after updating a board image.

// The exit code is this task's result — drift is reported by exiting
// non-zero, which is what a caller (a human, or a shell script) reads.
#![allow(
    clippy::exit,
    reason = "the exit status is the reported result of the check"
)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs, process};

/// The board `scripts/update-vendor.sh` and `scripts/smoke-test.sh`
/// default to.
pub(crate) const DEFAULT_HOST: &str = "root@bela.local";

/// Where `scripts/update-vendor.sh --board` takes the Bela files from.
const REMOTE_ROOT: &str = "/root/Bela";

/// Where it takes the NE10 headers from: Debian's package rather than
/// anything Bela ships, so a directory of its own.
const NE10_REMOTE_ROOT: &str = "/usr/include/ne10";

/// The library those headers describe. Nothing vendors its content —
/// it is a binary on the board — but `vendor/ne10/SOURCE` records
/// which build it was, because a rebuilt one with unchanged headers is
/// exactly the case `scripts/probe-fft.sh` has to be run again for.
const NE10_LIB: &str = "/usr/lib/aarch64-linux-gnu/libNE10.so.10";

/// One vendored tree.
struct Tree {
    /// Its directory under `bela-sys/vendor`.
    name: &'static str,
    /// The directory on the board whose files it mirrors, path for
    /// path.
    remote_root: &'static str,
    /// A header to read a version out of and report, where the tree
    /// has one that says which release it is.
    version_header: Option<&'static str>,
    /// A library whose identity `SOURCE` records, where the headers
    /// alone do not say which build they describe.
    library: Option<&'static str>,
}

const TREES: &[Tree] = &[
    Tree {
        name: "bela",
        remote_root: REMOTE_ROOT,
        version_header: Some(VERSION_HEADER),
        library: None,
    },
    Tree {
        name: "ne10",
        remote_root: NE10_REMOTE_ROOT,
        // NE10's version lives in versionheader.h, which nothing here
        // includes; `SOURCE` carries it as prose instead, next to the
        // build id that actually identifies the library.
        version_header: None,
        library: Some(NE10_LIB),
    },
];

/// The header carrying the `BELA_*_VERSION` macros, relative to the
/// `bela` tree.
const VERSION_HEADER: &str = "include/Bela.h";

const SSH_OPTIONS: &[&str] = &["-o", "ConnectTimeout=10"];

/// How much of a differing file to show before pointing at the update
/// script instead. A version bump of `Bela.h` diffs into hundreds of
/// lines, and the decision — "the pin is stale" — is made by the first
/// few.
const MAX_DIFF_LINES: usize = 60;

pub(crate) fn check(root: &Path, host: &str) -> ! {
    println!(
        "board:    {host} ({REMOTE_ROOT} at git HEAD {})",
        board_head(host)
    );

    let scratch = scratch_dir();
    let mut drifted = 0;
    let mut missing = 0;
    let mut libraries_rebuilt = 0;
    for tree in TREES {
        let Tree {
            name,
            remote_root,
            version_header,
            library,
        } = tree;
        let vendor = root.join("bela-sys/vendor").join(name);
        let source = fs::read_to_string(vendor.join("SOURCE")).expect("read vendor SOURCE file");
        println!();
        println!("vendor/{name} <- {host}:{remote_root}");
        for line in source.trim().lines() {
            println!("  {line}");
        }

        // Fetch everything first: the board's version comes out of the
        // header just pulled, not out of a separate query that could
        // answer for a different file.
        let fetched: Vec<(String, Result<PathBuf, String>)> = vendored_files(&vendor)
            .into_iter()
            .map(|rel| {
                let result = fetch(host, remote_root, &rel, &scratch.join(name));
                (rel, result)
            })
            .collect();

        if let Some(header) = version_header {
            let board_header = fetched
                .iter()
                .find(|(rel, _)| rel == header)
                .and_then(|(_, result)| result.as_ref().ok());
            println!(
                "  version:  vendored {}, board {}",
                version(&vendor.join(header)).unwrap_or_else(|| "unknown".into()),
                board_header
                    .and_then(|path| version(path))
                    .unwrap_or_else(|| "unknown".into()),
            );
        }

        for (rel, result) in &fetched {
            match result {
                Err(error) => {
                    report("MISSING", rel);
                    indent(error);
                    drifted += 1;
                    missing += 1;
                }
                Ok(path) => {
                    let vendored = vendor.join(rel);
                    let ours = fs::read(&vendored).expect("read a vendored file");
                    let theirs = fs::read(path).expect("read a file fetched from the board");
                    if ours == theirs {
                        report("ok", rel);
                    } else {
                        report("DRIFT", rel);
                        indent(&diff(&vendored, path, remote_root, rel, host));
                        drifted += 1;
                    }
                }
            }
        }

        // Headers that cannot say which build of a library they
        // describe: same soname, any implementation.
        if let Some(library) = library {
            let rebuilt = check_library_identity(host, library, &source);
            libraries_rebuilt += rebuilt;
            drifted += rebuilt;
        }
    }
    let _ = fs::remove_dir_all(&scratch);

    println!();
    if drifted == 0 {
        println!("the vendored headers and libraries match {host}");
        process::exit(0);
    }
    println!("{drifted} vendored item(s) differ from {host}");
    println!("Re-pin them with:");
    println!("  scripts/update-vendor.sh --board {host}");
    println!("  cargo xtask bindgen --sysroot <dir>");
    if libraries_rebuilt > 0 {
        println!();
        println!("A library was rebuilt, which the headers cannot show. What it does");
        println!("to its arguments was measured against the old one, so re-measure:");
        println!("  scripts/probe-fft.sh {host}");
        println!("and update the answers in docs/fft.md, which name the build id they");
        println!("were taken against.");
    }
    if missing > 0 {
        println!();
        println!("The update script only copies what the board has, so a file the");
        println!("board no longer carries stays behind: check whether the include");
        println!("closure shrank and remove it from the vendor directory by hand.");
    }
    process::exit(1);
}

/// Compares the identity `SOURCE` records for `library` with the
/// board's, and returns how many of the two values differ.
///
/// A build ID or a hash that could not be read is reported and counts
/// as neither: `readelf` is binutils, which a board image need not
/// carry, and a check that failed the run over a missing tool would
/// teach people to ignore it. A value that *was* read and differs is
/// drift, and it is the kind the headers cannot show — the FFT's
/// behaviour is measured against one build (`docs/fft.md`), so a
/// different one puts those measurements back in question.
fn check_library_identity(host: &str, library: &str, source: &str) -> u32 {
    let mut drifted = 0;
    for (key, command) in [
        (
            "build-id",
            format!("readelf -n {library} | awk '/Build ID:/ {{ print $3 }}'"),
        ),
        ("sha256", format!("sha256sum {library} | cut -d' ' -f1")),
    ] {
        let recorded = recorded_value(source, key);
        let what = format!("{library} {key}");
        match (recorded, board_value(host, &command)) {
            (Some(recorded), Some(board)) if recorded == board => report("ok", &what),
            (Some(recorded), Some(board)) => {
                report("DRIFT", &what);
                indent(&format!("recorded {recorded}\nboard    {board}"));
                drifted += 1;
            }
            (None, _) => {
                report("?", &what);
                indent("not recorded in vendor/ne10/SOURCE");
            }
            (Some(_), None) => {
                report("?", &what);
                indent("the board did not answer; is binutils installed?");
            }
        }
    }
    drifted
}

/// The value of a `key: value` line in a `SOURCE` file.
fn recorded_value(source: &str, key: &str) -> Option<String> {
    source
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{key}: ")))
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "unknown")
        .map(ToOwned::to_owned)
}

/// What `command` prints on the board, or [`None`] if it failed.
fn board_value(host: &str, command: &str) -> Option<String> {
    let output = Command::new("ssh")
        .args(SSH_OPTIONS)
        .arg(host)
        .arg(command)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!value.is_empty()).then_some(value)
}

/// The files `scripts/update-vendor.sh` copies from the board, as
/// paths relative to both the vendor directory and the board
/// directory it mirrors. Read from the directory rather than listed
/// here, so a header added to an include closure is checked without
/// touching this file. `SOURCE` is not among them: it is provenance
/// the update script writes, not a copy of anything.
fn vendored_files(vendor: &Path) -> Vec<String> {
    let mut files = Vec::new();
    collect_files(vendor, "", &mut files);
    // Directories first would put `LICENSE` above `include/Bela.h`;
    // sorting the whole list is what makes the report stable whichever
    // order the filesystem hands them over in.
    files.sort();
    files
}

/// Every file under `dir`, depth first, as paths relative to the tree
/// root and prefixed with `prefix`.
fn collect_files(dir: &Path, prefix: &str, files: &mut Vec<String>) {
    for entry in fs::read_dir(dir).expect("read a vendored directory") {
        let entry = entry.expect("read a vendored directory entry");
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        if entry.path().is_dir() {
            collect_files(&entry.path(), &rel, files);
        } else if rel != "SOURCE" {
            files.push(rel);
        }
    }
}

/// The board's `Bela` checkout, in the terms `SOURCE` records it. Also
/// the reachability check: everything after this assumes the board
/// answers.
fn board_head(host: &str) -> String {
    let output = Command::new("ssh")
        .args(SSH_OPTIONS)
        .arg(host)
        .arg(format!("git -C {REMOTE_ROOT} rev-parse --short HEAD"))
        .output();
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            eprintln!("could not run ssh: {error}");
            process::exit(2);
        }
    };
    if output.status.success() {
        return String::from_utf8_lossy(&output.stdout).trim().to_owned();
    }
    // ssh reports its own failures as 255, so anything else came from
    // the command it ran: a board whose `git` says nothing about the
    // overlaid sources is still worth comparing files against, an
    // unreachable one is not. A board answering "no drift" because
    // nothing could be fetched would be the wrong answer entirely.
    if output.status.code() == Some(255) {
        eprintln!("could not reach {host}:");
        eprintln!("{}", String::from_utf8_lossy(&output.stderr).trim());
        process::exit(2);
    }
    "unknown".to_owned()
}

fn fetch(host: &str, remote_root: &str, rel: &str, scratch: &Path) -> Result<PathBuf, String> {
    let local = scratch.join(rel);
    fs::create_dir_all(local.parent().expect("the fetched path has a parent"))
        .expect("create the scratch directory");
    let output = Command::new("scp")
        .args(SSH_OPTIONS)
        .arg(format!("{host}:{remote_root}/{rel}"))
        .arg(&local)
        .output()
        .map_err(|error| format!("could not run scp: {error}"))?;
    if output.status.success() {
        Ok(local)
    } else {
        Err(scp_error(&String::from_utf8_lossy(&output.stderr)))
    }
}

/// What scp had to say, without the login banner and locale warning the
/// image greets every session with (`scripts/smoke-test.sh` holds those
/// back the same way). Anything else — a connection failure, say — has
/// no `scp:` line and is reported whole.
fn scp_error(stderr: &str) -> String {
    let reported: Vec<&str> = stderr
        .lines()
        .filter(|line| line.starts_with("scp:"))
        .collect();
    if reported.is_empty() {
        stderr.trim().to_owned()
    } else {
        reported.join("\n")
    }
}

/// The `BELA_*_VERSION` macros in a header file, as `1.18.0`. A cheap
/// first signal: it names the drift the way the Bela changelog does,
/// while the content comparison is what actually decides it.
fn version(header: &Path) -> Option<String> {
    version_in(&fs::read_to_string(header).ok()?)
}

/// The version [`version`] reads, given the text of the header. All
/// three macros have to be there: two out of three is not a version,
/// and reporting `1.18` for it would read as one.
fn version_in(text: &str) -> Option<String> {
    let parts: Vec<&str> = ["MAJOR", "MINOR", "BUGFIX"]
        .iter()
        .filter_map(|part| {
            let define = format!("#define BELA_{part}_VERSION ");
            text.lines()
                .find_map(|line| line.strip_prefix(&define))
                .map(str::trim)
        })
        .collect();
    (parts.len() == 3).then(|| parts.join("."))
}

fn diff(vendored: &Path, fetched: &Path, remote_root: &str, rel: &str, host: &str) -> String {
    let output = Command::new("diff")
        .arg("-u")
        .args(["-L", &format!("vendored {rel}")])
        .args(["-L", &format!("{host}:{remote_root}/{rel}")])
        .args([vendored, fetched])
        .output();
    match output {
        Ok(output) => abridge(&String::from_utf8_lossy(&output.stdout)),
        Err(error) => format!("(the files differ; could not run diff: {error})"),
    }
}

/// A diff cut down to [`MAX_DIFF_LINES`], saying how much was left out
/// and where to read the rest. Anything that short is passed through
/// as it is, trailing newline included.
fn abridge(diff: &str) -> String {
    let lines: Vec<&str> = diff.lines().collect();
    if lines.len() <= MAX_DIFF_LINES {
        return diff.to_owned();
    }
    format!(
        "{}\n({} more diff line(s); update the pin and read the change as a git diff)",
        lines[..MAX_DIFF_LINES].join("\n"),
        lines.len() - MAX_DIFF_LINES,
    )
}

fn scratch_dir() -> PathBuf {
    let dir = env::temp_dir().join(format!("bela-rs-check-vendor.{}", process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create the scratch directory");
    dir
}

/// The same two columns `scripts/smoke-test.sh` reports its checks in.
fn report(state: &str, what: &str) {
    println!("  {state:<8}{what}");
}

fn indent(text: &str) {
    for line in text.lines() {
        println!("        {line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape `Bela.h` carries the macros in, with the surrounding
    /// lines that have to be looked past.
    const HEADER: &str = "\
#pragma once
#define BELA_MAJOR_VERSION 1
#define BELA_MINOR_VERSION 18
#define BELA_BUGFIX_VERSION 0

int Bela_initAudio(BelaInitSettings* settings, void* userData);
";

    fn repository_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask lives one level below the repository root")
            .to_path_buf()
    }

    #[test]
    fn a_version_is_read_from_the_three_macros() {
        assert_eq!(version_in(HEADER), Some("1.18.0".to_owned()));
    }

    #[test]
    fn a_partial_version_is_no_version() {
        for missing in ["MAJOR", "MINOR", "BUGFIX"] {
            let header: String = HEADER
                .lines()
                .filter(|line| !line.contains(&format!("BELA_{missing}_VERSION")))
                .collect::<Vec<_>>()
                .join("\n");
            assert_eq!(version_in(&header), None, "without BELA_{missing}_VERSION");
        }
        assert_eq!(version_in(""), None);
    }

    /// The vendored header is the one the task actually reports on, so
    /// the parsing has to hold for it and not only for a sample of it.
    /// Which version it states is not asserted: re-pinning to a new
    /// board image is expected to change it, and having to edit a test
    /// for that would say nothing about whether the pin is right.
    #[test]
    fn the_vendored_header_states_a_version() {
        let header = repository_root()
            .join("bela-sys/vendor/bela")
            .join(VERSION_HEADER);
        assert!(version(&header).is_some(), "{}", header.display());
    }

    #[test]
    fn scp_errors_are_reported_without_the_login_banner() {
        let stderr = "\
Linux bela 6.6.32 aarch64
Last login: Wed Aug  5 12:00:00 2026
-bash: warning: setlocale: LC_ALL: cannot change locale (en_US.UTF-8)
scp: /root/Bela/include/Bela.h: No such file or directory
";
        assert_eq!(
            scp_error(stderr),
            "scp: /root/Bela/include/Bela.h: No such file or directory"
        );
    }

    /// A failure that never reached scp — an unresolvable host, say —
    /// has no `scp:` line, and dropping everything would report it as
    /// no failure at all.
    #[test]
    fn a_failure_without_an_scp_line_is_reported_whole() {
        let stderr = "ssh: Could not resolve hostname bela.local\n";
        assert_eq!(
            scp_error(stderr),
            "ssh: Could not resolve hostname bela.local"
        );
    }

    #[test]
    fn a_short_diff_is_left_alone() {
        for count in [0, 1, MAX_DIFF_LINES - 1, MAX_DIFF_LINES] {
            let diff = "-a line\n".repeat(count);
            assert_eq!(abridge(&diff), diff, "{count} line(s)");
        }
    }

    #[test]
    fn a_long_diff_is_cut_down_and_says_how_much_is_missing() {
        let lines: Vec<String> = (0..MAX_DIFF_LINES + 5)
            .map(|line| format!("-line {line}"))
            .collect();
        let abridged = abridge(&format!("{}\n", lines.join("\n")));
        let lines: Vec<&str> = abridged.lines().collect();

        assert_eq!(lines.len(), MAX_DIFF_LINES + 1);
        assert_eq!(lines[0], "-line 0");
        assert_eq!(
            lines[MAX_DIFF_LINES - 1],
            format!("-line {}", MAX_DIFF_LINES - 1)
        );
        assert!(
            lines[MAX_DIFF_LINES].starts_with("(5 more diff line(s);"),
            "{:?}",
            lines[MAX_DIFF_LINES]
        );
    }

    /// What the task compares is whatever is vendored, so this reads
    /// the real directories: a header added to an include closure has
    /// to be picked up without anyone editing this crate.
    #[test]
    fn every_vendored_file_is_listed_once_in_order() {
        for tree in TREES {
            let vendor = repository_root().join("bela-sys/vendor").join(tree.name);
            let files = vendored_files(&vendor);

            assert!(!files.is_empty(), "vendor/{} is empty", tree.name);
            assert!(files.is_sorted(), "vendor/{}: {files:?}", tree.name);
            assert!(
                !files.iter().any(|rel| rel.ends_with("SOURCE")),
                "SOURCE is provenance the update script writes, not a copy: {files:?}"
            );
            for rel in &files {
                assert!(vendor.join(rel).is_file(), "vendor/{}: {rel}", tree.name);
            }
        }
    }

    /// Each tree's own shape, which is what the paths on the board
    /// have to line up with: Bela's headers sit under `include/` there
    /// and so do the vendored copies, while NE10's are the contents of
    /// one directory.
    #[test]
    fn each_tree_mirrors_the_board_directory_it_came_from() {
        let bela = vendored_files(&repository_root().join("bela-sys/vendor/bela"));
        assert!(
            bela.iter().any(|rel| rel == VERSION_HEADER),
            "{VERSION_HEADER} is the one the version is read from: {bela:?}"
        );
        assert!(
            bela.contains(&"LICENSE".to_owned()),
            "the LGPL text belongs with the headers it covers: {bela:?}"
        );

        let ne10 = vendored_files(&repository_root().join("bela-sys/vendor/ne10"));
        assert!(
            ne10.iter().all(|rel| !rel.contains('/')),
            "NE10's headers are one flat directory on the board: {ne10:?}"
        );
        // The include closure of abi/ne10_abi.c, which is what the two
        // are there to be a baseline for.
        for header in ["NE10_dsp.h", "NE10_types.h"] {
            assert!(ne10.contains(&header.to_owned()), "{header}: {ne10:?}");
        }
    }

    /// The identity `vendor/ne10/SOURCE` records, in the form
    /// `scripts/update-vendor.sh` writes and this task reads back.
    #[test]
    fn the_ne10_source_records_a_build_id_and_a_hash() {
        let source = fs::read_to_string(repository_root().join("bela-sys/vendor/ne10/SOURCE"))
            .expect("read vendor/ne10/SOURCE");

        let build_id = recorded_value(&source, "build-id").expect("a build id");
        assert!(
            build_id.len() == 40 && build_id.chars().all(|c| c.is_ascii_hexdigit()),
            "a GNU build id is 40 hex digits, got {build_id:?}"
        );
        let sha256 = recorded_value(&source, "sha256").expect("a hash");
        assert!(
            sha256.len() == 64 && sha256.chars().all(|c| c.is_ascii_hexdigit()),
            "a sha256 is 64 hex digits, got {sha256:?}"
        );
    }

    #[test]
    fn an_unrecorded_identity_is_not_a_value() {
        // What update-vendor.sh writes when the board could not answer.
        // Comparing against the word "unknown" would report drift on
        // every board rather than saying the value is missing.
        assert_eq!(recorded_value("build-id: unknown\n", "build-id"), None);
        assert_eq!(recorded_value("sha256: \n", "sha256"), None);
        assert_eq!(
            recorded_value("board root@bela.local: NE10 0.9.10\n", "sha256"),
            None
        );
        assert_eq!(
            recorded_value("sha256: abc123\n", "sha256"),
            Some("abc123".to_owned())
        );
    }
}
