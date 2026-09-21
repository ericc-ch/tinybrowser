//! Window-level listeners, location, and load events.

use super::{events, main_document, world};
use rquickjs::function::{Opt, This};

use std::cell::RefCell;

use std::rc::Rc;

use dom::NodeId;

use rquickjs::{Class, Ctx, Object, Result, Value};

use crate::js::events::JsEvent;
use crate::js::world::{EventTargetKey, World};
use crate::protocol::FrameId;

/// The window `this` belongs to. A `WindowProxy` method call from another realm
/// still registers on that frame's window
/// (<https://html.spec.whatwg.org/multipage/window-object.html#windowproxy-get>).
fn world_for_window_this<'js>(ctx: &Ctx<'js>, this: &Object<'js>) -> Result<Rc<RefCell<World>>> {
    let current = world(ctx)?;
    let Ok(frame) = this.get::<_, f64>("__tb_frameId") else {
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
    let frame = FrameId::new(frame as u64);
    let target = current.borrow().frame_world(frame);
    Ok(target.unwrap_or(current))
}

/// Installs the `Location` object. The engine has no navigation, so only the
/// read-only URL components exist
/// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#the-location-interface>).
pub(crate) fn install_location<'js>(
    ctx: &Ctx<'js>,
    globals: &Object<'js>,
    world: &Rc<RefCell<World>>,
) -> Result<()> {
    let (pathname, href, search, hash, origin, protocol, host, hostname, port) = {
        let world = world.borrow();
        let url = &world.document_url;
        let hostname = url.host_str().map_or_else(String::new, ToOwned::to_owned);
        let port = url.port().map_or_else(String::new, |port| port.to_string());
        let host = if port.is_empty() {
            hostname.clone()
        } else {
            format!("{hostname}:{port}")
        };
        (
            url.path().to_owned(),
            url.as_str().to_owned(),
            url.query()
                .map_or_else(String::new, |query| format!("?{query}")),
            url.fragment()
                .map_or_else(String::new, |fragment| format!("#{fragment}")),
            url.origin().ascii_serialization(),
            format!("{}:", url.scheme()),
            host,
            hostname,
            port,
        )
    };
    let location = Object::new(ctx.clone())?;
    location.set("pathname", pathname)?;
    location.set("href", href)?;
    location.set("search", search)?;
    location.set("hash", hash)?;
    location.set("origin", origin)?;
    location.set("protocol", protocol)?;
    location.set("host", host)?;
    location.set("hostname", hostname)?;
    location.set("port", port)?;
    globals.set("location", location)?;
    Ok(())
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
pub(crate) fn window_add_event_listener<'js>(
    ctx: Ctx<'js>,
    this: This<Object<'js>>,
    typ: Value<'js>,
    callback: Value<'js>,
    options: Opt<Value<'js>>,
) -> Result<()> {
    let world = world_for_window_this(&ctx, &this.0)?;
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
    this: This<Object<'js>>,
    typ: Value<'js>,
    callback: Value<'js>,
    options: Opt<Value<'js>>,
) -> Result<()> {
    let world = world_for_window_this(&ctx, &this.0)?;
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
    event: Class<'js, JsEvent>,
) -> Result<bool> {
    events::dispatch_event(&ctx, EventTargetKey::Window, &event)
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
pub(crate) fn window_dispatch_trusted_event<'js>(
    ctx: Ctx<'js>,
    token: Value<'js>,
    event: Class<'js, JsEvent>,
) -> Result<bool> {
    crate::js::bindings::check_host_token(&ctx, &token)?;
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
