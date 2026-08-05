//! Output levels and input gain: the codec's analogue volume controls.
//!
//! Four knobs, all of them on the codec rather than in the signal the
//! application renders: the line out level, the headphone level, the
//! audio input gain and the speaker amplifier's mute. They belong to
//! the audio system, so they are set through the
//! [`Bela`](crate::Bela) handle that owns it —
//! [`set_line_out_level`](crate::Bela::set_line_out_level) and the
//! rest.
//!
//! # Why they are not settings
//!
//! `BelaInitSettings` carries the same three gains as
//! `BelaChannelGainArray` fields, which reads like the natural home for
//! them. It is not: libbela applies those arrays by calling exactly the
//! functions wrapped here, from inside `Bela_initAudio`, and the codec
//! only writes its registers once audio starts. Setting a level between
//! [`Bela::new`](crate::Bela::new) and
//! [`Bela::start`](crate::Bela::start) therefore reaches the hardware in
//! the same state and at the same moment as a settings-time gain would,
//! which is why [`Bela::until_stopped`] is public: it is
//! [`Bela::run`](crate::Bela::run) with that window left open.
//!
//! That leaves nothing for [`Settings`](crate::Settings) to carry but
//! the arrays' storage and a second way to say the same thing — the
//! exception being
//! [`Settings::begin_muted`](crate::Settings::begin_muted), which
//! changes what `Bela_startAudio` does and so cannot be expressed as a
//! call before it.
//!
//! # What a level may be
//!
//! Any finite number of decibels up to [`MAX_DECIBELS`] in magnitude.
//! The codec clamps what it cannot do — Bela's own range for each knob
//! is on the method — but a value libbela could not convert into
//! register values at all is refused here rather than passed on: the
//! conversion is a C cast to `int`, which is undefined for a NaN or an
//! out-of-range float, and every clamp on the C side is a comparison a
//! NaN slips through.
//!
//! # Not real-time safe
//!
//! Each call talks to the codec over I²C, which makes no promises about
//! how long it takes; Bela's own documentation says not to call these
//! from `render`. Taking them on the `Bela` handle keeps them out of
//! reach there: `render` gets a [`Context`](crate::Context), and the
//! handle stays with the thread that owns the audio system.
//!
//! # On a Bela Gem Stereo
//!
//! What the codec does with a level is hardware-specific, and this is
//! what was measured on the board (see `docs/board-facts.md`):
//!
//! - only channels 0 and 1 exist. [`Channel::One`] above that is
//!   refused for the line out and the headphone output, and accepted
//!   but ignored for the input gain.
//! - a level outside the codec's range is clamped rather than refused,
//!   so nothing reports that `+18` dB on the line out became `+9`.
//! - there is no amplifier mute pin, so
//!   [`mute_speakers`](crate::Bela::mute_speakers) and
//!   [`Settings::begin_muted`](crate::Settings::begin_muted) succeed
//!   without doing anything. They are wrapped for the Bela hardware
//!   that does have one.
//!
//! [`Bela::until_stopped`]: crate::Bela::until_stopped

use core::ffi::c_int;

#[cfg(bela_device)]
use crate::application::BelaApplication;
use crate::error::Error;
#[cfg(bela_device)]
use crate::system::Bela;

/// Largest magnitude, in decibels, a level or gain may have.
///
/// Not a codec limit — every codec clamps long before this, and
/// `docs/board-facts.md` records where. It is the point past which
/// libbela cannot convert the value at all: it turns decibels into
/// register values with C casts of `floorf(decibels * 2 + 0.5)` and
/// `decibels * 2` to `int`, and converting a float that does not fit in
/// an `int` — or is not a number at all — is undefined behaviour in
/// C++. So [`Bela::set_line_out_level`](crate::Bela::set_line_out_level)
/// and its siblings refuse those values instead of passing them on;
/// see [`Error::Decibels`](crate::Error::Decibels).
pub const MAX_DECIBELS: f32 = 1e9;

/// Checks that libbela can convert `decibels` into register values.
///
/// The clamping each codec does is comparisons against its own range,
/// which a NaN fails on the way in and on the way out, so it reaches
/// the cast like any other value. Nothing downstream can defend
/// against that; this is where it has to happen.
///
/// # Errors
/// Returns [`Error::Decibels`] for a value that is not finite, or whose
/// magnitude is above [`MAX_DECIBELS`].
#[cfg_attr(
    not(bela_device),
    allow(
        dead_code,
        reason = "only the device-gated audio system sets levels; still unit-tested on the host"
    )
)]
fn check_decibels(decibels: f32) -> Result<(), Error> {
    if decibels.is_finite() && decibels.abs() <= MAX_DECIBELS {
        Ok(())
    } else {
        Err(Error::Decibels)
    }
}

/// Which channel a level or gain applies to.
///
/// Channels are numbered as the context's audio channels are, from
/// zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// Every channel the codec has.
    All,
    /// One channel, counted the way
    /// [`Context::audio_out_channels`](crate::Context::audio_out_channels)
    /// counts them.
    One(usize),
}

impl Channel {
    /// The C spelling, where a negative channel number means "all".
    ///
    /// A channel number too large for a C `int` saturates rather than
    /// wrapping: the wrapped value could be negative, which would set
    /// every channel instead of reporting a channel that does not
    /// exist.
    #[cfg_attr(
        not(bela_device),
        allow(
            dead_code,
            reason = "only the device-gated audio system sets levels; still unit-tested on the host"
        )
    )]
    const fn to_c_int(self) -> c_int {
        match self {
            Self::All => -1,
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_possible_wrap,
                reason = "the comparison rules out the values that would truncate or wrap"
            )]
            Self::One(channel) if channel <= c_int::MAX as usize => channel as c_int,
            Self::One(_) => c_int::MAX,
        }
    }
}

#[cfg(bela_device)]
impl<T: BelaApplication> Bela<T> {
    /// Sets the level of the line output, in decibels.
    ///
    /// Zero is full scale and negative values attenuate; how far in
    /// either direction depends on the codec. On a Bela Gem Stereo the
    /// line out is channels 0 and 1, attenuates in 0.5 dB steps down to
    /// -63.5 dB and boosts up to +9 dB, and a value outside that is
    /// clamped without being reported.
    ///
    /// Takes effect immediately once audio is running, and is otherwise
    /// remembered and applied when it starts — so this is also how a
    /// program that uses [`until_stopped`](Bela::until_stopped) sets
    /// the level audio comes up with:
    ///
    /// ```no_run
    /// use bela::{Bela, Channel, Settings};
    /// # use bela::{BelaApplication, Context};
    /// # struct App;
    /// # unsafe impl BelaApplication for App {
    /// #     fn render(&mut self, _context: &mut Context) {}
    /// # }
    ///
    /// fn main() -> Result<(), bela::Error> {
    ///     let mut bela = Bela::new(App, &Settings::new())?;
    ///     bela.set_line_out_level(Channel::All, -6.0)?;
    ///     bela.until_stopped()
    /// }
    /// ```
    ///
    /// # Errors
    /// Returns [`Error::LineOutLevel`] when the codec refuses the call,
    /// which on a Bela Gem Stereo is what a channel above 1 gets, and
    /// [`Error::Decibels`] for a level libbela could not convert — see
    /// [`MAX_DECIBELS`](crate::MAX_DECIBELS).
    pub fn set_line_out_level(&mut self, channel: Channel, decibels: f32) -> Result<(), Error> {
        check_decibels(decibels)?;
        // Safety: an audio system exists — this needs the handle that
        // owns it — and libbela's own settings path calls this the same
        // way, from the thread that brought the audio system up.
        let ret = unsafe { bela_sys::Bela_setLineOutLevel(channel.to_c_int(), decibels) };
        if ret == 0 {
            Ok(())
        } else {
            Err(Error::LineOutLevel(ret))
        }
    }

    /// Sets the level of the onboard headphone amplifier, in decibels.
    ///
    /// The headphone output only: the line out and the speakers are
    /// unaffected. Bela's documented range is -63.5 dB to 0 dB in
    /// 0.5 dB steps, and the default is -6 dB. Like
    /// [`set_line_out_level`](Bela::set_line_out_level), it applies at
    /// once while audio runs and is otherwise applied when it starts.
    ///
    /// # Errors
    /// Returns [`Error::HeadphoneLevel`] when the codec refuses the
    /// call, which on a Bela Gem Stereo is what a channel above 1 gets,
    /// and [`Error::Decibels`] for a level libbela could not convert —
    /// see [`MAX_DECIBELS`](crate::MAX_DECIBELS).
    pub fn set_headphone_level(&mut self, channel: Channel, decibels: f32) -> Result<(), Error> {
        check_decibels(decibels)?;
        // Safety: as for `set_line_out_level`.
        let ret = unsafe { bela_sys::Bela_setHpLevel(channel.to_c_int(), decibels) };
        if ret == 0 {
            Ok(())
        } else {
            Err(Error::HeadphoneLevel(ret))
        }
    }

    /// Sets the gain of the input preamplifier, in decibels.
    ///
    /// This is the programmable gain amplifier ahead of the ADC, so it
    /// changes what the audio inputs actually sample — turn it up for a
    /// quiet source rather than scaling in `render`, which only
    /// amplifies the noise the ADC already digitised. It does not
    /// affect the analog inputs.
    ///
    /// Bela's documented range is 0 dB to 59.5 dB in 0.5 dB steps, and
    /// the default is 16 dB.
    ///
    /// # Errors
    /// Returns [`Error::AudioInputGain`] when the codec refuses the
    /// call — a Bela Gem Stereo does not: a channel it does not have is
    /// accepted and ignored — and [`Error::Decibels`] for a gain
    /// libbela could not convert, see
    /// [`MAX_DECIBELS`](crate::MAX_DECIBELS).
    pub fn set_audio_input_gain(&mut self, channel: Channel, decibels: f32) -> Result<(), Error> {
        check_decibels(decibels)?;
        // Safety: as for `set_line_out_level`.
        let ret = unsafe { bela_sys::Bela_setAudioInputGain(channel.to_c_int(), decibels) };
        if ret == 0 {
            Ok(())
        } else {
            Err(Error::AudioInputGain(ret))
        }
    }

    /// Mutes or unmutes the onboard speaker amplifiers.
    ///
    /// This drives the amplifier's mute pin, so it silences the
    /// speakers without touching any level. Which state audio comes up
    /// in is
    /// [`Settings::begin_muted`](crate::Settings::begin_muted):
    /// [`start`](Bela::start) unmutes unless that asked otherwise, so
    /// muting before it has no effect.
    ///
    /// A Bela Gem Stereo has no amplifier mute pin — measured, see
    /// `docs/board-facts.md` — and libbela then reports success without
    /// doing anything.
    ///
    /// # Errors
    /// Returns [`Error::MuteSpeakers`] when libbela refuses the call.
    pub fn mute_speakers(&mut self, mute: bool) -> Result<(), Error> {
        // Safety: as for `set_line_out_level`.
        let ret = unsafe { bela_sys::Bela_muteSpeakers(c_int::from(mute)) };
        if ret == 0 {
            Ok(())
        } else {
            Err(Error::MuteSpeakers(ret))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_channels_is_belas_negative_channel() {
        assert_eq!(Channel::All.to_c_int(), -1);
    }

    #[test]
    fn a_channel_number_is_passed_through() {
        assert_eq!(Channel::One(0).to_c_int(), 0);
        assert_eq!(Channel::One(1).to_c_int(), 1);
        assert_eq!(
            Channel::One(c_int::MAX.unsigned_abs() as usize).to_c_int(),
            c_int::MAX
        );
    }

    #[test]
    fn ordinary_levels_are_accepted() {
        for decibels in [0.0, -6.0, 16.0, -63.5, 59.5, -200.0, MAX_DECIBELS] {
            assert_eq!(check_decibels(decibels), Ok(()), "{decibels} dB");
        }
        // The codec clamps its own range; refusing here would be this
        // crate inventing a limit libbela does not have.
        assert_eq!(check_decibels(-MAX_DECIBELS), Ok(()));
    }

    #[test]
    fn a_level_libbela_could_not_convert_is_refused() {
        // Each of these reaches a C cast to `int` — the clamps on the
        // way are comparisons a NaN fails — and that cast is undefined
        // behaviour for them, so they must not get that far.
        for decibels in [
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            MAX_DECIBELS * 2.0,
            -MAX_DECIBELS * 2.0,
            f32::MIN,
            f32::MAX,
        ] {
            assert_eq!(check_decibels(decibels), Err(Error::Decibels), "{decibels}");
        }
    }

    #[test]
    fn the_accepted_levels_survive_belas_conversion() {
        // What libbela does with a level: `floorf(dB * 2 + 0.5)` cast
        // to `int`. The limit is only worth anything if the largest
        // accepted value still fits there.
        // In f64, which holds these exactly, so the check is about the
        // limit rather than about its own rounding.
        let half_dbs = f64::from(MAX_DECIBELS) * 2.0;
        let converted = (half_dbs + 0.5).floor();

        assert!(
            converted <= f64::from(c_int::MAX),
            "{converted} does not fit in the C int libbela casts to"
        );
    }

    #[test]
    fn a_channel_number_too_large_for_a_c_int_saturates() {
        // Wrapping would turn a channel that does not exist into
        // "every channel", which is the one answer that must not
        // happen.
        for channel in [c_int::MAX.unsigned_abs() as usize + 1, usize::MAX] {
            assert!(
                Channel::One(channel).to_c_int() > 0,
                "channel {channel} must not arrive as a request to set every channel"
            );
        }
    }
}
