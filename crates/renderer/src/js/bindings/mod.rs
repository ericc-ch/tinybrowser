//! Platform objects for DOM nodes, one module per concern.
//!
//! `JsNode` is the single wrapper class for every node; the JS interface
//! prototypes copy its members, so the Rust class cannot split across
//! modules (`#[rquickjs::methods]` emits one `MethodImplementor` impl per
//! type). Its members stay in `node`.
use rquickjs::function::Constructor;

mod attributes;
mod clone;
mod collections;
mod document;
mod exceptions;
mod focus;
pub(crate) mod forms;
pub(crate) mod host;
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

use std::fmt;

use blitz_dom::NodeData;
use markup5ever::{LocalName, Namespace, Prefix, QualName};

use crate::js::world::NodeId;

use rquickjs::{
    Class, Ctx, Exception, FromJs, Function, Object, Persistent, Result, Value, class::Trace,
    prelude::This,
};

use super::events::{self, JsEvent, JsEventTarget};

use super::world::{Handle, RealmRegistry, WeakReferences, World};

thread_local! {
    /// JS world per live realm, keyed by its QuickJS context pointer.
    ///
    /// Runtime userdata would be one slot per renderer process, and the second
    /// frame would overwrite the first; each realm needs its own. One renderer
    /// thread hosts every frame, and entries are removed when the realm drops,
    /// so the map is thread-local and keyed by pointer identity.
    static REALM_WORLDS: RefCell<HashMap<usize, Weak<RefCell<World>>>> =
        RefCell::new(HashMap::new());
    static REALM_REGISTRIES: RefCell<HashMap<usize, Weak<RefCell<RealmRegistry>>>> =
        RefCell::new(HashMap::new());
}

/// Remembers `world` as the JS world of the realm behind `ctx`.
fn register_world(ctx: &Ctx<'_>, world: &Rc<RefCell<World>>) {
    let registry = world.borrow().registry();
    let context = ctx.as_raw().as_ptr() as usize;
    REALM_REGISTRIES.with(|registries| {
        registries
            .borrow_mut()
            .insert(context, Rc::downgrade(&registry));
    });
    registry.borrow_mut().realm_contexts.push(context);
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

pub(crate) fn forget_registry_contexts(contexts: &[usize]) {
    REALM_REGISTRIES.with(|registries| {
        let mut registries = registries.borrow_mut();
        for context in contexts {
            registries.remove(context);
        }
    });
}

/// The agent associated with a context, including a retired iframe context
/// kept alive by an adopted platform object's prototype.
pub(crate) fn realm_registry(ctx: &Ctx<'_>) -> Result<Rc<RefCell<RealmRegistry>>> {
    REALM_REGISTRIES
        .with(|registries| {
            registries
                .borrow()
                .get(&(ctx.as_raw().as_ptr() as usize))
                .and_then(Weak::upgrade)
        })
        .ok_or_else(|| Exception::throw_internal(ctx, "missing JS agent"))
}

/// The document of the current realm's global object.
pub(crate) fn main_document(ctx: &Ctx<'_>) -> Result<NodeId> {
    world(ctx)?
        .borrow()
        .with_main_document(|parsed| NodeId {
            document: parsed.id,
            node: parsed.document.base.root_node().id,
        })
        .ok_or_else(|| Exception::throw_type(ctx, "no document"))
}

/// Builds and throws a `DOMException` from Rust with a real prototype, so
/// `instanceof DOMException` and `constructor` checks pass.
pub(crate) fn throw_dom(ctx: &Ctx<'_>, name: &str, message: &str) -> rquickjs::Error {
    let exception = (|| {
        let name = rquickjs::String::from_str(ctx.clone(), name)?;
        let message = rquickjs::String::from_str(ctx.clone(), message)?;
        host::instance(ctx, JsDomException { name, message })
    })();
    match exception {
        Ok(exception) => ctx.throw(Class::into_value(exception)),
        Err(err) => err,
    }
}

/// A refused tree operation, mapped to its DOM exception class
/// (<https://dom.spec.whatwg.org/#dom-domerror> naming).
#[derive(Clone, Copy, Debug)]
pub(crate) enum TreeError {
    /// A handle named a node that no longer exists.
    Stale,
    /// The move would place a node inside its own subtree, or the content
    /// model forbids the operation.
    Hierarchy,
    /// The target has no parent to insert beside.
    NotFound,
}

impl fmt::Display for TreeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stale => f.write_str("stale node handle"),
            Self::Hierarchy => f.write_str("hierarchy or content model forbids this operation"),
            Self::NotFound => f.write_str("target has no parent to insert beside"),
        }
    }
}

/// Maps a refused tree operation onto its exception class
/// (<https://dom.spec.whatwg.org/#dom-domerror> naming).
pub(crate) fn throw_dom_error(ctx: &Ctx<'_>, err: TreeError) -> rquickjs::Error {
    match err {
        TreeError::Hierarchy => throw_dom(ctx, "HierarchyRequestError", &err.to_string()),
        TreeError::NotFound => throw_dom(ctx, "NotFoundError", &err.to_string()),
        // Programming errors, not web-visible DOM exceptions.
        TreeError::Stale => Exception::throw_type(ctx, &err.to_string()),
    }
}

/// Fires the realm window's `error` event for one uncaught exception, driving
/// `window.onerror` with the spec's five arguments. `base_line` translates the
/// inline stack's script-relative line into a document line (0 when unknown).
///
/// <https://html.spec.whatwg.org/multipage/webappapis.html#report-the-error>
pub(super) fn report_exception_value<'js>(
    ctx: &Ctx<'js>,
    caught: Value<'js>,
    base_line: u32,
    filename: &str,
) {
    let Ok(report) =
        crate::js::bridge::object(ctx).and_then(|host| host.get::<_, Value>("__tbReportException"))
    else {
        return;
    };
    let Some(report) = report.as_function() else {
        return;
    };
    let Ok(meta) = Object::new(ctx.clone()) else {
        return;
    };
    if meta.set("baseLine", base_line).is_err() || meta.set("filename", filename).is_err() {
        return;
    }
    // A throwing `onerror` is the reporter's problem to swallow; report_exception
    // already dropped the pending exception.
    if report.call::<_, ()>((caught, meta)).is_err() {
        // Clear the exception the reporter threw so it does not surface at the
        // next `catch`.
        let _ = ctx.catch();
    }
}

/// A promise rejected without a handler, waiting for either the
/// microtask-checkpoint `unhandledrejection` report or a late handler
/// (<https://html.spec.whatwg.org/multipage/webappapis.html#unhandled-promise-rejections>).
struct PendingRejection {
    world: Weak<RefCell<World>>,
    promise: Persistent<Value<'static>>,
    reason: Persistent<Value<'static>>,
    reported: bool,
}

/// A promise whose `unhandledrejection` already fired and which was handled
/// afterwards, with the realm that owns it.
struct LateHandled {
    world: Weak<RefCell<World>>,
    promise: Persistent<Value<'static>>,
    reason: Persistent<Value<'static>>,
}

thread_local! {
    static PENDING_REJECTIONS: RefCell<Vec<PendingRejection>> =
        const { RefCell::new(Vec::new()) };
    /// Promises whose `unhandledrejection` already fired and which were handled
    /// afterwards; each fires one `rejectionhandled`.
    static LATE_HANDLED: RefCell<Vec<LateHandled>> = const { RefCell::new(Vec::new()) };
}

/// `QuickJS`'s host rejection tracker, called when a promise is rejected with no
/// handler (`handled` false) and again when a handler is attached to a still
/// unhandled rejection (`handled` true). It only records state; the events fire
/// from [`drain_rejections`] at the microtask checkpoint, because firing a DOM
/// event from inside the promise machinery would reenter the engine.
pub(super) fn note_rejection<'js>(
    ctx: &Ctx<'js>,
    promise: Value<'js>,
    reason: Value<'js>,
    handled: bool,
) {
    PENDING_REJECTIONS.with(|queue| {
        let mut queue = queue.borrow_mut();
        let index = queue.iter().position(|entry| {
            entry
                .promise
                .clone()
                .restore(ctx)
                .is_ok_and(|stored| stored == promise)
        });
        if handled {
            let Some(index) = index else {
                return;
            };
            let entry = queue.remove(index);
            if entry.reported {
                LATE_HANDLED.with(|late| {
                    late.borrow_mut().push(LateHandled {
                        world: entry.world.clone(),
                        promise: entry.promise,
                        reason: entry.reason,
                    });
                });
            }
            return;
        }
        if index.is_some() {
            return;
        }
        let Ok(world) = world(ctx) else {
            return;
        };
        queue.push(PendingRejection {
            world: Rc::downgrade(&world),
            promise: Persistent::save(ctx, promise),
            reason: Persistent::save(ctx, reason),
            reported: false,
        });
    });
}

/// Fires the queued rejection events for `current`'s realm: one
/// `unhandledrejection` per newly unhandled rejection, then any
/// `rejectionhandled` a late handler queued. Called after a microtask
/// checkpoint.
pub(super) fn drain_rejections(ctx: &Ctx<'_>, current: &Rc<RefCell<World>>) {
    let pending: Vec<PendingRejection> = PENDING_REJECTIONS.with(|queue| {
        let mut queue = queue.borrow_mut();
        let mut mine = Vec::new();
        let mut rest = Vec::new();
        for entry in queue.drain(..) {
            let Some(world) = entry.world.upgrade() else {
                // The realm is gone; its promise no longer matters.
                continue;
            };
            if Rc::ptr_eq(&world, current) {
                mine.push(entry);
            } else {
                rest.push(entry);
            }
        }
        *queue = rest;
        mine
    });
    for mut entry in pending {
        if entry.reported {
            PENDING_REJECTIONS.with(|queue| queue.borrow_mut().push(entry));
            continue;
        }
        entry.reported = true;
        let restored = (
            entry.promise.clone().restore(ctx),
            entry.reason.clone().restore(ctx),
        );
        // Re-queue before firing, so a handler attached inside the
        // `unhandledrejection` event still produces `rejectionhandled`.
        PENDING_REJECTIONS.with(|queue| queue.borrow_mut().push(entry));
        if let (Ok(promise), Ok(reason)) = restored {
            fire_rejection(ctx, false, promise, reason);
        }
    }
    let late: Vec<LateHandled> = LATE_HANDLED.with(|queue| {
        let mut queue = queue.borrow_mut();
        let mut mine = Vec::new();
        let mut rest = Vec::new();
        for entry in queue.drain(..) {
            if entry
                .world
                .upgrade()
                .is_some_and(|world| Rc::ptr_eq(&world, current))
            {
                mine.push(entry);
            } else {
                rest.push(entry);
            }
        }
        *queue = rest;
        mine
    });
    for entry in late {
        if let (Ok(promise), Ok(reason)) = (entry.promise.restore(ctx), entry.reason.restore(ctx)) {
            fire_rejection(ctx, true, promise, reason);
        }
    }
}

/// Dispatches one `PromiseRejectionEvent` at the realm window through the
/// `__tbPromiseRejection` shim.
fn fire_rejection<'js>(ctx: &Ctx<'js>, handled: bool, promise: Value<'js>, reason: Value<'js>) {
    let Ok(report) = crate::js::bridge::object(ctx)
        .and_then(|host| host.get::<_, Value>("__tbPromiseRejection"))
    else {
        return;
    };
    let Some(report) = report.as_function() else {
        return;
    };
    if report.call::<_, Value>((handled, promise, reason)).is_err() {
        // Clear the exception the handler threw.
        let _ = ctx.catch();
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

impl<'js> rquickjs::FromJs<'js> for WebIdlCodeUnits {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
        Ok(Self(crate::dom_string::DomString::from_utf16(
            webidl_to_units(ctx, value)?,
        )))
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
    crate::js::bridge::install(ctx, world)?;
    let globals = ctx.globals();
    world
        .borrow_mut()
        .set_window(Persistent::save(ctx, globals.clone()));
    Class::<JsEvent>::define(&globals)?;
    crate::js::bridge::evaluate(ctx, events::install_event_ctor_js(ctx)?)?;
    install_webdriver_bridge(ctx)?;
    // `innerWidth`/`innerHeight`/`outerWidth`/`outerHeight` are live getters
    // defined in the web shims; they read the document viewport so CDP
    // emulation is visible without re-installing globals
    // (<https://drafts.csswg.org/cssom-view/#dom-window-innerwidth>).
    crate::js::bridge::object(ctx)?.set(
        "__tb_new_custom_event",
        rquickjs::prelude::Func::from(events::construct_custom_event),
    )?;
    crate::js::bridge::object(ctx)?.set(
        "__tb_init_custom_event",
        rquickjs::prelude::Func::from(events::init_custom_event),
    )?;
    crate::js::bridge::evaluate(ctx, events::install_custom_event_js(ctx)?)?;
    Class::<JsEventTarget>::define(&globals)?;
    events::install_event_target_bridge(ctx)?;
    crate::js::bridge::evaluate(ctx, events::install_event_target_ctor_js(ctx)?)?;
    Class::<JsNode>::define(&globals)?;
    Class::<JsNodeList>::define(&globals)?;
    Class::<JsHtmlCollection>::define(&globals)?;
    Class::<JsOptionsCollection>::define(&globals)?;
    Class::<JsDomException>::define(&globals)?;
    crate::js::bridge::evaluate(ctx, events::install_abort_js(ctx)?)?;
    Class::<JsImplementation>::define(&globals)?;
    Class::<JsTokenList>::define(&globals)?;
    Class::<JsNamedNodeMap>::define(&globals)?;
    forms::install(ctx, &crate::js::bridge::object(ctx)?)?;
    Class::<JsDomParser>::define(&globals)?;
    crate::js::bridge::evaluate(ctx, parsing::install_domparser_ctor_js(ctx)?)?;
    Class::<JsXmlSerializer>::define(&globals)?;
    Class::<JsMutationObserver>::define(&globals)?;
    Class::<JsMutationRecord>::define(&globals)?;
    install_host_functions(ctx)?;
    node::install_custom_construction(ctx)?;
    crate::js::bridge::object(ctx)?.set("__tb_handlerNames", HANDLER_ATTRIBUTES.to_vec())?;
    crate::js::bridge::object(ctx)?.set(
        "__tbGetNodeHandler",
        rquickjs::prelude::Func::from(get_node_handler),
    )?;
    crate::js::bridge::object(ctx)?.set(
        "__tbSetNodeHandler",
        rquickjs::prelude::Func::from(set_node_handler),
    )?;
    crate::js::bridge::object(ctx)?.set(
        "__tbGetWindowHandler",
        rquickjs::prelude::Func::from(get_window_handler),
    )?;
    crate::js::bridge::object(ctx)?.set(
        "__tbSetWindowHandler",
        rquickjs::prelude::Func::from(set_window_handler),
    )?;
    install_brands(ctx)?;
    // Generated members land on the brands.js `Node.prototype`; the derived
    // interface prototypes inherit them through the prototype chain.
    node::install(ctx)?;
    Class::<JsAttr>::define(&globals)?;
    install_collection_brand(ctx)?;
    capture_host_primitives(ctx, &globals, world)?;

    let document_id = world.borrow().with_main_document(|parsed| NodeId {
        document: parsed.id,
        node: parsed.document.base.root_node().id,
    });
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
    crate::js::bridge::object(ctx)?.set(
        "__tbDispatchTrusted",
        rquickjs::prelude::Func::from(window_dispatch_trusted_event),
    )?;
    Ok(())
}

/// Registers the `__tb_*` host functions page scripts call into.
fn install_host_functions(ctx: &Ctx<'_>) -> Result<()> {
    crate::js::bridge::object(ctx)?.set(
        "__tb_deliver_mutations",
        rquickjs::prelude::Func::from(deliver_mutations),
    )?;
    crate::js::bridge::object(ctx)?.set(
        "__tb_viewport_size",
        rquickjs::prelude::Func::from(viewport_size),
    )?;
    crate::js::bridge::object(ctx)?.set(
        "__tb_construct",
        rquickjs::prelude::Func::from(construct_node),
    )?;
    crate::js::bridge::object(ctx)?.set(
        "__tb_construct_in_frame",
        rquickjs::prelude::Func::from(construct_node_in_frame),
    )?;
    Ok(())
}

/// The active document's viewport size in CSS pixels, for the live `window`
/// metrics (<https://drafts.csswg.org/cssom-view/#dom-window-innerwidth>).
#[allow(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
fn viewport_size(ctx: Ctx<'_>) -> Vec<f64> {
    let (width, height) = world(&ctx).ok().map_or_else(
        || crate::engine::DEFAULT_VIEWPORT,
        |world| world.borrow().viewport_size.get(),
    );
    vec![f64::from(width), f64::from(height)]
}

/// Captures the pristine intrinsics and host entry points every later lookup
/// must use instead of `ctx.globals()`.
fn capture_host_primitives<'js>(
    ctx: &Ctx<'js>,
    globals: &Object<'js>,
    world: &Rc<RefCell<World>>,
) -> Result<()> {
    let number: Function = globals.get("Number")?;
    let boolean: Function = globals.get("Boolean")?;
    let reflect: Object = globals.get("Reflect")?;
    let reflect_set: Function = reflect.get("set")?;
    let deliver: Function = crate::js::bridge::object(ctx)?.get("__tb_deliver_mutations")?;
    let weak_ref: Constructor = globals.get("WeakRef")?;
    let weak_ref_prototype: Object = weak_ref.get("prototype")?;
    let weak_ref_deref: Function = weak_ref_prototype.get("deref")?;
    let mut world = world.borrow_mut();
    world.weak_references = Some(WeakReferences {
        constructor: Persistent::save(ctx, weak_ref),
        deref: Persistent::save(ctx, weak_ref_deref),
    });
    world.pristine_number = Some(Persistent::save(ctx, number));
    world.pristine_boolean = Some(Persistent::save(ctx, boolean));
    world.pristine_reflect_set = Some(Persistent::save(ctx, reflect_set));
    world.pristine_queue_microtask = globals
        .get::<_, Function>("queueMicrotask")
        .ok()
        .map(|queue| Persistent::save(ctx, queue));
    world.deliver_mutations_fn = Some(Persistent::save(ctx, deliver));
    Ok(())
}

pub(crate) fn host_node_id<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Option<NodeId> {
    Class::<JsNode>::from_js(ctx, value.clone())
        .ok()
        .map(|node| node.borrow().node_id())
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
        let base = &parsed.document.base;
        super::world::is_connected(base, node.node)
            && base
                .get_node(node.node)
                .is_some_and(|node| node.data.downcast_element().is_some())
    });
    if !valid {
        return Ok(Value::new_null(ctx));
    }
    wrap_node(&ctx, node)
}

/// Every element's border box from the Blitz layout, in tree order.
pub(super) fn layout_boxes(ctx: &Ctx<'_>, document: NodeId) -> Result<Vec<crate::render::NodeBox>> {
    let world = world_for_node(ctx, document)?;
    let world_ref = world.borrow();
    let Some(mut parsed) = world_ref.document_mut(document) else {
        return Ok(Vec::new());
    };
    let base = &mut parsed.document.base;
    base.resolve(0.0);
    let mut boxes = Vec::new();
    let mut stack = vec![base.root_node().id];
    while let Some(id) = stack.pop() {
        let Some(node) = base.get_node(id) else {
            continue;
        };
        if node.data.downcast_element().is_some() {
            let position = node.absolute_position(0.0, 0.0);
            let size = node.final_layout().size;
            let visible = node.primary_styles().is_none_or(|styles| {
                use style::properties::generated::longhands::visibility::computed_value::T as Visibility;
                styles.get_inherited_box().visibility == Visibility::Visible
            });
            boxes.push(crate::render::NodeBox {
                node: NodeId {
                    document: document.document,
                    node: id,
                },
                x: position.x,
                y: position.y,
                width: size.width,
                height: size.height,
                visible,
            });
        }
        stack.extend(node.children.iter().rev().copied());
    }
    Ok(boxes)
}

/// `node`'s border box `(left, top, width, height)` from the current layout,
/// if it has one.
pub(super) fn element_box(ctx: &Ctx<'_>, node: NodeId) -> Result<Option<(f64, f64, f64, f64)>> {
    let boxes = layout_boxes(ctx, node)?;
    Ok(boxes
        .into_iter()
        .find(|item| item.node == node)
        .map(|item| {
            (
                f64::from(item.x),
                f64::from(item.y),
                f64::from(item.width),
                f64::from(item.height),
            )
        }))
}

/// Current viewport offset from the document's scrolling element.
/// <https://drafts.csswg.org/cssom-view/#scrolling-viewport>
fn viewport_scroll(ctx: &Ctx<'_>, node: NodeId) -> Result<(f64, f64)> {
    let world = world_for_node(ctx, node)?;
    let world = world.borrow();
    let Some(parsed) = world.document(node) else {
        return Ok((0.0, 0.0));
    };
    let scroll = parsed.document.base.viewport_scroll();
    Ok((scroll.x, scroll.y))
}

/// The deepest element whose laid-out border box contains the point, if any.
pub(super) fn element_at_point(
    ctx: &Ctx<'_>,
    document: NodeId,
    x: f64,
    y: f64,
) -> Result<Option<NodeId>> {
    let (scroll_x, scroll_y) = viewport_scroll(ctx, document)?;
    let x = x + scroll_x;
    let y = y + scroll_y;
    let boxes = layout_boxes(ctx, document)?;
    let mut best = None;
    for item in boxes {
        let node = item.node;
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

/// Points `from`'s existing wrapper at `to` and moves the wrapper cache.
///
/// Adoption copies the node into the destination tree. The spec keeps the
/// same object, so a wrapper this realm can see follows the copy
/// (<https://dom.spec.whatwg.org/#concept-node-adopt>). A wrapper that lives
/// in another realm stays where it is: this context cannot update it.
pub(super) fn retarget_wrapper(ctx: &Ctx<'_>, from: NodeId, to: NodeId) -> Result<()> {
    let world_rc = world(ctx)?;
    if let Some(source) = world_rc.borrow().owner_world(from) {
        world_rc
            .borrow()
            .registry()
            .borrow_mut()
            .stamp_creation_realm(from, &source);
    }
    world_rc
        .borrow()
        .registry()
        .borrow_mut()
        .rekey_wrapper(from, to);
    let Some(saved) = world(ctx)?.borrow().shared_wrapper(to) else {
        return Ok(());
    };
    let Ok(Some(value)) = deref_weak(ctx, saved) else {
        return Ok(());
    };
    let Ok(class) = Class::<JsNode>::from_js(ctx, value) else {
        return Ok(());
    };
    class.borrow_mut().handle = Handle(to);
    Ok(())
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
    wrap_new_document_in_world(ctx, parsed, &world(ctx)?)
}

/// Publishes a new document in `world_rc`, which determines its and its
/// descendants' relevant realm, and returns the document's shared wrapper.
/// <https://dom.spec.whatwg.org/#create-a-document>
pub(super) fn wrap_new_document_in_world<'js>(
    ctx: &Ctx<'js>,
    parsed: crate::Parsed,
    world_rc: &Rc<RefCell<World>>,
) -> Result<Value<'js>> {
    let root = world_rc.borrow_mut().add_document(parsed);
    let registry = world_rc.borrow().registry();
    registry
        .borrow_mut()
        .insert_document(root.document_id(), world_rc);
    wrap_node(ctx, root)
}

fn instantiate_node<'js>(ctx: &Ctx<'js>, id: NodeId) -> Result<Value<'js>> {
    let is_fragment = {
        let world = world(ctx)?;
        let world = world.borrow();
        let Some(parsed) = world.document(id) else {
            return Err(Exception::throw_type(ctx, "stale node"));
        };
        parsed.document.is_fragment(id.node)
    };
    let brand = with_node_data(ctx, id, |data| match data {
        Some(NodeData::Document(_)) => Some(if document_is_xml_document(ctx, id) {
            "XMLDocument"
        } else {
            "Document"
        }),
        Some(NodeData::Element(_)) if is_fragment => Some("DocumentFragment"),
        Some(NodeData::Element(element)) => Some(element_interface(&element.name)),
        Some(NodeData::Text(_)) => Some("Text"),
        Some(NodeData::Comment { .. }) => Some("Comment"),
        _ => None,
    })?;
    let Some(brand) = brand else {
        return Err(Exception::throw_type(ctx, "stale node"));
    };
    wrap_with_brand(ctx, id, brand)
}

/// Instantiates a `JsNode` wrapper with the realm-owner's prototype for
/// `brand`.
fn wrap_with_brand<'js>(ctx: &Ctx<'js>, id: NodeId, brand: &str) -> Result<Value<'js>> {
    let class = Class::instance(ctx.clone(), JsNode { handle: Handle(id) })?;
    // A node's relevant realm is the one that created it, even after
    // `adoptNode` changes its node document
    // (<https://dom.spec.whatwg.org/#concept-node-adopt>,
    // <https://github.com/whatwg/dom/issues/977>).
    // An iframe document created while a parent script runs has no realm yet,
    // so that world has no brands; use this realm's until that realm exists
    // (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#the-iframe-element:html-element-insertion-steps>).
    let world_rc = world(ctx)?;
    let owner = world_rc.borrow().owner_world(id);
    if let Some(owner) = owner.as_ref() {
        world_rc
            .borrow()
            .registry()
            .borrow_mut()
            .stamp_creation_realm(id, owner);
    }
    let creator = world_rc.borrow().registry().borrow().creation_realm(id);
    let proto = creator
        .as_ref()
        .and_then(|creator| creator.borrow().brand(brand))
        .or_else(|| owner.as_ref().and_then(|owner| owner.borrow().brand(brand)))
        .or_else(|| world_rc.borrow().brand(brand));
    if let Some(proto) = proto {
        class.set_prototype(Some(&proto.restore(ctx)?))?;
    }
    Ok(Class::into_value(class))
}

/// The element interface for a qualified name
/// (<https://html.spec.whatwg.org/multipage/dom.html#elements-in-the-dom:html-element>
/// and <https://w3c.github.io/mathml-core/#dom-mathmlelement>).
/// Other namespaces use the base `Element` interface.
fn element_interface(name: &QualName) -> &'static str {
    if name.ns == super::world::html_namespace() {
        html_element_interface(name.local.as_ref())
    } else if name.ns == super::world::svg_namespace() {
        "SVGElement"
    } else if name.ns == super::world::mathml_namespace() {
        "MathMLElement"
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
    let ctor = world(ctx)?
        .borrow()
        .weak_references
        .as_ref()
        .ok_or_else(|| Exception::throw_internal(ctx, "weak references are not initialized"))?
        .constructor
        .clone()
        .restore(ctx)?;
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
    let deref = world(ctx)?
        .borrow()
        .weak_references
        .as_ref()
        .ok_or_else(|| Exception::throw_internal(ctx, "weak references are not initialized"))?
        .deref
        .clone()
        .restore(ctx)?;
    let value: Value = deref.call((This(object.clone()),))?;
    if value.is_undefined() {
        Ok(None)
    } else {
        Ok(Some(value))
    }
}

/// Prototype-brand installation script for the browser-realm platform
/// objects; one `define` per `WebIDL` interface. Deflated by `build.rs`,
/// inflated once per process.
const INSTALL_BRANDS_DEFLATE: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/js_blobs/brands.deflate"));

fn install_brands_js(ctx: &Ctx<'_>) -> Result<&'static str> {
    static CACHE: std::sync::OnceLock<Box<str>> = std::sync::OnceLock::new();
    super::blob::decompress(ctx, INSTALL_BRANDS_DEFLATE, &CACHE)
}

fn install_brands(ctx: &Ctx<'_>) -> Result<()> {
    crate::js::bridge::evaluate(ctx, install_brands_js(ctx)?)?;
    let table: Object = crate::js::bridge::object(ctx)?.get("__tb_brandTable")?;
    let entries = table
        .props::<String, Object>()
        .collect::<Result<Vec<(String, Object)>>>()?;
    for (name, proto) in entries {
        world(ctx)?
            .borrow_mut()
            .intern_brand(name, Persistent::save(ctx, proto));
    }
    crate::js::bridge::object(ctx)?.remove("__tb_brandTable")?;
    Ok(())
}

/// Collection prototypes and legacy named-property behavior for the browser realm.
/// Deflated by `build.rs`, inflated once per process.
pub(super) const INSTALL_COLLECTIONS_DEFLATE: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/js_blobs/collections.deflate"));

/// The collections shim, inflated once per process.
pub(super) fn install_collections_js(ctx: &Ctx<'_>) -> Result<&'static str> {
    static CACHE: std::sync::OnceLock<Box<str>> = std::sync::OnceLock::new();
    super::blob::decompress(ctx, INSTALL_COLLECTIONS_DEFLATE, &CACHE)
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
    let registry = realm_registry(ctx)?;
    let owner = registry.borrow().owner_world(id);
    owner.ok_or_else(|| Exception::throw_type(ctx, "stale node"))
}

pub(super) fn with_node_data<T>(
    ctx: &Ctx<'_>,
    id: NodeId,
    read: impl FnOnce(Option<&NodeData>) -> T,
) -> Result<T> {
    let owner = world_for_node(ctx, id)?;
    let parsed = owner.borrow();
    let Some(parsed) = parsed.document(id) else {
        return Err(Exception::throw_type(ctx, "no document"));
    };
    Ok(read(
        parsed
            .document
            .base
            .get_node(id.node)
            .map(|node| &node.data),
    ))
}

pub(crate) fn character_data(ctx: &Ctx<'_>, id: NodeId) -> Result<crate::dom_string::DomString> {
    let owner = world_for_node(ctx, id)?;
    let parsed = owner.borrow();
    let Some(parsed) = parsed.document(id) else {
        return Err(Exception::throw_type(ctx, "no document"));
    };
    Ok(parsed.document.character_data(id.node))
}

/// The value of `id`'s attribute `local`, or the empty string.
pub(super) fn attribute_value(ctx: &Ctx<'_>, id: NodeId, local: &str) -> Result<String> {
    let owner = world_for_node(ctx, id)?;
    Ok(owner
        .borrow()
        .document(id)
        .and_then(|parsed| {
            super::world::attr(&parsed.document.base, id.node, local).map(str::to_owned)
        })
        .unwrap_or_default())
}

/// [Replaces data](https://dom.spec.whatwg.org/#concept-cd-replace) on a
/// `CharacterData` node; other kinds are a silent no-op (`nodeValue` setter).
///
/// The tree stores UTF-8, so lone surrogates land as the replacement
/// character (see `docs/progress.md`); the algorithms otherwise operate on
/// UTF-16 code units.
pub(super) fn set_character_data(
    ctx: &Ctx<'_>,
    id: NodeId,
    data: &crate::dom_string::DomString,
) -> Result<()> {
    let utf8 = data.to_string_lossy().into_owned();
    let owner = world_for_node(ctx, id)?;
    let owner = owner.borrow();
    let Some(mut parsed) = owner.document_mut(id) else {
        return Ok(());
    };
    let old_value = {
        let is_character_data =
            parsed.document.base.get_node(id.node).is_some_and(|node| {
                matches!(node.data, NodeData::Text(_) | NodeData::Comment { .. })
            });
        if !is_character_data {
            return Ok(());
        }
        parsed.document.character_data(id.node)
    };
    {
        let base = &mut parsed.document.base;
        let Some(node) = base.get_node(id.node) else {
            return Ok(());
        };
        match &node.data {
            NodeData::Text(_) => {
                base.mutate().set_node_text(id.node, &utf8);
            }
            NodeData::Comment { .. } => {
                base.snapshot_node(id.node);
                if let Some(NodeData::Comment { contents }) =
                    base.get_node_mut(id.node).map(|node| &mut node.data)
                {
                    contents.clear();
                    contents.push_str(&utf8);
                }
            }
            _ => return Ok(()),
        }
    }
    parsed
        .document
        .record(crate::js::world::JournalEntry::CharacterData {
            target: id,
            old_value,
        });
    drop(parsed);
    drop(owner);
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

pub(super) fn child_value<'js>(ctx: &Ctx<'js>, id: Option<NodeId>) -> Result<Value<'js>> {
    match id {
        Some(id) => wrap_node(ctx, id),
        None => Ok(Value::new_null(ctx.clone())),
    }
}

pub(super) fn sibling_value<'js>(ctx: &Ctx<'js>, id: NodeId, forward: bool) -> Result<Value<'js>> {
    let world = world(ctx)?;
    let sibling = world.borrow().document(id).and_then(|parsed| {
        sibling(&parsed.document.base, id.node, forward).map(|node| NodeId {
            document: id.document,
            node,
        })
    });
    child_value(ctx, sibling)
}

/// The sibling of `id` in `forward` direction, or `None` at the edge.
pub(super) fn sibling(
    base: &blitz_dom::BaseDocument,
    id: super::world::BlitzId,
    forward: bool,
) -> Option<super::world::BlitzId> {
    let node = base.get_node(id)?;
    let parent = node.parent?;
    let siblings = &base.get_node(parent)?.children;
    let position = siblings.iter().position(|sibling| *sibling == id)?;
    if forward {
        siblings.get(position + 1).copied()
    } else {
        position
            .checked_sub(1)
            .and_then(|index| siblings.get(index).copied())
    }
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
        let base = &parsed.document.base;
        let mut cursor = sibling(base, id.node, forward);
        while let Some(next) = cursor {
            if is_real_element(&parsed.document, next) {
                break;
            }
            cursor = sibling(base, next, forward);
        }
        cursor.map(|node| NodeId {
            document: id.document,
            node,
        })
    };
    child_value(ctx, found)
}

pub(super) fn string_value<'js>(ctx: &Ctx<'js>, text: &str) -> Result<Value<'js>> {
    Ok(rquickjs::String::from_str(ctx.clone(), text)?.into_value())
}

/// The HTML rules for parsing non-negative integers: skip ASCII whitespace,
/// require an ASCII digit, then consume digits (modulo 2^32) and ignore
/// whatever follows them
/// (<https://infra.spec.whatwg.org/#rules-for-parsing-non-negative-integers>).
///
/// Divergence: Chromium returns the reflection default for values that
/// overflow `unsigned long` (a canvas `width="4294967296"` reads 300 there)
/// where the IDL conversion wraps to 0.
pub(crate) fn parse_non_negative_integer(value: &str) -> Option<u32> {
    let mut chars = value.chars().peekable();
    while chars.peek().is_some_and(char::is_ascii_whitespace) {
        chars.next();
    }
    if !chars.peek().is_some_and(char::is_ascii_digit) {
        return None;
    }
    let mut result = 0_u32;
    while let Some(digit) = chars.peek().and_then(|c| c.to_digit(10)) {
        result = result.wrapping_mul(10).wrapping_add(digit);
        chars.next();
    }
    Some(result)
}

/// Rebinds the `document` global to the realm world's active document. A realm
/// created while a navigation response was still streaming bound the outgoing
/// document; the installed one arrives later.
pub(crate) fn refresh_document_global(ctx: &Ctx<'_>) -> Result<()> {
    let world = world(ctx)?;
    let document_id = world.borrow().with_main_document(|parsed| NodeId {
        document: parsed.id,
        node: parsed.document.base.root_node().id,
    });
    if let Some(id) = document_id {
        ctx.globals().set("document", wrap_node(ctx, id)?)?;
    }
    Ok(())
}

/// A DOM string as a JavaScript string value, preserving every code unit.
pub(crate) fn dom_string<'js>(
    ctx: &Ctx<'js>,
    value: &crate::dom_string::DomString,
) -> Result<rquickjs::String<'js>> {
    rquickjs::String::from_utf16(ctx.clone(), &value.units())
}

/// [Descendant text content](https://dom.spec.whatwg.org/#concept-descendant-text-content):
/// the data of all `Text` descendants in tree order.
///
/// Descends only into elements: a `Document` or other non-container child
/// contributes nothing, so its subtree is not entered. Fragment backings are
/// plain elements, so fragments are covered.
pub(super) fn descendant_text(
    base: &blitz_dom::BaseDocument,
    id: super::world::BlitzId,
) -> crate::dom_string::DomString {
    let mut text = crate::dom_string::DomString::default();
    let mut stack = base
        .get_node(id)
        .map(|node| node.children.iter().rev().copied().collect::<Vec<_>>())
        .unwrap_or_default();
    while let Some(current) = stack.pop() {
        let Some(node) = base.get_node(current) else {
            continue;
        };
        match &node.data {
            NodeData::Text(data) => text.push_str(&data.content),
            NodeData::Element(_) => {
                stack.extend(node.children.iter().rev().copied());
            }
            _ => {}
        }
    }
    text
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
pub(crate) fn valid_element_local_name(name: &str) -> bool {
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
    if uppercase && name.ns == super::world::html_namespace() {
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
        CollectionKind::ElementsByTag {
            name: name.to_owned(),
            html_document: document_is_html_content(ctx, scope),
        },
        Some("HTMLCollection"),
    )
}

/// Builds a live collection wrapper. `brand` selects the interface:
/// `None` means `NodeList`, `Some("HTMLCollection")` and
/// `Some("HTMLOptionsCollection")` select those; anything else throws.
pub(super) fn live_collection<'js>(
    ctx: &Ctx<'js>,
    scope: NodeId,
    kind: CollectionKind,
    brand: Option<&str>,
) -> Result<Value<'js>> {
    let query = CollectionQuery {
        scope: Handle(scope),
        kind,
    };
    match brand {
        None => match &query.kind {
            CollectionKind::Static(handles) => static_node_list(ctx, scope, handles),
            _ => Ok(Class::into_value(host::instance_for_node(
                ctx,
                scope,
                JsNodeList { query },
            )?)),
        },
        Some("HTMLCollection") => Ok(Class::into_value(host::instance_for_node(
            ctx,
            scope,
            JsHtmlCollection { query },
        )?)),
        Some("HTMLOptionsCollection") => Ok(Class::into_value(host::instance_for_node(
            ctx,
            scope,
            JsOptionsCollection { query },
        )?)),
        Some(_) => Err(Exception::throw_type(
            ctx,
            "unsupported collection interface",
        )),
    }
}

pub(super) fn is_element(base: &blitz_dom::BaseDocument, id: super::world::BlitzId) -> bool {
    base.get_node(id)
        .is_some_and(|node| matches!(node.data, NodeData::Element(_)))
}

pub(super) fn is_real_element(
    doc: &crate::documents::BlitzDocument,
    id: super::world::BlitzId,
) -> bool {
    !doc.is_fragment(id) && is_element(&doc.base, id)
}

/// The root of the tree `id` participates in (itself when detached).
pub(super) fn root_of(
    base: &blitz_dom::BaseDocument,
    id: super::world::BlitzId,
) -> super::world::BlitzId {
    let mut root = id;
    while let Some(parent) = base.get_node(root).and_then(|node| node.parent) {
        root = parent;
    }
    root
}

/// `id` followed by its inclusive ancestors, nearest first.
pub(super) fn ancestor_chain(
    base: &blitz_dom::BaseDocument,
    document: u32,
    id: super::world::BlitzId,
) -> Vec<NodeId> {
    let mut chain = vec![NodeId { document, node: id }];
    let mut cursor = id;
    while let Some(parent) = base.get_node(cursor).and_then(|node| node.parent) {
        chain.push(NodeId {
            document,
            node: parent,
        });
        cursor = parent;
    }
    chain
}

/// Document order of two nodes in one tree
/// (<https://dom.spec.whatwg.org/#concept-tree-order>).
pub(super) fn tree_order(
    base: &blitz_dom::BaseDocument,
    document: u32,
    a: super::world::BlitzId,
    b: super::world::BlitzId,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let chain_a = ancestor_chain(base, document, a);
    let chain_b = ancestor_chain(base, document, b);
    let mut common = 0;
    while common < chain_a.len()
        && common < chain_b.len()
        && chain_a[chain_a.len() - 1 - common] == chain_b[chain_b.len() - 1 - common]
    {
        common += 1;
    }
    if common == chain_a.len() {
        return chain_a.len().cmp(&chain_b.len());
    }
    if common == chain_b.len() {
        return Ordering::Greater;
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
    let kids: Vec<NodeId> = base
        .get_node(parent.node)
        .map(|node| {
            node.children
                .iter()
                .map(|child| NodeId {
                    document,
                    node: *child,
                })
                .collect()
        })
        .unwrap_or_default();
    let position_a = kids.iter().position(|&kid| kid == child_a);
    let position_b = kids.iter().position(|&kid| kid == child_b);
    position_a.cmp(&position_b)
}

pub(super) fn find_element_by_id(
    base: &blitz_dom::BaseDocument,
    document: u32,
    scope: super::world::BlitzId,
    id: &str,
) -> Option<NodeId> {
    // An element with an empty ID has no ID, so no element matches
    // (<https://dom.spec.whatwg.org/#concept-id>).
    if id.is_empty() {
        return None;
    }
    let mut stack = vec![scope];
    while let Some(current) = stack.pop() {
        let Some(node) = base.get_node(current) else {
            continue;
        };
        if node.data.downcast_element().is_some_and(|element| {
            element
                .attr(markup5ever::LocalName::from("id"))
                .is_some_and(|value| value == id)
        }) {
            return Some(NodeId {
                document,
                node: current,
            });
        }
        stack.extend(node.children.iter().rev().copied());
    }
    None
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
    use crate::js::world::NodeId;
    use crate::js::{JsRealm, SharedJsRuntime, World};
    use crate::messaging::Shared;
    use crate::protocol::{
        BrowserServices, BrowsingContextHost, DialCompletion, DialRequest, FrameId, MessagingHost,
        NetworkHost, StorageHost, StorageKind,
    };

    struct NullServices;

    impl NetworkHost for NullServices {
        fn start_dial(
            &self,
            _request: DialRequest,
            completion: DialCompletion,
        ) -> crate::protocol::DialCancellation {
            completion(Err(crate::protocol::DialFailure::Connect));
            Box::new(|| {})
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
        js_runtime: &SharedJsRuntime,
        services: &Arc<dyn BrowserServices>,
        documents: &Rc<RefCell<crate::documents::DocumentStore>>,
        registry: &Rc<RefCell<crate::js::RealmRegistry>>,
        url: &str,
        html: &str,
    ) -> Rc<RefCell<World>> {
        let runtime = crate::document::FrameRuntime {
            services: Arc::clone(services),
            js_runtime: js_runtime.handle(),
            wake: Arc::new(tokio::sync::Notify::new()),
            stop: Arc::new(Stop::new()),
            documents: Rc::clone(documents),
            registry: Rc::clone(registry),
            shared: Rc::new(RefCell::new(Shared::default())),
            pending_storage: Rc::new(RefCell::new(Vec::new())),
            font_ctx: parley::FontContext::default(),
        };
        let mut world = World::new(
            Url::parse(url).expect("test url"),
            FrameId::MAIN,
            &runtime,
            Rc::new(RefCell::new(Vec::new())),
        );
        let id = world.replace_document(crate::parse_html(
            html,
            blitz_dom::DocumentConfig::default(),
        ));
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
            &shared,
            &services,
            &documents,
            &registry,
            "https://a.test/",
            "<!doctype html><p></p>",
        );
        let realm = JsRealm::new(&shared.handle(), world, stop).expect("realm");
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
            &shared,
            &services,
            &documents,
            &registry,
            "https://a.test/",
            "<!doctype html><p>hello</p>",
        );
        let realm = JsRealm::new(&shared.handle(), world, stop).expect("realm");
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
            &shared,
            &services,
            &documents,
            &registry,
            "https://a.test/",
            "<!doctype html><p id=a></p>",
        );
        let world_b = world_with_document(
            &shared,
            &services,
            &documents,
            &registry,
            "https://b.test/",
            "<!doctype html><p id=b></p>",
        );
        let realm_a =
            JsRealm::new(&shared.handle(), world_a.clone(), Arc::clone(&stop)).expect("realm a");
        let realm_b =
            JsRealm::new(&shared.handle(), world_b.clone(), Arc::clone(&stop)).expect("realm b");

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
            &shared,
            &services,
            &documents,
            &registry,
            "https://a.test/",
            "<!doctype html><p id=a></p>",
        );
        let world_b = world_with_document(
            &shared,
            &services,
            &documents,
            &registry,
            "https://b.test/",
            "<!doctype html><p id=b></p>",
        );
        let realm_a = JsRealm::new(&shared.handle(), world_a, Arc::clone(&stop)).expect("realm a");
        let realm_b =
            JsRealm::new(&shared.handle(), world_b.clone(), Arc::clone(&stop)).expect("realm b");
        let b_root = world_b
            .borrow()
            .with_main_document(|parsed| NodeId {
                document: parsed.id,
                node: parsed.document.base.root_node().id,
            })
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
