//! One Browser bound to one named Profile.

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::actor::{PageActor, PageHandle, PageId};
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
    pages: HashMap<PageId, PageActor>,
    next_page: u64,
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
                pages: HashMap::new(),
                next_page: 1,
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

    /// Starts a page actor and returns its handle.
    ///
    /// # Errors
    ///
    /// [`BrowserError::Stopped`] when the owning [`Browser`] has been dropped.
    pub fn create_page(&self) -> Result<PageHandle, BrowserError> {
        let mut inner = self.lock();
        if !inner.live {
            return Err(BrowserError::Stopped);
        }
        let id = PageId::new(inner.next_page);
        inner.next_page = inner.next_page.saturating_add(1);
        let fetch = inner.network.fetch_handle();
        let registry = Arc::clone(&inner.registry);
        let actor = PageActor::spawn(id, fetch, registry);
        let handle = actor.handle.clone();
        inner.pages.insert(id, actor);
        Ok(handle)
    }

    /// Live page identities.
    #[must_use]
    pub fn pages(&self) -> Vec<PageId> {
        self.lock().pages.keys().copied().collect()
    }

    /// Handle for a live page.
    ///
    /// # Errors
    ///
    /// [`BrowserError::UnknownPage`] when `id` is not in the registry.
    pub fn page(&self, id: PageId) -> Result<PageHandle, BrowserError> {
        self.lock()
            .pages
            .get(&id)
            .map(|actor| actor.handle.clone())
            .ok_or(BrowserError::UnknownPage)
    }

    /// Stops `id` and joins its actor thread.
    ///
    /// # Errors
    ///
    /// [`BrowserError::UnknownPage`] when `id` is not in the registry.
    pub fn close_page(&self, id: PageId) -> Result<(), BrowserError> {
        let mut inner = self.lock();
        let Some(mut actor) = inner.pages.remove(&id) else {
            return Err(BrowserError::UnknownPage);
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

    /// Stops every page, persists the profile, and refuses later commands.
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
        let actors: Vec<PageActor> = inner.pages.drain().map(|(_, actor)| actor).collect();
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
    /// No page with that id is live.
    UnknownPage,
    /// The owning [`Browser`] has been dropped.
    Stopped,
}

impl fmt::Display for BrowserError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownPage => f.write_str("unknown page"),
            Self::Stopped => f.write_str("browser stopped"),
        }
    }
}

impl std::error::Error for BrowserError {}
