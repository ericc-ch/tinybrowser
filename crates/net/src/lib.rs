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
mod secure_context;
mod transport;

pub use client::{Agent, AgentOptions, Body, Clock, Request, Response, UserAgent};
pub use cookies::{CookieRecord, CookieSameSite, InitiatorKind, site};
pub use error::{LimitExceeded, NetError, ProtocolError, TimeoutKind, TransportError};
pub use protocol::{
    HeaderError, HeaderMap, InvalidMethod, Method, combined_header, combined_header_in,
    get_decode_split, get_decode_split_decoded, isomorphic_decode, isomorphic_encode,
    split_header_value, split_header_value_str,
};
pub use referrer::{Referrer, ReferrerClient, ReferrerPolicy, determine_referrer};
pub use resolve::{InvalidResolveRule, ResolveRule};
pub use secure_context::is_potentially_trustworthy;
pub use transport::{InvalidCa, InvalidProxy, Proxy, TrustedCa};
