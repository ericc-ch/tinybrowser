//! Host objects for DOM nodes, one interface per class.

use std::cell::RefCell;
use std::rc::Rc;

use dom::{
    LocalName, Namespace, NodeId, NodeKind, Prefix, QualName, html_namespace, xml_namespace,
};
use rquickjs::{
    Array, Class, Ctx, Exception, FromJs, Function, Object, Persistent, Result, Value,
    class::{Trace, Tracer},
    function::Constructor,
    prelude::This,
};

use super::world::{EventTargetKey, Listener, SharedWorld, World};

#[derive(Clone, Copy)]
struct Handle(NodeId);

#[allow(unsafe_code)]
// SAFETY: `Handle` is three integers; it contains no JS values to retag.
unsafe impl rquickjs::JsLifetime<'_> for Handle {
    type Changed<'to> = Handle;
}

impl<'js> Trace<'js> for Handle {
    fn trace<'a>(&self, _tracer: Tracer<'a, 'js>) {}
}

macro_rules! branded_node {
    ($name:ident, $js:literal) => {
        #[derive(Trace)]
        #[rquickjs::class(rename = $js)]
        pub(crate) struct $name {
            handle: Handle,
        }

        #[allow(unsafe_code)]
        // SAFETY: holds only a `Handle` of integers.
        unsafe impl rquickjs::JsLifetime<'_> for $name {
            type Changed<'to> = $name;
        }
    };
}

#[derive(Trace)]
#[rquickjs::class(rename = "Event")]
pub struct JsEvent {
    typ: String,
}

#[allow(unsafe_code)]
// SAFETY: `JsEvent` holds only a Rust `String`.
unsafe impl rquickjs::JsLifetime<'_> for JsEvent {
    type Changed<'to> = JsEvent;
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

branded_node!(JsNode, "Node");

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
    fn child_nodes<'js>(&self, ctx: Ctx<'js>) -> Result<Array<'js>> {
        let world = world(&ctx)?;
        let ids: Vec<NodeId> = world
            .borrow()
            .parsed
            .as_ref()
            .and_then(|parsed| {
                parsed
                    .dom
                    .children(self.handle.0)
                    .map(|kids| kids.copied().collect())
            })
            .unwrap_or_default();
        let list = Array::new(ctx.clone())?;
        for (index, id) in ids.into_iter().enumerate() {
            list.set(index, wrap_node(&ctx, id)?)?;
        }
        Ok(list)
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
            .append(parent, kid)
            .map_err(|err| Exception::throw_type(&ctx, &err.to_string()))?;
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

    #[qjs(rename = "getElementsByTagName")]
    fn get_elements_by_tag_name<'js>(&self, ctx: Ctx<'js>, name: String) -> Result<Array<'js>> {
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
}

struct OptString(Option<String>);

impl<'js> rquickjs::FromJs<'js> for OptString {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
        if value.is_null() || value.is_undefined() {
            return Ok(Self(None));
        }
        Ok(Self(Some(String::from_js(ctx, value)?)))
    }
}

pub(super) fn install(ctx: &Ctx<'_>, world: &Rc<RefCell<World>>) -> Result<()> {
    ctx.store_userdata(SharedWorld(world.clone()))
        .map_err(|err| Exception::throw_internal(ctx, &err.to_string()))?;
    let globals = ctx.globals();
    Class::<JsEvent>::define(&globals)?;
    Class::<JsNode>::define(&globals)?;
    install_brands(ctx)?;

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
    let node_proto = Class::<JsNode>::prototype(ctx)?
        .ok_or_else(|| Exception::throw_type(ctx, "Node prototype"))?;
    for name in [
        "Document",
        "Element",
        "Text",
        "Comment",
        "DocumentType",
        "DocumentFragment",
    ] {
        let mut options = rquickjs::context::EvalOptions::default();
        options.strict = false;
        let ctor: Function = ctx.eval_with_options(
            "(function() { throw new TypeError('Illegal constructor'); })",
            options,
        )?;
        let proto = Object::new(ctx.clone())?;
        proto.set_prototype(Some(&node_proto))?;
        proto.set("constructor", ctor.clone())?;
        ctor.set("prototype", proto.clone())?;
        world(ctx)?
            .borrow_mut()
            .intern_brand(name, Persistent::save(ctx, proto));
        ctx.globals().set(name, ctor)?;
    }
    Ok(())
}

fn class_proto<'js>(ctx: &Ctx<'js>, name: &str) -> Result<Option<Object<'js>>> {
    let Some(saved) = world(ctx)?.borrow().brand(name) else {
        return Ok(None);
    };
    Ok(Some(saved.restore(ctx)?))
}

fn world(ctx: &Ctx<'_>) -> Result<Rc<RefCell<World>>> {
    ctx.userdata::<SharedWorld>()
        .map(|guard| guard.0.clone())
        .ok_or_else(|| Exception::throw_internal(ctx, "missing page world"))
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
        Some((prefix, local)) => !prefix.is_empty() && !local.is_empty() && !local.contains(':'),
        None => !tag.is_empty(),
    }
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

fn elements_by_tag<'js>(ctx: &Ctx<'js>, scope: NodeId, name: &str) -> Result<Array<'js>> {
    let world = world(ctx)?;
    let ids = {
        let parsed = world.borrow();
        let Some(parsed) = parsed.parsed.as_ref() else {
            return Array::new(ctx.clone());
        };
        collect_by_tag(&parsed.dom, scope, name)
    };
    let list = Array::new(ctx.clone())?;
    for (index, id) in ids.into_iter().enumerate() {
        list.set(index, wrap_node(ctx, id)?)?;
    }
    Ok(list)
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
