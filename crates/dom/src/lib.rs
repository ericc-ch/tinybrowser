//! Document storage for tinybrowser: a generational-arena DOM tree.
//!
//! Dependencies by charter: pinned `markup5ever` name types plus the Servo
//! selector stack, never a parsing dependency. This crate is representation
//! plus mutation commands; the html5ever adapter lives above it, and nothing
//! here knows how bytes become nodes.
//!
//! Everything crosses boundaries as [`NodeId`] handles. A handle outliving
//! its node is harmless (lookups report absence, never a different node),
//! which lets the `QuickJS` binding layer hold handles across GC cycles
//! without borrowing anything.
//!
//! # Seam map
//!
//! ```text
//! browser:  html5ever TreeSink → Dom mutations; QuickJS ↔ NodeId
//! dom:      slots, generations, children lists
//! ```

mod arena;
mod id;
mod node;
mod select;
mod state;

pub use state::{
    attr_value, direction_is, is_checked, is_default, is_defined, is_disabled, is_enabled, is_html,
    is_hyperlink, is_indeterminate, is_optional, is_placeholder_shown, is_read_only, is_read_write,
    is_required, lang_matches, local_is,
};

pub use arena::{Children, Dom, DomError, Lifecycle, Mutation, QuirksMode};
pub use id::NodeId;
pub use node::{
    Attribute, LocalName, Namespace, NodeKind, Prefix, QualName, html_namespace,
    html_qualified_name_eq, qualified_name_eq, svg_namespace, xlink_namespace, xml_namespace,
    xmlns_namespace,
};
pub use select::{ParseFail, ParseFailKind, SelectError};
