//! Node adoption, cloning, and document import.

use super::{TreeError, throw_dom, throw_dom_error, world, world_for_node, wrap_new_document};

use crate::documents::BlitzDocument;
use crate::js::world::{JournalEntry, NodeId};

use blitz_dom::NodeData;
use markup5ever::QualName;

use rquickjs::{Ctx, Exception, Result, Value};

/// Same-document insert returns `node`. A node from another document adopts
/// into the parent's document: the subtree snapshots, the original detaches,
/// and an equivalent subtree materializes in the target
/// ([adopt](https://dom.spec.whatwg.org/#concept-node-adopt)).
///
/// Blitz ids are per-tree, so the copy is a different id. Wrappers this realm
/// already holds follow that copy, including descendants, so
/// `parent.appendChild(node)` leaves `node.parentNode === parent`.
pub(crate) fn adopt_across_documents(
    ctx: &Ctx<'_>,
    parent: NodeId,
    node: NodeId,
) -> Result<NodeId> {
    if node.document == parent.document {
        return Ok(node);
    }
    // https://dom.spec.whatwg.org/#concept-node-adopt
    // Blitz ids are per-tree slot+version, so `target.mutate().deep_clone_node`
    // cannot clone across documents (it would alias the wrong slot in the
    // target arena). Cross-document adoption snapshots the source subtree and
    // materializes an equivalent detached subtree in the target instead.
    // `deep_clone_node` remains the path for same-document deep clones (see
    // `clone_within_document`).
    let source_world = world_for_node(ctx, node)?;
    let snapshot = {
        let source = source_world.borrow();
        let Some(parsed) = source.document(node) else {
            return Err(Exception::throw_type(ctx, "no document"));
        };
        import_snapshot(&parsed.document, node, true)
            .ok_or_else(|| throw_dom(ctx, "HierarchyRequestError", "node cannot be adopted"))?
    };
    {
        let source = source_world.borrow();
        let Some(mut parsed) = source.document_mut(node) else {
            return Err(Exception::throw_type(ctx, "no document"));
        };
        detach_for_adopt(&mut parsed.document, node).map_err(|err| throw_dom_error(ctx, err))?;
    }
    let fresh = {
        let target_world = world_for_node(ctx, parent)?;
        let target = target_world.borrow();
        let Some(mut parsed) = target.document_mut(parent) else {
            return Err(Exception::throw_type(ctx, "no document"));
        };
        materialize_import(&mut parsed.document, parent.document, &snapshot)
            .map_err(|err| throw_dom_error(ctx, err))?
    };
    retarget_adopted_subtree(ctx, node, fresh)?;
    Ok(fresh)
}

/// [Adopts](https://dom.spec.whatwg.org/#dom-document-adoptnode) `node` into
/// `document`. A document throws; a same-document node is only removed from
/// its parent.
pub(crate) fn adopt_into_document(ctx: &Ctx<'_>, document: NodeId, node: NodeId) -> Result<NodeId> {
    let is_document = {
        let owner = world_for_node(ctx, node)?;
        owner
            .borrow()
            .document(node)
            .is_some_and(|parsed| parsed.document.base.root_node().id == node.node)
    };
    if is_document {
        return Err(throw_dom(
            ctx,
            "NotSupportedError",
            "cannot adopt a document",
        ));
    }
    if node.document == document.document {
        let owner = world_for_node(ctx, node)?;
        let owner = owner.borrow();
        let Some(mut parsed) = owner.document_mut(node) else {
            return Err(Exception::throw_type(ctx, "no document"));
        };
        detach_for_adopt(&mut parsed.document, node).map_err(|err| throw_dom_error(ctx, err))?;
        return Ok(node);
    }
    adopt_across_documents(ctx, document, node)
}

/// Points wrappers for `from` and its descendants at the copied subtree `to`.
///
/// The two trees match child for child because `to` was materialized from a
/// deep snapshot of `from`. Each lookup borrows one document: the store is a
/// single `RefCell`.
fn retarget_adopted_subtree(ctx: &Ctx<'_>, from: NodeId, to: NodeId) -> Result<()> {
    super::retarget_wrapper(ctx, from, to)?;
    let from_children = adopted_children(ctx, from)?;
    let to_children = adopted_children(ctx, to)?;
    for (from_child, to_child) in from_children.into_iter().zip(to_children) {
        retarget_adopted_subtree(ctx, from_child, to_child)?;
    }
    Ok(())
}

fn adopted_children(ctx: &Ctx<'_>, id: NodeId) -> Result<Vec<NodeId>> {
    let owner = world_for_node(ctx, id)?;
    let owner = owner.borrow();
    let Some(parsed) = owner.document(id) else {
        return Err(Exception::throw_type(ctx, "no document"));
    };
    Ok(parsed
        .document
        .base
        .get_node(id.node)
        .map(|node| {
            node.children
                .iter()
                .copied()
                .map(|child| NodeId {
                    document: id.document,
                    node: child,
                })
                .collect()
        })
        .unwrap_or_default())
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
        // Blitz copies element, text, and comment data only. Doctype,
        // processing-instruction, and CDATA records live beside the tree.
        doc.copy_extra_subtree(id.node, cloned);
        return Ok(NodeId {
            document: doc_id,
            node: cloned,
        });
    }
    let snapshot = import_snapshot(doc, id, false).ok_or(TreeError::Hierarchy)?;
    let fresh = materialize_import(doc, doc_id, &snapshot)?;
    Ok(fresh)
}

/// [Clones](https://dom.spec.whatwg.org/#concept-node-clone) a document into a
/// new tree in this world. A document clone is a different document, not a
/// node in the same arena, so the document node itself is never snapshotted.
pub(crate) fn clone_document<'js>(ctx: &Ctx<'js>, id: NodeId, deep: bool) -> Result<Value<'js>> {
    let world_rc = world(ctx)?;
    let (content_type, xml_document, quirks_mode, children) = {
        let world = world_rc.borrow();
        let Some(parsed) = world.document(id) else {
            return Err(Exception::throw_type(ctx, "no document"));
        };
        let children = if deep {
            let root = parsed.document.base.root_node().id;
            let kids: Vec<blitz_traits::node_id::NodeId> = parsed
                .document
                .base
                .get_node(root)
                .map(|node| node.children.iter().copied().collect())
                .unwrap_or_default();
            kids.into_iter()
                .filter_map(|kid| {
                    import_snapshot(
                        &parsed.document,
                        NodeId {
                            document: id.document,
                            node: kid,
                        },
                        true,
                    )
                })
                .collect()
        } else {
            Vec::new()
        };
        (
            parsed.content_type,
            parsed.xml_document,
            parsed.quirks_mode,
            children,
        )
    };
    let font_ctx = world(ctx)?.borrow().runtime.font_ctx.clone();
    let mut parsed = crate::Parsed::script(content_type, font_ctx);
    parsed.xml_document = xml_document;
    parsed.quirks_mode = quirks_mode;
    let root = parsed.document.base.root_node().id;
    for child in &children {
        let child_id = materialize_import(&mut parsed.document, 0, child)
            .map_err(|err| throw_dom_error(ctx, err))?;
        parsed
            .document
            .base
            .mutate()
            .append_children(root, &[child_id.node]);
    }
    wrap_new_document(ctx, parsed)
}

/// Owned snapshot of a subtree for cross-document `importNode`.
///
/// Doctype, processing instruction, and CDATA are side-table records on a
/// comment or text backing
/// (<https://dom.spec.whatwg.org/#concept-node-clone>). Template contents
/// have no Blitz equivalent; `<template>` children snapshot as ordinary
/// element children.
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
        attributes: Vec<(String, String)>,
    },
    CData(crate::dom_string::DomString),
    Fragment(Vec<ImportSnapshot>),
}

fn extra_snapshot(doc: &BlitzDocument, id: NodeId) -> Option<ImportSnapshot> {
    match doc.extra(id.node).cloned() {
        Some(crate::documents::ExtraNode::DocumentType {
            name,
            public_id,
            system_id,
        }) => Some(ImportSnapshot::DocumentType {
            name,
            public_id,
            system_id,
        }),
        Some(crate::documents::ExtraNode::ProcessingInstruction { target, attributes }) => {
            Some(ImportSnapshot::ProcessingInstruction {
                target,
                data: doc.character_data(id.node),
                attributes,
            })
        }
        Some(crate::documents::ExtraNode::CDataSection) => {
            Some(ImportSnapshot::CData(doc.character_data(id.node)))
        }
        None => None,
    }
}

fn snapshot_children(
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
    if let Some(extra) = extra_snapshot(doc, id) {
        return Some(extra);
    }
    let node = doc.base.get_node(id.node)?;
    match &node.data {
        NodeData::Element(element) => {
            let name = element.name.clone();
            let attributes = element.attrs.iter().cloned().collect();
            let children = if deep {
                snapshot_children(doc, id.document, node.children.iter().copied())
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
        NodeData::AnonymousBlock(_) => {
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
            for child in children {
                let child_id = materialize_import(doc, document, child)?;
                doc.base
                    .mutate()
                    .append_children(blitz_id, &[child_id.node]);
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
            let blitz_id = doc.create_doctype(name.clone(), public_id.clone(), system_id.clone());
            Ok(NodeId {
                document,
                node: blitz_id,
            })
        }
        ImportSnapshot::ProcessingInstruction {
            target,
            data,
            attributes,
        } => {
            let text = data.to_string_lossy().into_owned();
            let blitz_id = doc.create_processing_instruction(target.clone(), &text);
            // The stored map wins over a fresh parse so a `setAttribute`
            // name the grammar rejects still clones
            // (<https://dom.spec.whatwg.org/#concept-node-clone>).
            doc.set_pi_attributes(blitz_id, attributes.clone());
            doc.set_exact_character_data(blitz_id, data);
            Ok(NodeId {
                document,
                node: blitz_id,
            })
        }
        ImportSnapshot::CData(data) => {
            let text = data.to_string_lossy().into_owned();
            let blitz_id = doc.create_cdata_section(&text);
            doc.set_exact_character_data(blitz_id, data);
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
