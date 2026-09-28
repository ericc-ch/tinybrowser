//! Connection transitions for renderer-owned frames and images.

use crate::shadow;
use crate::{Document, NodeId, NodeKind, html_namespace};

/// A connection transition of one element, for HTML lifecycle steps.
///
/// [HTML's post-connection and removing steps](https://html.spec.whatwg.org/multipage/iframe-embed-object.html#the-iframe-element)
/// hang off exactly these transitions: an iframe creates its content
/// navigable when it becomes connected and destroys it when disconnected.
/// Recorded always, independent of `MutationObserver` recording.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lifecycle {
    /// An element became connected to a document.
    Inserted(NodeId),
    /// An element became disconnected from a document.
    Removed(NodeId),
}

#[derive(Debug, Default)]
pub(crate) struct ConnectionState {
    lifecycle: Vec<Lifecycle>,
    connected_iframes: u32,
}

/// Drains the recorded connection transitions in order.
pub fn take(document: &mut Document) -> Vec<Lifecycle> {
    std::mem::take(&mut document.connections.lifecycle)
}

/// Records every `iframe` and `img` connection transition in a snapshot
/// taken before an operation. Snapshots only ever carry those elements
/// (see [`snapshot`]): filtering in the snapshot keeps
/// the parser's hot path free of per-element bookkeeping.
///
/// The snapshot carries each element's kind so a destroyed node still
/// moves the `iframe` count: after `destroy` frees it, `is_iframe_element`
/// can no longer answer.
pub(crate) fn record_snapshot(document: &mut Document, snapshot: Vec<(NodeId, bool, bool)>) {
    for (id, was_connected, is_iframe) in snapshot {
        let connected = is_connected(document, id);
        if connected != was_connected {
            if is_iframe {
                if connected {
                    document.connections.connected_iframes =
                        document.connections.connected_iframes.saturating_add(1);
                } else {
                    document.connections.connected_iframes =
                        document.connections.connected_iframes.saturating_sub(1);
                }
            }
            document.connections.lifecycle.push(if connected {
                Lifecycle::Inserted(id)
            } else {
                Lifecycle::Removed(id)
            });
        }
    }
}

/// How many `iframe` elements are connected in this document.
#[must_use]
pub fn connected_iframe_count(document: &Document) -> u32 {
    document.connections.connected_iframes
}

/// Whether `id` is an HTML `iframe` element.
#[must_use]
pub fn is_iframe_element(document: &Document, id: NodeId) -> bool {
    matches!(
        document.kind(id),
        Some(NodeKind::Element { name, .. })
            if name.ns == html_namespace() && name.local.as_ref() == "iframe"
    )
}

/// Whether `id` is an HTML `img` element.
#[must_use]
pub fn is_img_element(document: &Document, id: NodeId) -> bool {
    matches!(
        document.kind(id),
        Some(NodeKind::Element { name, .. })
            if name.ns == html_namespace() && name.local.as_ref() == "img"
    )
}

/// The iframe and img elements in `id`'s inclusive subtree with, for each,
/// its connectivity and whether it is an iframe (the only kind that moves
/// `connected_iframes`). Captured before an operation and handed to
/// [`record_snapshot`] after it, so the transition is measured across
/// the whole operation rather than at an internal step.
pub(crate) fn snapshot(document: &Document, id: NodeId) -> Vec<(NodeId, bool, bool)> {
    let mut snapshot = Vec::new();
    let mut stack = vec![id];
    while let Some(current) = stack.pop() {
        let is_iframe = is_iframe_element(document, current);
        if is_iframe || is_img_element(document, current) {
            snapshot.push((current, is_connected(document, current), is_iframe));
        }
        if let Some(children) = document.children(current) {
            stack.extend(children);
        }
        if let Some(root) = shadow::shadow_root(document, current) {
            stack.push(root);
        }
    }
    snapshot
}

/// Whether `id`'s ancestor chain reaches the document root.
#[must_use]
pub fn is_connected(document: &Document, id: NodeId) -> bool {
    let mut current = Some(id);
    while let Some(node) = current {
        if node == document.document() {
            return true;
        }
        current = document
            .parent(node)
            .or_else(|| shadow::shadow_host(document, node));
    }
    false
}
