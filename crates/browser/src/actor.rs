//! One async browser-side tab coordinator: identity, navigation, and renderer link.
//!
//! The browser process owns tabs as Tokio tasks. The renderer owns each
//! document.

use std::collections::{HashSet, VecDeque};
use std::convert::Infallible;
use std::fmt;
use std::future::pending;
use std::sync::Arc;
use std::time::Duration;

use crate::wire::{Command as RendererCommand, Reply};
use renderer::{DialFailure, FrameId, Mount, RemoteValue, RendererEvent, ResourceLimit, TabError};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep_until, timeout};
use url::Url;

use crate::exchange::{self, RequestId, ServerInput};
use crate::link::{
    AssignmentContext, AssignmentMountOptions, AssignmentStartResponseOptions, RendererAssignment,
};
use crate::manager::RendererProcessManager;
use crate::network::{NAV_BODY_LIMIT, NavOutcome, TabNetworkHandle, dial_failure};
use crate::site::Site;

const EVENT_SUBSCRIBER_CAPACITY: usize = 256;
const COMMAND_CAPACITY: usize = 256;
const MAX_WAITERS: usize = 256;
const MAX_SUBSCRIBERS: usize = 256;
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

/// Identity of one tab in a [`crate::Browser`] registry.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct TabId(u64);

/// Observable lifecycle transitions for one tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabEvent {
    /// A navigation committed its final URL.
    Navigated,
    /// The top-level document fired `load`.
    Load,
    /// A navigation failed and left the previous document active.
    NavigationFailed,
}

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
    },
    Goto {
        url: String,
    },
    Execute {
        frame: FrameId,
        source: String,
        timeout: Option<Duration>,
    },
    Screenshot {
        frame: FrameId,
        request: renderer::ScreenshotRequest,
    },
    RunUntilLoadTimeout {
        timeout: Duration,
    },
    RunUntilJsTrue {
        frame: FrameId,
        source: String,
        timeout: Duration,
    },
    /// Routes one `postMessage` from another tab into this tab's window.
    WindowMessage {
        payload: String,
    },
    DocumentUrl,
    LastNavigationFailed,
    Shutdown,
}

enum TabReply {
    LoadHtml(Result<(), TabError>),
    Goto(Result<(), TabError>),
    Execute(Result<RemoteValue, TabError>),
    Screenshot(Result<Vec<u8>, TabError>),
    RunUntilLoad(Result<bool, TabError>),
    RunUntilJs(Result<bool, TabError>),
    WindowMessage(Result<(), TabError>),
    DocumentUrl(String),
    LastNavigationFailed(bool),
    Shutdown,
}

type TabClient = exchange::Client<Command, Infallible, Infallible, TabReply, TabEvent>;
type TabServer = exchange::Server<Infallible, TabReply, TabEvent, Command, Infallible>;
type TabNotifier = exchange::Notifier<Infallible, TabReply, TabEvent>;

/// One pending `RunUntilLoadTimeout` (`source` is `None`) or `RunUntilJsTrue`
/// (`source` holds the predicate) wait.
struct Waiter {
    id: RequestId,
    deadline: Instant,
    source: Option<(FrameId, String)>,
}

/// Async value-only handle to one tab coordinator.
#[derive(Clone)]
pub struct TabHandle {
    id: TabId,
    client: TabClient,
}

/// One framed script evaluation requested through a [`TabHandle`].
pub struct ExecuteScriptInOptions<'a> {
    /// Frame to evaluate in.
    pub frame: FrameId,
    /// Script source.
    pub source: &'a str,
    /// Optional `QuickJS` interruption timeout.
    pub timeout: Option<Duration>,
}

/// One framed JS-truth wait requested through a [`TabHandle`].
pub struct RunUntilJsTrueInOptions<'a> {
    /// Frame to evaluate in.
    pub frame: FrameId,
    /// Predicate source; waits until it evaluates to JS `true`.
    pub source: &'a str,
    /// Maximum time to wait.
    pub timeout: Duration,
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
        match self
            .call(Command::LoadHtml {
                html: html.to_owned(),
            })
            .await?
        {
            TabReply::LoadHtml(result) => result,
            _ => Err(TabError::ActorStopped),
        }
    }

    /// Starts navigation. The tab continues independently; call
    /// [`TabHandle::run_until_load_timeout`] only when the caller needs to wait.
    ///
    /// # Errors
    ///
    /// [`TabError::InvalidUrl`] or [`TabError::ActorStopped`].
    pub async fn goto(&self, url: &str) -> Result<(), TabError> {
        match self
            .call(Command::Goto {
                url: url.to_owned(),
            })
            .await?
        {
            TabReply::Goto(result) => result,
            _ => Err(TabError::ActorStopped),
        }
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
        self.execute_script_in(ExecuteScriptInOptions {
            frame: FrameId::MAIN,
            source,
            timeout,
        })
        .await
    }

    /// Evaluates `source` in one frame, interrupting `QuickJS` if `timeout`
    /// elapses.
    ///
    /// # Errors
    ///
    /// [`TabError::UnknownFrame`], [`TabError::Script`], or
    /// [`TabError::ActorStopped`].
    pub async fn execute_script_in(
        &self,
        options: ExecuteScriptInOptions<'_>,
    ) -> Result<RemoteValue, TabError> {
        let ExecuteScriptInOptions {
            frame,
            source,
            timeout,
        } = options;
        match self
            .call(Command::Execute {
                frame,
                source: source.to_owned(),
                timeout,
            })
            .await?
        {
            TabReply::Execute(result) => result,
            _ => Err(TabError::ActorStopped),
        }
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
        self.screenshot_frame(FrameId::MAIN, request).await
    }

    /// Renders one frame to a PNG.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`], [`TabError::UnknownFrame`], or
    /// [`TabError::Render`] when the pipeline refuses the document.
    pub async fn screenshot_frame(
        &self,
        frame: FrameId,
        request: renderer::ScreenshotRequest,
    ) -> Result<Vec<u8>, TabError> {
        match self.call(Command::Screenshot { frame, request }).await? {
            TabReply::Screenshot(result) => result,
            _ => Err(TabError::ActorStopped),
        }
    }

    /// Waits until the current navigation has fired `load`, returning `false`
    /// on timeout.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`].
    pub async fn run_until_load_timeout(&self, timeout: Duration) -> Result<bool, TabError> {
        match self.call(Command::RunUntilLoadTimeout { timeout }).await? {
            TabReply::RunUntilLoad(result) => result,
            _ => Err(TabError::ActorStopped),
        }
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
        self.run_until_js_true_in(RunUntilJsTrueInOptions {
            frame: FrameId::MAIN,
            source,
            timeout,
        })
        .await
    }

    /// Waits until `source` evaluates to JS `true` in `frame`, returning
    /// `false` on timeout.
    ///
    /// # Errors
    ///
    /// [`TabError::UnknownFrame`] or [`TabError::ActorStopped`].
    pub async fn run_until_js_true_in(
        &self,
        options: RunUntilJsTrueInOptions<'_>,
    ) -> Result<bool, TabError> {
        let RunUntilJsTrueInOptions {
            frame,
            source,
            timeout,
        } = options;
        match self
            .call(Command::RunUntilJsTrue {
                frame,
                source: source.to_owned(),
                timeout,
            })
            .await?
        {
            TabReply::RunUntilJs(result) => result,
            _ => Err(TabError::ActorStopped),
        }
    }

    /// Document URL after navigation.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`].
    pub async fn document_url(&self) -> Result<String, TabError> {
        match self.call(Command::DocumentUrl).await? {
            TabReply::DocumentUrl(url) => Ok(url),
            _ => Err(TabError::ActorStopped),
        }
    }

    /// Subscribes to tab events emitted after this call.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`] when the coordinator has shut down.
    /// [`TabError::ResourceLimit`] when too many subscribers are already live.
    pub fn subscribe(&self) -> Result<mpsc::Receiver<TabEvent>, TabError> {
        self.client
            .subscribe(EVENT_SUBSCRIBER_CAPACITY, MAX_SUBSCRIBERS)
            .map_err(|error| match error {
                exchange::Error::SubscriberLimit => TabError::ResourceLimit {
                    resource: ResourceLimit::TabSubscribers,
                },
                exchange::Error::Closed | exchange::Error::Protocol => TabError::ActorStopped,
            })
    }

    /// True when the last navigation dial failed.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`].
    pub async fn last_navigation_failed(&self) -> Result<bool, TabError> {
        match self.call(Command::LastNavigationFailed).await? {
            TabReply::LastNavigationFailed(failed) => Ok(failed),
            _ => Err(TabError::ActorStopped),
        }
    }

    /// Delivers one remote `window` `message` payload into this tab.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`] when the coordinator or renderer is gone.
    pub async fn window_message(&self, payload: String) -> Result<(), TabError> {
        match self.call(Command::WindowMessage { payload }).await? {
            TabReply::WindowMessage(result) => result,
            _ => Err(TabError::ActorStopped),
        }
    }

    async fn call(&self, command: Command) -> Result<TabReply, TabError> {
        self.client
            .call(command)
            .await
            .map_err(|_| TabError::ActorStopped)
    }
}

/// Join handle and command sender for one tab coordinator task.
pub(crate) struct TabTask {
    pub handle: TabHandle,
    join: Option<JoinHandle<()>>,
}

/// Dependencies one [`TabTask`] coordinator starts with.
pub(crate) struct SpawnOptions {
    /// Tab identity to coordinate.
    pub(crate) id: TabId,
    /// Network capability for the tab.
    pub(crate) network: TabNetworkHandle,
    /// Renderer pool the tab acquires from.
    pub(crate) renderers: Arc<RendererProcessManager>,
}

impl TabTask {
    pub(crate) fn spawn(options: SpawnOptions) -> Self {
        let SpawnOptions {
            id,
            network,
            renderers,
        } = options;
        let (client, server) = exchange::local(COMMAND_CAPACITY);
        let handle = TabHandle { id, client };
        let tab = Tab::new(TabOptions {
            id,
            network,
            renderers,
            events: server.notifier(),
        });
        let join = tokio::spawn(coordinator_loop(server, tab));
        Self {
            handle,
            join: Some(join),
        }
    }

    pub(crate) async fn shutdown(&mut self) {
        let _result = timeout(SHUTDOWN_TIMEOUT, self.handle.call(Command::Shutdown)).await;
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
    network: TabNetworkHandle,
    renderer: Option<Arc<RendererAssignment>>,
    pending_mount: Option<Mount>,
    site: Option<Site>,
    events_rx: Option<mpsc::Receiver<(FrameId, RendererEvent)>>,
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
    events: TabNotifier,
}

/// Inputs for constructing one [`Tab`].
struct TabOptions {
    id: TabId,
    network: TabNetworkHandle,
    renderers: Arc<RendererProcessManager>,
    events: TabNotifier,
}

/// One completed document mount into the tab's renderer.
struct MountOptions<'a> {
    site: &'a Site,
    status: u16,
    mount: Mount,
}

/// One streamed response mount into the tab's renderer.
struct MountStreamOptions<'a> {
    site: &'a Site,
    status: u16,
    mount: Mount,
    body: net::Body,
}

impl Tab {
    fn new(options: TabOptions) -> Self {
        let TabOptions {
            id,
            network,
            renderers,
            events,
        } = options;
        let (dial_tx, dial_rx) = mpsc::unbounded_channel();
        Self {
            id,
            renderers,
            network,
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
            events,
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
            self.record_event(TabEvent::Load).await;
            return Ok(());
        }
        self.mount(MountOptions {
            site: &site,
            status: 200,
            mount,
        })
        .await
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
            .acquire(AssignmentContext {
                tab: self.id,
                site: site.clone(),
                network: self.network.clone(),
            })
            .await
            .map_err(|error| renderer_unavailable(&error.to_string()))?;
        self.drop_renderer().await;
        self.events_rx = Some(handle.subscribe());
        self.site = Some(site.clone());
        self.renderer = Some(handle);
        Ok(())
    }

    async fn mount(&mut self, options: MountOptions<'_>) -> Result<(), TabError> {
        let MountOptions {
            site,
            status,
            mount,
        } = options;
        self.ensure_renderer(site).await?;
        self.pending_mount = None;
        self.document_loaded = false;
        let result = self
            .renderer
            .as_ref()
            .ok_or(TabError::ActorStopped)?
            .mount(AssignmentMountOptions {
                frame: FrameId::MAIN,
                status,
                mount,
            })
            .await
            .and_then(reply_unit);
        if result.is_err() {
            self.drop_renderer().await;
        }
        result
    }

    async fn mount_stream(&mut self, options: MountStreamOptions<'_>) -> Result<(), TabError> {
        let MountStreamOptions {
            site,
            status,
            mount,
            mut body,
        } = options;
        self.ensure_renderer(site).await?;
        self.ensure_renderer(site).await?;
        self.pending_mount = None;
        self.document_loaded = false;
        let renderer = self.renderer.as_ref().ok_or(TabError::ActorStopped)?;
        let stream = renderer
            .start_response(AssignmentStartResponseOptions {
                frame: FrameId::MAIN,
                status,
                mount: &mount,
            })
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

    /// Routes one remote `postMessage` payload into this tab's main frame.
    async fn deliver_window_message(&mut self, payload: String) -> Result<(), TabError> {
        if self.renderer.is_none() {
            self.mount_virtual().await?;
        }
        match self
            .renderer_request(RendererCommand::WindowMessage { payload })
            .await?
        {
            Reply::Unit(result) => result,
            _ => Err(TabError::ActorStopped),
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
        self.mount(MountOptions {
            site: &site,
            status: 200,
            mount,
        })
        .await
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
        let guard = self
            .network
            .dial_navigation(crate::network::DialNavigationOptions {
                epoch,
                url,
                initiator,
                reply: self.dial_tx.clone(),
                cancel: cancel_rx,
            });
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
            let mounted = self.commit_navigation(outcome).await;
            self.record_event(if mounted {
                TabEvent::Navigated
            } else {
                TabEvent::NavigationFailed
            })
            .await;
        } else {
            self.navigation_failed = true;
            self.record_event(TabEvent::NavigationFailed).await;
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
            .mount_stream(MountStreamOptions {
                site: &site,
                status: outcome.status,
                mount,
                body: outcome.body,
            })
            .await
            .is_err()
        {
            self.navigation_failed = true;
            return false;
        }
        true
    }

    async fn handle_renderer_event(&mut self, frame: FrameId, event: RendererEvent) {
        if frame == FrameId::MAIN && event == RendererEvent::Load {
            self.document_loaded = true;
            self.record_event(TabEvent::Load).await;
        }
    }

    async fn record_event(&self, event: TabEvent) {
        let _result = self.events.notify(event).await;
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
    Command(Option<ServerInput<Command, Infallible>>),
    Navigation(Option<(u64, Result<NavOutcome, DialFailure>)>),
    Renderer(Option<(FrameId, RendererEvent)>),
    WaiterDeadline,
}

async fn coordinator_loop(mut server: TabServer, mut tab: Tab) {
    let mut waiters = Vec::new();
    let mut cancelled = HashSet::new();
    let mut cancelled_order = VecDeque::new();
    loop {
        tab.launch_navigation();
        let deadline = next_waiter_deadline(&waiters);
        let wake = {
            let dial_rx = &mut tab.dial_rx;
            let events_rx = &mut tab.events_rx;
            tokio::select! {
                command = server.recv() => Wake::Command(command),
                navigation = dial_rx.recv() => Wake::Navigation(navigation),
                event = next_renderer_event(events_rx) => Wake::Renderer(event),
                () = wait_for_deadline(deadline) => Wake::WaiterDeadline,
            }
        };
        match wake {
            Wake::Command(Some(ServerInput::Call { id, body })) => {
                if cancelled.remove(&id) {
                    continue;
                }
                if handle_command(HandleCommandOptions {
                    tab: &mut tab,
                    server: &server,
                    id,
                    command: body,
                    waiters: &mut waiters,
                })
                .await
                {
                    return;
                }
            }
            Wake::Command(Some(ServerInput::Cancel { id })) => {
                let before = waiters.len();
                waiters.retain(|waiter| waiter.id != id);
                if waiters.len() == before {
                    remember_cancelled(RememberCancelledOptions {
                        cancelled: &mut cancelled,
                        order: &mut cancelled_order,
                        id,
                    });
                }
            }
            Wake::Command(Some(
                ServerInput::RequestChunk { id } | ServerInput::RequestEnd { id },
            )) => {
                cancelled.remove(&id);
            }
            Wake::Command(Some(ServerInput::Notify(_)) | None) => break,
            Wake::Navigation(Some((epoch, result))) => {
                tab.handle_navigation(epoch, result).await;
            }
            Wake::Navigation(None) | Wake::WaiterDeadline => {}
            Wake::Renderer(Some((frame, event))) => tab.handle_renderer_event(frame, event).await,
            Wake::Renderer(None) => {
                tab.drop_renderer().await;
                fail_waiters(FailWaitersOptions {
                    server: &server,
                    waiters: &mut waiters,
                    error: &TabError::ActorStopped,
                })
                .await;
            }
        }
        resolve_waiters(ResolveWaitersOptions {
            tab: &mut tab,
            server: &server,
            waiters: &mut waiters,
        })
        .await;
    }
    tab.stop_renderer().await;
}

async fn next_renderer_event(
    events: &mut Option<mpsc::Receiver<(FrameId, RendererEvent)>>,
) -> Option<(FrameId, RendererEvent)> {
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

/// Bounded record of cancelled waiter ids.
struct RememberCancelledOptions<'a> {
    cancelled: &'a mut HashSet<RequestId>,
    order: &'a mut VecDeque<RequestId>,
    id: RequestId,
}

/// One coordinator command with its routing context.
struct HandleCommandOptions<'a> {
    tab: &'a mut Tab,
    server: &'a TabServer,
    id: RequestId,
    command: Command,
    waiters: &'a mut Vec<Waiter>,
}

/// One frame screenshot of the tab's live renderer.
struct ScreenshotFrameOptions<'a> {
    tab: &'a mut Tab,
    frame: FrameId,
    request: renderer::ScreenshotRequest,
}

/// One waiter registration.
struct RetainWaiterOptions<'a> {
    waiters: &'a mut Vec<Waiter>,
    waiter: Waiter,
    server: &'a TabServer,
}

/// One waiter failure broadcast.
struct FailWaitersOptions<'a> {
    server: &'a TabServer,
    waiters: &'a mut Vec<Waiter>,
    error: &'a TabError,
}

/// One waiter resolution pass over live tab state.
struct ResolveWaitersOptions<'a> {
    tab: &'a mut Tab,
    server: &'a TabServer,
    waiters: &'a mut Vec<Waiter>,
}

fn remember_cancelled(options: RememberCancelledOptions<'_>) {
    let RememberCancelledOptions {
        cancelled,
        order,
        id,
    } = options;
    if cancelled.insert(id) {
        order.push_back(id);
    }
    while cancelled.len() > MAX_WAITERS {
        let Some(oldest) = order.pop_front() else {
            break;
        };
        cancelled.remove(&oldest);
    }
}

/// Handles one command. `true` means the coordinator returns.
async fn handle_command(options: HandleCommandOptions<'_>) -> bool {
    let HandleCommandOptions {
        tab,
        server,
        id,
        command,
        waiters,
    } = options;
    match command {
        Command::LoadHtml { html } => {
            let _result = server
                .reply(id, TabReply::LoadHtml(tab.load_html(&html).await))
                .await;
        }
        Command::Goto { url } => {
            let _result = server.reply(id, TabReply::Goto(tab.goto(&url))).await;
        }
        Command::Execute {
            frame,
            source,
            timeout,
        } => {
            let result = tab
                .renderer_request(RendererCommand::ExecuteScript {
                    frame,
                    source,
                    timeout_ms: timeout.map(millis),
                })
                .await
                .and_then(reply_value);
            let _result = server.reply(id, TabReply::Execute(result)).await;
        }
        Command::Screenshot { frame, request } => {
            let result = screenshot_frame(ScreenshotFrameOptions {
                tab,
                frame,
                request,
            })
            .await;
            let _result = server.reply(id, TabReply::Screenshot(result)).await;
        }
        Command::RunUntilLoadTimeout { timeout } => {
            retain_waiter(RetainWaiterOptions {
                waiters,
                waiter: Waiter {
                    id,
                    deadline: Instant::now() + timeout,
                    source: None,
                },
                server,
            })
            .await;
        }
        Command::RunUntilJsTrue {
            frame,
            source,
            timeout,
        } => {
            retain_waiter(RetainWaiterOptions {
                waiters,
                waiter: Waiter {
                    id,
                    deadline: Instant::now() + timeout,
                    source: Some((frame, source)),
                },
                server,
            })
            .await;
        }
        Command::DocumentUrl => {
            let _result = server
                .reply(id, TabReply::DocumentUrl(tab.document_url.to_string()))
                .await;
        }
        Command::LastNavigationFailed => {
            let _result = server
                .reply(id, TabReply::LastNavigationFailed(tab.navigation_failed))
                .await;
        }
        Command::WindowMessage { payload } => {
            let result = tab.deliver_window_message(payload).await;
            let _result = server.reply(id, TabReply::WindowMessage(result)).await;
        }
        Command::Shutdown => {
            tab.stop_renderer().await;
            let _result = server.reply(id, TabReply::Shutdown).await;
            return true;
        }
    }
    false
}

async fn screenshot_frame(options: ScreenshotFrameOptions<'_>) -> Result<Vec<u8>, TabError> {
    let ScreenshotFrameOptions {
        tab,
        frame,
        request,
    } = options;
    tab.renderer_request_bytes(RendererCommand::Screenshot { frame, request })
        .await
}

async fn retain_waiter(options: RetainWaiterOptions<'_>) {
    let RetainWaiterOptions {
        waiters,
        waiter,
        server,
    } = options;
    if waiters.len() < MAX_WAITERS {
        waiters.push(waiter);
        return;
    }
    let error = TabError::ResourceLimit {
        resource: ResourceLimit::TabWaiters,
    };
    let reply = waiter_reply(&waiter, Err(error));
    let _result = server.reply(waiter.id, reply).await;
}

async fn fail_waiters(options: FailWaitersOptions<'_>) {
    let FailWaitersOptions {
        server,
        waiters,
        error,
    } = options;
    for waiter in waiters.drain(..) {
        let reply = waiter_reply(&waiter, Err(error.clone()));
        let _result = server.reply(waiter.id, reply).await;
    }
}

async fn resolve_waiters(options: ResolveWaitersOptions<'_>) {
    let ResolveWaitersOptions {
        tab,
        server,
        waiters,
    } = options;
    let now = Instant::now();
    let mut pending = Vec::new();
    for waiter in std::mem::take(waiters) {
        match waiter.source.clone() {
            // A load wait finishes as soon as the tab stops waiting, before
            // its deadline is considered.
            None if !tab.waiting_for_load() => {
                let _result = server
                    .reply(waiter.id, TabReply::RunUntilLoad(Ok(true)))
                    .await;
            }
            None if now >= waiter.deadline => {
                let _result = server
                    .reply(waiter.id, TabReply::RunUntilLoad(Ok(false)))
                    .await;
            }
            None => pending.push(waiter),
            Some((frame, source)) => {
                if now >= waiter.deadline {
                    let _result = server
                        .reply(waiter.id, TabReply::RunUntilJs(Ok(false)))
                        .await;
                } else if matches!(
                    tab.renderer_request(RendererCommand::ExecuteScript {
                        frame,
                        source,
                        timeout_ms: None
                    })
                    .await,
                    Ok(Reply::Value(Ok(RemoteValue::Bool(true))))
                ) {
                    let _result = server
                        .reply(waiter.id, TabReply::RunUntilJs(Ok(true)))
                        .await;
                } else {
                    pending.push(waiter);
                }
            }
        }
    }
    *waiters = pending;
}

fn waiter_reply(waiter: &Waiter, result: Result<bool, TabError>) -> TabReply {
    if waiter.source.is_some() {
        TabReply::RunUntilJs(result)
    } else {
        TabReply::RunUntilLoad(result)
    }
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
