//! The FFT: a real-to-complex transform and its inverse.
//!
//! Built on NE10's `float32` real FFT, the same one Bela's `Fft` class
//! calls, reached through [`bela_sys`] rather than through that class.
//! `docs/fft.md` in the repository records why, and what a board said
//! the transforms do — which is where the length range below comes
//! from.
//!
//! A plan ([`RealFft`]) is created once and used for as many
//! transforms as the program needs. Creating one allocates, so it
//! belongs in [`setup`](crate::BelaApplication::setup); the transforms
//! themselves allocate nothing and are what `render` calls.

#[cfg(bela_device)]
use core::ffi::c_int;
use core::fmt;
#[cfg(bela_device)]
use core::ptr::NonNull;

use crate::error::Error;

/// A transform length: a power of two from [`MIN`](FftLength::MIN) to
/// [`MAX`](FftLength::MAX).
///
/// The range is narrower than "any power of two", and the bottom of it
/// is the interesting end. NE10 allocates a plan for 2 and 4 points
/// happily, and its NEON kernels then write outside every buffer they
/// are given — three bins before the start of the spectrum at 2, and
/// twenty-six floats past the end of the signal on the way back —
/// which a wrapper cannot make safe, because the memory being written
/// is not memory the caller owns. Measured on a board; `docs/fft.md`
/// has the numbers. 8 is the shortest transform that stays inside its
/// arguments, so it is the shortest this type holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FftLength(usize);

impl FftLength {
    /// The shortest transform this crate supports, 8 points.
    ///
    /// Not a preference: 2 and 4 point transforms corrupt memory
    /// outside their arguments on the board's NE10. See the type
    /// documentation.
    pub const MIN: Self = Self(8);

    /// The longest transform this crate supports, 65536 points.
    ///
    /// A cap this crate sets rather than a limit NE10 states. It is
    /// the longest that was measured working, and it keeps a length
    /// inside the `int` NE10 takes, so the conversion needs no check
    /// of its own.
    pub const MAX: Self = Self(65536);

    /// A transform length, or [`None`] unless `length` is a power of
    /// two from [`MIN`](Self::MIN) to [`MAX`](Self::MAX).
    ///
    /// `0` and `1` are both [`None`], and so are `2` and `4` — powers
    /// of two that NE10 will plan for and cannot transform safely.
    #[must_use]
    pub const fn new(length: usize) -> Option<Self> {
        if !length.is_power_of_two() || length < Self::MIN.0 || length > Self::MAX.0 {
            return None;
        }
        Some(Self(length))
    }

    /// The shortest supported length of at least `length`, or [`None`]
    /// above [`MAX`](Self::MAX).
    ///
    /// Anything below [`MIN`](Self::MIN) rounds up to it, which is
    /// where this parts company with Bela's `Fft::roundUpToPowerOfTwo`
    /// — that one answers 2, which is a length this crate refuses.
    #[must_use]
    pub const fn rounded_up(length: usize) -> Option<Self> {
        if length <= Self::MIN.0 {
            return Some(Self::MIN);
        }
        if length > Self::MAX.0 {
            return None;
        }
        // `length` is at least 9 here, so `next_power_of_two` cannot
        // overflow before the check above catches it.
        Some(Self(length.next_power_of_two()))
    }

    /// The length, in samples.
    #[must_use]
    pub const fn get(self) -> usize {
        self.0
    }

    /// The length as NE10 takes it: a C `int`.
    ///
    /// Infallible, and by this type's own range rather than by luck:
    /// [`MAX`](Self::MAX) is 65536, which the assertion below pins
    /// well inside what a `c_int` holds on the one target this is
    /// compiled for. That invariant is the whole reason the cast needs
    /// no check.
    #[cfg(bela_device)]
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        reason = "FftLength cannot hold more than MAX, which is 65536"
    )]
    const fn as_nfft(self) -> c_int {
        self.0 as c_int
    }

    /// How many bins a transform of this length produces:
    /// `length / 2 + 1`.
    ///
    /// A real signal has a conjugate-symmetric spectrum, so the bins
    /// above Nyquist repeat the ones below it and are not stored. The
    /// count includes both DC and Nyquist.
    #[must_use]
    pub const fn spectrum_len(self) -> usize {
        self.0 / 2 + 1
    }
}

// What `as_nfft` relies on, written as a literal rather than as
// `i32::MAX as usize` so that the check itself needs no cast.
const _: () = assert!(
    FftLength::MAX.0 <= 2_147_483_647,
    "a transform length has to fit the C int NE10 takes"
);

impl From<FftLength> for usize {
    fn from(length: FftLength) -> Self {
        length.0
    }
}

impl TryFrom<usize> for FftLength {
    type Error = Error;

    /// A transform length, or [`Error::FftLength`].
    ///
    /// The same check as [`new`](Self::new), for code that converts
    /// generically. `new` stays because a range check with one way to
    /// fail says as much with [`Option`].
    ///
    /// # Errors
    ///
    /// [`Error::FftLength`] unless `length` is a power of two from
    /// [`MIN`](Self::MIN) to [`MAX`](Self::MAX).
    fn try_from(length: usize) -> Result<Self, Error> {
        Self::new(length).ok_or(Error::FftLength { value: length })
    }
}

impl fmt::Display for FftLength {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// One frequency bin: a complex number, single precision.
///
/// Laid out as NE10 lays out its own, so a slice of these is handed to
/// the transform as it stands rather than converted. Not
/// `num_complex`'s `Complex32`, which this crate would then owe a
/// major version to; the fields are public and the conversions cheap.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FftBin {
    /// The real part.
    pub re: f32,
    /// The imaginary part.
    pub im: f32,
}

impl FftBin {
    /// The origin: both parts zero.
    ///
    /// The same value [`Default`] gives, in a form a `const` item and
    /// an array initialiser can use.
    pub const ZERO: Self = Self { re: 0.0, im: 0.0 };

    /// A bin from its two parts.
    #[must_use]
    pub const fn new(re: f32, im: f32) -> Self {
        Self { re, im }
    }

    /// The magnitude, `sqrt(re² + im²)`.
    ///
    /// The same quantity Bela's `Fft::fda` reports, not the same
    /// bits: that one uses `sqrtf_neon`, an approximation from
    /// `libraries/math_neon`, and short-circuits to 0 when both parts
    /// are zero.
    #[must_use]
    pub fn magnitude(self) -> f32 {
        self.magnitude_squared().sqrt()
    }

    /// The magnitude squared, `re² + im²`.
    ///
    /// What to compare bins with: it orders them the same way
    /// [`magnitude`](Self::magnitude) does and has no square root in
    /// it.
    #[must_use]
    pub const fn magnitude_squared(self) -> f32 {
        self.re * self.re + self.im * self.im
    }

    /// The phase in radians, from `-π` to `π`.
    #[must_use]
    pub fn phase(self) -> f32 {
        self.im.atan2(self.re)
    }
}

impl From<(f32, f32)> for FftBin {
    fn from((re, im): (f32, f32)) -> Self {
        Self::new(re, im)
    }
}

impl From<[f32; 2]> for FftBin {
    fn from([re, im]: [f32; 2]) -> Self {
        Self::new(re, im)
    }
}

/// A real-to-complex FFT and its inverse, for one transform length.
///
/// Creating one allocates and can fail, so build it in
/// [`setup`](crate::BelaApplication::setup) — which can refuse the run
/// by returning `false` — and move it into the render state from
/// there. Not
/// [`create_render_state`](crate::BelaApplication::create_render_state),
/// which has no way to report a failure, and never `render`.
///
/// **One plan per render thread.** The scratch buffer a transform
/// writes through lives inside the plan, which is why the transforms
/// take `&mut self`: two of them at once on one plan would race. That
/// suits the application model — `render` gets `&mut RenderState` on
/// each thread — and it is why a plan is not something to share.
///
/// Dropping one frees; like creating one, that is not work for the
/// audio thread.
///
/// # Example
///
/// ```no_run
/// use bela::{BelaApplication, FftBin, FftLength, RealFft, RenderContext, SetupContext, ThreadInfo};
///
/// struct Analyser {
///     length: FftLength,
///     plans: Vec<RealFft>,
/// }
///
/// struct Bins {
///     fft: RealFft,
///     window: Vec<f32>,
///     spectrum: Vec<FftBin>,
/// }
///
/// impl BelaApplication for Analyser {
///     type RenderState = Option<Bins>;
///
///     fn setup(&mut self, context: &SetupContext) -> bool {
///         // The one place a failure can be reported.
///         match (0..context.thread_count())
///             .map(|_| RealFft::new(self.length))
///             .collect::<Result<Vec<_>, _>>()
///         {
///             Ok(plans) => {
///                 self.plans = plans;
///                 true
///             }
///             Err(error) => {
///                 println!("FFT: {error}");
///                 false
///             }
///         }
///     }
///
///     fn create_render_state(
///         &mut self,
///         _thread: ThreadInfo,
///         _context: &SetupContext,
///     ) -> Option<Bins> {
///         let fft = self.plans.pop()?;
///         Some(Bins {
///             window: fft.new_signal(),
///             spectrum: fft.new_spectrum(),
///             fft,
///         })
///     }
///
///     fn render(&self, state: &mut Option<Bins>, context: &mut RenderContext) {
///         let Some(state) = state else { return };
///         // …fill state.window from the input…
///         if state.fft.forward(&mut state.window, &mut state.spectrum).is_err() {
///             return; // the buffers came from the plan, so this cannot happen
///         }
///         // …read state.spectrum…
///     }
/// }
/// ```
pub struct RealFft {
    length: FftLength,
    #[cfg(bela_device)]
    plan: NonNull<bela_sys::ne10_fft_r2c_state_float32_t>,
}

// The plan is a `malloc`ed block with no thread affinity: no EVL
// registration, unlike `MidiOutput`, and nothing address-sensitive
// inside it. So it moves between threads soundly, which matters
// because `BelaApplication::RenderState` is `Send`.
//
// `Sync` is sound for a sharper reason than "the transforms take `&mut
// self`": **no `&self` method touches the plan**. `length`,
// `spectrum_len`, `new_signal` and `new_spectrum` read the length and
// nothing else, so a `&RealFft` shared between threads cannot reach
// the scratch buffer NE10 writes during a transform. Any later method
// that takes `&self` and passes the plan to NE10 would break this, and
// it would compile: the invariant lives here rather than in the type
// system.
unsafe impl Send for RealFft {}
unsafe impl Sync for RealFft {}

impl RealFft {
    /// A plan for transforms of `length` points.
    ///
    /// Allocates: NE10 builds the twiddle factors and the scratch
    /// buffer here, once, so that the transforms need none.
    ///
    /// # Errors
    ///
    /// [`Error::FftUnavailable`] off the device target, where there is
    /// no NE10 to plan with, and [`Error::FftCreate`] when NE10
    /// declined — an allocation failure, since the length is already
    /// one it takes.
    #[cfg(bela_device)]
    pub fn new(length: FftLength) -> Result<Self, Error> {
        // Safety: `length` is a power of two from 8 to 65536, which is
        // inside the 2 to 65536 the allocator documents, and the null
        // it answers a refusal with is handled.
        let plan = NonNull::new(unsafe { bela_sys::ne10_fft_alloc_r2c_float32(length.as_nfft()) })
            .ok_or_else(|| Error::FftCreate {
                length: length.get(),
            })?;
        Ok(Self { length, plan })
    }

    /// A plan for transforms of `length` points.
    ///
    /// # Errors
    ///
    /// Always [`Error::FftUnavailable`] off the device target: NE10 is
    /// on the board and nowhere else.
    #[cfg(not(bela_device))]
    #[allow(
        clippy::missing_const_for_fn,
        reason = "mirrors the device signature, which allocates"
    )]
    pub fn new(_length: FftLength) -> Result<Self, Error> {
        Err(Error::FftUnavailable)
    }

    /// The transform length this plan is for.
    #[must_use]
    pub const fn length(&self) -> FftLength {
        self.length
    }

    /// How many bins a transform with this plan produces:
    /// `length / 2 + 1`.
    #[must_use]
    pub const fn spectrum_len(&self) -> usize {
        self.length.spectrum_len()
    }

    /// A zeroed signal buffer of the length this plan transforms.
    ///
    /// Allocates, so `setup` rather than `render`. Its length is what
    /// [`forward`](Self::forward) and [`inverse`](Self::inverse)
    /// require, which is the point of it.
    #[must_use]
    pub fn new_signal(&self) -> Vec<f32> {
        vec![0.0; self.length.get()]
    }

    /// A zeroed spectrum buffer of the bin count this plan produces.
    ///
    /// Allocates, so `setup` rather than `render`.
    #[must_use]
    pub fn new_spectrum(&self) -> Vec<FftBin> {
        vec![FftBin::ZERO; self.spectrum_len()]
    }

    /// Transforms `signal` into `spectrum`, unscaled.
    ///
    /// Bin `k` of the result is the sum over the window rather than an
    /// average, so a full-scale sine at bin `k` reads
    /// `length / 2` there. The inverse is where the `1 / length`
    /// comes back.
    ///
    /// Both lengths are exact — [`length`](Self::length) samples and
    /// [`spectrum_len`](Self::spectrum_len) bins, not "at least". A
    /// longer buffer is an error rather than a slice this takes the
    /// front of, because a caller who sized one differently meant
    /// something by it. [`new_signal`](Self::new_signal) and
    /// [`new_spectrum`](Self::new_spectrum) make buffers that fit.
    ///
    /// `signal` is taken by `&mut` because NE10's parameter is not
    /// `const`: the transform is entitled to use its input as scratch.
    /// On the board's build it does not, and that is measured rather
    /// than promised (`docs/fft.md`) — a program that needs its window
    /// afterwards should keep a copy rather than rely on it.
    ///
    /// Real-time safe: no allocation, no system call, nothing to block
    /// on. What it costs is arithmetic, and how much of a block that is
    /// depends on the length — `examples/fft.rs` measures it.
    ///
    /// # Errors
    ///
    /// [`Error::FftSignalLen`] when `signal` is not
    /// [`length`](Self::length) samples, and [`Error::FftSpectrumLen`]
    /// when `spectrum` is not [`spectrum_len`](Self::spectrum_len)
    /// bins. Both are checked before anything is transformed, `signal`
    /// first, so an error leaves both buffers exactly as they were.
    pub fn forward(&mut self, signal: &mut [f32], spectrum: &mut [FftBin]) -> Result<(), Error> {
        self.check_signal(signal.len())?;
        self.check_spectrum(spectrum.len())?;
        self.forward_raw(signal, spectrum);
        Ok(())
    }

    /// Transforms `spectrum` back into `signal`, scaled so that a
    /// [`forward`](Self::forward) followed by this one returns the
    /// original signal.
    ///
    /// The scaling is this crate's contract rather than the backend's:
    /// the forward transform is unscaled and the inverse carries the
    /// whole `1 / length`, whoever applies it. (On the board's NE10,
    /// NE10 does — measured, so this costs nothing here.)
    ///
    /// The same exact lengths as `forward`, and `spectrum` is `&mut`
    /// for the same reason `signal` is there.
    ///
    /// Real-time safe, on the same terms.
    ///
    /// # Errors
    ///
    /// The same two, checked the same way and with the same promise
    /// that a rejected call changes nothing. `spectrum` is checked
    /// first here, matching the argument order.
    pub fn inverse(&mut self, spectrum: &mut [FftBin], signal: &mut [f32]) -> Result<(), Error> {
        self.check_spectrum(spectrum.len())?;
        self.check_signal(signal.len())?;
        self.inverse_raw(spectrum, signal);
        Ok(())
    }

    /// The length check `forward` and `inverse` share for the signal.
    const fn check_signal(&self, actual: usize) -> Result<(), Error> {
        let expected = self.length.get();
        if actual == expected {
            Ok(())
        } else {
            Err(Error::FftSignalLen { expected, actual })
        }
    }

    /// The length check they share for the spectrum.
    const fn check_spectrum(&self, actual: usize) -> Result<(), Error> {
        let expected = self.spectrum_len();
        if actual == expected {
            Ok(())
        } else {
            Err(Error::FftSpectrumLen { expected, actual })
        }
    }

    #[cfg(bela_device)]
    fn forward_raw(&mut self, signal: &mut [f32], spectrum: &mut [FftBin]) {
        // Safety: the plan is live, this `&mut self` is the only
        // access to it for the duration, and the two slices were just
        // checked to be the lengths it transforms — `length` samples
        // in and `length / 2 + 1` bins out. `FftBin` is `#[repr(C)]`
        // with NE10's layout, asserted in `bela-sys/abi/ne10_abi.c`,
        // and the two slices cannot overlap: they are different types
        // reached through separate `&mut`s.
        unsafe {
            bela_sys::ne10_fft_r2c_1d_float32_neon(
                spectrum.as_mut_ptr().cast(),
                signal.as_mut_ptr(),
                self.plan.as_ptr(),
            );
        }
    }

    #[cfg(not(bela_device))]
    #[allow(
        clippy::unused_self,
        clippy::needless_pass_by_ref_mut,
        clippy::missing_const_for_fn,
        reason = "mirrors the device signature, which transforms through the plan"
    )]
    fn forward_raw(&mut self, _signal: &mut [f32], _spectrum: &mut [FftBin]) {}

    #[cfg(bela_device)]
    fn inverse_raw(&mut self, spectrum: &mut [FftBin], signal: &mut [f32]) {
        // Safety: as `forward_raw`, with the roles swapped.
        unsafe {
            bela_sys::ne10_fft_c2r_1d_float32_neon(
                signal.as_mut_ptr(),
                spectrum.as_mut_ptr().cast(),
                self.plan.as_ptr(),
            );
        }
    }

    #[cfg(not(bela_device))]
    #[allow(
        clippy::unused_self,
        clippy::needless_pass_by_ref_mut,
        clippy::missing_const_for_fn,
        reason = "mirrors the device signature, which transforms through the plan"
    )]
    fn inverse_raw(&mut self, _spectrum: &mut [FftBin], _signal: &mut [f32]) {}
}

#[cfg(all(test, not(bela_device)))]
impl RealFft {
    /// A plan for the host tests below.
    ///
    /// Off the device target a `RealFft` holds nothing but its length
    /// — there is no plan, which is why [`new`](Self::new) refuses —
    /// so everything except the transform itself can be exercised
    /// here: the buffer sizes, the length checks, the order they are
    /// made in, and what an error carries.
    const fn for_test(length: FftLength) -> Self {
        Self { length }
    }
}

impl fmt::Debug for RealFft {
    /// The length, rather than the pointer: one plan is like another,
    /// and the address says nothing a reader wants.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RealFft")
            .field("length", &self.length)
            .finish_non_exhaustive()
    }
}

impl Drop for RealFft {
    /// Frees the plan.
    ///
    /// One `free`, and it does not block — but it is still `free`, so
    /// a plan is not something to drop on the audio thread.
    fn drop(&mut self) {
        #[cfg(bela_device)]
        // Safety: `plan` came from `ne10_fft_alloc_r2c_float32`, is
        // freed once, and nothing else holds it or is transforming
        // with it.
        unsafe {
            bela_sys::ne10_fft_destroy_r2c_float32(self.plan.as_ptr());
        }
    }
}

#[cfg(test)]
mod tests {
    use core::f32::consts::FRAC_PI_2;

    use super::*;

    #[test]
    fn a_length_is_a_power_of_two_in_range() {
        for length in [8, 16, 1024, 65536] {
            assert_eq!(
                FftLength::new(length).map(FftLength::get),
                Some(length),
                "{length} is supported"
            );
        }
        for length in [0, 1, 3, 7, 9, 1000, 131_072] {
            assert_eq!(FftLength::new(length), None, "{length} is not");
        }
    }

    /// The two powers of two this crate refuses, and the whole reason
    /// `MIN` is 8: NE10 plans for them and then writes outside the
    /// buffers it was given (`docs/fft.md`).
    #[test]
    fn the_lengths_that_corrupt_memory_are_not_lengths() {
        assert_eq!(FftLength::new(2), None);
        assert_eq!(FftLength::new(4), None);
        assert_eq!(FftLength::MIN.get(), 8);
    }

    #[test]
    fn rounding_up_lands_on_a_supported_length() {
        for (asked, expected) in [
            (0, 8),
            (1, 8),
            (2, 8),
            (5, 8),
            (8, 8),
            (9, 16),
            (1000, 1024),
            (65536, 65536),
        ] {
            assert_eq!(
                FftLength::rounded_up(asked).map(FftLength::get),
                Some(expected),
                "rounding {asked} up"
            );
        }
        assert_eq!(FftLength::rounded_up(65537), None);
        assert_eq!(FftLength::rounded_up(usize::MAX), None);
    }

    #[test]
    fn a_spectrum_holds_dc_nyquist_and_what_is_between() {
        assert_eq!(FftLength::MIN.spectrum_len(), 5);
        assert_eq!(
            FftLength::new(1024)
                .expect("1024 is supported")
                .spectrum_len(),
            513
        );
        assert_eq!(FftLength::MAX.spectrum_len(), 32769);
    }

    #[test]
    fn a_length_converts_both_ways() {
        let length = FftLength::new(256).expect("256 is supported");
        assert_eq!(usize::from(length), 256);
        assert_eq!(FftLength::try_from(256_usize), Ok(length));
        assert_eq!(
            FftLength::try_from(3_usize),
            Err(Error::FftLength { value: 3 })
        );
        assert_eq!(length.to_string(), "256");
    }

    #[test]
    fn a_bin_is_two_floats_laid_out_as_ne10_lays_them() {
        assert_eq!(size_of::<FftBin>(), size_of::<f32>() * 2);
        assert_eq!(align_of::<FftBin>(), align_of::<f32>());
        assert_eq!(FftBin::ZERO, FftBin::default());
        assert_eq!(FftBin::from((1.0, 2.0)), FftBin::new(1.0, 2.0));
        assert_eq!(FftBin::from([1.0, 2.0]), FftBin::new(1.0, 2.0));
    }

    #[test]
    fn a_bin_reports_magnitude_and_phase() {
        let bin = FftBin::new(3.0, 4.0);
        assert!((bin.magnitude() - 5.0).abs() < 1e-6);
        assert!((bin.magnitude_squared() - 25.0).abs() < 1e-6);
        assert!((FftBin::new(0.0, 1.0).phase() - FRAC_PI_2).abs() < 1e-6);
        assert!(FftBin::ZERO.magnitude().abs() < f32::EPSILON);
    }

    /// Off the device target every plan fails the same way, which is
    /// what makes the rest of an application compile and test on the
    /// host.
    #[test]
    #[cfg(not(bela_device))]
    fn a_plan_needs_a_board() {
        let length = FftLength::new(64).expect("64 is supported");
        assert_eq!(RealFft::new(length).unwrap_err(), Error::FftUnavailable);
    }

    /// Off the device target every plan fails the same way, which is
    /// what makes the rest of an application compile and test on the
    /// host.
    #[cfg(not(bela_device))]
    mod host {
        use super::*;

        fn plan() -> RealFft {
            RealFft::for_test(FftLength::new(64).expect("64 is supported"))
        }

        #[test]
        fn a_plan_reports_what_it_transforms() {
            let fft = plan();
            assert_eq!(fft.length().get(), 64);
            assert_eq!(fft.spectrum_len(), 33);
            assert!(
                format!("{fft:?}").contains("64"),
                "the Debug names the length"
            );
        }

        #[test]
        fn the_buffers_a_plan_makes_are_the_ones_it_takes() {
            let fft = plan();
            let mut signal = fft.new_signal();
            let mut spectrum = fft.new_spectrum();

            assert_eq!(signal.len(), fft.length().get());
            assert_eq!(spectrum.len(), fft.spectrum_len());
            assert!(signal.iter().all(|sample| *sample == 0.0));
            assert!(spectrum.iter().all(|bin| *bin == FftBin::ZERO));

            let mut fft = fft;
            assert_eq!(fft.forward(&mut signal, &mut spectrum), Ok(()));
            assert_eq!(fft.inverse(&mut spectrum, &mut signal), Ok(()));
        }

        #[test]
        fn a_buffer_of_the_wrong_length_is_refused_and_says_which() {
            let mut fft = plan();
            let mut signal = fft.new_signal();
            let mut spectrum = fft.new_spectrum();

            assert_eq!(
                fft.forward(&mut signal[..63], &mut spectrum),
                Err(Error::FftSignalLen {
                    expected: 64,
                    actual: 63
                })
            );
            assert_eq!(
                fft.forward(&mut signal, &mut spectrum[..32]),
                Err(Error::FftSpectrumLen {
                    expected: 33,
                    actual: 32
                })
            );
            assert_eq!(
                fft.inverse(&mut spectrum[..32], &mut signal),
                Err(Error::FftSpectrumLen {
                    expected: 33,
                    actual: 32
                })
            );
            assert_eq!(
                fft.inverse(&mut spectrum, &mut signal[..63]),
                Err(Error::FftSignalLen {
                    expected: 64,
                    actual: 63
                })
            );
        }

        /// A longer buffer is refused too: a caller who sized one
        /// differently meant something by it, and transforming into
        /// the front of it would go along with the misunderstanding.
        #[test]
        fn a_longer_buffer_is_refused_as_well() {
            let mut fft = plan();
            let mut signal = vec![0.0; 65];
            let mut spectrum = vec![FftBin::ZERO; 34];

            assert!(matches!(
                fft.forward(&mut signal, &mut spectrum),
                Err(Error::FftSignalLen { actual: 65, .. })
            ));
            assert!(matches!(
                fft.inverse(&mut spectrum, &mut signal),
                Err(Error::FftSpectrumLen { actual: 34, .. })
            ));
        }

        /// The documented order, which is what makes a call with two
        /// wrong buffers report the same thing every time.
        #[test]
        fn the_check_order_follows_the_argument_order() {
            let mut fft = plan();
            let mut signal = vec![0.0; 8];
            let mut spectrum = vec![FftBin::ZERO; 8];

            assert!(
                matches!(
                    fft.forward(&mut signal, &mut spectrum),
                    Err(Error::FftSignalLen { .. })
                ),
                "forward takes the signal first"
            );
            assert!(
                matches!(
                    fft.inverse(&mut spectrum, &mut signal),
                    Err(Error::FftSpectrumLen { .. })
                ),
                "inverse takes the spectrum first"
            );
        }
    }
}
