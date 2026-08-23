//! Node data kinds and attributes.
//!
//! Name types (`QualName`, `Namespace`, `LocalName`, `Prefix`) are
//! `markup5ever`'s — re-exported here so callers never touch that crate
//! directly. Sharing them means the future `TreeSink` adapter passes parser
//! names straight through with no conversion, and element names are interned
//! rather than copied per node.

use crate::children::Children;
use crate::id::NodeId;

pub use markup5ever::{LocalName, Namespace, Prefix, QualName};

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
    pub(crate) children: Children,
    pub(crate) kind: NodeKind,
}
