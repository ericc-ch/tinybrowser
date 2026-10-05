//! The single `Node` wrapper class and its members.

use super::{
    AttrArgument, CollectionKind, ImportSnapshot, JsImplementation, JsNamedNodeMap, JsTokenList,
    LegacyNullString, NodeContext, NodeOrString, OptString, Trace, WebIdlCodeUnits, WebIdlString,
    WebIdlUnsignedLong, adopt_across_documents, ancestor_chain, attached_attr_id, attr_owner,
    attr_state, attr_wrapper, attribute_local_name, attribute_value, blur_node, character_data,
    character_data_offset, child_value, clone_document, clone_within_document,
    convert_union_nodes_into_node, deref_weak, descendant_text, doctype_fields,
    document_base_url_string, document_is_html, document_is_html_content, document_url_string,
    dom_string, element_at_point, element_box, element_click, element_node_name,
    element_sibling_value, elements_by_tag, find_element_by_id, fixup_focus_after_removal,
    focus_node, host, host_node_id, import_snapshot, is_element, is_focusable, is_html_element,
    is_main_document, live_collection, main_document, make_weak, materialize_children,
    materialize_import, new_detached_attr, qualified_name, rect_object, remove_attribute_sync,
    required_node, root_of, schedule_mutation_delivery, select_error, set_attribute_node,
    set_attribute_sync, set_character_data, sibling, sibling_value, string_value, throw_dom,
    throw_dom_error, touch_attr, tree_order, valid_attribute_local_name, validate_and_extract,
    with_node_data, world, world_for_node, wrap_new_document, wrap_node,
};
use rquickjs::function::Rest;

use std::cell::RefCell;
use std::rc::Rc;

use blitz_dom::NodeData;
use markup5ever::{LocalName, Namespace, QualName};

use crate::dom_string::DomString;

use rquickjs::{Array, Class, Ctx, Exception, Function, Persistent, Result, Value};

use crate::js::events;
use crate::js::world::World;

use crate::js::world::{
    BlitzId, DocumentStreamCommand, Handle, JournalEntry, NodeId, NodeReference, Wrapper, attr,
    html_namespace, is_connected, svg_namespace,
};

use crate::ReadyState;

include!(concat!(env!("OUT_DIR"), "/Node.rs"));
include!(concat!(env!("OUT_DIR"), "/Document.rs"));
include!(concat!(env!("OUT_DIR"), "/DocumentFragment.rs"));
include!(concat!(env!("OUT_DIR"), "/Element.rs"));
include!(concat!(env!("OUT_DIR"), "/HTMLElement.rs"));
include!(concat!(env!("OUT_DIR"), "/SVGElement.rs"));
include!(concat!(env!("OUT_DIR"), "/MathMLElement.rs"));
include!(concat!(env!("OUT_DIR"), "/CharacterData.rs"));
include!(concat!(env!("OUT_DIR"), "/DocumentType.rs"));
include!(concat!(env!("OUT_DIR"), "/ProcessingInstruction.rs"));
include!(concat!(env!("OUT_DIR"), "/ParentNode.rs"));
include!(concat!(env!("OUT_DIR"), "/ChildNode.rs"));
include!(concat!(env!("OUT_DIR"), "/NonDocumentTypeChildNode.rs"));
include!(concat!(env!("OUT_DIR"), "/ElementCSSInlineStyle.rs"));
include!(concat!(env!("OUT_DIR"), "/ShadowRoot.rs"));
include!(concat!(env!("OUT_DIR"), "/HTMLFormElement.rs"));
include!(concat!(env!("OUT_DIR"), "/HTMLInputElement.rs"));
include!(concat!(env!("OUT_DIR"), "/HTMLTextAreaElement.rs"));
include!(concat!(env!("OUT_DIR"), "/HTMLSelectElement.rs"));
include!(concat!(env!("OUT_DIR"), "/HTMLOptionElement.rs"));
include!(concat!(env!("OUT_DIR"), "/HTMLButtonElement.rs"));
include!(concat!(env!("OUT_DIR"), "/HTMLFieldSetElement.rs"));
include!(concat!(env!("OUT_DIR"), "/HTMLOptGroupElement.rs"));
include!(concat!(env!("OUT_DIR"), "/HTMLIFrameElement.rs"));
include!(concat!(env!("OUT_DIR"), "/HTMLFrameElement.rs"));
include!(concat!(env!("OUT_DIR"), "/HTMLImageElement.rs"));
include!(concat!(env!("OUT_DIR"), "/HTMLHyperlinkElementUtils.rs"));
include!(concat!(env!("OUT_DIR"), "/HTMLBaseElement.rs"));
include!(concat!(env!("OUT_DIR"), "/HTMLLinkElement.rs"));
include!(concat!(env!("OUT_DIR"), "/HTMLMediaElement.rs"));
include!(concat!(env!("OUT_DIR"), "/HTMLEmbedElement.rs"));
include!(concat!(env!("OUT_DIR"), "/HTMLScriptElement.rs"));
include!(concat!(env!("OUT_DIR"), "/HTMLSourceElement.rs"));
include!(concat!(env!("OUT_DIR"), "/HTMLTrackElement.rs"));
include!(concat!(env!("OUT_DIR"), "/HTMLMetaElement.rs"));
include!(concat!(env!("OUT_DIR"), "/HTMLMapElement.rs"));
include!(concat!(env!("OUT_DIR"), "/HTMLObjectElement.rs"));
include!(concat!(env!("OUT_DIR"), "/HTMLOutputElement.rs"));
include!(concat!(env!("OUT_DIR"), "/HTMLParamElement.rs"));
include!(concat!(env!("OUT_DIR"), "/HTMLSlotElement.rs"));
include!(concat!(env!("OUT_DIR"), "/HTMLTemplateElement.rs"));

pub(super) fn install(ctx: &Ctx<'_>) -> Result<()> {
    node_generated::install(ctx)?;
    document_generated::install(ctx)?;
    document_fragment_generated::install(ctx)?;
    element_generated::install(ctx)?;
    html_element_generated::install(ctx)?;
    svg_element_generated::install(ctx)?;
    math_ml_element_generated::install(ctx)?;
    character_data_generated::install(ctx)?;
    document_type_generated::install(ctx)?;
    processing_instruction_generated::install(ctx)?;
    parent_node_generated::install(ctx)?;
    child_node_generated::install(ctx)?;
    non_document_type_child_node_generated::install(ctx)?;
    element_css_inline_style_generated::install(ctx)?;
    shadow_root_generated::install(ctx)?;
    html_form_element_generated::install(ctx)?;
    html_input_element_generated::install(ctx)?;
    html_text_area_element_generated::install(ctx)?;
    html_select_element_generated::install(ctx)?;
    html_option_element_generated::install(ctx)?;
    html_button_element_generated::install(ctx)?;
    html_field_set_element_generated::install(ctx)?;
    html_opt_group_element_generated::install(ctx)?;
    htmli_frame_element_generated::install(ctx)?;
    html_frame_element_generated::install(ctx)?;
    html_image_element_generated::install(ctx)?;
    html_hyperlink_element_utils_generated::install(ctx)?;
    html_base_element_generated::install(ctx)?;
    html_link_element_generated::install(ctx)?;
    html_media_element_generated::install(ctx)?;
    html_embed_element_generated::install(ctx)?;
    html_script_element_generated::install(ctx)?;
    html_source_element_generated::install(ctx)?;
    html_track_element_generated::install(ctx)?;
    html_meta_element_generated::install(ctx)?;
    html_map_element_generated::install(ctx)?;
    html_object_element_generated::install(ctx)?;
    html_output_element_generated::install(ctx)?;
    html_param_element_generated::install(ctx)?;
    html_slot_element_generated::install(ctx)?;
    html_template_element_generated::install(ctx)
}

fn insertion_tree_nodes(
    ctx: &Ctx<'_>,
    parent: NodeId,
    node: NodeReference,
    child: Option<NodeReference>,
) -> Result<(NodeId, Option<NodeId>)> {
    // https://dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity
    // https://dom.spec.whatwg.org/#concept-node-replace
    if matches!(child, Some(NodeReference::Attribute { .. })) {
        return Err(throw_dom(ctx, "NotFoundError", "attributes have no parent"));
    }
    let node = node.tree().ok_or_else(|| {
        throw_dom(
            ctx,
            "HierarchyRequestError",
            "attributes cannot be inserted",
        )
    })?;
    let owner = world_for_node(ctx, parent)?;
    let owner = owner.borrow();
    let parsed = owner
        .document(parent)
        .ok_or_else(|| Exception::throw_type(ctx, "stale parent"))?;
    let base = &parsed.document.base;
    // Only documents and elements can be parents. Fragment backings are
    // plain elements, so fragments are covered by the element case.
    // https://dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity
    let parent_is_document = base.root_node().id == parent.node;
    if !parent_is_document && !is_element(base, parent.node) {
        return Err(throw_dom(
            ctx,
            "HierarchyRequestError",
            "parent cannot have children",
        ));
    }
    // The reference child must already be parented here.
    let reference = match child {
        Some(NodeReference::Tree(id)) => Some(id),
        Some(NodeReference::Attribute { .. }) | None => None,
    };
    if let Some(reference) = reference {
        ensure_parented(ctx, base, parent, reference)?;
    }
    // The node itself must be live, resolved in its own document.
    let node_parsed = owner
        .document(node)
        .ok_or_else(|| Exception::throw_type(ctx, "stale node"))?;
    let node_base = &node_parsed.document.base;
    // A document (any document's root) can never be inserted.
    // https://dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity
    if node_base.root_node().id == node.node {
        return Err(throw_dom(
            ctx,
            "HierarchyRequestError",
            "a document cannot be inserted",
        ));
    }
    let node_is_fragment = node_parsed.document.is_fragment(node.node);
    let node_synthetic = node_parsed.document.synthetic_kind(node.node).cloned();
    let node_data = node_base
        .get_node(node.node)
        .map(|candidate| &candidate.data);
    let node_is_element = node_data.is_some_and(|data| data.downcast_element().is_some())
        && !matches!(
            node_synthetic,
            Some(crate::documents::SyntheticKind::Doctype { .. })
        );
    // Fragments, elements, text, comments, PIs, and CDATA sections insert.
    // Doctypes insert only under a document (checked below with the content
    // model). CDATA counts as text for the document content model.
    let node_is_text = node_data.is_some_and(|data| matches!(data, NodeData::Text(_)))
        || matches!(node_synthetic, Some(crate::documents::SyntheticKind::CData));
    let node_is_character_data =
        node_is_text || node_data.is_some_and(|data| matches!(data, NodeData::Comment { .. }));
    if !node_is_fragment && !node_is_element && !node_is_character_data {
        return Err(throw_dom(
            ctx,
            "HierarchyRequestError",
            "node cannot be inserted",
        ));
    }
    if !parent_is_document
        && matches!(
            node_synthetic,
            Some(crate::documents::SyntheticKind::Doctype { .. })
        )
    {
        return Err(throw_dom(
            ctx,
            "HierarchyRequestError",
            "doctype cannot be a child of an element",
        ));
    }
    // A node cannot be inserted into its own subtree.
    // https://dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity
    if node.document == parent.document {
        ensure_no_cycle(ctx, base, parent.node, node.node)?;
    }
    // The document content model: at most one element child, and no text
    // directly under the document.
    // https://dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity
    if parent_is_document {
        ensure_document_content_model(
            ctx,
            &parsed.document,
            &node_parsed.document,
            parent,
            node,
            node_is_fragment,
            node_is_element,
            node_is_text,
        )?;
    }
    Ok((node, reference))
}

/// The reference child of an insertion must already be parented at the
/// parent (<https://dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity>).
fn ensure_parented(
    ctx: &Ctx<'_>,
    base: &blitz_dom::BaseDocument,
    parent: NodeId,
    reference: NodeId,
) -> Result<()> {
    let parented = reference.document == parent.document
        && base
            .get_node(parent.node)
            .is_some_and(|candidate| candidate.children.contains(&reference.node));
    if parented {
        Ok(())
    } else {
        Err(throw_dom(
            ctx,
            "NotFoundError",
            "reference is not a child of parent",
        ))
    }
}

/// A node cannot be inserted into its own subtree
/// (<https://dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity>).
fn ensure_no_cycle(
    ctx: &Ctx<'_>,
    base: &blitz_dom::BaseDocument,
    parent: BlitzId,
    node: BlitzId,
) -> Result<()> {
    let mut cursor = Some(parent);
    while let Some(current) = cursor {
        if current == node {
            return Err(throw_dom(
                ctx,
                "HierarchyRequestError",
                "node cannot contain itself",
            ));
        }
        cursor = base
            .get_node(current)
            .and_then(|candidate| candidate.parent);
    }
    Ok(())
}

/// At most one doctype child of a document
/// (<https://dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity>).
fn ensure_single_doctype(
    ctx: &Ctx<'_>,
    doc: &crate::documents::BlitzDocument,
    node_doc: &crate::documents::BlitzDocument,
    fragment_children: &[BlitzId],
    parent: NodeId,
    node: NodeId,
    node_is_fragment: bool,
) -> Result<()> {
    let is_doctype =
        |doc: &crate::documents::BlitzDocument, id: BlitzId| {
            matches!(
                doc.synthetic_kind(id),
                Some(crate::documents::SyntheticKind::Doctype { .. })
            )
        };
    let inserted = usize::from(!node_is_fragment && is_doctype(node_doc, node.node))
        + if node_is_fragment {
            fragment_children
                .iter()
                .filter(|child| is_doctype(node_doc, **child))
                .count()
        } else {
            0
        };
    let existing = doc
        .base
        .get_node(parent.node)
        .map_or(0, |root| {
            root.children
                .iter()
                .filter(|child| {
                    (node.document != parent.document || **child != node.node)
                        && is_doctype(doc, **child)
                })
                .count()
        });
    if existing + inserted > 1 {
        return Err(throw_dom(
            ctx,
            "HierarchyRequestError",
            "a document can have only one doctype child",
        ));
    }
    Ok(())
}

/// The document content model for one insertion: at most one element child,
/// at most one doctype child, and no text directly under the document
/// (<https://dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity>).
#[expect(
    clippy::too_many_arguments,
    reason = "validity context is eight orthogonal inputs; a params struct would obscure the spec mapping"
)]
fn ensure_document_content_model(
    ctx: &Ctx<'_>,
    doc: &crate::documents::BlitzDocument,
    node_doc: &crate::documents::BlitzDocument,
    parent: NodeId,
    node: NodeId,
    node_is_fragment: bool,
    node_is_element: bool,
    node_is_text: bool,
) -> Result<()> {
    let base = &doc.base;
    let node_base = &node_doc.base;
    // CDATA sections count as text for the content model.
    let is_text = |doc: &crate::documents::BlitzDocument, id: BlitzId| {
        doc.base.get_node(id).is_some_and(|candidate| {
            matches!(candidate.data, NodeData::Text(_))
                || matches!(
                    doc.synthetic_kind(id),
                    Some(crate::documents::SyntheticKind::CData)
                )
        })
    };
    let fragment_children: Vec<BlitzId> = if node_is_fragment {
            node_base
                .get_node(node.node)
                .map(|fragment| fragment.children.iter().copied().collect())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        if !node_is_fragment && node_is_text {
            return Err(throw_dom(
                ctx,
                "HierarchyRequestError",
                "text cannot be a child of a document",
            ));
        }
        if node_is_fragment
            && fragment_children
                .iter()
                .any(|child| is_text(node_doc, *child))
        {
            return Err(throw_dom(
                ctx,
                "HierarchyRequestError",
                "text cannot be a child of a document",
            ));
        }
        let inserted_elements = if node_is_fragment {
            fragment_children
                .iter()
                .filter(|child| {
                    node_base
                        .get_node(**child)
                        .is_some_and(|candidate| candidate.data.downcast_element().is_some())
                })
                .count()
        } else {
            usize::from(node_is_element)
        };
        let existing_elements = base.get_node(parent.node).map_or(0, |root| {
            root.children
                .iter()
                .filter(|child| {
                    (node.document != parent.document || **child != node.node)
                        && base
                            .get_node(**child)
                            .is_some_and(|candidate| candidate.data.downcast_element().is_some())
                })
                .count()
        });
        if existing_elements + inserted_elements > 1 {
            return Err(throw_dom(
                ctx,
                "HierarchyRequestError",
                "a document can have only one element child",
            ));
        }
        // At most one doctype child; the node itself counts when it is one.
        ensure_single_doctype(
            ctx,
            doc,
            node_doc,
            &fragment_children,
            parent,
            node,
            node_is_fragment,
        )?;
    Ok(())
}

pub(super) fn compare_node_position(
    ctx: &Ctx<'_>,
    this: NodeReference,
    other: NodeReference,
) -> Result<u16> {
    // https://dom.spec.whatwg.org/#dom-node-comparedocumentposition
    const DISCONNECTED: u16 = 1;
    const PRECEDING: u16 = 2;
    const FOLLOWING: u16 = 4;
    const CONTAINS: u16 = 8;
    const CONTAINED_BY: u16 = 16;
    const IMPLEMENTATION_SPECIFIC: u16 = 32;
    if this == other {
        return Ok(0);
    }
    let node1 = match other {
        NodeReference::Tree(id) => Some(id),
        NodeReference::Attribute { scope, id } => attr_owner(ctx, scope, id),
    };
    let node2 = match this {
        NodeReference::Tree(id) => Some(id),
        NodeReference::Attribute { scope, id } => attr_owner(ctx, scope, id),
    };
    let disconnected =
        DISCONNECTED | IMPLEMENTATION_SPECIFIC | if other < this { PRECEDING } else { FOLLOWING };
    let (Some(node1), Some(node2)) = (node1, node2) else {
        return Ok(disconnected);
    };
    if node1.document != node2.document {
        return Ok(disconnected);
    }
    let owner = world_for_node(ctx, node1)?;
    let owner = owner.borrow();
    let parsed = owner
        .document(node1)
        .ok_or_else(|| Exception::throw_type(ctx, "stale node"))?;
    let base = &parsed.document.base;
    let document = parsed.id;
    if let (
        NodeReference::Attribute {
            scope: scope1,
            id: attr1,
        },
        NodeReference::Attribute {
            scope: scope2,
            id: attr2,
        },
    ) = (other, this)
        && node1 == node2
    {
        let a = attr_state(ctx, scope1, attr1)?;
        let b = attr_state(ctx, scope2, attr2)?;
        if let Some(element) = base
            .get_node(node1.node)
            .and_then(|candidate| candidate.data.downcast_element())
        {
            for attribute in element.attrs.iter() {
                if attribute.name.ns.as_ref() == a.namespace
                    && attribute.name.local.as_ref() == a.local
                {
                    return Ok(IMPLEMENTATION_SPECIFIC | PRECEDING);
                }
                if attribute.name.ns.as_ref() == b.namespace
                    && attribute.name.local.as_ref() == b.local
                {
                    return Ok(IMPLEMENTATION_SPECIFIC | FOLLOWING);
                }
            }
        }
    }
    if root_of(base, node1.node) != root_of(base, node2.node) {
        return Ok(disconnected);
    }
    let attr1 = matches!(other, NodeReference::Attribute { .. });
    let attr2 = matches!(this, NodeReference::Attribute { .. });
    let node1_ancestor = ancestor_chain(base, document, node2.node).contains(&node1);
    let node2_ancestor = ancestor_chain(base, document, node1.node).contains(&node2);
    if (node1_ancestor && !attr1) || (node1 == node2 && attr2) {
        return Ok(CONTAINS | PRECEDING);
    }
    if (node2_ancestor && !attr2) || (node1 == node2 && attr1) {
        return Ok(CONTAINED_BY | FOLLOWING);
    }
    Ok(
        if tree_order(base, document, node1.node, node2.node) == std::cmp::Ordering::Less {
            PRECEDING
        } else {
            FOLLOWING
        },
    )
}

/// Backs the constructible platform interfaces (`new Text(…)`); the current
/// realm's main document owns the new node
/// (<https://dom.spec.whatwg.org/#dom-text-text>).
#[allow(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes Ctx and Rest by value"
)]
pub(crate) fn construct_node<'js>(
    ctx: Ctx<'js>,
    name: WebIdlString,
    args: Rest<Value<'js>>,
) -> Result<Value<'js>> {
    let first = args.0.into_iter().next();
    match name.0.as_str() {
        "Text" => {
            let data = constructor_units(&ctx, first)?;
            let document = main_document(&ctx)?;
            create_node(&ctx, document, |parsed| {
                parsed
                    .document
                    .base
                    .mutate()
                    .create_text_node(&data.to_string_lossy())
            })
        }
        "Comment" => {
            let data = constructor_units(&ctx, first)?;
            let document = main_document(&ctx)?;
            create_node(&ctx, document, |parsed| {
                parsed
                    .document
                    .base
                    .mutate()
                    .create_comment_node(&data.to_string_lossy())
            })
        }
        "DocumentFragment" => {
            let document = main_document(&ctx)?;
            create_node(&ctx, document, |parsed| parsed.document.create_fragment())
        }
        "Document" | "XMLDocument" => {
            // The `Document` constructor creates an XML document
            // (<https://dom.spec.whatwg.org/#dom-document-document>).
            wrap_new_document(&ctx, crate::Parsed::empty("application/xml"))
        }
        "HTMLElement" => {
            // https://html.spec.whatwg.org/multipage/custom-elements.html#html-element-constructors
            // During upgrade, the construction stack supplies the existing
            // element rather than allocating a second wrapper.
            let id = world(&ctx)?
                .borrow_mut()
                .custom_construction
                .pop()
                .ok_or_else(|| Exception::throw_type(&ctx, "Illegal constructor"))?;
            wrap_node(&ctx, id)
        }
        other => Err(Exception::throw_type(
            &ctx,
            &format!("{other} is not a constructor"),
        )),
    }
}

pub(super) fn install_custom_construction(ctx: &Ctx<'_>) -> Result<()> {
    let globals = crate::js::bridge::object(ctx)?;
    globals.set(
        "__tbPushCustomConstruction",
        rquickjs::prelude::Func::from(push_custom_construction),
    )?;
    globals.set(
        "__tbDiscardCustomConstruction",
        rquickjs::prelude::Func::from(discard_custom_construction),
    )?;
    Ok(())
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes Ctx and Value by value"
)]
fn push_custom_construction<'js>(ctx: Ctx<'js>, value: Value<'js>) -> Result<()> {
    let id = host_node_id(&ctx, &value)
        .ok_or_else(|| Exception::throw_type(&ctx, "custom element candidate is not an element"))?;
    let is_element = with_node_data(&ctx, id, |data| matches!(data, Some(NodeData::Element(_))))?;
    if !is_element {
        return Err(Exception::throw_type(
            &ctx,
            "custom element candidate is not an element",
        ));
    }
    world(&ctx)?.borrow_mut().custom_construction.push(id);
    Ok(())
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes Ctx and Value by value"
)]
fn discard_custom_construction<'js>(ctx: Ctx<'js>, value: Value<'js>) -> Result<()> {
    let Some(id) = host_node_id(&ctx, &value) else {
        return Ok(());
    };
    let world = world(&ctx)?;
    let mut world = world.borrow_mut();
    if world.custom_construction.last() == Some(&id) {
        world.custom_construction.pop();
    }
    Ok(())
}

/// Constructor `DOMString` that keeps every UTF-16 code unit, for the
/// character-data constructors (`new Text(…)`).
fn constructor_units<'js>(
    ctx: &Ctx<'js>,
    value: Option<Value<'js>>,
) -> Result<crate::dom_string::DomString> {
    match value {
        None => Ok(crate::dom_string::DomString::default()),
        Some(value) if value.is_undefined() => Ok(crate::dom_string::DomString::default()),
        Some(value) => Ok(crate::dom_string::DomString::from_utf16(
            super::webidl_to_units(ctx, value)?,
        )),
    }
}

/// Creates a detached node in `document`'s tree and wraps it. Detached
/// creation records no mutation: no observer can watch a node with no parent.
fn create_node<'js>(
    ctx: &Ctx<'js>,
    document: NodeId,
    make: impl FnOnce(&mut crate::Parsed) -> BlitzId,
) -> Result<Value<'js>> {
    let world = world(ctx)?;
    let world = world.borrow();
    let Some(mut parsed) = world.document_mut(document) else {
        return Err(Exception::throw_type(ctx, "no document"));
    };
    let store = parsed.id;
    let blitz = make(&mut parsed);
    let id = NodeId {
        document: store,
        node: blitz,
    };
    drop(parsed);
    drop(world);
    wrap_node(ctx, id)
}

// https://dom.spec.whatwg.org/#dom-document-createelement
fn create_html_element<'js>(ctx: &Ctx<'js>, document: NodeId, tag: &str) -> Result<Value<'js>> {
    // https://dom.spec.whatwg.org/#dom-document-createelement: validate, then
    // lowercase for an HTML document.
    if !valid_element_local_name(tag) {
        return Err(throw_dom(
            ctx,
            "InvalidCharacterError",
            "tag name is not a valid element local name",
        ));
    }
    // XHTML documents use the HTML namespace but keep case; only text/html
    // is an "HTML document" for lowercasing
    // (<https://dom.spec.whatwg.org/#internal-createelementns-steps>).
    let namespace = if document_is_html(ctx, document) {
        html_namespace()
    } else {
        Namespace::from("")
    };
    let local = if document_is_html_content(ctx, document) {
        tag.to_ascii_lowercase()
    } else {
        tag.to_owned()
    };
    let name = QualName::new(None, namespace, LocalName::from(local));
    create_element_named(ctx, document, name)
}

fn create_element_named<'js>(
    ctx: &Ctx<'js>,
    document: NodeId,
    name: QualName,
) -> Result<Value<'js>> {
    // Known gap: Blitz has no template contents, so `<template>` children
    // live as ordinary element children.
    create_node(ctx, document, |parsed| {
        parsed
            .document
            .base
            .mutate()
            .create_element(name, Vec::new())
    })
}

/// [Valid element local name](https://dom.spec.whatwg.org/#valid-element-local-name).
fn valid_element_local_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if first.is_ascii_alphabetic() {
        return !name
            .chars()
            .any(|c| matches!(c, '\t' | '\n' | '\u{c}' | '\r' | ' ' | '\0' | '/' | '>'));
    }
    if first != ':' && first != '_' && u32::from(first) < 0x80 {
        return false;
    }
    name.chars().all(|c| {
        c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | ':' | '_') || u32::from(c) >= 0x80
    })
}

#[derive(Trace, rquickjs::JsLifetime)]
#[rquickjs::class(rename = "Node")]
pub(crate) struct JsNode {
    pub(crate) handle: Handle,
}

impl super::host::SharedClass for JsNode {
    // https://webidl.spec.whatwg.org/#es-attributes
    // https://webidl.spec.whatwg.org/#es-operations
    fn require_interface(&self, ctx: &Ctx<'_>, interface: &str) -> Result<()> {
        super::host::require_node_interface(ctx, self.handle.0, interface)
    }
}

impl JsNode {
    pub(crate) fn node_id(&self) -> NodeId {
        self.handle.0
    }
}

/// The context string the HTML fragment parser expects for an element with
/// this qualified name: html5lib's `svg `/`math ` prefixes for foreign
/// namespaces
/// (<https://html.spec.whatwg.org/multipage/parsing.html#html-fragment-parsing-algorithm>).
fn html_fragment_context(name: &QualName) -> String {
    if name.ns == svg_namespace() {
        format!("svg {}", name.local)
    } else if name.ns.as_ref() == "http://www.w3.org/1998/Math/MathML" {
        format!("math {}", name.local)
    } else {
        name.local.to_string()
    }
}

/// Places fragment children for the non-`beforebegin` `insertAdjacentHTML`
/// positions: `afterbegin` prepends, `afterend` appends after the target,
/// `beforeend` appends inside.
fn place_adjacent_rest(
    ctx: &Ctx<'_>,
    parsed: &mut crate::Parsed,
    position: &str,
    target: NodeId,
    moved: Vec<NodeId>,
) -> Result<()> {
    match position {
        "afterbegin" => {
            let reference = parsed
                .document
                .base
                .get_node(target.node)
                .and_then(|node| node.children.first().copied())
                .map(|node| NodeId {
                    document: target.document,
                    node,
                });
            for child in moved {
                place_journaled(parsed, target, child, reference);
            }
        }
        "afterend" => {
            let parent = parsed
                .document
                .base
                .get_node(target.node)
                .and_then(|node| node.parent)
                .map(|node| NodeId {
                    document: target.document,
                    node,
                })
                .ok_or_else(|| {
                    throw_dom(ctx, "NoModificationAllowedError", "element has no parent")
                })?;
            let mut reference =
                sibling(&parsed.document.base, target.node, true).map(|next| NodeId {
                    document: target.document,
                    node: next,
                });
            for child in moved {
                place_journaled(parsed, parent, child, reference);
                reference = Some(child);
            }
        }
        _ => {
            for child in moved {
                place_journaled(parsed, target, child, None);
            }
        }
    }
    Ok(())
}

/// The base URL a fragment scratch document parses under: the live
/// document's base, so eager subresource loads resolve instead of panicking
/// in Blitz. Falls back closed when the document is gone.
fn fragment_base_url(ctx: &Ctx<'_>, id: NodeId) -> String {
    world(ctx)
        .ok()
        .and_then(|world| {
            world.borrow().document(id).map(|parsed| {
                parsed.document.base.base_url().as_str().to_owned()
            })
        })
        .unwrap_or_else(|| "http://invalid/".to_owned())
}

/// Parses `markup` as an HTML fragment in `context` and snapshots the
/// resulting nodes for insertion into a document.
fn parse_html_fragment_snapshots(
    markup: &str,
    context: &str,
    base_url: &str,
) -> Vec<ImportSnapshot> {
    // The fragment parses into a scratch context element; only the parsed
    // children are snapshotted for insertion into a live document
    // (<https://html.spec.whatwg.org/multipage/parsing.html#html-fragment-parsing-algorithm>).
    // The scratch document carries the live base URL so eager subresource
    // loads resolve instead of panicking in Blitz.
    let context_name = fragment_context_name(context);
    let mut base = blitz_dom::BaseDocument::new(blitz_dom::DocumentConfig {
        base_url: Some(base_url.to_owned()),
        ..blitz_dom::DocumentConfig::default()
    });
    let context_id = {
        let mut mutator = base.mutate();
        let context_id = mutator.create_element(context_name, Vec::new());
        blitz_html::DocumentHtmlParser::parse_inner_html_into_mutator(
            &mut mutator,
            context_id,
            markup,
        );
        context_id
    };
    let scratch = crate::documents::BlitzDocument::from_base(base);
    let children: Vec<BlitzId> = scratch
        .base
        .get_node(context_id)
        .map(|node| node.children.iter().copied().collect())
        .unwrap_or_default();
    children
        .into_iter()
        .filter_map(|child| {
            import_snapshot(
                &scratch,
                NodeId {
                    document: 0,
                    node: child,
                },
                true,
            )
        })
        .collect()
}

/// The context element name for fragment parsing: html5lib's `svg `/`math `
/// prefixes select the foreign namespace.
fn fragment_context_name(spec: &str) -> QualName {
    const SVG: &str = "http://www.w3.org/2000/svg";
    const MATHML: &str = "http://www.w3.org/1998/Math/MathML";
    if let Some(local) = spec.strip_prefix("svg ") {
        QualName::new(None, Namespace::from(SVG), LocalName::from(local))
    } else if let Some(local) = spec.strip_prefix("math ") {
        QualName::new(None, Namespace::from(MATHML), LocalName::from(local))
    } else {
        QualName::new(None, html_namespace(), LocalName::from(spec))
    }
}

/// The current `body`/`frameset` child of the document element owning
/// `root`, if any.
fn current_body_child(
    ctx: &Ctx<'_>,
    world_rc: &Rc<RefCell<World>>,
    root: NodeId,
) -> Result<Option<NodeId>> {
    let world = world_rc.borrow();
    let Some(parsed) = world.document(root) else {
        return Err(Exception::throw_type(ctx, "stale node"));
    };
    let base = &parsed.document.base;
    Ok(base.get_node(root.node).and_then(|candidate| {
        candidate.children.iter().find_map(|child| {
            let node = base.get_node(*child)?;
            match &node.data {
                NodeData::Element(element) | NodeData::AnonymousBlock(element) => {
                    (element.name.ns == html_namespace()
                        && (element.name.local.as_ref() == "body"
                            || element.name.local.as_ref() == "frameset"))
                        .then_some(NodeId {
                            document: parsed.id,
                            node: *child,
                        })
                }
                _ => None,
            }
        })
    }))
}

/// The first node matching `selector` under `parsed`'s document root.
fn document_first(parsed: &crate::Parsed, selector: &str) -> Option<NodeId> {
    let root = parsed.document.base.root_node().id;
    parsed
        .document
        .base
        .query_selector_in(root, selector)
        .ok()
        .flatten()
        .map(|node| NodeId {
            document: parsed.id,
            node,
        })
}

/// The first child of `parsed`'s document root satisfying `want`.
fn document_first_child(parsed: &crate::Parsed, want: fn(&NodeData) -> bool) -> Option<NodeId> {
    let base = &parsed.document.base;
    base.get_node(base.root_node().id).and_then(|root| {
        root.children.iter().find_map(|child| {
            let node = base.get_node(*child)?;
            want(&node.data).then_some(NodeId {
                document: parsed.id,
                node: *child,
            })
        })
    })
}

/// Wraps the first node `find` selects in `id`'s document, or `null`.
fn document_value<'js>(
    ctx: &Ctx<'js>,
    id: NodeId,
    find: impl FnOnce(&crate::Parsed) -> Option<NodeId>,
) -> Result<Value<'js>> {
    let world = world(ctx)?;
    let found = world.borrow().document(id).and_then(|parsed| find(&parsed));
    child_value(ctx, found)
}

/// Snapshots `id` for cross-document moves, preserving fragment backings as
/// fragments. `import_snapshot` reads a backing as its plain backing element;
/// fragments must snapshot as their children instead, or adoption would
/// insert a stray `div`.
fn snapshot_for_adopt(
    doc: &crate::documents::BlitzDocument,
    id: NodeId,
    deep: bool,
) -> Option<ImportSnapshot> {
    if doc.is_fragment(id.node) {
        if !deep {
            return Some(ImportSnapshot::Fragment(Vec::new()));
        }
        let children: Vec<BlitzId> = doc
            .base
            .get_node(id.node)
            .map(|backing| backing.children.iter().copied().collect())
            .unwrap_or_default();
        let children = children
            .into_iter()
            .filter_map(|child| {
                import_snapshot(
                    doc,
                    NodeId {
                        document: id.document,
                        node: child,
                    },
                    true,
                )
            })
            .collect();
        return Some(ImportSnapshot::Fragment(children));
    }
    import_snapshot(doc, id, deep)
}

/// Same-document insert returns `node`. A node from another document adopts
/// into the parent's document, preserving fragment backings as fragments
/// ([adopt](https://dom.spec.whatwg.org/#concept-node-adopt)).
fn adopt_node(ctx: &Ctx<'_>, parent: NodeId, node: NodeId) -> Result<NodeId> {
    if node.document == parent.document {
        return Ok(node);
    }
    let source_is_fragment = {
        let source_world = world_for_node(ctx, node)?;
        let source = source_world.borrow();
        let Some(parsed) = source.document(node) else {
            return Err(Exception::throw_type(ctx, "no document"));
        };
        parsed.document.is_fragment(node.node)
    };
    if !source_is_fragment {
        return adopt_across_documents(ctx, parent, node);
    }
    let source_world = world_for_node(ctx, node)?;
    let snapshots = {
        let source = source_world.borrow();
        let Some(parsed) = source.document(node) else {
            return Err(Exception::throw_type(ctx, "no document"));
        };
        let children: Vec<BlitzId> = parsed
            .document
            .base
            .get_node(node.node)
            .map(|backing| backing.children.iter().copied().collect())
            .unwrap_or_default();
        children
            .into_iter()
            .filter_map(|child| {
                import_snapshot(
                    &parsed.document,
                    NodeId {
                        document: node.document,
                        node: child,
                    },
                    true,
                )
            })
            .collect::<Vec<_>>()
    };
    {
        let source = source_world.borrow();
        let Some(mut parsed) = source.document_mut(node) else {
            return Err(Exception::throw_type(ctx, "no document"));
        };
        unlink_journaled(&mut parsed, node);
    }
    let target_world = world_for_node(ctx, parent)?;
    let target = target_world.borrow();
    let Some(mut parsed) = target.document_mut(parent) else {
        return Err(Exception::throw_type(ctx, "no document"));
    };
    let mut fresh = materialize_children(&mut parsed.document, &snapshots)
        .map_err(|err| throw_dom_error(ctx, err))?;
    // `materialize_children` builds with a placeholder document id; remap to
    // the target document before insertion.
    fresh.document = parent.document;
    Ok(fresh)
}

/// Siblings around `child` within `parent`, for the mutation journal.
fn siblings_around(
    base: &blitz_dom::BaseDocument,
    document: u32,
    parent: BlitzId,
    child: BlitzId,
) -> (Option<NodeId>, Option<NodeId>) {
    let mut previous = None;
    let mut next = None;
    if let Some(node) = base.get_node(parent) {
        let mut seen = false;
        for &kid in &node.children {
            if kid == child {
                seen = true;
                continue;
            }
            let id = NodeId {
                document,
                node: kid,
            };
            if seen && next.is_none() {
                next = Some(id);
                break;
            }
            if !seen {
                previous = Some(id);
            }
        }
    }
    (previous, next)
}

/// Records the removal of an already-parented `target`, then detaches it. A
/// node with no parent is already detached; that is a no-op. The removal
/// keeps its own `childList` record, so a move reads as removed plus added
/// (<https://dom.spec.whatwg.org/#concept-node-remove>).
fn unlink_journaled(parsed: &mut crate::Parsed, target: NodeId) {
    let document = target.document;
    let (parent, previous, next) = {
        let base = &parsed.document.base;
        let parent = base.get_node(target.node).and_then(|node| node.parent);
        let (previous, next) = parent.map_or((None, None), |parent| {
            siblings_around(base, document, parent, target.node)
        });
        (parent, previous, next)
    };
    let Some(parent) = parent else {
        return;
    };
    parsed.document.base.mutate().remove_node(target.node);
    parsed.document.record(JournalEntry::ChildList {
        target: NodeId {
            document,
            node: parent,
        },
        added: Vec::new(),
        removed: vec![target],
        previous,
        next,
    });
}

/// Inserts the already-adopted `child` into `parent` before `reference` (or
/// at the end for `None`), recording one `childList` addition. Blitz detaches
/// from the old parent during the insert itself, so callers run
/// `unlink_journaled` first and the removal keeps its own record.
fn place_journaled(
    parsed: &mut crate::Parsed,
    parent: NodeId,
    child: NodeId,
    reference: Option<NodeId>,
) {
    let previous = match reference {
        Some(reference) => {
            siblings_around(
                &parsed.document.base,
                parent.document,
                parent.node,
                reference.node,
            )
            .0
        }
        None => parsed
            .document
            .base
            .get_node(parent.node)
            .and_then(|node| node.children.last().copied())
            .map(|node| NodeId {
                document: parent.document,
                node,
            }),
    };
    match reference {
        Some(reference) => parsed
            .document
            .base
            .mutate()
            .insert_nodes_before(reference.node, &[child.node]),
        None => parsed
            .document
            .base
            .mutate()
            .append_children(parent.node, &[child.node]),
    }
    parsed.document.record(JournalEntry::ChildList {
        target: parent,
        added: vec![child],
        removed: Vec::new(),
        previous,
        next: reference,
    });
}

/// Moves one already-adopted node into `parent`, expanding a fragment into
/// its children. Inserting beside itself stays put
/// (<https://dom.spec.whatwg.org/#concept-node-pre-insert>).
fn insert_parsed(
    parsed: &mut crate::Parsed,
    parent: NodeId,
    node: NodeId,
    mut reference: Option<NodeId>,
) {
    if parsed.document.is_fragment(node.node) {
        splice_parsed(parsed, parent, node, reference);
        return;
    }
    if reference == Some(node) {
        reference = sibling(&parsed.document.base, node.node, true).map(|next| NodeId {
            document: node.document,
            node: next,
        });
    }
    unlink_journaled(parsed, node);
    place_journaled(parsed, parent, node, reference);
}

/// Splices an adopted fragment's children into `parent` before `reference`:
/// one removal record on the fragment plus one addition record on the parent
/// (<https://dom.spec.whatwg.org/#concept-node-insert>).
fn splice_parsed(
    parsed: &mut crate::Parsed,
    parent: NodeId,
    fragment: NodeId,
    reference: Option<NodeId>,
) {
    let moved: Vec<NodeId> = parsed
        .document
        .base
        .get_node(fragment.node)
        .map(|node| {
            node.children
                .iter()
                .map(|child| NodeId {
                    document: fragment.document,
                    node: *child,
                })
                .collect()
        })
        .unwrap_or_default();
    if moved.is_empty() {
        return;
    }
    parsed.document.record(JournalEntry::ChildList {
        target: fragment,
        added: Vec::new(),
        removed: moved.clone(),
        previous: None,
        next: None,
    });
    let previous = match reference {
        Some(reference) => {
            siblings_around(
                &parsed.document.base,
                parent.document,
                parent.node,
                reference.node,
            )
            .0
        }
        None => parsed
            .document
            .base
            .get_node(parent.node)
            .and_then(|node| node.children.last().copied())
            .map(|node| NodeId {
                document: parent.document,
                node,
            }),
    };
    for child in &moved {
        match reference {
            Some(reference) => parsed
                .document
                .base
                .mutate()
                .insert_nodes_before(reference.node, &[child.node]),
            None => parsed
                .document
                .base
                .mutate()
                .append_children(parent.node, &[child.node]),
        }
    }
    parsed.document.record(JournalEntry::ChildList {
        target: parent,
        added: moved,
        removed: Vec::new(),
        previous,
        next: reference,
    });
}

/// Replaces `old` with the already-adopted `node` (or its children for a
/// fragment): the node's own unlink keeps a removal record when it moves,
/// then one combined record carries the replacement
/// (<https://dom.spec.whatwg.org/#concept-node-replace>). Returns the nodes
/// the parent gained, for the insertion fixups.
fn replace_parsed(
    parsed: &mut crate::Parsed,
    parent: NodeId,
    node: NodeId,
    old: NodeId,
) -> Vec<NodeId> {
    if node == old {
        let reference = sibling(&parsed.document.base, old.node, true).map(|next| NodeId {
            document: old.document,
            node: next,
        });
        unlink_journaled(parsed, node);
        place_journaled(parsed, parent, node, reference);
        return vec![node];
    }
    let (previous, next) = siblings_around(
        &parsed.document.base,
        parent.document,
        parent.node,
        old.node,
    );
    let (added, fragment) = if parsed.document.is_fragment(node.node) {
        let moved: Vec<NodeId> = parsed
            .document
            .base
            .get_node(node.node)
            .map(|backing| {
                backing
                    .children
                    .iter()
                    .map(|child| NodeId {
                        document: node.document,
                        node: *child,
                    })
                    .collect()
            })
            .unwrap_or_default();
        (moved, true)
    } else {
        (vec![node], false)
    };
    if fragment {
        parsed.document.record(JournalEntry::ChildList {
            target: node,
            added: Vec::new(),
            removed: added.clone(),
            previous: None,
            next: None,
        });
    } else {
        unlink_journaled(parsed, node);
    }
    for child in &added {
        parsed
            .document
            .base
            .mutate()
            .insert_nodes_before(old.node, &[child.node]);
    }
    parsed.document.base.mutate().remove_node(old.node);
    parsed.document.record(JournalEntry::ChildList {
        target: parent,
        added: added.clone(),
        removed: vec![old],
        previous,
        next,
    });
    added
}

/// Replaces every child of `parent` with `added`: standing children detach
/// silently and one record carries the swap. Replacing with the same
/// contents is silent
/// (<https://dom.spec.whatwg.org/#concept-node-replace-all>).
fn replace_all_journaled(parsed: &mut crate::Parsed, parent: NodeId, added: Vec<NodeId>) {
    let removed: Vec<NodeId> = parsed
        .document
        .base
        .get_node(parent.node)
        .map(|node| {
            node.children
                .iter()
                .map(|child| NodeId {
                    document: parent.document,
                    node: *child,
                })
                .collect()
        })
        .unwrap_or_default();
    for kid in &removed {
        parsed.document.base.mutate().remove_node(kid.node);
    }
    for child in &added {
        parsed
            .document
            .base
            .mutate()
            .append_children(parent.node, &[child.node]);
    }
    if !added.is_empty() || !removed.is_empty() {
        parsed.document.record(JournalEntry::ChildList {
            target: parent,
            added,
            removed,
            previous: None,
            next: None,
        });
    }
}

/// Inserts the adopted `node` into `parent` before `reference`, expanding
/// fragments, journaling the move, and running the insertion fixups. The
/// caller owns validity and adoption.
fn insert_tree_node(
    ctx: &Ctx<'_>,
    parent: NodeId,
    node: NodeId,
    reference: Option<NodeId>,
) -> Result<()> {
    let world_rc = world(ctx)?;
    let moved: Vec<NodeId> = {
        let world = world_rc.borrow();
        let Some(parsed) = world.document(parent) else {
            return Err(Exception::throw_type(ctx, "no document"));
        };
        if parsed.document.is_fragment(node.node) {
            parsed
                .document
                .base
                .get_node(node.node)
                .map(|backing| {
                    backing
                        .children
                        .iter()
                        .map(|child| NodeId {
                            document: node.document,
                            node: *child,
                        })
                        .collect()
                })
                .unwrap_or_default()
        } else {
            vec![node]
        }
    };
    {
        let world = world_rc.borrow();
        let Some(mut parsed) = world.document_mut(parent) else {
            return Err(Exception::throw_type(ctx, "no document"));
        };
        insert_parsed(&mut parsed, parent, node, reference);
    }
    for id in &moved {
        fixup_option_on_insert(ctx, *id)?;
        fixup_radio_on_insert(ctx, *id)?;
    }
    schedule_mutation_delivery(ctx)
}

/// An inserted selected option clears a single-select owner's other options
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-selected>).
/// Blitz keeps selectedness as the `selected` content attribute, so the rule
/// rewrites attributes instead of slots.
fn fixup_option_on_insert(ctx: &Ctx<'_>, id: NodeId) -> Result<()> {
    let world_rc = world(ctx)?;
    let siblings = {
        let world = world_rc.borrow();
        let Some(parsed) = world.document(id) else {
            return Ok(());
        };
        let base = &parsed.document.base;
        let Some(element) = base
            .get_node(id.node)
            .and_then(|node| node.data.downcast_element())
        else {
            return Ok(());
        };
        if element.name.ns != html_namespace() || element.name.local.as_ref() != "option" {
            return Ok(());
        }
        if attr(base, id.node, "selected").is_none() {
            return Ok(());
        }
        let Some(owner) = option_select_owner(base, id.document, id.node) else {
            return Ok(());
        };
        if attr(base, owner.node, "multiple").is_some() {
            return Ok(());
        }
        select_options_in(base, id.document, owner.node)
            .into_iter()
            .filter(|option| *option != id && attr(base, option.node, "selected").is_some())
            .collect::<Vec<_>>()
    };
    for option in siblings {
        remove_attribute_sync(ctx, option, "", "selected", false)?;
    }
    Ok(())
}

/// An inserted checked radio unchecks the other radios it joins.
fn fixup_radio_on_insert(ctx: &Ctx<'_>, id: NodeId) -> Result<()> {
    let world_rc = world(ctx)?;
    let group = {
        let world = world_rc.borrow();
        let Some(parsed) = world.document(id) else {
            return Ok(());
        };
        let base = &parsed.document.base;
        let Some(element) = base
            .get_node(id.node)
            .and_then(|node| node.data.downcast_element())
        else {
            return Ok(());
        };
        if element.name.ns != html_namespace() || element.name.local.as_ref() != "input" {
            return Ok(());
        }
        if attr(base, id.node, "type")
            .unwrap_or("text")
            .trim()
            .eq_ignore_ascii_case("radio")
            && attr(base, id.node, "checked").is_some()
        {
            let name = attr(base, id.node, "name").unwrap_or_default().to_owned();
            radio_group_members(base, id.document, &name, id.node)
        } else {
            Vec::new()
        }
    };
    for member in group {
        remove_attribute_sync(ctx, member, "", "checked", false)?;
    }
    Ok(())
}

/// Radios sharing a `name` in the same document, in tree order, excluding
/// `except`. Blitz keeps no form-owner model, so the group spans the document
/// instead of the form (a known cutover gap).
fn radio_group_members(
    base: &blitz_dom::BaseDocument,
    document: u32,
    name: &str,
    except: BlitzId,
) -> Vec<NodeId> {
    let mut group = Vec::new();
    let mut stack = vec![base.root_node().id];
    while let Some(id) = stack.pop() {
        let Some(node) = base.get_node(id) else {
            continue;
        };
        if id != except
            && node.data.downcast_element().is_some_and(|element| {
                element.name.ns == html_namespace()
                    && element.name.local.as_ref() == "input"
                    && attr(base, id, "type")
                        .unwrap_or("text")
                        .trim()
                        .eq_ignore_ascii_case("radio")
                    && attr(base, id, "name").unwrap_or_default() == name
            })
        {
            group.push(NodeId { document, node: id });
        }
        stack.extend(node.children.iter().rev().copied());
    }
    group.sort_by(|a, b| tree_order(base, document, a.node, b.node));
    group
}

/// The `option` elements under `select` in tree order. Blitz keeps no form
/// model, so a select's options are the descendant `option` elements.
fn select_options_in(
    base: &blitz_dom::BaseDocument,
    document: u32,
    select: BlitzId,
) -> Vec<NodeId> {
    let mut options = Vec::new();
    let mut stack: Vec<BlitzId> = base
        .get_node(select)
        .map(|node| node.children.iter().rev().copied().collect())
        .unwrap_or_default();
    while let Some(current) = stack.pop() {
        let Some(node) = base.get_node(current) else {
            continue;
        };
        if node.data.downcast_element().is_some_and(|element| {
            element.name.ns == html_namespace() && element.name.local.as_ref() == "option"
        }) {
            options.push(NodeId {
                document,
                node: current,
            });
        }
        stack.extend(node.children.iter().rev().copied());
    }
    options
}

/// The `select` owning `option`: its nearest ancestor `select`, if any.
fn option_select_owner(
    base: &blitz_dom::BaseDocument,
    document: u32,
    option: BlitzId,
) -> Option<NodeId> {
    let mut cursor = base.get_node(option).and_then(|node| node.parent);
    while let Some(current) = cursor {
        if crate::js::world::is_html_element(base, current, "select") {
            return Some(NodeId {
                document,
                node: current,
            });
        }
        cursor = base.get_node(current).and_then(|node| node.parent);
    }
    None
}

/// An `option`'s value: its `value` attribute, else its text
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-value>).
fn option_value_in(base: &blitz_dom::BaseDocument, option: BlitzId) -> String {
    attr(base, option, "value").map_or_else(|| option_text_in(base, option), str::to_owned)
}

/// An `option`'s text: its descendant text with ASCII whitespace stripped and
/// collapsed, skipping HTML and SVG `script` subtrees
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-text>).
fn option_text_in(base: &blitz_dom::BaseDocument, option: BlitzId) -> String {
    let mut text = String::new();
    let mut stack: Vec<BlitzId> = base
        .get_node(option)
        .map(|node| node.children.iter().rev().copied().collect())
        .unwrap_or_default();
    while let Some(current) = stack.pop() {
        let Some(node) = base.get_node(current) else {
            continue;
        };
        match &node.data {
            NodeData::Text(data) => text.push_str(&data.content),
            NodeData::Element(element) => {
                let is_script = element.name.local.as_ref().eq_ignore_ascii_case("script")
                    && (element.name.ns == html_namespace()
                        || element.name.ns.as_ref() == "http://www.w3.org/2000/svg");
                if !is_script {
                    stack.extend(node.children.iter().rev().copied());
                }
            }
            _ => {}
        }
    }
    collapse_option_text(&text)
}

/// Strips and collapses ASCII whitespace runs to single spaces, dropping
/// leading and trailing runs.
fn collapse_option_text(text: &str) -> String {
    let mut result = String::new();
    let mut pending_space = false;
    for character in text.chars() {
        if character.is_ascii_whitespace() {
            pending_space = !result.is_empty();
        } else {
            if pending_space {
                result.push(' ');
                pending_space = false;
            }
            result.push(character);
        }
    }
    result
}

/// A `select`'s value: the first selected option's value, else the empty
/// string. Blitz keeps selectedness as the `selected` attribute; selecting the
/// first option by default is a known cutover gap
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-value>).
fn select_value_in(base: &blitz_dom::BaseDocument, document: u32, select: BlitzId) -> String {
    for option in select_options_in(base, document, select) {
        if attr(base, option.node, "selected").is_some() {
            return option_value_in(base, option.node);
        }
    }
    String::new()
}

/// The form owner of a control: its nearest ancestor `form` element. Blitz
/// keeps no parser-associated form owner, so controls outside a form ancestor
/// have no owner here (a known cutover gap)
/// (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#reset-the-form-owner>).
fn form_owner_of(base: &blitz_dom::BaseDocument, document: u32, node: BlitzId) -> Option<NodeId> {
    let mut cursor = base.get_node(node).and_then(|target| target.parent);
    while let Some(current) = cursor {
        if crate::js::world::is_html_element(base, current, "form") {
            return Some(NodeId {
                document,
                node: current,
            });
        }
        cursor = base.get_node(current).and_then(|target| target.parent);
    }
    None
}

/// A control's value length in UTF-16 code units, the basis selection
/// offsets clamp against: the `value` attribute for inputs, descendant text
/// for textareas.
fn control_value_len(doc: &crate::documents::BlitzDocument, node: BlitzId) -> u32 {
    let base = &doc.base;
    let Some(element) = base
        .get_node(node)
        .and_then(|target| target.data.downcast_element())
    else {
        return 0;
    };
    if element.name.ns == html_namespace() && element.name.local.as_ref() == "textarea" {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "control values stay far below u32::MAX"
        )]
        let len = descendant_text(doc, node).units().len() as u32;
        return len;
    }
    let len = element
        .attr(markup5ever::LocalName::from("value"))
        .unwrap_or_default()
        .encode_utf16()
        .count();
    #[expect(
        clippy::cast_possible_truncation,
        reason = "control values stay far below u32::MAX"
    )]
    let len = len as u32;
    len
}
/// A control's stored selection start clamped to its value length.
fn selection_start_of(ctx: &rquickjs::Ctx<'_>, id: NodeId) -> rquickjs::Result<u32> {
    let world_rc = world(ctx)?;
    let world = world_rc.borrow();
    let parsed = world.document(id);
    let Some(parsed) = parsed else {
        return Ok(0);
    };
    let (start, _, _) = world.selection_of(id);
    Ok(start.min(control_value_len(&parsed.document, id.node)))
}

/// A control's stored selection end clamped to its value length.
fn selection_end_of(ctx: &rquickjs::Ctx<'_>, id: NodeId) -> rquickjs::Result<u32> {
    let world_rc = world(ctx)?;
    let world = world_rc.borrow();
    let Some(parsed) = world.document(id) else {
        return Ok(0);
    };
    let (_, end, _) = world.selection_of(id);
    Ok(end.min(control_value_len(&parsed.document, id.node)))
}

/// A control's stored selection direction code.
fn selection_direction_of(ctx: &rquickjs::Ctx<'_>, id: NodeId) -> rquickjs::Result<u8> {
    let world_rc = world(ctx)?;
    let world = world_rc.borrow();
    let Some(parsed) = world.document(id) else {
        return Ok(0);
    };
    if !selection_applies(&parsed.document.base, id.node) {
        return Ok(0);
    }
    Ok(world.selection_of(id).2)
}
/// Whether text selection applies to a control: textareas always, text-like
/// inputs only.
fn selection_applies(base: &blitz_dom::BaseDocument, node: BlitzId) -> bool {
    let Some(element) = base
        .get_node(node)
        .and_then(|target| target.data.downcast_element())
    else {
        return false;
    };
    if element.name.ns != html_namespace() {
        return false;
    }
    match element.name.local.as_ref() {
        "textarea" => true,
        "input" => {
            let typ = attr(base, node, "type")
                .unwrap_or("text")
                .trim()
                .to_ascii_lowercase();
            !matches!(
                typ.as_str(),
                "hidden" | "checkbox" | "radio" | "file" | "submit" | "image" | "reset" | "button"
            )
        }
        _ => false,
    }
}

/// The value of the `(namespace, local)` attribute on `id`, if present.
fn attr_ns_value(
    base: &blitz_dom::BaseDocument,
    id: BlitzId,
    namespace: &str,
    local: &str,
) -> Option<String> {
    base.get_node(id)
        .and_then(|node| node.data.downcast_element())
        .and_then(|element| {
            element
                .attrs
                .iter()
                .find(|attribute| {
                    attribute.name.ns.as_ref() == namespace
                        && attribute.name.local.as_ref() == local
                })
                .map(|attribute| attribute.value.clone())
        })
}

/// The element the namespace lookup starts from: the node itself when it is
/// an element, the document element for a document, else the parent element
/// (<https://dom.spec.whatwg.org/#locate-a-namespace>).
fn namespace_start(base: &blitz_dom::BaseDocument, document: u32, node: NodeId) -> Option<NodeId> {
    let data = &base.get_node(node.node)?.data;
    if data.downcast_element().is_some() {
        return Some(node);
    }
    if let NodeData::Document(_) = data {
        base.get_node(node.node)?.children.iter().find_map(|child| {
            let id = NodeId {
                document,
                node: *child,
            };
            is_element(base, *child).then_some(id)
        })
    } else {
        let parent = base.get_node(node.node)?.parent?;
        let id = NodeId {
            document,
            node: parent,
        };
        is_element(base, parent).then_some(id)
    }
}

/// [Locate a namespace](https://dom.spec.whatwg.org/#locate-a-namespace) for
/// `prefix` walking `cursor`'s inclusive ancestors.
fn locate_namespace(
    base: &blitz_dom::BaseDocument,
    document: u32,
    cursor: NodeId,
    prefix: Option<&str>,
) -> Option<String> {
    let mut cursor = namespace_start(base, document, cursor);
    while let Some(id) = cursor {
        if let Some(element) = base
            .get_node(id.node)
            .and_then(|node| node.data.downcast_element())
        {
            match prefix {
                Some("xml") => {
                    return Some("http://www.w3.org/XML/1998/namespace".to_owned());
                }
                Some("xmlns") => return Some("http://www.w3.org/2000/xmlns/".to_owned()),
                _ => {}
            }
            let actual = element
                .name
                .prefix
                .as_ref()
                .map(markup5ever::Prefix::as_ref)
                .filter(|prefix| !prefix.is_empty());
            if !element.name.ns.as_ref().is_empty() && actual == prefix {
                return Some(element.name.ns.as_ref().to_owned());
            }
            for attribute in element.attrs.iter() {
                if attribute.name.ns.as_ref() != "http://www.w3.org/2000/xmlns/" {
                    continue;
                }
                let declaration = match prefix {
                    Some(prefix) => {
                        attribute
                            .name
                            .prefix
                            .as_ref()
                            .is_some_and(|value| value.as_ref() == "xmlns")
                            && attribute.name.local.as_ref() == prefix
                    }
                    None => {
                        attribute.name.prefix.is_none() && attribute.name.local.as_ref() == "xmlns"
                    }
                };
                if declaration {
                    return (!attribute.value.is_empty()).then(|| attribute.value.clone());
                }
            }
        }
        cursor = base
            .get_node(id.node)
            .and_then(|node| node.parent)
            .map(|parent| NodeId {
                document,
                node: parent,
            })
            .filter(|parent| is_element(base, parent.node));
    }
    None
}

/// [Locate a namespace prefix](https://dom.spec.whatwg.org/#locate-a-namespace-prefix)
/// for `namespace` walking `cursor`'s inclusive ancestors.
fn locate_prefix(
    base: &blitz_dom::BaseDocument,
    document: u32,
    cursor: NodeId,
    namespace: &str,
) -> Option<String> {
    let mut cursor = namespace_start(base, document, cursor);
    while let Some(id) = cursor {
        if let Some(element) = base
            .get_node(id.node)
            .and_then(|node| node.data.downcast_element())
        {
            if element.name.ns.as_ref() == namespace
                && let Some(prefix) = element
                    .name
                    .prefix
                    .as_ref()
                    .map(markup5ever::Prefix::as_ref)
                    .filter(|prefix| !prefix.is_empty())
            {
                return Some(prefix.to_owned());
            }
            for attribute in element.attrs.iter() {
                if attribute
                    .name
                    .prefix
                    .as_ref()
                    .is_some_and(|prefix| prefix.as_ref() == "xmlns")
                    && attribute.value == namespace
                {
                    return Some(attribute.name.local.as_ref().to_owned());
                }
            }
        }
        cursor = base
            .get_node(id.node)
            .and_then(|node| node.parent)
            .map(|parent| NodeId {
                document,
                node: parent,
            })
            .filter(|parent| is_element(base, parent.node));
    }
    None
}

/// Structural `isEqualNode`
/// (<https://dom.spec.whatwg.org/#concept-node-equals>). Blitz has no
/// doctype, processing-instruction, or CDATA kinds; fragments compare by
/// their backing's children.
fn nodes_equal_in(
    base_a: &blitz_dom::BaseDocument,
    a: BlitzId,
    base_b: &blitz_dom::BaseDocument,
    b: BlitzId,
) -> bool {
    let node_a = base_a.get_node(a);
    let node_b = base_b.get_node(b);
    let (Some(node_a), Some(node_b)) = (node_a, node_b) else {
        return false;
    };
    let equal = match (&node_a.data, &node_b.data) {
        (NodeData::Document(_), NodeData::Document(_)) => true,
        (
            NodeData::Element(left) | NodeData::AnonymousBlock(left),
            NodeData::Element(right) | NodeData::AnonymousBlock(right),
        ) => {
            left.name == right.name
                && left.attrs.len() == right.attrs.len()
                && left.attrs.iter().all(|attribute| {
                    right.attrs.iter().any(|candidate| {
                        candidate.name == attribute.name && candidate.value == attribute.value
                    })
                })
        }
        (NodeData::Text(left), NodeData::Text(right)) => left.content == right.content,
        (NodeData::Comment { contents: left }, NodeData::Comment { contents: right }) => {
            left == right
        }
        _ => false,
    };
    if !equal {
        return false;
    }
    if node_a.children.len() != node_b.children.len() {
        return false;
    }
    node_a
        .children
        .iter()
        .zip(node_b.children.iter())
        .all(|(left, right)| nodes_equal_in(base_a, *left, base_b, *right))
}

/// Serializer output: UTF-16 code units, so a lone surrogate in character
/// data survives into the returned string. Escaping only ever inserts ASCII,
/// so tree units append verbatim.
struct HtmlOutput(Vec<u16>);

impl HtmlOutput {
    fn push_str(&mut self, text: &str) {
        self.0.extend(text.encode_utf16());
    }

    fn push_units(&mut self, units: &[u16]) {
        self.0.extend_from_slice(units);
    }

    fn finish(self) -> DomString {
        DomString::from_utf16(self.0)
    }
}

/// Whether an element serializes as void
/// (<https://html.spec.whatwg.org/multipage/parsing.html#serialises-as-void>).
fn serializes_as_void(name: &QualName) -> bool {
    name.ns == html_namespace()
        && matches!(
            name.local.as_ref(),
            "area"
                | "base"
                | "basefont"
                | "bgsound"
                | "br"
                | "col"
                | "embed"
                | "frame"
                | "hr"
                | "img"
                | "input"
                | "keygen"
                | "link"
                | "menuitem"
                | "meta"
                | "param"
                | "source"
                | "track"
                | "wbr"
        )
}

/// The element's HTML-serialized name: the local name for HTML, `MathML`, and
/// SVG elements, the qualified name otherwise
/// (<https://html.spec.whatwg.org/multipage/parsing.html#serialising-html-fragments>).
fn push_html_element_name(output: &mut HtmlOutput, name: &QualName) {
    if name.ns == html_namespace()
        || name.ns == svg_namespace()
        || name.ns.as_ref() == "http://www.w3.org/1998/Math/MathML"
    {
        output.push_str(name.local.as_ref());
    } else if let Some(prefix) = name.prefix.as_ref().filter(|prefix| !prefix.is_empty()) {
        output.push_str(prefix.as_ref());
        output.push_str(":");
        output.push_str(name.local.as_ref());
    } else {
        output.push_str(name.local.as_ref());
    }
}

/// The attribute's HTML-serialized name
/// (<https://html.spec.whatwg.org/multipage/parsing.html#attribute-s-serialized-name>).
fn push_html_attribute_name(output: &mut HtmlOutput, name: &QualName) {
    if name.ns.as_ref().is_empty() {
        output.push_str(name.local.as_ref());
    } else if name.ns.as_ref() == "http://www.w3.org/XML/1998/namespace" {
        output.push_str("xml:");
        output.push_str(name.local.as_ref());
    } else if name.ns.as_ref() == "http://www.w3.org/2000/xmlns/" {
        if name.local.as_ref() == "xmlns" {
            output.push_str("xmlns");
        } else {
            output.push_str("xmlns:");
            output.push_str(name.local.as_ref());
        }
    } else if name.ns.as_ref() == "http://www.w3.org/1999/xlink" {
        output.push_str("xlink:");
        output.push_str(name.local.as_ref());
    } else if let Some(prefix) = name.prefix.as_ref().filter(|prefix| !prefix.is_empty()) {
        output.push_str(prefix.as_ref());
        output.push_str(":");
        output.push_str(name.local.as_ref());
    } else {
        output.push_str(name.local.as_ref());
    }
}

fn push_escaped_html_text(output: &mut HtmlOutput, text: &str) {
    for unit in text.encode_utf16() {
        match unit {
            0x26 => output.push_str("&amp;"),
            0x00a0 => output.push_str("&nbsp;"),
            0x3c => output.push_str("&lt;"),
            0x3e => output.push_str("&gt;"),
            _ => output.push_units(&[unit]),
        }
    }
}

fn push_escaped_html_attribute(output: &mut HtmlOutput, value: &str) {
    for unit in value.encode_utf16() {
        match unit {
            0x26 => output.push_str("&amp;"),
            0x00a0 => output.push_str("&nbsp;"),
            0x22 => output.push_str("&quot;"),
            _ => output.push_units(&[unit]),
        }
    }
}

/// Serializes one HTML element with the fragment serialization algorithm.
/// Known gap: `crate::serialize` still targets the pre-cutover tree, so this
/// local port covers elements, text, and comments only (no doctypes or
/// processing instructions, which Blitz cannot hold)
/// (<https://html.spec.whatwg.org/multipage/parsing.html#serialising-html-fragments>).
fn serialize_html_element(
    doc: &crate::documents::BlitzDocument,
    id: BlitzId,
    name: &QualName,
    attributes: &[blitz_dom::Attribute],
    output: &mut HtmlOutput,
) {
    let base = &doc.base;
    output.push_str("<");
    push_html_element_name(output, name);
    for attribute in attributes {
        output.push_str(" ");
        push_html_attribute_name(output, &attribute.name);
        output.push_str("=\"");
        push_escaped_html_attribute(output, &attribute.value);
        output.push_str("\"");
    }
    output.push_str(">");
    if serializes_as_void(name) {
        return;
    }
    if let Some(node) = base.get_node(id) {
        for child in &node.children.clone() {
            serialize_html_node(doc, *child, Some(name), output);
        }
    }
    output.push_str("</");
    push_html_element_name(output, name);
    output.push_str(">");
}

fn serialize_html_node(
    doc: &crate::documents::BlitzDocument,
    id: BlitzId,
    parent: Option<&QualName>,
    output: &mut HtmlOutput,
) {
    let base = &doc.base;
    let Some(node) = base.get_node(id) else {
        return;
    };
    match &node.data {
        NodeData::Element(element) | NodeData::AnonymousBlock(element) => {
            let name = element.name.clone();
            let attributes = element.attrs.iter().cloned().collect::<Vec<_>>();
            serialize_html_element(doc, id, &name, &attributes, output);
        }
        NodeData::Text(data) => {
            let raw_text = parent.is_some_and(|name| {
                name.ns == html_namespace()
                    && matches!(
                        name.local.as_ref(),
                        "style"
                            | "script"
                            | "xmp"
                            | "iframe"
                            | "noembed"
                            | "noscript"
                            | "noframes"
                            | "plaintext"
                    )
            });
            if raw_text {
                output.push_str(&data.content);
            } else {
                push_escaped_html_text(output, &data.content);
            }
        }
        NodeData::Comment { .. } => serialize_comment_or_synthetic(doc, id, output),
        NodeData::Document(_) => {}
    }
}

/// Serializes a comment backing: plain comments as comments, synthetic
/// processing instructions, CDATA sections, and doctypes in their syntax.
fn serialize_comment_or_synthetic(
    doc: &crate::documents::BlitzDocument,
    id: BlitzId,
    output: &mut HtmlOutput,
) {
    let contents = doc
        .base
        .get_node(id)
        .and_then(|node| match &node.data {
            NodeData::Comment { contents } => Some(contents.clone()),
            _ => None,
        })
        .unwrap_or_default();
    match doc.synthetic_kind(id) {
        Some(crate::documents::SyntheticKind::Pi { target }) => {
            let data = contents
                .strip_prefix(target.as_str())
                .map_or(contents.as_str(), |rest| {
                    rest.strip_prefix(' ').unwrap_or(rest)
                });
            output.push_str("<?");
            output.push_str(target);
            if !data.is_empty() {
                output.push_str(" ");
                output.push_str(data);
            }
            output.push_str("?>");
        }
        Some(crate::documents::SyntheticKind::CData) => {
            output.push_str("<![CDATA[");
            output.push_str(&contents);
            output.push_str("]]>");
        }
        Some(crate::documents::SyntheticKind::Doctype {
            name,
            public_id,
            system_id,
        }) => {
            output.push_str("<!DOCTYPE ");
            output.push_str(name);
            if !public_id.is_empty() {
                output.push_str(" PUBLIC \"");
                output.push_str(public_id);
                output.push_str("\"");
                if !system_id.is_empty() {
                    output.push_str(" \"");
                    output.push_str(system_id);
                    output.push_str("\"");
                }
            } else if !system_id.is_empty() {
                output.push_str(" SYSTEM \"");
                output.push_str(system_id);
                output.push_str("\"");
            }
            output.push_str(">");
        }
        None => {
            output.push_str("<!--");
            output.push_str(&contents);
            output.push_str("-->");
        }
    }
}

/// Serializes the children of `parent` with the HTML fragment serialization
/// algorithm, keeping every code unit.
fn serialize_html_children(doc: &crate::documents::BlitzDocument, parent: BlitzId) -> DomString {
    let base = &doc.base;
    let mut output = HtmlOutput(Vec::new());
    if let Some(node) = base.get_node(parent) {
        for child in &node.children.clone() {
            serialize_html_node(doc, *child, None, &mut output);
        }
    }
    output.finish()
}

/// Serializes one element with the HTML fragment serialization algorithm,
/// returning its own markup (`outerHTML`).
fn serialize_html_outer(
    doc: &crate::documents::BlitzDocument,
    id: BlitzId,
    name: &QualName,
    attributes: &[blitz_dom::Attribute],
) -> DomString {
    let mut output = HtmlOutput(Vec::new());
    serialize_html_element(doc, id, name, attributes, &mut output);
    output.finish()
}

fn push_escaped_xml_text(output: &mut HtmlOutput, text: &str) {
    for unit in text.encode_utf16() {
        match unit {
            0x26 => output.push_str("&amp;"),
            0x3c => output.push_str("&lt;"),
            0x3e => output.push_str("&gt;"),
            _ => output.push_units(&[unit]),
        }
    }
}

fn push_escaped_xml_attribute(output: &mut HtmlOutput, value: &str) {
    for unit in value.encode_utf16() {
        match unit {
            0x26 => output.push_str("&amp;"),
            0x22 => output.push_str("&quot;"),
            0x3c => output.push_str("&lt;"),
            0x3e => output.push_str("&gt;"),
            0x09 => output.push_str("&#x9;"),
            0x0a => output.push_str("&#xA;"),
            0x0d => output.push_str("&#xD;"),
            _ => output.push_units(&[unit]),
        }
    }
}

/// XML-serializes one element: the qualified name with escaped attributes and
/// children. Known gap: namespace prefix synthesis is skipped; the stored
/// qualified name is used as-is
/// (<https://w3c.github.io/DOM-Parsing/#xml-serialization>).
fn serialize_xml_element(
    doc: &crate::documents::BlitzDocument,
    id: BlitzId,
    name: &QualName,
    attributes: &[blitz_dom::Attribute],
    output: &mut HtmlOutput,
) {
    let base = &doc.base;
    output.push_str("<");
    output.push_str(&qualified_name(name));
    for attribute in attributes {
        output.push_str(" ");
        output.push_str(&qualified_name(&attribute.name));
        output.push_str("=\"");
        push_escaped_xml_attribute(output, &attribute.value);
        output.push_str("\"");
    }
    let children: Vec<BlitzId> = base
        .get_node(id)
        .map(|node| node.children.iter().copied().collect())
        .unwrap_or_default();
    if children.is_empty() {
        output.push_str("/>");
        return;
    }
    output.push_str(">");
    for child in children {
        serialize_xml_node(doc, child, output);
    }
    output.push_str("</");
    output.push_str(&qualified_name(name));
    output.push_str(">");
}

fn serialize_xml_node(doc: &crate::documents::BlitzDocument, id: BlitzId, output: &mut HtmlOutput) {
    let base = &doc.base;
    let Some(node) = base.get_node(id) else {
        return;
    };
    match &node.data {
        NodeData::Document(_) => {
            for child in &node.children.clone() {
                serialize_xml_node(doc, *child, output);
            }
        }
        NodeData::Element(element) | NodeData::AnonymousBlock(element) => {
            let name = element.name.clone();
            let attributes = element.attrs.iter().cloned().collect::<Vec<_>>();
            serialize_xml_element(doc, id, &name, &attributes, output);
        }
        NodeData::Text(data) => push_escaped_xml_text(output, &data.content),
        NodeData::Comment { .. } => serialize_comment_or_synthetic(doc, id, output),
    }
}

/// XML-serializes the children of `parent`.
fn serialize_xml_children(doc: &crate::documents::BlitzDocument, parent: BlitzId) -> DomString {
    let base = &doc.base;
    let mut output = HtmlOutput(Vec::new());
    if let Some(node) = base.get_node(parent) {
        for child in &node.children.clone() {
            serialize_xml_node(doc, *child, &mut output);
        }
    }
    output.finish()
}

/// Whether `id` is an HTML `iframe` or `frame` container, registering any
/// pending browsing contexts before the caller looks its frame up. A script
/// may have appended the container in this same task, so the browsing context
/// is registered first; its realm follows at the next non-JS turn
/// (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#process-the-iframe-attributes>).
/// `frame` shares the lookup: the engine has no frameset navigation pipeline,
/// so no frame document ever exists yet and the callers below return null,
/// matching the pre-migration binary.
fn iframe_frame(ctx: &Ctx<'_>, id: NodeId) -> Result<bool> {
    let is_frame = with_node_data(ctx, id, |data| {
        is_html_element(data, "iframe") || is_html_element(data, "frame")
    })?;
    if is_frame {
        world(ctx)?.borrow_mut().register_pending_frames();
    }
    Ok(is_frame)
}

fn img_size(ctx: &Ctx<'_>, id: NodeId) -> Result<Option<(u32, u32)>> {
    if !with_node_data(ctx, id, |data| is_html_element(data, "img"))? {
        return Ok(None);
    }
    let world = world(ctx)?;
    Ok(world
        .borrow()
        .images
        .get(&id)
        .map(|image| (image.width, image.height)))
}

/// Current viewport offset for `id`'s document.
/// <https://drafts.csswg.org/cssom-view/#scrolling-viewport>
fn viewport_offset(ctx: &Ctx<'_>, id: NodeId) -> Result<(f64, f64)> {
    let world = world_for_node(ctx, id)?;
    let world = world.borrow();
    let Some(parsed) = world.document(id) else {
        return Ok((0.0, 0.0));
    };
    let scroll = parsed.document.base.viewport_scroll();
    Ok((scroll.x, scroll.y))
}

#[rquickjs::methods]
#[allow(
    clippy::needless_pass_by_value,
    clippy::unused_self,
    reason = "rquickjs method ABI passes Ctx by value; create* is a document method"
)]
impl JsNode {
    #[qjs(constructor)]
    fn ctor(ctx: Ctx<'_>) -> Result<Self> {
        Err(Exception::throw_type(&ctx, "Illegal constructor"))
    }
    #[qjs(skip)]
    fn node_type(&self, ctx: &Ctx<'_>) -> Result<u16> {
        // Synthetic backings report their real kind; fragment backings are
        // elements flagged in the document's fragment set.
        // https://dom.spec.whatwg.org/#dom-node-nodetype
        let world = world(ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Err(Exception::throw_type(ctx, "stale node"));
        };
        if let Some(kind) = parsed.document.synthetic_kind(self.handle.0.node) {
            return Ok(match kind {
                crate::documents::SyntheticKind::Pi { .. } => 7,
                crate::documents::SyntheticKind::CData => 4,
                crate::documents::SyntheticKind::Doctype { .. } => 10,
            });
        }
        if parsed.document.is_fragment(self.handle.0.node) {
            return Ok(11);
        }
        with_node_data(ctx, self.handle.0, |data| match data {
            Some(NodeData::Element(_) | NodeData::AnonymousBlock(_)) => Ok(1),
            Some(NodeData::Text(_)) => Ok(3),
            Some(NodeData::Comment { .. }) => Ok(8),
            Some(NodeData::Document(_)) => Ok(9),
            None => Err(Exception::throw_type(ctx, "stale node")),
        })?
    }

    // https://dom.spec.whatwg.org/#dom-node-nodename
    // https://dom.spec.whatwg.org/#concept-element-html-uppercased-qualified-name
    #[qjs(skip)]
    fn node_name<'js>(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let uppercase = document_is_html_content(ctx, self.handle.0);
        let world = world(ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Err(Exception::throw_type(ctx, "stale node"));
        };
        if let Some(kind) = parsed.document.synthetic_kind(self.handle.0.node) {
            let name = match kind {
                crate::documents::SyntheticKind::Pi { target } => target.clone(),
                crate::documents::SyntheticKind::CData => "#cdata-section".to_owned(),
                crate::documents::SyntheticKind::Doctype { name, .. } => name.clone(),
            };
            drop(parsed);
            return rquickjs::String::from_str(ctx.clone(), &name);
        }
        if parsed.document.is_fragment(self.handle.0.node) {
            drop(parsed);
            return rquickjs::String::from_str(ctx.clone(), "#document-fragment");
        }
        drop(parsed);
        let name = with_node_data(ctx, self.handle.0, |data| match data {
            Some(NodeData::Element(element) | NodeData::AnonymousBlock(element)) => {
                Ok(element_node_name(&element.name, uppercase))
            }
            Some(NodeData::Text(_)) => Ok("#text".into()),
            Some(NodeData::Comment { .. }) => Ok("#comment".into()),
            Some(NodeData::Document(_)) => Ok("#document".into()),
            None => Err(Exception::throw_type(ctx, "stale node")),
        })?;
        let name = name?;
        rquickjs::String::from_str(ctx.clone(), &name)
    }

    // https://dom.spec.whatwg.org/#dom-node-firstchild
    #[qjs(skip)]
    fn first_child<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let world = world(ctx)?;
        let id = world.borrow().document(self.handle.0).and_then(|parsed| {
            parsed
                .document
                .base
                .get_node(self.handle.0.node)
                .and_then(|node| node.children.first().copied())
                .map(|node| NodeId {
                    document: parsed.id,
                    node,
                })
        });
        child_value(ctx, id)
    }

    #[qjs(skip)]
    fn parent_node<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let world = world(ctx)?;
        let id = world.borrow().document(self.handle.0).and_then(|parsed| {
            parsed
                .document
                .base
                .get_node(self.handle.0.node)
                .and_then(|node| node.parent)
                .map(|node| NodeId {
                    document: parsed.id,
                    node,
                })
        });
        child_value(ctx, id)
    }

    #[qjs(skip)]
    fn child_nodes<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        live_collection(ctx, self.handle.0, CollectionKind::Children, None)
    }

    // https://dom.spec.whatwg.org/#dom-node-appendchild
    #[qjs(skip)]
    fn append_child<'js>(&self, ctx: Ctx<'js>, node: NodeReference) -> Result<Value<'js>> {
        let (node, _) = insertion_tree_nodes(&ctx, self.handle.0, node, None)?;
        let node = adopt_node(&ctx, self.handle.0, node)?;
        let parent = self.handle.0;
        insert_tree_node(&ctx, parent, node, None)?;
        wrap_node(&ctx, node)
    }

    // https://dom.spec.whatwg.org/#dom-document-activeelement
    #[qjs(skip)]
    fn active_element<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let document = self.handle.0;
        let world = world(ctx)?;
        let active = world.borrow().active_element(document.document_id());
        if let Some(node) = active
            && world
                .borrow()
                .document(node)
                .is_some_and(|parsed| is_connected(&parsed.document.base, node.node))
        {
            return wrap_node(ctx, node);
        }
        let fallback = {
            let world = world.borrow();
            let Some(parsed) = world.document(document) else {
                return Ok(Value::new_null(ctx.clone()));
            };
            document_first(&parsed, "body").or_else(|| {
                document_first_child(&parsed, |data| matches!(data, NodeData::Element(_)))
            })
        };
        match fallback {
            Some(node) => wrap_node(ctx, node),
            None => Ok(Value::new_null(ctx.clone())),
        }
    }

    // The element's border box from the render pipeline's layout; zero when
    // the element generates no box (for example `display: none`).
    #[qjs(skip)]
    fn get_bounding_client_rect<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        match element_box(&ctx, self.handle.0)? {
            Some((left, top, width, height)) => {
                let (scroll_x, scroll_y) = viewport_offset(&ctx, self.handle.0)?;
                Ok(rect_object(&ctx, left - scroll_x, top - scroll_y, width, height)?.into_value())
            }
            None => Ok(rect_object(&ctx, 0.0, 0.0, 0.0, 0.0)?.into_value()),
        }
    }

    #[qjs(skip)]
    fn get_client_rects<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let array = Array::new(ctx.clone())?;
        if let Some((left, top, width, height)) = element_box(&ctx, self.handle.0)? {
            let (scroll_x, scroll_y) = viewport_offset(&ctx, self.handle.0)?;
            array.set(
                0,
                rect_object(&ctx, left - scroll_x, top - scroll_y, width, height)?,
            )?;
        }
        Ok(array.into_value())
    }

    /// Scrolls the viewport so this element's border box is visible.
    /// <https://drafts.csswg.org/cssom-view/#dom-element-scrollintoview>
    #[qjs(skip)]
    fn scroll_into_view(&self, ctx: Ctx<'_>) -> Result<()> {
        let Some((left, top, width, _height)) = element_box(&ctx, self.handle.0)? else {
            return Ok(());
        };
        let world = world_for_node(&ctx, self.handle.0)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        // Known gap: per-element scroll containers have no Blitz offset API,
        // so scrolling moves the viewport, as if the document element
        // scrolled.
        // https://drafts.csswg.org/cssom-view/#dom-element-scrollintoview
        let scroll = parsed.document.base.viewport_scroll();
        let viewport_width = f64::from(crate::engine::VIEWPORT_WIDTH);
        let next_x = if left < scroll.x {
            left
        } else if left + width > scroll.x + viewport_width {
            left + width - viewport_width
        } else {
            scroll.x
        };
        let next_y = top;
        parsed.document.base.set_viewport_scroll(blitz_dom::Point {
            x: next_x.max(0.0),
            y: next_y.max(0.0),
        });
        Ok(())
    }

    // https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-document-defaultview
    #[qjs(skip)]
    #[allow(
        clippy::unnecessary_wraps,
        reason = "generated getters share one fallible call shape"
    )]
    fn default_view<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        // A document with no browsing context, such as one from DOMParser or
        // `createDocument`, has no view.
        let has_view = world(ctx).is_ok_and(|world| world.borrow().is_main_document(self.handle.0));
        Ok(if has_view {
            ctx.globals().into_value()
        } else {
            Value::new_null(ctx.clone())
        })
    }

    // https://html.spec.whatwg.org/multipage/interaction.html#dom-document-hasfocus
    #[qjs(skip)]
    #[allow(
        clippy::unnecessary_wraps,
        reason = "generated operations share one fallible call shape"
    )]
    fn has_focus(&self, ctx: Ctx<'_>) -> Result<bool> {
        Ok(is_main_document(&ctx, self.handle.0))
    }

    // https://drafts.csswg.org/cssom-view/#dom-document-elementfrompoint
    #[qjs(skip)]
    fn element_from_point<'js>(&self, ctx: Ctx<'js>, x: f64, y: f64) -> Result<Value<'js>> {
        let Some(node) = element_at_point(&ctx, self.handle.0, x, y)? else {
            return Ok(Value::new_null(ctx));
        };
        wrap_node(&ctx, node)
    }

    // The no-layout hit test: the deepest element whose virtual box contains
    // the point (see `element_at_point`).
    #[qjs(skip)]
    fn elements_from_point<'js>(&self, ctx: Ctx<'js>, x: f64, y: f64) -> Result<Vec<Value<'js>>> {
        let mut elements = Vec::new();
        let Some(node) = element_at_point(&ctx, self.handle.0, x, y)? else {
            return Ok(elements);
        };
        // The hit-test stack: the element and its ancestors, topmost first.
        let world = world_for_node(&ctx, node)?;
        let mut cursor = Some(node);
        while let Some(current) = cursor {
            elements.push(wrap_node(&ctx, current)?);
            cursor = world.borrow().node_parent(current);
        }
        Ok(elements)
    }

    // https://dom.spec.whatwg.org/#dom-document-createelement
    #[qjs(skip)]
    fn create_element<'js>(&self, ctx: Ctx<'js>, tag: WebIdlString) -> Result<Value<'js>> {
        create_html_element(&ctx, self.handle.0, &tag.0)
    }

    // https://dom.spec.whatwg.org/#dom-document-createelementns
    // https://dom.spec.whatwg.org/#internal-createelementns-steps
    #[qjs(skip)]
    fn create_element_ns<'js>(
        &self,
        ctx: Ctx<'js>,
        ns: OptString,
        tag: WebIdlString,
    ) -> Result<Value<'js>> {
        let name = validate_and_extract(&ctx, ns.0.as_deref(), &tag.0, NodeContext::Element)?;
        create_element_named(&ctx, self.handle.0, name)
    }

    #[qjs(skip)]
    fn create_text_node<'js>(&self, ctx: Ctx<'js>, data: WebIdlCodeUnits) -> Result<Value<'js>> {
        let text = data.0.to_string_lossy().into_owned();
        create_node(&ctx, self.handle.0, |parsed| {
            parsed.document.base.mutate().create_text_node(&text)
        })
    }

    // https://dom.spec.whatwg.org/#dom-document-createcomment
    #[qjs(skip)]
    fn create_comment<'js>(&self, ctx: Ctx<'js>, data: WebIdlCodeUnits) -> Result<Value<'js>> {
        let text = data.0.to_string_lossy().into_owned();
        create_node(&ctx, self.handle.0, |parsed| {
            parsed.document.base.mutate().create_comment_node(&text)
        })
    }

    // https://dom.spec.whatwg.org/#dom-document-createprocessinginstruction
    #[qjs(skip)]
    fn create_processing_instruction<'js>(
        &self,
        ctx: Ctx<'js>,
        target: WebIdlString,
        data: WebIdlCodeUnits,
    ) -> Result<Value<'js>> {
        if !crate::xml::is_valid_name(&target.0) {
            return Err(throw_dom(
                &ctx,
                "InvalidCharacterError",
                "target does not match the XML Name production",
            ));
        }
        // `xml`, ASCII case-insensitive, is reserved
        // (<https://dom.spec.whatwg.org/#dom-document-createprocessinginstruction>).
        if target.0.eq_ignore_ascii_case("xml") {
            return Err(throw_dom(
                &ctx,
                "InvalidCharacterError",
                "target must not be xml",
            ));
        }
        if data.0.to_string_lossy().contains("?>") {
            return Err(throw_dom(&ctx, "InvalidCharacterError", "data contains ?>"));
        }
        create_node(&ctx, self.handle.0, |parsed| {
            parsed
                .document
                .create_pi(&target.0, &data.0.to_string_lossy())
        })
    }

    // https://dom.spec.whatwg.org/#dom-document-createcdatasection
    #[qjs(skip)]
    fn create_cdata_section<'js>(
        &self,
        ctx: Ctx<'js>,
        data: WebIdlCodeUnits,
    ) -> Result<Value<'js>> {
        // CDATA sections cannot exist in HTML documents
        // (<https://dom.spec.whatwg.org/#dom-document-createcdatasection>).
        if document_is_html(&ctx, self.handle.0) {
            return Err(throw_dom(
                &ctx,
                "NotSupportedError",
                "CDATA sections are not supported in HTML documents",
            ));
        }
        if data.0.to_string_lossy().contains("]]>") {
            return Err(throw_dom(
                &ctx,
                "InvalidCharacterError",
                "data contains ]]>",
            ));
        }
        create_node(&ctx, self.handle.0, |parsed| {
            parsed
                .document
                .create_cdata(&data.0.to_string_lossy())
        })
    }

    // https://dom.spec.whatwg.org/#dom-document-createattribute
    #[qjs(skip)]
    fn create_attribute<'js>(&self, ctx: Ctx<'js>, name: WebIdlString) -> Result<Value<'js>> {
        if !valid_attribute_local_name(&name.0) {
            return Err(throw_dom(
                &ctx,
                "InvalidCharacterError",
                "attribute name is not a valid attribute local name",
            ));
        }
        let id = new_detached_attr(
            &ctx,
            self.handle.0,
            String::new(),
            None,
            name.0.clone(),
            name.0,
        )?;
        attr_wrapper(&ctx, id)
    }

    // https://dom.spec.whatwg.org/#dom-document-createattributens
    #[qjs(skip)]
    fn create_attribute_ns<'js>(
        &self,
        ctx: Ctx<'js>,
        namespace: OptString,
        qualified: WebIdlString,
    ) -> Result<Value<'js>> {
        let name = validate_and_extract(
            &ctx,
            namespace.0.as_deref(),
            &qualified.0,
            NodeContext::Attribute,
        )?;
        let prefix = name
            .prefix
            .as_ref()
            .filter(|prefix| !prefix.is_empty())
            .map(ToString::to_string);
        let id = new_detached_attr(
            &ctx,
            self.handle.0,
            name.ns.to_string(),
            prefix,
            name.local.to_string(),
            qualified_name(&name),
        )?;
        attr_wrapper(&ctx, id)
    }

    // https://dom.spec.whatwg.org/#dom-document-createdocumentfragment
    #[qjs(skip)]
    fn create_document_fragment<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        create_node(&ctx, self.handle.0, |parsed| {
            parsed.document.create_fragment()
        })
    }

    // https://dom.spec.whatwg.org/#dom-document-importnode
    #[qjs(skip)]
    fn import_node<'js>(&self, ctx: Ctx<'js>, node: Value<'js>, deep: bool) -> Result<Value<'js>> {
        let source_id = required_node(&ctx, &node)?;
        let world_rc = world_for_node(&ctx, self.handle.0)?;
        let tree = {
            let world = world_rc.borrow();
            let Some(source) = world.document(source_id) else {
                return Err(Exception::throw_type(&ctx, "stale node"));
            };
            if source.document.base.root_node().id == source_id.node {
                return Err(throw_dom(
                    &ctx,
                    "NotSupportedError",
                    "cannot import a document",
                ));
            }
            snapshot_for_adopt(&source.document, source_id, deep)
        };
        let Some(tree) = tree else {
            return Err(Exception::throw_type(&ctx, "stale node"));
        };
        let world = world_rc.borrow();
        let Some(mut target) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        let mut id = materialize_import(&mut target.document, &tree)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        // `materialize_import` builds detached trees with a placeholder
        // document id; remap to the target document before wrapping.
        id.document = self.handle.0.document;
        drop(target);
        drop(world);
        wrap_node(&ctx, id)
    }

    // https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-document-open
    #[qjs(skip)]
    fn open_document<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        world_for_node(&ctx, self.handle.0)?
            .borrow_mut()
            .queue_document_stream(DocumentStreamCommand::Open)
            .map_err(|()| Exception::throw_range(&ctx, "document stream budget exceeded"))?;
        wrap_node(&ctx, self.handle.0)
    }

    // https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#document-write-steps
    #[qjs(skip)]
    fn write(&self, ctx: Ctx<'_>, text: Vec<WebIdlString>) -> Result<()> {
        // Every argument stringifies and concatenates in order
        // (<https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-document-write>).
        let html: String = text.iter().map(|part| part.0.as_str()).collect();
        let world = world_for_node(&ctx, self.handle.0)?;
        let mut world = world.borrow_mut();
        if world.parser_active {
            if !world.reserve_stream_bytes(html.len()) {
                return Err(Exception::throw_range(
                    &ctx,
                    "document stream budget exceeded",
                ));
            }
            world.pending_html_writes.push(html);
        } else {
            world
                .queue_document_stream(DocumentStreamCommand::Write(html))
                .map_err(|()| Exception::throw_range(&ctx, "document stream budget exceeded"))?;
        }
        Ok(())
    }

    // https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-document-close
    #[qjs(skip)]
    fn close_document(&self, ctx: Ctx<'_>) -> Result<()> {
        world_for_node(&ctx, self.handle.0)?
            .borrow_mut()
            .queue_document_stream(DocumentStreamCommand::Close)
            .map_err(|()| Exception::throw_range(&ctx, "document stream budget exceeded"))
    }

    #[qjs(skip)]
    fn get_element_by_id<'js>(&self, ctx: Ctx<'js>, id: WebIdlString) -> Result<Value<'js>> {
        element_by_id(&ctx, self.handle.0, &id.0)
    }

    // https://dom.spec.whatwg.org/#dom-document-implementation
    #[qjs(skip)]
    fn implementation<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let world_rc = world(ctx)?;
        if let Some(saved) = world_rc.borrow().implementation(self.handle.0)
            && let Some(value) = deref_weak(ctx, saved)?
        {
            return Ok(value);
        }
        let value = Class::instance(
            ctx.clone(),
            JsImplementation {
                document: self.handle,
            },
        )?
        .into_value();
        let weak = make_weak(ctx, value.clone())?;
        world_rc
            .borrow_mut()
            .intern_implementation(self.handle.0, Persistent::save(ctx, weak));
        Ok(value)
    }

    #[qjs(skip)]
    fn get_elements_by_tag_name<'js>(
        &self,
        ctx: Ctx<'js>,
        name: WebIdlString,
    ) -> Result<Value<'js>> {
        elements_by_tag(&ctx, self.handle.0, &name.0)
    }

    #[qjs(skip)]
    fn body<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        document_value(ctx, self.handle.0, |parsed| document_first(parsed, "body"))
    }

    // https://html.spec.whatwg.org/multipage/dom.html#dom-document-body
    #[qjs(skip)]
    fn set_body<'js>(&self, ctx: &Ctx<'js>, value: Value<'js>) -> Result<()> {
        // A string or other non-node throws, like any interface conversion.
        let Some(id) = host_node_id(ctx, &value) else {
            return Err(Exception::throw_type(ctx, "body must be an element"));
        };
        let is_body = with_node_data(ctx, id, |data| match data {
            Some(NodeData::Element(element) | NodeData::AnonymousBlock(element)) => {
                element.name.ns == html_namespace()
                    && (element.name.local.as_ref() == "body"
                        || element.name.local.as_ref() == "frameset")
            }
            _ => false,
        })?;
        if !is_body {
            return Err(throw_dom(
                ctx,
                "HierarchyRequestError",
                "body must be a body or frameset element",
            ));
        }
        let world_rc = world(ctx)?;
        let root_element = {
            let world = world_rc.borrow();
            let Some(parsed) = world.document(self.handle.0) else {
                return Err(Exception::throw_type(ctx, "stale node"));
            };
            document_first_child(&parsed, |data| {
                matches!(data, NodeData::Element(_) | NodeData::AnonymousBlock(_))
            })
        };
        let Some(root_element) = root_element else {
            return Err(throw_dom(
                ctx,
                "HierarchyRequestError",
                "document has no document element",
            ));
        };
        let current = current_body_child(ctx, &world_rc, root_element)?;
        if current == Some(id) {
            return Ok(());
        }
        let node = adopt_node(ctx, self.handle.0, id)?;
        {
            let world = world_rc.borrow();
            let Some(parsed) = world.document(self.handle.0) else {
                return Err(Exception::throw_type(ctx, "no document"));
            };
            // The new body cannot live inside the subtree it joins.
            if node.document == self.handle.0.document {
                let base = &parsed.document.base;
                let mut cursor = Some(root_element.node);
                while let Some(current) = cursor {
                    if current == node.node {
                        return Err(throw_dom(
                            ctx,
                            "HierarchyRequestError",
                            "node cannot contain itself",
                        ));
                    }
                    cursor = base
                        .get_node(current)
                        .and_then(|candidate| candidate.parent);
                }
            }
        }
        {
            let world = world_rc.borrow();
            let Some(mut parsed) = world.document_mut(self.handle.0) else {
                return Err(Exception::throw_type(ctx, "no document"));
            };
            if let Some(current) = current {
                replace_parsed(&mut parsed, root_element, node, current);
                drop(parsed);
                drop(world);
                fixup_focus_after_removal(ctx, current)?;
            } else {
                insert_parsed(&mut parsed, root_element, node, None);
                drop(parsed);
                drop(world);
            }
        }
        schedule_mutation_delivery(ctx)
    }

    // https://html.spec.whatwg.org/multipage/dom.html#dom-document-head
    #[qjs(skip)]
    fn head<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        document_value(ctx, self.handle.0, |parsed| document_first(parsed, "head"))
    }

    // https://html.spec.whatwg.org/multipage/dom.html#dom-document-currentscript
    #[qjs(skip)]
    fn current_script<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let world = world(ctx)?;
        let current = world.borrow().current_script;
        match current {
            Some(id) if id.document_id() == self.handle.0.document_id() => wrap_node(ctx, id),
            _ => Ok(Value::new_null(ctx.clone())),
        }
    }

    #[qjs(skip)]
    fn document_element<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        document_value(ctx, self.handle.0, |parsed| {
            document_first_child(parsed, |data| {
                matches!(data, NodeData::Element(_) | NodeData::AnonymousBlock(_))
            })
        })
    }

    // https://dom.spec.whatwg.org/#dom-document-doctype
    #[qjs(skip)]
    fn doctype<'js>(&self, ctx: &Ctx<'js>) -> Value<'js> {
        let world = world(ctx);
        let found = world.ok().and_then(|world| {
            let parsed = world.borrow();
            let parsed = parsed.document(self.handle.0)?;
            let node = parsed.document.document_doctype()?;
            Some(NodeId {
                document: parsed.id,
                node,
            })
        });
        match found {
            Some(id) => wrap_node(ctx, id).unwrap_or_else(|_| Value::new_null(ctx.clone())),
            None => Value::new_null(ctx.clone()),
        }
    }

    // https://dom.spec.whatwg.org/#dom-document-readyState
    #[qjs(skip)]
    fn ready_state(&self, ctx: &Ctx<'_>) -> Result<String> {
        let world = world(ctx)?;
        let world = world.borrow();
        let Some(parsed) = world.document(self.handle.0) else {
            return Ok(String::new());
        };
        if parsed.document.base.root_node().id != self.handle.0.node {
            return Ok(String::new());
        }
        Ok(match parsed.ready_state {
            ReadyState::Loading => "loading".into(),
            ReadyState::Interactive => "interactive".into(),
            ReadyState::Complete => "complete".into(),
        })
    }

    // https://dom.spec.whatwg.org/#dom-document-url
    #[qjs(skip)]
    #[allow(
        clippy::unnecessary_wraps,
        reason = "generated getters share one fallible call shape"
    )]
    fn url(&self, ctx: &Ctx<'_>) -> Result<crate::dom_string::DomString> {
        Ok(document_url_string(ctx, self.handle.0).into())
    }

    // https://dom.spec.whatwg.org/#dom-document-documenturi
    #[qjs(skip)]
    #[allow(
        clippy::unnecessary_wraps,
        reason = "generated getters share one fallible call shape"
    )]
    fn document_uri(&self, ctx: &Ctx<'_>) -> Result<crate::dom_string::DomString> {
        Ok(document_url_string(ctx, self.handle.0).into())
    }

    // https://html.spec.whatwg.org/multipage/urls-and-fetching.html#dom-document-baseuri
    #[qjs(skip)]
    #[allow(
        clippy::unnecessary_wraps,
        reason = "generated getters share one fallible call shape"
    )]
    fn base_uri(&self, ctx: &Ctx<'_>) -> Result<crate::dom_string::DomString> {
        Ok(document_base_url_string(ctx, self.handle.0).into())
    }

    // https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-document-location
    #[qjs(skip)]
    fn location<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        if is_main_document(ctx, self.handle.0) {
            return ctx.globals().get("location");
        }
        Ok(Value::new_null(ctx.clone()))
    }

    // https://encoding.spec.whatwg.org/#dom-document-characterset
    #[qjs(skip)]
    #[allow(
        clippy::unnecessary_wraps,
        reason = "generated getters share one fallible call shape"
    )]
    fn character_set(&self, _ctx: &Ctx<'_>) -> Result<&'static str> {
        Ok("UTF-8")
    }

    #[qjs(skip)]
    #[allow(
        clippy::unnecessary_wraps,
        reason = "generated getters share one fallible call shape"
    )]
    fn charset(&self, _ctx: &Ctx<'_>) -> Result<&'static str> {
        Ok("UTF-8")
    }

    #[qjs(skip)]
    #[allow(
        clippy::unnecessary_wraps,
        reason = "generated getters share one fallible call shape"
    )]
    fn input_encoding(&self, _ctx: &Ctx<'_>) -> Result<&'static str> {
        Ok("UTF-8")
    }

    // https://dom.spec.whatwg.org/#dom-document-contenttype
    #[qjs(skip)]
    fn content_type(&self, ctx: &Ctx<'_>) -> Result<&'static str> {
        let world_rc = world(ctx)?;
        let parsed = world_rc.borrow();
        Ok(parsed
            .document(self.handle.0)
            .map_or("text/html", |parsed| parsed.content_type))
    }

    // https://dom.spec.whatwg.org/#dom-document-compatmode
    #[qjs(skip)]
    fn compat_mode(&self, ctx: &Ctx<'_>) -> Result<String> {
        let world = world(ctx)?;
        let parsed = world.borrow();
        let quirks = parsed
            .document(self.handle.0)
            .is_some_and(|parsed| parsed.quirks_mode == markup5ever::interface::QuirksMode::Quirks);
        Ok(if quirks { "BackCompat" } else { "CSS1Compat" }.into())
    }

    #[qjs(skip)]
    fn get_attribute<'js>(&self, ctx: Ctx<'js>, name: WebIdlString) -> Result<Value<'js>> {
        if !valid_attribute_local_name(&name.0) {
            return Ok(Value::new_null(ctx));
        }
        // HTML elements match ASCII-lowercased
        // (<https://dom.spec.whatwg.org/#concept-element-attributes-get-by-name>).
        let local = attribute_local_name(&ctx, self.handle.0, &name.0);
        let world = world(&ctx)?;
        let found = world.borrow().document(self.handle.0).and_then(|parsed| {
            attr(&parsed.document.base, self.handle.0.node, &local).map(ToOwned::to_owned)
        });
        match found {
            Some(value) => string_value(&ctx, &value),
            None => Ok(Value::new_null(ctx)),
        }
    }

    #[qjs(skip)]
    fn set_attribute(&self, ctx: Ctx<'_>, name: WebIdlString, value: WebIdlString) -> Result<()> {
        if !valid_attribute_local_name(&name.0) {
            return Err(throw_dom(
                &ctx,
                "InvalidCharacterError",
                "attribute name is not a valid attribute local name",
            ));
        }
        let local = attribute_local_name(&ctx, self.handle.0, &name.0);
        set_attribute_sync(&ctx, self.handle.0, &local, &value.0)
    }

    #[qjs(skip)]
    fn id(&self, ctx: &Ctx<'_>) -> Result<String> {
        attribute_value(ctx, self.handle.0, "id")
    }

    #[qjs(skip)]
    fn set_id(&self, ctx: &Ctx<'_>, value: WebIdlString) -> Result<()> {
        self.set_attribute(ctx.clone(), WebIdlString("id".into()), value)
    }

    // https://html.spec.whatwg.org/multipage/input.html#dom-input-value
    //
    // Known gap: Blitz keeps no control value store (no dirty value flag, no
    // checkedness or selectedness slots), so every value here is read from
    // content attributes and descendant text: an input's value is its `value`
    // attribute, a textarea's is its descendant text, and a select's comes
    // from the selected option.
    #[qjs(skip)]
    fn value(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(parsed) = world.document(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        let base = &parsed.document.base;
        let Some(element) = base
            .get_node(self.handle.0.node)
            .and_then(|node| node.data.downcast_element())
        else {
            return Ok(String::new());
        };
        if element.name.ns != html_namespace() {
            return Ok(String::new());
        }
        Ok(match element.name.local.as_ref() {
            "textarea" => descendant_text(&parsed.document, self.handle.0.node)
                .to_string_lossy()
                .into_owned(),
            "select" => select_value_in(base, self.handle.0.document, self.handle.0.node),
            "option" => option_value_in(base, self.handle.0.node),
            _ => attr(base, self.handle.0.node, "value")
                .unwrap_or_default()
                .to_owned(),
        })
    }

    // https://html.spec.whatwg.org/multipage/input.html#dom-input-value
    #[qjs(skip)]
    fn set_value(&self, ctx: Ctx<'_>, value: LegacyNullString) -> Result<()> {
        let local = with_node_data(&ctx, self.handle.0, |data| {
            data.and_then(|data| data.downcast_element())
                .filter(|element| element.name.ns == html_namespace())
                .map(|element| element.name.local.to_string())
        })?;
        let Some(local) = local else {
            return Ok(());
        };
        match local.as_str() {
            "textarea" => self.set_textarea_value(&ctx, value.0),
            "select" => self.set_select_value(&ctx, value.0),
            _ => set_attribute_sync(&ctx, self.handle.0, "value", &value.0),
        }
    }

    /// Replaces a textarea's children with one text node carrying `value`.
    #[qjs(skip)]
    fn set_textarea_value(&self, ctx: &Ctx<'_>, value: String) -> Result<()> {
        let world = world(ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(ctx, "no document"));
        };
        let added = if value.is_empty() {
            Vec::new()
        } else {
            let text = parsed.document.base.mutate().create_text_node(&value);
            vec![NodeId {
                document: parsed.id,
                node: text,
            }]
        };
        replace_all_journaled(&mut parsed, self.handle.0, added);
        drop(parsed);
        drop(world);
        schedule_mutation_delivery(ctx)
    }

    /// Selects the first option whose value is `value`, deselecting the rest;
    /// with no match every option is deselected.
    #[qjs(skip)]
    fn set_select_value(&self, ctx: &Ctx<'_>, value: String) -> Result<()> {
        let world_rc = world(ctx)?;
        let options = {
            let world = world_rc.borrow();
            let Some(parsed) = world.document(self.handle.0) else {
                return Err(Exception::throw_type(ctx, "no document"));
            };
            let base = &parsed.document.base;
            select_options_in(base, self.handle.0.document, self.handle.0.node)
        };
        let mut matched = false;
        for option in options {
            let selected = !matched && {
                let world = world_rc.borrow();
                let Some(parsed) = world.document(option) else {
                    continue;
                };
                option_value_in(&parsed.document.base, option.node) == value
            };
            if selected {
                matched = true;
            }
            if selected {
                set_attribute_sync(ctx, option, "selected", "")?;
            } else {
                remove_attribute_sync(ctx, option, "", "selected", false)?;
            }
        }
        Ok(())
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-textarea-defaultvalue
    #[qjs(skip)]
    fn default_value(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(parsed) = world.document(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        Ok(descendant_text(&parsed.document, self.handle.0.node)
            .to_string_lossy()
            .into_owned())
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-textarea-defaultvalue
    #[qjs(skip)]
    fn set_default_value(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        self.set_textarea_value(&ctx, value.0)
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-textarea-textlength
    #[qjs(skip)]
    fn text_length(&self, ctx: Ctx<'_>) -> Result<u32> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(parsed) = world.document(self.handle.0) else {
            return Ok(0);
        };
        #[allow(
            clippy::cast_possible_truncation,
            reason = "a 64-bit string length beyond u32 cannot be produced by this engine"
        )]
        Ok(descendant_text(&parsed.document, self.handle.0.node)
            .to_string_lossy()
            .encode_utf16()
            .count() as u32)
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-textarea/input-selectionstart
    #[qjs(skip)]
    fn set_selection_start(&self, ctx: Ctx<'_>, value: WebIdlUnsignedLong) -> Result<()> {
        let world_rc = world(&ctx)?;
        let (applies, len, current) = {
            let world = world_rc.borrow();
            let Some(parsed) = world.document(self.handle.0) else {
                return Ok(());
            };
            (
                selection_applies(&parsed.document.base, self.handle.0.node),
                control_value_len(&parsed.document, self.handle.0.node),
                world.selection_of(self.handle.0),
            )
        };
        if !applies {
            return Err(throw_dom(
                &ctx,
                "InvalidStateError",
                "selectionStart does not apply to this control",
            ));
        }
        let start = value.0.min(len);
        let (_, end, direction) = current;
        world_rc
            .borrow_mut()
            .set_selection(self.handle.0, start, end.max(start), direction);
        Ok(())
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-textarea/input-selectionend
    #[qjs(skip)]
    fn set_selection_end(&self, ctx: Ctx<'_>, value: WebIdlUnsignedLong) -> Result<()> {
        let world_rc = world(&ctx)?;
        let (applies, len, current) = {
            let world = world_rc.borrow();
            let Some(parsed) = world.document(self.handle.0) else {
                return Ok(());
            };
            (
                selection_applies(&parsed.document.base, self.handle.0.node),
                control_value_len(&parsed.document, self.handle.0.node),
                world.selection_of(self.handle.0),
            )
        };
        if !applies {
            return Err(throw_dom(
                &ctx,
                "InvalidStateError",
                "selectionEnd does not apply to this control",
            ));
        }
        let end = value.0.min(len);
        let (start, _, direction) = current;
        world_rc
            .borrow_mut()
            .set_selection(self.handle.0, start.min(end), end, direction);
        Ok(())
    }

    #[qjs(skip)]
    fn set_selection_direction(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        let direction = direction_code(&value.0);
        let world_rc = world(&ctx)?;
        let (applies, current) = {
            let world = world_rc.borrow();
            let Some(parsed) = world.document(self.handle.0) else {
                return Ok(());
            };
            (
                selection_applies(&parsed.document.base, self.handle.0.node),
                world.selection_of(self.handle.0),
            )
        };
        if !applies {
            return Err(throw_dom(
                &ctx,
                "InvalidStateError",
                "selectionDirection does not apply to this control",
            ));
        }
        let (start, end, _) = current;
        world_rc
            .borrow_mut()
            .set_selection(self.handle.0, start, end, direction);
        Ok(())
    }

    // https://html.spec.whatwg.org/multipage/forms.html#dom-form-reset
    #[qjs(skip)]
    fn reset_form(&self, ctx: Ctx<'_>) -> Result<()> {
        if !with_node_data(&ctx, self.handle.0, |data| is_html_element(data, "form"))? {
            return Err(throw_dom(
                &ctx,
                "InvalidStateError",
                "reset is only available on a form",
            ));
        }
        // Known gap: Blitz keeps no dirty value, checkedness, or
        // selectedness state apart from the content attributes themselves, so
        // there is nothing to restore and reset is a no-op.
        Ok(())
    }

    // https://html.spec.whatwg.org/multipage/forms.html#dom-form-action
    #[qjs(skip)]
    fn action(&self, ctx: Ctx<'_>) -> Result<String> {
        let world_rc = world(&ctx)?;
        let raw = world_rc
            .borrow()
            .document(self.handle.0)
            .and_then(|parsed| {
                attr(&parsed.document.base, self.handle.0.node, "action").map(ToOwned::to_owned)
            });
        let base = document_base_url_string(&ctx, self.handle.0);
        let Some(raw) = raw else {
            return Ok(base);
        };
        Ok(url::Url::parse(&base)
            .ok()
            .and_then(|base| base.join(&raw).ok())
            .map_or(raw, |url| url.to_string()))
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-fs-formaction
    #[qjs(skip)]
    fn form_action(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        let raw = world.borrow().document(self.handle.0).and_then(|parsed| {
            attr(&parsed.document.base, self.handle.0.node, "formaction").map(ToOwned::to_owned)
        });
        let base = document_base_url_string(&ctx, self.handle.0);
        let Some(raw) = raw else {
            return Ok(base);
        };
        Ok(url::Url::parse(&base)
            .ok()
            .and_then(|base| base.join(&raw).ok())
            .map_or(raw, |url| url.to_string()))
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-fs-formmethod
    #[qjs(skip)]
    fn form_method(&self, ctx: Ctx<'_>) -> Result<String> {
        let raw = attribute_value(&ctx, self.handle.0, "formmethod")?;
        Ok(match raw.trim().to_ascii_lowercase().as_str() {
            "post" => "post",
            "dialog" => "dialog",
            _ => "get",
        }
        .to_owned())
    }

    #[qjs(skip)]
    fn set_form_method(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        self.set_attribute(ctx, WebIdlString("formmethod".into()), value)
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-fs-formenctype
    #[qjs(skip)]
    fn form_enctype(&self, ctx: Ctx<'_>) -> Result<String> {
        Ok(encoding_keyword(&attribute_value(&ctx, self.handle.0, "formenctype")?).to_owned())
    }

    #[qjs(skip)]
    fn set_form_enctype(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        self.set_attribute(ctx, WebIdlString("formenctype".into()), value)
    }

    // https://drafts.csswg.org/cssom-view/#dom-element-scrollleft
    //
    // Known gap: Blitz exposes only the viewport scroll offset, not
    // per-element scroll boxes, so only the document element scrolls and
    // every other element reads zero.
    #[qjs(skip)]
    fn scroll_left(&self, ctx: &Ctx<'_>) -> Result<f64> {
        let world = world(ctx)?;
        let world = world.borrow();
        let Some(parsed) = world.document(self.handle.0) else {
            return Ok(0.0);
        };
        if document_first_child(&parsed, |data| {
            matches!(data, NodeData::Element(_) | NodeData::AnonymousBlock(_))
        }) == Some(self.handle.0)
        {
            Ok(parsed.document.base.viewport_scroll().x)
        } else {
            Ok(0.0)
        }
    }

    #[qjs(skip)]
    fn set_scroll_left(&self, ctx: &Ctx<'_>, value: f64) -> Result<()> {
        let world = world(ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        let is_root = document_first_child(&parsed, |data| {
            matches!(data, NodeData::Element(_) | NodeData::AnonymousBlock(_))
        }) == Some(self.handle.0);
        if !is_root {
            return Ok(());
        }
        let viewport = parsed.document.base.viewport_scroll();
        parsed.document.base.set_viewport_scroll(blitz_dom::Point {
            x: value,
            y: viewport.y,
        });
        Ok(())
    }

    // https://drafts.csswg.org/cssom-view/#dom-element-scrolltop
    #[qjs(skip)]
    fn scroll_top(&self, ctx: &Ctx<'_>) -> Result<f64> {
        let world = world(ctx)?;
        let world = world.borrow();
        let Some(parsed) = world.document(self.handle.0) else {
            return Ok(0.0);
        };
        if document_first_child(&parsed, |data| {
            matches!(data, NodeData::Element(_) | NodeData::AnonymousBlock(_))
        }) == Some(self.handle.0)
        {
            Ok(parsed.document.base.viewport_scroll().y)
        } else {
            Ok(0.0)
        }
    }

    #[qjs(skip)]
    fn set_scroll_top(&self, ctx: &Ctx<'_>, value: f64) -> Result<()> {
        let world = world(ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        let is_root = document_first_child(&parsed, |data| {
            matches!(data, NodeData::Element(_) | NodeData::AnonymousBlock(_))
        }) == Some(self.handle.0);
        if !is_root {
            return Ok(());
        }
        let viewport = parsed.document.base.viewport_scroll();
        parsed.document.base.set_viewport_scroll(blitz_dom::Point {
            x: viewport.x,
            y: value,
        });
        Ok(())
    }

    // https://html.spec.whatwg.org/multipage/forms.html#dom-fae-form
    #[qjs(skip)]
    fn form<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let form = {
            let world = world(&ctx)?;
            let world = world.borrow();
            world.document(self.handle.0).and_then(|parsed| {
                form_owner_of(
                    &parsed.document.base,
                    self.handle.0.document,
                    self.handle.0.node,
                )
            })
        };
        match form {
            Some(form) => wrap_node(&ctx, form),
            None => Ok(Value::new_null(ctx)),
        }
    }

    // https://html.spec.whatwg.org/multipage/input.html#dom-input-checked
    //
    // Blitz keeps checkedness as the `checked` content attribute's presence.
    #[qjs(skip)]
    fn checked(&self, ctx: Ctx<'_>) -> Result<bool> {
        let world = world(&ctx)?;
        let world = world.borrow();
        Ok(world.document(self.handle.0).is_some_and(|parsed| {
            attr(&parsed.document.base, self.handle.0.node, "checked").is_some()
        }))
    }

    #[qjs(skip)]
    fn set_checked(&self, ctx: Ctx<'_>, value: bool) -> Result<()> {
        if value {
            // Setting a radio's checkedness unchecks its group
            // (<https://html.spec.whatwg.org/multipage/input.html#dom-input-checked>).
            let group = {
                let world = world(&ctx)?;
                let world = world.borrow();
                let Some(parsed) = world.document(self.handle.0) else {
                    return Ok(());
                };
                let base = &parsed.document.base;
                let is_radio = base
                    .get_node(self.handle.0.node)
                    .and_then(|node| node.data.downcast_element())
                    .is_some_and(|element| {
                        element.name.ns == html_namespace()
                            && element.name.local.as_ref() == "input"
                            && attr(base, self.handle.0.node, "type")
                                .unwrap_or("text")
                                .trim()
                                .eq_ignore_ascii_case("radio")
                    });
                if is_radio {
                    let name = attr(base, self.handle.0.node, "name")
                        .unwrap_or_default()
                        .to_owned();
                    radio_group_members(base, self.handle.0.document, &name, self.handle.0.node)
                } else {
                    Vec::new()
                }
            };
            for member in group {
                remove_attribute_sync(&ctx, member, "", "checked", false)?;
            }
            set_attribute_sync(&ctx, self.handle.0, "checked", "")
        } else {
            remove_attribute_sync(&ctx, self.handle.0, "", "checked", false)
        }
    }

    // https://html.spec.whatwg.org/multipage/input.html#dom-input-indeterminate
    #[qjs(skip)]
    fn indeterminate(&self) -> bool {
        // Known gap: Blitz keeps no indeterminate slot, so inputs never read
        // as indeterminate.
        false
    }

    #[qjs(skip)]
    fn set_indeterminate(&self) {
        // Known gap: Blitz keeps no indeterminate slot, so the setter cannot
        // persist and is a no-op.
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-selected
    //
    // Blitz keeps selectedness as the `selected` content attribute's presence.
    #[qjs(skip)]
    fn selected(&self, ctx: Ctx<'_>) -> Result<bool> {
        let world = world(&ctx)?;
        let world = world.borrow();
        Ok(world.document(self.handle.0).is_some_and(|parsed| {
            attr(&parsed.document.base, self.handle.0.node, "selected").is_some()
        }))
    }

    #[qjs(skip)]
    fn set_selected(&self, ctx: Ctx<'_>, value: bool) -> Result<()> {
        if value {
            // Selecting an option in a single-select deselects the others
            // (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-selected>).
            let siblings = {
                let world = world(&ctx)?;
                let world = world.borrow();
                let Some(parsed) = world.document(self.handle.0) else {
                    return Ok(());
                };
                let base = &parsed.document.base;
                match option_select_owner(base, self.handle.0.document, self.handle.0.node) {
                    Some(owner) if attr(base, owner.node, "multiple").is_none() => {
                        select_options_in(base, self.handle.0.document, owner.node)
                    }
                    _ => Vec::new(),
                }
            };
            for option in siblings {
                if option != self.handle.0 {
                    remove_attribute_sync(&ctx, option, "", "selected", false)?;
                }
            }
            set_attribute_sync(&ctx, self.handle.0, "selected", "")
        } else {
            remove_attribute_sync(&ctx, self.handle.0, "", "selected", false)
        }
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-label
    #[qjs(skip)]
    fn label(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        let world = world.borrow();
        Ok(world
            .document(self.handle.0)
            .map_or_else(String::new, |parsed| {
                let base = &parsed.document.base;
                attr(base, self.handle.0.node, "label")
                    .map_or_else(|| option_text_in(base, self.handle.0.node), str::to_owned)
            }))
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-text
    #[qjs(skip)]
    fn text(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        let world = world.borrow();
        Ok(world
            .document(self.handle.0)
            .map_or_else(String::new, |parsed| {
                option_text_in(&parsed.document.base, self.handle.0.node)
            }))
    }

    #[qjs(skip)]
    fn set_text(&self, ctx: Ctx<'_>, value: WebIdlCodeUnits) -> Result<()> {
        // An option's text setter replaces its children with one text node
        // (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-text>).
        let text = value.0.to_string_lossy().into_owned();
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        let added = if text.is_empty() {
            Vec::new()
        } else {
            let text = parsed.document.base.mutate().create_text_node(&text);
            vec![NodeId {
                document: parsed.id,
                node: text,
            }]
        };
        replace_all_journaled(&mut parsed, self.handle.0, added);
        drop(parsed);
        drop(world);
        schedule_mutation_delivery(&ctx)
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-index
    #[qjs(skip)]
    fn index(&self, ctx: Ctx<'_>) -> Result<i32> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(parsed) = world.document(self.handle.0) else {
            return Ok(0);
        };
        let base = &parsed.document.base;
        let Some(select) = option_select_owner(base, self.handle.0.document, self.handle.0.node)
        else {
            return Ok(0);
        };
        for (index, option) in select_options_in(base, self.handle.0.document, select.node)
            .into_iter()
            .enumerate()
        {
            if option == self.handle.0 {
                return Ok(i32::try_from(index).unwrap_or(i32::MAX));
            }
        }
        Ok(0)
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-selectedindex
    #[qjs(skip)]
    fn selected_index(&self, ctx: Ctx<'_>) -> Result<i32> {
        let world = world(&ctx)?;
        let world = world.borrow();
        Ok(world.document(self.handle.0).map_or(-1, |parsed| {
            let base = &parsed.document.base;
            select_options_in(base, self.handle.0.document, self.handle.0.node)
                .iter()
                .position(|option| attr(base, option.node, "selected").is_some())
                .map_or(-1, |index| i32::try_from(index).unwrap_or(-1))
        }))
    }

    #[qjs(skip)]
    fn set_selected_index(&self, ctx: Ctx<'_>, value: i32) -> Result<()> {
        let world_rc = world(&ctx)?;
        let options = {
            let world = world_rc.borrow();
            let Some(parsed) = world.document(self.handle.0) else {
                return Ok(());
            };
            select_options_in(
                &parsed.document.base,
                self.handle.0.document,
                self.handle.0.node,
            )
        };
        // A negative index clears every option; otherwise the indexed option
        // selects and the rest deselect
        // (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-selectedindex>).
        let wanted = if value < 0 {
            None
        } else {
            usize::try_from(value).ok()
        };
        for (index, option) in options.into_iter().enumerate() {
            if Some(index) == wanted {
                set_attribute_sync(&ctx, option, "selected", "")?;
            } else {
                remove_attribute_sync(&ctx, option, "", "selected", false)?;
            }
        }
        Ok(())
    }

    #[qjs(skip)]
    fn options<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        live_collection(
            &ctx,
            self.handle.0,
            CollectionKind::SelectOptions,
            Some("HTMLOptionsCollection"),
        )
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-selectedoptions
    #[qjs(skip)]
    fn selected_options<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        live_collection(
            &ctx,
            self.handle.0,
            CollectionKind::SelectedOptions,
            Some("HTMLCollection"),
        )
    }

    // https://html.spec.whatwg.org/multipage/embedded-content.html#dom-img-naturalwidth
    #[qjs(skip)]
    fn natural_width(&self, ctx: Ctx<'_>) -> Result<u32> {
        Ok(img_size(&ctx, self.handle.0)?.map_or(0, |(width, _)| width))
    }

    // https://html.spec.whatwg.org/multipage/embedded-content.html#dom-img-naturalheight
    #[qjs(skip)]
    fn natural_height(&self, ctx: Ctx<'_>) -> Result<u32> {
        Ok(img_size(&ctx, self.handle.0)?.map_or(0, |(_, height)| height))
    }

    // https://html.spec.whatwg.org/multipage/embedded-content.html#dom-img-complete
    #[qjs(skip)]
    fn complete(&self, ctx: Ctx<'_>) -> Result<bool> {
        if !with_node_data(&ctx, self.handle.0, |data| is_html_element(data, "img"))? {
            return Ok(false);
        }
        let world = world(&ctx)?;
        let world = world.borrow();
        if world.image_loading.contains(&self.handle.0) {
            return Ok(false);
        }
        if world.images.contains_key(&self.handle.0) {
            return Ok(true);
        }
        if world.image_broken.contains(&self.handle.0) {
            return Ok(true);
        }
        let src = world
            .document(self.handle.0)
            .and_then(|parsed| {
                attr(&parsed.document.base, self.handle.0.node, "src").map(str::to_owned)
            });
        if src.as_deref().is_none_or(str::is_empty) {
            return Ok(true);
        }
        Ok(world.image_broken.contains(&self.handle.0))
    }

    // https://html.spec.whatwg.org/multipage/embedded-content.html#dom-img-currentsrc
    #[qjs(skip)]
    fn current_src(&self, ctx: Ctx<'_>) -> Result<String> {
        if !with_node_data(&ctx, self.handle.0, |data| is_html_element(data, "img"))? {
            return Ok(String::new());
        }
        let world = world(&ctx)?;
        Ok(world
            .borrow()
            .image_current_src
            .get(&self.handle.0)
            .cloned()
            .unwrap_or_default())
    }

    // https://html.spec.whatwg.org/multipage/iframe-embed-object.html#dom-iframe-contentdocument
    #[qjs(skip)]
    fn content_document<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        if !iframe_frame(&ctx, self.handle.0)? {
            return Ok(Value::new_null(ctx));
        }
        let world = world(&ctx)?;
        let root = world.borrow().frame_document(self.handle.0);
        match root {
            Some(root) => {
                let parent_origin = world.borrow().document_url.origin();
                let child_origin = world
                    .borrow()
                    .owner_world(root)
                    .map(|child| child.borrow().document_url.origin());
                if child_origin.is_some_and(|origin| origin == parent_origin) {
                    wrap_node(&ctx, root)
                } else {
                    Ok(Value::new_null(ctx))
                }
            }
            None => Ok(Value::new_null(ctx)),
        }
    }

    // https://html.spec.whatwg.org/multipage/iframe-embed-object.html#dom-iframe-contentwindow
    #[qjs(skip)]
    fn content_window<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        if !iframe_frame(&ctx, self.handle.0)? {
            return Ok(Value::new_null(ctx));
        }
        let frame = world(&ctx)?.borrow().frame_for_container(self.handle.0);
        match frame {
            Some(frame) => {
                let proxy: Function = crate::js::bridge::object(&ctx)?.get("__tbFrameProxy")?;
                proxy.call((crate::js::js_number(frame.get()),))
            }
            None => Ok(Value::new_null(ctx)),
        }
    }

    // https://dom.spec.whatwg.org/#dom-element-attachshadow
    #[qjs(skip)]
    fn attach_shadow<'js>(
        &self,
        ctx: Ctx<'js>,
        init: element_generated::ShadowRootInit,
    ) -> Result<Value<'js>> {
        let _ = (self, init);
        // Known gap: Blitz has no shadow DOM, so no element can host a shadow
        // root. Every call throws instead of handing back a half-built root.
        Err(throw_dom(
            &ctx,
            "NotSupportedError",
            "shadow DOM is not supported",
        ))
    }

    #[qjs(skip)]
    fn shadow_root<'js>(&self, ctx: &Ctx<'js>) -> Value<'js> {
        // Known gap: Blitz has no shadow DOM, so elements never host a shadow
        // root.
        Value::new_null(ctx.clone())
    }

    #[qjs(skip)]
    fn host<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        // Known gap: Blitz has no shadow DOM, so no node is ever a shadow
        // root. The brand check rejects every receiver first.
        Err(Exception::throw_type(ctx, "not a shadow root"))
    }

    #[qjs(skip)]
    fn mode(&self, ctx: &Ctx<'_>) -> Result<String> {
        // Known gap: Blitz has no shadow DOM, so no node is ever a shadow
        // root. The brand check rejects every receiver first.
        Err(Exception::throw_type(ctx, "not a shadow root"))
    }

    // https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-element-innerhtml
    #[qjs(skip)]
    fn inner_html<'js>(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let world = world(ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Err(Exception::throw_type(ctx, "no document"));
        };
        let base = &parsed.document.base;
        let Some(node) = base.get_node(self.handle.0.node) else {
            return Err(Exception::throw_type(ctx, "stale node"));
        };
        if node.data.downcast_element().is_none() {
            return Err(Exception::throw_type(ctx, "innerHTML requires an element"));
        }
        let markup = if parsed.content_type == "text/html" {
            serialize_html_children(&parsed.document, self.handle.0.node)
        } else {
            serialize_xml_children(&parsed.document, self.handle.0.node)
        };
        dom_string(ctx, &markup)
    }

    // https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-element-outerhtml
    #[qjs(skip)]
    fn outer_html<'js>(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let world = world(ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Err(Exception::throw_type(ctx, "no document"));
        };
        let base = &parsed.document.base;
        let Some(element) = base
            .get_node(self.handle.0.node)
            .and_then(|node| node.data.downcast_element())
        else {
            return Err(Exception::throw_type(ctx, "outerHTML requires an element"));
        };
        let name = element.name.clone();
        let attributes = element.attrs.iter().cloned().collect::<Vec<_>>();
        let markup = if parsed.content_type == "text/html" {
            serialize_html_outer(&parsed.document, self.handle.0.node, &name, &attributes)
        } else {
            let mut output = HtmlOutput(Vec::new());
            serialize_xml_element(&parsed.document, self.handle.0.node, &name, &attributes, &mut output);
            output.finish()
        };
        dom_string(ctx, &markup)
    }

    // https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-element-innerhtml
    #[qjs(skip)]
    fn set_inner_html(&self, ctx: &Ctx<'_>, value: LegacyNullString) -> Result<()> {
        let context = with_node_data(ctx, self.handle.0, |data| match data {
            Some(NodeData::Element(element) | NodeData::AnonymousBlock(element)) => {
                Some(html_fragment_context(&element.name))
            }
            _ => None,
        })?
        .ok_or_else(|| {
            Exception::throw_type(ctx, "innerHTML requires an element or shadow root")
        })?;

        let snapshots = { let base_url = fragment_base_url(ctx, self.handle.0); parse_html_fragment_snapshots(&value.0, &context, &base_url) };

        // Known gap: Blitz has no template contents, so `<template>` children
        // replace as ordinary element children.
        let world = world(ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(ctx, "no document"));
        };
        let mut replacement = materialize_children(&mut parsed.document, &snapshots)
            .map_err(|err| throw_dom_error(ctx, err))?;
        replacement.document = self.handle.0.document;
        let added: Vec<NodeId> = parsed
            .document
            .base
            .get_node(replacement.node)
            .map(|backing| {
                backing
                    .children
                    .iter()
                    .map(|child| NodeId {
                        document: replacement.document,
                        node: *child,
                    })
                    .collect()
            })
            .unwrap_or_default();
        replace_all_journaled(&mut parsed, self.handle.0, added);
        drop(parsed);
        drop(world);
        schedule_mutation_delivery(ctx)
    }

    // https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-element-insertadjacenthtml
    #[qjs(skip)]
    fn insert_adjacent_html(
        &self,
        ctx: Ctx<'_>,
        position: WebIdlString,
        text: WebIdlString,
    ) -> Result<()> {
        let position = position.0.to_ascii_lowercase();
        if !matches!(
            position.as_str(),
            "beforebegin" | "afterbegin" | "beforeend" | "afterend"
        ) {
            return Err(throw_dom(&ctx, "SyntaxError", "invalid position"));
        }
        let context = with_node_data(&ctx, self.handle.0, |data| match data {
            Some(NodeData::Element(element) | NodeData::AnonymousBlock(element)) => {
                Some(html_fragment_context(&element.name))
            }
            _ => None,
        })?
        .ok_or_else(|| Exception::throw_type(&ctx, "insertAdjacentHTML requires an element"))?;
        let snapshots = { let base_url = fragment_base_url(&ctx, self.handle.0); parse_html_fragment_snapshots(&text.0, &context, &base_url) };
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        let needs_parent = matches!(position.as_str(), "beforebegin" | "afterend");
        let parent = parsed
            .document
            .base
            .get_node(self.handle.0.node)
            .and_then(|node| node.parent)
            .map(|node| NodeId {
                document: self.handle.0.document,
                node,
            });
        if needs_parent && parent.is_none() {
            return Err(throw_dom(
                &ctx,
                "NoModificationAllowedError",
                "element has no parent",
            ));
        }
        let mut fragment = materialize_children(&mut parsed.document, &snapshots)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        fragment.document = self.handle.0.document;
        let moved: Vec<NodeId> = parsed
            .document
            .base
            .get_node(fragment.node)
            .map(|backing| {
                backing
                    .children
                    .iter()
                    .map(|child| NodeId {
                        document: fragment.document,
                        node: *child,
                    })
                    .collect()
            })
            .unwrap_or_default();
        match position.as_str() {
            "beforebegin" => {
                let parent = parent.ok_or_else(|| {
                    throw_dom(&ctx, "NoModificationAllowedError", "element has no parent")
                })?;
                for child in moved {
                    place_journaled(&mut parsed, parent, child, Some(self.handle.0));
                }
            }
            _ => place_adjacent_rest(
                &ctx,
                &mut parsed,
                position.as_str(),
                self.handle.0,
                moved,
            )?,
        }
        drop(parsed);
        drop(world);
        schedule_mutation_delivery(&ctx)
    }

    // https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-element-outerhtml
    #[qjs(skip)]
    fn set_outer_html(&self, ctx: &Ctx<'_>, value: LegacyNullString) -> Result<()> {
        let world_rc = world(ctx)?;
        let (parent, context) = {
            let world = world_rc.borrow();
            let Some(parsed) = world.document(self.handle.0) else {
                return Err(Exception::throw_type(ctx, "no document"));
            };
            let base = &parsed.document.base;
            let Some(parent) = base
                .get_node(self.handle.0.node)
                .and_then(|node| node.parent)
                .map(|node| NodeId {
                    document: self.handle.0.document,
                    node,
                })
            else {
                // A parentless element has nothing to replace.
                return Ok(());
            };
            if base.root_node().id == parent.node {
                return Err(throw_dom(
                    ctx,
                    "NoModificationAllowedError",
                    "the parent of the element is a Document",
                ));
            }
            // A DocumentFragment has no parsing context of its own; a
            // body element stands in
            // (<https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-element-outerhtml>).
            let context = base
                .get_node(parent.node)
                .and_then(|node| node.data.downcast_element())
                .map_or_else(
                    || "body".to_owned(),
                    |element| html_fragment_context(&element.name),
                );
            (parent, context)
        };
        let snapshots = { let base_url = fragment_base_url(ctx, self.handle.0); parse_html_fragment_snapshots(&value.0, &context, &base_url) };

        let world = world_rc.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(ctx, "no document"));
        };
        let mut replacement = materialize_children(&mut parsed.document, &snapshots)
            .map_err(|err| throw_dom_error(ctx, err))?;
        replacement.document = self.handle.0.document;
        let target = self.handle.0;
        replace_parsed(&mut parsed, parent, replacement, target);
        drop(parsed);
        drop(world);
        schedule_mutation_delivery(ctx)
    }

    #[qjs(skip)]
    fn style<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let world_rc = world_for_node(ctx, self.handle.0)?;
        if let Some(saved) = world_rc
            .borrow()
            .wrapper(self.handle.0, Wrapper::StyleDeclaration)
            && let Some(value) = deref_weak(ctx, saved)?
        {
            return Ok(value);
        }
        let factory: Function = crate::js::bridge::object(ctx)?.get("__tbMakeStyle")?;
        let element = wrap_node(ctx, self.handle.0)?;
        let value: Value = factory.call((element,))?;
        let weak = make_weak(ctx, value.clone())?;
        world_rc.borrow_mut().intern_wrapper(
            self.handle.0,
            Wrapper::StyleDeclaration,
            Persistent::save(ctx, weak),
        );
        Ok(value)
    }

    // https://dom.spec.whatwg.org/#dom-node-lastchild
    #[qjs(skip)]
    fn last_child<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let world = world(ctx)?;
        let id = world.borrow().document(self.handle.0).and_then(|parsed| {
            parsed
                .document
                .base
                .get_node(self.handle.0.node)
                .and_then(|node| node.children.last().copied())
                .map(|node| NodeId {
                    document: parsed.id,
                    node,
                })
        });
        child_value(ctx, id)
    }

    // https://dom.spec.whatwg.org/#dom-node-nextsibling
    #[qjs(skip)]
    fn next_sibling<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        sibling_value(ctx, self.handle.0, true)
    }

    // https://dom.spec.whatwg.org/#dom-node-previoussibling
    #[qjs(skip)]
    fn previous_sibling<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        sibling_value(ctx, self.handle.0, false)
    }

    // https://dom.spec.whatwg.org/#dom-node-ownerdocument
    #[qjs(skip)]
    fn owner_document<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let world = world(ctx)?;
        let id = world.borrow().document(self.handle.0).and_then(|parsed| {
            let root = parsed.document.base.root_node().id;
            (root != self.handle.0.node).then_some(NodeId {
                document: parsed.id,
                node: root,
            })
        });
        child_value(ctx, id)
    }

    // https://dom.spec.whatwg.org/#dom-node-haschildnodes
    #[qjs(skip)]
    fn has_child_nodes(&self, ctx: Ctx<'_>) -> Result<bool> {
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(false);
        };
        Ok(parsed
            .document
            .base
            .get_node(self.handle.0.node)
            .is_some_and(|node| !node.children.is_empty()))
    }

    // https://dom.spec.whatwg.org/#dom-node-nodevalue
    #[qjs(skip)]
    fn node_value<'js>(&self, ctx: &Ctx<'js>) -> Result<Option<rquickjs::String<'js>>> {
        let world = world(ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(None);
        };
        let synthetic = parsed.document.synthetic_kind(self.handle.0.node).cloned();
        let data = parsed
            .document
            .base
            .get_node(self.handle.0.node)
            .map(|node| &node.data);
        let value = match (synthetic, data) {
            (_, Some(NodeData::Text(text))) => Some(text.content.clone()),
            (
                Some(crate::documents::SyntheticKind::Pi { target }),
                Some(NodeData::Comment { contents }),
            ) => Some(
                contents
                    .strip_prefix(target.as_str())
                    .map_or(contents.clone(), |rest| {
                        rest.strip_prefix(' ').unwrap_or(rest).to_owned()
                    }),
            ),
            (Some(crate::documents::SyntheticKind::Doctype { .. }), _) => None,
            (_, Some(NodeData::Comment { contents })) => Some(contents.clone()),
            _ => None,
        };
        match value {
            // Doctype and non-character-data nodes have a null node value.
            // A synthetic doctype matches no arm above, so it lands here.
            Some(value) => dom_string(ctx, &DomString::from(value)).map(Some),
            None => Ok(None),
        }
    }

    // https://dom.spec.whatwg.org/#dom-node-nodevalue
    #[qjs(skip)]
    fn set_node_value(&self, ctx: &Ctx<'_>, value: Option<rquickjs::String<'_>>) -> Result<()> {
        let value = match value {
            Some(value) => crate::dom_string::DomString::from_utf16(value.to_utf16()?),
            None => crate::dom_string::DomString::default(),
        };
        set_character_data(ctx, self.handle.0, &value)
    }

    // https://dom.spec.whatwg.org/#dom-node-textcontent
    #[qjs(skip)]
    fn text_content<'js>(&self, ctx: &Ctx<'js>) -> Result<Option<rquickjs::String<'js>>> {
        let world = world(ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(None);
        };
        let base = &parsed.document.base;
        let synthetic = parsed.document.synthetic_kind(self.handle.0.node).cloned();
        let data = base.get_node(self.handle.0.node).map(|node| &node.data);
        match (synthetic, data) {
            (_, Some(NodeData::Element(_) | NodeData::AnonymousBlock(_))) => {
                let text = descendant_text(&parsed.document, self.handle.0.node);
                dom_string(ctx, &text).map(Some)
            }
            (_, Some(NodeData::Text(text))) => {
                dom_string(ctx, &DomString::from(text.content.clone())).map(Some)
            }
            (
                Some(crate::documents::SyntheticKind::Pi { target }),
                Some(NodeData::Comment { contents }),
            ) => {
                let data = contents
                    .strip_prefix(target.as_str())
                    .map_or(contents.clone(), |rest| {
                        rest.strip_prefix(' ').unwrap_or(rest).to_owned()
                    });
                dom_string(ctx, &DomString::from(data)).map(Some)
            }
            (Some(crate::documents::SyntheticKind::Doctype { .. }), _) => Ok(None),
            (_, Some(NodeData::Comment { contents })) => {
                dom_string(ctx, &DomString::from(contents.clone())).map(Some)
            }
            _ => Ok(None),
        }
    }

    #[qjs(skip)]
    fn set_text_content(&self, ctx: &Ctx<'_>, value: Option<rquickjs::String<'_>>) -> Result<()> {
        let text = match value {
            Some(value) => crate::dom_string::DomString::from_utf16(value.to_utf16()?),
            None => crate::dom_string::DomString::default(),
        };
        // A `CharacterData` node [replaces its data] in place; `Document`
        // nodes ignore the setter; every other node replaces its children
        // with one `Text` node
        // (<https://dom.spec.whatwg.org/#dom-node-textcontent>).
        let world_rc = world(ctx)?;
        let character_data = {
            let world = world_rc.borrow();
            let Some(parsed) = world.document(self.handle.0) else {
                return Ok(());
            };
            parsed
                .document
                .base
                .get_node(self.handle.0.node)
                .is_some_and(|node| {
                    matches!(node.data, NodeData::Text(_) | NodeData::Comment { .. })
                })
        };
        if character_data {
            return set_character_data(ctx, self.handle.0, &text);
        }
        let world = world_rc.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        if parsed.document.base.root_node().id == self.handle.0.node {
            return Ok(());
        }
        let text = text.to_string_lossy().into_owned();
        let added = if text.is_empty() {
            Vec::new()
        } else {
            let text = parsed.document.base.mutate().create_text_node(&text);
            vec![NodeId {
                document: parsed.id,
                node: text,
            }]
        };
        replace_all_journaled(&mut parsed, self.handle.0, added);
        drop(parsed);
        drop(world);
        schedule_mutation_delivery(ctx)
    }

    // https://dom.spec.whatwg.org/#dom-element-matches
    #[qjs(skip)]
    fn matches(&self, ctx: Ctx<'_>, selectors: WebIdlString) -> Result<bool> {
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(false);
        };
        parsed
            .document
            .base
            .matches_selector(self.handle.0.node, &selectors.0)
            .map_err(|err| select_error(&ctx, &err))
    }

    // https://dom.spec.whatwg.org/#dom-element-closest
    #[qjs(skip)]
    fn closest<'js>(&self, ctx: Ctx<'js>, selectors: WebIdlString) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let found = {
            let parsed = world.borrow();
            let Some(parsed) = parsed.document(self.handle.0) else {
                return Ok(Value::new_null(ctx));
            };
            parsed
                .document
                .base
                .closest(self.handle.0.node, &selectors.0)
                .map_err(|err| select_error(&ctx, &err))?
                .map(|node| NodeId {
                    document: parsed.id,
                    node,
                })
        };
        child_value(&ctx, found)
    }

    // https://dom.spec.whatwg.org/#dom-element-getelementsbyclassname
    #[qjs(skip)]
    fn get_elements_by_class_name<'js>(
        &self,
        ctx: Ctx<'js>,
        names: WebIdlString,
    ) -> Result<Value<'js>> {
        live_collection(
            &ctx,
            self.handle.0,
            CollectionKind::ElementsByClass(names.0),
            Some("HTMLCollection"),
        )
    }

    // https://dom.spec.whatwg.org/#dom-document-title
    #[qjs(skip)]
    fn title<'js>(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let world = world(ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return rquickjs::String::from_str(ctx.clone(), "");
        };
        let title = document_first(&parsed, "title");
        match title {
            Some(title) => dom_string(ctx, &descendant_text(&parsed.document, title.node)),
            None => rquickjs::String::from_str(ctx.clone(), ""),
        }
    }

    #[qjs(skip)]
    fn set_title(&self, ctx: &Ctx<'_>, value: WebIdlCodeUnits) -> Result<()> {
        let text = value.0.to_string_lossy().into_owned();
        let world = world(ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        let base_root = parsed.document.base.root_node().id;
        let document = parsed.id;
        let title = parsed
            .document
            .base
            .query_selector_in(base_root, "title")
            .ok()
            .flatten()
            .map_or_else(
                || {
                    let name = QualName::new(None, html_namespace(), LocalName::from("title"));
                    let title = parsed
                        .document
                        .base
                        .mutate()
                        .create_element(name, Vec::new());
                    let title = NodeId {
                        document,
                        node: title,
                    };
                    let target = parsed
                        .document
                        .base
                        .query_selector_in(base_root, "head")
                        .ok()
                        .flatten()
                        .or_else(|| {
                            parsed
                                .document
                                .base
                                .query_selector_in(base_root, "html")
                                .ok()
                                .flatten()
                        })
                        .map(|node| NodeId { document, node });
                    if let Some(target) = target {
                        insert_parsed(&mut parsed, target, title, None);
                    }
                    title
                },
                |node| NodeId { document, node },
            );
        let added = if text.is_empty() {
            Vec::new()
        } else {
            let text = parsed.document.base.mutate().create_text_node(&text);
            vec![NodeId {
                document: parsed.id,
                node: text,
            }]
        };
        replace_all_journaled(&mut parsed, title, added);
        drop(parsed);
        drop(world);
        schedule_mutation_delivery(ctx)
    }

    // https://html.spec.whatwg.org/multipage/dom.html#dom-document-getelementsbyname
    #[qjs(skip)]
    fn get_elements_by_name<'js>(&self, ctx: Ctx<'js>, name: WebIdlString) -> Result<Value<'js>> {
        live_collection(
            &ctx,
            self.handle.0,
            CollectionKind::ElementsByName(name.0),
            None,
        )
    }

    // https://dom.spec.whatwg.org/#dom-element-getelementsbytagnamens
    #[qjs(skip)]
    fn get_elements_by_tag_name_ns<'js>(
        &self,
        ctx: Ctx<'js>,
        namespace: OptString,
        local: WebIdlString,
    ) -> Result<Value<'js>> {
        // The namespace variant matches the local name exactly, in HTML
        // documents too: the lowercasing rule belongs to
        // `getElementsByTagName` alone
        // (<https://dom.spec.whatwg.org/#concept-getelementsbytagnamens>).
        live_collection(
            &ctx,
            self.handle.0,
            CollectionKind::ElementsByTagNs {
                namespace: namespace.0.unwrap_or_default(),
                local: local.0,
            },
            Some("HTMLCollection"),
        )
    }

    // ── Element identity and attributes ─────────────────────────────────

    // https://dom.spec.whatwg.org/#dom-element-tagname
    #[qjs(skip)]
    fn tag_name(&self, ctx: &Ctx<'_>) -> Result<String> {
        let uppercase = document_is_html_content(ctx, self.handle.0);
        with_node_data(ctx, self.handle.0, |data| match data {
            Some(NodeData::Element(element) | NodeData::AnonymousBlock(element)) => {
                element_node_name(&element.name, uppercase)
            }
            _ => String::new(),
        })
    }

    // https://dom.spec.whatwg.org/#dom-element-localname
    #[qjs(skip)]
    fn local_name(&self, ctx: &Ctx<'_>) -> Result<String> {
        with_node_data(ctx, self.handle.0, |data| match data {
            Some(NodeData::Element(element) | NodeData::AnonymousBlock(element)) => {
                element.name.local.to_string()
            }
            _ => String::new(),
        })
    }

    // https://dom.spec.whatwg.org/#dom-element-prefix
    #[qjs(skip)]
    fn prefix<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let prefix = with_node_data(ctx, self.handle.0, |data| match data {
            Some(NodeData::Element(element) | NodeData::AnonymousBlock(element)) => element
                .name
                .prefix
                .as_ref()
                .filter(|prefix| !prefix.is_empty())
                .map(ToString::to_string),
            _ => None,
        })?;
        match prefix {
            Some(prefix) => string_value(ctx, &prefix),
            None => Ok(Value::new_null(ctx.clone())),
        }
    }

    // https://dom.spec.whatwg.org/#dom-element-namespaceuri
    #[qjs(skip)]
    fn namespace_uri<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let namespace = with_node_data(ctx, self.handle.0, |data| match data {
            Some(NodeData::Element(element) | NodeData::AnonymousBlock(element)) => {
                (!element.name.ns.as_ref().is_empty()).then(|| element.name.ns.to_string())
            }
            _ => None,
        })?;
        match namespace {
            Some(namespace) => string_value(ctx, &namespace),
            None => Ok(Value::new_null(ctx.clone())),
        }
    }

    // https://dom.spec.whatwg.org/#dom-element-classname
    #[qjs(skip)]
    fn class_name(&self, ctx: &Ctx<'_>) -> Result<String> {
        attribute_value(ctx, self.handle.0, "class")
    }

    #[qjs(skip)]
    fn set_class_name(&self, ctx: &Ctx<'_>, value: WebIdlString) -> Result<()> {
        set_attribute_sync(ctx, self.handle.0, "class", &value.0)
    }

    // https://dom.spec.whatwg.org/#dom-element-classlist
    #[qjs(skip)]
    fn class_list<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let world_rc = world(ctx)?;
        if let Some(saved) = world_rc.borrow().wrapper(self.handle.0, Wrapper::TokenList)
            && let Some(value) = deref_weak(ctx, saved)?
        {
            return Ok(value);
        }
        let class = super::host::instance_for_node(
            ctx,
            self.handle.0,
            JsTokenList {
                element: self.handle,
            },
        )?;
        let value = Class::into_value(class);
        let weak = make_weak(ctx, value.clone())?;
        world_rc.borrow_mut().intern_wrapper(
            self.handle.0,
            Wrapper::TokenList,
            Persistent::save(ctx, weak),
        );
        Ok(value)
    }

    // https://html.spec.whatwg.org/multipage/dom.html#concept-domstringmap-pairs
    #[qjs(skip)]
    fn dataset<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let world_rc = world_for_node(ctx, self.handle.0)?;
        if let Some(saved) = world_rc.borrow().wrapper(self.handle.0, Wrapper::Dataset)
            && let Some(value) = deref_weak(ctx, saved)?
        {
            return Ok(value);
        }
        let factory: Function = crate::js::bridge::object(ctx)?.get("__tbMakeDataset")?;
        let element = wrap_node(ctx, self.handle.0)?;
        let value: Value = factory.call((element,))?;
        let weak = make_weak(ctx, value.clone())?;
        world_rc.borrow_mut().intern_wrapper(
            self.handle.0,
            Wrapper::Dataset,
            Persistent::save(ctx, weak),
        );
        Ok(value)
    }

    // https://dom.spec.whatwg.org/#dom-element-hasattribute
    #[qjs(skip)]
    fn has_attribute(&self, ctx: Ctx<'_>, name: WebIdlString) -> Result<bool> {
        if !valid_attribute_local_name(&name.0) {
            return Ok(false);
        }
        // HTML elements match ASCII-lowercased, like `getAttribute`.
        let local = attribute_local_name(&ctx, self.handle.0, &name.0);
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(false);
        };
        Ok(attr(&parsed.document.base, self.handle.0.node, &local).is_some())
    }

    // https://dom.spec.whatwg.org/#dom-element-getattributens
    #[qjs(skip)]
    fn get_attribute_ns<'js>(
        &self,
        ctx: Ctx<'js>,
        namespace: OptString,
        local: WebIdlString,
    ) -> Result<Value<'js>> {
        let namespace = namespace.0.unwrap_or_default();
        let world = world(&ctx)?;
        let found = world.borrow().document(self.handle.0).and_then(|parsed| {
            attr_ns_value(
                &parsed.document.base,
                self.handle.0.node,
                &namespace,
                &local.0,
            )
        });
        match found {
            Some(value) => string_value(&ctx, &value),
            None => Ok(Value::new_null(ctx)),
        }
    }

    // https://dom.spec.whatwg.org/#dom-element-hasattributens
    #[qjs(skip)]
    fn has_attribute_ns(
        &self,
        ctx: Ctx<'_>,
        namespace: OptString,
        local: WebIdlString,
    ) -> Result<bool> {
        let namespace = namespace.0.unwrap_or_default();
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(false);
        };
        Ok(attr_ns_value(
            &parsed.document.base,
            self.handle.0.node,
            &namespace,
            &local.0,
        )
        .is_some())
    }

    // https://dom.spec.whatwg.org/#dom-element-setattributens
    #[qjs(skip)]
    fn set_attribute_ns(
        &self,
        ctx: Ctx<'_>,
        namespace: OptString,
        qualified: WebIdlString,
        value: WebIdlString,
    ) -> Result<()> {
        // setAttributeNS never lowercases its name, even on HTML elements
        // (<https://dom.spec.whatwg.org/#dom-element-setattributens>).
        let name = validate_and_extract(
            &ctx,
            namespace.0.as_deref(),
            &qualified.0,
            NodeContext::Attribute,
        )?;
        let namespace = name.ns.to_string();
        let local = name.local.to_string();
        {
            let world = world(&ctx)?;
            let world = world.borrow();
            let Some(mut parsed) = world.document_mut(self.handle.0) else {
                return Err(Exception::throw_type(&ctx, "no document"));
            };
            let old_value = attr_ns_value(
                &parsed.document.base,
                self.handle.0.node,
                &namespace,
                &local,
            );
            // Reuse the stored qualified name when the attribute is already
            // present, so the write replaces instead of duplicating it.
            let stored = parsed
                .document
                .base
                .get_node(self.handle.0.node)
                .and_then(|node| node.data.downcast_element())
                .and_then(|element| {
                    element
                        .attrs
                        .iter()
                        .find(|attribute| {
                            attribute.name.ns.as_ref() == namespace
                                && attribute.name.local.as_ref() == local
                        })
                        .map(|attribute| attribute.name.clone())
                })
                .unwrap_or(name);
            let qualified = qualified_name(&stored);
            parsed
                .document
                .base
                .mutate()
                .set_attribute(self.handle.0.node, stored, &value.0);
            parsed.document.record(JournalEntry::Attributes {
                target: self.handle.0,
                name: qualified,
                namespace: namespace.clone(),
                old_value,
            });
        }
        touch_attr(&ctx, self.handle.0, &namespace, &local, &value.0)?;
        schedule_mutation_delivery(&ctx)
    }

    // https://dom.spec.whatwg.org/#dom-element-removeattribute
    #[qjs(skip)]
    fn remove_attribute(&self, ctx: Ctx<'_>, name: WebIdlString) -> Result<()> {
        if !valid_attribute_local_name(&name.0) {
            return Ok(());
        }
        let local = attribute_local_name(&ctx, self.handle.0, &name.0);
        remove_attribute_sync(&ctx, self.handle.0, "", &local, false)
    }

    // https://dom.spec.whatwg.org/#dom-element-removeattributens
    #[qjs(skip)]
    fn remove_attribute_ns(
        &self,
        ctx: Ctx<'_>,
        namespace: OptString,
        local: WebIdlString,
    ) -> Result<()> {
        let namespace = namespace.0.unwrap_or_default();
        remove_attribute_sync(&ctx, self.handle.0, &namespace, &local.0, true)
    }

    // https://dom.spec.whatwg.org/#dom-element-getattributenames
    #[qjs(skip)]
    fn get_attribute_names(&self, ctx: Ctx<'_>) -> Result<Vec<String>> {
        let world = world(&ctx)?;
        Ok(world
            .borrow()
            .document(self.handle.0)
            .and_then(|parsed| {
                parsed
                    .document
                    .base
                    .get_node(self.handle.0.node)
                    .and_then(|node| node.data.downcast_element())
                    .map(|element| {
                        element
                            .attrs
                            .iter()
                            .map(|attribute| qualified_name(&attribute.name))
                            .collect::<Vec<String>>()
                    })
            })
            .unwrap_or_default())
    }

    // https://dom.spec.whatwg.org/#dom-element-attributes
    #[qjs(skip)]
    fn attributes<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let world_rc = world_for_node(ctx, self.handle.0)?;
        if let Some(saved) = world_rc
            .borrow()
            .wrapper(self.handle.0, Wrapper::NamedNodeMap)
            && let Some(value) = deref_weak(ctx, saved)?
        {
            return Ok(value);
        }
        let class = super::host::instance_for_node(
            ctx,
            self.handle.0,
            JsNamedNodeMap {
                element: self.handle,
            },
        )?;
        let value = Class::into_value(class);
        let weak = make_weak(ctx, value.clone())?;
        world_rc.borrow_mut().intern_wrapper(
            self.handle.0,
            Wrapper::NamedNodeMap,
            Persistent::save(ctx, weak),
        );
        Ok(value)
    }

    // https://dom.spec.whatwg.org/#dom-element-hasattributes
    #[qjs(skip)]
    fn has_attributes(&self, ctx: Ctx<'_>) -> Result<bool> {
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(false);
        };
        Ok(parsed
            .document
            .base
            .get_node(self.handle.0.node)
            .and_then(|node| node.data.downcast_element())
            .is_some_and(|element| !element.attrs.is_empty()))
    }

    // https://dom.spec.whatwg.org/#dom-element-getattributenode
    #[qjs(skip)]
    fn get_attribute_node<'js>(&self, ctx: Ctx<'js>, name: WebIdlString) -> Result<Value<'js>> {
        if !valid_attribute_local_name(&name.0) {
            return Ok(Value::new_null(ctx));
        }
        let name = attribute_local_name(&ctx, self.handle.0, &name.0);
        match attached_attr_id(&ctx, self.handle.0, "", &name)? {
            Some(id) => attr_wrapper(&ctx, id),
            None => Ok(Value::new_null(ctx)),
        }
    }

    // https://dom.spec.whatwg.org/#dom-element-getattributenodens
    #[qjs(skip)]
    fn get_attribute_node_ns<'js>(
        &self,
        ctx: Ctx<'js>,
        namespace: OptString,
        local: WebIdlString,
    ) -> Result<Value<'js>> {
        let namespace = namespace.0.unwrap_or_default();
        match attached_attr_id(&ctx, self.handle.0, &namespace, &local.0)? {
            Some(id) => attr_wrapper(&ctx, id),
            None => Ok(Value::new_null(ctx)),
        }
    }

    // https://dom.spec.whatwg.org/#dom-element-setattributenode
    #[qjs(skip)]
    fn set_attribute_node<'js>(
        &self,
        ctx: Ctx<'js>,
        attr: AttrArgument<'js>,
    ) -> Result<Value<'js>> {
        set_attribute_node(&ctx, self.handle.0, &attr.into_value())
    }

    // https://dom.spec.whatwg.org/#dom-element-setattributenodens
    #[qjs(skip)]
    fn set_attribute_node_ns<'js>(
        &self,
        ctx: Ctx<'js>,
        attr: AttrArgument<'js>,
    ) -> Result<Value<'js>> {
        set_attribute_node(&ctx, self.handle.0, &attr.into_value())
    }

    // https://dom.spec.whatwg.org/#dom-element-removeattributenode
    #[qjs(skip)]
    fn remove_attribute_node<'js>(
        &self,
        ctx: Ctx<'js>,
        attr: AttrArgument<'js>,
    ) -> Result<Value<'js>> {
        let (id, scope) = {
            let attr = attr.borrow();
            (attr.id, attr.scope.0)
        };
        let state = attr_state(&ctx, scope, id)?;
        let attached_here = attr_owner(&ctx, scope, id) == Some(self.handle.0) && {
            let registry = world(&ctx)?.borrow().registry();
            registry
                .borrow()
                .attributes
                .attached(self.handle.0, &state.namespace, &state.local)
                == Some(id)
        };
        if !attached_here {
            return Err(throw_dom(
                &ctx,
                "NotFoundError",
                "attribute is not attached to this element",
            ));
        }
        remove_attribute_sync(&ctx, self.handle.0, &state.namespace, &state.local, true)?;
        Ok(attr.into_value())
    }

    // https://dom.spec.whatwg.org/#dom-element-toggleattribute
    #[qjs(skip)]
    fn toggle_attribute(
        &self,
        ctx: Ctx<'_>,
        name: WebIdlString,
        force: Option<bool>,
    ) -> Result<bool> {
        if !valid_attribute_local_name(&name.0) {
            return Err(throw_dom(
                &ctx,
                "InvalidCharacterError",
                "attribute name is not a valid attribute local name",
            ));
        }
        let local = attribute_local_name(&ctx, self.handle.0, &name.0);
        let exists = {
            let world = world(&ctx)?;
            let world = world.borrow();
            let Some(parsed) = world.document(self.handle.0) else {
                return Err(Exception::throw_type(&ctx, "no document"));
            };
            attr(&parsed.document.base, self.handle.0.node, &local).is_some()
        };
        let should_exist = force.unwrap_or(!exists);
        if should_exist == exists {
            return Ok(should_exist);
        }
        if should_exist {
            set_attribute_sync(&ctx, self.handle.0, &local, "")?;
        } else {
            remove_attribute_sync(&ctx, self.handle.0, "", &local, false)?;
        }
        Ok(should_exist)
    }

    // https://dom.spec.whatwg.org/#dom-node-normalize
    #[qjs(skip)]
    fn normalize(&self, ctx: Ctx<'_>) -> Result<()> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        let document = parsed.id;
        let mut containers: Vec<BlitzId> = vec![self.handle.0.node];
        let mut stack = vec![self.handle.0.node];
        while let Some(id) = stack.pop() {
            if let Some(node) = parsed.document.base.get_node(id) {
                for kid in &node.children.clone() {
                    if parsed
                        .document
                        .base
                        .get_node(*kid)
                        .is_some_and(|candidate| candidate.data.downcast_element().is_some())
                    {
                        containers.push(*kid);
                        stack.push(*kid);
                    }
                }
            }
        }
        for container in containers {
            let kids: Vec<BlitzId> = parsed
                .document
                .base
                .get_node(container)
                .map(|node| node.children.iter().copied().collect())
                .unwrap_or_default();
            // In tree order: drop empty Text nodes, merge contiguous runs
            // into the first non-empty one
            // (<https://dom.spec.whatwg.org/#dom-node-normalize>).
            let mut merged: Option<BlitzId> = None;
            for kid in kids {
                let content =
                    parsed
                        .document
                        .base
                        .get_node(kid)
                        .and_then(|node| match &node.data {
                            NodeData::Text(data) => Some(data.content.clone()),
                            _ => None,
                        });
                let Some(content) = content else {
                    merged = None;
                    continue;
                };
                if content.is_empty() {
                    unlink_journaled(
                        &mut parsed,
                        NodeId {
                            document,
                            node: kid,
                        },
                    );
                    continue;
                }
                if let Some(previous) = merged {
                    let previous_content = parsed
                        .document
                        .base
                        .get_node(previous)
                        .and_then(|node| match &node.data {
                            NodeData::Text(data) => Some(data.content.clone()),
                            _ => None,
                        })
                        .unwrap_or_default();
                    let mut joined = DomString::from(previous_content.clone());
                    joined.push_str(&content);
                    let old_value = DomString::from(previous_content);
                    parsed
                        .document
                        .base
                        .mutate()
                        .set_node_text(previous, &joined.to_string_lossy());
                    parsed.document.record(JournalEntry::CharacterData {
                        target: NodeId {
                            document,
                            node: previous,
                        },
                        old_value,
                    });
                    unlink_journaled(
                        &mut parsed,
                        NodeId {
                            document,
                            node: kid,
                        },
                    );
                } else {
                    merged = Some(kid);
                }
            }
        }
        drop(parsed);
        drop(world);
        schedule_mutation_delivery(&ctx)
    }

    // https://dom.spec.whatwg.org/#dom-node-comparedocumentposition
    #[qjs(skip)]
    fn compare_document_position(&self, ctx: Ctx<'_>, other: NodeReference) -> Result<u16> {
        compare_node_position(&ctx, NodeReference::Tree(self.handle.0), other)
    }

    // https://dom.spec.whatwg.org/#dom-node-parentelement
    #[qjs(skip)]
    fn parent_element<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let world_rc = world(ctx)?;
        let parent = {
            let parsed = world_rc.borrow();
            let Some(parsed) = parsed.document(self.handle.0) else {
                return Ok(Value::new_null(ctx.clone()));
            };
            parsed
                .document
                .base
                .get_node(self.handle.0.node)
                .and_then(|node| node.parent)
                .filter(|parent| is_element(&parsed.document.base, *parent))
                .map(|parent| NodeId {
                    document: parsed.id,
                    node: parent,
                })
        };
        child_value(ctx, parent)
    }

    // https://dom.spec.whatwg.org/#dom-node-issamenode
    #[qjs(skip)]
    #[allow(
        clippy::unnecessary_wraps,
        reason = "generated operations share one fallible call shape"
    )]
    fn is_same_node(&self, _ctx: Ctx<'_>, other: Option<NodeReference>) -> Result<bool> {
        Ok(other == Some(NodeReference::Tree(self.handle.0)))
    }

    // https://dom.spec.whatwg.org/#dom-node-isequalnode
    #[qjs(skip)]
    fn is_equal_node(&self, ctx: Ctx<'_>, other: Option<NodeReference>) -> Result<bool> {
        let Some(NodeReference::Tree(other)) = other else {
            return Ok(false);
        };
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let (Some(first), Some(second)) = (parsed.document(self.handle.0), parsed.document(other))
        else {
            return Ok(false);
        };
        Ok(nodes_equal_in(
            &first.document.base,
            self.handle.0.node,
            &second.document.base,
            other.node,
        ))
    }

    // https://dom.spec.whatwg.org/#dom-node-contains
    #[qjs(skip)]
    fn contains(&self, ctx: Ctx<'_>, other: Option<NodeReference>) -> Result<bool> {
        let Some(NodeReference::Tree(other)) = other else {
            return Ok(false);
        };
        if other.document != self.handle.0.document {
            return Ok(false);
        }
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(false);
        };
        let base = &parsed.document.base;
        let mut cursor = Some(other.node);
        while let Some(id) = cursor {
            if id == self.handle.0.node {
                return Ok(true);
            }
            cursor = base.get_node(id).and_then(|node| node.parent);
        }
        Ok(false)
    }

    // https://dom.spec.whatwg.org/#dom-node-getrootnode
    #[qjs(skip)]
    fn get_root_node<'js>(
        &self,
        ctx: Ctx<'js>,
        options: node_generated::GetRootNodeOptions,
    ) -> Result<Value<'js>> {
        let _ = options.composed;
        // Known gap: Blitz has no shadow DOM, so the composed option never
        // crosses a shadow boundary and the root is the tree root.
        let world = world(&ctx)?;
        let mut root = self.handle.0;
        {
            let parsed = world.borrow();
            let Some(parsed) = parsed.document(self.handle.0) else {
                return Ok(Value::new_null(ctx));
            };
            let base = &parsed.document.base;
            while let Some(parent) = base.get_node(root.node).and_then(|node| node.parent) {
                root = NodeId {
                    document: root.document,
                    node: parent,
                };
            }
        }
        wrap_node(&ctx, root)
    }

    // https://dom.spec.whatwg.org/#dom-node-isconnected
    #[qjs(skip)]
    fn is_connected(&self, ctx: &Ctx<'_>) -> Result<bool> {
        let world = world(ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(false);
        };
        Ok(is_connected(&parsed.document.base, self.handle.0.node))
    }

    // https://dom.spec.whatwg.org/#dom-node-clonenode
    #[qjs(skip)]
    fn clone_node<'js>(&self, ctx: Ctx<'js>, deep: bool) -> Result<Value<'js>> {
        let world_rc = world(&ctx)?;
        let is_document = {
            let world = world_rc.borrow();
            let Some(parsed) = world.document(self.handle.0) else {
                return Err(Exception::throw_type(&ctx, "no document"));
            };
            self.handle.0.node == parsed.document.base.root_node().id
        };
        if is_document {
            return clone_document(&ctx, self.handle.0, deep);
        }
        let world = world_rc.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        let store = parsed.id;
        // Known gap: form-state cloning (input checkedness/value) has no Blitz
        // equivalent yet; deep clones carry structure only.
        let clone = clone_within_document(&mut parsed.document, store, self.handle.0, deep)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        drop(parsed);
        drop(world);
        wrap_node(&ctx, clone)
    }

    // https://dom.spec.whatwg.org/#dom-node-insertbefore
    #[qjs(skip)]
    fn insert_before<'js>(
        &self,
        ctx: Ctx<'js>,
        node: NodeReference,
        child: Option<NodeReference>,
    ) -> Result<Value<'js>> {
        let (node, child) = insertion_tree_nodes(&ctx, self.handle.0, node, child)?;
        let node = adopt_node(&ctx, self.handle.0, node)?;
        insert_tree_node(&ctx, self.handle.0, node, child)?;
        wrap_node(&ctx, node)
    }

    // https://dom.spec.whatwg.org/#dom-node-removechild
    #[qjs(skip)]
    fn remove_child<'js>(&self, ctx: Ctx<'js>, child: NodeReference) -> Result<Value<'js>> {
        let child = child
            .tree()
            .ok_or_else(|| throw_dom(&ctx, "NotFoundError", "attributes have no parent"))?;
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        let parented = parsed
            .document
            .base
            .get_node(child.node)
            .and_then(|node| node.parent)
            == Some(self.handle.0.node)
            && child.document == self.handle.0.document;
        if !parented {
            return Err(throw_dom(
                &ctx,
                "NotFoundError",
                "child is not a child of this node",
            ));
        }
        unlink_journaled(&mut parsed, child);
        drop(parsed);
        drop(world);
        fixup_focus_after_removal(&ctx, child)?;
        schedule_mutation_delivery(&ctx)?;
        wrap_node(&ctx, child)
    }

    // https://dom.spec.whatwg.org/#dom-node-replacechild
    #[qjs(skip)]
    fn replace_child<'js>(
        &self,
        ctx: Ctx<'js>,
        node: NodeReference,
        child: NodeReference,
    ) -> Result<Value<'js>> {
        let (node, _) = insertion_tree_nodes(&ctx, self.handle.0, node, Some(child))?;
        let Some(child) = child.tree() else {
            return Err(throw_dom(
                &ctx,
                "NotFoundError",
                "attributes have no parent",
            ));
        };
        let node = adopt_node(&ctx, self.handle.0, node)?;
        let moved = {
            let world = world(&ctx)?;
            let world = world.borrow();
            let Some(mut parsed) = world.document_mut(self.handle.0) else {
                return Err(Exception::throw_type(&ctx, "no document"));
            };
            replace_parsed(&mut parsed, self.handle.0, node, child)
        };
        fixup_focus_after_removal(&ctx, child)?;
        for id in &moved {
            fixup_option_on_insert(&ctx, *id)?;
            fixup_radio_on_insert(&ctx, *id)?;
        }
        schedule_mutation_delivery(&ctx)?;
        wrap_node(&ctx, child)
    }

    // https://dom.spec.whatwg.org/#dom-node-lookupnamespaceuri
    #[qjs(skip)]
    fn lookup_namespace_uri<'js>(
        &self,
        ctx: Ctx<'js>,
        prefix: Option<rquickjs::String<'js>>,
    ) -> Result<Value<'js>> {
        let prefix = prefix
            .map(|value| value.to_string())
            .transpose()?
            .filter(|value| !value.is_empty());
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(Value::new_null(ctx));
        };
        match locate_namespace(
            &parsed.document.base,
            parsed.id,
            self.handle.0,
            prefix.as_deref(),
        ) {
            Some(namespace) => string_value(&ctx, &namespace),
            None => Ok(Value::new_null(ctx)),
        }
    }

    // https://dom.spec.whatwg.org/#dom-node-lookupprefix
    #[qjs(skip)]
    fn lookup_prefix<'js>(
        &self,
        ctx: Ctx<'js>,
        namespace: Option<rquickjs::String<'js>>,
    ) -> Result<Value<'js>> {
        let Some(namespace) = namespace
            .map(|value| value.to_string())
            .transpose()?
            .filter(|value| !value.is_empty())
        else {
            return Ok(Value::new_null(ctx));
        };
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(Value::new_null(ctx));
        };
        match locate_prefix(&parsed.document.base, parsed.id, self.handle.0, &namespace) {
            Some(prefix) => string_value(&ctx, &prefix),
            None => Ok(Value::new_null(ctx)),
        }
    }

    // https://dom.spec.whatwg.org/#dom-node-isdefaultnamespace
    #[qjs(skip)]
    fn is_default_namespace<'js>(
        &self,
        ctx: Ctx<'js>,
        namespace: Option<rquickjs::String<'js>>,
    ) -> Result<bool> {
        let namespace = namespace
            .map(|value| value.to_string())
            .transpose()?
            .filter(|value| !value.is_empty());
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(false);
        };
        let found = locate_namespace(&parsed.document.base, parsed.id, self.handle.0, None);
        Ok(found.as_deref() == namespace.as_deref())
    }
}

// https://dom.spec.whatwg.org/#dom-nonelementparentnode-getelementbyid
fn element_by_id<'js>(ctx: &Ctx<'js>, root: NodeId, id: &str) -> Result<Value<'js>> {
    let owner = world_for_node(ctx, root)?;
    let found = {
        let owner = owner.borrow();
        let Some(parsed) = owner.document(root) else {
            return Ok(Value::new_null(ctx.clone()));
        };
        find_element_by_id(&parsed.document.base, parsed.id, root.node, id)
    };
    match found {
        Some(node) => wrap_node(ctx, node),
        None => Ok(Value::new_null(ctx.clone())),
    }
}

impl<'js> From<parent_node_generated::DOMStringOrNode<'js>> for NodeOrString<'js> {
    fn from(item: parent_node_generated::DOMStringOrNode<'js>) -> Self {
        match item {
            parent_node_generated::DOMStringOrNode::Node(node) => Self::Node(node),
            parent_node_generated::DOMStringOrNode::DOMString(string) => Self::String(string),
        }
    }
}

impl<'js> From<child_node_generated::DOMStringOrNode<'js>> for NodeOrString<'js> {
    fn from(item: child_node_generated::DOMStringOrNode<'js>) -> Self {
        match item {
            child_node_generated::DOMStringOrNode::Node(node) => Self::Node(node),
            child_node_generated::DOMStringOrNode::DOMString(string) => Self::String(string),
        }
    }
}

/// Map generated `(Node or DOMString)` members into the shared insertion
/// representation. The enums differ per module but hold the same types.
fn union_nodes<'js, T>(nodes: Vec<T>) -> Vec<NodeOrString<'js>>
where
    NodeOrString<'js>: From<T>,
{
    nodes.into_iter().map(NodeOrString::from).collect()
}

impl<'js> node_generated::Node<'js> for JsNode {
    // https://dom.spec.whatwg.org/#dom-node-nodetype
    fn get_node_type(&self, ctx: &Ctx<'js>) -> Result<u16> {
        self.node_type(ctx)
    }

    // https://dom.spec.whatwg.org/#dom-node-nodename
    fn get_node_name(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        self.node_name(ctx)
    }

    // https://dom.spec.whatwg.org/#dom-node-baseuri
    fn get_base_uri(&self, ctx: &Ctx<'js>) -> Result<crate::dom_string::DomString> {
        self.base_uri(ctx)
    }

    // https://dom.spec.whatwg.org/#dom-node-isconnected
    fn get_is_connected(&self, ctx: &Ctx<'js>) -> Result<bool> {
        self.is_connected(ctx)
    }

    // https://dom.spec.whatwg.org/#dom-node-ownerdocument
    fn get_owner_document(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.owner_document(ctx)
    }

    // https://dom.spec.whatwg.org/#dom-node-getrootnode
    fn get_root_node(
        &self,
        ctx: Ctx<'js>,
        arg_0: node_generated::GetRootNodeOptions,
    ) -> Result<Value<'js>> {
        self.get_root_node(ctx, arg_0)
    }

    // https://dom.spec.whatwg.org/#dom-node-parentnode
    fn get_parent_node(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.parent_node(ctx)
    }

    // https://dom.spec.whatwg.org/#dom-node-parentelement
    fn get_parent_element(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.parent_element(ctx)
    }

    // https://dom.spec.whatwg.org/#dom-node-haschildnodes
    fn has_child_nodes(&self, ctx: Ctx<'js>) -> Result<bool> {
        self.has_child_nodes(ctx)
    }

    // https://dom.spec.whatwg.org/#dom-node-childnodes
    fn get_child_nodes(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.child_nodes(ctx)
    }

    // https://dom.spec.whatwg.org/#dom-node-firstchild
    fn get_first_child(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.first_child(ctx)
    }

    // https://dom.spec.whatwg.org/#dom-node-lastchild
    fn get_last_child(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.last_child(ctx)
    }

    // https://dom.spec.whatwg.org/#dom-node-previoussibling
    fn get_previous_sibling(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.previous_sibling(ctx)
    }

    // https://dom.spec.whatwg.org/#dom-node-nextsibling
    fn get_next_sibling(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.next_sibling(ctx)
    }

    // https://dom.spec.whatwg.org/#dom-node-nodevalue
    fn get_node_value(&self, ctx: &Ctx<'js>) -> Result<Option<rquickjs::String<'js>>> {
        self.node_value(ctx)
    }

    // https://dom.spec.whatwg.org/#dom-node-nodevalue
    fn set_node_value(&self, ctx: &Ctx<'js>, value: Option<rquickjs::String<'js>>) -> Result<()> {
        self.set_node_value(ctx, value)
    }

    // https://dom.spec.whatwg.org/#dom-node-textcontent
    fn get_text_content(&self, ctx: &Ctx<'js>) -> Result<Option<rquickjs::String<'js>>> {
        self.text_content(ctx)
    }

    // https://dom.spec.whatwg.org/#dom-node-textcontent
    fn set_text_content(&self, ctx: &Ctx<'js>, value: Option<rquickjs::String<'js>>) -> Result<()> {
        self.set_text_content(ctx, value)
    }

    // https://dom.spec.whatwg.org/#dom-node-normalize
    fn normalize(&self, ctx: Ctx<'js>) -> Result<()> {
        self.normalize(ctx)
    }

    // https://dom.spec.whatwg.org/#dom-node-clonenode
    fn clone_node(&self, ctx: Ctx<'js>, arg_0: bool) -> Result<Value<'js>> {
        self.clone_node(ctx, arg_0)
    }

    // https://dom.spec.whatwg.org/#dom-node-isequalnode
    fn is_equal_node(&self, ctx: Ctx<'js>, arg_0: Option<NodeReference>) -> Result<bool> {
        self.is_equal_node(ctx, arg_0)
    }

    // https://dom.spec.whatwg.org/#dom-node-issamenode
    fn is_same_node(&self, ctx: Ctx<'js>, arg_0: Option<NodeReference>) -> Result<bool> {
        self.is_same_node(ctx, arg_0)
    }

    // https://dom.spec.whatwg.org/#dom-node-comparedocumentposition
    fn compare_document_position(&self, ctx: Ctx<'js>, arg_0: NodeReference) -> Result<u16> {
        self.compare_document_position(ctx, arg_0)
    }

    // https://dom.spec.whatwg.org/#dom-node-contains
    fn contains(&self, ctx: Ctx<'js>, arg_0: Option<NodeReference>) -> Result<bool> {
        self.contains(ctx, arg_0)
    }

    // https://dom.spec.whatwg.org/#dom-node-lookupprefix
    fn lookup_prefix(
        &self,
        ctx: Ctx<'js>,
        arg_0: Option<rquickjs::String<'js>>,
    ) -> Result<Option<rquickjs::String<'js>>> {
        let value = self.lookup_prefix(ctx.clone(), arg_0)?;
        if value.is_null() || value.is_undefined() {
            Ok(None)
        } else {
            rquickjs::FromJs::from_js(&ctx, value).map(Some)
        }
    }

    // https://dom.spec.whatwg.org/#dom-node-lookupnamespaceuri
    fn lookup_namespace_uri(
        &self,
        ctx: Ctx<'js>,
        arg_0: Option<rquickjs::String<'js>>,
    ) -> Result<Option<rquickjs::String<'js>>> {
        let value = self.lookup_namespace_uri(ctx.clone(), arg_0)?;
        if value.is_null() || value.is_undefined() {
            Ok(None)
        } else {
            rquickjs::FromJs::from_js(&ctx, value).map(Some)
        }
    }

    // https://dom.spec.whatwg.org/#dom-node-isdefaultnamespace
    fn is_default_namespace(
        &self,
        ctx: Ctx<'js>,
        arg_0: Option<rquickjs::String<'js>>,
    ) -> Result<bool> {
        self.is_default_namespace(ctx, arg_0)
    }

    // https://dom.spec.whatwg.org/#dom-node-insertbefore
    fn insert_before(
        &self,
        ctx: Ctx<'js>,
        arg_0: NodeReference,
        arg_1: Option<NodeReference>,
    ) -> Result<Value<'js>> {
        self.insert_before(ctx, arg_0, arg_1)
    }

    // https://dom.spec.whatwg.org/#dom-node-appendchild
    fn append_child(&self, ctx: Ctx<'js>, arg_0: NodeReference) -> Result<Value<'js>> {
        self.append_child(ctx, arg_0)
    }

    // https://dom.spec.whatwg.org/#dom-node-replacechild
    fn replace_child(
        &self,
        ctx: Ctx<'js>,
        arg_0: NodeReference,
        arg_1: NodeReference,
    ) -> Result<Value<'js>> {
        self.replace_child(ctx, arg_0, arg_1)
    }

    // https://dom.spec.whatwg.org/#dom-node-removechild
    fn remove_child(&self, ctx: Ctx<'js>, arg_0: NodeReference) -> Result<Value<'js>> {
        self.remove_child(ctx, arg_0)
    }
}

impl<'js> element_generated::Element<'js> for JsNode {
    // https://dom.spec.whatwg.org/#dom-element-getelementsbytagname
    fn get_elements_by_tag_name(
        &self,
        ctx: Ctx<'js>,
        arg_0: rquickjs::String<'js>,
    ) -> Result<Value<'js>> {
        self.get_elements_by_tag_name(ctx, WebIdlString(arg_0.to_string()?))
    }

    // https://dom.spec.whatwg.org/#dom-element-getelementsbytagnamens
    fn get_elements_by_tag_name_ns(
        &self,
        ctx: Ctx<'js>,
        arg_0: Option<rquickjs::String<'js>>,
        arg_1: rquickjs::String<'js>,
    ) -> Result<Value<'js>> {
        self.get_elements_by_tag_name_ns(
            ctx,
            OptString(arg_0.map(|name| name.to_string()).transpose()?),
            WebIdlString(arg_1.to_string()?),
        )
    }

    // https://dom.spec.whatwg.org/#dom-element-getelementsbyclassname
    fn get_elements_by_class_name(
        &self,
        ctx: Ctx<'js>,
        arg_0: rquickjs::String<'js>,
    ) -> Result<Value<'js>> {
        self.get_elements_by_class_name(ctx, WebIdlString(arg_0.to_string()?))
    }

    // https://dom.spec.whatwg.org/#dom-element-matches
    fn matches(&self, ctx: Ctx<'js>, arg_0: rquickjs::String<'js>) -> Result<bool> {
        self.matches(ctx, WebIdlString(arg_0.to_string()?))
    }

    // https://dom.spec.whatwg.org/#dom-element-closest
    fn closest(&self, ctx: Ctx<'js>, arg_0: rquickjs::String<'js>) -> Result<Value<'js>> {
        self.closest(ctx, WebIdlString(arg_0.to_string()?))
    }

    // https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-innerhtml
    fn get_inner_html(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        self.inner_html(ctx)
    }

    // https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-innerhtml
    fn set_inner_html(&self, ctx: &Ctx<'js>, value: rquickjs::String<'js>) -> Result<()> {
        self.set_inner_html(ctx, LegacyNullString(value.to_string()?))
    }

    // https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-outerhtml
    fn get_outer_html(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        self.outer_html(ctx)
    }

    // https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-outerhtml
    fn set_outer_html(&self, ctx: &Ctx<'js>, value: rquickjs::String<'js>) -> Result<()> {
        self.set_outer_html(ctx, LegacyNullString(value.to_string()?))
    }

    // https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-insertadjacenthtml
    fn insert_adjacent_html(
        &self,
        ctx: Ctx<'js>,
        arg_0: rquickjs::String<'js>,
        arg_1: rquickjs::String<'js>,
    ) -> Result<()> {
        self.insert_adjacent_html(
            ctx,
            WebIdlString(arg_0.to_string()?),
            WebIdlString(arg_1.to_string()?),
        )
    }

    // https://drafts.csswg.org/cssom-view/#dom-element-getboundingclientrect
    fn get_bounding_client_rect(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        self.get_bounding_client_rect(ctx)
    }

    // https://drafts.csswg.org/cssom-view/#dom-element-getclientrects
    fn get_client_rects(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        self.get_client_rects(ctx)
    }

    // https://drafts.csswg.org/cssom-view/#dom-element-scrollintoview
    fn scroll_into_view(
        &self,
        ctx: Ctx<'js>,
        _arg_0: element_generated::BooleanOrScrollIntoViewOptions,
    ) -> Result<()> {
        self.scroll_into_view(ctx)
    }

    // https://drafts.csswg.org/cssom-view/#dom-element-scrollleft
    fn get_scroll_left(&self, ctx: &Ctx<'js>) -> Result<f64> {
        self.scroll_left(ctx)
    }

    // https://drafts.csswg.org/#dom-element-scrollleft
    fn set_scroll_left(&self, ctx: &Ctx<'js>, value: f64) -> Result<()> {
        self.set_scroll_left(ctx, value)
    }

    // https://drafts.csswg.org/cssom-view/#dom-element-scrolltop
    fn get_scroll_top(&self, ctx: &Ctx<'js>) -> Result<f64> {
        self.scroll_top(ctx)
    }

    // https://drafts.csswg.org/#dom-element-scrolltop
    fn set_scroll_top(&self, ctx: &Ctx<'js>, value: f64) -> Result<()> {
        self.set_scroll_top(ctx, value)
    }

    // https://dom.spec.whatwg.org/#dom-element-attachshadow
    fn attach_shadow(
        &self,
        ctx: Ctx<'js>,
        arg_0: element_generated::ShadowRootInit,
    ) -> Result<Value<'js>> {
        self.attach_shadow(ctx, arg_0)
    }

    // https://dom.spec.whatwg.org/#dom-element-shadowroot
    fn get_shadow_root(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        Ok(self.shadow_root(ctx))
    }

    // https://dom.spec.whatwg.org/#dom-element-tagname
    fn get_tag_name(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let name = self.tag_name(ctx)?;
        rquickjs::String::from_str(ctx.clone(), &name)
    }

    // https://dom.spec.whatwg.org/#dom-element-localname
    fn get_local_name(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let name = self.local_name(ctx)?;
        rquickjs::String::from_str(ctx.clone(), &name)
    }

    // https://dom.spec.whatwg.org/#dom-element-prefix
    fn get_prefix(&self, ctx: &Ctx<'js>) -> Result<Option<rquickjs::String<'js>>> {
        let value = self.prefix(ctx)?;
        if value.is_null() || value.is_undefined() {
            Ok(None)
        } else {
            rquickjs::FromJs::from_js(ctx, value).map(Some)
        }
    }

    // https://dom.spec.whatwg.org/#dom-element-namespaceuri
    fn get_namespace_uri(&self, ctx: &Ctx<'js>) -> Result<Option<rquickjs::String<'js>>> {
        let value = self.namespace_uri(ctx)?;
        if value.is_null() || value.is_undefined() {
            Ok(None)
        } else {
            rquickjs::FromJs::from_js(ctx, value).map(Some)
        }
    }

    // https://dom.spec.whatwg.org/#dom-element-id
    fn get_id(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let id = self.id(ctx)?;
        rquickjs::String::from_str(ctx.clone(), &id)
    }

    // https://dom.spec.whatwg.org/#dom-element-id
    fn set_id(&self, ctx: &Ctx<'js>, value: rquickjs::String<'js>) -> Result<()> {
        self.set_id(ctx, WebIdlString(value.to_string()?))
    }

    // https://dom.spec.whatwg.org/#dom-element-classname
    fn get_class_name(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let name = self.class_name(ctx)?;
        rquickjs::String::from_str(ctx.clone(), &name)
    }

    // https://dom.spec.whatwg.org/#dom-element-classname
    fn set_class_name(&self, ctx: &Ctx<'js>, value: rquickjs::String<'js>) -> Result<()> {
        self.set_class_name(ctx, WebIdlString(value.to_string()?))
    }

    // https://dom.spec.whatwg.org/#dom-element-classlist
    fn get_class_list(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.class_list(ctx)
    }

    // https://dom.spec.whatwg.org/#dom-element-getattribute
    fn get_attribute(
        &self,
        ctx: Ctx<'js>,
        arg_0: rquickjs::String<'js>,
    ) -> Result<Option<rquickjs::String<'js>>> {
        let value = self.get_attribute(ctx.clone(), WebIdlString(arg_0.to_string()?))?;
        if value.is_null() || value.is_undefined() {
            Ok(None)
        } else {
            rquickjs::FromJs::from_js(&ctx, value).map(Some)
        }
    }

    // https://dom.spec.whatwg.org/#dom-element-setattribute
    fn set_attribute(
        &self,
        ctx: Ctx<'js>,
        arg_0: rquickjs::String<'js>,
        arg_1: rquickjs::String<'js>,
    ) -> Result<()> {
        self.set_attribute(
            ctx,
            WebIdlString(arg_0.to_string()?),
            WebIdlString(arg_1.to_string()?),
        )
    }

    // https://dom.spec.whatwg.org/#dom-element-hasattribute
    fn has_attribute(&self, ctx: Ctx<'js>, arg_0: rquickjs::String<'js>) -> Result<bool> {
        self.has_attribute(ctx, WebIdlString(arg_0.to_string()?))
    }

    // https://dom.spec.whatwg.org/#dom-element-removeattribute
    fn remove_attribute(&self, ctx: Ctx<'js>, arg_0: rquickjs::String<'js>) -> Result<()> {
        self.remove_attribute(ctx, WebIdlString(arg_0.to_string()?))
    }

    // https://dom.spec.whatwg.org/#dom-element-toggleattribute
    fn toggle_attribute(
        &self,
        ctx: Ctx<'js>,
        arg_0: rquickjs::String<'js>,
        arg_1: Option<bool>,
    ) -> Result<bool> {
        self.toggle_attribute(ctx, WebIdlString(arg_0.to_string()?), arg_1)
    }

    // https://dom.spec.whatwg.org/#dom-element-getattributens
    fn get_attribute_ns(
        &self,
        ctx: Ctx<'js>,
        arg_0: Option<rquickjs::String<'js>>,
        arg_1: rquickjs::String<'js>,
    ) -> Result<Option<rquickjs::String<'js>>> {
        let value = self.get_attribute_ns(
            ctx.clone(),
            OptString(arg_0.map(|name| name.to_string()).transpose()?),
            WebIdlString(arg_1.to_string()?),
        )?;
        if value.is_null() || value.is_undefined() {
            Ok(None)
        } else {
            rquickjs::FromJs::from_js(&ctx, value).map(Some)
        }
    }

    // https://dom.spec.whatwg.org/#dom-element-setattributens
    fn set_attribute_ns(
        &self,
        ctx: Ctx<'js>,
        arg_0: Option<rquickjs::String<'js>>,
        arg_1: rquickjs::String<'js>,
        arg_2: rquickjs::String<'js>,
    ) -> Result<()> {
        self.set_attribute_ns(
            ctx,
            OptString(arg_0.map(|name| name.to_string()).transpose()?),
            WebIdlString(arg_1.to_string()?),
            WebIdlString(arg_2.to_string()?),
        )
    }

    // https://dom.spec.whatwg.org/#dom-element-hasattributens
    fn has_attribute_ns(
        &self,
        ctx: Ctx<'js>,
        arg_0: Option<rquickjs::String<'js>>,
        arg_1: rquickjs::String<'js>,
    ) -> Result<bool> {
        self.has_attribute_ns(
            ctx,
            OptString(arg_0.map(|name| name.to_string()).transpose()?),
            WebIdlString(arg_1.to_string()?),
        )
    }

    // https://dom.spec.whatwg.org/#dom-element-removeattributens
    fn remove_attribute_ns(
        &self,
        ctx: Ctx<'js>,
        arg_0: Option<rquickjs::String<'js>>,
        arg_1: rquickjs::String<'js>,
    ) -> Result<()> {
        self.remove_attribute_ns(
            ctx,
            OptString(arg_0.map(|name| name.to_string()).transpose()?),
            WebIdlString(arg_1.to_string()?),
        )
    }

    // https://dom.spec.whatwg.org/#dom-element-getattributenames
    fn get_attribute_names(&self, ctx: Ctx<'js>) -> Result<Vec<String>> {
        self.get_attribute_names(ctx)
    }

    // https://dom.spec.whatwg.org/#dom-element-attributes
    fn get_attributes(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.attributes(ctx)
    }

    // https://dom.spec.whatwg.org/#dom-element-hasattributes
    fn has_attributes(&self, ctx: Ctx<'js>) -> Result<bool> {
        self.has_attributes(ctx)
    }

    // https://dom.spec.whatwg.org/#dom-element-getattributenode
    fn get_attribute_node(
        &self,
        ctx: Ctx<'js>,
        arg_0: rquickjs::String<'js>,
    ) -> Result<Value<'js>> {
        self.get_attribute_node(ctx, WebIdlString(arg_0.to_string()?))
    }

    // https://dom.spec.whatwg.org/#dom-element-getattributenodens
    fn get_attribute_node_ns(
        &self,
        ctx: Ctx<'js>,
        arg_0: Option<rquickjs::String<'js>>,
        arg_1: rquickjs::String<'js>,
    ) -> Result<Value<'js>> {
        self.get_attribute_node_ns(
            ctx,
            OptString(arg_0.map(|name| name.to_string()).transpose()?),
            WebIdlString(arg_1.to_string()?),
        )
    }

    // https://dom.spec.whatwg.org/#dom-element-setattributenode
    fn set_attribute_node(&self, ctx: Ctx<'js>, arg_0: Value<'js>) -> Result<Value<'js>> {
        let attr = AttrArgument::from_value(&arg_0)?;
        self.set_attribute_node(ctx, attr)
    }

    // https://dom.spec.whatwg.org/#dom-element-setattributenodens
    fn set_attribute_node_ns(&self, ctx: Ctx<'js>, arg_0: Value<'js>) -> Result<Value<'js>> {
        let attr = AttrArgument::from_value(&arg_0)?;
        self.set_attribute_node_ns(ctx, attr)
    }

    // https://dom.spec.whatwg.org/#dom-element-removeattributenode
    fn remove_attribute_node(&self, ctx: Ctx<'js>, arg_0: Value<'js>) -> Result<Value<'js>> {
        let attr = AttrArgument::from_value(&arg_0)?;
        self.remove_attribute_node(ctx, attr)
    }
}

/// `ElementCSSInlineStyle` is a spec mixin included by `HTMLElement`,
/// `SVGElement`, and `MathMLElement`, so the one contract installs on all
/// three prototypes from the IDL includes
/// (<https://drafts.csswg.org/cssom/#the-elementcssinlinestyle-mixin>).
impl<'js> element_css_inline_style_generated::ElementCSSInlineStyle<'js> for JsNode {
    // https://drafts.csswg.org/cssom/#dom-elementcssinlinestyle-style
    fn get_style(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.style(ctx)
    }
}

impl<'js> document_generated::Document<'js> for JsNode {
    // https://dom.spec.whatwg.org/#dom-document-implementation
    fn get_implementation(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.implementation(ctx)
    }

    // https://dom.spec.whatwg.org/#dom-document-documentelement
    fn get_document_element(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.document_element(ctx)
    }

    // https://html.spec.whatwg.org/multipage/dom.html#dom-document-body
    fn get_body(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.body(ctx)
    }

    // https://html.spec.whatwg.org/multipage/dom.html#dom-document-body
    fn set_body(&self, ctx: &Ctx<'js>, value: Value<'js>) -> Result<()> {
        self.set_body(ctx, value)
    }

    // https://html.spec.whatwg.org/multipage/dom.html#dom-document-head
    fn get_head(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.head(ctx)
    }

    // https://html.spec.whatwg.org/multipage/dom.html#dom-document-currentscript
    fn get_current_script(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.current_script(ctx)
    }

    // https://dom.spec.whatwg.org/#dom-document-doctype
    fn get_doctype(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        Ok(self.doctype(ctx))
    }

    // https://dom.spec.whatwg.org/#dom-document-documenturi
    fn get_document_uri(&self, ctx: &Ctx<'js>) -> Result<crate::dom_string::DomString> {
        self.document_uri(ctx)
    }

    // https://dom.spec.whatwg.org/#dom-document-url
    fn get_url(&self, ctx: &Ctx<'js>) -> Result<crate::dom_string::DomString> {
        self.url(ctx)
    }

    // https://dom.spec.whatwg.org/#dom-document-compatmode
    fn get_compat_mode(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let mode = self.compat_mode(ctx)?;
        rquickjs::String::from_str(ctx.clone(), &mode)
    }

    // https://encoding.spec.whatwg.org/#dom-document-characterset
    fn get_character_set(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let set = self.character_set(ctx)?;
        rquickjs::String::from_str(ctx.clone(), set)
    }

    // https://encoding.spec.whatwg.org/#dom-document-charset
    fn get_charset(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let set = self.charset(ctx)?;
        rquickjs::String::from_str(ctx.clone(), set)
    }

    // https://encoding.spec.whatwg.org/#dom-document-inputencoding
    fn get_input_encoding(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let encoding = self.input_encoding(ctx)?;
        rquickjs::String::from_str(ctx.clone(), encoding)
    }

    // https://dom.spec.whatwg.org/#dom-document-contenttype
    fn get_content_type(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let content_type = self.content_type(ctx)?;
        rquickjs::String::from_str(ctx.clone(), content_type)
    }

    // https://html.spec.whatwg.org/multipage/dom.html#dom-document-readystate
    fn get_ready_state(&self, ctx: &Ctx<'js>) -> Result<document_generated::DocumentReadyState> {
        Ok(match self.ready_state(ctx)?.as_str() {
            "interactive" => document_generated::DocumentReadyState::Interactive,
            "complete" => document_generated::DocumentReadyState::Complete,
            _ => document_generated::DocumentReadyState::Loading,
        })
    }

    // https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-document-defaultview
    fn get_default_view(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.default_view(ctx)
    }

    // https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-document-location
    fn get_location(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.location(ctx)
    }

    // https://html.spec.whatwg.org/multipage/interaction.html#dom-document-hasfocus
    fn has_focus(&self, ctx: Ctx<'js>) -> Result<bool> {
        self.has_focus(ctx)
    }

    // https://dom.spec.whatwg.org/#dom-document-activeelement
    fn get_active_element(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.active_element(ctx)
    }

    // https://html.spec.whatwg.org/multipage/dom.html#dom-document-title
    fn get_title(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        self.title(ctx)
    }

    // https://html.spec.whatwg.org/multipage/dom.html#dom-document-title
    fn set_title(&self, ctx: &Ctx<'js>, value: rquickjs::String<'js>) -> Result<()> {
        // `document.title` keeps every code unit, including lone surrogates.
        self.set_title(
            ctx,
            WebIdlCodeUnits(crate::dom_string::DomString::from_utf16(value.to_utf16()?)),
        )
    }

    // https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-document-write
    fn write(&self, ctx: Ctx<'js>, arg_0: Vec<rquickjs::String<'js>>) -> Result<()> {
        let mut text = Vec::with_capacity(arg_0.len());
        for part in arg_0 {
            text.push(WebIdlString(part.to_string()?));
        }
        self.write(ctx, text)
    }

    // https://dom.spec.whatwg.org/#dom-document-getelementsbyclassname
    fn get_elements_by_class_name(
        &self,
        ctx: Ctx<'js>,
        arg_0: rquickjs::String<'js>,
    ) -> Result<Value<'js>> {
        self.get_elements_by_class_name(ctx, WebIdlString(arg_0.to_string()?))
    }

    // https://dom.spec.whatwg.org/#dom-document-createevent
    fn create_event(&self, ctx: Ctx<'js>, arg_0: rquickjs::String<'js>) -> Result<Value<'js>> {
        let interface = arg_0.to_string()?;
        events::create_event(&ctx, &interface)
    }

    // https://dom.spec.whatwg.org/#dom-document-createelement
    fn create_element(
        &self,
        ctx: Ctx<'js>,
        arg_0: rquickjs::String<'js>,
        _options: document_generated::DOMStringOrElementCreationOptions,
    ) -> Result<Value<'js>> {
        self.create_element(ctx, WebIdlString(arg_0.to_string()?))
    }

    // https://dom.spec.whatwg.org/#dom-document-createelementns
    fn create_element_ns(
        &self,
        ctx: Ctx<'js>,
        arg_0: Option<rquickjs::String<'js>>,
        arg_1: rquickjs::String<'js>,
        _options: document_generated::DOMStringOrElementCreationOptions,
    ) -> Result<Value<'js>> {
        self.create_element_ns(
            ctx,
            OptString(arg_0.map(|name| name.to_string()).transpose()?),
            WebIdlString(arg_1.to_string()?),
        )
    }

    // https://dom.spec.whatwg.org/#dom-document-createtextnode
    fn create_text_node(&self, ctx: Ctx<'js>, arg_0: rquickjs::String<'js>) -> Result<Value<'js>> {
        self.create_text_node(
            ctx,
            WebIdlCodeUnits(crate::dom_string::DomString::from_utf16(arg_0.to_utf16()?)),
        )
    }

    // https://dom.spec.whatwg.org/#dom-document-createcomment
    fn create_comment(&self, ctx: Ctx<'js>, arg_0: rquickjs::String<'js>) -> Result<Value<'js>> {
        self.create_comment(
            ctx,
            WebIdlCodeUnits(crate::dom_string::DomString::from_utf16(arg_0.to_utf16()?)),
        )
    }

    // https://dom.spec.whatwg.org/#dom-document-createprocessinginstruction
    fn create_processing_instruction(
        &self,
        ctx: Ctx<'js>,
        arg_0: rquickjs::String<'js>,
        arg_1: rquickjs::String<'js>,
    ) -> Result<Value<'js>> {
        self.create_processing_instruction(
            ctx,
            WebIdlString(arg_0.to_string()?),
            WebIdlCodeUnits(crate::dom_string::DomString::from_utf16(arg_1.to_utf16()?)),
        )
    }

    // https://dom.spec.whatwg.org/#dom-document-createcdatasection
    fn create_cdata_section(
        &self,
        ctx: Ctx<'js>,
        arg_0: rquickjs::String<'js>,
    ) -> Result<Value<'js>> {
        self.create_cdata_section(
            ctx,
            WebIdlCodeUnits(crate::dom_string::DomString::from_utf16(arg_0.to_utf16()?)),
        )
    }

    // https://dom.spec.whatwg.org/#dom-document-createattribute
    fn create_attribute(&self, ctx: Ctx<'js>, arg_0: rquickjs::String<'js>) -> Result<Value<'js>> {
        self.create_attribute(ctx, WebIdlString(arg_0.to_string()?))
    }

    // https://dom.spec.whatwg.org/#dom-document-createattributens
    fn create_attribute_ns(
        &self,
        ctx: Ctx<'js>,
        arg_0: Option<rquickjs::String<'js>>,
        arg_1: rquickjs::String<'js>,
    ) -> Result<Value<'js>> {
        self.create_attribute_ns(
            ctx,
            OptString(arg_0.map(|name| name.to_string()).transpose()?),
            WebIdlString(arg_1.to_string()?),
        )
    }

    // https://dom.spec.whatwg.org/#dom-document-createdocumentfragment
    fn create_document_fragment(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        self.create_document_fragment(ctx)
    }

    // https://dom.spec.whatwg.org/#dom-document-importnode
    fn import_node(
        &self,
        ctx: Ctx<'js>,
        arg_0: NodeReference,
        arg_1: document_generated::BooleanOrImportNodeOptions,
    ) -> Result<Value<'js>> {
        // `ImportNodeOptions` inverts the boolean form: selecting only the
        // node itself is a shallow import.
        let deep = match arg_1 {
            document_generated::BooleanOrImportNodeOptions::Boolean(deep) => deep,
            document_generated::BooleanOrImportNodeOptions::ImportNodeOptions(options) => {
                !options.self_only
            }
        };
        let node = match arg_0 {
            NodeReference::Tree(id) => wrap_node(&ctx, id)?,
            NodeReference::Attribute { id, .. } => attr_wrapper(&ctx, id)?,
        };
        self.import_node(ctx, node, deep)
    }

    // https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-document-open
    // Both arguments are ignored upstream; the navigation overload stays
    // absent with the rest of navigation.
    fn open(
        &self,
        ctx: Ctx<'js>,
        _arg_0: Option<rquickjs::String<'js>>,
        _arg_1: Option<rquickjs::String<'js>>,
    ) -> Result<Value<'js>> {
        self.open_document(ctx)
    }

    // https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-document-close
    fn close(&self, ctx: Ctx<'js>) -> Result<()> {
        self.close_document(ctx)
    }

    // https://dom.spec.whatwg.org/#dom-nonelementparentnode-getelementbyid
    fn get_element_by_id(&self, ctx: Ctx<'js>, arg_0: rquickjs::String<'js>) -> Result<Value<'js>> {
        self.get_element_by_id(ctx, WebIdlString(arg_0.to_string()?))
    }

    // https://dom.spec.whatwg.org/#dom-document-getelementsbytagname
    fn get_elements_by_tag_name(
        &self,
        ctx: Ctx<'js>,
        arg_0: rquickjs::String<'js>,
    ) -> Result<Value<'js>> {
        self.get_elements_by_tag_name(ctx, WebIdlString(arg_0.to_string()?))
    }

    // https://dom.spec.whatwg.org/#dom-document-getelementsbytagnamens
    fn get_elements_by_tag_name_ns(
        &self,
        ctx: Ctx<'js>,
        arg_0: Option<rquickjs::String<'js>>,
        arg_1: rquickjs::String<'js>,
    ) -> Result<Value<'js>> {
        self.get_elements_by_tag_name_ns(
            ctx,
            OptString(arg_0.map(|name| name.to_string()).transpose()?),
            WebIdlString(arg_1.to_string()?),
        )
    }

    // https://html.spec.whatwg.org/multipage/dom.html#dom-document-getelementsbyname
    fn get_elements_by_name(
        &self,
        ctx: Ctx<'js>,
        arg_0: rquickjs::String<'js>,
    ) -> Result<Value<'js>> {
        self.get_elements_by_name(ctx, WebIdlString(arg_0.to_string()?))
    }

    // https://drafts.csswg.org/cssom-view/#dom-document-elementfrompoint
    fn element_from_point(&self, ctx: Ctx<'js>, arg_0: f64, arg_1: f64) -> Result<Value<'js>> {
        self.element_from_point(ctx, arg_0, arg_1)
    }

    // https://drafts.csswg.org/cssom-view/#dom-document-elementsfrompoint
    fn elements_from_point(
        &self,
        ctx: Ctx<'js>,
        arg_0: f64,
        arg_1: f64,
    ) -> Result<Vec<Value<'js>>> {
        self.elements_from_point(ctx, arg_0, arg_1)
    }
}

impl<'js> parent_node_generated::ParentNode<'js> for JsNode {
    // https://dom.spec.whatwg.org/#dom-parentnode-children
    fn get_children(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        live_collection(
            ctx,
            self.handle.0,
            CollectionKind::ElementChildren,
            Some("HTMLCollection"),
        )
    }

    // https://dom.spec.whatwg.org/#dom-parentnode-firstelementchild
    fn get_first_element_child(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let world = world(ctx)?;
        let found = {
            let parsed = world.borrow();
            let Some(parsed) = parsed.document(self.handle.0) else {
                return Ok(Value::new_null(ctx.clone()));
            };
            let base = &parsed.document.base;
            base.get_node(self.handle.0.node).and_then(|node| {
                node.children
                    .iter()
                    .find(|kid| is_element(base, **kid))
                    .map(|kid| NodeId {
                        document: parsed.id,
                        node: *kid,
                    })
            })
        };
        child_value(ctx, found)
    }

    // https://dom.spec.whatwg.org/#dom-parentnode-lastelementchild
    fn get_last_element_child(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let world = world(ctx)?;
        let found = {
            let parsed = world.borrow();
            let Some(parsed) = parsed.document(self.handle.0) else {
                return Ok(Value::new_null(ctx.clone()));
            };
            let base = &parsed.document.base;
            base.get_node(self.handle.0.node).and_then(|node| {
                node.children
                    .iter()
                    .rev()
                    .find(|kid| is_element(base, **kid))
                    .map(|kid| NodeId {
                        document: parsed.id,
                        node: *kid,
                    })
            })
        };
        child_value(ctx, found)
    }

    // https://dom.spec.whatwg.org/#dom-parentnode-childelementcount
    fn get_child_element_count(&self, ctx: &Ctx<'js>) -> Result<usize> {
        let world = world(ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(0);
        };
        let base = &parsed.document.base;
        Ok(base.get_node(self.handle.0.node).map_or(0, |node| {
            node.children
                .iter()
                .filter(|kid| is_element(base, **kid))
                .count()
        }))
    }

    // https://dom.spec.whatwg.org/#dom-parentnode-append
    fn append(
        &self,
        ctx: Ctx<'js>,
        nodes: Vec<parent_node_generated::DOMStringOrNode<'js>>,
    ) -> Result<()> {
        let node = convert_union_nodes_into_node(&ctx, self.handle.0, union_nodes(nodes))?;
        let (node, _) = insertion_tree_nodes(&ctx, self.handle.0, NodeReference::Tree(node), None)?;
        insert_tree_node(&ctx, self.handle.0, node, None)
    }

    // https://dom.spec.whatwg.org/#dom-parentnode-prepend
    fn prepend(
        &self,
        ctx: Ctx<'js>,
        nodes: Vec<parent_node_generated::DOMStringOrNode<'js>>,
    ) -> Result<()> {
        let node = convert_union_nodes_into_node(&ctx, self.handle.0, union_nodes(nodes))?;
        let reference = {
            let world = world(&ctx)?;
            let world = world.borrow();
            let Some(parsed) = world.document(self.handle.0) else {
                return Err(Exception::throw_type(&ctx, "no document"));
            };
            parsed
                .document
                .base
                .get_node(self.handle.0.node)
                .and_then(|parent| parent.children.first().copied())
                .map(|node| NodeId {
                    document: self.handle.0.document,
                    node,
                })
        };
        let (node, reference) = insertion_tree_nodes(
            &ctx,
            self.handle.0,
            NodeReference::Tree(node),
            reference.map(NodeReference::Tree),
        )?;
        insert_tree_node(&ctx, self.handle.0, node, reference)
    }

    // https://dom.spec.whatwg.org/#dom-parentnode-replacechildren
    fn replace_children(
        &self,
        ctx: Ctx<'js>,
        nodes: Vec<parent_node_generated::DOMStringOrNode<'js>>,
    ) -> Result<()> {
        let node = convert_union_nodes_into_node(&ctx, self.handle.0, union_nodes(nodes))?;
        let (node, _) = insertion_tree_nodes(&ctx, self.handle.0, NodeReference::Tree(node), None)?;
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        let added = if parsed.document.is_fragment(node.node) {
            let moved: Vec<NodeId> = parsed
                .document
                .base
                .get_node(node.node)
                .map(|backing| {
                    backing
                        .children
                        .iter()
                        .map(|child| NodeId {
                            document: node.document,
                            node: *child,
                        })
                        .collect()
                })
                .unwrap_or_default();
            parsed.document.record(JournalEntry::ChildList {
                target: node,
                added: Vec::new(),
                removed: moved.clone(),
                previous: None,
                next: None,
            });
            moved
        } else {
            unlink_journaled(&mut parsed, node);
            vec![node]
        };
        replace_all_journaled(&mut parsed, self.handle.0, added);
        drop(parsed);
        drop(world);
        schedule_mutation_delivery(&ctx)
    }

    // https://dom.spec.whatwg.org/#dom-parentnode-queryselector
    fn query_selector(
        &self,
        ctx: Ctx<'js>,
        selectors: rquickjs::String<'js>,
    ) -> Result<Value<'js>> {
        let selectors = selectors.to_string()?;
        let world = world(&ctx)?;
        let found = {
            let parsed = world.borrow();
            let Some(parsed) = parsed.document(self.handle.0) else {
                return Ok(Value::new_null(ctx));
            };
            parsed
                .document
                .base
                .query_selector_in(self.handle.0.node, &selectors)
                .map_err(|err| select_error(&ctx, &err))?
                .map(|node| NodeId {
                    document: parsed.id,
                    node,
                })
        };
        child_value(&ctx, found)
    }

    // https://dom.spec.whatwg.org/#dom-parentnode-queryselectorall
    fn query_selector_all(
        &self,
        ctx: Ctx<'js>,
        selectors: rquickjs::String<'js>,
    ) -> Result<Value<'js>> {
        let selectors = selectors.to_string()?;
        let world = world(&ctx)?;
        let ids = {
            let parsed = world.borrow();
            // A missing document answers with an empty list, not null
            // (<https://dom.spec.whatwg.org/#dom-parentnode-queryselectorall>).
            parsed
                .document(self.handle.0)
                .map(|parsed| {
                    parsed
                        .document
                        .base
                        .query_selector_all_in(self.handle.0.node, &selectors)
                        .map_err(|err| select_error(&ctx, &err))
                        .map(|ids| {
                            ids.into_iter()
                                .map(|node| {
                                    Handle(NodeId {
                                        document: parsed.id,
                                        node,
                                    })
                                })
                                .collect::<Vec<_>>()
                        })
                })
                .transpose()?
                .unwrap_or_default()
        };
        live_collection(&ctx, self.handle.0, CollectionKind::Static(ids), None)
    }
}

impl<'js> child_node_generated::ChildNode<'js> for JsNode {
    // https://dom.spec.whatwg.org/#dom-childnode-before
    fn before(
        &self,
        ctx: Ctx<'js>,
        nodes: Vec<child_node_generated::DOMStringOrNode<'js>>,
    ) -> Result<()> {
        let node = convert_union_nodes_into_node(&ctx, self.handle.0, union_nodes(nodes))?;
        let (parent, reference) = {
            let world = world(&ctx)?;
            let world = world.borrow();
            let Some(parsed) = world.document(self.handle.0) else {
                return Err(Exception::throw_type(&ctx, "no document"));
            };
            let Some(parent) = parsed
                .document
                .base
                .get_node(self.handle.0.node)
                .and_then(|node| node.parent)
                .map(|node| NodeId {
                    document: self.handle.0.document,
                    node,
                })
            else {
                return Ok(());
            };
            (parent, self.handle.0)
        };
        let (node, reference) = insertion_tree_nodes(
            &ctx,
            parent,
            NodeReference::Tree(node),
            Some(NodeReference::Tree(reference)),
        )?;
        insert_tree_node(&ctx, parent, node, reference)
    }

    // https://dom.spec.whatwg.org/#dom-childnode-after
    fn after(
        &self,
        ctx: Ctx<'js>,
        nodes: Vec<child_node_generated::DOMStringOrNode<'js>>,
    ) -> Result<()> {
        let node = convert_union_nodes_into_node(&ctx, self.handle.0, union_nodes(nodes))?;
        let (parent, reference) = {
            let world = world(&ctx)?;
            let world = world.borrow();
            let Some(parsed) = world.document(self.handle.0) else {
                return Err(Exception::throw_type(&ctx, "no document"));
            };
            let base = &parsed.document.base;
            let Some(parent) = base
                .get_node(self.handle.0.node)
                .and_then(|node| node.parent)
                .map(|node| NodeId {
                    document: self.handle.0.document,
                    node,
                })
            else {
                return Ok(());
            };
            let reference = sibling(base, self.handle.0.node, true).map(|node| NodeId {
                document: self.handle.0.document,
                node,
            });
            (parent, reference)
        };
        let (node, reference) = insertion_tree_nodes(
            &ctx,
            parent,
            NodeReference::Tree(node),
            reference.map(NodeReference::Tree),
        )?;
        insert_tree_node(&ctx, parent, node, reference)
    }

    // https://dom.spec.whatwg.org/#dom-childnode-replacewith
    fn replace_with(
        &self,
        ctx: Ctx<'js>,
        nodes: Vec<child_node_generated::DOMStringOrNode<'js>>,
    ) -> Result<()> {
        let node = convert_union_nodes_into_node(&ctx, self.handle.0, union_nodes(nodes))?;
        let parent = {
            let world = world(&ctx)?;
            let world = world.borrow();
            let Some(parsed) = world.document(self.handle.0) else {
                return Err(Exception::throw_type(&ctx, "no document"));
            };
            let Some(parent) = parsed
                .document
                .base
                .get_node(self.handle.0.node)
                .and_then(|node| node.parent)
                .map(|node| NodeId {
                    document: self.handle.0.document,
                    node,
                })
            else {
                return Ok(());
            };
            parent
        };
        let (node, _) = insertion_tree_nodes(
            &ctx,
            parent,
            NodeReference::Tree(node),
            Some(NodeReference::Tree(self.handle.0)),
        )?;
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        let moved = replace_parsed(&mut parsed, parent, node, self.handle.0);
        drop(parsed);
        drop(world);
        for id in &moved {
            fixup_option_on_insert(&ctx, *id)?;
            fixup_radio_on_insert(&ctx, *id)?;
        }
        schedule_mutation_delivery(&ctx)
    }

    // https://dom.spec.whatwg.org/#dom-childnode-remove
    fn remove(&self, ctx: Ctx<'js>) -> Result<()> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        if parsed.document.base.root_node().id == self.handle.0.node {
            return Err(throw_dom(
                &ctx,
                "HierarchyRequestError",
                "a document cannot be removed",
            ));
        }
        unlink_journaled(&mut parsed, self.handle.0);
        drop(parsed);
        drop(world);
        fixup_focus_after_removal(&ctx, self.handle.0)?;
        schedule_mutation_delivery(&ctx)
    }
}

impl<'js> non_document_type_child_node_generated::NonDocumentTypeChildNode<'js> for JsNode {
    // https://dom.spec.whatwg.org/#dom-nondocumenttypechildnode-previouselementsibling
    fn get_previous_element_sibling(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        element_sibling_value(ctx, self.handle.0, false)
    }

    // https://dom.spec.whatwg.org/#dom-nondocumenttypechildnode-nextelementsibling
    fn get_next_element_sibling(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        element_sibling_value(ctx, self.handle.0, true)
    }
}

impl<'js> document_fragment_generated::DocumentFragment<'js> for JsNode {
    // https://dom.spec.whatwg.org/#dom-nonelementparentnode-getelementbyid
    fn get_element_by_id(&self, ctx: Ctx<'js>, id: rquickjs::String<'js>) -> Result<Value<'js>> {
        element_by_id(&ctx, self.handle.0, &id.to_string()?)
    }
}

/// Moves focus to `id` when it is focusable
/// (<https://html.spec.whatwg.org/multipage/interaction.html#dom-focus>).
fn focus_element(ctx: &Ctx<'_>, id: NodeId) -> Result<()> {
    if is_focusable(ctx, id)? {
        focus_node(ctx, id)?;
    }
    Ok(())
}

impl<'js> html_element_generated::HTMLElement<'js> for JsNode {
    // https://html.spec.whatwg.org/multipage/dom.html#dom-dataset
    fn get_dataset(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.dataset(ctx)
    }

    // https://html.spec.whatwg.org/multipage/interaction.html#dom-click
    fn click(&self, ctx: Ctx<'js>) -> Result<()> {
        element_click(&ctx, self.handle.0)
    }

    // https://html.spec.whatwg.org/multipage/interaction.html#dom-focus
    fn focus(&self, ctx: Ctx<'js>, _options: html_element_generated::FocusOptions) -> Result<()> {
        focus_element(&ctx, self.handle.0)
    }

    // https://html.spec.whatwg.org/multipage/interaction.html#dom-blur
    fn blur(&self, ctx: Ctx<'js>) -> Result<()> {
        blur_node(&ctx, self.handle.0)
    }
}

impl<'js> svg_element_generated::SVGElement<'js> for JsNode {
    // https://html.spec.whatwg.org/multipage/dom.html#dom-dataset
    fn get_dataset(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.dataset(ctx)
    }

    // https://html.spec.whatwg.org/multipage/interaction.html#dom-focus
    fn focus(&self, ctx: Ctx<'js>, _options: svg_element_generated::FocusOptions) -> Result<()> {
        focus_element(&ctx, self.handle.0)
    }

    // https://html.spec.whatwg.org/multipage/interaction.html#dom-blur
    fn blur(&self, ctx: Ctx<'js>) -> Result<()> {
        blur_node(&ctx, self.handle.0)
    }
}

impl<'js> math_ml_element_generated::MathMLElement<'js> for JsNode {
    // https://html.spec.whatwg.org/multipage/dom.html#dom-dataset
    fn get_dataset(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.dataset(ctx)
    }

    // https://html.spec.whatwg.org/multipage/interaction.html#dom-focus
    fn focus(
        &self,
        ctx: Ctx<'js>,
        _options: math_ml_element_generated::FocusOptions,
    ) -> Result<()> {
        focus_element(&ctx, self.handle.0)
    }

    // https://html.spec.whatwg.org/multipage/interaction.html#dom-blur
    fn blur(&self, ctx: Ctx<'js>) -> Result<()> {
        blur_node(&ctx, self.handle.0)
    }
}

impl html_opt_group_element_generated::HTMLOptGroupElement<'_> for JsNode {}

impl<'js> html_button_element_generated::HTMLButtonElement<'js> for JsNode {
    // https://html.spec.whatwg.org/multipage/forms.html#dom-fae-form
    fn get_form(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.form(ctx.clone())
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-fs-formaction
    fn get_form_action(&self, ctx: &Ctx<'js>) -> Result<crate::dom_string::DomString> {
        Ok(self.form_action(ctx.clone())?.into())
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-fs-formmethod
    fn get_form_method(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let method = self.form_method(ctx.clone())?;
        rquickjs::String::from_str(ctx.clone(), &method)
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-fs-formmethod
    fn set_form_method(&self, ctx: &Ctx<'js>, value: rquickjs::String<'js>) -> Result<()> {
        self.set_form_method(ctx.clone(), WebIdlString(value.to_string()?))
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-fs-formenctype
    fn get_form_enctype(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let enctype = self.form_enctype(ctx.clone())?;
        rquickjs::String::from_str(ctx.clone(), &enctype)
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-fs-formenctype
    fn set_form_enctype(&self, ctx: &Ctx<'js>, value: rquickjs::String<'js>) -> Result<()> {
        self.set_form_enctype(ctx.clone(), WebIdlString(value.to_string()?))
    }
}

impl<'js> html_field_set_element_generated::HTMLFieldSetElement<'js> for JsNode {
    // https://html.spec.whatwg.org/multipage/forms.html#dom-fae-form
    fn get_form(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.form(ctx.clone())
    }
}

impl<'js> html_select_element_generated::HTMLSelectElement<'js> for JsNode {
    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-value
    fn get_value(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let value = self.value(ctx.clone())?;
        rquickjs::String::from_str(ctx.clone(), &value)
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-value
    fn set_value(&self, ctx: &Ctx<'js>, value: rquickjs::String<'js>) -> Result<()> {
        self.set_value(ctx.clone(), LegacyNullString(value.to_string()?))
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-selectedindex
    fn get_selected_index(&self, ctx: &Ctx<'js>) -> Result<i32> {
        self.selected_index(ctx.clone())
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-selectedindex
    fn set_selected_index(&self, ctx: &Ctx<'js>, value: i32) -> Result<()> {
        self.set_selected_index(ctx.clone(), value)
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-options
    fn get_options(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.options(ctx.clone())
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-selectedoptions
    fn get_selected_options(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.selected_options(ctx.clone())
    }

    // https://html.spec.whatwg.org/multipage/forms.html#dom-fae-form
    fn get_form(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.form(ctx.clone())
    }
}

impl<'js> html_text_area_element_generated::HTMLTextAreaElement<'js> for JsNode {
    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-textarea-value
    fn get_value(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let value = self.value(ctx.clone())?;
        rquickjs::String::from_str(ctx.clone(), &value)
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-textarea-value
    fn set_value(&self, ctx: &Ctx<'js>, value: rquickjs::String<'js>) -> Result<()> {
        self.set_value(ctx.clone(), LegacyNullString(value.to_string()?))
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-textarea-defaultvalue
    fn get_default_value(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let value = self.default_value(ctx.clone())?;
        rquickjs::String::from_str(ctx.clone(), &value)
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-textarea-defaultvalue
    fn set_default_value(&self, ctx: &Ctx<'js>, value: rquickjs::String<'js>) -> Result<()> {
        self.set_default_value(ctx.clone(), WebIdlString(value.to_string()?))
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-textarea-textlength
    fn get_text_length(&self, ctx: &Ctx<'js>) -> Result<usize> {
        Ok(self.text_length(ctx.clone())? as usize)
    }

    // Selection getters are non-nullable upstream, answering the stored
    // range clamped to the control value.
    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#textFieldSelection

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-textarea/input-selectionstart
    fn get_selection_start(&self, ctx: &Ctx<'js>) -> Result<usize> {
        Ok(selection_start_of(ctx, self.handle.0)? as usize)
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-textarea/input-selectionstart
    fn set_selection_start(&self, ctx: &Ctx<'js>, value: u32) -> Result<()> {
        self.set_selection_start(ctx.clone(), WebIdlUnsignedLong(value))
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-textarea/input-selectionend
    fn get_selection_end(&self, ctx: &Ctx<'js>) -> Result<usize> {
        Ok(selection_end_of(ctx, self.handle.0)? as usize)
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-textarea/input-selectionend
    fn set_selection_end(&self, ctx: &Ctx<'js>, value: u32) -> Result<()> {
        self.set_selection_end(ctx.clone(), WebIdlUnsignedLong(value))
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-textarea/input-selectiondirection
    fn get_selection_direction(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let direction = selection_direction_of(ctx, self.handle.0)?;
        rquickjs::String::from_str(ctx.clone(), direction_name(direction))
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-textarea/input-selectiondirection
    fn set_selection_direction(&self, ctx: &Ctx<'js>, value: rquickjs::String<'js>) -> Result<()> {
        self.set_selection_direction(ctx.clone(), WebIdlString(value.to_string()?))
    }

    // https://html.spec.whatwg.org/multipage/forms.html#dom-fae-form
    fn get_form(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.form(ctx.clone())
    }
}

impl<'js> html_input_element_generated::HTMLInputElement<'js> for JsNode {
    // https://html.spec.whatwg.org/multipage/input.html#dom-input-checked
    fn get_checked(&self, ctx: &Ctx<'js>) -> Result<bool> {
        self.checked(ctx.clone())
    }

    // https://html.spec.whatwg.org/multipage/input.html#dom-input-checked
    fn set_checked(&self, ctx: &Ctx<'js>, value: bool) -> Result<()> {
        self.set_checked(ctx.clone(), value)
    }

    // https://html.spec.whatwg.org/multipage/forms.html#dom-fae-form
    fn get_form(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.form(ctx.clone())
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-fs-formaction
    fn get_form_action(&self, ctx: &Ctx<'js>) -> Result<crate::dom_string::DomString> {
        Ok(self.form_action(ctx.clone())?.into())
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-fs-formmethod
    fn get_form_method(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let method = self.form_method(ctx.clone())?;
        rquickjs::String::from_str(ctx.clone(), &method)
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-fs-formmethod
    fn set_form_method(&self, ctx: &Ctx<'js>, value: rquickjs::String<'js>) -> Result<()> {
        self.set_form_method(ctx.clone(), WebIdlString(value.to_string()?))
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-fs-formenctype
    fn get_form_enctype(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let enctype = self.form_enctype(ctx.clone())?;
        rquickjs::String::from_str(ctx.clone(), &enctype)
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-fs-formenctype
    fn set_form_enctype(&self, ctx: &Ctx<'js>, value: rquickjs::String<'js>) -> Result<()> {
        self.set_form_enctype(ctx.clone(), WebIdlString(value.to_string()?))
    }

    // https://html.spec.whatwg.org/multipage/input.html#dom-input-indeterminate
    fn get_indeterminate(&self, ctx: &Ctx<'js>) -> Result<bool> {
        let _ = ctx;
        Ok(self.indeterminate())
    }

    // https://html.spec.whatwg.org/multipage/input.html#dom-input-indeterminate
    fn set_indeterminate(&self, ctx: &Ctx<'js>, value: bool) -> Result<()> {
        let _ = (ctx, value);
        self.set_indeterminate();
        Ok(())
    }

    // https://html.spec.whatwg.org/multipage/input.html#dom-input-value
    fn get_value(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let value = self.value(ctx.clone())?;
        rquickjs::String::from_str(ctx.clone(), &value)
    }

    // https://html.spec.whatwg.org/multipage/input.html#dom-input-value
    fn set_value(&self, ctx: &Ctx<'js>, value: rquickjs::String<'js>) -> Result<()> {
        self.set_value(ctx.clone(), LegacyNullString(value.to_string()?))
    }

    // Upstream selection accessors are nullable, keeping the spec-prose
    // null for input types where selection does not apply. No selection state
    // is stored (a known cutover gap), so applicable inputs answer zero.
    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#textFieldSelection

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-textarea/input-selectionstart
    fn get_selection_start(&self, ctx: &Ctx<'js>) -> Result<Option<u32>> {
        let world = world(ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(None);
        };
        if !selection_applies(&parsed.document.base, self.handle.0.node) {
            return Ok(None);
        }
        drop(parsed);
        Ok(Some(selection_start_of(ctx, self.handle.0)?))
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-textarea/input-selectionstart
    fn set_selection_start(&self, ctx: &Ctx<'js>, value: Option<u32>) -> Result<()> {
        // Null converts as zero, matching the previous numeric conversion.
        self.set_selection_start(ctx.clone(), WebIdlUnsignedLong(value.unwrap_or(0)))
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-textarea/input-selectionend
    fn get_selection_end(&self, ctx: &Ctx<'js>) -> Result<Option<u32>> {
        let world = world(ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(None);
        };
        if !selection_applies(&parsed.document.base, self.handle.0.node) {
            return Ok(None);
        }
        drop(parsed);
        Ok(Some(selection_end_of(ctx, self.handle.0)?))
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-textarea/input-selectionend
    fn set_selection_end(&self, ctx: &Ctx<'js>, value: Option<u32>) -> Result<()> {
        // Null converts as zero, matching the previous numeric conversion.
        self.set_selection_end(ctx.clone(), WebIdlUnsignedLong(value.unwrap_or(0)))
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-textarea/input-selectiondirection
    fn get_selection_direction(&self, ctx: &Ctx<'js>) -> Result<Option<rquickjs::String<'js>>> {
        let world = world(ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(None);
        };
        if !selection_applies(&parsed.document.base, self.handle.0.node) {
            return Ok(None);
        }
        drop(parsed);
        let direction = selection_direction_of(ctx, self.handle.0)?;
        rquickjs::String::from_str(ctx.clone(), direction_name(direction)).map(Some)
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-textarea/input-selectiondirection
    fn set_selection_direction(
        &self,
        ctx: &Ctx<'js>,
        value: Option<rquickjs::String<'js>>,
    ) -> Result<()> {
        // Null stringified to "null" before, which maps to no direction;
        // answer "none" directly.
        let value = match value {
            Some(value) => value.to_string()?,
            None => "none".into(),
        };
        self.set_selection_direction(ctx.clone(), WebIdlString(value))
    }
}

impl<'js> html_form_element_generated::HTMLFormElement<'js> for JsNode {
    // https://html.spec.whatwg.org/multipage/forms.html#dom-form-action
    fn get_action(&self, ctx: &Ctx<'js>) -> Result<crate::dom_string::DomString> {
        Ok(self.action(ctx.clone())?.into())
    }

    // https://html.spec.whatwg.org/multipage/forms.html#dom-form-method
    fn get_method(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let raw = attribute_value(ctx, self.handle.0, "method")?;
        let method = match raw.trim().to_ascii_lowercase().as_str() {
            "post" => "post",
            "dialog" => "dialog",
            _ => "get",
        };
        rquickjs::String::from_str(ctx.clone(), method)
    }

    // https://html.spec.whatwg.org/multipage/forms.html#dom-form-method
    fn set_method(&self, ctx: &Ctx<'js>, value: rquickjs::String<'js>) -> Result<()> {
        self.set_attribute(
            ctx.clone(),
            WebIdlString("method".into()),
            WebIdlString(value.to_string()?),
        )
    }

    // https://html.spec.whatwg.org/multipage/forms.html#dom-form-enctype
    fn get_enctype(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let raw = attribute_value(ctx, self.handle.0, "enctype")?;
        rquickjs::String::from_str(ctx.clone(), encoding_keyword(&raw))
    }

    // https://html.spec.whatwg.org/multipage/forms.html#dom-form-enctype
    fn set_enctype(&self, ctx: &Ctx<'js>, value: rquickjs::String<'js>) -> Result<()> {
        self.set_attribute(
            ctx.clone(),
            WebIdlString("enctype".into()),
            WebIdlString(value.to_string()?),
        )
    }

    // https://html.spec.whatwg.org/multipage/forms.html#dom-form-encoding
    fn get_encoding(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        self.get_enctype(ctx)
    }

    // https://html.spec.whatwg.org/multipage/forms.html#dom-form-encoding
    fn set_encoding(&self, ctx: &Ctx<'js>, value: rquickjs::String<'js>) -> Result<()> {
        self.set_enctype(ctx, value)
    }

    // https://html.spec.whatwg.org/multipage/forms.html#dom-form-reset
    fn reset(&self, ctx: Ctx<'js>) -> Result<()> {
        self.reset_form(ctx)
    }
}

impl<'js> html_option_element_generated::HTMLOptionElement<'js> for JsNode {
    // https://html.spec.whatwg.org/multipage/forms.html#dom-fae-form
    fn get_form(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.form(ctx.clone())
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-label
    fn get_label(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let label = self.label(ctx.clone())?;
        rquickjs::String::from_str(ctx.clone(), &label)
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-selected
    fn get_selected(&self, ctx: &Ctx<'js>) -> Result<bool> {
        self.selected(ctx.clone())
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-selected
    fn set_selected(&self, ctx: &Ctx<'js>, value: bool) -> Result<()> {
        self.set_selected(ctx.clone(), value)
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-value
    fn get_value(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let value = self.value(ctx.clone())?;
        rquickjs::String::from_str(ctx.clone(), &value)
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-text
    fn get_text(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let text = self.text(ctx.clone())?;
        rquickjs::String::from_str(ctx.clone(), &text)
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-text
    fn set_text(&self, ctx: &Ctx<'js>, value: rquickjs::String<'js>) -> Result<()> {
        // DOMString keeps every code unit, including lone surrogates.
        self.set_text(
            ctx.clone(),
            WebIdlCodeUnits(crate::dom_string::DomString::from_utf16(value.to_utf16()?)),
        )
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-index
    fn get_index(&self, ctx: &Ctx<'js>) -> Result<i32> {
        self.index(ctx.clone())
    }
}

impl<'js> shadow_root_generated::ShadowRoot<'js> for JsNode {
    // https://dom.spec.whatwg.org/#dom-shadowroot-host
    fn get_host(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.host(ctx)
    }

    // https://dom.spec.whatwg.org/#dom-shadowroot-mode
    fn get_mode(&self, ctx: &Ctx<'js>) -> Result<shadow_root_generated::ShadowRootMode> {
        Ok(match self.mode(ctx)?.as_str() {
            "closed" => shadow_root_generated::ShadowRootMode::Closed,
            _ => shadow_root_generated::ShadowRootMode::Open,
        })
    }

    // https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-innerhtml
    fn get_inner_html(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        self.inner_html(ctx)
    }

    // https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-innerhtml
    fn set_inner_html(&self, ctx: &Ctx<'js>, value: rquickjs::String<'js>) -> Result<()> {
        self.set_inner_html(ctx, LegacyNullString(value.to_string()?))
    }

    // https://dom.spec.whatwg.org/#dom-documentorshadowroot-activeelement
    fn get_active_element(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.active_element(ctx)
    }
}

impl<'js> htmli_frame_element_generated::HTMLIFrameElement<'js> for JsNode {
    // https://html.spec.whatwg.org/multipage/iframe-embed-object.html#dom-iframe-contentdocument
    fn get_content_document(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.content_document(ctx.clone())
    }

    // https://html.spec.whatwg.org/multipage/iframe-embed-object.html#dom-iframe-contentwindow
    fn get_content_window(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.content_window(ctx.clone())
    }
}

impl<'js> html_frame_element_generated::HTMLFrameElement<'js> for JsNode {
    // https://html.spec.whatwg.org/multipage/obsolete.html#dom-frame-contentdocument
    fn get_content_document(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.content_document(ctx.clone())
    }

    // https://html.spec.whatwg.org/multipage/obsolete.html#dom-frame-contentwindow
    fn get_content_window(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.content_window(ctx.clone())
    }
}

impl<'js> html_image_element_generated::HTMLImageElement<'js> for JsNode {
    // https://html.spec.whatwg.org/multipage/embedded-content.html#dom-img-naturalwidth
    fn get_natural_width(&self, ctx: &Ctx<'js>) -> Result<usize> {
        Ok(self.natural_width(ctx.clone())? as usize)
    }

    // https://html.spec.whatwg.org/multipage/embedded-content.html#dom-img-naturalheight
    fn get_natural_height(&self, ctx: &Ctx<'js>) -> Result<usize> {
        Ok(self.natural_height(ctx.clone())? as usize)
    }

    // https://html.spec.whatwg.org/multipage/embedded-content.html#dom-img-complete
    fn get_complete(&self, ctx: &Ctx<'js>) -> Result<bool> {
        self.complete(ctx.clone())
    }

    // https://html.spec.whatwg.org/multipage/embedded-content.html#dom-img-currentsrc
    fn get_current_src(&self, ctx: &Ctx<'js>) -> Result<crate::dom_string::DomString> {
        Ok(self.current_src(ctx.clone())?.into())
    }
}

/// `HTMLHyperlinkElementUtils` is a spec mixin included by `HTMLAnchorElement`
/// and `HTMLAreaElement`, so the one contract installs on both prototypes
/// from the IDL includes
/// (<https://html.spec.whatwg.org/multipage/links.html#htmlhyperlinked>).
impl<'js> html_hyperlink_element_utils_generated::HTMLHyperlinkElementUtils<'js> for JsNode {
    // https://html.spec.whatwg.org/multipage/links.html#dom-hyperlink-href
    fn get_href(&self, ctx: &Ctx<'js>) -> Result<crate::dom_string::DomString> {
        host::reflect_url_string(ctx, self.handle.0, "href")
    }
}

impl<'js> html_base_element_generated::HTMLBaseElement<'js> for JsNode {
    // Unlike generic URL reflection, an absent `href` falls back to the
    // document base URL rather than the empty string
    // (<https://html.spec.whatwg.org/multipage/semantics.html#dom-base-href>).
    fn get_href(&self, ctx: &Ctx<'js>) -> Result<crate::dom_string::DomString> {
        // Probe the node's owner document, not the calling realm's: a node
        // reached across documents must answer from its own attributes.
        let present = world_for_node(ctx, self.handle.0)
            .ok()
            .and_then(|owner| {
                owner.borrow().with_document(self.handle.0, |parsed| {
                    attr(&parsed.document.base, self.handle.0.node, "href").is_some()
                })
            })
            .unwrap_or(false);
        if present {
            host::reflect_url_string(ctx, self.handle.0, "href")
        } else {
            Ok(document_base_url_string(ctx, self.handle.0).into())
        }
    }
}

impl html_link_element_generated::HTMLLinkElement<'_> for JsNode {}

impl<'js> html_media_element_generated::HTMLMediaElement<'js> for JsNode {
    // No media pipeline: the network state never leaves its initial value
    // (<https://html.spec.whatwg.org/multipage/media.html#dom-media-networkstate>).
    fn get_network_state(&self, ctx: &Ctx<'js>) -> Result<u16> {
        // The sync section of resource selection picks a candidate before any
        // fetch runs: a `src` attribute, or a `<source>` child that itself
        // has `src`, moves the state out of NETWORK_EMPTY. With no media
        // pipeline nothing runs past this point, so a candidate reads as
        // NETWORK_NO_SOURCE and its absence as NETWORK_EMPTY
        // (<https://html.spec.whatwg.org/multipage/media.html#concept-media-load-algorithm>).
        const NETWORK_NO_SOURCE: u16 = 3;
        const NETWORK_EMPTY: u16 = 0;
        let selected = world_for_node(ctx, self.handle.0)
            .ok()
            .and_then(|owner| {
                owner.borrow().with_document(self.handle.0, |parsed| {
                    let base = &parsed.document.base;
                    if attr(base, self.handle.0.node, "src").is_some() {
                        return true;
                    }
                    base.get_node(self.handle.0.node).is_some_and(|node| {
                        node.children.iter().any(|kid| {
                            base.get_node(*kid).is_some_and(|candidate| {
                                candidate.data.downcast_element().is_some_and(|element| {
                                    element.name.ns == html_namespace()
                                        && element.name.local.as_ref() == "source"
                                })
                            }) && attr(base, *kid, "src").is_some()
                        })
                    })
                })
            })
            .unwrap_or(false);
        Ok(if selected {
            NETWORK_NO_SOURCE
        } else {
            NETWORK_EMPTY
        })
    }

    // No media pipeline: the ready state never leaves its initial value
    // (<https://html.spec.whatwg.org/multipage/media.html#dom-media-readystate>).
    fn get_ready_state(&self, _ctx: &Ctx<'js>) -> Result<u16> {
        Ok(0)
    }
}

impl html_embed_element_generated::HTMLEmbedElement<'_> for JsNode {}

impl html_script_element_generated::HTMLScriptElement<'_> for JsNode {}

impl html_source_element_generated::HTMLSourceElement<'_> for JsNode {}

impl html_track_element_generated::HTMLTrackElement<'_> for JsNode {}

impl html_meta_element_generated::HTMLMetaElement<'_> for JsNode {}

impl html_map_element_generated::HTMLMapElement<'_> for JsNode {}

impl html_object_element_generated::HTMLObjectElement<'_> for JsNode {}

impl html_output_element_generated::HTMLOutputElement<'_> for JsNode {}

impl html_param_element_generated::HTMLParamElement<'_> for JsNode {}

impl html_slot_element_generated::HTMLSlotElement<'_> for JsNode {}

impl<'js> html_template_element_generated::HTMLTemplateElement<'js> for JsNode {
    // https://html.spec.whatwg.org/multipage/scripting.html#dom-template-contents
    fn get_content(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        // Known gap: Blitz has no template contents, so templates expose no
        // separate content fragment.
        Ok(Value::new_null(ctx.clone()))
    }
}

impl<'js> processing_instruction_generated::ProcessingInstruction<'js> for JsNode {
    // https://dom.spec.whatwg.org/#dom-processinginstruction-target
    fn get_target(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        // Known gap: Blitz has no processing-instruction nodes, so the brand
        // never instantiates and the target reads empty.
        let _ = self.handle;
        rquickjs::String::from_str(ctx.clone(), "")
    }
}

impl<'js> document_type_generated::DocumentType<'js> for JsNode {
    // https://dom.spec.whatwg.org/#dom-documenttype-name
    fn get_name(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        // Known gap: Blitz has no doctype nodes, so the brand never
        // instantiates and the name reads empty.
        let _ = self.handle;
        rquickjs::String::from_str(ctx.clone(), "")
    }

    // https://dom.spec.whatwg.org/#dom-documenttype-publicid
    fn get_public_id(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let world = world(ctx)?;
        let world = world.borrow();
        let value = world
            .document(self.handle.0)
            .and_then(|parsed| doctype_fields(&parsed, self.handle.0))
            .map_or(String::new(), |(_, public_id, _)| public_id);
        rquickjs::String::from_str(ctx.clone(), &value)
    }

    // https://dom.spec.whatwg.org/#dom-documenttype-systemid
    fn get_system_id(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let world = world(ctx)?;
        let world = world.borrow();
        let value = world
            .document(self.handle.0)
            .and_then(|parsed| doctype_fields(&parsed, self.handle.0))
            .map_or(String::new(), |(_, _, system_id)| system_id);
        rquickjs::String::from_str(ctx.clone(), &value)
    }
}

impl<'js> character_data_generated::CharacterData<'js> for JsNode {
    // https://dom.spec.whatwg.org/#dom-characterdata-data
    fn get_data(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        dom_string(ctx, &character_data(ctx, self.handle.0)?)
    }

    // https://dom.spec.whatwg.org/#dom-characterdata-data
    fn set_data(&self, ctx: &Ctx<'js>, value: rquickjs::String<'js>) -> Result<()> {
        let data = crate::dom_string::DomString::from_utf16(value.to_utf16()?);
        set_character_data(ctx, self.handle.0, &data)
    }

    // https://dom.spec.whatwg.org/#dom-characterdata-length
    fn get_length(&self, ctx: &Ctx<'js>) -> Result<usize> {
        Ok(character_data(ctx, self.handle.0)?.len())
    }

    // https://dom.spec.whatwg.org/#dom-characterdata-substringdata
    fn substring_data(
        &self,
        ctx: Ctx<'js>,
        offset: u32,
        count: u32,
    ) -> Result<rquickjs::String<'js>> {
        let units = character_data(&ctx, self.handle.0)?.units().into_owned();
        let offset = character_data_offset(&ctx, offset, units.len())?;
        let end = offset
            .saturating_add(usize::try_from(count).unwrap_or(usize::MAX))
            .min(units.len());
        rquickjs::String::from_utf16(ctx.clone(), &units[offset..end])
    }

    // https://dom.spec.whatwg.org/#dom-characterdata-appenddata
    fn append_data(&self, ctx: Ctx<'js>, data: rquickjs::String<'js>) -> Result<()> {
        let mut current = character_data(&ctx, self.handle.0)?;
        current.push_dom(&crate::dom_string::DomString::from_utf16(data.to_utf16()?));
        set_character_data(&ctx, self.handle.0, &current)
    }

    // https://dom.spec.whatwg.org/#dom-characterdata-insertdata
    fn insert_data(&self, ctx: Ctx<'js>, offset: u32, data: rquickjs::String<'js>) -> Result<()> {
        let mut units = character_data(&ctx, self.handle.0)?.units().into_owned();
        let offset = character_data_offset(&ctx, offset, units.len())?;
        let insert = data.to_utf16()?;
        units.splice(offset..offset, insert.iter().copied());
        set_character_data(
            &ctx,
            self.handle.0,
            &crate::dom_string::DomString::from_utf16(units),
        )
    }

    // https://dom.spec.whatwg.org/#dom-characterdata-deletedata
    fn delete_data(&self, ctx: Ctx<'js>, offset: u32, count: u32) -> Result<()> {
        let mut units = character_data(&ctx, self.handle.0)?.units().into_owned();
        let offset = character_data_offset(&ctx, offset, units.len())?;
        let end = offset
            .saturating_add(usize::try_from(count).unwrap_or(usize::MAX))
            .min(units.len());
        units.drain(offset..end);
        set_character_data(
            &ctx,
            self.handle.0,
            &crate::dom_string::DomString::from_utf16(units),
        )
    }

    // https://dom.spec.whatwg.org/#dom-characterdata-replacedata
    fn replace_data(
        &self,
        ctx: Ctx<'js>,
        offset: u32,
        count: u32,
        data: rquickjs::String<'js>,
    ) -> Result<()> {
        let mut units = character_data(&ctx, self.handle.0)?.units().into_owned();
        let offset = character_data_offset(&ctx, offset, units.len())?;
        let end = offset
            .saturating_add(usize::try_from(count).unwrap_or(usize::MAX))
            .min(units.len());
        let replacement = data.to_utf16()?;
        units.splice(offset..end, replacement.iter().copied());
        set_character_data(
            &ctx,
            self.handle.0,
            &crate::dom_string::DomString::from_utf16(units),
        )
    }
}

/// The `SelectionMode` direction name for the stored code
/// (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#selection-direction>).
fn direction_name(direction: u8) -> &'static str {
    match direction {
        1 => "forward",
        2 => "backward",
        _ => "none",
    }
}

/// Maps a `selectionDirection` string to its stored code; anything but the two
/// known keywords is "none".
fn direction_code(direction: &str) -> u8 {
    match direction {
        "forward" => 1,
        "backward" => 2,
        _ => 0,
    }
}

/// The form `enctype` keyword for a raw attribute value: the three known
/// keywords, case-insensitively, with the urlencoded default for a missing or
/// invalid value
/// (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#attr-fs-enctype>).
fn encoding_keyword(raw: &str) -> &'static str {
    match raw.trim().to_ascii_lowercase().as_str() {
        "multipart/form-data" => "multipart/form-data",
        "text/plain" => "text/plain",
        _ => "application/x-www-form-urlencoded",
    }
}
