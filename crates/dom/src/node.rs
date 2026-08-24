//! Node data kinds and attributes.
//!
//! Name types (`QualName`, `Namespace`, `LocalName`, `Prefix`) are
//! `markup5ever`'s — re-exported here so callers never touch that crate
//! directly. Sharing them means the future `TreeSink` adapter passes parser
//! names straight through with no conversion, and element names are interned
//! rather than copied per node.

use crate::id::NodeId;

pub use markup5ever::{LocalName, Namespace, Prefix, QualName};

/// The HTML namespace URL.
///
/// `markup5ever`'s `ns!(html)` wraps this same string; exposed as a plain
/// function so callers never touch macro machinery. Selector matching and
/// the future `TreeSink` adapter both key off it.
#[must_use]
pub fn html_namespace() -> Namespace {
    Namespace::from("http://www.w3.org/1999/xhtml")
}

/// One attribute: a qualified name and its value.
///
/// Deliberately *not* `markup5ever::Attribute` — that one stores its value as
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
