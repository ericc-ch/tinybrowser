//! Shared JS world for the renderer.

use std::cell::{Ref, RefCell, RefMut};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use dom::NodeId;
use rquickjs::{Object, Persistent, Value, class::Trace, function::Function};
use url::Url;

use crate::Parsed;
use crate::documents::DocumentStore;
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
    /// Every tree the renderer process holds, shared by all realms.
    documents: Rc<RefCell<DocumentStore>>,
    /// The active document of the frame this realm belongs to.
    document: Option<u32>,
    /// Document ids this realm created; only these feed its observers.
    owned: HashSet<u32>,
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
    pub(crate) fn new(
        services: Arc<dyn BrowserServices>,
        document_url: Url,
        documents: Rc<RefCell<DocumentStore>>,
    ) -> Self {
        Self {
            documents,
            document: None,
            owned: HashSet::new(),
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

    /// Turns mutation recording on for this realm's documents.
    pub(crate) fn set_recording(&mut self, recording: bool) {
        let mut documents = self.documents.borrow_mut();
        for id in &self.owned {
            if let Some(parsed) = documents.get_mut(*id) {
                parsed.dom.set_record_mutations(recording);
            }
        }
    }

    /// Drains this realm's documents' mutation logs and matches the mutations
    /// against all registered observers, appending to their queues.
    pub(crate) fn drain_mutations(&mut self) {
        let World {
            documents,
            owned,
            observers,
            ..
        } = self;
        let mut documents = documents.borrow_mut();
        for id in owned.iter() {
            let Some(parsed) = documents.get_mut(*id) else {
                continue;
            };
            let mutations = parsed.dom.take_mutations();
            if mutations.is_empty() || observers.is_empty() {
                continue;
            }
            for mutation in mutations {
                for observer in observers.values_mut() {
                    if let Some(record) = match_observation(&parsed.dom, observer, &mutation) {
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
        self.drop_active_document();
        let id = self.documents.borrow_mut().insert(parsed);
        self.document = Some(id);
        self.owned.insert(id);
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

    /// Installs the frame's active document without clearing realm caches;
    /// the parser owns the tree mid-parse.
    pub(crate) fn set_document(&mut self, parsed: Parsed) {
        self.drop_active_document();
        let id = self.documents.borrow_mut().insert(parsed);
        self.document = Some(id);
        self.owned.insert(id);
    }

    fn drop_active_document(&mut self) {
        if let Some(old) = self.document.take() {
            self.owned.remove(&old);
            self.documents.borrow_mut().remove(old);
        }
    }

    /// The active document of the frame this realm belongs to.
    pub(crate) fn main_document(&self) -> Option<Ref<'_, Parsed>> {
        let id = self.document?;
        Ref::filter_map(self.documents.borrow(), |store| store.get(id)).ok()
    }

    /// Mutable access to the active document.
    pub(crate) fn main_document_mut(&self) -> Option<RefMut<'_, Parsed>> {
        let id = self.document?;
        RefMut::filter_map(self.documents.borrow_mut(), |store| store.get_mut(id)).ok()
    }

    /// Removes the active document from the store, for the parser to own.
    pub(crate) fn take_main_document(&mut self) -> Option<Parsed> {
        let id = self.document.take()?;
        self.owned.remove(&id);
        self.documents.borrow_mut().remove(id)
    }

    /// Id of the active document, when one is installed.
    pub(crate) fn main_document_id(&self) -> Option<u32> {
        self.document
    }

    /// One `DOMImplementation` object per document, for identity.
    pub(crate) fn implementation(&self, id: NodeId) -> Option<Persistent<Value<'static>>> {
        self.implementations.get(&id.document_id()).cloned()
    }

    pub(crate) fn intern_implementation(&mut self, id: NodeId, value: Persistent<Value<'static>>) {
        self.implementations.insert(id.document_id(), value);
    }

    /// The document tree that owns `id`.
    pub(crate) fn document(&self, id: NodeId) -> Option<Ref<'_, Parsed>> {
        Ref::filter_map(self.documents.borrow(), |store| store.get(id.document_id())).ok()
    }

    /// Runs `reader` against the tree owning `id`.
    ///
    /// Prefer this to [`World::document`] when a guard would outlive the
    /// caller's `World` borrow; the tree borrow ends with the closure.
    pub(crate) fn with_document<R>(
        &self,
        id: NodeId,
        reader: impl FnOnce(&Parsed) -> R,
    ) -> Option<R> {
        let store = self.documents.borrow();
        store.get(id.document_id()).map(reader)
    }

    /// Runs `reader` against the frame's active document.
    pub(crate) fn with_main_document<R>(&self, reader: impl FnOnce(&Parsed) -> R) -> Option<R> {
        let id = self.document?;
        let store = self.documents.borrow();
        store.get(id).map(reader)
    }

    /// Mutable version of [`World::document`].
    pub(crate) fn document_mut(&self, id: NodeId) -> Option<RefMut<'_, Parsed>> {
        RefMut::filter_map(self.documents.borrow_mut(), |store| {
            store.get_mut(id.document_id())
        })
        .ok()
    }

    /// Stores a secondary document and returns its root id.
    pub(crate) fn add_document(&mut self, parsed: Parsed) -> NodeId {
        let root = parsed.dom.document();
        let id = self.documents.borrow_mut().insert(parsed);
        self.owned.insert(id);
        root
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
