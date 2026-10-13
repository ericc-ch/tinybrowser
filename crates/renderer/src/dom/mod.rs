//! Realm-free DOM algorithms over the parsed trees.
//!
//! Everything in this module and everything below it (`crate::documents`)
//! stays free of the JS engine: no `Ctx`, no `Value`, no `Persistent`, no
//! `JsNode`. Functions here take `&Parsed`/`&mut Parsed`, a
//! `&blitz_dom::BaseDocument`, `NodeId`s, and realm side data that is itself
//! realm-free. The binding layer (`crate::js`) converts values and applies the
//! effects these algorithms return.
//!
//! This mirrors the realm-agnostic DOM layer of Blink and Gecko: the tree does
//! not belong to a realm, and only the JS wrappers do
//! (<https://dom.spec.whatwg.org/#concept-node-adopt>).

pub(crate) mod equality;
pub(crate) mod insertion;
pub(crate) mod serialization;
pub(crate) mod tree;

/// A refused tree operation, mapped to its DOM exception class
/// (<https://dom.spec.whatwg.org/#dom-domerror> naming).
#[derive(Clone, Copy, Debug)]
pub(crate) enum TreeError {
    /// A handle named a node that no longer exists.
    Stale,
    /// The move would place a node inside its own subtree, or the content
    /// model forbids the operation.
    Hierarchy,
    /// The target has no parent to insert beside.
    NotFound,
}

impl std::fmt::Display for TreeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stale => f.write_str("stale node handle"),
            Self::Hierarchy => f.write_str("hierarchy or content model forbids this operation"),
            Self::NotFound => f.write_str("target has no parent to insert beside"),
        }
    }
}
