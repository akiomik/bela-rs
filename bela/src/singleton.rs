//! The process-wide claim on the Bela audio system.
//!
//! libbela keeps the audio system in globals: one set of settings, one
//! audio thread, one list of auxiliary tasks, one set of CPU monitoring
//! counters. A second [`Bela`](crate::Bela) would share all of it, so
//! there may only ever be one at a time.
//!
//! Saying so in the documentation was not enough. Setting an audio
//! system up touches those globals *before* libbela gets a chance to
//! refuse — CPU monitoring is reset before `Bela_initAudio` is even
//! called, so that the `setup` callback running inside it already sees
//! the result — and a second setup racing the first audio thread would
//! be a data race on the counters that thread is writing.
//!
//! So the claim is taken first, atomically, and everything that touches
//! the globals happens behind it.

use core::sync::atomic::{AtomicBool, Ordering};

use crate::error::Error;

/// Whether an audio system currently exists in this process.
static TAKEN: AtomicBool = AtomicBool::new(false);

/// Proof that this is the only audio system in the process.
///
/// Released on drop, including when construction fails partway through
/// and when the audio system is dropped after a panic.
#[derive(Debug)]
pub struct Claim {
    /// Keeps the type unconstructible outside this module.
    _private: (),
}

impl Claim {
    /// Takes the claim, or reports that something else holds it.
    ///
    /// # Errors
    /// Returns [`Error::AudioSystemExists`] when an audio system
    /// already exists, from this thread or any other.
    #[cfg_attr(
        not(bela_device),
        allow(
            dead_code,
            reason = "only the device-gated audio system takes it; still unit-tested on the host"
        )
    )]
    pub fn take() -> Result<Self, Error> {
        // Acquire on success: everything the previous holder did before
        // releasing happens-before whatever this one does to the same
        // globals.
        TAKEN
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .map(|_| Self { _private: () })
            .map_err(|_| Error::AudioSystemExists)
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        TAKEN.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use std::panic::catch_unwind;
    use std::sync::{Mutex, PoisonError};
    use std::thread;

    use super::*;

    /// The claim is process-wide, so the tests that take it have to run
    /// one at a time.
    static SERIALISE: Mutex<()> = Mutex::new(());

    fn serialised() -> impl Drop {
        SERIALISE.lock().unwrap_or_else(PoisonError::into_inner)
    }

    #[test]
    fn only_one_claim_exists_at_a_time() {
        let _order = serialised();

        let first = Claim::take().expect("the claim should be free");
        assert_eq!(
            Claim::take().unwrap_err(),
            Error::AudioSystemExists,
            "a second audio system must be refused"
        );

        drop(first);
        Claim::take().expect("dropping the first should have freed the claim");
    }

    #[test]
    fn another_thread_is_refused_too() {
        let _order = serialised();

        let held = Claim::take().expect("the claim should be free");
        let from_other_thread = thread::spawn(|| Claim::take().map(|_| ()))
            .join()
            .expect("the thread should not panic");

        assert_eq!(
            from_other_thread.unwrap_err(),
            Error::AudioSystemExists,
            "the claim is process-wide, not per-thread"
        );
        drop(held);
    }

    #[test]
    fn a_claim_dropped_while_unwinding_is_released() {
        let _order = serialised();

        let panicked = catch_unwind(|| {
            let _claim = Claim::take().expect("the claim should be free");
            panic!("something went wrong during setup");
        });
        assert!(panicked.is_err(), "the panic should propagate");

        Claim::take().expect("a panic must not leave the claim held forever");
    }
}
