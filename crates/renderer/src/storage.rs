//! Storage changes waiting for delivery in a renderer.

use crate::protocol::{FrameId, StorageKind};

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
