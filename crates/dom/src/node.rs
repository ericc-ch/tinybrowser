//! Node data kinds and attributes.
//!
//! Name types (`QualName`, `Namespace`, `LocalName`, `Prefix`) are
//! `markup5ever`'s, re-exported here so callers never touch that crate
//! directly. Sharing them means the future `TreeSink` adapter passes parser
//! names straight through with no conversion, and element names are interned
//! rather than copied per node.

use crate::id::NodeId;

pub use markup5ever::{LocalName, Namespace, Prefix, QualName};

/// The HTML namespace URL.
///
/// The interned `markup5ever` atom, so namespace checks do not re-hash the URL.
#[must_use]
pub fn html_namespace() -> Namespace {
    markup5ever::ns!(html)
}

/// The XML namespace URL (`xml:lang` lives here).
#[must_use]
pub fn xml_namespace() -> Namespace {
    markup5ever::ns!(xml)
}

/// The SVG namespace URL.
#[must_use]
pub fn svg_namespace() -> Namespace {
    markup5ever::ns!(svg)
}

/// The `xmlns` declaration namespace URL
/// (<https://www.w3.org/TR/xml-names/#ns-decl>).
#[must_use]
pub fn xmlns_namespace() -> Namespace {
    markup5ever::ns!(xmlns)
}

/// The `xlink` namespace URL, used only by HTML serialization's attribute
/// name rule (<https://html.spec.whatwg.org/multipage/parsing.html#attribute-s-serialized-name>).
#[must_use]
pub fn xlink_namespace() -> Namespace {
    markup5ever::ns!(xlink)
}

/// Whether `name`'s serialization (`prefix:local` or `local`) equals `query`.
#[must_use]
pub fn qualified_name_eq(name: &QualName, query: &str) -> bool {
    match name.prefix.as_ref() {
        Some(prefix) if !prefix.is_empty() => {
            let prefix = prefix.as_ref();
            query.len() == prefix.len() + 1 + name.local.len()
                && query.as_bytes().get(prefix.len()) == Some(&b':')
                && &query[..prefix.len()] == prefix
                && &query[prefix.len() + 1..] == name.local.as_ref()
        }
        _ => name.local.as_ref() == query,
    }
}

/// HTML's get-an-attribute-by-name match: the query as if ASCII-lowercased,
/// then an exact compare to the stored qualified name
/// (<https://dom.spec.whatwg.org/#concept-element-attributes-get-by-name>).
#[must_use]
pub fn html_qualified_name_eq(name: &QualName, query: &str) -> bool {
    match name.prefix.as_ref() {
        Some(prefix) if !prefix.is_empty() => {
            let prefix = prefix.as_ref();
            query.len() == prefix.len() + 1 + name.local.len()
                && query.as_bytes().get(prefix.len()) == Some(&b':')
                && stored_eq_lowercased_query(prefix, &query[..prefix.len()])
                && stored_eq_lowercased_query(name.local.as_ref(), &query[prefix.len() + 1..])
        }
        _ => stored_eq_lowercased_query(name.local.as_ref(), query),
    }
}

fn stored_eq_lowercased_query(stored: &str, query: &str) -> bool {
    stored.len() == query.len()
        && stored
            .bytes()
            .zip(query.bytes())
            .all(|(stored, query)| stored == query.to_ascii_lowercase())
}

/// One attribute: a qualified name and its value.
///
/// Deliberately *not* `markup5ever::Attribute`: that one stores its value as
/// a `StrTendril`, which would leak the tokenizer's buffer type into every
/// consumer of the tree. The adapter converts at the boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attribute {
    pub name: QualName,
    pub value: String,
}

/// What kind of node this is, and the data unique to that kind.
#[derive(Clone, Debug)]
pub enum NodeKind {
    /// The root created with the `Dom`; every document has exactly one.
    Document,
    /// The `<!DOCTYPE html>` declaration.
    Doctype {
        name: String,
        public_id: String,
        system_id: String,
    },
    /// An element such as `<p class="x">`.
    Element {
        name: QualName,
        attributes: Vec<Attribute>,
    },
    /// A document fragment: a container outside the main tree. Serves as the
    /// contents root of `<template>` elements today, and as the context root
    /// for fragment parsing (`innerHTML`) later.
    Fragment,
    /// Character data; adjacent runs are *not* merged by dom itself.
    Text { data: String },
    /// CDATA character data (`<![CDATA[...]]>` in XML).
    CDataSection { data: String },
    /// An XML processing instruction.
    ProcessingInstruction { target: String, data: String },
    /// An HTML comment.
    Comment { data: String },
}

/// One node record: where it sits in the tree, plus its kind-specific data.
///
/// Everything here is arena-internal; outside code sees only [`crate::NodeId`]
/// handles and the read/mutation API on [`crate::Dom`].
#[derive(Debug)]
pub(crate) struct Node {
    pub(crate) parent: Option<NodeId>,
    pub(crate) children: Vec<NodeId>,
    pub(crate) kind: NodeKind,
}
