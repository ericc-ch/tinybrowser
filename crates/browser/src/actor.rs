//! One browser-side tab: identity, navigation, and the renderer link.
//!
//! [ADR 0011](../../../docs/adrs/0011-renderer-processes-per-site.md): the browser
//! process dials, picks the site renderer, and mounts the document; the renderer
//! owns the document. `TabHandle` is the protocol surface and stays value-only.

use std::fmt;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use renderer::{
    Command as RendererCommand, FrameId, Mount, RemoteValue, Reply, ResourceLimit, TabError,
    TabEvent,
};
use url::Url;

use crate::link::{RendererFactory, RendererHandle};
use crate::network::{FetchHandle, NavOutcome};
use crate::site::Site;

const EVENT_SUBSCRIBER_CAPACITY: usize = 256;
const COMMAND_CAPACITY: usize = 256;
const MAX_WAITERS: usize = 256;
const MAX_SUBSCRIBERS: usize = 256;

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
    RunUntilLoadTimeout {
        timeout: Duration,
        reply: Sender<Result<bool, TabError>>,
    },
    RunUntilJsTrue {
        source: String,
        timeout: Duration,
        reply: Sender<Result<bool, TabError>>,
    },
    DocumentUrl {
        reply: Sender<String>,
    },
    Subscribe {
        reply: Sender<Result<Receiver<TabEvent>, TabError>>,
    },
    LastNavigationFailed {
        reply: Sender<bool>,
    },
    Shutdown {
        reply: Sender<()>,
    },
}

enum Waiter {
    LoadTimeout {
        deadline: Instant,
        reply: Sender<Result<bool, TabError>>,
    },
    JsTrue {
        source: String,
        deadline: Instant,
        reply: Sender<Result<bool, TabError>>,
    },
}

/// Value-only handle to one [`TabActor`].
#[derive(Clone)]
pub struct TabHandle {
    id: TabId,
    current: Arc<Mutex<Option<Arc<RendererHandle>>>>,
    tx: SyncSender<Command>,
}

impl TabHandle {
    /// Tab identity in the owning browser.
    #[must_use]
    pub fn id(&self) -> TabId {
        self.id
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
    /// [`TabHandle::run_until_load_timeout`] only when the caller needs to wait.
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

    /// Waits until the current navigation has fired `load`, returning `false`
    /// on timeout.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`].
    pub fn run_until_load_timeout(&self, timeout: Duration) -> Result<bool, TabError> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::RunUntilLoadTimeout { timeout, reply })?;
        recv_result(&rx)
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
        recv_result(&rx)
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

    /// Subscribes to tab events emitted after this call.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`] when the actor has shut down.
    pub fn subscribe(&self) -> Result<Receiver<TabEvent>, TabError> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::Subscribe { reply })?;
        recv_result(&rx)
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
        self.tx.send(command).map_err(|_| TabError::ActorStopped)
    }
}

fn recv_result<T>(rx: &Receiver<Result<T, TabError>>) -> Result<T, TabError> {
    rx.recv().unwrap_or(Err(TabError::ActorStopped))
}

fn recv_bool(rx: &Receiver<bool>) -> Result<bool, TabError> {
    rx.recv().map_err(|_| TabError::ActorStopped)
}

fn recv_text(rx: &Receiver<String>) -> Result<String, TabError> {
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
        let _ = self.handle.tx.send(Command::Shutdown { reply });
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
    document_loaded: bool,
    nav_epoch: u64,
    nav_in_flight: Option<u64>,
    navigation_failed: bool,
    nav: Option<ActiveNavigation>,
    dial_tx: Sender<(u64, Result<NavOutcome, ()>)>,
    dial_rx: Receiver<(u64, Result<NavOutcome, ()>)>,
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
            document_loaded: false,
            nav_epoch: 0,
            nav_in_flight: None,
            navigation_failed: false,
            nav: None,
            dial_tx,
            dial_rx,
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
                let mounted = self.commit_navigation(outcome);
                self.record_event(if mounted {
                    TabEvent::Navigated
                } else {
                    TabEvent::NavigationFailed
                });
            } else {
                self.record_event(TabEvent::FetchFailed);
                self.navigation_failed = true;
                self.record_event(TabEvent::NavigationFailed);
            }
        }
    }

    /// Returns true when the new document mounted.
    fn commit_navigation(&mut self, outcome: NavOutcome) -> bool {
        let site = Site::for_url(&outcome.final_url).unwrap_or_else(|| Site::opaque(self.id));
        self.document_url = outcome.final_url.clone();
        let mount = Mount {
            url: outcome.final_url.to_string(),
            content_type: outcome.content_type,
            content_language: outcome.content_language,
            body: outcome.body,
        };
        if self.mount(&site, mount).is_err() {
            self.navigation_failed = true;
            return false;
        }
        true
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
                self.record_event(event);
            } else if event == TabEvent::Load {
                self.record_event(TabEvent::ChildLoad);
            } else {
                self.record_event(event);
            }
        }
    }

    fn record_event(&mut self, event: TabEvent) {
        self.subscribers
            .retain(|subscriber| matches!(subscriber.try_send(event), Ok(())));
    }

    fn busy(&self) -> bool {
        self.nav.is_some()
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

fn actor_loop(rx: &Receiver<Command>, mut tab: Tab) {
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
        let command = match received {
            Ok(command) => command,
            Err(RecvTimeoutError::Timeout) => {
                resolve_waiters(&mut tab, &mut waiters);
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        if handle_command(&mut tab, command, &mut waiters) {
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
        Command::RunUntilLoadTimeout { timeout, reply } => {
            retain_waiter(
                waiters,
                Waiter::LoadTimeout {
                    deadline: Instant::now() + timeout,
                    reply,
                },
            );
        }
        Command::RunUntilJsTrue {
            source,
            timeout,
            reply,
        } => {
            retain_waiter(
                waiters,
                Waiter::JsTrue {
                    source,
                    deadline: Instant::now() + timeout,
                    reply,
                },
            );
        }
        Command::DocumentUrl { reply } => {
            let _ = reply.send(tab.document_url.to_string());
        }
        Command::Subscribe { reply } => {
            if tab.subscribers.len() == MAX_SUBSCRIBERS {
                let _ = reply.send(Err(TabError::ResourceLimit {
                    resource: ResourceLimit::TabSubscribers,
                }));
            } else {
                let (events, event_rx) = mpsc::sync_channel(EVENT_SUBSCRIBER_CAPACITY);
                tab.subscribers.push(events);
                let _ = reply.send(Ok(event_rx));
            }
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

fn retain_waiter(waiters: &mut Vec<Waiter>, waiter: Waiter) {
    if waiters.len() < MAX_WAITERS {
        waiters.push(waiter);
        return;
    }
    let error = TabError::ResourceLimit {
        resource: ResourceLimit::TabWaiters,
    };
    match waiter {
        Waiter::LoadTimeout { reply, .. } | Waiter::JsTrue { reply, .. } => {
            let _ = reply.send(Err(error));
        }
    }
}

fn resolve_waiters(tab: &mut Tab, waiters: &mut Vec<Waiter>) {
    let now = Instant::now();
    let mut pending = Vec::new();
    for waiter in std::mem::take(waiters) {
        match waiter {
            Waiter::LoadTimeout { reply, .. } if !tab.waiting_for_load() => {
                let _ = reply.send(Ok(true));
            }
            Waiter::LoadTimeout { deadline, reply } if now >= deadline => {
                let _ = reply.send(Ok(false));
            }
            Waiter::JsTrue {
                source,
                deadline,
                reply,
            } => {
                if now >= deadline {
                    let _ = reply.send(Ok(false));
                } else if matches!(
                    tab.renderer_request(RendererCommand::ExecuteScript {
                        frame: FrameId::MAIN,
                        source: source.clone(),
                        timeout_ms: None
                    }),
                    Ok(Reply::Value(Ok(RemoteValue::Bool(true))))
                ) {
                    let _ = reply.send(Ok(true));
                } else {
                    pending.push(Waiter::JsTrue {
                        source,
                        deadline,
                        reply,
                    });
                }
            }
            other @ Waiter::LoadTimeout { .. } => pending.push(other),
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
