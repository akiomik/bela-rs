//! Reconfiguring a built binary through Bela's standard command-line
//! options, and adding one of your own.
//!
//! The application asks for 32-frame blocks and for libbela's LEDs in
//! its own `Settings`; the command line is applied on top, so
//! `--period 64` wins over the first and `--disable-led` over the
//! second:
//!
//! ```sh
//! ./command_line                  # 32 frames per block, LEDs on
//! ./command_line --period 64 -v   # 64, plus libbela's verbose logging
//! ./command_line --disable-led    # the LEDs the application asked for, off
//! ./command_line --help
//! ```
//!
//! The two are reported from different places, because they resolve at
//! different times to different audiences. `setup` reports what the
//! audio system was actually configured with, which is the block size
//! after libbela has had it; `validate_settings` reports the LED
//! setting, which is a request the hardware never answers — nothing in
//! `SetupContext` says whether the indicators are on, so the resolved
//! settings are the last place it can be read.
//!
//! `--help` is this program's own option rather than one of Bela's, so
//! it is handled here before the remaining arguments are handed on —
//! which is how a program adds options of its own: parse them first
//! with whatever argument parser it already uses, and pass the rest to
//! `Bela::run_with_args`.
//!
//! `cleanup` reports how many blocks the run rendered, because what
//! `setup` prints is what libbela says it configured rather than what
//! the hardware then did. An option libbela accepts and the board
//! ignores looks identical in the `setup` line and different in the
//! block count: blocks divided by the seconds the run lasted is the
//! rate the audio thread actually ran at, against the sample rate and
//! block size `setup` reported. That is how `-r` was measured for
//! `docs/board-facts.md`.
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

use bela::{
    BelaApplication, BlockContext, CleanupContext, RenderContext, ResolvedSettings, SetupContext,
    ThreadInfo, rt_println,
};

/// The application's own default block size, which the command line
/// can override.
const PERIOD_SIZE: u32 = 32;

/// Whether the application asks for libbela's running and underrun
/// LEDs. Bela's own default too, and said out loud so that
/// `--disable-led` has an explicit request to override rather than a
/// default to agree with.
const ENABLE_LED: bool = true;

#[derive(Default)]
struct Report {
    blocks: u64,
}

impl BelaApplication for Report {
    type RenderState = ();

    // Runs on the calling thread before any audio exists, so an
    // ordinary `println!` is fine here; `setup` below is the one that
    // prints from inside `Bela_initAudio`.
    fn validate_settings(&self, settings: &ResolvedSettings<'_>) -> Result<(), &'static str> {
        println!("settings: enable_led={}", settings.enable_led());
        Ok(())
    }

    fn setup(&mut self, context: &SetupContext) -> bool {
        rt_println!(
            "setup: {} Hz, {} frames per block, {} in / {} out audio channels, \
             {} analog in / {} analog out, {} digital, {} render thread(s), \
             analog {} Hz, digital {} Hz",
            context.audio_sample_rate(),
            context.audio_frames(),
            context.audio_in_channels(),
            context.audio_out_channels(),
            context.analog_in_channels(),
            context.analog_out_channels(),
            context.digital_channels(),
            context.thread_count(),
            context.analog_sample_rate(),
            context.digital_sample_rate()
        );
        true
    }

    fn create_render_state(&mut self, _thread: ThreadInfo, _context: &SetupContext) {}

    // Silence: this example is about how the audio system was
    // configured, not about what it renders.
    fn render(&self, _state: &mut (), _context: &mut RenderContext) {}

    // Counted in `render_post` rather than in `render`: a block is one
    // block however many threads rendered it, and this runs once per
    // block on the main audio thread with `&mut self`.
    fn render_post(&mut self, _states: &mut [()], _context: &mut BlockContext) {
        self.blocks += 1;
    }

    fn cleanup(&mut self, _states: &mut [()], context: &CleanupContext) {
        rt_println!(
            "cleanup: {} blocks, {} frames elapsed, {} underruns",
            self.blocks,
            context.audio_frames_elapsed(),
            context.underrun_count()
        );
    }
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

    let settings = bela::Settings::new()
        .period_size(PERIOD_SIZE)
        .enable_led(ENABLE_LED);
    match bela::Bela::run_with_args(Report::default(), &settings, args) {
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
