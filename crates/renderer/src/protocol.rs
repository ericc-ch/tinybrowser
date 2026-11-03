//! What a carrier exchanges with the engine, and the port it calls back on.
//!
//! These are the engine's own types: frame identity, mounts, page events,
//! dials, script results, and explicit errors. DOM handles, `QuickJS` values,
//! callbacks, and `net` types never cross. The messages that carry them over a
//! socket live in the browser crate's wire module, because only a carrier
//! needs to know how they travel.

use std::fmt;

use enumerated::Enumerated;
use serde::{Deserialize, Serialize};
use url::Url;

pub use webstorage::{STORAGE_QUOTA_BYTES, StorageChange, StorageError};

use net::cors::CrossOrigin;

/// The CORS settings state a request carried, as the policy the carrier sends
/// the dial under. This is the one translation from the element-level type to
/// the wire type; the browser process does not link the engine, so the wire
/// cannot carry `blitz_traits`'s
/// (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#cors-settings-attributes>).
pub(crate) fn cross_origin(setting: Option<blitz_traits::net::CorsSetting>) -> Option<CrossOrigin> {
    setting.map(|setting| match setting {
        blitz_traits::net::CorsSetting::Anonymous => CrossOrigin::Anonymous,
        blitz_traits::net::CorsSetting::UseCredentials => CrossOrigin::UseCredentials,
    })
}

/// The element's `referrerpolicy` state, as the policy the carrier applies to
/// the dial. Blitz parses and compares the attribute state; the carrier owns
/// the referrer algorithm, so this is the one keyword-to-keyword translation
/// (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#referrer-policy-attribute>).
pub(crate) fn referrer_policy(policy: blitz_traits::net::ReferrerPolicy) -> net::ReferrerPolicy {
    match policy {
        blitz_traits::net::ReferrerPolicy::NoReferrer => net::ReferrerPolicy::NoReferrer,
        blitz_traits::net::ReferrerPolicy::NoReferrerWhenDowngrade => {
            net::ReferrerPolicy::NoReferrerWhenDowngrade
        }
        blitz_traits::net::ReferrerPolicy::Origin => net::ReferrerPolicy::Origin,
        blitz_traits::net::ReferrerPolicy::OriginWhenCrossOrigin => {
            net::ReferrerPolicy::OriginWhenCrossOrigin
        }
        blitz_traits::net::ReferrerPolicy::SameOrigin => net::ReferrerPolicy::SameOrigin,
        blitz_traits::net::ReferrerPolicy::StrictOrigin => net::ReferrerPolicy::StrictOrigin,
        blitz_traits::net::ReferrerPolicy::StrictOriginWhenCrossOrigin => {
            net::ReferrerPolicy::StrictOriginWhenCrossOrigin
        }
        blitz_traits::net::ReferrerPolicy::UnsafeUrl => net::ReferrerPolicy::UnsafeUrl,
    }
}

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

    /// The area a shim token names, or `None` for anything else. The shim is
    /// the only caller, so an unknown token is an engine bug rather than page
    /// input.
    #[must_use]
    pub fn parse(token: &str) -> Option<Self> {
        match token {
            "local" => Some(Self::Local),
            "session" => Some(Self::Session),
            _ => None,
        }
    }
}

/// Declares the sandboxing flags: the variants, the full list, and the bit each
/// one owns. The variant list feeds all three, so a flag cannot be declared
/// without being set by a bare `sandbox`, which is what the oracle test checks
/// (<https://html.spec.whatwg.org/multipage/browsers.html#sandboxing-flag-set>).
macro_rules! sandbox_flags {
    ($($(#[$doc:meta])* $variant:ident),+ $(,)?) => {
        /// One flag in a sandboxing flag set.
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        #[repr(u8)]
        pub(crate) enum SandboxFlag {
            $($(#[$doc])* $variant),+
        }

        impl SandboxFlag {
            /// Every flag, in bit order, so `parse` and the tests walk one
            /// list.
            const ALL: &'static [Self] = &[$(Self::$variant),+];

            /// This flag's bit: its position in [`Self::ALL`].
            const fn bit(self) -> u32 {
                1 << (self as u32)
            }
        }
    };
}

sandbox_flags! {
    /// `iframe sandbox`: no navigating any other browsing context.
    Navigation,
    /// `allow-popups`: creating new auxiliary browsing contexts.
    AuxiliaryNavigation,
    /// `allow-top-navigation`: navigating the top-level context without
    /// transient activation.
    TopLevelNavigationWithoutUserActivation,
    /// `allow-top-navigation` or `allow-top-navigation-by-user-activation`:
    /// navigating the top-level context with transient activation.
    TopLevelNavigationWithUserActivation,
    /// `allow-same-origin`: an opaque origin, no cookies or web storage.
    Origin,
    /// `allow-forms`: form submission.
    Forms,
    /// `allow-pointer-lock`.
    PointerLock,
    /// `allow-scripts`: script execution, event handler content attributes,
    /// and `javascript:` navigations.
    Scripts,
    /// `allow-scripts`: features that trigger automatically, such as
    /// declarative refresh, autoplay, and autofocus.
    AutomaticFeatures,
    /// The `document.domain` setter is always blocked while sandboxed.
    DocumentDomain,
    /// `allow-popups-to-escape-sandbox`: the sandbox propagates into
    /// auxiliary contexts.
    PropagatesToAuxiliary,
    /// `allow-modals`.
    Modals,
    /// `allow-orientation-lock`.
    OrientationLock,
    /// `allow-presentation`.
    Presentation,
    /// `allow-downloads`.
    Downloads,
    /// `allow-top-navigation-to-custom-protocols`.
    CustomProtocolsNavigation,
}

/// A sandboxing flag set: the flags one `iframe` (and its ancestors) impose on
/// the documents it loads, parsed from the `sandbox` content attribute
/// (<https://html.spec.whatwg.org/multipage/browsers.html#sandboxing-flag-set>).
///
/// Every flag the standard defines is modelled, so a feature added later reads
/// the flag that gates it instead of counting as allowed. Only the flags this
/// engine currently has a gate for change observable behaviour.
///
/// A set has two sources: an `iframe sandbox` attribute, parsed with
/// [`Self::parse`], or the popup sandboxing flag set an opener's active set
/// becomes for the auxiliary context it creates
/// (<https://html.spec.whatwg.org/multipage/browsers.html#popup-sandboxing-flag-set>).
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SandboxFlags(u32);

impl SandboxFlags {
    /// Every flag, so a bare `sandbox` can start from the full set.
    const ALL: u32 = Self::bits(SandboxFlag::ALL);

    const fn bits(flags: &[SandboxFlag]) -> u32 {
        let mut bits = 0;
        let mut index = 0;
        while index < flags.len() {
            bits |= flags[index].bit();
            index += 1;
        }
        bits
    }

    /// The empty set: an unsandboxed document, and what an auxiliary context
    /// released by `allow-popups-to-escape-sandbox` starts with.
    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    /// The `sandbox` keywords and the flags each one clears. This is the only
    /// list: [`Self::parse`] reads it and [`Self::supports`] answers from it,
    /// so a keyword cannot be advertised without taking effect
    /// (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#attr-iframe-sandbox>).
    const TOKENS: &'static [(&'static str, &'static [SandboxFlag])] = &[
        (
            "allow-popups",
            &[
                SandboxFlag::AuxiliaryNavigation,
                SandboxFlag::CustomProtocolsNavigation,
            ],
        ),
        (
            "allow-top-navigation",
            &[
                SandboxFlag::TopLevelNavigationWithoutUserActivation,
                SandboxFlag::TopLevelNavigationWithUserActivation,
                SandboxFlag::CustomProtocolsNavigation,
            ],
        ),
        (
            "allow-top-navigation-by-user-activation",
            &[SandboxFlag::TopLevelNavigationWithUserActivation],
        ),
        ("allow-same-origin", &[SandboxFlag::Origin]),
        ("allow-forms", &[SandboxFlag::Forms]),
        ("allow-pointer-lock", &[SandboxFlag::PointerLock]),
        (
            "allow-scripts",
            &[SandboxFlag::Scripts, SandboxFlag::AutomaticFeatures],
        ),
        (
            "allow-popups-to-escape-sandbox",
            &[SandboxFlag::PropagatesToAuxiliary],
        ),
        ("allow-modals", &[SandboxFlag::Modals]),
        ("allow-orientation-lock", &[SandboxFlag::OrientationLock]),
        ("allow-presentation", &[SandboxFlag::Presentation]),
        ("allow-downloads", &[SandboxFlag::Downloads]),
        (
            "allow-top-navigation-to-custom-protocols",
            &[SandboxFlag::CustomProtocolsNavigation],
        ),
    ];

    /// Whether `token` is a supported `sandbox` keyword. The comparison is
    /// ASCII case-insensitive
    /// (<https://dom.spec.whatwg.org/#concept-domtokenlist-validation>).
    pub(crate) fn supports(token: &str) -> bool {
        Self::TOKENS
            .iter()
            .any(|(keyword, _)| keyword.eq_ignore_ascii_case(token))
    }

    /// Parses one `sandbox` attribute value, or returns the empty set when the
    /// attribute is absent
    /// (<https://html.spec.whatwg.org/multipage/browsers.html#parse-a-sandboxing-directive>).
    #[must_use]
    pub fn parse(attribute: Option<&str>) -> Self {
        let Some(attribute) = attribute else {
            return Self::default();
        };
        let mut flags = Self(Self::ALL);
        for token in attribute.split_ascii_whitespace() {
            if let Some((_, cleared)) = Self::TOKENS
                .iter()
                .find(|(keyword, _)| keyword.eq_ignore_ascii_case(token))
            {
                for flag in *cleared {
                    flags.0 &= !flag.bit();
                }
            }
        }
        flags
    }

    /// Whether `flag` is set.
    pub(crate) fn contains(self, flag: SandboxFlag) -> bool {
        self.0 & flag.bit() != 0
    }

    /// The union of two flag sets, which is how an `iframe`'s directive and the
    /// embedder's active set combine
    /// (<https://html.spec.whatwg.org/multipage/browsers.html#determining-the-creation-sandboxing-flags>).
    #[must_use]
    pub(crate) fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether scripts are blocked
    /// (<https://html.spec.whatwg.org/multipage/browsers.html#sandboxed-scripts-browsing-context-flag>).
    pub(crate) fn scripts_disabled(self) -> bool {
        self.contains(SandboxFlag::Scripts)
    }

    /// Whether this document gets a fresh opaque origin
    /// (<https://html.spec.whatwg.org/multipage/browsers.html#sandboxed-origin-browsing-context-flag>).
    #[must_use]
    pub fn origin_is_opaque(self) -> bool {
        self.contains(SandboxFlag::Origin)
    }

    /// The flags an auxiliary context this document opens inherits: the whole
    /// set while the sandbox propagates, nothing after
    /// `allow-popups-to-escape-sandbox`
    /// (<https://html.spec.whatwg.org/multipage/browsers.html#sandbox-propagates-to-auxiliary-browsing-contexts-flag>).
    pub(crate) fn auxiliary_flags(self) -> Self {
        if self.contains(SandboxFlag::PropagatesToAuxiliary) {
            self
        } else {
            Self::empty()
        }
    }
}

/// The browsing-context state a mount carries: the target name, the popup
/// sandboxing flag set, and whether the context keeps its name across origins.
///
/// All three belong to the context rather than to any one document, so they
/// travel with every mount: a renderer replacement must rebuild them, and a
/// document commit is when each one can change
/// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-name>,
/// <https://html.spec.whatwg.org/multipage/browsers.html#popup-sandboxing-flag-set>).
/// A carrier with no browser behind it, the WebAssembly component, sends the
/// default value: no name, no opener, and no sandbox.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabState {
    /// The tab's target name (`window.name`) as of this commit, after any
    /// reset the navigation performed
    /// (<https://html.spec.whatwg.org/multipage/browsing-the-web.html#resetBCName>).
    pub name: String,
    /// Whether this tab is an auxiliary browsing context whose opener
    /// browsing context is non-null, which keeps its target name when it
    /// navigates to another origin
    /// (<https://html.spec.whatwg.org/multipage/browsing-the-web.html#resetBCName>).
    pub auxiliary_with_opener: bool,
    /// The popup sandboxing flag set of the browsing context this mount
    /// installs into: empty for a tab web content did not open, and the
    /// opener's active flags for a popup that did not ask for
    /// `allow-popups-to-escape-sandbox`. The browser owns it and sends it with
    /// every commit, so no navigation of a sandboxed popup can install an
    /// unsandboxed document
    /// (<https://html.spec.whatwg.org/multipage/browsers.html#popup-sandboxing-flag-set>).
    pub popup_sandbox: SandboxFlags,
}

/// One top-level response: a complete [`ResponseHead`] plus its body. The
/// head travels in the embedded/wasm carrier path; the browser streams the
/// same head separately in its wire protocol and the body arrives in raw
/// bounded IPC frames.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mount {
    /// Response metadata installed before the body parses.
    pub head: ResponseHead,
    /// The browsing-context state the tab gives every document it installs.
    pub tab: TabState,
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
    /// Whether the document's environment is a secure context
    /// (<https://html.spec.whatwg.org/multipage/webappapis.html#secure-contexts>).
    ///
    /// The bit is part of the container because it inherits exactly like the
    /// rest of it: a local-scheme document clones its initiator's container, so
    /// the opener of a popup is its creator. Chromium carries the same field on
    /// `PolicyContainerPolicies` and computes it at navigation commit.
    pub secure_context: bool,
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
        /// there is one. `None` is a document with no creator, which is not the
        /// same as an insecure creator: its own secure-context bit then comes
        /// from its URL alone
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

    /// The policy container this document runs under: an HTTP response's
    /// headers, or the container a local-scheme document inherited.
    ///
    /// A document with no creator inherits the empty container, which has the
    /// same policy fields but no secure-context bit to inherit; the bit is read
    /// from the creator separately, because a missing creator is not an
    /// insecure one.
    #[must_use]
    pub fn policy(&self) -> &PolicyContainer {
        // A `static`, not a runtime default: `PolicyContainer` owns a `Vec`, so
        // a constant can live here but cannot be promoted to a borrowed
        // `'static` from a temporary.
        static EMPTY: PolicyContainer = PolicyContainer {
            csp: crate::csp::List::empty(),
            referrer_policy: None,
            secure_context: false,
        };
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
                        net::combined_header_in(headers, "content-security-policy").as_deref(),
                        net::combined_header_in(headers, "content-security-policy-report-only")
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
                    // Only the environment knows its creator, so the renderer
                    // computes the bit when it installs this head
                    // (<https://html.spec.whatwg.org/multipage/webappapis.html#secure-contexts>).
                    secure_context: false,
                },
                reporting_endpoints: net::combined_header_in(headers, "reporting-endpoints"),
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
    /// The origin the request runs under: the calling document's serialized
    /// origin, which the browser authorizes against the renderer's site lock
    /// before it becomes the request's `Origin`, `Sec-Fetch-*` source, and
    /// cookie `SameSite` input
    /// (<https://fetch.spec.whatwg.org/#concept-request-origin>). The claim is
    /// bounded by the site, not by the top-level document, because one renderer
    /// hosts every same-site frame (`Site::authorize_origin`); cookie
    /// `SameSite` only compares sites, so which same-site origin is named
    /// cannot change it.
    pub initiator: SerializedOrigin,
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
    /// their request adds. The initiator is the calling document's origin,
    /// captured as the serialized origin the browser authorizes.
    #[must_use]
    pub fn new(policy: RequestPolicy, url: &Url, initiator: &url::Origin) -> Self {
        Self {
            policy,
            url: url.clone(),
            initiator: SerializedOrigin::from(initiator),
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

enumerated::enumerated! {
    /// How a dial delivers its response body to the renderer.
    #[derive(serde::Serialize, serde::Deserialize)]
    pub BodyDelivery {
        /// Read the whole body before answering.
        Buffer => "buffer",
        /// Answer with the head and stream the body in chunks.
        Stream => "stream",
    }
}

impl BodyDelivery {
    /// Parses the `delivery` value `fetch.js` sends, which is exact-match: the
    /// one spelling of each delivery the renderer accepts, so a typo cannot
    /// become a silent buffer request.
    #[must_use]
    pub fn from_idl(value: &str) -> Option<Self> {
        Self::KEYWORDS
            .iter()
            .copied()
            .find(|delivery| delivery.keyword() == value)
    }
}

enumerated::enumerated! {
    /// A `fetch` request's cache mode
    /// (<https://fetch.spec.whatwg.org/#concept-request-cache-mode>). The
    /// renderer parses the `RequestInit.cache` string once; the browser matches
    /// on this instead of re-reading a string.
    #[derive(Default, serde::Serialize, serde::Deserialize)]
    pub RequestCache {
        /// Consult the HTTP cache, and revalidate a stale entry.
        #[default]
        Default => "default",
        /// Neither consult nor populate the HTTP cache.
        NoStore => "no-store",
        /// Bypass the HTTP cache for this request and update it.
        Reload => "reload",
        /// Bypass the HTTP cache only after revalidating a stored entry.
        NoCache => "no-cache",
        /// Use a stored entry even when it is stale.
        ForceCache => "force-cache",
        /// Fail when no stored entry exists.
        OnlyIfCached => "only-if-cached",
    }
}

impl RequestCache {
    /// Parses the IDL enum's string value, which is exact-match
    /// (<https://fetch.spec.whatwg.org/#requestinit>).
    #[must_use]
    pub fn from_idl(value: &str) -> Option<Self> {
        Self::KEYWORDS
            .iter()
            .copied()
            .find(|mode| mode.keyword() == value)
    }
}

enumerated::enumerated! {
    /// A page request's CORS mode, which is Fetch's request mode without
    /// `navigate`
    /// (<https://fetch.spec.whatwg.org/#concept-request-mode>).
    ///
    /// `navigate` exists for document navigations, and a page request cannot
    /// send under it: `RequestInit.mode` rejects the keyword, and a page
    /// `fetch` that did would skip the CORS check on a response the page can
    /// read. Keeping it out of the type makes that pairing unrepresentable here
    /// and across the wire.
    #[derive(serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "kebab-case")]
    pub CorsMode {
        /// No CORS: same-origin reads, opaque cross-origin reads.
        NoCors => "no-cors",
        /// A cross-origin request fails outright.
        SameOrigin => "same-origin",
        /// The full CORS protocol runs, and a cross-origin response is readable.
        Cors => "cors",
    }
}

impl CorsMode {
    /// Parses a `RequestInit.mode` string, which is exact-match
    /// (<https://fetch.spec.whatwg.org/#requestinit>).
    #[must_use]
    pub fn from_idl(value: &str) -> Option<Self> {
        Self::KEYWORDS
            .iter()
            .copied()
            .find(|mode| mode.keyword() == value)
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
    /// The response's URL after redirects. A filtered opaque response hides
    /// this from page script, but the engine still knows it: the empty URL
    /// list is the page-script view, not a property of the outcome
    /// (<https://fetch.spec.whatwg.org/#concept-filtered-response-opaque>).
    pub final_url: Url,
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
        final_url: Url,
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
    fn storage_get(&self, kind: StorageKind, origin: &StorageKey, key: &str) -> Option<String>;

    /// The keys of `origin`'s local storage area, in the area's iteration
    /// order (<https://html.spec.whatwg.org/multipage/webstorage.html#dom-storage-key>).
    fn storage_keys(&self, kind: StorageKind, origin: &StorageKey) -> Vec<String>;

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
        origin: &StorageKey,
        url: &str,
        key: &str,
        value: &str,
        source: FrameId,
    ) -> Result<Option<StorageChange>, StorageError>;

    /// `localStorage.removeItem(key)`; `None` means the key was absent.
    fn storage_remove(
        &self,
        kind: StorageKind,
        origin: &StorageKey,
        url: &str,
        key: &str,
        source: FrameId,
    ) -> Option<StorageChange>;

    /// `localStorage.clear()`; `None` means the area was empty.
    fn storage_clear(
        &self,
        kind: StorageKind,
        origin: &StorageKey,
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
    /// renderer-hosted auxiliary context resolves it from its creator, and a
    /// browser-hosted tab from the calling document the wire carries (#198).
    Keep,
    /// Drop the opener relationship and return no `WindowProxy`.
    NoOpener,
    /// [`NoOpener`](Self::NoOpener), and the navigation sends no referrer.
    NoReferrer,
}

/// One browsing-context target name, parsed once from the string a page
/// supplied
/// (<https://html.spec.whatwg.org/multipage/document-sequences.html#valid-browsing-context-name-or-keyword>).
///
/// Keywords match ASCII case-insensitively. Which spelling the empty string
/// is depends on the caller: choosing a navigable treats it as `_self`, while
/// the window open steps treat it as `_blank`
/// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#window-open-steps>),
/// which is why there are two constructors instead of one default.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TargetName {
    /// The empty string or `_self`.
    Current,
    /// `_parent`.
    Parent,
    /// `_top`.
    Top,
    /// `_blank`.
    Blank,
    /// A named context.
    Named(String),
}

impl TargetName {
    /// Parses a target for the rules for choosing a navigable, where the
    /// empty string selects the current context.
    #[must_use]
    pub fn parse(raw: &str) -> Self {
        if raw.is_empty() {
            Self::Current
        } else {
            Self::keyword(raw).unwrap_or_else(|| Self::Named(raw.to_owned()))
        }
    }

    /// Parses a target for the window open steps, where an absent target is
    /// `_blank`.
    #[must_use]
    pub fn for_window_open(raw: &str) -> Self {
        if raw.is_empty() {
            Self::Blank
        } else {
            Self::parse(raw)
        }
    }

    fn keyword(raw: &str) -> Option<Self> {
        if raw.eq_ignore_ascii_case("_self") {
            Some(Self::Current)
        } else if raw.eq_ignore_ascii_case("_parent") {
            Some(Self::Parent)
        } else if raw.eq_ignore_ascii_case("_top") {
            Some(Self::Top)
        } else if raw.eq_ignore_ascii_case("_blank") {
            Some(Self::Blank)
        } else {
            None
        }
    }

    /// Whether `noopener` withholds the `WindowProxy` for this target: the
    /// window open steps except `_self`, `_parent`, and `_top`
    /// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#window-open-steps>).
    #[must_use]
    pub fn noopener_withholds(&self) -> bool {
        matches!(self, Self::Blank | Self::Named(_))
    }

    /// The name a newly created context carries, when the target names one.
    #[must_use]
    pub fn new_context_name(&self) -> Option<&str> {
        match self {
            Self::Named(name) => Some(name),
            Self::Current | Self::Parent | Self::Top | Self::Blank => None,
        }
    }
}

/// The calling document's navigation inputs: the origin the request runs
/// under, its `"client"` referrer source (the document URL, walked out of an
/// `srcdoc` chain), and the policy that decides the `Referer` header.
///
/// The three travel together because every navigation seam needs all three,
/// and splitting them let each caller reassemble a different answer
/// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#window-open-steps>,
/// <https://w3c.github.io/webappsec-referrer-policy/#determine-requests-referrer>).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NavigationSource {
    /// The calling document's serialized origin; opaque for a sandboxed or
    /// non-network document.
    pub origin: SerializedOrigin,
    /// The `"client"` referrer source.
    pub referrer: Url,
    /// The concrete referrer policy the calling document determined.
    pub referrer_policy: net::ReferrerPolicy,
}

/// Browsing-context effects the page engine asks its host to perform.
pub trait BrowsingContextHost: Send + Sync + 'static {
    /// `window.open(url, target, features)`; `None` when the browser refused
    /// to open a window. `url` is absolute, or `None` when the call named no
    /// URL, which an existing context must not navigate and a new one starts
    /// on `about:blank` anyway.
    ///
    /// `source` is the calling document's origin, referrer source, and policy.
    /// `noreferrer` is not applied here: the browser folds it into the policy
    /// it receives, because it owns the feature's meaning. `allow_popups` is
    /// the calling document's sandbox bit; the browser enforces it too,
    /// because it is the trust boundary
    /// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#window-open-steps>,
    /// <https://w3c.github.io/webappsec-referrer-policy/#determine-requests-referrer>).
    fn window_open(
        &self,
        url: Option<&Url>,
        target: &TargetName,
        opener: OpenerPolicy,
        source: &NavigationSource,
        allow_popups: bool,
        popup_sandbox: SandboxFlags,
    ) -> Option<WindowId>;

    /// The browser tab this tab's browsing context group holds under `name`,
    /// if any
    /// (<https://html.spec.whatwg.org/multipage/document-sequences.html#the-rules-for-choosing-a-navigable>).
    fn window_by_name(&self, name: &str) -> Option<WindowId>;

    /// Records the target name of this realm's tab, after a script set
    /// `window.name` or a commit reset it. The browser keeps it because it
    /// searches target names across tabs and because the name outlives the
    /// renderer that reported it
    /// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-name>).
    fn set_window_name(&self, name: &str);

    /// Navigates a browser tab `target` already selected under `name`. The
    /// browser re-runs the lookup, so a renderer cannot navigate a tab outside
    /// its own browsing context group. A `None` `url` is the window open
    /// steps' null `urlRecord`, which navigates nothing.
    fn window_navigate(
        &self,
        window: WindowId,
        name: &str,
        url: Option<&Url>,
        opener: OpenerPolicy,
        source: &NavigationSource,
    );

    /// `window.close()` on a window this realm opened.
    fn window_close(&self, window: WindowId);

    /// `window.opener` for this realm's tab; `None` when there is none.
    fn window_opener(&self) -> Option<WindowId>;

    /// `postMessage` to a window this realm opened, encoded by the caller's
    /// realm. The target resolves the message when the delivery task runs.
    fn window_post_message(&self, window: WindowId, message: PostedMessage);

    /// Reads one key from another tab's same-origin session area.
    fn remote_session_get(
        &self,
        window: WindowId,
        origin: &StorageKey,
        key: &str,
    ) -> Option<String>;
}

/// Cross-context messaging effects the page engine asks its host to perform.
pub trait MessagingHost: Send + Sync + 'static {
    /// `BroadcastChannel.postMessage` for every same-origin channel.
    fn broadcast_post(&self, origin: &StorageKey, name: &str, payload: &str, channel: u64);
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

    /// Whether this origin has no tuple to authorize or fetch under.
    #[must_use]
    pub fn is_opaque(&self) -> bool {
        matches!(self, Self::Opaque)
    }

    /// The URL the request seams carry for this origin. Fetch's request
    /// initiator is a URL, and every reader takes only its origin; an opaque
    /// origin has none to carry
    /// (<https://fetch.spec.whatwg.org/#concept-request-origin>).
    #[must_use]
    pub fn to_url(&self) -> Option<Url> {
        match self {
            Self::Opaque => None,
            Self::Tuple(origin) => Url::parse(&origin.ascii_serialization()).ok(),
        }
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

/// The key of one origin's storage areas: a tuple origin, never opaque.
///
/// Storage exists only for tuple origins
/// (<https://html.spec.whatwg.org/multipage/webstorage.html#the-localstorage-attribute>).
/// Keying the areas by a serialized string let several spellings of one origin
/// (`https://example.test`, `https://example.test/path`,
/// `HTTPS://EXAMPLE.TEST`) create separate areas, and let `about:blank` stand
/// for every opaque document. Parsing normalizes a URL to its origin, so the
/// wire and the persisted profile stay strings while the map sees one value per
/// origin.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StorageKey(url::Origin);

impl StorageKey {
    /// The key for `origin`, or `None` when it is opaque.
    #[must_use]
    pub fn from_origin(origin: &url::Origin) -> Option<Self> {
        matches!(origin, url::Origin::Tuple(..)).then(|| Self(origin.clone()))
    }

    /// Reads one serialized origin; a full URL normalizes to its origin.
    #[must_use]
    pub fn parse(spec: &str) -> Option<Self> {
        match SerializedOrigin::parse(spec)? {
            SerializedOrigin::Tuple(origin) => Some(Self(origin)),
            SerializedOrigin::Opaque => None,
        }
    }

    /// The origin this key names.
    #[must_use]
    pub fn origin(&self) -> &url::Origin {
        &self.0
    }
}

impl std::fmt::Display for StorageKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0.ascii_serialization())
    }
}

impl Serialize for StorageKey {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for StorageKey {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).ok_or_else(|| serde::de::Error::custom("a storage key is a tuple origin"))
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

        let inherited =
            ResponseHead::local(url("about:blank"), None, Some(PolicyContainer::default()));
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

#[cfg(test)]
mod serialized_origin_tests {
    use super::SerializedOrigin;
    use url::Url;

    fn url(spec: &str) -> Url {
        Url::parse(spec).expect("URL")
    }

    /// A serialized origin is the claim itself, not a URL that happens to
    /// carry one: a path, query, or fragment is dropped, a blob URL names the
    /// tuple its entry wraps, and `null` stays opaque
    /// (<https://html.spec.whatwg.org/multipage/browsers.html#serialized-origins>,
    /// <https://fetch.spec.whatwg.org/#concept-request-origin>).
    #[test]
    fn a_serialized_origin_keeps_only_its_origin() {
        let tuple = SerializedOrigin::parse("https://a.test/x?q#f").expect("origin");
        assert_eq!(
            tuple.to_url(),
            Some(url("https://a.test/")),
            "the path and query do not survive"
        );
        assert!(!tuple.is_opaque());

        assert_eq!(
            SerializedOrigin::parse("blob:https://a.test/uuid").expect("origin"),
            SerializedOrigin::Tuple(url("blob:https://a.test/uuid").origin()),
            "a blob URL names the origin its entry was created in"
        );
        assert_eq!(
            SerializedOrigin::parse("null"),
            Some(SerializedOrigin::Opaque)
        );
        assert!(SerializedOrigin::parse("garbage").is_none());
        let opaque = SerializedOrigin::Opaque;
        assert!(opaque.is_opaque());
        assert_eq!(opaque.to_url(), None);
    }
}

#[cfg(test)]
mod target_name_tests {
    use super::TargetName;

    /// Choosing a navigable treats an empty target as `_self`; the window open
    /// steps treat it as `_blank`. Keywords match ASCII case-insensitively and
    /// every other name stays case-sensitive
    /// (<https://html.spec.whatwg.org/multipage/document-sequences.html#valid-browsing-context-name-or-keyword>,
    /// <https://html.spec.whatwg.org/multipage/nav-history-apis.html#window-open-steps>).
    #[test]
    fn a_target_name_parses_once_per_caller() {
        assert_eq!(TargetName::parse(""), TargetName::Current);
        assert_eq!(TargetName::for_window_open(""), TargetName::Blank);
        for spelling in ["_SELF", "_Self", "_self"] {
            assert_eq!(
                TargetName::parse(spelling),
                TargetName::Current,
                "{spelling}"
            );
        }
        for spelling in ["_PARENT", "_Top", "_blank"] {
            assert_eq!(
                TargetName::parse(spelling),
                match spelling.to_ascii_lowercase().as_str() {
                    "_parent" => TargetName::Parent,
                    "_top" => TargetName::Top,
                    _ => TargetName::Blank,
                },
                "{spelling}"
            );
        }
        // A missing underscore or an unknown keyword is a plain name, matched
        // case-sensitively.
        for name in ["blank", "_new", "_BLANKX", "frame", "_current"] {
            assert_eq!(
                TargetName::parse(name),
                TargetName::Named(name.to_owned()),
                "{name}"
            );
            assert_eq!(
                TargetName::for_window_open(name),
                TargetName::Named(name.to_owned()),
                "{name}"
            );
        }
    }

    /// `_blank` and a name create a context, so `noopener` withholds their
    /// `WindowProxy`; `_self`, `_parent`, and `_top` do not.
    /// Only a named target gives a new context a name: `_blank` leaves it
    /// empty
    /// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#window-open-steps>).
    #[test]
    fn only_a_named_or_new_target_carries_the_opener_rules() {
        assert!(TargetName::Blank.noopener_withholds());
        assert!(TargetName::Named("frame".to_owned()).noopener_withholds());
        for target in [TargetName::Current, TargetName::Parent, TargetName::Top] {
            assert!(!target.noopener_withholds(), "{target:?}");
            assert_eq!(target.new_context_name(), None, "{target:?}");
        }
        assert_eq!(TargetName::Blank.new_context_name(), None);
        assert_eq!(
            TargetName::Named("frame".to_owned()).new_context_name(),
            Some("frame")
        );
    }
}

#[cfg(test)]
mod sandbox_flag_tests {
    use super::{SandboxFlag, SandboxFlags};

    #[test]
    fn an_absent_attribute_sets_no_flags() {
        assert!(!SandboxFlags::parse(None).contains(SandboxFlag::Origin));
        assert!(!SandboxFlags::parse(None).scripts_disabled());
    }

    #[test]
    fn every_keyword_clears_exactly_the_flags_the_standard_names() {
        let expected: &[(&str, &[SandboxFlag])] = &[
            (
                "allow-popups",
                &[
                    SandboxFlag::AuxiliaryNavigation,
                    SandboxFlag::CustomProtocolsNavigation,
                ],
            ),
            (
                "allow-top-navigation",
                &[
                    SandboxFlag::TopLevelNavigationWithoutUserActivation,
                    SandboxFlag::TopLevelNavigationWithUserActivation,
                    SandboxFlag::CustomProtocolsNavigation,
                ],
            ),
            (
                "allow-top-navigation-by-user-activation",
                &[SandboxFlag::TopLevelNavigationWithUserActivation],
            ),
            ("allow-same-origin", &[SandboxFlag::Origin]),
            ("allow-forms", &[SandboxFlag::Forms]),
            ("allow-pointer-lock", &[SandboxFlag::PointerLock]),
            (
                "allow-scripts",
                &[SandboxFlag::Scripts, SandboxFlag::AutomaticFeatures],
            ),
            (
                "allow-popups-to-escape-sandbox",
                &[SandboxFlag::PropagatesToAuxiliary],
            ),
            ("allow-modals", &[SandboxFlag::Modals]),
            ("allow-orientation-lock", &[SandboxFlag::OrientationLock]),
            ("allow-presentation", &[SandboxFlag::Presentation]),
            ("allow-downloads", &[SandboxFlag::Downloads]),
            (
                "allow-top-navigation-to-custom-protocols",
                &[SandboxFlag::CustomProtocolsNavigation],
            ),
        ];
        for (keyword, cleared) in expected {
            // `sandbox` is a set of space-separated tokens that are ASCII
            // case-insensitive, so a keyword matches in either spelling
            // (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#attr-iframe-sandbox>).
            for spelling in [(*keyword).to_owned(), keyword.to_ascii_uppercase()] {
                let flags = SandboxFlags::parse(Some(&spelling));
                for flag in SandboxFlag::ALL {
                    assert_eq!(
                        flags.contains(*flag),
                        !cleared.contains(flag),
                        "{spelling}: flag {flag:?}"
                    );
                }
            }
        }
        // The table is the parser's own list, not a copy: a keyword added to
        // `TOKENS` without a row here would otherwise go unchecked, while
        // `supports` already advertises it.
        for (keyword, _) in SandboxFlags::TOKENS {
            assert!(
                expected.iter().any(|(listed, _)| listed == keyword),
                "{keyword} is supported but not in this table"
            );
        }
        // `ALL` is the only list `parse` folds, and every loop above reads it,
        // so a flag dropped from it would take itself out of the oracle. Check
        // the bare-sandbox set against this table instead: a clearable flag
        // missing from `ALL` is never set, and a new variant that forgot `ALL`
        // leaves the count short.
        assert_eq!(
            SandboxFlag::ALL.len(),
            16,
            "a new flag is added to `SandboxFlag::ALL`"
        );
        let bare = SandboxFlags::parse(Some(""));
        for (_, cleared) in expected {
            for flag in *cleared {
                assert!(bare.contains(*flag), "{flag:?} missing from a bare sandbox");
            }
        }
        assert!(
            bare.contains(SandboxFlag::Navigation) && bare.contains(SandboxFlag::DocumentDomain)
        );
        // Every flag is either cleared by some keyword or one of the two no
        // keyword may clear. A flag absent from this table is a flag the sandbox
        // would never lift, which no feature could notice.
        for flag in SandboxFlag::ALL {
            let clearable = expected.iter().any(|(_, flags)| flags.contains(flag));
            let always_set = matches!(flag, SandboxFlag::Navigation | SandboxFlag::DocumentDomain);
            assert!(
                clearable || always_set,
                "{flag:?} is neither clearable nor always set"
            );
        }
    }

    /// `parse` reads the whole attribute, so a sandbox with two keywords sets
    /// both halves
    /// (<https://html.spec.whatwg.org/multipage/browsers.html#parse-a-sandboxing-directive>).
    #[test]
    fn a_sandbox_with_two_keywords_clears_both() {
        let flags = SandboxFlags::parse(Some("allow-scripts allow-forms"));
        assert!(!flags.scripts_disabled());
        assert!(!flags.contains(SandboxFlag::Forms));
        assert!(
            flags.contains(SandboxFlag::Origin),
            "a keyword it does not name stays clear"
        );
    }

    #[test]
    fn supports_ignores_ascii_case_and_rejects_unlisted_keywords() {
        assert!(SandboxFlags::supports("allow-scripts"));
        assert!(SandboxFlags::supports("ALLOW-SCRIPTS"));
        assert!(SandboxFlags::supports("Allow-Same-Origin"));
        assert!(!SandboxFlags::supports("allow-frames"));
        assert!(!SandboxFlags::supports(
            "allow-storage-access-by-user-activation"
        ));
    }

    #[test]
    fn an_embedder_sandbox_is_unioned_into_the_child() {
        let parent = SandboxFlags::parse(Some("allow-scripts"));
        let child = SandboxFlags::parse(Some("allow-same-origin")).union(parent);
        assert!(child.scripts_disabled());
        assert!(child.origin_is_opaque());
    }

    #[test]
    fn sandbox_propagation_lets_an_escaping_popup_out() {
        let propagating = SandboxFlags::parse(Some("allow-popups"));
        assert!(propagating.auxiliary_flags().origin_is_opaque());
        let escaping = SandboxFlags::parse(Some("allow-popups-to-escape-sandbox"));
        assert_eq!(
            escaping.auxiliary_flags(),
            SandboxFlags::empty(),
            "an escaping popup starts unsandboxed"
        );
    }
}
