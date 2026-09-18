//! Stylo's view of our DOM.
//!
//! Servo's style engine is written against its own `TElement`/`TNode`
//! traits, so this module wraps our immutable [`dom::Dom`] in per-pass node
//! records and hands out borrowed handles. It mirrors `dom/src/select.rs`
//! (same tree, same state policy) but against Stylo's `SelectorImpl` and
//! node traits.
//!
//! Two constraints shape the design:
//!
//! - [`TElement`] requires `Copy`, and Stylo's style-sharing cache asserts
//!   that the handle is pointer-sized (its thread-local cache reinterprets
//!   handles as `usize`). So the handle is `&StyloNode<'_>`, one pointer;
//!   the records live in one `Vec` per styling pass.
//! - Tree navigation must return handles for neighbors, so each record
//!   carries `Cell`s holding its parent/child/sibling handles, filled after
//!   all records exist.
//!
//! Element data lives in a pre-populated side table rather than on the
//! arena: one styling pass fills it, then the borrow ends.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use dom::{Dom, NodeId};
use selectors::attr::{AttrSelectorOperation, NamespaceConstraint};
use selectors::matching::ElementSelectorFlags;
use style::data::{ElementData, ElementDataMut, ElementDataRef, ElementDataWrapper};
use style::dom::{LayoutIterator, NodeInfo, OpaqueNode, TDocument, TElement, TNode, TShadowRoot};
use style::properties::PropertyDeclarationBlock;
use style::selector_parser::SelectorImpl;
use style::servo_arc::{Arc, ArcBorrow};
use style::shared_lock::{Locked, SharedRwLock};
use style::values::computed::Display as ComputedDisplay;

/// The borrowed handle Stylo names as `E`.
pub(crate) type StyloElement<'a> = &'a StyloNode<'a>;

/// Per-element Stylo state for one styling pass.
///
/// Everything hangs off shared references because the traversal is
/// single-threaded: Stylo's `unsafe` data hooks are sound here for the same
/// reason Blitz's rayon traversal is sound there — exclusive logical access —
/// except ours is simpler, since no second thread exists at all.
pub(crate) struct StyloTables {
    /// One data wrapper per element, defaulted before styling.
    pub(crate) data: HashMap<NodeId, ElementDataWrapper>,
    /// Parsed `style` attributes, keyed by element.
    pub(crate) style_attrs: HashMap<NodeId, Arc<Locked<PropertyDeclarationBlock>>>,
    /// Interned `id` attributes, keyed by element, backing [`TElement::id`].
    pub(crate) ids: HashMap<NodeId, style::Atom>,
    /// Elements with unstyled descendants, for the dirty-descendants bit.
    pub(crate) dirty: RefCell<HashSet<NodeId>>,
    /// Exact numeric identity per node, since the arena's slot index stays
    /// crate-private.
    pub(crate) indices: HashMap<NodeId, usize>,
    /// Lock owning every parsed sheet and style attribute.
    pub(crate) lock: SharedRwLock,
}

impl std::fmt::Debug for StyloTables {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StyloTables")
            .field("elements", &self.data.len())
            .field("style_attrs", &self.style_attrs.len())
            .finish_non_exhaustive()
    }
}

impl StyloTables {
    /// Empty tables with a fresh lock.
    pub(crate) fn new() -> Self {
        Self {
            data: HashMap::new(),
            style_attrs: HashMap::new(),
            ids: HashMap::new(),
            dirty: RefCell::new(HashSet::new()),
            indices: HashMap::new(),
            lock: SharedRwLock::new(),
        }
    }

    /// Registers `id` and returns its index.
    pub(crate) fn register(&mut self, id: NodeId) -> usize {
        let next = self.indices.len();
        *self.indices.entry(id).or_insert(next)
    }

    /// The index of a registered node.
    pub(crate) fn index(&self, id: NodeId) -> usize {
        self.indices.get(&id).copied().unwrap_or(usize::MAX)
    }
}

/// One node as Stylo sees it: a borrowed tree, a handle, and neighbor
/// handles behind cells (filled after every record exists).
pub(crate) struct StyloNode<'a> {
    /// The tree.
    pub(crate) dom: &'a Dom,
    /// This node.
    pub(crate) id: NodeId,
    /// The pass's side tables.
    pub(crate) tables: &'a StyloTables,
    /// Parent node.
    parent: Cell<Option<&'a StyloNode<'a>>>,
    /// First child.
    first_child: Cell<Option<&'a StyloNode<'a>>>,
    /// Last child.
    last_child: Cell<Option<&'a StyloNode<'a>>>,
    /// Previous sibling.
    prev_sibling: Cell<Option<&'a StyloNode<'a>>>,
    /// Next sibling.
    next_sibling: Cell<Option<&'a StyloNode<'a>>>,
    /// The document at the root of this node's tree.
    owner_doc: Cell<Option<&'a StyloNode<'a>>>,
}

impl std::fmt::Debug for StyloNode<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StyloNode")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl PartialEq for StyloNode<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for StyloNode<'_> {}

impl Hash for StyloNode<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl<'a> StyloNode<'a> {
    /// Builds every node record for `dom` in registration order, with empty
    /// neighbor cells.
    ///
    /// The caller owns the returned `Vec` for the whole pass, borrows it,
    /// and calls [`StyloNode::fill_neighbors`] with that borrow so the cells
    /// can point at other records.
    pub(crate) fn build_all(dom: &'a Dom, tables: &'a StyloTables) -> Vec<StyloNode<'a>> {
        let document = dom.document();
        std::iter::once(document)
            .chain(dom.descendants(document))
            .map(|id| StyloNode {
                dom,
                id,
                tables,
                parent: Cell::new(None),
                first_child: Cell::new(None),
                last_child: Cell::new(None),
                prev_sibling: Cell::new(None),
                next_sibling: Cell::new(None),
                owner_doc: Cell::new(None),
            })
            .collect()
    }

    /// Fills this record's neighbor cells from the tree.
    ///
    /// Every record already exists and the slice is borrowed for the rest of
    /// the pass, so the references handed out here stay valid.
    pub(crate) fn fill_neighbors<'b>(&'b self, nodes: &'b [StyloNode<'a>])
    where
        'b: 'a,
    {
        self.parent
            .set(node_at(nodes, self.tables, self.dom.parent(self.id)));
        if let Some(mut kids) = self.dom.children(self.id) {
            let first = kids.next().copied();
            let last = kids.next_back().copied();
            self.first_child.set(node_at(nodes, self.tables, first));
            self.last_child.set(node_at(nodes, self.tables, last));
        }
        self.prev_sibling
            .set(node_at(nodes, self.tables, self.dom.sibling(self.id, false)));
        self.next_sibling
            .set(node_at(nodes, self.tables, self.dom.sibling(self.id, true)));
        let mut root = self.id;
        while let Some(parent) = self.dom.parent(root) {
            root = parent;
        }
        self.owner_doc.set(node_at(nodes, self.tables, Some(root)));
    }

    /// Whether this node is an element.
    pub(crate) fn is_element(&self) -> bool {
        matches!(self.dom.kind(self.id), Some(dom::NodeKind::Element { .. }))
    }

    /// Whether this node is character data.
    pub(crate) fn is_text_node(&self) -> bool {
        matches!(
            self.dom.kind(self.id),
            Some(dom::NodeKind::Text { .. } | dom::NodeKind::CDataSection { .. })
        )
    }

    /// The element's name, if this node is an element.
    fn name(&self) -> Option<&dom::QualName> {
        match self.dom.kind(self.id) {
            Some(dom::NodeKind::Element { name, .. }) => Some(name),
            _ => None,
        }
    }
}

/// The record for `id`, looked up through the registration index.
pub(crate) fn node_at<'a, 'b>(
    nodes: &'b [StyloNode<'a>],
    tables: &StyloTables,
    id: Option<NodeId>,
) -> Option<&'b StyloNode<'a>> {
    let id = id?;
    let index = tables.index(id);
    nodes.get(index)
}

/// Children of one node, in tree order, for Stylo's traversal.
pub(crate) struct Traverser<'a> {
    /// The next child to hand out.
    next: Option<&'a StyloNode<'a>>,
}

impl<'a> Iterator for Traverser<'a> {
    type Item = StyloElement<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let node = self.next?;
        self.next = node.next_sibling.get();
        Some(node)
    }
}

impl<'a> TNode for &'a StyloNode<'a> {
    type ConcreteElement = Self;
    type ConcreteDocument = Self;
    type ConcreteShadowRoot = Self;

    fn parent_node(&self) -> Option<Self> {
        self.parent.get()
    }

    fn traversal_parent(&self) -> Option<Self::ConcreteElement> {
        self.parent.get().filter(NodeInfo::is_element)
    }

    fn first_child(&self) -> Option<Self> {
        self.first_child.get()
    }

    fn last_child(&self) -> Option<Self> {
        self.last_child.get()
    }

    fn prev_sibling(&self) -> Option<Self> {
        self.prev_sibling.get()
    }

    fn next_sibling(&self) -> Option<Self> {
        self.next_sibling.get()
    }

    fn owner_doc(&self) -> Self::ConcreteDocument {
        self.owner_doc.get().expect("every record knows its document")
    }

    fn is_in_document(&self) -> bool {
        self.dom.is_connected(self.id)
    }

    fn opaque(&self) -> OpaqueNode {
        OpaqueNode(self.tables.index(self.id))
    }

    fn debug_id(self) -> usize {
        self.tables.index(self.id)
    }

    fn as_element(&self) -> Option<Self::ConcreteElement> {
        self.is_element().then_some(*self)
    }

    fn as_document(&self) -> Option<Self::ConcreteDocument> {
        matches!(self.dom.kind(self.id), Some(dom::NodeKind::Document)).then_some(*self)
    }

    fn as_shadow_root(&self) -> Option<Self::ConcreteShadowRoot> {
        // No shadow trees exist here.
        None
    }
}

impl NodeInfo for &StyloNode<'_> {
    fn is_element(&self) -> bool {
        StyloNode::is_element(self)
    }

    fn is_text_node(&self) -> bool {
        StyloNode::is_text_node(self)
    }
}

impl<'a> TDocument for &'a StyloNode<'a> {
    type ConcreteNode = Self;

    fn as_node(&self) -> Self::ConcreteNode {
        *self
    }

    fn is_html_document(&self) -> bool {
        true
    }

    fn quirks_mode(&self) -> selectors::matching::QuirksMode {
        selectors::matching::QuirksMode::NoQuirks
    }

    fn shared_lock(&self) -> &SharedRwLock {
        &self.tables.lock
    }
}

impl<'a> TShadowRoot for &'a StyloNode<'a> {
    type ConcreteNode = Self;

    fn as_node(&self) -> Self::ConcreteNode {
        *self
    }

    fn host(&self) -> <Self::ConcreteNode as TNode>::ConcreteElement {
        // Unreachable: `as_shadow_root` always returns `None`, so no shadow
        // root handle ever exists to call this on.
        unreachable!("no shadow trees exist in this DOM")
    }

    fn style_data<'b>(&self) -> Option<&'b style::stylist::CascadeData>
    where
        Self: 'b,
    {
        None
    }
}

impl<'a> selectors::Element for &'a StyloNode<'a> {
    type Impl = SelectorImpl;

    fn opaque(&self) -> selectors::OpaqueElement {
        // Stylo's element and selectors' opaque views share the registration
        // index; nothing dereferences these, they only compare and hash.
        let index = self.tables.index(self.id).wrapping_add(1);
        selectors::OpaqueElement::from_non_null_ptr(
            std::ptr::NonNull::new(index as *mut ()).expect("node index is never zero"),
        )
    }

    fn parent_element(&self) -> Option<Self> {
        self.parent.get().filter(NodeInfo::is_element)
    }

    fn parent_node_is_shadow_root(&self) -> bool {
        false
    }

    fn containing_shadow_host(&self) -> Option<Self> {
        None
    }

    fn is_pseudo_element(&self) -> bool {
        false
    }

    fn prev_sibling_element(&self) -> Option<Self> {
        let mut node = self.prev_sibling.get();
        while let Some(candidate) = node {
            if candidate.is_element() {
                return Some(candidate);
            }
            node = candidate.prev_sibling.get();
        }
        None
    }

    fn next_sibling_element(&self) -> Option<Self> {
        let mut node = self.next_sibling.get();
        while let Some(candidate) = node {
            if candidate.is_element() {
                return Some(candidate);
            }
            node = candidate.next_sibling.get();
        }
        None
    }

    fn first_element_child(&self) -> Option<Self> {
        let mut node = self.first_child.get();
        while let Some(candidate) = node {
            if candidate.is_element() {
                return Some(candidate);
            }
            node = candidate.next_sibling.get();
        }
        None
    }

    fn is_html_element_in_html_document(&self) -> bool {
        self.is_element() && dom::is_html(self.dom, self.id)
    }

    fn has_local_name(&self, local_name: &web_atoms::LocalName) -> bool {
        dom::local_is(self.dom, self.id, &[local_name])
    }

    fn has_namespace(&self, ns: &web_atoms::Namespace) -> bool {
        self.name().is_some_and(|name| name.ns == *ns)
    }

    fn is_same_type(&self, other: &Self) -> bool {
        match (self.name(), other.name()) {
            (Some(a), Some(b)) => {
                let (a_name, b_name): (&str, &str) = (&a.local, &b.local);
                a.ns == b.ns
                    && if dom::is_html(self.dom, self.id) {
                        a_name.eq_ignore_ascii_case(b_name)
                    } else {
                        a_name == b_name
                    }
            }
            _ => false,
        }
    }

    fn attr_matches(
        &self,
        ns: &NamespaceConstraint<&style::Namespace>,
        local_name: &style::LocalName,
        operation: &AttrSelectorOperation<&style::values::AtomString>,
    ) -> bool {
        let Some(attributes) = self.dom.attributes(self.id) else {
            return false;
        };
        attributes.iter().any(|attribute| {
            let namespace_ok = match ns {
                NamespaceConstraint::Any => true,
                NamespaceConstraint::Specific(url) => attribute.name.ns == url.0,
            };
            if !namespace_ok {
                return false;
            }
            // Name casing was already chosen by the engine (see
            // `has_local_name`); value case handling rides inside `operation`.
            let stored = attribute.name.local.as_ref();
            let wanted: &str = &local_name.0;
            let named = if dom::is_html(self.dom, self.id) {
                stored.eq_ignore_ascii_case(wanted)
            } else {
                stored == wanted
            };
            named && operation.eval_str(attribute.value.as_str())
        })
    }

    fn match_non_ts_pseudo_class(
        &self,
        pc: &style::selector_parser::NonTSPseudoClass,
        _context: &mut selectors::context::MatchingContext<SelectorImpl>,
    ) -> bool {
        use style::selector_parser::NonTSPseudoClass as Pc;
        let (dom, id) = (self.dom, self.id);
        match pc {
            Pc::AnyLink | Pc::Link => dom::is_hyperlink(dom, id),
            Pc::Enabled => dom::is_enabled(dom, id),
            Pc::Disabled => dom::is_disabled(dom, id),
            Pc::Checked => dom::is_checked(dom, id),
            Pc::Required => dom::is_required(dom, id),
            Pc::Optional => dom::is_optional(dom, id),
            Pc::ReadOnly => dom::is_read_only(dom, id),
            Pc::ReadWrite => dom::is_read_write(dom, id),
            Pc::PlaceholderShown => dom::is_placeholder_shown(dom, id),
            Pc::Defined => dom::is_defined(dom, id),
            Pc::Indeterminate => dom::is_indeterminate(dom, id),
            Pc::Default => dom::is_default(dom, id),
            Pc::Lang(lang) => dom::lang_matches(dom, id, std::slice::from_ref(lang)),
            // Interaction, media, and dialog states have no model in a
            // headless one-shot tree; they miss vacuously, like `:hover`
            // does in our own matcher.
            Pc::Active
            | Pc::Autofill
            | Pc::CustomState(_)
            | Pc::Focus
            | Pc::FocusWithin
            | Pc::FocusVisible
            | Pc::Fullscreen
            | Pc::Hover
            | Pc::InRange
            | Pc::Invalid
            | Pc::Modal
            | Pc::MozMeterOptimum
            | Pc::MozMeterSubOptimum
            | Pc::MozMeterSubSubOptimum
            | Pc::Open
            | Pc::OutOfRange
            | Pc::PopoverOpen
            | Pc::ServoNonZeroBorder
            | Pc::Target
            | Pc::UserInvalid
            | Pc::UserValid
            | Pc::Valid
            | Pc::Visited => false,
        }
    }

    fn match_pseudo_element(
        &self,
        _pe: &style::selector_parser::PseudoElement,
        _context: &mut selectors::context::MatchingContext<SelectorImpl>,
    ) -> bool {
        // Known pseudo-elements parse like browsers' and never match: no
        // layout, no boxes, no generated content exists in this tree.
        false
    }

    fn apply_selector_flags(&self, _flags: ElementSelectorFlags) {
        // No invalidation machinery exists here: styling is one-shot.
    }

    fn is_link(&self) -> bool {
        dom::is_hyperlink(self.dom, self.id)
    }

    fn is_html_slot_element(&self) -> bool {
        false
    }

    fn has_id(
        &self,
        id: &style::values::AtomIdent,
        case_sensitivity: selectors::attr::CaseSensitivity,
    ) -> bool {
        dom::attr_value(self.dom, self.id, "id")
            .is_some_and(|value| case_sensitivity.eq(value.as_bytes(), id.0.as_bytes()))
    }

    fn has_class(
        &self,
        name: &style::values::AtomIdent,
        case_sensitivity: selectors::attr::CaseSensitivity,
    ) -> bool {
        dom::attr_value(self.dom, self.id, "class").is_some_and(|value| {
            value
                .split_ascii_whitespace()
                .any(|token| case_sensitivity.eq(token.as_bytes(), name.0.as_bytes()))
        })
    }

    fn has_custom_state(&self, _name: &style::values::AtomIdent) -> bool {
        false
    }

    fn imported_part(&self, _name: &style::values::AtomIdent) -> Option<style::values::AtomIdent> {
        None
    }

    fn is_part(&self, _name: &style::values::AtomIdent) -> bool {
        false
    }

    fn is_empty(&self) -> bool {
        let Some(mut kids) = self.dom.children(self.id) else {
            return false;
        };
        kids.all(|&kid| match self.dom.kind(kid) {
            Some(dom::NodeKind::Text { data }) => data.is_empty(),
            Some(dom::NodeKind::Element { .. }) => false,
            _ => true,
        })
    }

    fn is_root(&self) -> bool {
        matches!(
            self.dom.parent(self.id),
            Some(parent) if matches!(self.dom.kind(parent), Some(dom::NodeKind::Document))
        )
    }

    fn add_element_unique_hashes(&self, _filter: &mut selectors::bloom::BloomFilter) -> bool {
        false
    }
}

#[expect(
    unsafe_code,
    reason = "upstream Stylo declares five data hooks as unsafe fn; the bodies perform no unsafe operations and the pass is single-threaded"
)]
impl<'a> TElement for &'a StyloNode<'a> {
    type ConcreteNode = Self;
    type TraversalChildrenIterator = Traverser<'a>;

    fn as_node(&self) -> Self::ConcreteNode {
        *self
    }

    fn traversal_children(&self) -> LayoutIterator<Self::TraversalChildrenIterator> {
        LayoutIterator(Traverser {
            next: self.first_child.get(),
        })
    }

    fn is_html_element(&self) -> bool {
        selectors::Element::is_html_element_in_html_document(self)
    }

    fn is_mathml_element(&self) -> bool {
        false
    }

    fn is_svg_element(&self) -> bool {
        self.name()
            .is_some_and(|name| name.ns == dom::svg_namespace())
    }

    fn style_attribute(&self) -> Option<ArcBorrow<'_, Locked<PropertyDeclarationBlock>>> {
        self.tables
            .style_attrs
            .get(&self.id)
            .map(|block| block.borrow_arc())
    }

    fn state(&self) -> stylo_dom::ElementState {
        // Mirrors `dom/src/select.rs`: only link-ness feeds the
        // non-structural matching here; every interactive state misses
        // vacuously.
        let mut state = stylo_dom::ElementState::empty();
        if dom::is_hyperlink(self.dom, self.id) {
            state.insert(stylo_dom::ElementState::VISITED_OR_UNVISITED);
        }
        state
    }

    fn has_part_attr(&self) -> bool {
        false
    }

    fn exports_any_part(&self) -> bool {
        false
    }

    fn id(&self) -> Option<&style::Atom> {
        self.tables.ids.get(&self.id)
    }

    fn each_class<F>(&self, mut callback: F)
    where
        F: FnMut(&style::values::AtomIdent),
    {
        if let Some(class) = dom::attr_value(self.dom, self.id, "class") {
            for token in class.split_ascii_whitespace() {
                let atom = style::Atom::from(token);
                callback(style::values::AtomIdent::cast(&atom));
            }
        }
    }

    fn each_attr_name<F>(&self, mut callback: F)
    where
        F: FnMut(&style::LocalName),
    {
        if let Some(attributes) = self.dom.attributes(self.id) {
            // Own the interned names for the callback's duration.
            let names: Vec<style::LocalName> = attributes
                .iter()
                .map(|attribute| style::LocalName::new(attribute.name.local.clone()))
                .collect();
            for name in &names {
                callback(name);
            }
        }
    }

    fn each_custom_state<F>(&self, _callback: F)
    where
        F: FnMut(&style::values::AtomIdent),
    {
    }

    fn has_dirty_descendants(&self) -> bool {
        self.tables.dirty.borrow().contains(&self.id)
    }

    fn has_snapshot(&self) -> bool {
        false
    }

    fn handled_snapshot(&self) -> bool {
        true
    }

    unsafe fn set_handled_snapshot(&self) {}

    unsafe fn set_dirty_descendants(&self) {
        self.tables.dirty.borrow_mut().insert(self.id);
    }

    unsafe fn unset_dirty_descendants(&self) {
        self.tables.dirty.borrow_mut().remove(&self.id);
    }

    fn store_children_to_process(&self, _n: isize) {
        // Never called: postorder traversal is disabled below.
    }

    fn did_process_child(&self) -> isize {
        // Never called: postorder traversal is disabled below.
        0
    }

    unsafe fn ensure_data(&self) -> ElementDataMut<'_> {
        // Single-threaded traversal with pre-populated tables; no other
        // borrow of this element's wrapper is live while styling runs.
        self.tables
            .data
            .get(&self.id)
            .expect("every styled element is registered before the traversal")
            .borrow_mut()
    }

    unsafe fn clear_data(&self) {
        // Same exclusive logical access as `ensure_data`.
        if let Some(wrapper) = self.tables.data.get(&self.id) {
            *wrapper.borrow_mut() = ElementData::default();
        }
    }

    fn has_data(&self) -> bool {
        self.tables.data.contains_key(&self.id)
    }

    fn borrow_data(&self) -> Option<ElementDataRef<'_>> {
        self.tables.data.get(&self.id).map(ElementDataWrapper::borrow)
    }

    fn mutate_data(&self) -> Option<ElementDataMut<'_>> {
        self.tables
            .data
            .get(&self.id)
            .map(ElementDataWrapper::borrow_mut)
    }

    fn skip_item_display_fixup(&self) -> bool {
        false
    }

    fn may_have_animations(&self) -> bool {
        false
    }

    fn has_animations(&self, _context: &style::context::SharedStyleContext) -> bool {
        false
    }

    fn has_css_animations(
        &self,
        _context: &style::context::SharedStyleContext,
        _pseudo_element: Option<style::selector_parser::PseudoElement>,
    ) -> bool {
        false
    }

    fn has_css_transitions(
        &self,
        _context: &style::context::SharedStyleContext,
        _pseudo_element: Option<style::selector_parser::PseudoElement>,
    ) -> bool {
        false
    }

    fn animation_rule(
        &self,
        _context: &style::context::SharedStyleContext,
    ) -> Option<Arc<Locked<PropertyDeclarationBlock>>> {
        None
    }

    fn transition_rule(
        &self,
        _context: &style::context::SharedStyleContext,
    ) -> Option<Arc<Locked<PropertyDeclarationBlock>>> {
        None
    }

    fn shadow_root(&self) -> Option<<Self::ConcreteNode as TNode>::ConcreteShadowRoot> {
        None
    }

    fn containing_shadow(&self) -> Option<<Self::ConcreteNode as TNode>::ConcreteShadowRoot> {
        None
    }

    fn lang_attr(&self) -> Option<style::selector_parser::AttrValue> {
        None
    }

    fn match_element_lang(
        &self,
        override_lang: Option<Option<style::selector_parser::AttrValue>>,
        value: &style::selector_parser::Lang,
    ) -> bool {
        match override_lang {
            // Snapshot overrides never occur one-shot (`has_snapshot` is
            // false); match the single tag by the standard prefix rule.
            Some(Some(tag)) => {
                let tag: &str = &tag.0;
                tag.eq_ignore_ascii_case(value)
                    || tag.len() > value.len()
                        && tag.as_bytes()[value.len()] == b'-'
                        && tag[..value.len()].eq_ignore_ascii_case(value)
            }
            Some(None) => false,
            None => dom::lang_matches(self.dom, self.id, std::slice::from_ref(value)),
        }
    }

    fn is_html_document_body_element(&self) -> bool {
        if !dom::local_is(self.dom, self.id, &["body"]) || !dom::is_html(self.dom, self.id) {
            return false;
        }
        let Some(parent) = self.dom.parent(self.id) else {
            return false;
        };
        dom::local_is(self.dom, parent, &["html"])
    }

    fn synthesize_presentational_hints_for_legacy_attributes<V>(
        &self,
        _visited_handling: selectors::matching::VisitedHandlingMode,
        _hints: &mut V,
    ) where
        V: selectors::sink::Push<style::applicable_declarations::ApplicableDeclarationBlock>,
    {
        // No presentational attributes are mapped: the old engine did not map
        // them either, and each mapping would be web-compat surface to verify.
    }

    fn local_name(&self) -> &web_atoms::LocalName {
        &self
            .name()
            .expect("Stylo only queries names on elements")
            .local
    }

    fn namespace(&self) -> &web_atoms::Namespace {
        &self
            .name()
            .expect("Stylo only queries namespaces on elements")
            .ns
    }

    fn query_container_size(
        &self,
        _display: &ComputedDisplay,
    ) -> euclid::default::Size2D<Option<app_units::Au>> {
        // Container queries are disabled by returning no size, like Blitz.
        euclid::default::Size2D::default()
    }

    fn has_selector_flags(&self, _flags: ElementSelectorFlags) -> bool {
        // No selector flags are ever set: styling is one-shot.
        false
    }

    fn relative_selector_search_direction(&self) -> ElementSelectorFlags {
        ElementSelectorFlags::empty()
    }

    fn get_attr(&self, attr: &style::LocalName, namespace: &style::Namespace) -> Option<String> {
        let wanted_ns: &str = &namespace.0;
        let wanted: &str = &attr.0;
        self.dom
            .attributes(self.id)?
            .iter()
            .find_map(|attribute| {
                (attribute.name.ns.as_ref() == wanted_ns && attribute.name.local.as_ref() == wanted)
                    .then(|| attribute.value.clone())
            })
    }
}
