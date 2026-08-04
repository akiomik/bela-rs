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
//! Debugging output from the audio thread goes through
//! [`rt_println!`], which formats into a fixed-size stack buffer and
//! hands it to Bela's real-time print function — `println!` allocates
//! and blocks, and is forbidden in `render`.
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
mod context;
mod error;
mod print;
mod settings;
#[cfg(bela_device)]
mod system;
mod util;

pub use application::BelaApplication;
pub use context::{Context, PinMode};
pub use error::Error;
pub use print::{MESSAGE_CAPACITY, print_args, println_args};
pub use settings::Settings;
#[cfg(bela_device)]
pub use system::Bela;
pub use util::{constrain, map};

pub use bela_sys;
