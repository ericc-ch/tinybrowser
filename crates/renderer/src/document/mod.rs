//! One document: HTML tasks we own, browser services for dials and cookies. The
//! renderer loop owns every wait; the browser process owns the tab and drives
//! navigation.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant as WallClock;

use tokio::sync::Notify;

use blitz_traits::net::NetHandler;
use tokio::time::Instant;
use url::Url;

use crate::Parsed;
use crate::ReadyState;
use crate::js::{
    DocumentStreamCommand, FrameNavigation, FrameRuntime, JsRuntimeHandle, PolicyContainer, World,
};
use crate::messaging::SharedHandle;
use crate::protocol::{
    BrowserServices, DialOutcome, DialRequest, FrameId, Mount, RendererEvent, ResponseHead,
    ScriptFailure, TabError,
};

use self::dials::DialLedger;
use self::load::{LoadDelay, LoadDelayGuard};

pub(crate) mod dial;
mod dials;
mod drain;

pub(crate) use drain::DrainStep;

mod intern;
mod load;

pub(crate) use crate::js::ScriptValue;

/// A child frame document this document created for the engine to adopt:
/// frame, `iframe` container, and the document itself.
pub(crate) type CreatedFrame = (FrameId, crate::dom::tree::NodeId, Document);

/// Concurrent `fetch` jobs one document may keep in flight.
const MAX_PENDING_JS_FETCHES: usize = 256;
/// Retained events before a document reports overflow.
const MAX_PENDING_EVENTS: usize = 2048;

enum Task {
    Timer(u32),
    /// Boxed: a completed dial carries the request and response with it, and
    /// the variant otherwise dominates every other task's size.
    DialFinished(u64, Box<CompletedDial>),
    DialFailed(u64, DialContext),
    WindowMessage(WindowMessage),
    StorageEvent(crate::storage::PendingStorageEvent),
    /// One cross-tab `message` payload from another renderer.
    RemoteMessage(String),
    /// One `BroadcastChannel` message for this frame's channels.
    BroadcastMessage {
        origin: String,
        name: String,
        payload: String,
        source: Option<u64>,
    },
    PortMessage {
        endpoint: u64,
        payload: String,
        ports: Vec<u64>,
    },
    PortClosed {
        endpoint: u64,
    },
    /// A subresource event for one element, queued so a listener attached
    /// later in the same script still sees it: `update the image data` and the
    /// stylesheet fetch steps queue a task rather than firing inside the
    /// mutation that started the request
    /// (<https://html.spec.whatwg.org/multipage/images.html#updating-the-image-data>,
    /// <https://html.spec.whatwg.org/multipage/semantics.html#the-link-element>).
    NodeLoad(crate::dom::tree::NodeId),
    NodeError(crate::dom::tree::NodeId),
}

/// One posted window message, ready for the target realm's task queue.
pub(crate) struct WindowMessage {
    pub(crate) source: FrameId,
    pub(crate) origin: String,
    pub(crate) payload: String,
    pub(crate) ports: Vec<u64>,
}

/// The identity of one outstanding dial, shared by its request and its
/// completion so the frame knows which operation the response belongs to.
#[derive(Clone, Copy)]
pub(crate) enum DialContext {
    JsFetch {
        id: i32,
    },
    ClassicScript {
        element: crate::dom::tree::NodeId,
    },
    /// A child frame's own navigation
    /// (<https://html.spec.whatwg.org/multipage/browsing-the-web.html#navigate>);
    /// starting another navigation cancels this one through the ledger.
    FrameLoad {
        /// The embedding document, when this navigation is a nested browsing
        /// context. `None` for the top-level document.
        embedder: Option<FrameId>,
    },
    /// A Blitz subresource (stylesheet, image, font) fetch. The response
    /// handler waits in [`Document::blitz_handlers`] under this id; delivery
    /// hands Blitz its bytes through the handler, which releases Blitz's own
    /// critical hold.
    BlitzResource {
        id: u64,
        /// `style-src-elem`, `img-src`, or `font-src` for this fetch.
        destination: Option<crate::csp::Destination>,
    },
    /// A `report-uri` violation report. The response is dropped; reports
    /// never gate load and are never CSP-checked themselves
    /// (<https://w3c.github.io/webappsec-csp/#report-violation>).
    CspReport,
}

/// One dial waiting to start: the wire request plus the dial-local context
/// the backend keeps for the response.
pub(crate) struct QueuedDial {
    pub(crate) context: DialContext,
    /// The request the dial starts as, built where the dial is queued: kind,
    /// URLs, method, body, referrer, CSP redirect gate, and CORS parameters
    /// are final.
    pub(crate) req: DialRequest,
    /// Ancestor document URLs nearest-first, snapshotted when a nested frame
    /// navigation starts. The child's `frame-ancestors` policy is checked
    /// against these at commit
    /// (<https://w3c.github.io/webappsec-csp/#frame-ancestors-navigation-response>).
    pub(crate) ancestors: Vec<Url>,
    /// Raw `integrity` attribute for a classic script dial, verified against
    /// the response bytes. `None` enforces nothing.
    pub(crate) integrity: Option<String>,
}

impl QueuedDial {
    /// The wire kind a dial context maps to. Plain page subresources reuse
    /// the image kind; no carrier policy keys off the kind except for
    /// script-initiated fetches.
    fn dial_kind(context: &DialContext) -> crate::protocol::DialKind {
        match context {
            DialContext::JsFetch { .. } => crate::protocol::DialKind::JsFetch,
            DialContext::ClassicScript { .. } => crate::protocol::DialKind::ClassicScript,
            DialContext::FrameLoad { .. } => crate::protocol::DialKind::FrameLoad,
            DialContext::BlitzResource { destination, .. } => match destination {
                Some(crate::csp::Destination::StyleSheet) => crate::protocol::DialKind::Stylesheet,
                Some(crate::csp::Destination::Font) => crate::protocol::DialKind::Font,
                _ => crate::protocol::DialKind::Image,
            },
            DialContext::CspReport => crate::protocol::DialKind::CspReport,
        }
    }

    /// A fresh GET dial: the shape of every dial but a form submission and a
    /// page `fetch`. The call sites that need more set the request's fields
    /// directly (`dial.req.…`).
    pub(crate) fn new(
        context: DialContext,
        url: &Url,
        initiator: &url::Origin,
        referrer_url: &Url,
    ) -> Self {
        Self {
            req: crate::protocol::DialRequest {
                kind: Self::dial_kind(&context),
                url: url.to_string(),
                initiator: initiator.ascii_serialization(),
                referrer_url: referrer_url.to_string(),
                read_body: true,
                method: "GET".to_owned(),
                body: Vec::new(),
                content_type: None,
                headers: Vec::new(),
                referrer: net::Referrer::NoReferrer,
                timeout_ms: 0,
                csp: None,
                cors: None,
                keepalive: false,
                body_len: 0,
                integrity: None,
                cache: None,
                stream_body: false,
                referrer_policy: net::ReferrerPolicy::default(),
            },
            context,
            ancestors: Vec::new(),
            integrity: None,
        }
    }
}

/// One finished dial with the response it produced.
pub(crate) struct CompletedDial {
    pub(crate) context: DialContext,
    pub(crate) outcome: DialOutcome,
    /// The wire request the dial started as: the request URL tells a redirect
    /// from the original hop, and the CSP gate reports refused redirects.
    pub(crate) req: DialRequest,
    /// Ancestor document URLs for a nested frame navigation, checked against
    /// the response's `frame-ancestors` at commit.
    pub(crate) ancestors: Vec<Url>,
    /// Raw `integrity` attribute for a classic script response, verified
    /// against the response bytes before execution.
    pub(crate) integrity: Option<String>,
}

/// Who feeds the active parser, and therefore who may end it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ParserOwner {
    /// A response body, or a document handed over whole: only the carrier
    /// decides when the input ends.
    Carrier,
    /// A script's `document.open()`: it writes text, and `document.close()`
    /// ends the parser.
    Script,
}

/// The markup an active parser accumulates and the delay token that keeps
/// `load` pending for that parse. Taking the value ends the parse.
struct ActiveParse {
    buffer: String,
    delay: LoadDelayGuard,
}

/// A connection transition for one `iframe` container in this document.
pub(crate) enum IframeLifecycle {
    /// A container whose frame is gone or disconnected.
    Removed(crate::dom::tree::NodeId),
}

/// A connection transition for one `<img>` element in this document.
pub(crate) enum ImageLifecycle {
    /// A newly connected image to start fetching.
    Connected(crate::dom::tree::NodeId),
    /// A disconnected image to stop fetching.
    Disconnected(crate::dom::tree::NodeId),
}

struct Timer {
    when: Instant,
    fired: bool,
    /// The script-visible id, when this timer belongs to a realm.
    js_id: Option<i32>,
}

/// Builds and queues the initial `about:blank` document for one connected
/// `iframe` so `contentDocument` is available in the inserting script
/// (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#the-iframe-element:html-element-insertion-steps>).
///
/// The nested realm is deferred: `QuickJS` cannot enter a new context while
/// the parent script still runs.
/// Maps a Blitz request kind onto the CSP destination it is checked as.
fn subresource_destination(
    kind: blitz_traits::net::RequestKind,
) -> Option<crate::csp::Destination> {
    match kind {
        blitz_traits::net::RequestKind::Stylesheet => Some(crate::csp::Destination::StyleSheet),
        blitz_traits::net::RequestKind::Image => Some(crate::csp::Destination::Img),
        blitz_traits::net::RequestKind::Font => Some(crate::csp::Destination::Font),
        blitz_traits::net::RequestKind::Other => None,
    }
}

fn materialize_iframe_document(
    world_rc: &Rc<RefCell<World>>,
    frame: FrameId,
    container: crate::dom::tree::NodeId,
    new_frames: &Rc<RefCell<Vec<CreatedFrame>>>,
) {
    let (runtime, base_url, referrer_url, origin, init_scripts, viewport) = {
        let world = world_rc.borrow();
        (
            world.runtime.clone(),
            world.document_url.clone(),
            world.referrer_url.clone(),
            world.origin.clone(),
            Rc::clone(&world.init_scripts),
            world.viewport_size.get(),
        )
    };
    let mut document = Document::with_shared(frame, &runtime, &init_scripts);
    document.set_viewport_size(viewport);
    document.load_about_blank_tree(Some(base_url.as_str()), Some(origin));
    // The initial blank reports its creator's URL, walked out of a `srcdoc`
    // chain the same way the referrer source is
    // (<https://html.spec.whatwg.org/multipage/browsers.html#creating-a-new-browsing-context>).
    document.set_document_referrer(referrer_url.as_str());
    let inherited = world_rc.borrow().policy.clone();
    document.world().borrow_mut().policy = inherited;
    if let Some(root) = document.document_root() {
        world_rc
            .borrow()
            .registry()
            .borrow_mut()
            .set_frame_document(container, root);
    }
    new_frames.borrow_mut().push((frame, container, document));
}

/// One document: tree, task list, `QuickJS` realm, and browser services.
#[allow(
    clippy::struct_excessive_bools,
    reason = "each bool mirrors an HTML document or parser flag"
)]
pub(crate) struct Document {
    services: Arc<dyn BrowserServices>,
    world: Rc<RefCell<World>>,
    js_runtime: JsRuntimeHandle,
    wake: Arc<Notify>,
    /// Blitz subresource fetches run through our agent on the carrier
    /// runtime; Blitz owns stylesheets and images once they land.
    net_provider: std::sync::Arc<crate::render::TinyNetProvider>,
    /// Blitz link/form navigations, drained into frame navigations on tick.
    blitz_nav: std::sync::Arc<crate::render::TinyNav>,
    /// Blitz subresource fetches awaiting carrier dials.
    blitz_fetch_rx:
        std::sync::mpsc::Receiver<(crate::render::BlitzFetch, crate::render::CountingHandler)>,
    /// Filed Blitz response handlers by fetch id, delivered on completion.
    blitz_handlers: HashMap<u64, crate::render::CountingHandler>,
    /// Request URL for each filed Blitz handler, so an image delivery settles
    /// the `<img>` waiters selected for that URL.
    blitz_request_urls: HashMap<u64, String>,
    /// Raw subresource bytes by URL, so a repeat Blitz fetch for a URL reuses
    /// bytes instead of dialing again.
    subresource_bytes: HashMap<String, Vec<u8>>,
    /// Referrer policy each fetched CSS style sheet's response carried, keyed
    /// by the URL the sheet was located at. A request the sheet initiates uses
    /// it and the sheet's URL, not the document's own
    /// (<https://w3c.github.io/webappsec-referrer-policy/#integration-with-css>).
    style_policies: HashMap<String, net::ReferrerPolicy>,
    /// The browsing context this document belongs to.
    frame: FrameId,
    /// The frame's persistent viewport in device pixels (the scale is fixed
    /// at 1, so device and CSS pixels coincide). CDP emulation and every
    /// document mount use it, so Blitz layout, the painted size, and
    /// `window` metrics all agree
    /// (<https://chromedevtools.github.io/devtools-protocol/tot/Emulation/#method-setDeviceMetricsOverride>).
    pub(crate) viewport_size: (u32, u32),
    /// The renderer-process state every frame shares.
    shared: SharedHandle,
    url: Url,
    tasks: VecDeque<Task>,
    timers: HashMap<u32, Timer>,
    next_timer_id: u32,
    /// Every queued, in-flight, and cancelled dial for this document.
    dials: DialLedger,
    /// `<img>` elements awaiting Blitz image state. They delay the document
    /// load event; Blitz fetches and decodes, we only watch.
    /// Selected source URL for the current request
    /// (<https://html.spec.whatwg.org/multipage/images.html#updating-the-image-data>).
    image_selected_src: HashMap<crate::dom::tree::NodeId, String>,
    /// Elements with an unresolved image request for the current generation.
    /// The value is the load-event delay token, released when the request
    /// settles or the element goes away.
    in_flight_images: HashMap<crate::dom::tree::NodeId, LoadDelayGuard>,
    /// Connected `<img>` elements the last lifecycle scan saw, so insertion
    /// starts a fetch and removal drops one
    /// (<https://html.spec.whatwg.org/multipage/images.html#updating-the-image-data>).
    connected_images: HashSet<crate::dom::tree::NodeId>,
    /// Connected `<link rel=stylesheet>` elements already reported as loaded,
    /// with the resolved URL that arrival was for, so a re-fetch after an
    /// `href` or `rel` change reports again
    /// (<https://html.spec.whatwg.org/multipage/semantics.html#the-link-element>).
    linked_sheets: HashMap<crate::dom::tree::NodeId, String>,
    /// Stylesheet generation observed at the last linked-stylesheet scan.
    stylesheet_generation: u64,
    /// Whether the frame is still on the `about:blank` document a browser
    /// creates at insertion. A URL comparison cannot answer this: a frame
    /// whose `src` resolves to the parent's own URL is still on that document
    /// (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#process-the-iframe-attributes>).
    initial_blank: bool,
    /// The `iframe` containers awaiting their child's load event; this
    /// document's own `load` event is delayed until they all have
    /// (<https://html.spec.whatwg.org/multipage/parsing.html#delay-the-load-event>).
    /// Each value is that container's delay token.
    pending_frame_loads: HashMap<crate::dom::tree::NodeId, LoadDelayGuard>,
    /// Child frame documents this document's realm created for the engine to
    /// adopt
    /// (<https://html.spec.whatwg.org/multipage/document-sequences.html#document-tree-child-navigable>).
    new_frames: Rc<RefCell<Vec<CreatedFrame>>>,
    /// Whether this document's `load` event has fired.
    load_fired: bool,
    /// Whether the shared declarative refresh steps already ran for this
    /// document; a later `Refresh` value is ignored
    /// (<https://html.spec.whatwg.org/multipage/semantics.html#attr-meta-http-equiv-refresh>).
    will_declaratively_refresh: bool,
    /// The target of a refresh waiting for the document to finish loading,
    /// with its delay. The clock starts at the completely-loaded point, not
    /// when the value was parsed.
    pending_refresh: Option<(Url, std::time::Duration)>,
    /// The timer a refresh is waiting on, so a new navigation can cancel it.
    refresh_timer: Option<u32>,
    /// The URL an armed refresh navigates to when its timer fires.
    refresh_target: Option<Url>,
    events: Vec<RendererEvent>,
    events_overflowed: bool,
    js: Option<crate::js::JsRealm>,
    /// Live JS timers by the id script holds. The timer owns the page id.
    js_timers: HashMap<i32, u32>,
    /// One count for every operation delaying this document's `load` event.
    load_delay: LoadDelay,
    /// Whether init scripts were deferred because the realm was created while
    /// a navigation response was still streaming.
    init_scripts_deferred: bool,
    /// Skip `QuickJS` realm creation while installing a document. Nested
    /// iframe insertion runs inside an active realm, and `QuickJS` forbids
    /// entering another one until that script returns.
    defer_js: bool,
    /// The current parse's markup and delay, or `None` when no parse is active.
    active_parse: Option<ActiveParse>,
    /// Whether `document.open` or `document.close` ran and the engine still
    /// needs to publish the frame's document. `Some(true)` also restarts the
    /// parent's load wait for a closed stream.
    stream_publish: Option<bool>,
    /// Scripts already executed for the current parse, so a walk resumed
    /// after a `src` fetch does not run them twice.
    executed_scripts: HashSet<crate::dom::tree::NodeId>,
    parser_eof: bool,
    /// Who owns the active parser, which decides whether `document.close()` may
    /// end it.
    parser_owner: ParserOwner,
    /// Decoder for a body that is still arriving; `None` when the markup is
    /// already in hand.
    decoder: Option<dial::ResponseDecoder>,
    /// Encoding sniffed for the current response, reported as
    /// `document.characterSet`.
    character_set: &'static str,
    /// The response `Content-Type` for the active parser. XML MIME types
    /// select the XML parser; anything else, including a script's
    /// `document.open()`, stays HTML.
    response_content_type: Option<String>,
    /// `Content-Security-Policy` and `Content-Security-Policy-Report-Only`
    /// from the response being parsed, parsed once
    /// (<https://w3c.github.io/webappsec-csp/#parse-response-csp>).
    response_policy: crate::csp::List,
    /// `Referrer-Policy` from the response being parsed, applied after the
    /// parser installs the new document (the state reset clears the realm).
    response_referrer_policy: Option<net::ReferrerPolicy>,
    /// The policy container a local-scheme document inherits, installed
    /// atomically after the parser replaces the document's state
    /// (<https://html.spec.whatwg.org/multipage/browsers.html#policy-containers>).
    inherited_policy: Option<PolicyContainer>,
    /// When set, [`Document::install_parsed`] replaces the world's policy
    /// with the response headers. A script's `document.open()` leaves it
    /// unset so the existing policy survives.
    csp_from_response: bool,
    /// Endpoint names and URLs from the navigation response's
    /// `Reporting-Endpoints` headers. A violated policy's `report-to`
    /// directive resolves its first known name here
    /// (<https://w3c.github.io/reporting/#header>).
    ///
    /// Only [`Document::install_head`] and `set_reporting_endpoints` write
    /// it; no caller reaches around them.
    reporting_endpoints: Vec<(String, String)>,
    /// HTTP status of the committed navigation response, reported as the
    /// violation `status-code`. Zero when unknown
    /// (<https://w3c.github.io/webappsec-csp/#create-violation-for-global>).
    http_status: u16,
    /// First 1445 raw response bytes, the MIME sniff resource header.
    sniff_header: Vec<u8>,
    /// Whether [`Document::finish_parse`] runs the MIME sniffing algorithm.
    apply_mime_sniff: bool,
    /// `nosniff` and whether this body came from HTTP.
    mime_guard: crate::sniff::MimeGuard,
    /// The parser-inserted script the parser waits for, if any. A dynamic
    /// script that finishes first must not resume the walk: the synchronous
    /// scripts after a parser-inserted `src` run only once it executes
    /// (<https://html.spec.whatwg.org/multipage/scripting.html#prepare-the-script-element>).
    parser_blocked_script: Option<crate::dom::tree::NodeId>,
    deferred_modules: Vec<(crate::dom::tree::NodeId, crate::js::ScriptSource)>,
    stop: Arc<crate::js::Stop>,
    /// `frame-src` redirect violations to fire in the embedding document.
    /// The navigating frame cannot dispatch into its parent's realm.
    embedder_csp_reports: Vec<(FrameId, Vec<crate::csp::Violation>)>,
}

impl Drop for Document {
    fn drop(&mut self) {
        self.release();
    }
}

/// Parses one declarative refresh value: `"<seconds>"` or
/// `"<seconds>; url=<url>"`, with or without the `url=` prefix, optional
/// quotes, and any ASCII whitespace. `None` is the algorithm's early return,
/// which means the value names nothing to refresh to. The target defaults to
/// the document's own URL.
/// (<https://html.spec.whatwg.org/multipage/semantics.html#attr-meta-http-equiv-refresh>).
fn parse_declarative_refresh(
    input: &str,
    document_url: &Url,
) -> Option<(Url, std::time::Duration)> {
    let mut position = 0;
    skip_ascii_whitespace(input, &mut position);
    let time_start = position;
    while next_char(input, position).is_some_and(|c| c.is_ascii_digit()) {
        position += 1;
    }
    let time = if position == time_start {
        if next_char(input, position) != Some('.') {
            return None;
        }
        0
    } else {
        input[time_start..position]
            .parse::<u64>()
            .unwrap_or(u64::MAX)
    };
    while next_char(input, position).is_some_and(|c| c.is_ascii_digit() || c == '.') {
        position += 1;
    }
    let mut target = document_url.clone();
    if position < input.len() {
        let separator = next_char(input, position)?;
        if separator != ';' && separator != ',' && !separator.is_ascii_whitespace() {
            return None;
        }
        skip_ascii_whitespace(input, &mut position);
        if matches!(next_char(input, position), Some(';' | ',')) {
            position += 1;
        }
        skip_ascii_whitespace(input, &mut position);
    }
    if position < input.len() {
        let mut url_start = position;
        // `url=` is optional and case-insensitive. A prefix without `=` falls
        // through to the URL text itself, so `0; urlish` is a relative URL.
        if starts_with_ascii_case(&input[position..], "url") {
            let mut probe = position + 3;
            skip_ascii_whitespace(input, &mut probe);
            if next_char(input, probe) == Some('=') {
                probe += 1;
                skip_ascii_whitespace(input, &mut probe);
                url_start = probe;
            }
        }
        let mut text = input.get(url_start..)?;
        let quote = match text.chars().next() {
            Some(quote @ ('\'' | '"')) => {
                text = &text[1..];
                Some(quote)
            }
            _ => None,
        };
        let text = match quote {
            Some(quote) => text.split_once(quote).map_or(text, |(before, _)| before),
            None => text,
        };
        let text = text.trim();
        // The document's own URL stands when the value names nothing.
        target = if text.is_empty() {
            document_url.clone()
        } else {
            document_url.join(text).ok()?
        };
        if target.scheme() == "javascript" {
            return None;
        }
    }
    Some((target, std::time::Duration::from_secs(time)))
}

fn skip_ascii_whitespace(input: &str, position: &mut usize) {
    while let Some(rest) = input.get(*position..) {
        match rest.chars().next() {
            Some(character) if character.is_ascii_whitespace() => {
                *position += character.len_utf8();
            }
            _ => break,
        }
    }
}

fn next_char(input: &str, position: usize) -> Option<char> {
    input.get(position..)?.chars().next()
}

fn starts_with_ascii_case(input: &str, prefix: &str) -> bool {
    input.len() >= prefix.len() && input[..prefix.len()].eq_ignore_ascii_case(prefix)
}

impl Document {
    /// A document sharing its renderer process's `QuickJS` heap, wake handle,
    /// document store, realm registry, and frame tree.
    pub(crate) fn with_shared(
        frame: FrameId,
        runtime: &FrameRuntime,
        init_scripts: &Rc<RefCell<Vec<(u64, String)>>>,
    ) -> Self {
        let document_url = Url::parse("about:blank").expect("about:blank is a valid URL");
        let world = Rc::new(RefCell::new(World::new(
            document_url.clone(),
            frame,
            runtime,
            Rc::clone(init_scripts),
        )));
        runtime.registry.borrow_mut().insert_frame(frame, &world);
        // Document creation belongs to this layer: the realm asks the frame
        // host for a child document and never names a document type.
        let new_frames: Rc<RefCell<Vec<CreatedFrame>>> = Rc::new(RefCell::new(Vec::new()));
        world.borrow_mut().set_frame_host(Rc::new({
            let new_frames = Rc::clone(&new_frames);
            move |world_rc: &Rc<RefCell<World>>, frame, container| {
                materialize_iframe_document(world_rc, frame, container, &new_frames);
            }
        }));
        // Blitz subresources enqueue here and ride carrier dials: the
        // renderer owns no network runtime of its own.
        let (blitz_tx, blitz_rx) = std::sync::mpsc::channel();
        let net_provider = std::sync::Arc::new(crate::render::TinyNetProvider::new(blitz_tx));
        let blitz_nav = std::sync::Arc::new(crate::render::TinyNav::new());
        Self {
            world,
            services: Arc::clone(&runtime.services),
            js_runtime: runtime.js_runtime.clone(),
            wake: Arc::clone(&runtime.wake),
            net_provider,
            blitz_nav,
            blitz_fetch_rx: blitz_rx,
            blitz_handlers: HashMap::new(),
            blitz_request_urls: HashMap::new(),
            subresource_bytes: HashMap::new(),
            style_policies: HashMap::new(),
            frame,
            viewport_size: crate::engine::DEFAULT_VIEWPORT,
            shared: Rc::clone(&runtime.shared),
            url: document_url,
            tasks: VecDeque::new(),
            timers: HashMap::new(),
            next_timer_id: 1,
            dials: DialLedger::new(Arc::clone(&runtime.stop), Arc::clone(&runtime.wake)),
            image_selected_src: HashMap::new(),
            in_flight_images: HashMap::new(),
            connected_images: HashSet::new(),
            linked_sheets: HashMap::new(),
            stylesheet_generation: 0,
            initial_blank: true,
            pending_frame_loads: HashMap::new(),
            new_frames,
            load_fired: false,
            will_declaratively_refresh: false,
            pending_refresh: None,
            refresh_timer: None,
            refresh_target: None,
            events: Vec::new(),
            events_overflowed: false,
            js: None,
            js_timers: HashMap::new(),
            load_delay: LoadDelay::default(),
            init_scripts_deferred: false,
            defer_js: false,
            active_parse: None,
            stream_publish: None,
            executed_scripts: HashSet::new(),
            parser_eof: true,
            parser_owner: ParserOwner::Carrier,
            decoder: None,
            character_set: "UTF-8",
            response_content_type: None,
            response_policy: crate::csp::List::default(),
            response_referrer_policy: None,
            inherited_policy: None,
            embedder_csp_reports: Vec::new(),
            csp_from_response: false,
            reporting_endpoints: Vec::new(),
            http_status: 0,
            sniff_header: Vec::new(),
            apply_mime_sniff: false,
            mime_guard: crate::sniff::MimeGuard::default(),
            parser_blocked_script: None,
            deferred_modules: Vec::new(),
            stop: Arc::clone(&runtime.stop),
        }
    }

    /// The world this document's realm belongs to.
    pub(crate) fn world(&self) -> Rc<RefCell<World>> {
        Rc::clone(&self.world)
    }

    /// The serialized origin of the frame's document.
    #[must_use]
    pub(crate) fn origin_string(&self) -> String {
        self.world.borrow().origin.ascii_serialization()
    }

    /// The root node of the frame's active document.
    pub(crate) fn document_root(&self) -> Option<crate::dom::tree::NodeId> {
        self.world
            .borrow()
            .with_main_document(|parsed| crate::dom::tree::NodeId {
                document: parsed.id,
                node: parsed.document.base.root_node().id,
            })
    }

    /// The `iframe`'s `referrerpolicy` attribute parsed to a policy, or
    /// `None` when absent or invalid, which leaves the document's policy in
    /// force
    /// (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#attr-iframe-referrerpolicy>).
    #[must_use]
    pub(crate) fn frame_referrer_policy(
        &self,
        container: crate::dom::tree::NodeId,
    ) -> Option<net::ReferrerPolicy> {
        let world = self.world.borrow();
        world.document(container).and_then(|parsed| {
            crate::dom::tree::attr(&parsed.document.base, container.node, "referrerpolicy")
                .and_then(net::ReferrerPolicy::from_attribute)
        })
    }

    /// The `iframe`'s `src` attribute value, when the element has a
    /// non-empty one
    /// (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#attr-iframe-src>).
    #[must_use]
    pub(crate) fn frame_src(&self, container: crate::dom::tree::NodeId) -> Option<String> {
        let world = self.world.borrow();
        let value = world.document(container).and_then(|parsed| {
            crate::dom::tree::attr(&parsed.document.base, container.node, "src").map(str::to_owned)
        })?;
        (!value.is_empty()).then_some(value)
    }

    /// The `iframe`'s `srcdoc` attribute value, when the element has one —
    /// even an empty one, which loads the empty `about:srcdoc` document
    /// (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#attr-iframe-srcdoc>).
    #[must_use]
    pub(crate) fn frame_srcdoc(&self, container: crate::dom::tree::NodeId) -> Option<String> {
        let world = self.world.borrow();
        world.document(container).and_then(|parsed| {
            crate::dom::tree::attr(&parsed.document.base, container.node, "srcdoc")
                .map(str::to_owned)
        })
    }

    /// Resolves one of the frame's URLs the way an attribute would: absolute
    /// against the document, or joined with its base URL
    /// (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#parse-a-url>).
    #[must_use]
    pub(crate) fn resolve_frame_url(&self, spec: &str) -> Option<Url> {
        Url::parse(spec)
            .or_else(|_| self.base_url().join(spec))
            .ok()
    }

    /// An object URL's contents, when this document's origin created it
    /// (<https://w3c.github.io/FileAPI/#blob-url-store>).
    #[must_use]
    pub(crate) fn object_url_contents(&self, url: &str) -> Option<Rc<str>> {
        self.world.borrow().object_url_contents(url)
    }

    /// The `Content-Type` an object URL was created from.
    #[must_use]
    pub(crate) fn object_url_type(&self, url: &str) -> Option<Rc<str>> {
        self.world.borrow().object_url_type(url)
    }

    /// The creator origin stored for an object URL this document may use.
    #[must_use]
    pub(crate) fn object_url_origin(&self, url: &str) -> Option<url::Origin> {
        self.world.borrow().object_url_origin(url)
    }

    /// Creates the browsing context of every connected `iframe` that lacks
    /// one, so a script sees it in the same task
    /// (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#process-the-iframe-attributes>).
    pub(crate) fn adopt_pending_frames(&mut self) {
        self.adopt_pending_frames_before_node(None);
    }

    /// Creates browsing contexts for connected `iframe`s that precede `before`
    /// in tree order, or every remaining iframe when `before` is `None`.
    ///
    /// Whole-document parse inserts every iframe before any script runs. A
    /// real incremental parser would only have inserted iframes that precede
    /// the current script; src loads that start earlier fire `load` before
    /// later classic scripts define their onload functions
    /// (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#the-iframe-element:html-element-insertion-steps>).
    fn adopt_pending_frames_before_node(&mut self, before: Option<crate::dom::tree::NodeId>) {
        let created = self.world.borrow_mut().register_pending_frames(before);
        self.materialize_pending_frames();
        for container in created {
            // The child is loading from this moment on, so this document's
            // `load` event waits for it
            // (<https://html.spec.whatwg.org/multipage/parsing.html#delay-the-load-event>).
            self.mark_frame_load_pending(container);
        }
    }

    /// Loads every registered frame document without holding the world
    /// borrow: pending frames and frame data are taken first, documents load
    /// unborrowed, and only then are the results published. Loading runs the
    /// parser, which delivers mutations, which fans out across live worlds;
    /// holding the world's borrow across that would alias the fan-out
    /// borrows and panic the renderer.
    fn materialize_pending_frames(&mut self) {
        let pending = self.world.borrow_mut().take_pending_frames();
        if pending.is_empty() {
            return;
        }
        let (runtime, base_url, referrer_url, origin, init_scripts, viewport) = {
            let world = self.world.borrow();
            (
                world.runtime.clone(),
                world.document_url.clone(),
                world.referrer_url.clone(),
                world.origin.clone(),
                Rc::clone(&world.init_scripts),
                world.viewport_size.get(),
            )
        };
        let mut loaded = Vec::with_capacity(pending.len());
        let inherited = self.world.borrow().policy.clone();
        for (frame, container) in pending {
            let mut document = Document::with_shared(frame, &runtime, &init_scripts);
            // Frames created from parsed markup share the tab's viewport too.
            // This path runs before page script, so the realm can exist for
            // `contentWindow` in the following classic script
            // (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#the-iframe-element:html-element-insertion-steps>).
            document.set_viewport_size(viewport);
            document.load_about_blank(Some(base_url.as_str()), Some(origin.clone()));
            document.set_document_referrer(referrer_url.as_str());
            document.world().borrow_mut().policy = inherited.clone();
            loaded.push((frame, container, document));
        }
        self.new_frames.borrow_mut().extend(loaded);
    }

    /// Applies writes the realm stored on a child frame's `contentWindow`
    /// before that frame's realm existed.
    pub(crate) fn flush_frame_proxy_sets(&mut self, child: FrameId) {
        self.fire_js(|js| js.flush_frame_sets(child.get()));
        self.adopt_js_work();
    }

    /// Takes the child frames this document's realm created for the engine to
    /// adopt.
    pub(crate) fn take_new_frames(&mut self) -> Vec<CreatedFrame> {
        std::mem::take(&mut *self.new_frames.borrow_mut())
    }

    /// The document URL used by `about:blank` frames, which inherit the
    /// parent's origin and, in this engine, its URL
    /// (<https://html.spec.whatwg.org/multipage/browsers.html#determining-the-origin>).
    #[must_use]
    pub(crate) fn inherited_url(&self) -> String {
        self.url.as_str().to_owned()
    }

    /// Sets the referrer the active document reports. Local-scheme installs
    /// have no navigation request to read one from; their document reports
    /// the creator's URL instead
    /// (<https://html.spec.whatwg.org/multipage/browsers.html#creating-a-new-browsing-context>).
    pub(crate) fn set_document_referrer(&mut self, referrer: &str) {
        let mut world = self.world.borrow_mut();
        referrer.clone_into(&mut world.document_referrer);
    }

    pub(crate) fn take_lifecycle(&mut self) -> (Vec<IframeLifecycle>, Vec<ImageLifecycle>) {
        let (connected, connected_images) = self
            .world
            .borrow()
            .with_main_document(|parsed| {
                let base = &parsed.document.base;
                let document = parsed.id;
                let mut containers = Vec::new();
                let mut images = Vec::new();
                let mut stack = vec![base.root_node().id];
                while let Some(id) = stack.pop() {
                    if crate::dom::tree::is_connected(base, id) {
                        if crate::dom::tree::is_child_navigable_container(base, id) {
                            containers.push(crate::dom::tree::NodeId { document, node: id });
                        }
                        if crate::dom::tree::is_html_element(base, id, "img") {
                            images.push(crate::dom::tree::NodeId { document, node: id });
                        }
                    }
                    if let Some(node) = base.get_node(id) {
                        stack.extend(node.children.iter().rev().copied());
                    }
                }
                (containers, images)
            })
            .unwrap_or_default();
        let shared = self.shared.borrow();
        let known: std::collections::HashSet<crate::dom::tree::NodeId> = shared
            .tree
            .children(self.frame)
            .iter()
            .filter_map(|child| shared.tree.container(*child))
            .collect();
        drop(shared);
        let connected_set: std::collections::HashSet<crate::dom::tree::NodeId> =
            connected.iter().copied().collect();
        let mut iframe_events = Vec::new();
        // Insertion is the frame-host materialization (`Document`'s pending
        // frames) / `materialize_iframe` in tree order with scripts. Scanning
        // connected iframes here would start parser `src` loads before
        // preceding classic scripts run.
        for container in &known {
            if !connected_set.contains(container) {
                iframe_events.push(IframeLifecycle::Removed(*container));
            }
        }
        let connected_image_set: std::collections::HashSet<crate::dom::tree::NodeId> =
            connected_images.iter().copied().collect();
        let mut image_events = Vec::new();
        for element in &connected_images {
            if !self.connected_images.contains(element) {
                image_events.push(ImageLifecycle::Connected(*element));
            }
        }
        let previous = std::mem::replace(&mut self.connected_images, connected_image_set);
        for element in previous {
            if !self.connected_images.contains(&element) {
                image_events.push(ImageLifecycle::Disconnected(element));
            }
        }
        (iframe_events, image_events)
    }

    pub(crate) fn take_frame_navigations(&mut self) -> Vec<FrameNavigation> {
        let mut navigations = self.world.borrow_mut().take_frame_navigations();
        // A click Blitz routes through its navigation provider names no target
        // (`NavigationOptions` carries the URL alone), so the empty target
        // leaves it navigating the frame it was clicked in.
        navigations.extend(self.blitz_nav.drain().into_iter().map(|url| {
            FrameNavigation::get(
                crate::js::NavigationTarget::Target(String::new()),
                url.as_str().to_owned(),
            )
        }));
        navigations
    }

    pub(crate) fn take_stream_publish(&mut self) -> Option<bool> {
        self.stream_publish.take()
    }

    pub(crate) fn fire_node_load(&mut self, id: crate::dom::tree::NodeId) {
        self.fire_js(|js| js.fire_node_load(id));
        self.adopt_js_work();
    }

    /// Queues the image fetch for a newly connected `<img>`, if it still needs one
    /// (<https://html.spec.whatwg.org/multipage/images.html#updating-the-image-data>).
    pub(crate) fn queue_connected_image(&mut self, element: crate::dom::tree::NodeId) {
        self.queue_image(element, false);
    }

    /// Drops a disconnected `<img>`'s decoded size and ignores its waiter.
    pub(crate) fn disconnect_image(&mut self, element: crate::dom::tree::NodeId) {
        self.in_flight_images.remove(&element);
        self.image_selected_src.remove(&element);
        // A re-insertion starts a new load: drop the old trigger so the
        // move attributes to itself rather than to a stale `src` set.
        self.world
            .borrow_mut()
            .state
            .creator_stacks
            .remove(&element);
        self.world.borrow_mut().forget_image(element);
    }

    /// Queues one dial after applying CSP `upgrade-insecure-requests` to it.
    ///
    /// The rewrite happens at the top of main fetch, before the request leaves
    /// the renderer: an `http` request URL becomes `https`, and an explicit
    /// `http` referrer follows. A nested navigation always upgrades because
    /// the embedder's security state depends on it; a top-level navigation
    /// upgrades only a host in the document's upgrade set, which is the
    /// protected resource's own host and port. Every other request upgrades
    /// unconditionally
    /// (<https://w3c.github.io/webappsec-upgrade-insecure-requests/#upgrade-request>).
    fn queue_dial(&mut self, mut dial: QueuedDial) {
        if self.world.borrow().policy.csp.upgrades_insecure_requests() {
            let nested = matches!(dial.context, DialContext::FrameLoad { embedder: Some(_) });
            let upgrades = if dial.req.kind == crate::protocol::DialKind::FrameLoad && !nested {
                // A top-level navigation must target the protected resource's
                // own host and port, so a policy cannot upgrade a link to a
                // third party
                // (<https://w3c.github.io/webappsec-upgrade-insecure-requests/#upgrade-request>).
                Self::same_host_port(&dial.req.url, self.url.as_str())
            } else {
                true
            };
            if upgrades {
                Self::upgrade_scheme(&mut dial.req.url);
                if let net::Referrer::Url(url) = &mut dial.req.referrer
                    && url.scheme() == "http"
                {
                    let _ = url.set_scheme("https");
                }
            }
        }
        // A navigation request advertises the client's upgrade capability so a
        // server can feature-detect it
        // (<https://w3c.github.io/webappsec-upgrade-insecure-requests/#upgrade-insecure-requests-http-request-header-field>).
        if dial.req.kind == crate::protocol::DialKind::FrameLoad
            && !dial
                .req
                .headers
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("upgrade-insecure-requests"))
        {
            dial.req
                .headers
                .push(("Upgrade-Insecure-Requests".to_owned(), "1".to_owned()));
        }
        self.dials.queue(dial, &self.load_delay);
    }

    /// Whether `left` and `right` name the same host and port
    /// (<https://url.spec.whatwg.org/#concept-url-host>).
    fn same_host_port(left: &str, right: &str) -> bool {
        let (Ok(left), Ok(right)) = (Url::parse(left), Url::parse(right)) else {
            return false;
        };
        left.host_str() == right.host_str()
            && left.port_or_known_default() == right.port_or_known_default()
    }

    /// Rewrites one serialized URL's scheme from `http` to `https`, leaving
    /// every other scheme alone.
    fn upgrade_scheme(spec: &mut String) {
        let Ok(mut url) = Url::parse(spec) else {
            return;
        };
        if url.scheme() != "http" {
            return;
        }
        if url.set_scheme("https").is_ok() {
            *spec = url.to_string();
        }
    }

    /// Starts one frame-load dial. `req` is the wire request, with the method,
    /// body, `Content-Type`, referrer, and the embedder's `frame-src` redirect
    /// gate already on it; `ancestors` are the ancestor URLs the child
    /// response's `frame-ancestors` is checked against at commit. It forces the
    /// response body to be read and cancels an earlier navigation: the newest
    /// load owns the frame.
    pub(crate) fn navigate(
        &mut self,
        embedder: Option<FrameId>,
        mut req: crate::protocol::DialRequest,
        ancestors: Vec<Url>,
    ) {
        req.kind = crate::protocol::DialKind::FrameLoad;
        req.read_body = true;
        self.dials.cancel_frame_loads();
        self.initial_blank = false;
        self.queue_dial(QueuedDial {
            context: DialContext::FrameLoad { embedder },
            req,
            ancestors,
            integrity: None,
        });
    }

    /// Gives a blank document the URL `about:blank` and the creator's base
    /// URL for relative resolution.
    fn apply_blank_url(
        &mut self,
        inherited_url: Option<&str>,
        inherited_origin: Option<url::Origin>,
    ) {
        let Some(base) = inherited_url.and_then(|url| Url::parse(url).ok()) else {
            return;
        };
        let blank = Url::parse("about:blank").expect("about:blank is a valid URL");
        self.set_document_url(blank, inherited_origin);
        self.world.borrow_mut().base_url = Some(base);
    }

    /// Loads the frame's initial `about:blank`, with `inherited_url` as its
    /// fallback base URL and `inherited_origin` as the captured origin. The
    /// document URL stays `about:blank`: it is what `location.href` reports,
    /// while relative URLs still resolve against the creator's base URL
    /// (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#process-the-iframe-attributes>,
    /// <https://html.spec.whatwg.org/multipage/browsers.html#determining-the-origin>,
    /// <https://html.spec.whatwg.org/multipage/urls-and-fetching.html#fallback-base-url>).
    pub(crate) fn load_about_blank(
        &mut self,
        inherited_url: Option<&str>,
        inherited_origin: Option<url::Origin>,
    ) {
        self.apply_blank_url(inherited_url, inherited_origin);
        // A blank document's referrer is its creator's URL, unredacted by any
        // policy: the initial `about:blank` was never fetched
        // (<https://html.spec.whatwg.org/multipage/browsers.html#creating-a-new-browsing-context>).
        self.world.borrow_mut().document_referrer =
            inherited_url.map_or_else(String::new, ToOwned::to_owned);
        // No HTTP response stands behind a blank document.
        self.http_status = 0;
        self.load_html("");
        self.initial_blank = true;
        self.ensure_js_ok();
    }

    /// Loads the frame's `about:srcdoc` document: `markup` parsed as HTML,
    /// the document URL `about:srcdoc`, and the creator's base URL, origin,
    /// referrer policy, and CSP list inherited. Relative URLs inside an
    /// `iframe srcdoc document` resolve against the creator's base URL,
    /// which the document's own URL cannot supply
    /// (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#attr-iframe-srcdoc>,
    /// <https://html.spec.whatwg.org/multipage/urls-and-fetching.html#fallback-base-url>).
    pub(crate) fn load_frame_srcdoc(
        &mut self,
        base: &Url,
        referrer_url: &Url,
        origin: url::Origin,
        policy: &PolicyContainer,
        markup: &str,
    ) {
        let url = Url::parse("about:srcdoc").expect("about:srcdoc parses");
        self.http_status = 0;
        self.set_document_url(url, Some(origin));
        {
            let mut world = self.world.borrow_mut();
            world.base_url = Some(base.clone());
            world.referrer_url = referrer_url.clone();
            // A `srcdoc` document's referrer is its creator's URL, like the
            // initial `about:blank` it replaces
            // (<https://html.spec.whatwg.org/multipage/browsers.html#creating-a-new-browsing-context>).
            world.document_referrer = referrer_url.to_string();
        }
        self.inherited_policy = Some(policy.clone());
        self.initial_blank = false;
        self.load_html(markup);
    }

    /// Installs the initial `about:blank` tree without creating a realm
    /// (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#process-the-iframe-attributes>).
    ///
    /// Used when a parent script inserts an `iframe`: the document must exist
    /// for `contentDocument` in the same task, but `QuickJS` cannot enter a new
    /// realm until that script returns.
    pub(crate) fn load_about_blank_tree(
        &mut self,
        inherited_url: Option<&str>,
        inherited_origin: Option<url::Origin>,
    ) {
        self.apply_blank_url(inherited_url, inherited_origin);
        // The initial blank reports its creator's URL as its referrer
        // (<https://html.spec.whatwg.org/multipage/browsers.html#creating-a-new-browsing-context>).
        self.world.borrow_mut().document_referrer =
            inherited_url.map_or_else(String::new, ToOwned::to_owned);
        self.defer_js = true;
        self.load_html("");
        self.defer_js = false;
        self.initial_blank = true;
    }

    /// Whether the frame is still on the document created at insertion.
    #[must_use]
    pub(crate) fn is_initial_blank(&self) -> bool {
        self.initial_blank
    }

    /// Runs one `javascript:` frame URL's script in the frame's current realm,
    /// replacing the document only when the result is a string. A void
    /// result leaves the document (and its realm) untouched; the script
    /// observes the page's own state, not a fresh blank realm. The result
    /// document keeps the target's URL
    /// (<https://html.spec.whatwg.org/multipage/browsing-the-web.html#javascript-protocol>).
    pub(crate) fn load_javascript_frame(
        &mut self,
        script: &str,
        source: &str,
        policy: &crate::csp::List,
    ) {
        if self.js.is_none() {
            self.ensure_js_ok();
        }
        self.eval_frame_script(script, source, policy);
    }

    /// Runs one `javascript:` URL script in this frame's realm. A string
    /// result replaces the document with that string parsed as HTML
    /// (<https://html.spec.whatwg.org/multipage/browsing-the-web.html#javascript-protocol>).
    ///
    /// `source` is the serialized `javascript:` URL, which is what CSP source
    /// matching and hashes use
    /// (<https://w3c.github.io/webappsec-csp/#should-block-navigation-request>).
    pub(crate) fn eval_frame_script(
        &mut self,
        script: &str,
        source: &str,
        policy: &crate::csp::List,
    ) {
        if script.is_empty() {
            return;
        }
        let violations = Self::navigation_script_violations(policy, &self.url, source);
        if !violations.is_empty()
            && let Some(target) = self.document_node()
        {
            self.fire_csp_violations(target, &violations);
        }
        if crate::csp::blocks(&violations) {
            return;
        }
        match self.eval(script) {
            Ok(result) if !result.is_empty() => self.load_html(&result),
            Ok(_) => {}
            Err(_) => self.record_event(RendererEvent::ScriptFailed),
        }
    }

    /// Writes the document URL and its captured origin together.
    ///
    /// [`Self::url`] and [`World::document_url`] are mirrors, and origin
    /// checks read the captured origin rather than the URL's serialization.
    /// A navigation that writes one and not the other leaves the document
    /// reporting a stale URL or origin to the next frame.
    fn set_document_url(&mut self, url: Url, origin: Option<url::Origin>) {
        let mut world = self.world.borrow_mut();
        match origin {
            Some(origin) => world.set_url_with_origin(url.clone(), origin),
            None => world.set_url(url.clone()),
        }
        drop(world);
        self.url = url;
    }

    fn apply_document_url_same_origin(&mut self, url: &str) {
        if let Ok(url) = Url::parse(url) {
            let origin = self.world.borrow().origin.clone();
            self.set_document_url(url, Some(origin));
        }
    }

    /// Sets the realm-visible session history of a new document, before its
    /// realm runs so the `history` bindings read the fresh entry.
    pub(crate) fn set_history(&mut self, history: &crate::protocol::HistorySnapshot) {
        self.world.borrow_mut().history = history.clone();
    }

    /// Restores a same-document session history entry while preserving the realm.
    /// <https://html.spec.whatwg.org/multipage/browsing-the-web.html#update-document-for-history-step-application>
    pub(crate) fn traverse_history(
        &mut self,
        url: &str,
        history: &crate::protocol::HistorySnapshot,
    ) -> Result<(), TabError> {
        let url = Url::parse(url).map_err(|_| TabError::InvalidUrl {
            spec: url.to_owned(),
        })?;
        if !self.world.borrow().url_matches_origin(&url) {
            return Err(TabError::InvalidUrl { spec: url.into() });
        }
        let previous_url = self.url.clone();
        let previous_history = self.world.borrow().history.clone();
        self.apply_document_url_same_origin(url.as_str());
        self.world.borrow_mut().history = history.clone();
        let restored_state = self
            .js
            .as_ref()
            .map(|js| js.restore_history(history.state.as_deref(), history.length))
            .transpose();
        match restored_state {
            Ok(Some(state)) => self.fire_js(|js| js.fire_popstate(&state)),
            Ok(None) => {}
            Err(error) => {
                self.apply_document_url_same_origin(previous_url.as_str());
                self.world.borrow_mut().history = previous_history;
                return Err(TabError::from(error));
            }
        }
        Ok(())
    }

    /// Queues one posted window message as a task on this frame's task source
    /// (<https://html.spec.whatwg.org/multipage/web-messaging.html#posted-message-task-source>).
    pub(crate) fn push_window_message(&mut self, message: WindowMessage) {
        self.tasks.push_back(Task::WindowMessage(message));
    }

    /// Queues one `storage` event as a task on this frame's task source
    /// (<https://html.spec.whatwg.org/multipage/webstorage.html#concept-storage-broadcast>).
    pub(crate) fn push_storage_event(&mut self, event: crate::storage::PendingStorageEvent) {
        self.tasks.push_back(Task::StorageEvent(event));
    }

    /// Queues one cross-tab `message` as a task on this frame's task source
    /// (<https://html.spec.whatwg.org/multipage/web-messaging.html#posted-message-task-source>).
    pub(crate) fn push_remote_message(&mut self, payload: String) {
        self.tasks.push_back(Task::RemoteMessage(payload));
    }

    /// Queues one `BroadcastChannel` message on this frame's task source
    /// (<https://html.spec.whatwg.org/multipage/web-messaging.html#broadcasting-to-other-browsing-contexts>).
    pub(crate) fn push_broadcast_message(
        &mut self,
        origin: String,
        name: String,
        payload: String,
        source: Option<u64>,
    ) {
        self.tasks.push_back(Task::BroadcastMessage {
            origin,
            name,
            payload,
            source,
        });
    }

    /// Queues one channel message as a task on this frame's task source.
    pub(crate) fn push_port_message(&mut self, endpoint: u64, payload: String, ports: Vec<u64>) {
        self.tasks.push_back(Task::PortMessage {
            endpoint,
            payload,
            ports,
        });
    }

    /// Queues a `close` event for one channel endpoint.
    pub(crate) fn push_port_closed(&mut self, endpoint: u64) {
        self.tasks.push_back(Task::PortClosed { endpoint });
    }

    /// Dispatches one cross-tab `message` event at this frame's window.
    fn deliver_remote_message(&mut self, payload: &str) {
        if !self.ensure_js_ok() {
            return;
        }
        self.fire_js(|js| js.deliver_remote_message(payload));
        self.adopt_js_work();
    }

    /// Dispatches one `BroadcastChannel` message to this frame's channels.
    fn deliver_broadcast_message(
        &mut self,
        origin: &str,
        name: &str,
        payload: &str,
        source: Option<u64>,
    ) {
        if !self.ensure_js_ok() {
            return;
        }
        self.fire_js(|js| js.deliver_broadcast_message(origin, name, payload, source));
        self.adopt_js_work();
    }

    /// Fires one `storage` event at this frame's window
    /// (<https://html.spec.whatwg.org/multipage/webstorage.html#concept-storage-broadcast>).
    fn deliver_storage_event(&mut self, event: &crate::storage::PendingStorageEvent) {
        if !self.ensure_js_ok() {
            return;
        }
        self.fire_js(|js| js.fire_storage_event(event));
        self.adopt_js_work();
    }

    fn deliver_window_message(&mut self, message: &WindowMessage) {
        if !self.ensure_js_ok() {
            return;
        }
        let delivered = match &self.js {
            Some(js) => js.deliver_window_message(
                message.source.get(),
                &message.origin,
                &message.payload,
                &message.ports,
            ),
            None => return,
        };
        match delivered {
            Ok(true) => {}
            Ok(false) => self.fire_js(|js| {
                js.deliver_window_message_error(message.source.get(), &message.origin)
            }),
            Err(_) => self.record_event(RendererEvent::ScriptFailed),
        }
        self.adopt_js_work();
    }

    fn deliver_port_message(&mut self, endpoint: u64, payload: &str, ports: &[u64]) {
        // The port may have moved to another realm between the message being
        // queued and this task; it goes back on the port's queue, which the
        // new realm flushes when the port is enabled there
        // (<https://html.spec.whatwg.org/multipage/web-messaging.html#message-port-post-message-steps>).
        if self.endpoint_moved(endpoint) {
            let shared = self.shared.clone();
            let mut shared = shared.borrow_mut();
            shared.requeue_port_message(endpoint, payload.to_owned(), ports.to_vec());
            return;
        }
        if !self.ensure_js_ok() {
            return;
        }
        let delivered = match &self.js {
            Some(js) => js.deliver_port_message(endpoint, payload, ports),
            None => return,
        };
        match delivered {
            Ok(true) => {}
            Ok(false) => self.fire_js(|js| js.deliver_port_message_error(endpoint)),
            Err(_) => self.record_event(RendererEvent::ScriptFailed),
        }
        self.adopt_js_work();
    }

    fn deliver_port_close(&mut self, endpoint: u64) {
        if self.endpoint_moved(endpoint) {
            let shared = self.shared.clone();
            shared.borrow_mut().requeue_port_close(endpoint);
            return;
        }
        if !self.ensure_js_ok() {
            return;
        }
        self.fire_js(|js| js.deliver_port_close(endpoint));
        self.adopt_js_work();
    }

    /// Whether `endpoint` no longer belongs to this frame, either because it
    /// moved to another realm or because it is in transit.
    ///
    /// A port can move between the moment a delivery is queued and the moment
    /// its task runs; the delivery then belongs to the new owner's realm
    /// (<https://html.spec.whatwg.org/multipage/web-messaging.html#transfer-receiving-steps>).
    fn endpoint_moved(&self, endpoint: u64) -> bool {
        let shared = self.shared.borrow();
        shared.ports.owner(endpoint) != Some(self.frame)
    }

    /// Document URL (cookie initiator and relative-URL base).
    #[must_use]
    pub(crate) fn document_url(&self) -> &str {
        self.url.as_str()
    }

    /// Evaluates `source` as classic script on this document's JS context.
    ///
    /// # Errors
    ///
    /// [`TabError::Script`] when the engine cannot start or the script throws.
    pub(crate) fn eval(&mut self, source: &str) -> Result<String, TabError> {
        self.adopt_pending_frames();
        self.ensure_js()?;
        let Some(js) = self.js.as_ref() else {
            return Err(TabError::Script(ScriptFailure::HostMissing));
        };
        let out = js.eval(source).map_err(TabError::from);
        self.adopt_js_work();
        out
    }

    pub(crate) fn execute_script_deadline(
        &mut self,
        source: crate::ScriptSource<&str>,
        deadline: Option<WallClock>,
    ) -> Result<ScriptValue, TabError> {
        self.ensure_js()?;
        let Some(js) = self.js.as_ref() else {
            return Err(TabError::Script(ScriptFailure::HostMissing));
        };
        let out = js
            .eval_value_deadline(source, deadline)
            .map_err(TabError::from);
        self.adopt_js_work();
        out
    }

    pub(crate) fn take_events(&mut self) -> Result<Vec<RendererEvent>, ()> {
        if std::mem::take(&mut self.events_overflowed) {
            self.events.clear();
            return Err(());
        }
        Ok(std::mem::take(&mut self.events))
    }

    /// Replaces the document from a host mount: new realm, decoded bytes,
    /// parsed to load. The host has already dialed and chosen this renderer.
    pub(crate) fn mount(&mut self, mount: &Mount) -> Result<(), TabError> {
        // An invalid mount spec fails before anything is touched.
        if Url::parse(&mount.head.url).is_err() {
            return Err(TabError::InvalidUrl {
                spec: mount.head.url.clone(),
            });
        }
        self.set_history(&mount.head.history);
        self.set_document_referrer("");
        self.install_response(&mount.head, &mount.body, None);
        Ok(())
    }

    /// Opens a parse on `input`, taking ownership of the input stream.
    ///
    /// `decoder` is set exactly when bytes still arrive from the carrier; a
    /// script's `document.write` feeds text directly instead. The markup only
    /// accumulates here; `finish_parse` parses it whole.
    fn start_parser(
        &mut self,
        input: &str,
        eof: bool,
        owner: ParserOwner,
        decoder: Option<dial::ResponseDecoder>,
    ) {
        self.world.borrow_mut().parser_active = true;
        self.parser_eof = eof;
        self.decoder = decoder;
        self.parser_owner = owner;
        self.response_content_type = None;
        self.response_policy = crate::csp::List::default();
        self.response_referrer_policy = None;
        self.csp_from_response = false;
        self.sniff_header.clear();
        self.apply_mime_sniff = false;
        self.mime_guard = crate::sniff::MimeGuard::default();
        self.active_parse = Some(ActiveParse {
            buffer: input.to_owned(),
            delay: self.load_delay.guard(),
        });
        // A new parse starts undecided: without this an `about:blank` or
        // `document.open()` document inherits the previous response's
        // charset, and `document.characterSet` lies. The response decoder,
        // when one exists, reports the real charset at EOF.
        self.character_set = "UTF-8";
    }

    /// One funnel: opens the realm from a response head — new realm, URL
    /// (with an optional captured origin), status, CSP policy, MIME guard,
    /// and reporting endpoints — then opens a carrier-owned parser that
    /// consumes bytes until [`Self::end_body`].
    ///
    /// Every response-install path runs the same derivation in one order
    /// here; callers only choose when the body arrives. A URL that does not
    /// parse leaves the document on its previous URL, as a streamed response
    /// with an unusable final URL does.
    pub(crate) fn install_head(&mut self, head: &ResponseHead, origin: Option<url::Origin>) {
        self.reset_js_realm(false);
        // A new response brings its own HTTP status: documents without one
        // (blank, data) report zero rather than a previous page's status.
        // Violation `status-code` reports it
        // (<https://w3c.github.io/webappsec-csp/#create-violation-for-global>).
        self.http_status = head.status;
        self.set_reporting_endpoints(head);
        if let Some((width, height)) = head.viewport {
            self.viewport_size = (width, height);
        }
        // The parsed URL decides. An unparseable response URL leaves the
        // previous URL in place, and with it the captured origin: `about:blank`,
        // `data:`, and `blob:` URLs cannot re-derive one.
        self.initial_blank = false;
        // A `blob:` response carries its store entry's creator, which is not
        // what `Url::origin()` returns for the blob URL
        // (<https://w3c.github.io/FileAPI/#blob-url-entry>,
        // <https://html.spec.whatwg.org/multipage/browsers.html#determining-the-origin>).
        match Url::parse(&head.url) {
            Ok(url) => self.set_document_url(url, origin),
            Err(_) => {
                if let Some(origin) = origin {
                    self.set_document_url(self.url.clone(), Some(origin));
                }
            }
        }
        let mut world = self.world.borrow_mut();
        // A response document resolves against its own URL; any inherited
        // `about:srcdoc` base stops applying.
        world.base_url = None;
        drop(world);
        // The realm is already new when this runs; this opens the parser and
        // the decoder for the response that will feed it.
        self.start_parser(
            "",
            false,
            ParserOwner::Carrier,
            // The decoder resolves the charset from the extracted MIME type,
            // not from the header value as served: a `charset` in
            // `a;charset=x, b` does not decode `b`
            // (<https://fetch.spec.whatwg.org/#concept-header-extract-mime-type>).
            Some(dial::ResponseDecoder::new(
                head.content_type
                    .as_deref()
                    .and_then(crate::sniff::extract_mime_type),
            )),
        );
        // Kept raw until `finish_parse`: the Apache `text/plain` check needs
        // the exact bytes the server sent before the extraction runs
        // (<https://mimesniff.spec.whatwg.org/#supplied-mime-type-detection-algorithm>).
        self.response_content_type.clone_from(&head.content_type);
        self.response_policy = head.inherited_policy.clone().unwrap_or_else(|| {
            crate::csp::List::from_headers(
                head.content_security_policy.as_deref(),
                head.content_security_policy_report_only.as_deref(),
            )
        });
        self.response_referrer_policy = head.inherited_referrer_policy.or(head.referrer_policy);
        self.csp_from_response = true;
        self.apply_mime_sniff = true;
        self.mime_guard = crate::sniff::MimeGuard {
            no_sniff: head.no_sniff,
            http: head.http_response,
        };
        // A response `Refresh` starts the document's one declarative refresh
        // (<https://html.spec.whatwg.org/multipage/document-lifecycle.html#creating-and-initializing-a-document-object>).
        if let Some(refresh) = &head.refresh {
            self.start_declarative_refresh(refresh);
        }
    }

    /// Installs a complete response head and body: [`Self::install_head`]
    /// plus the whole body, ending the parse at once.
    fn install_response(&mut self, head: &ResponseHead, body: &[u8], origin: Option<url::Origin>) {
        self.install_head(head, origin);
        self.write_body(body);
        self.end_body();
    }

    /// Installs a complete response into this frame's document, as the
    /// frame's own `src` load or a committed child-frame response delivers.
    pub(crate) fn install_frame_response(
        &mut self,
        head: &ResponseHead,
        body: &[u8],
        origin: Option<url::Origin>,
    ) {
        self.install_response(head, body, origin);
        // Nested HTML documents without scripts skip a realm at parse;
        // `fire_ready_frame_loads` creates one when the parent is not in the
        // encoding-label burst
        // (<https://html.spec.whatwg.org/multipage/window-object.html#the-window-object>).
        if self.js.is_none() && self.shared.borrow().tree.parent(self.frame).is_none() {
            self.ensure_js_ok();
        }
    }

    /// Installs the response's `Reporting-Endpoints` endpoints: a violated
    /// policy's `report-to` directive resolves its endpoint names against
    /// them (<https://w3c.github.io/reporting/#header>). A refused
    /// `frame-ancestors` response dispatches its violations on the fallback
    /// document against the new response's endpoints, so this is also set
    /// before the check.
    fn set_reporting_endpoints(&mut self, head: &ResponseHead) {
        self.reporting_endpoints = head
            .reporting_endpoints
            .as_deref()
            .map(crate::csp::parse_reporting_endpoints_value)
            .unwrap_or_default();
    }

    /// Feeds response bytes. The decoder holds them until the encoding is
    /// known, then the parser sees the markup.
    pub(crate) fn write_body(&mut self, bytes: &[u8]) {
        if self.sniff_header.len() < crate::sniff::RESOURCE_HEADER_LEN {
            let room = crate::sniff::RESOURCE_HEADER_LEN - self.sniff_header.len();
            self.sniff_header.extend(bytes.iter().copied().take(room));
        }
        let Some(decoder) = self.decoder.as_mut() else {
            return;
        };
        let text = decoder.push(bytes);
        self.write_text(&text);
    }

    /// Feeds markup that is already decoded, as a script's `document.write`
    /// provides.
    pub(crate) fn write_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if let Some(parse) = self.active_parse.as_mut() {
            parse.buffer.push_str(text);
        }
        // Known gap: without an incremental parser a write after the parse
        // finished has nowhere to go, so it is ignored instead of re-parsing.
    }

    /// Ends a body: flushes the decoder and lets the parser finish.
    pub(crate) fn end_body(&mut self) {
        if let Some(decoder) = self.decoder.take() {
            let (text, encoding) = decoder.finish();
            self.character_set = encoding;
            self.write_text(&text);
        }
        self.parser_eof = true;
        self.finish_parse();
    }

    /// Aborts a body the carrier will not finish.
    ///
    /// Without this the frame waits for bytes that never come, so its load
    /// never fires and the tab stays loading forever.
    pub(crate) fn abort_body(&mut self) {
        self.decoder = None;
        self.parser_eof = true;
        self.finish_parse();
    }

    /// A script's `document.open()`: a new realm, a parser the script owns, and
    /// any live response aborted, because the script now holds the input
    /// stream.
    ///
    /// <https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#document-open-steps>
    pub(crate) fn reopen_document(&mut self) {
        self.reset_js_realm(true);
        // A script writes text, not bytes, and it takes the input stream away
        // from any live response: dropping the decoder ignores the rest of
        // that body instead of decoding it with the wrong charset.
        self.start_parser("", false, ParserOwner::Script, None);
    }

    /// Parses `input` into this document and starts a new JS realm.
    ///
    /// The caller already holds the whole document, so the parser starts at
    /// EOF with the markup in hand and no decoder, but the carrier owns this
    /// parser: `document.close()` must not end it.
    pub(crate) fn load_html(&mut self, input: &str) {
        self.reset_js_realm(false);
        self.start_parser(input, true, ParserOwner::Carrier, None);
        self.finish_parse();
    }

    /// Records `document` as this realm's so wrappers resolve its owner.
    fn register_document(&self, document: u32) {
        let registry = self.world.borrow().registry();
        registry.borrow_mut().insert_document(document, &self.world);
    }

    /// Runs one realm operation and records a failed script when it throws.
    fn fire_js(
        &mut self,
        operation: impl FnOnce(&crate::js::JsRealm) -> Result<(), crate::js::JsError>,
    ) {
        let failed = self.js.as_ref().is_some_and(|js| operation(js).is_err());
        if failed {
            self.record_event(RendererEvent::ScriptFailed);
        }
    }

    /// Ensures the frame's realm exists, recording a failed script when it
    /// cannot be created.
    pub(crate) fn ensure_js_ok(&mut self) -> bool {
        if self.ensure_js().is_err() {
            self.record_event(RendererEvent::ScriptFailed);
            return false;
        }
        true
    }

    fn ensure_js(&mut self) -> Result<(), TabError> {
        if self.js.is_none() {
            self.js = Some(
                crate::js::JsRealm::new(
                    &self.js_runtime,
                    self.world.clone(),
                    Arc::clone(&self.stop),
                )
                .map_err(TabError::from)?,
            );
            if self.decoder.is_some() && self.active_parse.is_some() {
                // The realm exists before the new document installs; defer the
                // init scripts until `bind_realm_document`.
                self.init_scripts_deferred = true;
            } else {
                self.run_init_scripts();
            }
        }
        Ok(())
    }

    /// Runs every registered init script in the fresh realm, before any page
    /// script. A failing script is reported, not fatal.
    fn run_init_scripts(&mut self) {
        let _turn = self.js.as_ref().map(crate::js::JsRealm::enter_turn);
        let scripts: Vec<String> = self
            .world
            .borrow()
            .init_scripts
            .borrow()
            .iter()
            .map(|(_, source)| source.clone())
            .collect();
        for source in scripts {
            let failed = match self.js.as_ref() {
                Some(js) => js.eval(&source).is_err(),
                None => return,
            };
            if failed {
                self.record_event(RendererEvent::ScriptFailed);
            }
        }
    }

    /// Runs one init script in this frame's realm immediately, creating the
    /// realm when needed (`Page.addScriptToEvaluateOnNewDocument` with
    /// `runImmediately`).
    pub(crate) fn run_init_script(&mut self, source: &str) {
        if !self.ensure_js_ok() {
            return;
        }
        let failed = match self.js.as_ref() {
            Some(js) => js.eval(source).is_err(),
            None => true,
        };
        if failed {
            self.record_event(RendererEvent::ScriptFailed);
        }
    }

    fn reset_js_realm(&mut self, keep_stream_writes: bool) {
        let old = std::mem::replace(
            &mut self.dials,
            DialLedger::new(Arc::clone(&self.stop), Arc::clone(&self.wake)),
        );
        old.terminate();
        self.load_delay = LoadDelay::default();
        // A pending `load` from the replaced document must not be reported as
        // the new document's readiness.
        self.events.retain(|event| {
            !matches!(event, RendererEvent::Load | RendererEvent::DomContentLoaded)
        });
        // The old realm's ports are gone with it; the peers fire `close`
        // (<https://html.spec.whatwg.org/multipage/web-messaging.html#disentangle>).
        // A navigation also destroys this frame's own children.
        {
            let mut shared = self.shared.borrow_mut();
            shared.close_frame_ports(self.frame);
            shared.detach_child_frames(self.frame);
        }
        self.load_fired = false;
        self.will_declaratively_refresh = false;
        self.pending_refresh = None;
        if let Some(id) = self.refresh_timer.take() {
            self.timers.remove(&id);
        }
        self.refresh_target = None;
        self.pending_frame_loads.clear();
        for (_, handler) in self.blitz_handlers.drain() {
            Box::new(handler).bytes(String::new(), blitz_traits::net::Bytes::from(Vec::new()));
        }
        self.blitz_request_urls.clear();
        while self.blitz_fetch_rx.try_recv().is_ok() {}
        self.subresource_bytes.clear();
        self.style_policies.clear();
        self.world.borrow_mut().clear_images();
        self.image_selected_src.clear();
        self.in_flight_images.clear();
        self.connected_images.clear();
        self.timers.retain(|_, timer| timer.js_id.is_none());
        self.js_timers.clear();
        self.js = None;
        self.init_scripts_deferred = false;
        self.active_parse = None;
        self.stream_publish = None;
        self.executed_scripts.clear();
        self.parser_eof = true;
        self.world.borrow_mut().parser_active = false;
        self.world.borrow_mut().state.current_script = None;
        if keep_stream_writes {
            self.world.borrow().clear_outbox_for_open();
        } else {
            self.world.borrow().clear_outbox();
        }
        self.parser_blocked_script = None;
        self.deferred_modules.clear();
    }

    /// `document.open()`, `document.write()`, and `document.close()` all land
    /// on the same parser feed the network path uses, so a pending classic
    /// script blocks them the same way.
    pub(crate) fn apply_document_stream(&mut self, command: DocumentStreamCommand) {
        match command {
            DocumentStreamCommand::Open => self.reopen_document(),
            // Only a parser a script opened is a script's to close; a response
            // body ends when its exchange does.
            // https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#closing-the-input-stream
            DocumentStreamCommand::Close => {
                if self.parser_owner == ParserOwner::Script {
                    self.end_body();
                }
            }
        }
    }

    fn eval_classic(
        &mut self,
        source: &str,
        element: Option<crate::dom::tree::NodeId>,
        base_line: u32,
        filename: &str,
    ) {
        // https://html.spec.whatwg.org/multipage/webappapis.html#run-a-classic-script
        let previous = self.world.borrow().state.current_script;
        self.world.borrow_mut().state.current_script = element;
        if !self.ensure_js_ok() {
            return;
        }
        self.fire_js(|js| js.eval_classic_script(source, base_line, filename));
        self.world.borrow_mut().state.current_script = previous;
        self.adopt_js_work();
    }

    /// Drains parser-driven mutation records at a microtask checkpoint
    /// (<https://html.spec.whatwg.org/multipage/webappapis.html#perform-a-microtask-checkpoint>).
    /// Parser insertions bypass the JS bindings that schedule delivery, so
    /// the document's `MutationObserver`s would otherwise not fire until the
    /// next scripted mutation.
    fn deliver_mutations(&mut self) {
        self.fire_js(crate::js::JsRealm::deliver_mutations);
    }

    /// The Blitz configuration every document of this frame shares: our net
    /// provider for subresources, a no-op shell (screenshots re-resolve
    /// explicitly), the frame's base URL, and the process-shared font
    /// context (fresh scans cost ~25ms per document).
    fn blitz_config(&self) -> blitz_dom::DocumentConfig {
        // Subresource URLs in the markup resolve against the document's base
        // URL, which for `about:srcdoc` is the creator's
        // (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#document-base-url>).
        let base_url = self.base_url();
        blitz_dom::DocumentConfig {
            net_provider: Some(std::sync::Arc::clone(&self.net_provider)
                as std::sync::Arc<dyn blitz_traits::net::NetProvider>),
            navigation_provider: Some(std::sync::Arc::clone(&self.blitz_nav)
                as std::sync::Arc<dyn blitz_traits::navigation::NavigationProvider>),
            shell_provider: Some(std::sync::Arc::new(crate::render::TinyShell)),
            base_url: Some(crate::render::blitz_base_url(&base_url)),
            font_ctx: Some(self.shared_font_ctx()),
            ..blitz_dom::DocumentConfig::default()
        }
    }

    /// The process-shared font context, cloned cheaply per document.
    fn shared_font_ctx(&self) -> parley::FontContext {
        self.world.borrow().runtime.font_ctx.clone()
    }

    /// Nested HTML without scripts defers a realm: encoding-label WPT files
    /// create one iframe per label, and installing `QuickJS` in each stalls
    /// those tests. Inline event handler content attributes need a realm too
    /// (`<body onload=...>`, `<img onerror=...>` compile into handler
    /// functions), as do registered init scripts, which evaluate in every
    /// new realm. XML documents always get a realm
    /// (<https://html.spec.whatwg.org/multipage/window-object.html#the-window-object>).
    fn skip_nested_html_realm(&self, parsed: &crate::Parsed) -> bool {
        if parsed.xml_document || html_needs_realm(parsed) {
            return false;
        }
        if !self.world.borrow().init_scripts.borrow().is_empty() {
            return false;
        }
        self.shared.borrow().tree.parent(self.frame).is_some()
    }

    /// Installs `parsed` as the active document and registers it, reporting
    /// whether a realm owns it afterwards.
    fn install_parsed(&mut self, parsed: Parsed) -> bool {
        if self.js.is_none() {
            let skip_js = self.defer_js || self.skip_nested_html_realm(&parsed);
            let document = self.world.borrow_mut().replace_document(parsed);
            self.register_document(document);
            // Style resolution and scripts both read the viewport; apply the
            // frame's size before the realm evaluates anything.
            self.apply_viewport();
            self.install_policy();
            self.mark_parser_scripts();
            if skip_js {
                return true;
            }
            self.ensure_js().is_ok()
        } else {
            let document = self.world.borrow_mut().set_document(parsed);
            self.register_document(document);
            self.apply_viewport();
            self.bind_realm_document();
            self.install_policy();
            self.mark_parser_scripts();
            true
        }
    }

    /// Marks every script element of the freshly parsed tree as
    /// parser-inserted
    /// (<https://html.spec.whatwg.org/multipage/scripting.html#parser-inserted>).
    fn mark_parser_scripts(&mut self) {
        let scripts: Vec<crate::dom::tree::NodeId> = {
            let world = self.world.borrow();
            let Some(parsed) = world.main_document() else {
                return;
            };
            let base = &parsed.document.base;
            let document = parsed.id;
            let mut scripts = Vec::new();
            let mut stack = vec![base.root_node().id];
            while let Some(node) = stack.pop() {
                let Some(tree) = base.get_node(node) else {
                    continue;
                };
                stack.extend(tree.children.iter().copied());
                if crate::dom::tree::is_html_element(base, node, "script") {
                    scripts.push(crate::dom::tree::NodeId { document, node });
                }
            }
            scripts
        };
        let mut world = self.world.borrow_mut();
        for id in scripts {
            world.mark_parser_inserted(id);
        }
    }

    /// Replaces the world's policy with the response headers when this parse
    /// came from a navigation. `document.open()` leaves the flag unset.
    fn install_policy(&mut self) {
        if self.csp_from_response {
            self.world.borrow_mut().policy = PolicyContainer {
                csp: self.response_policy.clone(),
                referrer_policy: self.response_referrer_policy,
            };
            return;
        }
        if let Some(policy) = self.inherited_policy.take() {
            self.world.borrow_mut().policy = policy;
        }
    }

    /// Rebinds the realm's `document` global to the installed document and
    /// runs init scripts deferred while the response streamed, so page code
    /// always sees the new tree under the same realm.
    fn bind_realm_document(&mut self) {
        if let Some(js) = self.js.as_ref()
            && js.refresh_document().is_err()
        {
            self.record_event(RendererEvent::ScriptFailed);
        }
        if std::mem::take(&mut self.init_scripts_deferred) {
            self.run_init_scripts();
        }
    }

    /// Applies this frame's viewport size to the active Blitz document.
    fn apply_viewport(&self) {
        let (width, height) = self.viewport_size;
        self.world.borrow().viewport_size.set((width, height));
        let world = self.world.borrow();
        let Some(mut parsed) = world.main_document_mut() else {
            return;
        };
        parsed
            .document
            .base
            .set_viewport(blitz_traits::shell::Viewport::new(
                width,
                height,
                1.0,
                blitz_traits::shell::ColorScheme::Light,
            ));
    }

    /// Persists a new viewport size for this frame and re-lays out the active
    /// document at it.
    pub(crate) fn set_viewport(&mut self, width: u32, height: u32) {
        self.viewport_size = (width, height);
        self.apply_viewport();
        {
            let world = self.world.borrow();
            if let Some(mut parsed) = world.main_document_mut() {
                crate::render::resolve_until_settled(&mut parsed.document.base);
            }
        }
        // Viewport changes can flip `matchMedia` results
        // (<https://drafts.csswg.org/cssom-view/#evaluate-media-queries-and-report-changes>).
        self.fire_js(super::js::JsRealm::report_media_changes);
    }

    /// Sets the frame's persisted viewport size without touching its current
    /// document, for callers that apply it during construction.
    pub(crate) fn set_viewport_size(&mut self, size: (u32, u32)) {
        self.viewport_size = size;
        self.world.borrow().viewport_size.set(size);
    }

    /// The active document's scrollable content size in CSS pixels, at least
    /// the frame's viewport
    /// (<https://chromedevtools.github.io/devtools-protocol/tot/Page/#method-getLayoutMetrics>).
    pub(crate) fn content_size(&mut self) -> (f32, f32) {
        let (viewport_width, viewport_height) = self.viewport_size;
        #[expect(
            clippy::cast_precision_loss,
            reason = "viewport sides are at most 4096, exactly representable"
        )]
        let mut content = (viewport_width as f32, viewport_height as f32);
        let world = self.world.borrow();
        let Some(mut parsed) = world.main_document_mut() else {
            return content;
        };
        let base = &mut parsed.document.base;
        crate::render::resolve_until_settled(base);
        // The document element is the scrolling element
        // (<https://drafts.csswg.org/cssom-view/#scrolling-element>); the
        // root's first child can be a doctype, which has no layout.
        let root = base.root_element();
        content.0 = content.0.max(root.scroll_width());
        content.1 = content.1.max(root.scroll_height());
        content
    }

    /// Parses the accumulated markup as one whole document, installs it, and
    /// runs its scripts in tree order
    /// (<https://html.spec.whatwg.org/multipage/parsing.html#the-end>).
    fn finish_parse(&mut self) {
        if !self.parser_eof {
            return;
        }
        self.adopt_js_work();
        let Some(ActiveParse { buffer, delay }) = self.active_parse.take() else {
            return;
        };
        drop(delay);
        // Writes queued while the parser was active join this parse; later
        // writes have no incremental parser to feed (see `write_text`).
        if self.apply_mime_sniff {
            // The stored value is the raw header list. The Apache check needs
            // it exactly as sent; the sniff needs the extracted MIME type.
            // mimesniff names "the value of the last Content-Type header" as
            // the supplied type, but browsers extract — last value that
            // parses, `*/*` skipped — and the WPT `fetch/content-type`
            // expectations follow the browsers
            // (<https://fetch.spec.whatwg.org/#concept-header-extract-mime-type>,
            // <https://mimesniff.spec.whatwg.org/#supplied-mime-type-detection-algorithm>).
            let raw = self.response_content_type.as_deref();
            let supplied = raw.and_then(crate::sniff::extract_mime_type);
            let apache =
                self.mime_guard.http && raw.is_some_and(crate::sniff::apache_bug_content_type);
            self.response_content_type = Some(crate::sniff::sniff_mime(
                &self.sniff_header,
                supplied.as_deref(),
                self.mime_guard.no_sniff,
                apache,
            ));
        }
        // XML MIME types use the XML parser. HTML, and a script's
        // `document.open()`, do not
        // (<https://html.spec.whatwg.org/multipage/parsing.html#xml-parser>,
        // <https://mimesniff.spec.whatwg.org/#xml-mime-type>).
        let parsed = if let Some(content_type) = self
            .response_content_type
            .as_deref()
            .and_then(crate::xml::navigated_content_type)
        {
            let mut parsed =
                crate::xml::parse_navigated(&buffer, content_type, self.blitz_config());
            parsed.character_set = self.character_set;
            // The parser needs the XML kind; `contentType` reports the
            // actual essence (`application/atom+xml`, not `application/xml`).
            if let Some(header) = self.response_content_type.as_deref() {
                parsed.content_type = crate::xml::document_content_type(header);
            }
            parsed
        } else {
            // `text/plain` is not parsed as markup: the response bytes become
            // the document's text verbatim, so character references such as
            // `&#601;` stay literal. Wrapping the escaped text in `pre` keeps
            // whitespace and gives the frame a body with that text
            // (<https://mimesniff.spec.whatwg.org/#sniffing-a-mime-type>,
            // <https://html.spec.whatwg.org/multipage/rendering.html#the-pre-element>).
            let plain = self
                .response_content_type
                .as_deref()
                .and_then(crate::sniff::extract_mime_type)
                .is_some_and(|mime| {
                    mime.split(';').next().is_some_and(|essence| {
                        essence.trim_ascii().eq_ignore_ascii_case("text/plain")
                    })
                });
            let markup = if plain {
                let escaped = buffer
                    .replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;");
                format!("<pre>\n{escaped}</pre>")
            } else {
                buffer
            };
            let mut parsed = crate::parse_html(&markup, self.blitz_config());
            parsed.character_set = self.character_set;
            // A non-XML response still records its MIME type on the
            // document. Images, style sheets, and plain text in a frame
            // parse as HTML wrapping, but `contentType` is the type
            // (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#concept-document-content-type>).
            if let Some(header) = self.response_content_type.as_deref() {
                parsed.content_type = crate::xml::document_content_type(header);
            }
            parsed
        };
        if !self.install_parsed(parsed) {
            self.record_event(RendererEvent::ScriptFailed);
            return;
        }
        self.world.borrow_mut().parser_active = false;
        // Microtask checkpoint before scripts run; parser mutations queued
        // since the last script deliver now. Parser iframes are adopted in
        // tree order with scripts, not all at once before the first fetch.
        self.deliver_mutations();
        self.resume_scripts();
    }

    /// Continues the document after a parse settled or a parser-blocking
    /// fetch completed: runs the remaining scripts, then the end-of-parse
    /// steps unless another fetch blocks the walk.
    fn resume_scripts(&mut self) {
        let _turn = self.js.as_ref().map(crate::js::JsRealm::enter_turn);
        if self.run_scripts() {
            return;
        }
        self.load_images();
        // Deliver parser mutations before the document's events.
        self.deliver_mutations();
        self.fire_document_end();
        let _ = self.check_load();
    }

    /// Appends one in-head `meta` policy the first time it is seen
    /// (<https://w3c.github.io/webappsec-csp/#meta-element>).
    fn apply_meta_csp(&mut self, id: crate::dom::tree::NodeId) {
        self.world.borrow_mut().apply_meta_policy(id);
    }

    /// A parser-inserted `<meta name=referrer>` sets the document's policy,
    /// mirroring the JS insertion path
    /// (<https://html.spec.whatwg.org/multipage/semantics.html#meta-referrer>).
    fn apply_meta_referrer(&mut self, id: crate::dom::tree::NodeId) {
        let policy = self.world.borrow().document(id).and_then(|parsed| {
            let base = &parsed.document.base;
            let name = crate::dom::tree::attr(base, id.node, "name").unwrap_or_default();
            if !name.eq_ignore_ascii_case("referrer") {
                return None;
            }
            let content = crate::dom::tree::attr(base, id.node, "content").unwrap_or_default();
            net::ReferrerPolicy::from_meta(content)
        });
        if let Some(policy) = policy {
            self.world.borrow_mut().policy.referrer_policy = Some(policy);
        }
    }

    pub(in crate::document) fn inline_script_violations(
        &self,
        source: &str,
        id: crate::dom::tree::NodeId,
    ) -> Vec<crate::csp::Violation> {
        let world = self.world.borrow();
        let slot = world.state.nonce_slots.get(&id).map(String::as_str);
        let parser_inserted = world.is_parser_inserted(id);
        let (nonce, nonceable) = world.document(id).map_or_else(
            || (String::new(), false),
            |parsed| crate::csp::element_nonce(&parsed.document.base, id.node, slot),
        );
        world.policy.csp.check_inline(
            &self.url,
            &crate::csp::InlineScript {
                source,
                nonce: &nonce,
                nonceable,
                parser_inserted,
                kind: crate::csp::InlineKind::Script,
            },
        )
    }

    fn navigation_script_violations(
        policy: &crate::csp::List,
        document_url: &Url,
        source: &str,
    ) -> Vec<crate::csp::Violation> {
        policy.check_inline(
            document_url,
            &crate::csp::InlineScript {
                source,
                nonce: "",
                nonceable: false,
                parser_inserted: false,
                kind: crate::csp::InlineKind::Navigation,
            },
        )
    }

    pub(in crate::document) fn script_url_violations(
        &self,
        url: &Url,
        id: crate::dom::tree::NodeId,
    ) -> Vec<crate::csp::Violation> {
        let world = self.world.borrow();
        let slot = world.state.nonce_slots.get(&id).map(String::as_str);
        let parser_inserted = world.is_parser_inserted(id);
        let (nonce, integrity) = world.document(id).map_or_else(
            || (String::new(), String::new()),
            |parsed| {
                let (nonce, _) = crate::csp::element_nonce(&parsed.document.base, id.node, slot);
                let integrity = crate::dom::tree::attr(&parsed.document.base, id.node, "integrity")
                    .unwrap_or_default()
                    .to_owned();
                (nonce, integrity)
            },
        );
        world.policy.csp.check_request(
            &world.origin,
            &world.document_url,
            url,
            crate::csp::Destination::Script,
            Some(crate::csp::ScriptRequest {
                nonce: &nonce,
                parser_inserted,
                integrity: &integrity,
            }),
        )
    }

    /// The main document's root node, the violation target for checks that
    /// are not attributable to an element.
    pub(crate) fn document_node(&self) -> Option<crate::dom::tree::NodeId> {
        self.world
            .borrow()
            .with_main_document(|parsed| crate::dom::tree::NodeId {
                document: parsed.id,
                node: parsed.document.base.root_node().id,
            })
    }

    /// A JSON string literal: `"` and `\` plus the control escapes a report
    /// body needs. The report fields are URLs and policy text, so full
    /// Unicode passes through unescaped.
    fn json_string(value: &str) -> String {
        use std::fmt::Write as _;
        let mut out = String::with_capacity(value.len() + 2);
        out.push('"');
        for c in value.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                c if (c as u32) < 0x20 => {
                    let _ = write!(out, "\\u{:04x}", c as u32);
                }
                c => out.push(c),
            }
        }
        out.push('"');
        out
    }

    /// Dispatches the `securitypolicyviolation` events a check produced,
    /// and queues the legacy `report-uri` POSTs. Reports are fire-and-forget
    /// dials: the response is dropped, they never gate load, and they carry
    /// no CSP gate of their own, so a restrictive policy cannot silence its
    /// own reports
    /// (<https://w3c.github.io/webappsec-csp/#report-violation>).
    pub(crate) fn fire_csp_violations(
        &mut self,
        target: crate::dom::tree::NodeId,
        violations: &[crate::csp::Violation],
    ) {
        if violations.is_empty() {
            return;
        }
        let mut enriched: Vec<crate::csp::Violation> = violations.to_vec();
        {
            let world = self.world.borrow();
            for violation in &mut enriched {
                // A fetch violation attributes to the element whose trigger
                // (insertion or connected `src` set) the executing script ran.
                // Inline and `eval` violations carry their own source (sample
                // or the eval hook's capture), so the creator map stays out.
                if violation.source_file.is_none()
                    && violation.blocked_uri != "inline"
                    && violation.blocked_uri != "eval"
                    && let Some(creator) = world.state.creator_stacks.get(&target)
                {
                    violation.source_file = Some(creator.url.clone());
                    violation.line_number = creator.line;
                    violation.column_number = creator.column;
                }
                if violation.status_code == 0 {
                    violation.status_code = self.http_status;
                }
            }
        }
        self.fire_js(|js| js.fire_csp_violations(target, &enriched));
        self.queue_csp_reports(&enriched);
    }

    /// Queues the `report-uri` / `report-to` POSTs for violations without
    /// firing DOM events. `frame-ancestors` response violations report this
    /// way: they are created with a null global, so no event target exists
    /// (<https://w3c.github.io/webappsec-csp/#should-navigation-response-to-navigation-request-of-type-in-target-be-blocked-by-content-security-policy>,
    /// <https://w3c.github.io/webappsec-csp/#report-violation>).
    pub(crate) fn queue_csp_reports(&mut self, violations: &[crate::csp::Violation]) {
        for violation in violations {
            // A `report-to` name the response's `Reporting-Endpoints`
            // defined takes the report as a Reporting API `csp-violation`
            // POST and suppresses `report-uri`
            // (<https://w3c.github.io/webappsec-csp/#directive-report-to>).
            if let Some(url) = self.reporting_url(&violation.report_to) {
                let body = Self::reporting_api_body(violation);
                self.queue_report(&url, body, "application/reports+json");
                continue;
            }
            if violation.report_uris.is_empty() {
                continue;
            }
            // The legacy `report-uri` serialization, built without a JSON
            // dependency: every field is a string except `status-code`,
            // `line-number`, and `column-number`. `source-file` and its line
            // and column only appear when a source file exists
            // (<https://w3c.github.io/webappsec-csp/#deprecated-serialize-violation>).
            let source_fields =
                violation
                    .source_file
                    .as_deref()
                    .map_or_else(String::new, |source| {
                        format!(
                            ",\"source-file\":{},\"line-number\":{},\"column-number\":{}",
                            Self::json_string(&crate::csp::strip_url_for_reports(source)),
                            violation.line_number,
                            violation.column_number,
                        )
                    });
            let body = format!(
                "{{\"csp-report\":{{\"document-uri\":{},\"referrer\":\"\",\"blocked-uri\":{},\"effective-directive\":{},\"violated-directive\":{},\"original-policy\":{},\"disposition\":{},\"status-code\":{},\"script-sample\":{}{}}}}}",
                Self::json_string(&violation.document_uri),
                Self::json_string(&violation.blocked_uri),
                Self::json_string(&violation.effective_directive),
                Self::json_string(&violation.effective_directive),
                Self::json_string(&violation.original_policy),
                Self::json_string(violation.disposition.as_str()),
                violation.status_code,
                Self::json_string(violation.sample.as_deref().unwrap_or_default()),
                source_fields,
            )
            .into_bytes();
            for endpoint in &violation.report_uris {
                let Ok(url) = self.url.join(endpoint) else {
                    continue;
                };
                if !matches!(url.scheme(), "http" | "https") {
                    continue;
                }
                self.queue_report(&url, body.clone(), "application/csp-report");
            }
        }
    }

    /// First `Reporting-Endpoints` URL named by `report-to`, resolved
    /// against the document URL. `None` falls back to `report-uri`.
    fn reporting_url(&self, names: &[String]) -> Option<Url> {
        for name in names {
            let url = self
                .reporting_endpoints
                .iter()
                .find(|(endpoint, _)| endpoint == name)
                .map(|(_, url)| url)?;
            let Ok(url) = self.url.join(url) else {
                continue;
            };
            if matches!(url.scheme(), "http" | "https") {
                return Some(url);
            }
        }
        None
    }

    /// Queues one violation report POST. The response is dropped and the
    /// dial never gates load.
    fn queue_report(&mut self, url: &Url, body: Vec<u8>, content_type: &str) {
        let mut dial = QueuedDial::new(
            DialContext::CspReport,
            url,
            &self.dial_initiator(),
            &self.referrer_url(),
        );
        "POST".clone_into(&mut dial.req.method);
        dial.req.body = body;
        dial.req.content_type = Some(content_type.to_owned());
        self.queue_dial(dial);
    }

    /// The Reporting API envelope for a violation, with the
    /// `CSPViolationReportBody` camelCase fields. The endpoint receives a
    /// list of reports, even when only one fired
    /// (<https://w3c.github.io/webappsec-csp/#cspviolationreportbody>,
    /// <https://w3c.github.io/reporting/#send-reports>).
    fn reporting_api_body(violation: &crate::csp::Violation) -> Vec<u8> {
        let sample = violation
            .sample
            .as_deref()
            .map_or_else(|| "null".to_owned(), Self::json_string);
        let source_file = violation.source_file.as_deref().map_or_else(
            || "null".to_owned(),
            |source| Self::json_string(&crate::csp::strip_url_for_reports(source)),
        );
        let (line_number, column_number) = violation.source_file.as_deref().map_or_else(
            || ("null".to_owned(), "null".to_owned()),
            |_| {
                (
                    violation.line_number.to_string(),
                    violation.column_number.to_string(),
                )
            },
        );
        format!(
            "[{{\"type\":\"csp-violation\",\"age\":0,\"url\":{},\"user_agent\":\"tinybrowser\",\"body\":{{\"documentURL\":{},\"referrer\":\"\",\"blockedURL\":{},\"effectiveDirective\":{},\"originalPolicy\":{},\"sourceFile\":{},\"lineNumber\":{},\"columnNumber\":{},\"sample\":{},\"disposition\":{},\"statusCode\":{}}}}}]",
            Self::json_string(&violation.document_uri),
            Self::json_string(&violation.document_uri),
            Self::json_string(&violation.blocked_uri),
            Self::json_string(&violation.effective_directive),
            Self::json_string(&violation.original_policy),
            source_file,
            line_number,
            column_number,
            sample,
            Self::json_string(violation.disposition.as_str()),
            violation.status_code,
        )
        .into_bytes()
    }

    /// Runs every not-yet-executed script in tree order, stopping (returning
    /// true) when a `src` fetch starts: the fetch completion resumes the
    /// walk, so later scripts still run in order
    /// (<https://html.spec.whatwg.org/multipage/scripting.html#pending-parsing-blocking-script>).
    ///
    /// <https://html.spec.whatwg.org/multipage/parsing.html#parsing-main-intext>
    /// <https://html.spec.whatwg.org/multipage/scripting.html#prepare-the-script-element>
    fn run_scripts(&mut self) -> bool {
        enum Step {
            Base(crate::dom::tree::NodeId),
            Meta(crate::dom::tree::NodeId),
            Script(crate::dom::tree::NodeId),
        }
        let steps: Vec<Step> = {
            let world = self.world.borrow();
            let Some(parsed) = world.main_document() else {
                return false;
            };
            let base = &parsed.document.base;
            let document = parsed.id;
            let mut steps = Vec::new();
            let mut stack = vec![base.root_node().id];
            while let Some(id) = stack.pop() {
                let Some(node) = base.get_node(id) else {
                    continue;
                };
                let node_id = crate::dom::tree::NodeId { document, node: id };
                // A `<meta name=referrer>` applies its policy in the same
                // insertion steps as a CSP meta
                // (<https://html.spec.whatwg.org/multipage/semantics.html#meta-referrer>).
                let referrer_meta = crate::dom::tree::is_html_element(base, id, "meta")
                    && crate::dom::tree::attr(base, id, "name")
                        .is_some_and(|name| name.eq_ignore_ascii_case("referrer"));
                if referrer_meta || crate::csp::meta_policy(base, id).is_some() {
                    steps.push(Step::Meta(node_id));
                }
                // A `<base href>` changes the base URL for the elements parsed
                // after it, so the walk carries the first one it meets. A
                // script's request URL is fixed when the parser reaches its
                // end tag, which tree order reproduces: a `<base>` later in
                // the document must not rewrite an earlier script's URL
                // (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#document-base-url>).
                if crate::dom::tree::is_html_element(base, id, "base")
                    && crate::dom::tree::attr(base, id, "href").is_some()
                {
                    steps.push(Step::Base(node_id));
                }
                if crate::dom::tree::is_html_element(base, id, "script") {
                    steps.push(Step::Script(node_id));
                }
                stack.extend(node.children.iter().rev().copied());
            }
            steps
        };
        let has_script = steps.iter().any(|step| matches!(step, Step::Script(_)));
        if has_script && !self.ensure_js_ok() {
            return false;
        }
        let mut parse_base = self.fallback_base_url();
        let mut base_seen = false;
        let _turn = self.js.as_ref().map(crate::js::JsRealm::enter_turn);
        for step in steps {
            let id = match step {
                Step::Base(id) => {
                    if !base_seen {
                        base_seen = true;
                        if let Some(href) = self.base_element_href(id) {
                            parse_base = parse_base.join(&href).unwrap_or(parse_base);
                        }
                    }
                    continue;
                }
                Step::Meta(id) => {
                    self.apply_meta_referrer(id);
                    self.apply_meta_csp(id);
                    continue;
                }
                Step::Script(id) => id,
            };
            if !self.executed_scripts.insert(id) {
                continue;
            }
            let launched = self.dynamic_script_launched(id);
            // Mirror the run into the tree's already-started flag: the
            // binding layer's prepare step checks that flag, so without
            // this every move, clone, or adopt of a parser-run script
            // would execute it again
            // (<https://html.spec.whatwg.org/multipage/scripting.html#prepare-the-script-element>).
            if let Some(mut parsed) = self.world.borrow().document_mut(id) {
                parsed.document.mark_script_started(id.node);
            }
            // Iframes that precede this script in tree order already have
            // browsing contexts; later ones wait, matching incremental
            // insertion
            // (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#the-iframe-element:html-element-insertion-steps>).
            self.adopt_pending_frames_before_node(Some(id));
            // Microtask checkpoint before the script runs; parser
            // mutations queued since the last script deliver now.
            self.deliver_mutations();
            let script = crate::js::script_at(&self.world.borrow(), id);
            match script {
                Some(crate::js::Script::Classic(crate::js::ScriptSource::Inline {
                    source,
                    line,
                })) => {
                    let violations = self.inline_script_violations(&source, id);
                    if !violations.is_empty() {
                        self.fire_csp_violations(id, &violations);
                    }
                    if crate::csp::blocks(&violations) {
                        continue;
                    }
                    let filename = self.url.as_str().to_owned();
                    self.eval_classic(&source, Some(id), line, &filename);
                }
                Some(crate::js::Script::Classic(crate::js::ScriptSource::Src(src))) => {
                    if self.start_src_script(id, &src, launched, &parse_base) {
                        return true;
                    }
                }
                Some(crate::js::Script::Module(source)) => {
                    // Module scripts are deferred by default: parsing
                    // continues, then the queue runs in document order
                    // before `DOMContentLoaded`
                    // (<https://html.spec.whatwg.org/multipage/scripting.html#attr-script-defer>).
                    self.deferred_modules.push((id, source));
                }
                None => {}
            }
        }
        // The walk finished without starting a blocking fetch: every script
        // has run, so the iframes after the last one (or all of them, when
        // there are no scripts) get browsing contexts now. Deferred modules
        // and `DOMContentLoaded` handlers must already see them; leaving
        // adoption to `fire_document_end` puts it after `DOMContentLoaded`
        // (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#the-iframe-element:html-element-insertion-steps>).
        self.adopt_pending_frames();
        false
    }

    /// Whether a script-inserted fetch `adopt_js_work` already launched
    /// for `id`. A later tree walk must not start it again. Callers read
    /// this before marking the step started; after the mark every script
    /// looks launched
    /// (<https://html.spec.whatwg.org/multipage/scripting.html#prepare-the-script-element>).
    fn dynamic_script_launched(&self, id: crate::dom::tree::NodeId) -> bool {
        self.world
            .borrow()
            .document(id)
            .is_some_and(|parsed| parsed.document.script_already_started(id.node))
    }

    /// Starts the fetch for a tree-walked `src` script, unless a
    /// script-inserted fetch already launched for `id`. Returns whether the
    /// walk blocks on the fetch
    /// (<https://html.spec.whatwg.org/multipage/scripting.html#prepare-the-script-element>).
    fn start_src_script(
        &mut self,
        id: crate::dom::tree::NodeId,
        src: &str,
        launched: bool,
        base: &Url,
    ) -> bool {
        if launched {
            return false;
        }
        let Ok(url) = Self::resolve_script_url(base, src) else {
            return false;
        };
        let violations = self.script_url_violations(&url, id);
        if !violations.is_empty() {
            self.fire_csp_violations(id, &violations);
        }
        if crate::csp::blocks(&violations) {
            return false;
        }
        self.parser_blocked_script = Some(id);
        let (referrer, referrer_policy) = self.subresource_referrer(Some(id));
        let mut dial = QueuedDial::new(
            DialContext::ClassicScript { element: id },
            &url,
            &self.dial_initiator(),
            &self.referrer_url(),
        );
        dial.req.referrer = referrer;
        dial.req.referrer_policy = referrer_policy;
        dial.req.csp = Some(Box::new(
            self.redirect_gate(crate::csp::Destination::Script),
        ));
        dial.integrity = self.script_integrity(id);
        self.queue_dial(dial);
        true
    }

    /// Raw `integrity` attribute of a classic script element, for SRI
    /// verification and the CSP integrity-metadata allowance
    /// (<https://www.w3.org/TR/SRI/#parse-metadata>).
    fn script_integrity(&self, id: crate::dom::tree::NodeId) -> Option<String> {
        let world = self.world.borrow();
        world.document(id).and_then(|parsed| {
            crate::dom::tree::attr(&parsed.document.base, id.node, "integrity").map(str::to_owned)
        })
    }

    /// The request referrer and policy a page subresource sends: the
    /// element's `referrerpolicy` content attribute when present and valid,
    /// the document's policy otherwise, and the client environment as the
    /// source. Sampled when the element starts its request, which is where
    /// HTML creates the request, so a later `<meta name=referrer>` does not
    /// rewrite an earlier element's policy
    /// (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#dom-referrerpolicy>,
    /// <https://w3c.github.io/webappsec-referrer-policy/#determine-requests-referrer>).
    fn subresource_referrer(
        &self,
        element: Option<crate::dom::tree::NodeId>,
    ) -> (net::Referrer, net::ReferrerPolicy) {
        let world = self.world.borrow();
        let policy = element
            .and_then(|element| {
                world.document(element).and_then(|parsed| {
                    crate::dom::tree::attr(&parsed.document.base, element.node, "referrerpolicy")
                        .and_then(net::ReferrerPolicy::from_attribute)
                })
            })
            .or(world.policy.referrer_policy)
            .unwrap_or_default();
        (net::Referrer::Client, policy)
    }

    pub(in crate::document) fn resolve_dial_url(&self, spec: &str) -> Result<Url, TabError> {
        let url = self
            .resolve_frame_url(spec)
            .ok_or_else(|| TabError::InvalidUrl { spec: spec.into() })?;
        // `data:` is fetched by decoding the URL, not by a carrier dial
        // (<https://fetch.spec.whatwg.org/#scheme-fetch>).
        if url.scheme() != "http" && url.scheme() != "https" && url.scheme() != "data" {
            return Err(TabError::InvalidUrl { spec: spec.into() });
        }
        Ok(url)
    }

    /// The origin a dial from this document claims as its initiator. The
    /// inherited origin, never the URL: `<base href>` changes the base URL
    /// but not the document origin, and the browser derives CORS and the
    /// site lock from this value
    /// (<https://html.spec.whatwg.org/multipage/browsers.html#concept-origin>,
    /// <https://fetch.spec.whatwg.org/#concept-request-origin>).
    pub(crate) fn dial_initiator(&self) -> url::Origin {
        self.world.borrow().origin.clone()
    }

    /// The document URL the `"client"` referrer branch reads. For an
    /// `iframe srcdoc` document this walks to the container document's URL,
    /// which the world already captured
    /// (<https://w3c.github.io/webappsec-referrer-policy/#determine-requests-referrer>).
    pub(crate) fn referrer_url(&self) -> Url {
        self.world.borrow().referrer_url.clone()
    }

    /// This frame's base URL: the first `base` element's `href` resolved
    /// against the fallback base URL, or the fallback itself
    /// (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#document-base-url>).
    pub(crate) fn base_url(&self) -> Url {
        let world = self.world.borrow();
        // An `about:srcdoc` document's own URL cannot be a base; its
        // fallback base URL is the creator document's base URL
        // (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#fallback-base-url>).
        let document_url = world.base_url.clone().unwrap_or_else(|| self.url.clone());
        let base_href = world
            .with_main_document(|parsed| {
                let base = &parsed.document.base;
                let root = base.root_node().id;
                let base_el = base.query_selector_in(root, "base[href]").ok().flatten()?;
                crate::dom::tree::attr(base, base_el, "href").map(str::to_owned)
            })
            .flatten();
        let Some(href) = base_href else {
            return document_url;
        };
        document_url.join(&href).unwrap_or(document_url)
    }

    /// The URL a document's `<base href>` elements resolve against: the
    /// document URL, or the creator's base URL for an `about:srcdoc`
    /// document
    /// (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#fallback-base-url>).
    fn fallback_base_url(&self) -> Url {
        let world = self.world.borrow();
        world.base_url.clone().unwrap_or_else(|| self.url.clone())
    }

    /// A `<base>` element's `href` value.
    fn base_element_href(&self, id: crate::dom::tree::NodeId) -> Option<String> {
        let world = self.world.borrow();
        let parsed = world.document(id)?;
        crate::dom::tree::attr(&parsed.document.base, id.node, "href").map(str::to_owned)
    }

    /// Resolves a parser-inserted script's `src` against the base URL in
    /// effect where the parser reached it. Only schemes a dial can fetch are
    /// script resources
    /// (<https://html.spec.whatwg.org/multipage/scripting.html#prepare-the-script-element>).
    fn resolve_script_url(base: &Url, spec: &str) -> Result<Url, TabError> {
        let url = Url::parse(spec)
            .or_else(|_| base.join(spec))
            .map_err(|_| TabError::InvalidUrl { spec: spec.into() })?;
        if !matches!(url.scheme(), "http" | "https" | "data") {
            return Err(TabError::InvalidUrl { spec: spec.into() });
        }
        Ok(url)
    }

    pub(in crate::document) fn finish_dial(&mut self, done: CompletedDial) {
        let CompletedDial {
            context,
            outcome,
            req,
            ancestors,
            integrity,
        } = done;
        let redirected = outcome.final_url != req.url;
        match context {
            DialContext::JsFetch { id } => {
                self.record_event(RendererEvent::Fetch {
                    status: outcome.status,
                });
                // Post-request check: a redirect may have landed on a URL
                // the source list does not allow
                // (<https://w3c.github.io/webappsec-csp/#should-block-response>).
                let blocked = redirected
                    && self.response_url_blocked(
                        &outcome.final_url,
                        &req.url,
                        crate::csp::Destination::Connect,
                        None,
                    );
                if blocked {
                    self.settle_js_fetch(id, None);
                } else {
                    self.settle_js_fetch(id, Some(outcome));
                }
            }
            DialContext::ClassicScript { element } => {
                self.record_event(RendererEvent::Fetch {
                    status: outcome.status,
                });
                // Only the script the parser waits for resumes the walk.
                // Any other completion still runs its load steps, but the
                // walk stays paused until the blocking fetch settles.
                let unblocks_parser = self
                    .parser_blocked_script
                    .is_none_or(|waiting| waiting == element);
                if unblocks_parser {
                    self.parser_blocked_script = None;
                }
                // A redirect may have landed outside `script-src`. The
                // network refuses that hop; this is the report, and the
                // check for a host that already followed.
                // <https://w3c.github.io/webappsec-csp/#should-block-response>
                let blocked = redirected
                    && self.response_url_blocked(
                        &outcome.final_url,
                        &req.url,
                        crate::csp::Destination::Script,
                        Some(element),
                    );
                // SRI verification fails the load like a network error:
                // no execution, no `load` event, and no CSP violation
                // (<https://www.w3.org/TR/SRI/#verify-integrity>).
                let body = outcome.body.as_ref().ok();
                let integrity_ok = integrity.as_deref().is_none_or(|metadata| {
                    body.is_some_and(|body| crate::csp::integrity_verifies(metadata, body))
                });
                // A classic script with a non-JavaScript MIME type does not
                // execute (<https://html.spec.whatwg.org/multipage/scripting.html#prepare-the-script-element>).
                let mime_ok = crate::js::classic_script_mime(outcome.content_type.as_deref());
                if !blocked
                    && integrity_ok
                    && mime_ok
                    && let Some(body) = body
                    && (200..300).contains(&outcome.status)
                {
                    let source = String::from_utf8_lossy(body);
                    let filename = outcome.final_url.clone();
                    self.eval_classic(&source, Some(element), 1, &filename);
                    self.fire_js(|js| js.fire_node_load(element));
                } else {
                    self.fire_js(|js| js.fire_node_error(element));
                }
                if unblocks_parser {
                    self.resume_scripts();
                }
            }
            DialContext::FrameLoad { embedder } => {
                // A redirect the embedder's `frame-src` refuses is not loaded.
                // Report-only still reports and then loads.
                // <https://w3c.github.io/webappsec-csp/#should-block-response>
                if redirected
                    && let Some(embedder) = embedder
                    && let Some(gate) = req.csp.as_deref()
                    && self.note_frame_redirect(embedder, gate, &outcome.final_url, &req.url)
                {
                    // Leave the current document in place.
                } else if let Ok(url) = Url::parse(&outcome.final_url) {
                    self.commit_frame_response(&outcome, &url, embedder, &ancestors);
                }
            }
            DialContext::BlitzResource { id, destination } => {
                let cors_clean = req
                    .cors
                    .as_ref()
                    .is_some_and(|cors| cors.mode == net::cors::RequestMode::Cors);
                self.deliver_blitz_resource(
                    id,
                    destination,
                    &outcome,
                    &req.url,
                    redirected,
                    cors_clean,
                );
            }
            // Fire-and-forget: the endpoint already stored the report, if it
            // cares. A failure is invisible by design.
            DialContext::CspReport => {}
        }
        self.adopt_js_work();
    }

    /// Commits a child frame's HTTP response, unless its `frame-ancestors`
    /// refuses the ancestors. A refused response falls back to an opaque
    /// `about:blank` the embedder can no longer read; the embedder dispatches
    /// the violations
    /// (<https://w3c.github.io/webappsec-csp/#frame-ancestors-navigation-response>).
    fn commit_frame_response(
        &mut self,
        outcome: &DialOutcome,
        url: &Url,
        embedder: Option<FrameId>,
        ancestors: &[Url],
    ) {
        let head = ResponseHead::http(
            outcome.status,
            url.to_string(),
            outcome.content_type.clone(),
            &outcome.headers,
        );
        let policy = crate::csp::List::from_headers(
            head.content_security_policy.as_deref(),
            head.content_security_policy_report_only.as_deref(),
        );
        // Set before the `frame-ancestors` decision: a refused response
        // dispatches its violations on the fallback document against the new
        // response's endpoints.
        self.set_reporting_endpoints(&head);
        if embedder.is_some()
            && !ancestors.is_empty()
            && self.note_frame_ancestors_block(embedder, url, &policy, ancestors)
        {
            // The blank completes and fires the container's load.
            return;
        }
        // Announce the commit before the new document's load, so the
        // browser can re-create the top-level execution contexts
        // (<https://chromedevtools.github.io/devtools-protocol/tot/Page/#event-frameNavigated>).
        self.record_event(RendererEvent::Navigated {
            url: url.to_string(),
        });
        let Ok(body) = &outcome.body else {
            return;
        };
        // Set before the parse runs: the new document's own scripts read
        // `document.referrer` while their inline scripts execute. The value is
        // the referrer the browser actually sent on the final hop, so redirect
        // policy and downgrade rules cannot disagree with the header
        // (<https://html.spec.whatwg.org/multipage/document-lifecycle.html#creating-and-initializing-a-document-object>).
        self.set_document_referrer(outcome.referrer.as_deref().unwrap_or_default());
        self.install_frame_response(&head, body, None);
    }

    /// Hands one Blitz subresource response to its handler. A redirect the
    /// policy refuses is a failed load: empty bytes, and an `error` on the
    /// element when the tree has one. `cors_clean` records that the request
    /// went out in CORS mode and survived the check, which makes the loaded
    /// image origin-clean for canvas.
    fn deliver_blitz_resource(
        &mut self,
        id: u64,
        destination: Option<crate::csp::Destination>,
        outcome: &DialOutcome,
        request_url: &str,
        redirected: bool,
        cors_clean: bool,
    ) {
        let blocked = redirected
            && destination.is_some_and(|destination| {
                self.response_url_blocked(&outcome.final_url, request_url, destination, None)
            });
        let body = outcome.body.as_ref().ok();
        if !blocked
            && (200..300).contains(&outcome.status)
            && let Some(body) = body
        {
            self.subresource_bytes
                .insert(outcome.final_url.clone(), body.clone());
        }
        // A style sheet's response is the policy source for every request the
        // sheet initiates: its own `Referrer-Policy` when it sent one, else the
        // document policy the sheet inherited at fetch time. Record it under
        // both the requested and the final URL, because Blitz files the sheet
        // under the URL it asked for
        // (<https://w3c.github.io/webappsec-referrer-policy/#integration-with-css>).
        if destination == Some(crate::csp::Destination::StyleSheet)
            && !blocked
            && (200..300).contains(&outcome.status)
        {
            let inherited = self
                .world
                .borrow()
                .policy
                .referrer_policy
                .unwrap_or_default();
            let policy = net::ReferrerPolicy::from_header(
                outcome
                    .headers
                    .iter()
                    .filter(|(name, _)| name.eq_ignore_ascii_case("referrer-policy"))
                    .map(|(_, value)| value.as_str()),
            )
            .unwrap_or(inherited);
            self.style_policies
                .insert(outcome.final_url.clone(), policy);
            if request_url != outcome.final_url {
                self.style_policies.insert(request_url.to_owned(), policy);
            }
        }
        let Some(handler) = self.blitz_handlers.remove(&id) else {
            return;
        };
        let request_url = self.blitz_request_urls.remove(&id).unwrap_or_default();
        if blocked || body.is_none() {
            Box::new(handler).bytes(String::new(), blitz_traits::net::Bytes::new());
            let element = Url::parse(&request_url)
                .ok()
                .and_then(|url| self.subresource_target(&url));
            if let Some(element) = element {
                if destination == Some(crate::csp::Destination::Img) {
                    self.world
                        .borrow_mut()
                        .fail_image(element, request_url.clone());
                } else {
                    self.tasks.push_back(Task::NodeError(element));
                }
            }
        } else {
            Box::new(handler).bytes(
                outcome.final_url.clone(),
                blitz_traits::net::Bytes::from(body.cloned().unwrap_or_default()),
            );
        }
        // The arrival changes style or layout; apply it now so the tree is
        // current for the next capture instead of waiting for one to pump
        // the resolve loop.
        self.settle_blitz_layout();
        self.settle_image_waiters(&request_url, cors_clean);
    }

    pub(in crate::document) fn fail_dial(&mut self, fail: DialContext) {
        self.record_event(RendererEvent::FetchFailed);
        match fail {
            DialContext::JsFetch { id } => {
                self.settle_js_fetch(id, None);
            }
            DialContext::ClassicScript { element } => {
                let unblocks_parser = self
                    .parser_blocked_script
                    .is_none_or(|waiting| waiting == element);
                if unblocks_parser {
                    self.parser_blocked_script = None;
                }
                self.fire_js(|js| js.fire_node_error(element));
                if unblocks_parser {
                    self.resume_scripts();
                }
            }
            // Keep the frame's current (about:blank) document; the
            // container's load event still fires because the frame is no
            // longer waiting. A lost report is invisible by design.
            DialContext::FrameLoad { .. } | DialContext::CspReport => {}
            DialContext::BlitzResource { id, destination: _ } => {
                // Deliver empty bytes so Blitz releases the resource as
                // failed instead of holding it pending forever.
                if let Some(handler) = self.blitz_handlers.remove(&id) {
                    let request_url = self.blitz_request_urls.remove(&id).unwrap_or_default();
                    Box::new(handler).bytes(String::new(), blitz_traits::net::Bytes::new());
                    let target = Url::parse(&request_url)
                        .ok()
                        .and_then(|url| self.subresource_target(&url));
                    let is_image = target.is_some_and(|element| {
                        self.world.borrow().document(element).is_some_and(|parsed| {
                            crate::dom::tree::is_html_element(
                                &parsed.document.base,
                                element.node,
                                "img",
                            )
                        })
                    });
                    if let Some(element) = target {
                        // A failed request must be visible to later scans; a
                        // waiterless element would otherwise re-register a
                        // waiter that never settles. Images report their
                        // `error` through the lifecycle; other elements here.
                        if is_image {
                            self.world
                                .borrow_mut()
                                .fail_image(element, request_url.clone());
                        } else {
                            self.tasks.push_back(Task::NodeError(element));
                        }
                    }
                    self.settle_blitz_layout();
                    self.settle_image_waiters(&request_url, false);
                }
            }
        }
        self.adopt_js_work();
    }

    /// Applies Blitz's queued resource messages and re-lays out the active
    /// document, so a stylesheet or image arrival is reflected before the
    /// next capture instead of waiting for one to pump the resolve loop.
    /// The arrival may also be the last one holding the load event.
    fn settle_blitz_layout(&mut self) {
        let world = self.world.borrow();
        let Some(mut parsed) = world.main_document_mut() else {
            return;
        };
        crate::render::resolve_until_settled(&mut parsed.document.base);
        drop(parsed);
        drop(world);
        self.note_linked_stylesheets();
    }

    /// Fires `load` on connected `<link rel=stylesheet>` elements whose sheet
    /// has arrived. Blitz attaches a sheet to the link node only after the
    /// response parses, so the sheet's presence is the fetch-complete signal;
    /// the scan runs when the document's stylesheet generation moved, so image
    /// and font deliveries pay nothing
    /// (<https://html.spec.whatwg.org/multipage/semantics.html#the-link-element>,
    /// <https://html.spec.whatwg.org/multipage/links.html#link-type-stylesheet>).
    fn note_linked_stylesheets(&mut self) {
        let (generation, arrivals, stale) = {
            let world = self.world.borrow();
            let Some(parsed) = world.main_document() else {
                return;
            };
            let base = &parsed.document.base;
            let generation = base.stylesheet_generation();
            if generation == self.stylesheet_generation {
                return;
            }
            let document = parsed.id;
            let linked = |id: blitz_dom::NodeId| {
                crate::dom::tree::is_stylesheet_link(base, id)
                    && base.node_has_stylesheet(id)
                    && crate::dom::tree::is_connected(base, id)
            };
            let arrivals: Vec<(crate::dom::tree::NodeId, String)> = base
                .stylesheet_owner_nodes()
                .filter(|id| linked(*id))
                .filter_map(|id| {
                    crate::dom::tree::attr(base, id, "href")
                        .and_then(|href| base.resolve_url(href))
                        .map(|url| {
                            (
                                crate::dom::tree::NodeId { document, node: id },
                                url.as_str().to_owned(),
                            )
                        })
                })
                .collect();
            let stale: Vec<crate::dom::tree::NodeId> = self
                .linked_sheets
                .keys()
                .copied()
                .filter(|node| node.document != document || !linked(node.node))
                .collect();
            (generation, arrivals, stale)
        };
        self.stylesheet_generation = generation;
        for node in stale {
            self.linked_sheets.remove(&node);
        }
        for (element, url) in arrivals {
            if self.linked_sheets.get(&element) == Some(&url) {
                continue;
            }
            self.linked_sheets.insert(element, url);
            self.tasks.push_back(Task::NodeLoad(element));
        }
    }

    pub(in crate::document) fn settle_js_fetch(&mut self, id: i32, outcome: Option<DialOutcome>) {
        self.fire_js(|js| js.finish_js_fetch(id, outcome));
    }

    /// Runs the post-parsing steps of "the end": set readiness to
    /// `interactive`, fire `DOMContentLoaded`, then fire `load`
    /// (<https://html.spec.whatwg.org/multipage/parsing.html#the-end>).
    fn fire_document_end(&mut self) {
        if self.world.borrow().main_ready_state() != ReadyState::Loading {
            return;
        }
        self.world
            .borrow_mut()
            .set_main_ready_state(ReadyState::Interactive);
        self.fire_js(crate::js::JsRealm::fire_ready_state_change);
        self.run_deferred_modules();
        self.fire_js(crate::js::JsRealm::fire_dom_content_loaded);
        self.adopt_js_work();
        self.record_event(RendererEvent::DomContentLoaded);
        // A script may have appended an iframe after the parser finished; its
        // load must delay this document's own load event
        // (<https://html.spec.whatwg.org/multipage/parsing.html#delay-the-load-event>).
        self.adopt_pending_frames();
    }

    fn run_deferred_modules(&mut self) {
        let modules = std::mem::take(&mut self.deferred_modules);
        for (index, (element, source)) in modules.into_iter().enumerate() {
            let result = match source {
                crate::js::ScriptSource::Inline { source, .. } => {
                    let violations = self.inline_script_violations(&source, element);
                    if !violations.is_empty() {
                        self.fire_csp_violations(element, &violations);
                    }
                    if crate::csp::blocks(&violations) {
                        continue;
                    }
                    let name = format!("{}#inline-module-{index}", self.url);
                    self.js
                        .as_ref()
                        .map_or(Ok(()), |js| js.eval_inline_module(&name, &source))
                }
                crate::js::ScriptSource::Src(src) => match self.resolve_dial_url(&src) {
                    Ok(url) => {
                        let violations = self.script_url_violations(&url, element);
                        if !violations.is_empty() {
                            self.fire_csp_violations(element, &violations);
                        }
                        if crate::csp::blocks(&violations) {
                            continue;
                        }
                        self.js
                            .as_ref()
                            .map_or(Ok(()), |js| js.eval_external_module(url.as_str()))
                    }
                    Err(_) => Err(crate::js::JsError::Engine(
                        "invalid module script URL".into(),
                    )),
                },
            };
            if result.is_err() {
                self.record_event(RendererEvent::ScriptFailed);
            } else {
                self.fire_js(|js| js.fire_node_load(element));
            }
            self.adopt_js_work();
        }
    }

    /// Runs the shared declarative refresh steps for one value, from a
    /// `Refresh` response head or a `meta http-equiv=refresh`. The first value
    /// wins; a later one is ignored
    /// (<https://html.spec.whatwg.org/multipage/semantics.html#attr-meta-http-equiv-refresh>).
    fn start_declarative_refresh(&mut self, value: &str) {
        if self.will_declaratively_refresh {
            return;
        }
        let document_url = self.world.borrow().document_url.clone();
        let Some((target, delay)) = parse_declarative_refresh(value, &document_url) else {
            return;
        };
        self.will_declaratively_refresh = true;
        self.pending_refresh = Some((target, delay));
        self.arm_refresh();
    }

    /// Starts a pending refresh's clock. The delay is measured from the
    /// document's completely-loaded point, or from the `meta` insertion when
    /// the document already loaded
    /// (<https://html.spec.whatwg.org/multipage/semantics.html#attr-meta-http-equiv-refresh>).
    fn arm_refresh(&mut self) {
        if !self.load_fired || self.refresh_timer.is_some() {
            return;
        }
        let Some((target, delay)) = self.pending_refresh.take() else {
            return;
        };
        self.refresh_target = Some(target);
        let id = self.schedule_timer(delay);
        self.refresh_timer = Some(id);
    }

    /// Navigates this document's browsing context to its armed refresh target,
    /// using this document as the navigation's source.
    fn fire_refresh(&mut self) {
        let Some(target) = self.refresh_target.take() else {
            return;
        };
        let navigation = FrameNavigation::get(
            crate::js::NavigationTarget::Target("_self".to_owned()),
            target.to_string(),
        );
        self.world.borrow_mut().queue_frame_navigation(navigation);
    }

    /// The single `load` check. Readiness must have reached `interactive`, the
    /// delay count must be zero, and any Blitz request that a completion just
    /// produced is made a dial first, so its guard exists before the count is
    /// consulted
    /// (<https://html.spec.whatwg.org/multipage/parsing.html#the-end>).
    pub(crate) fn check_load(&mut self) -> bool {
        if self.load_fired {
            return false;
        }
        if self.world.borrow().main_ready_state() == ReadyState::Loading {
            return false;
        }
        self.drain_blitz_fetches();
        if self.load_delay.is_pending() {
            return false;
        }
        self.complete_ready_state();
        self.load_fired = true;
        self.arm_refresh();
        self.record_event(RendererEvent::Load);
        self.fire_js(crate::js::JsRealm::fire_load);
        self.adopt_js_work();
        true
    }

    /// Moves readiness to `complete` and dispatches its change event once. A
    /// child frame can be the last thing delaying `load` after the parser
    /// already reached `interactive`.
    fn complete_ready_state(&mut self) {
        if self.world.borrow().main_ready_state() == ReadyState::Complete {
            return;
        }
        self.world
            .borrow_mut()
            .set_main_ready_state(ReadyState::Complete);
        self.fire_js(crate::js::JsRealm::fire_ready_state_change);
    }

    /// Registers the current document's `<img src>` requests as Blitz image
    /// waiters. Image requests delay the load event until they succeed or fail
    /// (<https://html.spec.whatwg.org/multipage/images.html#updating-the-image-data>).
    fn load_images(&mut self) {
        let images: Vec<crate::dom::tree::NodeId> = self
            .world
            .borrow()
            .with_main_document(|parsed| {
                let base = &parsed.document.base;
                let document = parsed.id;
                let mut images = Vec::new();
                let mut stack = vec![base.root_node().id];
                while let Some(id) = stack.pop() {
                    if crate::dom::tree::is_html_element(base, id, "img")
                        && crate::dom::tree::attr(base, id, "src").is_some()
                    {
                        images.push(crate::dom::tree::NodeId { document, node: id });
                    }
                    if let Some(node) = base.get_node(id) {
                        stack.extend(node.children.iter().rev().copied());
                    }
                }
                images
            })
            .unwrap_or_default();
        for element in images {
            self.queue_image(element, false);
        }
    }

    /// Moves every queued Blitz subresource fetch onto a carrier dial,
    /// filing its response handler under the fetch id.
    fn drain_blitz_fetches(&mut self) {
        while let Ok((fetch, handler)) = self.blitz_fetch_rx.try_recv() {
            let url = fetch.url.as_str().to_owned();
            if let Some(bytes) = self.subresource_bytes.get(&url).cloned() {
                Box::new(handler).bytes(url.clone(), blitz_traits::net::Bytes::from(bytes));
                self.settle_blitz_layout();
                self.settle_image_waiters(&url, false);
                continue;
            }
            // Per-destination policy: a blocked stylesheet, image, or font is
            // never dialed; the handler receives empty bytes so Blitz
            // releases the resource as failed
            // (<https://w3c.github.io/webappsec-csp/#should-block-request>).
            let destination = subresource_destination(fetch.kind);
            let element = self.subresource_target(&fetch.url);
            if let Some(destination) = destination {
                let violations = self.subresource_violations(&fetch.url, destination);
                let target = element.or_else(|| self.document_node());
                if !violations.is_empty()
                    && let Some(target) = target
                {
                    self.fire_csp_violations(target, &violations);
                }
                if crate::csp::blocks(&violations) {
                    Box::new(handler).bytes(String::new(), blitz_traits::net::Bytes::new());
                    if let Some(element) = element {
                        if matches!(destination, crate::csp::Destination::Img) {
                            // The image lifecycle reports the error when the
                            // DOM walk reaches the element, so scripts that
                            // precede it have run.
                            self.world.borrow_mut().fail_image(element, url.clone());
                        } else {
                            // A blocked element load fails with an `error`
                            // event
                            // (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#fetching-resources>).
                            self.tasks.push_back(Task::NodeError(element));
                        }
                    }
                    self.settle_blitz_layout();
                    self.settle_image_waiters(&url, false);
                    continue;
                }
            }
            self.blitz_request_urls.insert(fetch.id, url);
            self.blitz_handlers.insert(fetch.id, handler);
            // The Fetch initiator is the document, not the fetch target: the
            // provider never sees the document URL, so it stamps the target
            // as initiator and the document corrects it here
            // (<https://fetch.spec.whatwg.org/#concept-request-initiator>).
            let (referrer, referrer_policy) = match fetch.style_sheet.clone() {
                // A request a style sheet initiates uses the sheet's URL as
                // its referrer and the policy that sheet's response carried,
                // not the document's
                // (<https://w3c.github.io/webappsec-referrer-policy/#integration-with-css>).
                Some(sheet) => {
                    let policy = self
                        .style_policies
                        .get(sheet.as_str())
                        .copied()
                        .or(self.world.borrow().policy.referrer_policy)
                        .unwrap_or_default();
                    (net::Referrer::Url(sheet), policy)
                }
                None => self.subresource_referrer(element),
            };
            let mut dial = QueuedDial::new(
                DialContext::BlitzResource {
                    id: fetch.id,
                    destination,
                },
                &fetch.url,
                &self.dial_initiator(),
                &self.referrer_url(),
            );
            dial.req.referrer = referrer;
            dial.req.referrer_policy = referrer_policy;
            // An image with a `crossorigin` content attribute loads in CORS
            // mode: the network sends `Origin` and refuses the load without a
            // valid `Access-Control-Allow-Origin`; a load that survives marks
            // the image origin-clean for canvas
            // (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#cors-settings-attributes>,
            // <https://html.spec.whatwg.org/multipage/canvas.html#the-image-argument>).
            if destination == Some(crate::csp::Destination::Img) {
                dial.req.cors = element.and_then(|element| {
                    let world = self.world.borrow();
                    let parsed = world.main_document()?;
                    let base = &parsed.document.base;
                    let cross = crate::dom::tree::attr(base, element.node, "crossorigin");
                    let credentials = match cross.map(str::trim) {
                        Some(value) if value.eq_ignore_ascii_case("use-credentials") => {
                            net::cors::CredentialsMode::Include
                        }
                        Some(_) => net::cors::CredentialsMode::SameOrigin,
                        None => return None,
                    };
                    Some(crate::protocol::CorsRequest {
                        mode: net::cors::RequestMode::Cors,
                        credentials,
                        redirect: net::cors::RedirectMode::Follow,
                    })
                });
            }
            dial.req.csp = destination.map(|destination| Box::new(self.redirect_gate(destination)));
            self.queue_dial(dial);
        }
    }

    pub(crate) fn redirect_gate(
        &self,
        destination: crate::csp::Destination,
    ) -> crate::RedirectGate {
        let world = self.world.borrow();
        crate::RedirectGate::new(world.policy.csp.clone(), &world.origin, destination)
    }

    /// Records `frame-src` violations for a redirect and reports whether the
    /// enforced policy refuses the response. The embedding document dispatches
    /// the events; this frame only queues them.
    fn note_frame_redirect(
        &mut self,
        embedder: FrameId,
        gate: &crate::RedirectGate,
        final_url: &str,
        request_url: &str,
    ) -> bool {
        let Ok(response_url) = Url::parse(final_url) else {
            return false;
        };
        let Ok(blocked_url) = Url::parse(request_url) else {
            return false;
        };
        let Ok(document_url) = Url::parse(self.document_url()) else {
            return false;
        };
        let violations = gate.redirected_violations(&document_url, &response_url, &blocked_url);
        let blocked = crate::csp::blocks(&violations);
        if !violations.is_empty() {
            self.embedder_csp_reports.push((embedder, violations));
        }
        blocked
    }

    /// Records `frame-ancestors` violations for a child response and reports
    /// whether the enforced policy refuses the embed. A refused response is
    /// replaced with an opaque `about:blank` the embedder can no longer read.
    /// No DOM event fires: the response-policy violation is created with a
    /// null global, so only the network reports are sent
    /// (<https://w3c.github.io/webappsec-csp/#should-navigation-response-to-navigation-request-of-type-in-target-be-blocked-by-content-security-policy>).
    fn note_frame_ancestors_block(
        &mut self,
        embedder: Option<FrameId>,
        child_url: &Url,
        policy: &crate::csp::List,
        ancestors: &[Url],
    ) -> bool {
        if embedder.is_none() {
            return false;
        }
        let violations = policy.check_frame_ancestors(child_url, ancestors);
        let blocked = crate::csp::blocks(&violations);
        if !violations.is_empty() {
            self.queue_csp_reports(&violations);
        }
        if blocked {
            let opaque = Url::parse("about:blank")
                .map_or_else(|_| child_url.origin(), |blank| blank.origin());
            self.load_about_blank(Some("about:blank"), Some(opaque));
        }
        blocked
    }

    /// `frame-src` redirect violations queued for the embedding document.
    pub(crate) fn take_embedder_csp_reports(
        &mut self,
    ) -> Vec<(FrameId, Vec<crate::csp::Violation>)> {
        std::mem::take(&mut self.embedder_csp_reports)
    }

    /// Whether the response URL of a completed request violates the policy
    /// after redirects, firing the violations. Paths are ignored, as the
    /// post-request check requires
    /// (<https://w3c.github.io/webappsec-csp/#should-block-response>).
    pub(in crate::document) fn response_url_blocked(
        &mut self,
        final_url: &str,
        request_url: &str,
        destination: crate::csp::Destination,
        target: Option<crate::dom::tree::NodeId>,
    ) -> bool {
        let Ok(url) = Url::parse(final_url) else {
            return false;
        };
        let Ok(blocked_url) = Url::parse(request_url) else {
            return false;
        };
        let violations = {
            let world = self.world.borrow();
            world.policy.csp.check_redirected_request(
                &world.origin,
                &world.document_url,
                &url,
                &blocked_url,
                destination,
            )
        };
        if !violations.is_empty()
            && let Some(target) = target.or_else(|| self.document_node())
        {
            self.fire_csp_violations(target, &violations);
        }
        crate::csp::blocks(&violations)
    }

    /// The violations a Blitz subresource fetch produces.
    pub(in crate::document) fn subresource_violations(
        &self,
        url: &Url,
        destination: crate::csp::Destination,
    ) -> Vec<crate::csp::Violation> {
        let world = self.world.borrow();
        world
            .policy
            .csp
            .check_request(&world.origin, &world.document_url, url, destination, None)
    }

    /// The element whose subresource load `url` is: a `<link>` or `<img>`
    /// whose resolved URL matches, when the fetch came from markup.
    fn subresource_target(&self, url: &Url) -> Option<crate::dom::tree::NodeId> {
        let candidates: Vec<(crate::dom::tree::NodeId, String)> = {
            let world = self.world.borrow();
            let parsed = world.main_document()?;
            let base = &parsed.document.base;
            let document = parsed.id;
            let mut candidates = Vec::new();
            let mut stack = vec![base.root_node().id];
            while let Some(id) = stack.pop() {
                let Some(node) = base.get_node(id) else {
                    continue;
                };
                stack.extend(node.children.iter().copied());
                let Some(element) = node.data.downcast_element() else {
                    continue;
                };
                if element.name.ns != crate::dom::tree::html_namespace() {
                    continue;
                }
                let attribute = match element.name.local.as_str() {
                    "link" => "href",
                    "img" => "src",
                    _ => continue,
                };
                if let Some(spec) = crate::dom::tree::attr(base, id, attribute) {
                    candidates.push((
                        crate::dom::tree::NodeId { document, node: id },
                        spec.to_owned(),
                    ));
                }
            }
            candidates
        };
        for (id, spec) in candidates {
            if let Ok(resolved) = self.resolve_dial_url(&spec)
                && resolved == *url
            {
                return Some(id);
            }
        }
        None
    }

    /// Registers the current `<img>` request as a waiter on Blitz image
    /// state. Blitz fetches and decodes on its own — initial parse,
    /// insertion, and `src` sets all run through its loader — so this only
    /// watches: a request Blitz already settled (its image cache, or a
    /// delivery that beat this registration) settles here, otherwise the
    /// Blitz delivery for the selected URL settles it.
    ///
    /// `force` is a `src` mutation: a connected insert skips work when a
    /// request is already in flight or decoded.
    pub(in crate::document) fn queue_image(
        &mut self,
        element: crate::dom::tree::NodeId,
        force: bool,
    ) {
        if !force
            && (self.in_flight_images.contains_key(&element)
                || self.world.borrow().images.sizes.contains_key(&element))
        {
            return;
        }
        let src = self.world.borrow().document(element).and_then(|parsed| {
            crate::dom::tree::is_html_element(&parsed.document.base, element.node, "img")
                .then(|| {
                    crate::dom::tree::attr(&parsed.document.base, element.node, "src")
                        .map(str::to_owned)
                })
                .flatten()
        });
        let Some(src) = src.filter(|src| !src.is_empty()) else {
            self.in_flight_images.remove(&element);
            self.image_selected_src.remove(&element);
            self.world.borrow_mut().forget_image(element);
            self.tasks.push_back(Task::NodeError(element));
            return;
        };
        let Ok(url) = self.resolve_dial_url(&src) else {
            self.in_flight_images.remove(&element);
            self.image_selected_src.remove(&element);
            self.world.borrow_mut().fail_image(element, src);
            self.tasks.push_back(Task::NodeError(element));
            return;
        };
        let selected = url.as_str().to_owned();
        // A request that already failed (blocked or network error) must not
        // register a waiter: no delivery will ever settle it, which would
        // hold the load event forever. The failure reports its `error` event
        // here, in parser order, instead of when the block decided.
        if self.world.borrow().image_failed(element, &selected) {
            self.in_flight_images.remove(&element);
            self.image_selected_src.remove(&element);
            if self.world.borrow_mut().report_image_error(element) {
                self.tasks.push_back(Task::NodeError(element));
            }
            return;
        }
        self.image_selected_src.insert(element, selected.clone());
        self.world
            .borrow_mut()
            .begin_image(element, selected.clone());
        if let Some((width, height)) = self.blitz_image_dims(element) {
            self.image_selected_src.remove(&element);
            self.world
                .borrow_mut()
                .store_image_dims(element, width, height, selected);
            self.tasks.push_back(Task::NodeLoad(element));
            return;
        }
        self.in_flight_images
            .insert(element, self.load_delay.guard());
    }

    /// Decoded dimensions Blitz holds for `element`'s current request, if it
    /// decoded one. Blitz decodes synchronously on delivery, so after
    /// [`Self::settle_blitz_layout`] this is settled for every delivered URL.
    fn blitz_image_dims(&self, element: crate::dom::tree::NodeId) -> Option<(u32, u32)> {
        let world = self.world.borrow();
        let parsed = world.main_document()?;
        let node = parsed.document.base.get_node(element.node)?;
        match node.element_data()?.image_data()? {
            blitz_dom::node::ImageData::Raster(raster) => Some((raster.width, raster.height)),
            blitz_dom::node::ImageData::Svg(svg) => Some(crate::render::svg_natural_size(svg)),
            blitz_dom::node::ImageData::None => None,
        }
    }

    /// Settles every waiter selected for `url` from Blitz's decoded state:
    /// decoded dimensions fire `load`, anything else fires `error`.
    /// Settles the in-flight image elements waiting on `url`. `cors_clean`
    /// marks the elements' settled request as origin-clean for canvas: the
    /// request went out in CORS mode and survived the check. Every other
    /// settle path passes `false`, which falls back to the same-origin rule.
    fn settle_image_waiters(&mut self, url: &str, cors_clean: bool) {
        let waiting: Vec<crate::dom::tree::NodeId> = self
            .in_flight_images
            .keys()
            .copied()
            .filter(|element| {
                self.image_selected_src
                    .get(element)
                    .is_some_and(|selected| selected == url)
            })
            .collect();
        for element in &waiting {
            self.in_flight_images.remove(element);
            let selected = self.image_selected_src.remove(element).unwrap_or_default();
            if cors_clean {
                self.world.borrow_mut().images.cors_clean.insert(*element);
            }
            if let Some((width, height)) = self.blitz_image_dims(*element) {
                self.world
                    .borrow_mut()
                    .store_image_dims(*element, width, height, selected);
                self.fire_js(|js| js.fire_node_load(*element));
            } else {
                self.world.borrow_mut().fail_image(*element, selected);
                if self.world.borrow_mut().report_image_error(*element) {
                    self.fire_js(|js| js.fire_node_error(*element));
                }
            }
            self.adopt_js_work();
        }
    }

    /// A child browsing context started loading; this document's `load` event
    /// waits for it, and the container is remembered until it finishes.
    pub(crate) fn mark_frame_load_pending(&mut self, container: crate::dom::tree::NodeId) {
        self.pending_frame_loads
            .insert(container, self.load_delay.guard());
    }

    /// The containers whose child frames have not finished loading.
    pub(crate) fn pending_frame_loads(&self) -> Vec<crate::dom::tree::NodeId> {
        self.pending_frame_loads.keys().copied().collect()
    }

    /// Fires one container's `load` event and lets this document's own `load`
    /// event proceed once no child is left loading.
    pub(crate) fn finish_frame_load(&mut self, container: crate::dom::tree::NodeId) -> bool {
        if self.pending_frame_loads.remove(&container).is_none() {
            return false;
        }
        // A src-less iframe already fired `load` from its insertion steps;
        // that firing is recorded per container, not re-read from the current
        // attributes, so a later navigation reusing the container still fires
        // (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#the-iframe-element:html-element-insertion-steps>).
        let already_fired = self
            .world
            .borrow_mut()
            .insertion_load_fired
            .remove(&container);
        if !already_fired {
            self.fire_node_load(container);
        }
        true
    }

    /// Drops a container that went away before its child finished loading.
    pub(crate) fn cancel_frame_load(&mut self, container: crate::dom::tree::NodeId) {
        let _ = self
            .world
            .borrow_mut()
            .insertion_load_fired
            .remove(&container);
        self.pending_frame_loads.remove(&container);
    }

    pub(in crate::document) fn record_event(&mut self, event: RendererEvent) {
        if self.events.len() == MAX_PENDING_EVENTS {
            self.events_overflowed = true;
            return;
        }
        self.events.push(event);
    }
}

/// Whether parsed HTML needs a realm: a `script` element or any event
/// handler content attribute (`on*`, same predicate as attribute-set
/// compilation in `bindings/attributes.rs`). The walk is iterative so deep
/// documents cannot overflow the stack.
fn html_needs_realm(parsed: &crate::Parsed) -> bool {
    let base = &parsed.document.base;
    let mut stack = vec![base.root_node().id];
    while let Some(id) = stack.pop() {
        let Some(node) = base.get_node(id) else {
            continue;
        };
        if crate::dom::tree::is_html_element(base, id, "script") {
            return true;
        }
        if let Some(element) = node.data.downcast_element()
            && element.attrs.iter().any(|attribute| {
                attribute.name.prefix.as_ref().is_none()
                    && attribute.name.ns.as_str().is_empty()
                    && crate::dom::tree::is_handler_attribute_name(attribute.name.local.as_str())
            })
        {
            return true;
        }
        stack.extend(node.children.iter().rev().copied());
    }
    false
}

impl From<crate::js::JsError> for TabError {
    fn from(err: crate::js::JsError) -> Self {
        match err {
            crate::js::JsError::Engine(message) => Self::Script(ScriptFailure::Engine { message }),
            crate::js::JsError::Interrupted => Self::Script(ScriptFailure::Interrupted),
            crate::js::JsError::BadTimerId => Self::Script(ScriptFailure::BadTimerId),
        }
    }
}
