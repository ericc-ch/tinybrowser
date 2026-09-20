//! One async Browser bound to one named Profile.

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::path::Path;
use std::sync::Arc;

use renderer::StorageSeed;
use tokio::sync::{mpsc, oneshot};
use url::Url;

use crate::actor::{TabHandle, TabId, TabTask};
use crate::manager::RendererProcessManager;
use crate::network::NetworkSession;
use crate::profile::Profile;
use crate::store::ProfileStore;

const BROWSER_COMMAND_CAPACITY: usize = 256;

/// Process-owned engine for one profile.
///
/// Renderer processes are the current executable invoked with `renderer` as
/// its first argument. An embedding executable must dispatch that invocation
/// to [`crate::child::serve`].
pub struct Browser {
    handle: BrowserHandle,
}

/// Async value-only handle protocols use to drive [`Browser`].
#[derive(Clone)]
pub struct BrowserHandle {
    tx: mpsc::Sender<Command>,
}

enum Command {
    CreateTab {
        reply: oneshot::Sender<Result<TabHandle, BrowserError>>,
    },
    Tabs {
        reply: oneshot::Sender<Vec<TabId>>,
    },
    Tab {
        id: TabId,
        reply: oneshot::Sender<Result<TabHandle, BrowserError>>,
    },
    CloseTab {
        id: TabId,
        reply: oneshot::Sender<Result<(), BrowserError>>,
    },
    IsLive {
        reply: oneshot::Sender<bool>,
    },
    Close {
        reply: oneshot::Sender<io::Result<()>>,
    },
    CookieRecords {
        url: Url,
        reply: oneshot::Sender<Vec<net::CookieRecord>>,
    },
    ClearCookies {
        reply: oneshot::Sender<()>,
    },
    AddCookie {
        cookie: String,
        url: Url,
        reply: oneshot::Sender<bool>,
    },
    /// Opens one auxiliary browsing context for `window.open`.
    OpenWindow {
        url: String,
        /// Tab that called `window.open`, when the link could resolve it.
        source: Option<TabId>,
        /// The caller asked for `noopener`/`noreferrer`; no opener link.
        noopener: bool,
        /// Session copy for the new tab, when the opener sent one.
        seed: Option<StorageSeed>,
        reply: oneshot::Sender<Result<TabId, BrowserError>>,
    },
    /// Records which tab owns one renderer assignment.
    RegisterAssignment {
        assignment: u64,
        tab: TabId,
    },
    /// Forgets a released renderer assignment, so cross-site navigation does
    /// not accumulate stale entries for the tab's lifetime.
    UnregisterAssignment {
        assignment: u64,
    },
    /// The tab that owns one renderer assignment.
    AssignmentTab {
        assignment: u64,
        reply: oneshot::Sender<Option<TabId>>,
    },
    /// `window.opener` for one assignment's tab.
    OpenerTab {
        assignment: u64,
        reply: oneshot::Sender<Option<TabId>>,
    },
    /// Routes one `postMessage` to a live tab.
    WindowMessage {
        target: TabId,
        payload: String,
        reply: oneshot::Sender<Result<(), BrowserError>>,
    },
    /// Reads one key of a live tab's session area for `origin`.
    RemoteSessionGet {
        target: TabId,
        origin: String,
        key: String,
        reply: oneshot::Sender<Result<Option<String>, BrowserError>>,
    },
}

struct BrowserState {
    live: bool,
    network: NetworkSession,
    renderers: Arc<RendererProcessManager>,
    browser: BrowserHandle,
    tabs: HashMap<TabId, TabTask>,
    next_tab: u64,
    /// Which tab owns each renderer assignment.
    assignments: HashMap<u64, TabId>,
    /// Auxiliary browsing context relationships: openee -> opener.
    openers: HashMap<TabId, TabId>,
}

impl Browser {
    /// Opens a browser on `profile` with cookies under `data_home`, renderer
    /// processes, and default transport settings.
    ///
    /// # Errors
    ///
    /// The profile directory cannot be created, read, or exclusively locked.
    pub fn open_in(data_home: &Path, profile: &Profile) -> io::Result<Self> {
        Self::open_in_with(data_home, profile, net::AgentBuilder::new())
    }

    /// Opens a browser on `profile` with cookies under `data_home`, renderer
    /// processes, and `builder`'s transport settings.
    ///
    /// # Errors
    ///
    /// The profile directory cannot be created, read, or exclusively locked;
    /// stored profile data cannot be loaded; or the caller is not running
    /// inside the executable-owned Tokio runtime.
    pub fn open_in_with(
        data_home: &Path,
        profile: &Profile,
        builder: net::AgentBuilder,
    ) -> io::Result<Self> {
        let network =
            NetworkSession::from_builder(builder, ProfileStore::open_in(data_home, profile)?)?;
        Self::open_with_network(network)
    }

    /// Opens a browser that shares `network` and its cookie jar with renderer
    /// processes.
    ///
    /// # Errors
    ///
    /// The caller is not running inside the executable-owned Tokio runtime.
    pub fn open_with_network(network: NetworkSession) -> io::Result<Self> {
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|error| io::Error::other(format!("browser runtime unavailable: {error}")))?;
        let (tx, rx) = mpsc::channel(BROWSER_COMMAND_CAPACITY);
        let handle = BrowserHandle { tx };
        // Renderer links create tabs for `window.open`, so they get the same
        // command handle the adapters use.
        let renderers = Arc::new(RendererProcessManager::new(
            network.fetch_handle(),
            handle.clone(),
        ));
        let state = BrowserState {
            live: true,
            network,
            renderers,
            browser: handle.clone(),
            tabs: HashMap::new(),
            next_tab: 1,
            assignments: HashMap::new(),
            openers: HashMap::new(),
        };
        runtime.spawn(browser_loop(rx, state));
        Ok(Self { handle })
    }

    /// Value-only handle for this browser.
    #[must_use]
    pub fn handle(&self) -> BrowserHandle {
        self.handle.clone()
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        self.handle.request_close();
    }
}

impl BrowserHandle {
    /// Starts a tab coordinator and returns its handle.
    ///
    /// # Errors
    ///
    /// [`BrowserError::Stopped`] when the owning [`Browser`] has closed.
    pub async fn create_tab(&self) -> Result<TabHandle, BrowserError> {
        self.request(|reply| Command::CreateTab { reply })
            .await
            .and_then(|result| result)
    }

    /// Live tab identities.
    ///
    /// # Errors
    ///
    /// [`BrowserError::Stopped`] when the browser task has stopped.
    pub async fn tabs(&self) -> Result<Vec<TabId>, BrowserError> {
        self.request(|reply| Command::Tabs { reply }).await
    }

    /// Handle for a live tab.
    ///
    /// # Errors
    ///
    /// [`BrowserError::UnknownTab`] when `id` is not in the registry.
    pub async fn tab(&self, id: TabId) -> Result<TabHandle, BrowserError> {
        self.request(|reply| Command::Tab { id, reply })
            .await
            .and_then(|result| result)
    }

    /// Stops `id` and waits for its coordinator task.
    ///
    /// # Errors
    ///
    /// [`BrowserError::UnknownTab`] when `id` is not in the registry.
    pub async fn close_tab(&self, id: TabId) -> Result<(), BrowserError> {
        self.request(|reply| Command::CloseTab { id, reply })
            .await
            .and_then(|result| result)
    }

    /// Whether this browser still accepts commands.
    ///
    /// # Errors
    ///
    /// [`BrowserError::Stopped`] when the browser task has stopped.
    pub async fn is_live(&self) -> Result<bool, BrowserError> {
        self.request(|reply| Command::IsLive { reply }).await
    }

    /// Cookies visible to `url`, including session and `HttpOnly` cookies.
    ///
    /// # Errors
    ///
    /// [`BrowserError::Stopped`] when the browser task has stopped.
    pub async fn cookie_records(&self, url: &Url) -> Result<Vec<net::CookieRecord>, BrowserError> {
        let url = url.clone();
        self.request(move |reply| Command::CookieRecords { url, reply })
            .await
    }

    /// Drops every cookie from the live jar.
    ///
    /// # Errors
    ///
    /// [`BrowserError::Stopped`] when the browser task has stopped.
    pub async fn clear_cookies(&self) -> Result<(), BrowserError> {
        self.request(|reply| Command::ClearCookies { reply }).await
    }

    /// Stores one `Set-Cookie` line for `url` with HTTP-level rules,
    /// returning whether the jar stored it.
    ///
    /// # Errors
    ///
    /// [`BrowserError::Stopped`] when the browser task has stopped.
    pub async fn add_cookie(&self, cookie: &str, url: &Url) -> Result<bool, BrowserError> {
        let cookie = cookie.to_owned();
        let url = url.clone();
        self.request(move |reply| Command::AddCookie { cookie, url, reply })
            .await
    }

    /// Opens one auxiliary browsing context for `window.open`; an empty `url`
    /// leaves the new tab on `about:blank`. `source` records the opener
    /// relationship unless `noopener` is set.
    ///
    /// # Errors
    ///
    /// [`BrowserError::Stopped`] when the browser task has stopped.
    pub async fn open_window(
        &self,
        url: String,
        source: Option<TabId>,
        noopener: bool,
        seed: Option<StorageSeed>,
    ) -> Result<TabId, BrowserError> {
        self.request(move |reply| Command::OpenWindow {
            url,
            source,
            noopener,
            seed,
            reply,
        })
        .await
        .and_then(|result| result)
    }

    /// Records which tab owns one renderer assignment.
    ///
    /// # Errors
    ///
    /// [`BrowserError::Stopped`] when the browser task has stopped.
    pub async fn register_assignment(
        &self,
        assignment: u64,
        tab: TabId,
    ) -> Result<(), BrowserError> {
        self.send(Command::RegisterAssignment { assignment, tab })
            .await
    }

    /// Forgets a released renderer assignment.
    ///
    /// # Errors
    ///
    /// [`BrowserError::Stopped`] when the browser task has stopped.
    pub async fn unregister_assignment(&self, assignment: u64) -> Result<(), BrowserError> {
        self.send(Command::UnregisterAssignment { assignment })
            .await
    }

    /// The tab that owns one renderer assignment.
    ///
    /// # Errors
    ///
    /// [`BrowserError::Stopped`] when the browser task has stopped.
    pub async fn assignment_tab(&self, assignment: u64) -> Result<Option<TabId>, BrowserError> {
        self.request(move |reply| Command::AssignmentTab { assignment, reply })
            .await
    }

    /// `window.opener` for one assignment's tab.
    ///
    /// # Errors
    ///
    /// [`BrowserError::Stopped`] when the browser task has stopped.
    pub async fn opener_tab(&self, assignment: u64) -> Result<Option<TabId>, BrowserError> {
        self.request(move |reply| Command::OpenerTab { assignment, reply })
            .await
    }

    /// Routes one `postMessage` payload to `target`.
    ///
    /// # Errors
    ///
    /// [`BrowserError::UnknownTab`] when the target is gone.
    pub async fn window_message(&self, target: TabId, payload: String) -> Result<(), BrowserError> {
        self.request(move |reply| Command::WindowMessage {
            target,
            payload,
            reply,
        })
        .await
        .and_then(|result| result)
    }

    /// Reads one key of `target`'s session area for `origin`.
    ///
    /// # Errors
    ///
    /// [`BrowserError::UnknownTab`] when the target is gone.
    pub async fn remote_session_get(
        &self,
        target: TabId,
        origin: String,
        key: String,
    ) -> Result<Option<String>, BrowserError> {
        self.request(move |reply| Command::RemoteSessionGet {
            target,
            origin,
            key,
            reply,
        })
        .await
        .and_then(|result| result)
    }

    /// Stops every tab, persists the profile, and refuses later commands.
    ///
    /// # Errors
    ///
    /// The final durable profile write failed or the browser task stopped.
    pub async fn close(&self) -> io::Result<()> {
        self.request(|reply| Command::Close { reply })
            .await
            .map_err(|_| stopped())?
    }

    /// Sends one command and waits for its reply.
    async fn request<T>(
        &self,
        command: impl FnOnce(oneshot::Sender<T>) -> Command,
    ) -> Result<T, BrowserError> {
        let (reply, rx) = oneshot::channel();
        self.send(command(reply)).await?;
        rx.await.map_err(|_| BrowserError::Stopped)
    }

    async fn send(&self, command: Command) -> Result<(), BrowserError> {
        self.tx
            .send(command)
            .await
            .map_err(|_| BrowserError::Stopped)
    }

    fn request_close(&self) {
        let (reply, _rx) = oneshot::channel();
        let _result = self.tx.try_send(Command::Close { reply });
    }
}

fn stopped() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "browser stopped")
}

/// Creates one tab coordinator; `window.open` and `Target.createTarget` both
/// land here.
fn create_tab(state: &mut BrowserState) -> Result<TabHandle, BrowserError> {
    if !state.live {
        return Err(BrowserError::Stopped);
    }
    let id = TabId::new(state.next_tab);
    state.next_tab = state.next_tab.saturating_add(1);
    let task = TabTask::spawn(
        id,
        state.network.fetch_handle(),
        Arc::clone(&state.renderers),
        state.browser.clone(),
    );
    let handle = task.handle.clone();
    state.tabs.insert(id, task);
    Ok(handle)
}

/// Opens one auxiliary browsing context and starts its first navigation in
/// the background.
fn open_window(
    state: &mut BrowserState,
    url: String,
    source: Option<TabId>,
    noopener: bool,
    seed: Option<StorageSeed>,
) -> Result<TabId, BrowserError> {
    let handle = create_tab(state)?;
    let id = handle.id();
    if let (Some(source), false) = (source, noopener) {
        state.openers.insert(id, source);
    }
    let navigate = !url.is_empty() && url != "about:blank";
    if let Some(seed) = seed {
        // The seed must land before the first navigation runs the new
        // document's scripts, so one task performs both in order.
        tokio::spawn(async move {
            let _result = handle.seed_session(seed).await;
            if navigate {
                let _result = handle.goto(&url).await;
            }
        });
    } else if navigate {
        // The tab starts on about:blank; the first real navigation runs in
        // the background so one slow load cannot stall the command loop.
        tokio::spawn(async move {
            let _result = handle.goto(&url).await;
        });
    }
    Ok(id)
}

/// Routes one `postMessage` payload to a live tab's main frame.
async fn route_window_message(
    state: &BrowserState,
    target: TabId,
    payload: String,
) -> Result<(), BrowserError> {
    match state.tabs.get(&target) {
        Some(task) => task
            .handle
            .window_message(payload)
            .await
            .map_err(|_| BrowserError::UnknownTab),
        None => Err(BrowserError::UnknownTab),
    }
}

/// Reads one key of a live tab's session area.
async fn route_remote_session(
    state: &BrowserState,
    target: TabId,
    origin: String,
    key: String,
) -> Result<Option<String>, BrowserError> {
    match state.tabs.get(&target) {
        Some(task) => task
            .handle
            .remote_session_get(origin, key)
            .await
            .map_err(|_| BrowserError::UnknownTab),
        None => Err(BrowserError::UnknownTab),
    }
}

/// `window.opener` for one renderer assignment.
fn opener_tab(state: &BrowserState, assignment: u64) -> Option<TabId> {
    state
        .assignments
        .get(&assignment)
        .and_then(|tab| state.openers.get(tab))
        .copied()
}

/// Answers one assignment lookup without growing the command loop.
fn route_assignment_tab(
    state: &BrowserState,
    assignment: u64,
    reply: oneshot::Sender<Option<TabId>>,
) {
    let _result = reply.send(state.assignments.get(&assignment).copied());
}

/// Synchronous state queries; returns the command back when it needs awaits.
fn route_sync(state: &mut BrowserState, command: Command) -> Option<Command> {
    match command {
        Command::Tabs { reply } => {
            let tabs = state.tabs.keys().copied().collect();
            let _result = reply.send(tabs);
        }
        Command::Tab { id, reply } => {
            let tab = state
                .tabs
                .get(&id)
                .map(|task| task.handle.clone())
                .ok_or(BrowserError::UnknownTab);
            let _result = reply.send(tab);
        }
        Command::IsLive { reply } => {
            let _result = reply.send(state.live);
        }
        Command::CookieRecords { url, reply } => {
            let _result = reply.send(state.network.cookie_records(&url));
        }
        Command::ClearCookies { reply } => {
            state.network.clear_cookies();
            let _result = reply.send(());
        }
        Command::AddCookie { cookie, url, reply } => {
            let _result = reply.send(state.network.add_cookie(&cookie, &url));
        }
        command => return Some(command),
    }
    None
}

async fn browser_loop(mut commands: mpsc::Receiver<Command>, mut state: BrowserState) {
    while let Some(command) = commands.recv().await {
        let Some(command) = route_sync(&mut state, command) else {
            continue;
        };
        match command {
            // Synchronous queries never reach the loop; `route_sync` answers
            // them above.
            Command::Tabs { .. }
            | Command::Tab { .. }
            | Command::IsLive { .. }
            | Command::CookieRecords { .. }
            | Command::ClearCookies { .. }
            | Command::AddCookie { .. } => {
                unreachable!("route_sync answers every synchronous query")
            }
            Command::CreateTab { reply } => {
                let result = create_tab(&mut state);
                let _result = reply.send(result);
            }
            Command::OpenWindow {
                url,
                source,
                noopener,
                seed,
                reply,
            } => {
                let result = open_window(&mut state, url, source, noopener, seed);
                let _result = reply.send(result);
            }
            Command::RegisterAssignment { assignment, tab } => {
                state.assignments.insert(assignment, tab);
            }
            Command::UnregisterAssignment { assignment } => {
                state.assignments.remove(&assignment);
            }
            Command::AssignmentTab { assignment, reply } => {
                route_assignment_tab(&state, assignment, reply);
            }
            Command::OpenerTab { assignment, reply } => {
                let _result = reply.send(opener_tab(&state, assignment));
            }
            Command::WindowMessage {
                target,
                payload,
                reply,
            } => {
                let result = route_window_message(&state, target, payload).await;
                let _result = reply.send(result);
            }
            Command::RemoteSessionGet {
                target,
                origin,
                key,
                reply,
            } => {
                let result = route_remote_session(&state, target, origin, key).await;
                let _result = reply.send(result);
            }
            Command::CloseTab { id, reply } => {
                let result = if let Some(mut task) = state.tabs.remove(&id) {
                    state.assignments.retain(|_, tab| *tab != id);
                    state.openers.remove(&id);
                    task.shutdown().await;
                    Ok(())
                } else {
                    Err(BrowserError::UnknownTab)
                };
                let _result = reply.send(result);
            }
            Command::Close { reply } => {
                state.live = false;
                close_all(&mut state.tabs).await;
                match state.network.persist().await {
                    Ok(()) => {
                        let _result = reply.send(Ok(()));
                        return;
                    }
                    Err(error) => {
                        let _result = reply.send(Err(error));
                    }
                }
            }
        }
    }
    // Every command sender is gone: this is the drop path when
    // `Browser::drop` could not queue `Command::Close`. Persist so a
    // full channel never loses the profile's cookies.
    close_all(&mut state.tabs).await;
    if state.live {
        state.live = false;
        let _result = state.network.persist().await;
    }
}

async fn close_all(tabs: &mut HashMap<TabId, TabTask>) {
    for mut task in tabs.drain().map(|(_, task)| task) {
        task.shutdown().await;
    }
}

/// Why a browser handle call was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserError {
    /// No tab with that id is live.
    UnknownTab,
    /// The owning [`Browser`] has stopped.
    Stopped,
}

impl fmt::Display for BrowserError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownTab => f.write_str("unknown tab"),
            Self::Stopped => f.write_str("browser stopped"),
        }
    }
}

impl std::error::Error for BrowserError {}
