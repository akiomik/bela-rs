//! MIDI: reading what a device sends, and sending to one.
//!
//! Built on Bela's `Midi` class through the C surface in
//! [`bela_sys`], which runs the input thread, the parser and the
//! device recovery. `docs/midi.md` in the repository records what that
//! class does, what it does not report, and why this crate wraps it
//! rather than talking to ALSA.
//!
//! Input and output are separate types holding separate `Midi`
//! objects, so a program that only listens starts no output task and
//! one that only sends starts no input thread. They can name the same
//! port.

use core::fmt;
use core::iter;
#[cfg(bela_device)]
use core::ptr::{self, NonNull};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
#[cfg(bela_device)]
use std::ffi::CString;
use std::sync::Arc;
use std::thread::{self, ThreadId};

use crate::context::{CallbackContext, SetupContext};
use crate::error::Error;
use crate::task::{AuxiliaryTask, Priority};

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

/// System real-time status bytes, which are whole messages. Shared by
/// both directions so that reading and writing cannot drift apart.
const CLOCK: u8 = 0xF8;
const START: u8 = 0xFA;
const CONTINUE: u8 = 0xFB;
const STOP: u8 = 0xFC;
const ACTIVE_SENSING: u8 = 0xFE;
const RESET: u8 = 0xFF;

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
                CLOCK => Self::Clock,
                START => Self::Start,
                CONTINUE => Self::Continue,
                STOP => Self::Stop,
                ACTIVE_SENSING => Self::ActiveSensing,
                RESET => Self::Reset,
                // 0xF9 and 0xFD are undefined, and the system common
                // messages below 0xF8 never reach the ring.
                _ => return None,
            },
            _ => return None,
        })
    }

    /// The message as it goes on the wire, and how many of the three
    /// bytes it is.
    ///
    /// The inverse of [`from_bytes`](Self::from_bytes) for everything
    /// that type can produce.
    const fn to_bytes(self) -> ([u8; MESSAGE_MAX], usize) {
        // A status byte carries the channel in its low nibble, and a
        // system real-time message is a status byte on its own.
        const fn status(kind: u8, channel: MidiChannel) -> u8 {
            kind | channel.get()
        }
        match self {
            Self::NoteOff {
                channel,
                note,
                velocity,
            } => ([status(NOTE_OFF, channel), note.get(), velocity.get()], 3),
            Self::NoteOn {
                channel,
                note,
                velocity,
            } => ([status(NOTE_ON, channel), note.get(), velocity.get()], 3),
            Self::KeyPressure {
                channel,
                note,
                pressure,
            } => (
                [status(KEY_PRESSURE, channel), note.get(), pressure.get()],
                3,
            ),
            Self::ControlChange {
                channel,
                controller,
                value,
            } => (
                [
                    status(CONTROL_CHANGE, channel),
                    controller.get(),
                    value.get(),
                ],
                3,
            ),
            Self::ProgramChange { channel, program } => {
                ([status(PROGRAM_CHANGE, channel), program.get(), 0], 2)
            }
            Self::ChannelPressure { channel, pressure } => {
                ([status(CHANNEL_PRESSURE, channel), pressure.get(), 0], 2)
            }
            Self::PitchBend { channel, bend } => {
                let (low, high) = bend.to_bits();
                ([status(PITCH_BEND, channel), low, high], 3)
            }
            Self::Clock => ([CLOCK, 0, 0], 1),
            Self::Start => ([START, 0, 0], 1),
            Self::Continue => ([CONTINUE, 0, 0], 1),
            Self::Stop => ([STOP, 0, 0], 1),
            Self::ActiveSensing => ([ACTIVE_SENSING, 0, 0], 1),
            Self::Reset => ([RESET, 0, 0], 1),
        }
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
            /// The smallest value, 0.
            pub const MIN: Self = Self(0);

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

impl Note {
    /// Middle C, 60 — the note a MIDI keyboard's middle C sends.
    pub const MIDDLE_C: Self = Self(60);
}

/// Which of the sixteen MIDI channels a message arrived on.
///
/// Numbered 0 to 15 here, as on the wire. Devices and their manuals
/// usually count from 1, so channel 0 is the one a synthesiser calls
/// channel 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct MidiChannel(u8);

impl MidiChannel {
    /// The first channel, 0 — the one a device calls channel 1.
    pub const MIN: Self = Self(0);

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
    /// The smallest value, 0: bent as far down as the wheel goes.
    pub const MIN: Self = Self(0);

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

    /// The two data bytes, least significant first.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "each half is masked to seven bits"
    )]
    const fn to_bits(self) -> (u8, u8) {
        ((self.0 & 0x7F) as u8, (self.0 >> 7) as u8)
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

/// A MIDI port opened for output.
///
/// Sending from `render` puts a message in a queue this type owns and
/// asks an [`AuxiliaryTask`] to empty it; nothing on the audio thread
/// touches ALSA, or Bela's output pipe, or anything that can print.
/// `docs/midi.md` is the argument for that arrangement, measured
/// against the alternative of calling Bela's `writeOutput` from
/// `render` directly.
///
/// # One queue per render thread
///
/// The queue behind a [`MidiSender`] has one writer, and
/// [`take_sender`](MidiOutput::take_sender) hands each one out once, so
/// which thread writes to it is settled when the sender is taken —
/// normally in
/// [`create_render_state`](crate::BelaApplication::create_render_state),
/// with the sender kept in the render state.
///
/// Messages from one thread keep their order. Messages from different
/// threads do not have one: a drain empties the queues in thread
/// order, so what two threads sent in the same block leaves in that
/// order rather than in the order they were sent. A program that needs
/// one order sends from
/// [`render_pre`](crate::BelaApplication::render_pre) or
/// [`render_post`](crate::BelaApplication::render_post), which are one
/// thread.
///
/// # What a full queue means
///
/// [`capacity`](MidiOutput::capacity) is a budget the program declares:
/// how many messages one thread may queue between drains, and a drain
/// happens whenever the task gets to run — normally within the block.
/// Exceeding it is [`Error::MidiQueueFull`], and that is the only thing
/// that error means. It is **not** news about the device or about
/// Bela's own pipe, neither of which reports anything: `writeOutput`
/// returns the same value whether the bytes were queued or dropped.
/// See `docs/midi.md`.
///
/// # Sending after the last block
///
/// A message still queued when the audio system stops is never sent:
/// the task is deleted with every other one, and scheduling a handle
/// from a stopped audio system does nothing. A closing all-notes-off
/// belongs in [`cleanup`](crate::BelaApplication::cleanup), through
/// [`send`](MidiOutput::send), which does not go through the task at
/// all.
///
/// # Example
///
/// ```no_run
/// use bela::{
///     BelaApplication, MidiChannel, MidiMessage, MidiOutput, MidiSender, Note, RenderContext,
///     SetupContext, ThreadInfo, Velocity,
/// };
///
/// struct Sequencer {
///     output: Option<MidiOutput>,
///     port: String,
/// }
///
/// impl BelaApplication for Sequencer {
///     // The sender lives here: one per render thread, and `render`
///     // gets its own exclusively.
///     type RenderState = Option<MidiSender>;
///
///     fn setup(&mut self, context: &SetupContext) -> bool {
///         // Room for eight messages per thread per drain.
///         self.output = MidiOutput::open(&self.port, context, 8).ok();
///         self.output.is_some()
///     }
///
///     fn create_render_state(
///         &mut self,
///         thread: ThreadInfo,
///         _context: &SetupContext,
///     ) -> Option<MidiSender> {
///         self.output
///             .as_mut()
///             .and_then(|output| output.take_sender(thread.index()))
///     }
///
///     fn render(&self, sender: &mut Option<MidiSender>, context: &mut RenderContext) {
///         if let Some(sender) = sender {
///             let _ = sender.send(
///                 context,
///                 MidiMessage::NoteOn {
///                     channel: MidiChannel::MIN,
///                     note: Note::MIDDLE_C,
///                     velocity: Velocity::MAX,
///                 },
///             );
///         }
///     }
/// }
/// ```
#[derive(Debug)]
pub struct MidiOutput {
    shared: Arc<Shared>,
    task: Arc<AuxiliaryTask>,
    /// Which senders are still to be taken, in thread order.
    untaken: Vec<bool>,
    /// Where [`send`](MidiOutput::send) and [`flush`](MidiOutput::flush)
    /// may be called from: the thread that opened the port, and no
    /// other. See [`Error::MidiThread`].
    owner: ThreadId,
    /// Scratch for a drain outside the task: the task's, plus the one
    /// message [`send`](MidiOutput::send) writes behind it.
    buffer: Vec<u8>,
}

impl MidiOutput {
    /// Opens `port` for output, with room for `capacity` messages per
    /// render thread between drains.
    ///
    /// A `capacity` of 0 is read as 1, since a queue that can hold
    /// nothing is an output that can send nothing.
    ///
    /// The name is one of [`midi_ports`]. There are as many senders as
    /// [`SetupContext::thread_count`] reports, which is how many
    /// threads `render` will be called on.
    ///
    /// Opening allocates, opens a device, and creates an auxiliary
    /// task and starts its thread, so it belongs in
    /// [`setup`](crate::BelaApplication::setup) — where tasks are
    /// created anyway, and where starting that thread cannot collide
    /// with the render threads — and on the thread that will later
    /// call [`send`](MidiOutput::send) or
    /// [`flush`](MidiOutput::flush).
    ///
    /// # Errors
    ///
    /// [`Error::MidiPortName`] when `port` contains a NUL byte,
    /// [`Error::MidiCreate`] when the object could not be created —
    /// which is also what happens off the device target —
    /// [`Error::MidiOpen`] when the port could not be opened, and
    /// whatever [`AuxiliaryTask::new`] reports when the task behind the
    /// queue could not be created — [`Error::TaskCreate`] off the
    /// device target, where there is no audio system to create it in
    /// and so no output either.
    pub fn open(port: &str, context: &SetupContext, capacity: usize) -> Result<Self, Error> {
        let handle = MidiHandle::open(port)?;
        let output = Self::assemble(handle, context.thread_count(), capacity)?;
        // The first schedule of a task is what starts its thread, and
        // libbela does that by raising and restoring a priority — two
        // render threads arriving at once make it print
        // `Force starting scheduled thread didn't work` on standard
        // error. Doing it here, from `setup`, is what keeps that off
        // the audio path in a type whose whole point is to keep
        // writing off it. See `AuxiliaryTask::schedule`. The drain it
        // asks for finds empty queues and writes nothing.
        output.task.schedule(context);
        Ok(output)
    }

    /// Builds the queues and the task around an opened port.
    fn assemble(handle: MidiHandle, threads: usize, capacity: usize) -> Result<Self, Error> {
        let shared = Self::shared(handle, threads, capacity);
        // The task holds a weak reference: dropping this type closes
        // the port, and a task that outlives it — they are deleted by
        // the audio system, not by us — then has nothing to write to
        // rather than a dangling pointer.
        let weak = Arc::downgrade(&shared);
        let mut scratch = Vec::with_capacity(shared.drain_bytes());
        // Bela names the task's thread, and asks for names to be
        // unique across the system; a program holding two ports would
        // otherwise create two tasks called the same thing.
        let name = format!(
            "{DRAIN_TASK_NAME}-{}",
            NEXT_DRAIN.fetch_add(1, Ordering::Relaxed)
        );
        let task = AuxiliaryTask::new(&name, DRAIN_PRIORITY, move || {
            if let Some(shared) = weak.upgrade() {
                shared.drain(&mut scratch);
            }
        })?;
        Ok(Self::with_task(shared, task))
    }

    /// The queues, one per render thread and at least one.
    ///
    /// `open` cannot ask for none: [`SetupContext::thread_count`]
    /// reports 1 for a context that says 0, which is how a
    /// `BelaContext` can spell one render thread. The floor is here
    /// for the other caller — the tests — and to keep this total: with
    /// no queues there is no sender to hand out and
    /// [`capacity`](MidiOutput::capacity) has nothing to report.
    fn shared(handle: MidiHandle, threads: usize, capacity: usize) -> Arc<Shared> {
        Arc::new(Shared {
            midi: handle,
            queues: (0..threads.max(1)).map(|_| Queue::new(capacity)).collect(),
            draining: AtomicBool::new(false),
        })
    }

    /// The handle around queues and a task that already exist.
    fn with_task(shared: Arc<Shared>, task: AuxiliaryTask) -> Self {
        Self {
            untaken: vec![true; shared.queues.len()],
            // One message more than a drain takes, which is what
            // `send` writes after it.
            buffer: Vec::with_capacity(shared.drain_bytes() + MESSAGE_MAX),
            shared,
            task: Arc::new(task),
            owner: thread::current().id(),
        }
    }

    /// Takes the sender for render thread `thread`.
    ///
    /// [`None`] when that thread has no sender left to take, which is
    /// either because it has been taken already or because `thread` is
    /// not one of the render threads. Taking it once is what makes the
    /// queue single-writer, so this is the whole of the check.
    ///
    /// Belongs in
    /// [`create_render_state`](crate::BelaApplication::create_render_state),
    /// which is called once per thread with the number to pass.
    pub fn take_sender(&mut self, thread: usize) -> Option<MidiSender> {
        let untaken = self.untaken.get_mut(thread)?;
        if !*untaken {
            return None;
        }
        *untaken = false;
        Some(MidiSender {
            shared: Arc::clone(&self.shared),
            task: Arc::clone(&self.task),
            thread,
        })
    }

    /// How many messages one thread may queue between drains.
    #[must_use]
    pub fn capacity(&self) -> usize {
        // There is always at least one queue: `open` makes one per
        // render thread and a thread count of zero is read as one.
        self.shared.queues[0].capacity()
    }

    /// Empties the queues and sends `message` behind what was in them,
    /// in one write.
    ///
    /// For [`setup`](crate::BelaApplication::setup) and
    /// [`cleanup`](crate::BelaApplication::cleanup): a closing
    /// all-notes-off has no block left to be drained in, and this is
    /// how it still leaves. Not for a render callback — it writes to
    /// Bela's pipe itself, which is what
    /// [`MidiSender::send`](MidiSender::send) exists to keep off the
    /// audio thread.
    ///
    /// **This waits**, on the same terms as [`flush`](Self::flush): it
    /// takes the drain rather than skipping when the task has it, and
    /// keeps going while messages arrive. In `setup` and `cleanup`
    /// there is nothing rendering, so there is nothing to wait for.
    ///
    /// # Errors
    ///
    /// [`Error::MidiThread`] when called from a thread other than the
    /// one that opened the port. Bela's pipe is written through an EVL
    /// out-of-band call, which a thread EVL knows nothing about cannot
    /// make: measured on the board, such a write reports success,
    /// delivers nothing, and leaves the output stream misaligned for
    /// the rest of the run.
    pub fn send(&mut self, message: MidiMessage) -> Result<(), Error> {
        if thread::current().id() != self.owner {
            return Err(Error::MidiThread);
        }
        let (bytes, len) = message.to_bytes();
        // In the same drain as what is queued, and after it: one
        // writer at a time is what Bela's buffer asks for, and one
        // write is what puts this behind the messages already there.
        self.shared.drain_and_write(&mut self.buffer, &bytes[..len]);
        Ok(())
    }

    /// Sends everything queued, now.
    ///
    /// The same work the task does, on the calling thread. Only worth
    /// asking for where the task cannot be relied on to run — after
    /// the last block, above all.
    ///
    /// **This waits.** Two things can drain and only one at a time, so
    /// this spins until the task is done rather than skipping — two
    /// writers is the one thing the buffer under Bela's pipe does not
    /// allow. It then keeps draining until the queues are observed
    /// empty, so calling it while render threads are still sending is
    /// a loop bounded by them rather than by anything here. Call it
    /// where nothing is rendering: [`setup`](crate::BelaApplication::setup)
    /// and [`cleanup`](crate::BelaApplication::cleanup).
    ///
    /// # Errors
    ///
    /// [`Error::MidiThread`], on the same terms as
    /// [`send`](MidiOutput::send).
    pub fn flush(&mut self) -> Result<(), Error> {
        if thread::current().id() != self.owner {
            return Err(Error::MidiThread);
        }
        self.shared.drain_and_write(&mut self.buffer, &[]);
        Ok(())
    }
}

/// The name Bela gives the thread behind the drain task, before the
/// number that makes it unique.
const DRAIN_TASK_NAME: &str = "bela-rs-midi-out";

/// The number the next drain task gets.
static NEXT_DRAIN: AtomicUsize = AtomicUsize::new(0);

/// Real-time priority of that thread.
///
/// Below the audio thread, which is 95, and above the priority 1 Bela
/// gives the non-real-time thread this hands over to. A drain is a
/// memcpy and one out-of-band write, so what the number buys is how
/// soon a block's messages leave rather than how long they take.
const DRAIN_PRIORITY: Priority = Priority::new(50).expect("50 is within Bela's priority range");

/// One render thread's end of the output queue.
///
/// Held in that thread's
/// [`RenderState`](crate::BelaApplication::RenderState), taken from
/// [`MidiOutput::take_sender`]. Not [`Clone`]: one sender is one
/// writer, and that is what the queue behind it relies on.
#[derive(Debug)]
pub struct MidiSender {
    shared: Arc<Shared>,
    task: Arc<AuxiliaryTask>,
    thread: usize,
}

impl MidiSender {
    /// Queues `message` and asks for the queue to be emptied.
    ///
    /// Real-time safe, and the whole of what `render` does: one store
    /// into a ring, one index update, and a `schedule` — the same one
    /// [`AuxiliaryTask::schedule`] documents. Bela's pipe is written on
    /// the task's thread, not here.
    ///
    /// Not the same call as [`MidiOutput::send`], which shares its
    /// name and little else: that one writes to Bela's pipe itself and
    /// waits for the drain, and belongs in `setup` or `cleanup`. This
    /// one is the one a render callback may make.
    ///
    /// The context is the callback this is being sent from, as
    /// [`AuxiliaryTask::schedule`] requires.
    ///
    /// # When it leaves
    ///
    /// With the next drain, which the `schedule` here asks for. A
    /// request that arrives while the drain is running is dropped
    /// rather than queued — that is
    /// [`AuxiliaryTask::schedule`](crate::AuxiliaryTask::schedule)'s
    /// documented behaviour — so the drain looks again before it
    /// finishes, and only stops when the queues are empty.
    ///
    /// What is left is the few instructions between that last look and
    /// the callback returning. A message queued there waits for the
    /// next `send` to schedule a drain. A program that sends
    /// continuously never notices; one that sends a note off and then
    /// nothing can, which is the case to know about.
    ///
    /// # Errors
    ///
    /// [`Error::MidiQueueFull`] when this thread has already queued
    /// [`MidiOutput::capacity`] messages that have not been drained.
    /// The message is not queued, and nothing later is affected: the
    /// next drain empties what is there and the next `send` succeeds.
    /// It says the program outran the budget it declared, and nothing
    /// about the device — see [`MidiOutput`].
    pub fn send(
        &mut self,
        context: &impl CallbackContext,
        message: MidiMessage,
    ) -> Result<(), Error> {
        let (bytes, len) = message.to_bytes();
        // The index came from `take_sender`, which only hands out ones
        // it has.
        let queue = &self.shared.queues[self.thread];
        queue.push(Slot::pack(bytes, len))?;
        self.task.schedule(context);
        Ok(())
    }

    /// Which render thread this sender belongs to.
    #[must_use]
    pub const fn thread(&self) -> usize {
        self.thread
    }
}

/// What the senders, the task and the port share.
#[derive(Debug)]
struct Shared {
    /// Closed when the last of the senders, the task and the
    /// [`MidiOutput`] has let go of it.
    #[cfg_attr(
        not(bela_device),
        allow(dead_code, reason = "only the device build writes through it")
    )]
    midi: MidiHandle,
    queues: Box<[Queue]>,
    /// Held by whoever is emptying the queues.
    ///
    /// One consumer at a time is what the queues need, and there are
    /// two candidates: the task, and a [`MidiOutput::flush`] on the
    /// thread that opened the port. A drain that finds the flag set
    /// leaves the messages where they are — the other drain is about
    /// to take them.
    draining: AtomicBool,
}

impl Shared {
    /// The most bytes one drain can produce.
    fn drain_bytes(&self) -> usize {
        self.queues.iter().map(Queue::capacity).sum::<usize>() * MESSAGE_MAX
    }

    /// Whether every queue is empty.
    fn is_empty(&self) -> bool {
        self.queues.iter().all(Queue::is_empty)
    }

    /// Takes the drain, or reports that someone else has it.
    fn try_take(&self) -> bool {
        !self.draining.swap(true, Ordering::Acquire)
    }

    /// Gives it back.
    fn release(&self) {
        self.draining.store(false, Ordering::Release);
    }

    /// Empties every queue into `buffer` and writes it, with the drain
    /// already taken.
    ///
    /// `tail` goes out after what was queued, in the same write, which
    /// is what makes [`MidiOutput::send`] land behind the messages the
    /// render threads had already put in.
    fn drain_taken(&self, buffer: &mut Vec<u8>, tail: &[u8]) {
        buffer.clear();
        self.collect(buffer);
        buffer.extend_from_slice(tail);
        if !buffer.is_empty() {
            self.write(buffer);
        }
    }

    /// Takes each queue's messages, at most a queue's worth from each.
    ///
    /// The bound is what keeps one drain to the budget the program
    /// declared: without it a writer pushing while this runs can hold
    /// the loop open, and `buffer` — sized for exactly one queue's
    /// worth per thread — grows on the task's thread. What is left
    /// over is taken by the next pass, which [`drain`](Self::drain)
    /// makes sure there is one of.
    fn collect(&self, buffer: &mut Vec<u8>) {
        for queue in &self.queues {
            for _ in 0..queue.capacity() {
                let Some(slot) = queue.pop() else { break };
                let (bytes, len) = slot.unpack();
                buffer.extend_from_slice(&bytes[..len]);
            }
        }
    }

    /// Empties the queues from the task's thread, and keeps going
    /// while messages arrive.
    ///
    /// The loop is what a lost wakeup makes necessary. A `schedule`
    /// arriving while this callback runs is dropped rather than queued
    /// (see [`AuxiliaryTask::schedule`]), so a message pushed into a
    /// queue this pass has already been past would sit there with
    /// nothing left to ask for it — until whatever the program sent
    /// next, which for a note off means a note that does not stop.
    ///
    /// So the drain is given back and the queues are asked again, and
    /// only a genuinely empty set of them ends it. What remains is the
    /// few instructions between that check and the callback
    /// returning: a push landing there is carried by the next
    /// `schedule`, which is the next `send`.
    fn drain(&self, buffer: &mut Vec<u8>) {
        while self.try_take() {
            self.drain_taken(buffer, &[]);
            self.release();
            if self.is_empty() {
                return;
            }
        }
    }

    /// Empties the queues from the thread that opened the port, and
    /// writes `tail` after them.
    ///
    /// Waits for the drain rather than skipping when the task has it,
    /// because this is not a real-time context and because skipping is
    /// what would let two threads write to Bela's pipe at once — the
    /// one thing the buffer underneath it does not allow.
    ///
    /// The wait has no timeout, which is only safe because the flag
    /// cannot be left set by a thread that is gone. Deleting a task
    /// joins its thread — `AuxTaskRT::~AuxTaskRT` calls `join()`
    /// (`core/AuxTaskRT.cpp:12`), which is `thread.join()`
    /// (`core/SchedulableTask.cpp:71`) — and that thread ends by
    /// leaving the loop rather than by being stopped inside a
    /// callback: `shouldStop` is read between invocations and never
    /// during one (`core/SchedulableTask.cpp:100`, `:108`). So a
    /// callback that took the drain has run to the end, and given it
    /// back, before anything can take the task away. What it does wait for, without bound, is render threads
    /// that keep filling the queues — see [`MidiOutput::flush`].
    fn drain_and_write(&self, buffer: &mut Vec<u8>, tail: &[u8]) {
        let mut tail = tail;
        loop {
            while !self.try_take() {
                thread::yield_now();
            }
            self.drain_taken(buffer, tail);
            tail = &[];
            self.release();
            // The same lost wakeup as above: a schedule that arrived
            // while this held the drain found the task's callback
            // doing nothing and was spent.
            if self.is_empty() {
                return;
            }
        }
    }

    /// Hands bytes to Bela's output task.
    ///
    /// One call per drain rather than one per message: MIDI is a byte
    /// stream on the wire, and Bela's pipe holds records, of which
    /// there is room for far fewer than the 640 KB it appears to have.
    /// See `docs/midi.md`.
    #[cfg(bela_device)]
    fn write(&self, bytes: &[u8]) {
        let length = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
        // Safety: the pointer is live, and the length is what it
        // describes. The return value is deliberately dropped: it is 1
        // whether or not the bytes were queued, which is why the
        // budget this crate can report on is its own queue's.
        let _ = unsafe {
            bela_sys::bela_midi_write_output(self.midi.raw.as_ptr(), bytes.as_ptr(), length)
        };
    }

    #[cfg(not(bela_device))]
    #[allow(
        clippy::unused_self,
        clippy::missing_const_for_fn,
        reason = "mirrors the device signature; unreachable because no port can be opened"
    )]
    fn write(&self, _bytes: &[u8]) {}
}

/// Owns the `Midi` object, and closes it when the last holder goes.
#[derive(Debug)]
struct MidiHandle {
    #[cfg(bela_device)]
    raw: NonNull<bela_sys::BelaMidi>,
}

impl MidiHandle {
    /// Creates a `Midi` object and opens `port` on it for output.
    #[cfg(bela_device)]
    fn open(port: &str) -> Result<Self, Error> {
        let name = CString::new(port).map_err(|_| Error::MidiPortName)?;
        // Safety: the shim allocates and hands back ownership, or null.
        let raw = NonNull::new(unsafe { bela_sys::bela_midi_new() }).ok_or(Error::MidiCreate)?;
        let handle = Self { raw };
        // Safety: `raw` is the object just created, and the shim copies
        // what it needs of `name`. Dropping `handle` closes it again.
        let opened = unsafe { bela_sys::bela_midi_write_to(handle.raw.as_ptr(), name.as_ptr()) };
        if opened < 0 {
            return Err(Error::MidiOpen(opened));
        }
        Ok(handle)
    }

    /// A handle to nothing, off the device target.
    ///
    /// Succeeding here rather than failing keeps everything after it
    /// — the queues, the senders, the drain — compiled and testable on
    /// the host. Opening still fails: the drain is an
    /// [`AuxiliaryTask`], and there is no audio system to create one
    /// in.
    #[cfg(not(bela_device))]
    #[allow(
        clippy::missing_const_for_fn,
        clippy::unnecessary_wraps,
        reason = "mirrors the device signature, which opens a device and can fail"
    )]
    fn open(_port: &str) -> Result<Self, Error> {
        Ok(Self {})
    }
}

// Reached from the thread that opened the port and from the task's
// thread, one at a time — `Shared::draining` is what makes it one.
unsafe impl Send for MidiHandle {}
unsafe impl Sync for MidiHandle {}

impl Drop for MidiHandle {
    fn drop(&mut self) {
        #[cfg(bela_device)]
        // Safety: `raw` came from `bela_midi_new`, and this runs once,
        // when the last of the senders, the task's closure and the
        // MidiOutput has let go.
        unsafe {
            bela_sys::bela_midi_delete(self.raw.as_ptr());
        }
    }
}

/// The most bytes a message can take on the wire: a status byte and
/// two data bytes.
///
/// The same number as [`bela_sys::BELA_MIDI_MESSAGE_MAX`], which is
/// how many the shim writes when it reads one, and for the same
/// reason — but a bound on what this crate encodes rather than on
/// what that buffer holds, so they are asserted equal rather than
/// derived from each other.
const MESSAGE_MAX: usize = 3;

const _: () = assert!(
    MESSAGE_MAX == bela_sys::BELA_MIDI_MESSAGE_MAX,
    "what a message takes on the wire and what the shim writes have to agree"
);

const _: () = assert!(
    MESSAGE_MAX == 3,
    "a Slot holds three bytes and a length in one word, and the shifts in `pack` assume it"
);

/// A queued message: its length and up to [`MESSAGE_MAX`] bytes,
/// packed into one word so that a slot is written and read atomically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Slot(u32);

#[allow(
    clippy::cast_possible_truncation,
    reason = "the length is clamped to 3, and each byte read back is one octet of the word"
)]
impl Slot {
    /// Packs `len` bytes, with the length in the top byte.
    ///
    /// Three bytes and a length is what a word holds, and what the
    /// shifts here are written for; the assertion next to
    /// [`MESSAGE_MAX`] is what keeps that true. It is module-level
    /// rather than an associated constant here, because an associated
    /// constant nothing reads is never evaluated — an assertion that
    /// cannot fail is worse than none.
    fn pack(bytes: [u8; MESSAGE_MAX], len: usize) -> Self {
        let mut packed = (len.min(MESSAGE_MAX) as u32) << 24;
        for (index, byte) in bytes.iter().enumerate() {
            packed |= u32::from(*byte) << (16 - index * 8);
        }
        Self(packed)
    }

    /// The bytes and how many of them are the message.
    fn unpack(self) -> ([u8; MESSAGE_MAX], usize) {
        let len = (self.0 >> 24) as usize;
        let bytes = [(self.0 >> 16) as u8, (self.0 >> 8) as u8, self.0 as u8];
        (bytes, len.min(MESSAGE_MAX))
    }
}

/// A single-writer, single-reader ring of messages.
///
/// One writer is the render thread holding the [`MidiSender`]; one
/// reader is whoever holds [`Shared::draining`]. Neither waits for the
/// other, and neither allocates.
#[derive(Debug)]
struct Queue {
    slots: Box<[AtomicU32]>,
    /// How many messages have ever been written.
    write: AtomicUsize,
    /// How many have ever been read.
    read: AtomicUsize,
}

impl Queue {
    /// A queue holding `capacity` messages, and at least one.
    ///
    /// A capacity of zero is read as one, since a queue that can hold
    /// nothing is an output that can send nothing.
    ///
    /// Exactly `capacity` slots, so the counts above index them with a
    /// remainder — an integer division per message, on the render
    /// thread. It is bounded and allocates nothing, and a block is
    /// milliseconds against a handful of messages, so it stays. The
    /// way out, if a measurement ever asks for one, is to round the
    /// slots up to a power of two and mask instead, at the price of up
    /// to twice the memory and a capacity that is no longer the number
    /// of slots.
    fn new(capacity: usize) -> Self {
        Self {
            slots: (0..capacity.max(1)).map(|_| AtomicU32::new(0)).collect(),
            write: AtomicUsize::new(0),
            read: AtomicUsize::new(0),
        }
    }

    /// How many messages fit.
    fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// Whether there is nothing to take.
    ///
    /// The counts never wrap, which is what makes this answer safely
    /// from outside the drain: a `read` that is stale is smaller than
    /// the real one and can only report a queue that is empty as full
    /// of something, never the other way round. A drain that has let
    /// go therefore looks again once too often at worst, where a
    /// wrapping index could have matched by coincidence and stopped a
    /// message short.
    fn is_empty(&self) -> bool {
        self.read.load(Ordering::Relaxed) == self.write.load(Ordering::Acquire)
    }

    /// Adds a message, or reports the queue full.
    ///
    /// # Errors
    ///
    /// [`Error::MidiQueueFull`], leaving the queue as it was.
    fn push(&self, slot: Slot) -> Result<(), Error> {
        // Relaxed: this is the only writer, so nothing else moves it.
        let write = self.write.load(Ordering::Relaxed);
        // Acquire: pairs with the reader's release, so a slot it has
        // finished with is seen as free.
        if write - self.read.load(Ordering::Acquire) == self.slots.len() {
            return Err(Error::MidiQueueFull);
        }
        self.slots[write % self.slots.len()].store(slot.0, Ordering::Relaxed);
        // Release: the slot above is written before the reader can see
        // the count that offers it.
        self.write.store(write + 1, Ordering::Release);
        Ok(())
    }

    /// Takes the oldest message, or [`None`] when there is none.
    fn pop(&self) -> Option<Slot> {
        let read = self.read.load(Ordering::Relaxed);
        // Acquire: pairs with the writer's release above.
        if read == self.write.load(Ordering::Acquire) {
            return None;
        }
        let slot = Slot(self.slots[read % self.slots.len()].load(Ordering::Relaxed));
        self.read.store(read + 1, Ordering::Release);
        Some(slot)
    }
}

#[cfg(test)]
mod tests {
    #[cfg(not(bela_device))]
    use core::time::Duration;

    use crate::context::tests::Fixture;
    #[cfg(not(bela_device))]
    use crate::task::test_handle;

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
    fn a_message_survives_the_round_trip_to_bytes() {
        let messages = [
            MidiMessage::NoteOff {
                channel: MidiChannel::MAX,
                note: Note::MIDDLE_C,
                velocity: Velocity::MIN,
            },
            MidiMessage::NoteOn {
                channel: MidiChannel::MIN,
                note: Note::MAX,
                velocity: Velocity::MAX,
            },
            MidiMessage::KeyPressure {
                channel: MidiChannel::new(9).unwrap(),
                note: Note::MIDDLE_C,
                pressure: Pressure::new(64).unwrap(),
            },
            MidiMessage::ControlChange {
                channel: MidiChannel::new(1).unwrap(),
                controller: Controller::new(7).unwrap(),
                value: ControlValue::new(100).unwrap(),
            },
            MidiMessage::ProgramChange {
                channel: MidiChannel::new(2).unwrap(),
                program: Program::new(5).unwrap(),
            },
            MidiMessage::ChannelPressure {
                channel: MidiChannel::new(3).unwrap(),
                pressure: Pressure::new(80).unwrap(),
            },
            MidiMessage::PitchBend {
                channel: MidiChannel::new(4).unwrap(),
                bend: PitchBend::CENTRE,
            },
            MidiMessage::Clock,
            MidiMessage::Start,
            MidiMessage::Continue,
            MidiMessage::Stop,
            MidiMessage::ActiveSensing,
            MidiMessage::Reset,
        ];
        for message in messages {
            let (bytes, len) = message.to_bytes();
            assert_eq!(
                MidiMessage::from_bytes(&bytes[..len]),
                Some(message),
                "{message:?} came back as something else"
            );
        }
    }

    #[test]
    fn a_pitch_bend_goes_out_low_byte_first() {
        let (bytes, len) = MidiMessage::PitchBend {
            channel: MidiChannel::MIN,
            bend: PitchBend::CENTRE,
        }
        .to_bytes();
        assert_eq!(
            (&bytes[..len], len),
            ([0xE0, 0x00, 0x40].as_slice(), 3),
            "8192 is 0x00 0x40 on the wire"
        );
    }

    #[test]
    fn a_slot_carries_the_message_and_its_length() {
        for (bytes, len) in [([0x90, 60, 100], 3), ([0xC0, 5, 0], 2), ([0xF8, 0, 0], 1)] {
            let slot = Slot::pack(bytes, len);
            let (back, back_len) = slot.unpack();
            assert_eq!(back_len, len, "length changed");
            assert_eq!(&back[..len], &bytes[..len], "bytes changed");
        }
    }

    #[test]
    fn a_queue_returns_messages_in_order() {
        let queue = Queue::new(4);
        for note in 0..4u8 {
            queue
                .push(Slot::pack([0x90, note, 100], 3))
                .expect("the queue has room for four");
        }
        for note in 0..4u8 {
            let (bytes, len) = queue.pop().expect("four were pushed").unpack();
            assert_eq!((&bytes[..len], len), ([0x90, note, 100].as_slice(), 3));
        }
        assert!(queue.pop().is_none(), "and then it is empty");
    }

    #[test]
    fn a_full_queue_refuses_rather_than_overwrites() {
        let queue = Queue::new(2);
        assert_eq!(queue.capacity(), 2, "capacity is what was asked for");
        queue.push(Slot::pack([0x90, 1, 1], 3)).expect("first");
        queue.push(Slot::pack([0x90, 2, 2], 3)).expect("second");
        assert_eq!(
            queue.push(Slot::pack([0x90, 3, 3], 3)),
            Err(Error::MidiQueueFull),
            "a third does not fit"
        );

        // The oldest is still the oldest: nothing was overwritten.
        let (bytes, _) = queue.pop().expect("first is still there").unpack();
        assert_eq!(bytes[1], 1);
        queue
            .push(Slot::pack([0x90, 3, 3], 3))
            .expect("and now there is room again");
    }

    #[test]
    fn a_queue_wraps_around_its_slots() {
        let queue = Queue::new(2);
        // Three times round a two-message queue, one at a time.
        for round in 0..6u8 {
            queue.push(Slot::pack([0x90, round, 64], 3)).expect("room");
            let (bytes, _) = queue.pop().expect("just pushed").unpack();
            assert_eq!(bytes[1], round, "round {round} came back wrong");
        }
    }

    #[test]
    fn a_queue_survives_a_writer_and_a_reader_at_once() {
        use std::thread;

        const MESSAGES: u8 = 100;

        let queue = Arc::new(Queue::new(4));
        let writer = Arc::clone(&queue);
        let sender = thread::spawn(move || {
            let mut note = 0;
            while note < MESSAGES {
                if writer.push(Slot::pack([0x90, note, 64], 3)).is_ok() {
                    note += 1;
                }
            }
        });

        let mut received = Vec::new();
        while received.len() < usize::from(MESSAGES) {
            if let Some(slot) = queue.pop() {
                let (bytes, _) = slot.unpack();
                received.push(bytes[1]);
            }
        }
        sender.join().expect("the writer should not panic");

        assert_eq!(
            received,
            (0..MESSAGES).collect::<Vec<_>>(),
            "every message once, in order"
        );
    }

    /// A `MidiOutput` with no port behind it: off-device the handle
    /// opens nothing and every write is a no-op, which leaves the
    /// queues, the drain and the senders — the parts that are this
    /// crate's own — to be exercised.
    #[cfg(not(bela_device))]
    fn output(threads: usize, capacity: usize) -> MidiOutput {
        // The same two steps `assemble` takes, with a task standing in
        // for the one no off-device audio system can create — so the
        // queues and the handle are built by the code under test
        // rather than beside it.
        let handle = MidiHandle::open("nowhere").expect("no port is opened off-device");
        let shared = MidiOutput::shared(handle, threads, capacity);
        MidiOutput::with_task(shared, test_handle())
    }

    /// One message per queue, so a drain has something to order.
    #[cfg(not(bela_device))]
    fn note(number: u8) -> Slot {
        Slot::pack([0x90, number, 64], 3)
    }

    #[test]
    #[cfg(not(bela_device))]
    fn a_drain_takes_the_queues_in_thread_order() {
        let output = output(3, 4);
        // Out of order, to show the order comes from the queues.
        output.shared.queues[2].push(note(2)).unwrap();
        output.shared.queues[0].push(note(0)).unwrap();
        output.shared.queues[1].push(note(1)).unwrap();

        let mut buffer = Vec::new();
        output.shared.collect(&mut buffer);
        assert_eq!(
            buffer,
            vec![0x90, 0, 64, 0x90, 1, 64, 0x90, 2, 64],
            "thread 0's messages, then thread 1's, then thread 2's"
        );
        assert!(output.shared.is_empty(), "and nothing is left");
    }

    #[test]
    #[cfg(not(bela_device))]
    fn a_second_drain_takes_nothing_while_one_is_running() {
        let output = output(1, 4);
        output.shared.queues[0].push(note(60)).unwrap();

        assert!(output.shared.try_take(), "the drain is free to take");
        let mut buffer = Vec::new();
        // What the task does when a flush already holds it.
        output.shared.drain(&mut buffer);
        assert!(
            !output.shared.is_empty(),
            "the message belongs to whoever holds the drain"
        );

        output.shared.release();
        output.shared.drain(&mut buffer);
        assert!(output.shared.is_empty(), "and is taken once it is free");
    }

    #[test]
    #[cfg(not(bela_device))]
    fn a_drain_keeps_going_while_a_thread_is_still_pushing() {
        // The lost wakeup: a schedule that arrives while the callback
        // runs is dropped, so a drain that stops at one pass leaves
        // messages with nothing to ask for them. This is that, with
        // the pushes landing during the drain rather than after it.
        const MESSAGES: u8 = 200;

        let output = output(1, 4);
        let queue = Arc::clone(&output.shared);
        let pushing = thread::spawn(move || {
            let mut sent = 0;
            while sent < MESSAGES {
                if queue.queues[0].push(note(sent)).is_ok() {
                    sent += 1;
                }
            }
        });

        let mut buffer = Vec::new();
        let mut taken = 0;
        // Drains until the writer is done and the queues are empty,
        // which is what the loop inside `drain` has to make possible.
        while taken < usize::from(MESSAGES) {
            output.shared.drain_taken(&mut buffer, &[]);
            taken += buffer.len() / 3;
            assert!(
                buffer.len() <= output.capacity() * MESSAGE_MAX,
                "one pass took {} bytes, more than the declared budget",
                buffer.len()
            );
        }
        pushing.join().expect("the writer should not panic");
        output.shared.drain(&mut buffer);
        assert!(output.shared.is_empty(), "the drain ends with nothing left");
    }

    #[test]
    #[cfg(not(bela_device))]
    fn a_sender_is_handed_out_once() {
        let mut output = output(2, 4);
        assert!(output.take_sender(0).is_some(), "thread 0's, once");
        assert!(
            output.take_sender(0).is_none(),
            "and not again: one sender is one writer"
        );
        assert!(output.take_sender(1).is_some(), "thread 1 has its own");
        assert!(
            output.take_sender(2).is_none(),
            "and a thread that does not render has none"
        );
    }

    #[test]
    #[cfg(not(bela_device))]
    fn a_queue_that_could_hold_nothing_holds_one() {
        assert_eq!(output(1, 4).capacity(), 4, "what was asked for");
        assert_eq!(
            output(1, 0).capacity(),
            1,
            "a queue that can hold nothing is an output that can send nothing"
        );
    }

    #[test]
    #[cfg(not(bela_device))]
    fn a_sender_queues_until_the_budget_is_spent() {
        // The public way in, rather than Queue::push: a sender, a
        // render context, and the Err a program actually meets.
        let mut fixture = Fixture::new();
        let mut output = output(1, 2);
        let mut sender = output.take_sender(0).expect("thread 0's sender");

        let first = MidiMessage::NoteOn {
            channel: MidiChannel::MIN,
            note: Note::MIDDLE_C,
            velocity: Velocity::MAX,
        };
        let second = MidiMessage::Stop;
        assert_eq!(sender.send(fixture.render(0), first), Ok(()));
        assert_eq!(sender.send(fixture.render(0), second), Ok(()));
        assert_eq!(
            sender.send(fixture.render(0), MidiMessage::Clock),
            Err(Error::MidiQueueFull),
            "two is what this output was opened for"
        );

        let mut buffer = Vec::new();
        output.shared.collect(&mut buffer);
        assert_eq!(
            buffer,
            vec![0x90, 60, 127, 0xFC],
            "both messages, in the order they were sent, and not the refused one"
        );

        // And the queue is usable again once it has been drained.
        assert_eq!(sender.send(fixture.render(0), MidiMessage::Clock), Ok(()));
    }

    #[test]
    #[cfg(not(bela_device))]
    fn a_flush_empties_the_queues() {
        let mut fixture = Fixture::new();
        let mut output = output(1, 4);
        let mut sender = output.take_sender(0).expect("thread 0's sender");
        sender
            .send(fixture.render(0), MidiMessage::Clock)
            .expect("room for one");

        assert_eq!(output.flush(), Ok(()), "on the thread that opened it");
        assert!(
            output.shared.is_empty(),
            "a flush takes what the task would have"
        );
    }

    #[test]
    #[cfg(not(bela_device))]
    fn a_flush_waits_for_a_drain_it_cannot_take() {
        // Not a timing assertion: the holder publishes that it has let
        // go *before* letting go, so a flush that returned early would
        // see the flag unset.
        let mut output = output(1, 4);
        let shared = Arc::clone(&output.shared);
        let released = Arc::new(AtomicBool::new(false));
        let published = Arc::clone(&released);

        assert!(shared.try_take(), "take the drain out from under it");
        let holder = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            published.store(true, Ordering::Release);
            shared.release();
        });

        output.flush().expect("on the owning thread");
        assert!(
            released.load(Ordering::Acquire),
            "the flush returned while another drain still held it"
        );
        holder.join().expect("the holder should not panic");
    }

    #[test]
    #[cfg(not(bela_device))]
    fn a_queue_set_is_never_empty() {
        // Not a case `open` can produce — SetupContext::thread_count
        // reports 1 for a context that says 0 — but what keeps
        // `shared` total for the caller that can, which is this one.
        let output = output(0, 4);
        assert_eq!(output.shared.queues.len(), 1);
        assert_eq!(output.capacity(), 4, "and it is a usable queue");
    }

    #[test]
    #[cfg(not(bela_device))]
    fn a_send_lands_behind_what_was_queued() {
        // The point of draining and writing together: one write, with
        // the message after the ones the render threads left.
        let output = output(2, 4);
        output.shared.queues[0].push(note(1)).unwrap();
        output.shared.queues[1].push(note(2)).unwrap();

        let mut buffer = Vec::new();
        let (tail, len) = MidiMessage::Stop.to_bytes();
        output.shared.drain_taken(&mut buffer, &tail[..len]);
        assert_eq!(
            buffer,
            vec![0x90, 1, 64, 0x90, 2, 64, 0xFC],
            "both queues, then the message this call is for"
        );
    }

    #[test]
    #[cfg(not(bela_device))]
    fn only_the_thread_that_opened_the_port_may_send() {
        // The check that stands between a caller and a write that
        // reports success, delivers nothing, and misaligns everything
        // after it. MidiOutput is Send, so this is reachable.
        let mut output = output(1, 4);
        let elsewhere = thread::spawn(move || {
            let sent = output.send(MidiMessage::Stop);
            let flushed = output.flush();
            (sent, flushed)
        });
        let (sent, flushed) = elsewhere.join().expect("the thread should not panic");
        assert_eq!(sent, Err(Error::MidiThread), "send from another thread");
        assert_eq!(flushed, Err(Error::MidiThread), "and flush");
    }

    #[test]
    fn no_port_can_be_opened_for_output_off_device() {
        // The queues and the senders are built off-device; what cannot
        // exist is the task that drains them.
        let mut fixture = Fixture::with_threads(2);
        assert_eq!(
            MidiOutput::open("hw:0,0,0", fixture.setup(), 8).unwrap_err(),
            Error::TaskCreate,
            "off-device there is no audio system to create the drain task in"
        );
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
