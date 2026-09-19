//! The browser-owned local storage area.
//!
//! `localStorage` is one area per origin, shared by every document in the
//! profile and persisted with it
//! (<https://html.spec.whatwg.org/multipage/webstorage.html#the-localstorage-attribute>).
//! The browser process owns the map so independent renderers observe one area;
//! renderers call in over the service path. `sessionStorage` is scoped to one
//! top-level browsing context and stays in the renderer.
//!
//! The mutation methods implement the spec's `Storage` algorithms and return
//! the change a `storage` event needs; `None` means the mutation was a no-op.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

use renderer::{FrameId, STORAGE_QUOTA_BYTES, StorageChange, StorageError, StorageKind};

use crate::wire::RendererAssignmentId;

/// The renderer assignment and frame whose script made a change; the
/// assignment's own engine excludes the frame from the broadcast.
pub(crate) type StorageSource = (RendererAssignmentId, FrameId);

/// One renderer's delivery hook for `localStorage` changes. Returning `false`
/// removes the listener (the renderer is gone).
pub(crate) type StorageSink = Box<dyn Fn(StorageBroadcast) -> bool + Send + Sync>;

/// One `storage` event a renderer should fire, minus the receiver's own
/// `storageArea`.
#[derive(Clone, Debug)]
pub(crate) struct StorageBroadcast {
    pub(crate) origin: String,
    pub(crate) kind: StorageKind,
    pub(crate) key: Option<String>,
    pub(crate) old_value: Option<String>,
    pub(crate) new_value: Option<String>,
    pub(crate) url: String,
    pub(crate) source: StorageSource,
}

struct StorageListener {
    sink: StorageSink,
}

/// One profile's local storage areas, keyed by origin.
#[derive(Default)]
pub(crate) struct LocalStorage {
    areas: Mutex<BTreeMap<String, BTreeMap<String, String>>>,
    listeners: Mutex<Vec<StorageListener>>,
    dirty: AtomicBool,
}

impl LocalStorage {
    /// Registers `sink` as a renderer's `storage`-event delivery hook.
    pub(crate) fn subscribe(&self, sink: StorageSink) {
        self.listeners
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(StorageListener { sink });
    }

    /// `Storage.getItem(key)`
    /// (<https://html.spec.whatwg.org/multipage/webstorage.html#dom-storage-getitem>).
    pub(crate) fn get(&self, origin: &str, key: &str) -> Option<String> {
        self.areas
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(origin)
            .and_then(|area| area.get(key))
            .cloned()
    }

    /// `Storage.key(index)` support: every key, in this area's iteration order.
    pub(crate) fn keys(&self, origin: &str) -> Vec<String> {
        self.areas
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(origin)
            .map(|area| area.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// `Storage.setItem(key, value)`
    /// (<https://html.spec.whatwg.org/multipage/webstorage.html#dom-storage-setitem>).
    /// `url` is the mutating document's URL for the event.
    pub(crate) fn set(
        &self,
        origin: &str,
        key: &str,
        value: &str,
        url: &str,
        source: StorageSource,
    ) -> Result<Option<StorageChange>, StorageError> {
        let mut areas = self.areas.lock().unwrap_or_else(PoisonError::into_inner);
        let area = areas.entry(origin.to_owned()).or_default();
        let old = area.get(key).cloned();
        if old.as_deref() == Some(value) {
            return Ok(None);
        }
        if !fits(area, key, old.as_deref(), value) {
            return Err(StorageError::QuotaExceeded);
        }
        // A new or updated entry returns to the end of the map: the spec sets
        // `reorder` for a new key and refuses to reorder only an unchanged
        // value, which returned above.
        area.remove(key);
        area.insert(key.to_owned(), value.to_owned());
        drop(areas);
        self.dirty.store(true, Ordering::SeqCst);
        let change = StorageChange {
            key: Some(key.to_owned()),
            old_value: old,
            new_value: Some(value.to_owned()),
        };
        self.broadcast(&StorageBroadcast {
            origin: origin.to_owned(),
            kind: StorageKind::Local,
            key: change.key.clone(),
            old_value: change.old_value.clone(),
            new_value: change.new_value.clone(),
            url: url.to_owned(),
            source,
        });
        Ok(Some(change))
    }

    /// `Storage.removeItem(key)`
    /// (<https://html.spec.whatwg.org/multipage/webstorage.html#dom-storage-removeitem>).
    pub(crate) fn remove(
        &self,
        origin: &str,
        key: &str,
        url: &str,
        source: StorageSource,
    ) -> Option<StorageChange> {
        let mut areas = self.areas.lock().unwrap_or_else(PoisonError::into_inner);
        let old = areas.get_mut(origin)?.remove(key)?;
        drop(areas);
        self.dirty.store(true, Ordering::SeqCst);
        let change = StorageChange {
            key: Some(key.to_owned()),
            old_value: Some(old),
            new_value: None,
        };
        self.broadcast(&StorageBroadcast {
            origin: origin.to_owned(),
            kind: StorageKind::Local,
            key: change.key.clone(),
            old_value: change.old_value.clone(),
            new_value: change.new_value.clone(),
            url: url.to_owned(),
            source,
        });
        Some(change)
    }

    /// `Storage.clear()`
    /// (<https://html.spec.whatwg.org/multipage/webstorage.html#dom-storage-clear>).
    pub(crate) fn clear(
        &self,
        origin: &str,
        url: &str,
        source: StorageSource,
    ) -> Option<StorageChange> {
        let mut areas = self.areas.lock().unwrap_or_else(PoisonError::into_inner);
        let area = areas.get_mut(origin)?;
        if area.is_empty() {
            return None;
        }
        area.clear();
        drop(areas);
        self.dirty.store(true, Ordering::SeqCst);
        let change = StorageChange {
            key: None,
            old_value: None,
            new_value: None,
        };
        self.broadcast(&StorageBroadcast {
            origin: origin.to_owned(),
            kind: StorageKind::Local,
            key: None,
            old_value: None,
            new_value: None,
            url: url.to_owned(),
            source,
        });
        Some(change)
    }

    /// Sends one change to every live renderer, pruning dead ones. The source
    /// renderer is not skipped: its session loop excludes the mutating window
    /// (<https://html.spec.whatwg.org/multipage/webstorage.html#concept-storage-broadcast>).
    fn broadcast(&self, event: &StorageBroadcast) {
        let mut listeners = self
            .listeners
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        listeners.retain(|listener| (listener.sink)(event.clone()));
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
            for (key, value) in area {
                rows.push((origin.clone(), key.clone(), value.clone()));
            }
        }
        rows
    }

    /// Replaces the areas with the profile store's rows.
    pub(crate) fn import(&self, rows: impl IntoIterator<Item = (String, String, String)>) {
        let mut areas = self.areas.lock().unwrap_or_else(PoisonError::into_inner);
        areas.clear();
        for (origin, key, value) in rows {
            areas.entry(origin).or_default().insert(key, value);
        }
    }
}

/// Whether storing `key = value` keeps `area` within [`STORAGE_QUOTA_BYTES`],
/// counting the replaced entry at its stored size.
fn fits(area: &BTreeMap<String, String>, key: &str, old: Option<&str>, value: &str) -> bool {
    let used: usize = area
        .iter()
        .map(|(stored_key, stored_value)| stored_key.len() + stored_value.len())
        .sum();
    let replaced = old.map_or(0, |old| key.len() + old.len());
    used - replaced + key.len() + value.len() <= STORAGE_QUOTA_BYTES
}

#[cfg(test)]
mod tests {
    use renderer::{FrameId, STORAGE_QUOTA_BYTES, StorageError};

    use super::LocalStorage;

    #[test]
    fn mutations_report_the_spec_change() {
        let storage = LocalStorage::default();
        let origin = "https://example.test";
        let url = "https://example.test/page";
        let source = (crate::wire::RendererAssignmentId::new(1), FrameId::MAIN);

        let change = storage
            .set(origin, "a", "1", url, source)
            .expect("within quota")
            .expect("new key changes");
        assert_eq!(change.old_value, None);
        assert_eq!(change.new_value.as_deref(), Some("1"));
        assert_eq!(
            storage.set(origin, "a", "1", url, source),
            Ok(None),
            "same value is a no-op"
        );
        assert_eq!(
            storage
                .set(origin, "a", "2", url, source)
                .expect("within quota")
                .expect("update")
                .old_value,
            Some("1".into())
        );
        assert_eq!(storage.remove(origin, "missing", url, source), None);
        assert_eq!(
            storage
                .remove(origin, "a", url, source)
                .expect("removal")
                .new_value,
            None
        );
        assert_eq!(
            storage.clear(origin, url, source),
            None,
            "the area is empty again"
        );
    }

    #[test]
    fn writes_beyond_the_quota_fail_without_storing() {
        let storage = LocalStorage::default();
        let origin = "https://example.test";
        let url = "https://example.test/";
        let source = (crate::wire::RendererAssignmentId::new(1), FrameId::MAIN);
        let value = "x".repeat(STORAGE_QUOTA_BYTES + 1);

        assert_eq!(
            storage.set(origin, "big", &value, url, source),
            Err(StorageError::QuotaExceeded)
        );
        assert_eq!(storage.get(origin, "big"), None);
    }

    #[test]
    fn clear_of_an_empty_area_is_a_no_op() {
        let storage = LocalStorage::default();
        let source = (crate::wire::RendererAssignmentId::new(1), FrameId::MAIN);
        assert_eq!(
            storage.clear("https://example.test", "https://example.test/", source),
            None
        );
    }

    #[test]
    fn export_and_import_round_trip() {
        let source_storage = LocalStorage::default();
        let origin = "https://example.test";
        let url = "https://example.test/";
        let source = (crate::wire::RendererAssignmentId::new(1), FrameId::MAIN);
        assert!(
            source_storage
                .set(
                    origin,
                    "key\twith\ttabs",
                    "value\nwith\nnewlines",
                    url,
                    source
                )
                .expect("within quota")
                .is_some()
        );
        assert!(
            source_storage
                .set(origin, "plain", "value", url, source)
                .expect("within quota")
                .is_some()
        );
        let rows = source_storage.export();

        let target = LocalStorage::default();
        target.import(rows);
        assert_eq!(
            target.get(origin, "key\twith\ttabs").as_deref(),
            Some("value\nwith\nnewlines")
        );
        assert_eq!(target.get(origin, "plain").as_deref(), Some("value"));
    }
}
