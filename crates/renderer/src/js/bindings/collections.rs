//! Live node collections (`NodeList`, `HTMLCollection`).

use super::{INSTALL_COLLECTIONS_JS, collection_ids, host_node_id, live_collection, world, wrap_node};

use rquickjs::{
    Array, Class, Ctx, Exception, Function, Object, Persistent, Result, Value, class::Trace,
    prelude::Func,
};

use crate::js::world::Handle;
use dom::{NodeKind, html_namespace};
use std::collections::HashSet;

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
    WindowNamed(String),
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
    ctx.globals()
        .set("__tbWindowNamedValue", Func::from(window_named_value))?;
    ctx.globals()
        .set("__tbWindowNamedHas", Func::from(window_named_has))?;
    ctx.globals()
        .set("__tbCollectionNamed", Func::from(collection_named))?;
    ctx.globals()
        .set("__tbCollectionKeys", Func::from(collection_keys))?;
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
        matches!(parsed.document.kind(id), Some(NodeKind::Element { name, .. })
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
        .document
        .append_blank_options(id, count)
        .map_err(|_| Exception::throw_type(&ctx, "not a select"))
}

/// Determines the value of a Window named property: the single named element,
/// or an `HTMLCollection` when several share the name
/// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#named-access-on-the-window-object>).
fn window_named_value(ctx: Ctx<'_>, name: String) -> Result<Value<'_>> {
    if name.is_empty() {
        return Ok(Value::new_undefined(ctx.clone()));
    }
    let document = world(&ctx)?.borrow().main_document_root();
    let Some(document) = document else {
        return Ok(Value::new_undefined(ctx.clone()));
    };
    let ids = collection_ids(&ctx, document, &CollectionKind::WindowNamed(name.clone()))?;
    match ids.as_slice() {
        [] => Ok(Value::new_undefined(ctx)),
        [id] => wrap_node(&ctx, *id),
        _ => live_collection(
            &ctx,
            document,
            CollectionKind::WindowNamed(name),
            Some("HTMLCollection"),
        ),
    }
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
/// Whether `name` is a supported Window named-property name
/// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#named-access-on-the-window-object>).
fn window_named_has(ctx: Ctx<'_>, name: String) -> Result<bool> {
    if name.is_empty() {
        return Ok(false);
    }
    let world = world(&ctx)?;
    let world = world.borrow();
    let Some(mut parsed) = world.main_document_mut() else {
        return Ok(false);
    };
    Ok(dom::named::exists(&mut parsed.document, &name))
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
/// The named item of an `HTMLCollection`: the first element whose id is
/// `name`, or whose `name` attribute is `name` in the HTML namespace
/// (<https://dom.spec.whatwg.org/#dom-htmlcollection-nameditem>).
fn collection_named<'js>(ctx: Ctx<'js>, target: Value<'js>, name: String) -> Result<Value<'js>> {
    if name.is_empty() {
        return Ok(Value::new_null(ctx));
    }
    let Ok(collection) = Class::<JsCollection>::from_value(&target) else {
        return Ok(Value::new_null(ctx));
    };
    let ids = {
        let collection = collection.borrow();
        collection_ids(&ctx, collection.scope.0, &collection.kind)?
    };
    for id in ids {
        let matches = {
            let world = world(&ctx)?;
            let world = world.borrow();
            let Some(parsed) = world.document(id) else {
                return Ok(Value::new_null(ctx));
            };
            let id_matches =
                parsed.document.no_namespace_attribute(id, "id").as_deref() == Some(name.as_str());
            let name_matches = matches!(
                parsed.document.kind(id),
                Some(NodeKind::Element { name: qual, .. }) if qual.ns == html_namespace()
            ) && parsed.document.no_namespace_attribute(id, "name").as_deref() == Some(name.as_str());
            id_matches || name_matches
        };
        if matches {
            return wrap_node(&ctx, id);
        }
    }
    Ok(Value::new_null(ctx))
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
/// The named keys of an `HTMLCollection`, in tree order with later duplicates
/// ignored (<https://dom.spec.whatwg.org/#interface-htmlcollection>).
fn collection_keys<'js>(ctx: Ctx<'js>, target: Value<'js>) -> Result<Array<'js>> {
    let array = Array::new(ctx.clone())?;
    let Ok(collection) = Class::<JsCollection>::from_value(&target) else {
        return Ok(array);
    };
    let ids = {
        let collection = collection.borrow();
        collection_ids(&ctx, collection.scope.0, &collection.kind)?
    };
    let mut keys: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    {
        let world = world(&ctx)?;
        let world = world.borrow();
        for &id in &ids {
            let Some(parsed) = world.document(id) else {
                continue;
            };
            if let Some(element_id) = parsed.document.no_namespace_attribute(id, "id")
                && !element_id.is_empty()
                && seen.insert(element_id.clone())
            {
                keys.push(element_id);
            }
            let exposes_name = matches!(
                parsed.document.kind(id),
                Some(NodeKind::Element { name, .. }) if name.ns == html_namespace()
            );
            if exposes_name
                && let Some(element_name) = parsed.document.no_namespace_attribute(id, "name")
                && !element_name.is_empty()
                && seen.insert(element_name.clone())
            {
                keys.push(element_name);
            }
        }
    }
    for (index, key) in keys.iter().enumerate() {
        array.set(index, key.as_str())?;
    }
    Ok(array)
}
