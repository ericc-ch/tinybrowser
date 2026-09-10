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
    /// Documents created by DOM APIs (`createHTMLDocument`, `createDocument`)
    /// beyond the parser's main document, keyed by their `NodeId` document id.
    pub extra_documents: HashMap<u32, Parsed>,
    pub document_url: Url,
    pub services: Arc<dyn BrowserServices>,
    pub pending_cancels: Vec<i32>,
    pub pending_html_writes: Vec<String>,
    pub parser_active: bool,
    pub document_ready: bool,
    listeners: HashMap<EventTargetKey, Vec<Listener>>,
    wrappers: HashMap<NodeId, Persistent<Value<'static>>>,
    token_lists: HashMap<NodeId, Persistent<Value<'static>>>,
    named_node_maps: HashMap<NodeId, Persistent<Value<'static>>>,
    implementations: HashMap<u32, Persistent<Value<'static>>>,
    brands: HashMap<String, Persistent<Object<'static>>>,
    /// `Attr` platform-object identity, keyed by a per-realm id.
    pub(crate) attrs: HashMap<u64, AttrState>,
    /// Owner element for each `Attr` id; `None` while detached.
    pub(crate) attr_owners: HashMap<u64, Option<NodeId>>,
    /// Last known value, so a detached `Attr` keeps its data.
    pub(crate) attr_values: HashMap<u64, String>,
    /// Wrapper object for each `Attr` id (identity is the id).
    pub(crate) attr_wrappers: HashMap<u64, Persistent<Value<'static>>>,
    /// Attached attributes: (element, namespace, local) -> `Attr` id.
    pub(crate) attr_ids: HashMap<(NodeId, String, String), u64>,
    pub(crate) next_attr_id: u64,
}

impl World {
    pub(crate) fn new(services: Arc<dyn BrowserServices>, document_url: Url) -> Self {
        Self {
            parsed: None,
            extra_documents: HashMap::new(),
            document_url,
            services,
            pending_cancels: Vec::new(),
            pending_html_writes: Vec::new(),
            parser_active: false,
            document_ready: false,
            listeners: HashMap::new(),
            wrappers: HashMap::new(),
            token_lists: HashMap::new(),
            named_node_maps: HashMap::new(),
            implementations: HashMap::new(),
            brands: HashMap::new(),
            attrs: HashMap::new(),
            attr_owners: HashMap::new(),
            attr_values: HashMap::new(),
            attr_wrappers: HashMap::new(),
            attr_ids: HashMap::new(),
            next_attr_id: 0,
        }
    }

    pub(crate) fn replace_document(&mut self, parsed: Parsed) {
        self.parsed = Some(parsed);
        self.document_ready = false;
        self.listeners.clear();
        self.wrappers.clear();
        self.token_lists.clear();
        self.named_node_maps.clear();
        self.implementations.clear();
    }

    /// One `DOMImplementation` object per document, for identity.
    pub(crate) fn implementation(&self, id: NodeId) -> Option<Persistent<Value<'static>>> {
        self.implementations.get(&id.document_id()).cloned()
    }

    pub(crate) fn intern_implementation(&mut self, id: NodeId, value: Persistent<Value<'static>>) {
        self.implementations.insert(id.document_id(), value);
    }

    /// The document tree that owns `id`.
    pub(crate) fn document(&self, id: NodeId) -> Option<&Parsed> {
        if let Some(parsed) = self.parsed.as_ref()
            && parsed.dom.document_id() == id.document_id()
        {
            return Some(parsed);
        }
        self.extra_documents.get(&id.document_id())
    }

    /// Mutable version of [`World::document`].
    pub(crate) fn document_mut(&mut self, id: NodeId) -> Option<&mut Parsed> {
        if let Some(parsed) = self.parsed.as_ref()
            && parsed.dom.document_id() == id.document_id()
        {
            return self.parsed.as_mut();
        }
        self.extra_documents.get_mut(&id.document_id())
    }

    /// Stores a secondary document and returns its root id.
    pub(crate) fn add_document(&mut self, parsed: Parsed) -> NodeId {
        let id = parsed.dom.document();
        self.extra_documents.insert(id.document_id(), parsed);
        id
    }

    pub(crate) fn add_listener(&mut self, target: EventTargetKey, listener: Listener) {
        self.listeners.entry(target).or_default().push(listener);
    }

    pub(crate) fn clear_listeners(&mut self) {
        self.listeners.clear();
        self.wrappers.clear();
        self.token_lists.clear();
        self.named_node_maps.clear();
        self.implementations.clear();
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

    pub(crate) fn named_node_map(&self, id: NodeId) -> Option<Persistent<Value<'static>>> {
        self.named_node_maps.get(&id).cloned()
    }

    pub(crate) fn intern_named_node_map(&mut self, id: NodeId, value: Persistent<Value<'static>>) {
        self.named_node_maps.insert(id, value);
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

/// Identity of one `Attr` platform object.
pub(crate) struct AttrState {
    pub namespace: String,
    pub prefix: Option<String>,
    pub local: String,
    pub qualified: String,
}
