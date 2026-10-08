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
/// The known deviation is wrapper identity: `NodeId` is arena-scoped, so
/// pre-existing wrappers still point at the detached original instead of
/// following the adoption the way a single-arena engine would.
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
    let target_world = world_for_node(ctx, parent)?;
    let target = target_world.borrow();
    let Some(mut parsed) = target.document_mut(parent) else {
        return Err(Exception::throw_type(ctx, "no document"));
    };
    let fresh = materialize_import(&mut parsed.document, parent.document, &snapshot)
        .map_err(|err| throw_dom_error(ctx, err))?;
    Ok(fresh)
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
    let (content_type, quirks_mode, children) = {
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
        (parsed.content_type, parsed.quirks_mode, children)
    };
    let font_ctx = world(ctx)?.borrow().runtime.font_ctx.clone();
    let mut parsed = crate::Parsed::script(content_type, font_ctx);
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

pub(crate) fn import_snapshot(
    doc: &BlitzDocument,
    id: NodeId,
    deep: bool,
) -> Option<ImportSnapshot> {
    let node = doc.base.get_node(id.node)?;
    match doc.extra(id.node) {
        Some(crate::documents::ExtraNode::DocumentType {
            name,
            public_id,
            system_id,
        }) => {
            return Some(ImportSnapshot::DocumentType {
                name: name.clone(),
                public_id: public_id.clone(),
                system_id: system_id.clone(),
            });
        }
        Some(crate::documents::ExtraNode::ProcessingInstruction { target, attributes }) => {
            let data = match &node.data {
                NodeData::Comment { contents } => {
                    crate::dom_string::DomString::from(contents.clone())
                }
                _ => crate::dom_string::DomString::default(),
            };
            return Some(ImportSnapshot::ProcessingInstruction {
                target: target.clone(),
                data,
                attributes: attributes.clone(),
            });
        }
        Some(crate::documents::ExtraNode::CDataSection) => {
            let data = match &node.data {
                NodeData::Text(text) => crate::dom_string::DomString::from(text.content.clone()),
                _ => crate::dom_string::DomString::default(),
            };
            return Some(ImportSnapshot::CData(data));
        }
        None => {}
    }
    match &node.data {
        NodeData::Element(element) => {
            let name = element.name.clone();
            let attributes = element.attrs.iter().cloned().collect();
            let children = if deep {
                node.children
                    .iter()
                    .filter_map(|child| {
                        import_snapshot(
                            doc,
                            NodeId {
                                document: id.document,
                                node: *child,
                            },
                            true,
                        )
                    })
                    .collect()
            } else {
                Vec::new()
            };
            Some(ImportSnapshot::Element {
                name,
                attributes,
                children,
            })
        }
        NodeData::Text(text) => Some(ImportSnapshot::Text(crate::dom_string::DomString::from(
            text.content.clone(),
        ))),
        NodeData::Comment { contents } => Some(ImportSnapshot::Comment(
            crate::dom_string::DomString::from(contents.clone()),
        )),
        NodeData::AnonymousBlock(_) => {
            if deep {
                let children = node
                    .children
                    .iter()
                    .filter_map(|child| {
                        import_snapshot(
                            doc,
                            NodeId {
                                document: id.document,
                                node: *child,
                            },
                            true,
                        )
                    })
                    .collect();
                Some(ImportSnapshot::Fragment(children))
            } else {
                Some(ImportSnapshot::Fragment(Vec::new()))
            }
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
            let text = data.to_string_lossy().into_owned();
            let blitz_id = doc.base.mutate().create_text_node(&text);
            Ok(NodeId {
                document,
                node: blitz_id,
            })
        }
        ImportSnapshot::Comment(data) => {
            let text = data.to_string_lossy().into_owned();
            let blitz_id = doc.base.mutate().create_comment_node(&text);
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
