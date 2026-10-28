//! Browser-side crate: `Browser`, the tab registry, and both
//! ends of the renderer wire. The page engine itself lives in the `renderer`
//! crate.
//!
//! The renderer runs either as a child process — this crate spawns it and
//! [`child::serve`] is its entry point — or inside a WebAssembly component,
//! which uses the engine without any of this.
//!
//! The `cdp` and `webdriver` crates depend on this crate. They do not depend
//! on each other, `dom`, `net`, or `renderer`.
//! Browser and tab state live in bounded Tokio owner tasks.

mod actor;
mod assignment;
mod broadcast;
mod browser;
pub mod child;
mod context;
mod exchange;
mod history;
mod link;
mod manager;
mod network;
mod profile;
mod site;
mod storage;
mod wire;

pub use actor::{ExecuteScriptOptions, RunUntilJsTrueOptions, TabEvent, TabHandle, TabId};
pub use browser::{Browser, BrowserError, BrowserHandle, BrowserOpenError, BrowserOptions};
pub use history::{EntryId, HistoryEntry, HistorySnapshot};
pub use net::{
    Agent, AgentOptions, CookieRecord, CookieSameSite, Proxy, ResolveRule, TrustedCa, UserAgent,
};
pub use profile::{Profile, ProfileError, ProfileName, default_data_home};
pub use renderer::{
    DEFAULT_VIEWPORT, FrameId, RemoteValue, ResourceLimit, ScreenshotClip, ScreenshotRequest,
    ScriptFailure, ScriptSource, TabError,
};

/// Dark `Sec-CH-Prefers-Color-Scheme`, kept in lockstep with Stylo and `matchMedia`
/// (<https://wicg.github.io/user-preference-media-features-headers/#sec-ch-prefers-color-scheme>).
pub const SEC_CH_PREFERS_COLOR_SCHEME: &str = "dark";
