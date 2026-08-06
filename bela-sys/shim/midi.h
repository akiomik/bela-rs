/* A C surface over Bela's Midi class, for bela-sys.
 *
 * Bela ships one of its own in libraries/Midi/Midi_c.h, and this is
 * that file written again: it covers input only, it drops the return
 * value of readFrom() so a port that failed to open is indistinguishable
 * from one that opened, and it enables the parser after starting the
 * thread that reads the flag. See ../../docs/midi.md for the whole
 * reckoning, including why output does not go through here from render.
 *
 * The Midi class is LGPL 3.0 (libraries/Midi/lib.metadata); these crates
 * are MIT OR Apache-2.0 and reach it by linking libbelaextra.so
 * dynamically. This file calls that class and is compiled by
 * ../build.rs when the sysroot carries its sources.
 */
#pragma once

#ifdef __cplusplus
extern "C" {
#endif

/* An opened Midi object. Created by bela_midi_new, and not the same
 * type as Bela's `Midi`: it is deliberately incomplete so that no
 * caller can depend on a C++ layout. */
typedef struct BelaMidi BelaMidi;

/* The largest message bela_midi_get_message writes: a status byte and
 * the two data bytes of the longest channel message. */
#define BELA_MIDI_MESSAGE_MAX 3

/* Why a port could not be opened, where ALSA had no say.
 *
 * Everything else a port-opening call returns is `-errno` as ALSA
 * reported it, so these sit far outside the errno range rather than at
 * -1: -1 would be both "no port by that name" and EPERM. */
#define BELA_MIDI_NO_SUCH_PORT (-1000)
#define BELA_MIDI_ALREADY_OPEN (-1001)

/* Except where a function says otherwise, `midi` must be a pointer
 * from bela_midi_new that has not been passed to bela_midi_delete, and
 * `port` and `buf` must not be NULL. Nothing here checks. */

/* Writes the names of every MIDI port ALSA reports into `buf` as
 * NUL-terminated strings, one after another, and returns the number of
 * bytes the whole list needs — which may be more than `len`, in which
 * case `buf` holds as many whole names as fit.
 *
 * The names are Bela's, and Bela's are card, device *and* subdevice:
 * `hw:0,0,0` where `amidi -l` prints `hw:0,0`. Midi::readFrom and
 * Midi::writeTo compare the string they are given against this list, so
 * the two-number form opens nothing.
 *
 * A return of 0 means there are no ports, and also means the query
 * itself threw. Those are not told apart, and there is nothing finer
 * to report: Midi::listAllPorts answers an ALSA failure by printing
 * and returning the ports it had collected so far, so a partial list
 * already looks like a complete one. What remains for the exception
 * path is allocation failure.
 *
 * Allocates and reads the ALSA control interface; not for render. */
unsigned int bela_midi_list_ports(char *buf, unsigned int len);

/* Creates a Midi object with its input parser enabled, and opens
 * nothing. Returns NULL if the object could not be created.
 *
 * The parser is enabled here, before any port is opened, because
 * Midi::enableParser writes a flag that the input thread reads: the
 * order Midi_c uses (readFrom, then enableParser) sets it under a
 * running reader. Nothing turns it off again, which also keeps
 * enableParser(false) — a delete that leaves the pointer behind — out
 * of reach.
 *
 * A callback that discards system exclusive bytes is set at the same
 * time. Sysex never reaches the message ring, and a parser with no
 * sysex callback prints every byte of it to the console instead.
 *
 * The parser it creates is never destroyed: Midi::cleanup() frees the
 * output task and the ALSA handles but not the parser, so each object
 * costs about 2.4 KB that bela_midi_delete does not give back. Calling
 * enableParser(false) would free it and is not an option — it deletes
 * before it clears the flag the input thread is reading. One object
 * per port for the life of a program is what this is sized for. */
BelaMidi *bela_midi_new(void);

/* Destroys a Midi object, joining its input thread. NULL is accepted
 * and does nothing.
 *
 * This blocks for as long as the input thread takes to notice: it
 * polls with a 50 ms timeout. */
void bela_midi_delete(BelaMidi *midi);

/* Opens `port` for input and starts reading from it.
 *
 * **Once per object.** Midi's own `inputEnabled` never goes false
 * again, so a second call cannot be judged by it — and Bela's readFrom
 * would overwrite the ALSA handle and start a second reader thread on
 * the same object. A second call is refused with
 * BELA_MIDI_ALREADY_OPEN instead.
 *
 * Returns 0 when input is enabled afterwards,
 * BELA_MIDI_NO_SUCH_PORT when no port has that name — the names are
 * bela_midi_list_ports's, which carry the subdevice —
 * BELA_MIDI_ALREADY_OPEN as above, and otherwise a negative value from
 * Bela, which is `-errno` when ALSA refused the device. That input is
 * open at all is read back from Midi::isInputEnabled rather than taken
 * from readFrom, whose 1 and -1 each cover more than one case. */
int bela_midi_read_from(BelaMidi *midi, const char *port);

/* Opens `port` for output.
 *
 * **Once per object**, on the same terms as bela_midi_read_from, and
 * with the same return values. Reading Midi::isOutputEnabled is what
 * makes the success answer trustworthy: writeTo returns 1 both when it
 * succeeded and when the port does not exist, and in the second case
 * every later write is discarded by a check the caller cannot see. */
int bela_midi_write_to(BelaMidi *midi, const char *port);

/* Returns how many parsed messages are waiting, or 0 if there is no
 * parser. Reads two indices of a ring the input thread writes: no
 * allocation, no system call, safe to call from render.
 *
 * "Safe" here means real-time safe and single-reader. The two indices
 * are plain unsigned ints that the input thread writes without
 * synchronisation, which is a data race by the letter of C++ and is
 * how Bela's own examples read this ring; what it can cost is a count
 * one message stale, never a torn pointer, on a target where those
 * loads are atomic. */
int bela_midi_available_messages(BelaMidi *midi);

/* Writes the oldest waiting message into `buf`, which must have room
 * for BELA_MIDI_MESSAGE_MAX bytes, and returns how many bytes were
 * written.
 *
 * Returns 0 when nothing was waiting, leaving `buf` untouched. Bela's
 * version answers that case with a one-byte message built out of a
 * cleared record, which cannot be told from a real one.
 *
 * One reader only: taking a message advances a pointer nothing
 * synchronises. Same data-race note as above.
 *
 * The status byte carries the channel in its low nibble, as it does on
 * the wire. Safe to call from render, on the same terms as
 * bela_midi_available_messages. */
unsigned int bela_midi_get_message(BelaMidi *midi, unsigned char *buf);

/* Hands `length` bytes to Bela's output task and returns 1, or 0 if
 * output was never enabled.
 *
 * **Not for render.** The bytes reach a pipe whose failures nothing
 * reports, on a path that prints to stderr from the calling thread
 * when it is full; the crate calls this from an auxiliary task
 * instead. docs/midi.md is the whole argument.
 *
 * The value is passed through from Midi::writeOutput rather than
 * folded the way the port-opening calls are, because there is nothing
 * to fold: the -1 in that function is unreachable while
 * AuxTaskNonRT::commsSend reports success unconditionally, so 1 and 0
 * are the whole range. If that is ever fixed upstream, -1 becomes
 * reachable and this contract changes with it — see docs/midi.md,
 * finding 4.
 *
 * So a caller should know the return value is not evidence: 1 means
 * the bytes were handed over, not that they were queued, and certainly
 * not that they were sent. */
int bela_midi_write_output(BelaMidi *midi, const unsigned char *bytes,
                           unsigned int length);

#ifdef __cplusplus
} /* extern "C" */
#endif
