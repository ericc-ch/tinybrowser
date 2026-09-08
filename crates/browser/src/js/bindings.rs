//! Host objects for `Node`, `Document`, `Element`, `Event`, and `Window`.
//!
//! `JsLifetime` is an unsafe rquickjs trait. The types here store no JS
//! pointers, so `Changed<'to> = Self` is sound.

#![allow(unsafe_code)]
#![allow(clippy::needless_pass_by_value, clippy::unused_self)]

use std::cell::RefCell;
use std::rc::Rc;

use dom::{LocalName, Namespace, NodeId, NodeKind, QualName, html_namespace};
use rquickjs::{
    Array, Class, Ctx, Exception, Function, Object, Persistent, Result, Value,
    class::{Trace, Tracer},
};

use super::world::{EventTargetKey, Listener, SharedWorld, World};

#[derive(Clone, Copy)]
struct Handle(NodeId);

// SAFETY: `Handle` is three integers; it contains no JS values to retag.
unsafe impl rquickjs::JsLifetime<'_> for Handle {
    type Changed<'to> = Handle;
}

impl<'js> Trace<'js> for Handle {
    fn trace<'a>(&self, _tracer: Tracer<'a, 'js>) {}
}

#[derive(Trace)]
#[rquickjs::class(rename = "Event")]
pub struct JsEvent {
    typ: String,
}

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

#[derive(Trace)]
#[rquickjs::class(rename = "Node")]
pub(crate) struct JsNode {
    handle: Handle,
}

impl JsNode {
    pub(crate) fn node_id(&self) -> NodeId {
        self.handle.0
    }
}

// SAFETY: `JsNode` holds only a `Handle` of integers.
unsafe impl rquickjs::JsLifetime<'_> for JsNode {
    type Changed<'to> = JsNode;
}

#[rquickjs::methods]
impl JsNode {
    #[qjs(get, rename = "nodeType")]
    fn node_type(&self, ctx: Ctx<'_>) -> Result<i32> {
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.parsed.as_ref() else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        match parsed.dom.get(self.handle.0).map(|node| node.kind()) {
            Some(NodeKind::Element { .. }) => Ok(1),
            Some(NodeKind::Text { .. }) => Ok(3),
            Some(NodeKind::Comment { .. }) => Ok(8),
            Some(NodeKind::Document) => Ok(9),
            Some(NodeKind::Doctype { .. }) => Ok(10),
            Some(NodeKind::Fragment) => Ok(11),
            None => Err(Exception::throw_type(&ctx, "stale node")),
        }
    }

    #[qjs(get, rename = "nodeName")]
    fn node_name(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.parsed.as_ref() else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        match parsed.dom.get(self.handle.0).map(|node| node.kind()) {
            Some(NodeKind::Element { name, .. }) => Ok(name.local.as_ref().to_ascii_uppercase()),
            Some(NodeKind::Text { .. }) => Ok("#text".into()),
            Some(NodeKind::Comment { .. }) => Ok("#comment".into()),
            Some(NodeKind::Document) => Ok("#document".into()),
            Some(NodeKind::Doctype { name, .. }) => Ok(name.clone()),
            Some(NodeKind::Fragment) => Ok("#document-fragment".into()),
            None => Err(Exception::throw_type(&ctx, "stale node")),
        }
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
            Some(child) => wrap_node(&ctx, child).map(rquickjs::Class::into_value),
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
            Some(parent) => wrap_node(&ctx, parent).map(rquickjs::Class::into_value),
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
    fn append_child<'js>(
        &self,
        ctx: Ctx<'js>,
        child: Class<'js, JsNode>,
    ) -> Result<Class<'js, JsNode>> {
        let parent = self.handle.0;
        let kid = child.borrow().handle.0;
        let world = world(&ctx)?;
        let mut world = world.borrow_mut();
        let Some(parsed) = world.parsed.as_mut() else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        parsed
            .dom
            .append(parent, kid)
            .map_err(|err| Exception::throw_type(&ctx, &err.to_string()))?;
        Ok(child)
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
        let world = world(&ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.parsed.as_ref() else {
            return Ok(String::new());
        };
        match parsed.dom.get(self.handle.0).map(|node| node.kind()) {
            Some(NodeKind::Text { data } | NodeKind::Comment { data }) => Ok(data.clone()),
            _ => Ok(String::new()),
        }
    }

    #[qjs(rename = "getElementsByTagName")]
    fn get_elements_by_tag_name<'js>(&self, ctx: Ctx<'js>, name: String) -> Result<Array<'js>> {
        let world = world(&ctx)?;
        let ids = {
            let parsed = world.borrow();
            let Some(parsed) = parsed.parsed.as_ref() else {
                return Array::new(ctx);
            };
            collect_by_tag(&parsed.dom, self.handle.0, &name)
        };
        let list = Array::new(ctx.clone())?;
        for (index, id) in ids.into_iter().enumerate() {
            list.set(index, wrap_node(&ctx, id)?)?;
        }
        Ok(list)
    }

    #[qjs(rename = "getElementById")]
    fn get_element_by_id<'js>(&self, ctx: Ctx<'js>, id: String) -> Result<Value<'js>> {
        let world = world(&ctx)?;
        let found = {
            let parsed = world.borrow();
            let Some(parsed) = parsed.parsed.as_ref() else {
                return Ok(Value::new_null(ctx));
            };
            parsed
                .dom
                .select_first(self.handle.0, &format!("#{id}"))
                .ok()
                .flatten()
        };
        match found {
            Some(node) => wrap_node(&ctx, node).map(rquickjs::Class::into_value),
            None => Ok(Value::new_null(ctx)),
        }
    }

    #[qjs(rename = "createElement")]
    fn create_element<'js>(&self, ctx: Ctx<'js>, tag: String) -> Result<Class<'js, JsNode>> {
        let world = world(&ctx)?;
        let mut world = world.borrow_mut();
        let Some(parsed) = world.parsed.as_mut() else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        let name = QualName::new(
            None,
            html_namespace(),
            LocalName::from(tag.to_ascii_lowercase()),
        );
        let id = parsed.dom.create_element(name, Vec::new());
        drop(world);
        wrap_node(&ctx, id)
    }

    #[qjs(rename = "createElementNS")]
    fn create_element_ns<'js>(
        &self,
        ctx: Ctx<'js>,
        ns: OptString,
        tag: String,
    ) -> Result<Class<'js, JsNode>> {
        let world = world(&ctx)?;
        let mut world = world.borrow_mut();
        let Some(parsed) = world.parsed.as_mut() else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        let namespace = match ns.0 {
            Some(ns) if !ns.is_empty() => Namespace::from(ns),
            _ => Namespace::from(""),
        };
        let local = tag.rsplit(':').next().unwrap_or(&tag);
        let name = QualName::new(None, namespace, LocalName::from(local));
        let id = parsed.dom.create_element(name, Vec::new());
        drop(world);
        wrap_node(&ctx, id)
    }

    #[qjs(rename = "createTextNode")]
    fn create_text_node<'js>(&self, ctx: Ctx<'js>, data: String) -> Result<Class<'js, JsNode>> {
        let world = world(&ctx)?;
        let mut world = world.borrow_mut();
        let Some(parsed) = world.parsed.as_mut() else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        let id = parsed.dom.create_text(data);
        drop(world);
        wrap_node(&ctx, id)
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
            Some(id) => wrap_node(&ctx, id).map(rquickjs::Class::into_value),
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
            Some(id) => wrap_node(&ctx, id).map(rquickjs::Class::into_value),
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
        fire(&ctx, EventTargetKey::Node(self.handle.0), &typ, event)
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

    globals.set(
        "addEventListener",
        rquickjs::prelude::Func::from(window_add_event_listener),
    )?;
    Ok(())
}

fn window_add_event_listener<'js>(
    ctx: Ctx<'js>,
    typ: String,
    callback: Function<'js>,
) -> Result<()> {
    add_listener(&ctx, EventTargetKey::Window, typ, callback)
}

pub(super) fn fire_window_load(ctx: &Ctx<'_>) -> Result<()> {
    let event = Class::instance(ctx.clone(), JsEvent { typ: "load".into() })?;
    fire(ctx, EventTargetKey::Window, "load", event)?;
    Ok(())
}

fn wrap_node<'js>(ctx: &Ctx<'js>, id: NodeId) -> Result<Class<'js, JsNode>> {
    Class::instance(ctx.clone(), JsNode { handle: Handle(id) })
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
    event: Class<'js, JsEvent>,
) -> Result<bool> {
    let callbacks = world(ctx)?.borrow().listeners(target, typ);
    for callback in callbacks {
        let func = callback.restore(ctx)?;
        func.call::<_, ()>((event.clone(),))?;
    }
    Ok(true)
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
