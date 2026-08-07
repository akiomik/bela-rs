//! The C surface over Bela's `Midi` class.
//!
//! Unlike the rest of this crate, these declarations are not generated:
//! the C they describe is `shim/midi.h` in this crate, compiled by
//! `build.rs` and linked into device binaries alongside
//! `libbelaextra`. That header is the contract, this module mirrors it
//! by hand, and the two have to be edited together.
//!
//! Bela ships a C surface of its own in `libraries/Midi/Midi_c.h`,
//! which the shim replaces rather than extends; `docs/midi.md` in the
//! repository records why, along with what the class does on the audio
//! thread and what it does not report.
//!
//! Off the device target the shim is not compiled, so these symbols do
//! not resolve. Declaring them anyway keeps the module compiling
//! everywhere the rest of the crate does.
//!
//! Every function takes a `midi` from [`bela_midi_new`] that has not
//! been deleted, and a non-null `port` or `buf` where it takes one.
//! Nothing checks; [`bela_midi_delete`] is the one that also accepts
//! null.

use core::ffi::{c_char, c_int, c_uchar, c_uint};
use core::marker::{PhantomData, PhantomPinned};

/// An opened `Midi` object, as an opaque pointee.
///
/// Deliberately not a description of the C++ class: nothing on this
/// side may depend on its layout, and the shim is what allocates it.
///
/// The marker is what the nomicon asks of an opaque type: a bare
/// zero-length array would make this `Send`, `Sync` and `Unpin`, and
/// the object behind it is none of those — it owns a thread that reads
/// the port, and its address is in the C++ object's own members.
#[repr(C)]
#[derive(Debug)]
pub struct BelaMidi {
    _data: [u8; 0],
    _marker: PhantomData<(*mut u8, PhantomPinned)>,
}

/// Bytes of the longest message [`bela_midi_get_message`] writes: a
/// status byte and two data bytes.
pub const BELA_MIDI_MESSAGE_MAX: usize = 3;

/// No port has the name that was given.
///
/// The names are [`bela_midi_list_ports`]'s, which carry the
/// subdevice. Far outside the `errno` range on purpose: `-1` would be
/// this and `EPERM` at once.
pub const BELA_MIDI_NO_SUCH_PORT: c_int = -1000;

/// That direction of this object is already open.
///
/// Bela's `inputEnabled` and `outputEnabled` are set once and never
/// cleared, so a second open cannot be judged by them; it is refused
/// rather than allowed to leak an ALSA handle and start a second
/// reader thread.
pub const BELA_MIDI_ALREADY_OPEN: c_int = -1001;

unsafe extern "C" {
    /// Writes every MIDI port ALSA reports into `buf` as NUL-terminated
    /// names, one after another, and returns the bytes the whole list
    /// needs. A `len` shorter than that holds as many whole names as
    /// fit. `buf` may be null when `len` is 0, which is how to ask for
    /// the size before allocating.
    ///
    /// 0 means there are no ports, and also means the query threw;
    /// the two are not told apart, because the C++ underneath answers
    /// an ALSA failure with a partial list rather than an error.
    ///
    /// The names carry card, device *and* subdevice — `hw:0,0,0` where
    /// `amidi -l` prints `hw:0,0` — and [`bela_midi_read_from`] and
    /// [`bela_midi_write_to`] match against exactly these.
    ///
    /// Allocates and reads the ALSA control interface.
    pub fn bela_midi_list_ports(buf: *mut c_char, len: c_uint) -> c_uint;

    /// Creates a `Midi` object with its input parser enabled, opening
    /// no port. Returns null if it could not be created.
    pub fn bela_midi_new() -> *mut BelaMidi;

    /// Destroys a `Midi` object, joining its input thread. Null is
    /// accepted. Blocks for as long as that thread takes to notice,
    /// which is up to its 50 ms poll timeout.
    pub fn bela_midi_delete(midi: *mut BelaMidi);

    /// Opens `port` for input and starts reading from it, once per
    /// object. Returns 0 when input is enabled afterwards,
    /// [`BELA_MIDI_NO_SUCH_PORT`], [`BELA_MIDI_ALREADY_OPEN`], or a
    /// negative value from Bela — `-errno` when ALSA refused the
    /// device.
    ///
    /// A failure ends the object: Bela can fail with the ALSA device
    /// already open and the flag the guard reads still false, so a
    /// retry opens a second device over the first. Delete it and make
    /// another.
    pub fn bela_midi_read_from(midi: *mut BelaMidi, port: *const c_char) -> c_int;

    /// Opens `port` for output, once per object, with the same return
    /// values as [`bela_midi_read_from`] and the same end after a
    /// failure.
    pub fn bela_midi_write_to(midi: *mut BelaMidi, port: *const c_char) -> c_int;

    /// How many parsed messages are waiting. Reads two ring indices:
    /// no allocation, no system call — and no synchronisation either,
    /// the input thread writing them as plain `unsigned int`s. What
    /// that can cost is a count one message stale.
    pub fn bela_midi_available_messages(midi: *mut BelaMidi) -> c_int;

    /// Writes the oldest waiting message into `buf`, which must have
    /// room for [`BELA_MIDI_MESSAGE_MAX`] bytes, and returns how many
    /// bytes it wrote — 0 when nothing was waiting, leaving `buf`
    /// untouched. The status byte carries the channel in its low
    /// nibble, as on the wire.
    pub fn bela_midi_get_message(midi: *mut BelaMidi, buf: *mut c_uchar) -> c_uint;

    /// Hands `length` bytes to Bela's output task. Returns 1, or 0 if
    /// output was never enabled — and 1 says the bytes were handed
    /// over, not that they were queued or sent. The `-1` in the C++ is
    /// unreachable while `commsSend` reports success unconditionally,
    /// which is what makes 1 and 0 the whole range.
    ///
    /// Not to be called from `render`: on a full pipe the path below
    /// this prints to `stderr` from the calling thread.
    pub fn bela_midi_write_output(
        midi: *mut BelaMidi,
        bytes: *const c_uchar,
        length: c_uint,
    ) -> c_int;
}
