//! Live node collections (`NodeList`, `HTMLCollection`).

use super::{INSTALL_COLLECTIONS_JS, collection_ids, host_node_id, live_collection, world, wrap_node};

use rquickjs::{
    Array, Ctx, Exception, Function, Object, Persistent, Result, Value, class::Trace, prelude::Func,
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
        .set("__tbWindowNamedNames", Func::from(window_named_names))?;
    ctx.globals()
        .set("__tbWindowNamedSerial", Func::from(window_named_serial))?;
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
/// The supported property names of the Window, in tree order with later
/// duplicates ignored
/// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#named-access-on-the-window-object>).
fn window_named_names(ctx: Ctx<'_>) -> Result<Array<'_>> {
    let array = Array::new(ctx.clone())?;
    let document = world(&ctx)?.borrow().main_document_root();
    let Some(document) = document else {
        return Ok(array);
    };
    let mut names: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(parsed) = world.document(document) else {
            return Ok(array);
        };
        for id in parsed.dom.descendants(document) {
            let Some(NodeKind::Element { name, .. }) = parsed.dom.kind(id) else {
                continue;
            };
            if let Some(element_id) = parsed.dom.no_namespace_attribute(id, "id")
                && !element_id.is_empty()
                && seen.insert(element_id.clone())
            {
                names.push(element_id);
            }
            // Only these HTML elements expose a `name` to the Window
            // (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#named-access-on-the-window-object>).
            if name.ns == html_namespace()
                && matches!(name.local.as_ref(), "embed" | "form" | "img" | "object")
                && let Some(element_name) = parsed.dom.no_namespace_attribute(id, "name")
                && !element_name.is_empty()
                && seen.insert(element_name.clone())
            {
                names.push(element_name);
            }
        }
    }
    for (index, name) in names.iter().enumerate() {
        array.set(index, name.as_str())?;
    }
    Ok(array)
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
/// The document's mutation serial, so the shim can rebuild the name set only
/// when the tree changed.
fn window_named_serial(ctx: Ctx<'_>) -> Result<String> {
    let serial = world(&ctx)?
        .borrow()
        .main_document()
        .map_or(0, |parsed| parsed.dom.mutation_serial());
    Ok(serial.to_string())
}
