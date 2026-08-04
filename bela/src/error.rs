use core::fmt;
use std::error;

/// Errors returned by the Bela audio system lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// `Bela_initAudio` failed with the contained return code.
    Init(i32),
    /// `Bela_startAudio` failed with the contained return code.
    Start(i32),
    /// The requested number of render threads cannot be served by
    /// [`BelaApplication`](crate::BelaApplication).
    ///
    /// Bela calls `render` concurrently on every thread with the same
    /// user data, which would mean several `&mut self` to one
    /// application at once; see `docs/multithreaded-rendering.md`.
    ThreadCountUnsupported(u32),
    /// An auxiliary task name contained a NUL byte.
    TaskName,
    /// `Bela_createAuxiliaryTask` failed, or the crate was built for a
    /// target with no audio system to create the task in.
    TaskCreate,
    /// An auxiliary task was created while an audio system was being
    /// torn down, which would have deleted it again immediately.
    ///
    /// This is what a `cleanup` callback gets: it runs inside that
    /// teardown.
    TaskCreateWhileStopping,
    /// `Bela_cpuMonitoringInit` failed.
    CpuMonitoring,
    /// The requested CPU monitoring acquisition cycle does not fit in a
    /// C `int`, which is how libbela takes it.
    CpuMonitoringCycle(u32),
    /// CPU monitoring was requested with a period size big enough that
    /// libbela runs `render` on a different thread from the one it
    /// measures.
    ///
    /// See
    /// [`MAX_MONITORED_PERIOD_SIZE`](crate::MAX_MONITORED_PERIOD_SIZE).
    CpuMonitoringPeriodSize(i32),
    /// Another [`Bela`](crate::Bela) audio system already exists in
    /// this process.
    ///
    /// The C API is a process-wide singleton, so a second one would
    /// share — and reset — the state the first is using.
    AudioSystemExists,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Init(code) => write!(f, "Bela_initAudio failed with code {code}"),
            Self::Start(code) => write!(f, "Bela_startAudio failed with code {code}"),
            Self::ThreadCountUnsupported(threads) => write!(
                f,
                "thread_count is {threads}: BelaApplication::render would be called \
                 concurrently on {threads} threads with &mut self"
            ),
            Self::TaskName => write!(f, "the auxiliary task name contains a NUL byte"),
            Self::TaskCreate => write!(f, "Bela_createAuxiliaryTask failed"),
            Self::TaskCreateWhileStopping => write!(
                f,
                "auxiliary tasks cannot be created while the audio system is stopping"
            ),
            Self::CpuMonitoring => write!(f, "Bela_cpuMonitoringInit failed"),
            Self::CpuMonitoringCycle(count) => write!(
                f,
                "the CPU monitoring cycle is {count} measurements, \
                 which does not fit in the C int libbela takes"
            ),
            Self::CpuMonitoringPeriodSize(frames) => write!(
                f,
                "CPU monitoring needs a period size of at most {max} frames, not {frames}: \
                 above that libbela renders on a separate thread from the one it measures",
                max = crate::MAX_MONITORED_PERIOD_SIZE
            ),
            Self::AudioSystemExists => write!(
                f,
                "a Bela audio system already exists in this process; the C API is a singleton"
            ),
        }
    }
}

impl error::Error for Error {}
