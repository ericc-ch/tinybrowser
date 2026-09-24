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
mod link;
mod manager;
mod network;
mod profile;
mod site;
mod storage;
mod wire;

pub use actor::{ExecuteScriptInOptions, RunUntilJsTrueInOptions, TabEvent, TabHandle, TabId};
pub use browser::{Browser, BrowserError, BrowserHandle, BrowserOpenError, BrowserOptions};
pub use net::{Agent, AgentOptions, CookieRecord, CookieSameSite};
pub use profile::{Profile, ProfileError, ProfileName, default_data_home};
pub use renderer::{
    FrameId, RemoteValue, ResourceLimit, ScreenshotClip, ScreenshotRequest, ScriptFailure, TabError,
};

/// Chrome-compatible identity sent by the browser and exposed through CDP.
/// TLS fingerprinting is a separate transport concern and is not changed here.
pub const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/152.0.0.0 Safari/537.36";

/// Low-entropy `Sec-CH-UA` brands, kept in lockstep with `navigator.userAgentData`
/// (<https://wicg.github.io/ua-client-hints/#sec-ch-ua>).
pub const SEC_CH_UA: &str = r#""Not_A Brand";v="99", "Chromium";v="152", "Google Chrome";v="152""#;

/// Desktop Chrome `Sec-CH-UA-Mobile` boolean
/// (<https://wicg.github.io/ua-client-hints/#sec-ch-ua-mobile>).
pub const SEC_CH_UA_MOBILE: &str = "?0";

/// Linux `Sec-CH-UA-Platform` brand
/// (<https://wicg.github.io/ua-client-hints/#sec-ch-ua-platform>).
pub const SEC_CH_UA_PLATFORM: &str = r#""Linux""#;

/// Dark `Sec-CH-Prefers-Color-Scheme`, kept in lockstep with Stylo and `matchMedia`
/// (<https://wicg.github.io/user-preference-media-features-headers/#sec-ch-prefers-color-scheme>).
pub const SEC_CH_PREFERS_COLOR_SCHEME: &str = "dark";
