//! Reconfiguring a built binary through Bela's standard command-line
//! options, and adding one of your own.
//!
//! The application asks for 32-frame blocks in its own `Settings`; the
//! command line is applied on top, so `--period 64` wins over it and
//! `setup` reports what the audio system was actually configured with:
//!
//! ```sh
//! ./command_line                  # 32 frames per block
//! ./command_line --period 64 -v   # 64, plus libbela's verbose logging
//! ./command_line --help
//! ```
//!
//! `--help` is this program's own option rather than one of Bela's, so
//! it is handled here before the remaining arguments are handed on —
//! which is how a program adds options of its own: parse them first
//! with whatever argument parser it already uses, and pass the rest to
//! `Bela::run_with_args`.
//!
//! Cross-compile and run on the board (see docs/cross-compile.md):
//!
//! ```sh
//! cargo build -p bela --release --target aarch64-unknown-linux-gnu --example command_line
//! ```

#![cfg_attr(
    not(bela_device),
    allow(
        dead_code,
        reason = "only the fallback main is reachable off-device; the application code should still compile and lint"
    )
)]

#[cfg(bela_device)]
use std::env;
#[cfg(bela_device)]
use std::ffi::OsString;
use std::process::ExitCode;

use bela::{BelaApplication, Context, rt_println};

/// The application's own default block size, which the command line
/// can override.
const PERIOD_SIZE: u32 = 32;

struct Report;

// Safety: setup prints through the real-time safe path and render does
// nothing at all — no allocation, blocking, system calls or panicking
// code paths.
unsafe impl BelaApplication for Report {
    fn setup(&mut self, context: &mut Context) -> bool {
        rt_println!(
            "setup: {} Hz, {} frames per block, {} in / {} out audio channels, \
             {} analog in / {} analog out, {} digital",
            context.audio_sample_rate(),
            context.audio_frames(),
            context.audio_in_channels(),
            context.audio_out_channels(),
            context.analog_in_channels(),
            context.analog_out_channels(),
            context.digital_channels()
        );
        true
    }

    // Silence: this example is about how the audio system was
    // configured, not about what it renders.
    fn render(&mut self, _context: &mut Context) {}
}

#[cfg(bela_device)]
fn usage() {
    eprintln!("Usage: command_line [options]");
    bela::print_usage();
    eprintln!("   --help [-h]:                        Print this menu");
}

#[cfg(bela_device)]
fn main() -> ExitCode {
    let args: Vec<OsString> = env::args_os().collect();
    // Bela's standard options have no `--help`, and anything they do
    // not recognise is an error, so this one is answered before the
    // rest of the arguments go on to `run_with_args`.
    if args
        .iter()
        .skip(1)
        .any(|arg| matches!(arg.to_str(), Some("-h" | "--help")))
    {
        usage();
        return ExitCode::SUCCESS;
    }

    let settings = bela::Settings::new().period_size(PERIOD_SIZE);
    match bela::Bela::run_with_args(Report, &settings, args) {
        Ok(()) => ExitCode::SUCCESS,
        // Returning the error from `main` would print its `Debug`
        // form; the `Display` one says what went wrong. The options are
        // worth repeating when it was the command line that was
        // rejected, and only then.
        Err(error) => {
            eprintln!("Error: {error}");
            if matches!(
                error,
                bela::Error::CommandLine(_) | bela::Error::CommandLineNul
            ) {
                usage();
            }
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(bela_device))]
fn main() -> ExitCode {
    eprintln!("This example must be cross-compiled for Bela Gem (aarch64-unknown-linux-gnu).");
    ExitCode::FAILURE
}
