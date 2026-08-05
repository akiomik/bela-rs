//! `check-vendor`: compares the vendored Bela headers with the ones on
//! a board.
//!
//! The vendored headers are pinned to the Bela version shipped on a
//! particular board image (see `bela-sys/vendor/bela/SOURCE` and
//! `docs/board-facts.md`), and `bela-sys/src/bindings.rs` is generated
//! from them. Updating the image changes that version, and nothing in
//! the build notices: the committed bindings would keep describing the
//! ABI of the older headers while the `libbela` they link against has
//! moved on, which can shift `BelaContext` field offsets underneath
//! running code.
//!
//! This needs a board, so it cannot run in CI. It is the check a human
//! runs after updating a board image.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs, process};

/// The board `scripts/update-vendor.sh` and `scripts/smoke-test.sh`
/// default to.
pub const DEFAULT_HOST: &str = "root@bela.local";

/// Where `scripts/update-vendor.sh --board` takes the files from.
const REMOTE_ROOT: &str = "/root/Bela";

/// The header carrying the `BELA_*_VERSION` macros.
const VERSION_HEADER: &str = "include/Bela.h";

const SSH_OPTIONS: &[&str] = &["-o", "ConnectTimeout=10"];

/// How much of a differing file to show before pointing at the update
/// script instead. A version bump of `Bela.h` diffs into hundreds of
/// lines, and the decision — "the pin is stale" — is made by the first
/// few.
const MAX_DIFF_LINES: usize = 60;

pub fn check(root: &Path, host: &str) -> ! {
    let vendor = root.join("bela-sys/vendor/bela");
    let source = fs::read_to_string(vendor.join("SOURCE")).expect("read vendor SOURCE file");
    let files = vendored_files(&vendor);

    println!("vendored: {}", source.trim());
    println!(
        "board:    {host}:{REMOTE_ROOT} (git HEAD {})",
        board_head(host)
    );

    // Fetch everything first: the board's version comes out of the
    // header just pulled, not out of a separate query that could
    // answer for a different file.
    let scratch = scratch_dir();
    let fetched: Vec<(String, Result<PathBuf, String>)> = files
        .into_iter()
        .map(|rel| {
            let result = fetch(host, &rel, &scratch);
            (rel, result)
        })
        .collect();

    let board_header = fetched
        .iter()
        .find(|(rel, _)| rel == VERSION_HEADER)
        .and_then(|(_, result)| result.as_ref().ok());
    println!(
        "version:  vendored {}, board {}",
        version(&vendor.join(VERSION_HEADER)).unwrap_or_else(|| "unknown".into()),
        board_header
            .and_then(|path| version(path))
            .unwrap_or_else(|| "unknown".into()),
    );

    let mut drifted = 0;
    let mut missing = 0;
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
                    indent(&diff(&vendored, path, rel, host));
                    drifted += 1;
                }
            }
        }
    }
    let _ = fs::remove_dir_all(&scratch);

    println!();
    if drifted == 0 {
        println!("the vendored headers match {host}");
        process::exit(0);
    }
    println!("{drifted} vendored file(s) differ from {host}");
    println!("Re-pin them with:");
    println!("  scripts/update-vendor.sh --board {host}");
    println!("  cargo xtask bindgen --sysroot <dir>");
    if missing > 0 {
        println!();
        println!("The update script only copies what the board has, so a file the");
        println!("board no longer carries stays behind: check whether the include");
        println!("closure shrank and remove it from the vendor directory by hand.");
    }
    process::exit(1);
}

/// The files `scripts/update-vendor.sh` copies from the board, as paths
/// relative to the vendor directory. Read from the directory rather
/// than listed here, so a header added to the include closure is
/// checked without touching this file. `SOURCE` is not among them: it
/// is provenance the update script writes, not a copy of anything.
fn vendored_files(vendor: &Path) -> Vec<String> {
    let mut headers: Vec<String> = fs::read_dir(vendor.join("include"))
        .expect("read the vendored include directory")
        .map(|entry| entry.expect("read a vendored header entry").file_name())
        .map(|name| format!("include/{}", name.to_string_lossy()))
        .collect();
    headers.sort();
    headers.push("LICENSE".into());
    headers
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
        return String::from_utf8_lossy(&output.stdout).trim().to_string();
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
    "unknown".to_string()
}

fn fetch(host: &str, rel: &str, scratch: &Path) -> Result<PathBuf, String> {
    let local = scratch.join(rel);
    fs::create_dir_all(local.parent().expect("the fetched path has a parent"))
        .expect("create the scratch directory");
    let output = Command::new("scp")
        .args(SSH_OPTIONS)
        .arg(format!("{host}:{REMOTE_ROOT}/{rel}"))
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
        stderr.trim().to_string()
    } else {
        reported.join("\n")
    }
}

/// The `BELA_*_VERSION` macros, as `1.18.0`. A cheap first signal: it
/// names the drift the way the Bela changelog does, while the content
/// comparison is what actually decides it.
fn version(header: &Path) -> Option<String> {
    let text = fs::read_to_string(header).ok()?;
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

fn diff(vendored: &Path, fetched: &Path, rel: &str, host: &str) -> String {
    let output = Command::new("diff")
        .arg("-u")
        .args(["-L", &format!("vendored {rel}")])
        .args(["-L", &format!("{host}:{REMOTE_ROOT}/{rel}")])
        .args([vendored, fetched])
        .output();
    let text = match output {
        Ok(output) => String::from_utf8_lossy(&output.stdout).into_owned(),
        Err(error) => return format!("(the files differ; could not run diff: {error})"),
    };

    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= MAX_DIFF_LINES {
        return text;
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
