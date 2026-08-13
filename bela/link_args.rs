// Reads the device link arguments `bela-sys` publishes as `links`
// metadata (see `bela-sys/link_args.rs`), and republishes them under
// this crate's own `links` name — `bela_relay` — since `links`
// metadata reaches only an immediate dependent, and an application
// depending on `bela` is not one of bela-sys's.
//
// The encoding function is duplicated rather than shared: a build
// script is packaged with its own crate directory only, so nothing
// outside `bela/` can be `include!`d here once `bela-sys` and `bela`
// are two separately published crates.
//
// Included by build.rs, where both directions run, and by src/lib.rs
// under cfg(test) — the same split shim_compiler.rs uses in bela-sys.
const LINK_ARGS_METADATA_KEY: &str = "LINK_ARGS";

/// The link arguments published under `{prefix}_LINK_ARGS_*`, in
/// order — empty if the count is absent (nothing was published, e.g.
/// a host build) or if any indexed value the count promised is
/// missing (an inconsistency between publisher and reader, which
/// should link-fail loudly rather than apply a silently incomplete
/// subset of the arguments).
fn decode_link_args(get_var: impl Fn(&str) -> Option<String>, prefix: &str) -> Vec<String> {
    let count: usize = get_var(&format!("{prefix}_{LINK_ARGS_METADATA_KEY}_COUNT"))
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let mut args = Vec::with_capacity(count);
    for index in 0..count {
        match get_var(&format!("{prefix}_{LINK_ARGS_METADATA_KEY}_{index}")) {
            Some(value) => args.push(value),
            None => return Vec::new(),
        }
    }
    args
}

/// The `cargo::metadata=KEY=VALUE` pairs that publish `args` under
/// [`LINK_ARGS_METADATA_KEY`]: a count, then the arguments themselves
/// in order. See `bela-sys/link_args.rs`, which this mirrors.
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
