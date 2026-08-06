use core::ops::Range;

use crate::context::{BlockContext, CleanupContext, RenderContext, SetupContext, partition};

/// Which render thread a [`RenderState`](BelaApplication::RenderState)
/// is being made for.
///
/// Handed to
/// [`create_render_state`](BelaApplication::create_render_state), which
/// is called once per thread, in order, before audio starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ThreadInfo {
    index: usize,
    count: usize,
}

impl ThreadInfo {
    pub(crate) const fn new(index: usize, count: usize) -> Self {
        Self { index, count }
    }

    /// Which thread this is, in `0..count()`.
    ///
    /// The same number [`RenderContext::this_thread`] reports, so a
    /// state built here and the context `render` gets are always
    /// talking about the same thread.
    #[must_use]
    pub const fn index(&self) -> usize {
        self.index
    }

    /// How many render threads there are in total.
    #[must_use]
    pub const fn count(&self) -> usize {
        self.count
    }

    /// Whether this is the only render thread.
    ///
    /// Worth branching on where a parallel arrangement costs something
    /// a single-threaded one does not — an extra mixing bus, say.
    #[must_use]
    pub const fn is_only(&self) -> bool {
        self.count == 1
    }

    /// This thread's share of `frames`, split the way
    /// [`RenderContext`] splits a block.
    ///
    /// The same ranges [`RenderContext::audio_frame_range`] and its
    /// analog and digital counterparts hand out, worked out from a
    /// frame count rather than from a context — so a state built in
    /// [`create_render_state`](BelaApplication::create_render_state)
    /// can be sized and positioned for the frames it will be asked to
    /// write, without the caller having to reproduce the split.
    ///
    /// Pass the count from the [`SetupContext`] of the same callback:
    /// [`audio_frames`](SetupContext::audio_frames) for the audio
    /// range, and its analog or digital sibling for those. Those
    /// counts are fixed for the run — libbela has them before it calls
    /// `setup`, and the render contexts report the same numbers — so
    /// a range worked out here is the range `render` will be given.
    ///
    /// ```
    /// # use bela::{BelaApplication, RenderContext, SetupContext, ThreadInfo};
    /// # struct App;
    /// struct Bus {
    ///     first_frame: usize,
    ///     samples: Vec<f32>,
    /// }
    ///
    /// # impl BelaApplication for App {
    /// # type RenderState = Bus;
    /// fn create_render_state(&mut self, thread: ThreadInfo, context: &SetupContext) -> Bus {
    ///     let frames = thread.frame_range(context.audio_frames());
    ///     Bus {
    ///         first_frame: frames.start,
    ///         // Allocating here is fine: audio has not started.
    ///         samples: vec![0.0; frames.len()],
    ///     }
    /// }
    /// # fn render(&self, _s: &mut Bus, _c: &mut RenderContext) {}
    /// # }
    /// ```
    #[must_use]
    pub const fn frame_range(&self, frames: usize) -> Range<usize> {
        partition(frames, self.index, self.count)
    }
}

/// A Bela application: user code driven by the audio system callbacks.
///
/// The callbacks come in three groups, and the difference between them
/// is what Bela's multithreaded rendering makes of `self`:
///
/// - [`setup`](BelaApplication::setup),
///   [`create_render_state`](BelaApplication::create_render_state) and
///   [`cleanup`](BelaApplication::cleanup) run once, on their own,
///   outside the real-time context.
/// - [`render_pre`](BelaApplication::render_pre) and
///   [`render_post`](BelaApplication::render_post) run once per block
///   on the main audio thread, bracketing the parallel section. They
///   see the whole block and every render state.
/// - [`render`](BelaApplication::render) runs once per block **on every
///   render thread at the same time**, which is why it takes `&self`.
///   Each call gets one [`RenderState`](BelaApplication::RenderState),
///   exclusively, and one thread's share of the output buffers.
///
/// That shape is Bela's, not this crate's: with
/// [`Settings::thread_count`](crate::Settings::thread_count) above 1,
/// libbela calls `render` concurrently on every thread, for the same
/// block, with the same user data and the same unpartitioned buffers.
/// See `docs/multithreaded-rendering.md` for the measurements. One
/// render thread is the same model with one state and one partition
/// covering the block, so there is nothing extra to write for it.
///
/// # Where the mutable state goes
///
/// Anything `render` mutates belongs in
/// [`RenderState`](BelaApplication::RenderState): filters, phases,
/// per-thread scratch buffers, counters. The application itself holds
/// what every thread reads — coefficients, tables, handles — and is
/// shared as `&self` while rendering.
///
/// State that is genuinely one thing for the whole block, like an
/// oscillator's phase, is prepared in `render_pre` and folded back in
/// `render_post`; `examples/sine.rs` shows the pattern.
///
/// # Real-time safety
///
/// `render`, `render_pre` and `render_post` run on real-time threads.
/// They must not:
///
/// - allocate or free heap memory,
/// - block (locks, channels, sleeping) or make system calls (including
///   I/O; use [`rt_println!`](crate::rt_println) for debugging, and an
///   [`AuxiliaryTask`](crate::AuxiliaryTask) for work that has to
///   allocate or block),
/// - panic in code paths that can actually be hit — a panic crossing
///   the callback boundary aborts the whole process.
///
/// This is an operational contract rather than a memory-safety one,
/// which is why the trait is safe to implement: breaking it costs
/// dropouts, not undefined behaviour. `setup`, `create_render_state`
/// and `cleanup` run outside the real-time context and are not subject
/// to it (panics still abort the process).
///
/// # Example
///
/// ```
/// use bela::{BelaApplication, RenderContext, SetupContext, ThreadInfo};
///
/// struct Passthrough;
///
/// impl BelaApplication for Passthrough {
///     // Nothing to carry from block to block.
///     type RenderState = ();
///
///     fn create_render_state(&mut self, _thread: ThreadInfo, _context: &SetupContext) {}
///
///     fn render(&self, _state: &mut (), context: &mut RenderContext) {
///         let channels = context
///             .audio_in_channels()
///             .min(context.audio_out_channels());
///         // Only this thread's frames; the ranges tile the block.
///         for frame in context.audio_frame_range() {
///             for channel in 0..channels {
///                 let sample = context.audio_read(frame, channel);
///                 context.audio_write(frame, channel, sample);
///             }
///         }
///     }
/// }
/// ```
pub trait BelaApplication: Send + Sync {
    /// Everything `render` mutates, one per render thread.
    ///
    /// `()` for an application whose `render` carries nothing from one
    /// block to the next.
    ///
    /// [`Send`], because the states are built on the main thread and
    /// then used on the render threads. Not [`Sync`]: each one is only
    /// ever reached by the thread it belongs to, and by `render_pre` /
    /// `render_post` while no thread is rendering.
    type RenderState: Send;

    /// Called once before audio starts, before any render state is
    /// created. Return `false` to abort startup.
    ///
    /// Aborting fails [`Bela::new`](crate::Bela::new) with
    /// [`Error::Init`](crate::Error::Init), and does so after libbela
    /// has brought the audio hardware up — which, as that method
    /// documents, leaves the process unable to build another audio
    /// system: every later `Bela::new` fails with
    /// [`Error::AudioSystemPoisoned`](crate::Error::AudioSystemPoisoned).
    /// So abort to end the program, not to try again with different
    /// settings.
    fn setup(&mut self, _context: &SetupContext) -> bool {
        true
    }

    /// Called once per render thread, after
    /// [`setup`](BelaApplication::setup) has agreed to start, to build
    /// that thread's [`RenderState`](BelaApplication::RenderState).
    ///
    /// Runs on the main thread before audio starts, so it may allocate
    /// — a per-thread scratch buffer is the point of it — and it can
    /// use whatever `setup` worked out.
    ///
    /// [`ThreadInfo::count`] is the number of threads that will render,
    /// which is [`Settings::thread_count`](crate::Settings::thread_count)
    /// as libbela resolved it, so an application can size a per-thread
    /// arrangement here rather than guessing.
    fn create_render_state(
        &mut self,
        thread: ThreadInfo,
        context: &SetupContext,
    ) -> Self::RenderState;

    /// Called once per block, on the main audio thread, before the
    /// render threads are woken.
    ///
    /// Nothing else is running: this is where the whole block and every
    /// render state can be touched at once — reading the inputs,
    /// clearing the outputs, handing each thread the state its share of
    /// the block starts from.
    ///
    /// `states` is in thread order and `states.len()` is
    /// [`BlockContext::thread_count`], so `states[n]` is the state
    /// thread `n` will render with, and
    /// [`ThreadInfo::frame_range`] says which frames that is. A
    /// callback whose context disagrees with the states is refused
    /// before it reaches here.
    fn render_pre(&mut self, _states: &mut [Self::RenderState], _context: &mut BlockContext) {}

    /// Called once per block **on every render thread at the same
    /// time**, each with its own state and its own share of the output.
    ///
    /// `state` is the [`RenderState`](BelaApplication::RenderState) of
    /// [`RenderContext::this_thread`], held exclusively for the call.
    /// `context` reads the whole block and writes only
    /// [`RenderContext::audio_frame_range`] and its analog and digital
    /// counterparts.
    fn render(&self, state: &mut Self::RenderState, context: &mut RenderContext);

    /// Called once per block, on the main audio thread, after the last
    /// render thread has finished.
    ///
    /// The place to reduce: mix the per-thread busses down, advance the
    /// state the block as a whole carries, publish a reading for an
    /// [`AuxiliaryTask`](crate::AuxiliaryTask) to report.
    ///
    /// `states` is in thread order, as it is in
    /// [`render_pre`](BelaApplication::render_pre).
    fn render_post(&mut self, _states: &mut [Self::RenderState], _context: &mut BlockContext) {}

    /// Called once after audio rendering stops, with the render states
    /// still intact.
    fn cleanup(&mut self, _states: &mut [Self::RenderState], _context: &CleanupContext) {}
}
