//! The arena: flat slot array, generational handles, tree mutations.

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

use crate::shadow::{self, ShadowState};
pub use self::tree::Children;
use self::tree::Slot;
pub use self::tree::Tree;
use crate::lifecycle::{self, ConnectionState};
use crate::metadata::{self, ActiveElements, ScriptLines, ScrollOffsets, Settings};
use crate::mutation::{self, Mutation, MutationJournal};



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
    /// `HierarchyRequestError`; see `mutation::insert::ensure_pre_insert_validity`.)
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
    pub(crate) settings: Settings,
    pub(crate) script_lines: ScriptLines,
    pub(crate) scroll_offsets: ScrollOffsets,
    pub(crate) active_elements: ActiveElements,
    pub(crate) shadow: ShadowState,
    pub(crate) form: FormState,
    pub(crate) journal: MutationJournal,
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
            settings: Settings::default(),
            script_lines: ScriptLines::default(),
            scroll_offsets: ScrollOffsets::default(),
            active_elements: ActiveElements::default(),
            shadow: ShadowState::default(),
            form: FormState::default(),
            journal: MutationJournal::default(),
            connections: ConnectionState::default(),
            named: NamedIndex::default(),
            _share_forbidden: PhantomData,
        }
    }

    /// `id`'s ancestors, nearest first, `id` excluded.
    pub fn ancestors(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        std::iter::successors(self.parent(id), |&node| self.parent(node))
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
        if let Some(root) = shadow::shadow_root(self, id) {
            return self.rendered_children(root);
        }
        if shadow::is_html_slot(self, id) {
            let assigned = shadow::assigned_nodes(self, id);
            if !assigned.is_empty() {
                return assigned;
            }
        }
        self.children(id).map(Iterator::collect).unwrap_or_default()
    }

    /// Parent in the flattened tree used for style and box construction.
    #[must_use]
    pub fn rendered_parent(&self, id: NodeId) -> Option<NodeId> {
        if let Some(slot) = shadow::assigned_slot(self, id) {
            return Some(slot);
        }
        match self.parent(id) {
            Some(parent) => shadow::shadow_host(self, parent).or(Some(parent)),
            None => shadow::shadow_host(self, id),
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
    /// `<template>` elements (associated with [`crate::shadow::set_template_contents`]) and
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
            if let Some(contents) = shadow::template_contents(self, source) {
                let cloned_contents = self.create_fragment();
                shadow::set_template_contents(self, target, cloned_contents)?;
                if subtree {
                    pending.push((contents, cloned_contents));
                }
            }
            if subtree {
                let kids: Vec<NodeId> = self.children(source).ok_or(DomError::StaleNode)?.collect();
                for kid in kids {
                    let child = self.alloc(self.kind(kid).ok_or(DomError::StaleNode)?.clone());
                    crate::mutation::append(self, target, child)?;
                    pending.push((kid, child));
                }
            }
        }
        Ok(copy)
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
        mutation::record(
            self,
            Mutation::Attributes {
                target: id,
                name: removed.name.local.to_string(),
                namespace: removed.name.ns.to_string(),
                old_value: Some(removed.value),
            },
        );
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
        mutation::record(
            self,
            Mutation::Attributes {
                target: id,
                name: local.to_owned(),
                namespace: ns.to_owned(),
                old_value: Some(removed.value),
            },
        );
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
            crate::mutation::ensure_document_content_model(self, &incoming)?;
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
        mutation::suppress(self);
        // Detach the standing children through the single-node primitive; its
        // O(1) steps make the loop O(k), and it keeps `unlink` the only
        // remover of a node. Recording is suppressed, so no observer sees
        // these individual removals.
        for &kid in &removed {
            crate::mutation::unlink_from_current_parent(self, kid);
        }
        if self.is_fragment(node) {
            crate::mutation::splice_fragment(self, parent, node, None);
        } else {
            crate::mutation::place_node(self, parent, node, None);
        }
        mutation::resume(self);
        // Removal before insertion: a replacement frees its frame slot before
        // the new frame is checked against the frame cap.
        lifecycle::record_snapshot(self, removed_snapshot);
        lifecycle::record_snapshot(self, node_tracked);
        // "If either addedNodes or removedNodes is not empty, then queue a
        // tree mutation record" (<https://dom.spec.whatwg.org/#concept-node-replace-all>):
        // e.g. `textContent = ""` on an already-empty element changes nothing
        // and is silent.
        if !added.is_empty() || !removed.is_empty() {
            mutation::record(
                self,
                Mutation::ChildList {
                    target: parent,
                    added,
                    removed,
                    previous: None,
                    next: None,
                },
            );
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
        mutation::record(
            self,
            Mutation::Attributes {
                target: id,
                name: local.to_owned(),
                namespace: namespace.to_owned(),
                old_value,
            },
        );
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
        mutation::record(
            self,
            Mutation::Attributes {
                target: id,
                name: recorded_name,
                namespace: recorded_namespace,
                old_value,
            },
        );
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
        let recording = mutation::recording(self);
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
            mutation::record(
                self,
                Mutation::CharacterData {
                    target: id,
                    old_value,
                },
            );
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
        crate::mutation::unlink_from_current_parent(self, id);

        let mut pending = vec![id];
        while let Some(current) = pending.pop() {
            self.form.forget(current);
            metadata::forget(self, current);
            self.shadow.forget(current, &mut pending);
            self.tree.retire(current, &mut pending);
        }
        lifecycle::record_snapshot(self, tracked);
        Ok(())
    }

    /// Whether `id` is one of the container kinds that may hold children.
    pub(crate) fn can_contain_children(&self, id: NodeId) -> bool {
        matches!(
            self.kind(id),
            Some(NodeKind::Document | NodeKind::Element { .. } | NodeKind::Fragment)
        )
    }

    /// The kinds DOM's *ensure pre-insert validity* accepts as an inserted
    /// node: everything but a document
    /// (<https://dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity>).
    pub(crate) fn is_insertable(&self, id: NodeId) -> bool {
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

    pub(crate) fn is_document(&self, id: NodeId) -> bool {
        matches!(self.kind(id), Some(NodeKind::Document))
    }

    pub(crate) fn is_doctype(&self, id: NodeId) -> bool {
        matches!(self.kind(id), Some(NodeKind::Doctype { .. }))
    }

    pub(crate) fn is_fragment(&self, id: NodeId) -> bool {
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



    /// Nodes the insert algorithm actually places: a fragment's children,
    /// otherwise the node itself
    /// (<https://dom.spec.whatwg.org/#concept-node-insert>).
    pub(crate) fn incoming_nodes(&self, node: NodeId) -> Vec<NodeId> {
        if self.is_fragment(node) {
            self.children(node)
                .map(Iterator::collect)
                .unwrap_or_default()
        } else {
            vec![node]
        }
    }

    pub(crate) fn ensure_alive(&self, a: NodeId, b: NodeId) -> Result<(), DomError> {
        self.require_live(a)?;
        self.require_live(b)
    }

    /// Single-handle variant of [`Document::ensure_alive`].
    pub(crate) fn require_live(&self, a: NodeId) -> Result<(), DomError> {
        if self.contains(a) {
            Ok(())
        } else {
            Err(DomError::StaleNode)
        }
    }

    /// True iff placing `subtree` under `into` would nest it inside itself.
    pub(crate) fn would_cycle(&self, subtree: NodeId, into: NodeId) -> bool {
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
                mutation::record(
                    self,
                    Mutation::CharacterData {
                        target: id,
                        old_value,
                    },
                );
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
