//! `tinybrowser`: the smallest headless browser for AI agents.
//!
//! One executable. No separately shipped helper. Engine stops at DOM + JS.
//! The same executable may self-spawn a profile daemon
//! ([ADR 0009](../docs/adrs/0009-named-profile-daemon.md)) and one renderer
//! process per site instance ([ADR 0011](../docs/adrs/0011-renderer-processes-per-site.md)).
//! The embeddable surface lives here; CDP is a peer crate.
//!
//! # Seam map
//!
//! ```text
//! main → {browser, cdp, webdriver, renderer}
//!        cdp → {browser, axum}
//!        webdriver → {browser, axum}
//!        browser → {net, renderer}
//!        renderer → {dom}
//! ```
//!
//! See [ADR 0007](../docs/adrs/0007-engine-charter.md).

pub use browser::{
    AgentBuilder, Browser, BrowserError, BrowserHandle, Profile, ProfileError, ProfileName,
    RemoteValue, RequestId, ScriptFailure, TabError, TabEvent, TabHandle, TabId,
};
