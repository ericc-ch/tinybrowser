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
use tokio::time::Instant;
use url::Url;

use crate::ActiveParser;
use crate::Parsed;
use crate::ReadyState;
use crate::documents::DocumentStore;
use crate::js::{DocumentStreamCommand, FrameNavigation, RealmRegistry, SharedJsRuntime, World};
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
        element: dom::NodeId,
        epoch: u64,
    },
    /// A `<link rel=stylesheet>` sheet; loading sheets delay the load event.
    Stylesheet {
        element: dom::NodeId,
        epoch: u64,
    },
    /// An `<img>` resource selected by its `src` attribute.
    Image {
        element: dom::NodeId,
        epoch: u64,
        generation: u64,
    },
    /// A child frame's own navigation
    /// (<https://html.spec.whatwg.org/multipage/browsing-the-web.html#navigate>);
    /// a superseded load is dropped when it completes.
    FrameLoad {
        sequence: u64,
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
    pub(crate) js_runtime: SharedJsRuntime,
    pub(crate) wake: Arc<Notify>,
    pub(crate) stop: Arc<Stop>,
    pub(crate) documents: Rc<RefCell<DocumentStore>>,
    pub(crate) registry: Rc<RefCell<RealmRegistry>>,
    pub(crate) shared: SharedHandle,
    /// Storage changes waiting for the `storage`-event task in their receiving
    /// frames.
    pub(crate) pending_storage: Rc<RefCell<Vec<crate::storage::PendingStorageEvent>>>,
}

/// One document: tree, task list, `QuickJS` realm, and browser services.
#[allow(
    clippy::struct_excessive_bools,
    reason = "each bool mirrors an HTML document or parser flag"
)]
pub(crate) struct Document {
    services: Arc<dyn BrowserServices>,
    world: Rc<RefCell<World>>,
    js_runtime: SharedJsRuntime,
    wake: Arc<Notify>,
    /// The browsing context this document belongs to.
    frame: FrameId,
    /// The renderer-process state every frame shares.
    shared: SharedHandle,
    url: Url,
    content_language: Option<String>,
    tasks: VecDeque<Task>,
    timers: Vec<Timer>,
    next_timer_id: u32,
    dial_tx: Sender<Result<CompletedDial, DialContext>>,
    dial_rx: Receiver<Result<CompletedDial, DialContext>>,
    in_flight_dials: usize,
    queued_dials: Vec<QueuedDial>,
    /// Loaded `<link rel=stylesheet>` sheets, keyed by their link element so
    /// the renderer can splice them into the cascade at the right position.
    stylesheets: HashMap<dom::NodeId, String>,
    /// Every stylesheet URL already queued or loaded, so re-scans do not
    /// refetch.
    stylesheet_urls: HashSet<String>,
    /// Stylesheet dials queued or in flight; the load event waits for them
    /// (<https://html.spec.whatwg.org/multipage/links.html#link-type-stylesheet>).
    pending_stylesheets: usize,
    /// Image fetches queued or in flight. They delay the document load event.
    pending_images: usize,
    /// Per-element fetch generation so a superseded `src` completion is ignored.
    image_generations: HashMap<dom::NodeId, u64>,
    /// Selected source URL for the current generation
    /// (<https://html.spec.whatwg.org/multipage/images.html#updating-the-image-data>).
    image_selected_src: HashMap<dom::NodeId, String>,
    /// Elements with an in-flight image fetch for the current generation.
    in_flight_images: HashSet<dom::NodeId>,
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
    /// Mutation serial the child frame order was last computed from, so a
    /// same-document move of an `iframe` is noticed without walking the tree
    /// every turn (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-length>).
    frame_order_serial: u64,
    /// The `iframe` containers awaiting their child's load event; this
    /// document's own `load` event is delayed until they all have
    /// (<https://html.spec.whatwg.org/multipage/parsing.html#delay-the-load-event>).
    pending_frame_loads: HashSet<dom::NodeId>,
    /// Whether this document's `load` event has fired.
    load_fired: bool,
    events: Vec<RendererEvent>,
    events_overflowed: bool,
    js: Option<crate::js::JsRealm>,
    js_timer_slots: HashMap<u32, i32>,
    js_epoch: u64,
    active_parser: Option<ActiveParser>,
    parser_eof: bool,
    /// Who owns the active parser, which decides whether `document.close()` may
    /// end it.
    parser_owner: ParserOwner,
    /// Decoder for a body that is still arriving; `None` when the markup is
    /// already in hand.
    decoder: Option<dial::ResponseDecoder>,
    classic_fetch_in_flight: bool,
    deferred_modules: Vec<(dom::NodeId, crate::js::ScriptSource)>,
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
    pub(crate) fn with_shared(frame: FrameId, runtime: &FrameRuntime) -> Self {
        let document_url = Url::parse("about:blank").expect("about:blank is a valid URL");
        let (dial_tx, dial_rx) = mpsc::channel();
        let world = Rc::new(RefCell::new(World::new(
            document_url.clone(),
            frame,
            runtime,
        )));
        runtime.registry.borrow_mut().insert_frame(frame, &world);
        Self {
            world,
            services: Arc::clone(&runtime.services),
            js_runtime: runtime.js_runtime.clone(),
            wake: Arc::clone(&runtime.wake),
            frame,
            shared: Rc::clone(&runtime.shared),
            url: document_url,
            content_language: None,
            tasks: VecDeque::new(),
            timers: Vec::new(),
            next_timer_id: 1,
            dial_tx,
            dial_rx,
            in_flight_dials: 0,
            queued_dials: Vec::new(),
            stylesheets: HashMap::new(),
            stylesheet_urls: HashSet::new(),
            pending_stylesheets: 0,
            pending_images: 0,
            image_generations: HashMap::new(),
            image_selected_src: HashMap::new(),
            in_flight_images: HashSet::new(),
            frame_load_sequence: 0,
            frame_load_in_flight: false,
            initial_blank: true,
            frame_order_serial: u64::MAX,
            pending_frame_loads: HashSet::new(),
            load_fired: false,
            events: Vec::new(),
            events_overflowed: false,
            js: None,
            js_timer_slots: HashMap::new(),
            js_epoch: 0,
            active_parser: None,
            parser_eof: true,
            parser_owner: ParserOwner::Carrier,
            decoder: None,
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
    pub(crate) fn document_root(&self) -> Option<dom::NodeId> {
        self.world
            .borrow()
            .with_main_document(|parsed| parsed.dom.document())
    }

    /// The `iframe`'s `src` attribute value, when the element has a
    /// non-empty one
    /// (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#attr-iframe-src>).
    #[must_use]
    pub(crate) fn frame_src(&self, container: dom::NodeId) -> Option<String> {
        let world = self.world.borrow();
        let value = world
            .document(container)
            .and_then(|parsed| parsed.dom.attribute(container, "src"))?;
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
    pub(crate) fn iframe_containers_in_order(&self) -> Vec<dom::NodeId> {
        self.world.borrow().iframe_containers_in_order()
    }

    /// Creates the browsing context of every connected `iframe` that lacks
    /// one, so a script sees it in the same task
    /// (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#process-the-iframe-attributes>).
    pub(crate) fn adopt_pending_frames(&mut self) {
        let created = self.world.borrow_mut().adopt_pending_frames();
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
    pub(crate) fn take_new_frames(&mut self) -> Vec<(FrameId, dom::NodeId, Document)> {
        self.world.borrow_mut().take_new_frames()
    }

    /// The document URL used by `about:blank` frames, which inherit the
    /// parent's origin and, in this engine, its URL
    /// (<https://html.spec.whatwg.org/multipage/browsers.html#determining-the-origin>).
    #[must_use]
    pub(crate) fn inherited_url(&self) -> String {
        self.url.as_str().to_owned()
    }

    pub(crate) fn take_lifecycle(&mut self) -> Vec<dom::Lifecycle> {
        self.world
            .borrow()
            .main_document_mut()
            .map_or_else(Vec::new, |mut parsed| parsed.dom.take_lifecycle())
    }

    pub(crate) fn take_frame_navigations(&mut self) -> Vec<FrameNavigation> {
        self.world.borrow_mut().take_frame_navigations()
    }

    pub(crate) fn take_document_stream(&mut self) -> Vec<DocumentStreamCommand> {
        self.world.borrow_mut().take_document_stream()
    }

    pub(crate) fn fire_node_load(&mut self, id: dom::NodeId) {
        self.fire_js(|js| js.fire_node_load(id));
        self.adopt_js_work();
    }

    /// Whether `id` is an HTML `iframe` in this document.
    #[must_use]
    pub(crate) fn is_iframe_element(&self, id: dom::NodeId) -> bool {
        self.world
            .borrow()
            .document(id)
            .is_some_and(|parsed| parsed.dom.is_iframe_element(id))
    }

    /// Queues the image fetch for a newly connected `<img>`, if it still needs one
    /// (<https://html.spec.whatwg.org/multipage/images.html#updating-the-image-data>).
    pub(crate) fn queue_connected_image(&mut self, element: dom::NodeId) {
        self.queue_image(element, false);
    }

    /// Drops a disconnected `<img>`'s decoded pixels and ignores in-flight fetches.
    pub(crate) fn disconnect_image(&mut self, element: dom::NodeId) {
        self.bump_image_generation(element);
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

    /// Whether the frame is still on the document created at insertion.
    #[must_use]
    pub(crate) fn is_initial_blank(&self) -> bool {
        self.initial_blank
    }

    /// Whether the child frame order may have changed since the last scan.
    ///
    /// The DOM mutation serial is the cheap detector for a same-document
    /// `iframe` move, which no connection lifecycle event reports
    /// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-length>).
    pub(crate) fn frame_order_changed(&mut self) -> bool {
        let serial = self
            .world
            .borrow()
            .with_main_document(|parsed| parsed.dom.mutation_serial());
        match serial {
            Some(serial) if serial == self.frame_order_serial => false,
            Some(serial) => {
                self.frame_order_serial = serial;
                true
            }
            None => false,
        }
    }

    /// Replaces this frame's document with a complete response, as the frame's
    /// own `src` load delivers it.
    pub(crate) fn load_frame_response(
        &mut self,
        url: &Url,
        content_type: Option<&str>,
        content_language: Option<&str>,
        body: &[u8],
    ) {
        self.load_response_body(url, content_type, content_language, body);
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

    /// Runs one `javascript:` URL script in this frame's realm; a string
    /// result would replace the document, which the engine does not model, so
    /// the result is discarded.
    pub(crate) fn eval_frame_script(&mut self, script: &str) {
        if script.is_empty() {
            return;
        }
        if self.eval(script).is_err() {
            self.record_event(RendererEvent::ScriptFailed);
        }
    }

    fn apply_document_url(&mut self, url: &str) {
        if let Ok(url) = Url::parse(url) {
            self.url = url.clone();
            self.world.borrow_mut().document_url = url;
        }
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
        source: &str,
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
        self.load_response_body(
            &url,
            mount.content_type.as_deref(),
            mount.content_language.as_deref(),
            &mount.body,
        );
        Ok(())
    }

    /// Opens a parser on `input`, taking ownership of the input stream.
    ///
    /// `decoder` is set exactly when bytes still arrive from the carrier; a
    /// script's `document.write` feeds text directly instead.
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
        self.active_parser = Some(ActiveParser::new(input));
    }

    /// Starts a document from a network response: a new realm, the response's
    /// URL and language, and a body that may arrive in pieces.
    ///
    /// A URL that does not parse leaves the document on its previous URL, and
    /// an absent language clears it, because both come from this response.
    pub(crate) fn begin_response(
        &mut self,
        url: Option<&Url>,
        content_type: Option<&str>,
        content_language: Option<&str>,
    ) {
        self.reset_js_realm();
        if let Some(url) = url {
            self.url = url.clone();
        }
        self.initial_blank = false;
        self.content_language = content_language.map(str::to_owned);
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
    }

    /// Starts a document from a complete response: new realm, the response's
    /// URL and language, and the whole body.
    fn load_response_body(
        &mut self,
        url: &Url,
        content_type: Option<&str>,
        content_language: Option<&str>,
        body: &[u8],
    ) {
        self.begin_response(Some(url), content_type, content_language);
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
        self.write_text(text);
    }

    /// Feeds markup that is already decoded, as a script's `document.write`
    /// provides.
    pub(crate) fn write_text(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        if let Some(parser) = &self.active_parser {
            parser.append_html(text);
            // A pending parsing-blocking script stops the parser: advancing
            // past it would run later scripts first. Appending is safe, so the
            // markup is not lost.
            // https://html.spec.whatwg.org/multipage/scripting.html#pending-parsing-blocking-script
            if !self.classic_fetch_in_flight {
                self.advance_parser();
            }
        }
    }

    /// Ends a body: flushes the decoder and lets the parser finish.
    pub(crate) fn end_body(&mut self) {
        if let Some(decoder) = self.decoder.take() {
            let text = decoder.finish();
            self.write_text(text);
        }
        self.parser_eof = true;
        if !self.classic_fetch_in_flight {
            self.advance_parser();
        }
    }

    /// Aborts a body the carrier will not finish.
    ///
    /// Without this the frame waits for bytes that never come, so its load
    /// never fires and the tab stays loading forever.
    pub(crate) fn abort_body(&mut self) {
        self.decoder = None;
        self.parser_eof = true;
        if !self.classic_fetch_in_flight {
            self.advance_parser();
        }
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
        self.advance_parser();
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
        }
        Ok(())
    }

    fn reset_js_realm(&mut self) {
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
        // Sheets belong to the replaced document; the new parse re-scans.
        self.stylesheets.clear();
        self.stylesheet_urls.clear();
        self.pending_stylesheets = 0;
        self.world.borrow_mut().clear_images();
        self.world.borrow_mut().take_image_updates();
        self.pending_images = 0;
        self.image_generations.clear();
        self.image_selected_src.clear();
        self.in_flight_images.clear();
        let js_timer_ids: std::collections::HashSet<u32> =
            self.js_timer_slots.keys().copied().collect();
        self.timers
            .retain(|timer| !js_timer_ids.contains(&timer.id));
        self.js_timer_slots.clear();
        self.js = None;
        self.active_parser = None;
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
            DocumentStreamCommand::Write(html) => self.write_text(html),
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
        element: Option<dom::NodeId>,
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

    /// Installs `parsed` as the active document and registers it, reporting
    /// whether a realm owns it afterwards.
    fn install_parsed(&mut self, mut parsed: Parsed) -> bool {
        parsed
            .dom
            .set_document_language(self.content_language.clone());
        if self.js.is_none() {
            let document = self.world.borrow_mut().replace_document(parsed);
            self.register_document(document);
            self.ensure_js().is_ok()
        } else {
            let document = self.world.borrow_mut().set_document(parsed);
            self.register_document(document);
            true
        }
    }

    // https://html.spec.whatwg.org/multipage/parsing.html#parsing-main-intext
    // https://html.spec.whatwg.org/multipage/scripting.html#prepare-the-script-element
    fn advance_parser(&mut self) {
        loop {
            let Some(parser) = self.active_parser.as_ref() else {
                return;
            };
            match parser.advance() {
                crate::ParseProgress::Script(id) => {
                    let parsed = parser.take_state();
                    if !self.install_parsed(parsed) {
                        self.record_event(RendererEvent::ScriptFailed);
                        self.sync_parser_from_world();
                        continue;
                    }
                    // A script may query a child frame's window; the browsing
                    // context must exist by then.
                    self.adopt_pending_frames();
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
                            self.sync_parser_from_world();
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
                                return;
                            }
                            self.sync_parser_from_world();
                        }
                        Some(crate::js::Script::Module(source)) => {
                            // Module scripts are deferred by default: parsing
                            // continues, then the queue runs in document order
                            // before `DOMContentLoaded`
                            // (<https://html.spec.whatwg.org/multipage/scripting.html#attr-script-defer>).
                            self.deferred_modules.push((id, source));
                            self.sync_parser_from_world();
                        }
                        None => self.sync_parser_from_world(),
                    }
                }
                crate::ParseProgress::Done => {
                    if !self.parser_eof {
                        return;
                    }
                    let Some(parser) = self.active_parser.take() else {
                        return;
                    };
                    let parsed = parser.finish();
                    if !self.install_parsed(parsed) {
                        self.record_event(RendererEvent::ScriptFailed);
                        return;
                    }
                    self.world.borrow_mut().parser_active = false;
                    // Style sheets delay the load event, so queue them before
                    // the document's end events.
                    self.load_stylesheets();
                    self.load_images();
                    // Deliver parser mutations before the document's events.
                    self.deliver_mutations();
                    self.fire_document_end();
                    return;
                }
            }
        }
    }

    pub(in crate::document) fn resolve_dial_url(&self, spec: &str) -> Result<Url, TabError> {
        let url = self
            .resolve_frame_url(spec)
            .ok_or_else(|| TabError::InvalidUrl { spec: spec.into() })?;
        if url.scheme() != "http" && url.scheme() != "https" {
            return Err(TabError::InvalidUrl { spec: spec.into() });
        }
        Ok(url)
    }

    fn base_url(&self) -> Url {
        let world = self.world.borrow();
        let Some(parsed) = world.main_document() else {
            return self.url.clone();
        };
        let Ok(Some(base_el)) = parsed.dom.select_first(parsed.dom.document(), "base[href]") else {
            return self.url.clone();
        };
        let Some(href) = parsed.dom.attribute(base_el, "href") else {
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
                    let body = String::from_utf8_lossy(&outcome.body);
                    self.settle_js_fetch(id, true, i32::from(outcome.status), &body);
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
                    self.sync_parser_from_world();
                    self.advance_parser();
                }
            }
            DialContext::Stylesheet { element, epoch } => {
                // A navigation supersedes this dial: the counter and the
                // stylesheet map now belong to the new document, so a stale
                // completion must not decrement them or fire its load event
                // early.
                if epoch != self.js_epoch {
                    return;
                }
                self.record_event(RendererEvent::Fetch {
                    status: outcome.status,
                });
                self.pending_stylesheets = self.pending_stylesheets.saturating_sub(1);
                if (200..300).contains(&outcome.status) {
                    self.stylesheets
                        .insert(element, String::from_utf8_lossy(&outcome.body).into_owned());
                }
                self.fire_document_load();
            }
            DialContext::Image {
                element,
                epoch,
                generation,
            } => {
                if epoch != self.js_epoch {
                    return;
                }
                self.record_event(RendererEvent::Fetch {
                    status: outcome.status,
                });
                self.pending_images = self.pending_images.saturating_sub(1);
                if self.image_generation(element) == generation {
                    self.in_flight_images.remove(&element);
                    let selected = self.image_selected_src.remove(&element).unwrap_or_default();
                    let loaded = (200..300).contains(&outcome.status)
                        && crate::render::decode_image(&outcome.body).is_some_and(|image| {
                            self.world
                                .borrow_mut()
                                .store_image(element, image, selected.clone())
                        });
                    if loaded {
                        self.fire_js(|js| js.fire_node_load(element));
                    } else {
                        self.world.borrow_mut().fail_image(element, selected);
                        self.fire_js(|js| js.fire_node_error(element));
                    }
                    self.adopt_js_work();
                }
                self.fire_document_load();
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
                self.load_frame_response(
                    &url,
                    outcome.content_type.as_deref(),
                    outcome.content_language.as_deref(),
                    &outcome.body,
                );
            }
        }
        self.adopt_js_work();
    }

    pub(in crate::document) fn fail_dial(&mut self, fail: DialContext) {
        self.record_event(RendererEvent::FetchFailed);
        match fail {
            DialContext::JsFetch { id, epoch } => {
                if epoch == self.js_epoch {
                    self.settle_js_fetch(id, false, 0, "");
                }
            }
            DialContext::ClassicScript { epoch, .. } => {
                if epoch == self.js_epoch {
                    self.classic_fetch_in_flight = false;
                    self.sync_parser_from_world();
                    self.advance_parser();
                }
            }
            DialContext::Stylesheet { epoch, .. } => {
                // A failed sheet is simply absent; the load event proceeds.
                // Stale dials from a superseded navigation are ignored.
                if epoch == self.js_epoch {
                    self.pending_stylesheets = self.pending_stylesheets.saturating_sub(1);
                    self.fire_document_load();
                }
            }
            DialContext::Image {
                element,
                epoch,
                generation,
            } => {
                if epoch == self.js_epoch {
                    self.pending_images = self.pending_images.saturating_sub(1);
                    if self.image_generation(element) == generation {
                        self.in_flight_images.remove(&element);
                        let selected = self.image_selected_src.remove(&element).unwrap_or_default();
                        self.world.borrow_mut().fail_image(element, selected);
                        self.fire_js(|js| js.fire_node_error(element));
                        self.adopt_js_work();
                    }
                    self.fire_document_load();
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
        }
        self.adopt_js_work();
    }

    pub(in crate::document) fn settle_js_fetch(
        &mut self,
        id: i32,
        ok: bool,
        status: i32,
        body: &str,
    ) {
        self.fire_js(|js| js.finish_js_fetch(id, ok, status, body));
    }

    fn sync_parser_from_world(&self) {
        let Some(parser) = self.active_parser.as_ref() else {
            return;
        };
        let mut world = self.world.borrow_mut();
        let pending = std::mem::take(&mut world.pending_html_writes);
        let bytes = pending.iter().map(String::len).sum::<usize>();
        world.release_stream_bytes(bytes);
        let writes = pending.concat();
        if let Some(parsed) = world.take_main_document() {
            parser.restore(parsed);
        }
        drop(world);
        if !writes.is_empty() {
            parser.insert_html(writes);
        }
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
        if self.world.borrow().main_ready_state() == ReadyState::Complete {
            return;
        }
        // Style sheets that are still loading hold the load event
        // (<https://html.spec.whatwg.org/multipage/links.html#link-type-stylesheet>).
        if self.pending_stylesheets > 0 || self.pending_images > 0 {
            return;
        }
        self.world
            .borrow_mut()
            .set_main_ready_state(ReadyState::Complete);
        self.fire_js(crate::js::JsRealm::fire_ready_state_change);
        self.maybe_fire_load();
    }

    /// Queues every `<link rel=stylesheet>` whose URL has not been requested
    /// yet. Sheets delay the load event, so this runs before the document end
    /// events.
    fn load_stylesheets(&mut self) {
        let links: Vec<(dom::NodeId, String)> = {
            let world = self.world.borrow();
            let Some(parsed) = world.main_document() else {
                return;
            };
            let document = parsed.dom.document();
            let Ok(links) = parsed.dom.select_all(document, "link[rel~=\"stylesheet\"]") else {
                return;
            };
            links
                .into_iter()
                .filter_map(|link| parsed.dom.attribute(link, "href").map(|href| (link, href)))
                .collect()
        };
        let initiator = self.url.clone();
        let mut queued = 0usize;
        for (element, href) in links {
            let Ok(url) = self.resolve_dial_url(&href) else {
                continue;
            };
            if !self.stylesheet_urls.insert(url.as_str().to_owned()) {
                continue;
            }
            self.queued_dials.push(QueuedDial::get(
                DialContext::Stylesheet {
                    element,
                    epoch: self.js_epoch,
                },
                url,
                initiator.clone(),
            ));
            self.pending_stylesheets = self.pending_stylesheets.saturating_add(1);
            queued += 1;
        }
        if queued > 0 {
            self.launch_queued_dials();
        }
    }

    /// Queues the current document's `<img src>` resources. Image requests
    /// delay the load event until they succeed or fail
    /// (<https://html.spec.whatwg.org/multipage/images.html#updating-the-image-data>).
    fn load_images(&mut self) {
        let images: Vec<dom::NodeId> = {
            let world = self.world.borrow();
            let Some(parsed) = world.main_document() else {
                return;
            };
            let document = parsed.dom.document();
            parsed
                .dom
                .select_all(document, "img[src]")
                .unwrap_or_default()
        };
        for element in images {
            self.queue_image(element, false);
        }
    }

    fn image_generation(&self, element: dom::NodeId) -> u64 {
        self.image_generations.get(&element).copied().unwrap_or(0)
    }

    fn bump_image_generation(&mut self, element: dom::NodeId) -> u64 {
        let generation = self.image_generations.entry(element).or_insert(0);
        *generation = generation.wrapping_add(1);
        *generation
    }

    /// Starts or replaces the fetch for one `<img>`.
    ///
    /// `force` is a `src` mutation: a connected insert skips work when a fetch
    /// or decoded image is already current.
    pub(in crate::document) fn queue_image(&mut self, element: dom::NodeId, force: bool) {
        if !force
            && (self.in_flight_images.contains(&element)
                || self.world.borrow().images.contains_key(&element))
        {
            return;
        }
        let generation = self.bump_image_generation(element);
        self.in_flight_images.insert(element);
        let src = self.world.borrow().document(element).and_then(|parsed| {
            parsed
                .dom
                .is_img_element(element)
                .then(|| parsed.dom.attribute(element, "src"))
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
        let initiator = self.url.clone();
        let selected = url.as_str().to_owned();
        self.image_selected_src.insert(element, selected.clone());
        self.world.borrow_mut().begin_image(element, selected);
        self.queued_dials.push(QueuedDial::get(
            DialContext::Image {
                element,
                epoch: self.js_epoch,
                generation,
            },
            url,
            initiator,
        ));
        self.pending_images = self.pending_images.saturating_add(1);
        self.launch_queued_dials();
    }

    /// The loaded CSS of one `<link rel=stylesheet>`, if it arrived.
    pub(crate) fn stylesheet_for(&self, element: dom::NodeId) -> Option<&str> {
        self.stylesheets.get(&element).map(String::as_str)
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
    pub(crate) fn mark_frame_load_pending(&mut self, container: dom::NodeId) {
        self.pending_frame_loads.insert(container);
    }

    /// The containers whose child frames have not finished loading.
    pub(crate) fn pending_frame_loads(&self) -> Vec<dom::NodeId> {
        self.pending_frame_loads.iter().copied().collect()
    }

    /// Fires one container's `load` event and lets this document's own `load`
    /// event proceed once no child is left loading.
    pub(crate) fn finish_frame_load(&mut self, container: dom::NodeId) -> bool {
        if !self.pending_frame_loads.remove(&container) {
            return false;
        }
        self.fire_node_load(container);
        self.maybe_fire_load();
        true
    }

    /// Drops a container that went away before its child finished loading.
    pub(crate) fn cancel_frame_load(&mut self, container: dom::NodeId) {
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
