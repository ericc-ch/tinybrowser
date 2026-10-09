use std::cell::RefCell;
use std::rc::Rc;

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
    if let Some(slots) = &mut registry.borrow_mut().private_slots {
        slots.realms.insert(ctx.as_raw().as_ptr() as usize);
    }
    world.borrow_mut().bridge = Some(Persistent::save(ctx, bridge));
    Ok(())
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

pub(crate) fn evaluate(ctx: &Ctx<'_>, source: &str) -> Result<()> {
    let primordials = include_str!("scripts/primordials.js");
    let bindings = include_str!("scripts/bindings.js");
    let initializer: Function = ctx.eval(format!(
        "(function(host) {{ 'use strict';\n{primordials}\n{bindings}\n{source}\n}})"
    ))?;
    initializer.call((object(ctx)?,))
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "rquickjs native-function ABI supplies an owned Ctx"
)]
pub(crate) fn evaluate_page(ctx: Ctx<'_>, source: String) -> Result<Value<'_>> {
    let mut options = EvalOptions::default();
    options.strict = false;
    ctx.eval_with_options(source, options)
}
