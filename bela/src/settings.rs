use core::ffi::c_int;
use core::num::NonZeroU32;

use bela_sys::BelaInitSettings;

use crate::error::Error;

/// Overrides applied on top of Bela's default initialisation settings.
///
/// Unset fields keep the values produced by `Bela_defaultSettings()` on
/// the device, so this type never has to replicate the C-side defaults.
///
/// Every method here is a `const fn`, starting with
/// [`new`](Settings::new), so a whole configuration can be settled at
/// compile time and handed to every audio system a program builds:
///
/// ```
/// use bela::Settings;
///
/// const SETTINGS: Settings = Settings::new().period_size(64).use_analog(true);
/// # let _ = SETTINGS;
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Settings {
    period_size: Option<u32>,
    use_analog: Option<bool>,
    use_digital: Option<bool>,
    num_analog_in_channels: Option<u32>,
    num_analog_out_channels: Option<u32>,
    num_digital_channels: Option<u32>,
    detect_underruns: Option<bool>,
    verbose: Option<bool>,
    high_performance_mode: Option<bool>,
    uniform_sample_rate: Option<bool>,
    stop_button_pin: Option<i32>,
    thread_count: Option<u32>,
    cpu_monitoring: Option<NonZeroU32>,
    begin_muted: Option<bool>,
}

impl Default for Settings {
    /// The same empty set of overrides [`new`](Settings::new) makes.
    fn default() -> Self {
        Self::new()
    }
}

impl Settings {
    /// Creates an empty set of overrides.
    ///
    /// The fields are written out rather than derived so that this can
    /// be `const`, which is what lets a whole configuration be one —
    /// every builder method below already was.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            period_size: None,
            use_analog: None,
            use_digital: None,
            num_analog_in_channels: None,
            num_analog_out_channels: None,
            num_digital_channels: None,
            detect_underruns: None,
            verbose: None,
            high_performance_mode: None,
            uniform_sample_rate: None,
            stop_button_pin: None,
            thread_count: None,
            cpu_monitoring: None,
            begin_muted: None,
        }
    }

    /// Number of audio frames per period ("block size").
    ///
    /// # Digital I/O stops working at 256 frames and above
    ///
    /// On a Bela Gem Stereo, a period of 256 frames or more leaves the
    /// digital pins dead: nothing written reaches a pin and nothing
    /// driven into one is read, while initialisation succeeds, the
    /// audio runs and no warning is printed. The PRU's digital buffer
    /// is 256 words and libbela does not check the period against it.
    /// The largest period measured to work is 255; see "What a digital
    /// pin does" in `docs/board-facts.md`.
    ///
    /// Nothing here rejects such a period, because it is only the
    /// digital domain that is affected and a program that never touches
    /// a pin is unharmed by it.
    #[must_use]
    pub const fn period_size(mut self, frames: u32) -> Self {
        self.period_size = Some(frames);
        self
    }

    /// Whether to use the analog input and output.
    #[must_use]
    pub const fn use_analog(mut self, enabled: bool) -> Self {
        self.use_analog = Some(enabled);
        self
    }

    /// Whether to use the programmable GPIOs.
    #[must_use]
    pub const fn use_digital(mut self, enabled: bool) -> Self {
        self.use_digital = Some(enabled);
        self
    }

    /// How many analog input channels to use.
    #[must_use]
    pub const fn num_analog_in_channels(mut self, channels: u32) -> Self {
        self.num_analog_in_channels = Some(channels);
        self
    }

    /// How many analog output channels to use.
    ///
    /// # It has to match the inputs, and a Gem Stereo gives none
    ///
    /// libbela refuses a different number of analog inputs and outputs
    /// — `Bela_initAudio` prints `TODO: a different number of channels
    /// for inputs and outputs is not yet supported` and fails — so this
    /// is only usable together with
    /// [`num_analog_in_channels`](Settings::num_analog_in_channels) set
    /// to the same number. A mismatch costs more than the error says:
    /// a failed initialisation leaves the process unable to build
    /// another audio system (see [`Bela::new`](crate::Bela::new)).
    ///
    /// A Bela Gem Stereo then reports 0 analog output channels
    /// whatever was asked for, because it has none. Measured on the
    /// board; see `docs/board-facts.md`.
    #[must_use]
    pub const fn num_analog_out_channels(mut self, channels: u32) -> Self {
        self.num_analog_out_channels = Some(channels);
        self
    }

    /// How many digital (GPIO) channels to use.
    #[must_use]
    pub const fn num_digital_channels(mut self, channels: u32) -> Self {
        self.num_digital_channels = Some(channels);
        self
    }

    /// Whether to detect and log underruns.
    #[must_use]
    pub const fn detect_underruns(mut self, enabled: bool) -> Self {
        self.detect_underruns = Some(enabled);
        self
    }

    /// Whether to use verbose logging.
    #[must_use]
    pub const fn verbose(mut self, enabled: bool) -> Self {
        self.verbose = Some(enabled);
        self
    }

    /// Whether to give more CPU to the audio task. The Linux side of
    /// the board may freeze while the program is running.
    #[must_use]
    pub const fn high_performance_mode(mut self, enabled: bool) -> Self {
        self.high_performance_mode = Some(enabled);
        self
    }

    /// Whether analog channels should be resampled to the audio sample
    /// rate. Enabled by default on Bela Gem.
    ///
    /// What it removes is a frame count that follows the analog
    /// channel count rather than the block. Measured on a Gem Stereo
    /// with 16-frame audio blocks at 44100 Hz: with it off, 8 analog
    /// input channels give 8 analog frames at 22050 Hz, 4 give 16 at
    /// 44100 Hz and 2 give 32 at 88200 Hz. With it on, every one of
    /// them gives 16 frames at 44100 Hz — the audio block's own —
    /// which is what lets one loop over
    /// [`audio_frames`](crate::BlockContext::audio_frames) read analog
    /// inputs as it goes. See `docs/board-facts.md`.
    #[must_use]
    pub const fn uniform_sample_rate(mut self, enabled: bool) -> Self {
        self.uniform_sample_rate = Some(enabled);
        self
    }

    /// GPIO pin monitored for stopping the program; pass `-1` to
    /// disable monitoring.
    #[must_use]
    pub const fn stop_button_pin(mut self, pin: i32) -> Self {
        self.stop_button_pin = Some(pin);
        self
    }

    /// Number of threads used for `render` (multithreaded rendering on
    /// the quad-core Bela Gem).
    ///
    /// libbela creates `threads - 1` extra real-time threads and calls
    /// [`render`](crate::BelaApplication::render) on all of them at
    /// once, for the same block, over the same buffers. It partitions
    /// nothing itself; the crate does, handing each thread its own
    /// [`RenderState`](crate::BelaApplication::RenderState) and its own
    /// share of the output frames. See
    /// `docs/multithreaded-rendering.md`.
    ///
    /// More threads than the board has cores buys nothing: they render
    /// the same block and every one of them has to finish before it
    /// can be handed over. A Bela Gem has four.
    ///
    /// # It has to be the number libbela then renders on
    ///
    /// The render states are built from this value, resolved against
    /// Bela's defaults and the command line, before `Bela_initAudio` is
    /// called — so a libbela that went on to render on a different
    /// number of threads would leave some of them without a state, and
    /// the frame ranges would no longer tile the block.
    ///
    /// [`Bela::new`](crate::Bela::new) refuses that rather than
    /// rendering it: the `setup` callback checks the count the context
    /// reports and aborts if it disagrees. That fails the
    /// initialisation with [`Error::Init`](crate::Error::Init) — which,
    /// as `Bela::new` documents, is fatal to the *process*, so every
    /// later `Bela::new` in it returns
    /// [`Error::AudioSystemPoisoned`](crate::Error::AudioSystemPoisoned).
    ///
    /// The Bela this crate is pinned to copies `threadCount` through
    /// unchanged, so the disagreement has not been seen; the check is
    /// there because a future one might not.
    #[must_use]
    pub const fn thread_count(mut self, threads: u32) -> Self {
        self.thread_count = Some(threads);
        self
    }

    /// Measures how much of each block the audio thread uses,
    /// averaging over `measurements_per_cycle` blocks.
    ///
    /// [`BlockContext::cpu_usage`](crate::BlockContext::cpu_usage) reads
    /// the
    /// result; without this it returns `None`. The cycle length trades
    /// responsiveness against noise: at 44.1 kHz and 16 frames per
    /// block, a block is about 0.36 ms, so 2000 blocks is a reading
    /// roughly every 0.7 s.
    ///
    /// # Why it is a setting
    ///
    /// Turning monitoring on resets counters the audio thread owns, and
    /// libbela decides whether to measure at all when that thread
    /// starts. Both make this something to say before audio exists, so
    /// it is applied by [`Bela::new`](crate::Bela::new) — which is also
    /// what keeps it out of reach of code that could race with a
    /// running audio thread.
    ///
    /// Note that this one is *not* applied by
    /// [`apply_to`](Settings::apply_to): it is a separate C call rather
    /// than a field of `BelaInitSettings`.
    #[must_use]
    pub const fn cpu_monitoring(mut self, measurements_per_cycle: NonZeroU32) -> Self {
        self.cpu_monitoring = Some(measurements_per_cycle);
        self
    }

    /// Whether the speaker amplifiers come up muted.
    ///
    /// The one level control that has to be a setting: [`Bela::start`]
    /// unmutes the amplifiers unless this asked otherwise, so a
    /// [`Bela::mute_speakers`] call before it is undone again. Everything
    /// else the codec can be told — the line out level, the headphone
    /// level and the input gain — is a call on the [`Bela`] handle,
    /// which reaches the hardware in the same state when made before
    /// [`Bela::start`].
    ///
    /// A Bela Gem Stereo has no amplifier mute pin, so this has no
    /// effect there; see [`Bela::mute_speakers`].
    ///
    /// [`Bela`]: crate::Bela
    /// [`Bela::start`]: crate::Bela::start
    /// [`Bela::mute_speakers`]: crate::Bela::mute_speakers
    #[must_use]
    pub const fn begin_muted(mut self, muted: bool) -> Self {
        self.begin_muted = Some(muted);
        self
    }

    /// The requested acquisition cycle for the audio thread, if any.
    #[cfg_attr(
        not(bela_device),
        allow(
            dead_code,
            reason = "only the device-gated audio system applies it; still unit-tested on the host"
        )
    )]
    pub(crate) const fn cpu_monitoring_cycle(&self) -> Option<NonZeroU32> {
        self.cpu_monitoring
    }

    /// Applies the overrides to a raw `BelaInitSettings`, leaving unset
    /// fields untouched.
    ///
    /// This is the escape hatch for driving `Bela_initAudio` manually;
    /// normally the audio system applies it for you on top of
    /// `Bela_defaultSettings()`.
    pub fn apply_to(&self, raw: &mut BelaInitSettings) {
        if let Some(v) = self.period_size {
            raw.periodSize = to_c_int(v);
        }
        if let Some(v) = self.use_analog {
            raw.useAnalog = c_int::from(v);
        }
        if let Some(v) = self.use_digital {
            raw.useDigital = c_int::from(v);
        }
        if let Some(v) = self.num_analog_in_channels {
            raw.numAnalogInChannels = to_c_int(v);
        }
        if let Some(v) = self.num_analog_out_channels {
            raw.numAnalogOutChannels = to_c_int(v);
        }
        if let Some(v) = self.num_digital_channels {
            raw.numDigitalChannels = to_c_int(v);
        }
        if let Some(v) = self.detect_underruns {
            raw.detectUnderruns = c_int::from(v);
        }
        if let Some(v) = self.verbose {
            raw.verbose = c_int::from(v);
        }
        if let Some(v) = self.high_performance_mode {
            raw.highPerformanceMode = c_int::from(v);
        }
        if let Some(v) = self.uniform_sample_rate {
            raw.uniformSampleRate = c_int::from(v);
        }
        if let Some(v) = self.stop_button_pin {
            raw.stopButtonPin = v;
        }
        if let Some(v) = self.thread_count {
            raw.threadCount = v;
        }
        if let Some(v) = self.begin_muted {
            raw.beginMuted = c_int::from(v);
        }
    }
}

// Settings values far exceed c_int::MAX in no realistic configuration;
// saturate instead of wrapping if one ever does.
fn to_c_int(value: u32) -> c_int {
    c_int::try_from(value).unwrap_or(c_int::MAX)
}

/// How many analog input channels the Multiplexer Capelet needs, which
/// is all of them: `Error: multiplexer capelet can only be used with 8
/// analog channels`.
const MULTIPLEXER_ANALOG_CHANNELS: c_int = 8;

/// Checks resolved settings for the combinations libbela accepts here
/// and refuses — or cannot survive — later.
///
/// "Resolved" is the whole point of where this is called from: Bela's
/// defaults, [`Settings`] and the command line have all had their say,
/// so a program that asks for one half of a bad combination in its
/// [`Settings`] and the other half on the command line is refused the
/// same way as one that asks for both in either place.
///
/// Each check replaces a failure that has already been measured on the
/// board and costs the caller more than an error (see "The Multiplexer
/// Capelet" and "Command-line options" in `docs/board-facts.md`):
///
/// - five of them fail inside `Bela_initAudio`: a sample rate of 0 and
///   the two PRU rules in its own sanity checks, and the two
///   multiplexer counts in the `BelaContextManager::setup` it calls.
///   What that costs is not the attempt but the process, which can
///   build no audio system afterwards — see
///   [`Bela::new`](crate::Bela::new);
/// - the sixth, the multiplexer with the analog inputs off, is checked
///   nowhere: libbela's count rules sit behind an `if` that analog
///   being off skips, so the settings reach the PRU firmware, which
///   gives up and ends the process from inside libbela with nothing
///   returned to the caller at all.
///
/// Nothing valid is refused here. Multiplexer channel counts of 2, 4
/// and 8 come up and run on a Gem, and stay accepted although the
/// buffer they fill has no accessor in this crate; PRU 0 is a valid
/// setting of its own, and only the multiplexer requires PRU 1.
#[cfg_attr(
    not(bela_device),
    allow(
        dead_code,
        reason = "only the device-gated audio system initialises; still unit-tested on the host"
    )
)]
pub(crate) const fn check_resolved(raw: &BelaInitSettings) -> Result<(), Error> {
    if raw.audioSampleRate == 0.0 {
        return Err(Error::SampleRate);
    }
    if raw.pruNumber < 0 || raw.pruNumber > 1 {
        return Err(Error::PruNumber(raw.pruNumber));
    }
    // Everything below is about a multiplexer that was asked for. Off
    // is the default and says nothing about the rest of the settings:
    // with no multiplexer, PRU 0 and the analog inputs disabled are
    // both ordinary configurations.
    if raw.numMuxChannels == 0 {
        return Ok(());
    }
    if !matches!(raw.numMuxChannels, 2 | 4 | 8) {
        return Err(Error::MultiplexerChannels(raw.numMuxChannels));
    }
    if raw.pruNumber != 1 {
        return Err(Error::MultiplexerPru(raw.pruNumber));
    }
    if raw.useAnalog == 0 {
        return Err(Error::MultiplexerWithoutAnalog);
    }
    if raw.numAnalogInChannels != MULTIPLEXER_ANALOG_CHANNELS {
        return Err(Error::MultiplexerAnalogChannels(raw.numAnalogInChannels));
    }
    Ok(())
}

/// How many threads `render` will be called on, once the settings are
/// resolved against Bela's defaults.
///
/// At least 1: libbela passes `threadCount` through unchanged and only
/// creates *extra* threads above 1, so 0 and 1 both mean the one thread
/// that always renders. This is what the audio system sizes the render
/// states from, and what
/// [`RenderContext::thread_count`](crate::RenderContext::thread_count)
/// reports back.
#[cfg_attr(
    not(bela_device),
    allow(
        dead_code,
        reason = "only the device-gated audio system applies settings; still unit-tested on the host"
    )
)]
pub(crate) fn render_threads(raw: &BelaInitSettings) -> usize {
    (raw.threadCount as usize).max(1)
}

#[cfg(test)]
mod tests {
    use core::mem;

    use super::*;

    // Stands in for the output of `Bela_defaultSettings()`, which needs
    // libbela and therefore the board. The fields are the ones
    // `Bela_defaultSettings` sets, so what is built on top of this is
    // what the checks below would see on a board — a zeroed structure
    // would fail them for a sample rate and a PRU number nobody asked
    // for.
    fn fake_defaults() -> BelaInitSettings {
        let mut raw: BelaInitSettings = unsafe { mem::zeroed() };
        raw.audioSampleRate = 44100.0;
        raw.periodSize = 16;
        raw.useAnalog = 1;
        raw.numAnalogInChannels = 8;
        raw.numAnalogOutChannels = 8;
        raw.numMuxChannels = 0;
        raw.pruNumber = 1;
        raw.uniformSampleRate = 1;
        raw.stopButtonPin = 115;
        raw.verbose = 0;
        raw
    }

    // A resolved configuration with the multiplexer on, which is the
    // one every rule about it applies to.
    fn with_multiplexer(channels: c_int) -> BelaInitSettings {
        let mut raw = fake_defaults();
        raw.numMuxChannels = channels;
        raw
    }

    #[test]
    fn a_whole_configuration_can_be_a_const() {
        // The point of `new` being const: this is evaluated at compile
        // time, so a builder method that stopped being one would fail
        // to compile here rather than fail an assertion.
        const SETTINGS: Settings = Settings::new().period_size(64).thread_count(4);

        let mut raw = fake_defaults();
        SETTINGS.apply_to(&mut raw);

        assert_eq!(raw.periodSize, 64);
        assert_eq!(raw.threadCount, 4);
    }

    #[test]
    fn default_is_the_same_empty_set_of_overrides_as_new() {
        assert_eq!(Settings::default(), Settings::new());
    }

    #[test]
    fn empty_settings_leave_defaults_untouched() {
        let mut raw = fake_defaults();
        Settings::new().apply_to(&mut raw);

        assert_eq!(raw.periodSize, 16);
        assert_eq!(raw.useAnalog, 1);
        assert_eq!(raw.uniformSampleRate, 1);
        assert_eq!(raw.stopButtonPin, 115);
    }

    #[test]
    fn set_fields_override_defaults() {
        let mut raw = fake_defaults();
        Settings::new()
            .period_size(64)
            .use_analog(false)
            .verbose(true)
            .stop_button_pin(-1)
            .thread_count(4)
            .apply_to(&mut raw);

        assert_eq!(raw.periodSize, 64);
        assert_eq!(raw.useAnalog, 0);
        assert_eq!(raw.verbose, 1);
        assert_eq!(raw.stopButtonPin, -1);
        assert_eq!(raw.threadCount, 4);
        // Untouched by the overrides above.
        assert_eq!(raw.uniformSampleRate, 1);
    }

    #[test]
    fn bools_map_to_c_ints() {
        let mut raw = fake_defaults();
        Settings::new()
            .use_digital(true)
            .detect_underruns(false)
            .high_performance_mode(true)
            .uniform_sample_rate(false)
            .begin_muted(true)
            .apply_to(&mut raw);

        assert_eq!(raw.useDigital, 1);
        assert_eq!(raw.detectUnderruns, 0);
        assert_eq!(raw.highPerformanceMode, 1);
        assert_eq!(raw.uniformSampleRate, 0);
        assert_eq!(raw.beginMuted, 1);
    }

    #[test]
    fn coming_up_unmuted_is_still_said_out_loud() {
        // Bela's default, but an application that says so should not
        // depend on a board's configured `CL=` line agreeing.
        let mut raw = fake_defaults();
        raw.beginMuted = 1;

        Settings::new().begin_muted(false).apply_to(&mut raw);

        assert_eq!(raw.beginMuted, 0);
    }

    #[test]
    fn cpu_monitoring_is_recorded_but_not_an_init_setting() {
        let cycle = NonZeroU32::new(2000).expect("2000 is not zero");
        let settings = Settings::new().cpu_monitoring(cycle);
        assert_eq!(settings.cpu_monitoring_cycle(), Some(cycle));
        assert_eq!(
            Settings::new().cpu_monitoring_cycle(),
            None,
            "monitoring should be off unless it was asked for"
        );

        // It is a separate C call, so it must leave BelaInitSettings
        // alone; the audio system applies it itself.
        let mut raw = fake_defaults();
        let untouched = fake_defaults();
        settings.apply_to(&mut raw);
        assert_eq!(raw.periodSize, untouched.periodSize);
        assert_eq!(raw.useAnalog, untouched.useAnalog);
        assert_eq!(raw.uniformSampleRate, untouched.uniformSampleRate);
        assert_eq!(raw.stopButtonPin, untouched.stopButtonPin);
    }

    #[test]
    fn both_spellings_of_one_render_thread_count_as_one() {
        // libbela creates extra threads only above 1, so a threadCount
        // of 0 renders on the one thread that always exists.
        for spelling in [0, 1] {
            let mut raw: BelaInitSettings = unsafe { mem::zeroed() };
            raw.threadCount = spelling;

            assert_eq!(render_threads(&raw), 1, "threadCount {spelling}");
        }
    }

    #[test]
    fn extra_render_threads_are_counted_as_asked_for() {
        let mut raw: BelaInitSettings = unsafe { mem::zeroed() };
        raw.threadCount = 4;

        assert_eq!(render_threads(&raw), 4);
    }

    #[test]
    fn belas_own_defaults_are_accepted() {
        assert_eq!(check_resolved(&fake_defaults()), Ok(()));
    }

    #[test]
    fn a_sample_rate_of_zero_is_refused() {
        // What `-r abc` resolves to, `atof` giving 0, and what `-r -5`
        // is clamped to. libbela fails initialisation for it with a
        // message about a cape.
        let mut raw = fake_defaults();
        raw.audioSampleRate = 0.0;

        assert_eq!(check_resolved(&raw), Err(Error::SampleRate));
    }

    #[test]
    fn both_prus_are_accepted() {
        // libbela runs the audio code on either; only the multiplexer
        // needs PRU 1, which is a rule of its own.
        for number in [0, 1] {
            let mut raw = fake_defaults();
            raw.pruNumber = number;

            assert_eq!(check_resolved(&raw), Ok(()), "PRU {number}");
        }
    }

    #[test]
    fn a_pru_the_board_does_not_have_is_refused() {
        for number in [-1, 2, 5] {
            let mut raw = fake_defaults();
            raw.pruNumber = number;

            assert_eq!(
                check_resolved(&raw),
                Err(Error::PruNumber(number)),
                "PRU {number}"
            );
        }
    }

    #[test]
    fn the_multiplexer_channel_counts_libbela_takes_are_accepted() {
        // 0 is off; the rest come up and run on a Gem, and stay
        // accepted although this crate has no accessor for the buffer
        // they fill.
        for channels in [0, 2, 4, 8] {
            assert_eq!(
                check_resolved(&with_multiplexer(channels)),
                Ok(()),
                "{channels} multiplexer channels"
            );
        }
    }

    #[test]
    fn any_other_multiplexer_channel_count_is_refused() {
        for channels in [-1, 1, 3, 16] {
            assert_eq!(
                check_resolved(&with_multiplexer(channels)),
                Err(Error::MultiplexerChannels(channels)),
                "{channels} multiplexer channels"
            );
        }
    }

    #[test]
    fn the_multiplexer_is_refused_on_the_wrong_pru() {
        let mut raw = with_multiplexer(8);
        raw.pruNumber = 0;

        assert_eq!(check_resolved(&raw), Err(Error::MultiplexerPru(0)));
    }

    #[test]
    fn the_multiplexer_is_refused_without_the_analog_inputs() {
        // The combination nothing on the ARM side objects to: without
        // this check the PRU firmware ends the process.
        let mut raw = with_multiplexer(8);
        raw.useAnalog = 0;

        assert_eq!(check_resolved(&raw), Err(Error::MultiplexerWithoutAnalog));
    }

    #[test]
    fn the_multiplexer_is_refused_with_any_other_number_of_analog_inputs() {
        // 2 and 4 are what `-C 2` and `-C 4` resolve to; `-C 100` snaps
        // to 8 and is accepted, so this is the resolved count rather
        // than what was written on the command line. 16 is not
        // reachable from the command line at all and is from
        // `Settings`, which is passed on as it stands — the rule is
        // "other than 8", not "fewer than 8".
        for channels in [2, 4, 16] {
            let mut raw = with_multiplexer(8);
            raw.numAnalogInChannels = channels;

            assert_eq!(
                check_resolved(&raw),
                Err(Error::MultiplexerAnalogChannels(channels)),
                "{channels} analog input channels"
            );
        }
    }

    #[test]
    fn the_multiplexer_rules_only_apply_when_it_is_on() {
        // PRU 0 and no analog inputs are an ordinary configuration
        // until a multiplexer is asked for.
        let mut raw = fake_defaults();
        raw.pruNumber = 0;
        raw.useAnalog = 0;
        raw.numAnalogInChannels = 2;

        assert_eq!(check_resolved(&raw), Ok(()));
    }

    #[test]
    fn a_settings_and_a_command_line_half_make_the_same_refusal() {
        // The reason this is checked on the resolved settings: the
        // multiplexer from one place and four analog inputs from the
        // other is the same mistake as both from either.
        let mut from_settings = fake_defaults();
        Settings::new()
            .num_analog_in_channels(4)
            .apply_to(&mut from_settings);
        from_settings.numMuxChannels = 8;

        assert_eq!(
            check_resolved(&from_settings),
            Err(Error::MultiplexerAnalogChannels(4))
        );
    }
}
