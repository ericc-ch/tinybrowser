//! One async browser-side tab coordinator: identity, navigation, and renderer link.
//!
//! The browser process owns tabs as Tokio tasks. The renderer owns each
//! document.

use std::convert::Infallible;
use std::fmt;
use std::future::pending;
use std::sync::Arc;
use std::time::Duration;

use crate::wire::{Command as RendererCommand, Reply};
use renderer::{
    DialFailure, FrameId, HistorySnapshot, MAX_RESPONSE_BODY_BYTES, Mount, RemoteValue,
    RendererEvent, ResourceLimit, ResponseHead, TabError,
};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep_until, timeout};
use url::Url;

use crate::assignment::Assignment;
use crate::exchange::{self, RequestId, ServerInput};
use crate::history::SessionHistory;
use crate::manager::RendererProcessManager;
use crate::network::{NavOutcome, TabNetworkHandle, dial_failure};
use crate::site::Site;

const EVENT_SUBSCRIBER_CAPACITY: usize = 256;
const COMMAND_CAPACITY: usize = 256;
/// Bounded `postMessage` mailbox per tab. Senders enqueue without waiting; a
/// full mailbox drops the message so a busy target cannot stall the sender's
/// renderer service task. The coordinator drains it in order.
const DELIVERY_CAPACITY: usize = 256;
const MAX_WAITERS: usize = 256;
const MAX_SUBSCRIBERS: usize = 256;
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

/// A URL safe to write in a log: no userinfo, query, or fragment.
fn log_url(url: &Url) -> String {
    let mut logged = url.clone();
    if logged.set_username("").is_ok() {
        let _ = logged.set_password(None);
    }
    logged.set_query(None);
    logged.set_fragment(None);
    logged.to_string()
}

/// Identity of one tab in a [`crate::Browser`] registry.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct TabId(u64);

/// Observable lifecycle transitions for one tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabEvent {
    /// A navigation committed its final URL.
    Navigated,
    /// The active document changed URL without replacing its JS realm.
    SameDocumentNavigation,
    /// The top-level document fired `load`.
    Load,
    /// The top-level document fired `DOMContentLoaded`; subresources may
    /// still be loading.
    DomContentLoaded,
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

/// One tab operation. The typed reply channel lives in the variant, so the
/// coordinator cannot answer with another operation's result and the caller
/// never matches a shared reply enum.
enum Command {
    LoadHtml {
        html: String,
        reply: ReplySender<()>,
    },
    Goto {
        url: String,
        reply: ReplySender<()>,
    },
    Execute {
        frame: FrameId,
        source: renderer::ScriptSource,
        timeout: Option<Duration>,
        reply: ReplySender<RemoteValue>,
    },
    AddInitScript {
        source: String,
        run_immediately: bool,
        reply: ReplySender<u64>,
    },
    RemoveInitScript {
        id: u64,
        reply: ReplySender<()>,
    },
    SetViewport {
        frame: FrameId,
        width: u32,
        height: u32,
        reply: ReplySender<()>,
    },
    ViewportSize {
        reply: ReplySender<(u32, u32)>,
    },
    ContentSize {
        frame: FrameId,
        reply: ReplySender<(f32, f32)>,
    },
    Screenshot {
        frame: FrameId,
        request: renderer::ScreenshotRequest,
        reply: ReplySender<Vec<u8>>,
    },
    RunUntilLoadTimeout {
        timeout: Duration,
        reply: ReplySender<bool>,
    },
    RunUntilJsTrue {
        frame: FrameId,
        source: renderer::ScriptSource,
        timeout: Duration,
        reply: ReplySender<bool>,
    },
    DocumentUrl {
        reply: ReplySender<String>,
    },
    LastNavigationFailed {
        reply: ReplySender<bool>,
    },
    Shutdown {
        reply: ReplySender<()>,
    },
}

/// A command's typed answer. The sender's type is the operation's result, so
/// a mismatch is a compile error instead of a discarded reply.
type ReplySender<T> = oneshot::Sender<Result<T, TabError>>;

type TabClient = exchange::Client<Command, Infallible, Infallible, (), TabEvent>;
type TabServer = exchange::Server<Infallible, (), TabEvent, Command, Infallible>;
type TabNotifier = exchange::Notifier<Infallible, (), TabEvent>;

/// Coordinator-side resources one operation executes with.
struct ServeContext<'a> {
    /// Live tab state owned by the coordinator loop.
    tab: &'a mut Tab,
    /// Live `run_until` waiters.
    waiters: &'a mut Vec<Waiter>,
    /// The exchange request that is serving this command, so a retained
    /// waiter can be removed when its caller cancels.
    id: RequestId,
}

/// How [`handle_command`] finishes one served operation.
enum TabOutcome {
    /// The coordinator keeps serving.
    Continue,
    /// A waiter owns the exchange acknowledgement and sends it when it
    /// resolves, so dropping the caller keeps its cancellation armed.
    Retained,
    /// The operation stopped the renderer; the loop returns.
    Shutdown,
}

/// One pending `RunUntilLoadTimeout` (`source` is `None`) or `RunUntilJsTrue`
/// (`source` holds the predicate) wait.
struct Waiter {
    id: RequestId,
    deadline: Instant,
    source: Option<(FrameId, renderer::ScriptSource)>,
    reply: ReplySender<bool>,
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
    pub source: renderer::ScriptSource<&'a str>,
    /// Optional `QuickJS` interruption timeout.
    pub timeout: Option<Duration>,
}

/// One framed JS-truth wait requested through a [`TabHandle`].
pub struct RunUntilJsTrueInOptions<'a> {
    /// Frame to evaluate in.
    pub frame: FrameId,
    /// Predicate source; waits until it evaluates to JS `true`.
    pub source: renderer::ScriptSource<&'a str>,
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
        self.ask(|reply| Command::LoadHtml {
            html: html.to_owned(),
            reply,
        })
        .await
    }

    /// Starts navigation. The tab continues independently; call
    /// [`TabHandle::run_until_load_timeout`] only when the caller needs to wait.
    ///
    /// # Errors
    ///
    /// [`TabError::InvalidUrl`] or [`TabError::ActorStopped`].
    pub async fn goto(&self, url: &str) -> Result<(), TabError> {
        self.ask(|reply| Command::Goto {
            url: url.to_owned(),
            reply,
        })
        .await
    }

    /// Evaluates `source` and returns a value-only script result.
    ///
    /// # Errors
    ///
    /// [`TabError::Script`] or [`TabError::ActorStopped`].
    pub async fn execute_script(&self, source: &str) -> Result<RemoteValue, TabError> {
        self.execute_script_in(ExecuteScriptInOptions {
            frame: FrameId::MAIN,
            source: renderer::ScriptSource::Page(source),
            timeout: None,
        })
        .await
    }

    /// Evaluates a browser-owned expression with private realm capabilities.
    /// Never interpolate page code into `source`; use the private page evaluator
    /// with a separately encoded string. Returns a value-only result.
    ///
    /// # Errors
    ///
    /// [`TabError::Script`] or [`TabError::ActorStopped`].
    pub async fn execute_browser_script(&self, source: &str) -> Result<RemoteValue, TabError> {
        self.execute_script_in(ExecuteScriptInOptions {
            frame: FrameId::MAIN,
            source: renderer::ScriptSource::Browser(source),
            timeout: None,
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
        self.ask(|reply| Command::Execute {
            frame,
            source: source.into_owned(),
            timeout,
            reply,
        })
        .await
    }

    /// The tab's current emulated viewport size in device pixels.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`].
    pub async fn viewport_size(&self) -> Result<(u32, u32), TabError> {
        self.ask(|reply| Command::ViewportSize { reply }).await
    }

    /// The main frame's scrollable content size in CSS pixels.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`].
    pub async fn content_size(&self) -> Result<(f32, f32), TabError> {
        self.ask(|reply| Command::ContentSize {
            frame: FrameId::MAIN,
            reply,
        })
        .await
    }

    /// Sets the main frame's persistent viewport size, re-laying out its
    /// active document.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`].
    pub async fn set_viewport(&self, width: u32, height: u32) -> Result<(), TabError> {
        self.ask(|reply| Command::SetViewport {
            frame: FrameId::MAIN,
            width,
            height,
            reply,
        })
        .await
    }

    /// Registers `source` page-wide; returns the identifier used to remove
    /// it. `run_immediately` also evaluates it in every existing realm.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`].
    pub async fn add_init_script(
        &self,
        source: String,
        run_immediately: bool,
    ) -> Result<u64, TabError> {
        self.ask(|reply| Command::AddInitScript {
            source,
            run_immediately,
            reply,
        })
        .await
    }

    /// Removes a registered init script.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`].
    pub async fn remove_init_script(&self, id: u64) -> Result<(), TabError> {
        self.ask(|reply| Command::RemoveInitScript { id, reply })
            .await
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
        self.ask(|reply| Command::Screenshot {
            frame,
            request,
            reply,
        })
        .await
    }

    /// Waits until the current navigation has fired `load`, returning `false`
    /// on timeout.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`].
    pub async fn run_until_load_timeout(&self, timeout: Duration) -> Result<bool, TabError> {
        self.ask(|reply| Command::RunUntilLoadTimeout { timeout, reply })
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
        self.ask(|reply| Command::RunUntilJsTrue {
            frame,
            source: source.into_owned(),
            timeout,
            reply,
        })
        .await
    }

    /// Document URL after navigation.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`].
    pub async fn document_url(&self) -> Result<String, TabError> {
        self.ask(|reply| Command::DocumentUrl { reply }).await
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
        self.ask(|reply| Command::LastNavigationFailed { reply })
            .await
    }

    /// Sends one command and waits for its typed answer. The exchange call is
    /// only an acknowledgement that the command reached the coordinator; the
    /// result travels on the channel built into the variant.
    async fn ask<T>(&self, command: impl FnOnce(ReplySender<T>) -> Command) -> Result<T, TabError> {
        let (reply, result) = oneshot::channel();
        self.client
            .call(command(reply))
            .await
            .map_err(|_| TabError::ActorStopped)?;
        result.await.unwrap_or(Err(TabError::ActorStopped))
    }
}

/// Join handle and delivery mailbox for one tab coordinator task.
pub(crate) struct TabTask {
    pub handle: TabHandle,
    /// Ordered `postMessage` deliveries for this tab. Senders enqueue without
    /// waiting; the coordinator drains it in order, so a busy target never
    /// blocks the sender's renderer or the browser owner task.
    deliveries: mpsc::Sender<String>,
    join: Option<JoinHandle<()>>,
}

impl TabTask {
    pub(crate) fn spawn(
        id: TabId,
        network: TabNetworkHandle,
        renderers: Arc<RendererProcessManager>,
    ) -> Self {
        let (client, server) = exchange::local(COMMAND_CAPACITY);
        let handle = TabHandle { id, client };
        let tab = Tab::new(id, network, renderers, server.notifier());
        let (deliveries, delivery_rx) = mpsc::channel(DELIVERY_CAPACITY);
        let join = tokio::spawn(coordinator_loop(server, tab, delivery_rx));
        Self {
            handle,
            deliveries,
            join: Some(join),
        }
    }

    /// The tab's `postMessage` mailbox.
    pub(crate) fn deliveries(&self) -> mpsc::Sender<String> {
        self.deliveries.clone()
    }

    pub(crate) async fn shutdown(&mut self) {
        let _result = timeout(
            SHUTDOWN_TIMEOUT,
            self.handle.ask(|reply| Command::Shutdown { reply }),
        )
        .await;
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
    renderer: Option<Arc<Assignment>>,
    pending_mount: Option<Mount>,
    site: Option<Site>,
    events_rx: Option<mpsc::Receiver<(FrameId, RendererEvent)>>,
    document_url: Url,
    /// Emulated viewport size in device pixels
    /// (<https://chromedevtools.github.io/devtools-protocol/tot/Emulation/#method-setDeviceMetricsOverride>).
    /// Divergence: the override is tab state and outlives the CDP session,
    /// unlike Chromium, which clears it on disconnect (corpus test
    /// `inspector-protocol/emulation/device-metrics-override-cleared-on-disconnect.js`).
    viewport: (u32, u32),
    /// Page-scoped init scripts, replayed on every renderer acquisition
    /// (<https://chromedevtools.github.io/devtools-protocol/tot/Page/#method-addScriptToEvaluateOnNewDocument>).
    init_scripts: Vec<(u64, String)>,
    /// Next init-script identifier.
    next_init_script: u64,
    history: SessionHistory,
    traversal: Option<crate::history::EntryId>,
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

impl Tab {
    fn new(
        id: TabId,
        network: TabNetworkHandle,
        renderers: Arc<RendererProcessManager>,
        events: TabNotifier,
    ) -> Self {
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
            viewport: renderer::DEFAULT_VIEWPORT,
            init_scripts: Vec::new(),
            next_init_script: 1,
            history: SessionHistory::new(
                Url::parse("about:blank").expect("about:blank is a valid URL"),
            ),
            traversal: None,
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
            head: ResponseHead {
                history: self.history.snapshot(),
                viewport: Some(self.viewport),
                init_scripts: self.init_scripts.clone(),
                ..ResponseHead::non_http(
                    self.document_url.to_string(),
                    Some("text/html; charset=utf-8".to_owned()),
                )
            },
            body: html.as_bytes().to_vec(),
        };
        if self.renderer.is_none() && self.document_url.scheme() == "about" {
            self.pending_mount = Some(mount);
            self.document_loaded = true;
            // The deferred blank document still fired parsing's end, so both
            // readiness events happen before `load`.
            self.record_event(TabEvent::DomContentLoaded).await;
            self.record_event(TabEvent::Load).await;
            return Ok(());
        }
        self.mount(&site, mount).await
    }

    fn goto(&mut self, spec: &str) -> Result<(), TabError> {
        let url = self.resolve_url(spec)?;
        self.traversal = None;
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
        if !matches!(url.scheme(), "http" | "https" | "data") {
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
            .acquire(self.id, site.clone(), self.network.clone())
            .await
            .map_err(|error| renderer_unavailable(&error.to_string()))?;
        self.drop_renderer();
        self.events_rx = Some(handle.subscribe());
        self.site = Some(site.clone());
        self.renderer = Some(handle);
        Ok(())
    }

    async fn mount(&mut self, site: &Site, mount: Mount) -> Result<(), TabError> {
        self.ensure_renderer(site).await?;
        self.pending_mount = None;
        self.document_loaded = false;
        let document_url = mount.head.url.clone();
        let result = self
            .renderer
            .as_ref()
            .ok_or(TabError::ActorStopped)?
            .mount(FrameId::MAIN, mount)
            .await
            .and_then(reply_unit);
        if result.is_err() {
            self.drop_renderer();
        } else {
            self.record_committed_origin(&document_url);
        }
        result
    }

    async fn mount_stream(
        &mut self,
        site: &Site,
        mount: Mount,
        mut body: net::Body,
    ) -> Result<(), TabError> {
        self.ensure_renderer(site).await?;
        self.pending_mount = None;
        self.document_loaded = false;
        let document_url = mount.head.url.clone();
        let renderer = self.renderer.as_ref().ok_or(TabError::ActorStopped)?;
        let stream = renderer.start_response(FrameId::MAIN, &mount).await?;
        let mut received = 0usize;
        loop {
            match body.read_chunk().await {
                Ok(Some(chunk)) => {
                    received = received.saturating_add(chunk.len());
                    if received > MAX_RESPONSE_BODY_BYTES {
                        let _ = stream.abort(DialFailure::Limit).await;
                        self.drop_renderer();
                        return Err(renderer_unavailable(&format!(
                            "navigation body exceeds {MAX_RESPONSE_BODY_BYTES} bytes ({received} received)"
                        )));
                    }
                    // One HTTP frame can be larger than the IPC body chunk.
                    // The buffered mount already splits; this path must too.
                    for piece in chunk.chunks(crate::wire::channel::MAX_BODY_CHUNK_BYTES) {
                        if let Err(error) = stream.write(piece.to_vec()).await {
                            drop(stream);
                            self.drop_renderer();
                            return Err(error);
                        }
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    let failure = dial_failure(&error);
                    let _ = stream.abort(failure).await;
                    self.drop_renderer();
                    return Err(renderer_unavailable(&error.to_string()));
                }
            }
        }
        let result = stream.finish().await.and_then(reply_unit);
        if result.is_err() {
            self.drop_renderer();
        } else {
            self.record_committed_origin(&document_url);
        }
        result
    }

    /// Records the browser-owned origin of the document the renderer mounted.
    fn record_committed_origin(&self, url: &str) {
        if let Some(renderer) = self.renderer.as_ref()
            && let Ok(url) = Url::parse(url)
        {
            renderer.set_committed_origin(url.origin());
        }
    }

    fn drop_renderer(&mut self) {
        self.site = None;
        self.events_rx = None;
        // Dropping the assignment is the release: it removes the registry
        // record, notifies the renderer, and lets the process manager
        // evaluate shutdown.
        self.renderer = None;
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
        let result = renderer.request(command).await;
        if result.is_err() {
            // Transport failures and timeouts both mean this renderer is not
            // usable: release it so the next operation acquires a fresh one.
            self.drop_renderer();
        }
        result
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
        let result = renderer.request_bytes(command).await;
        if result.is_err() {
            self.drop_renderer();
        }
        result
    }

    /// Mounts the virtual blank document this tab has been carrying.
    async fn mount_virtual(&mut self) -> Result<(), TabError> {
        let mut mount = self.pending_mount.take().unwrap_or_else(blank_mount);
        // The replacement renderer must come up with the tab's emulated
        // viewport and script list, not the defaults.
        mount.head.viewport = Some(self.viewport);
        mount.head.init_scripts = self.init_scripts.clone();
        let site = Site::for_url(&self.document_url)
            .or_else(|| self.site.clone())
            .unwrap_or_else(|| Site::opaque(self.id));
        self.mount(&site, mount).await
    }

    /// Takes a pending `data:` navigation so it can be committed without a dial.
    ///
    /// `data:` is decoded in-process
    /// (<https://fetch.spec.whatwg.org/#data-url-processor>).
    fn take_data_navigation(&mut self) -> Option<Url> {
        let data = self
            .nav
            .as_ref()
            .is_some_and(|nav| !nav.submitted && nav.url.scheme() == "data");
        if !data {
            return None;
        }
        self.nav.take().map(|nav| nav.url)
    }

    /// Mounts a `data:` document as the top-level page.
    async fn commit_data_navigation(&mut self, url: Url) -> bool {
        // Shared with subframe dials: reject a raw URL past the data-URL
        // ceiling before decode, then reject a decoded body past the cap.
        let Some((content_type, body)) = renderer::decode_data_url_bounded(url.as_str()) else {
            self.navigation_failed = true;
            return false;
        };
        let site = Site::for_url(&url).unwrap_or_else(|| Site::opaque(self.id));
        let mut history = self.history.clone();
        if !history.navigate(url.clone(), self.traversal.take()) {
            self.navigation_failed = true;
            return false;
        }
        let mount = Mount {
            head: ResponseHead {
                history: history.snapshot(),
                viewport: Some(self.viewport),
                init_scripts: self.init_scripts.clone(),
                ..ResponseHead::non_http(url.to_string(), Some(content_type))
            },
            body,
        };
        if let Err(error) = self.mount(&site, mount).await {
            logging::error!(
                target: "browser::tab",
                "data navigation mount failed for {}: {error}",
                log_url(&url)
            );
            self.navigation_failed = true;
            return false;
        }
        self.history = history;
        self.document_url = url;
        self.navigation_failed = false;
        true
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
            self.network
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
        let url = self.nav.as_ref().map(|nav| nav.url.clone());
        self.dial_cancel = None;
        self.dial_guard = None;
        self.nav = None;
        match result {
            Ok(outcome) => {
                let mounted = self.commit_navigation(outcome).await;
                self.record_event(if mounted {
                    TabEvent::Navigated
                } else {
                    TabEvent::NavigationFailed
                })
                .await;
            }
            Err(failure) => {
                logging::error!(
                    target: "browser::tab",
                    "navigation dial failed for {}: {failure:?}",
                    url.as_ref().map(log_url).as_deref().unwrap_or("unknown")
                );
                self.traversal = None;
                self.navigation_failed = true;
                self.record_event(TabEvent::NavigationFailed).await;
            }
        }
    }

    /// Returns true when the new document mounted.
    async fn commit_navigation(&mut self, outcome: NavOutcome) -> bool {
        let Ok(final_url) = Url::parse(&outcome.head.url) else {
            self.navigation_failed = true;
            return false;
        };
        let site = Site::for_url(&final_url).unwrap_or_else(|| Site::opaque(self.id));
        let mut history = self.history.clone();
        if !history.navigate(final_url.clone(), self.traversal.take()) {
            logging::error!(
                target: "browser::tab",
                "navigation rejected by history for {}",
                log_url(&final_url)
            );
            self.navigation_failed = true;
            return false;
        }
        let head = ResponseHead {
            history: history.snapshot(),
            viewport: Some(self.viewport),
            init_scripts: self.init_scripts.clone(),
            ..outcome.head
        };
        if let Err(error) = self
            .mount_stream(
                &site,
                Mount {
                    head,
                    body: Vec::new(),
                },
                outcome.body,
            )
            .await
        {
            logging::error!(
                target: "browser::tab",
                "navigation mount failed for {}: {error}",
                log_url(&final_url)
            );
            self.navigation_failed = true;
            return false;
        }
        self.history = history;
        self.document_url = final_url;
        true
    }

    async fn handle_renderer_event(&mut self, frame: FrameId, event: &RendererEvent) {
        if frame != FrameId::MAIN {
            return;
        }
        match event {
            RendererEvent::Navigated { url } => {
                if let Ok(url) = Url::parse(url) {
                    self.history.navigate(url.clone(), None);
                    self.document_url = url;
                }
                // The committed origin is an authorization input; record it from
                // the navigated URL, as a browser-initiated mount does
                // (<https://html.spec.whatwg.org/multipage/browsers.html#concept-origin>).
                self.record_committed_origin(url);
                self.document_loaded = false;
                self.navigation_failed = false;
                self.record_event(TabEvent::Navigated).await;
            }
            RendererEvent::Load => {
                self.document_loaded = true;
                self.record_event(TabEvent::Load).await;
            }
            RendererEvent::DomContentLoaded => {
                self.record_event(TabEvent::DomContentLoaded).await;
            }
            RendererEvent::HistoryUpdated {
                url,
                state,
                replace,
            } => {
                if let Ok(url) = Url::parse(url)
                    && url.origin() == self.document_url.origin()
                {
                    self.history.update(url.clone(), state.clone(), *replace);
                    self.document_url = url;
                    self.record_event(TabEvent::SameDocumentNavigation).await;
                }
            }
            RendererEvent::HistoryTraversal { delta } => {
                self.traverse_history(*delta).await;
            }
            _ => {}
        }
    }

    async fn traverse_history(&mut self, delta: i32) {
        // `go(0)` reloads the current entry without touching history
        // (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-history-go>).
        if delta == 0 {
            let url = self.document_url.to_string();
            if self.goto(&url).is_ok() {
                self.traversal = Some(self.history.current_entry());
            }
            return;
        }
        let Some((index, entry, url, state, same_document)) = self.history.target(delta) else {
            return;
        };
        let url = url.to_string();
        let state = state.map(str::to_owned);
        if same_document {
            let result = self
                .renderer_request(RendererCommand::HistoryTraverse {
                    url: url.clone(),
                    history: HistorySnapshot {
                        state,
                        index,
                        length: self.history.snapshot().length,
                    },
                })
                .await;
            if matches!(result, Ok(Reply::Unit(Ok(())))) {
                self.history.traverse_same_document(index);
                if let Ok(url) = Url::parse(&url) {
                    self.document_url = url;
                }
                self.record_event(TabEvent::SameDocumentNavigation).await;
            }
            return;
        }
        if self.goto(&url).is_ok() {
            self.traversal = Some(entry);
        }
    }

    async fn record_event(&self, event: TabEvent) {
        let _result = self.events.notify(event).await;
    }

    fn waiting_for_load(&self) -> bool {
        self.nav.is_some() || (!self.document_loaded && !self.navigation_failed)
    }

    fn stop_renderer(&mut self) {
        self.cancel_dial();
        self.drop_renderer();
    }
}

fn blank_mount() -> Mount {
    Mount {
        head: ResponseHead::non_http("about:blank", Some("text/html; charset=utf-8".to_owned())),
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
    // Boxed: a navigation outcome carries response bodies and headers, and
    // the unboxed form trips `large_enum_variant` next to the small arms.
    Navigation(Box<Option<(u64, Result<NavOutcome, DialFailure>)>>),
    Renderer(Option<(FrameId, RendererEvent)>),
    Delivery(Option<String>),
    WaiterDeadline,
}

async fn coordinator_loop(
    mut server: TabServer,
    mut tab: Tab,
    delivery_rx: mpsc::Receiver<String>,
) {
    let mut waiters = Vec::new();
    let mut delivery_rx = Some(delivery_rx);
    loop {
        if let Some(url) = tab.take_data_navigation() {
            let mounted = tab.commit_data_navigation(url).await;
            tab.record_event(if mounted {
                TabEvent::Navigated
            } else {
                TabEvent::NavigationFailed
            })
            .await;
            continue;
        }
        tab.launch_navigation();
        let deadline = next_waiter_deadline(&waiters);
        let wake = {
            let dial_rx = &mut tab.dial_rx;
            let events_rx = &mut tab.events_rx;
            tokio::select! {
                command = server.recv() => Wake::Command(command),
                navigation = dial_rx.recv() => Wake::Navigation(Box::new(navigation)),
                event = next_renderer_event(events_rx) => Wake::Renderer(event),
                delivery = next_delivery(&mut delivery_rx) => Wake::Delivery(delivery),
                () = wait_for_deadline(deadline) => Wake::WaiterDeadline,
            }
        };
        match wake {
            Wake::Command(Some(ServerInput::Call { id, body })) => {
                if handle_command(&mut tab, &server, id, body, &mut waiters).await {
                    return;
                }
            }
            Wake::Command(Some(ServerInput::Cancel { id })) => {
                // A caller dropped its future. Drop the waiter now; a command
                // still in the queue is covered by `retain_waiter`.
                waiters.retain(|waiter| waiter.id != id && !waiter.reply.is_closed());
            }
            Wake::Command(Some(ServerInput::RequestChunk | ServerInput::RequestEnd))
            | Wake::WaiterDeadline => {}
            Wake::Command(Some(ServerInput::Notify(_)) | None) => break,
            Wake::Navigation(boxed) => {
                if let Some((epoch, result)) = *boxed {
                    tab.handle_navigation(epoch, result).await;
                }
            }
            Wake::Renderer(Some((frame, event))) => {
                let navigated = matches!(event, RendererEvent::Navigated { .. });
                tab.handle_renderer_event(frame, &event).await;
                if navigated {
                    // The frame's realm is gone, so its JS-truth waiters cannot
                    // settle; resolve them instead of polling to the deadline.
                    resolve_frame_waiters(&server, &mut waiters, frame).await;
                }
            }
            Wake::Renderer(None) => {
                tab.drop_renderer();
                fail_waiters(&server, &mut waiters, &TabError::ActorStopped).await;
            }
            Wake::Delivery(Some(payload)) => {
                let _result = tab.deliver_window_message(payload).await;
            }
            Wake::Delivery(None) => {
                // Every delivery sender is gone; stop selecting on the lane.
                delivery_rx = None;
            }
        }
        resolve_waiters(&mut tab, &server, &mut waiters).await;
    }
    tab.stop_renderer();
}

async fn next_renderer_event(
    events: &mut Option<mpsc::Receiver<(FrameId, RendererEvent)>>,
) -> Option<(FrameId, RendererEvent)> {
    match events {
        Some(receiver) => receiver.recv().await,
        None => pending().await,
    }
}

async fn next_delivery(deliveries: &mut Option<mpsc::Receiver<String>>) -> Option<String> {
    match deliveries {
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
async fn handle_command(
    tab: &mut Tab,
    server: &TabServer,
    id: RequestId,
    command: Command,
    waiters: &mut Vec<Waiter>,
) -> bool {
    // A cancellation on the control lane can overtake its call on the data
    // lane. The caller's receiver is already dropped then, so skip a side
    // effect nobody observes. `Shutdown` always runs.
    let outcome = if command.abandoned() {
        TabOutcome::Continue
    } else {
        command.serve(ServeContext { tab, waiters, id }).await
    };
    // A retained waiter acknowledges when it resolves; an immediate ack would
    // disarm the caller's `CancelOnDrop` and leave a dropped waiter to poll
    // until its deadline. Everything else acknowledges here.
    if !matches!(outcome, TabOutcome::Retained) {
        let _ack = server.reply(id, ()).await;
    }
    matches!(outcome, TabOutcome::Shutdown)
}

impl Command {
    /// True when the caller dropped its receiver before dispatch.
    ///
    /// `Shutdown` is exempt: the renderer must stop even when the caller
    /// stopped waiting. Every other variant must be listed here, so a new
    /// command cannot silently run after its caller cancelled.
    fn abandoned(&self) -> bool {
        match self {
            Command::Shutdown { .. } => false,
            Command::LoadHtml { reply, .. } | Command::Goto { reply, .. } => reply.is_closed(),
            Command::Execute { reply, .. } => reply.is_closed(),
            Command::AddInitScript { reply, .. } => reply.is_closed(),
            Command::RemoveInitScript { reply, .. } | Command::SetViewport { reply, .. } => {
                reply.is_closed()
            }
            Command::ViewportSize { reply } => reply.is_closed(),
            Command::ContentSize { reply, .. } => reply.is_closed(),
            Command::Screenshot { reply, .. } => reply.is_closed(),
            Command::RunUntilLoadTimeout { reply, .. } | Command::RunUntilJsTrue { reply, .. } => {
                reply.is_closed()
            }
            Command::DocumentUrl { reply } => reply.is_closed(),
            Command::LastNavigationFailed { reply } => reply.is_closed(),
        }
    }

    /// Runs one operation and sends its typed result. The sender in the
    /// variant is the only way out, so a command cannot answer with another
    /// command's type.
    async fn serve(self, ctx: ServeContext<'_>) -> TabOutcome {
        let ServeContext { tab, waiters, id } = ctx;
        match self {
            Command::LoadHtml { html, reply } => {
                let _result = reply.send(tab.load_html(&html).await);
            }
            Command::Goto { url, reply } => {
                let _result = reply.send(tab.goto(&url));
            }
            Command::Execute {
                frame,
                source,
                timeout,
                reply,
            } => {
                let result = execute_script(tab, frame, source, timeout).await;
                let _result = reply.send(result);
            }
            Command::AddInitScript {
                source,
                run_immediately,
                reply,
            } => {
                let result = add_init_script(tab, source, run_immediately).await;
                let _result = reply.send(result);
            }
            Command::RemoveInitScript { id: script, reply } => {
                let result = remove_init_script(tab, script).await;
                let _result = reply.send(result);
            }
            Command::SetViewport {
                frame,
                width,
                height,
                reply,
            } => {
                let result = set_viewport(tab, frame, width, height).await;
                let _result = reply.send(result);
            }
            Command::ViewportSize { reply } => {
                let _result = reply.send(Ok(tab.viewport));
            }
            Command::ContentSize { frame, reply } => {
                let result = tab
                    .renderer_request(RendererCommand::ContentSize { frame })
                    .await
                    .and_then(reply_content_size);
                let _result = reply.send(result);
            }
            Command::Screenshot {
                frame,
                request,
                reply,
            } => {
                let result = screenshot_frame(tab, frame, request).await;
                let _result = reply.send(result);
            }
            Command::RunUntilLoadTimeout { timeout, reply } => {
                if retain_load_waiter(waiters, id, timeout, reply) {
                    return TabOutcome::Retained;
                }
            }
            Command::RunUntilJsTrue {
                frame,
                source,
                timeout,
                reply,
            } => {
                if retain_js_waiter(waiters, id, frame, source, timeout, reply) {
                    return TabOutcome::Retained;
                }
            }
            Command::DocumentUrl { reply } => {
                let _result = reply.send(Ok(tab.document_url.to_string()));
            }
            Command::LastNavigationFailed { reply } => {
                let _result = reply.send(Ok(tab.navigation_failed));
            }
            Command::Shutdown { reply } => {
                tab.stop_renderer();
                let _result = reply.send(Ok(()));
                return TabOutcome::Shutdown;
            }
        }
        TabOutcome::Continue
    }
}

async fn execute_script(
    tab: &mut Tab,
    frame: FrameId,
    source: renderer::ScriptSource,
    timeout: Option<Duration>,
) -> Result<RemoteValue, TabError> {
    tab.renderer_request(RendererCommand::ExecuteScript {
        frame,
        source,
        timeout_ms: timeout.map(millis),
    })
    .await
    .and_then(reply_value)
}

async fn add_init_script(
    tab: &mut Tab,
    source: String,
    run_immediately: bool,
) -> Result<u64, TabError> {
    let id = tab.next_init_script;
    tab.next_init_script = tab.next_init_script.wrapping_add(1);
    tab.init_scripts.push((id, source.clone()));
    if tab.renderer.is_some() {
        tab.renderer_request(RendererCommand::AddInitScript {
            id,
            source,
            run_immediately,
        })
        .await
        .and_then(reply_unit)
        .map(|()| id)
    } else {
        // No renderer yet: the next mount carries the list.
        Ok(id)
    }
}

async fn remove_init_script(tab: &mut Tab, id: u64) -> Result<(), TabError> {
    tab.init_scripts.retain(|(script_id, _)| *script_id != id);
    if tab.renderer.is_some() {
        tab.renderer_request(RendererCommand::RemoveInitScript { id })
            .await
            .and_then(reply_unit)
    } else {
        Ok(())
    }
}

async fn set_viewport(
    tab: &mut Tab,
    frame: FrameId,
    width: u32,
    height: u32,
) -> Result<(), TabError> {
    let result = tab
        .renderer_request(RendererCommand::SetViewport {
            frame,
            width,
            height,
        })
        .await
        .and_then(reply_unit);
    if result.is_ok() {
        tab.viewport = (width, height);
    }
    result
}

async fn screenshot_frame(
    tab: &mut Tab,
    frame: FrameId,
    request: renderer::ScreenshotRequest,
) -> Result<Vec<u8>, TabError> {
    tab.renderer_request_bytes(RendererCommand::Screenshot { frame, request })
        .await
}

fn retain_load_waiter(
    waiters: &mut Vec<Waiter>,
    id: RequestId,
    timeout: Duration,
    reply: ReplySender<bool>,
) -> bool {
    retain_waiter(
        waiters,
        Waiter {
            id,
            deadline: Instant::now() + timeout,
            source: None,
            reply,
        },
    )
}

fn retain_js_waiter(
    waiters: &mut Vec<Waiter>,
    id: RequestId,
    frame: FrameId,
    source: renderer::ScriptSource,
    timeout: Duration,
    reply: ReplySender<bool>,
) -> bool {
    retain_waiter(
        waiters,
        Waiter {
            id,
            deadline: Instant::now() + timeout,
            source: Some((frame, source)),
            reply,
        },
    )
}

fn retain_waiter(waiters: &mut Vec<Waiter>, waiter: Waiter) -> bool {
    // The caller may already have dropped its future while the command was in
    // the queue; a receiver-less waiter would sit until its deadline.
    if waiter.reply.is_closed() {
        return false;
    }
    if waiters.len() < MAX_WAITERS {
        waiters.push(waiter);
        return true;
    }
    let error = TabError::ResourceLimit {
        resource: ResourceLimit::TabWaiters,
    };
    let _result = waiter.reply.send(Err(error));
    false
}

async fn fail_waiters(server: &TabServer, waiters: &mut Vec<Waiter>, error: &TabError) {
    for waiter in waiters.drain(..) {
        let _result = waiter.reply.send(Err(error.clone()));
        let _ack = server.reply(waiter.id, ()).await;
    }
}

/// Resolves the `RunUntilJs` waiters for a frame that just navigated: the
/// predicate ran against the old realm and cannot settle, so reply `false`
/// instead of polling to the deadline.
async fn resolve_frame_waiters(server: &TabServer, waiters: &mut Vec<Waiter>, frame: FrameId) {
    let mut pending = Vec::new();
    for waiter in std::mem::take(waiters) {
        if waiter
            .source
            .as_ref()
            .is_some_and(|(wait_frame, _)| *wait_frame == frame)
        {
            let _result = waiter.reply.send(Ok(false));
            let _ack = server.reply(waiter.id, ()).await;
        } else {
            pending.push(waiter);
        }
    }
    *waiters = pending;
}

async fn resolve_waiters(tab: &mut Tab, server: &TabServer, waiters: &mut Vec<Waiter>) {
    let now = Instant::now();
    let mut pending = Vec::new();
    for waiter in std::mem::take(waiters) {
        // The caller dropped its future; its `Cancel` (or the exchange closing)
        // already released the request, so there is nothing left to answer.
        if waiter.reply.is_closed() {
            continue;
        }
        let result = match waiter.source.clone() {
            // A load wait finishes as soon as the tab stops waiting, before
            // its deadline is considered.
            None if !tab.waiting_for_load() => Some(Ok(true)),
            None if now >= waiter.deadline => Some(Ok(false)),
            None => None,
            Some((frame, source)) => {
                if now >= waiter.deadline {
                    Some(Ok(false))
                } else if matches!(
                    tab.renderer_request(RendererCommand::ExecuteScript {
                        frame,
                        source,
                        timeout_ms: None
                    })
                    .await,
                    Ok(Reply::Value(Ok(RemoteValue::Bool(true))))
                ) {
                    Some(Ok(true))
                } else {
                    None
                }
            }
        };
        if let Some(result) = result {
            let _result = waiter.reply.send(result);
            let _ack = server.reply(waiter.id, ()).await;
        } else {
            pending.push(waiter);
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

fn reply_content_size(reply: Reply) -> Result<(f32, f32), TabError> {
    match reply {
        Reply::ContentSize(result) => result,
        _ => Err(TabError::ActorStopped),
    }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
