//! Node identity, classification, and tree walks over Blitz trees.
//!
//! Every function takes a `&BaseDocument` (or `&Parsed`) plus `NodeId`s and
//! returns plain data, so callers in the binding layer, the frame actor, and
//! the render paths share one implementation per concept.

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
            element.name.ns == html_namespace() && element.name.local.as_ref() == local
        }
        _ => false,
    }
}

/// Whether `name` is the HTML `template` tag: the only element with template
/// contents
/// (<https://html.spec.whatwg.org/multipage/scripting.html#the-template-element>).
pub(crate) fn is_template_tag(name: &markup5ever::QualName) -> bool {
    name.ns == html_namespace() && name.local.as_ref() == "template"
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
