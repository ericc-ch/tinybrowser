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
use crate::{Document, DomError, NodeId};

pub use attributes::{
    add_attrs_if_missing, remove_attribute, remove_attribute_ns, set_attribute, set_attribute_by_ns,
};
pub use clone::clone_node;
pub(crate) use attributes::{find_attribute, merge_attrs};
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
    CharacterData { target: NodeId, old_value: String },
}

/// Enable or disable mutation-observer recording for this document.
pub fn set_recording(document: &mut Document, recording: bool) {
    document.journal.set_recording(recording);
}

/// Drain mutation records in operation order.
pub fn take(document: &mut Document) -> Vec<Mutation> {
    document.journal.take()
}

/// The serial advances on record requests and shadow-root attachment. Unlink
/// bookkeeping can skip a record request when observers are disabled.
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
