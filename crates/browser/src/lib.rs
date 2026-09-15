//! Browser-side crate: `Browser`, the tab registry, `NetworkSession`, and the
//! value-only protocol surface. The tab engine lives in the `renderer` crate.
//!
//! The `cdp` and `webdriver` crates depend on this crate. They do not depend
//! on each other, `dom`, `net`, or `renderer`. Browser owns
//! [`NetworkSession`].
//! Browser and tab state live in bounded Tokio owner tasks.

mod actor;
mod browser;
mod link;
mod manager;
mod network;
mod profile;
mod site;
mod store;

pub use actor::{TabHandle, TabId};
pub use browser::{Browser, BrowserError, BrowserHandle};
pub use net::{Agent, AgentBuilder};
pub use network::NetworkSession;
pub use profile::{Profile, ProfileError, ProfileName};
pub use renderer::{RemoteValue, ResourceLimit, ScriptFailure, TabError, TabEvent};
pub use store::ProfileStore;
