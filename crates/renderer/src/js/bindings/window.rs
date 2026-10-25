//! Window-level listeners, location, and load events.

use super::{events, main_document, world};
use rquickjs::function::{Opt, This};
use rquickjs::object::Accessor;

use std::cell::RefCell;

use std::rc::Rc;

use crate::dom::tree::NodeId;

use rquickjs::{Class, Ctx, Exception, Object, Persistent, Result, Value};

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
    let same_origin = target.borrow().origin == current.borrow().origin;
    if same_origin { Ok(target) } else { Ok(current) }
}

/// The URL components `Location` exposes.
#[derive(Clone, Copy)]
enum Component {
    Pathname,
    Href,
    Search,
    Hash,
    Origin,
    Protocol,
    Host,
    Hostname,
    Port,
}

impl Component {
    /// The component's value with the shape the IDL getters return.
    fn read(self, world: &World) -> String {
        let url = &world.document_url;
        match self {
            Self::Pathname => url.path().to_owned(),
            Self::Href => url.as_str().to_owned(),
            Self::Search => url
                .query()
                .filter(|query| !query.is_empty())
                .map_or_else(String::new, |query| format!("?{query}")),
            Self::Hash => url
                .fragment()
                .filter(|fragment| !fragment.is_empty())
                .map_or_else(String::new, |fragment| format!("#{fragment}")),
            // The settings object's origin, not the URL's: `about:blank`,
            // `about:srcdoc`, and `data:` documents carry a captured origin
            // (<https://html.spec.whatwg.org/multipage/browsers.html#concept-origin>).
            Self::Origin => world.origin.ascii_serialization(),
            Self::Protocol => format!("{}:", url.scheme()),
            Self::Host => match url.port() {
                Some(port) => format!("{}:{port}", url.host_str().unwrap_or_default()),
                None => url.host_str().unwrap_or_default().to_owned(),
            },
            Self::Hostname => url.host_str().unwrap_or_default().to_owned(),
            Self::Port => url.port().map_or_else(String::new, |port| port.to_string()),
        }
    }

    /// Replaces this component of `url` with `value`, the way the IDL setter
    /// does. Returns whether the URL changed, so assigning the value the
    /// document already has does not reload it
    /// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#the-location-interface>).
    fn replace(self, url: &mut url::Url, value: &str) -> bool {
        let before = url.as_str().to_owned();
        match self {
            Self::Pathname => url.set_path(value),
            Self::Search => {
                url.set_query(
                    (!value.is_empty()).then(|| value.strip_prefix('?').unwrap_or(value)),
                );
            }
            Self::Protocol => {
                if url
                    .set_scheme(value.strip_suffix(':').unwrap_or(value))
                    .is_err()
                {
                    return false;
                }
            }
            Self::Host => {
                let (host, port) = split_host_port(value);
                if !replace_host(url, host) {
                    return false;
                }
                if let Some(port) = port {
                    match port.parse() {
                        Ok(port) => {
                            if url.set_port(Some(port)).is_err() {
                                return false;
                            }
                        }
                        Err(_) => return false,
                    }
                }
            }
            Self::Hostname => {
                if !replace_host(url, value) {
                    return false;
                }
            }
            Self::Port => {
                let ok = if value.is_empty() {
                    url.set_port(None).is_ok()
                } else {
                    value
                        .parse()
                        .is_ok_and(|port| url.set_port(Some(port)).is_ok())
                };
                if !ok {
                    return false;
                }
            }
            // `href` names a whole URL and `hash` has no same-document
            // navigation to run; neither replaces a component.
            Self::Href | Self::Hash | Self::Origin => return false,
        }
        url.as_str() != before
    }
}

/// Splits a `host[:port]` pair, keeping an IPv6 literal's brackets whole.
fn split_host_port(value: &str) -> (&str, Option<&str>) {
    let host_len = if value.starts_with('[') {
        value.find(']').map_or(value.len(), |end| end + 1)
    } else {
        value.find(':').unwrap_or(value.len())
    };
    let (host, rest) = value.split_at(host_len);
    (host, rest.strip_prefix(':'))
}

/// Sets `url`'s host from the IDL `host`/`hostname` value, parsed as a host.
/// An invalid host leaves the URL alone: the setter performs no navigation
/// (<https://url.spec.whatwg.org/#concept-host-parser>).
fn replace_host(url: &mut url::Url, value: &str) -> bool {
    let Ok(host) = url::Host::parse(value) else {
        return false;
    };
    url.set_host(Some(&host.to_string())).is_ok()
}

/// Queues a navigation of `owner`'s frame to `spec`, which is what assigning
/// to `Location.href`, to any other navigating `Location` component, and to a
/// window's `location` does
/// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-location-href>).
fn navigate_location(owner: &Rc<RefCell<World>>, spec: String) {
    owner
        .borrow_mut()
        .queue_frame_navigation(crate::js::world::FrameNavigation::get(
            crate::js::world::NavigationTarget::Target("_self".to_owned()),
            spec,
        ));
}

/// The `Location` setter shared by every component that navigates.
///
/// `href` names a whole URL, resolved when the navigation runs; every other
/// component replaces itself in the current URL and navigates only when that
/// changed something.
fn set_component(owner: &Rc<RefCell<World>>, component: Component, value: Value<'_>) -> Result<()> {
    let ctx = value.ctx().clone();
    let value = super::webidl::webidl_to_string(&ctx, value)?;
    if matches!(component, Component::Href) {
        navigate_location(owner, value);
        return Ok(());
    }
    let mut url = owner.borrow().document_url.clone();
    if component.replace(&mut url, &value) {
        navigate_location(owner, url.as_str().to_owned());
    }
    Ok(())
}

/// Installs the `Location` object
/// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#the-location-interface>).
///
/// Empty components return empty strings, as specified by
/// <https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-location-search>
/// and <https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-location-hash>.
///
/// Assigning to a window's `location` navigates, like `Location.href`. The
/// `hash` setter stays read-only: its same-document branch needs "navigate to
/// a fragment", which this engine has no algorithm for, and a full reload
/// would be the wrong answer.
pub(crate) fn install_location<'js>(
    ctx: &Ctx<'js>,
    globals: &Object<'js>,
    world: &Rc<RefCell<World>>,
) -> Result<()> {
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
        let getter = Accessor::new_get(move || component.read(&owner.borrow()));
        if matches!(component, Component::Hash | Component::Origin) {
            location.prop(name, getter.enumerable())?;
            continue;
        }
        let owner = Rc::clone(world);
        location.prop(
            name,
            getter
                .set(move |value: Value<'js>| set_component(&owner, component, value))
                .enumerable(),
        )?;
    }
    world.borrow_mut().location_object = Some(Persistent::save(ctx, location));
    let getter_owner = Rc::clone(world);
    let setter_owner = Rc::clone(world);
    globals.prop(
        "location",
        Accessor::new(
            move |ctx: Ctx<'js>| -> Result<Value<'js>> {
                let saved = getter_owner.borrow().location_object.clone();
                let Some(saved) = saved else {
                    return Err(Exception::throw_internal(&ctx, "location is gone"));
                };
                Ok(saved.restore(&ctx)?.into_value())
            },
            move |value: Value<'js>| set_component(&setter_owner, Component::Href, value),
        )
        .enumerable(),
    )?;
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
fn window_world_for_call<'js>(ctx: &Ctx<'js>, this: &Value<'js>) -> Result<Rc<RefCell<World>>> {
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

/// Dispatches one trusted `securitypolicyviolation` event per blocked policy
/// at `target` (<https://w3c.github.io/webappsec-csp/#report-violation>).
pub(crate) fn fire_csp_violations(
    ctx: &Ctx<'_>,
    target: NodeId,
    violations: &[crate::csp::Violation],
) -> Result<()> {
    for violation in violations {
        events::fire_security_policy_violation(ctx, EventTargetKey::Node(target), violation)?;
    }
    Ok(())
}
