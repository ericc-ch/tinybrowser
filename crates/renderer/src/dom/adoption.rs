//! Adoption, cloning, and cross-document import.
//!
//! Snapshots (`ImportSnapshot`) decouple the source subtree from the target
//! arena: Blitz ids are per-tree, so cross-document adoption snapshots the
//! source, detaches it, and materializes an equivalent subtree in the target
//! (<https://dom.spec.whatwg.org/#concept-node-adopt>,
//! <https://dom.spec.whatwg.org/#concept-node-clone>). Everything here works
//! on `&BlitzDocument` / `&mut DocumentStore` and ids; wrapper retargeting
//! and nonce slots stay with the binding layer.

use blitz_dom::NodeData;
use markup5ever::QualName;

use crate::documents::{BlitzDocument, DocumentStore, JournalEntry};
use crate::dom::TreeError;
use crate::dom::tree::NodeId;

/// Owned snapshot of a subtree for cross-document `importNode`.
pub(crate) enum ImportSnapshot {
    Element {
        name: QualName,
        attributes: Vec<blitz_dom::Attribute>,
        children: Vec<ImportSnapshot>,
    },
    Text(crate::dom_string::DomString),
    Comment(crate::dom_string::DomString),
    DocumentType {
        name: String,
        public_id: String,
        system_id: String,
    },
    ProcessingInstruction {
        target: String,
        data: crate::dom_string::DomString,
    },
    CData(crate::dom_string::DomString),
    Fragment(Vec<ImportSnapshot>),
}

/// Deep-snapshots `children` for cross-document cloning.
pub(crate) fn snapshot_children(
    doc: &BlitzDocument,
    document: u32,
    children: impl IntoIterator<Item = blitz_traits::node_id::NodeId>,
) -> Vec<ImportSnapshot> {
    children
        .into_iter()
        .filter_map(|child| {
            import_snapshot(
                doc,
                NodeId {
                    document,
                    node: child,
                },
                true,
            )
        })
        .collect()
}

pub(crate) fn import_snapshot(
    doc: &BlitzDocument,
    id: NodeId,
    deep: bool,
) -> Option<ImportSnapshot> {
    // Fragment backings must snapshot as fragments, not as their `div`
    // element. Import and adopt would otherwise insert a stray element
    // (<https://dom.spec.whatwg.org/#concept-node-clone>).
    if doc.is_fragment(id.node) {
        let children = if deep {
            snapshot_children(
                doc,
                id.document,
                doc.base
                    .get_node(id.node)
                    .map(|backing| backing.children.iter().copied().collect::<Vec<_>>())
                    .unwrap_or_default(),
            )
        } else {
            Vec::new()
        };
        return Some(ImportSnapshot::Fragment(children));
    }
    let node = doc.base.get_node(id.node)?;
    match &node.data {
        NodeData::Element(element) => {
            let name = element.name.clone();
            let attributes = element.attrs.iter().cloned().collect();
            // Template children live in the template contents fragment, not
            // as element children: snapshot from the contents
            // (<https://html.spec.whatwg.org/multipage/scripting.html#the-template-element>).
            let children = if deep {
                if crate::dom::tree::is_template_tag(&name) {
                    doc.base
                        .get_node(id.node)
                        .and_then(|node| node.data.downcast_element())
                        .and_then(|element| element.template_contents)
                        .map(|fragment| {
                            doc.base
                                .get_node(fragment)
                                .map(|backing| backing.children.iter().copied().collect::<Vec<_>>())
                                .unwrap_or_default()
                        })
                        .map(|child_ids| snapshot_children(doc, id.document, child_ids))
                        .unwrap_or_default()
                } else {
                    snapshot_children(doc, id.document, node.children.iter().copied())
                }
            } else {
                Vec::new()
            };
            Some(ImportSnapshot::Element {
                name,
                attributes,
                children,
            })
        }
        NodeData::Text(_) => Some(ImportSnapshot::Text(doc.character_data(id.node))),
        NodeData::Comment { .. } => Some(ImportSnapshot::Comment(doc.character_data(id.node))),
        NodeData::Doctype {
            name,
            public_id,
            system_id,
        } => Some(ImportSnapshot::DocumentType {
            name: name.clone(),
            public_id: public_id.clone(),
            system_id: system_id.clone(),
        }),
        NodeData::ProcessingInstruction { target, .. } => {
            Some(ImportSnapshot::ProcessingInstruction {
                target: target.clone(),
                data: doc.character_data(id.node),
            })
        }
        NodeData::CDataSection { .. } => Some(ImportSnapshot::CData(doc.character_data(id.node))),
        NodeData::AnonymousBlock(_) | NodeData::Fragment { .. } => {
            let children = if deep {
                snapshot_children(doc, id.document, node.children.iter().copied())
            } else {
                Vec::new()
            };
            Some(ImportSnapshot::Fragment(children))
        }
        NodeData::Document(_) => None,
    }
}

/// Materializes `snapshot` as detached nodes in `doc` for `document`.
/// Children are appended without journal recording: the parent
/// is detached and has no observers yet; the eventual insertion records.
pub(crate) fn materialize_import(
    doc: &mut BlitzDocument,
    document: u32,
    snapshot: &ImportSnapshot,
) -> std::result::Result<NodeId, TreeError> {
    match snapshot {
        ImportSnapshot::Element {
            name,
            attributes,
            children,
        } => {
            let blitz_id = doc
                .base
                .mutate()
                .create_element(name.clone(), attributes.clone());
            // Template children materialize into the fresh element's
            // contents fragment, which creation already established.
            let parent = if crate::dom::tree::is_template_tag(name) {
                doc.base.mutate().template_contents(blitz_id)
            } else {
                blitz_id
            };
            for child in children {
                let child_id = materialize_import(doc, document, child)?;
                doc.base.mutate().append_children(parent, &[child_id.node]);
            }
            Ok(NodeId {
                document,
                node: blitz_id,
            })
        }
        ImportSnapshot::Text(data) => {
            let blitz_id = doc.create_text(data);
            Ok(NodeId {
                document,
                node: blitz_id,
            })
        }
        ImportSnapshot::Comment(data) => {
            let blitz_id = doc.create_comment(data);
            Ok(NodeId {
                document,
                node: blitz_id,
            })
        }
        ImportSnapshot::DocumentType {
            name,
            public_id,
            system_id,
        } => {
            let blitz_id = doc.create_doctype(name, public_id, system_id);
            Ok(NodeId {
                document,
                node: blitz_id,
            })
        }
        ImportSnapshot::ProcessingInstruction { target, data } => {
            let text = data.to_string_lossy().into_owned();
            let blitz_id = doc.create_processing_instruction(target, &text);
            Ok(NodeId {
                document,
                node: blitz_id,
            })
        }
        ImportSnapshot::CData(data) => {
            let text = data.to_string_lossy().into_owned();
            let blitz_id = doc.create_cdata_section(&text);
            Ok(NodeId {
                document,
                node: blitz_id,
            })
        }
        ImportSnapshot::Fragment(children) => materialize_children(doc, document, children),
    }
}

/// Materializes `snapshots` into a fresh fragment backing in tree order.
///
/// Blitz has no fragment node kind: the fragment is a detached backing
/// element (see `BlitzDocument::create_fragment`) whose children are the
/// fragment's children.
pub(crate) fn materialize_children(
    doc: &mut BlitzDocument,
    document: u32,
    snapshots: &[ImportSnapshot],
) -> std::result::Result<NodeId, TreeError> {
    let backing = doc.create_fragment();
    for snapshot in snapshots {
        let child = materialize_import(doc, document, snapshot)?;
        doc.base.mutate().append_children(backing, &[child.node]);
    }
    Ok(NodeId {
        document,
        node: backing,
    })
}

/// Detaches `id` from its parent for adoption, recording the removal.
///
/// A node with no parent is already detached; that is a no-op.
pub(crate) fn detach_for_adopt(
    doc: &mut BlitzDocument,
    id: NodeId,
) -> std::result::Result<(), TreeError> {
    let Some(parent) = doc.base.get_node(id.node).and_then(|node| node.parent) else {
        return Ok(());
    };
    let (previous, next) = {
        let parent_node = doc.base.get_node(parent).ok_or(TreeError::Stale)?;
        let position = parent_node
            .children
            .iter()
            .position(|child| *child == id.node)
            .ok_or(TreeError::NotFound)?;
        let previous = position
            .checked_sub(1)
            .and_then(|index| parent_node.children.get(index).copied())
            .map(|node| NodeId {
                document: id.document,
                node,
            });
        let next = parent_node
            .children
            .get(position + 1)
            .copied()
            .map(|node| NodeId {
                document: id.document,
                node,
            });
        (previous, next)
    };
    doc.base.mutate().remove_node(id.node);
    doc.record(JournalEntry::ChildList {
        target: NodeId {
            document: id.document,
            node: parent,
        },
        added: Vec::new(),
        removed: vec![id],
        previous,
        next,
    });
    Ok(())
}

/// Same-document clone via Blitz.
/// Blitz ids are per-tree, so this must never run across documents;
/// cross-document clones go through `import_snapshot`/`materialize_import`.
pub(crate) fn clone_within_document(
    doc: &mut BlitzDocument,
    doc_id: u32,
    id: NodeId,
    deep: bool,
) -> std::result::Result<NodeId, TreeError> {
    // Fragment backings are ordinary `div` elements in the arena. Blitz
    // `deep_clone_node` would copy that element without the fragment set,
    // so `cloneNode` would return an `HTMLDivElement` (nodeType 1) instead
    // of a `DocumentFragment` (nodeType 11)
    // (<https://dom.spec.whatwg.org/#concept-node-clone>).
    if doc.is_fragment(id.node) {
        let snapshot = import_snapshot(doc, id, deep).ok_or(TreeError::Hierarchy)?;
        return materialize_import(doc, doc_id, &snapshot);
    }
    if deep {
        let cloned = doc.base.mutate().deep_clone_node(id.node);
        // Blitz copies node data only; the PI attribute map lives beside
        // the tree and follows in lockstep.
        doc.copy_pi_subtree(id.node, cloned);
        return Ok(NodeId {
            document: doc_id,
            node: cloned,
        });
    }
    let snapshot = import_snapshot(doc, id, false).ok_or(TreeError::Hierarchy)?;
    let fresh = materialize_import(doc, doc_id, &snapshot)?;
    Ok(fresh)
}

/// Adopts `node` into `parent`'s document through the shared store: snapshot
/// the source subtree, detach it, and materialize an equivalent subtree in
/// the target. Same-document nodes return as-is. The caller retargets
/// wrappers and carries nonce slots, which are realm state.
pub(crate) fn adopt_across_store(
    store: &mut DocumentStore,
    parent: NodeId,
    node: NodeId,
) -> Result<NodeId, TreeError> {
    if node.document == parent.document {
        return Ok(node);
    }
    let snapshot = store
        .get(node.document)
        .and_then(|parsed| import_snapshot(&parsed.document, node, true))
        .ok_or(TreeError::Hierarchy)?;
    store
        .get_mut(node.document)
        .ok_or(TreeError::Stale)
        .and_then(|parsed| detach_for_adopt(&mut parsed.document, node))?;
    let target = store.get_mut(parent.document).ok_or(TreeError::Stale)?;
    materialize_import(&mut target.document, parent.document, &snapshot)
}
