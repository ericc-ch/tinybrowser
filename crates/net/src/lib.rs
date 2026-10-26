//! Blocking HTTP client for the page engine.
//!
//! Types in this crate are the public network surface. Behavior follows
//! [Fetch](https://fetch.spec.whatwg.org/).

mod client;
pub mod cors;
mod error;
mod protocol;
pub mod referrer;
mod resolve;
mod transport;

pub use client::{Agent, AgentOptions, Body, Clock, Request, Response, UserAgent};
pub use cookies::{CookieRecord, CookieSameSite, InitiatorKind, site};
pub use error::{LimitExceeded, NetError, ProtocolError, TimeoutKind, TransportError};
pub use protocol::{HeaderError, HeaderMap, InvalidMethod, Method, combined_header};
pub use referrer::{
    Referrer, ReferrerClient, ReferrerPolicy, determine_referrer, is_potentially_trustworthy,
};
pub use resolve::ResolveRule;
pub use transport::{Proxy, TrustedCa};
