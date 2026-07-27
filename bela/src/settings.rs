use core::ffi::c_int;

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
}

impl Settings {
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
    }
}

// Settings values far exceed c_int::MAX in no realistic configuration;
// saturate instead of wrapping if one ever does.
fn to_c_int(value: u32) -> c_int {
    c_int::try_from(value).unwrap_or(c_int::MAX)
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
            .apply_to(&mut raw);

        assert_eq!(raw.periodSize, 64);
        assert_eq!(raw.useAnalog, 0);
        assert_eq!(raw.verbose, 1);
        assert_eq!(raw.stopButtonPin, -1);
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
            .apply_to(&mut raw);

        assert_eq!(raw.useDigital, 1);
        assert_eq!(raw.detectUnderruns, 0);
        assert_eq!(raw.highPerformanceMode, 1);
        assert_eq!(raw.uniformSampleRate, 0);
    }
}
