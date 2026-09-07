# Scope

What these crates wrap, what is left out on purpose, and what is
simply not written yet.

The inventory below is the board's rather than upstream's:
`/root/Bela/libraries` and `/root/Bela/include` on the image this crate
is developed against — Bela 1.18.0, Debian Bookworm image 2026-03-25 —
as synced by `scripts/sync-sysroot.sh` and read there on 2026-09-08.
That checkout is `fb362a5` with an image overlay on top, and its
`Bela.h` reports a version no published branch carries, so a library
listed here may be missing from [BelaPlatform/Bela] and the other way
round. [board-facts.md](board-facts.md) records how that was
established.

This file follows `main`, not a release. What it calls wrapped is
wrapped on `main` — at least what the newest published version has, and
possibly more. [CHANGELOG.md](../CHANGELOG.md) is where a particular
version answers for itself.

[BelaPlatform/Bela]: https://github.com/BelaPlatform/Bela

## The rule

Bela's C++ libraries fall into two groups.

Some of them reach hardware, or real-time machinery, that only Bela's
own code knows how to drive: the Trill protocol, the IDE channel the
scope draws through, the boundary between an audio thread and an
ordinary one. Nothing on crates.io substitutes for those. A binding is
the only way a Rust program gets them at all.

The rest are ordinary code that happens to be written in C++ — a
biquad, a delay line, a UDP socket, a WAV reader. Wrapping one buys a
C++ shim, a build dependency and an `unsafe` boundary, in exchange for
something a Rust crate already does, and does with the borrow checker's
help.

So: **wrapped if it needs Bela's own code to work at all; otherwise
left to the application.** Everything below is sorted by that rule, and
where the rule is the whole reason a library is absent, the entry says
so rather than pretending the work is queued.

The rule yields to a measurement. Several of the excluded libraries are
NEON-tuned, and if one of them is measurably faster on a Gem than what
a Rust program can write, that is an argument for wrapping it. Nothing
here has been measured that way; the exclusions below are arguments
from what a wrapper would buy, not from timings.

## What is wrapped today

The C core API — `BelaContext`, the `setup`/`render`/`cleanup`
callbacks, `Bela_initAudio` and its siblings, the codec levels — is
what these crates are for, and the exceptions are listed under [Corners
of the C API](#corners-of-the-c-api-that-are-not-reached) below.

Of the 38 directories under `libraries/`, three are answered:

| Library | State |
|---|---|
| `Midi` | Wrapped, through a C++ shim this workspace compiles. [midi.md](midi.md) records what part of it the crate uses and why output leaves `render` through a queue of the crate's own. |
| `Fft` | Not wrapped, and will not be: the class is a C++ wrapper over NE10, and `bela-sys` declares NE10's transform directly. `RealFft` is what a program uses instead. [fft.md](fft.md). |
| `ne10` | The real-to-complex pair only (`ne10_fft_r2c_1d_float32_neon` and its inverse, with the alloc/destroy calls around them). The four dozen others that `NE10_dsp.h` declares — FIR, IIR, vector maths — are not, and fall under the rule above rather than under a plan. |

## Not written yet

Six of the 38 need Bela's own code, so the rule says wrap them; nobody
has. Nothing here is promised, and each entry is an invitation rather
than a backlog with an order.

| Library | What a binding would be for |
|---|---|
| `Scope` | The browser oscilloscope. The only debugging output this crate has today is `rt_print!` and `rt_println!`, which makes this the most valuable single entry on the list. Pulls in `WSServer`, `JSON` and `RtLock`. |
| `Trill` | The Trill capacitive sensors. Derives from `I2c` and carries Bela's centroid detection; the protocol is not something an application writes for itself. |
| `Gui`, `GuiController` | Sliders and plots in the browser, over the IDE's websocket channel. |
| `WSServer` | The transport under the two rows above. Worth wrapping on its own only if something wants the channel without the scope or the GUI on top. |
| `Pipe` | A typedef of `RtNonRtMsgFifo`, kept for legacy code; the header to wrap is `include/RtMsgFifo.h`. This is the real-time-to-ordinary-thread boundary, which is currently only crossable through an `AuxiliaryTask` and whatever the application puts beside it. |

The headers under `include/` divide the same way. Not wrapped, and
under the rule they should be:

- **The real-time boundary**: `AuxTaskNonRT`, `AuxTaskRT`, `RtThread`,
  `RtLock`, `RtMsgFifo`, `RtWrappers`, `SchedulableTask`. Bela's
  real-time auxiliary task is reached through the C API already, as
  [`AuxiliaryTask`](../bela/src/task.rs); the *non*-real-time task —
  Bela's own way to run work on an ordinary thread that a render
  callback can trigger — has no equivalent here.
- **Peripherals outside the audio context**: `Gpio`, `I2c`. These are
  sysfs and `ioctl` wrappers, so by the rule above they are the
  application's to write — except that `Gpio` is how libbela drives the
  LEDs and the stop button, which makes what it may touch a question
  about this board rather than about Linux. The C functions underneath
  it are [#156](https://github.com/akiomik/bela-rs/issues/156).
- **Block-size adaptation**: `BelaContextFifo`, `BelaContextSplitter`,
  `BelaContextManager`. These are libbela's own, not an application's,
  but what the first one does to digital output persistence is
  [#89](https://github.com/akiomik/bela-rs/issues/89), so a program can
  be affected by machinery it cannot see.

## Out of scope on purpose

### Boards other than Bela Gem

The original Bela and Bela Mini are armv7, and every header, library
and measurement here comes from a Gem image. A port needs its own
headers, its own real-time runtime and its own measurements. The README
says the rest, and a port from someone with the hardware is welcome.

### The Multiplexer Capelet

`multiplexerAnalogRead` and `multiplexerChannelForFrame` have no safe
accessors. Like the rest of the I/O accessors they are `static inline`
in `Bela.h`, so what a Rust program would have is a rewrite rather than
a binding, and that rewrite is the part not written. The Capelet is an
accessory for the
original Bela cape and cannot be attached to a Gem, so what a reading
means — which Capelet pin it came from — cannot be checked on the board
this crate is measured against. What a Gem does with `--mux-channels`
regardless is in [board-facts.md](board-facts.md).

### Hosting another language runtime

`libpd`, `BelaLibpd`, `pd-externals`, `csound` and `BelaArduino` run
Pure Data patches, Csound orchestras and Arduino sketches on the board.
Each is a project of its own rather than a binding, and none of them
gets easier by being reached from Rust: a program that wants to run a Pd
patch is better served by Bela's own build for it.

### Code an application writes better in Rust

Under the rule, these are absent because a wrapper would buy nothing,
not because the work is queued. Twenty-four of the 38 libraries are here:

| Libraries | Why not |
|---|---|
| `ADSR`, `Biquad` (`QuadBiquad`), `Convolver`, `DelayLine`, `EnvelopeDetector`, `OnePole`, `Oscillator`, `OscillatorBank`, `math_neon` | Ordinary DSP with no Bela hardware in it. Rust has these, or they are a few lines in the application, and either way they keep the borrow checker. `QuadBiquad` (`arm_neon.h`), `OscillatorBank` ("highly optimized, written in NEON assembly"), `Convolver` and `math_neon` are the NEON-tuned ones, and so the likeliest to be overturned by a measurement — `OscillatorBank` above all, having no Rust equivalent to lose to. |
| `Debounce` (`BelaDebounce`, `GpioDebounce`), `Encoder` (`BelaEncoder`), `PulseIn`, `ShiftRegister`, `SteppedPot` | Logic on top of `digital_read` and `digital_write`, which the crate already has. Each is small enough that wrapping it costs more than writing it. |
| `UdpClient`, `UdpServer`, `OscSender`, `OscReceiver`, `Serial` | Protocols, not hardware. `std::net`, a serial crate and an OSC crate cover them; `Serial` in particular is a termios wrapper over `/dev/ttyS*` with nothing Bela-specific in it. |
| `WriteFile`, `Spi`, `Eeprom` | Linux interfaces with a thread or an `ioctl` in front of them, and no Bela code behind them: `WriteFile` is a ring drained by a `std::thread` it puts on `SCHED_FIFO`, `Spi` is `linux/spi/spidev.h`, `Eeprom` is the `24cXXX` driver's sysfs files. Rust reaches all three. What Bela's versions carry that a Rust program cannot write for itself is not code but board facts — which bus, which device node, what libbela has already claimed — and those belong in [board-facts.md](board-facts.md). |
| `AudioFile`, `sndfile` | File I/O. The utility half is libsndfile, which Rust has several answers for. `AudioFileReader`'s streaming mode does use a thread of the library's own, and is the part of this row that could be argued back the other way. |

The utility headers under `include/` go the same way:
`CircularBuffer`, `DataBuffer`, `z_ringbuffer`, `IirFilter`,
`FormatConvert`, `MiscUtilities`, `DigitalChannelManager`,
`DigitalToMessage`, `JSON`/`json.hpp`/`JSONValue`, `oscpkt.hh` and
`stats.hpp`.

### libbela's internals

`PRU.h`, `PruManager.h`, `PruBinary.h`, `PruArmCommon.h`, `Mcasp.h`,
`Mmap.h`, `AudioCodec.h`, the codec classes (`I2c_Codec`, `Es9080_Codec`,
`Tlv320*`, `Gem_Multi_Codec`, `Spi_Codec` and the rest),
`InternalBelaContext.h`, `board_detect.h`, `bela_hw_settings.h`,
`bela_sw_settings.h`, `digital_gpio_mapping.h`, `m_private_utils.h` and
`legacy/`. These are how libbela is built, not an API it offers. What
an application needs from them it gets through the C API — board
detection through `Bela_detectHw`, which `Board::detect` wraps.

## Corners of the C API that are not reached

Bound in `bela-sys`, no safe wrapper in `bela`:

| Function | What it is for |
|---|---|
| `Bela_setAdcLevel`, `Bela_setADCLevel` | The ADC volume, and the only codec control with no wrapper at all — the second is the deprecated `channel = -1` spelling of the first, which is not deprecated. It is a different stage from the PGA gain `Bela::set_audio_input_gain` sets, and `adcGains` is empty in `Bela_defaultSettings` where the other three gain arrays are not (both in [board-facts.md](board-facts.md)). What it does on a Gem has not been measured, so neither is it known what a wrapper would be worth. |
| `Bela_setPgaGain`, `Bela_setDacLevel`, `Bela_setDACLevel`, `Bela_setHeadphoneLevel` | Deprecated in `Bela.h`, every one of them, and each a second spelling of a call the crate already wraps: `Bela_setPgaGain` is `Bela_setAudioInputGain` with its two arguments the other way round, `Bela_setDacLevel` is told to use `Bela_setLineOutLevel` instead, and `Bela_setDACLevel` and `Bela_setHeadphoneLevel` are the `channel = -1` case of `Bela_setDacLevel` and of `Bela_setHpLevel`, which `Bela::set_headphone_level` wraps. Wrapping a deprecated spelling of a wrapped call buys nothing. |
| `Bela_runInSameThread` | Running the audio loop on the calling thread instead of a thread of libbela's own, which `Bela::start` does. |
| `Bela_setUserData` | Replacing the pointer handed to the callbacks. The crate owns that pointer, so exposing it needs a story for what happens to the application it points at. |
| `Bela_setVerboseLevel` | Verbosity after `Bela_initAudio`; `Settings::verbose` only sets it before. |
| `Bela_printFlushBuffers` | Flushing the real-time print buffers, which `rt_println!` fills. |
| `Bela_initRtBackend` | Bringing the real-time backend up separately from the audio system. |
| `Bela_gettime`, `Bela_clock_gettime`, `Bela_nanosleep` | Real-time safe time and sleep. `std::time` is not safe to call from a render callback, so these have no Rust equivalent on the audio thread. |
| `Bela_HwConfig_new`, `Bela_HwConfig_delete` | The hardware configuration object. |

Not bound at all, and for two different reasons, which is worth
keeping apart: only one of them could be answered by changing the
generator.

`cargo xtask bindgen` allows through the functions named `Bela_*` and
`rt_*` and nothing else (`xtask/src/generate.rs`). **One family is lost
to that alone:**

- **`GPIOcontrol.h`** — thirteen functions, `gpio_setup` through
  `led_set_trigger`, every one of them exported from the `libbela` the
  crate already links. The header is vendored, `Bela.h` includes it,
  and the allowlist is the only thing between it and Rust. That is an
  oversight rather than a decision:
  [#156](https://github.com/akiomik/bela-rs/issues/156).

**Everything else the headers declare and `bindings.rs` does not is
`static inline`**, which bindgen skips whatever the allowlist says —
`wrap_static_fns` is not enabled — and for which `libbela` exports no
symbol. Widening the allowlist would produce nothing for these, and a
hand-written `extern` would fail to link. Having them in Rust means
writing them in Rust, which is what the crate does:

- The I/O accessors — `audioRead` and `audioWrite`, `analogRead`,
  `analogWrite` and `analogWriteOnce`, `digitalRead`, `digitalWrite`
  and `digitalWriteOnce`, `pinMode` and `pinModeOnce`, and the `NI`
  variants of each — rewritten as the methods on `BlockContext` and
  `RenderContext`, which do the same indexing against the same
  `BelaContext` fields.
- `constrain`, `map`, `min` and `max` from `Utilities.h` — `constrain`
  and `map` are public in `bela`, and the other two are `std`.
- `multiplexerAnalogRead` and `multiplexerChannelForFrame`, the Capelet
  accessors. What is deliberate about those is the absence of the Rust
  rewrite, not the absence of a binding: like every accessor beside
  them, they could not have been bound. See above for why they are not
  written.

Excluded deliberately, with the reason recorded where it is done:

- The `FILE*` and `va_list` printf variants — `Bela_fprintf`,
  `Bela_vprintf`, `rt_fprintf`, `rt_vfprintf` — are blocklisted in
  `xtask/src/generate.rs`: they would drag glibc internals into the
  bindings and are not usable from Rust anyway. `Bela_printf` and
  `rt_printf` are bound, and `rt_println!` is built on the second.
- `Bela_runAuxiliaryTask` is C++ only — `Bela.h` declares it inside an
  `#ifdef __cplusplus`, for the sake of its default arguments — so a C
  binding cannot reach it at all. It is `Bela_createAuxiliaryTask`
  followed by `Bela_scheduleAuxiliaryTask`, and `AuxiliaryTask` wraps
  both.

## Settings and context fields not exposed

`Settings` writes 15 of the fields in `BelaInitSettings`, and the audio
system writes five more that are not an application's to choose: the
`setup`, `render_pre`, `render`, `render_post` and `cleanup` pointers,
which `Bela::new` sets to the trampolines that reach
[`BelaApplication`](../bela/src/application.rs) (`system.rs:387-391`).
Everything else is left at whatever `Bela_defaultSettings()` — and
therefore the board's `~/.bela/belaconfig` — gives it. Reaching one
means
`Settings::apply_to` on a `BelaInitSettings` of the caller's own and
`Bela_initAudio` driven by hand, which is what that method is the
escape hatch for:

- `numAudioInChannels`, `numAudioOutChannels`
- `lineOutGains`, `headphoneGains`, `audioInputGains` and `adcGains`,
  the `BelaChannelGainArray` fields that carry the codec levels at
  start-up, together with the deprecated scalars they replaced —
  `dacLevel`, `adcLevel`, `headphoneLevel` and `pgaGain`. That the
  arrays are absent is argued rather than overlooked, in `level.rs`'s
  own documentation and measured in
  [board-facts.md](board-facts.md): `Bela_initAudio` applies each array
  by calling the very functions `Bela::set_line_out_level` and its
  siblings wrap, and the codec writes its registers only once audio
  starts, so a call between `Bela::new` and `Bela::start` reaches the
  hardware in the same state and at the same moment. There is nothing
  left for a setting to carry but a second way to say it.
- `interleave`, `analogOutputsPersist`, `disabledDigitalChannels`
- `audioThreadStackSize`, `auxiliaryTaskStackSize`
- `ampMutePin`, `codecMode`, `board`, `projectName`
- `pruNumber`, `pruFilename`
- `audioExpanderInputs`, `audioExpanderOutputs`
- `audioThreadDone`, the callback that runs when the audio thread ends
- `numMuxChannels` — the Capelet, deliberately

On `BelaContext`, the accessors cover the buffers, the frame and
channel counts, the sample rates, `audioFramesElapsed`, `underrunCount`
and the thread identity. Not exposed: `flags`
(`BELA_FLAG_INTERLEAVED`, `BELA_FLAG_ANALOG_OUTPUTS_PERSIST`,
`BELA_FLAG_DETECT_UNDERRUNS`, `BELA_FLAG_OFFLINE`), `projectName`,
`audioExpanderEnabled`, and the multiplexer fields.

## If you want one of these

Open an issue before writing much of it. Anything that reaches the
board has to be measured on one before it can be documented as working,
which is the slow half — [CONTRIBUTING.md](../CONTRIBUTING.md) sets out
what that means, and [board-facts.md](board-facts.md) is where the
measurements land. Issues exist for the entries that have one; the rest
of this file is inventory, and an entry here is not a claim that anyone
is working on it.
