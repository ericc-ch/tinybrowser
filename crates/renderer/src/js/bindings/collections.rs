//! Live node collections (`NodeList`, `HTMLCollection`).

use super::{INSTALL_COLLECTIONS_JS, collection_ids, world, wrap_node};

use rquickjs::{Ctx, Exception, Function, Object, Persistent, Result, Value, class::Trace};

use crate::js::world::Handle;

#[derive(Trace, rquickjs::JsLifetime)]
pub(crate) enum CollectionKind {
    Children,
    ElementChildren,
    ElementsByTag(String),
    ElementsByTagNs { namespace: String, local: String },
    ElementsByClass(String),
    ElementsByName(String),
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
    ctx.eval::<(), _>(INSTALL_COLLECTIONS_JS)?;
    let ctor: Function = ctx.globals().get("HTMLCollection")?;
    let proto: Object = ctor.get("prototype")?;
    world(ctx)?
        .borrow_mut()
        .intern_brand("HTMLCollection", Persistent::save(ctx, proto));
    Ok(())
}
