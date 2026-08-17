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
//! Being reconfigurable from outside means being given configurations
//! the program was not written for, so an application that needs
//! particular ones says so in
//! [`validate_settings`](BelaApplication::validate_settings). It is
//! asked about the [`ResolvedSettings`] — everything applied, the
//! command line included — before the audio system is built, and what
//! it refuses comes back as [`Error::SettingsRefused`] with the
//! process untouched. That is the only place an application can
//! decline: [`setup`](BelaApplication::setup) runs inside
//! `Bela_initAudio` with the hardware already up, and refusing from
//! there leaves the process unable to build another audio system.
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
    BlockContext, CallbackContext, CleanupContext, PairedIo, PinMode, RenderContext, SetupContext,
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
pub use settings::{ResolvedSettings, Settings};
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
///
/// Unlike the rest of the audio system this exists on every target.
/// Off-device it is always `false`: there is no audio system, so
/// nothing has asked it to stop. That keeps a loop of the program's
/// own that winds down with the audio system one piece of code that
/// compiles, lints and unit-tests on a development machine and runs on
/// the board, rather than one guarded by a `cfg` and so absent from
/// the build its tests run in.
///
/// ```
/// use std::sync::mpsc::Receiver;
/// use std::time::Duration;
///
/// /// Prints peaks a render callback published, on a thread of the
/// /// program's own, until the audio system is asked to stop.
/// fn report(peaks: &Receiver<f32>) {
///     // Bounded, so that the flag is read whether or not peaks are
///     // still arriving: a `recv` with nothing to return would either
///     // block past the stop or, once the sender is gone, spin.
///     while !bela::stop_requested() {
///         if let Ok(peak) = peaks.recv_timeout(Duration::from_millis(100)) {
///             println!("peak {peak:.3}");
///         }
///     }
/// }
/// ```
///
/// # A render callback cannot react to an outside stop
///
/// libbela's render loop reads this same flag itself — as its loop
/// condition, and again part-way through the iteration — and both
/// reads come before the block reaches the application. So a stop
/// asked for between blocks, which is where a stop from the stop
/// button, the IDE, or the signal handlers
/// [`Bela::until_stopped`](crate::Bela::until_stopped) installs almost
/// always lands, ends the loop rather than arriving in
/// [`render_pre`](BelaApplication::render_pre),
/// [`render`](BelaApplication::render) or
/// [`render_post`](BelaApplication::render_post).
///
/// What is left is the block itself: a stop asked for by another
/// thread while the callbacks are running sets a flag they can still
/// read, and nothing rules that out. It is a race no application can
/// arrange or rely on, and on a Bela Gem Stereo at 48 kHz with a
/// period of 16 it did not happen at all — a run of 25784 blocks
/// ended with SIGINT had all three callbacks read `false` in every one
/// of them, the last block rendered included.
///
/// The stop a callback does see is one the application asked for
/// itself. [`request_stop`] from `render_pre` is visible to that
/// block's `render` and `render_post`, measured on the same board — and
/// that block is the last one, because libbela's next read of the flag
/// ends the loop.
///
/// So an output that must not be abandoned mid-value — a digital pin
/// held high, an indicator lit — has no hook for an outside stop.
/// [`cleanup`](BelaApplication::cleanup) is not one: it runs after the
/// audio thread has been joined, and [`CleanupContext`] carries no
/// buffers to write to. The pin goes on driving its last value after
/// the program exits, until the next audio system to start opens every
/// channel as an input. An application that must not leave one driving
/// has to be the one requesting the stop, and to have written the
/// value it wants left behind in an earlier block.
#[must_use]
#[cfg_attr(
    not(bela_device),
    allow(
        clippy::missing_const_for_fn,
        reason = "the device implementation reads a flag through libbela and cannot be const; one signature for both targets is the point"
    )
)]
pub fn stop_requested() -> bool {
    runtime::stop_requested()
}

#[cfg(all(test, not(bela_device)))]
mod tests {
    #[test]
    fn a_stop_is_never_requested_off_the_device() {
        // The point of the function existing here at all: an
        // application's callbacks can ask, and get the answer that
        // matches a host build — no audio system, so nothing has asked
        // it to stop.
        assert!(
            !super::stop_requested(),
            "off-device there is no audio system for anything to have stopped"
        );
    }
}

// The relay build.rs performs, tested where a build script cannot be:
// `cargo test` builds this crate, not `build.rs`. See ../link_args.rs
// and bela-sys/src/lib.rs's own `link_args` module, which this mirrors
// for the decoding half.
#[cfg(test)]
mod link_args {
    include!("../link_args.rs");

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| {
            pairs
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| (*value).to_owned())
        }
    }

    #[test]
    fn nothing_published_decodes_to_no_arguments() {
        // A host build, or bela-sys on a native build with
        // BELA_SYSROOT unset: no DEP_BELA_LINK_ARGS_COUNT at all.
        assert_eq!(decode_link_args(env(&[]), "DEP_BELA"), Vec::<String>::new());
    }

    #[test]
    fn a_published_count_of_zero_also_decodes_to_none() {
        assert_eq!(
            decode_link_args(env(&[("DEP_BELA_LINK_ARGS_COUNT", "0")]), "DEP_BELA"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn arguments_are_read_back_in_order() {
        assert_eq!(
            decode_link_args(
                env(&[
                    ("DEP_BELA_LINK_ARGS_COUNT", "2"),
                    ("DEP_BELA_LINK_ARGS_0", "--sysroot=/opt/bela"),
                    ("DEP_BELA_LINK_ARGS_1", "-Bfoo"),
                ]),
                "DEP_BELA",
            ),
            vec!["--sysroot=/opt/bela".to_owned(), "-Bfoo".to_owned()]
        );
    }

    #[test]
    fn the_prefix_selects_which_dependency_is_read() {
        // bela reads DEP_BELA_*, from bela-sys; an application reads
        // DEP_BELA_RELAY_*, from bela. A stray DEP_BELA_* is not
        // mistaken for the other.
        let pairs = [
            ("DEP_BELA_LINK_ARGS_COUNT", "1"),
            ("DEP_BELA_LINK_ARGS_0", "-Wl,-rpath-link=/opt/bela/lib"),
            ("DEP_BELA_RELAY_LINK_ARGS_COUNT", "1"),
            ("DEP_BELA_RELAY_LINK_ARGS_0", "--sysroot=/opt/bela"),
        ];
        assert_eq!(
            decode_link_args(env(&pairs), "DEP_BELA"),
            vec!["-Wl,-rpath-link=/opt/bela/lib".to_owned()]
        );
        assert_eq!(
            decode_link_args(env(&pairs), "DEP_BELA_RELAY"),
            vec!["--sysroot=/opt/bela".to_owned()]
        );
    }

    #[test]
    fn a_missing_indexed_value_decodes_to_no_arguments_rather_than_a_partial_set() {
        // The count promised two; only the first is there. Applying
        // just it would drop `-B` or `-rpath-link` silently and fail
        // at link with a confusing error far from the cause, so this
        // reads as "nothing published" instead.
        assert_eq!(
            decode_link_args(
                env(&[
                    ("DEP_BELA_LINK_ARGS_COUNT", "2"),
                    ("DEP_BELA_LINK_ARGS_0", "--sysroot=/opt/bela"),
                ]),
                "DEP_BELA",
            ),
            Vec::<String>::new()
        );
    }

    #[test]
    fn encoding_what_was_decoded_reproduces_the_input() {
        let args = vec!["--sysroot=/opt/bela".to_owned(), "-Bfoo".to_owned()];
        assert_eq!(
            encode_link_args(&args),
            vec![
                ("LINK_ARGS_COUNT".to_owned(), "2".to_owned()),
                ("LINK_ARGS_0".to_owned(), "--sysroot=/opt/bela".to_owned()),
                ("LINK_ARGS_1".to_owned(), "-Bfoo".to_owned()),
            ]
        );
    }
}
