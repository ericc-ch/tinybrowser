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

use net::cors::CrossOrigin;

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

/// Browser-process identity of one top-level browsing context, as a renderer
/// holds it through a `WindowProxy`.
///
/// The value is unauthenticated: a renderer receives it over the wire and can
/// name any tab with it. Every browser-side use re-authorizes the target
/// against the opener graph (`close_related`/`message_related`), which is the
/// guard that makes the id safe to accept.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct WindowId(u64);

impl WindowId {
    /// Constructs a window id from a protocol integer.
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

/// Observable HTML-job outcomes, in the order the renderer ran them.
///
/// The IPC wire carries only [`BrowserEvent`]; timer, fetch, and script
/// failures are renderer-side progress that no host acts on, so they never
/// cross the process boundary.
/// Identity of one host timer in a document's timer table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TimerId(u64);

impl TimerId {
    pub(crate) fn take_next(&mut self) -> Self {
        let id = *self;
        self.0 += 1;
        id
    }

    /// The id as the WIT boundary spells it.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RendererEvent {
    /// An outcome the browser acts on.
    Browser(BrowserEvent),
    /// A host timer whose delay elapsed.
    Timer(TimerId),
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
        match self {
            Self::Browser(event) => Some(event),
            Self::Timer(..) | Self::Fetch { .. } | Self::FetchFailed | Self::ScriptFailed => None,
        }
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

/// The document's policy container: CSP list and referrer policy
/// (<https://html.spec.whatwg.org/multipage/browsers.html#policy-containers>).
///
/// A document carries one of these, and a local-scheme response inherits one
/// clone, so no load path can leave the two fields out of sync.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PolicyContainer {
    /// The content security policies that apply.
    pub csp: crate::csp::List,
    /// The document's referrer policy, when it has one.
    pub referrer_policy: Option<net::ReferrerPolicy>,
}

/// The document state a response installs: URL, content type, and the
/// browser-supplied page state.
///
/// Every response-install path builds one of these — the browser streams the
/// top-level response, and carriers, `data:`/`blob:` frames, and child-frame
/// commits construct it locally — so the renderer derives policy, MIME guard,
/// reporting endpoints, and status from one head, in one order, in one place
/// (`Document::install_head`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResponseHead {
    /// Absolute document URL.
    pub url: Url,
    /// Every `Content-Type` response header value joined with `", "`
    /// (<https://fetch.spec.whatwg.org/#concept-header-list-get-decode-split>),
    /// when the document came from the network.
    pub content_type: Option<String>,
    /// Session history visible in the newly opened document.
    pub history: HistorySnapshot,
    /// Emulated viewport size to apply before the document lays out, when the
    /// browser has one.
    pub viewport: Option<(u32, u32)>,
    /// Page-scoped init scripts to register before the document's realm runs
    /// (<https://chromedevtools.github.io/devtools-protocol/tot/Page/#method-addScriptToEvaluateOnNewDocument>).
    pub init_scripts: Vec<(u64, String)>,
    /// Whether this document came from an HTTP response or a local scheme.
    pub source: ResponseSource,
}

/// What produced one document. An HTTP response carries the status and the
/// headers that shape the document; a local-scheme response (`data:`,
/// `blob:`, a carrier mount, `about:blank`) carries only the policy container
/// it inherits from its initiator.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResponseSource {
    /// A document fetched over HTTP(S).
    Http(HttpHead),
    /// A document whose bytes never came over the network.
    Local {
        /// The policy container the response inherits from its initiator, when
        /// there is one
        /// (<https://html.spec.whatwg.org/multipage/browsers.html#determining-navigation-params-policy-container>).
        inherited: Option<PolicyContainer>,
    },
}

/// The response state only an HTTP response has, derived once from its
/// header list.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HttpHead {
    /// Final HTTP status. Violation `status-code` reports it
    /// (<https://w3c.github.io/webappsec-csp/#create-violation-for-global>).
    pub status: u16,
    /// The policy container the headers establish, parsed at the head.
    pub policy: PolicyContainer,
    /// Combined `Reporting-Endpoints` header, when the response sent one.
    /// A violated policy's `report-to` directive resolves its endpoint names
    /// against it (<https://w3c.github.io/reporting/#header>).
    pub reporting_endpoints: Option<String>,
    /// `Refresh` response header value, decoded as ISO-8859-1. The document
    /// creation algorithm runs the shared declarative refresh steps on it
    /// (<https://html.spec.whatwg.org/multipage/document-lifecycle.html#creating-and-initializing-a-document-object>).
    pub refresh: Option<String>,
    /// `X-Content-Type-Options: nosniff` on the response.
    pub no_sniff: bool,
}

impl ResponseSource {
    /// The HTTP status a violation reports; documents without an HTTP response
    /// carry zero
    /// (<https://w3c.github.io/webappsec-csp/#create-violation-for-global>).
    #[must_use]
    pub fn status(&self) -> u16 {
        match self {
            Self::Http(head) => head.status,
            Self::Local { .. } => 0,
        }
    }

    /// The policy container this document runs under.
    #[must_use]
    pub fn policy(&self) -> &PolicyContainer {
        static EMPTY: std::sync::LazyLock<PolicyContainer> =
            std::sync::LazyLock::new(PolicyContainer::default);
        match self {
            Self::Http(head) => &head.policy,
            Self::Local { inherited } => inherited.as_ref().unwrap_or(&EMPTY),
        }
    }

    /// The reporting endpoint list, when an HTTP response sent one.
    #[must_use]
    pub fn reporting_endpoints(&self) -> Option<&str> {
        match self {
            Self::Http(head) => head.reporting_endpoints.as_deref(),
            Self::Local { .. } => None,
        }
    }

    /// The declarative refresh value, when an HTTP response sent one.
    #[must_use]
    pub fn refresh(&self) -> Option<&str> {
        match self {
            Self::Http(head) => head.refresh.as_deref(),
            Self::Local { .. } => None,
        }
    }

    /// Whether the Apache `text/plain` check applies, which needs a real HTTP
    /// response behind the body.
    #[must_use]
    pub fn is_http(&self) -> bool {
        matches!(self, Self::Http(_))
    }

    /// Whether the response sent `X-Content-Type-Options: nosniff`.
    #[must_use]
    pub fn no_sniff(&self) -> bool {
        match self {
            Self::Http(head) => head.no_sniff,
            Self::Local { .. } => false,
        }
    }
}

impl ResponseHead {
    /// An HTTP response head derived from raw response headers: CSP
    /// directives, `Reporting-Endpoints`, and the nosniff flag all come from
    /// `headers`, so every summary path needs nothing but status, URL,
    /// `Content-Type`, and the pairs.
    ///
    /// History, viewport, and init scripts are caller state; they are set
    /// before the head is installed.
    #[must_use]
    pub fn http(
        status: u16,
        final_url: Url,
        content_type: Option<String>,
        headers: &[(String, String)],
    ) -> Self {
        Self {
            url: final_url,
            content_type,
            history: HistorySnapshot::default(),
            viewport: None,
            init_scripts: Vec::new(),
            source: ResponseSource::Http(HttpHead {
                status,
                policy: PolicyContainer {
                    csp: crate::csp::List::from_headers(
                        crate::csp::combined_header(headers, "content-security-policy").as_deref(),
                        crate::csp::combined_header(headers, "content-security-policy-report-only")
                            .as_deref(),
                    ),
                    // The policy parser needs the field values one by one:
                    // extraction fails as a whole when one value is malformed,
                    // which joining with ", " would hide
                    // (<https://w3c.github.io/webappsec-referrer-policy/#parse-referrer-policy-from-header>).
                    referrer_policy: net::ReferrerPolicy::from_header(
                        headers
                            .iter()
                            .filter(|(name, _)| name.eq_ignore_ascii_case("referrer-policy"))
                            .map(|(_, value)| value.as_str()),
                    ),
                },
                reporting_endpoints: crate::csp::combined_header(headers, "reporting-endpoints"),
                refresh: headers.iter().find_map(|(name, value)| {
                    name.eq_ignore_ascii_case("refresh").then(|| value.clone())
                }),
                no_sniff: crate::sniff::headers_nosniff(headers),
            }),
        }
    }

    /// The head of a response that is not an HTTP response: an inline
    /// `<!doctype html>` document, embedded/wasm carrier mounts, or a decoded
    /// `data:`/`blob:` URL. The document's status reports zero, and the policy
    /// container comes from `inherited` alone.
    #[must_use]
    pub fn local(
        final_url: Url,
        content_type: Option<String>,
        inherited: Option<PolicyContainer>,
    ) -> Self {
        Self {
            url: final_url,
            content_type,
            history: HistorySnapshot::default(),
            viewport: None,
            init_scripts: Vec::new(),
            source: ResponseSource::Local { inherited },
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
        match handling {
            HistoryHandling::Push => {
                self.index = self.index.saturating_add(1);
                self.length = self.index.saturating_add(1);
            }
            HistoryHandling::Replace => {}
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
    /// An `<img>` resource, or a CSS image such as `content: url()`.
    Image,
    /// A web font a CSS `@font-face` rule names.
    Font,
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

/// What the renderer knows about a request's referrer source before the
/// browser resolves it against the client environment the browser owns
/// (<https://fetch.spec.whatwg.org/#concept-request-referrer>).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReferrerRequest {
    /// No `Referer` at all.
    #[default]
    NoReferrer,
    /// The initiator document's URL, walked out of any `srcdoc` chain. The
    /// browser keeps it only when it is same-origin with the authorized
    /// initiator
    /// (<https://w3c.github.io/webappsec-referrer-policy/#determine-requests-referrer>).
    Client {
        /// The document URL the `"client"` branch reads.
        url: Url,
    },
    /// An explicit source URL: the document's referrer for a navigation, or a
    /// style sheet's URL for a CSS-initiated request. The browser authorizes
    /// it the same way it authorizes a navigation's referrer.
    Url(Url),
}

impl ReferrerRequest {
    /// The IDL spelling: `""`, `"about:client"`, or a URL
    /// (<https://fetch.spec.whatwg.org/#concept-request-referrer>).
    /// `client_url` is the document the `"client"` spelling resolves to.
    #[must_use]
    pub fn from_idl(value: &str, client_url: &Url) -> Option<Self> {
        if value.is_empty() {
            return Some(Self::NoReferrer);
        }
        if value.eq_ignore_ascii_case("about:client") {
            return Some(Self::Client {
                url: client_url.clone(),
            });
        }
        Url::parse(value).ok().map(Self::Url)
    }
}

/// One blocking request the renderer asks the browser process to perform
/// (a GET for most dials; a navigation may carry a method and body).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DialRequest {
    /// Why the dial happens, and the CORS parameters it is sent under.
    pub policy: RequestPolicy,
    /// Absolute request URL.
    pub url: Url,
    /// Document URL used as the `SameSite`/`Sec-Fetch-*` initiator.
    pub initiator: String,
    /// Whether and how the browser delivers the response body.
    pub delivery: BodyDelivery,
    /// HTTP method
    /// (<https://fetch.spec.whatwg.org/#concept-request-method>). The wire
    /// carries the spelling [`net::Method::parse`] accepts, so a malformed
    /// method is a decode error at the boundary, not a forged request later.
    pub method: net::Method,
    /// Request body bytes, empty for `GET`.
    pub body: Vec<u8>,
    /// `Content-Type` header for `body`, when there is one.
    pub content_type: Option<String>,
    /// Author-supplied request headers, in insertion order.
    pub headers: Vec<(String, String)>,
    /// Fetch's request referrer, before the browser resolves it against the
    /// client environment and verifies an explicit URL is authorized
    /// (<https://fetch.spec.whatwg.org/#concept-request-referrer>).
    pub referrer: ReferrerRequest,
    /// Caller deadline in milliseconds. `0` keeps the carrier timeout.
    /// A synchronous XHR uses its `timeout` attribute here
    /// (<https://xhr.spec.whatwg.org/#the-timeout-attribute>).
    pub timeout_ms: u64,
    /// Enforced CSP for this subresource dial. `None` skips the check, which
    /// is how a document navigation is sent. The network re-checks every
    /// redirect hop with it.
    pub csp: Option<Box<crate::RedirectGate>>,
    /// A `fetch(..., {keepalive: true})` request. The browser enforces the
    /// in-flight byte quota across realms
    /// (<https://fetch.spec.whatwg.org/#keepalive-flag>).
    pub keepalive: bool,
    /// Request-body length for the keepalive quota.
    pub body_len: u64,
    /// Subresource Integrity metadata; the browser checks the decoded body
    /// before returning it (<https://www.w3.org/TR/SRI/#verify-integrity>).
    pub integrity: Option<String>,
    /// Request cache mode
    /// (<https://fetch.spec.whatwg.org/#concept-request-cache-mode>).
    pub cache: RequestCache,
    /// The concrete policy main fetch determines under. The renderer resolves
    /// the element's attribute or the document's policy before queueing, so
    /// this is never unset
    /// (<https://fetch.spec.whatwg.org/#concept-request-referrer-policy>).
    pub referrer_policy: net::ReferrerPolicy,
}

impl DialRequest {
    /// A fresh GET dial: buffered body, no body bytes, no CSP gate, default
    /// cache mode, and no referrer. Callers with more to say set the fields
    /// their request adds.
    #[must_use]
    pub fn new(policy: RequestPolicy, url: &Url, initiator: &str) -> Self {
        Self {
            policy,
            url: url.clone(),
            initiator: initiator.to_owned(),
            delivery: BodyDelivery::Buffer,
            method: net::Method::GET,
            body: Vec::new(),
            content_type: None,
            headers: Vec::new(),
            referrer: ReferrerRequest::NoReferrer,
            timeout_ms: 0,
            csp: None,
            keepalive: false,
            body_len: 0,
            integrity: None,
            cache: RequestCache::Default,
            referrer_policy: net::ReferrerPolicy::default(),
        }
    }
}

/// How a dial delivers its response body to the renderer.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum BodyDelivery {
    /// Read the whole body before answering.
    Buffer,
    /// Answer with the head and stream the body in chunks.
    Stream,
}

impl BodyDelivery {
    /// Parses the `delivery` value `fetch.js` sends: the one spelling of each
    /// delivery the renderer accepts, so a typo cannot become a silent buffer
    /// request.
    #[must_use]
    pub fn from_idl(value: &str) -> Option<Self> {
        match value {
            "buffer" => Some(Self::Buffer),
            "stream" => Some(Self::Stream),
            _ => None,
        }
    }
}

/// A `fetch` request's cache mode
/// (<https://fetch.spec.whatwg.org/#concept-request-cache-mode>). The renderer
/// parses the `RequestInit.cache` string once; the browser matches on this
/// instead of re-reading a string.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum RequestCache {
    /// Consult the HTTP cache, and revalidate a stale entry.
    #[default]
    Default,
    /// Neither consult nor populate the HTTP cache.
    NoStore,
    /// Bypass the HTTP cache for this request and update it.
    Reload,
    /// Bypass the HTTP cache only after revalidating a stored entry.
    NoCache,
    /// Use a stored entry even when it is stale.
    ForceCache,
    /// Fail when no stored entry exists.
    OnlyIfCached,
}

impl RequestCache {
    /// Parses the IDL enum's string value
    /// (<https://fetch.spec.whatwg.org/#requestinit>).
    #[must_use]
    pub fn from_idl(value: &str) -> Option<Self> {
        match value {
            "default" => Some(Self::Default),
            "no-store" => Some(Self::NoStore),
            "reload" => Some(Self::Reload),
            "no-cache" => Some(Self::NoCache),
            "force-cache" => Some(Self::ForceCache),
            "only-if-cached" => Some(Self::OnlyIfCached),
            _ => None,
        }
    }
}

/// A page request's CORS mode, which is Fetch's request mode without
/// `navigate`
/// (<https://fetch.spec.whatwg.org/#concept-request-mode>).
///
/// `navigate` exists for document navigations, and a page request cannot send
/// under it: `RequestInit.mode` rejects the keyword, and a page `fetch` that
/// did would skip the CORS check on a response the page can read. Keeping it
/// out of the type makes that pairing unrepresentable here and across the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CorsMode {
    /// No CORS: same-origin reads, opaque cross-origin reads.
    NoCors,
    /// A cross-origin request fails outright.
    SameOrigin,
    /// The full CORS protocol runs, and a cross-origin response is readable.
    Cors,
}

impl CorsMode {
    /// Parses a `RequestInit.mode` string
    /// (<https://fetch.spec.whatwg.org/#requestinit>).
    #[must_use]
    pub fn from_idl(value: &str) -> Option<Self> {
        match value {
            "no-cors" => Some(Self::NoCors),
            "same-origin" => Some(Self::SameOrigin),
            "cors" => Some(Self::Cors),
            _ => None,
        }
    }

    /// The request mode this sends under.
    #[must_use]
    pub fn request_mode(self) -> net::cors::RequestMode {
        match self {
            Self::NoCors => net::cors::RequestMode::NoCors,
            Self::SameOrigin => net::cors::RequestMode::SameOrigin,
            Self::Cors => net::cors::RequestMode::Cors,
        }
    }
}

/// The CORS parameters a page `fetch` or XHR is sent under. The browser fills
/// in the origin from the initiator it authorized, so a dial cannot claim an
/// origin outside the renderer's site.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorsRequest {
    /// CORS mode (<https://fetch.spec.whatwg.org/#concept-request-mode>).
    pub mode: CorsMode,
    /// Credentials mode
    /// (<https://fetch.spec.whatwg.org/#concept-request-credentials-mode>).
    pub credentials: net::cors::CredentialsMode,
    /// Redirect mode
    /// (<https://fetch.spec.whatwg.org/#concept-request-redirect-mode>).
    pub redirect: net::cors::RedirectMode,
}

/// One dial's request policy: what the renderer is fetching, and the CORS
/// parameters that fetch is sent under
/// (<https://fetch.spec.whatwg.org/#concept-request-mode>).
///
/// Each variant is one fetch. The kind, mode, credentials mode, and redirect
/// mode all come from that variant's own rules, so a dial cannot report one
/// kind and go out under another fetch's mode. A navigation is the only dial
/// that sends `navigate`; a page request cannot express it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequestPolicy {
    /// A JavaScript `fetch` or `XMLHttpRequest`: the page's own modes, which
    /// the `Request` constructor already normalized.
    Fetch(CorsRequest),
    /// A classic `<script src>` load, with the element's `crossorigin` state
    /// (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#create-a-potential-cors-request>).
    ClassicScript { crossorigin: Option<CrossOrigin> },
    /// A module graph fetch: always `cors`, with the credentials mode the
    /// graph root's `crossorigin` attribute selects
    /// (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#cors-settings-attributes>).
    ///
    /// The graph root's element supplies the mode: the document records it as
    /// the current script while the graph loads, so every descendant fetch
    /// inherits it. A dynamic `import()` outside any script evaluation uses the
    /// document's default instead.
    ModuleScript { crossorigin: Option<CrossOrigin> },
    /// A style sheet: a `<link rel=stylesheet>`, a CSS `@import`, or a sheet a
    /// script loaded, with the element's `crossorigin` state
    /// (<https://html.spec.whatwg.org/multipage/semantics.html#create-a-link-request>).
    Stylesheet { crossorigin: Option<CrossOrigin> },
    /// An `<img>` resource or a CSS image, with the element's `crossorigin`
    /// state. A CSS image has no element and so no attribute.
    Image { crossorigin: Option<CrossOrigin> },
    /// A web font a CSS `@font-face` rule names: `cors`, same-origin
    /// credentials, so a cross-origin font needs an `Access-Control-Allow-Origin`
    /// (<https://drafts.csswg.org/css-fonts-4/#font-fetching-requirements>).
    Font,
    /// A child frame's navigation.
    FrameLoad,
    /// A Reporting API `csp-violation` delivery for a `report-to` endpoint:
    /// `cors` mode with same-origin credentials, so a cross-origin endpoint
    /// must allow the origin
    /// (<https://w3c.github.io/reporting/#try-delivery>).
    CspReportTo,
    /// The deprecated `report-uri` delivery: `no-cors` mode with same-origin
    /// credentials, and a redirect is an error. The response is ignored, and a
    /// preflight would make the report depend on an answer the collector never
    /// promised
    /// (<https://w3c.github.io/webappsec-csp/#report-violation>).
    ///
    /// Chromium sends the same request and refuses the redirect, so this is
    /// the browser behavior rather than the literal Fetch algorithm, which
    /// would reject a `no-cors` request whose redirect mode is not `follow`
    /// before sending it. The disagreement is recorded in `docs/upstream.md`.
    CspReportUri,
}

impl RequestPolicy {
    /// Why the dial happens.
    #[must_use]
    pub fn kind(self) -> DialKind {
        match self {
            Self::Fetch(_) => DialKind::JsFetch,
            Self::ClassicScript { .. } => DialKind::ClassicScript,
            Self::ModuleScript { .. } => DialKind::ModuleScript,
            Self::Stylesheet { .. } => DialKind::Stylesheet,
            Self::Image { .. } => DialKind::Image,
            Self::Font => DialKind::Font,
            Self::FrameLoad => DialKind::FrameLoad,
            Self::CspReportTo | Self::CspReportUri => DialKind::CspReport,
        }
    }

    /// The CORS parameters the dial is sent under. One match, one resolved
    /// triple: the browser cannot reinterpret the variant it was handed.
    #[must_use]
    pub fn cors(self) -> net::cors::CorsParameters {
        match self {
            Self::Fetch(cors) => net::cors::CorsParameters {
                mode: cors.mode.request_mode(),
                credentials: cors.credentials,
                redirect: cors.redirect,
            },
            Self::ClassicScript { crossorigin }
            | Self::Stylesheet { crossorigin }
            | Self::Image { crossorigin } => potential_cors(crossorigin),
            Self::ModuleScript { crossorigin } => {
                cors_protocol(cross_origin_credentials(crossorigin))
            }
            Self::Font | Self::CspReportTo => cors_protocol(net::cors::CredentialsMode::SameOrigin),
            Self::CspReportUri => net::cors::CorsParameters {
                mode: net::cors::RequestMode::NoCors,
                credentials: net::cors::CredentialsMode::SameOrigin,
                redirect: net::cors::RedirectMode::Error,
            },
            Self::FrameLoad => net::cors::CorsParameters {
                mode: net::cors::RequestMode::Navigate,
                credentials: net::cors::CredentialsMode::Include,
                redirect: net::cors::RedirectMode::Follow,
            },
        }
    }

    /// Whether the response goes back to page script.
    ///
    /// A `fetch` or XHR response is filtered for the page: an opaque one has no
    /// status, no URL, no headers, and no body
    /// (<https://fetch.spec.whatwg.org/#concept-filtered-response>). Every
    /// other dial's response is consumed by the engine itself, which reads the
    /// unsafe response — the status, URL, header list, and body the server
    /// sent (<https://fetch.spec.whatwg.org/#concept-internal-response>).
    #[must_use]
    pub fn exposes_response(self) -> bool {
        matches!(self, Self::Fetch(_))
    }
}

/// The parameters a `cors` request uses: CORS mode, the credentials mode its
/// caller names, and redirects followed.
fn cors_protocol(credentials: net::cors::CredentialsMode) -> net::cors::CorsParameters {
    net::cors::CorsParameters {
        mode: net::cors::RequestMode::Cors,
        credentials,
        redirect: net::cors::RedirectMode::Follow,
    }
}

/// The parameters a subresource inherits from its `crossorigin` state
/// (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#create-a-potential-cors-request>).
///
/// An absent attribute is the `No CORS` state: `no-cors` mode with credentials
/// included. A present value is `Anonymous` or `Use Credentials`: `cors` mode
/// with that state's credentials mode.
fn potential_cors(crossorigin: Option<CrossOrigin>) -> net::cors::CorsParameters {
    match crossorigin {
        None => net::cors::CorsParameters {
            mode: net::cors::RequestMode::NoCors,
            credentials: net::cors::CredentialsMode::Include,
            redirect: net::cors::RedirectMode::Follow,
        },
        Some(cross) => cors_protocol(cross.credentials()),
    }
}

/// The `CORS settings attribute credentials mode`: `same-origin` for the
/// `No CORS` and `Anonymous` states, `include` for `Use Credentials`
/// (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#cors-settings-attributes>).
fn cross_origin_credentials(crossorigin: Option<CrossOrigin>) -> net::cors::CredentialsMode {
    // A module fetch is always CORS, so an absent attribute means the
    // `same-origin` credentials of the `No CORS` and `Anonymous` states
    // (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#cors-settings-attributes>).
    crossorigin.map_or(
        net::cors::CredentialsMode::SameOrigin,
        CrossOrigin::credentials,
    )
}

/// Browser-side identity of one progressive response body.
///
/// Minted by the browser's stream counter. The renderer carries it back
/// unchanged, and the browser checks the calling assignment owns it before
/// reading or cancelling.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StreamId(u64);

impl StreamId {
    /// The stream handle's wire value, and the number `fetch.js` passes back
    /// to `__tbFetchRead` / `__tbFetchCancel`.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Mints an id from the browser's stream counter.
    #[must_use]
    pub const fn mint(id: u64) -> Self {
        Self(id)
    }

    /// Rebuilds the handle from the number a page script holds. The bridge is
    /// the only caller: the page never names a stream the browser did not
    /// mint, and the browser checks ownership before reading it.
    #[must_use]
    pub const fn from_page(id: u64) -> Self {
        Self(id)
    }
}

/// What one dial answered with, in the one shape the delivery mode decides
/// (<https://fetch.spec.whatwg.org/#concept-response-body>).
///
/// each state carries only its own data: a body is either the whole bytes, a
/// decode failure, or a handle to bytes that are still arriving. The old
/// `Result` inside a variant made "streamed with a decode error" spellable and
/// forced every caller to unwrap two layers.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ResponseBody {
    /// The whole body arrived.
    Bytes(Vec<u8>),
    /// The whole body arrived but its `Content-Encoding` decode failed
    /// (<https://fetch.spec.whatwg.org/#concept-http-network-fetch>).
    Undecodable,
    /// The head arrived and the bytes follow through the network host.
    Streamed(StreamId),
}

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
    /// The response header list this dial reports, in wire order: the
    /// filtered list for a page fetch ([`RequestPolicy::exposes_response`]),
    /// the unsafe list the server sent for every other dial, which the engine
    /// reads for MIME checks, a style sheet's referrer policy, and frame
    /// policy. `Set-Cookie` and `Set-Cookie2` are removed in both.
    pub headers: Vec<(String, String)>,
    /// How this dial delivers its response body.
    pub body: ResponseBody,
    /// How the response was filtered
    /// (<https://fetch.spec.whatwg.org/#concept-response-type>): `basic` for a
    /// same-origin response, `cors` for a cross-origin one that passed the CORS
    /// check, and `opaque` for a cross-origin `no-cors` response, whose
    /// subresource consumer still reads the unsafe body and headers.
    pub response_type: net::cors::ResponseType,
    /// Whether any redirect hop's URL origin differed from the request origin.
    /// A `no-cors` request that crosses an origin gets an opaque response and
    /// never returns to basic, so this is the taint a subresource keeps
    /// (<https://fetch.spec.whatwg.org/#concept-request-response-tainting>).
    pub crossed_origin: bool,
    /// Redirect hops followed before this response.
    pub redirect_count: u32,
    /// The stripped referrer the final hop was sent with, when it sent one.
    /// A navigated document reports it as `document.referrer`
    /// (<https://html.spec.whatwg.org/multipage/document-lifecycle.html#creating-and-initializing-a-document-object>).
    pub referrer: Option<Url>,
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
            body: ResponseBody::Bytes(body),
            response_type: net::cors::ResponseType::Basic,
            crossed_origin: false,
            redirect_count: 0,
            referrer: None,
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
    fn dial_read(&self, _stream: StreamId) -> Result<Option<Vec<u8>>, DialFailure> {
        Err(DialFailure::Connect)
    }

    /// Cancels a streamed response body.
    fn cancel_stream(&self, _stream: StreamId) {}

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

/// What `window.open`'s `noopener` and `noreferrer` features do to the
/// context it opens
/// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-open>).
///
/// `noreferrer` implies `noopener`, so the three states are exhaustive and no
/// caller can carry "noopener off, noreferrer on".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpenerPolicy {
    /// Keep the opener relationship. The referrer is not decided here: a
    /// renderer-hosted auxiliary context resolves it from its creator, while a
    /// browser-hosted tab currently starts from `about:blank` and sends none
    /// (#198).
    Keep,
    /// Drop the opener relationship and return no `WindowProxy`.
    NoOpener,
    /// [`NoOpener`](Self::NoOpener), and the navigation sends no referrer.
    NoReferrer,
}

impl OpenerPolicy {
    /// The token the page shim exchanges with the host. The shim only passes
    /// back a token [`Self::parse`] answered, so a rejected token means the
    /// shim and the engine disagree about the vocabulary.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Keep => "keep",
            Self::NoOpener => "noopener",
            Self::NoReferrer => "noreferrer",
        }
    }

    /// Reads one [`Self::token`].
    #[must_use]
    pub fn parse(token: &str) -> Option<Self> {
        match token {
            "keep" => Some(Self::Keep),
            "noopener" => Some(Self::NoOpener),
            "noreferrer" => Some(Self::NoReferrer),
            _ => None,
        }
    }
}

/// Browsing-context effects the page engine asks its host to perform.
pub trait BrowsingContextHost: Send + Sync + 'static {
    /// `window.open(url, target, features)`; `None` when the browser refused
    /// to open a window. `url` is absolute, or empty for `about:blank`.
    fn window_open(&self, url: &str, name: &str, opener: OpenerPolicy) -> Option<WindowId>;

    /// `window.close()` on a window this realm opened.
    fn window_close(&self, window: WindowId);

    /// `window.opener` for this realm's tab; `None` when there is none.
    fn window_opener(&self) -> Option<WindowId>;

    /// `postMessage` to a window this realm opened, encoded by the caller's
    /// realm. The target resolves the message when the delivery task runs.
    fn window_post_message(&self, window: WindowId, message: PostedMessage);

    /// Reads one key from another tab's same-origin session area.
    fn remote_session_get(&self, window: WindowId, origin: &str, key: &str) -> Option<String>;
}

/// Cross-context messaging effects the page engine asks its host to perform.
pub trait MessagingHost: Send + Sync + 'static {
    /// `BroadcastChannel.postMessage` for every same-origin channel.
    fn broadcast_post(&self, origin: &str, name: &str, payload: &str, channel: u64);
}

/// An origin as a cross-tab message carries it: a tuple origin, or the opaque
/// `"null"` serialization that every opaque document shares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SerializedOrigin {
    /// An opaque origin.
    Opaque,
    /// A tuple origin: scheme, host, and port.
    Tuple(url::Origin),
}

impl SerializedOrigin {
    /// Reads one serialized origin: `"null"`, or a URL whose origin is the
    /// value. A non-special URL (`data:`, `blob:`) has an opaque origin, so it
    /// names [`Opaque`](Self::Opaque) rather than a synthetic tuple.
    #[must_use]
    pub fn parse(spec: &str) -> Option<Self> {
        if spec == "null" {
            return Some(Self::Opaque);
        }
        Url::parse(spec).ok().map(|url| Self::from(&url.origin()))
    }
}

impl From<&url::Origin> for SerializedOrigin {
    fn from(origin: &url::Origin) -> Self {
        match origin {
            url::Origin::Opaque(_) => Self::Opaque,
            url::Origin::Tuple(..) => Self::Tuple(origin.clone()),
        }
    }
}

impl std::fmt::Display for SerializedOrigin {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Opaque => formatter.write_str("null"),
            Self::Tuple(origin) => formatter.write_str(&origin.ascii_serialization()),
        }
    }
}

impl Serialize for SerializedOrigin {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for SerializedOrigin {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).ok_or_else(|| serde::de::Error::custom("invalid origin"))
    }
}

/// The intended target origin one posted message carries, resolved at the
/// post-time boundary: `*` accepts any document, and an exact origin compares
/// with origin identity, so two opaque origins (`"null"` serialized) never
/// match each other
/// (<https://html.spec.whatwg.org/multipage/web-messaging.html#window-post-message-steps>).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TargetOrigin {
    /// `*`: any document may receive the message.
    Any,
    /// Only a document on this origin may receive it.
    Exact(url::Origin),
}

impl TargetOrigin {
    /// Resolves the sender's target-origin argument. `sender` is the posting
    /// document's origin when the caller knows its identity; `/` names it
    /// exactly
    /// (<https://html.spec.whatwg.org/multipage/web-messaging.html#window-post-message-steps>).
    #[must_use]
    pub fn parse(spec: &str, sender: Option<&url::Origin>) -> Option<Self> {
        match spec {
            "*" => Some(Self::Any),
            "/" => sender.map(|origin| Self::Exact(origin.clone())),
            other => Url::parse(other).ok().map(|url| Self::Exact(url.origin())),
        }
    }

    /// Whether a document with `origin` may receive the message.
    #[must_use]
    pub fn admits(&self, origin: &url::Origin) -> bool {
        match self {
            Self::Any => true,
            Self::Exact(target) => target == origin,
        }
    }
}

impl Serialize for TargetOrigin {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Any => serializer.serialize_str("*"),
            Self::Exact(origin) => serializer.serialize_str(&origin.ascii_serialization()),
        }
    }
}

impl<'de> Deserialize<'de> for TargetOrigin {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        match raw.as_str() {
            "*" => Ok(Self::Any),
            // `"null"` is the opaque origin of a URL like `about:blank`. It
            // cannot match another document, so it is a valid, never-delivered
            // target rather than a decode failure that would stop the read
            // loop.
            other => match SerializedOrigin::parse(other) {
                Some(SerializedOrigin::Opaque) => Ok(Self::Exact(url::Origin::new_opaque())),
                Some(SerializedOrigin::Tuple(origin)) => Ok(Self::Exact(origin)),
                None => Err(serde::de::Error::custom("invalid target origin")),
            },
        }
    }
}

/// One cross-context post, as it travels from the posting realm to the target
/// document. The browser authorizes `origin` before forwarding the rest
/// unchanged, so every hop carries the same value instead of rebuilding it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PostedMessage {
    /// The sender's origin at post time.
    pub origin: SerializedOrigin,
    /// The intended target, resolved where `/` still had the sender's origin
    /// identity.
    pub target_origin: TargetOrigin,
    /// `__tbEncode` payload from the sender's realm.
    pub payload: String,
}

/// Complete host capability set used by an engine.
pub trait BrowserServices: NetworkHost + StorageHost + BrowsingContextHost + MessagingHost {}

impl<T> BrowserServices for T where
    T: NetworkHost + StorageHost + BrowsingContextHost + MessagingHost
{
}

#[cfg(test)]
mod request_policy_tests {
    use super::{CorsMode, CorsRequest, RequestPolicy};
    use net::cors::{CredentialsMode, CrossOrigin, RedirectMode, RequestMode};

    fn fetch(mode: CorsMode) -> RequestPolicy {
        RequestPolicy::Fetch(CorsRequest {
            mode,
            credentials: CredentialsMode::SameOrigin,
            redirect: RedirectMode::Follow,
        })
    }

    /// Every policy resolved to the `(mode, credentials, redirect)` triple its
    /// own rules name. The table is the contract: a variant that changes one
    /// of them changes what reaches the wire.
    #[test]
    fn every_policy_resolves_to_the_parameters_its_rules_name() {
        for (policy, mode, credentials) in policy_table() {
            let resolved = policy.cors();
            assert_eq!(
                (resolved.mode, resolved.credentials, resolved.redirect),
                (mode, credentials, RedirectMode::Follow),
                "{:?}",
                policy.kind()
            );
        }
    }

    // One row per variant and per `crossorigin` state, which no smaller shape
    // states as clearly.
    #[allow(clippy::type_complexity, reason = "one row per policy state")]
    fn policy_table() -> [(RequestPolicy, RequestMode, CredentialsMode); 16] {
        let anonymous = Some(CrossOrigin::Anonymous);
        let credentials = Some(CrossOrigin::UseCredentials);
        [
            (
                fetch(CorsMode::NoCors),
                RequestMode::NoCors,
                CredentialsMode::SameOrigin,
            ),
            (
                fetch(CorsMode::Cors),
                RequestMode::Cors,
                CredentialsMode::SameOrigin,
            ),
            (
                fetch(CorsMode::SameOrigin),
                RequestMode::SameOrigin,
                CredentialsMode::SameOrigin,
            ),
            // The `No CORS` state: `no-cors` with credentials included
            // (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#create-a-potential-cors-request>).
            (
                RequestPolicy::ClassicScript { crossorigin: None },
                RequestMode::NoCors,
                CredentialsMode::Include,
            ),
            (
                RequestPolicy::ClassicScript {
                    crossorigin: anonymous,
                },
                RequestMode::Cors,
                CredentialsMode::SameOrigin,
            ),
            (
                RequestPolicy::ClassicScript {
                    crossorigin: credentials,
                },
                RequestMode::Cors,
                CredentialsMode::Include,
            ),
            (
                RequestPolicy::Stylesheet { crossorigin: None },
                RequestMode::NoCors,
                CredentialsMode::Include,
            ),
            (
                RequestPolicy::Image { crossorigin: None },
                RequestMode::NoCors,
                CredentialsMode::Include,
            ),
            (
                RequestPolicy::Image {
                    crossorigin: anonymous,
                },
                RequestMode::Cors,
                CredentialsMode::SameOrigin,
            ),
            (
                RequestPolicy::Image {
                    crossorigin: credentials,
                },
                RequestMode::Cors,
                CredentialsMode::Include,
            ),
            // A module graph is always `cors`; the attribute only selects the
            // CORS settings attribute credentials mode
            // (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#cors-settings-attributes>).
            (
                RequestPolicy::ModuleScript { crossorigin: None },
                RequestMode::Cors,
                CredentialsMode::SameOrigin,
            ),
            (
                RequestPolicy::ModuleScript {
                    crossorigin: anonymous,
                },
                RequestMode::Cors,
                CredentialsMode::SameOrigin,
            ),
            (
                RequestPolicy::ModuleScript {
                    crossorigin: credentials,
                },
                RequestMode::Cors,
                CredentialsMode::Include,
            ),
            // Fonts and a `report-to` delivery are `cors` with same-origin
            // credentials (<https://drafts.csswg.org/css-fonts-4/#font-fetching-requirements>,
            // <https://w3c.github.io/reporting/#try-delivery>).
            (
                RequestPolicy::Font,
                RequestMode::Cors,
                CredentialsMode::SameOrigin,
            ),
            (
                RequestPolicy::CspReportTo,
                RequestMode::Cors,
                CredentialsMode::SameOrigin,
            ),
            (
                RequestPolicy::FrameLoad,
                RequestMode::Navigate,
                CredentialsMode::Include,
            ),
        ]
    }

    /// The deprecated `report-uri` request is `no-cors`, so a collector that
    /// never promised CORS still receives the report, and a redirect is an
    /// error (<https://w3c.github.io/webappsec-csp/#report-violation>).
    #[test]
    fn a_report_uri_delivery_does_not_follow_redirects() {
        let resolved = RequestPolicy::CspReportUri.cors();
        assert_eq!(resolved.mode, RequestMode::NoCors);
        assert_eq!(resolved.credentials, CredentialsMode::SameOrigin);
        assert_eq!(resolved.redirect, RedirectMode::Error);
    }

    #[test]
    fn a_page_fetch_mode_cannot_be_navigate() {
        assert_eq!(CorsMode::from_idl("navigate"), None);
        assert_eq!(CorsMode::from_idl("no-cors"), Some(CorsMode::NoCors));
        assert_eq!(CorsMode::from_idl("cors"), Some(CorsMode::Cors));
        assert_eq!(
            CorsMode::from_idl("same-origin"),
            Some(CorsMode::SameOrigin)
        );
        assert_eq!(CorsMode::from_idl("cors "), None);
    }
}

#[cfg(test)]
mod response_head_tests {
    use super::{PolicyContainer, ResponseHead};
    use url::Url;

    fn url(spec: &str) -> Url {
        Url::parse(spec).expect("URL")
    }

    /// The variant owns what it means: a local response has no status, no
    /// refresh, no reporting endpoints, and no nosniff, so there is nothing to
    /// keep in sync with a sibling field.
    #[test]
    fn a_local_head_has_no_http_state() {
        let head = ResponseHead::local(
            url("data:text/html,x"),
            Some("text/html".to_owned()),
            Some(PolicyContainer {
                referrer_policy: Some(net::ReferrerPolicy::NoReferrer),
                ..PolicyContainer::default()
            }),
        );
        assert_eq!(head.source.status(), 0);
        assert_eq!(head.source.refresh(), None);
        assert_eq!(head.source.reporting_endpoints(), None);
        assert!(!head.source.no_sniff());
        assert!(!head.source.is_http());
        assert_eq!(
            head.source.policy().referrer_policy,
            Some(net::ReferrerPolicy::NoReferrer),
            "the inherited container is the only policy source"
        );

        let inherited = ResponseHead::local(url("about:blank"), None, None);
        assert_eq!(inherited.source.policy().referrer_policy, None);
    }

    /// An HTTP head parses its policy once, from the headers, and reports the
    /// HTTP-only state the variant owns.
    #[test]
    fn an_http_head_derives_its_policy_from_the_headers() {
        let headers = vec![
            (
                "Content-Security-Policy".to_owned(),
                "default-src 'none'".to_owned(),
            ),
            ("Referrer-Policy".to_owned(), "origin".to_owned()),
            ("X-Content-Type-Options".to_owned(), "nosniff".to_owned()),
            ("Refresh".to_owned(), "0; url=/next".to_owned()),
        ];
        let head = ResponseHead::http(
            204,
            url("https://example.test/"),
            Some("text/html".to_owned()),
            &headers,
        );
        assert_eq!(head.source.status(), 204);
        assert!(head.source.is_http());
        assert!(head.source.no_sniff());
        assert_eq!(head.source.refresh(), Some("0; url=/next"));
        assert_eq!(
            head.source.policy().referrer_policy,
            Some(net::ReferrerPolicy::Origin)
        );
        let document = url("https://example.test/");
        let gate = crate::csp::RedirectGate::new(
            head.source.policy().csp.clone(),
            &document.origin(),
            crate::csp::Destination::Script,
        );
        assert!(
            !gate.allows(&document, &url("https://example.test/app.js"), 0),
            "the header's policy is enforced, parsed at the head"
        );
    }
}
