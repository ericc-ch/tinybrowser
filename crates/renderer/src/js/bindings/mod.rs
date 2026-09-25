//! Platform objects for DOM nodes, one module per concern.
//!
//! `JsNode` is the single wrapper class for every node; the JS interface
//! prototypes copy its members, so the Rust class cannot split across
//! modules (`#[rquickjs::methods]` emits one `MethodImplementor` impl per
//! type). Its members stay in `node`.
use rquickjs::function::Constructor;

macro_rules! branded_node {
    ($name:ident, $js:literal) => {
        #[derive(Trace, rquickjs::JsLifetime)]
        #[rquickjs::class(rename = $js)]
        pub(crate) struct $name {
            pub(crate) handle: Handle,
        }
    };
}

mod attributes;
mod clone;
mod collections;
mod document;
mod exceptions;
mod focus;
mod messaging;
mod mutation;
mod node;
mod parsing;
mod webidl;
mod window;

pub(super) use attributes::*;
pub(super) use clone::*;
pub(super) use collections::*;
pub(super) use document::*;
pub(super) use exceptions::*;
pub(super) use focus::*;
pub(super) use messaging::*;
pub(super) use mutation::*;
pub(super) use node::*;
pub(super) use parsing::*;
pub(super) use webidl::*;
pub(super) use window::*;

use std::cell::RefCell;

use std::collections::HashMap;

use std::rc::{Rc, Weak};

use dom::{
    DomError, LocalName, Namespace, NodeId, NodeKind, Prefix, QualName, html_namespace,
    qualified_name_eq, svg_namespace,
};

use rquickjs::{
    Class, Ctx, Exception, FromJs, Function, Object, Persistent, Result, Symbol, Value,
    class::Trace, prelude::This,
};

use super::events::{self, JsEvent, JsEventTarget};

use super::world::{EventTargetKey, Handle, World};

thread_local! {
    /// JS world per live realm, keyed by its QuickJS context pointer.
    ///
    /// Runtime userdata would be one slot per renderer process, and the second
    /// frame would overwrite the first; each realm needs its own. One renderer
    /// thread hosts every frame, and entries are removed when the realm drops,
    /// so the map is thread-local and keyed by pointer identity.
    static REALM_WORLDS: RefCell<HashMap<usize, Weak<RefCell<World>>>> =
        RefCell::new(HashMap::new());
}

/// Remembers `world` as the JS world of the realm behind `ctx`.
fn register_world(ctx: &Ctx<'_>, world: &Rc<RefCell<World>>) {
    REALM_WORLDS.with(|worlds| {
        worlds
            .borrow_mut()
            .insert(ctx.as_raw().as_ptr() as usize, Rc::downgrade(world));
    });
}

/// Forgets the realm behind `context`; called when its realm is dropped.
pub(crate) fn forget_world(context: &rquickjs::Context) {
    REALM_WORLDS.with(|worlds| {
        worlds
            .borrow_mut()
            .remove(&(context.as_raw().as_ptr() as usize));
    });
}

/// The document of the current realm's global object.
pub(crate) fn main_document(ctx: &Ctx<'_>) -> Result<NodeId> {
    world(ctx)?
        .borrow()
        .with_main_document(|parsed| parsed.dom.document())
        .ok_or_else(|| Exception::throw_type(ctx, "no document"))
}

/// Builds and throws a `DOMException` from Rust with a real prototype, so
/// `instanceof DOMException` and `constructor` checks pass.
pub(crate) fn throw_dom(ctx: &Ctx<'_>, name: &str, message: &str) -> rquickjs::Error {
    match Class::instance(
        ctx.clone(),
        JsDomException {
            name: name.into(),
            message: message.into(),
        },
    ) {
        Ok(exception) => ctx.throw(Class::into_value(exception)),
        Err(err) => err,
    }
}

/// Maps a refused DOM mutation onto its exception class
/// (<https://dom.spec.whatwg.org/#dom-domerror> naming).
pub(crate) fn throw_dom_error(ctx: &Ctx<'_>, err: DomError) -> rquickjs::Error {
    match err {
        DomError::CycleForbidden | DomError::HierarchyRequest => {
            throw_dom(ctx, "HierarchyRequestError", &err.to_string())
        }
        DomError::NoParent => throw_dom(ctx, "NotFoundError", &err.to_string()),
        // Programming errors, not web-visible DOM exceptions.
        DomError::StaleNode | DomError::WrongNodeType => {
            Exception::throw_type(ctx, &err.to_string())
        }
    }
}

impl<'js> rquickjs::FromJs<'js> for OptString {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
        if value.is_null() || value.is_undefined() {
            return Ok(Self(None));
        }
        Ok(Self(Some(webidl_to_string(ctx, value)?)))
    }
}

impl<'js> rquickjs::FromJs<'js> for WebIdlString {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
        Ok(Self(webidl_to_string(ctx, value)?))
    }
}

impl<'js> rquickjs::FromJs<'js> for LegacyNullString {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
        if value.is_null() {
            return Ok(Self(String::new()));
        }
        Ok(Self(webidl_to_string(ctx, value)?))
    }
}

impl<'js> rquickjs::FromJs<'js> for OptionalTitle {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
        if value.is_undefined() {
            return Ok(Self(None));
        }
        Ok(Self(Some(webidl_to_string(ctx, value)?)))
    }
}

impl<'js> rquickjs::FromJs<'js> for WebIdlUnsignedLong {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
        // The pristine `Number`, captured at install: a page-assigned global
        // must not hijack `unsigned long` conversion. Conversion errors
        // propagate; the clobberable global is only a fallback when install
        // predates the capture.
        let pristine = world(ctx)?.borrow().pristine_number.clone();
        if let Some(pristine) = pristine {
            let to_number: Function = pristine.restore(ctx)?;
            let number: f64 = to_number.call((value.clone(),))?;
            return Ok(Self(webidl_unsigned_long(number)));
        }
        let to_number: Function = ctx.globals().get("Number")?;
        let number: f64 = to_number.call((value,))?;
        Ok(Self(webidl_unsigned_long(number)))
    }
}

pub(super) fn handler_target<'js>(ctx: &Ctx<'js>, node: &Value<'js>) -> Result<NodeId> {
    host_node_id(ctx, node)
        .ok_or_else(|| Exception::throw_type(ctx, "event handler target is not a node"))
}

pub(crate) fn install(ctx: &Ctx<'_>, world: &Rc<RefCell<World>>) -> Result<()> {
    register_world(ctx, world);
    let globals = ctx.globals();
    world
        .borrow_mut()
        .set_window(Persistent::save(ctx, globals.clone()));
    Class::<JsEvent>::define(&globals)?;
    ctx.eval::<(), _>(events::INSTALL_EVENT_CTOR_JS)?;
    install_webdriver_bridge(ctx, &globals)?;
    globals.set("innerWidth", f64::from(crate::engine::VIEWPORT_WIDTH))?;
    globals.set("innerHeight", f64::from(crate::engine::VIEWPORT_HEIGHT))?;
    // No browser chrome exists, so the outer window equals the inner viewport
    // (<https://drafts.csswg.org/cssom-view/#dom-window-outerwidth>).
    globals.set("outerWidth", f64::from(crate::engine::VIEWPORT_WIDTH))?;
    globals.set("outerHeight", f64::from(crate::engine::VIEWPORT_HEIGHT))?;
    globals.set(
        "__tb_new_custom_event",
        rquickjs::prelude::Func::from(events::construct_custom_event),
    )?;
    globals.set(
        "__tb_init_custom_event",
        rquickjs::prelude::Func::from(events::init_custom_event),
    )?;
    ctx.eval::<(), _>(events::INSTALL_CUSTOM_EVENT_JS)?;
    Class::<JsEventTarget>::define(&globals)?;
    ctx.eval::<(), _>(events::INSTALL_EVENT_TARGET_CTOR_JS)?;
    Class::<JsNode>::define(&globals)?;
    Class::<JsCollection>::define(&globals)?;
    Class::<JsDomException>::define(&globals)?;
    inherit_error_prototype(&globals)?;
    ctx.eval::<(), _>(events::INSTALL_ABORT_JS)?;
    Class::<JsImplementation>::define(&globals)?;
    Class::<JsTokenList>::define(&globals)?;
    Class::<JsAttr>::define(&globals)?;
    Class::<JsNamedNodeMap>::define(&globals)?;
    Class::<JsDomParser>::define(&globals)?;
    ctx.eval::<(), _>(parsing::INSTALL_DOMPARSER_CTOR_JS)?;
    Class::<JsXmlSerializer>::define(&globals)?;
    Class::<JsMutationObserver>::define(&globals)?;
    Class::<JsMutationRecord>::define(&globals)?;
    globals.set(
        "__tb_deliver_mutations",
        rquickjs::prelude::Func::from(deliver_mutations),
    )?;
    globals.set(
        "__tb_construct",
        rquickjs::prelude::Func::from(construct_node),
    )?;
    node::install_custom_construction(ctx)?;
    globals.set("__tb_handlerNames", HANDLER_ATTRIBUTES.to_vec())?;
    globals.set(
        "__tbGetNodeHandler",
        rquickjs::prelude::Func::from(get_node_handler),
    )?;
    globals.set(
        "__tbSetNodeHandler",
        rquickjs::prelude::Func::from(set_node_handler),
    )?;
    globals.set(
        "__tbGetWindowHandler",
        rquickjs::prelude::Func::from(get_window_handler),
    )?;
    globals.set(
        "__tbSetWindowHandler",
        rquickjs::prelude::Func::from(set_window_handler),
    )?;
    install_brands(ctx)?;
    install_collection_brand(ctx)?;
    install_dom_exception_codes(ctx)?;

    let document_id = world
        .borrow()
        .with_main_document(|parsed| parsed.dom.document());
    if let Some(id) = document_id {
        globals.set("document", wrap_node(ctx, id)?)?;
    }

    install_location(ctx, &globals, world)?;
    globals.set("window", globals.clone())?;
    globals.set("self", globals.clone())?;
    // https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-window-event
    globals.set("event", Value::new_undefined(ctx.clone()))?;
    // https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-frames
    globals.set("frames", globals.clone())?;
    globals.set("opener", Value::new_null(ctx.clone()))?;

    globals.set(
        "addEventListener",
        rquickjs::prelude::Func::from(window_add_event_listener),
    )?;
    globals.set(
        "removeEventListener",
        rquickjs::prelude::Func::from(window_remove_event_listener),
    )?;
    globals.set(
        "dispatchEvent",
        rquickjs::prelude::Func::from(window_dispatch_event),
    )?;
    // User-agent delivery for shim-fired events (window.postMessage). Page
    // script must pass the host token our shims close over; without it the
    // bridge throws instead of forging a trusted event.
    globals.set(
        "__tbDispatchTrusted",
        rquickjs::prelude::Func::from(window_dispatch_trusted_event),
    )?;
    // Capture pristine intrinsics and the host token before any page script
    // runs. Conversions and scheduling use these, never `ctx.globals()`,
    // which page script can clobber.
    capture_host_primitives(ctx, &globals, world)?;
    Ok(())
}

/// Captures the pristine intrinsics and host entry points every later lookup
/// must use instead of `ctx.globals()`.
fn capture_host_primitives<'js>(
    ctx: &Ctx<'js>,
    globals: &Object<'js>,
    world: &Rc<RefCell<World>>,
) -> Result<()> {
    let string: Function = globals.get("String")?;
    let number: Function = globals.get("Number")?;
    let boolean: Function = globals.get("Boolean")?;
    let deliver: Function = globals.get("__tb_deliver_mutations")?;
    let token = Symbol::new(ctx.clone())?.into_value();
    globals.set("__tbHostToken", token.clone())?;
    let mut world = world.borrow_mut();
    world.pristine_string = Some(Persistent::save(ctx, string));
    world.pristine_number = Some(Persistent::save(ctx, number));
    world.pristine_boolean = Some(Persistent::save(ctx, boolean));
    world.pristine_queue_microtask = globals
        .get::<_, Function>("queueMicrotask")
        .ok()
        .map(|queue| Persistent::save(ctx, queue));
    world.deliver_mutations_fn = Some(Persistent::save(ctx, deliver));
    world.host_token = Some(Persistent::save(ctx, token));
    Ok(())
}

pub(crate) fn host_node_id<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Option<NodeId> {
    Class::<JsNode>::from_js(ctx, value.clone())
        .ok()
        .map(|node| node.borrow().node_id())
}

/// Rejects trusted-bridge calls that do not carry the host token our shims
/// close over. Page script cannot name the token (install deletes the global
/// after the shims capture it), so only our shims can ask for trusted
/// dispatch; Rust never goes through the global.
pub(crate) fn check_host_token<'js>(ctx: &Ctx<'js>, token: &Value<'js>) -> Result<()> {
    let world_rc = world(ctx)?;
    let owned = world_rc.borrow().host_token.clone();
    match owned {
        Some(expected) => {
            let expected: Value = expected.restore(ctx)?;
            if token == &expected {
                return Ok(());
            }
            Err(Exception::throw_type(ctx, "illegal invocation"))
        }
        // Install predates the token: accept (yesterday's behavior).
        None => Ok(()),
    }
}

/// The `WebDriver` "element send keys" step: focus the element and append
/// `text` to its value, firing a trusted `input` event.
#[allow(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
pub(super) fn webdriver_send_keys<'js>(
    ctx: Ctx<'js>,
    element: Value<'js>,
    text: String,
) -> Result<()> {
    let Some(node) = host_node_id(&ctx, &element) else {
        return Err(Exception::throw_type(&ctx, "not an element"));
    };
    if is_focusable(&ctx, node)? {
        focus_node(&ctx, node)?;
    }
    if !is_text_control(&ctx, node)? {
        return Ok(());
    }
    let value = wrap_node(&ctx, node)?;
    let Some(object) = value.as_object().cloned() else {
        return Ok(());
    };
    let current = match object.get::<_, Value>("value")? {
        value if value.is_undefined() || value.is_null() => String::new(),
        value => value
            .as_string()
            .and_then(|string| string.to_string().ok())
            .unwrap_or_default(),
    };
    object.set("value", format!("{current}{text}"))?;
    events::fire_trusted(&ctx, EventTargetKey::Node(node), "input", true, false)?;
    Ok(())
}

/// Resolves a `WebDriver` element id to its wrapper, or `null` when no node
/// owns the id. Element commands use this so references returned by
/// `execute_script` work as well as Find Element results
/// (<https://w3c.github.io/webdriver/#elements>).
#[allow(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
pub(super) fn webdriver_element(ctx: Ctx<'_>, remote_id: f64) -> Result<Value<'_>> {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "WebDriver element ids are small non-negative integers"
    )]
    let remote = remote_id as u64;
    let world = world(&ctx)?;
    let Some(node) = world.borrow().node_for_remote(remote) else {
        return Ok(Value::new_null(ctx));
    };
    // Only a live, connected element is a valid element reference.
    let valid = world.borrow().document(node).is_some_and(|parsed| {
        parsed.dom.is_connected(node)
            && matches!(parsed.dom.kind(node), Some(NodeKind::Element { .. }))
    });
    if !valid {
        return Ok(Value::new_null(ctx));
    }
    wrap_node(&ctx, node)
}

/// Every element's border box from the render pipeline's layout, in tree
/// order. Anonymous boxes carry `node: None`.
pub(super) fn layout_boxes(ctx: &Ctx<'_>, document: NodeId) -> Result<Vec<crate::render::NodeBox>> {
    let world = world_for_node(ctx, document)?;
    let world = world.borrow();
    let Some(parsed) = world.document(document) else {
        return Ok(Vec::new());
    };
    let sheets = inline_stylesheets(&parsed.dom);
    let options = crate::render::RenderOptions {
        width: crate::engine::VIEWPORT_WIDTH,
        height: crate::engine::VIEWPORT_HEIGHT,
        scale: 1.0,
    };
    Ok(
        crate::render::layout_boxes(&parsed.dom, &sheets, &options, &world.images)
            .unwrap_or_default(),
    )
}

/// Inline `<style>` text in document order. External sheets are not mirrored
/// into script geometry yet, so a page styled only by `<link>` lays out
/// without the author rules.
fn inline_stylesheets(dom: &dom::Dom) -> Vec<String> {
    let mut sheets = Vec::new();
    for node in dom.descendants(dom.document()) {
        let Some(NodeKind::Element { name, .. }) = dom.kind(node) else {
            continue;
        };
        if name.ns != dom::html_namespace() || name.local.as_ref() != "style" {
            continue;
        }
        let mut css = String::new();
        if let Some(children) = dom.children(node) {
            for child in children {
                if let Some(NodeKind::Text { data }) = dom.kind(child) {
                    css.push_str(data);
                }
            }
        }
        if !css.trim().is_empty() {
            sheets.push(css);
        }
    }
    sheets
}

/// `node`'s border box `(left, top, width, height)` from the current layout,
/// if it has one.
pub(super) fn element_box(ctx: &Ctx<'_>, node: NodeId) -> Result<Option<(f64, f64, f64, f64)>> {
    let boxes = layout_boxes(ctx, node)?;
    Ok(boxes
        .into_iter()
        .find(|item| item.node == Some(node))
        .map(|item| {
            (
                f64::from(item.x),
                f64::from(item.y),
                f64::from(item.width),
                f64::from(item.height),
            )
        }))
}

/// The deepest element whose laid-out border box contains the point, if any.
pub(super) fn element_at_point(
    ctx: &Ctx<'_>,
    document: NodeId,
    x: f64,
    y: f64,
) -> Result<Option<NodeId>> {
    let boxes = layout_boxes(ctx, document)?;
    let mut best = None;
    for item in boxes {
        let Some(node) = item.node else {
            continue;
        };
        // Hidden boxes keep their geometry but never win hit testing; the
        // flat pre-order walk still reaches visible descendants.
        if !item.visible {
            continue;
        }
        if x >= f64::from(item.x)
            && x < f64::from(item.x + item.width)
            && y >= f64::from(item.y)
            && y < f64::from(item.y + item.height)
        {
            // Tree order is pre-order, so the last containing box is the
            // deepest one.
            best = Some(node);
        }
    }
    Ok(best)
}

pub(super) fn rect_object<'js>(
    ctx: &Ctx<'js>,
    left: f64,
    top: f64,
    width: f64,
    height: f64,
) -> Result<Object<'js>> {
    let object = Object::new(ctx.clone())?;
    object.set("x", left)?;
    object.set("y", top)?;
    object.set("width", width)?;
    object.set("height", height)?;
    object.set("top", top)?;
    object.set("left", left)?;
    object.set("right", left + width)?;
    object.set("bottom", top + height)?;
    Ok(object)
}

pub(crate) fn wrap_node<'js>(ctx: &Ctx<'js>, id: NodeId) -> Result<Value<'js>> {
    let world_rc = world(ctx)?;
    if let Some(saved) = world_rc.borrow().shared_wrapper(id)
        && let Some(value) = deref_weak(ctx, saved)?
    {
        return Ok(value);
    }
    let value = instantiate_node(ctx, id)?;
    let weak = make_weak(ctx, value.clone())?;
    world_rc
        .borrow()
        .intern_shared_wrapper(id, Persistent::save(ctx, weak));
    Ok(value)
}

/// Publishes `parsed` as a new document of this realm's world and wraps its
/// root.
pub(super) fn wrap_new_document<'js>(ctx: &Ctx<'js>, parsed: crate::Parsed) -> Result<Value<'js>> {
    let world_rc = world(ctx)?;
    let root = world_rc.borrow_mut().add_document(parsed);
    let registry = world_rc.borrow().registry();
    registry
        .borrow_mut()
        .insert_document(root.document_id(), &world_rc);
    wrap_node(ctx, root)
}

fn instantiate_node<'js>(ctx: &Ctx<'js>, id: NodeId) -> Result<Value<'js>> {
    let is_shadow_root = world(ctx)?
        .borrow()
        .document(id)
        .is_some_and(|parsed| parsed.dom.shadow_host(id).is_some());
    let brand = with_node_kind(ctx, id, |kind| match kind {
        Some(NodeKind::Document) => Some(if document_is_html_content(ctx, id) {
            "Document"
        } else {
            "XMLDocument"
        }),
        Some(NodeKind::Element { name, .. }) => Some(element_interface(name)),
        Some(NodeKind::Text { .. }) => Some("Text"),
        Some(NodeKind::CDataSection { .. }) => Some("CDATASection"),
        Some(NodeKind::ProcessingInstruction { .. }) => Some("ProcessingInstruction"),
        Some(NodeKind::Comment { .. }) => Some("Comment"),
        Some(NodeKind::Doctype { .. }) => Some("DocumentType"),
        Some(NodeKind::Fragment) => Some(if is_shadow_root {
            "ShadowRoot"
        } else {
            "DocumentFragment"
        }),
        None => None,
    })?;
    let Some(brand) = brand else {
        return Err(Exception::throw_type(ctx, "stale node"));
    };
    let class = Class::instance(ctx.clone(), JsNode { handle: Handle(id) })?;
    // The wrapper belongs to the realm that owns the node's document, not to
    // the realm that happens to create it first. Its prototypes come from the
    // owner realm, so `instanceof` and `getPrototypeOf` stay realm-correct
    // even when a same-site frame reads another frame's DOM.
    let owner = world(ctx)?.borrow().owner_world(id);
    let proto = match &owner {
        Some(owner) => owner.borrow().brand(brand),
        None => world(ctx)?.borrow().brand(brand),
    };
    if let Some(proto) = proto {
        class.set_prototype(Some(&proto.restore(ctx)?))?;
    }
    Ok(Class::into_value(class))
}

/// The element interface for a qualified name
/// (<https://html.spec.whatwg.org/multipage/dom.html#elements-in-the-dom:html-element>
/// and its SVG counterparts). Names outside the HTML and SVG namespaces keep
/// the base `Element` interface.
fn element_interface(name: &QualName) -> &'static str {
    if name.ns == html_namespace() {
        html_element_interface(name.local.as_ref())
    } else if name.ns == svg_namespace() {
        "SVGElement"
    } else {
        "Element"
    }
}

/// `local` name to element interface
/// (<https://html.spec.whatwg.org/multipage/dom.html#elements-in-the-dom:html-element>).
const ELEMENT_INTERFACES: &[(&str, &str)] = &[
    ("a", "HTMLAnchorElement"),
    ("abbr", "HTMLElement"),
    ("acronym", "HTMLElement"),
    ("address", "HTMLElement"),
    ("area", "HTMLAreaElement"),
    ("article", "HTMLElement"),
    ("aside", "HTMLElement"),
    ("audio", "HTMLAudioElement"),
    ("b", "HTMLElement"),
    ("base", "HTMLBaseElement"),
    ("bdi", "HTMLElement"),
    ("bdo", "HTMLElement"),
    ("bgsound", "HTMLElement"),
    ("big", "HTMLElement"),
    ("blockquote", "HTMLElement"),
    ("body", "HTMLBodyElement"),
    ("br", "HTMLBRElement"),
    ("button", "HTMLButtonElement"),
    ("canvas", "HTMLCanvasElement"),
    ("caption", "HTMLTableCaptionElement"),
    ("center", "HTMLElement"),
    ("cite", "HTMLElement"),
    ("code", "HTMLElement"),
    ("col", "HTMLTableColElement"),
    ("colgroup", "HTMLTableColElement"),
    ("data", "HTMLDataElement"),
    ("datalist", "HTMLDataListElement"),
    ("dd", "HTMLElement"),
    ("del", "HTMLModElement"),
    ("details", "HTMLElement"),
    ("dfn", "HTMLElement"),
    ("dialog", "HTMLDialogElement"),
    ("dir", "HTMLDirectoryElement"),
    ("div", "HTMLDivElement"),
    ("dl", "HTMLDListElement"),
    ("dt", "HTMLElement"),
    ("embed", "HTMLEmbedElement"),
    ("fieldset", "HTMLFieldSetElement"),
    ("figcaption", "HTMLElement"),
    ("figure", "HTMLElement"),
    ("font", "HTMLFontElement"),
    ("footer", "HTMLElement"),
    ("form", "HTMLFormElement"),
    ("frame", "HTMLFrameElement"),
    ("frameset", "HTMLFrameSetElement"),
    ("h1", "HTMLHeadingElement"),
    ("h2", "HTMLHeadingElement"),
    ("h3", "HTMLHeadingElement"),
    ("h4", "HTMLHeadingElement"),
    ("h5", "HTMLHeadingElement"),
    ("h6", "HTMLHeadingElement"),
    ("head", "HTMLHeadElement"),
    ("header", "HTMLElement"),
    ("hgroup", "HTMLElement"),
    ("hr", "HTMLHRElement"),
    ("html", "HTMLHtmlElement"),
    ("i", "HTMLElement"),
    ("iframe", "HTMLIFrameElement"),
    ("img", "HTMLImageElement"),
    ("input", "HTMLInputElement"),
    ("ins", "HTMLModElement"),
    ("isindex", "HTMLElement"),
    ("kbd", "HTMLElement"),
    ("label", "HTMLLabelElement"),
    ("legend", "HTMLLegendElement"),
    ("li", "HTMLLIElement"),
    ("link", "HTMLLinkElement"),
    ("main", "HTMLElement"),
    ("map", "HTMLMapElement"),
    ("mark", "HTMLElement"),
    ("marquee", "HTMLElement"),
    ("meta", "HTMLMetaElement"),
    ("meter", "HTMLMeterElement"),
    ("nav", "HTMLElement"),
    ("nobr", "HTMLElement"),
    ("noframes", "HTMLElement"),
    ("noscript", "HTMLElement"),
    ("object", "HTMLObjectElement"),
    ("ol", "HTMLOListElement"),
    ("optgroup", "HTMLOptGroupElement"),
    ("option", "HTMLOptionElement"),
    ("output", "HTMLOutputElement"),
    ("p", "HTMLParagraphElement"),
    ("param", "HTMLParamElement"),
    ("pre", "HTMLPreElement"),
    ("progress", "HTMLProgressElement"),
    ("q", "HTMLQuoteElement"),
    ("rp", "HTMLElement"),
    ("rt", "HTMLElement"),
    ("ruby", "HTMLElement"),
    ("s", "HTMLElement"),
    ("samp", "HTMLElement"),
    ("script", "HTMLScriptElement"),
    ("section", "HTMLElement"),
    ("select", "HTMLSelectElement"),
    ("small", "HTMLElement"),
    ("source", "HTMLSourceElement"),
    ("slot", "HTMLSlotElement"),
    ("spacer", "HTMLElement"),
    ("span", "HTMLSpanElement"),
    ("strike", "HTMLElement"),
    ("style", "HTMLStyleElement"),
    ("sub", "HTMLElement"),
    ("summary", "HTMLElement"),
    ("sup", "HTMLElement"),
    ("table", "HTMLTableElement"),
    ("tbody", "HTMLTableSectionElement"),
    ("td", "HTMLTableCellElement"),
    ("template", "HTMLTemplateElement"),
    ("textarea", "HTMLTextAreaElement"),
    ("th", "HTMLTableCellElement"),
    ("time", "HTMLTimeElement"),
    ("title", "HTMLTitleElement"),
    ("tr", "HTMLTableRowElement"),
    ("track", "HTMLTrackElement"),
    ("tt", "HTMLElement"),
    ("u", "HTMLElement"),
    ("ul", "HTMLUListElement"),
    ("unknown", "HTMLUnknownElement"),
    ("var", "HTMLElement"),
    ("video", "HTMLVideoElement"),
    ("wbr", "HTMLElement"),
];

fn html_element_interface(local: &str) -> &'static str {
    if let Some((_, interface)) = ELEMENT_INTERFACES.iter().find(|(name, _)| *name == local) {
        return interface;
    }
    if local.contains('-') {
        return "HTMLElement";
    }
    "HTMLUnknownElement"
}

pub(super) fn make_weak<'js>(ctx: &Ctx<'js>, target: Value<'js>) -> Result<Value<'js>> {
    let ctor: Constructor = ctx.globals().get("WeakRef")?;
    ctor.construct((target,))
}

pub(super) fn deref_weak<'js>(
    ctx: &Ctx<'js>,
    saved: Persistent<Value<'static>>,
) -> Result<Option<Value<'js>>> {
    let weak = saved.restore(ctx)?;
    let object = weak
        .as_object()
        .ok_or_else(|| Exception::throw_type(ctx, "weak wrapper"))?;
    let deref: Function = object.get("deref")?;
    let value: Value = deref.call((This(object.clone()),))?;
    if value.is_undefined() {
        Ok(None)
    } else {
        Ok(Some(value))
    }
}

/// Prototype-brand installation script for the browser-realm platform
/// objects; one `define` per `WebIDL` interface.
const INSTALL_BRANDS_JS: &str = include_str!("../scripts/brands.js");

fn install_brands(ctx: &Ctx<'_>) -> Result<()> {
    ctx.eval::<(), _>(INSTALL_BRANDS_JS)?;
    let table: Object = ctx.globals().get("__tb_brandTable")?;
    let entries = table
        .props::<String, Object>()
        .collect::<Result<Vec<(String, Object)>>>()?;
    for (name, proto) in entries {
        world(ctx)?
            .borrow_mut()
            .intern_brand(name, Persistent::save(ctx, proto));
    }
    ctx.eval::<(), _>("delete globalThis.__tb_brandTable")?;
    Ok(())
}

/// Collection and legacy index-property proxies for the browser realm.
pub(super) const INSTALL_COLLECTIONS_JS: &str = include_str!("../scripts/collections.js");

fn class_proto<'js>(ctx: &Ctx<'js>, name: &str) -> Result<Option<Object<'js>>> {
    let Some(saved) = world(ctx)?.borrow().brand(name) else {
        return Ok(None);
    };
    Ok(Some(saved.restore(ctx)?))
}

/// `DOMException` is an exception interface: its interface prototype object's
/// `[[Prototype]]` is `%Error.prototype%`, so `String(exception)` is
/// `"Name: message"` and `instanceof Error` holds
/// (<https://webidl.spec.whatwg.org/#js-DOMException-specialness>).
fn inherit_error_prototype(globals: &Object<'_>) -> Result<()> {
    let constructor: Object = globals.get("DOMException")?;
    let prototype: Object = constructor.get("prototype")?;
    let error: Object = globals.get("Error")?;
    let error_prototype: Object = error.get("prototype")?;
    prototype.set_prototype(Some(&error_prototype))?;
    Ok(())
}

/// `WebIDL` constants appear on the `interface object`, the `interface
/// prototype object`, and the `named constructor`
/// (<https://webidl.spec.whatwg.org/#interface-object>).
fn install_dom_exception_codes(ctx: &Ctx<'_>) -> Result<()> {
    // `DOMException.prototype[@@toStringTag]` is `"DOMException"`, which is
    // also how a structured clone recognizes the interface across realms
    // (<https://webidl.spec.whatwg.org/#idl-DOMException>).
    ctx.eval::<(), _>(
        r"Object.defineProperty(globalThis.DOMException.prototype, Symbol.toStringTag, {
  value: 'DOMException', writable: false, enumerable: false, configurable: true,
});",
    )?;
    let ctor: Object = ctx.globals().get("DOMException")?;
    let proto: Object = ctor.get("prototype")?;
    for (name, _, code) in DOM_EXCEPTION_CODES {
        ctor.set(name, code)?;
        proto.set(name, code)?;
    }
    Ok(())
}

pub(crate) fn world(ctx: &Ctx<'_>) -> Result<Rc<RefCell<World>>> {
    REALM_WORLDS
        .with(|worlds| {
            worlds
                .borrow()
                .get(&(ctx.as_raw().as_ptr() as usize))
                .and_then(Weak::upgrade)
        })
        .ok_or_else(|| Exception::throw_internal(ctx, "missing JS world"))
}

pub(crate) fn world_for_node(ctx: &Ctx<'_>, id: NodeId) -> Result<Rc<RefCell<World>>> {
    let current = world(ctx)?;
    let owner = current.borrow().owner_world(id);
    Ok(owner.unwrap_or(current))
}

pub(super) fn with_node_kind<T>(
    ctx: &Ctx<'_>,
    id: NodeId,
    read: impl FnOnce(Option<&NodeKind>) -> T,
) -> Result<T> {
    let world = world(ctx)?;
    let parsed = world.borrow();
    let Some(parsed) = parsed.document(id) else {
        return Err(Exception::throw_type(ctx, "no document"));
    };
    Ok(read(parsed.dom.kind(id)))
}

pub(crate) fn character_data(ctx: &Ctx<'_>, id: NodeId) -> Result<String> {
    with_node_kind(ctx, id, |kind| match kind {
        Some(
            NodeKind::Text { data }
            | NodeKind::CDataSection { data }
            | NodeKind::ProcessingInstruction { data, .. }
            | NodeKind::Comment { data },
        ) => data.clone(),
        _ => String::new(),
    })
}

/// The value of `id`'s attribute `local`, or the empty string.
pub(super) fn attribute_value(ctx: &Ctx<'_>, id: NodeId, local: &str) -> Result<String> {
    let world = world(ctx)?;
    Ok(world
        .borrow()
        .document(id)
        .and_then(|parsed| parsed.dom.attribute(id, local))
        .unwrap_or_default())
}

/// [Replaces data](https://dom.spec.whatwg.org/#concept-cd-replace) on a
/// `CharacterData` node; other kinds are a silent no-op (`nodeValue` setter).
pub(super) fn set_character_data(ctx: &Ctx<'_>, id: NodeId, data: String) -> Result<()> {
    let world = world(ctx)?;
    let world = world.borrow();
    let Some(mut parsed) = world.document_mut(id) else {
        return Ok(());
    };
    match parsed.dom.kind(id) {
        Some(NodeKind::Text { .. }) => {
            parsed
                .dom
                .set_text(id, data)
                .map_err(|err| throw_dom_error(ctx, err))?;
        }
        Some(NodeKind::CDataSection { .. }) => {
            parsed
                .dom
                .set_cdata_section(id, data)
                .map_err(|err| throw_dom_error(ctx, err))?;
        }
        Some(NodeKind::ProcessingInstruction { .. }) => {
            parsed
                .dom
                .set_processing_instruction(id, data)
                .map_err(|err| throw_dom_error(ctx, err))?;
        }
        Some(NodeKind::Comment { .. }) => {
            parsed
                .dom
                .set_comment(id, data)
                .map_err(|err| throw_dom_error(ctx, err))?;
        }
        _ => return Ok(()),
    }
    drop(parsed);
    drop(world);
    schedule_mutation_delivery(ctx)
}

/// `WebIDL` `unsigned long` offset conversion plus the `CharacterData` bounds
/// check: offsets beyond the data throw `IndexSizeError`
/// (<https://dom.spec.whatwg.org/#concept-cd-substring>).
pub(crate) fn character_data_offset(ctx: &Ctx<'_>, offset: u32, length: usize) -> Result<usize> {
    let offset = usize::try_from(offset).unwrap_or(usize::MAX);
    if offset > length {
        return Err(throw_dom(
            ctx,
            "IndexSizeError",
            "offset is outside the data",
        ));
    }
    Ok(offset)
}

pub(super) fn required_node<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<NodeId> {
    host_node_id(ctx, value).ok_or_else(|| Exception::throw_type(ctx, "argument is not a Node"))
}

pub(super) fn optional_node<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<Option<NodeId>> {
    if value.is_null() || value.is_undefined() {
        Ok(None)
    } else {
        Ok(Some(required_node(ctx, value)?))
    }
}

pub(super) fn child_value<'js>(ctx: &Ctx<'js>, id: Option<NodeId>) -> Result<Value<'js>> {
    match id {
        Some(id) => wrap_node(ctx, id),
        None => Ok(Value::new_null(ctx.clone())),
    }
}

pub(super) fn sibling_value<'js>(ctx: &Ctx<'js>, id: NodeId, forward: bool) -> Result<Value<'js>> {
    let world = world(ctx)?;
    let sibling = world
        .borrow()
        .document(id)
        .and_then(|parsed| parsed.dom.sibling(id, forward));
    child_value(ctx, sibling)
}

/// The nearest element sibling in the given direction
/// (<https://dom.spec.whatwg.org/#dom-nondocumenttypechildnode-nextelementsibling>).
pub(super) fn element_sibling_value<'js>(
    ctx: &Ctx<'js>,
    id: NodeId,
    forward: bool,
) -> Result<Value<'js>> {
    let world = world(ctx)?;
    let found = {
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(id) else {
            return Ok(Value::new_null(ctx.clone()));
        };
        let mut cursor = parsed.dom.sibling(id, forward);
        while let Some(sibling) = cursor {
            if is_element(&parsed.dom, sibling) {
                break;
            }
            cursor = parsed.dom.sibling(sibling, forward);
        }
        cursor
    };
    child_value(ctx, found)
}

pub(super) fn string_value<'js>(ctx: &Ctx<'js>, text: &str) -> Result<Value<'js>> {
    Ok(rquickjs::String::from_str(ctx.clone(), text)?.into_value())
}

/// [Descendant text content](https://dom.spec.whatwg.org/#concept-descendant-text-content):
/// the data of all `Text` descendants in tree order.
///
/// Descends only into elements and fragments: a `Document` or other
/// non-container child contributes nothing, so its subtree is not entered.
pub(super) fn descendant_text(dom: &dom::Dom, id: NodeId) -> String {
    let mut text = String::new();
    let mut stack: Vec<NodeId> = dom
        .children(id)
        .map(Iterator::collect)
        .unwrap_or_default();
    stack.reverse();
    while let Some(current) = stack.pop() {
        match dom.kind(current) {
            Some(NodeKind::Text { data } | NodeKind::CDataSection { data }) => text.push_str(data),
            Some(NodeKind::Element { .. } | NodeKind::Fragment) => {
                if let Some(kids) = dom.children(current) {
                    let mut kids: Vec<NodeId> = kids.collect();
                    kids.reverse();
                    stack.extend(kids);
                }
            }
            _ => {}
        }
    }
    text
}

/// Structural `isEqualNode`
/// (<https://dom.spec.whatwg.org/#concept-node-equals>).
pub(super) fn nodes_equal(dom: &dom::Dom, a: NodeId, b: NodeId) -> bool {
    if a == b {
        return true;
    }
    let (Some(first), Some(second)) = (dom.kind(a), dom.kind(b)) else {
        return false;
    };
    let equal = match (first, second) {
        (NodeKind::Document, NodeKind::Document) | (NodeKind::Fragment, NodeKind::Fragment) => true,
        (
            NodeKind::Doctype {
                name: name_a,
                public_id: public_a,
                system_id: system_a,
            },
            NodeKind::Doctype {
                name: name_b,
                public_id: public_b,
                system_id: system_b,
            },
        ) => name_a == name_b && public_a == public_b && system_a == system_b,
        (
            NodeKind::Element {
                name: name_a,
                attributes: attributes_a,
            },
            NodeKind::Element {
                name: name_b,
                attributes: attributes_b,
            },
        ) => {
            name_a == name_b
                && attributes_a.len() == attributes_b.len()
                && attributes_a
                    .iter()
                    .all(|attribute| attributes_b.iter().any(|candidate| candidate == attribute))
        }
        (NodeKind::Text { data: data_a }, NodeKind::Text { data: data_b })
        | (NodeKind::CDataSection { data: data_a }, NodeKind::CDataSection { data: data_b })
        | (NodeKind::Comment { data: data_a }, NodeKind::Comment { data: data_b }) => {
            data_a == data_b
        }
        (
            NodeKind::ProcessingInstruction {
                target: target_a,
                data: data_a,
            },
            NodeKind::ProcessingInstruction {
                target: target_b,
                data: data_b,
            },
        ) => target_a == target_b && data_a == data_b,
        _ => false,
    };
    if !equal {
        return false;
    }
    let kids_a: Vec<NodeId> = dom.children(a).expect("live node has no child list").collect();
    let kids_b: Vec<NodeId> = dom.children(b).expect("live node has no child list").collect();
    kids_a.len() == kids_b.len()
        && kids_a
            .iter()
            .zip(kids_b.iter())
            .all(|(&first, &second)| nodes_equal(dom, first, second))
}

/// [Locate a namespace](https://dom.spec.whatwg.org/#locate-a-namespace) for
/// `prefix` walking `cursor`'s inclusive ancestors.
pub(super) fn locate_namespace(
    dom: &dom::Dom,
    cursor: NodeId,
    prefix: Option<&str>,
) -> Option<Namespace> {
    let mut cursor = Some(cursor);
    while let Some(id) = cursor {
        if let Some(NodeKind::Element { name, .. }) = dom.kind(id) {
            let actual = name
                .prefix
                .as_ref()
                .map(Prefix::as_ref)
                .filter(|prefix| !prefix.is_empty());
            if !name.ns.is_empty() && actual == prefix {
                return Some(name.ns.clone());
            }
        }
        cursor = dom.parent(id);
    }
    None
}

/// [Locate a namespace prefix](https://dom.spec.whatwg.org/#locate-a-namespace-prefix)
/// for `namespace` walking `cursor`'s inclusive ancestors.
pub(super) fn locate_prefix(dom: &dom::Dom, cursor: NodeId, namespace: &str) -> Option<String> {
    let mut cursor = Some(cursor);
    while let Some(id) = cursor {
        if let Some(NodeKind::Element { name, attributes }) = dom.kind(id) {
            if name.ns.as_ref() == namespace
                && let Some(prefix) = name.prefix.as_ref().filter(|prefix| !prefix.is_empty())
            {
                return Some(prefix.to_string());
            }
            for attribute in attributes {
                if attribute.name.ns.as_ref() == namespace
                    && let Some(prefix) = attribute
                        .name
                        .prefix
                        .as_ref()
                        .filter(|prefix| !prefix.is_empty())
                {
                    return Some(prefix.to_string());
                }
            }
        }
        cursor = dom.parent(id);
    }
    None
}

pub(super) fn create_html_element<'js>(
    ctx: &Ctx<'js>,
    document: NodeId,
    tag: &str,
) -> Result<Value<'js>> {
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

pub(super) fn create_element_named<'js>(
    ctx: &Ctx<'js>,
    document: NodeId,
    name: QualName,
) -> Result<Value<'js>> {
    let is_template = is_html_name(&name, "template");
    let world = world(ctx)?;
    let world = world.borrow();
    let Some(mut parsed) = world.document_mut(document) else {
        return Err(Exception::throw_type(ctx, "no document"));
    };
    let id = parsed.dom.create_element(name, Vec::new());
    if is_template {
        let contents = parsed.dom.create_fragment();
        parsed
            .dom
            .set_template_contents(id, contents)
            .map_err(|err| throw_dom_error(ctx, err))?;
    }
    drop(parsed);
    drop(world);
    wrap_node(ctx, id)
}

pub(super) fn create_kind<'js>(
    ctx: &Ctx<'js>,
    document: NodeId,
    make: impl FnOnce(&mut dom::Dom) -> NodeId,
) -> Result<Value<'js>> {
    let world = world(ctx)?;
    let world = world.borrow();
    let Some(mut parsed) = world.document_mut(document) else {
        return Err(Exception::throw_type(ctx, "no document"));
    };
    let id = make(&mut parsed.dom);
    drop(parsed);
    drop(world);
    wrap_node(ctx, id)
}

/// Which context a qualified name is validated in
/// (<https://dom.spec.whatwg.org/#validate-and-extract> steps 6 and 7).
#[derive(Clone, Copy)]
pub(super) enum NodeContext {
    Attribute,
    Element,
}

/// [Validate and extract](https://dom.spec.whatwg.org/#validate-and-extract)
/// a namespace and qualified name.
pub(super) fn validate_and_extract(
    ctx: &Ctx<'_>,
    namespace: Option<&str>,
    qualified: &str,
    context: NodeContext,
) -> Result<QualName> {
    let namespace = namespace.filter(|namespace| !namespace.is_empty());
    let (prefix, local) = match qualified.split_once(':') {
        Some((prefix, local)) => {
            if !valid_namespace_prefix(prefix) {
                return Err(throw_dom(
                    ctx,
                    "InvalidCharacterError",
                    "invalid namespace prefix",
                ));
            }
            (Some(prefix), local)
        }
        None => (None, qualified),
    };
    let local_valid = match context {
        NodeContext::Attribute => valid_attribute_local_name(local),
        NodeContext::Element => valid_element_local_name(local),
    };
    if !local_valid {
        return Err(throw_dom(
            ctx,
            "InvalidCharacterError",
            "invalid local name",
        ));
    }
    let xml = "http://www.w3.org/XML/1998/namespace";
    let xmlns = "http://www.w3.org/2000/xmlns/";
    if prefix.is_some() && namespace.is_none() {
        return Err(throw_dom(ctx, "NamespaceError", "prefix without namespace"));
    }
    if prefix == Some("xml") && namespace != Some(xml) {
        return Err(throw_dom(
            ctx,
            "NamespaceError",
            "xml prefix with wrong namespace",
        ));
    }
    if (qualified == "xmlns" || prefix == Some("xmlns")) && namespace != Some(xmlns) {
        return Err(throw_dom(
            ctx,
            "NamespaceError",
            "xmlns name with wrong namespace",
        ));
    }
    if namespace == Some(xmlns) && qualified != "xmlns" && prefix != Some("xmlns") {
        return Err(throw_dom(
            ctx,
            "NamespaceError",
            "xmlns namespace without xmlns name",
        ));
    }
    Ok(QualName::new(
        prefix.map(Prefix::from),
        Namespace::from(namespace.unwrap_or("")),
        LocalName::from(local),
    ))
}

/// [Valid namespace prefix](https://dom.spec.whatwg.org/#valid-namespace-prefix).
fn valid_namespace_prefix(prefix: &str) -> bool {
    !prefix.is_empty()
        && !prefix
            .chars()
            .any(|c| is_infra_whitespace(c) || matches!(c, '\0' | '/' | '>'))
}

/// [Valid attribute local name](https://dom.spec.whatwg.org/#valid-attribute-local-name).
pub(super) fn valid_attribute_local_name(local: &str) -> bool {
    !local.is_empty()
        && !local
            .chars()
            .any(|c| is_infra_whitespace(c) || matches!(c, '\0' | '/' | '=' | '>'))
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
            .any(|c| is_infra_whitespace(c) || matches!(c, '\0' | '/' | '>'));
    }
    if first != ':' && first != '_' && u32::from(first) < 0x80 {
        return false;
    }
    name.chars().all(|c| {
        c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | ':' | '_') || u32::from(c) >= 0x80
    })
}

fn is_infra_whitespace(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\u{c}' | '\r' | ' ')
}

pub(super) fn element_node_name(name: &QualName, uppercase: bool) -> String {
    let qualified = qualified_name(name);
    if uppercase && name.ns == html_namespace() {
        qualified.to_ascii_uppercase()
    } else {
        qualified
    }
}

pub(super) fn qualified_name(name: &QualName) -> String {
    match &name.prefix {
        Some(prefix) if !prefix.is_empty() => format!("{prefix}:{}", name.local),
        _ => name.local.to_string(),
    }
}

pub(super) fn elements_by_tag<'js>(
    ctx: &Ctx<'js>,
    scope: NodeId,
    name: &str,
) -> Result<Value<'js>> {
    live_collection(
        ctx,
        scope,
        CollectionKind::ElementsByTag(name.to_owned()),
        Some("HTMLCollection"),
    )
}

pub(super) fn live_collection<'js>(
    ctx: &Ctx<'js>,
    scope: NodeId,
    kind: CollectionKind,
    brand: Option<&str>,
) -> Result<Value<'js>> {
    let class = Class::instance(
        ctx.clone(),
        JsCollection {
            scope: Handle(scope),
            kind,
        },
    )?;
    if let Some(brand) = brand
        && let Some(proto) = class_proto(ctx, brand)?
    {
        class.set_prototype(Some(&proto))?;
    }
    let proxy: Function = ctx.globals().get("__tb_liveCollection")?;
    proxy.call((Class::into_value(class),))
}

pub(super) fn collection_ids(
    ctx: &Ctx<'_>,
    scope: NodeId,
    kind: &CollectionKind,
) -> Result<Vec<NodeId>> {
    let world = world(ctx)?;
    let parsed = world.borrow();
    let Some(parsed) = parsed.document(scope) else {
        return Ok(Vec::new());
    };
    Ok(match kind {
        CollectionKind::Children => parsed
            .dom
            .children(scope)
            .map(Iterator::collect)
            .unwrap_or_default(),
        CollectionKind::ElementChildren => parsed
            .dom
            .children(scope)
            .map(|children| {
                children
                    .filter(|&kid| is_element(&parsed.dom, kid))
                    .collect()
            })
            .unwrap_or_default(),
        CollectionKind::ElementsByTag(name) => collect_by_tag(&parsed.dom, scope, name),
        CollectionKind::ElementsByTagNs { namespace, local } => {
            collect_by_tag_ns(&parsed.dom, scope, namespace, local)
        }
        CollectionKind::ElementsByClass(names) => collect_by_class(&parsed.dom, scope, names),
        CollectionKind::ElementsByName(name) => collect_by_name(&parsed.dom, scope, name),
        CollectionKind::Static(handles) => handles.iter().map(|handle| handle.0).collect(),
    })
}

fn collect_by_tag(dom: &dom::Dom, scope: NodeId, name: &str) -> Vec<NodeId> {
    // In an HTML document, an HTML-namespace element matches the queried
    // name ASCII-lowercased; other elements match the name exactly
    // (<https://dom.spec.whatwg.org/#concept-getelementsbytagname>).
    let lowered = name.to_ascii_lowercase();
    dom.descendants(scope)
        .filter(|&id| {
            let Some(NodeKind::Element { name: qual, .. }) = dom.kind(id) else {
                return false;
            };
            name == "*"
                || if qual.ns == html_namespace() {
                    qualified_name_eq(qual, &lowered)
                } else {
                    qualified_name_eq(qual, name)
                }
        })
        .collect()
}

fn collect_by_name(dom: &dom::Dom, scope: NodeId, name: &str) -> Vec<NodeId> {
    dom.descendants(scope)
        .filter(|&id| is_element(dom, id) && dom.attribute(id, "name").as_deref() == Some(name))
        .collect()
}

fn collect_by_tag_ns(dom: &dom::Dom, scope: NodeId, namespace: &str, local: &str) -> Vec<NodeId> {
    dom.descendants(scope)
        .filter(|&id| {
            matches!(
                dom.kind(id),
                Some(NodeKind::Element { name, .. })
                    if (namespace == "*" || name.ns.as_ref() == namespace)
                        && (local == "*" || name.local.as_ref() == local)
            )
        })
        .collect()
}

fn collect_by_class(dom: &dom::Dom, scope: NodeId, names: &str) -> Vec<NodeId> {
    let wanted: Vec<&str> = names.split_ascii_whitespace().collect();
    // An empty class set matches nothing
    // (<https://dom.spec.whatwg.org/#concept-getelementsbyclassname>).
    if wanted.is_empty() {
        return Vec::new();
    }
    dom.descendants(scope)
        .filter(|&id| {
            if !is_element(dom, id) {
                return false;
            }
            let classes = dom.attribute(id, "class").unwrap_or_default();
            let tokens: Vec<&str> = classes.split_ascii_whitespace().collect();
            wanted.iter().all(|want| tokens.contains(want))
        })
        .collect()
}

pub(super) fn is_element(dom: &dom::Dom, id: NodeId) -> bool {
    matches!(dom.kind(id), Some(NodeKind::Element { .. }))
}

/// Whether `name` is an element in the HTML namespace with local name
/// `local`.
pub(super) fn is_html_name(name: &QualName, local: &str) -> bool {
    name.ns == html_namespace() && name.local.as_ref() == local
}

/// Whether `kind` is an element in the HTML namespace with local name `local`.
pub(super) fn is_html_element(kind: Option<&NodeKind>, local: &str) -> bool {
    matches!(
        kind,
        Some(NodeKind::Element { name, .. }) if is_html_name(name, local)
    )
}

/// Whether `kind` is an HTML `<template>` element.
pub(super) fn is_template_element(kind: Option<&NodeKind>) -> bool {
    is_html_element(kind, "template")
}

/// The root of the tree `id` participates in (itself when detached).
pub(super) fn root_of(dom: &dom::Dom, id: NodeId) -> NodeId {
    let mut root = id;
    while let Some(parent) = dom.parent(root) {
        root = parent;
    }
    root
}

/// `id` followed by its inclusive ancestors, nearest first.
pub(super) fn ancestor_chain(dom: &dom::Dom, id: NodeId) -> Vec<NodeId> {
    let mut chain = vec![id];
    let mut cursor = id;
    while let Some(parent) = dom.parent(cursor) {
        chain.push(parent);
        cursor = parent;
    }
    chain
}

/// Document order of two nodes in one tree
/// (<https://dom.spec.whatwg.org/#concept-tree-order>).
pub(super) fn tree_order(dom: &dom::Dom, a: NodeId, b: NodeId) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let chain_a = ancestor_chain(dom, a);
    let chain_b = ancestor_chain(dom, b);
    let mut common = 0;
    while common < chain_a.len()
        && common < chain_b.len()
        && chain_a[chain_a.len() - 1 - common] == chain_b[chain_b.len() - 1 - common]
    {
        common += 1;
    }
    let Some(parent) = chain_a
        .len()
        .checked_sub(common)
        .and_then(|index| chain_a.get(index))
    else {
        return Ordering::Less;
    };
    let child_a = chain_a[chain_a.len() - 1 - common];
    let child_b = chain_b[chain_b.len() - 1 - common];
    let kids: Vec<NodeId> = dom
        .children(*parent)
        .map(Iterator::collect)
        .unwrap_or_default();
    let position_a = kids.iter().position(|&kid| kid == child_a);
    let position_b = kids.iter().position(|&kid| kid == child_b);
    position_a.cmp(&position_b)
}

pub(super) fn find_element_by_id(dom: &dom::Dom, scope: NodeId, id: &str) -> Option<NodeId> {
    dom.descendants(scope)
        .find(|&node| is_element(dom, node) && dom.attribute(node, "id").as_deref() == Some(id))
}

#[cfg(test)]
mod realm_tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Arc;

    use rquickjs::{Persistent, Value};
    use url::Url;

    use super::{world, wrap_node};
    use crate::document::Stop;
    use crate::js::{JsRealm, SharedJsRuntime, World};
    use crate::messaging::Shared;
    use crate::protocol::{
        BrowserServices, BrowsingContextHost, DialCompletion, DialRequest, FrameId, MessagingHost,
        NetworkHost, StorageHost, StorageKind,
    };

    struct NullServices;

    impl NetworkHost for NullServices {
        fn start_dial(&self, _request: DialRequest, completion: DialCompletion) {
            completion(Err(crate::protocol::DialFailure::Connect));
        }

        fn cookies_for(&self, _url: &Url) -> String {
            String::new()
        }

        fn set_cookie(&self, _value: &str, _url: &Url) {}
    }

    impl StorageHost for NullServices {
        fn storage_get(&self, _kind: StorageKind, _origin: &str, _key: &str) -> Option<String> {
            None
        }

        fn storage_keys(&self, _kind: StorageKind, _origin: &str) -> Vec<String> {
            Vec::new()
        }

        fn storage_set(
            &self,
            _kind: StorageKind,
            _origin: &str,
            _url: &str,
            _key: &str,
            _value: &str,
            _source: FrameId,
        ) -> Result<Option<crate::protocol::StorageChange>, crate::protocol::StorageError> {
            Ok(None)
        }

        fn storage_remove(
            &self,
            _kind: StorageKind,
            _origin: &str,
            _url: &str,
            _key: &str,
            _source: FrameId,
        ) -> Option<crate::protocol::StorageChange> {
            None
        }

        fn storage_clear(
            &self,
            _kind: StorageKind,
            _origin: &str,
            _url: &str,
            _source: FrameId,
        ) -> Option<crate::protocol::StorageChange> {
            None
        }
    }

    impl BrowsingContextHost for NullServices {
        fn window_open(&self, _url: &str, _name: &str, _features: &str) -> Option<u64> {
            None
        }

        fn window_close(&self, _tab: u64) {}

        fn window_opener(&self) -> Option<u64> {
            None
        }

        fn window_post_message(&self, _tab: u64, _payload: &str) {}

        fn remote_session_get(&self, _tab: u64, _origin: &str, _key: &str) -> Option<String> {
            None
        }
    }

    impl MessagingHost for NullServices {
        fn broadcast_post(&self, _origin: &str, _name: &str, _payload: &str, _channel: u64) {}
    }

    fn world_with_document(
        services: &Arc<dyn BrowserServices>,
        documents: &Rc<RefCell<crate::documents::DocumentStore>>,
        registry: &Rc<RefCell<crate::js::RealmRegistry>>,
        url: &str,
        html: &str,
    ) -> Rc<RefCell<World>> {
        let runtime = crate::document::FrameRuntime {
            services: Arc::clone(services),
            js_runtime: SharedJsRuntime::default(),
            wake: Arc::new(tokio::sync::Notify::new()),
            stop: Arc::new(Stop::new()),
            documents: Rc::clone(documents),
            registry: Rc::clone(registry),
            shared: Rc::new(RefCell::new(Shared::default())),
            pending_storage: Rc::new(RefCell::new(Vec::new())),
        };
        let mut world = World::new(Url::parse(url).expect("test url"), FrameId::MAIN, &runtime);
        let id = world.replace_document(crate::parse_html(html));
        let world = Rc::new(RefCell::new(world));
        registry.borrow_mut().insert_document(id, &world);
        registry.borrow_mut().insert_frame(FrameId::MAIN, &world);
        world
    }

    /// A JS-reachable event must not pin its `QuickJS` context past realm
    /// teardown. Before event state moved out of the class, this aborted the
    /// runtime with `JS_FreeRuntime: Assertion 'list_empty(&rt->gc_obj_list)'`.
    #[test]
    fn teardown_with_retained_custom_event() {
        let services: Arc<dyn BrowserServices> = Arc::new(NullServices);
        let shared = SharedJsRuntime::default();
        let stop = Arc::new(Stop::new());
        let documents = Rc::new(RefCell::new(crate::documents::DocumentStore::default()));
        let registry = Rc::new(RefCell::new(crate::js::RealmRegistry::default()));
        let world = world_with_document(
            &services,
            &documents,
            &registry,
            "https://a.test/",
            "<!doctype html><p></p>",
        );
        let realm = JsRealm::new(&shared, world, stop).expect("realm");
        realm
            .eval("window.ev = new CustomEvent('x', {detail: 1})")
            .expect("eval");
        drop(realm);
        drop(shared);
    }

    #[test]
    fn teardown_after_document_lifecycle() {
        let services: Arc<dyn BrowserServices> = Arc::new(NullServices);
        let shared = SharedJsRuntime::default();
        let stop = Arc::new(Stop::new());
        let documents = Rc::new(RefCell::new(crate::documents::DocumentStore::default()));
        let registry = Rc::new(RefCell::new(crate::js::RealmRegistry::default()));
        let world = world_with_document(
            &services,
            &documents,
            &registry,
            "https://a.test/",
            "<!doctype html><p>hello</p>",
        );
        let realm = JsRealm::new(&shared, world, stop).expect("realm");
        realm
            .eval("window.onload = function(){}; document.onreadystatechange = function(){};")
            .expect("eval");
        realm.fire_ready_state_change().expect("readystatechange");
        realm.fire_dom_content_loaded().expect("DOMContentLoaded");
        realm.fire_ready_state_change().expect("readystatechange 2");
        realm.fire_load().expect("load");
        drop(realm);
        drop(shared);
    }

    #[test]
    fn realms_share_a_heap_and_resolve_their_own_world() {
        let services: Arc<dyn BrowserServices> = Arc::new(NullServices);
        let shared = SharedJsRuntime::default();
        let stop = Arc::new(Stop::new());
        let documents = Rc::new(RefCell::new(crate::documents::DocumentStore::default()));
        let registry = Rc::new(RefCell::new(crate::js::RealmRegistry::default()));
        let world_a = world_with_document(
            &services,
            &documents,
            &registry,
            "https://a.test/",
            "<!doctype html><p id=a></p>",
        );
        let world_b = world_with_document(
            &services,
            &documents,
            &registry,
            "https://b.test/",
            "<!doctype html><p id=b></p>",
        );
        let realm_a = JsRealm::new(&shared, world_a.clone(), Arc::clone(&stop)).expect("realm a");
        let realm_b = JsRealm::new(&shared, world_b.clone(), Arc::clone(&stop)).expect("realm b");

        // Each realm resolves its own world; one runtime-wide slot would
        // clobber the first world when the second realm installs.
        for (realm, expected) in [(&realm_a, &world_a), (&realm_b, &world_b)] {
            realm.context.with(|ctx| {
                let resolved = world(&ctx).expect("world");
                assert!(
                    Rc::ptr_eq(&resolved, expected),
                    "realm resolved a foreign world"
                );
            });
        }

        // A function created in realm A is callable in realm B: one heap.
        let add_one = realm_a.context.with(|ctx| {
            let value: Value = super::super::eval_classic(&ctx, "(x) => x + 1").expect("function");
            Persistent::save(&ctx, value)
        });
        realm_b.context.with(|ctx| {
            let value = add_one.restore(&ctx).expect("restore");
            let function = value.into_function().expect("function value");
            let result: i32 = function.call((41,)).expect("call");
            assert_eq!(result, 42);
        });

        // Separate documents stay separate trees.
        realm_a.context.with(|ctx| {
            let mine: bool =
                super::super::eval_classic(&ctx, "document.getElementById('a') !== null")
                    .expect("own tree");
            let theirs: bool =
                super::super::eval_classic(&ctx, "document.getElementById('b') === null")
                    .expect("foreign tree");
            assert!(mine && theirs);
        });
    }

    #[test]
    fn wrappers_are_shared_with_the_owner_realms_prototypes() {
        let services: Arc<dyn BrowserServices> = Arc::new(NullServices);
        let shared = SharedJsRuntime::default();
        let stop = Arc::new(Stop::new());
        let documents = Rc::new(RefCell::new(crate::documents::DocumentStore::default()));
        let registry = Rc::new(RefCell::new(crate::js::RealmRegistry::default()));
        let world_a = world_with_document(
            &services,
            &documents,
            &registry,
            "https://a.test/",
            "<!doctype html><p id=a></p>",
        );
        let world_b = world_with_document(
            &services,
            &documents,
            &registry,
            "https://b.test/",
            "<!doctype html><p id=b></p>",
        );
        let realm_a = JsRealm::new(&shared, world_a, Arc::clone(&stop)).expect("realm a");
        let realm_b = JsRealm::new(&shared, world_b.clone(), Arc::clone(&stop)).expect("realm b");
        let b_root = world_b
            .borrow()
            .with_main_document(|parsed| parsed.dom.document())
            .expect("b document");

        // Realm A wraps realm B's document: the same object as B's `document`.
        let from_a = realm_a.context.with(|ctx| {
            let value = match wrap_node(&ctx, b_root) {
                Ok(value) => value,
                Err(err) => {
                    let caught = ctx.catch();
                    let message: String = caught
                        .as_object()
                        .and_then(|object| object.get("message").ok())
                        .unwrap_or_default();
                    panic!("wrap failed: {err:?}: {message}");
                }
            };
            Persistent::save(&ctx, value)
        });
        realm_b.context.with(|ctx| {
            let own: Value = super::super::eval_classic(&ctx, "document").expect("b document");
            let wrapped = from_a.restore(&ctx).expect("restore");
            assert_eq!(own, wrapped, "one wrapper is shared across realms");
            ctx.globals().set("__w", wrapped).expect("test global");
            let is_document: bool = super::super::eval_classic(
                &ctx,
                "Object.getPrototypeOf(__w) === Document.prototype",
            )
            .expect("prototype");
            assert!(is_document, "wrapper keeps the owner realm's prototype");
        });

        // Reading and mutating from A are visible in B through the same tree.
        realm_a.context.with(|ctx| {
            let value = wrap_node(&ctx, b_root).expect("wrap b document in a");
            let object = value.into_object().expect("object");
            ctx.globals().set("__b", object).expect("test global");
            let _: () = super::super::eval_classic(&ctx, "__b.body.setAttribute('x', '1')")
                .expect("mutate");
        });
        realm_b.context.with(|ctx| {
            let seen: bool =
                super::super::eval_classic(&ctx, "document.body.getAttribute('x') === '1'")
                    .expect("read");
            assert!(seen, "mutations from realm A reach realm B's tree");
            let element: Value =
                super::super::eval_classic(&ctx, "document.documentElement").expect("element");
            ctx.globals().set("__el", element).expect("test global");
        });
        let element_from_a = realm_a.context.with(|ctx| {
            let value = wrap_node(&ctx, b_root).expect("wrap b document in a");
            let object = value.into_object().expect("object");
            let element: Value = object.get("documentElement").expect("documentElement");
            Persistent::save(&ctx, element)
        });
        realm_b.context.with(|ctx| {
            let restored = element_from_a.restore(&ctx).expect("restore");
            ctx.globals()
                .set("__el_from_a", restored)
                .expect("test global");
            let same: bool =
                super::super::eval_classic(&ctx, "__el_from_a === __el").expect("identity");
            assert!(same, "cross-realm property reads return the shared wrapper");
        });
    }
}
