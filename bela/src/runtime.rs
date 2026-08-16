//! What sits between libbela's C callbacks and a
//! [`BelaApplication`](crate::BelaApplication).
//!
//! libbela hands the same `void* userData` to every callback, on every
//! render thread, all at once. Turning that into the references the
//! trait promises — `&mut self` for the single-threaded phases, `&self`
//! plus one exclusive [`RenderState`] for each concurrent `render` — is
//! this module's job, and it is where the `unsafe` for it lives.
//!
//! # Failing closed
//!
//! The references are only sound while libbela keeps to the callback
//! protocol it was measured to keep:
//!
//! - `setup` and `cleanup` are made once, on their own;
//! - `render_pre` runs before the render threads are woken and
//!   `render_post` after the last of them has finished;
//! - each `render` for a block arrives on a different thread number,
//!   below the thread count the context reports.
//!
//! That is an implementation detail of a C library rather than a
//! documented guarantee, and at least one edge of it is thin: when a
//! stop is requested mid-block, `render_wrapper` gives up waiting for
//! the secondary threads and calls `render_post` anyway, which can
//! overlap a `render` that has not finished.
//!
//! So the protocol is checked rather than assumed. [`Guard`] is a pair
//! of non-blocking atomic claims — one exclusive claim for the
//! single-threaded phases, one slot per render thread — and every
//! reference is built *after* the claim is granted. A callback that
//! cannot claim what it needs records a fault, asks the audio system to
//! stop, and returns without running any user code at all: no aliased
//! reference is ever created, not even briefly.
//!
//! # The one thing assumed rather than checked
//!
//! Concurrent `render` calls must arrive with *different* `BelaContext`
//! structs. libbela's are `BelaContextSplitter::contextMirror` copies,
//! one per thread — measured, and the reason the buffer pointers are
//! shared while `thisThread` is not — so each call has a struct of its
//! own to borrow. A guard cannot check that: two threads holding
//! distinct claims would still be holding one struct if libbela ever
//! passed the same pointer to both.
//!
//! What the guard does cover is that the claim comes first. The thread
//! number is read straight from the raw pointer, without a reference,
//! and the claim is taken before anything is borrowed — so a `render`
//! that is refused never touches the context at all.
//!
//! [`RenderState`]: crate::BelaApplication::RenderState

use core::cell::UnsafeCell;
use core::ffi::c_void;
use core::ptr;
use core::slice;
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, Ordering};

use bela_sys::BelaContext;

use crate::application::{BelaApplication, ThreadInfo};
use crate::context::{BlockContext, CleanupContext, RenderContext, SetupContext};

/// Bit of [`Guard::state`] set while a single-threaded phase runs. The
/// bits below it count the render calls in flight.
const EXCLUSIVE: u64 = 1 << 63;

/// Asks the audio system to stop, the way a protocol violation does.
///
/// Off-device there is no audio system to ask; the fault counter is
/// what the tests read instead.
#[cfg(bela_device)]
pub(crate) fn request_stop() {
    // Safety: Bela_requestStop only sets a flag, and is documented as
    // callable from anywhere, including a signal handler.
    unsafe { bela_sys::Bela_requestStop() }
}

#[cfg(not(bela_device))]
const fn request_stop() {}

/// Whether a stop has already been asked for, by anyone.
///
/// What tells an expected refusal from an anomalous one — see
/// [`Guard::fault`]. Off-device nothing can request a stop, so every
/// fault a test raises counts as one raised while running, which is
/// what those tests are about. [`crate::stop_requested`] is the same
/// answer, published: it exists on both targets, so callback code
/// that reacts to a pending stop needs no `cfg` of its own.
///
/// `Bela_stopRequested` reads `int volatile gShouldStop`, which is
/// volatile rather than atomic: a thread that has not been told about
/// the store by some other means is not guaranteed to see it. Every
/// caller here is a thread that has been — see
/// [`Guard::enter_exclusive`] for the one place that needed arranging
/// rather than observing.
#[cfg(bela_device)]
pub(crate) fn stop_requested() -> bool {
    // Safety: Bela_stopRequested only reads a flag.
    unsafe { bela_sys::Bela_stopRequested() != 0 }
}

#[cfg(not(bela_device))]
pub(crate) const fn stop_requested() -> bool {
    false
}

/// The non-blocking claims that keep the callback phases apart.
struct Guard {
    /// [`EXCLUSIVE`] plus the number of `render` calls in flight, in
    /// one word so that claiming either is a single atomic operation.
    state: AtomicU64,
    /// One flag per render thread, so that the same thread number
    /// cannot be inside `render` twice.
    busy: Box<[AtomicBool]>,
    /// How many callbacks have been refused while the run was live.
    /// Never reset.
    faults: AtomicU32,
    /// How many have been refused after a stop was already asked for,
    /// which is a different thing entirely — see [`Guard::fault`].
    faults_while_stopping: AtomicU32,
    /// What the thread taking the current exclusive claim saw of the
    /// stop flag, published by the claim itself.
    ///
    /// Only meaningful to a thread that has observed that claim, which
    /// is what gives it an ordering — see
    /// [`enter_exclusive`](Guard::enter_exclusive).
    stopping: AtomicBool,
}

#[cfg_attr(
    not(bela_device),
    allow(
        dead_code,
        reason = "only the device-gated system module builds a runtime; still unit-tested on the host"
    )
)]
impl Guard {
    fn new(threads: usize) -> Self {
        Self {
            state: AtomicU64::new(0),
            busy: (0..threads).map(|_| AtomicBool::new(false)).collect(),
            faults: AtomicU32::new(0),
            faults_while_stopping: AtomicU32::new(0),
            stopping: AtomicBool::new(false),
        }
    }

    /// How many render threads this guard was built for.
    fn threads(&self) -> usize {
        self.busy.len()
    }

    fn faults(&self) -> u32 {
        self.faults.load(Ordering::Relaxed)
    }

    fn faults_while_stopping(&self) -> u32 {
        self.faults_while_stopping.load(Ordering::Relaxed)
    }

    /// Records a refused callback, and asks the audio system to stop if
    /// it is not stopping already.
    ///
    /// # Two kinds of refusal
    ///
    /// A refusal *while the run is live* means something is wrong: the
    /// callbacks are not arriving in a shape the render states can be
    /// handed out for, and there is no reason to expect the next block
    /// to be better. It is counted, reported, and the run is stopped.
    ///
    /// A refusal *after a stop has been asked for* is the shutdown
    /// libbela does, seen from inside. `render_wrapper` waits for the
    /// secondary threads in
    /// `while(!allThreadsDone && !Bela_stopRequested())`, and the
    /// threads themselves check the same flag just before calling
    /// `render` — so a stop landing in between leaves one of them
    /// inside `render` while the main thread gives up waiting and calls
    /// `render_post`. Refusing that `render_post` is the guard working,
    /// not a symptom: the block is being abandoned anyway. It is
    /// counted separately, and asking for a stop that has already been
    /// asked for would be pointless.
    ///
    /// Keeping the two apart is what lets an ordinary Ctrl-C stay an
    /// ordinary Ctrl-C. Mixing them would make
    /// [`Error::CallbackFaults`](crate::Error::CallbackFaults) fire on
    /// a clean shutdown that happened to land in that window.
    ///
    /// Real-time safe either way: atomic increments, a flag read, a
    /// flag store, and — for the first live fault only, so that a run
    /// cannot be drowned in them — one real-time safe line, since a
    /// program stopped for that reason has no other way of finding out
    /// why.
    fn fault(&self) {
        self.record_fault(stop_requested());
    }

    /// [`fault`](Guard::fault) with the answer supplied, so that both
    /// sides of the split can be tested off-device — where nothing can
    /// request a stop, and so nothing would ever take the second one.
    fn record_fault(&self, while_stopping: bool) {
        if while_stopping {
            self.faults_while_stopping.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if self.faults.fetch_add(1, Ordering::Relaxed) == 0 {
            crate::rt_println!(
                "bela: a callback broke the protocol the render states rely on and was refused; \
                 stopping"
            );
        }
        request_stop();
    }

    /// Claims the runtime for a single-threaded phase, which needs
    /// every render thread to be out.
    fn enter_exclusive(&self) -> Option<Exclusive<'_>> {
        self.enter_exclusive_seeing(stop_requested())
    }

    /// [`enter_exclusive`](Guard::enter_exclusive) with the stop flag
    /// already read, so that both answers can be tested off-device.
    ///
    /// # Publishing what was seen
    ///
    /// A render refused because this claim is held is refused *on
    /// another thread*, and that thread's own read of the stop flag
    /// proves nothing: it passed its check before the flag was set, and
    /// `gShouldStop` is volatile rather than atomic, so re-reading it
    /// carries no guarantee of seeing the store.
    ///
    /// The claim itself carries the answer instead. `stopping` is
    /// stored before the claim is taken and the claim is taken with
    /// `AcqRel`, so a thread that loads the word with `Acquire` and
    /// finds [`EXCLUSIVE`] set has synchronised with that store and is
    /// guaranteed to read what this thread saw. Which is the useful
    /// question anyway: what matters is whether the phase that turned
    /// the render away was itself part of a shutdown.
    fn enter_exclusive_seeing(&self, stopping: bool) -> Option<Exclusive<'_>> {
        // Before the claim, so the claim publishes it. Meaningless
        // until then, since only a thread that observes the claim reads
        // it.
        self.stopping.store(stopping, Ordering::Relaxed);
        // Only from a completely idle state: a non-zero word is either
        // another exclusive phase or a render still in flight.
        if self
            .state
            .compare_exchange(0, EXCLUSIVE, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            // This thread is the one that read the flag, so its own
            // reading is the right one to classify by.
            self.record_fault(stopping);
            return None;
        }
        Some(Exclusive { guard: self })
    }

    /// Claims render slot `thread`.
    fn enter_render(&self, thread: usize) -> Option<Active<'_>> {
        let Some(busy) = self.busy.get(thread) else {
            // A thread number the states do not reach. Nothing about a
            // shutdown produces that, so it is classified by the flag
            // like any other anomaly.
            self.fault();
            return None;
        };
        // Joining a block an exclusive phase is in would alias what
        // that phase holds by &mut.
        if self
            .state
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                (state & EXCLUSIVE == 0).then(|| state + 1)
            })
            .is_err()
        {
            // Acquired the word above and found EXCLUSIVE set, so the
            // store that phase made before taking it is visible here.
            // That reading, not this thread's own, is the one that
            // means anything.
            self.record_fault(self.stopping.load(Ordering::Relaxed));
            return None;
        }
        if busy.swap(true, Ordering::Acquire) {
            // This thread number is already inside `render`, so its
            // state is already held by &mut somewhere.
            self.state.fetch_sub(1, Ordering::Release);
            self.fault();
            return None;
        }
        Some(Active {
            guard: self,
            thread,
        })
    }
}

/// A granted exclusive claim, released on drop.
struct Exclusive<'a> {
    guard: &'a Guard,
}

impl Drop for Exclusive<'_> {
    fn drop(&mut self) {
        self.guard.state.store(0, Ordering::Release);
    }
}

/// A granted render slot, released on drop.
struct Active<'a> {
    guard: &'a Guard,
    thread: usize,
}

impl Drop for Active<'_> {
    fn drop(&mut self) {
        self.guard.busy[self.thread].store(false, Ordering::Release);
        self.guard.state.fetch_sub(1, Ordering::Release);
    }
}

/// Owns the application and its per-thread render states, and hands
/// them to the callbacks under [`Guard`].
pub(crate) struct Runtime<T: BelaApplication> {
    /// `&mut` during the single-threaded phases, `&` during `render`;
    /// which of the two is what the guard decides.
    app: UnsafeCell<T>,
    /// Owns the render states. Filled once, by `setup`, and not
    /// reached through this handle again: everything afterwards goes
    /// through [`Runtime::states`], so that the references the render
    /// threads hold all descend from one pointer.
    storage: UnsafeCell<Vec<T::RenderState>>,
    /// The render states, published when `setup` finished building
    /// them and null until then.
    states: AtomicPtr<T::RenderState>,
    guard: Guard,
}

// Safety: the fields are only reached through the claims Guard grants,
// which never overlap a &mut with anything else — see the module
// documentation. What crosses threads is `T` by shared reference and
// one `T::RenderState` by exclusive reference, which is what the
// trait's `Sync` and `Send` bounds are for.
//
// Only `Sync` is spelled out. `Send` follows from the fields, and
// claiming it by hand would go on holding after a field that is not
// `Send` was added.
unsafe impl<T: BelaApplication> Sync for Runtime<T> {}

#[cfg_attr(
    not(bela_device),
    allow(
        dead_code,
        reason = "only the device-gated system module builds a runtime; still unit-tested on the host"
    )
)]
impl<T: BelaApplication> Runtime<T> {
    /// Prepares `application` to be rendered on `threads` threads.
    ///
    /// A `threads` of 0 is read as 1, matching
    /// [`RenderContext::thread_count`]: libbela passes
    /// [`Settings::thread_count`](crate::Settings::thread_count)
    /// through unchanged and creates extra threads only above 1, so
    /// both spellings mean the same single render thread.
    pub(crate) fn new(application: T, threads: usize) -> Self {
        let threads = threads.max(1);
        Self {
            app: UnsafeCell::new(application),
            storage: UnsafeCell::new(Vec::with_capacity(threads)),
            states: AtomicPtr::new(ptr::null_mut()),
            guard: Guard::new(threads),
        }
    }

    /// How many callbacks have been refused while the run was live.
    ///
    /// Zero for every run that behaved; anything else means the audio
    /// system was asked to stop because a callback arrived somewhere it
    /// could not be served safely. See [`Guard::fault`] for what this
    /// deliberately does not count.
    pub(crate) fn faults(&self) -> u32 {
        self.guard.faults()
    }

    /// How many callbacks have been refused after a stop was already
    /// asked for.
    ///
    /// Expected rather than alarming, and usually 0 or 1: see
    /// [`Guard::fault`].
    pub(crate) fn faults_while_stopping(&self) -> u32 {
        self.guard.faults_while_stopping()
    }

    /// Whether `threads` is the number of render states this runtime
    /// was built with, faulting if it is not.
    ///
    /// The states and the slots were sized from the settings, before
    /// `Bela_initAudio` was called. A context that renders on a
    /// different number of threads would leave some of them without
    /// either, and the partitions the contexts hand out would not tile
    /// the block: some frames written by two threads, some by none.
    ///
    /// Checked in every phase, so that `states.len()` and
    /// `thread_count()` are the same number wherever an application
    /// can see both.
    fn threads_agree(&self, threads: usize) -> bool {
        if threads == self.guard.threads() {
            return true;
        }
        self.guard.fault();
        false
    }

    /// # Safety
    /// `context` must satisfy [`SetupContext::from_mut_ptr`].
    unsafe fn setup(&self, context: *mut BelaContext) -> bool {
        let Some(exclusive) = self.guard.enter_exclusive() else {
            return false;
        };
        // Safety: the caller's contract, and the claim above means no
        // other callback holds this context.
        let context: &SetupContext = unsafe { SetupContext::from_mut_ptr(context) };
        if !self.threads_agree(context.thread_count()) {
            return false;
        }
        if !self.states.load(Ordering::Acquire).is_null() {
            // A second setup would build a second set of states over
            // the ones the render threads are already using.
            self.guard.fault();
            return false;
        }
        // Safety: the exclusive claim is what makes this the only
        // reference to the application.
        let app = unsafe { &mut *self.app.get() };
        if !app.setup(context) {
            return false;
        }
        // Safety: as above, and nothing reads `states` until it is
        // published below.
        let storage = unsafe { &mut *self.storage.get() };
        let threads = self.guard.threads();
        for index in 0..threads {
            storage.push(app.create_render_state(ThreadInfo::new(index, threads), context));
        }
        // Release: the render threads acquire this pointer, and with it
        // everything written into the states above.
        self.states.store(storage.as_mut_ptr(), Ordering::Release);
        drop(exclusive);
        true
    }

    /// # Safety
    /// `context` must satisfy [`RenderContext::from_mut_ptr`].
    unsafe fn render(&self, context: *mut BelaContext) {
        // A field read through the raw pointer, not a reference: the
        // claim below is what makes borrowing this context sound, so
        // nothing may be borrowed ahead of it.
        //
        // Safety: the caller's contract.
        let thread = unsafe { (*context).thisThread } as usize;
        let Some(active) = self.guard.enter_render(thread) else {
            return;
        };
        // Safety: the caller's contract, plus the claim — and libbela
        // gives each render thread a mirrored copy of the context
        // struct to itself, which is the assumption the module
        // documentation names as the one the guard cannot check.
        let context = unsafe { RenderContext::from_mut_ptr(context) };
        if !self.threads_agree(context.thread_count()) {
            return;
        }
        let base = self.states.load(Ordering::Acquire);
        if base.is_null() {
            // Rendering before setup finished building the states.
            self.guard.fault();
            return;
        }
        // Safety: the claim gives this call thread number
        // `active.thread` alone, and the numbers are distinct, so the
        // states the concurrent calls reach are distinct elements of
        // one allocation. `enter_render` has already checked that the
        // number is below the length.
        let state = unsafe { &mut *base.add(active.thread) };
        // Safety: the guard keeps the exclusive phases out for as long
        // as this claim is held, so nothing holds the application by
        // &mut.
        let app = unsafe { &*self.app.get() };
        app.render(state, context);
    }

    /// # Safety
    /// `context` must satisfy [`BlockContext::from_mut_ptr`].
    unsafe fn render_pre(&self, context: *mut BelaContext) {
        let Some(exclusive) = self.guard.enter_exclusive() else {
            return;
        };
        let Some(states) = self.states_mut(&exclusive) else {
            return;
        };
        // Safety: the caller's contract, plus the claim.
        let context = unsafe { BlockContext::from_mut_ptr(context) };
        if !self.threads_agree(context.thread_count()) {
            return;
        }
        // Safety: the exclusive claim.
        let app = unsafe { &mut *self.app.get() };
        app.render_pre(states, context);
    }

    /// # Safety
    /// `context` must satisfy [`BlockContext::from_mut_ptr`].
    unsafe fn render_post(&self, context: *mut BelaContext) {
        let Some(exclusive) = self.guard.enter_exclusive() else {
            return;
        };
        let Some(states) = self.states_mut(&exclusive) else {
            return;
        };
        // Safety: the caller's contract, plus the claim.
        let context = unsafe { BlockContext::from_mut_ptr(context) };
        if !self.threads_agree(context.thread_count()) {
            return;
        }
        // Safety: the exclusive claim.
        let app = unsafe { &mut *self.app.get() };
        app.render_post(states, context);
    }

    /// # Safety
    /// `context` must satisfy [`CleanupContext::from_mut_ptr`].
    unsafe fn cleanup(&self, context: *mut BelaContext) {
        let Some(exclusive) = self.guard.enter_exclusive() else {
            return;
        };
        let Some(states) = self.states_mut(&exclusive) else {
            return;
        };
        // Safety: the caller's contract, plus the claim.
        let context: &CleanupContext = unsafe { CleanupContext::from_mut_ptr(context) };
        if !self.threads_agree(context.thread_count()) {
            return;
        }
        // Safety: the exclusive claim.
        let app = unsafe { &mut *self.app.get() };
        app.cleanup(states, context);
    }

    /// Every render state at once, for a phase that holds the
    /// exclusive claim — which is what the witness argument is.
    ///
    /// `None`, with a fault recorded, before `setup` has published
    /// them.
    #[allow(
        clippy::mut_from_ref,
        reason = "the exclusive claim, not the &self, is what makes this the only reference"
    )]
    fn states_mut(&self, _exclusive: &Exclusive<'_>) -> Option<&mut [T::RenderState]> {
        let base = self.states.load(Ordering::Acquire);
        if base.is_null() {
            self.guard.fault();
            return None;
        }
        // Safety: `setup` published a pointer to a Vec of exactly this
        // many states and nothing has resized it since; the claim
        // means no render thread holds one of them.
        Some(unsafe { slice::from_raw_parts_mut(base, self.guard.threads()) })
    }
}

/// `extern "C"` shims installed into `BelaInitSettings`, bridging the C
/// callbacks to the [`Runtime`] reached through `userData`.
///
/// Safety contract shared by all five: `context` must be a valid
/// `BelaContext` for the callback being made, and `user_data` must
/// point to a live `Runtime<T>` that outlives the call.
#[cfg_attr(
    not(bela_device),
    allow(
        dead_code,
        reason = "only called by the device-gated system module; still unit-tested on the host"
    )
)]
pub(crate) mod trampoline {
    use core::ffi::c_void;

    use bela_sys::BelaContext;

    use super::{BelaApplication, Runtime};

    pub(crate) unsafe extern "C" fn setup<T: BelaApplication>(
        context: *mut BelaContext,
        user_data: *mut c_void,
    ) -> bool {
        let runtime = unsafe { &*user_data.cast::<Runtime<T>>() };
        unsafe { runtime.setup(context) }
    }

    pub(crate) unsafe extern "C" fn render_pre<T: BelaApplication>(
        context: *mut BelaContext,
        user_data: *mut c_void,
    ) {
        let runtime = unsafe { &*user_data.cast::<Runtime<T>>() };
        unsafe { runtime.render_pre(context) };
    }

    pub(crate) unsafe extern "C" fn render<T: BelaApplication>(
        context: *mut BelaContext,
        user_data: *mut c_void,
    ) {
        let runtime = unsafe { &*user_data.cast::<Runtime<T>>() };
        unsafe { runtime.render(context) };
    }

    pub(crate) unsafe extern "C" fn render_post<T: BelaApplication>(
        context: *mut BelaContext,
        user_data: *mut c_void,
    ) {
        let runtime = unsafe { &*user_data.cast::<Runtime<T>>() };
        unsafe { runtime.render_post(context) };
    }

    pub(crate) unsafe extern "C" fn cleanup<T: BelaApplication>(
        context: *mut BelaContext,
        user_data: *mut c_void,
    ) {
        let runtime = unsafe { &*user_data.cast::<Runtime<T>>() };
        unsafe { runtime.cleanup(context) };
    }
}

/// Keeps `user_data` honest at the call sites: the pointer handed to
/// libbela and the one the trampolines expect are the same thing.
#[cfg_attr(
    not(bela_device),
    allow(
        dead_code,
        reason = "only the device-gated system module hands the pointer over"
    )
)]
pub(crate) const fn user_data<T: BelaApplication>(runtime: *mut Runtime<T>) -> *mut c_void {
    runtime.cast::<c_void>()
}

#[cfg(test)]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::float_cmp,
    reason = "tests use small exact values where these casts and comparisons are lossless"
)]
mod tests {
    use std::sync::{Arc, Barrier};
    use std::thread;

    use super::*;
    use crate::context::tests::Fixture;

    /// A raw pointer that may be handed to another thread, standing in
    /// for what libbela does with the mirrored contexts and the user
    /// data.
    #[derive(Clone, Copy)]
    struct Shared<T>(*mut T);

    // Safety: the tests only send pointers to values that outlive every
    // thread they are sent to, and the runtime is what synchronises the
    // access.
    unsafe impl<T> Send for Shared<T> {}

    /// One mirrored context per render thread: a memcpy of the same
    /// struct with only the thread number changed, which is what
    /// `BelaContextSplitter::contextMirror` produces.
    fn mirrors(fixture: &Fixture, threads: usize) -> Vec<BelaContext> {
        (0..threads)
            .map(|thread| {
                let mut mirror = fixture.context;
                mirror.thisThread = thread as u32;
                mirror
            })
            .collect()
    }

    /// Records which callbacks ran, and writes its thread number into
    /// the frames it owns.
    #[derive(Default)]
    struct Recorder {
        setup_calls: AtomicU32,
        pre_calls: AtomicU32,
        render_calls: AtomicU32,
        post_calls: AtomicU32,
        cleanup_calls: AtomicU32,
        setup_ok: bool,
    }

    /// Per-thread state carrying the thread number it was built for, so
    /// that a state reaching the wrong `render` would show up.
    #[derive(Debug, PartialEq, Eq)]
    struct Slot {
        thread: usize,
        renders: usize,
    }

    impl BelaApplication for Recorder {
        type RenderState = Slot;

        fn setup(&mut self, _context: &SetupContext) -> bool {
            self.setup_calls.fetch_add(1, Ordering::Relaxed);
            self.setup_ok
        }

        fn create_render_state(&mut self, thread: ThreadInfo, _context: &SetupContext) -> Slot {
            Slot {
                thread: thread.index(),
                renders: 0,
            }
        }

        fn render_pre(&mut self, _states: &mut [Slot], _context: &mut BlockContext) {
            self.pre_calls.fetch_add(1, Ordering::Relaxed);
        }

        fn render(&self, state: &mut Slot, context: &mut RenderContext) {
            self.render_calls.fetch_add(1, Ordering::Relaxed);
            state.renders += 1;
            assert_eq!(
                state.thread,
                context.this_thread(),
                "each render must get its own thread's state"
            );
            let marker = state.thread as f32 + 1.0;
            for frame in context.audio_frame_range() {
                context.audio_write(frame, 0, marker);
            }
            // The analog and digital buffers as well, because they are
            // shared the same way — and the digital words are read and
            // written through the same pointer, so a reader that
            // reached past this thread's range would race a writer.
            for frame in context.analog_frame_range() {
                context.analog_write_once(frame, 0, marker);
            }
            for frame in context.digital_frame_range() {
                context.digital_write_once(frame, state.thread, true);
                assert!(
                    context.digital_read(frame, state.thread),
                    "a thread should read back what it just wrote"
                );
            }
        }

        fn render_post(&mut self, _states: &mut [Slot], _context: &mut BlockContext) {
            self.post_calls.fetch_add(1, Ordering::Relaxed);
        }

        fn cleanup(&mut self, _states: &mut [Slot], _context: &CleanupContext) {
            self.cleanup_calls.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn recorder() -> Recorder {
        Recorder {
            setup_ok: true,
            ..Recorder::default()
        }
    }

    /// The counts a run left behind, read once the runtime is done
    /// with the application.
    fn counts(runtime: &Runtime<Recorder>) -> [u32; 5] {
        // Safety: the test holds the runtime by &mut in spirit — every
        // callback has returned by the time this is called.
        let app = unsafe { &*runtime.app.get() };
        [
            app.setup_calls.load(Ordering::Relaxed),
            app.pre_calls.load(Ordering::Relaxed),
            app.render_calls.load(Ordering::Relaxed),
            app.post_calls.load(Ordering::Relaxed),
            app.cleanup_calls.load(Ordering::Relaxed),
        ]
    }

    #[test]
    fn a_block_runs_pre_then_render_then_post() {
        let mut fixture = Fixture::new();
        let runtime = Runtime::new(recorder(), 1);
        let context = &raw mut fixture.context;

        unsafe {
            assert!(runtime.setup(context));
            runtime.render_pre(context);
            runtime.render(context);
            runtime.render_post(context);
            runtime.cleanup(context);
        }

        assert_eq!(counts(&runtime), [1, 1, 1, 1, 1]);
        assert_eq!(runtime.faults(), 0);
    }

    #[test]
    fn a_setup_that_aborts_leaves_no_states() {
        let mut fixture = Fixture::new();
        let runtime = Runtime::new(Recorder::default(), 1);
        let context = &raw mut fixture.context;

        assert!(!unsafe { runtime.setup(context) });
        assert!(
            runtime.states.load(Ordering::Acquire).is_null(),
            "no render state should exist for an application that refused to start"
        );
        // Not a protocol violation: the application said no.
        assert_eq!(runtime.faults(), 0);
    }

    #[test]
    fn rendering_before_setup_is_refused() {
        let mut fixture = Fixture::new();
        let runtime = Runtime::new(recorder(), 1);

        unsafe { runtime.render(&raw mut fixture.context) };

        assert_eq!(counts(&runtime)[2], 0, "render must not have run");
        assert_eq!(runtime.faults(), 1);
    }

    #[test]
    fn the_block_phases_and_cleanup_are_refused_before_setup() {
        // No states exist yet, so there is no `&mut [RenderState]` to
        // hand out — not even an empty one, which would be a different
        // number of threads from the one the run was built for.
        let mut fixture = Fixture::new();
        let runtime = Runtime::new(recorder(), 1);
        let context = &raw mut fixture.context;

        unsafe {
            runtime.render_pre(context);
            runtime.render_post(context);
            runtime.cleanup(context);
        }

        assert_eq!(counts(&runtime), [0, 0, 0, 0, 0], "no callback should run");
        assert_eq!(runtime.faults(), 3);
    }

    #[test]
    fn a_second_setup_is_refused() {
        let mut fixture = Fixture::new();
        let runtime = Runtime::new(recorder(), 1);
        let context = &raw mut fixture.context;

        assert!(unsafe { runtime.setup(context) });
        assert!(!unsafe { runtime.setup(context) });

        assert_eq!(counts(&runtime)[0], 1, "the application saw one setup");
        assert_eq!(runtime.faults(), 1);
    }

    #[test]
    fn a_context_with_the_wrong_thread_count_is_refused() {
        // The runtime was sized for one thread; a context that says
        // four would leave three of them without a state.
        let mut fixture = Fixture::with_threads(4);
        let runtime = Runtime::new(recorder(), 1);
        let context = &raw mut fixture.context;

        assert!(!unsafe { runtime.setup(context) });
        assert_eq!(runtime.faults(), 1);

        unsafe { runtime.render(context) };
        assert_eq!(runtime.faults(), 2);
        assert_eq!(counts(&runtime)[2], 0);
    }

    #[test]
    fn a_thread_number_outside_the_states_is_refused() {
        let mut fixture = Fixture::with_threads(2);
        let runtime = Runtime::new(recorder(), 2);
        assert!(unsafe { runtime.setup(&raw mut fixture.context) });

        // A mirrored context claiming to be thread 2 of 2.
        let mut rogue = fixture.context;
        rogue.thisThread = 2;
        unsafe { runtime.render(&raw mut rogue) };

        assert_eq!(counts(&runtime)[2], 0, "render must not have run");
        assert_eq!(runtime.faults(), 1);
    }

    #[test]
    fn a_zero_thread_count_is_one_render_thread() {
        // libbela passes threadCount through unchanged, so 0 arrives
        // as 0 and means the one thread that always renders.
        let mut fixture = Fixture::with_threads(0);
        let runtime = Runtime::new(recorder(), 0);
        let context = &raw mut fixture.context;

        unsafe {
            assert!(runtime.setup(context));
            runtime.render_pre(context);
            runtime.render(context);
            runtime.render_post(context);
        }

        assert_eq!(counts(&runtime), [1, 1, 1, 1, 0]);
        assert_eq!(runtime.faults(), 0);
        assert_eq!(
            fixture.audio_out[0], 1.0,
            "the one thread rendered the whole block"
        );
    }

    #[test]
    fn more_threads_than_frames_renders_empty_shares() {
        // Four frames over eight threads: half of them own nothing,
        // and must still be called and still write nothing.
        let mut fixture = Fixture::with_threads(8);
        let runtime = Runtime::new(recorder(), 8);
        assert!(unsafe { runtime.setup(&raw mut fixture.context) });

        let mut mirrors = mirrors(&fixture, 8);
        for mirror in &mut mirrors {
            unsafe { runtime.render(&raw mut *mirror) };
        }

        assert_eq!(counts(&runtime)[2], 8);
        assert_eq!(runtime.faults(), 0);
        // Threads 1, 3, 5 and 7 own one frame each, in order, and
        // each writes its own number plus one.
        let channels = fixture.context.audioOutChannels as usize;
        let written: Vec<f32> = (0..4).map(|f| fixture.audio_out[f * channels]).collect();
        assert_eq!(written, vec![2.0, 4.0, 6.0, 8.0]);
    }

    // --- Concurrency ---

    #[test]
    fn every_thread_renders_its_own_share_of_the_shared_buffers() {
        const THREADS: usize = 4;

        let mut fixture = Fixture::with_threads(THREADS as u32);
        let runtime = Runtime::new(recorder(), THREADS);
        assert!(unsafe { runtime.setup(&raw mut fixture.context) });

        let mut mirrors = mirrors(&fixture, THREADS);
        let runtime = Arc::new(runtime);
        // Makes the calls overlap, which is what the guard is about.
        let start = Arc::new(Barrier::new(THREADS));

        thread::scope(|scope| {
            for mirror in &mut mirrors {
                let context = Shared(&raw mut *mirror);
                let runtime = Arc::clone(&runtime);
                let start = Arc::clone(&start);
                scope.spawn(move || {
                    // Captured whole, so that the pointer travels
                    // inside the wrapper that is Send.
                    let context = context;
                    start.wait();
                    unsafe { runtime.render(context.0) };
                });
            }
        });

        assert_eq!(runtime.faults(), 0);
        // Each thread wrote its own number into its own frame, so the
        // block is covered exactly once.
        let channels = fixture.context.audioOutChannels as usize;
        let written: Vec<f32> = (0..4).map(|f| fixture.audio_out[f * channels]).collect();
        assert_eq!(written, vec![1.0, 2.0, 3.0, 4.0]);
        // The same for the buffers whose words the threads share.
        let analog_channels = fixture.context.analogOutChannels as usize;
        let analog: Vec<f32> = (0..4)
            .map(|f| fixture.analog_out[f * analog_channels])
            .collect();
        assert_eq!(analog, vec![1.0, 2.0, 3.0, 4.0]);
        let digital: Vec<u32> = (0..4).map(|f| fixture.digital[f]).collect();
        assert_eq!(digital, (0..4).map(|t| 1 << (t + 16)).collect::<Vec<u32>>());
    }

    #[test]
    fn a_block_phase_arriving_while_a_render_is_in_flight_is_refused() {
        // What a stop requested mid-block can produce: libbela gives
        // up waiting for the secondary threads and calls render_post
        // over states one of them is still holding.
        let mut fixture = Fixture::with_threads(2);
        let runtime = Arc::new(Runtime::new(Blocker::new(true), 2));
        assert!(unsafe { runtime.setup(&raw mut fixture.context) });

        let mut mirrors = mirrors(&fixture, 2);
        let inside = Arc::clone(&unsafe { &*runtime.app.get() }.inside);
        let release = Arc::clone(&unsafe { &*runtime.app.get() }.release);
        let context = Shared(&raw mut mirrors[1]);

        thread::scope(|scope| {
            let rendering = {
                let runtime = Arc::clone(&runtime);
                scope.spawn(move || {
                    let context = context;
                    unsafe { runtime.render(context.0) };
                })
            };
            // Wait until that render is definitely inside the callback.
            inside.wait();
            unsafe { runtime.render_post(&raw mut fixture.context) };
            release.wait();
            rendering
                .join()
                .expect("the render thread should not panic");
        });

        // A live fault here because nothing off-device can request a
        // stop. On a board this same overlap arrives during a shutdown,
        // where it lands in the other counter — see `Guard::fault`.
        assert_eq!(
            runtime.faults(),
            1,
            "the block phase must have been refused"
        );
        assert_eq!(
            unsafe { &*runtime.app.get() }
                .post_calls
                .load(Ordering::Relaxed),
            0,
            "render_post must not have run"
        );
    }

    /// Holds one callback open until the test lets it go, so that
    /// another phase can be tried while it is in flight.
    struct Blocker {
        /// Whether `render` is the callback that waits; `render_pre`
        /// waits instead when this is false.
        block_render: bool,
        inside: Arc<Barrier>,
        release: Arc<Barrier>,
        render_calls: AtomicU32,
        post_calls: AtomicU32,
    }

    impl Blocker {
        fn new(block_render: bool) -> Self {
            Self {
                block_render,
                inside: Arc::new(Barrier::new(2)),
                release: Arc::new(Barrier::new(2)),
                render_calls: AtomicU32::new(0),
                post_calls: AtomicU32::new(0),
            }
        }

        /// Waits for the test to look, and then for it to let go.
        fn hold(&self) {
            self.inside.wait();
            self.release.wait();
        }
    }

    impl BelaApplication for Blocker {
        type RenderState = ();

        fn create_render_state(&mut self, _thread: ThreadInfo, _context: &SetupContext) {}

        fn render_pre(&mut self, _states: &mut [()], _context: &mut BlockContext) {
            if !self.block_render {
                self.hold();
            }
        }

        fn render(&self, _state: &mut (), _context: &mut RenderContext) {
            self.render_calls.fetch_add(1, Ordering::Relaxed);
            if self.block_render {
                self.hold();
            }
        }

        fn render_post(&mut self, _states: &mut [()], _context: &mut BlockContext) {
            self.post_calls.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn a_render_arriving_while_a_block_phase_holds_the_runtime_is_refused() {
        // The other side of the same coin: `render_pre` has the
        // application and every state by &mut, so a render thread that
        // woke early must not be handed one of them.
        let mut fixture = Fixture::with_threads(2);
        let runtime = Arc::new(Runtime::new(Blocker::new(false), 2));
        assert!(unsafe { runtime.setup(&raw mut fixture.context) });

        let mut mirrors = mirrors(&fixture, 2);
        let app = || unsafe { &*runtime.app.get() };
        let inside = Arc::clone(&app().inside);
        let release = Arc::clone(&app().release);
        let rendering = Shared(&raw mut mirrors[1]);
        let block = Shared(&raw mut fixture.context);

        thread::scope(|scope| {
            let preparing = {
                let runtime = Arc::clone(&runtime);
                scope.spawn(move || {
                    let block = block;
                    unsafe { runtime.render_pre(block.0) };
                })
            };
            // Wait until render_pre is definitely inside the callback.
            inside.wait();
            unsafe { runtime.render(rendering.0) };
            release.wait();
            preparing.join().expect("the block phase should not panic");
        });

        assert_eq!(runtime.faults(), 1, "the render must have been refused");
        assert_eq!(
            app().render_calls.load(Ordering::Relaxed),
            0,
            "render must not have run"
        );
    }

    // --- The guard on its own ---

    #[test]
    fn a_refusal_while_stopping_is_counted_apart() {
        // The shutdown libbela does can leave a `render_post`
        // overlapping a `render` that has not finished. Refusing it is
        // the guard working, so it must not turn an ordinary Ctrl-C
        // into `Error::CallbackFaults`.
        let guard = Guard::new(1);

        guard.record_fault(false);
        guard.record_fault(true);
        guard.record_fault(true);

        assert_eq!(guard.faults(), 1, "only the live refusal is a fault");
        assert_eq!(
            guard.faults_while_stopping(),
            2,
            "the refusals during the shutdown are counted, and kept apart"
        );
    }

    #[test]
    fn a_render_turned_away_by_a_stopping_phase_is_counted_as_stopping() {
        // The narrow shape: a secondary thread passed its own stop
        // check before the flag was set, was slow to arrive, and finds
        // `render_post` already holding the claim. Its own reading of
        // the flag proves nothing — the claim's does.
        let guard = Guard::new(2);
        let stopping = guard
            .enter_exclusive_seeing(true)
            .expect("nothing is in flight");

        assert!(guard.enter_render(1).is_none(), "the claim is held");

        assert_eq!(guard.faults(), 0, "a shutdown is not a live fault");
        assert_eq!(guard.faults_while_stopping(), 1);
        drop(stopping);
    }

    #[test]
    fn a_render_turned_away_by_a_live_phase_is_still_a_fault() {
        // The same refusal outside a shutdown is the anomaly it looks
        // like, and must not be excused by the same path.
        let guard = Guard::new(2);
        let live = guard
            .enter_exclusive_seeing(false)
            .expect("nothing is in flight");

        assert!(guard.enter_render(1).is_none(), "the claim is held");

        assert_eq!(guard.faults(), 1);
        assert_eq!(guard.faults_while_stopping(), 0);
        drop(live);
    }

    #[test]
    fn the_same_thread_number_cannot_render_twice() {
        let guard = Guard::new(2);
        let first = guard.enter_render(1).expect("the slot is free");
        assert!(
            guard.enter_render(1).is_none(),
            "a second claim on the same slot must be refused"
        );
        assert_eq!(guard.faults(), 1);
        // The refused claim must not have left the counter raised.
        drop(first);
        assert_eq!(guard.state.load(Ordering::Acquire), 0);
    }

    #[test]
    fn different_thread_numbers_render_together() {
        let guard = Guard::new(4);
        let claims: Vec<Active<'_>> = (0..4)
            .map(|thread| guard.enter_render(thread).expect("each slot is free"))
            .collect();
        assert_eq!(guard.faults(), 0);
        drop(claims);
        assert_eq!(guard.state.load(Ordering::Acquire), 0);
    }

    #[test]
    fn an_exclusive_phase_and_a_render_never_overlap() {
        let guard = Guard::new(2);

        let rendering = guard.enter_render(0).expect("the slot is free");
        assert!(
            guard.enter_exclusive().is_none(),
            "an exclusive phase must wait for the render threads to be out"
        );
        drop(rendering);

        let exclusive = guard.enter_exclusive().expect("nothing is in flight");
        assert!(
            guard.enter_render(0).is_none(),
            "a render must not join a block an exclusive phase is in"
        );
        assert!(
            guard.enter_exclusive().is_none(),
            "two exclusive phases must not overlap"
        );
        drop(exclusive);

        assert_eq!(guard.faults(), 3);
        assert!(
            guard.enter_exclusive().is_some(),
            "and then it is free again"
        );
    }
}
