//! `tinybrowser`: the smallest headless browser for AI agents.
//!
//! One executable. No separately shipped helper. Engine stops at DOM + JS.
//! The same executable may self-spawn a profile daemon and one renderer
//! process per site instance.
//! The embeddable surface lives here; CDP is a peer crate.
//!
//! # Seam map
//!
//! ```text
//! main → {browser, cdp, webdriver}
//!        cdp → {browser, server}
//!        webdriver → {browser, server}
//!        browser → {net, renderer}
//!        renderer → {dom, render}
//!        render → {dom}
//! ```

pub use browser::{
    AgentBuilder, Browser, BrowserError, BrowserHandle, Profile, ProfileError, ProfileName,
    RemoteValue, ScriptFailure, TabError, TabEvent, TabHandle, TabId,
};
