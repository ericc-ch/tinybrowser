//! Live browser context and its default storage partition.

use std::io;
use std::path::Path;
use std::sync::Arc;

use url::Url;

use crate::actor::TabId;
use crate::broadcast::RendererEventHub;
use crate::network::{NetworkContext, TabNetworkHandle};
use crate::profile::Profile;
use crate::profile::store::ProfileStore;
use crate::storage::{LocalStorage, SessionStorage};

/// Services shared by renderer assignments in one storage partition.
#[derive(Clone)]
pub(crate) struct PartitionServices {
    pub(crate) local_storage: Arc<LocalStorage>,
    pub(crate) events: Arc<RendererEventHub>,
}

/// Network and site data shared by documents in one partition.
struct StoragePartition {
    network: NetworkContext,
    local_storage: Arc<LocalStorage>,
    events: Arc<RendererEventHub>,
}

/// One live profile and its default storage partition.
pub(crate) struct BrowserContext {
    store: Arc<ProfileStore>,
    partition: StoragePartition,
    sessions: Arc<SessionStorage>,
}

/// Inputs for opening one [`BrowserContext`].
pub(crate) struct BrowserContextOptions<'a> {
    /// Root below which profiles are stored.
    pub(crate) data_home: &'a Path,
    /// Durable profile to open exclusively.
    pub(crate) profile: &'a Profile,
    /// Live partition network.
    pub(crate) network: NetworkContext,
}

impl BrowserContext {
    /// Creates one context and restores its durable partition state.
    ///
    /// # Errors
    ///
    /// The profile directory cannot be created or exclusively locked, or stored
    /// profile data could not be read or quarantined.
    pub(crate) fn new(config: BrowserContextOptions<'_>) -> io::Result<Self> {
        let BrowserContextOptions {
            data_home,
            profile,
            network,
        } = config;
        let store = ProfileStore::new(data_home, profile)?;
        store.load_into(&network.agent())?;
        let local_storage = Arc::new(LocalStorage::default());
        store.load_local_storage(&local_storage)?;
        Ok(Self {
            store: Arc::new(store),
            partition: StoragePartition {
                network,
                local_storage,
                events: Arc::new(RendererEventHub::default()),
            },
            sessions: Arc::new(SessionStorage::default()),
        })
    }

    #[must_use]
    pub(crate) fn tab_network(&self) -> TabNetworkHandle {
        self.partition.network.tab_handle()
    }

    #[must_use]
    pub(crate) fn partition_services(&self) -> PartitionServices {
        PartitionServices {
            local_storage: Arc::clone(&self.partition.local_storage),
            events: Arc::clone(&self.partition.events),
        }
    }

    #[must_use]
    pub(crate) fn session_storage(&self) -> Arc<SessionStorage> {
        Arc::clone(&self.sessions)
    }

    pub(crate) fn create_tab(&self, tab: TabId) {
        self.sessions.create(tab);
    }

    pub(crate) fn copy_session(&self, source: TabId, target: TabId) {
        self.sessions.copy(source, target);
    }

    pub(crate) fn close_tab(&self, tab: TabId) {
        self.sessions.remove_namespace(tab);
    }

    pub(crate) async fn persist(&self) -> io::Result<()> {
        let store = Arc::clone(&self.store);
        let agent = self.partition.network.agent();
        let local_storage = Arc::clone(&self.partition.local_storage);
        tokio::task::spawn_blocking(move || {
            store.save_from(&agent)?;
            store.save_local_storage(&local_storage)
        })
        .await
        .map_err(io::Error::other)?
    }

    #[must_use]
    pub(crate) fn cookie_records(&self, url: &Url) -> Vec<net::CookieRecord> {
        self.partition.network.cookie_records(url)
    }

    pub(crate) fn clear_cookies(&self) {
        self.partition.network.clear_cookies();
    }

    pub(crate) fn add_cookie(&self, cookie: &str, url: &Url) -> bool {
        self.partition.network.add_cookie(cookie, url)
    }
}
