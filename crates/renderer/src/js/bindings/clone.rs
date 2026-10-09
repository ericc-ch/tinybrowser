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
    let snapshot = import_snapshot_live(ctx, node, true)?
        .ok_or_else(|| throw_dom(ctx, "HierarchyRequestError", "node cannot be adopted"))?;
    let source_world = world_for_node(ctx, node)?;
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
    // A template's contents fragment follows its host: retarget it so a
    // previously fetched `content` wrapper keeps pointing at live content.
    let contents = |id: NodeId| {
        world_for_node(ctx, id).ok().and_then(|owner| {
            let owner = owner.borrow();
            owner.document(id).and_then(|parsed| {
                parsed
                    .document
                    .base
                    .get_node(id.node)
                    .and_then(|node| node.data.downcast_element())
                    .filter(|element| {
                        element.name.ns == crate::js::world::html_namespace()
                            && element.name.local.as_ref() == "template"
                    })
                    .and_then(|element| element.template_contents)
                    .map(|fragment| NodeId {
                        document: id.document,
                        node: fragment,
                    })
            })
        })
    };
    if let Some(from_contents) = contents(from)
        && let Some(to_contents) = contents(to)
    {
        super::retarget_wrapper(ctx, from_contents, to_contents)?;
    }
    let from_children = adopted_children(ctx, from)?;
    let to_children = adopted_children(ctx, to)?;
    for (from_child, to_child) in from_children.into_iter().zip(to_children) {
        retarget_adopted_subtree(ctx, from_child, to_child)?;
    }
    Ok(())
}

/// The live children of an adopted node for wrapper retargeting.
/// Template children live in the template contents fragment, so those are
/// retargeted instead of the (empty) element children.
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
            if let Some(element) = node.data.downcast_element()
                && element.name.ns == crate::js::world::html_namespace()
                && element.name.local.as_ref() == "template"
                && let Some(fragment) = element.template_contents
            {
                return parsed
                    .document
                    .base
                    .get_node(fragment)
                    .map(|backing| {
                        backing
                            .children
                            .iter()
                            .copied()
                            .map(|child| NodeId {
                                document: id.document,
                                node: child,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
            }
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
            parsed
                .document
                .base
                .get_node(root)
                .map(|node| {
                    node.children
                        .iter()
                        .copied()
                        .map(|kid| NodeId {
                            document: id.document,
                            node: kid,
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
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
    let mut snapshots = Vec::new();
    for child in children {
        if let Some(snapshot) = import_snapshot_live(ctx, child, true)? {
            snapshots.push(snapshot);
        }
    }
    let font_ctx = world(ctx)?.borrow().runtime.font_ctx.clone();
    let mut parsed = crate::Parsed::script(content_type, font_ctx);
    parsed.xml_document = xml_document;
    parsed.quirks_mode = quirks_mode;
    let root = parsed.document.base.root_node().id;
    for child in &snapshots {
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


/// Whether `name` is an HTML `template` element, whose children live in its
/// template contents fragment
/// (<https://html.spec.whatwg.org/multipage/scripting.html#the-template-element>).
fn is_html_template(name: &QualName) -> bool {
    name.ns == crate::js::world::html_namespace() && name.local.as_ref() == "template"
}

/// Deep-snapshots `children` for cross-document cloning.
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
    let node = doc.base.get_node(id.node)?;
    match &node.data {
        NodeData::Element(element) => {
            let name = element.name.clone();
            let attributes = element.attrs.iter().cloned().collect();
            // Template children live in the template contents fragment, not
            // as element children: snapshot from the contents
            // (<https://html.spec.whatwg.org/multipage/scripting.html#the-template-element>).
            let children = if deep {
                if is_html_template(&name) {
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
        NodeData::AnonymousBlock(_) => {
            let children = if deep {
                snapshot_children(doc, id.document, node.children.iter().copied())
            } else {
                Vec::new()
            };
            Some(ImportSnapshot::Fragment(children))
        }
        NodeData::Fragment { .. } => {
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

/// Snapshots `id` for cross-document cloning
/// (<https://dom.spec.whatwg.org/#concept-node-clone>).
pub(crate) fn import_snapshot_live(
    ctx: &Ctx<'_>,
    id: NodeId,
    deep: bool,
) -> Result<Option<ImportSnapshot>> {
    let kind = live_kind(ctx, id)?;
    match kind {
        LiveKind::None => Ok(None),
        LiveKind::Ready(snapshot) => Ok(Some(snapshot)),
        LiveKind::Fragment(children) => {
            let children = if deep {
                live_snapshot_children(ctx, children)?
            } else {
                Vec::new()
            };
            Ok(Some(ImportSnapshot::Fragment(children)))
        }
        LiveKind::Element {
            name,
            attributes,
            children,
        } => {
            let children = if deep {
                live_snapshot_children(ctx, children)?
            } else {
                Vec::new()
            };
            Ok(Some(ImportSnapshot::Element {
                name,
                attributes,
                children,
            }))
        }
    }
}

/// Classifies one live node for snapshotting: fragments snapshot as
/// fragments, elements with their name/attributes/children, text and
/// comments as character data.
fn live_kind(ctx: &Ctx<'_>, id: NodeId) -> Result<LiveKind> {
    let owner = world_for_node(ctx, id)?;
    let world = owner.borrow();
    let Some(parsed) = world.document(id) else {
        return Ok(LiveKind::None);
    };
    let doc = &parsed.document;
    if doc.is_fragment(id.node) {
        return Ok(LiveKind::Fragment(live_child_ids(doc, id)));
    }
    Ok(match doc.base.get_node(id.node).map(|node| &node.data) {
        Some(NodeData::Element(element)) => {
            // Template children live in the contents fragment.
            let children = if is_html_template(&element.name) {
                element
                    .template_contents
                    .map(|fragment| {
                        live_child_ids(
                            doc,
                            NodeId {
                                document: id.document,
                                node: fragment,
                            },
                        )
                    })
                    .unwrap_or_default()
            } else {
                live_child_ids(doc, id)
            };
            LiveKind::Element {
                name: element.name.clone(),
                attributes: element.attrs.iter().cloned().collect(),
                children,
            }
        }
        Some(NodeData::Text(_)) => {
            LiveKind::Ready(ImportSnapshot::Text(doc.character_data(id.node)))
        }
        Some(NodeData::Comment { .. }) => {
            LiveKind::Ready(ImportSnapshot::Comment(doc.character_data(id.node)))
        }
        Some(NodeData::Doctype {
            name,
            public_id,
            system_id,
        }) => LiveKind::Ready(ImportSnapshot::DocumentType {
            name: name.clone(),
            public_id: public_id.clone(),
            system_id: system_id.clone(),
        }),
        Some(NodeData::ProcessingInstruction { target, .. }) => {
            LiveKind::Ready(ImportSnapshot::ProcessingInstruction {
                target: target.clone(),
                data: doc.character_data(id.node),
            })
        }
        Some(NodeData::CDataSection { .. }) => {
            LiveKind::Ready(ImportSnapshot::CData(doc.character_data(id.node)))
        }
        Some(NodeData::AnonymousBlock(_)) => LiveKind::Fragment(live_child_ids(doc, id)),
        Some(NodeData::Fragment { .. }) => LiveKind::Fragment(live_child_ids(doc, id)),
        Some(NodeData::Document(_)) | None => LiveKind::None,
    })
}

enum LiveKind {
    None,
    Ready(ImportSnapshot),
    Fragment(Vec<NodeId>),
    Element {
        name: QualName,
        attributes: Vec<blitz_dom::Attribute>,
        children: Vec<NodeId>,
    },
}

/// The live children of `id` as cross-document handles.
fn live_child_ids(doc: &BlitzDocument, id: NodeId) -> Vec<NodeId> {
    doc.base
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
        .unwrap_or_default()
}

/// Deep-snapshots live `children` through their owning realms.
fn live_snapshot_children(ctx: &Ctx<'_>, children: Vec<NodeId>) -> Result<Vec<ImportSnapshot>> {
    let mut snapshots = Vec::new();
    for child in children {
        if let Some(snapshot) = import_snapshot_live(ctx, child, true)? {
            snapshots.push(snapshot);
        }
    }
    Ok(snapshots)
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
            let parent = if is_html_template(name) {
                doc.base
                    .mutate()
                    .ensure_template_contents(blitz_id)
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
            let blitz_id = doc.create_doctype(name.clone(), public_id.clone(), system_id.clone());
            Ok(NodeId {
                document,
                node: blitz_id,
            })
        }
        ImportSnapshot::ProcessingInstruction { target, data } => {
            let text = data.to_string_lossy().into_owned();
            let blitz_id = doc.create_processing_instruction(target.clone(), &text);
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
