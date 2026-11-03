//! In-memory WPT origin: every request reads a file from the checkout.
//!
//! Nothing is served over a socket. `testharnessreport.js` is replaced with a
//! small reporter so the host can read the harness result without `WebDriver`'s
//! message queue. Response metadata follows wptserve: the content-type table
//! comes from `wptserve.constants` (passed by `cli.py`), and headers are read
//! the way `wptserve.handlers.load_headers` reads them.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::SystemTime;

use cookies::{CookieJar, CookieOp, InitiatorKind, RetrievalKind};
use renderer::{
    BrowsingContextHost, DialCancellation, DialCompletion, DialFailure, DialOutcome, DialRequest,
    FrameId, MessagingHost, NetworkHost, OpenerPolicy, PostedMessage, StorageChange, StorageError,
    StorageHost, StorageKey, StorageKind, TargetName, WindowId,
};
use url::Url;

use crate::template::{self, Context};
use crate::wrapper::Wrapper;

/// The origin wptserve gives ordinary test documents.
pub const BASE: &str = "http://web-platform.test:8000/";

/// Replaces wptrunner's report script. The page records the same completion
/// payload the `WebDriver` harness reads from its message queue.
const REPORT: &str = r#"
(function () {
  add_completion_callback(function (tests, harness_status) {
    window.__tinybrowser_result = JSON.stringify({
      status: harness_status.status,
      message: harness_status.message,
      stack: harness_status.stack,
      tests: tests.map(function (test) {
        return {
          name: test.name,
          status: test.status,
          message: test.message,
          stack: test.stack
        };
      })
    });
  });
  setup({output: 0, timeout_multiplier: 1, explicit_timeout: false, debug: false,
         message_events: ["completion"]});
})();
"#;

/// The headers and content type wptserve answers one resource with.
pub(crate) struct Headers {
    /// The `Content-Type` a `.headers` file overrides, when it does.
    pub(crate) content_type: Option<String>,
    /// Every header, in wptserve's concatenation order with later values
    /// replacing earlier ones of the same name.
    pub(crate) list: Vec<(String, String)>,
}

/// The request facts a `.sub.headers` template can read.
pub(crate) struct RequestContext<'a> {
    /// The URL being served.
    pub(crate) url: &'a Url,
    /// Request headers.
    pub(crate) headers: &'a [(String, String)],
}

/// A host that maps every URL path to a file under one checkout.
pub struct FsServices {
    root: PathBuf,
    /// `wptserve.constants.content_types`, as a JSON map handed over by
    /// `cli.py`. An extension it does not name is `application/octet-stream`,
    /// exactly like `guess_content_type`.
    content_types: HashMap<String, String>,
    /// The engine's own jar, so `document.cookie` obeys path, domain, expiry,
    /// and `SameSite` here exactly as it does in a browser tab.
    cookies: Mutex<CookieJar>,
    storage: Mutex<HashMap<(bool, StorageKey, String), String>>,
    /// Every path this host read or tried to read for the current file.
    read: Mutex<Vec<PathBuf>>,
}

impl FsServices {
    /// Creates a host rooted at `root`.
    #[must_use]
    pub fn new(root: PathBuf, content_types: HashMap<String, String>) -> Self {
        Self {
            root,
            content_types,
            cookies: Mutex::new(CookieJar::default()),
            storage: Mutex::new(HashMap::new()),
            read: Mutex::new(Vec::new()),
        }
    }

    /// Drops cookies and storage between files.
    ///
    /// A worker builds a fresh engine per file, so state a file leaves behind
    /// must not reach the next one. Upstream also clears browser state between
    /// tests.
    pub fn clear(&self) {
        self.jar().clear();
        self.storage
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        self.read
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }

    fn jar(&self) -> MutexGuard<'_, CookieJar> {
        self.cookies.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Records one path as an input and returns it.
    fn record(&self, path: PathBuf) -> PathBuf {
        self.read
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(path.clone());
        path
    }

    /// Takes the paths read since the last [`Self::clear`].
    #[must_use]
    pub fn take_inputs(&self) -> Vec<PathBuf> {
        std::mem::take(&mut *self.read.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Records `path` as an input without reading it.
    pub fn record_file(&self, path: PathBuf) {
        self.record(path);
    }

    /// The content type wptserve's `guess_content_type` returns for `file`.
    pub(crate) fn content_type_for(&self, file: &Path) -> String {
        file.extension()
            .and_then(|extension| extension.to_str())
            .and_then(|extension| self.content_types.get(extension))
            .cloned()
            .unwrap_or_else(|| "application/octet-stream".to_owned())
    }

    /// Reads the headers wptserve sends for `file`.
    pub(crate) fn headers_for(&self, file: &Path, request: &RequestContext<'_>) -> Headers {
        let mut list = Vec::new();
        // Directory headers apply to the whole directory, then the file's own
        // headers follow
        // (`wptserve/handlers.py::load_headers`).
        if let Some(directory) = file.parent() {
            self.load_headers(&directory.join("__dir__"), request, &mut list);
        }
        self.load_headers(file, request, &mut list);
        let content_type = list
            .iter()
            .rev()
            .find(|(name, _)| name == "content-type")
            .map(|(_, value)| value.clone());
        // The response carries one `Content-Type`, set from the field above;
        // leaving the raw header in the list would send it twice.
        list.retain(|(name, _)| name != "content-type");
        Headers { content_type, list }
    }

    /// Appends one `__dir__` or resource header block.
    ///
    /// A `.sub.headers` sibling wins over `.headers`, and its templates are
    /// expanded against the request. An expression this harness cannot expand
    /// is a visible error: dropping the header would change the page silently.
    fn load_headers(
        &self,
        base: &Path,
        request: &RequestContext<'_>,
        list: &mut Vec<(String, String)>,
    ) {
        let sub = path_with_suffix(base, ".sub.headers");
        let plain = path_with_suffix(base, ".headers");
        // Both candidates are inputs, whether or not they exist: creating one
        // later must invalidate a cached verdict.
        self.record(sub.clone());
        self.record(plain.clone());
        let (path, expand) = if sub.is_file() {
            (sub, true)
        } else if plain.is_file() {
            (plain, false)
        } else {
            return;
        };
        let Ok(bytes) = std::fs::read(&path) else {
            return;
        };
        let text = if expand {
            let context = Context {
                url: request.url,
                headers: request.headers,
            };
            match template::substitute(&String::from_utf8_lossy(&bytes), &context) {
                Ok(text) => text,
                Err(error) => {
                    eprintln!("wpt-harness: {}: {error}", path.display());
                    return;
                }
            }
        } else {
            String::from_utf8_lossy(&bytes).into_owned()
        };
        for line in text.lines() {
            if line.is_empty() {
                continue;
            }
            let Some((name, value)) = line.split_once(':') else {
                continue;
            };
            set_header(list, name.trim(), value.trim());
        }
    }

    fn serve(&self, request: &DialRequest) -> Result<DialOutcome, DialFailure> {
        let path = percent_encoding::percent_decode_str(request.url.path())
            .decode_utf8()
            .map_err(|_| DialFailure::Connect)?;
        if path == "/resources/testharnessreport.js" {
            return Ok(DialOutcome::basic(
                200,
                request.url.clone(),
                Some("text/javascript".into()),
                Vec::new(),
                REPORT.as_bytes().to_vec(),
            ));
        }
        let relative = Path::new(path.trim_start_matches('/'));
        // A test URL is trusted input, but a `..` segment would read outside
        // the checkout. Refuse it instead of canonicalizing every request.
        if relative
            .components()
            .any(|component| component == Component::ParentDir)
        {
            return Err(DialFailure::Connect);
        }
        // wptserve executes a `.py` handler for such a path. Serving the
        // handler's own source instead would hand a test a 200 body no browser
        // ever sees, so the resource is a network error in this lane until the
        // handlers are implemented.
        if relative
            .extension()
            .is_some_and(|extension| extension == "py")
        {
            return Err(DialFailure::Connect);
        }
        let context = RequestContext {
            url: &request.url,
            headers: &request.headers,
        };
        // `.any.html` and `.window.html` are synthesized from their script.
        if let Some((wrapper, source)) = Wrapper::for_url(&path) {
            let file = self.record(self.root.join(&source));
            let body = std::fs::read(&file).map_err(|_| DialFailure::Connect)?;
            let headers = self.headers_for(&file, &context);
            let js_url = &request.url.path()[..request.url.path().len() - ".html".len()];
            let html = wrapper.render(&body, &format!("{js_url}.js"));
            return Ok(DialOutcome::basic(
                200,
                request.url.clone(),
                Some(
                    headers
                        .content_type
                        .unwrap_or_else(|| "text/html".to_owned()),
                ),
                headers.list,
                html.into_bytes(),
            ));
        }
        let file = self.record(self.root.join(relative));
        let body = std::fs::read(&file).map_err(|_| DialFailure::Connect)?;
        let headers = self.headers_for(&file, &context);
        Ok(DialOutcome::basic(
            200,
            request.url.clone(),
            Some(
                headers
                    .content_type
                    .unwrap_or_else(|| self.content_type_for(&file)),
            ),
            headers.list,
            body,
        ))
    }
}

/// Appends `suffix` to a path that has no extension of its own, such as
/// `dir/__dir__`.
fn path_with_suffix(base: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{suffix}", base.display()))
}

/// Sets one header, replacing an earlier value of the same name.
///
/// wptserve's `ResponseHeaders.set` overwrites, so a file's header wins over
/// the directory's and only one value is sent.
fn set_header(list: &mut Vec<(String, String)>, name: &str, value: &str) {
    let name = name.to_ascii_lowercase();
    if let Some(entry) = list.iter_mut().find(|(existing, _)| *existing == name) {
        value.clone_into(&mut entry.1);
    } else {
        list.push((name, value.to_owned()));
    }
}

impl NetworkHost for FsServices {
    fn start_dial(&self, request: DialRequest, completion: DialCompletion) -> DialCancellation {
        completion(self.serve(&request));
        Box::new(|| {})
    }

    fn cookies_for(&self, url: &Url) -> String {
        self.jar().cookie_string(cookie_op(url))
    }

    fn set_cookie(&self, value: &str, url: &Url) {
        let _stored = self.jar().store(value, cookie_op(url));
    }
}

/// One `document.cookie` operation for `url`.
///
/// `now` is the wall clock, not the paused one: expiry is a property of the
/// cookie, and a WPT file should not be able to observe it as time travel.
/// The document is its own initiator, so a request with no initiator context
/// (same-site) is the honest description of a script-set cookie.
fn cookie_op(url: &Url) -> CookieOp<'_> {
    CookieOp {
        url,
        now: SystemTime::now(),
        kind: RetrievalKind::NonHttp,
        initiator_kind: InitiatorKind::Fetch,
        method_is_safe: true,
        initiator: None,
        cross_site_redirect: false,
    }
}

impl StorageHost for FsServices {
    fn storage_get(&self, kind: StorageKind, origin: &StorageKey, key: &str) -> Option<String> {
        self.storage
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&(kind == StorageKind::Local, origin.clone(), key.to_owned()))
            .cloned()
    }

    fn storage_keys(&self, kind: StorageKind, origin: &StorageKey) -> Vec<String> {
        self.storage
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .filter(|(local, key_origin, _)| {
                *local == (kind == StorageKind::Local) && key_origin == origin
            })
            .map(|(_, _, key)| key.clone())
            .collect()
    }

    fn storage_set(
        &self,
        kind: StorageKind,
        origin: &StorageKey,
        _url: &str,
        key: &str,
        value: &str,
        _source: FrameId,
    ) -> Result<Option<StorageChange>, StorageError> {
        self.storage
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                (kind == StorageKind::Local, origin.clone(), key.to_owned()),
                value.to_owned(),
            );
        Ok(None)
    }

    fn storage_remove(
        &self,
        kind: StorageKind,
        origin: &StorageKey,
        _url: &str,
        key: &str,
        _source: FrameId,
    ) -> Option<StorageChange> {
        self.storage
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&(kind == StorageKind::Local, origin.clone(), key.to_owned()));
        None
    }

    fn storage_clear(
        &self,
        kind: StorageKind,
        origin: &StorageKey,
        _url: &str,
        _source: FrameId,
    ) -> Option<StorageChange> {
        let mut storage = self.storage.lock().unwrap_or_else(PoisonError::into_inner);
        storage.retain(|(local, key_origin, _), _| {
            !(*local == (kind == StorageKind::Local) && key_origin == origin)
        });
        None
    }
}

impl BrowsingContextHost for FsServices {
    fn window_open(
        &self,
        _url: Option<&Url>,
        _target: &TargetName,
        _opener: OpenerPolicy,
        _source: &renderer::NavigationSource,
    ) -> Option<WindowId> {
        None
    }

    fn navigate_named(
        &self,
        _name: &renderer::ContextName,
        _url: Option<&Url>,
        _opener: OpenerPolicy,
        _source: &renderer::NavigationSource,
    ) -> Option<WindowId> {
        None
    }

    fn window_close(&self, _window: WindowId) {}

    /// A target name only matters to a browser that keeps a cross-tab table.
    /// This host has one tab, so the name has nowhere to go.
    fn set_window_name(&self, _name: Option<&renderer::ContextName>) {}

    fn window_opener(&self) -> Option<WindowId> {
        None
    }

    fn window_post_message(&self, _window: WindowId, _message: PostedMessage) {}

    fn remote_session_get(
        &self,
        _window: WindowId,
        _origin: &StorageKey,
        _key: &str,
    ) -> Option<String> {
        None
    }
}

impl MessagingHost for FsServices {
    fn broadcast_post(&self, _origin: &StorageKey, _name: &str, _payload: &str, _channel: u64) {}
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use renderer::{DialFailure, DialOutcome};

    use super::{FsServices, RequestContext};

    fn root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "wpt-harness-services-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("dir")).expect("root created");
        root
    }

    fn services(name: &str) -> FsServices {
        FsServices::new(
            root(name),
            HashMap::from([("html".to_owned(), "text/html".to_owned())]),
        )
    }

    fn outcome(services: &FsServices, path: &str) -> Result<DialOutcome, DialFailure> {
        let url =
            url::Url::parse(&format!("http://web-platform.test:8000{path}")).expect("test URL");
        services.serve(&renderer::DialRequest::new(
            renderer::RequestPolicy::Image { crossorigin: None },
            &url,
            &url.origin(),
        ))
    }

    fn headers(services: &FsServices, url: &str) -> super::Headers {
        let url = url::Url::parse(url).expect("URL");
        services.headers_for(
            &services.root.join("dir/data.txt"),
            &RequestContext {
                url: &url,
                headers: &[],
            },
        )
    }

    #[test]
    fn a_static_file_is_served_and_a_handler_is_not() {
        let services = services("handler");
        let root = services.root.clone();
        std::fs::write(root.join("dir/data.txt"), b"from a file").expect("file written");
        std::fs::write(
            root.join("dir/handler.py"),
            b"def main(request, response): ...",
        )
        .expect("handler written");

        let served = outcome(&services, "/dir/data.txt").expect("static file");
        assert!(
            matches!(&served.body, renderer::ResponseBody::Bytes(bytes) if bytes == b"from a file")
        );

        // Serving the handler's source would hand a test a 200 that no browser
        // ever sees for that URL.
        assert!(
            outcome(&services, "/dir/handler.py").is_err(),
            "a wptserve handler is a network error, not its own source"
        );
        assert!(
            outcome(&services, "/dir/missing.txt").is_err(),
            "an absent file is a network error"
        );
        std::fs::remove_dir_all(&root).expect("root removed");
    }

    #[test]
    fn content_types_and_header_files_follow_wptserve() {
        let services = services("headers");
        let root = services.root.clone();
        std::fs::write(root.join("dir/data.txt"), b"x").expect("file");
        // Directory headers apply to every file below the directory.
        std::fs::write(
            root.join("dir/__dir__.headers"),
            "X-Content-Type-Options: nosniff\n",
        )
        .expect("dir headers");
        // A `.sub.headers` sibling wins over `.headers`, and expands.
        std::fs::write(root.join("dir/data.txt.headers"), "X-Plain: yes\n").expect("headers");
        std::fs::write(
            root.join("dir/data.txt.sub.headers"),
            "Access-Control-Allow-Origin: {{location[server]}}\n",
        )
        .expect("sub headers");
        let headers = headers(&services, "http://web-platform.test:8000/dir/data.txt");
        assert_eq!(
            headers.list,
            vec![
                ("x-content-type-options".to_owned(), "nosniff".to_owned()),
                (
                    "access-control-allow-origin".to_owned(),
                    "http://web-platform.test:8000".to_owned()
                ),
            ],
            "the .sub.headers sibling replaces the plain one"
        );
        assert_eq!(headers.content_type, None);
        // The table is wptserve's: `txt` is text/plain, unknown is binary.
        assert_eq!(
            services.content_type_for(std::path::Path::new("a.txt")),
            "application/octet-stream",
            "only extensions the passed table names are known"
        );
        std::fs::remove_dir_all(&root).expect("root removed");
    }

    #[test]
    fn an_any_wrapper_is_served_from_its_script() {
        let services = services("wrapper");
        let root = services.root.clone();
        std::fs::write(
            root.join("dir/multi.any.js"),
            "// META: global=window\ntest(() => {}, 'x');\n",
        )
        .expect("source");
        let served = outcome(&services, "/dir/multi.any.html").expect("wrapper");
        let renderer::ResponseBody::Bytes(bytes) = served.body else {
            panic!("a wrapper has a byte body");
        };
        let html = String::from_utf8(bytes).expect("HTML");
        assert!(html.contains("self.GLOBAL"));
        assert!(html.contains("<script src=\"/dir/multi.any.js\"></script>"));
        assert!(
            services
                .take_inputs()
                .iter()
                .any(|path| path.ends_with("dir/multi.any.js")),
            "the wrapped script is a cache input"
        );
        std::fs::remove_dir_all(&root).expect("root removed");
    }
}
