use core::fmt;

/// Errors returned by the Bela audio system lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// `Bela_initAudio` failed with the contained return code.
    Init(i32),
    /// `Bela_startAudio` failed with the contained return code.
    Start(i32),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Init(code) => write!(f, "Bela_initAudio failed with code {code}"),
            Error::Start(code) => write!(f, "Bela_startAudio failed with code {code}"),
        }
    }
}

impl std::error::Error for Error {}
