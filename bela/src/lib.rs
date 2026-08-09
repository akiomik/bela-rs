//! Safe Rust API for real-time audio on [Bela Gem].
//!
//! Built on top of the raw FFI bindings in [`bela_sys`]. User code
//! implements the [`BelaApplication`] trait and hands an instance to
//! [`Bela::run`]:
//!
//! ```
//! use bela::{BelaApplication, RenderContext, SetupContext, ThreadInfo};
//!
//! struct Passthrough;
//!
//! impl BelaApplication for Passthrough {
//!     type RenderState = ();
//!
//!     fn create_render_state(&mut self, _thread: ThreadInfo, _context: &SetupContext) {}
//!
//!     fn render(&self, _state: &mut (), context: &mut RenderContext) {
//!         let channels = context
//!             .audio_in_channels()
//!             .min(context.audio_out_channels());
//!         // This thread's share of the block; with one render thread,
//!         // all of it.
//!         for frame in context.audio_frame_range() {
//!             for channel in 0..channels {
//!                 let sample = context.audio_read(frame, channel);
//!                 context.audio_write(frame, channel, sample);
//!             }
//!         }
//!     }
//! }
//! ```
//!
//! `Bela::run(Passthrough, &Settings::new().period_size(64))` then runs
//! it until something asks it to stop. That line is not part of the
//! example above because [`Bela`] only exists on the device target, and
//! this doc test is compiled on the host — where the application is,
//! which is the part that has to keep up with the trait.
//! `examples/passthrough.rs` is the whole program, and the rest of
//! `examples/` covers the crate a piece at a time.
//!
//! # One application model, one or four threads
//!
//! Bela can render a block on several threads at once — a Bela Gem has
//! four cores — and it does so by calling `render` on all of them
//! simultaneously, for the same block, over the same buffers. Nothing
//! is partitioned on the C side.
//!
//! [`BelaApplication`] is shaped for that, and a single render thread
//! is the same shape with one of everything:
//!
//! - the application is shared as `&self` while rendering, so whatever
//!   `render` mutates lives in a
//!   [`RenderState`](BelaApplication::RenderState), one per thread;
//! - [`RenderContext`] reads the whole block but writes only this
//!   thread's [`audio_frame_range`](RenderContext::audio_frame_range),
//!   and the ranges tile the block exactly;
//! - [`render_pre`](BelaApplication::render_pre) and
//!   [`render_post`](BelaApplication::render_post) bracket the parallel
//!   section on the main audio thread, with the whole block and every
//!   render state to themselves — where per-block preparation and
//!   mixing down belong.
//!
//! [`Settings::thread_count`] chooses how many threads; nothing else
//! about an application changes with it. What Bela actually does, and
//! how it was measured, is in `docs/multithreaded-rendering.md`.
//!
//! # Everything else
//!
//! Work that must not happen in `render` — file and network I/O,
//! expensive calculations, anything that allocates or blocks — belongs
//! in an [`AuxiliaryTask`], which `render` triggers with a real-time
//! safe `schedule` call.
//!
//! MIDI arrives through [`MidiInput`], opened in `setup` with a name
//! from [`midi_ports`] and read once per block from
//! [`render_pre`](BelaApplication::render_pre): taking a message is a
//! ring read, and what it changes is what `render` then plays. The
//! messages are [`MidiMessage`], and what they carry — [`Note`],
//! [`Velocity`], [`MidiChannel`] and the rest — are types of their own
//! rather than bytes, because a note and a velocity are two numbers in
//! the same range.
//!
//! Sending goes through [`MidiOutput`], which hands each render thread
//! a [`MidiSender`] to keep in its render state. Sending queues a
//! message and schedules a task; the writing to Bela happens on that
//! task's thread rather than on the audio thread, for reasons
//! `docs/midi.md` sets out. What has no block left to be sent in — a
//! closing all-notes-off — goes through
//! [`MidiOutput::send`](MidiOutput::send) from
//! [`cleanup`](BelaApplication::cleanup).
//!
//! Debugging output from the audio thread goes through
//! [`rt_println!`], which formats into a fixed-size stack buffer and
//! hands it to Bela's real-time print function — `println!` allocates
//! and blocks, and is forbidden in `render`.
//!
//! Whether rendering fits within its block deadline is answered by
//! [`Settings::cpu_monitoring`], which makes
//! [`BlockContext::cpu_usage`] report how much of each block the audio
//! thread uses, and by [`CpuTimer`], which measures one section at a
//! time. Without them the first sign of running out of headroom is a
//! dropout, after the fact.
//!
//! The codec's own volume controls — the line out level, the headphone
//! level and the gain of the preamplifier ahead of the ADC — are set
//! through the [`Bela`] handle, with
//! [`set_line_out_level`](Bela::set_line_out_level) and its siblings.
//! They can be set before audio starts as well as while it runs, which
//! is what [`Bela::until_stopped`] leaves room for.
//!
//! A built binary stays reconfigurable through Bela's standard
//! command-line options — `--period`, `--verbose`, `--use-analog` and
//! the rest, the same ones every other way of writing a Bela program
//! accepts. [`Bela::run_with_args`] applies them on top of
//! [`Settings`], so the application keeps its own defaults, and
//! [`print_usage`] prints the list.
//!
//! One corner of the C core API has no safe accessors here on purpose:
//! the Multiplexer Capelet, `multiplexerAnalogRead` and
//! `multiplexerChannelForFrame`. The Capelet is an accessory for the
//! original Bela cape and cannot be attached to a Gem, so what a
//! reading means — which Capelet pin it came from — cannot be checked
//! on the board this crate is measured against. `docs/board-facts.md`
//! records what a Gem does with the multiplexer settings regardless.
//!
//! What the program is running on is [`Board::detect`] and
//! [`Version::running`] — the board libbela says it found, and the
//! version of the library it found it with. Both answer before there is
//! an audio system, so a program built and measured against one board
//! can say so and decline rather than fail partway through bringing one
//! up, and an `examples/board_info` run is the first thing to ask for
//! from anyone reporting a problem.
//!
//! [`Bela`] itself calls into `libbela` and therefore only exists when
//! compiling for the device target (`aarch64-unknown-linux-gnu`); the
//! rest of the crate — [`BelaApplication`], the contexts, [`Settings`]
//! — is target-independent and unit-tested on the host.
//!
//! Binaries should set `panic = "abort"` in their release profile: a
//! panic crossing the audio callback boundary aborts the process either
//! way, and `abort` avoids shipping unwinding machinery.
//!
//! [Bela Gem]: https://bela.io

mod application;
mod cmdline;
mod context;
mod cpu;
mod error;
mod hardware;
mod level;
mod midi;
mod print;
mod runtime;
mod settings;
mod singleton;
#[cfg(bela_device)]
mod system;
mod task;
mod util;

pub use application::{BelaApplication, ThreadInfo};
#[cfg(bela_device)]
pub use cmdline::print_usage;
pub use context::{
    BlockContext, CallbackContext, CleanupContext, PinMode, RenderContext, SetupContext,
};
pub use cpu::{CpuSection, CpuTimer, CpuUsage, MAX_MONITORED_PERIOD_SIZE};
pub use error::Error;
pub use hardware::{Board, DetectMode, Version};
pub use level::{Channel, MAX_DECIBELS};
pub use midi::{
    ControlValue, Controller, MidiChannel, MidiInput, MidiMessage, MidiOutput, MidiSender, Note,
    PitchBend, Pressure, Program, Velocity, midi_ports,
};
pub use print::{MESSAGE_CAPACITY, print_args, println_args};
pub use settings::Settings;
#[cfg(bela_device)]
pub use system::Bela;
pub use task::{AuxiliaryTask, Priority};
pub use util::{constrain, map};

pub use bela_sys;

/// Requests that the audio system stop.
///
/// This is safe to call from a signal handler or an auxiliary thread.
#[cfg(bela_device)]
pub fn request_stop() {
    runtime::request_stop();
}

/// Whether a stop has been requested by the stop button, IDE, or
/// [`request_stop`].
#[cfg(bela_device)]
#[must_use]
pub fn stop_requested() -> bool {
    runtime::stop_requested()
}
