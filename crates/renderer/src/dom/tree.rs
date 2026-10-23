//! Node identity, classification, and tree walks over Blitz trees.
//!
//! Every function takes a `&BaseDocument` (or `&Parsed`) plus `NodeId`s and
//! returns plain data, so callers in the binding layer, the frame actor, and
//! the render paths share one implementation per concept.

use blitz_dom::NodeData;
use blitz_traits::node_id::NodeId as BlitzNodeId;
/// Blitz tree ids, re-exported for helper signatures across the renderer.
pub(crate) use blitz_traits::node_id::NodeId as BlitzId;

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

/// The parent of `id`, or the host when `id` is a shadow root
/// (<https://dom.spec.whatwg.org/#concept-shadow-including-parent>).
pub(crate) fn shadow_including_parent(
    base: &blitz_dom::BaseDocument,
    id: BlitzNodeId,
) -> Option<BlitzNodeId> {
    let node = base.get_node(id)?;
    if let Some(parent) = node.parent {
        return Some(parent);
    }
    match &node.data {
        blitz_dom::NodeData::ShadowRoot { host, .. } => Some(*host),
        _ => None,
    }
}

/// The shadow-including root of `id`
/// (<https://dom.spec.whatwg.org/#concept-shadow-including-root>).
pub(crate) fn shadow_including_root(
    base: &blitz_dom::BaseDocument,
    id: BlitzNodeId,
) -> BlitzNodeId {
    let mut root = id;
    while let Some(parent) = shadow_including_parent(base, root) {
        root = parent;
    }
    root
}

/// Whether `node` is connected: its shadow-including root is the document
/// (<https://dom.spec.whatwg.org/#connected>).
pub(crate) fn is_connected(base: &blitz_dom::BaseDocument, id: BlitzNodeId) -> bool {
    shadow_including_root(base, id) == base.root_node().id
}

/// Whether `node` is an HTML `iframe` element.
pub(crate) fn is_iframe_element(base: &blitz_dom::BaseDocument, id: BlitzNodeId) -> bool {
    base.get_node(id).is_some_and(|node| {
        let blitz_dom::NodeData::Element(element) = &node.data else {
            return false;
        };
        element.name.ns == html_namespace()
            && (element.name.local == markup5ever::local_name!("iframe")
                || element.name.local == markup5ever::local_name!("frame"))
    })
}

/// Whether `node` is a `<link>` whose `rel` lists the `stylesheet` keyword
/// (<https://html.spec.whatwg.org/multipage/links.html#link-type-stylesheet>).
pub(crate) fn is_stylesheet_link(base: &blitz_dom::BaseDocument, id: BlitzNodeId) -> bool {
    is_html_element(base, id, "link")
        && attr(base, id, "rel").is_some_and(|rel| {
            rel.split_ascii_whitespace()
                .any(|token| token.eq_ignore_ascii_case("stylesheet"))
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

/// The value of the first attribute of `id` matching `match_name`.
fn attr_find(
    base: &blitz_dom::BaseDocument,
    id: BlitzNodeId,
    match_name: impl Fn(&markup5ever::QualName) -> bool,
) -> Option<&str> {
    base.get_node(id)?
        .data
        .downcast_element()?
        .attrs
        .iter()
        .find(|attribute| match_name(&attribute.name))
        .map(|attribute| attribute.value.as_str())
}

/// The value of the attribute named `name` on `id`, when it is an element.
pub(crate) fn attr<'a>(
    base: &'a blitz_dom::BaseDocument,
    id: BlitzNodeId,
    name: &str,
) -> Option<&'a str> {
    let local = markup5ever::LocalName::from(name);
    attr_find(base, id, |qualified| qualified.local == local)
}

/// The value of the first attribute with the given local name in no
/// namespace. HTML attributes and their reflected IDL properties never match
/// a same-named attribute in another namespace
/// (<https://dom.spec.whatwg.org/#concept-element-attributes-get-by-name>).
pub(crate) fn attr_no_namespace<'a>(
    base: &'a blitz_dom::BaseDocument,
    id: BlitzNodeId,
    name: &str,
) -> Option<&'a str> {
    let local = markup5ever::LocalName::from(name);
    attr_find(base, id, |qualified| {
        qualified.local == local && qualified.ns == markup5ever::ns!()
    })
}

/// The value of the first attribute whose qualified name is `qualified`.
///
/// <https://dom.spec.whatwg.org/#concept-element-attributes-get-by-name>
pub(crate) fn attr_by_qualified_name<'a>(
    base: &'a blitz_dom::BaseDocument,
    id: BlitzNodeId,
    qualified: &str,
) -> Option<&'a str> {
    attr_find(base, id, |name| {
        crate::names::qualified_name_eq(name, qualified)
    })
}

/// Whether `local` is an event-handler content attribute name (`on*` with a
/// non-empty lowercase/digit type). HTML parsing lowercases attribute names,
/// so `ONCLICK` arrives as `onclick`; the single spelling here keeps the
/// realm-need walk and handler compilation in agreement.
pub(crate) fn is_handler_attribute_name(local: &str) -> bool {
    local.strip_prefix("on").is_some_and(|typ| {
        !typ.is_empty()
            && typ
                .chars()
                .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
    })
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
            element.name.ns == html_namespace() && element.name.local.as_str() == local
        }
        _ => false,
    }
}

/// Whether `name` is the HTML `template` tag: the only element with template
/// contents
/// (<https://html.spec.whatwg.org/multipage/scripting.html#the-template-element>).
pub(crate) fn is_template_tag(name: &markup5ever::QualName) -> bool {
    name.ns == html_namespace() && name.local.as_str() == "template"
}

/// The template contents fragment of `id`, when it is a `template` element.
pub(crate) fn template_contents(
    base: &blitz_dom::BaseDocument,
    id: BlitzNodeId,
) -> Option<BlitzNodeId> {
    base.get_node(id)?
        .data
        .downcast_element()
        .filter(|element| is_template_tag(&element.name))
        .and_then(|element| element.template_contents)
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
            element.attrs().iter().any(|attribute| {
                attribute.name.local.as_str() == "id"
                    && attribute.name.ns == markup5ever::ns!()
                    && attribute.value == id
            })
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
/// If the attribute is set on a connected element but does not identify a
/// `form`, the owner is null; the ancestor is not consulted.
pub(crate) fn form_owner_of(
    base: &blitz_dom::BaseDocument,
    document: u32,
    node: BlitzId,
) -> Option<NodeId> {
    // Reset-the-form-owner: a present `form` attribute on a connected
    // element is authoritative. A miss (unknown or empty ID) leaves the
    // owner null — it does not fall through to the ancestor, and neither
    // does Chromium nor WPT `form_attribute.html`
    // (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#reset-the-form-owner>).
    if let Some(form_id) = attr(base, node, "form")
        && is_connected(base, node)
    {
        if let Some(found) = first_element_with_id(base, document, form_id)
            && is_html_element(base, found.node, "form")
        {
            return Some(found);
        }
        return None;
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

/// The DOM qualified name of `name`: `prefix:local` when a prefix exists, the
/// local name otherwise
/// (<https://dom.spec.whatwg.org/#concept-element-qualified-name>).
pub(crate) fn qualified_name(name: &markup5ever::QualName) -> String {
    match &name.prefix {
        Some(prefix) if !prefix.is_empty() => format!("{prefix}:{}", name.local),
        _ => name.local.to_string(),
    }
}

/// `Node.nodeName` for an element: the HTML-uppercased qualified name in HTML
/// documents, the qualified name otherwise
/// (<https://dom.spec.whatwg.org/#concept-element-html-uppercased-qualified-name>).
pub(crate) fn element_node_name(name: &markup5ever::QualName, uppercase: bool) -> String {
    let qualified = qualified_name(name);
    if uppercase && name.ns == html_namespace() {
        qualified.to_ascii_uppercase()
    } else {
        qualified
    }
}

/// The previous (`forward == false`) or next sibling of `id` in its parent.
pub(crate) fn sibling(
    base: &blitz_dom::BaseDocument,
    id: BlitzId,
    forward: bool,
) -> Option<BlitzId> {
    let node = base.get_node(id)?;
    let parent = node.parent?;
    let siblings = &base.get_node(parent)?.children;
    let position = siblings.iter().position(|sibling| *sibling == id)?;
    if forward {
        siblings.get(position + 1).copied()
    } else {
        position
            .checked_sub(1)
            .and_then(|index| siblings.get(index).copied())
    }
}

/// Whether `id` is an element node (including fragment backings, which are
/// flagged elements).
pub(crate) fn is_element(base: &blitz_dom::BaseDocument, id: BlitzId) -> bool {
    base.get_node(id)
        .is_some_and(|node| matches!(node.data, NodeData::Element(_)))
}

/// Whether `id` is an element that is not a fragment backing.
pub(crate) fn is_real_element(doc: &crate::documents::BlitzDocument, id: BlitzId) -> bool {
    !doc.is_fragment(id) && is_element(&doc.base, id)
}

/// The root of the tree `id` participates in (itself when detached).
pub(crate) fn root_of(base: &blitz_dom::BaseDocument, id: BlitzId) -> BlitzId {
    let mut root = id;
    while let Some(parent) = base.get_node(root).and_then(|node| node.parent) {
        root = parent;
    }
    root
}

/// `id` followed by its inclusive ancestors, nearest first.
pub(crate) fn ancestor_chain(
    base: &blitz_dom::BaseDocument,
    document: u32,
    id: BlitzId,
) -> Vec<NodeId> {
    let mut chain = vec![NodeId { document, node: id }];
    let mut cursor = id;
    while let Some(parent) = base.get_node(cursor).and_then(|node| node.parent) {
        chain.push(NodeId {
            document,
            node: parent,
        });
        cursor = parent;
    }
    chain
}

/// Document order of two nodes in one tree
/// (<https://dom.spec.whatwg.org/#concept-tree-order>).
pub(crate) fn tree_order(
    base: &blitz_dom::BaseDocument,
    document: u32,
    a: BlitzId,
    b: BlitzId,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let chain_a = ancestor_chain(base, document, a);
    let chain_b = ancestor_chain(base, document, b);
    let mut common = 0;
    while common < chain_a.len()
        && common < chain_b.len()
        && chain_a[chain_a.len() - 1 - common] == chain_b[chain_b.len() - 1 - common]
    {
        common += 1;
    }
    if common == chain_a.len() {
        return chain_a.len().cmp(&chain_b.len());
    }
    if common == chain_b.len() {
        return Ordering::Greater;
    }
    let Some(parent) = chain_a
        .len()
        .checked_sub(common)
        .and_then(|index| chain_a.get(index))
    else {
        return Ordering::Less;
    };
    let child_a = chain_a[chain_a.len() - 1 - common];
    let child_b = chain_b[chain_b.len() - 1 - common];
    let kids: Vec<NodeId> = base
        .get_node(parent.node)
        .map(|node| {
            node.children
                .iter()
                .map(|child| NodeId {
                    document,
                    node: *child,
                })
                .collect()
        })
        .unwrap_or_default();
    let position_a = kids.iter().position(|&kid| kid == child_a);
    let position_b = kids.iter().position(|&kid| kid == child_b);
    position_a.cmp(&position_b)
}
