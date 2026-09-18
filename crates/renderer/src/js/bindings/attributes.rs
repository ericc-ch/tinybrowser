//! Attribute, class, and handler-attribute objects and plumbing.

use super::{
    FromJs, OptString, WebIdlString, child_value, deref_weak, element_is_html, is_html_element,
    make_weak, qualified_name, schedule_mutation_delivery, string_value, throw_dom,
    throw_dom_error, with_node_kind, world, world_for_node, wrap_node,
};
use rquickjs::function::{Opt, Rest};

use std::cell::RefCell;

use std::rc::Rc;

use dom::{NodeId, qualified_name_eq};

use rquickjs::{Class, Ctx, Exception, Function, Persistent, Result, Value, class::Trace};

use crate::js::events::report_exception;
use crate::js::world::{AttrState, FrameNavigation, Handle, World, Wrapper};

/// `DOMTokenList` for `Element.classList`
/// (<https://dom.spec.whatwg.org/#interface-domtokenlist>).
#[derive(Trace, rquickjs::JsLifetime)]
#[rquickjs::class(rename = "DOMTokenList")]
pub struct JsTokenList {
    pub(crate) element: Handle,
}

#[rquickjs::methods]
#[allow(
    clippy::needless_pass_by_value,
    clippy::unused_self,
    reason = "rquickjs method ABI passes Ctx by value; DOMTokenList methods may ignore self"
)]
impl JsTokenList {
    #[qjs(constructor)]
    fn ctor(ctx: Ctx<'_>) -> Result<Self> {
        Err(Exception::throw_type(&ctx, "Illegal constructor"))
    }

    // https://dom.spec.whatwg.org/#dom-domtokenlist-length
    #[qjs(get)]
    fn length(&self, ctx: Ctx<'_>) -> Result<usize> {
        Ok(class_tokens(&ctx, self.element.0)?.len())
    }

    // https://dom.spec.whatwg.org/#dom-domtokenlist-value
    #[qjs(get)]
    fn value(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        Ok(world
            .borrow()
            .document(self.element.0)
            .and_then(|parsed| parsed.dom.attribute(self.element.0, "class"))
            .unwrap_or_default())
    }

    #[qjs(set, rename = "value")]
    fn set_value(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        write_class(&ctx, self.element.0, &value.0)
    }

    // https://dom.spec.whatwg.org/#dom-domtokenlist-item
    #[qjs(rename = "item")]
    fn item<'js>(&self, ctx: Ctx<'js>, index: i64) -> Result<Value<'js>> {
        let tokens = class_tokens(&ctx, self.element.0)?;
        match usize::try_from(index)
            .ok()
            .and_then(|index| tokens.get(index))
        {
            Some(token) => string_value(&ctx, token),
            None => Ok(Value::new_null(ctx)),
        }
    }

    // https://dom.spec.whatwg.org/#dom-domtokenlist-contains
    #[qjs(rename = "contains")]
    fn contains(&self, ctx: Ctx<'_>, token: WebIdlString) -> Result<bool> {
        // `contains` does not validate its argument
        // (<https://dom.spec.whatwg.org/#dom-domtokenlist-contains>).
        Ok(class_tokens(&ctx, self.element.0)?.contains(&token.0))
    }

    // https://dom.spec.whatwg.org/#dom-domtokenlist-add
    #[qjs(rename = "add")]
    fn add(&self, ctx: Ctx<'_>, tokens: Rest<WebIdlString>) -> Result<()> {
        let mut current = class_tokens(&ctx, self.element.0)?;
        for token in tokens.0 {
            validate_token(&ctx, &token.0)?;
            if !current.contains(&token.0) {
                current.push(token.0);
            }
        }
        // The update steps run even when no token was added, normalizing
        // the attribute (<https://dom.spec.whatwg.org/#dom-domtokenlist-add>).
        write_class_tokens(&ctx, self.element.0, &current)
    }

    // https://dom.spec.whatwg.org/#dom-domtokenlist-remove
    #[qjs(rename = "remove")]
    fn remove(&self, ctx: Ctx<'_>, tokens: Rest<WebIdlString>) -> Result<()> {
        let mut current = class_tokens(&ctx, self.element.0)?;
        for token in tokens.0 {
            validate_token(&ctx, &token.0)?;
            current.retain(|existing| existing != &token.0);
        }
        write_class_tokens(&ctx, self.element.0, &current)
    }

    // https://dom.spec.whatwg.org/#dom-domtokenlist-toggle
    #[qjs(rename = "toggle")]
    fn toggle(&self, ctx: Ctx<'_>, token: WebIdlString, force: Opt<bool>) -> Result<bool> {
        validate_token(&ctx, &token.0)?;
        let mut current = class_tokens(&ctx, self.element.0)?;
        let present = current.contains(&token.0);
        let should_be_present = force.0.unwrap_or(!present);
        if should_be_present != present {
            if should_be_present {
                current.push(token.0);
            } else {
                current.retain(|existing| existing != &token.0);
            }
            write_class_tokens(&ctx, self.element.0, &current)?;
        }
        Ok(should_be_present)
    }

    // https://dom.spec.whatwg.org/#dom-domtokenlist-replace
    #[qjs(rename = "replace")]
    fn replace(&self, ctx: Ctx<'_>, old: WebIdlString, new: WebIdlString) -> Result<bool> {
        validate_token_pair(&ctx, &old.0, &new.0)?;
        let mut current = class_tokens(&ctx, self.element.0)?;
        if !current.contains(&old.0) {
            return Ok(false);
        }
        for token in &mut current {
            if token == &old.0 {
                token.clone_from(&new.0);
            }
        }
        // Replacing can duplicate an existing token; the token set keeps the
        // first occurrence (https://github.com/whatwg/dom/issues/443).
        let mut deduped: Vec<String> = Vec::with_capacity(current.len());
        for token in current {
            if !deduped.contains(&token) {
                deduped.push(token);
            }
        }
        write_class_tokens(&ctx, self.element.0, &deduped)?;
        Ok(true)
    }

    // https://dom.spec.whatwg.org/#dom-domtokenlist-supports
    #[qjs(rename = "supports")]
    fn supports(&self, ctx: Ctx<'_>, _token: WebIdlString) -> Result<bool> {
        // The spec ends with "throw a TypeError"
        // (<https://dom.spec.whatwg.org/#dom-domtokenlist-supports>).
        Err(Exception::throw_type(
            &ctx,
            "DOMTokenList has no supported tokens",
        ))
    }

    // https://dom.spec.whatwg.org/#interface-domtokenlist: stringifier
    #[qjs(rename = "toString")]
    fn to_string_js(&self, ctx: Ctx<'_>) -> Result<String> {
        self.value(ctx)
    }
}

/// The element's class tokens: an ordered set, so duplicates collapse.
fn class_tokens(ctx: &Ctx<'_>, id: NodeId) -> Result<Vec<String>> {
    let world = world(ctx)?;
    let mut tokens: Vec<String> = Vec::new();
    for token in world
        .borrow()
        .document(id)
        .and_then(|parsed| parsed.dom.attribute(id, "class"))
        .unwrap_or_default()
        .split_ascii_whitespace()
    {
        if !tokens.iter().any(|existing| existing == token) {
            tokens.push(token.to_string());
        }
    }
    Ok(tokens)
}

/// [Token validation](https://dom.spec.whatwg.org/#validate-a-token).
fn validate_token(ctx: &Ctx<'_>, token: &str) -> Result<()> {
    if token.is_empty() {
        return Err(throw_dom(ctx, "SyntaxError", "token is empty"));
    }
    if token
        .chars()
        .any(|c| matches!(c, '\t' | '\n' | '\u{c}' | '\r' | ' '))
    {
        return Err(throw_dom(
            ctx,
            "InvalidCharacterError",
            "token contains ASCII whitespace",
        ));
    }
    Ok(())
}

/// `replace` validates emptiness for both tokens before whitespace
/// (<https://dom.spec.whatwg.org/#dom-domtokenlist-replace>).
fn validate_token_pair(ctx: &Ctx<'_>, old: &str, new: &str) -> Result<()> {
    if old.is_empty() || new.is_empty() {
        return Err(throw_dom(ctx, "SyntaxError", "token is empty"));
    }
    if old
        .chars()
        .chain(new.chars())
        .any(|c| matches!(c, '\t' | '\n' | '\u{c}' | '\r' | ' '))
    {
        return Err(throw_dom(
            ctx,
            "InvalidCharacterError",
            "token contains ASCII whitespace",
        ));
    }
    Ok(())
}

/// Writes the token list back as the class attribute. The update steps set
/// the attribute even when the list serializes to the empty string; when the
/// attribute does not exist and the value is empty they do nothing
/// (<https://dom.spec.whatwg.org/#concept-dtl-update>).
fn write_class_tokens(ctx: &Ctx<'_>, id: NodeId, tokens: &[String]) -> Result<()> {
    write_class(ctx, id, &tokens.join(" "))
}

fn write_class(ctx: &Ctx<'_>, id: NodeId, value: &str) -> Result<()> {
    let world = world(ctx)?;
    let world = world.borrow();
    let Some(mut parsed) = world.document_mut(id) else {
        return Ok(());
    };
    if value.is_empty() && parsed.dom.attribute(id, "class").is_none() {
        return Ok(());
    }
    parsed
        .dom
        .set_attribute(id, "class", value)
        .map_err(|err| throw_dom_error(ctx, err))?;
    drop(parsed);
    drop(world);
    schedule_mutation_delivery(ctx)
}

/// One `Attr` platform object
/// (<https://dom.spec.whatwg.org/#interface-attr>). Identity lives in the
/// World's attr registry so `el.attributes[0] === el.getAttributeNode(name)`.
#[derive(Trace, rquickjs::JsLifetime)]
#[rquickjs::class(rename = "Attr")]
pub struct JsAttr {
    pub(crate) id: u64,
    pub(crate) scope: Handle,
}

#[rquickjs::methods]
#[allow(
    clippy::needless_pass_by_value,
    clippy::unused_self,
    reason = "rquickjs method ABI passes Ctx by value; Attr methods may ignore self"
)]
impl JsAttr {
    #[qjs(constructor)]
    fn ctor(ctx: Ctx<'_>) -> Result<Self> {
        Err(Exception::throw_type(&ctx, "Illegal constructor"))
    }

    // https://dom.spec.whatwg.org/#dom-attr-name
    #[qjs(get)]
    fn name(&self, ctx: Ctx<'_>) -> Result<String> {
        Ok(attr_state(&ctx, self.scope.0, self.id)?.qualified)
    }

    // https://dom.spec.whatwg.org/#dom-attr-localname
    #[qjs(get, rename = "localName")]
    fn local_name(&self, ctx: Ctx<'_>) -> Result<String> {
        Ok(attr_state(&ctx, self.scope.0, self.id)?.local)
    }

    // https://dom.spec.whatwg.org/#dom-attr-prefix
    #[qjs(get)]
    fn prefix<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        match attr_state(&ctx, self.scope.0, self.id)?.prefix {
            Some(prefix) => string_value(&ctx, &prefix),
            None => Ok(Value::new_null(ctx)),
        }
    }

    // https://dom.spec.whatwg.org/#dom-attr-namespaceuri
    #[qjs(get, rename = "namespaceURI")]
    fn namespace_uri<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let namespace = attr_state(&ctx, self.scope.0, self.id)?.namespace;
        if namespace.is_empty() {
            Ok(Value::new_null(ctx))
        } else {
            string_value(&ctx, &namespace)
        }
    }

    // https://dom.spec.whatwg.org/#dom-attr-value
    #[qjs(get)]
    fn value(&self, ctx: Ctx<'_>) -> Result<String> {
        attr_value(&ctx, self.scope.0, self.id)
    }

    #[qjs(set, rename = "value")]
    fn set_value(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        set_attr_value(&ctx, self.scope.0, self.id, value.0)
    }

    // https://dom.spec.whatwg.org/#dom-node-nodevalue
    #[qjs(get, rename = "nodeValue")]
    fn node_value(&self, ctx: Ctx<'_>) -> Result<String> {
        self.value(ctx)
    }

    #[qjs(set, rename = "nodeValue")]
    fn set_node_value(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        self.set_value(ctx, value)
    }

    // https://dom.spec.whatwg.org/#dom-node-textcontent
    #[qjs(get, rename = "textContent")]
    fn text_content(&self, ctx: Ctx<'_>) -> Result<String> {
        self.value(ctx)
    }

    #[qjs(set, rename = "textContent")]
    fn set_text_content(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        self.set_value(ctx, value)
    }

    // https://dom.spec.whatwg.org/#dom-attr-ownerelement
    #[qjs(get, rename = "ownerElement")]
    fn owner_element<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        child_value(&ctx, attr_owner(&ctx, self.scope.0, self.id))
    }

    // https://dom.spec.whatwg.org/#dom-attr-specified
    #[qjs(get)]
    fn specified(&self) -> bool {
        true
    }

    #[qjs(get, rename = "nodeType")]
    fn node_type(&self) -> i32 {
        2
    }

    #[qjs(get, rename = "nodeName")]
    fn node_name(&self, ctx: Ctx<'_>) -> Result<String> {
        self.name(ctx)
    }

    #[qjs(get, rename = "ownerDocument")]
    fn owner_document<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        // An attached Attr belongs to its owner element's document; a
        // detached one reports the main document.
        let document = attr_owner(&ctx, self.scope.0, self.id).map_or_else(
            || {
                world_for_node(&ctx, self.scope.0).ok().and_then(|world| {
                    world
                        .borrow()
                        .with_document(self.scope.0, |parsed| parsed.dom.document())
                })
            },
            |owner| {
                world_for_node(&ctx, owner).ok().and_then(|world| {
                    world
                        .borrow()
                        .with_document(owner, |parsed| parsed.dom.document())
                })
            },
        );
        child_value(&ctx, document)
    }
}

/// `NamedNodeMap`, live over the element's attribute list
/// (<https://dom.spec.whatwg.org/#interface-namednodemap>).
#[derive(Trace, rquickjs::JsLifetime)]
#[rquickjs::class(rename = "NamedNodeMap")]
pub struct JsNamedNodeMap {
    pub(crate) element: Handle,
}

#[rquickjs::methods]
#[allow(
    clippy::needless_pass_by_value,
    clippy::unused_self,
    reason = "rquickjs method ABI passes Ctx by value"
)]
impl JsNamedNodeMap {
    #[qjs(constructor)]
    fn ctor(ctx: Ctx<'_>) -> Result<Self> {
        Err(Exception::throw_type(&ctx, "Illegal constructor"))
    }

    // https://dom.spec.whatwg.org/#dom-namednodemap-length
    #[qjs(get)]
    fn length(&self, ctx: Ctx<'_>) -> Result<usize> {
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.element.0) else {
            return Ok(0);
        };
        Ok(parsed
            .dom
            .attributes(self.element.0)
            .map_or(0, <[dom::Attribute]>::len))
    }

    // https://dom.spec.whatwg.org/#dom-namednodemap-item
    #[qjs(rename = "item")]
    fn item<'js>(&self, ctx: Ctx<'js>, index: i64) -> Result<Value<'js>> {
        let Some(attribute) = attribute_at(&ctx, self.element.0, index)? else {
            return Ok(Value::new_null(ctx));
        };
        match attached_attr_id(&ctx, self.element.0, &attribute.0, &attribute.1)? {
            Some(id) => attr_wrapper(&ctx, self.element.0, id),
            None => Ok(Value::new_null(ctx)),
        }
    }

    // https://dom.spec.whatwg.org/#dom-namednodemap-getnameditem
    #[qjs(rename = "getNamedItem")]
    fn get_named_item<'js>(&self, ctx: Ctx<'js>, name: WebIdlString) -> Result<Value<'js>> {
        named_item(&ctx, self.element.0, &name.0)
    }

    // https://dom.spec.whatwg.org/#dom-namednodemap-getnameditemns
    #[qjs(rename = "getNamedItemNS")]
    fn get_named_item_ns<'js>(
        &self,
        ctx: Ctx<'js>,
        namespace: OptString,
        local: WebIdlString,
    ) -> Result<Value<'js>> {
        let namespace = namespace.0.unwrap_or_default();
        match attached_attr_id(&ctx, self.element.0, &namespace, &local.0)? {
            Some(id) => attr_wrapper(&ctx, self.element.0, id),
            None => Ok(Value::new_null(ctx)),
        }
    }

    // https://dom.spec.whatwg.org/#dom-namednodemap-setnameditem
    #[qjs(rename = "setNamedItem")]
    fn set_named_item<'js>(&self, ctx: Ctx<'js>, attr: Value<'js>) -> Result<Value<'js>> {
        set_attribute_node(&ctx, self.element.0, &attr)
    }

    // https://dom.spec.whatwg.org/#dom-namednodemap-setnameditemns
    #[qjs(rename = "setNamedItemNS")]
    fn set_named_item_ns<'js>(&self, ctx: Ctx<'js>, attr: Value<'js>) -> Result<Value<'js>> {
        set_attribute_node(&ctx, self.element.0, &attr)
    }

    // https://dom.spec.whatwg.org/#dom-namednodemap-removenameditem
    #[qjs(rename = "removeNamedItem")]
    fn remove_named_item<'js>(&self, ctx: Ctx<'js>, name: WebIdlString) -> Result<Value<'js>> {
        let world_rc = world(&ctx)?;
        let Some((namespace, local, id)) =
            named_attribute_id(&ctx, &world_rc, self.element.0, &name.0)?
        else {
            return Err(throw_dom(&ctx, "NotFoundError", "no such attribute"));
        };
        let value = attr_wrapper(&ctx, self.element.0, id)?;
        remove_attribute_sync(&ctx, self.element.0, &namespace, &local, true)?;
        Ok(value)
    }

    // https://dom.spec.whatwg.org/#dom-namednodemap-removenameditemns
    #[qjs(rename = "removeNamedItemNS")]
    fn remove_named_item_ns<'js>(
        &self,
        ctx: Ctx<'js>,
        namespace: OptString,
        local: WebIdlString,
    ) -> Result<Value<'js>> {
        let namespace = namespace.0.unwrap_or_default();
        let value = match attached_attr_id(&ctx, self.element.0, &namespace, &local.0)? {
            Some(id) => attr_wrapper(&ctx, self.element.0, id)?,
            None => return Err(throw_dom(&ctx, "NotFoundError", "no such attribute")),
        };
        remove_attribute_sync(&ctx, self.element.0, &namespace, &local.0, true)?;
        Ok(value)
    }
}

/// `(namespace, local)` of the attribute at `index`, if any.
fn attribute_at(ctx: &Ctx<'_>, element: NodeId, index: i64) -> Result<Option<(String, String)>> {
    let world_rc = world_for_node(ctx, element)?;
    let world = world_rc.borrow();
    let Some(parsed) = world.document(element) else {
        return Ok(None);
    };
    let Some(list) = parsed.dom.attributes(element) else {
        return Ok(None);
    };
    Ok(usize::try_from(index)
        .ok()
        .and_then(|index| list.get(index))
        .map(|attribute| {
            (
                attribute.name.ns.to_string(),
                attribute.name.local.to_string(),
            )
        }))
}

/// The attribute with qualified name `name`, as an `Attr` wrapper.
fn named_item<'js>(ctx: &Ctx<'js>, element: NodeId, name: &str) -> Result<Value<'js>> {
    let world_rc = world_for_node(ctx, element)?;
    match named_attribute_id(ctx, &world_rc, element, name)? {
        Some((_, _, id)) => attr_wrapper(ctx, element, id),
        None => Ok(Value::new_null(ctx.clone())),
    }
}

/// The DOM local name `name` refers to on `element`: an HTML element coerces
/// to ASCII lowercase, any other element keeps the name
/// (<https://dom.spec.whatwg.org/#concept-element-attributes-get-by-name>).
pub(super) fn attribute_local_name(ctx: &Ctx<'_>, element: NodeId, name: &str) -> String {
    if element_is_html(ctx, element) {
        name.to_ascii_lowercase()
    } else {
        name.to_owned()
    }
}

/// The attached `Attr` id of the attribute with qualified name `name`, with
/// its `(namespace, local)` key. The world must be the one whose document
/// store the caller wants searched.
fn named_attribute_id(
    ctx: &Ctx<'_>,
    world_rc: &Rc<RefCell<World>>,
    element: NodeId,
    name: &str,
) -> Result<Option<(String, String, u64)>> {
    let name = attribute_local_name(ctx, element, name);
    let found = {
        let world = world_rc.borrow();
        let Some(parsed) = world.document(element) else {
            return Ok(None);
        };
        parsed.dom.attributes(element).and_then(|list| {
            list.iter()
                .find(|attribute| qualified_name_eq(&attribute.name, &name))
                .map(|attribute| {
                    (
                        attribute.name.ns.to_string(),
                        attribute.name.local.to_string(),
                    )
                })
        })
    };
    let Some((namespace, local)) = found else {
        return Ok(None);
    };
    Ok(attached_attr_id(ctx, element, &namespace, &local)?.map(|id| (namespace, local, id)))
}

// ── Attr registry helpers ────────────────────────────────────────────────

pub(crate) fn attr_state(ctx: &Ctx<'_>, scope: NodeId, id: u64) -> Result<AttrState> {
    let world_rc = world_for_node(ctx, scope)?;
    let world = world_rc.borrow();
    world
        .attrs
        .get(&id)
        .cloned()
        .ok_or_else(|| Exception::throw_type(ctx, "stale attribute"))
}

/// The attached element for `id`, or `None` when detached.
pub(crate) fn attr_owner(ctx: &Ctx<'_>, scope: NodeId, id: u64) -> Option<NodeId> {
    let world_rc = world_for_node(ctx, scope).ok()?;
    let world = world_rc.borrow();
    let owner = world.attr_owners.get(&id).copied().flatten()?;
    let state = world.attrs.get(&id)?;
    let parsed = world.document(owner)?;
    parsed
        .dom
        .attribute_ns(owner, &state.namespace, &state.local)
        .map(|_| owner)
}

fn attr_value(ctx: &Ctx<'_>, scope: NodeId, id: u64) -> Result<String> {
    if let Some(owner) = attr_owner(ctx, scope, id) {
        let world_rc = world_for_node(ctx, scope)?;
        let world = world_rc.borrow();
        if let Some(state) = world.attrs.get(&id)
            && let Some(parsed) = world.document(owner)
            && let Some(value) = parsed
                .dom
                .attribute_ns(owner, &state.namespace, &state.local)
        {
            return Ok(value);
        }
    }
    let world_rc = world_for_node(ctx, scope)?;
    Ok(world_rc
        .borrow()
        .attr_values
        .get(&id)
        .cloned()
        .unwrap_or_default())
}

fn set_attr_value(ctx: &Ctx<'_>, scope: NodeId, id: u64, value: String) -> Result<()> {
    let world_rc = world_for_node(ctx, scope)?;
    let mut world = world_rc.borrow_mut();
    world.attr_values.insert(id, value.clone());
    let Some(owner) = world.attr_owners.get(&id).copied().flatten() else {
        return Ok(());
    };
    let Some(state) = world.attrs.get(&id) else {
        return Ok(());
    };
    let (namespace, prefix, local) = (
        state.namespace.clone(),
        state.prefix.clone(),
        state.local.clone(),
    );
    let Some(mut parsed) = world.document_mut(owner) else {
        return Ok(());
    };
    parsed
        .dom
        .set_attribute_by_ns(owner, &namespace, prefix.as_deref(), &local, value)
        .map_err(|err| throw_dom_error(ctx, err))?;
    drop(parsed);
    drop(world);
    schedule_mutation_delivery(ctx)
}

/// Rebuilds the `NamedNodeMap` object's own index and named properties
/// (<https://dom.spec.whatwg.org/#interface-namednodemap>: named properties
/// never shadow interface members). HTML elements expose only qualified
/// names that survive ASCII lowercasing, since the named getter lowercases
/// (Firefox: `nsDOMAttributeMap::GetSupportedNames`).
pub(crate) fn refresh_named_node_map<'js>(
    ctx: &Ctx<'js>,
    element: NodeId,
    map: &Value<'js>,
) -> Result<()> {
    let html = element_is_html(ctx, element);
    let names: Vec<String> = {
        let world_rc = world_for_node(ctx, element)?;
        let world = world_rc.borrow();
        world
            .document(element)
            .map(|parsed| parsed.dom.attribute_names(element))
            .unwrap_or_default()
    };
    let refresh: Function = ctx.globals().get("__tb_refreshNamedNodeMap")?;
    refresh.call::<_, ()>((map.clone(), names, html))?;
    Ok(())
}

/// Refreshes the cached `NamedNodeMap` after a mutation, when one exists.
fn touch_named_node_map(ctx: &Ctx<'_>, element: NodeId) -> Result<()> {
    let world_rc = world_for_node(ctx, element)?;
    let Some(saved) = world_rc.borrow().wrapper(element, Wrapper::NamedNodeMap) else {
        return Ok(());
    };
    if let Some(value) = deref_weak(ctx, saved)? {
        refresh_named_node_map(ctx, element, &value)?;
    }
    Ok(())
}

/// The `Attr` id attached at `(element, namespace, local)`, creating the
/// platform object's identity on first access. Clears a stale mapping when
/// the attribute is gone.
pub(crate) fn attached_attr_id(
    ctx: &Ctx<'_>,
    element: NodeId,
    namespace: &str,
    local: &str,
) -> Result<Option<u64>> {
    let world_rc = world_for_node(ctx, element)?;
    let mut world = world_rc.borrow_mut();
    let info = {
        let Some(parsed) = world.document(element) else {
            return Ok(None);
        };
        parsed.dom.attributes(element).and_then(|list| {
            list.iter()
                .find(|attribute| {
                    attribute.name.ns.as_ref() == namespace
                        && attribute.name.local.as_ref() == local
                })
                .map(|attribute| {
                    (
                        attribute
                            .name
                            .prefix
                            .as_ref()
                            .filter(|prefix| !prefix.is_empty())
                            .map(ToString::to_string),
                        qualified_name(&attribute.name),
                        attribute.value.clone(),
                    )
                })
        })
    };
    let key = (element, namespace.to_owned(), local.to_owned());
    let Some((prefix, qualified, value)) = info else {
        if let Some(id) = world.attr_ids.remove(&key) {
            world.attr_owners.insert(id, None);
        }
        return Ok(None);
    };
    if let Some(&id) = world.attr_ids.get(&key) {
        world.attr_values.insert(id, value);
        world.attr_owners.insert(id, Some(element));
        return Ok(Some(id));
    }
    let id = world.next_attr_id;
    world.next_attr_id += 1;
    world.attrs.insert(
        id,
        AttrState {
            namespace: namespace.to_owned(),
            prefix,
            local: local.to_owned(),
            qualified,
        },
    );
    world.attr_owners.insert(id, Some(element));
    world.attr_values.insert(id, value);
    world.attr_ids.insert(key, id);
    Ok(Some(id))
}

/// Restores or creates the wrapper for an `Attr` id.
pub(crate) fn attr_wrapper<'js>(ctx: &Ctx<'js>, scope: NodeId, id: u64) -> Result<Value<'js>> {
    let world_rc = world_for_node(ctx, scope)?;
    if let Some(saved) = world_rc.borrow().attr_wrappers.get(&id).cloned()
        && let Some(value) = deref_weak(ctx, saved)?
    {
        return Ok(value);
    }
    let class = Class::instance(
        ctx.clone(),
        JsAttr {
            id,
            scope: Handle(scope),
        },
    )?;
    let value = Class::into_value(class);
    let weak = make_weak(ctx, value.clone())?;
    world_rc
        .borrow_mut()
        .attr_wrappers
        .insert(id, Persistent::save(ctx, weak));
    Ok(value)
}

/// Creates a detached `Attr` identity; attaches via `setAttributeNode`.
pub(crate) fn new_detached_attr(
    ctx: &Ctx<'_>,
    scope: NodeId,
    namespace: String,
    prefix: Option<String>,
    local: String,
    qualified: String,
) -> Result<u64> {
    let world_rc = world_for_node(ctx, scope)?;
    let mut world = world_rc.borrow_mut();
    let id = world.next_attr_id;
    world.next_attr_id += 1;
    world.attrs.insert(
        id,
        AttrState {
            namespace,
            prefix,
            local,
            qualified,
        },
    );
    world.attr_owners.insert(id, None);
    world.attr_values.insert(id, String::new());
    Ok(id)
}

/// Marks the `Attr` at `(element, namespace, local)` detached.
pub(crate) fn detach_attr(
    ctx: &Ctx<'_>,
    element: NodeId,
    namespace: &str,
    local: &str,
) -> Result<()> {
    let world_rc = world_for_node(ctx, element)?;
    let mut world = world_rc.borrow_mut();
    if let Some(id) = world
        .attr_ids
        .remove(&(element, namespace.to_owned(), local.to_owned()))
    {
        world.attr_owners.insert(id, None);
    }
    drop(world);
    touch_named_node_map(ctx, element)?;
    schedule_mutation_delivery(ctx)
}

/// Keeps an existing attached `Attr` wrapper in sync after a value change.
pub(crate) fn touch_attr(
    ctx: &Ctx<'_>,
    element: NodeId,
    namespace: &str,
    local: &str,
    value: &str,
) -> Result<()> {
    let world_rc = world_for_node(ctx, element)?;
    let mut world = world_rc.borrow_mut();
    if let Some(&id) = world
        .attr_ids
        .get(&(element, namespace.to_owned(), local.to_owned()))
    {
        world.attr_values.insert(id, value.to_owned());
        world.attr_owners.insert(id, Some(element));
    }
    drop(world);
    touch_named_node_map(ctx, element)?;
    schedule_mutation_delivery(ctx)
}

/// Event handler attribute names that the `<body>` element forwards to the
/// window ([WindowEventHandlers](https://html.spec.whatwg.org/multipage/webappapis.html#windoweventhandlers)).
const WINDOW_HANDLER_ATTRIBUTES: &[&str] = &[
    "onafterprint",
    "onbeforeprint",
    "onbeforeunload",
    "onhashchange",
    "onlanguagechange",
    "onmessage",
    "onmessageerror",
    "onoffline",
    "ononline",
    "onpagehide",
    "onpageshow",
    "onpopstate",
    "onrejectionhandled",
    "onstorage",
    "onunhandledrejection",
    "onunload",
];

/// Compiles or clears one element's event handler content attribute.
fn compile_handler_attribute(ctx: &Ctx<'_>, element: NodeId, typ: &str) -> Result<()> {
    let name = format!("on{typ}");
    let body = world(ctx)?
        .borrow()
        .document(element)
        .and_then(|parsed| parsed.dom.attribute(element, &name));
    let Some(object) = wrap_node(ctx, element)?.as_object().cloned() else {
        return Ok(());
    };
    // A `body` element's window event handler attributes register on the
    // window itself
    // (<https://html.spec.whatwg.org/multipage/dom.html#body-element-event-handlers>).
    let forwarded = WINDOW_HANDLER_ATTRIBUTES.contains(&name.as_str())
        && with_node_kind(ctx, element, |kind| is_html_element(kind, "body"))?;
    match body {
        Some(body) if !body.trim().is_empty() => {
            let source = format!("(function(event) {{\n{body}\n}})");
            match ctx.eval::<Function, _>(source) {
                Ok(compiled) => {
                    object.set(name.as_str(), compiled.clone())?;
                    if forwarded {
                        ctx.globals().set(name.as_str(), compiled)?;
                    }
                }
                Err(error) => {
                    // A malformed handler reports the error and clears the
                    // slot; setting the attribute must not throw
                    // (<https://html.spec.whatwg.org/multipage/webappapis.html#event-handler-content-attributes>).
                    report_exception(ctx, &error);
                    object.set(name.as_str(), Value::new_null(ctx.clone()))?;
                    if forwarded {
                        ctx.globals()
                            .set(name.as_str(), Value::new_null(ctx.clone()))?;
                    }
                }
            }
        }
        _ => {
            object.set(name.as_str(), Value::new_null(ctx.clone()))?;
            if forwarded {
                ctx.globals()
                    .set(name.as_str(), Value::new_null(ctx.clone()))?;
            }
        }
    }
    Ok(())
}

/// After an attribute change, runs the element's attribute-change hooks: an
/// `iframe`'s `src` drives its browsing context's navigation
/// (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#process-the-iframe-attributes>),
/// and an event handler content attribute compiles into its handler property
/// (<https://html.spec.whatwg.org/multipage/webappapis.html#event-handler-content-attributes>).
pub(crate) fn after_attribute_change(ctx: &Ctx<'_>, element: NodeId, local: &str) -> Result<()> {
    if let Some(typ) = local.strip_prefix("on")
        && !typ.is_empty()
        && typ
            .chars()
            .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
    {
        compile_handler_attribute(ctx, element, typ)?;
    }
    if local != "src" {
        return Ok(());
    }
    let is_iframe = with_node_kind(ctx, element, |kind| is_html_element(kind, "iframe"))?;
    if !is_iframe {
        return Ok(());
    }
    let world = world(ctx)?;
    let spec = world
        .borrow()
        .document(element)
        .and_then(|parsed| parsed.dom.attribute(element, "src"))
        .unwrap_or_default();
    // A detached `iframe` has no browsing context yet; insertion reads the
    // current attribute, so queueing here would navigate it twice.
    let connected = world
        .borrow()
        .document(element)
        .is_some_and(|parsed| parsed.dom.is_connected(element));
    if connected {
        world.borrow_mut().queue_frame_navigation(FrameNavigation {
            container: element,
            spec,
        });
    }
    Ok(())
}

/// The value of one event handler content attribute, when the element has it.
pub(crate) fn handler_attribute(ctx: &Ctx<'_>, id: NodeId, name: &str) -> Result<Option<String>> {
    let world = world(ctx)?;
    Ok(world
        .borrow()
        .document(id)
        .and_then(|parsed| parsed.dom.attribute(id, name)))
}

/// Whether script explicitly cleared this element's handler property, which
/// must keep a dispatch from falling back to the content attribute.
pub(crate) fn handler_cleared(ctx: &Ctx<'_>, id: NodeId, name: &str) -> Result<bool> {
    let world = world_for_node(ctx, id)?;
    Ok(world.borrow().handler_cleared(Some(id), name))
}

/// Removes the DOM attribute identified by `(namespace, local)` and syncs
/// the registry; used by `removeAttributeNode` and `NamedNodeMap.remove*`.
pub(crate) fn remove_attribute_sync(
    ctx: &Ctx<'_>,
    element: NodeId,
    namespace: &str,
    local: &str,
    by_namespace: bool,
) -> Result<()> {
    let world_rc = world_for_node(ctx, element)?;
    {
        let world = world_rc.borrow();
        let Some(mut parsed) = world.document_mut(element) else {
            return Ok(());
        };
        if by_namespace {
            parsed
                .dom
                .remove_attribute_ns(element, namespace, local)
                .map_err(|err| throw_dom_error(ctx, err))?;
        } else {
            parsed
                .dom
                .remove_attribute(element, local)
                .map_err(|err| throw_dom_error(ctx, err))?;
        }
    }
    {
        let mut world = world_rc.borrow_mut();
        if let Some(id) = world
            .attr_ids
            .remove(&(element, namespace.to_owned(), local.to_owned()))
        {
            world.attr_owners.insert(id, None);
        }
    }
    touch_named_node_map(ctx, element)?;
    after_attribute_change(ctx, element, local)?;
    schedule_mutation_delivery(ctx)
}

/// `setAttributeNode` / `setAttributeNodeNS`
/// (<https://dom.spec.whatwg.org/#concept-element-attributes-set>).
pub(crate) fn set_attribute_node<'js>(
    ctx: &Ctx<'js>,
    element: NodeId,
    attr: &Value<'js>,
) -> Result<Value<'js>> {
    let class = Class::<JsAttr>::from_js(ctx, attr.clone())
        .map_err(|_| Exception::throw_type(ctx, "argument is not an Attr"))?;
    let (id, scope) = {
        let attr = class.borrow();
        (attr.id, attr.scope.0)
    };
    let state = attr_state(ctx, scope, id)?;
    let owner = attr_owner(ctx, scope, id);
    if let Some(owner) = owner
        && owner != element
    {
        return Err(throw_dom(
            ctx,
            "InUseAttributeError",
            "attribute is already associated with another element",
        ));
    }
    let value = attr_value(ctx, scope, id)?;
    // The previous Attr with this identity becomes detached and is returned.
    let previous = {
        let world_rc = world_for_node(ctx, element)?;
        let key = (element, state.namespace.clone(), state.local.clone());
        world_rc
            .borrow()
            .attr_ids
            .get(&key)
            .copied()
            .filter(|previous| *previous != id)
    };
    // Setting an attribute that is already attached here is a no-op that
    // returns the attribute itself
    // (<https://dom.spec.whatwg.org/#concept-element-attributes-set> step 4).
    if previous.is_none() && attr_owner(ctx, scope, id) == Some(element) && {
        let world_rc = world_for_node(ctx, element)?;
        let world = world_rc.borrow();
        world
            .attr_ids
            .get(&(element, state.namespace.clone(), state.local.clone()))
            == Some(&id)
    } {
        return Ok(attr.clone());
    }
    {
        let world_rc = world_for_node(ctx, element)?;
        let mut world = world_rc.borrow_mut();
        if let Some(previous) = previous {
            world.attr_owners.insert(previous, None);
        }
        {
            let Some(mut parsed) = world.document_mut(element) else {
                return Err(Exception::throw_type(ctx, "no document"));
            };
            parsed
                .dom
                .set_attribute_by_ns(
                    element,
                    &state.namespace,
                    state.prefix.as_deref(),
                    &state.local,
                    value.clone(),
                )
                .map_err(|err| throw_dom_error(ctx, err))?;
        }
        world.attr_values.insert(id, value);
        world.attr_owners.insert(id, Some(element));
        world
            .attr_ids
            .insert((element, state.namespace.clone(), state.local.clone()), id);
    }
    touch_named_node_map(ctx, element)?;
    after_attribute_change(ctx, element, &state.local)?;
    schedule_mutation_delivery(ctx)?;
    match previous {
        Some(previous) => attr_wrapper(ctx, element, previous),
        None => Ok(Value::new_null(ctx.clone())),
    }
}
