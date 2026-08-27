//! `tinybrowser`: the smallest headless browser for AI agents.
//!
//! Single binary, no sidecar processes; engine stops at DOM + JS.
//! The embeddable surface lives here; CDP will arrive as its own crate.
//!
//! # Seam map
//!
//! Dependencies go downward only; `browser` is the single fan-in point.
//! See [`wiki/adrs/0001-workspace-crates-with-enforced-edges.md`](../wiki/adrs/0001-workspace-crates-with-enforced-edges.md).
//!
//! ```text
//! main → browser → {dom, net, js}
//!      (future: cdp → browser)
//! ```
