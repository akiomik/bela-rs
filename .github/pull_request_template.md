<!--
What the change is and why, in prose and in English (CONTRIBUTING.md).

The two sections below are questions rather than rules: the rules live
where they already lived, and each question links to them. They are here
because they are the checks that leave no trace when they are skipped —
an answer nobody wrote reads exactly like an answer nobody needed.
-->

## Semver

Does this add, remove or change anything in the public API of `bela` or
`bela-sys` — a method, a trait implementation, an enum variant, a
signature?

- [ ] No.
- [ ] Yes, and it is not breaking, because:
- [ ] Yes, and it is breaking. Its changelog entry opens with
      `Breaking:` under `### Changed`, and this pull request carries the
      `breaking` label.

"Not breaking" is a claim about code that is not in this repository:
whether a downstream crate could already have an extension trait, a
wrapper or an `impl` of that name filling the gap this closes. Searching
this repository cannot answer that — it can only ever find nothing. What
decides it is the drop-in test in
[docs/release.md](https://github.com/akiomik/bela-rs/blob/main/docs/release.md).

## Hardware

- [ ] Nothing here touches the device path.
- [ ] `scripts/smoke-test.sh` passed on a board, and its verdict is in
      the description above.
- [ ] It touches the device path and has not been run on a board, which
      is a thing to say out loud rather than to leave to the reader.
