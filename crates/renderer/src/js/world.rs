//! Shared JS world for the renderer.

use std::cell::{Cell, Ref, RefCell, RefMut};
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};

use dom::NodeId;
use rquickjs::{Object, Persistent, Value, class::Trace, function::Function};
use url::Url;

use crate::document::{Document, FrameRuntime};
use crate::messaging::{MAX_FRAMES, SharedHandle};
use crate::protocol::{FrameId, StorageChange, StorageError, StorageKind};
use crate::storage::PendingStorageEvent;
use crate::{Parsed, ReadyState};

/// Renderer-process realm bookkeeping shared by every frame.
///
/// Owned by the page engine and dropped while its `QuickJS` runtime is still
/// alive, so cached wrappers never outlive the heap.
#[derive(Default)]
pub(crate) struct RealmRegistry {
    budget: Rc<RefCell<ResourceBudget>>,
    /// The World that owns each document id, for wrapper realm resolution.
    documents: HashMap<u32, Weak<RefCell<World>>>,
    /// The World that owns each frame, for `window.parent` and `event.source`.
    frames: HashMap<FrameId, Weak<RefCell<World>>>,
    /// One wrapper per node, shared by every realm in this renderer process.
    /// The persistent holds a `WeakRef`, so an unreferenced wrapper can still
    /// be collected.
    wrappers: HashMap<NodeId, Persistent<Value<'static>>>,
    /// The active child document for each connected iframe container.
    frame_documents: HashMap<NodeId, NodeId>,
    /// `WebDriver` element ids, allocated across every world and frame so a
    /// reference cannot alias between browsing contexts.
    next_remote: u64,
}

impl RealmRegistry {
    /// The next process-unique `WebDriver` element id.
    pub(crate) fn allocate_remote(&mut self) -> u64 {
        self.next_remote = self.next_remote.saturating_add(1);
        self.next_remote
    }
}

#[derive(Default)]
struct ResourceBudget {
    pending_stream_bytes: usize,
    object_url_bytes: usize,
}

const MAX_PENDING_STREAM_BYTES: usize = 8 * 1024 * 1024;
const MAX_OBJECT_URL_BYTES: usize = 8 * 1024 * 1024;

impl RealmRegistry {
    /// Remembers that `world` owns the document `id`.
    pub(crate) fn insert_document(&mut self, id: u32, world: &Rc<RefCell<World>>) {
        self.documents.insert(id, Rc::downgrade(world));
    }

    /// Remembers that `world` is the realm of `frame`.
    pub(crate) fn insert_frame(&mut self, frame: FrameId, world: &Rc<RefCell<World>>) {
        self.frames.insert(frame, Rc::downgrade(world));
    }

    /// The realm of `frame`, while it is alive.
    pub(crate) fn frame_world(&self, frame: FrameId) -> Option<Rc<RefCell<World>>> {
        self.frames.get(&frame).and_then(Weak::upgrade)
    }

    /// Drops the realm association for a frame that is gone.
    pub(crate) fn forget_frame_world(&mut self, frame: FrameId) {
        self.frames.remove(&frame);
    }

    /// The realm that owns the document `id` points into, if still alive.
    pub(crate) fn owner_world(&self, id: NodeId) -> Option<Rc<RefCell<World>>> {
        self.documents
            .get(&id.document_id())
            .and_then(Weak::upgrade)
    }

    /// Drops every realm and wrapper association for a document that is gone.
    pub(crate) fn forget_document(&mut self, id: u32) {
        self.documents.remove(&id);
        self.wrappers.retain(|node, _| node.document_id() != id);
        self.frame_documents
            .retain(|_, document| document.document_id() != id);
    }

    /// The shared wrapper cache entry for `id`, when one exists.
    pub(crate) fn wrapper(&self, id: NodeId) -> Option<Persistent<Value<'static>>> {
        self.wrappers.get(&id).cloned()
    }

    /// Publishes the wrapper cache entry for `id`.
    pub(crate) fn intern_wrapper(&mut self, id: NodeId, value: Persistent<Value<'static>>) {
        self.wrappers.insert(id, value);
    }

    pub(crate) fn frame_document(&self, container: NodeId) -> Option<NodeId> {
        self.frame_documents.get(&container).copied()
    }

    pub(crate) fn set_frame_document(&mut self, container: NodeId, document: NodeId) {
        self.frame_documents.insert(container, document);
    }

    pub(crate) fn forget_frame(&mut self, container: NodeId) {
        self.frame_documents.remove(&container);
    }

    /// Drops every cached wrapper; called while the runtime is still alive.
    pub(crate) fn clear(&mut self) {
        self.documents.clear();
        self.frames.clear();
        self.wrappers.clear();
        self.frame_documents.clear();
    }

    fn budget(&self) -> Rc<RefCell<ResourceBudget>> {
        Rc::clone(&self.budget)
    }
}

/// A child frame navigation queued from inside a script, applied after it
/// stops: the `iframe`'s `src` changed (or the element just connected).
pub(crate) struct FrameNavigation {
    /// The `iframe` container whose frame should navigate.
    pub(crate) container: NodeId,
    /// The spec to navigate to, resolved against the parent document.
    pub(crate) spec: String,
}

pub(crate) enum DocumentStreamCommand {
    Open,
    Write(String),
    Close,
}

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

impl RecordData {
    /// A record of one mutation type on one target, with every payload
    /// collection empty.
    pub(crate) fn new(typ: &str, target: Handle) -> Self {
        Self {
            typ: typ.to_owned(),
            target,
            added: Vec::new(),
            removed: Vec::new(),
            previous: None,
            next: None,
            attribute_name: None,
            attribute_namespace: None,
            old_value: None,
        }
    }
}

pub(crate) struct ObserverState {
    pub callback: Persistent<Function<'static>>,
    /// The observer platform object, for the callback's `this` value and
    /// second argument
    /// (<https://dom.spec.whatwg.org/#notify-mutation-observers>). Present
    /// while the observer is registered; cleared by `disconnect`.
    pub object: Option<Persistent<Object<'static>>>,
    pub observations: Vec<Observation>,
    pub queue: Vec<RecordData>,
}

/// One observer with queued records, ready for callback delivery.
pub(crate) struct ReadyObserver {
    /// Creation-order id; delivery follows it.
    pub id: u64,
    pub callback: Persistent<Function<'static>>,
    pub object: Persistent<Object<'static>>,
    pub records: Vec<RecordData>,
}

pub(crate) struct Listener {
    pub typ: String,
    pub callback: Option<Persistent<Value<'static>>>,
    pub capture: bool,
    pub once: bool,
    pub passive: bool,
    pub signal: Option<Persistent<Object<'static>>>,
    /// Cleared when the listener is removed; a clone taken for dispatch still
    /// sees the removal (<https://dom.spec.whatwg.org/#concept-event-listener>).
    pub removed: Cell<bool>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum EventTargetKey {
    Window,
    Node(NodeId),
    /// A constructible `EventTarget`, numbered per world.
    Standalone(u64),
}

/// One `blob:` URL's data and the `Blob.type` it was created from, so a
/// fetch of the URL can carry a `Content-Type`
/// (<https://w3c.github.io/FileAPI/#blob-url>).
pub(crate) struct ObjectUrlEntry {
    pub(crate) contents: Rc<str>,
    pub(crate) content_type: Rc<str>,
}

/// One lazily-created sub-object cached per node.
///
/// Each interface creates its platform object once per element, so
/// `element.classList === element.classList`; the cache is keyed by node and
/// interface and cleared whenever the realm's wrappers are invalidated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Wrapper {
    /// `Element.classList`.
    TokenList,
    /// `Element.attributes`.
    NamedNodeMap,
    /// `HTMLElement.style`.
    StyleDeclaration,
    /// `HTMLElement.dataset`.
    Dataset,
}

pub(crate) struct World {
    /// The handles every frame of this renderer process shares: trees,
    /// registry and wrapper cache, ports, JS heap, wake handle, and stop flag.
    pub(crate) runtime: FrameRuntime,
    /// The frame this realm belongs to.
    frame: FrameId,
    /// Frames whose browsing context was registered inside a script but whose
    /// realm cannot be created until JS execution has stopped.
    pending_frames: Vec<(FrameId, NodeId)>,
    /// Child frames this realm created for the engine to adopt.
    new_frames: Vec<(FrameId, NodeId, Document)>,
    /// The active document of the frame this realm belongs to.
    document: Option<u32>,
    /// Document ids this realm created; only these feed its observers.
    owned: HashSet<u32>,
    pub document_url: Url,
    pub pending_cancels: Vec<i32>,
    pub pending_html_writes: Vec<String>,
    frame_navigations: Vec<FrameNavigation>,
    document_stream: Vec<DocumentStreamCommand>,
    object_urls: HashMap<String, ObjectUrlEntry>,
    budget: Rc<RefCell<ResourceBudget>>,
    next_object_url: u64,
    pub parser_active: bool,
    pub current_script: Option<NodeId>,
    /// The realm's window object, for event targets that belong to this world
    /// but are reached from another realm's call frame.
    window: Option<Persistent<Object<'static>>>,
    listeners: HashMap<EventTargetKey, Vec<Rc<Listener>>>,
    /// The `EventTarget` object behind each `EventTargetKey::Standalone`.
    standalone_targets: HashMap<u64, Persistent<Object<'static>>>,
    next_standalone_target: u64,
    /// Lazily-created platform objects whose identity is one per (node,
    /// interface), so `element.classList === element.classList` and friends.
    wrappers: HashMap<(NodeId, Wrapper), Persistent<Value<'static>>>,
    implementations: HashMap<u32, Persistent<Value<'static>>>,
    /// The focused element of each document
    /// (<https://html.spec.whatwg.org/multipage/interaction.html#focused-area-of-the-document>).
    active_elements: HashMap<u32, NodeId>,
    /// Elements whose `click()` is running, so a nested `click()` returns
    /// (<https://html.spec.whatwg.org/multipage/interaction.html#dom-click>).
    clicks_in_progress: HashSet<NodeId>,
    brands: HashMap<String, Persistent<Object<'static>>>,
    /// `Attr` platform-object identity, keyed by a per-realm id.
    pub(crate) attrs: HashMap<u64, AttrState>,
    /// Owner element for each `Attr` id; `None` while detached.
    pub(crate) attr_owners: HashMap<u64, Option<NodeId>>,
    /// Event handler properties (`element.onload`, `window.onmessage`) live
    /// here rather than on the wrapper, which may be collected while the node
    /// stays alive. Keyed by `(None, name)` for the window and
    /// `(Some(node), name)` for an element
    /// (<https://html.spec.whatwg.org/multipage/webappapis.html#event-handlers>).
    handler_attributes: HashMap<(Option<NodeId>, String), Persistent<Value<'static>>>,
    /// Handler properties explicitly set to null or undefined, so a dispatch
    /// does not resurrect the element's content attribute
    /// (<https://html.spec.whatwg.org/multipage/webappapis.html#event-handler-content-attributes>).
    cleared_handlers: HashSet<(Option<NodeId>, String)>,
    /// Last known value, so a detached `Attr` keeps its data.
    pub(crate) attr_values: HashMap<u64, String>,
    /// Wrapper object for each `Attr` id (identity is the id).
    pub(crate) attr_wrappers: HashMap<u64, Persistent<Value<'static>>>,
    /// Stable `WebDriver` element ids for nodes, and the reverse lookup.
    remote_ids: HashMap<NodeId, u64>,
    remote_nodes: HashMap<u64, NodeId>,
    /// Attached attributes: (element, namespace, local) -> `Attr` id.
    pub(crate) attr_ids: HashMap<(NodeId, String, String), u64>,
    pub(crate) next_attr_id: u64,
    /// Registered `MutationObserver`s, keyed by their platform id.
    pub(crate) observers: HashMap<u64, ObserverState>,
    pub(crate) next_observer_id: u64,
    pub(crate) delivery_scheduled: bool,
}

impl Drop for World {
    fn drop(&mut self) {
        let object_bytes = self
            .object_urls
            .values()
            .map(|entry| entry.contents.len() + entry.content_type.len())
            .sum::<usize>();
        let mut budget = self.budget.borrow_mut();
        budget.object_url_bytes = budget.object_url_bytes.saturating_sub(object_bytes);
        let stream_bytes = self
            .document_stream
            .iter()
            .filter_map(|command| match command {
                DocumentStreamCommand::Write(value) => Some(value.len()),
                _ => None,
            })
            .chain(self.pending_html_writes.iter().map(String::len))
            .sum::<usize>();
        budget.pending_stream_bytes = budget.pending_stream_bytes.saturating_sub(stream_bytes);
    }
}

impl World {
    pub(crate) fn new(document_url: Url, frame: FrameId, runtime: &FrameRuntime) -> Self {
        Self {
            runtime: runtime.clone(),
            frame,
            pending_frames: Vec::new(),
            new_frames: Vec::new(),
            document: None,
            owned: HashSet::new(),
            document_url,
            pending_cancels: Vec::new(),
            pending_html_writes: Vec::new(),
            frame_navigations: Vec::new(),
            document_stream: Vec::new(),
            object_urls: HashMap::new(),
            budget: runtime.registry.borrow().budget(),
            next_object_url: 0,
            parser_active: false,
            current_script: None,
            window: None,
            listeners: HashMap::new(),
            standalone_targets: HashMap::new(),
            next_standalone_target: 0,
            wrappers: HashMap::new(),
            implementations: HashMap::new(),
            active_elements: HashMap::new(),
            clicks_in_progress: HashSet::new(),
            brands: HashMap::new(),
            attrs: HashMap::new(),
            attr_owners: HashMap::new(),
            handler_attributes: HashMap::new(),
            cleared_handlers: HashSet::new(),
            attr_values: HashMap::new(),
            attr_wrappers: HashMap::new(),
            remote_ids: HashMap::new(),
            remote_nodes: HashMap::new(),
            attr_ids: HashMap::new(),
            next_attr_id: 0,
            observers: HashMap::new(),
            next_observer_id: 0,
            delivery_scheduled: false,
        }
    }

    /// Turns mutation recording on for this realm's documents.
    pub(crate) fn set_recording(&mut self, recording: bool) {
        let mut documents = self.runtime.documents.borrow_mut();
        for id in &self.owned {
            if let Some(parsed) = documents.get_mut(*id) {
                parsed.dom.set_record_mutations(recording);
            }
        }
    }

    /// Drains this realm's documents' mutation logs and matches the mutations
    /// against all registered observers, appending to their queues.
    pub(crate) fn drain_mutations(&mut self) {
        let mut documents = self.runtime.documents.borrow_mut();
        for id in &self.owned {
            let Some(parsed) = documents.get_mut(*id) else {
                continue;
            };
            let mutations = parsed.dom.take_mutations();
            if mutations.is_empty() || self.observers.is_empty() {
                continue;
            }
            for mutation in mutations {
                for observer in self.observers.values_mut() {
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

    /// Removes observers that have queued records, for callback delivery
    /// (<https://dom.spec.whatwg.org/#notify-mutation-observers>).
    ///
    /// Sorted by observer id so delivery is deterministic and follows
    /// registration order; `HashMap` iteration order is not.
    pub(crate) fn take_ready(&mut self) -> Vec<ReadyObserver> {
        let mut ready = Vec::new();
        for (&id, state) in &mut self.observers {
            if !state.queue.is_empty()
                && let Some(object) = &state.object
            {
                ready.push(ReadyObserver {
                    id,
                    callback: state.callback.clone(),
                    object: object.clone(),
                    records: std::mem::take(&mut state.queue),
                });
            }
        }
        ready.sort_by_key(|observer| observer.id);
        ready
    }

    /// Installs `parsed` as the active document and returns its id.
    pub(crate) fn replace_document(&mut self, parsed: Parsed) -> u32 {
        self.drop_active_document();
        let id = self.runtime.documents.borrow_mut().insert(parsed);
        self.document = Some(id);
        self.owned.insert(id);
        self.current_script = None;
        self.listeners.clear();
        self.wrappers.clear();
        self.implementations.clear();
        self.active_elements.clear();
        self.clear_attributes();
        self.frame_navigations.clear();
        self.remote_ids.clear();
        self.remote_nodes.clear();
        let pending = self.take_document_stream();
        drop(pending);
        // A new realm owns fresh observers; navigation drops the old ones.
        self.observers.clear();
        self.delivery_scheduled = false;
        id
    }

    /// Installs the frame's active document without clearing realm caches;
    /// the parser owns the tree mid-parse. Returns the new document id.
    ///
    /// The parser hands the same tree back at every script boundary, so the
    /// document, its wrappers, and its realm keep their identity while the
    /// object is replaced in the store.
    pub(crate) fn set_document(&mut self, parsed: Parsed) -> u32 {
        let id = parsed.dom.document_id();
        if self.document == Some(id) {
            self.runtime.documents.borrow_mut().insert(parsed);
            self.current_script = None;
            return id;
        }
        self.drop_active_document();
        let id = self.runtime.documents.borrow_mut().insert(parsed);
        self.document = Some(id);
        self.owned.insert(id);
        self.current_script = None;
        id
    }

    fn drop_active_document(&mut self) {
        if let Some(old) = self.document.take() {
            self.owned.remove(&old);
            self.runtime.documents.borrow_mut().remove(old);
            self.runtime.registry.borrow_mut().forget_document(old);
        }
    }

    /// Drops realm and wrapper associations for every document this realm
    /// owns; called when the frame goes away.
    pub(crate) fn forget_owned_documents(&mut self) {
        for id in self.owned.drain() {
            self.runtime.documents.borrow_mut().remove(id);
            self.runtime.registry.borrow_mut().forget_document(id);
            self.handler_attributes
                .retain(|(node, _), _| node.is_none_or(|node| node.document_id() != id));
            self.cleared_handlers
                .retain(|(node, _)| node.is_none_or(|node| node.document_id() != id));
        }
        self.document = None;
    }

    /// The registry every realm of this renderer process shares.
    pub(crate) fn registry(&self) -> Rc<RefCell<RealmRegistry>> {
        Rc::clone(&self.runtime.registry)
    }

    /// The realm that owns the document `id` points into, when alive.
    pub(crate) fn owner_world(&self, id: NodeId) -> Option<Rc<RefCell<World>>> {
        self.runtime.registry.borrow().owner_world(id)
    }

    /// The shared wrapper cached for `id`, when one exists.
    pub(crate) fn shared_wrapper(&self, id: NodeId) -> Option<Persistent<Value<'static>>> {
        self.runtime.registry.borrow().wrapper(id)
    }

    /// Publishes the shared wrapper cached for `id`.
    pub(crate) fn intern_shared_wrapper(&self, id: NodeId, value: Persistent<Value<'static>>) {
        self.runtime.registry.borrow_mut().intern_wrapper(id, value);
    }

    /// The active document of the frame this realm belongs to.
    pub(crate) fn main_document(&self) -> Option<Ref<'_, Parsed>> {
        let id = self.document?;
        Ref::filter_map(self.runtime.documents.borrow(), |store| store.get(id)).ok()
    }

    /// Mutable access to the active document.
    pub(crate) fn main_document_mut(&self) -> Option<RefMut<'_, Parsed>> {
        let id = self.document?;
        RefMut::filter_map(self.runtime.documents.borrow_mut(), |store| {
            store.get_mut(id)
        })
        .ok()
    }

    /// Removes the active document from the store, for the parser to own.
    pub(crate) fn take_main_document(&mut self) -> Option<Parsed> {
        let id = self.document.take()?;
        self.owned.remove(&id);
        self.runtime.documents.borrow_mut().remove(id)
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
        Ref::filter_map(self.runtime.documents.borrow(), |store| {
            store.get(id.document_id())
        })
        .ok()
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
        let store = self.runtime.documents.borrow();
        store.get(id.document_id()).map(reader)
    }

    /// Runs `reader` against the frame's active document.
    pub(crate) fn with_main_document<R>(&self, reader: impl FnOnce(&Parsed) -> R) -> Option<R> {
        let id = self.document?;
        let store = self.runtime.documents.borrow();
        store.get(id).map(reader)
    }

    /// Mutable version of [`World::document`].
    pub(crate) fn document_mut(&self, id: NodeId) -> Option<RefMut<'_, Parsed>> {
        RefMut::filter_map(self.runtime.documents.borrow_mut(), |store| {
            store.get_mut(id.document_id())
        })
        .ok()
    }

    /// Stores a secondary document and returns its root id.
    pub(crate) fn add_document(&mut self, parsed: Parsed) -> NodeId {
        let root = parsed.dom.document();
        let id = self.runtime.documents.borrow_mut().insert(parsed);
        self.owned.insert(id);
        root
    }

    pub(crate) fn frame_document(&self, container: NodeId) -> Option<NodeId> {
        self.runtime.registry.borrow().frame_document(container)
    }

    /// The frame this realm belongs to.
    pub(crate) fn frame(&self) -> FrameId {
        self.frame
    }

    /// The renderer-process state shared by every realm.
    pub(crate) fn shared(&self) -> SharedHandle {
        Rc::clone(&self.runtime.shared)
    }

    /// Creates a frame's document sharing this realm's runtime, services, and
    /// stores; used for child frames created from inside a script.
    pub(crate) fn create_frame_document(&self, frame: FrameId) -> Document {
        Document::with_shared(frame, &self.runtime)
    }

    /// Registers a browsing context for every connected `iframe` in this
    /// frame's document that does not have one yet.
    ///
    /// Safe to call while a script runs: it allocates frame identities and
    /// tree entries only. The document and its realm are created later, by
    /// [`World::materialize_frames`], because `QuickJS` forbids entering a new
    /// realm while another realm executes.
    pub(crate) fn register_pending_frames(&mut self) -> Vec<NodeId> {
        let mut created = Vec::new();
        let has_iframes = self
            .with_main_document(|parsed| parsed.dom.connected_iframe_count() > 0)
            .unwrap_or(false);
        if !has_iframes {
            return created;
        }
        let containers = self.iframe_containers_in_order();
        for container in containers {
            if self
                .runtime
                .shared
                .borrow()
                .tree
                .frame_for_container(container)
                .is_some()
            {
                continue;
            }
            if self.runtime.shared.borrow().tree.len() >= MAX_FRAMES {
                return created;
            }
            let frame = {
                let mut shared = self.runtime.shared.borrow_mut();
                let frame = shared.allocate_frame();
                shared.tree.add(self.frame, frame, container);
                frame
            };
            self.pending_frames.push((frame, container));
            created.push(container);
        }
        created
    }

    /// Creates the documents and realms for every registered frame.
    ///
    /// Must not run while a `QuickJS` realm is executing; every caller is a
    /// renderer-loop entry point or a bindings path that runs outside JS.
    pub(crate) fn materialize_frames(&mut self) -> Vec<NodeId> {
        let mut created = Vec::new();
        for (frame, container) in std::mem::take(&mut self.pending_frames) {
            let mut document = self.create_frame_document(frame);
            document.load_about_blank(Some(self.document_url.as_str()));
            self.new_frames.push((frame, container, document));
            created.push(container);
        }
        created
    }

    /// Registers and materializes frames; safe only outside JS execution.
    pub(crate) fn adopt_pending_frames(&mut self) -> Vec<NodeId> {
        let mut created = self.register_pending_frames();
        created.extend(self.materialize_frames());
        created
    }

    /// Takes the child frames this realm created for the engine to adopt.
    pub(crate) fn take_new_frames(&mut self) -> Vec<(FrameId, NodeId, Document)> {
        std::mem::take(&mut self.new_frames)
    }

    /// The frame's connected `iframe` containers in tree order, which is what
    /// orders the frame's child browsing contexts
    /// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-length>).
    #[must_use]
    pub(crate) fn iframe_containers_in_order(&self) -> Vec<NodeId> {
        self.with_main_document(|parsed| {
            let mut containers = Vec::new();
            let mut stack = vec![parsed.dom.document()];
            while let Some(id) = stack.pop() {
                if parsed.dom.is_iframe_element(id) && parsed.dom.is_connected(id) {
                    containers.push(id);
                }
                if let Some(children) = parsed.dom.children(id) {
                    stack.extend(children.rev().copied());
                }
            }
            containers
        })
        .unwrap_or_default()
    }

    /// The realm of `frame`, while it is alive.
    pub(crate) fn frame_world(&self, frame: FrameId) -> Option<Rc<RefCell<World>>> {
        self.runtime.registry.borrow().frame_world(frame)
    }

    /// The child frame owned by `container`, when its browsing context
    /// exists (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#dom-iframe-contentwindow>).
    pub(crate) fn frame_for_container(&self, container: NodeId) -> Option<FrameId> {
        self.runtime
            .shared
            .borrow()
            .tree
            .frame_for_container(container)
    }

    /// The serialized origin of this realm's document
    /// (<https://html.spec.whatwg.org/multipage/browsers.html#concept-origin>).
    pub(crate) fn origin_string(&self) -> String {
        self.document_url.origin().ascii_serialization()
    }

    /// The origin scoping this realm's storage areas, or `None` for an opaque
    /// origin, where the storage getters must throw `SecurityError`
    /// (<https://html.spec.whatwg.org/multipage/webstorage.html#the-localstorage-attribute>).
    pub(crate) fn storage_origin(&self) -> Option<String> {
        match self.document_url.origin() {
            url::Origin::Tuple(..) => Some(self.origin_string()),
            url::Origin::Opaque(_) => None,
        }
    }

    /// `Storage.getItem(key)` for the `"local"` or `"session"` area. The
    /// renderer owns the session area; the browser owns the local one.
    pub(crate) fn storage_get(&self, kind: &str, key: &str) -> Option<String> {
        let origin = self.storage_origin()?;
        match kind {
            "session" => self.runtime.session_storage.borrow().get(&origin, key),
            _ => self.runtime.services.storage_get(&origin, key),
        }
    }

    /// The keys of one storage area, in the area's iteration order.
    pub(crate) fn storage_keys(&self, kind: &str) -> Vec<String> {
        let Some(origin) = self.storage_origin() else {
            return Vec::new();
        };
        match kind {
            "session" => self.runtime.session_storage.borrow().keys(&origin),
            _ => self.runtime.services.storage_keys(&origin),
        }
    }

    /// `Storage.setItem(key, value)`; `Ok(None)` means no change and the error
    /// is a refused write. A change queues the `storage` event for every other
    /// same-origin frame of the engine
    /// (<https://html.spec.whatwg.org/multipage/webstorage.html#concept-storage-broadcast>).
    pub(crate) fn storage_set(
        &self,
        kind: &str,
        key: &str,
        value: &str,
    ) -> Result<Option<StorageChange>, StorageError> {
        let Some(origin) = self.storage_origin() else {
            return Ok(None);
        };
        let kind = Self::storage_kind(kind);
        let change = match kind {
            StorageKind::Session => self
                .runtime
                .session_storage
                .borrow_mut()
                .set(&origin, key, value),
            StorageKind::Local => self.runtime.services.storage_set(
                &origin,
                self.document_url.as_str(),
                key,
                value,
                self.frame,
            ),
        }?;
        if let Some(change) = &change {
            self.queue_storage_event(kind, &origin, change);
        }
        Ok(change)
    }

    /// `Storage.removeItem(key)`; `None` means the key was absent.
    pub(crate) fn storage_remove(&self, kind: &str, key: &str) -> Option<StorageChange> {
        let origin = self.storage_origin()?;
        let kind = Self::storage_kind(kind);
        let change = match kind {
            StorageKind::Session => self
                .runtime
                .session_storage
                .borrow_mut()
                .remove(&origin, key),
            StorageKind::Local => self.runtime.services.storage_remove(
                &origin,
                self.document_url.as_str(),
                key,
                self.frame,
            ),
        };
        if let Some(change) = &change {
            self.queue_storage_event(kind, &origin, change);
        }
        change
    }

    /// `Storage.clear()`; `None` means the area was empty.
    pub(crate) fn storage_clear(&self, kind: &str) -> Option<StorageChange> {
        let origin = self.storage_origin()?;
        let kind = Self::storage_kind(kind);
        let change = match kind {
            StorageKind::Session => self.runtime.session_storage.borrow_mut().clear(&origin),
            StorageKind::Local => {
                self.runtime
                    .services
                    .storage_clear(&origin, self.document_url.as_str(), self.frame)
            }
        };
        if let Some(change) = &change {
            self.queue_storage_event(kind, &origin, change);
        }
        change
    }

    fn storage_kind(kind: &str) -> StorageKind {
        if kind == "session" {
            StorageKind::Session
        } else {
            StorageKind::Local
        }
    }

    /// Queues the event for a change this engine made. Only `sessionStorage`
    /// needs it: `localStorage` changes return as a browser broadcast, which
    /// excludes the source frame.
    fn queue_storage_event(&self, kind: StorageKind, origin: &str, change: &StorageChange) {
        if kind != StorageKind::Session {
            return;
        }
        self.runtime
            .pending_storage
            .borrow_mut()
            .push(PendingStorageEvent {
                source: Some(self.frame),
                origin: origin.to_owned(),
                kind,
                key: change.key.clone(),
                old_value: change.old_value.clone(),
                new_value: change.new_value.clone(),
                url: self.document_url.to_string(),
            });
    }

    /// The root node of the frame's active document.
    pub(crate) fn main_document_root(&self) -> Option<NodeId> {
        self.main_document().map(|parsed| parsed.dom.document())
    }

    pub(crate) fn queue_frame_navigation(&mut self, navigation: FrameNavigation) {
        self.frame_navigations.push(navigation);
    }

    pub(crate) fn take_frame_navigations(&mut self) -> Vec<FrameNavigation> {
        std::mem::take(&mut self.frame_navigations)
    }

    pub(crate) fn queue_document_stream(
        &mut self,
        command: DocumentStreamCommand,
    ) -> std::result::Result<(), ()> {
        if let DocumentStreamCommand::Write(value) = &command
            && !self.reserve_stream_bytes(value.len())
        {
            return Err(());
        }
        self.document_stream.push(command);
        Ok(())
    }

    pub(crate) fn take_document_stream(&mut self) -> Vec<DocumentStreamCommand> {
        let commands = std::mem::take(&mut self.document_stream);
        let bytes = commands
            .iter()
            .map(|command| match command {
                DocumentStreamCommand::Write(value) => value.len(),
                _ => 0,
            })
            .sum::<usize>();
        self.release_stream_bytes(bytes);
        commands
    }

    pub(crate) fn reserve_stream_bytes(&self, bytes: usize) -> bool {
        let mut budget = self.budget.borrow_mut();
        let Some(total) = budget.pending_stream_bytes.checked_add(bytes) else {
            return false;
        };
        if total > MAX_PENDING_STREAM_BYTES {
            return false;
        }
        budget.pending_stream_bytes = total;
        true
    }

    pub(crate) fn release_stream_bytes(&self, bytes: usize) {
        let mut budget = self.budget.borrow_mut();
        budget.pending_stream_bytes = budget.pending_stream_bytes.saturating_sub(bytes);
    }

    pub(crate) fn create_object_url(
        &mut self,
        contents: String,
        content_type: String,
    ) -> Option<String> {
        let length = contents.len() + content_type.len();
        let mut budget = self.budget.borrow_mut();
        let total = budget.object_url_bytes.checked_add(length)?;
        if total > MAX_OBJECT_URL_BYTES {
            return None;
        }
        let id = self.next_object_url;
        self.next_object_url = self.next_object_url.wrapping_add(1);
        let url = format!("blob:tinybrowser/{id}");
        self.object_urls.insert(
            url.clone(),
            ObjectUrlEntry {
                contents: Rc::from(contents),
                content_type: Rc::from(content_type),
            },
        );
        budget.object_url_bytes = total;
        Some(url)
    }

    pub(crate) fn object_url_contents(&self, url: &str) -> Option<Rc<str>> {
        self.object_urls
            .get(url)
            .map(|entry| Rc::clone(&entry.contents))
    }

    pub(crate) fn object_url_type(&self, url: &str) -> Option<Rc<str>> {
        self.object_urls
            .get(url)
            .map(|entry| Rc::clone(&entry.content_type))
    }

    pub(crate) fn revoke_object_url(&mut self, url: &str) {
        if let Some(entry) = self.object_urls.remove(url) {
            let mut budget = self.budget.borrow_mut();
            budget.object_url_bytes = budget
                .object_url_bytes
                .saturating_sub(entry.contents.len() + entry.content_type.len());
        }
    }

    pub(crate) fn add_listener(&mut self, target: EventTargetKey, listener: Rc<Listener>) {
        self.listeners.entry(target).or_default().push(listener);
    }

    /// A clone of one target's listener list, taken when dispatch invokes the
    /// target (<https://dom.spec.whatwg.org/#concept-event-listener-invoke>).
    pub(crate) fn listener_snapshot(&self, target: EventTargetKey) -> Vec<Rc<Listener>> {
        self.listeners.get(&target).cloned().unwrap_or_default()
    }

    /// Drops one listener from a target's list; the listener's `removed` flag
    /// is what a concurrent dispatch checks, so both happen together.
    pub(crate) fn remove_listener(&mut self, target: EventTargetKey, listener: &Rc<Listener>) {
        if let Some(list) = self.listeners.get_mut(&target) {
            list.retain(|existing| !Rc::ptr_eq(existing, listener));
        }
    }

    pub(crate) fn next_standalone_target(&mut self) -> u64 {
        let id = self.next_standalone_target;
        self.next_standalone_target = self.next_standalone_target.wrapping_add(1);
        id
    }

    pub(crate) fn intern_standalone_target(
        &mut self,
        id: u64,
        target: Persistent<Object<'static>>,
    ) {
        self.standalone_targets.insert(id, target);
    }

    pub(crate) fn standalone_target(&self, id: u64) -> Option<Persistent<Object<'static>>> {
        self.standalone_targets.get(&id).cloned()
    }

    /// The window object of this realm; event dispatch uses it when a target
    /// from this world is reached from another realm's call frame.
    pub(crate) fn set_window(&mut self, window: Persistent<Object<'static>>) {
        self.window = Some(window);
    }

    pub(crate) fn window_object(&self) -> Option<Persistent<Object<'static>>> {
        self.window.clone()
    }

    /// The active document's readiness
    /// (<https://html.spec.whatwg.org/multipage/dom.html#current-document-readiness>).
    pub(crate) fn main_ready_state(&self) -> ReadyState {
        self.main_document()
            .map_or(ReadyState::Complete, |parsed| parsed.ready_state)
    }

    pub(crate) fn set_main_ready_state(&mut self, state: ReadyState) {
        if let Some(mut parsed) = self.main_document_mut() {
            parsed.ready_state = state;
        }
    }

    /// The parent of `id` in its tree, if any.
    pub(crate) fn node_parent(&self, id: NodeId) -> Option<NodeId> {
        self.document(id).and_then(|parsed| parsed.dom.parent(id))
    }

    /// Whether `id` is the root document node of its tree.
    pub(crate) fn node_is_document(&self, id: NodeId) -> bool {
        self.document(id)
            .is_some_and(|parsed| parsed.dom.document() == id)
    }

    pub(crate) fn clear_listeners(&mut self) {
        self.listeners.clear();
        self.standalone_targets.clear();
        self.window = None;
        self.next_standalone_target = 0;
        self.wrappers.clear();
        self.implementations.clear();
        self.brands.clear();
        self.handler_attributes.clear();
        self.cleared_handlers.clear();
        self.clear_attributes();
        self.observers.clear();
    }

    fn clear_attributes(&mut self) {
        self.attrs.clear();
        self.attr_owners.clear();
        self.attr_values.clear();
        self.attr_wrappers.clear();
        self.attr_ids.clear();
        self.next_attr_id = 0;
    }

    /// One cached platform object, if this realm created it.
    pub(crate) fn wrapper(&self, id: NodeId, kind: Wrapper) -> Option<Persistent<Value<'static>>> {
        self.wrappers.get(&(id, kind)).cloned()
    }

    /// Caches one platform object for the node and interface.
    pub(crate) fn intern_wrapper(
        &mut self,
        id: NodeId,
        kind: Wrapper,
        value: Persistent<Value<'static>>,
    ) {
        self.wrappers.insert((id, kind), value);
    }

    /// The focused element of a document, if any.
    pub(crate) fn active_element(&self, document: u32) -> Option<NodeId> {
        self.active_elements.get(&document).copied()
    }

    /// The stable `WebDriver` element id for `node`, allocating one on first
    /// use. Ids come from the process-wide registry so they cannot alias
    /// between windows
    /// (<https://w3c.github.io/webdriver/#elements>).
    pub(crate) fn remote_id(&mut self, node: NodeId) -> u64 {
        if let Some(&remote) = self.remote_ids.get(&node) {
            return remote;
        }
        let remote = self.runtime.registry.borrow_mut().allocate_remote();
        self.remote_ids.insert(node, remote);
        self.remote_nodes.insert(remote, node);
        remote
    }

    /// The node behind a `WebDriver` element id.
    pub(crate) fn node_for_remote(&self, remote: u64) -> Option<NodeId> {
        self.remote_nodes.get(&remote).copied()
    }

    /// Whether `node` is the realm's active document, which is what decides
    /// `document.defaultView`
    /// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-document-defaultview>).
    pub(crate) fn is_main_document(&self, node: NodeId) -> bool {
        self.document == Some(node.document_id())
    }

    /// One event handler property, when set.
    pub(crate) fn handler_attribute(
        &self,
        node: Option<NodeId>,
        name: &str,
    ) -> Option<Persistent<Value<'static>>> {
        self.handler_attributes
            .get(&(node, name.to_owned()))
            .cloned()
    }

    /// Sets or clears one event handler property.
    pub(crate) fn set_handler_attribute(
        &mut self,
        node: Option<NodeId>,
        name: &str,
        value: Option<Persistent<Value<'static>>>,
    ) {
        if let Some(value) = value {
            self.cleared_handlers.remove(&(node, name.to_owned()));
            self.handler_attributes
                .insert((node, name.to_owned()), value);
        } else {
            self.handler_attributes.remove(&(node, name.to_owned()));
            self.cleared_handlers.insert((node, name.to_owned()));
        }
    }

    /// Whether one handler property was explicitly cleared by script.
    pub(crate) fn handler_cleared(&self, node: Option<NodeId>, name: &str) -> bool {
        self.cleared_handlers.contains(&(node, name.to_owned()))
    }

    pub(crate) fn set_active_element(&mut self, document: u32, node: Option<NodeId>) {
        match node {
            Some(node) => {
                self.active_elements.insert(document, node);
            }
            None => {
                self.active_elements.remove(&document);
            }
        }
    }

    /// Whether `node`'s `click()` is already running.
    pub(crate) fn click_in_progress(&self, node: NodeId) -> bool {
        self.clicks_in_progress.contains(&node)
    }

    /// Marks `node`'s `click()` as running; the caller clears it when done.
    pub(crate) fn set_click_in_progress(&mut self, node: NodeId, running: bool) {
        if running {
            self.clicks_in_progress.insert(node);
        } else {
            self.clicks_in_progress.remove(&node);
        }
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
}

/// Whether `mutation` is observable by `observer`, producing a record when
/// it is (<https://dom.spec.whatwg.org/#concept-mo-queue>).
///
/// Every in-scope, enabled registration contributes; one observer gets one
/// record per mutation, and it carries the old value when *any* interested
/// registration asked for it (the spec's `interestedObservers` map folds the
/// registrations per observer).
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
    let mut matched = false;
    let mut want_attribute_old_value = false;
    let mut want_character_data_old_value = false;
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
                        // A filter only ever matches unnamespaced attributes;
                        // namespaced ones are always skipped
                        // (<https://dom.spec.whatwg.org/#queue-a-mutation-record>).
                        (
                            Some(filter),
                            dom::Mutation::Attributes {
                                name, namespace, ..
                            },
                        ) => namespace.is_empty() && filter.iter().any(|wanted| wanted == name),
                        _ => true,
                    }
            }
            _ => observation.options.character_data,
        };
        if !enabled {
            continue;
        }
        matched = true;
        want_attribute_old_value |= observation.options.attribute_old_value;
        want_character_data_old_value |= observation.options.character_data_old_value;
    }
    matched.then(|| {
        record(
            want_attribute_old_value,
            want_character_data_old_value,
            mutation,
        )
    })
}

fn record(
    want_attribute_old_value: bool,
    want_character_data_old_value: bool,
    mutation: &dom::Mutation,
) -> RecordData {
    match mutation {
        dom::Mutation::ChildList {
            target,
            added,
            removed,
            previous,
            next,
        } => RecordData {
            added: added.iter().copied().map(Handle).collect(),
            removed: removed.iter().copied().map(Handle).collect(),
            previous: previous.map(Handle),
            next: next.map(Handle),
            ..RecordData::new("childList", Handle(*target))
        },
        dom::Mutation::Attributes {
            target,
            name,
            namespace,
            old_value,
        } => RecordData {
            attribute_name: Some(name.clone()),
            attribute_namespace: (!namespace.is_empty()).then(|| namespace.clone()),
            old_value: want_attribute_old_value
                .then(|| old_value.clone())
                .flatten(),
            ..RecordData::new("attributes", Handle(*target))
        },
        dom::Mutation::CharacterData { target, old_value } => RecordData {
            old_value: want_character_data_old_value.then(|| old_value.clone()),
            ..RecordData::new("characterData", Handle(*target))
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
#[derive(Clone)]
pub(crate) struct AttrState {
    pub namespace: String,
    pub prefix: Option<String>,
    pub local: String,
    pub qualified: String,
}
