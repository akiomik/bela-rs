//! Sets the codec's levels and gain before audio starts, then plays a
//! sine tone through them.
//!
//! The levels are the codec's own analogue volume controls, so they are
//! set on the `Bela` handle rather than in `render`: the line out
//! level, the headphone level and the gain of the preamplifier ahead of
//! the ADC. `Bela::new` brings the audio system up, the levels are set
//! while it is not yet running, and `until_stopped` then runs it the
//! way `Bela::run` would — that window is what `until_stopped` exists
//! for.
//!
//! The summary line reports what each call returned, including one for
//! a channel no Bela codec has, which is refused rather than quietly
//! ignored.
//!
//! Cross-compile and run on the board (see docs/cross-compile.md):
//!
//! ```sh
//! cargo build -p bela --release --target aarch64-unknown-linux-gnu --example levels
//! ```

#![cfg_attr(
    not(bela_device),
    allow(
        dead_code,
        reason = "only the fallback main is reachable off-device; the application code should still compile and lint"
    )
)]

use core::f32::consts::TAU;
#[cfg(not(bela_device))]
use std::process::ExitCode;

use bela::{BelaApplication, Context};

const FREQUENCY: f32 = 440.0;
const AMPLITUDE: f32 = 0.3;

/// Attenuates the line out, so the tone comes out quieter than the
/// signal `render` writes without `render` knowing anything about it.
const LINE_OUT_LEVEL: f32 = -12.0;
/// Bela's own default is -6 dB; this one is quieter still, because a
/// 440 Hz tone in headphones is not a pleasant surprise.
const HEADPHONE_LEVEL: f32 = -20.0;
/// The default is 16 dB. Higher suits a quiet source, such as a
/// microphone straight into the audio input.
const INPUT_GAIN: f32 = 30.0;
/// No Bela codec has this many channels; the call has to be refused.
const MISSING_CHANNEL: usize = 64;

struct Sine {
    phase: f32,
    phase_increment: f32,
}

impl Sine {
    const fn new() -> Self {
        Self {
            phase: 0.0,
            phase_increment: 0.0,
        }
    }
}

// Safety: render only does arithmetic and writes to the context
// buffers — no allocation, blocking, system calls or panicking code
// paths. The levels are set from the main thread, before audio starts.
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

/// How a call went, as one word for the summary line.
#[cfg(bela_device)]
fn outcome(result: Result<(), bela::Error>) -> String {
    match result {
        Ok(()) => "ok".to_owned(),
        Err(error) => format!("failed({error})"),
    }
}

#[cfg(bela_device)]
fn main() -> Result<(), bela::Error> {
    use bela::{Bela, Channel, Settings};

    // Audio exists but is not running yet: the codec remembers what it
    // is told here and applies it when the audio thread starts.
    let mut bela = Bela::new(Sine::new(), &Settings::new())?;
    let line_out = outcome(bela.set_line_out_level(Channel::All, LINE_OUT_LEVEL));
    let headphone = outcome(bela.set_headphone_level(Channel::All, HEADPHONE_LEVEL));
    let input_gain = outcome(bela.set_audio_input_gain(Channel::All, INPUT_GAIN));
    // A Bela Gem Stereo has no speaker amplifier mute pin, so this
    // succeeds without doing anything; see `docs/board-facts.md`.
    let unmuted = outcome(bela.mute_speakers(false));
    let missing = match bela.set_line_out_level(Channel::One(MISSING_CHANNEL), LINE_OUT_LEVEL) {
        Err(bela::Error::LineOutLevel(_)) => "refused".to_owned(),
        Ok(()) => "accepted".to_owned(),
        Err(error) => format!("other-error({error})"),
    };
    println!(
        "levels: line-out={line_out} headphone={headphone} input-gain={input_gain} \
         unmute={unmuted} missing-channel={missing}"
    );

    // `Bela::run` without the construction, so the levels above could
    // be set in between.
    bela.until_stopped()
}

#[cfg(not(bela_device))]
fn main() -> ExitCode {
    eprintln!("This example must be cross-compiled for Bela Gem (aarch64-unknown-linux-gnu).");
    ExitCode::FAILURE
}
