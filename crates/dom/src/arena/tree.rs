//! Node storage, generational handles, and structural tree links.

use std::iter::FusedIterator;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::id::NodeId;
use crate::node::NodeKind;

/// One node's structural links and kind-specific data. Only `Tree` can write
/// the links; operation code sees just the mutable kind.
///
/// The five intrusive links give O(1) access to either sibling and either end
/// of a child run. They cost 40 bytes per node versus the former child vector,
/// but make repeated `:nth-*` walks linear
/// (<https://drafts.csswg.org/selectors-4/#the-nth-child-pseudo>).
#[derive(Debug)]
struct Node {
    parent: Option<NodeId>,
    first_child: Option<NodeId>,
    last_child: Option<NodeId>,
    previous_sibling: Option<NodeId>,
    next_sibling: Option<NodeId>,
    kind: NodeKind,
}

// A document id identifies one arena independently of its node generations.
static NEXT_DOCUMENT_ID: AtomicU32 = AtomicU32::new(0);

/// One node slot. Selector caches use the address of a live slot as identity.
#[derive(Debug)]
pub(crate) struct Slot {
    generation: u32,
    node: Option<Node>,
}

#[derive(Debug)]
pub(crate) struct Tree {
    slots: Vec<Slot>,
    free: Vec<u32>,
    document: NodeId,
}

/// A double-ended walk through a parent's sibling links.
pub struct Children<'a> {
    tree: &'a Tree,
    front: Option<NodeId>,
    back: Option<NodeId>,
}

impl Iterator for Children<'_> {
    type Item = NodeId;

    fn next(&mut self) -> Option<NodeId> {
        let current = self.front?;
        if self.back == Some(current) {
            self.front = None;
            self.back = None;
        } else {
            self.front = self.tree.next_sibling(current);
        }
        Some(current)
    }
}

impl DoubleEndedIterator for Children<'_> {
    fn next_back(&mut self) -> Option<NodeId> {
        let current = self.back?;
        if self.front == Some(current) {
            self.front = None;
            self.back = None;
        } else {
            self.back = self.tree.previous_sibling(current);
        }
        Some(current)
    }
}

impl FusedIterator for Children<'_> {}

impl Tree {
    pub(super) fn new() -> Self {
        let document_id = NEXT_DOCUMENT_ID.fetch_add(1, Ordering::Relaxed);
        let root = Node {
            parent: None,
            first_child: None,
            last_child: None,
            previous_sibling: None,
            next_sibling: None,
            kind: NodeKind::Document,
        };
        Self {
            slots: vec![Slot {
                generation: 0,
                node: Some(root),
            }],
            free: Vec::new(),
            document: NodeId::new(document_id, 0, 0),
        }
    }

    pub(crate) fn document(&self) -> NodeId {
        self.document
    }

    pub(super) fn contains(&self, id: NodeId) -> bool {
        self.live_slot(id).is_some()
    }

    pub(crate) fn kind(&self, id: NodeId) -> Option<&NodeKind> {
        Some(&self.live_slot(id)?.node.as_ref()?.kind)
    }

    pub(super) fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.live_slot(id)?.node.as_ref()?.parent
    }

    pub(crate) fn children(&self, id: NodeId) -> Option<Children<'_>> {
        let node = self.live_slot(id)?.node.as_ref()?;
        Some(Children {
            tree: self,
            front: node.first_child,
            back: node.last_child,
        })
    }

    pub(super) fn first_child(&self, id: NodeId) -> Option<NodeId> {
        self.live_slot(id)?.node.as_ref()?.first_child
    }

    /// The value of a no-namespace attribute with this exact local name.
    pub(crate) fn no_namespace_attribute(&self, id: NodeId, local: &str) -> Option<&str> {
        let NodeKind::Element { attributes, .. } = self.kind(id)? else {
            return None;
        };
        attributes
            .iter()
            .find(|attribute| {
                attribute.name.ns.as_ref().is_empty() && attribute.name.local.as_ref() == local
            })
            .map(|attribute| attribute.value.as_str())
    }

    pub(super) fn last_child(&self, id: NodeId) -> Option<NodeId> {
        self.live_slot(id)?.node.as_ref()?.last_child
    }

    pub(super) fn previous_sibling(&self, id: NodeId) -> Option<NodeId> {
        self.live_slot(id)?.node.as_ref()?.previous_sibling
    }

    pub(super) fn next_sibling(&self, id: NodeId) -> Option<NodeId> {
        self.live_slot(id)?.node.as_ref()?.next_sibling
    }

    pub(super) fn live_slot(&self, id: NodeId) -> Option<&Slot> {
        if id.document != self.document.document {
            return None;
        }
        self.slots
            .get(id.index())
            .filter(|slot| slot.generation == id.generation && slot.node.is_some())
    }

    pub(super) fn kind_mut(&mut self, id: NodeId) -> Option<&mut NodeKind> {
        Some(&mut self.node_mut(id)?.kind)
    }

    fn node_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        if id.document != self.document.document {
            return None;
        }
        self.slots
            .get_mut(id.index())
            .filter(|slot| slot.generation == id.generation)
            .and_then(|slot| slot.node.as_mut())
    }

    /// Links the unparented `node` under `parent` immediately before
    /// `before` (or as last child when `before` is `None`).
    ///
    /// `before`, when present, must already be a child of `parent`; `node`
    /// must carry no parent and no sibling links. This is the single place
    /// that writes the four link fields on insertion.
    pub(super) fn insert_linked(&mut self, parent: NodeId, node: NodeId, before: Option<NodeId>) {
        let previous = match before {
            Some(before) => self.previous_sibling(before),
            None => self.last_child(parent),
        };
        {
            let attached = self.node_mut(node).expect("verified-live node has no slot");
            attached.parent = Some(parent);
            attached.previous_sibling = previous;
            attached.next_sibling = before;
        }
        match previous {
            Some(previous) => {
                self.node_mut(previous)
                    .expect("linked previous sibling has no slot")
                    .next_sibling = Some(node);
            }
            None => {
                self.node_mut(parent)
                    .expect("verified-live parent has no slot")
                    .first_child = Some(node);
            }
        }
        match before {
            Some(before) => {
                self.node_mut(before)
                    .expect("linked reference child has no slot")
                    .previous_sibling = Some(node);
            }
            None => {
                self.node_mut(parent)
                    .expect("verified-live parent has no slot")
                    .last_child = Some(node);
            }
        }
    }

    /// Unlinks one attached node, preserving its slot and descendants
    /// (<https://dom.spec.whatwg.org/#concept-node-remove>).
    pub(super) fn unlink_linked(&mut self, id: NodeId) {
        let Some((parent, previous, next)) = self.node_mut(id).and_then(|node| {
            node.parent
                .map(|parent| (parent, node.previous_sibling, node.next_sibling))
        }) else {
            return;
        };
        if let Some(previous) = previous {
            self.node_mut(previous)
                .expect("previous sibling has no slot")
                .next_sibling = next;
        } else {
            let first = self
                .node_mut(parent)
                .expect("live parent has no slot")
                .first_child;
            assert_eq!(first, Some(id), "parent's head is not the unlinked node");
            self.node_mut(parent)
                .expect("live parent has no slot")
                .first_child = next;
        }
        if let Some(next) = next {
            self.node_mut(next)
                .expect("next sibling has no slot")
                .previous_sibling = previous;
        } else {
            let last = self
                .node_mut(parent)
                .expect("live parent has no slot")
                .last_child;
            assert_eq!(last, Some(id), "parent's tail is not the unlinked node");
            self.node_mut(parent)
                .expect("live parent has no slot")
                .last_child = previous;
        }
        let detached = self.node_mut(id).expect("live node has no slot");
        detached.parent = None;
        detached.previous_sibling = None;
        detached.next_sibling = None;
    }

    /// A freed slot stays empty until reuse ticks its generation.
    pub(super) fn retire(&mut self, id: NodeId, pending: &mut Vec<NodeId>) {
        pending.extend(self.children(id).expect("retiring a live node"));
        self.slots[id.index()].node = None;
        self.free.push(id.slot);
    }

    /// Places a fresh node into a recycled or newly grown slot.
    ///
    /// # Panics
    ///
    /// Only when more than `u32::MAX` slots would be needed.
    pub(super) fn alloc(&mut self, kind: NodeKind) -> NodeId {
        let node = Node {
            parent: None,
            first_child: None,
            last_child: None,
            previous_sibling: None,
            next_sibling: None,
            kind,
        };
        if let Some(slot) = self.free.pop() {
            let index = slot as usize;
            let generation = self.slots[index].generation.wrapping_add(1);
            self.slots[index] = Slot {
                generation,
                node: Some(node),
            };
            return NodeId::new(self.document.document, slot, generation);
        }
        let slot = u32::try_from(self.slots.len())
            .expect("arena exhausted: >u32::MAX slots requires hundreds of GB of RAM");
        self.slots.push(Slot {
            generation: 0,
            node: Some(node),
        });
        NodeId::new(self.document.document, slot, 0)
    }
}
