//! Safe Rust API for real-time audio on [Bela Gem].
//!
//! Built on top of the raw FFI bindings in [`bela_sys`]. The planned
//! surface is a builder for audio settings, an `unsafe` real-time trait
//! implemented by user code (the `render` path must not allocate, block
//! or panic), and RAII management of `Bela_startAudio` /
//! `Bela_stopAudio` / `Bela_cleanupAudio`.
//!
//! [Bela Gem]: https://bela.io

pub use bela_sys;
