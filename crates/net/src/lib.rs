//! Blocking HTTP client for the page engine.
//!
//! Types in this crate are the public network surface. Behavior follows
//! [Fetch](https://fetch.spec.whatwg.org/).

mod client;
pub mod cors;
mod error;
mod identity;
mod protocol;
pub mod referrer;
mod resolve;
mod transport;

pub use client::{Agent, AgentOptions, Body, Clock, Request, Response};
pub use cookies::{CookieRecord, CookieSameSite, InitiatorKind, site};
pub use error::{NetError, TimeoutKind, TransportError};
pub use identity::{Brand, ClientHints, DEFAULT_USER_AGENT, Identity, UserAgent};
pub use protocol::{
    HeaderError, HeaderMap, InvalidMethod, Method, combined_header, combined_header_in,
    get_decode_split, get_decode_split_decoded, isomorphic_decode, isomorphic_encode,
    split_header_value,
};
pub use referrer::{Referrer, ReferrerClient, ReferrerPolicy, determine_referrer};
pub use resolve::{InvalidResolveRule, ResolveRule};
pub use transport::{InvalidCa, InvalidProxy, Proxy, TrustedCa};
