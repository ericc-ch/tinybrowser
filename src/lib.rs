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
