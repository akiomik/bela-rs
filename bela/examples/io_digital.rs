//! Hardware probe for what the digital accessors do to a real pin.
//!
//! The other half of #11, and the companion to `io_analog`. The digital
//! bit layout the accessors implement — bit *n* is the direction of
//! channel *n* with 1 meaning input, bit *n*+16 is its value — is what
//! `Bela.h`'s inline helpers do, so it is not guesswork. What has never
//! been checked is that a pin then behaves the way that layout says: a
//! channel set to output actually drives, a channel set to input
//! actually follows what is driven into it, and the index in the call is
//! the number on the silkscreen.
//!
//! # Wiring
//!
//! ```text
//! D0 --[ 1k ]-- D1        loopback: D0 drives, D1 reads
//! D2 --[ 1k ]-- LED --- GND    (a red, yellow or green one: at 3.3V a
//!                               blue or white LED barely lights)
//! ```
//!
//! The series resistor on the loopback is insurance against the very
//! thing being measured. If the direction bit means the opposite of what
//! the accessors assume, both pins end up as outputs and one of them
//! drives against the other; 1 kΩ makes that 3.3 mA instead of a short.
//! It costs nothing when the test passes — a digital input draws
//! essentially no current, so there is no voltage across it.
//!
//! # How the latency is measured
//!
//! The interesting number is not whether the loopback works but *when*
//! it works. libbela fills the digital input buffer from the PRU before
//! `render` is called and applies the output buffer after it returns, so
//! a value written in one block cannot appear on the input in that same
//! block. How much later it appears is a property of the board that no
//! header documents.
//!
//! So the output is toggled once every [`WRITE_PERIOD_BLOCKS`] blocks,
//! at frame [`WRITE_FRAME`] rather than at the start of a block, and
//! every frame of the input channel is scanned for the edge. The
//! distance between the two, counted in digital frames, is the latency.
//! Writing at a frame in the middle of the block is what makes the
//! measurement finer than "some number of blocks".
//!
//! Toggling every few blocks rather than every block also tests
//! persistence for free. `Bela.h` says digital pins always persist, so
//! one write should produce exactly one edge; a pin that reverted
//! between blocks would show up as extra edges, which are counted
//! separately as `unexpected`.
//!
//! # What each field decides
//!
//! ```text
//! digital: initial-word=0x0000ffff
//! digital: after-pin-mode=0x0000fffa
//! digital: edges:344 misses:0 unexpected:0 latency-frames:33..33
//! digital: out-readback:true level:true in-now:true
//! ```
//!
//! That is a real Gem Stereo at the default period; what it means is
//! under "What a digital pin does" in `docs/board-facts.md`.
//!
//! - `initial-word` is the digital word as the first block presents it,
//!   before this probe touches anything. With bits 0-15 set it says
//!   every channel starts as an input, which is what `PinMode::default()`
//!   claims.
//! - `after-pin-mode` is the same word once `D0` and `D2` have been made
//!   outputs and `D1` left an input. Their direction bits should have
//!   gone to 0 and nothing else should have moved.
//! - `edges` against `misses` is the loopback itself: a miss is a write
//!   that produced no edge before the next one, which is what a channel
//!   that does not drive, or an index that is not the pin on the
//!   silkscreen, looks like.
//! - `unexpected` counts edges nobody asked for — noise on an
//!   undriven pin, or an output that did not persist.
//! - `latency-frames` is the write-to-read distance described above.
//! - `out-readback` is what `digital_read` returns for the channel just
//!   written. The accessor reads the same value bit `digital_write`
//!   sets, so this should echo `level` rather than report the pin.
//!
//! The LED on `D2` changes state once per report line. It confirms the
//! channel-to-pin mapping the way no printed number can: if the LED
//! blinking is not the one wired to the third digital pin, the index is
//! not the silkscreen's.
//!
//! Cross-compile and run on the board (see docs/cross-compile.md):
//!
//! ```sh
//! cargo build -p bela --release --target aarch64-unknown-linux-gnu --example io_digital
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

use bela::{
    BelaApplication, BlockContext, PinMode, RenderContext, SetupContext, ThreadInfo, rt_println,
};

/// The channel that drives the loopback.
const OUT_CHANNEL: usize = 0;
/// The channel that reads it.
const IN_CHANNEL: usize = 1;
/// The channel with the LED on it, for confirming the mapping by eye.
const LED_CHANNEL: usize = 2;

/// Which frame of the block the output is toggled at.
///
/// Deliberately not 0: an edge written at the start of a block could
/// only ever be timed to the block, and the point is to time it to the
/// frame. Clamped to the block if the period is smaller than this.
const WRITE_FRAME: usize = 4;

/// How many blocks pass between one toggle and the next.
///
/// Long enough that an edge has somewhere to land before the next write
/// muddies it, short enough that a second of running still produces
/// hundreds of them. At 16 frames and 44100 Hz this is about 2.9 ms.
const WRITE_PERIOD_BLOCKS: u32 = 8;

/// How long each report covers.
const REPORT_SECONDS: f32 = 1.0;

#[allow(
    clippy::struct_excessive_bools,
    reason = "each one is an independent one-bit fact about the loopback, and grouping them into a state enum would only hide which are true together"
)]
struct Loopback {
    /// Whether the pin directions have been set. Done from the first
    /// block rather than from `setup` so that the word can be reported
    /// as it arrives, untouched.
    configured: bool,

    /// Digital frames since the run started, which is the clock the
    /// latency is measured against.
    frame_clock: u64,
    /// The frame the outstanding write happened at, on that clock.
    write_at: u64,
    /// Whether a write is still waiting for its edge.
    awaiting: bool,

    /// The level last written to [`OUT_CHANNEL`].
    level: bool,
    /// The level last read from [`IN_CHANNEL`], so that an edge is a
    /// change from it rather than from the start of the block.
    last_seen: bool,

    blocks: u32,
    blocks_per_report: u32,
    blocks_since_write: u32,

    // Statistics for the window in progress.
    edges: u32,
    misses: u32,
    unexpected: u32,
    min_latency: u64,
    max_latency: u64,
    /// What `digital_read` returned for the output channel at the last
    /// write.
    out_readback: bool,

    /// The state of the LED channel, changed once per report.
    led: bool,
}

impl Loopback {
    const fn new() -> Self {
        Self {
            configured: false,
            frame_clock: 0,
            write_at: 0,
            awaiting: false,
            level: false,
            last_seen: false,
            blocks: 0,
            blocks_per_report: 0,
            blocks_since_write: 0,
            edges: 0,
            misses: 0,
            unexpected: 0,
            min_latency: u64::MAX,
            max_latency: 0,
            out_readback: false,
            led: false,
        }
    }

    /// Empties the statistics for the next window, leaving the loopback
    /// state — the clock, the level, the outstanding write — alone.
    const fn reset(&mut self) {
        self.blocks = 0;
        self.edges = 0;
        self.misses = 0;
        self.unexpected = 0;
        self.min_latency = u64::MAX;
        self.max_latency = 0;
    }

    /// One window's findings, then the LED changes state.
    fn report(&mut self, context: &mut BlockContext) {
        // A window with no edge in it has no latency to report, and
        // saying so beats printing the sentinel as if it were a
        // measurement.
        if self.edges == 0 {
            rt_println!(
                "digital: edges:0 misses:{} unexpected:{} latency-frames:none",
                self.misses,
                self.unexpected
            );
        } else {
            rt_println!(
                "digital: edges:{} misses:{} unexpected:{} latency-frames:{}..{}",
                self.edges,
                self.misses,
                self.unexpected,
                self.min_latency,
                self.max_latency
            );
        }
        rt_println!(
            "digital: out-readback:{} level:{} in-now:{}",
            self.out_readback,
            self.level,
            self.last_seen
        );

        // Slow enough to see, and tied to the report so that what the
        // eye sees and what the log says cannot drift apart.
        self.led = !self.led;
        context.digital_write(0, LED_CHANNEL, self.led);

        self.reset();
    }
}

impl BelaApplication for Loopback {
    type RenderState = ();

    fn setup(&mut self, context: &SetupContext) -> bool {
        // `println!`, not `rt_println!`: `setup` runs inside
        // `Bela_initAudio`, before there is an audio thread.
        println!(
            "digital: shape=channels:{},frames:{},rate:{},audio-frames:{}",
            context.digital_channels(),
            context.digital_frames(),
            context.digital_sample_rate(),
            context.audio_frames()
        );

        // Refusing rather than running: with too few channels the
        // accessors below would panic on the audio thread, and with
        // none at all there is nothing to probe.
        let needed = LED_CHANNEL + 1;
        if context.digital_channels() < needed || context.digital_frames() == 0 {
            println!("digital: need {needed} digital channels and a frame to use them in");
            return false;
        }

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
        let frames = context.digital_frames();
        // Clamped, so that a period shorter than the intended write
        // point still writes inside the block it belongs to.
        let write_frame = WRITE_FRAME.min(frames - 1);

        if !self.configured {
            // The word before anything has been said to it: this is
            // where "every pin starts as an input" is either true or
            // not.
            rt_println!("digital: initial-word=0x{:08x}", context.digital()[0]);
            context.pin_mode(0, OUT_CHANNEL, PinMode::Output);
            context.pin_mode(0, IN_CHANNEL, PinMode::Input);
            context.pin_mode(0, LED_CHANNEL, PinMode::Output);
            rt_println!("digital: after-pin-mode=0x{:08x}", context.digital()[0]);
            self.last_seen = context.digital_read(0, IN_CHANNEL);
            self.configured = true;
        }

        // Read before writing. The input frames in this block were
        // sampled before the callback ran, so they can only carry the
        // consequences of earlier blocks; writing first would not change
        // them, but it would make the order of the two look like it
        // mattered.
        for frame in 0..frames {
            let value = context.digital_read(frame, IN_CHANNEL);
            if value == self.last_seen {
                continue;
            }
            self.last_seen = value;
            if self.awaiting {
                let latency = self.frame_clock + frame as u64 - self.write_at;
                self.min_latency = self.min_latency.min(latency);
                self.max_latency = self.max_latency.max(latency);
                self.edges += 1;
                self.awaiting = false;
            } else {
                // An edge with no write behind it: noise on the pin, or
                // an output that did not hold its value.
                self.unexpected += 1;
            }
        }

        self.blocks_since_write += 1;
        if self.blocks_since_write >= WRITE_PERIOD_BLOCKS {
            if self.awaiting {
                // The previous write never arrived. Counted and dropped,
                // so that its edge cannot later be attributed to this
                // one.
                self.misses += 1;
            }
            self.level = !self.level;
            context.digital_write(write_frame, OUT_CHANNEL, self.level);
            // Read back the channel just written. `digital_write` sets
            // the value bit and `digital_read` reads it, so this says
            // what the buffer holds rather than what the pin is doing.
            self.out_readback = context.digital_read(write_frame, OUT_CHANNEL);
            self.write_at = self.frame_clock + write_frame as u64;
            self.awaiting = true;
            self.blocks_since_write = 0;
        }

        self.frame_clock += frames as u64;
        self.blocks += 1;
        if self.blocks >= self.blocks_per_report {
            self.report(context);
        }
    }

    // Everything here is the whole block's, so `render_pre` does it all.
    fn render(&self, _state: &mut (), _context: &mut RenderContext) {}
}

#[cfg(bela_device)]
fn main() -> Result<(), bela::Error> {
    use std::env::args_os;

    bela::Bela::run_with_args(Loopback::new(), &bela::Settings::new(), args_os())
}

#[cfg(not(bela_device))]
fn main() -> ExitCode {
    eprintln!("This example must be cross-compiled for Bela Gem (aarch64-unknown-linux-gnu).");
    ExitCode::FAILURE
}
