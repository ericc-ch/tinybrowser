//! Live node collections (`NodeList`, `HTMLCollection`).

use super::{
    install_collections_js, live_collection, world, wrap_node,
};

use rquickjs::{
    Atom, Ctx, Exception, Object, Result, Value,
    class::{ExoticSetResult, Trace},
    prelude::Func,
};

use crate::js::world::{BlitzId, Handle, JournalEntry, NodeId, attr, html_namespace};
use crate::names::qualified_name_eq;
use markup5ever::{LocalName, QualName};
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
    fn ids(&self, ctx: &Ctx<'_>) -> Result<Vec<NodeId>> {
        query_ids(ctx, self.scope.0, &self.kind)
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

/// The live ids of `kind` scoped at `scope`, resolved through the agent's
/// document registry. Blitz trees are walked directly: descendants come from
/// `base.get_node` children stacks, and matching reads element names and
/// attributes off `ElementData`.
fn query_ids(ctx: &Ctx<'_>, scope: NodeId, kind: &CollectionKind) -> Result<Vec<NodeId>> {
    let registry = super::realm_registry(ctx)?;
    let Some(owner) = registry.borrow().owner_world(scope) else {
        return Ok(Vec::new());
    };
    let owner = owner.borrow();
    let Some(parsed) = owner.document(scope) else {
        return Ok(Vec::new());
    };
    let base = &parsed.document.base;
    let document = scope.document;
    Ok(match kind {
        CollectionKind::Children => crate::js::world::child_ids(base, document, scope.node),
        CollectionKind::ElementChildren => crate::js::world::child_ids(base, document, scope.node)
            .into_iter()
            .filter(|kid| super::is_element(base, kid.node))
            .collect(),
        CollectionKind::ElementsByTag(name) => collect_by_tag(base, document, scope.node, name),
        CollectionKind::ElementsByTagNs { namespace, local } => {
            collect_by_tag_ns(base, document, scope.node, namespace, local)
        }
        CollectionKind::ElementsByClass(names) => {
            collect_by_class(base, document, scope.node, names)
        }
        CollectionKind::ElementsByName(name) => {
            collect_by_name(base, document, scope.node, name)
        }
        CollectionKind::SelectOptions => select_options(base, document, scope.node),
        CollectionKind::SelectedOptions => select_options(base, document, scope.node)
            .into_iter()
            .filter(|option| option_selected(base, option.node))
            .collect(),
        CollectionKind::WindowNamed(name) => {
            collect_window_named(base, document, scope.node, name)
        }
        CollectionKind::Static(handles) => handles.iter().map(|handle| handle.0).collect(),
    })
}

/// Descendant Blitz ids of `scope` in tree order, excluding `scope` itself.
fn descendants(base: &blitz_dom::BaseDocument, scope: BlitzId) -> Vec<BlitzId> {
    let mut order = Vec::new();
    let mut stack: Vec<BlitzId> = base
        .get_node(scope)
        .map(|node| node.children.iter().rev().copied().collect())
        .unwrap_or_default();
    while let Some(id) = stack.pop() {
        order.push(id);
        if let Some(node) = base.get_node(id) {
            stack.extend(node.children.iter().rev().copied());
        }
    }
    order
}

fn element_name(base: &blitz_dom::BaseDocument, id: BlitzId) -> Option<&QualName> {
    base.get_node(id)
        .and_then(|node| node.data.downcast_element())
        .map(|element| &element.name)
}

/// Whether `id` is an `option` element, the Blitz stand-in for the options
/// list. Blitz keeps no form model, so a select's options are simply the
/// `option` elements in its subtree, in tree order; optgroup nesting is
/// covered because the walk descends.
fn is_option_element(base: &blitz_dom::BaseDocument, id: BlitzId) -> bool {
    element_name(base, id).is_some_and(|name| {
        name.ns == html_namespace() && name.local.as_ref() == "option"
    })
}

/// A select's options: the `option` elements in its subtree, in tree order.
fn select_options(
    base: &blitz_dom::BaseDocument,
    document: u32,
    select: BlitzId,
) -> Vec<NodeId> {
    descendants(base, select)
        .into_iter()
        .filter(|&id| is_option_element(base, id))
        .map(|node| NodeId { document, node })
        .collect()
}

/// Selectedness is the presence of the `selected` attribute; Blitz keeps no
/// selectedness slot.
fn option_selected(base: &blitz_dom::BaseDocument, id: BlitzId) -> bool {
    attr(base, id, "selected").is_some()
}

/// The index of the first selected option, or `-1`
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-selectedindex>).
fn select_selected_index(
    base: &blitz_dom::BaseDocument,
    document: u32,
    select: BlitzId,
) -> i32 {
    select_options(base, document, select)
        .iter()
        .position(|option| option_selected(base, option.node))
        .map_or(-1, |index| i32::try_from(index).unwrap_or(-1))
}

/// Sets an option's selectedness through its `selected` content attribute,
/// reusing the stored qualified name so clearing removes the exact attribute
/// the parser kept (see `forms.rs` for the same pattern).
fn set_option_selected(parsed: &mut crate::Parsed, option: NodeId, selected: bool) {
    let (name, old_value) = {
        let base = &parsed.document.base;
        let Some(node) = base.get_node(option.node) else {
            return;
        };
        let Some(element) = node.data.downcast_element() else {
            return;
        };
        let name = element
            .attrs
            .iter()
            .find(|attribute| attribute.name.local.as_ref() == "selected")
            .map_or_else(
                || {
                    QualName::new(
                        None,
                        markup5ever::Namespace::from(""),
                        LocalName::from("selected"),
                    )
                },
                |attribute| attribute.name.clone(),
            );
        (name, attr(base, option.node, "selected").map(str::to_owned))
    };
    if selected == old_value.is_some() {
        return;
    }
    if selected {
        parsed
            .document
            .base
            .mutate()
            .set_attribute(option.node, name, "");
    } else {
        parsed
            .document
            .base
            .mutate()
            .clear_attribute(option.node, name);
    }
    parsed.document.record(JournalEntry::Attributes {
        target: option,
        name: "selected".to_owned(),
        namespace: String::new(),
        old_value,
    });
}

/// Sets the select's selected option by index; a negative index clears every
/// option. Blitz has no select value slot or dirtiness flag, so the
/// attributes are the whole state.
fn set_select_selected_index(
    parsed: &mut crate::Parsed,
    document: u32,
    select: BlitzId,
    value: i32,
) {
    let options = {
        let base = &parsed.document.base;
        select_options(base, document, select)
    };
    if value < 0 {
        for option in options {
            set_option_selected(parsed, option, false);
        }
        return;
    }
    let wanted = usize::try_from(value).unwrap_or(usize::MAX);
    for (index, option) in options.into_iter().enumerate() {
        set_option_selected(parsed, option, index == wanted);
    }
}

fn collect_by_tag(
    base: &blitz_dom::BaseDocument,
    document: u32,
    scope: BlitzId,
    name: &str,
) -> Vec<NodeId> {
    // In an HTML document, an HTML-namespace element matches the queried
    // name ASCII-lowercased; other elements match the name exactly
    // (<https://dom.spec.whatwg.org/#concept-getelementsbytagname>).
    let lowered = name.to_ascii_lowercase();
    descendants(base, scope)
        .into_iter()
        .filter(|&id| {
            let Some(qual) = element_name(base, id) else {
                return false;
            };
            name == "*"
                || if qual.ns == html_namespace() {
                    qualified_name_eq(qual, &lowered)
                } else {
                    qualified_name_eq(qual, name)
                }
        })
        .map(|node| NodeId { document, node })
        .collect()
}

fn collect_by_name(
    base: &blitz_dom::BaseDocument,
    document: u32,
    scope: BlitzId,
    name: &str,
) -> Vec<NodeId> {
    descendants(base, scope)
        .into_iter()
        .filter(|&id| {
            super::is_element(base, id) && attr(base, id, "name").is_some_and(|value| value == name)
        })
        .map(|node| NodeId { document, node })
        .collect()
}

/// The Window named objects with `name`: every element whose ID is `name`,
/// plus `embed`, `form`, `img`, and `object` elements whose `name` is `name`,
/// in tree order
/// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#named-access-on-the-window-object>).
fn collect_window_named(
    base: &blitz_dom::BaseDocument,
    document: u32,
    scope: BlitzId,
    name: &str,
) -> Vec<NodeId> {
    if name.is_empty() {
        return Vec::new();
    }
    descendants(base, scope)
        .into_iter()
        .filter(|&id| {
            if attr(base, id, "id").is_some_and(|value| value == name) {
                return true;
            }
            element_name(base, id).is_some_and(|qual| {
                qual.ns == html_namespace()
                    && matches!(qual.local.as_ref(), "embed" | "form" | "img" | "object")
                    && attr(base, id, "name").is_some_and(|value| value == name)
            })
        })
        .map(|node| NodeId { document, node })
        .collect()
}

/// Whether a named getter exposes `name`: some descendant carries it as `id`,
/// or an `embed`/`form`/`img`/`object` carries it as `name`.
fn named_exists(base: &blitz_dom::BaseDocument, scope: BlitzId, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    descendants(base, scope).into_iter().any(|id| {
        if attr(base, id, "id").is_some_and(|value| value == name) {
            return true;
        }
        element_name(base, id).is_some_and(|qual| {
            qual.ns == html_namespace()
                && matches!(qual.local.as_ref(), "embed" | "form" | "img" | "object")
                && attr(base, id, "name").is_some_and(|value| value == name)
        })
    })
}

fn collect_by_tag_ns(
    base: &blitz_dom::BaseDocument,
    document: u32,
    scope: BlitzId,
    namespace: &str,
    local: &str,
) -> Vec<NodeId> {
    descendants(base, scope)
        .into_iter()
        .filter(|&id| {
            element_name(base, id).is_some_and(|name| {
                (namespace == "*" || name.ns.as_ref() == namespace)
                    && (local == "*" || name.local.as_ref() == local)
            })
        })
        .map(|node| NodeId { document, node })
        .collect()
}

fn collect_by_class(
    base: &blitz_dom::BaseDocument,
    document: u32,
    scope: BlitzId,
    names: &str,
) -> Vec<NodeId> {
    let wanted: Vec<&str> = names.split_ascii_whitespace().collect();
    // An empty class set matches nothing
    // (<https://dom.spec.whatwg.org/#concept-getelementsbyclassname>).
    if wanted.is_empty() {
        return Vec::new();
    }
    descendants(base, scope)
        .into_iter()
        .filter(|&id| {
            if !super::is_element(base, id) {
                return false;
            }
            let classes = attr(base, id, "class").unwrap_or_default();
            let tokens: Vec<&str> = classes.split_ascii_whitespace().collect();
            wanted.iter().all(|want| tokens.contains(want))
        })
        .map(|node| NodeId { document, node })
        .collect()
}

/// The siblings around `child` within `parent`, for the mutation journal.
fn siblings_around(
    base: &blitz_dom::BaseDocument,
    document: u32,
    parent: BlitzId,
    child: BlitzId,
) -> (Option<NodeId>, Option<NodeId>) {
    let mut previous = None;
    let mut next = None;
    if let Some(node) = base.get_node(parent) {
        let mut seen = false;
        for &kid in &node.children {
            if kid == child {
                seen = true;
                continue;
            }
            let id = NodeId { document, node: kid };
            if seen && next.is_none() {
                next = Some(id);
                break;
            }
            if !seen {
                previous = Some(id);
            }
        }
    }
    (previous, next)
}

/// Detaches `target` from its parent, recording the `childList` journal entry
/// observers deliver. Blitz performs no hierarchy validation here; callers
/// establish the parent first.
fn detach_node(parsed: &mut crate::Parsed, target: NodeId) {
    let document = target.document;
    let (parent, previous, next) = {
        let base = &parsed.document.base;
        let parent = base.get_node(target.node).and_then(|node| node.parent);
        let (previous, next) = parent.map_or((None, None), |parent| {
            siblings_around(base, document, parent, target.node)
        });
        (parent, previous, next)
    };
    parsed.document.base.mutate().remove_node(target.node);
    if let Some(parent) = parent {
        let target_parent = NodeId { document, node: parent };
        parsed.document.record(JournalEntry::ChildList {
            target: target_parent,
            added: Vec::new(),
            removed: vec![target],
            previous,
            next,
        });
    }
}

/// Appends `child` to `parent`, recording the `childList` journal entry.
fn append_node(parsed: &mut crate::Parsed, parent: NodeId, child: NodeId) {
    let previous = {
        let base = &parsed.document.base;
        base.get_node(parent.node)
            .and_then(|node| node.children.last().copied())
            .map(|node| NodeId {
                document: parent.document,
                node,
            })
    };
    parsed
        .document
        .base
        .mutate()
        .append_children(parent.node, &[child.node]);
    parsed.document.record(JournalEntry::ChildList {
        target: parent,
        added: vec![child],
        removed: Vec::new(),
        previous,
        next: None,
    });
}

/// Inserts `child` before `reference`, recording the `childList` journal
/// entry on the reference's parent.
fn insert_node_before(parsed: &mut crate::Parsed, reference: NodeId, child: NodeId) {
    let document = reference.document;
    let (parent, previous) = {
        let base = &parsed.document.base;
        let parent = base.get_node(reference.node).and_then(|node| node.parent);
        let previous =
            parent.and_then(|parent| siblings_around(base, document, parent, reference.node).0);
        (parent, previous)
    };
    let Some(parent) = parent else {
        return;
    };
    parsed
        .document
        .base
        .mutate()
        .insert_nodes_before(reference.node, &[child.node]);
    parsed.document.record(JournalEntry::ChildList {
        target: NodeId { document, node: parent },
        added: vec![child],
        removed: Vec::new(),
        previous,
        next: Some(reference),
    });
}

/// Replaces `old` with `new` in its parent, recording the `childList` journal
/// entry.
fn replace_node(parsed: &mut crate::Parsed, old: NodeId, new: NodeId) {
    let document = old.document;
    let (parent, previous, next) = {
        let base = &parsed.document.base;
        let parent = base.get_node(old.node).and_then(|node| node.parent);
        let (previous, next) = parent.map_or((None, None), |parent| {
            siblings_around(base, document, parent, old.node)
        });
        (parent, previous, next)
    };
    let Some(parent) = parent else {
        return;
    };
    parsed
        .document
        .base
        .mutate()
        .replace_node_with(old.node, &[new.node]);
    parsed.document.record(JournalEntry::ChildList {
        target: NodeId { document, node: parent },
        added: vec![new],
        removed: vec![old],
        previous,
        next,
    });
}

/// Creates a blank `option` element for a growing options collection.
fn create_option_element(parsed: &mut crate::Parsed, document: u32) -> NodeId {
    let name = QualName::new(None, html_namespace(), LocalName::from("option"));
    let node = parsed.document.base.mutate().create_element(name, Vec::new());
    NodeId { document, node }
}

/// Appends `count` blank options to `select`.
fn append_blank_options(parsed: &mut crate::Parsed, select: NodeId, count: usize) {
    for _ in 0..count {
        let option = create_option_element(parsed, select.document);
        append_node(parsed, select, option);
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
///
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

impl<'js> node_list_generated::NodeList<'js> for JsNodeList {
    // https://dom.spec.whatwg.org/#dom-nodelist-length
    fn get_length(&self, ctx: &Ctx<'js>) -> Result<usize> {
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
                .base
                .get_node(self.query.scope.0.node)
                .map_or(0, |node| node.children.len()),
            CollectionKind::ElementsByName(name) => {
                let base = &parsed.document.base;
                let name = name.clone();
                descendants(base, self.query.scope.0.node)
                    .into_iter()
                    .filter(|&id| {
                        super::is_element(base, id)
                            && attr(base, id, "name").is_some_and(|value| value == name)
                    })
                    .count()
            }
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
    fn item(&self, ctx: Ctx<'js>, index: u32) -> Result<Value<'js>> {
        self.query.item(&ctx, index as usize)
    }
}

#[derive(Trace, rquickjs::JsLifetime)]
pub(crate) struct JsHtmlCollection {
    pub(crate) query: CollectionQuery,
}

include!(concat!(env!("OUT_DIR"), "/HTMLCollection.rs"));

impl<'js> html_collection_generated::HTMLCollection<'js> for JsHtmlCollection {
    // https://dom.spec.whatwg.org/#dom-htmlcollection-length
    fn get_length(&self, ctx: &Ctx<'js>) -> Result<usize> {
        self.query.ids(ctx).map(|ids| ids.len())
    }

    // https://dom.spec.whatwg.org/#dom-htmlcollection-item
    fn item(&self, ctx: Ctx<'js>, index: u32) -> Result<Value<'js>> {
        self.query.item(&ctx, index as usize)
    }

    fn named_item(&self, ctx: Ctx<'js>, name: rquickjs::String<'js>) -> Result<Value<'js>> {
        named_item(&ctx, &self.query, &name.to_string()?)
    }

    fn supported_names(&self, ctx: &Ctx<'js>) -> Result<Vec<String>> {
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
        let current = {
            let base = &parsed.document.base;
            select_options(base, select.document, select.node).len()
        };
        if length == current {
            return Ok(());
        }
        if length > current {
            if length > 100_000 {
                return Ok(());
            }
            append_blank_options(&mut parsed, select, length - current);
            drop(parsed);
            drop(owner);
            super::mutation::schedule_mutation_delivery(ctx)?;
            return Ok(());
        }
        let removed: Vec<NodeId> = {
            let base = &parsed.document.base;
            select_options(base, select.document, select.node)[length..].to_vec()
        };
        for option in removed {
            detach_node(&mut parsed, option);
        }
        drop(parsed);
        drop(owner);
        super::mutation::schedule_mutation_delivery(ctx)?;
        Ok(())
    }

    // https://html.spec.whatwg.org/multipage/common-dom-interfaces.html#dom-htmloptionscollection-add
    fn add(
        &self,
        ctx: Ctx<'_>,
        element: html_options_collection_generated::HTMLOptGroupElementOrHTMLOptionElement,
        before: Option<html_options_collection_generated::HTMLElementOrLong>,
    ) -> Result<()> {
        use html_options_collection_generated::{
            HTMLElementOrLong, HTMLOptGroupElementOrHTMLOptionElement,
        };
        use super::host::NodeReference;
        let select = self.query.scope.0;
        // The union conversion already brand-checks the element, so only a
        // tree node arrives here.
        let element = match element {
            HTMLOptGroupElementOrHTMLOptionElement::HTMLOptGroupElement(reference)
            | HTMLOptGroupElementOrHTMLOptionElement::HTMLOptionElement(reference) => reference,
        };
        let NodeReference::Tree(element) = element else {
            return Err(Exception::throw_type(
                &ctx,
                "add requires an option or optgroup element",
            ));
        };
        match before {
            None => append_add_element(&ctx, select, element),
            Some(HTMLElementOrLong::HTMLElement(NodeReference::Tree(before))) => {
                if before == element {
                    return Ok(());
                }
                insert_before_element(&ctx, select, element, before)
            }
            Some(HTMLElementOrLong::HTMLElement(_)) => Err(Exception::throw_type(
                &ctx,
                "add requires an option or optgroup element",
            )),
            Some(HTMLElementOrLong::Long(index)) => {
                insert_before_index(&ctx, select, element, index)
            }
        }
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
            let base = &parsed.document.base;
            select_options(base, select.document, select.node)
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
        detach_node(&mut parsed, target);
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
            let base = &parsed.document.base;
            select_selected_index(base, select.document, select.node)
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
        set_select_selected_index(&mut parsed, select.document, select.node, value);
        Ok(())
    }
}
impl<'js> html_options_collection_generated::HTMLOptionsCollection<'js> for JsOptionsCollection {

    // https://html.spec.whatwg.org/multipage/common-dom-interfaces.html#dom-htmloptionscollection-length
    fn get_length(&self, ctx: &Ctx<'js>) -> Result<usize> {
        self.query.ids(ctx).map(|ids| ids.len())
    }

    // Inherited `item` installs on this prototype too, derived from the
    // parent chain instead of repeated by hand.
    // https://dom.spec.whatwg.org/#dom-htmlcollection-item
    fn item(&self, ctx: Ctx<'js>, arg_0: u32) -> Result<Value<'js>> {
        self.query.item(&ctx, arg_0 as usize)
    }

    // https://dom.spec.whatwg.org/#dom-htmlcollection-nameditem
    fn named_item(
        &self,
        ctx: Ctx<'js>,
        arg_0: rquickjs::String<'js>,
    ) -> Result<Value<'js>> {
        named_item(&ctx, &self.query, &arg_0.to_string()?)
    }

    // https://html.spec.whatwg.org/multipage/common-dom-interfaces.html#dom-htmloptionscollection-length
    fn set_length(&self, ctx: &Ctx<'js>, value: u32) -> Result<()> {
        self.set_length(ctx, value)
    }

    fn supported_names(&self, ctx: &Ctx<'js>) -> Result<Vec<String>> {
        named_keys(ctx, &self.query.ids(ctx)?)
    }

    // https://html.spec.whatwg.org/multipage/common-dom-interfaces.html#dom-htmloptionscollection-add
    fn add(
        &self,
        ctx: Ctx<'js>,
        arg_0: html_options_collection_generated::HTMLOptGroupElementOrHTMLOptionElement,
        arg_1: Option<html_options_collection_generated::HTMLElementOrLong>,
    ) -> Result<()> {
        self.add(ctx, arg_0, arg_1)
    }

    // https://html.spec.whatwg.org/multipage/common-dom-interfaces.html#htmloptionscollection-set-indexed
    fn set_indexed(&self, ctx: Ctx<'js>, index: u32, value: Value<'js>) -> Result<()> {
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
            let base = &parsed.document.base;
            select_options(base, select.document, select.node).len()
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


    // https://html.spec.whatwg.org/multipage/common-dom-interfaces.html#dom-htmloptionscollection-remove
    fn remove(&self, ctx: Ctx<'js>, arg_0: i32) -> Result<()> {
        self.remove(ctx, arg_0)
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-selectedindex
    fn get_selected_index(&self, ctx: &Ctx<'js>) -> Result<i32> {
        self.selected_index(ctx)
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-selectedindex
    fn set_selected_index(&self, ctx: &Ctx<'js>, value: i32) -> Result<()> {
        self.set_selected_index(ctx, value)
    }
}

fn require_option_element<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<NodeId> {
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
    if is_option_element(&parsed.document.base, option.node) {
        Ok(option)
    } else {
        Err(Exception::throw_type(ctx, "option must be an HTMLOptionElement"))
    }
}

fn append_add_element(ctx: &Ctx<'_>, select: NodeId, element: NodeId) -> Result<()> {
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
    let parent = parsed
        .document
        .base
        .get_node(adopted.node)
        .and_then(|node| node.parent)
        .map(|parent| NodeId {
            document: adopted.document,
            node: parent,
        });
    if parent == Some(select) {
        return Ok(());
    }
    append_node(&mut parsed, select, adopted);
    drop(parsed);
    drop(owner);
    super::mutation::schedule_mutation_delivery(ctx)?;
    Ok(())
}

fn insert_before_element(
    ctx: &Ctx<'_>,
    select: NodeId,
    element: NodeId,
    before_id: NodeId,
) -> Result<()> {
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
        parsed
            .document
            .base
            .get_node(before_id.node)
            .and_then(|node| node.parent)
            .map(|parent| NodeId {
                document: before_id.document,
                node: parent,
            })
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
    insert_node_before(&mut parsed, before_id, adopted);
    drop(parsed);
    drop(owner);
    super::mutation::schedule_mutation_delivery(ctx)?;
    Ok(())
}

fn insert_before_index(ctx: &Ctx<'_>, select: NodeId, element: NodeId, index: i32) -> Result<()> {
    let reference = select_option_at(ctx, select, index)?;
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
        parsed
            .document
            .base
            .get_node(reference.node)
            .and_then(|node| node.parent)
            .map(|parent| NodeId {
                document: reference.document,
                node: parent,
            })
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
    insert_node_before(&mut parsed, reference, adopted);
    drop(parsed);
    drop(owner);
    super::mutation::schedule_mutation_delivery(ctx)?;
    Ok(())
}

fn select_option_at(ctx: &Ctx<'_>, select: NodeId, before_index: i32) -> Result<Option<NodeId>> {
    let registry = super::realm_registry(ctx)?;
    let Some(owner) = registry.borrow().owner_world(select) else {
        return Ok(None);
    };
    let owner = owner.borrow();
    let Some(parsed) = owner.document(select) else {
        return Ok(None);
    };
    let base = &parsed.document.base;
    let options = select_options(base, select.document, select.node);
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
    select: NodeId,
    descendant: NodeId,
) -> bool {
    let select_owner = select_owner.borrow();
    let Some(parsed) = select_owner.document(select) else {
        return false;
    };
    let base = &parsed.document.base;
    let mut current = Some(descendant.node);
    while let Some(id) = current {
        if id == select.node {
            return true;
        }
        current = base.get_node(id).and_then(|node| node.parent);
    }
    false
}

fn replace_collection_option(
    ctx: &Ctx<'_>,
    select: NodeId,
    index: usize,
    option: NodeId,
) -> Result<()> {
    let registry = super::realm_registry(ctx)?;
    let existing = {
        let Some(owner) = registry.borrow().owner_world(select) else {
            return Ok(());
        };
        let owner = owner.borrow();
        let Some(parsed) = owner.document(select) else {
            return Ok(());
        };
        let base = &parsed.document.base;
        let options = select_options(base, select.document, select.node);
        let Some(&existing) = options.get(index) else {
            return Ok(());
        };
        let parent = base
            .get_node(existing.node)
            .and_then(|node| node.parent)
            .map(|parent| NodeId {
                document: existing.document,
                node: parent,
            });
        parent.map(|parent| (existing, parent))
    };
    let Some((existing, parent)) = existing else {
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
    replace_node(&mut parsed, existing, adopted);
    drop(parsed);
    drop(owner);
    super::mutation::schedule_mutation_delivery(ctx)?;
    Ok(())
}

fn grow_collection_options(ctx: &Ctx<'_>, select: NodeId, delta: usize) -> Result<()> {
    let registry = super::realm_registry(ctx)?;
    let Some(owner) = registry.borrow().owner_world(select) else {
        return Ok(());
    };
    let owner = owner.borrow();
    let Some(mut parsed) = owner.document_mut(select) else {
        return Ok(());
    };
    append_blank_options(&mut parsed, select, delta);
    drop(parsed);
    drop(owner);
    super::mutation::schedule_mutation_delivery(ctx)?;
    Ok(())
}

fn append_collection_option(ctx: &Ctx<'_>, select: NodeId, option: NodeId) -> Result<()> {
    let adopted = super::clone::adopt_across_documents(ctx, select, option)?;
    let registry = super::realm_registry(ctx)?;
    let Some(owner) = registry.borrow().owner_world(select) else {
        return Ok(());
    };
    let owner = owner.borrow();
    let Some(mut parsed) = owner.document_mut(select) else {
        return Ok(());
    };
    append_node(&mut parsed, select, adopted);
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
    let ids = query_ids(&ctx, document, &CollectionKind::WindowNamed(name.clone()))?;
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
    let Some(parsed) = world.main_document() else {
        return Ok(false);
    };
    let base = &parsed.document.base;
    Ok(named_exists(base, base.root_node().id, &name))
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
            attr(&parsed.document.base, id.node, "id").is_some_and(|value| value == name)
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
            let base = &parsed.document.base;
            element_name(base, id.node).is_some_and(|qual| qual.ns == html_namespace())
                && attr(base, id.node, "name").is_some_and(|value| value == name)
        };
        if matches {
            return wrap_node(ctx, *id);
        }
    }
    Ok(Value::new_null(ctx.clone()))
}

/// The supported named properties of an `HTMLCollection`, in tree order.
/// <https://dom.spec.whatwg.org/#interface-htmlcollection>
fn named_keys(ctx: &Ctx<'_>, ids: &[NodeId]) -> Result<Vec<String>> {
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
            let base = &parsed.document.base;
            if let Some(element_id) = attr(base, id.node, "id")
                && !element_id.is_empty()
                && seen.insert(element_id.to_owned())
            {
                keys.push(element_id.to_owned());
            }
            let exposes_name = element_name(base, id.node)
                .is_some_and(|qual| qual.ns == html_namespace());
            if exposes_name
                && let Some(element_name) = attr(base, id.node, "name")
                && !element_name.is_empty()
                && seen.insert(element_name.to_owned())
            {
                keys.push(element_name.to_owned());
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
