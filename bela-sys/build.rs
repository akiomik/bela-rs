//! Placeholder build script.
//
// Planned responsibilities (see the v0.1.0 milestone issues):
// - run bindgen on wrapper.h (Bela.h and required core headers), with the
//   header location overridable via an environment variable so it can be
//   re-pinned to the Bela version shipped on the board
// - emit the `-l` / search-path link flags collected in docs/board-facts.md
//   (requires the board; until then only `cargo check` is supported for
//   the aarch64 target)
fn main() {
    println!("cargo::rerun-if-changed=build.rs");
}
