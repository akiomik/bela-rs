//! The real-to-complex FFT from NE10, the ARM DSP library the board
//! ships as `libNE10.so.10`.
//!
//! Unlike the rest of this crate, these declarations are neither
//! generated nor a shim of ours: NE10 is plain C, and this module
//! mirrors four of its functions and one of its structs by hand. The
//! headers they are written against are vendored in `vendor/ne10/` as
//! a drift baseline — nothing generates from them — and
//! `abi/ne10_abi.c` asserts at build time that the board's headers
//! still agree with what is written here.
//!
//! Bela has an `Fft` class of its own in `libbelaextra` wrapping these
//! same calls. `docs/fft.md` in the repository records why this crate
//! calls NE10 instead, along with what the transforms do to their
//! arguments.
//!
//! The transforms come in two spellings. `ne10_fft_r2c_1d_float32` is
//! a function pointer that stays null until `ne10_init` runs; the
//! `_neon` symbols declared here are the implementations behind it,
//! called directly, which is what Bela's own class does. The target is
//! `ARMv8` with NEON, so there is nothing to dispatch.
//!
//! Off the device target the library is not linked and these symbols
//! do not resolve. Declaring them anyway keeps the module compiling
//! everywhere the rest of the crate does.

use core::ffi::c_int;
use core::marker::{PhantomData, PhantomPinned};

/// One complex value, as NE10 lays it out: real part, then imaginary.
///
/// `#[repr(C)]` because slices of these are handed to the transforms
/// as they stand. `abi/ne10_abi.c` pins the size, the alignment, the
/// offsets and both field types against the board's header.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
#[allow(non_camel_case_types, reason = "the name the C header gives it")]
pub struct ne10_fft_cpx_float32_t {
    /// The real part.
    pub r: f32,
    /// The imaginary part.
    pub i: f32,
}

/// A real-to-complex plan, as an opaque pointee.
///
/// Deliberately not a description of NE10's struct: its layout is
/// conditional on `NE10_UNROLL_LEVEL`, a define this crate does not
/// see and must not depend on. What matters about it is that one plan
/// holds the twiddles *and* the scratch buffer a transform writes
/// through, so a plan has one user at a time.
///
/// The marker is what the nomicon asks of an opaque type: a bare
/// zero-length array would make this `Send`, `Sync` and `Unpin`.
#[repr(C)]
#[derive(Debug)]
#[allow(non_camel_case_types, reason = "the name the C header gives it")]
pub struct ne10_fft_r2c_state_float32_t {
    _data: [u8; 0],
    _marker: PhantomData<(*mut u8, PhantomPinned)>,
}

unsafe extern "C" {
    /// Allocates a plan for transforms of `nfft` real points, or
    /// returns null. One `malloc`: the twiddles and the scratch are
    /// offsets inside the block it returns.
    ///
    /// # Safety
    ///
    /// `nfft` is a power of two from 2 to 65536. Anything else is not
    /// established to be harmless — NE10 sizes the allocation from
    /// `nfft` in signed arithmetic, and neither the header nor this
    /// repository's measurements say what a negative or absurd one
    /// does on the way to giving up.
    ///
    /// Allocating is safe at 2 and 4; **transforming is not**, so the
    /// range a plan may usefully be built over is 8 to 65536. That
    /// narrower range is what `bela::FftLength` holds. See the two
    /// transforms below and `docs/fft.md`.
    ///
    /// The result may be null and has to be checked. A non-null
    /// result is owned by the caller and freed with
    /// [`ne10_fft_destroy_r2c_float32`].
    pub fn ne10_fft_alloc_r2c_float32(nfft: c_int) -> *mut ne10_fft_r2c_state_float32_t;

    /// Frees a plan. Null is accepted.
    ///
    /// # Safety
    ///
    /// `cfg` is null, or a pointer from
    /// [`ne10_fft_alloc_r2c_float32`] that has not been destroyed
    /// already, with no transform on it in flight. Passing one twice
    /// is a double free.
    pub fn ne10_fft_destroy_r2c_float32(cfg: *mut ne10_fft_r2c_state_float32_t);

    /// Transforms the plan's `nfft` real samples at `fin` into the
    /// `nfft / 2 + 1` bins at `fout`. Unscaled.
    ///
    /// # Safety
    ///
    /// All of:
    ///
    /// - **The plan's `nfft` is at least 8.** At 2 and 4 the NEON
    ///   kernels write outside the buffers they are given, on both
    ///   sides and in both directions — three bins before bin 0 of the
    ///   spectrum at 2, twenty-six floats past the signal on the way
    ///   back — so no buffer a caller can allocate makes the call
    ///   sound. Measured on a board; `docs/fft.md` has the numbers.
    /// - `cfg` is a live plan from [`ne10_fft_alloc_r2c_float32`] and
    ///   this call has exclusive use of it for its duration: the
    ///   scratch buffer the transform writes through lives in the
    ///   plan, so two concurrent calls on one plan race.
    /// - `fin` is valid for reads **and writes** of the plan's `nfft`
    ///   `f32`s. The parameter is not `const`, and NE10 may use the
    ///   input as scratch.
    /// - `fout` is valid for writes of `nfft / 2 + 1`
    ///   [`ne10_fft_cpx_float32_t`]s.
    /// - Both are aligned for their type with their contents
    ///   initialised, and neither overlaps the other or the plan.
    ///
    /// The length is the plan's, not an argument: a buffer sized for
    /// another one is read or written out of bounds with nothing to
    /// report it.
    pub fn ne10_fft_r2c_1d_float32_neon(
        fout: *mut ne10_fft_cpx_float32_t,
        fin: *mut f32,
        cfg: *mut ne10_fft_r2c_state_float32_t,
    );

    /// Transforms the `nfft / 2 + 1` bins at `fin` back into the
    /// plan's `nfft` real samples at `fout`.
    ///
    /// # Safety
    ///
    /// The contract of [`ne10_fft_r2c_1d_float32_neon`] with the
    /// roles swapped: `fin` is valid for reads and writes of
    /// `nfft / 2 + 1` bins, `fout` for writes of `nfft` `f32`s.
    ///
    /// The floor of 8 on the plan's `nfft` is the same and for the
    /// same reason, measured in this direction too: at 4 points this
    /// writes twenty-four floats past the output it was given, and at
    /// 2 it writes two floats before it as well.
    pub fn ne10_fft_c2r_1d_float32_neon(
        fout: *mut f32,
        fin: *mut ne10_fft_cpx_float32_t,
        cfg: *mut ne10_fft_r2c_state_float32_t,
    );
}

// What `abi/ne10_abi.c` asserts on the C side, asserted here on ours:
// the two together are "the header and this module agree". These catch
// a mistake in the declarations above, which a build against a correct
// header would otherwise compile happily.
const _: () = {
    assert!(
        size_of::<ne10_fft_cpx_float32_t>() == 8,
        "a bin is two f32s, and slices of them are handed to NE10 as they stand"
    );
    assert!(
        align_of::<ne10_fft_cpx_float32_t>() == align_of::<f32>(),
        "a bin is aligned as its fields are; anything else would need padding"
    );
};
