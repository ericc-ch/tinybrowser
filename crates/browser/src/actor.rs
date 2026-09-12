//! One browser-side tab: identity, navigation, and the renderer link.
//!
//! [ADR 0011](../../../docs/adrs/0011-renderer-processes-per-site.md): the browser
//! process dials, picks the site renderer, and mounts the document; the renderer
//! owns the document. `TabHandle` is the protocol surface and stays value-only.

use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use renderer::{
    Command as RendererCommand, FrameId, Mount, RemoteValue, Reply, TabError, TabEvent,
};
use url::Url;

use crate::link::{RendererFactory, RendererHandle};
use crate::network::{FetchHandle, NavOutcome};
use crate::site::Site;

const MAX_EVENT_HISTORY: usize = 1024;
const EVENT_SUBSCRIBER_CAPACITY: usize = 256;
const COMMAND_CAPACITY: usize = 256;

/// Identity of one tab in a [`crate::Browser`] registry.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct TabId(u64);

impl TabId {
    /// Constructs a tab id from a protocol integer.
    #[must_use]
    pub fn new(raw: u64) -> Self {
        Self(raw)
    }

    /// Stable numeric identity for protocol adapters.
    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for TabId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Correlates one command with its reply.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct RequestId(u64);

impl RequestId {
    /// Numeric identity of this request.
    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

enum Command {
    LoadHtml {
        html: String,
        reply: Sender<Result<(), TabError>>,
    },
    Goto {
        url: String,
        reply: Sender<Result<(), TabError>>,
    },
    Eval {
        source: String,
        reply: Sender<Result<String, TabError>>,
    },
    Execute {
        source: String,
        timeout: Option<Duration>,
        reply: Sender<Result<RemoteValue, TabError>>,
    },
    Run {
        reply: Sender<()>,
    },
    RunUntilLoad {
        reply: Sender<()>,
    },
    RunUntilLoadTimeout {
        timeout: Duration,
        reply: Sender<bool>,
    },
    RunUntilJsTrue {
        source: String,
        timeout: Duration,
        reply: Sender<bool>,
    },
    DocumentUrl {
        reply: Sender<String>,
    },
    ContentLanguage {
        reply: Sender<Option<String>>,
    },
    CookieGet {
        reply: Sender<String>,
    },
    CookieSet {
        value: String,
        reply: Sender<()>,
    },
    SetDocumentUrl {
        url: String,
        reply: Sender<Result<(), TabError>>,
    },
    Events {
        reply: Sender<Vec<TabEvent>>,
    },
    Subscribe {
        reply: Sender<Receiver<TabEvent>>,
    },
    LastNavigationFailed {
        reply: Sender<bool>,
    },
    Shutdown {
        reply: Sender<()>,
    },
}

struct Envelope {
    request_id: RequestId,
    command: Command,
}

enum Waiter {
    Idle(Sender<()>),
    Load(Sender<()>),
    LoadTimeout {
        deadline: Instant,
        reply: Sender<bool>,
    },
    JsTrue {
        source: String,
        deadline: Instant,
        reply: Sender<bool>,
    },
}

/// Value-only handle to one [`TabActor`].
#[derive(Clone)]
pub struct TabHandle {
    id: TabId,
    next_request: Arc<AtomicU64>,
    current: Arc<Mutex<Option<Arc<RendererHandle>>>>,
    tx: SyncSender<Envelope>,
}

impl TabHandle {
    /// Tab identity in the owning browser.
    #[must_use]
    pub fn id(&self) -> TabId {
        self.id
    }

    /// Next request id that a command from this handle would use.
    #[must_use]
    pub fn next_request_id(&self) -> RequestId {
        RequestId(self.next_request.load(Ordering::Relaxed))
    }

    /// Parses `html` into this tab and starts a new JS realm.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`] when the tab or its renderer has shut down.
    pub fn load_html(&self, html: &str) -> Result<(), TabError> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::LoadHtml {
            html: html.to_owned(),
            reply,
        })?;
        recv_result(&rx)
    }

    /// Starts navigation. The tab continues independently; call
    /// [`TabHandle::run_until_load`] only when the caller needs to wait.
    ///
    /// # Errors
    ///
    /// [`TabError::InvalidUrl`] or [`TabError::ActorStopped`].
    pub fn goto(&self, url: &str) -> Result<(), TabError> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::Goto {
            url: url.to_owned(),
            reply,
        })?;
        recv_result(&rx)
    }

    /// Evaluates `source` and returns its string coercion.
    ///
    /// # Errors
    ///
    /// [`TabError::Script`] or [`TabError::ActorStopped`].
    pub fn eval(&self, source: &str) -> Result<String, TabError> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::Eval {
            source: source.to_owned(),
            reply,
        })?;
        recv_result(&rx)
    }

    /// Evaluates `source` and returns a value-only script result.
    ///
    /// # Errors
    ///
    /// [`TabError::Script`] or [`TabError::ActorStopped`].
    pub fn execute_script(&self, source: &str) -> Result<RemoteValue, TabError> {
        self.execute_script_timeout(source, None)
    }

    /// Evaluates `source`, interrupting `QuickJS` if `timeout` elapses.
    ///
    /// # Errors
    ///
    /// [`TabError::Script`] or [`TabError::ActorStopped`].
    pub fn execute_script_timeout(
        &self,
        source: &str,
        timeout: Option<Duration>,
    ) -> Result<RemoteValue, TabError> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::Execute {
            source: source.to_owned(),
            timeout,
            reply,
        })?;
        recv_result(&rx)
    }

    /// Waits until no tab jobs remain without preventing other commands.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`].
    pub fn run(&self) -> Result<(), TabError> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::Run { reply })?;
        recv_unit(&rx)
    }

    /// Waits until the current navigation has fired `load`.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`].
    pub fn run_until_load(&self) -> Result<(), TabError> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::RunUntilLoad { reply })?;
        recv_unit(&rx)
    }

    /// Waits like [`TabHandle::run_until_load`], returning `false` on timeout.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`].
    pub fn run_until_load_timeout(&self, timeout: Duration) -> Result<bool, TabError> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::RunUntilLoadTimeout { timeout, reply })?;
        recv_bool(&rx)
    }

    /// Waits until `source` evaluates to JS `true`, returning `false` on timeout.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`].
    pub fn run_until_js_true(&self, source: &str, timeout: Duration) -> Result<bool, TabError> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::RunUntilJsTrue {
            source: source.to_owned(),
            timeout,
            reply,
        })?;
        recv_bool(&rx)
    }

    /// Document URL after navigation.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`].
    pub fn document_url(&self) -> Result<String, TabError> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::DocumentUrl { reply })?;
        recv_text(&rx)
    }

    /// Document `Content-Language`, if any.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`].
    pub fn content_language(&self) -> Result<Option<String>, TabError> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::ContentLanguage { reply })?;
        recv_optional_text(&rx)
    }

    /// `document.cookie` getter.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`].
    pub fn document_cookie(&self) -> Result<String, TabError> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::CookieGet { reply })?;
        recv_text(&rx)
    }

    /// `document.cookie` setter.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`].
    pub fn set_document_cookie(&self, value: &str) -> Result<(), TabError> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::CookieSet {
            value: value.to_owned(),
            reply,
        })?;
        recv_unit(&rx)
    }

    /// Sets the initial document URL before the first mount.
    ///
    /// # Errors
    ///
    /// [`TabError::InvalidUrl`] or [`TabError::ActorStopped`].
    pub fn set_document_url(&self, url: &str) -> Result<(), TabError> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::SetDocumentUrl {
            url: url.to_owned(),
            reply,
        })?;
        recv_result(&rx)
    }

    /// The most recent jobs that ran, in order, capped at 1,024 events.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`].
    pub fn events(&self) -> Result<Vec<TabEvent>, TabError> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::Events { reply })?;
        recv_events(&rx)
    }

    /// Subscribes to tab events emitted after this call.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`] when the actor has shut down.
    pub fn subscribe(&self) -> Result<Receiver<TabEvent>, TabError> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::Subscribe { reply })?;
        rx.recv().map_err(|_| TabError::ActorStopped)
    }

    /// True when the last navigation dial failed.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`].
    pub fn last_navigation_failed(&self) -> Result<bool, TabError> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::LastNavigationFailed { reply })?;
        recv_bool(&rx)
    }

    /// Asks the tab to stop. Further commands fail with
    /// [`TabError::ActorStopped`].
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`] if the actor is already gone.
    pub fn shutdown(&self) -> Result<(), TabError> {
        self.interrupt_renderer();
        let (reply, rx) = mpsc::channel();
        self.send(Command::Shutdown { reply })?;
        recv_unit(&rx)
    }

    fn interrupt_renderer(&self) {
        if let Some(renderer) = self
            .current
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
        {
            renderer.interrupt();
        }
    }

    fn send(&self, command: Command) -> Result<(), TabError> {
        let request_id = RequestId(self.next_request.fetch_add(1, Ordering::Relaxed));
        self.tx
            .send(Envelope {
                request_id,
                command,
            })
            .map_err(|_| TabError::ActorStopped)
    }
}

fn recv_result<T>(rx: &Receiver<Result<T, TabError>>) -> Result<T, TabError> {
    rx.recv().unwrap_or(Err(TabError::ActorStopped))
}

fn recv_unit(rx: &Receiver<()>) -> Result<(), TabError> {
    rx.recv().map_err(|_| TabError::ActorStopped)
}

fn recv_bool(rx: &Receiver<bool>) -> Result<bool, TabError> {
    rx.recv().map_err(|_| TabError::ActorStopped)
}

fn recv_text(rx: &Receiver<String>) -> Result<String, TabError> {
    rx.recv().map_err(|_| TabError::ActorStopped)
}

fn recv_optional_text(rx: &Receiver<Option<String>>) -> Result<Option<String>, TabError> {
    rx.recv().map_err(|_| TabError::ActorStopped)
}

fn recv_events(rx: &Receiver<Vec<TabEvent>>) -> Result<Vec<TabEvent>, TabError> {
    rx.recv().map_err(|_| TabError::ActorStopped)
}

/// Join handle and command sender for one tab actor thread.
pub(crate) struct TabActor {
    pub handle: TabHandle,
    join: Option<JoinHandle<()>>,
}

impl TabActor {
    pub(crate) fn spawn(id: TabId, fetch: FetchHandle, renderers: Arc<RendererFactory>) -> Self {
        let (tx, rx) = mpsc::sync_channel(COMMAND_CAPACITY);
        let current = Arc::new(Mutex::new(None));
        let handle = TabHandle {
            id,
            next_request: Arc::new(AtomicU64::new(1)),
            current: Arc::clone(&current),
            tx,
        };
        let tab = Tab::new(id, fetch, renderers, current);
        let join = thread::Builder::new()
            .name(format!("tab-{id}"))
            .spawn(move || actor_loop(&rx, tab))
            .expect("tab actor thread");
        Self {
            handle,
            join: Some(join),
        }
    }

    pub(crate) fn shutdown(&mut self) {
        self.handle.interrupt_renderer();
        let (reply, rx) = mpsc::channel();
        let _ = self.handle.tx.send(Envelope {
            request_id: RequestId(0),
            command: Command::Shutdown { reply },
        });
        let _ = rx.recv_timeout(Duration::from_secs(2));
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for TabActor {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct ActiveNavigation {
    url: Url,
    initiator: Url,
    epoch: u64,
    submitted: bool,
}

/// Browser-owned tab state: identity, URL, navigation, and the renderer link.
struct Tab {
    id: TabId,
    renderers: Arc<RendererFactory>,
    fetch: FetchHandle,
    current: Arc<Mutex<Option<Arc<RendererHandle>>>>,
    renderer: Option<Arc<RendererHandle>>,
    site: Option<Site>,
    events_rx: Option<Receiver<(FrameId, TabEvent)>>,
    document_url: Url,
    content_language: Option<String>,
    document_loaded: bool,
    nav_epoch: u64,
    nav_in_flight: Option<u64>,
    navigation_failed: bool,
    nav: Option<ActiveNavigation>,
    dial_tx: Sender<(u64, Result<NavOutcome, ()>)>,
    dial_rx: Receiver<(u64, Result<NavOutcome, ()>)>,
    events: VecDeque<TabEvent>,
    subscribers: Vec<SyncSender<TabEvent>>,
}

impl Tab {
    fn new(
        id: TabId,
        fetch: FetchHandle,
        renderers: Arc<RendererFactory>,
        current: Arc<Mutex<Option<Arc<RendererHandle>>>>,
    ) -> Self {
        let (dial_tx, dial_rx) = mpsc::channel();
        Self {
            id,
            renderers,
            fetch,
            current,
            renderer: None,
            site: None,
            events_rx: None,
            document_url: Url::parse("about:blank").expect("about:blank is a valid URL"),
            content_language: None,
            document_loaded: false,
            nav_epoch: 0,
            nav_in_flight: None,
            navigation_failed: false,
            nav: None,
            dial_tx,
            dial_rx,
            events: VecDeque::new(),
            subscribers: Vec::new(),
        }
    }

    fn load_html(&mut self, html: &str) -> Result<(), TabError> {
        let site = Site::for_url(&self.document_url)
            .or_else(|| self.site.clone())
            .unwrap_or_else(|| Site::opaque(self.id));
        let mount = Mount {
            url: self.document_url.to_string(),
            content_type: Some("text/html; charset=utf-8".to_owned()),
            content_language: None,
            body: html.as_bytes().to_vec(),
        };
        self.mount(&site, mount)
    }

    fn goto(&mut self, spec: &str) -> Result<(), TabError> {
        let url = self.resolve_url(spec)?;
        self.nav_epoch = self.nav_epoch.saturating_add(1);
        self.navigation_failed = false;
        self.nav_in_flight = None;
        self.nav = Some(ActiveNavigation {
            url,
            initiator: self.document_url.clone(),
            epoch: self.nav_epoch,
            submitted: false,
        });
        Ok(())
    }

    fn resolve_url(&self, spec: &str) -> Result<Url, TabError> {
        let url = Url::parse(spec)
            .or_else(|_| self.document_url.join(spec))
            .map_err(|_| TabError::InvalidUrl { spec: spec.into() })?;
        if url.scheme() != "http" && url.scheme() != "https" {
            return Err(TabError::InvalidUrl { spec: spec.into() });
        }
        Ok(url)
    }

    fn set_document_url(&mut self, url: &str) -> Result<(), TabError> {
        let parsed = Url::parse(url).map_err(|_| TabError::InvalidUrl { spec: url.into() })?;
        if self.renderer.is_some() {
            return Err(TabError::InvalidUrl { spec: url.into() });
        }
        self.document_url = parsed;
        Ok(())
    }

    fn document_cookie(&self) -> String {
        self.fetch.cookies_for(&self.document_url)
    }

    fn set_document_cookie(&self, value: &str) {
        self.fetch.set_cookie(value, &self.document_url);
    }

    fn ensure_renderer(&mut self, site: &Site) -> Result<(), TabError> {
        if self.renderer.is_some() && self.site.as_ref() == Some(site) {
            return Ok(());
        }
        let handle =
            self.renderers
                .acquire(site)
                .map_err(|error| TabError::RendererUnavailable {
                    message: error.to_string(),
                })?;
        self.drop_renderer();
        self.events_rx = Some(handle.subscribe());
        *self.current.lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::clone(&handle));
        self.site = Some(site.clone());
        self.renderer = Some(handle);
        Ok(())
    }

    fn mount(&mut self, site: &Site, mount: Mount) -> Result<(), TabError> {
        self.ensure_renderer(site)?;
        self.document_loaded = false;
        let result = self
            .renderer_request(RendererCommand::Mount {
                frame: FrameId::MAIN,
                mount,
            })
            .and_then(reply_unit);
        if result.is_err() {
            // A dead renderer must not be reused: drop it so the next mount
            // acquires a fresh one for this site.
            self.drop_renderer();
        }
        result
    }

    fn drop_renderer(&mut self) {
        *self.current.lock().unwrap_or_else(PoisonError::into_inner) = None;
        self.renderer = None;
        self.site = None;
        self.events_rx = None;
    }

    fn renderer_request(&self, command: RendererCommand) -> Result<Reply, TabError> {
        match &self.renderer {
            Some(renderer) => renderer.request(command),
            None => Err(TabError::ActorStopped),
        }
    }

    fn launch_navigation(&mut self) {
        let Some(nav) = self.nav.as_ref() else {
            return;
        };
        if nav.submitted {
            return;
        }
        let epoch = nav.epoch;
        let url = nav.url.clone();
        let initiator = nav.initiator.clone();
        if self
            .fetch
            .dial_navigation(epoch, url, initiator, self.dial_tx.clone())
            .is_ok()
            && let Some(nav) = self.nav.as_mut()
        {
            nav.submitted = true;
            self.nav_in_flight = Some(epoch);
        }
    }

    fn pump_navigation(&mut self) {
        while let Ok((epoch, result)) = self.dial_rx.try_recv() {
            if self.nav_in_flight == Some(epoch) {
                self.nav_in_flight = None;
            }
            let active = self.nav.as_ref().is_some_and(|nav| nav.epoch == epoch);
            if !active {
                continue;
            }
            self.nav = None;
            if let Ok(outcome) = result {
                self.record_event(TabEvent::Fetch {
                    status: outcome.status,
                });
                self.commit_navigation(outcome);
            } else {
                self.record_event(TabEvent::FetchFailed);
                self.navigation_failed = true;
            }
        }
    }

    fn commit_navigation(&mut self, outcome: NavOutcome) {
        let site = Site::for_url(&outcome.final_url).unwrap_or_else(|| Site::opaque(self.id));
        self.document_url = outcome.final_url.clone();
        self.content_language.clone_from(&outcome.content_language);
        let mount = Mount {
            url: outcome.final_url.to_string(),
            content_type: outcome.content_type,
            content_language: outcome.content_language,
            body: outcome.body,
        };
        if self.mount(&site, mount).is_err() {
            self.navigation_failed = true;
        }
    }

    fn pump_renderer(&mut self) {
        let mut arrived = Vec::new();
        if let Some(events) = &self.events_rx {
            while let Ok(event) = events.try_recv() {
                arrived.push(event);
            }
        }
        for (frame, event) in arrived {
            if frame == FrameId::MAIN && event == TabEvent::Load {
                self.document_loaded = true;
            }
            self.record_event(event);
        }
    }

    fn record_event(&mut self, event: TabEvent) {
        if self.events.len() == MAX_EVENT_HISTORY {
            self.events.pop_front();
        }
        self.events.push_back(event);
        self.subscribers
            .retain(|subscriber| matches!(subscriber.try_send(event), Ok(())));
    }

    fn busy(&self) -> bool {
        self.nav.is_some()
    }

    fn has_background_work(&mut self) -> bool {
        if self.nav.is_some() {
            return true;
        }
        if self.renderer.is_none() {
            return false;
        }
        match self.renderer_request(RendererCommand::IsIdle) {
            Ok(Reply::Bool(idle)) => !idle,
            _ => false,
        }
    }

    fn waiting_for_load(&self) -> bool {
        self.nav.is_some() || (!self.document_loaded && !self.navigation_failed)
    }

    fn stop_renderer(&mut self) {
        *self.current.lock().unwrap_or_else(PoisonError::into_inner) = None;
        if let Some(renderer) = self.renderer.take() {
            renderer.request_shutdown();
        }
    }
}

fn actor_loop(rx: &Receiver<Envelope>, mut tab: Tab) {
    let mut waiters = Vec::new();
    loop {
        tab.pump_renderer();
        tab.launch_navigation();
        tab.pump_navigation();
        let received = if tab.busy() || !waiters.is_empty() || !tab.subscribers.is_empty() {
            rx.recv_timeout(Duration::from_millis(10))
        } else {
            rx.recv().map_err(|_| RecvTimeoutError::Disconnected)
        };
        let envelope = match received {
            Ok(envelope) => envelope,
            Err(RecvTimeoutError::Timeout) => {
                resolve_waiters(&mut tab, &mut waiters);
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        let _request_id = envelope.request_id;
        if handle_command(&mut tab, envelope.command, &mut waiters) {
            return;
        }
        tab.pump_renderer();
        resolve_waiters(&mut tab, &mut waiters);
    }
    tab.stop_renderer();
}

/// Handles one command; `true` means the actor returns.
fn handle_command(tab: &mut Tab, command: Command, waiters: &mut Vec<Waiter>) -> bool {
    match command {
        Command::LoadHtml { html, reply } => {
            let _ = reply.send(tab.load_html(&html));
        }
        Command::Goto { url, reply } => {
            let _ = reply.send(tab.goto(&url));
        }
        Command::Eval { source, reply } => {
            let result = tab
                .renderer_request(RendererCommand::Eval {
                    frame: FrameId::MAIN,
                    source,
                })
                .and_then(reply_text);
            let _ = reply.send(result);
        }
        Command::Execute {
            source,
            timeout,
            reply,
        } => {
            let result = tab
                .renderer_request(RendererCommand::ExecuteScript {
                    frame: FrameId::MAIN,
                    source,
                    timeout_ms: timeout.map(millis),
                })
                .and_then(reply_value);
            let _ = reply.send(result);
        }
        Command::Run { reply } => {
            waiters.push(Waiter::Idle(reply));
        }
        Command::RunUntilLoad { reply } => {
            waiters.push(Waiter::Load(reply));
        }
        Command::RunUntilLoadTimeout { timeout, reply } => {
            waiters.push(Waiter::LoadTimeout {
                deadline: Instant::now() + timeout,
                reply,
            });
        }
        Command::RunUntilJsTrue {
            source,
            timeout,
            reply,
        } => {
            waiters.push(Waiter::JsTrue {
                source,
                deadline: Instant::now() + timeout,
                reply,
            });
        }
        Command::DocumentUrl { reply } => {
            let _ = reply.send(tab.document_url.to_string());
        }
        Command::ContentLanguage { reply } => {
            let _ = reply.send(tab.content_language.clone());
        }
        Command::CookieGet { reply } => {
            let _ = reply.send(tab.document_cookie());
        }
        Command::CookieSet { value, reply } => {
            tab.set_document_cookie(&value);
            let _ = reply.send(());
        }
        Command::SetDocumentUrl { url, reply } => {
            let _ = reply.send(tab.set_document_url(&url));
        }
        Command::Events { reply } => {
            let _ = reply.send(tab.events.iter().copied().collect());
        }
        Command::Subscribe { reply } => {
            let (events, event_rx) = mpsc::sync_channel(EVENT_SUBSCRIBER_CAPACITY);
            tab.subscribers.push(events);
            let _ = reply.send(event_rx);
        }
        Command::LastNavigationFailed { reply } => {
            let _ = reply.send(tab.navigation_failed);
        }
        Command::Shutdown { reply } => {
            tab.stop_renderer();
            let _ = reply.send(());
            return true;
        }
    }
    false
}

fn resolve_waiters(tab: &mut Tab, waiters: &mut Vec<Waiter>) {
    let now = Instant::now();
    let mut pending = Vec::new();
    for waiter in std::mem::take(waiters) {
        match waiter {
            Waiter::Idle(reply) if !tab.has_background_work() => {
                let _ = reply.send(());
            }
            Waiter::Load(reply) if !tab.waiting_for_load() => {
                let _ = reply.send(());
            }
            Waiter::LoadTimeout { reply, .. } if !tab.waiting_for_load() => {
                let _ = reply.send(true);
            }
            Waiter::LoadTimeout { deadline, reply } if now >= deadline => {
                let _ = reply.send(false);
            }
            Waiter::JsTrue {
                source,
                deadline,
                reply,
            } => {
                if now >= deadline {
                    let _ = reply.send(false);
                } else if matches!(
                    tab.renderer_request(RendererCommand::ExecuteScript {
                        frame: FrameId::MAIN,
                        source: source.clone(),
                        timeout_ms: None
                    }),
                    Ok(Reply::Value(Ok(RemoteValue::Bool(true))))
                ) {
                    let _ = reply.send(true);
                } else {
                    pending.push(Waiter::JsTrue {
                        source,
                        deadline,
                        reply,
                    });
                }
            }
            other => pending.push(other),
        }
    }
    *waiters = pending;
}

fn reply_unit(reply: Reply) -> Result<(), TabError> {
    match reply {
        Reply::Unit(result) => result,
        _ => Err(TabError::ActorStopped),
    }
}

fn reply_text(reply: Reply) -> Result<String, TabError> {
    match reply {
        Reply::Text(result) => result,
        _ => Err(TabError::ActorStopped),
    }
}

fn reply_value(reply: Reply) -> Result<RemoteValue, TabError> {
    match reply {
        Reply::Value(result) => result,
        _ => Err(TabError::ActorStopped),
    }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
