<!--
What the change is and why, in prose and in English
([CONTRIBUTING.md](https://github.com/akiomik/bela-rs/blob/main/CONTRIBUTING.md)).

The two sections below are questions rather than rules: the rules live
where they already lived, and each question links to them. They are here
because they are the checks that leave no trace when they are skipped —
an answer nobody wrote reads exactly like an answer nobody needed.
-->

## Semver

Does this touch any of the things the drop-in test weighs? The public
API is the usual case — a method, a trait implementation, an enum
variant, a signature — but not the only one: 0.4.0 broke nothing in its
API and still went out as a minor.

- [ ] No.
- [ ] Yes, and it is not breaking, because:
- [ ] Yes, and it is breaking. Its changelog entry opens with
      `Breaking:`, and this pull request carries the `breaking` label.

Where the public API is what changed, "not breaking" is a claim about
code that is not in this repository. What decides it, there and for
everything else the question reaches, is the drop-in test in
[docs/release.md](https://github.com/akiomik/bela-rs/blob/main/docs/release.md).

## Hardware

- [ ] Nothing here touches the device path.
- [ ] It ran on a board, and the verdict is in the description above —
      `scripts/smoke-test.sh`, or what was run in its place where the
      change is outside what that covers.
- [ ] It touches the device path and has not been run on a board. That
      is a thing to say out loud rather than leave to the reader, and
      this pull request carries the `hardware` label so that the board
      it still needs can be found later.
