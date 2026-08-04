//! Auxiliary tasks: work moved off the audio thread.

#[cfg(bela_device)]
use core::ffi::c_void;
use core::sync::atomic::{AtomicU64, Ordering};
use std::ffi::CString;
use std::sync::{Mutex, PoisonError};

use crate::context::Context;
use crate::error::Error;

/// Priority of the audio thread. Auxiliary tasks should run below it,
/// otherwise they preempt audio rendering.
#[allow(
    clippy::cast_possible_wrap,
    reason = "BELA_AUDIO_PRIORITY is 95; Bela's own range is 0..99"
)]
pub const AUDIO_PRIORITY: i32 = bela_sys::BELA_AUDIO_PRIORITY as i32;

/// Which set of tasks libbela currently holds.
///
/// `Bela_stopAudio` ends with `Bela_deleteAllAuxiliaryTasks`, which
/// frees every task at once and leaves the handles dangling. Each
/// handle records the generation it was created in, and the counter is
/// bumped when the audio system is torn down, so a handle from an
/// earlier audio system stays dead even after a later one creates
/// tasks of its own. Reading it is a single atomic load, which
/// [`AuxiliaryTask::schedule`] can afford on the render path.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Serialises creating a task against tearing the audio system down.
///
/// Both sides run outside the real-time context — creating a task
/// allocates and starts a thread, and stopping joins threads — so an
/// ordinary mutex is the right tool. Holding it across
/// `Bela_stopAudio` is what stops a task created concurrently from
/// being deleted by that same stop while its handle still looks
/// current.
static LIFECYCLE: Mutex<()> = Mutex::new(());

/// Runs the audio system teardown with the task handles retired first.
///
/// The generation is bumped *before* `stop` deletes the tasks, so no
/// handle can be seen as live while the memory behind it is being
/// freed.
#[cfg_attr(
    not(bela_device),
    allow(
        dead_code,
        reason = "only the device-gated system module stops the audio system"
    )
)]
pub fn with_teardown<R>(stop: impl FnOnce() -> R) -> R {
    // A poisoned lock means a panic while creating a task or stopping
    // audio; the counter is still consistent, so carry on rather than
    // panicking again on the way down.
    let _guard = LIFECYCLE.lock().unwrap_or_else(PoisonError::into_inner);
    GENERATION.fetch_add(1, Ordering::Release);
    stop()
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
/// system stops, which deletes all of them at once: the C API has no
/// way to delete one task, so dropping this handle does not destroy the
/// task, and the callback's state stays allocated for the life of the
/// process.
///
/// Because that teardown frees the tasks behind the handles, a handle
/// records which audio system it belongs to and scheduling it
/// afterwards does nothing — including from a handle that outlived one
/// audio system while a later one is running.
///
/// # Example
///
/// ```no_run
/// use core::sync::atomic::{AtomicU64, Ordering};
/// use std::sync::Arc;
///
/// use bela::{AuxiliaryTask, BelaApplication, Context, rt_println};
///
/// struct App {
///     task: Option<AuxiliaryTask>,
///     blocks: Arc<AtomicU64>,
/// }
///
/// unsafe impl BelaApplication for App {
///     fn setup(&mut self, _context: &mut Context) -> bool {
///         let blocks = Arc::clone(&self.blocks);
///         self.task = AuxiliaryTask::new("report", 50, move || {
///             rt_println!("{} blocks so far", blocks.load(Ordering::Relaxed));
///         })
///         .ok();
///         self.task.is_some()
///     }
///
///     fn render(&mut self, context: &mut Context) {
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
// side expects it to be used from the audio thread — which is where the
// application, and with it this handle, ends up.
unsafe impl Send for AuxiliaryTask {}

impl AuxiliaryTask {
    /// Creates a task that runs `callback` each time it is scheduled.
    ///
    /// `name` must be unique across the system (Bela names the
    /// underlying thread with it), and `priority` is a real-time
    /// priority between 0 and 99 that should stay below
    /// [`AUDIO_PRIORITY`].
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
    /// Returns [`Error::TaskName`] when `name` contains a NUL byte, and
    /// [`Error::TaskCreate`] when Bela could not create the task —
    /// which is also what happens off-device, where there is no audio
    /// system to create it in.
    pub fn new<F>(name: &str, priority: i32, callback: F) -> Result<Self, Error>
    where
        F: FnMut() + Send + 'static,
    {
        let name = CString::new(name).map_err(|_| Error::TaskName)?;
        // Deliberately leaked: the callback must stay put for as long
        // as libbela might call it, and libbela offers no per-task
        // delete to hang a free off. See the lifetime note above.
        let state: *mut F = Box::into_raw(Box::new(callback));
        // Held across the creation so that a teardown running
        // concurrently either happens entirely before it — and the new
        // task belongs to the new generation — or entirely after.
        let _guard = LIFECYCLE.lock().unwrap_or_else(PoisonError::into_inner);
        Self::create::<F>(&name, priority, state)
    }

    #[cfg(bela_device)]
    fn create<F: FnMut()>(name: &CString, priority: i32, state: *mut F) -> Result<Self, Error> {
        // Safety: the callback pointer and `state` outlive the task
        // (state is leaked), and Bela copies the name.
        let raw = unsafe {
            bela_sys::Bela_createAuxiliaryTask(
                Some(trampoline::<F>),
                priority,
                name.as_ptr(),
                state.cast::<c_void>(),
            )
        };
        if raw.is_null() {
            // Nothing took ownership, so the state can be reclaimed.
            drop(unsafe { Box::from_raw(state) });
            return Err(Error::TaskCreate);
        }
        Ok(Self {
            raw,
            generation: GENERATION.load(Ordering::Acquire),
        })
    }

    #[cfg(not(bela_device))]
    #[allow(
        clippy::unnecessary_wraps,
        reason = "mirrors the device signature, which can succeed"
    )]
    fn create<F: FnMut()>(_name: &CString, _priority: i32, state: *mut F) -> Result<Self, Error> {
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
    /// The `&Context` is a witness that this is a Bela callback —
    /// `setup`, `render` or `cleanup` — which is the only place
    /// scheduling is sound. Stopping the audio system frees every task,
    /// and libbela joins the render thread before it does so, so a
    /// schedule made from a callback can never be in flight while the
    /// task behind it is freed. A handle sent to some other thread
    /// (the type is [`Send`], since applications are) has no context to
    /// schedule with, and so cannot race with that teardown.
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
    pub fn schedule(&self, _context: &Context) {
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
    use core::ptr;

    use super::*;

    /// The generation is process-wide, so the tests that move it have
    /// to run one at a time. Not [`LIFECYCLE`], which
    /// [`with_teardown`] takes itself.
    static SERIALISE: Mutex<()> = Mutex::new(());

    /// A handle standing in for one from the current audio system. Its
    /// pointer is never dereferenced: off-device `schedule_raw` does
    /// nothing.
    fn handle() -> AuxiliaryTask {
        AuxiliaryTask {
            raw: ptr::null_mut(),
            generation: GENERATION.load(Ordering::Acquire),
        }
    }

    #[test]
    fn tearing_down_the_audio_system_retires_the_handles() {
        let _order = SERIALISE.lock().unwrap_or_else(PoisonError::into_inner);

        let task = handle();
        assert!(task.is_current(), "a fresh handle should be live");

        with_teardown(|| ());

        assert!(
            !task.is_current(),
            "stopping deletes the task behind the handle"
        );
    }

    #[test]
    fn a_later_audio_system_does_not_revive_an_earlier_handle() {
        let _order = SERIALISE.lock().unwrap_or_else(PoisonError::into_inner);

        let old = handle();
        with_teardown(|| ());
        // Stands in for a task created for the next audio system.
        let new = handle();

        assert!(
            !old.is_current(),
            "a handle from the previous audio system must stay retired"
        );
        assert!(new.is_current(), "the new handle should be live");
    }

    #[test]
    fn a_name_with_an_interior_nul_is_rejected() {
        let error = AuxiliaryTask::new("no\0pe", 50, || {}).unwrap_err();
        assert_eq!(error, Error::TaskName, "expected the name to be rejected");
    }

    #[test]
    fn tasks_cannot_be_created_off_device() {
        let error = AuxiliaryTask::new("report", 50, || {}).unwrap_err();
        assert_eq!(
            error,
            Error::TaskCreate,
            "off-device there is no audio system to create a task in"
        );
    }

    #[test]
    fn audio_priority_matches_bela() {
        assert_eq!(AUDIO_PRIORITY, 95, "BELA_AUDIO_PRIORITY changed");
    }
}
