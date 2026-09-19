//! Browser-side crate: `Browser`, the tab registry, `NetworkSession`, and both
//! ends of the renderer wire. The page engine itself lives in the `renderer`
//! crate.
//!
//! The renderer runs either as a child process — this crate spawns it and
//! [`child::serve`] is its entry point — or inside a WebAssembly component,
//! which uses the engine without any of this.
//!
//! The `cdp` and `webdriver` crates depend on this crate. They do not depend
//! on each other, `dom`, `net`, or `renderer`. Browser owns
//! [`NetworkSession`].
//! Browser and tab state live in bounded Tokio owner tasks.

mod actor;
mod broadcast;
mod browser;
pub mod child;
mod link;
mod manager;
mod network;
mod profile;
mod site;
mod storage;
mod store;
mod wire;

pub use actor::{TabHandle, TabId};
pub use browser::{Browser, BrowserError, BrowserHandle};
pub use net::{Agent, AgentBuilder, CookieRecord, CookieSameSite};
pub use network::NetworkSession;
pub use profile::{Profile, ProfileError, ProfileName};
pub use renderer::{
    FrameId, RemoteValue, ResourceLimit, ScreenshotClip, ScreenshotRequest, ScriptFailure,
    TabError, TabEvent,
};
pub use store::ProfileStore;
