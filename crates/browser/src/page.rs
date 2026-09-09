//! One page: HTML jobs we own, Tokio current-thread as the waiter, fetch handle for HTTP.
//!
//! [ADR 0007](../../../docs/adrs/0007-engine-charter.md): the page thread is
//! Tokio `rt`+`time` only. Blocking send runs on `spawn_blocking`.

use std::cell::{Ref, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::future::{Future, pending, poll_fn};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Poll, Waker};
use std::time::{Duration, Instant as WallClock};

use net::{Agent, AgentBuilder, Context, Method};
use tokio::runtime::Runtime;
use tokio::task::JoinSet;
use tokio::time::{Instant, sleep_until};
use url::Url;

use crate::js::{ClassicScript, World, collect_classic_scripts};
use crate::network::{FetchHandle, PAGE_FETCH_TIMEOUT};
use crate::{Parsed, parse_html};

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

    /// Parses `input` into this page's tree and starts a new JS realm.
    pub fn load_html(&mut self, input: &str) {
        self.nav_epoch = self.nav_epoch.saturating_add(1);
        self.queued_dials
            .retain(|dial| !matches!(dial, QueuedDial::Navigate { .. }));
        self.reset_js_realm();
        {
            let mut world = self.world.borrow_mut();
            world.replace_document(parse_html(input));
            if let Some(parsed) = world.parsed.as_mut() {
                parsed
                    .dom
                    .set_document_language(self.content_language.clone());
            }
        }
        self.boot_document();
        self.fetch.persist();
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

    /// HTML host timer: fire [`PageEvent::Timer`] after `delay`.
    #[must_use]
    pub fn schedule_timer(&mut self, delay: Duration) -> u32 {
        let id = self.next_timer_id;
        self.next_timer_id = self.next_timer_id.saturating_add(1);
        self.timers.push(HostTimer {
            id,
            when: Instant::now() + delay,
            fired: false,
        });
        id
    }

    /// Queues a GET `fetch` job. [`Page::run`] performs the send.
    /// Relative URLs resolve against `<base href>` or the document URL.
    ///
    /// # Errors
    ///
    /// [`PageError::InvalidUrl`] when `url` cannot be parsed or joined, or
    /// is not `http`/`https`.
    pub fn start_fetch(&mut self, url: &str) -> Result<(), PageError> {
        let url = self.resolve_dial_url(url)?;
        let initiator = self.document_url.clone();
        self.queued_dials.push(QueuedDial::Fetch { url, initiator });
        Ok(())
    }

    /// Queues a navigation GET with [`Context::Navigation`]. [`Page::run`]
    /// performs the send, parses the body as HTML, stores `Content-Language`,
    /// and starts a new JS realm.
    ///
    /// # Errors
    ///
    /// [`PageError::InvalidUrl`] when `url` cannot be parsed or joined, or
    /// is not `http`/`https`.
    pub fn goto(&mut self, url: &str) -> Result<(), PageError> {
        let url = self.resolve_dial_url(url)?;
        self.nav_epoch = self.nav_epoch.saturating_add(1);
        self.navigation_failed = false;
        self.nav_in_flight = None;
        let epoch = self.nav_epoch;
        let initiator = self.document_url.clone();
        self.queued_dials
            .retain(|dial| !matches!(dial, QueuedDial::Navigate { .. }));
        self.queued_dials.push(QueuedDial::Navigate {
            url,
            initiator,
            epoch,
        });
        Ok(())
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

    pub(crate) fn execute_remote(
        &mut self,
        source: &str,
        timeout: Option<Duration>,
    ) -> Result<crate::RemoteValue, PageError> {
        let deadline = timeout.map(|duration| WallClock::now() + duration);
        let value = self.execute_script_deadline(source, deadline)?;
        Ok(self.intern_script(value))
    }

    fn intern_script(&mut self, value: ScriptValue) -> crate::RemoteValue {
        match value {
            ScriptValue::Undefined => crate::RemoteValue::Undefined,
            ScriptValue::Null => crate::RemoteValue::Null,
            ScriptValue::Bool(flag) => crate::RemoteValue::Bool(flag),
            ScriptValue::Number(number) => crate::RemoteValue::Number(number),
            ScriptValue::String(text) => crate::RemoteValue::String(text),
            ScriptValue::List(items) => crate::RemoteValue::List(
                items
                    .into_iter()
                    .map(|item| self.intern_script(item))
                    .collect(),
            ),
            ScriptValue::Map(entries) => crate::RemoteValue::Map(
                entries
                    .into_iter()
                    .map(|(key, item)| (key, self.intern_script(item)))
                    .collect(),
            ),
            ScriptValue::Node(_) => {
                self.next_remote = self.next_remote.saturating_add(1);
                crate::RemoteValue::Node(self.next_remote)
            }
        }
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

    /// Parks this thread as the page Tokio waiter until no jobs, timers,
    /// queued dials, or in-flight fetches remain. Must not run inside another
    /// runtime.
    ///
    /// # Panics
    ///
    /// If called from inside a Tokio runtime, if the current-thread runtime
    /// cannot be built, or a `spawn_blocking` fetch worker panics.
    pub fn run(&mut self) {
        self.block_on_pump(None, |page| !page.stopped());
    }

    /// Parks until the current navigation has fired `load`, without waiting
    /// for leftover host timers. `WebDriver` page-load strategy `normal`.
    ///
    /// [WebDriver navigate to URL](https://w3c.github.io/webdriver/#navigate-to)
    ///
    /// # Panics
    ///
    /// Same conditions as [`Page::run`].
    pub fn run_until_load(&mut self) {
        self.block_on_pump(None, |page| page.waiting_for_load() && !page.stopped());
    }

    /// Parks like [`Page::run_until_load`], returning `false` if `timeout` elapses first.
    ///
    /// # Panics
    ///
    /// Same conditions as [`Page::run`].
    pub fn run_until_load_timeout(&mut self, timeout: Duration) -> bool {
        self.run_until_timeout(timeout, |page| !page.waiting_for_load())
    }

    pub(crate) fn run_until_js_true(&mut self, source: &str, timeout: Duration) -> bool {
        self.run_until_timeout(timeout, |page| {
            matches!(page.execute_script(source), Ok(ScriptValue::Bool(true)))
        })
    }

    /// Parks like [`Page::run`], but returns as soon as `stop` is true.
    ///
    /// # Panics
    ///
    /// Same conditions as [`Page::run`].
    pub fn run_until(&mut self, mut stop: impl FnMut(&mut Self) -> bool) {
        self.block_on_pump(None, |page| !stop(page) && !page.stopped());
    }

    /// Parks like [`Page::run_until`], returning `false` if `timeout` elapses first.
    ///
    /// # Panics
    ///
    /// Same conditions as [`Page::run`].
    pub fn run_until_timeout(
        &mut self,
        timeout: Duration,
        mut stop: impl FnMut(&mut Self) -> bool,
    ) -> bool {
        let deadline = Instant::now() + timeout;
        self.block_on_pump(Some(deadline), |page| {
            Instant::now() < deadline && !stop(page) && !page.stopped()
        });
        stop(self)
    }

    fn block_on_pump(&mut self, cap: Option<Instant>, keep_waiting: impl FnMut(&mut Self) -> bool) {
        assert!(
            tokio::runtime::Handle::try_current().is_err(),
            "Page::run must not run inside another Tokio runtime"
        );
        let runtime = self.runtime.take().unwrap_or_else(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .expect("current-thread Tokio runtime for the page thread")
        });
        runtime.block_on(self.pump(cap, keep_waiting));
        self.runtime = Some(runtime);
    }

    pub(crate) fn shutdown_runtime(&mut self) {
        self.fetch.persist();
        self.stop.request();
        self.fetches.abort_all();
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }

    fn stopped(&self) -> bool {
        self.stop.is_set()
    }

    fn resolve_dial_url(&self, spec: &str) -> Result<Url, PageError> {
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
            return self.document_url.clone();
        };
        let Ok(Some(base_el)) = parsed.dom.select_first(parsed.dom.document(), "base[href]") else {
            return self.document_url.clone();
        };
        let Some(href) = parsed.dom.attribute(base_el, "href") else {
            return self.document_url.clone();
        };
        self.document_url
            .join(&href)
            .unwrap_or_else(|_| self.document_url.clone())
    }

    async fn pump(
        &mut self,
        cap: Option<Instant>,
        mut keep_waiting: impl FnMut(&mut Self) -> bool,
    ) {
        loop {
            self.adopt_js_work();
            self.launch_queued_dials();
            while let Some(job) = self.jobs.pop_front() {
                self.run_job(job);
                self.adopt_js_work();
                self.launch_queued_dials();
                if !keep_waiting(self) {
                    return;
                }
            }
            if !keep_waiting(self) {
                return;
            }
            if self
                .js
                .as_ref()
                .is_some_and(crate::js::JsHost::has_pending_work)
            {
                continue;
            }
            let fetches_pending = !self.fetches.is_empty();
            let queued = !self.queued_dials.is_empty();
            let next_deadline = match (self.next_timer_deadline(), cap) {
                (Some(timer), Some(limit)) => Some(timer.min(limit)),
                (timer, limit) => timer.or(limit),
            };
            if !fetches_pending && !queued && next_deadline.is_none() {
                break;
            }
            if queued && !fetches_pending {
                continue;
            }
            let deadline = wait_until(next_deadline);
            let mut deadline = std::pin::pin!(deadline);
            let stop = Arc::clone(&self.stop);
            let job = poll_fn(|cx| {
                stop.register(cx.waker());
                if stop.is_set() {
                    return Poll::Ready(None);
                }
                if deadline.as_mut().poll(cx).is_ready() {
                    return Poll::Ready(None);
                }
                if fetches_pending
                    && let Poll::Ready(Some(joined)) = self.fetches.poll_join_next(cx)
                {
                    return Poll::Ready(Some(joined));
                }
                Poll::Pending
            })
            .await;
            if let Some(joined) = job {
                match joined.expect("fetch worker panicked") {
                    Ok(done) => self.jobs.push_back(HtmlJob::DialFinished(done)),
                    Err(fail) => self.jobs.push_back(HtmlJob::DialFailed(fail)),
                }
            } else {
                while let Some(id) = self.due_timer() {
                    self.jobs.push_back(HtmlJob::Timer(id));
                }
            }
        }
    }

    fn waiting_for_load(&self) -> bool {
        self.queued_dials.iter().any(|dial| match dial {
            QueuedDial::Navigate { epoch, .. } => *epoch == self.nav_epoch,
            QueuedDial::ClassicScript { epoch, .. } => *epoch == self.js_epoch,
            QueuedDial::Fetch { .. } | QueuedDial::JsFetch { .. } => false,
        }) || self.nav_in_flight == Some(self.nav_epoch)
            || self.classic_fetch_in_flight
            || !self.pending_classic.is_empty()
            || !self.world.borrow().document_ready
    }

    fn launch_queued_dials(&mut self) {
        let mut leftover = Vec::new();
        let queued = std::mem::take(&mut self.queued_dials);
        for dial in queued {
            if self.fetches.len() >= MAX_IN_FLIGHT_DIALS {
                leftover.push(dial);
                continue;
            }
            if let QueuedDial::Navigate { epoch, .. } = &dial {
                self.nav_in_flight = Some(*epoch);
            }
            let fetch = self.fetch.clone();
            self.fetches
                .spawn_blocking(move || send_dial(&fetch, &dial));
        }
        leftover.extend(std::mem::take(&mut self.queued_dials));
        self.queued_dials = leftover;
    }

    fn finish_dial(&mut self, done: CompletedDial) {
        match done {
            CompletedDial::Fetch { status } => {
                self.events.push(PageEvent::Fetch { status });
            }
            CompletedDial::Navigate {
                status,
                body,
                final_url,
                content_language,
                epoch,
            } => {
                self.events.push(PageEvent::Fetch { status });
                if self.nav_in_flight == Some(epoch) {
                    self.nav_in_flight = None;
                }
                if epoch == self.nav_epoch {
                    self.navigation_failed = false;
                    self.apply_navigation(final_url, content_language, &body);
                }
            }
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
                    self.pending_classic.pop_front();
                    if (200..300).contains(&status) {
                        let source = String::from_utf8_lossy(&body);
                        self.eval_classic(&source);
                    }
                    self.advance_classic_scripts();
                }
            }
        }
        self.adopt_js_work();
    }

    fn fail_dial(&mut self, fail: DialFail) {
        self.events.push(PageEvent::FetchFailed);
        match fail {
            DialFail::Fetch => {}
            DialFail::Navigate { epoch } => {
                if self.nav_in_flight == Some(epoch) {
                    self.nav_in_flight = None;
                }
                if epoch == self.nav_epoch {
                    self.navigation_failed = true;
                }
            }
            DialFail::JsFetch { id, epoch } => {
                if epoch == self.js_epoch {
                    self.settle_js_fetch(id, false, 0, "");
                }
            }
            DialFail::ClassicScript { epoch } => {
                if epoch == self.js_epoch {
                    self.classic_fetch_in_flight = false;
                    self.pending_classic.pop_front();
                    self.advance_classic_scripts();
                }
            }
        }
        self.adopt_js_work();
    }

    fn apply_navigation(&mut self, final_url: Url, content_language: Option<String>, body: &[u8]) {
        self.reset_js_realm();
        let html = String::from_utf8_lossy(body);
        self.document_url = final_url.clone();
        self.content_language.clone_from(&content_language);
        {
            let mut world = self.world.borrow_mut();
            world.document_url = final_url;
            world.replace_document(parse_html(&html));
            if let Some(parsed) = world.parsed.as_mut() {
                parsed.dom.set_document_language(content_language);
            }
        }
        self.boot_document();
    }

    fn settle_js_fetch(&mut self, id: i32, ok: bool, status: i32, body: &str) {
        if let Some(js) = &self.js {
            Self::note_script(
                &mut self.events,
                js.finish_js_fetch(id, ok, status, body).is_err(),
            );
        }
    }

    fn note_script(events: &mut Vec<PageEvent>, failed: bool) {
        if failed {
            events.push(PageEvent::ScriptFailed);
        }
    }

    fn run_job(&mut self, job: HtmlJob) {
        match job {
            HtmlJob::Timer(id) => {
                self.events.push(PageEvent::Timer(id));
                if let Some(js_id) = self.js_timer_slots.remove(&id)
                    && let Some(js) = &self.js
                {
                    Self::note_script(&mut self.events, js.fire_timer(js_id).is_err());
                }
                self.timers.retain(|timer| timer.id != id);
                self.adopt_js_work();
            }
            HtmlJob::DialFinished(done) => self.finish_dial(done),
            HtmlJob::DialFailed(fail) => self.fail_dial(fail),
        }
    }

    fn adopt_js_work(&mut self) {
        let timeouts = self
            .js
            .as_ref()
            .map(crate::js::JsHost::take_pending_timeouts)
            .unwrap_or_default();
        let fetches = self
            .js
            .as_ref()
            .map(crate::js::JsHost::take_pending_fetches)
            .unwrap_or_default();
        let cancels: HashSet<i32> = self
            .js
            .as_ref()
            .map(crate::js::JsHost::take_pending_cancels)
            .unwrap_or_default()
            .into_iter()
            .collect();
        for js_id in &cancels {
            if let Some(page_id) = self
                .js_timer_slots
                .iter()
                .find_map(|(&page_id, &id)| (id == *js_id).then_some(page_id))
            {
                self.js_timer_slots.remove(&page_id);
                self.timers.retain(|timer| timer.id != page_id);
            }
        }
        for timeout in timeouts {
            if cancels.contains(&timeout.js_id) {
                continue;
            }
            let id = self.schedule_timer(timeout.delay);
            self.js_timer_slots.insert(id, timeout.js_id);
        }
        for fetch in fetches {
            if let Ok(url) = self.resolve_dial_url(&fetch.url) {
                let initiator = self.document_url.clone();
                self.queued_dials.push(QueuedDial::JsFetch {
                    url,
                    initiator,
                    id: fetch.js_id,
                    epoch: self.js_epoch,
                });
            } else {
                self.events.push(PageEvent::FetchFailed);
                self.settle_js_fetch(fetch.js_id, false, 0, "");
            }
        }
    }

    fn reset_js_realm(&mut self) {
        self.js_epoch = self.js_epoch.saturating_add(1);
        self.queued_dials.retain(|dial| {
            !matches!(
                dial,
                QueuedDial::JsFetch { .. } | QueuedDial::ClassicScript { .. }
            )
        });
        let js_timer_ids: HashSet<u32> = self.js_timer_slots.keys().copied().collect();
        self.timers
            .retain(|timer| !js_timer_ids.contains(&timer.id));
        self.js_timer_slots.clear();
        self.js = None;
        self.pending_classic.clear();
        self.classic_fetch_in_flight = false;
    }

    fn ensure_js(&mut self) -> Result<(), PageError> {
        if self.js.is_none() {
            self.js = Some(crate::js::JsHost::new(self.world.clone()).map_err(PageError::from)?);
        }
        Ok(())
    }

    fn boot_document(&mut self) {
        if self.ensure_js().is_err() {
            self.events.push(PageEvent::ScriptFailed);
            return;
        }
        let scripts = collect_classic_scripts(&self.world.borrow());
        self.pending_classic = scripts.into();
        self.advance_classic_scripts();
    }

    fn eval_classic(&mut self, source: &str) {
        if let Some(js) = &self.js {
            Self::note_script(&mut self.events, js.eval(source).is_err());
        }
        self.adopt_js_work();
    }

    fn advance_classic_scripts(&mut self) {
        loop {
            match self.pending_classic.front().cloned() {
                Some(ClassicScript::Inline(source)) => {
                    self.pending_classic.pop_front();
                    self.eval_classic(&source);
                }
                Some(ClassicScript::Src(src)) => {
                    if self.classic_fetch_in_flight {
                        return;
                    }
                    if let Ok(url) = self.resolve_dial_url(&src) {
                        self.classic_fetch_in_flight = true;
                        let initiator = self.document_url.clone();
                        self.queued_dials.push(QueuedDial::ClassicScript {
                            url,
                            initiator,
                            epoch: self.js_epoch,
                        });
                    } else {
                        self.pending_classic.pop_front();
                        continue;
                    }
                    return;
                }
                None => {
                    self.fire_document_load();
                    return;
                }
            }
        }
    }

    fn fire_document_load(&mut self) {
        if self.world.borrow().document_ready {
            return;
        }
        self.world.borrow_mut().document_ready = true;
        if let Some(js) = &self.js {
            Self::note_script(&mut self.events, js.fire_load().is_err());
        }
        self.adopt_js_work();
    }

    fn next_timer_deadline(&self) -> Option<Instant> {
        self.timers
            .iter()
            .filter(|timer| !timer.fired)
            .map(|timer| timer.when)
            .min()
    }

    fn due_timer(&mut self) -> Option<u32> {
        let now = Instant::now();
        let timer = self
            .timers
            .iter_mut()
            .filter(|timer| !timer.fired && timer.when <= now)
            .min_by_key(|timer| timer.when)?;
        timer.fired = true;
        Some(timer.id)
    }
}

fn send_dial(fetch: &FetchHandle, dial: &QueuedDial) -> Result<CompletedDial, DialFail> {
    let fail = match dial {
        QueuedDial::Fetch { .. } => DialFail::Fetch,
        QueuedDial::Navigate { epoch, .. } => DialFail::Navigate { epoch: *epoch },
        QueuedDial::JsFetch { id, epoch, .. } => DialFail::JsFetch {
            id: *id,
            epoch: *epoch,
        },
        QueuedDial::ClassicScript { epoch, .. } => DialFail::ClassicScript { epoch: *epoch },
    };
    let (url, context, initiator, read_body) = match dial {
        QueuedDial::Fetch { url, initiator } => (url, Context::Fetch, initiator, false),
        QueuedDial::Navigate { url, initiator, .. } => (url, Context::Navigation, initiator, true),
        QueuedDial::JsFetch { url, initiator, .. }
        | QueuedDial::ClassicScript { url, initiator, .. } => {
            (url, Context::Fetch, initiator, true)
        }
    };
    let response = fetch
        .request(Method::GET, url.clone())
        .with_context(context)
        .with_initiator(initiator.clone())
        .send()
        .map_err(|_| fail)?;
    fetch.mark_dirty();
    if matches!(dial, QueuedDial::Navigate { .. }) {
        fetch.persist();
    }
    let status = response.status();
    let final_url = response.final_url().clone();
    let content_language = response
        .headers()
        .get("content-language")
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .and_then(content_language_tag);
    let body = if read_body {
        response
            .into_body()
            .bytes(FETCH_BODY_LIMIT)
            .map_err(|_| fail)?
    } else {
        Vec::new()
    };
    Ok(match dial {
        QueuedDial::Fetch { .. } => CompletedDial::Fetch { status },
        QueuedDial::Navigate { epoch, .. } => CompletedDial::Navigate {
            status,
            body,
            final_url,
            content_language,
            epoch: *epoch,
        },
        QueuedDial::JsFetch { id, epoch, .. } => CompletedDial::JsFetch {
            status,
            body,
            id: *id,
            epoch: *epoch,
        },
        QueuedDial::ClassicScript { epoch, .. } => CompletedDial::ClassicScript {
            status,
            body,
            epoch: *epoch,
        },
    })
}

/// One `Content-Language` tag, or `None` when the header lists several
/// languages ([HTML document language](https://html.spec.whatwg.org/multipage/dom.html#language)).
fn content_language_tag(raw: &str) -> Option<String> {
    let mut tags = raw
        .split(',')
        .map(|part| part.split(';').next().unwrap_or(part).trim())
        .filter(|tag| !tag.is_empty());
    let first = tags.next()?.to_owned();
    if tags.next().is_some() {
        return None;
    }
    Some(first)
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

async fn wait_until(deadline: Option<Instant>) {
    match deadline {
        Some(when) => sleep_until(when).await,
        None => pending().await,
    }
}
