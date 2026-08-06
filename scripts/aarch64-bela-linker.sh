#!/bin/sh
# Linker wrapper for device builds, used via .cargo/config.toml.
#
# Three adjustments are needed to link against a copy of the board's
# filesystem:
#
# - --sysroot, because the Debian linker scripts (e.g. libc.so) refer to
#   absolute paths that must resolve inside the sysroot.
# - -B on the multiarch directory, because Debian keeps the startup
#   files (Scrt1.o, crti.o, ...) in /usr/lib/aarch64-linux-gnu, which a
#   toolchain built for a different triple — aarch64-unknown-linux-gnu —
#   does not search on its own. For one whose triple already matches it
#   is redundant, and harmless.
# - -rpath-link, so the linker can resolve the dependencies *of the
#   shared libraries* it links (libbela needs libevl and libstdc++,
#   which in turn need libbpf, libm and friends). These paths affect
#   link-time lookup only; at runtime the board uses its own.
#
# BELA_SYSROOT is the same variable bela-sys/build.rs uses for the
# library search paths; when unset (e.g. a native build on the board)
# the wrapper adds nothing.
#
# BELA_CC names the compiler to call, and defaults to
# aarch64-unknown-linux-gnu-gcc, the one from the macOS tap this project
# documents. A cross toolchain is named after the triple it was built
# for, so the name differs by where it came from — Debian and Ubuntu
# ship theirs as aarch64-linux-gnu-gcc. The wrapper is attached to the
# target rather than to cross-compiling, so a build that is not cross
# comes through here too and wants plain gcc. See docs/cross-compile.md.
set -eu

CC="${BELA_CC:-aarch64-unknown-linux-gnu-gcc}"

if [ -z "${BELA_SYSROOT:-}" ]; then
  exec "$CC" "$@"
fi

RPATH_LINK="$BELA_SYSROOT/root/Bela/lib"
for dir in /usr/evl/lib/aarch64-linux-gnu /usr/local/lib /usr/lib/aarch64-linux-gnu; do
  RPATH_LINK="$RPATH_LINK:$BELA_SYSROOT$dir"
done

exec "$CC" \
  --sysroot="$BELA_SYSROOT" \
  -B"$BELA_SYSROOT/usr/lib/aarch64-linux-gnu" \
  -Wl,-rpath-link="$RPATH_LINK" \
  "$@"
