//! Shared JS world for the renderer.

use std::collections::HashMap;
use std::sync::Arc;

use dom::NodeId;
use rquickjs::{Object, Persistent, Value, class::Trace, function::Function};
use url::Url;

use crate::Parsed;
use crate::protocol::BrowserServices;

/// One DOM node handle owned by the JS world.
#[derive(Clone, Copy, rquickjs::JsLifetime)]
pub(crate) struct Handle(pub(crate) NodeId);

impl<'js> Trace<'js> for Handle {
    fn trace<'a>(&self, _tracer: rquickjs::class::Tracer<'a, 'js>) {}
}

/// `MutationObserver` options, already normalized. The booleans mirror the
/// `MutationObserverInit` dictionary one-for-one.
#[allow(
    clippy::struct_excessive_bools,
    reason = "each bool is a MutationObserverInit dictionary member"
)]
#[derive(Clone, Default)]
pub(crate) struct ObserverOptions {
    pub child_list: bool,
    pub attributes: bool,
    pub character_data: bool,
    pub subtree: bool,
    pub attribute_old_value: bool,
    pub character_data_old_value: bool,
    pub attribute_filter: Option<Vec<String>>,
}

#[derive(Clone)]
pub(crate) struct Observation {
    pub target: Handle,
    pub options: ObserverOptions,
}

/// One queued `MutationRecord`, ready to wrap for JS.
#[derive(Clone, rquickjs::JsLifetime, Trace)]
pub(crate) struct RecordData {
    pub typ: String,
    pub target: Handle,
    pub added: Vec<Handle>,
    pub removed: Vec<Handle>,
    pub previous: Option<Handle>,
    pub next: Option<Handle>,
    pub attribute_name: Option<String>,
    pub attribute_namespace: Option<String>,
    pub old_value: Option<String>,
}

pub(crate) struct ObserverState {
    pub callback: Persistent<Function<'static>>,
    pub observations: Vec<Observation>,
    pub queue: Vec<RecordData>,
}

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
    /// Registered `MutationObserver`s, keyed by their platform id.
    pub(crate) observers: HashMap<u64, ObserverState>,
    pub(crate) next_observer_id: u64,
    pub(crate) delivery_scheduled: bool,
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
            observers: HashMap::new(),
            next_observer_id: 0,
            delivery_scheduled: false,
        }
    }

    /// Turns mutation recording on for every document in the world.
    pub(crate) fn set_recording(&mut self, recording: bool) {
        if let Some(parsed) = self.parsed.as_mut() {
            parsed.dom.set_record_mutations(recording);
        }
        for parsed in self.extra_documents.values_mut() {
            parsed.dom.set_record_mutations(recording);
        }
    }

    /// Drains every document's mutation log and matches the mutations
    /// against all registered observers, appending to their queues.
    pub(crate) fn drain_mutations(&mut self) {
        let mut drained: Vec<(u32, Vec<dom::Mutation>)> = Vec::new();
        if let Some(parsed) = self.parsed.as_mut() {
            let mutations = parsed.dom.take_mutations();
            if !mutations.is_empty() {
                drained.push((parsed.dom.document_id(), mutations));
            }
        }
        for parsed in self.extra_documents.values_mut() {
            let mutations = parsed.dom.take_mutations();
            if !mutations.is_empty() {
                drained.push((parsed.dom.document_id(), mutations));
            }
        }
        if drained.is_empty() || self.observers.is_empty() {
            return;
        }
        let World {
            parsed,
            extra_documents,
            observers,
            ..
        } = self;
        for (document_id, mutations) in drained {
            let dom = parsed
                .as_ref()
                .filter(|parsed| parsed.dom.document_id() == document_id)
                .map(|parsed| &parsed.dom)
                .or_else(|| extra_documents.get(&document_id).map(|parsed| &parsed.dom));
            let Some(dom) = dom else {
                continue;
            };
            for mutation in mutations {
                for observer in observers.values_mut() {
                    if let Some(record) = match_observation(dom, observer, &mutation) {
                        observer.queue.push(record);
                    }
                }
            }
        }
    }

    /// Removes and returns one observer's queued records.
    pub(crate) fn take_observer_queue(&mut self, observer: u64) -> Vec<RecordData> {
        self.observers
            .get_mut(&observer)
            .map(|state| std::mem::take(&mut state.queue))
            .unwrap_or_default()
    }

    /// Removes observers that have queued records, for callback delivery.
    pub(crate) fn take_ready(
        &mut self,
    ) -> Vec<(u64, Persistent<Function<'static>>, Vec<RecordData>)> {
        let mut ready = Vec::new();
        for (&id, state) in &mut self.observers {
            if !state.queue.is_empty() {
                ready.push((id, state.callback.clone(), std::mem::take(&mut state.queue)));
            }
        }
        ready
    }

    pub(crate) fn replace_document(&mut self, parsed: Parsed) {
        self.parsed = Some(parsed);
        self.document_ready = false;
        self.listeners.clear();
        self.wrappers.clear();
        self.token_lists.clear();
        self.named_node_maps.clear();
        self.implementations.clear();
        // A new realm owns fresh observers; navigation drops the old ones.
        self.observers.clear();
        self.delivery_scheduled = false;
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
        self.observers.clear();
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

/// Whether `mutation` is observable by `observer`, producing a record when
/// it is (<https://dom.spec.whatwg.org/#concept-mo-queue>).
fn match_observation(
    dom: &dom::Dom,
    observer: &ObserverState,
    mutation: &dom::Mutation,
) -> Option<RecordData> {
    let (target, kind) = match mutation {
        dom::Mutation::ChildList { target, .. } => (*target, 0_u8),
        dom::Mutation::Attributes { target, .. } => (*target, 1_u8),
        dom::Mutation::CharacterData { target, .. } => (*target, 2_u8),
    };
    for observation in &observer.observations {
        let in_scope = observation.target.0 == target
            || (observation.options.subtree
                && inclusive_descendant(dom, observation.target.0, target));
        if !in_scope {
            continue;
        }
        let enabled = match kind {
            0 => observation.options.child_list,
            1 => {
                observation.options.attributes
                    && match (&observation.options.attribute_filter, mutation) {
                        (Some(filter), dom::Mutation::Attributes { name, .. }) => {
                            filter.iter().any(|wanted| wanted == name)
                        }
                        _ => true,
                    }
            }
            _ => observation.options.character_data,
        };
        if !enabled {
            continue;
        }
        return Some(record(observation, mutation));
    }
    None
}

fn record(observation: &Observation, mutation: &dom::Mutation) -> RecordData {
    match mutation {
        dom::Mutation::ChildList {
            target,
            added,
            removed,
            previous,
            next,
        } => RecordData {
            typ: "childList".into(),
            target: Handle(*target),
            added: added.iter().copied().map(Handle).collect(),
            removed: removed.iter().copied().map(Handle).collect(),
            previous: previous.map(Handle),
            next: next.map(Handle),
            attribute_name: None,
            attribute_namespace: None,
            old_value: None,
        },
        dom::Mutation::Attributes {
            target,
            name,
            namespace,
            old_value,
        } => RecordData {
            typ: "attributes".into(),
            target: Handle(*target),
            added: Vec::new(),
            removed: Vec::new(),
            previous: None,
            next: None,
            attribute_name: Some(name.clone()),
            attribute_namespace: (!namespace.is_empty()).then(|| namespace.clone()),
            old_value: observation
                .options
                .attribute_old_value
                .then(|| old_value.clone())
                .flatten(),
        },
        dom::Mutation::CharacterData { target, old_value } => RecordData {
            typ: "characterData".into(),
            target: Handle(*target),
            added: Vec::new(),
            removed: Vec::new(),
            previous: None,
            next: None,
            attribute_name: None,
            attribute_namespace: None,
            old_value: observation
                .options
                .character_data_old_value
                .then(|| old_value.clone()),
        },
    }
}

fn inclusive_descendant(dom: &dom::Dom, ancestor: NodeId, node: NodeId) -> bool {
    let mut cursor = Some(node);
    while let Some(id) = cursor {
        if id == ancestor {
            return true;
        }
        cursor = dom.parent(id);
    }
    false
}

/// Identity of one `Attr` platform object.
pub(crate) struct AttrState {
    pub namespace: String,
    pub prefix: Option<String>,
    pub local: String,
    pub qualified: String,
}
