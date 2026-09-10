//! One document: HTML jobs we own, Tokio current-thread as the waiter, host
//! services for dials and cookies. The renderer owns the document; the host
//! owns the tab and drives navigation
//! ([ADR 0011](../../../../docs/adrs/0011-renderer-processes-per-site.md)).

use std::cell::{Ref, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TrySendError};
use std::task::Waker;
use std::time::Instant as WallClock;

use tokio::runtime::Runtime;
use tokio::time::Instant;
use url::Url;

use crate::js::World;
use crate::protocol::{HostServices, Mount, PageError, PageEvent, ScriptFailure};
use crate::{ActiveParser, Parsed};

mod dial;
mod intern;
mod pump;

pub use crate::js::ScriptValue;

/// Upper bound on a host-fetch or document body.
pub(crate) const FETCH_BODY_LIMIT: usize = 1_048_576;

const MAX_QUEUED_JS_FETCHES: usize = 256;

enum HtmlJob {
    Timer(u32),
    DialFinished(CompletedDial),
    DialFailed(DialFail),
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
        epoch: u64,
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
        epoch: u64,
    },
}

#[derive(Clone, Copy)]
pub(crate) enum DialFail {
    JsFetch { id: i32, epoch: u64 },
    ClassicScript { epoch: u64 },
}

struct HostTimer {
    id: u32,
    when: Instant,
    fired: bool,
}

/// One document: tree, HTML job list, `QuickJS` realm, and host services.
pub struct Document {
    services: Arc<dyn HostServices>,
    world: Rc<RefCell<World>>,
    url: Url,
    content_language: Option<String>,
    jobs: VecDeque<HtmlJob>,
    timers: Vec<HostTimer>,
    next_timer_id: u32,
    dial_tx: Sender<Result<CompletedDial, DialFail>>,
    dial_rx: Receiver<Result<CompletedDial, DialFail>>,
    dial_pool: DialPool,
    in_flight_dials: usize,
    queued_dials: Vec<QueuedDial>,
    events: Vec<PageEvent>,
    js: Option<crate::js::JsHost>,
    js_timer_slots: HashMap<u32, i32>,
    js_epoch: u64,
    active_parser: Option<ActiveParser>,
    classic_fetch_in_flight: bool,
    runtime: Option<Runtime>,
    stop: Arc<Stop>,
    next_remote: u64,
    remote_by_node: HashMap<dom::NodeId, u64>,
}

impl Drop for Document {
    fn drop(&mut self) {
        self.shutdown_runtime();
    }
}

impl Document {
    /// An empty document whose dials and cookies go through `services`.
    #[must_use]
    pub fn new(services: Arc<dyn HostServices>) -> Self {
        Self::with_stop(services, Arc::new(Stop::new()))
    }

    pub(crate) fn with_stop(services: Arc<dyn HostServices>, stop: Arc<Stop>) -> Self {
        let document_url = Url::parse("about:blank").expect("about:blank is a valid URL");
        let (dial_tx, dial_rx) = mpsc::channel();
        Self {
            world: Rc::new(RefCell::new(World::new(
                Arc::clone(&services),
                document_url.clone(),
            ))),
            services,
            url: document_url,
            content_language: None,
            jobs: VecDeque::new(),
            timers: Vec::new(),
            next_timer_id: 1,
            dial_tx,
            dial_rx,
            dial_pool: DialPool::new(),
            in_flight_dials: 0,
            queued_dials: Vec::new(),
            events: Vec::new(),
            js: None,
            js_timer_slots: HashMap::new(),
            js_epoch: 0,
            active_parser: None,
            classic_fetch_in_flight: false,
            runtime: None,
            stop,
            next_remote: 0,
            remote_by_node: HashMap::new(),
        }
    }

    /// Last parse result, if any.
    #[must_use]
    pub fn parsed(&self) -> Option<Ref<'_, Parsed>> {
        Ref::filter_map(self.world.borrow(), |world| world.parsed.as_ref()).ok()
    }

    /// Document URL (cookie initiator and relative-URL base).
    #[must_use]
    pub fn document_url(&self) -> &str {
        self.url.as_str()
    }

    /// Document language: mount `Content-Language`, or the last
    /// [`Document::set_content_language`] value.
    #[must_use]
    pub fn content_language(&self) -> Option<&str> {
        self.content_language.as_deref()
    }

    /// Records the document-level `Content-Language` default.
    pub fn set_content_language(&mut self, value: Option<String>) {
        self.content_language.clone_from(&value);
        if let Some(parsed) = self.world.borrow_mut().parsed.as_mut() {
            parsed.dom.set_document_language(value);
        }
    }

    /// Sets the document URL used as cookie initiator and relative-URL base.
    ///
    /// # Errors
    ///
    /// [`PageError::InvalidUrl`] when `url` is not an absolute URL.
    pub fn set_document_url(&mut self, url: &str) -> Result<(), PageError> {
        self.url = Url::parse(url).map_err(|_| PageError::InvalidUrl { spec: url.into() })?;
        self.world.borrow_mut().document_url = self.url.clone();
        Ok(())
    }

    /// `document.cookie` getter: non-HTTP jar read for this document URL.
    #[must_use]
    pub fn document_cookie(&self) -> String {
        self.services.cookies_for(&self.url)
    }

    /// `document.cookie` setter: non-HTTP jar write for this document URL.
    pub fn set_document_cookie(&self, value: &str) {
        self.services.set_cookie(value, &self.url);
    }

    /// Evaluates `source` as classic script on this document's JS context.
    ///
    /// # Errors
    ///
    /// [`PageError::Script`] when the engine cannot start or the script throws.
    pub fn eval(&mut self, source: &str) -> Result<String, PageError> {
        self.ensure_js()?;
        let Some(js) = self.js.as_ref() else {
            return Err(PageError::Script(ScriptFailure::HostMissing));
        };
        let out = js.eval(source).map_err(PageError::from);
        self.adopt_js_work();
        out
    }

    /// Evaluates `source` and returns a structured JS value.
    ///
    /// # Errors
    ///
    /// [`PageError::Script`] when the engine cannot start or the script throws.
    pub fn execute_script(&mut self, source: &str) -> Result<ScriptValue, PageError> {
        self.execute_script_deadline(source, None)
    }

    pub(crate) fn execute_script_deadline(
        &mut self,
        source: &str,
        deadline: Option<WallClock>,
    ) -> Result<ScriptValue, PageError> {
        self.ensure_js()?;
        let Some(js) = self.js.as_ref() else {
            return Err(PageError::Script(ScriptFailure::HostMissing));
        };
        let out = js
            .eval_value_deadline(source, deadline)
            .map_err(PageError::from);
        self.adopt_js_work();
        out
    }

    /// Jobs that have already run, in order.
    #[must_use]
    pub fn events(&self) -> &[PageEvent] {
        &self.events
    }

    /// Replaces the document from a host mount: new realm, decoded bytes,
    /// parsed to load. The host has already dialed and chosen this renderer.
    pub fn mount(&mut self, mount: &Mount) {
        self.reset_js_realm();
        let html = dial::decode_html(&mount.body, mount.content_type.as_deref());
        if let Ok(url) = Url::parse(&mount.url) {
            self.url = url;
        }
        self.content_language.clone_from(&mount.content_language);
        let mut world = self.world.borrow_mut();
        world.document_url = self.url.clone();
        drop(world);
        self.start_document(&html);
    }

    /// Parses `input` into this document and starts a new JS realm.
    pub fn load_html(&mut self, input: &str) {
        self.reset_js_realm();
        self.start_document(input);
    }

    fn ensure_js(&mut self) -> Result<(), PageError> {
        if self.js.is_none() {
            self.js = Some(
                crate::js::JsHost::new(self.world.clone(), Arc::clone(&self.stop))
                    .map_err(PageError::from)?,
            );
        }
        Ok(())
    }

    fn reset_js_realm(&mut self) {
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
        self.world.borrow_mut().parser_active = false;
        self.world.borrow_mut().pending_html_writes.clear();
        self.classic_fetch_in_flight = false;
        self.remote_by_node.clear();
    }

    fn start_document(&mut self, html: &str) {
        self.world.borrow_mut().parser_active = true;
        self.active_parser = Some(ActiveParser::new(html));
        self.advance_parser();
    }

    fn eval_classic(&mut self, source: &str) {
        if let Some(js) = &self.js {
            note_script(&mut self.events, js.eval(source).is_err());
        }
        self.adopt_js_work();
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
                        self.world.borrow_mut().replace_document(parsed);
                        if self.ensure_js().is_err() {
                            self.events.push(PageEvent::ScriptFailed);
                            self.sync_parser_from_world();
                            continue;
                        }
                    } else {
                        self.world.borrow_mut().parsed = Some(parsed);
                    }
                    if let Some(parsed) = self.world.borrow_mut().parsed.as_mut() {
                        parsed
                            .dom
                            .set_document_language(self.content_language.clone());
                    }
                    let script = crate::js::classic_script_at(&self.world.borrow(), id);
                    match script {
                        Some(crate::js::ClassicScript::Inline(source)) => {
                            self.eval_classic(&source);
                            self.sync_parser_from_world();
                        }
                        Some(crate::js::ClassicScript::Src(src)) => {
                            if let Ok(url) = self.resolve_dial_url(&src) {
                                self.classic_fetch_in_flight = true;
                                let initiator = self.url.clone();
                                self.queued_dials.push(QueuedDial::ClassicScript {
                                    url,
                                    initiator,
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
                    let Some(parser) = self.active_parser.take() else {
                        return;
                    };
                    let mut parsed = parser.finish();
                    parsed
                        .dom
                        .set_document_language(self.content_language.clone());
                    if self.js.is_none() {
                        self.world.borrow_mut().replace_document(parsed);
                        if self.ensure_js().is_err() {
                            self.events.push(PageEvent::ScriptFailed);
                            return;
                        }
                    } else {
                        self.world.borrow_mut().parsed = Some(parsed);
                    }
                    self.world.borrow_mut().parser_active = false;
                    self.fire_document_load();
                    return;
                }
            }
        }
    }

    pub(in crate::document) fn resolve_dial_url(&self, spec: &str) -> Result<Url, PageError> {
        let url = Url::parse(spec)
            .or_else(|_| self.base_url().join(spec))
            .map_err(|_| PageError::InvalidUrl { spec: spec.into() })?;
        if url.scheme() != "http" && url.scheme() != "https" {
            return Err(PageError::InvalidUrl { spec: spec.into() });
        }
        Ok(url)
    }

    fn base_url(&self) -> Url {
        let world = self.world.borrow();
        let Some(parsed) = world.parsed.as_ref() else {
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
                self.events.push(PageEvent::Fetch { status });
                if epoch == self.js_epoch {
                    let body = String::from_utf8_lossy(&body);
                    self.settle_js_fetch(id, true, i32::from(status), &body);
                }
            }
            CompletedDial::ClassicScript {
                status,
                body,
                epoch,
            } => {
                self.events.push(PageEvent::Fetch { status });
                if epoch == self.js_epoch {
                    self.classic_fetch_in_flight = false;
                    if (200..300).contains(&status) {
                        let source = String::from_utf8_lossy(&body);
                        self.eval_classic(&source);
                    }
                    self.sync_parser_from_world();
                    self.advance_parser();
                }
            }
        }
        self.adopt_js_work();
    }

    pub(in crate::document) fn fail_dial(&mut self, fail: DialFail) {
        self.events.push(PageEvent::FetchFailed);
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
        if let Some(js) = &self.js {
            note_script(
                &mut self.events,
                js.finish_js_fetch(id, ok, status, body).is_err(),
            );
        }
    }

    fn sync_parser_from_world(&self) {
        let Some(parser) = self.active_parser.as_ref() else {
            return;
        };
        let mut world = self.world.borrow_mut();
        let writes = std::mem::take(&mut world.pending_html_writes).concat();
        if let Some(parsed) = world.parsed.take() {
            parser.restore(parsed);
        }
        drop(world);
        if !writes.is_empty() {
            parser.insert_html(writes);
        }
    }

    fn fire_document_load(&mut self) {
        if self.world.borrow().document_ready {
            return;
        }
        self.world.borrow_mut().document_ready = true;
        self.events.push(PageEvent::Load);
        if let Some(js) = &self.js {
            note_script(&mut self.events, js.fire_load().is_err());
        }
        self.adopt_js_work();
    }
}

pub(crate) fn note_script(events: &mut Vec<PageEvent>, failed: bool) {
    if failed {
        events.push(PageEvent::ScriptFailed);
    }
}

impl From<crate::js::JsError> for PageError {
    fn from(err: crate::js::JsError) -> Self {
        match err {
            crate::js::JsError::Engine(message) => Self::Script(ScriptFailure::Engine { message }),
            crate::js::JsError::Interrupted => Self::Script(ScriptFailure::Interrupted),
            crate::js::JsError::BadTimerId => Self::Script(ScriptFailure::BadTimerId),
        }
    }
}

type DialJob = Box<dyn FnOnce() + Send + 'static>;

/// Bounded workers for blocking [`HostServices::dial`] calls.
struct DialPool {
    tx: SyncSender<DialJob>,
}

impl DialPool {
    fn new() -> Self {
        const WORKERS: usize = 16;
        const QUEUE: usize = 256;
        let (tx, rx) = mpsc::sync_channel::<DialJob>(QUEUE);
        let receiver = Arc::new(Mutex::new(rx));
        for _ in 0..WORKERS {
            let receiver = Arc::clone(&receiver);
            std::thread::spawn(move || {
                loop {
                    let job = receiver
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .recv();
                    let Ok(job) = job else {
                        return;
                    };
                    job();
                }
            });
        }
        Self { tx }
    }

    fn try_submit(&self, job: impl FnOnce() + Send + 'static) -> Result<(), ()> {
        match self.tx.try_send(Box::new(job)) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => Err(()),
        }
    }
}

/// Page-stop flag shared by the renderer loop, JS interrupt handler, and dials.
pub struct Stop {
    flag: AtomicBool,
    waker: Mutex<Option<Waker>>,
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
            waker: Mutex::new(None),
        }
    }

    /// Requests a stop: interrupts `QuickJS` and wakes the pump.
    pub fn request(&self) {
        self.flag.store(true, Ordering::Relaxed);
        if let Some(waker) = self
            .waker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            waker.wake();
        }
    }

    /// Whether [`Stop::request`] has run.
    #[must_use]
    pub fn is_set(&self) -> bool {
        self.flag.load(Ordering::Relaxed)
    }

    fn register(&self, waker: &Waker) {
        *self
            .waker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(waker.clone());
    }
}
