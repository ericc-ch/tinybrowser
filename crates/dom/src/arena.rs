//! The arena: flat slot array, generational handles, tree mutations.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::iter::FusedIterator;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::id::NodeId;
use crate::node::{
    Attribute, LocalName, Namespace, Node, NodeKind, Prefix, QualName, html_namespace,
    html_qualified_name_eq, qualified_name_eq,
};
use crate::value::{
    is_valid_date, is_valid_floating_point, is_valid_local_date_time, is_valid_month,
    is_valid_simple_color, is_valid_time, is_valid_week,
};

/// Next document id for a freshly constructed [`Dom`]. Relaxed arithmetic is
/// enough: the only requirement is that two live `Dom` values do not share
/// an id until the counter wraps (2^32 documents, accepted like generation
/// wrap).
static NEXT_DOCUMENT_ID: AtomicU32 = AtomicU32::new(0);

const INPUT_TYPES: &[&str] = &[
    "hidden",
    "text",
    "search",
    "tel",
    "url",
    "email",
    "password",
    "date",
    "month",
    "week",
    "time",
    "datetime-local",
    "number",
    "range",
    "color",
    "checkbox",
    "radio",
    "file",
    "submit",
    "image",
    "reset",
    "button",
];

/// The document-compatibility mode a query runs under: what html5ever's
/// tree builder reports and parsed pages carry.
///
/// It changes exactly one matching behavior: in full quirks mode, class
/// and id selector values compare ASCII-case-insensitively (the WHATWG
/// id/class quirk). Standards and limited-quirks modes stay exact.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum QuirksMode {
    /// Standards mode: full CSS case rules.
    #[default]
    NoQuirks,
    /// Limited quirks: same selector rules as standards mode.
    LimitedQuirks,
    /// Full quirks: legacy case-insensitive class/id matching.
    Quirks,
}

/// One recorded tree mutation, for `MutationObserver` delivery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mutation {
    /// Children added and/or removed on `target`, in one operation.
    ChildList {
        target: NodeId,
        added: Vec<NodeId>,
        removed: Vec<NodeId>,
        previous: Option<NodeId>,
        next: Option<NodeId>,
    },
    /// An attribute set, changed, or removed on `target`.
    Attributes {
        target: NodeId,
        name: String,
        namespace: String,
        old_value: Option<String>,
    },
    /// Character data replaced on `target`.
    CharacterData { target: NodeId, old_value: String },
}

/// A connection transition of one element, for HTML lifecycle steps.
///
/// [HTML's post-connection and removing steps](https://html.spec.whatwg.org/multipage/iframe-embed-object.html#the-iframe-element)
/// hang off exactly these transitions: an iframe creates its content
/// navigable when it becomes connected and destroys it when disconnected.
/// Recorded always, independent of `MutationObserver` recording.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lifecycle {
    /// An element became connected to a document.
    Inserted(NodeId),
    /// An element became disconnected from a document.
    Removed(NodeId),
}

/// Why a mutation was refused.
///
/// Stale handles and structural mistakes surface as values, never as panics,
/// so future JS bindings can map them straight onto DOM exceptions: one
/// variant per exception class, not per call site.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DomError {
    /// A handle named a node that no longer exists.
    StaleNode,
    /// The move would place a node inside its own subtree. Browsers also
    /// report this as `HierarchyRequestError`; kept distinct so a binding
    /// can map both without losing the cycle case.
    CycleForbidden,
    /// The tree's hierarchy or content model forbids the operation:
    /// a document gaining a second root or a misplaced doctype, character
    /// data under a document, a leaf node asked to parent children, the
    /// document root asked to gain a parent. (Maps to
    /// `HierarchyRequestError`; see `Dom::ensure_pre_insert_validity`.)
    HierarchyRequest,
    /// The operation does not apply to that kind of node (setting text data
    /// on an element, attributes on a text node). (Maps to a type error at
    /// the binding layer.)
    WrongNodeType,
    /// An insert was requested beside a node with no parent to sit under.
    /// (Maps to `NotFoundError`.)
    NoParent,
    /// The control's state forbids the operation, such as setting a non-empty
    /// `value` on a `type=file` input. (Maps to `InvalidStateError`.)
    InvalidState,
}

impl fmt::Display for DomError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StaleNode => f.write_str("stale node handle"),
            Self::CycleForbidden => f.write_str("operation would create a cycle"),
            Self::HierarchyRequest => {
                f.write_str("hierarchy or content model forbids this operation")
            }
            Self::WrongNodeType => f.write_str("operation not valid for this node kind"),
            Self::NoParent => f.write_str("target has no parent to insert beside"),
            Self::InvalidState => f.write_str("operation invalid for the control's state"),
        }
    }
}

impl std::error::Error for DomError {}

/// One cell of the arena: current contents plus how many times it changed hands.
///
/// Crate-visible only so selector matching can take a node's storage address
/// as a stable identity token (see [`Dom::cache_identity`]).
#[derive(Debug)]
pub(crate) struct Slot {
    generation: u32,
    node: Option<Node>,
}

/// A document: every node lives inside one flat slot array.
///
/// All access goes through [`NodeId`] handles. Handles outliving their node
/// are harmless (lookups report absence), which lets the `QuickJS` binding
/// layer hold handles across garbage-collection cycles without borrowing
/// anything.
///
/// [`Send`] but deliberately not [`Sync`]: a `Dom` may be handed between
/// workers, but two threads can never touch one simultaneously (one worker
/// per document). The marker field below is what suppresses the
/// otherwise-auto-derived `Sync`.
#[derive(Debug)]
pub struct Dom {
    slots: Vec<Slot>,
    free: Vec<u32>,
    document: NodeId,
    quirks_mode: QuirksMode,
    /// HTTP `Content-Language` and parsed `document` language default for
    /// `:lang()` when no `lang` / `xml:lang` is on the ancestor chain.
    document_language: Option<String>,
    /// `<template>` element → its contents fragment. Contents live outside
    /// the element's child list
    /// (<https://html.spec.whatwg.org/multipage/scripting.html#the-template-element>).
    template_contents: HashMap<NodeId, NodeId>,
    /// Shadow host → (shadow root, open mode). Shadow roots are detached
    /// fragments in the node tree and cross to their host only for the
    /// shadow-including tree algorithms.
    shadow_roots: HashMap<NodeId, (NodeId, bool)>,
    /// Shadow root → host, the inverse of `shadow_roots`.
    shadow_hosts: HashMap<NodeId, NodeId>,
    /// Dirty value state for text-like controls (`input`, `textarea`).
    ///
    /// An entry means the dirty value flag is set and stores the raw value.
    /// Absence means the live value still follows the control's default: the
    /// `value` content attribute for `input`, the child text content for
    /// `textarea` (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#concept-fe-dirty>).
    input_values: HashMap<NodeId, String>,
    /// The source-text start line of an inline `script` element, recorded by
    /// the parser so reported exceptions carry a document line number
    /// (<https://html.spec.whatwg.org/multipage/webappapis.html#script's-line-number>).
    script_lines: HashMap<NodeId, u32>,
    /// An `input`'s checkedness while the dirty checkedness flag is set;
    /// absence means the `checked` content attribute decides
    /// (<https://html.spec.whatwg.org/multipage/input.html#concept-input-checked-dirty-flag>).
    checkedness: HashMap<NodeId, bool>,
    /// An `input`'s indeterminateness, independent of its checkedness
    /// (<https://html.spec.whatwg.org/multipage/input.html#concept-input-indeterminate>).
    indeterminate: HashMap<NodeId, bool>,
    /// The focused element per document, backing the `:focus` family
    /// (<https://drafts.csswg.org/selectors-4/#the-focus-pseudo>).
    active_element: HashMap<u32, NodeId>,
    /// An `option`'s selectedness value
    /// (<https://html.spec.whatwg.org/multipage/form-elements.html#concept-option-selectedness>).
    option_selectedness: HashMap<NodeId, bool>,
    /// Whether an `option`'s dirty selectedness flag is set; when clear, the
    /// `selected` content attribute drives selectedness.
    option_dirty_selected: HashSet<NodeId>,
    /// Per-element scroll offsets `(left, top)`. The engine has no scrollable
    /// overflow yet, but `scrollLeft`/`scrollTop` must round-trip a set value
    /// (<https://drafts.csswg.org/cssom-view/#dom-element-scrollleft>).
    scroll_offsets: HashMap<NodeId, (f64, f64)>,
    /// Whether an `input`'s type supported a text selection the last time its
    /// `type` changed, so a change back to a selectable type can reset the
    /// cursor (<https://html.spec.whatwg.org/multipage/input.html#the-input-element>).
    input_selectable: HashMap<NodeId, bool>,
    /// Text selection for text-like controls: `(start, end, direction)` in
    /// UTF-16 code units. Direction is 0 "none", 1 "forward", 2 "backward"
    /// (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#concept-textarea/input-selection>).
    /// Absence means the initial selection `(0, 0, "none")`.
    selections: HashMap<NodeId, (u32, u32, u8)>,
    /// Recorded mutations, drained by the renderer's `MutationObserver`
    /// plumbing; empty and unrecorded unless someone observes the document.
    mutations: Vec<Mutation>,
    record_mutations: bool,
    recording_suppressed: bool,
    /// Connection transitions in order; never suppressed, because the
    /// renderer's frame lifetime hangs off them, not off observers.
    lifecycle: Vec<Lifecycle>,
    /// Connected `iframe` elements, so the renderer can tell whether a frame
    /// scan is needed at all.
    connected_iframes: u32,
    /// Bumped by every mutation; see [`Dom::mutation_serial`].
    mutation_serial: u64,
    /// `Cell<()>` is `Send` + `!Sync`; `PhantomData` makes `Dom` inherit
    /// exactly that split. Deleting this field would silently re-derive
    /// `Sync`, which is the point: that deletion has to be a conscious act.
    _share_forbidden: PhantomData<Cell<()>>,
}

impl Default for Dom {
    fn default() -> Self {
        Self::new()
    }
}

/// The children of one node, walked through the sibling links.
///
/// Double-ended because the tree is: `next_back` reads `last_child` and the
/// `previous_sibling` links, so both directions are O(1) per step. Obtained
/// from [`Dom::children`]; `None` there means the handle is stale, while a
/// live leaf yields an empty iterator.
pub struct Children<'a> {
    dom: &'a Dom,
    front: Option<NodeId>,
    back: Option<NodeId>,
}

impl Iterator for Children<'_> {
    type Item = NodeId;

    fn next(&mut self) -> Option<NodeId> {
        let current = self.front?;
        // When the two cursors meet on the last element, both must retire:
        // leaving `back` behind would let `next_back` yield the same node
        // again to a caller that mixes directions.
        if self.back == Some(current) {
            self.front = None;
            self.back = None;
        } else {
            self.front = self.dom.next_sibling(current);
        }
        Some(current)
    }
}

impl DoubleEndedIterator for Children<'_> {
    fn next_back(&mut self) -> Option<NodeId> {
        let current = self.back?;
        if self.front == Some(current) {
            self.front = None;
            self.back = None;
        } else {
            self.back = self.dom.previous_sibling(current);
        }
        Some(current)
    }
}

// Once both cursors retire the iterator stays exhausted, never resurrecting a
// node through a later `next`/`next_back`.
impl FusedIterator for Children<'_> {}

impl Dom {
    /// An empty document containing just the root `Document` node.
    #[must_use]
    pub fn new() -> Self {
        let document_id = NEXT_DOCUMENT_ID.fetch_add(1, Ordering::Relaxed);
        let root = Node {
            parent: None,
            first_child: None,
            last_child: None,
            previous_sibling: None,
            next_sibling: None,
            kind: NodeKind::Document,
        };
        Self {
            slots: vec![Slot {
                generation: 0,
                node: Some(root),
            }],
            free: Vec::new(),
            document: NodeId::new(document_id, 0, 0),
            quirks_mode: QuirksMode::NoQuirks,
            document_language: None,
            template_contents: HashMap::new(),
            shadow_roots: HashMap::new(),
            shadow_hosts: HashMap::new(),
            input_values: HashMap::new(),
            selections: HashMap::new(),
            script_lines: HashMap::new(),
            checkedness: HashMap::new(),
            indeterminate: HashMap::new(),
            active_element: HashMap::new(),
            option_selectedness: HashMap::new(),
            option_dirty_selected: HashSet::new(),
            scroll_offsets: HashMap::new(),
            input_selectable: HashMap::new(),
            mutations: Vec::new(),
            record_mutations: false,
            recording_suppressed: false,
            lifecycle: Vec::new(),
            connected_iframes: 0,
            mutation_serial: 0,
            _share_forbidden: PhantomData,
        }
    }

    /// Turns mutation recording on or off; recording costs nothing while no
    /// `MutationObserver` is registered.
    pub fn set_record_mutations(&mut self, recording: bool) {
        self.record_mutations = recording;
        if !recording {
            self.mutations.clear();
        }
    }

    /// Drains the recorded mutations in order.
    pub fn take_mutations(&mut self) -> Vec<Mutation> {
        std::mem::take(&mut self.mutations)
    }

    /// Drains the recorded connection transitions in order.
    pub fn take_lifecycle(&mut self) -> Vec<Lifecycle> {
        std::mem::take(&mut self.lifecycle)
    }

    /// Records every `iframe` and `img` connection transition in a snapshot
    /// taken before an operation. Snapshots only ever carry those elements
    /// (see [`Dom::connection_snapshot`]): filtering in the snapshot keeps
    /// the parser's hot path free of per-element bookkeeping.
    ///
    /// The snapshot carries each element's kind so a destroyed node still
    /// moves the `iframe` count: after `destroy` frees it, `is_iframe_element`
    /// can no longer answer.
    fn record_snapshot(&mut self, snapshot: Vec<(NodeId, bool, bool)>) {
        for (id, was_connected, is_iframe) in snapshot {
            let connected = self.is_connected(id);
            if connected != was_connected {
                if is_iframe {
                    if connected {
                        self.connected_iframes = self.connected_iframes.saturating_add(1);
                    } else {
                        self.connected_iframes = self.connected_iframes.saturating_sub(1);
                    }
                }
                self.lifecycle.push(if connected {
                    Lifecycle::Inserted(id)
                } else {
                    Lifecycle::Removed(id)
                });
            }
        }
    }

    /// How many `iframe` elements are connected in this document.
    #[must_use]
    pub fn connected_iframe_count(&self) -> u32 {
        self.connected_iframes
    }

    /// Whether `id` is an HTML `iframe` element.
    #[must_use]
    pub fn is_iframe_element(&self, id: NodeId) -> bool {
        matches!(
            self.kind(id),
            Some(NodeKind::Element { name, .. })
                if name.ns == html_namespace() && name.local.as_ref() == "iframe"
        )
    }

    /// Whether `id` is an HTML `img` element.
    #[must_use]
    pub fn is_img_element(&self, id: NodeId) -> bool {
        matches!(
            self.kind(id),
            Some(NodeKind::Element { name, .. })
                if name.ns == html_namespace() && name.local.as_ref() == "img"
        )
    }

    fn is_html_slot(&self, id: NodeId) -> bool {
        matches!(
            self.kind(id),
            Some(NodeKind::Element { name, .. })
                if name.ns == html_namespace() && name.local.as_ref() == "slot"
        )
    }

    fn slottable_name(&self, id: NodeId) -> Option<String> {
        match self.kind(id) {
            Some(NodeKind::Element { .. }) => Some(self.attribute(id, "slot").unwrap_or_default()),
            Some(NodeKind::Text { .. }) => Some(String::new()),
            _ => None,
        }
    }

    fn containing_shadow_root(&self, id: NodeId) -> Option<NodeId> {
        let mut current = Some(id);
        while let Some(node) = current {
            if self.shadow_host(node).is_some() {
                return Some(node);
            }
            current = self.parent(node);
        }
        None
    }

    fn first_slot(&self, root: NodeId, name: &str) -> Option<NodeId> {
        let mut stack: Vec<NodeId> = self
            .children(root)
            .map(Iterator::collect)
            .unwrap_or_default();
        stack.reverse();
        while let Some(node) = stack.pop() {
            if self.is_html_slot(node) && self.attribute(node, "name").unwrap_or_default() == name {
                return Some(node);
            }
            if self.shadow_root(node).is_some() {
                continue;
            }
            if let Some(children) = self.children(node) {
                stack.extend(children.rev());
            }
        }
        None
    }

    fn assigned_nodes(&self, slot: NodeId) -> Vec<NodeId> {
        let Some(root) = self.containing_shadow_root(slot) else {
            return Vec::new();
        };
        let Some(host) = self.shadow_host(root) else {
            return Vec::new();
        };
        let name = self.attribute(slot, "name").unwrap_or_default();
        if self.first_slot(root, &name) != Some(slot) {
            return Vec::new();
        }
        self.children(host)
            .into_iter()
            .flatten()
            .filter(|&child| self.slottable_name(child).as_deref() == Some(name.as_str()))
            .collect()
    }

    fn assigned_slot(&self, id: NodeId) -> Option<NodeId> {
        let host = self.parent(id)?;
        let root = self.shadow_root(host)?;
        let name = self.slottable_name(id)?;
        let slot = self.first_slot(root, &name)?;
        Some(slot).filter(|&slot| self.assigned_nodes(slot).contains(&id))
    }

    /// The iframe and img elements in `id`'s inclusive subtree with, for each,
    /// its connectivity and whether it is an iframe (the only kind that moves
    /// `connected_iframes`). Captured before an operation and handed to
    /// [`Dom::record_snapshot`] after it, so the transition is measured across
    /// the whole operation rather than at an internal step.
    fn connection_snapshot(&self, id: NodeId) -> Vec<(NodeId, bool, bool)> {
        let mut snapshot = Vec::new();
        let mut stack = vec![id];
        while let Some(current) = stack.pop() {
            let is_iframe = self.is_iframe_element(current);
            if is_iframe || self.is_img_element(current) {
                snapshot.push((current, self.is_connected(current), is_iframe));
            }
            if let Some(children) = self.children(current) {
                stack.extend(children);
            }
            if let Some(root) = self.shadow_root(current) {
                stack.push(root);
            }
        }
        snapshot
    }

    /// Whether `id`'s ancestor chain reaches the document root.
    #[must_use]
    pub fn is_connected(&self, id: NodeId) -> bool {
        let mut current = Some(id);
        while let Some(node) = current {
            if node == self.document {
                return true;
            }
            current = self.parent(node).or_else(|| self.shadow_host(node));
        }
        false
    }

    /// `id`'s ancestors, nearest first, `id` excluded.
    pub fn ancestors(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        std::iter::successors(self.parent(id), |&node| self.parent(node))
    }

    fn record(&mut self, mutation: Mutation) {
        // Bumped even while recording is suppressed: the tree changed, and
        // the renderer's frame-order cache keys off this serial.
        self.mutation_serial = self.mutation_serial.wrapping_add(1);
        if self.record_mutations && !self.recording_suppressed {
            self.mutations.push(mutation);
        }
    }

    /// A counter that changes on every recorded mutation.
    ///
    /// Consumers that must rescan the tree (frame order) compare this instead
    /// of walking the whole document on every turn.
    #[must_use]
    pub fn mutation_serial(&self) -> u64 {
        self.mutation_serial
    }

    /// Compatibility mode this document answers selector queries under.
    #[must_use]
    pub fn quirks_mode(&self) -> QuirksMode {
        self.quirks_mode
    }

    /// Sets the compatibility mode. The html5ever adapter writes this from
    /// the tree builder; tests may set it to exercise the id/class quirk.
    pub fn set_quirks_mode(&mut self, mode: QuirksMode) {
        self.quirks_mode = mode;
    }

    /// Document-level language from HTTP `Content-Language`, used by
    /// `:lang()` when no element `lang` / `xml:lang` applies.
    #[must_use]
    pub fn document_language(&self) -> Option<&str> {
        self.document_language.as_deref()
    }

    /// Sets the document language default. `browser` writes this after
    /// navigation; tests may set it directly.
    pub fn set_document_language(&mut self, language: Option<String>) {
        self.document_language = language;
    }

    /// The root `Document` node; every other node descends from it.
    #[must_use]
    pub fn document(&self) -> NodeId {
        self.document
    }

    /// This document's arena id, for per-document renderer lookups.
    #[must_use]
    pub fn document_id(&self) -> u32 {
        self.document.document_id()
    }

    /// Whether `id` names a currently live node.
    ///
    /// A destroyed node's handle fails here even though a different node may
    /// later occupy the same slot; that is the whole point of generations.
    #[must_use]
    pub fn contains(&self, id: NodeId) -> bool {
        self.live_slot(id).is_some()
    }

    /// What kind of node `id` names, or `None` for a stale handle.
    #[must_use]
    pub fn kind(&self, id: NodeId) -> Option<&NodeKind> {
        Some(&self.live_slot(id)?.node.as_ref()?.kind)
    }

    /// The parent of `id`, or `None` if it is unparented or `id` is stale.
    #[must_use]
    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.live_slot(id)?.node.as_ref()?.parent
    }

    /// The children of `id` in document order, or `None` for a stale handle.
    ///
    /// Mirrors DOM `childNodes`: every live node answers a list; leaves
    /// (text, comment, doctype) answer an empty one because the mutation
    /// gate below refuses to give them children. Childless and dead remain
    /// different answers; only staleness is `None`.
    #[must_use]
    pub fn children(&self, id: NodeId) -> Option<Children<'_>> {
        let node = self.live_slot(id)?.node.as_ref()?;
        Some(Children {
            dom: self,
            front: node.first_child,
            back: node.last_child,
        })
    }

    /// The first child of `id`, or `None` when it has none or is stale.
    #[must_use]
    pub fn first_child(&self, id: NodeId) -> Option<NodeId> {
        self.live_slot(id)?.node.as_ref()?.first_child
    }

    /// The last child of `id`, or `None` when it has none or is stale.
    #[must_use]
    pub fn last_child(&self, id: NodeId) -> Option<NodeId> {
        self.live_slot(id)?.node.as_ref()?.last_child
    }

    /// The sibling immediately before `id`, or `None`.
    #[must_use]
    pub fn previous_sibling(&self, id: NodeId) -> Option<NodeId> {
        self.live_slot(id)?.node.as_ref()?.previous_sibling
    }

    /// The sibling immediately after `id`, or `None`.
    #[must_use]
    pub fn next_sibling(&self, id: NodeId) -> Option<NodeId> {
        self.live_slot(id)?.node.as_ref()?.next_sibling
    }

    /// The sibling of `id` adjacent in the given direction, or `None`.
    #[must_use]
    pub fn sibling(&self, id: NodeId, forward: bool) -> Option<NodeId> {
        if forward {
            self.next_sibling(id)
        } else {
            self.previous_sibling(id)
        }
    }

    /// Children in the flattened tree used for style and box construction.
    /// A shadow host yields its shadow tree, and a slot yields its assigned
    /// nodes when any exist
    /// (<https://drafts.csswg.org/css-scoping/#flattening>).
    #[must_use]
    pub fn rendered_children(&self, id: NodeId) -> Vec<NodeId> {
        if let Some(root) = self.shadow_root(id) {
            return self.rendered_children(root);
        }
        if self.is_html_slot(id) {
            let assigned = self.assigned_nodes(id);
            if !assigned.is_empty() {
                return assigned;
            }
        }
        self.children(id).map(Iterator::collect).unwrap_or_default()
    }

    /// Parent in the flattened tree used for style and box construction.
    #[must_use]
    pub fn rendered_parent(&self, id: NodeId) -> Option<NodeId> {
        if let Some(slot) = self.assigned_slot(id) {
            return Some(slot);
        }
        match self.parent(id) {
            Some(parent) => self.shadow_host(parent).or(Some(parent)),
            None => self.shadow_host(id),
        }
    }

    /// Adjacent sibling in the rendered, shadow-including tree.
    #[must_use]
    pub fn rendered_sibling(&self, id: NodeId, forward: bool) -> Option<NodeId> {
        let parent = self.rendered_parent(id)?;
        let children = self.rendered_children(parent);
        let position = children.iter().position(|&child| child == id)?;
        if forward {
            children.get(position + 1).copied()
        } else {
            position
                .checked_sub(1)
                .and_then(|index| children.get(index).copied())
        }
    }

    /// Descendants in pre-order through rendered shadow trees.
    #[must_use]
    pub fn rendered_descendants(&self, id: NodeId) -> Vec<NodeId> {
        let mut descendants = Vec::new();
        let mut stack = self.rendered_children(id);
        stack.reverse();
        while let Some(node) = stack.pop() {
            descendants.push(node);
            let mut children = self.rendered_children(node);
            children.reverse();
            stack.extend(children);
        }
        descendants
    }

    /// A stable identity token for `id`, for selector-engine caches.
    ///
    /// One live node owns exactly one slot, and matching runs under a shared
    /// borrow that freezes the arena, so the slot's address is unique per
    /// node and stable for the whole query. Detached nodes keep their slots,
    /// so identity survives detachment too.
    #[must_use]
    pub(crate) fn cache_identity(&self, id: NodeId) -> &Slot {
        self.live_slot(id)
            .expect("selector cache identity requires a live handle")
    }

    /// Creates an element node, unattached until something appends it.
    ///
    /// Attribute names are unique in the DOM: `NamedNodeMap` is keyed by
    /// qualified name (<https://dom.spec.whatwg.org/#concept-attribute>,
    /// "an attribute list is essentially a map of names to attributes"),
    /// so later duplicates are dropped and the first occurrence wins,
    /// matching the merge rule the parser drives through
    /// [`Dom::add_attrs_if_missing`]. Hand-built callers get the same
    /// normalization instead of an unrepresentable state.
    ///
    /// # Panics
    ///
    /// Only if the arena exceeds `u32::MAX` slots: terabytes of RAM, not a
    /// reachable runtime condition; the bound guards the handle width.
    pub fn create_element(&mut self, name: QualName, attributes: Vec<Attribute>) -> NodeId {
        let mut unique: Vec<Attribute> = Vec::with_capacity(attributes.len());
        merge_attrs(&mut unique, attributes);
        self.alloc(NodeKind::Element {
            name,
            attributes: unique,
        })
    }

    /// Creates a text node holding `data`.
    ///
    /// # Panics
    ///
    /// See [`Dom::create_element`]: unreachable except beyond `u32::MAX` nodes.
    pub fn create_text(&mut self, data: impl Into<String>) -> NodeId {
        self.alloc(NodeKind::Text { data: data.into() })
    }

    /// Creates a comment node holding `data`.
    ///
    /// # Panics
    ///
    /// See [`Dom::create_element`]: unreachable except beyond `u32::MAX` nodes.
    pub fn create_comment(&mut self, data: impl Into<String>) -> NodeId {
        self.alloc(NodeKind::Comment { data: data.into() })
    }

    /// Creates a CDATA section holding `data`.
    ///
    /// # Panics
    ///
    /// See [`Dom::create_element`]: unreachable except beyond `u32::MAX` nodes.
    pub fn create_cdata_section(&mut self, data: impl Into<String>) -> NodeId {
        self.alloc(NodeKind::CDataSection { data: data.into() })
    }

    /// Creates a processing instruction with `target` and `data`.
    ///
    /// # Panics
    ///
    /// See [`Dom::create_element`]: unreachable except beyond `u32::MAX` nodes.
    pub fn create_processing_instruction(
        &mut self,
        target: impl Into<String>,
        data: impl Into<String>,
    ) -> NodeId {
        self.alloc(NodeKind::ProcessingInstruction {
            target: target.into(),
            data: data.into(),
        })
    }

    /// Creates a doctype node.
    ///
    /// # Panics
    ///
    /// See [`Dom::create_element`]: unreachable except beyond `u32::MAX` nodes.
    pub fn create_doctype(
        &mut self,
        name: impl Into<String>,
        public_id: impl Into<String>,
        system_id: impl Into<String>,
    ) -> NodeId {
        self.alloc(NodeKind::Doctype {
            name: name.into(),
            public_id: public_id.into(),
            system_id: system_id.into(),
        })
    }

    /// Creates an empty document fragment, unattached like every fresh node.
    ///
    /// Fragments are containers outside the main tree: the contents root of
    /// `<template>` elements (associated with [`Dom::set_template_contents`]) and
    /// the context node for `innerHTML`-style fragment parsing.
    ///
    /// # Panics
    ///
    /// See [`Dom::create_element`]: unreachable except beyond `u32::MAX` nodes.
    pub fn create_fragment(&mut self) -> NodeId {
        self.alloc(NodeKind::Fragment)
    }

    /// [Clones](https://dom.spec.whatwg.org/#concept-node-clone) `id` into a
    /// new unattached node. `subtree` copies descendants (and a template's
    /// contents fragment). The document node is refused: cloning a document
    /// is a different spec operation.
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if `id` is stale.
    /// - [`DomError::WrongNodeType`] if `id` is the document.
    pub fn clone_node(&mut self, id: NodeId, subtree: bool) -> Result<NodeId, DomError> {
        self.require_live(id)?;
        if id == self.document {
            return Err(DomError::WrongNodeType);
        }
        let copy = self.alloc(self.kind(id).ok_or(DomError::StaleNode)?.clone());
        let mut pending = vec![(id, copy)];
        while let Some((source, target)) = pending.pop() {
            // Cloning steps for text-like controls propagate the raw value and
            // dirty value flag from source to copy
            // (<https://html.spec.whatwg.org/multipage/form-elements.html#the-textarea-element:concept-node-clone-ext>).
            if let Some(value) = self.input_values.get(&source).cloned() {
                self.input_values.insert(target, value);
            }
            // https://html.spec.whatwg.org/multipage/scripting.html#the-template-element:cloning-steps
            if let Some(contents) = self.template_contents(source) {
                let cloned_contents = self.create_fragment();
                self.set_template_contents(target, cloned_contents)?;
                if subtree {
                    pending.push((contents, cloned_contents));
                }
            }
            if subtree {
                let kids: Vec<NodeId> = self.children(source).ok_or(DomError::StaleNode)?.collect();
                for kid in kids {
                    let child = self.alloc(self.kind(kid).ok_or(DomError::StaleNode)?.clone());
                    self.append(target, child)?;
                    pending.push((kid, child));
                }
            }
        }
        Ok(copy)
    }

    /// Associates `contents` as the [template contents](https://html.spec.whatwg.org/multipage/scripting.html#template-contents)
    /// of `template`. The fragment stays out of `template`'s child list.
    ///
    /// Replacing an existing association destroys the previous fragment.
    ///
    /// # Errors
    ///
    /// - [`DomError::CycleForbidden`] if the association creates a host-including cycle.
    /// - [`DomError::HierarchyRequest`] if replacement would destroy the new contents.
    /// - [`DomError::StaleNode`] if either handle is stale.
    /// - [`DomError::WrongNodeType`] if `template` is not an HTML `template`
    ///   element, `contents` is not a fragment, or `contents` already belongs
    ///   to another template.
    pub fn set_template_contents(
        &mut self,
        template: NodeId,
        contents: NodeId,
    ) -> Result<(), DomError> {
        self.require_live(template)?;
        self.require_live(contents)?;
        if !self.is_html_template_element(template) {
            return Err(DomError::WrongNodeType);
        }
        if !self.is_fragment(contents) {
            return Err(DomError::WrongNodeType);
        }
        if self
            .template_contents
            .iter()
            .any(|(&owner, &mapped)| mapped == contents && owner != template)
        {
            return Err(DomError::WrongNodeType);
        }
        // https://dom.spec.whatwg.org/#concept-tree-host-including-inclusive-ancestor
        if self.would_cycle(contents, template) {
            return Err(DomError::CycleForbidden);
        }
        if let Some(old) = self.template_contents.get(&template).copied()
            && old != contents
        {
            if self.would_cycle(old, contents) {
                return Err(DomError::HierarchyRequest);
            }
            self.destroy(old)?;
            self.template_contents.remove(&template);
        }
        self.template_contents.insert(template, contents);
        Ok(())
    }

    /// The contents fragment of `template`, if this document associated one.
    #[must_use]
    pub fn template_contents(&self, template: NodeId) -> Option<NodeId> {
        let contents = self.template_contents.get(&template).copied()?;
        self.contains(contents).then_some(contents)
    }

    /// Attaches a new shadow root to `host`.
    ///
    /// Implements the tree association from the DOM "attach a shadow root"
    /// algorithm; policy checks such as the HTML element safelist remain at
    /// the binding boundary.
    /// <https://dom.spec.whatwg.org/#concept-attach-a-shadow-root>
    ///
    /// # Errors
    ///
    /// Returns [`DomError::HierarchyRequest`] if the host already has a
    /// shadow root, and [`DomError::WrongNodeType`] for a non-element host.
    pub fn attach_shadow(&mut self, host: NodeId, open: bool) -> Result<NodeId, DomError> {
        self.require_live(host)?;
        if !matches!(self.kind(host), Some(NodeKind::Element { .. })) {
            return Err(DomError::WrongNodeType);
        }
        if self.shadow_roots.contains_key(&host) {
            return Err(DomError::HierarchyRequest);
        }
        let root = self.create_fragment();
        self.shadow_roots.insert(host, (root, open));
        self.shadow_hosts.insert(root, host);
        self.mutation_serial = self.mutation_serial.wrapping_add(1);
        Ok(root)
    }

    /// The shadow root associated with `host`, including a closed root.
    #[must_use]
    pub fn shadow_root(&self, host: NodeId) -> Option<NodeId> {
        let (root, _) = self.shadow_roots.get(&host).copied()?;
        self.contains(root).then_some(root)
    }

    /// The open shadow root associated with `host`.
    #[must_use]
    pub fn open_shadow_root(&self, host: NodeId) -> Option<NodeId> {
        let (root, open) = self.shadow_roots.get(&host).copied()?;
        (open && self.contains(root)).then_some(root)
    }

    /// The host of a shadow-root fragment.
    #[must_use]
    pub fn shadow_host(&self, root: NodeId) -> Option<NodeId> {
        let host = self.shadow_hosts.get(&root).copied()?;
        self.contains(host).then_some(host)
    }

    /// Whether `root` is an open shadow root.
    #[must_use]
    pub fn shadow_root_is_open(&self, root: NodeId) -> Option<bool> {
        let host = self.shadow_host(root)?;
        self.shadow_roots
            .get(&host)
            .and_then(|&(candidate, open)| (candidate == root).then_some(open))
    }

    /// Appends `child` as the last child of `parent`.
    ///
    /// Moving semantics: a `child` already attached elsewhere is detached
    /// first, mirroring DOM `appendChild`.
    ///
    /// # Panics
    ///
    /// Only on an internal invariant defect (a verified-live node missing its
    /// child list), never on user input; input failures return errors.
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if either handle is stale.
    /// - [`DomError::HierarchyRequest`] if the content model forbids it:
    ///   `child` is the document root, `parent` is a leaf kind, a document
    ///   would gain a second element child or a misplaced doctype, or a
    ///   doctype is placed outside a document.
    /// - [`DomError::CycleForbidden`] if `child` is an ancestor of `parent`.
    pub fn append(&mut self, parent: NodeId, child: NodeId) -> Result<(), DomError> {
        self.ensure_alive(parent, child)?;
        if child == self.document {
            // The root must never gain a parent; that is how a document
            // gets orphaned from itself. (Maps to HierarchyRequestError.)
            return Err(DomError::HierarchyRequest);
        }
        self.ensure_pre_insert_validity(parent, child, None)?;
        let tracked = self.connection_snapshot(child);
        if self.is_fragment(child) {
            self.splice_fragment(parent, child, None);
        } else {
            self.place_node(parent, child, None);
        }
        self.record_snapshot(tracked);
        Ok(())
    }

    /// Inserts `node` immediately before `sibling` under sibling's parent.
    ///
    /// Moving semantics, like [`Dom::append`]. Inserting a node beside
    /// itself is a legal stay-put no-op (WHATWG DOM's *ensure pre-insert
    /// validity* returns without doing anything in that case), not an
    /// error.
    ///
    /// # Panics
    ///
    /// Only on an internal invariant defect (a live sibling missing from its
    /// own parent's list), never on user input.
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if either handle is stale.
    /// - [`DomError::NoParent`] if `sibling` has no parent to insert beside
    ///   (`NotFoundError`, including the detached-sibling case).
    /// - [`DomError::HierarchyRequest`] / [`DomError::CycleForbidden`] as
    ///   from `Dom::ensure_pre_insert_validity`.
    pub fn insert_before(&mut self, sibling: NodeId, node: NodeId) -> Result<(), DomError> {
        self.ensure_alive(sibling, node)?;
        // The reference child must sit under some parent to be inserted
        // beside; a detached one has none: the outer pre-insert
        // algorithm's parent-null refusal (`NotFoundError`).
        let parent = self.parent(sibling).ok_or(DomError::NoParent)?;
        if node == self.document {
            return Err(DomError::HierarchyRequest);
        }
        // Node-beside-itself means "stay put": the gate would reject this
        // as a cycle (a node contains itself), but the spec's answer is a
        // silent return, so it short-circuits before validation.
        if sibling == node {
            return Ok(());
        }
        self.ensure_pre_insert_validity(parent, node, Some(sibling))?;
        let tracked = self.connection_snapshot(node);
        if self.is_fragment(node) {
            self.splice_fragment(parent, node, Some(sibling));
        } else {
            self.place_node(parent, node, Some(sibling));
        }
        self.record_snapshot(tracked);
        Ok(())
    }

    /// [Replaces](https://dom.spec.whatwg.org/#concept-node-replace) `child`
    /// with `node` inside `parent`. `child` stays alive (detached) after the
    /// call; the caller returns it.
    ///
    /// Validation runs against the child sequence *without* `child`
    /// (`childrenToExclude` in the spec's ensure-pre-insert-validity), so a
    /// refused replacement leaves the tree untouched.
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if any handle is stale.
    /// - [`DomError::NoParent`] if `child` is not a child of `parent`
    ///   (`NotFoundError`).
    /// - [`DomError::HierarchyRequest`] / [`DomError::CycleForbidden`] as
    ///   from `Dom::ensure_pre_insert_validity`.
    pub fn replace_child(
        &mut self,
        parent: NodeId,
        node: NodeId,
        child: NodeId,
    ) -> Result<(), DomError> {
        self.require_live(parent)?;
        self.require_live(node)?;
        self.require_live(child)?;
        if !self.can_contain_children(parent) {
            return Err(DomError::HierarchyRequest);
        }
        if self.would_cycle(node, parent) {
            return Err(DomError::CycleForbidden);
        }
        if self.parent(child) != Some(parent) {
            return Err(DomError::NoParent);
        }
        // Same insertability gate as pre-insert
        // (<https://dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity>).
        if !self.is_insertable(node) {
            return Err(DomError::HierarchyRequest);
        }
        if !self.is_document(parent) && !self.is_fragment(node) && self.is_doctype(node) {
            return Err(DomError::HierarchyRequest);
        }
        if self.is_document(parent) {
            let incoming = self.incoming_nodes(node);
            let mut sequence: Vec<NodeId> = Vec::new();
            for existing in self.children(parent).into_iter().flatten() {
                if existing == child {
                    sequence.extend_from_slice(&incoming);
                } else {
                    sequence.push(existing);
                }
            }
            self.ensure_document_content_model(&sequence)?;
        }
        let previous = self.sibling(child, false);
        let mut reference = self.sibling(child, true);
        if reference == Some(node) {
            reference = self.sibling(node, true);
        }
        let added = self.incoming_nodes(node);
        // Adopt `node` first: removing it from its old parent stays
        // observable even though the replacement suppresses observers
        // (<https://dom.spec.whatwg.org/#concept-node-adopt>). Its connection
        // transitions coalesce, like every other move.
        self.record_unlink(node);
        // Remove `child` and insert `node` with observers suppressed
        // (<https://dom.spec.whatwg.org/#concept-node-replace>). The removal
        // still runs the removing steps, so iframe connection transitions
        // fire: a replaced iframe's frame must not survive.
        //
        // Both snapshots are taken before any removal: `node` can sit inside
        // `child`'s subtree, and detaching `child` first would read the wrong
        // starting state. Both are recorded after the whole operation, so a
        // disconnected-then-reinserted iframe reports no transition at all,
        // and the removal is recorded before the insertion so a replacement
        // frees its frame slot before the new frame is capped.
        let node_tracked = self.connection_snapshot(node);
        let child_tracked = (child != node && self.parent(child) == Some(parent))
            .then(|| self.connection_snapshot(child));
        let mut removed = Vec::new();
        self.recording_suppressed = true;
        if child != node && self.parent(child) == Some(parent) {
            self.unlink_from_current_parent(child);
            removed.push(child);
        }
        if self.is_fragment(node) {
            self.splice_fragment(parent, node, reference);
        } else {
            self.place_node(parent, node, reference);
        }
        self.recording_suppressed = false;
        if let Some(child_tracked) = child_tracked {
            self.record_snapshot(child_tracked);
        }
        self.record_snapshot(node_tracked);
        self.record(Mutation::ChildList {
            target: parent,
            added,
            removed,
            previous,
            next: reference,
        });
        Ok(())
    }

    /// Pre-insert validation steps 1–3 only: parent type, ancestor cycle,
    /// and reference membership
    /// (<https://dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity>).
    ///
    /// # Errors
    ///
    /// - [`DomError::HierarchyRequest`] when `parent` cannot contain children.
    /// - [`DomError::CycleForbidden`] if `node` is an ancestor of `parent`.
    /// - [`DomError::NoParent`] if `reference` is not a child of `parent`.
    pub fn validate_pre_insert(
        &self,
        parent: NodeId,
        node: NodeId,
        reference: Option<NodeId>,
    ) -> Result<(), DomError> {
        if !self.can_contain_children(parent) {
            return Err(DomError::HierarchyRequest);
        }
        if self.would_cycle(node, parent) {
            return Err(DomError::CycleForbidden);
        }
        if let Some(reference) = reference
            && self.parent(reference) != Some(parent)
        {
            return Err(DomError::NoParent);
        }
        Ok(())
    }

    /// [Pre-inserts](https://dom.spec.whatwg.org/#concept-node-pre-insert)
    /// `node` into `parent` before null or `reference`, validating in spec
    /// order and returning the node that was inserted.
    ///
    /// `append`/`insert_before` derive the parent from an existing sibling and
    /// serve trusted parser flows; this is the public web-visible entry point
    /// whose validation order (parent type, ancestor cycle, reference
    /// membership) tests observe.
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if any handle is stale.
    /// - [`DomError::HierarchyRequest`] if `parent` cannot contain children
    ///   or the content model refuses `node`.
    /// - [`DomError::CycleForbidden`] if `node` is an ancestor of `parent`.
    /// - [`DomError::NoParent`] if `reference` is not a child of `parent`.
    pub fn pre_insert(
        &mut self,
        parent: NodeId,
        node: NodeId,
        reference: Option<NodeId>,
    ) -> Result<(), DomError> {
        self.ensure_alive(parent, node)?;
        if !self.can_contain_children(parent) {
            return Err(DomError::HierarchyRequest);
        }
        if self.would_cycle(node, parent) {
            return Err(DomError::CycleForbidden);
        }
        if let Some(reference) = reference {
            self.require_live(reference)?;
            if self.parent(reference) != Some(parent) {
                return Err(DomError::NoParent);
            }
        }
        self.ensure_pre_insert_validity(parent, node, reference)?;
        // The reference child may be the node itself: inserting a node beside
        // itself is a legal stay-put no-op.
        let reference = if reference == Some(node) {
            self.sibling(node, true)
        } else {
            reference
        };
        let tracked = self.connection_snapshot(node);
        if self.is_fragment(node) {
            self.splice_fragment(parent, node, reference);
        } else {
            self.place_node(parent, node, reference);
        }
        self.record_snapshot(tracked);
        Ok(())
    }

    /// WHATWG DOM's *ensure pre-insert validity*: the one gate every
    /// insertion path walks (`append`, `insert_before`), mirroring
    /// <https://dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity>.
    /// Engine reference: Firefox's `EnsureAllowedAsChild` in
    /// `dom/base/nsINode.cpp` (mozilla-firefox/firefox).
    ///
    /// Comments map each refusal to its rule in the spec's current wording
    /// (the algorithm has been restructured before; anchors, not step
    /// numbers, are what stay true). Refusals here are always
    /// [`DomError::HierarchyRequest`] or [`DomError::CycleForbidden`]; the
    /// document content model itself is encoded exactly once, in
    /// [`Dom::ensure_document_content_model`].
    fn ensure_pre_insert_validity(
        &self,
        parent: NodeId,
        node: NodeId,
        reference: Option<NodeId>,
    ) -> Result<(), DomError> {
        // Container kinds only. Leaves with a child list would be arena corruption.
        // Kind stays borrowed: this gate is on the parse hot path.
        if !self.can_contain_children(parent) {
            return Err(DomError::HierarchyRequest);
        }
        // Only DocumentFragment, DocumentType, Element, and CharacterData
        // nodes are insertable; a Document node is refused here
        // (<https://dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity> step 4).
        if !self.is_insertable(node) {
            return Err(DomError::HierarchyRequest);
        }
        // Reference-child membership (the reference belongs to `parent`) is
        // enforced by construction: the only caller that passes `Some`
        // derives `parent` from that same handle's own parent pointer.
        //
        // Cycle rule: `node` must not be an inclusive ancestor of `parent`;
        // inserting a subtree into itself would tear the arena's acyclicity.
        if self.would_cycle(node, parent) {
            return Err(DomError::CycleForbidden);
        }
        // `insert_before` handles node-beside-itself (spec: do nothing)
        // before this gate.
        //
        // Content-model rules. Outside a document, a doctype as the
        // inserted node is never welcome; a fragment's children are not
        // checked here (the spec returns after the doctype-on-node
        // test: <https://dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity>).
        if !self.is_document(parent) {
            if !self.is_fragment(node) && self.is_doctype(node) {
                return Err(DomError::HierarchyRequest);
            }
            return Ok(());
        }
        // Inside a document: splice the incoming nodes (the node itself,
        // or a fragment's children —
        // <https://dom.spec.whatwg.org/#concept-node-insert>) into the
        // standing children at the insertion point and hand the whole
        // resulting sequence to the content model.
        let incoming = self.incoming_nodes(node);
        let mut sequence: Vec<NodeId> = Vec::new();
        let mut inserted = false;
        if let Some(kids) = self.children(parent) {
            for existing in kids {
                if !inserted && Some(existing) == reference {
                    sequence.extend_from_slice(&incoming);
                    inserted = true;
                }
                sequence.push(existing);
            }
        }
        if !inserted {
            sequence.extend_from_slice(&incoming);
        }
        self.ensure_document_content_model(&sequence)
    }

    /// Unlinks `id` from its parent, keeping the whole subtree alive.
    ///
    /// Idempotent: detaching an already-detached node succeeds. The document
    /// root cannot be detached.
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if `id` is stale.
    /// - [`DomError::HierarchyRequest`] for the document root.
    pub fn detach(&mut self, id: NodeId) -> Result<(), DomError> {
        self.require_live(id)?;
        if id == self.document {
            return Err(DomError::HierarchyRequest);
        }
        let tracked = self.connection_snapshot(id);
        self.unlink_from_current_parent(id);
        self.record_snapshot(tracked);
        Ok(())
    }

    /// Moves every child of `from` to the end of `to`'s child list.
    ///
    /// This is the bulk-move primitive behind foster parenting and the
    /// adoption agency: order is preserved and each moved child's parent
    /// pointer is updated. Endpoints must be container kinds, and draining
    /// the document root is refused. For non-document destinations the
    /// shape of the moved run stays unvalidated: the html5ever tree
    /// builder's trusted internal flows never emit content-model
    /// violations, and gated insertion paths make smuggling
    /// impossible (a doctype can only ever sit directly under the root,
    /// so none can appear in a moved run). When `to` **is** the document,
    /// the full document content model applies to the *resulting* sequence;
    /// see `Dom::ensure_document_content_model`. A bulk move is one
    /// operation: `[html, main]` into an empty document would pass
    /// per-child and fail as a pair.
    ///
    /// # Panics
    ///
    /// Only on an internal invariant defect (a verified-live node missing its
    /// slot), never on user input.
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if either handle is stale.
    /// - [`DomError::HierarchyRequest`] if `from` is the document root (the
    ///   move could then never complete without stranding the document),
    ///   if either endpoint is not a container kind, or if landing the run
    ///   in a document would break its content model.
    /// - [`DomError::CycleForbidden`] if `to` sits inside `from`'s subtree.
    pub fn reparent_children(&mut self, from: NodeId, to: NodeId) -> Result<(), DomError> {
        self.ensure_alive(from, to)?;
        if from == to {
            return Ok(());
        }
        // Both endpoints accept children, or the move would strand nodes under leaves.
        for endpoint in [from, to] {
            if !self.can_contain_children(endpoint) {
                return Err(DomError::HierarchyRequest);
            }
        }
        if from == self.document {
            // Draining the root would strand the entire document inside an
            // arbitrary detached subtree. (Maps to HierarchyRequestError.)
            return Err(DomError::HierarchyRequest);
        }
        if self.would_cycle(from, to) {
            return Err(DomError::CycleForbidden);
        }
        if self.is_document(to) {
            // The model sees the whole *resulting sequence*: the document's
            // standing children followed by the moved run.
            let mut sequence: Vec<NodeId> = Vec::new();
            if let Some(kids) = self.children(to) {
                sequence.extend(kids);
            }
            if let Some(kids) = self.children(from) {
                sequence.extend(kids);
            }
            self.ensure_document_content_model(&sequence)?;
        }
        let moved: Vec<NodeId> = self
            .children(from)
            .expect("verified-live `from` has a node")
            .collect();
        if moved.is_empty() {
            return Ok(());
        }
        // Defect guards, not input errors: both handles were verified live
        // above, so a miss here means the parent-pointer/sibling-link duality
        // is broken. Panicking beats reporting a lying "stale node".
        let from_connected = self.is_connected(from);
        let to_connected = self.is_connected(to);
        let tracked: Vec<(NodeId, bool, bool)> = if from_connected == to_connected {
            Vec::new()
        } else {
            moved
                .iter()
                .flat_map(|&id| self.connection_snapshot(id))
                .collect()
        };
        // Move the run one node at a time through the single-node primitives:
        // each `unlink`/`insert_linked` step is O(1), so the move stays O(k)
        // and the link invariant has exactly two writers.
        for &id in &moved {
            self.unlink(id);
        }
        for &id in &moved {
            self.insert_linked(to, id, None);
        }
        self.record_snapshot(tracked);
        Ok(())
    }

    /// The document content model over one candidate child sequence. No
    /// character data anywhere, at most one element child, at most one
    /// doctype placed strictly ahead of that element; comments may sit
    /// anywhere, and fragments stay opaque containers. Deliberately the *only* encoding of the model:
    /// incremental insertions arrive as their resulting sequence from
    /// `Dom::ensure_pre_insert_validity`, bulk moves as the document's
    /// standing children followed by the moved run. Per-child checks cannot
    /// see a violating pair like `[html, main]`.
    fn ensure_document_content_model(&self, sequence: &[NodeId]) -> Result<(), DomError> {
        let mut element_seen = false;
        let mut doctype_seen = false;
        for &id in sequence {
            match self.kind(id) {
                Some(NodeKind::Element { .. }) => {
                    if element_seen {
                        return Err(DomError::HierarchyRequest);
                    }
                    element_seen = true;
                }
                Some(NodeKind::Doctype { .. }) => {
                    if doctype_seen || element_seen {
                        return Err(DomError::HierarchyRequest);
                    }
                    doctype_seen = true;
                }
                Some(NodeKind::Text { .. }) => return Err(DomError::HierarchyRequest),
                _ => {}
            }
        }
        Ok(())
    }

    /// Adds each attribute that `id` does not already carry, matched by
    /// qualified name.
    ///
    /// The adapter's `add_attrs_if_missing` landing pad: html5ever merges
    /// attributes from repeated start-tag tokens through this call.
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if `id` is stale.
    /// - [`DomError::WrongNodeType`] if `id` is not an element.
    pub fn add_attrs_if_missing(
        &mut self,
        id: NodeId,
        attrs: Vec<Attribute>,
    ) -> Result<(), DomError> {
        let (_, attributes) = self.element_mut(id)?;
        merge_attrs(attributes, attrs);
        Ok(())
    }

    /// The value of the attribute whose qualified name is `local` on element
    /// `id`.
    ///
    /// [DOM getAttribute](https://dom.spec.whatwg.org/#dom-element-getattribute)
    /// after HTML’s ASCII-lowercase name conversion.
    #[must_use]
    pub fn attribute(&self, id: NodeId, local: &str) -> Option<String> {
        self.find_attribute(id, local)
            .map(|attribute| attribute.value.clone())
    }

    /// The live value of an HTML `input` element.
    ///
    /// Before the dirty value flag is set, the value follows the content
    /// attribute; setting the IDL value stores an independent value
    /// (<https://html.spec.whatwg.org/multipage/input.html#dom-input-value>).
    #[must_use]
    pub fn input_value(&self, id: NodeId) -> Option<String> {
        let typ = self.input_type(id)?;
        Some(match input_value_mode(&typ) {
            ValueMode::Value => {
                let value = self
                    .input_values
                    .get(&id)
                    .cloned()
                    .or_else(|| self.attribute(id, "value"))
                    .unwrap_or_default();
                self.sanitize_input_value(id, value)
            }
            ValueMode::Default => self.attribute(id, "value").unwrap_or_default(),
            ValueMode::DefaultOn => self
                .attribute(id, "value")
                .unwrap_or_else(|| "on".to_owned()),
            ValueMode::Filename => String::new(),
        })
    }

    /// Sets an HTML `input` element's live value and its dirty value flag.
    ///
    /// <https://html.spec.whatwg.org/multipage/input.html#dom-input-value>
    ///
    /// # Errors
    ///
    /// Returns [`DomError::WrongNodeType`] when `id` is not an HTML `input`,
    /// or [`DomError::InvalidState`] when setting a non-empty value on a
    /// `type=file` input.
    pub fn set_input_value(&mut self, id: NodeId, value: String) -> Result<(), DomError> {
        let typ = self.input_type(id).ok_or(DomError::WrongNodeType)?;
        match input_value_mode(&typ) {
            ValueMode::Value => {
                let value = self.sanitize_input_value(id, value);
                self.input_values.insert(id, value);
            }
            ValueMode::Default | ValueMode::DefaultOn => {
                self.set_attribute(id, "value", value)?;
            }
            ValueMode::Filename => {
                if !value.is_empty() {
                    return Err(DomError::InvalidState);
                }
            }
        }
        Ok(())
    }

    /// Runs the input type-change steps when `type` moves to `new_type`
    /// (<https://html.spec.whatwg.org/multipage/input.html#the-input-element:type-change-state>).
    ///
    /// # Errors
    ///
    /// Returns [`DomError::WrongNodeType`] when `id` is not an HTML `input`.
    pub fn input_type_change(&mut self, id: NodeId, new_type: &str) -> Result<(), DomError> {
        let old = self.input_type(id).ok_or(DomError::WrongNodeType)?;
        if old == new_type {
            return Ok(());
        }
        match (input_value_mode(&old), input_value_mode(new_type)) {
            // A value-mode value becomes the new default, unless it is empty.
            (ValueMode::Value, ValueMode::Default | ValueMode::DefaultOn) => {
                let value = self.input_value(id).unwrap_or_default();
                if !value.is_empty() {
                    self.set_attribute(id, "value", value)?;
                }
                self.input_values.remove(&id);
            }
            // A non-value state follows the content attribute again.
            (mode, ValueMode::Value) if mode != ValueMode::Value => {
                self.input_values.remove(&id);
            }
            // A non-filename state clears the value for a file input.
            (mode, ValueMode::Filename) if mode != ValueMode::Filename => {
                self.input_values.remove(&id);
            }
            _ => {}
        }
        Ok(())
    }

    /// The normalized type state of an HTML `input` element.
    #[must_use]
    pub fn input_type(&self, id: NodeId) -> Option<String> {
        self.input_value_element(id)?;
        let value = self
            .attribute(id, "type")
            .unwrap_or_else(|| "text".into())
            .to_ascii_lowercase();
        Some(if INPUT_TYPES.contains(&value.as_str()) {
            value
        } else {
            "text".into()
        })
    }

    fn input_value_element(&self, id: NodeId) -> Option<()> {
        let (name, _) = self.element(id)?;
        (name.ns == html_namespace() && name.local.as_ref() == "input").then_some(())
    }

    /// Applies the value sanitization algorithm for each input state whose
    /// value has a grammar. A string that does not match is replaced by the
    /// state's default (the empty string, or `#000000` for color)
    /// <https://html.spec.whatwg.org/multipage/input.html#value-sanitization-algorithm>.
    fn sanitize_input_value(&self, id: NodeId, value: String) -> String {
        let typ = self.input_type(id).unwrap_or_else(|| "text".into());
        match typ.as_str() {
            "text" | "search" | "tel" | "password" => value.replace(['\r', '\n'], ""),
            "url" | "email" => value
                .replace(['\r', '\n'], "")
                .trim_matches(|character: char| character.is_ascii_whitespace())
                .to_owned(),
            "number" => sanitize_grammar(value, is_valid_floating_point),
            "range" => sanitize_range_value(
                &value,
                self.attribute(id, "min").as_deref().and_then(parse_finite),
                self.attribute(id, "max").as_deref().and_then(parse_finite),
                self.attribute(id, "step").as_deref(),
            ),
            "date" => sanitize_grammar(value, is_valid_date),
            "month" => sanitize_grammar(value, is_valid_month),
            "week" => sanitize_grammar(value, is_valid_week),
            "time" => sanitize_grammar(value, is_valid_time),
            "datetime-local" => sanitize_local_date_time_value(&value),
            "color" => {
                if is_valid_simple_color(&value) {
                    value.to_ascii_lowercase()
                } else {
                    "#000000".to_owned()
                }
            }
            _ => value,
        }
    }

    /// Whether `id` is an HTML element with local name `local` (exact match).
    fn html_local_is(&self, id: NodeId, local: &str) -> bool {
        self.element(id).is_some_and(|(name, _)| {
            name.ns == html_namespace() && name.local.as_ref() == local
        })
    }

    /// The [child text content](https://dom.spec.whatwg.org/#concept-child-text-content)
    /// of node `id`: the concatenated data of its `Text` children.
    ///
    /// `CDATASection` is a `Text` node ([DOM](https://dom.spec.whatwg.org/#interface-cdatasection)),
    /// so its data is included. A stale handle has no children, so this
    /// yields the empty string.
    #[must_use]
    pub fn child_text_content(&self, id: NodeId) -> String {
        let mut text = String::new();
        if let Some(children) = self.children(id) {
            for child in children {
                if let Some(NodeKind::Text { data } | NodeKind::CDataSection { data }) =
                    self.kind(child)
                {
                    text.push_str(data);
                }
            }
        }
        text
    }

    /// The raw value of an HTML `textarea`: its stored raw value while the
    /// dirty value flag is set, otherwise its child text content.
    ///
    /// <https://html.spec.whatwg.org/multipage/form-elements.html#concept-textarea-raw-value>
    #[must_use]
    pub fn textarea_raw_value(&self, id: NodeId) -> Option<String> {
        if !self.html_local_is(id, "textarea") {
            return None;
        }
        Some(
            self.input_values
                .get(&id)
                .cloned()
                .unwrap_or_else(|| self.child_text_content(id)),
        )
    }

    /// The API value of an HTML `textarea`: its raw value with newlines
    /// normalized to LF.
    ///
    /// <https://html.spec.whatwg.org/multipage/form-elements.html#concept-fe-api-value>
    #[must_use]
    pub fn textarea_value(&self, id: NodeId) -> Option<String> {
        self.textarea_raw_value(id)
            .map(|value| normalize_newlines(&value))
    }

    /// Sets an HTML `textarea`'s raw value and dirty value flag.
    ///
    /// <https://html.spec.whatwg.org/multipage/form-elements.html#dom-textarea-value>
    ///
    /// # Errors
    ///
    /// Returns [`DomError::WrongNodeType`] when `id` is not an HTML `textarea`.
    pub fn set_textarea_value(&mut self, id: NodeId, value: String) -> Result<(), DomError> {
        if !self.html_local_is(id, "textarea") {
            return Err(DomError::WrongNodeType);
        }
        self.input_values.insert(id, value);
        Ok(())
    }

    /// The live value of a text-like control, as the `value` IDL attribute
    /// reports it: the API value for `input` and `textarea`.
    #[must_use]
    pub fn control_value(&self, id: NodeId) -> Option<String> {
        if self.html_local_is(id, "input") {
            self.input_value(id)
        } else {
            self.textarea_value(id)
        }
    }

    /// Sets the live value of a text-like control and its dirty value flag.
    ///
    /// When the API value changes, the text entry cursor moves to the end and
    /// the selection direction resets
    /// (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-textarea-value>,
    /// <https://html.spec.whatwg.org/multipage/input.html#dom-input-value>).
    ///
    /// # Errors
    ///
    /// Returns [`DomError::WrongNodeType`] when `id` is not an HTML `input` or
    /// `textarea`.
    pub fn set_control_value(&mut self, id: NodeId, value: String) -> Result<(), DomError> {
        let old = self.control_value(id);
        if self.html_local_is(id, "input") {
            self.set_input_value(id, value)?;
        } else {
            self.set_textarea_value(id, value)?;
        }
        if self.selection_supported(id) && self.control_value(id) != old {
            let end = self
                .control_value(id)
                .map_or(0, |value| utf16_length(&value));
            self.set_selection(id, end, end, 0);
        }
        Ok(())
    }

    /// Whether a text selection applies to `id`: an `HTMLTextAreaElement`, or
    /// an `HTMLInputElement` whose type is a text-entry type. Other input
    /// types report null and rejecting setters
    /// (<https://html.spec.whatwg.org/multipage/input.html#do-not-apply>).
    #[must_use]
    pub fn selection_supported(&self, id: NodeId) -> bool {
        if self.html_local_is(id, "textarea") {
            return true;
        }
        if !self.html_local_is(id, "input") {
            return false;
        }
        matches!(
            self.input_type(id).as_deref(),
            Some("text" | "search" | "tel" | "url" | "password")
        )
    }

    /// The stored selection `(start, end, direction)`, clamped to the current
    /// API value length. `None` when no text selection applies.
    #[must_use]
    pub fn selection(&self, id: NodeId) -> Option<(u32, u32, u8)> {
        if !self.selection_supported(id) {
            return None;
        }
        let length = self.control_value(id).map_or(0, |value| utf16_length(&value));
        let (start, end, direction) = self.selections.get(&id).copied().unwrap_or((0, 0, 0));
        Some((start.min(length), end.min(length), direction))
    }

    /// The API value of a non-dirty `textarea` `parent`, or `None` when
    /// `parent` is not such a control. Used to detect whether a child change
    /// really changed the value.
    fn textarea_value_before_change(&self, parent: NodeId) -> Option<String> {
        if !self.html_local_is(parent, "textarea") || self.input_values.contains_key(&parent) {
            return None;
        }
        self.textarea_value(parent)
    }

    /// A `textarea` with no stored raw value (dirty value flag unset) derives
    /// its value from its children, so a child change that alters the API value
    /// invalidates its selection: reset it to the start. A change that leaves
    /// the API value alone (for example one that only differs in raw newlines)
    /// keeps the selection. A dirty textarea keeps both its value and its
    /// selection.
    fn reset_textarea_selection_if_changed(&mut self, parent: NodeId, before: Option<String>) {
        let Some(before) = before else {
            return;
        };
        if self.textarea_value(parent).as_deref() != Some(before.as_str()) {
            self.selections.insert(parent, (0, 0, 0));
        }
    }

    /// Stores `id`'s selection, clamped to the current API value length, and
    /// reports whether the stored tuple actually changed (so the caller can
    /// queue a `select` event only for a real modification).
    pub fn set_selection(&mut self, id: NodeId, start: u32, end: u32, direction: u8) -> bool {
        if !self.selection_supported(id) {
            return false;
        }
        let length = self.control_value(id).map_or(0, |value| utf16_length(&value));
        let mut end = end.min(length);
        let mut start = start.min(length);
        // If end is less than or equal to start, both are placed immediately
        // before the character with offset end
        // (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#set-the-selection-range>).
        if end <= start {
            start = end;
        }
        end = end.max(start);
        let next = (start, end, direction.min(2));
        let previous = self.selections.get(&id).copied().unwrap_or((0, 0, 0));
        self.selections.insert(id, next);
        next != previous
    }

    /// Records the source-text start line of the inline `script` element `id`,
    /// as the parser counted it (1-based)
    /// (<https://html.spec.whatwg.org/multipage/webappapis.html#script's-line-number>).
    pub fn set_script_line(&mut self, id: NodeId, line: u32) {
        self.script_lines.insert(id, line);
    }

    /// The start line recorded for the inline `script` element `id`.
    #[must_use]
    pub fn script_line(&self, id: NodeId) -> Option<u32> {
        self.script_lines.get(&id).copied()
    }

    /// The scrolled offset `(left, top)` of `id`; `(0, 0)` when never set.
    #[must_use]
    pub fn scroll_offset(&self, id: NodeId) -> (f64, f64) {
        self.scroll_offsets.get(&id).copied().unwrap_or((0.0, 0.0))
    }

    /// Stores `id`'s scroll offset.
    pub fn set_scroll_offset(&mut self, id: NodeId, left: f64, top: f64) {
        self.scroll_offsets.insert(id, (left, top));
    }

    /// Whether `id`'s input type supported a text selection when its `type`
    /// last changed. The input default (`type=text`) is selectable.
    #[must_use]
    pub fn input_selectable(&self, id: NodeId) -> bool {
        self.input_selectable.get(&id).copied().unwrap_or(true)
    }

    /// Records whether `id`'s input type currently supports a text selection.
    pub fn set_input_selectable(&mut self, id: NodeId, selectable: bool) {
        self.input_selectable.insert(id, selectable);
    }

    /// The reset algorithm for a text-like control: clear the dirty value flag
    /// so the value reverts to its default
    /// (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#concept-form-reset-control>).
    pub fn reset_control(&mut self, id: NodeId) {
        if self.html_local_is(id, "input") || self.html_local_is(id, "textarea") {
            self.input_values.remove(&id);
            self.checkedness.remove(&id);
        }
    }

    /// An `input`'s checkedness: the stored value while the dirty checkedness
    /// flag is set, else whether the `checked` content attribute is present
    /// (<https://html.spec.whatwg.org/multipage/input.html#dom-input-checked>).
    #[must_use]
    pub fn checkedness(&self, id: NodeId) -> bool {
        self.checkedness
            .get(&id)
            .copied()
            .unwrap_or_else(|| self.attribute(id, "checked").is_some())
    }

    /// Sets `id`'s checkedness and the dirty checkedness flag.
    pub fn set_checkedness(&mut self, id: NodeId, checked: bool) {
        self.checkedness.insert(id, checked);
    }

    /// Sets an input's checkedness, unchecking the rest of a radio button
    /// group when checking a radio
    /// (<https://html.spec.whatwg.org/multipage/input.html#radio-button-state-(type=radio)>).
    pub fn set_input_checkedness(&mut self, id: NodeId, checked: bool) {
        if checked
            && self.input_type(id).as_deref() == Some("radio")
            && let Some(name) = self.attribute(id, "name")
            && !name.is_empty()
        {
            let owner = self.nearest_form_ancestor(id);
            let scope = owner.unwrap_or_else(|| self.tree_root_of(id));
            let others: Vec<NodeId> = self
                .descendants(scope)
                .filter(|&other| {
                    other != id
                        && self.is_radio_named(other, &name)
                        && self.nearest_form_ancestor(other) == owner
                })
                .collect();
            for other in others {
                self.checkedness.insert(other, false);
            }
        }
        self.checkedness.insert(id, checked);
    }

    /// The form owner of a form-associated element: the form named by its
    /// `form` attribute, else its nearest ancestor form
    /// (<https://html.spec.whatwg.org/multipage/forms.html#form-owner>).
    #[must_use]
    pub fn form_owner(&self, id: NodeId) -> Option<NodeId> {
        if let Some(reference) = self.attribute(id, "form")
            && !reference.is_empty()
        {
            let root = self.tree_root_of(id);
            let found = self.descendants(root).find(|&other| {
                self.html_local_is(other, "form")
                    && self.attribute(other, "id").as_deref() == Some(reference.as_str())
            });
            if found.is_some() {
                return found;
            }
        }
        self.nearest_form_ancestor(id)
    }

    /// The checked radio in `id`'s radio button group, if any.
    #[must_use]
    pub fn radio_group_checked(&self, id: NodeId) -> Option<NodeId> {
        let name = self.attribute(id, "name")?;
        if name.is_empty() {
            return None;
        }
        let owner = self.nearest_form_ancestor(id);
        let scope = owner.unwrap_or_else(|| self.tree_root_of(id));
        self.descendants(scope).find(|&other| {
            other != id && self.is_radio_named(other, &name) && self.checkedness(other)
        })
    }

    /// The nearest ancestor `form` of `node`, if any.
    fn nearest_form_ancestor(&self, node: NodeId) -> Option<NodeId> {
        let mut current = self.parent(node);
        while let Some(parent) = current {
            if self.html_local_is(parent, "form") {
                return Some(parent);
            }
            current = self.parent(parent);
        }
        None
    }

    /// The root of `node`'s tree, for grouping radios outside any form.
    fn tree_root_of(&self, node: NodeId) -> NodeId {
        let mut current = node;
        while let Some(parent) = self.parent(current) {
            current = parent;
        }
        current
    }

    /// Whether `node` is a radio with the given group name.
    fn is_radio_named(&self, node: NodeId, name: &str) -> bool {
        self.input_type(node).as_deref() == Some("radio")
            && self.attribute(node, "name").as_deref() == Some(name)
    }

    /// An `input`'s indeterminateness
    /// (<https://html.spec.whatwg.org/multipage/input.html#concept-input-indeterminate>).
    #[must_use]
    pub fn indeterminate(&self, id: NodeId) -> bool {
        self.indeterminate.get(&id).copied().unwrap_or(false)
    }

    /// Sets `id`'s indeterminateness.
    pub fn set_indeterminate(&mut self, id: NodeId, indeterminate: bool) {
        self.indeterminate.insert(id, indeterminate);
    }

    /// Records the focused element for a document, backing `:focus`.
    pub fn set_active_element(&mut self, document: u32, node: Option<NodeId>) {
        match node {
            Some(node) => {
                self.active_element.insert(document, node);
            }
            None => {
                self.active_element.remove(&document);
            }
        }
    }

    /// The focused element for a document, if any.
    #[must_use]
    pub fn active_element(&self, document: u32) -> Option<NodeId> {
        self.active_element.get(&document).copied()
    }

    /// The input/option cloning steps: propagate value, dirty value, and
    /// checkedness state from `from` to `to`
    /// (<https://html.spec.whatwg.org/multipage/input.html#the-input-element:cloning-steps>).
    pub fn clone_form_state(&mut self, from: NodeId, to: NodeId) {
        if let Some(value) = self.input_values.get(&from).cloned() {
            self.input_values.insert(to, value);
        }
        if let Some(checked) = self.checkedness.get(&from).copied() {
            self.checkedness.insert(to, checked);
        }
        if let Some(indeterminate) = self.indeterminate.get(&from).copied() {
            self.indeterminate.insert(to, indeterminate);
        }
        if let Some(selected) = self.option_selectedness.get(&from).copied() {
            self.option_selectedness.insert(to, selected);
        }
        if self.option_dirty_selected.contains(&from) {
            self.option_dirty_selected.insert(to);
        }
    }

    /// An `option`'s selectedness: the stored value while the dirty
    /// selectedness flag is set, else whether `selected` is present.
    #[must_use]
    pub fn option_selected(&self, id: NodeId) -> bool {
        self.option_selectedness
            .get(&id)
            .copied()
            .unwrap_or_else(|| self.attribute(id, "selected").is_some())
    }

    /// Sets `id`'s selectedness and the dirty selectedness flag.
    pub fn set_option_selected(&mut self, id: NodeId, selected: bool) {
        self.option_selectedness.insert(id, selected);
        self.option_dirty_selected.insert(id);
    }

    /// Sets `id`'s selectedness without the dirty flag, as the `Option`
    /// constructor does.
    pub fn set_option_selectedness(&mut self, id: NodeId, selected: bool) {
        self.option_selectedness.insert(id, selected);
    }

    /// Re-reads an `option`'s selectedness from its `selected` attribute when
    /// the dirty flag is clear.
    fn refresh_option_selectedness(&mut self, id: NodeId) {
        if self.html_local_is(id, "option") && !self.option_dirty_selected.contains(&id) {
            let selected = self.attribute(id, "selected").is_some();
            self.option_selectedness.insert(id, selected);
        }
    }

    /// The `select` ancestor of an `option`, if any.
    #[must_use]
    pub fn option_select_owner(&self, option: NodeId) -> Option<NodeId> {
        let mut current = self.parent(option);
        while let Some(parent) = current {
            if self.html_local_is(parent, "select") {
                return Some(parent);
            }
            current = self.parent(parent);
        }
        None
    }

    /// Sets an option's selectedness; selecting an option in a single-select
    /// clears the others.
    pub fn set_option_selected_in_select(&mut self, option: NodeId, selected: bool) {
        if selected
            && let Some(select) = self.option_select_owner(option)
            && self.attribute(select, "multiple").is_none()
        {
            for other in self.select_options(select) {
                self.option_selectedness.insert(other, false);
                self.option_dirty_selected.insert(other);
            }
        }
        self.option_selectedness.insert(option, selected);
        self.option_dirty_selected.insert(option);
    }

    /// The text content of `id`: every descendant text node's data.
    #[must_use]
    pub fn text_content(&self, id: NodeId) -> String {
        let mut text = String::new();
        for node in self.descendants(id) {
            if let Some(NodeKind::Text { data } | NodeKind::CDataSection { data }) = self.kind(node)
            {
                text.push_str(data);
            }
        }
        text
    }

    /// The `option` elements under a `select`, in tree order.
    #[must_use]
    pub fn select_options(&self, select: NodeId) -> Vec<NodeId> {
        if !self.html_local_is(select, "select") {
            return Vec::new();
        }
        self.descendants(select)
            .filter(|&id| self.html_local_is(id, "option"))
            .collect()
    }

    /// An `option`'s value: its `value` attribute, else its text content
    /// (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-value>).
    #[must_use]
    pub fn option_value(&self, id: NodeId) -> String {
        self.no_namespace_attribute(id, "value")
            .unwrap_or_else(|| self.option_text(id))
    }

    /// An `option`'s text: its child text content with ASCII whitespace
    /// stripped and collapsed, skipping HTML and SVG `script` subtrees
    /// (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-text>).
    #[must_use]
    pub fn option_text(&self, id: NodeId) -> String {
        let mut text = String::new();
        self.collect_option_text(id, &mut text);
        collapse_whitespace(&text)
    }

    /// Appends the option text under `id`, skipping HTML and SVG `script`.
    fn collect_option_text(&self, id: NodeId, text: &mut String) {
        let Some(children) = self.children(id) else {
            return;
        };
        for child in children {
            match self.kind(child) {
                Some(NodeKind::Text { data } | NodeKind::CDataSection { data }) => {
                    text.push_str(data);
                }
                Some(NodeKind::Element { name, .. }) => {
                    let is_script = name.local.as_ref().eq_ignore_ascii_case("script");
                    let skippable = name.ns == html_namespace()
                        || name.ns.as_ref() == "http://www.w3.org/2000/svg";
                    if !(is_script && skippable) {
                        self.collect_option_text(child, text);
                    }
                }
                _ => {}
            }
        }
    }

    /// An attribute in no namespace with the given local name, as HTML
    /// `getAttribute` matches
    /// (<https://dom.spec.whatwg.org/#concept-element-attributes-get-by-name>).
    #[must_use]
    pub fn no_namespace_attribute(&self, id: NodeId, local: &str) -> Option<String> {
        let (_, attributes) = self.element(id)?;
        attributes
            .iter()
            .find(|attribute| {
                attribute.name.ns.as_ref().is_empty() && attribute.name.local.as_ref() == local
            })
            .map(|attribute| attribute.value.clone())
    }

    /// A `select`'s value: the first selected option's value, else the empty
    /// string (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-value>).
    #[must_use]
    pub fn select_value(&self, id: NodeId) -> String {
        let options = self.select_options(id);
        for &option in &options {
            if self.option_selected(option) {
                return self.option_value(option);
            }
        }
        // A single-select keeps one option selected; the first is the default.
        if self.attribute(id, "multiple").is_none()
            && let Some(&first) = options.first()
        {
            return self.option_value(first);
        }
        String::new()
    }

    /// A `select`'s selected index: the first selected option's index, else -1.
    #[must_use]
    pub fn select_selected_index(&self, id: NodeId) -> i32 {
        let options = self.select_options(id);
        for (index, &option) in options.iter().enumerate() {
            if self.option_selected(option) {
                return i32::try_from(index).unwrap_or(i32::MAX);
            }
        }
        if self.attribute(id, "multiple").is_none() && !options.is_empty() {
            return 0;
        }
        -1
    }

    /// Selects the option at `index`; a single-select clears the others first.
    pub fn set_select_selected_index(&mut self, id: NodeId, index: i32) {
        let options = self.select_options(id);
        if self.attribute(id, "multiple").is_none() {
            for &option in &options {
                self.option_selectedness.insert(option, false);
                self.option_dirty_selected.insert(option);
            }
        }
        if let Ok(index) = usize::try_from(index)
            && let Some(&option) = options.get(index)
        {
            self.option_selectedness.insert(option, true);
            self.option_dirty_selected.insert(option);
        }
    }

    /// Selects the first option whose value is `value`, clearing the rest
    /// (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-value>).
    pub fn set_select_value(&mut self, id: NodeId, value: &str) {
        let options = self.select_options(id);
        for &option in &options {
            self.option_selectedness.insert(option, false);
            self.option_dirty_selected.insert(option);
        }
        for option in options {
            if self.option_value(option) == value {
                self.option_selectedness.insert(option, true);
                self.option_dirty_selected.insert(option);
                break;
            }
        }
    }

    /// The `value` IDL value for any element that has one: `option`, `select`,
    /// or a text-like control.
    #[must_use]
    pub fn element_value(&self, id: NodeId) -> Option<String> {
        if self.html_local_is(id, "option") {
            Some(self.option_value(id))
        } else if self.html_local_is(id, "select") {
            Some(self.select_value(id))
        } else {
            self.control_value(id)
        }
    }

    /// Sets the `value` IDL value for `option`, `select`, or a text-like
    /// control.
    ///
    /// # Errors
    ///
    /// Returns [`DomError::WrongNodeType`] when `id` has no `value`.
    pub fn set_element_value(&mut self, id: NodeId, value: String) -> Result<(), DomError> {
        if self.html_local_is(id, "option") {
            return self.set_attribute(id, "value", value);
        }
        if self.html_local_is(id, "select") {
            self.set_select_value(id, &value);
            return Ok(());
        }
        self.set_control_value(id, value)
    }

    /// The `defaultValue` of a text-like control: the `value` content
    /// attribute for `input`, the child text content for `textarea`.
    ///
    /// <https://html.spec.whatwg.org/multipage/form-elements.html#dom-textarea-defaultvalue>
    #[must_use]
    pub fn control_default_value(&self, id: NodeId) -> Option<String> {
        if self.html_local_is(id, "input") {
            Some(self.attribute(id, "value").unwrap_or_default())
        } else if self.html_local_is(id, "textarea") {
            Some(self.child_text_content(id))
        } else {
            None
        }
    }

    /// Sets the `defaultValue` of a text-like control: assigning the `value`
    /// content attribute for `input`, string-replacing all children for
    /// `textarea`.
    ///
    /// <https://html.spec.whatwg.org/multipage/form-elements.html#dom-textarea-defaultvalue>
    ///
    /// # Errors
    ///
    /// Returns [`DomError::WrongNodeType`] when `id` is not an HTML `input` or
    /// `textarea`.
    pub fn set_control_default_value(&mut self, id: NodeId, value: String) -> Result<(), DomError> {
        if self.html_local_is(id, "input") {
            return self.set_attribute(id, "value", value);
        }
        if !self.html_local_is(id, "textarea") {
            return Err(DomError::WrongNodeType);
        }
        let replacement = self.create_fragment();
        if !value.is_empty() {
            let text = self.create_text(value);
            self.append(replacement, text)?;
        }
        self.replace_all(id, replacement)
    }

    /// [Element.hasAttribute](https://dom.spec.whatwg.org/#dom-element-hasattribute):
    /// exact local-name match; HTML elements lowercase the queried name.
    #[must_use]
    pub fn has_attribute(&self, id: NodeId, local: &str) -> bool {
        self.find_attribute(id, local).is_some()
    }

    /// [Element.getAttributeNS](https://dom.spec.whatwg.org/#dom-element-getattributens):
    /// exact namespace and local-name match, prefix ignored.
    #[must_use]
    pub fn attribute_ns(&self, id: NodeId, ns: &str, local: &str) -> Option<String> {
        self.element(id)?.1.iter().find_map(|attribute| {
            (attribute.name.ns.as_ref() == ns && attribute.name.local.as_ref() == local)
                .then(|| attribute.value.clone())
        })
    }

    /// [Element.getAttributeNames](https://dom.spec.whatwg.org/#dom-element-getattributenames):
    /// qualified names in attribute order.
    #[must_use]
    pub fn attribute_names(&self, id: NodeId) -> Vec<String> {
        self.element(id)
            .map(|(_, attributes)| {
                attributes
                    .iter()
                    .map(|attribute| Self::serialize_qualified_name(&attribute.name))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The element's attribute list, or `None` when `id` is stale or not an
    /// element. Used by the `NamedNodeMap` platform object.
    #[must_use]
    pub fn attributes(&self, id: NodeId) -> Option<&[Attribute]> {
        self.element(id).map(|(_, attributes)| attributes)
    }

    /// [Element.removeAttribute](https://dom.spec.whatwg.org/#dom-element-removeattribute).
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if `id` is stale.
    /// - [`DomError::WrongNodeType`] if `id` is not an element.
    pub fn remove_attribute(&mut self, id: NodeId, local: &str) -> Result<(), DomError> {
        let (name, attributes) = self.element_mut(id)?;
        let Some(index) = attributes
            .iter()
            .position(|attribute| Self::attr_query_eq(&name.ns, &attribute.name, local))
        else {
            return Ok(());
        };
        let removed = attributes.remove(index);
        // `MutationRecord.attributeName` is the attribute's local
        // name and `attributeNamespace` its namespace, not the
        // queried qualified name
        // (<https://dom.spec.whatwg.org/#dom-mutationrecord-attributename>).
        self.record(Mutation::Attributes {
            target: id,
            name: removed.name.local.to_string(),
            namespace: removed.name.ns.to_string(),
            old_value: Some(removed.value),
        });
        if local == "selected" {
            self.refresh_option_selectedness(id);
        }
        Ok(())
    }

    /// [Element.removeAttributeNS](https://dom.spec.whatwg.org/#dom-element-removeattributens).
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if `id` is stale.
    /// - [`DomError::WrongNodeType`] if `id` is not an element.
    pub fn remove_attribute_ns(
        &mut self,
        id: NodeId,
        ns: &str,
        local: &str,
    ) -> Result<(), DomError> {
        let (_, attributes) = self.element_mut(id)?;
        let Some(index) = attributes.iter().position(|attribute| {
            attribute.name.ns.as_ref() == ns && attribute.name.local.as_ref() == local
        }) else {
            return Ok(());
        };
        let removed = attributes.remove(index);
        self.record(Mutation::Attributes {
            target: id,
            name: local.to_owned(),
            namespace: ns.to_owned(),
            old_value: Some(removed.value),
        });
        Ok(())
    }

    /// Replaces every child of `parent` with `node`
    /// (<https://dom.spec.whatwg.org/#concept-node-replace-all>). Removed
    /// children stay alive, detached, like the spec's remove step.
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if either handle is stale.
    /// - [`DomError::HierarchyRequest`] if `parent` cannot contain children,
    ///   `node` is a doctype outside a document, or the document content
    ///   model refuses the replacement.
    /// - [`DomError::CycleForbidden`] if `node` is an ancestor of `parent`.
    ///
    /// # Panics
    ///
    /// Only on an internal invariant defect (a verified-live node missing its
    /// slot), never on user input.
    pub fn replace_all(&mut self, parent: NodeId, node: NodeId) -> Result<(), DomError> {
        self.ensure_alive(parent, node)?;
        if !self.can_contain_children(parent) {
            return Err(DomError::HierarchyRequest);
        }
        if self.would_cycle(node, parent) {
            return Err(DomError::CycleForbidden);
        }
        if self.is_document(parent) {
            let incoming = self.incoming_nodes(node);
            self.ensure_document_content_model(&incoming)?;
        } else if !self.is_fragment(node) && self.is_doctype(node) {
            return Err(DomError::HierarchyRequest);
        }
        let parent_connected = self.is_connected(parent);
        let removed: Vec<NodeId> = self
            .children(parent)
            .map(Iterator::collect)
            .unwrap_or_default();
        let removed_snapshot: Vec<(NodeId, bool, bool)> = if parent_connected {
            removed
                .iter()
                .flat_map(|&id| self.connection_snapshot(id))
                .collect()
        } else {
            Vec::new()
        };
        let added = self.incoming_nodes(node);
        // `node` can be one of the removed children (JS `replaceChildren(
        // firstChild)`), so its starting connectivity must be read before the
        // detach loop below.
        let node_tracked = self.connection_snapshot(node);
        self.recording_suppressed = true;
        // Detach the standing children through the single-node primitive; its
        // O(1) steps make the loop O(k), and it keeps `unlink` the only
        // remover of a node. Recording is suppressed, so no observer sees
        // these individual removals.
        for &kid in &removed {
            self.unlink_from_current_parent(kid);
        }
        if self.is_fragment(node) {
            self.splice_fragment(parent, node, None);
        } else {
            self.place_node(parent, node, None);
        }
        self.recording_suppressed = false;
        // Removal before insertion: a replacement frees its frame slot before
        // the new frame is checked against the frame cap.
        self.record_snapshot(removed_snapshot);
        self.record_snapshot(node_tracked);
        // "If either addedNodes or removedNodes is not empty, then queue a
        // tree mutation record" (<https://dom.spec.whatwg.org/#concept-node-replace-all>):
        // e.g. `textContent = ""` on an already-empty element changes nothing
        // and is silent.
        if !added.is_empty() || !removed.is_empty() {
            self.record(Mutation::ChildList {
                target: parent,
                added,
                removed,
                previous: None,
                next: None,
            });
        }
        Ok(())
    }

    /// Sets an attribute identified by namespace and local name, replacing
    /// the first attribute with that namespace and local name (the existing
    /// prefix is kept, matching "set an attribute value").
    ///
    /// [DOM setAttributeNS](https://dom.spec.whatwg.org/#dom-element-setattributens)
    /// and `setAttributeNode` land here.
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if `id` is stale.
    /// - [`DomError::WrongNodeType`] if `id` is not an element.
    pub fn set_attribute_by_ns(
        &mut self,
        id: NodeId,
        namespace: &str,
        prefix: Option<&str>,
        local: &str,
        value: impl Into<String>,
    ) -> Result<(), DomError> {
        let value = value.into();
        let (_, attributes) = self.element_mut(id)?;
        let index = attributes.iter().position(|attribute| {
            attribute.name.ns.as_ref() == namespace && attribute.name.local.as_ref() == local
        });
        let old_value = if let Some(index) = index {
            Some(std::mem::replace(&mut attributes[index].value, value))
        } else {
            attributes.push(Attribute {
                name: QualName::new(
                    prefix.map(Prefix::from),
                    Namespace::from(namespace),
                    LocalName::from(local),
                ),
                value,
            });
            None
        };
        self.record(Mutation::Attributes {
            target: id,
            name: local.to_owned(),
            namespace: namespace.to_owned(),
            old_value,
        });
        Ok(())
    }

    /// Sets the unnamespaced attribute `local` on element `id`, replacing a
    /// same-name attribute if one exists.
    ///
    /// [DOM setAttribute](https://dom.spec.whatwg.org/#dom-element-setattribute)
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if `id` is stale.
    /// - [`DomError::WrongNodeType`] if `id` is not an element.
    pub fn set_attribute(
        &mut self,
        id: NodeId,
        local: &str,
        value: impl Into<String>,
    ) -> Result<(), DomError> {
        let value = value.into();
        let (name, attributes) = self.element_mut(id)?;
        let html = name.ns == html_namespace();
        let existing = attributes
            .iter()
            .position(|attribute| Self::attr_query_eq(&name.ns, &attribute.name, local));
        // A matched attribute can carry a namespace even though the query is
        // unnamespaced (`setAttribute("xlink:href", …)` on an SVG element);
        // the record reports the changed attribute's real name and namespace
        // (<https://dom.spec.whatwg.org/#dom-mutationrecord-attributename>).
        let (recorded_name, recorded_namespace, old_value) = if let Some(index) = existing {
            let attribute = &mut attributes[index];
            let old_value = std::mem::replace(&mut attribute.value, value);
            (
                attribute.name.local.to_string(),
                attribute.name.ns.to_string(),
                Some(old_value),
            )
        } else {
            let local = if html {
                local.to_ascii_lowercase()
            } else {
                local.to_owned()
            };
            attributes.push(Attribute {
                name: QualName::new(None, Namespace::from(""), LocalName::from(local.as_str())),
                value,
            });
            (local, String::new(), None)
        };
        self.record(Mutation::Attributes {
            target: id,
            name: recorded_name,
            namespace: recorded_namespace,
            old_value,
        });
        if local == "selected" {
            self.refresh_option_selectedness(id);
        }
        Ok(())
    }

    /// Replaces the data of the text node `id`.
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if `id` is stale.
    /// - [`DomError::WrongNodeType`] if `id` is not a text node.
    pub fn set_text(&mut self, id: NodeId, data: impl Into<String>) -> Result<(), DomError> {
        self.set_data(
            id,
            |kind| match kind {
                NodeKind::Text { data } => Some(data),
                _ => None,
            },
            data.into(),
        )
    }

    /// Appends `extra` to the text node `id`.
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if `id` is stale.
    /// - [`DomError::WrongNodeType`] if `id` is not a text node.
    pub fn append_text(&mut self, id: NodeId, extra: &str) -> Result<(), DomError> {
        let parent = self.parent(id);
        let value_before = parent.and_then(|parent| self.textarea_value_before_change(parent));
        let recording = self.record_mutations && !self.recording_suppressed;
        let old_value = {
            let node = self.node_mut(id).ok_or(DomError::StaleNode)?;
            let NodeKind::Text { data } = &mut node.kind else {
                return Err(DomError::WrongNodeType);
            };
            let old_value = recording.then(|| data.clone());
            data.push_str(extra);
            old_value
        };
        if let Some(old_value) = old_value {
            self.record(Mutation::CharacterData {
                target: id,
                old_value,
            });
        }
        if let Some(parent) = parent {
            self.reset_textarea_selection_if_changed(parent, value_before);
        }
        Ok(())
    }

    /// Replaces the data of the comment node `id`.
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if `id` is stale.
    /// - [`DomError::WrongNodeType`] if `id` is not a comment node.
    pub fn set_comment(&mut self, id: NodeId, data: impl Into<String>) -> Result<(), DomError> {
        self.set_data(
            id,
            |kind| match kind {
                NodeKind::Comment { data } => Some(data),
                _ => None,
            },
            data.into(),
        )
    }

    /// Replaces the data of the CDATA section `id`.
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if `id` is stale.
    /// - [`DomError::WrongNodeType`] if `id` is not a CDATA section.
    pub fn set_cdata_section(
        &mut self,
        id: NodeId,
        data: impl Into<String>,
    ) -> Result<(), DomError> {
        self.set_data(
            id,
            |kind| match kind {
                NodeKind::CDataSection { data } => Some(data),
                _ => None,
            },
            data.into(),
        )
    }

    /// Replaces the data of the processing instruction `id`.
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if `id` is stale.
    /// - [`DomError::WrongNodeType`] if `id` is not a processing instruction.
    pub fn set_processing_instruction(
        &mut self,
        id: NodeId,
        data: impl Into<String>,
    ) -> Result<(), DomError> {
        self.set_data(
            id,
            |kind| match kind {
                NodeKind::ProcessingInstruction { data, .. } => Some(data),
                _ => None,
            },
            data.into(),
        )
    }

    /// Destroys `id` and its entire subtree, recycling their slots.
    ///
    /// Every handle into the destroyed region goes stale at once. Destroying
    /// the document root is refused.
    ///
    /// A connected subtree's `iframe` connection transitions are recorded,
    /// even though the nodes are gone by the time they are reported.
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if `id` is stale.
    /// - [`DomError::HierarchyRequest`] for the document root.
    pub fn destroy(&mut self, id: NodeId) -> Result<(), DomError> {
        self.require_live(id)?;
        if id == self.document {
            return Err(DomError::HierarchyRequest);
        }
        // Read connectivity before unlinking; the snapshot carries the
        // iframe-ness so the count still moves once the slots are freed.
        let tracked = self.connection_snapshot(id);
        self.unlink_from_current_parent(id);

        let mut pending = vec![id];
        while let Some(current) = pending.pop() {
            self.input_values.remove(&current);
            self.selections.remove(&current);
            self.script_lines.remove(&current);
            self.checkedness.remove(&current);
            self.indeterminate.remove(&current);
            self.option_selectedness.remove(&current);
            self.option_dirty_selected.remove(&current);
            self.scroll_offsets.remove(&current);
            self.input_selectable.remove(&current);
            if let Some(contents) = self.template_contents.remove(&current) {
                pending.push(contents);
            }
            if let Some((root, _)) = self.shadow_roots.remove(&current) {
                self.shadow_hosts.remove(&root);
                pending.push(root);
            }
            if let Some(host) = self.shadow_hosts.remove(&current) {
                self.shadow_roots.remove(&host);
            }
            self.template_contents
                .retain(|_, contents| *contents != current);
            // Collect the child run through the links while the slot is still
            // populated. Descendants are processed later, so their own links
            // are intact when their turn comes.
            let mut child = self.slots[current.index()]
                .node
                .as_ref()
                .and_then(|node| node.first_child);
            while let Some(id) = child {
                pending.push(id);
                child = self.slots[id.index()]
                    .node
                    .as_ref()
                    .and_then(|node| node.next_sibling);
            }
            self.slots[current.index()].node = None;
            // No generation tick here: emptiness is what makes the handle
            // dead (`live_slot` requires `node.is_some()`), and the single
            // tick happens at reallocation in `alloc`.
            self.free.push(current.slot);
        }
        self.record_snapshot(tracked);
        Ok(())
    }

    // ── internals ────────────────────────────────────────────────────────

    fn live_slot(&self, id: NodeId) -> Option<&Slot> {
        if id.document != self.document.document {
            return None;
        }
        self.slots
            .get(id.index())
            .filter(|slot| slot.generation == id.generation && slot.node.is_some())
    }

    fn node_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        if id.document != self.document.document {
            return None;
        }
        self.slots
            .get_mut(id.index())
            .filter(|slot| slot.generation == id.generation)
            .and_then(|slot| slot.node.as_mut())
    }

    /// Whether `id` is one of the container kinds that may hold children.
    fn can_contain_children(&self, id: NodeId) -> bool {
        matches!(
            self.kind(id),
            Some(NodeKind::Document | NodeKind::Element { .. } | NodeKind::Fragment)
        )
    }

    /// The kinds DOM's *ensure pre-insert validity* accepts as an inserted
    /// node: everything but a document
    /// (<https://dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity>).
    fn is_insertable(&self, id: NodeId) -> bool {
        matches!(
            self.kind(id),
            Some(
                NodeKind::Fragment
                    | NodeKind::Doctype { .. }
                    | NodeKind::Element { .. }
                    | NodeKind::Text { .. }
                    | NodeKind::CDataSection { .. }
                    | NodeKind::ProcessingInstruction { .. }
                    | NodeKind::Comment { .. }
            )
        )
    }

    fn is_document(&self, id: NodeId) -> bool {
        matches!(self.kind(id), Some(NodeKind::Document))
    }

    fn is_doctype(&self, id: NodeId) -> bool {
        matches!(self.kind(id), Some(NodeKind::Doctype { .. }))
    }

    fn is_fragment(&self, id: NodeId) -> bool {
        matches!(self.kind(id), Some(NodeKind::Fragment))
    }

    /// The element's qualified name and attribute list, or `None` when `id`
    /// is stale or not an element.
    pub(crate) fn element(&self, id: NodeId) -> Option<(&QualName, &[Attribute])> {
        match self.kind(id)? {
            NodeKind::Element { name, attributes } => Some((name, attributes)),
            _ => None,
        }
    }

    /// Mutable variant of [`Dom::element`] for the attribute mutators.
    fn element_mut(&mut self, id: NodeId) -> Result<(&QualName, &mut Vec<Attribute>), DomError> {
        match &mut self.node_mut(id).ok_or(DomError::StaleNode)?.kind {
            NodeKind::Element { name, attributes } => Ok((name, attributes)),
            _ => Err(DomError::WrongNodeType),
        }
    }

    /// A qualified name's serialization: `prefix:local` or just `local`.
    fn serialize_qualified_name(name: &QualName) -> String {
        match &name.prefix {
            Some(prefix) if !prefix.is_empty() => format!("{prefix}:{}", name.local),
            _ => name.local.to_string(),
        }
    }

    fn attr_query_eq(element_ns: &Namespace, name: &QualName, query: &str) -> bool {
        if *element_ns == html_namespace() {
            html_qualified_name_eq(name, query)
        } else {
            qualified_name_eq(name, query)
        }
    }

    /// The first attribute on `id` whose **qualified name** matches `local`
    /// under the element's case regime; HTML elements ASCII-lowercase the
    /// queried name first
    /// ([DOM get-an-attribute-by-name](https://dom.spec.whatwg.org/#concept-element-attributes-get-by-name)).
    fn find_attribute<'a>(&'a self, id: NodeId, local: &str) -> Option<&'a Attribute> {
        let (name, attributes) = self.element(id)?;
        attributes
            .iter()
            .find(|attribute| Self::attr_query_eq(&name.ns, &attribute.name, local))
    }

    fn is_html_template_element(&self, id: NodeId) -> bool {
        match self.kind(id) {
            Some(NodeKind::Element { name, .. }) => {
                name.ns == html_namespace() && name.local.as_ref().eq_ignore_ascii_case("template")
            }
            _ => false,
        }
    }

    /// Nodes the insert algorithm actually places: a fragment's children,
    /// otherwise the node itself
    /// (<https://dom.spec.whatwg.org/#concept-node-insert>).
    fn incoming_nodes(&self, node: NodeId) -> Vec<NodeId> {
        if self.is_fragment(node) {
            self.children(node)
                .map(Iterator::collect)
                .unwrap_or_default()
        } else {
            vec![node]
        }
    }

    /// Places a non-fragment `node` under `parent` before `before` (or at
    /// the end when `before` is `None`).
    ///
    /// Structural only: lifecycle recording belongs to the calling operation,
    /// which captures one snapshot before it starts and records it once the
    /// whole operation is done. Recording here would both misread a node the
    /// caller already detached and pin the event order relative to a removal.
    fn place_node(&mut self, parent: NodeId, node: NodeId, before: Option<NodeId>) {
        let value_before = self.textarea_value_before_change(parent);
        self.unlink_from_current_parent(node);
        // Sibling references for the mutation record, read from the run the
        // node is about to join. Computed only while recording: no observer
        // exists on the parse path, so the lookups stay off it.
        let (previous, next) = if self.record_mutations && !self.recording_suppressed {
            let previous = match before {
                Some(before) => self.previous_sibling(before),
                None => self.last_child(parent),
            };
            (previous, before)
        } else {
            (None, None)
        };
        self.insert_linked(parent, node, before);
        self.record(Mutation::ChildList {
            target: parent,
            added: vec![node],
            removed: Vec::new(),
            previous,
            next,
        });
        self.reset_textarea_selection_if_changed(parent, value_before);
    }

    /// Links the unparented `node` under `parent` immediately before
    /// `before` (or as last child when `before` is `None`).
    ///
    /// `before`, when present, must already be a child of `parent`; `node`
    /// must carry no parent and no sibling links. This is the single place
    /// that writes the four link fields on insertion.
    fn insert_linked(&mut self, parent: NodeId, node: NodeId, before: Option<NodeId>) {
        let previous = match before {
            Some(before) => self.previous_sibling(before),
            None => self.last_child(parent),
        };
        {
            let attached = self.node_mut(node).expect("verified-live node has no slot");
            attached.parent = Some(parent);
            attached.previous_sibling = previous;
            attached.next_sibling = before;
        }
        match previous {
            Some(previous) => {
                self.node_mut(previous)
                    .expect("linked previous sibling has no slot")
                    .next_sibling = Some(node);
            }
            None => {
                self.node_mut(parent)
                    .expect("verified-live parent has no slot")
                    .first_child = Some(node);
            }
        }
        match before {
            Some(before) => {
                self.node_mut(before)
                    .expect("linked reference child has no slot")
                    .previous_sibling = Some(node);
            }
            None => {
                self.node_mut(parent)
                    .expect("verified-live parent has no slot")
                    .last_child = Some(node);
            }
        }
    }

    /// Insert a fragment by moving its children under `parent`, leaving the
    /// fragment empty and unparented
    /// (<https://dom.spec.whatwg.org/#concept-node-insert>).
    ///
    /// Structural only; the caller records the fragment subtree's lifecycle
    /// snapshot (see [`Dom::place_node`]).
    fn splice_fragment(&mut self, parent: NodeId, fragment: NodeId, before: Option<NodeId>) {
        let moved: Vec<NodeId> = self
            .children(fragment)
            .map(Iterator::collect)
            .unwrap_or_default();
        self.unlink_from_current_parent(fragment);
        if moved.is_empty() {
            return;
        }
        self.record(Mutation::ChildList {
            target: fragment,
            added: Vec::new(),
            removed: moved.clone(),
            previous: None,
            next: None,
        });
        // Sibling references for the mutation record, read before the run
        // leaves the fragment.
        let (previous, next) = if self.record_mutations && !self.recording_suppressed {
            let previous = match before {
                Some(before) => self.previous_sibling(before),
                None => self.last_child(parent),
            };
            (previous, before)
        } else {
            (None, None)
        };
        // Move the run one node at a time. Inserting each immediately before
        // the same reference preserves order, and O(1) steps keep the splice
        // O(k) with `insert_linked` as the only insert writer.
        for &id in &moved {
            self.unlink(id);
        }
        for &id in &moved {
            self.insert_linked(parent, id, before);
        }
        self.record(Mutation::ChildList {
            target: parent,
            added: moved,
            removed: Vec::new(),
            previous,
            next,
        });
    }

    fn ensure_alive(&self, a: NodeId, b: NodeId) -> Result<(), DomError> {
        self.require_live(a)?;
        self.require_live(b)
    }

    /// Single-handle variant of [`Dom::ensure_alive`].
    fn require_live(&self, a: NodeId) -> Result<(), DomError> {
        if self.contains(a) {
            Ok(())
        } else {
            Err(DomError::StaleNode)
        }
    }

    /// True iff placing `subtree` under `into` would nest it inside itself.
    fn would_cycle(&self, subtree: NodeId, into: NodeId) -> bool {
        // https://dom.spec.whatwg.org/#concept-tree-host-including-inclusive-ancestor
        let mut cursor = Some(into);
        while let Some(id) = cursor {
            if id == subtree {
                return true;
            }
            cursor = self.parent(id).or_else(|| {
                if self.is_fragment(id) {
                    self.shadow_hosts.get(&id).copied().or_else(|| {
                        self.template_contents
                            .iter()
                            .find_map(|(&host, &contents)| (contents == id).then_some(host))
                    })
                } else {
                    None
                }
            });
        }
        false
    }

    /// Queues the removal record for `id` from its current parent, without
    /// touching the tree.
    ///
    /// Shared by [`Dom::unlink_from_current_parent`] and the replace
    /// algorithm's adopt step, whose removal is observable even though the
    /// rest of the replacement suppresses observers
    /// (<https://dom.spec.whatwg.org/#concept-node-adopt>). The caller has
    /// verified `id` live; a missing parent is a silent no-op (the node is
    /// already detached).
    fn record_unlink(&mut self, id: NodeId) {
        if !self.record_mutations || self.recording_suppressed {
            return;
        }
        let Some(parent) = self.parent(id) else {
            return;
        };
        self.record(Mutation::ChildList {
            target: parent,
            added: Vec::new(),
            removed: vec![id],
            previous: self.previous_sibling(id),
            next: self.next_sibling(id),
        });
    }

    /// Removes `id` from whichever parent currently holds it.
    fn unlink_from_current_parent(&mut self, id: NodeId) {
        if self.parent(id).is_none() {
            return;
        }
        self.record_unlink(id);
        self.unlink(id);
    }

    /// Splices `id` out of its parent's child run and clears its parent and
    /// sibling links. A no-op when `id` is unparented.
    ///
    /// Defect policy, like every other structural site in this module: `id`
    /// was verified live, so a `Some` parent must name it and the link
    /// fields must agree. Parent-pointer/sibling-link divergence is arena
    /// corruption; panicking beats silently producing a node with two
    /// parents (or none), which later mutations would compound.
    fn unlink(&mut self, id: NodeId) {
        let Some((parent, previous, next)) = self.node_mut(id).and_then(|node| {
            node.parent
                .map(|parent| (parent, node.previous_sibling, node.next_sibling))
        }) else {
            return;
        };
        let value_before = self.textarea_value_before_change(parent);
        if let Some(previous) = previous {
            self.node_mut(previous)
                .expect("previous sibling has no slot")
                .next_sibling = next;
        } else {
            let first = self
                .node_mut(parent)
                .expect("live parent has no slot")
                .first_child;
            assert_eq!(first, Some(id), "parent's head is not the unlinked node");
            self.node_mut(parent)
                .expect("live parent has no slot")
                .first_child = next;
        }
        if let Some(next) = next {
            self.node_mut(next)
                .expect("next sibling has no slot")
                .previous_sibling = previous;
        } else {
            let last = self
                .node_mut(parent)
                .expect("live parent has no slot")
                .last_child;
            assert_eq!(last, Some(id), "parent's tail is not the unlinked node");
            self.node_mut(parent)
                .expect("live parent has no slot")
                .last_child = previous;
        }
        let detached = self.node_mut(id).expect("live node has no slot");
        detached.parent = None;
        detached.previous_sibling = None;
        detached.next_sibling = None;
        self.reset_textarea_selection_if_changed(parent, value_before);
    }

    fn set_data(
        &mut self,
        id: NodeId,
        extract: impl Fn(&mut NodeKind) -> Option<&mut String>,
        data: String,
    ) -> Result<(), DomError> {
        let parent = self.parent(id);
        let value_before = parent.and_then(|parent| self.textarea_value_before_change(parent));
        let node = self.node_mut(id).ok_or(DomError::StaleNode)?;
        match extract(&mut node.kind) {
            Some(field) => {
                let old_value = std::mem::replace(field, data);
                self.record(Mutation::CharacterData {
                    target: id,
                    old_value,
                });
                if let Some(parent) = parent {
                    self.reset_textarea_selection_if_changed(parent, value_before);
                }
                Ok(())
            }
            None => Err(DomError::WrongNodeType),
        }
    }

    /// Places a fresh node into a recycled or newly grown slot.
    ///
    /// # Panics
    ///
    /// Only when the arena would need more than `u32::MAX` slots: hundreds
    /// of GB of RAM, not a reachable runtime condition; the bound guards the
    /// handle width (`NodeId.slot` is a `u32`).
    fn alloc(&mut self, kind: NodeKind) -> NodeId {
        let node = Node {
            parent: None,
            first_child: None,
            last_child: None,
            previous_sibling: None,
            next_sibling: None,
            kind,
        };
        if let Some(slot) = self.free.pop() {
            // Lossless widening cast (u32 → usize); no From impl exists for it.
            let index = slot as usize;
            // The single generation tick per change of hands happens here.
            let generation = self.slots[index].generation.wrapping_add(1);
            self.slots[index] = Slot {
                generation,
                node: Some(node),
            };
            return NodeId::new(self.document.document, slot, generation);
        }
        // The bound is the `u32` handle width itself; exhausting it requires
        // >4 billion slots (hundreds of GB of arena), an impossible runtime
        // condition rather than an error to handle. Generations wrap after
        // 2^32 recycles of one slot; that residual ABA window is accepted by
        // design. Exploiting it needs billions of death/reuse cycles on a
        // single slot while some outside handle to that slot still exists.
        let slot = u32::try_from(self.slots.len())
            .expect("arena exhausted: >u32::MAX slots requires hundreds of GB of RAM");
        self.slots.push(Slot {
            generation: 0,
            node: Some(node),
        });
        NodeId::new(self.document.document, slot, 0)
    }
}

/// Keeps `value` when it satisfies `valid`, else replaces it with the empty
/// string, the default for every grammar-constrained input state except color.
fn sanitize_grammar(value: String, valid: fn(&str) -> bool) -> String {
    if valid(&value) { value } else { String::new() }
}

/// Strips leading and trailing ASCII whitespace and collapses internal runs to
/// one space
/// (<https://infra.spec.whatwg.org/#strip-and-collapse-ascii-whitespace>).
fn collapse_whitespace(text: &str) -> String {
    let mut result = String::new();
    let mut pending_space = false;
    for character in text.chars() {
        if character.is_ascii_whitespace() {
            pending_space = !result.is_empty();
        } else {
            if pending_space {
                result.push(' ');
                pending_space = false;
            }
            result.push(character);
        }
    }
    result
}

/// The `value` IDL attribute's mode for an input state
/// (<https://html.spec.whatwg.org/multipage/input.html#the-input-element:value-mode>).
#[derive(Clone, Copy, PartialEq, Eq)]
enum ValueMode {
    Value,
    Default,
    DefaultOn,
    Filename,
}

fn input_value_mode(typ: &str) -> ValueMode {
    match typ {
        "checkbox" | "radio" => ValueMode::DefaultOn,
        "file" => ValueMode::Filename,
        "hidden" | "submit" | "image" | "reset" | "button" => ValueMode::Default,
        _ => ValueMode::Value,
    }
}

/// Parses a finite floating-point number, or `None` when the text is not a
/// valid floating-point number.
fn parse_finite(text: &str) -> Option<f64> {
    if !is_valid_floating_point(text) {
        return None;
    }
    text.parse::<f64>().ok().filter(|number| number.is_finite())
}

/// The local date and time state's value sanitization: the separator becomes
/// `T` and the time is normalized.
fn sanitize_local_date_time_value(value: &str) -> String {
    if !is_valid_local_date_time(value) {
        return String::new();
    }
    let Some((date, time)) = value.split_once(['T', ' ']) else {
        return String::new();
    };
    format!("{date}T{}", normalize_time(time))
}

/// Drops a zero seconds field and a zero fraction from a valid time string.
fn normalize_time(time: &str) -> String {
    let mut result = time.to_owned();
    if let Some((base, fraction)) = result.split_once('.')
        && fraction.bytes().all(|byte| byte == b'0')
    {
        result.truncate(base.len());
    }
    let parts: Vec<&str> = result.split(':').collect();
    if parts.len() == 3 && parts[2] == "00" {
        result = format!("{}:{}", parts[0], parts[1]);
    }
    result
}

/// The range state's value sanitization: an invalid value becomes the default
/// (the midpoint of the range, or 50), then the value is clamped to the
/// min/max range
/// (<https://html.spec.whatwg.org/multipage/input.html#range-state-(type=range):value-sanitization-algorithm>).
fn sanitize_range_value(
    value: &str,
    min: Option<f64>,
    max: Option<f64>,
    step_attr: Option<&str>,
) -> String {
    // A reversed range collapses to its minimum.
    if let (Some(min), Some(max)) = (min, max)
        && max < min
    {
        return format!("{min}");
    }
    let mut number = parse_finite(value).unwrap_or(f64::NAN);
    if !number.is_finite() {
        number = match (min, max) {
            (Some(min), Some(max)) => min + (max - min) / 2.0,
            (Some(min), None) => min,
            (None, Some(max)) => max - max / 2.0,
            (None, None) => 50.0,
        };
    }
    let step_any = step_attr.is_some_and(|text| text.trim().eq_ignore_ascii_case("any"));
    if !step_any {
        let step = step_attr
            .and_then(parse_finite)
            .filter(|step| *step > 0.0)
            .unwrap_or(1.0);
        let base = min.unwrap_or(0.0);
        number = base + ((number - base) / step).round() * step;
    }
    if let Some(min) = min
        && number < min
    {
        number = min;
    }
    if let Some(max) = max
        && number > max
    {
        number = max;
    }
    format!("{number}")
}

/// Replaces CRLF and lone CR with LF
/// (<https://infra.spec.whatwg.org/#normalize-newlines>).
fn normalize_newlines(text: &str) -> String {
    let mut normalized = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            normalized.push('\n');
        } else {
            normalized.push(character);
        }
    }
    normalized
}

/// The [length](https://infra.spec.whatwg.org/#string-length) of `text` in
/// UTF-16 code units, the unit the selection APIs use.
#[allow(
    clippy::cast_possible_truncation,
    reason = "a 64-bit string length beyond u32 cannot be produced by this engine"
)]
fn utf16_length(text: &str) -> u32 {
    text.encode_utf16().count() as u32
}

/// Adds each attribute whose qualified name is not already present; the
/// first occurrence wins, matching the DOM's name-keyed attribute list
/// (<https://dom.spec.whatwg.org/#concept-attribute>).
fn merge_attrs(attributes: &mut Vec<Attribute>, attrs: impl IntoIterator<Item = Attribute>) {
    for attr in attrs {
        if !attributes.iter().any(|existing| existing.name == attr.name) {
            attributes.push(attr);
        }
    }
}
