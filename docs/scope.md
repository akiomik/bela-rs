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

Line numbers follow from that. A citation naming a file under
`libraries/`, `include/` or `core/` is into the board's tree as
`scripts/sync-sysroot.sh` copies it, which is git-ignored — so checking
one takes a board and that script rather than a click, and it will not
land on the same line in [BelaPlatform/Bela]. Citations naming a `.rs`
file, or one of the five vendored headers — `Bela.h`, `Utilities.h` and
`GPIOcontrol.h` under `bela-sys/vendor/bela/`, `NE10_dsp.h` and
`NE10_types.h` under `bela-sys/vendor/ne10/` — are into this repository
and need no board. The three Bela ones are byte-identical to the
board's copies.

This file follows `main`, not a release. What it calls wrapped is
wrapped on `main` — at least what the newest published version has, and
possibly more. [CHANGELOG.md](../CHANGELOG.md) is where a particular
version answers for itself.

[BelaPlatform/Bela]: https://github.com/BelaPlatform/Bela

## The rule

Bela's C++ libraries fall into two groups.

Some of them reach hardware, or real-time machinery, that only Bela's
own code knows how to drive: the Trill protocol, the IDE channel the
scope draws through, the fifo a render callback crosses to get work off
the audio thread. Nothing on crates.io substitutes for those. A binding is
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

Three of the 38 directories under `libraries/` are answered:

| Library | State |
|---|---|
| `Midi` | Wrapped, through a C++ shim this workspace compiles. [midi.md](midi.md) records what part of it the crate uses and why output leaves `render` through a queue of the crate's own. |
| `Fft` | Not wrapped, and will not be: the class is a C++ wrapper over NE10, and `bela-sys` declares NE10's transform directly. `RealFft` is what a program uses instead. [fft.md](fft.md). |
| `ne10` | The real-to-complex pair only (`ne10_fft_r2c_1d_float32_neon` and its inverse, with the alloc/destroy calls around them). The forty-six other plain declarations in `NE10_dsp.h` — the same transforms for `int16` and `int32`, complex-to-complex, the `_c` fallbacks beside the `_neon` versions, and FIR and IIR — are not, nor are the seventeen runtime-dispatch function pointers the header declares beside them, which `bela-sys` bypasses by naming the `_neon` implementations directly. All of it falls under the rule above rather than under a plan. NE10's vector maths is in `NE10_math.h`, which is not vendored at all. |

## Not written yet

Seven of the 38 need Bela's own code, so the rule says wrap them;
nobody has. Nothing here is promised, and each entry is an invitation rather
than a backlog with an order.

| Library | What a binding would be for |
|---|---|
| `Scope` | The browser oscilloscope. The only debugging output this crate has today is `rt_print!` and `rt_println!`, which makes this the most valuable single entry on the list. Reaches `WSServer`, `JSON`, `RtThread` and `MiscUtilities` (`Scope.cpp:3-8`), so it is Bela's code most of the way down. |
| `Trill` | The Trill capacitive sensors. Derives from Bela's `I2c` (`Trill.h:13`) and carries its centroid detection; the protocol is not something an application writes for itself. |
| `Gui`, `GuiController` | Sliders and plots in the browser. `Gui.h:6-11` is `JSON`, `DataBuffer` and `WSServer`; `GuiController` is `Gui` with widgets on top. |
| `WSServer` | The transport under the two rows above: seasocks, driven by an `AuxTaskNonRT` and an `RtLock` (`WSServer.cpp:3-7`). A websocket crate replaces seasocks; the two Bela pieces are the reason this is not simply that. Worth wrapping on its own only if something wants the channel without the scope or the GUI on top. |
| `OscSender` | Sending OSC needs no Bela code, but this implementation does: the send runs on an `AuxTaskNonRT` (`OscSender.cpp:6,30`), the non-real-time task listed below, which is the part a Rust program cannot get from crates.io. `OscReceiver` is the asymmetric case and stays out of scope — its receive loop is a plain `std::thread`. |
| `Pipe` | A typedef of `RtNonRtMsgFifo` (`Pipe.h:4`), kept for legacy code; the header to wrap is `include/RtMsgFifo.h`. This is the real-time-to-ordinary-thread boundary, which is currently only crossable through an `AuxiliaryTask` and whatever the application puts beside it. |

The headers under `include/` divide the same way, and mostly onto this
side of it:

- **The real-time boundary**: `AuxTaskNonRT`, `AuxTaskRT`, `RtThread`,
  `RtLock`, `RtMsgFifo`, `RtWrappers`, `SchedulableTask`. Bela's
  real-time auxiliary task is reached through the C API already, as
  [`AuxiliaryTask`](../bela/src/task.rs). `AuxTaskNonRT` has no
  equivalent here: a render callback triggers it by writing to an
  `RtNonRtMsgFifo` (`AuxTaskNonRT.h:19`, `AuxTaskNonRT.cpp:11-25`),
  where `AuxTaskRT` holds an `RtMsgFifo` (`AuxTaskRT.h:21`) — two
  classes in the same header (`RtMsgFifo.h:8,74`). The fifo is what
  separates them; the thread is not, both going through the same
  `SchedulableTask`, which starts it with `RtThread`
  (`SchedulableTask.cpp:45`).
- **Peripherals outside the audio context**: `Gpio`, `I2c`. `I2c` is an
  `ioctl` wrapper and so the application's to write by the rule above.
  `Gpio` is not: it `mmap`s the GPIO bank and reads and writes the
  registers directly (`Gpio.h:3,57,64,70`), reaching sysfs only to claim
  the pin — `Gpio::open` calls `gpio_export` and then maps the bank
  (`Gpio.cpp:99,108-109`), and nothing after that is a file. So the
  sysfs functions in `GPIOcontrol.h` are one path to a pin and this
  class is the other, and they are not interchangeable. `bela-sys`
  binds the first, and a safe wrapper over it is
  [#156](https://github.com/akiomik/bela-rs/issues/156); whether `bela`
  should also reach the register path — which means `Gpio` and `Mmap`
  rather than those functions — is a question nothing has asked yet.
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

Five of the 38 put another language's runtime on the board. `libpd`,
`BelaLibpd` and `pd-externals` run Pure Data patches; `csound` is
Csound's own API, shipped as a header; `BelaArduino` puts an
Arduino-shaped API — `Print`, `Stream`, `Wire` — over that same libpd
machinery (`BelaArduino.h:1-5`).
Each is a project of its own rather than a binding, and none of them
gets easier by being reached from Rust: a program that wants to run a Pd
patch is better served by Bela's own build for it.

### Code an application writes better in Rust

Under the rule, these are absent because a wrapper would buy nothing,
not because the work is queued. Twenty-three of the 38 libraries are
here, and the grouping is checkable rather than asserted: `grep -rl
'Bela\.h\|BelaContext'` over the twenty-three directories — sources as
well as headers — hits thirteen files in seven of them.

Five of the seven are the pin helpers, which take a `BelaContext*` —
three in a `Bela*` variant beside a plain one (`BelaDebounce`,
`BelaEncoder`, `BelaSteppedPot`) and two, `PulseIn` and
`ShiftRegister`, in the only class they have. The other two are
`OnePole`, which includes `Bela.h` and uses nothing from it at all,
and `WriteFile`, which uses one line — an `rt_fprintf` on the overrun
path (`WriteFile.cpp:287`). The second gratuitous include is `Encoder`'s
plain `Encoder.cpp`, in a directory the pin helpers already account
for; `WriteFile`'s is not gratuitous, being the one line above.

The remaining sixteen name neither `Bela.h` nor `BelaContext`, which is
all the grep tests. Four of them do reach sideways into Bela's
`libraries/` tree — `AudioFile` includes `sndfile`, `Convolver`
includes `AudioFile` and `ne10`, `Oscillator` includes `math_neon`,
`OscReceiver` includes `UdpServer` — but every one of those is another
library in this same group, or `ne10`, which is answered above. None of
it is the core API, and none of it changes the grouping.

| Libraries | Why not |
|---|---|
| `ADSR`, `Biquad` (`QuadBiquad`), `Convolver`, `DelayLine`, `EnvelopeDetector`, `OnePole`, `Oscillator`, `OscillatorBank`, `math_neon` | Ordinary DSP with no Bela hardware in it. Rust has these, or they are a few lines in the application, and either way they keep the borrow checker. `QuadBiquad` (`arm_neon.h`), `OscillatorBank` ("highly optimized, written in NEON assembly"), `Convolver` and `math_neon` are the NEON-tuned ones, and so the likeliest to be overturned by a measurement — `OscillatorBank` above all, having no Rust equivalent to lose to. |
| `Debounce` (`BelaDebounce`, `GpioDebounce`), `Encoder` (`BelaEncoder`), `PulseIn`, `ShiftRegister`, `SteppedPot` | Logic over accessors the crate already has, and no others: `digitalRead` and `pinMode` in all four of the digital ones, `digitalWriteOnce` in `ShiftRegister`, `analogRead` in `SteppedPot`, which touches no digital pin at all — and `digitalWrite` in none of them. Those, over frame and channel counts a `RenderContext` reports, are the whole of what they ask Bela for. What they need from Bela is what a `RenderContext` already is, and each is small enough that wrapping it would cost more than writing it. `GpioDebounce` is the one exception in the row: it debounces a `Gpio` rather than a context channel (`GpioDebounce.h:2`), so what it waits on is the register path — the `Gpio` bullet under [Not written yet](#not-written-yet) — which the sysfs functions `bela-sys` binds do not reach, and which no issue tracks. |
| `UdpClient`, `UdpServer`, `OscReceiver`, `Serial` | Protocols, not hardware. `std::net`, a serial crate and an OSC crate cover them; `Serial` in particular is a termios wrapper over `/dev/ttyS*` with nothing Bela-specific in it. |
| `WriteFile`, `Spi`, `Eeprom` | Linux interfaces with a thread or an `ioctl` in front of them: `WriteFile` is a ring drained by a `std::thread` it puts on `SCHED_FIFO`, `Spi` is `linux/spi/spidev.h`, `Eeprom` is the `24cXXX` driver's sysfs files. The one thing any of them takes from Bela is `WriteFile`'s overrun warning, an `rt_fprintf` inside `log()` (`WriteFile.cpp:277-292`) — which is to say on the audio thread, that being the method a render callback calls, and which is why it is `rt_fprintf` and not `fprintf`. A Rust program has `rt_println!` for exactly that, so this is a line the crate already covers rather than one it lacks. Rust reaches all three. What Bela's versions carry that a Rust program cannot write for itself is not code but board facts — which bus, which device node, what libbela has already claimed — and those belong in [board-facts.md](board-facts.md). |
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

Both lists below are set differences rather than hand-picked ones, so a
reader who suspects an entry has gone missing can recompute them.

### Bound, with no safe wrapper

Every `pub fn` in `bela-sys/src/bindings.rs` that no `bela_sys::` call
site in `bela/src` names — thirty-two of the fifty-six:

| Function | What it is for |
|---|---|
| `Bela_setPgaGain`, `Bela_setAdcLevel`, `Bela_setADCLevel`, `Bela_setDacLevel`, `Bela_setDACLevel`, `Bela_setHeadphoneLevel` | Six further spellings of the three calls the crate does wrap. `Bela_setPgaGain`, `Bela_setAdcLevel` and `Bela_setADCLevel` all end in the same `gAudioCodec->setInputGain` as `Bela_setAudioInputGain`, which is why one function has a 0.5 dB positive half and a 1.5 dB negative one — measured, and traced through libbela, in [board-facts.md](board-facts.md). `Bela_setDacLevel` is told by its own header to use `Bela_setLineOutLevel`, and `Bela_setDACLevel` and `Bela_setHeadphoneLevel` are the `channel = -1` case of `Bela_setDacLevel` and of `Bela_setHpLevel`. All but `Bela_setAdcLevel` are marked deprecated. None reaches anything the handle does not. |
| `Bela_runInSameThread` | Offered by the header as running the audio loop on the calling thread instead of a thread of libbela's own. On this image it is a stub: `RTAudio.cpp:1014-1018` prints "Turning the current thread into the audio thread is not supported with the POSIX skin." and calls `exit(1)`. Read rather than run — exercising it would end the process — so what is unwrapped here is a name, with nothing behind it to wrap. |
| `Bela_setUserData` | Replacing the pointer handed to the callbacks. The crate owns that pointer, so exposing it needs a story for what happens to the application it points at. |
| `Bela_setVerboseLevel` | Verbosity after `Bela_initAudio`; `Settings::verbose` only sets it before. |
| `Bela_printFlushBuffers` | Flushing the real-time print buffers that `rt_println!` fills — except that on this image it does nothing. Its whole body is a lone `#ifdef __COBALT__` (`RtWrappers.cpp:325-330`), and this board's real-time core is EVL rather than Xenomai 3 Cobalt ([board-facts.md](board-facts.md), where `ldd libbela.so` shows `libevl`). Its neighbours in the same file do have `BELA_EVL` branches, so this one is a gap in libbela rather than a choice. Read, not run. |
| `Bela_initRtBackend` | Bringing the real-time backend up separately from the audio system. |
| `Bela_gettime`, `Bela_clock_gettime`, `Bela_nanosleep` | Real-time safe time and sleep. `std::time` is not safe to call from a render callback, so these have no Rust equivalent on the audio thread. |
| `Bela_HwConfig_new`, `Bela_HwConfig_delete` | The hardware configuration object — which libbela's own comment says cannot be obtained: "this will always return error because of nullptr. Codec detection needs to be factored out of `Bela_initAudio`" (`RTAudio.cpp:114-125`). A wrapper would return `None` every time until that is fixed upstream. |
| `Bela_userSettings` | Not a call the program makes but a hook it may define: `Bela.h:742` marks it `#pragma weak`, and `Bela_defaultSettings` calls it if it is there. A Rust program can define it for itself; the crate neither does nor offers a way to, because where a program does hold the call, `Settings` and `validate_settings` cover the same ground with the types checked. |
| `Bela_deleteAllAuxiliaryTasks`, `rt_printf` | Left alone for reasons the crate already records. `task.rs:36` has the first: it frees every task at once and leaves the handles dangling, which is what `AuxiliaryTask`'s generation counter exists to survive. `print.rs:165` has the second: `rt_println!` calls `Bela_printf` instead, the header describing the `Bela_*` spellings as the future-proof wrappers. |
| `gpio_setup`, `gpio_export`, `gpio_unexport`, `gpio_set_dir`, `gpio_set_value`, `gpio_get_value`, `gpio_set_edge`, `gpio_fd_open`, `gpio_fd_close`, `gpio_write`, `gpio_read`, `gpio_dismiss`, `led_set_trigger` | The whole of `GPIOcontrol.h`: sysfs GPIO, one pin at a time, and the only path this crate offers to a pin outside a render callback, or to one that is not among the sixteen digital channels. libbela reaches a pin a second way, through the registers, which is the `Gpio` bullet under [Not written yet](#not-written-yet) and is not this. Several of the thirteen behave unexpectedly, all of it libbela's rather than the binding's. An export is not owned by whoever made it: `gpio_export` returns `0` for a pin it merely found already exported, indistinguishably from one it created (`core/GPIOcontrol.cpp:75-82`), and `gpio_unexport` and `gpio_dismiss` will remove one whoever made it — libbela's own pins included. `gpio_read` has no rewind, so a second read on one descriptor succeeds and reports the pin *high* whatever it is doing — only the third fails — and after a `gpio_write` even the first has nothing to read. `gpio_dismiss` returns `0` whatever happened. A failed `gpio_setup` can leave the pin exported and returns no descriptor, so undoing it means `gpio_unexport`, which takes none, rather than `gpio_dismiss`, which does. `led_set_trigger` numbers this board's LEDs from 1, where the path it builds is written for a `BeagleBone` numbering from 0. And built without `BELA_HAS_GPIO` every one of them is `{ return 0; }` instead (`:348-363`), which inverts all of it. [board-facts.md](board-facts.md) has the measurements, now including what an application asking for a pin libbela is holding gets: nothing refuses it, and taking one away is silent in both directions. So the safe wrapper, [#156](https://github.com/akiomik/bela-rs/issues/156), is a design question rather than a measurement one. |

### Not bound at all

Thirty-one *function declarations* in the three Bela headers reach no
binding — `NE10_dsp.h` and `NE10_types.h` are vendored too, and what
they declare is counted with `ne10` above rather than here. They fall
into three groups, and every member is named below. There were four:
the fourth was the `GPIOcontrol.h` family, which the allowlist alone
kept out, and it is now bound. Things that are not function
declarations follow them.

**Twenty-one are `static inline`.** bindgen skips those whatever the
allowlist says — `wrap_static_fns` is not enabled — and `libbela`
exports no symbol for any of them, so a hand-written `extern` would not
link either. Having them in Rust means writing them in Rust:

- The I/O accessors — `audioRead`, `audioWrite`, `analogRead`,
  `analogWrite`, `analogWriteOnce`, `digitalRead`, `digitalWrite`,
  `digitalWriteOnce`, `pinMode` and `pinModeOnce` — rewritten as the
  methods on `BlockContext` and `RenderContext`, which do the same
  indexing against the same `BelaContext` fields.
- Their non-interleaved counterparts — `audioReadNI`, `audioWriteNI`,
  `analogReadNI`, `analogWriteNI` and `analogWriteOnceNI`, the only
  five there are, the digital and `pinMode` helpers having none —
  **not** rewritten. The crate's accessors assume the interleaved
  layout (`context.rs:312`) and nothing checks that the assumption
  holds, which is
  [#158](https://github.com/akiomik/bela-rs/issues/158).
- `constrain`, `map`, `min` and `max` from `Utilities.h` — `constrain`
  and `map` are public in `bela`, and the other two are `std`.
- `multiplexerAnalogRead` and `multiplexerChannelForFrame`, the Capelet
  accessors. What is deliberate about those is the absence of the Rust
  rewrite, not the absence of a binding: like every accessor beside
  them, they could not have been bound. See above for why they are not
  written.

**Six are the `FILE*` and `va_list` printf variants** — `Bela_fprintf`,
`Bela_vfprintf`, `Bela_vprintf`, `rt_fprintf`, `rt_vfprintf` and
`rt_vprintf` — blocklisted in `xtask/src/generate.rs` with the reason
beside them: they would drag glibc internals into the bindings and are
not usable from Rust anyway.

**Four are none of those.** `setup`, `render` and `cleanup`
(`Bela.h:663`, `:679`, `:696`) are ordinary prototypes the allowlist
drops — but they are what a C Bela program *defines* rather than calls,
and this crate defines its own trampolines and hands them to
`Bela_initAudio` as `BelaInitSettings` fields, so nothing is missing.
`Bela_runAuxiliaryTask` is declared inside an `#ifdef __cplusplus`
(`Bela.h:1182`), for the sake of its default arguments, so bindgen —
which parses `wrapper.h` as C — never sees it. The symbol itself is
ordinary: `Bela.h`'s `extern "C"` block spans the declaration, and
`libbela` exports `Bela_runAuxiliaryTask` unmangled, so a hand-written
`extern` would link. It is not worth writing one: the function is
`Bela_createAuxiliaryTask` followed by `Bela_scheduleAuxiliaryTask`,
and `AuxiliaryTask` wraps both.

Beyond those thirty-one, **four names in `Bela.h` are function-like
macros rather than declarations**, which bindgen does not translate:
`Bela_setBit`, `Bela_clearBit`, `Bela_getBit` and `Bela_changeBit`
(`Bela.h:1223-1232`). They are bit twiddling on a `uint32_t` — `Bela.h`
implements `digitalRead` itself as `Bela_getBit(context->digital[frame],
channel + 16)` — so a Rust program writes the shift and mask where a C
one writes the macro, which is what `digital_read` does. `_ATTRIBUTE`
is the only other function-like macro in the three headers, and it is
the `printf`-attribute plumbing rather than API.

`GPIOcontrol.h`'s **object-like macros** are outside both
`allowlist_var` patterns and so are absent as well. A `grep` for
`#define` in it finds five; the fifth is the include guard,
`SIMPLEGPIO_H_`, which is not API. Of the other four,
`SYSFS_GPIO_DIR` and `SYSFS_LED_DIR` are the two directories the
family's own `snprintf` calls build paths in, `MAX_BUF` is the size of
the buffer they build them in, and `POLL_TIMEOUT` is defined in the
header and used nowhere on the board. None of them is an argument to
anything bound here. The two enums beside them, `PIN_DIRECTION` and
`PIN_VALUE`, are arguments, so those are bound.

## Settings and context fields not exposed

`Settings` writes 15 of the fields in `BelaInitSettings`, and the audio
system writes five more that are not an application's to choose: the
`setup`, `render_pre`, `render`, `render_post` and `cleanup` pointers,
which `Bela::new` sets to the trampolines that reach
[`BelaApplication`](../bela/src/application.rs) (`system.rs:387-391`).
The remaining 25 of the struct's 45 fields start at whatever
`Bela_defaultSettings()` — and therefore the board's
`~/.bela/belaconfig` — gives them, and they are all below.

Not exposed is not the same as unreachable, and the gap is wider than it
looks. Bela's own command line is a layer above `Settings` and wins over
it (`cmdline.rs:11-23`), so `Bela::run_with_args` and
`Bela::new_with_args` hand it straight through — and eleven of the
options libbela's own usage text lists, the text
[`print_usage`](../bela/src/cmdline.rs) prints, write into this list:
`--mux-channels`, `--pru-number`, `--pru-file`, `--board`,
`--codec-mode`, `--audio-expander-inputs`, `--audio-expander-outputs`,
`--disabled-digital-channels`, and the three that fill the gain arrays,
`--line-out-level`, `--hp-level` and `--audio-input-gain`
(`RTAudioCommandLine.cpp:306-319,505-521`). `--adc-level` and the two
`--pga-gain-*` are accepted and discarded with a deprecation warning
naming `--audio-input-gain` in their place, which is the command line
making the same substitution the gain arrays below made for the
deprecated scalars. It is not the same judgement as the function table
above, where `Bela_setAdcLevel` is the one spelling carrying no
deprecation note: what is legacy here is the option, not the call.

Those eleven are spellings rather than gates. `--json-file` and
`--json-string` reach the same fields by another road:
`jsonSettingsInit` turns the JSON back into an argv and calls
`Bela_getopt_long` with it (`RTAudio.cpp:1379-1459`), and a
`userArguments` key splices in an arbitrary option string. No new field
becomes reachable that way — it is the same parser — but a program
auditing for the eleven flags alone would miss it, and
`Bela_defaultSettings` runs the board's own `CL=` line from
`~/.bela/belaconfig` through that parser too, which
[`Bela::new`](../bela/src/system.rs) documents while looking at no
arguments at all.

Two of them the crate refuses before an audio system is built —
`--mux-channels` and `--pru-number`, in `check_resolved`
(`settings.rs:895-916`) — precisely because a program cannot have set
them itself. The other nine arrive unexamined, and four of those nine
have been run on a board: [board-facts.md](board-facts.md) records
`--board BelaMini` logged as requested and then ignored in favour of the
board libbela detected, `--codec-mode garbage` and
`--disabled-digital-channels 65535` doing nothing visible, and
`--pru-file /nonexistent` failing in `Bela_startAudio`.

That range — silently ignored, silently accepted, or fatal — is
libbela's doing rather than this crate's, and it is the other half of
why the two that are checked are checked. The same page measured what
the alternative costs: `--pru-number 5`, left to libbela, fails inside
`Bela_initAudio`, and a failure there takes every later audio system in
the process with it rather than only the attempt. What is missing for
the rest is a way for the program to state a value, not a way for one to
arrive. The escape hatch for that is `Settings::apply_to` on a
`BelaInitSettings` of the caller's own, with `Bela_initAudio` driven by
hand:

- `numAudioInChannels`, `numAudioOutChannels`
- `lineOutGains`, `headphoneGains`, `audioInputGains` and `adcGains`,
  the `BelaChannelGainArray` fields that carry the codec levels at
  start-up, together with the deprecated scalars they replaced —
  `dacLevel`, `adcLevel`, `headphoneLevel` and `pgaGain`. That the
  arrays are absent is argued rather than overlooked, in `level.rs`'s
  own documentation and measured in
  [board-facts.md](board-facts.md): `Bela_initAudio` applies three of the four
  arrays by calling the very functions `Bela::set_line_out_level` and
  its siblings wrap, and the fourth, `adcGains`, by calling
  `Bela_setAdcLevel`, which the table above lists as unwrapped and
  which ends in the same `setInputGain` anyway
  (`RTAudio.cpp:737-746`). The codec writes its registers only once
  audio starts, so a call between `Bela::new` and `Bela::start` reaches the
  hardware in the same state and at the same moment. There is nothing
  left for a setting to carry but a second way to say it.
- `interleave` — which the accessors nonetheless assume the value of,
  and nothing checks:
  [#158](https://github.com/akiomik/bela-rs/issues/158)
- `analogOutputsPersist`, `disabledDigitalChannels`
- `audioThreadStackSize`, `auxiliaryTaskStackSize`
- `ampMutePin` — which `Bela_defaultSettings` already reports as `-1`
  on a Gem, there being no amplifier mute pin on this board to name
  ([board-facts.md](board-facts.md)), so what an application would set
  it to is a question for hardware that is out of scope anyway
- `codecMode`, `board`, `projectName`
- `pruNumber`, `pruFilename`
- `audioExpanderInputs`, `audioExpanderOutputs`
- `audioThreadDone`, the callback that runs when the audio thread ends
- `numMuxChannels` — the Capelet, deliberately. `--mux-channels` still
  reaches it, and `check_resolved` still rejects what it can before the
  audio system is built, which is the case above in its clearest form:
  what the crate declines to offer is a way of asking for a Capelet,
  not a defence against one being asked for.

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
