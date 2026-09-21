//! Shared Web Storage area algorithms.

use serde::{Deserialize, Serialize};

/// Upper bound on one origin's stored bytes per storage area. The specification
/// leaves the number to the user agent; tinybrowser uses 5 MiB.
pub const STORAGE_QUOTA_BYTES: usize = 5 * 1024 * 1024;

/// What one storage mutation changed, ready for a `storage` event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageChange {
    /// The key that changed; `None` for `clear()`.
    pub key: Option<String>,
    /// The value before the change; `None` when the key did not exist.
    pub old_value: Option<String>,
    /// The value after the change; `None` when the key was removed.
    pub new_value: Option<String>,
}

/// Why a storage mutation failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StorageError {
    /// The write would exceed the area's quota.
    QuotaExceeded,
}

/// One origin's ordered storage area.
#[derive(Clone, Debug, Default)]
pub struct StorageArea {
    entries: Vec<(String, String)>,
}

impl StorageArea {
    /// Builds an area from entries in iteration order. A later duplicate value
    /// replaces the earlier value without changing the key's position.
    #[must_use]
    pub fn from_entries(entries: impl IntoIterator<Item = (String, String)>) -> Self {
        let mut area = Self::default();
        for (key, value) in entries {
            if let Some((_, stored)) = area.entries.iter_mut().find(|(stored, _)| stored == &key) {
                *stored = value;
            } else {
                area.entries.push((key, value));
            }
        }
        area
    }

    /// Returns the value stored under `key`
    /// (<https://html.spec.whatwg.org/multipage/webstorage.html#dom-storage-getitem>).
    #[must_use]
    pub fn get(&self, key: &str) -> Option<String> {
        self.entries
            .iter()
            .find(|(stored, _)| stored == key)
            .map(|(_, value)| value.clone())
    }

    /// Returns keys in the area's stable iteration order
    /// (<https://html.spec.whatwg.org/multipage/webstorage.html#dom-storage-key>).
    #[must_use]
    pub fn keys(&self) -> Vec<String> {
        self.entries.iter().map(|(key, _)| key.clone()).collect()
    }

    /// Returns entries in the area's stable iteration order.
    pub fn entries(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
    }

    /// Stores `value` under `key`, preserving an existing key's position
    /// (<https://html.spec.whatwg.org/multipage/webstorage.html#dom-storage-setitem>).
    ///
    /// # Errors
    ///
    /// [`StorageError::QuotaExceeded`] when the resulting area would exceed
    /// [`STORAGE_QUOTA_BYTES`].
    pub fn set(&mut self, key: &str, value: &str) -> Result<Option<StorageChange>, StorageError> {
        let existing = self.entries.iter().position(|(stored, _)| stored == key);
        let old = existing.map(|index| self.entries[index].1.clone());
        if old.as_deref() == Some(value) {
            return Ok(None);
        }
        let used: usize = self
            .entries
            .iter()
            .map(|(stored_key, stored_value)| stored_key.len() + stored_value.len())
            .sum();
        let replaced = old.as_ref().map_or(0, |old| key.len() + old.len());
        if used - replaced + key.len() + value.len() > STORAGE_QUOTA_BYTES {
            return Err(StorageError::QuotaExceeded);
        }
        if let Some(index) = existing {
            value.clone_into(&mut self.entries[index].1);
        } else {
            self.entries.push((key.to_owned(), value.to_owned()));
        }
        Ok(Some(StorageChange {
            key: Some(key.to_owned()),
            old_value: old,
            new_value: Some(value.to_owned()),
        }))
    }

    /// Removes `key`; `None` means it was absent
    /// (<https://html.spec.whatwg.org/multipage/webstorage.html#dom-storage-removeitem>).
    pub fn remove(&mut self, key: &str) -> Option<StorageChange> {
        let index = self.entries.iter().position(|(stored, _)| stored == key)?;
        let (_, old) = self.entries.remove(index);
        Some(StorageChange {
            key: Some(key.to_owned()),
            old_value: Some(old),
            new_value: None,
        })
    }

    /// Clears this area; `None` means it was already empty
    /// (<https://html.spec.whatwg.org/multipage/webstorage.html#dom-storage-clear>).
    pub fn clear(&mut self) -> Option<StorageChange> {
        if self.entries.is_empty() {
            return None;
        }
        self.entries.clear();
        Some(StorageChange {
            key: None,
            old_value: None,
            new_value: None,
        })
    }

    /// Whether this area has no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
