//! Live ranges: boundary points, comparison, and mutation adjustment
//! (<https://dom.spec.whatwg.org/#ranges>).
//!
//! Realm-free algorithms over one document's tree: node ids and offsets only.
//! Callers split `&base` from `&mut ranges` so adjustment borrows stay
//! disjoint. The binding layer owns wrappers and converts values.

use std::cmp::Ordering;

use blitz_dom::NodeData;

use crate::dom::tree::{NodeId, child_ids};

/// One boundary point: a node and an offset into it
/// (<https://dom.spec.whatwg.org/#concept-range-bp>).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BoundaryPoint {
    pub(crate) node: NodeId,
    pub(crate) offset: u32,
}

/// A boundary point inside an attribute.
///
/// Attributes are not tree nodes, so the point's root is the attribute
/// itself, no tree mutation can move it, and a range in one is always
/// collapsed (<https://dom.spec.whatwg.org/#concept-range-bp>).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AttrPoint {
    /// The element the attribute belongs to, which supplies the document.
    pub(crate) scope: NodeId,
    /// The attribute registry id the wrapper reports.
    pub(crate) id: u64,
}

/// A live range's start and end.
///
/// `attr` is set when both points live in one attribute; `start` and `end`
/// then hold a collapsed placeholder at the attribute's scope element so the
/// tree-point algorithms stay valid. Slots live in the document that owns the
/// boundary points and free with it
/// (<https://dom.spec.whatwg.org/#concept-range>).
#[derive(Clone, Copy, Debug)]
pub(crate) struct RangeState {
    pub(crate) start: BoundaryPoint,
    pub(crate) end: BoundaryPoint,
    pub(crate) attr: Option<AttrPoint>,
}

impl RangeState {
    /// A collapsed range at a tree point.
    pub(crate) fn collapsed(point: BoundaryPoint) -> Self {
        Self {
            start: point,
            end: point,
            attr: None,
        }
    }

    /// A collapsed range in an attribute.
    pub(crate) fn attribute(attr: AttrPoint) -> Self {
        let placeholder = BoundaryPoint {
            node: attr.scope,
            offset: 0,
        };
        Self {
            start: placeholder,
            end: placeholder,
            attr: Some(attr),
        }
    }

    /// The document that owns the boundary points, and so the slot.
    pub(crate) fn document(&self) -> u32 {
        match self.attr {
            Some(attr) => attr.scope.document,
            None => self.start.node.document,
        }
    }
}

/// One document's live-range slots.
///
/// A slot is `None` once freed: the wrapper was collected, or the range
/// re-registered in another document because `setStart`/`setEnd` moved its
/// boundary points there. Freed slots are reused, so a page that creates
/// ranges in a loop does not grow the table, and a document with no live
/// ranges scans nothing on mutation
/// (<https://dom.spec.whatwg.org/#concept-range>).
#[derive(Default)]
pub(crate) struct RangeTable {
    slots: Vec<Option<RangeState>>,
    free: Vec<usize>,
    live: usize,
}

impl RangeTable {
    /// Registers a live range, returning its slot index.
    pub(crate) fn alloc(&mut self, state: RangeState) -> usize {
        self.live += 1;
        if let Some(slot) = self.free.pop() {
            self.slots[slot] = Some(state);
            slot
        } else {
            self.slots.push(Some(state));
            self.slots.len() - 1
        }
    }

    /// The state at `slot`, or `None` for a freed slot.
    pub(crate) fn get(&self, slot: usize) -> Option<RangeState> {
        self.slots.get(slot).copied().flatten()
    }

    /// Overwrites a live slot. Returns false when the slot is freed.
    pub(crate) fn set(&mut self, slot: usize, state: RangeState) -> bool {
        match self.slots.get_mut(slot) {
            Some(entry) if entry.is_some() => {
                *entry = Some(state);
                true
            }
            _ => false,
        }
    }

    /// Frees `slot` for reuse. Freeing a freed slot is a no-op.
    pub(crate) fn free(&mut self, slot: usize) {
        if let Some(entry) = self.slots.get_mut(slot)
            && entry.take().is_some()
        {
            self.free.push(slot);
            self.live -= 1;
        }
    }

    /// Whether the document has no live ranges.
    pub(crate) fn is_empty(&self) -> bool {
        self.live == 0
    }

    /// Every live range, in slot order.
    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = &mut RangeState> {
        self.slots.iter_mut().flatten()
    }
}

fn utf16_length(text: &str) -> u32 {
    u32::try_from(text.encode_utf16().count()).unwrap_or(u32::MAX)
}

/// The length of a node: child count, character-data length in UTF-16 code
/// units, or zero (<https://dom.spec.whatwg.org/#concept-node-length>).
pub(crate) fn node_length(base: &blitz_dom::BaseDocument, id: NodeId) -> u32 {
    let Some(node) = base.get_node(id.node) else {
        return 0;
    };
    match &node.data {
        NodeData::Document(_)
        | NodeData::Element(_)
        | NodeData::AnonymousBlock(_)
        | NodeData::Fragment { .. }
        | NodeData::ShadowRoot { .. } => u32::try_from(node.children.len()).unwrap_or(u32::MAX),
        NodeData::Text(text) => utf16_length(&text.content),
        NodeData::Comment { contents }
        | NodeData::ProcessingInstruction { contents, .. }
        | NodeData::CDataSection { contents } => utf16_length(contents),
        NodeData::Doctype { .. } => 0,
    }
}

/// Whether `id` is a `Text` node for range text: `Text` or `CDATASection`.
pub(crate) fn is_text(base: &blitz_dom::BaseDocument, id: NodeId) -> bool {
    matches!(
        base.get_node(id.node).map(|node| &node.data),
        Some(NodeData::Text(_) | NodeData::CDataSection { .. })
    )
}

/// Whether `id` is character data: `Text`, `CDATASection`, `Comment`, or
/// `ProcessingInstruction`. The mutation algorithms address all four;
/// stringification only counts `Text`
/// (<https://dom.spec.whatwg.org/#interface-characterdata>).
pub(crate) fn is_character_data(base: &blitz_dom::BaseDocument, id: NodeId) -> bool {
    matches!(
        base.get_node(id.node).map(|node| &node.data),
        Some(
            NodeData::Text(_)
                | NodeData::CDataSection { .. }
                | NodeData::Comment { .. }
                | NodeData::ProcessingInstruction { .. }
        )
    )
}

/// The range's text: a same-node substring, or the partial start, every
/// fully contained `Text` node in tree order, and the partial end
/// (<https://dom.spec.whatwg.org/#dom-range-stringifier>).
pub(crate) fn range_text(
    document: &crate::documents::BlitzDocument,
    _document_id: u32,
    state: &RangeState,
) -> Vec<u16> {
    let base = &document.base;
    let mut units = Vec::new();
    let data_of = |node: NodeId| document.character_data(node.node).units().into_owned();
    if state.start.node == state.end.node && is_text(base, state.start.node) {
        let data = data_of(state.start.node);
        let start = (state.start.offset as usize).min(data.len());
        let end = (state.end.offset as usize).min(data.len()).max(start);
        units.extend_from_slice(&data[start..end]);
        return units;
    }
    if is_text(base, state.start.node) {
        let data = data_of(state.start.node);
        units.extend_from_slice(&data[(state.start.offset as usize).min(data.len())..]);
    }
    let ancestor = common_ancestor(base, state.start.node, state.end.node);
    append_contained_text(document, state, ancestor, &mut units);
    if is_text(base, state.end.node) {
        let data = data_of(state.end.node);
        units.extend_from_slice(&data[..(state.end.offset as usize).min(data.len())]);
    }
    units
}

/// Append every `Text` node contained in the range under `ancestor`, in tree
/// order. Boundary nodes never satisfy containment, so they cannot recur.
fn append_contained_text(
    document: &crate::documents::BlitzDocument,
    state: &RangeState,
    ancestor: NodeId,
    units: &mut Vec<u16>,
) {
    let base = &document.base;
    for descendant in child_ids(base, ancestor.document, ancestor.node) {
        if is_text(base, descendant) {
            if is_contained(base, ancestor.document, state, descendant) {
                units.extend_from_slice(&document.character_data(descendant.node).units());
            }
        } else {
            append_contained_text(document, state, descendant, units);
        }
    }
}

/// Whether `id` is a doctype, which can never hold a boundary point
/// (<https://dom.spec.whatwg.org/#concept-range-bp>).
pub(crate) fn is_doctype(base: &blitz_dom::BaseDocument, id: NodeId) -> bool {
    matches!(
        base.get_node(id.node).map(|node| &node.data),
        Some(NodeData::Doctype { .. })
    )
}

fn parent_of(base: &blitz_dom::BaseDocument, id: NodeId) -> Option<NodeId> {
    base.get_node(id.node)
        .and_then(|node| node.parent)
        .map(|parent| NodeId {
            document: id.document,
            node: parent,
        })
}

/// Whether `possible` is `node` or an ancestor of it, staying in one arena.
/// Stale ids compare false.
pub(crate) fn is_inclusive_ancestor(
    base: &blitz_dom::BaseDocument,
    possible: NodeId,
    node: NodeId,
) -> bool {
    if possible.document != node.document {
        return false;
    }
    let mut cursor = node.node;
    loop {
        if cursor == possible.node {
            return true;
        }
        let Some(parent) = base.get_node(cursor).and_then(|node| node.parent) else {
            return false;
        };
        cursor = parent;
    }
}

/// Whether `possible` is an inclusive descendant of `node`.
fn is_inclusive_descendant(base: &blitz_dom::BaseDocument, possible: NodeId, node: NodeId) -> bool {
    is_inclusive_ancestor(base, node, possible)
}

/// The position of `(node_a, offset_a)` relative to `(node_b, offset_b)`.
/// Both points must share a root; callers check that first
/// (<https://dom.spec.whatwg.org/#concept-range-bp-after>).
pub(crate) fn position(
    base: &blitz_dom::BaseDocument,
    document: u32,
    a: BoundaryPoint,
    b: BoundaryPoint,
) -> Ordering {
    if a.node == b.node {
        return a.offset.cmp(&b.offset);
    }
    // If A follows B, decide the reverse and flip.
    if crate::dom::tree::tree_order(base, document, b.node.node, a.node.node) == Ordering::Less {
        return position(base, document, b, a).reverse();
    }
    if is_inclusive_ancestor(base, a.node, b.node) {
        let mut child = b.node.node;
        while base.get_node(child).and_then(|node| node.parent) != Some(a.node.node) {
            let Some(parent) = base.get_node(child).and_then(|node| node.parent) else {
                return Ordering::Less;
            };
            child = parent;
        }
        let index = child_ids(base, document, a.node.node)
            .iter()
            .position(|sibling| sibling.node == child)
            .and_then(|index| u32::try_from(index).ok())
            .unwrap_or(u32::MAX);
        if index < a.offset {
            return Ordering::Greater;
        }
        return Ordering::Less;
    }
    Ordering::Less
}

/// Whether two points share a root. Points in different arenas never do, and
/// a stale id has no root to share.
pub(crate) fn same_root(base: &blitz_dom::BaseDocument, a: NodeId, b: NodeId) -> bool {
    if a.document != b.document {
        return false;
    }
    if base.get_node(a.node).is_none() || base.get_node(b.node).is_none() {
        return false;
    }
    crate::dom::tree::root_of(base, a.node) == crate::dom::tree::root_of(base, b.node)
}

/// The node furthest from the document that contains both `a` and `b`.
/// Callers only invoke this on points that share a root
/// (<https://dom.spec.whatwg.org/#concept-range-common-ancestor>).
pub(crate) fn common_ancestor(base: &blitz_dom::BaseDocument, a: NodeId, b: NodeId) -> NodeId {
    let mut container = a;
    while !is_inclusive_ancestor(base, container, b) {
        let Some(parent) = parent_of(base, container) else {
            break;
        };
        container = parent;
    }
    container
}

/// Whether `node` is contained in the range: same root, after start, and the
/// end of the node before end
/// (<https://dom.spec.whatwg.org/#concept-range-contained>).
pub(crate) fn is_contained(
    base: &blitz_dom::BaseDocument,
    document: u32,
    state: &RangeState,
    node: NodeId,
) -> bool {
    if !same_root(base, node, state.start.node) {
        return false;
    }
    let length = node_length(base, node);
    position(
        base,
        document,
        BoundaryPoint { node, offset: 0 },
        state.start,
    ) == Ordering::Greater
        && position(
            base,
            document,
            BoundaryPoint {
                node,
                offset: length,
            },
            state.end,
        ) == Ordering::Less
}

/// Whether `node` is an inclusive ancestor of exactly one end
/// (<https://dom.spec.whatwg.org/#concept-range-partially-contained>).
pub(crate) fn is_partially_contained(
    base: &blitz_dom::BaseDocument,
    state: &RangeState,
    node: NodeId,
) -> bool {
    is_inclusive_ancestor(base, node, state.start.node)
        != is_inclusive_ancestor(base, node, state.end.node)
}

/// The children of `ancestor` to visit for cloning, extraction, or
/// surrounding: the first and last partially contained children plus the
/// fully contained ones. `want_first`/`want_last` follow the spec's guards:
/// no first partial when the start covers the end, and vice versa. Returns
/// `None` when the endpoints are disconnected from `ancestor`.
pub(crate) fn ancestor_span(
    base: &blitz_dom::BaseDocument,
    document: u32,
    state: &RangeState,
    ancestor: NodeId,
    want_first: bool,
    want_last: bool,
) -> Option<(Option<NodeId>, Vec<NodeId>, Option<NodeId>)> {
    if !is_inclusive_ancestor(base, ancestor, state.start.node)
        || !is_inclusive_ancestor(base, ancestor, state.end.node)
    {
        return None;
    }
    let mut first = None;
    let mut last = None;
    let mut contained = Vec::new();
    for child in child_ids(base, ancestor.document, ancestor.node) {
        if is_partially_contained(base, state, child) {
            if want_first && first.is_none() {
                first = Some(child);
            }
            if want_last {
                last = Some(child);
            }
        } else if is_contained(base, document, state, child) {
            contained.push(child);
        }
    }
    Some((first, contained, last))
}

/// Shift boundary points in `parent` past an insertion of `count` children
/// at `index` (<https://dom.spec.whatwg.org/#concept-node-insert>).
pub(crate) fn adjust_for_insert(ranges: &mut RangeTable, parent: NodeId, index: u32, count: u32) {
    for range in ranges.iter_mut() {
        for point in [&mut range.start, &mut range.end] {
            if point.node == parent && point.offset > index {
                point.offset = point.offset.saturating_add(count);
            }
        }
    }
}

/// Reparent boundary points for the pre-removal of `removed` at `index` of
/// `parent` (<https://dom.spec.whatwg.org/#concept-node-remove> live-range
/// pre-remove steps).
pub(crate) fn adjust_for_remove(
    base: &blitz_dom::BaseDocument,
    ranges: &mut RangeTable,
    parent: NodeId,
    index: u32,
    removed: NodeId,
) {
    for range in ranges.iter_mut() {
        for point in [&mut range.start, &mut range.end] {
            if is_inclusive_descendant(base, point.node, removed) {
                *point = BoundaryPoint {
                    node: parent,
                    offset: index,
                };
            } else if point.node == parent && point.offset > index {
                point.offset = point.offset.saturating_sub(1);
            }
        }
    }
}

/// Adjust boundary points for replacing `count` units at `offset` of `node`
/// with `length` units
/// (<https://dom.spec.whatwg.org/#concept-cd-replace>).
pub(crate) fn adjust_for_replace_data(
    ranges: &mut RangeTable,
    node: NodeId,
    offset: u32,
    count: u32,
    length: u32,
) {
    for range in ranges.iter_mut() {
        for point in [&mut range.start, &mut range.end] {
            if point.node != node {
                continue;
            }
            if point.offset > offset && point.offset <= offset.saturating_add(count) {
                point.offset = offset;
            } else if point.offset > offset.saturating_add(count) {
                point.offset = point
                    .offset
                    .saturating_add(length)
                    .saturating_sub(count.min(point.offset.saturating_sub(offset)));
            }
        }
    }
}

/// Adjust boundary points for merging `sibling` into `node` during
/// `normalize`: points in the dying node move over at the accumulated
/// length, and parent points at its slot move to the merge target
/// (<https://dom.spec.whatwg.org/#dom-node-normalize>).
pub(crate) fn adjust_for_normalize(
    ranges: &mut RangeTable,
    node: NodeId,
    sibling: NodeId,
    parent: NodeId,
    index: u32,
    length: u32,
) {
    for range in ranges.iter_mut() {
        for point in [&mut range.start, &mut range.end] {
            if point.node == sibling {
                point.node = node;
                point.offset = point.offset.saturating_add(length);
            } else if point.node == parent && point.offset == index {
                point.node = node;
                point.offset = length;
            }
        }
    }
}

/// Adjust boundary points for splitting `node` at `offset` into `new_node`
/// after it: points past the split move over, and parent points at the old
/// next-sibling slot shift (<https://dom.spec.whatwg.org/#concept-text-split>).
pub(crate) fn adjust_for_split(
    ranges: &mut RangeTable,
    node: NodeId,
    new_node: NodeId,
    parent: NodeId,
    index: u32,
    offset: u32,
) {
    for range in ranges.iter_mut() {
        for point in [&mut range.start, &mut range.end] {
            if point.node == node && point.offset > offset {
                point.node = new_node;
                point.offset -= offset;
            }
            if point.node == parent && point.offset == index.saturating_add(1) {
                point.offset += 1;
            }
        }
    }
}

/// Collapse boundary points for replacing every child of `parent`: the
/// removals run the pre-remove steps in tree order, so every affected point
/// lands on `(parent, 0)`
/// (<https://dom.spec.whatwg.org/#concept-node-replace-all>).
pub(crate) fn adjust_for_replace_all(
    base: &blitz_dom::BaseDocument,
    ranges: &mut RangeTable,
    parent: NodeId,
) {
    for range in ranges.iter_mut() {
        for point in [&mut range.start, &mut range.end] {
            if point.node == parent {
                if point.offset > 0 {
                    point.offset = 0;
                }
            } else if point.node != parent && is_inclusive_descendant(base, point.node, parent) {
                *point = BoundaryPoint {
                    node: parent,
                    offset: 0,
                };
            }
        }
    }
}
