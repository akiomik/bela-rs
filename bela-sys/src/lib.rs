//! Raw FFI bindings to the Bela core API (`libbela`) for [Bela Gem].
//!
//! This crate exposes the C surface of the Bela core API (`Bela.h`):
//! `BelaContext`, `setup` / `render` / `cleanup`, `Bela_initAudio`,
//! `Bela_startAudio` and friends. Bindings are generated with `bindgen`
//! at build time. Higher-level C++ libraries (Scope, Trill, Fft, Gui,
//! Midi) are out of scope.
//!
//! Target platform is Bela Gem on PocketBeagle 2
//! (`aarch64-unknown-linux-gnu`). For a safe API, use the `bela-rs`
//! crate instead.
//!
//! [Bela Gem]: https://bela.io
#![no_std]

// TODO: include!(concat!(env!("OUT_DIR"), "/bindings.rs")) once the
// bindgen build script lands.
