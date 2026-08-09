//! Auxiliary tasks: work moved off the audio thread.

#[cfg(bela_device)]
use core::ffi::c_void;
use core::ops::DerefMut;
#[cfg(test)]
use core::ptr;
use core::sync::atomic::{AtomicU64, Ordering};
use std::ffi::CString;
use std::sync::{Mutex, PoisonError};

use crate::context::CallbackContext;
use crate::error::Error;

/// A real-time priority Bela accepts for an auxiliary task: 0 to 99.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Priority(u8);

impl Priority {
    /// Priority of the audio thread. Auxiliary tasks should run below
    /// it, otherwise they preempt audio rendering.
    pub const AUDIO: Self = Self(95);

    /// A priority, or [`None`] above 99.
    #[must_use]
    pub const fn new(priority: u8) -> Option<Self> {
        if priority > 99 {
            return None;
        }
        Some(Self(priority))
    }
}

/// Which set of tasks libbela currently holds.
///
/// `Bela_deleteAllAuxiliaryTasks` frees every task at once and leaves
/// the handles dangling. Each handle records the generation it was
/// created in, and the counter is bumped when an audio system is torn
/// down, so a handle from an earlier one stays dead even after a later
/// one creates tasks of its own. Reading it is a single atomic load,
/// which [`AuxiliaryTask::schedule`] can afford on the render path.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Serialises creating tasks against tearing an audio system down.
///
/// Both sides run outside the real-time context — creating a task
/// allocates and starts a thread, tearing down joins threads — so an
/// ordinary mutex is the right tool. It is held only across the state
/// changes, never across the C calls: the teardown runs the user's
/// `cleanup` callback, which may itself try to create a task, and a
/// lock held across that would deadlock. Instead the window is marked
/// closed and creation inside it fails with
/// [`Error::TaskCreateWhileStopping`].
static LIFECYCLE: Mutex<Lifecycle> = Mutex::new(Lifecycle {
    generation: 0,
    accepting: true,
});

struct Lifecycle {
    /// Bumped once per teardown; mirrored into [`GENERATION`].
    generation: u64,
    /// Whether tasks may be created at the moment.
    accepting: bool,
}

/// Runs an audio system teardown with the task handles retired first.
///
/// Everything that can delete tasks belongs inside `teardown`: the
/// stop, the cleanup, and the drop of an audio system that was never
/// started. Which C function does the deleting is a moving target —
/// the build this crate is pinned to deletes at the end of
/// `Bela_stopAudio`, while upstream `fb362a5` deletes in
/// `Bela_cleanupAudio` after the user's `cleanup` callback has run — so
/// the window covers the whole teardown rather than one call.
///
/// Inside it, no task can be created and every existing handle is
/// already retired, which is what keeps a delete from racing a create
/// (they touch the same unsynchronised vector in libbela) or from
/// leaving a live-looking handle to freed memory.
#[cfg_attr(
    not(bela_device),
    allow(
        dead_code,
        reason = "only the device-gated system module tears an audio system down"
    )
)]
pub(crate) fn teardown<R>(shut_down: impl FnOnce() -> R) -> R {
    // Reopens the window even if the teardown panics on the way out.
    struct Reopen;
    impl Drop for Reopen {
        fn drop(&mut self) {
            lifecycle().accepting = true;
        }
    }

    {
        let mut lifecycle = lifecycle();
        lifecycle.accepting = false;
        lifecycle.generation += 1;
        GENERATION.store(lifecycle.generation, Ordering::Release);
    }
    let _reopen = Reopen;
    shut_down()
}

/// Takes the lifecycle lock.
///
/// A poisoned lock means a panic while creating a task or tearing an
/// audio system down. The state behind it stays consistent — the
/// generation only ever grows — so recovering beats panicking again,
/// often on the way down from the first panic.
fn lifecycle() -> impl DerefMut<Target = Lifecycle> {
    LIFECYCLE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A task that runs a callback on a lower-priority thread when the
/// audio thread asks it to.
///
/// This is how a Bela program does work that must not happen in
/// `render`: file and network I/O, expensive calculations that would
/// overrun the block deadline, anything that allocates or blocks.
/// `render` calls [`schedule`](AuxiliaryTask::schedule), which is
/// real-time safe, and the callback then runs on its own thread.
///
/// # Ownership of the task's state
///
/// The callback owns everything it touches: it is a `'static` closure,
/// moved into the task at creation. It cannot borrow from the
/// application, because it runs on its own thread while the audio
/// thread is inside `render` holding `&mut self`. Share state with
/// `render` the way threads normally do — atomics, or a lock-free
/// queue — and keep the real-time end of it allocation- and lock-free.
///
/// # Lifetime
///
/// Create tasks in `setup` (creating one allocates and starts a
/// thread, so `render` is the wrong place). They live until the audio
/// system they were created in is torn down, which deletes all of them
/// at once: the C API has no way to delete one task, so dropping this
/// handle does not destroy the task, and the callback's state stays
/// allocated for the life of the process.
///
/// Because that teardown frees the tasks behind the handles, a handle
/// records which audio system it belongs to, and scheduling it
/// afterwards does nothing — including a handle that outlived one
/// audio system while a later one is running, and including one from
/// an audio system that was initialised but never started.
///
/// Tasks cannot be created during a teardown at all: from `cleanup`,
/// or from another thread while an audio system is being dropped,
/// [`new`](AuxiliaryTask::new) fails with
/// [`Error::TaskCreateWhileStopping`] rather than handing back a task
/// that is about to be deleted.
///
/// # Shared, but only within a callback
///
/// The handle is [`Send`] and [`Sync`], because an application is both
/// and holds its tasks: with more than one render thread, every one of
/// them reaches the same handle through `&self`. Scheduling from
/// several at once is what libbela does itself —
/// `Bela_scheduleAuxiliaryTask` takes the task's mutex and notifies its
/// condition variable, and a call that cannot take the lock reports the
/// request as lost, which is already the documented behaviour below.
///
/// What keeps that from being a licence to schedule from anywhere is
/// the context [`schedule`](AuxiliaryTask::schedule) asks for: a
/// context cannot leave the callback it was handed to, so a handle sent
/// to an unrelated thread still has nothing to schedule with.
///
/// # Example
///
/// ```no_run
/// use core::sync::atomic::{AtomicU64, Ordering};
/// use std::sync::Arc;
///
/// use bela::{
///     AuxiliaryTask, BelaApplication, Priority, RenderContext, SetupContext, ThreadInfo,
///     rt_println,
/// };
///
/// struct App {
///     task: Option<AuxiliaryTask>,
///     blocks: Arc<AtomicU64>,
/// }
///
/// impl BelaApplication for App {
///     type RenderState = ();
///
///     fn setup(&mut self, _context: &SetupContext) -> bool {
///         let blocks = Arc::clone(&self.blocks);
///         let priority = Priority::new(50).expect("50 is within Bela's priority range");
///         self.task = AuxiliaryTask::new("report", priority, move || {
///             rt_println!("{} blocks so far", blocks.load(Ordering::Relaxed));
///         })
///         .ok();
///         self.task.is_some()
///     }
///
///     fn create_render_state(&mut self, _thread: ThreadInfo, _context: &SetupContext) {}
///
///     fn render(&self, _state: &mut (), context: &mut RenderContext) {
///         let blocks = self.blocks.fetch_add(1, Ordering::Relaxed) + 1;
///         if blocks % 1000 == 0 {
///             if let Some(task) = &self.task {
///                 task.schedule(context);
///             }
///         }
///     }
/// }
/// ```
#[derive(Debug)]
pub struct AuxiliaryTask {
    #[cfg_attr(
        not(bela_device),
        allow(
            dead_code,
            reason = "tasks cannot be created off-device, but the type still has to compile there"
        )
    )]
    raw: bela_sys::AuxiliaryTask,
    /// The value [`GENERATION`] had when the task was created.
    generation: u64,
}

// The handle is a plain pointer into libbela's task list, and the C
// side expects it to be used from the audio threads — which is where
// the application, and with it this handle, ends up. Scheduling through
// it is Bela_scheduleAuxiliaryTask, which serialises on the task's own
// mutex, so several render threads may hold the same handle at once.
unsafe impl Send for AuxiliaryTask {}
unsafe impl Sync for AuxiliaryTask {}

impl AuxiliaryTask {
    /// Creates a task that runs `callback` each time it is scheduled.
    ///
    /// `name` must be unique across the system (Bela names the
    /// underlying thread with it), and `priority` should stay below
    /// [`Priority::AUDIO`]. Bela permits the full range represented by
    /// [`Priority`], including the audio thread's own priority and
    /// priorities above it.
    ///
    /// The callback runs on a real-time thread of its own, but one
    /// that is allowed to miss deadlines: it may allocate, block and
    /// make system calls. Doing so still costs the rest of the
    /// real-time system, so prefer to keep it modest.
    ///
    /// A panic inside the callback crosses a C boundary; binaries
    /// should set `panic = "abort"` as the crate documentation
    /// recommends.
    ///
    /// # Errors
    /// Returns [`Error::TaskName`] when `name` contains a NUL byte,
    /// [`Error::TaskCreateWhileStopping`] when an audio system is being
    /// torn down — including from a `cleanup` callback, which runs
    /// inside that teardown — and [`Error::TaskCreate`] when Bela could
    /// not create the task, which is also what happens off-device,
    /// where there is no audio system to create it in.
    pub fn new<F>(name: &str, priority: Priority, callback: F) -> Result<Self, Error>
    where
        F: FnMut() + Send + 'static,
    {
        let name = CString::new(name).map_err(|_| Error::TaskName)?;
        // Deliberately leaked: the callback must stay put for as long
        // as libbela might call it, and libbela offers no per-task
        // delete to hang a free off. See the lifetime note above.
        let state: *mut F = Box::into_raw(Box::new(callback));
        // Held across the creation, so a teardown either retires the
        // handles before this task exists — and it is created into the
        // new generation — or waits until it does.
        let lifecycle = lifecycle();
        if !lifecycle.accepting {
            // A task created now would be deleted by the teardown that
            // is already under way.
            drop(unsafe { Box::from_raw(state) });
            return Err(Error::TaskCreateWhileStopping);
        }
        Self::create::<F>(&name, priority, lifecycle.generation, state)
    }

    #[cfg(bela_device)]
    fn create<F: FnMut()>(
        name: &CString,
        priority: Priority,
        generation: u64,
        state: *mut F,
    ) -> Result<Self, Error> {
        // Safety: the callback pointer and `state` outlive the task
        // (state is leaked), and Bela copies the name.
        let raw = unsafe {
            bela_sys::Bela_createAuxiliaryTask(
                Some(trampoline::<F>),
                i32::from(priority.0),
                name.as_ptr(),
                state.cast::<c_void>(),
            )
        };
        if raw.is_null() {
            // Nothing took ownership, so the state can be reclaimed.
            drop(unsafe { Box::from_raw(state) });
            return Err(Error::TaskCreate);
        }
        Ok(Self { raw, generation })
    }

    #[cfg(not(bela_device))]
    #[allow(
        clippy::unnecessary_wraps,
        reason = "mirrors the device signature, which can succeed"
    )]
    fn create<F: FnMut()>(
        _name: &CString,
        _priority: Priority,
        _generation: u64,
        state: *mut F,
    ) -> Result<Self, Error> {
        // Off-device there is no libbela to hand the task to, so the
        // state is reclaimed rather than leaked.
        drop(unsafe { Box::from_raw(state) });
        Err(Error::TaskCreate)
    }

    /// Asks for the callback to run.
    ///
    /// Real-time safe: this is the call `render` makes. It returns
    /// immediately, without waiting for the callback.
    ///
    /// # Why it takes a context
    ///
    /// The context is a witness that this is a Bela callback — any of
    /// them — which is the only place scheduling is sound. Stopping the
    /// audio system frees every task, and `Bela_stopAudio` joins the
    /// main audio thread and then every render thread before it does
    /// so, so a schedule made from a callback can never be in flight
    /// while the task behind it is freed. A handle sent to some other
    /// thread (the type is [`Send`], since applications are) has no
    /// context to schedule with, and so cannot race with that teardown.
    ///
    /// # Requests can be lost
    ///
    /// A request that arrives while the callback is still running is
    /// dropped, not queued, and nothing reports it: Bela wakes a
    /// condition variable the task is not waiting on, and its return
    /// value only says whether that wakeup could be delivered. Measured
    /// on the board, a task sleeping 2 ms scheduled from every block
    /// ran 1507 times for 9052 requests, with every request reported as
    /// successful.
    ///
    /// So do not treat one schedule as one run. When it matters, have
    /// the callback count its own invocations and compare that with the
    /// number of requests.
    ///
    /// Once the audio system this task belongs to has stopped, every
    /// task is gone and this does nothing — `cleanup` runs after that
    /// point.
    ///
    /// # First schedule of a task
    ///
    /// libbela forces a task's thread to start the first time the task
    /// is scheduled, by raising and restoring its priority. Two render
    /// threads doing that at the same time can make libbela print
    /// `Force starting scheduled thread didn't work` on standard error;
    /// nothing else comes of it, and the next schedule finds the thread
    /// started. Scheduling a task once from `setup` or `render_pre`
    /// before the render threads share it avoids the message.
    pub fn schedule(&self, _context: &impl CallbackContext) {
        if !self.is_current() {
            return;
        }
        self.schedule_raw();
    }

    /// Whether the task behind this handle still exists.
    fn is_current(&self) -> bool {
        GENERATION.load(Ordering::Acquire) == self.generation
    }

    #[cfg(bela_device)]
    fn schedule_raw(&self) {
        // Safety: the handle came from Bela_createAuxiliaryTask and the
        // check above rules out the one point where it is freed.
        //
        // The return value is deliberately ignored: it reports whether
        // the wakeup was delivered, which says nothing about whether
        // the callback will run.
        let _ = unsafe { bela_sys::Bela_scheduleAuxiliaryTask(self.raw) };
    }

    #[cfg(not(bela_device))]
    #[allow(
        clippy::unused_self,
        reason = "mirrors the device signature; unreachable because tasks cannot be created off-device"
    )]
    const fn schedule_raw(&self) {}
}

/// A handle standing in for one from the current audio system, for
/// tests in other modules.
///
/// Its pointer is never dereferenced. `cfg(test)` is not a target
/// condition, so this is compiled for the device target too — the CI
/// job that lints it builds the test targets — and there the pointer
/// reaches `Bela_scheduleAuxiliaryTask` only if a test schedules it,
/// which none does. Nothing outside `cfg(test)` can build an
/// `AuxiliaryTask` without libbela agreeing to one.
#[cfg(test)]
pub(crate) fn test_handle() -> AuxiliaryTask {
    AuxiliaryTask {
        raw: ptr::null_mut(),
        generation: GENERATION.load(Ordering::Acquire),
    }
}

/// The `extern "C"` shim libbela calls on the task's thread.
///
/// Safety: `arg` must point to a live `F` that nothing else touches.
/// The state is leaked by [`AuxiliaryTask::new`], and Bela runs one
/// invocation of a task at a time — a schedule arriving while the
/// callback runs is dropped — so the `&mut` is exclusive.
#[cfg(bela_device)]
unsafe extern "C" fn trampoline<F: FnMut()>(arg: *mut c_void) {
    let callback = unsafe { &mut *arg.cast::<F>() };
    callback();
}

#[cfg(test)]
mod tests {
    use core::time::Duration;
    use std::panic::catch_unwind;
    use std::sync::mpsc;
    use std::thread;

    use super::*;

    /// The lifecycle state is process-wide, so every test that touches
    /// it — [`teardown`] or [`AuxiliaryTask::new`], which reads the
    /// window — has to take this first, or it can observe another
    /// test's teardown. Not [`LIFECYCLE`], which the code under test
    /// takes itself.
    static SERIALISE: Mutex<()> = Mutex::new(());

    const TASK_PRIORITY: Priority = Priority::new(50).expect("50 is within Bela's priority range");

    use super::test_handle as handle;

    #[test]
    fn tearing_down_the_audio_system_retires_the_handles() {
        let _order = SERIALISE.lock().unwrap_or_else(PoisonError::into_inner);

        let task = handle();
        assert!(task.is_current(), "a fresh handle should be live");

        teardown(|| {
            assert!(
                !task.is_current(),
                "handles must be retired before anything can delete the tasks"
            );
        });

        assert!(!task.is_current(), "and stay retired afterwards");
    }

    #[test]
    fn a_later_audio_system_does_not_revive_an_earlier_handle() {
        let _order = SERIALISE.lock().unwrap_or_else(PoisonError::into_inner);

        let old = handle();
        teardown(|| ());
        // Stands in for a task created for the next audio system.
        let new = handle();

        assert!(
            !old.is_current(),
            "a handle from the previous audio system must stay retired"
        );
        assert!(new.is_current(), "the new handle should be live");
    }

    #[test]
    fn tasks_cannot_be_created_during_a_teardown() {
        let _order = SERIALISE.lock().unwrap_or_else(PoisonError::into_inner);

        teardown(|| {
            // What a `cleanup` callback sees: an explicit failure
            // rather than a deadlock or a task about to be deleted.
            let error = AuxiliaryTask::new("report", TASK_PRIORITY, || {}).unwrap_err();
            assert_eq!(
                error,
                Error::TaskCreateWhileStopping,
                "creating a task during a teardown must fail"
            );
        });

        let error = AuxiliaryTask::new("report", TASK_PRIORITY, || {}).unwrap_err();
        assert_ne!(
            error,
            Error::TaskCreateWhileStopping,
            "creation should be accepted again once the teardown is over"
        );
    }

    #[test]
    fn creating_a_task_never_blocks_on_a_teardown() {
        let _order = SERIALISE.lock().unwrap_or_else(PoisonError::into_inner);

        teardown(|| {
            // The lock must not be held across the teardown: a thread
            // asking for a task has to be turned away, not parked until
            // the window closes.
            let (sender, receiver) = mpsc::channel();
            thread::spawn(move || {
                let error = AuxiliaryTask::new("report", TASK_PRIORITY, || {}).unwrap_err();
                let _ = sender.send(error);
            });

            let error = receiver
                .recv_timeout(Duration::from_secs(5))
                .expect("creating a task blocked while an audio system was being torn down");
            assert_eq!(error, Error::TaskCreateWhileStopping);
        });
    }

    #[test]
    fn a_panicking_teardown_still_reopens_creation() {
        let _order = SERIALISE.lock().unwrap_or_else(PoisonError::into_inner);

        let panicked = catch_unwind(|| teardown(|| panic!("teardown blew up")));
        assert!(panicked.is_err(), "the panic should propagate");

        let error = AuxiliaryTask::new("report", TASK_PRIORITY, || {}).unwrap_err();
        assert_ne!(
            error,
            Error::TaskCreateWhileStopping,
            "the window must not stay closed after a panic"
        );
    }

    #[test]
    fn a_name_with_an_interior_nul_is_rejected() {
        let _order = SERIALISE.lock().unwrap_or_else(PoisonError::into_inner);

        let error = AuxiliaryTask::new("no\0pe", TASK_PRIORITY, || {}).unwrap_err();
        assert_eq!(error, Error::TaskName, "expected the name to be rejected");
    }

    #[test]
    fn tasks_cannot_be_created_off_device() {
        let _order = SERIALISE.lock().unwrap_or_else(PoisonError::into_inner);

        let error = AuxiliaryTask::new("report", TASK_PRIORITY, || {}).unwrap_err();
        assert_eq!(
            error,
            Error::TaskCreate,
            "off-device there is no audio system to create a task in"
        );
    }

    #[test]
    fn priority_accepts_exactly_belas_range() {
        assert_eq!(Priority::new(0), Some(Priority(0)));
        assert_eq!(Priority::new(99), Some(Priority(99)));
        assert_eq!(Priority::new(100), None);
    }

    #[test]
    fn audio_priority_matches_bela() {
        assert_eq!(u32::from(Priority::AUDIO.0), bela_sys::BELA_AUDIO_PRIORITY);
    }
}
