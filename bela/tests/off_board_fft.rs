//! Testing DSP built on [`bela::RealFft`] without a board.
//!
//! [`bela::RealFft::new`] returns [`bela::Error::FftUnavailable`] off
//! the device target, because NE10 is on the board and nowhere else.
//! The arithmetic built *around* the transform — bin selection,
//! magnitude and phase work, thresholds, spectral gain — is not
//! device-specific, and this is how to run it on a laptop: a trait the
//! program owns, one implementation per backend, and the program's DSP
//! written against the trait.
//!
//! **The trait belongs in the program, not in `bela`.** That is not a
//! shortcut around a missing feature; it is the only arrangement that
//! does not break down. A trait in `bela` would be public API owed a
//! contract over numbers that are not the board's, and a trait in a
//! host-side crate would put a device build's user code behind a
//! host-side dependency. `docs/fft.md` records why this crate ships no
//! host FFT backend of its own.
//!
//! Three things the implementations below have to agree on, and one
//! they cannot:
//!
//! - **The scaling is the program's contract, not the backend's.**
//!   `forward` is unscaled and `inverse` restores the original
//!   amplitudes, whoever applies the `1 / length`. NE10 applies it
//!   itself; `realfft` normalises neither direction, so the host
//!   implementation applies it by hand. That disagreement is the
//!   reason this uses `realfft` rather than a DFT written on the spot:
//!   an implementation whose author picks the convention cannot show
//!   the convention being honoured.
//! - **The length range is the same on both sides.** [`FftLength`]
//!   already refuses what NE10 cannot transform safely, and the host
//!   side has no reason of its own to refuse those lengths. It refuses
//!   them anyway, by construction: a program that runs on a laptop and
//!   then fails on the board is worse than one that refuses the same
//!   lengths everywhere.
//! - **A refusal changes nothing.** Both backends check the lengths in
//!   argument order and then the endpoint bins, before either of them
//!   transforms anything.
//! - **The results will not match bit for bit** — different
//!   algorithms, different rounding, and NE10 runs `-ffast-math` code.
//!   So the tests below compare against what the DSP means, within a
//!   tolerance, and never against recorded values. A golden-value test
//!   written here would be asserting `realfft`'s arithmetic and
//!   claiming it was the board's.
//!
//! The tests run against whichever backend the target has, so a
//! `cargo test` on a laptop exercises `realfft` and one on a board
//! exercises NE10. On a board, run them once with `--test-threads=1`
//! before reading a failure: `cargo test` runs them in parallel by
//! default, and while that is sound — every plan here is its own, and
//! `docs/fft.md` measures four threads transforming at once — a
//! serial first run leaves nothing to rule out.
//!
//! In CI the device half is type-checked rather than run:
//! `clippy-aarch64` builds `--all-targets` for
//! `aarch64-unknown-linux-gnu`, which is what stops the two
//! implementations drifting apart.

use core::f32::consts::TAU;

use bela::{FftBin, FftLength};

#[cfg(not(bela_device))]
use std::sync::Arc;

#[cfg(not(bela_device))]
use realfft::num_complex::Complex32;
#[cfg(not(bela_device))]
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

/// How the program's own transform refuses a call.
///
/// One error for both backends, which is half the point of the trait:
/// the DSP handles the same failures wherever it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransformError {
    /// The signal buffer was not `length` samples.
    SignalLen { expected: usize, actual: usize },
    /// The spectrum buffer was not `length / 2 + 1` bins.
    SpectrumLen { expected: usize, actual: usize },
    /// A spectrum's DC or Nyquist bin carried a non-zero imaginary
    /// part, so it is not the spectrum of a real signal.
    ///
    /// This one exists because of `realfft`, and it is the interesting
    /// case in the whole file. `ComplexToReal::process_with_scratch`
    /// answers such a spectrum with `FftError::InputValues` — *after*
    /// transforming it and writing the output, to say the result may
    /// not be right. NE10 says nothing at all. Neither behaviour is
    /// one a program can act on, so both backends check the two bins
    /// up front and refuse before transforming. The divergence
    /// disappears, and the promise that a refusal leaves both buffers
    /// alone survives on both sides.
    EndpointNotReal,
}

/// A real-to-complex transform and its inverse, in the program's own
/// terms.
///
/// The shape is [`bela::RealFft`]'s, because there is no reason to
/// invent a different one: exact buffer lengths, `&mut` inputs because
/// a transform is entitled to use its input as scratch, and no
/// allocation in either direction.
trait Transform {
    /// The transform length this instance is for.
    fn length(&self) -> FftLength;

    /// Transforms `signal` into `spectrum`, unscaled.
    fn forward(
        &mut self,
        signal: &mut [f32],
        spectrum: &mut [FftBin],
    ) -> Result<(), TransformError>;

    /// Transforms `spectrum` back into `signal`, scaled so that a
    /// `forward` followed by this one returns the original signal.
    fn inverse(
        &mut self,
        spectrum: &mut [FftBin],
        signal: &mut [f32],
    ) -> Result<(), TransformError>;

    /// A zeroed signal buffer of the length this transform takes.
    ///
    /// Allocates, so `setup` rather than `render` — the same division
    /// [`bela::RealFft::new_signal`] is for, and the same reason to
    /// have it: the buffers a transform accepts should come from the
    /// transform rather than from a length written out twice.
    fn new_signal(&self) -> Vec<f32> {
        vec![0.0; self.length().get()]
    }

    /// A zeroed spectrum buffer of the bin count this transform
    /// produces.
    fn new_spectrum(&self) -> Vec<FftBin> {
        vec![FftBin::ZERO; self.length().spectrum_len()]
    }
}

/// The signal-length check both backends share.
const fn check_signal(length: FftLength, signal: &[f32]) -> Result<(), TransformError> {
    let expected = length.get();
    if signal.len() == expected {
        Ok(())
    } else {
        Err(TransformError::SignalLen {
            expected,
            actual: signal.len(),
        })
    }
}

/// The spectrum-length check both backends share.
const fn check_spectrum(length: FftLength, spectrum: &[FftBin]) -> Result<(), TransformError> {
    let expected = length.spectrum_len();
    if spectrum.len() == expected {
        Ok(())
    } else {
        Err(TransformError::SpectrumLen {
            expected,
            actual: spectrum.len(),
        })
    }
}

/// The endpoint check: DC is the first bin and Nyquist the last, and
/// a real signal's spectrum has no imaginary part in either.
///
/// Written with `first` and `last` rather than `[0]` and
/// `[len - 1]` so that it says nothing about when it is called. It
/// runs after the length checks today; indexing would make that
/// ordering load-bearing, and rearranging two lines would turn a
/// wrong length into a panic instead of an error.
#[allow(
    clippy::float_cmp,
    reason = "an endpoint bin has to be exactly zero — that is what a real \
              signal produces and what realfft requires of it"
)]
fn check_endpoints(spectrum: &[FftBin]) -> Result<(), TransformError> {
    let is_real = |bin: Option<&FftBin>| bin.is_none_or(|bin| bin.im == 0.0);
    if is_real(spectrum.first()) && is_real(spectrum.last()) {
        Ok(())
    } else {
        Err(TransformError::EndpointNotReal)
    }
}

// ---------------------------------------------------------------------
// The board: NE10, through `bela::RealFft`.
// ---------------------------------------------------------------------

#[cfg(bela_device)]
struct BoardTransform(bela::RealFft);

#[cfg(bela_device)]
impl BoardTransform {
    fn new(length: FftLength) -> Result<Self, bela::Error> {
        bela::RealFft::new(length).map(Self)
    }
}

#[cfg(bela_device)]
impl Transform for BoardTransform {
    fn length(&self) -> FftLength {
        self.0.length()
    }

    fn forward(
        &mut self,
        signal: &mut [f32],
        spectrum: &mut [FftBin],
    ) -> Result<(), TransformError> {
        // Checked here rather than left to `RealFft`, so that both
        // backends refuse the same calls with the same error. The
        // plan's own checks then pass by construction.
        check_signal(self.0.length(), signal)?;
        check_spectrum(self.0.length(), spectrum)?;
        self.0
            .forward(signal, spectrum)
            .expect("the lengths were just checked against this plan");
        Ok(())
    }

    fn inverse(
        &mut self,
        spectrum: &mut [FftBin],
        signal: &mut [f32],
    ) -> Result<(), TransformError> {
        check_spectrum(self.0.length(), spectrum)?;
        check_signal(self.0.length(), signal)?;
        check_endpoints(spectrum)?;
        // NE10 applies the `1 / length` itself, so nothing is scaled
        // here. `docs/fft.md` records that as a measurement rather
        // than a promise; if a board image stopped, the factor would
        // be applied here exactly as the host side applies it.
        self.0
            .inverse(spectrum, signal)
            .expect("the lengths were just checked against this plan");
        Ok(())
    }
}

// ---------------------------------------------------------------------
// A laptop: `realfft`.
// ---------------------------------------------------------------------

#[cfg(not(bela_device))]
struct HostTransform {
    length: FftLength,
    forward: Arc<dyn RealToComplex<f32>>,
    inverse: Arc<dyn ComplexToReal<f32>>,
    /// `FftBin` is not `Complex32`, so a transform copies through
    /// this. Allocated here for the same reason the scratch is: what
    /// this stands in for runs in `render`.
    bins: Vec<Complex32>,
    /// `process` would allocate its own scratch on every call.
    /// `process_with_scratch` is what keeps the transforms
    /// allocation-free, and it needs this.
    scratch: Vec<Complex32>,
}

#[cfg(not(bela_device))]
impl HostTransform {
    fn new(length: FftLength) -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let forward = planner.plan_fft_forward(length.get());
        let inverse = planner.plan_fft_inverse(length.get());
        let scratch =
            vec![Complex32::default(); forward.get_scratch_len().max(inverse.get_scratch_len())];
        Self {
            length,
            bins: vec![Complex32::default(); length.spectrum_len()],
            scratch,
            forward,
            inverse,
        }
    }
}

#[cfg(not(bela_device))]
impl Transform for HostTransform {
    fn length(&self) -> FftLength {
        self.length
    }

    fn forward(
        &mut self,
        signal: &mut [f32],
        spectrum: &mut [FftBin],
    ) -> Result<(), TransformError> {
        check_signal(self.length, signal)?;
        check_spectrum(self.length, spectrum)?;
        self.forward
            .process_with_scratch(signal, &mut self.bins, &mut self.scratch)
            .expect("the buffers are the lengths this plan was built for");
        // `realfft` leaves the forward transform unscaled, and so does
        // NE10, so there is nothing to apply on this side.
        for (bin, out) in self.bins.iter().zip(spectrum.iter_mut()) {
            *out = FftBin::new(bin.re, bin.im);
        }
        Ok(())
    }

    #[allow(
        clippy::cast_precision_loss,
        reason = "a length is a power of two from 8 to 65536, which f32 holds exactly"
    )]
    fn inverse(
        &mut self,
        spectrum: &mut [FftBin],
        signal: &mut [f32],
    ) -> Result<(), TransformError> {
        check_spectrum(self.length, spectrum)?;
        check_signal(self.length, signal)?;
        check_endpoints(spectrum)?;
        for (bin, out) in spectrum.iter().zip(self.bins.iter_mut()) {
            *out = Complex32::new(bin.re, bin.im);
        }
        self.inverse
            .process_with_scratch(&mut self.bins, signal, &mut self.scratch)
            .expect("the endpoints and the buffer lengths were just checked");
        // Here is the contract being paid for by hand. `realfft` "does
        // not normalize the output of either forward or inverse FFT",
        // where NE10 applies the `1 / length` to the inverse itself.
        // Without this loop a round trip would come back `length`
        // times too loud on a laptop and correct on the board, which
        // is the exact shape of bug this whole file exists to prevent.
        let scale = 1.0 / self.length.get() as f32;
        for sample in signal.iter_mut() {
            *sample *= scale;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------
// The DSP, and the tests. Neither knows which backend it has.
// ---------------------------------------------------------------------

/// The transform this target can build.
///
/// The only `cfg` the program's own code needs: everything below is
/// written against [`Transform`].
#[cfg(bela_device)]
fn transform(length: FftLength) -> impl Transform {
    BoardTransform::new(length).expect("NE10 plans every length FftLength holds")
}

/// The transform this target can build. See the device version above.
#[cfg(not(bela_device))]
fn transform(length: FftLength) -> impl Transform {
    HostTransform::new(length)
}

/// A length every test here uses, short enough to read and long enough
/// to have bins worth selecting.
const fn length() -> FftLength {
    match FftLength::new(64) {
        Some(length) => length,
        None => unreachable!(),
    }
}

/// `amplitude * cos(2π · bin · n / length)`, which puts all of its
/// energy in one bin.
#[allow(
    clippy::cast_precision_loss,
    reason = "sample indices and bin numbers here are far below f32's integer range"
)]
fn cosine(length: FftLength, bin: usize, amplitude: f32) -> Vec<f32> {
    let n = length.get();
    (0..n)
        .map(|i| {
            let phase = TAU * (bin * i) as f32 / n as f32;
            amplitude * phase.cos()
        })
        .collect()
}

/// The worst absolute difference between two signals.
fn worst_difference(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        // Not `fold(0.0, f32::max)`. That one returns the *other*
        // argument when one of them is NaN, so a backend answering
        // with nothing but NaN would read as a difference of zero and
        // pass every tolerance below. NaN is how a broken transform
        // usually arrives — uninitialised scratch, a division by a
        // zero length, a lost code path — which makes it the one
        // result these tests must not swallow.
        .fold(0.0_f32, |worst, difference| {
            if difference.is_nan() {
                f32::INFINITY
            } else {
                worst.max(difference)
            }
        })
}

/// A tolerance both backends meet with room to spare, and far too
/// loose to be a golden value: `docs/fft.md` measures NE10's round
/// trip at 2.98e-7.
const TOLERANCE: f32 = 1e-4;

/// The scaling contract, from the forward side: unscaled, so a
/// full-scale cosine at bin `k` reads `length / 2` there.
///
/// This is the test that fails if a backend's normalisation is taken
/// on trust.
#[test]
#[allow(
    clippy::cast_precision_loss,
    reason = "a length is a power of two from 8 to 65536, which f32 holds exactly"
)]
fn a_forward_transform_is_unscaled() {
    let length = length();
    let mut fft = transform(length);
    let mut signal = cosine(length, 6, 1.0);
    let mut spectrum = fft.new_spectrum();

    fft.forward(&mut signal, &mut spectrum)
        .expect("the buffers came from the plan's own lengths");

    let expected = length.get() as f32 / 2.0;
    assert!(
        (spectrum[6].magnitude() - expected).abs() < expected * 1e-4,
        "bin 6 should read length / 2 = {expected}, not {}",
        spectrum[6].magnitude()
    );
    for (index, bin) in spectrum.iter().enumerate() {
        if index != 6 {
            assert!(
                bin.magnitude() < expected * 1e-4,
                "bin {index} should be empty, and reads {}",
                bin.magnitude()
            );
        }
    }
}

/// The scaling contract, from the other side: `inverse` restores the
/// original amplitudes, whichever backend applies the factor.
#[test]
fn a_round_trip_returns_the_original_signal() {
    let length = length();
    let mut fft = transform(length);
    let original = cosine(length, 3, 0.75);
    let mut signal = original.clone();
    let mut spectrum = fft.new_spectrum();

    fft.forward(&mut signal, &mut spectrum)
        .expect("the buffers came from the plan's own lengths");
    // Both transforms are entitled to use their input as scratch, so
    // the round trip reads from `original` rather than from `signal`.
    let mut returned = fft.new_signal();
    fft.inverse(&mut spectrum, &mut returned)
        .expect("a real signal's spectrum has real endpoints");

    let worst = worst_difference(&original, &returned);
    assert!(
        worst < TOLERANCE,
        "a round trip should return the original within {TOLERANCE}, and differs by {worst}"
    );
}

/// The DSP this whole arrangement is for: a spectral operation with a
/// result the test can state without knowing whose arithmetic ran.
#[test]
fn a_spectral_low_pass_keeps_what_it_should() {
    let length = length();
    let mut fft = transform(length);
    let low = cosine(length, 3, 0.5);
    let high = cosine(length, 20, 0.5);
    let mut signal: Vec<f32> = low.iter().zip(&high).map(|(a, b)| a + b).collect();
    let mut spectrum = fft.new_spectrum();

    fft.forward(&mut signal, &mut spectrum)
        .expect("the buffers came from the plan's own lengths");

    // The program's own DSP, and the only part worth testing off the
    // board: everything above bin 10 goes.
    for bin in &mut spectrum[10..] {
        *bin = FftBin::ZERO;
    }

    let mut filtered = fft.new_signal();
    fft.inverse(&mut spectrum, &mut filtered)
        .expect("zeroing bins leaves the endpoints real");

    let worst = worst_difference(&low, &filtered);
    assert!(
        worst < TOLERANCE,
        "the partial below the cutoff should survive alone, and differs by {worst}"
    );
}

/// A refusal leaves both buffers as they were, on either backend.
#[test]
#[allow(
    clippy::float_cmp,
    reason = "a buffer a refused call must not have touched still holds \
              exactly what was put in it"
)]
fn a_buffer_of_the_wrong_length_is_refused_and_changes_nothing() {
    let length = length();
    let mut fft = transform(length);
    let mut signal = vec![1.0_f32; length.get() + 1];
    let mut spectrum = vec![FftBin::new(2.0, 3.0); fft.length().spectrum_len()];

    assert_eq!(
        fft.forward(&mut signal, &mut spectrum),
        Err(TransformError::SignalLen {
            expected: length.get(),
            actual: length.get() + 1,
        }),
        "a signal one sample too long is refused, and says which buffer"
    );
    assert!(
        signal.iter().all(|&sample| sample == 1.0),
        "a refused call transforms nothing"
    );
    assert!(
        spectrum.iter().all(|&bin| bin == FftBin::new(2.0, 3.0)),
        "a refused call writes nothing"
    );
}

/// The length checks run in argument order, so the error that comes
/// back names the buffer to look at first.
///
/// The counterpart of `the_check_order_follows_the_argument_order` in
/// `bela`'s own tests, and the only thing that holds the order the
/// module documentation promises: with one buffer wrong there is
/// nothing to order.
#[test]
fn the_check_order_follows_the_argument_order() {
    let length = length();
    let mut fft = transform(length);
    let mut signal = vec![0.0_f32; length.get() + 1];
    let mut spectrum = vec![FftBin::ZERO; length.spectrum_len() + 1];

    // `forward(signal, spectrum)` takes the signal first.
    assert_eq!(
        fft.forward(&mut signal, &mut spectrum),
        Err(TransformError::SignalLen {
            expected: length.get(),
            actual: length.get() + 1,
        }),
        "with both buffers wrong, forward reports the one it takes first"
    );
    // `inverse(spectrum, signal)` takes the spectrum first.
    assert_eq!(
        fft.inverse(&mut spectrum, &mut signal),
        Err(TransformError::SpectrumLen {
            expected: length.spectrum_len(),
            actual: length.spectrum_len() + 1,
        }),
        "with both buffers wrong, inverse reports the one it takes first"
    );
}

/// The endpoint check: a spectrum no real signal could have is refused
/// before anything is transformed, rather than after, and identically
/// on both backends.
#[test]
#[allow(
    clippy::float_cmp,
    reason = "a buffer a refused call must not have touched still holds \
              exactly what was put in it"
)]
fn a_spectrum_with_a_complex_endpoint_is_refused() {
    let length = length();
    let mut fft = transform(length);
    let mut spectrum = fft.new_spectrum();
    spectrum[0] = FftBin::new(1.0, 0.5);
    let mut signal = vec![7.0_f32; length.get()];

    assert_eq!(
        fft.inverse(&mut spectrum, &mut signal),
        Err(TransformError::EndpointNotReal),
        "a DC bin with an imaginary part is not a real signal's spectrum"
    );
    assert!(
        signal.iter().all(|&sample| sample == 7.0),
        "the refusal came before the transform, not after it"
    );

    // The same spectrum with its endpoints made real is accepted, so
    // the test is about the endpoints rather than about the length.
    spectrum[0] = FftBin::new(1.0, 0.0);
    assert_eq!(
        fft.inverse(&mut spectrum, &mut signal),
        Ok(()),
        "a real DC bin is fine"
    );
}
