//! Live node collections (`NodeList`, `HTMLCollection`).

use super::{INSTALL_COLLECTIONS_JS, collection_ids, host_node_id, world, wrap_node};

use rquickjs::{Ctx, Exception, Function, Object, Persistent, Result, Value, class::Trace, prelude::Func};

use crate::js::world::Handle;
use dom::{NodeKind, html_namespace};

#[derive(Trace, rquickjs::JsLifetime)]
pub(crate) enum CollectionKind {
    Children,
    ElementChildren,
    ElementsByTag(String),
    ElementsByTagNs { namespace: String, local: String },
    ElementsByClass(String),
    ElementsByName(String),
    SelectOptions,
    SelectedOptions,
    Static(Vec<Handle>),
}

#[derive(Trace, rquickjs::JsLifetime)]
#[rquickjs::class(rename = "NodeList")]
pub(crate) struct JsCollection {
    pub(crate) scope: Handle,
    pub(crate) kind: CollectionKind,
}

#[rquickjs::methods]
impl JsCollection {
    #[qjs(constructor)]
    fn ctor(ctx: Ctx<'_>) -> Result<Self> {
        let error = Exception::throw_type(&ctx, "Illegal constructor");
        drop(ctx);
        Err(error)
    }

    #[qjs(get)]
    fn length(&self, ctx: Ctx<'_>) -> Result<usize> {
        let result = collection_ids(&ctx, self.scope.0, &self.kind).map(|ids| ids.len());
        drop(ctx);
        result
    }

    fn item<'js>(&self, ctx: Ctx<'js>, index: usize) -> Result<Value<'js>> {
        match collection_ids(&ctx, self.scope.0, &self.kind)?
            .get(index)
            .copied()
        {
            Some(id) => wrap_node(&ctx, id),
            None => Ok(Value::new_null(ctx)),
        }
    }
}

pub(crate) fn install_collection_brand(ctx: &Ctx<'_>) -> Result<()> {
    ctx.globals()
        .set("__tbIsOptionNode", Func::from(is_option_node))?;
    ctx.globals()
        .set("__tbAppendBlankOptions", Func::from(append_blank_options))?;
    ctx.eval::<(), _>(INSTALL_COLLECTIONS_JS)?;
    let ctor: Function = ctx.globals().get("HTMLCollection")?;
    let proto: Object = ctor.get("prototype")?;
    let options_ctor: Function = ctx.globals().get("HTMLOptionsCollection")?;
    let options_proto: Object = options_ctor.get("prototype")?;
    let world = world(ctx)?;
    let mut world = world.borrow_mut();
    world.intern_brand("HTMLCollection", Persistent::save(ctx, proto));
    world.intern_brand(
        "HTMLOptionsCollection",
        Persistent::save(ctx, options_proto),
    );
    Ok(())
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
fn is_option_node<'js>(ctx: Ctx<'js>, value: Value<'js>) -> Result<bool> {
    let Some(id) = host_node_id(&ctx, &value) else {
        return Ok(false);
    };
    let world = world(&ctx)?;
    let world = world.borrow();
    Ok(world.document(id).is_some_and(|parsed| {
        matches!(parsed.dom.kind(id), Some(NodeKind::Element { name, .. })
            if name.ns == html_namespace() && name.local.as_ref() == "option")
    }))
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
fn append_blank_options<'js>(ctx: Ctx<'js>, select: Value<'js>, count: u32) -> Result<()> {
    let Some(id) = host_node_id(&ctx, &select) else {
        return Err(Exception::throw_type(&ctx, "not a select"));
    };
    let count = usize::try_from(count).map_err(|_| Exception::throw_type(&ctx, "invalid count"))?;
    let world = world(&ctx)?;
    let world = world.borrow();
    let Some(mut parsed) = world.document_mut(id) else {
        return Err(Exception::throw_type(&ctx, "no document"));
    };
    parsed
        .dom
        .append_blank_options(id, count)
        .map_err(|_| Exception::throw_type(&ctx, "not a select"))
}
