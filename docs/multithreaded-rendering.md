# Multithreaded rendering

What `threadCount` actually does on a Bela Gem, measured on the board
on 2026-08-05 (Bela 1.18.0, Debian Bookworm image 2026-03-25, EVL
real-time core), and what it means for the safe Rust API.

## Summary

`render` is called **concurrently on every thread for the same block**,
with **the same user data pointer** and **the same buffers**. Bela does
not partition anything: splitting the work is entirely the
application's job, using `thisThread` and `threadCount` from the
context.

That is incompatible with `BelaApplication::render(&mut self, ...)`,
which is why `Bela::new` currently rejects a `thread_count` above 1
(see [Consequences for the Rust API](#consequences-for-the-rust-api)).

## How Bela implements it

From the sources on the board (`/root/Bela/core`):

- `Bela_initAudio` creates `threadCount - 1` extra real-time threads
  (`RTAudio.cpp`, `mpAudioLoop`) and one mirrored context per thread.
- The mirror is `BelaContextSplitter::contextMirror`, a plain `memcpy`
  of the context struct, so every copy holds **the same buffer
  pointers**. Only `thisThread` is then overwritten.
- Every block, `render_wrapper` signals the secondary threads on a
  condition variable, calls `render` itself for thread 0, and then
  spins until all of them report completion. Each secondary thread
  calls `(*gUserRender)(&gMPBelaContexts[n], gUserData)` — the same
  `gUserData` every thread gets.
- `setup` and `cleanup` are called once, on the main context
  (`thisThread == 0`). `Bela_stopAudio` joins the extra threads before
  `Bela_cleanupAudio` runs `cleanup`, so neither callback overlaps
  `render`.
- `render_pre` / `render_post` in `BelaInitSettings` are the hooks that
  run once per block around the parallel section. They are not wrapped
  by this crate yet.

## What was measured

A probe built on `bela-sys` directly (not on the safe trait, since the
safe trait is what was under test) recorded, from every `render` call:
the thread index, the buffer pointers, `audioFramesElapsed`, the Linux
thread id, and a counter of how many threads were inside `render` at
the same time. Thread 0 stamped a marker into the first output sample
and the other threads checked whether they could read it back.

Run for 3 s at 44.1 kHz with a block size of 16 (2757 blocks/s):

| `threadCount` | max threads inside `render` at once | calls per thread | `audioFramesElapsed` | `audioIn` / `audioOut` / `digital` | marker seen by other threads |
|---|---|---|---|---|---|
| 1 | 1 | 8274 | 132368 | — | — |
| 2 | 2 | 8271 each | 132320, identical | identical across threads | every block |
| 4 | 4 | 8273 each | 132352, identical | identical across threads | every block |

Reading the table:

- **Concurrency is real.** With `threadCount = 4`, four distinct
  thread ids were inside `render` simultaneously.
- **Every thread renders the same block.** Call counts and
  `audioFramesElapsed` match across threads.
- **Buffers are shared, not partitioned.** The pointers are identical,
  and a value written by thread 0 was read back by the others in every
  single block.
- **No metadata is partitioned either**: `audioFrames` and the channel
  counts are the same on every thread.

So naive user code that ignores `thisThread` does the same work N times
over the same buffers, racing on every output sample.

To reproduce: build a probe against `bela-sys` (a `render` callback
that only touches atomics is enough), set `settings.threadCount`, and
record the values above. Stop `bela_daemon` first
(`systemctl stop bela_daemon`).

## Consequences for the Rust API

`BelaApplication::render` takes `&mut self`, and the trampoline turns
the user-data pointer into `&mut T`. With more than one render thread
the C side hands that same pointer to every thread at the same time, so
several `&mut T` to one value would exist at once: undefined behaviour,
reachable from entirely safe user code.

Until a trait shaped for concurrent rendering exists, `Bela::new`
returns [`Error::ThreadCountUnsupported`] when the effective
`threadCount` is above 1, rather than initialising something unsound.
`Settings::thread_count` is kept, since it is the same knob the future
API will use.

The shape that fits the C behaviour is a separate trait whose `render`
takes `&self` and whose implementor is `Sync`, leaving `setup` and
`cleanup` on `&mut self` (they are single-threaded, as measured above).
Applications would partition by `context.this_thread()` and use
interior mutability — atomics, or per-thread state indexed by the
thread number — for anything they mutate. That trait is not designed
yet; it needs its own issue before the surface is treated as stable.
