use core::ops::Range;

use crate::context::{BlockContext, CleanupContext, RenderContext, SetupContext, partition};
use crate::settings::ResolvedSettings;

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
/// - [`validate_settings`](BelaApplication::validate_settings) runs
///   before there is an audio system at all, on the settings one is
///   about to be built with. It is the only place an application can
///   decline a configuration without costing the process the ability
///   to build another one.
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

    /// Called once with the settings the audio system is about to be
    /// built with, before anything has been done about them. Return
    /// `Err` to refuse the configuration.
    ///
    /// This is where an application says what it needs of the
    /// configuration it is given — two render threads, six analog
    /// inputs, a sample rate it has coefficients for. It runs after
    /// `Bela_defaultSettings()`, [`Settings`](crate::Settings) and the
    /// command line have all been applied, and after this crate's own
    /// checks on the result, but before the CPU monitoring counters are
    /// touched, before any render state is allocated and before
    /// `Bela_initAudio` is called. So a refusal here is an ordinary
    /// error out of [`Bela::new`](crate::Bela::new) —
    /// [`Error::SettingsRefused`](crate::Error::SettingsRefused),
    /// carrying the reason given — and the process is left exactly as
    /// it was: another audio system can be built straight away, with
    /// settings the application does accept.
    ///
    /// That is the whole difference from refusing in
    /// [`setup`](BelaApplication::setup), which runs inside
    /// `Bela_initAudio` with the hardware already up and costs the
    /// process every later audio system.
    ///
    /// The default accepts everything.
    ///
    /// ```
    /// # use bela::{BelaApplication, RenderContext, ResolvedSettings, SetupContext, ThreadInfo};
    /// # struct Stereo;
    /// # impl BelaApplication for Stereo {
    /// # type RenderState = ();
    /// fn validate_settings(&self, settings: &ResolvedSettings) -> Result<(), &'static str> {
    ///     if settings.thread_count() != 1 {
    ///         return Err("this application renders on one thread");
    ///     }
    ///     if settings.num_analog_in_channels() < 6 {
    ///         return Err("this application reads six analog inputs");
    ///     }
    ///     Ok(())
    /// }
    /// # fn create_render_state(&mut self, _t: ThreadInfo, _c: &SetupContext) {}
    /// # fn render(&self, _s: &mut (), _c: &mut RenderContext) {}
    /// # }
    /// ```
    ///
    /// # It sees the request, not the hardware
    ///
    /// [`ResolvedSettings`] is what will be asked of libbela.
    /// What the board makes of it is
    /// [`SetupContext`](crate::SetupContext), which does not exist
    /// until `Bela_initAudio` has run — so a check here cannot promise
    /// what the codec will deliver, only that nobody asked for
    /// something else. The type documents where the two part company.
    ///
    /// # Errors
    /// Whatever the application will not run under, as a `&'static
    /// str` that finishes the sentence "the application refused the
    /// resolved settings: ". A static message keeps
    /// [`Error`](crate::Error) allocation-free and `Copy`, which is
    /// what lets it be returned from a real-time-adjacent API without
    /// a heap behind it.
    fn validate_settings(&self, _settings: &ResolvedSettings) -> Result<(), &'static str> {
        Ok(())
    }

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
    ///
    /// A configuration the application will not run under is not a
    /// reason to abort from here:
    /// [`validate_settings`](BelaApplication::validate_settings) asks
    /// the same question before the hardware is up, where the answer
    /// costs an error rather than the process. What is left for this
    /// callback is what only exists once libbela has answered —
    /// the channel counts and frame counts the
    /// [`SetupContext`] reports, a MIDI port that could not be opened
    /// — and even there, aborting ends the program.
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
