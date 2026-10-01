//! Attribute, class, and handler-attribute objects and plumbing.

use super::{
    FromJs, OptString, WebIdlString, child_value, deref_weak, element_is_html, is_html_element,
    make_weak, qualified_name, realm_registry, schedule_mutation_delivery, string_value, throw_dom,
    throw_dom_error, with_node_kind, world, world_for_node, wrap_node,
};

use std::cell::RefCell;

use std::rc::Rc;

use dom::{NodeId, qualified_name_eq};

use rquickjs::{Class, Ctx, Exception, Function, Persistent, Result, Value, class::Trace};

use crate::js::events::{self, JsEvent, report_exception};
use crate::js::world::{
    AttrAttachError, AttrState, EventTargetKey, FrameNavigation, Handle, NavigationTarget, World,
    Wrapper,
};

/// `DOMTokenList` for `Element.classList`
/// (<https://dom.spec.whatwg.org/#interface-domtokenlist>).
#[derive(Trace, rquickjs::JsLifetime)]
pub struct JsTokenList {
    pub(crate) element: Handle,
}

include!(concat!(env!("OUT_DIR"), "/DOMTokenList.rs"));

#[allow(
    clippy::needless_pass_by_value,
    clippy::unused_self,
    reason = "generated dispatch passes Ctx by value and invokes operations on the receiver"
)]
impl JsTokenList {
    // https://dom.spec.whatwg.org/#dom-domtokenlist-length
    fn length(&self, ctx: &Ctx<'_>) -> Result<usize> {
        Ok(class_tokens(ctx, self.element.0)?.len())
    }

    // https://dom.spec.whatwg.org/#dom-domtokenlist-value
    fn value<'js>(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let world = world(ctx)?;
        let value = world
            .borrow()
            .document(self.element.0)
            .and_then(|parsed| parsed.document.attribute(self.element.0, "class"))
            .unwrap_or_default();
        rquickjs::String::from_str(ctx.clone(), &value)
    }

    fn set_value(&self, ctx: &Ctx<'_>, value: WebIdlString) -> Result<()> {
        write_class(ctx, self.element.0, &value.0)
    }

    // https://dom.spec.whatwg.org/#dom-domtokenlist-item
    fn item<'js>(&self, ctx: Ctx<'js>, index: u32) -> Result<Value<'js>> {
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
    fn contains(&self, ctx: Ctx<'_>, token: WebIdlString) -> Result<bool> {
        // `contains` does not validate its argument
        // (<https://dom.spec.whatwg.org/#dom-domtokenlist-contains>).
        Ok(class_tokens(&ctx, self.element.0)?.contains(&token.0))
    }

    // https://dom.spec.whatwg.org/#dom-domtokenlist-add
    fn add(&self, ctx: Ctx<'_>, tokens: Vec<WebIdlString>) -> Result<()> {
        for token in &tokens {
            validate_token(&ctx, &token.0)?;
        }
        let mut current = class_tokens(&ctx, self.element.0)?;
        for token in tokens {
            if !current.contains(&token.0) {
                current.push(token.0);
            }
        }
        // The update steps run even when no token was added, normalizing
        // the attribute (<https://dom.spec.whatwg.org/#dom-domtokenlist-add>).
        write_class_tokens(&ctx, self.element.0, &current)
    }

    // https://dom.spec.whatwg.org/#dom-domtokenlist-remove
    fn remove(&self, ctx: Ctx<'_>, tokens: Vec<WebIdlString>) -> Result<()> {
        for token in &tokens {
            validate_token(&ctx, &token.0)?;
        }
        let mut current = class_tokens(&ctx, self.element.0)?;
        for token in tokens {
            current.retain(|existing| existing != &token.0);
        }
        write_class_tokens(&ctx, self.element.0, &current)
    }

    // https://dom.spec.whatwg.org/#dom-domtokenlist-toggle
    fn toggle(&self, ctx: Ctx<'_>, token: WebIdlString, force: Option<bool>) -> Result<bool> {
        validate_token(&ctx, &token.0)?;
        let mut current = class_tokens(&ctx, self.element.0)?;
        let present = current.contains(&token.0);
        let should_be_present = force.unwrap_or(!present);
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
    fn supports(&self, ctx: Ctx<'_>, _token: WebIdlString) -> Result<bool> {
        // The spec ends with "throw a TypeError"
        // (<https://dom.spec.whatwg.org/#dom-domtokenlist-supports>).
        Err(Exception::throw_type(
            &ctx,
            "DOMTokenList has no supported tokens",
        ))
    }
}

/// The element's class tokens: an ordered set, so duplicates collapse.
fn class_tokens(ctx: &Ctx<'_>, id: NodeId) -> Result<Vec<String>> {
    let world = world(ctx)?;
    let mut tokens: Vec<String> = Vec::new();
    for token in world
        .borrow()
        .document(id)
        .and_then(|parsed| parsed.document.attribute(id, "class"))
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
    if tokens.is_empty()
        && world(ctx)?
            .borrow()
            .document(id)
            .is_none_or(|parsed| parsed.document.attribute(id, "class").is_none())
    {
        return Ok(());
    }
    write_class(ctx, id, &tokens.join(" "))
}

fn write_class(ctx: &Ctx<'_>, id: NodeId, value: &str) -> Result<()> {
    let world = world(ctx)?;
    let world = world.borrow();
    let Some(mut parsed) = world.document_mut(id) else {
        return Ok(());
    };
    dom::mutation::set_attribute(&mut parsed.document, id, "class", value)
        .map_err(|err| throw_dom_error(ctx, err))?;
    drop(parsed);
    drop(world);
    schedule_mutation_delivery(ctx)
}

/// One `Attr` platform object
/// (<https://dom.spec.whatwg.org/#interface-attr>). Identity lives in the
/// agent's Attr registry so `el.attributes[0] === el.getAttributeNode(name)`.
#[derive(Trace, rquickjs::JsLifetime)]
pub struct JsAttr<'js> {
    pub(crate) id: u64,
    pub(crate) scope: Handle,
    child_nodes: AttrChildren<'js>,
}

#[derive(rquickjs::JsLifetime)]
struct AttrChildren<'js>(RefCell<Option<Value<'js>>>);

impl<'js> Trace<'js> for AttrChildren<'js> {
    fn trace<'a>(&self, tracer: rquickjs::class::Tracer<'a, 'js>) {
        // No JavaScript runs while the cache is borrowed or replaced.
        self.0.borrow().trace(tracer);
    }
}

include!(concat!(env!("OUT_DIR"), "/Attr.rs"));

pub(super) type AttrArgument<'js> = Class<'js, JsAttr<'js>>;

#[allow(
    clippy::needless_pass_by_value,
    clippy::unused_self,
    reason = "generated dispatch passes Ctx by value and invokes operations on the receiver"
)]
impl<'object> JsAttr<'object> {
    fn event_target_key(&self, ctx: &Ctx<'_>) -> Result<EventTargetKey> {
        let state = attr_state(ctx, self.scope.0, self.id)?;
        Ok(EventTargetKey::Attribute {
            scope: state.scope,
            id: self.id,
        })
    }

    // https://dom.spec.whatwg.org/#dom-eventtarget-addeventlistener
    pub(super) fn add_event_listener<'js>(
        &self,
        ctx: Ctx<'js>,
        typ: rquickjs::String<'js>,
        callback: Value<'js>,
        options: Value<'js>,
    ) -> Result<()> {
        let home = attr_context(&ctx, self.scope.0, self.id)?;
        events::add_listener(
            &home,
            self.event_target_key(&home)?,
            typ.into_value(),
            callback,
            Some(options),
        )
    }

    // https://dom.spec.whatwg.org/#dom-eventtarget-removeeventlistener
    pub(super) fn remove_event_listener<'js>(
        &self,
        ctx: Ctx<'js>,
        typ: rquickjs::String<'js>,
        callback: Value<'js>,
        options: Value<'js>,
    ) -> Result<()> {
        let home = attr_context(&ctx, self.scope.0, self.id)?;
        events::remove_listener(
            &home,
            self.event_target_key(&home)?,
            typ.into_value(),
            callback,
            Some(options),
        )
    }

    // https://dom.spec.whatwg.org/#dom-eventtarget-dispatchevent
    pub(super) fn dispatch_event<'js>(&self, ctx: Ctx<'js>, event: Value<'js>) -> Result<bool> {
        let home = attr_context(&ctx, self.scope.0, self.id)?;
        let event = Class::<JsEvent>::from_js(&ctx, event)?;
        events::dispatch_event(&home, self.event_target_key(&home)?, &event)
    }

    // https://dom.spec.whatwg.org/#dom-attr-name
    fn name(&self, ctx: &Ctx<'_>) -> Result<String> {
        Ok(attr_state(ctx, self.scope.0, self.id)?.qualified)
    }

    // https://dom.spec.whatwg.org/#dom-attr-localname
    fn local_name(&self, ctx: &Ctx<'_>) -> Result<String> {
        Ok(attr_state(ctx, self.scope.0, self.id)?.local)
    }

    // https://dom.spec.whatwg.org/#dom-attr-prefix
    fn prefix<'js>(&self, ctx: &Ctx<'js>) -> Result<Option<rquickjs::String<'js>>> {
        attr_state(ctx, self.scope.0, self.id)?
            .prefix
            .map(|prefix| rquickjs::String::from_str(ctx.clone(), &prefix))
            .transpose()
    }

    // https://dom.spec.whatwg.org/#dom-attr-namespaceuri
    fn namespace_uri<'js>(&self, ctx: &Ctx<'js>) -> Result<Option<rquickjs::String<'js>>> {
        let namespace = attr_state(ctx, self.scope.0, self.id)?.namespace;
        if namespace.is_empty() {
            Ok(None)
        } else {
            rquickjs::String::from_str(ctx.clone(), &namespace).map(Some)
        }
    }

    // https://dom.spec.whatwg.org/#dom-attr-value
    fn value(&self, ctx: &Ctx<'_>) -> Result<String> {
        attr_value(ctx, self.scope.0, self.id)
    }

    fn set_value<'js>(&self, ctx: &Ctx<'js>, value: rquickjs::String<'js>) -> Result<()> {
        set_attr_value(ctx, self.scope.0, self.id, value.to_string()?)
    }

    // https://dom.spec.whatwg.org/#dom-node-nodevalue
    pub(super) fn node_value<'js>(&self, ctx: &Ctx<'js>) -> Result<Option<rquickjs::String<'js>>> {
        rquickjs::String::from_str(ctx.clone(), &self.value(ctx)?).map(Some)
    }

    pub(super) fn set_node_value<'js>(
        &self,
        ctx: &Ctx<'js>,
        value: Option<rquickjs::String<'js>>,
    ) -> Result<()> {
        // `Attr.nodeValue` follows the node rule: null acts as the empty
        // string (<https://dom.spec.whatwg.org/#dom-node-nodevalue>).
        set_attr_value(
            ctx,
            self.scope.0,
            self.id,
            value
                .map(|value| value.to_string())
                .transpose()?
                .unwrap_or_default(),
        )
    }

    // https://dom.spec.whatwg.org/#dom-node-textcontent
    pub(super) fn text_content<'js>(
        &self,
        ctx: &Ctx<'js>,
    ) -> Result<Option<rquickjs::String<'js>>> {
        self.node_value(ctx)
    }

    pub(super) fn set_text_content<'js>(
        &self,
        ctx: &Ctx<'js>,
        value: Option<rquickjs::String<'js>>,
    ) -> Result<()> {
        self.set_node_value(ctx, value)
    }

    // https://dom.spec.whatwg.org/#dom-attr-ownerelement
    fn owner_element<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let home = attr_context(ctx, self.scope.0, self.id)?;
        child_value(&home, attr_owner(&home, self.scope.0, self.id))
    }

    // https://dom.spec.whatwg.org/#dom-attr-specified
    #[allow(
        clippy::unnecessary_wraps,
        reason = "generated getters share a fallible call shape"
    )]
    fn specified(&self, _ctx: &Ctx<'_>) -> Result<bool> {
        Ok(true)
    }

    #[allow(
        clippy::unnecessary_wraps,
        reason = "generated getters share a fallible call shape"
    )]
    pub(super) fn node_type(&self, _ctx: &Ctx<'_>) -> Result<u16> {
        Ok(2)
    }

    pub(super) fn node_name(&self, ctx: &Ctx<'_>) -> Result<String> {
        self.name(ctx)
    }

    pub(super) fn owner_document<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        // https://dom.spec.whatwg.org/#concept-node-document
        let home = attr_context(ctx, self.scope.0, self.id)?;
        wrap_node(&home, attr_state(&home, self.scope.0, self.id)?.document)
    }

    // https://dom.spec.whatwg.org/#dom-node-baseuri
    pub(super) fn base_uri(&self, ctx: &Ctx<'_>) -> Result<dom::DomString> {
        let home = attr_context(ctx, self.scope.0, self.id)?;
        let document = attr_state(&home, self.scope.0, self.id)?.document;
        Ok(super::document_base_url_string(&home, document).into())
    }

    // https://dom.spec.whatwg.org/#dom-node-isconnected
    #[allow(
        clippy::unnecessary_wraps,
        reason = "generated getters share a fallible call shape"
    )]
    pub(super) fn is_connected(&self, _ctx: &Ctx<'_>) -> Result<bool> {
        Ok(false)
    }

    // https://dom.spec.whatwg.org/#dom-node-parentnode
    #[allow(
        clippy::unnecessary_wraps,
        reason = "generated getters share a fallible call shape"
    )]
    pub(super) fn parent_node<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        Ok(Value::new_null(ctx.clone()))
    }

    pub(super) fn parent_element<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.parent_node(ctx)
    }
    pub(super) fn first_child<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.parent_node(ctx)
    }
    pub(super) fn last_child<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.parent_node(ctx)
    }
    pub(super) fn next_sibling<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.parent_node(ctx)
    }
    pub(super) fn previous_sibling<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.parent_node(ctx)
    }

    // https://dom.spec.whatwg.org/#dom-node-childnodes
    pub(super) fn child_nodes(&self, ctx: &Ctx<'object>) -> Result<Value<'object>> {
        if let Some(saved) = self.child_nodes.0.borrow().clone() {
            return Ok(saved);
        }
        let home = attr_context(ctx, self.scope.0, self.id)?;
        let list = super::live_collection(
            &home,
            self.scope.0,
            super::CollectionKind::Static(Vec::new()),
            None,
        )?;
        *self.child_nodes.0.borrow_mut() = Some(list.clone());
        Ok(list)
    }

    #[allow(
        clippy::unnecessary_wraps,
        reason = "generated operations share a fallible call shape"
    )]
    pub(super) fn has_child_nodes(&self, _ctx: Ctx<'_>) -> Result<bool> {
        Ok(false)
    }

    // https://dom.spec.whatwg.org/#dom-node-getrootnode
    pub(super) fn get_root_node<'js>(
        &self,
        ctx: Ctx<'js>,
        _options: super::node::node_generated::GetRootNodeOptions,
    ) -> Result<Value<'js>> {
        attr_wrapper(&ctx, self.id)
    }

    // https://dom.spec.whatwg.org/#dom-node-normalize
    #[allow(
        clippy::unnecessary_wraps,
        reason = "generated operations share a fallible call shape"
    )]
    pub(super) fn normalize(&self, _ctx: Ctx<'_>) -> Result<()> {
        Ok(())
    }

    // https://dom.spec.whatwg.org/#dom-node-clonenode
    pub(super) fn clone_node<'js>(&self, ctx: Ctx<'js>, _deep: bool) -> Result<Value<'js>> {
        let state = attr_state(&ctx, self.scope.0, self.id)?;
        let clone = new_detached_attr(
            &ctx,
            state.document,
            state.namespace,
            state.prefix,
            state.local,
            state.qualified,
        )?;
        set_attr_value(
            &ctx,
            state.document,
            clone,
            attr_value(&ctx, self.scope.0, self.id)?,
        )?;
        attr_wrapper(&ctx, clone)
    }

    // https://dom.spec.whatwg.org/#dom-node-issamenode
    #[allow(
        clippy::unnecessary_wraps,
        reason = "generated operations share a fallible call shape"
    )]
    pub(super) fn is_same_node(
        &self,
        _ctx: Ctx<'_>,
        other: Option<super::host::NodeReference>,
    ) -> Result<bool> {
        Ok(other
            == Some(super::host::NodeReference::Attribute {
                scope: self.scope.0,
                id: self.id,
            }))
    }

    // https://dom.spec.whatwg.org/#concept-node-equals
    pub(super) fn is_equal_node(
        &self,
        ctx: Ctx<'_>,
        other: Option<super::host::NodeReference>,
    ) -> Result<bool> {
        let Some(super::host::NodeReference::Attribute { scope, id }) = other else {
            return Ok(false);
        };
        let a = attr_state(&ctx, self.scope.0, self.id)?;
        let b = attr_state(&ctx, scope, id)?;
        Ok(a.namespace == b.namespace
            && a.local == b.local
            && attr_value(&ctx, self.scope.0, self.id)? == attr_value(&ctx, scope, id)?)
    }

    // https://dom.spec.whatwg.org/#dom-node-contains
    pub(super) fn contains(
        &self,
        ctx: Ctx<'_>,
        other: Option<super::host::NodeReference>,
    ) -> Result<bool> {
        self.is_same_node(ctx, other)
    }

    pub(super) fn compare_document_position(
        &self,
        ctx: Ctx<'_>,
        other: super::host::NodeReference,
    ) -> Result<u16> {
        super::compare_node_position(
            &ctx,
            super::host::NodeReference::Attribute {
                scope: self.scope.0,
                id: self.id,
            },
            other,
        )
    }

    // https://dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity
    pub(super) fn append_child<'js>(
        &self,
        ctx: Ctx<'js>,
        _node: super::host::NodeReference,
    ) -> Result<Value<'js>> {
        Err(throw_dom(
            &ctx,
            "HierarchyRequestError",
            "attributes cannot have children",
        ))
    }
    pub(super) fn insert_before<'js>(
        &self,
        ctx: Ctx<'js>,
        node: super::host::NodeReference,
        _child: Option<super::host::NodeReference>,
    ) -> Result<Value<'js>> {
        self.append_child(ctx, node)
    }
    // https://dom.spec.whatwg.org/#concept-node-pre-remove
    pub(super) fn remove_child<'js>(
        &self,
        ctx: Ctx<'js>,
        _child: super::host::NodeReference,
    ) -> Result<Value<'js>> {
        Err(throw_dom(
            &ctx,
            "NotFoundError",
            "attributes have no children",
        ))
    }
    // https://dom.spec.whatwg.org/#concept-node-replace
    pub(super) fn replace_child<'js>(
        &self,
        ctx: Ctx<'js>,
        node: super::host::NodeReference,
        _child: super::host::NodeReference,
    ) -> Result<Value<'js>> {
        self.append_child(ctx, node)
    }

    // https://dom.spec.whatwg.org/#locate-a-namespace
    pub(super) fn lookup_namespace_uri<'js>(
        &self,
        ctx: Ctx<'js>,
        prefix: Option<rquickjs::String<'js>>,
    ) -> Result<Value<'js>> {
        let prefix = prefix
            .map(|value| value.to_string())
            .transpose()?
            .filter(|value| !value.is_empty());
        let Some(owner) = attr_owner(&ctx, self.scope.0, self.id) else {
            return Ok(Value::new_null(ctx));
        };
        let world = world_for_node(&ctx, owner)?;
        let world = world.borrow();
        let parsed = world
            .document(owner)
            .ok_or_else(|| Exception::throw_type(&ctx, "stale attribute owner"))?;
        match super::locate_namespace(&parsed.document, owner, prefix.as_deref()) {
            Some(namespace) => string_value(&ctx, &namespace),
            None => Ok(Value::new_null(ctx)),
        }
    }
    // https://dom.spec.whatwg.org/#dom-node-lookupprefix
    pub(super) fn lookup_prefix<'js>(
        &self,
        ctx: Ctx<'js>,
        namespace: Option<rquickjs::String<'js>>,
    ) -> Result<Value<'js>> {
        let namespace = namespace
            .map(|value| value.to_string())
            .transpose()?
            .filter(|value| !value.is_empty());
        let (Some(owner), Some(namespace)) = (attr_owner(&ctx, self.scope.0, self.id), namespace)
        else {
            return Ok(Value::new_null(ctx));
        };
        let world = world_for_node(&ctx, owner)?;
        let world = world.borrow();
        let parsed = world
            .document(owner)
            .ok_or_else(|| Exception::throw_type(&ctx, "stale attribute owner"))?;
        match super::locate_prefix(&parsed.document, owner, &namespace) {
            Some(prefix) => string_value(&ctx, &prefix),
            None => Ok(Value::new_null(ctx)),
        }
    }
    // https://dom.spec.whatwg.org/#dom-node-isdefaultnamespace
    pub(super) fn is_default_namespace<'js>(
        &self,
        ctx: Ctx<'js>,
        namespace: Option<rquickjs::String<'js>>,
    ) -> Result<bool> {
        let namespace = namespace
            .map(|value| value.to_string())
            .transpose()?
            .filter(|value| !value.is_empty());
        let found = self.lookup_namespace_uri(ctx.clone(), None)?;
        let found = if found.is_null() {
            None
        } else {
            Some(rquickjs::String::from_js(&ctx, found)?.to_string()?)
        };
        Ok(found == namespace)
    }
}

/// `NamedNodeMap`, live over the element's attribute list
/// (<https://dom.spec.whatwg.org/#interface-namednodemap>).
#[derive(Trace, rquickjs::JsLifetime)]
pub struct JsNamedNodeMap {
    pub(crate) element: Handle,
}

include!(concat!(env!("OUT_DIR"), "/NamedNodeMap.rs"));

#[allow(
    clippy::needless_pass_by_value,
    clippy::unused_self,
    reason = "generated dispatch passes Ctx by value and invokes operations on the receiver"
)]
impl JsNamedNodeMap {
    // https://dom.spec.whatwg.org/#dom-namednodemap-length
    fn length(&self, ctx: &Ctx<'_>) -> Result<usize> {
        let world = world(ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.element.0) else {
            return Ok(0);
        };
        Ok(parsed
            .document
            .attributes(self.element.0)
            .map_or(0, <[dom::Attribute]>::len))
    }

    // https://dom.spec.whatwg.org/#dom-namednodemap-item
    fn item<'js>(&self, ctx: Ctx<'js>, index: u32) -> Result<Value<'js>> {
        let Some(attribute) = attribute_at(&ctx, self.element.0, i64::from(index))? else {
            return Ok(Value::new_null(ctx));
        };
        match attached_attr_id(&ctx, self.element.0, &attribute.0, &attribute.1)? {
            Some(id) => attr_wrapper(&ctx, id),
            None => Ok(Value::new_null(ctx)),
        }
    }

    // https://dom.spec.whatwg.org/#dom-namednodemap-getnameditem
    fn get_named_item<'js>(&self, ctx: Ctx<'js>, name: WebIdlString) -> Result<Value<'js>> {
        named_item(&ctx, self.element.0, &name.0)
    }

    // https://dom.spec.whatwg.org/#dom-namednodemap-getnameditemns
    fn get_named_item_ns<'js>(
        &self,
        ctx: Ctx<'js>,
        namespace: OptString,
        local: WebIdlString,
    ) -> Result<Value<'js>> {
        let namespace = namespace.0.unwrap_or_default();
        match attached_attr_id(&ctx, self.element.0, &namespace, &local.0)? {
            Some(id) => attr_wrapper(&ctx, id),
            None => Ok(Value::new_null(ctx)),
        }
    }

    // https://dom.spec.whatwg.org/#dom-namednodemap-setnameditem
    fn set_named_item<'js>(&self, ctx: Ctx<'js>, attr: AttrArgument<'js>) -> Result<Value<'js>> {
        set_attribute_node(&ctx, self.element.0, &attr.into_value())
    }

    // https://dom.spec.whatwg.org/#dom-namednodemap-setnameditemns
    fn set_named_item_ns<'js>(&self, ctx: Ctx<'js>, attr: AttrArgument<'js>) -> Result<Value<'js>> {
        set_attribute_node(&ctx, self.element.0, &attr.into_value())
    }

    // https://dom.spec.whatwg.org/#dom-namednodemap-removenameditem
    fn remove_named_item<'js>(&self, ctx: Ctx<'js>, name: WebIdlString) -> Result<Value<'js>> {
        let world_rc = world(&ctx)?;
        let Some((namespace, local, id)) =
            named_attribute_id(&ctx, &world_rc, self.element.0, &name.0)?
        else {
            return Err(throw_dom(&ctx, "NotFoundError", "no such attribute"));
        };
        let value = attr_wrapper(&ctx, id)?;
        remove_attribute_sync(&ctx, self.element.0, &namespace, &local, true)?;
        Ok(value)
    }

    // https://dom.spec.whatwg.org/#dom-namednodemap-removenameditemns
    fn remove_named_item_ns<'js>(
        &self,
        ctx: Ctx<'js>,
        namespace: OptString,
        local: WebIdlString,
    ) -> Result<Value<'js>> {
        let namespace = namespace.0.unwrap_or_default();
        let value = match attached_attr_id(&ctx, self.element.0, &namespace, &local.0)? {
            Some(id) => attr_wrapper(&ctx, id)?,
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
    let Some(list) = parsed.document.attributes(element) else {
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
        Some((_, _, id)) => attr_wrapper(ctx, id),
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
        parsed.document.attributes(element).and_then(|list| {
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
    let registry = realm_registry(ctx)?;
    registry
        .borrow()
        .attributes
        .state(id)
        .filter(|state| state.scope == scope)
        .cloned()
        .ok_or_else(|| Exception::throw_type(ctx, "stale attribute"))
}

fn attr_context<'js>(ctx: &Ctx<'js>, scope: NodeId, id: u64) -> Result<Ctx<'js>> {
    let state = attr_state(ctx, scope, id)?;
    let owner = world_for_node(ctx, state.document)?;
    let prototype = owner
        .borrow()
        .brand("Attr")
        .ok_or_else(|| Exception::throw_internal(ctx, "missing Attr prototype"))?;
    Ok(prototype.restore(ctx)?.ctx().clone())
}

/// The attached element for `id`, or `None` when detached.
pub(crate) fn attr_owner(ctx: &Ctx<'_>, scope: NodeId, id: u64) -> Option<NodeId> {
    let state = attr_state(ctx, scope, id).ok()?;
    let owner = state.owner?;
    let world_rc = world_for_node(ctx, owner).ok()?;
    let world = world_rc.borrow();
    let parsed = world.document(owner)?;
    parsed
        .document
        .attribute_ns(owner, &state.namespace, &state.local)
        .map(|_| owner)
}

fn attr_value(ctx: &Ctx<'_>, scope: NodeId, id: u64) -> Result<String> {
    let state = attr_state(ctx, scope, id)?;
    if let Some(owner) = attr_owner(ctx, scope, id) {
        let world_rc = world_for_node(ctx, owner)?;
        let world = world_rc.borrow();
        if let Some(parsed) = world.document(owner)
            && let Some(value) = parsed
                .document
                .attribute_ns(owner, &state.namespace, &state.local)
        {
            return Ok(value);
        }
    }
    Ok(state.value)
}

fn set_attr_value(ctx: &Ctx<'_>, scope: NodeId, id: u64, value: String) -> Result<()> {
    let home = attr_context(ctx, scope, id)?;
    let snapshot = attr_state(ctx, scope, id)?;
    let world_rc = match snapshot.owner {
        Some(owner) => world_for_node(&home, owner)?,
        None => world(&home)?,
    };
    let world = world_rc.borrow();
    let registry = world.registry();
    let mut registry = registry.borrow_mut();
    let state = registry
        .attributes
        .state_mut(id)
        .filter(|state| state.scope == scope)
        .ok_or_else(|| Exception::throw_type(ctx, "stale attribute"))?;
    if let Some(owner) = state.owner {
        let mut parsed = world
            .document_mut(owner)
            .ok_or_else(|| Exception::throw_type(ctx, "stale attribute owner"))?;
        dom::mutation::set_attribute_by_ns(
            &mut parsed.document,
            owner,
            &state.namespace,
            state.prefix.as_deref(),
            &state.local,
            value.clone(),
        )
        .map_err(|err| throw_dom_error(ctx, err))?;
    }
    state.value = value;
    drop(registry);
    drop(world);
    schedule_mutation_delivery(&home)
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
            .map(|parsed| parsed.document.attribute_names(element))
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
    let world = world_rc.borrow();
    let info = {
        let Some(parsed) = world.document(element) else {
            return Ok(None);
        };
        parsed.document.attributes(element).and_then(|list| {
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
    let document = world
        .document(element)
        .ok_or_else(|| Exception::throw_type(ctx, "stale attribute owner"))?
        .document
        .document();
    let registry = world.registry();
    drop(world);
    let mut registry = registry.borrow_mut();
    let Some((prefix, qualified, value)) = info else {
        registry.attributes.detach(element, namespace, local);
        return Ok(None);
    };
    if let Some(id) = registry.attributes.attached(element, namespace, local) {
        registry.attributes.touch(element, namespace, local, value);
        return Ok(Some(id));
    }
    let id = registry
        .attributes
        .create(AttrState {
            scope: document,
            document,
            owner: Some(element),
            value,
            namespace: namespace.to_owned(),
            prefix,
            local: local.to_owned(),
            qualified,
        })
        .ok_or_else(|| Exception::throw_internal(ctx, "attribute ids exhausted"))?;
    Ok(Some(id))
}

/// Restores or creates the wrapper for an `Attr` id.
pub(crate) fn attr_wrapper<'js>(ctx: &Ctx<'js>, id: u64) -> Result<Value<'js>> {
    let registry = realm_registry(ctx)?;
    let state = registry
        .borrow()
        .attributes
        .state(id)
        .cloned()
        .ok_or_else(|| Exception::throw_type(ctx, "stale attribute"))?;
    let home = attr_context(ctx, state.scope, id)?;
    if let Some(saved) = registry.borrow().attributes.wrapper(id)
        && let Some(value) = deref_weak(&home, saved)?
    {
        return Ok(value);
    }
    // https://webidl.spec.whatwg.org/#internally-create-a-new-object-implementing-the-interface
    let owner = world_for_node(&home, state.document)?;
    let proto = owner
        .borrow()
        .brand("Attr")
        .ok_or_else(|| Exception::throw_internal(ctx, "missing Attr prototype"))?;
    let proto = proto.restore(ctx)?;
    let class = Class::instance_proto(
        JsAttr {
            id,
            scope: Handle(state.scope),
            child_nodes: AttrChildren(RefCell::new(None)),
        },
        proto,
    )?;
    let value = Class::into_value(class);
    let weak = make_weak(&home, value.clone())?;
    registry
        .borrow_mut()
        .attributes
        .intern_wrapper(id, Persistent::save(&home, weak));
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
    let world = world_rc.borrow();
    let document = world
        .document(scope)
        .ok_or_else(|| Exception::throw_type(ctx, "stale attribute document"))?
        .document
        .document();
    let registry = world.registry();
    drop(world);
    let id = registry
        .borrow_mut()
        .attributes
        .create(AttrState {
            scope: document,
            document,
            owner: None,
            value: String::new(),
            namespace,
            prefix,
            local,
            qualified,
        })
        .ok_or_else(|| Exception::throw_internal(ctx, "attribute ids exhausted"))?;
    Ok(id)
}

/// Keeps an existing attached `Attr` wrapper in sync after a value change.
pub(crate) fn touch_attr(
    ctx: &Ctx<'_>,
    element: NodeId,
    namespace: &str,
    local: &str,
    value: &str,
) -> Result<()> {
    let registry = realm_registry(ctx)?;
    registry
        .borrow_mut()
        .attributes
        .touch(element, namespace, local, value.to_owned());
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
        .and_then(|parsed| parsed.document.attribute(element, &name));
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
            // The `onerror` handler takes the spec's five arguments, not the
            // usual single event argument
            // (<https://html.spec.whatwg.org/multipage/webappapis.html#the-event-handler-processing-algorithm>).
            let params = if name == "onerror" {
                "event, source, lineno, colno, error"
            } else {
                "event"
            };
            let source = format!("(function({params}) {{\n{body}\n}})");
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

/// The input `type` change step that touches the text selection: when an
/// `input` becomes selectable again after a non-selectable type (for example
/// `color` back to `text`), the text entry cursor moves to the beginning
/// (<https://html.spec.whatwg.org/multipage/input.html#the-input-element>).
fn apply_input_type_change(ctx: &Ctx<'_>, element: NodeId) -> Result<()> {
    if !with_node_kind(ctx, element, |kind| is_html_element(kind, "input"))? {
        return Ok(());
    }
    let world = world(ctx)?;
    let world = world.borrow();
    let Some(mut parsed) = world.document_mut(element) else {
        return Ok(());
    };
    let now = dom::form::selection_supported(&parsed.document, element);
    let previously = dom::form::input_selectable(&parsed.document, element);
    if !previously && now {
        dom::form::set_selection(&mut parsed.document, element, 0, 0, 0);
    }
    dom::form::set_input_selectable(&mut parsed.document, element, now);
    Ok(())
}

/// After an attribute change, runs the element's attribute-change hooks: an
/// `iframe`'s `src` drives its browsing context's navigation
/// (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#process-the-iframe-attributes>),
/// an `<img>` `src` updates the image data
/// (<https://html.spec.whatwg.org/multipage/images.html#updating-the-image-data>),
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
    if local == "type" {
        apply_input_type_change(ctx, element)?;
    }
    if local != "src" {
        return Ok(());
    }
    let is_iframe = with_node_kind(ctx, element, |kind| is_html_element(kind, "iframe"))?;
    let is_img = with_node_kind(ctx, element, |kind| is_html_element(kind, "img"))?;
    if !is_iframe && !is_img {
        return Ok(());
    }
    let world = world(ctx)?;
    let spec = world
        .borrow()
        .document(element)
        .and_then(|parsed| parsed.document.attribute(element, "src"))
        .unwrap_or_default();
    // A detached `iframe` has no browsing context yet; insertion reads the
    // current attribute, so queueing here would navigate it twice. The same
    // is true of `<img>`: connection starts the fetch.
    let connected = world
        .borrow()
        .document(element)
        .is_some_and(|parsed| dom::lifecycle::is_connected(&parsed.document, element));
    if !connected {
        return Ok(());
    }
    if is_iframe {
        world
            .borrow_mut()
            .queue_frame_navigation(FrameNavigation::get(
                NavigationTarget::Container(element),
                spec,
            ));
    } else {
        world.borrow_mut().queue_image_update(element);
    }
    Ok(())
}

/// The value of one event handler content attribute, when the element has it.
pub(crate) fn handler_attribute(ctx: &Ctx<'_>, id: NodeId, name: &str) -> Result<Option<String>> {
    let world = world(ctx)?;
    Ok(world
        .borrow()
        .document(id)
        .and_then(|parsed| parsed.document.attribute(id, name)))
}

/// Whether script explicitly cleared this element's handler property, which
/// must keep a dispatch from falling back to the content attribute.
pub(crate) fn handler_cleared(ctx: &Ctx<'_>, id: NodeId, name: &str) -> Result<bool> {
    let world = world_for_node(ctx, id)?;
    Ok(world.borrow().handler_cleared(Some(id), name))
}

/// Whether script cleared the window-scoped handler property. Body `on*`
/// content attributes forward to the window, so a cleared window flag
/// suppresses the body attribute too
/// (<https://html.spec.whatwg.org/multipage/dom.html#body-element-event-handlers>).
pub(crate) fn window_handler_cleared(ctx: &Ctx<'_>, id: NodeId, name: &str) -> Result<bool> {
    let world = world_for_node(ctx, id)?;
    Ok(world.borrow().handler_cleared(None, name))
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
    let removed = {
        let world = world_rc.borrow();
        let Some(mut parsed) = world.document_mut(element) else {
            return Ok(());
        };
        let removed = parsed.document.attributes(element).and_then(|attributes| {
            attributes
                .iter()
                .find(|attribute| {
                    if by_namespace {
                        attribute.name.ns.as_ref() == namespace
                            && attribute.name.local.as_ref() == local
                    } else {
                        qualified_name_eq(&attribute.name, local)
                    }
                })
                .map(|attribute| {
                    (
                        attribute.name.ns.to_string(),
                        attribute.name.local.to_string(),
                        attribute.value.clone(),
                    )
                })
        });
        if by_namespace {
            dom::mutation::remove_attribute_ns(&mut parsed.document, element, namespace, local)
                .map_err(|err| throw_dom_error(ctx, err))?;
        } else {
            dom::mutation::remove_attribute(&mut parsed.document, element, local)
                .map_err(|err| throw_dom_error(ctx, err))?;
        }
        removed
    };
    if let Some((namespace, local, value)) = removed {
        let registry = world_rc.borrow().registry();
        registry
            .borrow_mut()
            .attributes
            .touch(element, &namespace, &local, value);
        registry
            .borrow_mut()
            .attributes
            .detach(element, &namespace, &local);
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
    let previous = attached_attr_id(ctx, element, &state.namespace, &state.local)?;
    // Setting an attribute that is already attached here is a no-op that
    // returns the attribute itself
    // (<https://dom.spec.whatwg.org/#concept-element-attributes-set> step 4).
    if previous == Some(id) {
        return Ok(attr.clone());
    }
    let previous = {
        let world_rc = world_for_node(ctx, element)?;
        let world = world_rc.borrow();
        let mut parsed = world
            .document_mut(element)
            .ok_or_else(|| Exception::throw_type(ctx, "no document"))?;
        let registry = world.registry();
        registry
            .borrow_mut()
            .attributes
            .attach(id, &mut parsed.document, element, value)
            .map_err(|err| match err {
                AttrAttachError::Stale => Exception::throw_type(ctx, "stale attribute"),
                AttrAttachError::InUse => throw_dom(
                    ctx,
                    "InUseAttributeError",
                    "attribute is already associated with another element",
                ),
                AttrAttachError::Dom(err) => throw_dom_error(ctx, err),
            })?
    };
    touch_named_node_map(ctx, element)?;
    after_attribute_change(ctx, element, &state.local)?;
    schedule_mutation_delivery(ctx)?;
    match previous {
        Some(previous) => attr_wrapper(ctx, previous),
        None => Ok(Value::new_null(ctx.clone())),
    }
}
