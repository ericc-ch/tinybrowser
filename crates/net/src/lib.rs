//! Blocking HTTP client for the page engine.
//!
//! Types in this crate are the public network surface. Behavior follows
//! [Fetch](https://fetch.spec.whatwg.org/).

mod client;
pub mod cors;
mod error;
mod protocol;
mod resolve;
mod transport;

pub use client::{Agent, AgentOptions, Body, Clock, Request, Response};
pub use cookies::{CookieRecord, CookieSameSite, InitiatorKind, site};
pub use error::{LimitExceeded, NetError, ProtocolError, TimeoutKind, TransportError};
pub use protocol::{
    HeaderError, HeaderMap, InvalidMethod, Method, combined_header, referrer_for_policy,
    referrer_policy_from_header,
};
pub use transport::Proxy;
