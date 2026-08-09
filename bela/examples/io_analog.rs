//! Hardware probe for what `analog_read` returns from a real pin.
//!
//! This is the half of #11 that `io_config` could not answer. That probe
//! settled the *shape* of the analog block — how many channels, how many
//! frames, at what rate — from the numbers libbela reports. What it
//! could not say is whether the accessor then reads the pin the caller
//! asked for, and what the 0-to-1 it returns means in volts.
//!
//! # The claim under test
//!
//! `Bela.h` documents `analogRead` as
//!
//! > The returned value ranges from 0 to 1, corresponding to a voltage
//! > range of 0 to 4.096V.
//!
//! which is the ADS8166's internal reference. The board's copy of the
//! header and the one vendored under `bela-sys/vendor` say this word for
//! word, so it is libbela's claim about this hardware and not an
//! inference. What a Gem Stereo did with the wiring below is recorded
//! under "What an analog input reads" in `docs/board-facts.md`.
//!
//! # Wiring, and why it needs no meter
//!
//! ```text
//! A0 <- potentiometer wiper   (ends to 3.3V on P2, and to GND)
//! A1 <- 3.3V   (P2; note that P1's equivalent pin is 5V on a Gem)
//! A2 <- GND
//! A3..A7 left floating
//! ```
//!
//! The two tied channels are the measurement; the potentiometer is only
//! the sanity check around it. A rail is a voltage already known without
//! an instrument, so what `A1` reads divides the two candidate
//! full-scales on its own:
//!
//! - `A1` near **0.806** — full scale is 4.096 V, as the header says
//!   (3.3 / 4.096 = 0.8057).
//! - `A1` near **1.000** — full scale is 3.3 V, and the header does not
//!   describe this board as configured.
//!
//! No third value is plausible from a rail, so the reading decides it.
//! A meter is still worth having on the rail if one is to hand — 3.3 V
//! is nominal, and the ratio above is only as good as it — but the two
//! candidates are 24% apart, which no rail tolerance closes.
//!
//! `A2` at GND is the other end of the same question, and `A3..A7`
//! floating are there to be recognised: a floating input is not a
//! reading, and what it settles at is worth knowing before it is
//! mistaken for one.
//!
//! # What each column decides
//!
//! One line per channel per report window:
//!
//! ```text
//! analog-in: ch0 mean:0.4123 min:0.4109 max:0.4138 spread:0.0006
//! ```
//!
//! - `mean` against a tied channel gives the full scale, as above.
//! - `mean` across channels gives the channel mapping: with this wiring
//!   exactly one channel sits at the rail, one at zero, and one follows
//!   the potentiometer. If the channel that moves is not `ch0`, the
//!   index the accessor takes is not the `A`*n* on the silkscreen.
//! - `min` and `max` over the window give the noise on a held input, and
//!   the range the potentiometer actually covers when turned.
//! - `spread` is the widest range seen *within a single block*, which is
//!   the frame axis rather than time. A DC input spread much wider than
//!   the per-window noise would mean `frame * channels + channel` is
//!   indexing across channels rather than along frames — the layout the
//!   accessor assumes, checked against a signal that should not move
//!   within a block.
//!
//! Run it once with the defaults and once with `--uniform-sample-rate 0`
//! (libbela's own option, see below): that changes the analog frame count
//! per block without changing the wiring, and `spread` is the column that
//! would notice if the frame axis were read wrongly under it.
//!
//! # Options
//!
//! Bela's standard command-line options are accepted and applied on top
//! of the defaults, so the block can be reshaped without rebuilding.
//!
//! Cross-compile and run on the board (see docs/cross-compile.md):
//!
//! ```sh
//! cargo build -p bela --release --target aarch64-unknown-linux-gnu --example io_analog
//! ```

#![cfg_attr(
    not(bela_device),
    allow(
        dead_code,
        reason = "only the fallback main is reachable off-device; the probe code should still compile and lint"
    )
)]

#[cfg(not(bela_device))]
use std::process::ExitCode;

use bela::{BelaApplication, BlockContext, RenderContext, SetupContext, ThreadInfo, rt_println};

/// How many channels are watched at once.
///
/// A Gem Stereo has eight analog inputs and `io_config` found no
/// settings that raise the count, so this is the whole board. A context
/// reporting more is not an error here — the extra channels are left
/// unwatched and said so, rather than growing an allocation on the audio
/// thread.
const MAX_CHANNELS: usize = 8;

/// How long each report covers.
///
/// Long enough that the mean is worth reading and short enough to follow
/// a potentiometer by hand. Held as a duration rather than a block count
/// because the block count depends on the period size, which is an
/// option.
const REPORT_SECONDS: f32 = 1.0;

/// Per-channel accumulators over one report window.
///
/// Kept as arrays rather than a `Vec` of anything: `render_pre` runs on
/// the audio thread, where allocating is what the whole design is
/// arranged to avoid.
struct Watch {
    /// Sum of every sample in the window, in `f64` because a window is
    /// tens of thousands of samples and an `f32` running total loses its
    /// last digits well before the end of one.
    sum: [f64; MAX_CHANNELS],
    min: [f32; MAX_CHANNELS],
    max: [f32; MAX_CHANNELS],
    /// The widest range seen within a single block, per channel — the
    /// frame axis, as opposed to the `min`/`max` pair which is the whole
    /// window.
    spread: [f32; MAX_CHANNELS],
    /// Frames accumulated so far, which is what the mean divides by.
    frames: u32,
    /// Blocks accumulated so far, against `blocks_per_report`.
    blocks: u32,
    /// How many blocks make up [`REPORT_SECONDS`], worked out in `setup`
    /// from the rate and period the audio system actually came up with.
    blocks_per_report: u32,
    /// Channels being watched: the context's count, capped at
    /// [`MAX_CHANNELS`].
    channels: usize,
}

impl Watch {
    const fn new() -> Self {
        Self {
            sum: [0.0; MAX_CHANNELS],
            min: [f32::INFINITY; MAX_CHANNELS],
            max: [f32::NEG_INFINITY; MAX_CHANNELS],
            spread: [0.0; MAX_CHANNELS],
            frames: 0,
            blocks: 0,
            blocks_per_report: 0,
            channels: 0,
        }
    }

    /// Empties the accumulators for the next window.
    const fn reset(&mut self) {
        self.sum = [0.0; MAX_CHANNELS];
        self.min = [f32::INFINITY; MAX_CHANNELS];
        self.max = [f32::NEG_INFINITY; MAX_CHANNELS];
        self.spread = [0.0; MAX_CHANNELS];
        self.frames = 0;
        self.blocks = 0;
    }

    /// One line per channel, then start the next window.
    ///
    /// One key per line because `rt_println!` caps a message at
    /// `MESSAGE_CAPACITY` bytes, and a truncated line would read as a
    /// missing column rather than as a lost one.
    fn report(&mut self) {
        // A window with no frames in it would divide by zero, and it is
        // a finding of its own: an audio system that ran without ever
        // presenting an analog frame.
        if self.frames == 0 {
            rt_println!("analog-in: window had no analog frames");
            self.reset();
            return;
        }
        let frames = f64::from(self.frames);
        for channel in 0..self.channels {
            rt_println!(
                "analog-in: ch{} mean:{:.4} min:{:.4} max:{:.4} spread:{:.4}",
                channel,
                self.sum[channel] / frames,
                self.min[channel],
                self.max[channel],
                self.spread[channel]
            );
        }
        self.reset();
    }
}

impl BelaApplication for Watch {
    type RenderState = ();

    fn setup(&mut self, context: &SetupContext) -> bool {
        // `println!`, not `rt_println!`: `setup` runs inside
        // `Bela_initAudio` before there is an audio thread, and a
        // flushed write is what an operator reading this over ssh needs
        // before the per-window lines start.
        println!(
            "analog-in: shape=channels:{},frames:{},rate:{},audio-frames:{},audio-rate:{}",
            context.analog_in_channels(),
            context.analog_frames(),
            context.analog_sample_rate(),
            context.audio_frames(),
            context.audio_sample_rate()
        );

        self.channels = context.analog_in_channels().min(MAX_CHANNELS);
        if self.channels < context.analog_in_channels() {
            println!(
                "analog-in: watching the first {} of {} channels",
                self.channels,
                context.analog_in_channels()
            );
        }
        if self.channels == 0 {
            // Refusing here rather than running silently: with analog
            // off there is nothing this probe can say, and an audio
            // system that comes up anyway invites reading its empty
            // output as a measurement.
            println!("analog-in: no analog inputs; nothing to probe");
            return false;
        }

        // Blocks rather than a clock, because the report happens on the
        // audio thread where the block count is the time that is already
        // being kept. At least one, so that a period longer than the
        // window still reports.
        #[allow(
            clippy::cast_precision_loss,
            reason = "a period size is a few hundred frames at most"
        )]
        let blocks_per_second = context.audio_sample_rate() / context.audio_frames() as f32;
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "blocks per second times a one-second window is a small positive number"
        )]
        let blocks_per_report = (blocks_per_second * REPORT_SECONDS) as u32;
        self.blocks_per_report = blocks_per_report.max(1);
        true
    }

    fn create_render_state(&mut self, _thread: ThreadInfo, _context: &SetupContext) {}

    fn render_pre(&mut self, _states: &mut [()], context: &mut BlockContext) {
        // `render_pre` and not `render`: the analog inputs are the whole
        // block's, so reading them once here is both cheaper and
        // independent of how many render threads the block was split
        // across.
        let channels = self.channels;
        // This block's own extremes, gathered in the same pass as the
        // window's. Stack arrays rather than fields: they start again
        // at every block, and a field would have to be cleared here
        // anyway.
        let mut block_low = [f32::INFINITY; MAX_CHANNELS];
        let mut block_high = [f32::NEG_INFINITY; MAX_CHANNELS];
        for frame in 0..context.analog_frames() {
            for channel in 0..channels {
                let value = context.analog_read(frame, channel);
                self.sum[channel] += f64::from(value);
                self.min[channel] = self.min[channel].min(value);
                self.max[channel] = self.max[channel].max(value);
                // Separate accumulators for the block, because the two
                // answer different questions: these are the frame axis
                // within one block, those are the whole window.
                block_low[channel] = block_low[channel].min(value);
                block_high[channel] = block_high[channel].max(value);
            }
        }

        // The widest within-block range seen so far, per channel. A
        // block with no frames leaves the sentinels untouched and
        // contributes nothing.
        for channel in 0..channels {
            if block_high[channel] >= block_low[channel] {
                self.spread[channel] =
                    self.spread[channel].max(block_high[channel] - block_low[channel]);
            }
        }

        #[allow(
            clippy::cast_possible_truncation,
            reason = "an analog frame count is a few hundred at most"
        )]
        let frames = context.analog_frames() as u32;
        self.frames += frames;
        self.blocks += 1;
        if self.blocks >= self.blocks_per_report {
            self.report();
        }
    }

    // Silent: this probe reads inputs and writes no sample anywhere.
    fn render(&self, _state: &mut (), _context: &mut RenderContext) {}
}

#[cfg(bela_device)]
fn main() -> Result<(), bela::Error> {
    use std::env::args_os;

    // Bela's own options on top of the defaults, so that the analog
    // frame count can be changed — `--uniform-sample-rate 0` above —
    // without rebuilding and redeploying.
    bela::Bela::run_with_args(Watch::new(), &bela::Settings::new(), args_os())
}

#[cfg(not(bela_device))]
fn main() -> ExitCode {
    eprintln!("This example must be cross-compiled for Bela Gem (aarch64-unknown-linux-gnu).");
    ExitCode::FAILURE
}
