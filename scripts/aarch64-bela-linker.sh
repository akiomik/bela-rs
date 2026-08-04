#!/bin/sh
# Linker wrapper for device builds, used via .cargo/config.toml.
#
# Three adjustments are needed to link against a copy of the board's
# filesystem:
#
# - --sysroot, because the Debian linker scripts (e.g. libc.so) refer to
#   absolute paths that must resolve inside the sysroot.
# - -B on the multiarch directory, because Debian keeps the startup
#   files (Scrt1.o, crti.o, ...) in /usr/lib/aarch64-linux-gnu while the
#   cross toolchain is built for the aarch64-unknown-linux-gnu triple
#   and does not search that path on its own.
# - -rpath-link, so the linker can resolve the dependencies *of the
#   shared libraries* it links (libbela needs libevl and libstdc++,
#   which in turn need libbpf, libm and friends). These paths affect
#   link-time lookup only; at runtime the board uses its own.
#
# BELA_SYSROOT is the same variable bela-sys/build.rs uses for the
# library search paths; when unset (e.g. a native build on the board)
# the wrapper adds nothing.
set -eu

if [ -z "${BELA_SYSROOT:-}" ]; then
  exec aarch64-unknown-linux-gnu-gcc "$@"
fi

RPATH_LINK="$BELA_SYSROOT/root/Bela/lib"
for dir in /usr/evl/lib/aarch64-linux-gnu /usr/local/lib /usr/lib/aarch64-linux-gnu; do
  RPATH_LINK="$RPATH_LINK:$BELA_SYSROOT$dir"
done

exec aarch64-unknown-linux-gnu-gcc \
  --sysroot="$BELA_SYSROOT" \
  -B"$BELA_SYSROOT/usr/lib/aarch64-linux-gnu" \
  -Wl,-rpath-link="$RPATH_LINK" \
  "$@"
