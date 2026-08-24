//! The arena: flat slot array, generational handles, tree mutations.

use std::cell::Cell;
use std::fmt;
use std::marker::PhantomData;

use crate::id::NodeId;
use crate::node::{Attribute, Node, NodeKind, QualName};

/// Why a mutation was refused.
///
/// Stale handles and structural mistakes surface as values, never as panics,
/// so future JS bindings can map them straight onto DOM exceptions — one
/// variant per exception class, not per call site.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DomError {
    /// A handle named a node that no longer exists.
    StaleNode,
    /// The move would place a node inside its own subtree.
    CycleForbidden,
    /// The document root may not gain a parent, be detached, drained, or
    /// destroyed. (Maps to `HierarchyRequestError`.)
    ProtectedNode,
    /// The operation does not apply to that kind of node (setting text data
    /// on an element, attributes on a text node). (Maps to a type error at
    /// the binding layer.)
    WrongNodeType,
    /// An insert was requested beside a node with no parent to sit under.
    /// (Maps to `NotFoundError`.)
    NoParent,
    /// A node was asked to be inserted before itself.
    SelfInsert,
}

impl fmt::Display for DomError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StaleNode => f.write_str("stale node handle"),
            Self::CycleForbidden => f.write_str("operation would create a cycle"),
            Self::ProtectedNode => f.write_str("document root is protected"),
            Self::WrongNodeType => f.write_str("operation not valid for this node kind"),
            Self::NoParent => f.write_str("target has no parent to insert beside"),
            Self::SelfInsert => f.write_str("cannot insert a node before itself"),
        }
    }
}

impl std::error::Error for DomError {}

/// One cell of the arena: current contents plus how many times it changed hands.
///
/// Crate-visible only so selector matching can take a node's storage address
/// as a stable identity token (see [`Dom::cache_identity`]).
#[derive(Debug)]
pub(crate) struct Slot {
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
///
/// [`Send`] but deliberately not [`Sync`]: a `Dom` may be handed between
/// workers, but two threads can never touch one simultaneously (one worker
/// per page; see the dom-layer wayfinding tickets). The marker field below is
/// what suppresses the otherwise-auto-derived `Sync`.
#[derive(Debug)]
pub struct Dom {
    slots: Vec<Slot>,
    free: Vec<u32>,
    document: NodeId,
    /// `Cell<()>` is `Send` + `!Sync`; `PhantomData` makes `Dom` inherit
    /// exactly that split. Deleting this field would silently re-derive
    /// `Sync` — which is the point: that deletion has to be a conscious act.
    _share_forbidden: PhantomData<Cell<()>>,
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
            children: Vec::new(),
            kind: NodeKind::Document,
        };
        Self {
            slots: vec![Slot {
                generation: 0,
                node: Some(root),
            }],
            free: Vec::new(),
            document: NodeId::new(0, 0),
            _share_forbidden: PhantomData,
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

    /// The children of `id` in document order, or `None` for a stale handle.
    ///
    /// Unlike the other reads this cannot quietly report "no children" for a
    /// destroyed node — childless and dead are different answers.
    #[must_use]
    pub fn children(&self, id: NodeId) -> Option<std::slice::Iter<'_, NodeId>> {
        self.live_slot(id)
            .and_then(|slot| slot.node.as_ref())
            .map(|node| node.children.iter())
    }

    /// A stable identity token for `id`, for selector-engine caches.
    ///
    /// One live node owns exactly one slot, and matching runs under a shared
    /// borrow that freezes the arena — so the slot's address is unique per
    /// node and stable for the whole query. Detached nodes keep their slots,
    /// so identity survives detachment too.
    #[must_use]
    pub(crate) fn cache_identity(&self, id: NodeId) -> &Slot {
        &self.slots[id.index()]
    }

    /// Creates an element node, unattached until something appends it.
    ///
    /// # Panics
    ///
    /// Only if the arena exceeds `u32::MAX` slots — terabytes of RAM, not a
    /// reachable runtime condition; the bound guards the handle width.
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

    /// Creates an empty document fragment, unattached like every fresh node.
    ///
    /// Fragments are containers outside the main tree — the contents root of
    /// `<template>` elements (the parser associates them via its own map) and,
    /// later, the context node for `innerHTML`-style fragment parsing.
    ///
    /// # Panics
    ///
    /// See [`Dom::create_element`]: unreachable except beyond `u32::MAX` nodes.
    pub fn create_fragment(&mut self) -> NodeId {
        self.alloc(NodeKind::Fragment)
    }

    /// Appends `child` as the last child of `parent`.
    ///
    /// Moving semantics: a `child` already attached elsewhere is detached
    /// first, mirroring DOM `appendChild`. Appending a node into itself or its
    /// own subtree is refused.
    ///
    /// # Panics
    ///
    /// Only on an internal invariant defect (a verified-live node missing its
    /// child list) — never on user input; input failures return errors.
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if either handle is stale.
    /// - [`DomError::ProtectedNode`] if `child` is the document root.
    /// - [`DomError::CycleForbidden`] if `child` is an ancestor of `parent`.
    pub fn append(&mut self, parent: NodeId, child: NodeId) -> Result<(), DomError> {
        self.ensure_alive(parent, child)?;
        if child == self.document {
            // The root must never gain a parent — that is how a document
            // gets orphaned from itself. (Maps to HierarchyRequestError.)
            return Err(DomError::ProtectedNode);
        }
        if self.would_cycle(child, parent) {
            return Err(DomError::CycleForbidden);
        }
        self.unlink_from_current_parent(child);

        // Defect guards, not input errors: liveness was checked above, so
        // `None` here means the parent-pointer/child-list duality is broken.
        // Panicking beats reporting a lying "stale node" to callers.
        let list = self
            .children_mut(parent)
            .expect("verified-live parent has no child list");
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
    /// # Panics
    ///
    /// Only on an internal invariant defect (a live sibling missing from its
    /// own parent's list) — never on user input.
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if either handle is stale.
    /// - [`DomError::NoParent`] if `sibling` has no parent to insert beside.
    /// - [`DomError::SelfInsert`] if `sibling == node`.
    /// - [`DomError::ProtectedNode`] if `sibling` is the document root.
    /// - [`DomError::CycleForbidden`] if `node` is an ancestor of the parent.
    pub fn insert_before(&mut self, sibling: NodeId, node: NodeId) -> Result<(), DomError> {
        self.ensure_alive(sibling, node)?;
        if sibling == node {
            return Err(DomError::SelfInsert);
        }
        if sibling == self.document {
            return Err(DomError::ProtectedNode);
        }
        let parent = self.parent(sibling).ok_or(DomError::NoParent)?;
        if self.would_cycle(node, parent) {
            return Err(DomError::CycleForbidden);
        }
        self.unlink_from_current_parent(node);

        // Defect guards, not input errors: `parent` and `sibling` were both
        // verified live above, so a miss here means the parent-pointer/
        // child-list duality is broken. Panicking beats reporting a lying
        // "stale node" to callers.
        let list = self
            .children_mut(parent)
            .expect("verified-live parent has no child list");
        let position = list
            .iter()
            .position(|&entry| entry == sibling)
            .expect("live sibling missing from its own parent's list");
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
    /// - [`DomError::ProtectedNode`] for the document root.
    pub fn detach(&mut self, id: NodeId) -> Result<(), DomError> {
        self.require_live(id)?;
        if id == self.document {
            return Err(DomError::ProtectedNode);
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
    /// # Panics
    ///
    /// Only on an internal invariant defect (a verified-live node missing its
    /// child list) — never on user input.
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if either handle is stale.
    /// - [`DomError::ProtectedNode`] if `from` is the document root (the
    ///   move could then never complete without stranding the document).
    /// - [`DomError::CycleForbidden`] if `to` sits inside `from`'s subtree.
    pub fn reparent_children(&mut self, from: NodeId, to: NodeId) -> Result<(), DomError> {
        self.ensure_alive(from, to)?;
        if from == to {
            return Ok(());
        }
        if from == self.document {
            // Draining the root would strand the entire document inside an
            // arbitrary detached subtree. (Maps to HierarchyRequestError.)
            return Err(DomError::ProtectedNode);
        }
        if self.would_cycle(from, to) {
            return Err(DomError::CycleForbidden);
        }
        // Defect guards, not input errors: both handles were verified live
        // above, so a miss here means the parent-pointer/child-list duality
        // is broken. Panicking beats reporting a lying "stale node".
        let moved = self
            .children_mut(from)
            .map(std::mem::take)
            .expect("verified-live `from` has no child list");
        let list = self
            .children_mut(to)
            .expect("verified-live `to` has no child list");
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

    /// Adds each attribute that `id` does not already carry, matched by
    /// qualified name.
    ///
    /// The adapter's `add_attrs_if_missing` landing pad: html5ever merges
    /// attributes from repeated start-tag tokens through this call.
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if `id` is stale.
    /// - [`DomError::WrongNodeType`] if `id` is not an element.
    pub fn add_attrs_if_missing(
        &mut self,
        id: NodeId,
        attrs: Vec<Attribute>,
    ) -> Result<(), DomError> {
        let node = self.node_mut(id).ok_or(DomError::StaleNode)?;
        let NodeKind::Element { attributes, .. } = &mut node.kind else {
            return Err(DomError::WrongNodeType);
        };
        for attr in attrs {
            if !attributes.iter().any(|existing| existing.name == attr.name) {
                attributes.push(attr);
            }
        }
        Ok(())
    }

    /// Replaces the data of the text node `id`.
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if `id` is stale.
    /// - [`DomError::WrongNodeType`] if `id` is not a text node.
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
    /// - [`DomError::WrongNodeType`] if `id` is not a comment node.
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
    /// - [`DomError::ProtectedNode`] for the document root.
    pub fn destroy(&mut self, id: NodeId) -> Result<(), DomError> {
        self.require_live(id)?;
        if id == self.document {
            return Err(DomError::ProtectedNode);
        }
        self.unlink_from_current_parent(id);

        let mut pending = vec![id];
        while let Some(current) = pending.pop() {
            let index = current.index();
            let slot = &mut self.slots[index];
            if let Some(node) = slot.node.take() {
                pending.extend(node.children.iter().copied());
            }
            // No generation tick here: emptiness is what makes the handle
            // dead (`live_slot` requires `node.is_some()`), and the single
            // tick happens at reallocation in `alloc`.
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

    fn children_mut(&mut self, id: NodeId) -> Option<&mut Vec<NodeId>> {
        self.node_mut(id).map(|node| &mut node.children)
    }

    fn ensure_alive(&self, a: NodeId, b: NodeId) -> Result<(), DomError> {
        if self.contains(a) && self.contains(b) {
            Ok(())
        } else {
            Err(DomError::StaleNode)
        }
    }

    /// Single-handle variant of [`Dom::ensure_alive`].
    fn require_live(&self, a: NodeId) -> Result<(), DomError> {
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

    /// Removes `id` from whichever list currently holds it.
    ///
    /// Defect policy, like every other structural site in this module: `id`
    /// was verified live, so a `Some` parent must own a list and that list
    /// must name `id`. Parent-pointer/child-list divergence is arena
    /// corruption — panicking beats silently producing a node with two
    /// parents (or none), which later mutations would compound.
    fn unlink_from_current_parent(&mut self, id: NodeId) {
        if let Some(old_parent) = self.parent(id) {
            let list = self
                .children_mut(old_parent)
                .expect("live parent has no child list");
            let position = list
                .iter()
                .position(|&entry| entry == id)
                .expect("child missing from the very list its parent pointer names");
            list.remove(position);
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
            None => Err(DomError::WrongNodeType),
        }
    }

    /// Places a fresh node into a recycled or newly grown slot.
    ///
    /// # Panics
    ///
    /// Only when the arena would need more than `u32::MAX` slots — hundreds
    /// of GB of RAM, not a reachable runtime condition; the bound guards the
    /// handle width (`NodeId.slot` is a `u32`).
    fn alloc(&mut self, kind: NodeKind) -> NodeId {
        let node = Node {
            parent: None,
            children: Vec::new(),
            kind,
        };
        if let Some(slot) = self.free.pop() {
            // Lossless widening cast (u32 → usize); no From impl exists for it.
            let index = slot as usize;
            // The single generation tick per change of hands happens here.
            let generation = self.slots[index].generation.wrapping_add(1);
            self.slots[index] = Slot {
                generation,
                node: Some(node),
            };
            return NodeId::new(slot, generation);
        }
        // The bound is the `u32` handle width itself; exhausting it requires
        // >4 billion slots (hundreds of GB of arena), an impossible runtime
        // condition rather than an error to handle. Generations wrap after
        // 2^32 recycles of one slot; that residual ABA window is accepted by
        // design — exploiting it needs billions of death/reuse cycles on a
        // single slot while some outside handle to that slot still exists.
        let slot = u32::try_from(self.slots.len())
            .expect("arena exhausted: >u32::MAX slots requires hundreds of GB of RAM");
        self.slots.push(Slot {
            generation: 0,
            node: Some(node),
        });
        NodeId::new(slot, 0)
    }
}
