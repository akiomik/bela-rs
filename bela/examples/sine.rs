//! Plays a 440 Hz sine tone on all audio output channels.
//!
//! Cross-compile and run on the board (see docs/cross-compile.md):
//!
//! ```sh
//! cargo build -p bela --release --target aarch64-unknown-linux-gnu --example sine
//! ```

// On non-device targets only the fallback main is reachable; keep the
// application code compiling (and linted) without dead-code noise.
#![cfg_attr(not(bela_device), allow(dead_code))]

use core::f32::consts::TAU;

use bela::{BelaApplication, Context};

const FREQUENCY: f32 = 440.0;
const AMPLITUDE: f32 = 0.3;

struct Sine {
    phase: f32,
    phase_increment: f32,
}

impl Sine {
    fn new() -> Self {
        Sine {
            phase: 0.0,
            phase_increment: 0.0,
        }
    }
}

// Safety: render only does arithmetic and writes to the context
// buffers — no allocation, blocking, system calls or panicking code
// paths.
unsafe impl BelaApplication for Sine {
    fn setup(&mut self, context: &mut Context) -> bool {
        self.phase_increment = TAU * FREQUENCY / context.audio_sample_rate();
        true
    }

    fn render(&mut self, context: &mut Context) {
        for frame in 0..context.audio_frames() {
            let sample = AMPLITUDE * self.phase.sin();
            for channel in 0..context.audio_out_channels() {
                context.audio_write(frame, channel, sample);
            }
            self.phase += self.phase_increment;
            if self.phase >= TAU {
                self.phase -= TAU;
            }
        }
    }
}

#[cfg(bela_device)]
fn main() -> Result<(), bela::Error> {
    bela::Bela::run(Sine::new(), &bela::Settings::new())
}

#[cfg(not(bela_device))]
fn main() {
    eprintln!("This example must be cross-compiled for Bela Gem (aarch64-unknown-linux-gnu).");
    std::process::exit(1);
}
