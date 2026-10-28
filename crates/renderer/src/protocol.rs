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

use crate::dom::microsyntaxes::CrossOrigin;

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
    /// `Referrer-Policy` response header, parsed once at the head
    /// (<https://w3c.github.io/webappsec-referrer-policy/#parse-referrer-policy-from-header>).
    pub referrer_policy: Option<net::ReferrerPolicy>,
    /// `Refresh` response header value, decoded as ISO-8859-1. The document
    /// creation algorithm runs the shared declarative refresh steps on it
    /// (<https://html.spec.whatwg.org/multipage/document-lifecycle.html#creating-and-initializing-a-document-object>).
    pub refresh: Option<String>,
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
    /// The referrer policy a local-scheme response (`data:`, `blob:`)
    /// inherits from its initiator, installed with `inherited_policy`.
    pub inherited_referrer_policy: Option<net::ReferrerPolicy>,
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
            // The policy parser needs the field values one by one: extraction
            // fails as a whole when one value is malformed, which joining
            // with ", " would hide
            // (<https://w3c.github.io/webappsec-referrer-policy/#parse-referrer-policy-from-header>).
            referrer_policy: net::ReferrerPolicy::from_header(
                headers
                    .iter()
                    .filter(|(name, _)| name.eq_ignore_ascii_case("referrer-policy"))
                    .map(|(_, value)| value.as_str()),
            ),
            refresh: headers.iter().find_map(|(name, value)| {
                name.eq_ignore_ascii_case("refresh").then(|| value.clone())
            }),
            no_sniff: crate::sniff::headers_nosniff(headers),
            http_response: true,
            history: HistorySnapshot::default(),
            viewport: None,
            init_scripts: Vec::new(),
            inherited_policy: None,
            inherited_referrer_policy: None,
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
            refresh: None,
            no_sniff: false,
            http_response: false,
            history: HistorySnapshot::default(),
            viewport: None,
            init_scripts: Vec::new(),
            inherited_policy: None,
            inherited_referrer_policy: None,
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

/// One blocking request the renderer asks the browser process to perform
/// (a GET for most dials; a navigation may carry a method and body).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DialRequest {
    /// Why the dial happens, and the CORS parameters it is sent under.
    pub policy: RequestPolicy,
    /// Absolute request URL, canonically serialized: every queue site
    /// stringifies a parsed [`url::Url`], so the scheme is lowercase and a
    /// `scheme:` prefix test is exact.
    pub url: String,
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
    /// Fetch's request referrer: a client, an explicit URL, or no referrer.
    /// The browser resolves "client" against the authorized initiator and
    /// verifies an explicit URL is same-origin with it
    /// (<https://fetch.spec.whatwg.org/#concept-request-referrer>).
    pub referrer: net::Referrer,
    /// The document URL the `"client"` branch reads, walked out of `srcdoc`
    /// to the container document
    /// (<https://w3c.github.io/webappsec-referrer-policy/#determine-requests-referrer>).
    pub referrer_url: String,
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
        match net::cors::RequestMode::from_idl(value)? {
            net::cors::RequestMode::NoCors => Some(Self::NoCors),
            net::cors::RequestMode::SameOrigin => Some(Self::SameOrigin),
            net::cors::RequestMode::Cors => Some(Self::Cors),
            // `from_idl` never accepts `navigate`, and this type cannot hold it.
            net::cors::RequestMode::Navigate => None,
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

/// The CORS parameters a request sends over the wire. The browser fills in the
/// origin from the document URL, so the renderer cannot claim one.
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
/// The kind and the CORS parameters used to be two independent fields, and the
/// parameters were optional: an absent `cors` became *navigate* mode with
/// credentials included, which is how a plain `<img>` dial was sent as a
/// navigation. They are one value now. Each variant names one fetch, and the
/// mode, credentials mode, and redirect mode come from that variant's own
/// rules, so a kind and a mode cannot disagree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequestPolicy {
    /// A JavaScript `fetch` or `XMLHttpRequest`: the page's own modes, which
    /// the `Request` constructor already normalized.
    Fetch(CorsRequest),
    /// A classic `<script src>` load, with the element's `crossorigin` state
    /// (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#create-a-potential-cors-request>).
    ClassicScript { crossorigin: Option<CrossOrigin> },
    /// A module graph fetch: always `cors`, with the credentials mode the
    /// element's `crossorigin` attribute names
    /// (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#cors-settings-attributes>).
    ///
    /// The loader hands every module in a graph the same policy, because a
    /// nested import has no element of its own. `crossorigin` comes from the
    /// script that started the fetch, which covers a dynamic `import()` inside
    /// a classic script; the `<script type=module>` element that starts a graph
    /// is not tracked while the graph loads, so a `use-credentials` there still
    /// sends the default credentials mode.
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
    /// credentials. The response is ignored, and a preflight would make the
    /// report depend on an answer the collector never promised
    /// (<https://w3c.github.io/webappsec-csp/#report-violation>).
    ///
    /// CSP leaves the request's redirect mode at `"error"`; Fetch makes a
    /// `no-cors` request whose redirect mode is not `"follow"` a network error
    /// before it is sent, so this follows redirects instead
    /// (<https://fetch.spec.whatwg.org/#main-fetch>). The report still has no
    /// reader, so where it lands is unobservable.
    CspReportUri,
}

impl RequestPolicy {
    /// A JavaScript `fetch` or `XMLHttpRequest`.
    #[must_use]
    pub fn fetch(cors: CorsRequest) -> Self {
        Self::Fetch(cors)
    }

    /// A classic `<script src>` load.
    #[must_use]
    pub fn classic_script(crossorigin: Option<CrossOrigin>) -> Self {
        Self::ClassicScript { crossorigin }
    }

    /// A module graph fetch.
    #[must_use]
    pub fn module_script(crossorigin: Option<CrossOrigin>) -> Self {
        Self::ModuleScript { crossorigin }
    }

    /// A style sheet.
    #[must_use]
    pub fn stylesheet(crossorigin: Option<CrossOrigin>) -> Self {
        Self::Stylesheet { crossorigin }
    }

    /// An `<img>` resource or a CSS image.
    #[must_use]
    pub fn image(crossorigin: Option<CrossOrigin>) -> Self {
        Self::Image { crossorigin }
    }

    /// A web font a CSS `@font-face` rule names.
    #[must_use]
    pub fn font() -> Self {
        Self::Font
    }

    /// A child frame's navigation.
    #[must_use]
    pub fn frame_load() -> Self {
        Self::FrameLoad
    }

    /// A Reporting API `csp-violation` delivery for a `report-to` endpoint.
    #[must_use]
    pub fn csp_report_to() -> Self {
        Self::CspReportTo
    }

    /// The deprecated `report-uri` delivery.
    #[must_use]
    pub fn csp_report_uri() -> Self {
        Self::CspReportUri
    }

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

    /// The request mode the dial is sent under.
    #[must_use]
    pub fn mode(self) -> net::cors::RequestMode {
        self.cors().mode()
    }

    /// The credentials mode the dial is sent under.
    #[must_use]
    pub fn credentials(self) -> net::cors::CredentialsMode {
        self.cors().credentials()
    }

    /// The redirect mode the dial is sent under.
    #[must_use]
    pub fn redirect(self) -> net::cors::RedirectMode {
        self.cors().redirect()
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

    /// The CORS parameters one variant's fetch specifies.
    fn cors(self) -> CorsPolicy {
        match self {
            Self::Fetch(cors) => CorsPolicy::Request(cors),
            Self::ClassicScript { crossorigin }
            | Self::Stylesheet { crossorigin }
            | Self::Image { crossorigin } => subresource_cors(crossorigin),
            Self::ModuleScript { crossorigin } => {
                cors_protocol(cross_origin_credentials(crossorigin))
            }
            Self::Font | Self::CspReportTo => cors_protocol(net::cors::CredentialsMode::SameOrigin),
            Self::CspReportUri => CorsPolicy::Request(CorsRequest {
                mode: CorsMode::NoCors,
                credentials: net::cors::CredentialsMode::SameOrigin,
                redirect: net::cors::RedirectMode::Follow,
            }),
            Self::FrameLoad => CorsPolicy::Navigate,
        }
    }
}

/// The CORS parameters one dial is sent under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CorsPolicy {
    /// A navigation: no CORS protocol, credentials included, and the response
    /// is never filtered.
    Navigate,
    /// No CORS protocol, credentials included. A cross-origin response is
    /// opaque to page script, while the engine consumer (image decoder, script
    /// compiler, style sheet parser) still reads the unsafe response
    /// (<https://fetch.spec.whatwg.org/#concept-request-mode>).
    NoCors,
    /// The request's own mode, credentials, and redirect parameters.
    Request(CorsRequest),
}

impl CorsPolicy {
    fn mode(self) -> net::cors::RequestMode {
        match self {
            Self::Navigate => net::cors::RequestMode::Navigate,
            Self::NoCors => net::cors::RequestMode::NoCors,
            Self::Request(cors) => cors.mode.request_mode(),
        }
    }

    fn credentials(self) -> net::cors::CredentialsMode {
        match self {
            Self::Navigate | Self::NoCors => net::cors::CredentialsMode::Include,
            Self::Request(cors) => cors.credentials,
        }
    }

    fn redirect(self) -> net::cors::RedirectMode {
        match self {
            Self::Navigate | Self::NoCors => net::cors::RedirectMode::Follow,
            Self::Request(cors) => cors.redirect,
        }
    }
}

fn cors_protocol(credentials: net::cors::CredentialsMode) -> CorsPolicy {
    CorsPolicy::Request(CorsRequest {
        mode: CorsMode::Cors,
        credentials,
        redirect: net::cors::RedirectMode::Follow,
    })
}

/// The CORS parameters a subresource request goes out with, from the element's
/// `crossorigin` state
/// (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#create-a-potential-cors-request>).
///
/// An absent attribute is the `No CORS` state: `no-cors` mode with credentials
/// included. Any value is `Anonymous` or `Use Credentials`: `cors` mode with
/// that state's credentials mode. An empty or unknown value already parsed to
/// `Anonymous` (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#cors-settings-attributes>).
fn subresource_cors(crossorigin: Option<CrossOrigin>) -> CorsPolicy {
    match crossorigin {
        None => CorsPolicy::NoCors,
        Some(cross) => cors_protocol(cross.credentials()),
    }
}

/// The `CORS settings attribute credentials mode`: `same-origin` for the
/// `No CORS` and `Anonymous` states, `include` for `Use Credentials`
/// (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#cors-settings-attributes>).
fn cross_origin_credentials(crossorigin: Option<CrossOrigin>) -> net::cors::CredentialsMode {
    if crossorigin == Some(CrossOrigin::UseCredentials) {
        net::cors::CredentialsMode::Include
    } else {
        net::cors::CredentialsMode::SameOrigin
    }
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
    pub referrer: Option<String>,
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
    /// realm. The target resolves the message when the delivery task runs.
    fn window_post_message(&self, tab: u64, message: PostedMessage);

    /// Reads one key from another tab's same-origin session area.
    fn remote_session_get(&self, tab: u64, origin: &str, key: &str) -> Option<String>;
}

/// Cross-context messaging effects the page engine asks its host to perform.
pub trait MessagingHost: Send + Sync + 'static {
    /// `BroadcastChannel.postMessage` for every same-origin channel.
    fn broadcast_post(&self, origin: &str, name: &str, payload: &str, channel: u64);
}

/// One cross-context post, as it travels from the posting realm to the target
/// document. The browser authorizes `origin` before forwarding the rest
/// unchanged, so every hop carries the same value instead of rebuilding it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PostedMessage {
    /// The sender's serialized origin at post time.
    pub origin: String,
    /// The normalized target origin: `*`, `/`, or a serialized origin.
    pub target_origin: String,
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
    use super::RequestPolicy;
    use crate::dom::microsyntaxes::CrossOrigin;
    use net::cors::{CredentialsMode, RedirectMode, RequestMode};

    fn mode_of(policy: RequestPolicy) -> RequestMode {
        policy.mode()
    }

    fn credentials_of(policy: RequestPolicy) -> CredentialsMode {
        policy.credentials()
    }

    #[test]
    fn a_subresource_without_crossorigin_is_no_cors_with_credentials() {
        // The `No CORS` state: `no-cors` mode with credentials included
        // (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#create-a-potential-cors-request>).
        for policy in [
            RequestPolicy::classic_script(None),
            RequestPolicy::stylesheet(None),
            RequestPolicy::image(None),
        ] {
            assert_eq!(mode_of(policy), RequestMode::NoCors);
            assert_eq!(credentials_of(policy), CredentialsMode::Include);
            assert_eq!(policy.redirect(), RedirectMode::Follow);
        }
    }

    #[test]
    fn an_anonymous_crossorigin_sends_cors_without_credentials() {
        for policy in [
            RequestPolicy::classic_script(Some(CrossOrigin::Anonymous)),
            RequestPolicy::stylesheet(Some(CrossOrigin::Anonymous)),
            RequestPolicy::image(Some(CrossOrigin::Anonymous)),
        ] {
            assert_eq!(mode_of(policy), RequestMode::Cors);
            assert_eq!(credentials_of(policy), CredentialsMode::SameOrigin);
        }
    }

    #[test]
    fn use_credentials_sends_cors_with_credentials() {
        for policy in [
            RequestPolicy::classic_script(Some(CrossOrigin::UseCredentials)),
            RequestPolicy::stylesheet(Some(CrossOrigin::UseCredentials)),
            RequestPolicy::image(Some(CrossOrigin::UseCredentials)),
        ] {
            assert_eq!(mode_of(policy), RequestMode::Cors);
            assert_eq!(credentials_of(policy), CredentialsMode::Include);
        }
    }

    #[test]
    fn module_scripts_are_cors_with_the_attributes_credentials_mode() {
        // The CORS settings attribute credentials mode: `same-origin` for
        // `No CORS` and `Anonymous`, `include` for `Use Credentials`
        // (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#cors-settings-attributes>).
        for (crossorigin, expected) in [
            (None, CredentialsMode::SameOrigin),
            (Some(CrossOrigin::Anonymous), CredentialsMode::SameOrigin),
            (Some(CrossOrigin::UseCredentials), CredentialsMode::Include),
        ] {
            let policy = RequestPolicy::module_script(crossorigin);
            assert_eq!(mode_of(policy), RequestMode::Cors);
            assert_eq!(credentials_of(policy), expected);
        }
    }

    #[test]
    fn fonts_reports_and_frames_take_the_modes_their_specs_name() {
        let font = RequestPolicy::font();
        assert_eq!(mode_of(font), RequestMode::Cors);
        assert_eq!(credentials_of(font), CredentialsMode::SameOrigin);

        // A `report-to` delivery is a `cors` request with same-origin
        // credentials (<https://w3c.github.io/reporting/#try-delivery>).
        let report_to = RequestPolicy::csp_report_to();
        assert_eq!(mode_of(report_to), RequestMode::Cors);
        assert_eq!(credentials_of(report_to), CredentialsMode::SameOrigin);

        // The deprecated `report-uri` delivery is `no-cors`, so a collector
        // that never promised CORS still receives the report
        // (<https://w3c.github.io/webappsec-csp/#report-violation>).
        let report_uri = RequestPolicy::csp_report_uri();
        assert_eq!(mode_of(report_uri), RequestMode::NoCors);
        assert_eq!(credentials_of(report_uri), CredentialsMode::SameOrigin);
        assert_eq!(report_uri.redirect(), RedirectMode::Follow);

        let frame = RequestPolicy::frame_load();
        assert_eq!(mode_of(frame), RequestMode::Navigate);
        assert_eq!(credentials_of(frame), CredentialsMode::Include);
    }

    #[test]
    fn only_a_page_fetch_exposes_its_response_to_script() {
        assert!(
            RequestPolicy::fetch(super::CorsRequest {
                mode: super::CorsMode::Cors,
                credentials: CredentialsMode::SameOrigin,
                redirect: RedirectMode::Follow,
            })
            .exposes_response()
        );
        for policy in [
            RequestPolicy::classic_script(None),
            RequestPolicy::module_script(None),
            RequestPolicy::stylesheet(None),
            RequestPolicy::image(None),
            RequestPolicy::font(),
            RequestPolicy::frame_load(),
            RequestPolicy::csp_report_to(),
            RequestPolicy::csp_report_uri(),
        ] {
            assert!(
                !policy.exposes_response(),
                "{:?} is consumed by the engine",
                policy.kind()
            );
        }
    }

    #[test]
    fn a_policy_reports_the_kind_its_constructor_names() {
        use super::DialKind;
        assert_eq!(
            RequestPolicy::image(None).kind(),
            DialKind::Image,
            "an image dial cannot report another fetch's kind"
        );
        assert_eq!(
            RequestPolicy::classic_script(None).kind(),
            DialKind::ClassicScript
        );
        assert_eq!(RequestPolicy::frame_load().kind(), DialKind::FrameLoad);
    }

    #[test]
    fn a_page_fetch_mode_cannot_be_navigate() {
        // `navigate` is not a `CorsMode`, so a page fetch cannot be sent under
        // it; the IDL parser rejects the keyword for the same reason
        // (<https://fetch.spec.whatwg.org/#dom-request>).
        assert_eq!(super::CorsMode::from_idl("navigate"), None);
        assert_eq!(
            super::CorsMode::from_idl("no-cors"),
            Some(super::CorsMode::NoCors)
        );
        assert_eq!(
            super::CorsMode::from_idl("cors"),
            Some(super::CorsMode::Cors)
        );
    }
}
