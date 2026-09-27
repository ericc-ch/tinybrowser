//! The arena: flat slot array, generational handles, tree mutations.

mod journal;
mod metadata;
mod shadow;
mod tree;

use std::cell::Cell;
use std::fmt;
use std::marker::PhantomData;

use crate::form::{self, FormState};
use crate::id::NodeId;
use crate::named::{self, NamedIndex};
use crate::node::{
    Attribute, LocalName, Namespace, NodeKind, Prefix, QualName, html_namespace,
    html_qualified_name_eq, qualified_name_eq,
};

use self::journal::MutationJournal;
use self::metadata::Metadata;
use self::shadow::ShadowState;
pub use self::tree::Children;
use self::tree::Slot;
pub use self::tree::Tree;
use crate::lifecycle::{self, ConnectionState};

/// The document-compatibility mode a query runs under: what html5ever's
/// tree builder reports and parsed pages carry.
///
/// It changes exactly one matching behavior: in full quirks mode, class
/// and id selector values compare ASCII-case-insensitively (the WHATWG
/// id/class quirk). Standards and limited-quirks modes stay exact.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum QuirksMode {
    /// Standards mode: full CSS case rules.
    #[default]
    NoQuirks,
    /// Limited quirks: same selector rules as standards mode.
    LimitedQuirks,
    /// Full quirks: legacy case-insensitive class/id matching.
    Quirks,
}

/// One recorded tree mutation, for `MutationObserver` delivery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mutation {
    /// Children added and/or removed on `target`, in one operation.
    ChildList {
        target: NodeId,
        added: Vec<NodeId>,
        removed: Vec<NodeId>,
        previous: Option<NodeId>,
        next: Option<NodeId>,
    },
    /// An attribute set, changed, or removed on `target`.
    Attributes {
        target: NodeId,
        name: String,
        namespace: String,
        old_value: Option<String>,
    },
    /// Character data replaced on `target`.
    CharacterData { target: NodeId, old_value: String },
}

/// Why a mutation was refused.
///
/// Stale handles and structural mistakes surface as values, never as panics,
/// so future JS bindings can map them straight onto DOM exceptions: one
/// variant per exception class, not per call site.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DomError {
    /// A handle named a node that no longer exists.
    StaleNode,
    /// The move would place a node inside its own subtree. Browsers also
    /// report this as `HierarchyRequestError`; kept distinct so a binding
    /// can map both without losing the cycle case.
    CycleForbidden,
    /// The tree's hierarchy or content model forbids the operation:
    /// a document gaining a second root or a misplaced doctype, character
    /// data under a document, a leaf node asked to parent children, the
    /// document root asked to gain a parent. (Maps to
    /// `HierarchyRequestError`; see `Document::ensure_pre_insert_validity`.)
    HierarchyRequest,
    /// The operation does not apply to that kind of node (setting text data
    /// on an element, attributes on a text node). (Maps to a type error at
    /// the binding layer.)
    WrongNodeType,
    /// An insert was requested beside a node with no parent to sit under.
    /// (Maps to `NotFoundError`.)
    NoParent,
    /// The control's state forbids the operation, such as setting a non-empty
    /// `value` on a `type=file` input. (Maps to `InvalidStateError`.)
    InvalidState,
}

impl fmt::Display for DomError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StaleNode => f.write_str("stale node handle"),
            Self::CycleForbidden => f.write_str("operation would create a cycle"),
            Self::HierarchyRequest => {
                f.write_str("hierarchy or content model forbids this operation")
            }
            Self::WrongNodeType => f.write_str("operation not valid for this node kind"),
            Self::NoParent => f.write_str("target has no parent to insert beside"),
            Self::InvalidState => f.write_str("operation invalid for the control's state"),
        }
    }
}

impl std::error::Error for DomError {}

/// A document: every node lives inside one flat slot array.
///
/// All access goes through [`NodeId`] handles. Handles outliving their node
/// are harmless (lookups report absence), which lets the `QuickJS` binding
/// layer hold handles across garbage-collection cycles without borrowing
/// anything.
///
/// [`Send`] but deliberately not [`Sync`]: a `Document` may be handed between
/// workers, but two threads can never touch one simultaneously (one worker
/// per document). The marker field below is what suppresses the
/// otherwise-auto-derived `Sync`.
#[derive(Debug)]
pub struct Document {
    pub(crate) tree: Tree,
    metadata: Metadata,
    shadow: ShadowState,
    pub(crate) form: FormState,
    journal: MutationJournal,
    pub(crate) connections: ConnectionState,
    pub(crate) named: NamedIndex,
    /// `Cell<()>` is `Send` + `!Sync`; `PhantomData` makes `Document` inherit
    /// exactly that split. Deleting this field would silently re-derive
    /// `Sync`, which is the point: that deletion has to be a conscious act.
    _share_forbidden: PhantomData<Cell<()>>,
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

impl Document {
    /// An empty document containing just the root `Document` node.
    #[must_use]
    pub fn new() -> Self {
        Self {
            tree: Tree::new(),
            metadata: Metadata::default(),
            shadow: ShadowState::default(),
            form: FormState::default(),
            journal: MutationJournal::default(),
            connections: ConnectionState::default(),
            named: NamedIndex::default(),
            _share_forbidden: PhantomData,
        }
    }

    /// Turns mutation recording on or off; recording costs nothing while no
    /// `MutationObserver` is registered.
    pub fn set_record_mutations(&mut self, recording: bool) {
        self.journal.set_recording(recording);
    }

    /// Drains the recorded mutations in order.
    pub fn take_mutations(&mut self) -> Vec<Mutation> {
        self.journal.take()
    }

    /// `id`'s ancestors, nearest first, `id` excluded.
    pub fn ancestors(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        std::iter::successors(self.parent(id), |&node| self.parent(node))
    }

    fn record(&mut self, mutation: Mutation) {
        self.journal.record(mutation);
    }

    /// A counter that changes on every recorded mutation.
    ///
    /// Consumers that must rescan the tree (frame order) compare this instead
    /// of walking the whole document on every turn.
    #[must_use]
    pub fn mutation_serial(&self) -> u64 {
        self.journal.serial()
    }

    /// The root `Document` node; every other node descends from it.
    #[must_use]
    pub fn document(&self) -> NodeId {
        self.tree.document()
    }

    /// A read-only view of node storage and traversal.
    #[must_use]
    pub fn tree(&self) -> &Tree {
        &self.tree
    }

    /// This document's arena id, for per-document renderer lookups.
    #[must_use]
    pub fn document_id(&self) -> u32 {
        self.tree.document().document_id()
    }

    /// Whether `id` names a currently live node.
    ///
    /// A destroyed node's handle fails here even though a different node may
    /// later occupy the same slot; that is the whole point of generations.
    #[must_use]
    pub fn contains(&self, id: NodeId) -> bool {
        self.tree.contains(id)
    }

    /// What kind of node `id` names, or `None` for a stale handle.
    #[must_use]
    pub fn kind(&self, id: NodeId) -> Option<&NodeKind> {
        self.tree.kind(id)
    }

    /// The parent of `id`, or `None` if it is unparented or `id` is stale.
    #[must_use]
    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.tree.parent(id)
    }

    /// The children of `id` in document order, or `None` for a stale handle.
    ///
    /// Mirrors DOM `childNodes`: every live node answers a list; leaves
    /// (text, comment, doctype) answer an empty one because the mutation
    /// gate below refuses to give them children. Childless and dead remain
    /// different answers; only staleness is `None`.
    #[must_use]
    pub fn children(&self, id: NodeId) -> Option<Children<'_>> {
        self.tree.children(id)
    }

    /// The first child of `id`, or `None` when it has none or is stale.
    #[must_use]
    pub fn first_child(&self, id: NodeId) -> Option<NodeId> {
        self.tree.first_child(id)
    }

    /// The last child of `id`, or `None` when it has none or is stale.
    #[must_use]
    pub fn last_child(&self, id: NodeId) -> Option<NodeId> {
        self.tree.last_child(id)
    }

    /// The sibling immediately before `id`, or `None`.
    #[must_use]
    pub fn previous_sibling(&self, id: NodeId) -> Option<NodeId> {
        self.tree.previous_sibling(id)
    }

    /// The sibling immediately after `id`, or `None`.
    #[must_use]
    pub fn next_sibling(&self, id: NodeId) -> Option<NodeId> {
        self.tree.next_sibling(id)
    }

    /// The sibling of `id` adjacent in the given direction, or `None`.
    #[must_use]
    pub fn sibling(&self, id: NodeId, forward: bool) -> Option<NodeId> {
        if forward {
            self.next_sibling(id)
        } else {
            self.previous_sibling(id)
        }
    }

    /// Children in the flattened tree used for style and box construction.
    /// A shadow host yields its shadow tree, and a slot yields its assigned
    /// nodes when any exist
    /// (<https://drafts.csswg.org/css-scoping/#flattening>).
    #[must_use]
    pub fn rendered_children(&self, id: NodeId) -> Vec<NodeId> {
        if let Some(root) = self.shadow_root(id) {
            return self.rendered_children(root);
        }
        if self.is_html_slot(id) {
            let assigned = self.assigned_nodes(id);
            if !assigned.is_empty() {
                return assigned;
            }
        }
        self.children(id).map(Iterator::collect).unwrap_or_default()
    }

    /// Parent in the flattened tree used for style and box construction.
    #[must_use]
    pub fn rendered_parent(&self, id: NodeId) -> Option<NodeId> {
        if let Some(slot) = self.assigned_slot(id) {
            return Some(slot);
        }
        match self.parent(id) {
            Some(parent) => self.shadow_host(parent).or(Some(parent)),
            None => self.shadow_host(id),
        }
    }

    /// Adjacent sibling in the rendered, shadow-including tree.
    #[must_use]
    pub fn rendered_sibling(&self, id: NodeId, forward: bool) -> Option<NodeId> {
        let parent = self.rendered_parent(id)?;
        let children = self.rendered_children(parent);
        let position = children.iter().position(|&child| child == id)?;
        if forward {
            children.get(position + 1).copied()
        } else {
            position
                .checked_sub(1)
                .and_then(|index| children.get(index).copied())
        }
    }

    /// Descendants in pre-order through rendered shadow trees.
    #[must_use]
    pub fn rendered_descendants(&self, id: NodeId) -> Vec<NodeId> {
        let mut descendants = Vec::new();
        let mut stack = self.rendered_children(id);
        stack.reverse();
        while let Some(node) = stack.pop() {
            descendants.push(node);
            let mut children = self.rendered_children(node);
            children.reverse();
            stack.extend(children);
        }
        descendants
    }

    /// A stable identity token for `id`, for selector-engine caches.
    ///
    /// One live node owns exactly one slot, and matching runs under a shared
    /// borrow that freezes the arena, so the slot's address is unique per
    /// node and stable for the whole query. Detached nodes keep their slots,
    /// so identity survives detachment too.
    #[must_use]
    pub(crate) fn cache_identity(&self, id: NodeId) -> &Slot {
        self.tree
            .live_slot(id)
            .expect("selector cache identity requires a live handle")
    }

    /// Creates an element node, unattached until something appends it.
    ///
    /// Attribute names are unique in the DOM: `NamedNodeMap` is keyed by
    /// qualified name (<https://dom.spec.whatwg.org/#concept-attribute>,
    /// "an attribute list is essentially a map of names to attributes"),
    /// so later duplicates are dropped and the first occurrence wins,
    /// matching the merge rule the parser drives through
    /// [`Document::add_attrs_if_missing`]. Hand-built callers get the same
    /// normalization instead of an unrepresentable state.
    ///
    /// # Panics
    ///
    /// Only if the arena exceeds `u32::MAX` slots: terabytes of RAM, not a
    /// reachable runtime condition; the bound guards the handle width.
    pub fn create_element(&mut self, name: QualName, attributes: Vec<Attribute>) -> NodeId {
        let mut unique: Vec<Attribute> = Vec::with_capacity(attributes.len());
        merge_attrs(&mut unique, attributes);
        self.alloc(NodeKind::Element {
            name,
            attributes: unique,
        })
    }

    /// Creates a text node holding `data`.
    ///
    /// # Panics
    ///
    /// See [`Document::create_element`]: unreachable except beyond `u32::MAX` nodes.
    pub fn create_text(&mut self, data: impl Into<String>) -> NodeId {
        self.alloc(NodeKind::Text { data: data.into() })
    }

    /// Creates a comment node holding `data`.
    ///
    /// # Panics
    ///
    /// See [`Document::create_element`]: unreachable except beyond `u32::MAX` nodes.
    pub fn create_comment(&mut self, data: impl Into<String>) -> NodeId {
        self.alloc(NodeKind::Comment { data: data.into() })
    }

    /// Creates a CDATA section holding `data`.
    ///
    /// # Panics
    ///
    /// See [`Document::create_element`]: unreachable except beyond `u32::MAX` nodes.
    pub fn create_cdata_section(&mut self, data: impl Into<String>) -> NodeId {
        self.alloc(NodeKind::CDataSection { data: data.into() })
    }

    /// Creates a processing instruction with `target` and `data`.
    ///
    /// # Panics
    ///
    /// See [`Document::create_element`]: unreachable except beyond `u32::MAX` nodes.
    pub fn create_processing_instruction(
        &mut self,
        target: impl Into<String>,
        data: impl Into<String>,
    ) -> NodeId {
        self.alloc(NodeKind::ProcessingInstruction {
            target: target.into(),
            data: data.into(),
        })
    }

    /// Creates a doctype node.
    ///
    /// # Panics
    ///
    /// See [`Document::create_element`]: unreachable except beyond `u32::MAX` nodes.
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
    /// Fragments are containers outside the main tree: the contents root of
    /// `<template>` elements (associated with [`Document::set_template_contents`]) and
    /// the context node for `innerHTML`-style fragment parsing.
    ///
    /// # Panics
    ///
    /// See [`Document::create_element`]: unreachable except beyond `u32::MAX` nodes.
    pub fn create_fragment(&mut self) -> NodeId {
        self.alloc(NodeKind::Fragment)
    }

    /// [Clones](https://dom.spec.whatwg.org/#concept-node-clone) `id` into a
    /// new unattached node. `subtree` copies descendants (and a template's
    /// contents fragment). The document node is refused: cloning a document
    /// is a different spec operation.
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if `id` is stale.
    /// - [`DomError::WrongNodeType`] if `id` is the document.
    pub fn clone_node(&mut self, id: NodeId, subtree: bool) -> Result<NodeId, DomError> {
        self.require_live(id)?;
        if id == self.tree.document() {
            return Err(DomError::WrongNodeType);
        }
        let copy = self.alloc(self.kind(id).ok_or(DomError::StaleNode)?.clone());
        let mut pending = vec![(id, copy)];
        while let Some((source, target)) = pending.pop() {
            self.form.clone_dirty_value(source, target);
            // https://html.spec.whatwg.org/multipage/scripting.html#the-template-element:cloning-steps
            if let Some(contents) = self.template_contents(source) {
                let cloned_contents = self.create_fragment();
                self.set_template_contents(target, cloned_contents)?;
                if subtree {
                    pending.push((contents, cloned_contents));
                }
            }
            if subtree {
                let kids: Vec<NodeId> = self.children(source).ok_or(DomError::StaleNode)?.collect();
                for kid in kids {
                    let child = self.alloc(self.kind(kid).ok_or(DomError::StaleNode)?.clone());
                    self.append(target, child)?;
                    pending.push((kid, child));
                }
            }
        }
        Ok(copy)
    }

    /// Appends `child` as the last child of `parent`.
    ///
    /// Moving semantics: a `child` already attached elsewhere is detached
    /// first, mirroring DOM `appendChild`.
    ///
    /// # Panics
    ///
    /// Only on an internal invariant defect (a verified-live node missing its
    /// child list), never on user input; input failures return errors.
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if either handle is stale.
    /// - [`DomError::HierarchyRequest`] if the content model forbids it:
    ///   `child` is the document root, `parent` is a leaf kind, a document
    ///   would gain a second element child or a misplaced doctype, or a
    ///   doctype is placed outside a document.
    /// - [`DomError::CycleForbidden`] if `child` is an ancestor of `parent`.
    pub fn append(&mut self, parent: NodeId, child: NodeId) -> Result<(), DomError> {
        self.ensure_alive(parent, child)?;
        if child == self.tree.document() {
            // The root must never gain a parent; that is how a document
            // gets orphaned from itself. (Maps to HierarchyRequestError.)
            return Err(DomError::HierarchyRequest);
        }
        self.ensure_pre_insert_validity(parent, child, None)?;
        let tracked = lifecycle::snapshot(self, child);
        if self.is_fragment(child) {
            self.splice_fragment(parent, child, None);
        } else {
            self.place_node(parent, child, None);
        }
        lifecycle::record_snapshot(self, tracked);
        Ok(())
    }

    /// Inserts `node` immediately before `sibling` under sibling's parent.
    ///
    /// Moving semantics, like [`Document::append`]. Inserting a node beside
    /// itself is a legal stay-put no-op (WHATWG DOM's *ensure pre-insert
    /// validity* returns without doing anything in that case), not an
    /// error.
    ///
    /// # Panics
    ///
    /// Only on an internal invariant defect (a live sibling missing from its
    /// own parent's list), never on user input.
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if either handle is stale.
    /// - [`DomError::NoParent`] if `sibling` has no parent to insert beside
    ///   (`NotFoundError`, including the detached-sibling case).
    /// - [`DomError::HierarchyRequest`] / [`DomError::CycleForbidden`] as
    ///   from `Document::ensure_pre_insert_validity`.
    pub fn insert_before(&mut self, sibling: NodeId, node: NodeId) -> Result<(), DomError> {
        self.ensure_alive(sibling, node)?;
        // The reference child must sit under some parent to be inserted
        // beside; a detached one has none: the outer pre-insert
        // algorithm's parent-null refusal (`NotFoundError`).
        let parent = self.parent(sibling).ok_or(DomError::NoParent)?;
        if node == self.tree.document() {
            return Err(DomError::HierarchyRequest);
        }
        // Node-beside-itself means "stay put": the gate would reject this
        // as a cycle (a node contains itself), but the spec's answer is a
        // silent return, so it short-circuits before validation.
        if sibling == node {
            return Ok(());
        }
        self.ensure_pre_insert_validity(parent, node, Some(sibling))?;
        let tracked = lifecycle::snapshot(self, node);
        if self.is_fragment(node) {
            self.splice_fragment(parent, node, Some(sibling));
        } else {
            self.place_node(parent, node, Some(sibling));
        }
        lifecycle::record_snapshot(self, tracked);
        Ok(())
    }

    /// [Replaces](https://dom.spec.whatwg.org/#concept-node-replace) `child`
    /// with `node` inside `parent`. `child` stays alive (detached) after the
    /// call; the caller returns it.
    ///
    /// Validation runs against the child sequence *without* `child`
    /// (`childrenToExclude` in the spec's ensure-pre-insert-validity), so a
    /// refused replacement leaves the tree untouched.
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if any handle is stale.
    /// - [`DomError::NoParent`] if `child` is not a child of `parent`
    ///   (`NotFoundError`).
    /// - [`DomError::HierarchyRequest`] / [`DomError::CycleForbidden`] as
    ///   from `Document::ensure_pre_insert_validity`.
    pub fn replace_child(
        &mut self,
        parent: NodeId,
        node: NodeId,
        child: NodeId,
    ) -> Result<(), DomError> {
        self.require_live(parent)?;
        self.require_live(node)?;
        self.require_live(child)?;
        if !self.can_contain_children(parent) {
            return Err(DomError::HierarchyRequest);
        }
        if self.would_cycle(node, parent) {
            return Err(DomError::CycleForbidden);
        }
        if self.parent(child) != Some(parent) {
            return Err(DomError::NoParent);
        }
        // Same insertability gate as pre-insert
        // (<https://dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity>).
        if !self.is_insertable(node) {
            return Err(DomError::HierarchyRequest);
        }
        if !self.is_document(parent) && !self.is_fragment(node) && self.is_doctype(node) {
            return Err(DomError::HierarchyRequest);
        }
        if self.is_document(parent) {
            let incoming = self.incoming_nodes(node);
            let mut sequence: Vec<NodeId> = Vec::new();
            for existing in self.children(parent).into_iter().flatten() {
                if existing == child {
                    sequence.extend_from_slice(&incoming);
                } else {
                    sequence.push(existing);
                }
            }
            self.ensure_document_content_model(&sequence)?;
        }
        let previous = self.sibling(child, false);
        let mut reference = self.sibling(child, true);
        if reference == Some(node) {
            reference = self.sibling(node, true);
        }
        let added = self.incoming_nodes(node);
        // Adopt `node` first: removing it from its old parent stays
        // observable even though the replacement suppresses observers
        // (<https://dom.spec.whatwg.org/#concept-node-adopt>). Its connection
        // transitions coalesce, like every other move.
        self.record_unlink(node);
        // Remove `child` and insert `node` with observers suppressed
        // (<https://dom.spec.whatwg.org/#concept-node-replace>). The removal
        // still runs the removing steps, so iframe connection transitions
        // fire: a replaced iframe's frame must not survive.
        //
        // Both snapshots are taken before any removal: `node` can sit inside
        // `child`'s subtree, and detaching `child` first would read the wrong
        // starting state. Both are recorded after the whole operation, so a
        // disconnected-then-reinserted iframe reports no transition at all,
        // and the removal is recorded before the insertion so a replacement
        // frees its frame slot before the new frame is capped.
        let node_tracked = lifecycle::snapshot(self, node);
        let child_tracked = (child != node && self.parent(child) == Some(parent))
            .then(|| lifecycle::snapshot(self, child));
        let mut removed = Vec::new();
        self.journal.suppress(true);
        if child != node && self.parent(child) == Some(parent) {
            self.unlink_from_current_parent(child);
            removed.push(child);
        }
        if self.is_fragment(node) {
            self.splice_fragment(parent, node, reference);
        } else {
            self.place_node(parent, node, reference);
        }
        self.journal.suppress(false);
        if let Some(child_tracked) = child_tracked {
            lifecycle::record_snapshot(self, child_tracked);
        }
        lifecycle::record_snapshot(self, node_tracked);
        self.record(Mutation::ChildList {
            target: parent,
            added,
            removed,
            previous,
            next: reference,
        });
        Ok(())
    }

    /// Pre-insert validation steps 1–3 only: parent type, ancestor cycle,
    /// and reference membership
    /// (<https://dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity>).
    ///
    /// # Errors
    ///
    /// - [`DomError::HierarchyRequest`] when `parent` cannot contain children.
    /// - [`DomError::CycleForbidden`] if `node` is an ancestor of `parent`.
    /// - [`DomError::NoParent`] if `reference` is not a child of `parent`.
    pub fn validate_pre_insert(
        &self,
        parent: NodeId,
        node: NodeId,
        reference: Option<NodeId>,
    ) -> Result<(), DomError> {
        if !self.can_contain_children(parent) {
            return Err(DomError::HierarchyRequest);
        }
        if self.would_cycle(node, parent) {
            return Err(DomError::CycleForbidden);
        }
        if let Some(reference) = reference
            && self.parent(reference) != Some(parent)
        {
            return Err(DomError::NoParent);
        }
        Ok(())
    }

    /// [Pre-inserts](https://dom.spec.whatwg.org/#concept-node-pre-insert)
    /// `node` into `parent` before null or `reference`, validating in spec
    /// order and returning the node that was inserted.
    ///
    /// `append`/`insert_before` derive the parent from an existing sibling and
    /// serve trusted parser flows; this is the public web-visible entry point
    /// whose validation order (parent type, ancestor cycle, reference
    /// membership) tests observe.
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if any handle is stale.
    /// - [`DomError::HierarchyRequest`] if `parent` cannot contain children
    ///   or the content model refuses `node`.
    /// - [`DomError::CycleForbidden`] if `node` is an ancestor of `parent`.
    /// - [`DomError::NoParent`] if `reference` is not a child of `parent`.
    pub fn pre_insert(
        &mut self,
        parent: NodeId,
        node: NodeId,
        reference: Option<NodeId>,
    ) -> Result<(), DomError> {
        self.ensure_alive(parent, node)?;
        if !self.can_contain_children(parent) {
            return Err(DomError::HierarchyRequest);
        }
        if self.would_cycle(node, parent) {
            return Err(DomError::CycleForbidden);
        }
        if let Some(reference) = reference {
            self.require_live(reference)?;
            if self.parent(reference) != Some(parent) {
                return Err(DomError::NoParent);
            }
        }
        self.ensure_pre_insert_validity(parent, node, reference)?;
        // The reference child may be the node itself: inserting a node beside
        // itself is a legal stay-put no-op.
        let reference = if reference == Some(node) {
            self.sibling(node, true)
        } else {
            reference
        };
        let tracked = lifecycle::snapshot(self, node);
        if self.is_fragment(node) {
            self.splice_fragment(parent, node, reference);
        } else {
            self.place_node(parent, node, reference);
        }
        lifecycle::record_snapshot(self, tracked);
        Ok(())
    }

    /// WHATWG DOM's *ensure pre-insert validity*: the one gate every
    /// insertion path walks (`append`, `insert_before`), mirroring
    /// <https://dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity>.
    /// Engine reference: Firefox's `EnsureAllowedAsChild` in
    /// `dom/base/nsINode.cpp` (mozilla-firefox/firefox).
    ///
    /// Comments map each refusal to its rule in the spec's current wording
    /// (the algorithm has been restructured before; anchors, not step
    /// numbers, are what stay true). Refusals here are always
    /// [`DomError::HierarchyRequest`] or [`DomError::CycleForbidden`]; the
    /// document content model itself is encoded exactly once, in
    /// [`Document::ensure_document_content_model`].
    fn ensure_pre_insert_validity(
        &self,
        parent: NodeId,
        node: NodeId,
        reference: Option<NodeId>,
    ) -> Result<(), DomError> {
        // Container kinds only. Leaves with a child list would be arena corruption.
        // Kind stays borrowed: this gate is on the parse hot path.
        if !self.can_contain_children(parent) {
            return Err(DomError::HierarchyRequest);
        }
        // Only DocumentFragment, DocumentType, Element, and CharacterData
        // nodes are insertable; a Document node is refused here
        // (<https://dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity> step 4).
        if !self.is_insertable(node) {
            return Err(DomError::HierarchyRequest);
        }
        // Reference-child membership (the reference belongs to `parent`) is
        // enforced by construction: the only caller that passes `Some`
        // derives `parent` from that same handle's own parent pointer.
        //
        // Cycle rule: `node` must not be an inclusive ancestor of `parent`;
        // inserting a subtree into itself would tear the arena's acyclicity.
        if self.would_cycle(node, parent) {
            return Err(DomError::CycleForbidden);
        }
        // `insert_before` handles node-beside-itself (spec: do nothing)
        // before this gate.
        //
        // Content-model rules. Outside a document, a doctype as the
        // inserted node is never welcome; a fragment's children are not
        // checked here (the spec returns after the doctype-on-node
        // test: <https://dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity>).
        if !self.is_document(parent) {
            if !self.is_fragment(node) && self.is_doctype(node) {
                return Err(DomError::HierarchyRequest);
            }
            return Ok(());
        }
        // Inside a document: splice the incoming nodes (the node itself,
        // or a fragment's children —
        // <https://dom.spec.whatwg.org/#concept-node-insert>) into the
        // standing children at the insertion point and hand the whole
        // resulting sequence to the content model.
        let incoming = self.incoming_nodes(node);
        let mut sequence: Vec<NodeId> = Vec::new();
        let mut inserted = false;
        if let Some(kids) = self.children(parent) {
            for existing in kids {
                if !inserted && Some(existing) == reference {
                    sequence.extend_from_slice(&incoming);
                    inserted = true;
                }
                sequence.push(existing);
            }
        }
        if !inserted {
            sequence.extend_from_slice(&incoming);
        }
        self.ensure_document_content_model(&sequence)
    }

    /// Unlinks `id` from its parent, keeping the whole subtree alive.
    ///
    /// Idempotent: detaching an already-detached node succeeds. The document
    /// root cannot be detached.
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if `id` is stale.
    /// - [`DomError::HierarchyRequest`] for the document root.
    pub fn detach(&mut self, id: NodeId) -> Result<(), DomError> {
        self.require_live(id)?;
        if id == self.tree.document() {
            return Err(DomError::HierarchyRequest);
        }
        let tracked = lifecycle::snapshot(self, id);
        self.unlink_from_current_parent(id);
        lifecycle::record_snapshot(self, tracked);
        Ok(())
    }

    /// Moves every child of `from` to the end of `to`'s child list.
    ///
    /// This is the bulk-move primitive behind foster parenting and the
    /// adoption agency: order is preserved and each moved child's parent
    /// pointer is updated. Endpoints must be container kinds, and draining
    /// the document root is refused. For non-document destinations the
    /// shape of the moved run stays unvalidated: the html5ever tree
    /// builder's trusted internal flows never emit content-model
    /// violations, and gated insertion paths make smuggling
    /// impossible (a doctype can only ever sit directly under the root,
    /// so none can appear in a moved run). When `to` **is** the document,
    /// the full document content model applies to the *resulting* sequence;
    /// see `Document::ensure_document_content_model`. A bulk move is one
    /// operation: `[html, main]` into an empty document would pass
    /// per-child and fail as a pair.
    ///
    /// # Panics
    ///
    /// Only on an internal invariant defect (a verified-live node missing its
    /// slot), never on user input.
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if either handle is stale.
    /// - [`DomError::HierarchyRequest`] if `from` is the document root (the
    ///   move could then never complete without stranding the document),
    ///   if either endpoint is not a container kind, or if landing the run
    ///   in a document would break its content model.
    /// - [`DomError::CycleForbidden`] if `to` sits inside `from`'s subtree.
    pub fn reparent_children(&mut self, from: NodeId, to: NodeId) -> Result<(), DomError> {
        self.ensure_alive(from, to)?;
        if from == to {
            return Ok(());
        }
        // Both endpoints accept children, or the move would strand nodes under leaves.
        for endpoint in [from, to] {
            if !self.can_contain_children(endpoint) {
                return Err(DomError::HierarchyRequest);
            }
        }
        if from == self.tree.document() {
            // Draining the root would strand the entire document inside an
            // arbitrary detached subtree. (Maps to HierarchyRequestError.)
            return Err(DomError::HierarchyRequest);
        }
        if self.would_cycle(from, to) {
            return Err(DomError::CycleForbidden);
        }
        if self.is_document(to) {
            // The model sees the whole *resulting sequence*: the document's
            // standing children followed by the moved run.
            let mut sequence: Vec<NodeId> = Vec::new();
            if let Some(kids) = self.children(to) {
                sequence.extend(kids);
            }
            if let Some(kids) = self.children(from) {
                sequence.extend(kids);
            }
            self.ensure_document_content_model(&sequence)?;
        }
        let moved: Vec<NodeId> = self
            .children(from)
            .expect("verified-live `from` has a node")
            .collect();
        if moved.is_empty() {
            return Ok(());
        }
        // Defect guards, not input errors: both handles were verified live
        // above, so a miss here means the parent-pointer/sibling-link duality
        // is broken. Panicking beats reporting a lying "stale node".
        let from_connected = lifecycle::is_connected(self, from);
        let to_connected = lifecycle::is_connected(self, to);
        let tracked: Vec<(NodeId, bool, bool)> = if from_connected == to_connected {
            Vec::new()
        } else {
            moved
                .iter()
                .flat_map(|&id| lifecycle::snapshot(self, id))
                .collect()
        };
        // Move the run one node at a time through the single-node primitives:
        // each `unlink`/`insert_linked` step is O(1), so the move stays O(k)
        // and the link invariant has exactly two writers.
        for &id in &moved {
            self.unlink(id);
        }
        for &id in &moved {
            self.tree.insert_linked(to, id, None);
            if !from_connected && to_connected {
                named::inserted(self, id);
            }
        }
        lifecycle::record_snapshot(self, tracked);
        Ok(())
    }

    /// The document content model over one candidate child sequence. No
    /// character data anywhere, at most one element child, at most one
    /// doctype placed strictly ahead of that element; comments may sit
    /// anywhere, and fragments stay opaque containers. Deliberately the *only* encoding of the model:
    /// incremental insertions arrive as their resulting sequence from
    /// `Document::ensure_pre_insert_validity`, bulk moves as the document's
    /// standing children followed by the moved run. Per-child checks cannot
    /// see a violating pair like `[html, main]`.
    fn ensure_document_content_model(&self, sequence: &[NodeId]) -> Result<(), DomError> {
        let mut element_seen = false;
        let mut doctype_seen = false;
        for &id in sequence {
            match self.kind(id) {
                Some(NodeKind::Element { .. }) => {
                    if element_seen {
                        return Err(DomError::HierarchyRequest);
                    }
                    element_seen = true;
                }
                Some(NodeKind::Doctype { .. }) => {
                    if doctype_seen || element_seen {
                        return Err(DomError::HierarchyRequest);
                    }
                    doctype_seen = true;
                }
                Some(NodeKind::Text { .. }) => return Err(DomError::HierarchyRequest),
                _ => {}
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
        let (_, attributes) = self.element_mut(id)?;
        let added_name = attrs
            .iter()
            .find(|attribute| {
                attribute.name.ns.as_ref().is_empty()
                    && matches!(attribute.name.local.as_ref(), "id" | "name")
                    && !attributes
                        .iter()
                        .any(|existing| existing.name == attribute.name)
            })
            .map(|attribute| attribute.name.local.to_string());
        merge_attrs(attributes, attrs);
        if let Some(name) = added_name {
            named::attribute_changed(self, id, &name);
        }
        Ok(())
    }

    /// The value of the attribute whose qualified name is `local` on element
    /// `id`.
    ///
    /// [DOM getAttribute](https://dom.spec.whatwg.org/#dom-element-getattribute)
    /// after HTML’s ASCII-lowercase name conversion.
    #[must_use]
    pub fn attribute(&self, id: NodeId, local: &str) -> Option<String> {
        self.find_attribute(id, local)
            .map(|attribute| attribute.value.clone())
    }

    /// Whether `id` is an HTML element with local name `local` (exact match).
    pub(crate) fn html_local_is(&self, id: NodeId, local: &str) -> bool {
        self.element(id)
            .is_some_and(|(name, _)| name.ns == html_namespace() && name.local.as_ref() == local)
    }

    /// The [child text content](https://dom.spec.whatwg.org/#concept-child-text-content)
    /// of node `id`: the concatenated data of its `Text` children.
    ///
    /// `CDATASection` is a `Text` node ([DOM](https://dom.spec.whatwg.org/#interface-cdatasection)),
    /// so its data is included. A stale handle has no children, so this
    /// yields the empty string.
    #[must_use]
    pub fn child_text_content(&self, id: NodeId) -> String {
        let mut text = String::new();
        if let Some(children) = self.children(id) {
            for child in children {
                if let Some(NodeKind::Text { data } | NodeKind::CDataSection { data }) =
                    self.kind(child)
                {
                    text.push_str(data);
                }
            }
        }
        text
    }

    /// The text content of `id`: every descendant text node's data.
    #[must_use]
    pub fn text_content(&self, id: NodeId) -> String {
        let mut text = String::new();
        for node in self.tree.descendants(id) {
            if let Some(NodeKind::Text { data } | NodeKind::CDataSection { data }) = self.kind(node)
            {
                text.push_str(data);
            }
        }
        text
    }

    /// An attribute in no namespace with the given local name, as HTML
    /// `getAttribute` matches
    /// (<https://dom.spec.whatwg.org/#concept-element-attributes-get-by-name>).
    #[must_use]
    pub fn no_namespace_attribute(&self, id: NodeId, local: &str) -> Option<String> {
        self.tree
            .no_namespace_attribute(id, local)
            .map(str::to_owned)
    }

    /// [Element.hasAttribute](https://dom.spec.whatwg.org/#dom-element-hasattribute):
    /// exact local-name match; HTML elements lowercase the queried name.
    #[must_use]
    pub fn has_attribute(&self, id: NodeId, local: &str) -> bool {
        self.find_attribute(id, local).is_some()
    }

    /// [Element.getAttributeNS](https://dom.spec.whatwg.org/#dom-element-getattributens):
    /// exact namespace and local-name match, prefix ignored.
    #[must_use]
    pub fn attribute_ns(&self, id: NodeId, ns: &str, local: &str) -> Option<String> {
        self.element(id)?.1.iter().find_map(|attribute| {
            (attribute.name.ns.as_ref() == ns && attribute.name.local.as_ref() == local)
                .then(|| attribute.value.clone())
        })
    }

    /// [Element.getAttributeNames](https://dom.spec.whatwg.org/#dom-element-getattributenames):
    /// qualified names in attribute order.
    #[must_use]
    pub fn attribute_names(&self, id: NodeId) -> Vec<String> {
        self.element(id)
            .map(|(_, attributes)| {
                attributes
                    .iter()
                    .map(|attribute| Self::serialize_qualified_name(&attribute.name))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The element's attribute list, or `None` when `id` is stale or not an
    /// element. Used by the `NamedNodeMap` platform object.
    #[must_use]
    pub fn attributes(&self, id: NodeId) -> Option<&[Attribute]> {
        self.element(id).map(|(_, attributes)| attributes)
    }

    /// [Element.removeAttribute](https://dom.spec.whatwg.org/#dom-element-removeattribute).
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if `id` is stale.
    /// - [`DomError::WrongNodeType`] if `id` is not an element.
    pub fn remove_attribute(&mut self, id: NodeId, local: &str) -> Result<(), DomError> {
        let (name, attributes) = self.element_mut(id)?;
        let Some(index) = attributes
            .iter()
            .position(|attribute| Self::attr_query_eq(&name.ns, &attribute.name, local))
        else {
            return Ok(());
        };
        let removed = attributes.remove(index);
        // `MutationRecord.attributeName` is the attribute's local
        // name and `attributeNamespace` its namespace, not the
        // queried qualified name
        // (<https://dom.spec.whatwg.org/#dom-mutationrecord-attributename>).
        self.record(Mutation::Attributes {
            target: id,
            name: removed.name.local.to_string(),
            namespace: removed.name.ns.to_string(),
            old_value: Some(removed.value),
        });
        named::attribute_changed(self, id, local);
        form::attribute_removed(self, id, local)
    }

    /// [Element.removeAttributeNS](https://dom.spec.whatwg.org/#dom-element-removeattributens).
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if `id` is stale.
    /// - [`DomError::WrongNodeType`] if `id` is not an element.
    pub fn remove_attribute_ns(
        &mut self,
        id: NodeId,
        ns: &str,
        local: &str,
    ) -> Result<(), DomError> {
        let (_, attributes) = self.element_mut(id)?;
        let Some(index) = attributes.iter().position(|attribute| {
            attribute.name.ns.as_ref() == ns && attribute.name.local.as_ref() == local
        }) else {
            return Ok(());
        };
        let removed = attributes.remove(index);
        self.record(Mutation::Attributes {
            target: id,
            name: local.to_owned(),
            namespace: ns.to_owned(),
            old_value: Some(removed.value),
        });
        named::attribute_changed(self, id, local);
        Ok(())
    }

    /// Replaces every child of `parent` with `node`
    /// (<https://dom.spec.whatwg.org/#concept-node-replace-all>). Removed
    /// children stay alive, detached, like the spec's remove step.
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if either handle is stale.
    /// - [`DomError::HierarchyRequest`] if `parent` cannot contain children,
    ///   `node` is a doctype outside a document, or the document content
    ///   model refuses the replacement.
    /// - [`DomError::CycleForbidden`] if `node` is an ancestor of `parent`.
    ///
    /// # Panics
    ///
    /// Only on an internal invariant defect (a verified-live node missing its
    /// slot), never on user input.
    pub fn replace_all(&mut self, parent: NodeId, node: NodeId) -> Result<(), DomError> {
        self.ensure_alive(parent, node)?;
        if !self.can_contain_children(parent) {
            return Err(DomError::HierarchyRequest);
        }
        if self.would_cycle(node, parent) {
            return Err(DomError::CycleForbidden);
        }
        if self.is_document(parent) {
            let incoming = self.incoming_nodes(node);
            self.ensure_document_content_model(&incoming)?;
        } else if !self.is_fragment(node) && self.is_doctype(node) {
            return Err(DomError::HierarchyRequest);
        }
        let parent_connected = lifecycle::is_connected(self, parent);
        let removed: Vec<NodeId> = self
            .children(parent)
            .map(Iterator::collect)
            .unwrap_or_default();
        let removed_snapshot: Vec<(NodeId, bool, bool)> = if parent_connected {
            removed
                .iter()
                .flat_map(|&id| lifecycle::snapshot(self, id))
                .collect()
        } else {
            Vec::new()
        };
        let added = self.incoming_nodes(node);
        // `node` can be one of the removed children (JS `replaceChildren(
        // firstChild)`), so its starting connectivity must be read before the
        // detach loop below.
        let node_tracked = lifecycle::snapshot(self, node);
        self.journal.suppress(true);
        // Detach the standing children through the single-node primitive; its
        // O(1) steps make the loop O(k), and it keeps `unlink` the only
        // remover of a node. Recording is suppressed, so no observer sees
        // these individual removals.
        for &kid in &removed {
            self.unlink_from_current_parent(kid);
        }
        if self.is_fragment(node) {
            self.splice_fragment(parent, node, None);
        } else {
            self.place_node(parent, node, None);
        }
        self.journal.suppress(false);
        // Removal before insertion: a replacement frees its frame slot before
        // the new frame is checked against the frame cap.
        lifecycle::record_snapshot(self, removed_snapshot);
        lifecycle::record_snapshot(self, node_tracked);
        // "If either addedNodes or removedNodes is not empty, then queue a
        // tree mutation record" (<https://dom.spec.whatwg.org/#concept-node-replace-all>):
        // e.g. `textContent = ""` on an already-empty element changes nothing
        // and is silent.
        if !added.is_empty() || !removed.is_empty() {
            self.record(Mutation::ChildList {
                target: parent,
                added,
                removed,
                previous: None,
                next: None,
            });
        }
        Ok(())
    }

    /// Sets an attribute identified by namespace and local name, replacing
    /// the first attribute with that namespace and local name (the existing
    /// prefix is kept, matching "set an attribute value").
    ///
    /// [DOM setAttributeNS](https://dom.spec.whatwg.org/#dom-element-setattributens)
    /// and `setAttributeNode` land here.
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if `id` is stale.
    /// - [`DomError::WrongNodeType`] if `id` is not an element.
    pub fn set_attribute_by_ns(
        &mut self,
        id: NodeId,
        namespace: &str,
        prefix: Option<&str>,
        local: &str,
        value: impl Into<String>,
    ) -> Result<(), DomError> {
        let value = value.into();
        let (_, attributes) = self.element_mut(id)?;
        let index = attributes.iter().position(|attribute| {
            attribute.name.ns.as_ref() == namespace && attribute.name.local.as_ref() == local
        });
        let old_value = if let Some(index) = index {
            Some(std::mem::replace(&mut attributes[index].value, value))
        } else {
            attributes.push(Attribute {
                name: QualName::new(
                    prefix.map(Prefix::from),
                    Namespace::from(namespace),
                    LocalName::from(local),
                ),
                value,
            });
            None
        };
        self.record(Mutation::Attributes {
            target: id,
            name: local.to_owned(),
            namespace: namespace.to_owned(),
            old_value,
        });
        named::attribute_changed(self, id, local);
        Ok(())
    }

    /// Sets the unnamespaced attribute `local` on element `id`, replacing a
    /// same-name attribute if one exists.
    ///
    /// [DOM setAttribute](https://dom.spec.whatwg.org/#dom-element-setattribute)
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if `id` is stale.
    /// - [`DomError::WrongNodeType`] if `id` is not an element.
    pub fn set_attribute(
        &mut self,
        id: NodeId,
        local: &str,
        value: impl Into<String>,
    ) -> Result<(), DomError> {
        let value = value.into();
        let (name, attributes) = self.element_mut(id)?;
        let html = name.ns == html_namespace();
        let existing = attributes
            .iter()
            .position(|attribute| Self::attr_query_eq(&name.ns, &attribute.name, local));
        // A matched attribute can carry a namespace even though the query is
        // unnamespaced (`setAttribute("xlink:href", …)` on an SVG element);
        // the record reports the changed attribute's real name and namespace
        // (<https://dom.spec.whatwg.org/#dom-mutationrecord-attributename>).
        let (recorded_name, recorded_namespace, old_value) = if let Some(index) = existing {
            let attribute = &mut attributes[index];
            let old_value = std::mem::replace(&mut attribute.value, value);
            (
                attribute.name.local.to_string(),
                attribute.name.ns.to_string(),
                Some(old_value),
            )
        } else {
            let local = if html {
                local.to_ascii_lowercase()
            } else {
                local.to_owned()
            };
            attributes.push(Attribute {
                name: QualName::new(None, Namespace::from(""), LocalName::from(local.as_str())),
                value,
            });
            (local, String::new(), None)
        };
        self.record(Mutation::Attributes {
            target: id,
            name: recorded_name,
            namespace: recorded_namespace,
            old_value,
        });
        named::attribute_changed(self, id, local);
        form::attribute_set(self, id, local)
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

    /// Appends `extra` to the text node `id`.
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if `id` is stale.
    /// - [`DomError::WrongNodeType`] if `id` is not a text node.
    pub fn append_text(&mut self, id: NodeId, extra: &str) -> Result<(), DomError> {
        let parent = self.parent(id);
        let value_before = parent.and_then(|parent| self.textarea_value_before_change(parent));
        let recording = self.journal.recording();
        let old_value = {
            let kind = self.tree.kind_mut(id).ok_or(DomError::StaleNode)?;
            let NodeKind::Text { data } = kind else {
                return Err(DomError::WrongNodeType);
            };
            let old_value = recording.then(|| data.clone());
            data.push_str(extra);
            old_value
        };
        if let Some(old_value) = old_value {
            self.record(Mutation::CharacterData {
                target: id,
                old_value,
            });
        }
        if let Some(parent) = parent {
            self.reset_textarea_selection_if_changed(parent, value_before);
        }
        Ok(())
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

    /// Replaces the data of the CDATA section `id`.
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if `id` is stale.
    /// - [`DomError::WrongNodeType`] if `id` is not a CDATA section.
    pub fn set_cdata_section(
        &mut self,
        id: NodeId,
        data: impl Into<String>,
    ) -> Result<(), DomError> {
        self.set_data(
            id,
            |kind| match kind {
                NodeKind::CDataSection { data } => Some(data),
                _ => None,
            },
            data.into(),
        )
    }

    /// Replaces the data of the processing instruction `id`.
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if `id` is stale.
    /// - [`DomError::WrongNodeType`] if `id` is not a processing instruction.
    pub fn set_processing_instruction(
        &mut self,
        id: NodeId,
        data: impl Into<String>,
    ) -> Result<(), DomError> {
        self.set_data(
            id,
            |kind| match kind {
                NodeKind::ProcessingInstruction { data, .. } => Some(data),
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
    /// A connected subtree's `iframe` connection transitions are recorded,
    /// even though the nodes are gone by the time they are reported.
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if `id` is stale.
    /// - [`DomError::HierarchyRequest`] for the document root.
    pub fn destroy(&mut self, id: NodeId) -> Result<(), DomError> {
        self.require_live(id)?;
        if id == self.tree.document() {
            return Err(DomError::HierarchyRequest);
        }
        // Read connectivity before unlinking; the snapshot carries the
        // iframe-ness so the count still moves once the slots are freed.
        let tracked = lifecycle::snapshot(self, id);
        self.unlink_from_current_parent(id);

        let mut pending = vec![id];
        while let Some(current) = pending.pop() {
            self.form.forget(current);
            self.metadata.forget(current);
            self.shadow.forget(current, &mut pending);
            self.tree.retire(current, &mut pending);
        }
        lifecycle::record_snapshot(self, tracked);
        Ok(())
    }

    /// Whether `id` is one of the container kinds that may hold children.
    fn can_contain_children(&self, id: NodeId) -> bool {
        matches!(
            self.kind(id),
            Some(NodeKind::Document | NodeKind::Element { .. } | NodeKind::Fragment)
        )
    }

    /// The kinds DOM's *ensure pre-insert validity* accepts as an inserted
    /// node: everything but a document
    /// (<https://dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity>).
    fn is_insertable(&self, id: NodeId) -> bool {
        matches!(
            self.kind(id),
            Some(
                NodeKind::Fragment
                    | NodeKind::Doctype { .. }
                    | NodeKind::Element { .. }
                    | NodeKind::Text { .. }
                    | NodeKind::CDataSection { .. }
                    | NodeKind::ProcessingInstruction { .. }
                    | NodeKind::Comment { .. }
            )
        )
    }

    fn is_document(&self, id: NodeId) -> bool {
        matches!(self.kind(id), Some(NodeKind::Document))
    }

    fn is_doctype(&self, id: NodeId) -> bool {
        matches!(self.kind(id), Some(NodeKind::Doctype { .. }))
    }

    fn is_fragment(&self, id: NodeId) -> bool {
        matches!(self.kind(id), Some(NodeKind::Fragment))
    }

    /// The element's qualified name and attribute list, or `None` when `id`
    /// is stale or not an element.
    pub(crate) fn element(&self, id: NodeId) -> Option<(&QualName, &[Attribute])> {
        match self.kind(id)? {
            NodeKind::Element { name, attributes } => Some((name, attributes)),
            _ => None,
        }
    }

    /// Mutable variant of [`Document::element`] for the attribute mutators.
    fn element_mut(&mut self, id: NodeId) -> Result<(&QualName, &mut Vec<Attribute>), DomError> {
        match self.tree.kind_mut(id).ok_or(DomError::StaleNode)? {
            NodeKind::Element { name, attributes } => Ok((name, attributes)),
            _ => Err(DomError::WrongNodeType),
        }
    }

    /// A qualified name's serialization: `prefix:local` or just `local`.
    fn serialize_qualified_name(name: &QualName) -> String {
        match &name.prefix {
            Some(prefix) if !prefix.is_empty() => format!("{prefix}:{}", name.local),
            _ => name.local.to_string(),
        }
    }

    fn attr_query_eq(element_ns: &Namespace, name: &QualName, query: &str) -> bool {
        if *element_ns == html_namespace() {
            html_qualified_name_eq(name, query)
        } else {
            qualified_name_eq(name, query)
        }
    }

    /// The first attribute on `id` whose **qualified name** matches `local`
    /// under the element's case regime; HTML elements ASCII-lowercase the
    /// queried name first
    /// ([DOM get-an-attribute-by-name](https://dom.spec.whatwg.org/#concept-element-attributes-get-by-name)).
    fn find_attribute<'a>(&'a self, id: NodeId, local: &str) -> Option<&'a Attribute> {
        let (name, attributes) = self.element(id)?;
        attributes
            .iter()
            .find(|attribute| Self::attr_query_eq(&name.ns, &attribute.name, local))
    }

    fn is_html_template_element(&self, id: NodeId) -> bool {
        match self.kind(id) {
            Some(NodeKind::Element { name, .. }) => {
                name.ns == html_namespace() && name.local.as_ref().eq_ignore_ascii_case("template")
            }
            _ => false,
        }
    }

    /// Nodes the insert algorithm actually places: a fragment's children,
    /// otherwise the node itself
    /// (<https://dom.spec.whatwg.org/#concept-node-insert>).
    fn incoming_nodes(&self, node: NodeId) -> Vec<NodeId> {
        if self.is_fragment(node) {
            self.children(node)
                .map(Iterator::collect)
                .unwrap_or_default()
        } else {
            vec![node]
        }
    }

    /// Places a non-fragment `node` under `parent` before `before` (or at
    /// the end when `before` is `None`).
    ///
    /// Structural only: lifecycle recording belongs to the calling operation,
    /// which captures one snapshot before it starts and records it once the
    /// whole operation is done. Recording here would both misread a node the
    /// caller already detached and pin the event order relative to a removal.
    fn place_node(&mut self, parent: NodeId, node: NodeId, before: Option<NodeId>) {
        let value_before = self.textarea_value_before_change(parent);
        self.unlink_from_current_parent(node);
        // A checked radio whose form owner changes on insertion unchecks its
        // new group
        // (<https://html.spec.whatwg.org/multipage/input.html#radio-button-group>).
        let radio_owner_before = self.checked_radio_form_owner(node);
        // Sibling references for the mutation record, read from the run the
        // node is about to join. Computed only while recording: no observer
        // exists on the parse path, so the lookups stay off it.
        let (previous, next) = if self.journal.recording() {
            let previous = match before {
                Some(before) => self.previous_sibling(before),
                None => self.last_child(parent),
            };
            (previous, before)
        } else {
            (None, None)
        };
        self.tree.insert_linked(parent, node, before);
        named::inserted(self, node);
        self.record(Mutation::ChildList {
            target: parent,
            added: vec![node],
            removed: Vec::new(),
            previous,
            next,
        });
        self.reset_textarea_selection_if_changed(parent, value_before);
        if radio_owner_before != self.checked_radio_form_owner(node) {
            self.refresh_radio_group(node);
        }
        // An option joining a select follows the option insertion steps: a
        // selected option clears the others, then the selectedness setting
        // algorithm supplies the default when nothing is selected. Only a
        // select whose list of options actually gained the node runs it.
        if self.html_local_is(node, "option") {
            self.option_added_to_select(node);
        }
        if let Some(select) = self.inserted_list_owner(node) {
            self.apply_default_selectedness(select);
        }
    }

    /// Insert a fragment by moving its children under `parent`, leaving the
    /// fragment empty and unparented
    /// (<https://dom.spec.whatwg.org/#concept-node-insert>).
    ///
    /// Structural only; the caller records the fragment subtree's lifecycle
    /// snapshot (see [`Document::place_node`]).
    fn splice_fragment(&mut self, parent: NodeId, fragment: NodeId, before: Option<NodeId>) {
        let moved: Vec<NodeId> = self
            .children(fragment)
            .map(Iterator::collect)
            .unwrap_or_default();
        self.unlink_from_current_parent(fragment);
        if moved.is_empty() {
            return;
        }
        // Appending a run of unselected options yields the same selection as
        // applying the setting algorithm after every insertion. No script can
        // observe the intermediate state during the fragment splice
        // (<https://html.spec.whatwg.org/multipage/form-elements.html#selectedness-setting-algorithm>).
        let blank_options = self.html_local_is(parent, "select")
            && moved
                .iter()
                .all(|&id| self.html_local_is(id, "option") && !self.option_selected(id));
        self.record(Mutation::ChildList {
            target: fragment,
            added: Vec::new(),
            removed: moved.clone(),
            previous: None,
            next: None,
        });
        // Sibling references for the mutation record, read before the run
        // leaves the fragment.
        let (previous, next) = if self.journal.recording() {
            let previous = match before {
                Some(before) => self.previous_sibling(before),
                None => self.last_child(parent),
            };
            (previous, before)
        } else {
            (None, None)
        };
        // Move the run one node at a time. Inserting each immediately before
        // the same reference preserves order, and O(1) steps keep the splice
        // O(k) with `insert_linked` as the only insert writer.
        for &id in &moved {
            self.unlink(id);
        }
        // Insert each node and run its group rule immediately, so a radio or
        // option sees the run in insertion order: radios moved out of a
        // fragment into a form join a group, and options joining a select may
        // become the default selection.
        for &id in &moved {
            self.tree.insert_linked(parent, id, before);
            named::inserted(self, id);
            if self.checked_radio_form_owner(id).is_some() {
                self.refresh_radio_group(id);
            }
            if !blank_options {
                if self.html_local_is(id, "option") {
                    self.option_added_to_select(id);
                }
                if let Some(select) = self.inserted_list_owner(id) {
                    self.apply_default_selectedness(select);
                }
            }
        }
        if blank_options {
            self.apply_default_selectedness(parent);
        }
        self.record(Mutation::ChildList {
            target: parent,
            added: moved,
            removed: Vec::new(),
            previous,
            next,
        });
    }

    /// Appends a run of fresh, unparented leaves to a live container as one
    /// child-list mutation. The caller performs element-specific insertion
    /// steps; only the link writer here touches parent and sibling pointers
    /// (<https://dom.spec.whatwg.org/#concept-node-insert>).
    pub(crate) fn append_fresh_children(&mut self, parent: NodeId, added: Vec<NodeId>) {
        let previous = self.last_child(parent);
        for &node in &added {
            self.tree.insert_linked(parent, node, None);
            named::inserted(self, node);
        }
        self.record(Mutation::ChildList {
            target: parent,
            added,
            removed: Vec::new(),
            previous,
            next: None,
        });
    }

    fn ensure_alive(&self, a: NodeId, b: NodeId) -> Result<(), DomError> {
        self.require_live(a)?;
        self.require_live(b)
    }

    /// Single-handle variant of [`Document::ensure_alive`].
    fn require_live(&self, a: NodeId) -> Result<(), DomError> {
        if self.contains(a) {
            Ok(())
        } else {
            Err(DomError::StaleNode)
        }
    }

    /// True iff placing `subtree` under `into` would nest it inside itself.
    fn would_cycle(&self, subtree: NodeId, into: NodeId) -> bool {
        // https://dom.spec.whatwg.org/#concept-tree-host-including-inclusive-ancestor
        let mut cursor = Some(into);
        while let Some(id) = cursor {
            if id == subtree {
                return true;
            }
            cursor = self.parent(id).or_else(|| {
                if self.is_fragment(id) {
                    self.shadow.associated_host(id)
                } else {
                    None
                }
            });
        }
        false
    }

    /// Queues the removal record for `id` from its current parent, without
    /// touching the tree.
    ///
    /// Shared by [`Document::unlink_from_current_parent`] and the replace
    /// algorithm's adopt step, whose removal is observable even though the
    /// rest of the replacement suppresses observers
    /// (<https://dom.spec.whatwg.org/#concept-node-adopt>). The caller has
    /// verified `id` live; a missing parent is a silent no-op (the node is
    /// already detached).
    fn record_unlink(&mut self, id: NodeId) {
        if !self.journal.recording() {
            return;
        }
        let Some(parent) = self.parent(id) else {
            return;
        };
        self.record(Mutation::ChildList {
            target: parent,
            added: Vec::new(),
            removed: vec![id],
            previous: self.previous_sibling(id),
            next: self.next_sibling(id),
        });
    }

    /// Removes `id` from whichever parent currently holds it.
    fn unlink_from_current_parent(&mut self, id: NodeId) {
        if self.parent(id).is_none() {
            return;
        }
        self.record_unlink(id);
        self.unlink(id);
    }

    /// Splices `id` out of its parent's child run and clears its parent and
    /// sibling links. A no-op when `id` is unparented.
    ///
    /// Defect policy, like every other structural site in this module: `id`
    /// was verified live, so a `Some` parent must name it and the link
    /// fields must agree. Parent-pointer/sibling-link divergence is arena
    /// corruption; panicking beats silently producing a node with two
    /// parents (or none), which later mutations would compound.
    fn unlink(&mut self, id: NodeId) {
        let Some(parent) = self.parent(id) else {
            return;
        };
        let select = self.inserted_list_owner(id);
        let value_before = self.textarea_value_before_change(parent);
        self.tree.unlink_linked(id);
        self.reset_textarea_selection_if_changed(parent, value_before);
        if let Some(select) = select {
            self.apply_default_selectedness(select);
        }
    }

    fn set_data(
        &mut self,
        id: NodeId,
        extract: impl Fn(&mut NodeKind) -> Option<&mut String>,
        data: String,
    ) -> Result<(), DomError> {
        let parent = self.parent(id);
        let value_before = parent.and_then(|parent| self.textarea_value_before_change(parent));
        let kind = self.tree.kind_mut(id).ok_or(DomError::StaleNode)?;
        match extract(kind) {
            Some(field) => {
                let old_value = std::mem::replace(field, data);
                self.record(Mutation::CharacterData {
                    target: id,
                    old_value,
                });
                if let Some(parent) = parent {
                    self.reset_textarea_selection_if_changed(parent, value_before);
                }
                Ok(())
            }
            None => Err(DomError::WrongNodeType),
        }
    }

    fn alloc(&mut self, kind: NodeKind) -> NodeId {
        self.tree.alloc(kind)
    }
}

/// Adds each attribute whose qualified name is not already present; the
/// first occurrence wins, matching the DOM's name-keyed attribute list
/// (<https://dom.spec.whatwg.org/#concept-attribute>).
fn merge_attrs(attributes: &mut Vec<Attribute>, attrs: impl IntoIterator<Item = Attribute>) {
    for attr in attrs {
        if !attributes.iter().any(|existing| existing.name == attr.name) {
            attributes.push(attr);
        }
    }
}
