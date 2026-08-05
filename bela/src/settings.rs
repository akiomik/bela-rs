use core::ffi::c_int;
use core::num::NonZeroU32;

use bela_sys::BelaInitSettings;

/// Overrides applied on top of Bela's default initialisation settings.
///
/// Unset fields keep the values produced by `Bela_defaultSettings()` on
/// the device, so this type never has to replicate the C-side defaults.
///
/// ```
/// use bela::Settings;
///
/// let settings = Settings::new().period_size(64).use_analog(true);
/// # let _ = settings;
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
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

impl Settings {
    /// Creates an empty set of overrides.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of audio frames per period ("block size").
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
    /// Note that on Bela Gem the analog outputs are part of the audio
    /// outputs.
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
    #[must_use]
    pub const fn thread_count(mut self, threads: u32) -> Self {
        self.thread_count = Some(threads);
        self
    }

    /// Measures how much of each block the audio thread uses,
    /// averaging over `measurements_per_cycle` blocks.
    ///
    /// [`Context::cpu_usage`](crate::Context::cpu_usage) reads the
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
pub fn render_threads(raw: &BelaInitSettings) -> usize {
    (raw.threadCount as usize).max(1)
}

#[cfg(test)]
mod tests {
    use core::mem;

    use super::*;

    // Stands in for the output of `Bela_defaultSettings()`, which needs
    // libbela and therefore the board.
    fn fake_defaults() -> BelaInitSettings {
        let mut raw: BelaInitSettings = unsafe { mem::zeroed() };
        raw.periodSize = 16;
        raw.useAnalog = 1;
        raw.uniformSampleRate = 1;
        raw.stopButtonPin = 115;
        raw.verbose = 0;
        raw
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
}
