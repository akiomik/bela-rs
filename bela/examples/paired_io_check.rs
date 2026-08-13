//! Confirms on real hardware that `PairedIo`'s `audio_io().frames()`
//! path (#110) copies the same samples the indexed `audio_read` /
//! `audio_write` path does — the two ways to write `passthrough.rs`.
//!
//! # The claim under test
//!
//! Every block, `render` first fills the output with the indexed path,
//! exactly as `passthrough.rs` does. It then walks `audio_io().frames()`
//! over the same channels: for each sample, it reads what the indexed
//! path just wrote (still sitting in the output buffer), compares it
//! bit-for-bit against the sample `frames()` pairs with it on the input
//! side, and only then overwrites it. A mismatch means the two paths
//! disagree about which input sample belongs to which output sample —
//! exactly the off-by-partition mistake `frames()` exists to rule out
//! (see the type documentation on `PairedIo`).
//!
//! No known input signal is needed: whatever is on the analog/audio
//! inputs, both paths read the same live buffer, so agreement is a
//! property of the two accessors, not of the signal. `cleanup` reports
//! `checked` and `mismatches`; a correct build never sees the second
//! move off zero.
//!
//! Cross-compile and run on the board (see docs/cross-compile.md):
//!
//! ```sh
//! cargo build -p bela --release --target aarch64-unknown-linux-gnu --example paired_io_check
//! ```

#![cfg_attr(
    not(bela_device),
    allow(
        dead_code,
        reason = "only the fallback main is reachable off-device; the application code should still compile and lint"
    )
)]

use core::sync::atomic::{AtomicU64, Ordering};
#[cfg(not(bela_device))]
use std::process::ExitCode;

use bela::{BelaApplication, CleanupContext, RenderContext, SetupContext, ThreadInfo, rt_println};

#[derive(Default)]
struct PairedIoCheck {
    checked: AtomicU64,
    mismatches: AtomicU64,
}

impl BelaApplication for PairedIoCheck {
    type RenderState = ();

    fn create_render_state(&mut self, _thread: ThreadInfo, _context: &SetupContext) {}

    fn render(&self, _state: &mut (), context: &mut RenderContext) {
        let channels = context
            .audio_in_channels()
            .min(context.audio_out_channels());
        let range = context.audio_frame_range();

        // Ground truth: the indexed path, exactly as `passthrough.rs`
        // uses it.
        for frame in range {
            for channel in 0..channels {
                let sample = context.audio_read(frame, channel);
                context.audio_write(frame, channel, sample);
            }
        }

        // The paired path, over the same channels — compared against
        // what the indexed path already wrote, before it overwrites it.
        let mut io = context.audio_io();
        for (input, output) in io.frames() {
            for channel in 0..channels {
                let expected = output[channel];
                let paired = input[channel];
                self.checked.fetch_add(1, Ordering::Relaxed);
                if paired.to_bits() != expected.to_bits() {
                    self.mismatches.fetch_add(1, Ordering::Relaxed);
                }
                output[channel] = paired;
            }
        }
    }

    fn cleanup(&mut self, _states: &mut [()], _context: &CleanupContext) {
        rt_println!(
            "paired_io_check: checked={} mismatches={}",
            self.checked.load(Ordering::Relaxed),
            self.mismatches.load(Ordering::Relaxed)
        );
    }
}

#[cfg(bela_device)]
fn main() -> Result<(), bela::Error> {
    bela::Bela::run(PairedIoCheck::default(), &bela::Settings::new())
}

#[cfg(not(bela_device))]
fn main() -> ExitCode {
    eprintln!("This example must be cross-compiled for Bela Gem (aarch64-unknown-linux-gnu).");
    ExitCode::FAILURE
}
