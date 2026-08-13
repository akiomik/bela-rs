// Encodes device link arguments as `links` metadata: a count key plus
// one key per argument, indexed from zero, rather than one joined
// string. A joined value needs a parser on the reading side to survive
// a BELA_SYSROOT containing whitespace; a count and separate keys need
// none. See docs/cross-compile.md for the two crates that read this.
//
// Included by build.rs, where it runs, and by src/lib.rs under
// cfg(test) — the same split shim_compiler.rs uses, because a build
// script is not a target `cargo test` builds.
const LINK_ARGS_METADATA_KEY: &str = "LINK_ARGS";

/// The `cargo::metadata=KEY=VALUE` pairs that publish `args` under
/// [`LINK_ARGS_METADATA_KEY`]: a count, then the arguments themselves
/// in order.
fn encode_link_args(args: &[String]) -> Vec<(String, String)> {
    let mut pairs = Vec::with_capacity(args.len() + 1);
    pairs.push((
        format!("{LINK_ARGS_METADATA_KEY}_COUNT"),
        args.len().to_string(),
    ));
    for (index, arg) in args.iter().enumerate() {
        pairs.push((format!("{LINK_ARGS_METADATA_KEY}_{index}"), arg.clone()));
    }
    pairs
}
