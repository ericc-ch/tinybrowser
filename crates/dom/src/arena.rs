//! The arena: flat slot array, generational handles, tree mutations.

use std::fmt;

use crate::children::Children;
use crate::id::NodeId;
use crate::node::{Attribute, Node, NodeKind, QualName};

/// Why a mutation was refused.
///
/// Stale handles and structural mistakes surface as values, never as panics,
/// so future JS bindings can map them straight onto DOM exceptions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DomError {
    /// A handle named a node that no longer exists.
    StaleNode,
    /// The move would place a node inside its own subtree.
    CycleForbidden,
    /// The operation does not apply to that kind or position of node.
    IllegalTarget,
}

impl fmt::Display for DomError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StaleNode => f.write_str("stale node handle"),
            Self::CycleForbidden => f.write_str("operation would create a cycle"),
            Self::IllegalTarget => f.write_str("operation not valid for this node"),
        }
    }
}

impl std::error::Error for DomError {}

/// One cell of the arena: current contents plus how many times it changed hands.
#[derive(Debug)]
struct Slot {
    generation: u32,
    node: Option<Node>,
}

/// A borrowed view of one live node, returned by [`Dom::get`].
///
/// The borrow keeps the arena frozen against mutation for its lifetime, which
/// is exactly the guarantee callers want when reading.
#[derive(Clone, Copy, Debug)]
pub struct NodeRef<'a> {
    node: &'a Node,
}

impl<'a> NodeRef<'a> {
    /// What kind of node this is, with its kind-specific data.
    ///
    /// The returned reference lives as long as the [`Dom`] borrow behind
    /// this view, not as long as the view itself.
    #[must_use]
    pub fn kind(&self) -> &'a NodeKind {
        &self.node.kind
    }
}

/// A document: every node lives inside one flat slot array.
///
/// All access goes through [`NodeId`] handles. Handles outliving their node
/// are harmless — lookups report absence — which is what will let the `QuickJS`
/// binding layer hold handles across garbage-collection cycles without
/// borrowing anything.
#[derive(Debug)]
pub struct Dom {
    slots: Vec<Slot>,
    free: Vec<u32>,
    document: NodeId,
}

impl Default for Dom {
    fn default() -> Self {
        Self::new()
    }
}

impl Dom {
    /// An empty document containing just the root `Document` node.
    #[must_use]
    pub fn new() -> Self {
        let root = Node {
            parent: None,
            children: Children::new(),
            kind: NodeKind::Document,
        };
        Self {
            slots: vec![Slot {
                generation: 0,
                node: Some(root),
            }],
            free: Vec::new(),
            document: NodeId::new(0, 0),
        }
    }

    /// The root `Document` node; every other node descends from it.
    #[must_use]
    pub fn document(&self) -> NodeId {
        self.document
    }

    /// Whether `id` names a currently live node.
    ///
    /// A destroyed node's handle fails here even though a different node may
    /// later occupy the same slot — that is the whole point of generations.
    #[must_use]
    pub fn contains(&self, id: NodeId) -> bool {
        self.live_slot(id).is_some()
    }

    /// Borrows the live node `id` names, or `None` for a stale handle.
    #[must_use]
    pub fn get(&self, id: NodeId) -> Option<NodeRef<'_>> {
        self.live_slot(id)
            .and_then(|slot| slot.node.as_ref())
            .map(|node| NodeRef { node })
    }

    /// The parent of `id`, or `None` if it is unparented or `id` is stale.
    #[must_use]
    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.live_slot(id)?.node.as_ref()?.parent
    }

    /// The children of `id` in document order.
    ///
    /// A stale handle yields an empty iteration rather than an error; pair
    /// with [`Dom::contains`] when absence versus death matters.
    pub fn children(&self, id: NodeId) -> std::slice::Iter<'_, NodeId> {
        self.live_slot(id)
            .and_then(|slot| slot.node.as_ref())
            .map(|node| node.children.iter())
            .unwrap_or_default()
    }

    /// Creates an element node, unattached until something appends it.
    ///
    /// # Panics
    ///
    /// Only if the arena exceeds `u32::MAX` slots — terabytes of RAM, not a
    /// reachable runtime condition; the bound guards the internal sentinel
    /// scheme (see `Children`).
    pub fn create_element(&mut self, name: QualName, attributes: Vec<Attribute>) -> NodeId {
        self.alloc(NodeKind::Element { name, attributes })
    }

    /// Creates a text node holding `data`.
    ///
    /// # Panics
    ///
    /// See [`Dom::create_element`]: unreachable except beyond `u32::MAX` nodes.
    pub fn create_text(&mut self, data: impl Into<String>) -> NodeId {
        self.alloc(NodeKind::Text { data: data.into() })
    }

    /// Creates a comment node holding `data`.
    ///
    /// # Panics
    ///
    /// See [`Dom::create_element`]: unreachable except beyond `u32::MAX` nodes.
    pub fn create_comment(&mut self, data: impl Into<String>) -> NodeId {
        self.alloc(NodeKind::Comment { data: data.into() })
    }

    /// Creates a doctype node.
    ///
    /// # Panics
    ///
    /// See [`Dom::create_element`]: unreachable except beyond `u32::MAX` nodes.
    pub fn create_doctype(
        &mut self,
        name: impl Into<String>,
        public_id: impl Into<String>,
        system_id: impl Into<String>,
    ) -> NodeId {
        self.alloc(NodeKind::Doctype {
            name: name.into(),
            public_id: public_id.into(),
            system_id: system_id.into(),
        })
    }

    /// Appends `child` as the last child of `parent`.
    ///
    /// Moving semantics: a `child` already attached elsewhere is detached
    /// first, mirroring DOM `appendChild`. Appending a node into itself or its
    /// own subtree is refused.
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if either handle is stale.
    /// - [`DomError::CycleForbidden`] if `child` is an ancestor of `parent`.
    pub fn append(&mut self, parent: NodeId, child: NodeId) -> Result<(), DomError> {
        self.ensure_alive(parent, child)?;
        if self.would_cycle(child, parent) {
            return Err(DomError::CycleForbidden);
        }
        self.unlink_from_current_parent(child);

        let list = self.children_mut(parent).ok_or(DomError::StaleNode)?;
        list.push(child);
        if let Some(node) = self.node_mut(child) {
            node.parent = Some(parent);
        }
        Ok(())
    }

    /// Inserts `node` immediately before `sibling` under sibling's parent.
    ///
    /// Moving semantics, like [`Dom::append`].
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if either handle is stale.
    /// - [`DomError::IllegalTarget`] if `sibling` has no parent to insert
    ///   beside, or `sibling == node`.
    /// - [`DomError::CycleForbidden`] if `node` is an ancestor of the parent.
    pub fn insert_before(&mut self, sibling: NodeId, node: NodeId) -> Result<(), DomError> {
        self.ensure_alive(sibling, node)?;
        if sibling == node || sibling == self.document {
            return Err(DomError::IllegalTarget);
        }
        let parent = self.parent(sibling).ok_or(DomError::IllegalTarget)?;
        if self.would_cycle(node, parent) {
            return Err(DomError::CycleForbidden);
        }
        self.unlink_from_current_parent(node);

        let list = self.children_mut(parent).ok_or(DomError::StaleNode)?;
        let position = list.position_of(sibling).ok_or(DomError::StaleNode)?;
        list.insert(position, node);
        if let Some(attached) = self.node_mut(node) {
            attached.parent = Some(parent);
        }
        Ok(())
    }

    /// Unlinks `id` from its parent, keeping the whole subtree alive.
    ///
    /// Idempotent: detaching an already-detached node succeeds. The document
    /// root cannot be detached.
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if `id` is stale.
    /// - [`DomError::IllegalTarget`] for the document root.
    pub fn detach(&mut self, id: NodeId) -> Result<(), DomError> {
        self.ensure_alive1(id)?;
        if id == self.document {
            return Err(DomError::IllegalTarget);
        }
        self.unlink_from_current_parent(id);
        if let Some(node) = self.node_mut(id) {
            node.parent = None;
        }
        Ok(())
    }

    /// Moves every child of `from` to the end of `to`'s child list.
    ///
    /// This is the bulk-move primitive behind foster parenting and the
    /// adoption agency: order is preserved and each moved child's parent
    /// pointer is updated.
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if either handle is stale.
    /// - [`DomError::CycleForbidden`] if `to` sits inside `from`'s subtree
    ///   (the move could then never complete).
    pub fn reparent_children(&mut self, from: NodeId, to: NodeId) -> Result<(), DomError> {
        self.ensure_alive(from, to)?;
        if from == to {
            return Ok(());
        }
        if self.would_cycle(from, to) {
            return Err(DomError::CycleForbidden);
        }
        let moved = self
            .children_mut(from)
            .map(Children::take_all)
            .ok_or(DomError::StaleNode)?;
        let list = self.children_mut(to).ok_or(DomError::StaleNode)?;
        for id in &moved {
            list.push(*id);
        }
        for id in moved {
            if let Some(node) = self.node_mut(id) {
                node.parent = Some(to);
            }
        }
        Ok(())
    }

    /// Replaces the data of the text node `id`.
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if `id` is stale.
    /// - [`DomError::IllegalTarget`] if `id` is not a text node.
    pub fn set_text(&mut self, id: NodeId, data: impl Into<String>) -> Result<(), DomError> {
        self.set_data(
            id,
            |kind| match kind {
                NodeKind::Text { data } => Some(data),
                _ => None,
            },
            data.into(),
        )
    }

    /// Replaces the data of the comment node `id`.
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if `id` is stale.
    /// - [`DomError::IllegalTarget`] if `id` is not a comment node.
    pub fn set_comment(&mut self, id: NodeId, data: impl Into<String>) -> Result<(), DomError> {
        self.set_data(
            id,
            |kind| match kind {
                NodeKind::Comment { data } => Some(data),
                _ => None,
            },
            data.into(),
        )
    }

    /// Destroys `id` and its entire subtree, recycling their slots.
    ///
    /// Every handle into the destroyed region goes stale at once. Destroying
    /// the document root is refused.
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if `id` is stale.
    /// - [`DomError::IllegalTarget`] for the document root.
    pub fn destroy(&mut self, id: NodeId) -> Result<(), DomError> {
        self.ensure_alive1(id)?;
        if id == self.document {
            return Err(DomError::IllegalTarget);
        }
        self.unlink_from_current_parent(id);

        let mut pending = vec![id];
        while let Some(current) = pending.pop() {
            let index = current.index();
            let slot = &mut self.slots[index];
            if let Some(node) = slot.node.take() {
                pending.extend(node.children.iter().copied());
            }
            slot.generation = slot.generation.wrapping_add(1);
            self.free.push(current.slot);
        }
        Ok(())
    }

    // ── internals ────────────────────────────────────────────────────────

    fn live_slot(&self, id: NodeId) -> Option<&Slot> {
        self.slots
            .get(id.index())
            .filter(|slot| slot.generation == id.generation && slot.node.is_some())
    }

    fn node_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.slots
            .get_mut(id.index())
            .filter(|slot| slot.generation == id.generation)
            .and_then(|slot| slot.node.as_mut())
    }

    fn children_mut(&mut self, id: NodeId) -> Option<&mut Children> {
        self.node_mut(id).map(|node| &mut node.children)
    }

    fn ensure_alive(&self, a: NodeId, b: NodeId) -> Result<(), DomError> {
        if self.contains(a) && self.contains(b) {
            Ok(())
        } else {
            Err(DomError::StaleNode)
        }
    }

    fn ensure_alive1(&self, a: NodeId) -> Result<(), DomError> {
        if self.contains(a) {
            Ok(())
        } else {
            Err(DomError::StaleNode)
        }
    }

    /// True iff placing `subtree` under `into` would nest it inside itself.
    fn would_cycle(&self, subtree: NodeId, into: NodeId) -> bool {
        let mut cursor = Some(into);
        while let Some(id) = cursor {
            if id == subtree {
                return true;
            }
            cursor = self.parent(id);
        }
        false
    }

    fn unlink_from_current_parent(&mut self, id: NodeId) {
        if let Some(old_parent) = self.parent(id)
            && let Some(list) = self.children_mut(old_parent)
            && let Some(position) = list.position_of(id)
        {
            list.remove_at(position);
        }
    }

    fn set_data(
        &mut self,
        id: NodeId,
        extract: impl Fn(&mut NodeKind) -> Option<&mut String>,
        data: String,
    ) -> Result<(), DomError> {
        let node = self.node_mut(id).ok_or(DomError::StaleNode)?;
        match extract(&mut node.kind) {
            Some(field) => {
                *field = data;
                Ok(())
            }
            None => Err(DomError::IllegalTarget),
        }
    }

    /// Places a fresh node into a recycled or newly grown slot.
    ///
    /// # Panics
    ///
    /// Only past `u32::MAX` slots; see [`Dom::create_element`]. The bound is
    /// what makes `Children`'s empty-cell sentinel sound, so it is enforced
    /// with a loud, documented stop rather than silent wraparound.
    fn alloc(&mut self, kind: NodeKind) -> NodeId {
        let node = Node {
            parent: None,
            children: Children::new(),
            kind,
        };
        if let Some(slot) = self.free.pop() {
            // Lossless widening cast (u32 → usize); no From impl exists for it.
            let index = slot as usize;
            let generation = self.slots[index].generation.wrapping_add(1);
            self.slots[index] = Slot {
                generation,
                node: Some(node),
            };
            return NodeId::new(slot, generation);
        }
        let slot = u32::try_from(self.slots.len()).expect("arena exhausted: >u32::MAX nodes");
        self.slots.push(Slot {
            generation: 0,
            node: Some(node),
        });
        NodeId::new(slot, 0)
    }
}
