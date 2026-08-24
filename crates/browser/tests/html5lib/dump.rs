//! Renders a [`Dom`] in the html5lib tree-dump format so parsed output can be
//! compared byte-for-byte against the suite's `#document` sections.
//!
//! Format rules (upstream `tree-construction/README.md`): each node is one
//! line — `| ` plus two spaces per ancestor above the document's children;
//! element attributes sort by local name and render as if they were first
//! children; SVG/MathML elements and XLink/XML/XMLNS attributes carry a
//! namespace designator prefix; template contents appear under a `content`
//! pseudo-node. The reference serializer is html5ever's rcdom test driver.

use std::collections::HashMap;
use std::sync::LazyLock;

use dom::{Dom, NodeId, NodeKind};

const SVG_NS_URL: &str = "http://www.w3.org/2000/svg";
const MATHML_NS_URL: &str = "http://www.w3.org/1998/Math/MathML";
const XLINK_NS_URL: &str = "http://www.w3.org/1999/xlink";
const XML_NS_URL: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS_NS_URL: &str = "http://www.w3.org/2000/xmlns/";

/// The suite's namespace designators, keyed by full namespace URL. Built
/// through [`dom::Namespace`]'s public constructor so no markup5ever macro
/// machinery leaks into tests.
static SVG_NS: LazyLock<dom::Namespace> = LazyLock::new(|| dom::Namespace::from(SVG_NS_URL));
static MATHML_NS: LazyLock<dom::Namespace> = LazyLock::new(|| dom::Namespace::from(MATHML_NS_URL));
static XLINK_NS: LazyLock<dom::Namespace> = LazyLock::new(|| dom::Namespace::from(XLINK_NS_URL));
static XML_NS: LazyLock<dom::Namespace> = LazyLock::new(|| dom::Namespace::from(XML_NS_URL));
static XMLNS_NS: LazyLock<dom::Namespace> = LazyLock::new(|| dom::Namespace::from(XMLNS_NS_URL));

/// Dumps the children of `dom`'s document — the shape the suite expects for
/// full-document parses. Template contents come from the parse result's side
/// map, since they live outside every child list.
pub fn dump_document(dom: &Dom, templates: &HashMap<NodeId, NodeId>) -> String {
    let mut out = String::new();
    let document = dom.document();
    let top_level = dom
        .children(document)
        .map(Iterator::collect::<Vec<_>>)
        .unwrap_or_default();
    for child in top_level {
        serialize(dom, templates, *child, 1, &mut out);
    }
    out.truncate(out.trim_end_matches('\n').len());
    out
}

fn serialize(
    dom: &Dom,
    templates: &HashMap<NodeId, NodeId>,
    id: NodeId,
    indent: usize,
    out: &mut String,
) {
    out.push('|');
    out.extend(std::iter::repeat_n(' ', indent));

    // Clone the kind data up front: the borrow must end before recursion.
    let Some(node) = dom.get(id) else {
        unreachable!("dump walks only live nodes of its own tree");
    };
    let kind = node.kind().clone();

    match kind {
        NodeKind::Document => unreachable!("the walk root is the document's children"),
        NodeKind::Fragment => unreachable!("fragments surface only via template contents"),
        NodeKind::Doctype {
            name,
            public_id,
            system_id,
        } => {
            out.push_str("<!DOCTYPE ");
            out.push_str(&name);
            if !public_id.is_empty() || !system_id.is_empty() {
                out.push_str(" \"");
                out.push_str(&public_id);
                out.push_str("\" \"");
                out.push_str(&system_id);
                out.push('"');
            }
            out.push_str(">\n");
        }
        NodeKind::Text { data } => {
            out.push('"');
            out.push_str(&data);
            out.push_str("\"\n");
        }
        NodeKind::Comment { data } => {
            out.push_str("<!-- ");
            out.push_str(&data);
            out.push_str(" -->\n");
        }
        NodeKind::Element { name, attributes } => {
            out.push('<');
            if name.ns == *SVG_NS {
                out.push_str("svg ");
            } else if name.ns == *MATHML_NS {
                out.push_str("math ");
            }
            out.push_str(&name.local);
            out.push_str(">\n");

            // Upstream sorts attribute *local* names (a noted FIXME says
            // UTF-16 order); namespaced locals are distinct strings, so plain
            // byte order matches every case in the suite.
            let mut sorted = attributes;
            sorted.sort_by(|a, b| a.name.local.cmp(&b.name.local));
            for attr in sorted {
                out.push('|');
                out.extend(std::iter::repeat_n(' ', indent + 2));
                if attr.name.ns == *XLINK_NS {
                    out.push_str("xlink ");
                } else if attr.name.ns == *XML_NS {
                    out.push_str("xml ");
                } else if attr.name.ns == *XMLNS_NS {
                    out.push_str("xmlns ");
                }
                out.push_str(&attr.name.local);
                out.push_str("=\"");
                out.push_str(&attr.value);
                out.push_str("\"\n");
            }
        }
    }

    let kids = dom
        .children(id)
        .map(Iterator::collect::<Vec<_>>)
        .unwrap_or_default();
    for child in kids {
        serialize(dom, templates, *child, indent + 2, out);
    }

    if let Some(contents) = templates.get(&id).copied() {
        out.push('|');
        out.extend(std::iter::repeat_n(' ', indent + 2));
        out.push_str("content\n");
        let inner = dom
            .children(contents)
            .map(Iterator::collect::<Vec<_>>)
            .unwrap_or_default();
        for child in inner {
            serialize(dom, templates, *child, indent + 4, out);
        }
    }
}
