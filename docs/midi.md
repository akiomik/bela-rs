# MIDI

What this crate wraps to reach MIDI on a Bela Gem, and why the output
path does not call Bela's own output function from `render`.

Everything here is **read from the sources on the board** (Bela 1.18.0,
Debian Bookworm image 2026-03-25, EVL real-time core), as synced into
the sysroot by `scripts/sync-sysroot.sh`; line numbers are into that
copy. Where something was run rather than read, it says so.

Those sources are not upstream's. The board's checkout is `fb362a5`
with an image overlay on top, and `include/Bela.h` there reports a
version no published branch carries (see
[board-facts.md](board-facts.md)). So a line number here will not land
on the same line in [BelaPlatform/Bela], and a difference between the
two is the overlay rather than a mistake. Checking one takes a synced
sysroot, which is git-ignored, so it is a board and
`scripts/sync-sysroot.sh` away rather than a click away.

[BelaPlatform/Bela]: https://github.com/BelaPlatform/Bela

## Summary

- MIDI comes from **Bela's `Midi` class**, reached through a small C++
  shim this workspace compiles, rather than from ALSA's rawmidi API
  directly.
- **Input** is Bela's as it stands: an input thread of Bela's making,
  a parser ring that `render` drains. The crate adds nothing to it.
- **Output** does not call `Midi::writeOutput` from `render`. The crate
  holds a queue of its own, and an
  [`AuxiliaryTask`](../bela/src/task.rs) drains it into `writeOutput`
  off the audio thread.

The first two follow from what Bela already does well. The third goes
against the grain of the first two — Bela's output path is *designed*
to be called from `render` — so most of this file is the argument for
it.

## Bela's `Midi` rather than ALSA rawmidi

The sysroot ships `alsa/rawmidi.h`, so talking to the device directly
is available with no C++ at all. What that costs is everything
`libraries/Midi/` already does beyond opening a port:

- an `RtThread` running the input read loop (`Midi.cpp:338`),
- an `AuxTaskNonRT` carrying output off the audio thread
  (`Midi.cpp:364`),
- a `MidiParser` with a 100-message ring (`Midi.h:155`),
- `attemptRecoveryRead()` / `attemptRecoveryWrite()`, for a device
  unplugged and plugged back in again mid-run.

Those four are the real-time design, and Bela maintains them. Writing
them again to avoid a C++ boundary is the wrong trade for this crate,
which exists to make Bela reachable from Rust rather than to replace
it. The same reasoning is why the shim calls the class rather than
reimplementing its output side (see [Not the third
way](#not-the-third-way)).

Practical consequences of the choice:

- The symbols live in `libbelaextra.so`, not `libbela.so`, so device
  builds link one more library. Its `DT_NEEDED` entries are
  `libstdc++.so.6`, `libseasocks.so`, `libasound.so.2`,
  `libNE10.so.10`, `libm.so.6`, `libgcc_s.so.1` and `libc.so.6`, and
  all of them already resolve inside the sysroot: every one but
  `libseasocks.so` is in the multiarch directory, and that one is in
  `/usr/local/lib`, which `bela-sys/build.rs` already searches and
  `scripts/aarch64-bela-linker.sh` already passes to `-rpath-link`. So
  nothing else changes.
- `libraries/Midi/lib.metadata` declares the class **LGPL 3.0**, where
  these crates are MIT OR Apache-2.0. Dynamic linking against
  `libbelaextra.so` is the ordinary arrangement for that; linking
  `libbelaextra.a` would not be, and this workspace does not.
- Half a C surface already exists — `Midi_c.cpp` exports `Midi_new`,
  `Midi_delete`, `Midi_availableMessages`, `Midi_getMessage` — but it
  covers input only, drops `readFrom`'s return value, and enables the
  parser *after* starting the input thread (`Midi_c.cpp:19`, against a
  loop already reading `parserEnabled`). The shim here is that file
  written again with those fixed, plus output.

## What the output path does on this board

```
render() → Midi::writeOutput(bytes, len)                  Midi.cpp:512
         → midiOutputTask->schedule(bytes, size)          Midi.cpp:519
         → SchedulableTask::schedule()                    core/SchedulableTask.cpp:53
         → AuxTaskNonRT::commsSend()                      core/AuxTaskNonRT.cpp:19
         → RtNonRtMsgFifo::writeRt()                      core/RtMsgFifo.cpp:567
              ├── payload → CircularBuffer fromRt                    :572
              └── header  → oob_write(xbuf, BufMsg)                  :578
   ─────────────────────── EVL boundary ───────────────────────
   AuxTaskNonRT's thread → Midi::doWriteOutput() → snd_rawmidi_write()
```

No ALSA call happens on the caller's thread: `writeTo()` creates one
`AuxTaskNonRT` and `writeOutput` only hands bytes to it. That much is
what the class advertises, and it is true. Five things about it are
not visible from the headers.

**1. The queue is 1 KB of records, not 640 KB of bytes.**
`AuxTaskNonRT::commsInit` asks for `65536 * 10`
(`core/AuxTaskNonRT.cpp:14`), and on the EVL build `createBelaRtPipe`
discards that: `poolsz = 1024` immediately before `evl_create_xbuf`
(`core/RtMsgFifo.cpp:392`). The 640 KB goes to the `CircularBuffer`
pair instead (`p = new Private(size)`, `core/RtMsgFifo.cpp:461`), which
is where payloads travel; the pipe carries one `BufMsg` per message,
and a `BufMsg` is one `uint32_t` (`core/RtMsgFifo.cpp:12`).

So the queue is bounded by **256 records** rather than by MIDI bytes —
1024 / 4, assuming the xbuf charges nothing per message, which is not
established here. The line after the allocation reads

```c
// core/RtMsgFifo.cpp:462
size = sizeof(BufMsg) * 64; // pretty arbitrary innit
```

and is dead: `size` is a by-value parameter and `createBelaRtPipe` was
called with it eleven lines earlier. It says 64 was the intended depth
and 256 is the one in effect, which is the same order of magnitude and
the same conclusion.

Against that, the payload ring holds 640 KB. A three-byte MIDI message
is 4 bytes of header and 3 of payload, so the pipe runs out about 850
times sooner: roughly 256 messages against roughly 218000. **For MIDI
the pipe is the limit and the ring never fills**, which decides which
of the next two findings is the one that happens.

**2. The failure path prints from the thread that called it.**

```c
// core/RtMsgFifo.cpp:572
ssize_t ret = p->fromRt.write(ptr, size);
if(ret != size) {
	fprintf(stderr, "RtNonRtMsgFifo::writeRt() failed %zd\n", ret);
	return false;
}
```

`fprintf`, not the `rt_fprintf` used one frame above it in
`commsSend`. On the audio thread that is a mode switch out of the
real-time domain, in exactly the situation the caller is least able to
afford one.

It fires when the **payload ring** fills, though, which by finding 1 is
the thing that does not happen for messages this short. The ring is
reached first only above 640 KB / 256 records — **2560 bytes per
message on average** — so this is what a program sending long system
exclusive dumps would meet, and what a MIDI program meets is finding 3,
which prints nothing at all.

**3. Losing a header desynchronizes the stream, and it is the first
thing to go.** `writeRt` writes the payload to the ring first and the
header to the pipe second, and the reader does the reverse: `oob_read`
one `BufMsg`, then `rb.read(ptr, m.size)` (`core/RtMsgFifo.cpp:708`).
By finding 1 the pipe is what fills first, and when it does, the
payload is already in the ring with nothing that will ever consume it.
Nothing is printed on that path: the `fprintf` above belongs to the
ring's failure, and a failed `oob_write` only returns false.

`CircularBuffer`'s
`preserveBoundaries` defaults to `false` (`include/CircularBuffer.h:11`)
and `Private` constructs with the default, so the ring is a plain byte
stream: every later read takes `m.size` bytes from the wrong offset.
That is corruption of the whole output stream from that point on, not
one lost message, and nothing in the API resynchronizes it short of
constructing a new `Midi`. **Not measured** — it is what the two files
say when read together.

**4. Nothing above learns about any of it.** `commsSend` assigns a
`bool` to an `int` and tests it for negativity
(`core/AuxTaskNonRT.cpp:21`), so its `ret < 0` branch cannot be taken
and it returns 0 whether the write landed or not. `writeOutput` reads
that 0 as success, which makes its `usleep(10000)` retry
(`Midi.cpp:525`) dead code and its return value 1 for every call that
gets past the `outputEnabled` check — 0 being reserved for output that
was never opened (`Midi.cpp:513`). A `Result` built on it would have an
`Err` that cannot occur.

**5. Concurrent callers are outside the contract.** `CircularBuffer`
says so itself:

```c
// include/CircularBuffer.h:14
* Write to the circular buffer. Call this from a single thread.
```

`Settings::thread_count` makes several render threads the normal case
for this crate, and every one of them would reach the same
`CircularBuffer` through the same `Midi`. That is a correctness
problem, not a preference.

## Why output goes through a queue of our own

Four reasons, in the order they matter:

1. **It makes one thread the only caller.** A drain is what calls
   `writeOutput`, and there is one drain at a time — the task, or a
   `flush` on the thread that opened the port, never both. That is the
   answer to finding 5, and the only one of these reasons that is about
   correctness rather than about degree: `CircularBuffer::write` asks
   for a single thread, and several render threads calling
   `writeOutput` do not give it one.
2. **It decides how many records Bela's pipe ever holds.** MIDI on the
   wire is a byte stream, so a drain may concatenate everything it
   finds into a single `writeOutput` call: one record per drain
   replaces one record per message. Since finding 1 makes the pipe the
   thing that fills, and finding 3 makes filling it a silent
   misalignment of everything sent afterwards, this is the whole of the
   defence against that — nothing downstream reports it, so it has to
   be kept from happening.
   It does not become a constant: a drain runs when the task runs, so
   records in flight track *blocks that produced output while the
   non-real-time side was asleep*, not messages. The compression is by
   however many messages a block emits, which for a chord plus a
   controller sweep is ten to twenty — against a pipe of 256.
3. **It keeps the audio thread out of the failure paths.** Today those
   are quiet on this side: finding 3 prints nothing, and finding 2's
   `fprintf` needs the payload ring to fill, which for three-byte
   messages it does not. What the queue insures against is the two that
   are one upstream edit away — the `fprintf` for a program that sends
   long system exclusive dumps, and the `usleep(10000)` retry that
   becomes reachable the moment `commsSend` reports anything. Both then
   happen on the task's thread, which is allowed to miss deadlines.
4. **It gives `Err` a meaning that is true.** Not "the device did not
   receive this", which nothing in the chain can report, but "this was
   not accepted into the queue".

## What the queue does not buy

- **No backpressure, and none is observable.** `writeOutput` always
  reports success, so a drain always succeeds and the queue always
  empties. The queue therefore fills only when the program puts
  messages in faster than the drain takes them out, never because the
  device or the pipe is behind. Anything below our queue stays as
  silent as finding 4 leaves it.
- **`Err` is a budget, so the budget has to be a real number.** Since
  the only way to fill the queue is to outrun the drain, its capacity
  is a declared allowance — messages between drains — and `Err` means
  the program exceeded what it declared. Documented that way, it is
  honest and testable on the host; documented as "the device is busy",
  it would be finding 4 hidden one level higher.
- **One more hop.** `render` → queue → task → pipe → non-real-time
  thread → ALSA, against Bela's `render` → pipe → thread → ALSA. The
  added latency is one task wakeup, against the ~1 ms a three-byte
  message takes on a 31250 baud wire.
- **Residue at teardown.** Messages still in the queue when the audio
  system stops are never sent: the tasks are deleted, and a handle from
  a stopped audio system schedules nothing (`bela/src/task.rs`). A
  program that wants a final all-notes-off has to send it and keep
  rendering, or flush explicitly, rather than send it on the way out.

## The shape that follows

- **The producer end is per render thread**, handed out where the
  crate already hands out per-thread state, so single-producer is a
  property of the type rather than a rule to follow. With one render
  thread it is one queue; with four it is four, drained by one task.
- **The consumer end is one drain at a time**, which is what reason 1
  above rests on. Two things can drain — the task, and a `flush` from
  the thread that opened the port — so they take turns rather than
  overlap, and `writeOutput` sees one caller however many threads are
  rendering.
- **Ordering between render threads is not guaranteed.** Each thread's
  messages keep their order; two threads' messages interleave by drain
  order. Any program where that matters should emit MIDI from
  `render_pre` / `render_post`, which run on one thread.
- **A message is in the queue whole or not at all.** That is the
  invariant `Err` is defined against, and it is what makes system
  exclusive messages — which cannot be split around other messages —
  expressible at all.
- **Flushing is explicit and happens while rendering.** `Drop` cannot
  do it: the drain must run on a thread attached to EVL, and it also
  cannot block the way `C-DTOR-BLOCK` warns against. A `flush` called
  before the audio system stops is the alternative that guideline asks
  for.

## Input

Nothing here needs redesigning, so nothing is redesigned.

`readFrom()` starts an `RtThread` (`Midi.cpp:338`) that polls with a
50 ms timeout, appends to a 1000-byte ring and feeds a `MidiParser`
byte by byte. Parsed messages land in a 100-message ring
(`Midi.h:155`) that `getNextChannelMessage()` drains. Read from
`render`, that is a ring read: no allocation, no system call, no
blocking.

Two details the wrapper is shaped by:

- **The parser callback runs on the input thread**, not in `render`.
  `setParserCallback` (`Midi.h:314`) only hands it to the parser; what
  calls it is `readInputLoop`, which feeds each byte to
  `MidiParser::parse` (`Midi.cpp:235`), and `parse` invokes the
  callback as soon as a message completes (`Midi.cpp:96`). The crate
  exposes the polling interface only, so there is one answer to where
  user code runs and it is the audio thread.
- **The first byte of the raw stream is a phantom `0x00`.** `setup()`
  leaves `inputBytesReadPointer` at `size() - 1` with the write pointer
  at 0 (`Midi.cpp:114`), so the first `getInput()` returns a byte
  nobody sent. The parser discards it; a raw-byte reader would not,
  which is a second reason the crate exposes parsed messages rather
  than bytes.

## Ports, as named on the board

Measured 2026-08-07 on a Gem with nothing attached to it.

There is one port, and its name depends on who is asking:

| asked by | answer |
| --- | --- |
| `amidi -l` | `hw:0,0`, direction `IO` |
| `Midi::listAllPorts()` | `hw:0,0,0` |

Bela's names carry the subdevice, and `readFrom`/`writeTo` compare the
string they are given against exactly that list (`Midi.cpp:313`), so
the name `amidi` prints opens nothing and reports `-1`. That is worth a
listing call of its own in the shim rather than a line in a
documentation comment nobody reads before the first failure.

The port itself is the USB gadget (`f_midi`, from `g_multi`), which is
present whether or not a host is attached — `/proc/asound/cards` shows
`MIDI Gadget` with the board's USB device port unplugged. So both
directions can be exercised over a USB cable to a host before any MIDI
hardware is involved, and `snd-virmidi` is on the image for the case
where there is no cable either.

Measured with the shim, on the same board:

- opening `hw:0,0,0` for input and for output both report success;
- opening `hw:9,9` — a port that does not exist — reports `-1` for
  both, which is the case Bela's own `writeTo` answers with `1`;
- `bela_midi_write_output` returns 1 with the port open and 0 without
  one, and there is nothing to receive it with the cable out.

## Defects designed around

| Where | What | How the crate answers |
| --- | --- | --- |
| `Midi.cpp:350` | `writeTo()` returns `1` for a port that does not exist — the same value as success — leaving `outputEnabled` false | the shim pairs it with `isOutputEnabled()` and reports that |
| `Midi.cpp:359` | `writeTo()` marks `inPortFull.hasOutput`, the *input* port descriptor | not exposed |
| `Midi.cpp:559` | `writeMessage(const MidiChannelMessage&)` sizes a VLA `1 + getNumDataBytes()` but always writes `bytes[1]`: a one-byte stack overflow for a type with no data bytes | the shim calls `writeOutput(bytes, len)` and never this |
| `Midi.cpp:151` | `enableParser(false)` deletes `inputParser` without clearing it; twice is a double free, and `cleanup()` never deletes it at all | the parser is on for the object's whole life, established once in the shim |
| `Midi_c.cpp:19` | `Midi_new` enables the parser after starting the input thread | the shim enables it first |
| `Midi.cpp:52` | `MidiParser::parse()` counts each status byte twice in its return value | no caller uses it |

## Not the third way

The issue that opened this question named a third option: a shim that
holds its own `RtNonRtMsgFifo` and returns `writeRt`'s `bool`
unchanged. It is the only arrangement that gets an honest `Err` with
no second queue and no extra hop.

It is also the one that gives up the reason the class was chosen.
`Midi::midiOutputTask` and `AuxTaskNonRT::fifo` are both private, so
reaching the FIFO means writing the output thread and the
`snd_rawmidi_write` side again — and with them `attemptRecoveryWrite()`,
one of the four things listed at the top of this file as worth
inheriting. Two of the four, on the output side, to avoid one queue.

## Open, and needing a board

- **Whether the overflow lags or corrupts** (finding 3). Reachable by
  putting more than 256 messages between one drain and the next and
  watching the receiving end start reading from the wrong offset. There
  is no signal to wait for — `stderr` stays quiet, since the one line
  that path could print belongs to finding 2 — and **that silence is
  itself part of what the run has to confirm**.
- **Whether Bela's `cleanup` callback runs on a thread attached to
  EVL.** `evl_get_self()` answers it in one call, and the answer says
  whether a flush can run there at all or whether it has to happen
  while rendering.
- **Whether more than one port ever needs opening at once.** One `Midi`
  is one port pair, and a program wanting several devices holds several
  of them, each with its own input thread. Nothing here says where that
  stops being reasonable.
