//! Analyses the audio input with an FFT, and measures what the
//! transform costs the audio thread.
//!
//! Two things at once, because they answer each other, and they run in
//! different callbacks for a reason.
//!
//! The **analysis** is the ordinary use of [`RealFft`](bela::RealFft):
//! fill a window from the input, transform it, report the loudest bin.
//! It happens in `render_post`, which sees the whole block. `render`
//! would be the wrong place: it is called on every render thread with
//! that thread's share of the block, so with more than one thread each
//! window would hold every fourth quarter of the signal spliced
//! together, and the frequency that came out of it would be a fiction.
//! Whole-block work belongs where the whole block is.
//!
//! The **measurement** is the question a program has to answer before
//! it puts an FFT in `render` at all — how much of the block deadline
//! one costs — and that one belongs per thread, in `render`, where the
//! cost is actually paid. It is taken at several lengths, one per
//! block in rotation, with a `CpuTimer` around each. Running with four
//! render threads (`fft 4`) measures four transforms happening at
//! once, which is not four times cheaper.
//!
//! The plans are built in `setup`, one per render thread, because that
//! is the only callback that can refuse the run: `create_render_state`
//! returns a state rather than a `Result`, and the release profile
//! aborts on panic. `setup` hands them out through the render states.
//!
//! The reporting happens on an auxiliary task, as in `examples/cpu.rs`:
//! the numbers are read on the audio thread in `render_post` and
//! handed over through atomics.
//!
//! Cross-compile and run on the board (see docs/cross-compile.md):
//!
//! ```sh
//! cargo build -p bela --release --target aarch64-unknown-linux-gnu --example fft
//! ```
//!
//! What it prints belongs in `docs/fft.md`, which records what this
//! board's NE10 does.

#![cfg_attr(
    not(bela_device),
    allow(
        dead_code,
        reason = "only the fallback main is reachable off-device; the application code should still compile and lint"
    )
)]

use core::f32::consts::TAU;
use core::num::NonZeroU32;
use core::sync::atomic::{AtomicU32, Ordering};
#[cfg(not(bela_device))]
use std::process::ExitCode;
use std::sync::Arc;

use bela::{
    AuxiliaryTask, BelaApplication, BlockContext, CleanupContext, CpuTimer, FftBin, FftLength,
    Priority, RealFft, RenderContext, SetupContext, ThreadInfo, rt_println,
};

/// The window the input is analysed with. 1024 points at 44.1 kHz is
/// 23 ms, and 43 Hz per bin.
const ANALYSIS_LENGTH: usize = 1024;

/// The lengths the cost is measured at, one per block in rotation, so
/// that a run reports the whole curve rather than one point of it.
const MEASURED_LENGTHS: [usize; 5] = [256, 512, 1024, 2048, 4096];

/// Long enough that the counters rarely roll over mid-report: the
/// means below are read out of the cycle in progress.
const MEASUREMENTS_PER_CYCLE: u32 = 100_000;

/// Bela's own monitoring of the whole audio thread, which reports a
/// percentage only when a cycle completes — so it is short enough that
/// a run of a few seconds has one, where the timers above want a long
/// cycle for their means.
const MONITORING_MEASUREMENTS: u32 = 2000;

/// Render threads when the command line does not say: one, which is
/// what an analysis of a whole block wants. Pass a count as the first
/// argument to use more.
const DEFAULT_THREADS: NonZeroU32 = NonZeroU32::new(1).expect("one thread is non-zero");

/// Below the audio thread, so the report can never delay it.
const TASK_PRIORITY: Priority = Priority::new(75).expect("75 is within Bela's priority range");

/// What the audio thread publishes for the task to print: each number
/// as `f32::to_bits`.
#[derive(Debug, Default)]
struct Published {
    /// The loudest bin's frequency, in Hz.
    peak_hz: AtomicU32,
    /// Its magnitude, as the transform reports it.
    peak_magnitude: AtomicU32,
    /// Mean microseconds per transform, one per [`MEASURED_LENGTHS`].
    micros: [AtomicU32; MEASURED_LENGTHS.len()],
}

/// The analysis plan and the window it fills.
struct Analysis {
    fft: RealFft,
    window: Vec<f32>,
    spectrum: Vec<FftBin>,
    /// How much of `window` holds input so far.
    filled: usize,
    timer: CpuTimer,
}

/// One measured length: a plan, buffers, and the timer around it.
struct Cost {
    fft: RealFft,
    signal: Vec<f32>,
    spectrum: Vec<FftBin>,
    timer: CpuTimer,
}

/// What one render thread measures with: the cost plans and where the
/// rotation is up to. The analysis is not here — it is the
/// application's, because it is whole-block work.
struct Plans {
    costs: Vec<Cost>,
    /// Which of `costs` the next block measures.
    next: usize,
}

struct Analyser {
    published: Arc<Published>,
    task: Option<AuxiliaryTask>,
    /// The whole-block analysis, used from `render_post` and so held
    /// by the application rather than by a render state.
    analysis: Option<Analysis>,
    /// Built in `setup`, one per render thread, taken in
    /// `create_render_state`.
    plans: Vec<Plans>,
    sample_rate: f32,
    blocks: u64,
    blocks_per_report: u64,
}

impl Analyser {
    fn new() -> Self {
        Self {
            published: Arc::new(Published::default()),
            task: None,
            analysis: None,
            plans: Vec::new(),
            sample_rate: 0.0,
            blocks: 0,
            // Replaced in setup, once the block size is known.
            blocks_per_report: 1,
        }
    }
}

/// [`ANALYSIS_LENGTH`] as the float the bin spacing is computed in.
#[allow(clippy::cast_precision_loss, reason = "1024 is exact in f32")]
const fn analysis_length_as_float() -> f32 {
    ANALYSIS_LENGTH as f32
}

const fn cycle() -> NonZeroU32 {
    NonZeroU32::new(MEASUREMENTS_PER_CYCLE).expect("the cycle length is a non-zero constant")
}

const fn monitoring_cycle() -> NonZeroU32 {
    NonZeroU32::new(MONITORING_MEASUREMENTS).expect("the cycle length is a non-zero constant")
}

/// A plan of `length` points with the buffers it transforms, or the
/// error that stopped it.
fn plan_of(length: usize) -> Result<(RealFft, Vec<f32>, Vec<FftBin>), bela::Error> {
    let length = FftLength::try_from(length)?;
    let fft = RealFft::new(length)?;
    let signal = fft.new_signal();
    let spectrum = fft.new_spectrum();
    Ok((fft, signal, spectrum))
}

/// The whole-block analysis, built where a failure can still be
/// reported.
fn analysis_plan() -> Result<Analysis, bela::Error> {
    let (fft, window, spectrum) = plan_of(ANALYSIS_LENGTH)?;
    Ok(Analysis {
        fft,
        window,
        spectrum,
        filled: 0,
        timer: CpuTimer::new(cycle()),
    })
}

/// What one render thread measures with, built in the same place.
fn plans_for_one_thread() -> Result<Plans, bela::Error> {
    let mut costs = Vec::with_capacity(MEASURED_LENGTHS.len());
    for length in MEASURED_LENGTHS {
        let (fft, mut signal, spectrum) = plan_of(length)?;
        // A cosine at bin 1 rather than silence: what a transform
        // costs should not be measured on a buffer of zeros, where
        // denormals can flatter or punish it.
        for (index, sample) in signal.iter_mut().enumerate() {
            #[allow(
                clippy::cast_precision_loss,
                reason = "an index within a transform length is far below f32's exact integer range"
            )]
            let phase = TAU * index as f32 / length as f32;
            *sample = phase.cos();
        }
        costs.push(Cost {
            fft,
            signal,
            spectrum,
            timer: CpuTimer::new(cycle()),
        });
    }

    Ok(Plans { costs, next: 0 })
}

/// The mean time one measured section took, in microseconds, or 0
/// before the first measurement.
///
/// `busy` and the count are both what the *current* acquisition cycle
/// has accumulated, so this is a mean over that cycle rather than over
/// the run — which is why the cycle above is long.
#[allow(
    clippy::cast_precision_loss,
    reason = "a cycle is 100_000 measurements of a few microseconds; both are far inside f32's exact integer range"
)]
fn mean_micros(timer: &CpuTimer) -> f32 {
    let usage = timer.usage();
    let taken = usage.measurements_taken();
    if taken == 0 {
        return 0.0;
    }
    let nanos = usage.busy().as_nanos() as f32;
    nanos / taken as f32 / 1000.0
}

/// Transforms a cosine and transforms it back, and reports how far
/// the result strayed.
///
/// Not real-time work — it allocates — so `setup` is where it belongs.
/// What it demonstrates is the scaling: an unscaled forward and an
/// inverse that restores the original amplitudes, so the worst
/// difference here is rounding and nothing else.
fn round_trip_error() -> Result<f32, bela::Error> {
    let (mut fft, mut signal, mut spectrum) = plan_of(ANALYSIS_LENGTH)?;
    #[allow(
        clippy::cast_precision_loss,
        reason = "an index within a transform length is far below f32's exact integer range"
    )]
    for (index, sample) in signal.iter_mut().enumerate() {
        *sample = (TAU * 4.0 * index as f32 / analysis_length_as_float()).cos();
    }
    let original = signal.clone();

    fft.forward(&mut signal, &mut spectrum)?;
    fft.inverse(&mut spectrum, &mut signal)?;

    Ok(original
        .iter()
        .zip(&signal)
        .map(|(before, after)| (before - after).abs())
        .fold(0.0_f32, f32::max))
}

impl Analyser {
    /// Fills the analysis window from the whole block, and transforms
    /// it whenever it comes up full.
    ///
    /// Called from `render_post`, which sees every frame of the block
    /// in order. Doing this in `render` would see only one thread's
    /// share of it, and a window spliced together from every fourth
    /// quarter of the signal reports a frequency that is not there.
    fn analyse(&mut self, context: &BlockContext) {
        let Some(analysis) = &mut self.analysis else {
            return;
        };
        if context.audio_in_channels() == 0 {
            return;
        }

        for frame in 0..context.audio_frames() {
            analysis.window[analysis.filled] = context.audio_read(frame, 0);
            analysis.filled += 1;
            if analysis.filled < analysis.window.len() {
                continue;
            }
            analysis.filled = 0;

            let transformed = {
                let _section = analysis.timer.measure();
                analysis
                    .fft
                    .forward(&mut analysis.window, &mut analysis.spectrum)
            };
            if transformed.is_err() {
                // Both buffers came from the plan, so their lengths
                // agree by construction; saying so beats going quiet
                // if a later edit changes one. Once per window rather
                // than once per block, which is why this one prints
                // where the measurement below does not.
                rt_println!("render_post: the analysis buffers no longer fit the plan");
                continue;
            }

            // The loudest bin, skipping DC — which a little offset on
            // the input would otherwise win every time.
            let peak = analysis
                .spectrum
                .iter()
                .enumerate()
                .skip(1)
                .max_by(|(_, a), (_, b)| a.magnitude_squared().total_cmp(&b.magnitude_squared()));
            if let Some((bin, value)) = peak {
                #[allow(
                    clippy::cast_precision_loss,
                    reason = "a bin index is far below f32's exact integer range"
                )]
                let hz = self.sample_rate * bin as f32 / analysis_length_as_float();
                self.published
                    .peak_hz
                    .store(hz.to_bits(), Ordering::Relaxed);
                self.published
                    .peak_magnitude
                    .store(value.magnitude().to_bits(), Ordering::Relaxed);
            }
        }
    }
}

impl BelaApplication for Analyser {
    type RenderState = Option<Plans>;

    fn setup(&mut self, context: &SetupContext) -> bool {
        self.sample_rate = context.audio_sample_rate();
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the sample rate is a small positive number"
        )]
        let sample_rate_hz = self.sample_rate as u64;
        self.blocks_per_report = (sample_rate_hz / context.audio_frames().max(1) as u64).max(1);

        // The one place a plan that could not be built can be
        // reported: returning false here refuses the run before audio
        // starts, where a panic in a later callback would abort.
        match analysis_plan() {
            Ok(analysis) => self.analysis = Some(analysis),
            Err(error) => {
                rt_println!("setup: no analysis plan: {error}");
                return false;
            }
        }
        for thread in 0..context.thread_count() {
            match plans_for_one_thread() {
                Ok(plans) => self.plans.push(plans),
                Err(error) => {
                    rt_println!("setup: no FFT plans for thread {thread}: {error}");
                    return false;
                }
            }
        }

        // The inverse, once, where allocating and printing are free:
        // a round trip returns the signal it started from, which is
        // this crate's contract rather than NE10's (docs/fft.md).
        match round_trip_error() {
            Ok(worst) => rt_println!("setup: round trip differs by at most {worst:.2e}"),
            Err(error) => {
                rt_println!("setup: the round trip failed: {error}");
                return false;
            }
        }

        let published = Arc::clone(&self.published);
        let task = AuxiliaryTask::new("bela-rs-fft", TASK_PRIORITY, move || {
            let hz = f32::from_bits(published.peak_hz.load(Ordering::Relaxed));
            let magnitude = f32::from_bits(published.peak_magnitude.load(Ordering::Relaxed));
            rt_println!("peak: {hz:.0} Hz at magnitude {magnitude:.1}");
            for (length, micros) in MEASURED_LENGTHS.iter().zip(&published.micros) {
                let micros = f32::from_bits(micros.load(Ordering::Relaxed));
                rt_println!("  {length:>5} points: {micros:.1} us per transform");
            }
        });

        match task {
            Ok(task) => {
                self.task = Some(task);
                rt_println!(
                    "setup: {ANALYSIS_LENGTH}-point analysis over {} render thread(s), \
                     {:.0} Hz per bin; reporting every {} blocks",
                    context.thread_count(),
                    self.sample_rate / analysis_length_as_float(),
                    self.blocks_per_report
                );
                true
            }
            Err(error) => {
                rt_println!("setup: could not create the task: {error}");
                false
            }
        }
    }

    fn create_render_state(
        &mut self,
        _thread: ThreadInfo,
        _context: &SetupContext,
    ) -> Option<Plans> {
        // `setup` built one set per thread and agreed to start, so
        // this hands one over rather than making it. `None` is
        // unreachable, and costs a branch in `render` rather than an
        // abort here.
        self.plans.pop()
    }

    // Real-time safe: copies, arithmetic, one transform that allocates
    // nothing, and a clock read through the CPU timer.
    fn render(&self, state: &mut Option<Plans>, context: &mut RenderContext) {
        let Some(state) = state else { return };

        // Passthrough, so what is analysed can be heard. This thread's
        // share of the block, which is what `render` is handed.
        let channels = context
            .audio_in_channels()
            .min(context.audio_out_channels());
        for frame in context.audio_frame_range() {
            for channel in 0..channels {
                context.audio_write(frame, channel, context.audio_read(frame, channel));
            }
        }

        // One measured transform per block, at the next length in
        // turn. Per thread on purpose: what a transform costs when
        // four of them run at once is the number worth having, and the
        // analysis in `render_post` is the one that has to see whole
        // blocks.
        let index = state.next;
        state.next = (index + 1) % state.costs.len();
        if let Some(cost) = state.costs.get_mut(index) {
            let _section = cost.timer.measure();
            // Deliberately dropped, where the analysis reports the
            // same impossible error: this runs every block on every
            // thread, and a buffer that stopped fitting would print
            // thousands of times a second. The lengths are the plan's
            // own, and `cleanup` shows the transform count they
            // produced.
            let _ = cost.fft.forward(&mut cost.signal, &mut cost.spectrum);
        }
    }

    // Real-time safe: copies, arithmetic, a transform that allocates
    // nothing, atomic stores and a schedule.
    fn render_post(&mut self, states: &mut [Option<Plans>], context: &mut BlockContext) {
        self.analyse(context);

        self.blocks += 1;
        if self.blocks % self.blocks_per_report != 0 {
            return;
        }
        // Thread 0's timers stand for the rest: every thread runs the
        // same rotation over the same lengths.
        if let Some(Some(plans)) = states.first() {
            for (cost, published) in plans.costs.iter().zip(&self.published.micros) {
                published.store(mean_micros(&cost.timer).to_bits(), Ordering::Relaxed);
            }
        }
        if let Some(task) = &self.task {
            task.schedule(context);
        }
    }

    fn cleanup(&mut self, states: &mut [Option<Plans>], context: &CleanupContext) {
        if let Some(usage) = context.cpu_usage() {
            rt_println!("cleanup: audio thread {usage}");
        }
        if let Some(analysis) = &self.analysis {
            rt_println!(
                "cleanup: {} whole-block analysis transforms at {:.1} us each",
                analysis.timer.usage().measurements_taken(),
                mean_micros(&analysis.timer)
            );
        }
        if let Some(Some(plans)) = states.first() {
            for (length, cost) in MEASURED_LENGTHS.iter().zip(&plans.costs) {
                rt_println!(
                    "cleanup: {length:>5} points: {:.1} us per transform over {} of them",
                    mean_micros(&cost.timer),
                    cost.timer.usage().measurements_taken()
                );
            }
        }
        rt_println!("cleanup: {} blocks rendered", self.blocks);
    }
}

/// The render thread count from this program's own first argument, as
/// `examples/parallel.rs` takes it.
///
/// Worth passing: one plan per render thread is the arrangement the
/// API is built around, and more than one thread is where handing them
/// out in `create_render_state` has to be right.
fn requested_threads() -> NonZeroU32 {
    use std::env;

    env::args()
        .nth(1)
        .and_then(|argument| argument.parse().ok())
        .unwrap_or(DEFAULT_THREADS)
}

#[cfg(bela_device)]
fn main() -> Result<(), bela::Error> {
    // A small period keeps the block deadline tight, which is the
    // point of comparison the microseconds are read against.
    bela::Bela::run(
        Analyser::new(),
        &bela::Settings::new()
            .period_size(64)
            .thread_count(requested_threads())
            .cpu_monitoring(monitoring_cycle()),
    )
}

#[cfg(not(bela_device))]
fn main() -> ExitCode {
    eprintln!("This example must be cross-compiled for Bela Gem (aarch64-unknown-linux-gnu).");
    ExitCode::FAILURE
}
