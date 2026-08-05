# Multithreaded rendering

What `threadCount` actually does on a Bela Gem, measured on the board
on 2026-08-05 and again on 2026-08-06 through the safe API (Bela
1.18.0, Debian Bookworm image 2026-03-25, EVL real-time core), and what
it means for that API.

## Summary

`render` is called **concurrently on every thread for the same block**,
with **the same user data pointer** and **the same buffers**. Bela does
not partition anything: splitting the work is entirely the
application's job, using `thisThread` and `threadCount` from the
context.

`BelaApplication` is shaped around that — `render` takes `&self` and one
per-thread `RenderState`, and `RenderContext` hands out only this
thread's frames — and the crate does the partitioning Bela does not.
See [Consequences for the Rust API](#consequences-for-the-rust-api).

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
  run once per block around the parallel section, on the main audio
  thread. `render_wrapper` calls them whenever they are set, whatever
  the thread count, so they are not a multithreading-only path: with
  one render thread the sequence is still pre, render, post.

There is one thin edge in that arrangement, and the crate is built to
survive it rather than to assume it away. The loop `render_wrapper`
waits in is

```c
while(!allThreadsDone && !Bela_stopRequested()) { ... }
```

and each secondary thread checks the same flag just *before* calling
`render`. So a stop landing mid-block ends the main thread's wait early
and can leave `render_post` overlapping a `render` that has not
finished. That is the one measured way the callback protocol can be
broken, and it is why the crate checks the protocol with atomic claims
instead of trusting it: a `render_post` that arrives then is refused
rather than served with references a running `render` already holds.

It is also the ordinary way a run ends, which is why the crate counts a
refusal made *while a stop is already pending* separately from one made
during a live run. Only the second fails
[`Bela::until_stopped`](../bela/src/system.rs) with
`Error::CallbackFaults`; the first is reported on the console and means
the last block may be short. Mixing them would make Ctrl-C an error.

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

A `render` that took `&mut self` could not describe this: the C side
hands one user-data pointer to every thread at the same time, so
several `&mut T` to one value would exist at once — undefined
behaviour, reachable from entirely safe user code.

So `BelaApplication` is built the other way round, and one render
thread is the degenerate case of the same model rather than a separate
one:

- `render(&self, state: &mut Self::RenderState, context: &mut RenderContext)`.
  The application is shared; everything `render` mutates lives in a
  `RenderState`, one per thread, built by `create_render_state` before
  audio starts.
- `RenderContext` reads the whole block but writes only
  `audio_frame_range()` and its analog and digital counterparts —
  contiguous frame ranges that tile the block exactly. The crate does
  the partitioning Bela does not; `partition` in `bela/src/context.rs`
  is the whole of it.
- `render_pre` and `render_post` take `&mut self`, every state, and a
  `BlockContext` over the whole block. They are where per-block
  preparation and mixing down belong, and the only place a
  multithreaded application may touch frames it does not own.
- `setup` and `cleanup` keep `&mut self`, as measured above.

The digital words are the exception to "inputs are shared": they are
the outputs too, so `RenderContext`'s digital accessors — reads
included — are bounded by this thread's range, while `audio_in` and
`analog_in` are not.

None of that is trusted to libbela. `bela/src/runtime.rs` grants a
non-blocking atomic claim before building any reference — one exclusive
claim for the single-threaded phases, one slot per render thread — and
a callback that cannot claim what it needs records a fault, requests a
stop and returns without running user code. `Bela::callback_faults`
reports how many times that happened; the count is 0 for a run that
behaved.

## Running it

`bela/examples/parallel.rs` renders a bank of 192 sine oscillators
split by frame, and reports what each thread did. Measured on the board
on 2026-08-06 (same image and libbela as above), 16 frames per block at
44.1 kHz, 6-second runs, with every thread on a core of its own:

| `thread_count` | cores used | frames per thread per block | busiest thread's share of the block | audio thread busy |
|---|---|---|---|---|
| 1 | 3 | 16 | 41.4% | 48.5% |
| 2 | 3, 2 | 8 | 20.9% | 42.1% |
| 4 | 3, 2, 1, 0 | 4 | 10.6% | 32.2% |

Every run reported `rendered + uncovered = expected` and `faults=0`:
every frame was written once or not at all — a frame written twice
would push `rendered` past `expected` — and no callback had to be
refused. So the block was divided, not rendered four times over.

The work per thread falls with the thread count almost exactly — 41.4%
to 10.6% over four threads — while the audio thread's own figure falls
much less, from 48.5% to 32.2%. The difference is the cost of the
arrangement itself: the main thread wakes the others, renders its own
share, and then spins waiting for the last of them, and all of that
counts as busy. Multithreaded rendering buys headroom, not four times
the headroom.

### The last block is sometimes short

The `while(!allThreadsDone && !Bela_stopRequested())` edge described
above is visible from the safe API, and takes two shapes depending on
where the stop lands relative to the secondary thread's own check of
the same flag. Running the example for 5 s with two threads, one run in
several ends in one of them.

**The thread bows out before rendering.** `render_post` runs and finds
the frames it owned still carrying the sentinel:

```
parallel: blocks=11729 frames=16 rendered=187688 expected=187696
parallel: uncovered=8 abandoned=1 unfinished=0
```

`uncovered=8` is exactly one thread's share of exactly one block, and
`rendered + uncovered` still adds up. Nothing overlapped, so nothing
was refused.

**The thread is inside `render` when the stop lands.** The main thread
gives up waiting and calls `render_post` over the top of it, the crate
refuses that callback, and the block is never accounted for:
`unfinished=1`, with `rendered + uncovered` short by up to one block.
`Bela::until_stopped` prints a line about the refusal and still returns
`Ok(())`, because this is what a clean shutdown looks like from inside.

Either way the shortfall is at most one block and never negative — a
frame written twice would push `rendered + uncovered` *past* `expected`
— which is what the smoke test checks, along with
`abandoned + unfinished <= 1`. It is worth knowing for an application
that counts frames rather than blocks: the last block of a
multithreaded run may be partly silent, and its `render_post` may not
run at all.

`scripts/smoke-test.sh` runs the same example at 1, 2 and 4 threads and
checks the coverage identity, the cut-short-block bound, the live fault
count and the fall in per-thread work.
