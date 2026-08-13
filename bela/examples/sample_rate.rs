//! Hardware probe for `Settings::audio_sample_rate`, exercised through
//! the setter rather than through `--sample-rate`.
//!
//! #109's rate ladder (`scripts/probe-command-line.sh`, recorded in
//! `docs/board-facts.md`) drove `BelaInitSettings::audioSampleRate`
//! through Bela's own `-r` parser. This drives the same field through
//! `Settings::audio_sample_rate` and `Bela::new` instead, with no
//! command line involved at all, so a divergence between the two paths
//! — a wrong field, a lossy `NonZeroU32` -> `f32` conversion — would
//! show up as a rate this reports differently from the one asked for,
//! rather than being assumed identical because both end up in the same
//! C struct.
//!
//! Takes the requested rate in Hz as its one argument, of this
//! program's own — not one of Bela's standard options — so
//! `Bela::new` is used rather than `Bela::run_with_args`. `setup`
//! prints what the audio system was actually configured with; `cleanup`
//! reports how many blocks ran, which is what tells a rate the board
//! honoured from one it accepted and ignored (see `command_line.rs`).
//!
//! One rate per process, the way `probe-command-line.sh` runs its
//! ladder: a rate past the board's ceiling aborts the process from
//! inside the codec, and there is no call this crate can make afterwards
//! that would run under it.
//!
//! ```sh
//! cargo build -p bela --release --target aarch64-unknown-linux-gnu --example sample_rate
//! ./sample_rate 96000
//! ```

#![cfg_attr(
    not(bela_device),
    allow(
        dead_code,
        reason = "only the fallback main is reachable off-device; the application code should still compile and lint"
    )
)]

use std::process::ExitCode;

use bela::{
    BelaApplication, BlockContext, CleanupContext, RenderContext, SetupContext, ThreadInfo,
    rt_println,
};

#[derive(Default)]
struct Report {
    blocks: u64,
}

impl BelaApplication for Report {
    type RenderState = ();

    fn setup(&mut self, context: &SetupContext) -> bool {
        rt_println!(
            "setup: {} Hz, {} frames per block, analog {} Hz, digital {} Hz",
            context.audio_sample_rate(),
            context.audio_frames(),
            context.analog_sample_rate(),
            context.digital_sample_rate()
        );
        true
    }

    fn create_render_state(&mut self, _thread: ThreadInfo, _context: &SetupContext) {}

    fn render(&self, _state: &mut (), _context: &mut RenderContext) {}

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
fn main() -> ExitCode {
    use core::num::NonZeroU32;
    use std::env::args;

    let Some(hz) = args().nth(1) else {
        eprintln!("usage: sample_rate <hz>");
        return ExitCode::FAILURE;
    };
    let Ok(hz) = hz.parse::<u32>().map(NonZeroU32::try_from) else {
        eprintln!("sample_rate takes a rate in Hz, not {hz:?}");
        return ExitCode::FAILURE;
    };
    let Ok(hz) = hz else {
        eprintln!("the requested rate must not be 0");
        return ExitCode::FAILURE;
    };

    let settings = bela::Settings::new().audio_sample_rate(hz);
    match bela::Bela::run(Report::default(), &settings) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(bela_device))]
fn main() -> ExitCode {
    eprintln!("This example must be cross-compiled for Bela Gem (aarch64-unknown-linux-gnu).");
    ExitCode::FAILURE
}
