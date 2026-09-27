//! DOM mutation operations and their observer records.

mod journal;

use crate::NodeId;

pub(crate) use journal::MutationJournal;

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
pub fn set_recording(document: &mut crate::Document, recording: bool) {
    document.journal.set_recording(recording);
}

/// Drain mutation records in operation order.
pub fn take(document: &mut crate::Document) -> Vec<Mutation> {
    document.journal.take()
}

/// The serial advances on record requests and shadow-root attachment. Unlink
/// bookkeeping can skip a record request when observers are disabled.
#[must_use]
pub fn serial(document: &crate::Document) -> u64 {
    document.journal.serial()
}

pub(crate) fn record(document: &mut crate::Document, mutation: Mutation) {
    document.journal.record(mutation);
}

pub(crate) fn recording(document: &crate::Document) -> bool {
    document.journal.recording()
}

pub(crate) fn suppress(document: &mut crate::Document) {
    document.journal.suppress(true);
}

pub(crate) fn resume(document: &mut crate::Document) {
    document.journal.suppress(false);
}

pub(crate) fn bump(document: &mut crate::Document) {
    document.journal.bump();
}
