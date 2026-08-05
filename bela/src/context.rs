//! The views a [`BelaApplication`](crate::BelaApplication) sees of the
//! `BelaContext` libbela hands to each callback.
//!
//! There is one type per phase rather than one type for all of them,
//! because the phases do not have the same rights over the block:
//!
//! - [`SetupContext`] and [`CleanupContext`] describe the audio
//!   configuration. They run when there are no buffers to speak of, so
//!   they expose none.
//! - [`BlockContext`] is the whole block, and belongs to the
//!   single-threaded [`render_pre`](crate::BelaApplication::render_pre)
//!   and [`render_post`](crate::BelaApplication::render_post) hooks.
//! - [`RenderContext`] is what [`render`](crate::BelaApplication::render)
//!   gets, on every thread at once. Its inputs are the whole block, its
//!   outputs are only this thread's share of it.
//!
//! The split is the reason more than one render thread can be used at
//! all: Bela hands every thread the same buffers and partitions
//! nothing, so a view that handed each of them `&mut [f32]` over all
//! the outputs would be the aliasing the design exists to avoid. See
//! `docs/multithreaded-rendering.md`.

use core::ops::Range;
use core::slice;

use bela_sys::BelaContext;

/// Direction of a digital (GPIO) pin. All pins begin as inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinMode {
    /// The pin reads external logic levels (the default).
    Input,
    /// The pin drives the value set with the `digital_write` family.
    Output,
}

// Within a digital frame word, bits 0-15 hold pin directions
// (1 = input) and bits 16-31 hold pin values.
const DIGITAL_VALUE_SHIFT: usize = 16;

/// The `thread`th of `count` contiguous parts of `len` frames.
///
/// The parts tile `0..len` exactly — each one starts where the last
/// ended, the first starts at 0 and the last ends at `len` — so the
/// threads between them cover the block once, whatever the remainder
/// of the division is. Parts can be empty, which is what more threads
/// than frames means.
///
/// A `count` of 0 is read as 1, the way [`thread_count`] does: it is
/// how a `BelaContext` can spell "one render thread".
///
/// [`thread_count`]: RenderContext::thread_count
#[allow(
    clippy::redundant_pub_crate,
    reason = "ThreadInfo::frame_range is the public spelling of it, and lives in another module"
)]
#[inline]
pub(crate) const fn partition(len: usize, thread: usize, count: usize) -> Range<usize> {
    let count = if count == 0 { 1 } else { count };
    if thread >= count {
        // Not a share of anything; the caller is out of range.
        return len..len;
    }
    // `len` is a frame count, so neither product can overflow.
    let start = len * thread / count;
    let end = len * (thread + 1) / count;
    start..end
}

/// Generates the accessors every phase has: the audio configuration,
/// and where in the run this block is.
macro_rules! metadata_accessors {
    ($($context:ty),+ $(,)?) => {
        $(
            impl $context {
                /// Read access to the underlying `BelaContext`.
                #[must_use]
                #[inline]
                pub const fn as_sys(&self) -> &BelaContext {
                    &self.0
                }

                /// Number of audio frames per block.
                #[must_use]
                #[inline]
                pub const fn audio_frames(&self) -> usize {
                    self.0.audioFrames as usize
                }

                /// Number of audio input channels.
                #[must_use]
                #[inline]
                pub const fn audio_in_channels(&self) -> usize {
                    self.0.audioInChannels as usize
                }

                /// Number of audio output channels.
                ///
                /// On a Bela Gem Stereo this is 2, and the analog
                /// outputs are not among them — the board has none.
                /// See the type-level documentation.
                #[must_use]
                #[inline]
                pub const fn audio_out_channels(&self) -> usize {
                    self.0.audioOutChannels as usize
                }

                /// Audio sample rate in Hz.
                #[must_use]
                #[inline]
                pub const fn audio_sample_rate(&self) -> f32 {
                    self.0.audioSampleRate
                }

                /// Number of analog frames per block; 0 if analog I/O
                /// is disabled.
                #[must_use]
                #[inline]
                pub const fn analog_frames(&self) -> usize {
                    self.0.analogFrames as usize
                }

                /// Number of analog input channels; 0 if analog I/O is
                /// disabled.
                #[must_use]
                #[inline]
                pub const fn analog_in_channels(&self) -> usize {
                    self.0.analogInChannels as usize
                }

                /// Number of analog output channels; 0 if analog I/O is
                /// disabled.
                #[must_use]
                #[inline]
                pub const fn analog_out_channels(&self) -> usize {
                    self.0.analogOutChannels as usize
                }

                /// Analog sample rate in Hz; 0 if analog I/O is
                /// disabled.
                #[must_use]
                #[inline]
                pub const fn analog_sample_rate(&self) -> f32 {
                    self.0.analogSampleRate
                }

                /// Number of digital frames per block; 0 if digital I/O
                /// is disabled.
                #[must_use]
                #[inline]
                pub const fn digital_frames(&self) -> usize {
                    self.0.digitalFrames as usize
                }

                /// Number of digital (GPIO) channels; 0 if digital I/O
                /// is disabled.
                #[must_use]
                #[inline]
                pub const fn digital_channels(&self) -> usize {
                    self.0.digitalChannels as usize
                }

                /// Digital sample rate in Hz.
                #[must_use]
                #[inline]
                pub const fn digital_sample_rate(&self) -> f32 {
                    self.0.digitalSampleRate
                }

                /// Total audio frames elapsed as of the beginning of
                /// this block.
                #[must_use]
                #[inline]
                pub const fn audio_frames_elapsed(&self) -> u64 {
                    self.0.audioFramesElapsed
                }

                /// Number of detected underruns.
                #[must_use]
                #[inline]
                pub const fn underrun_count(&self) -> u32 {
                    self.0.underrunCount
                }

                /// Which render thread this context belongs to, in
                /// `0..thread_count()`.
                ///
                /// Always 0 outside
                /// [`render`](crate::BelaApplication::render): every
                /// other callback is made once, on the main audio
                /// thread.
                #[must_use]
                #[inline]
                pub const fn this_thread(&self) -> usize {
                    self.0.thisThread as usize
                }

                /// How many threads [`render`] is called on for each
                /// block, which is how many
                /// [`RenderState`](crate::BelaApplication::RenderState)s
                /// there are.
                ///
                /// At least 1. A `BelaContext` can spell one render
                /// thread as either 1 or 0 — libbela copies
                /// [`Settings::thread_count`](crate::Settings::thread_count)
                /// through unchanged and only creates *extra* threads
                /// above 1 — and this reports the number of threads
                /// that actually render, so both come back as 1.
                ///
                /// [`render`]: crate::BelaApplication::render
                #[must_use]
                #[inline]
                pub const fn thread_count(&self) -> usize {
                    let count = self.0.threadCount as usize;
                    if count == 0 { 1 } else { count }
                }
            }
        )+
    };
}

/// What [`setup`](crate::BelaApplication::setup) and
/// [`create_render_state`](crate::BelaApplication::create_render_state)
/// see: the audio configuration, before any audio has been rendered.
///
/// The buffers are not part of it. `setup` runs inside
/// `Bela_initAudio`, before the audio thread exists, so there is no
/// block to read or write — and reading the sample rate, the block size
/// and the channel counts is what a `setup` is for.
///
/// [`cpu_usage`](SetupContext::cpu_usage) is here too, so that a
/// program can find out in `setup` whether
/// [`Settings::cpu_monitoring`](crate::Settings::cpu_monitoring)
/// reached the hardware.
#[repr(transparent)]
pub struct SetupContext(BelaContext);

/// What [`cleanup`](crate::BelaApplication::cleanup) sees: the same
/// audio configuration [`SetupContext`] described, after the audio
/// thread has been joined.
///
/// No buffers, for the same reason: there is no block in flight. The
/// counters that describe the run as a whole —
/// [`audio_frames_elapsed`](CleanupContext::audio_frames_elapsed),
/// [`underrun_count`](CleanupContext::underrun_count) and
/// [`cpu_usage`](CleanupContext::cpu_usage) — are, which is what makes
/// `cleanup` the place for a closing report.
#[repr(transparent)]
pub struct CleanupContext(BelaContext);

/// What [`render_pre`](crate::BelaApplication::render_pre) and
/// [`render_post`](crate::BelaApplication::render_post) see: the whole
/// block, with nothing else running.
///
/// These two hooks bracket the parallel section — libbela calls
/// `render_pre` before it wakes the render threads and `render_post`
/// after the last of them has finished — so they are the one place a
/// multithreaded application may touch the whole block: preparing the
/// per-thread state and the buffers on the way in, mixing down what
/// the threads produced on the way out.
///
/// The accessors mirror the C helpers from `Bela.h` / `Utilities.h`
/// and assume the default interleaved buffer layout: sample index =
/// `frame * channels + channel`.
///
/// # Bela Gem semantics
///
/// On a Bela Gem Stereo there is nothing to write an analog output
/// with: [`analog_out_channels`](BlockContext::analog_out_channels) is
/// 0 however many the settings ask for, and
/// [`audio_out_channels`](BlockContext::audio_out_channels) is 2 —
/// the analog outputs are not folded in there either. So
/// [`analog_write`](BlockContext::analog_write) has no channel it can
/// take on that board, while
/// [`analog_read`](BlockContext::analog_read) has eight. Bela's
/// migration guide describes writing an analog output as
/// [`audio_write`](BlockContext::audio_write) with the channel offset
/// by +2; that is a Gem Multi, which has the outputs, and it has not
/// been measured here.
///
/// [`uniform_sample_rate`](crate::Settings::uniform_sample_rate) is on
/// by default, so analog frames == audio frames unless it is turned
/// off. Output values do not persist across blocks; the within-block
/// persistence of [`analog_write`](BlockContext::analog_write) and
/// [`digital_write`](BlockContext::digital_write) (writing from `frame`
/// to the end of the block) is unchanged.
///
/// Measured on the board; see `docs/board-facts.md`.
///
/// # Panics
///
/// The indexed accessors panic when `frame` or `channel` is out of
/// range (the C equivalents would read or write out of bounds). On the
/// device a panic aborts the whole process, so treat these as
/// programming errors, not recoverable conditions.
#[repr(transparent)]
pub struct BlockContext(BelaContext);

/// What [`render`](crate::BelaApplication::render) sees: the whole
/// block to read, this thread's share of it to write.
///
/// Bela calls `render` on every render thread at once, for the same
/// block, over the same buffers, and partitions nothing itself. This
/// type is where the partitioning happens: the reading accessors cover
/// the block, because inputs are shared and nobody writes them, while
/// every writing accessor is confined to
/// [`audio_frame_range`](RenderContext::audio_frame_range) and its
/// analog and digital counterparts — contiguous ranges of frames that
/// tile the block exactly across the threads.
///
/// With one render thread the range is the whole block and this
/// behaves like [`BlockContext`] minus the whole-buffer writes.
///
/// # Writing a loop
///
/// The indexed writers work the partition out on every call, since
/// that is what bounds the frame they were given. A loop over frames
/// is cheaper through the slice accessors —
/// [`audio_out`](RenderContext::audio_out) and its siblings — which
/// work it out once:
///
/// ```ignore
/// let channels = context.audio_out_channels();
/// for samples in context.audio_out().chunks_mut(channels) {
///     samples.fill(value);
/// }
/// ```
///
/// # What is not here
///
/// - `as_sys_mut`: the raw context is the way back to the whole output
///   buffer, which is exactly what must not be reachable from several
///   threads at once.
/// - `cpu_usage`: libbela's counters are written by the main audio
///   thread without synchronisation, and a secondary render thread
///   reading them would be a data race. Read it in
///   [`render_pre`](crate::BelaApplication::render_pre) or
///   [`render_post`](crate::BelaApplication::render_post), which run on
///   that thread, and hand the number on.
///
/// # Work that does not divide by frame
///
/// Contiguous frames are the partition this type can hand out safely,
/// and a filter or an oscillator does not survive being cut into
/// pieces that different threads carry across blocks. Keep such state
/// in [`RenderState`](crate::BelaApplication::RenderState), one per
/// thread, and use `render_pre` to line the pieces up before the block
/// and `render_post` to mix them afterwards; `examples/sine.rs` does
/// exactly that with a phase.
///
/// # Panics
///
/// Reading accessors panic when `frame` or `channel` is out of range,
/// like [`BlockContext`]'s. Writing accessors also panic when `frame`
/// is outside this thread's range, since writing there would race
/// whichever thread owns it.
#[repr(transparent)]
pub struct RenderContext(BelaContext);

metadata_accessors!(SetupContext, CleanupContext, BlockContext, RenderContext);

/// Generates the `from_mut_ptr` constructor each context needs, since
/// the safety contract is the same for all of them.
macro_rules! from_mut_ptr {
    ($($context:ident: $phase:literal),+ $(,)?) => {
        $(
            impl $context {
                #[doc = concat!(
                    "Reborrows a raw `BelaContext` pointer as a [`",
                    stringify!($context),
                    "`].\n\n# Safety\n\n`ptr` must be non-null, properly aligned, and point to a \
                     live `BelaContext` that is not accessed through any other reference for the \
                     duration of `'a`. The buffer pointers inside must be either null or valid \
                     for the lengths implied by the frame and channel counts.\n\nThe result \
                     stands in for the context of the ", $phase, " callback, and some accessors \
                     take it as proof of being in one — see the type documentation for what they \
                     rely on. A context conjured up elsewhere is not that proof."
                )]
                pub const unsafe fn from_mut_ptr<'a>(ptr: *mut BelaContext) -> &'a mut Self {
                    // repr(transparent) makes the cast sound.
                    unsafe { &mut *ptr.cast::<Self>() }
                }
            }
        )+
    };
}

from_mut_ptr!(
    SetupContext: "setup",
    CleanupContext: "cleanup",
    BlockContext: "render_pre / render_post",
    RenderContext: "render",
);

impl BlockContext {
    /// Mutable access to the underlying `BelaContext`.
    ///
    /// # Safety
    ///
    /// The caller must not invalidate data the audio system or the
    /// safe accessors rely on, e.g. by overwriting buffer pointers,
    /// frame counts or channel counts. Writing *through* the output
    /// buffer pointers is fine.
    pub const unsafe fn as_sys_mut(&mut self) -> &mut BelaContext {
        &mut self.0
    }

    // --- Whole-buffer access (interleaved) ---

    /// Audio input samples; empty with audio disabled. Length is
    /// `audio_frames() * audio_in_channels()`.
    #[must_use]
    #[inline]
    pub const fn audio_in(&self) -> &[f32] {
        unsafe {
            shared(
                self.0.audioIn,
                self.audio_frames() * self.audio_in_channels(),
            )
        }
    }

    /// Audio output samples. Length is
    /// `audio_frames() * audio_out_channels()`.
    #[inline]
    pub const fn audio_out(&mut self) -> &mut [f32] {
        unsafe {
            exclusive(
                self.0.audioOut,
                self.audio_frames() * self.audio_out_channels(),
            )
        }
    }

    /// Analog input samples; empty if analog I/O is disabled. Length
    /// is `analog_frames() * analog_in_channels()`.
    #[must_use]
    #[inline]
    pub const fn analog_in(&self) -> &[f32] {
        unsafe {
            shared(
                self.0.analogIn,
                self.analog_frames() * self.analog_in_channels(),
            )
        }
    }

    /// Analog output samples; empty if analog I/O is disabled. Length
    /// is `analog_frames() * analog_out_channels()`.
    #[inline]
    pub const fn analog_out(&mut self) -> &mut [f32] {
        unsafe {
            exclusive(
                self.0.analogOut,
                self.analog_frames() * self.analog_out_channels(),
            )
        }
    }

    /// Digital I/O words, one per digital frame. Prefer the
    /// `digital_*` / `pin_mode*` accessors, which encapsulate the bit
    /// layout.
    #[must_use]
    #[inline]
    pub const fn digital(&self) -> &[u32] {
        unsafe { shared(self.0.digital, self.digital_frames()) }
    }

    /// Mutable access to the digital I/O words. Prefer the
    /// `digital_write*` / `pin_mode*` accessors, which encapsulate the
    /// bit layout.
    #[inline]
    pub const fn digital_mut(&mut self) -> &mut [u32] {
        unsafe { exclusive(self.0.digital, self.digital_frames()) }
    }

    // --- Indexed access (mirrors the C helpers) ---

    /// Audio input sample at `frame` for `channel` (`audioRead`).
    ///
    /// # Panics
    /// If `frame` or `channel` is out of range.
    #[must_use]
    #[inline]
    pub fn audio_read(&self, frame: usize, channel: usize) -> f32 {
        let channels = self.audio_in_channels();
        assert!(channel < channels, "audio input channel out of range");
        self.audio_in()[frame * channels + channel]
    }

    /// Sets the audio output at `frame` for `channel` (`audioWrite`).
    /// Audio outputs never persist.
    ///
    /// # Panics
    /// If `frame` or `channel` is out of range.
    #[inline]
    pub fn audio_write(&mut self, frame: usize, channel: usize, value: f32) {
        let channels = self.audio_out_channels();
        assert!(channel < channels, "audio output channel out of range");
        self.audio_out()[frame * channels + channel] = value;
    }

    /// Analog input sample at `frame` for `channel` (`analogRead`).
    ///
    /// # Panics
    /// If `frame` or `channel` is out of range.
    #[must_use]
    #[inline]
    pub fn analog_read(&self, frame: usize, channel: usize) -> f32 {
        let channels = self.analog_in_channels();
        assert!(channel < channels, "analog input channel out of range");
        self.analog_in()[frame * channels + channel]
    }

    /// Sets the analog output for `channel` from `frame` to the end of
    /// the block (`analogWrite`). A Bela Gem Stereo has no analog
    /// outputs, so every channel is out of range there — see the
    /// type-level documentation.
    ///
    /// # Panics
    /// If `channel` is out of range.
    pub fn analog_write(&mut self, frame: usize, channel: usize, value: f32) {
        let channels = self.analog_out_channels();
        assert!(channel < channels, "analog output channel out of range");
        let frames = self.analog_frames();
        let out = self.analog_out();
        for f in frame..frames {
            out[f * channels + channel] = value;
        }
    }

    /// Sets the analog output at `frame` only (`analogWriteOnce`).
    ///
    /// # Panics
    /// If `frame` or `channel` is out of range.
    #[inline]
    pub fn analog_write_once(&mut self, frame: usize, channel: usize, value: f32) {
        let channels = self.analog_out_channels();
        assert!(channel < channels, "analog output channel out of range");
        self.analog_out()[frame * channels + channel] = value;
    }

    /// Value of the digital `channel` at `frame` (`digitalRead`).
    ///
    /// # Panics
    /// If `frame` or `channel` is out of range.
    #[must_use]
    #[inline]
    pub fn digital_read(&self, frame: usize, channel: usize) -> bool {
        let mask = digital_value_mask(self.digital_channels(), channel);
        self.digital()[frame] & mask != 0
    }

    /// Sets the digital output `channel` from `frame` to the end of
    /// the block (`digitalWrite`).
    ///
    /// # Panics
    /// If `channel` is out of range.
    pub fn digital_write(&mut self, frame: usize, channel: usize, value: bool) {
        let mask = digital_value_mask(self.digital_channels(), channel);
        for word in self.digital_mut().iter_mut().skip(frame) {
            set_bits(word, mask, value);
        }
    }

    /// Sets the digital output `channel` at `frame` only
    /// (`digitalWriteOnce`).
    ///
    /// # Panics
    /// If `frame` or `channel` is out of range.
    #[inline]
    pub fn digital_write_once(&mut self, frame: usize, channel: usize, value: bool) {
        let mask = digital_value_mask(self.digital_channels(), channel);
        set_bits(&mut self.digital_mut()[frame], mask, value);
    }

    /// Sets the direction of digital `channel` from `frame` to the end
    /// of the block (`pinMode`).
    ///
    /// # Panics
    /// If `channel` is out of range.
    pub fn pin_mode(&mut self, frame: usize, channel: usize, mode: PinMode) {
        let mask = digital_direction_mask(self.digital_channels(), channel);
        for word in self.digital_mut().iter_mut().skip(frame) {
            set_bits(word, mask, mode == PinMode::Input);
        }
    }

    /// Sets the direction of digital `channel` at `frame` only
    /// (`pinModeOnce`).
    ///
    /// # Panics
    /// If `frame` or `channel` is out of range.
    pub fn pin_mode_once(&mut self, frame: usize, channel: usize, mode: PinMode) {
        let mask = digital_direction_mask(self.digital_channels(), channel);
        set_bits(&mut self.digital_mut()[frame], mask, mode == PinMode::Input);
    }
}

impl RenderContext {
    // --- This thread's share of the block ---

    /// The audio frames this thread is responsible for writing.
    ///
    /// Contiguous, and disjoint from what every other thread gets: the
    /// ranges tile `0..audio_frames()` exactly. Empty when there are
    /// more threads than frames.
    #[must_use]
    #[inline]
    pub const fn audio_frame_range(&self) -> Range<usize> {
        partition(self.audio_frames(), self.this_thread(), self.thread_count())
    }

    /// The analog frames this thread is responsible for writing.
    ///
    /// Split the same way as
    /// [`audio_frame_range`](RenderContext::audio_frame_range), so the
    /// two cover the same stretch of the block even when the analog
    /// frame count differs from the audio one.
    #[must_use]
    #[inline]
    pub const fn analog_frame_range(&self) -> Range<usize> {
        partition(
            self.analog_frames(),
            self.this_thread(),
            self.thread_count(),
        )
    }

    /// The digital frames this thread is responsible for writing.
    ///
    /// Split like [`audio_frame_range`](RenderContext::audio_frame_range).
    #[must_use]
    #[inline]
    pub const fn digital_frame_range(&self) -> Range<usize> {
        partition(
            self.digital_frames(),
            self.this_thread(),
            self.thread_count(),
        )
    }

    // --- Whole-block reads (interleaved) ---

    /// Audio input samples for the whole block; empty with audio
    /// disabled. Length is `audio_frames() * audio_in_channels()`.
    ///
    /// The whole block, not this thread's share: inputs are read-only
    /// and shared, and a filter needs to look back past the start of
    /// its own range.
    #[must_use]
    pub const fn audio_in(&self) -> &[f32] {
        unsafe {
            shared(
                self.0.audioIn,
                self.audio_frames() * self.audio_in_channels(),
            )
        }
    }

    /// Analog input samples for the whole block; empty if analog I/O
    /// is disabled. Length is `analog_frames() * analog_in_channels()`.
    #[must_use]
    pub const fn analog_in(&self) -> &[f32] {
        unsafe {
            shared(
                self.0.analogIn,
                self.analog_frames() * self.analog_in_channels(),
            )
        }
    }

    /// This thread's digital I/O words, one per digital frame in
    /// [`digital_frame_range`](RenderContext::digital_frame_range).
    ///
    /// This thread's share, unlike the audio and analog inputs: the
    /// digital words are the outputs too, so a word outside this range
    /// is one another thread may be writing at this very moment. Read
    /// the whole block's digital state in
    /// [`render_pre`](crate::BelaApplication::render_pre) instead,
    /// where nothing else is running.
    #[must_use]
    #[inline]
    pub const fn digital(&self) -> &[u32] {
        let range = self.digital_frame_range();
        // Safety: the buffer is valid for `digital_frames()` words, and
        // the range is within that; the guard in the runtime is what
        // makes it this thread's alone.
        unsafe { share(self.0.digital, self.digital_frames(), 1, range) }
    }

    // --- This thread's share of the outputs ---

    /// This thread's audio output samples, interleaved.
    ///
    /// The samples of the frames in
    /// [`audio_frame_range`](RenderContext::audio_frame_range), so
    /// index 0 is channel 0 of frame `audio_frame_range().start`, not
    /// of frame 0. [`audio_write`](RenderContext::audio_write) indexes
    /// by block frame instead, if that is easier to keep straight.
    ///
    /// This is the accessor to reach for in a loop over frames: it
    /// works the partition out once, where the indexed writers work it
    /// out on every call.
    #[inline]
    pub const fn audio_out(&mut self) -> &mut [f32] {
        let range = self.audio_frame_range();
        self.audio_share(range)
    }

    /// This thread's analog output samples, interleaved.
    ///
    /// The samples of the frames in
    /// [`analog_frame_range`](RenderContext::analog_frame_range); see
    /// [`audio_out`](RenderContext::audio_out) for what index 0 means
    /// and for why a loop should hold on to the slice.
    #[inline]
    pub const fn analog_out(&mut self) -> &mut [f32] {
        let range = self.analog_frame_range();
        self.analog_share(range)
    }

    /// This thread's digital I/O words, one per digital frame in
    /// [`digital_frame_range`](RenderContext::digital_frame_range).
    #[inline]
    pub const fn digital_mut(&mut self) -> &mut [u32] {
        let range = self.digital_frame_range();
        self.digital_share(range)
    }

    // --- Indexed access (mirrors the C helpers) ---

    /// Audio input sample at `frame` for `channel` (`audioRead`).
    ///
    /// `frame` is a block frame, and may be outside this thread's
    /// range: the audio inputs are a buffer of their own that nobody
    /// writes.
    ///
    /// # Panics
    /// If `frame` or `channel` is out of range.
    #[must_use]
    pub fn audio_read(&self, frame: usize, channel: usize) -> f32 {
        let channels = self.audio_in_channels();
        assert!(channel < channels, "audio input channel out of range");
        self.audio_in()[frame * channels + channel]
    }

    /// Sets the audio output at `frame` for `channel` (`audioWrite`).
    /// Audio outputs never persist.
    ///
    /// # Panics
    /// If `channel` is out of range, or `frame` is outside
    /// [`audio_frame_range`](RenderContext::audio_frame_range).
    #[inline]
    pub fn audio_write(&mut self, frame: usize, channel: usize, value: f32) {
        let channels = self.audio_out_channels();
        assert!(channel < channels, "audio output channel out of range");
        let range = self.audio_frame_range();
        let index = frame_offset(&range, frame, "audio") * channels + channel;
        self.audio_share(range)[index] = value;
    }

    /// Analog input sample at `frame` for `channel` (`analogRead`).
    ///
    /// `frame` is a block frame, and may be outside this thread's
    /// range: the analog inputs are a buffer of their own that nobody
    /// writes.
    ///
    /// # Panics
    /// If `frame` or `channel` is out of range.
    #[must_use]
    pub fn analog_read(&self, frame: usize, channel: usize) -> f32 {
        let channels = self.analog_in_channels();
        assert!(channel < channels, "analog input channel out of range");
        self.analog_in()[frame * channels + channel]
    }

    /// Sets the analog output for `channel` from `frame` to the end of
    /// **this thread's range** (`analogWrite`).
    ///
    /// The C helper writes to the end of the block; the frames past
    /// this thread's range belong to another thread, so this stops
    /// there. With one render thread the two are the same thing. To
    /// hold an analog output across the whole block whatever the
    /// thread count, write it from
    /// [`render_pre`](crate::BelaApplication::render_pre).
    ///
    /// # Panics
    /// If `channel` is out of range, or `frame` is outside
    /// [`analog_frame_range`](RenderContext::analog_frame_range).
    pub fn analog_write(&mut self, frame: usize, channel: usize, value: f32) {
        let channels = self.analog_out_channels();
        assert!(channel < channels, "analog output channel out of range");
        let range = self.analog_frame_range();
        let skip = frame_offset(&range, frame, "analog");
        for samples in self.analog_share(range).chunks_mut(channels).skip(skip) {
            samples[channel] = value;
        }
    }

    /// Sets the analog output at `frame` only (`analogWriteOnce`).
    ///
    /// # Panics
    /// If `channel` is out of range, or `frame` is outside
    /// [`analog_frame_range`](RenderContext::analog_frame_range).
    #[inline]
    pub fn analog_write_once(&mut self, frame: usize, channel: usize, value: f32) {
        let channels = self.analog_out_channels();
        assert!(channel < channels, "analog output channel out of range");
        let range = self.analog_frame_range();
        let index = frame_offset(&range, frame, "analog") * channels + channel;
        self.analog_share(range)[index] = value;
    }

    /// Value of the digital `channel` at `frame` (`digitalRead`).
    ///
    /// # Panics
    /// If `channel` is out of range, or `frame` is outside
    /// [`digital_frame_range`](RenderContext::digital_frame_range) —
    /// see [`digital`](RenderContext::digital) for why reading is
    /// bounded here and not for the audio and analog inputs.
    #[must_use]
    #[inline]
    pub fn digital_read(&self, frame: usize, channel: usize) -> bool {
        let mask = digital_value_mask(self.digital_channels(), channel);
        let range = self.digital_frame_range();
        let index = frame_offset(&range, frame, "digital");
        // Safety: as for `digital`, whose range this is.
        let words = unsafe { share(self.0.digital, self.digital_frames(), 1, range) };
        words[index] & mask != 0
    }

    /// Sets the digital output `channel` from `frame` to the end of
    /// **this thread's range** (`digitalWrite`).
    ///
    /// Stops at the end of the range for the same reason
    /// [`analog_write`](RenderContext::analog_write) does.
    ///
    /// # Panics
    /// If `channel` is out of range, or `frame` is outside
    /// [`digital_frame_range`](RenderContext::digital_frame_range).
    pub fn digital_write(&mut self, frame: usize, channel: usize, value: bool) {
        let mask = digital_value_mask(self.digital_channels(), channel);
        let range = self.digital_frame_range();
        let skip = frame_offset(&range, frame, "digital");
        for word in self.digital_share(range).iter_mut().skip(skip) {
            set_bits(word, mask, value);
        }
    }

    /// Sets the digital output `channel` at `frame` only
    /// (`digitalWriteOnce`).
    ///
    /// # Panics
    /// If `channel` is out of range, or `frame` is outside
    /// [`digital_frame_range`](RenderContext::digital_frame_range).
    #[inline]
    pub fn digital_write_once(&mut self, frame: usize, channel: usize, value: bool) {
        let mask = digital_value_mask(self.digital_channels(), channel);
        let range = self.digital_frame_range();
        let index = frame_offset(&range, frame, "digital");
        set_bits(&mut self.digital_share(range)[index], mask, value);
    }

    /// Sets the direction of digital `channel` from `frame` to the end
    /// of **this thread's range** (`pinMode`).
    ///
    /// # Panics
    /// If `channel` is out of range, or `frame` is outside
    /// [`digital_frame_range`](RenderContext::digital_frame_range).
    pub fn pin_mode(&mut self, frame: usize, channel: usize, mode: PinMode) {
        let mask = digital_direction_mask(self.digital_channels(), channel);
        let range = self.digital_frame_range();
        let skip = frame_offset(&range, frame, "digital");
        for word in self.digital_share(range).iter_mut().skip(skip) {
            set_bits(word, mask, mode == PinMode::Input);
        }
    }

    /// Sets the direction of digital `channel` at `frame` only
    /// (`pinModeOnce`).
    ///
    /// # Panics
    /// If `channel` is out of range, or `frame` is outside
    /// [`digital_frame_range`](RenderContext::digital_frame_range).
    pub fn pin_mode_once(&mut self, frame: usize, channel: usize, mode: PinMode) {
        let mask = digital_direction_mask(self.digital_channels(), channel);
        let range = self.digital_frame_range();
        let index = frame_offset(&range, frame, "digital");
        set_bits(
            &mut self.digital_share(range)[index],
            mask,
            mode == PinMode::Input,
        );
    }

    // --- This thread's share, for a range already worked out ---
    //
    // `partition` is two multiplications and two divisions, and the
    // indexed accessors need the range before they can bound-check the
    // frame. Taking it as an argument is what keeps them to one.

    /// Safety: the reference covers this thread's frames and no
    /// others, which is what keeps the concurrent calls apart.
    #[inline]
    const fn audio_share(&mut self, range: Range<usize>) -> &mut [f32] {
        unsafe {
            share_mut(
                self.0.audioOut,
                self.audio_frames(),
                self.audio_out_channels(),
                range,
            )
        }
    }

    /// Safety: as for [`audio_share`](RenderContext::audio_share).
    #[inline]
    const fn analog_share(&mut self, range: Range<usize>) -> &mut [f32] {
        unsafe {
            share_mut(
                self.0.analogOut,
                self.analog_frames(),
                self.analog_out_channels(),
                range,
            )
        }
    }

    /// Safety: as for [`audio_share`](RenderContext::audio_share).
    #[inline]
    const fn digital_share(&mut self, range: Range<usize>) -> &mut [u32] {
        unsafe { share_mut(self.0.digital, self.digital_frames(), 1, range) }
    }
}

/// Proof that the caller is inside one of the Bela callbacks.
///
/// Some operations are only sound there — scheduling an
/// [`AuxiliaryTask`](crate::AuxiliaryTask) is the one this exists for —
/// and taking a `&impl CallbackContext` is how they say so. The four
/// context types implement it and nothing else can: they are handed out
/// by the callbacks and cannot be built without `unsafe`.
pub trait CallbackContext: sealed::Sealed {}

mod sealed {
    pub trait Sealed {}
}

macro_rules! callback_context {
    ($($context:ty),+ $(,)?) => {
        $(
            impl sealed::Sealed for $context {}
            impl CallbackContext for $context {}
        )+
    };
}

callback_context!(SetupContext, CleanupContext, BlockContext, RenderContext);

/// Where in an interleaved buffer of `frames` frames and `channels`
/// channels the samples of `range` are, clamped to the buffer.
///
/// The clamping is for a context whose frame counts and the range
/// derived from them disagree, which cannot happen for a range from
/// [`partition`] but is cheaper to rule out than to reason about.
#[inline]
const fn samples(frames: usize, channels: usize, range: &Range<usize>) -> (usize, usize) {
    let end = if range.end < frames {
        range.end
    } else {
        frames
    };
    let start = if range.start < end { range.start } else { end };
    (start * channels, (end - start) * channels)
}

/// A shared reference to the samples of `range` alone.
///
/// # Safety
/// `ptr` must be null or valid for reads of `frames * channels`
/// elements for the lifetime of the returned slice. Only the samples
/// of `range` are covered by it, so the rest of the buffer may be
/// borrowed elsewhere.
#[inline]
const unsafe fn share<'a, T>(
    ptr: *const T,
    frames: usize,
    channels: usize,
    range: Range<usize>,
) -> &'a [T] {
    if ptr.is_null() {
        return &[];
    }
    let (offset, len) = samples(frames, channels, &range);
    unsafe { slice::from_raw_parts(ptr.add(offset), len) }
}

/// An exclusive reference to the samples of `range` alone.
///
/// This is what makes concurrent `render` calls sound: each one asks
/// for a different `range`, so the references never overlap, and none
/// of them ever covers a sample belonging to another thread — not even
/// for as long as it takes to index into it.
///
/// # Safety
/// `ptr` must be null or valid for reads and writes of
/// `frames * channels` elements for the lifetime of the returned
/// slice, with the samples of `range` unaliased.
#[inline]
const unsafe fn share_mut<'a, T>(
    ptr: *mut T,
    frames: usize,
    channels: usize,
    range: Range<usize>,
) -> &'a mut [T] {
    if ptr.is_null() {
        return &mut [];
    }
    let (offset, len) = samples(frames, channels, &range);
    unsafe { slice::from_raw_parts_mut(ptr.add(offset), len) }
}

/// Where block `frame` sits within this thread's share of the block,
/// which is how the indexed accessors reach it once the reference
/// covers only that share.
///
/// # Panics
/// If `frame` is outside `range`.
#[inline]
fn frame_offset(range: &Range<usize>, frame: usize, domain: &str) -> usize {
    assert!(
        range.contains(&frame),
        "{domain} frame {frame} is outside this thread's range {range:?}"
    );
    frame - range.start
}

/// # Panics
/// If `channel` is out of range.
#[inline]
fn digital_value_mask(channels: usize, channel: usize) -> u32 {
    assert!(channel < channels, "digital channel out of range");
    1 << (channel + DIGITAL_VALUE_SHIFT)
}

/// # Panics
/// If `channel` is out of range.
#[inline]
fn digital_direction_mask(channels: usize, channel: usize) -> u32 {
    assert!(channel < channels, "digital channel out of range");
    1 << channel
}

/// # Safety
/// `ptr` must be null or valid for reads of `len` elements for the
/// lifetime of the returned slice.
#[inline]
const unsafe fn shared<'a, T>(ptr: *const T, len: usize) -> &'a [T] {
    if ptr.is_null() {
        &[]
    } else {
        unsafe { slice::from_raw_parts(ptr, len) }
    }
}

/// # Safety
/// `ptr` must be null or valid for reads and writes of `len` elements,
/// unaliased for the lifetime of the returned slice.
#[inline]
const unsafe fn exclusive<'a, T>(ptr: *mut T, len: usize) -> &'a mut [T] {
    if ptr.is_null() {
        &mut []
    } else {
        unsafe { slice::from_raw_parts_mut(ptr, len) }
    }
}

#[inline]
const fn set_bits(word: &mut u32, mask: u32, on: bool) {
    if on {
        *word |= mask;
    } else {
        *word &= !mask;
    }
}

#[cfg(test)]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::float_cmp,
    reason = "tests use small exact values where these casts and comparisons are lossless"
)]
#[allow(
    clippy::redundant_pub_crate,
    reason = "the fixture is shared with the runtime tests, which are in another module"
)]
pub(crate) mod tests {
    use core::mem;

    use super::*;

    const AUDIO_FRAMES: usize = 4;
    const AUDIO_IN_CHANNELS: usize = 2;
    // Gem-like: two audio channels plus two analog-outs-as-audio.
    const AUDIO_OUT_CHANNELS: usize = 4;
    const ANALOG_FRAMES: usize = 4;
    const ANALOG_IN_CHANNELS: usize = 4;
    const ANALOG_OUT_CHANNELS: usize = 2;
    const DIGITAL_FRAMES: usize = 4;
    const DIGITAL_CHANNELS: usize = 16;

    /// Buffers plus a context pointing at them, standing in for what
    /// libbela hands a callback.
    ///
    /// Boxed because the context holds pointers into the fixture's own
    /// fields, which must not move.
    pub(crate) struct Fixture {
        audio_in: Vec<f32>,
        pub(crate) audio_out: Vec<f32>,
        analog_in: Vec<f32>,
        pub(crate) analog_out: Vec<f32>,
        pub(crate) digital: Vec<u32>,
        pub(crate) context: BelaContext,
    }

    impl Fixture {
        pub(crate) fn new() -> Box<Self> {
            Self::with_threads(1)
        }

        /// A fixture whose context reports `threads` render threads.
        pub(crate) fn with_threads(threads: u32) -> Box<Self> {
            // Input samples encode their own position as frame*10+channel.
            let audio_in: Vec<f32> = (0..AUDIO_FRAMES * AUDIO_IN_CHANNELS)
                .map(|i| {
                    let (frame, channel) = (i / AUDIO_IN_CHANNELS, i % AUDIO_IN_CHANNELS);
                    (frame * 10 + channel) as f32
                })
                .collect();
            let analog_in: Vec<f32> = (0..ANALOG_FRAMES * ANALOG_IN_CHANNELS)
                .map(|i| {
                    let (frame, channel) = (i / ANALOG_IN_CHANNELS, i % ANALOG_IN_CHANNELS);
                    (frame * 10 + channel) as f32
                })
                .collect();
            let mut fixture = Box::new(Self {
                audio_in,
                audio_out: vec![0.0; AUDIO_FRAMES * AUDIO_OUT_CHANNELS],
                analog_in,
                analog_out: vec![0.0; ANALOG_FRAMES * ANALOG_OUT_CHANNELS],
                digital: vec![0; DIGITAL_FRAMES],
                context: unsafe { mem::zeroed() },
            });
            fixture.context.audioIn = fixture.audio_in.as_ptr();
            fixture.context.audioOut = fixture.audio_out.as_mut_ptr();
            fixture.context.analogIn = fixture.analog_in.as_ptr();
            fixture.context.analogOut = fixture.analog_out.as_mut_ptr();
            fixture.context.digital = fixture.digital.as_mut_ptr();
            fixture.context.audioFrames = AUDIO_FRAMES as u32;
            fixture.context.audioInChannels = AUDIO_IN_CHANNELS as u32;
            fixture.context.audioOutChannels = AUDIO_OUT_CHANNELS as u32;
            fixture.context.audioSampleRate = 44100.0;
            fixture.context.analogFrames = ANALOG_FRAMES as u32;
            fixture.context.analogInChannels = ANALOG_IN_CHANNELS as u32;
            fixture.context.analogOutChannels = ANALOG_OUT_CHANNELS as u32;
            fixture.context.analogSampleRate = 44100.0;
            fixture.context.digitalFrames = DIGITAL_FRAMES as u32;
            fixture.context.digitalChannels = DIGITAL_CHANNELS as u32;
            fixture.context.audioFramesElapsed = 128;
            fixture.context.thisThread = 0;
            fixture.context.threadCount = threads;
            fixture
        }

        pub(crate) fn block(&mut self) -> &mut BlockContext {
            unsafe { BlockContext::from_mut_ptr(&raw mut self.context) }
        }

        pub(crate) fn setup(&mut self) -> &mut SetupContext {
            unsafe { SetupContext::from_mut_ptr(&raw mut self.context) }
        }

        /// The render context thread `thread` would see, with the
        /// thread number written into the context the way libbela's
        /// mirrored copies carry it.
        pub(crate) fn render(&mut self, thread: u32) -> &mut RenderContext {
            self.context.thisThread = thread;
            unsafe { RenderContext::from_mut_ptr(&raw mut self.context) }
        }
    }

    #[test]
    fn metadata_accessors_reflect_the_struct() {
        let mut fixture = Fixture::with_threads(4);
        let context = fixture.block();

        assert_eq!(context.audio_frames(), AUDIO_FRAMES);
        assert_eq!(context.audio_in_channels(), AUDIO_IN_CHANNELS);
        assert_eq!(context.audio_out_channels(), AUDIO_OUT_CHANNELS);
        assert_eq!(context.audio_sample_rate(), 44100.0);
        assert_eq!(context.analog_frames(), ANALOG_FRAMES);
        assert_eq!(context.digital_channels(), DIGITAL_CHANNELS);
        assert_eq!(context.audio_frames_elapsed(), 128);
        assert_eq!(context.underrun_count(), 0);
        assert_eq!(context.this_thread(), 0);
        assert_eq!(context.thread_count(), 4);
    }

    #[test]
    fn the_same_metadata_is_on_every_phase() {
        let mut fixture = Fixture::new();
        assert_eq!(fixture.setup().audio_frames(), AUDIO_FRAMES);
        assert_eq!(fixture.render(0).audio_sample_rate(), 44100.0);
        let cleanup = unsafe { CleanupContext::from_mut_ptr(&raw mut fixture.context) };
        assert_eq!(cleanup.audio_frames_elapsed(), 128);
    }

    #[test]
    fn one_render_thread_is_spelled_either_way() {
        for spelling in [0, 1] {
            let mut fixture = Fixture::with_threads(spelling);
            assert_eq!(
                fixture.render(0).thread_count(),
                1,
                "threadCount {spelling} means one render thread"
            );
            assert_eq!(
                fixture.render(0).audio_frame_range(),
                0..AUDIO_FRAMES,
                "the one thread gets the whole block"
            );
        }
    }

    // --- Partitioning ---

    #[test]
    fn partitions_tile_the_block_exactly() {
        for frames in 0..40_usize {
            for count in 1..8_usize {
                let mut previous_end = 0;
                for thread in 0..count {
                    let range = partition(frames, thread, count);
                    assert_eq!(
                        range.start, previous_end,
                        "{frames} frames, {count} threads"
                    );
                    assert!(range.start <= range.end, "{frames} frames, {count} threads");
                    previous_end = range.end;
                }
                assert_eq!(previous_end, frames, "{frames} frames, {count} threads");
            }
        }
    }

    #[test]
    fn uneven_partitions_differ_by_at_most_one_frame() {
        // 7 frames over 4 threads: 1, 2, 2, 2 rather than 1, 1, 1, 4.
        let lengths: Vec<usize> = (0..4).map(|t| partition(7, t, 4).len()).collect();
        assert_eq!(lengths, vec![1, 2, 2, 2]);
        assert_eq!(lengths.iter().sum::<usize>(), 7);
    }

    #[test]
    fn more_threads_than_frames_gives_empty_ranges() {
        let ranges: Vec<Range<usize>> = (0..4).map(|t| partition(2, t, 4)).collect();
        assert_eq!(ranges, vec![0..0, 0..1, 1..1, 1..2]);
    }

    #[test]
    fn a_thread_outside_the_count_gets_nothing() {
        // Never reached through a context — the runtime refuses such a
        // callback before building one — but the split itself must not
        // hand out somebody else's frames if it is.
        assert_eq!(partition(8, 4, 4), 8..8);
        assert_eq!(partition(8, 9, 4), 8..8);
    }

    #[test]
    fn a_zero_thread_count_is_one_thread() {
        assert_eq!(partition(8, 0, 0), 0..8);
    }

    // --- BlockContext ---

    #[test]
    fn audio_read_uses_the_interleaved_layout() {
        let mut fixture = Fixture::new();
        let context = fixture.block();

        // Sample values encode frame*10+channel.
        assert_eq!(context.audio_read(0, 0), 0.0);
        assert_eq!(context.audio_read(0, 1), 1.0);
        assert_eq!(context.audio_read(3, 1), 31.0);
        assert_eq!(context.audio_in().len(), AUDIO_FRAMES * AUDIO_IN_CHANNELS);
    }

    #[test]
    fn audio_write_targets_exactly_one_sample() {
        let mut fixture = Fixture::new();
        fixture.block().audio_write(2, 3, 0.5);

        let index = 2 * AUDIO_OUT_CHANNELS + 3;
        for (i, &sample) in fixture.audio_out.iter().enumerate() {
            let expected = if i == index { 0.5 } else { 0.0 };
            assert_eq!(sample, expected, "sample {i}");
        }
    }

    #[test]
    fn analog_read_uses_the_interleaved_layout() {
        let mut fixture = Fixture::new();
        assert_eq!(fixture.block().analog_read(2, 3), 23.0);
    }

    #[test]
    fn analog_write_persists_to_the_end_of_the_block() {
        let mut fixture = Fixture::new();
        fixture.block().analog_write(1, 0, 0.7);

        for frame in 0..ANALOG_FRAMES {
            let expected = if frame >= 1 { 0.7 } else { 0.0 };
            assert_eq!(fixture.analog_out[frame * ANALOG_OUT_CHANNELS], expected);
            // The other channel is untouched.
            assert_eq!(fixture.analog_out[frame * ANALOG_OUT_CHANNELS + 1], 0.0);
        }
    }

    #[test]
    fn analog_write_once_targets_exactly_one_sample() {
        let mut fixture = Fixture::new();
        fixture.block().analog_write_once(1, 1, 0.7);

        let index = ANALOG_OUT_CHANNELS + 1;
        for (i, &sample) in fixture.analog_out.iter().enumerate() {
            let expected = if i == index { 0.7 } else { 0.0 };
            assert_eq!(sample, expected, "sample {i}");
        }
    }

    #[test]
    fn digital_value_bits_live_in_the_high_half_word() {
        let mut fixture = Fixture::new();
        let context = fixture.block();

        context.digital_write_once(0, 3, true);
        assert_eq!(fixture.digital[0], 1 << (3 + 16));

        let context = fixture.block();
        assert!(context.digital_read(0, 3));
        assert!(!context.digital_read(0, 2));
        assert!(!context.digital_read(1, 3));
    }

    #[test]
    fn digital_write_persists_and_clears() {
        let mut fixture = Fixture::new();
        fixture.block().digital_write(1, 5, true);
        for frame in 0..DIGITAL_FRAMES {
            assert_eq!(fixture.digital[frame], u32::from(frame >= 1) << (5 + 16));
        }

        fixture.block().digital_write(2, 5, false);
        for frame in 0..DIGITAL_FRAMES {
            assert_eq!(fixture.digital[frame], u32::from(frame == 1) << (5 + 16));
        }
    }

    #[test]
    fn pin_mode_sets_direction_bits_in_the_low_half_word() {
        let mut fixture = Fixture::new();
        fixture.block().pin_mode(0, 7, PinMode::Input);
        for frame in 0..DIGITAL_FRAMES {
            assert_eq!(fixture.digital[frame], 1 << 7);
        }

        fixture.block().pin_mode_once(2, 7, PinMode::Output);
        for frame in 0..DIGITAL_FRAMES {
            assert_eq!(fixture.digital[frame], u32::from(frame != 2) << 7);
        }
    }

    #[test]
    fn disabled_io_yields_empty_slices() {
        let mut context: BelaContext = unsafe { mem::zeroed() };
        let context = unsafe { BlockContext::from_mut_ptr(&raw mut context) };

        assert!(context.audio_in().is_empty());
        assert!(context.audio_out().is_empty());
        assert!(context.analog_in().is_empty());
        assert!(context.analog_out().is_empty());
        assert!(context.digital().is_empty());
    }

    #[test]
    #[should_panic(expected = "audio input channel out of range")]
    fn audio_read_rejects_out_of_range_channels() {
        let mut fixture = Fixture::new();
        let _ = fixture.block().audio_read(0, AUDIO_IN_CHANNELS);
    }

    #[test]
    #[should_panic(expected = "index out of bounds")]
    fn audio_read_rejects_out_of_range_frames() {
        let mut fixture = Fixture::new();
        let _ = fixture.block().audio_read(AUDIO_FRAMES, 0);
    }

    #[test]
    #[should_panic(expected = "digital channel out of range")]
    fn digital_write_rejects_out_of_range_channels() {
        let mut fixture = Fixture::new();
        fixture.block().digital_write(0, DIGITAL_CHANNELS, true);
    }

    // --- RenderContext ---

    #[test]
    fn a_render_context_reads_the_whole_block() {
        let mut fixture = Fixture::with_threads(4);
        let context = fixture.render(3);

        assert_eq!(context.audio_frame_range(), 3..4, "one frame of four");
        // Frame 0 belongs to another thread, and is still readable.
        assert_eq!(context.audio_read(0, 1), 1.0);
        assert_eq!(context.audio_in().len(), AUDIO_FRAMES * AUDIO_IN_CHANNELS);
        assert_eq!(context.analog_read(0, 3), 3.0);
    }

    #[test]
    fn a_render_context_writes_only_its_own_frames() {
        let mut fixture = Fixture::with_threads(2);

        for thread in 0..2 {
            let context = fixture.render(thread);
            let range = context.audio_frame_range();
            for frame in range {
                context.audio_write(frame, 0, frame as f32 + 1.0);
            }
        }

        // Between them the two threads covered the block exactly.
        for frame in 0..AUDIO_FRAMES {
            assert_eq!(
                fixture.audio_out[frame * AUDIO_OUT_CHANNELS],
                frame as f32 + 1.0,
                "frame {frame}"
            );
        }
    }

    #[test]
    fn the_output_slice_is_this_threads_share() {
        let mut fixture = Fixture::with_threads(2);
        let context = fixture.render(1);

        let out = context.audio_out();
        assert_eq!(out.len(), 2 * AUDIO_OUT_CHANNELS, "two of four frames");
        // Index 0 is the first sample of the range, which is frame 2.
        out[0] = 9.0;
        assert_eq!(fixture.audio_out[2 * AUDIO_OUT_CHANNELS], 9.0);
        assert_eq!(fixture.audio_out[0], 0.0, "frame 0 belongs to thread 0");
    }

    #[test]
    fn analog_and_digital_slices_are_partitioned_too() {
        let mut fixture = Fixture::with_threads(4);
        let context = fixture.render(2);

        assert_eq!(context.analog_frame_range(), 2..3);
        assert_eq!(context.digital_frame_range(), 2..3);
        assert_eq!(context.analog_out().len(), ANALOG_OUT_CHANNELS);
        assert_eq!(context.digital_mut().len(), 1);
    }

    #[test]
    fn persisting_writes_stop_at_the_end_of_the_range() {
        let mut fixture = Fixture::with_threads(2);
        // Thread 0 owns frames 0..2, so this must not reach frame 2.
        fixture.render(0).analog_write(0, 0, 0.7);
        fixture.render(0).digital_write(0, 5, true);

        for frame in 0..ANALOG_FRAMES {
            let expected = if frame < 2 { 0.7 } else { 0.0 };
            assert_eq!(
                fixture.analog_out[frame * ANALOG_OUT_CHANNELS],
                expected,
                "analog frame {frame}"
            );
        }
        for frame in 0..DIGITAL_FRAMES {
            assert_eq!(
                fixture.digital[frame],
                u32::from(frame < 2) << (5 + 16),
                "digital frame {frame}"
            );
        }
    }

    #[test]
    fn an_empty_range_hands_out_nothing_to_write() {
        // Four threads, and a block with fewer frames than that would
        // need: the first thread's share is empty.
        let mut fixture = Fixture::with_threads(8);
        let context = fixture.render(0);

        assert_eq!(context.audio_frame_range(), 0..0);
        assert!(context.audio_out().is_empty());
        assert!(context.analog_out().is_empty());
        assert!(context.digital_mut().is_empty());
    }

    #[test]
    #[should_panic(expected = "audio frame 0 is outside this thread's range 2..4")]
    fn writing_another_threads_frame_panics() {
        let mut fixture = Fixture::with_threads(2);
        fixture.render(1).audio_write(0, 0, 1.0);
    }

    #[test]
    #[should_panic(expected = "analog frame 3 is outside this thread's range 0..2")]
    fn a_persisting_analog_write_outside_the_range_panics() {
        let mut fixture = Fixture::with_threads(2);
        fixture.render(0).analog_write(3, 0, 1.0);
    }

    #[test]
    #[should_panic(expected = "digital frame 0 is outside this thread's range 2..4")]
    fn a_digital_write_outside_the_range_panics() {
        let mut fixture = Fixture::with_threads(2);
        fixture.render(1).digital_write(0, 5, true);
    }

    #[test]
    fn digital_reads_are_this_threads_share_too() {
        // Unlike the audio and analog inputs, the digital words are
        // where the outputs go, so a read past this thread's range
        // would be a read of what another thread is writing.
        let mut fixture = Fixture::with_threads(2);
        fixture.render(0).digital_write_once(1, 5, true);

        let context = fixture.render(0);
        assert_eq!(context.digital().len(), 2, "two of four frames");
        assert!(context.digital_read(1, 5));
        assert!(!context.digital_read(0, 5));
    }

    #[test]
    #[should_panic(expected = "digital frame 3 is outside this thread's range 0..2")]
    fn a_digital_read_outside_the_range_panics() {
        let mut fixture = Fixture::with_threads(2);
        let _ = fixture.render(0).digital_read(3, 5);
    }

    #[test]
    fn audio_and_analog_reads_are_not_bounded_that_way() {
        // Their inputs are buffers of their own, which nothing writes.
        let mut fixture = Fixture::with_threads(2);
        let context = fixture.render(1);

        assert_eq!(context.audio_read(0, 0), 0.0);
        assert_eq!(context.analog_read(0, 0), 0.0);
    }
}
