//! Browser-context event fan-out to renderer processes.
//!
//! One `postMessage` reaches every `BroadcastChannel` object with the same
//! name and origin, in every renderer
//! (<https://html.spec.whatwg.org/multipage/web-messaging.html#broadcasting-to-other-browsing-contexts>).
//! The sender's renderer is not skipped: like storage events, its session
//! loop excludes only the posting channel of the matching assignment.

use std::sync::{Mutex, PoisonError};

use renderer::{FrameId, StorageKind};

use crate::wire::RendererAssignmentId;

/// One `BroadcastChannel.postMessage` broadcast.
#[derive(Clone, Debug)]
pub(crate) struct BroadcastMessage {
    pub(crate) origin: String,
    pub(crate) name: String,
    pub(crate) payload: String,
    /// Assignment and per-realm channel id that posted it.
    pub(crate) source: (RendererAssignmentId, u64),
}

/// One storage mutation delivered to eligible renderer frames.
#[derive(Clone, Debug)]
pub(crate) struct StorageBroadcast {
    pub(crate) origin: String,
    pub(crate) kind: StorageKind,
    pub(crate) key: Option<String>,
    pub(crate) old_value: Option<String>,
    pub(crate) new_value: Option<String>,
    pub(crate) url: String,
    pub(crate) source: (RendererAssignmentId, FrameId),
}

/// One browser-context event for renderer delivery.
#[derive(Clone, Debug)]
pub(crate) enum ContextEvent {
    Storage(StorageBroadcast),
    Broadcast(BroadcastMessage),
}

/// Delivery hook; returning `false` removes the listener.
pub(crate) type EventSink = Box<dyn Fn(&ContextEvent) -> bool + Send + Sync>;

/// Browser-owned fan-out to every live renderer.
#[derive(Default)]
pub(crate) struct RendererEventHub {
    listeners: Mutex<Vec<EventSink>>,
}

impl RendererEventHub {
    /// Registers `sink` as a renderer's broadcast delivery hook.
    pub(crate) fn subscribe(&self, sink: EventSink) {
        self.listeners
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(sink);
    }

    /// Sends one message to every live renderer, pruning dead ones.
    pub(crate) fn broadcast(&self, event: &ContextEvent) {
        let mut listeners = self
            .listeners
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        listeners.retain(|listener| listener(event));
    }
}
