//! Shared JS world for the renderer.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use dom::NodeId;
use rquickjs::{Object, Persistent, Value, function::Function};
use url::Url;

use crate::Parsed;
use crate::protocol::BrowserServices;

pub(crate) struct Listener {
    pub typ: String,
    pub callback: Persistent<Function<'static>>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum EventTargetKey {
    Window,
    Node(NodeId),
}

pub(crate) struct World {
    pub parsed: Option<Parsed>,
    pub document_url: Url,
    pub services: Arc<dyn BrowserServices>,
    pub pending_cancels: Vec<i32>,
    pub pending_html_writes: Vec<String>,
    pub parser_active: bool,
    pub document_ready: bool,
    listeners: HashMap<EventTargetKey, Vec<Listener>>,
    wrappers: HashMap<NodeId, Persistent<Value<'static>>>,
    token_lists: HashMap<NodeId, Persistent<Value<'static>>>,
    brands: HashMap<String, Persistent<Object<'static>>>,
}

impl World {
    pub(crate) fn new(services: Arc<dyn BrowserServices>, document_url: Url) -> Self {
        Self {
            parsed: None,
            document_url,
            services,
            pending_cancels: Vec::new(),
            pending_html_writes: Vec::new(),
            parser_active: false,
            document_ready: false,
            listeners: HashMap::new(),
            wrappers: HashMap::new(),
            token_lists: HashMap::new(),
            brands: HashMap::new(),
        }
    }

    pub(crate) fn replace_document(&mut self, parsed: Parsed) {
        self.parsed = Some(parsed);
        self.document_ready = false;
        self.listeners.clear();
        self.wrappers.clear();
        self.token_lists.clear();
    }

    pub(crate) fn add_listener(&mut self, target: EventTargetKey, listener: Listener) {
        self.listeners.entry(target).or_default().push(listener);
    }

    pub(crate) fn clear_listeners(&mut self) {
        self.listeners.clear();
        self.wrappers.clear();
        self.token_lists.clear();
        self.brands.clear();
    }

    pub(crate) fn wrapper(&self, id: NodeId) -> Option<Persistent<Value<'static>>> {
        self.wrappers.get(&id).cloned()
    }

    pub(crate) fn intern_wrapper(&mut self, id: NodeId, value: Persistent<Value<'static>>) {
        self.wrappers.insert(id, value);
    }

    pub(crate) fn token_list(&self, id: NodeId) -> Option<Persistent<Value<'static>>> {
        self.token_lists.get(&id).cloned()
    }

    pub(crate) fn intern_token_list(&mut self, id: NodeId, value: Persistent<Value<'static>>) {
        self.token_lists.insert(id, value);
    }

    pub(crate) fn intern_brand(
        &mut self,
        name: impl Into<String>,
        proto: Persistent<Object<'static>>,
    ) {
        self.brands.insert(name.into(), proto);
    }

    pub(crate) fn brand(&self, name: &str) -> Option<Persistent<Object<'static>>> {
        self.brands.get(name).cloned()
    }

    pub(crate) fn listeners(
        &self,
        target: EventTargetKey,
        typ: &str,
    ) -> Vec<Persistent<Function<'static>>> {
        self.listeners
            .get(&target)
            .into_iter()
            .flatten()
            .filter(|listener| listener.typ == typ)
            .map(|listener| listener.callback.clone())
            .collect()
    }
}

#[derive(Clone, rquickjs::JsLifetime)]
pub(crate) struct SharedWorld(pub Rc<RefCell<World>>);
