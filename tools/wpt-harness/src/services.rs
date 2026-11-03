//! In-memory WPT origin: every request reads a file from the checkout.
//!
//! Nothing is served over a socket. `testharnessreport.js` is replaced with a
//! small reporter so the host can read the harness result without `WebDriver`'s
//! message queue.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use renderer::{
    BrowsingContextHost, DialCancellation, DialCompletion, DialFailure, DialOutcome, DialRequest,
    FrameId, MessagingHost, NetworkHost, OpenerPolicy, PostedMessage, StorageChange, StorageError,
    StorageHost, StorageKey, StorageKind, TargetName, WindowId,
};
use url::Url;

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

/// A host that maps every URL path to a file under one checkout.
pub struct FsServices {
    root: PathBuf,
    cookies: Mutex<HashMap<String, String>>,
    storage: Mutex<HashMap<(bool, StorageKey, String), String>>,
    /// Every path this host read or tried to read for the current file.
    read: Mutex<Vec<PathBuf>>,
}

impl FsServices {
    /// Creates a host rooted at `root`.
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            cookies: Mutex::new(HashMap::new()),
            storage: Mutex::new(HashMap::new()),
            read: Mutex::new(Vec::new()),
        }
    }

    /// Drops cookies and storage between files.
    ///
    /// A worker keeps one engine and navigates it from test to test, so state
    /// a file leaves behind must not reach the next one. Upstream also clears
    /// browser state between tests.
    pub fn clear(&self) {
        self.cookies
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        self.storage
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        self.read
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
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
        std::mem::take(
            &mut *self
                .read
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        )
    }

    /// Records `path` as an input without reading it.
    pub fn record_file(&self, path: PathBuf) {
        self.record(path);
    }

    /// Reads one resource's `.headers` sibling, recording it as an input.
    pub(crate) fn headers_for(&self, file: &Path) -> (Option<String>, Vec<(String, String)>) {
        let path = self.record(PathBuf::from(format!("{}.headers", file.display())));
        read_headers(file, &path)
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
        let file = self.record(self.root.join(relative));
        let body = std::fs::read(&file).map_err(|_| DialFailure::Connect)?;
        // `.headers` is wptserve's per-resource header file.
        let (content_type, headers) = self.headers_for(&file);
        Ok(DialOutcome::basic(
            200,
            request.url.clone(),
            content_type,
            headers,
            body,
        ))
    }
}

/// Reads the header list from `headers` for a resource at `file`.
fn read_headers(file: &Path, headers: &Path) -> (Option<String>, Vec<(String, String)>) {
    let mut list = Vec::new();
    let mut content_type = content_type(file);
    if let Ok(text) = std::fs::read_to_string(headers) {
        for line in text.lines() {
            let Some((name, value)) = line.split_once(':') else {
                continue;
            };
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim().to_owned();
            if name == "content-type" {
                content_type = Some(value.clone());
            }
            list.push((name, value));
        }
    }
    (content_type, list)
}

/// The content type a static file reports.
fn content_type(path: &Path) -> Option<String> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    let value = match extension.as_str() {
        "html" | "htm" => "text/html",
        "xhtml" => "application/xhtml+xml",
        "js" | "mjs" => "text/javascript",
        "css" => "text/css",
        "json" => "application/json",
        "xml" => "application/xml",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        "txt" => "text/plain",
        _ => "application/octet-stream",
    };
    Some(value.to_owned())
}

impl NetworkHost for FsServices {
    fn start_dial(&self, request: DialRequest, completion: DialCompletion) -> DialCancellation {
        completion(self.serve(&request));
        Box::new(|| {})
    }

    fn cookies_for(&self, url: &Url) -> String {
        self.cookies
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&url.origin().ascii_serialization())
            .cloned()
            .unwrap_or_default()
    }

    fn set_cookie(&self, value: &str, url: &Url) {
        self.cookies
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(url.origin().ascii_serialization(), value.to_owned());
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
        _allow_popups: bool,
    ) -> Option<WindowId> {
        None
    }

    fn window_by_name(&self, _name: &str) -> Option<WindowId> {
        None
    }

    fn window_navigate(
        &self,
        _window: WindowId,
        _name: &str,
        _url: Option<&Url>,
        _opener: OpenerPolicy,
        _source: &renderer::NavigationSource,
    ) {
    }

    fn window_close(&self, _window: WindowId) {}

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
