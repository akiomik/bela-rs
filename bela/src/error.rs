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
        }
    }
}

impl error::Error for Error {}
