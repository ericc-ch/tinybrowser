//! Renderer-process state shared by every frame realm: the browsing context
//! tree, `MessagePort` endpoints, and the delivery queue between realms.
//!
//! One renderer thread hosts every frame of a tab and each frame is its own
//! `QuickJS` realm, so no JS value can cross. This module owns what must:
//! which frame holds which port, where a serialized message goes, and the
//! order it arrives in. Serialization itself happens in the sender realm (a
//! versioned payload string); the target realm decodes it when the task runs
//! ([window post message steps](https://html.spec.whatwg.org/multipage/web-messaging.html#window-post-message-steps)).

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use dom::NodeId;

use crate::protocol::FrameId;

/// Handle every realm holds to the renderer-process shared state.
pub(crate) type SharedHandle = Rc<RefCell<Shared>>;

/// Most child frames one renderer process will host; the main frame is
/// not counted.
pub(crate) const MAX_FRAMES: usize = 64;

/// Prefix every payload carries; a payload from another build (or a future
/// worker boundary) is refused instead of misdecoded.
pub(crate) const PAYLOAD_VERSION: &str = "tb1:";

/// The browsing context tree of one tab.
///
/// `children` order is the tree order of the frames' `iframe` containers,
/// which is what `window.length` and the indexed getter expose
/// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-length>).
#[derive(Default)]
pub(crate) struct FrameTree {
    /// Each frame's parent browsing context.
    parents: HashMap<FrameId, FrameId>,
    /// Each frame's direct children, in container tree order.
    children: HashMap<FrameId, Vec<FrameId>>,
    /// Each child frame's owning `iframe`.
    containers: HashMap<FrameId, NodeId>,
    /// Reverse of `containers`, for `contentWindow`.
    owners: HashMap<NodeId, FrameId>,
}

impl FrameTree {
    /// How many frames the tree holds, so spawns can respect the cap without
    /// walking it.
    pub(crate) fn len(&self) -> usize {
        self.containers.len()
    }

    /// Adds `child` under `parent`, owned by `container`.
    pub(crate) fn add(&mut self, parent: FrameId, child: FrameId, container: NodeId) {
        self.parents.insert(child, parent);
        self.containers.insert(child, container);
        self.owners.insert(container, child);
        self.children.entry(parent).or_default().push(child);
    }

    /// Removes one frame and returns its `(parent, container)`.
    pub(crate) fn remove(&mut self, child: FrameId) -> Option<(FrameId, NodeId)> {
        let parent = self.parents.remove(&child)?;
        if let Some(siblings) = self.children.get_mut(&parent) {
            siblings.retain(|frame| *frame != child);
        }
        self.children.remove(&child);
        let container = self.containers.remove(&child)?;
        self.owners.remove(&container);
        Some((parent, container))
    }

    /// Whether the tree holds `frame`.
    pub(crate) fn contains(&self, frame: FrameId) -> bool {
        self.containers.contains_key(&frame)
    }
    /// The parent browsing context of `frame`, when it is a child frame.
    pub(crate) fn parent(&self, frame: FrameId) -> Option<FrameId> {
        self.parents.get(&frame).copied()
    }

    /// The direct children of `frame`, in container tree order.
    pub(crate) fn children(&self, frame: FrameId) -> &[FrameId] {
        self.children.get(&frame).map_or(&[], Vec::as_slice)
    }

    /// The `iframe` that owns `frame`.
    pub(crate) fn container(&self, frame: FrameId) -> Option<NodeId> {
        self.containers.get(&frame).copied()
    }

    /// The child frame of `container`, when its browsing context exists.
    pub(crate) fn frame_for_container(&self, container: NodeId) -> Option<FrameId> {
        self.owners.get(&container).copied()
    }

    /// Reorders `parent`'s children to match the tree order of their
    /// containers; containers the caller no longer declares keep their place
    /// at the end.
    pub(crate) fn reorder(&mut self, parent: FrameId, containers: &[NodeId]) {
        let Some(children) = self.children.get_mut(&parent) else {
            return;
        };
        let mut ordered: Vec<FrameId> = Vec::with_capacity(children.len());
        for container in containers {
            if let Some(frame) = self.owners.get(container)
                && children.contains(frame)
                && !ordered.contains(frame)
            {
                ordered.push(*frame);
            }
        }
        for child in children.iter() {
            if !ordered.contains(child) {
                ordered.push(*child);
            }
        }
        *children = ordered;
    }
}

/// One endpoint of a channel. Two endpoints entangled to each other form the
/// channel; messages queue on the receiving endpoint and run as tasks on the
/// frame that owns it.
///
/// The queue and the entanglement live here rather than in JS so a port can be
/// transferred to another realm (or, later, another thread) and still be the
/// same port ([transferable objects](https://html.spec.whatwg.org/multipage/structured-data.html#transferable-objects)).
#[derive(Default)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "each bool mirrors a MessagePort state flag"
)]
pub(crate) struct PortEndpoint {
    /// The other endpoint, while entangled.
    peer: Option<u64>,
    /// The frame whose realm holds the port object, while it is materialized.
    owner: Option<FrameId>,
    /// Whether the port message queue is enabled (`start()` / `onmessage`).
    enabled: bool,
    /// `close()` ran, or the port's document was destroyed.
    closed: bool,
    /// Set while the endpoint is inside a transfer that has not been
    /// received yet; the object is unusable on the sending side.
    detached: bool,
    /// The frame that may receive this endpoint, recorded when it is
    /// detached for a transfer and re-aimed at delivery. Only that frame's
    /// realm may materialize the port, so a hostile page cannot adopt an
    /// endpoint by guessing its id
    /// (<https://html.spec.whatwg.org/multipage/web-messaging.html#transfer-receiving-steps>).
    recipient: Option<FrameId>,
    /// The peer disentangled while this endpoint was detached: the received
    /// port fires `close` as soon as it is materialized.
    close_pending: bool,
    /// Payloads waiting for the queue to enable.
    queue: VecDeque<QueuedPortMessage>,
}

/// One message waiting in a port's queue, with the endpoints it transfers.
#[derive(Default)]
struct QueuedPortMessage {
    payload: String,
    ports: Vec<u64>,
}

/// Every channel endpoint in the renderer process.
#[derive(Default)]
pub(crate) struct PortTable {
    next: u64,
    ports: HashMap<u64, PortEndpoint>,
}

impl PortTable {
    fn alloc(&mut self) -> u64 {
        let id = self.next;
        self.next = self.next.saturating_add(1);
        self.ports.insert(id, PortEndpoint::default());
        id
    }

    /// A fresh entangled pair owned by `owner`.
    pub(crate) fn new_pair(&mut self, owner: FrameId) -> (u64, u64) {
        let port1 = self.alloc();
        let port2 = self.alloc();
        let first = self.ports.get_mut(&port1).expect("fresh endpoint");
        first.peer = Some(port2);
        first.owner = Some(owner);
        first.recipient = Some(owner);
        let second = self.ports.get_mut(&port2).expect("fresh endpoint");
        second.peer = Some(port1);
        second.owner = Some(owner);
        second.recipient = Some(owner);
        (port1, port2)
    }

    /// Aims a detached endpoint at the frame whose realm will receive it. The
    /// engine calls this when a message carrying the endpoint is delivered
    /// (<https://html.spec.whatwg.org/multipage/web-messaging.html#transfer-receiving-steps>).
    pub(crate) fn route(&mut self, endpoint: u64, recipient: FrameId) {
        if let Some(port) = self.ports.get_mut(&endpoint)
            && port.detached
        {
            port.recipient = Some(recipient);
        }
    }

    /// Sends one payload through `from`; the receiving endpoint's queue either
    /// produces delivery tasks now or holds them until it is enabled.
    pub(crate) fn post(
        &mut self,
        from: u64,
        payload: String,
        ports: Vec<u64>,
        deliveries: &mut Vec<Delivery>,
    ) {
        let Some(port) = self.ports.get(&from) else {
            return;
        };
        if port.closed || port.detached {
            return;
        }
        let Some(peer_id) = port.peer else {
            return;
        };
        let Some(peer) = self.ports.get_mut(&peer_id) else {
            return;
        };
        if peer.closed {
            return;
        }
        peer.queue.push_back(QueuedPortMessage { payload, ports });
        if peer.enabled && !peer.detached {
            while let Some(message) = peer.queue.pop_front() {
                deliveries.push(Delivery::PortMessage {
                    endpoint: peer_id,
                    payload: message.payload,
                    ports: message.ports,
                });
            }
        }
    }

    /// Puts a delivery back on `endpoint`'s queue because its task ran in the
    /// wrong realm, or before the endpoint was materialized there.
    pub(crate) fn requeue(
        &mut self,
        endpoint: u64,
        payload: String,
        ports: Vec<u64>,
        deliveries: &mut Vec<Delivery>,
    ) {
        let Some(port) = self.ports.get_mut(&endpoint) else {
            return;
        };
        if port.closed {
            return;
        }
        // The requeued message was posted before whatever is already queued
        // while the endpoint was in transit, so it goes back at the front
        // (<https://html.spec.whatwg.org/multipage/web-messaging.html#port-message-queue>).
        port.queue.push_front(QueuedPortMessage { payload, ports });
        if port.enabled && !port.detached {
            while let Some(message) = port.queue.pop_front() {
                deliveries.push(Delivery::PortMessage {
                    endpoint,
                    payload: message.payload,
                    ports: message.ports,
                });
            }
        }
    }

    /// Re-aims a `close` event at an endpoint that moved realms.
    pub(crate) fn requeue_close(&mut self, endpoint: u64, deliveries: &mut Vec<Delivery>) {
        let Some(port) = self.ports.get_mut(&endpoint) else {
            return;
        };
        if port.closed {
            return;
        }
        if port.detached {
            // The received port fires `close` as soon as it materializes.
            port.close_pending = true;
        } else {
            deliveries.push(Delivery::PortClosed { endpoint });
        }
    }

    /// Enables `endpoint`'s queue and delivers whatever waited in it.
    pub(crate) fn start(&mut self, endpoint: u64, deliveries: &mut Vec<Delivery>) {
        let Some(port) = self.ports.get_mut(&endpoint) else {
            return;
        };
        if port.closed {
            return;
        }
        port.enabled = true;
        if port.detached {
            return;
        }
        while let Some(message) = port.queue.pop_front() {
            deliveries.push(Delivery::PortMessage {
                endpoint,
                payload: message.payload,
                ports: message.ports,
            });
        }
    }

    /// Disentangles `endpoint` and fires `close` at its peer
    /// (<https://html.spec.whatwg.org/multipage/web-messaging.html#disentangle>).
    pub(crate) fn close(&mut self, endpoint: u64, deliveries: &mut Vec<Delivery>) {
        let Some(port) = self.ports.get_mut(&endpoint) else {
            return;
        };
        if port.closed {
            return;
        }
        port.closed = true;
        port.enabled = false;
        port.queue.clear();
        if let Some(peer_id) = port.peer.take()
            && let Some(peer) = self.ports.get_mut(&peer_id)
        {
            peer.peer = None;
            if !peer.closed {
                if peer.detached {
                    // In transit: the received port fires `close` when
                    // materialized.
                    peer.close_pending = true;
                } else if peer.owner.is_some() {
                    deliveries.push(Delivery::PortClosed { endpoint: peer_id });
                }
            }
        }
        // The row is done: the JS handle keeps its own `closed` flag, and an
        // endpoint without a peer can never be addressed again.
        self.ports.remove(&endpoint);
    }

    /// Takes `endpoint` out of its realm for a transfer. The sender may only
    /// detach the port its own realm holds.
    ///
    /// Chromium (`MessagePort::DisentanglePorts`) and Firefox
    /// (`MessagePort` transfer steps) both move the port's queue with its
    /// identity and clear the enabled flag for the received port; the queue
    /// therefore lives on the endpoint, not on the realm.
    pub(crate) fn detach(&mut self, endpoint: u64, frame: FrameId) -> bool {
        let Some(port) = self.ports.get_mut(&endpoint) else {
            return false;
        };
        if port.closed || port.detached || port.owner != Some(frame) {
            return false;
        }
        port.owner = None;
        port.detached = true;
        // Until a message carries this endpoint, only the sender may
        // re-adopt it (in-realm `structuredClone`); a post re-aims it at the
        // message's target before the target realm can materialize it.
        port.recipient = Some(frame);
        // The transferred port's queue stays, but the received port starts
        // disabled again
        // (<https://html.spec.whatwg.org/multipage/web-messaging.html#transfer-receiving-steps>).
        port.enabled = false;
        true
    }

    /// Materializes a transferred `endpoint` in `frame`'s realm and reports
    /// whether the received port must fire `close` immediately. Only the
    /// frame the transfer is addressed to may receive it.
    pub(crate) fn adopt(&mut self, endpoint: u64, frame: FrameId) -> Option<bool> {
        let port = self.ports.get_mut(&endpoint)?;
        if !port.detached || port.recipient != Some(frame) {
            return None;
        }
        port.detached = false;
        port.owner = Some(frame);
        port.recipient = None;
        Some(std::mem::take(&mut port.close_pending))
    }

    /// The peer of `endpoint`, for the shim's "posted to itself" check.
    pub(crate) fn peer(&self, endpoint: u64) -> Option<u64> {
        self.ports.get(&endpoint).and_then(|port| port.peer)
    }

    /// Whether `frame` is the endpoint's recorded sender and may still hand
    /// it on in a transfer list.
    ///
    /// The post host functions are reachable from page script, so a transfer
    /// list must name only endpoints the caller detached; otherwise a hostile
    /// frame could aim another frame's in-transit port at a target it chooses
    /// (<https://html.spec.whatwg.org/multipage/web-messaging.html#transfer-receiving-steps>).
    pub(crate) fn in_transit_from(&self, endpoint: u64, frame: FrameId) -> bool {
        self.ports
            .get(&endpoint)
            .is_some_and(|port| port.detached && !port.closed && port.recipient == Some(frame))
    }

    /// Whether every endpoint in `endpoints` may be handed on by `frame`.
    pub(crate) fn all_in_transit_from(&self, endpoints: &[u64], frame: FrameId) -> bool {
        endpoints
            .iter()
            .all(|endpoint| self.in_transit_from(*endpoint, frame))
    }

    /// The frame whose realm holds `endpoint`'s port object, when it is
    /// materialized somewhere.
    pub(crate) fn owner(&self, endpoint: u64) -> Option<FrameId> {
        self.ports.get(&endpoint).and_then(|port| port.owner)
    }

    /// Closes every endpoint the frame owns, firing `close` at the peers.
    /// Called when the frame's realm (or the frame itself) goes away.
    pub(crate) fn close_frame_ports(&mut self, frame: FrameId, deliveries: &mut Vec<Delivery>) {
        let owned: Vec<u64> = self
            .ports
            .iter()
            .filter_map(|(&id, port)| (port.owner == Some(frame) && !port.closed).then_some(id))
            .collect();
        for endpoint in &owned {
            self.close(*endpoint, deliveries);
        }
    }

    /// Drops an endpoint that no realm can ever receive again (its frame was
    /// destroyed or the carrying message was dropped), disentangling its peer
    /// so the surviving port fires `close`
    /// (<https://html.spec.whatwg.org/multipage/web-messaging.html#disentangle>).
    pub(crate) fn forget(&mut self, endpoint: u64, deliveries: &mut Vec<Delivery>) {
        let Some(port) = self.ports.remove(&endpoint) else {
            return;
        };
        let Some(peer_id) = port.peer else {
            return;
        };
        let Some(peer) = self.ports.get_mut(&peer_id) else {
            return;
        };
        peer.peer = None;
        if peer.closed {
            return;
        }
        if peer.detached {
            peer.close_pending = true;
        } else if peer.owner.is_some() {
            deliveries.push(Delivery::PortClosed { endpoint: peer_id });
        }
    }
}

/// One task the engine must run in a named frame's realm.
pub(crate) enum Delivery {
    /// A posted window message, after its origin was re-checked against the
    /// target document's origin
    /// ([step 9.1](https://html.spec.whatwg.org/multipage/web-messaging.html#window-post-message-steps)).
    WindowMessage {
        /// The frame whose realm must decode and dispatch the message.
        target: FrameId,
        /// The frame whose realm encoded the message.
        source: FrameId,
        /// The sender's origin at post time, serialized.
        origin: String,
        /// The target origin the sender asked for (`*`, or an origin).
        target_origin: String,
        payload: String,
        /// Transferred endpoints, in transfer-list order.
        ports: Vec<u64>,
    },
    /// A message through a channel endpoint.
    PortMessage {
        endpoint: u64,
        payload: String,
        ports: Vec<u64>,
    },
    /// The peer of a closed endpoint: fire `close` at this port.
    PortClosed { endpoint: u64 },
}

/// Everything the frames of one renderer process share outside the `QuickJS`
/// heap.
pub(crate) struct Shared {
    pub(crate) tree: FrameTree,
    pub(crate) ports: PortTable,
    /// Completed sends waiting to be routed to their frame's task queue.
    pub(crate) deliveries: Vec<Delivery>,
    /// Frame ids are minted here so a frame's browsing context can be created
    /// from inside a script (an `iframe` insertion), not only from the engine.
    next_frame: u64,
    /// Frames whose browsing contexts a navigation destroyed, with the
    /// `iframe` containers that owned them; the engine drops their documents
    /// at its next turn.
    removed_frames: Vec<(FrameId, Option<NodeId>)>,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            tree: FrameTree::default(),
            ports: PortTable::default(),
            deliveries: Vec::new(),
            // `FrameId::MAIN` is 0 and is not minted here.
            next_frame: 1,
            removed_frames: Vec::new(),
        }
    }
}

impl Shared {
    /// A fresh frame id. Ids are never reused, so a window proxy for a dead
    /// frame can never alias a live one.
    pub(crate) fn allocate_frame(&mut self) -> FrameId {
        let frame = FrameId::new(self.next_frame);
        self.next_frame = self.next_frame.saturating_add(1);
        frame
    }

    /// Takes the pending deliveries for the engine to route.
    pub(crate) fn take_deliveries(&mut self) -> Vec<Delivery> {
        std::mem::take(&mut self.deliveries)
    }

    /// Disentangles every port the frame owns; used when its realm (or the
    /// frame itself) is destroyed
    /// (<https://html.spec.whatwg.org/multipage/web-messaging.html#disentangle>).
    pub(crate) fn close_frame_ports(&mut self, frame: FrameId) {
        self.ports.close_frame_ports(frame, &mut self.deliveries);
    }

    /// Drops transferred endpoints whose message will never be delivered,
    /// firing `close` at their peers.
    pub(crate) fn forget_endpoints(&mut self, endpoints: &[u64]) {
        for endpoint in endpoints {
            self.ports.forget(*endpoint, &mut self.deliveries);
        }
    }

    /// Requeues a port delivery whose task ran in a realm the port has left.
    pub(crate) fn requeue_port_message(&mut self, endpoint: u64, payload: String, ports: Vec<u64>) {
        self.ports
            .requeue(endpoint, payload, ports, &mut self.deliveries);
    }

    /// Re-aims a `close` event at an endpoint that moved realms.
    pub(crate) fn requeue_port_close(&mut self, endpoint: u64) {
        self.ports.requeue_close(endpoint, &mut self.deliveries);
    }

    /// Detaches every descendant browsing context of `frame` (a navigation
    /// replaces that frame's document, so its children are destroyed) and
    /// closes the ports they owned.
    pub(crate) fn detach_child_frames(&mut self, frame: FrameId) {
        let mut removed = Vec::new();
        let mut pending: Vec<FrameId> = self.tree.children(frame).to_vec();
        while let Some(child) = pending.pop() {
            pending.extend(self.tree.children(child).iter().copied());
            removed.push(child);
        }
        for child in &removed {
            let container = self.tree.remove(*child).map(|(_, container)| container);
            self.close_frame_ports(*child);
            self.removed_frames.push((*child, container));
        }
    }

    /// Takes the frames whose documents the engine must drop.
    pub(crate) fn take_removed_frames(&mut self) -> Vec<(FrameId, Option<NodeId>)> {
        std::mem::take(&mut self.removed_frames)
    }
}
