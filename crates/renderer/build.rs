//! Compresses the renderer's embedded JS shims at compile time.
//!
//! The Web API shims ship inside the binary (~323 KB raw, ~72 KB deflated),
//! so they are stored deflated under `OUT_DIR/js_blobs` and inflated once per
//! process (see `js::blob`). The web bundle order lives in
//! `src/js/scripts/web/order.txt`: that file is the single source of truth,
//! read here and by the round-trip test, so adding a shim cannot silently
//! diverge from what realms evaluate.

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// (blob file name, shim path relative to the crate root).
const SINGLES: &[(&str, &str)] = &[
    ("intl.deflate", "src/js/scripts/intl.js"),
    ("brands.deflate", "src/js/scripts/brands.js"),
    ("collections.deflate", "src/js/scripts/collections.js"),
    (
        "dom_parser_ctor.deflate",
        "src/js/scripts/parsing/dom_parser_ctor.js",
    ),
    (
        "event_target_ctor.deflate",
        "src/js/scripts/events/event_target_ctor.js",
    ),
    ("abort.deflate", "src/js/scripts/events/abort.js"),
    (
        "event_ctor.deflate",
        "src/js/scripts/events/event_ctor.js",
    ),
    (
        "custom_event.deflate",
        "src/js/scripts/events/custom_event.js",
    ),
];

/// Wrapper around the concatenated web bundle, matching the previous
/// `concat!("(function(){", ..., "})();")` shape in `js::JsRealm::install`.
const WEB_PREFIX: &str = "(function(){";
const WEB_SUFFIX: &str = "})();";

fn main() {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set"));
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set")).join("js_blobs");
    fs::create_dir_all(&out).expect("create js_blobs dir");

    let mut manifest = String::new();

    let order_path = root.join("src/js/scripts/web/order.txt");
    println!("cargo:rerun-if-changed={}", order_path.display());
    let mut bundle = WEB_PREFIX.to_owned();
    let mut bundle_inputs = Vec::new();
    let order = fs::read_to_string(&order_path).expect("read web order.txt");
    for file in order.lines() {
        let file = file.trim();
        if file.is_empty() || file.starts_with('#') {
            continue;
        }
        let rel = format!("src/js/scripts/web/{file}");
        println!("cargo:rerun-if-changed={}", root.join(&rel).display());
        bundle.push_str(&fs::read_to_string(root.join(&rel)).expect("read web shim"));
        bundle_inputs.push(rel);
    }
    bundle.push_str(WEB_SUFFIX);
    write_blob(&out, "web_bundle.deflate", bundle.as_bytes());
    writeln!(
        manifest,
        "web_bundle.deflate\tbundle\t{WEB_PREFIX}|{WEB_SUFFIX}\t{}",
        bundle_inputs.join(",")
    )
    .expect("write manifest line");

    for (blob, input) in SINGLES {
        println!("cargo:rerun-if-changed={}", root.join(input).display());
        let bytes = fs::read(root.join(input)).expect("read shim");
        write_blob(&out, blob, &bytes);
        writeln!(manifest, "{blob}\tsingle\t{input}").expect("write manifest line");
    }
    fs::write(out.join("manifest.txt"), manifest).expect("write manifest");
}

/// Deflate-compresses `bytes` at maximum level; output is deterministic for
/// one `flate2` version, which `Cargo.lock` pins.
fn write_blob(out: &Path, name: &str, bytes: &[u8]) {
    let mut encoder =
        flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::best());
    encoder.write_all(bytes).expect("deflate shim");
    let compressed = encoder.finish().expect("finish deflate");
    fs::write(out.join(name), compressed).expect("write blob");
}
