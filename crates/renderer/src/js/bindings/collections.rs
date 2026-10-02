//! Live node collections (`NodeList`, `HTMLCollection`).

use super::{
    collection_ids, install_collections_js, live_collection, world, wrap_node,
};

use rquickjs::{
    Atom, Ctx, Exception, Object, Result, Value,
    class::{ExoticSetResult, Trace},
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
pub(crate) struct JsOptionsCollection {
    pub(crate) query: CollectionQuery,
}

include!(concat!(env!("OUT_DIR"), "/HTMLOptionsCollection.rs"));

#[expect(
    clippy::needless_pass_by_value,
    reason = "generated operation dispatch passes Ctx by value"
)]
impl JsOptionsCollection {
    // https://dom.spec.whatwg.org/#dom-htmlcollection-length
    fn length(&self, ctx: &Ctx<'_>) -> Result<usize> {
        self.query.ids(ctx).map(|ids| ids.len())
    }

    // https://html.spec.whatwg.org/multipage/common-dom-interfaces.html#dom-htmloptionscollection-length
    fn set_length(&self, ctx: &Ctx<'_>, value: u32) -> Result<()> {
        let select = self.query.scope.0;
        let registry = super::realm_registry(ctx)?;
        let Some(owner) = registry.borrow().owner_world(select) else {
            return Ok(());
        };
        let length = value as usize;
        let owner = owner.borrow();
        let Some(mut parsed) = owner.document_mut(select) else {
            return Ok(());
        };
        let current = dom::form::select_options(&parsed.document, select).len();
        if length == current {
            return Ok(());
        }
        if length > current {
            if length > 100_000 {
                return Ok(());
            }
            drop(parsed);
            drop(owner);
            let owner = registry.borrow().owner_world(select).ok_or_else(|| {
                Exception::throw_internal(ctx, "missing JS world")
            })?;
            owner.borrow().document_mut(select).map_or(Ok(()), |mut parsed| {
                dom::form::append_blank_options(&mut parsed.document, select, length - current)
                    .map_err(|_| Exception::throw_type(ctx, "not a select"))
            })?;
            super::mutation::schedule_mutation_delivery(ctx)?;
            return Ok(());
        }
        let options = dom::form::select_options(&parsed.document, select);
        let removed: Vec<dom::NodeId> = options[length..].to_vec();
        for option in removed {
            dom::mutation::detach(&mut parsed.document, option)
                .map_err(|err| super::throw_dom_error(ctx, err))?;
        }
        drop(parsed);
        drop(owner);
        super::mutation::schedule_mutation_delivery(ctx)?;
        Ok(())
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

    // https://html.spec.whatwg.org/multipage/common-dom-interfaces.html#htmloptionscollection-set-indexed
    fn set_indexed<'js>(&self, ctx: Ctx<'js>, index: u32, value: Value<'js>) -> Result<()> {
        let select = self.query.scope.0;
        if value.is_null() || value.is_undefined() {
            let index = i32::try_from(index).unwrap_or(i32::MAX);
            return self.remove_option(&ctx, index);
        }
        let option = require_option_element(&ctx, &value)?;
        let index_usize = index as usize;
        let registry = super::realm_registry(&ctx)?;
        let Some(owner) = registry.borrow().owner_world(select) else {
            return Ok(());
        };
        let current = {
            let owner = owner.borrow();
            let Some(parsed) = owner.document(select) else {
                return Ok(());
            };
            dom::form::select_options(&parsed.document, select).len()
        };
        if index_usize > current && index_usize >= 100_000 {
            return Ok(());
        }
        if index_usize < current {
            return replace_collection_option(&ctx, select, index_usize, option);
        }
        if index_usize > current {
            grow_collection_options(&ctx, select, index_usize - current)?;
        }
        append_collection_option(&ctx, select, option)
    }

    // https://html.spec.whatwg.org/multipage/common-dom-interfaces.html#dom-htmloptionscollection-add
    fn add<'js>(&self, ctx: Ctx<'js>, element: Value<'js>, before: Value<'js>) -> Result<()> {
        let select = self.query.scope.0;
        let element = require_option_group(&ctx, &element)?;
        if let Some(before_id) = super::host_node_id(&ctx, &before)
            && before_id == element
        {
            return Ok(());
        }
        if before.is_null() || before.is_undefined() {
            return append_add_element(&ctx, select, element);
        }
        if super::host_node_id(&ctx, &before).is_some() {
            return insert_before_element(&ctx, select, element, &before);
        }
        insert_before_index(&ctx, select, element, before)
    }

    // https://html.spec.whatwg.org/multipage/common-dom-interfaces.html#dom-htmloptionscollection-remove
    fn remove(&self, ctx: Ctx<'_>, index: i32) -> Result<()> {
        self.remove_option(&ctx, index)
    }

    fn remove_option(&self, ctx: &Ctx<'_>, index: i32) -> Result<()> {
        let select = self.query.scope.0;
        let Ok(index) = usize::try_from(index) else {
            return Ok(());
        };
        let registry = super::realm_registry(ctx)?;
        let Some(owner) = registry.borrow().owner_world(select) else {
            return Ok(());
        };
        let target = {
            let owner = owner.borrow();
            let Some(parsed) = owner.document(select) else {
                return Ok(());
            };
            dom::form::select_options(&parsed.document, select)
                .get(index)
                .copied()
        };
        let Some(target) = target else {
            return Ok(());
        };
        let owner = registry.borrow().owner_world(select).ok_or_else(|| {
            Exception::throw_internal(ctx, "missing JS world")
        })?;
        let owner = owner.borrow();
        let Some(mut parsed) = owner.document_mut(select) else {
            return Ok(());
        };
        dom::mutation::detach(&mut parsed.document, target)
            .map_err(|err| super::throw_dom_error(ctx, err))?;
        drop(parsed);
        drop(owner);
        super::mutation::schedule_mutation_delivery(ctx)?;
        Ok(())
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-selectedindex
    fn selected_index(&self, ctx: &Ctx<'_>) -> Result<i32> {
        let select = self.query.scope.0;
        let registry = super::realm_registry(ctx)?;
        let Some(owner) = registry.borrow().owner_world(select) else {
            return Ok(-1);
        };
        let owner = owner.borrow();
        Ok(owner.document(select).map_or(-1, |parsed| {
            dom::form::select_selected_index(&parsed.document, select)
        }))
    }

    fn set_selected_index(&self, ctx: &Ctx<'_>, value: i32) -> Result<()> {
        let select = self.query.scope.0;
        let registry = super::realm_registry(ctx)?;
        let Some(owner) = registry.borrow().owner_world(select) else {
            return Ok(());
        };
        let owner = owner.borrow();
        let Some(mut parsed) = owner.document_mut(select) else {
            return Ok(());
        };
        dom::form::set_select_selected_index(&mut parsed.document, select, value);
        Ok(())
    }
}

fn require_option_element<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<dom::NodeId> {
    let Some(option) = super::host_node_id(ctx, value) else {
        return Err(Exception::throw_type(ctx, "option must be an HTMLOptionElement"));
    };
    let registry = super::realm_registry(ctx)?;
    let Some(owner) = registry.borrow().owner_world(option) else {
        return Err(Exception::throw_type(ctx, "option must be an HTMLOptionElement"));
    };
    let owner = owner.borrow();
    let Some(parsed) = owner.document(option) else {
        return Err(Exception::throw_type(ctx, "option must be an HTMLOptionElement"));
    };
    let is_option = matches!(
        parsed.document.kind(option),
        Some(NodeKind::Element { name, .. }) if name.ns == html_namespace() && name.local.as_ref() == "option"
    );
    if is_option {
        Ok(option)
    } else {
        Err(Exception::throw_type(ctx, "option must be an HTMLOptionElement"))
    }
}

fn require_option_group<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<dom::NodeId> {
    let Some(element) = super::host_node_id(ctx, value) else {
        return Err(Exception::throw_type(ctx, "add requires an option or optgroup element"));
    };
    let registry = super::realm_registry(ctx)?;
    let Some(owner) = registry.borrow().owner_world(element) else {
        return Err(Exception::throw_type(ctx, "add requires an option or optgroup element"));
    };
    let owner = owner.borrow();
    let Some(parsed) = owner.document(element) else {
        return Err(Exception::throw_type(ctx, "add requires an option or optgroup element"));
    };
    let is_option_group = matches!(
        parsed.document.kind(element),
        Some(NodeKind::Element { name, .. }) if name.ns == html_namespace()
            && matches!(name.local.as_ref(), "option" | "optgroup")
    );
    if is_option_group {
        Ok(element)
    } else {
        Err(Exception::throw_type(ctx, "add requires an option or optgroup element"))
    }
}

fn append_add_element(ctx: &Ctx<'_>, select: dom::NodeId, element: dom::NodeId) -> Result<()> {
    let adopted = super::clone::adopt_across_documents(ctx, select, element)?;
    let registry = super::realm_registry(ctx)?;
    let Some(owner) = registry.borrow().owner_world(select) else {
        return Ok(());
    };
    let owner = owner.borrow();
    let Some(mut parsed) = owner.document_mut(select) else {
        return Ok(());
    };
    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-add
    if parsed.document.parent(element) == Some(select) {
        return Ok(());
    }
    dom::mutation::append(&mut parsed.document, select, adopted)
        .map_err(|err| super::throw_dom_error(ctx, err))?;
    drop(parsed);
    drop(owner);
    super::mutation::schedule_mutation_delivery(ctx)?;
    Ok(())
}

fn insert_before_element<'js>(
    ctx: &Ctx<'js>,
    select: dom::NodeId,
    element: dom::NodeId,
    before: &Value<'js>,
) -> Result<()> {
    let Some(before_id) = super::host_node_id(ctx, before) else {
        return Ok(());
    };
    let registry = super::realm_registry(ctx)?;
    let Some(select_owner) = registry.borrow().owner_world(select) else {
        return Ok(());
    };
    if !is_select_descendant(&select_owner, select, before_id) {
        return Err(super::throw_dom(
            ctx,
            "NotFoundError",
            "reference is not a child of this select",
        ));
    }
    if before_id == select {
        return Err(super::throw_dom(
            ctx,
            "NotFoundError",
            "reference is not a child of this select",
        ));
    }
    let parent = {
        let select_owner = select_owner.borrow();
        let Some(parsed) = select_owner.document(select) else {
            return Ok(());
        };
        parsed.document.parent(before_id)
    };
    let Some(parent) = parent else {
        return Err(super::throw_dom(
            ctx,
            "NotFoundError",
            "reference is not a child of this select",
        ));
    };
    let adopted = super::clone::adopt_across_documents(ctx, parent, element)?;
    let Some(owner) = registry.borrow().owner_world(select) else {
        return Ok(());
    };
    let owner = owner.borrow();
    let Some(mut parsed) = owner.document_mut(select) else {
        return Ok(());
    };
    dom::mutation::insert_before(&mut parsed.document, before_id, adopted)
        .map_err(|err| super::throw_dom_error(ctx, err))?;
    drop(parsed);
    drop(owner);
    super::mutation::schedule_mutation_delivery(ctx)?;
    Ok(())
}

fn insert_before_index<'js>(ctx: &Ctx<'js>, select: dom::NodeId, element: dom::NodeId, before: Value<'js>) -> Result<()> {
    let converted: rquickjs::Coerced<i32> = rquickjs::FromJs::from_js(ctx, before)?;
    let reference = select_option_at(ctx, select, converted.0)?;
    let Some(reference) = reference else {
        return append_add_element(ctx, select, element);
    };
    let parent = {
        let registry = super::realm_registry(ctx)?;
        let Some(owner) = registry.borrow().owner_world(select) else {
            return Ok(());
        };
        let owner = owner.borrow();
        let Some(parsed) = owner.document(select) else {
            return Ok(());
        };
        parsed.document.parent(reference)
    };
    let Some(parent) = parent else {
        return Ok(());
    };
    let adopted = super::clone::adopt_across_documents(ctx, parent, element)?;
    let registry = super::realm_registry(ctx)?;
    let Some(owner) = registry.borrow().owner_world(select) else {
        return Ok(());
    };
    let owner = owner.borrow();
    let Some(mut parsed) = owner.document_mut(select) else {
        return Ok(());
    };
    dom::mutation::insert_before(&mut parsed.document, reference, adopted)
        .map_err(|err| super::throw_dom_error(ctx, err))?;
    drop(parsed);
    drop(owner);
    super::mutation::schedule_mutation_delivery(ctx)?;
    Ok(())
}

fn select_option_at(ctx: &Ctx<'_>, select: dom::NodeId, before_index: i32) -> Result<Option<dom::NodeId>> {
    let registry = super::realm_registry(ctx)?;
    let Some(owner) = registry.borrow().owner_world(select) else {
        return Ok(None);
    };
    let owner = owner.borrow();
    let Some(parsed) = owner.document(select) else {
        return Ok(None);
    };
    let options = dom::form::select_options(&parsed.document, select);
    if before_index < 0 {
        return Ok(None);
    }
    let Ok(index) = usize::try_from(before_index) else {
        return Ok(None);
    };
    Ok(options.get(index).copied())
}

fn is_select_descendant(
    select_owner: &std::rc::Rc<std::cell::RefCell<crate::js::world::World>>,
    select: dom::NodeId,
    descendant: dom::NodeId,
) -> bool {
    let select_owner = select_owner.borrow();
    let Some(parsed) = select_owner.document(select) else {
        return false;
    };
    let mut current = Some(descendant);
    while let Some(id) = current {
        if id == select {
            return true;
        }
        current = parsed.document.parent(id);
    }
    false
}

fn replace_collection_option(
    ctx: &Ctx<'_>,
    select: dom::NodeId,
    index: usize,
    option: dom::NodeId,
) -> Result<()> {
    let registry = super::realm_registry(ctx)?;
    let (existing, parent) = {
        let Some(owner) = registry.borrow().owner_world(select) else {
            return Ok(());
        };
        let owner = owner.borrow();
        let Some(parsed) = owner.document(select) else {
            return Ok(());
        };
        let options = dom::form::select_options(&parsed.document, select);
        let Some(&existing) = options.get(index) else {
            return Ok(());
        };
        (existing, parsed.document.parent(existing))
    };
    let Some(parent) = parent else {
        return Ok(());
    };
    let adopted = super::clone::adopt_across_documents(ctx, parent, option)?;
    let Some(owner) = registry.borrow().owner_world(select) else {
        return Ok(());
    };
    let owner = owner.borrow();
    let Some(mut parsed) = owner.document_mut(select) else {
        return Ok(());
    };
    dom::mutation::replace_child(&mut parsed.document, parent, adopted, existing)
        .map_err(|err| super::throw_dom_error(ctx, err))?;
    drop(parsed);
    drop(owner);
    super::mutation::schedule_mutation_delivery(ctx)?;
    Ok(())
}

fn grow_collection_options(ctx: &Ctx<'_>, select: dom::NodeId, delta: usize) -> Result<()> {
    let registry = super::realm_registry(ctx)?;
    let Some(owner) = registry.borrow().owner_world(select) else {
        return Ok(());
    };
    let owner = owner.borrow();
    let Some(mut parsed) = owner.document_mut(select) else {
        return Ok(());
    };
    dom::form::append_blank_options(&mut parsed.document, select, delta)
        .map_err(|_| Exception::throw_type(ctx, "not a select"))?;
    drop(parsed);
    drop(owner);
    super::mutation::schedule_mutation_delivery(ctx)?;
    Ok(())
}

fn append_collection_option(ctx: &Ctx<'_>, select: dom::NodeId, option: dom::NodeId) -> Result<()> {
    let adopted = super::clone::adopt_across_documents(ctx, select, option)?;
    let registry = super::realm_registry(ctx)?;
    let Some(owner) = registry.borrow().owner_world(select) else {
        return Ok(());
    };
    let owner = owner.borrow();
    let Some(mut parsed) = owner.document_mut(select) else {
        return Ok(());
    };
    dom::mutation::append(&mut parsed.document, select, adopted)
        .map_err(|err| super::throw_dom_error(ctx, err))?;
    drop(parsed);
    drop(owner);
    super::mutation::schedule_mutation_delivery(ctx)?;
    Ok(())
}

pub(crate) fn install_collection_brand(ctx: &Ctx<'_>) -> Result<()> {
    crate::js::bridge::object(ctx)?
        .set("__tbWindowNamedValue", Func::from(window_named_value))?;
    crate::js::bridge::object(ctx)?
        .set("__tbWindowNamedHas", Func::from(window_named_has))?;
    crate::js::bridge::evaluate(ctx, install_collections_js(ctx)?)?;
    Ok(())
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
