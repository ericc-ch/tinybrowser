//! Browser-side crate: `Browser`, the tab (tab) registry, `NetworkSession`, and the
//! value-only protocol surface. The tab engine lives in the `renderer` crate
//! ([ADR 0011](../../../docs/adrs/0011-renderer-processes-per-site.md)).
//!
//! The `cdp` and `webdriver` crates depend on this crate. They do not depend
//! on each other, `dom`, `net`, or `renderer`. Browser owns
//! [`NetworkSession`] ([ADR 0010](../../../docs/adrs/0010-tab-actor-ownership.md)).
//! Blocking sends run on the browser-owned bounded network executor; renderer
//! dials arrive through the browser seam.

mod actor;
mod browser;
mod link;
mod network;
mod profile;
mod services;
mod site;

pub use actor::{RequestId, TabHandle, TabId};
pub use browser::{Browser, BrowserError, BrowserHandle};
pub use link::Renderers;
pub use net::{Agent, AgentBuilder};
pub use network::{FetchHandle, NetworkSession, ProfileStore};
pub use profile::{Profile, ProfileError, ProfileName};
pub use renderer::{RemoteValue, ScriptFailure, TabError, TabEvent};
