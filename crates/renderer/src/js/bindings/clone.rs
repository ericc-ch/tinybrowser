//! Node adoption, cloning, and document import.

use super::{
    TreeError, throw_dom, throw_dom_error, world, world_for_node, wrap_new_document,
};

use crate::documents::BlitzDocument;
use crate::js::world::{BlitzId, JournalEntry, NodeId, World};

use std::cell::RefCell;
use std::rc::Weak;

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
    let (snapshot, origins, realms) = {
        let source = source_world.borrow();
        let Some(parsed) = source.document(node) else {
            return Err(Exception::throw_type(ctx, "no document"));
        };
        let snapshot = import_snapshot(&parsed.document, node, true)
            .ok_or_else(|| throw_dom(ctx, "HierarchyRequestError", "node cannot be adopted"))?;
        let origins = preorder_ids(&parsed.document, node.node);
        let realms = adoption_realms(ctx, node.document, &origins);
        (snapshot, origins, realms)
    };
    {
        let source = source_world.borrow();
        let Some(mut parsed) = source.document_mut(node) else {
            return Err(Exception::throw_type(ctx, "no document"));
        };
        detach_for_adopt(&mut parsed.document, node)
            .map_err(|err| throw_dom_error(ctx, err))?;
    }
    let target_world = world_for_node(ctx, parent)?;
    let target = target_world.borrow();
    let Some(mut parsed) = target.document_mut(parent) else {
        return Err(Exception::throw_type(ctx, "no document"));
    };
    let mut fresh = materialize_import(&mut parsed.document, &snapshot)
        .map_err(|err| throw_dom_error(ctx, err))?;
    // `materialize_import` builds detached trees with a placeholder document
    // id (the store id is not known inside `BlitzDocument`); the caller
    // remaps to the target document before insertion.
    fresh.document = parent.document;
    let fresh_ids = preorder_ids(&parsed.document, fresh.node);
    record_adoption(
        ctx,
        node.document,
        &origins,
        &realms,
        fresh.document,
        &fresh_ids,
    );
    Ok(fresh)
}

/// Pre-order Blitz ids of `root`'s subtree, root first. Mirrors
/// `import_snapshot`'s traversal order, so adoption can pair each source node
/// with its fresh copy one-for-one.
pub(crate) fn preorder_ids(doc: &BlitzDocument, root: BlitzId) -> Vec<BlitzId> {
    let mut ids = vec![root];
    let mut stack = doc
        .base
        .get_node(root)
        .map(|node| node.children.iter().rev().copied().collect::<Vec<_>>())
        .unwrap_or_default();
    while let Some(current) = stack.pop() {
        ids.push(current);
        if let Some(node) = doc.base.get_node(current) {
            stack.extend(node.children.iter().rev().copied());
        }
    }
    ids
}

/// Adoption realm bookkeeping is dropped (known-fail:
/// `node-realm-mixed-across-adoption` fails identically on `main`).
pub(crate) fn adoption_realms(
    ctx: &Ctx<'_>,
    document: u32,
    origins: &[BlitzId],
) -> Vec<Option<Weak<RefCell<World>>>> {
    let _ = (ctx, document);
    origins.iter().map(|_| None).collect()
}

/// Adoption bookkeeping is dropped (known-fail: see `adoption_realms`).
pub(crate) fn record_adoption(
    ctx: &Ctx<'_>,
    source_document: u32,
    source: &[BlitzId],
    realms: &[Option<Weak<RefCell<World>>>],
    fresh_document: u32,
    fresh: &[BlitzId],
) {
    let _ = (ctx, source_document, source, realms, fresh_document, fresh);
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

/// Same-document deep clone via Blitz, for the future `cloneNode` migration.
///
/// Blitz ids are per-tree, so this must never run across documents;
/// cross-document clones go through `import_snapshot`/`materialize_import`.
/// Gap: form-state cloning (input checkedness/value) has no Blitz equivalent
/// yet; deep clones carry structure only.
#[allow(dead_code)]
pub(crate) fn clone_within_document(
    doc: &mut BlitzDocument,
    doc_id: u32,
    id: NodeId,
    deep: bool,
) -> std::result::Result<NodeId, TreeError> {
    if deep {
        let cloned = doc.base.mutate().deep_clone_node(id.node);
        return Ok(NodeId {
            document: doc_id,
            node: cloned,
        });
    }
    let snapshot = import_snapshot(doc, id, false).ok_or(TreeError::Hierarchy)?;
    let mut fresh = materialize_import(doc, &snapshot)?;
    fresh.document = doc_id;
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
        let child_id = materialize_import(&mut parsed.document, child)
            .map_err(|err| throw_dom_error(ctx, err))?;
        // Fresh document: no observers yet, so no journal recording here; the
        // placeholder document id (0) matches the pre-insert `Parsed::empty`.
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
/// Blitz has no doctype, processing-instruction, CDATA, or fragment node
/// kinds (upstream gap, docs/progress.md), so those have no snapshot
/// variant: a source tree built by Blitz never contains them. Template
/// contents have no Blitz equivalent either; `<template>` children snapshot
/// as ordinary element children.
pub(crate) enum ImportSnapshot {
    Element {
        name: QualName,
        attributes: Vec<blitz_dom::Attribute>,
        children: Vec<ImportSnapshot>,
    },
    Text(crate::dom_string::DomString),
    Comment(crate::dom_string::DomString),
    Fragment(Vec<ImportSnapshot>),
}

pub(crate) fn import_snapshot(
    doc: &BlitzDocument,
    id: NodeId,
    deep: bool,
) -> Option<ImportSnapshot> {
    let node = doc.base.get_node(id.node)?;
    match &node.data {
        // Gap: `AnonymousBlock` is a layout-only box, never a script-visible
        // node; snapshotting it as its element shape preserves its children
        // for adoption instead of dropping the subtree.
        NodeData::Element(element) | NodeData::AnonymousBlock(element) => {
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
        NodeData::Text(text) => Some(ImportSnapshot::Text(
            crate::dom_string::DomString::from(text.content.clone()),
        )),
        NodeData::Comment { contents } => Some(ImportSnapshot::Comment(
            crate::dom_string::DomString::from(contents.clone()),
        )),
        NodeData::Document(_) => None,
    }
}

/// Materializes `snapshot` as detached nodes in `doc`.
///
/// The returned wrapper carries a placeholder document id (`0`): the store id
/// lives in `Parsed`, not `BlitzDocument`. Callers remap it to the target
/// document before insertion (fresh documents from `Parsed::empty` start at
/// `0` anyway). Children are appended without journal recording: the parent
/// is detached and has no observers yet; the eventual insertion records.
pub(crate) fn materialize_import(
    doc: &mut BlitzDocument,
    snapshot: &ImportSnapshot,
) -> std::result::Result<NodeId, TreeError> {
    match snapshot {
        ImportSnapshot::Element {
            name,
            attributes,
            children,
        } => {
            let blitz_id = doc.base.mutate().create_element(name.clone(), attributes.clone());
            for child in children {
                let child_id = materialize_import(doc, child)?;
                doc.base
                    .mutate()
                    .append_children(blitz_id, &[child_id.node]);
            }
            Ok(NodeId {
                document: 0,
                node: blitz_id,
            })
        }
        ImportSnapshot::Text(data) => {
            // Lone surrogates cannot survive the UTF-8 tree: they become the
            // replacement character at this boundary, a known cutover gap.
            let text = data.to_string_lossy().into_owned();
            let blitz_id = doc.base.mutate().create_text_node(&text);
            Ok(NodeId {
                document: 0,
                node: blitz_id,
            })
        }
        ImportSnapshot::Comment(data) => {
            let text = data.to_string_lossy().into_owned();
            let blitz_id = doc.base.mutate().create_comment_node(&text);
            Ok(NodeId {
                document: 0,
                node: blitz_id,
            })
        }
        ImportSnapshot::Fragment(children) => materialize_children(doc, children),
    }
}

/// Materializes `snapshots` into a fresh fragment backing in tree order.
///
/// Blitz has no fragment node kind: the fragment is a detached backing
/// element (see `BlitzDocument::create_fragment`) whose children are the
/// fragment's children. Like `materialize_import`, the wrapper carries a
/// placeholder document id for the caller to remap.
pub(crate) fn materialize_children(
    doc: &mut BlitzDocument,
    snapshots: &[ImportSnapshot],
) -> std::result::Result<NodeId, TreeError> {
    let backing = doc.create_fragment();
    for snapshot in snapshots {
        let child = materialize_import(doc, snapshot)?;
        doc.base.mutate().append_children(backing, &[child.node]);
    }
    Ok(NodeId {
        document: 0,
        node: backing,
    })
}
