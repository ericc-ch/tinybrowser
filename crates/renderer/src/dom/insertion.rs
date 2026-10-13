//! Tree mutation: pre-insert classification, insert, replace, and move.
//!
//! These algorithms work on `&mut Parsed` and `NodeId`s only, and they return
//! the realm-side work they cannot do themselves (child navigables to destroy,
//! insertion steps to apply) to the caller that owns the realm
//! (<https://dom.spec.whatwg.org/#concept-node-insert>,
//! <https://dom.spec.whatwg.org/#concept-node-replace>).

use blitz_dom::NodeData;

use crate::documents::JournalEntry;
use crate::dom::tree::{
    BlitzId, NodeId, attr, html_namespace, is_html_element, is_iframe_element, mathml_namespace,
    sibling, svg_namespace,
};

/// The inserted node's side of ensure pre-insert validity.
pub(crate) enum InsertedNode {
    Fragment {
        elements: usize,
        has_text: bool,
    },
    Element,
    /// A `Text` node, including a CDATA section.
    Text,
    Doctype,
    /// A comment or processing instruction.
    CharacterData,
    Other,
}

/// The inserted node's side of ensure pre-insert validity: fragments
/// report their shape, elements/text/comments report their kind, anything
/// else cannot be inserted
/// (<https://dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity>).
pub(crate) fn classify_inserted(
    doc: &crate::documents::BlitzDocument,
    id: BlitzId,
) -> InsertedNode {
    // Fragment backings (flagged elements) and real `Fragment` nodes both
    // splice their children on insertion.
    if doc.is_fragment(id) {
        // Flatten nested fragments: insertion splices fragment children, so a
        // fragment holding a nested fragment with two elements still inserts
        // two elements. Iterative: fragment nesting is not depth-limited.
        // (<https://dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity>).
        let mut elements = 0;
        let mut has_text = false;
        let mut stack = vec![id];
        while let Some(current) = stack.pop() {
            let kids: Vec<BlitzId> = doc
                .base
                .get_node(current)
                .map(|n| n.children.iter().copied().collect())
                .unwrap_or_default();
            for child in kids {
                if doc.is_fragment(child) {
                    stack.push(child);
                    continue;
                }
                if doc
                    .base
                    .get_node(child)
                    .is_some_and(|candidate| candidate.data.downcast_element().is_some())
                {
                    elements += 1;
                }
                if doc.base.get_node(child).is_some_and(|candidate| {
                    matches!(
                        candidate.data,
                        NodeData::Text(_) | NodeData::CDataSection { .. }
                    )
                }) {
                    has_text = true;
                }
            }
        }
        return InsertedNode::Fragment { elements, has_text };
    }
    match doc.base.get_node(id).map(|node| &node.data) {
        Some(NodeData::Element(_)) => InsertedNode::Element,
        Some(NodeData::Text(_) | NodeData::CDataSection { .. }) => InsertedNode::Text,
        Some(NodeData::Doctype { .. }) => InsertedNode::Doctype,
        Some(NodeData::Comment { .. } | NodeData::ProcessingInstruction { .. }) => {
            InsertedNode::CharacterData
        }
        _ => InsertedNode::Other,
    }
}

/// Whether `id` is a doctype node.
pub(crate) fn is_doctype(doc: &crate::documents::BlitzDocument, id: BlitzId) -> bool {
    doc.base
        .get_node(id)
        .is_some_and(|node| matches!(node.data, NodeData::Doctype { .. }))
}

/// Whether `parent` has a doctype child other than `inserted` and the
/// `excluded` children.
pub(crate) fn parent_has_doctype(
    doc: &crate::documents::BlitzDocument,
    parent: BlitzId,
    excluded: &[BlitzId],
    same_document: bool,
    inserted: BlitzId,
) -> bool {
    doc.base.get_node(parent).is_some_and(|root| {
        root.children.iter().any(|child| {
            !(excluded.contains(child) || same_document && *child == inserted)
                && is_doctype(doc, *child)
        })
    })
}

/// Whether a doctype follows `child` among `parent`'s children.
pub(crate) fn sibling_doctype_follows(
    doc: &crate::documents::BlitzDocument,
    parent: BlitzId,
    child: BlitzId,
) -> bool {
    let Some(node) = doc.base.get_node(parent) else {
        return false;
    };
    let Some(position) = node.children.iter().position(|kid| *kid == child) else {
        return false;
    };
    node.children
        .iter()
        .skip(position + 1)
        .any(|kid| is_doctype(doc, *kid))
}

/// Whether an element precedes `child` among `parent`'s children.
pub(crate) fn sibling_element_precedes(
    doc: &crate::documents::BlitzDocument,
    parent: BlitzId,
    child: BlitzId,
) -> bool {
    let Some(node) = doc.base.get_node(parent) else {
        return false;
    };
    let Some(position) = node.children.iter().position(|kid| *kid == child) else {
        return false;
    };
    node.children.iter().take(position).any(|kid| {
        doc.base
            .get_node(*kid)
            .is_some_and(|candidate| candidate.data.downcast_element().is_some())
            && !doc.is_fragment(*kid)
    })
}

/// Whether `parent` has an element child other than `inserted` and the
/// `excluded` children.
pub(crate) fn parent_has_element(
    doc: &crate::documents::BlitzDocument,
    parent: BlitzId,
    excluded: &[BlitzId],
    same_document: bool,
    inserted: BlitzId,
) -> bool {
    doc.base.get_node(parent).is_some_and(|root| {
        root.children.iter().any(|child| {
            !(excluded.contains(child) || same_document && *child == inserted)
                && doc
                    .base
                    .get_node(*child)
                    .is_some_and(|candidate| candidate.data.downcast_element().is_some())
                && !doc.is_fragment(*child)
        })
    })
}

/// Siblings around `child` within `parent`, for the mutation journal.
pub(crate) fn siblings_around(
    base: &blitz_dom::BaseDocument,
    document: u32,
    parent: BlitzId,
    child: BlitzId,
) -> (Option<NodeId>, Option<NodeId>) {
    let mut previous = None;
    let mut next = None;
    if let Some(node) = base.get_node(parent) {
        let mut seen = false;
        for &kid in &node.children {
            if kid == child {
                seen = true;
                continue;
            }
            let id = NodeId {
                document,
                node: kid,
            };
            if seen && next.is_none() {
                next = Some(id);
                break;
            }
            if !seen {
                previous = Some(id);
            }
        }
    }
    (previous, next)
}

/// Records the removal of an already-parented `target`, then detaches it. A
/// node with no parent is already detached; that is a no-op. The removal
/// keeps its own `childList` record, so a move reads as removed plus added
/// (<https://dom.spec.whatwg.org/#concept-node-remove>).
pub(crate) fn iframe_containers_in_subtrees(
    parsed: &crate::Parsed,
    roots: &[NodeId],
) -> Vec<NodeId> {
    let mut containers = Vec::new();
    for root in roots {
        let mut stack = vec![root.node];
        while let Some(id) = stack.pop() {
            if is_iframe_element(&parsed.document.base, id) {
                containers.push(NodeId {
                    document: root.document,
                    node: id,
                });
            }
            if let Some(node) = parsed.document.base.get_node(id) {
                stack.extend(node.children.iter().rev().copied());
            }
        }
    }
    containers
}

pub(crate) fn unlink_journaled(parsed: &mut crate::Parsed, target: NodeId) {
    let document = target.document;
    let (parent, previous, next) = {
        let base = &parsed.document.base;
        let parent = base.get_node(target.node).and_then(|node| node.parent);
        let (previous, next) = parent.map_or((None, None), |parent| {
            siblings_around(base, document, parent, target.node)
        });
        (parent, previous, next)
    };
    let Some(parent) = parent else {
        return;
    };
    parsed.document.base.mutate().remove_node(target.node);
    parsed.document.record(JournalEntry::ChildList {
        target: NodeId {
            document,
            node: parent,
        },
        added: Vec::new(),
        removed: vec![target],
        previous,
        next,
    });
}

/// Inserts the already-adopted `child` into `parent` before `reference` (or
/// at the end for `None`), recording one `childList` addition. Blitz detaches
/// from the old parent during the insert itself, so callers run
/// `unlink_journaled` first and the removal keeps its own record.
pub(crate) fn place_journaled(
    parsed: &mut crate::Parsed,
    parent: NodeId,
    child: NodeId,
    reference: Option<NodeId>,
) {
    let previous = match reference {
        Some(reference) => {
            siblings_around(
                &parsed.document.base,
                parent.document,
                parent.node,
                reference.node,
            )
            .0
        }
        None => parsed
            .document
            .base
            .get_node(parent.node)
            .and_then(|node| node.children.last().copied())
            .map(|node| NodeId {
                document: parent.document,
                node,
            }),
    };
    match reference {
        Some(reference) => parsed
            .document
            .base
            .mutate()
            .insert_nodes_before(reference.node, &[child.node]),
        None => parsed
            .document
            .base
            .mutate()
            .append_children(parent.node, &[child.node]),
    }
    parsed.document.record(JournalEntry::ChildList {
        target: parent,
        added: vec![child],
        removed: Vec::new(),
        previous,
        next: reference,
    });
}

/// Moves one already-adopted node into `parent`, expanding a fragment into
/// its children. Inserting beside itself stays put
/// (<https://dom.spec.whatwg.org/#concept-node-pre-insert>).
pub(crate) fn insert_parsed(
    parsed: &mut crate::Parsed,
    parent: NodeId,
    node: NodeId,
    mut reference: Option<NodeId>,
) {
    if parsed.document.is_fragment(node.node) {
        splice_parsed(parsed, parent, node, reference);
        return;
    }
    if reference == Some(node) {
        reference = sibling(&parsed.document.base, node.node, true).map(|next| NodeId {
            document: node.document,
            node: next,
        });
    }
    unlink_journaled(parsed, node);
    place_journaled(parsed, parent, node, reference);
}

/// Splices an adopted fragment's children into `parent` before `reference`:
/// one removal record on the fragment plus one addition record on the parent
/// (<https://dom.spec.whatwg.org/#concept-node-insert>).
pub(crate) fn splice_parsed(
    parsed: &mut crate::Parsed,
    parent: NodeId,
    fragment: NodeId,
    reference: Option<NodeId>,
) {
    let moved: Vec<NodeId> = parsed
        .document
        .base
        .get_node(fragment.node)
        .map(|node| {
            node.children
                .iter()
                .map(|child| NodeId {
                    document: fragment.document,
                    node: *child,
                })
                .collect()
        })
        .unwrap_or_default();
    if moved.is_empty() {
        return;
    }
    parsed.document.record(JournalEntry::ChildList {
        target: fragment,
        added: Vec::new(),
        removed: moved.clone(),
        previous: None,
        next: None,
    });
    let previous = match reference {
        Some(reference) => {
            siblings_around(
                &parsed.document.base,
                parent.document,
                parent.node,
                reference.node,
            )
            .0
        }
        None => parsed
            .document
            .base
            .get_node(parent.node)
            .and_then(|node| node.children.last().copied())
            .map(|node| NodeId {
                document: parent.document,
                node,
            }),
    };
    for child in &moved {
        match reference {
            Some(reference) => parsed
                .document
                .base
                .mutate()
                .insert_nodes_before(reference.node, &[child.node]),
            None => parsed
                .document
                .base
                .mutate()
                .append_children(parent.node, &[child.node]),
        }
    }
    parsed.document.record(JournalEntry::ChildList {
        target: parent,
        added: moved,
        removed: Vec::new(),
        previous,
        next: reference,
    });
}

/// Replace all with `node` within `parent`
/// (<https://dom.spec.whatwg.org/#concept-node-replace-all>).
///
/// Children of `parent` are removed with observers suppressed. Inserting a
/// fragment then records one empty childList on the fragment (the insert
/// algorithm always queues that record). The parent gets one combined
/// childList for the swap.
pub(crate) fn replace_all_with_node(
    parsed: &mut crate::Parsed,
    parent: NodeId,
    node: NodeId,
) -> Vec<NodeId> {
    let added = if parsed.document.is_fragment(node.node) {
        let moved: Vec<NodeId> = parsed
            .document
            .base
            .get_node(node.node)
            .map(|backing| {
                backing
                    .children
                    .iter()
                    .map(|child| NodeId {
                        document: node.document,
                        node: *child,
                    })
                    .collect()
            })
            .unwrap_or_default();
        for child in &moved {
            parsed.document.base.mutate().remove_node(child.node);
        }
        parsed.document.record(JournalEntry::ChildList {
            target: node,
            added: Vec::new(),
            removed: moved.clone(),
            previous: None,
            next: None,
        });
        moved
    } else {
        vec![node]
    };
    replace_all_journaled(parsed, parent, added)
}

/// Replaces every child of `parent` with `added`: standing children detach
/// silently and one record carries the swap. Replacing with the same
/// contents is silent
/// (<https://dom.spec.whatwg.org/#concept-node-replace-all>).
pub(crate) fn replace_all_journaled(
    parsed: &mut crate::Parsed,
    parent: NodeId,
    added: Vec<NodeId>,
) -> Vec<NodeId> {
    let removed: Vec<NodeId> = parsed
        .document
        .base
        .get_node(parent.node)
        .map(|node| {
            node.children
                .iter()
                .map(|child| NodeId {
                    document: parent.document,
                    node: *child,
                })
                .collect()
        })
        .unwrap_or_default();
    let iframes = iframe_containers_in_subtrees(parsed, &removed);
    for kid in &removed {
        parsed.document.base.mutate().remove_node(kid.node);
    }
    for child in &added {
        // Insert adopts: a node with a parent is removed first, and that
        // removal is not suppressed
        // (<https://dom.spec.whatwg.org/#concept-node-adopt>).
        unlink_journaled(parsed, *child);
        parsed
            .document
            .base
            .mutate()
            .append_children(parent.node, &[child.node]);
    }
    if !added.is_empty() || !removed.is_empty() {
        parsed.document.record(JournalEntry::ChildList {
            target: parent,
            added,
            removed,
            previous: None,
            next: None,
        });
    }
    iframes
}

/// The HTML, SVG, and `MathML` elements under `id` whose `nonce` content
/// attribute clears on browsing-context connection.
pub(crate) fn collect_nonce_elements(
    base: &blitz_dom::BaseDocument,
    id: NodeId,
    out: &mut Vec<NodeId>,
) {
    if let Some(element) = base
        .get_node(id.node)
        .and_then(|node| node.data.downcast_element())
    {
        let html = element.name.ns == html_namespace();
        let svg = element.name.ns == svg_namespace();
        let mathml = element.name.ns == mathml_namespace();
        if (html || svg || mathml)
            && attr(base, id.node, "nonce").is_some_and(|value| !value.is_empty())
        {
            out.push(id);
        }
    }
    let children: Vec<BlitzId> = base
        .get_node(id.node)
        .map(|node| node.children.iter().copied().collect())
        .unwrap_or_default();
    for child in children {
        collect_nonce_elements(
            base,
            NodeId {
                document: id.document,
                node: child,
            },
            out,
        );
    }
}

pub(crate) enum InsertionStep {
    Iframe(NodeId),
    Script(NodeId),
    MetaReferrer(NodeId),
}

/// Initial about:blank iframes fire `load` from insertion steps. An iframe
/// with `src` or `srcdoc` navigates instead, and that navigation fires `load`
/// (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#the-iframe-element:html-element-insertion-steps>).
pub(crate) fn iframe_fires_initial_load(base: &blitz_dom::BaseDocument, node: BlitzId) -> bool {
    attr(base, node, "src").is_none() && attr(base, node, "srcdoc").is_none()
}

/// The insertion steps an inserted subtree owes, in tree order: iframe
/// contexts, classic scripts, and referrer-policy updates.
pub(crate) fn collect_html_insertion_steps(
    base: &blitz_dom::BaseDocument,
    id: NodeId,
    out: &mut Vec<InsertionStep>,
) {
    if is_html_element(base, id.node, "script") {
        out.push(InsertionStep::Script(id));
    } else if is_iframe_element(base, id.node) {
        out.push(InsertionStep::Iframe(id));
    } else if is_html_element(base, id.node, "meta") {
        out.push(InsertionStep::MetaReferrer(id));
    }
    let children: Vec<BlitzId> = base
        .get_node(id.node)
        .map(|node| node.children.iter().copied().collect())
        .unwrap_or_default();
    for child in children {
        collect_html_insertion_steps(
            base,
            NodeId {
                document: id.document,
                node: child,
            },
            out,
        );
    }
}
