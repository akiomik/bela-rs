//! Measures what NE10's real-to-complex FFT does on a board.
//!
//! Run it with `scripts/probe-fft.sh`, which builds it, copies it over
//! and keeps the output. The findings belong in `docs/fft.md`, next to
//! the `libNE10.so.10` build ID they were measured against
//! (`bela-sys/vendor/ne10/SOURCE`): a rebuilt library can answer
//! differently, and only the board can say.
//!
//! This is an experiment, not a check, the way `scripts/probe-io.sh`
//! is: every question below has an expected answer read from upstream
//! sources, and the point is what *this* build does. It exits non-zero
//! only when it could not ask — an allocation that failed, a length
//! that would not plan — never because an answer was surprising.
//!
//! No audio system is created: NE10 is a plain C library and nothing
//! here calls libbela. So the "one audio system per process" rule does
//! not apply, the whole length sweep fits in one run, and a crash
//! leaves no process holding the audio device.
//!
//! What it answers, in the order the questions are numbered in the
//! design note and in issue #138:
//!
//! 1. Does the inverse transform write into its input?
//! 2. Does the forward transform write into its input?
//! 3. Is the inverse scaled by `1/N`, or must the caller scale?
//! 4. Which lengths allocate *and* transform correctly?
//! 5. Does the alignment of the caller's buffers matter?
//! 6. Does the forward transform write exactly `N/2 + 1` bins?

fn main() {
    imp::main();
}

#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
mod imp {
    use core::f32::consts::PI;
    use std::process;

    use bela_sys::{
        ne10_fft_alloc_r2c_float32, ne10_fft_c2r_1d_float32_neon, ne10_fft_cpx_float32_t,
        ne10_fft_destroy_r2c_float32, ne10_fft_r2c_1d_float32_neon, ne10_fft_r2c_state_float32_t,
    };

    /// The length the questions that need only one are asked at: long
    /// enough to be a real transform, short enough to print.
    const N: usize = 16;

    /// Written past the end of what a transform should touch, and
    /// looked for afterwards. Not a number any of these transforms
    /// produces.
    const CANARY: f32 = -98765.5;

    /// How far a value may be from the expected one and still count as
    /// it. Generous: this asks which of two behaviours a build has,
    /// not how accurate it is.
    const EPSILON: f32 = 1e-3;

    /// An NE10 plan, freed on the way out.
    struct Plan {
        cfg: *mut ne10_fft_r2c_state_float32_t,
        length: usize,
    }

    impl Plan {
        fn new(length: usize) -> Option<Self> {
            let nfft = i32::try_from(length).ok()?;
            // Safety: `nfft` is a power of two in the range the sweep
            // below establishes, which is what the declaration asks
            // for; a null result is handled.
            let cfg = unsafe { ne10_fft_alloc_r2c_float32(nfft) };
            (!cfg.is_null()).then_some(Self { cfg, length })
        }

        /// `signal` (`length` samples) into `spectrum`
        /// (`length / 2 + 1` bins).
        fn forward(&self, signal: &mut [f32], spectrum: &mut [ne10_fft_cpx_float32_t]) {
            assert_eq!(signal.len(), self.length, "the signal is the plan's length");
            assert!(
                spectrum.len() > self.length / 2,
                "the spectrum holds every bin"
            );
            // Safety: the plan is live and used by this thread alone,
            // the two buffers are ours, distinct, and long enough —
            // the spectrum deliberately longer, so that finding 6 can
            // look past the last bin.
            unsafe {
                ne10_fft_r2c_1d_float32_neon(spectrum.as_mut_ptr(), signal.as_mut_ptr(), self.cfg);
            }
        }

        /// `spectrum` back into `signal`.
        fn inverse(&self, spectrum: &mut [ne10_fft_cpx_float32_t], signal: &mut [f32]) {
            assert_eq!(signal.len(), self.length, "the signal is the plan's length");
            assert!(
                spectrum.len() > self.length / 2,
                "the spectrum holds every bin"
            );
            // Safety: as above, with the roles swapped.
            unsafe {
                ne10_fft_c2r_1d_float32_neon(signal.as_mut_ptr(), spectrum.as_mut_ptr(), self.cfg);
            }
        }
    }

    impl Drop for Plan {
        fn drop(&mut self) {
            // Safety: the pointer came from the allocator, is not null,
            // and no transform is in flight — this is the only owner.
            unsafe { ne10_fft_destroy_r2c_float32(self.cfg) };
        }
    }

    /// A spectrum buffer with room for one bin past the last, holding
    /// the canary.
    fn spectrum_with_canary(length: usize) -> Vec<ne10_fft_cpx_float32_t> {
        let mut bins = vec![
            ne10_fft_cpx_float32_t {
                r: CANARY,
                i: CANARY
            };
            length / 2 + 2
        ];
        bins[..=(length / 2)].fill(ne10_fft_cpx_float32_t::default());
        bins
    }

    /// A cosine at `bin` cycles per window, amplitude 1.
    #[allow(
        clippy::cast_precision_loss,
        reason = "every length here is a power of two up to 65536, exact in f32"
    )]
    fn cosine(length: usize, bin: usize) -> Vec<f32> {
        (0..length)
            .map(|n| {
                let phase = 2.0 * PI * bin as f32 * n as f32 / length as f32;
                phase.cos()
            })
            .collect()
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() <= EPSILON * b.abs().max(1.0)
    }

    /// A transform length as the float the expected magnitudes are in
    /// terms of.
    #[allow(
        clippy::cast_precision_loss,
        reason = "every length here is a power of two up to 65536, exact in f32"
    )]
    const fn as_float(length: usize) -> f32 {
        length as f32
    }

    /// One finding, in the two columns `scripts/probe-io.sh` reports in.
    fn finding(question: &str, answer: &str) {
        println!("  {question:<28}{answer}");
    }

    pub(crate) fn main() {
        println!("ne10_probe: what this board's libNE10 does");
        println!(
            "record the answers in docs/fft.md against the build id in \
             bela-sys/vendor/ne10/SOURCE"
        );

        let Some(plan) = Plan::new(N) else {
            eprintln!("could not allocate a plan of {N} points; nothing can be asked");
            process::exit(1);
        };

        let reference = scratch_use(&plan);
        inverse_scaling(&plan);
        bins_written(&reference);
        alignment(&plan, &reference);
        drop(plan);
        length_sweep();

        println!();
        println!("done");
    }

    /// Questions 1 and 2: does either transform write into the buffer
    /// it reads? Answers with the spectrum of a known signal, which
    /// the questions after this one compare against.
    fn scratch_use(plan: &Plan) -> Vec<ne10_fft_cpx_float32_t> {
        println!();
        println!("1/2. does a transform write into its input? (N = {N})");

        let signal = cosine(N, 1);
        let mut scratch = signal.clone();
        let mut spectrum = spectrum_with_canary(N);
        plan.forward(&mut scratch, &mut spectrum);
        finding(
            "forward, its signal",
            if scratch == signal {
                "preserved"
            } else {
                "WRITTEN INTO"
            },
        );

        let reference = spectrum.clone();
        let mut restored = vec![0.0; N];
        plan.inverse(&mut spectrum, &mut restored);
        finding(
            "inverse, its spectrum",
            if spectrum == reference {
                "preserved"
            } else {
                "WRITTEN INTO"
            },
        );
        // Which bins, if it did: upstream's c2r writes into bin 0 and
        // reads the Nyquist bin, and "all of them" would be a
        // different API on the Rust side.
        if spectrum != reference {
            let changed: Vec<usize> = (0..=(N / 2))
                .filter(|&bin| spectrum[bin] != reference[bin])
                .collect();
            finding(
                "  bins it changed",
                &format!("{changed:?} of 0..={}", N / 2),
            );
        }
        reference
    }

    /// Question 3: does the inverse apply the `1/N`, or must a caller?
    fn inverse_scaling(plan: &Plan) {
        println!();
        println!("3. is the inverse scaled? (round trip of a unit cosine)");

        let mut signal = cosine(N, 1);
        let mut spectrum = spectrum_with_canary(N);
        plan.forward(&mut signal, &mut spectrum);
        let mut restored = vec![0.0; N];
        plan.inverse(&mut spectrum, &mut restored);

        let peak = restored
            .iter()
            .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
        finding("round-trip peak", &format!("{peak:.6} (input was 1.0)"));
        finding(
            "so the inverse",
            if close(peak, 1.0) {
                "applies 1/N itself"
            } else if close(peak, as_float(N)) {
                "does NOT scale; the caller must apply 1/N"
            } else {
                "neither 1 nor N: read the number above"
            },
        );
    }

    /// Question 6: exactly `N / 2 + 1` bins, or more?
    fn bins_written(reference: &[ne10_fft_cpx_float32_t]) {
        println!();
        println!("6. how many bins does the forward transform write?");

        let past_the_end = reference[N / 2 + 1];
        finding(
            "past bin N/2",
            if past_the_end.r == CANARY && past_the_end.i == CANARY {
                "untouched, so exactly N/2 + 1 bins"
            } else {
                "OVERWRITTEN, so more than N/2 + 1 bins"
            },
        );
    }

    /// Question 5: does a buffer aligned only as a `f32` transform the
    /// same as one `malloc` handed out?
    fn alignment(plan: &Plan, reference: &[ne10_fft_cpx_float32_t]) {
        println!();
        println!("5. does the caller's alignment matter?");

        // The window offset by one f32 inside a larger allocation: 4
        // bytes of alignment, and neither 8 nor 16.
        let mut backing = [0.0_f32; N + 1];
        backing[1..].copy_from_slice(&cosine(N, 1));
        let mut spectrum = spectrum_with_canary(N);
        plan.forward(&mut backing[1..], &mut spectrum);

        let same = (0..=(N / 2)).all(|bin| close(spectrum[bin].r, reference[bin].r));
        finding(
            "4-byte aligned input",
            if same {
                "same spectrum as an aligned one"
            } else {
                "DIFFERENT; the API must own aligned buffers"
            },
        );
    }

    /// Question 4: which lengths plan, and of those, which transform
    /// correctly?
    fn length_sweep() {
        println!();
        println!("4. which lengths plan and transform correctly?");

        let mut planned = Vec::new();
        let mut correct = Vec::new();
        let mut length = 2;
        while length <= 65536 {
            let Some(plan) = Plan::new(length) else {
                println!("  {length:<7}no plan");
                length *= 2;
                continue;
            };
            planned.push(length);
            let wrong = check_length(&plan, length);
            if wrong.is_empty() {
                correct.push(length);
            } else {
                println!("  {length:<7}{}", wrong.join(", "));
            }
            length *= 2;
        }
        finding("planned", &format!("{planned:?}"));
        finding("and transformed correctly", &format!("{correct:?}"));
    }

    /// The known signals a length is checked with, as the ones that
    /// exist at that length: `N = 2` has only bins 0 and 1, so a
    /// single-bin cosine is not one of its cases.
    fn check_length(plan: &Plan, length: usize) -> Vec<String> {
        let mut wrong = Vec::new();
        let mut spectrum = spectrum_with_canary(length);

        // DC: every sample 1, so bin 0 is N and every other bin 0.
        let mut signal = vec![1.0_f32; length];
        plan.forward(&mut signal, &mut spectrum);
        let expected = as_float(length);
        if !close(spectrum[0].r, expected) {
            wrong.push(format!("DC bin 0 is {} not {expected}", spectrum[0].r));
        }
        if (1..=length / 2).any(|bin| !close(spectrum[bin].r, 0.0)) {
            wrong.push("DC leaked into another bin".to_owned());
        }

        // Nyquist: alternating +1/-1, so all the energy in the last bin.
        let mut signal: Vec<f32> = (0..length)
            .map(|n| if n % 2 == 0 { 1.0 } else { -1.0 })
            .collect();
        plan.forward(&mut signal, &mut spectrum);
        if !close(spectrum[length / 2].r, expected) {
            wrong.push(format!(
                "Nyquist bin {} is {} not {expected}",
                length / 2,
                spectrum[length / 2].r
            ));
        }

        // A cosine on one bin, where there is one to put it on. Its
        // bin holds N/2 for a unit amplitude, the rest nothing.
        if length >= 4 {
            let mut signal = cosine(length, 1);
            plan.forward(&mut signal, &mut spectrum);
            if !close(spectrum[1].r, expected / 2.0) {
                wrong.push(format!(
                    "bin 1 of a unit cosine is {} not {}",
                    spectrum[1].r,
                    expected / 2.0
                ));
            }
            if (2..=length / 2).any(|bin| !close(spectrum[bin].r, 0.0)) {
                wrong.push("the cosine leaked into another bin".to_owned());
            }
        }

        let last = spectrum[length / 2 + 1];
        if last.r != CANARY || last.i != CANARY {
            wrong.push("wrote past bin N/2".to_owned());
        }
        wrong
    }
}

#[cfg(not(all(target_arch = "aarch64", target_os = "linux")))]
mod imp {
    pub(crate) fn main() {
        // Built on the host by `cargo test`, which builds every
        // example. NE10 is on the board and nowhere else, so there is
        // nothing here to call and nothing to link against.
        eprintln!(
            "ne10_probe asks a board what its libNE10 does; \
             build it for aarch64-unknown-linux-gnu and run it there \
             (scripts/probe-fft.sh)"
        );
    }
}
