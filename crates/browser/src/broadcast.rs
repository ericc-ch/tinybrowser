//! Same-origin `BroadcastChannel` fan-out.
//!
//! One `postMessage` reaches every `BroadcastChannel` object with the same
//! name and origin, in every renderer
//! (<https://html.spec.whatwg.org/multipage/web-messaging.html#broadcasting-to-other-browsing-contexts>).
//! The sender's renderer is not skipped: like storage events, its session
//! loop excludes only the posting channel of the matching assignment.

use std::sync::{Mutex, PoisonError};

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

/// Delivery hook; returning `false` removes the listener.
pub(crate) type BroadcastSink = Box<dyn Fn(&BroadcastMessage) -> bool + Send + Sync>;

/// Browser-owned fan-out to every live renderer.
#[derive(Default)]
pub(crate) struct BroadcastBus {
    listeners: Mutex<Vec<BroadcastSink>>,
}

impl BroadcastBus {
    /// Registers `sink` as a renderer's broadcast delivery hook.
    pub(crate) fn subscribe(&self, sink: BroadcastSink) {
        self.listeners
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(sink);
    }

    /// Sends one message to every live renderer, pruning dead ones.
    pub(crate) fn broadcast(&self, message: &BroadcastMessage) {
        let mut listeners = self
            .listeners
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        listeners.retain(|listener| listener(message));
    }
}
