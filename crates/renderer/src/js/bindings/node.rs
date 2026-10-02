//! The single `Node` wrapper class and its members.

use super::{
    AttrArgument, CollectionKind, FromJs, ImportSnapshot, JsImplementation, JsNamedNodeMap,
    JsTokenList, LegacyNullString, NodeContext, OptString, Trace, WebIdlCodeUnits, WebIdlString,
    WebIdlUnsignedLong, adopt_across_documents, after_attribute_change, ancestor_chain,
    attached_attr_id, attr_owner, attr_state, attr_wrapper, attribute_local_name, attribute_value,
    blur_node, character_data, character_data_offset, child_value, clone_document,
    convert_nodes_into_node, create_element_named, create_html_element, create_kind, deref_weak,
    descendant_text, doctype_fields, document_base_url_string, document_is_html,
    document_is_html_content, document_url_string, dom_string, element_at_point, element_box,
    element_click, element_node_name, element_sibling_value, elements_by_tag, find_element_by_id,
    fixup_focus_after_removal, focus_node, host_node_id, import_snapshot, is_element, is_focusable,
    is_html_element, is_main_document, is_template_element, live_collection, locate_namespace,
    locate_prefix, main_document, make_weak, materialize_children, materialize_import,
    new_detached_attr, nodes_equal, qualified_name, rect_object, refresh_named_node_map,
    remove_attribute_sync, required_node, root_of, schedule_mutation_delivery, select_error,
    set_attribute_node, set_character_data, sibling_value, string_value, throw_dom,
    throw_dom_error, touch_attr, tree_order, valid_attribute_local_name, validate_and_extract,
    webidl_to_string, with_node_kind, world, world_for_node, wrap_new_document, wrap_node,
};
use rquickjs::function::Rest;

use dom::{LocalName, NodeId, NodeKind, QualName, html_namespace, svg_namespace};

use rquickjs::{Array, Class, Ctx, Exception, Function, Persistent, Result, Value};

use crate::js::events::{self, JsEvent};

use crate::js::world::{DocumentStreamCommand, EventTargetKey, Handle, NodeReference, Wrapper};

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
include!(concat!(env!("OUT_DIR"), "/HTMLImageElement.rs"));
include!(concat!(env!("OUT_DIR"), "/ElementReflections.rs"));

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
    html_image_element_generated::install(ctx)?;
    element_reflections_generated::install(ctx)
}

fn insertion_tree_nodes(
    ctx: &Ctx<'_>,
    parent: NodeId,
    node: NodeReference,
    child: Option<NodeReference>,
) -> Result<(NodeId, Option<NodeId>)> {
    // https://dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity
    // https://dom.spec.whatwg.org/#concept-node-replace
    let owner = world_for_node(ctx, parent)?;
    let owner = owner.borrow();
    let parsed = owner
        .document(parent)
        .ok_or_else(|| Exception::throw_type(ctx, "stale parent"))?;
    let document = &parsed.document;
    if !matches!(
        document.kind(parent),
        Some(NodeKind::Document | NodeKind::Fragment | NodeKind::Element { .. })
    ) {
        return Err(throw_dom(
            ctx,
            "HierarchyRequestError",
            "parent cannot have children",
        ));
    }
    let reference = child.and_then(NodeReference::tree);
    if let Some(node) = node.tree() {
        dom::mutation::validate_pre_insert(document, parent, node, reference)
            .map_err(|error| throw_dom_error(ctx, error))?;
    } else if reference.is_some_and(|child| document.parent(child) != Some(parent)) {
        return Err(throw_dom(
            ctx,
            "NotFoundError",
            "reference is not a child of parent",
        ));
    }
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
    Ok((node, reference))
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
    let owner = world_for_node(ctx, node1)?;
    let owner = owner.borrow();
    let parsed = owner
        .document(node1)
        .ok_or_else(|| Exception::throw_type(ctx, "stale node"))?;
    let dom = &parsed.document;
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
        for attribute in dom.attributes(node1).unwrap_or_default() {
            if attribute.name.ns.as_ref() == a.namespace && attribute.name.local.as_ref() == a.local
            {
                return Ok(IMPLEMENTATION_SPECIFIC | PRECEDING);
            }
            if attribute.name.ns.as_ref() == b.namespace && attribute.name.local.as_ref() == b.local
            {
                return Ok(IMPLEMENTATION_SPECIFIC | FOLLOWING);
            }
        }
    }
    if root_of(dom, node1) != root_of(dom, node2) {
        return Ok(disconnected);
    }
    let attr1 = matches!(other, NodeReference::Attribute { .. });
    let attr2 = matches!(this, NodeReference::Attribute { .. });
    let node1_ancestor = ancestor_chain(dom, node2).contains(&node1);
    let node2_ancestor = ancestor_chain(dom, node1).contains(&node2);
    if (node1_ancestor && !attr1) || (node1 == node2 && attr2) {
        return Ok(CONTAINS | PRECEDING);
    }
    if (node2_ancestor && !attr2) || (node1 == node2 && attr1) {
        return Ok(CONTAINED_BY | FOLLOWING);
    }
    Ok(
        if tree_order(dom, node1, node2) == std::cmp::Ordering::Less {
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
            create_kind(&ctx, document, |dom| dom.create_text(data))
        }
        "Comment" => {
            let data = constructor_units(&ctx, first)?;
            let document = main_document(&ctx)?;
            create_kind(&ctx, document, |dom| dom.create_comment(data))
        }
        "DocumentFragment" => {
            let document = main_document(&ctx)?;
            create_kind(&ctx, document, dom::Document::create_fragment)
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
    let is_element = with_node_kind(&ctx, id, |kind| {
        matches!(kind, Some(NodeKind::Element { .. }))
    })?;
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
fn constructor_units<'js>(ctx: &Ctx<'js>, value: Option<Value<'js>>) -> Result<dom::DomString> {
    match value {
        None => Ok(dom::DomString::default()),
        Some(value) if value.is_undefined() => Ok(dom::DomString::default()),
        Some(value) => Ok(dom::DomString::from_utf16(super::webidl_to_units(
            ctx, value,
        )?)),
    }
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

/// Parses `markup` as an HTML fragment in `context` and snapshots the
/// resulting nodes for insertion into a document.
fn parse_html_fragment_snapshots(
    ctx: &Ctx<'_>,
    markup: &str,
    context: &str,
) -> Result<Vec<ImportSnapshot>> {
    let parsed_fragment = crate::parse_html_fragment(markup, context, true);
    let fragment_root = parsed_fragment
        .document
        .children(parsed_fragment.document.document())
        .and_then(|mut children| {
            children.find(|&id| {
                matches!(
                    parsed_fragment.document.kind(id),
                    Some(NodeKind::Element { name, .. })
                        if name.ns == html_namespace() && name.local.as_ref() == "html"
                )
            })
        })
        .ok_or_else(|| Exception::throw_internal(ctx, "fragment parser omitted its root"))?;
    Ok(parsed_fragment
        .document
        .children(fragment_root)
        .map(|children| {
            children
                .filter_map(|child| import_snapshot(&parsed_fragment.document, child, true))
                .collect()
        })
        .unwrap_or_default())
}

/// The first node matching `selector` under `parsed`'s document root.
fn document_first(parsed: &crate::Parsed, selector: &str) -> Option<NodeId> {
    dom::selector::select_first(&parsed.document, parsed.document.document(), selector)
        .ok()
        .flatten()
}

/// The first child of `parsed`'s document root satisfying `want`.
fn document_first_child(parsed: &crate::Parsed, want: fn(&NodeKind) -> bool) -> Option<NodeId> {
    parsed
        .document
        .children(parsed.document.document())
        .into_iter()
        .flatten()
        .find(|&id| parsed.document.kind(id).is_some_and(want))
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

/// Whether `id` is an HTML `iframe`, registering any pending browsing
/// contexts before the caller looks its frame up. A script may have appended
/// the iframe in this same task, so the browsing context is registered first;
/// its realm follows at the next non-JS turn
/// (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#process-the-iframe-attributes>).
fn iframe_frame(ctx: &Ctx<'_>, id: NodeId) -> Result<bool> {
    let is_iframe = with_node_kind(ctx, id, |kind| is_html_element(kind, "iframe"))?;
    if is_iframe {
        world(ctx)?.borrow_mut().register_pending_frames();
    }
    Ok(is_iframe)
}

fn img_size(ctx: &Ctx<'_>, id: NodeId) -> Result<Option<(u32, u32)>> {
    if !with_node_kind(ctx, id, |kind| is_html_element(kind, "img"))? {
        return Ok(None);
    }
    let world = world(ctx)?;
    Ok(world
        .borrow()
        .images
        .get(&id)
        .map(|image| (image.width, image.height)))
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
        with_node_kind(ctx, self.handle.0, |kind| match kind {
            Some(NodeKind::Element { .. }) => Ok(1),
            Some(NodeKind::Text { .. }) => Ok(3),
            Some(NodeKind::CDataSection { .. }) => Ok(4),
            Some(NodeKind::ProcessingInstruction { .. }) => Ok(7),
            Some(NodeKind::Comment { .. }) => Ok(8),
            Some(NodeKind::Document) => Ok(9),
            Some(NodeKind::Doctype { .. }) => Ok(10),
            Some(NodeKind::Fragment) => Ok(11),
            None => Err(Exception::throw_type(ctx, "stale node")),
        })?
    }

    // https://dom.spec.whatwg.org/#dom-node-nodename
    // https://dom.spec.whatwg.org/#concept-element-html-uppercased-qualified-name
    #[qjs(skip)]
    fn node_name<'js>(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let uppercase = document_is_html_content(ctx, self.handle.0);
        let name = with_node_kind(ctx, self.handle.0, |kind| match kind {
            Some(NodeKind::Element { name, .. }) => Ok(element_node_name(name, uppercase)),
            Some(NodeKind::Text { .. }) => Ok("#text".into()),
            Some(NodeKind::CDataSection { .. }) => Ok("#cdata-section".into()),
            Some(NodeKind::ProcessingInstruction { target, .. }) => Ok(target.clone()),
            Some(NodeKind::Comment { .. }) => Ok("#comment".into()),
            Some(NodeKind::Document) => Ok("#document".into()),
            Some(NodeKind::Doctype { name, .. }) => Ok(name.clone()),
            Some(NodeKind::Fragment) => Ok("#document-fragment".into()),
            None => Err(Exception::throw_type(ctx, "stale node")),
        })?;
        let name = name?;
        rquickjs::String::from_str(ctx.clone(), &name)
    }

    // https://dom.spec.whatwg.org/#dom-node-firstchild
    #[qjs(skip)]
    fn first_child<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let world = world(ctx)?;
        let id = world
            .borrow()
            .with_document(self.handle.0, |parsed| {
                parsed
                    .document
                    .children(self.handle.0)
                    .and_then(|mut kids| kids.next())
            })
            .flatten();
        child_value(ctx, id)
    }

    #[qjs(skip)]
    fn parent_node<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let world = world(ctx)?;
        let id = world
            .borrow()
            .document(self.handle.0)
            .and_then(|parsed| parsed.document.parent(self.handle.0));
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
        let node = adopt_across_documents(&ctx, self.handle.0, node)?;
        let parent = self.handle.0;
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        dom::mutation::pre_insert(&mut parsed.document, parent, node, None)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        drop(parsed);
        drop(world);
        schedule_mutation_delivery(&ctx)?;
        wrap_node(&ctx, node)
    }

    // https://dom.spec.whatwg.org/#dom-eventtarget-addeventlistener
    #[qjs(skip)]
    fn add_event_listener<'js>(
        &self,
        ctx: Ctx<'js>,
        typ: rquickjs::String<'js>,
        callback: Value<'js>,
        options: Value<'js>,
    ) -> Result<()> {
        events::add_listener(
            &ctx,
            EventTargetKey::Node(self.handle.0),
            typ.into_value(),
            callback,
            Some(options),
        )
    }

    // https://dom.spec.whatwg.org/#dom-eventtarget-removeeventlistener
    #[qjs(skip)]
    fn remove_event_listener<'js>(
        &self,
        ctx: Ctx<'js>,
        typ: rquickjs::String<'js>,
        callback: Value<'js>,
        options: Value<'js>,
    ) -> Result<()> {
        events::remove_listener(
            &ctx,
            EventTargetKey::Node(self.handle.0),
            typ.into_value(),
            callback,
            Some(options),
        )
    }

    // https://dom.spec.whatwg.org/#dom-eventtarget-dispatchevent
    #[qjs(skip)]
    fn dispatch_event<'js>(&self, ctx: Ctx<'js>, event: Value<'js>) -> Result<bool> {
        let event = Class::<JsEvent>::from_js(&ctx, event)?;
        events::dispatch_event(&ctx, EventTargetKey::Node(self.handle.0), &event)
    }

    // https://html.spec.whatwg.org/multipage/interaction.html#dom-click
    #[qjs(skip)]
    fn click(&self, ctx: Ctx<'_>) -> Result<()> {
        element_click(&ctx, self.handle.0)
    }

    // https://html.spec.whatwg.org/multipage/interaction.html#dom-focus
    #[qjs(skip)]
    fn focus(&self, ctx: Ctx<'_>) -> Result<()> {
        if is_focusable(&ctx, self.handle.0)? {
            focus_node(&ctx, self.handle.0)?;
        }
        Ok(())
    }

    // https://html.spec.whatwg.org/multipage/interaction.html#dom-blur
    #[qjs(skip)]
    fn blur(&self, ctx: Ctx<'_>) -> Result<()> {
        blur_node(&ctx, self.handle.0)
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
                .is_some_and(|parsed| dom::lifecycle::is_connected(&parsed.document, node))
        {
            return wrap_node(ctx, node);
        }
        let fallback = {
            let world = world.borrow();
            let Some(parsed) = world.document(document) else {
                return Ok(Value::new_null(ctx.clone()));
            };
            document_first(&parsed, "body").or_else(|| {
                document_first_child(&parsed, |kind| matches!(kind, NodeKind::Element { .. }))
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
                let (scroll_x, scroll_y) = super::viewport_scroll(&ctx, self.handle.0)?;
                Ok(rect_object(&ctx, left - scroll_x, top - scroll_y, width, height)?.into_value())
            }
            None => Ok(rect_object(&ctx, 0.0, 0.0, 0.0, 0.0)?.into_value()),
        }
    }

    #[qjs(skip)]
    fn get_client_rects<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let array = Array::new(ctx.clone())?;
        if let Some((left, top, width, height)) = element_box(&ctx, self.handle.0)? {
            let (scroll_x, scroll_y) = super::viewport_scroll(&ctx, self.handle.0)?;
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
    fn scroll_into_view<'js>(&self, ctx: Ctx<'js>, _options: Value<'js>) -> Result<()> {
        let Some((left, top, width, _height)) = element_box(&ctx, self.handle.0)? else {
            return Ok(());
        };
        let world = world_for_node(&ctx, self.handle.0)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        let Some(root) =
            dom::selector::select_first(&parsed.document, parsed.document.document(), "html")
                .ok()
                .flatten()
        else {
            return Ok(());
        };
        let (scroll_x, _scroll_y) = dom::metadata::scroll_offset(&parsed.document, root);
        let viewport_width = f64::from(crate::engine::VIEWPORT_WIDTH);
        let next_x = if left < scroll_x {
            left
        } else if left + width > scroll_x + viewport_width {
            left + width - viewport_width
        } else {
            scroll_x
        };
        // https://drafts.csswg.org/cssom-view/#dom-element-scrollintoview
        let next_y = top;
        dom::metadata::set_scroll_offset(
            &mut parsed.document,
            root,
            next_x.max(0.0),
            next_y.max(0.0),
        );
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

    // https://dom.spec.whatwg.org/#dom-document-createevent
    #[qjs(skip)]
    fn create_event<'js>(&self, ctx: Ctx<'js>, interface: Value<'js>) -> Result<Value<'js>> {
        let interface = webidl_to_string(&ctx, interface)?;
        events::create_event(&ctx, &interface)
    }

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
        create_kind(&ctx, self.handle.0, |dom| dom.create_text(data.0))
    }

    // https://dom.spec.whatwg.org/#dom-document-createcomment
    #[qjs(skip)]
    fn create_comment<'js>(&self, ctx: Ctx<'js>, data: WebIdlCodeUnits) -> Result<Value<'js>> {
        create_kind(&ctx, self.handle.0, |dom| dom.create_comment(data.0))
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
        create_kind(&ctx, self.handle.0, |dom| {
            dom.create_processing_instruction(target.0, data.0)
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
        create_kind(&ctx, self.handle.0, |dom| dom.create_cdata_section(data.0))
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
        create_kind(&ctx, self.handle.0, dom::Document::create_fragment)
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
            if source.document.document() == source_id {
                return Err(throw_dom(
                    &ctx,
                    "NotSupportedError",
                    "cannot import a document",
                ));
            }
            import_snapshot(&source.document, source_id, deep)
        };
        let Some(tree) = tree else {
            return Err(Exception::throw_type(&ctx, "stale node"));
        };
        let world = world_rc.borrow();
        let Some(mut target) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        let id = materialize_import(&mut target.document, &tree)
            .map_err(|err| throw_dom_error(&ctx, err))?;
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
        let world = world(&ctx)?;
        let found = {
            let parsed = world.borrow();
            let Some(parsed) = parsed.document(self.handle.0) else {
                return Ok(Value::new_null(ctx));
            };
            find_element_by_id(&parsed.document, self.handle.0, &id.0)
        };
        match found {
            Some(node) => wrap_node(&ctx, node),
            None => Ok(Value::new_null(ctx)),
        }
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
            document_first_child(parsed, |kind| matches!(kind, NodeKind::Element { .. }))
        })
    }

    // https://dom.spec.whatwg.org/#dom-document-doctype
    #[qjs(skip)]
    fn doctype<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        document_value(ctx, self.handle.0, |parsed| {
            document_first_child(parsed, |kind| matches!(kind, NodeKind::Doctype { .. }))
        })
    }

    // https://dom.spec.whatwg.org/#dom-document-readyState
    #[qjs(skip)]
    fn ready_state(&self, ctx: &Ctx<'_>) -> Result<String> {
        let world = world(ctx)?;
        let world = world.borrow();
        let Some(parsed) = world.document(self.handle.0) else {
            return Ok(String::new());
        };
        if parsed.document.document() != self.handle.0 {
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
    fn url(&self, ctx: &Ctx<'_>) -> Result<dom::DomString> {
        Ok(document_url_string(ctx, self.handle.0).into())
    }

    // https://dom.spec.whatwg.org/#dom-document-documenturi
    #[qjs(skip)]
    #[allow(
        clippy::unnecessary_wraps,
        reason = "generated getters share one fallible call shape"
    )]
    fn document_uri(&self, ctx: &Ctx<'_>) -> Result<dom::DomString> {
        Ok(document_url_string(ctx, self.handle.0).into())
    }

    // https://html.spec.whatwg.org/multipage/urls-and-fetching.html#dom-document-baseuri
    #[qjs(skip)]
    #[allow(
        clippy::unnecessary_wraps,
        reason = "generated getters share one fallible call shape"
    )]
    fn base_uri(&self, ctx: &Ctx<'_>) -> Result<dom::DomString> {
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
        let found = world
            .borrow()
            .document(self.handle.0)
            .and_then(|parsed| parsed.document.attribute(self.handle.0, &local));
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
        {
            let world = world(&ctx)?;
            let world = world.borrow();
            let Some(mut parsed) = world.document_mut(self.handle.0) else {
                return Err(Exception::throw_type(&ctx, "no document"));
            };
            dom::mutation::set_attribute(
                &mut parsed.document,
                self.handle.0,
                &local,
                value.0.clone(),
            )
            .map_err(|err| throw_dom_error(&ctx, err))?;
        }
        touch_attr(&ctx, self.handle.0, "", &local, &value.0)?;
        after_attribute_change(&ctx, self.handle.0, &local)?;
        schedule_mutation_delivery(&ctx)
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
    #[qjs(skip)]
    fn value(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(parsed) = world.document(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        Ok(dom::form::element_value(&parsed.document, self.handle.0).unwrap_or_default())
    }

    // https://html.spec.whatwg.org/multipage/input.html#dom-input-value
    #[qjs(skip)]
    fn set_value(&self, ctx: Ctx<'_>, value: LegacyNullString) -> Result<()> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        dom::form::set_element_value(&mut parsed.document, self.handle.0, value.0)
            .map_err(|err| throw_dom_error(&ctx, err))?;
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
        Ok(dom::form::control_default_value(&parsed.document, self.handle.0).unwrap_or_default())
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-textarea-defaultvalue
    #[qjs(skip)]
    fn set_default_value(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        dom::form::set_control_default_value(&mut parsed.document, self.handle.0, value.0)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        Ok(())
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
        Ok(dom::form::textarea_value(&parsed.document, self.handle.0)
            .map_or(0, |value| value.encode_utf16().count() as u32))
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-textarea/input-selectionstart
    #[qjs(skip)]
    fn selection_start<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(parsed) = world.document(self.handle.0) else {
            return Ok(Value::new_null(ctx));
        };
        match dom::form::selection(&parsed.document, self.handle.0) {
            Some((start, _, _)) => Ok(Value::new_number(ctx, f64::from(start))),
            None => Ok(Value::new_null(ctx)),
        }
    }

    #[qjs(skip)]
    fn set_selection_start(&self, ctx: Ctx<'_>, value: WebIdlUnsignedLong) -> Result<()> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        let Some((_, end, direction)) = dom::form::selection(&parsed.document, self.handle.0)
        else {
            return Err(throw_dom(
                &ctx,
                "InvalidStateError",
                "selectionStart does not apply to this control",
            ));
        };
        let start = value.0;
        let changed = dom::form::set_selection(
            &mut parsed.document,
            self.handle.0,
            start,
            end.max(start),
            direction,
        );
        drop(parsed);
        if changed {
            world.queue_select(self.handle.0);
        }
        Ok(())
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-textarea/input-selectionend
    #[qjs(skip)]
    fn selection_end<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(parsed) = world.document(self.handle.0) else {
            return Ok(Value::new_null(ctx));
        };
        match dom::form::selection(&parsed.document, self.handle.0) {
            Some((_, end, _)) => Ok(Value::new_number(ctx, f64::from(end))),
            None => Ok(Value::new_null(ctx)),
        }
    }

    #[qjs(skip)]
    fn set_selection_end(&self, ctx: Ctx<'_>, value: WebIdlUnsignedLong) -> Result<()> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        let Some((start, _, direction)) = dom::form::selection(&parsed.document, self.handle.0)
        else {
            return Err(throw_dom(
                &ctx,
                "InvalidStateError",
                "selectionEnd does not apply to this control",
            ));
        };
        let changed = dom::form::set_selection(
            &mut parsed.document,
            self.handle.0,
            start,
            value.0,
            direction,
        );
        drop(parsed);
        if changed {
            world.queue_select(self.handle.0);
        }
        Ok(())
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-textarea/input-selectiondirection
    #[qjs(skip)]
    fn selection_direction<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(parsed) = world.document(self.handle.0) else {
            return Ok(Value::new_null(ctx));
        };
        match dom::form::selection(&parsed.document, self.handle.0) {
            Some((_, _, direction)) => Ok(string_value(&ctx, direction_name(direction))?),
            None => Ok(Value::new_null(ctx)),
        }
    }

    #[qjs(skip)]
    fn set_selection_direction(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        let Some((start, end, _)) = dom::form::selection(&parsed.document, self.handle.0) else {
            return Err(throw_dom(
                &ctx,
                "InvalidStateError",
                "selectionDirection does not apply to this control",
            ));
        };
        let direction = direction_code(&value.0);
        let changed =
            dom::form::set_selection(&mut parsed.document, self.handle.0, start, end, direction);
        drop(parsed);
        if changed {
            world.queue_select(self.handle.0);
        }
        Ok(())
    }

    // https://html.spec.whatwg.org/multipage/forms.html#dom-form-reset
    #[qjs(skip)]
    fn reset(&self, ctx: Ctx<'_>) -> Result<()> {
        if !with_node_kind(&ctx, self.handle.0, |kind| is_html_element(kind, "form"))? {
            return Err(throw_dom(
                &ctx,
                "InvalidStateError",
                "reset is only available on a form",
            ));
        }
        let world_rc = world(&ctx)?;
        let world = world_rc.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        // The reset algorithm runs on the form owner's controls: for now, the
        // descendants that are form controls. Form-owner association lands with
        // the form units.
        let mut controls = Vec::new();
        let mut stack = vec![self.handle.0];
        while let Some(node) = stack.pop() {
            let children: Vec<NodeId> = parsed
                .document
                .children(node)
                .map(Iterator::collect)
                .unwrap_or_default();
            for child in children {
                if let Some(NodeKind::Element { name, .. }) = parsed.document.kind(child)
                    && name.ns == html_namespace()
                    && matches!(name.local.as_ref(), "input" | "textarea" | "select")
                {
                    controls.push(child);
                }
                stack.push(child);
            }
        }
        for control in controls {
            dom::form::reset_control(&mut parsed.document, control);
        }
        Ok(())
    }

    // https://html.spec.whatwg.org/multipage/forms.html#dom-form-action
    #[qjs(skip)]
    fn action(&self, ctx: Ctx<'_>) -> Result<String> {
        let world_rc = world(&ctx)?;
        let raw = world_rc
            .borrow()
            .document(self.handle.0)
            .and_then(|parsed| parsed.document.attribute(self.handle.0, "action"));
        let base = document_base_url_string(&ctx, self.handle.0);
        let Some(raw) = raw else {
            return Ok(base);
        };
        Ok(url::Url::parse(&base)
            .ok()
            .and_then(|base| base.join(&raw).ok())
            .map_or(raw, |url| url.to_string()))
    }

    #[qjs(skip)]
    fn set_action(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        self.set_attribute(ctx, WebIdlString("action".into()), value)
    }

    // https://html.spec.whatwg.org/multipage/forms.html#dom-form-method
    #[qjs(skip)]
    fn method(&self, ctx: Ctx<'_>) -> Result<String> {
        let raw = attribute_value(&ctx, self.handle.0, "method")?;
        Ok(match raw.trim().to_ascii_lowercase().as_str() {
            "post" => "post",
            "dialog" => "dialog",
            _ => "get",
        }
        .to_owned())
    }

    #[qjs(skip)]
    fn set_method(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        self.set_attribute(ctx, WebIdlString("method".into()), value)
    }

    // https://html.spec.whatwg.org/multipage/forms.html#dom-form-enctype
    #[qjs(skip)]
    fn enctype(&self, ctx: Ctx<'_>) -> Result<String> {
        Ok(encoding_keyword(&attribute_value(&ctx, self.handle.0, "enctype")?).to_owned())
    }

    #[qjs(skip)]
    fn set_enctype(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        self.set_attribute(ctx, WebIdlString("enctype".into()), value)
    }

    #[qjs(skip)]
    fn encoding(&self, ctx: Ctx<'_>) -> Result<String> {
        Ok(encoding_keyword(&attribute_value(&ctx, self.handle.0, "enctype")?).to_owned())
    }

    #[qjs(skip)]
    fn set_encoding(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        self.set_attribute(ctx, WebIdlString("enctype".into()), value)
    }

    #[qjs(skip)]
    fn set_target(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        self.set_attribute(ctx, WebIdlString("target".into()), value)
    }

    // https://html.spec.whatwg.org/multipage/forms.html#dom-form-novalidate
    #[qjs(skip)]
    fn no_validate(&self, ctx: Ctx<'_>) -> Result<bool> {
        self.attribute_present(&ctx, "novalidate")
    }

    #[qjs(skip)]
    fn set_no_validate(&self, ctx: Ctx<'_>, value: bool) -> Result<()> {
        self.reflect_boolean(ctx, "novalidate", value)
    }

    // https://html.spec.whatwg.org/multipage/forms.html#dom-form-acceptcharset
    #[qjs(skip)]
    fn accept_charset(&self, ctx: Ctx<'_>) -> Result<String> {
        attribute_value(&ctx, self.handle.0, "accept-charset")
    }

    #[qjs(skip)]
    fn set_accept_charset(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        self.set_attribute(ctx, WebIdlString("accept-charset".into()), value)
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-fs-formaction
    #[qjs(skip)]
    fn form_action(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        let raw = world
            .borrow()
            .document(self.handle.0)
            .and_then(|parsed| parsed.document.attribute(self.handle.0, "formaction"));
        let base = document_base_url_string(&ctx, self.handle.0);
        let Some(raw) = raw else {
            return Ok(base);
        };
        Ok(url::Url::parse(&base)
            .ok()
            .and_then(|base| base.join(&raw).ok())
            .map_or(raw, |url| url.to_string()))
    }

    #[qjs(skip)]
    fn set_form_action(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        self.set_attribute(ctx, WebIdlString("formaction".into()), value)
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

    #[qjs(skip)]
    fn form_target(&self, ctx: Ctx<'_>) -> Result<String> {
        attribute_value(&ctx, self.handle.0, "formtarget")
    }

    #[qjs(skip)]
    fn set_form_target(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        self.set_attribute(ctx, WebIdlString("formtarget".into()), value)
    }

    #[qjs(skip)]
    fn form_no_validate(&self, ctx: Ctx<'_>) -> Result<bool> {
        self.attribute_present(&ctx, "formnovalidate")
    }

    #[qjs(skip)]
    fn set_form_no_validate(&self, ctx: Ctx<'_>, value: bool) -> Result<()> {
        self.reflect_boolean(ctx, "formnovalidate", value)
    }

    // https://html.spec.whatwg.org/multipage/input.html#the-pattern-attribute
    #[qjs(skip)]
    fn pattern(&self, ctx: Ctx<'_>) -> Result<String> {
        attribute_value(&ctx, self.handle.0, "pattern")
    }

    #[qjs(skip)]
    fn set_pattern(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        self.set_attribute(ctx, WebIdlString("pattern".into()), value)
    }

    // https://html.spec.whatwg.org/multipage/input.html#the-min-and-max-attributes
    #[qjs(skip)]
    fn min(&self, ctx: Ctx<'_>) -> Result<String> {
        attribute_value(&ctx, self.handle.0, "min")
    }

    #[qjs(skip)]
    fn set_min(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        self.set_attribute(ctx, WebIdlString("min".into()), value)
    }

    #[qjs(skip)]
    fn max(&self, ctx: Ctx<'_>) -> Result<String> {
        attribute_value(&ctx, self.handle.0, "max")
    }

    #[qjs(skip)]
    fn set_max(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        self.set_attribute(ctx, WebIdlString("max".into()), value)
    }

    // https://html.spec.whatwg.org/multipage/input.html#the-step-attribute
    #[qjs(skip)]
    fn step(&self, ctx: Ctx<'_>) -> Result<String> {
        attribute_value(&ctx, self.handle.0, "step")
    }

    #[qjs(skip)]
    fn set_step(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        self.set_attribute(ctx, WebIdlString("step".into()), value)
    }

    // https://drafts.csswg.org/cssom-view/#dom-element-scrollleft
    #[qjs(skip)]
    fn scroll_left(&self, ctx: &Ctx<'_>) -> Result<f64> {
        let world = world(ctx)?;
        let world = world.borrow();
        Ok(world.document(self.handle.0).map_or(0.0, |parsed| {
            dom::metadata::scroll_offset(&parsed.document, self.handle.0).0
        }))
    }

    #[qjs(skip)]
    fn set_scroll_left(&self, ctx: &Ctx<'_>, value: f64) -> Result<()> {
        let world = world(ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        let (_, top) = dom::metadata::scroll_offset(&parsed.document, self.handle.0);
        dom::metadata::set_scroll_offset(&mut parsed.document, self.handle.0, value, top);
        Ok(())
    }

    // https://drafts.csswg.org/cssom-view/#dom-element-scrolltop
    #[qjs(skip)]
    fn scroll_top(&self, ctx: &Ctx<'_>) -> Result<f64> {
        let world = world(ctx)?;
        let world = world.borrow();
        Ok(world.document(self.handle.0).map_or(0.0, |parsed| {
            dom::metadata::scroll_offset(&parsed.document, self.handle.0).1
        }))
    }

    #[qjs(skip)]
    fn set_scroll_top(&self, ctx: &Ctx<'_>, value: f64) -> Result<()> {
        let world = world(ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        let (left, _) = dom::metadata::scroll_offset(&parsed.document, self.handle.0);
        dom::metadata::set_scroll_offset(&mut parsed.document, self.handle.0, left, value);
        Ok(())
    }

    /// Reflecting boolean attribute shared by the form-control states: the
    /// engine exposes one element wrapper, so these answer on every element,
    /// the same shortcut `value` takes.
    #[qjs(skip)]
    fn reflect_boolean(&self, ctx: Ctx<'_>, name: &str, value: bool) -> Result<()> {
        if value {
            self.set_attribute(ctx, WebIdlString(name.into()), WebIdlString(String::new()))
        } else {
            self.remove_attribute(ctx, WebIdlString(name.into()))
        }
    }

    #[qjs(skip)]
    fn attribute_present(&self, ctx: &Ctx<'_>, name: &str) -> Result<bool> {
        let world = world(ctx)?;
        let world = world.borrow();
        Ok(world
            .document(self.handle.0)
            .is_some_and(|parsed| parsed.document.attribute(self.handle.0, name).is_some()))
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-fe-disabled
    #[qjs(skip)]
    fn disabled(&self, ctx: Ctx<'_>) -> Result<bool> {
        self.attribute_present(&ctx, "disabled")
    }

    #[qjs(skip)]
    fn set_disabled(&self, ctx: Ctx<'_>, value: bool) -> Result<()> {
        self.reflect_boolean(ctx, "disabled", value)
    }

    // https://html.spec.whatwg.org/multipage/input.html#dom-input-readonly
    #[qjs(skip)]
    fn read_only(&self, ctx: Ctx<'_>) -> Result<bool> {
        self.attribute_present(&ctx, "readonly")
    }

    #[qjs(skip)]
    fn set_read_only(&self, ctx: Ctx<'_>, value: bool) -> Result<()> {
        self.reflect_boolean(ctx, "readonly", value)
    }

    // https://html.spec.whatwg.org/multipage/input.html#dom-input-required
    #[qjs(skip)]
    fn required(&self, ctx: Ctx<'_>) -> Result<bool> {
        self.attribute_present(&ctx, "required")
    }

    #[qjs(skip)]
    fn set_required(&self, ctx: Ctx<'_>, value: bool) -> Result<()> {
        self.reflect_boolean(ctx, "required", value)
    }

    // https://html.spec.whatwg.org/multipage/select.html#dom-select-multiple
    #[qjs(skip)]
    fn multiple(&self, ctx: Ctx<'_>) -> Result<bool> {
        self.attribute_present(&ctx, "multiple")
    }

    #[qjs(skip)]
    fn set_multiple(&self, ctx: Ctx<'_>, value: bool) -> Result<()> {
        self.reflect_boolean(ctx, "multiple", value)
    }

    // https://html.spec.whatwg.org/multipage/forms.html#dom-fae-form
    #[qjs(skip)]
    fn form<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let form = {
            let world = world(&ctx)?;
            let world = world.borrow();
            world
                .document(self.handle.0)
                .and_then(|parsed| dom::form::form_owner(&parsed.document, self.handle.0))
        };
        match form {
            Some(form) => wrap_node(&ctx, form),
            None => Ok(Value::new_null(ctx)),
        }
    }

    // https://html.spec.whatwg.org/multipage/input.html#dom-input-checked
    #[qjs(skip)]
    fn checked(&self, ctx: Ctx<'_>) -> Result<bool> {
        let world = world(&ctx)?;
        let world = world.borrow();
        Ok(world
            .document(self.handle.0)
            .is_some_and(|parsed| dom::form::checkedness(&parsed.document, self.handle.0)))
    }

    #[qjs(skip)]
    fn set_checked(&self, ctx: Ctx<'_>, value: bool) -> Result<()> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        dom::form::set_input_checkedness(&mut parsed.document, self.handle.0, value);
        Ok(())
    }

    // https://html.spec.whatwg.org/multipage/input.html#dom-input-indeterminate
    #[qjs(skip)]
    fn indeterminate(&self, ctx: Ctx<'_>) -> Result<bool> {
        let world = world(&ctx)?;
        let world = world.borrow();
        Ok(world
            .document(self.handle.0)
            .is_some_and(|parsed| dom::form::indeterminate(&parsed.document, self.handle.0)))
    }

    #[qjs(skip)]
    fn set_indeterminate(&self, ctx: Ctx<'_>, value: bool) -> Result<()> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        dom::form::set_indeterminate(&mut parsed.document, self.handle.0, value);
        Ok(())
    }

    // https://html.spec.whatwg.org/multipage/input.html#dom-input-defaultchecked
    #[qjs(skip)]
    fn default_checked(&self, ctx: Ctx<'_>) -> Result<bool> {
        self.attribute_present(&ctx, "checked")
    }

    #[qjs(skip)]
    fn set_default_checked(&self, ctx: Ctx<'_>, value: bool) -> Result<()> {
        self.reflect_boolean(ctx, "checked", value)
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-selected
    #[qjs(skip)]
    fn selected(&self, ctx: Ctx<'_>) -> Result<bool> {
        let world = world(&ctx)?;
        let world = world.borrow();
        Ok(world
            .document(self.handle.0)
            .is_some_and(|parsed| dom::form::option_selected(&parsed.document, self.handle.0)))
    }

    #[qjs(skip)]
    fn set_selected(&self, ctx: Ctx<'_>, value: bool) -> Result<()> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        dom::form::set_option_selected_in_select(&mut parsed.document, self.handle.0, value);
        Ok(())
    }

    #[qjs(skip)]
    fn default_selected(&self, ctx: Ctx<'_>) -> Result<bool> {
        self.attribute_present(&ctx, "selected")
    }

    #[qjs(skip)]
    fn set_default_selected(&self, ctx: Ctx<'_>, value: bool) -> Result<()> {
        self.reflect_boolean(ctx, "selected", value)
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-label
    #[qjs(skip)]
    fn label(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        let world = world.borrow();
        Ok(world
            .document(self.handle.0)
            .map_or_else(String::new, |parsed| {
                parsed
                    .document
                    .no_namespace_attribute(self.handle.0, "label")
                    .unwrap_or_else(|| dom::form::option_text(&parsed.document, self.handle.0))
            }))
    }

    #[qjs(skip)]
    fn set_label(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        self.set_attribute(ctx, WebIdlString("label".into()), value)
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-text
    #[qjs(skip)]
    fn text(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        let world = world.borrow();
        Ok(world
            .document(self.handle.0)
            .map_or_else(String::new, |parsed| {
                dom::form::option_text(&parsed.document, self.handle.0)
            }))
    }

    #[qjs(skip)]
    fn set_text(&self, ctx: Ctx<'_>, value: WebIdlCodeUnits) -> Result<()> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        let replacement = parsed.document.create_fragment();
        if !value.0.is_empty() {
            let text = parsed.document.create_text(value.0);
            dom::mutation::append(&mut parsed.document, replacement, text)
                .map_err(|err| throw_dom_error(&ctx, err))?;
        }
        dom::mutation::replace_all(&mut parsed.document, self.handle.0, replacement)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        Ok(())
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-index
    #[qjs(skip)]
    fn index(&self, ctx: Ctx<'_>) -> Result<i32> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(parsed) = world.document(self.handle.0) else {
            return Ok(0);
        };
        let Some(select) = dom::form::option_select_owner(&parsed.document, self.handle.0) else {
            return Ok(0);
        };
        for (index, option) in dom::form::select_options(&parsed.document, select)
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
            dom::form::select_selected_index(&parsed.document, self.handle.0)
        }))
    }

    #[qjs(skip)]
    fn set_selected_index(&self, ctx: Ctx<'_>, value: i32) -> Result<()> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        dom::form::set_select_selected_index(&mut parsed.document, self.handle.0, value);
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

    /// URL-reflected `src`: parsed against the document base and stored
    /// serialized, like `href`; an absent attribute reflects as the empty
    /// string (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#reflecting-content-attributes-in-idl-attributes>).
    #[qjs(skip)]
    fn src(&self, ctx: Ctx<'_>) -> Result<String> {
        let world_rc = world(&ctx)?;
        let raw = world_rc
            .borrow()
            .document(self.handle.0)
            .and_then(|parsed| parsed.document.attribute(self.handle.0, "src"));
        let Some(raw) = raw else {
            return Ok(String::new());
        };
        let base = document_base_url_string(&ctx, self.handle.0);
        Ok(url::Url::parse(&base)
            .ok()
            .and_then(|base| base.join(&raw).ok())
            .map_or(raw, |url| url.to_string()))
    }

    #[qjs(skip)]
    fn set_src(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        self.set_attribute(ctx, WebIdlString("src".into()), value)
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
        if !with_node_kind(&ctx, self.handle.0, |kind| is_html_element(kind, "img"))? {
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
        let src = world
            .document(self.handle.0)
            .and_then(|parsed| parsed.document.attribute(self.handle.0, "src"));
        if src.as_deref().is_none_or(str::is_empty) {
            return Ok(true);
        }
        Ok(world.image_broken.contains(&self.handle.0))
    }

    // https://html.spec.whatwg.org/multipage/embedded-content.html#dom-img-currentsrc
    #[qjs(skip)]
    fn current_src(&self, ctx: Ctx<'_>) -> Result<String> {
        if !with_node_kind(&ctx, self.handle.0, |kind| is_html_element(kind, "img"))? {
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

    #[qjs(skip)]
    fn name(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(parsed) = world.document(self.handle.0) else {
            return Ok(String::new());
        };
        Ok(parsed
            .document
            .attribute(self.handle.0, "name")
            .unwrap_or_default())
    }

    #[qjs(skip)]
    fn set_name(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        self.set_attribute(ctx, WebIdlString("name".into()), value)
    }

    #[qjs(skip)]
    fn content<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let template_contents = {
            let world = world.borrow();
            world.document(self.handle.0).and_then(|parsed| {
                is_template_element(parsed.document.kind(self.handle.0))
                    .then(|| dom::shadow::template_contents(&parsed.document, self.handle.0))
                    .flatten()
            })
        };
        if let Some(contents) = template_contents {
            return wrap_node(&ctx, contents);
        }

        let value = world
            .borrow()
            .document(self.handle.0)
            .and_then(|parsed| parsed.document.attribute(self.handle.0, "content"))
            .unwrap_or_default();
        Ok(Value::from_string(rquickjs::String::from_str(ctx, &value)?))
    }

    #[qjs(skip)]
    fn set_content(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        self.set_attribute(ctx, WebIdlString("content".into()), value)
    }

    // https://dom.spec.whatwg.org/#dom-element-attachshadow
    #[qjs(skip)]
    fn attach_shadow<'js>(&self, ctx: Ctx<'js>, init: Value<'js>) -> Result<Value<'js>> {
        let init = init
            .into_object()
            .ok_or_else(|| Exception::throw_type(&ctx, "dictionary must be an object"))?;
        let mode: String = init.get("mode")?;
        let open = match mode.as_str() {
            "open" => true,
            "closed" => false,
            _ => {
                return Err(Exception::throw_type(
                    &ctx,
                    "mode must be 'open' or 'closed'",
                ));
            }
        };
        let local = with_node_kind(&ctx, self.handle.0, |kind| match kind {
            Some(NodeKind::Element { name, .. }) if name.ns == html_namespace() => {
                Some(name.local.to_string())
            }
            _ => None,
        })?;
        let Some(local) = local else {
            return Err(throw_dom(
                &ctx,
                "NotSupportedError",
                "element cannot host a shadow root",
            ));
        };
        let allowed = local.contains('-')
            || matches!(
                local.as_str(),
                "article"
                    | "aside"
                    | "blockquote"
                    | "body"
                    | "div"
                    | "footer"
                    | "h1"
                    | "h2"
                    | "h3"
                    | "h4"
                    | "h5"
                    | "h6"
                    | "header"
                    | "main"
                    | "nav"
                    | "p"
                    | "section"
                    | "span"
            );
        if !allowed {
            return Err(throw_dom(
                &ctx,
                "NotSupportedError",
                "element cannot host a shadow root",
            ));
        }

        let world = world(&ctx)?;
        let world_ref = world.borrow();
        let Some(mut parsed) = world_ref.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        if dom::shadow::shadow_root(&parsed.document, self.handle.0).is_some() {
            return Err(throw_dom(
                &ctx,
                "NotSupportedError",
                "element already hosts a shadow root",
            ));
        }
        let root = dom::shadow::attach_shadow(&mut parsed.document, self.handle.0, open)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        drop(parsed);
        drop(world_ref);
        wrap_node(&ctx, root)
    }

    #[qjs(skip)]
    fn shadow_root<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let world = world(ctx)?;
        let root = world
            .borrow()
            .document(self.handle.0)
            .and_then(|parsed| dom::shadow::open_shadow_root(&parsed.document, self.handle.0));
        child_value(ctx, root)
    }

    #[qjs(skip)]
    fn host<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let world = world(ctx)?;
        let host = world
            .borrow()
            .document(self.handle.0)
            .and_then(|parsed| dom::shadow::shadow_host(&parsed.document, self.handle.0));
        match host {
            Some(host) => wrap_node(ctx, host),
            None => Err(Exception::throw_type(ctx, "not a shadow root")),
        }
    }

    #[qjs(skip)]
    fn mode(&self, ctx: &Ctx<'_>) -> Result<String> {
        let world = world(ctx)?;
        let open = world
            .borrow()
            .document(self.handle.0)
            .and_then(|parsed| dom::shadow::shadow_root_is_open(&parsed.document, self.handle.0))
            .ok_or_else(|| Exception::throw_type(ctx, "not a shadow root"))?;
        Ok(if open { "open" } else { "closed" }.into())
    }

    // https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-element-innerhtml
    #[qjs(skip)]
    fn inner_html<'js>(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let world = world(ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Err(Exception::throw_type(ctx, "no document"));
        };
        if parsed.content_type == "text/html" {
            let markup = crate::serialize::serialize_html_fragment(&parsed.document, self.handle.0);
            dom_string(ctx, &markup)
        } else {
            let markup =
                crate::serialize::serialize_xml_children(&parsed.document, self.handle.0, true)
                    .map_err(|err| throw_dom(ctx, "InvalidStateError", &err.to_string()))?;
            dom_string(ctx, &markup)
        }
    }

    // https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-element-outerhtml
    #[qjs(skip)]
    fn outer_html<'js>(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let world = world(ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Err(Exception::throw_type(ctx, "no document"));
        };
        let Some(NodeKind::Element { name, attributes }) = parsed.document.kind(self.handle.0)
        else {
            return Err(Exception::throw_type(ctx, "outerHTML requires an element"));
        };
        if parsed.content_type == "text/html" {
            let markup = crate::serialize::serialize_html_outer(
                &parsed.document,
                self.handle.0,
                name,
                attributes,
            );
            dom_string(ctx, &markup)
        } else {
            let markup = crate::serialize::serialize_xml(&parsed.document, self.handle.0, true)
                .map_err(|err| throw_dom(ctx, "InvalidStateError", &err.to_string()))?;
            dom_string(ctx, &markup)
        }
    }

    // https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-element-innerhtml
    #[qjs(skip)]
    fn set_inner_html(&self, ctx: &Ctx<'_>, value: LegacyNullString) -> Result<()> {
        let (context, is_template) = with_node_kind(ctx, self.handle.0, |kind| match kind {
            Some(NodeKind::Element { name, .. }) => {
                Some((html_fragment_context(name), is_template_element(kind)))
            }
            _ => None,
        })?
        .or_else(|| {
            let world = world(ctx).ok()?;
            let world = world.borrow();
            let parsed = world.document(self.handle.0)?;
            let host = dom::shadow::shadow_host(&parsed.document, self.handle.0)?;
            let Some(NodeKind::Element { name, .. }) = parsed.document.kind(host) else {
                return None;
            };
            Some((html_fragment_context(name), false))
        })
        .ok_or_else(|| {
            Exception::throw_type(ctx, "innerHTML requires an element or shadow root")
        })?;

        let snapshots = parse_html_fragment_snapshots(ctx, &value.0, &context)?;

        let world = world(ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(ctx, "no document"));
        };
        let target = if is_template {
            if let Some(contents) = dom::shadow::template_contents(&parsed.document, self.handle.0)
            {
                contents
            } else {
                let contents = parsed.document.create_fragment();
                dom::shadow::set_template_contents(&mut parsed.document, self.handle.0, contents)
                    .map_err(|err| throw_dom_error(ctx, err))?;
                contents
            }
        } else {
            self.handle.0
        };
        let replacement = materialize_children(&mut parsed.document, &snapshots)
            .map_err(|err| throw_dom_error(ctx, err))?;
        dom::mutation::replace_all(&mut parsed.document, target, replacement)
            .map_err(|err| throw_dom_error(ctx, err))?;
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
        let context = with_node_kind(&ctx, self.handle.0, |kind| match kind {
            Some(NodeKind::Element { name, .. }) => Some(html_fragment_context(name)),
            _ => None,
        })?
        .ok_or_else(|| Exception::throw_type(&ctx, "insertAdjacentHTML requires an element"))?;
        let snapshots = parse_html_fragment_snapshots(&ctx, &text.0, &context)?;
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        let needs_parent = matches!(position.as_str(), "beforebegin" | "afterend");
        if needs_parent && parsed.document.parent(self.handle.0).is_none() {
            return Err(throw_dom(
                &ctx,
                "NoModificationAllowedError",
                "element has no parent",
            ));
        }
        let fragment = materialize_children(&mut parsed.document, &snapshots)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        let result = if position == "beforebegin" {
            dom::mutation::insert_before(&mut parsed.document, self.handle.0, fragment)
        } else if position == "afterbegin" {
            match parsed.document.first_child(self.handle.0) {
                Some(first) => dom::mutation::insert_before(&mut parsed.document, first, fragment),
                None => dom::mutation::append(&mut parsed.document, self.handle.0, fragment),
            }
        } else if position == "afterend" {
            if let Some(next) = parsed.document.next_sibling(self.handle.0) {
                dom::mutation::insert_before(&mut parsed.document, next, fragment)
            } else {
                let Some(parent) = parsed.document.parent(self.handle.0) else {
                    return Ok(());
                };
                dom::mutation::append(&mut parsed.document, parent, fragment)
            }
        } else {
            dom::mutation::append(&mut parsed.document, self.handle.0, fragment)
        };
        result.map_err(|err| throw_dom_error(&ctx, err))?;
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
            let Some(parent) = parsed.document.parent(self.handle.0) else {
                // A parentless element has nothing to replace.
                return Ok(());
            };
            let Some(kind) = parsed.document.kind(parent).cloned() else {
                return Err(Exception::throw_type(ctx, "no document"));
            };
            match kind {
                NodeKind::Document => {
                    return Err(throw_dom(
                        ctx,
                        "NoModificationAllowedError",
                        "the parent of the element is a Document",
                    ));
                }
                NodeKind::Element { name, .. } => (parent, html_fragment_context(&name)),
                // A DocumentFragment has no parsing context of its own; a
                // body element stands in
                // (<https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-element-outerhtml>).
                _ => (parent, "body".to_owned()),
            }
        };
        let snapshots = parse_html_fragment_snapshots(ctx, &value.0, &context)?;

        let world = world_rc.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(ctx, "no document"));
        };
        let replacement = materialize_children(&mut parsed.document, &snapshots)
            .map_err(|err| throw_dom_error(ctx, err))?;
        dom::mutation::replace_child(&mut parsed.document, parent, replacement, self.handle.0)
            .map_err(|err| throw_dom_error(ctx, err))?;
        drop(parsed);
        drop(world);
        schedule_mutation_delivery(ctx)
    }

    /// URL-reflected `href`: parsed against the document base and stored
    /// serialized (percent-encoded).
    #[qjs(skip)]
    fn href(&self, ctx: Ctx<'_>) -> Result<String> {
        let world_rc = world(&ctx)?;
        let raw = world_rc
            .borrow()
            .document(self.handle.0)
            .and_then(|parsed| parsed.document.attribute(self.handle.0, "href"))
            .unwrap_or_default();
        let base = document_base_url_string(&ctx, self.handle.0);
        Ok(url::Url::parse(&base)
            .ok()
            .and_then(|base| base.join(&raw).ok())
            .map_or(raw, |url| url.to_string()))
    }

    #[qjs(skip)]
    fn set_href(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        // URL reflection stores the given value; resolution happens on get
        // (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#reflecting-content-attributes-in-idl-attributes>).
        self.set_attribute(ctx, WebIdlString("href".into()), value)
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

    #[qjs(skip)]
    fn set_style(&self, ctx: &Ctx<'_>, value: WebIdlString) -> Result<()> {
        self.set_attribute(ctx.clone(), WebIdlString("style".into()), value)
    }

    // https://dom.spec.whatwg.org/#dom-node-lastchild
    #[qjs(skip)]
    fn last_child<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let world = world(ctx)?;
        let id = world
            .borrow()
            .with_document(self.handle.0, |parsed| {
                parsed
                    .document
                    .children(self.handle.0)
                    .and_then(|mut kids| kids.next_back())
            })
            .flatten();
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
            let document = parsed.document.document();
            (document != self.handle.0).then_some(document)
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
            .children(self.handle.0)
            .is_some_and(|mut kids| kids.next().is_some()))
    }

    // https://dom.spec.whatwg.org/#dom-node-nodevalue
    #[qjs(skip)]
    fn node_value<'js>(&self, ctx: &Ctx<'js>) -> Result<Option<rquickjs::String<'js>>> {
        let world = world(ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(None);
        };
        match parsed.document.kind(self.handle.0) {
            Some(
                NodeKind::Text { data }
                | NodeKind::CDataSection { data }
                | NodeKind::ProcessingInstruction { data, .. }
                | NodeKind::Comment { data },
            ) => dom_string(ctx, data).map(Some),
            _ => Ok(None),
        }
    }

    // https://dom.spec.whatwg.org/#dom-node-nodevalue
    #[qjs(skip)]
    fn set_node_value(&self, ctx: &Ctx<'_>, value: Option<rquickjs::String<'_>>) -> Result<()> {
        let value = match value {
            Some(value) => dom::DomString::from_utf16(value.to_utf16()?),
            None => dom::DomString::default(),
        };
        set_character_data(ctx, self.handle.0, value)
    }

    // https://dom.spec.whatwg.org/#dom-node-textcontent
    #[qjs(skip)]
    fn text_content<'js>(&self, ctx: &Ctx<'js>) -> Result<Option<rquickjs::String<'js>>> {
        let world = world(ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(None);
        };
        match parsed.document.kind(self.handle.0) {
            Some(NodeKind::Element { .. } | NodeKind::Fragment) => {
                let text = descendant_text(&parsed.document, self.handle.0);
                dom_string(ctx, &text).map(Some)
            }
            Some(
                NodeKind::Text { data }
                | NodeKind::CDataSection { data }
                | NodeKind::ProcessingInstruction { data, .. }
                | NodeKind::Comment { data },
            ) => dom_string(ctx, data).map(Some),
            _ => Ok(None),
        }
    }

    #[qjs(skip)]
    fn set_text_content(&self, ctx: &Ctx<'_>, value: Option<rquickjs::String<'_>>) -> Result<()> {
        let text = match value {
            Some(value) => dom::DomString::from_utf16(value.to_utf16()?),
            None => dom::DomString::default(),
        };
        // A `CharacterData` node [replaces its data] in place; `Document` and
        // `DocumentType` ignore the setter; every other node replaces its
        // children with one `Text` node
        // (<https://dom.spec.whatwg.org/#dom-node-textcontent>).
        let world_rc = world(ctx)?;
        let character_data = {
            let world = world_rc.borrow();
            let Some(parsed) = world.document(self.handle.0) else {
                return Ok(());
            };
            matches!(
                parsed.document.kind(self.handle.0),
                Some(
                    NodeKind::Text { .. }
                        | NodeKind::CDataSection { .. }
                        | NodeKind::ProcessingInstruction { .. }
                        | NodeKind::Comment { .. }
                )
            )
        };
        if character_data {
            return set_character_data(ctx, self.handle.0, text);
        }
        let world = world_rc.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        if matches!(
            parsed.document.kind(self.handle.0),
            Some(NodeKind::Document | NodeKind::Doctype { .. })
        ) {
            return Ok(());
        }
        let dom = &mut parsed.document;
        let replacement = dom.create_fragment();
        if !text.is_empty() {
            let text_id = dom.create_text(text);
            dom::mutation::append(dom, replacement, text_id)
                .map_err(|err| throw_dom_error(ctx, err))?;
        }
        dom::mutation::replace_all(dom, self.handle.0, replacement)
            .map_err(|err| throw_dom_error(ctx, err))?;
        drop(parsed);
        drop(world);
        schedule_mutation_delivery(ctx)
    }

    // ── ParentNode ───────────────────────────────────────────────────────

    // https://dom.spec.whatwg.org/#dom-parentnode-children
    #[qjs(skip)]
    fn children<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        live_collection(
            ctx,
            self.handle.0,
            CollectionKind::ElementChildren,
            Some("HTMLCollection"),
        )
    }

    // https://dom.spec.whatwg.org/#dom-parentnode-firstelementchild
    #[qjs(skip)]
    fn first_element_child<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let world = world(ctx)?;
        let found = {
            let parsed = world.borrow();
            let Some(parsed) = parsed.document(self.handle.0) else {
                return Ok(Value::new_null(ctx.clone()));
            };
            parsed
                .document
                .children(self.handle.0)
                .and_then(|mut kids| kids.find(|&kid| is_element(&parsed.document, kid)))
        };
        child_value(ctx, found)
    }

    // https://dom.spec.whatwg.org/#dom-parentnode-lastelementchild
    #[qjs(skip)]
    fn last_element_child<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let world = world(ctx)?;
        let found = {
            let parsed = world.borrow();
            let Some(parsed) = parsed.document(self.handle.0) else {
                return Ok(Value::new_null(ctx.clone()));
            };
            parsed
                .document
                .children(self.handle.0)
                .and_then(|kids| kids.rev().find(|&kid| is_element(&parsed.document, kid)))
        };
        child_value(ctx, found)
    }

    // https://dom.spec.whatwg.org/#dom-parentnode-childelementcount
    #[qjs(skip)]
    fn child_element_count(&self, ctx: &Ctx<'_>) -> Result<usize> {
        let world = world(ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(0);
        };
        Ok(parsed.document.children(self.handle.0).map_or(0, |kids| {
            kids.filter(|&kid| is_element(&parsed.document, kid))
                .count()
        }))
    }

    // https://dom.spec.whatwg.org/#dom-parentnode-append
    #[qjs(skip)]
    fn append<'js>(&self, ctx: Ctx<'js>, nodes: Rest<Value<'js>>) -> Result<()> {
        let node = convert_nodes_into_node(&ctx, self.handle.0, nodes)?;
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        dom::mutation::pre_insert(&mut parsed.document, self.handle.0, node, None)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        drop(parsed);
        drop(world);
        schedule_mutation_delivery(&ctx)
    }

    // https://dom.spec.whatwg.org/#dom-parentnode-prepend
    #[qjs(skip)]
    fn prepend<'js>(&self, ctx: Ctx<'js>, nodes: Rest<Value<'js>>) -> Result<()> {
        let node = convert_nodes_into_node(&ctx, self.handle.0, nodes)?;
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        let reference = parsed
            .document
            .children(self.handle.0)
            .and_then(|mut kids| kids.next());
        dom::mutation::pre_insert(&mut parsed.document, self.handle.0, node, reference)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        drop(parsed);
        drop(world);
        schedule_mutation_delivery(&ctx)
    }

    // https://dom.spec.whatwg.org/#dom-parentnode-replacechildren
    #[qjs(skip)]
    fn replace_children<'js>(&self, ctx: Ctx<'js>, nodes: Rest<Value<'js>>) -> Result<()> {
        let node = convert_nodes_into_node(&ctx, self.handle.0, nodes)?;
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        dom::mutation::replace_all(&mut parsed.document, self.handle.0, node)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        drop(parsed);
        drop(world);
        schedule_mutation_delivery(&ctx)
    }

    // https://dom.spec.whatwg.org/#dom-parentnode-queryselector
    #[qjs(skip)]
    fn query_selector<'js>(&self, ctx: Ctx<'js>, selectors: WebIdlString) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let found = {
            let parsed = world.borrow();
            let Some(parsed) = parsed.document(self.handle.0) else {
                return Ok(Value::new_null(ctx));
            };
            dom::selector::select_first(&parsed.document, self.handle.0, &selectors.0)
                .map_err(|err| select_error(&ctx, &err))?
        };
        child_value(&ctx, found)
    }

    // https://dom.spec.whatwg.org/#dom-parentnode-queryselectorall
    #[qjs(skip)]
    fn query_selector_all<'js>(
        &self,
        ctx: Ctx<'js>,
        selectors: WebIdlString,
    ) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let ids = {
            let parsed = world.borrow();
            // A missing document answers with an empty list, not null
            // (<https://dom.spec.whatwg.org/#dom-parentnode-queryselectorall>).
            parsed
                .document(self.handle.0)
                .map(|parsed| {
                    dom::selector::select_all(&parsed.document, self.handle.0, &selectors.0)
                        .map_err(|err| select_error(&ctx, &err))
                })
                .transpose()?
                .unwrap_or_default()
        };
        let handles = ids.into_iter().map(Handle).collect();
        live_collection(&ctx, self.handle.0, CollectionKind::Static(handles), None)
    }

    // https://dom.spec.whatwg.org/#dom-element-matches
    #[qjs(skip)]
    fn matches(&self, ctx: Ctx<'_>, selectors: WebIdlString) -> Result<bool> {
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(false);
        };
        dom::selector::matches(&parsed.document, self.handle.0, &selectors.0)
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
            let mut candidate = None;
            let mut cursor = Some(self.handle.0);
            while let Some(id) = cursor {
                if is_element(&parsed.document, id)
                    && dom::selector::matches(&parsed.document, id, &selectors.0)
                        .map_err(|err| select_error(&ctx, &err))?
                {
                    candidate = Some(id);
                    break;
                }
                cursor = parsed.document.parent(id);
            }
            candidate
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
        let title =
            dom::selector::select_first(&parsed.document, parsed.document.document(), "title")
                .ok()
                .flatten();
        match title {
            Some(title) => dom_string(ctx, &descendant_text(&parsed.document, title)),
            None => rquickjs::String::from_str(ctx.clone(), ""),
        }
    }

    #[qjs(skip)]
    fn set_title(&self, ctx: &Ctx<'_>, value: WebIdlCodeUnits) -> Result<()> {
        let world = world(ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        let dom = &mut parsed.document;
        let title = dom::selector::select_first(dom, dom.document(), "title")
            .ok()
            .flatten()
            .unwrap_or_else(|| {
                let name = QualName::new(None, html_namespace(), LocalName::from("title"));
                let title = dom.create_element(name, Vec::new());
                let target = dom::selector::select_first(dom, dom.document(), "head")
                    .ok()
                    .flatten()
                    .or_else(|| {
                        dom::selector::select_first(dom, dom.document(), "html")
                            .ok()
                            .flatten()
                    });
                if let Some(target) = target {
                    let _ = dom::mutation::append(dom, target, title);
                }
                title
            });
        let kids: Vec<NodeId> = dom
            .children(title)
            .map(Iterator::collect)
            .unwrap_or_default();
        for kid in kids {
            dom::mutation::detach(dom, kid).map_err(|err| throw_dom_error(ctx, err))?;
        }
        if !value.0.is_empty() {
            let text = dom.create_text(value.0);
            dom::mutation::append(dom, title, text).map_err(|err| throw_dom_error(ctx, err))?;
        }
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

    // ── ChildNode / NonDocumentTypeChildNode ─────────────────────────────

    // https://dom.spec.whatwg.org/#dom-childnode-before
    #[qjs(skip)]
    fn before<'js>(&self, ctx: Ctx<'js>, nodes: Rest<Value<'js>>) -> Result<()> {
        let node = convert_nodes_into_node(&ctx, self.handle.0, nodes)?;
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        let Some(parent) = parsed.document.parent(self.handle.0) else {
            return Ok(());
        };
        let previous = parsed.document.sibling(self.handle.0, false);
        let reference = match previous {
            Some(previous) => parsed.document.sibling(previous, true),
            None => parsed
                .document
                .children(parent)
                .and_then(|mut kids| kids.next()),
        };
        dom::mutation::pre_insert(&mut parsed.document, parent, node, reference)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        drop(parsed);
        drop(world);
        schedule_mutation_delivery(&ctx)
    }

    // https://dom.spec.whatwg.org/#dom-childnode-after
    #[qjs(skip)]
    fn after<'js>(&self, ctx: Ctx<'js>, nodes: Rest<Value<'js>>) -> Result<()> {
        let node = convert_nodes_into_node(&ctx, self.handle.0, nodes)?;
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        let Some(parent) = parsed.document.parent(self.handle.0) else {
            return Ok(());
        };
        let reference = parsed.document.sibling(self.handle.0, true);
        dom::mutation::pre_insert(&mut parsed.document, parent, node, reference)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        drop(parsed);
        drop(world);
        schedule_mutation_delivery(&ctx)
    }

    // https://dom.spec.whatwg.org/#dom-childnode-replacewith
    #[qjs(skip)]
    fn replace_with<'js>(&self, ctx: Ctx<'js>, nodes: Rest<Value<'js>>) -> Result<()> {
        let node = convert_nodes_into_node(&ctx, self.handle.0, nodes)?;
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        let Some(parent) = parsed.document.parent(self.handle.0) else {
            return Ok(());
        };
        dom::mutation::replace_child(&mut parsed.document, parent, node, self.handle.0)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        drop(parsed);
        drop(world);
        schedule_mutation_delivery(&ctx)
    }

    // https://dom.spec.whatwg.org/#dom-nondocumenttypechildnode-previouselementsibling
    #[qjs(skip)]
    fn previous_element_sibling<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        element_sibling_value(ctx, self.handle.0, false)
    }

    // https://dom.spec.whatwg.org/#dom-nondocumenttypechildnode-nextelementsibling
    #[qjs(skip)]
    fn next_element_sibling<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        element_sibling_value(ctx, self.handle.0, true)
    }

    // ── Element identity and attributes ─────────────────────────────────

    // https://dom.spec.whatwg.org/#dom-element-tagname
    #[qjs(skip)]
    fn tag_name(&self, ctx: &Ctx<'_>) -> Result<String> {
        let uppercase = document_is_html_content(ctx, self.handle.0);
        with_node_kind(ctx, self.handle.0, |kind| match kind {
            Some(NodeKind::Element { name, .. }) => element_node_name(name, uppercase),
            _ => String::new(),
        })
    }

    // https://html.spec.whatwg.org/multipage/forms.html#dom-form-target
    #[qjs(skip)]
    fn target(&self, ctx: Ctx<'_>) -> Result<String> {
        attribute_value(&ctx, self.handle.0, "target")
    }

    // https://dom.spec.whatwg.org/#dom-element-localname
    #[qjs(skip)]
    fn local_name(&self, ctx: &Ctx<'_>) -> Result<String> {
        with_node_kind(ctx, self.handle.0, |kind| match kind {
            Some(NodeKind::Element { name, .. }) => name.local.to_string(),
            _ => String::new(),
        })
    }

    // https://dom.spec.whatwg.org/#dom-element-prefix
    #[qjs(skip)]
    fn prefix<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let prefix = with_node_kind(ctx, self.handle.0, |kind| match kind {
            Some(NodeKind::Element { name, .. }) => name
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
        let namespace = with_node_kind(ctx, self.handle.0, |kind| match kind {
            Some(NodeKind::Element { name, .. }) => {
                (!name.ns.is_empty()).then(|| name.ns.to_string())
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
        let world = world(ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(ctx, "no document"));
        };
        dom::mutation::set_attribute(&mut parsed.document, self.handle.0, "class", value.0)
            .map_err(|err| throw_dom_error(ctx, err))?;
        drop(parsed);
        drop(world);
        schedule_mutation_delivery(ctx)
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
        Ok(parsed.document.has_attribute(self.handle.0, &local))
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
            parsed
                .document
                .attribute_ns(self.handle.0, &namespace, &local.0)
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
        Ok(parsed
            .document
            .attribute_ns(self.handle.0, &namespace, &local.0)
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
        let prefix = name
            .prefix
            .as_ref()
            .filter(|prefix| !prefix.is_empty())
            .map(ToString::to_string);
        let local = name.local.to_string();
        {
            let world = world(&ctx)?;
            let world = world.borrow();
            let Some(mut parsed) = world.document_mut(self.handle.0) else {
                return Err(Exception::throw_type(&ctx, "no document"));
            };
            dom::mutation::set_attribute_by_ns(
                &mut parsed.document,
                self.handle.0,
                &namespace,
                prefix.as_deref(),
                &local,
                value.0.clone(),
            )
            .map_err(|err| throw_dom_error(&ctx, err))?;
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
            .map(|parsed| parsed.document.attribute_names(self.handle.0))
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
            refresh_named_node_map(ctx, self.handle.0, &value)?;
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
        refresh_named_node_map(ctx, self.handle.0, &value)?;
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
            .attributes(self.handle.0)
            .is_some_and(|list| !list.is_empty()))
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
        let (should_exist, changed) = {
            let world = world(&ctx)?;
            let world = world.borrow();
            let Some(mut parsed) = world.document_mut(self.handle.0) else {
                return Err(Exception::throw_type(&ctx, "no document"));
            };
            let exists = parsed.document.has_attribute(self.handle.0, &local);
            let should_exist = force.unwrap_or(!exists);
            if should_exist && !exists {
                dom::mutation::set_attribute(
                    &mut parsed.document,
                    self.handle.0,
                    &local,
                    String::new(),
                )
                .map_err(|err| throw_dom_error(&ctx, err))?;
            }
            (should_exist, should_exist != exists)
        };
        if changed {
            if should_exist {
                touch_attr(&ctx, self.handle.0, "", &local, "")?;
            } else {
                remove_attribute_sync(&ctx, self.handle.0, "", &local, false)?;
            }
            schedule_mutation_delivery(&ctx)?;
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
        let dom = &mut parsed.document;
        let mut containers: Vec<NodeId> = vec![self.handle.0];
        let mut stack = vec![self.handle.0];
        while let Some(id) = stack.pop() {
            if let Some(kids) = dom.children(id) {
                for kid in kids {
                    if matches!(dom.kind(kid), Some(NodeKind::Element { .. })) {
                        containers.push(kid);
                        stack.push(kid);
                    }
                }
            }
        }
        for container in containers {
            let kids: Vec<NodeId> = dom
                .children(container)
                .map(Iterator::collect)
                .unwrap_or_default();
            // In tree order: drop empty Text nodes, merge contiguous runs
            // into the first non-empty one
            // (<https://dom.spec.whatwg.org/#dom-node-normalize>).
            let mut merged: Option<NodeId> = None;
            for kid in kids {
                match dom.kind(kid) {
                    Some(NodeKind::Text { data }) if data.is_empty() => {
                        dom::mutation::detach(dom, kid)
                            .map_err(|err| throw_dom_error(&ctx, err))?;
                    }
                    Some(NodeKind::Text { data }) => {
                        if let Some(previous) = merged {
                            let mut joined = match dom.kind(previous) {
                                Some(NodeKind::Text { data }) => data.clone(),
                                _ => dom::DomString::default(),
                            };
                            joined.push_dom(data);
                            dom::mutation::set_text(dom, previous, joined)
                                .map_err(|err| throw_dom_error(&ctx, err))?;
                            dom::mutation::detach(dom, kid)
                                .map_err(|err| throw_dom_error(&ctx, err))?;
                        } else {
                            merged = Some(kid);
                        }
                    }
                    _ => merged = None,
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
                .parent(self.handle.0)
                .filter(|&parent| is_element(&parsed.document, parent))
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
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(false);
        };
        Ok(nodes_equal(&parsed.document, self.handle.0, other))
    }

    // https://dom.spec.whatwg.org/#dom-node-contains
    #[qjs(skip)]
    fn contains(&self, ctx: Ctx<'_>, other: Option<NodeReference>) -> Result<bool> {
        let Some(NodeReference::Tree(other)) = other else {
            return Ok(false);
        };
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(false);
        };
        let mut cursor = Some(other);
        while let Some(id) = cursor {
            if id == self.handle.0 {
                return Ok(true);
            }
            cursor = parsed.document.parent(id);
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
        let world = world(&ctx)?;
        let mut root = self.handle.0;
        {
            let parsed = world.borrow();
            let Some(parsed) = parsed.document(self.handle.0) else {
                return Ok(Value::new_null(ctx));
            };
            loop {
                while let Some(parent) = parsed.document.parent(root) {
                    root = parent;
                }
                if options.composed
                    && let Some(host) = dom::shadow::shadow_host(&parsed.document, root)
                {
                    root = host;
                } else {
                    break;
                }
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
        Ok(dom::lifecycle::is_connected(
            &parsed.document,
            self.handle.0,
        ))
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
            self.handle.0 == parsed.document.document()
        };
        if is_document {
            return clone_document(&ctx, self.handle.0, deep);
        }
        let world = world_rc.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        let clone = dom::mutation::clone_node(&mut parsed.document, self.handle.0, deep)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        // Run the form-control cloning steps for the source and clone in
        // parallel tree order, so a deep clone carries each control's value and
        // checkedness
        // (<https://html.spec.whatwg.org/multipage/input.html#the-input-element:cloning-steps>).
        let pairs: Vec<(dom::NodeId, dom::NodeId)> = {
            let dom = &parsed.document;
            let mut from_stack = vec![self.handle.0];
            let mut to_stack = vec![clone];
            let mut pairs = Vec::new();
            while let (Some(from), Some(to)) = (from_stack.pop(), to_stack.pop()) {
                pairs.push((from, to));
                let from_children: Vec<_> = dom
                    .children(from)
                    .map(Iterator::collect)
                    .unwrap_or_default();
                let to_children: Vec<_> =
                    dom.children(to).map(Iterator::collect).unwrap_or_default();
                for child in from_children.into_iter().rev() {
                    from_stack.push(child);
                }
                for child in to_children.into_iter().rev() {
                    to_stack.push(child);
                }
            }
            pairs
        };
        for (from, to) in pairs {
            dom::form::clone_form_state(&mut parsed.document, from, to);
        }
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
        let node = adopt_across_documents(&ctx, self.handle.0, node)?;
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        let dom = &mut parsed.document;
        dom::mutation::pre_insert(dom, self.handle.0, node, child)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        drop(parsed);
        drop(world);
        schedule_mutation_delivery(&ctx)?;
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
        if parsed.document.parent(child) != Some(self.handle.0) {
            return Err(throw_dom(
                &ctx,
                "NotFoundError",
                "child is not a child of this node",
            ));
        }
        dom::mutation::detach(&mut parsed.document, child)
            .map_err(|err| throw_dom_error(&ctx, err))?;
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
        let node = adopt_across_documents(&ctx, self.handle.0, node)?;
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        dom::mutation::replace_child(&mut parsed.document, self.handle.0, node, child)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        drop(parsed);
        drop(world);
        fixup_focus_after_removal(&ctx, child)?;
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
        match locate_namespace(&parsed.document, self.handle.0, prefix.as_deref()) {
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
        match locate_prefix(&parsed.document, self.handle.0, &namespace) {
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
        let found = locate_namespace(&parsed.document, self.handle.0, None);
        Ok(found.as_deref() == namespace.as_deref())
    }

    // https://dom.spec.whatwg.org/#dom-childnode-remove
    #[qjs(skip)]
    fn remove(&self, ctx: Ctx<'_>) -> Result<()> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        dom::mutation::detach(&mut parsed.document, self.handle.0)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        drop(parsed);
        drop(world);
        fixup_focus_after_removal(&ctx, self.handle.0)?;
        schedule_mutation_delivery(&ctx)
    }
}

impl<'js> processing_instruction_generated::ProcessingInstruction<'js> for JsNode {
    // https://dom.spec.whatwg.org/#dom-processinginstruction-target
    fn get_target(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let target = with_node_kind(ctx, self.handle.0, |kind| match kind {
            Some(NodeKind::ProcessingInstruction { target, .. }) => target.clone(),
            _ => String::new(),
        })?;
        rquickjs::String::from_str(ctx.clone(), &target)
    }
}

impl<'js> document_type_generated::DocumentType<'js> for JsNode {
    // https://dom.spec.whatwg.org/#dom-documenttype-name
    fn get_name(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let name = with_node_kind(ctx, self.handle.0, |kind| match kind {
            Some(NodeKind::Doctype { name, .. }) => name.clone(),
            _ => String::new(),
        })?;
        rquickjs::String::from_str(ctx.clone(), &name)
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
        let data = dom::DomString::from_utf16(value.to_utf16()?);
        set_character_data(ctx, self.handle.0, data)
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
        current.push_dom(&dom::DomString::from_utf16(data.to_utf16()?));
        set_character_data(&ctx, self.handle.0, current)
    }

    // https://dom.spec.whatwg.org/#dom-characterdata-insertdata
    fn insert_data(&self, ctx: Ctx<'js>, offset: u32, data: rquickjs::String<'js>) -> Result<()> {
        let mut units = character_data(&ctx, self.handle.0)?.units().into_owned();
        let offset = character_data_offset(&ctx, offset, units.len())?;
        let insert = data.to_utf16()?;
        units.splice(offset..offset, insert.iter().copied());
        set_character_data(&ctx, self.handle.0, dom::DomString::from_utf16(units))
    }

    // https://dom.spec.whatwg.org/#dom-characterdata-deletedata
    fn delete_data(&self, ctx: Ctx<'js>, offset: u32, count: u32) -> Result<()> {
        let mut units = character_data(&ctx, self.handle.0)?.units().into_owned();
        let offset = character_data_offset(&ctx, offset, units.len())?;
        let end = offset
            .saturating_add(usize::try_from(count).unwrap_or(usize::MAX))
            .min(units.len());
        units.drain(offset..end);
        set_character_data(&ctx, self.handle.0, dom::DomString::from_utf16(units))
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
        set_character_data(&ctx, self.handle.0, dom::DomString::from_utf16(units))
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
