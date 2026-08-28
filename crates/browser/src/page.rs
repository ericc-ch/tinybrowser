//! One page: HTML jobs we own, Tokio current-thread as the waiter, `Agent` for HTTP.
//!
//! [ADR 0007](../../../wiki/adrs/0007-engine-charter.md): the page thread is
//! Tokio `rt`+`time` only. `Agent::send` runs on `spawn_blocking`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::future::pending;
use std::time::Duration;

use dom::NodeKind;
use net::{Agent, AgentBuilder, Context, Method};
use tokio::task::JoinSet;
use tokio::time::{Instant, sleep_until};
use url::Url;

use crate::{Parsed, parse_html};

/// Upper bound on a host-fetch / navigation body read inside `spawn_blocking`.
const FETCH_BODY_LIMIT: usize = 1_048_576;

/// Caps concurrent `spawn_blocking` dials so one eval loop cannot exhaust threads.
const MAX_IN_FLIGHT_DIALS: usize = 16;

/// Default `Agent` per-call timeout for [`Page::new`].
const PAGE_FETCH_TIMEOUT: Duration = Duration::from_secs(30);

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
    Script {
        /// Engine or host message.
        message: String,
    },
}

impl fmt::Display for PageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUrl { spec } => write!(f, "invalid url: {spec}"),
            Self::Script { message } => write!(f, "script: {message}"),
        }
    }
}

impl std::error::Error for PageError {}

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
}

#[derive(Clone, Copy)]
enum DialKind {
    Fetch,
    Navigate,
    JsFetch,
}

struct CompletedDial {
    kind: DialKind,
    status: u16,
    body: Vec<u8>,
    final_url: Url,
    content_language: Option<String>,
    js_fetch_id: Option<i32>,
    nav_epoch: Option<u64>,
    js_epoch: Option<u64>,
}

#[derive(Clone, Copy)]
struct DialFail {
    js_fetch_id: Option<i32>,
    js_epoch: Option<u64>,
}

struct HostTimer {
    id: u32,
    when: Instant,
    fired: bool,
}

/// One browsing context: tree, cookie jar via [`Agent`], HTML job list.
pub struct Page {
    agent: Agent,
    parsed: Option<Parsed>,
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
}

impl Default for Page {
    fn default() -> Self {
        Self::new()
    }
}

impl Page {
    /// An empty page with a default [`Agent`] (30s per-call timeout) and
    /// document URL `about:blank`.
    #[must_use]
    pub fn new() -> Self {
        Self::with_agent(
            AgentBuilder::new()
                .timeout_per_call(PAGE_FETCH_TIMEOUT)
                .build(),
        )
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
        Self {
            agent,
            parsed: None,
            document_url: Url::parse("about:blank").expect("about:blank is a valid URL"),
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
        }
    }

    /// Last parse result, if any.
    #[must_use]
    pub fn parsed(&self) -> Option<&Parsed> {
        self.parsed.as_ref()
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
        self.parsed
            .as_ref()
            .and_then(|parsed| parsed.dom.document_language())
            .or(self.content_language.as_deref())
    }

    /// Records the document-level `Content-Language` default.
    pub fn set_content_language(&mut self, value: Option<String>) {
        self.content_language.clone_from(&value);
        if let Some(parsed) = &mut self.parsed {
            parsed.dom.set_document_language(value);
        }
    }

    /// Parses `input` into this page's tree and starts a new JS realm.
    pub fn load_html(&mut self, input: &str) {
        self.reset_js_realm();
        self.parsed = Some(parse_html(input));
        if let Some(parsed) = &mut self.parsed {
            parsed
                .dom
                .set_document_language(self.content_language.clone());
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
        if let Some(js) = &self.js {
            js.set_document_url(self.document_url.clone());
        }
        Ok(())
    }

    /// `document.cookie` getter: non-HTTP jar read for this document URL.
    #[must_use]
    pub fn document_cookie(&self) -> String {
        self.agent.cookies_for(&self.document_url)
    }

    /// `document.cookie` setter: non-HTTP jar write for this document URL.
    pub fn set_document_cookie(&self, value: &str) {
        self.agent.set_cookie(value, &self.document_url);
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
        if self.js.is_none() {
            self.js = Some(
                crate::js::JsHost::new(self.agent.clone(), self.document_url.clone())
                    .map_err(|message| PageError::Script { message })?,
            );
        }
        let out = self
            .js
            .as_ref()
            .ok_or_else(|| PageError::Script {
                message: "js host missing after install".into(),
            })?
            .eval(source)
            .map_err(|message| PageError::Script { message })?;
        self.adopt_js_work();
        Ok(out)
    }

    /// Jobs that have already run, in order.
    #[must_use]
    pub fn events(&self) -> &[PageEvent] {
        &self.events
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
        assert!(
            tokio::runtime::Handle::try_current().is_err(),
            "Page::run must not run inside another Tokio runtime"
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("current-thread Tokio runtime for the page thread");
        runtime.block_on(self.pump());
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
        let Some(parsed) = &self.parsed else {
            return self.document_url.clone();
        };
        let Ok(Some(base_el)) = parsed.dom.select_first(parsed.dom.document(), "base[href]") else {
            return self.document_url.clone();
        };
        let Some(href) = element_attr(&parsed.dom, base_el, "href") else {
            return self.document_url.clone();
        };
        self.document_url
            .join(&href)
            .unwrap_or_else(|_| self.document_url.clone())
    }

    async fn pump(&mut self) {
        loop {
            self.adopt_js_work();
            self.launch_queued_dials();
            while let Some(job) = self.jobs.pop_front() {
                self.run_job(job);
                self.adopt_js_work();
                self.launch_queued_dials();
            }
            let fetches_pending = !self.fetches.is_empty();
            let queued = !self.queued_dials.is_empty();
            let next_deadline = self.next_timer_deadline();
            if !fetches_pending && !queued && next_deadline.is_none() {
                break;
            }
            if queued && !fetches_pending {
                continue;
            }
            tokio::select! {
                Some(joined) = self.fetches.join_next(), if fetches_pending => {
                    match joined.expect("fetch worker panicked") {
                        Ok(done) => self.jobs.push_back(HtmlJob::DialFinished(done)),
                        Err(fail) => self.jobs.push_back(HtmlJob::DialFailed(fail)),
                    }
                }
                () = wait_until(next_deadline) => {
                    while let Some(id) = self.due_timer() {
                        self.jobs.push_back(HtmlJob::Timer(id));
                    }
                }
            }
        }
    }

    fn launch_queued_dials(&mut self) {
        let mut leftover = Vec::new();
        let queued = std::mem::take(&mut self.queued_dials);
        for dial in queued {
            if self.fetches.len() >= MAX_IN_FLIGHT_DIALS {
                leftover.push(dial);
                continue;
            }
            let (kind, url, context, js_fetch_id, initiator, nav_epoch, js_epoch) = match dial {
                QueuedDial::Fetch { url, initiator } => (
                    DialKind::Fetch,
                    url,
                    Context::Fetch,
                    None,
                    initiator,
                    None,
                    None,
                ),
                QueuedDial::Navigate {
                    url,
                    initiator,
                    epoch,
                } => (
                    DialKind::Navigate,
                    url,
                    Context::Navigation,
                    None,
                    initiator,
                    Some(epoch),
                    None,
                ),
                QueuedDial::JsFetch {
                    url,
                    initiator,
                    id,
                    epoch,
                } => (
                    DialKind::JsFetch,
                    url,
                    Context::Fetch,
                    Some(id),
                    initiator,
                    None,
                    Some(epoch),
                ),
            };
            let agent = self.agent.clone();
            self.fetches.spawn_blocking(move || {
                let fail = DialFail {
                    js_fetch_id,
                    js_epoch,
                };
                let response = agent
                    .request(Method::GET, url)
                    .with_context(context)
                    .with_initiator(initiator)
                    .send()
                    .map_err(|_| fail)?;
                let status = response.status();
                let final_url = response.final_url().clone();
                let content_language = response
                    .headers()
                    .get("content-language")
                    .and_then(|bytes| std::str::from_utf8(bytes).ok())
                    .and_then(content_language_tag);
                let body = if matches!(kind, DialKind::Fetch) {
                    Vec::new()
                } else {
                    response
                        .into_body()
                        .bytes(FETCH_BODY_LIMIT)
                        .map_err(|_| fail)?
                };
                Ok(CompletedDial {
                    kind,
                    status,
                    body,
                    final_url,
                    content_language,
                    js_fetch_id,
                    nav_epoch,
                    js_epoch,
                })
            });
        }
        leftover.extend(std::mem::take(&mut self.queued_dials));
        self.queued_dials = leftover;
    }

    fn finish_dial(&mut self, done: CompletedDial) {
        self.events.push(PageEvent::Fetch {
            status: done.status,
        });
        if matches!(done.kind, DialKind::Navigate) && done.nav_epoch == Some(self.nav_epoch) {
            self.apply_navigation(done.final_url, done.content_language, &done.body);
        }
        if let (DialKind::JsFetch, Some(id), Some(epoch)) =
            (done.kind, done.js_fetch_id, done.js_epoch)
            && epoch == self.js_epoch
        {
            let body = String::from_utf8_lossy(&done.body);
            self.settle_js_fetch(id, true, i32::from(done.status), &body);
        }
        self.adopt_js_work();
    }

    fn fail_dial(&mut self, fail: DialFail) {
        self.events.push(PageEvent::FetchFailed);
        if let (Some(id), Some(epoch)) = (fail.js_fetch_id, fail.js_epoch)
            && epoch == self.js_epoch
        {
            self.settle_js_fetch(id, false, 0, "");
        }
        self.adopt_js_work();
    }

    fn apply_navigation(&mut self, final_url: Url, content_language: Option<String>, body: &[u8]) {
        self.reset_js_realm();
        let html = String::from_utf8_lossy(body);
        self.document_url = final_url;
        self.content_language.clone_from(&content_language);
        self.parsed = Some(parse_html(&html));
        if let Some(parsed) = &mut self.parsed {
            parsed.dom.set_document_language(content_language);
        }
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
        for timeout in timeouts {
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
        self.queued_dials
            .retain(|dial| !matches!(dial, QueuedDial::JsFetch { .. }));
        let js_timer_ids: HashSet<u32> = self.js_timer_slots.keys().copied().collect();
        self.timers
            .retain(|timer| !js_timer_ids.contains(&timer.id));
        self.js_timer_slots.clear();
        self.js = None;
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

fn element_attr(dom: &dom::Dom, id: dom::NodeId, name: &str) -> Option<String> {
    match dom.get(id).map(|node| node.kind()) {
        Some(NodeKind::Element { attributes, .. }) => attributes.iter().find_map(|attribute| {
            (attribute.name.ns.is_empty()
                && attribute.name.local.as_ref().eq_ignore_ascii_case(name))
            .then(|| attribute.value.clone())
        }),
        _ => None,
    }
}

async fn wait_until(deadline: Option<Instant>) {
    match deadline {
        Some(when) => sleep_until(when).await,
        None => pending().await,
    }
}
