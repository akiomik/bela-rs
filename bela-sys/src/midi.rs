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

use core::ffi::{c_char, c_int, c_uchar, c_uint};

/// An opened `Midi` object, as an opaque pointee.
///
/// Deliberately not a description of the C++ class: nothing on this
/// side may depend on its layout, and the shim is what allocates it.
#[repr(C)]
#[derive(Debug)]
pub struct BelaMidi {
    _opaque: [u8; 0],
}

/// Bytes of the longest message [`bela_midi_get_message`] writes: a
/// status byte and two data bytes.
pub const BELA_MIDI_MESSAGE_MAX: usize = 3;

unsafe extern "C" {
    /// Writes every MIDI port ALSA reports into `buf` as NUL-terminated
    /// names, one after another, and returns the bytes the whole list
    /// needs. A `len` shorter than that holds as many whole names as
    /// fit, so passing 0 asks for the size.
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

    /// Opens `port` for input and starts reading from it. Returns 0
    /// when input is enabled afterwards, a negative value otherwise.
    pub fn bela_midi_read_from(midi: *mut BelaMidi, port: *const c_char) -> c_int;

    /// Opens `port` for output. Returns 0 when output is enabled
    /// afterwards, a negative value otherwise.
    pub fn bela_midi_write_to(midi: *mut BelaMidi, port: *const c_char) -> c_int;

    /// How many parsed messages are waiting. Reads two ring indices:
    /// no allocation, no system call.
    pub fn bela_midi_available_messages(midi: *mut BelaMidi) -> c_int;

    /// Writes the oldest waiting message into `buf`, which must have
    /// room for [`BELA_MIDI_MESSAGE_MAX`] bytes, and returns how many
    /// bytes it wrote — 0 when nothing was waiting, leaving `buf`
    /// untouched. The status byte carries the channel in its low
    /// nibble, as on the wire.
    pub fn bela_midi_get_message(midi: *mut BelaMidi, buf: *mut c_uchar) -> c_uint;

    /// Hands `length` bytes to Bela's output task. Returns 1, or 0 if
    /// output was never enabled — and 1 says the bytes were handed
    /// over, not that they were queued or sent.
    ///
    /// Not to be called from `render`: on a full pipe the path below
    /// this prints to `stderr` from the calling thread.
    pub fn bela_midi_write_output(
        midi: *mut BelaMidi,
        bytes: *const c_uchar,
        length: c_uint,
    ) -> c_int;
}
