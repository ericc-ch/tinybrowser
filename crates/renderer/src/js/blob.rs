//! Inflate-once access to the build-compressed JS shims.
//!
//! `build.rs` stores every install script deflated (`OUT_DIR/js_blobs`);
//! each blob inflates at most once per process into a `&'static str` that the
//! realm installers evaluate. A corrupt blob is a build bug, so inflation
//! failure throws instead of aborting: every call site already returns a
//! `rquickjs::Result`.

use std::io::Read as _;
use std::sync::OnceLock;

use rquickjs::{Ctx, Exception, Result};

/// Inflates `deflated` on first call through `cache`, returning the cached
/// source afterwards. A second caller racing the first blocks in
/// `get_or_init` and receives the winner's value; inflation is idempotent.
pub(crate) fn decompress(
    ctx: &Ctx<'_>,
    deflated: &'static [u8],
    cache: &'static OnceLock<Box<str>>,
) -> Result<&'static str> {
    if let Some(cached) = cache.get() {
        return Ok(cached);
    }
    let mut out = String::new();
    flate2::read::DeflateDecoder::new(deflated)
        .read_to_string(&mut out)
        .map_err(|err| Exception::throw_internal(ctx, &err.to_string()))?;
    Ok(cache.get_or_init(|| out.into_boxed_str()))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Read as _;
    use std::path::PathBuf;

    /// Every blob `build.rs` emitted inflates to the sources it was built
    /// from. `manifest.txt` is the machine-readable record of that mapping,
    /// so adding a shim without compressing it (or vice versa) fails here
    /// instead of shipping a realm that evaluates stale script.
    #[test]
    fn blobs_round_trip() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let dir = PathBuf::from(env!("OUT_DIR")).join("js_blobs");
        let manifest =
            fs::read_to_string(dir.join("manifest.txt")).expect("read blob manifest");
        assert!(!manifest.trim().is_empty(), "blob manifest is empty");
        for line in manifest.lines() {
            let (blob, rest) = line.split_once('\t').expect("manifest blob field");
            let compressed = fs::read(dir.join(blob)).expect("read blob");
            let mut out = String::new();
            flate2::read::DeflateDecoder::new(&compressed[..])
                .read_to_string(&mut out)
                .expect("inflate blob");
            let (mode, payload) = rest.split_once('\t').expect("manifest mode field");
            match mode {
                "single" => {
                    let source =
                        fs::read_to_string(root.join(payload)).expect("read shim source");
                    assert_eq!(out, source, "{blob} drifted from {payload}");
                }
                "bundle" => {
                    let (wrap, paths) = payload.split_once('\t').expect("bundle manifest");
                    let (prefix, suffix) = wrap.split_once('|').expect("bundle wrapper");
                    assert!(
                        out.starts_with(prefix) && out.ends_with(suffix),
                        "{blob} lost its wrapper"
                    );
                    let body = &out[prefix.len()..out.len() - suffix.len()];
                    for path in paths.split(',') {
                        let source =
                            fs::read_to_string(root.join(path)).expect("read shim source");
                        assert!(
                            body.contains(&source),
                            "{blob} drifted from {path}"
                        );
                    }
                }
                _ => panic!("unknown manifest mode {mode}"),
            }
        }
    }
}
