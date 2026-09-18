//! One async browser-side tab coordinator: identity, navigation, and renderer link.
//!
//! The browser process owns tabs as Tokio tasks. The renderer owns each
//! document.

use std::fmt;
use std::future::pending;
use std::sync::Arc;
use std::time::Duration;

use crate::wire::{Command as RendererCommand, Reply};
use renderer::{DialFailure, FrameId, Mount, RemoteValue, ResourceLimit, TabError, TabEvent};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep_until, timeout};
use url::Url;

use crate::link::RendererAssignment;
use crate::manager::RendererProcessManager;
use crate::network::{FetchHandle, NAV_BODY_LIMIT, NavOutcome, dial_failure};
use crate::site::Site;

const EVENT_SUBSCRIBER_CAPACITY: usize = 256;
const COMMAND_CAPACITY: usize = 256;
const MAX_WAITERS: usize = 256;
const MAX_SUBSCRIBERS: usize = 256;
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

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
        reply: oneshot::Sender<Result<(), TabError>>,
    },
    Goto {
        url: String,
        reply: oneshot::Sender<Result<(), TabError>>,
    },
    Execute {
        source: String,
        timeout: Option<Duration>,
        reply: oneshot::Sender<Result<RemoteValue, TabError>>,
    },
    Screenshot {
        request: renderer::ScreenshotRequest,
        reply: oneshot::Sender<Result<Vec<u8>, TabError>>,
    },
    RunUntilLoadTimeout {
        timeout: Duration,
        reply: oneshot::Sender<Result<bool, TabError>>,
    },
    RunUntilJsTrue {
        source: String,
        timeout: Duration,
        reply: oneshot::Sender<Result<bool, TabError>>,
    },
    DocumentUrl {
        reply: oneshot::Sender<String>,
    },
    Subscribe {
        reply: oneshot::Sender<Result<mpsc::Receiver<TabEvent>, TabError>>,
    },
    LastNavigationFailed {
        reply: oneshot::Sender<bool>,
    },
    Shutdown {
        reply: oneshot::Sender<()>,
    },
}

/// One pending `RunUntilLoadTimeout` (`source` is `None`) or `RunUntilJsTrue`
/// (`source` holds the predicate) wait.
struct Waiter {
    deadline: Instant,
    source: Option<String>,
    reply: oneshot::Sender<Result<bool, TabError>>,
}

/// Async value-only handle to one tab coordinator.
#[derive(Clone)]
pub struct TabHandle {
    id: TabId,
    tx: mpsc::Sender<Command>,
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
    pub async fn load_html(&self, html: &str) -> Result<(), TabError> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::LoadHtml {
            html: html.to_owned(),
            reply,
        })
        .await?;
        recv_result(rx).await
    }

    /// Starts navigation. The tab continues independently; call
    /// [`TabHandle::run_until_load_timeout`] only when the caller needs to wait.
    ///
    /// # Errors
    ///
    /// [`TabError::InvalidUrl`] or [`TabError::ActorStopped`].
    pub async fn goto(&self, url: &str) -> Result<(), TabError> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::Goto {
            url: url.to_owned(),
            reply,
        })
        .await?;
        recv_result(rx).await
    }

    /// Evaluates `source` and returns a value-only script result.
    ///
    /// # Errors
    ///
    /// [`TabError::Script`] or [`TabError::ActorStopped`].
    pub async fn execute_script(&self, source: &str) -> Result<RemoteValue, TabError> {
        self.execute_script_timeout(source, None).await
    }

    /// Evaluates `source`, interrupting `QuickJS` if `timeout` elapses.
    ///
    /// # Errors
    ///
    /// [`TabError::Script`] or [`TabError::ActorStopped`].
    pub async fn execute_script_timeout(
        &self,
        source: &str,
        timeout: Option<Duration>,
    ) -> Result<RemoteValue, TabError> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::Execute {
            source: source.to_owned(),
            timeout,
            reply,
        })
        .await?;
        recv_result(rx).await
    }

    /// Renders the tab's top-level document to a PNG.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`], [`TabError::UnknownFrame`], or
    /// [`TabError::Render`] when the pipeline refuses the document.
    pub async fn screenshot(
        &self,
        request: renderer::ScreenshotRequest,
    ) -> Result<Vec<u8>, TabError> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::Screenshot { request, reply }).await?;
        recv_result(rx).await
    }

    /// Waits until the current navigation has fired `load`, returning `false`
    /// on timeout.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`].
    pub async fn run_until_load_timeout(&self, timeout: Duration) -> Result<bool, TabError> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::RunUntilLoadTimeout { timeout, reply })
            .await?;
        recv_result(rx).await
    }

    /// Waits until `source` evaluates to JS `true`, returning `false` on timeout.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`].
    pub async fn run_until_js_true(
        &self,
        source: &str,
        timeout: Duration,
    ) -> Result<bool, TabError> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::RunUntilJsTrue {
            source: source.to_owned(),
            timeout,
            reply,
        })
        .await?;
        recv_result(rx).await
    }

    /// Document URL after navigation.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`].
    pub async fn document_url(&self) -> Result<String, TabError> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::DocumentUrl { reply }).await?;
        rx.await.map_err(|_| TabError::ActorStopped)
    }

    /// Subscribes to tab events emitted after this call.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`] when the coordinator has shut down.
    pub async fn subscribe(&self) -> Result<mpsc::Receiver<TabEvent>, TabError> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::Subscribe { reply }).await?;
        recv_result(rx).await
    }

    /// True when the last navigation dial failed.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`].
    pub async fn last_navigation_failed(&self) -> Result<bool, TabError> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::LastNavigationFailed { reply }).await?;
        rx.await.map_err(|_| TabError::ActorStopped)
    }

    /// Queues one command for the coordinator task.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`] when the coordinator has stopped.
    async fn send(&self, command: Command) -> Result<(), TabError> {
        self.tx
            .send(command)
            .await
            .map_err(|_| TabError::ActorStopped)
    }
}

async fn recv_result<T>(rx: oneshot::Receiver<Result<T, TabError>>) -> Result<T, TabError> {
    rx.await.unwrap_or(Err(TabError::ActorStopped))
}

/// Join handle and command sender for one tab coordinator task.
pub(crate) struct TabTask {
    pub handle: TabHandle,
    join: Option<JoinHandle<()>>,
}

impl TabTask {
    pub(crate) fn spawn(
        id: TabId,
        fetch: FetchHandle,
        renderers: Arc<RendererProcessManager>,
    ) -> Self {
        let (tx, rx) = mpsc::channel(COMMAND_CAPACITY);
        let handle = TabHandle { id, tx };
        let tab = Tab::new(id, fetch, renderers);
        let join = tokio::spawn(coordinator_loop(rx, tab));
        Self {
            handle,
            join: Some(join),
        }
    }

    pub(crate) async fn shutdown(&mut self) {
        let (reply, rx) = oneshot::channel();
        let sent = timeout(
            SHUTDOWN_TIMEOUT,
            self.handle.tx.send(Command::Shutdown { reply }),
        )
        .await;
        if matches!(sent, Ok(Ok(()))) {
            let _result = timeout(SHUTDOWN_TIMEOUT, rx).await;
        }
        if let Some(mut join) = self.join.take()
            && timeout(SHUTDOWN_TIMEOUT, &mut join).await.is_err()
        {
            join.abort();
            let _result = join.await;
        }
    }
}

impl Drop for TabTask {
    fn drop(&mut self) {
        if let Some(join) = self.join.take() {
            join.abort();
        }
    }
}

struct ActiveNavigation {
    url: Url,
    initiator: Url,
    epoch: u64,
    submitted: bool,
}

/// Browser-owned tab state: identity, URL, navigation, and renderer link.
struct Tab {
    id: TabId,
    renderers: Arc<RendererProcessManager>,
    fetch: FetchHandle,
    renderer: Option<Arc<RendererAssignment>>,
    pending_mount: Option<Mount>,
    site: Option<Site>,
    events_rx: Option<mpsc::Receiver<(FrameId, TabEvent)>>,
    document_url: Url,
    document_loaded: bool,
    navigation_failed: bool,
    nav: Option<ActiveNavigation>,
    /// Monotonic per tab, never derived from `nav`: see `goto`.
    nav_epoch: u64,
    dial_tx: mpsc::UnboundedSender<(u64, Result<NavOutcome, DialFailure>)>,
    dial_rx: mpsc::UnboundedReceiver<(u64, Result<NavOutcome, DialFailure>)>,
    dial_cancel: Option<watch::Sender<bool>>,
    dial_guard: Option<JoinHandle<()>>,
    subscribers: Vec<mpsc::Sender<TabEvent>>,
}

impl Tab {
    fn new(id: TabId, fetch: FetchHandle, renderers: Arc<RendererProcessManager>) -> Self {
        let (dial_tx, dial_rx) = mpsc::unbounded_channel();
        Self {
            id,
            renderers,
            fetch,
            renderer: None,
            pending_mount: Some(blank_mount()),
            site: None,
            events_rx: None,
            document_url: Url::parse("about:blank").expect("about:blank is a valid URL"),
            document_loaded: false,
            navigation_failed: false,
            nav: None,
            nav_epoch: 0,
            dial_tx,
            dial_rx,
            dial_cancel: None,
            dial_guard: None,
            subscribers: Vec::new(),
        }
    }

    async fn load_html(&mut self, html: &str) -> Result<(), TabError> {
        let site = Site::for_url(&self.document_url)
            .or_else(|| self.site.clone())
            .unwrap_or_else(|| Site::opaque(self.id));
        let mount = Mount {
            url: self.document_url.to_string(),
            content_type: Some("text/html; charset=utf-8".to_owned()),
            content_language: None,
            body: html.as_bytes().to_vec(),
        };
        if self.renderer.is_none() && self.document_url.scheme() == "about" {
            self.pending_mount = Some(mount);
            self.document_loaded = true;
            self.record_event(TabEvent::Load);
            return Ok(());
        }
        self.mount(&site, 200, mount).await
    }

    fn goto(&mut self, spec: &str) -> Result<(), TabError> {
        let url = self.resolve_url(spec)?;
        // Monotonic across the tab's life, never derived from the live
        // navigation: `cancel_dial`'s abort is not a barrier, so a cancelled
        // dial's completion can still be enqueued after its successor
        // completes. A reused epoch would let that stale completion alias the
        // new navigation.
        self.nav_epoch = self.nav_epoch.saturating_add(1);
        let epoch = self.nav_epoch;
        self.navigation_failed = false;
        self.nav = Some(ActiveNavigation {
            url,
            initiator: self.document_url.clone(),
            epoch,
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

    async fn ensure_renderer(&mut self, site: &Site) -> Result<(), TabError> {
        if self.renderer.is_some() && self.site.as_ref() == Some(site) {
            return Ok(());
        }
        let handle = self
            .renderers
            .acquire(site)
            .await
            .map_err(|error| renderer_unavailable(&error.to_string()))?;
        self.drop_renderer().await;
        self.events_rx = Some(handle.subscribe());
        self.site = Some(site.clone());
        self.renderer = Some(handle);
        Ok(())
    }

    async fn mount(&mut self, site: &Site, status: u16, mount: Mount) -> Result<(), TabError> {
        self.ensure_renderer(site).await?;
        self.pending_mount = None;
        self.document_loaded = false;
        let result = self
            .renderer
            .as_ref()
            .ok_or(TabError::ActorStopped)?
            .mount(FrameId::MAIN, status, mount)
            .await
            .and_then(reply_unit);
        if result.is_err() {
            self.drop_renderer().await;
        }
        result
    }

    async fn mount_stream(
        &mut self,
        site: &Site,
        status: u16,
        mount: Mount,
        mut body: net::Body,
    ) -> Result<(), TabError> {
        self.ensure_renderer(site).await?;
        self.pending_mount = None;
        self.document_loaded = false;
        let renderer = self.renderer.as_ref().ok_or(TabError::ActorStopped)?;
        let stream = renderer
            .start_response(FrameId::MAIN, status, &mount)
            .await?;
        let mut received = 0usize;
        loop {
            match body.read_chunk().await {
                Ok(Some(chunk)) => {
                    received = received.saturating_add(chunk.len());
                    if received > NAV_BODY_LIMIT {
                        let _ = stream.abort(DialFailure::Limit).await;
                        self.drop_renderer().await;
                        return Err(renderer_unavailable("navigation body exceeds limit"));
                    }
                    if let Err(error) = stream.write(chunk).await {
                        drop(stream);
                        self.drop_renderer().await;
                        return Err(error);
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    let failure = dial_failure(&error);
                    let _ = stream.abort(failure).await;
                    self.drop_renderer().await;
                    return Err(renderer_unavailable(&error.to_string()));
                }
            }
        }
        let result = stream.finish().await.and_then(reply_unit);
        if result.is_err() {
            self.drop_renderer().await;
        }
        result
    }

    async fn drop_renderer(&mut self) {
        self.site = None;
        self.events_rx = None;
        if let Some(renderer) = self.renderer.take() {
            self.renderers.release(renderer).await;
        }
    }

    async fn renderer_request(&mut self, command: RendererCommand) -> Result<Reply, TabError> {
        if self.renderer.is_none() {
            self.mount_virtual().await?;
        }
        let Some(renderer) = self.renderer.clone() else {
            return Err(TabError::ActorStopped);
        };
        renderer.request(command).await
    }

    /// One streamed byte request (screenshots) against the tab's renderer,
    /// mounting the virtual blank document when the tab has none.
    async fn renderer_request_bytes(
        &mut self,
        command: RendererCommand,
    ) -> Result<Vec<u8>, TabError> {
        if self.renderer.is_none() {
            self.mount_virtual().await?;
        }
        let Some(renderer) = self.renderer.clone() else {
            return Err(TabError::ActorStopped);
        };
        renderer.request_bytes(command).await
    }

    /// Mounts the virtual blank document this tab has been carrying.
    async fn mount_virtual(&mut self) -> Result<(), TabError> {
        let mount = self.pending_mount.take().unwrap_or_else(blank_mount);
        let site = Site::for_url(&self.document_url)
            .or_else(|| self.site.clone())
            .unwrap_or_else(|| Site::opaque(self.id));
        self.mount(&site, 200, mount).await
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
        self.cancel_dial();
        let (cancel, cancel_rx) = watch::channel(false);
        let guard =
            self.fetch
                .dial_navigation(epoch, url, initiator, self.dial_tx.clone(), cancel_rx);
        self.dial_cancel = Some(cancel);
        self.dial_guard = Some(guard);
        if let Some(nav) = self.nav.as_mut() {
            nav.submitted = true;
        }
    }

    /// Cancels the in-flight dial: signals the token and aborts the task.
    fn cancel_dial(&mut self) {
        if let Some(cancel) = self.dial_cancel.take() {
            let _ = cancel.send(true);
        }
        if let Some(guard) = self.dial_guard.take() {
            guard.abort();
        }
    }

    async fn handle_navigation(&mut self, epoch: u64, result: Result<NavOutcome, DialFailure>) {
        let active = self.nav.as_ref().is_some_and(|nav| nav.epoch == epoch);
        if !active {
            return;
        }
        self.dial_cancel = None;
        self.dial_guard = None;
        self.nav = None;
        if let Ok(outcome) = result {
            self.record_event(TabEvent::Fetch {
                status: outcome.status,
            });
            let mounted = self.commit_navigation(outcome).await;
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

    /// Returns true when the new document mounted.
    async fn commit_navigation(&mut self, outcome: NavOutcome) -> bool {
        let site = Site::for_url(&outcome.final_url).unwrap_or_else(|| Site::opaque(self.id));
        self.document_url = outcome.final_url.clone();
        let mount = Mount {
            url: outcome.final_url.to_string(),
            content_type: outcome.content_type.clone(),
            content_language: outcome.content_language.clone(),
            body: Vec::new(),
        };
        if self
            .mount_stream(&site, outcome.status, mount, outcome.body)
            .await
            .is_err()
        {
            self.navigation_failed = true;
            return false;
        }
        true
    }

    fn handle_renderer_event(&mut self, frame: FrameId, event: TabEvent) {
        if frame == FrameId::MAIN && event == TabEvent::Load {
            self.document_loaded = true;
            self.record_event(event);
        } else if event == TabEvent::Load {
            self.record_event(TabEvent::ChildLoad);
        } else {
            self.record_event(event);
        }
    }

    fn record_event(&mut self, event: TabEvent) {
        self.subscribers
            .retain(|subscriber| subscriber.try_send(event).is_ok());
    }

    fn waiting_for_load(&self) -> bool {
        self.nav.is_some() || (!self.document_loaded && !self.navigation_failed)
    }

    async fn stop_renderer(&mut self) {
        self.cancel_dial();
        self.drop_renderer().await;
    }
}

fn blank_mount() -> Mount {
    Mount {
        url: "about:blank".into(),
        content_type: Some("text/html; charset=utf-8".into()),
        content_language: None,
        body: b"<!doctype html><title></title>".to_vec(),
    }
}

impl Drop for Tab {
    fn drop(&mut self) {
        self.cancel_dial();
    }
}

enum Wake {
    Command(Option<Command>),
    Navigation(Option<(u64, Result<NavOutcome, DialFailure>)>),
    Renderer(Option<(FrameId, TabEvent)>),
    WaiterDeadline,
}

async fn coordinator_loop(mut commands: mpsc::Receiver<Command>, mut tab: Tab) {
    let mut waiters = Vec::new();
    loop {
        tab.launch_navigation();
        let deadline = next_waiter_deadline(&waiters);
        let wake = {
            let dial_rx = &mut tab.dial_rx;
            let events_rx = &mut tab.events_rx;
            tokio::select! {
                command = commands.recv() => Wake::Command(command),
                navigation = dial_rx.recv() => Wake::Navigation(navigation),
                event = next_renderer_event(events_rx) => Wake::Renderer(event),
                () = wait_for_deadline(deadline) => Wake::WaiterDeadline,
            }
        };
        match wake {
            Wake::Command(Some(command)) => {
                if handle_command(&mut tab, command, &mut waiters).await {
                    return;
                }
            }
            Wake::Command(None) => break,
            Wake::Navigation(Some((epoch, result))) => {
                tab.handle_navigation(epoch, result).await;
            }
            Wake::Navigation(None) | Wake::WaiterDeadline => {}
            Wake::Renderer(Some((frame, event))) => tab.handle_renderer_event(frame, event),
            Wake::Renderer(None) => {
                tab.drop_renderer().await;
                fail_waiters(&mut waiters, &TabError::ActorStopped);
            }
        }
        resolve_waiters(&mut tab, &mut waiters).await;
    }
    tab.stop_renderer().await;
}

async fn next_renderer_event(
    events: &mut Option<mpsc::Receiver<(FrameId, TabEvent)>>,
) -> Option<(FrameId, TabEvent)> {
    match events {
        Some(receiver) => receiver.recv().await,
        None => pending().await,
    }
}

async fn wait_for_deadline(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => sleep_until(deadline).await,
        None => pending().await,
    }
}

fn next_waiter_deadline(waiters: &[Waiter]) -> Option<Instant> {
    waiters.iter().map(|waiter| waiter.deadline).min()
}

/// Handles one command. `true` means the coordinator returns.
async fn handle_command(tab: &mut Tab, command: Command, waiters: &mut Vec<Waiter>) -> bool {
    match command {
        Command::LoadHtml { html, reply } => {
            let _result = reply.send(tab.load_html(&html).await);
        }
        Command::Goto { url, reply } => {
            let _result = reply.send(tab.goto(&url));
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
                .await
                .and_then(reply_value);
            let _result = reply.send(result);
        }
        Command::Screenshot { request, reply } => {
            let result = tab
                .renderer_request_bytes(RendererCommand::Screenshot {
                    frame: FrameId::MAIN,
                    request,
                })
                .await;
            let _result = reply.send(result);
        }
        Command::RunUntilLoadTimeout { timeout, reply } => {
            retain_waiter(
                waiters,
                Waiter {
                    deadline: Instant::now() + timeout,
                    source: None,
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
                Waiter {
                    deadline: Instant::now() + timeout,
                    source: Some(source),
                    reply,
                },
            );
        }
        Command::DocumentUrl { reply } => {
            let _result = reply.send(tab.document_url.to_string());
        }
        Command::Subscribe { reply } => {
            if tab.subscribers.len() == MAX_SUBSCRIBERS {
                let _result = reply.send(Err(TabError::ResourceLimit {
                    resource: ResourceLimit::TabSubscribers,
                }));
            } else {
                let (events, event_rx) = mpsc::channel(EVENT_SUBSCRIBER_CAPACITY);
                tab.subscribers.push(events);
                let _result = reply.send(Ok(event_rx));
            }
        }
        Command::LastNavigationFailed { reply } => {
            let _result = reply.send(tab.navigation_failed);
        }
        Command::Shutdown { reply } => {
            tab.stop_renderer().await;
            let _result = reply.send(());
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
    let _result = waiter.reply.send(Err(error));
}

fn fail_waiters(waiters: &mut Vec<Waiter>, error: &TabError) {
    for waiter in waiters.drain(..) {
        let _result = waiter.reply.send(Err(error.clone()));
    }
}

async fn resolve_waiters(tab: &mut Tab, waiters: &mut Vec<Waiter>) {
    let now = Instant::now();
    let mut pending = Vec::new();
    for waiter in std::mem::take(waiters) {
        match waiter.source.clone() {
            // A load wait finishes as soon as the tab stops waiting, before
            // its deadline is considered.
            None if !tab.waiting_for_load() => {
                let _result = waiter.reply.send(Ok(true));
            }
            None if now >= waiter.deadline => {
                let _result = waiter.reply.send(Ok(false));
            }
            None => pending.push(waiter),
            Some(source) => {
                if now >= waiter.deadline {
                    let _result = waiter.reply.send(Ok(false));
                } else if matches!(
                    tab.renderer_request(RendererCommand::ExecuteScript {
                        frame: FrameId::MAIN,
                        source,
                        timeout_ms: None
                    })
                    .await,
                    Ok(Reply::Value(Ok(RemoteValue::Bool(true))))
                ) {
                    let _result = waiter.reply.send(Ok(true));
                } else {
                    pending.push(waiter);
                }
            }
        }
    }
    *waiters = pending;
}

fn renderer_unavailable(message: &str) -> TabError {
    TabError::RendererUnavailable {
        message: message.to_owned(),
    }
}

fn reply_unit(reply: Reply) -> Result<(), TabError> {
    match reply {
        Reply::Unit(result) => result,
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
