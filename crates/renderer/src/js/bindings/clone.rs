//! Node adoption, cloning, and document import.

use super::{throw_dom, throw_dom_error, world, wrap_new_document};

use dom::{NodeId, NodeKind, QualName};

use rquickjs::{Ctx, Exception, Result, Value};

/// Same-document insert returns `node`. Cross-document insert is refused:
/// [adopt](https://dom.spec.whatwg.org/#concept-node-adopt) keeps the same
/// node, and `NodeId` is document-scoped until the handle can retarget.
pub(crate) fn adopt_across_documents(
    ctx: &Ctx<'_>,
    parent: NodeId,
    node: NodeId,
) -> Result<NodeId> {
    if node.document_id() == parent.document_id() {
        return Ok(node);
    }
    // https://dom.spec.whatwg.org/#concept-node-adopt
    Err(throw_dom(
        ctx,
        "HierarchyRequestError",
        "nodes belong to different documents",
    ))
}

/// [Clones](https://dom.spec.whatwg.org/#concept-node-clone) a document into a
/// new tree in this world. `Dom::clone_node` refuses the document node because
/// a document clone is a different document, not a node in the same arena.
pub(crate) fn clone_document<'js>(ctx: &Ctx<'js>, id: NodeId, deep: bool) -> Result<Value<'js>> {
    let world_rc = world(ctx)?;
    let (content_type, quirks_mode, children) = {
        let world = world_rc.borrow();
        let Some(parsed) = world.document(id) else {
            return Err(Exception::throw_type(ctx, "no document"));
        };
        let children = if deep {
            parsed
                .dom
                .children(id)
                .map(|kids| {
                    kids.copied()
                        .filter_map(|kid| import_snapshot(&parsed.dom, kid, true))
                        .collect()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        (parsed.content_type, parsed.quirks_mode, children)
    };
    let mut parsed = crate::Parsed::empty(content_type);
    parsed.quirks_mode = quirks_mode;
    let document = parsed.dom.document();
    for child in children {
        let child =
            materialize_import(&mut parsed.dom, &child).map_err(|err| throw_dom_error(ctx, err))?;
        parsed
            .dom
            .append(document, child)
            .map_err(|err| throw_dom_error(ctx, err))?;
    }
    wrap_new_document(ctx, parsed)
}

/// Owned snapshot of a subtree for cross-document `importNode`.
pub(crate) enum ImportSnapshot {
    Element {
        name: QualName,
        attributes: Vec<dom::Attribute>,
        children: Vec<ImportSnapshot>,
        template_contents: Option<Vec<ImportSnapshot>>,
    },
    Text(String),
    CData(String),
    ProcessingInstruction {
        target: String,
        data: String,
    },
    Comment(String),
    Doctype {
        name: String,
        public_id: String,
        system_id: String,
    },
    Fragment(Vec<ImportSnapshot>),
}

pub(crate) fn import_snapshot(dom: &dom::Dom, id: NodeId, deep: bool) -> Option<ImportSnapshot> {
    let children = |deep: bool| -> Vec<ImportSnapshot> {
        if !deep {
            return Vec::new();
        }
        dom.children(id)
            .map(|kids| {
                kids.copied()
                    .filter_map(|kid| import_snapshot(dom, kid, true))
                    .collect()
            })
            .unwrap_or_default()
    };
    match dom.kind(id)? {
        NodeKind::Element { name, attributes } => Some(ImportSnapshot::Element {
            name: name.clone(),
            attributes: attributes.clone(),
            children: children(deep),
            template_contents: dom.template_contents(id).map(|contents| {
                if deep {
                    dom.children(contents)
                        .map(|kids| {
                            kids.copied()
                                .filter_map(|kid| import_snapshot(dom, kid, true))
                                .collect()
                        })
                        .unwrap_or_default()
                } else {
                    Vec::new()
                }
            }),
        }),
        NodeKind::Text { data } => Some(ImportSnapshot::Text(data.clone())),
        NodeKind::CDataSection { data } => Some(ImportSnapshot::CData(data.clone())),
        NodeKind::ProcessingInstruction { target, data } => {
            Some(ImportSnapshot::ProcessingInstruction {
                target: target.clone(),
                data: data.clone(),
            })
        }
        NodeKind::Comment { data } => Some(ImportSnapshot::Comment(data.clone())),
        NodeKind::Doctype {
            name,
            public_id,
            system_id,
        } => Some(ImportSnapshot::Doctype {
            name: name.clone(),
            public_id: public_id.clone(),
            system_id: system_id.clone(),
        }),
        NodeKind::Fragment => Some(ImportSnapshot::Fragment(children(deep))),
        NodeKind::Document => None,
    }
}

pub(crate) fn materialize_import(
    dom: &mut dom::Dom,
    snapshot: &ImportSnapshot,
) -> std::result::Result<NodeId, dom::DomError> {
    match snapshot {
        ImportSnapshot::Element {
            name,
            attributes,
            children,
            template_contents,
        } => {
            let id = dom.create_element(name.clone(), attributes.clone());
            for child in children {
                let child = materialize_import(dom, child)?;
                dom.append(id, child)?;
            }
            if let Some(contents) = template_contents {
                let fragment = materialize_children(dom, contents)?;
                dom.set_template_contents(id, fragment)?;
            }
            Ok(id)
        }
        ImportSnapshot::Text(data) => Ok(dom.create_text(data.clone())),
        ImportSnapshot::CData(data) => Ok(dom.create_cdata_section(data.clone())),
        ImportSnapshot::ProcessingInstruction { target, data } => {
            Ok(dom.create_processing_instruction(target.clone(), data.clone()))
        }
        ImportSnapshot::Comment(data) => Ok(dom.create_comment(data.clone())),
        ImportSnapshot::Doctype {
            name,
            public_id,
            system_id,
        } => Ok(dom.create_doctype(name.clone(), public_id.clone(), system_id.clone())),
        ImportSnapshot::Fragment(children) => materialize_children(dom, children),
    }
}

/// Materializes `snapshots` into a fresh fragment in tree order.
pub(crate) fn materialize_children(
    dom: &mut dom::Dom,
    snapshots: &[ImportSnapshot],
) -> std::result::Result<NodeId, dom::DomError> {
    let fragment = dom.create_fragment();
    for snapshot in snapshots {
        let child = materialize_import(dom, snapshot)?;
        dom.append(fragment, child)?;
    }
    Ok(fragment)
}
