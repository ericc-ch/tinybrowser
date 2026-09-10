//! One Browser bound to one named Profile.

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::actor::{TabActor, TabHandle, TabId};
use crate::link::{RendererRegistry, Renderers};
use crate::network::{NetworkSession, ProfileStore};
use crate::profile::{Profile, ProfileName};

/// Process-owned engine for one profile.
pub struct Browser {
    inner: Arc<Mutex<BrowserInner>>,
}

struct BrowserInner {
    live: bool,
    network: NetworkSession,
    registry: Arc<RendererRegistry>,
    tabs: HashMap<TabId, TabActor>,
    next_tab: u64,
}

/// Value-only handle protocols use to drive [`Browser`].
#[derive(Clone)]
pub struct BrowserHandle {
    inner: Arc<Mutex<BrowserInner>>,
}

impl Browser {
    /// Opens an in-memory browser with in-process renderers whose profile is
    /// discarded at shutdown. Tests and embedding use this fast path.
    ///
    /// # Errors
    ///
    /// The JavaScript or networking session could not be initialized.
    pub fn ephemeral() -> io::Result<Self> {
        Self::with_store(
            ProfileStore::memory(&Profile::default()),
            net::AgentBuilder::new(),
            Renderers::Local,
        )
    }

    /// Opens a browser on `profile` with cookies under the process XDG data
    /// home and renderer processes ([ADR 0011]).
    ///
    /// [ADR 0011]: ../../../docs/adrs/0011-renderer-processes-per-site.md
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
        Self::open_in_with(data_home, profile, Renderers::Process)
    }

    /// [`Browser::open_in`] with an explicit renderer backend.
    ///
    /// # Errors
    ///
    /// The profile directory cannot be created, read, or exclusively locked.
    pub fn open_in_with(
        data_home: &Path,
        profile: &Profile,
        renderers: Renderers,
    ) -> io::Result<Self> {
        Self::with_store(
            ProfileStore::open_in(data_home, profile)?,
            net::AgentBuilder::new(),
            renderers,
        )
    }

    /// Opens a browser that shares `network` (and its cookie jar) with
    /// renderer processes.
    #[must_use]
    pub fn open_with_network(network: NetworkSession) -> Self {
        Self::open_with_network_and(network, Renderers::Process)
    }

    /// [`Browser::open_with_network`] with an explicit renderer backend.
    #[must_use]
    pub fn open_with_network_and(network: NetworkSession, renderers: Renderers) -> Self {
        let registry = Arc::new(RendererRegistry::new(renderers, network.fetch_handle()));
        Self {
            inner: Arc::new(Mutex::new(BrowserInner {
                live: true,
                network,
                registry,
                tabs: HashMap::new(),
                next_tab: 1,
            })),
        }
    }

    fn with_store(
        store: ProfileStore,
        builder: net::AgentBuilder,
        renderers: Renderers,
    ) -> io::Result<Self> {
        NetworkSession::from_builder(builder, store)
            .map(|network| Self::open_with_network_and(network, renderers))
    }

    /// Value-only handle for this browser.
    #[must_use]
    pub fn handle(&self) -> BrowserHandle {
        BrowserHandle {
            inner: Arc::clone(&self.inner),
        }
    }

    /// Profile this browser is bound to.
    #[must_use]
    pub fn profile_name(&self) -> ProfileName {
        self.handle().profile_name()
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _result = self.handle().close();
    }
}

impl BrowserHandle {
    /// Profile this browser is bound to.
    #[must_use]
    pub fn profile(&self) -> Profile {
        Profile::named(self.lock().network.profile_name())
    }

    /// Profile name, same as [`BrowserHandle::profile`].
    #[must_use]
    pub fn profile_name(&self) -> ProfileName {
        self.profile().name().clone()
    }

    /// Starts a tab actor and returns its handle.
    ///
    /// # Errors
    ///
    /// [`BrowserError::Stopped`] when the owning [`Browser`] has been dropped.
    pub fn create_tab(&self) -> Result<TabHandle, BrowserError> {
        let mut inner = self.lock();
        if !inner.live {
            return Err(BrowserError::Stopped);
        }
        let id = TabId::new(inner.next_tab);
        inner.next_tab = inner.next_tab.saturating_add(1);
        let fetch = inner.network.fetch_handle();
        let registry = Arc::clone(&inner.registry);
        let actor = TabActor::spawn(id, fetch, registry);
        let handle = actor.handle.clone();
        inner.tabs.insert(id, actor);
        Ok(handle)
    }

    /// Live tab identities.
    #[must_use]
    pub fn tabs(&self) -> Vec<TabId> {
        self.lock().tabs.keys().copied().collect()
    }

    /// Handle for a live tab.
    ///
    /// # Errors
    ///
    /// [`BrowserError::UnknownTab`] when `id` is not in the registry.
    pub fn tab(&self, id: TabId) -> Result<TabHandle, BrowserError> {
        self.lock()
            .tabs
            .get(&id)
            .map(|actor| actor.handle.clone())
            .ok_or(BrowserError::UnknownTab)
    }

    /// Stops `id` and joins its actor thread.
    ///
    /// # Errors
    ///
    /// [`BrowserError::UnknownTab`] when `id` is not in the registry.
    pub fn close_tab(&self, id: TabId) -> Result<(), BrowserError> {
        let mut inner = self.lock();
        let Some(mut actor) = inner.tabs.remove(&id) else {
            return Err(BrowserError::UnknownTab);
        };
        drop(inner);
        actor.shutdown();
        Ok(())
    }

    /// Whether this browser still accepts commands.
    #[must_use]
    pub fn is_live(&self) -> bool {
        self.lock().live
    }

    /// Stops every tab, persists the profile, and refuses later commands.
    ///
    /// # Errors
    ///
    /// The final durable profile write failed.
    pub fn close(&self) -> io::Result<()> {
        let should_close_pages = {
            let mut inner = self.lock();
            if inner.live {
                inner.live = false;
                true
            } else {
                false
            }
        };
        if should_close_pages {
            self.close_all();
            self.lock().registry.shutdown();
        }
        self.persist()
    }

    fn persist(&self) -> io::Result<()> {
        self.lock().network.persist()
    }

    fn close_all(&self) {
        let mut inner = self.lock();
        let actors: Vec<TabActor> = inner.tabs.drain().map(|(_, actor)| actor).collect();
        drop(inner);
        for mut actor in actors {
            actor.shutdown();
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BrowserInner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Why a browser handle call was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserError {
    /// No tab with that id is live.
    UnknownTab,
    /// The owning [`Browser`] has been dropped.
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
