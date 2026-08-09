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
//! So the output is toggled once every `WRITE_PERIOD_BLOCKS` blocks,
//! at frame `WRITE_FRAME` rather than at the start of a block, and
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
//! # The split mode
//!
//! `--split` moves the writing from `render_pre` to `render`, where
//! every render thread runs at once and each one owns a contiguous
//! share of the block. Each thread then holds *its own share* at its
//! own level — even threads low, odd threads high — so with two threads
//! the block is low over the first half and high over the second.
//!
//! That makes the edge position the measurement. `RenderContext`'s
//! `digital_write` is documented as stopping at the end of this
//! thread's range rather than at the end of the block, which is where
//! it departs from the C helper it wraps; if it ran to the end of the
//! block instead, the last thread to finish would take the tail and the
//! edge would wander from block to block. A fixed edge, one frame past
//! the boundary between the shares, is that documentation confirmed on
//! hardware.
//!
//! ```text
//! digital: split threads:4 frames:128 boundaries:32,64,96
//! digital: split edges:1376 per-block:4.00 edge-frames:1,33,65,97
//! ```
//!
//! `boundaries` is where each share after the first begins, and
//! `edge-frames` is every distinct frame an edge was seen on during the
//! window — both listed in full rather than summarised, so the line can
//! be read as the measurement rather than as evidence for one.
//!
//! With one render thread the whole block is one share at one level, so
//! `edges:0` and `edge-frames:none` are the expected — and confirming —
//! result there.
//!
//! ```sh
//! ./io_digital --split --threads 2 --period 128
//! ```
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

use core::fmt::{self, Write as _};
use core::str;
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

/// How many distinct edge positions one window can record.
///
/// The split mode produces one position per render thread — one edge
/// per share boundary, plus the wrap into the next block — so this
/// covers up to sixteen threads, which is four times what a Gem has
/// cores for. Past that, or if the pin carries edges the model does
/// not predict, the positions that did not fit are dropped and the
/// report says so with a trailing `…` rather than presenting what fit
/// as the whole of it.
const MAX_EDGE_FRAMES: usize = 16;

#[allow(
    clippy::struct_excessive_bools,
    reason = "each one is an independent one-bit fact about the loopback, and grouping them into a state enum would only hide which are true together"
)]
struct Loopback {
    /// Whether the output is driven from `render`, one value per render
    /// thread, rather than toggled from `render_pre`. See
    /// [the split mode](self#the-split-mode) above.
    split: bool,

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

    /// Every distinct frame an edge landed on, in the split mode. The
    /// whole point of that mode: the frame an edge appears at is what
    /// says where one thread's share of the block ended, and with more
    /// than two threads there is a position per boundary rather than
    /// one. Kept as the set of positions and not as a range, so that
    /// what is written down is what was seen.
    ///
    /// Fixed capacity, because this is filled from the audio thread. A
    /// window that finds more positions than fit ends its list with a
    /// `…` rather than dropping them quietly: more positions than
    /// boundaries is itself the finding.
    edge_frames: [usize; MAX_EDGE_FRAMES],
    edge_frame_count: usize,
    edge_frames_overflowed: bool,

    /// The state of the LED channel, changed once per report.
    led: bool,
}

/// A comma-separated list of numbers, built without allocating.
///
/// The split report wants its two lists on one line each, and both are
/// assembled on the audio thread. `rt_println!` takes what it is given
/// and cannot join a slice, so the joining happens here into a fixed
/// buffer.
struct List {
    bytes: [u8; List::CAPACITY],
    len: usize,
    /// Whether anything was dropped, either by the buffer filling or by
    /// the caller. A list that lost entries says so instead of reading
    /// as a complete one.
    truncated: bool,
}

impl List {
    /// Enough for sixteen five-digit numbers and their separators.
    const CAPACITY: usize = 96;

    const fn new() -> Self {
        Self {
            bytes: [0; Self::CAPACITY],
            len: 0,
            truncated: false,
        }
    }

    /// Appends one number, with a separator if it is not the first.
    ///
    /// All of it or none of it: if the number will not fit, the
    /// separator that was about to introduce it is rolled back, because
    /// a trailing comma reads as an entry that is not there.
    fn push(&mut self, value: usize) {
        let mark = self.len;
        if self.len > 0 && write!(self, ",").is_err() {
            return;
        }
        if write!(self, "{value}").is_err() {
            self.len = mark;
        }
    }

    /// Marks the list as incomplete for a reason of the caller's.
    const fn mark_truncated(&mut self) {
        self.truncated = true;
    }

    /// The list so far, and nothing about what it lost — `none` when
    /// nothing was ever added, `…` when entries existed and none of
    /// them fit.
    fn as_str(&self) -> &str {
        if self.len == 0 {
            return if self.truncated { "…" } else { "none" };
        }
        // Only ASCII digits and commas are ever written, so a prefix of
        // the buffer is always valid UTF-8; the fallback is there to
        // keep this total rather than because it can happen.
        str::from_utf8(&self.bytes[..self.len]).unwrap_or("?")
    }

    /// What to print after [`as_str`](List::as_str) so that a list
    /// which lost entries cannot be read as a complete one.
    ///
    /// Separate from `as_str` because that hands out a borrow of the
    /// buffer and so cannot append to itself. Empty unless something
    /// was dropped, and empty as well when nothing fit at all, because
    /// `as_str` has already said so.
    const fn suffix(&self) -> &'static str {
        if self.truncated && self.len > 0 {
            ",…"
        } else {
            ""
        }
    }
}

impl fmt::Write for List {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let room = Self::CAPACITY - self.len;
        if text.len() > room {
            // Refusing a partial write: half a number would read as a
            // different number.
            self.truncated = true;
            return Err(fmt::Error);
        }
        self.bytes[self.len..self.len + text.len()].copy_from_slice(text.as_bytes());
        self.len += text.len();
        Ok(())
    }
}

impl Loopback {
    const fn new(split: bool) -> Self {
        Self {
            split,
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
            edge_frames: [0; MAX_EDGE_FRAMES],
            edge_frame_count: 0,
            edge_frames_overflowed: false,
            led: false,
        }
    }

    /// Adds `frame` to the positions seen this window, if it is new.
    ///
    /// A linear scan of at most [`MAX_EDGE_FRAMES`] entries, which is
    /// cheaper on the audio thread than anything that would keep them
    /// sorted, and the positions are wanted in the order they were
    /// first met anyway: that order is the shape of the block.
    fn record_edge_frame(&mut self, frame: usize) {
        if self.edge_frames[..self.edge_frame_count].contains(&frame) {
            return;
        }
        if self.edge_frame_count == MAX_EDGE_FRAMES {
            self.edge_frames_overflowed = true;
            return;
        }
        self.edge_frames[self.edge_frame_count] = frame;
        self.edge_frame_count += 1;
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
        self.edge_frame_count = 0;
        self.edge_frames_overflowed = false;
    }

    /// One window's findings, then the LED changes state.
    fn report(&mut self, context: &mut BlockContext) {
        if self.split {
            self.report_split(context);
            self.reset();
            return;
        }
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
        self.blink(context);

        self.reset();
    }

    /// One window's findings in the split mode.
    ///
    /// The numbers to read together are `per-block` and `edge-frames`.
    /// Threads writing alternating levels over their own shares make
    /// the output change at every boundary between them, so the block
    /// carries one edge per boundary plus one for the wrap into the
    /// next block, and each sits at a fixed frame. `boundaries` lists
    /// where every share after the first begins, by the same division
    /// `RenderContext::digital_frame_range` makes, and each edge should
    /// be one frame past one of them — for the same reason the loopback
    /// latency has a `+1` in it.
    ///
    /// Both lists are printed in full rather than as a range, so that
    /// the line can be read as the measurement instead of as evidence
    /// for one.
    fn report_split(&mut self, context: &mut BlockContext) {
        let threads = context.thread_count();
        let frames = context.digital_frames();

        let mut boundaries = List::new();
        for thread in 1..threads {
            // The start of `thread`'s share, as `partition` computes it.
            boundaries.push(frames * thread / threads);
        }

        let mut positions = List::new();
        for index in 0..self.edge_frame_count {
            positions.push(self.edge_frames[index]);
        }
        if self.edge_frames_overflowed {
            positions.mark_truncated();
        }

        // Edges per block to two decimals, without a float: the whole
        // claim is that this is an exact small integer, and a rounded
        // 2.0 would hide a block that occasionally carried three.
        let per_block = if self.blocks == 0 {
            0
        } else {
            u64::from(self.edges) * 100 / u64::from(self.blocks)
        };
        rt_println!(
            "digital: split threads:{} frames:{} boundaries:{}{}",
            threads,
            frames,
            boundaries.as_str(),
            boundaries.suffix()
        );
        rt_println!(
            "digital: split edges:{} per-block:{}.{:02} edge-frames:{}{}",
            self.edges,
            per_block / 100,
            per_block % 100,
            positions.as_str(),
            positions.suffix()
        );
        self.blink(context);
    }

    /// Changes the LED channel, from `render_pre` and so over the whole
    /// block whatever the thread count.
    fn blink(&mut self, context: &mut BlockContext) {
        self.led = !self.led;
        context.digital_write(0, LED_CHANNEL, self.led);
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
        let write_frame = WRITE_FRAME.min(frames.saturating_sub(1));

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
            if self.split {
                // Where the edge is, not how long it took: the frame it
                // lands on is what says where a thread's share ended.
                self.record_edge_frame(frame);
                self.edges += 1;
            } else if self.awaiting {
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
        // In the split mode the output belongs to `render`, and a write
        // from here would be writing over every thread's share of it.
        if !self.split && self.blocks_since_write >= WRITE_PERIOD_BLOCKS {
            if self.awaiting {
                // The previous write produced no edge in the whole
                // period it had. Counted, and then forgotten: if its
                // edge does turn up later it will be taken for this
                // write's, and land in `max_latency` as an outlier
                // wider than a period. So `misses` above 0 makes every
                // latency in the same window suspect, which is why the
                // two are reported on one line.
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

    fn render(&self, _state: &mut (), context: &mut RenderContext) {
        // Outside the split mode everything is the whole block's, and
        // `render_pre` has already done it.
        if !self.split {
            return;
        }
        // Each thread holds its own share at its own level, so the
        // block comes out low over the first share and high over the
        // second. If `digital_write` ran to the end of the *block* the
        // way the C helper does, the last thread to run would take the
        // tail and the edge would move from block to block; that it
        // does not is the thing being measured.
        let range = context.digital_frame_range();
        if range.is_empty() {
            // More threads than frames. Nothing of this block is this
            // thread's, and writing anything would be writing into
            // another thread's share.
            return;
        }
        let level = context.this_thread() % 2 == 1;
        context.digital_write(range.start, OUT_CHANNEL, level);
    }
}

#[cfg(bela_device)]
fn main() -> ExitCode {
    use std::env::args_os;
    use std::ffi::OsString;

    // `--split` and `--threads` are this probe's own, and Bela's option
    // parser treats anything it does not know as an error, so they are
    // taken out before the rest of the arguments go on.
    // `examples/command_line` is the worked example of the pattern.
    //
    // The thread count has to be one of ours: libbela's option list has
    // no spelling for it, so the only way in is `Settings`.
    let mut split = false;
    let mut threads = None;
    let mut want_threads = false;
    let mut args: Vec<OsString> = Vec::new();
    for argument in args_os() {
        if want_threads {
            want_threads = false;
            threads = argument.to_str().and_then(|value| value.parse().ok());
            if threads.is_none() {
                eprintln!("--threads wants a number");
                return ExitCode::FAILURE;
            }
            continue;
        }
        match argument.to_str() {
            Some("--split") => split = true,
            Some("--threads") => want_threads = true,
            _ => args.push(argument),
        }
    }
    if want_threads {
        // `--threads` with nothing after it. Refusing rather than
        // falling back to the default: the split measurement is a
        // statement about a thread count, so a run that quietly used a
        // different one would produce numbers that look right and mean
        // something else.
        eprintln!("--threads wants a number");
        return ExitCode::FAILURE;
    }

    let mut settings = bela::Settings::new();
    if let Some(threads) = threads {
        settings = settings.thread_count(threads);
    }
    match bela::Bela::run_with_args(Loopback::new(split), &settings, args) {
        Ok(()) => ExitCode::SUCCESS,
        // The `Display` form says what went wrong; returning the error
        // from `main` would print its `Debug` one.
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
