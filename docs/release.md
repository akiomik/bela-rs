# Release procedure

## Versioning policy: lockstep

`bela-sys` and `bela` always share one version, defined once as
`workspace.package.version` in the root `Cargo.toml`. **This includes
releases where only one crate changed**: fixing a bug in `bela` alone
still bumps and republishes both crates at the new version.

Why lockstep instead of per-crate versions:

- `bela X.Y.Z` always depends on `bela-sys X.Y.Z`, so any released
  pair is known-coherent — there is no matrix of combinations to
  reason about or test.
- One `vX.Y.Z` tag identifies one workspace state; the release
  workflow stays a single tag-triggered job with no tag-name parsing.
- Republishing an unchanged crate under a new version is cheap; the
  release workflow handles ordering and skipping automatically.

If the crates ever evolve at very different rates (e.g. `bela-sys`
frozen while `bela` iterates), revisit this and switch to per-crate
tags (`bela-vX.Y.Z` / `bela-sys-vX.Y.Z`) — a deliberate change to
this document and the release workflow, not an ad-hoc one.

Version meanings while pre-1.0:

- `0.x.y` — the current phase, from `0.1.0` (the first
  hardware-validated release) onwards. A release that is not a drop-in
  replacement for the one before it bumps the minor, one that is bumps
  the patch, which is what caret semantics already mean on `0.x`. The
  API is not settled, so minor bumps are expected rather than
  exceptional.
- `0.0.x` — the pre-hardware releases, a phase now over. Under caret
  semantics every `0.0.x` was incompatible with every other, which was
  accurate then: nothing had run on a board.

## Minor or patch: the drop-in test

The question a version number answers is a counterfactual, because a
caret requirement on `0.x` does not cross into the next minor: were
this release to go out as a patch, `cargo update` would hand it to
somebody who has `bela = "0.x"` in their `Cargo.toml` and reads
nothing. If that would leave them worse off than the version they
already had, it goes out as a minor instead.

The Rust API is only part of what can leave them worse off. This crate
wraps a C library, a board image and a cross toolchain, and a release
can break a build without a single signature changing. What counts:

- **The API.** An item removed, renamed or given a different
  signature; a trait gaining a required method or associated type; a
  variant added to an enum that is not `#[non_exhaustive]`. Additions
  usually are not this — a new type, a new variant on `Error`, which
  is `#[non_exhaustive]` so matching on it already has a wildcard arm,
  a method made `const` — but additive is not a synonym for safe. A
  new `impl` of a standard trait can leave an existing `.into()`
  without a type to infer, and a new inherent method can shadow the
  one an extension trait was providing; the Cargo book files both
  under [possibly-breaking](https://doc.rust-lang.org/cargo/reference/semver.html)
  rather than never-breaking. What decides it is whether a plausible
  caller stops compiling, not whether anything was taken away.

  That plausible caller is not in this repository, so searching this
  one cannot answer the question — it can only ever find nothing, which
  reads like a check that passed. Ask instead whether a downstream
  crate *could* already have written the item being added: a gap named
  in an open issue, or one whose workaround this crate documents
  (`ResolvedSettings::as_sys`, `Settings::apply_to`, the raw `bela-sys`
  fields), is a gap somebody had a reason to fill. `0.6.0`'s
  `Settings::audio_sample_rate` and `0.7.0`'s `Settings::enable_led`
  are both that shape.
- **What a device build links or needs.** A library added to the link
  line, a compiler the toolchain did not have to include, an
  environment variable a build now depends on. `0.4.0` is the worked
  example, and the reason this section exists: nothing in its API was
  removed or changed shape, and it is a minor release because
  `bela_midi_*` is a C++ shim over Bela's `Midi` class, which put
  `libbelaextra` on the link line and a C++ cross compiler among the
  things a device build needs.
- **The minimum Rust version.** Raising `rust-version` bumps the
  minor. On an older toolchain the update either fails to build or,
  with an MSRV-aware resolver, is quietly never offered — a patch
  should do neither.
- **What the board has to be.** Vendored headers moved to a newer
  Bela, or anything else that stops a board image the last release
  worked on from working. The headers are pinned and
  `cargo xtask check-vendor --board` says which image they match.
- **Behaviour.** The same call doing something different — a
  configuration that used to be accepted now refused, a default
  changed, a callback arriving somewhere else.

What does not count, however large it looks in the changelog:
documentation corrected to say what the code always did. `0.4.0`
carries several — a Gem Stereo turning out to have no analog outputs
at all, an analog full scale of 4.096 V rather than an unnamed one —
and each of them is a patch-level change wearing a lot of prose: the
value a program reads is the same value it read before, and only what
this crate claims about it changed. A program built on the wrong claim
was already wrong, and no version number can put that right.

## Cutting a release

1. **Bump the version** — minor or patch by the drop-in test above —
   in two places (they must match):
   - `workspace.package.version` in the root `Cargo.toml`
   - the `bela-sys` dependency `version` in `bela/Cargo.toml`
     (needed on every minor bump while pre-1.0: a caret requirement
     on `0.x` does not cross into the next minor, so leaving it
     behind would publish a `bela` that asks for the previous
     `bela-sys`. Only a patch bump can leave it alone.)
2. **Cut the changelog**: move the `[Unreleased]` content into a new
   `[X.Y.Z] - YYYY-MM-DD` section and update the comparison links at
   the bottom.
3. **Verify locally** (same as CI):

   ```sh
   cargo fmt --all --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   cargo clippy --workspace --all-targets --target aarch64-unknown-linux-gnu -- -D warnings
   ```

4. **Verify on hardware**, which CI cannot do:

   ```sh
   BELA_SYSROOT="$PWD/bela-sysroot" scripts/smoke-test.sh
   cargo xtask check-vendor --board
   ```

   The smoke test builds the examples, runs each of them on the board
   and checks that audio actually ran at the right rate and shut down
   cleanly. `check-vendor` confirms that the headers the published
   bindings were generated from are still the ones the board ships.

5. **Open a release pull request** (`chore(release): prepare vX.Y.Z`)
   and merge it once CI is green. `main` takes changes through a pull
   request and requires linear history, so the release commit gets
   there the same way as any other; the hardware checks of step 4 go
   in the description, since CI cannot repeat them.
6. **Tag and push the tag** — this is the publish trigger:

   ```sh
   git tag -a vX.Y.Z -m "vX.Y.Z"
   git push origin vX.Y.Z
   ```

   The [release workflow](../.github/workflows/release.yml) then:
   - checks that the tag matches the workspace version,
   - authenticates via crates.io Trusted Publishing (OIDC, no stored
     token),
   - publishes `bela-sys` first, then `bela`, skipping any version
     that is already on crates.io and retrying through the index
     propagation delay.
7. **Create the GitHub release** from the changelog section:

   ```sh
   gh release create vX.Y.Z --title "vX.Y.Z" --notes "..."
   ```

## Failure modes

- **Workflow fails partway** (e.g. `bela-sys` published, `bela` not):
  fix the cause and re-run the workflow. The already-on-crates.io
  check makes re-runs idempotent — published versions are skipped.
- **Wrong tag**: the version-match guard fails the run before
  anything is published. Delete the tag, fix, re-tag.
- **A published version is broken**: versions on crates.io are
  immutable — publish a fixed `x.y.z+1` and `cargo yank` the broken
  one (`cargo yank --version X.Y.Z bela`). Never delete tags of
  versions that reached crates.io.

## One-time setup (already done)

- The first publish of a new crate must be manual (crates.io does not
  support Trusted Publishing for not-yet-existing crates); 0.0.1 was
  published this way.
- Trusted Publishing is registered on crates.io for both crates:
  repository `akiomik/bela-rs`, workflow `release.yml`, no
  environment.
