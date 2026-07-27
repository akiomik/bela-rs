# Bela Gem board facts

Values measured on the actual board during the fact-finding phase (the
first task once the board arrives; see issue #4). Record **only values
confirmed on the device** — no guesses.

> **Status: not started (board has not arrived).**

## System

- [ ] `uname -a` output (architecture, kernel version):
- [ ] Xenomai version and generation (including how Cobalt/Dovetail was
      verified):
- [ ] Debian version:
- [ ] Bela software version / branch / commit on the board
      (git info of `/root/Bela`). **The bindgen header pin is moved to
      this exact version**:

## Build and link information

Collect from a verbose build of a C++ example (`make VERBOSE=1` or
similar).

- [ ] Compile-time `-I` include paths:
- [ ] Link-time `-l` flags:
- [ ] Library search paths (`-L`) and the actual location of
      `libbela*.so`:
- [ ] Xenomai-related link flags (`libcobalt` etc.) and any wrapper
      scripts involved:
- [ ] Signature of the per-core render() extension API (check the
      headers):

## Operations

- [ ] How to stop/disable the Bela IDE default program:
- [ ] ssh access (hostname, authentication):
- [ ] List of paths that must be synced as the sysroot:
