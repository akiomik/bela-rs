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
  program that calls `Bela_initAudio` leaves a cache behind, which is
  why the smoke test takes its reading before the first binary runs
  rather than around the one probe that detects on purpose.
- **Nothing here is measured on a Gem Multi.** There is no Multi to
  measure on, so `Board::GemMulti` is a name taken from the headers and
  nothing in this file claims what one reports.

## Build and link information

Captured from a verbose on-board build:
`make -C /root/Bela PROJECT=<name> AT=` (compiler is `clang++`).

- Include paths: the project dir, `/root/Bela/include/legacy`,
  `/root/Bela/include`, `/root/Bela/build/pru`, `/root/Bela`,
  `/usr/evl/include`.
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
