//! Connection transitions for renderer-owned frames and images.

use super::{Dom, Lifecycle, NodeId, NodeKind, html_namespace};

#[derive(Debug, Default)]
pub(super) struct ConnectionState {
    lifecycle: Vec<Lifecycle>,
    connected_iframes: u32,
}

impl Dom {
    /// Drains the recorded connection transitions in order.
    pub fn take_lifecycle(&mut self) -> Vec<Lifecycle> {
        std::mem::take(&mut self.connections.lifecycle)
    }

    /// Records every `iframe` and `img` connection transition in a snapshot
    /// taken before an operation. Snapshots only ever carry those elements
    /// (see [`Dom::connection_snapshot`]): filtering in the snapshot keeps
    /// the parser's hot path free of per-element bookkeeping.
    ///
    /// The snapshot carries each element's kind so a destroyed node still
    /// moves the `iframe` count: after `destroy` frees it, `is_iframe_element`
    /// can no longer answer.
    pub(super) fn record_snapshot(&mut self, snapshot: Vec<(NodeId, bool, bool)>) {
        for (id, was_connected, is_iframe) in snapshot {
            let connected = self.is_connected(id);
            if connected != was_connected {
                if is_iframe {
                    if connected {
                        self.connections.connected_iframes =
                            self.connections.connected_iframes.saturating_add(1);
                    } else {
                        self.connections.connected_iframes =
                            self.connections.connected_iframes.saturating_sub(1);
                    }
                }
                self.connections.lifecycle.push(if connected {
                    Lifecycle::Inserted(id)
                } else {
                    Lifecycle::Removed(id)
                });
            }
        }
    }

    /// How many `iframe` elements are connected in this document.
    #[must_use]
    pub fn connected_iframe_count(&self) -> u32 {
        self.connections.connected_iframes
    }

    /// Whether `id` is an HTML `iframe` element.
    #[must_use]
    pub fn is_iframe_element(&self, id: NodeId) -> bool {
        matches!(
            self.kind(id),
            Some(NodeKind::Element { name, .. })
                if name.ns == html_namespace() && name.local.as_ref() == "iframe"
        )
    }

    /// Whether `id` is an HTML `img` element.
    #[must_use]
    pub fn is_img_element(&self, id: NodeId) -> bool {
        matches!(
            self.kind(id),
            Some(NodeKind::Element { name, .. })
                if name.ns == html_namespace() && name.local.as_ref() == "img"
        )
    }

    /// The iframe and img elements in `id`'s inclusive subtree with, for each,
    /// its connectivity and whether it is an iframe (the only kind that moves
    /// `connected_iframes`). Captured before an operation and handed to
    /// [`Dom::record_snapshot`] after it, so the transition is measured across
    /// the whole operation rather than at an internal step.
    pub(super) fn connection_snapshot(&self, id: NodeId) -> Vec<(NodeId, bool, bool)> {
        let mut snapshot = Vec::new();
        let mut stack = vec![id];
        while let Some(current) = stack.pop() {
            let is_iframe = self.is_iframe_element(current);
            if is_iframe || self.is_img_element(current) {
                snapshot.push((current, self.is_connected(current), is_iframe));
            }
            if let Some(children) = self.children(current) {
                stack.extend(children);
            }
            if let Some(root) = self.shadow_root(current) {
                stack.push(root);
            }
        }
        snapshot
    }

    /// Whether `id`'s ancestor chain reaches the document root.
    #[must_use]
    pub fn is_connected(&self, id: NodeId) -> bool {
        let mut current = Some(id);
        while let Some(node) = current {
            if node == self.document {
                return true;
            }
            current = self.parent(node).or_else(|| self.shadow_host(node));
        }
        false
    }
}
