//! Platform objects for DOM nodes, one interface per class.

use std::cell::RefCell;
use std::rc::Rc;

use dom::{
    DomError, LocalName, Namespace, NodeId, NodeKind, Prefix, QualName, html_namespace,
    xml_namespace,
};
use rquickjs::{
    Class, Ctx, Exception, FromJs, Function, Object, Persistent, Result, Value,
    class::{Trace, Tracer},
    function::{Constructor, Rest},
    prelude::This,
};

use super::world::{EventTargetKey, Listener, SharedWorld, World};

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
    _reserved: Option<Handle>,
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
        let Some(parsed) = world.parsed.as_mut() else {
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
    fn create_document(&self, ctx: Ctx<'_>, _args: Rest<Value<'_>>) -> Result<()> {
        Err(throw_dom(
            &ctx,
            "NotSupportedError",
            "multiple documents are not supported yet",
        ))
    }

    // https://dom.spec.whatwg.org/#dom-domimplementation-createhtmldocument
    #[qjs(rename = "createHTMLDocument")]
    fn create_html_document(&self, ctx: Ctx<'_>, _args: Rest<Value<'_>>) -> Result<()> {
        Err(throw_dom(
            &ctx,
            "NotSupportedError",
            "multiple documents are not supported yet",
        ))
    }
}

/// [Valid doctype name](https://dom.spec.whatwg.org/#valid-doctype-name): no
/// ASCII whitespace, NULL, or `>`.
fn valid_doctype_name(name: &str) -> bool {
    !name
        .chars()
        .any(|c| matches!(c, '\t' | '\n' | '\u{c}' | '\r' | ' ' | '\0' | '>'))
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
    ElementsByTag(String),
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
        with_node_kind(&ctx, self.handle.0, |kind| match kind {
            Some(NodeKind::Element { name, .. }) => Ok(element_node_name(name)),
            Some(NodeKind::Text { .. }) => Ok("#text".into()),
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
            let Some(parsed) = parsed.parsed.as_ref() else {
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
            .parsed
            .as_ref()
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
        let parent = self.handle.0;
        let world = world(&ctx)?;
        let mut world = world.borrow_mut();
        let Some(parsed) = world.parsed.as_mut() else {
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
        create_html_element(&ctx, &tag)
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
        let namespace_is_null = ns.0.as_ref().is_none_or(String::is_empty);
        let namespace = match ns.0 {
            Some(ns) if !ns.is_empty() => Namespace::from(ns),
            _ => Namespace::from(""),
        };
        if !valid_qualified_name(&tag) {
            return Err(Exception::throw_type(&ctx, "InvalidCharacterError"));
        }
        let (prefix, local) = split_qualified_name(&tag);
        // https://dom.spec.whatwg.org/#validate-and-extract
        if prefix.is_some() && namespace_is_null {
            return Err(Exception::throw_type(&ctx, "NamespaceError"));
        }
        if prefix == Some("xml") && namespace != xml_namespace() {
            return Err(Exception::throw_type(&ctx, "NamespaceError"));
        }
        let xmlns = Namespace::from("http://www.w3.org/2000/xmlns/");
        if (prefix == Some("xmlns") || tag == "xmlns") && namespace != xmlns {
            return Err(Exception::throw_type(&ctx, "NamespaceError"));
        }
        if namespace == xmlns && prefix != Some("xmlns") && tag != "xmlns" {
            return Err(Exception::throw_type(&ctx, "NamespaceError"));
        }
        let name = QualName::new(prefix.map(Prefix::from), namespace, LocalName::from(local));
        create_element_named(&ctx, name)
    }

    #[qjs(rename = "createTextNode")]
    fn create_text_node<'js>(&self, ctx: Ctx<'js>, data: String) -> Result<Value<'js>> {
        create_kind(&ctx, |dom| dom.create_text(data))
    }

    // https://dom.spec.whatwg.org/#dom-document-createcomment
    #[qjs(rename = "createComment")]
    fn create_comment<'js>(&self, ctx: Ctx<'js>, data: String) -> Result<Value<'js>> {
        create_kind(&ctx, |dom| dom.create_comment(data))
    }

    // https://dom.spec.whatwg.org/#dom-document-createdocumentfragment
    #[qjs(rename = "createDocumentFragment")]
    fn create_document_fragment<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        create_kind(&ctx, dom::Dom::create_fragment)
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
            let Some(parsed) = parsed.parsed.as_ref() else {
                return Ok(Value::new_null(ctx));
            };
            find_element_by_id(&parsed.dom, parsed.dom.document(), &id)
        };
        match found {
            Some(node) => wrap_node(&ctx, node),
            None => Ok(Value::new_null(ctx)),
        }
    }

    // https://dom.spec.whatwg.org/#dom-document-implementation
    #[qjs(get)]
    fn implementation<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        Ok(Class::instance(ctx.clone(), JsImplementation { _reserved: None })?.into_value())
    }

    #[qjs(rename = "getElementsByTagName")]
    fn get_elements_by_tag_name<'js>(&self, ctx: Ctx<'js>, name: String) -> Result<Value<'js>> {
        elements_by_tag(&ctx, self.handle.0, &name)
    }

    #[qjs(get)]
    fn body<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let found = world.borrow().parsed.as_ref().and_then(|parsed| {
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
            let Some(parsed) = parsed.parsed.as_ref() else {
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
            let Some(parsed) = parsed.parsed.as_ref() else {
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

    #[qjs(get, rename = "readyState")]
    fn ready_state(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let is_document = world
            .parsed
            .as_ref()
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

    #[qjs(rename = "getAttribute")]
    fn get_attribute(&self, ctx: Ctx<'_>, name: String) -> Result<Option<String>> {
        let world = world(&ctx)?;
        Ok(world
            .borrow()
            .parsed
            .as_ref()
            .and_then(|parsed| parsed.dom.attribute(self.handle.0, &name)))
    }

    #[qjs(rename = "setAttribute")]
    fn set_attribute(&self, ctx: Ctx<'_>, name: String, value: String) -> Result<()> {
        let world = world(&ctx)?;
        let mut world = world.borrow_mut();
        let Some(parsed) = world.parsed.as_mut() else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        parsed
            .dom
            .set_attribute(self.handle.0, &name, value)
            .map_err(|err| Exception::throw_type(&ctx, &err.to_string()))
    }

    #[qjs(get)]
    fn id(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        Ok(world
            .borrow()
            .parsed
            .as_ref()
            .and_then(|parsed| parsed.dom.attribute(self.handle.0, "id"))
            .unwrap_or_default())
    }

    #[qjs(set, rename = "id")]
    fn set_id(&self, ctx: Ctx<'_>, value: String) -> Result<()> {
        self.set_attribute(ctx, "id".into(), value)
    }

    #[qjs(get)]
    fn src(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        Ok(world
            .borrow()
            .parsed
            .as_ref()
            .and_then(|parsed| parsed.dom.attribute(self.handle.0, "src"))
            .unwrap_or_default())
    }

    #[qjs(get)]
    fn name(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        Ok(world
            .borrow()
            .parsed
            .as_ref()
            .and_then(|parsed| parsed.dom.attribute(self.handle.0, "name"))
            .unwrap_or_default())
    }

    #[qjs(get)]
    fn content(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        Ok(world
            .borrow()
            .parsed
            .as_ref()
            .and_then(|parsed| parsed.dom.attribute(self.handle.0, "content"))
            .unwrap_or_default())
    }

    #[qjs(get)]
    fn data(&self, ctx: Ctx<'_>) -> Result<String> {
        character_data(&ctx, self.handle.0)
    }

    #[qjs(set, rename = "data")]
    fn set_data(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        set_character_data(&ctx, self.handle.0, value.0)
    }

    // https://dom.spec.whatwg.org/#dom-node-lastchild
    #[qjs(get, rename = "lastChild")]
    fn last_child<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let id = world
            .borrow()
            .parsed
            .as_ref()
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
        let id = world.borrow().parsed.as_ref().and_then(|parsed| {
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
        let Some(parsed) = parsed.parsed.as_ref() else {
            return Ok(false);
        };
        Ok(parsed
            .dom
            .children(self.handle.0)
            .is_some_and(|kids| kids.len() > 0))
    }

    // https://dom.spec.whatwg.org/#dom-node-nodevalue
    #[qjs(get, rename = "nodeValue")]
    fn node_value<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.parsed.as_ref() else {
            return Ok(Value::new_null(ctx));
        };
        match parsed.dom.get(self.handle.0).map(|node| node.kind()) {
            Some(NodeKind::Text { data } | NodeKind::Comment { data }) => string_value(&ctx, data),
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
        let Some(parsed) = parsed.parsed.as_ref() else {
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
        let Some(parsed) = world.parsed.as_mut() else {
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
        let Some(parsed) = parsed.parsed.as_ref() else {
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
        let Some(parsed) = parsed.parsed.as_ref() else {
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
            let Some(parsed) = parsed.parsed.as_ref() else {
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
        let Some(parsed) = parsed.parsed.as_ref() else {
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
    fn clone_node<'js>(&self, ctx: Ctx<'js>, deep: Option<bool>) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let mut world = world.borrow_mut();
        let Some(parsed) = world.parsed.as_mut() else {
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
            .clone_node(self.handle.0, deep.unwrap_or(false))
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
        let world = world(&ctx)?;
        let mut world = world.borrow_mut();
        let Some(parsed) = world.parsed.as_mut() else {
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
        let Some(parsed) = world.parsed.as_mut() else {
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
        let world = world(&ctx)?;
        let mut world = world.borrow_mut();
        let Some(parsed) = world.parsed.as_mut() else {
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
        let Some(parsed) = parsed.parsed.as_ref() else {
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
        let Some(parsed) = parsed.parsed.as_ref() else {
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
        let Some(parsed) = parsed.parsed.as_ref() else {
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
        let Some(parsed) = world.parsed.as_mut() else {
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
        Some(NodeKind::Document) => Some("Document"),
        Some(NodeKind::Element { .. }) => Some("Element"),
        Some(NodeKind::Text { .. }) => Some("Text"),
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

fn install_brands(ctx: &Ctx<'_>) -> Result<()> {
    // https://webidl.spec.whatwg.org/#interface-prototype-object
    // https://dom.spec.whatwg.org/#interface-node
    ctx.eval::<(), _>(
        r"
(function() {
  const native = globalThis.Node.prototype;
  function illegal() { throw new TypeError('Illegal constructor'); }
  function define(name, parent, members) {
    const ctor = function() { return illegal(); };
    const proto = Object.create(parent ? parent.prototype : Object.prototype);
    for (const member of members) {
      const descriptor = Object.getOwnPropertyDescriptor(native, member);
      if (descriptor) Object.defineProperty(proto, member, descriptor);
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
    'lookupPrefix', 'isDefaultNamespace'
  ]);
  const DocumentInterface = define('Document', NodeInterface, [
    'createElement', 'createElementNS', 'createTextNode', 'createComment',
    'createDocumentFragment', 'write', 'getElementById', 'getElementsByTagName',
    'body', 'documentElement', 'doctype', 'readyState', 'implementation'
  ]);
  const ElementInterface = define('Element', NodeInterface, [
    'getElementsByTagName', 'getAttribute', 'setAttribute', 'id', 'src',
    'name', 'content', 'remove'
  ]);
  const CharacterDataInterface = define('CharacterData', NodeInterface, [
    'data', 'length', 'substringData', 'appendData', 'insertData', 'deleteData',
    'replaceData', 'remove'
  ]);
  define('Text', CharacterDataInterface, []);
  define('Comment', CharacterDataInterface, []);
  define('DocumentType', NodeInterface, ['remove']);
  define('DocumentFragment', NodeInterface, []);
  void DocumentInterface;
  void ElementInterface;
})();
",
    )?;
    for name in [
        "Document",
        "Element",
        "CharacterData",
        "Text",
        "Comment",
        "DocumentType",
        "DocumentFragment",
    ] {
        let ctor: Function = ctx.globals().get(name)?;
        let proto: Object = ctor.get("prototype")?;
        world(ctx)?
            .borrow_mut()
            .intern_brand(name, Persistent::save(ctx, proto));
    }
    Ok(())
}

fn install_collection_brand(ctx: &Ctx<'_>) -> Result<()> {
    ctx.eval::<(), _>(
        r"
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
      return new Proxy(target, {
        get: function(inner, property) {
          if (typeof property === 'string' && /^(0|[1-9][0-9]*)$/.test(property)) {
            return inner.item(Number(property));
          }
          const value = Reflect.get(inner, property, inner);
          return typeof value === 'function' ? value.bind(inner) : value;
        }
      });
    }
  });
})();
",
    )?;
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
    let Some(parsed) = parsed.parsed.as_ref() else {
        return Err(Exception::throw_type(ctx, "no document"));
    };
    Ok(read(parsed.dom.get(id).map(|node| node.kind())))
}

fn character_data(ctx: &Ctx<'_>, id: NodeId) -> Result<String> {
    with_node_kind(ctx, id, |kind| match kind {
        Some(NodeKind::Text { data } | NodeKind::Comment { data }) => data.clone(),
        _ => String::new(),
    })
}

/// [Replaces data](https://dom.spec.whatwg.org/#concept-cd-replace) on a
/// `CharacterData` node; other kinds are a silent no-op (`nodeValue` setter).
fn set_character_data(ctx: &Ctx<'_>, id: NodeId, data: String) -> Result<()> {
    let world = world(ctx)?;
    let mut world = world.borrow_mut();
    let Some(parsed) = world.parsed.as_mut() else {
        return Ok(());
    };
    match parsed.dom.get(id).map(|node| node.kind()) {
        Some(NodeKind::Text { .. }) => parsed
            .dom
            .set_text(id, data)
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
        .parsed
        .as_ref()
        .and_then(|parsed| parsed.dom.sibling(id, forward));
    child_value(ctx, sibling)
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
            Some(NodeKind::Text { data }) => text.push_str(data),
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
        | (NodeKind::Comment { data: data_a }, NodeKind::Comment { data: data_b }) => {
            data_a == data_b
        }
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

fn create_html_element<'js>(ctx: &Ctx<'js>, tag: &str) -> Result<Value<'js>> {
    let name = QualName::new(
        None,
        html_namespace(),
        LocalName::from(tag.to_ascii_lowercase()),
    );
    create_element_named(ctx, name)
}

fn create_element_named<'js>(ctx: &Ctx<'js>, name: QualName) -> Result<Value<'js>> {
    create_kind(ctx, |dom| dom.create_element(name, Vec::new()))
}

fn create_kind<'js>(
    ctx: &Ctx<'js>,
    make: impl FnOnce(&mut dom::Dom) -> NodeId,
) -> Result<Value<'js>> {
    let world = world(ctx)?;
    let mut world = world.borrow_mut();
    let Some(parsed) = world.parsed.as_mut() else {
        return Err(Exception::throw_type(ctx, "no document"));
    };
    let id = make(&mut parsed.dom);
    drop(world);
    wrap_node(ctx, id)
}

fn valid_qualified_name(tag: &str) -> bool {
    match tag.split_once(':') {
        Some((prefix, local)) => valid_ncname(prefix) && valid_ncname(local),
        None => valid_ncname(tag),
    }
}

// https://www.w3.org/TR/xml-names/#NT-NCName
fn valid_ncname(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if is_name_start(first) => chars.all(is_name_char),
        _ => false,
    }
}

// https://www.w3.org/TR/xml/#NT-NameStartChar minus ":"
fn is_name_start(c: char) -> bool {
    matches!(c, 'A'..='Z' | '_' | 'a'..='z' | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{2FF}' | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}' | '\u{200C}'..='\u{200D}' | '\u{2070}'..='\u{218F}' | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}' | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}')
        || ('\u{10000}'..='\u{EFFFF}').contains(&c)
}

// https://www.w3.org/TR/xml/#NT-NameChar minus ":"
fn is_name_char(c: char) -> bool {
    is_name_start(c)
        || matches!(c, '-' | '.' | '0'..='9' | '\u{B7}' | '\u{0300}'..='\u{036F}' | '\u{203F}'..='\u{2040}')
}

fn split_qualified_name(tag: &str) -> (Option<&str>, &str) {
    match tag.split_once(':') {
        Some((prefix, local)) if !prefix.is_empty() && !local.is_empty() => (Some(prefix), local),
        _ => (None, tag),
    }
}

fn element_node_name(name: &QualName) -> String {
    let qualified = match &name.prefix {
        Some(prefix) if !prefix.is_empty() => format!("{prefix}:{}", name.local),
        _ => name.local.to_string(),
    };
    if name.ns == html_namespace() {
        qualified.to_ascii_uppercase()
    } else {
        qualified
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
    let Some(parsed) = parsed.parsed.as_ref() else {
        return Ok(Vec::new());
    };
    Ok(match kind {
        CollectionKind::Children => parsed
            .dom
            .children(scope)
            .map(|children| children.copied().collect())
            .unwrap_or_default(),
        CollectionKind::ElementsByTag(name) => collect_by_tag(&parsed.dom, scope, name),
    })
}

fn collect_by_tag(dom: &dom::Dom, scope: NodeId, name: &str) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut stack: Vec<NodeId> = dom
        .children(scope)
        .map(|kids| kids.copied().collect())
        .unwrap_or_default();
    stack.reverse();
    while let Some(id) = stack.pop() {
        if let Some(NodeKind::Element { name: qual, .. }) = dom.get(id).map(|node| node.kind())
            && (name == "*" || qual.local.as_ref().eq_ignore_ascii_case(name))
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
