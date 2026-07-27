use bela_sys::BelaContext;

/// Direction of a digital (GPIO) pin. All pins begin as inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinMode {
    Input,
    Output,
}

// Within a digital frame word, bits 0-15 hold pin directions
// (1 = input) and bits 16-31 hold pin values.
const DIGITAL_VALUE_SHIFT: usize = 16;

/// View over the [`BelaContext`] passed to the application callbacks.
///
/// The accessors mirror the C helpers from `Bela.h` / `Utilities.h`
/// and assume the default interleaved buffer layout: sample index =
/// `frame * channels + channel`.
///
/// # Bela Gem semantics
///
/// On Bela Gem the analog outputs are part of the audio outputs: use
/// [`audio_write`](Context::audio_write) with the channel offset by +2
/// instead of [`analog_write`](Context::analog_write), and expect
/// [`uniform_sample_rate`](crate::Settings::uniform_sample_rate)
/// behaviour (analog frames == audio frames) by default. Output values
/// do not persist across blocks; the within-block persistence of
/// [`analog_write`](Context::analog_write) and
/// [`digital_write`](Context::digital_write) (writing from `frame` to
/// the end of the block) is unchanged.
///
/// # Panics
///
/// The indexed accessors panic when `frame` or `channel` is out of
/// range (the C equivalents would read or write out of bounds). On the
/// device a panic aborts the whole process, so treat these as
/// programming errors, not recoverable conditions.
#[repr(transparent)]
pub struct Context(BelaContext);

impl Context {
    /// Reborrows a raw `BelaContext` pointer as a [`Context`].
    ///
    /// # Safety
    ///
    /// `ptr` must be non-null, properly aligned, and point to a live
    /// `BelaContext` that is not accessed through any other reference
    /// for the duration of `'a`. The buffer pointers inside must be
    /// either null or valid for the lengths implied by the frame and
    /// channel counts.
    pub unsafe fn from_mut_ptr<'a>(ptr: *mut BelaContext) -> &'a mut Context {
        // repr(transparent) makes the cast sound.
        unsafe { &mut *ptr.cast::<Context>() }
    }

    /// Read access to the underlying `BelaContext`.
    pub fn as_sys(&self) -> &BelaContext {
        &self.0
    }

    /// Mutable access to the underlying `BelaContext`.
    ///
    /// # Safety
    ///
    /// The caller must not invalidate data the audio system or the
    /// safe accessors rely on, e.g. by overwriting buffer pointers,
    /// frame counts or channel counts. Writing *through* the output
    /// buffer pointers is fine.
    pub unsafe fn as_sys_mut(&mut self) -> &mut BelaContext {
        &mut self.0
    }

    // --- Frame counts, channel counts and sample rates ---

    /// Number of audio frames per block.
    pub fn audio_frames(&self) -> usize {
        self.0.audioFrames as usize
    }

    pub fn audio_in_channels(&self) -> usize {
        self.0.audioInChannels as usize
    }

    pub fn audio_out_channels(&self) -> usize {
        self.0.audioOutChannels as usize
    }

    /// Audio sample rate in Hz.
    pub fn audio_sample_rate(&self) -> f32 {
        self.0.audioSampleRate
    }

    /// Number of analog frames per block; 0 if analog I/O is disabled.
    pub fn analog_frames(&self) -> usize {
        self.0.analogFrames as usize
    }

    pub fn analog_in_channels(&self) -> usize {
        self.0.analogInChannels as usize
    }

    pub fn analog_out_channels(&self) -> usize {
        self.0.analogOutChannels as usize
    }

    /// Analog sample rate in Hz; 0 if analog I/O is disabled.
    pub fn analog_sample_rate(&self) -> f32 {
        self.0.analogSampleRate
    }

    /// Number of digital frames per block; 0 if digital I/O is disabled.
    pub fn digital_frames(&self) -> usize {
        self.0.digitalFrames as usize
    }

    pub fn digital_channels(&self) -> usize {
        self.0.digitalChannels as usize
    }

    /// Digital sample rate in Hz.
    pub fn digital_sample_rate(&self) -> f32 {
        self.0.digitalSampleRate
    }

    /// Total audio frames elapsed as of the beginning of this block.
    pub fn audio_frames_elapsed(&self) -> u64 {
        self.0.audioFramesElapsed
    }

    /// Number of detected underruns.
    pub fn underrun_count(&self) -> u32 {
        self.0.underrunCount
    }

    // --- Whole-buffer access (interleaved) ---

    /// Audio input samples; empty outside `render` or with audio
    /// disabled. Length is `audio_frames() * audio_in_channels()`.
    pub fn audio_in(&self) -> &[f32] {
        unsafe {
            shared(
                self.0.audioIn,
                self.audio_frames() * self.audio_in_channels(),
            )
        }
    }

    /// Audio output samples. Length is
    /// `audio_frames() * audio_out_channels()`.
    pub fn audio_out(&mut self) -> &mut [f32] {
        unsafe {
            exclusive(
                self.0.audioOut,
                self.audio_frames() * self.audio_out_channels(),
            )
        }
    }

    /// Analog input samples; empty if analog I/O is disabled. Length
    /// is `analog_frames() * analog_in_channels()`.
    pub fn analog_in(&self) -> &[f32] {
        unsafe {
            shared(
                self.0.analogIn,
                self.analog_frames() * self.analog_in_channels(),
            )
        }
    }

    /// Analog output samples; empty if analog I/O is disabled. Length
    /// is `analog_frames() * analog_out_channels()`.
    pub fn analog_out(&mut self) -> &mut [f32] {
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
    pub fn digital(&self) -> &[u32] {
        unsafe { shared(self.0.digital, self.digital_frames()) }
    }

    pub fn digital_mut(&mut self) -> &mut [u32] {
        unsafe { exclusive(self.0.digital, self.digital_frames()) }
    }

    // --- Indexed access (mirrors the C helpers) ---

    /// Audio input sample at `frame` for `channel` (`audioRead`).
    pub fn audio_read(&self, frame: usize, channel: usize) -> f32 {
        let channels = self.audio_in_channels();
        assert!(channel < channels, "audio input channel out of range");
        self.audio_in()[frame * channels + channel]
    }

    /// Sets the audio output at `frame` for `channel` (`audioWrite`).
    /// Audio outputs never persist.
    pub fn audio_write(&mut self, frame: usize, channel: usize, value: f32) {
        let channels = self.audio_out_channels();
        assert!(channel < channels, "audio output channel out of range");
        self.audio_out()[frame * channels + channel] = value;
    }

    /// Analog input sample at `frame` for `channel` (`analogRead`).
    pub fn analog_read(&self, frame: usize, channel: usize) -> f32 {
        let channels = self.analog_in_channels();
        assert!(channel < channels, "analog input channel out of range");
        self.analog_in()[frame * channels + channel]
    }

    /// Sets the analog output for `channel` from `frame` to the end of
    /// the block (`analogWrite`). Not the primary path on Bela Gem —
    /// see the type-level documentation.
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
    pub fn analog_write_once(&mut self, frame: usize, channel: usize, value: f32) {
        let channels = self.analog_out_channels();
        assert!(channel < channels, "analog output channel out of range");
        self.analog_out()[frame * channels + channel] = value;
    }

    /// Value of the digital `channel` at `frame` (`digitalRead`).
    pub fn digital_read(&self, frame: usize, channel: usize) -> bool {
        let mask = self.digital_value_mask(channel);
        self.digital()[frame] & mask != 0
    }

    /// Sets the digital output `channel` from `frame` to the end of
    /// the block (`digitalWrite`).
    pub fn digital_write(&mut self, frame: usize, channel: usize, value: bool) {
        let mask = self.digital_value_mask(channel);
        for word in self.digital_mut().iter_mut().skip(frame) {
            set_bits(word, mask, value);
        }
    }

    /// Sets the digital output `channel` at `frame` only
    /// (`digitalWriteOnce`).
    pub fn digital_write_once(&mut self, frame: usize, channel: usize, value: bool) {
        let mask = self.digital_value_mask(channel);
        set_bits(&mut self.digital_mut()[frame], mask, value);
    }

    /// Sets the direction of digital `channel` from `frame` to the end
    /// of the block (`pinMode`).
    pub fn pin_mode(&mut self, frame: usize, channel: usize, mode: PinMode) {
        let mask = self.digital_direction_mask(channel);
        for word in self.digital_mut().iter_mut().skip(frame) {
            set_bits(word, mask, mode == PinMode::Input);
        }
    }

    /// Sets the direction of digital `channel` at `frame` only
    /// (`pinModeOnce`).
    pub fn pin_mode_once(&mut self, frame: usize, channel: usize, mode: PinMode) {
        let mask = self.digital_direction_mask(channel);
        set_bits(&mut self.digital_mut()[frame], mask, mode == PinMode::Input);
    }

    fn digital_value_mask(&self, channel: usize) -> u32 {
        assert!(
            channel < self.digital_channels(),
            "digital channel out of range"
        );
        1 << (channel + DIGITAL_VALUE_SHIFT)
    }

    fn digital_direction_mask(&self, channel: usize) -> u32 {
        assert!(
            channel < self.digital_channels(),
            "digital channel out of range"
        );
        1 << channel
    }
}

/// # Safety
/// `ptr` must be null or valid for reads of `len` elements for the
/// lifetime of the returned slice.
unsafe fn shared<'a, T>(ptr: *const T, len: usize) -> &'a [T] {
    if ptr.is_null() {
        &[]
    } else {
        unsafe { core::slice::from_raw_parts(ptr, len) }
    }
}

/// # Safety
/// `ptr` must be null or valid for reads and writes of `len` elements,
/// unaliased for the lifetime of the returned slice.
unsafe fn exclusive<'a, T>(ptr: *mut T, len: usize) -> &'a mut [T] {
    if ptr.is_null() {
        &mut []
    } else {
        unsafe { core::slice::from_raw_parts_mut(ptr, len) }
    }
}

fn set_bits(word: &mut u32, mask: u32, on: bool) {
    if on {
        *word |= mask;
    } else {
        *word &= !mask;
    }
}

#[cfg(test)]
mod tests {
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

    struct Fixture {
        _audio_in: Vec<f32>,
        audio_out: Vec<f32>,
        _analog_in: Vec<f32>,
        analog_out: Vec<f32>,
        digital: Vec<u32>,
        context: BelaContext,
    }

    impl Fixture {
        fn new() -> Box<Fixture> {
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
            let mut fixture = Box::new(Fixture {
                _audio_in: audio_in,
                audio_out: vec![0.0; AUDIO_FRAMES * AUDIO_OUT_CHANNELS],
                _analog_in: analog_in,
                analog_out: vec![0.0; ANALOG_FRAMES * ANALOG_OUT_CHANNELS],
                digital: vec![0; DIGITAL_FRAMES],
                context: unsafe { core::mem::zeroed() },
            });
            fixture.context.audioIn = fixture._audio_in.as_ptr();
            fixture.context.audioOut = fixture.audio_out.as_mut_ptr();
            fixture.context.analogIn = fixture._analog_in.as_ptr();
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
            fixture
        }

        fn context(&mut self) -> &mut Context {
            unsafe { Context::from_mut_ptr(&raw mut self.context) }
        }
    }

    #[test]
    fn metadata_accessors_reflect_the_struct() {
        let mut fixture = Fixture::new();
        let context = fixture.context();

        assert_eq!(context.audio_frames(), AUDIO_FRAMES);
        assert_eq!(context.audio_in_channels(), AUDIO_IN_CHANNELS);
        assert_eq!(context.audio_out_channels(), AUDIO_OUT_CHANNELS);
        assert_eq!(context.audio_sample_rate(), 44100.0);
        assert_eq!(context.analog_frames(), ANALOG_FRAMES);
        assert_eq!(context.digital_channels(), DIGITAL_CHANNELS);
        assert_eq!(context.audio_frames_elapsed(), 128);
        assert_eq!(context.underrun_count(), 0);
    }

    #[test]
    fn audio_read_uses_the_interleaved_layout() {
        let mut fixture = Fixture::new();
        let context = fixture.context();

        // Sample values encode frame*10+channel.
        assert_eq!(context.audio_read(0, 0), 0.0);
        assert_eq!(context.audio_read(0, 1), 1.0);
        assert_eq!(context.audio_read(3, 1), 31.0);
        assert_eq!(context.audio_in().len(), AUDIO_FRAMES * AUDIO_IN_CHANNELS);
    }

    #[test]
    fn audio_write_targets_exactly_one_sample() {
        let mut fixture = Fixture::new();
        fixture.context().audio_write(2, 3, 0.5);

        let index = 2 * AUDIO_OUT_CHANNELS + 3;
        for (i, &sample) in fixture.audio_out.iter().enumerate() {
            let expected = if i == index { 0.5 } else { 0.0 };
            assert_eq!(sample, expected, "sample {i}");
        }
    }

    #[test]
    fn analog_read_uses_the_interleaved_layout() {
        let mut fixture = Fixture::new();
        assert_eq!(fixture.context().analog_read(2, 3), 23.0);
    }

    #[test]
    fn analog_write_persists_to_the_end_of_the_block() {
        let mut fixture = Fixture::new();
        fixture.context().analog_write(1, 0, 0.7);

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
        fixture.context().analog_write_once(1, 1, 0.7);

        let index = ANALOG_OUT_CHANNELS + 1;
        for (i, &sample) in fixture.analog_out.iter().enumerate() {
            let expected = if i == index { 0.7 } else { 0.0 };
            assert_eq!(sample, expected, "sample {i}");
        }
    }

    #[test]
    fn digital_value_bits_live_in_the_high_half_word() {
        let mut fixture = Fixture::new();
        let context = fixture.context();

        context.digital_write_once(0, 3, true);
        assert_eq!(fixture.digital[0], 1 << (3 + 16));

        let context = fixture.context();
        assert!(context.digital_read(0, 3));
        assert!(!context.digital_read(0, 2));
        assert!(!context.digital_read(1, 3));
    }

    #[test]
    fn digital_write_persists_and_clears() {
        let mut fixture = Fixture::new();
        fixture.context().digital_write(1, 5, true);
        for frame in 0..DIGITAL_FRAMES {
            assert_eq!(fixture.digital[frame], u32::from(frame >= 1) << (5 + 16));
        }

        fixture.context().digital_write(2, 5, false);
        for frame in 0..DIGITAL_FRAMES {
            assert_eq!(fixture.digital[frame], u32::from(frame == 1) << (5 + 16));
        }
    }

    #[test]
    fn pin_mode_sets_direction_bits_in_the_low_half_word() {
        let mut fixture = Fixture::new();
        fixture.context().pin_mode(0, 7, PinMode::Input);
        for frame in 0..DIGITAL_FRAMES {
            assert_eq!(fixture.digital[frame], 1 << 7);
        }

        fixture.context().pin_mode_once(2, 7, PinMode::Output);
        for frame in 0..DIGITAL_FRAMES {
            assert_eq!(fixture.digital[frame], u32::from(frame != 2) << 7);
        }
    }

    #[test]
    fn disabled_io_yields_empty_slices() {
        let mut context: BelaContext = unsafe { core::mem::zeroed() };
        let context = unsafe { Context::from_mut_ptr(&raw mut context) };

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
        fixture.context().audio_read(0, AUDIO_IN_CHANNELS);
    }

    #[test]
    #[should_panic]
    fn audio_read_rejects_out_of_range_frames() {
        let mut fixture = Fixture::new();
        fixture.context().audio_read(AUDIO_FRAMES, 0);
    }

    #[test]
    #[should_panic(expected = "digital channel out of range")]
    fn digital_write_rejects_out_of_range_channels() {
        let mut fixture = Fixture::new();
        fixture.context().digital_write(0, DIGITAL_CHANNELS, true);
    }
}
