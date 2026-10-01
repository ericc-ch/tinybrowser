//! Live node collections (`NodeList`, `HTMLCollection`).

use super::{
    WebIdlUnsignedLong, collection_ids, host_node_id, install_collections_js, live_collection,
    world, wrap_node,
};

use rquickjs::{
    Atom, Ctx, Exception, Function, Object, Persistent, Result, Value,
    class::{ExoticDefineResult, ExoticSetResult, PropertyDescriptor, PropertyName, Trace},
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
pub(crate) struct CollectionQuery {
    pub(crate) scope: Handle,
    pub(crate) kind: CollectionKind,
}

impl CollectionQuery {
    fn ids(&self, ctx: &Ctx<'_>) -> Result<Vec<dom::NodeId>> {
        collection_ids(ctx, self.scope.0, &self.kind)
    }

    fn item<'js>(&self, ctx: &Ctx<'js>, index: usize) -> Result<Value<'js>> {
        let id = match &self.kind {
            CollectionKind::Static(handles) => {
                if super::realm_registry(ctx)?
                    .borrow()
                    .owner_world(self.scope.0)
                    .is_none()
                {
                    return Ok(Value::new_null(ctx.clone()));
                }
                handles.get(index).map(|handle| handle.0)
            }
            CollectionKind::Children
            | CollectionKind::ElementChildren
            | CollectionKind::ElementsByTag(_)
            | CollectionKind::ElementsByTagNs { .. }
            | CollectionKind::ElementsByClass(_)
            | CollectionKind::ElementsByName(_)
            | CollectionKind::SelectOptions
            | CollectionKind::SelectedOptions
            | CollectionKind::WindowNamed(_) => self.ids(ctx)?.get(index).copied(),
        };
        match id {
            Some(id) => wrap_node(ctx, id),
            None => Ok(Value::new_null(ctx.clone())),
        }
    }
}

// https://webidl.spec.whatwg.org/#is-an-array-index
pub(super) fn array_index(name: &str) -> Option<u32> {
    let index = name
        .parse::<u32>()
        .ok()
        .filter(|index| *index != u32::MAX)?;
    (index.to_string() == name).then_some(index)
}

/// The property key as a string, or `None` for symbols.
/// Symbols (including `Symbol.iterator` and `Symbol("0")`) are never
/// supported indexed or named properties, so every exotic hook falls through
/// to the ordinary path instead of treating a symbol description as a name.
/// Re-interning distinguishes string atoms from symbol atoms that share a
/// description; numeric atoms are checked via both string and `u32` forms.
pub(super) fn atom_name<'js>(ctx: &Ctx<'js>, atom: &Atom<'js>) -> Option<String> {
    let name = atom.to_string().ok()?;
    if let Ok(string_atom) = Atom::from_str(ctx.clone(), &name)
        && string_atom == *atom
    {
        return Some(name);
    }
    if let Some(index) = array_index(&name)
        && let Ok(number_atom) = Atom::from_u32(ctx.clone(), index)
        && number_atom == *atom
    {
        return Some(name);
    }
    None
}

fn indexed_descriptor<'js>(
    ctx: &Ctx<'js>,
    query: &CollectionQuery,
    name: &str,
    writable: bool,
) -> Result<Option<PropertyDescriptor<'js>>> {
    let Some(index) = array_index(name) else {
        return Ok(None);
    };
    let Some(id) = query.ids(ctx)?.get(index as usize).copied() else {
        return Ok(None);
    };
    Ok(Some(PropertyDescriptor::new_value(
        wrap_node(ctx, id)?,
        true,
        true,
        writable,
    )))
}

fn indexed_names<'js>(ctx: &Ctx<'js>, len: usize) -> Result<Vec<PropertyName<'js>>> {
    (0..len)
        .map(|index| {
            Ok(PropertyName {
                atom: Atom::from_u32(
                    ctx.clone(),
                    u32::try_from(index)
                        .map_err(|_| Exception::throw_range(ctx, "collection index too large"))?,
                )?,
                is_enumerable: true,
            })
        })
        .collect()
}

pub(super) fn reject_indexed_write<'js>(
    name: &str,
    object: &Value<'js>,
    receiver: &Value<'js>,
) -> ExoticSetResult {
    // https://webidl.spec.whatwg.org/#legacy-platform-object-set: any array
    // index is rejected (even past the end) so `coll[999] = 1` does not create
    // an expando; `HTMLCollection-supported-property-indices.html` requires it.
    if object == receiver && array_index(name).is_some() {
        ExoticSetResult::Handled(false)
    } else {
        ExoticSetResult::Fallthrough
    }
}

#[derive(Trace, rquickjs::JsLifetime)]
pub(crate) struct JsNodeList {
    pub(crate) query: CollectionQuery,
}

include!(concat!(env!("OUT_DIR"), "/NodeList.rs"));

#[expect(
    clippy::needless_pass_by_value,
    reason = "generated operation dispatch passes Ctx by value"
)]
impl JsNodeList {
    // https://dom.spec.whatwg.org/#dom-nodelist-length
    fn length(&self, ctx: &Ctx<'_>) -> Result<usize> {
        let registry = super::realm_registry(ctx)?;
        let Some(world) = registry.borrow().owner_world(self.query.scope.0) else {
            return Ok(0);
        };
        let world = world.borrow();
        let Some(parsed) = world.document(self.query.scope.0) else {
            return Ok(0);
        };
        Ok(match &self.query.kind {
            CollectionKind::Static(handles) => handles.len(),
            CollectionKind::Children => parsed
                .document
                .children(self.query.scope.0)
                .map_or(0, Iterator::count),
            CollectionKind::ElementsByName(name) => parsed
                .document
                .tree()
                .descendants(self.query.scope.0)
                .filter(|&id| {
                    super::is_element(&parsed.document, id)
                        && parsed.document.attribute(id, "name").as_deref() == Some(name)
                })
                .count(),
            CollectionKind::ElementChildren
            | CollectionKind::ElementsByTag(_)
            | CollectionKind::ElementsByTagNs { .. }
            | CollectionKind::ElementsByClass(_)
            | CollectionKind::SelectOptions
            | CollectionKind::SelectedOptions
            | CollectionKind::WindowNamed(_) => self.query.ids(ctx)?.len(),
        })
    }

    // https://dom.spec.whatwg.org/#dom-nodelist-item
    fn item<'js>(&self, ctx: Ctx<'js>, index: u32) -> Result<Value<'js>> {
        self.query.item(&ctx, index as usize)
    }
}

#[derive(Trace, rquickjs::JsLifetime)]
pub(crate) struct JsHtmlCollection {
    pub(crate) query: CollectionQuery,
}

include!(concat!(env!("OUT_DIR"), "/HTMLCollection.rs"));

#[expect(
    clippy::needless_pass_by_value,
    reason = "generated operation dispatch passes Ctx by value"
)]
impl JsHtmlCollection {
    // https://dom.spec.whatwg.org/#dom-htmlcollection-length
    fn length(&self, ctx: &Ctx<'_>) -> Result<usize> {
        self.query.ids(ctx).map(|ids| ids.len())
    }

    // https://dom.spec.whatwg.org/#dom-htmlcollection-item
    fn item<'js>(&self, ctx: Ctx<'js>, index: u32) -> Result<Value<'js>> {
        self.query.item(&ctx, index as usize)
    }

    fn named_item<'js>(&self, ctx: Ctx<'js>, name: rquickjs::String<'js>) -> Result<Value<'js>> {
        named_item(&ctx, &self.query, &name.to_string()?)
    }

    fn supported_names(&self, ctx: &Ctx<'_>) -> Result<Vec<String>> {
        named_keys(ctx, &self.query.ids(ctx)?)
    }
}

#[derive(Trace, rquickjs::JsLifetime)]
#[rquickjs::class(rename = "HTMLOptionsCollection", exotic)]
pub(crate) struct JsOptionsCollection {
    pub(crate) query: CollectionQuery,
}

#[rquickjs::methods]
#[expect(
    clippy::needless_pass_by_value,
    reason = "rquickjs constructor ABI passes Ctx by value"
)]
impl JsOptionsCollection {
    #[qjs(constructor)]
    fn ctor(ctx: Ctx<'_>) -> Result<Self> {
        Err(Exception::throw_type(&ctx, "Illegal constructor"))
    }

    // No native `length`: the install script defines a custom accessor with a
    // truncating/expanding setter, which would conflict with a non-configurable
    // native own property. `item` stays native.
    #[qjs(rename = "item")]
    fn native_item<'js>(&self, ctx: Ctx<'js>, index: WebIdlUnsignedLong) -> Result<Value<'js>> {
        self.query.item(&ctx, index.0 as usize)
    }

    #[qjs(skip)]
    fn item<'js>(&self, ctx: Ctx<'js>, index: u32) -> Result<Value<'js>> {
        self.query.item(&ctx, index as usize)
    }

    #[qjs(skip)]
    fn length(&self, ctx: &Ctx<'_>) -> Result<usize> {
        self.query.ids(ctx).map(|ids| ids.len())
    }

    #[qjs(skip)]
    fn named_item<'js>(&self, ctx: Ctx<'js>, name: rquickjs::String<'js>) -> Result<Value<'js>> {
        named_item(&ctx, &self.query, &name.to_string()?)
    }
}

fn option_setter<'js>(ctx: &Ctx<'js>) -> Result<Function<'js>> {
    let world = world(ctx)?;
    let setter = world.borrow().option_setter.clone();
    match setter {
        Some(setter) => setter.restore(ctx),
        None => Err(Exception::throw_type(ctx, "options setter not installed")),
    }
}

#[rquickjs::exotic]
#[expect(
    clippy::needless_pass_by_value,
    reason = "rquickjs exotic callback ABI passes owned arguments"
)]
impl JsOptionsCollection {
    #[qjs(define_own_property)]
    fn define<'js>(
        &self,
        ctx: &Ctx<'js>,
        atom: Atom<'js>,
        value: Value<'js>,
        is_data: bool,
    ) -> Result<ExoticDefineResult> {
        // https://webidl.spec.whatwg.org/#legacy-platform-object-defineownproperty
        let Some(name) = atom_name(ctx, &atom) else {
            return Ok(ExoticDefineResult::Fallthrough);
        };
        if let Some(index) = array_index(&name) {
            if !is_data {
                return Ok(ExoticDefineResult::Handled(false));
            }
            let select = wrap_node(ctx, self.query.scope.0)?;
            option_setter(ctx)?.call::<_, ()>((select, index, value))?;
            return Ok(ExoticDefineResult::Handled(true));
        }
        // `[LegacyOverrideBuiltIns]`: named properties override built-ins, so
        // no prototype-visibility check here.
        // (<https://html.spec.whatwg.org/multipage/common-dom-interfaces.html#htmloptionscollection>)
        Ok(if named_keys(ctx, &self.query.ids(ctx)?)?.contains(&name) {
            ExoticDefineResult::Handled(false)
        } else {
            ExoticDefineResult::Fallthrough
        })
    }

    #[qjs(get_own_property)]
    fn own_property<'js>(
        &self,
        ctx: &Ctx<'js>,
        atom: Atom<'js>,
        object: Value<'js>,
    ) -> Result<Option<PropertyDescriptor<'js>>> {
        // https://webidl.spec.whatwg.org/#legacy-platform-object-getownproperty
        let Some(name) = atom_name(ctx, &atom) else {
            return Ok(None);
        };
        collection_descriptor(ctx, &self.query, &name, &object, true)
    }

    #[qjs(get_own_property_names)]
    fn own_names<'js>(&self, ctx: &Ctx<'js>, object: Value<'js>) -> Result<Vec<PropertyName<'js>>> {
        collection_names(ctx, &self.query, &object, true)
    }

    #[qjs(set)]
    fn set<'js>(
        &self,
        ctx: &Ctx<'js>,
        atom: Atom<'js>,
        object: Value<'js>,
        receiver: Value<'js>,
        value: Value<'js>,
    ) -> Result<ExoticSetResult> {
        // https://webidl.spec.whatwg.org/#legacy-platform-object-set
        let Some(name) = atom_name(ctx, &atom) else {
            return Ok(ExoticSetResult::Fallthrough);
        };
        if object != receiver {
            return Ok(if array_index(&name).is_some() {
                ExoticSetResult::Fallthrough
            } else {
                ExoticSetResult::FallthroughSkippingOwnProperty
            });
        }
        let Some(index) = array_index(&name) else {
            return Ok(ExoticSetResult::Fallthrough);
        };
        let select = wrap_node(ctx, self.query.scope.0)?;
        option_setter(ctx)?.call::<_, ()>((select, index, value))?;
        Ok(ExoticSetResult::Handled(true))
    }

    #[qjs(delete)]
    fn delete<'js>(&self, ctx: &Ctx<'js>, atom: Atom<'js>, object: Value<'js>) -> Result<bool> {
        // https://webidl.spec.whatwg.org/#legacy-platform-object-delete
        let Some(name) = atom_name(ctx, &atom) else {
            return Ok(true);
        };
        collection_delete(ctx, &self.query, &name, &object)
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
    ctx.eval::<(), _>(install_collections_js(ctx)?)?;
    // Capture the options indexed-write entry point, then remove it from the
    // page-visible global so `__tbSetOption` is not fingerprintable.
    let setter: Function = ctx.globals().get("__tbSetOption")?;
    world(ctx)?.borrow_mut().option_setter = Some(Persistent::save(ctx, setter));
    ctx.globals().remove("__tbSetOption")?;
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
    dom::form::append_blank_options(&mut parsed.document, id, count)
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

fn named_item<'js>(ctx: &Ctx<'js>, query: &CollectionQuery, name: &str) -> Result<Value<'js>> {
    // https://dom.spec.whatwg.org/#dom-htmlcollection-nameditem: an empty key
    // returns null without searching; id has global precedence over name, so
    // two passes in tree order. Each lookup resolves through the agent's
    // document registry, not the callback realm, so cross-frame collections
    // keep working after navigation or iframe retirement.
    if name.is_empty() {
        return Ok(Value::new_null(ctx.clone()));
    }
    let ids = query.ids(ctx)?;
    let registry = super::realm_registry(ctx)?;
    for id in &ids {
        let owner = registry.borrow().owner_world(*id);
        let Some(owner) = owner else {
            continue;
        };
        let matches = {
            let owner = owner.borrow();
            let Some(parsed) = owner.document(*id) else {
                continue;
            };
            parsed.document.no_namespace_attribute(*id, "id").as_deref() == Some(name)
        };
        if matches {
            return wrap_node(ctx, *id);
        }
    }
    for id in &ids {
        let owner = registry.borrow().owner_world(*id);
        let Some(owner) = owner else {
            continue;
        };
        let matches = {
            let owner = owner.borrow();
            let Some(parsed) = owner.document(*id) else {
                continue;
            };
            matches!(
                parsed.document.kind(*id),
                Some(NodeKind::Element { name: qual, .. }) if qual.ns == html_namespace()
            ) && parsed
                .document
                .no_namespace_attribute(*id, "name")
                .as_deref()
                == Some(name)
        };
        if matches {
            return wrap_node(ctx, *id);
        }
    }
    Ok(Value::new_null(ctx.clone()))
}

/// The supported named properties of an `HTMLCollection`, in tree order.
/// <https://dom.spec.whatwg.org/#interface-htmlcollection>
fn named_keys(ctx: &Ctx<'_>, ids: &[dom::NodeId]) -> Result<Vec<String>> {
    let mut keys: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    {
        let registry = super::realm_registry(ctx)?;
        for &id in ids {
            let owner = registry.borrow().owner_world(id);
            let Some(owner) = owner else {
                continue;
            };
            let owner = owner.borrow();
            let Some(parsed) = owner.document(id) else {
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
    Ok(keys)
}

// https://webidl.spec.whatwg.org/#dfn-named-property-visibility
pub(super) fn named_key_visible(object: &Value<'_>, name: &str) -> Result<bool> {
    let mut prototype = object.as_object().and_then(Object::get_prototype);
    while let Some(current) = prototype {
        if current.contains_own_key(name)? {
            return Ok(false);
        }
        prototype = current.get_prototype();
    }
    Ok(true)
}

fn collection_descriptor<'js>(
    ctx: &Ctx<'js>,
    query: &CollectionQuery,
    name: &str,
    object: &Value<'js>,
    is_options: bool,
) -> Result<Option<PropertyDescriptor<'js>>> {
    if let Some(descriptor) = indexed_descriptor(ctx, query, name, is_options)? {
        return Ok(Some(descriptor));
    }
    if array_index(name).is_some()
        || !named_keys(ctx, &query.ids(ctx)?)?.iter().any(|key| key == name)
        // `HTMLOptionsCollection` is `[LegacyOverrideBuiltIns]`: named
        // properties override built-ins, so no visibility check.
        // (<https://html.spec.whatwg.org/multipage/common-dom-interfaces.html#htmloptionscollection>)
        // `HTMLCollection` keeps `[LegacyUnenumerableNamedProperties]` hiding.
        || (!is_options && !named_key_visible(object, name)?)
    {
        return Ok(None);
    }
    let value = named_item(ctx, query, name)?;
    Ok(Some(PropertyDescriptor::new_value(
        value, true, is_options, false,
    )))
}

fn collection_names<'js>(
    ctx: &Ctx<'js>,
    query: &CollectionQuery,
    object: &Value<'js>,
    is_options: bool,
) -> Result<Vec<PropertyName<'js>>> {
    let ids = query.ids(ctx)?;
    let mut names = indexed_names(ctx, ids.len())?;
    for name in named_keys(ctx, &ids)? {
        if array_index(&name).is_none() && (is_options || named_key_visible(object, &name)?) {
            names.push(PropertyName {
                atom: Atom::from_str(ctx.clone(), &name)?,
                is_enumerable: is_options,
            });
        }
    }
    Ok(names)
}

fn collection_delete(
    ctx: &Ctx<'_>,
    query: &CollectionQuery,
    name: &str,
    object: &Value<'_>,
) -> Result<bool> {
    if let Some(index) = array_index(name) {
        return Ok(index as usize >= query.ids(ctx)?.len());
    }
    // Options named properties override built-ins, so visibility only gates
    // plain `HTMLCollection`.
    let visible = query_ids_are_options(query) || named_key_visible(object, name).unwrap_or(true);
    Ok(!named_keys(ctx, &query.ids(ctx)?)?
        .iter()
        .any(|key| key == name)
        || !visible)
}

fn query_ids_are_options(query: &CollectionQuery) -> bool {
    matches!(
        query.kind,
        CollectionKind::SelectOptions | CollectionKind::SelectedOptions
    )
}
