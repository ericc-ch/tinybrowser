//! Platform objects for DOM nodes, one interface per class.

use std::cell::RefCell;
use std::rc::Rc;

use dom::{
    DomError, LocalName, Namespace, NodeId, NodeKind, Prefix, QualName, html_namespace,
    svg_namespace,
};
use rquickjs::{
    Class, Ctx, Exception, FromJs, Function, Object, Persistent, Result, Value,
    class::{Trace, Tracer},
    function::{Constructor, Opt, Rest},
    prelude::This,
};

use super::world::{AttrState, EventTargetKey, Listener, SharedWorld, World};

#[derive(Clone, Copy, rquickjs::JsLifetime)]
struct Handle(NodeId);

impl<'js> Trace<'js> for Handle {
    fn trace<'a>(&self, _tracer: Tracer<'a, 'js>) {}
}

macro_rules! branded_node {
    ($name:ident, $js:literal) => {
        #[derive(Trace, rquickjs::JsLifetime)]
        #[rquickjs::class(rename = $js)]
        pub(crate) struct $name {
            handle: Handle,
        }
    };
}

#[derive(Trace, rquickjs::JsLifetime)]
#[rquickjs::class(rename = "Event")]
pub struct JsEvent {
    typ: String,
}

#[rquickjs::methods]
impl JsEvent {
    #[qjs(constructor)]
    fn new(typ: String) -> Self {
        Self { typ }
    }

    #[qjs(get, rename = "type")]
    fn event_type(&self) -> String {
        self.typ.clone()
    }
}

/// Legacy `DOMException` constants: name used to derive `code`.
///
/// <https://webidl.spec.whatwg.org/#idl-DOMException>
const DOM_EXCEPTION_CODES: [(&str, i32); 25] = [
    ("INDEX_SIZE_ERR", 1),
    ("DOMSTRING_SIZE_ERR", 2),
    ("HIERARCHY_REQUEST_ERR", 3),
    ("WRONG_DOCUMENT_ERR", 4),
    ("INVALID_CHARACTER_ERR", 5),
    ("NO_DATA_ALLOWED_ERR", 6),
    ("NO_MODIFICATION_ALLOWED_ERR", 7),
    ("NOT_FOUND_ERR", 8),
    ("NOT_SUPPORTED_ERR", 9),
    ("INUSE_ATTRIBUTE_ERR", 10),
    ("INVALID_STATE_ERR", 11),
    ("SYNTAX_ERR", 12),
    ("INVALID_MODIFICATION_ERR", 13),
    ("NAMESPACE_ERR", 14),
    ("INVALID_ACCESS_ERR", 15),
    ("VALIDATION_ERR", 16),
    ("TYPE_MISMATCH_ERR", 17),
    ("SECURITY_ERR", 18),
    ("NETWORK_ERR", 19),
    ("ABORT_ERR", 20),
    ("URL_MISMATCH_ERR", 21),
    ("QUOTA_EXCEEDED_ERR", 22),
    ("TIMEOUT_ERR", 23),
    ("INVALID_NODE_TYPE_ERR", 24),
    ("DATA_CLONE_ERR", 25),
];

/// `DOMException` as a hand-written Rust platform object
/// (<https://webidl.spec.whatwg.org/#idl-DOMException>). DOM operations
/// throw these; a plain `TypeError` would fail `assert_throws_dom`.
#[derive(Trace, rquickjs::JsLifetime)]
#[rquickjs::class(rename = "DOMException")]
pub struct JsDomException {
    name: String,
    message: String,
}

#[rquickjs::methods]
impl JsDomException {
    // https://webidl.spec.whatwg.org/#dom-domexception-domexception
    #[qjs(constructor)]
    fn new(message: OptString, name: OptString) -> Self {
        let name = name.0.filter(|value| !value.is_empty());
        Self {
            name: name.unwrap_or_else(|| "Error".into()),
            message: message.0.unwrap_or_default(),
        }
    }

    #[qjs(get, rename = "name")]
    fn get_name(&self) -> String {
        self.name.clone()
    }

    #[qjs(get, rename = "message")]
    fn get_message(&self) -> String {
        self.message.clone()
    }

    /// Legacy `code`, derived from the name
    /// (<https://webidl.spec.whatwg.org/#dom-domexception-code>).
    #[qjs(get, rename = "code")]
    fn get_code(&self) -> i32 {
        dom_exception_code(&self.name)
    }
}

fn dom_exception_code(name: &str) -> i32 {
    // https://webidl.spec.whatwg.org/#dom-domexception-code: legacy names
    // map to their constant's value; anything else is 0.
    match name {
        "IndexSizeError" => 1,
        "DOMStringSizeError" => 2,
        "HierarchyRequestError" => 3,
        "WrongDocumentError" => 4,
        "InvalidCharacterError" => 5,
        "NoDataAllowedError" => 6,
        "NoModificationAllowedError" => 7,
        "NotFoundError" => 8,
        "NotSupportedError" => 9,
        "InUseAttributeError" => 10,
        "InvalidStateError" => 11,
        "SyntaxError" => 12,
        "InvalidModificationError" => 13,
        "NamespaceError" => 14,
        "InvalidAccessError" => 15,
        "ValidationError" => 16,
        "TypeMismatchError" => 17,
        "SecurityError" => 18,
        "NetworkError" => 19,
        "AbortError" => 20,
        "URLMismatchError" => 21,
        "QuotaExceededError" => 22,
        "TimeoutError" => 23,
        "InvalidNodeTypeError" => 24,
        "DataCloneError" => 25,
        _ => 0,
    }
}

/// `DOMImplementation` as a platform object
/// (<https://dom.spec.whatwg.org/#interface-domimplementation>).
#[derive(Trace, rquickjs::JsLifetime)]
#[rquickjs::class(rename = "DOMImplementation")]
pub struct JsImplementation {
    document: Handle,
}

#[rquickjs::methods]
#[allow(
    clippy::needless_pass_by_value,
    clippy::unused_self,
    reason = "rquickjs method ABI passes Ctx by value; DOMImplementation methods ignore self"
)]
impl JsImplementation {
    #[qjs(constructor)]
    fn ctor(ctx: Ctx<'_>) -> Result<Self> {
        Err(Exception::throw_type(&ctx, "Illegal constructor"))
    }

    // https://dom.spec.whatwg.org/#dom-domimplementation-hasfeature
    #[qjs(rename = "hasFeature")]
    fn has_feature(&self, _args: Rest<Value<'_>>) -> bool {
        true
    }

    // https://dom.spec.whatwg.org/#dom-domimplementation-createdocumenttype
    #[qjs(rename = "createDocumentType")]
    fn create_document_type<'js>(
        &self,
        ctx: Ctx<'js>,
        name: String,
        public_id: String,
        system_id: String,
    ) -> Result<Value<'js>> {
        if !valid_doctype_name(&name) {
            return Err(throw_dom(
                &ctx,
                "InvalidCharacterError",
                "doctype name contains invalid characters",
            ));
        }
        let world = world(&ctx)?;
        let mut world = world.borrow_mut();
        let Some(parsed) = world.document_mut(self.document.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        let id = parsed.dom.create_doctype(name, public_id, system_id);
        drop(world);
        wrap_node(&ctx, id)
    }

    /// Creating a second document needs the renderer's World to own several
    /// trees; refused honestly until then.
    // https://dom.spec.whatwg.org/#dom-domimplementation-createdocument
    #[qjs(rename = "createDocument")]
    fn create_document<'js>(
        &self,
        ctx: Ctx<'js>,
        namespace: OptString,
        qualified: LegacyNullString,
        doctype: Value<'js>,
    ) -> Result<Value<'js>> {
        let namespace = namespace.0.unwrap_or_default();
        let content_type = match namespace.as_str() {
            "http://www.w3.org/1999/xhtml" => "application/xhtml+xml",
            "http://www.w3.org/2000/svg" => "image/svg+xml",
            _ => "application/xml",
        };
        let mut parsed = crate::Parsed {
            dom: dom::Dom::new(),
            quirks_mode: markup5ever::interface::QuirksMode::NoQuirks,
            parse_errors: 0,
            content_type,
        };
        let document = parsed.dom.document();
        if !doctype.is_null()
            && !doctype.is_undefined()
            && doctype_fields(&ctx, host_node_id(&ctx, &doctype).unwrap_or(document)).is_none()
        {
            return Err(Exception::throw_type(
                &ctx,
                "doctype argument is not a DocumentType",
            ));
        }
        if let Some(doctype) = host_node_id(&ctx, &doctype)
            && let Some((name, public_id, system_id)) = doctype_fields(&ctx, doctype)
        {
            let node = parsed.dom.create_doctype(name, public_id, system_id);
            parsed
                .dom
                .append(document, node)
                .map_err(|err| throw_dom_error(&ctx, err))?;
        }
        if !qualified.0.is_empty() {
            let name = validate_and_extract(
                &ctx,
                (!namespace.is_empty()).then_some(namespace.as_str()),
                &qualified.0,
                NodeContext::Element,
            )?;
            let element = parsed.dom.create_element(name, Vec::new());
            parsed
                .dom
                .append(document, element)
                .map_err(|err| throw_dom_error(&ctx, err))?;
        }
        let root = world(&ctx)?.borrow_mut().add_document(parsed);
        wrap_node(&ctx, root)
    }

    // https://dom.spec.whatwg.org/#dom-domimplementation-createhtmldocument
    #[qjs(rename = "createHTMLDocument")]
    fn create_html_document<'js>(
        &self,
        ctx: Ctx<'js>,
        title: Opt<OptionalTitle>,
    ) -> Result<Value<'js>> {
        let mut parsed = crate::Parsed {
            dom: dom::Dom::new(),
            quirks_mode: markup5ever::interface::QuirksMode::NoQuirks,
            parse_errors: 0,
            content_type: "text/html",
        };
        let document = parsed.dom.document();
        let doctype = parsed.dom.create_doctype("html", "", "");
        parsed
            .dom
            .append(document, doctype)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        let html = parsed
            .dom
            .create_element(html_element_name("html"), Vec::new());
        parsed
            .dom
            .append(document, html)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        let head = parsed
            .dom
            .create_element(html_element_name("head"), Vec::new());
        parsed
            .dom
            .append(html, head)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        if let Some(title) = title.0.and_then(|title| title.0) {
            let title_element = parsed
                .dom
                .create_element(html_element_name("title"), Vec::new());
            parsed
                .dom
                .append(head, title_element)
                .map_err(|err| throw_dom_error(&ctx, err))?;
            let text = parsed.dom.create_text(title);
            parsed
                .dom
                .append(title_element, text)
                .map_err(|err| throw_dom_error(&ctx, err))?;
        }
        let body = parsed
            .dom
            .create_element(html_element_name("body"), Vec::new());
        parsed
            .dom
            .append(html, body)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        let root = world(&ctx)?.borrow_mut().add_document(parsed);
        wrap_node(&ctx, root)
    }
}

/// The doctype's name, public id, and system id, from its own document.
fn doctype_fields(ctx: &Ctx<'_>, id: NodeId) -> Option<(String, String, String)> {
    let world_rc = world(ctx).ok()?;
    let world = world_rc.borrow();
    let parsed = world.document(id)?;
    match parsed.dom.get(id).map(|node| node.kind()) {
        Some(NodeKind::Doctype {
            name,
            public_id,
            system_id,
        }) => Some((name.clone(), public_id.clone(), system_id.clone())),
        _ => None,
    }
}

/// An HTML-namespace qualified name for document construction.
fn html_element_name(local: &str) -> QualName {
    QualName::new(None, html_namespace(), LocalName::from(local))
}

/// [Valid doctype name](https://dom.spec.whatwg.org/#valid-doctype-name): no
/// ASCII whitespace, NULL, or `>`.
fn valid_doctype_name(name: &str) -> bool {
    !name
        .chars()
        .any(|c| matches!(c, '\t' | '\n' | '\u{c}' | '\r' | ' ' | '\0' | '>'))
}

/// `DOMTokenList` for `Element.classList`
/// (<https://dom.spec.whatwg.org/#interface-domtokenlist>).
#[derive(Trace, rquickjs::JsLifetime)]
#[rquickjs::class(rename = "DOMTokenList")]
pub struct JsTokenList {
    element: Handle,
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
        let world = world(&ctx)?;
        Ok(world
            .borrow()
            .document(self.element.0)
            .and_then(|parsed| parsed.dom.attribute(self.element.0, "class"))
            .unwrap_or_default())
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
    let mut world = world.borrow_mut();
    let Some(parsed) = world.document_mut(id) else {
        return Ok(());
    };
    if value.is_empty() && parsed.dom.attribute(id, "class").is_none() {
        return Ok(());
    }
    parsed
        .dom
        .set_attribute(id, "class", value)
        .map_err(|err| throw_dom_error(ctx, err))
}

/// One `Attr` platform object
/// (<https://dom.spec.whatwg.org/#interface-attr>). Identity lives in the
/// World's attr registry so `el.attributes[0] === el.getAttributeNode(name)`.
#[derive(Trace, rquickjs::JsLifetime)]
#[rquickjs::class(rename = "Attr")]
pub struct JsAttr {
    id: u64,
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
        Ok(attr_state(&ctx, self.id)?.qualified)
    }

    // https://dom.spec.whatwg.org/#dom-attr-localname
    #[qjs(get, rename = "localName")]
    fn local_name(&self, ctx: Ctx<'_>) -> Result<String> {
        Ok(attr_state(&ctx, self.id)?.local)
    }

    // https://dom.spec.whatwg.org/#dom-attr-prefix
    #[qjs(get)]
    fn prefix<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        match attr_state(&ctx, self.id)?.prefix {
            Some(prefix) => string_value(&ctx, &prefix),
            None => Ok(Value::new_null(ctx)),
        }
    }

    // https://dom.spec.whatwg.org/#dom-attr-namespaceuri
    #[qjs(get, rename = "namespaceURI")]
    fn namespace_uri<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let namespace = attr_state(&ctx, self.id)?.namespace;
        if namespace.is_empty() {
            Ok(Value::new_null(ctx))
        } else {
            string_value(&ctx, &namespace)
        }
    }

    // https://dom.spec.whatwg.org/#dom-attr-value
    #[qjs(get)]
    fn value(&self, ctx: Ctx<'_>) -> Result<String> {
        attr_value(&ctx, self.id)
    }

    #[qjs(set, rename = "value")]
    fn set_value(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        set_attr_value(&ctx, self.id, value.0)
    }

    // https://dom.spec.whatwg.org/#dom-node-nodevalue
    #[qjs(get, rename = "nodeValue")]
    fn node_value(&self, ctx: Ctx<'_>) -> Result<String> {
        attr_value(&ctx, self.id)
    }

    #[qjs(set, rename = "nodeValue")]
    fn set_node_value(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        set_attr_value(&ctx, self.id, value.0)
    }

    // https://dom.spec.whatwg.org/#dom-node-textcontent
    #[qjs(get, rename = "textContent")]
    fn text_content(&self, ctx: Ctx<'_>) -> Result<String> {
        attr_value(&ctx, self.id)
    }

    #[qjs(set, rename = "textContent")]
    fn set_text_content(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        set_attr_value(&ctx, self.id, value.0)
    }

    // https://dom.spec.whatwg.org/#dom-attr-ownerelement
    #[qjs(get, rename = "ownerElement")]
    fn owner_element<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        child_value(&ctx, attr_owner(&ctx, self.id))
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
        Ok(attr_state(&ctx, self.id)?.qualified)
    }

    #[qjs(get, rename = "ownerDocument")]
    fn owner_document<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        // An attached Attr belongs to its owner element's document; a
        // detached one reports the main document.
        let document = attr_owner(&ctx, self.id).map_or_else(
            || {
                world(&ctx).ok().and_then(|world| {
                    world
                        .borrow()
                        .parsed
                        .as_ref()
                        .map(|parsed| parsed.dom.document())
                })
            },
            |owner| {
                world(&ctx).ok().and_then(|world| {
                    world
                        .borrow()
                        .document(owner)
                        .map(|parsed| parsed.dom.document())
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
    element: Handle,
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
            Some(id) => attr_wrapper(&ctx, id),
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
            Some(id) => attr_wrapper(&ctx, id),
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
        let name = if element_is_html(&ctx, self.element.0) {
            name.0.to_ascii_lowercase()
        } else {
            name.0
        };
        let world_rc = world(&ctx)?;
        let found = {
            let world = world_rc.borrow();
            let Some(parsed) = world.document(self.element.0) else {
                return Err(throw_dom(&ctx, "NotFoundError", "no such attribute"));
            };
            parsed.dom.attributes(self.element.0).and_then(|list| {
                list.iter()
                    .find(|attribute| qualified_name(&attribute.name) == name)
                    .map(|attribute| {
                        (
                            attribute.name.ns.to_string(),
                            attribute.name.local.to_string(),
                        )
                    })
            })
        };
        let Some((namespace, local)) = found else {
            return Err(throw_dom(&ctx, "NotFoundError", "no such attribute"));
        };
        let value = match attached_attr_id(&ctx, self.element.0, &namespace, &local)? {
            Some(id) => attr_wrapper(&ctx, id)?,
            None => Value::new_null(ctx.clone()),
        };
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
            Some(id) => attr_wrapper(&ctx, id)?,
            None => return Err(throw_dom(&ctx, "NotFoundError", "no such attribute")),
        };
        remove_attribute_sync(&ctx, self.element.0, &namespace, &local.0, true)?;
        Ok(value)
    }
}

/// `(namespace, local)` of the attribute at `index`, if any.
fn attribute_at(ctx: &Ctx<'_>, element: NodeId, index: i64) -> Result<Option<(String, String)>> {
    let world_rc = world(ctx)?;
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
    let name = if element_is_html(ctx, element) {
        name.to_ascii_lowercase()
    } else {
        name.to_owned()
    };
    let world_rc = world(ctx)?;
    let found = {
        let world = world_rc.borrow();
        let Some(parsed) = world.document(element) else {
            return Ok(Value::new_null(ctx.clone()));
        };
        parsed.dom.attributes(element).and_then(|list| {
            list.iter()
                .find(|attribute| qualified_name(&attribute.name) == name)
                .map(|attribute| {
                    (
                        attribute.name.ns.to_string(),
                        attribute.name.local.to_string(),
                    )
                })
        })
    };
    match found {
        Some((namespace, local)) => match attached_attr_id(ctx, element, &namespace, &local)? {
            Some(id) => attr_wrapper(ctx, id),
            None => Ok(Value::new_null(ctx.clone())),
        },
        None => Ok(Value::new_null(ctx.clone())),
    }
}

// ── Attr registry helpers ────────────────────────────────────────────────

fn attr_state(ctx: &Ctx<'_>, id: u64) -> Result<AttrState> {
    let world_rc = world(ctx)?;
    let world = world_rc.borrow();
    world
        .attrs
        .get(&id)
        .map(|state| AttrState {
            namespace: state.namespace.clone(),
            prefix: state.prefix.clone(),
            local: state.local.clone(),
            qualified: state.qualified.clone(),
        })
        .ok_or_else(|| Exception::throw_type(ctx, "stale attribute"))
}

/// The attached element for `id`, or `None` when detached.
fn attr_owner(ctx: &Ctx<'_>, id: u64) -> Option<NodeId> {
    let world_rc = world(ctx).ok()?;
    let world = world_rc.borrow();
    let owner = world.attr_owners.get(&id).copied().flatten()?;
    let state = world.attrs.get(&id)?;
    let parsed = world.document(owner)?;
    parsed
        .dom
        .attribute_ns(owner, &state.namespace, &state.local)
        .map(|_| owner)
}

fn attr_value(ctx: &Ctx<'_>, id: u64) -> Result<String> {
    if let Some(owner) = attr_owner(ctx, id) {
        let world_rc = world(ctx)?;
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
    let world_rc = world(ctx)?;
    Ok(world_rc
        .borrow()
        .attr_values
        .get(&id)
        .cloned()
        .unwrap_or_default())
}

fn set_attr_value(ctx: &Ctx<'_>, id: u64, value: String) -> Result<()> {
    let world_rc = world(ctx)?;
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
    let Some(parsed) = world.document_mut(owner) else {
        return Ok(());
    };
    parsed
        .dom
        .set_attribute_by_ns(owner, &namespace, prefix.as_deref(), &local, value)
        .map_err(|err| throw_dom_error(ctx, err))
}

/// Rebuilds the `NamedNodeMap` object's own index and named properties
/// (<https://dom.spec.whatwg.org/#interface-namednodemap>: named properties
/// never shadow interface members). HTML elements expose only qualified
/// names that survive ASCII lowercasing, since the named getter lowercases
/// (Firefox: `nsDOMAttributeMap::GetSupportedNames`).
fn refresh_named_node_map<'js>(ctx: &Ctx<'js>, element: NodeId, map: &Value<'js>) -> Result<()> {
    let html = element_is_html(ctx, element);
    let names: Vec<String> = {
        let world_rc = world(ctx)?;
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
    let world_rc = world(ctx)?;
    let Some(saved) = world_rc.borrow().named_node_map(element) else {
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
fn attached_attr_id(
    ctx: &Ctx<'_>,
    element: NodeId,
    namespace: &str,
    local: &str,
) -> Result<Option<u64>> {
    let world_rc = world(ctx)?;
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
fn attr_wrapper<'js>(ctx: &Ctx<'js>, id: u64) -> Result<Value<'js>> {
    let world_rc = world(ctx)?;
    if let Some(saved) = world_rc.borrow().attr_wrappers.get(&id).cloned()
        && let Some(value) = deref_weak(ctx, saved)?
    {
        return Ok(value);
    }
    let class = Class::instance(ctx.clone(), JsAttr { id })?;
    let value = Class::into_value(class);
    let weak = make_weak(ctx, value.clone())?;
    world_rc
        .borrow_mut()
        .attr_wrappers
        .insert(id, Persistent::save(ctx, weak));
    Ok(value)
}

/// Creates a detached `Attr` identity; attaches via `setAttributeNode`.
fn new_detached_attr(
    ctx: &Ctx<'_>,
    namespace: String,
    prefix: Option<String>,
    local: String,
    qualified: String,
) -> Result<u64> {
    let world_rc = world(ctx)?;
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
fn detach_attr(ctx: &Ctx<'_>, element: NodeId, namespace: &str, local: &str) -> Result<()> {
    let world_rc = world(ctx)?;
    let mut world = world_rc.borrow_mut();
    if let Some(id) = world
        .attr_ids
        .remove(&(element, namespace.to_owned(), local.to_owned()))
    {
        world.attr_owners.insert(id, None);
    }
    drop(world);
    touch_named_node_map(ctx, element)
}

/// Keeps an existing attached `Attr` wrapper in sync after a value change.
fn touch_attr(
    ctx: &Ctx<'_>,
    element: NodeId,
    namespace: &str,
    local: &str,
    value: &str,
) -> Result<()> {
    let world_rc = world(ctx)?;
    let mut world = world_rc.borrow_mut();
    if let Some(&id) = world
        .attr_ids
        .get(&(element, namespace.to_owned(), local.to_owned()))
    {
        world.attr_values.insert(id, value.to_owned());
        world.attr_owners.insert(id, Some(element));
    }
    drop(world);
    touch_named_node_map(ctx, element)
}

/// Removes the DOM attribute identified by `(namespace, local)` and syncs
/// the registry; used by `removeAttributeNode` and `NamedNodeMap.remove*`.
fn remove_attribute_sync(
    ctx: &Ctx<'_>,
    element: NodeId,
    namespace: &str,
    local: &str,
    by_namespace: bool,
) -> Result<()> {
    let world_rc = world(ctx)?;
    let mut world = world_rc.borrow_mut();
    let Some(parsed) = world.document_mut(element) else {
        return Ok(());
    };
    let result = if by_namespace {
        parsed
            .dom
            .remove_attribute_ns(element, namespace, local)
            .map_err(|err| throw_dom_error(ctx, err))
    } else {
        parsed
            .dom
            .remove_attribute(element, local)
            .map_err(|err| throw_dom_error(ctx, err))
    };
    result?;
    if let Some(id) = world
        .attr_ids
        .remove(&(element, namespace.to_owned(), local.to_owned()))
    {
        world.attr_owners.insert(id, None);
    }
    drop(world);
    touch_named_node_map(ctx, element)
}

/// `setAttributeNode` / `setAttributeNodeNS`
/// (<https://dom.spec.whatwg.org/#concept-element-attributes-set>).
fn set_attribute_node<'js>(
    ctx: &Ctx<'js>,
    element: NodeId,
    attr: &Value<'js>,
) -> Result<Value<'js>> {
    let class = Class::<JsAttr>::from_js(ctx, attr.clone())
        .map_err(|_| Exception::throw_type(ctx, "argument is not an Attr"))?;
    let id = class.borrow().id;
    let state = attr_state(ctx, id)?;
    let owner = attr_owner(ctx, id);
    if let Some(owner) = owner
        && owner != element
    {
        return Err(throw_dom(
            ctx,
            "InUseAttributeError",
            "attribute is already associated with another element",
        ));
    }
    let value = attr_value(ctx, id)?;
    // The previous Attr with this identity becomes detached and is returned.
    let previous = {
        let world_rc = world(ctx)?;
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
    if previous.is_none() && attr_owner(ctx, id) == Some(element) && {
        let world_rc = world(ctx)?;
        let world = world_rc.borrow();
        world
            .attr_ids
            .get(&(element, state.namespace.clone(), state.local.clone()))
            == Some(&id)
    } {
        return Ok(attr.clone());
    }
    {
        let world_rc = world(ctx)?;
        let mut world = world_rc.borrow_mut();
        if let Some(previous) = previous {
            world.attr_owners.insert(previous, None);
        }
        let Some(parsed) = world.document_mut(element) else {
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
        world.attr_values.insert(id, value);
        world.attr_owners.insert(id, Some(element));
        world
            .attr_ids
            .insert((element, state.namespace.clone(), state.local.clone()), id);
    }
    touch_named_node_map(ctx, element)?;
    match previous {
        Some(previous) => attr_wrapper(ctx, previous),
        None => Ok(Value::new_null(ctx.clone())),
    }
}

/// `DOMParser` (<https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-parsing-and-serialization>).
#[derive(Trace, rquickjs::JsLifetime)]
#[rquickjs::class(rename = "DOMParser")]
pub struct JsDomParser {
    _reserved: Option<Handle>,
}

#[rquickjs::methods]
#[allow(
    clippy::needless_pass_by_value,
    clippy::unused_self,
    reason = "rquickjs method ABI passes Ctx by value; DOMParser is a stateless constructor"
)]
impl JsDomParser {
    #[qjs(constructor)]
    fn new() -> Self {
        Self { _reserved: None }
    }

    // https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-domparser-parsefromstring
    #[qjs(rename = "parseFromString")]
    fn parse_from_string<'js>(
        &self,
        ctx: Ctx<'js>,
        source: WebIdlString,
        type_: WebIdlString,
    ) -> Result<Value<'js>> {
        let content_type = match type_.0.as_str() {
            "text/html" => "text/html",
            "text/xml" => "text/xml",
            "application/xml" => "application/xml",
            "application/xhtml+xml" => "application/xhtml+xml",
            "image/svg+xml" => "image/svg+xml",
            other => {
                return Err(Exception::throw_message(
                    &ctx,
                    &format!("The provided value '{other}' is not a valid enum value"),
                ));
            }
        };
        let parsed = if content_type == "text/html" {
            let mut parsed = crate::parse_html(&source.0);
            parsed.content_type = content_type;
            parsed
        } else {
            parse_xml_document(&source.0, content_type)
        };
        let root = world(&ctx)?.borrow_mut().add_document(parsed);
        wrap_node(&ctx, root)
    }
}

/// A small well-formedness-oriented XML parser for `DOMParser`'s XML types.
///
/// html5ever only parses HTML, and the XML types here exist to give scripts a
/// separate document tree with the requested content type. Supported syntax:
/// elements with quoted attributes, text, comments, declarations, and
/// self-closing tags. Malformed input yields a document holding a
/// `parsererror` element.
fn parse_xml_document(input: &str, content_type: &'static str) -> crate::Parsed {
    let mut parsed = crate::Parsed {
        dom: dom::Dom::new(),
        quirks_mode: markup5ever::interface::QuirksMode::NoQuirks,
        parse_errors: 0,
        content_type,
    };
    let document = parsed.dom.document();
    let mut stack = vec![document];
    let mut rest = input;
    while let Some(start) = rest.find('<') {
        if start > 0 {
            let text = &rest[..start];
            let parent = *stack.last().unwrap_or(&document);
            let node = parsed.dom.create_text(text);
            if parsed.dom.append(parent, node).is_err() {
                xml_malformed(&mut parsed, document);
                return parsed;
            }
        }
        rest = &rest[start..];
        let Some(remaining) = parse_xml_construct(&mut parsed, document, &mut stack, rest) else {
            xml_malformed(&mut parsed, document);
            return parsed;
        };
        rest = remaining;
    }
    if !rest.is_empty() {
        let parent = *stack.last().unwrap_or(&document);
        let node = parsed.dom.create_text(rest);
        let _ = parsed.dom.append(parent, node);
    }
    parsed
}

/// Adds the `parsererror` element to a malformed XML document.
fn xml_malformed(parsed: &mut crate::Parsed, document: NodeId) {
    let error = parsed.dom.create_element(
        QualName::new(None, Namespace::from(""), LocalName::from("parsererror")),
        Vec::new(),
    );
    let _ = parsed.dom.append(document, error);
}

/// Consumes one `<...>` XML construct, returning the remaining input.
fn parse_xml_construct<'a>(
    parsed: &mut crate::Parsed,
    document: NodeId,
    stack: &mut Vec<NodeId>,
    rest: &'a str,
) -> Option<&'a str> {
    if let Some(body) = rest.strip_prefix("<![CDATA[") {
        let end = body.find("]]>")?;
        let text = parsed.dom.create_cdata_section(&body[..end]);
        let parent = *stack.last().unwrap_or(&document);
        parsed.dom.append(parent, text).ok()?;
        return Some(&body[end + 3..]);
    }
    if rest.starts_with("<?") {
        let end = rest.find("?>")?;
        let inner = &rest[2..end];
        let mut parts = inner.splitn(2, char::is_whitespace);
        let target = parts.next().unwrap_or_default();
        let data = parts.next().unwrap_or_default().trim_start();
        if !target.eq_ignore_ascii_case("xml") {
            let node = parsed.dom.create_processing_instruction(target, data);
            let parent = *stack.last().unwrap_or(&document);
            parsed.dom.append(parent, node).ok()?;
        }
        return Some(&rest[end + 2..]);
    }
    if rest.starts_with("<!--") {
        let end = rest.find("-->")?;
        return Some(&rest[end + 3..]);
    }
    if rest.starts_with("<!") {
        let end = rest.find('>')?;
        return Some(&rest[end + 1..]);
    }
    let closing = rest.starts_with("</");
    let tag_end = find_tag_end(rest)?;
    let inner = rest[if closing { 2 } else { 1 }..tag_end].trim();
    let remaining = &rest[tag_end + 1..];
    if closing {
        if stack.len() > 1 {
            stack.pop();
        }
        return Some(remaining);
    }
    let self_closing = inner.ends_with('/');
    let inner = inner.trim_end_matches('/');
    let mut parts = inner.splitn(2, char::is_whitespace);
    let name = parts.next().filter(|name| !name.is_empty())?;
    let attributes = match parts.next() {
        Some(rest) => parse_xml_attributes(rest)?,
        None => Vec::new(),
    };
    let (prefix, local) = match name.split_once(':') {
        Some((prefix, local)) => (Some(prefix), local),
        None => (None, name),
    };
    let mut namespace = Namespace::from("");
    let mut kept = Vec::with_capacity(attributes.len());
    for attribute in attributes {
        let attribute_name = attribute.name.local.as_ref();
        if attribute_name == "xmlns" {
            namespace = Namespace::from(attribute.value.as_str());
        } else if let Some(declared) = attribute_name.strip_prefix("xmlns:") {
            if Some(declared) == prefix {
                namespace = Namespace::from(attribute.value.as_str());
            }
        } else {
            kept.push(attribute);
        }
    }
    let element = parsed.dom.create_element(
        QualName::new(
            prefix.map(dom::Prefix::from),
            namespace,
            LocalName::from(local),
        ),
        kept,
    );
    let parent = *stack.last().unwrap_or(&document);
    parsed.dom.append(parent, element).ok()?;
    if !self_closing {
        stack.push(element);
    }
    Some(remaining)
}

/// Index of the `>` that closes a tag, respecting quoted attribute values.
fn find_tag_end(rest: &str) -> Option<usize> {
    let mut quote = None;
    for (index, ch) in rest.char_indices() {
        match (quote, ch) {
            (Some(open), close) if close == open => quote = None,
            (None, '"' | '\'') => quote = Some(ch),
            (None, '>') => return Some(index),
            _ => {}
        }
    }
    None
}

/// Parses `name="value"` pairs; `None` on unquoted or truncated input.
fn parse_xml_attributes(input: &str) -> Option<Vec<dom::Attribute>> {
    let mut attributes = Vec::new();
    let mut rest = input.trim();
    while !rest.is_empty() {
        let eq = rest.find('=')?;
        let name = rest[..eq].trim();
        if name.is_empty() {
            return None;
        }
        let after = rest[eq + 1..].trim_start();
        let quote = after.chars().next()?;
        if quote != '"' && quote != '\'' {
            return None;
        }
        let end = after[1..].find(quote)? + 1;
        let value = after[1..end].to_owned();
        attributes.push(dom::Attribute {
            name: QualName::new(None, Namespace::from(""), LocalName::from(name)),
            value,
        });
        rest = after[end + 1..].trim_start();
    }
    Some(attributes)
}

/// Cross-document insertion: a `DocumentType` is copied into the parent's
/// document; anything else is refused until real adoption exists.
fn adopt_across_documents(ctx: &Ctx<'_>, parent: NodeId, node: NodeId) -> Result<NodeId> {
    if node.document_id() == parent.document_id() {
        return Ok(node);
    }
    let world_rc = world(ctx)?;
    let tree = {
        let world = world_rc.borrow();
        let Some(source) = world.document(node) else {
            return Err(throw_dom(
                ctx,
                "HierarchyRequestError",
                "nodes belong to different documents",
            ));
        };
        match source.dom.get(node).map(|node| node.kind()) {
            Some(NodeKind::Doctype { .. }) => import_snapshot(&source.dom, node, false),
            _ => None,
        }
    };
    let Some(tree) = tree else {
        return Err(throw_dom(
            ctx,
            "HierarchyRequestError",
            "nodes belong to different documents",
        ));
    };
    let mut world = world_rc.borrow_mut();
    let Some(target) = world.document_mut(parent) else {
        return Err(throw_dom(
            ctx,
            "HierarchyRequestError",
            "nodes belong to different documents",
        ));
    };
    materialize_import(&mut target.dom, &tree).map_err(|err| throw_dom_error(ctx, err))
}

/// Owned snapshot of a subtree for cross-document `importNode`.
enum ImportSnapshot {
    Element {
        name: QualName,
        attributes: Vec<dom::Attribute>,
        children: Vec<ImportSnapshot>,
    },
    Text(String),
    CData(String),
    ProcessingInstruction {
        target: String,
        data: String,
    },
    Comment(String),
    Doctype {
        name: String,
        public_id: String,
        system_id: String,
    },
    Fragment(Vec<ImportSnapshot>),
}

fn import_snapshot(dom: &dom::Dom, id: NodeId, deep: bool) -> Option<ImportSnapshot> {
    let children = |deep: bool| -> Vec<ImportSnapshot> {
        if !deep {
            return Vec::new();
        }
        dom.children(id)
            .map(|kids| {
                kids.copied()
                    .filter_map(|kid| import_snapshot(dom, kid, true))
                    .collect()
            })
            .unwrap_or_default()
    };
    match dom.get(id)?.kind() {
        NodeKind::Element { name, attributes } => Some(ImportSnapshot::Element {
            name: name.clone(),
            attributes: attributes.clone(),
            children: children(deep),
        }),
        NodeKind::Text { data } => Some(ImportSnapshot::Text(data.clone())),
        NodeKind::CDataSection { data } => Some(ImportSnapshot::CData(data.clone())),
        NodeKind::ProcessingInstruction { target, data } => {
            Some(ImportSnapshot::ProcessingInstruction {
                target: target.clone(),
                data: data.clone(),
            })
        }
        NodeKind::Comment { data } => Some(ImportSnapshot::Comment(data.clone())),
        NodeKind::Doctype {
            name,
            public_id,
            system_id,
        } => Some(ImportSnapshot::Doctype {
            name: name.clone(),
            public_id: public_id.clone(),
            system_id: system_id.clone(),
        }),
        NodeKind::Fragment => Some(ImportSnapshot::Fragment(children(deep))),
        NodeKind::Document => None,
    }
}

fn materialize_import(
    dom: &mut dom::Dom,
    snapshot: &ImportSnapshot,
) -> std::result::Result<NodeId, dom::DomError> {
    match snapshot {
        ImportSnapshot::Element {
            name,
            attributes,
            children,
        } => {
            let id = dom.create_element(name.clone(), attributes.clone());
            for child in children {
                let child = materialize_import(dom, child)?;
                dom.append(id, child)?;
            }
            Ok(id)
        }
        ImportSnapshot::Text(data) => Ok(dom.create_text(data.clone())),
        ImportSnapshot::CData(data) => Ok(dom.create_cdata_section(data.clone())),
        ImportSnapshot::ProcessingInstruction { target, data } => {
            Ok(dom.create_processing_instruction(target.clone(), data.clone()))
        }
        ImportSnapshot::Comment(data) => Ok(dom.create_comment(data.clone())),
        ImportSnapshot::Doctype {
            name,
            public_id,
            system_id,
        } => Ok(dom.create_doctype(name.clone(), public_id.clone(), system_id.clone())),
        ImportSnapshot::Fragment(children) => {
            let id = dom.create_fragment();
            for child in children {
                let child = materialize_import(dom, child)?;
                dom.append(id, child)?;
            }
            Ok(id)
        }
    }
}

/// Builds and throws a `DOMException` from Rust with a real prototype, so
/// `instanceof DOMException` and `constructor` checks pass.
fn throw_dom(ctx: &Ctx<'_>, name: &str, message: &str) -> rquickjs::Error {
    match Class::instance(
        ctx.clone(),
        JsDomException {
            name: name.into(),
            message: message.into(),
        },
    ) {
        Ok(exception) => ctx.throw(Class::into_value(exception)),
        Err(err) => err,
    }
}

/// Maps a refused DOM mutation onto its exception class
/// (<https://dom.spec.whatwg.org/#dom-domerror> naming).
fn throw_dom_error(ctx: &Ctx<'_>, err: DomError) -> rquickjs::Error {
    match err {
        DomError::CycleForbidden | DomError::HierarchyRequest => {
            throw_dom(ctx, "HierarchyRequestError", &err.to_string())
        }
        DomError::NoParent => throw_dom(ctx, "NotFoundError", &err.to_string()),
        // Programming errors, not web-visible DOM exceptions.
        DomError::StaleNode | DomError::WrongNodeType => {
            Exception::throw_type(ctx, &err.to_string())
        }
    }
}

branded_node!(JsNode, "Node");

#[derive(Trace, rquickjs::JsLifetime)]
enum CollectionKind {
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
struct JsCollection {
    scope: Handle,
    kind: CollectionKind,
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

impl JsNode {
    pub(crate) fn node_id(&self) -> NodeId {
        self.handle.0
    }
}

#[rquickjs::methods]
#[allow(
    clippy::needless_pass_by_value,
    clippy::unused_self,
    reason = "rquickjs method ABI passes Ctx by value; create* is a document method"
)]
impl JsNode {
    #[qjs(constructor)]
    fn ctor(ctx: Ctx<'_>) -> Result<Self> {
        Err(Exception::throw_type(&ctx, "Illegal constructor"))
    }
    #[qjs(get, rename = "nodeType")]
    fn node_type(&self, ctx: Ctx<'_>) -> Result<i32> {
        with_node_kind(&ctx, self.handle.0, |kind| match kind {
            Some(NodeKind::Element { .. }) => Ok(1),
            Some(NodeKind::Text { .. }) => Ok(3),
            Some(NodeKind::CDataSection { .. }) => Ok(4),
            Some(NodeKind::ProcessingInstruction { .. }) => Ok(7),
            Some(NodeKind::Comment { .. }) => Ok(8),
            Some(NodeKind::Document) => Ok(9),
            Some(NodeKind::Doctype { .. }) => Ok(10),
            Some(NodeKind::Fragment) => Ok(11),
            None => Err(Exception::throw_type(&ctx, "stale node")),
        })?
    }

    // https://dom.spec.whatwg.org/#dom-node-nodename
    // https://dom.spec.whatwg.org/#concept-element-html-uppercased-qualified-name
    #[qjs(get, rename = "nodeName")]
    fn node_name(&self, ctx: Ctx<'_>) -> Result<String> {
        let uppercase = document_is_html_content(&ctx, self.handle.0);
        with_node_kind(&ctx, self.handle.0, |kind| match kind {
            Some(NodeKind::Element { name, .. }) => Ok(element_node_name(name, uppercase)),
            Some(NodeKind::Text { .. }) => Ok("#text".into()),
            Some(NodeKind::CDataSection { .. }) => Ok("#cdata-section".into()),
            Some(NodeKind::ProcessingInstruction { target, .. }) => Ok(target.clone()),
            Some(NodeKind::Comment { .. }) => Ok("#comment".into()),
            Some(NodeKind::Document) => Ok("#document".into()),
            Some(NodeKind::Doctype { name, .. }) => Ok(name.clone()),
            Some(NodeKind::Fragment) => Ok("#document-fragment".into()),
            None => Err(Exception::throw_type(&ctx, "stale node")),
        })?
    }

    #[qjs(get, rename = "firstChild")]
    fn first_child<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let id = {
            let parsed = world.borrow();
            let Some(parsed) = parsed.document(self.handle.0) else {
                return Err(Exception::throw_type(&ctx, "no document"));
            };
            parsed
                .dom
                .children(self.handle.0)
                .and_then(|mut kids| kids.next().copied())
        };
        match id {
            Some(child) => wrap_node(&ctx, child),
            None => Ok(Value::new_null(ctx)),
        }
    }

    #[qjs(get, rename = "parentNode")]
    fn parent_node<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let id = world
            .borrow()
            .document(self.handle.0)
            .and_then(|parsed| parsed.dom.parent(self.handle.0));
        match id {
            Some(parent) => wrap_node(&ctx, parent),
            None => Ok(Value::new_null(ctx)),
        }
    }

    #[qjs(get, rename = "childNodes")]
    fn child_nodes<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        live_collection(&ctx, self.handle.0, CollectionKind::Children, None)
    }

    #[qjs(rename = "appendChild")]
    fn append_child<'js>(&self, ctx: Ctx<'js>, child: Value<'js>) -> Result<Value<'js>> {
        let Some(kid) = host_node_id(&ctx, &child) else {
            return Err(Exception::throw_type(&ctx, "not a node"));
        };
        {
            let world = world(&ctx)?;
            let parsed = world.borrow();
            let Some(parsed) = parsed.document(self.handle.0) else {
                return Err(Exception::throw_type(&ctx, "no document"));
            };
            parsed
                .dom
                .validate_pre_insert(self.handle.0, kid, None)
                .map_err(|err| throw_dom_error(&ctx, err))?;
        }
        let kid = adopt_across_documents(&ctx, self.handle.0, kid)?;
        let parent = self.handle.0;
        let world = world(&ctx)?;
        let mut world = world.borrow_mut();
        let Some(parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        parsed
            .dom
            .pre_insert(parent, kid, None)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        drop(world);
        wrap_node(&ctx, kid)
    }

    #[qjs(rename = "addEventListener")]
    fn add_event_listener<'js>(
        &self,
        ctx: Ctx<'js>,
        typ: String,
        callback: Function<'js>,
    ) -> Result<()> {
        add_listener(&ctx, EventTargetKey::Node(self.handle.0), typ, callback)
    }

    #[qjs(rename = "dispatchEvent")]
    fn dispatch_event<'js>(&self, ctx: Ctx<'js>, event: Class<'js, JsEvent>) -> Result<bool> {
        let typ = event.borrow().typ.clone();
        fire(&ctx, EventTargetKey::Node(self.handle.0), &typ, &event)
    }

    #[qjs(rename = "createElement")]
    fn create_element<'js>(&self, ctx: Ctx<'js>, tag: String) -> Result<Value<'js>> {
        create_html_element(&ctx, self.handle.0, &tag)
    }

    // https://dom.spec.whatwg.org/#dom-document-createelementns
    // https://dom.spec.whatwg.org/#internal-createelementns-steps
    #[qjs(rename = "createElementNS")]
    fn create_element_ns<'js>(
        &self,
        ctx: Ctx<'js>,
        ns: OptString,
        tag: String,
    ) -> Result<Value<'js>> {
        let name = validate_and_extract(&ctx, ns.0.as_deref(), &tag, NodeContext::Element)?;
        create_element_named(&ctx, self.handle.0, name)
    }

    #[qjs(rename = "createTextNode")]
    fn create_text_node<'js>(&self, ctx: Ctx<'js>, data: String) -> Result<Value<'js>> {
        create_kind(&ctx, self.handle.0, |dom| dom.create_text(data))
    }

    // https://dom.spec.whatwg.org/#dom-document-createcomment
    #[qjs(rename = "createComment")]
    fn create_comment<'js>(&self, ctx: Ctx<'js>, data: String) -> Result<Value<'js>> {
        create_kind(&ctx, self.handle.0, |dom| dom.create_comment(data))
    }

    // https://dom.spec.whatwg.org/#dom-document-createprocessinginstruction
    #[qjs(rename = "createProcessingInstruction")]
    fn create_processing_instruction<'js>(
        &self,
        ctx: Ctx<'js>,
        target: WebIdlString,
        data: WebIdlString,
    ) -> Result<Value<'js>> {
        if !valid_xml_name(&target.0) {
            return Err(throw_dom(
                &ctx,
                "InvalidCharacterError",
                "target does not match the XML Name production",
            ));
        }
        if data.0.contains("?>") {
            return Err(throw_dom(&ctx, "InvalidCharacterError", "data contains ?>"));
        }
        create_kind(&ctx, self.handle.0, |dom| {
            dom.create_processing_instruction(target.0, data.0)
        })
    }

    // https://dom.spec.whatwg.org/#dom-document-createcdatasection
    #[qjs(rename = "createCDATASection")]
    fn create_cdata_section<'js>(&self, ctx: Ctx<'js>, data: WebIdlString) -> Result<Value<'js>> {
        if data.0.contains("]]>") {
            return Err(throw_dom(
                &ctx,
                "InvalidCharacterError",
                "data contains ]]>",
            ));
        }
        create_kind(&ctx, self.handle.0, |dom| dom.create_cdata_section(data.0))
    }

    // https://dom.spec.whatwg.org/#dom-document-createattribute
    #[qjs(rename = "createAttribute")]
    fn create_attribute<'js>(&self, ctx: Ctx<'js>, name: WebIdlString) -> Result<Value<'js>> {
        if !valid_attribute_local_name(&name.0) {
            return Err(throw_dom(
                &ctx,
                "InvalidCharacterError",
                "attribute name is not a valid attribute local name",
            ));
        }
        let id = new_detached_attr(&ctx, String::new(), None, name.0.clone(), name.0)?;
        attr_wrapper(&ctx, id)
    }

    // https://dom.spec.whatwg.org/#dom-document-createattributens
    #[qjs(rename = "createAttributeNS")]
    fn create_attribute_ns<'js>(
        &self,
        ctx: Ctx<'js>,
        namespace: OptString,
        qualified: WebIdlString,
    ) -> Result<Value<'js>> {
        let name = validate_and_extract(
            &ctx,
            namespace.0.as_deref(),
            &qualified.0,
            NodeContext::Attribute,
        )?;
        let prefix = name
            .prefix
            .as_ref()
            .filter(|prefix| !prefix.is_empty())
            .map(ToString::to_string);
        let id = new_detached_attr(
            &ctx,
            name.ns.to_string(),
            prefix,
            name.local.to_string(),
            qualified_name(&name),
        )?;
        attr_wrapper(&ctx, id)
    }

    // https://dom.spec.whatwg.org/#dom-document-createdocumentfragment
    #[qjs(rename = "createDocumentFragment")]
    fn create_document_fragment<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        create_kind(&ctx, self.handle.0, dom::Dom::create_fragment)
    }

    // https://dom.spec.whatwg.org/#dom-document-importnode
    #[qjs(rename = "importNode")]
    fn import_node<'js>(
        &self,
        ctx: Ctx<'js>,
        node: Value<'js>,
        deep: Opt<bool>,
    ) -> Result<Value<'js>> {
        let source_id = required_node(&ctx, &node)?;
        let world_rc = world(&ctx)?;
        let tree = {
            let world = world_rc.borrow();
            let Some(source) = world.document(source_id) else {
                return Err(Exception::throw_type(&ctx, "stale node"));
            };
            if source.dom.document() == source_id {
                return Err(throw_dom(
                    &ctx,
                    "NotSupportedError",
                    "cannot import a document",
                ));
            }
            import_snapshot(&source.dom, source_id, deep.0.unwrap_or(false))
        };
        let Some(tree) = tree else {
            return Err(Exception::throw_type(&ctx, "stale node"));
        };
        let mut world = world_rc.borrow_mut();
        let Some(target) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        let id =
            materialize_import(&mut target.dom, &tree).map_err(|err| throw_dom_error(&ctx, err))?;
        drop(world);
        wrap_node(&ctx, id)
    }

    // https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#document-write-steps
    #[qjs(rename = "write")]
    fn write(&self, ctx: Ctx<'_>, html: String) -> Result<()> {
        let world = world(&ctx)?;
        let mut world = world.borrow_mut();
        if world.parser_active {
            world.pending_html_writes.push(html);
        }
        Ok(())
    }

    #[qjs(rename = "getElementById")]
    fn get_element_by_id<'js>(&self, ctx: Ctx<'js>, id: String) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let found = {
            let parsed = world.borrow();
            let Some(parsed) = parsed.document(self.handle.0) else {
                return Ok(Value::new_null(ctx));
            };
            find_element_by_id(&parsed.dom, self.handle.0, &id)
        };
        match found {
            Some(node) => wrap_node(&ctx, node),
            None => Ok(Value::new_null(ctx)),
        }
    }

    // https://dom.spec.whatwg.org/#dom-document-implementation
    #[qjs(get)]
    fn implementation<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world_rc = world(&ctx)?;
        if let Some(saved) = world_rc.borrow().implementation(self.handle.0)
            && let Some(value) = deref_weak(&ctx, saved)?
        {
            return Ok(value);
        }
        let value = Class::instance(
            ctx.clone(),
            JsImplementation {
                document: self.handle,
            },
        )?
        .into_value();
        let weak = make_weak(&ctx, value.clone())?;
        world_rc
            .borrow_mut()
            .intern_implementation(self.handle.0, Persistent::save(&ctx, weak));
        Ok(value)
    }

    #[qjs(rename = "getElementsByTagName")]
    fn get_elements_by_tag_name<'js>(&self, ctx: Ctx<'js>, name: String) -> Result<Value<'js>> {
        elements_by_tag(&ctx, self.handle.0, &name)
    }

    #[qjs(get)]
    fn body<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let found = world.borrow().document(self.handle.0).and_then(|parsed| {
            parsed
                .dom
                .select_first(parsed.dom.document(), "body")
                .ok()
                .flatten()
        });
        match found {
            Some(id) => wrap_node(&ctx, id),
            None => Ok(Value::new_null(ctx)),
        }
    }

    #[qjs(get, rename = "documentElement")]
    fn document_element<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let found = {
            let parsed = world.borrow();
            let Some(parsed) = parsed.document(self.handle.0) else {
                return Ok(Value::new_null(ctx));
            };
            parsed
                .dom
                .children(parsed.dom.document())
                .into_iter()
                .flatten()
                .copied()
                .find(|&id| {
                    matches!(
                        parsed.dom.get(id).map(|node| node.kind()),
                        Some(NodeKind::Element { .. })
                    )
                })
        };
        match found {
            Some(id) => wrap_node(&ctx, id),
            None => Ok(Value::new_null(ctx)),
        }
    }

    // https://dom.spec.whatwg.org/#dom-document-doctype
    #[qjs(get, rename = "doctype")]
    fn doctype<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let found = {
            let parsed = world.borrow();
            let Some(parsed) = parsed.document(self.handle.0) else {
                return Ok(Value::new_null(ctx));
            };
            parsed
                .dom
                .children(parsed.dom.document())
                .into_iter()
                .flatten()
                .copied()
                .find(|&id| {
                    matches!(
                        parsed.dom.get(id).map(|node| node.kind()),
                        Some(NodeKind::Doctype { .. })
                    )
                })
        };
        match found {
            Some(id) => wrap_node(&ctx, id),
            None => Ok(Value::new_null(ctx)),
        }
    }

    // https://dom.spec.whatwg.org/#dom-document-readyState
    #[qjs(get, rename = "readyState")]
    fn ready_state(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let is_document = world
            .document(self.handle.0)
            .is_some_and(|parsed| parsed.dom.document() == self.handle.0);
        if !is_document {
            return Ok(String::new());
        }
        Ok(if world.document_ready {
            "complete".into()
        } else {
            "loading".into()
        })
    }

    // https://dom.spec.whatwg.org/#dom-document-url
    #[qjs(get, rename = "URL")]
    fn url(&self, ctx: Ctx<'_>) -> String {
        document_url_string(&ctx, self.handle.0)
    }

    // https://dom.spec.whatwg.org/#dom-document-documenturi
    #[qjs(get, rename = "documentURI")]
    fn document_uri(&self, ctx: Ctx<'_>) -> String {
        document_url_string(&ctx, self.handle.0)
    }

    // https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-document-location
    #[qjs(get)]
    fn location<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        if is_main_document(&ctx, self.handle.0) {
            return ctx.globals().get("location");
        }
        Ok(Value::new_null(ctx))
    }

    // https://encoding.spec.whatwg.org/#dom-document-characterset
    #[qjs(get, rename = "characterSet")]
    fn character_set(&self) -> &'static str {
        "UTF-8"
    }

    #[qjs(get, rename = "charset")]
    fn charset(&self) -> &'static str {
        "UTF-8"
    }

    #[qjs(get, rename = "inputEncoding")]
    fn input_encoding(&self) -> &'static str {
        "UTF-8"
    }

    // https://dom.spec.whatwg.org/#dom-document-contenttype
    #[qjs(get, rename = "contentType")]
    fn content_type(&self, ctx: Ctx<'_>) -> Result<&'static str> {
        let world_rc = world(&ctx)?;
        let parsed = world_rc.borrow();
        Ok(parsed
            .document(self.handle.0)
            .map_or("text/html", |parsed| parsed.content_type))
    }

    // https://dom.spec.whatwg.org/#dom-document-compatmode
    #[qjs(get, rename = "compatMode")]
    fn compat_mode(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let quirks = parsed
            .document(self.handle.0)
            .is_some_and(|parsed| parsed.quirks_mode == markup5ever::interface::QuirksMode::Quirks);
        Ok(if quirks { "BackCompat" } else { "CSS1Compat" }.into())
    }

    #[qjs(rename = "getAttribute")]
    fn get_attribute<'js>(&self, ctx: Ctx<'js>, name: String) -> Result<Value<'js>> {
        if !valid_attribute_local_name(&name) {
            return Ok(Value::new_null(ctx));
        }
        let world = world(&ctx)?;
        let found = world
            .borrow()
            .document(self.handle.0)
            .and_then(|parsed| parsed.dom.attribute(self.handle.0, &name));
        match found {
            Some(value) => string_value(&ctx, &value),
            None => Ok(Value::new_null(ctx)),
        }
    }

    #[qjs(rename = "setAttribute")]
    fn set_attribute(&self, ctx: Ctx<'_>, name: String, value: WebIdlString) -> Result<()> {
        if !valid_attribute_local_name(&name) {
            return Err(throw_dom(
                &ctx,
                "InvalidCharacterError",
                "attribute name is not a valid attribute local name",
            ));
        }
        let local = if element_is_html(&ctx, self.handle.0) {
            name.to_ascii_lowercase()
        } else {
            name
        };
        {
            let world = world(&ctx)?;
            let mut world = world.borrow_mut();
            let Some(parsed) = world.document_mut(self.handle.0) else {
                return Err(Exception::throw_type(&ctx, "no document"));
            };
            parsed
                .dom
                .set_attribute(self.handle.0, &local, value.0.clone())
                .map_err(|err| throw_dom_error(&ctx, err))?;
        }
        touch_attr(&ctx, self.handle.0, "", &local, &value.0)
    }

    #[qjs(get)]
    fn id(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        Ok(world
            .borrow()
            .document(self.handle.0)
            .and_then(|parsed| parsed.dom.attribute(self.handle.0, "id"))
            .unwrap_or_default())
    }

    #[qjs(set, rename = "id")]
    fn set_id(&self, ctx: Ctx<'_>, value: String) -> Result<()> {
        self.set_attribute(ctx, "id".into(), WebIdlString(value))
    }

    #[qjs(get)]
    fn src(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        Ok(world
            .borrow()
            .document(self.handle.0)
            .and_then(|parsed| parsed.dom.attribute(self.handle.0, "src"))
            .unwrap_or_default())
    }

    #[qjs(set, rename = "src")]
    fn set_src(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        self.set_attribute(ctx, "src".into(), value)
    }

    #[qjs(get)]
    fn name(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        Ok(world
            .borrow()
            .document(self.handle.0)
            .map_or(String::new(), |parsed| {
                match parsed.dom.get(self.handle.0).map(|node| node.kind()) {
                    Some(NodeKind::Doctype { name, .. }) => name.clone(),
                    _ => parsed
                        .dom
                        .attribute(self.handle.0, "name")
                        .unwrap_or_default(),
                }
            }))
    }

    // https://dom.spec.whatwg.org/#dom-documenttype-publicid
    #[qjs(get, rename = "publicId")]
    fn public_id(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        Ok(world
            .borrow()
            .document(self.handle.0)
            .map_or(String::new(), |parsed| {
                match parsed.dom.get(self.handle.0).map(|node| node.kind()) {
                    Some(NodeKind::Doctype { public_id, .. }) => public_id.clone(),
                    _ => String::new(),
                }
            }))
    }

    // https://dom.spec.whatwg.org/#dom-documenttype-systemid
    #[qjs(get, rename = "systemId")]
    fn system_id(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        Ok(world
            .borrow()
            .document(self.handle.0)
            .map_or(String::new(), |parsed| {
                match parsed.dom.get(self.handle.0).map(|node| node.kind()) {
                    Some(NodeKind::Doctype { system_id, .. }) => system_id.clone(),
                    _ => String::new(),
                }
            }))
    }

    #[qjs(set, rename = "name")]
    fn set_name(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        self.set_attribute(ctx, "name".into(), value)
    }

    #[qjs(get)]
    fn content(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        Ok(world
            .borrow()
            .document(self.handle.0)
            .and_then(|parsed| parsed.dom.attribute(self.handle.0, "content"))
            .unwrap_or_default())
    }

    #[qjs(set, rename = "content")]
    fn set_content(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        self.set_attribute(ctx, "content".into(), value)
    }

    /// URL-reflected `href`: parsed against the document base and stored
    /// serialized (percent-encoded).
    #[qjs(get)]
    fn href(&self, ctx: Ctx<'_>) -> Result<String> {
        let world_rc = world(&ctx)?;
        let raw = world_rc
            .borrow()
            .document(self.handle.0)
            .and_then(|parsed| parsed.dom.attribute(self.handle.0, "href"))
            .unwrap_or_default();
        let base = document_url_string(&ctx, self.handle.0);
        Ok(url::Url::parse(&base)
            .ok()
            .and_then(|base| base.join(&raw).ok())
            .map_or(raw, |url| url.to_string()))
    }

    #[qjs(set, rename = "href")]
    fn set_href(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        let base = document_url_string(&ctx, self.handle.0);
        let resolved = url::Url::parse(&value.0)
            .or_else(|_| url::Url::parse(&base).and_then(|base| base.join(&value.0)))
            .map_or_else(|_| value.0, |url| url.to_string());
        self.set_attribute(ctx, "href".into(), WebIdlString(resolved))
    }

    /// Reflected inline style content attribute. A real
    /// `CSSStyleDeclaration` needs the CSSOM; this reflects the string.
    #[qjs(get, rename = "style")]
    fn style(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        Ok(world
            .borrow()
            .document(self.handle.0)
            .and_then(|parsed| parsed.dom.attribute(self.handle.0, "style"))
            .unwrap_or_default())
    }

    #[qjs(set, rename = "style")]
    fn set_style(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        self.set_attribute(ctx, "style".into(), value)
    }

    #[qjs(get)]
    fn data(&self, ctx: Ctx<'_>) -> Result<String> {
        character_data(&ctx, self.handle.0)
    }

    #[qjs(set, rename = "data")]
    fn set_data(&self, ctx: Ctx<'_>, value: LegacyNullString) -> Result<()> {
        set_character_data(&ctx, self.handle.0, value.0)
    }

    // https://dom.spec.whatwg.org/#dom-node-lastchild
    #[qjs(get, rename = "lastChild")]
    fn last_child<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let id = world
            .borrow()
            .document(self.handle.0)
            .and_then(|parsed| parsed.dom.children(self.handle.0))
            .and_then(|kids| kids.last().copied());
        child_value(&ctx, id)
    }

    // https://dom.spec.whatwg.org/#dom-node-nextsibling
    #[qjs(get, rename = "nextSibling")]
    fn next_sibling<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        sibling_value(&ctx, self.handle.0, true)
    }

    // https://dom.spec.whatwg.org/#dom-node-previoussibling
    #[qjs(get, rename = "previousSibling")]
    fn previous_sibling<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        sibling_value(&ctx, self.handle.0, false)
    }

    // https://dom.spec.whatwg.org/#dom-node-ownerdocument
    #[qjs(get, rename = "ownerDocument")]
    fn owner_document<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let id = world.borrow().document(self.handle.0).and_then(|parsed| {
            let document = parsed.dom.document();
            (document != self.handle.0).then_some(document)
        });
        child_value(&ctx, id)
    }

    // https://dom.spec.whatwg.org/#dom-node-haschildnodes
    #[qjs(rename = "hasChildNodes")]
    fn has_child_nodes(&self, ctx: Ctx<'_>) -> Result<bool> {
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(false);
        };
        Ok(parsed
            .dom
            .children(self.handle.0)
            .is_some_and(|mut kids| kids.next().is_some()))
    }

    // https://dom.spec.whatwg.org/#dom-node-nodevalue
    #[qjs(get, rename = "nodeValue")]
    fn node_value<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(Value::new_null(ctx));
        };
        match parsed.dom.get(self.handle.0).map(|node| node.kind()) {
            Some(
                NodeKind::Text { data }
                | NodeKind::CDataSection { data }
                | NodeKind::ProcessingInstruction { data, .. }
                | NodeKind::Comment { data },
            ) => string_value(&ctx, data),
            _ => Ok(Value::new_null(ctx)),
        }
    }

    #[qjs(set, rename = "nodeValue")]
    fn set_node_value(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        set_character_data(&ctx, self.handle.0, value.0)
    }

    // https://dom.spec.whatwg.org/#dom-node-textcontent
    #[qjs(get, rename = "textContent")]
    fn text_content<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(Value::new_null(ctx));
        };
        match parsed.dom.get(self.handle.0).map(|node| node.kind()) {
            Some(NodeKind::Element { .. } | NodeKind::Fragment) => {
                let text = descendant_text(&parsed.dom, self.handle.0);
                string_value(&ctx, &text)
            }
            Some(NodeKind::Text { data } | NodeKind::Comment { data }) => string_value(&ctx, data),
            _ => Ok(Value::new_null(ctx)),
        }
    }

    #[qjs(set, rename = "textContent")]
    fn set_text_content(&self, ctx: Ctx<'_>, value: OptString) -> Result<()> {
        let text = value.0.unwrap_or_default();
        let world = world(&ctx)?;
        let mut world = world.borrow_mut();
        let Some(parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        let dom = &mut parsed.dom;
        match dom.get(self.handle.0).map(|node| node.kind()) {
            Some(NodeKind::Text { .. }) => {
                return dom
                    .set_text(self.handle.0, text)
                    .map_err(|err| throw_dom_error(&ctx, err));
            }
            Some(NodeKind::Comment { .. }) => {
                return dom
                    .set_comment(self.handle.0, text)
                    .map_err(|err| throw_dom_error(&ctx, err));
            }
            Some(NodeKind::CDataSection { .. }) => {
                return dom
                    .set_cdata_section(self.handle.0, text)
                    .map_err(|err| throw_dom_error(&ctx, err));
            }
            Some(NodeKind::ProcessingInstruction { .. }) => {
                return dom
                    .set_processing_instruction(self.handle.0, text)
                    .map_err(|err| throw_dom_error(&ctx, err));
            }
            Some(NodeKind::Document | NodeKind::Doctype { .. }) => return Ok(()),
            _ => {}
        }
        let kids: Vec<NodeId> = dom
            .children(self.handle.0)
            .map(|kids| kids.copied().collect())
            .unwrap_or_default();
        for kid in kids {
            dom.destroy(kid).map_err(|err| throw_dom_error(&ctx, err))?;
        }
        if !text.is_empty() {
            let text_id = dom.create_text(text);
            dom.append(self.handle.0, text_id)
                .map_err(|err| throw_dom_error(&ctx, err))?;
        }
        Ok(())
    }

    // ── ParentNode ───────────────────────────────────────────────────────

    // https://dom.spec.whatwg.org/#dom-parentnode-children
    #[qjs(get)]
    fn children<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        live_collection(
            &ctx,
            self.handle.0,
            CollectionKind::ElementChildren,
            Some("HTMLCollection"),
        )
    }

    // https://dom.spec.whatwg.org/#dom-parentnode-firstelementchild
    #[qjs(get, rename = "firstElementChild")]
    fn first_element_child<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let found = {
            let parsed = world.borrow();
            let Some(parsed) = parsed.document(self.handle.0) else {
                return Ok(Value::new_null(ctx));
            };
            parsed
                .dom
                .children(self.handle.0)
                .and_then(|kids| kids.copied().find(|&kid| is_element(&parsed.dom, kid)))
        };
        child_value(&ctx, found)
    }

    // https://dom.spec.whatwg.org/#dom-parentnode-lastelementchild
    #[qjs(get, rename = "lastElementChild")]
    fn last_element_child<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let found = {
            let parsed = world.borrow();
            let Some(parsed) = parsed.document(self.handle.0) else {
                return Ok(Value::new_null(ctx));
            };
            parsed.dom.children(self.handle.0).and_then(|kids| {
                kids.rev()
                    .copied()
                    .find(|&kid| is_element(&parsed.dom, kid))
            })
        };
        child_value(&ctx, found)
    }

    // https://dom.spec.whatwg.org/#dom-parentnode-childelementcount
    #[qjs(get, rename = "childElementCount")]
    fn child_element_count(&self, ctx: Ctx<'_>) -> Result<usize> {
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(0);
        };
        Ok(parsed.dom.children(self.handle.0).map_or(0, |kids| {
            kids.filter(|&kid| is_element(&parsed.dom, *kid)).count()
        }))
    }

    // https://dom.spec.whatwg.org/#dom-parentnode-append
    #[qjs(rename = "append")]
    fn append<'js>(&self, ctx: Ctx<'js>, nodes: Rest<Value<'js>>) -> Result<()> {
        let node = convert_nodes_into_node(&ctx, self.handle.0, nodes)?;
        let world = world(&ctx)?;
        let mut world = world.borrow_mut();
        let Some(parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        parsed
            .dom
            .pre_insert(self.handle.0, node, None)
            .map_err(|err| throw_dom_error(&ctx, err))
    }

    // https://dom.spec.whatwg.org/#dom-parentnode-prepend
    #[qjs(rename = "prepend")]
    fn prepend<'js>(&self, ctx: Ctx<'js>, nodes: Rest<Value<'js>>) -> Result<()> {
        let node = convert_nodes_into_node(&ctx, self.handle.0, nodes)?;
        let world = world(&ctx)?;
        let mut world = world.borrow_mut();
        let Some(parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        let reference = parsed
            .dom
            .children(self.handle.0)
            .and_then(|kids| kids.copied().next());
        parsed
            .dom
            .pre_insert(self.handle.0, node, reference)
            .map_err(|err| throw_dom_error(&ctx, err))
    }

    // https://dom.spec.whatwg.org/#dom-parentnode-replacechildren
    #[qjs(rename = "replaceChildren")]
    fn replace_children<'js>(&self, ctx: Ctx<'js>, nodes: Rest<Value<'js>>) -> Result<()> {
        let node = convert_nodes_into_node(&ctx, self.handle.0, nodes)?;
        let world = world(&ctx)?;
        let mut world = world.borrow_mut();
        let Some(parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        parsed
            .dom
            .replace_all(self.handle.0, node)
            .map_err(|err| throw_dom_error(&ctx, err))
    }

    // https://dom.spec.whatwg.org/#dom-parentnode-queryselector
    #[qjs(rename = "querySelector")]
    fn query_selector<'js>(&self, ctx: Ctx<'js>, selectors: WebIdlString) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let found = {
            let parsed = world.borrow();
            let Some(parsed) = parsed.document(self.handle.0) else {
                return Ok(Value::new_null(ctx));
            };
            parsed
                .dom
                .select_first(self.handle.0, &selectors.0)
                .map_err(|err| select_error(&ctx, &err))?
        };
        child_value(&ctx, found)
    }

    // https://dom.spec.whatwg.org/#dom-parentnode-queryselectorall
    #[qjs(rename = "querySelectorAll")]
    fn query_selector_all<'js>(
        &self,
        ctx: Ctx<'js>,
        selectors: WebIdlString,
    ) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let ids = {
            let parsed = world.borrow();
            let Some(parsed) = parsed.document(self.handle.0) else {
                return Ok(Value::new_null(ctx));
            };
            parsed
                .dom
                .select_all(self.handle.0, &selectors.0)
                .map_err(|err| select_error(&ctx, &err))?
        };
        let handles = ids.into_iter().map(Handle).collect();
        live_collection(&ctx, self.handle.0, CollectionKind::Static(handles), None)
    }

    // https://dom.spec.whatwg.org/#dom-element-matches
    #[qjs(rename = "matches")]
    fn matches(&self, ctx: Ctx<'_>, selectors: WebIdlString) -> Result<bool> {
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(false);
        };
        parsed
            .dom
            .matches(self.handle.0, &selectors.0)
            .map_err(|err| select_error(&ctx, &err))
    }

    // https://dom.spec.whatwg.org/#dom-element-closest
    #[qjs(rename = "closest")]
    fn closest<'js>(&self, ctx: Ctx<'js>, selectors: WebIdlString) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let found = {
            let parsed = world.borrow();
            let Some(parsed) = parsed.document(self.handle.0) else {
                return Ok(Value::new_null(ctx));
            };
            let mut candidate = None;
            let mut cursor = Some(self.handle.0);
            while let Some(id) = cursor {
                if is_element(&parsed.dom, id)
                    && parsed
                        .dom
                        .matches(id, &selectors.0)
                        .map_err(|err| select_error(&ctx, &err))?
                {
                    candidate = Some(id);
                    break;
                }
                cursor = parsed.dom.parent(id);
            }
            candidate
        };
        child_value(&ctx, found)
    }

    // https://dom.spec.whatwg.org/#dom-element-getelementsbyclassname
    #[qjs(rename = "getElementsByClassName")]
    fn get_elements_by_class_name<'js>(
        &self,
        ctx: Ctx<'js>,
        names: WebIdlString,
    ) -> Result<Value<'js>> {
        live_collection(
            &ctx,
            self.handle.0,
            CollectionKind::ElementsByClass(names.0),
            Some("HTMLCollection"),
        )
    }

    // https://dom.spec.whatwg.org/#dom-document-title
    #[qjs(get)]
    fn title(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(String::new());
        };
        let title = parsed
            .dom
            .select_first(parsed.dom.document(), "title")
            .ok()
            .flatten();
        Ok(title.map_or_else(String::new, |title| descendant_text(&parsed.dom, title)))
    }

    #[qjs(set, rename = "title")]
    fn set_title(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        let world = world(&ctx)?;
        let mut world = world.borrow_mut();
        let Some(parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        let dom = &mut parsed.dom;
        let title = dom
            .select_first(dom.document(), "title")
            .ok()
            .flatten()
            .unwrap_or_else(|| {
                let name = QualName::new(None, html_namespace(), LocalName::from("title"));
                let title = dom.create_element(name, Vec::new());
                let target = dom
                    .select_first(dom.document(), "head")
                    .ok()
                    .flatten()
                    .or_else(|| dom.select_first(dom.document(), "html").ok().flatten());
                if let Some(target) = target {
                    let _ = dom.append(target, title);
                }
                title
            });
        let kids: Vec<NodeId> = dom
            .children(title)
            .map(|kids| kids.copied().collect())
            .unwrap_or_default();
        for kid in kids {
            dom.detach(kid).map_err(|err| throw_dom_error(&ctx, err))?;
        }
        if !value.0.is_empty() {
            let text = dom.create_text(value.0);
            dom.append(title, text)
                .map_err(|err| throw_dom_error(&ctx, err))?;
        }
        Ok(())
    }

    // https://html.spec.whatwg.org/multipage/dom.html#dom-document-getelementsbyname
    #[qjs(rename = "getElementsByName")]
    fn get_elements_by_name<'js>(&self, ctx: Ctx<'js>, name: WebIdlString) -> Result<Value<'js>> {
        live_collection(
            &ctx,
            self.handle.0,
            CollectionKind::ElementsByName(name.0),
            None,
        )
    }

    // https://dom.spec.whatwg.org/#dom-element-getelementsbytagnamens
    #[qjs(rename = "getElementsByTagNameNS")]
    fn get_elements_by_tag_name_ns<'js>(
        &self,
        ctx: Ctx<'js>,
        namespace: OptString,
        local: WebIdlString,
    ) -> Result<Value<'js>> {
        let local = if document_is_html(&ctx, self.handle.0) {
            local.0.to_ascii_lowercase()
        } else {
            local.0
        };
        live_collection(
            &ctx,
            self.handle.0,
            CollectionKind::ElementsByTagNs {
                namespace: namespace.0.unwrap_or_default(),
                local,
            },
            Some("HTMLCollection"),
        )
    }

    // ── ChildNode / NonDocumentTypeChildNode ─────────────────────────────

    // https://dom.spec.whatwg.org/#dom-childnode-before
    #[qjs(rename = "before")]
    fn before<'js>(&self, ctx: Ctx<'js>, nodes: Rest<Value<'js>>) -> Result<()> {
        let node = convert_nodes_into_node(&ctx, self.handle.0, nodes)?;
        let world = world(&ctx)?;
        let mut world = world.borrow_mut();
        let Some(parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        let Some(parent) = parsed.dom.parent(self.handle.0) else {
            return Ok(());
        };
        let previous = parsed.dom.sibling(self.handle.0, false);
        let reference = match previous {
            Some(previous) => parsed.dom.sibling(previous, true),
            None => parsed
                .dom
                .children(parent)
                .and_then(|kids| kids.copied().next()),
        };
        parsed
            .dom
            .pre_insert(parent, node, reference)
            .map_err(|err| throw_dom_error(&ctx, err))
    }

    // https://dom.spec.whatwg.org/#dom-childnode-after
    #[qjs(rename = "after")]
    fn after<'js>(&self, ctx: Ctx<'js>, nodes: Rest<Value<'js>>) -> Result<()> {
        let node = convert_nodes_into_node(&ctx, self.handle.0, nodes)?;
        let world = world(&ctx)?;
        let mut world = world.borrow_mut();
        let Some(parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        let Some(parent) = parsed.dom.parent(self.handle.0) else {
            return Ok(());
        };
        let reference = parsed.dom.sibling(self.handle.0, true);
        parsed
            .dom
            .pre_insert(parent, node, reference)
            .map_err(|err| throw_dom_error(&ctx, err))
    }

    // https://dom.spec.whatwg.org/#dom-childnode-replacewith
    #[qjs(rename = "replaceWith")]
    fn replace_with<'js>(&self, ctx: Ctx<'js>, nodes: Rest<Value<'js>>) -> Result<()> {
        let node = convert_nodes_into_node(&ctx, self.handle.0, nodes)?;
        let world = world(&ctx)?;
        let mut world = world.borrow_mut();
        let Some(parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        let Some(parent) = parsed.dom.parent(self.handle.0) else {
            return Ok(());
        };
        if parsed.dom.parent(self.handle.0) == Some(parent) {
            return parsed
                .dom
                .replace_child(parent, node, self.handle.0)
                .map_err(|err| throw_dom_error(&ctx, err));
        }
        let reference = parsed.dom.sibling(self.handle.0, true);
        parsed
            .dom
            .pre_insert(parent, node, reference)
            .map_err(|err| throw_dom_error(&ctx, err))
    }

    // https://dom.spec.whatwg.org/#dom-nondocumenttypechildnode-previouselementsibling
    #[qjs(get, rename = "previousElementSibling")]
    fn previous_element_sibling<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        element_sibling_value(&ctx, self.handle.0, false)
    }

    // https://dom.spec.whatwg.org/#dom-nondocumenttypechildnode-nextelementsibling
    #[qjs(get, rename = "nextElementSibling")]
    fn next_element_sibling<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        element_sibling_value(&ctx, self.handle.0, true)
    }

    // ── Element identity and attributes ─────────────────────────────────

    // https://dom.spec.whatwg.org/#dom-element-tagname
    #[qjs(get, rename = "tagName")]
    fn tag_name(&self, ctx: Ctx<'_>) -> Result<String> {
        let uppercase = document_is_html_content(&ctx, self.handle.0);
        with_node_kind(&ctx, self.handle.0, |kind| match kind {
            Some(NodeKind::Element { name, .. }) => element_node_name(name, uppercase),
            _ => String::new(),
        })
    }

    // https://dom.spec.whatwg.org/#dom-processinginstruction-target
    #[qjs(get)]
    fn target(&self, ctx: Ctx<'_>) -> Result<String> {
        with_node_kind(&ctx, self.handle.0, |kind| match kind {
            Some(NodeKind::ProcessingInstruction { target, .. }) => target.clone(),
            _ => String::new(),
        })
    }

    // https://dom.spec.whatwg.org/#dom-element-localname
    #[qjs(get, rename = "localName")]
    fn local_name(&self, ctx: Ctx<'_>) -> Result<String> {
        with_node_kind(&ctx, self.handle.0, |kind| match kind {
            Some(NodeKind::Element { name, .. }) => name.local.to_string(),
            _ => String::new(),
        })
    }

    // https://dom.spec.whatwg.org/#dom-element-prefix
    #[qjs(get)]
    fn prefix<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let prefix = with_node_kind(&ctx, self.handle.0, |kind| match kind {
            Some(NodeKind::Element { name, .. }) => name
                .prefix
                .as_ref()
                .filter(|prefix| !prefix.is_empty())
                .map(ToString::to_string),
            _ => None,
        })?;
        match prefix {
            Some(prefix) => string_value(&ctx, &prefix),
            None => Ok(Value::new_null(ctx)),
        }
    }

    // https://dom.spec.whatwg.org/#dom-element-namespaceuri
    #[qjs(get, rename = "namespaceURI")]
    fn namespace_uri<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let namespace = with_node_kind(&ctx, self.handle.0, |kind| match kind {
            Some(NodeKind::Element { name, .. }) => {
                (!name.ns.is_empty()).then(|| name.ns.to_string())
            }
            _ => None,
        })?;
        match namespace {
            Some(namespace) => string_value(&ctx, &namespace),
            None => Ok(Value::new_null(ctx)),
        }
    }

    // https://dom.spec.whatwg.org/#dom-element-classname
    #[qjs(get, rename = "className")]
    fn class_name(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        Ok(world
            .borrow()
            .document(self.handle.0)
            .and_then(|parsed| parsed.dom.attribute(self.handle.0, "class"))
            .unwrap_or_default())
    }

    #[qjs(set, rename = "className")]
    fn set_class_name(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        let world = world(&ctx)?;
        let mut world = world.borrow_mut();
        let Some(parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        parsed
            .dom
            .set_attribute(self.handle.0, "class", value.0)
            .map_err(|err| throw_dom_error(&ctx, err))
    }

    // https://dom.spec.whatwg.org/#dom-element-classlist
    #[qjs(get, rename = "classList")]
    fn class_list<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world_rc = world(&ctx)?;
        if let Some(saved) = world_rc.borrow().token_list(self.handle.0)
            && let Some(value) = deref_weak(&ctx, saved)?
        {
            return Ok(value);
        }
        let class = Class::instance(
            ctx.clone(),
            JsTokenList {
                element: self.handle,
            },
        )?;
        let raw = Class::into_value(class);
        // Indexed access (`classList[0]`) goes through the shared proxy.
        let proxy: Function = ctx.globals().get("__tb_liveCollection")?;
        let value: Value = proxy.call((raw,))?;
        let weak = make_weak(&ctx, value.clone())?;
        world_rc
            .borrow_mut()
            .intern_token_list(self.handle.0, Persistent::save(&ctx, weak));
        Ok(value)
    }

    // https://dom.spec.whatwg.org/#dom-element-hasattribute
    #[qjs(rename = "hasAttribute")]
    fn has_attribute(&self, ctx: Ctx<'_>, name: WebIdlString) -> Result<bool> {
        if !valid_attribute_local_name(&name.0) {
            return Ok(false);
        }
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(false);
        };
        Ok(parsed.dom.has_attribute(self.handle.0, &name.0))
    }

    // https://dom.spec.whatwg.org/#dom-element-getattributens
    #[qjs(rename = "getAttributeNS")]
    fn get_attribute_ns<'js>(
        &self,
        ctx: Ctx<'js>,
        namespace: OptString,
        local: WebIdlString,
    ) -> Result<Value<'js>> {
        let namespace = namespace.0.unwrap_or_default();
        let world = world(&ctx)?;
        let found = world
            .borrow()
            .document(self.handle.0)
            .and_then(|parsed| parsed.dom.attribute_ns(self.handle.0, &namespace, &local.0));
        match found {
            Some(value) => string_value(&ctx, &value),
            None => Ok(Value::new_null(ctx)),
        }
    }

    // https://dom.spec.whatwg.org/#dom-element-hasattributens
    #[qjs(rename = "hasAttributeNS")]
    fn has_attribute_ns(
        &self,
        ctx: Ctx<'_>,
        namespace: OptString,
        local: WebIdlString,
    ) -> Result<bool> {
        let namespace = namespace.0.unwrap_or_default();
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(false);
        };
        Ok(parsed
            .dom
            .attribute_ns(self.handle.0, &namespace, &local.0)
            .is_some())
    }

    // https://dom.spec.whatwg.org/#dom-element-setattributens
    #[qjs(rename = "setAttributeNS")]
    fn set_attribute_ns(
        &self,
        ctx: Ctx<'_>,
        namespace: OptString,
        qualified: WebIdlString,
        value: WebIdlString,
    ) -> Result<()> {
        // setAttributeNS never lowercases its name, even on HTML elements
        // (<https://dom.spec.whatwg.org/#dom-element-setattributens>).
        let name = validate_and_extract(
            &ctx,
            namespace.0.as_deref(),
            &qualified.0,
            NodeContext::Attribute,
        )?;
        let namespace = name.ns.to_string();
        let prefix = name
            .prefix
            .as_ref()
            .filter(|prefix| !prefix.is_empty())
            .map(ToString::to_string);
        let local = name.local.to_string();
        {
            let world = world(&ctx)?;
            let mut world = world.borrow_mut();
            let Some(parsed) = world.document_mut(self.handle.0) else {
                return Err(Exception::throw_type(&ctx, "no document"));
            };
            parsed
                .dom
                .set_attribute_by_ns(
                    self.handle.0,
                    &namespace,
                    prefix.as_deref(),
                    &local,
                    value.0.clone(),
                )
                .map_err(|err| throw_dom_error(&ctx, err))?;
        }
        touch_attr(&ctx, self.handle.0, &namespace, &local, &value.0)
    }

    // https://dom.spec.whatwg.org/#dom-element-removeattribute
    #[qjs(rename = "removeAttribute")]
    fn remove_attribute(&self, ctx: Ctx<'_>, name: WebIdlString) -> Result<()> {
        if !valid_attribute_local_name(&name.0) {
            return Ok(());
        }
        let local = if element_is_html(&ctx, self.handle.0) {
            name.0.to_ascii_lowercase()
        } else {
            name.0
        };
        {
            let world = world(&ctx)?;
            let mut world = world.borrow_mut();
            let Some(parsed) = world.document_mut(self.handle.0) else {
                return Ok(());
            };
            parsed
                .dom
                .remove_attribute(self.handle.0, &local)
                .map_err(|err| throw_dom_error(&ctx, err))?;
        }
        detach_attr(&ctx, self.handle.0, "", &local)
    }

    // https://dom.spec.whatwg.org/#dom-element-removeattributens
    #[qjs(rename = "removeAttributeNS")]
    fn remove_attribute_ns(
        &self,
        ctx: Ctx<'_>,
        namespace: OptString,
        local: WebIdlString,
    ) -> Result<()> {
        let namespace = namespace.0.unwrap_or_default();
        {
            let world = world(&ctx)?;
            let mut world = world.borrow_mut();
            let Some(parsed) = world.document_mut(self.handle.0) else {
                return Ok(());
            };
            parsed
                .dom
                .remove_attribute_ns(self.handle.0, &namespace, &local.0)
                .map_err(|err| throw_dom_error(&ctx, err))?;
        }
        detach_attr(&ctx, self.handle.0, &namespace, &local.0)
    }

    // https://dom.spec.whatwg.org/#dom-element-getattributenames
    #[qjs(rename = "getAttributeNames")]
    fn get_attribute_names(&self, ctx: Ctx<'_>) -> Result<Vec<String>> {
        let world = world(&ctx)?;
        Ok(world
            .borrow()
            .document(self.handle.0)
            .map(|parsed| parsed.dom.attribute_names(self.handle.0))
            .unwrap_or_default())
    }

    // https://dom.spec.whatwg.org/#dom-element-attributes
    #[qjs(get)]
    fn attributes<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world_rc = world(&ctx)?;
        if let Some(saved) = world_rc.borrow().named_node_map(self.handle.0)
            && let Some(value) = deref_weak(&ctx, saved)?
        {
            refresh_named_node_map(&ctx, self.handle.0, &value)?;
            return Ok(value);
        }
        let class = Class::instance(
            ctx.clone(),
            JsNamedNodeMap {
                element: self.handle,
            },
        )?;
        let value = Class::into_value(class);
        refresh_named_node_map(&ctx, self.handle.0, &value)?;
        let weak = make_weak(&ctx, value.clone())?;
        world_rc
            .borrow_mut()
            .intern_named_node_map(self.handle.0, Persistent::save(&ctx, weak));
        Ok(value)
    }

    // https://dom.spec.whatwg.org/#dom-element-hasattributes
    #[qjs(rename = "hasAttributes")]
    fn has_attributes(&self, ctx: Ctx<'_>) -> Result<bool> {
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(false);
        };
        Ok(parsed
            .dom
            .attributes(self.handle.0)
            .is_some_and(|list| !list.is_empty()))
    }

    // https://dom.spec.whatwg.org/#dom-element-getattributenode
    #[qjs(rename = "getAttributeNode")]
    fn get_attribute_node<'js>(&self, ctx: Ctx<'js>, name: WebIdlString) -> Result<Value<'js>> {
        if !valid_attribute_local_name(&name.0) {
            return Ok(Value::new_null(ctx));
        }
        let name = if element_is_html(&ctx, self.handle.0) {
            name.0.to_ascii_lowercase()
        } else {
            name.0
        };
        match attached_attr_id(&ctx, self.handle.0, "", &name)? {
            Some(id) => attr_wrapper(&ctx, id),
            None => Ok(Value::new_null(ctx)),
        }
    }

    // https://dom.spec.whatwg.org/#dom-element-getattributenodens
    #[qjs(rename = "getAttributeNodeNS")]
    fn get_attribute_node_ns<'js>(
        &self,
        ctx: Ctx<'js>,
        namespace: OptString,
        local: WebIdlString,
    ) -> Result<Value<'js>> {
        let namespace = namespace.0.unwrap_or_default();
        match attached_attr_id(&ctx, self.handle.0, &namespace, &local.0)? {
            Some(id) => attr_wrapper(&ctx, id),
            None => Ok(Value::new_null(ctx)),
        }
    }

    // https://dom.spec.whatwg.org/#dom-element-setattributenode
    #[qjs(rename = "setAttributeNode")]
    fn set_attribute_node<'js>(&self, ctx: Ctx<'js>, attr: Value<'js>) -> Result<Value<'js>> {
        set_attribute_node(&ctx, self.handle.0, &attr)
    }

    // https://dom.spec.whatwg.org/#dom-element-setattributenodens
    #[qjs(rename = "setAttributeNodeNS")]
    fn set_attribute_node_ns<'js>(&self, ctx: Ctx<'js>, attr: Value<'js>) -> Result<Value<'js>> {
        set_attribute_node(&ctx, self.handle.0, &attr)
    }

    // https://dom.spec.whatwg.org/#dom-element-removeattributenode
    #[qjs(rename = "removeAttributeNode")]
    fn remove_attribute_node<'js>(&self, ctx: Ctx<'js>, attr: Value<'js>) -> Result<Value<'js>> {
        let class = Class::<JsAttr>::from_js(&ctx, attr.clone())
            .map_err(|_| Exception::throw_type(&ctx, "argument is not an Attr"))?;
        let id = class.borrow().id;
        let state = attr_state(&ctx, id)?;
        let attached_here = attr_owner(&ctx, id) == Some(self.handle.0) && {
            let world_rc = world(&ctx)?;
            let world = world_rc.borrow();
            world
                .attr_ids
                .get(&(self.handle.0, state.namespace.clone(), state.local.clone()))
                == Some(&id)
        };
        if !attached_here {
            return Err(throw_dom(
                &ctx,
                "NotFoundError",
                "attribute is not attached to this element",
            ));
        }
        remove_attribute_sync(&ctx, self.handle.0, &state.namespace, &state.local, true)?;
        Ok(attr)
    }

    // https://dom.spec.whatwg.org/#dom-element-toggleattribute
    #[qjs(rename = "toggleAttribute")]
    fn toggle_attribute(&self, ctx: Ctx<'_>, name: WebIdlString, force: Opt<bool>) -> Result<bool> {
        if !valid_attribute_local_name(&name.0) {
            return Err(throw_dom(
                &ctx,
                "InvalidCharacterError",
                "attribute name is not a valid attribute local name",
            ));
        }
        let local = if element_is_html(&ctx, self.handle.0) {
            name.0.to_ascii_lowercase()
        } else {
            name.0
        };
        let (should_exist, changed) = {
            let world = world(&ctx)?;
            let mut world = world.borrow_mut();
            let Some(parsed) = world.document_mut(self.handle.0) else {
                return Err(Exception::throw_type(&ctx, "no document"));
            };
            let exists = parsed.dom.has_attribute(self.handle.0, &local);
            let should_exist = force.0.unwrap_or(!exists);
            if should_exist && !exists {
                parsed
                    .dom
                    .set_attribute(self.handle.0, &local, String::new())
                    .map_err(|err| throw_dom_error(&ctx, err))?;
            } else if !should_exist && exists {
                parsed
                    .dom
                    .remove_attribute(self.handle.0, &local)
                    .map_err(|err| throw_dom_error(&ctx, err))?;
            }
            (should_exist, should_exist != exists)
        };
        if changed {
            if should_exist {
                touch_attr(&ctx, self.handle.0, "", &local, "")?;
            } else {
                detach_attr(&ctx, self.handle.0, "", &local)?;
            }
        }
        Ok(should_exist)
    }

    // https://dom.spec.whatwg.org/#dom-node-normalize
    #[qjs(rename = "normalize")]
    fn normalize(&self, ctx: Ctx<'_>) -> Result<()> {
        let world = world(&ctx)?;
        let mut world = world.borrow_mut();
        let Some(parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        let dom = &mut parsed.dom;
        // A Text node normalizes only itself; containers merge adjacent
        // Text children and drop empty ones.
        if let Some(NodeKind::Text { data }) = dom.get(self.handle.0).map(|node| node.kind()) {
            if data.is_empty() {
                dom.detach(self.handle.0)
                    .map_err(|err| throw_dom_error(&ctx, err))?;
            }
            return Ok(());
        }
        let mut containers: Vec<NodeId> = vec![self.handle.0];
        let mut stack = vec![self.handle.0];
        while let Some(id) = stack.pop() {
            if let Some(kids) = dom.children(id) {
                for &kid in kids {
                    if matches!(
                        dom.get(kid).map(|node| node.kind()),
                        Some(NodeKind::Element { .. })
                    ) {
                        containers.push(kid);
                        stack.push(kid);
                    }
                }
            }
        }
        for container in containers {
            loop {
                let kids: Vec<NodeId> = dom
                    .children(container)
                    .map(|kids| kids.copied().collect())
                    .unwrap_or_default();
                let run_start = kids.windows(2).position(|window| {
                    matches!(
                        (
                            dom.get(window[0]).map(|node| node.kind()),
                            dom.get(window[1]).map(|node| node.kind()),
                        ),
                        (Some(NodeKind::Text { .. }), Some(NodeKind::Text { .. }))
                    )
                });
                match run_start {
                    Some(start) => {
                        let mut run = vec![kids[start]];
                        for &id in &kids[start + 1..] {
                            if matches!(
                                dom.get(id).map(|node| node.kind()),
                                Some(NodeKind::Text { .. })
                            ) {
                                run.push(id);
                            } else {
                                break;
                            }
                        }
                        let mut data = String::new();
                        for &id in &run {
                            if let Some(NodeKind::Text { data: part }) =
                                dom.get(id).map(|node| node.kind())
                            {
                                data.push_str(part);
                            }
                        }
                        let first = run[0];
                        dom.set_text(first, data)
                            .map_err(|err| throw_dom_error(&ctx, err))?;
                        for &id in &run[1..] {
                            dom.detach(id).map_err(|err| throw_dom_error(&ctx, err))?;
                        }
                    }
                    None => break,
                }
            }
            let kids: Vec<NodeId> = dom
                .children(container)
                .map(|kids| kids.copied().collect())
                .unwrap_or_default();
            for kid in kids {
                if matches!(
                    dom.get(kid).map(|node| node.kind()),
                    Some(NodeKind::Text { data }) if data.is_empty()
                ) {
                    dom.detach(kid).map_err(|err| throw_dom_error(&ctx, err))?;
                }
            }
        }
        Ok(())
    }

    // https://dom.spec.whatwg.org/#dom-node-comparedocumentposition
    #[qjs(rename = "compareDocumentPosition")]
    fn compare_document_position<'js>(&self, ctx: Ctx<'js>, other: Value<'js>) -> Result<u16> {
        const DISCONNECTED: u16 = 1;
        const PRECEDING: u16 = 2;
        const FOLLOWING: u16 = 4;
        const CONTAINS: u16 = 8;
        const CONTAINED_BY: u16 = 16;
        const IMPLEMENTATION_SPECIFIC: u16 = 32;
        let Some(other) = host_node_id(&ctx, &other) else {
            return Err(Exception::throw_type(&ctx, "argument is not a Node"));
        };
        let a = self.handle.0;
        if a == other {
            return Ok(0);
        }
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(DISCONNECTED | IMPLEMENTATION_SPECIFIC);
        };
        let dom = &parsed.dom;
        if root_of(dom, a) != root_of(dom, other) {
            return Ok(DISCONNECTED | IMPLEMENTATION_SPECIFIC);
        }
        let ancestors_a = ancestor_chain(dom, a);
        let ancestors_b = ancestor_chain(dom, other);
        if ancestors_a.contains(&other) {
            return Ok(CONTAINED_BY | FOLLOWING);
        }
        if ancestors_b.contains(&a) {
            return Ok(CONTAINS | PRECEDING);
        }
        Ok(if tree_order(dom, a, other) == std::cmp::Ordering::Less {
            PRECEDING
        } else {
            FOLLOWING
        })
    }

    // https://dom.spec.whatwg.org/#dom-node-issamenode
    #[qjs(rename = "isSameNode")]
    fn is_same_node<'js>(&self, ctx: Ctx<'js>, other: Value<'js>) -> bool {
        host_node_id(&ctx, &other) == Some(self.handle.0)
    }

    // https://dom.spec.whatwg.org/#dom-node-isequalnode
    #[qjs(rename = "isEqualNode")]
    fn is_equal_node<'js>(&self, ctx: Ctx<'js>, other: Value<'js>) -> Result<bool> {
        let Some(other) = host_node_id(&ctx, &other) else {
            return Ok(false);
        };
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(false);
        };
        Ok(nodes_equal(&parsed.dom, self.handle.0, other))
    }

    // https://dom.spec.whatwg.org/#dom-node-contains
    #[qjs(rename = "contains")]
    fn contains<'js>(&self, ctx: Ctx<'js>, other: Value<'js>) -> Result<bool> {
        let Some(other) = host_node_id(&ctx, &other) else {
            return Ok(false);
        };
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(false);
        };
        let mut cursor = Some(other);
        while let Some(id) = cursor {
            if id == self.handle.0 {
                return Ok(true);
            }
            cursor = parsed.dom.parent(id);
        }
        Ok(false)
    }

    // https://dom.spec.whatwg.org/#dom-node-getrootnode
    #[qjs(rename = "getRootNode")]
    fn get_root_node<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let mut root = self.handle.0;
        {
            let parsed = world.borrow();
            let Some(parsed) = parsed.document(self.handle.0) else {
                return Ok(Value::new_null(ctx));
            };
            while let Some(parent) = parsed.dom.parent(root) {
                root = parent;
            }
        }
        wrap_node(&ctx, root)
    }

    // https://dom.spec.whatwg.org/#dom-node-isconnected
    #[qjs(get, rename = "isConnected")]
    fn is_connected(&self, ctx: Ctx<'_>) -> Result<bool> {
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(false);
        };
        let mut root = self.handle.0;
        while let Some(parent) = parsed.dom.parent(root) {
            root = parent;
        }
        Ok(root == parsed.dom.document())
    }

    // https://dom.spec.whatwg.org/#dom-node-clonenode
    #[qjs(rename = "cloneNode")]
    fn clone_node<'js>(&self, ctx: Ctx<'js>, deep: Opt<bool>) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let mut world = world.borrow_mut();
        let Some(parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        if self.handle.0 == parsed.dom.document() {
            return Err(throw_dom(
                &ctx,
                "NotSupportedError",
                "cannot clone a document",
            ));
        }
        let clone = parsed
            .dom
            .clone_node(self.handle.0, deep.0.unwrap_or(false))
            .map_err(|err| throw_dom_error(&ctx, err))?;
        drop(world);
        wrap_node(&ctx, clone)
    }

    // https://dom.spec.whatwg.org/#dom-node-insertbefore
    #[qjs(rename = "insertBefore")]
    fn insert_before<'js>(
        &self,
        ctx: Ctx<'js>,
        node: Value<'js>,
        child: Value<'js>,
    ) -> Result<Value<'js>> {
        let node = required_node(&ctx, &node)?;
        let reference = optional_node(&ctx, &child)?;
        {
            let world = world(&ctx)?;
            let parsed = world.borrow();
            let Some(parsed) = parsed.document(self.handle.0) else {
                return Err(Exception::throw_type(&ctx, "no document"));
            };
            parsed
                .dom
                .validate_pre_insert(self.handle.0, node, reference)
                .map_err(|err| throw_dom_error(&ctx, err))?;
        }
        let node = adopt_across_documents(&ctx, self.handle.0, node)?;
        let world = world(&ctx)?;
        let mut world = world.borrow_mut();
        let Some(parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        let dom = &mut parsed.dom;
        dom.pre_insert(self.handle.0, node, reference)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        drop(world);
        wrap_node(&ctx, node)
    }

    // https://dom.spec.whatwg.org/#dom-node-removechild
    #[qjs(rename = "removeChild")]
    fn remove_child<'js>(&self, ctx: Ctx<'js>, child: Value<'js>) -> Result<Value<'js>> {
        let child = required_node(&ctx, &child)?;
        let world = world(&ctx)?;
        let mut world = world.borrow_mut();
        let Some(parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        if parsed.dom.parent(child) != Some(self.handle.0) {
            return Err(throw_dom(
                &ctx,
                "NotFoundError",
                "child is not a child of this node",
            ));
        }
        parsed
            .dom
            .detach(child)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        drop(world);
        wrap_node(&ctx, child)
    }

    // https://dom.spec.whatwg.org/#dom-node-replacechild
    #[qjs(rename = "replaceChild")]
    fn replace_child<'js>(
        &self,
        ctx: Ctx<'js>,
        node: Value<'js>,
        child: Value<'js>,
    ) -> Result<Value<'js>> {
        let node = required_node(&ctx, &node)?;
        let child = required_node(&ctx, &child)?;
        {
            let world = world(&ctx)?;
            let parsed = world.borrow();
            let Some(parsed) = parsed.document(self.handle.0) else {
                return Err(Exception::throw_type(&ctx, "no document"));
            };
            parsed
                .dom
                .validate_pre_insert(self.handle.0, node, Some(child))
                .map_err(|err| throw_dom_error(&ctx, err))?;
        }
        let node = adopt_across_documents(&ctx, self.handle.0, node)?;
        let world = world(&ctx)?;
        let mut world = world.borrow_mut();
        let Some(parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        parsed
            .dom
            .replace_child(self.handle.0, node, child)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        drop(world);
        wrap_node(&ctx, child)
    }

    // https://dom.spec.whatwg.org/#dom-node-lookupnamespaceuri
    #[qjs(rename = "lookupNamespaceURI")]
    fn lookup_namespace_uri<'js>(&self, ctx: Ctx<'js>, prefix: OptString) -> Result<Value<'js>> {
        let prefix = prefix.0.filter(|value| !value.is_empty());
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(Value::new_null(ctx));
        };
        match locate_namespace(&parsed.dom, self.handle.0, prefix.as_deref()) {
            Some(namespace) => string_value(&ctx, &namespace),
            None => Ok(Value::new_null(ctx)),
        }
    }

    // https://dom.spec.whatwg.org/#dom-node-lookupprefix
    #[qjs(rename = "lookupPrefix")]
    fn lookup_prefix<'js>(&self, ctx: Ctx<'js>, namespace: OptString) -> Result<Value<'js>> {
        let Some(namespace) = namespace.0.filter(|value| !value.is_empty()) else {
            return Ok(Value::new_null(ctx));
        };
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(Value::new_null(ctx));
        };
        match locate_prefix(&parsed.dom, self.handle.0, &namespace) {
            Some(prefix) => string_value(&ctx, &prefix),
            None => Ok(Value::new_null(ctx)),
        }
    }

    // https://dom.spec.whatwg.org/#dom-node-isdefaultnamespace
    #[qjs(rename = "isDefaultNamespace")]
    fn is_default_namespace(&self, ctx: Ctx<'_>, namespace: OptString) -> Result<bool> {
        let namespace = namespace.0.filter(|value| !value.is_empty());
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(false);
        };
        let found = locate_namespace(&parsed.dom, self.handle.0, None);
        Ok(found.as_deref() == namespace.as_deref())
    }

    // https://dom.spec.whatwg.org/#dom-childnode-remove
    #[qjs(rename = "remove")]
    fn remove(&self, ctx: Ctx<'_>) -> Result<()> {
        let world = world(&ctx)?;
        let mut world = world.borrow_mut();
        let Some(parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        parsed
            .dom
            .detach(self.handle.0)
            .map_err(|err| throw_dom_error(&ctx, err))
    }

    // https://dom.spec.whatwg.org/#dom-characterdata-length
    #[qjs(get, rename = "length")]
    fn length(&self, ctx: Ctx<'_>) -> Result<usize> {
        Ok(character_data(&ctx, self.handle.0)?.encode_utf16().count())
    }

    // https://dom.spec.whatwg.org/#dom-characterdata-substringdata
    #[qjs(rename = "substringData")]
    fn substring_data(
        &self,
        ctx: Ctx<'_>,
        offset: WebIdlUnsignedLong,
        count: WebIdlUnsignedLong,
    ) -> Result<String> {
        let data = character_data(&ctx, self.handle.0)?;
        let units: Vec<u16> = data.encode_utf16().collect();
        let offset = character_data_offset(&ctx, offset.0, units.len())?;
        let end = offset
            .saturating_add(usize::try_from(count.0).unwrap_or(usize::MAX))
            .min(units.len());
        Ok(String::from_utf16_lossy(&units[offset..end]))
    }

    // https://dom.spec.whatwg.org/#dom-characterdata-appenddata
    #[qjs(rename = "appendData")]
    fn append_data(&self, ctx: Ctx<'_>, data: WebIdlString) -> Result<()> {
        let mut current = character_data(&ctx, self.handle.0)?;
        current.push_str(&data.0);
        set_character_data(&ctx, self.handle.0, current)
    }

    // https://dom.spec.whatwg.org/#dom-characterdata-insertdata
    #[qjs(rename = "insertData")]
    fn insert_data(
        &self,
        ctx: Ctx<'_>,
        offset: WebIdlUnsignedLong,
        data: WebIdlString,
    ) -> Result<()> {
        let current = character_data(&ctx, self.handle.0)?;
        let mut units: Vec<u16> = current.encode_utf16().collect();
        let offset = character_data_offset(&ctx, offset.0, units.len())?;
        let insert: Vec<u16> = data.0.encode_utf16().collect();
        units.splice(offset..offset, insert);
        set_character_data(&ctx, self.handle.0, String::from_utf16_lossy(&units))
    }

    // https://dom.spec.whatwg.org/#dom-characterdata-deletedata
    #[qjs(rename = "deleteData")]
    fn delete_data(
        &self,
        ctx: Ctx<'_>,
        offset: WebIdlUnsignedLong,
        count: WebIdlUnsignedLong,
    ) -> Result<()> {
        let current = character_data(&ctx, self.handle.0)?;
        let mut units: Vec<u16> = current.encode_utf16().collect();
        let offset = character_data_offset(&ctx, offset.0, units.len())?;
        let end = offset
            .saturating_add(usize::try_from(count.0).unwrap_or(usize::MAX))
            .min(units.len());
        units.drain(offset..end);
        set_character_data(&ctx, self.handle.0, String::from_utf16_lossy(&units))
    }

    // https://dom.spec.whatwg.org/#dom-characterdata-replacedata
    #[qjs(rename = "replaceData")]
    fn replace_data(
        &self,
        ctx: Ctx<'_>,
        offset: WebIdlUnsignedLong,
        count: WebIdlUnsignedLong,
        data: WebIdlString,
    ) -> Result<()> {
        let current = character_data(&ctx, self.handle.0)?;
        let mut units: Vec<u16> = current.encode_utf16().collect();
        let offset = character_data_offset(&ctx, offset.0, units.len())?;
        let end = offset
            .saturating_add(usize::try_from(count.0).unwrap_or(usize::MAX))
            .min(units.len());
        units.splice(offset..end, data.0.encode_utf16());
        set_character_data(&ctx, self.handle.0, String::from_utf16_lossy(&units))
    }
}

struct OptString(Option<String>);

impl<'js> rquickjs::FromJs<'js> for OptString {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
        if value.is_null() || value.is_undefined() {
            return Ok(Self(None));
        }
        Ok(Self(Some(webidl_to_string(ctx, value)?)))
    }
}

/// `WebIDL` `DOMString` conversion: `ToString(value)`
/// (<https://webidl.spec.whatwg.org/#es-DOMString>).
struct WebIdlString(String);

impl<'js> rquickjs::FromJs<'js> for WebIdlString {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
        Ok(Self(webidl_to_string(ctx, value)?))
    }
}

/// `[LegacyNullToEmptyString]` `DOMString`: `null` becomes the empty string
/// (<https://webidl.spec.whatwg.org/#LegacyNullToEmptyString>).
struct LegacyNullString(String);

impl<'js> rquickjs::FromJs<'js> for LegacyNullString {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
        if value.is_null() {
            return Ok(Self(String::new()));
        }
        Ok(Self(webidl_to_string(ctx, value)?))
    }
}

/// An optional title argument: omitted and `undefined` mean "not given",
/// `null` is the string "null" like any other `DOMString`.
struct OptionalTitle(Option<String>);

impl<'js> rquickjs::FromJs<'js> for OptionalTitle {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
        if value.is_undefined() {
            return Ok(Self(None));
        }
        Ok(Self(Some(webidl_to_string(ctx, value)?)))
    }
}

/// `WebIDL` `unsigned long` conversion
/// (<https://webidl.spec.whatwg.org/#es-unsigned-long>).
struct WebIdlUnsignedLong(u32);

impl<'js> rquickjs::FromJs<'js> for WebIdlUnsignedLong {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
        let to_number: Function = ctx.globals().get("Number")?;
        let number: f64 = to_number.call((value,))?;
        Ok(Self(webidl_unsigned_long(number)))
    }
}

/// `ToString` without a raw conversion API: call the global `String`.
fn webidl_to_string<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<String> {
    let to_string: Function = ctx.globals().get("String")?;
    let text: rquickjs::String = to_string.call((value,))?;
    text.to_string()
}

/// Whether the document that owns `id` reports `text/html`.
fn document_is_html_content(ctx: &Ctx<'_>, id: NodeId) -> bool {
    let Ok(world_rc) = world(ctx) else {
        return false;
    };
    let world = world_rc.borrow();
    world
        .document(id)
        .is_some_and(|parsed| parsed.content_type == "text/html")
}

/// Whether `id` is the parser's main document (the one with a browsing
/// context's URL); created documents report `about:blank`.
fn is_main_document(ctx: &Ctx<'_>, id: NodeId) -> bool {
    let Ok(world_rc) = world(ctx) else {
        return false;
    };
    let world = world_rc.borrow();
    world
        .parsed
        .as_ref()
        .is_some_and(|parsed| parsed.dom.document_id() == id.document_id())
}

fn document_url_string(ctx: &Ctx<'_>, id: NodeId) -> String {
    if is_main_document(ctx, id) {
        return world(ctx)
            .map(|world| world.borrow().document_url.as_str().to_owned())
            .unwrap_or_default();
    }
    "about:blank".to_owned()
}

/// Whether the document that owns `id` is an HTML document
/// (`text/html` or `application/xhtml+xml`).
fn document_is_html(ctx: &Ctx<'_>, id: NodeId) -> bool {
    let Ok(world_rc) = world(ctx) else {
        return false;
    };
    let world = world_rc.borrow();
    world
        .document(id)
        .is_some_and(|parsed| matches!(parsed.content_type, "text/html" | "application/xhtml+xml"))
}

/// Whether `id` names an element in the HTML namespace.
fn element_is_html(ctx: &Ctx<'_>, id: NodeId) -> bool {
    let Ok(world) = world(ctx) else {
        return false;
    };
    let parsed = world.borrow();
    matches!(
        parsed
            .document(id)
            .and_then(|parsed| parsed.dom.get(id))
            .map(|node| node.kind()),
        Some(NodeKind::Element { name, .. }) if name.ns == html_namespace()
    )
}

/// [Converting nodes into a node](https://dom.spec.whatwg.org/#convert-nodes-into-a-node):
/// strings become `Text`, one node stays itself, several become a fragment.
fn convert_nodes_into_node<'js>(
    ctx: &Ctx<'js>,
    document: NodeId,
    nodes: Rest<Value<'js>>,
) -> Result<NodeId> {
    let world = world(ctx)?;
    let mut world = world.borrow_mut();
    let Some(parsed) = world.document_mut(document) else {
        return Err(Exception::throw_type(ctx, "no document"));
    };
    let dom = &mut parsed.dom;
    let mut ids: Vec<NodeId> = Vec::with_capacity(nodes.0.len());
    for value in nodes.0 {
        if let Some(id) = host_node_id(ctx, &value) {
            if id.document_id() != document.document_id() {
                return Err(throw_dom(
                    ctx,
                    "HierarchyRequestError",
                    "nodes belong to different documents",
                ));
            }
            ids.push(id);
        } else {
            let text = webidl_to_string(ctx, value)?;
            ids.push(dom.create_text(text));
        }
    }
    if ids.len() == 1 {
        return Ok(ids[0]);
    }
    let fragment = dom.create_fragment();
    for id in ids {
        dom.append(fragment, id)
            .map_err(|err| throw_dom_error(ctx, err))?;
    }
    Ok(fragment)
}

/// A selector syntax error is a `SyntaxError` `DOMException`
/// (<https://dom.spec.whatwg.org/#scope-match-a-selectors-string>).
fn select_error(ctx: &Ctx<'_>, err: &dom::SelectError) -> rquickjs::Error {
    match err {
        dom::SelectError::Syntax(_) => throw_dom(ctx, "SyntaxError", &err.to_string()),
        _ => Exception::throw_internal(ctx, &err.to_string()),
    }
}

/// Integer conversion from a `double`
/// (<https://webidl.spec.whatwg.org/#abstract-opdef-converttoint>): NaN and
/// infinities become 0, then the truncated value wraps modulo 2^32.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "`rem_euclid` returns a value in [0, 2^32), so the cast is exact and non-negative"
)]
fn webidl_unsigned_long(number: f64) -> u32 {
    if !number.is_finite() || number == 0.0 {
        return 0;
    }
    let modulo = number.trunc().rem_euclid(4_294_967_296.0);
    modulo as u32
}

pub(super) fn install(ctx: &Ctx<'_>, world: &Rc<RefCell<World>>) -> Result<()> {
    ctx.store_userdata(SharedWorld(world.clone()))
        .map_err(|err| Exception::throw_internal(ctx, &err.to_string()))?;
    let globals = ctx.globals();
    Class::<JsEvent>::define(&globals)?;
    Class::<JsNode>::define(&globals)?;
    Class::<JsCollection>::define(&globals)?;
    Class::<JsDomException>::define(&globals)?;
    Class::<JsImplementation>::define(&globals)?;
    Class::<JsTokenList>::define(&globals)?;
    Class::<JsAttr>::define(&globals)?;
    Class::<JsNamedNodeMap>::define(&globals)?;
    Class::<JsDomParser>::define(&globals)?;
    install_brands(ctx)?;
    install_collection_brand(ctx)?;
    install_dom_exception_codes(ctx)?;

    let document_id = world
        .borrow()
        .parsed
        .as_ref()
        .map(|parsed| parsed.dom.document());
    if let Some(id) = document_id {
        globals.set("document", wrap_node(ctx, id)?)?;
    }

    let pathname = world.borrow().document_url.path().to_owned();
    let href = world.borrow().document_url.as_str().to_owned();
    let location = Object::new(ctx.clone())?;
    location.set("pathname", pathname)?;
    location.set("href", href)?;
    globals.set("location", location)?;
    globals.set("window", globals.clone())?;
    globals.set("self", globals.clone())?;
    // https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-frames
    globals.set("frames", globals.clone())?;
    // A top-level context has no child browsing contexts yet.
    // https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-length
    globals.set("length", 0_i32)?;
    // https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-parent
    globals.set("parent", globals.clone())?;
    // https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-top
    globals.set("top", globals.clone())?;
    globals.set("opener", Value::new_null(ctx.clone()))?;

    globals.set(
        "addEventListener",
        rquickjs::prelude::Func::from(window_add_event_listener),
    )?;
    Ok(())
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes Ctx by value"
)]
fn window_add_event_listener<'js>(
    ctx: Ctx<'js>,
    typ: String,
    callback: Function<'js>,
) -> Result<()> {
    add_listener(&ctx, EventTargetKey::Window, typ, callback)
}

pub(super) fn fire_window_load(ctx: &Ctx<'_>) -> Result<()> {
    let event = Class::instance(ctx.clone(), JsEvent { typ: "load".into() })?;
    fire(ctx, EventTargetKey::Window, "load", &event)?;
    Ok(())
}

pub(super) fn host_node_id<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Option<NodeId> {
    Class::<JsNode>::from_js(ctx, value.clone())
        .ok()
        .map(|node| node.borrow().node_id())
}

fn wrap_node<'js>(ctx: &Ctx<'js>, id: NodeId) -> Result<Value<'js>> {
    let world_rc = world(ctx)?;
    if let Some(saved) = world_rc.borrow().wrapper(id)
        && let Some(value) = deref_weak(ctx, saved)?
    {
        return Ok(value);
    }
    let value = instantiate_node(ctx, id)?;
    let weak = make_weak(ctx, value.clone())?;
    world_rc
        .borrow_mut()
        .intern_wrapper(id, Persistent::save(ctx, weak));
    Ok(value)
}

fn instantiate_node<'js>(ctx: &Ctx<'js>, id: NodeId) -> Result<Value<'js>> {
    let brand = with_node_kind(ctx, id, |kind| match kind {
        Some(NodeKind::Document) => Some(if document_is_html_content(ctx, id) {
            "Document"
        } else {
            "XMLDocument"
        }),
        Some(NodeKind::Element { name, .. }) => Some(element_interface(name)),
        Some(NodeKind::Text { .. }) => Some("Text"),
        Some(NodeKind::CDataSection { .. }) => Some("CDATASection"),
        Some(NodeKind::ProcessingInstruction { .. }) => Some("ProcessingInstruction"),
        Some(NodeKind::Comment { .. }) => Some("Comment"),
        Some(NodeKind::Doctype { .. }) => Some("DocumentType"),
        Some(NodeKind::Fragment) => Some("DocumentFragment"),
        None => None,
    })?;
    let Some(brand) = brand else {
        return Err(Exception::throw_type(ctx, "stale node"));
    };
    let class = Class::instance(ctx.clone(), JsNode { handle: Handle(id) })?;
    if let Some(proto) = class_proto(ctx, brand)? {
        class.set_prototype(Some(&proto))?;
    }
    Ok(Class::into_value(class))
}

/// The element interface for a qualified name
/// (<https://html.spec.whatwg.org/multipage/dom.html#elements-in-the-dom:html-element>
/// and its SVG counterparts). Names outside the HTML and SVG namespaces keep
/// the base `Element` interface.
fn element_interface(name: &QualName) -> &'static str {
    if name.ns == html_namespace() {
        html_element_interface(name.local.as_ref())
    } else if name.ns == svg_namespace() {
        "SVGElement"
    } else {
        "Element"
    }
}

/// `local` name to element interface
/// (<https://html.spec.whatwg.org/multipage/dom.html#elements-in-the-dom:html-element>).
const ELEMENT_INTERFACES: &[(&str, &str)] = &[
    ("a", "HTMLAnchorElement"),
    ("abbr", "HTMLElement"),
    ("acronym", "HTMLElement"),
    ("address", "HTMLElement"),
    ("area", "HTMLAreaElement"),
    ("article", "HTMLElement"),
    ("aside", "HTMLElement"),
    ("audio", "HTMLAudioElement"),
    ("b", "HTMLElement"),
    ("base", "HTMLBaseElement"),
    ("bdi", "HTMLElement"),
    ("bdo", "HTMLElement"),
    ("bgsound", "HTMLElement"),
    ("big", "HTMLElement"),
    ("blockquote", "HTMLElement"),
    ("body", "HTMLBodyElement"),
    ("br", "HTMLBRElement"),
    ("button", "HTMLButtonElement"),
    ("canvas", "HTMLCanvasElement"),
    ("caption", "HTMLTableCaptionElement"),
    ("center", "HTMLElement"),
    ("cite", "HTMLElement"),
    ("code", "HTMLElement"),
    ("col", "HTMLTableColElement"),
    ("colgroup", "HTMLTableColElement"),
    ("data", "HTMLDataElement"),
    ("datalist", "HTMLDataListElement"),
    ("dd", "HTMLElement"),
    ("del", "HTMLModElement"),
    ("details", "HTMLElement"),
    ("dfn", "HTMLElement"),
    ("dialog", "HTMLDialogElement"),
    ("dir", "HTMLDirectoryElement"),
    ("div", "HTMLDivElement"),
    ("dl", "HTMLDListElement"),
    ("dt", "HTMLElement"),
    ("embed", "HTMLEmbedElement"),
    ("fieldset", "HTMLFieldSetElement"),
    ("figcaption", "HTMLElement"),
    ("figure", "HTMLElement"),
    ("font", "HTMLFontElement"),
    ("footer", "HTMLElement"),
    ("form", "HTMLFormElement"),
    ("frame", "HTMLFrameElement"),
    ("frameset", "HTMLFrameSetElement"),
    ("h1", "HTMLHeadingElement"),
    ("h2", "HTMLHeadingElement"),
    ("h3", "HTMLHeadingElement"),
    ("h4", "HTMLHeadingElement"),
    ("h5", "HTMLHeadingElement"),
    ("h6", "HTMLHeadingElement"),
    ("head", "HTMLHeadElement"),
    ("header", "HTMLElement"),
    ("hgroup", "HTMLElement"),
    ("hr", "HTMLHRElement"),
    ("html", "HTMLHtmlElement"),
    ("i", "HTMLElement"),
    ("iframe", "HTMLIFrameElement"),
    ("img", "HTMLImageElement"),
    ("input", "HTMLInputElement"),
    ("ins", "HTMLModElement"),
    ("isindex", "HTMLElement"),
    ("kbd", "HTMLElement"),
    ("label", "HTMLLabelElement"),
    ("legend", "HTMLLegendElement"),
    ("li", "HTMLLIElement"),
    ("link", "HTMLLinkElement"),
    ("main", "HTMLElement"),
    ("map", "HTMLMapElement"),
    ("mark", "HTMLElement"),
    ("marquee", "HTMLElement"),
    ("meta", "HTMLMetaElement"),
    ("meter", "HTMLMeterElement"),
    ("nav", "HTMLElement"),
    ("nobr", "HTMLElement"),
    ("noframes", "HTMLElement"),
    ("noscript", "HTMLElement"),
    ("object", "HTMLObjectElement"),
    ("ol", "HTMLOListElement"),
    ("optgroup", "HTMLOptGroupElement"),
    ("option", "HTMLOptionElement"),
    ("output", "HTMLOutputElement"),
    ("p", "HTMLParagraphElement"),
    ("param", "HTMLParamElement"),
    ("pre", "HTMLPreElement"),
    ("progress", "HTMLProgressElement"),
    ("q", "HTMLQuoteElement"),
    ("rp", "HTMLElement"),
    ("rt", "HTMLElement"),
    ("ruby", "HTMLElement"),
    ("s", "HTMLElement"),
    ("samp", "HTMLElement"),
    ("script", "HTMLScriptElement"),
    ("section", "HTMLElement"),
    ("select", "HTMLSelectElement"),
    ("small", "HTMLElement"),
    ("source", "HTMLSourceElement"),
    ("spacer", "HTMLElement"),
    ("span", "HTMLSpanElement"),
    ("strike", "HTMLElement"),
    ("style", "HTMLStyleElement"),
    ("sub", "HTMLElement"),
    ("summary", "HTMLElement"),
    ("sup", "HTMLElement"),
    ("table", "HTMLTableElement"),
    ("tbody", "HTMLTableSectionElement"),
    ("td", "HTMLTableCellElement"),
    ("template", "HTMLTemplateElement"),
    ("textarea", "HTMLTextAreaElement"),
    ("th", "HTMLTableCellElement"),
    ("time", "HTMLTimeElement"),
    ("title", "HTMLTitleElement"),
    ("tr", "HTMLTableRowElement"),
    ("track", "HTMLTrackElement"),
    ("tt", "HTMLElement"),
    ("u", "HTMLElement"),
    ("ul", "HTMLUListElement"),
    ("unknown", "HTMLUnknownElement"),
    ("var", "HTMLElement"),
    ("video", "HTMLVideoElement"),
    ("wbr", "HTMLElement"),
];

fn html_element_interface(local: &str) -> &'static str {
    if let Some((_, interface)) = ELEMENT_INTERFACES.iter().find(|(name, _)| *name == local) {
        return interface;
    }
    if local.contains('-') {
        return "HTMLElement";
    }
    "HTMLUnknownElement"
}

fn make_weak<'js>(ctx: &Ctx<'js>, target: Value<'js>) -> Result<Value<'js>> {
    let ctor: Constructor = ctx.globals().get("WeakRef")?;
    ctor.construct((target,))
}

fn deref_weak<'js>(
    ctx: &Ctx<'js>,
    saved: Persistent<Value<'static>>,
) -> Result<Option<Value<'js>>> {
    let weak = saved.restore(ctx)?;
    let object = weak
        .as_object()
        .ok_or_else(|| Exception::throw_type(ctx, "weak wrapper"))?;
    let deref: Function = object.get("deref")?;
    let value: Value = deref.call((This(object.clone()),))?;
    if value.is_undefined() {
        Ok(None)
    } else {
        Ok(Some(value))
    }
}

/// Prototype-brand installation script for the browser-realm platform
/// objects; one `define` per `WebIDL` interface.
const INSTALL_BRANDS_JS: &str = r"
(function() {
  const native = globalThis.Node.prototype;
  function illegal() { throw new TypeError('Illegal constructor'); }
  function define(name, parent, members) {
    const ctor = function() { return illegal(); };
    const proto = Object.create(parent ? parent.prototype : Object.prototype);
    for (const member of members) {
      const descriptor = Object.getOwnPropertyDescriptor(native, member);
      if (descriptor) {
        descriptor.configurable = true;
        Object.defineProperty(proto, member, descriptor);
      }
    }
    Object.defineProperty(proto, 'constructor', { value: ctor, writable: true, configurable: true });
    Object.defineProperty(ctor, 'prototype', { value: proto, writable: false });
    Object.defineProperty(globalThis, name, { value: ctor, writable: true, configurable: true });
    return ctor;
  }
  const EventTargetInterface = define('EventTarget', null, [
    'addEventListener', 'dispatchEvent'
  ]);
  const NodeInterface = define('Node', EventTargetInterface, [
    'nodeType', 'nodeName', 'firstChild', 'lastChild', 'nextSibling',
    'previousSibling', 'parentNode', 'childNodes', 'appendChild',
    'ownerDocument', 'hasChildNodes', 'nodeValue', 'textContent', 'isSameNode',
    'isEqualNode', 'contains', 'getRootNode', 'isConnected', 'cloneNode',
    'insertBefore', 'removeChild', 'replaceChild', 'lookupNamespaceURI',
    'lookupPrefix', 'isDefaultNamespace', 'normalize',
    'compareDocumentPosition'
  ]);
  for (const [name, value] of [
    ['ELEMENT_NODE', 1],
    ['ATTRIBUTE_NODE', 2],
    ['TEXT_NODE', 3],
    ['CDATA_SECTION_NODE', 4],
    ['ENTITY_REFERENCE_NODE', 5],
    ['ENTITY_NODE', 6],
    ['PROCESSING_INSTRUCTION_NODE', 7],
    ['COMMENT_NODE', 8],
    ['DOCUMENT_NODE', 9],
    ['DOCUMENT_TYPE_NODE', 10],
    ['DOCUMENT_FRAGMENT_NODE', 11],
    ['NOTATION_NODE', 12],
    ['DOCUMENT_POSITION_DISCONNECTED', 1],
    ['DOCUMENT_POSITION_PRECEDING', 2],
    ['DOCUMENT_POSITION_FOLLOWING', 4],
    ['DOCUMENT_POSITION_CONTAINS', 8],
    ['DOCUMENT_POSITION_CONTAINED_BY', 16],
    ['DOCUMENT_POSITION_IMPLEMENTATION_SPECIFIC', 32],
  ]) {
    Object.defineProperty(NodeInterface, name, { value, writable: false, configurable: true });
    Object.defineProperty(NodeInterface.prototype, name, { value, writable: false, configurable: true });
  }
  const DocumentInterface = define('Document', NodeInterface, [
    'createElement', 'createElementNS', 'createTextNode', 'createComment',
    'createProcessingInstruction', 'createCDATASection', 'createAttribute',
    'createAttributeNS', 'createDocumentFragment', 'write',
    'getElementById', 'getElementsByTagName',
    'getElementsByTagNameNS', 'getElementsByClassName',
    'body', 'documentElement', 'doctype', 'readyState', 'implementation',
    'children', 'firstElementChild', 'lastElementChild', 'childElementCount',
    'append', 'prepend', 'replaceChildren', 'querySelector', 'querySelectorAll',
    'URL', 'documentURI', 'location', 'characterSet', 'charset',
    'inputEncoding', 'contentType', 'compatMode', 'title',
    'getElementsByName', 'importNode'
  ]);
  const ElementInterface = define('Element', NodeInterface, [
    'getElementsByTagName', 'getElementsByTagNameNS', 'getElementsByClassName',
    'getAttribute',
    'setAttribute', 'hasAttribute', 'getAttributeNS', 'setAttributeNS',
    'hasAttributeNS', 'removeAttribute', 'removeAttributeNS',
    'getAttributeNames', 'toggleAttribute', 'attributes', 'hasAttributes',
    'getAttributeNode', 'getAttributeNodeNS', 'setAttributeNode',
    'setAttributeNodeNS', 'removeAttributeNode', 'matches', 'closest',
    'children',
    'firstElementChild', 'lastElementChild', 'childElementCount', 'append',
    'prepend', 'replaceChildren', 'querySelector', 'querySelectorAll',
    'before', 'after', 'replaceWith', 'previousElementSibling',
    'nextElementSibling', 'tagName', 'localName', 'prefix', 'namespaceURI',
    'className', 'classList', 'id', 'src', 'href', 'name', 'content', 'style',
    'remove'
  ]);
  // classList is `[PutForwards=value]`: assigning to it sets `.value`
  // (<https://dom.spec.whatwg.org/#dom-element-classlist>).
  {
    const descriptor = Object.getOwnPropertyDescriptor(ElementInterface.prototype, 'classList');
    Object.defineProperty(ElementInterface.prototype, 'classList', {
      get: descriptor.get,
      set: function(value) { this.classList.value = value; },
      enumerable: true,
      configurable: true,
    });
  }
  const XMLDocumentInterface = define('XMLDocument', DocumentInterface, []);
  const CharacterDataInterface = define('CharacterData', NodeInterface, [
    'data', 'length', 'substringData', 'appendData', 'insertData', 'deleteData',
    'replaceData', 'remove', 'before', 'after', 'replaceWith',
    'previousElementSibling', 'nextElementSibling'
  ]);
  const TextInterface = define('Text', CharacterDataInterface, []);
  const CDATASectionInterface = define('CDATASection', TextInterface, []);
  const ProcessingInstructionInterface = define('ProcessingInstruction', CharacterDataInterface, [
    'target'
  ]);
  const CommentInterface = define('Comment', CharacterDataInterface, []);
  const DocumentTypeInterface = define('DocumentType', NodeInterface, [
    'remove', 'before', 'after', 'replaceWith', 'name', 'publicId', 'systemId'
  ]);
  const DocumentFragmentInterface = define('DocumentFragment', NodeInterface, [
    'children', 'firstElementChild', 'lastElementChild', 'childElementCount',
    'append', 'prepend', 'replaceChildren', 'querySelector', 'querySelectorAll',
    'getElementById'
  ]);
  const HTMLElementInterface = define('HTMLElement', ElementInterface, []);
  const HTMLUnknownElementInterface = define('HTMLUnknownElement', HTMLElementInterface, []);
  const HTMLMediaElementInterface = define('HTMLMediaElement', HTMLElementInterface, []);
  const SVGElementInterface = define('SVGElement', ElementInterface, []);
  const table = {
    Document: DocumentInterface.prototype,
    XMLDocument: XMLDocumentInterface.prototype,
    Element: ElementInterface.prototype,
    CharacterData: CharacterDataInterface.prototype,
    Text: TextInterface.prototype,
    CDATASection: CDATASectionInterface.prototype,
    ProcessingInstruction: ProcessingInstructionInterface.prototype,
    Comment: CommentInterface.prototype,
    DocumentType: DocumentTypeInterface.prototype,
    DocumentFragment: DocumentFragmentInterface.prototype,
    HTMLElement: HTMLElementInterface.prototype,
    HTMLUnknownElement: HTMLUnknownElementInterface.prototype,
    HTMLMediaElement: HTMLMediaElementInterface.prototype,
    SVGElement: SVGElementInterface.prototype,
  };
  // Every element interface chains to HTMLElement except the media pair,
  // which chains through HTMLMediaElement, and SVG, which chains to Element.
  for (const [name, parent] of [
    ['HTMLAnchorElement', HTMLElementInterface],
    ['HTMLAreaElement', HTMLElementInterface],
    ['HTMLAudioElement', HTMLMediaElementInterface],
    ['HTMLBaseElement', HTMLElementInterface],
    ['HTMLBodyElement', HTMLElementInterface],
    ['HTMLBRElement', HTMLElementInterface],
    ['HTMLButtonElement', HTMLElementInterface],
    ['HTMLCanvasElement', HTMLElementInterface],
    ['HTMLDataElement', HTMLElementInterface],
    ['HTMLDataListElement', HTMLElementInterface],
    ['HTMLDialogElement', HTMLElementInterface],
    ['HTMLDirectoryElement', HTMLElementInterface],
    ['HTMLDivElement', HTMLElementInterface],
    ['HTMLDListElement', HTMLElementInterface],
    ['HTMLEmbedElement', HTMLElementInterface],
    ['HTMLFieldSetElement', HTMLElementInterface],
    ['HTMLFontElement', HTMLElementInterface],
    ['HTMLFormElement', HTMLElementInterface],
    ['HTMLFrameElement', HTMLElementInterface],
    ['HTMLFrameSetElement', HTMLElementInterface],
    ['HTMLHeadingElement', HTMLElementInterface],
    ['HTMLHeadElement', HTMLElementInterface],
    ['HTMLHRElement', HTMLElementInterface],
    ['HTMLHtmlElement', HTMLElementInterface],
    ['HTMLIFrameElement', HTMLElementInterface],
    ['HTMLImageElement', HTMLElementInterface],
    ['HTMLInputElement', HTMLElementInterface],
    ['HTMLLabelElement', HTMLElementInterface],
    ['HTMLLIElement', HTMLElementInterface],
    ['HTMLLegendElement', HTMLElementInterface],
    ['HTMLLinkElement', HTMLElementInterface],
    ['HTMLMapElement', HTMLElementInterface],
    ['HTMLMetaElement', HTMLElementInterface],
    ['HTMLMeterElement', HTMLElementInterface],
    ['HTMLModElement', HTMLElementInterface],
    ['HTMLObjectElement', HTMLElementInterface],
    ['HTMLOListElement', HTMLElementInterface],
    ['HTMLOptGroupElement', HTMLElementInterface],
    ['HTMLOptionElement', HTMLElementInterface],
    ['HTMLOutputElement', HTMLElementInterface],
    ['HTMLParagraphElement', HTMLElementInterface],
    ['HTMLParamElement', HTMLElementInterface],
    ['HTMLPreElement', HTMLElementInterface],
    ['HTMLProgressElement', HTMLElementInterface],
    ['HTMLQuoteElement', HTMLElementInterface],
    ['HTMLScriptElement', HTMLElementInterface],
    ['HTMLSelectElement', HTMLElementInterface],
    ['HTMLSourceElement', HTMLElementInterface],
    ['HTMLSpanElement', HTMLElementInterface],
    ['HTMLStyleElement', HTMLElementInterface],
    ['HTMLTableCaptionElement', HTMLElementInterface],
    ['HTMLTableCellElement', HTMLElementInterface],
    ['HTMLTableColElement', HTMLElementInterface],
    ['HTMLTableElement', HTMLElementInterface],
    ['HTMLTableRowElement', HTMLElementInterface],
    ['HTMLTableSectionElement', HTMLElementInterface],
    ['HTMLTemplateElement', HTMLElementInterface],
    ['HTMLTextAreaElement', HTMLElementInterface],
    ['HTMLTimeElement', HTMLElementInterface],
    ['HTMLTitleElement', HTMLElementInterface],
    ['HTMLTrackElement', HTMLElementInterface],
    ['HTMLUListElement', HTMLElementInterface],
    ['HTMLVideoElement', HTMLMediaElementInterface],
  ]) {
    table[name] = define(name, parent, []).prototype;
  }
  Object.defineProperty(globalThis, '__tb_brandTable', {
    enumerable: false,
    configurable: true,
    value: table,
  });
})();
";

fn install_brands(ctx: &Ctx<'_>) -> Result<()> {
    ctx.eval::<(), _>(INSTALL_BRANDS_JS)?;
    let table: Object = ctx.globals().get("__tb_brandTable")?;
    let entries = table
        .props::<String, Object>()
        .collect::<Result<Vec<(String, Object)>>>()?;
    for (name, proto) in entries {
        world(ctx)?
            .borrow_mut()
            .intern_brand(name, Persistent::save(ctx, proto));
    }
    ctx.eval::<(), _>("delete globalThis.__tb_brandTable")?;
    Ok(())
}

/// Collection and legacy index-property proxies for the browser realm.
const INSTALL_COLLECTIONS_JS: &str = r"
(function() {
  const native = globalThis.NodeList.prototype;
  const ctor = function() { throw new TypeError('Illegal constructor'); };
  const proto = Object.create(Object.prototype);
  for (const member of ['length', 'item']) {
    const descriptor = Object.getOwnPropertyDescriptor(native, member);
    if (descriptor) Object.defineProperty(proto, member, descriptor);
  }
  Object.defineProperty(proto, 'constructor', { value: ctor, writable: true, configurable: true });
  Object.defineProperty(ctor, 'prototype', { value: proto, writable: false });
  Object.defineProperty(globalThis, 'HTMLCollection', { value: ctor, writable: true, configurable: true });
  Object.defineProperty(globalThis, '__tb_liveCollection', {
    enumerable: false,
    configurable: false,
    writable: false,
    value: function(target) {
      // Only platform methods need binding to the target; Object.prototype
      // built-ins like hasOwnProperty must see the proxy as `this`.
      function isPlatformMethod(inner, property) {
        let proto = Object.getPrototypeOf(inner);
        while (proto && proto !== Object.prototype) {
          if (Object.prototype.hasOwnProperty.call(proto, property)) {
            return true;
          }
          proto = Object.getPrototypeOf(proto);
        }
        return false;
      }
      return new Proxy(target, {
        get: function(inner, property) {
          if (typeof property === 'string' && /^(0|[1-9][0-9]*)$/.test(property)) {
            const index = Number(property);
            return index < inner.length ? inner.item(index) : undefined;
          }
          const value = Reflect.get(inner, property, inner);
          if (typeof value === 'function' && isPlatformMethod(inner, property)) {
            return value.bind(inner);
          }
          return value;
        },
        has: function(inner, property) {
          if (typeof property === 'string' && /^(0|[1-9][0-9]*)$/.test(property)) {
            return Number(property) < inner.length;
          }
          return Reflect.has(inner, property);
        },
        ownKeys: function(inner) {
          const keys = [];
          for (let i = 0; i < inner.length; i++) {
            keys.push(String(i));
          }
          return keys;
        },
        getOwnPropertyDescriptor: function(inner, property) {
          if (typeof property === 'string' && /^(0|[1-9][0-9]*)$/.test(property)) {
            const index = Number(property);
            if (index < inner.length) {
              return {
                value: inner.item(index),
                enumerable: true,
                configurable: true,
                writable: false,
              };
            }
          }
          return Reflect.getOwnPropertyDescriptor(inner, property);
        },
        set: function(inner, property, value) {
          if (typeof property === 'string' && /^(0|[1-9][0-9]*)$/.test(property)) {
            return true;
          }
          return Reflect.set(inner, property, value, inner);
        }
      });
    }
  });
  // NamedNodeMap exposes both indexed and named properties, and its own
  // property names are the indices followed by the qualified names
  // (<https://dom.spec.whatwg.org/#interface-namednodemap>).
  // NamedNodeMap exposes indexed and named properties as real own
  // properties; interface members and prototype methods always win.
  Object.defineProperty(globalThis, '__tb_refreshNamedNodeMap', {
    enumerable: false,
    configurable: false,
    writable: false,
    value: function(map, names, lowercaseOnly) {
      for (const key of Object.getOwnPropertyNames(map)) {
        delete map[key];
      }
      for (let i = 0; i < names.length; i++) {
        const attr = map.item(i);
        Object.defineProperty(map, String(i), {
          value: attr,
          enumerable: true,
          configurable: true,
          writable: false,
        });
        const name = names[i];
        const named = !lowercaseOnly || !/[A-Z]/.test(name);
        if (named && !(name in map)) {
          Object.defineProperty(map, name, {
            value: attr,
            enumerable: false,
            configurable: true,
            writable: false,
          });
        }
      }
    }
  });
})();
";

fn install_collection_brand(ctx: &Ctx<'_>) -> Result<()> {
    ctx.eval::<(), _>(INSTALL_COLLECTIONS_JS)?;
    let ctor: Function = ctx.globals().get("HTMLCollection")?;
    let proto: Object = ctor.get("prototype")?;
    world(ctx)?
        .borrow_mut()
        .intern_brand("HTMLCollection", Persistent::save(ctx, proto));
    Ok(())
}

fn class_proto<'js>(ctx: &Ctx<'js>, name: &str) -> Result<Option<Object<'js>>> {
    let Some(saved) = world(ctx)?.borrow().brand(name) else {
        return Ok(None);
    };
    Ok(Some(saved.restore(ctx)?))
}

/// `WebIDL` constants appear on the `interface object`, the `interface
/// prototype object`, and the `named constructor`
/// (<https://webidl.spec.whatwg.org/#interface-object>).
fn install_dom_exception_codes(ctx: &Ctx<'_>) -> Result<()> {
    let ctor: Object = ctx.globals().get("DOMException")?;
    let proto: Object = ctor.get("prototype")?;
    for (name, code) in DOM_EXCEPTION_CODES {
        ctor.set(name, code)?;
        proto.set(name, code)?;
    }
    Ok(())
}

fn world(ctx: &Ctx<'_>) -> Result<Rc<RefCell<World>>> {
    ctx.userdata::<SharedWorld>()
        .map(|guard| guard.0.clone())
        .ok_or_else(|| Exception::throw_internal(ctx, "missing JS world"))
}

fn add_listener<'js>(
    ctx: &Ctx<'js>,
    target: EventTargetKey,
    typ: String,
    callback: Function<'js>,
) -> Result<()> {
    let saved = Persistent::save(ctx, callback);
    world(ctx)?.borrow_mut().add_listener(
        target,
        Listener {
            typ,
            callback: saved,
        },
    );
    Ok(())
}

fn fire<'js>(
    ctx: &Ctx<'js>,
    target: EventTargetKey,
    typ: &str,
    event: &Class<'js, JsEvent>,
) -> Result<bool> {
    let callbacks = world(ctx)?.borrow().listeners(target, typ);
    for callback in callbacks {
        let func = callback.restore(ctx)?;
        func.call::<_, ()>((event.clone(),))?;
    }
    Ok(true)
}

fn with_node_kind<T>(
    ctx: &Ctx<'_>,
    id: NodeId,
    read: impl FnOnce(Option<&NodeKind>) -> T,
) -> Result<T> {
    let world = world(ctx)?;
    let parsed = world.borrow();
    let Some(parsed) = parsed.document(id) else {
        return Err(Exception::throw_type(ctx, "no document"));
    };
    Ok(read(parsed.dom.get(id).map(|node| node.kind())))
}

fn character_data(ctx: &Ctx<'_>, id: NodeId) -> Result<String> {
    with_node_kind(ctx, id, |kind| match kind {
        Some(
            NodeKind::Text { data }
            | NodeKind::CDataSection { data }
            | NodeKind::ProcessingInstruction { data, .. }
            | NodeKind::Comment { data },
        ) => data.clone(),
        _ => String::new(),
    })
}

/// [Replaces data](https://dom.spec.whatwg.org/#concept-cd-replace) on a
/// `CharacterData` node; other kinds are a silent no-op (`nodeValue` setter).
fn set_character_data(ctx: &Ctx<'_>, id: NodeId, data: String) -> Result<()> {
    let world = world(ctx)?;
    let mut world = world.borrow_mut();
    let Some(parsed) = world.document_mut(id) else {
        return Ok(());
    };
    match parsed.dom.get(id).map(|node| node.kind()) {
        Some(NodeKind::Text { .. }) => parsed
            .dom
            .set_text(id, data)
            .map_err(|err| throw_dom_error(ctx, err)),
        Some(NodeKind::CDataSection { .. }) => parsed
            .dom
            .set_cdata_section(id, data)
            .map_err(|err| throw_dom_error(ctx, err)),
        Some(NodeKind::ProcessingInstruction { .. }) => parsed
            .dom
            .set_processing_instruction(id, data)
            .map_err(|err| throw_dom_error(ctx, err)),
        Some(NodeKind::Comment { .. }) => parsed
            .dom
            .set_comment(id, data)
            .map_err(|err| throw_dom_error(ctx, err)),
        _ => Ok(()),
    }
}

/// `WebIDL` `unsigned long` offset conversion plus the `CharacterData` bounds
/// check: offsets beyond the data throw `IndexSizeError`
/// (<https://dom.spec.whatwg.org/#concept-cd-substring>).
fn character_data_offset(ctx: &Ctx<'_>, offset: u32, length: usize) -> Result<usize> {
    let offset = usize::try_from(offset).unwrap_or(usize::MAX);
    if offset > length {
        return Err(throw_dom(
            ctx,
            "IndexSizeError",
            "offset is outside the data",
        ));
    }
    Ok(offset)
}

fn required_node<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<NodeId> {
    host_node_id(ctx, value).ok_or_else(|| Exception::throw_type(ctx, "argument is not a Node"))
}

fn optional_node<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<Option<NodeId>> {
    if value.is_null() || value.is_undefined() {
        Ok(None)
    } else {
        Ok(Some(required_node(ctx, value)?))
    }
}

fn child_value<'js>(ctx: &Ctx<'js>, id: Option<NodeId>) -> Result<Value<'js>> {
    match id {
        Some(id) => wrap_node(ctx, id),
        None => Ok(Value::new_null(ctx.clone())),
    }
}

fn sibling_value<'js>(ctx: &Ctx<'js>, id: NodeId, forward: bool) -> Result<Value<'js>> {
    let world = world(ctx)?;
    let sibling = world
        .borrow()
        .document(id)
        .and_then(|parsed| parsed.dom.sibling(id, forward));
    child_value(ctx, sibling)
}

/// The nearest element sibling in the given direction
/// (<https://dom.spec.whatwg.org/#dom-nondocumenttypechildnode-nextelementsibling>).
fn element_sibling_value<'js>(ctx: &Ctx<'js>, id: NodeId, forward: bool) -> Result<Value<'js>> {
    let world = world(ctx)?;
    let found = {
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(id) else {
            return Ok(Value::new_null(ctx.clone()));
        };
        let mut cursor = parsed.dom.sibling(id, forward);
        while let Some(sibling) = cursor {
            if is_element(&parsed.dom, sibling) {
                break;
            }
            cursor = parsed.dom.sibling(sibling, forward);
        }
        cursor
    };
    child_value(ctx, found)
}

fn string_value<'js>(ctx: &Ctx<'js>, text: &str) -> Result<Value<'js>> {
    Ok(rquickjs::String::from_str(ctx.clone(), text)?.into_value())
}

/// [Descendant text content](https://dom.spec.whatwg.org/#concept-descendant-text-content):
/// the data of all `Text` descendants in tree order.
fn descendant_text(dom: &dom::Dom, id: NodeId) -> String {
    let mut text = String::new();
    let mut stack: Vec<NodeId> = dom
        .children(id)
        .map(|kids| kids.copied().collect())
        .unwrap_or_default();
    stack.reverse();
    while let Some(current) = stack.pop() {
        match dom.get(current).map(|node| node.kind()) {
            Some(NodeKind::Text { data } | NodeKind::CDataSection { data }) => text.push_str(data),
            Some(NodeKind::Element { .. } | NodeKind::Fragment) => {
                if let Some(kids) = dom.children(current) {
                    let mut kids: Vec<NodeId> = kids.copied().collect();
                    kids.reverse();
                    stack.extend(kids);
                }
            }
            _ => {}
        }
    }
    text
}

/// Structural `isEqualNode`
/// (<https://dom.spec.whatwg.org/#concept-node-equals>).
fn nodes_equal(dom: &dom::Dom, a: NodeId, b: NodeId) -> bool {
    if a == b {
        return true;
    }
    let (Some(first), Some(second)) = (dom.get(a), dom.get(b)) else {
        return false;
    };
    let equal = match (first.kind(), second.kind()) {
        (NodeKind::Document, NodeKind::Document) | (NodeKind::Fragment, NodeKind::Fragment) => true,
        (
            NodeKind::Doctype {
                name: name_a,
                public_id: public_a,
                system_id: system_a,
            },
            NodeKind::Doctype {
                name: name_b,
                public_id: public_b,
                system_id: system_b,
            },
        ) => name_a == name_b && public_a == public_b && system_a == system_b,
        (
            NodeKind::Element {
                name: name_a,
                attributes: attributes_a,
            },
            NodeKind::Element {
                name: name_b,
                attributes: attributes_b,
            },
        ) => {
            name_a == name_b
                && attributes_a.len() == attributes_b.len()
                && attributes_a
                    .iter()
                    .all(|attribute| attributes_b.iter().any(|candidate| candidate == attribute))
        }
        (NodeKind::Text { data: data_a }, NodeKind::Text { data: data_b })
        | (NodeKind::CDataSection { data: data_a }, NodeKind::CDataSection { data: data_b })
        | (NodeKind::Comment { data: data_a }, NodeKind::Comment { data: data_b }) => {
            data_a == data_b
        }
        (
            NodeKind::ProcessingInstruction {
                target: target_a,
                data: data_a,
            },
            NodeKind::ProcessingInstruction {
                target: target_b,
                data: data_b,
            },
        ) => target_a == target_b && data_a == data_b,
        _ => false,
    };
    if !equal {
        return false;
    }
    let kids_a: Vec<NodeId> = dom
        .children(a)
        .map(|kids| kids.copied().collect())
        .unwrap_or_default();
    let kids_b: Vec<NodeId> = dom
        .children(b)
        .map(|kids| kids.copied().collect())
        .unwrap_or_default();
    kids_a.len() == kids_b.len()
        && kids_a
            .iter()
            .zip(&kids_b)
            .all(|(&first, &second)| nodes_equal(dom, first, second))
}

/// [Locate a namespace](https://dom.spec.whatwg.org/#locate-a-namespace) for
/// `prefix` walking `cursor`'s inclusive ancestors.
fn locate_namespace(dom: &dom::Dom, cursor: NodeId, prefix: Option<&str>) -> Option<Namespace> {
    let mut cursor = Some(cursor);
    while let Some(id) = cursor {
        if let Some(NodeKind::Element { name, .. }) = dom.get(id).map(|node| node.kind()) {
            let actual = name
                .prefix
                .as_ref()
                .map(Prefix::as_ref)
                .filter(|prefix| !prefix.is_empty());
            if !name.ns.is_empty() && actual == prefix {
                return Some(name.ns.clone());
            }
        }
        cursor = dom.parent(id);
    }
    None
}

/// [Locate a namespace prefix](https://dom.spec.whatwg.org/#locate-a-namespace-prefix)
/// for `namespace` walking `cursor`'s inclusive ancestors.
fn locate_prefix(dom: &dom::Dom, cursor: NodeId, namespace: &str) -> Option<String> {
    let mut cursor = Some(cursor);
    while let Some(id) = cursor {
        if let Some(NodeKind::Element { name, attributes }) = dom.get(id).map(|node| node.kind()) {
            if name.ns.as_ref() == namespace
                && let Some(prefix) = name.prefix.as_ref().filter(|prefix| !prefix.is_empty())
            {
                return Some(prefix.to_string());
            }
            for attribute in attributes {
                if attribute.name.ns.as_ref() == namespace
                    && let Some(prefix) = attribute
                        .name
                        .prefix
                        .as_ref()
                        .filter(|prefix| !prefix.is_empty())
                {
                    return Some(prefix.to_string());
                }
            }
        }
        cursor = dom.parent(id);
    }
    None
}

fn create_html_element<'js>(ctx: &Ctx<'js>, document: NodeId, tag: &str) -> Result<Value<'js>> {
    // https://dom.spec.whatwg.org/#dom-document-createelement: validate, then
    // lowercase for an HTML document.
    if !valid_element_local_name(tag) {
        return Err(throw_dom(
            ctx,
            "InvalidCharacterError",
            "tag name is not a valid element local name",
        ));
    }
    // XHTML documents use the HTML namespace but keep case; only text/html
    // is an "HTML document" for lowercasing
    // (<https://dom.spec.whatwg.org/#internal-createelementns-steps>).
    let namespace = if document_is_html(ctx, document) {
        html_namespace()
    } else {
        Namespace::from("")
    };
    let local = if document_is_html_content(ctx, document) {
        tag.to_ascii_lowercase()
    } else {
        tag.to_owned()
    };
    let name = QualName::new(None, namespace, LocalName::from(local));
    create_element_named(ctx, document, name)
}

fn create_element_named<'js>(
    ctx: &Ctx<'js>,
    document: NodeId,
    name: QualName,
) -> Result<Value<'js>> {
    create_kind(ctx, document, |dom| dom.create_element(name, Vec::new()))
}

fn create_kind<'js>(
    ctx: &Ctx<'js>,
    document: NodeId,
    make: impl FnOnce(&mut dom::Dom) -> NodeId,
) -> Result<Value<'js>> {
    let world = world(ctx)?;
    let mut world = world.borrow_mut();
    let Some(parsed) = world.document_mut(document) else {
        return Err(Exception::throw_type(ctx, "no document"));
    };
    let id = make(&mut parsed.dom);
    drop(world);
    wrap_node(ctx, id)
}

/// Which context a qualified name is validated in
/// (<https://dom.spec.whatwg.org/#validate-and-extract> steps 6 and 7).
#[derive(Clone, Copy)]
enum NodeContext {
    Attribute,
    Element,
}

/// [Validate and extract](https://dom.spec.whatwg.org/#validate-and-extract)
/// a namespace and qualified name.
fn validate_and_extract(
    ctx: &Ctx<'_>,
    namespace: Option<&str>,
    qualified: &str,
    context: NodeContext,
) -> Result<QualName> {
    let namespace = namespace.filter(|namespace| !namespace.is_empty());
    let (prefix, local) = match qualified.split_once(':') {
        Some((prefix, local)) => {
            if !valid_namespace_prefix(prefix) {
                return Err(throw_dom(
                    ctx,
                    "InvalidCharacterError",
                    "invalid namespace prefix",
                ));
            }
            (Some(prefix), local)
        }
        None => (None, qualified),
    };
    let local_valid = match context {
        NodeContext::Attribute => valid_attribute_local_name(local),
        NodeContext::Element => valid_element_local_name(local),
    };
    if !local_valid {
        return Err(throw_dom(
            ctx,
            "InvalidCharacterError",
            "invalid local name",
        ));
    }
    let xml = "http://www.w3.org/XML/1998/namespace";
    let xmlns = "http://www.w3.org/2000/xmlns/";
    if prefix.is_some() && namespace.is_none() {
        return Err(throw_dom(ctx, "NamespaceError", "prefix without namespace"));
    }
    if prefix == Some("xml") && namespace != Some(xml) {
        return Err(throw_dom(
            ctx,
            "NamespaceError",
            "xml prefix with wrong namespace",
        ));
    }
    if (qualified == "xmlns" || prefix == Some("xmlns")) && namespace != Some(xmlns) {
        return Err(throw_dom(
            ctx,
            "NamespaceError",
            "xmlns name with wrong namespace",
        ));
    }
    if namespace == Some(xmlns) && qualified != "xmlns" && prefix != Some("xmlns") {
        return Err(throw_dom(
            ctx,
            "NamespaceError",
            "xmlns namespace without xmlns name",
        ));
    }
    Ok(QualName::new(
        prefix.map(Prefix::from),
        Namespace::from(namespace.unwrap_or("")),
        LocalName::from(local),
    ))
}

/// [Valid namespace prefix](https://dom.spec.whatwg.org/#valid-namespace-prefix).
fn valid_namespace_prefix(prefix: &str) -> bool {
    !prefix.is_empty()
        && !prefix
            .chars()
            .any(|c| is_infra_whitespace(c) || matches!(c, '\0' | '/' | '>'))
}

/// The XML `Name` production (colons allowed), used by
/// `createProcessingInstruction` targets
/// (<https://www.w3.org/TR/xml/#NT-Name>).
fn valid_xml_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if is_name_start(first) => chars.all(is_name_char),
        _ => false,
    }
}

// https://www.w3.org/TR/xml/#NT-NameStartChar
fn is_name_start(c: char) -> bool {
    matches!(c, ':' | 'A'..='Z' | '_' | 'a'..='z' | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{2FF}' | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}' | '\u{200C}'..='\u{200D}' | '\u{2070}'..='\u{218F}' | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}' | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}')
        || ('\u{10000}'..='\u{EFFFF}').contains(&c)
}

// https://www.w3.org/TR/xml/#NT-NameChar
fn is_name_char(c: char) -> bool {
    is_name_start(c)
        || matches!(c, '-' | '.' | '0'..='9' | '\u{B7}' | '\u{0300}'..='\u{036F}' | '\u{203F}'..='\u{2040}')
}

/// [Valid attribute local name](https://dom.spec.whatwg.org/#valid-attribute-local-name).
fn valid_attribute_local_name(local: &str) -> bool {
    !local.is_empty()
        && !local
            .chars()
            .any(|c| is_infra_whitespace(c) || matches!(c, '\0' | '/' | '=' | '>'))
}

/// [Valid element local name](https://dom.spec.whatwg.org/#valid-element-local-name).
fn valid_element_local_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if first.is_ascii_alphabetic() {
        return !name
            .chars()
            .any(|c| is_infra_whitespace(c) || matches!(c, '\0' | '/' | '>'));
    }
    if first != ':' && first != '_' && u32::from(first) < 0x80 {
        return false;
    }
    name.chars().all(|c| {
        c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | ':' | '_') || u32::from(c) >= 0x80
    })
}

fn is_infra_whitespace(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\u{c}' | '\r' | ' ')
}

fn element_node_name(name: &QualName, uppercase: bool) -> String {
    let qualified = qualified_name(name);
    if uppercase && name.ns == html_namespace() {
        qualified.to_ascii_uppercase()
    } else {
        qualified
    }
}

fn qualified_name(name: &QualName) -> String {
    match &name.prefix {
        Some(prefix) if !prefix.is_empty() => format!("{prefix}:{}", name.local),
        _ => name.local.to_string(),
    }
}

fn elements_by_tag<'js>(ctx: &Ctx<'js>, scope: NodeId, name: &str) -> Result<Value<'js>> {
    live_collection(
        ctx,
        scope,
        CollectionKind::ElementsByTag(name.to_owned()),
        Some("HTMLCollection"),
    )
}

fn live_collection<'js>(
    ctx: &Ctx<'js>,
    scope: NodeId,
    kind: CollectionKind,
    brand: Option<&str>,
) -> Result<Value<'js>> {
    let class = Class::instance(
        ctx.clone(),
        JsCollection {
            scope: Handle(scope),
            kind,
        },
    )?;
    if let Some(brand) = brand
        && let Some(proto) = class_proto(ctx, brand)?
    {
        class.set_prototype(Some(&proto))?;
    }
    let proxy: Function = ctx.globals().get("__tb_liveCollection")?;
    proxy.call((Class::into_value(class),))
}

fn collection_ids(ctx: &Ctx<'_>, scope: NodeId, kind: &CollectionKind) -> Result<Vec<NodeId>> {
    let world = world(ctx)?;
    let parsed = world.borrow();
    let Some(parsed) = parsed.document(scope) else {
        return Ok(Vec::new());
    };
    Ok(match kind {
        CollectionKind::Children => parsed
            .dom
            .children(scope)
            .map(|children| children.copied().collect())
            .unwrap_or_default(),
        CollectionKind::ElementChildren => parsed
            .dom
            .children(scope)
            .map(|children| {
                children
                    .copied()
                    .filter(|&kid| is_element(&parsed.dom, kid))
                    .collect()
            })
            .unwrap_or_default(),
        CollectionKind::ElementsByTag(name) => collect_by_tag(&parsed.dom, scope, name),
        CollectionKind::ElementsByTagNs { namespace, local } => {
            collect_by_tag_ns(&parsed.dom, scope, namespace, local)
        }
        CollectionKind::ElementsByClass(names) => collect_by_class(&parsed.dom, scope, names),
        CollectionKind::ElementsByName(name) => collect_by_name(&parsed.dom, scope, name),
        CollectionKind::Static(handles) => handles.iter().map(|handle| handle.0).collect(),
    })
}

fn collect_by_tag(dom: &dom::Dom, scope: NodeId, name: &str) -> Vec<NodeId> {
    // In an HTML document, an HTML-namespace element matches the queried
    // name ASCII-lowercased; other elements match the name exactly
    // (<https://dom.spec.whatwg.org/#concept-getelementsbytagname>).
    let lowered = name.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut stack: Vec<NodeId> = dom
        .children(scope)
        .map(|kids| kids.copied().collect())
        .unwrap_or_default();
    stack.reverse();
    while let Some(id) = stack.pop() {
        if let Some(NodeKind::Element { name: qual, .. }) = dom.get(id).map(|node| node.kind()) {
            let matches = if qual.ns == html_namespace() {
                qualified_name(qual) == lowered
            } else {
                qualified_name(qual) == name
            };
            if name == "*" || matches {
                out.push(id);
            }
        }
        if let Some(kids) = dom.children(id) {
            let mut kids: Vec<_> = kids.copied().collect();
            kids.reverse();
            stack.extend(kids);
        }
    }
    out
}

fn collect_by_name(dom: &dom::Dom, scope: NodeId, name: &str) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut stack: Vec<NodeId> = dom
        .children(scope)
        .map(|kids| kids.copied().collect())
        .unwrap_or_default();
    stack.reverse();
    while let Some(id) = stack.pop() {
        if is_element(dom, id) && dom.attribute(id, "name").as_deref() == Some(name) {
            out.push(id);
        }
        if let Some(kids) = dom.children(id) {
            let mut kids: Vec<_> = kids.copied().collect();
            kids.reverse();
            stack.extend(kids);
        }
    }
    out
}

fn collect_by_tag_ns(dom: &dom::Dom, scope: NodeId, namespace: &str, local: &str) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut stack: Vec<NodeId> = dom
        .children(scope)
        .map(|kids| kids.copied().collect())
        .unwrap_or_default();
    stack.reverse();
    while let Some(id) = stack.pop() {
        if let Some(NodeKind::Element { name, .. }) = dom.get(id).map(|node| node.kind())
            && (namespace == "*" || name.ns.as_ref() == namespace)
            && (local == "*" || name.local.as_ref() == local)
        {
            out.push(id);
        }
        if let Some(kids) = dom.children(id) {
            let mut kids: Vec<_> = kids.copied().collect();
            kids.reverse();
            stack.extend(kids);
        }
    }
    out
}

fn collect_by_class(dom: &dom::Dom, scope: NodeId, names: &str) -> Vec<NodeId> {
    let wanted: Vec<&str> = names.split_ascii_whitespace().collect();
    let mut out = Vec::new();
    let mut stack: Vec<NodeId> = dom
        .children(scope)
        .map(|kids| kids.copied().collect())
        .unwrap_or_default();
    stack.reverse();
    while let Some(id) = stack.pop() {
        if let Some(NodeKind::Element { .. }) = dom.get(id).map(|node| node.kind()) {
            let classes = dom.attribute(id, "class").unwrap_or_default();
            let tokens: Vec<&str> = classes.split_ascii_whitespace().collect();
            if wanted.iter().all(|want| tokens.contains(want)) {
                out.push(id);
            }
        }
        if let Some(kids) = dom.children(id) {
            let mut kids: Vec<_> = kids.copied().collect();
            kids.reverse();
            stack.extend(kids);
        }
    }
    out
}

fn is_element(dom: &dom::Dom, id: NodeId) -> bool {
    matches!(
        dom.get(id).map(|node| node.kind()),
        Some(NodeKind::Element { .. })
    )
}

/// The root of the tree `id` participates in (itself when detached).
fn root_of(dom: &dom::Dom, id: NodeId) -> NodeId {
    let mut root = id;
    while let Some(parent) = dom.parent(root) {
        root = parent;
    }
    root
}

/// `id` followed by its inclusive ancestors, nearest first.
fn ancestor_chain(dom: &dom::Dom, id: NodeId) -> Vec<NodeId> {
    let mut chain = vec![id];
    let mut cursor = id;
    while let Some(parent) = dom.parent(cursor) {
        chain.push(parent);
        cursor = parent;
    }
    chain
}

/// Document order of two nodes in one tree
/// (<https://dom.spec.whatwg.org/#concept-tree-order>).
fn tree_order(dom: &dom::Dom, a: NodeId, b: NodeId) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let chain_a = ancestor_chain(dom, a);
    let chain_b = ancestor_chain(dom, b);
    let mut common = 0;
    while common < chain_a.len()
        && common < chain_b.len()
        && chain_a[chain_a.len() - 1 - common] == chain_b[chain_b.len() - 1 - common]
    {
        common += 1;
    }
    let Some(parent) = chain_a
        .len()
        .checked_sub(common)
        .and_then(|index| chain_a.get(index))
    else {
        return Ordering::Less;
    };
    let child_a = chain_a[chain_a.len() - 1 - common];
    let child_b = chain_b[chain_b.len() - 1 - common];
    let kids: Vec<NodeId> = dom
        .children(*parent)
        .map(|kids| kids.copied().collect())
        .unwrap_or_default();
    let position_a = kids.iter().position(|&kid| kid == child_a);
    let position_b = kids.iter().position(|&kid| kid == child_b);
    position_a.cmp(&position_b)
}

fn find_element_by_id(dom: &dom::Dom, scope: NodeId, id: &str) -> Option<NodeId> {
    let mut stack: Vec<NodeId> = dom
        .children(scope)
        .map(|kids| kids.copied().collect())
        .unwrap_or_default();
    stack.reverse();
    while let Some(node) = stack.pop() {
        if matches!(
            dom.get(node).map(|item| item.kind()),
            Some(NodeKind::Element { .. })
        ) && dom.attribute(node, "id").as_deref() == Some(id)
        {
            return Some(node);
        }
        if let Some(kids) = dom.children(node) {
            let mut kids: Vec<_> = kids.copied().collect();
            kids.reverse();
            stack.extend(kids);
        }
    }
    None
}
