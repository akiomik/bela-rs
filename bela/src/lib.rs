//! Safe Rust API for real-time audio on [Bela Gem].
//!
//! Built on top of the raw FFI bindings in [`bela_sys`]. User code
//! implements the [`BelaApplication`] trait — an `unsafe` trait, because
//! implementing it is a promise that `render` is real-time safe — and
//! hands an instance to [`Bela::run`]:
//!
//! ```ignore
//! use bela::{Bela, BelaApplication, Context, Settings};
//!
//! struct Passthrough;
//!
//! unsafe impl BelaApplication for Passthrough {
//!     fn render(&mut self, context: &mut Context) {
//!         // Copy audio input to audio output...
//!     }
//! }
//!
//! fn main() -> Result<(), bela::Error> {
//!     Bela::run(Passthrough, &Settings::new().period_size(64))
//! }
//! ```
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
//! Whether `render` fits within its block deadline is answered by
//! [`Settings::cpu_monitoring`], which makes [`Context::cpu_usage`]
//! report how much of each block the audio thread uses, and by
//! [`CpuTimer`], which measures one section of `render` at a time.
//! Without them the first sign of running out of headroom is a
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
//! [`Bela`] itself calls into `libbela` and therefore only exists when
//! compiling for the device target (`aarch64-unknown-linux-gnu`); the
//! rest of the crate — [`BelaApplication`], [`Context`], [`Settings`] —
//! is target-independent and unit-tested on the host.
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
mod level;
mod print;
mod settings;
mod singleton;
#[cfg(bela_device)]
mod system;
mod task;
mod util;

pub use application::BelaApplication;
#[cfg(bela_device)]
pub use cmdline::print_usage;
pub use context::{Context, PinMode};
pub use cpu::{CpuSection, CpuTimer, CpuUsage, MAX_MONITORED_PERIOD_SIZE};
pub use error::Error;
pub use level::{Channel, MAX_DECIBELS};
pub use print::{MESSAGE_CAPACITY, print_args, println_args};
pub use settings::Settings;
#[cfg(bela_device)]
pub use system::Bela;
pub use task::{AUDIO_PRIORITY, AuxiliaryTask};
pub use util::{constrain, map};

pub use bela_sys;
