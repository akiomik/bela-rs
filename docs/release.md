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

- `0.0.x` — pre-hardware development releases. Note that under caret
  semantics every `0.0.x` is incompatible with every other, which is
  accurate for this phase.
- `0.1.0` — reserved for the first hardware-validated release
  ([milestone v0.1.0](https://github.com/akiomik/bela-rs/milestone/1)).

## Cutting a release

1. **Bump the version** in two places (they must match):
   - `workspace.package.version` in the root `Cargo.toml`
   - the `bela-sys` dependency `version` in `bela/Cargo.toml`
     (mandatory while on `0.0.x`, where caret requirements do not
     cross versions; from `0.x` onwards only needed when the
     requirement no longer matches)
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

4. **Commit and push** (`chore(release): prepare vX.Y.Z`), then wait
   for CI to go green on `main`.
5. **Tag and push the tag** — this is the publish trigger:

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
6. **Create the GitHub release** from the changelog section:

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
