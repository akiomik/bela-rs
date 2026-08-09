# Bela Gem board facts

Measured on the actual board (Bela Gem Stereo on PocketBeagle 2),
collected 2026-08-05 over the USB gadget network. Only values confirmed
on the device are recorded here.

## System

- `uname -a`:
  `Linux bela 6.12.49-ti-arm64-r55-evl-2 #2 SMP EVL Thu Feb 19 16:28:06 UTC 2026 aarch64 GNU/Linux`
- **Real-time core: EVL (Xenomai 4 lineage), not Xenomai 3
  Cobalt/Dovetail.** `evl --version` reports
  `evl.0.55 -- #98a8b88 (2025-10-08) [requires ABI 42]`. Userspace
  library is `libevl.so.6` in `/usr/evl/lib/aarch64-linux-gnu`, headers
  in `/usr/evl/include` (confirmed via the kernel name, `evl
  --version`, and `ldd libbela.so`).
- OS: Debian 12 bookworm — "Bela Debian Bookworm Image 2026-03-25".
- Bela software in `/root/Bela`: git HEAD is `fb362a54` (`master`,
  2025-03-29) — **but the working tree is overlaid with newer,
  unpublished sources**. `include/Bela.h` reports **version 1.18.0**
  (upstream `master` at that commit is 1.13, the public `dev` branch
  1.14). `git status` shows the include tree as staged deletions; the
  on-disk files are what the shipped `libbela` was built from and are
  the ground truth for the ABI.
- Header changelog highlights beyond our current vendored copy (1.14):
  - 1.15.0: `threadCount` in `BelaInitSettings`; `const uint32_t
    thisThread` / `threadCount` in `BelaContext` (multithreaded
    rendering — this is how the "per-core render" feature surfaces; no
    separate callback signature). **`BelaContext` layout change: the
    committed bindings must be regenerated.**
  - 1.16.0: `BelaGem` hardware support.
  - 1.17.0: `sampleRate` in `BelaInitSettings`, `Bela_initRtBackend()`,
    `Bela_gettime()`, `Bela_nanosleep()`, `Bela_printFlushBuffers()`;
    `INPUT`/`OUTPUT` became `enum BelaDigitalDirection`.
  - 1.18.0: `Bela_clock_gettime()`.
- **Header pin implication**: vendor the headers from the board
  (`/root/Bela/include`), not from an upstream git ref. Of the include
  closure, `GPIOcontrol.h` and `Utilities.h` are byte-identical to the
  currently vendored copies; only `Bela.h` differs.

## Board identity and version

Collected 2026-08-06 with `bela/examples/board_info`, which brings no
audio system up: everything here is what libbela answers before
`Bela_initAudio` is called at all.

- **The library reports version 1.18.0** (`Bela_getVersion`), which is
  what `include/Bela.h` says and what the vendored headers carry as
  `BELA_MAJOR_VERSION` and its siblings. So on this image the running
  library and the committed bindings describe the same API — the
  comparison is only worth printing because it is not guaranteed: the
  version comes from the library at run time and the macros from the
  headers at build time.
- **The board detects as `GemStereo` through four of the five detect
  modes**, and `/run/bela/belaconfig` holds `HARDWARE=GemStereo`. The
  table is in the order they were asked, which is not the order the C
  enum declares them: `Scan` writes the file the three cache and user
  modes read, so it goes last and the four above it report what was
  already on the board rather than what this run had just written.

  | mode | answer |
  |---|---|
  | `Cache` | `GemStereo` |
  | `CacheOnly` | `GemStereo` |
  | `User` | `GemStereo` |
  | `UserOnly` | `NoHw` |
  | `Scan` | `GemStereo` |

- **`UserOnly` answers `NoHw` on a board that plainly has hardware**,
  because there is no `~/.bela/belaconfig` — `/root/.bela` does not
  exist on this image — and that mode reads only the user's file with
  no fallback. The measurement is what "the detect modes are not
  interchangeable" means in practice: a program that picked `UserOnly`
  as its default would decide it was running on nothing.
- **`Scan` is the mode with a side effect, and it agreed with the cache
  the daemon had written.** It goes out over the buses and writes
  `/run/bela/belaconfig`; the other four read a file. Asked last, after
  the cache modes had reported what the daemon left at boot, it
  answered `GemStereo` and left the file holding the same
  `HARDWARE=GemStereo` it started with — so the daemon's cache and a
  fresh scan of the buses say the same thing on this board.
  `scripts/smoke-test.sh` keeps that honest from the other end: it reads
  the file before it runs anything and again after everything, and puts
  it back if the run changed it.
- **Bringing an audio system up writes the cache too, and a scan is not
  the only thing that does** (measured 2026-08-07). With
  `/run/bela/belaconfig` removed and `bela_daemon` stopped,
  `bela/examples/sine` — which detects nothing itself — was run for
  three seconds, and the file was back afterwards holding
  `HARDWARE=GemStereo`. So on a board that has never been scanned, any
  program that brings an audio system up leaves a cache behind, which
  is why the smoke test takes its reading before the first binary runs
  rather than around the one probe that detects on purpose. **Which
  call writes it is not measured**: a run that comes up goes through
  `Bela_defaultSettings`, `Bela_initAudio`, `Bela_startAudio` and
  `Bela_cleanupAudio`, the headers do not say which of them detects,
  and separating them would need a probe that calls one and stops.
  Nothing here depends on the answer — reading before the first binary
  covers all four.
- **Nothing here is measured on a Gem Multi.** There is no Multi to
  measure on, so `Board::GemMulti` is a name taken from the headers and
  nothing in this file claims what one reports.

## Build and link information

Captured from a verbose on-board build:
`make -C /root/Bela PROJECT=<name> AT=` (compiler is `clang++`).

- Include paths: the project dir, `/root/Bela/include/legacy`,
  `/root/Bela/include`, `/root/Bela/build/pru`, `/root/Bela`,
  `/usr/evl/include`.
- Higher-level classes are sources rather than installed headers: what
  a program includes as `<libraries/<Name>/<Name>.h>` resolves through
  the `/root/Bela` entry above to `/root/Bela/libraries/<Name>/`, which
  holds the `.h`, the `.cpp` and a `lib.metadata` naming the link flags
  that library asks for of its own (`Midi` asks for `-lasound`).
  `/root/Bela/include/legacy/` holds three-line shims for the flat
  include paths these replaced in 2019: each one `#warning`s and then
  includes the `libraries/` copy, except `OSCClient.h` and
  `OSCServer.h`, which `#error` because that API is gone.
- The real-time plumbing those classes are built on is sources too, in
  `/root/Bela/core/`: `AuxTaskNonRT.cpp`, `AuxTaskRT.cpp`,
  `RtMsgFifo.cpp`, `CircularBuffer.cpp`, `RtThread.cpp` and the rest of
  what `libbela` is made of. The headers in `include/` declare them and
  stop there, so what a call costs the audio thread is only readable
  here — and it is not always what the declaration suggests, because
  `RtMsgFifo.cpp` implements `RtNonRtMsgFifo` three ways behind
  `__COBALT__`, `BELA_EVL` and plain Linux (`RtMsgFifo` itself has two,
  `BELA_EVL` and everything else). `BELA_EVL` is the one this board
  builds (see the defines below), and on its real-time side a message's
  payload travels through a `CircularBuffer` while the pipe carries
  only a header.
- Defines (also needed as clang args when running bindgen):
  `BELA_USE_POLL`, `ENABLE_PRU_UIO=0`, `ENABLE_PRU_RPROC=1`,
  `IS_AM62_PB2`, `IS_AM62`, `BELA_HAS_GPIO`, `BELA_HAS_PRU_AND_MCASP`,
  `BELA_RT_WRAP(call)=call`, `BELA_EVL`, `NDEBUG`, plus
  `firmwareBelaRProc*` paths for the PRU firmware.
- Codegen flags: `-mcpu=cortex-a53 -mtune=cortex-a53 -O3 -ffast-math
  -ftree-vectorize -std=c++14 -no-pie -pthread`.
- Link line (in-tree examples link the core object files directly):
  `-Wl,--as-needed -no-pie -pthread -Llib/ -lseasocks
  -L/usr/evl/lib/aarch64-linux-gnu -levl -lpthread -lrt`.
- For external programs, `/root/Bela/lib/` provides `libbela.so` /
  `libbela.a` (and `libbelaextra.*`). `ldd libbela.so` resolves
  `libevl.so.6` (`/usr/evl/lib/aarch64-linux-gnu`), `libseasocks.so`
  (`/usr/local/lib`), `libstdc++`, `libbpf`, `libelf`, `libz`, `libm`,
  `libc` — so the expected external link is `-L/root/Bela/lib -lbela`
  with a sysroot that carries those transitive dependencies.
- **`libbelaextra.so` needs `libbela.so` and does not say so.**
  `readelf -d` lists `libstdc++`, `libseasocks`, `libasound.so.2`,
  `libNE10.so.10`, `libm`, `libgcc_s` and `libc` — not `libbela` —
  while its undefined symbols include `RtThread::create`,
  `SchedulableTask::create`, `AuxTaskNonRT::~AuxTaskNonRT`,
  `IoUtils::glob` and `StringUtils::trim`, all defined in `libbela.so`.
  So `-lbelaextra` has to come *before* `-lbela` on a link line using
  `--as-needed`, which rustc does: named the other way round, `libbela`
  is dropped as unused before the library that needs it is read.
- **The board's C++ layout and a cross toolchain's agree** (measured
  2026-08-07). `libbelaextra.so` was built by the image's `clang++`
  with `-std=c++14`, and code that allocates one of its classes has to
  match it. Compiling the same file both ways —
  `aarch64-unknown-linux-gnu-g++` 15.2.0 against the sysroot, and the
  board's `clang++` on the board — gives `sizeof(Midi)` 456 and
  `alignof` 8 on both, with `MidiParser` 72/8, `MidiChannelMessage`
  24/8 and `RtThread` 88. Nothing in those classes is conditional on
  the build defines, and libstdc++'s `std::string`, `std::vector` and
  `std::function` have not changed layout since GCC 5, which is what
  makes the agreement expected rather than lucky.

## Audio thread

- **Large period sizes move `render` to a second thread.** libbela
  renders in short "native" blocks and, when the requested period size
  exceeds what the PRU memory allows, splits the work in two: the core
  audio thread (`audioLoop` → `PRU::loop`) keeps the short blocks, and
  the user's `render` runs on `bela-audio-fifo` (`fifoLoop`) behind a
  context FIFO. The block size the application sees is the requested
  one either way, and nothing in `BelaContext` distinguishes the two
  arrangements.

  Measured with `Settings::verbose(true)`, which makes libbela print
  `gFifoFactor`:

  | `period_size` | `gFifoFactor` | core `audioFrames` | user `audioFrames` |
  |---------------|---------------|--------------------|--------------------|
  | 16            | 1             | 16                 | 16                 |
  | 32            | 1             | 32                 | 32                 |
  | 64            | 1             | 64                 | 64                 |
  | 128           | 1             | 128                | 128                |
  | 256           | 2             | 128                | 256                |

  So the split starts above 128 frames on this board, i.e. the divisor
  is 128 — the same one upstream `fb362a5` uses for `BelaHw_Bela`,
  whose `switch` has no Gem case at all (Gem support is in the image
  overlay).

  This matters for the CPU monitoring counters, which `PRU::loop`
  updates on the core audio thread: above 128 frames a read from
  `render` would be a cross-thread read of an unsynchronised structure.
  `Settings::cpu_monitoring` is refused there
  (`MAX_MONITORED_PERIOD_SIZE`), and `scripts/smoke-test.sh` re-measures
  the two boundary values on every run so the constant cannot drift away
  from the hardware.

- **A failed initialisation poisons its process, and only its
  process.** Returning `false` from `setup` fails `Bela_initAudio` with
  1 and leaves libbela's globals in that process still believing the
  audio system is up. What follows is not arrangement-dependent.
  Measured by `scripts/probe-init-failure.sh`, which runs each probe in
  a process of its own between two full audio cycles in processes of
  their own; each crashing case was repeated three times and was
  identical every time.

  | after a `setup`-aborted `Bela_initAudio` | what libbela does |
  |---|---|
  | nothing more | the process exits 0 |
  | another `Bela_initAudio` | `evl_attach_thread for WSServer_40100 failed with File exists (17)`, `Error in evl_create_xbuf: 17 EEXIST`, `Unable to create pipe p_WSClient_signalMonitor40100_bin`, `Mcasp::start() called while already running`, then SIGSEGV |
  | `Bela_cleanupAudio()` | SIGSEGV inside the call, both ways it was tried |
  | `Bela_cleanupAudio()` then another `Bela_initAudio` | never reached: the cleanup call is the crash |

  That is libbela, and it has not changed. What has is that `Bela::new`
  no longer takes you there: since the fix for #30 it releases the
  process-wide claim as unusable and every later call returns
  `Error::AudioSystemPoisoned`. Re-measured with the refusal in place,
  `abort-then-new` reports `second=failed-AudioSystemPoisoned` and
  exits 0 where it used to segfault, while `raw-cleanup`, which goes
  round the crate to the C API, still segfaults exactly as above. The
  rows are kept because they are what the refusal is standing in front
  of, and what a future board image would have to be re-measured
  against.

  **`Bela_cleanupAudio` was tried two ways**, because the arrangement
  could have been the crash rather than the call. `Bela.h` says it
  fires the user's `cleanup()` callback, and `Bela::new` frees the
  application before returning `Error::Init` — so a probe outside the
  crate (`abort-cleanup`) necessarily calls it with the application
  already gone, which is not what a fix inside `Bela::new` would do.
  Going through the C API directly, mirroring `Bela::new` up to the
  failing `Bela_initAudio` and calling `Bela_cleanupAudio` with the
  application still alive (`raw-cleanup`), the callback **did** run and
  read its application intact — and the process segfaulted immediately
  afterwards regardless. So the call is the crash, not the arrangement.

  Two things followed for #30. The damage is confined to the process
  that took it: after every case above, including the ones that
  segfaulted, the next process brought up an audio system and rendered
  the expected ~2760 blocks per second, with no reboot, no restart of
  `bela_daemon` and nothing left to kill in between. And
  `Bela_cleanupAudio` is not the way out, which left refusing later
  attempts as the only repair `Bela::new` could make — which is what it
  now does.

  Two things also follow for anyone tempted to call it anyway. The
  callback running means calling `Bela_cleanupAudio` *after* freeing
  the application would be a use-after-free for any application that is
  not zero-sized — `abort-cleanup` only survives to its own crash
  because its application is a ZST with the default do-nothing
  `cleanup`. And `Bela.h` says the call belongs after `Bela_stopAudio`,
  which a failed initialisation never reaches, so nothing here is being
  used as its documentation intends.

- **Several audio systems in one process are fine, as long as each one
  succeeds.** Twelve full cycles (`Bela::new`, `start`, render, drop)
  in one process — `CYCLE_COUNT=12 scripts/probe-init-failure.sh
  <host> cycles` — and eight build-and-drop cycles with audio never
  started — `CYCLE_COUNT=8 … init-cycles` — both ran clean. An earlier
  note here put a bus error at four or five of the latter; it did not
  reproduce in either shape on image 2026-03-25. What stops a second
  audio system is a *failed* first one, not how many there have been.

- **The board does not refuse a second process.** While one process was
  rendering, another brought up its own audio system and its `render`
  ran at the same rate alongside it — 2759 blocks in its 1 s window,
  against the holder's 16541 in 6 s — and both exited 0. Whether the
  sound is right was not judged, only that both callbacks ran. So
  "the audio hardware is unavailable or already in use" is not a
  failure this board produces by that route, and no `Error::Init` other
  than the `setup` abort has been measured.

## Codec levels and gain

Measured with a C probe against the shipped `libbela` (the level
functions, `Bela_defaultSettings`), on a Bela Gem Stereo.

- **Defaults** (`Bela_defaultSettings`): `lineOutGains` is one entry,
  all channels at 0 dB; `headphoneGains` all channels at -6 dB;
  `audioInputGains` all channels at 16 dB; `adcGains` is empty. The
  deprecated scalars (`dacLevel`, `adcLevel`, `headphoneLevel`,
  `pgaGain`) are all `BELA_INVALID_GAIN` (999999), which is how libbela
  spells "not set" — it only applies them when they are not that.
  `beginMuted` is 0.
- **The gain arrays are the same calls, made earlier.**
  `Bela_initAudio` walks each array and calls
  `Bela_setAudioInputGain` / `Bela_setHpLevel` / `Bela_setLineOutLevel`
  per entry, after `initCodec()` and before the audio thread exists.
  The codec stores what it is told and only writes its registers once
  it is running, so a call between `Bela::new` and `Bela::start`
  reaches the hardware in the same state and at the same moment. This
  is why `bela` wraps the level API on the `Bela` handle rather than in
  `Settings`.
- **Only channels 0 and 1 exist.** `Bela_setLineOutLevel` and
  `Bela_setHpLevel` return 1 for any channel above 1 (checked at 2, 9,
  10 and 20), before and while audio runs. `Bela_setAudioInputGain`
  returns 0 for the same channels and does nothing with them: the
  TLV320AIC3106 path writes no register unless the channel is 0, 1 or
  negative. A negative channel means "all" throughout.
- **Levels are clamped, not validated.** `-1, +18 dB` and
  `-1, -200 dB` on the line out and `-1, 999 dB` on the input gain all
  return 0. The codec clamps: line out and headphone boost stops at
  +9 dB and attenuation runs in 0.5 dB steps to -63.5 dB, and an input
  gain at or below -96 dB is taken as a mute. libbela's own comment in
  `I2c_Codec::writeRoutingVolumeControlReg` notes that its conversion
  from decibels to register values is only approximate below -18 dB, so
  an attenuation set there is not quite the one that arrives.
- **There is no speaker amplifier mute pin.** `kAmplifierMutePin` is
  the default-constructed (invalid) `Gpio::Pin` for `IS_AM62_PB2`, so
  `Bela_defaultSettings` reports `ampMutePin = -1`, `Bela_initAudio`
  never opens the pin, and `Bela_muteSpeakers` returns 0 without doing
  anything — before, during and after a run. `beginMuted` therefore has
  no effect either.
- **Before and after an audio system.** With no audio system,
  `gAudioCodec` is null and all three level functions return -1
  (`Bela_muteSpeakers` still returns 0). After `Bela_cleanupAudio` the
  codec object outlives the audio system: the line out and headphone
  calls still report success, and the input gain reports failure
  because its I²C write no longer goes through. The safe API keeps both
  windows out of reach by hanging the calls off the `Bela` handle.

## Analog and digital I/O

Collected 2026-08-06 with `scripts/probe-io.sh`, which runs
`bela/examples/io_config` one configuration per process and reports the
`BelaContext` each one produces. Nothing was wired to the board for
these: they are the numbers libbela derives from the hardware and the
settings, which is what the accessors on the context types index
against. Whether a pin then does what the accessor says is a separate
measurement and is not recorded here yet.

- **The board identifies as `GemStereo`** (see "Board identity and
  version" above), **and libbela has no `BelaHwConfig` for it**:
  `Bela_detectHw(Cache)` returns 2 (`BelaHw_GemStereo`) and
  `Bela_HwConfig_new` on that value returns null. So the channel counts
  below do not come from the table `Bela_HwConfig_new` reads; on this
  hardware only `Bela_initAudio` knows them.
- **What `Bela_defaultSettings` asks for is not what the block gets.**
  The defaults are 8 analog in, **8 analog out**, 16 digital, 16 frames
  per period, 44100 Hz, one render thread, `useAnalog` and `useDigital`
  both on, `uniformSampleRate` on. The context that comes back from
  those same defaults has **0 analog outputs**:

  | | audio | analog | digital |
  |---|---|---|---|
  | channels | 2 in / 2 out | 8 in / **0 out** | 16 |
  | frames | 16 | 16 | 16 |
  | rate | 44100 | 44100 | 44100 |

- **A Gem Stereo has no analog outputs, and they are not folded into
  the audio outputs either.** `analogOutChannels` is 0 whether the
  settings ask for 8 (the default), 4 or 2, and `audioOutChannels`
  stays 2 throughout — it is never 2 plus the analog outputs. Bela's
  own specifications agree, listing expanded and DC-coupled outputs as
  the Gem Multi's; the migration guide's "+2 to the channel number" is
  about a board that has them. **Nothing here confirms the +2 offset,
  and nothing here refutes it for a Multi** — this board simply never
  presents the case.
- **Asking for a different number of analog inputs and outputs fails
  the initialisation.** `--analog-out 2` against the default 8 inputs
  makes libbela print `TODO: a different number of channels for inputs
  and outputs is not yet supported` and `Bela_initAudio` return 1, so
  `Bela::new` fails with `Error::Init(1)` and the process is poisoned
  for good (see "Audio thread" above). Setting both to the same number
  is accepted — and still yields 0 outputs.
- **`uniformSampleRate` is on by default, and what it removes is a
  frame ratio that follows the analog channel count.** With it on, the
  analog frame count and sample rate equal the audio ones whatever the
  channel count. With it off, the ratio Bela's migration guide
  describes is exactly what the board produces:

  | analog in channels | `uniformSampleRate` | analog frames | analog rate |
  |---|---|---|---|
  | 8 | on (default) | 16 | 44100 |
  | 8 | off | 8 | 22050 |
  | 4 | on | 16 | 44100 |
  | 4 | off | 16 | 44100 |
  | 2 | on | 16 | 44100 |
  | 2 | off | 32 | 88200 |

  Audio stayed at 16 frames and 44100 Hz in every row, and the digital
  frames and rate followed audio rather than analog throughout.
- **Disabling a domain zeroes its channels and frames.** `useAnalog`
  off gives 0 analog frames, 0 in and 0 out; `useDigital` off gives 0
  digital frames and 0 channels. The one asymmetry is the sample rate
  the context still reports: the digital rate falls to 0 with digital
  off, while the analog rate stays at 44100 with analog off.
- **`setup` and the first block agree**, in every configuration above,
  on every audio, analog and digital field. A frame count filled in
  only once the audio thread exists would have shown up as a
  difference; none did.
- **The period size does not change any of this.** At 16 and at 256
  frames — either side of the 128-frame boundary where libbela moves
  `render` behind a context FIFO — the analog and digital frame counts
  equal the audio one.
- **A one-thread run leaves `threadCount` at 0, not 1.** With no
  `threadCount` asked for, `Bela_defaultSettings` reports 1 but the
  `BelaContext` that comes back carries `threadCount = 0` and
  `thisThread = 0`, in `setup` and in the block alike, at every period
  size and channel count tried. So a context spells "one render
  thread" as a zero, which is why `BlockContext::thread_count` reads 0
  as 1 and why the crate's number cannot be used to tell the two
  apart. `thisThread` was only read from `setup` and `render_pre`,
  both of which run on the main audio thread; what a secondary render
  thread reports is measured by `examples/parallel` instead.

## What an analog input reads

Collected 2026-08-09 with `bela/examples/io_analog`, which reports the
mean, the range and the widest within-block spread of every analog input
once a second. This is the half of the section above that needed a
voltage on a pin: those numbers were the shape of the block, these are
what the accessor returns from it.

The wiring was a potentiometer between the 3.3 V rail on P2 and GND with
its wiper on `A0`, a direct tie from the same rail to `A1`, and `A2`
through `A7` left floating. A rail is a voltage known without an
instrument, which is what makes the full-scale answer below a
measurement rather than an estimate.

- **`analog_read` returns 1.0 for 4.096 V, as `Bela.h` says it does.**
  3.3 V on `A1` reads **0.8064**, against the 0.80566 that 3.3/4.096
  predicts — 0.1% out. The same rail through the potentiometer at its
  top end read 0.8065 on `A0` in a separate run. A 3.3 V full scale
  would have given 1.0, which is 24% away and outside anything a rail
  tolerance covers. So the internal reference is in use and the
  header's claim holds on a Gem Stereo.
- **The channel index is the number on the silkscreen.** Between two
  runs the only changes were the voltage on `A0` and the tie on `A1`,
  and the only channels whose readings moved were `ch0` and `ch1`;
  `ch2`–`ch7` held their floating values to the fourth decimal. Turning
  the potentiometer moves `ch0` alone.
- **The full range is covered and monotonic.** Swept end to end, `ch0`
  ran 0.0000 to 0.8074 continuously, reaching the same top as the
  directly tied `A1`. `ch1` stayed within 0.8064–0.8065 across all
  twenty report windows while `ch0` swept, so a moving channel does not
  disturb a held one.
- **The bottom clips to exactly 0.** At the potentiometer's earthed end
  `ch0` reads 0.0000 with a min, max and spread of 0.0000 — no noise at
  all, where the same channel at the top of its range wobbles by about
  0.0018. The reading does not go negative and does not dither around
  zero.
- **The frames within a block are in time order.** The widest
  within-block spread sits at 0.0015 with the input still and rises to
  0.0050–0.0059 only in the windows where the potentiometer was being
  turned quickly. A block whose frames were not consecutive in time
  would not track the speed of the hand turning the knob.
- **The interleaved index survives a different frame count.** With
  `--uniform-sample-rate 0` the analog block becomes 8 frames at
  22050 Hz against 16 audio frames (as the section above records), and
  every channel reports the same value and the same spread as at 16
  frames. `frame * channels + channel` is the layout on the device.
- **A floating input is not a zero, and it is an aerial.** Unconnected
  channels sat at steady but unequal values — 0.147, 0.015, 0.119,
  0.031, 0.019 and 0.043 on `A2` through `A7` — and in the windows
  where a hand was near the board they swung much wider, `A7` ranging
  0.0136 to 0.1053. `ch0` and `ch1` were unmoved throughout, so this is
  pickup on the open pins rather than anything in the reading path.
- **The ADC is held in reset until a program using analog inputs
  starts.** `PRU.cpp` releases `gemAdcPin` (GPIO0_53, `P2_18`) inside
  the PRU initialisation, and only when `analogInChannels` is non-zero.
  Nothing measured on the analog pins before then means anything —
  including the `REF` pin on P1, which reads 0 V with the board idle.

## What a digital pin does

Collected 2026-08-09 with `bela/examples/io_digital`, wired as a
loopback — `D0` driving `D1` through 1 kΩ — with an LED on `D2` for the
things a printed number cannot settle. The probe toggles the output
every eight blocks at a frame in the middle of the block and scans every
frame of the input for the edge, so the write-to-read distance comes out
in digital frames rather than in blocks.

- **Every channel starts as an input.** The digital word of the first
  block, before this crate has said anything to it, is `0x0000ffff`:
  all sixteen direction bits set, no value bits. That is
  `PinMode::default()` and the "1 means input" reading of the layout,
  both confirmed on the device. `PRU.cpp` agrees from the other side —
  it opens all sixteen pins with `Gpio::INPUT` during initialisation.
- **`pin_mode` moves the bit the accessors think it moves.** After
  making `D0` and `D2` outputs and leaving `D1` an input the word is
  `0x0000fffa` — bits 0 and 2 cleared, bit 1 untouched, nothing else
  changed.
- **The loopback works, and the index is the number on the
  silkscreen.** Every write produced exactly one edge on the input:
  344 or 345 edges a second against the same number of writes, with no
  misses. The LED wired to `D2` blinks when channel 2 is written.
- **Reading an output channel gives back what was written, not the
  pin.** `digital_read` on the channel just written echoes the value
  every time, which is what sharing bit *n*+16 with `digital_write`
  implies.
- **Outputs persist between blocks, as `Bela.h` says.** The probe
  writes once every eight blocks and counts unasked-for edges
  separately; that count stayed at 0. A pin that reverted when nobody
  wrote it would have shown up there.
- **The loopback latency is exactly two blocks plus one frame, with no
  jitter.** Measured at every period size that works — 16, 64, 128,
  129, 160, 192, 255 — the write-to-read distance was a single value,
  never a range, and always `2 * period + 1` digital frames. A value
  written at frame *f* of block *n* is readable at frame *f*+1 of block
  *n*+2. The 128-frame boundary where libbela moves `render` behind a
  context FIFO does not appear in it.
- **Digital I/O stops working entirely at a period of 256 frames or
  more, and nothing says so.** At 256 and at 320 not one edge arrived,
  and the LED on `D2` did not blink either, so it is the whole
  transfer and not one direction of it. `Bela_initAudio` succeeds, the
  audio runs, and no warning is printed. The reason is in `PRU.cpp`,
  where the PRU's shared-memory digital buffer is declared as
  `PRU_MEM_DIGITAL_BUFFER1_OFFSET 0x400` — "256 words … 256 is the
  maximum number of frames allowed" — and nothing checks `digitalFrames`
  against it. The comment's 256 is optimistic by one: 255 is the
  largest period at which digital I/O was seen to work.
- **A program leaves its digital outputs driving after it exits, and
  the next program to start clears them.** The LED stayed lit after a
  run that ended with `D2` high — teardown does not return the pin to
  an input — and went out the moment the following run started, which
  is the initialisation opening all sixteen pins as inputs again. So a
  pin left high outlives its process, but only until something else
  brings the audio system up.

## The Multiplexer Capelet

Collected 2026-08-07: partly on the board with a throwaway C++ project
that printed the multiplexer fields of `BelaContext`, partly by reading
the board's own sources (`/root/Bela`, `/opt/source/dtb-6.12-Beagle`).
The question behind it is whether the Capelet API in `Bela.h` means
anything on a Gem; the short answer is that the software path works and
the hardware cannot be attached.

- **The Capelet is not a Gem accessory.** It is an add-on for the Bela
  cape — Bela's shop says it "is not compatible with Bela Gem [and]
  only works in conjunction with a Bela cape", and the knowledge base
  adds that it is not compatible with a Bela Mini either. Its channel
  select lines are six PRU outputs on `P8.41`–`P8.46`
  (`pr1_pru1_pru_r30_[0-5]`, `MODE5 | OUTPUT | PRU`), which is a
  BeagleBone header; a Gem has PocketBeagle 2's `P1`/`P2`.
- **Every select line does reach a PocketBeagle 2 header pin, but the
  stock overlay routes none of them.** Cross-referencing the header
  table (`/opt/source/.../pocketbeagle2-pins.txt`) with what
  `PB2-BELA.dtbo` claims and what `bela_hw_settings.h` uses each pin
  for:

  | `R30` | pin (ball, mode) | what the Gem uses it for |
  |---|---|---|
  | `GPO0` | P1_02 (AA19, 3) / **P2_02 (U22, 2)** | audio (`MCASP2_AXR4`) / blue LED (`GPIO0_45`) |
  | `GPO1` | **P1_35 (AE21, 3)** / P2_04 (V24, 2) | nothing / red underrun LED (`GPIO0_46`) |
  | `GPO2` | P1_04 (Y18, 3) / **P2_06 (W25, 2)** | audio / stop button (`GPIO0_47`) |
  | `GPO3` | **P2_08 (W24, 2)** / P2_31 (AA18, 3) | nothing libbela names (`GPIO0_48`) / audio |
  | `GPO4` | P2_10 (AD21, 3) / **P2_20 (Y25, 2)** | audio / SPI DAC chip select (`GPIO0_49`) |
  | `GPO5` | **P1_20 (Y24, 2)** | nothing |

  None of the six needs an audio, SPI-ADC, I²C or ADC-reset pin
  (`P2_18`, `GPIO0_53`), and none collides with the 16 digital channels
  (`P1_21`–`P2_35`). So a hand-wired rig is conceivable at the cost
  of two LEDs, the stop button and a custom overlay — but on an
  unmodified image `PB2-BELA.dtbo` leaves all six in GPIO mode, where
  the PRU's `r30` writes reach no pin at all.
- **libbela still implements the whole path on this board, and the PRU
  firmware still runs the multiplexer state machine.** `--mux-channels`
  is in the option list, `pru/pru_rtaudio_irq.p` has `ENABLE_MUXER`
  enabled, and with `-X 8` a Gem Stereo initialises, runs and stops
  with no complaint.
- **The state machine can be caught running, but only at a period size
  that makes it drift.** With mux 8 and 16 analog frames per block,
  `multiplexerStartingChannel` stays 0 for every block, which proves
  nothing: it is what a PRU that never wrote `COMM_MUX_END_CHANNEL`
  would also give. At `--period 20` it advances by 2 per block — 4, 6,
  0, 2, … with 8 channels, and 0, 2, 0, 2 with 4 — and that value is
  derived from what the PRU writes, so the firmware is cycling. The
  step of 2 is `10 mod 8`: the hardware still samples 8 channels at
  22050 Hz (10 frames per 20-frame block) and the resampling to 44100
  that `uniformSampleRate` performs happens afterwards.
- **The demultiplexed buffer is filled.** With `-X 8` the context
  carries `multiplexerChannels = 8` and a non-null `multiplexerAnalogIn`
  whose entries track the analog inputs. With nothing wired to the
  board, and with no select lines routed, what the eight slots hold is
  the same input sampled at eight different times — not eight pins.
- **Off, the context reports 0 channels and a null buffer** —
  `multiplexerChannels = 0`, `multiplexerAnalogIn = (nil)`, measured in
  `setup` and in the block. The header's field comment says the count
  "will be 2, 4 or 8 [...] otherwise it will be 1"; the value is 0. And
  `multiplexerChannelForFrame` returns **1**, not 0, for a disabled
  multiplexer — its guard is `if(multiplexerChannels <= 1) return 1;`.
  A safe wrapper mirroring the C helper has to reproduce that, or
  refuse to answer at all.
- **The accepted counts are 0, 2, 4 and 8; `1` is an error.** `-X 1`
  and `-X 3` both print `Error: N is not a valid number of multiplexer
  channels (options: 0 = off, 2, 4, 8)` and fail
  `Bela_initAudio` — which, as "Audio thread" above records, poisons
  the process.
- **Two more combinations fail, one of them badly.** `-X 8` with fewer
  than 8 analog inputs (`-C 4`) is refused with `Error: multiplexer
  capelet can only be used with 8 analog channels`, and
  `--pru-number 0 -X 8` with `Incompatible settings: multiplexer can
  only be run using PRU 1`. But `-X 8 --use-analog 0` passes every
  check libbela makes on the ARM side and dies in the firmware instead:
  `Invalid PRU configuration settings`, `PRU timeout`, `McASP error,
  abort`.
- **What cannot be measured here** is the only thing the API's
  documentation is really about: which Capelet pin a given
  `multiplexerAnalogRead(context, x, y)` reading came from. That needs
  the Capelet, and no Gem can carry one.

## Command-line options

Collected 2026-08-08 with `scripts/probe-command-line.sh`, which runs
`bela/examples/command_line` one case per process and records where
each one was caught. The question is not whether libbela accepts an
option — it is which of four places notices, because they cost the
caller different things.

- **There are four places, and only two of them leave the caller able
  to do anything about it.** The parse (`Bela_getopt_long` returns a
  positive value, which this crate turns into `Error::CommandLine`);
  `Bela_initAudio` (`Error::Init` — and, as "Audio thread" above
  records, the process can no longer build any audio system);
  `Bela_startAudio` (`Error::Start`, by which point `setup` has already
  run); and nowhere at all. A case in the last group either runs, or
  ends the process from inside libbela with no Rust error returned.

  | case | caught |
  |---|---|
  | `--nonsense` | parse |
  | `--period` (no value) | parse |
  | `--receive-port 9998` | parse |
  | `--transmit-port 9999` | parse |
  | `--server-name 127.0.0.1` | parse |
  | `--json-string {` | **`SIGABRT` in the parse** |
  | `-r abc` | `Bela_initAudio` |
  | `-r -5` | `Bela_initAudio` |
  | `--pru-number 5` | `Bela_initAudio` |
  | `-X 1` | `Bela_initAudio` |
  | `--pru-file /nonexistent` | `Bela_startAudio` |
  | `-p 0` (i.e. 1), `-p 3` | **nothing; the PRU gives up** |
  | `-p 2`, `-p 4` … `-p 16` | nothing; they ran |
  | `-p 1 -N 0`, `-p 3 -N 0`, `-p 3 -C 2` | nothing; they ran |
  | `-X 8 -N 0` | **nothing; the PRU gives up** |
  | `--json-file /nonexistent.json` | nothing; it ran |
  | `-C 0`, `-C 3`, `-C 100` | nothing; it ran |
  | `-B 0`, `-B 100` | nothing; it ran |
  | `-N 2`, `-U 5` | nothing; it ran |
  | `--board BelaMini`, `--board nonsense` | nothing; it ran |
  | `--stop-button-pin 9999` | nothing; it ran |
  | `--disabled-digital-channels 65535` | nothing; it ran |
  | `--codec-mode garbage` | nothing; it ran |
  | `-X 8` | nothing; it ran |
  | `-Y 0,1` | nothing; it ran |

- **`Bela_usage` advertises three options libbela does not implement.**
  `--receive-port`, `--transmit-port` and `--server-name` are printed
  by it — and therefore by this crate's `print_usage` — but appear in
  neither `gDefaultShortOptions` nor `gDefaultLongOptions`, so getopt
  reports them as unrecognised and the parse fails. A program that
  prints the list this crate offers is telling its users about options
  it will then refuse.
- **A malformed `--json-string` takes the process down with it.**
  `--json-string {` throws an uncaught `nlohmann::json` `parse_error`
  and the process ends on `SIGABRT` (exit 134) — before any Bela call,
  with no error to return and nothing a caller could catch. The
  neighbouring option fails the other way: `--json-file` naming a
  missing file prints `jsonSettingsInitFile: missing, empty or
  corrupted file` and then carries on with the settings it already had.
- **Channel counts are reshaped rather than refused.** `-C` snaps to 8,
  4 or 2 — `-C 0` and `-C 3` both give **2** analog inputs, `-C 100`
  gives 8 — and `-B` clamps to 16, with `-B 0` turning digital off
  altogether. Asking for no analog channels and being given two is the
  one worth remembering: `-N 0` is the way to have none.
- **A period size the hardware cannot keep up with is caught by
  nothing until the PRU gives up, and `setup` has run by then.** `-p 0`
  is clamped to 1 by the parser and `-p 3` is taken as it stands; both
  reach `setup`, which reports the block size, and then print `PRU SPI
  transactions not done on time`, `PRU timeout` and `McASP error,
  abort`. The process exits 1 **from inside libbela** — `run_with_args`
  returns nothing, so a program cannot report on this or clean up after
  it. `-X 8 -N 0` ends the same way.
- **There is no floor to check for: the sizes that fail are 1 and 3,
  and only with eight analog inputs.** Every integer from 1 to 16 was
  run with the defaults, and 2 and 4 through 16 all came up — odd sizes
  included. Both failures move as soon as the analog configuration
  does:

  | `--period` | default (8 analog in) | `-N 0` | `-C 2` |
  |---|---|---|---|
  | 1 | PRU timeout | runs | — |
  | 2 | runs | — | — |
  | 3 | PRU timeout | runs | runs |
  | 4–16 | run | — | — |

  So `periodSize >= n` is not a check anyone can write: 2 is fine where
  3 is not, and 3 is fine as soon as there are two analog inputs rather
  than eight. **Why 3 fails where 2 does not was not established** —
  both give the same hardware analog frame count on this board — and
  guessing at the mechanism is not what this file is for. The runs
  above were repeated at 15 seconds as well as the probe's default 4,
  with the same outcome each way and no underrun reported by the ones
  that ran.
- **A sample rate of 0 fails initialisation, and says something else.**
  `-r abc` (`atof` gives 0) and `-r -5` (clamped to 0) both fail with
  `Error: audio sampling rate is 0. Is the codec enabled?` followed by
  `Error while retrieving hardware settings Bela hardware: is a cape
  connected?` — a message about hardware for a number the command line
  supplied.
- **`--board` naming a board you do not have is ignored.** With
  `--board BelaMini` on this board, verbose logging reports `Hardware
  specified at the command line: BelaMini` and then `Hardware to be
  used: GemStereo`; the run is indistinguishable from one without the
  option. An unrecognised name behaves the same way.
- **Several options are accepted and then do nothing visible.**
  `--codec-mode garbage`, `--disabled-digital-channels 65535` (the
  context still reports 16 digital channels), `-N 2` and `-U 5` — the
  last two are documented as booleans — and `-Y 0,1`, which asks for an
  Audio Expander Capelet that, like the Multiplexer, is not a Gem
  accessory. `--stop-button-pin 9999` prints
  `Gpio::getBankAddress(): requested module 62 out of range` and runs
  on without a working stop button.

## Operations

- USB gadget network: the board is `bela.local` (host-side interface
  gets `192.168.7.1/24`). `ssh root@bela.local` works without a
  password; there is also a `bela` user with one-time password
  `temppwd`.
- Networking is managed by `systemd-networkd`, and the image ships
  `.network` units for the gadget and wireless interfaces only — there
  is none for a wired interface, so a USB Ethernet adapter binds its
  driver but stays `unmanaged`. Setup and measured throughput for both
  paths: [board-network.md](board-network.md).
- The kernel command line carries `net.ifnames=0`, so interfaces keep
  kernel-style names (`eth0`, not `enx…`).
- Both USB ports are USB 2.0, and the SoC has no USB 3 at all, so
  480 Mbit/s is a hardware ceiling rather than a setting that could be
  lifted. The device tree (`ti,am625`) carries two DWC3 controllers,
  both declared `maximum-speed = "high-speed"`: `usb@31000000`
  (`dr_mode = peripheral`, the gadget tether) and `usb@31100000`
  (`dr_mode = host`, where a USB Ethernet adapter goes). Only one USB
  bus registers and it is `version 2.00`; `dmesg` reports `xhci-hcd:
  USB3 root hub has no ports`, and there is no SerDes node to carry
  SuperSpeed. A gigabit Ethernet adapter cannot be filled.
- Services: `bela_daemon.service` (IDE/daemon — stop with
  `systemctl stop bela_daemon` before running standalone binaries; not
  exercised yet), `bela_button.service` (cape button monitor),
  `bela-usb-gadgets.service`.
- Paths to sync as the cross-compilation sysroot: `/root/Bela/include`,
  `/root/Bela/lib`, `/usr/evl`, `/usr/local/lib` (seasocks),
  `/usr/include`, `/usr/lib/aarch64-linux-gnu`,
  `/lib/aarch64-linux-gnu`.
