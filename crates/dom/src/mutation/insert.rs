//! Insertion, replacement, removal, and bulk-move workflows.
//!
//! Every path here validates in WHATWG DOM's order, then asks `Tree` to write
//! the links, then runs the applicable domain steps and queues the mutation
//! record. `Tree` stays the only writer of parent and sibling pointers.

use crate::lifecycle;
use crate::mutation::{self, Mutation};
use crate::{Document, DomError, NodeId, NodeKind, named};

/// Appends `child` as the last child of `parent`.
///
/// Moving semantics: a `child` already attached elsewhere is detached first,
/// mirroring DOM `appendChild`.
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
pub fn append(document: &mut Document, parent: NodeId, child: NodeId) -> Result<(), DomError> {
    document.ensure_alive(parent, child)?;
    if child == document.tree.document() {
        // The root must never gain a parent; that is how a document
        // gets orphaned from itself. (Maps to HierarchyRequestError.)
        return Err(DomError::HierarchyRequest);
    }
    ensure_pre_insert_validity(document, parent, child, None)?;
    let tracked = lifecycle::snapshot(document, child);
    if document.is_fragment(child) {
        splice_fragment(document, parent, child, None);
    } else {
        place_node(document, parent, child, None);
    }
    lifecycle::record_snapshot(document, tracked);
    Ok(())
}

/// Inserts `node` immediately before `sibling` under sibling's parent.
///
/// Moving semantics, like [`append`]. Inserting a node beside itself is a
/// legal stay-put no-op (WHATWG DOM's *ensure pre-insert validity* returns
/// without doing anything in that case), not an error.
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
///   from `ensure_pre_insert_validity`.
pub fn insert_before(
    document: &mut Document,
    sibling: NodeId,
    node: NodeId,
) -> Result<(), DomError> {
    document.ensure_alive(sibling, node)?;
    // The reference child must sit under some parent to be inserted
    // beside; a detached one has none: the outer pre-insert
    // algorithm's parent-null refusal (`NotFoundError`).
    let parent = document.parent(sibling).ok_or(DomError::NoParent)?;
    if node == document.tree.document() {
        return Err(DomError::HierarchyRequest);
    }
    // Node-beside-itself means "stay put": the gate would reject this
    // as a cycle (a node contains itself), but the spec's answer is a
    // silent return, so it short-circuits before validation.
    if sibling == node {
        return Ok(());
    }
    ensure_pre_insert_validity(document, parent, node, Some(sibling))?;
    let tracked = lifecycle::snapshot(document, node);
    if document.is_fragment(node) {
        splice_fragment(document, parent, node, Some(sibling));
    } else {
        place_node(document, parent, node, Some(sibling));
    }
    lifecycle::record_snapshot(document, tracked);
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
///   from `ensure_pre_insert_validity`.
pub fn replace_child(
    document: &mut Document,
    parent: NodeId,
    node: NodeId,
    child: NodeId,
) -> Result<(), DomError> {
    document.require_live(parent)?;
    document.require_live(node)?;
    document.require_live(child)?;
    if !document.can_contain_children(parent) {
        return Err(DomError::HierarchyRequest);
    }
    if document.would_cycle(node, parent) {
        return Err(DomError::CycleForbidden);
    }
    if document.parent(child) != Some(parent) {
        return Err(DomError::NoParent);
    }
    // Same insertability gate as pre-insert
    // (<https://dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity>).
    if !document.is_insertable(node) {
        return Err(DomError::HierarchyRequest);
    }
    if !document.is_document(parent) && !document.is_fragment(node) && document.is_doctype(node) {
        return Err(DomError::HierarchyRequest);
    }
    if document.is_document(parent) {
        let incoming = document.incoming_nodes(node);
        let mut sequence: Vec<NodeId> = Vec::new();
        for existing in document.children(parent).into_iter().flatten() {
            if existing == child {
                sequence.extend_from_slice(&incoming);
            } else {
                sequence.push(existing);
            }
        }
        ensure_document_content_model(document, &sequence)?;
    }
    let previous = document.sibling(child, false);
    let mut reference = document.sibling(child, true);
    if reference == Some(node) {
        reference = document.sibling(node, true);
    }
    let added = document.incoming_nodes(node);
    // Adopt `node` first: removing it from its old parent stays
    // observable even though the replacement suppresses observers
    // (<https://dom.spec.whatwg.org/#concept-node-adopt>). Its connection
    // transitions coalesce, like every other move.
    record_unlink(document, node);
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
    let node_tracked = lifecycle::snapshot(document, node);
    let child_tracked = (child != node && document.parent(child) == Some(parent))
        .then(|| lifecycle::snapshot(document, child));
    let mut removed = Vec::new();
    mutation::suppress(document);
    if child != node && document.parent(child) == Some(parent) {
        unlink_from_current_parent(document, child);
        removed.push(child);
    }
    if document.is_fragment(node) {
        splice_fragment(document, parent, node, reference);
    } else {
        place_node(document, parent, node, reference);
    }
    mutation::resume(document);
    if let Some(child_tracked) = child_tracked {
        lifecycle::record_snapshot(document, child_tracked);
    }
    lifecycle::record_snapshot(document, node_tracked);
    mutation::record(
        document,
        Mutation::ChildList {
            target: parent,
            added,
            removed,
            previous,
            next: reference,
        },
    );
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
    document: &Document,
    parent: NodeId,
    node: NodeId,
    reference: Option<NodeId>,
) -> Result<(), DomError> {
    if !document.can_contain_children(parent) {
        return Err(DomError::HierarchyRequest);
    }
    if document.would_cycle(node, parent) {
        return Err(DomError::CycleForbidden);
    }
    if let Some(reference) = reference
        && document.parent(reference) != Some(parent)
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
    document: &mut Document,
    parent: NodeId,
    node: NodeId,
    reference: Option<NodeId>,
) -> Result<(), DomError> {
    document.ensure_alive(parent, node)?;
    if !document.can_contain_children(parent) {
        return Err(DomError::HierarchyRequest);
    }
    if document.would_cycle(node, parent) {
        return Err(DomError::CycleForbidden);
    }
    if let Some(reference) = reference {
        document.require_live(reference)?;
        if document.parent(reference) != Some(parent) {
            return Err(DomError::NoParent);
        }
    }
    ensure_pre_insert_validity(document, parent, node, reference)?;
    // The reference child may be the node itself: inserting a node beside
    // itself is a legal stay-put no-op.
    let reference = if reference == Some(node) {
        document.sibling(node, true)
    } else {
        reference
    };
    let tracked = lifecycle::snapshot(document, node);
    if document.is_fragment(node) {
        splice_fragment(document, parent, node, reference);
    } else {
        place_node(document, parent, node, reference);
    }
    lifecycle::record_snapshot(document, tracked);
    Ok(())
}

/// WHATWG DOM's *ensure pre-insert validity*: the one gate every
/// insertion path walks, mirroring
/// <https://dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity>.
/// Engine reference: Firefox's `EnsureAllowedAsChild` in
/// `dom/base/nsINode.cpp` (mozilla-firefox/firefox).
///
/// Comments map each refusal to its rule in the spec's current wording
/// (the algorithm has been restructured before; anchors, not step
/// numbers, are what stay true). Refusals here are always
/// [`DomError::HierarchyRequest`] or [`DomError::CycleForbidden`]; the
/// document content model itself is encoded exactly once, in
/// `ensure_document_content_model`.
fn ensure_pre_insert_validity(
    document: &Document,
    parent: NodeId,
    node: NodeId,
    reference: Option<NodeId>,
) -> Result<(), DomError> {
    // Container kinds only. Leaves with a child list would be arena corruption.
    // Kind stays borrowed: this gate is on the parse hot path.
    if !document.can_contain_children(parent) {
        return Err(DomError::HierarchyRequest);
    }
    // Only DocumentFragment, DocumentType, Element, and CharacterData
    // nodes are insertable; a Document node is refused here
    // (<https://dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity> step 4).
    if !document.is_insertable(node) {
        return Err(DomError::HierarchyRequest);
    }
    // Reference-child membership (the reference belongs to `parent`) is
    // enforced by construction: the only caller that passes `Some`
    // derives `parent` from that same handle's own parent pointer.
    //
    // Cycle rule: `node` must not be an inclusive ancestor of `parent`;
    // inserting a subtree into itself would tear the arena's acyclicity.
    if document.would_cycle(node, parent) {
        return Err(DomError::CycleForbidden);
    }
    // `insert_before` handles node-beside-itself (spec: do nothing)
    // before this gate.
    //
    // Content-model rules. Outside a document, a doctype as the
    // inserted node is never welcome; a fragment's children are not
    // checked here (the spec returns after the doctype-on-node
    // test: <https://dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity>).
    if !document.is_document(parent) {
        if !document.is_fragment(node) && document.is_doctype(node) {
            return Err(DomError::HierarchyRequest);
        }
        return Ok(());
    }
    // Inside a document: splice the incoming nodes (the node itself,
    // or a fragment's children —
    // <https://dom.spec.whatwg.org/#concept-node-insert>) into the
    // standing children at the insertion point and hand the whole
    // resulting sequence to the content model.
    let incoming = document.incoming_nodes(node);
    let mut sequence: Vec<NodeId> = Vec::new();
    let mut inserted = false;
    if let Some(kids) = document.children(parent) {
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
    ensure_document_content_model(document, &sequence)
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
/// see `ensure_document_content_model`. A bulk move is one
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
pub fn reparent_children(
    document: &mut Document,
    from: NodeId,
    to: NodeId,
) -> Result<(), DomError> {
    document.ensure_alive(from, to)?;
    if from == to {
        return Ok(());
    }
    // Both endpoints accept children, or the move would strand nodes under leaves.
    for endpoint in [from, to] {
        if !document.can_contain_children(endpoint) {
            return Err(DomError::HierarchyRequest);
        }
    }
    if from == document.tree.document() {
        // Draining the root would strand the entire document inside an
        // arbitrary detached subtree. (Maps to HierarchyRequestError.)
        return Err(DomError::HierarchyRequest);
    }
    if document.would_cycle(from, to) {
        return Err(DomError::CycleForbidden);
    }
    if document.is_document(to) {
        // The model sees the whole *resulting sequence*: the document's
        // standing children followed by the moved run.
        let mut sequence: Vec<NodeId> = Vec::new();
        if let Some(kids) = document.children(to) {
            sequence.extend(kids);
        }
        if let Some(kids) = document.children(from) {
            sequence.extend(kids);
        }
        ensure_document_content_model(document, &sequence)?;
    }
    let moved: Vec<NodeId> = document
        .children(from)
        .expect("verified-live `from` has a node")
        .collect();
    if moved.is_empty() {
        return Ok(());
    }
    // Defect guards, not input errors: both handles were verified live
    // above, so a miss here means the parent-pointer/sibling-link duality
    // is broken. Panicking beats reporting a lying "stale node".
    let from_connected = lifecycle::is_connected(document, from);
    let to_connected = lifecycle::is_connected(document, to);
    let tracked: Vec<(NodeId, bool, bool)> = if from_connected == to_connected {
        Vec::new()
    } else {
        moved
            .iter()
            .flat_map(|&id| lifecycle::snapshot(document, id))
            .collect()
    };
    // Move the run one node at a time through the single-node primitives:
    // each `unlink`/`insert_linked` step is O(1), so the move stays O(k)
    // and the link invariant has exactly two writers.
    for &id in &moved {
        unlink(document, id);
    }
    for &id in &moved {
        document.tree.insert_linked(to, id, None);
        if !from_connected && to_connected {
            named::inserted(document, id);
        }
    }
    lifecycle::record_snapshot(document, tracked);
    Ok(())
}

/// Places a non-fragment `node` under `parent` before `before` (or at
/// the end when `before` is `None`).
///
/// Structural only: lifecycle recording belongs to the calling operation,
/// which captures one snapshot before it starts and records it once the
/// whole operation is done. Recording here would both misread a node the
/// caller already detached and pin the event order relative to a removal.
pub(crate) fn place_node(
    document: &mut Document,
    parent: NodeId,
    node: NodeId,
    before: Option<NodeId>,
) {
    let value_before = document.textarea_value_before_change(parent);
    unlink_from_current_parent(document, node);
    // A checked radio whose form owner changes on insertion unchecks its
    // new group
    // (<https://html.spec.whatwg.org/multipage/input.html#radio-button-group>).
    let radio_owner_before = document.checked_radio_form_owner(node);
    // Sibling references for the mutation record, read from the run the
    // node is about to join. Computed only while recording: no observer
    // exists on the parse path, so the lookups stay off it.
    let (previous, next) = if mutation::recording(document) {
        let previous = match before {
            Some(before) => document.previous_sibling(before),
            None => document.last_child(parent),
        };
        (previous, before)
    } else {
        (None, None)
    };
    document.tree.insert_linked(parent, node, before);
    named::inserted(document, node);
    mutation::record(
        document,
        Mutation::ChildList {
            target: parent,
            added: vec![node],
            removed: Vec::new(),
            previous,
            next,
        },
    );
    document.reset_textarea_selection_if_changed(parent, value_before);
    if radio_owner_before != document.checked_radio_form_owner(node) {
        document.refresh_radio_group(node);
    }
    // An option joining a select follows the option insertion steps: a
    // selected option clears the others, then the selectedness setting
    // algorithm supplies the default when nothing is selected. Only a
    // select whose list of options actually gained the node runs it.
    if document.html_local_is(node, "option") {
        document.option_added_to_select(node);
    }
    if let Some(select) = document.inserted_list_owner(node) {
        document.apply_default_selectedness(select);
    }
}

/// Insert a fragment by moving its children under `parent`, leaving the
/// fragment empty and unparented
/// (<https://dom.spec.whatwg.org/#concept-node-insert>).
///
/// Structural only; the caller records the fragment subtree's lifecycle
/// snapshot (see [`place_node`]).
pub(crate) fn splice_fragment(
    document: &mut Document,
    parent: NodeId,
    fragment: NodeId,
    before: Option<NodeId>,
) {
    let moved: Vec<NodeId> = document
        .children(fragment)
        .map(Iterator::collect)
        .unwrap_or_default();
    unlink_from_current_parent(document, fragment);
    if moved.is_empty() {
        return;
    }
    // Appending a run of unselected options yields the same selection as
    // applying the setting algorithm after every insertion. No script can
    // observe the intermediate state during the fragment splice
    // (<https://html.spec.whatwg.org/multipage/form-elements.html#selectedness-setting-algorithm>).
    let blank_options = document.html_local_is(parent, "select")
        && moved
            .iter()
            .all(|&id| document.html_local_is(id, "option") && !document.option_selected(id));
    mutation::record(
        document,
        Mutation::ChildList {
            target: fragment,
            added: Vec::new(),
            removed: moved.clone(),
            previous: None,
            next: None,
        },
    );
    // Sibling references for the mutation record, read before the run
    // leaves the fragment.
    let (previous, next) = if mutation::recording(document) {
        let previous = match before {
            Some(before) => document.previous_sibling(before),
            None => document.last_child(parent),
        };
        (previous, before)
    } else {
        (None, None)
    };
    // Move the run one node at a time. Inserting each immediately before
    // the same reference preserves order, and O(1) steps keep the splice
    // O(k) with `insert_linked` as the only insert writer.
    for &id in &moved {
        unlink(document, id);
    }
    // Insert each node and run its group rule immediately, so a radio or
    // option sees the run in insertion order: radios moved out of a
    // fragment into a form join a group, and options joining a select may
    // become the default selection.
    for &id in &moved {
        document.tree.insert_linked(parent, id, before);
        named::inserted(document, id);
        if document.checked_radio_form_owner(id).is_some() {
            document.refresh_radio_group(id);
        }
        if !blank_options {
            if document.html_local_is(id, "option") {
                document.option_added_to_select(id);
            }
            if let Some(select) = document.inserted_list_owner(id) {
                document.apply_default_selectedness(select);
            }
        }
    }
    if blank_options {
        document.apply_default_selectedness(parent);
    }
    mutation::record(
        document,
        Mutation::ChildList {
            target: parent,
            added: moved,
            removed: Vec::new(),
            previous,
            next,
        },
    );
}

/// Appends a run of fresh, unparented leaves to a live container as one
/// child-list mutation. The caller performs element-specific insertion
/// steps; only the link writer here touches parent and sibling pointers
/// (<https://dom.spec.whatwg.org/#concept-node-insert>).
pub(crate) fn append_fresh_children(document: &mut Document, parent: NodeId, added: Vec<NodeId>) {
    let previous = document.last_child(parent);
    for &node in &added {
        document.tree.insert_linked(parent, node, None);
        named::inserted(document, node);
    }
    mutation::record(
        document,
        Mutation::ChildList {
            target: parent,
            added,
            removed: Vec::new(),
            previous,
            next: None,
        },
    );
}

/// The document content model over one candidate child sequence. No
/// character data anywhere, at most one element child, at most one
/// doctype placed strictly ahead of that element; comments may sit
/// anywhere, and fragments stay opaque containers. Deliberately the *only*
/// encoding of the model: incremental insertions arrive as their resulting
/// sequence from [`ensure_pre_insert_validity`], bulk moves as the
/// document's standing children followed by the moved run. Per-child checks
/// cannot see a violating pair like `[html, main]`.
pub(crate) fn ensure_document_content_model(
    document: &Document,
    sequence: &[NodeId],
) -> Result<(), DomError> {
    let mut element_seen = false;
    let mut doctype_seen = false;
    for &id in sequence {
        match document.kind(id) {
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

/// Queues the removal record for `id` from its current parent, without
/// touching the tree.
///
/// Shared by [`unlink_from_current_parent`] and the replace algorithm's
/// adopt step, whose removal is observable even though the rest of the
/// replacement suppresses observers
/// (<https://dom.spec.whatwg.org/#concept-node-adopt>). The caller has
/// verified `id` live; a missing parent is a silent no-op (the node is
/// already detached).
fn record_unlink(document: &mut Document, id: NodeId) {
    if !mutation::recording(document) {
        return;
    }
    let Some(parent) = document.parent(id) else {
        return;
    };
    mutation::record(
        document,
        Mutation::ChildList {
            target: parent,
            added: Vec::new(),
            removed: vec![id],
            previous: document.previous_sibling(id),
            next: document.next_sibling(id),
        },
    );
}

/// Removes `id` from whichever parent currently holds it.
pub(crate) fn unlink_from_current_parent(document: &mut Document, id: NodeId) {
    if document.parent(id).is_none() {
        return;
    }
    record_unlink(document, id);
    unlink(document, id);
}

/// Splices `id` out of its parent's child run and clears its parent and
/// sibling links. A no-op when `id` is unparented.
///
/// Defect policy, like every other structural site in this module: `id`
/// was verified live, so a `Some` parent must name it and the link
/// fields must agree. Parent-pointer/sibling-link divergence is arena
/// corruption; panicking beats silently producing a node with two
/// parents (or none), which later mutations would compound.
fn unlink(document: &mut Document, id: NodeId) {
    let Some(parent) = document.parent(id) else {
        return;
    };
    let select = document.inserted_list_owner(id);
    let value_before = document.textarea_value_before_change(parent);
    document.tree.unlink_linked(id);
    document.reset_textarea_selection_if_changed(parent, value_before);
    if let Some(select) = select {
        document.apply_default_selectedness(select);
    }
}
