//! Public browser facade and its private command protocol.

mod task;

pub(crate) use self::task::TabRegistry;

use std::convert::Infallible;
use std::fmt;
use std::io;
use std::path::PathBuf;

use url::Url;

use self::task::{BrowserTask, BrowserTaskOptions};
use crate::actor::{TabHandle, TabId};
use crate::context::{BrowserContext, BrowserContextOptions};
use crate::exchange;
use crate::network::NetworkContext;
use crate::profile::Profile;
use crate::profile::default_data_home;

const BROWSER_COMMAND_CAPACITY: usize = 256;

/// Inputs for opening one [`Browser`].
///
/// The default options use the platform data home, the `default` profile, and
/// the browser's default network configuration. Set [`Self::data_home`] to
/// isolate a browser in a caller-selected directory.
///
/// # Examples
///
/// ```no_run
/// use browser::{AgentOptions, Browser, BrowserOptions, Profile};
///
/// #[tokio::main(flavor = "current_thread")]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let browser = Browser::new(BrowserOptions {
///         data_home: Some("/tmp/tinybrowser-example".into()),
///         profile: Profile::default(),
///         network: AgentOptions::default(),
///     })?;
///     browser.close().await?;
///     Ok(())
/// }
/// ```
#[derive(Clone, Debug, Default)]
pub struct BrowserOptions {
    /// Root below which tinybrowser stores profiles, or `None` to use
    /// `XDG_DATA_HOME` and then `$HOME/.local/share`.
    pub data_home: Option<PathBuf>,
    /// Durable profile to open exclusively.
    pub profile: Profile,
    /// HTTP, proxy, resolver, timeout, and TLS configuration.
    pub network: net::AgentOptions,
}

/// Running browser owner for one durable profile.
///
/// Construction restores profile state and starts one owner task that
/// serializes browser commands. Use [`Browser::handle`] for temporary access
/// to the cloneable command handle. Dropping `Browser` requests background
/// shutdown; use [`Browser::close`] when the caller must wait for tab shutdown
/// and profile persistence.
///
/// Renderer processes are the current executable invoked with `renderer` as
/// its first argument. An embedding executable must dispatch that invocation
/// to [`crate::child::serve`].
pub struct Browser {
    handle: BrowserHandle,
}

/// Cloneable command handle for a running [`Browser`].
///
/// Every method sends a typed command through the browser exchange. Cloning
/// this handle creates another sender for the same browser; it does not copy
/// browser or profile state. Borrow the handle through [`Browser::handle`] for
/// immediate calls, and clone it only when another task or service must retain
/// its own sender.
#[derive(Clone)]
pub struct BrowserHandle {
    client: BrowserClient,
}

/// Parameters for a renderer-initiated `window.open`.
pub(crate) struct OpenWindowOptions {
    /// Requested URL, already filtered to a navigable scheme or empty.
    pub(crate) url: String,
    /// Tab that called `window.open`, when it has one.
    pub(crate) source: Option<TabId>,
    /// Whether the new context must not receive an opener.
    pub(crate) noopener: bool,
}

pub(super) enum Command {
    CreateTab,
    Tabs,
    Tab { id: TabId },
    CloseTab { id: TabId },
    Close,
    CookieRecords { url: Url },
    ClearCookies,
    AddCookie { cookie: String, url: Url },
    OpenWindow(OpenWindowOptions),
}

pub(super) enum Reply {
    CreateTab(Result<TabHandle, BrowserError>),
    Tabs(Vec<TabId>),
    Tab(Result<TabHandle, BrowserError>),
    CloseTab(Result<(), BrowserError>),
    Close(io::Result<()>),
    CookieRecords(Vec<net::CookieRecord>),
    ClearCookies,
    AddCookie(bool),
    OpenWindow(Result<TabId, BrowserError>),
}

#[derive(Clone)]
pub(super) enum Notice {
    Close,
}

pub(super) type BrowserClient = exchange::Client<Command, Infallible, Notice, Reply, Infallible>;
pub(super) type BrowserServer = exchange::Server<Infallible, Reply, Infallible, Command, Notice>;

/// One browser operation: its input, command mapping, reply extraction, and
/// owner execution.
///
/// Each [`BrowserHandle`] method builds one of these structs and passes it to
/// [`BrowserHandle::ask`]; [`dispatch`](BrowserTask::dispatch) routes the
/// [`Command`] back into [`serve`](Self::serve). The trait impls live in
/// `task.rs` beside [`BrowserTask`] state, so adding an operation touches the
/// struct here plus one impl and one dispatch arm there.
pub(super) trait BrowserOperation {
    /// Value [`ask`](BrowserHandle::ask) resolves on a matching [`Reply`].
    type Output;
    /// Command sent through the browser exchange.
    fn into_command(self) -> Command;
    /// Reply extraction; `None` means the owner answered for another operation.
    fn unwrap(reply: Reply) -> Option<Self::Output>;
    /// Owner-side execution.
    async fn serve(self, task: &mut BrowserTask) -> Reply;
}

/// Starts a tab coordinator and returns its handle.
pub(super) struct CreateTab;

/// Lists the identities of every live tab.
pub(super) struct ListTabs;

/// Returns the command handle for one live tab.
pub(super) struct GetTab {
    /// Tab to look up.
    pub(super) id: TabId,
}

/// Stops and unregisters one tab.
pub(super) struct CloseTab {
    /// Tab to stop.
    pub(super) id: TabId,
}

/// Returns cookies visible to `url`.
pub(super) struct CookieRecords {
    /// URL whose cookies to return.
    pub(super) url: Url,
}

/// Removes every cookie from the live profile.
pub(super) struct ClearCookies;

/// Stores one `Set-Cookie` line for `url`.
pub(super) struct AddCookie {
    /// Raw `Set-Cookie` line.
    pub(super) cookie: String,
    /// URL the cookie belongs to.
    pub(super) url: Url,
}

impl Browser {
    /// Opens the selected profile and starts its browser owner task.
    ///
    /// This function validates network options, exclusively locks and restores
    /// the profile, and starts renderer-process management. It must run inside
    /// an active Tokio runtime. No tab is created automatically.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::io;
    /// use browser::{Browser, BrowserOptions};
    ///
    /// #[tokio::main(flavor = "current_thread")]
    /// async fn main() -> Result<(), Box<dyn std::error::Error>> {
    ///     let browser = Browser::new(BrowserOptions::default())?;
    ///     let tab = browser.handle().create_tab().await?;
    ///     tab.load_html("<!doctype html><title>Example</title>").await?;
    ///     browser.close().await?;
    ///     Ok(())
    /// }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`BrowserOpenError::RuntimeUnavailable`] outside a Tokio
    /// runtime, [`BrowserOpenError::InvalidNetwork`] when a proxy, resolver, or
    /// TLS option is invalid, or [`BrowserOpenError::Profile`] when the data
    /// home or profile cannot be created, locked, restored, or quarantined.
    pub fn new(options: BrowserOptions) -> Result<Self, BrowserOpenError> {
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| BrowserOpenError::RuntimeUnavailable)?;
        let network =
            NetworkContext::new(options.network).map_err(BrowserOpenError::InvalidNetwork)?;
        let data_home = match options.data_home {
            Some(data_home) => data_home,
            None => default_data_home().map_err(BrowserOpenError::Profile)?,
        };
        let context = BrowserContext::new(BrowserContextOptions {
            data_home: &data_home,
            profile: &options.profile,
            network,
        })
        .map_err(BrowserOpenError::Profile)?;
        let (client, server) = exchange::local(BROWSER_COMMAND_CAPACITY);
        let handle = BrowserHandle { client };
        let task = BrowserTask::new(BrowserTaskOptions {
            server,
            context,
            browser: handle.clone(),
        });
        runtime.spawn(task.run());
        Ok(Self { handle })
    }

    /// Borrows the command handle for this browser.
    ///
    /// The returned reference is suitable for immediate calls and for APIs
    /// that clone a handle internally. Call `.clone()` explicitly before
    /// moving a handle into an independently running task.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use browser::{Browser, BrowserOptions};
    /// # async fn use_browser(browser: &Browser) -> Result<(), browser::BrowserError> {
    /// let tab = browser.handle().create_tab().await?;
    /// # let _ = tab;
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn handle(&self) -> &BrowserHandle {
        &self.handle
    }

    /// Stops every tab, persists profile state, and waits for owner shutdown.
    ///
    /// The browser task terminates even when persistence fails. The returned
    /// error reports that final persistence failure.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use browser::{Browser, BrowserOptions};
    /// # async fn close_browser() -> Result<(), Box<dyn std::error::Error>> {
    /// let browser = Browser::new(BrowserOptions::default())?;
    /// browser.close().await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns when the browser task has stopped before receiving the close
    /// command or when cookies or local storage cannot be persisted.
    pub async fn close(self) -> io::Result<()> {
        self.handle.close().await
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
    /// [`BrowserError::Stopped`] when the owning [`Browser`] has closed, or
    /// [`BrowserError::TabIdExhausted`] after every supported tab identity is
    /// consumed.
    pub async fn create_tab(&self) -> Result<TabHandle, BrowserError> {
        self.ask(CreateTab).await?
    }

    /// Returns the identities of every live tab.
    ///
    /// # Errors
    ///
    /// [`BrowserError::Stopped`] when the browser task has stopped.
    pub async fn tabs(&self) -> Result<Vec<TabId>, BrowserError> {
        self.ask(ListTabs).await
    }

    /// Returns the command handle for live tab `id`.
    ///
    /// # Errors
    ///
    /// [`BrowserError::UnknownTab`] when `id` is not live, or
    /// [`BrowserError::Stopped`] when the browser task has stopped.
    pub async fn tab(&self, id: TabId) -> Result<TabHandle, BrowserError> {
        self.ask(GetTab { id }).await?
    }

    /// Stops and unregisters tab `id`.
    ///
    /// # Errors
    ///
    /// [`BrowserError::UnknownTab`] when `id` is not live, or
    /// [`BrowserError::Stopped`] when the browser task has stopped.
    pub async fn close_tab(&self, id: TabId) -> Result<(), BrowserError> {
        self.ask(CloseTab { id }).await?
    }

    /// Returns whether the browser exchange has disconnected.
    ///
    /// This is a local observation and performs no browser round trip. Another
    /// task may begin shutdown immediately after this method returns `false`,
    /// so callers must still handle [`BrowserError::Stopped`] from operations.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.client.is_closed()
    }

    /// Returns cookies visible to `url`, including session and `HttpOnly`
    /// cookies.
    ///
    /// # Errors
    ///
    /// [`BrowserError::Stopped`] when the browser task has stopped.
    pub async fn cookie_records(&self, url: &Url) -> Result<Vec<net::CookieRecord>, BrowserError> {
        self.ask(CookieRecords { url: url.clone() }).await
    }

    /// Removes every cookie from the live profile.
    ///
    /// # Errors
    ///
    /// [`BrowserError::Stopped`] when the browser task has stopped.
    pub async fn clear_cookies(&self) -> Result<(), BrowserError> {
        self.ask(ClearCookies).await
    }

    /// Stores one `Set-Cookie` line for `url` using HTTP cookie rules.
    ///
    /// Returns `true` when the cookie was accepted and stored, and `false`
    /// when cookie parsing or policy rejected it.
    ///
    /// # Errors
    ///
    /// [`BrowserError::Stopped`] when the browser task has stopped.
    pub async fn add_cookie(&self, cookie: &str, url: &Url) -> Result<bool, BrowserError> {
        self.ask(AddCookie {
            cookie: cookie.to_owned(),
            url: url.clone(),
        })
        .await
    }

    pub(crate) async fn open_window(
        &self,
        request: OpenWindowOptions,
    ) -> Result<TabId, BrowserError> {
        self.ask(request).await?
    }

    /// Stops every tab and persists profile state.
    ///
    /// The browser task terminates after replying, including when persistence
    /// fails. Later calls through any cloned handle return
    /// [`BrowserError::Stopped`].
    ///
    /// # Errors
    ///
    /// Returns when the browser task has already stopped or when cookies or
    /// local storage cannot be persisted.
    pub async fn close(&self) -> io::Result<()> {
        match self.call(Command::Close).await.map_err(|_| stopped())? {
            Reply::Close(result) => {
                self.client.wait_closed().await;
                result
            }
            _ => Err(io::Error::other("browser protocol mismatch")),
        }
    }

    async fn call(&self, command: Command) -> Result<Reply, BrowserError> {
        self.client
            .call(command)
            .await
            .map_err(|_| BrowserError::Stopped)
    }

    async fn ask<Op: BrowserOperation>(&self, op: Op) -> Result<Op::Output, BrowserError> {
        Op::unwrap(self.call(op.into_command()).await?).ok_or(BrowserError::Protocol)
    }

    fn request_close(&self) {
        let _result = self.client.try_notify(Notice::Close);
    }
}

fn stopped() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "browser stopped")
}

/// Failure while opening a [`Browser`].
#[derive(Debug)]
pub enum BrowserOpenError {
    /// The data home or profile could not be created, locked, read, or
    /// quarantined.
    Profile(io::Error),
    /// Network options failed validation.
    InvalidNetwork(net::NetError),
    /// No Tokio runtime was active on the calling thread.
    RuntimeUnavailable,
}

impl fmt::Display for BrowserOpenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Profile(error) => write!(formatter, "profile failed: {error}"),
            Self::InvalidNetwork(error) => write!(formatter, "invalid network options: {error}"),
            Self::RuntimeUnavailable => formatter.write_str("browser runtime unavailable"),
        }
    }
}

impl std::error::Error for BrowserOpenError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Profile(error) => Some(error),
            Self::InvalidNetwork(error) => Some(error),
            Self::RuntimeUnavailable => None,
        }
    }
}

impl From<BrowserOpenError> for io::Error {
    fn from(error: BrowserOpenError) -> Self {
        match error {
            BrowserOpenError::Profile(error) => error,
            BrowserOpenError::InvalidNetwork(error) => {
                Self::new(io::ErrorKind::InvalidInput, error)
            }
            BrowserOpenError::RuntimeUnavailable => Self::other("browser runtime unavailable"),
        }
    }
}

/// Failure while sending a command to the browser owner task.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserError {
    /// No live tab has the requested identity.
    UnknownTab,
    /// Every supported tab identity has been consumed.
    TabIdExhausted,
    /// The browser owner task has stopped.
    Stopped,
    /// The owner returned a reply for a different operation.
    Protocol,
}

impl fmt::Display for BrowserError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownTab => formatter.write_str("unknown tab"),
            Self::TabIdExhausted => formatter.write_str("tab identity space exhausted"),
            Self::Stopped => formatter.write_str("browser stopped"),
            Self::Protocol => formatter.write_str("browser protocol mismatch"),
        }
    }
}

impl std::error::Error for BrowserError {}
