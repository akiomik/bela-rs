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
    use std::{env, process};

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

    /// How much room past the end of each buffer the canary fills.
    /// Wide enough that a transform overrunning by a vector's worth is
    /// caught here rather than in the allocator, which reports it as
    /// `free(): invalid pointer` from wherever the next free happens
    /// to be.
    const SLACK: usize = 8;

    /// How far a value may be from the expected one and still count as
    /// it. Generous: this asks which of two behaviours a build has,
    /// not how accurate it is.
    const EPSILON: f32 = 1e-3;

    /// An NE10 plan, freed on the way out.
    ///
    /// The transforms take `&mut self` because the scratch buffer they
    /// write through lives in the plan, which is the same reason the
    /// safe API in #138 will: one plan has one user at a time.
    struct Plan {
        cfg: *mut ne10_fft_r2c_state_float32_t,
        length: usize,
    }

    impl Plan {
        fn new(length: usize) -> Option<Self> {
            // The allocator's contract, checked here rather than
            // assumed of a command line: a power of two from 2 to
            // 65536. Not 8, which is where *transforming* becomes
            // safe — allocating at 2 and 4 is fine, and asking what
            // they do is the point of this program.
            if !length.is_power_of_two() || !(2..=65536).contains(&length) {
                return None;
            }
            let nfft = i32::try_from(length).ok()?;
            // Safety: `nfft` is a power of two in that range, which is
            // what the declaration asks of the allocator; a null
            // result is handled.
            let cfg = unsafe { ne10_fft_alloc_r2c_float32(nfft) };
            (!cfg.is_null()).then_some(Self { cfg, length })
        }

        /// `signal` (`length` samples) into `spectrum`
        /// (`length / 2 + 1` bins).
        fn forward(&mut self, signal: &mut [f32], spectrum: &mut [ne10_fft_cpx_float32_t]) {
            assert!(
                signal.len() >= self.length,
                "the signal holds the plan's length, and may hold canaries past it"
            );
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
        fn inverse(&mut self, spectrum: &mut [ne10_fft_cpx_float32_t], signal: &mut [f32]) {
            assert!(
                signal.len() >= self.length,
                "the signal holds the plan's length, and may hold canaries past it"
            );
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

    /// A spectrum buffer with [`SLACK`] bins of canary past the last
    /// one a transform should write.
    fn spectrum_with_canary(length: usize) -> Vec<ne10_fft_cpx_float32_t> {
        let mut bins = vec![
            ne10_fft_cpx_float32_t {
                r: CANARY,
                i: CANARY
            };
            length / 2 + 1 + SLACK
        ];
        bins[..=(length / 2)].fill(ne10_fft_cpx_float32_t::default());
        bins
    }

    /// A signal buffer holding `samples`, with [`SLACK`] floats of
    /// canary past the end.
    ///
    /// The transforms take the whole `length` and no more; anything
    /// they write past it lands in the canary rather than in the
    /// allocator's bookkeeping, which is the difference between an
    /// answer and an abort.
    fn signal_with_canary(samples: &[f32]) -> Vec<f32> {
        let mut signal = vec![CANARY; samples.len() + SLACK];
        signal[..samples.len()].copy_from_slice(samples);
        signal
    }

    /// Whether every canary past `used` survived.
    fn canary_intact(signal: &[f32], used: usize) -> bool {
        signal[used..].iter().all(|sample| *sample == CANARY)
    }

    /// Whether every canary bin past `used` survived.
    fn bin_canary_intact(spectrum: &[ne10_fft_cpx_float32_t], used: usize) -> bool {
        spectrum[used..]
            .iter()
            .all(|bin| bin.r == CANARY && bin.i == CANARY)
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

    /// Whether `value` is `expected`, to a tolerance set by the
    /// transform's own scale rather than by the value itself.
    ///
    /// A bin that should hold nothing cannot be judged against zero
    /// with a fixed tolerance: an unscaled forward transform of `N`
    /// points sums `N` products, so the rounding noise in the bins
    /// that cancel grows with `N` while the bins that do not stay at
    /// `N` or `N/2`. At 65536 points that noise passes a fixed 1e-3
    /// and a leak is still three orders of magnitude above it.
    fn close_to(value: f32, expected: f32, scale: f32) -> bool {
        (value - expected).abs() <= EPSILON * scale
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
        // With a length, only that length is asked about: one process
        // per length is how a sweep survives a length that corrupts
        // the heap, which is a thing this build does.
        let mut arguments = env::args().skip(1);
        if let Some(argument) = arguments.next() {
            let Ok(length) = argument.parse::<usize>() else {
                eprintln!("usage: ne10_probe [length [plan-only|overrun]]");
                process::exit(2);
            };
            // `plan-only` allocates and frees without transforming,
            // which is how a length that ends the process is narrowed
            // down: a plan that cannot even be freed is a different
            // fault from a transform that corrupts the heap.
            process::exit(match arguments.next().as_deref() {
                Some("plan-only") => one_length_plan_only(length),
                Some("overrun") => one_length_overrun(length),
                _ => one_length(length),
            });
        }

        println!("ne10_probe: what this board's libNE10 does");
        println!(
            "record the answers in docs/fft.md against the build id in \
             bela-sys/vendor/ne10/SOURCE"
        );
        println!("run it with a length for question 4: ne10_probe <length>");

        let Some(mut plan) = Plan::new(N) else {
            eprintln!("could not allocate a plan of {N} points; nothing can be asked");
            process::exit(1);
        };

        let reference = scratch_use(&mut plan);
        inverse_scaling(&mut plan);
        bins_written(&reference);
        alignment(&mut plan, &reference);

        println!();
        println!("done");
    }

    /// Questions 1 and 2: does either transform write into the buffer
    /// it reads? Answers with the spectrum of a known signal, which
    /// the questions after this one compare against.
    fn scratch_use(plan: &mut Plan) -> Vec<ne10_fft_cpx_float32_t> {
        println!();
        println!("1/2. does a transform write into its input? (N = {N})");

        let signal = cosine(N, 1);
        let mut scratch = signal_with_canary(&signal);
        let mut spectrum = spectrum_with_canary(N);
        plan.forward(&mut scratch, &mut spectrum);
        finding(
            "forward, its signal",
            if scratch[..N] == signal[..] {
                "preserved"
            } else {
                "WRITTEN INTO"
            },
        );

        let reference = spectrum.clone();
        let mut restored = signal_with_canary(&[0.0; N]);
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
    fn inverse_scaling(plan: &mut Plan) {
        println!();
        println!("3. is the inverse scaled? (round trip of a unit cosine)");

        let mut signal = signal_with_canary(&cosine(N, 1));
        let mut spectrum = spectrum_with_canary(N);
        plan.forward(&mut signal, &mut spectrum);
        let mut restored = signal_with_canary(&[0.0; N]);
        plan.inverse(&mut spectrum, &mut restored);

        let peak = restored[..N]
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

        finding(
            "past bin N/2",
            if bin_canary_intact(reference, N / 2 + 1) {
                "untouched, so exactly N/2 + 1 bins"
            } else {
                "OVERWRITTEN, so more than N/2 + 1 bins"
            },
        );
    }

    /// Question 5: does a buffer aligned only as a `f32` transform the
    /// same as one `malloc` handed out?
    fn alignment(plan: &mut Plan, reference: &[ne10_fft_cpx_float32_t]) {
        println!();
        println!("5. does the caller's alignment matter?");

        // The window offset by one f32 inside a larger allocation: 4
        // bytes of alignment, and neither 8 nor 16.
        let mut backing = signal_with_canary(&cosine(N, 1));
        backing.insert(0, CANARY);
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

    /// Question 4 without the transforms: does a plan of this length
    /// survive being allocated and freed?
    ///
    /// Returns the process's exit status, 0 for yes and 2 for a length
    /// that would not plan. A length whose plan cannot be freed takes
    /// the process with it and prints nothing after the first line.
    fn one_length_plan_only(length: usize) -> i32 {
        println!("{length}: allocating");
        let Some(plan) = Plan::new(length) else {
            println!("{length}: no plan");
            return 2;
        };
        println!("{length}: allocated, freeing");
        drop(plan);
        println!("{length}: ok, plan allocated and freed");
        0
    }

    /// How far past its buffers a transform writes, measured with
    /// enough canary after each that the overrun lands in the probe's
    /// own memory rather than in the allocator's bookkeeping — which
    /// is what turns a `free(): invalid pointer` abort into a number.
    ///
    /// Returns the process's exit status: 0 when nothing was written
    /// past either buffer, 1 when something was, 2 for a length that
    /// would not plan.
    fn one_length_overrun(length: usize) -> i32 {
        // Wide enough to hold any plausible overrun on either side: a
        // whole extra transform's worth, and never less than a page.
        // Both buffers sit in the middle of a larger allocation, so
        // that a write before the start lands in canary rather than in
        // the chunk header the allocator reads back at `free`.
        let slack = (length * 4).max(1024);

        let Some(mut plan) = Plan::new(length) else {
            println!("{length}: no plan");
            return 2;
        };

        let mut signal = vec![CANARY; slack + length + slack];
        signal[slack..slack + length].fill(1.0);
        let mut spectrum = vec![
            ne10_fft_cpx_float32_t {
                r: CANARY,
                i: CANARY
            };
            slack + length / 2 + 1 + slack
        ];
        spectrum[slack..=(slack + length / 2)].fill(ne10_fft_cpx_float32_t::default());

        plan.forward(&mut signal[slack..], &mut spectrum[slack..]);

        let is_written = |sample: &f32| *sample != CANARY;
        let bin_is_written = |bin: &ne10_fft_cpx_float32_t| bin.r != CANARY || bin.i != CANARY;
        println!(
            "{length}: forward wrote {} f32 before and {} past its {length}-sample input",
            written_before(&signal[..slack], is_written),
            written_past(&signal[slack + length..], is_written),
        );
        println!(
            "{length}: forward wrote {} bin(s) before bin 0 and {} past bin {}",
            written_before(&spectrum[..slack], bin_is_written),
            written_past(&spectrum[slack + length / 2 + 1..], bin_is_written),
            length / 2,
        );

        let mut restored = vec![CANARY; slack + length + slack];
        plan.inverse(&mut spectrum[slack..], &mut restored[slack..]);
        println!(
            "{length}: inverse wrote {} f32 before and {} past its {length}-sample output",
            written_before(&restored[..slack], is_written),
            written_past(&restored[slack + length..], is_written),
        );

        // Which allocation the damage is in, if the run ends here: the
        // last line printed names the free that aborted.
        println!("{length}: freeing the restored signal");
        drop(restored);
        println!("{length}: freeing the spectrum");
        drop(spectrum);
        println!("{length}: freeing the signal");
        drop(signal);
        println!("{length}: freeing the plan");
        drop(plan);
        println!("{length}: every allocation freed");
        0
    }

    /// How many elements before the buffer's end were changed, counted
    /// from the last one: the canary immediately before what a
    /// transform was given is index `len - 1` here, so a return of 3
    /// means it wrote three elements before the start.
    fn written_before<T>(before: &[T], changed: impl Fn(&T) -> bool) -> usize {
        before
            .iter()
            .position(&changed)
            .map_or(0, |index| before.len() - index)
    }

    /// How far into `after` — the canary past what a transform was
    /// given — anything was changed, counted to the last element that
    /// was: a transform that skips one and writes the next has still
    /// written that far.
    fn written_past<T>(after: &[T], changed: impl Fn(&T) -> bool) -> usize {
        after.iter().rposition(changed).map_or(0, |index| index + 1)
    }

    /// Question 4, for one length: does it plan, does it transform
    /// known signals correctly, and does it stay inside the buffers it
    /// was given?
    ///
    /// Returns the process's exit status: 0 for a length that works, 1
    /// for one that does not, 2 for one that would not plan. A length
    /// that corrupts the heap takes the process with it, which is the
    /// answer this is run one process at a time to get.
    fn one_length(length: usize) -> i32 {
        let Some(mut plan) = Plan::new(length) else {
            println!("{length}: no plan");
            return 2;
        };
        let wrong = check_length(&mut plan, length);
        if wrong.is_empty() {
            println!("{length}: ok");
            0
        } else {
            println!("{length}: {}", wrong.join(", "));
            1
        }
    }

    /// The known signals a length is checked with, as the ones that
    /// exist at that length: `N = 2` has only bins 0 and 1, so a
    /// single-bin cosine is not one of its cases.
    ///
    /// Every buffer carries canaries past its end, so a transform that
    /// writes outside what it was given is reported here rather than
    /// showing up later as an allocator failure somewhere else.
    fn check_length(plan: &mut Plan, length: usize) -> Vec<String> {
        let mut wrong = Vec::new();
        let mut spectrum = spectrum_with_canary(length);
        let expected = as_float(length);

        // DC: every sample 1, so bin 0 is N and every other bin 0.
        let dc = vec![1.0_f32; length];
        let mut signal = signal_with_canary(&dc);
        plan.forward(&mut signal, &mut spectrum);
        if !canary_intact(&signal, length) {
            wrong.push("wrote past the end of its input".to_owned());
        }
        // Questions 1 and 2 again, at this length: the code path
        // differs with the length, so "preserved at 16" is not an
        // answer for 1024.
        if signal[..length] != dc[..] {
            wrong.push("wrote into its input".to_owned());
        }
        if !close_to(spectrum[0].r, expected, expected) {
            wrong.push(format!("DC bin 0 is {} not {expected}", spectrum[0].r));
        }
        if (1..=length / 2).any(|bin| !close_to(spectrum[bin].r, 0.0, expected)) {
            wrong.push("DC leaked into another bin".to_owned());
        }

        // Nyquist: alternating +1/-1, so all the energy in the last bin.
        let alternating: Vec<f32> = (0..length)
            .map(|n| if n % 2 == 0 { 1.0 } else { -1.0 })
            .collect();
        let mut signal = signal_with_canary(&alternating);
        plan.forward(&mut signal, &mut spectrum);
        if !close_to(spectrum[length / 2].r, expected, expected) {
            wrong.push(format!(
                "Nyquist bin {} is {} not {expected}",
                length / 2,
                spectrum[length / 2].r
            ));
        }

        // A cosine on one bin, where there is one to put it on. Its
        // bin holds N/2 for a unit amplitude, the rest nothing.
        if length >= 4 {
            let mut signal = signal_with_canary(&cosine(length, 1));
            plan.forward(&mut signal, &mut spectrum);
            if !close_to(spectrum[1].r, expected / 2.0, expected) {
                wrong.push(format!(
                    "bin 1 of a unit cosine is {} not {}",
                    spectrum[1].r,
                    expected / 2.0
                ));
            }
            if (2..=length / 2).any(|bin| !close_to(spectrum[bin].r, 0.0, expected)) {
                wrong.push("the cosine leaked into another bin".to_owned());
            }
        }

        if !bin_canary_intact(&spectrum, length / 2 + 1) {
            wrong.push("wrote past bin N/2".to_owned());
        }

        // And back: the inverse at this length, from the spectrum the
        // last forward produced. A round trip that returns the input
        // says the scaling holds here too, and the copy says whether
        // the spectrum survived being read.
        let before = spectrum.clone();
        let mut restored = signal_with_canary(&vec![0.0_f32; length][..]);
        plan.inverse(&mut spectrum, &mut restored);
        if spectrum != before {
            wrong.push("the inverse wrote into its spectrum".to_owned());
        }
        if !canary_intact(&restored, length) {
            wrong.push("the inverse wrote past the end of its output".to_owned());
        }
        let expected_signal = if length >= 4 {
            cosine(length, 1)
        } else {
            alternating
        };
        if (0..length).any(|n| !close_to(restored[n], expected_signal[n], 1.0)) {
            wrong.push("the round trip did not return the signal".to_owned());
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
