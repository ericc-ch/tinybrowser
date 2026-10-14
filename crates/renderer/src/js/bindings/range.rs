//! `Range` and `AbstractRange` (<https://dom.spec.whatwg.org/#ranges>).

use std::cmp::Ordering;
use std::collections::BTreeMap;

use rquickjs::{Class, Ctx, Exception, Result, Value, class::Trace};

use super::{
    character_data, fixup_focus_after_removal, realm_registry, required_node,
    schedule_mutation_delivery, set_character_data, throw_dom, throw_dom_error, world,
    world_for_node, wrap_node,
};
use super::host::{NodeReference, instance_proto};
use super::node::core::{adopt_node, insert_tree_node, insertion_tree_nodes, split_text_node};
use super::node::document::{
    fragment_base_url, html_fragment_context, parse_html_fragment_snapshots,
};
use crate::dom::adoption::{ImportSnapshot, clone_within_document, materialize_children};
use crate::dom::insertion::{
    iframe_containers_in_subtrees, place_journaled, replace_all_journaled, unlink_journaled,
};
use crate::dom::range as ranges;
use crate::dom::range::{BoundaryPoint, RangeState};
use crate::dom::tree::{NodeId, child_ids, sibling};
use crate::dom_string::DomString;

/// `Range` (and `AbstractRange`) payload: an index into the owning document's
/// live-range table (<https://dom.spec.whatwg.org/#interface-range>).
#[derive(Trace, rquickjs::JsLifetime)]
#[rquickjs::class(rename = "Range")]
pub(crate) struct JsRange {
    document: u32,
    slot: usize,
}

include!(concat!(env!("OUT_DIR"), "/Range.rs"));
include!(concat!(env!("OUT_DIR"), "/AbstractRange.rs"));

pub(crate) fn install(ctx: &Ctx<'_>) -> Result<()> {
    range_generated::install(ctx)?;
    abstract_range_generated::install(ctx)?;
    Ok(())
}

impl super::host::SharedClass for JsRange {
    // https://dom.spec.whatwg.org/#interface-range
    // `Range` objects implement `Range` (and `AbstractRange` through the
    // prototype chain); anything else reaching the dispatch is an
    // incompatible receiver.
    fn require_interface(&self, ctx: &Ctx<'_>, interface: &str) -> Result<()> {
        if matches!(interface, "Range" | "AbstractRange") {
            Ok(())
        } else {
            Err(Exception::throw_type(ctx, "incompatible receiver"))
        }
    }
}

/// Allocate a collapsed range at `(root, 0)` of `root`'s document and wrap
/// it. Backs `new Range()` through the brands constructor.
pub(crate) fn new_range<'js>(ctx: &Ctx<'js>, root: NodeId) -> Result<Value<'js>> {
    let point = BoundaryPoint { node: root, offset: 0 };
    let (document, slot) = alloc_range(
        ctx,
        root.document,
        RangeState {
            start: point,
            end: point,
        },
    )?;
    wrap_range(ctx, document, slot)
}

/// A synthetic body element when the start node supplies no context element:
/// body in the HTML namespace, owned by the range's node document
/// (<https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-range-createcontextualfragment>).
fn synthetic_body_name() -> markup5ever::QualName {
    markup5ever::QualName::new(
        None,
        crate::dom::tree::html_namespace(),
        markup5ever::LocalName::from("body"),
    )
}

/// Select the contextual element and content type for a fragment parse: an
/// element itself, or the parent element of text, CDATA, and comments. Other
/// start nodes have no element context, so the fragment parser falls back to
/// a body context
/// (<https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-range-createcontextualfragment>).
fn contextual_fragment_context(
    ctx: &Ctx<'_>,
    start: NodeId,
) -> Result<(NodeId, markup5ever::QualName, &'static str)> {
    let world_rc = world(ctx)?;
    let world = world_rc.borrow();
    let store = world.store();
    let Some(parsed) = store.get(start.document) else {
        return Err(Exception::throw_type(ctx, "stale range"));
    };
    let content_type = parsed.content_type;
    let base = &parsed.document.base;
    let name = match base.get_node(start.node).map(|node| &node.data) {
        Some(blitz_dom::NodeData::Element(element)) => Some((start, element.name.clone())),
        Some(
            blitz_dom::NodeData::Text(_)
            | blitz_dom::NodeData::CDataSection { .. }
            | blitz_dom::NodeData::Comment { .. },
        ) => parent_of_raw(base, start).and_then(|parent| {
            match base.get_node(parent.node).map(|node| &node.data) {
                Some(blitz_dom::NodeData::Element(element)) => {
                    Some((parent, element.name.clone()))
                }
                _ => None,
            }
        }),
        _ => None,
    };
    let (context, name) = name.unwrap_or((start, synthetic_body_name()));
    Ok((context, name, content_type))
}

/// Parse `markup` as an HTML fragment in the target document and wrap the
/// result (<https://html.spec.whatwg.org/multipage/parsing.html#html-fragment-parsing-algorithm>).
fn html_contextual_fragment<'js>(
    ctx: &Ctx<'js>,
    start: NodeId,
    markup: &str,
    name: &markup5ever::QualName,
) -> Result<Value<'js>> {
    let context = if name.local.as_str() == "html"
        && name.ns == crate::dom::tree::html_namespace()
    {
        "body".to_owned()
    } else {
        html_fragment_context(name)
    };
    let (base_url, font_ctx) = fragment_base_url(ctx, start);
    let snapshots = parse_html_fragment_snapshots(markup, &context, &base_url, font_ctx);
    materialize_fragment_value(ctx, start, &snapshots)
}

/// Create a fragment owned by `start`'s document from parsed snapshots.
fn materialize_fragment_value<'js>(
    ctx: &Ctx<'js>,
    start: NodeId,
    snapshots: &[ImportSnapshot],
) -> Result<Value<'js>> {
    let owner = world_for_node(ctx, start)?;
    let world = owner.borrow();
    let Some(mut parsed) = world.document_mut(start) else {
        return Err(Exception::throw_type(ctx, "stale range"));
    };
    let fragment = materialize_children(&mut parsed.document, start.document, snapshots)
        .map_err(|error| throw_dom_error(ctx, error))?;
    drop(parsed);
    drop(world);
    wrap_node(ctx, fragment)
}

/// Escape an XML attribute value for a synthesized wrapper start tag.
fn escape_xml_attribute(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            '\t' => escaped.push_str("&#9;"),
            '\n' => escaped.push_str("&#10;"),
            '\r' => escaped.push_str("&#13;"),
            character => escaped.push(character),
        }
    }
    escaped
}

/// In-scope namespace bindings at `context`: each mapped prefix plus the
/// default namespace, used for the synthesized fragment wrapper
/// (<https://html.spec.whatwg.org/multipage/xhtml.html#parsing-xml-fragments>).
fn xml_scope(
    document: &crate::documents::BlitzDocument,
    context: NodeId,
    default_override: Option<&str>,
) -> (Option<String>, BTreeMap<String, String>) {
    const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";
    const XMLNS_NAMESPACE: &str = "http://www.w3.org/2000/xmlns/";
    let mut prefixes = BTreeMap::new();
    prefixes.insert("xml".to_owned(), XML_NAMESPACE.to_owned());
    let mut default = default_override.map(str::to_owned);
    let mut default_seen = default.is_some();
    let mut current = Some(context);
    while let Some(id) = current {
        let Some(node) = document.base.get_node(id.node) else {
            break;
        };
        if let Some(element) = node.data.downcast_element() {
            for attribute in element.attrs.iter() {
                if attribute.name.ns.as_str() != XMLNS_NAMESPACE {
                    continue;
                }
                match attribute.name.prefix.as_ref().map(markup5ever::Prefix::as_str) {
                    Some("xmlns") => {
                        if attribute.name.local.as_str() != "xmlns" {
                            prefixes
                                .entry(attribute.name.local.to_string())
                                .or_insert_with(|| attribute.value.clone());
                        }
                    }
                    None if attribute.name.local.as_str() == "xmlns" && !default_seen => {
                        default_seen = true;
                        if !attribute.value.is_empty() {
                            default = Some(attribute.value.clone());
                        }
                    }
                    _ => {}
                }
            }
        }
        current = node.parent.map(|parent| NodeId {
            document: id.document,
            node: parent,
        });
    }
    (default, prefixes)
}

/// Wrap `markup` in one container carrying the context's in-scope bindings,
/// so the XML parser resolves its names the same way.
fn xml_fragment_source(
    default: Option<&str>,
    prefixes: &BTreeMap<String, String>,
    markup: &str,
) -> String {
    const WRAPPER: &str = "tinybrowser-xml-fragment";
    let mut source = String::with_capacity(markup.len() + 128);
    source.push('<');
    source.push_str(WRAPPER);
    for (prefix, uri) in prefixes {
        source.push_str(" xmlns:");
        source.push_str(prefix);
        source.push_str("=\"");
        source.push_str(&escape_xml_attribute(uri));
        source.push('"');
    }
    if let Some(uri) = default {
        source.push_str(" xmlns=\"");
        source.push_str(&escape_xml_attribute(uri));
        source.push('"');
    }
    source.push('>');
    source.push_str(markup);
    source.push_str("</");
    source.push_str(WRAPPER);
    source.push('>');
    source
}

/// Parse `markup` as an XML fragment with the target's in-scope bindings and
/// wrap the imported result
/// (<https://html.spec.whatwg.org/multipage/xhtml.html#parsing-xml-fragments>).
fn xml_contextual_fragment<'js>(
    ctx: &Ctx<'js>,
    start: NodeId,
    name: &markup5ever::QualName,
    markup: &str,
) -> Result<Value<'js>> {
    let (default, prefixes, base_url, font_ctx) = {
        let world_rc = world(ctx)?;
        let world = world_rc.borrow();
        let Some(parsed) = world.document(start) else {
            return Err(Exception::throw_type(ctx, "stale range"));
        };
        let default_override = if name.local.as_str() == "html"
            && name.ns == crate::dom::tree::html_namespace()
        {
            Some("http://www.w3.org/1999/xhtml")
        } else {
            None
        };
        let scope = xml_scope(&parsed.document, start, default_override);
        let base_url = parsed.document.base.base_url().as_str().to_owned();
        let font_ctx = world.runtime.font_ctx.clone();
        (scope.0, scope.1, base_url, font_ctx)
    };
    let source = xml_fragment_source(default.as_deref(), &prefixes, markup);
    let mut base = blitz_dom::BaseDocument::new(blitz_dom::DocumentConfig {
        base_url: Some(base_url),
        font_ctx: Some(font_ctx),
        ua_stylesheets: Vec::new(),
        ..blitz_dom::DocumentConfig::default()
    });
    {
        let mut mutator = base.mutate();
        blitz_html::DocumentHtmlParser::parse_xml_into_mutator(&mut mutator, &source);
    }
    let mut scratch = crate::documents::BlitzDocument::from_base(base);
    if !scratch.base.take_parse_errors().is_empty() {
        return Err(throw_dom(ctx, "SyntaxError", "markup is not well-formed XML"));
    }
    let children = {
        let root = scratch.base.root_node().id;
        let top = scratch
            .base
            .get_node(root)
            .map(|node| node.children.clone())
            .unwrap_or_default();
        if top.len() != 1 {
            return Err(throw_dom(ctx, "SyntaxError", "markup is not well-formed XML"));
        }
        let wrapper = NodeId {
            document: start.document,
            node: top[0],
        };
        scratch
            .base
            .get_node(wrapper.node)
            .map(|node| node.children.clone())
            .unwrap_or_default()
            .into_iter()
            .map(|child| NodeId {
                document: start.document,
                node: child,
            })
            .collect::<Vec<_>>()
    };
    let snapshots = children
        .into_iter()
        .map(|child| {
            crate::dom::adoption::import_snapshot(&scratch, child, true).ok_or_else(|| {
                throw_dom(ctx, "SyntaxError", "markup is not well-formed XML")
            })
        })
        .collect::<Result<Vec<ImportSnapshot>>>()?;
    materialize_fragment_value(ctx, start, &snapshots)
}

/// Create a contextual fragment from `markup` for the range's start node
/// (<https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-range-createcontextualfragment>).
fn create_contextual_fragment<'js>(ctx: &Ctx<'js>, start: NodeId, markup: &str) -> Result<Value<'js>> {
    let (context, name, content_type) = contextual_fragment_context(ctx, start)?;
    if content_type == "text/html" {
        html_contextual_fragment(ctx, context, markup, &name)
    } else {
        xml_contextual_fragment(ctx, start, &name, markup)
    }
}

/// Allocate a collapsed range at `(document, 0)` and wrap it. Backs
/// `document.createRange()`
/// (<https://dom.spec.whatwg.org/#dom-document-createrange>).
pub(crate) fn create_range_value<'js>(ctx: &Ctx<'js>, document: NodeId) -> Result<Value<'js>> {
    let point = BoundaryPoint {
        node: document,
        offset: 0,
    };
    let (store, slot) = alloc_range(
        ctx,
        document.document,
        RangeState {
            start: point,
            end: point,
        },
    )?;
    wrap_range(ctx, store, slot)
}

/// Wrap the registered range as a platform object with the owner realm's
/// `Range` prototype
/// (<https://webidl.spec.whatwg.org/#internally-create-a-new-object-implementing-the-interface>).
pub(crate) fn wrap_range<'js>(ctx: &Ctx<'js>, document: u32, slot: usize) -> Result<Value<'js>> {
    let world = world(ctx)?;
    let proto = world
        .borrow()
        .brand("Range")
        .ok_or_else(|| Exception::throw_internal(ctx, "missing Range prototype"))?;
    let proto = proto.restore(ctx)?;
    Ok(Class::into_value(instance_proto(
        JsRange { document, slot },
        proto,
    )?))
}

fn load(ctx: &Ctx<'_>, document: u32, slot: usize) -> Result<RangeState> {
    let world_rc = world(ctx)?;
    let world = world_rc.borrow();
    let store = world.store();
    store
        .get(document)
        .and_then(|parsed| parsed.document.ranges.get(slot))
        .copied()
        .ok_or_else(|| Exception::throw_type(ctx, "stale range"))
}

fn save(ctx: &Ctx<'_>, document: u32, slot: usize, state: RangeState) -> Result<()> {
    let world_rc = world(ctx)?;
    let world = world_rc.borrow();
    let Some(mut parsed) = world.document_mut_by_id(document) else {
        return Err(Exception::throw_type(ctx, "stale range"));
    };
    let Some(entry) = parsed.document.ranges.get_mut(slot) else {
        return Err(Exception::throw_type(ctx, "stale range"));
    };
    *entry = state;
    Ok(())
}

fn alloc_range(ctx: &Ctx<'_>, document: u32, state: RangeState) -> Result<(u32, usize)> {
    let world_rc = world(ctx)?;
    let world = world_rc.borrow();
    let Some(mut parsed) = world.document_mut_by_id(document) else {
        return Err(Exception::throw_type(ctx, "stale range"));
    };
    let slot = ranges::alloc(&mut parsed.document.ranges, state);
    Ok((parsed.id, slot))
}

/// Tree nodes hold boundary points; attribute nodes live outside the Blitz
/// tree, so they are rejected until points can represent them.
fn tree_node(ctx: &Ctx<'_>, node: NodeReference) -> Result<NodeId> {
    node.tree().ok_or_else(|| {
        throw_dom(
            ctx,
            "InvalidNodeTypeError",
            "attribute nodes cannot hold a range boundary",
        )
    })
}

fn read_document<R>(
    ctx: &Ctx<'_>,
    document: u32,
    read: impl FnOnce(&blitz_dom::BaseDocument) -> Result<R>,
) -> Result<R> {
    let world_rc = world(ctx)?;
    let world = world_rc.borrow();
    let store = world.store();
    let Some(parsed) = store.get(document) else {
        return Err(Exception::throw_type(ctx, "stale range"));
    };
    read(&parsed.document.base)
}

/// Validate one boundary point: doctypes never hold one, and the offset must
/// fit the node (<https://dom.spec.whatwg.org/#concept-range-bp>).
fn check_point(
    ctx: &Ctx<'_>,
    base: &blitz_dom::BaseDocument,
    node: NodeId,
    offset: u32,
) -> Result<BoundaryPoint> {
    if ranges::is_doctype(base, node) {
        return Err(throw_dom(
            ctx,
            "InvalidNodeTypeError",
            "a doctype cannot hold a range boundary",
        ));
    }
    if offset > ranges::node_length(base, node) {
        return Err(throw_dom(ctx, "IndexSizeError", "offset is outside the node"));
    }
    Ok(BoundaryPoint { node, offset })
}

/// Whether two nodes share a root, resolving each side in its own arena.
fn same_tree(ctx: &Ctx<'_>, a: NodeId, b: NodeId) -> Result<bool> {
    if a.document != b.document {
        return Ok(false);
    }
    read_document(ctx, a.document, |base| {
        Ok(ranges::same_root(base, a, b))
    })
}

/// Set the start, collapsing the end when the roots differ or the new point
/// passes it (<https://dom.spec.whatwg.org/#concept-range-set-the-start>).
fn set_start_point(state: &mut RangeState, base: &blitz_dom::BaseDocument, point: BoundaryPoint) {
    if !ranges::same_root(base, state.start.node, point.node)
        || ranges::position(base, point.node.document, point, state.end) == Ordering::Greater
    {
        state.end = point;
    }
    state.start = point;
}

/// Set the end, collapsing the start when the roots differ or the new point
/// precedes it (<https://dom.spec.whatwg.org/#concept-range-set-the-end>).
fn set_end_point(state: &mut RangeState, base: &blitz_dom::BaseDocument, point: BoundaryPoint) {
    if !ranges::same_root(base, state.end.node, point.node)
        || ranges::position(base, point.node.document, point, state.start) == Ordering::Less
    {
        state.start = point;
    }
    state.end = point;
}

/// The index of `child` in its parent, for the sibling-boundary setters.
fn child_index_of(
    ctx: &Ctx<'_>,
    base: &blitz_dom::BaseDocument,
    parent: NodeId,
    child: NodeId,
) -> Result<u32> {
    child_ids(base, parent.document, parent.node)
        .iter()
        .position(|sibling| *sibling == child)
        .and_then(|index| u32::try_from(index).ok())
        .ok_or_else(|| Exception::throw_type(ctx, "stale range"))
}

/// Clamp an IDL offset into UTF-16 units.
fn bound(offset: u32, length: usize) -> usize {
    (offset as usize).min(length)
}

/// The single point deleteContents and extractContents collapse to, from the
/// original endpoints (<https://dom.spec.whatwg.org/#dom-range-deletecontents>).
fn collapse_point(base: &blitz_dom::BaseDocument, state: &RangeState) -> (NodeId, u32) {
    if ranges::is_inclusive_ancestor(base, state.start.node, state.end.node) {
        return (state.start.node, state.start.offset);
    }
    let mut reference = state.start.node;
    loop {
        let Some(parent) = base
            .get_node(reference.node)
            .and_then(|node| node.parent)
            .map(|parent| NodeId {
                document: reference.document,
                node: parent,
            })
        else {
            // Unreachable: the root is an inclusive ancestor of everything in
            // its tree, and both endpoints share a root.
            return (state.start.node, state.start.offset);
        };
        if ranges::is_inclusive_ancestor(base, parent, state.end.node) {
            let index = child_ids(base, parent.document, parent.node)
                .iter()
                .position(|sibling| *sibling == reference)
                .and_then(|index| u32::try_from(index).ok())
                .unwrap_or(0);
            return (parent, index.saturating_add(1));
        }
        reference = parent;
    }
}

/// Topmost contained nodes under `ancestor` in tree order: fully contained
/// children are taken, partially contained ones are descended into.
fn collect_contained(
    base: &blitz_dom::BaseDocument,
    document: u32,
    state: &RangeState,
    ancestor: NodeId,
    out: &mut Vec<NodeId>,
) {
    for child in child_ids(base, document, ancestor.node) {
        if ranges::is_contained(base, document, state, child) {
            out.push(child);
        } else if ranges::is_partially_contained(base, state, child) {
            collect_contained(base, document, state, child, out);
        }
    }
}

/// Create an empty fragment in `document` for extracted or cloned contents.
fn create_fragment_in(ctx: &Ctx<'_>, document: u32) -> Result<NodeId> {
    let world_rc = world(ctx)?;
    let world = world_rc.borrow();
    let Some(mut parsed) = world.document_mut_by_id(document) else {
        return Err(Exception::throw_type(ctx, "stale range"));
    };
    let node = parsed.document.create_fragment();
    Ok(NodeId {
        document: parsed.id,
        node,
    })
}

/// Unlink `nodes` (into `fragment` when given), destroying nested navigables
/// once. Callers schedule observer delivery.
fn move_nodes_out(ctx: &Ctx<'_>, nodes: &[NodeId], fragment: Option<NodeId>) -> Result<()> {
    if nodes.is_empty() {
        return Ok(());
    }
    let world_rc = world(ctx)?;
    let mut iframes = Vec::new();
    for node in nodes {
        let world = world_rc.borrow();
        let Some(parsed) = world.document(*node) else {
            return Err(Exception::throw_type(ctx, "stale range"));
        };
        iframes.extend(iframe_containers_in_subtrees(&parsed, &[*node]));
    }
    {
        let world = world_rc.borrow();
        for node in nodes {
            let Some(mut parsed) = world.document_mut(*node) else {
                return Err(Exception::throw_type(ctx, "stale range"));
            };
            unlink_journaled(&mut parsed, *node);
            if let Some(fragment) = fragment {
                place_journaled(&mut parsed, fragment, *node, None);
            }
            fixup_focus_after_removal(ctx, *node)?;
        }
    }
    world_rc.borrow().destroy_child_navigables(&iframes);
    Ok(())
}

/// Split endpoint data into the in-range slice (for the fragment clone) and
/// the kept remainder (for the original): the tail from `offset` at the
/// start side, the head through `offset` at the end side.
fn split_endpoint_data(data: Vec<u16>, offset: u32, at_start: bool) -> (Vec<u16>, Vec<u16>) {
    let offset = bound(offset, data.len());
    if at_start {
        let mut kept = data;
        let slice = kept.split_off(offset);
        (slice, kept)
    } else {
        let mut slice = data;
        let kept = slice.split_off(offset);
        (slice, kept)
    }
}


/// Clone `source` (deep when asked), replace a Text clone's data, and append
/// it to `fragment`. Returns the clone.
fn clone_into_fragment(
    ctx: &Ctx<'_>,
    fragment: NodeId,
    source: NodeId,
    deep: bool,
    data: Option<Vec<u16>>,
) -> Result<NodeId> {
    let clone = {
        let world_rc = world(ctx)?;
        let world = world_rc.borrow();
        let Some(mut parsed) = world.document_mut(source) else {
            return Err(Exception::throw_type(ctx, "stale range"));
        };
        let store = parsed.id;
        clone_within_document(&mut parsed.document, store, source, deep)
            .map_err(|error| throw_dom_error(ctx, error))?
    };
    if let Some(data) = data {
        set_character_data(ctx, clone, &DomString::from_utf16(data))?;
    }
    {
        let world_rc = world(ctx)?;
        let world = world_rc.borrow();
        let Some(mut parsed) = world.document_mut(fragment) else {
            return Err(Exception::throw_type(ctx, "stale range"));
        };
        place_journaled(&mut parsed, fragment, clone, None);
    }
    Ok(clone)
}

/// Whether the span operation removes source nodes (`extractContents`) or
/// leaves them alone (`cloneContents`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum ContentsTransfer {
    Cloned,
    Moved,
}

impl ContentsTransfer {
    fn verb(self) -> &'static str {
        match self {
            Self::Cloned => "cloned",
            Self::Moved => "extracted",
        }
    }
}

/// One resolved operation span: endpoints, partially contained ends, and the
/// fully contained middle.
type SpanParts = (
    NodeId,
    u32,
    NodeId,
    u32,
    Option<NodeId>,
    Vec<NodeId>,
    Option<NodeId>,
);

/// Resolve one span into its partially contained endpoints and fully
/// contained children, rejecting doctypes before any tree changes.
fn span_parts(ctx: &Ctx<'_>, state: &RangeState) -> Result<SpanParts> {
    let (snode, soff, enode, eoff) = (
        state.start.node,
        state.start.offset,
        state.end.node,
        state.end.offset,
    );
    let (first, contained, last) = read_document(ctx, snode.document, |base| {
        let ancestor = ranges::common_ancestor(base, snode, enode);
        // A partial endpoint only exists on the side its node does not
        // cover; otherwise the span would recurse onto itself.
        let want_first = !ranges::is_inclusive_ancestor(base, snode, enode);
        let want_last = !ranges::is_inclusive_ancestor(base, enode, snode);
        Ok(ranges::ancestor_span(
            base,
            state.start.node.document,
            state,
            ancestor,
            want_first,
            want_last,
        )
        .unwrap_or((None, Vec::new(), None)))
    })?;
    Ok((snode, soff, enode, eoff, first, contained, last))
}

/// Reject a doctype anywhere in the span before any tree changes.
fn reject_span_doctypes(ctx: &Ctx<'_>, contained: &[NodeId], verb: &str) -> Result<()> {
    for child in contained {
        let is_doctype = read_document(ctx, child.document, |base| {
            Ok(ranges::is_doctype(base, *child))
        })?;
        if is_doctype {
            return Err(throw_dom(
                ctx,
                "HierarchyRequestError",
                &format!("a doctype cannot be {verb}"),
            ));
        }
    }
    Ok(())
}

/// Transfer the in-range slice of one Text endpoint, deleting it from the
/// original when the operation moves nodes.
fn transfer_endpoint_text(
    ctx: &Ctx<'_>,
    fragment: NodeId,
    node: NodeId,
    offset: u32,
    at_start: bool,
    transfer: ContentsTransfer,
) -> Result<()> {
    let data = character_data(ctx, node)?.units().into_owned();
    let (slice, kept) = split_endpoint_data(data, offset, at_start);
    clone_into_fragment(ctx, fragment, node, false, Some(slice))?;
    if transfer == ContentsTransfer::Moved {
        set_character_data(ctx, node, &DomString::from_utf16(kept))?;
    }
    Ok(())
}

/// Clone a partially contained non-Text endpoint shell and recurse the
/// operation into it, moving the recursion target when the operation moves.
fn transfer_partial_element(
    ctx: &Ctx<'_>,
    fragment: NodeId,
    node: NodeId,
    sub: &RangeState,
    transfer: ContentsTransfer,
) -> Result<()> {
    let clone = clone_into_fragment(ctx, fragment, node, false, None)?;
    let scratch = create_fragment_in(ctx, fragment.document)?;
    produce_contents(ctx, scratch, sub, transfer)?;
    move_nodes_out(
        ctx,
        &read_document(ctx, scratch.document, |base| {
            Ok(child_ids(base, scratch.document, scratch.node))
        })?,
        Some(clone),
    )?;
    Ok(())
}

/// Transfer one non-Text endpoint's contents.
fn transfer_partial_endpoint(
    ctx: &Ctx<'_>,
    fragment: NodeId,
    sub: &RangeState,
    node: NodeId,
    at_start: bool,
    transfer: ContentsTransfer,
) -> Result<()> {
    let is_character_data = read_document(ctx, node.document, |base| {
        Ok(ranges::is_character_data(base, node))
    })?;
    if is_character_data {
        // The sub-range's start side holds the real start offset when this
        // is the first partially contained child, and its end side holds
        // the real end offset when it is the last
        // (<https://dom.spec.whatwg.org/#concept-range-extract>).
        let offset = if at_start {
            sub.start.offset
        } else {
            sub.end.offset
        };
        return transfer_endpoint_text(ctx, fragment, node, offset, at_start, transfer);
    }
    transfer_partial_element(ctx, fragment, node, sub, transfer)
}

/// Transfer the middle of a span: fully contained children move out or clone
/// in place.
fn transfer_middle(
    ctx: &Ctx<'_>,
    fragment: NodeId,
    contained: &[NodeId],
    transfer: ContentsTransfer,
) -> Result<()> {
    if transfer == ContentsTransfer::Moved {
        move_nodes_out(ctx, contained, Some(fragment))
    } else {
        for child in contained {
            clone_into_fragment(ctx, fragment, *child, true, None)?;
        }
        Ok(())
    }
}

/// Run the extract algorithm for `state` into `fragment`, on captured
/// originals; registered ranges (including the caller's, already moved)
/// track through the generic hooks
/// (<https://dom.spec.whatwg.org/#dom-range-extractcontents>).
fn extract_into(ctx: &Ctx<'_>, fragment: NodeId, state: &RangeState) -> Result<()> {
    produce_contents(ctx, fragment, state, ContentsTransfer::Moved)
}

/// Run the clone/extract-contents algorithm for `state` into `fragment`:
/// like-kind source handling differs only in whether source removals happen
/// (<https://dom.spec.whatwg.org/#dom-range-clonecontents>,
/// <https://dom.spec.whatwg.org/#dom-range-extractcontents>).
fn produce_contents(
    ctx: &Ctx<'_>,
    fragment: NodeId,
    state: &RangeState,
    transfer: ContentsTransfer,
) -> Result<()> {
    let (snode, soff, enode, eoff, first, contained, last) = span_parts(ctx, state)?;
    if state.start == state.end {
        return Ok(());
    }
    // Same CharacterData node: only the slice moves, and the replacement
    // collapses the boundary onto its start.
    let same_text = read_document(ctx, snode.document, |base| {
        Ok(snode == enode && ranges::is_character_data(base, snode))
    })?;
    if same_text {
        let data = character_data(ctx, snode)?.units().into_owned();
        let start = bound(soff, data.len());
        let end = bound(eoff, data.len()).max(start);
        clone_into_fragment(
            ctx,
            fragment,
            snode,
            false,
            Some(data[start..end].to_vec()),
        )?;
        if transfer == ContentsTransfer::Moved {
            let mut kept = data;
            kept.drain(start..end);
            set_character_data(ctx, snode, &DomString::from_utf16(kept))?;
        }
        return Ok(());
    }
    reject_span_doctypes(ctx, &contained, transfer.verb())?;
    if let Some(first) = first {
        let sub = RangeState {
            start: BoundaryPoint { node: snode, offset: soff },
            end: BoundaryPoint {
                node: first,
                offset: read_document(ctx, first.document, |base| {
                    Ok(ranges::node_length(base, first))
                })?,
            },
        };
        transfer_partial_endpoint(ctx, fragment, &sub, first, true, transfer)?;
    }
    transfer_middle(ctx, fragment, &contained, transfer)?;
    if let Some(last) = last {
        let sub = RangeState {
            start: BoundaryPoint { node: last, offset: 0 },
            end: BoundaryPoint { node: enode, offset: eoff },
        };
        transfer_partial_endpoint(ctx, fragment, &sub, last, false, transfer)?;
    }
    Ok(())
}
/// Run the clone-contents algorithm for `state` into `fragment`: like
/// extraction but the source tree is never touched
/// (<https://dom.spec.whatwg.org/#dom-range-clonecontents>).
fn clone_into(ctx: &Ctx<'_>, fragment: NodeId, state: &RangeState) -> Result<()> {
    produce_contents(ctx, fragment, state, ContentsTransfer::Cloned)
}


fn parent_of_raw(base: &blitz_dom::BaseDocument, id: NodeId) -> Option<NodeId> {
    base.get_node(id.node)
        .and_then(|node| node.parent)
        .map(|parent| NodeId {
            document: id.document,
            node: parent,
        })
}

fn parent_of(ctx: &Ctx<'_>, base: &blitz_dom::BaseDocument, id: NodeId) -> Result<NodeId> {
    parent_of_raw(base, id)
        .ok_or_else(|| throw_dom(ctx, "InvalidNodeTypeError", "the node has no parent"))
}

/// Reject a `surroundContents` parent that can never hold the contents:
/// documents, doctypes, and fragments (including fragment backings, which
/// are plain elements in the arena), before any mutation
/// (<https://dom.spec.whatwg.org/#dom-range-surroundcontents>).
fn check_surround_parent(ctx: &Ctx<'_>, new_parent: NodeId) -> Result<()> {
    let world_rc = world(ctx)?;
    let world = world_rc.borrow();
    let store = world.store();
    let Some(parsed) = store.get(new_parent.document) else {
        return Err(Exception::throw_type(ctx, "stale range"));
    };
    if parsed.document.is_fragment(new_parent.node) {
        return Err(throw_dom(
            ctx,
            "InvalidNodeTypeError",
            "a fragment cannot surround range contents",
        ));
    }
    let kind = parsed
        .document
        .base
        .get_node(new_parent.node)
        .map(|node| &node.data);
    if matches!(
        kind,
        Some(
            blitz_dom::NodeData::Document(_)
                | blitz_dom::NodeData::Doctype { .. }
                | blitz_dom::NodeData::Fragment { .. }
        )
    ) {
        return Err(throw_dom(
            ctx,
            "InvalidNodeTypeError",
            "only elements can surround range contents",
        ));
    }
    Ok(())
}

/// Appending the fragment is a real append: only elements take children, so
/// character data fails here as a side effect after the extract and insert
/// already ran
/// (<https://dom.spec.whatwg.org/#dom-range-surroundcontents>).
fn ensure_element_parent(ctx: &Ctx<'_>, new_parent: NodeId) -> Result<()> {
    let takes_children = read_document(ctx, new_parent.document, |base| {
        Ok(base
            .get_node(new_parent.node)
            .is_some_and(|node| node.data.downcast_element().is_some()))
    })?;
    if !takes_children {
        return Err(throw_dom(
            ctx,
            "HierarchyRequestError",
            "only elements can hold surrounded contents",
        ));
    }
    Ok(())
}

impl<'js> range_generated::Range<'js> for JsRange {
    // https://dom.spec.whatwg.org/#dom-range-commonancestorcontainer
    fn get_common_ancestor_container(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let state = load(ctx, self.document, self.slot)?;
        if !same_tree(ctx, state.start.node, state.end.node)? {
            return Ok(Value::new_null(ctx.clone()));
        }
        read_document(ctx, state.start.node.document, |base| {
            let ancestor = ranges::common_ancestor(base, state.start.node, state.end.node);
            wrap_node(ctx, ancestor)
        })
    }

    // https://dom.spec.whatwg.org/#dom-range-setstart
    fn set_start(&self, ctx: Ctx<'js>, node: NodeReference, offset: u32) -> Result<()> {
        let id = tree_node(&ctx, node)?;
        let mut state = load(&ctx, self.document, self.slot)?;
        read_document(&ctx, id.document, |base| {
            let point = check_point(&ctx, base, id, offset)?;
            set_start_point(&mut state, base, point);
            Ok(())
        })?;
        save(&ctx, self.document, self.slot, state)
    }

    // https://dom.spec.whatwg.org/#dom-range-setend
    fn set_end(&self, ctx: Ctx<'js>, node: NodeReference, offset: u32) -> Result<()> {
        let id = tree_node(&ctx, node)?;
        let mut state = load(&ctx, self.document, self.slot)?;
        read_document(&ctx, id.document, |base| {
            let point = check_point(&ctx, base, id, offset)?;
            set_end_point(&mut state, base, point);
            Ok(())
        })?;
        save(&ctx, self.document, self.slot, state)
    }

    // https://dom.spec.whatwg.org/#dom-range-setstartbefore
    fn set_start_before(&self, ctx: Ctx<'js>, node: NodeReference) -> Result<()> {
        let id = tree_node(&ctx, node)?;
        let mut state = load(&ctx, self.document, self.slot)?;
        read_document(&ctx, id.document, |base| {
            let parent = parent_of(&ctx, base, id)?;
            let index = child_index_of(&ctx, base, parent, id)?;
            set_start_point(
                &mut state,
                base,
                BoundaryPoint { node: parent, offset: index },
            );
            Ok(())
        })?;
        save(&ctx, self.document, self.slot, state)
    }

    // https://dom.spec.whatwg.org/#dom-range-setstartafter
    fn set_start_after(&self, ctx: Ctx<'js>, node: NodeReference) -> Result<()> {
        let id = tree_node(&ctx, node)?;
        let mut state = load(&ctx, self.document, self.slot)?;
        read_document(&ctx, id.document, |base| {
            let parent = parent_of(&ctx, base, id)?;
            let index = child_index_of(&ctx, base, parent, id)?;
            set_start_point(
                &mut state,
                base,
                BoundaryPoint {
                    node: parent,
                    offset: index.saturating_add(1),
                },
            );
            Ok(())
        })?;
        save(&ctx, self.document, self.slot, state)
    }

    // https://dom.spec.whatwg.org/#dom-range-setendbefore
    fn set_end_before(&self, ctx: Ctx<'js>, node: NodeReference) -> Result<()> {
        let id = tree_node(&ctx, node)?;
        let mut state = load(&ctx, self.document, self.slot)?;
        read_document(&ctx, id.document, |base| {
            let parent = parent_of(&ctx, base, id)?;
            let index = child_index_of(&ctx, base, parent, id)?;
            set_end_point(
                &mut state,
                base,
                BoundaryPoint { node: parent, offset: index },
            );
            Ok(())
        })?;
        save(&ctx, self.document, self.slot, state)
    }

    // https://dom.spec.whatwg.org/#dom-range-setendafter
    fn set_end_after(&self, ctx: Ctx<'js>, node: NodeReference) -> Result<()> {
        let id = tree_node(&ctx, node)?;
        let mut state = load(&ctx, self.document, self.slot)?;
        read_document(&ctx, id.document, |base| {
            let parent = parent_of(&ctx, base, id)?;
            let index = child_index_of(&ctx, base, parent, id)?;
            set_end_point(
                &mut state,
                base,
                BoundaryPoint {
                    node: parent,
                    offset: index.saturating_add(1),
                },
            );
            Ok(())
        })?;
        save(&ctx, self.document, self.slot, state)
    }

    // https://dom.spec.whatwg.org/#dom-range-collapse
    fn collapse(&self, ctx: Ctx<'js>, to_start: bool) -> Result<()> {
        let mut state = load(&ctx, self.document, self.slot)?;
        if to_start {
            state.end = state.start;
        } else {
            state.start = state.end;
        }
        save(&ctx, self.document, self.slot, state)
    }

    // https://dom.spec.whatwg.org/#dom-range-selectnode
    fn select_node(&self, ctx: Ctx<'js>, node: NodeReference) -> Result<()> {
        let id = tree_node(&ctx, node)?;
        let mut state = load(&ctx, self.document, self.slot)?;
        read_document(&ctx, id.document, |base| {
            let parent = parent_of(&ctx, base, id)?;
            let index = child_index_of(&ctx, base, parent, id)?;
            state.start = BoundaryPoint { node: parent, offset: index };
            state.end = BoundaryPoint {
                node: parent,
                offset: index.saturating_add(1),
            };
            Ok(())
        })?;
        save(&ctx, self.document, self.slot, state)
    }

    // https://dom.spec.whatwg.org/#dom-range-selectnodecontents
    fn select_node_contents(&self, ctx: Ctx<'js>, node: NodeReference) -> Result<()> {
        let id = tree_node(&ctx, node)?;
        let mut state = load(&ctx, self.document, self.slot)?;
        read_document(&ctx, id.document, |base| {
            if ranges::is_doctype(base, id) {
                return Err(throw_dom(
                    &ctx,
                    "InvalidNodeTypeError",
                    "a doctype cannot hold a range boundary",
                ));
            }
            let length = ranges::node_length(base, id);
            state.start = BoundaryPoint { node: id, offset: 0 };
            state.end = BoundaryPoint { node: id, offset: length };
            Ok(())
        })?;
        save(&ctx, self.document, self.slot, state)
    }

    // https://dom.spec.whatwg.org/#dom-range-compareboundarypoints
    fn compare_boundary_points(
        &self,
        ctx: Ctx<'js>,
        how: u16,
        source: Value<'js>,
    ) -> Result<i16> {
        let source = Class::<JsRange>::from_value(&source)?;
        let source = source.borrow();
        // The `how` check precedes the root check
        // (<https://dom.spec.whatwg.org/#dom-range-compareboundarypoints>).
        if !matches!(how, 0..=3) {
            return Err(throw_dom(
                &ctx,
                "NotSupportedError",
                "unknown boundary comparison",
            ));
        }
        let other = load(&ctx, source.document, source.slot)?;
        let state = load(&ctx, self.document, self.slot)?;
        if !same_tree(&ctx, state.start.node, other.start.node)? {
            return Err(throw_dom(
                &ctx,
                "WrongDocumentError",
                "ranges live in different trees",
            ));
        }
        let (mine, theirs) = match how {
            0 => (state.start, other.start),
            1 => (state.end, other.start),
            2 => (state.end, other.end),
            _ => (state.start, other.end),
        };
        read_document(&ctx, state.start.node.document, |base| {
            Ok(match ranges::position(base, state.start.node.document, mine, theirs) {
                Ordering::Less => -1,
                Ordering::Equal => 0,
                Ordering::Greater => 1,
            })
        })
    }

    // https://dom.spec.whatwg.org/#dom-range-clonerange
    fn clone_range(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let state = load(&ctx, self.document, self.slot)?;
        let (document, slot) = alloc_range(&ctx, self.document, state)?;
        wrap_range(&ctx, document, slot)
    }

    // https://dom.spec.whatwg.org/#dom-range-detach
    fn detach(&self, _ctx: Ctx<'js>) -> Result<()> {
        Ok(())
    }

    // https://dom.spec.whatwg.org/#dom-range-stringifier
    fn stringifier(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let state = load(ctx, self.document, self.slot)?;
        let world_rc = world(ctx)?;
        let world = world_rc.borrow();
        let store = world.store();
        let Some(parsed) = store.get(self.document) else {
            return Err(Exception::throw_type(ctx, "stale range"));
        };
        let units = ranges::range_text(&parsed.document, parsed.id, &state);
        rquickjs::String::from_utf16(ctx.clone(), &units)
    }

    // https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-range-createcontextualfragment
    fn create_contextual_fragment(
        &self,
        ctx: Ctx<'js>,
        arg_0: rquickjs::String<'js>,
    ) -> Result<Value<'js>> {
        let state = load(&ctx, self.document, self.slot)?;
        let markup = arg_0.to_string()?;
        create_contextual_fragment(&ctx, state.start.node, &markup)
    }

    // https://dom.spec.whatwg.org/#dom-range-ispointinrange
    fn is_point_in_range(
        &self,
        ctx: Ctx<'js>,
        node: NodeReference,
        offset: u32,
    ) -> Result<bool> {
        let id = tree_node(&ctx, node)?;
        let state = load(&ctx, self.document, self.slot)?;
        if !same_tree(&ctx, id, state.start.node)? {
            return Ok(false);
        }
        read_document(&ctx, id.document, |base| {
            let point = check_point(&ctx, base, id, offset)?;
            Ok(
                ranges::position(base, id.document, point, state.start) != Ordering::Less
                    && ranges::position(base, id.document, point, state.end) != Ordering::Greater,
            )
        })
    }

    // https://dom.spec.whatwg.org/#dom-range-comparepoint
    fn compare_point(&self, ctx: Ctx<'js>, node: NodeReference, offset: u32) -> Result<i16> {
        let id = tree_node(&ctx, node)?;
        let state = load(&ctx, self.document, self.slot)?;
        if !same_tree(&ctx, id, state.start.node)? {
            return Err(throw_dom(
                &ctx,
                "WrongDocumentError",
                "the point lives in a different tree",
            ));
        }
        read_document(&ctx, id.document, |base| {
            let point = check_point(&ctx, base, id, offset)?;
            if ranges::position(base, id.document, point, state.start) == Ordering::Less {
                Ok(-1)
            } else if ranges::position(base, id.document, point, state.end) == Ordering::Greater
            {
                Ok(1)
            } else {
                Ok(0)
            }
        })
    }

    // https://dom.spec.whatwg.org/#dom-range-intersectsnode
    fn intersects_node(&self, ctx: Ctx<'js>, node: NodeReference) -> Result<bool> {
        let id = tree_node(&ctx, node)?;
        let state = load(&ctx, self.document, self.slot)?;
        if !same_tree(&ctx, id, state.start.node)? {
            return Ok(false);
        }
        read_document(&ctx, id.document, |base| {
            let Some(parent) = parent_of_raw(base, id) else {
                return Ok(true);
            };
            let index = child_index_of(&ctx, base, parent, id)?;
            let before_end = ranges::position(
                base,
                id.document,
                BoundaryPoint { node: parent, offset: index },
                state.end,
            ) == Ordering::Less;
            let after_start = ranges::position(
                base,
                id.document,
                BoundaryPoint {
                    node: parent,
                    offset: index.saturating_add(1),
                },
                state.start,
            ) == Ordering::Greater;
            Ok(before_end && after_start)
        })
    }

    // https://dom.spec.whatwg.org/#dom-range-deletecontents
    fn delete_contents(&self, ctx: Ctx<'js>) -> Result<()> {
        let state = load(&ctx, self.document, self.slot)?;
        if state.start == state.end {
            return Ok(());
        }
        let (snode, soff, enode, eoff) = (
            state.start.node,
            state.start.offset,
            state.end.node,
            state.end.offset,
        );
        // Same Text node: replace the middle slice; the generic replacement
        // rule collapses both points onto its start.
        let same_text = read_document(&ctx, snode.document, |base| {
            Ok(snode == enode && ranges::is_character_data(base, snode))
        })?;
        if same_text {
            let data = character_data(&ctx, snode)?.units().into_owned();
            let mut kept = data;
            kept.drain(bound(soff, kept.len())..bound(eoff, kept.len()).max(bound(soff, kept.len())));
            set_character_data(&ctx, snode, &DomString::from_utf16(kept))?;
            return Ok(());
        }
        let (new_node, new_offset, doomed, start_text, end_text) =
            read_document(&ctx, snode.document, |base| {
                let point = collapse_point(base, &state);
                let ancestor = ranges::common_ancestor(base, snode, enode);
                let mut doomed = Vec::new();
                collect_contained(base, snode.document, &state, ancestor, &mut doomed);
                Ok((
                    point.0,
                    point.1,
                    doomed,
                    ranges::is_character_data(base, snode),
                    ranges::is_character_data(base, enode),
                ))
            })?;
        // Collapse first; later mutations track every range through the
        // generic hooks.
        save(
            &ctx,
            self.document,
            self.slot,
            RangeState {
                start: BoundaryPoint {
                    node: new_node,
                    offset: new_offset,
                },
                end: BoundaryPoint {
                    node: new_node,
                    offset: new_offset,
                },
            },
        )?;
        if start_text {
            let data = character_data(&ctx, snode)?.units().into_owned();
            let mut kept = data;
            kept.drain(bound(soff, kept.len())..);
            set_character_data(&ctx, snode, &DomString::from_utf16(kept))?;
        }
        move_nodes_out(&ctx, &doomed, None)?;
        if end_text {
            let data = character_data(&ctx, enode)?.units().into_owned();
            let mut kept = data;
            kept.drain(..bound(eoff, kept.len()));
            set_character_data(&ctx, enode, &DomString::from_utf16(kept))?;
        }
        schedule_mutation_delivery(&ctx)?;
        Ok(())
    }

    // https://dom.spec.whatwg.org/#dom-range-extractcontents
    fn extract_contents(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let state = load(&ctx, self.document, self.slot)?;
        let fragment = create_fragment_in(&ctx, state.start.node.document)?;
        self.extract_into_slot(&ctx, fragment, &state)?;
        wrap_node(&ctx, fragment)
    }

    // https://dom.spec.whatwg.org/#dom-range-clonecontents
    fn clone_contents(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let state = load(&ctx, self.document, self.slot)?;
        let fragment = create_fragment_in(&ctx, state.start.node.document)?;
        clone_into(&ctx, fragment, &state)?;
        wrap_node(&ctx, fragment)
    }

    // https://dom.spec.whatwg.org/#dom-range-insertnode
    fn insert_node(&self, ctx: Ctx<'js>, node: NodeReference) -> Result<()> {
        let arg = tree_node(&ctx, node)?;
        let state = load(&ctx, self.document, self.slot)?;
        let (parent, reference) = read_document(&ctx, state.start.node.document, |base| {
            let start_kind = base.get_node(state.start.node.node).map(|node| &node.data);
            if matches!(
                start_kind,
                Some(
                    blitz_dom::NodeData::ProcessingInstruction { .. }
                        | blitz_dom::NodeData::Comment { .. }
                )
            ) {
                return Err(throw_dom(
                    &ctx,
                    "HierarchyRequestError",
                    "a range starting in a PI or comment takes no nodes",
                ));
            }
            if ranges::is_text(base, state.start.node)
                && parent_of_raw(base, state.start.node).is_none()
            {
                return Err(throw_dom(
                    &ctx,
                    "HierarchyRequestError",
                    "a range starting in detached text takes no nodes",
                ));
            }
            if arg == state.start.node {
                return Err(throw_dom(
                    &ctx,
                    "HierarchyRequestError",
                    "a range cannot be inserted into itself",
                ));
            }
            if ranges::is_text(base, state.start.node) {
                let reference = state.start.node;
                let parent = parent_of(&ctx, base, reference)?;
                Ok((parent, Some(reference)))
            } else {
                let children =
                    child_ids(base, state.start.node.document, state.start.node.node);
                let reference = children
                    .get(state.start.offset as usize)
                    .copied();
                Ok((state.start.node, reference))
            }
        })?;
        let (node, reference) =
            insertion_tree_nodes(&ctx, parent, node, reference.map(NodeReference::Tree))?;
        let node = adopt_node(&ctx, parent, node)?;
        let mut reference = reference;
        if read_document(&ctx, state.start.node.document, |base| {
            Ok(ranges::is_text(base, state.start.node))
        })? {
            // Splitting adjusts live ranges itself; the new node becomes the
            // reference (<https://dom.spec.whatwg.org/#dom-range-insertnode>).
            let split = split_text_node(&ctx, state.start.node, state.start.offset)?;
            reference = Some(required_node(&ctx, &split)?);
        }
        if Some(node) == reference {
            reference = read_document(&ctx, parent.document, |base| {
                Ok(reference.and_then(|reference| {
                    sibling(base, reference.node, true).map(|next| NodeId {
                        document: parent.document,
                        node: next,
                    })
                }))
            })?;
        }
        insert_tree_node(&ctx, parent, node, reference)?;
        // A collapsed range selects the inserted content; anything else rode
        // the generic adjustments.
        let state = load(&ctx, self.document, self.slot)?;
        if state.start == state.end {
            let new_offset = read_document(&ctx, parent.document, |base| {
                Ok(match reference {
                    Some(reference) => child_ids(base, parent.document, parent.node)
                        .iter()
                        .position(|sibling| *sibling == reference)
                        .and_then(|index| u32::try_from(index).ok())
                        .unwrap_or(0),
                    None => ranges::node_length(base, parent),
                })
            })?;
            save(
                &ctx,
                self.document,
                self.slot,
                RangeState {
                    start: state.start,
                    end: BoundaryPoint {
                        node: parent,
                        offset: new_offset,
                    },
                },
            )?;
        }
        Ok(())
    }

    // https://dom.spec.whatwg.org/#dom-range-surroundcontents
    fn surround_contents(&self, ctx: Ctx<'js>, node: NodeReference) -> Result<()> {
        let new_parent = tree_node(&ctx, node)?;
        let state = load(&ctx, self.document, self.slot)?;
        // The partially-contained check runs before the newParent type check
        // (<https://dom.spec.whatwg.org/#dom-range-surroundcontents>).
        read_document(&ctx, state.start.node.document, |base| {
            let ancestor = ranges::common_ancestor(base, state.start.node, state.end.node);
            let (first, _, last) = ranges::ancestor_span(
                base,
                state.start.node.document,
                &state,
                ancestor,
                true,
                true,
            )
            .unwrap_or((None, Vec::new(), None));
            for part in [first, last].into_iter().flatten() {
                if !ranges::is_text(base, part) {
                    return Err(throw_dom(
                        &ctx,
                        "InvalidStateError",
                        "a partially contained non-text node cannot be surrounded",
                    ));
                }
            }
            Ok(())
        })?;
        check_surround_parent(&ctx, new_parent)?;
        let fragment = create_fragment_in(&ctx, state.start.node.document)?;
        self.extract_into_slot(&ctx, fragment, &state)?;
        {
            let world_rc = world(&ctx)?;
            let world = world_rc.borrow();
            let Some(mut parsed) = world.document_mut(new_parent) else {
                return Err(Exception::throw_type(&ctx, "stale range"));
            };
            let iframes = replace_all_journaled(&mut parsed, new_parent, Vec::new());
            drop(parsed);
            world.destroy_child_navigables(&iframes);
            drop(world);
        }
        self.insert_node(ctx.clone(), NodeReference::Tree(new_parent))?;
        // `insertNode` adopts a foreign parent into the range's document and
        // retargets its wrapper: follow the live id so the fragment lands in
        // the adopted copy and the select step reads its real parent
        // (<https://dom.spec.whatwg.org/#concept-node-adopt>).
        let new_parent = realm_registry(&ctx)?.borrow_mut().live_node_id(new_parent);
        ensure_element_parent(&ctx, new_parent)?;
        move_nodes_out(
            &ctx,
            &read_document(&ctx, fragment.document, |base| {
                Ok(child_ids(base, fragment.document, fragment.node))
            })?,
            Some(new_parent),
        )?;
        let (parent, index) = read_document(&ctx, new_parent.document, |base| {
            let parent = parent_of(&ctx, base, new_parent)?;
            let index = child_index_of(&ctx, base, parent, new_parent)?;
            Ok((parent, index))
        })?;
        save(
            &ctx,
            self.document,
            self.slot,
            RangeState {
                start: BoundaryPoint { node: parent, offset: index },
                end: BoundaryPoint {
                    node: parent,
                    offset: index.saturating_add(1),
                },
            },
        )?;
        schedule_mutation_delivery(&ctx)?;
        Ok(())
    }
}

impl<'js> abstract_range_generated::AbstractRange<'js> for JsRange {
    // https://dom.spec.whatwg.org/#dom-abstractrange-startcontainer
    fn get_start_container(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let state = load(ctx, self.document, self.slot)?;
        wrap_node(ctx, state.start.node)
    }

    // https://dom.spec.whatwg.org/#dom-abstractrange-startoffset
    fn get_start_offset(&self, ctx: &Ctx<'js>) -> Result<usize> {
        let offset = load(ctx, self.document, self.slot)?.start.offset;
        Ok(usize::try_from(offset).unwrap_or(usize::MAX))
    }

    // https://dom.spec.whatwg.org/#dom-abstractrange-endcontainer
    fn get_end_container(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let state = load(ctx, self.document, self.slot)?;
        wrap_node(ctx, state.end.node)
    }

    // https://dom.spec.whatwg.org/#dom-abstractrange-endoffset
    fn get_end_offset(&self, ctx: &Ctx<'js>) -> Result<usize> {
        let offset = load(ctx, self.document, self.slot)?.end.offset;
        Ok(usize::try_from(offset).unwrap_or(usize::MAX))
    }

    // https://dom.spec.whatwg.org/#dom-abstractrange-collapsed
    fn get_collapsed(&self, ctx: &Ctx<'js>) -> Result<bool> {
        let state = load(ctx, self.document, self.slot)?;
        Ok(state.start == state.end)
    }
}

/// Collapse `slot` to the extract/delete home point, then run the extract
/// algorithm. Shared by `extractContents` and `surroundContents`, whose later
/// steps overwrite the points again.
impl JsRange {
    fn extract_into_slot(&self, ctx: &Ctx<'_>, fragment: NodeId, state: &RangeState) -> Result<()> {
        // The doctype rejection precedes the range update
        // (<https://dom.spec.whatwg.org/#concept-range-extract>).
        if state.start != state.end {
            let same_text = read_document(ctx, state.start.node.document, |base| {
                Ok(state.start.node == state.end.node
                    && ranges::is_character_data(base, state.start.node))
            })?;
            if !same_text {
                let (_, _, _, _, _, contained, _) = span_parts(ctx, state)?;
                reject_span_doctypes(ctx, &contained, "extracted")?;
            }
        }
        let (new_node, new_offset) = read_document(ctx, state.start.node.document, |base| {
            Ok(collapse_point(base, state))
        })?;
        save(
            ctx,
            self.document,
            self.slot,
            RangeState {
                start: BoundaryPoint {
                    node: new_node,
                    offset: new_offset,
                },
                end: BoundaryPoint {
                    node: new_node,
                    offset: new_offset,
                },
            },
        )?;
        extract_into(ctx, fragment, state)?;
        Ok(())
    }
}
