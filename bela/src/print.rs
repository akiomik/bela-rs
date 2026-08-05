//! Real-time safe printing from the audio thread.
//!
//! [`rt_println!`](crate::rt_println) and [`rt_print!`](crate::rt_print)
//! take `format!`-style arguments, render them into a fixed-size buffer
//! on the stack and hand that buffer to Bela's real-time printing
//! function. Nothing allocates, so they are usable from `render`, where
//! `println!` is forbidden.

use core::ffi::CStr;
use core::fmt::{self, Write};

/// Maximum length in bytes of a single message, excluding the
/// terminator.
///
/// Longer messages are truncated on a `char` boundary and marked with
/// `...`; see [`print_args`].
pub const MESSAGE_CAPACITY: usize = 256;

/// Appended to a message that did not fit, so that truncation is
/// visible in the output rather than silent.
const TRUNCATION_MARKER: &str = "...";

/// A formatted message being assembled in a fixed-size stack buffer.
///
/// The buffer holds [`MESSAGE_CAPACITY`] bytes of text plus the NUL the
/// C side needs.
struct Message {
    bytes: [u8; MESSAGE_CAPACITY + 1],
    len: usize,
    truncated: bool,
}

impl Message {
    const fn new() -> Self {
        Self {
            bytes: [0; MESSAGE_CAPACITY + 1],
            len: 0,
            truncated: false,
        }
    }

    /// Appends as much of `s` as fits, cutting on a `char` boundary and
    /// recording that something was dropped.
    fn append(&mut self, s: &str) {
        let room = MESSAGE_CAPACITY - self.len;
        let end = if s.len() <= room {
            s.len()
        } else {
            self.truncated = true;
            floor_char_boundary(s.as_bytes(), room)
        };
        self.bytes[self.len..self.len + end].copy_from_slice(&s.as_bytes()[..end]);
        self.len += end;
    }

    /// Terminates the message, appending the truncation marker and an
    /// optional newline, and returns it ready to pass to C.
    ///
    /// Both are made to fit by giving back text from the end of the
    /// message, so the result never exceeds [`MESSAGE_CAPACITY`].
    fn finish(&mut self, newline: bool) -> &CStr {
        let newline_len = usize::from(newline);
        // A newline that does not fit is itself a form of truncation,
        // and one worth marking: without it the next message would run
        // into this one.
        if self.len + newline_len > MESSAGE_CAPACITY {
            self.truncated = true;
        }
        let tail = if self.truncated {
            TRUNCATION_MARKER.len() + newline_len
        } else {
            newline_len
        };
        if self.len + tail > MESSAGE_CAPACITY {
            self.len = floor_char_boundary(&self.bytes[..self.len], MESSAGE_CAPACITY - tail);
        }
        if self.truncated {
            self.push(TRUNCATION_MARKER.as_bytes());
        }
        if newline {
            self.push(b"\n");
        }
        self.bytes[self.len] = 0;
        self.as_c_str()
    }

    /// Appends bytes that are already known to fit.
    fn push(&mut self, bytes: &[u8]) {
        self.bytes[self.len..self.len + bytes.len()].copy_from_slice(bytes);
        self.len += bytes.len();
    }

    /// The message as a C string.
    ///
    /// A message containing an interior NUL ends there, which is what
    /// the C side would do with it anyway.
    fn as_c_str(&self) -> &CStr {
        // The buffer is always terminated by `finish`, and is zeroed
        // before that, so this cannot fail.
        CStr::from_bytes_until_nul(&self.bytes).unwrap_or(c"")
    }
}

impl Write for Message {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.append(s);
        // Once the buffer is full there is nothing left to write, and
        // the error is how `write_fmt` is told to stop. That matters on
        // the audio thread: the formatting machinery writes a piece at a
        // time, so padding a field to a huge width — `{:width$}` with
        // `width = 65_535` — is tens of thousands of calls that would
        // otherwise all run and all discard their input. `emit` ignores
        // this expected error and prints what was assembled.
        if self.truncated {
            Err(fmt::Error)
        } else {
            Ok(())
        }
    }
}

/// Rounds `index` down to a `char` boundary of the UTF-8 `bytes`.
fn floor_char_boundary(bytes: &[u8], index: usize) -> usize {
    let mut index = index;
    // Continuation bytes are 0b10xx_xxxx; a boundary is anything else.
    while index > 0 && index < bytes.len() && bytes[index] & 0b1100_0000 == 0b1000_0000 {
        index -= 1;
    }
    index
}

/// Prints formatted arguments in a real-time safe way, without a
/// trailing newline.
///
/// This is what [`rt_print!`](crate::rt_print) expands to; call it
/// directly with [`format_args!`] if a macro is inconvenient.
///
/// Messages longer than [`MESSAGE_CAPACITY`] bytes are truncated on a
/// `char` boundary and end with `...`.
pub fn print_args(args: fmt::Arguments<'_>) {
    emit(args, false);
}

/// Prints formatted arguments in a real-time safe way, followed by a
/// newline.
///
/// This is what [`rt_println!`](crate::rt_println) expands to; see
/// [`print_args`] for the details.
pub fn println_args(args: fmt::Arguments<'_>) {
    emit(args, true);
}

fn emit(args: fmt::Arguments<'_>, newline: bool) {
    let mut message = Message::new();
    // An `Err` here means either that the message filled the buffer or
    // that a `Display` implementation failed on its own. Both leave a
    // partial message worth printing, and truncation is reported by the
    // marker `finish` appends.
    let _ = message.write_fmt(args);
    write_c_str(message.finish(newline));
}

/// Hands the finished message to Bela.
///
/// `Bela_printf` rather than `rt_printf`: the header describes the
/// `Bela_*` functions as the future-proof wrappers, and on the EVL
/// backend both end up in the same real-time print proxy. The message
/// is passed as an argument to a literal `%s` format, so text
/// containing `%` cannot be interpreted as a format specifier.
#[cfg(bela_device)]
fn write_c_str(message: &CStr) {
    // Safety: both pointers are valid NUL-terminated C strings, and the
    // format string takes exactly the one `*const c_char` argument
    // given.
    let _ = unsafe { bela_sys::Bela_printf(c"%s".as_ptr(), message.as_ptr()) };
}

/// Off-device fallback: the same bytes, printed the ordinary way.
///
/// Keeps application code that prints compiling and behaving sensibly
/// on the host, with the same truncation, so what is tested there is
/// what the board prints. Nothing here is real-time safe, and nothing
/// off-device is.
#[cfg(not(bela_device))]
fn write_c_str(message: &CStr) {
    use std::io::{Write as _, stdout};

    let mut out = stdout();
    let _ = out.write_all(message.to_bytes());
    let _ = out.flush();
}

/// Prints to the Bela console in a real-time safe way, with
/// `format!`-style arguments and no trailing newline.
///
/// Usable from `render`: the message is formatted into a fixed-size
/// stack buffer, so nothing allocates or blocks. Formatting still costs
/// time on the audio thread, so print sparingly — once every N blocks
/// rather than every block, and never per frame.
///
/// Messages longer than [`MESSAGE_CAPACITY`] bytes are truncated on a
/// `char` boundary and end with `...`.
///
/// ```no_run
/// use bela::{BelaApplication, RenderContext, rt_print};
/// # use bela::{SetupContext, ThreadInfo};
///
/// # struct App;
/// # impl BelaApplication for App {
/// # type RenderState = ();
/// # fn create_render_state(&mut self, _t: ThreadInfo, _c: &SetupContext) {}
/// fn render(&self, _state: &mut (), context: &mut RenderContext) {
///     rt_print!("underruns so far: {}", context.underrun_count());
/// }
/// # }
/// ```
#[macro_export]
macro_rules! rt_print {
    ($($arg:tt)*) => {
        $crate::print_args(::core::format_args!($($arg)*))
    };
}

/// Prints to the Bela console in a real-time safe way, with
/// `format!`-style arguments and a trailing newline.
///
/// See [`rt_print!`] for the details; this is the form to reach for,
/// since the Bela console is line-buffered.
///
/// ```no_run
/// use bela::{BelaApplication, SetupContext, rt_println};
/// # use bela::{RenderContext, ThreadInfo};
///
/// # struct App;
/// # impl BelaApplication for App {
/// # type RenderState = ();
/// # fn create_render_state(&mut self, _t: ThreadInfo, _c: &SetupContext) {}
/// fn setup(&mut self, context: &SetupContext) -> bool {
///     rt_println!("{} Hz, {} frames", context.audio_sample_rate(), context.audio_frames());
///     true
/// }
/// # fn render(&self, _s: &mut (), _c: &mut RenderContext) {}
/// # }
/// ```
#[macro_export]
macro_rules! rt_println {
    () => {
        $crate::println_args(::core::format_args!(""))
    };
    ($($arg:tt)*) => {
        $crate::println_args(::core::format_args!($($arg)*))
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Formats like the macros do, but returns the bytes instead of
    /// printing them.
    fn format(args: fmt::Arguments<'_>, newline: bool) -> String {
        let mut message = Message::new();
        let _ = message.write_fmt(args);
        String::from_utf8(message.finish(newline).to_bytes().to_vec())
            .expect("messages are truncated on char boundaries, so they stay valid UTF-8")
    }

    #[test]
    fn formats_arguments() {
        assert_eq!(format(format_args!("x = {}", 42), false), "x = 42");
    }

    #[test]
    fn appends_a_newline_when_asked() {
        assert_eq!(format(format_args!("done"), true), "done\n");
    }

    #[test]
    fn a_message_of_exactly_the_capacity_is_left_alone() {
        let text = "a".repeat(MESSAGE_CAPACITY);
        assert_eq!(format(format_args!("{text}"), false), text);
    }

    #[test]
    fn a_message_over_the_capacity_is_marked_as_truncated() {
        let formatted = format(format_args!("{}", "a".repeat(MESSAGE_CAPACITY + 10)), false);
        assert_eq!(formatted.len(), MESSAGE_CAPACITY);
        assert!(
            formatted.ends_with(TRUNCATION_MARKER),
            "truncation should be visible: {formatted}"
        );
    }

    #[test]
    fn a_truncated_line_still_ends_with_a_newline() {
        let formatted = format(format_args!("{}", "a".repeat(MESSAGE_CAPACITY + 10)), true);
        assert_eq!(formatted.len(), MESSAGE_CAPACITY);
        assert!(
            formatted.ends_with("...\n"),
            "a truncated line should end with the marker and a newline: {formatted}"
        );
    }

    #[test]
    fn a_newline_that_does_not_fit_truncates_the_text() {
        let formatted = format(format_args!("{}", "a".repeat(MESSAGE_CAPACITY)), true);
        assert_eq!(formatted.len(), MESSAGE_CAPACITY);
        assert!(
            formatted.ends_with("...\n"),
            "the newline has to displace text: {formatted}"
        );
    }

    #[test]
    fn multi_byte_characters_are_not_split() {
        // 3 bytes each, so the last one straddles the capacity.
        let text = "あ".repeat(MESSAGE_CAPACITY / 3 + 1);
        let formatted = format(format_args!("{text}"), false);
        assert!(
            formatted.ends_with(TRUNCATION_MARKER),
            "expected a truncated message: {formatted}"
        );
        // Getting here at all means the bytes were valid UTF-8; the
        // count confirms no partial character was written.
        let characters = formatted
            .trim_end_matches(TRUNCATION_MARKER)
            .chars()
            .count();
        assert_eq!(characters * 3, formatted.len() - TRUNCATION_MARKER.len());
    }

    #[test]
    fn formatting_stops_once_the_buffer_is_full() {
        /// Passes writes through to a [`Message`], counting them.
        struct Counting<'a> {
            message: &'a mut Message,
            calls: usize,
        }

        impl Write for Counting<'_> {
            fn write_str(&mut self, s: &str) -> fmt::Result {
                self.calls += 1;
                self.message.write_str(s)
            }
        }

        // Padding is written one `char` at a time, so a field this wide
        // is 65_535 calls if nothing stops them.
        let mut message = Message::new();
        let mut counting = Counting {
            message: &mut message,
            calls: 0,
        };
        let _ = counting.write_fmt(format_args!("{:width$}", "", width = 65_535));
        let calls = counting.calls;
        assert!(
            calls < 2 * MESSAGE_CAPACITY,
            "the work should be bounded by the capacity, not by the width, but took {calls} writes"
        );
        // And what did fit is still printed, marked as truncated.
        let formatted = message.finish(false).to_bytes();
        assert_eq!(formatted.len(), MESSAGE_CAPACITY);
        assert!(formatted.ends_with(TRUNCATION_MARKER.as_bytes()));
    }

    #[test]
    fn a_message_ends_at_an_interior_nul() {
        assert_eq!(format(format_args!("before\0after"), false), "before");
    }

    #[test]
    fn percent_signs_are_not_format_specifiers() {
        // The C side is given a literal "%s", so this is passed
        // through as text rather than consuming an argument.
        assert_eq!(format(format_args!("100%s of it"), false), "100%s of it");
    }
}
