use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use rquickjs::{
    Context, Ctx, Exception, Function, Object, Persistent, Result, Runtime, Value,
    context::EvalOptions, prelude::Func,
};

use super::World;
use super::bindings::world;

pub(crate) fn prepare(runtime: &Runtime, world: &Rc<RefCell<World>>) -> Result<()> {
    let registry = world.borrow().registry();
    if registry.borrow().private_slots.is_none() {
        let context = Context::full(runtime)?;
        let factory = context.with(|ctx| {
            let factory: Function = ctx.eval(include_str!("scripts/private_state.js"))?;
            Ok::<_, rquickjs::Error>(Persistent::save(&ctx, factory))
        })?;
        registry.borrow_mut().private_slots = Some(super::world::PrivateSlots {
            factory,
            realms: std::collections::HashSet::new(),
        });
    }
    Ok(())
}

pub(crate) fn install(ctx: &Ctx<'_>, world: &Rc<RefCell<World>>) -> Result<()> {
    let bridge = Object::new(ctx.clone())?;
    bridge.set_prototype(None)?;
    let registry = world.borrow().registry();
    let saved_slots = registry
        .borrow()
        .private_slots
        .as_ref()
        .map(|slots| slots.factory.clone());
    let slots = saved_slots
        .ok_or_else(|| Exception::throw_internal(ctx, "private slot service is not initialized"))?
        .restore(ctx)?;
    bridge.set("slots", slots)?;
    let reflect: Object = ctx.globals().get("Reflect")?;
    bridge.set("apply", reflect.get::<_, Function>("apply")?)?;
    let json: Object = ctx.globals().get("JSON")?;
    bridge.set("stringify", json.get::<_, Function>("stringify")?)?;
    // Pristine `Function` constructor for event-handler compilation: pages
    // may replace the global afterwards, and handlers compile on demand.
    let function: Object = ctx.globals().get("Function")?;
    bridge.set("__tb_function", function)?;
    let table = Object::new(ctx.clone())?;
    table.set_prototype(None)?;
    bridge.set("__tb_handles", table)?;
    for name in ["__tb_async", "__tb_async_handles"] {
        let table = Object::new(ctx.clone())?;
        table.set_prototype(None)?;
        bridge.set(name, table)?;
    }
    bridge.set("evaluatePage", Func::from(evaluate_page))?;
    bridge.set("__tb_mint", Func::from(mint_handle))?;
    let install: Function = ctx.eval(format!(
        "(function(host) {{ 'use strict';\n{}\n{}\n}})",
        include_str!("scripts/primordials.js"),
        include_str!("scripts/remote_object.js")
    ))?;
    install.call::<_, ()>((bridge.clone(),))?;
    if let Some(slots) = &mut registry.borrow_mut().private_slots {
        slots.realms.insert(ctx.as_raw().as_ptr() as usize);
    }
    world.borrow_mut().bridge = Some(Persistent::save(ctx, bridge));
    // The environment's secure-context bit is fixed when the realm is created,
    // and every `[SecureContext]` surface reads this one global
    // (<https://html.spec.whatwg.org/multipage/webappapis.html#dom-issecurecontext>).
    // A Web IDL attribute is enumerable, configurable, and has no setter.
    let secure = world.borrow().is_secure_context();
    evaluate(
        ctx,
        &format!(
            "Object.defineProperty(globalThis, 'isSecureContext', \
             {{ value: {secure}, enumerable: true, configurable: true }});"
        ),
    )?;
    Ok(())
}

/// Next handle id handed to one page-side object. Rust owns the counter; the
/// realm only stores the value under the id it receives.
static NEXT_HANDLE: AtomicU64 = AtomicU64::new(1);

fn mint_handle() -> String {
    let id = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
    format!("h{id}")
}

pub(crate) fn release(ctx: &Ctx<'_>, world: &Rc<RefCell<World>>) {
    let registry = world.borrow().registry();
    let mut registry = registry.borrow_mut();
    if let Some(slots) = &mut registry.private_slots {
        slots.realms.remove(&(ctx.as_raw().as_ptr() as usize));
        if slots.realms.is_empty() {
            registry.private_slots = None;
        }
    }
}

pub(crate) fn object<'js>(ctx: &Ctx<'js>) -> Result<Object<'js>> {
    world(ctx)?
        .borrow()
        .bridge
        .clone()
        .ok_or_else(|| Exception::throw_internal(ctx, "realm bridge is not initialized"))?
        .restore(ctx)
}

/// One id or index a binding receives from script as a JS number.
///
/// The engine keys its maps with `u64` counters, and a page receives the value
/// as a JS number, which holds integers exactly only up to `2^53 - 1`
/// (<https://tc39.es/ecma262/#sec-number.max_safe_integer>). Both facts live
/// here: [`Self::MAX`] is the JS bound, and parsing a number yields the value or
/// nothing, so no caller narrows to `u32` or widens back.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct HostId(u64);

impl HostId {
    /// The largest id a JS number carries exactly, `Number.MAX_SAFE_INTEGER`.
    pub(crate) const MAX: u64 = (1 << 53) - 1;

    /// [`Self::MAX`] as the float [`Self::parse`] compares against, spelled out
    /// because casting it back would be the precision loss the bound prevents.
    const MAX_EXACT: f64 = 9_007_199_254_740_991.0;

    /// The id as the `u64` the engine's maps are keyed by.
    pub(crate) fn get(self) -> u64 {
        self.0
    }

    /// Parses one id from the number a binding received.
    pub(crate) fn parse(value: f64) -> Option<Self> {
        // The bound is compared as a float first, so the cast below only ever
        // sees an integer `u64` holds exactly (`2^53 - 1` is representable).
        if !value.is_finite() || value < 0.0 || value.fract() != 0.0 || value > Self::MAX_EXACT {
            return None;
        }
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the guards admit exactly the integers 0..=2^53-1"
        )]
        Some(Self(value as u64))
    }
}

/// A JS number naming a frame, or `None` when it is not a host id.
#[must_use]
pub(crate) fn frame_id(value: f64) -> Option<crate::protocol::FrameId> {
    HostId::parse(value).map(|id| crate::protocol::FrameId::new(id.get()))
}

pub(crate) fn evaluate(ctx: &Ctx<'_>, source: &str) -> Result<()> {
    let primordials = include_str!("scripts/primordials.js");
    let bindings = include_str!("scripts/bindings.js");
    // Engine code carries its own filename so violation source attribution
    // can tell the shim bundle apart from page script: the bundle's frames
    // never name a page location
    // (<https://w3c.github.io/webappsec-csp/#create-violation-for-global>).
    let mut options = EvalOptions::default();
    options.filename = Some(ENGINE_FILENAME.into());
    let initializer: Function = ctx.eval_with_options(
        format!("(function(host) {{ 'use strict';\n{primordials}\n{bindings}\n{source}\n}})"),
        options,
    )?;
    initializer.call((object(ctx)?,))
}

/// Filename for engine shim evaluations, so stacks distinguish engine frames
/// from page script. It parses as no URL scheme, so a leaked frame strips to
/// the empty string rather than a fake location.
pub(crate) const ENGINE_FILENAME: &str = "tinybrowser-shim";

pub(crate) fn evaluate_page(ctx: Ctx<'_>, source: String) -> Result<Value<'_>> {
    let mut options = EvalOptions::default();
    options.strict = false;
    ctx.eval_with_options(source, options)
}

#[cfg(test)]
mod tests {
    use super::HostId;

    #[test]
    fn a_host_id_is_a_js_integer_the_engine_can_key() {
        assert_eq!(HostId::parse(0.0).map(HostId::get), Some(0));
        assert_eq!(HostId::parse(7.0).map(HostId::get), Some(7));
        assert_eq!(
            HostId::parse(9_007_199_254_740_991.0).map(HostId::get),
            Some(HostId::MAX),
            "the largest exact JS integer is a host id"
        );
        assert_eq!(
            HostId::parse(f64::from(u32::MAX) * 10.0).map(HostId::get),
            Some(u64::from(u32::MAX) * 10),
            "an id past u32 is still exact in a JS number"
        );
        for rejected in [
            -1.0,
            1.5,
            f64::NAN,
            f64::INFINITY,
            9_007_199_254_740_992.0, // 2^53, one past MAX
        ] {
            assert_eq!(HostId::parse(rejected), None, "{rejected}");
        }
    }
}
