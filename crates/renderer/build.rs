//! Compresses the renderer's embedded JS shims at compile time.
//!
//! The Web API shims ship inside the binary (~323 KB raw, ~72 KB deflated),
//! so they are stored deflated under `OUT_DIR/js_blobs` and inflated once per
//! process (see `js::blob`). The web bundle order lives in
//! `src/js/scripts/web/order.txt`: that file is the single source of truth,
//! read here and by the round-trip test, so adding a shim cannot silently
//! diverge from what realms evaluate.

use std::collections::HashSet;
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
    ("event_ctor.deflate", "src/js/scripts/events/event_ctor.js"),
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
    generate_bindings();
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set"));
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set")).join("js_blobs");
    fs::create_dir_all(&out).expect("create js_blobs dir");

    let mut manifest = String::new();

    let order_path = root.join("src/js/scripts/web/order.txt");
    println!("cargo:rerun-if-changed={}", order_path.display());
    let mut bundle = WEB_PREFIX.to_owned();
    let mut bundle_inputs = Vec::new();
    let mut seen = HashSet::new();
    let order = fs::read_to_string(&order_path).expect("read web order.txt");
    for file in order.lines() {
        let file = file.trim();
        if file.is_empty() || file.starts_with('#') {
            continue;
        }
        assert!(
            file.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')),
            "order.txt entry {file:?} must be a plain file name"
        );
        assert!(
            seen.insert(file.to_owned()),
            "duplicate order.txt entry {file}"
        );
        let rel = format!("src/js/scripts/web/{file}");
        println!("cargo:rerun-if-changed={}", root.join(&rel).display());
        bundle.push_str(
            &fs::read_to_string(root.join(&rel))
                .unwrap_or_else(|err| panic!("read web shim {rel}: {err}")),
        );
        bundle_inputs.push(rel);
    }
    assert!(
        !bundle_inputs.is_empty(),
        "order.txt lists no shims; refusing to ship an empty web bundle"
    );
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
        let text = fs::read_to_string(root.join(input))
            .unwrap_or_else(|err| panic!("read shim {input}: {err}"));
        write_blob(&out, blob, text.as_bytes());
        writeln!(manifest, "{blob}\tsingle\t{input}").expect("write manifest line");
    }
    fs::write(out.join("manifest.txt"), manifest).expect("write manifest");
}

/// Deflate-compresses `bytes` at maximum level; output is deterministic for
/// one `flate2` version, which `Cargo.lock` pins.
fn write_blob(out: &Path, name: &str, bytes: &[u8]) {
    let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::best());
    encoder.write_all(bytes).expect("deflate shim");
    let compressed = encoder.finish().expect("finish deflate");
    fs::write(out.join(name), compressed).expect("write blob");
}

/// Compiles only the declared native surface. Unsupported semantics fail the
/// build instead of producing bindings that merely look like Web IDL.
fn generate_bindings() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let mut extracts = Vec::new();
    binding_sources(&root.join("../webidl-bindgen/idl"), "idl", &mut extracts);
    let mut implementations = Vec::new();
    binding_sources(&root.join("src/js"), "rs", &mut implementations);
    let extracts: Vec<_> = extracts
        .iter()
        .map(|(name, text)| webidl_bindgen::Source { name, text })
        .collect();
    let implementations: Vec<_> = implementations
        .iter()
        .map(|(name, text)| webidl_bindgen::Source { name, text })
        .collect();
    let bindings = webidl_bindgen::compile_contracts(&extracts, &implementations)
        .unwrap_or_else(|error| panic!("upstream IDL contracts: {error}"));
    let mut contract_interfaces = HashSet::new();
    for binding in bindings {
        contract_interfaces.insert(binding.interface.clone());
        fs::write(
            out.join(&binding.interface).with_extension("rs"),
            binding.rust,
        )
        .expect("write generated contract");
    }
    let idl = root.join("idl");
    println!("cargo:rerun-if-changed={}", idl.display());
    let mut inputs: Vec<_> = fs::read_dir(&idl)
        .expect("read native IDL directory")
        .map(|entry| entry.expect("read native IDL entry").path())
        .collect();
    inputs.sort();
    for input in inputs {
        assert!(
            input
                .extension()
                .is_some_and(|extension| extension == "webidl"),
            "unexpected IDL input: {}",
            input.display()
        );
        let source = fs::read_to_string(&input).expect("read native IDL");
        let generated = webidl_bindgen::compile(&source)
            .unwrap_or_else(|error| panic!("{}: {error}", input.display()));
        let name = input.file_stem().expect("native IDL file name");
        assert!(
            !contract_interfaces.contains(name.to_string_lossy().as_ref()),
            "interface has both legacy and upstream bindings: {}",
            name.to_string_lossy()
        );
        fs::write(out.join(name).with_extension("rs"), generated).expect("write generated binding");
    }
}

fn binding_sources(directory: &Path, extension: &str, sources: &mut Vec<(String, String)>) {
    println!("cargo:rerun-if-changed={}", directory.display());
    let mut paths: Vec<_> = fs::read_dir(directory)
        .expect("read binding input directory")
        .map(|entry| entry.expect("read binding input entry").path())
        .collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            binding_sources(&path, extension, sources);
        } else if path.extension().is_some_and(|item| item == extension) {
            sources.push((
                path.display().to_string(),
                fs::read_to_string(&path).expect("read binding input"),
            ));
        }
    }
}
