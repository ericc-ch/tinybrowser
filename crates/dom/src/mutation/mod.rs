//! DOM mutation operations and their observer records.
//!
//! This module owns the observable mutation workflows: removal, insertion,
//! attribute and text changes, and cloning. `Tree` remains the only writer of
//! parent and sibling links; operations here sequence the spec steps and queue
//! the observer records.

mod attributes;
mod clone;
mod insert;
mod journal;
mod text;

use crate::lifecycle;
use crate::string::DomString;
use crate::{Document, DomError, NodeId};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

/// Shared operation order for documents whose observer notifications share
/// an agent. Create one order, attach it with [`share_order`], then merge
/// [`take_ordered`] results by their position to preserve mutation chronology.
#[derive(Clone, Debug, Default)]
pub struct MutationOrder(Arc<AtomicU64>);

impl MutationOrder {
    fn next(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed)
    }
}

pub use attributes::{
    add_attrs_if_missing, remove_attribute, remove_attribute_ns, set_attribute, set_attribute_by_ns,
};
pub(crate) use attributes::{find_attribute, merge_attrs};
pub use clone::clone_node;
pub use insert::{
    append, destroy, insert_before, pre_insert, reparent_children, replace_all, replace_child,
    validate_pre_insert,
};
pub(crate) use insert::{
    append_fresh_children, ensure_document_content_model, place_node, splice_fragment,
    unlink_from_current_parent,
};
pub(crate) use journal::MutationJournal;
pub use text::{append_text, set_cdata_section, set_comment, set_processing_instruction, set_text};

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
    CharacterData {
        target: NodeId,
        old_value: DomString,
    },
}

/// Enable or disable mutation-observer recording for this document.
pub fn set_recording(document: &mut Document, recording: bool) {
    document.journal.set_recording(recording);
}

/// Drain mutation records in operation order.
pub fn take(document: &mut Document) -> Vec<Mutation> {
    document.journal.take()
}

/// Attach a document to a shared observer operation order. Existing queued
/// records receive positions at attachment time, in their original order.
/// Reattaching the same order preserves every queued position.
/// For example, attach each iframe tree to its renderer's `MutationOrder`.
pub fn share_order(document: &mut Document, order: &MutationOrder) {
    document.journal.share_order(order);
}

/// Drain records with their positions in the document's shared operation
/// order. For example, merge parent and iframe results and sort by position
/// before delivering records to an observer of both trees.
pub fn take_ordered(document: &mut Document) -> Vec<(u64, Mutation)> {
    document.journal.take_ordered()
}

/// The serial advances on record requests and shadow-root attachment. A
/// suppressed unlink skips its record request while observers are disabled,
/// so no serial bump happens for it; suppressed inserts still record (and
/// bump) through the normal path.
#[must_use]
pub fn serial(document: &Document) -> u64 {
    document.journal.serial()
}

pub(crate) fn record(document: &mut Document, mutation: Mutation) {
    document.journal.record(mutation);
}

pub(crate) fn recording(document: &Document) -> bool {
    document.journal.recording()
}

pub(crate) fn suppress(document: &mut Document) {
    document.journal.suppress(true);
}

pub(crate) fn resume(document: &mut Document) {
    document.journal.suppress(false);
}

pub(crate) fn bump(document: &mut Document) {
    document.journal.bump();
}

/// Unlinks `id` from its parent while leaving its subtree alive
/// (<https://dom.spec.whatwg.org/#concept-node-remove>). Detaching an already
/// detached node is a success, not an error; the document root cannot be
/// detached. Connection transitions for `iframe` and `img` descendants are
/// recorded from a snapshot taken before the removal.
///
/// # Errors
///
/// - [`DomError::StaleNode`] if `id` is stale.
/// - [`DomError::HierarchyRequest`] for the document root.
pub fn detach(document: &mut Document, id: NodeId) -> Result<(), DomError> {
    document.require_live(id)?;
    if id == document.document() {
        return Err(DomError::HierarchyRequest);
    }
    let tracked = lifecycle::snapshot(document, id);
    insert::unlink_from_current_parent(document, id);
    lifecycle::record_snapshot(document, tracked);
    Ok(())
}
