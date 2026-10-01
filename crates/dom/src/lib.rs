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
//! browser:  html5ever TreeSink → Document mutations; QuickJS ↔ NodeId
//! dom:      slots, generations, children lists; form/: the form control model
//! ```

mod arena;
pub mod form;
mod id;
pub mod lifecycle;
pub mod metadata;
pub mod mutation;
pub mod named;
mod node;
pub mod selector;
pub mod shadow;
mod state;
mod string;
mod value;

pub use state::{
    attr_value, direction_is, is_checked, is_default, is_defined, is_disabled, is_enabled, is_html,
    is_hyperlink, is_indeterminate, is_optional, is_placeholder_shown, is_read_only, is_read_write,
    is_required, lang_matches, local_is,
};

pub use arena::{Children, Document, DomError, Tree};
pub use id::NodeId;
pub use lifecycle::Lifecycle;
pub use metadata::QuirksMode;
pub use mutation::Mutation;
pub use node::{
    Attribute, LocalName, Namespace, NodeKind, Prefix, QualName, html_namespace,
    html_qualified_name_eq, mathml_namespace, qualified_name_eq, svg_namespace, xlink_namespace,
    xml_namespace, xmlns_namespace,
};
pub use selector::{ParseFail, ParseFailKind, SelectError};
pub use string::DomString;
pub use value::{
    is_valid_date, is_valid_floating_point, is_valid_local_date_time, is_valid_month,
    is_valid_simple_color, is_valid_time, is_valid_week,
};
