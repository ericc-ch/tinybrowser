//! Document storage for tinybrowser: a generational-arena DOM tree.
//!
//! Dependencies by charter ([ADR 0002](../../docs/adr/0002-dom-layer-architecture.md)):
//! pinned `markup5ever` name types plus the Servo selector stack — never a
//! parsing dependency. This crate is representation
//! plus mutation commands; the html5ever adapter lives above it, and nothing
//! here knows how bytes become nodes.
//!
//! Everything crosses boundaries as [`NodeId`] handles. A handle outliving
//! its node is harmless — lookups report absence, never a different node —
//! which is what will let the `QuickJS` binding layer hold handles across GC
//! cycles without borrowing anything.
//!
//! # Seam map
//!
//! ```text
//! browser (or glue crate):  html5ever TreeSink → Dom mutations
//! js:                       QuickJS wrappers ↔ NodeId handles
//! dom:                      slots, generations, children lists
//! ```

mod arena;
mod id;
mod node;
mod select;
mod state;

pub use arena::{Dom, DomError, NodeRef};
pub use id::NodeId;
pub use node::{Attribute, LocalName, Namespace, NodeKind, Prefix, QualName, html_namespace};
pub use select::{ParseFail, ParseFailKind, QuirksMode, SelectError};
