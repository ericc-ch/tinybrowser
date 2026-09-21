//! One async Browser bound to one named Profile.

use std::collections::HashMap;
use std::convert::Infallible;
use std::fmt;
use std::io;
use std::path::Path;
use std::sync::Arc;

use url::Url;

use crate::actor::{TabHandle, TabId, TabTask};
use crate::context::BrowserContext;
use crate::exchange::{self, ServerInput};
use crate::manager::RendererProcessManager;
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
    client: BrowserClient,
}

enum Command {
    CreateTab,
    Tabs,
    Tab {
        id: TabId,
    },
    CloseTab {
        id: TabId,
    },
    IsLive,
    Close,
    CookieRecords {
        url: Url,
    },
    ClearCookies,
    AddCookie {
        cookie: String,
        url: Url,
    },
    /// Opens one auxiliary browsing context for `window.open`.
    OpenWindow {
        url: String,
        /// Tab that called `window.open`, when the link could resolve it.
        source: Option<TabId>,
        /// The caller asked for `noopener`/`noreferrer`; no opener link.
        noopener: bool,
    },
    /// `window.opener` for one tab.
    OpenerTab {
        tab: TabId,
    },
    /// Routes one `postMessage` to a live tab.
    WindowMessage {
        target: TabId,
        payload: String,
    },
}

enum Reply {
    CreateTab(Result<TabHandle, BrowserError>),
    Tabs(Vec<TabId>),
    Tab(Result<TabHandle, BrowserError>),
    CloseTab(Result<(), BrowserError>),
    IsLive(bool),
    Close(io::Result<()>),
    CookieRecords(Vec<net::CookieRecord>),
    ClearCookies,
    AddCookie(bool),
    OpenWindow(Result<TabId, BrowserError>),
    OpenerTab(Option<TabId>),
    WindowMessage(Result<(), BrowserError>),
}

#[derive(Clone)]
enum Notice {
    Close,
}

type BrowserClient = exchange::Client<Command, Infallible, Notice, Reply, Infallible>;
type BrowserServer = exchange::Server<Infallible, Reply, Infallible, Command, Notice>;

struct BrowserState {
    live: bool,
    context: BrowserContext,
    renderers: Arc<RendererProcessManager>,
    tabs: HashMap<TabId, TabTask>,
    next_tab: u64,
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
        Self::open_in_with_network(data_home, profile, net::AgentBuilder::new())
    }

    /// Opens a browser on `profile` with cookies under `data_home`, renderer
    /// processes, and `builder`'s transport settings.
    ///
    /// # Errors
    ///
    /// The profile directory cannot be created, read, or exclusively locked;
    /// stored profile data cannot be loaded; or the caller is not running
    /// inside the executable-owned Tokio runtime.
    pub fn open_in_with_network(
        data_home: &Path,
        profile: &Profile,
        builder: net::AgentBuilder,
    ) -> io::Result<Self> {
        let context = BrowserContext::open(ProfileStore::open_in(data_home, profile)?, builder)?;
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|error| io::Error::other(format!("browser runtime unavailable: {error}")))?;
        let (client, server) = exchange::local(BROWSER_COMMAND_CAPACITY);
        let handle = BrowserHandle { client };
        // Renderer links create tabs for `window.open`, so they get the same
        // command handle the adapters use.
        let renderers = Arc::new(RendererProcessManager::new(
            context.partition_services(),
            context.session_storage(),
            handle.clone(),
        ));
        let state = BrowserState {
            live: true,
            context,
            renderers,
            tabs: HashMap::new(),
            next_tab: 1,
            openers: HashMap::new(),
        };
        runtime.spawn(browser_loop(server, state));
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
        match self.call(Command::CreateTab).await? {
            Reply::CreateTab(result) => result,
            _ => Err(BrowserError::Protocol),
        }
    }

    /// Live tab identities.
    ///
    /// # Errors
    ///
    /// [`BrowserError::Stopped`] when the browser task has stopped.
    pub async fn tabs(&self) -> Result<Vec<TabId>, BrowserError> {
        match self.call(Command::Tabs).await? {
            Reply::Tabs(tabs) => Ok(tabs),
            _ => Err(BrowserError::Protocol),
        }
    }

    /// Handle for a live tab.
    ///
    /// # Errors
    ///
    /// [`BrowserError::UnknownTab`] when `id` is not in the registry.
    pub async fn tab(&self, id: TabId) -> Result<TabHandle, BrowserError> {
        match self.call(Command::Tab { id }).await? {
            Reply::Tab(result) => result,
            _ => Err(BrowserError::Protocol),
        }
    }

    /// Stops `id` and waits for its coordinator task.
    ///
    /// # Errors
    ///
    /// [`BrowserError::UnknownTab`] when `id` is not in the registry.
    pub async fn close_tab(&self, id: TabId) -> Result<(), BrowserError> {
        match self.call(Command::CloseTab { id }).await? {
            Reply::CloseTab(result) => result,
            _ => Err(BrowserError::Protocol),
        }
    }

    /// Whether this browser still accepts commands.
    ///
    /// # Errors
    ///
    /// [`BrowserError::Stopped`] when the browser task has stopped.
    pub async fn is_live(&self) -> Result<bool, BrowserError> {
        match self.call(Command::IsLive).await? {
            Reply::IsLive(live) => Ok(live),
            _ => Err(BrowserError::Protocol),
        }
    }

    /// Cookies visible to `url`, including session and `HttpOnly` cookies.
    ///
    /// # Errors
    ///
    /// [`BrowserError::Stopped`] when the browser task has stopped.
    pub async fn cookie_records(&self, url: &Url) -> Result<Vec<net::CookieRecord>, BrowserError> {
        match self
            .call(Command::CookieRecords { url: url.clone() })
            .await?
        {
            Reply::CookieRecords(records) => Ok(records),
            _ => Err(BrowserError::Protocol),
        }
    }

    /// Drops every cookie from the live jar.
    ///
    /// # Errors
    ///
    /// [`BrowserError::Stopped`] when the browser task has stopped.
    pub async fn clear_cookies(&self) -> Result<(), BrowserError> {
        match self.call(Command::ClearCookies).await? {
            Reply::ClearCookies => Ok(()),
            _ => Err(BrowserError::Protocol),
        }
    }

    /// Stores one `Set-Cookie` line for `url` with HTTP-level rules,
    /// returning whether the jar stored it.
    ///
    /// # Errors
    ///
    /// [`BrowserError::Stopped`] when the browser task has stopped.
    pub async fn add_cookie(&self, cookie: &str, url: &Url) -> Result<bool, BrowserError> {
        match self
            .call(Command::AddCookie {
                cookie: cookie.to_owned(),
                url: url.clone(),
            })
            .await?
        {
            Reply::AddCookie(stored) => Ok(stored),
            _ => Err(BrowserError::Protocol),
        }
    }

    /// Opens one auxiliary browsing context for `window.open`; an empty `url`
    /// leaves the new tab on `about:blank`. `source` records the opener
    /// relationship unless `noopener` is set.
    ///
    /// # Errors
    ///
    /// [`BrowserError::Stopped`] when the browser task has stopped.
    pub(crate) async fn open_window(
        &self,
        url: String,
        source: Option<TabId>,
        noopener: bool,
    ) -> Result<TabId, BrowserError> {
        match self
            .call(Command::OpenWindow {
                url,
                source,
                noopener,
            })
            .await?
        {
            Reply::OpenWindow(result) => result,
            _ => Err(BrowserError::Protocol),
        }
    }

    /// `window.opener` for one tab.
    ///
    /// # Errors
    ///
    /// [`BrowserError::Stopped`] when the browser task has stopped.
    pub(crate) async fn opener_tab(&self, tab: TabId) -> Result<Option<TabId>, BrowserError> {
        match self.call(Command::OpenerTab { tab }).await? {
            Reply::OpenerTab(tab) => Ok(tab),
            _ => Err(BrowserError::Protocol),
        }
    }

    /// Routes one `postMessage` payload to `target`.
    ///
    /// # Errors
    ///
    /// [`BrowserError::UnknownTab`] when the target is gone.
    pub(crate) async fn window_message(
        &self,
        target: TabId,
        payload: String,
    ) -> Result<(), BrowserError> {
        match self
            .call(Command::WindowMessage { target, payload })
            .await?
        {
            Reply::WindowMessage(result) => result,
            _ => Err(BrowserError::Protocol),
        }
    }

    /// Stops every tab, persists the profile, and refuses later commands.
    ///
    /// # Errors
    ///
    /// The final durable profile write failed or the browser task stopped.
    pub async fn close(&self) -> io::Result<()> {
        match self.call(Command::Close).await.map_err(|_| stopped())? {
            Reply::Close(result) => result,
            _ => Err(io::Error::other("browser protocol mismatch")),
        }
    }

    async fn call(&self, command: Command) -> Result<Reply, BrowserError> {
        self.client
            .call(command)
            .await
            .map_err(|_| BrowserError::Stopped)
    }

    fn request_close(&self) {
        let _result = self.client.try_notify(Notice::Close);
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
    state.context.create_tab(id);
    let task = TabTask::spawn(
        id,
        state.context.tab_network(),
        Arc::clone(&state.renderers),
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
) -> Result<TabId, BrowserError> {
    let handle = create_tab(state)?;
    let id = handle.id();
    if let (Some(source), false) = (source, noopener) {
        state.openers.insert(id, source);
        state.context.copy_session(source, id);
    }
    let navigate = !url.is_empty() && url != "about:blank";
    if navigate {
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

/// `window.opener` for one tab.
fn opener_tab(state: &BrowserState, tab: TabId) -> Option<TabId> {
    state.openers.get(&tab).copied()
}

/// Synchronous state queries; returns the command back when it needs awaits.
fn route_sync(state: &mut BrowserState, command: Command) -> Result<Reply, Command> {
    match command {
        Command::Tabs => Ok(Reply::Tabs(state.tabs.keys().copied().collect())),
        Command::Tab { id } => {
            let tab = state
                .tabs
                .get(&id)
                .map(|task| task.handle.clone())
                .ok_or(BrowserError::UnknownTab);
            Ok(Reply::Tab(tab))
        }
        Command::IsLive => Ok(Reply::IsLive(state.live)),
        Command::CookieRecords { url } => {
            Ok(Reply::CookieRecords(state.context.cookie_records(&url)))
        }
        Command::ClearCookies => {
            state.context.clear_cookies();
            Ok(Reply::ClearCookies)
        }
        Command::AddCookie { cookie, url } => {
            Ok(Reply::AddCookie(state.context.add_cookie(&cookie, &url)))
        }
        command => Err(command),
    }
}

async fn browser_loop(mut server: BrowserServer, mut state: BrowserState) {
    while let Some(input) = server.recv().await {
        let ServerInput::Call { id, body: command } = input else {
            if matches!(input, ServerInput::Notify(Notice::Close)) {
                state.live = false;
                close_all(&mut state.tabs).await;
                let _result = state.context.persist().await;
                return;
            }
            continue;
        };
        let reply = match route_sync(&mut state, command) {
            Ok(reply) => reply,
            Err(command) => match command {
                // Synchronous queries never reach the loop; `route_sync` answers
                // them above.
                Command::Tabs
                | Command::Tab { .. }
                | Command::IsLive
                | Command::CookieRecords { .. }
                | Command::ClearCookies
                | Command::AddCookie { .. } => {
                    unreachable!("route_sync answers every synchronous query")
                }
                Command::CreateTab => Reply::CreateTab(create_tab(&mut state)),
                Command::OpenWindow {
                    url,
                    source,
                    noopener,
                } => Reply::OpenWindow(open_window(&mut state, url, source, noopener)),
                Command::OpenerTab { tab } => Reply::OpenerTab(opener_tab(&state, tab)),
                Command::WindowMessage { target, payload } => {
                    Reply::WindowMessage(route_window_message(&state, target, payload).await)
                }
                Command::CloseTab { id } => {
                    let result = if let Some(mut task) = state.tabs.remove(&id) {
                        state.openers.remove(&id);
                        task.shutdown().await;
                        state.context.close_tab(id);
                        Ok(())
                    } else {
                        Err(BrowserError::UnknownTab)
                    };
                    Reply::CloseTab(result)
                }
                Command::Close => {
                    state.live = false;
                    close_all(&mut state.tabs).await;
                    Reply::Close(state.context.persist().await)
                }
            },
        };
        let closes = matches!(reply, Reply::Close(Ok(())));
        if server.reply(id, reply).await.is_err() || closes {
            return;
        }
    }
    // Every command sender is gone: this is the drop path when
    // `Browser::drop` could not queue `Command::Close`. Persist so a
    // full channel never loses the profile's cookies.
    close_all(&mut state.tabs).await;
    if state.live {
        state.live = false;
        let _result = state.context.persist().await;
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
    /// The owner returned a reply for a different operation.
    Protocol,
}

impl fmt::Display for BrowserError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownTab => f.write_str("unknown tab"),
            Self::Stopped => f.write_str("browser stopped"),
            Self::Protocol => f.write_str("browser protocol mismatch"),
        }
    }
}

impl std::error::Error for BrowserError {}
