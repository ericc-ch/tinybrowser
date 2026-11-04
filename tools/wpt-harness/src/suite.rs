//! Selecting WPT tests from the checkout's manifest.
//!
//! The vendored `tools/manifest` decides what a test is, and `cli.py` keeps
//! `MANIFEST.json` current. This module consumes that manifest instead of
//! walking the tree and guessing from file names: a helper page, a reference,
//! and a crashtest are three different things, and only the manifest knows
//! which is which.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::wrapper::Wrapper;

/// Which executor a test needs, shared with the report so the two agree on a
/// test's kind.
pub use wpt_report::Kind;

/// One selected test.
#[derive(Clone, Debug)]
pub struct TestCase {
    /// The URL path and query the browser navigates to, relative to the
    /// origin. This is the wptreport id without its leading slash.
    pub relative: String,
    /// The on-disk source, relative to the WPT root.
    pub source: String,
    /// The executor to use.
    pub kind: Kind,
    /// Source bytes, read once before workers start.
    ///
    /// Bytes, not `String`: WPT files are not all UTF-8, and the response
    /// charset tells the parser how to decode them.
    pub bytes: Vec<u8>,
    /// The response content type for `bytes`.
    pub content_type: String,
    /// Whether the manifest gives the test the long timeout.
    pub long_timeout: bool,
}

impl TestCase {
    /// The wptreport identifier.
    #[must_use]
    pub fn id(&self) -> String {
        format!("/{}", self.relative)
    }
}

/// Reads the manifest's testharness and crashtest items below the requested
/// paths.
///
/// `paths` are relative to `root` and may name files or directories. An empty
/// list is rejected by the caller so a run cannot accidentally start the full
/// suite.
///
/// # Errors
///
/// The manifest is missing or malformed, a selected path matches nothing, or a
/// selected source cannot be read.
pub fn select(
    root: &Path,
    paths: &[String],
    offset: usize,
    limit: usize,
    content_types: &HashMap<String, String>,
) -> io::Result<Vec<TestCase>> {
    let manifest = read_manifest(root)?;
    let mut selected: Vec<Item> = Vec::new();
    for kind in ["testharness", "crashtest"] {
        let Some(tree) = manifest.get("items").and_then(|items| items.get(kind)) else {
            continue;
        };
        walk(tree, "", &mut |path, leaf| {
            collect_leaf(root, path, leaf, kind, paths, &mut selected);
        });
    }
    selected.sort_by(|left, right| left.relative.cmp(&right.relative));
    selected.dedup_by(|left, right| left.relative == right.relative);
    if selected.is_empty() {
        return Err(io::Error::other(format!(
            "no manifest test matches {}",
            paths.join(", ")
        )));
    }
    // A shard runs one contiguous slice of the sorted selection; the driver
    // can shrink a slice that crashes the engine without changing what the
    // slice means.
    let mut cases = Vec::new();
    for item in selected.into_iter().skip(offset).take(limit) {
        cases.push(item.materialize(root, content_types)?);
    }
    Ok(cases)
}

/// A test the manifest listed that still has to be matched and materialized.
struct Item {
    /// The URL path and query.
    relative: String,
    /// The on-disk source.
    source: String,
    kind: Kind,
    long_timeout: bool,
    /// The wrapper to synthesize, when the URL names one.
    wrapper: Option<Wrapper>,
}

impl Item {
    fn materialize(
        self,
        root: &Path,
        content_types: &HashMap<String, String>,
    ) -> io::Result<TestCase> {
        let bytes = fs::read(root.join(&self.source)).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("cannot read {}: {error}", self.source),
            )
        })?;
        let (bytes, content_type) = if let Some(wrapper) = self.wrapper {
            let js_url = js_url(&self.relative);
            (
                wrapper.render(&bytes, &js_url).into_bytes(),
                "text/html".to_owned(),
            )
        } else {
            let content_type = content_type_for(Path::new(&self.source), content_types);
            (bytes, content_type)
        };
        Ok(TestCase {
            relative: self.relative,
            source: self.source,
            kind: self.kind,
            bytes,
            content_type,
            long_timeout: self.long_timeout,
        })
    }
}

/// Reads `MANIFEST.json` from the checkout.
fn read_manifest(root: &Path) -> io::Result<Value> {
    let path = root.join("MANIFEST.json");
    let bytes = fs::read(&path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "cannot read {}: {error} (run `cli.py manifest` to build it)",
                path.display()
            ),
        )
    })?;
    serde_json::from_slice(&bytes).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{}: {error}", path.display()),
        )
    })
}

/// Visits every leaf of the manifest's directory tree.
fn walk(node: &Value, prefix: &str, visit: &mut impl FnMut(&str, &Value)) {
    let Some(entries) = node.as_object() else {
        return;
    };
    for (name, value) in entries {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        if value.is_object() {
            walk(value, &path, visit);
        } else {
            visit(&path, value);
        }
    }
}

/// Turns one manifest leaf (`[hash, variant...]`) into matchable items.
fn collect_leaf(
    root: &Path,
    path: &str,
    leaf: &Value,
    kind: &str,
    paths: &[String],
    selected: &mut Vec<Item>,
) {
    let Some(entries) = leaf.as_array() else {
        return;
    };
    // The first element is the source digest; the rest are test variants.
    for entry in entries.iter().skip(1) {
        let Some(entry) = entry.as_array() else {
            continue;
        };
        if entry.len() < 2 {
            continue;
        }
        // Testharness variants are `[url, props]`; a jsshell variant is the
        // URL-less form and only runs in the jsshell product, not a browser.
        let empty = Map::new();
        let props = entry.last().and_then(Value::as_object).unwrap_or(&empty);
        if props.contains_key("jsshell") {
            continue;
        }
        let url = entry[0].as_str().unwrap_or(path);
        let url_path = url.split('?').next().unwrap_or(url);
        let (source, wrapper) = match Wrapper::for_url(url_path) {
            Some((wrapper, source)) => (source, Some(wrapper)),
            // Worker, service-worker, module, and shadow-realm wrappers are
            // generated by handlers this lane does not run; their URL is not a
            // file, so the existence check below drops them.
            None => (url_path.trim_start_matches('/').to_owned(), None),
        };
        if !matches_requested(paths, &source, url_path) {
            continue;
        }
        if !root.join(&source).is_file() {
            continue;
        }
        let kind = match kind {
            "crashtest" => Kind::Crashtest,
            // testdriver.js needs a WebDriver session; select the file so the
            // summary counts it, but the runner skips it.
            _ if props.get("testdriver").and_then(Value::as_bool) == Some(true) => Kind::Testdriver,
            _ => Kind::Testharness,
        };
        selected.push(Item {
            relative: url.trim_start_matches('/').to_owned(),
            source,
            kind,
            long_timeout: props.get("timeout").and_then(Value::as_str) == Some("long"),
            wrapper,
        });
    }
}

/// Whether a test falls under one of the requested paths.
///
/// A request may name the test URL (`.any.html`) or the on-disk source
/// (`.any.js`); both are names a person would type.
fn matches_requested(paths: &[String], source: &str, url_path: &str) -> bool {
    let url_path = url_path.trim_start_matches('/');
    paths.iter().any(|request| {
        let request = request
            .trim_start_matches('/')
            .split('?')
            .next()
            .unwrap_or("")
            .trim_end_matches('/');
        request.is_empty()
            || [source, url_path].iter().any(|candidate| {
                *candidate == request || candidate.starts_with(&format!("{request}/"))
            })
    })
}

/// The script URL a wrapper loads: the document URL with `.html` swapped for
/// `.js`, as an absolute path so it resolves from the test's document.
fn js_url(document: &str) -> String {
    let path = document
        .split('?')
        .next()
        .unwrap_or(document)
        .trim_start_matches('/');
    let base = path.strip_suffix(".html").unwrap_or(path);
    format!("/{base}.js")
}

/// The content type wptserve's `guess_content_type` returns for `path`.
fn content_type_for(path: &Path, content_types: &HashMap<String, String>) -> String {
    path.extension()
        .and_then(|extension| extension.to_str())
        .and_then(|extension| content_types.get(extension))
        .cloned()
        .unwrap_or_else(|| "application/octet-stream".to_owned())
}

/// The default checkout location, relative to the repository root.
#[must_use]
pub fn default_root() -> PathBuf {
    PathBuf::from("third_party/wpt")
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;

    use super::{Kind, select};

    /// A checkout with a manifest listing one testharness file, one `.any.js`
    /// source, a worker variant, a jsshell variant, a crashtest, and a helper
    /// page the manifest calls support.
    fn checkout(name: &str) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("wpt-harness-suite-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("dom/nodes/resources")).expect("checkout");
        fs::create_dir_all(root.join("crashtests")).expect("crashtest directory");
        fs::write(
            root.join("dom/nodes/plain.html"),
            "<script src=/resources/testharness.js></script>",
        )
        .expect("plain test");
        fs::write(
            root.join("dom/nodes/multi.any.js"),
            "// META: global=window,worker\n// META: timeout=long\ntest(() => {}, 'x');\n",
        )
        .expect("multi source");
        fs::write(
            root.join("dom/nodes/window.window.js"),
            "test(() => {}, 'w');",
        )
        .expect("window source");
        fs::write(
            root.join("dom/nodes/resources/helper.html"),
            "<p>helper</p>",
        )
        .expect("helper");
        fs::write(root.join("crashtests/boom.html"), "<p>boom</p>").expect("crashtest");
        let manifest = serde_json::json!({
            "version": 9,
            "url_base": "/",
            "items": {
                "support": {
                    "dom": {"nodes": {"resources": {
                        "helper.html": ["h", [null, {}]]
                    }}}
                },
                "testharness": {
                    "dom": {"nodes": {
                        "plain.html": ["p", [null, {}]],
                        "multi.any.js": ["m",
                            [null, {"jsshell": true}],
                            ["dom/nodes/multi.any.html", {"timeout": "long"}],
                            ["dom/nodes/multi.any.worker.html", {}]
                        ],
                        "window.window.js": ["w", ["dom/nodes/window.window.html", {}]]
                    }}
                },
                "crashtest": {
                    "crashtests": {
                        "boom.html": ["c", ["crashtests/boom.html", {"timeout": "long", "testdriver": true}]]
                    }
                }
            }
        });
        fs::write(
            root.join("MANIFEST.json"),
            serde_json::to_vec(&manifest).expect("manifest"),
        )
        .expect("manifest written");
        root
    }

    fn types() -> HashMap<String, String> {
        HashMap::from([
            ("html".to_owned(), "text/html".to_owned()),
            ("js".to_owned(), "text/javascript".to_owned()),
        ])
    }

    #[test]
    fn the_manifest_decides_type_timeout_and_wrapper() {
        let root = checkout("all");
        let cases =
            select(&root, &["dom/nodes".into()], 0, usize::MAX, &types()).expect("selection");
        let ids: Vec<&str> = cases.iter().map(|case| case.relative.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "dom/nodes/multi.any.html",
                "dom/nodes/plain.html",
                "dom/nodes/window.window.html",
            ],
            "the worker wrapper and the jsshell variant are not browser tests"
        );
        let multi = &cases[0];
        assert_eq!(multi.kind, Kind::Testharness);
        assert!(multi.long_timeout, "the manifest marks the long timeout");
        assert_eq!(multi.source, "dom/nodes/multi.any.js");
        assert_eq!(multi.content_type, "text/html");
        let html = String::from_utf8(multi.bytes.clone()).expect("wrapper is UTF-8");
        assert!(html.contains("self.GLOBAL"));
        assert!(html.contains("<meta name=\"timeout\" content=\"long\">"));
        assert!(html.contains("<script src=\"/dom/nodes/multi.any.js\"></script>"));

        assert_eq!(cases[1].kind, Kind::Testharness);
        assert_eq!(cases[1].content_type, "text/html");
        assert!(!cases[1].long_timeout);

        let window = String::from_utf8(cases[2].bytes.clone()).expect("wrapper is UTF-8");
        assert!(
            !window.contains("self.GLOBAL"),
            "window.js has its own wrapper"
        );
        assert!(window.contains("/dom/nodes/window.window.js"));

        // A crashtest is a crashtest, and its testdriver flag is irrelevant:
        // the executor only loads the page.
        let crash = select(&root, &["crashtests".into()], 0, usize::MAX, &types())
            .expect("crashtest selection");
        assert_eq!(crash.len(), 1);
        assert_eq!(crash[0].kind, Kind::Crashtest);
        assert!(crash[0].long_timeout);
        fs::remove_dir_all(&root).expect("checkout removed");
    }

    #[test]
    fn a_testdriver_testharness_is_selected_and_skipped() {
        let root = checkout("testdriver");
        let manifest = serde_json::json!({
            "items": {"testharness": {"t.html": ["h", [null, {"testdriver": true}]]}}
        });
        fs::write(root.join("t.html"), "<script src=/resources/testdriver.js>").expect("test");
        fs::write(
            root.join("MANIFEST.json"),
            serde_json::to_vec(&manifest).expect("manifest"),
        )
        .expect("manifest written");
        let cases = select(&root, &["t.html".into()], 0, usize::MAX, &types()).expect("selection");
        assert_eq!(cases[0].kind, Kind::Testdriver);
        fs::remove_dir_all(&root).expect("checkout removed");
    }

    #[test]
    fn paths_select_files_directories_and_sources() {
        let root = checkout("paths");
        assert_eq!(
            select(
                &root,
                &["dom/nodes/plain.html".into()],
                0,
                usize::MAX,
                &types()
            )
            .expect("file")
            .len(),
            1
        );
        assert_eq!(
            select(
                &root,
                &["dom/nodes/multi.any.js".into()],
                0,
                usize::MAX,
                &types()
            )
            .expect("source name")
            .len(),
            1
        );
        assert_eq!(
            select(
                &root,
                &["dom/nodes/multi.any.html".into()],
                0,
                usize::MAX,
                &types()
            )
            .expect("test URL")
            .len(),
            1
        );
        // A slice keeps its place in the sorted selection.
        let sliced = select(&root, &["dom/nodes".into()], 1, 1, &types()).expect("slice");
        assert_eq!(sliced.len(), 1);
        assert_eq!(sliced[0].relative, "dom/nodes/plain.html");
        assert!(select(&root, &["missing".into()], 0, 10, &types()).is_err());
        fs::remove_dir_all(&root).expect("checkout removed");
    }
}
