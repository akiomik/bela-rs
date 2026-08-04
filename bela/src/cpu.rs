//! CPU monitoring: how much of the block deadline is actually used.
//!
//! Bela measures busy time by bracketing a section of code between a
//! *tic* and a *toc*, both of which read a monotonic clock. Tic-to-toc
//! is the busy time; tic-to-tic is the whole period the section repeats
//! on. Every `measurements_per_cycle` tics one *acquisition cycle*
//! completes and the ratio of the two, in percent, is published as
//! [`CpuUsage::percentage`].
//!
//! There are two ways in:
//!
//! - [`CpuMonitor`] turns on the monitoring libbela does for itself,
//!   which brackets the whole audio thread — one tic/toc pair per
//!   block. The percentage is then how much of the block deadline the
//!   audio thread uses, headroom included. This is what tells you
//!   whether `render` fits, before underruns say it does not.
//! - [`CpuTimer`] measures a section of `render` chosen by the
//!   application, using counters the application owns.
//!
//! Both are read as a [`CpuUsage`], which implements [`Display`] for
//! printing from an [`AuxiliaryTask`](crate::AuxiliaryTask) or from
//! `cleanup`.
//!
//! ```no_run
//! use core::num::NonZeroU32;
//!
//! use bela::{BelaApplication, Context, CpuMonitor, CpuTimer, rt_println};
//!
//! struct App {
//!     monitor: Option<CpuMonitor>,
//!     timer: CpuTimer,
//! }
//!
//! unsafe impl BelaApplication for App {
//!     fn setup(&mut self, _context: &mut Context) -> bool {
//!         self.monitor = CpuMonitor::enable(NonZeroU32::new(1000).unwrap()).ok();
//!         true
//!     }
//!
//!     fn render(&mut self, context: &mut Context) {
//!         let _section = self.timer.measure();
//!         // ... the work being measured ...
//!     }
//!
//!     fn cleanup(&mut self, _context: &mut Context) {
//!         if let Some(monitor) = self.monitor {
//!             rt_println!("audio thread: {}", monitor.usage());
//!         }
//!         rt_println!("render section: {}", self.timer.usage());
//!     }
//! }
//! ```
//!
//! # What is being measured
//!
//! The clock is monotonic wall time, not consumed CPU cycles, so the
//! busy time includes anything the thread waited for: a higher-priority
//! thread running, a blocking call, a page fault. On the audio thread
//! that is the useful reading — what matters is whether the block was
//! finished in time, not why it was not — but it does mean the numbers
//! are not comparable to `top`.
//!
//! Reading the clock is itself a system call (a vDSO one, so no context
//! switch, but not free). Bracketing one section per block costs
//! nothing worth measuring; bracketing per frame does not.
//!
//! [`Display`]: core::fmt::Display

use core::fmt;
use core::num::NonZeroU32;
use core::time::Duration;

use bela_sys::BelaCpuData;

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
    /// What a reading looks like before anything has been measured.
    const ZERO: Self = Self {
        percentage: 0.0,
        busy: 0,
        total: 0,
        measurements_per_cycle: 0,
        measurements_taken: 0,
    };

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
    /// tics — for [`CpuMonitor`], that many blocks.
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

/// The monitoring libbela does for the audio thread as a whole.
///
/// Enabling it makes the audio loop bracket every block, so
/// [`usage`](CpuMonitor::usage) reports how much of the block deadline
/// went into rendering it — the measurement that says whether `render`
/// fits, rather than [`Context::underrun_count`] saying afterwards that
/// it did not.
///
/// The counters live in a single process-wide structure inside
/// libbela, which is why this is a token rather than an owner: it
/// exists to say that monitoring was turned on, and copying it is
/// harmless.
///
/// # When to enable
///
/// Before audio starts, which in practice means from `setup` or before
/// [`Bela::run`](crate::Bela::run). The audio thread decides once, as
/// it starts, whether monitoring is on; enabling it later has no effect
/// on a thread that is already running, and the reset it performs would
/// race with one.
///
/// There is no way to turn it back off: `Bela_cpuMonitoringInit(0)`,
/// which the C documentation describes as disabling, returns without
/// doing anything.
///
/// # The first reading is low
///
/// Bela measures each period from the previous tic, and the audio
/// thread's first tic has nothing to measure from but the timestamp
/// enabling left behind — so the first cycle also counts everything
/// that happened between the two, which is the audio system starting
/// up. Measured on the board with the `cpu` example, that is a first
/// reading of 11.1% against a steady 19.0%.
///
/// Enabling as late as possible narrows the gap; nothing closes it.
/// Ignore the first reading, or size the cycle so that one goes by
/// before anyone looks.
///
/// # Reading is best-effort
///
/// The audio thread writes these counters while the reader reads them,
/// with no synchronisation on either side, so a reading can mix fields
/// from either side of a cycle boundary. For a number that is printed
/// and looked at that is fine; do not build control flow on one being
/// exactly consistent with the next.
///
/// [`Context::underrun_count`]: crate::Context::underrun_count
#[derive(Debug, Clone, Copy)]
pub struct CpuMonitor {
    /// Keeps the token unconstructible outside this module.
    _enabled: (),
}

impl CpuMonitor {
    /// Turns on the audio thread monitoring, with `measurements_per_cycle`
    /// blocks per acquisition cycle.
    ///
    /// The cycle length trades responsiveness against noise: at 44.1
    /// kHz and 16 frames per block, a block is about 0.36 ms, so 1000
    /// blocks is a reading every 0.36 s.
    ///
    /// See [when to enable](CpuMonitor#when-to-enable) — this has to
    /// happen before the audio thread starts.
    ///
    /// # Errors
    /// Returns [`Error::CpuMonitoring`] when `Bela_cpuMonitoringInit`
    /// fails, which is also what happens off-device, where there is no
    /// audio thread to monitor.
    pub fn enable(measurements_per_cycle: NonZeroU32) -> Result<Self, Error> {
        // Bela stores the count in an `unsigned int`, so a value that
        // does not survive the trip through `int` would come out as
        // something else entirely.
        let count = i32::try_from(measurements_per_cycle.get()).unwrap_or(i32::MAX);
        Self::init(count)
    }

    #[cfg(bela_device)]
    fn init(count: i32) -> Result<Self, Error> {
        // Safety: called before the audio thread exists, so nothing
        // else is touching the structure this resets.
        let data = unsafe {
            if bela_sys::Bela_cpuMonitoringInit(count) != 0 {
                return Err(Error::CpuMonitoring);
            }
            bela_sys::Bela_cpuMonitoringGet()
        };
        if data.is_null() {
            return Err(Error::CpuMonitoring);
        }
        // Safety: as above — a live structure owned by libbela, not yet
        // shared with a running audio thread.
        unsafe {
            bela_sys::Bela_cpuTic(data);
            discard_first_measurement(&mut *data);
        }
        Ok(Self { _enabled: () })
    }

    /// Off-device there is no audio thread to measure, so there is
    /// nothing to turn on.
    #[cfg(not(bela_device))]
    const fn init(_count: i32) -> Result<Self, Error> {
        Err(Error::CpuMonitoring)
    }

    /// Reads the current counters.
    ///
    /// Cheap and non-blocking, so it can be called from anywhere,
    /// including `render` — though the number only changes once per
    /// acquisition cycle, so there is nothing to gain from reading it
    /// more often than that. See [reading is
    /// best-effort](CpuMonitor#reading-is-best-effort).
    #[must_use]
    #[cfg_attr(
        not(bela_device),
        allow(
            clippy::missing_const_for_fn,
            reason = "only const off-device, where there is nothing to read"
        )
    )]
    pub fn usage(&self) -> CpuUsage {
        Self::read()
    }

    #[cfg(bela_device)]
    fn read() -> CpuUsage {
        // Safety: the pointer is to a static inside libbela, which
        // outlives everything. The read is volatile because the audio
        // thread writes the same memory; it is a snapshot, not an
        // atomic one.
        let data = unsafe { bela_sys::Bela_cpuMonitoringGet() };
        if data.is_null() {
            return CpuUsage::ZERO;
        }
        CpuUsage::from_raw(&unsafe { data.read_volatile() })
    }

    #[cfg(not(bela_device))]
    const fn read() -> CpuUsage {
        // Unreachable: `enable` never hands out a token off-device.
        CpuUsage::ZERO
    }
}

/// Measures a section of `render` chosen by the application.
///
/// Where [`CpuMonitor`] reports the whole audio thread, this reports
/// one part of it — which is how you find out *what* is using the
/// block, once the monitor says something is. Its counters belong to
/// the timer, so several can be in use at once, one per section.
///
/// Keep the tic/toc pair on the same path through `render`: the period
/// each measurement is a fraction of is the time from one tic to the
/// next, so a section that is entered on only some blocks reports the
/// fraction of *those* blocks, not of every block.
///
/// Every reading counts, including the first: the timer throws away
/// what its own first tic measured, which is what would otherwise
/// stretch the first cycle the way it does for
/// [`CpuMonitor`](CpuMonitor#the-first-reading-is-low).
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
        // Safety: the pointer is to a live structure this timer owns,
        // and Bela only reads and writes its fields.
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
        assert_eq!(CpuUsage::from_raw(&ZEROED_CPU_DATA), CpuUsage::ZERO);
    }

    #[test]
    fn a_timer_starts_with_the_cycle_it_was_given() {
        let usage = CpuTimer::new(cycle_of(64)).usage();

        assert_eq!(usage.measurements_per_cycle(), 64);
        assert_eq!(usage.measurements_taken(), 0);
        assert_eq!(usage.percentage(), 0.0);
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
    fn monitoring_cannot_be_enabled_off_device() {
        let error = CpuMonitor::enable(cycle_of(1000)).unwrap_err();
        assert_eq!(
            error,
            Error::CpuMonitoring,
            "off-device there is no audio thread to monitor"
        );
    }
}
