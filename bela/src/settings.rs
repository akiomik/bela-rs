use core::ffi::c_int;
use core::fmt;
use core::num::NonZeroU32;

use bela_sys::BelaInitSettings;

use crate::application::BelaApplication;
use crate::cpu;
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
    audio_sample_rate: Option<NonZeroU32>,
    use_analog: Option<bool>,
    use_digital: Option<bool>,
    num_analog_in_channels: Option<u32>,
    num_analog_out_channels: Option<u32>,
    num_digital_channels: Option<u32>,
    detect_underruns: Option<bool>,
    verbose: Option<bool>,
    high_performance_mode: Option<bool>,
    uniform_sample_rate: Option<bool>,
    stop_button_pin: Option<StopButtonPin>,
    thread_count: Option<NonZeroU32>,
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
            audio_sample_rate: None,
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
    /// # The context FIFO changes digital output persistence
    ///
    /// On a Bela Gem Stereo, a period of 256 frames or more moves the
    /// application callback behind libbela's context FIFO. A digital
    /// output configured once and written only when its value changes
    /// then stops driving the pin, while initialisation and audio still
    /// succeed without a warning. Re-applying its direction and current
    /// value in every application block restores the output loopback at
    /// 256 and 320 frames; `examples/io_digital --repeat` is the probe
    /// for that workaround. It does not independently establish whether
    /// FIFO-mode input sampling works. See "What a digital pin does" in
    /// `docs/board-facts.md` and [#89](https://github.com/akiomik/bela-rs/issues/89).
    ///
    /// Nothing here rejects such a period, because it is only the
    /// digital domain that is affected and a program that never touches
    /// a pin is unharmed by it.
    #[must_use]
    pub const fn period_size(mut self, frames: u32) -> Self {
        self.period_size = Some(frames);
        self
    }

    /// The audio sample rate, in Hz.
    ///
    /// `NonZeroU32` rather than the `f32` the C field is: zero is the
    /// one value this crate's own resolved-settings check refuses, so
    /// keeping it out of the type keeps that failure out of reach from
    /// this builder, and an integer matches how the rest of this crate
    /// spells a hardware quantity — [`thread_count`](Settings::thread_count) and
    /// [`stop_button_pin`](Settings::stop_button_pin) narrow the same
    /// way (C-CUSTOM-TYPE). What it cannot express is the C field's own
    /// range: a negative rate, which only the command line can produce
    /// and which resolves to 0 the same way a NaN from it does.
    ///
    /// # Nothing here checks what the hardware accepts
    ///
    /// A Bela Gem Stereo has been measured running at the rates
    /// `docs/board-facts.md` lists between 8000 Hz and 106000 Hz, with
    /// the analog and digital rates following when
    /// [`uniform_sample_rate`](Settings::uniform_sample_rate) is on,
    /// and aborting the process from inside the codec at every rate
    /// tried from 108000 Hz up. Those are discrete points, not a swept
    /// range: 106000 and 108000 are the closest pair measured on either
    /// side of the ceiling, the rates between them were never tried,
    /// and neither were most of the rates between the ones in the
    /// lower list — so a rate this method accepts because it looks
    /// close to a known-good one, such as 105000 Hz, is untested and
    /// can still end the process on `SIGABRT` with nothing returned to
    /// the caller, the same failure shape `--json-string {` has. That
    /// ceiling is one board's, not a portable libbela contract, which
    /// is why nothing here compiles it in as a check.
    ///
    /// # The command line still wins
    ///
    /// [`Bela::new_with_args`](crate::Bela::new_with_args) applies
    /// `--sample-rate` after this builder's overrides, the same order
    /// [`period_size`](Settings::period_size) loses to `--period` in —
    /// so a rate set here is only the value a program starts with, not
    /// one it is guaranteed to run under.
    #[must_use]
    pub const fn audio_sample_rate(mut self, hz: NonZeroU32) -> Self {
        self.audio_sample_rate = Some(hz);
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
    /// with 16-frame audio blocks, against whatever
    /// [`audio_sample_rate`](Settings::audio_sample_rate) resolved to:
    /// with it off, 8 analog input channels give 8 analog frames per
    /// block at half the audio rate, 4 give 16 frames at the audio rate
    /// itself and 2 give 32 frames at twice the audio rate. With it on,
    /// every one of them gives 16 frames at the audio rate — the audio
    /// block's own — which is what lets one loop over
    /// [`audio_frames`](crate::BlockContext::audio_frames) read analog
    /// inputs as it goes. See `docs/board-facts.md`.
    #[must_use]
    pub const fn uniform_sample_rate(mut self, enabled: bool) -> Self {
        self.uniform_sample_rate = Some(enabled);
        self
    }

    /// GPIO pin monitored for stopping the program.
    ///
    /// Pass `None` to disable monitoring. An unset setting leaves the
    /// board's default alone.
    #[must_use]
    pub const fn stop_button_pin(mut self, pin: Option<u32>) -> Self {
        self.stop_button_pin = Some(match pin {
            Some(pin) => StopButtonPin::Gpio(pin),
            None => StopButtonPin::Disabled,
        });
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
    /// The count cannot be zero: libbela treats 0 and 1 as two
    /// spellings of the same single render thread, while this API keeps
    /// one spelling for one configuration.
    ///
    /// # An application can insist on the count it gets
    ///
    /// Unset, the count is whatever `Bela_defaultSettings()` produced,
    /// which includes whatever a `Bela_userSettings()` hook the program
    /// links did to it. So an application that only works on a
    /// particular number of threads is better saying so than assuming
    /// it: [`validate_settings`](crate::BelaApplication::validate_settings)
    /// is given the resolved count as
    /// [`ResolvedSettings::thread_count`], before any of it has been
    /// acted on, and refusing there is an ordinary
    /// [`Error::SettingsRefused`](crate::Error::SettingsRefused).
    ///
    /// Bela's standard command-line options cannot change it: the
    /// version this crate is pinned to has none for the thread count.
    ///
    /// # It has to be the number libbela then renders on
    ///
    /// The render states are built from the resolved `threadCount`
    /// before `Bela_initAudio` is called — so a libbela that went on to
    /// render on a different number of threads would leave some of them
    /// without a state, and the frame ranges would no longer tile the
    /// block.
    ///
    /// This is a check on libbela rather than on the configuration,
    /// which is why it stays where it is: the count the settings
    /// resolved to has already been agreed with the application by
    /// then, and what is left to catch is a `BelaContext` that reports
    /// something else.
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
    pub const fn thread_count(mut self, threads: NonZeroU32) -> Self {
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
        if let Some(v) = self.audio_sample_rate {
            raw.audioSampleRate = to_c_float(v);
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
        if let Some(stop_button_pin) = self.stop_button_pin {
            raw.stopButtonPin = match stop_button_pin {
                StopButtonPin::Gpio(pin) => to_c_int(pin),
                StopButtonPin::Disabled => -1,
            };
        }
        if let Some(v) = self.thread_count {
            raw.threadCount = v.get();
        }
        if let Some(v) = self.begin_muted {
            raw.beginMuted = c_int::from(v);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum StopButtonPin {
    Gpio(u32),
    Disabled,
}

// Settings values far exceed c_int::MAX in no realistic configuration;
// saturate instead of wrapping if one ever does.
fn to_c_int(value: u32) -> c_int {
    c_int::try_from(value).unwrap_or(c_int::MAX)
}

// The measured rate ladder (docs/board-facts.md) tops out in the
// hundreds of thousands, far inside an f32's 24-bit exact integer
// range, so this loses no precision for any rate that has been seen to
// run.
#[allow(
    clippy::cast_precision_loss,
    reason = "audioSampleRate is a C float; requested rates stay far below where u32 -> f32 loses \
              precision"
)]
const fn to_c_float(value: NonZeroU32) -> f32 {
    value.get() as f32
}

/// The settings an audio system is about to be built with, as
/// [`validate_settings`](BelaApplication::validate_settings) sees them.
///
/// A borrowed view of the `BelaInitSettings` this crate holds at the
/// point where every layer has had its say: `Bela_defaultSettings()`
/// first — which on a board has already applied the `CL=` line from
/// `~/.bela/belaconfig` and whatever a `Bela_userSettings()` hook did
/// — then [`Settings`], then Bela's standard command-line options
/// where [`Bela::new_with_args`](crate::Bela::new_with_args) was given
/// any. Nothing writes to those settings afterwards, so what an
/// accessor here reports is what `Bela_initAudio` will be called with.
///
/// # What was asked for, not what the board will give
///
/// This is the request. What the hardware makes of it is
/// [`SetupContext`](crate::SetupContext), and there is no way to have
/// that any earlier: it does not exist until `Bela_initAudio` has run,
/// which is the call this view exists to be consulted before.
///
/// The two differ wherever libbela reshapes a request rather than
/// refusing it, and it does so often — `--analog-channels` snaps to 8,
/// 4 or 2, `--digital-channels` clamps to 16, and a Bela Gem Stereo
/// reports 0 analog output channels however many were asked for,
/// because it has none (`docs/board-facts.md`). So a check written
/// here answers "were these settings asked for", never "is this what
/// the codec gave".
///
/// That is not a gap this crate can close. Asking the board and then
/// declining means declining from
/// [`setup`](BelaApplication::setup), which runs inside
/// `Bela_initAudio` with the audio hardware already up, and a
/// refusal from there fails the initialisation and leaves the process
/// unable to build another audio system — see
/// [`Bela::new`](crate::Bela::new). Refusing the request costs
/// nothing; refusing the result costs the process.
///
/// # More than the C structure
///
/// [`Settings::cpu_monitoring`] is not a `BelaInitSettings` field —
/// it is a separate C call the audio system makes just before
/// `Bela_initAudio` — so it is carried here alongside the structure
/// and reported by [`cpu_monitoring`](ResolvedSettings::cpu_monitoring).
/// Without it this view would be missing the one setting that decides
/// whether [`BlockContext::cpu_usage`](crate::BlockContext::cpu_usage)
/// answers at all, and an application built around that reading would
/// have nothing to check.
///
/// # What is not here
///
/// `numAudioInChannels` and `numAudioOutChannels`, which `Bela.h`
/// marks `[ignored]`. Neither [`Settings`] nor any standard
/// command-line option writes them, and libbela does not read them, so
/// a comparison against one would be a comparison against a constant
/// that says nothing about the audio system being built. How many
/// audio channels there are is
/// [`SetupContext::audio_in_channels`](crate::SetupContext::audio_in_channels)
/// and its output counterpart, after the fact.
///
/// [`as_sys`](ResolvedSettings::as_sys) reaches the whole C structure
/// for anything else, including the fields this crate has no safe
/// vocabulary for.
pub struct ResolvedSettings<'a> {
    raw: &'a BelaInitSettings,
    cpu_monitoring: Option<NonZeroU32>,
}

impl<'a> ResolvedSettings<'a> {
    /// Borrows resolved settings, and the monitoring cycle that goes
    /// with them, as the view an application sees.
    ///
    /// Not public: what makes this a *resolved* configuration is where
    /// the audio system calls it from, and a view built anywhere else
    /// would carry the name without the property.
    #[cfg_attr(
        not(bela_device),
        allow(
            dead_code,
            reason = "only the device-gated audio system resolves settings; still unit-tested on \
                      the host"
        )
    )]
    pub(crate) const fn new(raw: &'a BelaInitSettings, cpu_monitoring: Option<NonZeroU32>) -> Self {
        Self {
            raw,
            cpu_monitoring,
        }
    }

    /// Read access to the underlying `BelaInitSettings`.
    ///
    /// Everything except [`cpu_monitoring`](ResolvedSettings::cpu_monitoring),
    /// which libbela keeps nowhere in this structure.
    #[must_use]
    #[inline]
    pub const fn as_sys(&self) -> &BelaInitSettings {
        self.raw
    }

    /// Requested number of audio frames per period ("block size").
    ///
    /// A C `int`, and reported as one: the resolved value is whatever
    /// survived Bela's own parser, which reshapes rather than refuses
    /// — `--period 0` arrives here as 1 — and this crate does not
    /// reshape it further. There is no period size to check against
    /// either: 2 runs on a Gem Stereo where 3 does not, and both
    /// failures move as soon as the analog configuration does
    /// (`docs/board-facts.md`).
    #[must_use]
    #[inline]
    pub const fn period_size(&self) -> i32 {
        self.raw.periodSize
    }

    /// Requested audio sample rate in Hz.
    ///
    /// 0 never reaches an application: it is what `--sample-rate`
    /// gives for anything `atof` cannot read, and it is refused with
    /// [`Error::SampleRate`] before this view is shown to anyone.
    #[must_use]
    #[inline]
    pub const fn audio_sample_rate(&self) -> f32 {
        self.raw.audioSampleRate
    }

    /// Whether the analog input and output were asked for.
    #[must_use]
    #[inline]
    pub const fn use_analog(&self) -> bool {
        self.raw.useAnalog != 0
    }

    /// Requested number of analog input channels.
    ///
    /// What the command line asked for after Bela's parser snapped it
    /// to 8, 4 or 2, or what
    /// [`Settings::num_analog_in_channels`] set, which is passed on as
    /// it stands. 0 here is not "no analog inputs" —
    /// [`use_analog`](ResolvedSettings::use_analog) is.
    #[must_use]
    #[inline]
    pub const fn num_analog_in_channels(&self) -> i32 {
        self.raw.numAnalogInChannels
    }

    /// Requested number of analog output channels.
    ///
    /// A Bela Gem Stereo has none, and reports 0 in the context
    /// whatever this says; see
    /// [`Settings::num_analog_out_channels`].
    #[must_use]
    #[inline]
    pub const fn num_analog_out_channels(&self) -> i32 {
        self.raw.numAnalogOutChannels
    }

    /// Whether the programmable GPIOs were asked for.
    #[must_use]
    #[inline]
    pub const fn use_digital(&self) -> bool {
        self.raw.useDigital != 0
    }

    /// Requested number of digital (GPIO) channels.
    #[must_use]
    #[inline]
    pub const fn num_digital_channels(&self) -> i32 {
        self.raw.numDigitalChannels
    }

    /// How many threads `render` will be called on, which is how many
    /// [`RenderState`](BelaApplication::RenderState)s this audio system
    /// will build.
    ///
    /// At least 1, for the same reason
    /// [`RenderContext::thread_count`](crate::RenderContext::thread_count)
    /// is: libbela spells one render thread as either 0 or 1, and this
    /// reports the number of threads that will render.
    ///
    /// The resolved value — Bela's defaults, whatever a
    /// `Bela_userSettings()` hook made of them, then
    /// [`Settings::thread_count`]. Bela's standard command-line options
    /// cannot change it, unlike most of this view; what they can change
    /// is whether the rest of the configuration still suits the number.
    /// So an application that only works on one thread — or only on
    /// four — can say so here, and refusing costs nothing.
    #[must_use]
    #[inline]
    pub const fn thread_count(&self) -> usize {
        render_threads(self.raw)
    }

    /// Whether the analog channels were asked to be resampled to the
    /// audio sample rate; see [`Settings::uniform_sample_rate`].
    #[must_use]
    #[inline]
    pub const fn uniform_sample_rate(&self) -> bool {
        self.raw.uniformSampleRate != 0
    }

    /// Whether high-performance mode was asked for.
    #[must_use]
    #[inline]
    pub const fn high_performance_mode(&self) -> bool {
        self.raw.highPerformanceMode != 0
    }

    /// Whether underrun detection and logging were asked for.
    #[must_use]
    #[inline]
    pub const fn detect_underruns(&self) -> bool {
        self.raw.detectUnderruns != 0
    }

    /// Whether libbela's own verbose logging was asked for.
    #[must_use]
    #[inline]
    pub const fn verbose(&self) -> bool {
        self.raw.verbose != 0
    }

    /// Whether the speaker amplifiers were asked to come up muted.
    #[must_use]
    #[inline]
    pub const fn begin_muted(&self) -> bool {
        self.raw.beginMuted != 0
    }

    /// The CPU monitoring acquisition cycle that was asked for, in
    /// measurements per cycle, or `None` when monitoring is off.
    ///
    /// What [`Settings::cpu_monitoring`] said, and the one thing here
    /// that is not a `BelaInitSettings` field: monitoring is a separate
    /// C call, which the audio system makes immediately after this hook
    /// has accepted the configuration. Nothing else can change it — it
    /// has no command-line option — so unlike the rest of this view it
    /// is the application's own setting read back, and it is here
    /// because an application that needs
    /// [`BlockContext::cpu_usage`](crate::BlockContext::cpu_usage) to
    /// answer has no other way to find out before `setup`.
    ///
    /// A cycle that is here has already passed this crate's checks:
    /// [`Error::CpuMonitoringCycle`] for a length libbela cannot take
    /// and [`Error::CpuMonitoringPeriodSize`] for a period size where
    /// the counters would not describe the thread that renders.
    #[must_use]
    #[inline]
    pub const fn cpu_monitoring(&self) -> Option<NonZeroU32> {
        self.cpu_monitoring
    }

    /// Which GPIO pin is monitored for stopping the program, or `None`
    /// when nothing is.
    ///
    /// The `Option` is libbela's own spelling read back: a negative pin
    /// is how monitoring is turned off, which is what
    /// [`Settings::stop_button_pin`] passes `None` on as. A pin the
    /// board does not have is not refused anywhere and is reported here
    /// as the number it is; such a run carries on without a working
    /// stop button.
    #[must_use]
    #[inline]
    pub const fn stop_button_pin(&self) -> Option<u32> {
        let pin = self.raw.stopButtonPin;
        if pin < 0 {
            None
        } else {
            #[allow(
                clippy::cast_sign_loss,
                reason = "the branch above is what rules the negative values out"
            )]
            Some(pin as u32)
        }
    }
}

/// The accessors, and not the whole C structure: `BelaInitSettings`
/// has a `Debug` of its own, with the callback pointers and the 256
/// bytes of `pruFilename` in it, and [`as_sys`](ResolvedSettings::as_sys)
/// is the way to it.
impl fmt::Debug for ResolvedSettings<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResolvedSettings")
            .field("period_size", &self.period_size())
            .field("audio_sample_rate", &self.audio_sample_rate())
            .field("use_analog", &self.use_analog())
            .field("num_analog_in_channels", &self.num_analog_in_channels())
            .field("num_analog_out_channels", &self.num_analog_out_channels())
            .field("use_digital", &self.use_digital())
            .field("num_digital_channels", &self.num_digital_channels())
            .field("thread_count", &self.thread_count())
            .field("uniform_sample_rate", &self.uniform_sample_rate())
            .field("high_performance_mode", &self.high_performance_mode())
            .field("detect_underruns", &self.detect_underruns())
            .field("verbose", &self.verbose())
            .field("begin_muted", &self.begin_muted())
            .field("cpu_monitoring", &self.cpu_monitoring())
            .field("stop_button_pin", &self.stop_button_pin())
            .finish_non_exhaustive()
    }
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
/// - five of them fail inside `Bela_initAudio`: a sample rate of 0 in
///   `Bela_getHwConfigPrivate`, the two PRU rules in `RTAudio.cpp`'s
///   initial sanity checks, and the multiplexer channel and analog
///   input counts in `PRU::initialise`.
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

/// Everything that is asked about a resolved configuration before
/// `Bela_initAudio` is called, in the order it is asked.
///
/// The crate's own checks first, because they describe libbela rather
/// than any one application: a configuration [`check_resolved`] refuses
/// is one no application could have run under, so reporting it as the
/// application's refusal would name the wrong culprit. The CPU
/// monitoring period size goes with them, being another rule of this
/// crate's own — and it is only asked when monitoring was asked for,
/// since the limit is about the thread the counters measure.
///
/// [`BelaApplication::validate_settings`] comes last, with the same
/// settings and nothing yet done about them: no monitoring counters
/// reset, no render states allocated, no audio hardware brought up. A
/// refusal from there is an ordinary [`Error`] and the process is left
/// as it was.
///
/// `cpu_monitoring` is the cycle [`Settings::cpu_monitoring`] asked
/// for, which is both what decides whether the period size is checked
/// at all and the one part of the configuration the application cannot
/// read off `BelaInitSettings`, so it is passed on to the view.
#[cfg_attr(
    not(bela_device),
    allow(
        dead_code,
        reason = "only the device-gated audio system initialises; still unit-tested on the host"
    )
)]
pub(crate) fn check_supported<T: BelaApplication>(
    raw: &BelaInitSettings,
    cpu_monitoring: Option<NonZeroU32>,
    application: &T,
) -> Result<(), Error> {
    check_resolved(raw)?;
    if cpu_monitoring.is_some() {
        // Needs the resolved period size: unset in `Settings` means
        // Bela's default, not "no period size". The raw value is
        // signed even though Settings only accepts u32; map a negative
        // resolved value to an impossible upper bound so the unsigned
        // range check refuses it.
        let period_size = u32::try_from(raw.periodSize).unwrap_or(u32::MAX);
        cpu::check_period_size(period_size)?;
    }
    application
        .validate_settings(&ResolvedSettings::new(raw, cpu_monitoring))
        .map_err(Error::SettingsRefused)
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
pub(crate) const fn render_threads(raw: &BelaInitSettings) -> usize {
    let count = raw.threadCount as usize;
    if count == 0 { 1 } else { count }
}

#[cfg(test)]
#[allow(
    clippy::float_cmp,
    reason = "the sample rate is copied verbatim, so the expected value is exact"
)]
mod tests {
    use core::mem;

    use super::*;
    use crate::application::ThreadInfo;
    use crate::context::{RenderContext, SetupContext};

    const FOUR_THREADS: NonZeroU32 = NonZeroU32::new(4).expect("the test thread count is non-zero");

    /// What an application refusing too few analog inputs says.
    const NEEDS_EIGHT: &str = "this application reads eight analog inputs";
    /// What an application refusing extra render threads says.
    const NEEDS_ONE_THREAD: &str = "this application renders on one thread";
    /// What an application refusing to run unmonitored says.
    const NEEDS_MONITORING: &str = "this application reports the CPU usage of every block";

    fn cycle_of(measurements: u32) -> NonZeroU32 {
        NonZeroU32::new(measurements).expect("the test cycles are non-zero")
    }

    /// Says nothing about the settings, which is the default hook.
    struct Anything;

    impl BelaApplication for Anything {
        type RenderState = ();

        fn create_render_state(&mut self, _thread: ThreadInfo, _context: &SetupContext) {}

        fn render(&self, _state: &mut (), _context: &mut RenderContext) {}
    }

    /// Will not run with fewer analog inputs than it reads.
    struct NeedsEightAnalogInputs;

    impl BelaApplication for NeedsEightAnalogInputs {
        type RenderState = ();

        fn validate_settings(&self, settings: &ResolvedSettings<'_>) -> Result<(), &'static str> {
            if settings.num_analog_in_channels() < 8 {
                return Err(NEEDS_EIGHT);
            }
            Ok(())
        }

        fn create_render_state(&mut self, _thread: ThreadInfo, _context: &SetupContext) {}

        fn render(&self, _state: &mut (), _context: &mut RenderContext) {}
    }

    /// Reports the CPU usage of every block, so it will not run
    /// without the monitoring that makes the reading exist.
    struct NeedsMonitoring;

    impl BelaApplication for NeedsMonitoring {
        type RenderState = ();

        fn validate_settings(&self, settings: &ResolvedSettings<'_>) -> Result<(), &'static str> {
            if settings.cpu_monitoring().is_some() {
                Ok(())
            } else {
                Err(NEEDS_MONITORING)
            }
        }

        fn create_render_state(&mut self, _thread: ThreadInfo, _context: &SetupContext) {}

        fn render(&self, _state: &mut (), _context: &mut RenderContext) {}
    }

    /// Built for one render thread, whoever asked for more.
    struct SingleThreaded;

    impl BelaApplication for SingleThreaded {
        type RenderState = ();

        fn validate_settings(&self, settings: &ResolvedSettings<'_>) -> Result<(), &'static str> {
            if settings.thread_count() == 1 {
                Ok(())
            } else {
                Err(NEEDS_ONE_THREAD)
            }
        }

        fn create_render_state(&mut self, _thread: ThreadInfo, _context: &SetupContext) {}

        fn render(&self, _state: &mut (), _context: &mut RenderContext) {}
    }

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
        const SETTINGS: Settings = Settings::new().period_size(64).thread_count(FOUR_THREADS);

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
        assert_eq!(raw.audioSampleRate, 44100.0);
        assert_eq!(raw.useAnalog, 1);
        assert_eq!(raw.uniformSampleRate, 1);
        assert_eq!(raw.stopButtonPin, 115);
    }

    #[test]
    fn an_audio_sample_rate_is_written_to_the_c_field() {
        let mut raw = fake_defaults();
        let hz = NonZeroU32::new(48000).expect("48000 is not zero");
        Settings::new().audio_sample_rate(hz).apply_to(&mut raw);

        assert_eq!(raw.audioSampleRate, 48000.0);
    }

    #[test]
    fn the_command_line_overrides_a_configured_sample_rate() {
        // The same order `Bela::new_with_args` applies them in:
        // `settings.apply_to` first, `--sample-rate` parsed on top of
        // it (`system.rs`'s `init`), so a rate set here is only the one
        // the run starts with.
        let mut raw = fake_defaults();
        let hz = NonZeroU32::new(48000).expect("48000 is not zero");
        Settings::new().audio_sample_rate(hz).apply_to(&mut raw);
        assert_eq!(raw.audioSampleRate, 48000.0);

        raw.audioSampleRate = 96000.0;
        assert_eq!(raw.audioSampleRate, 96000.0);
    }

    #[test]
    fn set_fields_override_defaults() {
        let mut raw = fake_defaults();
        Settings::new()
            .period_size(64)
            .use_analog(false)
            .verbose(true)
            .stop_button_pin(None)
            .thread_count(FOUR_THREADS)
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
    fn a_stop_button_pin_is_an_unsigned_gpio_number() {
        let mut raw = fake_defaults();
        Settings::new().stop_button_pin(Some(27)).apply_to(&mut raw);

        assert_eq!(raw.stopButtonPin, 27);

        Settings::new()
            .stop_button_pin(Some(u32::MAX))
            .apply_to(&mut raw);
        assert_eq!(raw.stopButtonPin, c_int::MAX);
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
    fn the_view_reports_the_settings_as_they_resolved() {
        let mut raw = fake_defaults();
        Settings::new()
            .period_size(64)
            .num_analog_in_channels(4)
            .use_digital(false)
            .verbose(true)
            .begin_muted(true)
            .stop_button_pin(Some(27))
            .apply_to(&mut raw);
        // `--sample-rate 48000` on top, applied where the audio system
        // applies the command line, and a thread count from a layer
        // `Settings` cannot reach — Bela's defaults or a
        // `Bela_userSettings()` hook.
        raw.audioSampleRate = 48000.0;
        raw.threadCount = 4;

        let settings = ResolvedSettings::new(&raw, None);

        assert_eq!(settings.period_size(), 64);
        assert_eq!(settings.audio_sample_rate(), 48000.0);
        assert!(settings.use_analog());
        assert_eq!(settings.num_analog_in_channels(), 4);
        assert_eq!(settings.num_analog_out_channels(), 8);
        assert!(!settings.use_digital());
        assert_eq!(settings.thread_count(), 4);
        assert!(settings.uniform_sample_rate());
        assert!(!settings.high_performance_mode());
        assert!(settings.verbose());
        assert!(settings.begin_muted());
        assert_eq!(settings.stop_button_pin(), Some(27));
        // The whole structure is still reachable for what has no
        // accessor here.
        assert_eq!(settings.as_sys().numMuxChannels, 0);
    }

    #[test]
    fn the_view_spells_one_render_thread_and_no_stop_button_the_way_the_rest_of_the_crate_does() {
        // A `threadCount` of 0 is libbela's other spelling of the one
        // thread that always renders, and a negative stop button pin is
        // how monitoring is turned off — the same two conversions
        // `render_threads` and `Settings::stop_button_pin` make.
        let mut raw = fake_defaults();
        raw.threadCount = 0;
        raw.stopButtonPin = -1;

        let settings = ResolvedSettings::new(&raw, None);

        assert_eq!(settings.thread_count(), 1);
        assert_eq!(settings.stop_button_pin(), None);
    }

    #[test]
    fn an_application_that_says_nothing_accepts_every_configuration() {
        // The default hook: a trait implementation from before this
        // existed keeps initialising exactly as it did.
        assert_eq!(
            check_supported(&fake_defaults(), None, &Anything),
            Ok(()),
            "the default validate_settings should accept Bela's own defaults"
        );
    }

    #[test]
    fn an_application_can_refuse_the_resolved_settings() {
        let mut raw = fake_defaults();
        Settings::new().num_analog_in_channels(4).apply_to(&mut raw);

        assert_eq!(
            check_supported(&raw, None, &NeedsEightAnalogInputs),
            Err(Error::SettingsRefused(NEEDS_EIGHT))
        );
        // And accepts the configuration it was built for.
        assert_eq!(
            check_supported(&fake_defaults(), None, &NeedsEightAnalogInputs),
            Ok(())
        );
    }

    #[test]
    fn an_application_can_refuse_a_resolved_thread_count() {
        // The hook is asked about the resolved settings rather than
        // about `Settings`, so a count the application never asked for
        // — from Bela's defaults or a `Bela_userSettings()` hook, which
        // both have their say before `Settings` is applied — is one it
        // can still turn down rather than render wrong.
        let mut raw = fake_defaults();
        raw.threadCount = 4;

        assert_eq!(
            check_supported(&raw, None, &SingleThreaded),
            Err(Error::SettingsRefused(NEEDS_ONE_THREAD))
        );

        // And the configuration it is built for is accepted, in both
        // of libbela's spellings of one render thread.
        for spelling in [0, 1] {
            raw.threadCount = spelling;

            assert_eq!(
                check_supported(&raw, None, &SingleThreaded),
                Ok(()),
                "threadCount {spelling}"
            );
        }
    }

    #[test]
    fn an_application_sees_the_cpu_monitoring_that_is_not_in_the_c_settings() {
        // `Settings::cpu_monitoring` is a separate C call and leaves
        // `BelaInitSettings` untouched, so without it being carried
        // into the view an application could not tell an audio system
        // that will report CPU usage from one that will not.
        let raw = fake_defaults();
        let cycle = cycle_of(2000);

        assert_eq!(check_supported(&raw, Some(cycle), &NeedsMonitoring), Ok(()));
        assert_eq!(
            check_supported(&raw, None, &NeedsMonitoring),
            Err(Error::SettingsRefused(NEEDS_MONITORING))
        );
        assert_eq!(
            ResolvedSettings::new(&raw, Some(cycle)).cpu_monitoring(),
            Some(cycle)
        );
        assert_eq!(ResolvedSettings::new(&raw, None).cpu_monitoring(), None);
    }

    #[test]
    fn the_crates_own_checks_are_made_before_the_applications() {
        // Both would refuse this configuration. The crate's answer is
        // the one to report: a sample rate of 0 is not a configuration
        // any application could have run under, so naming the
        // application as the one that turned it down would name the
        // wrong culprit.
        let mut raw = fake_defaults();
        raw.audioSampleRate = 0.0;
        raw.numAnalogInChannels = 4;

        assert_eq!(
            check_supported(&raw, None, &NeedsEightAnalogInputs),
            Err(Error::SampleRate)
        );
    }

    #[test]
    fn the_cpu_monitoring_period_size_is_checked_before_the_application_is_asked() {
        let mut raw = fake_defaults();
        raw.periodSize = c_int::try_from(crate::MAX_MONITORED_PERIOD_SIZE)
            .expect("the monitored period size limit fits in a C int")
            + 1;
        raw.numAnalogInChannels = 4;

        let period_size = u32::try_from(raw.periodSize).expect("the period size above is positive");
        assert_eq!(
            check_supported(&raw, Some(cycle_of(2000)), &NeedsEightAnalogInputs),
            Err(Error::CpuMonitoringPeriodSize(period_size))
        );
        // Without monitoring there is no such limit, and the
        // application is the only one with anything to say.
        assert_eq!(
            check_supported(&raw, None, &NeedsEightAnalogInputs),
            Err(Error::SettingsRefused(NEEDS_EIGHT))
        );
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
