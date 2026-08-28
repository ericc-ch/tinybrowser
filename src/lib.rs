//! `tinybrowser`: the smallest headless browser for AI agents.
//!
//! Single binary, no sidecar processes; engine stops at DOM + JS.
//! The embeddable surface lives here; CDP will arrive as its own crate.
//!
//! # Seam map
//!
//! ```text
//! main → browser → {dom, net}
//!      (future: cdp → browser)
//! ```
//!
//! See [ADR 0007](../wiki/adrs/0007-engine-charter.md).

pub use browser::{
    Agent, Dom, DomError, NodeId, Page, PageError, PageEvent, Parsed, parse_html,
    parse_html_fragment, parse_html_with_scripting,
};
