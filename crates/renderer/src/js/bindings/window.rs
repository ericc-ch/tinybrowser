//! Window-level listeners, location, and load events.

use super::{events, main_document, world};
use rquickjs::function::{Opt, This};
use rquickjs::object::Accessor;

use std::cell::RefCell;

use std::rc::Rc;

use crate::js::world::NodeId;

use rquickjs::{Class, Ctx, Exception, Object, Result, Value};

use crate::js::events::JsEvent;
use crate::js::world::{EventTargetKey, World};
use crate::protocol::FrameId;

fn world_for_window_this<'js>(ctx: &Ctx<'js>, this: &Object<'js>) -> Result<Rc<RefCell<World>>> {
    let current = world(ctx)?;
    if this.as_value() == ctx.globals().as_value() {
        return Ok(current);
    }
    let slots: rquickjs::Function = crate::js::bridge::object(ctx)?.get("slots")?;
    let data: Object = slots.call(("tinybrowser.window.data",))?;
    let get: rquickjs::Function = data.get("get")?;
    let value: Value = get.call((this.clone(),))?;
    let Some(data) = value.as_object() else {
        return Ok(current);
    };
    let Ok(frame) = data.get::<_, f64>("frame") else {
        return Ok(current);
    };
    if !frame.is_finite() || frame < 0.0 || frame.fract() != 0.0 || frame > f64::from(u32::MAX) {
        return Ok(current);
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the value is range-checked to a non-negative u32 above"
    )]
    let Some(target) = current.borrow().frame_world(FrameId::new(frame as u64)) else {
        return Ok(current);
    };
    // https://html.spec.whatwg.org/multipage/window-object.html#windowproxy-get
    let same_origin =
        target.borrow().document_url.origin() == current.borrow().document_url.origin();
    if same_origin { Ok(target) } else { Ok(current) }
}

/// Installs the `Location` object. The engine has no navigation, so only the
/// read-only URL components exist
/// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#the-location-interface>).
/// Empty components return empty strings, as specified by
/// <https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-location-search>
/// and <https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-location-hash>.
pub(crate) fn install_location<'js>(
    ctx: &Ctx<'js>,
    globals: &Object<'js>,
    world: &Rc<RefCell<World>>,
) -> Result<()> {
    enum Component { Pathname, Href, Search, Hash, Origin, Protocol, Host, Hostname, Port }
    let components = [
        ("pathname", Component::Pathname),
        ("href", Component::Href),
        ("search", Component::Search),
        ("hash", Component::Hash),
        ("origin", Component::Origin),
        ("protocol", Component::Protocol),
        ("host", Component::Host),
        ("hostname", Component::Hostname),
        ("port", Component::Port),
    ];
    let location = Object::new(ctx.clone())?;
    for (name, component) in components {
        let owner = Rc::clone(world);
        location.prop(name, Accessor::new_get(move || {
            let owner = owner.borrow();
            let url = &owner.document_url;
            match component {
                Component::Pathname => url.path().to_owned(),
                Component::Href => url.as_str().to_owned(),
                Component::Search => url.query().filter(|query| !query.is_empty()).map_or_else(String::new, |query| format!("?{query}")),
                Component::Hash => url.fragment().filter(|fragment| !fragment.is_empty()).map_or_else(String::new, |fragment| format!("#{fragment}")),
                Component::Origin => url.origin().ascii_serialization(),
                Component::Protocol => format!("{}:", url.scheme()),
                Component::Host => match url.port() {
                    Some(port) => format!("{}:{port}", url.host_str().unwrap_or_default()),
                    None => url.host_str().unwrap_or_default().to_owned(),
                },
                Component::Hostname => url.host_str().unwrap_or_default().to_owned(),
                Component::Port => url.port().map_or_else(String::new, |port| port.to_string()),
            }
        }).enumerable())?;
    }
    globals.set("location", location)?;
    Ok(())
}

/// The window a `Window` method call targets, resolving a sloppy bare call.
///
/// A host function invoked as a bare global (`addEventListener(type, fn)`)
/// receives `undefined` for `this`: host functions do not get the sloppy-mode
/// global substitution a page function does. A missing receiver means the
/// current realm's window; any other non-object receiver is an illegal
/// invocation
/// (<https://webidl.spec.whatwg.org/#es-operations>).
fn window_world_for_call<'js>(
    ctx: &Ctx<'js>,
    this: &Value<'js>,
) -> Result<Rc<RefCell<World>>> {
    if this.is_undefined() || this.is_null() {
        return world(ctx);
    }
    let Some(object) = this.as_object() else {
        return Err(Exception::throw_type(ctx, "Illegal invocation"));
    };
    world_for_window_this(ctx, object)
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
pub(crate) fn window_add_event_listener<'js>(
    ctx: Ctx<'js>,
    this: This<Value<'js>>,
    typ: Value<'js>,
    callback: Value<'js>,
    options: Opt<Value<'js>>,
) -> Result<()> {
    let world = window_world_for_call(&ctx, &this.0)?;
    events::add_listener_in(
        &ctx,
        &world,
        EventTargetKey::Window,
        typ,
        callback,
        options.0,
    )
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
pub(crate) fn window_remove_event_listener<'js>(
    ctx: Ctx<'js>,
    this: This<Value<'js>>,
    typ: Value<'js>,
    callback: Value<'js>,
    options: Opt<Value<'js>>,
) -> Result<()> {
    let world = window_world_for_call(&ctx, &this.0)?;
    events::remove_listener_in(
        &ctx,
        &world,
        EventTargetKey::Window,
        typ,
        callback,
        options.0,
    )
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
pub(crate) fn window_dispatch_event<'js>(
    ctx: Ctx<'js>,
    this: This<Value<'js>>,
    event: Class<'js, JsEvent>,
) -> Result<bool> {
    let world = window_world_for_call(&ctx, &this.0)?;
    events::dispatch_event_for_window(&ctx, &world, &event)
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
pub(crate) fn window_dispatch_trusted_event<'js>(
    ctx: Ctx<'js>,
    event: Class<'js, JsEvent>,
) -> Result<bool> {
    events::dispatch_trusted_event(&ctx, EventTargetKey::Window, &event)
}

pub(crate) fn fire_dom_content_loaded(ctx: &Ctx<'_>) -> Result<()> {
    let document = main_document(ctx)?;
    events::fire_trusted(
        ctx,
        EventTargetKey::Node(document),
        "DOMContentLoaded",
        true,
        false,
    )?;
    Ok(())
}

/// Fires `readystatechange` at the document after its readiness changed
/// (<https://html.spec.whatwg.org/multipage/dom.html#current-document-readiness>).
pub(crate) fn fire_ready_state_change(ctx: &Ctx<'_>) -> Result<()> {
    let document = main_document(ctx)?;
    events::fire_trusted(
        ctx,
        EventTargetKey::Node(document),
        "readystatechange",
        false,
        false,
    )?;
    Ok(())
}

pub(crate) fn fire_window_load(ctx: &Ctx<'_>) -> Result<()> {
    events::fire_trusted(ctx, EventTargetKey::Window, "load", false, false)
}

pub(crate) fn fire_node_load(ctx: &Ctx<'_>, id: NodeId) -> Result<()> {
    events::fire_trusted(ctx, EventTargetKey::Node(id), "load", false, false)
}

pub(crate) fn fire_node_error(ctx: &Ctx<'_>, id: NodeId) -> Result<()> {
    events::fire_trusted(ctx, EventTargetKey::Node(id), "error", false, false)
}
