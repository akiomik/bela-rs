# Contributing

## Language

Repository deliverables — documentation, code comments, commit
messages — are written in English. Local working notes that are not
meant to be published belong outside the repository (or in paths
listed in `.gitignore`).

## Commit messages

This project uses
[Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/):

```
<type>(<optional scope>): <description>
```

Types used here: `feat`, `fix`, `docs`, `test`, `refactor`, `build`,
`ci`, `chore`. Use scopes matching the crate or area when helpful,
e.g. `feat(bela-sys): ...`, `docs(cross-compile): ...`.

## Changelog

Notable changes are recorded in [CHANGELOG.md](CHANGELOG.md) following
the [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) format.
Add entries to the `[Unreleased]` section in the same commit as the
change itself.

Notable means notable to someone using the published crates: what
`bela` and `bela-sys` offer, how they behave, the examples that ship
with them, and the documentation written for people building against
them. The project's own machinery does not qualify — CI, `xtask`,
`scripts/`, and the notes under `docs/` that record measurements and
procedures — however welcome a change to it is; the commit and the
pull request are where those are recorded.

This applies to the released sections too. When the rule changes, the
whole file is brought to it rather than left as a record of what each
past release happened to think worth mentioning: a changelog read for
what a version did is only useful if every version was written to the
same standard.

Keep a Changelog has no category for breaking changes, so mark one by
opening its entry with `Breaking:`. Which heading it goes under is a
separate question: `Removed` when a feature is gone, `Changed` when
something that still exists behaves incompatibly.

## Releases

See [docs/release.md](docs/release.md). The two crates are versioned
in lockstep and published from GitHub Actions by pushing a version
tag.

## Toolchain and MSRV

The development toolchain is pinned in `rust-toolchain.toml` so that a
moving `stable` cannot break builds; upgrading it is a deliberate edit
to that file. The MSRV is declared as `rust-version` in the workspace
`Cargo.toml` and verified by a dedicated CI job — raise it consciously
and mention the change in the changelog.

## Checks

Before pushing, make sure the same checks as CI pass:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo check --workspace --exclude xtask --target aarch64-unknown-linux-gnu
```

`xtask` is a workspace member, so the host checks cover it; the device
target excludes it, because it is a task that runs on the development
machine and never on a board.

CI also measures test coverage with
[`cargo llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov) and
uploads it to Codecov. To see the same numbers locally:

```sh
cargo llvm-cov --workspace --exclude xtask --summary-only
```

It covers the host build only — the device-only code behind the
`bela_device` cfg is not compiled there, so it is absent from the
report rather than counted as untested.

CI stops there, because it has no board. Changes that touch the device
path — anything under `bela::system`, the settings applied to
`Bela_initAudio`, the examples, the linking setup — should also pass
the hardware smoke test:

```sh
BELA_SYSROOT="$PWD/bela-sysroot" scripts/smoke-test.sh [user@host] [seconds]
```

It builds the examples, runs each of them on the board, and checks that
audio ran at the sample rate it reported and shut down cleanly on
SIGINT. `bela_daemon` is stopped for the duration and restarted
afterwards.

After updating a board image, also check that the vendored headers
still match what the board now ships:

```sh
cargo xtask check-vendor --board [user@host]
```

It diffs `bela-sys/vendor/bela` against the board and exits non-zero on
drift, which is the signal to re-pin (see
[docs/cross-compile.md](docs/cross-compile.md)). The committed bindings
describe the vendored headers, so drift means they no longer describe
the `libbela` they link against.

## License

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the
Apache-2.0 license, shall be dual licensed under MIT OR Apache-2.0,
without any additional terms or conditions.
