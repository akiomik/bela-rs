//! MIDI input: reading what a device sends.
//!
//! Built on Bela's `Midi` class through the C surface in
//! [`bela_sys`], which runs the input thread, the parser and the
//! device recovery. `docs/midi.md` in the repository records what that
//! class does, what it does not report, and why this crate wraps it
//! rather than talking to ALSA.

use core::fmt;
use core::iter;
#[cfg(bela_device)]
use core::ptr::{self, NonNull};
#[cfg(bela_device)]
use std::ffi::CString;

use crate::error::Error;

/// Every MIDI port ALSA reports, by the name that opens it.
///
/// The names carry card, device **and** subdevice — `hw:0,0,0` where
/// `amidi -l` prints `hw:0,0` — and [`MidiInput::open`] matches against
/// exactly these, so the shorter form opens nothing.
///
/// Allocates and reads the ALSA control interface, so call it from
/// `setup` or before, never from a callback. Off the device target the
/// list is empty.
///
/// ```no_run
/// for port in bela::midi_ports() {
///     println!("{port}");
/// }
/// ```
#[must_use]
pub fn midi_ports() -> Vec<String> {
    ports()
}

#[cfg(bela_device)]
fn ports() -> Vec<String> {
    // Safety: asking for the size writes nothing, so a null buffer with
    // a length of zero is what the shim documents for it.
    let needed = unsafe { bela_sys::bela_midi_list_ports(ptr::null_mut(), 0) } as usize;
    let mut buffer = vec![0u8; needed];
    // Safety: the buffer is `needed` bytes long, which is what the call
    // above said the whole list takes. Both calls answer with the size
    // of the whole list rather than with what was written, so a list
    // that grew in between comes back larger than the buffer — the
    // shim copies whole names into what there is, and the truncate
    // below drops the rest.
    let needed_now = unsafe {
        bela_sys::bela_midi_list_ports(
            buffer.as_mut_ptr().cast(),
            u32::try_from(needed).unwrap_or(u32::MAX),
        )
    } as usize;
    buffer.truncate(needed_now.min(needed));
    buffer
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
        .map(|name| String::from_utf8_lossy(name).into_owned())
        .collect()
}

#[cfg(not(bela_device))]
#[allow(
    clippy::missing_const_for_fn,
    reason = "mirrors the device signature, which allocates"
)]
fn ports() -> Vec<String> {
    Vec::new()
}

/// A MIDI port opened for input.
///
/// Opening one starts a real-time thread inside Bela that reads the
/// device and parses what it reads into a ring of at most 100
/// messages. [`read`](MidiInput::read) takes them out of that ring:
/// two index reads and a copy, with no allocation, no system call and
/// nothing to block on.
///
/// # Where to read from
///
/// [`read`](MidiInput::read) takes `&mut self`, so it can be called
/// from [`render_pre`](crate::BelaApplication::render_pre) and
/// [`render_post`](crate::BelaApplication::render_post) — which run
/// once per block on the main audio thread — and not from
/// [`render`](crate::BelaApplication::render), which holds the
/// application as `&self` on every render thread at once.
///
/// That is the ring's own requirement rather than a choice: taking a
/// message advances a read pointer that nothing synchronises, so it
/// has one reader. `render_pre` is where a block's messages belong
/// anyway — the state they change is what `render` then plays.
///
/// # Running status is not delivered
///
/// A device may leave the status byte out when it repeats — a stream
/// of note ons as `90 3C 64`, `40 6E`, `43 71` — and most keyboards
/// do. Bela's parser discards those bytes rather than reading them as
/// the message they continue (`Midi.cpp:77`), so what reaches here is
/// the first message of such a run and nothing else, with nothing
/// reported.
///
/// Measured, and it is not a corner: through `snd-virmidi`, whose
/// sequencer re-encodes that way, `90 3C 64` followed by `90 40 6E`
/// arrives as one message. Alternating status bytes arrive whole.
///
/// Nothing in this crate can fix it: the parser is Bela's, and the
/// alternative is the raw byte path, which means writing a parser
/// instead of wrapping one — the thing `docs/midi.md` decided against.
///
/// # System exclusive
///
/// Not delivered. Bela's parser keeps sysex out of the message ring
/// and offers a callback on its input thread instead, which this crate
/// does not expose; the shim sets one that discards, because a parser
/// with no callback prints every byte it receives to the console.
///
/// # Example
///
/// ```no_run
/// use bela::{BelaApplication, BlockContext, MidiInput, MidiMessage, RenderContext, SetupContext, ThreadInfo};
///
/// struct Synth {
///     midi: MidiInput,
///     gate: bool,
/// }
///
/// impl BelaApplication for Synth {
///     type RenderState = ();
///
///     fn create_render_state(&mut self, _thread: ThreadInfo, _context: &SetupContext) {}
///
///     fn render_pre(&mut self, _states: &mut [()], _context: &mut BlockContext) {
///         while let Some(message) = self.midi.read() {
///             match message {
///                 MidiMessage::NoteOn { velocity, .. } if velocity.get() > 0 => self.gate = true,
///                 MidiMessage::NoteOn { .. } | MidiMessage::NoteOff { .. } => self.gate = false,
///                 _ => {}
///             }
///         }
///     }
///
///     fn render(&self, _state: &mut (), _context: &mut RenderContext) {
///         // ... play, or not, according to self.gate
///     }
/// }
/// ```
#[derive(Debug)]
pub struct MidiInput {
    #[cfg(bela_device)]
    raw: NonNull<bela_sys::BelaMidi>,
}

// The object behind the pointer is read here and written by the input
// thread Bela runs, which is the arrangement the class is built for.
// What it does not survive is two readers, and `read` taking `&mut
// self` is what rules that out — including across threads, where an
// application holding this is shared as `&self`.
//
// Sync therefore allows exactly one thing across threads:
// `available()` at the same time from several. That reads two indices
// the input thread writes without synchronisation, which the shim's
// header describes and bounds — a count one message stale, never a
// torn pointer.
unsafe impl Send for MidiInput {}
unsafe impl Sync for MidiInput {}

impl MidiInput {
    /// Opens `port` and starts reading from it.
    ///
    /// The name is one of [`midi_ports`]. Opening allocates and starts
    /// a thread, so this belongs in
    /// [`setup`](crate::BelaApplication::setup) or before it, not in a
    /// render callback.
    ///
    /// # Errors
    ///
    /// [`Error::MidiPortName`] when `port` contains a NUL byte,
    /// [`Error::MidiCreate`] when the object could not be created —
    /// which is also what happens off the device target, where there
    /// is no `libbelaextra` — and [`Error::MidiOpen`] when the port
    /// itself could not be opened, carrying what the shim reported:
    /// [`bela_sys::BELA_MIDI_NO_SUCH_PORT`] for a name no port has, or
    /// an ALSA failure as a negative `errno`.
    ///
    /// A port that something else already holds is one of those:
    /// measured on the board, a second reader of the same port gets
    /// `MidiOpen(-16)`, `EBUSY`. ALSA prints a line of its own about
    /// it as well, which is not this crate's doing.
    #[cfg(bela_device)]
    pub fn open(port: &str) -> Result<Self, Error> {
        let name = CString::new(port).map_err(|_| Error::MidiPortName)?;
        // Safety: the shim allocates and hands back ownership, or null.
        let raw = NonNull::new(unsafe { bela_sys::bela_midi_new() }).ok_or(Error::MidiCreate)?;
        let input = Self { raw };
        // Safety: `raw` is the object just created, and `name` outlives
        // the call — the shim copies what it needs.
        let opened = unsafe { bela_sys::bela_midi_read_from(input.raw.as_ptr(), name.as_ptr()) };
        if opened < 0 {
            // Dropping closes what was created; nothing was opened.
            return Err(Error::MidiOpen(opened));
        }
        Ok(input)
    }

    /// Opens `port` and starts reading from it.
    ///
    /// # Errors
    ///
    /// Always [`Error::MidiCreate`] off the device target: there is no
    /// `libbelaextra` to open a port with.
    #[cfg(not(bela_device))]
    #[allow(
        clippy::missing_const_for_fn,
        reason = "mirrors the device signature, which is not const"
    )]
    pub fn open(_port: &str) -> Result<Self, Error> {
        Err(Error::MidiCreate)
    }

    /// How many parsed messages are waiting.
    ///
    /// Real-time safe: two index reads. Worth looking at when a
    /// program wants to know it is falling behind: the ring holds 100
    /// messages (`Midi.h:155`), the input thread advances its write
    /// index without ever consulting the read one (`Midi.cpp:99`), and
    /// the count is their difference modulo the size (`Midi.h:240`).
    ///
    /// So a program that reads more slowly than a device sends loses
    /// the **oldest** unread messages, to the newer ones written over
    /// them, and this count drops rather than saturating — nothing
    /// anywhere reports the lap. Read from `render_pre` every block,
    /// 100 messages is far more than a block can bring; a count that
    /// approaches it is the warning. **Read, not measured**: it is
    /// what those three lines say.
    #[must_use]
    #[allow(
        clippy::missing_const_for_fn,
        reason = "const only off-device, where there is no ring to read"
    )]
    pub fn available(&self) -> usize {
        self.available_raw()
    }

    #[cfg(bela_device)]
    fn available_raw(&self) -> usize {
        // Safety: `raw` is live for as long as `self` is.
        let available = unsafe { bela_sys::bela_midi_available_messages(self.raw.as_ptr()) };
        usize::try_from(available).unwrap_or(0)
    }

    #[cfg(not(bela_device))]
    #[allow(
        clippy::unused_self,
        reason = "mirrors the device signature; there is no port to have read from"
    )]
    const fn available_raw(&self) -> usize {
        0
    }

    /// Takes the oldest waiting message, or [`None`] when none is
    /// waiting.
    ///
    /// Real-time safe: a ring read and a copy of at most three bytes.
    /// Call it in a loop — one call is one message, and a block can
    /// carry several.
    ///
    /// A message the parser produced that this crate has no name for
    /// is skipped rather than returned, so a loop reading until `None`
    /// still ends. The two undefined system real-time status bytes
    /// (`0xF9`, `0xFD`) are the only ones that can happen.
    ///
    /// [`None`] means the ring is empty and nothing else: the shim
    /// skips a record it cannot use rather than reporting it as an
    /// empty ring, which would end a drain with messages still in
    /// there.
    pub fn read(&mut self) -> Option<MidiMessage> {
        loop {
            let (bytes, len) = self.read_raw()?;
            // Only the bytes the shim says it wrote. A message whose
            // data bytes are missing is dropped rather than read with
            // zeroes in their place — which for a note on would be a
            // release nobody sent.
            if let Some(message) = MidiMessage::from_bytes(&bytes[..len]) {
                return Some(message);
            }
        }
    }

    /// Every message waiting, in the order they arrived.
    ///
    /// [`read`](Self::read) in the shape a `for` loop wants. The
    /// iterator borrows this exclusively, so it ends when the ring is
    /// empty and does not outlive the callback that drained it.
    ///
    /// ```no_run
    /// # use bela::{BelaApplication, BlockContext, MidiInput, MidiMessage};
    /// # struct App { midi: MidiInput, notes: u32 }
    /// # impl App {
    /// fn read_block(&mut self) {
    ///     for message in self.midi.messages() {
    ///         if let MidiMessage::NoteOn { .. } = message {
    ///             self.notes += 1;
    ///         }
    ///     }
    /// }
    /// # }
    /// ```
    pub fn messages(&mut self) -> impl Iterator<Item = MidiMessage> + '_ {
        iter::from_fn(move || self.read())
    }

    /// The next message as it came off the wire and how many bytes of
    /// it there are, or [`None`] when none is waiting.
    #[cfg(bela_device)]
    fn read_raw(&mut self) -> Option<([u8; bela_sys::BELA_MIDI_MESSAGE_MAX], usize)> {
        let mut bytes = [0u8; bela_sys::BELA_MIDI_MESSAGE_MAX];
        // Safety: `raw` is live, and the buffer is the size the shim
        // documents. `&mut self` is what makes this the only reader.
        let written =
            unsafe { bela_sys::bela_midi_get_message(self.raw.as_ptr(), bytes.as_mut_ptr()) };
        let len = usize::try_from(written)
            .unwrap_or(0)
            .min(bela_sys::BELA_MIDI_MESSAGE_MAX);
        (len > 0).then_some((bytes, len))
    }

    #[cfg(not(bela_device))]
    #[allow(
        clippy::unused_self,
        clippy::needless_pass_by_ref_mut,
        reason = "mirrors the device signature, where &mut self is what keeps the ring to one reader"
    )]
    const fn read_raw(&mut self) -> Option<([u8; bela_sys::BELA_MIDI_MESSAGE_MAX], usize)> {
        None
    }
}

impl Drop for MidiInput {
    /// Closes the port and joins the input thread.
    ///
    /// **This blocks**, for as long as that thread takes to notice: it
    /// polls the device with a 50 ms timeout. Nothing real-time may
    /// drop one, which is no different from opening one.
    fn drop(&mut self) {
        #[cfg(bela_device)]
        // Safety: `raw` came from `bela_midi_new`, is dropped once, and
        // nothing else holds it.
        unsafe {
            bela_sys::bela_midi_delete(self.raw.as_ptr());
        }
    }
}

/// A MIDI message, as Bela's parser hands it over.
///
/// Channel messages carry the channel they arrived on; the system
/// real-time messages have none. System common messages and system
/// exclusive never reach here — Bela's parser drops the first and
/// routes the second to a callback this crate does not expose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MidiMessage {
    /// A key was released.
    NoteOff {
        /// The channel it arrived on.
        channel: MidiChannel,
        /// Which key.
        note: Note,
        /// How fast it was released, where a device that cannot
        /// measure that sends 0 or 64.
        velocity: Velocity,
    },
    /// A key was pressed.
    ///
    /// A velocity of 0 means the same as [`NoteOff`](Self::NoteOff),
    /// and most devices send it that way rather than sending note off
    /// at all.
    NoteOn {
        /// The channel it arrived on.
        channel: MidiChannel,
        /// Which key.
        note: Note,
        /// How hard it was pressed, or 0 for a release.
        velocity: Velocity,
    },
    /// Pressure applied to one key that is already held.
    KeyPressure {
        /// The channel it arrived on.
        channel: MidiChannel,
        /// Which key.
        note: Note,
        /// How much pressure.
        pressure: Pressure,
    },
    /// A controller moved.
    ControlChange {
        /// The channel it arrived on.
        channel: MidiChannel,
        /// Which controller.
        controller: Controller,
        /// Where it moved to.
        value: ControlValue,
    },
    /// A different sound was selected.
    ProgramChange {
        /// The channel it arrived on.
        channel: MidiChannel,
        /// Which sound.
        program: Program,
    },
    /// Pressure applied to every key held on the channel.
    ChannelPressure {
        /// The channel it arrived on.
        channel: MidiChannel,
        /// How much pressure.
        pressure: Pressure,
    },
    /// The pitch wheel moved.
    PitchBend {
        /// The channel it arrived on.
        channel: MidiChannel,
        /// Where it moved to, with [`PitchBend::CENTRE`] for the
        /// middle.
        bend: PitchBend,
    },
    /// One tick of the sender's clock: 24 of them to a quarter note.
    Clock,
    /// Start playing from the beginning.
    Start,
    /// Start playing from where a [`Stop`](Self::Stop) left off.
    Continue,
    /// Stop playing.
    Stop,
    /// The sender is still there, sent when nothing else is.
    ActiveSensing,
    /// Reset to power-on state.
    Reset,
}

/// Status nibbles, as they arrive on the wire.
const NOTE_OFF: u8 = 0x80;
const NOTE_ON: u8 = 0x90;
const KEY_PRESSURE: u8 = 0xA0;
const CONTROL_CHANGE: u8 = 0xB0;
const PROGRAM_CHANGE: u8 = 0xC0;
const CHANNEL_PRESSURE: u8 = 0xD0;
const PITCH_BEND: u8 = 0xE0;
const SYSTEM: u8 = 0xF0;

impl MidiMessage {
    /// Reads a message out of the bytes the shim wrote.
    ///
    /// [`None`] for a status byte with no variant here, which is the
    /// two undefined system real-time messages and nothing else: the
    /// parser only ever queues channel messages and system real-time
    /// ones.
    fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let status = *bytes.first()?;
        let channel = MidiChannel::from_bits(status);
        // Missing rather than zero: a message that lost its data bytes
        // is not a message about note 0.
        let first = bytes.get(1).copied();
        let second = bytes.get(2).copied();
        Some(match status & 0xF0 {
            NOTE_OFF => Self::NoteOff {
                channel,
                note: Note::from_bits(first?),
                velocity: Velocity::from_bits(second?),
            },
            NOTE_ON => Self::NoteOn {
                channel,
                note: Note::from_bits(first?),
                velocity: Velocity::from_bits(second?),
            },
            KEY_PRESSURE => Self::KeyPressure {
                channel,
                note: Note::from_bits(first?),
                pressure: Pressure::from_bits(second?),
            },
            CONTROL_CHANGE => Self::ControlChange {
                channel,
                controller: Controller::from_bits(first?),
                value: ControlValue::from_bits(second?),
            },
            PROGRAM_CHANGE => Self::ProgramChange {
                channel,
                program: Program::from_bits(first?),
            },
            CHANNEL_PRESSURE => Self::ChannelPressure {
                channel,
                pressure: Pressure::from_bits(first?),
            },
            PITCH_BEND => Self::PitchBend {
                channel,
                // Low seven bits first, as the wire has them.
                bend: PitchBend::from_bits(first?, second?),
            },
            // The parser puts the whole status byte of a system
            // real-time message in the low nibble it usually gives a
            // channel, so this is the byte the device sent.
            SYSTEM => match status {
                0xF8 => Self::Clock,
                0xFA => Self::Start,
                0xFB => Self::Continue,
                0xFC => Self::Stop,
                0xFE => Self::ActiveSensing,
                0xFF => Self::Reset,
                // 0xF9 and 0xFD are undefined, and the system common
                // messages below 0xF8 never reach the ring.
                _ => return None,
            },
            _ => return None,
        })
    }

    /// The channel this arrived on, or [`None`] for a system message.
    #[must_use]
    pub const fn channel(self) -> Option<MidiChannel> {
        match self {
            Self::NoteOff { channel, .. }
            | Self::NoteOn { channel, .. }
            | Self::KeyPressure { channel, .. }
            | Self::ControlChange { channel, .. }
            | Self::ProgramChange { channel, .. }
            | Self::ChannelPressure { channel, .. }
            | Self::PitchBend { channel, .. } => Some(channel),
            Self::Clock
            | Self::Start
            | Self::Continue
            | Self::Stop
            | Self::ActiveSensing
            | Self::Reset => None,
        }
    }
}

/// Defines a seven-bit MIDI value: 0 to 127, as one data byte carries.
///
/// Each is its own type rather than all of them being `u8`, because
/// they are what a call site swaps by accident — a note where a
/// velocity belongs is two numbers in the same range.
macro_rules! seven_bit {
    ($(
        $(#[$meta:meta])*
        $name:ident, $what:literal;
    )*) => {$(
        $(#[$meta])*
        ///
        /// A seven-bit value: 0 to 127, which is what one MIDI data
        /// byte carries.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
        pub struct $name(u8);

        impl $name {
            /// The largest value, 127.
            pub const MAX: Self = Self(127);

            #[doc = concat!("A ", $what, ", or [`None`] above 127.")]
            ///
            /// Rejecting is the point: the wire cannot carry the eighth
            /// bit, and the alternative — masking it off, as Bela's own
            /// `writeMessage` does — turns a number that was wrong into
            /// a different number that is not.
            #[must_use]
            pub const fn new(value: u8) -> Option<Self> {
                if value > Self::MAX.0 {
                    return None;
                }
                Some(Self(value))
            }

            /// The value, 0 to 127.
            #[must_use]
            pub const fn get(self) -> u8 {
                self.0
            }

            /// The value from a byte off the wire, whose top bit is a
            /// status flag and so never set in a data byte.
            const fn from_bits(bits: u8) -> Self {
                Self(bits & 0x7F)
            }
        }

        impl From<$name> for u8 {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl TryFrom<u8> for $name {
            type Error = Error;

            #[doc = concat!("A ", $what, ", or [`Error::MidiValue`] above 127.")]
            ///
            /// The same check as [`new`](Self::new), for code that
            /// converts generically. `new` stays because a range check
            /// with one way to fail says as much with [`Option`].
            fn try_from(value: u8) -> Result<Self, Self::Error> {
                Self::new(value).ok_or_else(|| Error::MidiValue {
                    value: u16::from(value),
                    max: u16::from(Self::MAX.0),
                    kind: $what,
                })
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    )*};
}

seven_bit! {
    /// Which key a note message is about, where 60 is middle C.
    Note, "note number";
    /// How hard a key was pressed or released.
    ///
    /// A note on with velocity 0 is a release, which is how most
    /// devices end a note.
    Velocity, "velocity";
    /// Which controller a [`ControlChange`](MidiMessage::ControlChange)
    /// is about — 1 is the modulation wheel, 7 volume, 64 the sustain
    /// pedal.
    Controller, "controller number";
    /// What a controller moved to.
    ControlValue, "controller value";
    /// Which sound a [`ProgramChange`](MidiMessage::ProgramChange)
    /// selects.
    Program, "program number";
    /// How hard a key or a channel is being pressed after the note
    /// started.
    Pressure, "pressure";
}

/// Which of the sixteen MIDI channels a message arrived on.
///
/// Numbered 0 to 15 here, as on the wire. Devices and their manuals
/// usually count from 1, so channel 0 is the one a synthesiser calls
/// channel 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct MidiChannel(u8);

impl MidiChannel {
    /// The highest channel number, 15.
    pub const MAX: Self = Self(15);

    /// A channel, or [`None`] above 15.
    #[must_use]
    pub const fn new(channel: u8) -> Option<Self> {
        if channel > Self::MAX.0 {
            return None;
        }
        Some(Self(channel))
    }

    /// The channel, 0 to 15.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }

    /// The channel a status byte carries in its low nibble.
    const fn from_bits(status: u8) -> Self {
        Self(status & 0x0F)
    }
}

impl From<MidiChannel> for u8 {
    fn from(channel: MidiChannel) -> Self {
        channel.0
    }
}

impl TryFrom<u8> for MidiChannel {
    type Error = Error;

    /// A channel, or [`Error::MidiValue`] above 15.
    fn try_from(channel: u8) -> Result<Self, Self::Error> {
        Self::new(channel).ok_or_else(|| Error::MidiValue {
            value: u16::from(channel),
            max: u16::from(Self::MAX.0),
            kind: "channel",
        })
    }
}

impl fmt::Display for MidiChannel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Where the pitch wheel is: fourteen bits, 0 to 16383.
///
/// The wheel is sprung to the middle rather than to zero, so
/// [`CENTRE`](Self::CENTRE) — 8192 — is "no bend", and how far the ends
/// bend the pitch is the receiving instrument's business, not the
/// sender's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PitchBend(u16);

impl PitchBend {
    /// The largest value, 16383.
    pub const MAX: Self = Self(16383);

    /// No bend: the wheel at rest, 8192.
    pub const CENTRE: Self = Self(8192);

    /// A bend, or [`None`] above 16383.
    #[must_use]
    pub const fn new(bend: u16) -> Option<Self> {
        if bend > Self::MAX.0 {
            return None;
        }
        Some(Self(bend))
    }

    /// The value, 0 to 16383.
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }

    /// How far from the centre, -8192 to 8191.
    ///
    /// What an instrument multiplies by its bend range; the sign is
    /// the direction.
    #[must_use]
    #[allow(
        clippy::cast_possible_wrap,
        clippy::cast_possible_truncation,
        reason = "the difference of two 14-bit values is in -8192..=8191"
    )]
    pub const fn offset(self) -> i16 {
        self.0 as i16 - Self::CENTRE.0 as i16
    }

    /// The value from the two data bytes, least significant first, as
    /// the wire orders them.
    const fn from_bits(low: u8, high: u8) -> Self {
        Self(((high as u16 & 0x7F) << 7) | (low as u16 & 0x7F))
    }
}

impl Default for PitchBend {
    /// [`CENTRE`](Self::CENTRE): a wheel nobody is touching.
    fn default() -> Self {
        Self::CENTRE
    }
}

impl From<PitchBend> for u16 {
    fn from(bend: PitchBend) -> Self {
        bend.0
    }
}

impl TryFrom<u16> for PitchBend {
    type Error = Error;

    /// A bend, or [`Error::MidiValue`] above 16383.
    fn try_from(bend: u16) -> Result<Self, Self::Error> {
        Self::new(bend).ok_or(Error::MidiValue {
            value: bend,
            max: Self::MAX.0,
            kind: "pitch bend",
        })
    }
}

impl fmt::Display for PitchBend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_note_on_carries_channel_note_and_velocity() {
        let message = MidiMessage::from_bytes(&[0x93, 60, 100]).expect("a note on should parse");
        assert_eq!(
            message,
            MidiMessage::NoteOn {
                channel: MidiChannel::new(3).unwrap(),
                note: Note::new(60).unwrap(),
                velocity: Velocity::new(100).unwrap(),
            }
        );
    }

    #[test]
    fn a_note_on_with_velocity_zero_stays_a_note_on() {
        // The wire says note on; what it means is a release, and that
        // is the reader's business rather than something to rewrite
        // here.
        let message = MidiMessage::from_bytes(&[0x90, 60, 0]).expect("a note on should parse");
        assert!(
            matches!(message, MidiMessage::NoteOn { velocity, .. } if velocity.get() == 0),
            "expected a note on with velocity 0, got {message:?}"
        );
    }

    #[test]
    fn a_pitch_bend_reads_low_byte_first() {
        let message =
            MidiMessage::from_bytes(&[0xE0, 0x00, 0x40]).expect("a pitch bend should parse");
        assert_eq!(
            message,
            MidiMessage::PitchBend {
                channel: MidiChannel::new(0).unwrap(),
                bend: PitchBend::CENTRE,
            },
            "0x00 0x40 is 8192, the centre"
        );
    }

    #[test]
    fn every_channel_message_parses_whole() {
        // Whole messages rather than only their channel: the data
        // bytes of a three-byte message are two numbers in the same
        // range, and reading them the wrong way round is what the
        // types in this module exist to make hard to do.
        let channel = MidiChannel::MAX;
        let note = Note::new(60).unwrap();
        let expected = [
            (
                [0x8F, 60, 0].as_slice(),
                MidiMessage::NoteOff {
                    channel,
                    note,
                    velocity: Velocity::new(0).unwrap(),
                },
            ),
            (
                [0x9F, 60, 1].as_slice(),
                MidiMessage::NoteOn {
                    channel,
                    note,
                    velocity: Velocity::new(1).unwrap(),
                },
            ),
            (
                [0xAF, 60, 1].as_slice(),
                MidiMessage::KeyPressure {
                    channel,
                    note,
                    pressure: Pressure::new(1).unwrap(),
                },
            ),
            (
                [0xBF, 7, 100].as_slice(),
                MidiMessage::ControlChange {
                    channel,
                    controller: Controller::new(7).unwrap(),
                    value: ControlValue::new(100).unwrap(),
                },
            ),
            (
                [0xCF, 5].as_slice(),
                MidiMessage::ProgramChange {
                    channel,
                    program: Program::new(5).unwrap(),
                },
            ),
            (
                [0xDF, 50].as_slice(),
                MidiMessage::ChannelPressure {
                    channel,
                    pressure: Pressure::new(50).unwrap(),
                },
            ),
            (
                [0xEF, 0, 0x40].as_slice(),
                MidiMessage::PitchBend {
                    channel,
                    bend: PitchBend::CENTRE,
                },
            ),
        ];
        for (bytes, message) in expected {
            let parsed = MidiMessage::from_bytes(bytes).expect("should parse");
            assert_eq!(parsed, message, "{bytes:02x?} parsed as something else");
            assert_eq!(
                parsed.channel(),
                Some(channel),
                "{message:?} lost its channel"
            );
        }
    }

    #[test]
    fn system_real_time_messages_have_no_channel() {
        let expected = [
            (0xF8, MidiMessage::Clock),
            (0xFA, MidiMessage::Start),
            (0xFB, MidiMessage::Continue),
            (0xFC, MidiMessage::Stop),
            (0xFE, MidiMessage::ActiveSensing),
            (0xFF, MidiMessage::Reset),
        ];
        for (status, message) in expected {
            let parsed = MidiMessage::from_bytes(&[status]).expect("should parse");
            assert_eq!(parsed, message, "status {status:#04x}");
            assert_eq!(parsed.channel(), None, "{message:?} should have no channel");
        }
    }

    #[test]
    fn the_undefined_system_messages_are_skipped() {
        for status in [0xF9u8, 0xFD] {
            assert_eq!(
                MidiMessage::from_bytes(&[status]),
                None,
                "{status:#04x} is undefined and has nothing to report"
            );
        }
    }

    #[test]
    fn a_truncated_message_is_not_half_a_message() {
        // The shim never writes one, and a message missing its data
        // bytes must not become one with zeroes in their place.
        assert_eq!(MidiMessage::from_bytes(&[]), None, "no status byte");
        assert_eq!(MidiMessage::from_bytes(&[0x90]), None, "no note number");
    }

    #[test]
    fn seven_bit_values_reject_the_eighth_bit() {
        assert_eq!(Note::new(127).map(Note::get), Some(127));
        assert_eq!(Note::new(128), None, "128 needs eight bits");
        assert_eq!(Velocity::new(255), None);
        assert_eq!(Controller::MAX.get(), 127);
    }

    #[test]
    fn a_value_converts_both_ways() {
        // C-CONV: the same range check, reachable from generic code.
        assert_eq!(Note::try_from(60), Ok(Note::new(60).unwrap()));
        assert_eq!(u8::from(Note::new(60).unwrap()), 60);
        assert_eq!(
            Velocity::try_from(128),
            Err(Error::MidiValue {
                value: 128,
                max: 127,
                kind: "velocity"
            })
        );
        assert_eq!(
            MidiChannel::try_from(16),
            Err(Error::MidiValue {
                value: 16,
                max: 15,
                kind: "channel"
            })
        );
        assert_eq!(PitchBend::try_from(8192), Ok(PitchBend::CENTRE));
        assert_eq!(
            PitchBend::try_from(16384),
            Err(Error::MidiValue {
                value: 16384,
                max: 16383,
                kind: "pitch bend"
            })
        );
    }

    #[test]
    fn two_values_in_the_same_range_fail_differently() {
        // The types are separate because a note and a velocity are two
        // numbers in the same range; an error that forgot which would
        // put them back together.
        let note = Note::try_from(200).unwrap_err();
        let velocity = Velocity::try_from(200).unwrap_err();
        assert_ne!(note, velocity, "the same bytes, and not the same error");
        assert!(
            note.to_string().contains("note number"),
            "expected the type in the message, got: {note}"
        );
    }

    #[test]
    fn a_channel_is_one_of_sixteen() {
        assert_eq!(MidiChannel::new(15).map(MidiChannel::get), Some(15));
        assert_eq!(MidiChannel::new(16), None, "there are sixteen channels");
    }

    #[test]
    fn a_pitch_bend_measures_from_the_centre() {
        assert_eq!(PitchBend::CENTRE.offset(), 0);
        assert_eq!(PitchBend::new(0).map(PitchBend::offset), Some(-8192));
        assert_eq!(PitchBend::MAX.offset(), 8191);
        assert_eq!(PitchBend::new(16384), None, "14 bits is 16383");
        assert_eq!(PitchBend::default(), PitchBend::CENTRE);
    }

    #[test]
    #[cfg(not(bela_device))]
    // Builds a MidiInput without opening a port, which only the
    // off-device shape allows — and off-device the ring is always
    // empty, which is the property being tested: `messages` must end
    // rather than yield forever, or a `for` loop in render_pre never
    // returns.
    fn the_iterator_ends_when_the_ring_is_empty() {
        let mut input = MidiInput {};
        assert_eq!(input.messages().count(), 0);
    }

    #[test]
    fn no_port_can_be_opened_off_device() {
        assert_eq!(
            MidiInput::open("hw:0,0,0").unwrap_err(),
            Error::MidiCreate,
            "off-device there is no libbelaextra to open a port with"
        );
        assert!(
            midi_ports().is_empty(),
            "and nothing to list ports with either"
        );
    }
}
