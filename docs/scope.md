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
wrapped in full, with the exceptions listed under [Corners of the C
API](#corners-of-the-c-api-that-are-not-reached) below.

Of the 38 directories under `libraries/`, three are answered:

| Library | State |
|---|---|
| `Midi` | Wrapped, through a C++ shim this workspace compiles. [midi.md](midi.md) records what part of it the crate uses and why output leaves `render` through a queue of the crate's own. |
| `Fft` | Not wrapped, and will not be: the class is a C++ wrapper over NE10, and `bela-sys` declares NE10's transform directly. `RealFft` is what a program uses instead. [fft.md](fft.md). |
| `ne10` | The real-to-complex pair only (`ne10_fft_r2c_1d_float32_neon` and its inverse, with the alloc/destroy calls around them). NE10's other 60-odd functions — FIR, IIR, vector maths — are not declared, and fall under the rule above rather than under a plan. |

## Not written yet

These need Bela's own code, so the rule says wrap them; nobody has.
Nothing here is promised, and each entry is an invitation rather than a
backlog with an order.

| Library | What a binding would be for |
|---|---|
| `Scope` | The browser oscilloscope. The only debugging output this crate has today is `rt_println!`, which makes this the most valuable single entry on the list. Pulls in `WSServer`, `JSON` and `RtLock`. |
| `Trill` | The Trill capacitive sensors. Derives from `I2c` and carries Bela's centroid detection; the protocol is not something an application writes for itself. |
| `Gui`, `GuiController` | Sliders and plots in the browser, over the IDE's websocket channel. |
| `WSServer` | The transport under the two rows above. Worth wrapping on its own only if something wants the channel without the scope or the GUI on top. |
| `WriteFile` | Real-time safe logging to disk: a render callback pushes, a background thread of the library's own writes. The thread is the part a Rust program cannot get from crates.io. |
| `Pipe` | A typedef of `RtNonRtMsgFifo`, kept for legacy code; the header to wrap is `include/RtMsgFifo.h`. This is the real-time-to-ordinary-thread boundary, which is currently only crossable through an `AuxiliaryTask` and whatever the application puts beside it. |
| `Spi`, `Eeprom` | Board peripherals — user SPI, and the I2C EEPROM. The protocols are Linux's, and Rust has crates for both; what Bela's versions carry is the board wiring, which device node is which and what libbela is already driving. |

The headers under `include/` divide the same way. Not wrapped, and
under the rule they should be:

- **The real-time boundary**: `AuxTaskNonRT`, `AuxTaskRT`, `RtThread`,
  `RtLock`, `RtMsgFifo`, `RtWrappers`, `SchedulableTask`. Bela's
  real-time auxiliary task is reached through the C API already, as
  [`AuxiliaryTask`](../bela/src/task.rs); the *non*-real-time task —
  Bela's own way to run work on an ordinary thread that a render
  callback can trigger — has no equivalent here.
- **Peripherals outside the audio context**: `Gpio`, `I2c`. The C
  functions underneath `Gpio` are
  [#156](https://github.com/akiomik/bela-rs/issues/156).
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

`multiplexerAnalogRead` and `multiplexerChannelForFrame` are not bound
and have no safe accessors. The Capelet is an accessory for the
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
not because the work is queued. Twenty-one of the 38 libraries are here:

| Libraries | Why not |
|---|---|
| `ADSR`, `Biquad` (`QuadBiquad`), `Convolver`, `DelayLine`, `EnvelopeDetector`, `OnePole`, `Oscillator`, `OscillatorBank`, `math_neon` | Ordinary DSP with no Bela hardware in it. Rust has these, or they are a few lines in the application, and either way they keep the borrow checker. `math_neon` and `Convolver` are the NEON-tuned ones, and so the likeliest to be overturned by a measurement. |
| `Debounce` (`BelaDebounce`, `GpioDebounce`), `Encoder` (`BelaEncoder`), `PulseIn`, `ShiftRegister`, `SteppedPot` | Logic on top of `digital_read` and `digital_write`, which the crate already has. Each is small enough that wrapping it costs more than writing it. |
| `UdpClient`, `UdpServer`, `OscSender`, `OscReceiver`, `Serial` | Protocols, not hardware. `std::net`, a serial crate and an OSC crate cover them; `Serial` in particular is a termios wrapper over `/dev/ttyS*` with nothing Bela-specific in it. |
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
`bela_sw_settings.h`, `digital_gpio_mapping.h` and `legacy/`. These are
how libbela is built, not an API it offers. What an application needs
from them it gets through the C API — board detection through
`Bela_detectHw`, which `Board::detect` wraps.

## Corners of the C API that are not reached

Bound in `bela-sys`, no safe wrapper in `bela`:

| Function | What it is for |
|---|---|
| `Bela_setPgaGain` | The PGA gain directly, per channel. `Bela::set_audio_input_gain` covers the same hardware in decibels. |
| `Bela_runInSameThread` | Running the audio loop on the calling thread instead of a thread of libbela's own, which `Bela::start` does. |
| `Bela_setUserData` | Replacing the pointer handed to the callbacks. The crate owns that pointer, so exposing it needs a story for what happens to the application it points at. |
| `Bela_setVerboseLevel` | Verbosity after `Bela_initAudio`; `Settings::verbose` only sets it before. |
| `Bela_printFlushBuffers` | Flushing the real-time print buffers, which `rt_println!` fills. |
| `Bela_initRtBackend` | Bringing the real-time backend up separately from the audio system. |
| `Bela_gettime`, `Bela_clock_gettime`, `Bela_nanosleep` | Real-time safe time and sleep. `std::time` is not safe to call from a render callback, so these have no Rust equivalent on the audio thread. |
| `Bela_HwConfig_new`, `Bela_HwConfig_delete` | The hardware configuration object. |

Not bound at all. `cargo xtask bindgen` allows through the functions
named `Bela_*` and `rt_*` and nothing else (`xtask/src/generate.rs`),
so what the vendored headers declare under any other name never reaches
`bindings.rs`:

- **`GPIOcontrol.h`** — thirteen functions, `gpio_setup` through
  `led_set_trigger`, every one of them exported from the `libbela` the
  crate already links. The header is vendored, `Bela.h` includes it,
  and the allowlist is the only thing between it and Rust. That is an
  oversight rather than a decision:
  [#156](https://github.com/akiomik/bela-rs/issues/156).
- `constrain`, `map`, `min` and `max` from `Utilities.h`, which are
  reimplemented in Rust instead — `constrain` and `map` are public, and
  the other two are `std`.
- `multiplexerAnalogRead` and `multiplexerChannelForFrame`, the Capelet
  accessors, deliberately — see above.

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

`Settings` writes 15 of the fields in `BelaInitSettings`. The rest are
left at whatever `Bela_defaultSettings()` — and therefore the board's
`~/.bela/belaconfig` — gives them, and reaching them means
`Settings::apply_to` on a `BelaInitSettings` of the caller's own:

- `numAudioInChannels`, `numAudioOutChannels`
- `dacLevel`, `adcLevel`, `headphoneLevel`, `pgaGain` — as starting
  values. Running values are `Bela::set_line_out_level` and its
  siblings.
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
