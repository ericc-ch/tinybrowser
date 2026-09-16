//! One async Browser bound to one named Profile.

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::path::Path;
use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};
use url::Url;

use crate::actor::{TabHandle, TabId, TabTask};
use crate::manager::RendererProcessManager;
use crate::network::NetworkSession;
use crate::profile::{Profile, ProfileName};
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
    profile: Profile,
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
        reply: oneshot::Sender<()>,
    },
}

struct BrowserState {
    live: bool,
    network: NetworkSession,
    renderers: Arc<RendererProcessManager>,
    tabs: HashMap<TabId, TabTask>,
    next_tab: u64,
}

impl Browser {
    /// Opens a browser on `profile` with cookies under the process XDG data
    /// home and renderer processes.
    ///
    /// # Errors
    ///
    /// Both `XDG_DATA_HOME` and `HOME` are unset or empty.
    pub fn open(profile: &Profile) -> io::Result<Self> {
        Self::open_in(&ProfileStore::data_home()?, profile)
    }

    /// Opens a browser on `profile` with cookies under `data_home` and
    /// renderer processes.
    ///
    /// # Errors
    ///
    /// The profile directory cannot be created, read, or exclusively locked.
    pub fn open_in(data_home: &Path, profile: &Profile) -> io::Result<Self> {
        let network = NetworkSession::from_builder(
            net::AgentBuilder::new(),
            ProfileStore::open_in(data_home, profile)?,
        )?;
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
        let profile = Profile::named(network.profile_name());
        let renderers = Arc::new(RendererProcessManager::new(network.fetch_handle()));
        let state = BrowserState {
            live: true,
            network,
            renderers,
            tabs: HashMap::new(),
            next_tab: 1,
        };
        let (tx, rx) = mpsc::channel(BROWSER_COMMAND_CAPACITY);
        runtime.spawn(browser_loop(rx, state));
        Ok(Self {
            handle: BrowserHandle { profile, tx },
        })
    }

    /// Value-only handle for this browser.
    #[must_use]
    pub fn handle(&self) -> BrowserHandle {
        self.handle.clone()
    }

    /// Profile this browser is bound to.
    #[must_use]
    pub fn profile_name(&self) -> ProfileName {
        self.handle.profile_name()
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        self.handle.request_close();
    }
}

impl BrowserHandle {
    /// Profile this browser is bound to.
    #[must_use]
    pub fn profile(&self) -> Profile {
        self.profile.clone()
    }

    /// Profile name, same as [`BrowserHandle::profile`].
    #[must_use]
    pub fn profile_name(&self) -> ProfileName {
        self.profile.name().clone()
    }

    /// Starts a tab coordinator and returns its handle.
    ///
    /// # Errors
    ///
    /// [`BrowserError::Stopped`] when the owning [`Browser`] has closed.
    pub async fn create_tab(&self) -> Result<TabHandle, BrowserError> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::CreateTab { reply }).await?;
        rx.await.unwrap_or(Err(BrowserError::Stopped))
    }

    /// Live tab identities.
    ///
    /// # Errors
    ///
    /// [`BrowserError::Stopped`] when the browser task has stopped.
    pub async fn tabs(&self) -> Result<Vec<TabId>, BrowserError> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::Tabs { reply }).await?;
        rx.await.map_err(|_| BrowserError::Stopped)
    }

    /// Handle for a live tab.
    ///
    /// # Errors
    ///
    /// [`BrowserError::UnknownTab`] when `id` is not in the registry.
    pub async fn tab(&self, id: TabId) -> Result<TabHandle, BrowserError> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::Tab { id, reply }).await?;
        rx.await.unwrap_or(Err(BrowserError::Stopped))
    }

    /// Stops `id` and waits for its coordinator task.
    ///
    /// # Errors
    ///
    /// [`BrowserError::UnknownTab`] when `id` is not in the registry.
    pub async fn close_tab(&self, id: TabId) -> Result<(), BrowserError> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::CloseTab { id, reply }).await?;
        rx.await.unwrap_or(Err(BrowserError::Stopped))
    }

    /// Whether this browser still accepts commands.
    ///
    /// # Errors
    ///
    /// [`BrowserError::Stopped`] when the browser task has stopped.
    pub async fn is_live(&self) -> Result<bool, BrowserError> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::IsLive { reply }).await?;
        rx.await.map_err(|_| BrowserError::Stopped)
    }

    /// Cookies visible to `url`, including session and `HttpOnly` cookies.
    ///
    /// # Errors
    ///
    /// [`BrowserError::Stopped`] when the browser task has stopped.
    pub async fn cookie_records(&self, url: &Url) -> Result<Vec<net::CookieRecord>, BrowserError> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::CookieRecords {
            url: url.clone(),
            reply,
        })
        .await?;
        rx.await.map_err(|_| BrowserError::Stopped)
    }

    /// Drops every cookie from the live jar.
    ///
    /// # Errors
    ///
    /// [`BrowserError::Stopped`] when the browser task has stopped.
    pub async fn clear_cookies(&self) -> Result<(), BrowserError> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::ClearCookies { reply }).await?;
        rx.await.map_err(|_| BrowserError::Stopped)
    }

    /// Stores one `Set-Cookie` line for `url`.
    ///
    /// # Errors
    ///
    /// [`BrowserError::Stopped`] when the browser task has stopped.
    pub async fn add_cookie(&self, cookie: &str, url: &Url) -> Result<(), BrowserError> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::AddCookie {
            cookie: cookie.to_owned(),
            url: url.clone(),
            reply,
        })
        .await?;
        rx.await.map_err(|_| BrowserError::Stopped)
    }

    /// Stops every tab, persists the profile, and refuses later commands.
    ///
    /// # Errors
    ///
    /// The final durable profile write failed or the browser task stopped.
    pub async fn close(&self) -> io::Result<()> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Command::Close { reply })
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "browser stopped"))?;
        rx.await
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "browser stopped"))?
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

async fn browser_loop(mut commands: mpsc::Receiver<Command>, mut state: BrowserState) {
    while let Some(command) = commands.recv().await {
        match command {
            Command::CreateTab { reply } => {
                let result = if state.live {
                    let id = TabId::new(state.next_tab);
                    state.next_tab = state.next_tab.saturating_add(1);
                    let task = TabTask::spawn(
                        id,
                        state.network.fetch_handle(),
                        Arc::clone(&state.renderers),
                    );
                    let handle = task.handle.clone();
                    state.tabs.insert(id, task);
                    Ok(handle)
                } else {
                    Err(BrowserError::Stopped)
                };
                let _result = reply.send(result);
            }
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
            Command::CloseTab { id, reply } => {
                let result = if let Some(mut task) = state.tabs.remove(&id) {
                    task.shutdown().await;
                    Ok(())
                } else {
                    Err(BrowserError::UnknownTab)
                };
                let _result = reply.send(result);
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
                state.network.add_cookie(&cookie, &url);
                let _result = reply.send(());
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
