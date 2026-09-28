//! Node cloning workflows.
//!
//! Cloning copies one node shallowly, then its descendants and template
//! contents when asked. Domain state follows each copy: form dirty values
//! and template associations belong to their modules, not to the copy loop.

use crate::shadow;
use crate::{Document, DomError, NodeId};

/// [Clones](https://dom.spec.whatwg.org/#concept-node-clone) `id` into a
/// new unattached node. `subtree` copies descendants (and a template's
/// contents fragment). The document node is refused: cloning a document
/// is a different spec operation.
///
/// # Errors
///
/// - [`DomError::StaleNode`] if `id` is stale.
/// - [`DomError::WrongNodeType`] if `id` is the document.
pub fn clone_node(
    document: &mut Document,
    id: NodeId,
    subtree: bool,
) -> Result<NodeId, DomError> {
    document.require_live(id)?;
    if id == document.tree.document() {
        return Err(DomError::WrongNodeType);
    }
    let copy = document.alloc(document.kind(id).ok_or(DomError::StaleNode)?.clone());
    let mut pending = vec![(id, copy)];
    while let Some((source, target)) = pending.pop() {
        document.form.clone_dirty_value(source, target);
        // https://html.spec.whatwg.org/multipage/scripting.html#the-template-element:cloning-steps
        if let Some(contents) = shadow::template_contents(document, source) {
            let cloned_contents = document.create_fragment();
            shadow::set_template_contents(document, target, cloned_contents)?;
            if subtree {
                pending.push((contents, cloned_contents));
            }
        }
        if subtree {
            let kids: Vec<NodeId> = document
                .children(source)
                .ok_or(DomError::StaleNode)?
                .collect();
            for kid in kids {
                let child = document.alloc(document.kind(kid).ok_or(DomError::StaleNode)?.clone());
                super::append(document, target, child)?;
                pending.push((kid, child));
            }
        }
    }
    Ok(copy)
}
