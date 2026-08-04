//! CPU monitoring: how much of the block deadline is actually used.
//!
//! Bela measures busy time by bracketing a section of code between a
//! *tic* and a *toc*, both of which read a monotonic clock. Tic-to-toc
//! is the busy time; tic-to-tic is the whole period the section repeats
//! on. Every `measurements_per_cycle` tics one *acquisition cycle*
//! completes and the ratio of the two, in percent, is published as
//! [`CpuUsage::percentage`].
//!
//! There are two ways in, both read as a [`CpuUsage`]:
//! [`Settings::cpu_monitoring`](crate::Settings::cpu_monitoring) turns
//! on the monitoring libbela does for the whole audio thread, which
//! [`Context::cpu_usage`] reads, and [`CpuTimer`] measures a section of
//! `render` with counters the application owns.
//!
//! # The shape of the API is a soundness requirement
//!
//! The audio thread's counters are one unsynchronised structure inside
//! libbela, written by that thread as it runs. Reading them from
//! another thread at the same time is a data race, and a data race is
//! undefined behaviour however sensible the values look —
//! `read_volatile` does not make it one, since volatile is not atomic.
//! Turning monitoring on is worse still: it resets the same structure.
//!
//! So neither operation is reachable from a point where it could race.
//! Enabling is a [`Settings`](crate::Settings) field, applied by
//! [`Bela::new`](crate::Bela::new) before an audio thread exists — the
//! reason [`apply_monitoring`] is not public. Reading needs the
//! `&Context` that only a Bela callback has: `setup` runs before the
//! audio thread starts, `render` runs *on* it, and `cleanup` runs after
//! libbela has joined it.
//!
//! [`CpuTimer`] has no such constraint, because its counters belong to
//! the application rather than to libbela.

use core::ffi::c_int;
use core::fmt;
use core::num::NonZeroU32;
use core::time::Duration;

use bela_sys::BelaCpuData;

use crate::context::Context;
use crate::error::Error;

/// A reading of the CPU monitoring counters.
///
/// [`percentage`](CpuUsage::percentage) is the figure to look at; the
/// rest describes the acquisition cycle it came from and how far along
/// the next one is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CpuUsage {
    percentage: f32,
    busy: u64,
    total: u64,
    measurements_per_cycle: u32,
    measurements_taken: u32,
}

impl CpuUsage {
    /// Reads Bela's counters.
    const fn from_raw(raw: &BelaCpuData) -> Self {
        Self {
            percentage: raw.percentage,
            busy: raw.busy,
            total: raw.total,
            measurements_per_cycle: raw.count,
            measurements_taken: raw.currentCount,
        }
    }

    /// Percentage of the measured period spent busy, averaged over the
    /// last completed acquisition cycle.
    ///
    /// Zero until the first cycle completes, which takes
    /// [`measurements_per_cycle`](CpuUsage::measurements_per_cycle)
    /// tics — for the audio thread, that many blocks.
    #[must_use]
    pub const fn percentage(&self) -> f32 {
        self.percentage
    }

    /// Busy time accumulated so far in the *current*, incomplete cycle.
    #[must_use]
    pub const fn busy(&self) -> Duration {
        Duration::from_nanos(self.busy)
    }

    /// Total time accumulated so far in the *current*, incomplete cycle
    /// — busy and idle together.
    #[must_use]
    pub const fn total(&self) -> Duration {
        Duration::from_nanos(self.total)
    }

    /// How many tics make up an acquisition cycle (Bela's `count`).
    #[must_use]
    pub const fn measurements_per_cycle(&self) -> u32 {
        self.measurements_per_cycle
    }

    /// How many tics have happened in the current cycle (Bela's
    /// `currentCount`).
    #[must_use]
    pub const fn measurements_taken(&self) -> u32 {
        self.measurements_taken
    }
}

impl fmt::Display for CpuUsage {
    /// Formats as `12.3% busy, averaged over 1000 measurements`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:.1}% busy, averaged over {} measurements",
            self.percentage, self.measurements_per_cycle
        )
    }
}

impl Context {
    /// The audio thread's CPU usage, or `None` when monitoring was not
    /// enabled with
    /// [`Settings::cpu_monitoring`](crate::Settings::cpu_monitoring).
    ///
    /// Reports how much of the block deadline went into rendering the
    /// block — the measurement that says whether `render` fits, rather
    /// than [`underrun_count`](Context::underrun_count) saying
    /// afterwards that it did not.
    ///
    /// Cheap, non-blocking and real-time safe: it copies a structure
    /// libbela owns. The number only changes once per acquisition
    /// cycle, so there is nothing to gain from reading it more often
    /// than that.
    ///
    /// ```no_run
    /// use bela::{BelaApplication, Context, rt_println};
    ///
    /// # struct App;
    /// # unsafe impl BelaApplication for App {
    /// # fn render(&mut self, _context: &mut Context) {}
    /// fn cleanup(&mut self, context: &mut Context) {
    ///     if let Some(usage) = context.cpu_usage() {
    ///         rt_println!("audio thread: {usage}");
    ///     }
    /// }
    /// # }
    /// ```
    ///
    /// # What is being measured
    ///
    /// The clock is monotonic wall time, not consumed CPU cycles, so
    /// the busy time includes anything the thread waited for: a
    /// higher-priority thread running, a blocking call, a page fault.
    /// On the audio thread that is the useful reading — what matters is
    /// whether the block was finished in time, not why it was not — but
    /// it does mean the numbers are not comparable to `top`.
    ///
    /// # Why it is on the context
    ///
    /// The `&Context` is a witness that this is a Bela callback, which
    /// is the only place the read is sound. The counters are written by
    /// the audio thread with no synchronisation, so reading them from
    /// another thread would be a data race; inside a callback there is
    /// no other thread to race with — `setup` runs before the audio
    /// thread starts, `render` runs on it, and `cleanup` runs after
    /// libbela has joined it.
    ///
    /// To report from an [`AuxiliaryTask`](crate::AuxiliaryTask),
    /// publish the reading from `render` through an atomic and let the
    /// task print that; `examples/cpu.rs` does exactly this.
    ///
    /// # The first reading is low
    ///
    /// Bela measures each period from the previous tic, and the audio
    /// thread's first tic has nothing to measure from but the timestamp
    /// [`Bela::new`](crate::Bela::new) left behind when it enabled
    /// monitoring — so the first cycle also counts everything in
    /// between, which is the audio system starting up. Measured on the
    /// board with `examples/cpu.rs`, that is a first reading of 9.8%
    /// against a steady 19.0%.
    ///
    /// Ignore the first reading, or size the cycle so that one goes by
    /// before anyone looks.
    #[must_use]
    pub fn cpu_usage(&self) -> Option<CpuUsage> {
        monitoring_data().map(|data| CpuUsage::from_raw(&data))
    }
}

/// Copies the audio thread's counters, if monitoring is on.
///
/// Safety: the pointer is to a static inside libbela, which outlives
/// everything, and the caller holds a `&Context`, which means no other
/// thread is writing it — see [`Context::cpu_usage`].
#[cfg(bela_device)]
fn monitoring_data() -> Option<BelaCpuData> {
    let data = unsafe { bela_sys::Bela_cpuMonitoringGet() };
    if data.is_null() {
        return None;
    }
    let data = unsafe { *data };
    // A zero count is how libbela spells "monitoring is off"; the rest
    // of the structure is then untouched zeroes.
    (data.count != 0).then_some(data)
}

/// Off-device there is no audio thread, and so nothing to report.
#[cfg(not(bela_device))]
const fn monitoring_data() -> Option<BelaCpuData> {
    None
}

/// Puts the audio thread's monitoring into the state `cycle` asks for:
/// on with that acquisition cycle, or off.
///
/// Called by [`Bela::new`](crate::Bela::new) and nowhere else, before
/// `Bela_initAudio` — early enough that the `setup` callback running
/// inside that call already sees the result, and long before there is
/// an audio thread whose updates the reset and the priming tic would
/// race with. Keeping it out of the public API is what makes that hold,
/// rather than a note asking callers to be careful.
///
/// # Errors
/// Returns [`Error::CpuMonitoring`] when `Bela_cpuMonitoringInit`
/// fails.
#[cfg(bela_device)]
pub fn apply_monitoring(cycle: Option<c_int>) -> Result<(), Error> {
    let Some(count) = cycle else {
        return disable_monitoring();
    };
    // Safety: no audio thread exists yet, so nothing else is touching
    // the structure this resets.
    let data = unsafe {
        if bela_sys::Bela_cpuMonitoringInit(count) != 0 {
            return Err(Error::CpuMonitoring);
        }
        bela_sys::Bela_cpuMonitoringGet()
    };
    if data.is_null() {
        return Err(Error::CpuMonitoring);
    }
    // Safety: as above. Without this the audio thread's first tic would
    // measure from a zeroed timestamp — a period as long as the
    // monotonic clock — instead of from a moment just before it started.
    unsafe {
        bela_sys::Bela_cpuTic(data);
        discard_first_measurement(&mut *data);
    }
    Ok(())
}

/// Turns the audio thread's monitoring off.
///
/// `Bela_cpuMonitoringInit(0)`, which the C documentation describes as
/// disabling, returns without doing anything, so the count is cleared
/// directly — which is what "use 0 to disable" means for that field.
///
/// Without this an audio system that asked for monitoring would leave
/// it on for the next one that did not, right down to a stale reading
/// from [`Context::cpu_usage`]. The settings are meant to say what is
/// the case, not only what to switch on.
#[cfg(bela_device)]
fn disable_monitoring() -> Result<(), Error> {
    // Safety: no audio thread exists yet, so nothing else is touching
    // the field this clears.
    let data = unsafe { bela_sys::Bela_cpuMonitoringGet() };
    if data.is_null() {
        return Err(Error::CpuMonitoring);
    }
    unsafe { (*data).count = 0 };
    Ok(())
}

/// Converts an acquisition cycle length to the `int` libbela takes.
///
/// # Errors
/// Returns [`Error::CpuMonitoringCycle`] for a length that does not fit
/// in a C `int`. Saturating instead would leave the audio thread
/// measuring a cycle other than the one that was asked for, and
/// [`CpuUsage::measurements_per_cycle`] reporting that other value.
#[cfg_attr(
    not(bela_device),
    allow(
        dead_code,
        reason = "only the device-gated audio system enables monitoring; still unit-tested on the host"
    )
)]
pub const fn check_cycle(measurements_per_cycle: NonZeroU32) -> Result<c_int, Error> {
    let count = measurements_per_cycle.get();
    // Bela stores the count in an `unsigned int` but takes it as an
    // `int`, so anything above `int::MAX` would arrive as something
    // else entirely.
    #[allow(
        clippy::cast_possible_wrap,
        reason = "the comparison above rules out the values that would wrap"
    )]
    if count <= c_int::MAX as u32 {
        Ok(count as c_int)
    } else {
        Err(Error::CpuMonitoringCycle(count))
    }
}

/// Measures a section of `render` chosen by the application.
///
/// Where [`Context::cpu_usage`] reports the whole audio thread, this
/// reports one part of it — which is how you find out *what* is using
/// the block, once the audio thread's reading says something is. Its
/// counters belong to the timer, so several can be in use at once, one
/// per section, and reading one is never a race: it is an ordinary
/// `&self` read of memory the application owns.
///
/// It measures the same thing, the same way as
/// [`Context::cpu_usage`]: monotonic wall time, which counts waiting as
/// busy. Reading the clock is itself a system call (a vDSO one, so no
/// context switch, but not free), so bracketing one section per block
/// costs nothing worth measuring, while bracketing per frame does not.
///
/// Keep the tic/toc pair on the same path through `render`: the period
/// each measurement is a fraction of is the time from one tic to the
/// next, so a section that is entered on only some blocks reports the
/// fraction of *those* blocks, not of every block.
///
/// Every reading counts, including the first: the timer throws away
/// what its own first tic measured, which is what would otherwise
/// stretch the first cycle the way it does for the audio thread's own
/// first reading (see [`Context::cpu_usage`]).
///
/// # Example
///
/// [`measure`](CpuTimer::measure) brackets a scope, which pairs the two
/// halves for you:
///
/// ```no_run
/// use core::num::NonZeroU32;
///
/// use bela::{BelaApplication, Context, CpuTimer};
///
/// struct App {
///     timer: CpuTimer,
/// }
///
/// unsafe impl BelaApplication for App {
///     fn render(&mut self, context: &mut Context) {
///         let _filter = self.timer.measure();
///         // ... the work being measured ...
///     }
/// }
/// ```
///
/// [`tic`](CpuTimer::tic) and [`toc`](CpuTimer::toc) are there for the
/// cases a scope cannot express.
#[derive(Debug)]
pub struct CpuTimer {
    data: BelaCpuData,
    /// Whether the first tic — the one measuring from a zeroed
    /// timestamp — has been taken and thrown away.
    primed: bool,
}

impl CpuTimer {
    /// Creates a timer whose acquisition cycle is
    /// `measurements_per_cycle` tic/toc pairs long.
    ///
    /// Unlike the audio thread's cycle, this one never crosses the FFI
    /// boundary as an `int` — the counters are the timer's own — so
    /// every [`NonZeroU32`] is accepted as given.
    #[must_use]
    pub const fn new(measurements_per_cycle: NonZeroU32) -> Self {
        let mut data = ZEROED_CPU_DATA;
        data.count = measurements_per_cycle.get();
        Self {
            data,
            primed: false,
        }
    }

    /// Starts measuring, and stops again when the returned guard is
    /// dropped.
    ///
    /// Real-time safe, like the [`tic`](CpuTimer::tic) and
    /// [`toc`](CpuTimer::toc) it stands for.
    pub fn measure(&mut self) -> CpuSection<'_> {
        self.tic();
        CpuSection { timer: self }
    }

    /// Starts measuring a section, and ends the period the previous
    /// measurement was a fraction of.
    ///
    /// Real-time safe: it reads a monotonic clock and adds to a
    /// counter.
    #[cfg_attr(
        not(bela_device),
        allow(
            clippy::missing_const_for_fn,
            reason = "only const off-device, where the clock is never read"
        )
    )]
    pub fn tic(&mut self) {
        self.tic_raw();
        if !self.primed {
            // Bela measures the period from the previous tic, and the
            // first one has no previous tic — only a zeroed timestamp,
            // which makes the period the entire monotonic clock. Throw
            // that away and keep the timestamp it left behind, so the
            // first real measurement starts from here.
            discard_first_measurement(&mut self.data);
            self.primed = true;
        }
    }

    /// Stops measuring a section.
    ///
    /// Real-time safe. A `toc` without a matching `tic` before it
    /// counts the time since the last `tic` twice; prefer
    /// [`measure`](CpuTimer::measure), which cannot get that wrong.
    #[cfg_attr(
        not(bela_device),
        allow(
            clippy::missing_const_for_fn,
            reason = "only const off-device, where the clock is never read"
        )
    )]
    pub fn toc(&mut self) {
        self.toc_raw();
    }

    /// Reads the current counters.
    #[must_use]
    pub const fn usage(&self) -> CpuUsage {
        CpuUsage::from_raw(&self.data)
    }

    #[cfg(bela_device)]
    fn tic_raw(&mut self) {
        // Safety: the pointer is to a live structure this timer owns
        // exclusively — `&mut self` — and Bela only reads and writes
        // its fields.
        unsafe { bela_sys::Bela_cpuTic(&raw mut self.data) };
    }

    #[cfg(bela_device)]
    fn toc_raw(&mut self) {
        // Safety: as for `tic_raw`.
        unsafe { bela_sys::Bela_cpuToc(&raw mut self.data) };
    }

    /// Off-device there is no clock reading to do: without libbela
    /// there is no audio thread to measure, and the counters stay at
    /// zero so that application code still compiles and runs.
    #[cfg(not(bela_device))]
    #[allow(
        clippy::needless_pass_by_ref_mut,
        clippy::unused_self,
        reason = "mirrors the device signature, which mutates the counters"
    )]
    const fn tic_raw(&mut self) {}

    #[cfg(not(bela_device))]
    #[allow(
        clippy::needless_pass_by_ref_mut,
        clippy::unused_self,
        reason = "mirrors the device signature, which mutates the counters"
    )]
    const fn toc_raw(&mut self) {}
}

/// Throws away what the first tic accumulated, keeping the timestamp it
/// recorded.
///
/// Leaves exactly the state a legitimate first tic would have produced:
/// nothing measured yet, and the clock started.
const fn discard_first_measurement(data: &mut BelaCpuData) {
    data.busy = 0;
    data.total = 0;
    data.currentCount = 0;
    // Only ever set by that first tic when the cycle is one
    // measurement long, but a bogus reading is worth not publishing.
    data.percentage = 0.0;
}

/// An all-zero `BelaCpuData`, spelled out because the generated
/// `Default` is not `const`.
const ZEROED_CPU_DATA: BelaCpuData = BelaCpuData {
    count: 0,
    currentCount: 0,
    busy: 0,
    total: 0,
    tic: ZEROED_TIMESPEC,
    toc: ZEROED_TIMESPEC,
    percentage: 0.0,
};

const ZEROED_TIMESPEC: bela_sys::timespec = bela_sys::timespec {
    tv_sec: 0,
    tv_nsec: 0,
};

/// Measures for as long as it is alive; see [`CpuTimer::measure`].
///
/// Dropping it is the `toc`, so bind it to a name — `let _section =
/// ...`, not `let _ = ...`, which drops it there and then and measures
/// nothing.
#[derive(Debug)]
#[must_use = "the measurement ends when this is dropped, so `let _ = ...` measures nothing"]
pub struct CpuSection<'a> {
    timer: &'a mut CpuTimer,
}

impl Drop for CpuSection<'_> {
    fn drop(&mut self) {
        self.timer.toc();
    }
}

#[cfg(test)]
#[allow(
    clippy::float_cmp,
    reason = "the counters are copied verbatim, so the expected values are exact"
)]
mod tests {
    use super::*;

    fn cycle_of(measurements: u32) -> NonZeroU32 {
        NonZeroU32::new(measurements).expect("the test cycles are non-zero")
    }

    /// Counters as they would look partway through the second cycle.
    fn measured() -> BelaCpuData {
        BelaCpuData {
            count: 1000,
            currentCount: 250,
            busy: 3_000_000,
            total: 12_000_000,
            tic: bela_sys::timespec {
                tv_sec: 5,
                tv_nsec: 6,
            },
            toc: bela_sys::timespec {
                tv_sec: 7,
                tv_nsec: 8,
            },
            percentage: 12.34,
        }
    }

    #[test]
    fn a_reading_reports_belas_counters() {
        let usage = CpuUsage::from_raw(&measured());

        assert!(
            (usage.percentage() - 12.34).abs() < f32::EPSILON,
            "expected the percentage from the last completed cycle, got {}",
            usage.percentage()
        );
        assert_eq!(usage.busy(), Duration::from_millis(3));
        assert_eq!(usage.total(), Duration::from_millis(12));
        assert_eq!(usage.measurements_per_cycle(), 1000);
        assert_eq!(usage.measurements_taken(), 250);
    }

    #[test]
    fn a_reading_prints_the_percentage_and_the_cycle() {
        let usage = CpuUsage::from_raw(&measured());

        assert_eq!(
            usage.to_string(),
            "12.3% busy, averaged over 1000 measurements"
        );
    }

    #[test]
    fn nothing_measured_yet_reads_as_zero() {
        let usage = CpuUsage::from_raw(&ZEROED_CPU_DATA);

        assert_eq!(usage.percentage(), 0.0);
        assert_eq!(usage.busy(), Duration::ZERO);
        assert_eq!(usage.measurements_per_cycle(), 0);
        assert_eq!(usage.measurements_taken(), 0);
    }

    #[test]
    fn a_cycle_that_fits_in_a_c_int_is_passed_through() {
        assert_eq!(check_cycle(cycle_of(2000)), Ok(2000));
        assert_eq!(
            check_cycle(cycle_of(c_int::MAX.unsigned_abs())),
            Ok(c_int::MAX),
            "the largest representable cycle should still be accepted"
        );
    }

    #[test]
    fn a_cycle_too_large_for_a_c_int_is_refused() {
        // Saturating would leave the audio thread measuring a cycle
        // other than the one asked for, and reporting that other value.
        let over = c_int::MAX.unsigned_abs() + 1;

        assert_eq!(
            check_cycle(cycle_of(over)),
            Err(Error::CpuMonitoringCycle(over))
        );
        assert_eq!(
            check_cycle(cycle_of(u32::MAX)),
            Err(Error::CpuMonitoringCycle(u32::MAX))
        );
    }

    #[test]
    fn a_timer_starts_with_the_cycle_it_was_given() {
        let usage = CpuTimer::new(cycle_of(64)).usage();

        assert_eq!(usage.measurements_per_cycle(), 64);
        assert_eq!(usage.measurements_taken(), 0);
        assert_eq!(usage.percentage(), 0.0);
    }

    #[test]
    fn a_timer_takes_a_cycle_no_c_int_could_hold() {
        // Nothing converts these counters, so the limit the audio
        // thread's cycle has does not apply here.
        let usage = CpuTimer::new(cycle_of(u32::MAX)).usage();

        assert_eq!(usage.measurements_per_cycle(), u32::MAX);
    }

    #[test]
    fn the_first_measurement_is_discarded() {
        // What Bela's first tic leaves behind: a period as long as the
        // monotonic clock, and the timestamp to measure the next one
        // from.
        let mut data = BelaCpuData {
            count: 1,
            currentCount: 1,
            busy: 40_000,
            total: 9_000_000_000_000,
            percentage: 0.000_001,
            ..measured()
        };

        discard_first_measurement(&mut data);

        assert_eq!(data.busy, 0);
        assert_eq!(data.total, 0);
        assert_eq!(data.currentCount, 0);
        assert_eq!(data.percentage, 0.0);
        assert_eq!(
            (data.tic.tv_sec, data.tic.tv_nsec),
            (5, 6),
            "the timestamp is what the next measurement is taken from"
        );
    }

    #[test]
    fn only_the_first_tic_discards_anything() {
        let mut timer = CpuTimer::new(cycle_of(4));
        timer.tic();
        assert!(timer.primed, "the first tic should have primed the timer");

        // Stands in for a measurement the device would have taken.
        timer.data.busy = 40_000;
        timer.data.total = 360_000;
        timer.data.currentCount = 1;
        timer.tic();

        assert_eq!(timer.usage().busy(), Duration::from_nanos(40_000));
        assert_eq!(timer.usage().total(), Duration::from_nanos(360_000));
        assert_eq!(timer.usage().measurements_taken(), 1);
    }

    #[test]
    fn a_section_tocs_when_it_is_dropped() {
        let mut timer = CpuTimer::new(cycle_of(4));
        {
            let _section = timer.measure();
        }
        // Off-device the clock is never read, so the observable part is
        // the priming the tic did on the way in.
        assert!(timer.primed, "measure() should have tic'd");
    }

    #[test]
    fn there_is_nothing_to_report_off_device() {
        use core::mem;

        let mut context: bela_sys::BelaContext = unsafe { mem::zeroed() };
        let context = unsafe { Context::from_mut_ptr(&raw mut context) };

        assert_eq!(
            context.cpu_usage(),
            None,
            "off-device there is no audio thread to monitor"
        );
    }
}
