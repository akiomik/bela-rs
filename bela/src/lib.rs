//! Safe Rust API for real-time audio on [Bela Gem].
//!
//! Built on top of the raw FFI bindings in [`bela_sys`]. User code
//! implements the [`BelaApplication`] trait and hands an instance to
//! [`Bela::run`]:
//!
//! ```ignore
//! use bela::{Bela, BelaApplication, RenderContext, Settings, SetupContext, ThreadInfo};
//!
//! struct Passthrough;
//!
//! impl BelaApplication for Passthrough {
//!     type RenderState = ();
//!
//!     fn create_render_state(&mut self, _thread: ThreadInfo, _context: &SetupContext) {}
//!
//!     fn render(&self, _state: &mut (), context: &mut RenderContext) {
//!         // Copy audio input to audio output, for this thread's frames...
//!     }
//! }
//!
//! fn main() -> Result<(), bela::Error> {
//!     Bela::run(Passthrough, &Settings::new().period_size(64))
//! }
//! ```
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
pub use print::{MESSAGE_CAPACITY, print_args, println_args};
pub use settings::Settings;
#[cfg(bela_device)]
pub use system::Bela;
pub use task::{AUDIO_PRIORITY, AuxiliaryTask};
pub use util::{constrain, map};

pub use bela_sys;
