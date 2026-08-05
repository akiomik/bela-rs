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
//!
//! The claim has a third state, because a failed `Bela_initAudio`
//! leaves those globals believing an audio system is up and offers no
//! way to put them back — `Bela_cleanupAudio` segfaults on that path
//! (`docs/board-facts.md`). A claim released as *poisoned* is never
//! free again, so the second attempt is refused here instead of
//! segfaulting inside libbela.

use core::sync::atomic::{AtomicU8, Ordering};

use crate::error::Error;

/// No audio system exists and one may be built.
const FREE: u8 = 0;
/// An audio system exists in this process.
const TAKEN: u8 = 1;
/// An initialisation failed partway through and libbela cannot be
/// asked again. Terminal: nothing sets the state back from here.
const POISONED: u8 = 2;

/// What the audio system in this process is currently doing.
static STATE: AtomicU8 = AtomicU8::new(FREE);

/// Proof that this is the only audio system in the process.
///
/// Released on drop, including when construction fails partway through
/// and when the audio system is dropped after a panic. A claim
/// [poisoned](Claim::poison) first releases into [`POISONED`] instead,
/// which no later claim can take.
#[derive(Debug)]
pub struct Claim {
    /// Whether releasing this claim leaves the audio system unusable
    /// rather than free.
    poisoned: bool,
}

impl Claim {
    /// Takes the claim, or reports why it cannot be taken.
    ///
    /// # Errors
    /// Returns [`Error::AudioSystemExists`] when an audio system
    /// already exists, from this thread or any other, and
    /// [`Error::AudioSystemPoisoned`] when an initialisation failed
    /// earlier in this process.
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
        match STATE.compare_exchange(FREE, TAKEN, Ordering::Acquire, Ordering::Relaxed) {
            Ok(_) => Ok(Self { poisoned: false }),
            Err(POISONED) => Err(Error::AudioSystemPoisoned),
            Err(_) => Err(Error::AudioSystemExists),
        }
    }

    /// Gives up on the audio system for the rest of the process.
    ///
    /// For the one case that cannot be undone: `Bela_initAudio` failed
    /// partway through, so libbela is holding what it managed to take
    /// and there is no call that will make it let go. Releasing this
    /// claim as free would let the next `Bela::new` walk into that,
    /// which on a board means a segfault rather than an error.
    #[cfg_attr(
        not(bela_device),
        allow(
            dead_code,
            reason = "only the device-gated audio system poisons it; still unit-tested on the host"
        )
    )]
    pub const fn poison(&mut self) {
        self.poisoned = true;
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        let released = if self.poisoned { POISONED } else { FREE };
        STATE.store(released, Ordering::Release);
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

    /// Serialises this test against the others and gives it a state
    /// nothing else has written to.
    ///
    /// The reset is why this exists: poisoning is terminal by design,
    /// so without it the first test to poison would decide the result
    /// of every test after it. Nothing in the API can do this.
    fn serialised() -> impl Drop {
        let order = SERIALISE.lock().unwrap_or_else(PoisonError::into_inner);
        STATE.store(FREE, Ordering::Release);
        order
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

    #[test]
    fn a_poisoned_claim_is_never_free_again() {
        let _order = serialised();

        let mut claim = Claim::take().expect("the claim should be free");
        claim.poison();
        drop(claim);

        assert_eq!(
            Claim::take().unwrap_err(),
            Error::AudioSystemPoisoned,
            "a poisoned audio system must not be handed out again"
        );
        assert_eq!(
            Claim::take().unwrap_err(),
            Error::AudioSystemPoisoned,
            "and must stay refused however many times it is asked for"
        );
    }

    #[test]
    fn poisoning_is_refused_from_other_threads_too() {
        let _order = serialised();

        let mut claim = Claim::take().expect("the claim should be free");
        claim.poison();
        drop(claim);

        let from_other_thread = thread::spawn(|| Claim::take().map(|_| ()))
            .join()
            .expect("the thread should not panic");
        assert_eq!(
            from_other_thread.unwrap_err(),
            Error::AudioSystemPoisoned,
            "the poison is process-wide, like the claim it replaces"
        );
    }

    #[test]
    fn an_unpoisoned_claim_still_frees() {
        let _order = serialised();

        let claim = Claim::take().expect("the claim should be free");
        drop(claim);

        Claim::take().expect("only a poisoned claim should refuse the next one");
    }
}
