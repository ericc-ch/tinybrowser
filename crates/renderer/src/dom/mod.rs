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
