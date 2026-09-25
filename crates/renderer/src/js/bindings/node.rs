//! The single `Node` wrapper class and its members.

use super::{
    CollectionKind, FromJs, ImportSnapshot, JsAttr, JsImplementation, JsNamedNodeMap, JsTokenList,
    LegacyNullString, NodeContext, OptString, Trace, WebIdlString, WebIdlUnsignedLong,
    adopt_across_documents, after_attribute_change, ancestor_chain, attached_attr_id, attr_owner,
    attr_state, attr_wrapper, attribute_local_name, attribute_value, blur_node, character_data,
    character_data_offset, child_value, clone_document, convert_nodes_into_node,
    create_element_named, create_html_element, create_kind, deref_weak, descendant_text,
    detach_attr, doctype_fields, document_base_url_string, document_is_html,
    document_is_html_content, document_url_string, element_at_point, element_box, element_click,
    element_node_name, element_sibling_value, elements_by_tag, find_element_by_id,
    fixup_focus_after_removal, focus_node, host_node_id, import_snapshot, is_element, is_focusable,
    is_html_element, is_main_document, is_template_element, live_collection, locate_namespace,
    locate_prefix, main_document, make_weak, materialize_children, materialize_import,
    new_detached_attr, nodes_equal, optional_node, qualified_name, rect_object,
    refresh_named_node_map, remove_attribute_sync, required_node, root_of,
    schedule_mutation_delivery, select_error, set_attribute_node, set_character_data,
    sibling_value, string_value, throw_dom, throw_dom_error, touch_attr, tree_order,
    valid_attribute_local_name, validate_and_extract, webidl_to_string, with_node_kind, world,
    world_for_node, wrap_new_document, wrap_node,
};
use rquickjs::function::{Opt, Rest};

use dom::{LocalName, NodeId, NodeKind, QualName, html_namespace, svg_namespace};

use rquickjs::{Array, Class, Ctx, Exception, Function, Object, Persistent, Result, Value};

use crate::js::events::{self, JsEvent};

use crate::js::world::{DocumentStreamCommand, EventTargetKey, Handle, Wrapper};

use crate::ReadyState;

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
            let data = constructor_string(&ctx, first)?;
            let document = main_document(&ctx)?;
            create_kind(&ctx, document, |dom| dom.create_text(data))
        }
        "Comment" => {
            let data = constructor_string(&ctx, first)?;
            let document = main_document(&ctx)?;
            create_kind(&ctx, document, |dom| dom.create_comment(data))
        }
        "DocumentFragment" => {
            let document = main_document(&ctx)?;
            create_kind(&ctx, document, dom::Dom::create_fragment)
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
    let globals = ctx.globals();
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

/// Constructor `DOMString` with the IDL default: missing and `undefined` are
/// the empty string, `null` is "null" (<https://webidl.spec.whatwg.org/#es-DOMString>).
fn constructor_string<'js>(ctx: &Ctx<'js>, value: Option<Value<'js>>) -> Result<String> {
    match value {
        None => Ok(String::new()),
        Some(value) if value.is_undefined() => Ok(String::new()),
        Some(value) => webidl_to_string(ctx, value),
    }
}

branded_node!(JsNode, "Node");

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
        .dom
        .children(parsed_fragment.dom.document())
        .and_then(|mut children| {
            children.find(|&id| {
                matches!(
                    parsed_fragment.dom.kind(id),
                    Some(NodeKind::Element { name, .. })
                        if name.ns == html_namespace() && name.local.as_ref() == "html"
                )
            })
        })
        .ok_or_else(|| Exception::throw_internal(ctx, "fragment parser omitted its root"))?;
    Ok(parsed_fragment
        .dom
        .children(fragment_root)
        .map(|children| {
            children
                .filter_map(|child| import_snapshot(&parsed_fragment.dom, child, true))
                .collect()
        })
        .unwrap_or_default())
}

/// The first node matching `selector` under `parsed`'s document root.
fn document_first(parsed: &crate::Parsed, selector: &str) -> Option<NodeId> {
    parsed
        .dom
        .select_first(parsed.dom.document(), selector)
        .ok()
        .flatten()
}

/// The first child of `parsed`'s document root satisfying `want`.
fn document_first_child(parsed: &crate::Parsed, want: fn(&NodeKind) -> bool) -> Option<NodeId> {
    parsed
        .dom
        .children(parsed.dom.document())
        .into_iter()
        .flatten()
        .find(|&id| parsed.dom.kind(id).is_some_and(want))
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
    #[qjs(get, rename = "nodeType")]
    fn node_type(&self, ctx: Ctx<'_>) -> Result<i32> {
        with_node_kind(&ctx, self.handle.0, |kind| match kind {
            Some(NodeKind::Element { .. }) => Ok(1),
            Some(NodeKind::Text { .. }) => Ok(3),
            Some(NodeKind::CDataSection { .. }) => Ok(4),
            Some(NodeKind::ProcessingInstruction { .. }) => Ok(7),
            Some(NodeKind::Comment { .. }) => Ok(8),
            Some(NodeKind::Document) => Ok(9),
            Some(NodeKind::Doctype { .. }) => Ok(10),
            Some(NodeKind::Fragment) => Ok(11),
            None => Err(Exception::throw_type(&ctx, "stale node")),
        })?
    }

    // https://dom.spec.whatwg.org/#dom-node-nodename
    // https://dom.spec.whatwg.org/#concept-element-html-uppercased-qualified-name
    #[qjs(get, rename = "nodeName")]
    fn node_name(&self, ctx: Ctx<'_>) -> Result<String> {
        let uppercase = document_is_html_content(&ctx, self.handle.0);
        with_node_kind(&ctx, self.handle.0, |kind| match kind {
            Some(NodeKind::Element { name, .. }) => Ok(element_node_name(name, uppercase)),
            Some(NodeKind::Text { .. }) => Ok("#text".into()),
            Some(NodeKind::CDataSection { .. }) => Ok("#cdata-section".into()),
            Some(NodeKind::ProcessingInstruction { target, .. }) => Ok(target.clone()),
            Some(NodeKind::Comment { .. }) => Ok("#comment".into()),
            Some(NodeKind::Document) => Ok("#document".into()),
            Some(NodeKind::Doctype { name, .. }) => Ok(name.clone()),
            Some(NodeKind::Fragment) => Ok("#document-fragment".into()),
            None => Err(Exception::throw_type(&ctx, "stale node")),
        })?
    }

    #[qjs(get, rename = "firstChild")]
    fn first_child<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let id = {
            let parsed = world.borrow();
            let Some(parsed) = parsed.document(self.handle.0) else {
                return Err(Exception::throw_type(&ctx, "no document"));
            };
            parsed
                .dom
                .children(self.handle.0)
                .and_then(|mut kids| kids.next())
        };
        child_value(&ctx, id)
    }

    #[qjs(get, rename = "parentNode")]
    fn parent_node<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let id = world
            .borrow()
            .document(self.handle.0)
            .and_then(|parsed| parsed.dom.parent(self.handle.0));
        match id {
            Some(parent) => wrap_node(&ctx, parent),
            None => Ok(Value::new_null(ctx)),
        }
    }

    #[qjs(get, rename = "childNodes")]
    fn child_nodes<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        live_collection(&ctx, self.handle.0, CollectionKind::Children, None)
    }

    #[qjs(rename = "appendChild")]
    fn append_child<'js>(&self, ctx: Ctx<'js>, child: Value<'js>) -> Result<Value<'js>> {
        let Some(kid) = host_node_id(&ctx, &child) else {
            return Err(Exception::throw_type(&ctx, "not a node"));
        };
        {
            let world = world(&ctx)?;
            let parsed = world.borrow();
            let Some(parsed) = parsed.document(self.handle.0) else {
                return Err(Exception::throw_type(&ctx, "no document"));
            };
            parsed
                .dom
                .validate_pre_insert(self.handle.0, kid, None)
                .map_err(|err| throw_dom_error(&ctx, err))?;
        }
        let kid = adopt_across_documents(&ctx, self.handle.0, kid)?;
        let parent = self.handle.0;
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        parsed
            .dom
            .pre_insert(parent, kid, None)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        drop(parsed);
        drop(world);
        schedule_mutation_delivery(&ctx)?;
        wrap_node(&ctx, kid)
    }

    #[qjs(rename = "addEventListener")]
    fn add_event_listener<'js>(
        &self,
        ctx: Ctx<'js>,
        typ: Value<'js>,
        callback: Value<'js>,
        options: Opt<Value<'js>>,
    ) -> Result<()> {
        events::add_listener(
            &ctx,
            EventTargetKey::Node(self.handle.0),
            typ,
            callback,
            options.0,
        )
    }

    #[qjs(rename = "removeEventListener")]
    fn remove_event_listener<'js>(
        &self,
        ctx: Ctx<'js>,
        typ: Value<'js>,
        callback: Value<'js>,
        options: Opt<Value<'js>>,
    ) -> Result<()> {
        events::remove_listener(
            &ctx,
            EventTargetKey::Node(self.handle.0),
            typ,
            callback,
            options.0,
        )
    }

    #[qjs(rename = "dispatchEvent")]
    fn dispatch_event<'js>(&self, ctx: Ctx<'js>, event: Class<'js, JsEvent>) -> Result<bool> {
        events::dispatch_event(&ctx, EventTargetKey::Node(self.handle.0), &event)
    }

    // https://html.spec.whatwg.org/multipage/interaction.html#dom-click
    #[qjs(rename = "click")]
    fn click(&self, ctx: Ctx<'_>) -> Result<()> {
        element_click(&ctx, self.handle.0)
    }

    // https://html.spec.whatwg.org/multipage/interaction.html#dom-focus
    #[qjs(rename = "focus")]
    fn focus(&self, ctx: Ctx<'_>) -> Result<()> {
        if is_focusable(&ctx, self.handle.0)? {
            focus_node(&ctx, self.handle.0)?;
        }
        Ok(())
    }

    // https://html.spec.whatwg.org/multipage/interaction.html#dom-blur
    #[qjs(rename = "blur")]
    fn blur(&self, ctx: Ctx<'_>) -> Result<()> {
        blur_node(&ctx, self.handle.0)
    }

    // https://dom.spec.whatwg.org/#dom-document-activeelement
    #[qjs(get, rename = "activeElement")]
    fn active_element<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let document = self.handle.0;
        let world = world(&ctx)?;
        let active = world.borrow().active_element(document.document_id());
        if let Some(node) = active
            && world
                .borrow()
                .document(node)
                .is_some_and(|parsed| parsed.dom.is_connected(node))
        {
            return wrap_node(&ctx, node);
        }
        let fallback = {
            let world = world.borrow();
            let Some(parsed) = world.document(document) else {
                return Ok(Value::new_null(ctx));
            };
            document_first(&parsed, "body").or_else(|| {
                document_first_child(&parsed, |kind| matches!(kind, NodeKind::Element { .. }))
            })
        };
        match fallback {
            Some(node) => wrap_node(&ctx, node),
            None => Ok(Value::new_null(ctx)),
        }
    }

    // The element's border box from the render pipeline's layout; zero when
    // the element generates no box (for example `display: none`).
    #[qjs(rename = "getBoundingClientRect")]
    fn get_bounding_client_rect<'js>(&self, ctx: Ctx<'js>) -> Result<Object<'js>> {
        match element_box(&ctx, self.handle.0)? {
            Some((left, top, width, height)) => rect_object(&ctx, left, top, width, height),
            None => rect_object(&ctx, 0.0, 0.0, 0.0, 0.0),
        }
    }

    #[qjs(rename = "getClientRects")]
    fn get_client_rects<'js>(&self, ctx: Ctx<'js>) -> Result<Array<'js>> {
        let array = Array::new(ctx.clone())?;
        if let Some((left, top, width, height)) = element_box(&ctx, self.handle.0)? {
            array.set(0, rect_object(&ctx, left, top, width, height)?)?;
        }
        Ok(array)
    }

    /// No layout means there is nothing to scroll.
    #[qjs(rename = "scrollIntoView")]
    fn scroll_into_view(&self) {}

    // https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-document-defaultview
    #[qjs(get, rename = "defaultView")]
    fn default_view<'js>(&self, ctx: Ctx<'js>) -> Value<'js> {
        // A document with no browsing context, such as one from DOMParser or
        // `createDocument`, has no view.
        let has_view =
            world(&ctx).is_ok_and(|world| world.borrow().is_main_document(self.handle.0));
        if has_view {
            ctx.globals().into_value()
        } else {
            Value::new_null(ctx)
        }
    }

    // https://html.spec.whatwg.org/multipage/interaction.html#dom-document-hasfocus
    #[qjs(rename = "hasFocus")]
    fn has_focus(&self, ctx: Ctx<'_>) -> bool {
        is_main_document(&ctx, self.handle.0)
    }

    // https://drafts.csswg.org/cssom-view/#dom-document-elementfrompoint
    #[qjs(rename = "elementFromPoint")]
    fn element_from_point<'js>(&self, ctx: Ctx<'js>, x: f64, y: f64) -> Result<Value<'js>> {
        let Some(node) = element_at_point(&ctx, self.handle.0, x, y)? else {
            return Ok(Value::new_null(ctx));
        };
        wrap_node(&ctx, node)
    }

    // The no-layout hit test: the deepest element whose virtual box contains
    // the point (see `element_at_point`).
    #[qjs(rename = "elementsFromPoint")]
    fn elements_from_point<'js>(&self, ctx: Ctx<'js>, x: f64, y: f64) -> Result<Array<'js>> {
        let array = Array::new(ctx.clone())?;
        let Some(node) = element_at_point(&ctx, self.handle.0, x, y)? else {
            return Ok(array);
        };
        // The hit-test stack: the element and its ancestors, topmost first.
        let world = world_for_node(&ctx, node)?;
        let mut index = 0;
        let mut cursor = Some(node);
        while let Some(current) = cursor {
            array.set(index, wrap_node(&ctx, current)?)?;
            index += 1;
            cursor = world.borrow().node_parent(current);
        }
        Ok(array)
    }

    // https://dom.spec.whatwg.org/#dom-document-createevent
    #[qjs(rename = "createEvent")]
    fn create_event<'js>(&self, ctx: Ctx<'js>, interface: Value<'js>) -> Result<Value<'js>> {
        let interface = webidl_to_string(&ctx, interface)?;
        events::create_event(&ctx, &interface)
    }

    #[qjs(rename = "createElement")]
    fn create_element<'js>(&self, ctx: Ctx<'js>, tag: WebIdlString) -> Result<Value<'js>> {
        create_html_element(&ctx, self.handle.0, &tag.0)
    }

    // https://dom.spec.whatwg.org/#dom-document-createelementns
    // https://dom.spec.whatwg.org/#internal-createelementns-steps
    #[qjs(rename = "createElementNS")]
    fn create_element_ns<'js>(
        &self,
        ctx: Ctx<'js>,
        ns: OptString,
        tag: WebIdlString,
    ) -> Result<Value<'js>> {
        let name = validate_and_extract(&ctx, ns.0.as_deref(), &tag.0, NodeContext::Element)?;
        create_element_named(&ctx, self.handle.0, name)
    }

    #[qjs(rename = "createTextNode")]
    fn create_text_node<'js>(&self, ctx: Ctx<'js>, data: WebIdlString) -> Result<Value<'js>> {
        create_kind(&ctx, self.handle.0, |dom| dom.create_text(data.0))
    }

    // https://dom.spec.whatwg.org/#dom-document-createcomment
    #[qjs(rename = "createComment")]
    fn create_comment<'js>(&self, ctx: Ctx<'js>, data: WebIdlString) -> Result<Value<'js>> {
        create_kind(&ctx, self.handle.0, |dom| dom.create_comment(data.0))
    }

    // https://dom.spec.whatwg.org/#dom-document-createprocessinginstruction
    #[qjs(rename = "createProcessingInstruction")]
    fn create_processing_instruction<'js>(
        &self,
        ctx: Ctx<'js>,
        target: WebIdlString,
        data: WebIdlString,
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
        if data.0.contains("?>") {
            return Err(throw_dom(&ctx, "InvalidCharacterError", "data contains ?>"));
        }
        create_kind(&ctx, self.handle.0, |dom| {
            dom.create_processing_instruction(target.0, data.0)
        })
    }

    // https://dom.spec.whatwg.org/#dom-document-createcdatasection
    #[qjs(rename = "createCDATASection")]
    fn create_cdata_section<'js>(&self, ctx: Ctx<'js>, data: WebIdlString) -> Result<Value<'js>> {
        // CDATA sections cannot exist in HTML documents
        // (<https://dom.spec.whatwg.org/#dom-document-createcdatasection>).
        if document_is_html(&ctx, self.handle.0) {
            return Err(throw_dom(
                &ctx,
                "NotSupportedError",
                "CDATA sections are not supported in HTML documents",
            ));
        }
        if data.0.contains("]]>") {
            return Err(throw_dom(
                &ctx,
                "InvalidCharacterError",
                "data contains ]]>",
            ));
        }
        create_kind(&ctx, self.handle.0, |dom| dom.create_cdata_section(data.0))
    }

    // https://dom.spec.whatwg.org/#dom-document-createattribute
    #[qjs(rename = "createAttribute")]
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
        attr_wrapper(&ctx, self.handle.0, id)
    }

    // https://dom.spec.whatwg.org/#dom-document-createattributens
    #[qjs(rename = "createAttributeNS")]
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
        attr_wrapper(&ctx, self.handle.0, id)
    }

    // https://dom.spec.whatwg.org/#dom-document-createdocumentfragment
    #[qjs(rename = "createDocumentFragment")]
    fn create_document_fragment<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        create_kind(&ctx, self.handle.0, dom::Dom::create_fragment)
    }

    // https://dom.spec.whatwg.org/#dom-document-importnode
    #[qjs(rename = "importNode")]
    fn import_node<'js>(
        &self,
        ctx: Ctx<'js>,
        node: Value<'js>,
        deep: Opt<bool>,
    ) -> Result<Value<'js>> {
        let source_id = required_node(&ctx, &node)?;
        let world_rc = world_for_node(&ctx, self.handle.0)?;
        let tree = {
            let world = world_rc.borrow();
            let Some(source) = world.document(source_id) else {
                return Err(Exception::throw_type(&ctx, "stale node"));
            };
            if source.dom.document() == source_id {
                return Err(throw_dom(
                    &ctx,
                    "NotSupportedError",
                    "cannot import a document",
                ));
            }
            import_snapshot(&source.dom, source_id, deep.0.unwrap_or(false))
        };
        let Some(tree) = tree else {
            return Err(Exception::throw_type(&ctx, "stale node"));
        };
        let world = world_rc.borrow();
        let Some(mut target) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        let id =
            materialize_import(&mut target.dom, &tree).map_err(|err| throw_dom_error(&ctx, err))?;
        drop(target);
        drop(world);
        wrap_node(&ctx, id)
    }

    // https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-document-open
    #[qjs(rename = "open")]
    fn open_document<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        world_for_node(&ctx, self.handle.0)?
            .borrow_mut()
            .queue_document_stream(DocumentStreamCommand::Open)
            .map_err(|()| Exception::throw_range(&ctx, "document stream budget exceeded"))?;
        wrap_node(&ctx, self.handle.0)
    }

    // https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#document-write-steps
    #[qjs(rename = "write")]
    fn write(&self, ctx: Ctx<'_>, text: Rest<WebIdlString>) -> Result<()> {
        // Every argument stringifies and concatenates in order
        // (<https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-document-write>).
        let html: String = text.0.iter().map(|part| part.0.as_str()).collect();
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
    #[qjs(rename = "close")]
    fn close_document(&self, ctx: Ctx<'_>) -> Result<()> {
        world_for_node(&ctx, self.handle.0)?
            .borrow_mut()
            .queue_document_stream(DocumentStreamCommand::Close)
            .map_err(|()| Exception::throw_range(&ctx, "document stream budget exceeded"))
    }

    #[qjs(rename = "getElementById")]
    fn get_element_by_id<'js>(&self, ctx: Ctx<'js>, id: WebIdlString) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let found = {
            let parsed = world.borrow();
            let Some(parsed) = parsed.document(self.handle.0) else {
                return Ok(Value::new_null(ctx));
            };
            find_element_by_id(&parsed.dom, self.handle.0, &id.0)
        };
        match found {
            Some(node) => wrap_node(&ctx, node),
            None => Ok(Value::new_null(ctx)),
        }
    }

    // https://dom.spec.whatwg.org/#dom-document-implementation
    #[qjs(get)]
    fn implementation<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world_rc = world(&ctx)?;
        if let Some(saved) = world_rc.borrow().implementation(self.handle.0)
            && let Some(value) = deref_weak(&ctx, saved)?
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
        let weak = make_weak(&ctx, value.clone())?;
        world_rc
            .borrow_mut()
            .intern_implementation(self.handle.0, Persistent::save(&ctx, weak));
        Ok(value)
    }

    #[qjs(rename = "getElementsByTagName")]
    fn get_elements_by_tag_name<'js>(
        &self,
        ctx: Ctx<'js>,
        name: WebIdlString,
    ) -> Result<Value<'js>> {
        elements_by_tag(&ctx, self.handle.0, &name.0)
    }

    #[qjs(get)]
    fn body<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        document_value(&ctx, self.handle.0, |parsed| document_first(parsed, "body"))
    }

    // https://html.spec.whatwg.org/multipage/dom.html#dom-document-head
    #[qjs(get)]
    fn head<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        document_value(&ctx, self.handle.0, |parsed| document_first(parsed, "head"))
    }

    // https://html.spec.whatwg.org/multipage/dom.html#dom-document-currentscript
    #[qjs(get, rename = "currentScript")]
    fn current_script<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let current = world.borrow().current_script;
        match current {
            Some(id) if id.document_id() == self.handle.0.document_id() => wrap_node(&ctx, id),
            _ => Ok(Value::new_null(ctx)),
        }
    }

    #[qjs(get, rename = "documentElement")]
    fn document_element<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        document_value(&ctx, self.handle.0, |parsed| {
            document_first_child(parsed, |kind| matches!(kind, NodeKind::Element { .. }))
        })
    }

    // https://dom.spec.whatwg.org/#dom-document-doctype
    #[qjs(get, rename = "doctype")]
    fn doctype<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        document_value(&ctx, self.handle.0, |parsed| {
            document_first_child(parsed, |kind| matches!(kind, NodeKind::Doctype { .. }))
        })
    }

    // https://dom.spec.whatwg.org/#dom-document-readyState
    #[qjs(get, rename = "readyState")]
    fn ready_state(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(parsed) = world.document(self.handle.0) else {
            return Ok(String::new());
        };
        if parsed.dom.document() != self.handle.0 {
            return Ok(String::new());
        }
        Ok(match parsed.ready_state {
            ReadyState::Loading => "loading".into(),
            ReadyState::Interactive => "interactive".into(),
            ReadyState::Complete => "complete".into(),
        })
    }

    // https://dom.spec.whatwg.org/#dom-document-url
    #[qjs(get, rename = "URL")]
    fn url(&self, ctx: Ctx<'_>) -> String {
        document_url_string(&ctx, self.handle.0)
    }

    // https://dom.spec.whatwg.org/#dom-document-documenturi
    #[qjs(get, rename = "documentURI")]
    fn document_uri(&self, ctx: Ctx<'_>) -> String {
        document_url_string(&ctx, self.handle.0)
    }

    // https://html.spec.whatwg.org/multipage/urls-and-fetching.html#dom-document-baseuri
    #[qjs(get, rename = "baseURI")]
    fn base_uri(&self, ctx: Ctx<'_>) -> String {
        document_base_url_string(&ctx, self.handle.0)
    }

    // https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-document-location
    #[qjs(get)]
    fn location<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        if is_main_document(&ctx, self.handle.0) {
            return ctx.globals().get("location");
        }
        Ok(Value::new_null(ctx))
    }

    // https://encoding.spec.whatwg.org/#dom-document-characterset
    #[qjs(get, rename = "characterSet")]
    fn character_set(&self) -> &'static str {
        "UTF-8"
    }

    #[qjs(get, rename = "charset")]
    fn charset(&self) -> &'static str {
        "UTF-8"
    }

    #[qjs(get, rename = "inputEncoding")]
    fn input_encoding(&self) -> &'static str {
        "UTF-8"
    }

    // https://dom.spec.whatwg.org/#dom-document-contenttype
    #[qjs(get, rename = "contentType")]
    fn content_type(&self, ctx: Ctx<'_>) -> Result<&'static str> {
        let world_rc = world(&ctx)?;
        let parsed = world_rc.borrow();
        Ok(parsed
            .document(self.handle.0)
            .map_or("text/html", |parsed| parsed.content_type))
    }

    // https://dom.spec.whatwg.org/#dom-document-compatmode
    #[qjs(get, rename = "compatMode")]
    fn compat_mode(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let quirks = parsed
            .document(self.handle.0)
            .is_some_and(|parsed| parsed.quirks_mode == markup5ever::interface::QuirksMode::Quirks);
        Ok(if quirks { "BackCompat" } else { "CSS1Compat" }.into())
    }

    #[qjs(rename = "getAttribute")]
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
            .and_then(|parsed| parsed.dom.attribute(self.handle.0, &local));
        match found {
            Some(value) => string_value(&ctx, &value),
            None => Ok(Value::new_null(ctx)),
        }
    }

    #[qjs(rename = "setAttribute")]
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
            parsed
                .dom
                .set_attribute(self.handle.0, &local, value.0.clone())
                .map_err(|err| throw_dom_error(&ctx, err))?;
        }
        touch_attr(&ctx, self.handle.0, "", &local, &value.0)?;
        after_attribute_change(&ctx, self.handle.0, &local)?;
        schedule_mutation_delivery(&ctx)
    }

    #[qjs(get)]
    fn id(&self, ctx: Ctx<'_>) -> Result<String> {
        attribute_value(&ctx, self.handle.0, "id")
    }

    #[qjs(set, rename = "id")]
    fn set_id(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        self.set_attribute(ctx, WebIdlString("id".into()), value)
    }

    // https://html.spec.whatwg.org/multipage/input.html#dom-input-value
    #[qjs(get)]
    fn value(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(parsed) = world.document(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        Ok(parsed.dom.element_value(self.handle.0).unwrap_or_default())
    }

    // https://html.spec.whatwg.org/multipage/input.html#dom-input-value
    #[qjs(set, rename = "value")]
    fn set_value(&self, ctx: Ctx<'_>, value: LegacyNullString) -> Result<()> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        parsed
            .dom
            .set_element_value(self.handle.0, value.0)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        Ok(())
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-textarea-defaultvalue
    #[qjs(get, rename = "defaultValue")]
    fn default_value(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(parsed) = world.document(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        Ok(parsed
            .dom
            .control_default_value(self.handle.0)
            .unwrap_or_default())
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-textarea-defaultvalue
    #[qjs(set, rename = "defaultValue")]
    fn set_default_value(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        parsed
            .dom
            .set_control_default_value(self.handle.0, value.0)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        Ok(())
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-textarea-textlength
    #[qjs(get, rename = "textLength")]
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
        Ok(parsed
            .dom
            .textarea_value(self.handle.0)
            .map_or(0, |value| value.encode_utf16().count() as u32))
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-textarea/input-selectionstart
    #[qjs(get, rename = "selectionStart")]
    fn selection_start<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(parsed) = world.document(self.handle.0) else {
            return Ok(Value::new_null(ctx));
        };
        match parsed.dom.selection(self.handle.0) {
            Some((start, _, _)) => Ok(Value::new_number(ctx, f64::from(start))),
            None => Ok(Value::new_null(ctx)),
        }
    }

    #[qjs(set, rename = "selectionStart")]
    fn set_selection_start(&self, ctx: Ctx<'_>, value: WebIdlUnsignedLong) -> Result<()> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        let Some((_, end, direction)) = parsed.dom.selection(self.handle.0) else {
            return Err(throw_dom(
                &ctx,
                "InvalidStateError",
                "selectionStart does not apply to this control",
            ));
        };
        let start = value.0;
        let changed = parsed
            .dom
            .set_selection(self.handle.0, start, end.max(start), direction);
        drop(parsed);
        if changed {
            world.queue_select(self.handle.0);
        }
        Ok(())
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-textarea/input-selectionend
    #[qjs(get, rename = "selectionEnd")]
    fn selection_end<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(parsed) = world.document(self.handle.0) else {
            return Ok(Value::new_null(ctx));
        };
        match parsed.dom.selection(self.handle.0) {
            Some((_, end, _)) => Ok(Value::new_number(ctx, f64::from(end))),
            None => Ok(Value::new_null(ctx)),
        }
    }

    #[qjs(set, rename = "selectionEnd")]
    fn set_selection_end(&self, ctx: Ctx<'_>, value: WebIdlUnsignedLong) -> Result<()> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        let Some((start, _, direction)) = parsed.dom.selection(self.handle.0) else {
            return Err(throw_dom(
                &ctx,
                "InvalidStateError",
                "selectionEnd does not apply to this control",
            ));
        };
        let changed = parsed.dom.set_selection(self.handle.0, start, value.0, direction);
        drop(parsed);
        if changed {
            world.queue_select(self.handle.0);
        }
        Ok(())
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-textarea/input-selectiondirection
    #[qjs(get, rename = "selectionDirection")]
    fn selection_direction<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(parsed) = world.document(self.handle.0) else {
            return Ok(Value::new_null(ctx));
        };
        match parsed.dom.selection(self.handle.0) {
            Some((_, _, direction)) => Ok(string_value(&ctx, direction_name(direction))?),
            None => Ok(Value::new_null(ctx)),
        }
    }

    #[qjs(set, rename = "selectionDirection")]
    fn set_selection_direction(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        let Some((start, end, _)) = parsed.dom.selection(self.handle.0) else {
            return Err(throw_dom(
                &ctx,
                "InvalidStateError",
                "selectionDirection does not apply to this control",
            ));
        };
        let direction = direction_code(&value.0);
        let changed = parsed.dom.set_selection(self.handle.0, start, end, direction);
        drop(parsed);
        if changed {
            world.queue_select(self.handle.0);
        }
        Ok(())
    }

    // https://html.spec.whatwg.org/multipage/forms.html#dom-form-reset
    #[qjs(rename = "reset")]
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
                .dom
                .children(node)
                .map(Iterator::collect)
                .unwrap_or_default();
            for child in children {
                if let Some(NodeKind::Element { name, .. }) = parsed.dom.kind(child)
                    && name.ns == html_namespace()
                    && matches!(name.local.as_ref(), "input" | "textarea" | "select")
                {
                    controls.push(child);
                }
                stack.push(child);
            }
        }
        for control in controls {
            parsed.dom.reset_control(control);
        }
        Ok(())
    }

    // https://html.spec.whatwg.org/multipage/forms.html#dom-form-action
    #[qjs(get, rename = "action")]
    fn action(&self, ctx: Ctx<'_>) -> Result<String> {
        let world_rc = world(&ctx)?;
        let raw = world_rc
            .borrow()
            .document(self.handle.0)
            .and_then(|parsed| parsed.dom.attribute(self.handle.0, "action"));
        let base = document_base_url_string(&ctx, self.handle.0);
        let Some(raw) = raw else {
            return Ok(base);
        };
        Ok(url::Url::parse(&base)
            .ok()
            .and_then(|base| base.join(&raw).ok())
            .map_or(raw, |url| url.to_string()))
    }

    #[qjs(set, rename = "action")]
    fn set_action(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        self.set_attribute(ctx, WebIdlString("action".into()), value)
    }

    // https://html.spec.whatwg.org/multipage/forms.html#dom-form-method
    #[qjs(get, rename = "method")]
    fn method(&self, ctx: Ctx<'_>) -> Result<String> {
        let raw = attribute_value(&ctx, self.handle.0, "method")?;
        Ok(match raw.trim().to_ascii_lowercase().as_str() {
            "post" => "post",
            "dialog" => "dialog",
            _ => "get",
        }
        .to_owned())
    }

    #[qjs(set, rename = "method")]
    fn set_method(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        self.set_attribute(ctx, WebIdlString("method".into()), value)
    }

    // https://html.spec.whatwg.org/multipage/forms.html#dom-form-enctype
    #[qjs(get, rename = "enctype")]
    fn enctype(&self, ctx: Ctx<'_>) -> Result<String> {
        Ok(encoding_keyword(&attribute_value(&ctx, self.handle.0, "enctype")?).to_owned())
    }

    #[qjs(set, rename = "enctype")]
    fn set_enctype(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        self.set_attribute(ctx, WebIdlString("enctype".into()), value)
    }

    #[qjs(get, rename = "encoding")]
    fn encoding(&self, ctx: Ctx<'_>) -> Result<String> {
        Ok(encoding_keyword(&attribute_value(&ctx, self.handle.0, "enctype")?).to_owned())
    }

    #[qjs(set, rename = "encoding")]
    fn set_encoding(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        self.set_attribute(ctx, WebIdlString("enctype".into()), value)
    }

    #[qjs(set, rename = "target")]
    fn set_target(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        self.set_attribute(ctx, WebIdlString("target".into()), value)
    }

    // https://html.spec.whatwg.org/multipage/forms.html#dom-form-novalidate
    #[qjs(get, rename = "noValidate")]
    fn no_validate(&self, ctx: Ctx<'_>) -> Result<bool> {
        self.attribute_present(&ctx, "novalidate")
    }

    #[qjs(set, rename = "noValidate")]
    fn set_no_validate(&self, ctx: Ctx<'_>, value: bool) -> Result<()> {
        self.reflect_boolean(ctx, "novalidate", value)
    }

    // https://html.spec.whatwg.org/multipage/forms.html#dom-form-acceptcharset
    #[qjs(get, rename = "acceptCharset")]
    fn accept_charset(&self, ctx: Ctx<'_>) -> Result<String> {
        attribute_value(&ctx, self.handle.0, "accept-charset")
    }

    #[qjs(set, rename = "acceptCharset")]
    fn set_accept_charset(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        self.set_attribute(ctx, WebIdlString("accept-charset".into()), value)
    }

    // https://drafts.csswg.org/cssom-view/#dom-element-scrollleft
    #[qjs(get, rename = "scrollLeft")]
    fn scroll_left(&self, ctx: Ctx<'_>) -> Result<f64> {
        let world = world(&ctx)?;
        let world = world.borrow();
        Ok(world
            .document(self.handle.0)
            .map_or(0.0, |parsed| parsed.dom.scroll_offset(self.handle.0).0))
    }

    #[qjs(set, rename = "scrollLeft")]
    fn set_scroll_left(&self, ctx: Ctx<'_>, value: f64) -> Result<()> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        let (_, top) = parsed.dom.scroll_offset(self.handle.0);
        parsed.dom.set_scroll_offset(self.handle.0, value, top);
        Ok(())
    }

    // https://drafts.csswg.org/cssom-view/#dom-element-scrolltop
    #[qjs(get, rename = "scrollTop")]
    fn scroll_top(&self, ctx: Ctx<'_>) -> Result<f64> {
        let world = world(&ctx)?;
        let world = world.borrow();
        Ok(world
            .document(self.handle.0)
            .map_or(0.0, |parsed| parsed.dom.scroll_offset(self.handle.0).1))
    }

    #[qjs(set, rename = "scrollTop")]
    fn set_scroll_top(&self, ctx: Ctx<'_>, value: f64) -> Result<()> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        let (left, _) = parsed.dom.scroll_offset(self.handle.0);
        parsed.dom.set_scroll_offset(self.handle.0, left, value);
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
            .is_some_and(|parsed| parsed.dom.attribute(self.handle.0, name).is_some()))
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-fe-disabled
    #[qjs(get, rename = "disabled")]
    fn disabled(&self, ctx: Ctx<'_>) -> Result<bool> {
        self.attribute_present(&ctx, "disabled")
    }

    #[qjs(set, rename = "disabled")]
    fn set_disabled(&self, ctx: Ctx<'_>, value: bool) -> Result<()> {
        self.reflect_boolean(ctx, "disabled", value)
    }

    // https://html.spec.whatwg.org/multipage/input.html#dom-input-readonly
    #[qjs(get, rename = "readOnly")]
    fn read_only(&self, ctx: Ctx<'_>) -> Result<bool> {
        self.attribute_present(&ctx, "readonly")
    }

    #[qjs(set, rename = "readOnly")]
    fn set_read_only(&self, ctx: Ctx<'_>, value: bool) -> Result<()> {
        self.reflect_boolean(ctx, "readonly", value)
    }

    // https://html.spec.whatwg.org/multipage/input.html#dom-input-required
    #[qjs(get, rename = "required")]
    fn required(&self, ctx: Ctx<'_>) -> Result<bool> {
        self.attribute_present(&ctx, "required")
    }

    #[qjs(set, rename = "required")]
    fn set_required(&self, ctx: Ctx<'_>, value: bool) -> Result<()> {
        self.reflect_boolean(ctx, "required", value)
    }

    // https://html.spec.whatwg.org/multipage/select.html#dom-select-multiple
    #[qjs(get, rename = "multiple")]
    fn multiple(&self, ctx: Ctx<'_>) -> Result<bool> {
        self.attribute_present(&ctx, "multiple")
    }

    #[qjs(set, rename = "multiple")]
    fn set_multiple(&self, ctx: Ctx<'_>, value: bool) -> Result<()> {
        self.reflect_boolean(ctx, "multiple", value)
    }

    // https://html.spec.whatwg.org/multipage/input.html#dom-input-checked
    #[qjs(get)]
    fn checked(&self, ctx: Ctx<'_>) -> Result<bool> {
        let world = world(&ctx)?;
        let world = world.borrow();
        Ok(world
            .document(self.handle.0)
            .is_some_and(|parsed| parsed.dom.checkedness(self.handle.0)))
    }

    #[qjs(set, rename = "checked")]
    fn set_checked(&self, ctx: Ctx<'_>, value: bool) -> Result<()> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        parsed.dom.set_checkedness(self.handle.0, value);
        Ok(())
    }

    // https://html.spec.whatwg.org/multipage/input.html#dom-input-defaultchecked
    #[qjs(get, rename = "defaultChecked")]
    fn default_checked(&self, ctx: Ctx<'_>) -> Result<bool> {
        self.attribute_present(&ctx, "checked")
    }

    #[qjs(set, rename = "defaultChecked")]
    fn set_default_checked(&self, ctx: Ctx<'_>, value: bool) -> Result<()> {
        self.reflect_boolean(ctx, "checked", value)
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-selected
    #[qjs(get, rename = "selected")]
    fn selected(&self, ctx: Ctx<'_>) -> Result<bool> {
        let world = world(&ctx)?;
        let world = world.borrow();
        Ok(world
            .document(self.handle.0)
            .is_some_and(|parsed| parsed.dom.option_selected(self.handle.0)))
    }

    #[qjs(set, rename = "selected")]
    fn set_selected(&self, ctx: Ctx<'_>, value: bool) -> Result<()> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        parsed
            .dom
            .set_option_selected_in_select(self.handle.0, value);
        Ok(())
    }

    #[qjs(get, rename = "defaultSelected")]
    fn default_selected(&self, ctx: Ctx<'_>) -> Result<bool> {
        self.attribute_present(&ctx, "selected")
    }

    #[qjs(set, rename = "defaultSelected")]
    fn set_default_selected(&self, ctx: Ctx<'_>, value: bool) -> Result<()> {
        self.reflect_boolean(ctx, "selected", value)
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-text
    #[qjs(get)]
    fn text(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        let world = world.borrow();
        Ok(world
            .document(self.handle.0)
            .map_or_else(String::new, |parsed| parsed.dom.text_content(self.handle.0)))
    }

    #[qjs(set, rename = "text")]
    fn set_text(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        let replacement = parsed.dom.create_fragment();
        if !value.0.is_empty() {
            let text = parsed.dom.create_text(value.0);
            parsed
                .dom
                .append(replacement, text)
                .map_err(|err| throw_dom_error(&ctx, err))?;
        }
        parsed
            .dom
            .replace_all(self.handle.0, replacement)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        Ok(())
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-index
    #[qjs(get)]
    fn index(&self, ctx: Ctx<'_>) -> Result<i32> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(parsed) = world.document(self.handle.0) else {
            return Ok(0);
        };
        let Some(select) = parsed.dom.option_select_owner(self.handle.0) else {
            return Ok(0);
        };
        for (index, option) in parsed.dom.select_options(select).into_iter().enumerate() {
            if option == self.handle.0 {
                return Ok(i32::try_from(index).unwrap_or(i32::MAX));
            }
        }
        Ok(0)
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-selectedindex
    #[qjs(get, rename = "selectedIndex")]
    fn selected_index(&self, ctx: Ctx<'_>) -> Result<i32> {
        let world = world(&ctx)?;
        let world = world.borrow();
        Ok(world
            .document(self.handle.0)
            .map_or(-1, |parsed| parsed.dom.select_selected_index(self.handle.0)))
    }

    #[qjs(set, rename = "selectedIndex")]
    fn set_selected_index(&self, ctx: Ctx<'_>, value: i32) -> Result<()> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        parsed.dom.set_select_selected_index(self.handle.0, value);
        Ok(())
    }

    #[qjs(get)]
    fn options<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        live_collection(
            &ctx,
            self.handle.0,
            CollectionKind::ElementsByTag("option".to_owned()),
            Some("HTMLCollection"),
        )
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-selectedoptions
    #[qjs(get, rename = "selectedOptions")]
    fn selected_options<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(parsed) = world.document(self.handle.0) else {
            return Ok(Value::new_null(ctx));
        };
        let handles: Vec<Handle> = parsed
            .dom
            .select_options(self.handle.0)
            .into_iter()
            .filter(|&option| parsed.dom.option_selected(option))
            .map(Handle)
            .collect();
        live_collection(
            &ctx,
            self.handle.0,
            CollectionKind::Static(handles),
            Some("HTMLCollection"),
        )
    }

    /// URL-reflected `src`: parsed against the document base and stored
    /// serialized, like `href`; an absent attribute reflects as the empty
    /// string (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#reflecting-content-attributes-in-idl-attributes>).
    #[qjs(get)]
    fn src(&self, ctx: Ctx<'_>) -> Result<String> {
        let world_rc = world(&ctx)?;
        let raw = world_rc
            .borrow()
            .document(self.handle.0)
            .and_then(|parsed| parsed.dom.attribute(self.handle.0, "src"));
        let Some(raw) = raw else {
            return Ok(String::new());
        };
        let base = document_base_url_string(&ctx, self.handle.0);
        Ok(url::Url::parse(&base)
            .ok()
            .and_then(|base| base.join(&raw).ok())
            .map_or(raw, |url| url.to_string()))
    }

    #[qjs(set, rename = "src")]
    fn set_src(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        self.set_attribute(ctx, WebIdlString("src".into()), value)
    }

    // https://html.spec.whatwg.org/multipage/embedded-content.html#dom-img-naturalwidth
    #[qjs(get, rename = "naturalWidth")]
    fn natural_width(&self, ctx: Ctx<'_>) -> Result<u32> {
        Ok(img_size(&ctx, self.handle.0)?.map_or(0, |(width, _)| width))
    }

    // https://html.spec.whatwg.org/multipage/embedded-content.html#dom-img-naturalheight
    #[qjs(get, rename = "naturalHeight")]
    fn natural_height(&self, ctx: Ctx<'_>) -> Result<u32> {
        Ok(img_size(&ctx, self.handle.0)?.map_or(0, |(_, height)| height))
    }

    // https://html.spec.whatwg.org/multipage/embedded-content.html#dom-img-complete
    #[qjs(get)]
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
            .and_then(|parsed| parsed.dom.attribute(self.handle.0, "src"));
        if src.as_deref().is_none_or(str::is_empty) {
            return Ok(true);
        }
        Ok(world.image_broken.contains(&self.handle.0))
    }

    // https://html.spec.whatwg.org/multipage/embedded-content.html#dom-img-currentsrc
    #[qjs(get, rename = "currentSrc")]
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
    #[qjs(get, rename = "contentDocument")]
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
    #[qjs(get, rename = "contentWindow")]
    fn content_window<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        if !iframe_frame(&ctx, self.handle.0)? {
            return Ok(Value::new_null(ctx));
        }
        let frame = world(&ctx)?.borrow().frame_for_container(self.handle.0);
        match frame {
            Some(frame) => {
                let proxy: Function = ctx.globals().get("__tbFrameProxy")?;
                proxy.call((crate::js::js_number(frame.get()),))
            }
            None => Ok(Value::new_null(ctx)),
        }
    }

    #[qjs(get)]
    fn name(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(parsed) = world.document(self.handle.0) else {
            return Ok(String::new());
        };
        match doctype_fields(&parsed, self.handle.0) {
            Some((name, _, _)) => Ok(name),
            None => Ok(parsed
                .dom
                .attribute(self.handle.0, "name")
                .unwrap_or_default()),
        }
    }

    // https://dom.spec.whatwg.org/#dom-documenttype-publicid
    #[qjs(get, rename = "publicId")]
    fn public_id(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(parsed) = world.document(self.handle.0) else {
            return Ok(String::new());
        };
        Ok(doctype_fields(&parsed, self.handle.0)
            .map_or(String::new(), |(_, public_id, _)| public_id))
    }

    // https://dom.spec.whatwg.org/#dom-documenttype-systemid
    #[qjs(get, rename = "systemId")]
    fn system_id(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(parsed) = world.document(self.handle.0) else {
            return Ok(String::new());
        };
        Ok(doctype_fields(&parsed, self.handle.0)
            .map_or(String::new(), |(_, _, system_id)| system_id))
    }

    #[qjs(set, rename = "name")]
    fn set_name(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        self.set_attribute(ctx, WebIdlString("name".into()), value)
    }

    #[qjs(get)]
    fn content<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let template_contents = {
            let world = world.borrow();
            world.document(self.handle.0).and_then(|parsed| {
                is_template_element(parsed.dom.kind(self.handle.0))
                    .then(|| parsed.dom.template_contents(self.handle.0))
                    .flatten()
            })
        };
        if let Some(contents) = template_contents {
            return wrap_node(&ctx, contents);
        }

        let value = world
            .borrow()
            .document(self.handle.0)
            .and_then(|parsed| parsed.dom.attribute(self.handle.0, "content"))
            .unwrap_or_default();
        Ok(Value::from_string(rquickjs::String::from_str(ctx, &value)?))
    }

    #[qjs(set, rename = "content")]
    fn set_content(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        self.set_attribute(ctx, WebIdlString("content".into()), value)
    }

    // https://dom.spec.whatwg.org/#dom-element-attachshadow
    #[qjs(rename = "attachShadow")]
    fn attach_shadow<'js>(&self, ctx: Ctx<'js>, init: Object<'js>) -> Result<Value<'js>> {
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
        if parsed.dom.shadow_root(self.handle.0).is_some() {
            return Err(throw_dom(
                &ctx,
                "NotSupportedError",
                "element already hosts a shadow root",
            ));
        }
        let root = parsed
            .dom
            .attach_shadow(self.handle.0, open)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        drop(parsed);
        drop(world_ref);
        wrap_node(&ctx, root)
    }

    #[qjs(get, rename = "shadowRoot")]
    fn shadow_root<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let root = world
            .borrow()
            .document(self.handle.0)
            .and_then(|parsed| parsed.dom.open_shadow_root(self.handle.0));
        child_value(&ctx, root)
    }

    #[qjs(get)]
    fn host<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let host = world
            .borrow()
            .document(self.handle.0)
            .and_then(|parsed| parsed.dom.shadow_host(self.handle.0));
        match host {
            Some(host) => wrap_node(&ctx, host),
            None => Err(Exception::throw_type(&ctx, "not a shadow root")),
        }
    }

    #[qjs(get)]
    fn mode(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        let open = world
            .borrow()
            .document(self.handle.0)
            .and_then(|parsed| parsed.dom.shadow_root_is_open(self.handle.0))
            .ok_or_else(|| Exception::throw_type(&ctx, "not a shadow root"))?;
        Ok(if open { "open" } else { "closed" }.into())
    }

    // https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-element-innerhtml
    #[qjs(get, rename = "innerHTML")]
    fn inner_html(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        if parsed.content_type == "text/html" {
            Ok(crate::serialize::serialize_html_fragment(
                &parsed.dom,
                self.handle.0,
            ))
        } else {
            crate::serialize::serialize_xml_children(&parsed.dom, self.handle.0, true)
                .map_err(|err| throw_dom(&ctx, "InvalidStateError", &err.to_string()))
        }
    }

    // https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-element-outerhtml
    #[qjs(get, rename = "outerHTML")]
    fn outer_html(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        let Some(NodeKind::Element { name, attributes }) = parsed.dom.kind(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "outerHTML requires an element"));
        };
        if parsed.content_type == "text/html" {
            let mut output = String::new();
            crate::serialize::serialize_html_element(
                &parsed.dom,
                self.handle.0,
                name,
                attributes,
                &mut output,
            );
            Ok(output)
        } else {
            crate::serialize::serialize_xml(&parsed.dom, self.handle.0, true)
                .map_err(|err| throw_dom(&ctx, "InvalidStateError", &err.to_string()))
        }
    }

    // https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-element-innerhtml
    #[qjs(set, rename = "innerHTML")]
    fn set_inner_html(&self, ctx: Ctx<'_>, value: LegacyNullString) -> Result<()> {
        let (context, is_template) = with_node_kind(&ctx, self.handle.0, |kind| match kind {
            Some(NodeKind::Element { name, .. }) => {
                Some((html_fragment_context(name), is_template_element(kind)))
            }
            _ => None,
        })?
        .or_else(|| {
            let world = world(&ctx).ok()?;
            let world = world.borrow();
            let parsed = world.document(self.handle.0)?;
            let host = parsed.dom.shadow_host(self.handle.0)?;
            let Some(NodeKind::Element { name, .. }) = parsed.dom.kind(host) else {
                return None;
            };
            Some((html_fragment_context(name), false))
        })
        .ok_or_else(|| {
            Exception::throw_type(&ctx, "innerHTML requires an element or shadow root")
        })?;

        let snapshots = parse_html_fragment_snapshots(&ctx, &value.0, &context)?;

        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        let target = if is_template {
            if let Some(contents) = parsed.dom.template_contents(self.handle.0) {
                contents
            } else {
                let contents = parsed.dom.create_fragment();
                parsed
                    .dom
                    .set_template_contents(self.handle.0, contents)
                    .map_err(|err| throw_dom_error(&ctx, err))?;
                contents
            }
        } else {
            self.handle.0
        };
        let replacement = materialize_children(&mut parsed.dom, &snapshots)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        parsed
            .dom
            .replace_all(target, replacement)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        drop(parsed);
        drop(world);
        schedule_mutation_delivery(&ctx)
    }

    // https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-element-insertadjacenthtml
    #[qjs(rename = "insertAdjacentHTML")]
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
        if needs_parent && parsed.dom.parent(self.handle.0).is_none() {
            return Err(throw_dom(
                &ctx,
                "NoModificationAllowedError",
                "element has no parent",
            ));
        }
        let fragment = materialize_children(&mut parsed.dom, &snapshots)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        let result = if position == "beforebegin" {
            parsed.dom.insert_before(self.handle.0, fragment)
        } else if position == "afterbegin" {
            match parsed.dom.first_child(self.handle.0) {
                Some(first) => parsed.dom.insert_before(first, fragment),
                None => parsed.dom.append(self.handle.0, fragment),
            }
        } else if position == "afterend" {
            if let Some(next) = parsed.dom.next_sibling(self.handle.0) {
                parsed.dom.insert_before(next, fragment)
            } else {
                let Some(parent) = parsed.dom.parent(self.handle.0) else {
                    return Ok(());
                };
                parsed.dom.append(parent, fragment)
            }
        } else {
            parsed.dom.append(self.handle.0, fragment)
        };
        result.map_err(|err| throw_dom_error(&ctx, err))?;
        drop(parsed);
        drop(world);
        schedule_mutation_delivery(&ctx)
    }

    // https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-element-outerhtml
    #[qjs(set, rename = "outerHTML")]
    fn set_outer_html(&self, ctx: Ctx<'_>, value: LegacyNullString) -> Result<()> {
        let world_rc = world(&ctx)?;
        let (parent, context) = {
            let world = world_rc.borrow();
            let Some(parsed) = world.document(self.handle.0) else {
                return Err(Exception::throw_type(&ctx, "no document"));
            };
            let Some(parent) = parsed.dom.parent(self.handle.0) else {
                // A parentless element has nothing to replace.
                return Ok(());
            };
            let Some(kind) = parsed.dom.kind(parent).cloned() else {
                return Err(Exception::throw_type(&ctx, "no document"));
            };
            match kind {
                NodeKind::Document => {
                    return Err(throw_dom(
                        &ctx,
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
        let snapshots = parse_html_fragment_snapshots(&ctx, &value.0, &context)?;

        let world = world_rc.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        let replacement = materialize_children(&mut parsed.dom, &snapshots)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        parsed
            .dom
            .replace_child(parent, replacement, self.handle.0)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        drop(parsed);
        drop(world);
        schedule_mutation_delivery(&ctx)
    }

    /// URL-reflected `href`: parsed against the document base and stored
    /// serialized (percent-encoded).
    #[qjs(get)]
    fn href(&self, ctx: Ctx<'_>) -> Result<String> {
        let world_rc = world(&ctx)?;
        let raw = world_rc
            .borrow()
            .document(self.handle.0)
            .and_then(|parsed| parsed.dom.attribute(self.handle.0, "href"))
            .unwrap_or_default();
        let base = document_base_url_string(&ctx, self.handle.0);
        Ok(url::Url::parse(&base)
            .ok()
            .and_then(|base| base.join(&raw).ok())
            .map_or(raw, |url| url.to_string()))
    }

    #[qjs(set, rename = "href")]
    fn set_href(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        // URL reflection stores the given value; resolution happens on get
        // (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#reflecting-content-attributes-in-idl-attributes>).
        self.set_attribute(ctx, WebIdlString("href".into()), value)
    }

    #[qjs(get, rename = "style")]
    fn style<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world_rc = world_for_node(&ctx, self.handle.0)?;
        if let Some(saved) = world_rc
            .borrow()
            .wrapper(self.handle.0, Wrapper::StyleDeclaration)
            && let Some(value) = deref_weak(&ctx, saved)?
        {
            return Ok(value);
        }
        let factory: Function = ctx.globals().get("__tbMakeStyle")?;
        let element = wrap_node(&ctx, self.handle.0)?;
        let value: Value = factory.call((element,))?;
        let weak = make_weak(&ctx, value.clone())?;
        world_rc.borrow_mut().intern_wrapper(
            self.handle.0,
            Wrapper::StyleDeclaration,
            Persistent::save(&ctx, weak),
        );
        Ok(value)
    }

    #[qjs(set, rename = "style")]
    fn set_style(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        self.set_attribute(ctx, WebIdlString("style".into()), value)
    }

    #[qjs(get)]
    fn data(&self, ctx: Ctx<'_>) -> Result<String> {
        character_data(&ctx, self.handle.0)
    }

    #[qjs(set, rename = "data")]
    fn set_data(&self, ctx: Ctx<'_>, value: LegacyNullString) -> Result<()> {
        set_character_data(&ctx, self.handle.0, value.0)
    }

    // https://dom.spec.whatwg.org/#dom-node-lastchild
    #[qjs(get, rename = "lastChild")]
    fn last_child<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let id = world
            .borrow()
            .with_document(self.handle.0, |parsed| {
                parsed
                    .dom
                    .children(self.handle.0)
                    .and_then(|mut kids| kids.next_back())
            })
            .flatten();
        child_value(&ctx, id)
    }

    // https://dom.spec.whatwg.org/#dom-node-nextsibling
    #[qjs(get, rename = "nextSibling")]
    fn next_sibling<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        sibling_value(&ctx, self.handle.0, true)
    }

    // https://dom.spec.whatwg.org/#dom-node-previoussibling
    #[qjs(get, rename = "previousSibling")]
    fn previous_sibling<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        sibling_value(&ctx, self.handle.0, false)
    }

    // https://dom.spec.whatwg.org/#dom-node-ownerdocument
    #[qjs(get, rename = "ownerDocument")]
    fn owner_document<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let id = world.borrow().document(self.handle.0).and_then(|parsed| {
            let document = parsed.dom.document();
            (document != self.handle.0).then_some(document)
        });
        child_value(&ctx, id)
    }

    // https://dom.spec.whatwg.org/#dom-node-haschildnodes
    #[qjs(rename = "hasChildNodes")]
    fn has_child_nodes(&self, ctx: Ctx<'_>) -> Result<bool> {
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(false);
        };
        Ok(parsed
            .dom
            .children(self.handle.0)
            .is_some_and(|mut kids| kids.next().is_some()))
    }

    // https://dom.spec.whatwg.org/#dom-node-nodevalue
    #[qjs(get, rename = "nodeValue")]
    fn node_value<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(Value::new_null(ctx));
        };
        match parsed.dom.kind(self.handle.0) {
            Some(
                NodeKind::Text { data }
                | NodeKind::CDataSection { data }
                | NodeKind::ProcessingInstruction { data, .. }
                | NodeKind::Comment { data },
            ) => string_value(&ctx, data),
            _ => Ok(Value::new_null(ctx)),
        }
    }

    #[qjs(set, rename = "nodeValue")]
    fn set_node_value(&self, ctx: Ctx<'_>, value: OptString) -> Result<()> {
        // "If the given value is null, act as if it was the empty string"
        // (<https://dom.spec.whatwg.org/#dom-node-nodevalue>); `OptString`
        // maps null and undefined to `None`.
        set_character_data(&ctx, self.handle.0, value.0.unwrap_or_default())
    }

    // https://dom.spec.whatwg.org/#dom-node-textcontent
    #[qjs(get, rename = "textContent")]
    fn text_content<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(Value::new_null(ctx));
        };
        match parsed.dom.kind(self.handle.0) {
            Some(NodeKind::Element { .. } | NodeKind::Fragment) => {
                let text = descendant_text(&parsed.dom, self.handle.0);
                string_value(&ctx, &text)
            }
            Some(
                NodeKind::Text { data }
                | NodeKind::CDataSection { data }
                | NodeKind::ProcessingInstruction { data, .. }
                | NodeKind::Comment { data },
            ) => string_value(&ctx, data),
            _ => Ok(Value::new_null(ctx)),
        }
    }

    #[qjs(set, rename = "textContent")]
    fn set_text_content(&self, ctx: Ctx<'_>, value: OptString) -> Result<()> {
        let text = value.0.unwrap_or_default();
        // A `CharacterData` node [replaces its data] in place; `Document` and
        // `DocumentType` ignore the setter; every other node replaces its
        // children with one `Text` node
        // (<https://dom.spec.whatwg.org/#dom-node-textcontent>).
        let world_rc = world(&ctx)?;
        let character_data = {
            let world = world_rc.borrow();
            let Some(parsed) = world.document(self.handle.0) else {
                return Ok(());
            };
            matches!(
                parsed.dom.kind(self.handle.0),
                Some(
                    NodeKind::Text { .. }
                        | NodeKind::CDataSection { .. }
                        | NodeKind::ProcessingInstruction { .. }
                        | NodeKind::Comment { .. }
                )
            )
        };
        if character_data {
            return set_character_data(&ctx, self.handle.0, text);
        }
        let world = world_rc.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        if matches!(
            parsed.dom.kind(self.handle.0),
            Some(NodeKind::Document | NodeKind::Doctype { .. })
        ) {
            return Ok(());
        }
        let dom = &mut parsed.dom;
        let replacement = dom.create_fragment();
        if !text.is_empty() {
            let text_id = dom.create_text(text);
            dom.append(replacement, text_id)
                .map_err(|err| throw_dom_error(&ctx, err))?;
        }
        dom.replace_all(self.handle.0, replacement)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        drop(parsed);
        drop(world);
        schedule_mutation_delivery(&ctx)
    }

    // ── ParentNode ───────────────────────────────────────────────────────

    // https://dom.spec.whatwg.org/#dom-parentnode-children
    #[qjs(get)]
    fn children<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        live_collection(
            &ctx,
            self.handle.0,
            CollectionKind::ElementChildren,
            Some("HTMLCollection"),
        )
    }

    // https://dom.spec.whatwg.org/#dom-parentnode-firstelementchild
    #[qjs(get, rename = "firstElementChild")]
    fn first_element_child<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let found = {
            let parsed = world.borrow();
            let Some(parsed) = parsed.document(self.handle.0) else {
                return Ok(Value::new_null(ctx));
            };
            parsed
                .dom
                .children(self.handle.0)
                .and_then(|mut kids| kids.find(|&kid| is_element(&parsed.dom, kid)))
        };
        child_value(&ctx, found)
    }

    // https://dom.spec.whatwg.org/#dom-parentnode-lastelementchild
    #[qjs(get, rename = "lastElementChild")]
    fn last_element_child<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let found = {
            let parsed = world.borrow();
            let Some(parsed) = parsed.document(self.handle.0) else {
                return Ok(Value::new_null(ctx));
            };
            parsed
                .dom
                .children(self.handle.0)
                .and_then(|kids| kids.rev().find(|&kid| is_element(&parsed.dom, kid)))
        };
        child_value(&ctx, found)
    }

    // https://dom.spec.whatwg.org/#dom-parentnode-childelementcount
    #[qjs(get, rename = "childElementCount")]
    fn child_element_count(&self, ctx: Ctx<'_>) -> Result<usize> {
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(0);
        };
        Ok(parsed.dom.children(self.handle.0).map_or(0, |kids| {
            kids.filter(|&kid| is_element(&parsed.dom, kid)).count()
        }))
    }

    // https://dom.spec.whatwg.org/#dom-parentnode-append
    #[qjs(rename = "append")]
    fn append<'js>(&self, ctx: Ctx<'js>, nodes: Rest<Value<'js>>) -> Result<()> {
        let node = convert_nodes_into_node(&ctx, self.handle.0, nodes)?;
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        parsed
            .dom
            .pre_insert(self.handle.0, node, None)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        drop(parsed);
        drop(world);
        schedule_mutation_delivery(&ctx)
    }

    // https://dom.spec.whatwg.org/#dom-parentnode-prepend
    #[qjs(rename = "prepend")]
    fn prepend<'js>(&self, ctx: Ctx<'js>, nodes: Rest<Value<'js>>) -> Result<()> {
        let node = convert_nodes_into_node(&ctx, self.handle.0, nodes)?;
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        let reference = parsed
            .dom
            .children(self.handle.0)
            .and_then(|mut kids| kids.next());
        parsed
            .dom
            .pre_insert(self.handle.0, node, reference)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        drop(parsed);
        drop(world);
        schedule_mutation_delivery(&ctx)
    }

    // https://dom.spec.whatwg.org/#dom-parentnode-replacechildren
    #[qjs(rename = "replaceChildren")]
    fn replace_children<'js>(&self, ctx: Ctx<'js>, nodes: Rest<Value<'js>>) -> Result<()> {
        let node = convert_nodes_into_node(&ctx, self.handle.0, nodes)?;
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        parsed
            .dom
            .replace_all(self.handle.0, node)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        drop(parsed);
        drop(world);
        schedule_mutation_delivery(&ctx)
    }

    // https://dom.spec.whatwg.org/#dom-parentnode-queryselector
    #[qjs(rename = "querySelector")]
    fn query_selector<'js>(&self, ctx: Ctx<'js>, selectors: WebIdlString) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let found = {
            let parsed = world.borrow();
            let Some(parsed) = parsed.document(self.handle.0) else {
                return Ok(Value::new_null(ctx));
            };
            parsed
                .dom
                .select_first(self.handle.0, &selectors.0)
                .map_err(|err| select_error(&ctx, &err))?
        };
        child_value(&ctx, found)
    }

    // https://dom.spec.whatwg.org/#dom-parentnode-queryselectorall
    #[qjs(rename = "querySelectorAll")]
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
                    parsed
                        .dom
                        .select_all(self.handle.0, &selectors.0)
                        .map_err(|err| select_error(&ctx, &err))
                })
                .transpose()?
                .unwrap_or_default()
        };
        let handles = ids.into_iter().map(Handle).collect();
        live_collection(&ctx, self.handle.0, CollectionKind::Static(handles), None)
    }

    // https://dom.spec.whatwg.org/#dom-element-matches
    #[qjs(rename = "matches")]
    fn matches(&self, ctx: Ctx<'_>, selectors: WebIdlString) -> Result<bool> {
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(false);
        };
        parsed
            .dom
            .matches(self.handle.0, &selectors.0)
            .map_err(|err| select_error(&ctx, &err))
    }

    // https://dom.spec.whatwg.org/#dom-element-closest
    #[qjs(rename = "closest")]
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
                if is_element(&parsed.dom, id)
                    && parsed
                        .dom
                        .matches(id, &selectors.0)
                        .map_err(|err| select_error(&ctx, &err))?
                {
                    candidate = Some(id);
                    break;
                }
                cursor = parsed.dom.parent(id);
            }
            candidate
        };
        child_value(&ctx, found)
    }

    // https://dom.spec.whatwg.org/#dom-element-getelementsbyclassname
    #[qjs(rename = "getElementsByClassName")]
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
    #[qjs(get)]
    fn title(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(String::new());
        };
        let title = parsed
            .dom
            .select_first(parsed.dom.document(), "title")
            .ok()
            .flatten();
        Ok(title.map_or_else(String::new, |title| descendant_text(&parsed.dom, title)))
    }

    #[qjs(set, rename = "title")]
    fn set_title(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        let dom = &mut parsed.dom;
        let title = dom
            .select_first(dom.document(), "title")
            .ok()
            .flatten()
            .unwrap_or_else(|| {
                let name = QualName::new(None, html_namespace(), LocalName::from("title"));
                let title = dom.create_element(name, Vec::new());
                let target = dom
                    .select_first(dom.document(), "head")
                    .ok()
                    .flatten()
                    .or_else(|| dom.select_first(dom.document(), "html").ok().flatten());
                if let Some(target) = target {
                    let _ = dom.append(target, title);
                }
                title
            });
        let kids: Vec<NodeId> = dom
            .children(title)
            .map(Iterator::collect)
            .unwrap_or_default();
        for kid in kids {
            dom.detach(kid).map_err(|err| throw_dom_error(&ctx, err))?;
        }
        if !value.0.is_empty() {
            let text = dom.create_text(value.0);
            dom.append(title, text)
                .map_err(|err| throw_dom_error(&ctx, err))?;
        }
        drop(parsed);
        drop(world);
        schedule_mutation_delivery(&ctx)
    }

    // https://html.spec.whatwg.org/multipage/dom.html#dom-document-getelementsbyname
    #[qjs(rename = "getElementsByName")]
    fn get_elements_by_name<'js>(&self, ctx: Ctx<'js>, name: WebIdlString) -> Result<Value<'js>> {
        live_collection(
            &ctx,
            self.handle.0,
            CollectionKind::ElementsByName(name.0),
            None,
        )
    }

    // https://dom.spec.whatwg.org/#dom-element-getelementsbytagnamens
    #[qjs(rename = "getElementsByTagNameNS")]
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
    #[qjs(rename = "before")]
    fn before<'js>(&self, ctx: Ctx<'js>, nodes: Rest<Value<'js>>) -> Result<()> {
        let node = convert_nodes_into_node(&ctx, self.handle.0, nodes)?;
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        let Some(parent) = parsed.dom.parent(self.handle.0) else {
            return Ok(());
        };
        let previous = parsed.dom.sibling(self.handle.0, false);
        let reference = match previous {
            Some(previous) => parsed.dom.sibling(previous, true),
            None => parsed.dom.children(parent).and_then(|mut kids| kids.next()),
        };
        parsed
            .dom
            .pre_insert(parent, node, reference)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        drop(parsed);
        drop(world);
        schedule_mutation_delivery(&ctx)
    }

    // https://dom.spec.whatwg.org/#dom-childnode-after
    #[qjs(rename = "after")]
    fn after<'js>(&self, ctx: Ctx<'js>, nodes: Rest<Value<'js>>) -> Result<()> {
        let node = convert_nodes_into_node(&ctx, self.handle.0, nodes)?;
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        let Some(parent) = parsed.dom.parent(self.handle.0) else {
            return Ok(());
        };
        let reference = parsed.dom.sibling(self.handle.0, true);
        parsed
            .dom
            .pre_insert(parent, node, reference)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        drop(parsed);
        drop(world);
        schedule_mutation_delivery(&ctx)
    }

    // https://dom.spec.whatwg.org/#dom-childnode-replacewith
    #[qjs(rename = "replaceWith")]
    fn replace_with<'js>(&self, ctx: Ctx<'js>, nodes: Rest<Value<'js>>) -> Result<()> {
        let node = convert_nodes_into_node(&ctx, self.handle.0, nodes)?;
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        let Some(parent) = parsed.dom.parent(self.handle.0) else {
            return Ok(());
        };
        parsed
            .dom
            .replace_child(parent, node, self.handle.0)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        drop(parsed);
        drop(world);
        schedule_mutation_delivery(&ctx)
    }

    // https://dom.spec.whatwg.org/#dom-nondocumenttypechildnode-previouselementsibling
    #[qjs(get, rename = "previousElementSibling")]
    fn previous_element_sibling<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        element_sibling_value(&ctx, self.handle.0, false)
    }

    // https://dom.spec.whatwg.org/#dom-nondocumenttypechildnode-nextelementsibling
    #[qjs(get, rename = "nextElementSibling")]
    fn next_element_sibling<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        element_sibling_value(&ctx, self.handle.0, true)
    }

    // ── Element identity and attributes ─────────────────────────────────

    // https://dom.spec.whatwg.org/#dom-element-tagname
    #[qjs(get, rename = "tagName")]
    fn tag_name(&self, ctx: Ctx<'_>) -> Result<String> {
        let uppercase = document_is_html_content(&ctx, self.handle.0);
        with_node_kind(&ctx, self.handle.0, |kind| match kind {
            Some(NodeKind::Element { name, .. }) => element_node_name(name, uppercase),
            _ => String::new(),
        })
    }

    // https://dom.spec.whatwg.org/#dom-processinginstruction-target
    // https://html.spec.whatwg.org/multipage/forms.html#dom-form-target
    // One shared wrapper carries both: a processing instruction answers with
    // its target, any other element with its reflected `target` attribute.
    #[qjs(get)]
    fn target(&self, ctx: Ctx<'_>) -> Result<String> {
        let processing_instruction = with_node_kind(&ctx, self.handle.0, |kind| match kind {
            Some(NodeKind::ProcessingInstruction { target, .. }) => Some(target.clone()),
            _ => None,
        })?;
        match processing_instruction {
            Some(target) => Ok(target),
            None => Ok(attribute_value(&ctx, self.handle.0, "target")?),
        }
    }

    // https://dom.spec.whatwg.org/#dom-element-localname
    #[qjs(get, rename = "localName")]
    fn local_name(&self, ctx: Ctx<'_>) -> Result<String> {
        with_node_kind(&ctx, self.handle.0, |kind| match kind {
            Some(NodeKind::Element { name, .. }) => name.local.to_string(),
            _ => String::new(),
        })
    }

    // https://dom.spec.whatwg.org/#dom-element-prefix
    #[qjs(get)]
    fn prefix<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let prefix = with_node_kind(&ctx, self.handle.0, |kind| match kind {
            Some(NodeKind::Element { name, .. }) => name
                .prefix
                .as_ref()
                .filter(|prefix| !prefix.is_empty())
                .map(ToString::to_string),
            _ => None,
        })?;
        match prefix {
            Some(prefix) => string_value(&ctx, &prefix),
            None => Ok(Value::new_null(ctx)),
        }
    }

    // https://dom.spec.whatwg.org/#dom-element-namespaceuri
    #[qjs(get, rename = "namespaceURI")]
    fn namespace_uri<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let namespace = with_node_kind(&ctx, self.handle.0, |kind| match kind {
            Some(NodeKind::Element { name, .. }) => {
                (!name.ns.is_empty()).then(|| name.ns.to_string())
            }
            _ => None,
        })?;
        match namespace {
            Some(namespace) => string_value(&ctx, &namespace),
            None => Ok(Value::new_null(ctx)),
        }
    }

    // https://dom.spec.whatwg.org/#dom-element-classname
    #[qjs(get, rename = "className")]
    fn class_name(&self, ctx: Ctx<'_>) -> Result<String> {
        attribute_value(&ctx, self.handle.0, "class")
    }

    #[qjs(set, rename = "className")]
    fn set_class_name(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        parsed
            .dom
            .set_attribute(self.handle.0, "class", value.0)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        drop(parsed);
        drop(world);
        schedule_mutation_delivery(&ctx)
    }

    // https://dom.spec.whatwg.org/#dom-element-classlist
    #[qjs(get, rename = "classList")]
    fn class_list<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world_rc = world(&ctx)?;
        if let Some(saved) = world_rc.borrow().wrapper(self.handle.0, Wrapper::TokenList)
            && let Some(value) = deref_weak(&ctx, saved)?
        {
            return Ok(value);
        }
        let class = Class::instance(
            ctx.clone(),
            JsTokenList {
                element: self.handle,
            },
        )?;
        let raw = Class::into_value(class);
        // Indexed access (`classList[0]`) goes through the shared proxy.
        let proxy: Function = ctx.globals().get("__tb_liveCollection")?;
        let value: Value = proxy.call((raw,))?;
        let weak = make_weak(&ctx, value.clone())?;
        world_rc.borrow_mut().intern_wrapper(
            self.handle.0,
            Wrapper::TokenList,
            Persistent::save(&ctx, weak),
        );
        Ok(value)
    }

    // https://html.spec.whatwg.org/multipage/dom.html#concept-domstringmap-pairs
    #[qjs(get)]
    fn dataset<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world_rc = world_for_node(&ctx, self.handle.0)?;
        if let Some(saved) = world_rc.borrow().wrapper(self.handle.0, Wrapper::Dataset)
            && let Some(value) = deref_weak(&ctx, saved)?
        {
            return Ok(value);
        }
        let factory: Function = ctx.globals().get("__tbMakeDataset")?;
        let element = wrap_node(&ctx, self.handle.0)?;
        let value: Value = factory.call((element,))?;
        let weak = make_weak(&ctx, value.clone())?;
        world_rc.borrow_mut().intern_wrapper(
            self.handle.0,
            Wrapper::Dataset,
            Persistent::save(&ctx, weak),
        );
        Ok(value)
    }

    // https://dom.spec.whatwg.org/#dom-element-hasattribute
    #[qjs(rename = "hasAttribute")]
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
        Ok(parsed.dom.has_attribute(self.handle.0, &local))
    }

    // https://dom.spec.whatwg.org/#dom-element-getattributens
    #[qjs(rename = "getAttributeNS")]
    fn get_attribute_ns<'js>(
        &self,
        ctx: Ctx<'js>,
        namespace: OptString,
        local: WebIdlString,
    ) -> Result<Value<'js>> {
        let namespace = namespace.0.unwrap_or_default();
        let world = world(&ctx)?;
        let found = world
            .borrow()
            .document(self.handle.0)
            .and_then(|parsed| parsed.dom.attribute_ns(self.handle.0, &namespace, &local.0));
        match found {
            Some(value) => string_value(&ctx, &value),
            None => Ok(Value::new_null(ctx)),
        }
    }

    // https://dom.spec.whatwg.org/#dom-element-hasattributens
    #[qjs(rename = "hasAttributeNS")]
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
            .dom
            .attribute_ns(self.handle.0, &namespace, &local.0)
            .is_some())
    }

    // https://dom.spec.whatwg.org/#dom-element-setattributens
    #[qjs(rename = "setAttributeNS")]
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
            parsed
                .dom
                .set_attribute_by_ns(
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
    #[qjs(rename = "removeAttribute")]
    fn remove_attribute(&self, ctx: Ctx<'_>, name: WebIdlString) -> Result<()> {
        if !valid_attribute_local_name(&name.0) {
            return Ok(());
        }
        let local = attribute_local_name(&ctx, self.handle.0, &name.0);
        {
            let world = world(&ctx)?;
            let world = world.borrow();
            let Some(mut parsed) = world.document_mut(self.handle.0) else {
                return Ok(());
            };
            parsed
                .dom
                .remove_attribute(self.handle.0, &local)
                .map_err(|err| throw_dom_error(&ctx, err))?;
        }
        detach_attr(&ctx, self.handle.0, "", &local)?;
        schedule_mutation_delivery(&ctx)
    }

    // https://dom.spec.whatwg.org/#dom-element-removeattributens
    #[qjs(rename = "removeAttributeNS")]
    fn remove_attribute_ns(
        &self,
        ctx: Ctx<'_>,
        namespace: OptString,
        local: WebIdlString,
    ) -> Result<()> {
        let namespace = namespace.0.unwrap_or_default();
        {
            let world = world(&ctx)?;
            let world = world.borrow();
            let Some(mut parsed) = world.document_mut(self.handle.0) else {
                return Ok(());
            };
            parsed
                .dom
                .remove_attribute_ns(self.handle.0, &namespace, &local.0)
                .map_err(|err| throw_dom_error(&ctx, err))?;
        }
        detach_attr(&ctx, self.handle.0, &namespace, &local.0)?;
        schedule_mutation_delivery(&ctx)
    }

    // https://dom.spec.whatwg.org/#dom-element-getattributenames
    #[qjs(rename = "getAttributeNames")]
    fn get_attribute_names(&self, ctx: Ctx<'_>) -> Result<Vec<String>> {
        let world = world(&ctx)?;
        Ok(world
            .borrow()
            .document(self.handle.0)
            .map(|parsed| parsed.dom.attribute_names(self.handle.0))
            .unwrap_or_default())
    }

    // https://dom.spec.whatwg.org/#dom-element-attributes
    #[qjs(get)]
    fn attributes<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world_rc = world_for_node(&ctx, self.handle.0)?;
        if let Some(saved) = world_rc
            .borrow()
            .wrapper(self.handle.0, Wrapper::NamedNodeMap)
            && let Some(value) = deref_weak(&ctx, saved)?
        {
            refresh_named_node_map(&ctx, self.handle.0, &value)?;
            return Ok(value);
        }
        let class = Class::instance(
            ctx.clone(),
            JsNamedNodeMap {
                element: self.handle,
            },
        )?;
        let value = Class::into_value(class);
        refresh_named_node_map(&ctx, self.handle.0, &value)?;
        let weak = make_weak(&ctx, value.clone())?;
        world_rc.borrow_mut().intern_wrapper(
            self.handle.0,
            Wrapper::NamedNodeMap,
            Persistent::save(&ctx, weak),
        );
        Ok(value)
    }

    // https://dom.spec.whatwg.org/#dom-element-hasattributes
    #[qjs(rename = "hasAttributes")]
    fn has_attributes(&self, ctx: Ctx<'_>) -> Result<bool> {
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(false);
        };
        Ok(parsed
            .dom
            .attributes(self.handle.0)
            .is_some_and(|list| !list.is_empty()))
    }

    // https://dom.spec.whatwg.org/#dom-element-getattributenode
    #[qjs(rename = "getAttributeNode")]
    fn get_attribute_node<'js>(&self, ctx: Ctx<'js>, name: WebIdlString) -> Result<Value<'js>> {
        if !valid_attribute_local_name(&name.0) {
            return Ok(Value::new_null(ctx));
        }
        let name = attribute_local_name(&ctx, self.handle.0, &name.0);
        match attached_attr_id(&ctx, self.handle.0, "", &name)? {
            Some(id) => attr_wrapper(&ctx, self.handle.0, id),
            None => Ok(Value::new_null(ctx)),
        }
    }

    // https://dom.spec.whatwg.org/#dom-element-getattributenodens
    #[qjs(rename = "getAttributeNodeNS")]
    fn get_attribute_node_ns<'js>(
        &self,
        ctx: Ctx<'js>,
        namespace: OptString,
        local: WebIdlString,
    ) -> Result<Value<'js>> {
        let namespace = namespace.0.unwrap_or_default();
        match attached_attr_id(&ctx, self.handle.0, &namespace, &local.0)? {
            Some(id) => attr_wrapper(&ctx, self.handle.0, id),
            None => Ok(Value::new_null(ctx)),
        }
    }

    // https://dom.spec.whatwg.org/#dom-element-setattributenode
    #[qjs(rename = "setAttributeNode")]
    fn set_attribute_node<'js>(&self, ctx: Ctx<'js>, attr: Value<'js>) -> Result<Value<'js>> {
        set_attribute_node(&ctx, self.handle.0, &attr)
    }

    // https://dom.spec.whatwg.org/#dom-element-setattributenodens
    #[qjs(rename = "setAttributeNodeNS")]
    fn set_attribute_node_ns<'js>(&self, ctx: Ctx<'js>, attr: Value<'js>) -> Result<Value<'js>> {
        set_attribute_node(&ctx, self.handle.0, &attr)
    }

    // https://dom.spec.whatwg.org/#dom-element-removeattributenode
    #[qjs(rename = "removeAttributeNode")]
    fn remove_attribute_node<'js>(&self, ctx: Ctx<'js>, attr: Value<'js>) -> Result<Value<'js>> {
        let class = Class::<JsAttr>::from_js(&ctx, attr.clone())
            .map_err(|_| Exception::throw_type(&ctx, "argument is not an Attr"))?;
        let (id, scope) = {
            let attr = class.borrow();
            (attr.id, attr.scope.0)
        };
        let state = attr_state(&ctx, scope, id)?;
        let attached_here = attr_owner(&ctx, scope, id) == Some(self.handle.0) && {
            let world_rc = world_for_node(&ctx, self.handle.0)?;
            let world = world_rc.borrow();
            world
                .attr_ids
                .get(&(self.handle.0, state.namespace.clone(), state.local.clone()))
                == Some(&id)
        };
        if !attached_here {
            return Err(throw_dom(
                &ctx,
                "NotFoundError",
                "attribute is not attached to this element",
            ));
        }
        remove_attribute_sync(&ctx, self.handle.0, &state.namespace, &state.local, true)?;
        Ok(attr)
    }

    // https://dom.spec.whatwg.org/#dom-element-toggleattribute
    #[qjs(rename = "toggleAttribute")]
    fn toggle_attribute(&self, ctx: Ctx<'_>, name: WebIdlString, force: Opt<bool>) -> Result<bool> {
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
            let exists = parsed.dom.has_attribute(self.handle.0, &local);
            let should_exist = force.0.unwrap_or(!exists);
            if should_exist && !exists {
                parsed
                    .dom
                    .set_attribute(self.handle.0, &local, String::new())
                    .map_err(|err| throw_dom_error(&ctx, err))?;
            } else if !should_exist && exists {
                parsed
                    .dom
                    .remove_attribute(self.handle.0, &local)
                    .map_err(|err| throw_dom_error(&ctx, err))?;
            }
            (should_exist, should_exist != exists)
        };
        if changed {
            if should_exist {
                touch_attr(&ctx, self.handle.0, "", &local, "")?;
            } else {
                detach_attr(&ctx, self.handle.0, "", &local)?;
            }
            schedule_mutation_delivery(&ctx)?;
        }
        Ok(should_exist)
    }

    // https://dom.spec.whatwg.org/#dom-node-normalize
    #[qjs(rename = "normalize")]
    fn normalize(&self, ctx: Ctx<'_>) -> Result<()> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        let dom = &mut parsed.dom;
        // A Text node normalizes only itself; containers merge adjacent
        // Text children and drop empty ones.
        if let Some(NodeKind::Text { data }) = dom.kind(self.handle.0) {
            if data.is_empty() {
                dom.detach(self.handle.0)
                    .map_err(|err| throw_dom_error(&ctx, err))?;
            }
            return Ok(());
        }
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
                        dom.detach(kid).map_err(|err| throw_dom_error(&ctx, err))?;
                    }
                    Some(NodeKind::Text { data }) => {
                        if let Some(previous) = merged {
                            let mut joined = match dom.kind(previous) {
                                Some(NodeKind::Text { data }) => data.clone(),
                                _ => String::new(),
                            };
                            joined.push_str(data);
                            dom.set_text(previous, joined)
                                .map_err(|err| throw_dom_error(&ctx, err))?;
                            dom.detach(kid).map_err(|err| throw_dom_error(&ctx, err))?;
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
    #[qjs(rename = "compareDocumentPosition")]
    fn compare_document_position<'js>(&self, ctx: Ctx<'js>, other: Value<'js>) -> Result<u16> {
        const DISCONNECTED: u16 = 1;
        const PRECEDING: u16 = 2;
        const FOLLOWING: u16 = 4;
        const CONTAINS: u16 = 8;
        const CONTAINED_BY: u16 = 16;
        const IMPLEMENTATION_SPECIFIC: u16 = 32;
        let Some(other) = host_node_id(&ctx, &other) else {
            return Err(Exception::throw_type(&ctx, "argument is not a Node"));
        };
        let a = self.handle.0;
        if a == other {
            return Ok(0);
        }
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(DISCONNECTED | IMPLEMENTATION_SPECIFIC);
        };
        let dom = &parsed.dom;
        if root_of(dom, a) != root_of(dom, other) {
            return Ok(DISCONNECTED | IMPLEMENTATION_SPECIFIC);
        }
        let ancestors_a = ancestor_chain(dom, a);
        let ancestors_b = ancestor_chain(dom, other);
        if ancestors_a.contains(&other) {
            return Ok(CONTAINED_BY | FOLLOWING);
        }
        if ancestors_b.contains(&a) {
            return Ok(CONTAINS | PRECEDING);
        }
        Ok(if tree_order(dom, a, other) == std::cmp::Ordering::Less {
            PRECEDING
        } else {
            FOLLOWING
        })
    }

    // https://dom.spec.whatwg.org/#dom-node-parentelement
    #[qjs(get, rename = "parentElement")]
    fn parent_element<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world_rc = world(&ctx)?;
        let parent = {
            let parsed = world_rc.borrow();
            let Some(parsed) = parsed.document(self.handle.0) else {
                return Ok(Value::new_null(ctx));
            };
            parsed
                .dom
                .parent(self.handle.0)
                .filter(|&parent| is_element(&parsed.dom, parent))
        };
        child_value(&ctx, parent)
    }

    // https://dom.spec.whatwg.org/#dom-node-issamenode
    #[qjs(rename = "isSameNode")]
    fn is_same_node<'js>(&self, ctx: Ctx<'js>, other: Value<'js>) -> bool {
        host_node_id(&ctx, &other) == Some(self.handle.0)
    }

    // https://dom.spec.whatwg.org/#dom-node-isequalnode
    #[qjs(rename = "isEqualNode")]
    fn is_equal_node<'js>(&self, ctx: Ctx<'js>, other: Value<'js>) -> Result<bool> {
        let Some(other) = host_node_id(&ctx, &other) else {
            return Ok(false);
        };
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(false);
        };
        Ok(nodes_equal(&parsed.dom, self.handle.0, other))
    }

    // https://dom.spec.whatwg.org/#dom-node-contains
    #[qjs(rename = "contains")]
    fn contains<'js>(&self, ctx: Ctx<'js>, other: Value<'js>) -> Result<bool> {
        let Some(other) = host_node_id(&ctx, &other) else {
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
            cursor = parsed.dom.parent(id);
        }
        Ok(false)
    }

    // https://dom.spec.whatwg.org/#dom-node-getrootnode
    #[qjs(rename = "getRootNode")]
    fn get_root_node<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let mut root = self.handle.0;
        {
            let parsed = world.borrow();
            let Some(parsed) = parsed.document(self.handle.0) else {
                return Ok(Value::new_null(ctx));
            };
            while let Some(parent) = parsed.dom.parent(root) {
                root = parent;
            }
        }
        wrap_node(&ctx, root)
    }

    // https://dom.spec.whatwg.org/#dom-node-isconnected
    #[qjs(get, rename = "isConnected")]
    fn is_connected(&self, ctx: Ctx<'_>) -> Result<bool> {
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(false);
        };
        Ok(parsed.dom.is_connected(self.handle.0))
    }

    // https://dom.spec.whatwg.org/#dom-node-clonenode
    #[qjs(rename = "cloneNode")]
    fn clone_node<'js>(&self, ctx: Ctx<'js>, deep: Opt<bool>) -> Result<Value<'js>> {
        let deep = deep.0.unwrap_or(false);
        let world_rc = world(&ctx)?;
        let is_document = {
            let world = world_rc.borrow();
            let Some(parsed) = world.document(self.handle.0) else {
                return Err(Exception::throw_type(&ctx, "no document"));
            };
            self.handle.0 == parsed.dom.document()
        };
        if is_document {
            return clone_document(&ctx, self.handle.0, deep);
        }
        let world = world_rc.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        let clone = parsed
            .dom
            .clone_node(self.handle.0, deep)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        drop(parsed);
        drop(world);
        wrap_node(&ctx, clone)
    }

    // https://dom.spec.whatwg.org/#dom-node-insertbefore
    #[qjs(rename = "insertBefore")]
    fn insert_before<'js>(
        &self,
        ctx: Ctx<'js>,
        node: Value<'js>,
        child: Value<'js>,
    ) -> Result<Value<'js>> {
        let node = required_node(&ctx, &node)?;
        let reference = optional_node(&ctx, &child)?;
        {
            let world = world(&ctx)?;
            let parsed = world.borrow();
            let Some(parsed) = parsed.document(self.handle.0) else {
                return Err(Exception::throw_type(&ctx, "no document"));
            };
            parsed
                .dom
                .validate_pre_insert(self.handle.0, node, reference)
                .map_err(|err| throw_dom_error(&ctx, err))?;
        }
        let node = adopt_across_documents(&ctx, self.handle.0, node)?;
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        let dom = &mut parsed.dom;
        dom.pre_insert(self.handle.0, node, reference)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        drop(parsed);
        drop(world);
        schedule_mutation_delivery(&ctx)?;
        wrap_node(&ctx, node)
    }

    // https://dom.spec.whatwg.org/#dom-node-removechild
    #[qjs(rename = "removeChild")]
    fn remove_child<'js>(&self, ctx: Ctx<'js>, child: Value<'js>) -> Result<Value<'js>> {
        let child = required_node(&ctx, &child)?;
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        if parsed.dom.parent(child) != Some(self.handle.0) {
            return Err(throw_dom(
                &ctx,
                "NotFoundError",
                "child is not a child of this node",
            ));
        }
        parsed
            .dom
            .detach(child)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        drop(parsed);
        drop(world);
        fixup_focus_after_removal(&ctx, child)?;
        schedule_mutation_delivery(&ctx)?;
        wrap_node(&ctx, child)
    }

    // https://dom.spec.whatwg.org/#dom-node-replacechild
    #[qjs(rename = "replaceChild")]
    fn replace_child<'js>(
        &self,
        ctx: Ctx<'js>,
        node: Value<'js>,
        child: Value<'js>,
    ) -> Result<Value<'js>> {
        let node = required_node(&ctx, &node)?;
        let child = required_node(&ctx, &child)?;
        {
            let world = world(&ctx)?;
            let parsed = world.borrow();
            let Some(parsed) = parsed.document(self.handle.0) else {
                return Err(Exception::throw_type(&ctx, "no document"));
            };
            parsed
                .dom
                .validate_pre_insert(self.handle.0, node, Some(child))
                .map_err(|err| throw_dom_error(&ctx, err))?;
        }
        let node = adopt_across_documents(&ctx, self.handle.0, node)?;
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        parsed
            .dom
            .replace_child(self.handle.0, node, child)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        drop(parsed);
        drop(world);
        fixup_focus_after_removal(&ctx, child)?;
        schedule_mutation_delivery(&ctx)?;
        wrap_node(&ctx, child)
    }

    // https://dom.spec.whatwg.org/#dom-node-lookupnamespaceuri
    #[qjs(rename = "lookupNamespaceURI")]
    fn lookup_namespace_uri<'js>(&self, ctx: Ctx<'js>, prefix: OptString) -> Result<Value<'js>> {
        let prefix = prefix.0.filter(|value| !value.is_empty());
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(Value::new_null(ctx));
        };
        match locate_namespace(&parsed.dom, self.handle.0, prefix.as_deref()) {
            Some(namespace) => string_value(&ctx, &namespace),
            None => Ok(Value::new_null(ctx)),
        }
    }

    // https://dom.spec.whatwg.org/#dom-node-lookupprefix
    #[qjs(rename = "lookupPrefix")]
    fn lookup_prefix<'js>(&self, ctx: Ctx<'js>, namespace: OptString) -> Result<Value<'js>> {
        let Some(namespace) = namespace.0.filter(|value| !value.is_empty()) else {
            return Ok(Value::new_null(ctx));
        };
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(Value::new_null(ctx));
        };
        match locate_prefix(&parsed.dom, self.handle.0, &namespace) {
            Some(prefix) => string_value(&ctx, &prefix),
            None => Ok(Value::new_null(ctx)),
        }
    }

    // https://dom.spec.whatwg.org/#dom-node-isdefaultnamespace
    #[qjs(rename = "isDefaultNamespace")]
    fn is_default_namespace(&self, ctx: Ctx<'_>, namespace: OptString) -> Result<bool> {
        let namespace = namespace.0.filter(|value| !value.is_empty());
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(false);
        };
        let found = locate_namespace(&parsed.dom, self.handle.0, None);
        Ok(found.as_deref() == namespace.as_deref())
    }

    // https://dom.spec.whatwg.org/#dom-childnode-remove
    #[qjs(rename = "remove")]
    fn remove(&self, ctx: Ctx<'_>) -> Result<()> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        parsed
            .dom
            .detach(self.handle.0)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        drop(parsed);
        drop(world);
        fixup_focus_after_removal(&ctx, self.handle.0)?;
        schedule_mutation_delivery(&ctx)
    }

    // https://dom.spec.whatwg.org/#dom-characterdata-length
    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-length
    // One shared wrapper carries both: a character-data node answers with its
    // data length, a `select` with its option count.
    #[qjs(get, rename = "length")]
    fn length(&self, ctx: Ctx<'_>) -> Result<usize> {
        let character_data_node = with_node_kind(&ctx, self.handle.0, |kind| {
            matches!(
                kind,
                Some(
                    NodeKind::Text { .. }
                        | NodeKind::Comment { .. }
                        | NodeKind::CDataSection { .. }
                        | NodeKind::ProcessingInstruction { .. }
                )
            )
        })?;
        if character_data_node {
            return Ok(character_data(&ctx, self.handle.0)?.encode_utf16().count());
        }
        let world = world(&ctx)?;
        let world = world.borrow();
        Ok(world
            .document(self.handle.0)
            .map_or(0, |parsed| parsed.dom.select_options(self.handle.0).len()))
    }

    // https://dom.spec.whatwg.org/#dom-characterdata-substringdata
    #[qjs(rename = "substringData")]
    fn substring_data(
        &self,
        ctx: Ctx<'_>,
        offset: WebIdlUnsignedLong,
        count: WebIdlUnsignedLong,
    ) -> Result<String> {
        let data = character_data(&ctx, self.handle.0)?;
        let units: Vec<u16> = data.encode_utf16().collect();
        let offset = character_data_offset(&ctx, offset.0, units.len())?;
        let end = offset
            .saturating_add(usize::try_from(count.0).unwrap_or(usize::MAX))
            .min(units.len());
        Ok(String::from_utf16_lossy(&units[offset..end]))
    }

    // https://dom.spec.whatwg.org/#dom-characterdata-appenddata
    #[qjs(rename = "appendData")]
    fn append_data(&self, ctx: Ctx<'_>, data: WebIdlString) -> Result<()> {
        let mut current = character_data(&ctx, self.handle.0)?;
        current.push_str(&data.0);
        set_character_data(&ctx, self.handle.0, current)
    }

    // https://dom.spec.whatwg.org/#dom-characterdata-insertdata
    #[qjs(rename = "insertData")]
    fn insert_data(
        &self,
        ctx: Ctx<'_>,
        offset: WebIdlUnsignedLong,
        data: WebIdlString,
    ) -> Result<()> {
        let current = character_data(&ctx, self.handle.0)?;
        let mut units: Vec<u16> = current.encode_utf16().collect();
        let offset = character_data_offset(&ctx, offset.0, units.len())?;
        let insert: Vec<u16> = data.0.encode_utf16().collect();
        units.splice(offset..offset, insert);
        set_character_data(&ctx, self.handle.0, String::from_utf16_lossy(&units))
    }

    // https://dom.spec.whatwg.org/#dom-characterdata-deletedata
    #[qjs(rename = "deleteData")]
    fn delete_data(
        &self,
        ctx: Ctx<'_>,
        offset: WebIdlUnsignedLong,
        count: WebIdlUnsignedLong,
    ) -> Result<()> {
        let current = character_data(&ctx, self.handle.0)?;
        let mut units: Vec<u16> = current.encode_utf16().collect();
        let offset = character_data_offset(&ctx, offset.0, units.len())?;
        let end = offset
            .saturating_add(usize::try_from(count.0).unwrap_or(usize::MAX))
            .min(units.len());
        units.drain(offset..end);
        set_character_data(&ctx, self.handle.0, String::from_utf16_lossy(&units))
    }

    // https://dom.spec.whatwg.org/#dom-characterdata-replacedata
    #[qjs(rename = "replaceData")]
    fn replace_data(
        &self,
        ctx: Ctx<'_>,
        offset: WebIdlUnsignedLong,
        count: WebIdlUnsignedLong,
        data: WebIdlString,
    ) -> Result<()> {
        let current = character_data(&ctx, self.handle.0)?;
        let mut units: Vec<u16> = current.encode_utf16().collect();
        let offset = character_data_offset(&ctx, offset.0, units.len())?;
        let end = offset
            .saturating_add(usize::try_from(count.0).unwrap_or(usize::MAX))
            .min(units.len());
        units.splice(offset..end, data.0.encode_utf16());
        set_character_data(&ctx, self.handle.0, String::from_utf16_lossy(&units))
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
