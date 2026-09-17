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
use crate::ReadyState;
use crate::documents::DocumentStore;
use crate::js::{DocumentStreamCommand, FrameNavigation, RealmRegistry, SharedJsRuntime, World};
use crate::messaging::SharedHandle;
use crate::protocol::{BrowserServices, FrameId, Mount, ScriptFailure, TabError, TabEvent};

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
    DialFailed(DialFail),
    WindowMessage(WindowMessage),
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

#[derive(Clone)]
pub(crate) enum QueuedDial {
    JsFetch {
        url: Url,
        initiator: Url,
        id: i32,
        epoch: u64,
    },
    ClassicScript {
        url: Url,
        initiator: Url,
        element: dom::NodeId,
        epoch: u64,
    },
    /// A child frame's own navigation
    /// (<https://html.spec.whatwg.org/multipage/browsing-the-web.html#navigate>).
    FrameLoad {
        url: Url,
        initiator: Url,
        /// Identifies the frame's current navigation; a superseded load is
        /// dropped when it completes.
        sequence: u64,
    },
}

pub(crate) enum CompletedDial {
    JsFetch {
        status: u16,
        body: Vec<u8>,
        id: i32,
        epoch: u64,
    },
    ClassicScript {
        status: u16,
        body: Vec<u8>,
        element: dom::NodeId,
        epoch: u64,
    },
    FrameLoad {
        body: Vec<u8>,
        content_type: Option<String>,
        content_language: Option<String>,
        final_url: String,
        sequence: u64,
    },
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

#[derive(Clone, Copy)]
pub(crate) enum DialFail {
    JsFetch { id: i32, epoch: u64 },
    ClassicScript { epoch: u64 },
    FrameLoad { sequence: u64 },
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
    dial_tx: Sender<Result<CompletedDial, DialFail>>,
    dial_rx: Receiver<Result<CompletedDial, DialFail>>,
    in_flight_dials: usize,
    queued_dials: Vec<QueuedDial>,
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
    /// Child browsing contexts whose load event has not fired yet; this
    /// document's own `load` event is delayed until they all have
    /// (<https://html.spec.whatwg.org/multipage/parsing.html#delay-the-load-event>).
    pending_child_loads: u32,
    /// The `iframe` containers awaiting their child's load event.
    pending_frame_loads: HashSet<dom::NodeId>,
    /// Whether this document's `load` event has fired.
    load_fired: bool,
    events: Vec<TabEvent>,
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
            frame_load_sequence: 0,
            frame_load_in_flight: false,
            initial_blank: true,
            frame_order_serial: u64::MAX,
            pending_child_loads: 0,
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
        if let Some(js) = &self.js
            && js.flush_frame_sets(child.get()).is_err()
        {
            self.record_event(TabEvent::ScriptFailed);
        }
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
        if let Some(js) = &self.js
            && js.fire_node_load(id).is_err()
        {
            self.record_event(TabEvent::ScriptFailed);
        }
        self.adopt_js_work();
    }

    /// Starts the frame's own navigation. The dial runs on this document, so
    /// a navigation that replaces the frame cancels an unfinished one.
    pub(crate) fn navigate_to(&mut self, url: Url, initiator: Url) {
        self.frame_load_sequence = self.frame_load_sequence.wrapping_add(1);
        self.frame_load_in_flight = true;
        self.initial_blank = false;
        self.queued_dials.push(QueuedDial::FrameLoad {
            url,
            initiator,
            sequence: self.frame_load_sequence,
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
        self.ensure_frame_js();
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
        self.begin_response(Some(url), content_type, content_language);
        self.write_body(body);
        self.end_body();
        self.ensure_frame_js();
    }

    /// Every frame has a window; make sure the realm exists even when the
    /// document never runs a script, so a parent can set properties on it
    /// (<https://html.spec.whatwg.org/multipage/window-object.html#the-window-object>).
    fn ensure_frame_js(&mut self) {
        if self.ensure_js().is_err() {
            self.record_event(TabEvent::ScriptFailed);
        }
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
            self.record_event(TabEvent::ScriptFailed);
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

    fn deliver_window_message(&mut self, message: &WindowMessage) {
        if self.ensure_js().is_err() {
            self.record_event(TabEvent::ScriptFailed);
            return;
        }
        let Some(js) = &self.js else {
            return;
        };
        let delivered = js.deliver_window_message(
            message.source.get(),
            &message.origin,
            &message.payload,
            &message.ports,
        );
        match delivered {
            Ok(true) => {}
            Ok(false) => {
                if js
                    .deliver_window_message_error(message.source.get(), &message.origin)
                    .is_err()
                {
                    self.record_event(TabEvent::ScriptFailed);
                }
            }
            Err(_) => self.record_event(TabEvent::ScriptFailed),
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
        if self.ensure_js().is_err() {
            self.record_event(TabEvent::ScriptFailed);
            return;
        }
        let Some(js) = &self.js else {
            return;
        };
        match js.deliver_port_message(endpoint, payload, ports) {
            Ok(true) => {}
            Ok(false) => {
                if js.deliver_port_message_error(endpoint).is_err() {
                    self.record_event(TabEvent::ScriptFailed);
                }
            }
            Err(_) => self.record_event(TabEvent::ScriptFailed),
        }
        self.adopt_js_work();
    }

    fn deliver_port_close(&mut self, endpoint: u64) {
        if self.endpoint_moved(endpoint) {
            let shared = self.shared.clone();
            shared.borrow_mut().requeue_port_close(endpoint);
            return;
        }
        if self.ensure_js().is_err() {
            self.record_event(TabEvent::ScriptFailed);
            return;
        }
        if let Some(js) = &self.js
            && js.deliver_port_close(endpoint).is_err()
        {
            self.record_event(TabEvent::ScriptFailed);
        }
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

    pub(crate) fn take_events(&mut self) -> Result<Vec<TabEvent>, ()> {
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
        self.begin_response(
            Some(&url),
            mount.content_type.as_deref(),
            mount.content_language.as_deref(),
        );
        self.write_body(&mount.body);
        self.end_body();
        Ok(())
    }

    /// Opens a document body that will arrive in pieces.
    ///
    /// The realm is already new when this runs; this opens the parser and,
    /// when a response feeds it, the decoder for that response.
    fn open_parser(&mut self, content_type: Option<&str>, owner: ParserOwner) {
        self.world.borrow_mut().parser_active = true;
        self.parser_eof = false;
        // A script writes text, not bytes, and it takes the input stream away
        // from any live response: dropping the decoder ignores the rest of
        // that body instead of decoding it with the wrong charset.
        self.decoder = match owner {
            ParserOwner::Carrier => {
                Some(dial::ResponseDecoder::new(content_type.map(str::to_owned)))
            }
            ParserOwner::Script => None,
        };
        self.parser_owner = owner;
        self.active_parser = Some(ActiveParser::new(""));
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
        self.open_parser(content_type, ParserOwner::Carrier);
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
        self.open_parser(None, ParserOwner::Script);
    }

    /// Parses `input` into this document and starts a new JS realm.
    ///
    /// The caller already holds the whole document, so the parser starts at
    /// EOF with the markup in hand.
    pub(crate) fn load_html(&mut self, input: &str) {
        self.reset_js_realm();
        self.world.borrow_mut().parser_active = true;
        self.parser_eof = true;
        // The markup is in hand, so no decoder is involved, but the carrier
        // owns this parser: `document.close()` must not end it.
        self.decoder = None;
        self.parser_owner = ParserOwner::Carrier;
        self.active_parser = Some(ActiveParser::new(input));
        self.advance_parser();
    }

    /// Records `document` as this realm's so wrappers resolve its owner.
    fn register_document(&self, document: u32) {
        let registry = self.world.borrow().registry();
        registry.borrow_mut().insert_document(document, &self.world);
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
        self.pending_child_loads = 0;
        self.pending_frame_loads.clear();
        // Any frame load that completes after this point belongs to the
        // replaced document, even when it was started through a synchronous
        // path that never bumped the sequence
        // (<https://html.spec.whatwg.org/multipage/browsing-the-web.html#navigate>).
        self.frame_load_sequence = self.frame_load_sequence.wrapping_add(1);
        self.js_epoch = self.js_epoch.saturating_add(1);
        // Every queued dial belongs to the old realm; drop them all.
        self.queued_dials.clear();
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

    fn eval_classic(&mut self, source: &str, element: Option<dom::NodeId>) {
        // https://html.spec.whatwg.org/multipage/webappapis.html#run-a-classic-script
        let previous = self.world.borrow().current_script;
        self.world.borrow_mut().current_script = element;
        if let Some(js) = &self.js
            && js.eval(source).is_err()
        {
            self.record_event(TabEvent::ScriptFailed);
        }
        self.world.borrow_mut().current_script = previous;
        self.adopt_js_work();
    }

    /// Drains parser-driven mutation records at a microtask checkpoint
    /// (<https://html.spec.whatwg.org/multipage/webappapis.html#perform-a-microtask-checkpoint>).
    /// Parser insertions bypass the JS bindings that schedule delivery, so
    /// the document's `MutationObserver`s would otherwise not fire until the
    /// next scripted mutation.
    fn deliver_mutations(&mut self) {
        if let Some(js) = &self.js
            && js.deliver_mutations().is_err()
        {
            self.record_event(TabEvent::ScriptFailed);
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
                    if self.js.is_none() {
                        let document = self.world.borrow_mut().replace_document(parsed);
                        self.register_document(document);
                        if self.ensure_js().is_err() {
                            self.record_event(TabEvent::ScriptFailed);
                            self.sync_parser_from_world();
                            continue;
                        }
                    } else {
                        let document = self.world.borrow_mut().set_document(parsed);
                        self.register_document(document);
                    }
                    if let Some(mut parsed) = self.world.borrow().main_document_mut() {
                        parsed
                            .dom
                            .set_document_language(self.content_language.clone());
                    }
                    // A script may query a child frame's window; the browsing
                    // context must exist by then.
                    self.adopt_pending_frames();
                    // Microtask checkpoint before the script runs; parser
                    // mutations queued since the last script deliver now.
                    self.deliver_mutations();
                    let script = crate::js::classic_script_at(&self.world.borrow(), id);
                    match script {
                        Some(crate::js::ClassicScript::Inline(source)) => {
                            self.eval_classic(&source, Some(id));
                            self.sync_parser_from_world();
                        }
                        Some(crate::js::ClassicScript::Src(src)) => {
                            if let Ok(url) = self.resolve_dial_url(&src) {
                                self.classic_fetch_in_flight = true;
                                let initiator = self.url.clone();
                                self.queued_dials.push(QueuedDial::ClassicScript {
                                    url,
                                    initiator,
                                    element: id,
                                    epoch: self.js_epoch,
                                });
                                return;
                            }
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
                    let mut parsed = parser.finish();
                    parsed
                        .dom
                        .set_document_language(self.content_language.clone());
                    if self.js.is_none() {
                        let document = self.world.borrow_mut().replace_document(parsed);
                        self.register_document(document);
                        if self.ensure_js().is_err() {
                            self.record_event(TabEvent::ScriptFailed);
                            return;
                        }
                    } else {
                        let document = self.world.borrow_mut().set_document(parsed);
                        self.register_document(document);
                    }
                    self.world.borrow_mut().parser_active = false;
                    // Deliver parser mutations before the document's events.
                    self.deliver_mutations();
                    self.fire_document_end();
                    return;
                }
            }
        }
    }

    pub(in crate::document) fn resolve_dial_url(&self, spec: &str) -> Result<Url, TabError> {
        let url = Url::parse(spec)
            .or_else(|_| self.base_url().join(spec))
            .map_err(|_| TabError::InvalidUrl { spec: spec.into() })?;
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
        match done {
            CompletedDial::JsFetch {
                status,
                body,
                id,
                epoch,
            } => {
                self.record_event(TabEvent::Fetch { status });
                if epoch == self.js_epoch {
                    let body = String::from_utf8_lossy(&body);
                    self.settle_js_fetch(id, true, i32::from(status), &body);
                }
            }
            CompletedDial::ClassicScript {
                status,
                body,
                element,
                epoch,
            } => {
                self.record_event(TabEvent::Fetch { status });
                if epoch == self.js_epoch {
                    self.classic_fetch_in_flight = false;
                    if (200..300).contains(&status) {
                        let source = String::from_utf8_lossy(&body);
                        self.eval_classic(&source, Some(element));
                    }
                    self.sync_parser_from_world();
                    self.advance_parser();
                }
            }
            CompletedDial::FrameLoad {
                body,
                content_type,
                content_language,
                final_url,
                sequence,
            } => {
                if sequence != self.frame_load_sequence {
                    return;
                }
                self.frame_load_in_flight = false;
                let Ok(url) = Url::parse(&final_url) else {
                    return;
                };
                self.load_frame_response(
                    &url,
                    content_type.as_deref(),
                    content_language.as_deref(),
                    &body,
                );
            }
        }
        self.adopt_js_work();
    }

    pub(in crate::document) fn fail_dial(&mut self, fail: DialFail) {
        self.record_event(TabEvent::FetchFailed);
        match fail {
            DialFail::JsFetch { id, epoch } => {
                if epoch == self.js_epoch {
                    self.settle_js_fetch(id, false, 0, "");
                }
            }
            DialFail::ClassicScript { epoch } => {
                if epoch == self.js_epoch {
                    self.classic_fetch_in_flight = false;
                    self.sync_parser_from_world();
                    self.advance_parser();
                }
            }
            DialFail::FrameLoad { sequence } => {
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
        if let Some(js) = &self.js
            && js.finish_js_fetch(id, ok, status, body).is_err()
        {
            self.record_event(TabEvent::ScriptFailed);
        }
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
        if let Some(js) = &self.js
            && js.fire_ready_state_change().is_err()
        {
            self.record_event(TabEvent::ScriptFailed);
        }
        if let Some(js) = &self.js
            && js.fire_dom_content_loaded().is_err()
        {
            self.record_event(TabEvent::ScriptFailed);
        }
        self.adopt_js_work();
        // A script may have appended an iframe after the parser finished; its
        // load must delay this document's own load event
        // (<https://html.spec.whatwg.org/multipage/parsing.html#delay-the-load-event>).
        self.adopt_pending_frames();
        self.fire_document_load();
    }

    fn fire_document_load(&mut self) {
        if self.world.borrow().main_ready_state() == ReadyState::Complete {
            return;
        }
        self.world
            .borrow_mut()
            .set_main_ready_state(ReadyState::Complete);
        if let Some(js) = &self.js
            && js.fire_ready_state_change().is_err()
        {
            self.record_event(TabEvent::ScriptFailed);
        }
        self.maybe_fire_load();
    }

    /// Fires this document's `load` event once it and every child browsing
    /// context have finished loading
    /// (<https://html.spec.whatwg.org/multipage/parsing.html#delay-the-load-event>).
    pub(crate) fn maybe_fire_load(&mut self) {
        if self.load_fired
            || self.pending_child_loads > 0
            || self.world.borrow().main_ready_state() != ReadyState::Complete
        {
            return;
        }
        self.load_fired = true;
        self.record_event(TabEvent::Load);
        if let Some(js) = &self.js
            && js.fire_load().is_err()
        {
            self.record_event(TabEvent::ScriptFailed);
        }
        self.adopt_js_work();
    }

    /// A child browsing context started loading; this document's `load` event
    /// waits for it, and the container is remembered until it finishes.
    pub(crate) fn mark_frame_load_pending(&mut self, container: dom::NodeId) {
        if self.pending_frame_loads.insert(container) {
            self.pending_child_loads = self.pending_child_loads.saturating_add(1);
        }
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
        self.pending_child_loads = self.pending_child_loads.saturating_sub(1);
        self.fire_node_load(container);
        self.maybe_fire_load();
        true
    }

    /// Drops a container that went away before its child finished loading.
    pub(crate) fn cancel_frame_load(&mut self, container: dom::NodeId) {
        if self.pending_frame_loads.remove(&container) {
            self.pending_child_loads = self.pending_child_loads.saturating_sub(1);
            self.maybe_fire_load();
        }
    }

    pub(in crate::document) fn record_event(&mut self, event: TabEvent) {
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
