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
  (`MAX_MONITORED_PERIOD_SIZE`).

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
