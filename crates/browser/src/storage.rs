//! The browser-owned local storage area.
//!
//! `localStorage` is one area per origin, shared by every document in the
//! profile and persisted with it
//! (<https://html.spec.whatwg.org/multipage/webstorage.html#the-localstorage-attribute>).
//! The browser process owns the map so independent renderers observe one area;
//! renderers call in over the service path. `sessionStorage` is scoped to one
//! top-level browsing context and remains browser-owned across renderer swaps.
//!
//! The mutation methods implement the spec's `Storage` algorithms and return
//! the change a `storage` event needs; `None` means the mutation was a no-op.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

use renderer::{StorageChange, StorageError};
use webstorage::StorageArea;

use crate::actor::TabId;

/// One profile's local storage areas, keyed by origin.
#[derive(Default)]
pub(crate) struct LocalStorage {
    areas: Mutex<BTreeMap<String, StorageArea>>,
    dirty: AtomicBool,
}

/// Ephemeral storage namespaces keyed by top-level browsing context.
#[derive(Default)]
pub(crate) struct SessionStorage {
    namespaces: Mutex<HashMap<TabId, BTreeMap<String, StorageArea>>>,
}

/// Address of one `sessionStorage` entry in a top-level browsing context.
#[derive(Clone, Copy)]
pub(crate) struct SessionKeyOptions<'a> {
    /// Top-level browsing context that owns the namespace.
    pub(crate) tab: TabId,
    /// Origin that owns the area.
    pub(crate) origin: &'a str,
    /// Entry key.
    pub(crate) key: &'a str,
}

/// One `sessionStorage` write.
#[derive(Clone, Copy)]
pub(crate) struct SessionSetOptions<'a> {
    /// Top-level browsing context that owns the namespace.
    pub(crate) tab: TabId,
    /// Origin that owns the area.
    pub(crate) origin: &'a str,
    /// Entry key.
    pub(crate) key: &'a str,
    /// Entry value.
    pub(crate) value: &'a str,
}

/// One `localStorage` write.
#[derive(Clone, Copy)]
pub(crate) struct LocalSetOptions<'a> {
    /// Origin that owns the area.
    pub(crate) origin: &'a str,
    /// Entry key.
    pub(crate) key: &'a str,
    /// Entry value.
    pub(crate) value: &'a str,
}

impl SessionStorage {
    pub(crate) fn create(&self, tab: TabId) {
        self.namespaces
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(tab)
            .or_default();
    }

    /// Copies a top-level browsing context's session-storage namespace
    /// (<https://html.spec.whatwg.org/multipage/document-sequences.html#copy-session-storage>).
    pub(crate) fn copy(&self, source: TabId, target: TabId) {
        let mut namespaces = self
            .namespaces
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let namespace = namespaces.get(&source).cloned().unwrap_or_default();
        namespaces.insert(target, namespace);
    }

    pub(crate) fn remove_namespace(&self, tab: TabId) {
        self.namespaces
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&tab);
    }

    pub(crate) fn get(&self, address: SessionKeyOptions<'_>) -> Option<String> {
        let SessionKeyOptions { tab, origin, key } = address;
        self.namespaces
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&tab)
            .and_then(|namespace| namespace.get(origin))
            .and_then(|area| area.get(key))
    }

    pub(crate) fn keys(&self, tab: TabId, origin: &str) -> Vec<String> {
        self.namespaces
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&tab)
            .and_then(|namespace| namespace.get(origin))
            .map(StorageArea::keys)
            .unwrap_or_default()
    }

    pub(crate) fn set(
        &self,
        write: SessionSetOptions<'_>,
    ) -> Result<Option<StorageChange>, StorageError> {
        let SessionSetOptions {
            tab,
            origin,
            key,
            value,
        } = write;
        let mut namespaces = self
            .namespaces
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some(namespace) = namespaces.get_mut(&tab) else {
            return Ok(None);
        };
        namespace
            .entry(origin.to_owned())
            .or_default()
            .set(key, value)
    }

    pub(crate) fn remove(&self, address: SessionKeyOptions<'_>) -> Option<StorageChange> {
        let SessionKeyOptions { tab, origin, key } = address;
        self.namespaces
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_mut(&tab)?
            .get_mut(origin)?
            .remove(key)
    }

    pub(crate) fn clear(&self, tab: TabId, origin: &str) -> Option<StorageChange> {
        self.namespaces
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_mut(&tab)?
            .get_mut(origin)?
            .clear()
    }
}

impl LocalStorage {
    /// `Storage.getItem(key)`
    /// (<https://html.spec.whatwg.org/multipage/webstorage.html#dom-storage-getitem>).
    pub(crate) fn get(&self, origin: &str, key: &str) -> Option<String> {
        self.areas
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(origin)
            .and_then(|area| area.get(key))
    }

    /// `Storage.key(index)` support: every key, in this area's iteration order.
    pub(crate) fn keys(&self, origin: &str) -> Vec<String> {
        self.areas
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(origin)
            .map(StorageArea::keys)
            .unwrap_or_default()
    }

    /// `Storage.setItem(key, value)`
    /// (<https://html.spec.whatwg.org/multipage/webstorage.html#dom-storage-setitem>).
    pub(crate) fn set(
        &self,
        write: LocalSetOptions<'_>,
    ) -> Result<Option<StorageChange>, StorageError> {
        let LocalSetOptions { origin, key, value } = write;
        let mut areas = self.areas.lock().unwrap_or_else(PoisonError::into_inner);
        let area = areas.entry(origin.to_owned()).or_default();
        let change = area.set(key, value)?;
        let Some(change) = change else {
            return Ok(None);
        };
        drop(areas);
        self.dirty.store(true, Ordering::SeqCst);
        Ok(Some(change))
    }

    /// `Storage.removeItem(key)`
    /// (<https://html.spec.whatwg.org/multipage/webstorage.html#dom-storage-removeitem>).
    pub(crate) fn remove(&self, origin: &str, key: &str) -> Option<StorageChange> {
        let mut areas = self.areas.lock().unwrap_or_else(PoisonError::into_inner);
        let change = areas.get_mut(origin)?.remove(key)?;
        drop(areas);
        self.dirty.store(true, Ordering::SeqCst);
        Some(change)
    }

    /// `Storage.clear()`
    /// (<https://html.spec.whatwg.org/multipage/webstorage.html#dom-storage-clear>).
    pub(crate) fn clear(&self, origin: &str) -> Option<StorageChange> {
        let mut areas = self.areas.lock().unwrap_or_else(PoisonError::into_inner);
        let area = areas.get_mut(origin)?;
        let change = area.clear()?;
        drop(areas);
        self.dirty.store(true, Ordering::SeqCst);
        Some(change)
    }

    /// Whether unsaved mutations happened since the last [`Self::mark_clean`].
    pub(crate) fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::SeqCst)
    }

    pub(crate) fn mark_clean(&self) {
        self.dirty.store(false, Ordering::SeqCst);
    }

    /// Every `(origin, key, value)` row, for the profile store.
    pub(crate) fn export(&self) -> Vec<(String, String, String)> {
        let areas = self.areas.lock().unwrap_or_else(PoisonError::into_inner);
        let mut rows = Vec::new();
        for (origin, area) in areas.iter() {
            for (key, value) in area.entries() {
                rows.push((origin.clone(), key.to_owned(), value.to_owned()));
            }
        }
        rows
    }

    /// Replaces the areas with the profile store's rows.
    pub(crate) fn import(&self, rows: impl IntoIterator<Item = (String, String, String)>) {
        let mut areas = self.areas.lock().unwrap_or_else(PoisonError::into_inner);
        areas.clear();
        for (origin, key, value) in rows {
            let area = areas.entry(origin).or_default();
            let _result = area.set(&key, &value);
        }
    }
}
