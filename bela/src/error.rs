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
        }
    }
}

impl error::Error for Error {}
