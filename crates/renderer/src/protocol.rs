//! What a carrier exchanges with the engine, and the port it calls back on.
//!
//! These are the engine's own types: frame identity, mounts, page events,
//! dials, script results, and explicit errors. DOM handles, `QuickJS` values,
//! callbacks, and `net` types never cross. The messages that carry them over a
//! socket live in the browser crate's wire module, because only a carrier
//! needs to know how they travel.

use std::fmt;

use serde::{Deserialize, Serialize};
use url::Url;

pub use webstorage::{STORAGE_QUOTA_BYTES, StorageChange, StorageError};

/// Maximum aggregate bytes retained for one streamed response.
///
/// A Wikipedia article is already past 1 MiB. 8 MiB still caps one document
/// while leaving room for a long page and its streamed chunks.
pub const MAX_RESPONSE_BODY_BYTES: usize = 8 * 1024 * 1024;

/// Renderer-process identity of one frame.
///
/// The renderer mints ids for the frames it hosts; the browser process routes
/// frame-addressed commands and events by it. The tab's main frame is
/// [`FrameId::MAIN`] in every renderer.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FrameId(u64);

impl FrameId {
    /// The tab's main frame, present in every renderer.
    pub const MAIN: Self = Self(0);

    /// Constructs a frame id from a protocol integer.
    #[must_use]
    pub fn new(raw: u64) -> Self {
        Self(raw)
    }

    /// Stable numeric identity for protocol messages.
    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Selects the evaluation environment at the browser-to-renderer boundary.
/// Page sources run globally. Browser expressions receive the private bridge
/// and must never contain interpolated author code.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum ScriptSource<T = String> {
    /// Page code, evaluated in the global environment without engine capabilities.
    Page(T),
    /// A browser-owned expression, evaluated with the private realm bridge.
    /// Author-provided code must be passed separately to the page evaluator.
    Browser(T),
}

impl ScriptSource {
    /// Borrows the source while preserving its evaluation environment.
    #[must_use]
    pub fn as_ref(&self) -> ScriptSource<&str> {
        match self {
            Self::Page(source) => ScriptSource::Page(source),
            Self::Browser(source) => ScriptSource::Browser(source),
        }
    }
}

impl ScriptSource<&str> {
    /// Copies the source for transport while preserving its environment.
    #[must_use]
    pub fn into_owned(self) -> ScriptSource {
        match self {
            Self::Page(source) => ScriptSource::Page(source.to_owned()),
            Self::Browser(source) => ScriptSource::Browser(source.to_owned()),
        }
    }
}

/// Why a renderer API call was refused.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TabError {
    /// The URL string could not be parsed, joined, or was not `http`/`https`.
    InvalidUrl {
        /// The spec the caller passed.
        spec: String,
    },
    /// The renderer does not host the addressed frame.
    UnknownFrame {
        /// The frame id the caller passed.
        frame: u64,
    },
    /// `QuickJS` eval or a host callback failed.
    Script(ScriptFailure),
    /// The renderer is stopped.
    ActorStopped,
    /// The host could not start or reach a renderer.
    RendererUnavailable {
        /// Diagnostics from the spawn or handshake.
        message: String,
    },
    /// The browser refused work that would exceed a retained-resource budget.
    ResourceLimit {
        /// The exhausted budget.
        resource: ResourceLimit,
    },
    /// The screenshot render pipeline failed.
    Render {
        /// Diagnostics from style, layout, or paint.
        message: String,
    },
}

impl fmt::Display for TabError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUrl { spec } => write!(f, "invalid url: {spec}"),
            Self::UnknownFrame { frame } => write!(f, "unknown frame: {frame}"),
            Self::Script(failure) => write!(f, "script: {failure}"),
            Self::ActorStopped => f.write_str("renderer stopped"),
            Self::RendererUnavailable { message } => {
                write!(f, "renderer unavailable: {message}")
            }
            Self::ResourceLimit { resource } => write!(f, "resource limit reached: {resource}"),
            Self::Render { message } => write!(f, "render: {message}"),
        }
    }
}

impl std::error::Error for TabError {}

/// Browser-owned retained-resource budgets exposed through [`TabError`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResourceLimit {
    /// Concurrent waits retained by one tab coordinator.
    TabWaiters,
    /// Event subscriptions retained by one tab coordinator.
    TabSubscribers,
}

impl fmt::Display for ResourceLimit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TabWaiters => f.write_str("tab waiters"),
            Self::TabSubscribers => f.write_str("tab subscribers"),
        }
    }
}

/// Why `QuickJS` eval or a host callback failed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScriptFailure {
    /// Engine, parse, or thrown script error. `message` is diagnostics.
    Engine {
        /// Engine wording.
        message: Box<str>,
    },
    /// A timer slot id was not a valid array index.
    BadTimerId,
    /// The host was not present after construction. A defect if it surfaces.
    HostMissing,
    /// `QuickJS` interrupt handler stopped a long-running script.
    Interrupted,
}

impl fmt::Display for ScriptFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Engine { message } => f.write_str(message),
            Self::BadTimerId => f.write_str("bad timer id"),
            Self::HostMissing => f.write_str("js host missing"),
            Self::Interrupted => f.write_str("interrupted"),
        }
    }
}

impl std::error::Error for ScriptFailure {}

/// Whether a history API update replaces the current entry or pushes a new
/// one (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-history-pushstate>).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HistoryHandling {
    /// `history.pushState`: a new entry after the current one.
    Push,
    /// `history.replaceState`: the current entry in place.
    Replace,
}

/// Renderer outcomes, in the order the renderer ran them.
///
/// The browser wire carries only [`BrowserEvent`]: timer, fetch, and script
/// failures are renderer-side progress that no host acts on, so they never
/// cross the process boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RendererEvent {
    /// The document reached `readyState = "complete"` and dispatched `load`.
    Load,
    /// The document finished parsing and dispatched `DOMContentLoaded`
    /// (<https://html.spec.whatwg.org/multipage/parsing.html#the-end>);
    /// subresources may still be loading.
    DomContentLoaded,
    /// A frame committed a navigation to this URL. The actor re-announces the
    /// top-level document so a `CDP` client re-creates its execution contexts
    /// (<https://chromedevtools.github.io/devtools-protocol/tot/Page/#event-frameNavigated>).
    Navigated {
        /// The final URL after redirects.
        url: String,
    },
    /// A history API update in the active document (no new JS realm).
    HistoryUpdated {
        url: String,
        state: String,
        handling: HistoryHandling,
    },
    /// Request to traverse the tab's session history by `delta` entries.
    HistoryTraversal { delta: i32 },
    /// A host timer whose delay elapsed.
    Timer(u32),
    /// A `fetch` or navigation job finished with this HTTP status.
    Fetch {
        /// HTTP status.
        status: u16,
    },
    /// `send` or the body read failed.
    FetchFailed,
    /// A timer or `fetch` callback threw, or `execute_pending_job` failed.
    ScriptFailed,
}

impl RendererEvent {
    /// The event a host acts on, or `None` for renderer-internal progress.
    #[must_use]
    pub fn into_browser(self) -> Option<BrowserEvent> {
        Some(match self {
            Self::Load => BrowserEvent::Load,
            Self::DomContentLoaded => BrowserEvent::DomContentLoaded,
            Self::Navigated { url } => BrowserEvent::Navigated { url },
            Self::HistoryUpdated {
                url,
                state,
                handling,
            } => BrowserEvent::HistoryUpdated {
                url,
                state,
                handling,
            },
            Self::HistoryTraversal { delta } => BrowserEvent::HistoryTraversal { delta },
            Self::Timer(..) | Self::Fetch { .. } | Self::FetchFailed | Self::ScriptFailed => {
                return None;
            }
        })
    }
}

/// A renderer outcome the browser acts on. The IPC notice carries this type,
/// so the browser never receives an event it has no state for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BrowserEvent {
    /// The document reached `readyState = "complete"` and dispatched `load`.
    Load,
    /// The document finished parsing and dispatched `DOMContentLoaded`
    /// (<https://html.spec.whatwg.org/multipage/parsing.html#the-end>);
    /// subresources may still be loading.
    DomContentLoaded,
    /// A frame committed a navigation to this URL.
    Navigated {
        /// The final URL after redirects.
        url: String,
    },
    /// A history API update in the active document (no new JS realm).
    HistoryUpdated {
        url: String,
        state: String,
        handling: HistoryHandling,
    },
    /// Request to traverse the tab's session history by `delta` entries.
    HistoryTraversal { delta: i32 },
}

/// Which of the two storage areas a change belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StorageKind {
    /// `localStorage`: one area per origin in the profile.
    Local,
    /// `sessionStorage`: one area per origin in a top-level browsing context.
    Session,
}

impl StorageKind {
    /// The token the JavaScript shim uses to name the area.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Session => "session",
        }
    }
}

/// One top-level response: a complete [`ResponseHead`] plus its body. The
/// head travels in the embedded/wasm carrier path; the browser streams the
/// same head separately in its wire protocol and the body arrives in raw
/// bounded IPC frames.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mount {
    /// Response metadata installed before the body parses.
    pub head: ResponseHead,
    /// Raw document bytes; the renderer decodes them. It never travels in a
    /// control message, so it stays out of any serialized `Mount`.
    #[serde(skip, default)]
    pub body: Vec<u8>,
}

/// The document state a response installs: URL, HTTP status, response
/// headers that shape the document, and the browser-supplied page state.
///
/// Every response-install path builds one of these — the browser streams the
/// top-level response, and carriers, `data:`/`blob:` frames, and child-frame
/// commits construct it locally — so the renderer derives policy, MIME
/// guard, reporting endpoints, and status from one head, in one order, in
/// one place (`Document::install_head`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ResponseHead {
    /// Absolute document URL.
    pub url: String,
    /// Final HTTP status. Violation `status-code` reports it; documents
    /// without an HTTP response (blank, data) carry zero
    /// (<https://w3c.github.io/webappsec-csp/#create-violation-for-global>).
    pub status: u16,
    /// Every `Content-Type` response header value joined with `", "`
    /// (<https://fetch.spec.whatwg.org/#concept-header-list-get-decode-split>),
    /// when the document came from the network.
    pub content_type: Option<String>,
    /// Combined `Content-Security-Policy` header, when the response sent one.
    pub content_security_policy: Option<String>,
    /// Combined `Content-Security-Policy-Report-Only` header.
    pub content_security_policy_report_only: Option<String>,
    /// Combined `Reporting-Endpoints` header, when the response sent one.
    /// A violated policy's `report-to` directive resolves its endpoint names
    /// against it (<https://w3c.github.io/reporting/#header>).
    pub reporting_endpoints: Option<String>,
    /// `Referrer-Policy` response header, which seeds the document policy
    /// (<https://w3c.github.io/webappsec-referrer-policy/#referrer-policy-header>).
    pub referrer_policy: Option<String>,
    /// `X-Content-Type-Options: nosniff` on the response.
    pub no_sniff: bool,
    /// The body came from HTTP, so the Apache `text/plain` check applies.
    pub http_response: bool,
    /// Session history visible in the newly opened document.
    pub history: HistorySnapshot,
    /// Emulated viewport size to apply before the document lays out, when the
    /// browser has one.
    pub viewport: Option<(u32, u32)>,
    /// Page-scoped init scripts to register before the document's realm runs
    /// (<https://chromedevtools.github.io/devtools-protocol/tot/Page/#method-addScriptToEvaluateOnNewDocument>).
    pub init_scripts: Vec<(u64, String)>,
    /// The policy container a local-scheme response (`data:`, `blob:`)
    /// inherits from its initiator, already parsed
    /// (<https://html.spec.whatwg.org/multipage/browsers.html#determining-navigation-params-policy-container>).
    /// When set it replaces the head's CSP header derivation.
    pub inherited_policy: Option<crate::csp::List>,
}

impl ResponseHead {
    /// An HTTP response head derived from raw response headers: CSP
    /// directives, `Reporting-Endpoints`, and the nosniff flag all come from
    /// `headers`, so every summary path needs nothing but status, URL,
    /// `Content-Type`, and the pairs.
    ///
    /// History, viewport, and init scripts are caller state; they are
    /// pasted in before the head is installed.
    #[must_use]
    pub fn http(
        status: u16,
        final_url: impl Into<String>,
        content_type: Option<String>,
        headers: &[(String, String)],
    ) -> Self {
        Self {
            url: final_url.into(),
            status,
            content_type,
            content_security_policy: crate::csp::combined_header(
                headers,
                "content-security-policy",
            ),
            content_security_policy_report_only: crate::csp::combined_header(
                headers,
                "content-security-policy-report-only",
            ),
            reporting_endpoints: crate::csp::combined_header(headers, "reporting-endpoints"),
            referrer_policy: crate::csp::combined_header(headers, "referrer-policy"),
            no_sniff: crate::sniff::headers_nosniff(headers),
            http_response: true,
            history: HistorySnapshot::default(),
            viewport: None,
            init_scripts: Vec::new(),
            inherited_policy: None,
        }
    }

    /// The head of a response that is not an HTTP response: an inline
    /// `<!doctype html>` document, embedded/wasm carrier mounts, or a decoded
    /// `data:`/`blob:` URL. HTTP-derived state is empty and the document's
    /// own status reports zero; CSP still arrives through policy inheritance.
    #[must_use]
    pub fn non_http(final_url: impl Into<String>, content_type: Option<String>) -> Self {
        Self {
            url: final_url.into(),
            status: 0,
            content_type,
            content_security_policy: None,
            content_security_policy_report_only: None,
            reporting_endpoints: None,
            referrer_policy: None,
            no_sniff: false,
            http_response: false,
            history: HistorySnapshot::default(),
            viewport: None,
            init_scripts: Vec::new(),
            inherited_policy: None,
        }
    }
}

/// The browser's session history, as the renderer mirrors it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct HistorySnapshot {
    /// Total session-history entry count.
    pub length: usize,
    /// Zero-based position of the active entry, including any forward branch.
    pub index: usize,
    /// Serialized classic history API state of the active entry.
    pub state: Option<String>,
}

impl HistorySnapshot {
    /// Mirrors one same-document history update.
    ///
    /// The browser owns the entries; the renderer keeps this mirror because
    /// `history.length` and `history.state` are synchronous DOM reads, while
    /// the browser's authoritative update arrives later as a renderer event.
    /// The browser replaces this snapshot on mount and traversal, so the two
    /// only differ between a script's update and that event.
    pub(crate) fn apply(&mut self, state: String, handling: HistoryHandling) {
        self.state = Some(state);
        if matches!(handling, HistoryHandling::Push) {
            self.index = self.index.saturating_add(1);
            self.length = self.index.saturating_add(1);
        }
    }
}

/// Why a browser-service dial failed. Preserved across the renderer seam.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DialFailure {
    /// Host lookup failed.
    Dns,
    /// TCP or proxy connect failed.
    Connect,
    /// TLS handshake or certificate verification failed.
    Tls,
    /// A deadline expired.
    Timeout,
    /// A body or redirect cap was exceeded.
    Limit,
    /// A network permit was not available before the deadline.
    QueueFull,
    /// Navigation replacement, tab close, or renderer death cancelled the dial.
    Cancelled,
}

/// Why the renderer is dialing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DialKind {
    /// A JavaScript `fetch`.
    JsFetch,
    /// A classic `<script src>` load.
    ClassicScript,
    /// A JavaScript module graph fetch.
    ModuleScript,
    /// A `<link rel=stylesheet>` sheet, which delays the load event
    /// (<https://html.spec.whatwg.org/multipage/links.html#link-type-stylesheet>).
    Stylesheet,
    /// An `<img>` resource.
    Image,
    /// A child frame's navigation.
    FrameLoad,
    /// A legacy `report-uri` violation report POST. Fire-and-forget: the
    /// response is dropped and the dial is never CSP-gated, so a policy
    /// cannot silence its own reports
    /// (<https://w3c.github.io/webappsec-csp/#report-violation>).
    CspReport,
}

/// One screenshot request: viewport size plus an optional crop window.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScreenshotRequest {
    /// Capture surface width in device pixels: at least the clip extent, so a
    /// clip beyond the persistent viewport still rasterizes.
    pub viewport_width: u32,
    /// Capture surface height in device pixels.
    pub viewport_height: u32,
    /// Crop window in CSS pixels, when the caller wants less than the
    /// viewport (Playwright's `clip`).
    pub clip: Option<ScreenshotClip>,
}

/// A crop window for one screenshot.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScreenshotClip {
    /// Left edge in CSS pixels.
    pub x: f32,
    /// Top edge in CSS pixels.
    pub y: f32,
    /// Width in CSS pixels.
    pub width: f32,
    /// Height in CSS pixels.
    pub height: f32,
}

/// One blocking request the renderer asks the browser process to perform
/// (a GET for most dials; a navigation may carry a method and body).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DialRequest {
    /// Why the dial happens.
    pub kind: DialKind,
    /// Absolute request URL, canonically serialized: every queue site
    /// stringifies a parsed [`url::Url`], so the scheme is lowercase and a
    /// `scheme:` prefix test is exact.
    pub url: String,
    /// Document URL used as the `SameSite`/`Sec-Fetch-*` initiator.
    pub initiator: String,
    /// Read the response body.
    pub read_body: bool,
    /// HTTP method; defaults to `GET` for dials that predate bodies.
    #[serde(default = "default_dial_method")]
    pub method: String,
    /// Request body bytes, empty for `GET`.
    #[serde(default)]
    pub body: Vec<u8>,
    /// `Content-Type` header for `body`, when there is one.
    #[serde(default)]
    pub content_type: Option<String>,
    /// Author-supplied request headers, in insertion order.
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    /// Browser-generated `Referer` URL. `None` omits the header
    /// (<https://w3c.github.io/webappsec-referrer-policy/#determine-requests-referrer>).
    #[serde(default)]
    pub referrer: Option<String>,
    /// Caller deadline in milliseconds. `0` keeps the carrier timeout.
    /// A synchronous XHR uses its `timeout` attribute here
    /// (<https://xhr.spec.whatwg.org/#the-timeout-attribute>).
    #[serde(default)]
    pub timeout_ms: u64,
    /// Enforced CSP for this subresource dial. `None` skips the check, which
    /// is how a document navigation is sent. The network re-checks every
    /// redirect hop with it.
    #[serde(default)]
    pub csp: Option<Box<crate::RedirectGate>>,
    /// CORS protocol parameters for a page `fetch`/XHR. `None` for navigation
    /// and subresource dials.
    #[serde(default)]
    pub cors: Option<CorsRequest>,
    /// A `fetch(..., {keepalive: true})` request. The browser enforces the
    /// in-flight byte quota across realms
    /// (<https://fetch.spec.whatwg.org/#keepalive-flag>).
    #[serde(default)]
    pub keepalive: bool,
    /// Request-body length for the keepalive quota.
    #[serde(default)]
    pub body_len: u64,
    /// Subresource Integrity metadata; the browser checks the decoded body
    /// before returning it (<https://www.w3.org/TR/SRI/#verify-integrity>).
    #[serde(default)]
    pub integrity: Option<String>,
    /// Request cache mode
    /// (<https://fetch.spec.whatwg.org/#concept-request-cache-mode>).
    #[serde(default)]
    pub cache: Option<String>,
    /// Ask the browser to return headers first and stream the decoded body.
    #[serde(default)]
    pub stream_body: bool,
    /// Source document URL for recomputing the referrer on redirects.
    #[serde(default)]
    pub referrer_source: Option<String>,
    /// Referrer policy attached to the request.
    #[serde(default)]
    pub referrer_policy: Option<String>,
}

/// The CORS parameters a page `fetch`/XHR sends over the wire. The browser
/// fills in the origin from the document URL, so the renderer cannot claim
/// one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorsRequest {
    /// CORS mode (<https://fetch.spec.whatwg.org/#concept-request-mode>).
    pub mode: net::cors::RequestMode,
    /// Credentials mode
    /// (<https://fetch.spec.whatwg.org/#concept-request-credentials-mode>).
    pub credentials: net::cors::CredentialsMode,
    /// Redirect mode
    /// (<https://fetch.spec.whatwg.org/#concept-request-redirect-mode>).
    pub redirect: net::cors::RedirectMode,
}

fn default_dial_method() -> String {
    "GET".to_owned()
}

/// A response body that could not be decoded.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct BodyError;

/// Result of one blocking GET.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DialOutcome {
    /// HTTP status.
    pub status: u16,
    /// HTTP reason phrase, or `"OK"` for scheme fetches.
    pub status_text: String,
    /// Final URL after redirects.
    pub final_url: String,
    /// Every `Content-Type` response header value joined with `", "`, when
    /// the response sent any
    /// (<https://fetch.spec.whatwg.org/#concept-header-list-get-decode-split>).
    pub content_type: Option<String>,
    /// Response headers visible to page scripts, in wire order.
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    /// Response body, when [`DialRequest::read_body`]. `Err` means the body
    /// could not be decoded for its `Content-Encoding`; consuming it rejects
    /// instead of returning bytes
    /// (<https://fetch.spec.whatwg.org/#concept-http-network-fetch>).
    pub body: Result<Vec<u8>, BodyError>,
    /// How the response was filtered
    /// (<https://fetch.spec.whatwg.org/#concept-response-type>); `basic` for
    /// same-origin and subresource responses.
    #[serde(default)]
    pub response_type: net::cors::ResponseType,
    /// Redirect hops followed before this response.
    #[serde(default)]
    pub redirect_count: u32,
    /// Browser-side stream handle for a progressive body. The body is empty
    /// in the head outcome and chunks arrive through the network host.
    #[serde(default)]
    pub stream: Option<u64>,
}

impl DialOutcome {
    /// An unfiltered (`basic`) response, for carriers that do not run the CORS
    /// protocol.
    #[must_use]
    pub fn basic(
        status: u16,
        final_url: String,
        content_type: Option<String>,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) -> Self {
        Self {
            status,
            status_text: String::new(),
            final_url,
            content_type,
            headers,
            body: Ok(body),
            response_type: net::cors::ResponseType::Basic,
            redirect_count: 0,
            stream: None,
        }
    }
}

/// Completion for a dial submitted to the browser process.
pub type DialCompletion = Box<dyn FnOnce(Result<DialOutcome, DialFailure>) + Send + 'static>;

/// Cancels a submitted dial, including its redirect chain. Calling it still
/// delivers exactly one completion; dropping it leaves the request running.
pub type DialCancellation = Box<dyn FnOnce() + Send + 'static>;

/// Network effects the page engine asks its host to perform.
pub trait NetworkHost: Send + Sync + 'static {
    /// Submits one HTTP request without blocking the renderer thread and
    /// returns a handle that cancels it. The completion
    /// receives [`DialFailure`] for transport, timeout, queue, body-limit, or
    /// cancellation failure.
    /// Implementations must invoke it exactly once, including when submission
    /// is rejected.
    fn start_dial(&self, request: DialRequest, completion: DialCompletion) -> DialCancellation;

    /// Reads the next chunk of a streamed response body. `None` is the end of
    /// the body. Hosts without progressive bodies return a connect failure.
    ///
    /// # Errors
    ///
    /// The carrier's network failure or cancellation.
    fn dial_read(&self, _stream: u64) -> Result<Option<Vec<u8>>, DialFailure> {
        Err(DialFailure::Connect)
    }

    /// Cancels a streamed response body.
    fn cancel_stream(&self, _stream: u64) {}

    /// Performs `request` on the calling thread and returns when the response
    /// arrives. The browser process answers on the reader thread, so this does
    /// not wait on the renderer loop. Wasm cannot block inside a host callback,
    /// so the default is a connect failure.
    ///
    /// <https://xhr.spec.whatwg.org/#the-send()-method>
    ///
    /// # Errors
    ///
    /// [`DialFailure::Connect`] when this host cannot block, and whatever
    /// [`DialFailure`] the carrier reports for transport, timeout, or limits.
    fn dial_sync(&self, _request: DialRequest) -> Result<DialOutcome, DialFailure> {
        Err(DialFailure::Connect)
    }

    /// `document.cookie` getter for `url`.
    fn cookies_for(&self, url: &Url) -> String;

    /// `document.cookie` setter for `url`.
    fn set_cookie(&self, value: &str, url: &Url);
}

/// Web Storage effects the page engine asks its host to perform.
pub trait StorageHost: Send + Sync + 'static {
    /// `Storage.getItem(key)`
    /// (<https://html.spec.whatwg.org/multipage/webstorage.html#dom-storage-getitem>).
    fn storage_get(&self, kind: StorageKind, origin: &str, key: &str) -> Option<String>;

    /// The keys of `origin`'s local storage area, in the area's iteration
    /// order (<https://html.spec.whatwg.org/multipage/webstorage.html#dom-storage-key>).
    fn storage_keys(&self, kind: StorageKind, origin: &str) -> Vec<String>;

    /// `localStorage.setItem(key, value)`. `url` is the mutating document's
    /// URL and `source` its frame; the browser excludes it from the broadcast.
    ///
    /// # Errors
    ///
    /// [`StorageError::QuotaExceeded`] when the write would exceed the area's
    /// quota. `Ok(None)` means the value was unchanged.
    fn storage_set(
        &self,
        kind: StorageKind,
        origin: &str,
        url: &str,
        key: &str,
        value: &str,
        source: FrameId,
    ) -> Result<Option<StorageChange>, StorageError>;

    /// `localStorage.removeItem(key)`; `None` means the key was absent.
    fn storage_remove(
        &self,
        kind: StorageKind,
        origin: &str,
        url: &str,
        key: &str,
        source: FrameId,
    ) -> Option<StorageChange>;

    /// `localStorage.clear()`; `None` means the area was empty.
    fn storage_clear(
        &self,
        kind: StorageKind,
        origin: &str,
        url: &str,
        source: FrameId,
    ) -> Option<StorageChange>;
}

/// Browsing-context effects the page engine asks its host to perform.
pub trait BrowsingContextHost: Send + Sync + 'static {
    /// `window.open(url, target, features)`; `None` when the browser refused
    /// to open a window. `url` is absolute, or empty for `about:blank`. `seed`
    /// is the opener's session copy for the new tab, when there is one.
    fn window_open(&self, url: &str, name: &str, features: &str) -> Option<u64>;

    /// `window.close()` on a window this realm opened.
    fn window_close(&self, tab: u64);

    /// `window.opener` for this realm's tab; `None` when there is none.
    fn window_opener(&self) -> Option<u64>;

    /// `postMessage` to a window this realm opened, encoded by the caller's
    /// realm. `origin` is the sender's serialized origin and `target_origin`
    /// the normalized argument (`*`, `/`, or a parsed URL string); the target
    /// resolves both when the delivery task runs.
    fn window_post_message(&self, tab: u64, origin: &str, target_origin: &str, payload: &str);

    /// Reads one key from another tab's same-origin session area.
    fn remote_session_get(&self, tab: u64, origin: &str, key: &str) -> Option<String>;
}

/// Cross-context messaging effects the page engine asks its host to perform.
pub trait MessagingHost: Send + Sync + 'static {
    /// `BroadcastChannel.postMessage` for every same-origin channel.
    fn broadcast_post(&self, origin: &str, name: &str, payload: &str, channel: u64);
}

/// Complete host capability set used by an engine.
pub trait BrowserServices: NetworkHost + StorageHost + BrowsingContextHost + MessagingHost {}

impl<T> BrowserServices for T where
    T: NetworkHost + StorageHost + BrowsingContextHost + MessagingHost
{
}
