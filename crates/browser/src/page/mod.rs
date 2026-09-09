//! One page: HTML jobs we own, Tokio current-thread as the waiter, fetch handle for HTTP.
//!
//! [ADR 0007](../../../../docs/adrs/0007-engine-charter.md): the page thread is
//! Tokio `rt`+`time` only. Blocking send runs on `spawn_blocking`. `Page` stays
//! one owner; this module is split by job ([ADR 0010](../../../../docs/adrs/0010-page-actor-ownership.md)).

use std::cell::{Ref, RefCell};
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::Waker;
use std::time::Instant as WallClock;

use net::{Agent, AgentBuilder};
use tokio::runtime::Runtime;
use tokio::task::JoinSet;
use tokio::time::Instant;
use url::Url;

use crate::Parsed;
use crate::js::{ClassicScript, World};
use crate::network::{FetchHandle, PAGE_FETCH_TIMEOUT};

mod intern;
mod navigate;
mod pump;

pub use crate::js::ScriptValue;

/// Upper bound on a host-fetch / navigation body read inside `spawn_blocking`.
const FETCH_BODY_LIMIT: usize = 1_048_576;

/// Caps concurrent `spawn_blocking` dials so one eval loop cannot exhaust threads.
const MAX_IN_FLIGHT_DIALS: usize = 16;

/// Why `QuickJS` eval or a host callback failed.
#[derive(Clone, Debug, PartialEq, Eq)]
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

/// Why a page API call was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PageError {
    /// The URL string could not be parsed, joined, or was not `http`/`https`
    /// for a dial.
    InvalidUrl {
        /// The spec the caller passed.
        spec: String,
    },
    /// `QuickJS` eval or a host callback failed.
    Script(ScriptFailure),
    /// The page actor thread has stopped.
    ActorStopped,
}

impl fmt::Display for PageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUrl { spec } => write!(f, "invalid url: {spec}"),
            Self::Script(failure) => write!(f, "script: {failure}"),
            Self::ActorStopped => f.write_str("page actor stopped"),
        }
    }
}

impl std::error::Error for PageError {}

impl From<crate::js::JsError> for PageError {
    fn from(err: crate::js::JsError) -> Self {
        match err {
            crate::js::JsError::Engine(message) => Self::Script(ScriptFailure::Engine { message }),
            crate::js::JsError::Interrupted => Self::Script(ScriptFailure::Interrupted),
            crate::js::JsError::BadTimerId => Self::Script(ScriptFailure::BadTimerId),
        }
    }
}

/// Observable HTML-job outcomes, in the order the page ran them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageEvent {
    /// A host timer whose delay elapsed.
    Timer(u32),
    /// A `fetch` or navigation job finished with this HTTP status.
    Fetch {
        /// Status from [`net::Response::status`].
        status: u16,
    },
    /// `send` or the body read failed.
    FetchFailed,
    /// A timer or `fetch` callback threw, or `execute_pending_job` failed.
    ScriptFailed,
}

enum HtmlJob {
    Timer(u32),
    DialFinished(CompletedDial),
    DialFailed(DialFail),
}

enum QueuedDial {
    Fetch {
        url: Url,
        initiator: Url,
    },
    Navigate {
        url: Url,
        initiator: Url,
        epoch: u64,
    },
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

enum CompletedDial {
    Fetch {
        status: u16,
    },
    Navigate {
        status: u16,
        body: Vec<u8>,
        final_url: Url,
        content_language: Option<String>,
        epoch: u64,
    },
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
enum DialFail {
    Fetch,
    Navigate { epoch: u64 },
    JsFetch { id: i32, epoch: u64 },
    ClassicScript { epoch: u64 },
}

struct HostTimer {
    id: u32,
    when: Instant,
    fired: bool,
}

/// One browsing context: tree, cookie jar via [`FetchHandle`], HTML job list.
pub struct Page {
    fetch: FetchHandle,
    world: Rc<RefCell<World>>,
    document_url: Url,
    content_language: Option<String>,
    jobs: VecDeque<HtmlJob>,
    timers: Vec<HostTimer>,
    next_timer_id: u32,
    fetches: JoinSet<Result<CompletedDial, DialFail>>,
    queued_dials: Vec<QueuedDial>,
    events: Vec<PageEvent>,
    js: Option<crate::js::JsHost>,
    js_timer_slots: HashMap<u32, i32>,
    nav_epoch: u64,
    js_epoch: u64,
    pending_classic: VecDeque<ClassicScript>,
    classic_fetch_in_flight: bool,
    nav_in_flight: Option<u64>,
    navigation_failed: bool,
    runtime: Option<Runtime>,
    stop: Arc<Stop>,
    next_remote: u64,
    remote_by_node: HashMap<dom::NodeId, u64>,
}

impl Default for Page {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        self.shutdown_runtime();
    }
}

impl Page {
    /// An empty page with a default [`Agent`] (30s per-call timeout) and
    /// document URL `about:blank`.
    #[must_use]
    pub fn new() -> Self {
        Self::from_builder(AgentBuilder::new())
    }

    /// Empty page using `builder` plus the default per-call fetch timeout.
    #[must_use]
    pub fn from_builder(builder: AgentBuilder) -> Self {
        Self::with_agent(builder.timeout_per_call(PAGE_FETCH_TIMEOUT).build())
    }

    /// A page that uses `agent` for every dial and for `document.cookie`.
    ///
    /// Share a jar by passing the same [`Agent`]. Dial from this page only
    /// through [`Page::goto`], [`Page::start_fetch`], or `fetch` in [`Page::eval`].
    ///
    /// # Panics
    ///
    /// Only if `about:blank` fails to parse, which is a URL-crate defect.
    #[must_use]
    pub fn with_agent(agent: Agent) -> Self {
        Self::with_fetch(FetchHandle::from_agent(agent))
    }

    pub(crate) fn with_fetch(fetch: FetchHandle) -> Self {
        Self::with_fetch_stop(fetch, Arc::new(Stop::new()))
    }

    pub(crate) fn with_fetch_stop(fetch: FetchHandle, stop: Arc<Stop>) -> Self {
        let document_url = Url::parse("about:blank").expect("about:blank is a valid URL");
        Self {
            world: Rc::new(RefCell::new(World::new(
                fetch.clone(),
                document_url.clone(),
            ))),
            fetch,
            document_url,
            content_language: None,
            jobs: VecDeque::new(),
            timers: Vec::new(),
            next_timer_id: 1,
            fetches: JoinSet::new(),
            queued_dials: Vec::new(),
            events: Vec::new(),
            js: None,
            js_timer_slots: HashMap::new(),
            nav_epoch: 0,
            js_epoch: 0,
            pending_classic: VecDeque::new(),
            classic_fetch_in_flight: false,
            nav_in_flight: None,
            navigation_failed: false,
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

    /// Document URL after navigation (cookie initiator and relative-URL base).
    #[must_use]
    pub fn document_url(&self) -> &str {
        self.document_url.as_str()
    }

    /// Document language: HTTP `Content-Language` after navigation, or the
    /// last [`Page::set_content_language`] value. Stored on [`dom::Dom`] once
    /// a tree exists.
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
        self.document_url =
            Url::parse(url).map_err(|_| PageError::InvalidUrl { spec: url.into() })?;
        self.world.borrow_mut().document_url = self.document_url.clone();
        Ok(())
    }

    /// `document.cookie` getter: non-HTTP jar read for this document URL.
    #[must_use]
    pub fn document_cookie(&self) -> String {
        self.fetch.cookies_for(&self.document_url)
    }

    /// `document.cookie` setter: non-HTTP jar write for this document URL.
    pub fn set_document_cookie(&self, value: &str) {
        self.fetch.set_cookie(value, &self.document_url);
    }

    /// Evaluates `source` as classic script on this page's `QuickJS` context.
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

    /// Evaluates `source` and returns a structured JS value for `WebDriver`.
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

    /// True when the last [`Page::goto`] dial failed (DNS, connect, or body read).
    #[must_use]
    pub fn last_navigation_failed(&self) -> bool {
        self.navigation_failed
    }

    fn ensure_js(&mut self) -> Result<(), PageError> {
        if self.js.is_none() {
            self.js = Some(crate::js::JsHost::new(self.world.clone()).map_err(PageError::from)?);
        }
        Ok(())
    }
}

fn note_script(events: &mut Vec<PageEvent>, failed: bool) {
    if failed {
        events.push(PageEvent::ScriptFailed);
    }
}

pub(crate) struct Stop {
    flag: AtomicBool,
    waker: Mutex<Option<Waker>>,
}

impl Stop {
    pub(crate) fn new() -> Self {
        Self {
            flag: AtomicBool::new(false),
            waker: Mutex::new(None),
        }
    }

    pub(crate) fn request(&self) {
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

    fn is_set(&self) -> bool {
        self.flag.load(Ordering::Relaxed)
    }

    fn register(&self, waker: &Waker) {
        *self
            .waker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(waker.clone());
    }
}
