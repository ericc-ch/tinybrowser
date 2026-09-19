//! Session storage: one area per origin inside one top-level browsing context.
//!
//! `sessionStorage` never leaves the renderer process. Its scope is the tab's
//! traversable navigable, and this engine hosts one tab per engine
//! (<https://html.spec.whatwg.org/multipage/webstorage.html#the-sessionstorage-attribute>).
//! The mutation algorithms are the same ones the browser applies to
//! `localStorage`; a change that alters nothing returns `None`.

use std::collections::BTreeMap;

use crate::protocol::{FrameId, STORAGE_QUOTA_BYTES, StorageChange, StorageError, StorageKind};

/// One change waiting on the `storage`-event task of every receiving frame.
#[derive(Clone, Debug)]
pub struct PendingStorageEvent {
    /// The frame whose script made the change, when the change came from this
    /// same assignment; `None` for another renderer's or assignment's change.
    /// The source window itself never receives the event
    /// (<https://html.spec.whatwg.org/multipage/webstorage.html#concept-storage-broadcast>).
    pub source: Option<FrameId>,
    /// Serialized origin shared by the change and its receivers.
    pub origin: String,
    pub kind: StorageKind,
    pub key: Option<String>,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
    /// URL of the document whose script made the change.
    pub url: String,
}

impl PendingStorageEvent {
    /// A browser-broadcast change with no local source yet; the session loop
    /// sets `source` for the assignment that made it.
    #[must_use]
    pub fn broadcast(
        origin: String,
        kind: StorageKind,
        key: Option<String>,
        old_value: Option<String>,
        new_value: Option<String>,
        url: String,
    ) -> Self {
        Self {
            source: None,
            origin,
            kind,
            key,
            old_value,
            new_value,
            url,
        }
    }
}

/// One engine's session storage areas, keyed by origin.
#[derive(Default)]
pub(crate) struct SessionStorage {
    areas: BTreeMap<String, BTreeMap<String, String>>,
}

impl SessionStorage {
    pub(crate) fn get(&self, origin: &str, key: &str) -> Option<String> {
        self.areas
            .get(origin)
            .and_then(|area| area.get(key))
            .cloned()
    }

    pub(crate) fn keys(&self, origin: &str) -> Vec<String> {
        self.areas
            .get(origin)
            .map(|area| area.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// `Storage.setItem(key, value)`
    /// (<https://html.spec.whatwg.org/multipage/webstorage.html#dom-storage-setitem>).
    pub(crate) fn set(
        &mut self,
        origin: &str,
        key: &str,
        value: &str,
    ) -> Result<Option<StorageChange>, StorageError> {
        let area = self.areas.entry(origin.to_owned()).or_default();
        let old = area.get(key).cloned();
        if old.as_deref() == Some(value) {
            return Ok(None);
        }
        if !fits(area, key, old.as_deref(), value) {
            return Err(StorageError::QuotaExceeded);
        }
        area.remove(key);
        area.insert(key.to_owned(), value.to_owned());
        Ok(Some(StorageChange {
            key: Some(key.to_owned()),
            old_value: old,
            new_value: Some(value.to_owned()),
        }))
    }

    /// `Storage.removeItem(key)`
    /// (<https://html.spec.whatwg.org/multipage/webstorage.html#dom-storage-removeitem>).
    pub(crate) fn remove(&mut self, origin: &str, key: &str) -> Option<StorageChange> {
        let old = self.areas.get_mut(origin)?.remove(key)?;
        Some(StorageChange {
            key: Some(key.to_owned()),
            old_value: Some(old),
            new_value: None,
        })
    }

    /// `Storage.clear()`
    /// (<https://html.spec.whatwg.org/multipage/webstorage.html#dom-storage-clear>).
    pub(crate) fn clear(&mut self, origin: &str) -> Option<StorageChange> {
        let area = self.areas.get_mut(origin)?;
        if area.is_empty() {
            return None;
        }
        area.clear();
        Some(StorageChange {
            key: None,
            old_value: None,
            new_value: None,
        })
    }

    /// Replaces `origin`'s entries with a copy of another browsing context's
    /// area
    /// (<https://html.spec.whatwg.org/multipage/document-sequences.html#copy-session-storage>).
    pub(crate) fn import(&mut self, origin: &str, entries: Vec<(String, String)>) {
        let area = self.areas.entry(origin.to_owned()).or_default();
        area.clear();
        for (key, value) in entries {
            area.insert(key, value);
        }
    }
}

/// Whether storing `key = value` keeps `area` within [`STORAGE_QUOTA_BYTES`],
/// counting the replaced entry at its stored size. Session storage gets its
/// own quota, independent of the browser-owned local area.
fn fits(area: &BTreeMap<String, String>, key: &str, old: Option<&str>, value: &str) -> bool {
    let used: usize = area
        .iter()
        .map(|(stored_key, stored_value)| stored_key.len() + stored_value.len())
        .sum();
    let replaced = old.map_or(0, |old| key.len() + old.len());
    used - replaced + key.len() + value.len() <= STORAGE_QUOTA_BYTES
}
