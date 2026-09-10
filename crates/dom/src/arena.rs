//! The arena: flat slot array, generational handles, tree mutations.

use std::cell::Cell;
use std::collections::HashMap;
use std::fmt;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::id::NodeId;
use crate::node::{
    Attribute, LocalName, Namespace, Node, NodeKind, Prefix, QualName, html_namespace,
};

/// Next document id for a freshly constructed [`Dom`]. Relaxed arithmetic is
/// enough: the only requirement is that two live `Dom` values do not share
/// an id until the counter wraps (2^32 documents, accepted like generation
/// wrap).
static NEXT_DOCUMENT_ID: AtomicU32 = AtomicU32::new(0);

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
    /// `HierarchyRequestError`; see [`Dom::ensure_pre_insert_validity`].)
    HierarchyRequest,
    /// The operation does not apply to that kind of node (setting text data
    /// on an element, attributes on a text node). (Maps to a type error at
    /// the binding layer.)
    WrongNodeType,
    /// An insert was requested beside a node with no parent to sit under.
    /// (Maps to `NotFoundError`.)
    NoParent,
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
/// are harmless (lookups report absence), which is what will let the `QuickJS`
/// binding layer hold handles across garbage-collection cycles without
/// borrowing anything.
///
/// [`Send`] but deliberately not [`Sync`]: a `Dom` may be handed between
/// workers, but two threads can never touch one simultaneously (one worker
/// per document; see the dom-layer wayfinding tickets). The marker field below is
/// what suppresses the otherwise-auto-derived `Sync`.
#[derive(Debug)]
pub struct Dom {
    slots: Vec<Slot>,
    free: Vec<u32>,
    document: NodeId,
    document_id: u32,
    quirks_mode: QuirksMode,
    /// HTTP `Content-Language` (and later `document` language) default for
    /// `:lang()` when no `lang` / `xml:lang` is on the ancestor chain.
    document_language: Option<String>,
    /// `<template>` element → its contents fragment. Contents live outside
    /// the element's child list
    /// (<https://html.spec.whatwg.org/multipage/scripting.html#the-template-element>).
    template_contents: HashMap<NodeId, NodeId>,
    /// Recorded mutations, drained by the renderer's `MutationObserver`
    /// plumbing; empty and unrecorded unless someone observes the document.
    mutations: Vec<Mutation>,
    record_mutations: bool,
    recording_suppressed: bool,
    /// `Cell<()>` is `Send` + `!Sync`; `PhantomData` makes `Dom` inherit
    /// exactly that split. Deleting this field would silently re-derive
    /// `Sync`, which is the point: that deletion has to be a conscious act.
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
        let document_id = NEXT_DOCUMENT_ID.fetch_add(1, Ordering::Relaxed);
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
            document: NodeId::new(document_id, 0, 0),
            document_id,
            quirks_mode: QuirksMode::NoQuirks,
            document_language: None,
            template_contents: HashMap::new(),
            mutations: Vec::new(),
            record_mutations: false,
            recording_suppressed: false,
            _share_forbidden: PhantomData,
        }
    }

    /// Turns mutation recording on or off; recording costs nothing while no
    /// `MutationObserver` is registered.
    pub fn set_record_mutations(&mut self, recording: bool) {
        self.record_mutations = recording;
        if !recording {
            self.mutations.clear();
        }
    }

    /// Drains the recorded mutations in order.
    pub fn take_mutations(&mut self) -> Vec<Mutation> {
        std::mem::take(&mut self.mutations)
    }

    fn record(&mut self, mutation: Mutation) {
        if self.record_mutations && !self.recording_suppressed {
            self.mutations.push(mutation);
        }
    }

    /// Compatibility mode this document answers selector queries under.
    #[must_use]
    pub fn quirks_mode(&self) -> QuirksMode {
        self.quirks_mode
    }

    /// Sets the compatibility mode. The html5ever adapter writes this from
    /// the tree builder; tests may set it to exercise the id/class quirk.
    pub fn set_quirks_mode(&mut self, mode: QuirksMode) {
        self.quirks_mode = mode;
    }

    /// Document-level language from HTTP `Content-Language`, used by
    /// `:lang()` when no element `lang` / `xml:lang` applies.
    #[must_use]
    pub fn document_language(&self) -> Option<&str> {
        self.document_language.as_deref()
    }

    /// Sets the document language default. `browser` writes this after
    /// navigation; tests may set it directly.
    pub fn set_document_language(&mut self, language: Option<String>) {
        self.document_language = language;
    }

    /// The root `Document` node; every other node descends from it.
    #[must_use]
    pub fn document(&self) -> NodeId {
        self.document
    }

    /// This document's arena id, for per-document renderer lookups.
    #[must_use]
    pub fn document_id(&self) -> u32 {
        self.document_id
    }

    /// Whether `id` names a currently live node.
    ///
    /// A destroyed node's handle fails here even though a different node may
    /// later occupy the same slot; that is the whole point of generations.
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
    /// Mirrors DOM `childNodes`: every live node answers a list; leaves
    /// (text, comment, doctype) answer an empty one because the mutation
    /// gate below refuses to give them children. Childless and dead remain
    /// different answers; only staleness is `None`.
    #[must_use]
    pub fn children(&self, id: NodeId) -> Option<std::slice::Iter<'_, NodeId>> {
        self.live_slot(id)
            .and_then(|slot| slot.node.as_ref())
            .map(|node| node.children.iter())
    }

    /// The sibling of `id` adjacent in the given direction, or `None`.
    #[must_use]
    pub fn sibling(&self, id: NodeId, forward: bool) -> Option<NodeId> {
        let parent = self.parent(id)?;
        let kids: Vec<NodeId> = self.children(parent)?.copied().collect();
        let position = kids.iter().position(|&kid| kid == id)?;
        if forward {
            kids.get(position + 1).copied()
        } else {
            position
                .checked_sub(1)
                .and_then(|before| kids.get(before))
                .copied()
        }
    }

    /// A stable identity token for `id`, for selector-engine caches.
    ///
    /// One live node owns exactly one slot, and matching runs under a shared
    /// borrow that freezes the arena, so the slot's address is unique per
    /// node and stable for the whole query. Detached nodes keep their slots,
    /// so identity survives detachment too.
    #[must_use]
    pub(crate) fn cache_identity(&self, id: NodeId) -> &Slot {
        self.live_slot(id)
            .expect("selector cache identity requires a live handle")
    }

    /// Creates an element node, unattached until something appends it.
    ///
    /// Attribute names are unique in the DOM: `NamedNodeMap` is keyed by
    /// qualified name (<https://dom.spec.whatwg.org/#concept-attribute>,
    /// "an attribute list is essentially a map of names to attributes"),
    /// so later duplicates are dropped and the first occurrence wins,
    /// matching the merge rule the parser drives through
    /// [`Dom::add_attrs_if_missing`]. Hand-built callers get the same
    /// normalization instead of an unrepresentable state.
    ///
    /// # Panics
    ///
    /// Only if the arena exceeds `u32::MAX` slots: terabytes of RAM, not a
    /// reachable runtime condition; the bound guards the handle width.
    pub fn create_element(&mut self, name: QualName, attributes: Vec<Attribute>) -> NodeId {
        let mut unique: Vec<Attribute> = Vec::with_capacity(attributes.len());
        for attribute in attributes {
            if !unique.iter().any(|kept| kept.name == attribute.name) {
                unique.push(attribute);
            }
        }
        self.alloc(NodeKind::Element {
            name,
            attributes: unique,
        })
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

    /// Creates a CDATA section holding `data`.
    ///
    /// # Panics
    ///
    /// See [`Dom::create_element`]: unreachable except beyond `u32::MAX` nodes.
    pub fn create_cdata_section(&mut self, data: impl Into<String>) -> NodeId {
        self.alloc(NodeKind::CDataSection { data: data.into() })
    }

    /// Creates a processing instruction with `target` and `data`.
    ///
    /// # Panics
    ///
    /// See [`Dom::create_element`]: unreachable except beyond `u32::MAX` nodes.
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
    /// Fragments are containers outside the main tree: the contents root of
    /// `<template>` elements (associated with [`Dom::set_template_contents`]) and,
    /// later, the context node for `innerHTML`-style fragment parsing.
    ///
    /// # Panics
    ///
    /// See [`Dom::create_element`]: unreachable except beyond `u32::MAX` nodes.
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
        if id == self.document {
            return Err(DomError::WrongNodeType);
        }
        let copy = self.alloc(self.get(id).ok_or(DomError::StaleNode)?.kind().clone());
        let mut pending = vec![(id, copy)];
        while let Some((source, target)) = pending.pop() {
            // https://html.spec.whatwg.org/multipage/scripting.html#the-template-element:cloning-steps
            if let Some(contents) = self.template_contents(source) {
                let cloned_contents = self.create_fragment();
                self.set_template_contents(target, cloned_contents)?;
                if subtree {
                    pending.push((contents, cloned_contents));
                }
            }
            if subtree {
                let kids: Vec<NodeId> = self
                    .children(source)
                    .ok_or(DomError::StaleNode)?
                    .copied()
                    .collect();
                for kid in kids {
                    let child =
                        self.alloc(self.get(kid).ok_or(DomError::StaleNode)?.kind().clone());
                    self.append(target, child)?;
                    pending.push((kid, child));
                }
            }
        }
        Ok(copy)
    }

    /// Associates `contents` as the [template contents](https://html.spec.whatwg.org/multipage/scripting.html#template-contents)
    /// of `template`. The fragment stays out of `template`'s child list.
    ///
    /// Replacing an existing association destroys the previous fragment.
    ///
    /// # Errors
    ///
    /// - [`DomError::CycleForbidden`] if the association creates a host-including cycle.
    /// - [`DomError::HierarchyRequest`] if replacement would destroy the new contents.
    /// - [`DomError::StaleNode`] if either handle is stale.
    /// - [`DomError::WrongNodeType`] if `template` is not an HTML `template`
    ///   element, `contents` is not a fragment, or `contents` already belongs
    ///   to another template.
    pub fn set_template_contents(
        &mut self,
        template: NodeId,
        contents: NodeId,
    ) -> Result<(), DomError> {
        self.require_live(template)?;
        self.require_live(contents)?;
        if !self.is_html_template_element(template) {
            return Err(DomError::WrongNodeType);
        }
        if !self.is_fragment(contents) {
            return Err(DomError::WrongNodeType);
        }
        if self
            .template_contents
            .iter()
            .any(|(&owner, &mapped)| mapped == contents && owner != template)
        {
            return Err(DomError::WrongNodeType);
        }
        // https://dom.spec.whatwg.org/#concept-tree-host-including-inclusive-ancestor
        if self.would_cycle(contents, template) {
            return Err(DomError::CycleForbidden);
        }
        if let Some(old) = self.template_contents.get(&template).copied()
            && old != contents
        {
            if self.would_cycle(old, contents) {
                return Err(DomError::HierarchyRequest);
            }
            self.destroy(old)?;
            self.template_contents.remove(&template);
        }
        self.template_contents.insert(template, contents);
        Ok(())
    }

    /// The contents fragment of `template`, if this document associated one.
    #[must_use]
    pub fn template_contents(&self, template: NodeId) -> Option<NodeId> {
        let contents = self.template_contents.get(&template).copied()?;
        self.contains(contents).then_some(contents)
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
        if child == self.document {
            // The root must never gain a parent; that is how a document
            // gets orphaned from itself. (Maps to HierarchyRequestError.)
            return Err(DomError::HierarchyRequest);
        }
        self.ensure_pre_insert_validity(parent, child, None)?;
        if self.is_fragment(child) {
            self.splice_fragment(parent, child, None);
            return Ok(());
        }
        self.place_node(parent, child, None);
        Ok(())
    }

    /// Inserts `node` immediately before `sibling` under sibling's parent.
    ///
    /// Moving semantics, like [`Dom::append`]. Inserting a node beside
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
    ///   from [`Dom::ensure_pre_insert_validity`].
    pub fn insert_before(&mut self, sibling: NodeId, node: NodeId) -> Result<(), DomError> {
        self.ensure_alive(sibling, node)?;
        // The reference child must sit under some parent to be inserted
        // beside; a detached one has none: the outer pre-insert
        // algorithm's parent-null refusal (`NotFoundError`).
        let parent = self.parent(sibling).ok_or(DomError::NoParent)?;
        if node == self.document {
            return Err(DomError::HierarchyRequest);
        }
        // Node-beside-itself means "stay put": the gate would reject this
        // as a cycle (a node contains itself), but the spec's answer is a
        // silent return, so it short-circuits before validation.
        if sibling == node {
            return Ok(());
        }
        self.ensure_pre_insert_validity(parent, node, Some(sibling))?;
        if self.is_fragment(node) {
            self.splice_fragment(parent, node, Some(sibling));
            return Ok(());
        }
        self.place_node(parent, node, Some(sibling));
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
    ///   from [`Dom::ensure_pre_insert_validity`].
    pub fn replace_child(
        &mut self,
        parent: NodeId,
        node: NodeId,
        child: NodeId,
    ) -> Result<(), DomError> {
        self.require_live(parent)?;
        self.require_live(node)?;
        self.require_live(child)?;
        if !matches!(
            self.get(parent).map(|view| view.kind()),
            Some(NodeKind::Document | NodeKind::Element { .. } | NodeKind::Fragment)
        ) {
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
        if !matches!(
            self.get(node).map(|view| view.kind()),
            Some(
                NodeKind::Fragment
                    | NodeKind::Doctype { .. }
                    | NodeKind::Element { .. }
                    | NodeKind::Text { .. }
                    | NodeKind::CDataSection { .. }
                    | NodeKind::ProcessingInstruction { .. }
                    | NodeKind::Comment { .. }
            )
        ) {
            return Err(DomError::HierarchyRequest);
        }
        if !matches!(
            self.get(parent).map(|view| view.kind()),
            Some(NodeKind::Document)
        ) && !self.is_fragment(node)
            && matches!(
                self.get(node).map(|view| view.kind()),
                Some(NodeKind::Doctype { .. })
            )
        {
            return Err(DomError::HierarchyRequest);
        }
        if matches!(
            self.get(parent).map(|view| view.kind()),
            Some(NodeKind::Document)
        ) {
            let incoming = self.incoming_nodes(node);
            let mut sequence: Vec<NodeId> = Vec::new();
            for &existing in self.children(parent).into_iter().flatten() {
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
        self.recording_suppressed = true;
        self.unlink_from_current_parent(child);
        if let Some(detached) = self.node_mut(child) {
            detached.parent = None;
        }
        if self.is_fragment(node) {
            self.splice_fragment(parent, node, reference);
        } else {
            self.place_node(parent, node, reference);
        }
        self.recording_suppressed = false;
        self.record(Mutation::ChildList {
            target: parent,
            added,
            removed: vec![child],
            previous,
            next: reference,
        });
        Ok(())
    }

    /// Pre-insert validation steps 1–3 only: parent type, ancestor cycle,
    /// and reference membership
    /// (<https://dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity>).
    ///
    /// The bindings run this before copying a cross-document node so the
    /// observable error order matches the spec.
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
        if !matches!(
            self.get(parent).map(|view| view.kind()),
            Some(NodeKind::Document | NodeKind::Element { .. } | NodeKind::Fragment)
        ) {
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
        if !matches!(
            self.get(parent).map(|view| view.kind()),
            Some(NodeKind::Document | NodeKind::Element { .. } | NodeKind::Fragment)
        ) {
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
        if self.is_fragment(node) {
            self.splice_fragment(parent, node, reference);
        } else {
            self.place_node(parent, node, reference);
        }
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
    /// [`Dom::ensure_document_content_model`].
    fn ensure_pre_insert_validity(
        &self,
        parent: NodeId,
        node: NodeId,
        reference: Option<NodeId>,
    ) -> Result<(), DomError> {
        // Container kinds only. Leaves with a child list would be arena corruption.
        // Kind stays borrowed: this gate is on the parse hot path.
        if !matches!(
            self.get(parent).map(|view| view.kind()),
            Some(NodeKind::Document | NodeKind::Element { .. } | NodeKind::Fragment)
        ) {
            return Err(DomError::HierarchyRequest);
        }
        // Only DocumentFragment, DocumentType, Element, and CharacterData
        // nodes are insertable; a Document node is refused here
        // (<https://dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity> step 4).
        if !matches!(
            self.get(node).map(|view| view.kind()),
            Some(
                NodeKind::Fragment
                    | NodeKind::Doctype { .. }
                    | NodeKind::Element { .. }
                    | NodeKind::Text { .. }
                    | NodeKind::CDataSection { .. }
                    | NodeKind::ProcessingInstruction { .. }
                    | NodeKind::Comment { .. }
            )
        ) {
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
        if !matches!(
            self.get(parent).map(|view| view.kind()),
            Some(NodeKind::Document)
        ) {
            if !self.is_fragment(node)
                && matches!(
                    self.get(node).map(|view| view.kind()),
                    Some(NodeKind::Doctype { .. })
                )
            {
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
                if !inserted && Some(*existing) == reference {
                    sequence.extend_from_slice(&incoming);
                    inserted = true;
                }
                sequence.push(*existing);
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
        if id == self.document {
            return Err(DomError::HierarchyRequest);
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
    /// pointer is updated. Endpoints must be container kinds, and draining
    /// the document root is refused. For non-document destinations the
    /// shape of the moved run stays unvalidated: the html5ever tree
    /// builder's trusted internal flows (ADR 0002) never emit
    /// content-model violations, and gated insertion paths make smuggling
    /// impossible (a doctype can only ever sit directly under the root,
    /// so none can appear in a moved run). When `to` **is** the document,
    /// the full document content model applies to the *resulting* sequence;
    /// see [`Dom::ensure_document_content_model`]. A bulk move is one
    /// operation: `[html, main]` into an empty document would pass
    /// per-child and fail as a pair.
    ///
    /// # Panics
    ///
    /// Only on an internal invariant defect (a verified-live node missing its
    /// child list), never on user input.
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
            if !matches!(
                self.get(endpoint).map(|view| view.kind()),
                Some(NodeKind::Document | NodeKind::Element { .. } | NodeKind::Fragment)
            ) {
                return Err(DomError::HierarchyRequest);
            }
        }
        if from == self.document {
            // Draining the root would strand the entire document inside an
            // arbitrary detached subtree. (Maps to HierarchyRequestError.)
            return Err(DomError::HierarchyRequest);
        }
        if self.would_cycle(from, to) {
            return Err(DomError::CycleForbidden);
        }
        if matches!(
            self.get(to).map(|view| view.kind()),
            Some(NodeKind::Document)
        ) {
            // The model sees the whole *resulting sequence*: the document's
            // standing children followed by the moved run.
            let mut sequence: Vec<NodeId> = Vec::new();
            if let Some(kids) = self.children(to) {
                sequence.extend(kids.copied());
            }
            if let Some(kids) = self.children(from) {
                sequence.extend(kids.copied());
            }
            self.ensure_document_content_model(&sequence)?;
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

    /// The document content model over one candidate child sequence. No
    /// character data anywhere, at most one element child, at most one
    /// doctype placed strictly ahead of that element; comments may sit
    /// anywhere, and fragments stay opaque containers. Deliberately the *only* encoding of the model:
    /// incremental insertions arrive as their resulting sequence from
    /// [`Dom::ensure_pre_insert_validity`], bulk moves as the document's
    /// standing children followed by the moved run. Per-child checks cannot
    /// see a violating pair like `[html, main]`.
    fn ensure_document_content_model(&self, sequence: &[NodeId]) -> Result<(), DomError> {
        let mut element_seen = false;
        let mut doctype_seen = false;
        for &id in sequence {
            match self.get(id).map(|view| view.kind()) {
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
        match self.get(id).map(|node| node.kind()) {
            Some(NodeKind::Element { attributes, .. }) => attributes.iter().find_map(|attribute| {
                (attribute.name.ns.as_ref() == ns && attribute.name.local.as_ref() == local)
                    .then(|| attribute.value.clone())
            }),
            _ => None,
        }
    }

    /// [Element.getAttributeNames](https://dom.spec.whatwg.org/#dom-element-getattributenames):
    /// qualified names in attribute order.
    #[must_use]
    pub fn attribute_names(&self, id: NodeId) -> Vec<String> {
        match self.get(id).map(|node| node.kind()) {
            Some(NodeKind::Element { attributes, .. }) => attributes
                .iter()
                .map(|attribute| Self::qualified_name(&attribute.name))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The element's attribute list, or `None` when `id` is stale or not an
    /// element. Used by the `NamedNodeMap` platform object.
    #[must_use]
    pub fn attributes(&self, id: NodeId) -> Option<&[Attribute]> {
        match self.get(id).map(|node| node.kind()) {
            Some(NodeKind::Element { attributes, .. }) => Some(attributes),
            _ => None,
        }
    }

    /// [Element.removeAttribute](https://dom.spec.whatwg.org/#dom-element-removeattribute).
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if `id` is stale.
    /// - [`DomError::WrongNodeType`] if `id` is not an element.
    pub fn remove_attribute(&mut self, id: NodeId, local: &str) -> Result<(), DomError> {
        let node = self.node_mut(id).ok_or(DomError::StaleNode)?;
        let NodeKind::Element { name, attributes } = &mut node.kind else {
            return Err(DomError::WrongNodeType);
        };
        let local = if name.ns == html_namespace() {
            local.to_ascii_lowercase()
        } else {
            local.to_owned()
        };
        let mut removed = false;
        let mut removed_value = None;
        attributes.retain(|attribute| {
            if !removed && Self::qualified_name(&attribute.name) == local {
                removed = true;
                removed_value = Some(attribute.value.clone());
                return false;
            }
            true
        });
        if removed {
            self.record(Mutation::Attributes {
                target: id,
                name: local,
                namespace: String::new(),
                old_value: removed_value,
            });
        }
        Ok(())
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
        let removed_value = {
            let node = self.node_mut(id).ok_or(DomError::StaleNode)?;
            let NodeKind::Element { attributes, .. } = &mut node.kind else {
                return Err(DomError::WrongNodeType);
            };
            let mut removed = None;
            attributes.retain(|attribute| {
                if attribute.name.ns.as_ref() == ns && attribute.name.local.as_ref() == local {
                    removed = Some(attribute.value.clone());
                    return false;
                }
                true
            });
            removed
        };
        if removed_value.is_some() {
            self.record(Mutation::Attributes {
                target: id,
                name: local.to_owned(),
                namespace: ns.to_owned(),
                old_value: removed_value,
            });
        }
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
    pub fn replace_all(&mut self, parent: NodeId, node: NodeId) -> Result<(), DomError> {
        self.ensure_alive(parent, node)?;
        if !matches!(
            self.get(parent).map(|view| view.kind()),
            Some(NodeKind::Document | NodeKind::Element { .. } | NodeKind::Fragment)
        ) {
            return Err(DomError::HierarchyRequest);
        }
        if self.would_cycle(node, parent) {
            return Err(DomError::CycleForbidden);
        }
        if matches!(
            self.get(parent).map(|view| view.kind()),
            Some(NodeKind::Document)
        ) {
            let incoming = self.incoming_nodes(node);
            self.ensure_document_content_model(&incoming)?;
        } else if !self.is_fragment(node)
            && matches!(
                self.get(node).map(|view| view.kind()),
                Some(NodeKind::Doctype { .. })
            )
        {
            return Err(DomError::HierarchyRequest);
        }
        let removed: Vec<NodeId> = self
            .children(parent)
            .map(|kids| kids.copied().collect())
            .unwrap_or_default();
        let added = self.incoming_nodes(node);
        self.recording_suppressed = true;
        for &kid in &removed {
            self.unlink_from_current_parent(kid);
            if let Some(detached) = self.node_mut(kid) {
                detached.parent = None;
            }
        }
        if self.is_fragment(node) {
            self.splice_fragment(parent, node, None);
        } else {
            self.place_node(parent, node, None);
        }
        self.recording_suppressed = false;
        self.record(Mutation::ChildList {
            target: parent,
            added,
            removed,
            previous: None,
            next: None,
        });
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
        let node = self.node_mut(id).ok_or(DomError::StaleNode)?;
        let NodeKind::Element { attributes, .. } = &mut node.kind else {
            return Err(DomError::WrongNodeType);
        };
        let value = value.into();
        let old_value = attributes
            .iter()
            .find(|attribute| {
                attribute.name.ns.as_ref() == namespace && attribute.name.local.as_ref() == local
            })
            .map(|attribute| attribute.value.clone());
        if let Some(existing) = attributes.iter_mut().find(|attribute| {
            attribute.name.ns.as_ref() == namespace && attribute.name.local.as_ref() == local
        }) {
            existing.value = value;
        } else {
            attributes.push(Attribute {
                name: QualName::new(
                    prefix.map(Prefix::from),
                    Namespace::from(namespace),
                    LocalName::from(local),
                ),
                value,
            });
        }
        self.record(Mutation::Attributes {
            target: id,
            name: local.to_owned(),
            namespace: namespace.to_owned(),
            old_value,
        });
        Ok(())
    }

    /// Sets the attribute `name` on element `id`, replacing an attribute with
    /// the same qualified name.
    ///
    /// [DOM setAttributeNS](https://dom.spec.whatwg.org/#dom-element-setattributens)
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if `id` is stale.
    /// - [`DomError::WrongNodeType`] if `id` is not an element.
    pub fn set_attribute_named(
        &mut self,
        id: NodeId,
        name: QualName,
        value: impl Into<String>,
    ) -> Result<(), DomError> {
        let node = self.node_mut(id).ok_or(DomError::StaleNode)?;
        let NodeKind::Element { attributes, .. } = &mut node.kind else {
            return Err(DomError::WrongNodeType);
        };
        let value = value.into();
        let recorded_name = Self::qualified_name(&name);
        let recorded_namespace = name.ns.to_string();
        let old_value = attributes
            .iter()
            .find(|attribute| attribute.name == name)
            .map(|attribute| attribute.value.clone());
        if let Some(existing) = attributes
            .iter_mut()
            .find(|attribute| attribute.name == name)
        {
            existing.value = value;
        } else {
            attributes.push(Attribute { name, value });
        }
        self.record(Mutation::Attributes {
            target: id,
            name: recorded_name,
            namespace: recorded_namespace,
            old_value,
        });
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
        let node = self.node_mut(id).ok_or(DomError::StaleNode)?;
        let NodeKind::Element { name, attributes } = &mut node.kind else {
            return Err(DomError::WrongNodeType);
        };
        let local = if name.ns == html_namespace() {
            local.to_ascii_lowercase()
        } else {
            local.to_owned()
        };
        let value = value.into();
        let old_value = attributes
            .iter()
            .find(|attribute| Self::qualified_name(&attribute.name) == local)
            .map(|attribute| attribute.value.clone());
        if let Some(existing) = attributes
            .iter_mut()
            .find(|attribute| Self::qualified_name(&attribute.name) == local)
        {
            existing.value = value;
        } else {
            attributes.push(Attribute {
                name: QualName::new(None, Namespace::from(""), LocalName::from(local.as_str())),
                value,
            });
        }
        self.record(Mutation::Attributes {
            target: id,
            name: local,
            namespace: String::new(),
            old_value,
        });
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
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if `id` is stale.
    /// - [`DomError::HierarchyRequest`] for the document root.
    pub fn destroy(&mut self, id: NodeId) -> Result<(), DomError> {
        self.require_live(id)?;
        if id == self.document {
            return Err(DomError::HierarchyRequest);
        }
        self.unlink_from_current_parent(id);

        let mut pending = vec![id];
        while let Some(current) = pending.pop() {
            if let Some(contents) = self.template_contents.remove(&current) {
                pending.push(contents);
            }
            self.template_contents
                .retain(|_, contents| *contents != current);
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
        if id.document != self.document_id {
            return None;
        }
        self.slots
            .get(id.index())
            .filter(|slot| slot.generation == id.generation && slot.node.is_some())
    }

    fn node_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        if id.document != self.document_id {
            return None;
        }
        self.slots
            .get_mut(id.index())
            .filter(|slot| slot.generation == id.generation)
            .and_then(|slot| slot.node.as_mut())
    }

    fn is_fragment(&self, id: NodeId) -> bool {
        matches!(
            self.get(id).map(|view| view.kind()),
            Some(NodeKind::Fragment)
        )
    }

    /// A qualified name's serialization: `prefix:local` or just `local`.
    fn qualified_name(name: &QualName) -> String {
        match &name.prefix {
            Some(prefix) if !prefix.is_empty() => format!("{prefix}:{}", name.local),
            _ => name.local.to_string(),
        }
    }

    /// The element's own unnamespaced attribute whose local name matches
    /// `local`; HTML elements ASCII-lowercase the queried name first
    /// ([DOM has-attribute](https://dom.spec.whatwg.org/#concept-element-attribute-has)).
    /// The first attribute on `id` whose **qualified name** is `local`; HTML
    /// elements ASCII-lowercase the queried name first
    /// ([DOM get-an-attribute-by-name](https://dom.spec.whatwg.org/#concept-element-attributes-get-by-name)).
    fn find_attribute<'a>(&'a self, id: NodeId, local: &str) -> Option<&'a Attribute> {
        let NodeKind::Element { name, attributes } = self.get(id)?.kind() else {
            return None;
        };
        let local = if name.ns == html_namespace() {
            local.to_ascii_lowercase()
        } else {
            local.to_owned()
        };
        attributes
            .iter()
            .find(|attribute| Self::qualified_name(&attribute.name) == local)
    }

    fn is_html_template_element(&self, id: NodeId) -> bool {
        match self.get(id).map(|view| view.kind()) {
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
                .map(|kids| kids.copied().collect())
                .unwrap_or_default()
        } else {
            vec![node]
        }
    }

    /// Places a non-fragment `node` under `parent` before `before` (or at
    /// the end when `before` is `None`).
    fn place_node(&mut self, parent: NodeId, node: NodeId, before: Option<NodeId>) {
        self.unlink_from_current_parent(node);
        let (previous, next) = {
            let list: Vec<NodeId> = self
                .children(parent)
                .expect("verified-live parent has no child list")
                .copied()
                .collect();
            match before {
                None => (list.last().copied(), None),
                Some(sibling) => {
                    let position = list
                        .iter()
                        .position(|&entry| entry == sibling)
                        .expect("live sibling missing from its own parent's list");
                    (
                        position
                            .checked_sub(1)
                            .and_then(|index| list.get(index))
                            .copied(),
                        Some(sibling),
                    )
                }
            }
        };
        let list = self
            .children_mut(parent)
            .expect("verified-live parent has no child list");
        match before {
            None => list.push(node),
            Some(sibling) => {
                let position = list
                    .iter()
                    .position(|&entry| entry == sibling)
                    .expect("live sibling missing from its own parent's list");
                list.insert(position, node);
            }
        }
        if let Some(attached) = self.node_mut(node) {
            attached.parent = Some(parent);
        }
        self.record(Mutation::ChildList {
            target: parent,
            added: vec![node],
            removed: Vec::new(),
            previous,
            next,
        });
    }

    /// Insert a fragment by moving its children under `parent`, leaving the
    /// fragment empty and unparented
    /// (<https://dom.spec.whatwg.org/#concept-node-insert>).
    fn splice_fragment(&mut self, parent: NodeId, fragment: NodeId, before: Option<NodeId>) {
        self.unlink_from_current_parent(fragment);
        if let Some(node) = self.node_mut(fragment) {
            node.parent = None;
        }
        let moved = self
            .children_mut(fragment)
            .map(std::mem::take)
            .expect("verified-live fragment has no child list");
        if !moved.is_empty() {
            self.record(Mutation::ChildList {
                target: fragment,
                added: Vec::new(),
                removed: moved.clone(),
                previous: None,
                next: None,
            });
        }
        if moved.is_empty() {
            return;
        }
        let list = self
            .children_mut(parent)
            .expect("verified-live parent has no child list");
        let (previous, next) = match before {
            None => (list.last().copied(), None),
            Some(sibling) => {
                let position = list
                    .iter()
                    .position(|&entry| entry == sibling)
                    .expect("live sibling missing from its own parent's list");
                (
                    position
                        .checked_sub(1)
                        .and_then(|index| list.get(index))
                        .copied(),
                    Some(sibling),
                )
            }
        };
        let position = match before {
            None => list.len(),
            Some(sibling) => list
                .iter()
                .position(|&entry| entry == sibling)
                .expect("live sibling missing from its own parent's list"),
        };
        for (offset, id) in moved.iter().enumerate() {
            list.insert(position + offset, *id);
        }
        for id in &moved {
            if let Some(node) = self.node_mut(*id) {
                node.parent = Some(parent);
            }
        }
        self.record(Mutation::ChildList {
            target: parent,
            added: moved,
            removed: Vec::new(),
            previous,
            next,
        });
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
        // https://dom.spec.whatwg.org/#concept-tree-host-including-inclusive-ancestor
        let mut cursor = Some(into);
        while let Some(id) = cursor {
            if id == subtree {
                return true;
            }
            cursor = self.parent(id).or_else(|| {
                if self.is_fragment(id) {
                    self.template_contents
                        .iter()
                        .find_map(|(&host, &contents)| (contents == id).then_some(host))
                } else {
                    None
                }
            });
        }
        false
    }

    /// Removes `id` from whichever list currently holds it.
    ///
    /// Defect policy, like every other structural site in this module: `id`
    /// was verified live, so a `Some` parent must own a list and that list
    /// must name `id`. Parent-pointer/child-list divergence is arena
    /// corruption; panicking beats silently producing a node with two
    /// parents (or none), which later mutations would compound.
    fn unlink_from_current_parent(&mut self, id: NodeId) {
        if let Some(old_parent) = self.parent(id) {
            let (previous, next) = {
                let list: Vec<NodeId> = self
                    .children(old_parent)
                    .expect("live parent has no child list")
                    .copied()
                    .collect();
                let position = list
                    .iter()
                    .position(|&entry| entry == id)
                    .expect("child missing from the very list its parent pointer names");
                (
                    position
                        .checked_sub(1)
                        .and_then(|index| list.get(index))
                        .copied(),
                    list.get(position + 1).copied(),
                )
            };
            self.record(Mutation::ChildList {
                target: old_parent,
                added: Vec::new(),
                removed: vec![id],
                previous,
                next,
            });
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
                let old_value = std::mem::replace(field, data);
                self.record(Mutation::CharacterData {
                    target: id,
                    old_value,
                });
                Ok(())
            }
            None => Err(DomError::WrongNodeType),
        }
    }

    /// Places a fresh node into a recycled or newly grown slot.
    ///
    /// # Panics
    ///
    /// Only when the arena would need more than `u32::MAX` slots: hundreds
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
            return NodeId::new(self.document_id, slot, generation);
        }
        // The bound is the `u32` handle width itself; exhausting it requires
        // >4 billion slots (hundreds of GB of arena), an impossible runtime
        // condition rather than an error to handle. Generations wrap after
        // 2^32 recycles of one slot; that residual ABA window is accepted by
        // design. Exploiting it needs billions of death/reuse cycles on a
        // single slot while some outside handle to that slot still exists.
        let slot = u32::try_from(self.slots.len())
            .expect("arena exhausted: >u32::MAX slots requires hundreds of GB of RAM");
        self.slots.push(Slot {
            generation: 0,
            node: Some(node),
        });
        NodeId::new(self.document_id, slot, 0)
    }
}
