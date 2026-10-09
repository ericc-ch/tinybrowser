//! Shared JS world for the renderer.

use std::cell::{Cell, Ref, RefCell, RefMut};
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};

use blitz_traits::node_id::NodeId as BlitzNodeId;
/// Blitz tree ids, re-exported for helper signatures across the bindings.
pub(crate) use blitz_traits::node_id::NodeId as BlitzId;
use rquickjs::{Object, Persistent, Value, class::Trace, function::Function};
use url::Url;

use crate::dom_string::DomString;

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
    pub(crate) realm_contexts: Vec<usize>,
    pub(crate) attributes: AttributeRegistry,
    pub(crate) observers: super::observers::MutationObservers,
    pub(crate) reactions: super::reactions::CustomElementReactions,
    pub(crate) private_slots: Option<PrivateSlots>,
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
    /// `template` element → template contents fragment, possibly in another
    /// document after `adoptNode`
    /// (<https://html.spec.whatwg.org/multipage/scripting.html#the-template-element>).
    template_contents: HashMap<NodeId, NodeId>,
    /// Template-contents fragment → host `template` element.
    fragment_hosts: HashMap<NodeId, NodeId>,
    /// The realm that created each node. Adoption changes the node document
    /// but not this realm
    /// (<https://dom.spec.whatwg.org/#concept-node-adopt>,
    /// <https://github.com/whatwg/dom/issues/977>).
    creation_realms: HashMap<NodeId, Weak<RefCell<World>>>,
    /// Adopted node ids follow the destination copy so live collections still
    /// rooted at the pre-adopt handle walk the live tree
    /// (<https://dom.spec.whatwg.org/#concept-node-adopt>).
    adopted_ids: HashMap<NodeId, NodeId>,
    /// `WebDriver` element ids, allocated across every world and frame so a
    /// reference cannot alias between browsing contexts.
    next_remote: u64,
    /// `TextDecoder` session ids, allocated across every world and frame so
    /// a freed id can never alias a live session, even if a finalizer runs
    /// under another realm.
    next_decoder: u64,
}

pub(crate) struct PrivateSlots {
    pub(crate) factory: Persistent<Function<'static>>,
    pub(crate) realms: HashSet<usize>,
}

impl RealmRegistry {
    /// The next process-unique `WebDriver` element id.
    pub(crate) fn allocate_remote(&mut self) -> u64 {
        self.next_remote = self.next_remote.saturating_add(1);
        self.next_remote
    }

    /// The next process-unique `TextDecoder` session id.
    pub(crate) fn allocate_decoder(&mut self) -> u64 {
        self.next_decoder = self.next_decoder.saturating_add(1);
        self.next_decoder
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
        self.attributes.forget_document(id);
        self.documents.remove(&id);
        self.wrappers.retain(|node, _| node.document_id() != id);
        self.frame_documents
            .retain(|_, document| document.document_id() != id);
        self.template_contents.retain(|template, contents| {
            template.document_id() != id && contents.document_id() != id
        });
        self.fragment_hosts
            .retain(|fragment, host| fragment.document_id() != id && host.document_id() != id);
        self.creation_realms
            .retain(|node, _| node.document_id() != id);
        self.adopted_ids
            .retain(|from, to| from.document_id() != id && to.document_id() != id);
    }

    /// The shared wrapper cache entry for `id`, when one exists.
    pub(crate) fn wrapper(&self, id: NodeId) -> Option<Persistent<Value<'static>>> {
        self.wrappers.get(&id).cloned()
    }

    /// Publishes the wrapper cache entry for `id`.
    pub(crate) fn intern_wrapper(&mut self, id: NodeId, value: Persistent<Value<'static>>) {
        self.wrappers.insert(id, value);
    }

    /// Moves the shared wrapper cache entry from `from` to `to`.
    ///
    /// Adoption copies the node into another tree and retargets the existing
    /// wrapper so the caller's object is the adopted node
    /// (<https://dom.spec.whatwg.org/#concept-node-adopt>).
    pub(crate) fn rekey_wrapper(&mut self, from: NodeId, to: NodeId) {
        if from != to {
            self.adopted_ids.insert(from, to);
        }
        if let Some(value) = self.wrappers.remove(&from) {
            self.wrappers.insert(to, value);
        }
        if let Some(contents) = self.template_contents.remove(&from) {
            self.template_contents.insert(to, contents);
            if let Some(host) = self.fragment_hosts.get_mut(&contents) {
                *host = to;
            }
        }
        if let Some(host) = self.fragment_hosts.remove(&from) {
            self.fragment_hosts.insert(to, host);
            self.template_contents.insert(host, to);
        }
        if let Some(world) = self.creation_realms.remove(&from) {
            self.creation_realms.insert(to, world);
        }
    }

    /// Records the realm that created `id` the first time it is seen.
    pub(crate) fn stamp_creation_realm(&mut self, id: NodeId, world: &Rc<RefCell<World>>) {
        self.creation_realms
            .entry(id)
            .or_insert_with(|| Rc::downgrade(world));
    }

    /// The realm that created `id`, while that realm is alive.
    pub(crate) fn creation_realm(&self, id: NodeId) -> Option<Rc<RefCell<World>>> {
        self.creation_realms.get(&id).and_then(Weak::upgrade)
    }

    /// The current id of a node that may have been adopted since `id` was
    /// captured.
    pub(crate) fn live_node_id(&self, mut id: NodeId) -> NodeId {
        let mut seen = HashSet::new();
        while let Some(&next) = self.adopted_ids.get(&id) {
            if !seen.insert(id) {
                break;
            }
            id = next;
        }
        id
    }

    pub(crate) fn template_contents(&self, template: NodeId) -> Option<NodeId> {
        self.template_contents.get(&template).copied()
    }

    pub(crate) fn set_template_contents(&mut self, template: NodeId, fragment: NodeId) {
        if let Some(previous) = self.template_contents.insert(template, fragment) {
            self.fragment_hosts.remove(&previous);
        }
        self.fragment_hosts.insert(fragment, template);
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
        super::bindings::forget_registry_contexts(&self.realm_contexts);
        self.realm_contexts.clear();
        self.attributes.clear();
        self.reactions.clear();
        self.private_slots = None;
        self.documents.clear();
        self.frames.clear();
        self.wrappers.clear();
        self.frame_documents.clear();
        self.creation_realms.clear();
        self.adopted_ids.clear();
    }

    fn budget(&self) -> Rc<RefCell<ResourceBudget>> {
        Rc::clone(&self.budget)
    }
}

/// A child frame navigation queued from inside a script, applied after it
/// stops: the `iframe`'s `src` changed (or the element just connected).
/// Where a queued navigation lands.
pub(crate) enum NavigationTarget {
    /// The child frame whose container is this node.
    Container(NodeId),
    /// The frame that queued the navigation; used by form submission to the
    /// form's own frame (`_self`, `_top`, `_parent`).
    SelfFrame,
}

pub(crate) struct FrameNavigation {
    /// The frame to navigate.
    pub(crate) target: NavigationTarget,
    /// The spec to navigate to, absolute or resolvable against the queuing
    /// document.
    pub(crate) spec: String,
    /// HTTP method; `GET` for every navigation but a form submission.
    pub(crate) method: String,
    /// Request body, empty for a GET.
    pub(crate) body: Vec<u8>,
    /// `Content-Type` for `body`, when there is one.
    pub(crate) content_type: Option<String>,
}

impl FrameNavigation {
    /// A GET navigation, the shape of every navigation but a form submission.
    pub(crate) fn get(target: NavigationTarget, spec: String) -> Self {
        Self {
            target,
            spec,
            method: "GET".to_owned(),
            body: Vec::new(),
            content_type: None,
        }
    }
}

pub(crate) enum DocumentStreamCommand {
    Open,
    Write(String),
    Close,
}

/// One node in one document.
///
/// Blitz ids are per-tree: two documents can issue the same id. Document
/// identity rides along so wrapper caches and event maps stay sound across
/// documents. Replaces the arena handle: same name, same
/// [`NodeId::document_id`] accessor, new payload.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct NodeId {
    /// The document store id owning the tree.
    pub document: u32,
    /// The node inside that document's tree.
    pub node: BlitzNodeId,
}

impl NodeId {
    /// The document store id owning the tree.
    pub(crate) fn document_id(self) -> u32 {
        self.document
    }
}

impl std::fmt::Debug for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NodeId({}.{})", self.document, self.node.as_u64())
    }
}

impl PartialOrd for NodeId {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for NodeId {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.document, self.node.as_u64()).cmp(&(other.document, other.node.as_u64()))
    }
}

/// Whether `node` is connected: its ancestor chain reaches the document root.
pub(crate) fn is_connected(base: &blitz_dom::BaseDocument, id: BlitzNodeId) -> bool {
    let root = base.root_node().id;
    let mut current = Some(id);
    while let Some(node_id) = current {
        if node_id == root {
            return true;
        }
        current = base.get_node(node_id).and_then(|node| node.parent);
    }
    false
}

/// Whether `node` is an HTML `iframe` element.
pub(crate) fn is_iframe_element(base: &blitz_dom::BaseDocument, id: BlitzNodeId) -> bool {
    base.get_node(id).is_some_and(|node| {
        let blitz_dom::NodeData::Element(element) = &node.data else {
            return false;
        };
        element.name.ns == html_namespace()
            && element.name.local == markup5ever::local_name!("iframe")
    })
}

/// Wrapped children of `node` in `document`, in tree order.
pub(crate) fn child_ids(
    base: &blitz_dom::BaseDocument,
    document: u32,
    id: BlitzNodeId,
) -> Vec<NodeId> {
    base.get_node(id).map_or_else(Vec::new, |node| {
        node.children
            .iter()
            .map(|child| NodeId {
                document,
                node: *child,
            })
            .collect()
    })
}

/// The value of the attribute named `name` on `id`, when it is an element.
pub(crate) fn attr<'a>(
    base: &'a blitz_dom::BaseDocument,
    id: BlitzNodeId,
    name: &str,
) -> Option<&'a str> {
    base.get_node(id)?
        .data
        .downcast_element()?
        .attr(markup5ever::LocalName::from(name))
}

/// The value of the first attribute whose qualified name is `qualified`.
///
/// <https://dom.spec.whatwg.org/#concept-element-attributes-get-by-name>
pub(crate) fn attr_by_qualified_name<'a>(
    base: &'a blitz_dom::BaseDocument,
    id: BlitzNodeId,
    qualified: &str,
) -> Option<&'a str> {
    base.get_node(id)?
        .data
        .downcast_element()?
        .attrs
        .iter()
        .find(|attribute| crate::names::qualified_name_eq(&attribute.name, qualified))
        .map(|attribute| attribute.value.as_str())
}

/// The HTML namespace all HTML elements live in.
pub(crate) fn html_namespace() -> markup5ever::Namespace {
    markup5ever::ns!(html)
}

/// The SVG namespace.
pub(crate) fn svg_namespace() -> markup5ever::Namespace {
    markup5ever::ns!(svg)
}

/// The `MathML` namespace.
pub(crate) fn mathml_namespace() -> markup5ever::Namespace {
    markup5ever::ns!(mathml)
}

/// Whether `data` is an HTML element named `local`.
pub(crate) fn is_html_tag(data: Option<&blitz_dom::NodeData>, local: &str) -> bool {
    match data {
        Some(blitz_dom::NodeData::Element(element)) => {
            element.name.ns == html_namespace() && element.name.local.as_ref() == local
        }
        _ => false,
    }
}

/// Whether `node` is an HTML element named `local`.
pub(crate) fn is_html_element(
    base: &blitz_dom::BaseDocument,
    id: BlitzNodeId,
    local: &str,
) -> bool {
    base.get_node(id)
        .is_some_and(|node| is_html_tag(Some(&node.data), local))
}

/// First element in tree order whose `id` content attribute is `id`.
/// An empty ID matches nothing
/// (<https://dom.spec.whatwg.org/#concept-id>,
/// <https://dom.spec.whatwg.org/#dom-nonelementparentnode-getelementbyid>).
pub(crate) fn first_element_with_id(
    base: &blitz_dom::BaseDocument,
    document: u32,
    id: &str,
) -> Option<NodeId> {
    if id.is_empty() {
        return None;
    }
    let mut stack = vec![base.root_node().id];
    while let Some(current) = stack.pop() {
        let Some(node) = base.get_node(current) else {
            continue;
        };
        if node.data.downcast_element().is_some_and(|element| {
            element
                .attr(markup5ever::LocalName::from("id"))
                .is_some_and(|value| value == id)
        }) {
            return Some(NodeId {
                document,
                node: current,
            });
        }
        stack.extend(node.children.iter().rev().copied());
    }
    None
}

/// [Reset the form owner](https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#reset-the-form-owner)
/// lookup used by `form` IDL getters: a `form` content attribute names the
/// first matching `form` in tree order when the control is in a document;
/// otherwise the nearest ancestor `form` is used.
///
/// If the attribute is set but does not identify a `form`, this still falls
/// through to the ancestor (the spec's `new-form` steps). Chromium leaves the
/// owner null in that case; the spec algorithm is the one that applies here.
pub(crate) fn form_owner_of(
    base: &blitz_dom::BaseDocument,
    document: u32,
    node: BlitzId,
) -> Option<NodeId> {
    if let Some(form_id) = attr(base, node, "form")
        && is_connected(base, node)
        && let Some(found) = first_element_with_id(base, document, form_id)
        && is_html_element(base, found.node, "form")
    {
        return Some(found);
    }
    let mut cursor = base.get_node(node).and_then(|target| target.parent);
    while let Some(current) = cursor {
        if is_html_element(base, current, "form") {
            return Some(NodeId {
                document,
                node: current,
            });
        }
        cursor = base.get_node(current).and_then(|target| target.parent);
    }
    None
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
    pub attribute_filter: Option<Vec<Vec<u16>>>,
}

#[derive(Clone)]
pub(crate) struct Observation {
    pub target: NodeReference,
    pub options: ObserverOptions,
    pub order: u64,
}

/// Both native representations implementing Node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum NodeReference {
    Tree(NodeId),
    Attribute { scope: NodeId, id: u64 },
}

impl NodeReference {
    pub(crate) fn tree(self) -> Option<NodeId> {
        match self {
            Self::Tree(id) => Some(id),
            Self::Attribute { .. } => None,
        }
    }

    pub(crate) fn scope(self) -> NodeId {
        match self {
            Self::Tree(id) | Self::Attribute { scope: id, .. } => id,
        }
    }
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
    /// Not traced: a `DomString` holds no JavaScript value.
    #[qjs(skip_trace)]
    pub old_value: Option<crate::dom_string::DomString>,
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
    pub owner: FrameId,
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
    /// Attr registry identity: immutable creation scope and agent-issued id.
    Attribute {
        scope: NodeId,
        id: u64,
    },
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
    /// `Node.childNodes`.
    ChildNodes,
    /// `Document.scripts`.
    Scripts,
}

pub(crate) struct WeakReferences {
    pub(crate) constructor: Persistent<rquickjs::function::Constructor<'static>>,
    pub(crate) deref: Persistent<Function<'static>>,
}

pub(crate) struct World {
    /// The handles every frame of this renderer process shares: trees,
    /// registry and wrapper cache, ports, JS heap, wake handle, and stop flag.
    pub(crate) runtime: FrameRuntime,
    /// The frame this realm belongs to.
    frame: FrameId,
    /// Page-scoped scripts registered through CDP to run in every new realm
    /// of this tab, shared by every frame's world, with their identifiers
    /// (<https://chromedevtools.github.io/devtools-protocol/tot/Page/#method-addScriptToEvaluateOnNewDocument>).
    pub(crate) init_scripts: Rc<RefCell<Vec<(u64, String)>>>,
    /// The frame's persistent viewport size in device pixels, readable
    /// without an active document.
    pub(crate) viewport_size: Cell<(u32, u32)>,
    /// Frames whose browsing context was registered inside a script but whose
    /// realm cannot be created until JS execution has stopped.
    pending_frames: Vec<(FrameId, NodeId)>,
    /// Child frames this realm created for the engine to adopt.
    new_frames: Vec<(FrameId, NodeId, Document)>,
    /// The active document of the frame this realm belongs to.
    document: Option<u32>,
    /// Document ids this realm created. Its logs feed every realm's
    /// observers through the union drain.
    owned: HashSet<u32>,
    pub document_url: Url,
    /// Document referrer policy from a `meta name=referrer` insertion
    /// (<https://html.spec.whatwg.org/multipage/semantics.html#meta-referrer>).
    pub(crate) referrer_policy: String,
    pub(crate) history: crate::protocol::HistorySnapshot,
    pub(crate) pending_history: Vec<crate::protocol::RendererEvent>,
    pub pending_cancels: Vec<i32>,
    pub pending_fetch_cancels: Vec<i32>,
    pub pending_html_writes: Vec<String>,
    frame_navigations: Vec<FrameNavigation>,
    /// Connected `<img>` elements whose `src` changed inside script.
    image_updates: Vec<NodeId>,
    /// Files a script set on an `input[type=file]` through `input.files`, so
    /// the form entry list reads them without depending on JS wrapper identity
    /// (<https://html.spec.whatwg.org/multipage/input.html#dom-input-files>).
    input_files: HashMap<NodeId, Vec<Persistent<Value<'static>>>>,
    /// Streaming `TextDecoder` sessions, keyed by platform id. The decoder
    /// object buffers partial sequences internally across `decode()` calls,
    /// exactly as the Encoding Standard's streaming decoder does; the entry
    /// dies with the realm, and `FinalizationRegistry` frees it earlier when
    /// the engine runs the callback
    /// (<https://encoding.spec.whatwg.org/#dom-textdecoder-decode>).
    pub(crate) decoders: HashMap<u64, DecoderSession>,
    document_stream: Vec<DocumentStreamCommand>,
    object_urls: HashMap<String, ObjectUrlEntry>,
    budget: Rc<RefCell<ResourceBudget>>,
    next_object_url: u64,
    pub parser_active: bool,
    pub current_script: Option<NodeId>,
    /// Existing elements being synchronously upgraded by a custom-element
    /// constructor. `HTMLElement()` consumes the top candidate.
    pub(crate) custom_construction: Vec<NodeId>,
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
    /// Text selection per control `(start, end, direction code)`: the
    /// selection APIs clamp to the control's value length on read.
    selections: HashMap<NodeId, (u32, u32, u8)>,
    /// Elements whose `click()` is running, so a nested `click()` returns
    /// (<https://html.spec.whatwg.org/multipage/interaction.html#dom-click>).
    clicks_in_progress: HashSet<NodeId>,
    brands: HashMap<String, Persistent<Object<'static>>>,
    pub(crate) bridge: Option<Persistent<Object<'static>>>,
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
    /// Stable `WebDriver` element ids for nodes, and the reverse lookup.
    remote_ids: HashMap<NodeId, u64>,
    remote_nodes: HashMap<u64, NodeId>,
    /// Registered `MutationObserver`s, keyed by their platform id.
    /// Pristine intrinsics captured at install, before page script runs.
    /// `WebIDL` conversions and scheduling must use these, never
    /// `ctx.globals()`: a page that replaces `String`/`Number`/`Boolean` (or
    /// deletes `queueMicrotask`) must not change conversion behavior, which
    /// follows the realm's original intrinsics.
    pub(crate) pristine_number: Option<Persistent<Function<'static>>>,
    pub(crate) pristine_boolean: Option<Persistent<Function<'static>>>,
    pub(crate) pristine_reflect_set: Option<Persistent<Function<'static>>>,
    pub(crate) pristine_queue_microtask: Option<Persistent<Function<'static>>>,
    pub(crate) weak_references: Option<WeakReferences>,
    /// The realm's own mutation-delivery entry point, so scheduling never
    /// depends on a page-deletable global.
    pub(crate) deliver_mutations_fn: Option<Persistent<Function<'static>>>,
    /// Decoded `<img>` dimensions for script geometry (`naturalWidth/Height`).
    /// Blitz owns the pixels; this only records the settled size.
    pub(crate) images: HashMap<NodeId, (u32, u32)>,
    /// `<img>` elements whose current request has not finished, including a
    /// `src` mutation waiting for `update the image data`
    /// (<https://html.spec.whatwg.org/multipage/embedded-content.html#dom-img-complete>).
    pub(crate) image_loading: HashSet<NodeId>,
    /// Selected URL for each `<img>` current request
    /// (<https://html.spec.whatwg.org/multipage/embedded-content.html#dom-img-currentsrc>).
    pub(crate) image_current_src: HashMap<NodeId, String>,
    /// Current request is broken and there is no pending request
    /// (<https://html.spec.whatwg.org/multipage/images.html#img-error>).
    pub(crate) image_broken: HashSet<NodeId>,
}

/// One streaming `TextDecoder` session: the decoder plus what recreates it.
/// A `last=true` call ends the decoder (`encoding_rs` panics on reuse), so
/// every non-streaming `decode()` replaces it with a fresh one. `carry`
/// holds input bytes past a fatal error for the next call: the error itself
/// is consumed, but the queue behind it is not.
pub(crate) struct DecoderSession {
    pub(crate) decoder: encoding_rs::Decoder,
    pub(crate) encoding: &'static encoding_rs::Encoding,
    pub(crate) ignore_bom: bool,
    pub(crate) carry: Vec<u8>,
}

impl DecoderSession {
    pub(crate) fn fresh(encoding: &'static encoding_rs::Encoding, ignore_bom: bool) -> Self {
        let decoder = if ignore_bom {
            encoding.new_decoder_without_bom_handling()
        } else {
            encoding.new_decoder_with_bom_removal()
        };
        Self {
            decoder,
            encoding,
            ignore_bom,
            carry: Vec::new(),
        }
    }
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
    pub(crate) fn new(
        document_url: Url,
        frame: FrameId,
        runtime: &FrameRuntime,
        init_scripts: Rc<RefCell<Vec<(u64, String)>>>,
    ) -> Self {
        Self {
            runtime: runtime.clone(),
            frame,
            init_scripts,
            viewport_size: Cell::new(crate::engine::DEFAULT_VIEWPORT),
            pending_frames: Vec::new(),
            new_frames: Vec::new(),
            document: None,
            owned: HashSet::new(),
            document_url,
            referrer_policy: String::new(),
            history: crate::protocol::HistorySnapshot::default(),
            pending_history: Vec::new(),
            pending_cancels: Vec::new(),
            pending_fetch_cancels: Vec::new(),
            pending_html_writes: Vec::new(),
            frame_navigations: Vec::new(),
            image_updates: Vec::new(),
            input_files: HashMap::new(),
            decoders: HashMap::new(),
            document_stream: Vec::new(),
            object_urls: HashMap::new(),
            budget: runtime.registry.borrow().budget(),
            next_object_url: 0,
            parser_active: false,
            current_script: None,
            custom_construction: Vec::new(),
            window: None,
            listeners: HashMap::new(),
            standalone_targets: HashMap::new(),
            next_standalone_target: 0,
            wrappers: HashMap::new(),
            implementations: HashMap::new(),
            active_elements: HashMap::new(),
            selections: HashMap::new(),
            clicks_in_progress: HashSet::new(),
            brands: HashMap::new(),
            bridge: None,
            handler_attributes: HashMap::new(),
            cleared_handlers: HashSet::new(),
            remote_ids: HashMap::new(),
            remote_nodes: HashMap::new(),
            pristine_number: None,
            pristine_boolean: None,
            pristine_reflect_set: None,
            pristine_queue_microtask: None,
            weak_references: None,
            deliver_mutations_fn: None,
            images: HashMap::new(),
            image_loading: HashSet::new(),
            image_current_src: HashMap::new(),
            image_broken: HashSet::new(),
        }
    }

    pub(crate) fn clear_observers(&mut self) {
        self.runtime
            .registry
            .borrow_mut()
            .observers
            .forget_frame(self.frame, &mut self.runtime.documents.borrow_mut());
    }

    /// Installs `parsed` as the active document and returns its id.
    pub(crate) fn replace_document(&mut self, parsed: Parsed) -> u32 {
        self.drop_active_document();
        let id = self.runtime.documents.borrow_mut().insert(parsed);
        self.document = Some(id);
        self.owned.insert(id);
        self.referrer_policy.clear();
        self.current_script = None;
        self.custom_construction.clear();
        self.listeners.clear();
        self.wrappers.clear();
        self.implementations.clear();
        self.active_elements.clear();
        self.selections.clear();
        self.frame_navigations.clear();
        self.image_updates.clear();
        self.clear_images();
        self.remote_ids.clear();
        self.remote_nodes.clear();
        // Sessions die with the realm: a navigation must not inherit a
        // half-fed decoder.
        self.decoders.clear();
        // Navigation replaces the document's element handlers with it, but the
        // realm keeps its window object, so window-scoped handlers survive:
        // the old document's `onload` must not fire in the new document
        // (<https://html.spec.whatwg.org/multipage/webappapis.html#event-handlers>).
        self.handler_attributes
            .retain(|(node, _), _| node.is_none());
        self.cleared_handlers.retain(|(node, _)| node.is_none());
        let pending = self.take_document_stream();
        drop(pending);
        // A new realm owns fresh observers; navigation drops the old ones.
        self.clear_observers();
        id
    }

    /// Installs the frame's active document without clearing realm caches.
    /// Returns the document id, reusing the parsed document's id when it
    /// carries one (take-and-reinsert cycles keep wrapper identity).
    pub(crate) fn set_document(&mut self, parsed: Parsed) -> u32 {
        let id = self.runtime.documents.borrow_mut().insert(parsed);
        if self.document != Some(id) {
            self.drop_active_document();
            self.document = Some(id);
            self.owned.insert(id);
        }
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
                .retain(|(node, _), _| node.is_some_and(|node| node.document_id() != id));
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
        let node = parsed.document.base.root_node().id;
        let document = self.runtime.documents.borrow_mut().insert(parsed);
        self.owned.insert(document);
        NodeId { document, node }
    }

    pub(crate) fn frame_document(&self, container: NodeId) -> Option<NodeId> {
        self.runtime.registry.borrow().frame_document(container)
    }

    /// The frame this realm belongs to.
    pub(crate) fn frame(&self) -> FrameId {
        self.frame
    }

    /// Whether this realm's frame is still attached to its document. A
    /// removed iframe's frame may still sit in the tree until the engine
    /// reconciles, so the container's connectedness decides.
    pub(crate) fn is_attached(&self) -> bool {
        if self.frame == FrameId::MAIN {
            return true;
        }
        let container = self.runtime.shared.borrow().tree.container(self.frame);
        let Some(container) = container else {
            return false;
        };
        self.owner_world(container).is_some_and(|owner| {
            owner
                .borrow()
                .main_document()
                .is_some_and(|parsed| is_connected(&parsed.document.base, container.node))
        })
    }

    /// The renderer-process state shared by every realm.
    pub(crate) fn shared(&self) -> SharedHandle {
        Rc::clone(&self.runtime.shared)
    }

    /// Registers a browsing context for every connected `iframe` in this
    /// frame's document that does not have one yet.
    ///
    /// Safe to call while a script runs: it allocates frame identities and
    /// tree entries only. The document and its realm are created later, by
    /// [`World::materialize_frames`], because `QuickJS` forbids entering a new
    /// realm while another realm executes.
    pub(crate) fn register_pending_frames(&mut self, before: Option<NodeId>) -> Vec<NodeId> {
        let mut created = Vec::new();
        let containers = self.iframe_containers_before(before);
        for container in containers {
            if self.register_frame_for_container(container) {
                created.push(container);
            }
        }
        created
    }

    /// Creates a browsing context for one connected `iframe` if it does not
    /// already have one
    /// (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#the-iframe-element:html-element-insertion-steps>).
    ///
    /// Allocates the frame identity only. The nested document is created
    /// later, by [`World::materialize_frames`].
    pub(crate) fn register_frame_for_container(&mut self, container: NodeId) -> bool {
        if self
            .runtime
            .shared
            .borrow()
            .tree
            .frame_for_container(container)
            .is_some()
        {
            return false;
        }
        let eligible = self
            .with_document(container, |parsed| {
                let base = &parsed.document.base;
                is_iframe_element(base, container.node) && is_connected(base, container.node)
            })
            .unwrap_or(false);
        if !eligible || self.runtime.shared.borrow().tree.len() >= MAX_FRAMES {
            return false;
        }
        let name = self
            .with_document(container, |parsed| {
                attr(&parsed.document.base, container.node, "name")
                    .unwrap_or("")
                    .to_owned()
            })
            .unwrap_or_default();
        let frame = {
            let mut shared = self.runtime.shared.borrow_mut();
            let frame = shared.allocate_frame();
            shared.tree.add(self.frame, frame, container);
            if !name.is_empty() {
                shared.tree.set_name(frame, name);
            }
            frame
        };
        self.pending_frames.push((frame, container));
        true
    }

    /// Creates the documents and realms for every registered frame.
    ///
    /// Borrow discipline: no `World` borrow is held while a frame document
    /// loads. Loading runs the parser, which delivers mutations, which fans
    /// out across live worlds; holding this world's borrow across that
    /// would alias the fan-out borrows and panic the renderer.
    pub(crate) fn adopt_pending_frames(
        world_rc: &Rc<RefCell<World>>,
        before: Option<NodeId>,
    ) -> Vec<NodeId> {
        let mut created = world_rc.borrow_mut().register_pending_frames(before);
        created.extend(Self::materialize_frames(world_rc));
        created
    }

    /// Creates the initial `about:blank` document for one connected `iframe`
    /// so `contentDocument` is available in the inserting script
    /// (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#the-iframe-element:html-element-insertion-steps>).
    ///
    /// The nested realm is deferred: `QuickJS` cannot enter a new context while
    /// the parent script still runs.
    pub(crate) fn materialize_iframe(world_rc: &Rc<RefCell<World>>, container: NodeId) -> bool {
        world_rc
            .borrow_mut()
            .register_frame_for_container(container);
        let pending = {
            let mut world = world_rc.borrow_mut();
            world
                .pending_frames
                .iter()
                .position(|(_, existing)| *existing == container)
                .map(|index| world.pending_frames.remove(index))
        };
        let Some((frame, container)) = pending else {
            return world_rc
                .borrow()
                .runtime
                .registry
                .borrow()
                .frame_document(container)
                .is_some();
        };
        let (runtime, base_url, init_scripts, viewport) = {
            let world = world_rc.borrow();
            (
                world.runtime.clone(),
                world.document_url.clone(),
                Rc::clone(&world.init_scripts),
                world.viewport_size.get(),
            )
        };
        let mut document = Document::with_shared(frame, &runtime, &init_scripts);
        document.set_viewport_size(viewport);
        document.load_about_blank_tree(Some(base_url.as_str()));
        if let Some(root) = document.document_root() {
            world_rc
                .borrow()
                .registry()
                .borrow_mut()
                .set_frame_document(container, root);
        }
        world_rc
            .borrow_mut()
            .new_frames
            .push((frame, container, document));
        true
    }

    /// Loads every registered frame document without holding the world
    /// borrow: pending frames and the base URL are taken first, documents
    /// load unborrowed, and only then are the results published.
    fn materialize_frames(world_rc: &Rc<RefCell<World>>) -> Vec<NodeId> {
        let (pending, runtime, base_url, init_scripts, viewport) = {
            let mut world = world_rc.borrow_mut();
            (
                std::mem::take(&mut world.pending_frames),
                world.runtime.clone(),
                world.document_url.clone(),
                Rc::clone(&world.init_scripts),
                world.viewport_size.get(),
            )
        };
        let mut loaded = Vec::with_capacity(pending.len());
        let mut created = Vec::with_capacity(pending.len());
        for (frame, container) in pending {
            let mut document = Document::with_shared(frame, &runtime, &init_scripts);
            // Frames created from parsed markup share the tab's viewport too.
            // This path runs before page script, so the realm can exist for
            // `contentWindow` in the following classic script
            // (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#the-iframe-element:html-element-insertion-steps>).
            document.set_viewport_size(viewport);
            document.load_about_blank(Some(base_url.as_str()));
            loaded.push((frame, container, document));
            created.push(container);
        }
        {
            let mut world = world_rc.borrow_mut();
            for (frame, container, document) in loaded {
                world.new_frames.push((frame, container, document));
            }
        }
        created
    }

    /// Takes the child frames this realm created for the engine to adopt.
    pub(crate) fn take_new_frames(&mut self) -> Vec<(FrameId, NodeId, Document)> {
        std::mem::take(&mut self.new_frames)
    }

    /// Connected `iframe`s in tree order that precede `before`, or every
    /// connected iframe when `before` is `None`.
    fn iframe_containers_before(&self, before: Option<NodeId>) -> Vec<NodeId> {
        self.with_main_document(|parsed| {
            let base = &parsed.document.base;
            let document = parsed.id;
            let stop = before
                .filter(|node| node.document == document)
                .map(|node| node.node);
            let mut containers = Vec::new();
            let mut stack = vec![base.root_node().id];
            while let Some(id) = stack.pop() {
                if stop == Some(id) {
                    break;
                }
                if is_iframe_element(base, id) && is_connected(base, id) {
                    containers.push(NodeId { document, node: id });
                }
                if let Some(node) = base.get_node(id) {
                    stack.extend(node.children.iter().rev().copied());
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

    /// Destroys the child navigable of each `iframe` container.
    ///
    /// HTML iframe [removing steps] run destroy-a-child-navigable and do not
    /// fire `pagehide`/`unload` synchronously
    /// (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#the-iframe-element:html-element-removing-steps>).
    pub(crate) fn destroy_child_navigables(&self, containers: &[NodeId]) {
        let mut shared = self.runtime.shared.borrow_mut();
        for container in containers {
            if let Some(frame) = shared.tree.frame_for_container(*container) {
                shared.tree.remove(frame);
            }
        }
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

    /// `Storage.getItem(key)` for the `"local"` or `"session"` area.
    pub(crate) fn storage_get(&self, kind: &str, key: &str) -> Option<String> {
        let origin = self.storage_origin()?;
        self.runtime
            .services
            .storage_get(Self::storage_kind(kind), &origin, key)
    }

    /// The keys of one storage area, in the area's iteration order.
    pub(crate) fn storage_keys(&self, kind: &str) -> Vec<String> {
        let Some(origin) = self.storage_origin() else {
            return Vec::new();
        };
        self.runtime
            .services
            .storage_keys(Self::storage_kind(kind), &origin)
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
        let change = self.runtime.services.storage_set(
            kind,
            &origin,
            self.document_url.as_str(),
            key,
            value,
            self.frame,
        )?;
        if let Some(change) = &change {
            self.queue_storage_event(kind, &origin, change);
        }
        Ok(change)
    }

    /// `Storage.removeItem(key)`; `None` means the key was absent.
    pub(crate) fn storage_remove(&self, kind: &str, key: &str) -> Option<StorageChange> {
        let origin = self.storage_origin()?;
        let kind = Self::storage_kind(kind);
        let change = self.runtime.services.storage_remove(
            kind,
            &origin,
            self.document_url.as_str(),
            key,
            self.frame,
        );
        if let Some(change) = &change {
            self.queue_storage_event(kind, &origin, change);
        }
        change
    }

    /// `Storage.clear()`; `None` means the area was empty.
    pub(crate) fn storage_clear(&self, kind: &str) -> Option<StorageChange> {
        let origin = self.storage_origin()?;
        let kind = Self::storage_kind(kind);
        let change = self.runtime.services.storage_clear(
            kind,
            &origin,
            self.document_url.as_str(),
            self.frame,
        );
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
        let document = self.document?;
        self.main_document().map(|parsed| NodeId {
            document,
            node: parsed.document.base.root_node().id,
        })
    }

    pub(crate) fn queue_frame_navigation(&mut self, navigation: FrameNavigation) {
        self.frame_navigations.push(navigation);
    }

    pub(crate) fn take_frame_navigations(&mut self) -> Vec<FrameNavigation> {
        std::mem::take(&mut self.frame_navigations)
    }

    pub(crate) fn queue_image_update(&mut self, element: NodeId) {
        // Keep `currentSrc` and decoded pixels until the new request starts.
        // Mark unavailable now so `complete` is false while the update waits
        // (<https://html.spec.whatwg.org/multipage/images.html#updating-the-image-data>,
        // <https://html.spec.whatwg.org/multipage/embedded-content.html#dom-img-complete>).
        self.image_loading.insert(element);
        if !self.image_updates.contains(&element) {
            self.image_updates.push(element);
        }
    }

    pub(crate) fn take_image_updates(&mut self) -> Vec<NodeId> {
        std::mem::take(&mut self.image_updates)
    }

    /// Records the file list a script assigned to a `type=file` input.
    pub(crate) fn set_input_files(&mut self, id: NodeId, files: Vec<Persistent<Value<'static>>>) {
        self.input_files.insert(id, files);
    }

    /// The file list a script assigned to a `type=file` input, if any.
    pub(crate) fn input_files(&self, id: NodeId) -> Option<&[Persistent<Value<'static>>]> {
        self.input_files.get(&id).map(Vec::as_slice)
    }

    /// Starts a fetch for `url`. If the current request is still available,
    /// keep its pixels and `currentSrc` until this request commits
    /// (<https://html.spec.whatwg.org/multipage/images.html#updating-the-image-data>).
    pub(crate) fn begin_image(&mut self, element: NodeId, url: String) {
        self.image_loading.insert(element);
        self.image_broken.remove(&element);
        if !self.images.contains_key(&element) {
            self.image_current_src.insert(element, url);
        }
    }

    /// Records Blitz's decoded dimensions for `element`'s current request.
    pub(crate) fn store_image_dims(
        &mut self,
        element: NodeId,
        width: u32,
        height: u32,
        url: String,
    ) {
        self.image_loading.remove(&element);
        self.image_broken.remove(&element);
        self.images.remove(&element);
        self.image_current_src.insert(element, url);
        self.images.insert(element, (width, height));
    }

    /// The current request finished without usable pixels. `url` is the
    /// selected source, or the selected source string when URL parsing failed
    /// (<https://html.spec.whatwg.org/multipage/images.html#updating-the-image-data>).
    pub(crate) fn fail_image(&mut self, element: NodeId, url: String) {
        self.image_loading.remove(&element);
        self.images.remove(&element);
        self.image_current_src.insert(element, url);
        self.image_broken.insert(element);
    }

    pub(crate) fn forget_image(&mut self, element: NodeId) {
        self.image_loading.remove(&element);
        self.image_current_src.remove(&element);
        self.image_broken.remove(&element);
        self.images.remove(&element);
    }

    pub(crate) fn clear_images(&mut self) {
        self.images.clear();
        self.image_loading.clear();
        self.image_current_src.clear();
        self.image_broken.clear();
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
        if let EventTargetKey::Attribute { id, .. } = target {
            if let Some(entry) = self
                .runtime
                .registry
                .borrow_mut()
                .attributes
                .entries
                .get_mut(&id)
            {
                entry.listeners.push(listener);
            }
            return;
        }
        self.listeners.entry(target).or_default().push(listener);
    }

    /// A clone of one target's listener list, taken when dispatch invokes the
    /// target (<https://dom.spec.whatwg.org/#concept-event-listener-invoke>).
    pub(crate) fn listener_snapshot(&self, target: EventTargetKey) -> Vec<Rc<Listener>> {
        if let EventTargetKey::Attribute { id, .. } = target {
            return self
                .runtime
                .registry
                .borrow()
                .attributes
                .entries
                .get(&id)
                .map(|entry| entry.listeners.clone())
                .unwrap_or_default();
        }
        self.listeners.get(&target).cloned().unwrap_or_default()
    }

    /// Drops one listener from a target's list; the listener's `removed` flag
    /// is what a concurrent dispatch checks, so both happen together.
    pub(crate) fn remove_listener(&mut self, target: EventTargetKey, listener: &Rc<Listener>) {
        if let EventTargetKey::Attribute { id, .. } = target {
            if let Some(entry) = self
                .runtime
                .registry
                .borrow_mut()
                .attributes
                .entries
                .get_mut(&id)
            {
                entry
                    .listeners
                    .retain(|existing| !Rc::ptr_eq(existing, listener));
            }
            return;
        }
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
        self.document(id).and_then(|parsed| {
            parsed
                .document
                .base
                .get_node(id.node)
                .and_then(|node| node.parent)
                .map(|parent| NodeId {
                    document: id.document,
                    node: parent,
                })
        })
    }

    /// Whether `id` is the root document node of its tree.
    pub(crate) fn node_is_document(&self, id: NodeId) -> bool {
        self.document(id)
            .is_some_and(|parsed| parsed.document.base.root_node().id == id.node)
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
        self.clear_observers();
    }

    /// Drops the captured host primitives. Like every other JS-holding field,
    /// they must go before the realm's context does: a `Persistent` keeps its
    /// context alive, and a live context keeps the globals (which own
    /// `Rc<World>` closures) alive, so an unreleased primitive deadlocks
    /// teardown and trips `JS_FreeRuntime`'s live-object assertion.
    pub(crate) fn release_host_primitives(&mut self) {
        self.runtime
            .registry
            .borrow_mut()
            .reactions
            .forget_frame(self.frame);
        self.pristine_number = None;
        self.pristine_boolean = None;
        self.pristine_reflect_set = None;
        self.pristine_queue_microtask = None;
        self.weak_references = None;
        self.deliver_mutations_fn = None;
        self.bridge = None;
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

    /// A control's `(start, end, direction)` selection, defaulting to a
    /// collapsed caret at 0.
    pub(crate) fn selection_of(&self, id: NodeId) -> (u32, u32, u8) {
        self.selections.get(&id).copied().unwrap_or((0, 0, 0))
    }

    /// Stores a control's `(start, end, direction)` selection.
    pub(crate) fn set_selection(&mut self, id: NodeId, start: u32, end: u32, direction: u8) {
        self.selections.insert(id, (start, end, direction));
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
        // Mirror into the document so `:focus` and `:focus-within` match
        // (<https://drafts.csswg.org/selectors-4/#the-focus-pseudo>).
        if let Some(parsed) = self.runtime.documents.borrow_mut().get_mut(document) {
            match node {
                Some(focused) => {
                    parsed.document.base.set_focus_to(focused.node);
                }
                None => parsed.document.base.clear_focus(),
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
pub(super) fn match_observation(
    base: &blitz_dom::BaseDocument,
    document: u32,
    observer: &ObserverState,
    entry: &JournalEntry,
) -> Option<(usize, u64, RecordData)> {
    let (target, kind) = match entry {
        JournalEntry::ChildList { target, .. } => (*target, 0_u8),
        JournalEntry::Attributes { target, .. } => (*target, 1_u8),
        JournalEntry::CharacterData { target, .. } => (*target, 2_u8),
    };
    if target.document != document {
        return None;
    }
    let mut first_registration = None;
    let mut want_attribute_old_value = false;
    let mut want_character_data_old_value = false;
    for observation in &observer.observations {
        // `Attr` targets never match: journal records target elements and
        // `Attr` nodes are ancestors of nothing, so observing one registers
        // successfully but legitimately never fires.
        let Some(root) = observation.target.tree() else {
            continue;
        };
        if root.document != document {
            continue;
        }
        let Some(depth) = ancestor_distance(base, root.node, target.node) else {
            continue;
        };
        if depth != 0 && !observation.options.subtree {
            continue;
        }
        let enabled = match kind {
            0 => observation.options.child_list,
            1 => {
                observation.options.attributes
                    && match (&observation.options.attribute_filter, entry) {
                        // A filter only ever matches unnamespaced attributes;
                        // namespaced ones are always skipped
                        // (<https://dom.spec.whatwg.org/#queue-a-mutation-record>).
                        (
                            Some(filter),
                            JournalEntry::Attributes {
                                name, namespace, ..
                            },
                        ) => {
                            namespace.is_empty()
                                && filter
                                    .iter()
                                    .any(|wanted| wanted.iter().copied().eq(name.encode_utf16()))
                        }
                        _ => true,
                    }
            }
            _ => observation.options.character_data,
        };
        if !enabled {
            continue;
        }
        let position = (depth, observation.order);
        first_registration =
            Some(first_registration.map_or(position, |first: (usize, u64)| first.min(position)));
        want_attribute_old_value |= observation.options.attribute_old_value;
        want_character_data_old_value |= observation.options.character_data_old_value;
    }
    first_registration.map(|(depth, order)| {
        (
            depth,
            order,
            record(
                want_attribute_old_value,
                want_character_data_old_value,
                entry,
            ),
        )
    })
}

/// One recorded tree mutation, in observer-delivery shape.
///
/// Binding and parser call sites push these onto the owning document's
/// journal; Blitz only tracks a coarse changed set, so the delivery payload
/// lives here.
#[derive(Clone, Debug)]
pub(crate) enum JournalEntry {
    ChildList {
        target: NodeId,
        added: Vec<NodeId>,
        removed: Vec<NodeId>,
        previous: Option<NodeId>,
        next: Option<NodeId>,
    },
    Attributes {
        target: NodeId,
        name: String,
        namespace: String,
        old_value: Option<String>,
    },
    CharacterData {
        target: NodeId,
        old_value: DomString,
    },
}

fn record(
    want_attribute_old_value: bool,
    want_character_data_old_value: bool,
    entry: &JournalEntry,
) -> RecordData {
    match entry {
        JournalEntry::ChildList {
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
        JournalEntry::Attributes {
            target,
            name,
            namespace,
            old_value,
        } => RecordData {
            attribute_name: Some(name.clone()),
            attribute_namespace: (!namespace.is_empty()).then(|| namespace.clone()),
            old_value: want_attribute_old_value
                .then(|| old_value.clone())
                .flatten()
                .map(DomString::from),
            ..RecordData::new("attributes", Handle(*target))
        },
        JournalEntry::CharacterData { target, old_value } => RecordData {
            old_value: want_character_data_old_value.then(|| old_value.clone()),
            ..RecordData::new("characterData", Handle(*target))
        },
    }
}

fn ancestor_distance(
    base: &blitz_dom::BaseDocument,
    ancestor: BlitzNodeId,
    node: BlitzNodeId,
) -> Option<usize> {
    let mut cursor = Some(node);
    let mut depth = 0;
    while let Some(id) = cursor {
        if id == ancestor {
            return Some(depth);
        }
        cursor = base.get_node(id).and_then(|node| node.parent);
        depth += 1;
    }
    None
}

/// Identity of one `Attr` platform object.
#[derive(Clone)]
pub(crate) struct AttrState {
    pub scope: NodeId,
    pub document: NodeId,
    pub owner: Option<NodeId>,
    pub value: String,
    pub namespace: String,
    pub prefix: Option<String>,
    pub local: String,
    pub qualified: String,
}

/// Agent-owned Attr identities, attachment indexes, and weak wrappers.
/// Adoption changes the node document, never the creation scope or id
/// (<https://dom.spec.whatwg.org/#concept-element-attributes-append>).
#[derive(Default)]
pub(crate) struct AttributeRegistry {
    entries: HashMap<u64, AttrEntry>,
    attached: HashMap<(NodeId, String, String), u64>,
    next_id: u64,
}

struct AttrEntry {
    state: AttrState,
    wrapper: Option<Persistent<Value<'static>>>,
    listeners: Vec<Rc<Listener>>,
}

pub(crate) enum AttrAttachError {
    Stale,
    InUse,
    NotAnElement,
}

impl AttributeRegistry {
    pub(crate) fn create(&mut self, state: AttrState) -> Option<u64> {
        let id = self.next_id.checked_add(1)?;
        self.next_id = id;
        if let Some(owner) = state.owner {
            self.attached
                .insert((owner, state.namespace.clone(), state.local.clone()), id);
        }
        self.entries.insert(
            id,
            AttrEntry {
                state,
                wrapper: None,
                listeners: Vec::new(),
            },
        );
        Some(id)
    }

    pub(crate) fn state(&self, id: u64) -> Option<&AttrState> {
        self.entries.get(&id).map(|entry| &entry.state)
    }

    pub(crate) fn state_mut(&mut self, id: u64) -> Option<&mut AttrState> {
        self.entries.get_mut(&id).map(|entry| &mut entry.state)
    }

    pub(crate) fn wrapper(&self, id: u64) -> Option<Persistent<Value<'static>>> {
        self.entries
            .get(&id)
            .and_then(|entry| entry.wrapper.clone())
    }

    pub(crate) fn intern_wrapper(&mut self, id: u64, value: Persistent<Value<'static>>) {
        if let Some(entry) = self.entries.get_mut(&id) {
            entry.wrapper = Some(value);
        }
    }

    pub(crate) fn attached(&self, element: NodeId, namespace: &str, local: &str) -> Option<u64> {
        self.attached
            .get(&(element, namespace.to_owned(), local.to_owned()))
            .copied()
    }

    pub(crate) fn detach(&mut self, element: NodeId, namespace: &str, local: &str) {
        if let Some(id) = self
            .attached
            .remove(&(element, namespace.to_owned(), local.to_owned()))
            && let Some(state) = self.state_mut(id)
        {
            state.owner = None;
        }
    }

    pub(crate) fn touch(&mut self, element: NodeId, namespace: &str, local: &str, value: String) {
        if let Some(id) = self.attached(element, namespace, local)
            && let Some(state) = self.state_mut(id)
        {
            state.value = value;
        }
    }

    /// Validates the source identity and owner before mutating the destination,
    /// then publishes replacement and adoption in the same registry borrow.
    /// <https://dom.spec.whatwg.org/#concept-element-attributes-set>
    pub(crate) fn attach(
        &mut self,
        id: u64,
        base: &mut blitz_dom::BaseDocument,
        element: NodeId,
        value: String,
    ) -> Result<Option<u64>, AttrAttachError> {
        let entry = self.entries.get_mut(&id).ok_or(AttrAttachError::Stale)?;
        let state = &mut entry.state;
        if state.owner.is_some_and(|owner| owner != element) {
            return Err(AttrAttachError::InUse);
        }
        let key = (element, state.namespace.clone(), state.local.clone());
        let previous = self.attached.get(&key).copied();
        if previous == Some(id) {
            return Ok(previous);
        }
        let Some(node) = base.get_node(element.node) else {
            return Err(AttrAttachError::Stale);
        };
        if node.data.downcast_element().is_none() {
            return Err(AttrAttachError::NotAnElement);
        }
        let name = markup5ever::QualName::new(
            state.prefix.clone().map(markup5ever::Prefix::from),
            markup5ever::Namespace::from(state.namespace.clone()),
            markup5ever::LocalName::from(state.local.clone()),
        );
        base.mutate().set_attribute(element.node, name, &value);
        state.owner = Some(element);
        state.document = NodeId {
            document: element.document,
            node: base.root_node().id,
        };
        state.value = value;
        if let Some(previous) = previous
            && let Some(state) = self.state_mut(previous)
        {
            state.owner = None;
        }
        self.attached.insert(key, id);
        Ok(previous)
    }

    fn forget_document(&mut self, document: u32) {
        self.entries
            .retain(|_, entry| entry.state.document.document_id() != document);
        self.attached
            .retain(|(element, _, _), _| element.document_id() != document);
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.attached.clear();
    }
}
