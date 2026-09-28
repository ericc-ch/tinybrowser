//! Character-data mutation workflows.
//!
//! Each setter replaces one leaf node's data, queues the observer record,
//! and re-applies the textarea selection rule when the edited node sits
//! under a `textarea`.

use crate::mutation::{self, Mutation};
use crate::{Document, DomError, NodeId, NodeKind};

fn set_data(
    document: &mut Document,
    id: NodeId,
    extract: impl Fn(&mut NodeKind) -> Option<&mut String>,
    data: String,
) -> Result<(), DomError> {
    let parent = document.parent(id);
    let value_before = parent.and_then(|parent| document.textarea_value_before_change(parent));
    let kind = document.tree.kind_mut(id).ok_or(DomError::StaleNode)?;
    match extract(kind) {
        Some(field) => {
            let old_value = std::mem::replace(field, data);
            mutation::record(
                document,
                Mutation::CharacterData {
                    target: id,
                    old_value,
                },
            );
            if let Some(parent) = parent {
                document.reset_textarea_selection_if_changed(parent, value_before);
            }
            Ok(())
        }
        None => Err(DomError::WrongNodeType),
    }
}

/// Replaces the data of the text node `id`.
///
/// # Errors
///
/// - [`DomError::StaleNode`] if `id` is stale.
/// - [`DomError::WrongNodeType`] if `id` is not a text node.
pub fn set_text(
    document: &mut Document,
    id: NodeId,
    data: impl Into<String>,
) -> Result<(), DomError> {
    set_data(
        document,
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
pub fn append_text(document: &mut Document, id: NodeId, extra: &str) -> Result<(), DomError> {
    let parent = document.parent(id);
    let value_before = parent.and_then(|parent| document.textarea_value_before_change(parent));
    let recording = mutation::recording(document);
    let old_value = {
        let kind = document.tree.kind_mut(id).ok_or(DomError::StaleNode)?;
        let NodeKind::Text { data } = kind else {
            return Err(DomError::WrongNodeType);
        };
        let old_value = recording.then(|| data.clone());
        data.push_str(extra);
        old_value
    };
    if let Some(old_value) = old_value {
        mutation::record(
            document,
            Mutation::CharacterData {
                target: id,
                old_value,
            },
        );
    }
    if let Some(parent) = parent {
        document.reset_textarea_selection_if_changed(parent, value_before);
    }
    Ok(())
}

/// Replaces the data of the comment node `id`.
///
/// # Errors
///
/// - [`DomError::StaleNode`] if `id` is stale.
/// - [`DomError::WrongNodeType`] if `id` is not a comment node.
pub fn set_comment(
    document: &mut Document,
    id: NodeId,
    data: impl Into<String>,
) -> Result<(), DomError> {
    set_data(
        document,
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
    document: &mut Document,
    id: NodeId,
    data: impl Into<String>,
) -> Result<(), DomError> {
    set_data(
        document,
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
    document: &mut Document,
    id: NodeId,
    data: impl Into<String>,
) -> Result<(), DomError> {
    set_data(
        document,
        id,
        |kind| match kind {
            NodeKind::ProcessingInstruction { data, .. } => Some(data),
            _ => None,
        },
        data.into(),
    )
}
