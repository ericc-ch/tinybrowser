//! Window-level listeners, location, and load events.

use super::{events, main_document};
use rquickjs::function::Opt;

use std::cell::RefCell;

use std::rc::Rc;

use dom::NodeId;

use rquickjs::{Class, Ctx, Object, Result, Value};

use crate::js::events::JsEvent;

use crate::js::world::{EventTargetKey, World};

/// Installs the `Location` object. The engine has no navigation, so only the
/// read-only URL components exist
/// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#the-location-interface>).
pub(crate) fn install_location<'js>(
    ctx: &Ctx<'js>,
    globals: &Object<'js>,
    world: &Rc<RefCell<World>>,
) -> Result<()> {
    let (pathname, href, search, origin, protocol, host, hostname, port) = {
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
    typ: Value<'js>,
    callback: Value<'js>,
    options: Opt<Value<'js>>,
) -> Result<()> {
    events::add_listener(&ctx, EventTargetKey::Window, typ, callback, options.0)
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
pub(crate) fn window_remove_event_listener<'js>(
    ctx: Ctx<'js>,
    typ: Value<'js>,
    callback: Value<'js>,
    options: Opt<Value<'js>>,
) -> Result<()> {
    events::remove_listener(&ctx, EventTargetKey::Window, typ, callback, options.0)
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
