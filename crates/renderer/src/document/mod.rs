//! One document: HTML tasks we own, browser services for dials and cookies. The
//! renderer loop owns every wait; the browser process owns the tab and drives
//! navigation.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Instant as WallClock;

use tokio::sync::Notify;

use blitz_traits::net::NetHandler;
use tokio::time::Instant;
use url::Url;

use crate::Parsed;
use crate::ReadyState;
use crate::documents::DocumentStore;
use crate::js::{DocumentStreamCommand, FrameNavigation, JsRuntimeHandle, RealmRegistry, World};
use crate::messaging::SharedHandle;
use crate::protocol::{
    BrowserServices, DialOutcome, FrameId, Mount, RendererEvent, ScriptFailure, TabError,
};

mod dial;
mod drain;
mod intern;

pub(crate) use crate::js::ScriptValue;

/// Concurrent `fetch` jobs one document may keep in flight.
const MAX_PENDING_JS_FETCHES: usize = 256;
/// Retained events before a document reports overflow.
const MAX_PENDING_EVENTS: usize = 2048;

enum Task {
    Timer(u32),
    DialFinished(CompletedDial),
    DialFailed(DialContext),
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
        epoch: u64,
    },
    ClassicScript {
        element: crate::js::world::NodeId,
        epoch: u64,
    },
    /// A child frame's own navigation
    /// (<https://html.spec.whatwg.org/multipage/browsing-the-web.html#navigate>);
    /// a superseded load is dropped when it completes.
    FrameLoad {
        sequence: u64,
    },
    /// A Blitz subresource (stylesheet, image, font) fetch. The response
    /// handler waits in [`Document::blitz_handlers`] under this id; delivery
    /// hands Blitz its bytes through the handler, which releases Blitz's own
    /// critical hold.
    BlitzResource {
        id: u64,
    },
}

/// One dial waiting to start, with the URL and initiator it runs under.
#[derive(Clone)]
pub(crate) struct QueuedDial {
    pub(crate) context: DialContext,
    pub(crate) url: Url,
    pub(crate) initiator: Url,
    /// HTTP method. Every dial except a form navigation is a GET.
    pub(crate) method: String,
    /// Request body, empty for a GET.
    pub(crate) body: Vec<u8>,
    /// `Content-Type` for `body`, when there is one.
    pub(crate) content_type: Option<String>,
    pub(crate) headers: Vec<(String, String)>,
    /// Browser-generated `Referer` URL. `None` omits the header.
    pub(crate) referrer: Option<String>,
}

impl QueuedDial {
    /// A GET dial, the shape of every dial but a form navigation.
    pub(crate) fn get(context: DialContext, url: Url, initiator: Url) -> Self {
        Self {
            context,
            url,
            initiator,
            method: "GET".to_owned(),
            body: Vec::new(),
            content_type: None,
            headers: Vec::new(),
            referrer: None,
        }
    }
}

/// One finished dial with the response it produced.
pub(crate) struct CompletedDial {
    pub(crate) context: DialContext,
    pub(crate) outcome: DialOutcome,
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

/// A connection transition for one `iframe` container in this document.
pub(crate) enum IframeLifecycle {
    /// A container whose frame is gone or disconnected.
    Removed(crate::js::world::NodeId),
}

/// A connection transition for one `<img>` element in this document.
pub(crate) enum ImageLifecycle {
    /// A newly connected image to start fetching.
    Connected(crate::js::world::NodeId),
    /// A disconnected image to stop fetching.
    Disconnected(crate::js::world::NodeId),
}

struct Timer {
    id: u32,
    when: Instant,
    fired: bool,
}

/// The process-wide handles every frame document shares: services, the JS
/// heap, the wake handle, the stop flag, and the cross-frame stores.
#[derive(Clone)]
pub(crate) struct FrameRuntime {
    pub(crate) services: Arc<dyn BrowserServices>,
    pub(crate) js_runtime: JsRuntimeHandle,
    pub(crate) wake: Arc<Notify>,
    pub(crate) stop: Arc<Stop>,
    pub(crate) documents: Rc<RefCell<DocumentStore>>,
    pub(crate) registry: Rc<RefCell<RealmRegistry>>,
    pub(crate) shared: SharedHandle,
    /// Storage changes waiting for the `storage`-event task in their receiving
    /// frames.
    pub(crate) pending_storage: Rc<RefCell<Vec<crate::storage::PendingStorageEvent>>>,
    /// Parley's font context, built once per process: construction scans
    /// system fonts (~25ms), so every `BaseDocument` clones this instead of
    /// rescanning. Cloning is cheap; Blitz shares the source cache.
    pub(crate) font_ctx: parley::FontContext,
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
    timers: Vec<Timer>,
    next_timer_id: u32,
    dial_tx: Sender<Result<CompletedDial, DialContext>>,
    dial_rx: Receiver<Result<CompletedDial, DialContext>>,
    in_flight_dials: usize,
    fetch_cancellations: HashMap<(u64, i32), crate::protocol::DialCancellation>,
    queued_dials: Vec<QueuedDial>,
    /// `<img>` elements awaiting Blitz image state. They delay the document
    /// load event; Blitz fetches and decodes, we only watch.
    /// Selected source URL for the current request
    /// (<https://html.spec.whatwg.org/multipage/images.html#updating-the-image-data>).
    image_selected_src: HashMap<crate::js::world::NodeId, String>,
    /// Elements with an unresolved image request for the current generation.
    in_flight_images: HashSet<crate::js::world::NodeId>,
    /// Connected `<img>` elements the last lifecycle scan saw, so insertion
    /// starts a fetch and removal drops one
    /// (<https://html.spec.whatwg.org/multipage/images.html#updating-the-image-data>).
    connected_images: HashSet<crate::js::world::NodeId>,
    /// Identifies the frame's current navigation; completions from superseded
    /// loads are dropped.
    frame_load_sequence: u64,
    /// Whether the frame's own navigation is still in flight, which is what
    /// keeps the container's `load` event from firing early.
    frame_load_in_flight: bool,
    /// Whether the frame is still on the `about:blank` document a browser
    /// creates at insertion. A URL comparison cannot answer this: a frame
    /// whose `src` resolves to the parent's own URL is still on that document
    /// (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#process-the-iframe-attributes>).
    initial_blank: bool,
    /// The `iframe` containers awaiting their child's load event; this
    /// document's own `load` event is delayed until they all have
    /// (<https://html.spec.whatwg.org/multipage/parsing.html#delay-the-load-event>).
    pending_frame_loads: HashSet<crate::js::world::NodeId>,
    /// Whether this document's `load` event has fired.
    load_fired: bool,
    events: Vec<RendererEvent>,
    events_overflowed: bool,
    js: Option<crate::js::JsRealm>,
    js_timer_slots: HashMap<u32, i32>,
    js_epoch: u64,
    /// Whether init scripts were deferred because the realm was created while
    /// a navigation response was still streaming.
    init_scripts_deferred: bool,
    /// Skip `QuickJS` realm creation while installing a document. Nested
    /// iframe insertion runs inside an active realm, and `QuickJS` forbids
    /// entering another one until that script returns.
    defer_js: bool,
    active_buffer: Option<String>,
    /// Scripts already executed for the current parse, so a walk resumed
    /// after a `src` fetch does not run them twice.
    executed_scripts: HashSet<crate::js::world::NodeId>,
    parser_eof: bool,
    /// Who owns the active parser, which decides whether `document.close()` may
    /// end it.
    parser_owner: ParserOwner,
    /// Decoder for a body that is still arriving; `None` when the markup is
    /// already in hand.
    decoder: Option<dial::ResponseDecoder>,
    /// The response `Content-Type` for the active parser. XML MIME types
    /// select the XML parser; anything else, including a script's
    /// `document.open()`, stays HTML.
    response_content_type: Option<String>,
    classic_fetch_in_flight: bool,
    deferred_modules: Vec<(crate::js::world::NodeId, crate::js::ScriptSource)>,
    stop: Arc<Stop>,
}

impl Drop for Document {
    fn drop(&mut self) {
        self.release();
    }
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
        let (dial_tx, dial_rx) = mpsc::channel();
        let world = Rc::new(RefCell::new(World::new(
            document_url.clone(),
            frame,
            runtime,
            Rc::clone(init_scripts),
        )));
        runtime.registry.borrow_mut().insert_frame(frame, &world);
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
            frame,
            viewport_size: crate::engine::DEFAULT_VIEWPORT,
            shared: Rc::clone(&runtime.shared),
            url: document_url,
            tasks: VecDeque::new(),
            timers: Vec::new(),
            next_timer_id: 1,
            dial_tx,
            dial_rx,
            in_flight_dials: 0,
            fetch_cancellations: HashMap::new(),
            queued_dials: Vec::new(),
            image_selected_src: HashMap::new(),
            in_flight_images: HashSet::new(),
            connected_images: HashSet::new(),
            frame_load_sequence: 0,
            frame_load_in_flight: false,
            initial_blank: true,
            pending_frame_loads: HashSet::new(),
            load_fired: false,
            events: Vec::new(),
            events_overflowed: false,
            js: None,
            js_timer_slots: HashMap::new(),
            js_epoch: 0,
            init_scripts_deferred: false,
            defer_js: false,
            active_buffer: None,
            executed_scripts: HashSet::new(),
            parser_eof: true,
            parser_owner: ParserOwner::Carrier,
            decoder: None,
            response_content_type: None,
            classic_fetch_in_flight: false,
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
        self.url.origin().ascii_serialization()
    }

    /// The root node of the frame's active document.
    pub(crate) fn document_root(&self) -> Option<crate::js::world::NodeId> {
        self.world
            .borrow()
            .with_main_document(|parsed| crate::js::world::NodeId {
                document: parsed.id,
                node: parsed.document.base.root_node().id,
            })
    }

    /// The `iframe`'s `src` attribute value, when the element has a
    /// non-empty one
    /// (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#attr-iframe-src>).
    #[must_use]
    pub(crate) fn frame_src(&self, container: crate::js::world::NodeId) -> Option<String> {
        let world = self.world.borrow();
        let value = world.document(container).and_then(|parsed| {
            crate::js::world::attr(&parsed.document.base, container.node, "src").map(str::to_owned)
        })?;
        (!value.is_empty()).then_some(value)
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

    /// An object URL's contents, created by this realm
    /// (<https://w3c.github.io/FileAPI/#dfn-createObjectURL>).
    #[must_use]
    pub(crate) fn object_url_contents(&self, url: &str) -> Option<Rc<str>> {
        self.world.borrow().object_url_contents(url)
    }

    /// The `Content-Type` an object URL was created from.
    #[must_use]
    pub(crate) fn object_url_type(&self, url: &str) -> Option<Rc<str>> {
        self.world.borrow().object_url_type(url)
    }

    /// The frame's connected `iframe` containers in tree order, which is what
    /// orders the frame's child browsing contexts
    /// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-length>).
    #[must_use]
    pub(crate) fn iframe_containers_in_order(&self) -> Vec<crate::js::world::NodeId> {
        self.world.borrow().iframe_containers_in_order()
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
    fn adopt_pending_frames_before(&mut self, before: crate::js::world::NodeId) {
        self.adopt_pending_frames_before_node(Some(before));
    }

    fn adopt_pending_frames_before_node(&mut self, before: Option<crate::js::world::NodeId>) {
        let created = World::adopt_pending_frames(&self.world, before);
        for container in created {
            // The child is loading from this moment on, so this document's
            // `load` event waits for it
            // (<https://html.spec.whatwg.org/multipage/parsing.html#delay-the-load-event>).
            self.mark_frame_load_pending(container);
        }
    }

    /// Applies writes the realm stored on a child frame's `contentWindow`
    /// before that frame's realm existed.
    pub(crate) fn flush_frame_proxy_sets(&mut self, child: FrameId) {
        self.fire_js(|js| js.flush_frame_sets(child.get()));
        self.adopt_js_work();
    }

    /// Takes the child frames this document's realm created for the engine to
    /// adopt.
    pub(crate) fn take_new_frames(&mut self) -> Vec<(FrameId, crate::js::world::NodeId, Document)> {
        self.world.borrow_mut().take_new_frames()
    }

    /// The document URL used by `about:blank` frames, which inherit the
    /// parent's origin and, in this engine, its URL
    /// (<https://html.spec.whatwg.org/multipage/browsers.html#determining-the-origin>).
    #[must_use]
    pub(crate) fn inherited_url(&self) -> String {
        self.url.as_str().to_owned()
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
                    if crate::js::world::is_connected(base, id) {
                        if crate::js::world::is_iframe_element(base, id) {
                            containers.push(crate::js::world::NodeId { document, node: id });
                        }
                        if crate::js::world::is_html_element(base, id, "img") {
                            images.push(crate::js::world::NodeId { document, node: id });
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
        let known: std::collections::HashSet<crate::js::world::NodeId> = shared
            .tree
            .children(self.frame)
            .iter()
            .filter_map(|child| shared.tree.container(*child))
            .collect();
        drop(shared);
        let connected_set: std::collections::HashSet<crate::js::world::NodeId> =
            connected.iter().copied().collect();
        let mut iframe_events = Vec::new();
        // Insertion is World::adopt_pending_frames / materialize_iframe in
        // tree order with scripts. Scanning connected iframes here would
        // start parser `src` loads before preceding classic scripts run.
        for container in &known {
            if !connected_set.contains(container) {
                iframe_events.push(IframeLifecycle::Removed(*container));
            }
        }
        let connected_image_set: std::collections::HashSet<crate::js::world::NodeId> =
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
        // Blitz link clicks land here without a container: they target this
        // frame itself. Container attribution (which iframe was clicked)
        // arrives when subdocument click routing wires up.
        navigations.extend(self.blitz_nav.drain().into_iter().map(|url| {
            FrameNavigation::get(
                crate::js::NavigationTarget::SelfFrame,
                url.as_str().to_owned(),
            )
        }));
        navigations
    }

    pub(crate) fn take_document_stream(&mut self) -> Vec<DocumentStreamCommand> {
        self.world.borrow_mut().take_document_stream()
    }

    pub(crate) fn fire_node_load(&mut self, id: crate::js::world::NodeId) {
        self.fire_js(|js| js.fire_node_load(id));
        self.adopt_js_work();
    }

    /// Queues the image fetch for a newly connected `<img>`, if it still needs one
    /// (<https://html.spec.whatwg.org/multipage/images.html#updating-the-image-data>).
    pub(crate) fn queue_connected_image(&mut self, element: crate::js::world::NodeId) {
        self.queue_image(element, false);
    }

    /// Drops a disconnected `<img>`'s decoded size and ignores its waiter.
    pub(crate) fn disconnect_image(&mut self, element: crate::js::world::NodeId) {
        self.in_flight_images.remove(&element);
        self.image_selected_src.remove(&element);
        self.world.borrow_mut().forget_image(element);
    }

    /// Starts the frame's own navigation. The dial runs on this document, so
    /// a navigation that replaces the frame cancels an unfinished one.
    pub(crate) fn navigate_to(
        &mut self,
        url: Url,
        initiator: Url,
        method: String,
        body: Vec<u8>,
        content_type: Option<String>,
    ) {
        self.frame_load_sequence = self.frame_load_sequence.wrapping_add(1);
        self.frame_load_in_flight = true;
        self.initial_blank = false;
        self.queued_dials.push(QueuedDial {
            context: DialContext::FrameLoad {
                sequence: self.frame_load_sequence,
            },
            url,
            initiator,
            method,
            body,
            content_type,
            headers: Vec::new(),
            referrer: None,
        });
    }

    /// Loads the frame's initial `about:blank`, inheriting `inherited_url` as
    /// the document URL when the parent supplies one
    /// (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#process-the-iframe-attributes>).
    pub(crate) fn load_about_blank(&mut self, inherited_url: Option<&str>) {
        if let Some(url) = inherited_url {
            self.apply_document_url(url);
        }
        self.load_html("");
        self.initial_blank = true;
        self.ensure_js_ok();
    }

    /// Installs the initial `about:blank` tree without creating a realm
    /// (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#process-the-iframe-attributes>).
    ///
    /// Used when a parent script inserts an `iframe`: the document must exist
    /// for `contentDocument` in the same task, but `QuickJS` cannot enter a new
    /// realm until that script returns.
    pub(crate) fn load_about_blank_tree(&mut self, inherited_url: Option<&str>) {
        if let Some(url) = inherited_url {
            self.apply_document_url(url);
        }
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

    /// Replaces this frame's document with a complete response, as the frame's
    /// own `src` load delivers it.
    pub(crate) fn load_frame_response(
        &mut self,
        url: &Url,
        content_type: Option<&str>,
        body: &[u8],
    ) {
        self.load_response_body(url, content_type, body);
        // Every frame has a window; make sure the realm exists even when the
        // document never runs a script, so a parent can set properties on it
        // (<https://html.spec.whatwg.org/multipage/window-object.html#the-window-object>).
        self.ensure_js_ok();
    }

    /// Runs a `javascript:` frame URL's script in the frame's realm, replacing
    /// the frame's document with the inherited blank first
    /// (<https://html.spec.whatwg.org/multipage/browsing-the-web.html#javascript-protocol>).
    pub(crate) fn load_javascript_frame(&mut self, inherited_url: Option<&str>, script: &str) {
        self.load_about_blank(inherited_url);
        self.eval_frame_script(script);
    }

    /// Runs one `javascript:` URL script in this frame's realm. A string
    /// result replaces the document with that string parsed as HTML
    /// (<https://html.spec.whatwg.org/multipage/browsing-the-web.html#javascript-protocol>).
    pub(crate) fn eval_frame_script(&mut self, script: &str) {
        if script.is_empty() {
            return;
        }
        match self.eval(script) {
            Ok(result) if !result.is_empty() => self.load_html(&result),
            Ok(_) => {}
            Err(_) => self.record_event(RendererEvent::ScriptFailed),
        }
    }

    fn apply_document_url(&mut self, url: &str) {
        if let Ok(url) = Url::parse(url) {
            self.url = url.clone();
            self.world.borrow_mut().document_url = url;
        }
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
        if url.origin() != self.url.origin() {
            return Err(TabError::InvalidUrl { spec: url.into() });
        }
        let previous_url = self.url.clone();
        let previous_history = self.world.borrow().history.clone();
        self.apply_document_url(url.as_str());
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
                self.apply_document_url(previous_url.as_str());
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
        let url = Url::parse(&mount.url).map_err(|_| TabError::InvalidUrl {
            spec: mount.url.clone(),
        })?;
        if let Some((width, height)) = mount.viewport {
            self.viewport_size = (width, height);
        }
        self.world.borrow_mut().history = mount.history.clone();
        self.load_response_body(&url, mount.content_type.as_deref(), &mount.body);
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
        self.active_buffer = Some(input.to_owned());
    }

    /// Starts a document from a network response: a new realm, the response's
    /// URL, and a body that may arrive in pieces.
    ///
    /// A URL that does not parse leaves the document on its previous URL.
    pub(crate) fn begin_response(
        &mut self,
        url: Option<&Url>,
        content_type: Option<&str>,
        viewport: Option<(u32, u32)>,
    ) {
        self.reset_js_realm();
        if let Some((width, height)) = viewport {
            self.viewport_size = (width, height);
        }
        if let Some(url) = url {
            self.url = url.clone();
        }
        self.initial_blank = false;
        let mut world = self.world.borrow_mut();
        world.document_url = self.url.clone();
        drop(world);
        // The realm is already new when this runs; this opens the parser and
        // the decoder for the response that will feed it.
        self.start_parser(
            "",
            false,
            ParserOwner::Carrier,
            Some(dial::ResponseDecoder::new(content_type.map(str::to_owned))),
        );
        self.response_content_type = content_type.map(str::to_owned);
    }

    /// Starts a document from a complete response: new realm, the response's
    /// URL, and the whole body.
    fn load_response_body(&mut self, url: &Url, content_type: Option<&str>, body: &[u8]) {
        self.begin_response(Some(url), content_type, None);
        self.write_body(body);
        self.end_body();
    }

    /// Feeds response bytes. The decoder holds them until the encoding is
    /// known, then the parser sees the markup.
    pub(crate) fn write_body(&mut self, bytes: &[u8]) {
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
        if let Some(buffer) = self.active_buffer.as_mut() {
            buffer.push_str(text);
        }
        // Known gap: without an incremental parser a write after the parse
        // finished has nowhere to go, so it is ignored instead of re-parsing.
    }

    /// Ends a body: flushes the decoder and lets the parser finish.
    pub(crate) fn end_body(&mut self) {
        if let Some(decoder) = self.decoder.take() {
            let text = decoder.finish();
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
        self.reset_js_realm();
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
        self.reset_js_realm();
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
    fn ensure_js_ok(&mut self) -> bool {
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
            if self.decoder.is_some() && self.active_buffer.is_some() {
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

    fn reset_js_realm(&mut self) {
        for (_, cancel) in self.fetch_cancellations.drain() {
            cancel();
        }
        // The old realm's ports are gone with it; the peers fire `close`
        // (<https://html.spec.whatwg.org/multipage/web-messaging.html#disentangle>).
        // A navigation also destroys this frame's own children.
        {
            let mut shared = self.shared.borrow_mut();
            shared.close_frame_ports(self.frame);
            shared.detach_child_frames(self.frame);
        }
        self.frame_load_in_flight = false;
        self.load_fired = false;
        self.pending_frame_loads.clear();
        // Any frame load that completes after this point belongs to the
        // replaced document, even when it was started through a synchronous
        // path that never bumped the sequence
        // (<https://html.spec.whatwg.org/multipage/browsing-the-web.html#navigate>).
        self.frame_load_sequence = self.frame_load_sequence.wrapping_add(1);
        self.js_epoch = self.js_epoch.saturating_add(1);
        // Every queued dial belongs to the old realm; drop them all.
        self.queued_dials.clear();
        for (_, handler) in self.blitz_handlers.drain() {
            Box::new(handler).bytes(String::new(), blitz_traits::net::Bytes::from(Vec::new()));
        }
        self.blitz_request_urls.clear();
        while self.blitz_fetch_rx.try_recv().is_ok() {}
        self.subresource_bytes.clear();
        self.world.borrow_mut().clear_images();
        self.world.borrow_mut().take_image_updates();
        self.image_selected_src.clear();
        self.in_flight_images.clear();
        self.connected_images.clear();
        let js_timer_ids: std::collections::HashSet<u32> =
            self.js_timer_slots.keys().copied().collect();
        self.timers
            .retain(|timer| !js_timer_ids.contains(&timer.id));
        self.js_timer_slots.clear();
        self.js = None;
        self.init_scripts_deferred = false;
        self.active_buffer = None;
        self.executed_scripts.clear();
        self.parser_eof = true;
        self.world.borrow_mut().parser_active = false;
        self.world.borrow_mut().current_script = None;
        let mut world = self.world.borrow_mut();
        let bytes = world
            .pending_html_writes
            .iter()
            .map(String::len)
            .sum::<usize>();
        world.pending_html_writes.clear();
        world.release_stream_bytes(bytes);
        self.classic_fetch_in_flight = false;
        self.deferred_modules.clear();
    }

    /// `document.open()`, `document.write()`, and `document.close()` all land
    /// on the same parser feed the network path uses, so a pending classic
    /// script blocks them the same way.
    pub(crate) fn apply_document_stream(&mut self, command: DocumentStreamCommand) {
        match command {
            DocumentStreamCommand::Open => self.reopen_document(),
            DocumentStreamCommand::Write(html) => self.write_text(&html),
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
        element: Option<crate::js::world::NodeId>,
        base_line: u32,
        filename: &str,
    ) {
        // https://html.spec.whatwg.org/multipage/webappapis.html#run-a-classic-script
        let previous = self.world.borrow().current_script;
        self.world.borrow_mut().current_script = element;
        self.fire_js(|js| js.eval_classic_script(source, base_line, filename));
        self.world.borrow_mut().current_script = previous;
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
        blitz_dom::DocumentConfig {
            net_provider: Some(std::sync::Arc::clone(&self.net_provider)
                as std::sync::Arc<dyn blitz_traits::net::NetProvider>),
            navigation_provider: Some(std::sync::Arc::clone(&self.blitz_nav)
                as std::sync::Arc<dyn blitz_traits::navigation::NavigationProvider>),
            shell_provider: Some(std::sync::Arc::new(crate::render::TinyShell)),
            base_url: Some(crate::render::blitz_base_url(&self.url)),
            font_ctx: Some(self.shared_font_ctx()),
            ..blitz_dom::DocumentConfig::default()
        }
    }

    /// The process-shared font context, cloned cheaply per document.
    fn shared_font_ctx(&self) -> parley::FontContext {
        self.world.borrow().runtime.font_ctx.clone()
    }

    /// Installs `parsed` as the active document and registers it, reporting
    /// whether a realm owns it afterwards.
    fn install_parsed(&mut self, parsed: Parsed) -> bool {
        if self.js.is_none() {
            let document = self.world.borrow_mut().replace_document(parsed);
            self.register_document(document);
            // Style resolution and scripts both read the viewport; apply the
            // frame's size before the realm evaluates anything.
            self.apply_viewport();
            if self.defer_js {
                return true;
            }
            self.ensure_js().is_ok()
        } else {
            let document = self.world.borrow_mut().set_document(parsed);
            self.register_document(document);
            self.apply_viewport();
            self.bind_realm_document();
            true
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
        if let Some(root) = base
            .root_node()
            .children
            .first()
            .and_then(|id| base.get_node(*id))
        {
            content.0 = content.0.max(root.scroll_width());
            content.1 = content.1.max(root.scroll_height());
        }
        content
    }

    /// Parses the accumulated markup as one whole document, installs it, and
    /// runs its scripts in tree order
    /// (<https://html.spec.whatwg.org/multipage/parsing.html#the-end>).
    fn finish_parse(&mut self) {
        if !self.parser_eof {
            return;
        }
        let Some(mut buffer) = self.active_buffer.take() else {
            return;
        };
        // Writes queued while the parser was active join this parse; later
        // writes have no incremental parser to feed (see `write_text`).
        let pending = std::mem::take(&mut self.world.borrow_mut().pending_html_writes);
        let pending_bytes = pending.iter().map(String::len).sum::<usize>();
        self.world.borrow().release_stream_bytes(pending_bytes);
        for chunk in pending {
            buffer.push_str(&chunk);
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
            crate::xml::parse_navigated(&buffer, content_type, self.blitz_config())
        } else {
            let mut parsed = crate::parse_html(&buffer, self.blitz_config());
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
        if self.run_scripts() {
            return;
        }
        self.load_images();
        // Deliver parser mutations before the document's events.
        self.deliver_mutations();
        self.fire_document_end();
    }

    // https://html.spec.whatwg.org/multipage/parsing.html#parsing-main-intext
    // https://html.spec.whatwg.org/multipage/scripting.html#prepare-the-script-element
    //
    /// Runs every not-yet-executed script in tree order, stopping (returning
    /// true) when a `src` fetch starts: the fetch completion resumes the
    /// walk, so later scripts still run in order
    /// (<https://html.spec.whatwg.org/multipage/scripting.html#pending-parsing-blocking-script>).
    fn run_scripts(&mut self) -> bool {
        let scripts: Vec<crate::js::world::NodeId> = {
            let world = self.world.borrow();
            let Some(parsed) = world.main_document() else {
                return false;
            };
            let base = &parsed.document.base;
            let document = parsed.id;
            let mut scripts = Vec::new();
            let mut stack = vec![base.root_node().id];
            while let Some(id) = stack.pop() {
                let Some(node) = base.get_node(id) else {
                    continue;
                };
                if crate::js::world::is_html_element(base, id, "script") {
                    scripts.push(crate::js::world::NodeId { document, node: id });
                }
                stack.extend(node.children.iter().rev().copied());
            }
            scripts
        };
        for id in scripts {
            if !self.executed_scripts.insert(id) {
                continue;
            }
            // Iframes that precede this script in tree order already have
            // browsing contexts; later ones wait, matching incremental
            // insertion
            // (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#the-iframe-element:html-element-insertion-steps>).
            self.adopt_pending_frames_before(id);
            // Microtask checkpoint before the script runs; parser
            // mutations queued since the last script deliver now.
            self.deliver_mutations();
            let script = crate::js::script_at(&self.world.borrow(), id);
            match script {
                Some(crate::js::Script::Classic(crate::js::ScriptSource::Inline {
                    source,
                    line,
                })) => {
                    let filename = self.url.as_str().to_owned();
                    self.eval_classic(&source, Some(id), line, &filename);
                }
                Some(crate::js::Script::Classic(crate::js::ScriptSource::Src(src))) => {
                    if let Ok(url) = self.resolve_dial_url(&src) {
                        self.classic_fetch_in_flight = true;
                        let initiator = self.url.clone();
                        self.queued_dials.push(QueuedDial::get(
                            DialContext::ClassicScript {
                                element: id,
                                epoch: self.js_epoch,
                            },
                            url,
                            initiator,
                        ));
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
        false
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

    fn base_url(&self) -> Url {
        let world = self.world.borrow();
        let base_href = world
            .with_main_document(|parsed| {
                let base = &parsed.document.base;
                let root = base.root_node().id;
                let base_el = base.query_selector_in(root, "base[href]").ok().flatten()?;
                crate::js::world::attr(base, base_el, "href").map(str::to_owned)
            })
            .flatten();
        let Some(href) = base_href else {
            return self.url.clone();
        };
        self.url.join(&href).unwrap_or_else(|_| self.url.clone())
    }

    pub(in crate::document) fn finish_dial(&mut self, done: CompletedDial) {
        let CompletedDial { context, outcome } = done;
        match context {
            DialContext::JsFetch { id, epoch } => {
                self.record_event(RendererEvent::Fetch {
                    status: outcome.status,
                });
                if epoch == self.js_epoch {
                    self.settle_js_fetch(id, Some(outcome));
                }
            }
            DialContext::ClassicScript { element, epoch } => {
                self.record_event(RendererEvent::Fetch {
                    status: outcome.status,
                });
                if epoch == self.js_epoch {
                    self.classic_fetch_in_flight = false;
                    if (200..300).contains(&outcome.status) {
                        let source = String::from_utf8_lossy(&outcome.body);
                        let filename = outcome.final_url.clone();
                        self.eval_classic(&source, Some(element), 1, &filename);
                    }
                    self.resume_scripts();
                }
            }
            DialContext::FrameLoad { sequence } => {
                // The superseded-load early return deliberately skips the
                // trailing `adopt_js_work()` below.
                if sequence != self.frame_load_sequence {
                    return;
                }
                self.frame_load_in_flight = false;
                let Ok(url) = Url::parse(&outcome.final_url) else {
                    return;
                };
                // Announce the commit before the new document's load, so the
                // browser can re-create the top-level execution contexts
                // (<https://chromedevtools.github.io/devtools-protocol/tot/Page/#event-frameNavigated>).
                self.record_event(RendererEvent::Navigated {
                    url: url.to_string(),
                });
                self.load_frame_response(&url, outcome.content_type.as_deref(), &outcome.body);
            }
            DialContext::BlitzResource { id } => {
                // Stale deliveries are harmless: the handler reports into
                // Blitz's own channel, which a dropped tree no longer drains.
                if (200..300).contains(&outcome.status) {
                    self.subresource_bytes
                        .insert(outcome.final_url.clone(), outcome.body.clone());
                }
                if let Some(handler) = self.blitz_handlers.remove(&id) {
                    let request_url = self.blitz_request_urls.remove(&id).unwrap_or_default();
                    Box::new(handler).bytes(
                        outcome.final_url.clone(),
                        blitz_traits::net::Bytes::from(outcome.body),
                    );
                    // The arrival changes style or layout; apply it now so the
                    // tree is current for the next capture instead of waiting
                    // for one to pump the resolve loop.
                    self.settle_blitz_layout();
                    self.settle_image_waiters(&request_url);
                }
            }
        }
        self.adopt_js_work();
    }

    pub(in crate::document) fn fail_dial(&mut self, fail: DialContext) {
        self.record_event(RendererEvent::FetchFailed);
        match fail {
            DialContext::JsFetch { id, epoch } => {
                if epoch == self.js_epoch {
                    self.settle_js_fetch(id, None);
                }
            }
            DialContext::ClassicScript { epoch, .. } => {
                if epoch == self.js_epoch {
                    self.classic_fetch_in_flight = false;
                    self.resume_scripts();
                }
            }
            DialContext::FrameLoad { sequence } => {
                if sequence == self.frame_load_sequence {
                    // Keep the frame's current (about:blank) document; the
                    // container's load event still fires because the frame is
                    // no longer waiting.
                    self.frame_load_in_flight = false;
                }
            }
            DialContext::BlitzResource { id } => {
                // Deliver empty bytes so Blitz releases the resource as
                // failed instead of holding it pending forever.
                if let Some(handler) = self.blitz_handlers.remove(&id) {
                    let request_url = self.blitz_request_urls.remove(&id).unwrap_or_default();
                    Box::new(handler).bytes(String::new(), blitz_traits::net::Bytes::new());
                    self.settle_blitz_layout();
                    self.settle_image_waiters(&request_url);
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
        self.fire_document_load();
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
        self.fire_document_load();
    }

    fn run_deferred_modules(&mut self) {
        let modules = std::mem::take(&mut self.deferred_modules);
        for (index, (element, source)) in modules.into_iter().enumerate() {
            let result = match source {
                crate::js::ScriptSource::Inline { source, .. } => {
                    let name = format!("{}#inline-module-{index}", self.url);
                    self.js
                        .as_ref()
                        .map_or(Ok(()), |js| js.eval_inline_module(&name, &source))
                }
                crate::js::ScriptSource::Src(src) => match self.resolve_dial_url(&src) {
                    Ok(url) => self
                        .js
                        .as_ref()
                        .map_or(Ok(()), |js| js.eval_external_module(url.as_str())),
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

    fn fire_document_load(&mut self) {
        // `load` only follows `DOMContentLoaded`, which `fire_document_end`
        // dispatches before calling here.
        if self.world.borrow().main_ready_state() != ReadyState::Interactive {
            return;
        }
        if !self.in_flight_images.is_empty() {
            return;
        }
        // Blitz's critical resources (head stylesheets, fonts) delay the load
        // event too
        // (<https://html.spec.whatwg.org/multipage/parsing.html#delay-the-load-event>).
        let pending_critical = self
            .world
            .borrow()
            .main_document()
            .is_some_and(|parsed| parsed.document.base.has_pending_critical_resources());
        if pending_critical {
            return;
        }
        self.world
            .borrow_mut()
            .set_main_ready_state(ReadyState::Complete);
        self.fire_js(crate::js::JsRealm::fire_ready_state_change);
        self.maybe_fire_load();
    }

    /// Registers the current document's `<img src>` requests as Blitz image
    /// waiters. Image requests delay the load event until they succeed or fail
    /// (<https://html.spec.whatwg.org/multipage/images.html#updating-the-image-data>).
    fn load_images(&mut self) {
        let images: Vec<crate::js::world::NodeId> = self
            .world
            .borrow()
            .with_main_document(|parsed| {
                let base = &parsed.document.base;
                let document = parsed.id;
                let mut images = Vec::new();
                let mut stack = vec![base.root_node().id];
                while let Some(id) = stack.pop() {
                    if crate::js::world::is_html_element(base, id, "img")
                        && crate::js::world::attr(base, id, "src").is_some()
                    {
                        images.push(crate::js::world::NodeId { document, node: id });
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
                self.settle_image_waiters(&url);
                continue;
            }
            self.blitz_request_urls.insert(fetch.id, url);
            self.blitz_handlers.insert(fetch.id, handler);
            // The Fetch initiator is the document, not the fetch target: the
            // provider never sees the document URL, so it stamps the target
            // as initiator and the document corrects it here
            // (<https://fetch.spec.whatwg.org/#concept-request-initiator>).
            let initiator = self.url.clone();
            self.queued_dials.push(QueuedDial::get(
                DialContext::BlitzResource { id: fetch.id },
                fetch.url,
                initiator,
            ));
        }
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
        element: crate::js::world::NodeId,
        force: bool,
    ) {
        if !force
            && (self.in_flight_images.contains(&element)
                || self.world.borrow().images.contains_key(&element))
        {
            return;
        }
        let src = self.world.borrow().document(element).and_then(|parsed| {
            crate::js::world::is_html_element(&parsed.document.base, element.node, "img")
                .then(|| {
                    crate::js::world::attr(&parsed.document.base, element.node, "src")
                        .map(str::to_owned)
                })
                .flatten()
        });
        let Some(src) = src.filter(|src| !src.is_empty()) else {
            self.in_flight_images.remove(&element);
            self.image_selected_src.remove(&element);
            self.world.borrow_mut().forget_image(element);
            self.fire_js(|js| js.fire_node_error(element));
            return;
        };
        let Ok(url) = self.resolve_dial_url(&src) else {
            self.in_flight_images.remove(&element);
            self.image_selected_src.remove(&element);
            self.world.borrow_mut().fail_image(element, src);
            self.fire_js(|js| js.fire_node_error(element));
            return;
        };
        let selected = url.as_str().to_owned();
        self.image_selected_src.insert(element, selected.clone());
        self.world
            .borrow_mut()
            .begin_image(element, selected.clone());
        if let Some((width, height)) = self.blitz_image_dims(element) {
            self.image_selected_src.remove(&element);
            self.world
                .borrow_mut()
                .store_image_dims(element, width, height, selected);
            self.fire_js(|js| js.fire_node_load(element));
            self.fire_document_load();
            return;
        }
        self.in_flight_images.insert(element);
    }

    /// Decoded dimensions Blitz holds for `element`'s current request, if it
    /// decoded one. Blitz decodes synchronously on delivery, so after
    /// [`Self::settle_blitz_layout`] this is settled for every delivered URL.
    fn blitz_image_dims(&self, element: crate::js::world::NodeId) -> Option<(u32, u32)> {
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
    fn settle_image_waiters(&mut self, url: &str) {
        let waiting: Vec<crate::js::world::NodeId> = self
            .in_flight_images
            .iter()
            .filter(|element| {
                self.image_selected_src
                    .get(element)
                    .is_some_and(|selected| selected == url)
            })
            .copied()
            .collect();
        for element in &waiting {
            self.in_flight_images.remove(element);
            let selected = self.image_selected_src.remove(element).unwrap_or_default();
            if let Some((width, height)) = self.blitz_image_dims(*element) {
                self.world
                    .borrow_mut()
                    .store_image_dims(*element, width, height, selected);
                self.fire_js(|js| js.fire_node_load(*element));
            } else {
                self.world.borrow_mut().fail_image(*element, selected);
                self.fire_js(|js| js.fire_node_error(*element));
            }
            self.adopt_js_work();
        }
        if !waiting.is_empty() {
            self.fire_document_load();
        }
    }

    /// Fires this document's `load` event once it and every child browsing
    /// context have finished loading
    /// (<https://html.spec.whatwg.org/multipage/parsing.html#delay-the-load-event>).
    pub(crate) fn maybe_fire_load(&mut self) {
        if self.load_fired
            || !self.pending_frame_loads.is_empty()
            || self.world.borrow().main_ready_state() != ReadyState::Complete
        {
            return;
        }
        self.load_fired = true;
        self.record_event(RendererEvent::Load);
        self.fire_js(crate::js::JsRealm::fire_load);
        self.adopt_js_work();
    }

    /// A child browsing context started loading; this document's `load` event
    /// waits for it, and the container is remembered until it finishes.
    pub(crate) fn mark_frame_load_pending(&mut self, container: crate::js::world::NodeId) {
        self.pending_frame_loads.insert(container);
    }

    /// The containers whose child frames have not finished loading.
    pub(crate) fn pending_frame_loads(&self) -> Vec<crate::js::world::NodeId> {
        self.pending_frame_loads.iter().copied().collect()
    }

    /// Fires one container's `load` event and lets this document's own `load`
    /// event proceed once no child is left loading.
    pub(crate) fn finish_frame_load(&mut self, container: crate::js::world::NodeId) -> bool {
        if !self.pending_frame_loads.remove(&container) {
            return false;
        }
        self.fire_node_load(container);
        self.maybe_fire_load();
        true
    }

    /// Drops a container that went away before its child finished loading.
    pub(crate) fn cancel_frame_load(&mut self, container: crate::js::world::NodeId) {
        if self.pending_frame_loads.remove(&container) {
            self.maybe_fire_load();
        }
    }

    pub(in crate::document) fn record_event(&mut self, event: RendererEvent) {
        if self.events.len() == MAX_PENDING_EVENTS {
            self.events_overflowed = true;
            return;
        }
        self.events.push(event);
    }
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

/// Stop flag shared by a renderer loop, the JS interrupt handler, and dials.
///
/// Requesting a stop interrupts running script and marks the flag; every
/// engine sharing it winds down at its next turn.
pub struct Stop {
    flag: AtomicBool,
}

impl Default for Stop {
    fn default() -> Self {
        Self::new()
    }
}

impl Stop {
    /// A fresh, unset stop flag.
    #[must_use]
    pub fn new() -> Self {
        Self {
            flag: AtomicBool::new(false),
        }
    }

    /// Requests a stop: interrupts `QuickJS` and marks the flag.
    pub fn request(&self) {
        self.flag.store(true, Ordering::Relaxed);
    }

    /// Whether [`Stop::request`] has run.
    #[must_use]
    pub fn is_set(&self) -> bool {
        self.flag.load(Ordering::Relaxed)
    }
}
