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
    BrowserEvent, DialFailure, FrameId, HistorySnapshot as RendererHistorySnapshot,
    MAX_RESPONSE_BODY_BYTES, Mount, RemoteValue, ResourceLimit, ResponseHead, ScriptSource,
    TabError,
};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep_until, timeout};
use url::Url;

use crate::assignment::{Assignment, RendererWake};
use crate::browser::{TabNames, TabRegistry};
use crate::exchange::{self, RequestId, ServerInput};
use crate::history::{Commit, HistorySnapshot, SessionHistory};
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

/// One [`TabHandle`] navigation request.
struct GotoRequest {
    url: String,
    /// The document that started the navigation. `None` for a
    /// browser-initiated navigation (`CDP`, `WebDriver`, history), which sends
    /// no referrer and `Sec-Fetch-Site: none`.
    initiator: Option<NavigationInitiator>,
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
        /// The URL and, for a renderer-initiated navigation, the document that
        /// asked for it.
        request: GotoRequest,
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
    DocumentTitle {
        reply: ReplySender<String>,
    },
    History {
        reply: ReplySender<HistorySnapshot>,
    },
    TraverseHistory {
        delta: i32,
        reply: ReplySender<()>,
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

/// What one browser-driven commit replaced in the tab's name registry.
struct CommitNames {
    /// The pair the commit replaced; `None` when its tab had closed.
    previous: Option<TabNames>,
    /// The pair the commit installed, which a failed mount may put back.
    installed: TabNames,
}

/// One pending `RunUntilLoadTimeout` or `RunUntilJsTrue` wait.
struct Waiter {
    id: RequestId,
    deadline: Instant,
    kind: WaiterKind,
    reply: ReplySender<bool>,
}

/// What a waiter is waiting for. A JS-truth waiter lives in the renderer,
/// which evaluates its predicate after each drain and reports the id back.
enum WaiterKind {
    Load,
    Js {
        /// Identity the renderer knows this waiter by.
        renderer_id: u64,
        /// Frame whose realm runs the predicate, so a navigation can retire it.
        frame: FrameId,
    },
}

impl Waiter {
    /// The renderer-side identity, for waiters that have one.
    fn renderer_waiter(&self) -> Option<u64> {
        match self.kind {
            WaiterKind::Load => None,
            WaiterKind::Js { renderer_id, .. } => Some(renderer_id),
        }
    }
}

/// Async value-only handle to one tab coordinator.
#[derive(Clone)]
pub struct TabHandle {
    id: TabId,
    client: TabClient,
}

/// One framed script evaluation requested through a [`TabHandle`].
pub struct ExecuteScriptOptions<'a> {
    /// Frame to evaluate in.
    pub frame: FrameId,
    /// Script source.
    pub source: renderer::ScriptSource<&'a str>,
    /// Optional `QuickJS` interruption timeout.
    pub timeout: Option<Duration>,
}

/// One framed JS-truth wait requested through a [`TabHandle`].
pub struct RunUntilJsTrueOptions<'a> {
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
            request: GotoRequest {
                url: url.to_owned(),
                initiator: None,
            },
            reply,
        })
        .await
    }

    /// Starts a navigation a renderer asked for, carrying the calling
    /// document's origin, referrer source, and policy. `window.open` uses it
    /// for the browser-hosted popup; a browser-initiated navigation calls
    /// [`Self::goto`] instead
    /// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#window-open-steps>).
    pub(crate) async fn goto_from(
        &self,
        url: &str,
        initiator: NavigationInitiator,
    ) -> Result<(), TabError> {
        self.ask(|reply| Command::Goto {
            request: GotoRequest {
                url: url.to_owned(),
                initiator: Some(initiator),
            },
            reply,
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
    pub async fn execute_script(
        &self,
        options: ExecuteScriptOptions<'_>,
    ) -> Result<RemoteValue, TabError> {
        let ExecuteScriptOptions {
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

    /// Evaluates a browser-owned expression with private realm capabilities.
    /// Never interpolate page code into `source`; use the private page evaluator
    /// with a separately encoded string. Returns a value-only result.
    ///
    /// # Errors
    ///
    /// [`TabError::Script`] or [`TabError::ActorStopped`].
    pub async fn execute_browser_script(&self, source: &str) -> Result<RemoteValue, TabError> {
        self.execute_script(ExecuteScriptOptions {
            frame: FrameId::MAIN,
            source: renderer::ScriptSource::Browser(source),
            timeout: None,
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
    pub async fn run_until_js_true(
        &self,
        options: RunUntilJsTrueOptions<'_>,
    ) -> Result<bool, TabError> {
        let RunUntilJsTrueOptions {
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

    /// Document title after navigation. Empty when the tab has no renderer.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`].
    pub async fn document_title(&self) -> Result<String, TabError> {
        self.ask(|reply| Command::DocumentTitle { reply }).await
    }

    /// Committed session-history entries in traversal order and the active
    /// entry's index.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`].
    pub async fn history(&self) -> Result<HistorySnapshot, TabError> {
        self.ask(|reply| Command::History { reply }).await
    }

    /// Traverses `delta` entries through the tab's session history.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`].
    pub async fn traverse_history(&self, delta: i32) -> Result<(), TabError> {
        self.ask(|reply| Command::TraverseHistory { delta, reply })
            .await
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

/// One cross-tab `postMessage` in flight through the browser. The browser has
/// authorized the sender's origin; the target's realm checks the target origin
/// when the delivery task runs.
pub(crate) struct RemoteWindowMessage {
    pub(crate) message: renderer::PostedMessage,
}

/// Join handle and delivery mailbox for one tab coordinator task.
pub(crate) struct TabTask {
    pub handle: TabHandle,
    /// Ordered `postMessage` deliveries for this tab. Senders enqueue without
    /// waiting; the coordinator drains it in order, so a busy target never
    /// blocks the sender's renderer or the browser owner task.
    deliveries: mpsc::Sender<RemoteWindowMessage>,
    join: Option<JoinHandle<()>>,
}

impl TabTask {
    pub(crate) fn spawn(
        id: TabId,
        network: TabNetworkHandle,
        renderers: Arc<RendererProcessManager>,
        tabs: TabRegistry,
        tab: NewTab,
    ) -> Self {
        let (client, server) = exchange::local(COMMAND_CAPACITY);
        let handle = TabHandle { id, client };
        let tab = Tab::new(id, network, renderers, server.notifier(), tabs, tab);
        let (deliveries, delivery_rx) = mpsc::channel(DELIVERY_CAPACITY);
        let join = tokio::spawn(coordinator_loop(server, tab, delivery_rx));
        Self {
            handle,
            deliveries,
            join: Some(join),
        }
    }

    /// A task with no coordinator behind it, for unit tests that only need a
    /// live tab id.
    #[cfg(test)]
    pub(crate) fn fixture(id: TabId) -> Self {
        let (client, _server) = exchange::local(1);
        let (deliveries, _receiver) = mpsc::channel(1);
        Self {
            handle: TabHandle { id, client },
            deliveries,
            join: None,
        }
    }

    /// The tab's `postMessage` mailbox.
    pub(crate) fn deliveries(&self) -> mpsc::Sender<RemoteWindowMessage> {
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

/// The document a navigation starts from: the request origin, the referrer
/// source, and the policy that decides the `Referer` header.
///
/// A browser-initiated navigation has none: it sends no `Referer`,
/// `Sec-Fetch-Site: none`, and no `SameSite` initiator. The origin is a
/// [`url::Origin`], so an opaque document carries `Origin::Opaque` rather than
/// a URL sentinel
/// (<https://w3c.github.io/webappsec-referrer-policy/#determine-requests-referrer>).
#[derive(Clone, Debug)]
pub(crate) struct NavigationInitiator {
    /// The initiating document's origin. The browser authorized it against
    /// the renderer's site lock before constructing this.
    pub(crate) origin: url::Origin,
    /// The `"client"` referrer source, walked out of any `srcdoc` chain, when
    /// the browser authorized it against the renderer's site lock.
    pub(crate) referrer: Option<Url>,
    /// The concrete policy main fetch determines under.
    pub(crate) referrer_policy: net::ReferrerPolicy,
}

struct ActiveNavigation {
    url: Url,
    /// The document the navigation started from. `None` when the browser
    /// started it, which sends no referrer and `Sec-Fetch-Site: none`.
    initiator: Option<NavigationInitiator>,
    epoch: u64,
    submitted: bool,
}

/// What the browser hands a new tab coordinator: the browsing-context state
/// that belongs to the tab rather than to whatever document it commits first.
pub(crate) struct NewTab {
    /// The target name the tab starts with, which is the `window.open`
    /// target's name
    /// (<https://html.spec.whatwg.org/multipage/document-sequences.html#browsing-context-name>).
    pub(crate) name: Option<renderer::ContextName>,
    /// The tab that opened this one, when the opener relationship is live.
    /// A tab with a live opener keeps its target name across a cross-origin
    /// navigation
    /// (<https://html.spec.whatwg.org/multipage/browsing-the-web.html#resetBCName>).
    pub(crate) opener: Option<TabId>,
    /// The frame inside `opener` that opened this one, when it is live. A
    /// sandboxed document may navigate the auxiliary contexts it created, so
    /// the opener relationship a sandbox check reads is the frame's, not the
    /// tab's
    /// (<https://html.spec.whatwg.org/multipage/browsers.html#sandboxed-navigation-browsing-context-flag>).
    pub(crate) opener_frame: Option<renderer::FrameId>,
    /// The popup sandboxing flag set every document this tab installs
    /// inherits
    /// (<https://html.spec.whatwg.org/multipage/browsers.html#popup-sandboxing-flag-set>).
    pub(crate) popup_sandbox: renderer::SandboxFlags,
    /// The origin the tab's initial `about:blank` inherits: the opener's for a
    /// context web content opened, opaque for one the browser created
    /// (<https://html.spec.whatwg.org/multipage/browsers.html#determining-the-origin>).
    pub(crate) document_origin: url::Origin,
}

impl NewTab {
    /// A tab the browser created itself: no name, no opener, no sandbox, and
    /// an opaque initial document.
    pub(crate) fn browser_created() -> Self {
        Self {
            name: None,
            opener: None,
            opener_frame: None,
            popup_sandbox: renderer::SandboxFlags::empty(),
            document_origin: url::Origin::new_opaque(),
        }
    }
}

/// Browser-owned tab state: identity, URL, navigation, and renderer link.
struct Tab {
    id: TabId,
    renderers: Arc<RendererProcessManager>,
    network: TabNetworkHandle,
    renderer: Option<Arc<Assignment>>,
    pending_mount: Option<Mount>,
    /// Target names and opener links for the browsing context group. The
    /// registry is authoritative for the cross-tab `window.open` lookup; the
    /// document-side copy lives in the renderer's frame tree and mirrors it
    /// (<https://html.spec.whatwg.org/multipage/document-sequences.html#the-rules-for-choosing-a-navigable>).
    tabs: TabRegistry,
    /// The origin the active document runs under. `about:blank` keeps it
    /// because the blank inherits its creator's origin rather than deriving
    /// one from its own URL; every other document reports its URL's origin,
    /// which is the document's own for a tuple origin and opaque otherwise
    /// (<https://html.spec.whatwg.org/multipage/browsers.html#determining-the-origin>).
    document_origin: url::Origin,
    /// The popup sandboxing flag set of this tab: empty unless web content
    /// opened it from a document whose sandbox propagates. It is copied into
    /// every mount, so every document this tab installs inherits it
    /// (<https://html.spec.whatwg.org/multipage/browsers.html#popup-sandboxing-flag-set>).
    popup_sandbox: renderer::SandboxFlags,
    site: Option<Site>,
    events_rx: Option<mpsc::Receiver<RendererWake>>,
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
        tabs: TabRegistry,
        tab: NewTab,
    ) -> Self {
        let (dial_tx, dial_rx) = mpsc::unbounded_channel();
        let initial_name = tabs.name(id);
        let pending_mount = blank_mount(renderer::TabState {
            name: initial_name.clone(),
            auxiliary_with_opener: tabs.opener(id).is_some(),
            popup_sandbox: tab.popup_sandbox,
        });
        Self {
            id,
            renderers,
            network,
            renderer: None,
            pending_mount: Some(pending_mount),
            tabs,
            document_origin: tab.document_origin,
            popup_sandbox: tab.popup_sandbox,
            site: None,
            events_rx: None,
            document_url: net::about_blank(),
            viewport: renderer::DEFAULT_VIEWPORT,
            init_scripts: Vec::new(),
            next_init_script: 1,
            history: SessionHistory::new(net::about_blank(), initial_name),
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
        let url = self.document_url.clone();
        let site = Site::for_url(&url)
            .or_else(|| self.site.clone())
            .unwrap_or_else(|| Site::opaque(self.id));
        let mount = Mount {
            head: ResponseHead {
                history: self.history.renderer_snapshot(),
                viewport: Some(self.viewport),
                init_scripts: self.init_scripts.clone(),
                ..ResponseHead::local(
                    url.clone(),
                    Some("text/html; charset=utf-8".to_owned()),
                    None,
                )
            },
            tab: self.current_tab_state(),
            body: html.as_bytes().to_vec(),
        };
        if self.renderer.is_none() && self.document_url.scheme() == "about" {
            self.pending_mount = Some(mount);
            self.document_loaded = true;
            // The deferred blank document still fired parsing's end, so both
            // readiness events happen before `load`.
            self.record_event(TabEvent::DomContentLoaded).await;
            self.record_event(TabEvent::Load).await;
            self.history.set_current_title(String::new());
            return Ok(());
        }
        self.mount(&site, mount).await?;
        let title = self.document_title().await;
        self.history.set_current_title(title);
        Ok(())
    }

    /// The mounted document's title, recorded on its history entry. A tab
    /// without a renderer has no document title.
    async fn document_title(&mut self) -> String {
        if self.renderer.is_none() {
            return String::new();
        }
        match execute_script(
            self,
            FrameId::MAIN,
            renderer::ScriptSource::Browser("String(document.title)".to_owned()),
            None,
        )
        .await
        {
            Ok(RemoteValue::String(title)) => title,
            _ => String::new(),
        }
    }

    fn goto(&mut self, spec: &str, initiator: Option<NavigationInitiator>) -> Result<(), TabError> {
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
            initiator,
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
        let result = self
            .renderer
            .as_ref()
            .ok_or(TabError::ActorStopped)?
            .mount(FrameId::MAIN, mount)
            .await
            .and_then(reply_unit);
        if result.is_err() {
            self.drop_renderer();
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
        }
        result
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
    async fn deliver_window_message(
        &mut self,
        RemoteWindowMessage { message }: RemoteWindowMessage,
    ) -> Result<(), TabError> {
        if self.renderer.is_none() {
            self.mount_virtual().await?;
        }
        match self
            .renderer_request(RendererCommand::WindowMessage(message))
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

    /// The readiness and URL of the renderer document that produced the
    /// latest event.
    async fn renderer_document_state(&mut self) -> Option<(String, String)> {
        self.renderer.as_ref()?;
        let reply = self
            .renderer_request(RendererCommand::ExecuteScript {
                frame: FrameId::MAIN,
                source: ScriptSource::Page("document.readyState + '\\n' + document.URL".to_owned()),
                timeout_ms: Some(1_000),
            })
            .await;
        let Ok(Reply::Value(Ok(RemoteValue::String(state)))) = reply else {
            return None;
        };
        let (ready, url) = state.split_once('\n')?;
        Some((ready.to_owned(), url.to_owned()))
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
        let mut mount = self
            .pending_mount
            .take()
            .unwrap_or_else(|| blank_mount(self.current_tab_state()));
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
        self.sync_outgoing_name();
        let state = self.commit_state(&url);
        let commit = self.history_commit(&url, &state);
        let mut history = self.history.clone();
        if !history.commit(commit) {
            self.navigation_failed = true;
            return false;
        }
        let previous_names = self.begin_commit(&state);
        let mount = Mount {
            head: ResponseHead {
                history: history.renderer_snapshot(),
                viewport: Some(self.viewport),
                init_scripts: self.init_scripts.clone(),
                ..ResponseHead::local(url.clone(), Some(content_type), None)
            },
            tab: state,
            body,
        };
        if let Err(error) = self.mount(&site, mount).await {
            self.abort_commit(previous_names);
            logging::error!(
                target: "browser::tab",
                "data navigation mount failed for {}: {error}",
                log_url(&url)
            );
            self.navigation_failed = true;
            return false;
        }
        self.history = history;
        self.advance_document_url(url);
        self.navigation_failed = false;
        let title = self.document_title().await;
        self.history.set_current_title(title);
        true
    }

    /// Whether this tab is an auxiliary browsing context whose opener is
    /// still alive, which keeps its target name across a cross-origin
    /// navigation. The opener link is recorded when the tab is created, so a
    /// tab the browser created itself never qualifies
    /// (<https://html.spec.whatwg.org/multipage/browsing-the-web.html#resetBCName>).
    fn auxiliary_with_opener(&self) -> bool {
        self.tabs
            .opener(self.id)
            .is_some_and(|opener| self.tabs.tab(opener).is_some())
    }

    /// The browsing-context state a mount carries when no document is
    /// committed: the tab's name as it stands, with no reset.
    fn current_tab_state(&self) -> renderer::TabState {
        renderer::TabState {
            name: self.tabs.name(self.id),
            auxiliary_with_opener: self.auxiliary_with_opener(),
            popup_sandbox: self.popup_sandbox,
        }
    }

    /// The browsing-context state a mount at `url` carries: the tab's name
    /// after the commit's target-name reset, the popup sandboxing flag set,
    /// and whether the tab keeps its name across origins
    /// (<https://html.spec.whatwg.org/multipage/browsing-the-web.html#resetBCName>).
    fn tab_state_at(&self, url: &Url) -> renderer::TabState {
        let mut state = self.current_tab_state();
        if !self.keeps_target_name_at(url) {
            state.name = None;
        }
        state
    }

    /// Whether committing a document at `url` keeps this tab's target name.
    ///
    /// A commit clears the name when the new document's origin differs from
    /// the active document's, unless the tab is an auxiliary context with a
    /// live opener
    /// (<https://html.spec.whatwg.org/multipage/browsing-the-web.html#resetBCName>).
    ///
    /// An `about:` document inherits the active document's origin rather than
    /// deriving one from its own URL, so it never crosses one. A sandboxed
    /// document's origin is opaque, and a fresh opaque origin is never
    /// same-origin with the previous document's, so every sandboxed commit
    /// crosses one
    /// (<https://html.spec.whatwg.org/multipage/browsers.html#sandboxed-origin-browsing-context-flag>).
    fn keeps_target_name_at(&self, url: &Url) -> bool {
        if self.auxiliary_with_opener() || url.scheme() == "about" {
            return true;
        }
        !self.popup_sandbox.origin_is_opaque() && url.origin() == self.document_origin
    }

    /// Records that the tab's active document is now at `url`, which is what
    /// the next commit's target-name reset compares against
    /// (<https://html.spec.whatwg.org/multipage/browsing-the-web.html#resetBCName>).
    ///
    /// The name is not touched here: a browser-driven commit resolves it
    /// before the mount, because the document's own scripts may rename the tab
    /// while they run, and a renderer-driven commit is the renderer's to reset
    /// and report.
    fn advance_document_url(&mut self, url: Url) {
        if url.scheme() != "about" {
            self.document_origin = url.origin();
        }
        self.document_url = url;
    }

    /// The browsing-context state a commit of `url` installs.
    ///
    /// A traversal restores the target name stored with the history entry;
    /// every other commit takes the tab's live name through the target-name
    /// reset. "A navigable's target name is its active session history entry's
    /// document state's navigable target name", so the entry, not the live
    /// context, is what a traversal installs
    /// (<https://html.spec.whatwg.org/multipage/document-sequences.html#navigable>,
    /// <https://html.spec.whatwg.org/multipage/browsing-the-web.html#resetBCName>).
    fn commit_state(&self, url: &Url) -> renderer::TabState {
        match self.traversal {
            Some(entry) => renderer::TabState {
                name: self.history.name(entry),
                ..self.current_tab_state()
            },
            None => self.tab_state_at(url),
        }
    }

    /// Applies a commit's browsing-context state and returns the names
    /// [`Self::abort_commit`] restores if the mount fails.
    ///
    /// The registry is written *before* the mount runs the new document's
    /// scripts, so a script that sets `window.name` there is reported after
    /// this value and wins; resolving it after the mount would clobber the
    /// script's name
    /// (<https://html.spec.whatwg.org/multipage/browsing-the-web.html#resetBCName>).
    fn begin_commit(&mut self, state: &renderer::TabState) -> CommitNames {
        let installed = TabNames::from_name(state.name.as_ref());
        let previous = self.tabs.install_commit(self.id, state.name.as_ref());
        CommitNames {
            previous,
            installed,
        }
    }

    /// Undoes [`Self::begin_commit`] when the navigation did not commit and the
    /// previous document is still active.
    fn abort_commit(&mut self, names: CommitNames) {
        self.tabs
            .restore_commit(self.id, names.previous, &names.installed);
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
        // A browser-initiated navigation has no initiator: it sends no
        // referrer, `Sec-Fetch-Site: none`, and a same-site cookie context.
        // The tab's current document is not its initiator.
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
        // The document being replaced may have left `document_loaded` set.
        // The new mount clears it too, but a navigation that never reaches
        // `mount_stream` must not appear loaded.
        self.document_loaded = false;
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
        let final_url = outcome.head.url.clone();
        let site = Site::for_url(&final_url).unwrap_or_else(|| Site::opaque(self.id));
        self.sync_outgoing_name();
        let state = self.commit_state(&final_url);
        let commit = self.history_commit(&final_url, &state);
        let mut history = self.history.clone();
        if !history.commit(commit) {
            logging::error!(
                target: "browser::tab",
                "navigation rejected by history for {}",
                log_url(&final_url)
            );
            self.navigation_failed = true;
            return false;
        }
        let head = ResponseHead {
            history: history.renderer_snapshot(),
            viewport: Some(self.viewport),
            init_scripts: self.init_scripts.clone(),
            ..outcome.head
        };
        let previous_names = self.begin_commit(&state);
        if let Err(error) = self
            .mount_stream(
                &site,
                Mount {
                    head,
                    tab: state,
                    body: Vec::new(),
                },
                outcome.body,
            )
            .await
        {
            self.abort_commit(previous_names);
            logging::error!(
                target: "browser::tab",
                "navigation mount failed for {}: {error}",
                log_url(&final_url)
            );
            self.navigation_failed = true;
            return false;
        }
        self.history = history;
        self.advance_document_url(final_url);
        let title = self.document_title().await;
        self.history.set_current_title(title);
        true
    }

    async fn handle_renderer_event(&mut self, frame: FrameId, event: &BrowserEvent) {
        if frame != FrameId::MAIN {
            return;
        }
        match event {
            BrowserEvent::Navigated { url, outgoing_name } => {
                if let Ok(url) = Url::parse(url) {
                    // This commit was driven by the renderer, which resolved
                    // the reset itself. The event carries the name the document
                    // being replaced had, because the new document's scripts
                    // may already have renamed the tab; the new entry takes the
                    // live name
                    // (<https://html.spec.whatwg.org/multipage/browsing-the-web.html#resetBCName>).
                    self.history.sync_current_name(outgoing_name.as_ref());
                    let name = self.tabs.name(self.id);
                    self.history.commit(Commit::Push {
                        url: url.clone(),
                        name: name.clone(),
                    });
                    self.tabs
                        .settle_document_name(self.id, outgoing_name.as_ref(), name);
                    self.advance_document_url(url);
                }
                self.document_loaded = false;
                self.navigation_failed = false;
                self.record_event(TabEvent::Navigated).await;
            }
            BrowserEvent::Load => {
                // A `load` for the document a navigation replaced can arrive
                // after the carrier committed the new response. Only the
                // document the tab is navigating to may settle the wait.
                let expected = self
                    .nav
                    .as_ref()
                    .map_or_else(|| self.document_url.clone(), |nav| nav.url.clone());
                let Some((ready, current)) = self.renderer_document_state().await else {
                    return;
                };
                if ready != "complete" || current != expected.as_str() {
                    return;
                }
                self.document_loaded = true;
                self.record_event(TabEvent::Load).await;
            }
            BrowserEvent::DomContentLoaded => {
                self.record_event(TabEvent::DomContentLoaded).await;
            }
            BrowserEvent::HistoryUpdated {
                url,
                state,
                handling,
            } => {
                if let Ok(url) = Url::parse(url)
                    && url.origin() == self.document_url.origin()
                {
                    self.history.update(url.clone(), state.clone(), *handling);
                    self.document_url = url;
                    self.record_event(TabEvent::SameDocumentNavigation).await;
                }
            }
            BrowserEvent::HistoryTraversal { delta } => {
                self.traverse_history(*delta).await;
            }
        }
    }

    /// The history commit this navigation makes: a traversal re-populates the
    /// entry it selected, and every other commit pushes a new one.
    fn history_commit(&mut self, url: &Url, state: &renderer::TabState) -> Commit {
        match self.traversal.take() {
            Some(entry) => Commit::Traverse {
                entry,
                url: url.clone(),
            },
            None => Commit::Push {
                url: url.clone(),
                name: state.name.clone(),
            },
        }
    }

    /// Folds the active document state's name into the entry the document is
    /// about to leave.
    ///
    /// `window.name` writes the registry and not the entry, because the entry
    /// outlives the renderer that reported it. The registry keeps the value
    /// apart from the live name so a reset that lands before this commit does
    /// not erase it
    /// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-name>).
    fn sync_outgoing_name(&mut self) {
        let name = self.tabs.document_name(self.id);
        self.history.sync_current_name(name.as_ref());
    }

    async fn traverse_history(&mut self, delta: i32) {
        // A script may rename the tab and traverse in the same turn, so the
        // entry being left takes the value before the traversal reads any.
        self.sync_outgoing_name();
        // `go(0)` reloads the current entry without touching history
        // (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-history-go>).
        if delta == 0 {
            let url = self.document_url.to_string();
            if self.goto(&url, None).is_ok() {
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
                    history: RendererHistorySnapshot {
                        state,
                        index,
                        length: self.history.renderer_snapshot().length,
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
        if self.goto(&url, None).is_ok() {
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

fn blank_mount(tab: renderer::TabState) -> Mount {
    Mount {
        head: ResponseHead::local(
            net::about_blank(),
            Some("text/html; charset=utf-8".to_owned()),
            None,
        ),
        tab,
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
    Renderer(Option<RendererWake>),
    Delivery(Option<RemoteWindowMessage>),
    WaiterDeadline,
}

async fn coordinator_loop(
    mut server: TabServer,
    mut tab: Tab,
    delivery_rx: mpsc::Receiver<RemoteWindowMessage>,
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
                let mut canceled = Vec::new();
                waiters.retain(|waiter| {
                    let keep = waiter.id != id && !waiter.reply.is_closed();
                    if !keep {
                        canceled.push(waiter.renderer_waiter());
                    }
                    keep
                });
                for renderer_id in canceled.into_iter().flatten() {
                    let _result = tab
                        .renderer_request(RendererCommand::CancelWait { id: renderer_id })
                        .await;
                }
            }
            Wake::Command(Some(ServerInput::RequestChunk | ServerInput::RequestEnd))
            | Wake::WaiterDeadline => {}
            Wake::Command(Some(ServerInput::Notify(_)) | None) => break,
            Wake::Navigation(boxed) => {
                if let Some((epoch, result)) = *boxed {
                    tab.handle_navigation(epoch, result).await;
                }
            }
            Wake::Renderer(Some(RendererWake::WaiterSettled(id))) => {
                settle_waiter(&server, &mut waiters, id).await;
            }
            Wake::Renderer(Some(RendererWake::Event(frame, event))) => {
                let navigated = matches!(event, BrowserEvent::Navigated { .. });
                tab.handle_renderer_event(frame, &event).await;
                if navigated {
                    // The frame's realm is gone, so its JS-truth waiters cannot
                    // settle; resolve them instead of waiting to the deadline.
                    resolve_frame_waiters(&mut tab, &server, &mut waiters, frame).await;
                }
            }
            Wake::Renderer(None) => {
                tab.drop_renderer();
                fail_waiters(&mut tab, &server, &mut waiters, &TabError::ActorStopped).await;
            }
            Wake::Delivery(Some(message)) => {
                let _result = tab.deliver_window_message(message).await;
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
    events: &mut Option<mpsc::Receiver<RendererWake>>,
) -> Option<RendererWake> {
    match events {
        Some(receiver) => receiver.recv().await,
        None => pending().await,
    }
}

async fn next_delivery(
    deliveries: &mut Option<mpsc::Receiver<RemoteWindowMessage>>,
) -> Option<RemoteWindowMessage> {
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
            Command::LoadHtml { reply, .. }
            | Command::Goto { reply, .. }
            | Command::TraverseHistory { reply, .. } => reply.is_closed(),
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
            Command::DocumentUrl { reply } | Command::DocumentTitle { reply } => reply.is_closed(),
            Command::History { reply } => reply.is_closed(),
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
            Command::Goto { request, reply } => {
                let _result = reply.send(tab.goto(&request.url, request.initiator));
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
                if serve_js_waiter(tab, waiters, id, frame, source, timeout, reply).await {
                    return TabOutcome::Retained;
                }
            }
            Command::DocumentUrl { reply } => {
                let _result = reply.send(Ok(tab.document_url.to_string()));
            }
            Command::DocumentTitle { reply } => {
                let title = tab.document_title().await;
                let _result = reply.send(Ok(title));
            }
            Command::History { reply } => {
                let _result = reply.send(Ok(tab.history.snapshot()));
            }
            Command::TraverseHistory { delta, reply } => {
                tab.traverse_history(delta).await;
                let _result = reply.send(Ok(()));
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

/// Registers one JS-truth waiter in the renderer and retains the browser-side
/// side of it. Returns whether the waiter was retained.
async fn serve_js_waiter(
    tab: &mut Tab,
    waiters: &mut Vec<Waiter>,
    id: RequestId,
    frame: FrameId,
    source: renderer::ScriptSource,
    timeout: Duration,
    reply: ReplySender<bool>,
) -> bool {
    let renderer_id = id.get();
    let registered = tab
        .renderer_request(RendererCommand::WaitForJs {
            id: renderer_id,
            frame,
            source,
        })
        .await;
    match registered {
        Ok(_) => {
            if retain_js_waiter(waiters, id, frame, renderer_id, timeout, reply) {
                return true;
            }
            let _result = tab
                .renderer_request(RendererCommand::CancelWait { id: renderer_id })
                .await;
            false
        }
        Err(error) => {
            let _result = reply.send(Err(error));
            false
        }
    }
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
            // Completing is driven by `document_loaded`: a `load` from the
            // document a navigation is replacing is filtered by URL.
            kind: WaiterKind::Load,
            reply,
        },
    )
}

fn retain_js_waiter(
    waiters: &mut Vec<Waiter>,
    id: RequestId,
    frame: FrameId,
    renderer_id: u64,
    timeout: Duration,
    reply: ReplySender<bool>,
) -> bool {
    retain_waiter(
        waiters,
        Waiter {
            id,
            deadline: Instant::now() + timeout,
            kind: WaiterKind::Js { renderer_id, frame },
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

async fn fail_waiters(
    tab: &mut Tab,
    server: &TabServer,
    waiters: &mut Vec<Waiter>,
    error: &TabError,
) {
    for waiter in waiters.drain(..) {
        if let Some(id) = waiter.renderer_waiter() {
            let _result = tab
                .renderer_request(RendererCommand::CancelWait { id })
                .await;
        }
        let _result = waiter.reply.send(Err(error.clone()));
        let _ack = server.reply(waiter.id, ()).await;
    }
}

/// Resolves the `RunUntilJs` waiters for a frame that just navigated: the
/// predicate ran against the old realm and cannot settle, so reply `false`
/// instead of polling to the deadline.
async fn resolve_frame_waiters(
    tab: &mut Tab,
    server: &TabServer,
    waiters: &mut Vec<Waiter>,
    frame: FrameId,
) {
    let mut pending = Vec::new();
    for waiter in std::mem::take(waiters) {
        let navigated = matches!(
            waiter.kind,
            WaiterKind::Js {
                frame: wait_frame,
                ..
            } if wait_frame == frame
        );
        if navigated {
            cancel_renderer_waiter(tab, &waiter).await;
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
            cancel_renderer_waiter(tab, &waiter).await;
            continue;
        }
        let result = match waiter.kind {
            // A load wait finishes as soon as the tab stops waiting, before
            // its deadline is considered.
            WaiterKind::Load if !tab.waiting_for_load() => Some(Ok(true)),
            // A JS waiter is settled by the renderer; only its deadline is the
            // browser's business.
            WaiterKind::Load | WaiterKind::Js { .. } if now >= waiter.deadline => Some(Ok(false)),
            WaiterKind::Load | WaiterKind::Js { .. } => None,
        };
        if let Some(result) = result {
            if result == Ok(false) {
                cancel_renderer_waiter(tab, &waiter).await;
            }
            let _result = waiter.reply.send(result);
            let _ack = server.reply(waiter.id, ()).await;
        } else {
            pending.push(waiter);
        }
    }
    *waiters = pending;
}

/// Resolves one renderer-reported waiter, if the browser still knows it.
async fn settle_waiter(server: &TabServer, waiters: &mut Vec<Waiter>, renderer_id: u64) {
    let Some(index) = waiters
        .iter()
        .position(|waiter| waiter.renderer_waiter() == Some(renderer_id))
    else {
        return;
    };
    let waiter = waiters.remove(index);
    let _result = waiter.reply.send(Ok(true));
    let _ack = server.reply(waiter.id, ()).await;
}

async fn cancel_renderer_waiter(tab: &mut Tab, waiter: &Waiter) {
    if let Some(id) = waiter.renderer_waiter() {
        let _result = tab
            .renderer_request(RendererCommand::CancelWait { id })
            .await;
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

fn reply_content_size(reply: Reply) -> Result<(f32, f32), TabError> {
    match reply {
        Reply::ContentSize(result) => result,
        _ => Err(TabError::ActorStopped),
    }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
