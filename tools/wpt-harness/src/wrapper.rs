//! wptserve's wrapper documents for multi-global and window test sources.
//!
//! WPT stores `.any.js` and `.window.js` sources as JavaScript, and wptserve
//! synthesizes the HTML a browser navigates to. The bytes have to match the
//! server's, because a test reads `document.URL`, `self.GLOBAL`, and the
//! wrapper's metadata as facts about itself. The templates below are copied
//! from `third_party/wpt/tools/serve/serve.py` (`AnyHtmlHandler`,
//! `WindowHandler`) and the metadata rules from
//! `tools/manifest/sourcefile.py` (`js_meta_re`, `_meta_replacement`,
//! `_script_replacement`).

/// Which wptserve handler renders a test URL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wrapper {
    /// `AnyHtmlHandler`: `.any.html` from a `.any.js` source.
    Any,
    /// `WindowHandler`: `.window.html` from a `.window.js` source.
    Window,
}

impl Wrapper {
    /// The wrapper a test URL asks for, with the on-disk source it wraps.
    ///
    /// Only the two window-global wrappers are rendered; worker, service
    /// worker, module, and shadow-realm variants need executors this lane does
    /// not have, so they are not selected at all.
    #[must_use]
    pub fn for_url(url_path: &str) -> Option<(Self, String)> {
        if let Some(base) = url_path.strip_suffix(".any.html") {
            return Some((
                Self::Any,
                base.trim_start_matches('/').to_owned() + ".any.js",
            ));
        }
        let base = url_path.strip_suffix(".window.html")?;
        Some((
            Self::Window,
            format!("{}.window.js", base.trim_start_matches('/')),
        ))
    }

    /// The wrapper document, given the wrapped source and its URL path.
    ///
    /// `js_url` is the script's path as the browser will request it, so it
    /// keeps the test's directory and encoding.
    #[must_use]
    pub fn render(self, js_bytes: &[u8], js_url: &str) -> String {
        let metadata = metadata(js_bytes);
        let mut meta = String::new();
        let mut scripts = String::new();
        for (key, value) in &metadata {
            match key.as_str() {
                // `timeout` and `title` become document metadata;
                // `script` becomes a `<script src>` before the harness runs.
                "timeout" if value == "long" => {
                    meta.push_str("<meta name=\"timeout\" content=\"long\">\n");
                }
                "title" => {
                    let escaped = value.replace('&', "&amp;").replace('<', "&lt;");
                    meta.push_str("<title>");
                    meta.push_str(&escaped);
                    meta.push_str("</title>\n");
                }
                "script" => {
                    let escaped = value.replace('&', "&amp;").replace('"', "&quot;");
                    scripts.push_str("<script src=\"");
                    scripts.push_str(&escaped);
                    scripts.push_str("\"></script>\n");
                }
                _ => {}
            }
        }
        let header = format!("<!doctype html>\n<meta charset=utf-8>\n{meta}");
        let harness = "<script src=\"/resources/testharness.js\"></script>\n\
                       <script src=\"/resources/testharnessreport.js\"></script>\n";
        match self {
            // The document must look like a window to the shared test code.
            Self::Any => format!(
                "{header}\
                 <script>\n\
                 self.GLOBAL = {{\n\
                 \x20 isWindow: function() {{ return true; }},\n\
                 \x20 isWorker: function() {{ return false; }},\n\
                 \x20 isShadowRealm: function() {{ return false; }},\n\
                 }};\n\
                 </script>\n\
                 {harness}{scripts}<div id=log></div>\n\
                 <script src=\"{js_url}\"></script>\n"
            ),
            Self::Window => {
                format!(
                    "{header}{harness}{scripts}<div id=log></div>\n<script src=\"{js_url}\"></script>\n"
                )
            }
        }
    }
}

/// The script metadata at the top of a multi-global or window source.
///
/// Upstream reads `// META: key=value` lines from the top of the file and
/// stops at the first line that does not match
/// (`tools/manifest/sourcefile.py:42`, `read_script_metadata`).
fn metadata(bytes: &[u8]) -> Vec<(String, String)> {
    let text = String::from_utf8_lossy(bytes);
    let mut out = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let Some(rest) = line.strip_prefix("//") else {
            break;
        };
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix("META:") else {
            break;
        };
        let rest = rest.trim_start();
        let Some((key, value)) = rest.split_once('=') else {
            break;
        };
        out.push((key.to_owned(), value.to_owned()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{Wrapper, metadata};

    #[test]
    fn only_window_global_wrappers_are_rendered() {
        assert_eq!(
            Wrapper::for_url("/dom/foo.any.html"),
            Some((Wrapper::Any, "dom/foo.any.js".into()))
        );
        assert_eq!(
            Wrapper::for_url("/dom/foo.window.html"),
            Some((Wrapper::Window, "dom/foo.window.js".into()))
        );
        for url in [
            "/dom/foo.any.worker.html",
            "/dom/foo.any.serviceworker.html",
            "/dom/foo.any.sharedworker.html",
            "/dom/foo.any.window-module.html",
            "/dom/foo.html",
        ] {
            assert_eq!(Wrapper::for_url(url), None, "{url}");
        }
    }

    #[test]
    fn metadata_follows_upstreams_leading_comment_rule() {
        let source = concat!(
            "// META: global=window,worker\n",
            "// META: script=/resources/a.js\n",
            "// META: timeout=long\n",
            "// comment\n",
        );
        assert_eq!(
            metadata(source.as_bytes()),
            vec![
                ("global".into(), "window,worker".into()),
                ("script".into(), "/resources/a.js".into()),
                ("timeout".into(), "long".into()),
            ]
        );
        // Reading stops at the first non-metadata line, as upstream does.
        assert_eq!(metadata(b"// comment\n// META: script=/x.js\n").len(), 0);
    }

    #[test]
    fn a_wrapper_carries_the_harness_the_metadata_and_the_global() {
        let html = Wrapper::Any.render(
            b"// META: title=A & B\n// META: script=/resources/x.js?y=1&z=2\n// META: timeout=long\n",
            "/dom/foo.any.js",
        );
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("<title>A &amp; B</title>"));
        assert!(
            html.contains("<script src=\"/resources/x.js?y=1&amp;z=2\"></script>"),
            "{html}"
        );
        assert!(html.contains("<meta name=\"timeout\" content=\"long\">"));
        assert!(html.contains("self.GLOBAL"));
        assert!(html.contains("<script src=\"/dom/foo.any.js\"></script>"));
        assert!(html.contains("/resources/testharnessreport.js"));

        let window = Wrapper::Window.render(b"", "/dom/foo.window.js");
        assert!(!window.contains("self.GLOBAL"));
        assert!(window.contains("<script src=\"/dom/foo.window.js\"></script>"));
    }
}
