//! One Browser bound to one named Profile.
//!
//! [ADR 0010](../../../docs/adrs/0010-page-actor-ownership.md): owns
//! [`ProfileStore`], the shared [`NetworkSession`], and the page registry.

use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::actor::{PageActor, PageHandle, PageId};
use crate::network::{NetworkSession, ProfileStore};
use crate::profile::{Profile, ProfileName};

/// Process-owned engine for one profile.
pub struct Browser {
    inner: Arc<Mutex<BrowserInner>>,
}

struct BrowserInner {
    live: bool,
    network: NetworkSession,
    pages: HashMap<PageId, PageActor>,
    next_page: u64,
}

/// Value-only handle protocols use to drive [`Browser`].
#[derive(Clone)]
pub struct BrowserHandle {
    inner: Arc<Mutex<BrowserInner>>,
}

impl Browser {
    /// Opens a browser on `profile` with cookies under the process XDG data home.
    #[must_use]
    pub fn open(profile: &Profile) -> Self {
        Self::with_store(ProfileStore::open(profile), net::AgentBuilder::new())
    }

    /// Opens a browser on `profile` with cookies under `data_home`.
    #[must_use]
    pub fn open_in(data_home: &Path, profile: &Profile) -> Self {
        Self::with_store(
            ProfileStore::open_in(data_home, profile),
            net::AgentBuilder::new(),
        )
    }

    /// Opens a browser that shares `network` (and its cookie jar).
    #[must_use]
    pub fn open_with_network(network: NetworkSession) -> Self {
        Self {
            inner: Arc::new(Mutex::new(BrowserInner {
                live: true,
                network,
                pages: HashMap::new(),
                next_page: 1,
            })),
        }
    }

    fn with_store(store: ProfileStore, builder: net::AgentBuilder) -> Self {
        Self::open_with_network(NetworkSession::from_builder(builder, store))
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
        self.handle().close();
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
        let actor = PageActor::spawn(id, fetch);
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

    /// Stops every page and refuses later commands. Idempotent.
    pub fn close(&self) {
        {
            let mut inner = self.lock();
            if !inner.live {
                return;
            }
            inner.live = false;
        }
        self.persist();
        self.close_all();
    }

    fn persist(&self) {
        self.lock().network.persist();
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
