//! Auxiliary tasks: work moved off the audio thread.

#[cfg(bela_device)]
use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, Ordering};
use std::ffi::CString;

use crate::error::Error;

/// Priority of the audio thread. Auxiliary tasks should run below it,
/// otherwise they preempt audio rendering.
#[allow(
    clippy::cast_possible_wrap,
    reason = "BELA_AUDIO_PRIORITY is 95; Bela's own range is 0..99"
)]
pub const AUDIO_PRIORITY: i32 = bela_sys::BELA_AUDIO_PRIORITY as i32;

/// Whether the tasks created so far still exist.
///
/// `Bela_stopAudio` calls `Bela_deleteAllAuxiliaryTasks`, which joins
/// and frees every task, leaving the handles dangling. Scheduling one
/// afterwards — from `cleanup`, say, which runs after the stop — would
/// be a use-after-free reachable from safe code, so the handles are
/// invalidated at that point and [`AuxiliaryTask::schedule`] checks
/// this first. The check is a relaxed atomic load: cheap enough for the
/// render path.
static TASKS_ALIVE: AtomicBool = AtomicBool::new(true);

/// Marks every existing task handle as dangling.
///
/// Called after `Bela_stopAudio`, which is what deletes the tasks.
#[cfg_attr(
    not(bela_device),
    allow(
        dead_code,
        reason = "only the device-gated system module stops the audio system"
    )
)]
pub fn invalidate_all() {
    TASKS_ALIVE.store(false, Ordering::Relaxed);
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
///     fn render(&mut self, _context: &mut Context) {
///         let blocks = self.blocks.fetch_add(1, Ordering::Relaxed) + 1;
///         if blocks % 1000 == 0 {
///             if let Some(task) = &self.task {
///                 task.schedule();
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
        // A task created after a stop is alive again, along with any
        // handle that survived; the C side keeps them all in one list.
        TASKS_ALIVE.store(true, Ordering::Relaxed);
        Ok(Self { raw })
    }

    #[cfg(not(bela_device))]
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
    /// Once the audio system has stopped, every task is gone and this
    /// does nothing.
    pub fn schedule(&self) {
        if !TASKS_ALIVE.load(Ordering::Relaxed) {
            return;
        }
        self.schedule_raw();
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
    use super::*;

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
