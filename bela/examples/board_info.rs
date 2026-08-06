//! Prints what this binary is running on: the board, and the Bela
//! version.
//!
//! The first thing to ask for from anyone reporting a problem. It
//! brings no audio system up and touches no audio hardware, so it can
//! be run on a board that is already doing something else, and it
//! answers even when nothing else here works — an image whose libbela
//! is not the one the binary was built against says so in the first two
//! lines rather than through whatever failure that mismatch causes
//! later.
//!
//! ```text
//! board: GemStereo
//! version: 1.18.0
//! ```
//!
//! The version line names both versions when the library and the
//! vendored headers disagree, and only the one when they do not.
//!
//! With `--all-modes` it asks each detect mode in turn instead, which
//! is how the difference between them was measured. `scan` is in that
//! list and is the one mode with a side effect: it goes out over the
//! buses and writes `/run/bela/belaconfig`, so it needs permission to
//! write that file and is not what a program should call routinely.
//!
//! Cross-compile and run on the board (see docs/cross-compile.md):
//!
//! ```sh
//! cargo build -p bela --release --target aarch64-unknown-linux-gnu --example board_info
//! ```

use std::process::ExitCode;

#[cfg(bela_device)]
fn main() -> ExitCode {
    use std::env::args;

    use bela::{Board, DetectMode, Version};

    let arguments: Vec<String> = args().skip(1).collect();
    let all_modes = match arguments.as_slice() {
        [] => false,
        [flag] if flag == "--all-modes" => true,
        _ => {
            eprintln!(
                "usage: board_info [--all-modes]\n\
                 \x20 --all-modes  ask every detect mode, including the scan that writes\n\
                 \x20              /run/bela/belaconfig"
            );
            return ExitCode::FAILURE;
        }
    };

    if all_modes {
        // Named per line, because the interesting result is the one
        // that disagrees with the others.
        for mode in DetectMode::ALL {
            println!("board[{mode}]: {}", Board::detect(mode));
        }
    } else {
        // `Cache` rather than `Scan`: on a running board the daemon has
        // already written the file, and a diagnostic should not change
        // the thing it is reporting on.
        println!("board: {}", Board::detect(DetectMode::Cache));
    }

    // Both versions on one line. They agree on a board whose image is
    // the one the bindings were vendored from, and the whole point of
    // printing them together is the run where they do not.
    let running = Version::running();
    if running == Version::HEADERS {
        println!("version: {running}");
    } else {
        println!(
            "version: {running} (this binary was built against {headers})",
            headers = Version::HEADERS
        );
    }

    ExitCode::SUCCESS
}

#[cfg(not(bela_device))]
fn main() -> ExitCode {
    eprintln!("This example must be cross-compiled for Bela Gem (aarch64-unknown-linux-gnu).");
    ExitCode::FAILURE
}
